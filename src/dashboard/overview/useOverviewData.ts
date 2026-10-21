/**
 * 用量总览页取数 hook：并发拉取全部 Agent 的按日用量与额度快照（本地额度
 * 快照体系 + provider quota 通道），同步开启时再合并「其他设备」的用量
 * （数据范围见 OverviewScope）。
 *
 * 设计要点：
 *  - 本地 SQLite 型来源一次取满 365 天（热力图要铺满 52 周）；指标卡 /
 *    趋势图 / Agent 表仍按 90 天口径切片，Cursor 与远端合并维持 90 天窗口
 *    （见 model.ts 的窗口常量）——切换页头时间范围只重新聚合、不重新取数；
 *  - 与菜单栏标题同口径：不按 loadAgentVisibility 过滤（未安装的 CLI 静默
 *    回落 0，不占表行），本机 = 全部 USAGE_AGENT_IDS；「全部设备」再叠加
 *    远端其他设备（排除本机 device_id），成本一律按本机价格表折算；
 *  - 远端取数按 source 逐源请求（服务端 by_model 带 source、趋势桶不带，
 *    只能按来源分请求）；一律用 hour 桶再归到本地自然日——服务端 day 桶是
 *    UTC 零点对齐，非 UTC 时区会与「本地自然日」错位，无法与菜单栏对齐
 *    （见 model.addRemoteTrendDays 注释）；
 *  - 各 Agent 独立容错：「未安装 / 未登录」类错误静默按 0（不占表行、不进
 *    失败清单），其余错误只影响该 Agent 自己，其余照常渲染，失败清单在页面
 *    上以提示条列出；远端单源失败只是少了该源的远端贡献 + 置 remoteDegraded
 *    （弱提示条），不清空本机数据、不报错；
 *  - 额度快照固定近 8 天窗口（get_quota_history 是文件全量读，不该随页面
 *    范围放大），只取「当前」额度剩余，取不到则单元格回落 "--"；额度不合并
 *    远端（额度对比属于对比页的口径）。额度快照体系之外的 Agent（OpenCode
 *    Go / MiniMax Code）另走 provider quota 通道，映射为同一 QuotaInfo；
 *  - 双层缓存：最近一次完整结果同时存在模块变量与 localStorage（仅当天有效，
 *    跨天丢弃），再次进入页面（侧栏切页 / 窗口重开 / 进程重启）立即用缓存
 *    渲染、不出现加载态；缓存超过 60 秒才在后台静默刷新（不阻塞界面，完成后
 *    整份替换），手动刷新按钮永远强制重取；
 *  - 模型用量表的数据来源：各 Agent 日聚合的 by_model（本机 + 远端逐日已合并
 *    进 DayAgg.byModel）、Cursor 的范围内模型聚合旁路（cursorByModel，其 events
 *    无按日拆分）与本机价格表（pricing，统一折算模型成本，与远端合并共用）；
 *    时间范围切换同样是纯切片，不重新取数；
 *  - 一次性发布：本机数据、provider 额度与远端合并**全部就绪后才发布**，
 *    首屏要么骨架、要么完整数据，不再先出「仅本机」的较小数值、等远端到了
 *    再翻倍（用户可感知的数字从少到多跳变）。有上一次完整快照时先上屏旧值、
 *    后台静默刷新，完成后整份替换；请求序号守卫保证旧请求的结果不覆盖新数据；
 *  - 同步配置与本地取数并行发起，只有远端合并等待配置（避免「先按未启用同步
 *    的本机口径取一遍、拿到设备配置后又全量重取」的双取）；读取走两页共享
 *    缓存（../syncConfig，每次取数 refresh 强制重读），失败或超时按未启用
 *    同步处理，不卡首屏；
 *  - 数据范围（本机 / 全部设备）是纯视图切换：远端数据在同步开启时随加载
 *    一并取回，切范围不重新取数、不进入加载态（loading 只在首载与手动刷新
 *    出现）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AgentQuotaSource,
  AgentQuotaWindowKey,
  AgentQuotaSnapshot,
  PricingConfig,
  ProviderQuotaEntry,
  ProviderQuotaWindow,
  QuotaSnapshot,
  SyncConfig,
  TrendBucket,
} from "../../types";
import {
  fetchClaudeUsage,
  fetchCodexUsage,
  fetchCursorUsage,
  fetchDshUsage,
  fetchKimiUsage,
  fetchMinimaxcodeUsage,
  fetchOpencodeUsage,
  fetchPricing,
  fetchProviderQuota,
  fetchTrend,
  getAgentQuotaHistory,
  getCursorConfig,
  getQuotaHistory,
  remoteUsage,
} from "../../api";
import { remoteSourceOf } from "../compare/model";
import {
  peekSyncConfigCached,
  readSyncConfigCached,
} from "../syncConfig";
import {
  QUOTA_LOOKBACK_DAYS,
  REMOTE_WINDOW_DAYS,
  USAGE_AGENT_IDS,
  USAGE_WINDOW_DAYS,
  WINDOW_DAYS,
  addCursorDays,
  addLocalDays,
  addRemoteTrendDays,
  addTrendDays,
  agentLabel,
  buildDayKeys,
  loadOverviewScope,
  mergeDayMaps,
  saveOverviewScope,
  startOfLocalDay,
  type DayAgg,
  type OverviewScope,
  type UsageAgentId,
} from "./model";

/** 日桶粒度（趋势/热力图/指标卡共用；不随范围切换，避免重复取数） */
const TREND_BUCKET: TrendBucket = "day";

