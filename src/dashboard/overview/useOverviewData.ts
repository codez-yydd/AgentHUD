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
 *  - 模块级缓存：最近一次完整结果存在模块顶层变量里，再次进入页面（侧栏切页
 *    = 条件渲染 = 卸载重挂）立即用缓存渲染、不出现加载态；缓存超过 60 秒才在
 *    后台静默刷新（不阻塞界面，完成后替换数据），手动刷新按钮永远强制重取；
 *  - 渐进渲染：本机数据（SQLite 日桶 + 额度快照文件）就绪即发布首屏（指标卡 /
 *    趋势 / 表格立即可见），远端合并与 provider 额度随后各自就绪即二次更新
 *    （期间页头弱标记「远端合并中」）；请求序号守卫保证旧请求的结果不覆盖新
 *    数据，且各阶段都基于同一份累加状态发布，远端结果不会回退本地数据；
 *  - 同步配置与本地取数并行发起，只有远端合并等待配置（避免「先按未启用同步
 *    的本机口径取一遍、拿到设备配置后又全量重取」的双取）；getSyncConfig 失败
 *    或超时（SYNC_CONFIG_TIMEOUT_MS）按未启用同步处理，不卡首屏；
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
  getSyncConfig,
  remoteUsage,
} from "../../api";
import { remoteSourceOf } from "../compare/model";
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

/** 同步配置等待上限：超时按「未启用同步」回退本机口径单次加载（不卡首屏） */
const SYNC_CONFIG_TIMEOUT_MS = 5000;

/** 模块级结果缓存：主窗口切页（条件渲染 = 卸载）后重进即刻渲染，无加载态。
 *  只存在于本窗口的 JS 模块内存里，不落盘；进程退出即失效。 */
let overviewCache: { data: OverviewRaw; at: number } | null = null;

/** 缓存新鲜期：超过则在后台静默刷新（不阻塞界面，完成后替换数据） */
const CACHE_FRESH_MS = 60_000;

/** 最近一次读到的同步配置（模块级记忆）：重进页面时先按上次配置渲染，
 *  范围控件不会因「缓存命中、配置尚未重读」而短暂禁用/回落本机口径。 */
let cachedSyncConfig: SyncConfig | null = null;

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
  /** 远端（其他设备）各 Agent 的按日聚合；未启用同步 / 全部失败为空数组 */
  remotePerAgent: AgentDays[];
  /** 各 Agent 的当前额度剩余（取不到则无键；额度不合并远端） */
  quotas: Map<UsageAgentId, QuotaInfo>;
  /** USD→CNY 汇率（本机配置，用于 Cursor 与远端成本的按日人民币折算） */
  fxRate: number;
  /** 取数失败的 Agent 展示名（部分失败提示用，不阻断其余数据） */
  failures: string[];
  /** 「全部设备」下远端单源取数失败（弱提示条用，已降级为本机口径） */
  remoteDegraded: boolean;
}

/** 单次发布的载荷：阶段一（本机）与后续更新共用同一形状 */
interface OverviewPublish {
  data: OverviewRaw;
  /** 本次取数是否已全部完成（写缓存 + 结束加载态） */
  final: boolean;
  /** 全部 Agent 失败时的错误串（随阶段一给出，否则 null） */
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
 */
async function loadAgentDays(
  id: UsageAgentId,
  nowMs: number,
  fxPromise: Promise<number>
): Promise<Map<string, DayAgg>> {
  const days = new Map<string, DayAgg>();
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
      // 人民币折算需要汇率，等同一份已发起的汇率读取（不额外串行）
      const snapshot = await fetchCursorUsage(fromMs, toMs);
      addCursorDays(snapshot.daily, await fxPromise, days);
      break;
    }
  }
  return days;
}

/**
 * 单 Agent 的远端（其他设备）按日用量。
 * 来源名沿用对比页的域映射（remoteSourceOf：zai→zcode、opencodego→opencode、
 * minimax→minimaxcode，其余同名）；未登记远端来源的 Agent（Cursor 明细不上传）
 * 返回 null，调用方直接跳过。
 * 窗口固定 90 天（远端 hour 桶 × 全年体积太大）：更早的格子为纯本机口径。
 */