/** 模块级结果缓存：主窗口切页（条件渲染 = 卸载）后重进即刻渲染，无加载态。
 *  只存在于本窗口的 JS 模块内存里，不落盘；进程退出即失效。 */
let overviewCache: { data: OverviewRaw; at: number } | null = null;

/** 缓存新鲜期：超过则在后台静默刷新（不阻塞界面，完成后替换数据） */
const CACHE_FRESH_MS = 60_000;

// ===== 持久化快照（localStorage）=====
// 模块变量只能活到页面重载：主窗口 WebView 冷启动、开发态热重载都会清空它，
// 于是每次打开都要重新等「本机 1s + 远端 2s」两段数据，指标卡先小后大跳一次。
// 这里把上一次**完整**结果落盘，重开时同步恢复、直接按完整口径上屏；后台
// 刷新完成后整份替换。只接受同一天（windowStartMs 与今天对齐）的快照，跨天
// 直接丢弃——「今日」卡片与 24h 趋势不能拿昨天的数据冒充。
const OVERVIEW_CACHE_KEY = "zbar-dashboard-overview-cache";
const OVERVIEW_CACHE_VERSION = 1;
/** Map → 可 JSON 序列化的标记键（OverviewRaw 里 merged/quotas/byModel 都是 Map） */
const MAP_MARK = "__zbarMap";

function overviewReplacer(_key: string, value: unknown): unknown {
  return value instanceof Map ? { [MAP_MARK]: Array.from(value.entries()) } : value;
}

function overviewReviver(_key: string, value: unknown): unknown {
  if (
    value &&
    typeof value === "object" &&
    Array.isArray((value as Record<string, unknown>)[MAP_MARK])
  ) {
    return new Map(
      (value as Record<string, unknown>)[MAP_MARK] as [unknown, unknown][]
    );
  }
  return value;
}

/** 读取当天持久化快照；解析失败 / 结构不符 / 跨天一律返回 null（静默降级） */
function loadPersistedOverview(): { data: OverviewRaw; at: number } | null {
  try {
    const raw = localStorage.getItem(OVERVIEW_CACHE_KEY);
    if (!raw) return null;
    const saved = JSON.parse(raw, overviewReviver) as {
      version?: number;
      at?: number;
      data?: OverviewRaw;
    };
    if (
      saved?.version !== OVERVIEW_CACHE_VERSION ||
      typeof saved.at !== "number" ||
      !saved.data
    ) {
      return null;
    }
    const data = saved.data;
    const expectedStart = startOfLocalDay(
      addLocalDays(Date.now(), -(WINDOW_DAYS - 1))
    );
    if (
      data.windowStartMs !== expectedStart ||
      !Array.isArray(data.dayKeys) ||
      !(data.merged instanceof Map) ||
      !(data.quotas instanceof Map) ||
      !(data.cursorByModel instanceof Map) ||
      !Array.isArray(data.perAgent) ||
      !Array.isArray(data.remotePerAgent) ||
      !Array.isArray(data.failures)
    ) {
      return null;
    }
    return { data, at: saved.at };
  } catch {
    return null;
  }
}

/** 落盘完整快照；配额 / 序列化失败静默（缓存只是锦上添花） */
function savePersistedOverview(data: OverviewRaw, at: number): void {
  try {
    localStorage.setItem(
      OVERVIEW_CACHE_KEY,
      JSON.stringify(
        { version: OVERVIEW_CACHE_VERSION, at, data },
        overviewReplacer
      )
    );
  } catch {
    // 忽略：QuotaExceededError / 隐私模式 / 序列化失败
  }
}

/** 5 小时窗口剩余（无该窗口的 Agent 为 null） */
export interface Hour5Info {
  /** 剩余百分比 0-100（= 100 - 已用） */
  remainingPct: number;
  /** 下次重置时间（毫秒，可能为 null） */
  resetAt: number | null;
}

/** 月额度窗口剩余（provider 通道的 key="monthly" 窗口；快照体系没有月窗口，
 *  对应 Agent 的 QuotaInfo.monthly 恒为 null）。形状与 Hour5Info 一致。 */
export type MonthlyInfo = Hour5Info;

/** 单条额度结论（周额度剩余，Cursor 为账单周期） */
export interface QuotaInfo {
  /** 剩余百分比 0-100（= 100 - 已用） */
  remainingPct: number;
  /** 下次重置时间（毫秒，可能为 null） */
  resetAt: number | null;
  /** 窗口语义：weekly = 周额度；billing = 账单周期（Cursor） */
  window: "weekly" | "billing";
  /** 5 小时窗口剩余（快照 / provider quota 有该窗口时给出，否则 null） */
  hour5?: Hour5Info | null;
  /** 月额度窗口剩余（仅 provider quota 通道的 monthly 窗口，否则 null） */
  monthly: MonthlyInfo | null;
}

/** 单 Agent 的按日聚合（本机或远端一份） */
export interface AgentDays {
  id: UsageAgentId;
  days: Map<string, DayAgg>;
}

/** Cursor 范围内按模型的聚合（events 是「按请求范围整段」聚合，无按日拆分，
 *  故不并入 DayAgg.byModel，单独一条旁路给模型用量表）；成本用 Cursor 后端
 *  算好的 cost_usd（美元），人民币由展示层乘汇率折算，不重新按价格表折算。 */
export interface CursorModelAgg {
  requests: number;
  input: number;
  output: number;
  cacheRead: number;
  total: number;
  costUsd: number;
}

/** 单 Agent 本地取数结果：按日聚合 + Cursor 专有的范围内模型聚合旁路 */
interface AgentLocalLoad {
  days: Map<string, DayAgg>;
  /** 仅 Cursor 非空（其余 Agent 的 by_model 已逐日并入 days 的 DayAgg.byModel） */
  cursorModels: Map<string, CursorModelAgg>;
}

/** 页面一次性取数结果（范围/数据范围切换时只读它做切片聚合） */
export interface OverviewRaw {
  /** 取数窗口日键（365 天，升序，"MM-DD"，末位为今天）；指标卡/趋势/表格
   *  按 90 天切片、热力图取最近 52 周 */
  dayKeys: string[];
  /** 窗口起始日 0 点（热力图列对齐用） */
  windowStartMs: number;
  /** 本机跨 Agent 合并的按日聚合（缺失日期视为 0） */
  merged: Map<string, DayAgg>;
  /** 本机各 Agent 的按日聚合（Agent 表与透视用） */
  perAgent: AgentDays[];
  /** Cursor 范围内按模型聚合（Cursor events 口径，固定 90 天取数窗口；未登录 /
   *  取数失败为空表）——模型用量表把 Cursor 的模型数据从这里整份并入 */
  cursorByModel: Map<string, CursorModelAgg>;
  /** 远端（其他设备）各 Agent 的按日聚合；未启用同步 / 全部失败为空数组 */
  remotePerAgent: AgentDays[];
  /** 各 Agent 的当前额度剩余（取不到则无键；额度不合并远端） */
  quotas: Map<UsageAgentId, QuotaInfo>;
  /** USD→CNY 汇率（本机配置，用于 Cursor 与远端成本的按日人民币折算） */
  fxRate: number;
  /** 本机价格表（模型用量表折算模型成本用；读取失败为 null，成本按 0）。
   *  与远端合并共用同一份，不重复请求。 */
  pricing: PricingConfig | null;
  /** 取数失败的 Agent 展示名（部分失败提示用，不阻断其余数据） */
  failures: string[];
  /** 「全部设备」下远端单源取数失败（弱提示条用，已降级为本机口径） */
  remoteDegraded: boolean;
}

/** 一次完整发布的载荷：本机 + 远端 + provider 额度全部就绪后才发布 */
interface OverviewPublish {
  data: OverviewRaw;
  /** 全部 Agent 失败时的错误串（随唯一一次发布给出，否则 null） */
  fatalError: string | null;
}

export interface OverviewDataState {
  data: OverviewRaw | null;
  loading: boolean;
  /** 全部 Agent 都失败时的错误信息（部分失败走 failures 提示） */
  error: string | null;
  lastUpdatedAt: number | null;
  reload: () => void;
  /** 当前数据范围偏好（「全部设备」= 本机 + 其他设备） */
  scope: OverviewScope;
  setScope: (scope: OverviewScope) => void;
  /** 多设备同步是否已启用（未启用时范围控件禁用并脚注说明） */
  syncEnabled: boolean;
  /** 同步配置是否已读到（或已超时回退）：未就绪前不闪「未启用同步」提示 */
  syncReady: boolean;
  /** 远端（其他设备）数据仍在合并（页头弱标记用） */
  remotePending: boolean;
}

/** 错误信息截断（后端错误可能是长串中文提示，提示条只展示摘要） */
function shortError(error: unknown): string {
  const text = String(error).replace(/^Error:\s*/, "");
  return text.length > 120 ? text.slice(0, 117) + "…" : text;
}

/**
 * 「未安装 / 未登录 / 未使用过」类取数错误的识别。
 *
 * 正则匹配 Rust 后端返回的中文错误串（未找到 / 未安装 / 会话目录），只做布尔
 * 分支、不参与翻译——与面板 SummaryTab 的同款判定逐条对齐。总览按「全部
 * USAGE_AGENT_IDS 无条件取数」后，未安装的 CLI 会返回「未找到 xxx 会话目录」
 * 这类提示：它们不是取数失败，静默按 0 处理（不占表行、不进失败清单），
 * 否则每台机器都会常驻一条「部分 Agent 取数失败」的噪声提示。
 * 其余错误（数据库损坏、解析失败、Cursor 登录过期、网络失败）仍照常登记。
 */