async function loadRemoteAgentDays(
  id: UsageAgentId,
  fromMs: number,
  toMs: number,
  excludeDevice: string,
  pricingPromise: Promise<PricingConfig | null>,
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
  addRemoteTrendDays(remote, await pricingPromise, fxRate, windowKeys, days);
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
 * 拉取全部数据并按「本机 → 远端 / provider 额度」两段发布。
 *
 * - 阶段一（本机）：SQLite 日桶 + 额度快照 + 汇率并行取回，就绪即 publish
 *   （首屏不等远端）；任一 Agent 失败只登记 failures，全部失败时通过
 *   fatalError 交给页面展示错误态；
 * - 阶段二（并行两路，各自就绪即再发布）：provider 额度通道（本地库 / 官方
 *   接口，可能慢）与远端其他设备用量合并（需要同步配置，与阶段一并行发起）。
 *   各阶段都基于同一份累加状态发布，后到的结果只增补字段，不会回退本地数据。
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
  for (const { id, value, error } of results) {
    if (value) perAgent.push({ id, days: value });
    else if (!isMissingInstall(error)) {
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
  const fatalError =
    USAGE_AGENT_IDS.length > 0 && perAgent.length === 0 && failures.length > 0
      ? firstError ?? "数据加载失败"
      : null;

  // 累加状态：各阶段发布同一对象的新快照，后到阶段只增补字段
  const state: OverviewRaw = {
    dayKeys,
    windowStartMs,
    merged: mergeDayMaps(perAgent.map((entry) => entry.days)),
    perAgent,
    remotePerAgent: [],
    quotas,
    fxRate,
    failures,
    remoteDegraded: false,
  };

  // 阶段一：本机数据就绪即发布（首屏不等远端 / provider 额度）
  publish({ data: { ...state }, final: false, fatalError });
  if (fatalError) return;

  // 阶段二：provider 额度与远端合并并行，各自就绪即二次发布
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
    publish({ data: { ...state }, final: false, fatalError: null });
  })();

  const remoteTask = (async () => {
    const config = await syncPromise;
    const syncEnabled = !!config?.enabled && !!config.device_token;
    if (!syncEnabled) return;
    const excludeDevice = config?.device_id ?? "";
    // 价格表：远端不下发成本，用本机价格表按模型折算（与菜单栏标题同款）；
    // 读不到时远端成本按 0（token 照常统计）。与远端请求并行发起。
    const pricingPromise = fetchPricing().catch(() => null as PricingConfig | null);
    const remoteResults = await Promise.all(
      USAGE_AGENT_IDS.map(async (id) => ({
        id,
        ...(await settle(() =>
          loadRemoteAgentDays(
            id,
            remoteStartMs,
            toMs,
            excludeDevice,
            pricingPromise,
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
    publish({ data: { ...state }, final: false, fatalError: null });
  })();

  await Promise.allSettled([providerTask, remoteTask]);
  publish({ data: { ...state }, final: true, fatalError: null });
}

export function useOverviewData(): OverviewDataState {
  const [data, setData] = useState<OverviewRaw | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(null);
  const [remotePending, setRemotePending] = useState(false);
  // 同步配置初值取模块级记忆（本次会话上次读到的值）：重进页面时不闪
  // 「未启用同步」、范围控件不短暂禁用；随后仍会重读刷新
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(
    () => cachedSyncConfig
  );
  const [syncReady, setSyncReady] = useState(() => cachedSyncConfig != null);
  const [nonce, setNonce] = useState(0);
  const [scope, setScopeState] = useState<OverviewScope>(() => loadOverviewScope());
  // 请求序号：并发触发 / 阶段式发布时只应用最新一次请求的结果
  const reqIdRef = useRef(0);
  // 手动刷新标记：手动刷新永远强制重取（跳过缓存），并让刷新按钮转圈
  const manualRef = useRef(false);

  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // 取数：范围 / 数据范围切换不进入这里（纯视图切片），只有首载与手动刷新
  // 才重新取数；命中且未过期的模块级缓存时完全不取数（同步配置仍重读一次）。
  useEffect(() => {
    const reqId = ++reqIdRef.current;
    let cancelled = false;
    let syncTimer: number | null = null;
    const isCurrent = () => !cancelled && reqId === reqIdRef.current;
    const manual = manualRef.current;
    manualRef.current = false;

    // 缓存命中：立即用旧数据渲染（无加载态）
    const cached = manual ? null : overviewCache;
    if (cached) {
      setData(cached.data);
      setLastUpdatedAt(cached.at);
      setError(null);
    }

    // 同步配置与本地取数并行发起：仅远端合并等待它（读取失败 / 超时按
    // 「未启用同步」处理，本地文件读通常毫秒级，超时兜底不卡首屏）。
    // 命中新鲜缓存时也重读：面板里刚改过同步开关的话，范围控件不该停在旧状态。
    const syncPromise = Promise.race([
      getSyncConfig().catch(() => null),
      new Promise<null>((resolve) => {
        syncTimer = window.setTimeout(() => resolve(null), SYNC_CONFIG_TIMEOUT_MS);
      }),
    ]);
    const disposeSyncTimer = () => {
      if (syncTimer != null) window.clearTimeout(syncTimer);
    };
    syncPromise.then((config) => {
      disposeSyncTimer();
      cachedSyncConfig = config;
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
        disposeSyncTimer();
      };
    }

    // 首载或手动刷新才有加载态；有旧数据（缓存过期）时静默刷新，不阻塞界面
    if (!cached || manual) setLoading(true);
    setRemotePending(true);

    (async () => {
      try {
        await loadOverview(syncPromise, (payload) => {
          // 序号守卫：旧请求（含旧请求的后续阶段）的结果一律丢弃
          if (!isCurrent()) return;
          setData(payload.data);
          setError(payload.fatalError);
          // 仅在有可用数据时更新「上次刷新」时刻：全失败时显示旧时刻会误导
          if (!payload.fatalError) setLastUpdatedAt(Date.now());
          if (payload.final) {
            overviewCache = { data: payload.data, at: Date.now() };
            setRemotePending(false);
            setLoading(false);
          }
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
      disposeSyncTimer();
    };
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