const MISSING_INSTALL_RE = /未找到|未安装|会话目录/;

/** 该错误是否属于「未安装 / 未登录」的静默降级情形 */
function isMissingInstall(error: string | null): boolean {
  return error != null && MISSING_INSTALL_RE.test(error);
}

/** 单任务容错：失败返回 error 而不抛，保证 Promise.all 整体不中断 */
async function settle<T>(task: () => Promise<T>): Promise<{ value: T | null; error: string | null }> {
  try {
    return { value: await task(), error: null };
  } catch (e) {
    return { value: null, error: shortError(e) };
  }
}

/**
 * 单 Agent 的本地按日用量（各 usage 命令的 trend 已由后端按定价折算成本）。
 * 窗口：SQLite 型来源取满 365 天（热力图 52 周需要一年，单查询成本低）；
 * Cursor 维持 90 天（events 全年分页拉取太慢，90 天之前的热力格子 = 纯本机
 * 口径中不含 Cursor 的部分）。
 *
 * 除按日聚合外，Cursor 额外带出范围内按模型聚合（其 events 无按日拆分），
 * 供模型用量表使用，见 AgentLocalLoad。
 */
async function loadAgentDays(
  id: UsageAgentId,
  nowMs: number,
  fxPromise: Promise<number>
): Promise<AgentLocalLoad> {
  const days = new Map<string, DayAgg>();
  const cursorModels = new Map<string, CursorModelAgg>();
  const toMs = nowMs;
  const windowDays = id === "cursor" ? USAGE_WINDOW_DAYS : WINDOW_DAYS;
  const fromMs = startOfLocalDay(addLocalDays(nowMs, -(windowDays - 1)));
  switch (id) {
    case "zai": {
      // ZCode 主统计：trend 即按日聚合（与 get_stats 同库同口径）
      addTrendDays(await fetchTrend(fromMs, toMs, TREND_BUCKET), days);
      break;
    }
    case "codex": {
      addTrendDays((await fetchCodexUsage(fromMs, toMs, TREND_BUCKET)).trend, days);
      break;
    }
    case "claude": {
      addTrendDays((await fetchClaudeUsage(fromMs, toMs, TREND_BUCKET)).trend, days);
      break;
    }
    case "kimi": {
      addTrendDays((await fetchKimiUsage(fromMs, toMs, TREND_BUCKET)).trend, days);
      break;
    }
    case "dsh": {
      addTrendDays((await fetchDshUsage(fromMs, toMs, TREND_BUCKET)).trend, days);
      break;
    }
    case "opencodego": {
      // 本地取数命令是 get_opencode_usage（模块名 opencode 是后端来源名）
      addTrendDays((await fetchOpencodeUsage(fromMs, toMs, TREND_BUCKET)).trend, days);
      break;
    }
    case "minimax": {
      // 本地取数命令是 get_minimaxcode_usage（后端来源名 minimaxcode）
      addTrendDays((await fetchMinimaxcodeUsage(fromMs, toMs, TREND_BUCKET)).trend, days);
      break;
    }
    case "cursor": {
      // Cursor 走独立快照：daily 为按日明细，事件按请求范围后端已切片；
      // 人民币折算需要汇率，等同一份已发起的汇率读取（不额外串行）。
      // by_model 是「整段范围」聚合（无按日拆分），单独带出给模型用量表。
      const snapshot = await fetchCursorUsage(fromMs, toMs);
      addCursorDays(snapshot.daily, await fxPromise, days);
      for (const model of snapshot.by_model) {
        cursorModels.set(model.model, {
          requests: model.requests,
          input: model.input_tokens,
          output: model.output_tokens,
          cacheRead: model.cache_read_tokens,
          total: model.total_tokens,
          costUsd: model.cost_usd,
        });
      }
      break;
    }
  }
  return { days, cursorModels };
}

/**
 * 单 Agent 的远端（其他设备）按日用量。
 * 来源名沿用对比页的域映射（remoteSourceOf：zai→zcode、opencodego→opencode、
 * minimax→minimaxcode，其余同名）；未登记远端来源的 Agent（Cursor 明细不上传）
 * 返回 null，调用方直接跳过。
 * 窗口固定 90 天（远端 hour 桶 × 全年体积太大）：更早的格子为纯本机口径。
 * 价格表由调用方统一读取后传入（与模型用量表共用同一份，不重复请求）。
 */
async function loadRemoteAgentDays(
  id: UsageAgentId,
  fromMs: number,
  toMs: number,
  excludeDevice: string,
  pricing: PricingConfig | null,
  fxRate: number,
  windowKeys: Set<string>
): Promise<Map<string, DayAgg> | null> {
  const source = remoteSourceOf(id);
  if (!source) return null;
  const remote = await remoteUsage(fromMs, toMs, "hour", {
    excludeDevice,
    source,
  });
  const days = new Map<string, DayAgg>();
  addRemoteTrendDays(remote, pricing, fxRate, windowKeys, days);
  return days;
}

function clampPct(v: number): number {
  return Math.min(100, Math.max(0, v));
}

/** 本机 ZCode 额度快照 → 最新一条有效周额度 + 5 小时窗口（仅当前账号视角） */
function zaiQuotaFrom(history: QuotaSnapshot[]): QuotaInfo | null {
  let latest: QuotaSnapshot | null = null;
  for (const snapshot of history) {
    if (!Number.isFinite(snapshot.weekly_pct)) continue;
    if (!latest || snapshot.ts > latest.ts) latest = snapshot;
  }
  if (!latest) return null;
  return {
    remainingPct: clampPct(100 - latest.weekly_pct),
    resetAt: latest.weekly_reset ?? null,
    window: "weekly",
    // 5 小时窗口顺带记录在快照里（hour5_pct），无重置时刻字段
    hour5: Number.isFinite(latest.hour5_pct)
      ? { remainingPct: clampPct(100 - latest.hour5_pct), resetAt: null }
      : null,
    // 快照体系没有月窗口
    monthly: null,
  };
}

/** 各 Agent 额度窗口键：Cursor 是账单周期（cursor_auto），其余为周窗口 */
const AGENT_QUOTA_WINDOW: Record<AgentQuotaSource, AgentQuotaWindowKey> = {
  codex: "weekly",
  claude: "weekly",
  kimi: "weekly",
  cursor: "cursor_auto",
};

/** 快照窗口 → 5 小时剩余（无该窗口 / 百分比非法为 null） */
function hour5From(snapshot: AgentQuotaSnapshot): Hour5Info | null {
  const window = snapshot.windows.find((w) => w.key === "hour5");
  if (!window || !Number.isFinite(window.used_pct)) return null;
  return {
    remainingPct: clampPct(100 - window.used_pct),
    resetAt: window.reset_at ?? null,
  };
}

/** Codex / Claude / Cursor / Kimi 额度快照 → 各来源最新一条有效窗口 */
function agentQuotasFrom(
  snapshots: AgentQuotaSnapshot[]
): Map<UsageAgentId, QuotaInfo> {
  const latestBySource = new Map<AgentQuotaSource, AgentQuotaSnapshot>();
  for (const snapshot of snapshots) {
    const previous = latestBySource.get(snapshot.source);
    if (!previous || snapshot.ts > previous.ts) latestBySource.set(snapshot.source, snapshot);
  }
  const out = new Map<UsageAgentId, QuotaInfo>();
  for (const [source, snapshot] of latestBySource) {
    const windowKey = AGENT_QUOTA_WINDOW[source];
    const window = snapshot.windows.find((w) => w.key === windowKey);
    if (!window || !Number.isFinite(window.used_pct)) continue;
    out.set(source, {
      remainingPct: clampPct(100 - window.used_pct),
      resetAt: window.reset_at ?? null,
      window: source === "cursor" ? "billing" : "weekly",
      hour5: hour5From(snapshot),
      // 快照体系没有月窗口
      monthly: null,
    });
  }
  return out;
}

// ===== provider quota 通道（额度不在快照体系内的 Agent）=====

/**
 * 需要走 get_provider_quota 取额度的 Agent → provider id。
 *
 * 与窄窗各面板逐一对齐：
 *  - opencodego：OpencodePanel 的 Go 套餐额度走 providerQuota["opencodego"]；
 *  - minimax：MinimaxPanel 的 MiniMax Code 套餐额度走 providerQuota["minimax"]；
 *  - kimi：快照体系已有 weekly（agent_quota_history），按「快照优先」不接
 *    provider 通道（provider 通道只覆盖「其他账号」，与主面板额度的本地
 *    登录态不是同一份）；
 *  - dsh：DSH 没有订阅额度窗口（DeepSeek 平台余额是另一条凭证链路，且只有
 *    金额没有剩余百分比），额度列保持 "--"。
 */
const PROVIDER_QUOTA_OF: Partial<Record<UsageAgentId, string>> = {
  opencodego: "opencodego",
  minimax: "minimax",
};

/** provider 额度主窗口的键优先级：额度列是「周额度剩余」，优先取周窗口 */
const PROVIDER_WINDOW_PRIORITY = ["weekly", "monthly"] as const;

/** provider 窗口 → 统一剩余结构（百分比非法为 null）；5 小时与月窗口共用 */
function providerWindowInfo(
  window: ProviderQuotaWindow | undefined
): Hour5Info | null {
  if (!window || window.usedPercent == null || !Number.isFinite(window.usedPercent)) {
    return null;
  }
  return {
    remainingPct: clampPct(100 - window.usedPercent),
    resetAt: window.resetsAt ?? null,
  };
}

/**
 * provider 额度条目 → 统一 QuotaInfo（窄窗 QuotaEntryCard 的展示取舍的
 * 单值版本）：只消费正常态条目（error / expired 条目没有可用数值），
 * 主窗口优先取 weekly（本列语义）→ monthly → 首个带重置时刻 → 首个有百分比的
 * 窗口；5 小时窗口按 key 取（opencodego 的 "hour5"；minimax 的 "interval"
 * 就是 5 小时会话窗口，见 minimax.rs 的窗口映射注释）；月窗口按 key="monthly"
 * 取（opencodego 三窗口之一），无该窗口为 null。
 */
function providerQuotaFrom(entries: ProviderQuotaEntry[]): QuotaInfo | null {
  const withPercent = (entry: ProviderQuotaEntry): ProviderQuotaWindow[] =>
    entry.windows.filter(
      (w) => w.usedPercent != null && Number.isFinite(w.usedPercent)
    );
  const entry = entries.find(
    (item) =>
      item.status !== "error" && item.status !== "expired" && withPercent(item).length > 0
  );
  if (!entry) return null;
  const windows = withPercent(entry);
  let main: ProviderQuotaWindow | undefined;
  for (const key of PROVIDER_WINDOW_PRIORITY) {
    main = windows.find((w) => w.key === key);
    if (main) break;
  }
  main ??= windows.find((w) => w.resetsAt != null) ?? windows[0];
  if (!main || main.usedPercent == null) return null;
  const hour5Window =
    entry.windows.find((w) => w.key === "hour5") ??
    entry.windows.find((w) => w.key === "interval");
  return {
    remainingPct: clampPct(100 - main.usedPercent),
    resetAt: main.resetsAt ?? null,
    window: "weekly",
    hour5: providerWindowInfo(hour5Window),
    // 月窗口（opencodego 的 key="monthly"）：额度列第二行展示；无该窗口为 null
    monthly: providerWindowInfo(
      entry.windows.find((w) => w.key === "monthly")
    ),
  };
}

/** 拉取全部 provider 额度通道的结果（单 provider 失败静默：额度列回落 "--"） */
async function loadProviderQuotas(): Promise<Map<UsageAgentId, QuotaInfo>> {
  const out = new Map<UsageAgentId, QuotaInfo>();
  const providers = Object.entries(PROVIDER_QUOTA_OF) as [UsageAgentId, string][];
  const results = await Promise.all(
    providers.map(async ([id, provider]) => ({
      id,
      ...(await settle(() => fetchProviderQuota(provider))),
    }))
  );
  for (const { id, value } of results) {
    if (!value) continue;
    const quota = providerQuotaFrom(value);
    if (quota) out.set(id, quota);
  }
  return out;
}

// ===== 取数主流程 =====

/**
 * 拉取全部数据并在**全部就绪后一次性发布**。
 *
 * - 本机阶段：SQLite 日桶 + 额度快照 + 汇率并行取回；任一 Agent 失败只登记
 *   failures，全部失败时通过 fatalError 交给页面展示错误态；
 * - 并行补充：provider 额度通道（本地库 / 官方接口，可能慢）与远端其他设备
 *   用量合并（需要同步配置）。两者都结束后才 publish 完整口径——不发布
 *   「仅本机」中间态，避免指标卡 / 额度从少到多跳一次；
 * - 首屏速度由调用方的持久化快照兜底：有上次完整结果时先上屏旧值，这里只做
 *   后台刷新；没有快照时才显示骨架。
 */
async function loadOverview(
  syncPromise: Promise<SyncConfig | null>,
  publish: (payload: OverviewPublish) => void
): Promise<void> {
  const now = Date.now();
  const toMs = now;
  const windowStartMs = startOfLocalDay(addLocalDays(now, -(WINDOW_DAYS - 1)));
  const dayKeys = buildDayKeys(windowStartMs, WINDOW_DAYS);
  // 指标卡 / 趋势 / 表格与远端合并共用的 90 天切片（口径与改动前一致）
  const usageKeys = dayKeys.slice(-USAGE_WINDOW_DAYS);
  const usageKeySet = new Set(usageKeys);
  const remoteStartMs = startOfLocalDay(
    addLocalDays(now, -(REMOTE_WINDOW_DAYS - 1))
  );

  // 汇率仅用于 Cursor / 远端成本的按日人民币折算（其余来源的成本由后端算好）
  const fxPromise = getCursorConfig()
    .then((config) => (config.usd_cny_rate > 0 ? config.usd_cny_rate : 7.2))
    .catch(() => 7.2);

  // 价格表：模型用量表要按模型折算本机 + 远端成本（与菜单栏标题 / 面板汇总页
  // 同款口径），远端合并也复用同一份，因此提到主流程统一读取（失败按 null，
  // 成本列回落 0，token 照常统计）。读取是本地文件命令，与前述请求并行发起。
  const pricingPromise = fetchPricing().catch(() => null as PricingConfig | null);

  // 额度快照（Z.ai 与 Codex/Claude/Kimi/Cursor）与用量取数互相独立，同时发起
  const quotaFromMs = startOfLocalDay(addLocalDays(now, -QUOTA_LOOKBACK_DAYS));
  const zaiQuotaPromise = settle(() => getQuotaHistory(false, quotaFromMs));
  const agentQuotaPromise = settle(() => getAgentQuotaHistory(quotaFromMs, now));
  const providerQuotaPromise = loadProviderQuotas();

  // 各 Agent 独立容错：先并发取回结果，再按 USAGE_AGENT_IDS 的固定顺序消费——
  // 直接在并发回调里 push（失败时才 push）会让 failures 顺序随完成先后漂移，
  // 提示条里的 Agent 名单每次刷新都不一样。
  // 「未安装 / 未登录」类错误静默跳过（按 0 处理，不占行、不进失败清单）：
  // 该 Agent 若在其他设备有远端用量，仍会由「全部设备」视图从远端侧补上该行。
  const results = await Promise.all(
    USAGE_AGENT_IDS.map(async (id) => ({
      id,
      ...(await settle(() => loadAgentDays(id, now, fxPromise))),
    }))
  );
  const perAgent: AgentDays[] = [];
  const failures: string[] = [];
  let firstError: string | null = null;
  // Cursor 的模型聚合旁路：只有 Cursor 会填充（其 events 无按日拆分）
  const cursorByModel = new Map<string, CursorModelAgg>();
  for (const { id, value, error } of results) {
    if (value) {
      perAgent.push({ id, days: value.days });
      for (const [modelId, agg] of value.cursorModels) {
        cursorByModel.set(modelId, agg);
      }
    } else if (!isMissingInstall(error)) {
      failures.push(agentLabel(id));
      firstError ??= error;
    }
  }

  // 额度快照：失败静默，单元格回落 "--"
  const [zaiHistory, agentHistory] = await Promise.all([
    zaiQuotaPromise,
    agentQuotaPromise,
  ]);
  const quotas = new Map<UsageAgentId, QuotaInfo>();
  if (zaiHistory.value) {
    const zaiQuota = zaiQuotaFrom(zaiHistory.value);
    if (zaiQuota) quotas.set("zai", zaiQuota);
  }
  if (agentHistory.value) {
    for (const [id, quota] of agentQuotasFrom(agentHistory.value)) {
      quotas.set(id, quota);
    }
  }

  // 排序稳定：按 USAGE_AGENT_IDS 的既定顺序输出（表格自身再按用量降序）
  const orderOf = (id: UsageAgentId) => USAGE_AGENT_IDS.indexOf(id);
  perAgent.sort((a, b) => orderOf(a.id) - orderOf(b.id));

  const fxRate = await fxPromise;
  const pricing = await pricingPromise;
  const fatalError =
    USAGE_AGENT_IDS.length > 0 && perAgent.length === 0 && failures.length > 0
      ? firstError ?? "数据加载失败"
      : null;

  // 累加状态：provider / 远端两个任务各自增补字段，全部结束后只发布一次
  const state: OverviewRaw = {
    dayKeys,
    windowStartMs,
    merged: mergeDayMaps(perAgent.map((entry) => entry.days)),
    perAgent,
    cursorByModel,
    remotePerAgent: [],
    quotas,
    fxRate,
    pricing,
    failures,
    remoteDegraded: false,
  };

  // 全部 Agent 失败：没有任何可展示数据，直接以最终错误态发布（不再等第二阶段）
  if (fatalError) {
    publish({ data: { ...state }, fatalError });
    return;
  }

  // provider 额度与远端合并并行；两者都结束后才发布完整口径
  const providerTask = (async () => {
    const providerQuotas = await providerQuotaPromise;
    if (providerQuotas.size === 0) return;
    const next = new Map(state.quotas);
    let changed = false;
    for (const [id, quota] of providerQuotas) {
      // 快照体系已有该 Agent（Kimi）时快照优先，provider 通道不覆盖
      if (next.has(id)) continue;
      next.set(id, quota);
      changed = true;
    }
    if (!changed) return;
    state.quotas = next;
  })();

  const remoteTask = (async () => {
    const config = await syncPromise;
    const syncEnabled = !!config?.enabled && !!config.device_token;
    if (!syncEnabled) return;
    const excludeDevice = config?.device_id ?? "";
    const remoteResults = await Promise.all(
      USAGE_AGENT_IDS.map(async (id) => ({
        id,
        ...(await settle(() =>
          loadRemoteAgentDays(
            id,
            remoteStartMs,
            toMs,
            excludeDevice,
            pricing,
            fxRate,
            usageKeySet
          )
        )),
      }))
    );
    // 远端结果：只有真正带来数据的 Agent 才登记（远端无该来源数据时不算失败，
    // Cursor 未登记远端来源直接跳过）；任一源请求失败即置 remoteDegraded
    const remotePerAgent: AgentDays[] = [];
    let remoteDegraded = false;
    for (const { id, value, error } of remoteResults) {
      if (value == null) {
        // 请求失败（error 有值）→ 降级弱提示；未登记远端来源的 Agent（Cursor）
        // 返回 null 且无错误，不是失败
        if (error) remoteDegraded = true;
        continue;
      }
      if (value.size > 0) remotePerAgent.push({ id, days: value });
    }
    remotePerAgent.sort((a, b) => orderOf(a.id) - orderOf(b.id));
    state.remotePerAgent = remotePerAgent;
    state.remoteDegraded = remoteDegraded;
  })();

  await Promise.allSettled([providerTask, remoteTask]);
  publish({ data: { ...state }, fatalError: null });
}

export function useOverviewData(): OverviewDataState {
  // 持久化快照只读一次：首帧即按上次完整口径渲染（无骨架、无「先少后多」）；
  // 模块缓存还在时直接复用、不重复解析 localStorage；跨天 / 结构不符时
  // loadPersistedOverview 返回 null，退回正常首载流程
  const [persisted] = useState(() => overviewCache ?? loadPersistedOverview());
  const [data, setData] = useState<OverviewRaw | null>(
    persisted?.data ?? null
  );
  const [loading, setLoading] = useState(persisted == null);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(
    persisted?.at ?? null
  );
  const [remotePending, setRemotePending] = useState(false);
  // data 的 ref 镜像：取数 effect 只依赖 nonce，判断「当前是否已有内容可显示」
  // 必须读最新值（手动刷新时 data 仍在，不能误判为首载而切回骨架）
  const dataRef = useRef(data);
  dataRef.current = data;
  // 同步配置初值取共享模块级记忆（本次会话上次读到的有效配置，两页共用）：
  // 重进页面时不闪「未启用同步」、范围控件不短暂禁用；随后仍会重读刷新
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(
    () => peekSyncConfigCached()
  );
  const [syncReady, setSyncReady] = useState(
    () => peekSyncConfigCached() != null
  );
  const [nonce, setNonce] = useState(0);
  const [scope, setScopeState] = useState<OverviewScope>(() => loadOverviewScope());
  // 请求序号：并发触发时只应用最新一次请求的结果
  const reqIdRef = useRef(0);
  // 手动刷新标记：手动刷新永远强制重取（跳过缓存），并让刷新按钮转圈
  const manualRef = useRef(false);

  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // 取数：范围 / 数据范围切换不进入这里（纯视图切片），只有首载与手动刷新
  // 才重新取数；命中且未过期的缓存时完全不取数（同步配置仍重读一次）。
  useEffect(() => {
    const reqId = ++reqIdRef.current;
    let cancelled = false;
    const isCurrent = () => !cancelled && reqId === reqIdRef.current;
    const manual = manualRef.current;
    manualRef.current = false;

    // 缓存命中：立即用旧数据渲染（无加载态）。模块缓存优先，其次持久化快照
    const cached = manual ? null : (overviewCache ?? persisted);
    if (cached) {
      setData(cached.data);
      setLastUpdatedAt(cached.at);
      setError(null);
    }
    // 已有内容（缓存或当前 state）时刷新只静默替换，绝不退回骨架
    const hasVisible = dataRef.current != null || cached != null;

    // 同步配置与本地取数并行发起：仅远端合并等待它（读取失败 / 超时在共享
    // 读取内部按「未启用同步」处理，本地文件读通常毫秒级，超时兜底不卡首屏）。
    // 命中缓存也强制重读（refresh）：面板里刚改过同步开关的话，范围控件
    // 不该停在旧状态。同一份 promise 复用给远端合并，保证状态更新与远端
    // 等待看到同一次读取结果（不重复发 IPC）
    const syncRead = readSyncConfigCached({ refresh: true });
    syncRead.then((config) => {
      if (!isCurrent()) return;
      setSyncConfig(config);
      setSyncReady(true);
    });

    // 缓存还在新鲜期内：不取数，只让上面的配置读取跑完（切页/重进即刻渲染）
    if (cached && !manual && Date.now() - cached.at < CACHE_FRESH_MS) {
      setLoading(false);
      setRemotePending(false);
      return () => {
        cancelled = true;
      };
    }

    // 无任何可显示数据时才有骨架；手动刷新要转圈，但已有内容不遮挡
    if (!hasVisible || manual) setLoading(true);
    setRemotePending(true);

    (async () => {
      try {
        await loadOverview(syncRead, (payload) => {
          // 序号守卫：旧请求的结果一律丢弃
          if (!isCurrent()) return;
          setError(payload.fatalError);
          // 全失败时不要用空壳覆盖已有缓存（保留上次完整数据，只挂错误提示）
          if (payload.fatalError) {
            if (dataRef.current == null) setData(payload.data);
          } else {
            setData(payload.data);
            const at = Date.now();
            setLastUpdatedAt(at);
            overviewCache = { data: payload.data, at };
            savePersistedOverview(payload.data, at);
          }
          setRemotePending(false);
          setLoading(false);
        });
      } catch (e) {
        if (isCurrent()) {
          setError(shortError(e));
          setRemotePending(false);
          setLoading(false);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
    // persisted 在组件生命周期内不变，无需列入依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nonce]);

  const reload = useCallback(() => {
    manualRef.current = true;
    setNonce((n) => n + 1);
  }, []);

  // 数据范围切换是纯视图选择（远端数据随加载一并取回），只持久化偏好
  const setScope = useCallback((next: OverviewScope) => {
    setScopeState(next);
    saveOverviewScope(next);
  }, []);

  return {
    data,
    loading,
    error,
    lastUpdatedAt,
    reload,
    scope,
    setScope,
    syncEnabled,
    syncReady,
    remotePending,
  };
}
