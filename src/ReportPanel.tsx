import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type RefObject,
} from "react";
import type {
  ClaudeSnapshot,
  CodexSnapshot,
  Currency,
  CursorSnapshot,
  DeviceInfo,
  DshSnapshot,
  KimiSnapshot,
  MinimaxcodeSnapshot,
  OpencodeSnapshot,
  PricingConfig,
  RemoteUsage,
  Stats,
  SyncConfig,
  TrendBucket,
  TrendPoint,
} from "./types";
import {
  fetchClaudeUsage,
  fetchCodexUsage,
  fetchCursorUsage,
  fetchDshUsage,
  fetchKimiUsage,
  fetchMinimaxcodeUsage,
  fetchOpencodeUsage,
  fetchStats,
  fetchTrend,
  getCursorConfig,
  getSyncConfig,
  listRemoteDevices,
  remoteUsage,
  saveReport,
} from "./api";
import {
  formatCountLocale,
  formatCost,
  formatTokens,
  formatTokensLocale,
  formatTokensParts,
} from "./format";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  mergeByModel,
  mergeStats,
  mergeTrend,
  modelCost,
  remoteToStats,
} from "./merge";
import { BrandIcon, type BrandIconName } from "./BrandIcon";
import {
  displayModelName,
  foldCursorModelRows,
  foldModelStatRows,
  hasPositivePrice,
} from "./modelName";
import {
  AGENT_COLOR,
  sourceMeta,
  type AgentVisibility,
} from "./agentVisibility";
import {
  PageBody,
  SectionCard,
  BtnPrimary,
  BtnSecondary,
  AlertBanner,
  LoadingState,
} from "./layout";
import { ShareCardModal } from "./ShareCard";
import { useI18n, type TFn } from "./i18n";
import { dateLocale, type Locale } from "./i18n/locale";
import { loadCache, saveCache, removeCache } from "./cache";

interface Props {
  pricing: PricingConfig;
  currency: Currency;
  agentVisibility: AgentVisibility;
}

/** 分享卡片所需的报告页已聚合数据：分享卡不再自行拉取数据源
 *  （修复打开分享慢的问题），热力图/模型/Agent 区块直接复用本结构。
 *  bucket/fxRate 供模型切片聚合与花费折算（与页面同口径）。 */
export interface ShareReportData {
  fromMs: number;
  toMs: number;
  bucket: TrendBucket;
  fxRate: number;
  currency: Currency;
  agents: ReportAgent[];
  /** 范围内按本地日聚合的序列（dayMs 升序），热力图与活跃天数共用；
   *  成本随日携带（分享卡按币种切片汇总） */
  daily: {
    dayMs: number;
    tokens: number;
    requests: number;
    costCny: number;
    costUsd: number;
  }[];
}

interface ReportSource {
  stats: Stats;
  trend: TrendPoint[];
}

interface ReportModel {
  key: string;
  agentId: ReportAgentSourceId;
  model_id: string;
  provider_id: string;
  requests: number;
  total_tokens: number;
  cost_cny: number;
  cost_usd: number;
  priced: boolean;
}

export interface ReportAgent {
  id: ReportAgentSourceId;
  label: string;
  brand: BrandIconName;
  color: string;
  requests: number;
  total_tokens: number;
  cost_cny: number;
  cost_usd: number;
  trend: TrendPoint[];
  models: ReportModel[];
}

interface LoadedSource {
  source: ReportSource;
}

interface ReportData {
  from_ms: number;
  to_ms: number;
  bucket: TrendBucket;
  /** 装配时汇率（导出 Markdown / 分享卡切片成本折算用） */
  fxRate: number;
  agents: ReportAgent[];
  trend: TrendPoint[];
  warnings: string[];
  notes: string[];
}

/**
 * 报告页数据源注册表：id → 展示元数据 + 远端同步来源。新增 agent 数据源时
 * 在此登记（并在 load() 补对应本地加载分支），REPORT_AGENT_IDS 与 agent
 * 筛选行即自动扩展，无需再改筛选渲染。remoteSource 为远端同步协议里的
 * source 字符串（无远端支持的 agent 省略，如 Cursor 明细仅本机）。
 * 品牌色不在此登记：由 AGENT_COLOR[id] 统一取（dsh / opencodego / minimax
 * 已注册）。
 *
 * 键是 **UI 侧 AgentId**（agentVisibility / AGENT_COLOR / 筛选胶囊都用它），
 * 故 OpenCode 这里是 `opencodego`、MiniMax Code 是 `minimax`；而它们与后端
 * 交互的来源名分别是 "opencode" 与 "minimaxcode"
 * （见各条目的 remoteSource 与 load() 里的本地取数分支）。
 */
const REPORT_SOURCE_REGISTRY = {
  zai: { label: "Z.ai", brand: "zai", remoteSource: "zcode" },
  codex: { label: "Codex", brand: "codex", remoteSource: "codex" },
  claude: { label: "Claude", brand: "claude", remoteSource: "claude" },
  cursor: { label: "Cursor", brand: "cursor" },
  // Kimi Code（本地 ~/.kimi-code 会话采集型）：本地取数走 fetchKimiUsage，
  // remoteSource 是**后端来源名 "kimi"**（sync.rs 增量上传与 server 白名单
  // 同值）。订阅额度（rate_limits）只来自本机 OAuth 查询，报告页额度区块
  // 本就不含该源，合并的只有 stats/trend。
  kimi: { label: "Kimi Code", brand: "kimi", remoteSource: "kimi" },
  // 纯本地采集型（本地 ~/.dsh 会话数据）：取数走 fetchDshUsage（stats +
  // trend，无 rate_limits → 报告页额度区块本就不含该源）；remoteSource 是
  // **后端来源名 "dsh"**（sync.rs 增量上传与 server 白名单同值）。
  dsh: { label: "DeepSeek Harness", brand: "dsh", remoteSource: "dsh" },
  // OpenCode（本地 opencode.db 采集型，Go 套餐额度是并列的另一条链路）：
  // 本地取数走 fetchOpencodeUsage（get_opencode_usage）；remoteSource 是
  // **后端来源名 "opencode"**（sync.rs 增量上传与 server 白名单都用它），
  // 与 UI id `opencodego` 不同字面——写成 opencodego 会拉不到远端数据。
  opencodego: {
    label: "OpenCode",
    brand: "opencodego",
    remoteSource: "opencode",
  },
  // MiniMax Code（本地 ~/.minimax 采集型，套餐额度是并列的另一条链路）：
  // 本地取数走 fetchMinimaxcodeUsage（get_minimaxcode_usage）；remoteSource 是
  // **后端来源名 "minimaxcode"**（sync.rs 增量上传与 server 白名单都用它），
  // 与 UI id `minimax` 不同字面——写成 minimax 会拉不到远端数据。
  // 键是 UI 侧 agent id `minimax`，与 OpenCode 同款的两个域异名。
  minimax: {
    label: "MiniMax Code",
    brand: "minimax",
    remoteSource: "minimaxcode",
  },
} as const satisfies Record<
  string,
  { label: string; brand: BrandIconName; remoteSource?: string }
>;

/** 有本地数据源的报告 agent id。 */
type ReportAgentSourceId = keyof typeof REPORT_SOURCE_REGISTRY;

/** 报告统计的 agent 集合：由数据源注册表派生（不硬编码列表）。 */
const REPORT_AGENT_IDS = Object.keys(
  REPORT_SOURCE_REGISTRY
) as ReportAgentSourceId[];

/** 图例前缀用的来源名：优先取同步协议 provider 的注册名，未登记的 provider
 *  （dsh 的 provider_id 是自由字符串，如 "deepseek-account" / 第三方）回退到
 *  该模型所属报告 agent 的注册名，避免图例出现 "Unknown xxx"。
 *  OpenCode 的 provider_id 通常就是 "opencode"（sourceMeta 已登记 → 显示
 *  "OpenCode"），第三方路由才回退到注册表标签，两条路径都不会出 Unknown。
 *  MiniMax Code 同理（provider_id 通常是 "minimaxcode"，sourceMeta 已登记）。 */
function sourcePrefixOf(
  agentId: ReportAgentSourceId,
  providerId: string
): string {
  const label = sourceMeta(providerId).label;
  return label === "Unknown" ? REPORT_SOURCE_REGISTRY[agentId].label : label;
}

/** 本地日期 YYYY-MM-DD，避免 UTC 偏移。 */
function localDateStr(ms: number): string {
  const d = new Date(ms);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return y + "-" + m + "-" + day;
}

/** 本地日期偏移 n 天后格式化（用 Date 加减，避免毫秒减法的 DST 偏移）。 */
function offsetLocalDateStr(ms: number, days: number): string {
  const d = new Date(ms);
  d.setDate(d.getDate() + days);
  return localDateStr(d.getTime());
}

function startOfLocalDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 解析 YYYY-MM-DD 为本地当天 0 点，格式非法时返回 NaN。 */
function parseLocalDate(date: string): number {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(y, (m ?? 1) - 1, d ?? 1).getTime();
}

/** 报告数据口径固定为全量：from = 89 天前 0 点（数据只保留 90 天，
 *  与原 boundedRange 下限同款）、to = 现在，恒按天分桶。
 *  时间范围选择已按需求移除——报告页即「近 90 天全量报告」。 */
function fixedReportRange(): { fromMs: number; toMs: number; bucket: TrendBucket } {
  const now = Date.now();
  return {
    fromMs: startOfLocalDay(parseLocalDate(offsetLocalDateStr(now, -89))),
    toMs: now,
    bucket: "day",
  };
}

function shortError(error: unknown): string {
  const text = String(error).replace(/^Error:\s*/, "");
  return text.length > 90 ? text.slice(0, 87) + "…" : text;
}

function makeAgent(
  id: ReportAgentSourceId,
  source: ReportSource,
  pricing: PricingConfig,
  fxRate: number
): ReportAgent {
  const meta = REPORT_SOURCE_REGISTRY[id];
  // 报告数据由本组件自拉自合（不经 DataCache），折叠在此处收口
  const models = foldModelStatRows(source.stats.by_model).map((model) => ({
    key: id + "|" + model.provider_id + "|" + model.model_id,
    agentId: id,
    model_id: model.model_id,
    provider_id: model.provider_id,
    requests: model.requests,
    total_tokens: model.total_tokens,
    cost_cny: modelCost(
      model.model_id,
      model.input_tokens,
      model.output_tokens,
      model.cache_read_tokens,
      pricing,
      "cny",
      fxRate
    ),
    cost_usd: modelCost(
      model.model_id,
      model.input_tokens,
      model.output_tokens,
      model.cache_read_tokens,
      pricing,
      "usd",
      fxRate
    ),
    priced: hasPositivePrice(model.model_id, pricing),
  }));

  return {
    id,
    label: meta.label,
    brand: meta.brand,
    color: AGENT_COLOR[id],
    requests: source.stats.overall.requests,
    total_tokens: source.stats.overall.total_tokens,
    cost_cny: models.reduce((sum, model) => sum + model.cost_cny, 0),
    cost_usd: models.reduce((sum, model) => sum + model.cost_usd, 0),
    trend: source.trend,
    models,
  };
}

/** Cursor 返回的是按日明细，转换成报告统一的 Agent 结构。 */
function makeCursorAgent(
  snapshot: CursorSnapshot,
  fromMs: number,
  toMs: number,
  fxRate: number
): ReportAgent | null {
  if (
    !snapshot.logged_in &&
    snapshot.by_model.length === 0 &&
    !snapshot.events
  ) {
    return null;
  }

  // Cursor 数据同理由本组件自拉，折叠在此处收口
  const models: ReportModel[] = foldCursorModelRows(snapshot.by_model).map((model) => ({
    key: "cursor|cursor|" + model.model,
    agentId: "cursor",
    model_id: model.model,
    provider_id: "cursor",
    requests: model.requests,
    total_tokens: model.total_tokens,
    cost_usd: model.cost_usd,
    cost_cny: model.cost_usd * fxRate,
    priced: true,
  }));
  const modelTotals = models.reduce(
    (sum, model) => ({
      requests: sum.requests + model.requests,
      input_tokens: sum.input_tokens,
      output_tokens: sum.output_tokens,
      cache_read_tokens: sum.cache_read_tokens,
      total_tokens: sum.total_tokens + model.total_tokens,
    }),
    {
      requests: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      total_tokens: 0,
    }
  );
  const events = snapshot.events;
  const stats: Stats = {
    from_ms: fromMs,
    to_ms: toMs,
    overall: {
      requests: events?.requests ?? modelTotals.requests,
      input_tokens: events?.input_tokens ?? modelTotals.input_tokens,
      output_tokens: events?.output_tokens ?? modelTotals.output_tokens,
      cache_read_tokens:
        events?.cache_read_tokens ?? modelTotals.cache_read_tokens,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      total_tokens: events?.total_tokens ?? modelTotals.total_tokens,
    },
    by_model: models.map((model) => ({
      model_id: model.model_id,
      provider_id: model.provider_id,
      requests: model.requests,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
      total_tokens: model.total_tokens,
    })),
    earliest_ms: null,
    latest_ms: null,
  };
  const costUsd =
    events?.total_cost_usd ??
    models.reduce((sum, model) => sum + model.cost_usd, 0);

  return {
    id: "cursor",
    label: REPORT_SOURCE_REGISTRY.cursor.label,
    brand: REPORT_SOURCE_REGISTRY.cursor.brand,
    color: AGENT_COLOR.cursor,
    requests: stats.overall.requests,
    total_tokens: stats.overall.total_tokens,
    cost_cny: costUsd * fxRate,
    cost_usd: costUsd,
    trend: snapshot.daily.map((point) => ({
      label: point.date,
      total_tokens: point.total_tokens,
      requests: point.requests,
      cost_usd: point.cost_usd,
      cost_cny: point.cost_usd * fxRate,
    })),
    models,
  };
}

/** 来源在报告范围内是否有用量：stats 为查询口径（趋势扩到 89 天时可能
 *  大于报告范围），必须对 trend 逐点做范围切片后判断。 */
function hasUsageInRange(
  source: ReportSource,
  fromMs: number,
  toMs: number,
  bucket: TrendBucket
): boolean {
  if (
    source.stats.overall.requests > 0 ||
    source.stats.overall.total_tokens > 0
  ) {
    // stats 本身也可能大于报告范围（89 天查询口径），仍需趋势切片确认
    return source.trend.some((point) => {
      const day = labelToDayStart(point.label, fromMs, toMs, bucket);
      return (
        day != null &&
        (point.total_tokens > 0 ||
          point.requests > 0 ||
          point.cost_cny > 0 ||
          point.cost_usd > 0)
      );
    });
  }
  return false;
}

/** 把本地来源和远端同来源数据合成一份，避免报告重复计算。 */
function combineSource(
  local: ReportSource | null,
  remote: RemoteUsage | null,
  pricing: PricingConfig,
  fxRate: number,
  bucket: TrendBucket
): ReportSource | null {
  if (local && remote) {
    return {
      stats: mergeStats(local.stats, remote),
      trend: mergeTrend(local.trend, remote, pricing, fxRate, bucket),
    };
  }
  if (local) return local;
  if (remote) {
    return {
      stats: remoteToStats(remote),
      trend: mergeTrend([], remote, pricing, fxRate, bucket),
    };
  }
  return null;
}

/** 生成排序基准的期望标签序列：day 桶逐日 "MM-DD"（跨年也按时间顺序），
 *  hour 桶仅单日、逐小时 "HH:00"，均与后端 db.rs 的标签格式一致。 */
function expectedTrendLabels(
  bucket: TrendBucket,
  fromMs: number,
  toMs: number
): Map<string, number> {
  const order = new Map<string, number>();
  if (bucket === "hour") {
    for (let h = 0; h < 24; h++) {
      order.set(String(h).padStart(2, "0") + ":00", h);
    }
    return order;
  }
  const cursor = new Date(startOfLocalDay(fromMs));
  const end = startOfLocalDay(toMs);
  while (cursor.getTime() <= end && order.size < 400) {
    const label =
      String(cursor.getMonth() + 1).padStart(2, "0") +
      "-" +
      String(cursor.getDate()).padStart(2, "0");
    if (!order.has(label)) order.set(label, order.size);
    cursor.setDate(cursor.getDate() + 1);
  }
  return order;
}

function mergeReportTrends(
  agents: ReportAgent[],
  bucket: TrendBucket,
  fromMs: number,
  toMs: number
): TrendPoint[] {
  const byLabel = new Map<string, TrendPoint>();
  for (const agent of agents) {
    for (const point of agent.trend) {
      const previous = byLabel.get(point.label);
      if (!previous) {
        byLabel.set(point.label, { ...point });
      } else {
        previous.total_tokens += point.total_tokens;
        previous.requests += point.requests;
        previous.cost_cny += point.cost_cny;
        previous.cost_usd += point.cost_usd;
        previous.by_model = mergeByModel(previous.by_model, point.by_model);
      }
    }
  }
  // 按时间范围生成的期望序列排序，避免 "MM-DD" 标签跨年时按字典序错乱；
  // 序列外的标签（如 Cursor 的 "YYYY-MM-DD"）排在最后。
  const order = expectedTrendLabels(bucket, fromMs, toMs);
  const rank = (label: string) => order.get(label) ?? Number.MAX_SAFE_INTEGER;
  return [...byLabel.values()].sort((a, b) => rank(a.label) - rank(b.label));
}

function selectedCost(
  item: { cost_cny: number; cost_usd: number },
  currency: Currency
): number {
  return currency === "cny" ? item.cost_cny : item.cost_usd;
}

// ============================================================
// 本地秒显缓存（对齐 ComparePanel 的 zbar-compare-cache 模式）：
// 挂载/口径变化时先渲染上次结果（标注后台刷新中），load 完成后替换。
// 存「已合并（本地+远端）的 per-agent 原始数据」而非渲染结果，
// 重建时按当前 pricing 与缓存时汇率重算花费与派生结构。
// ============================================================

const REPORT_CACHE_KEY = "zbar-report-cache-v5";
/** 上一版缓存键（按范围多条目），写入新键时顺带清理。 */
const REPORT_CACHE_KEY_LEGACY = "zbar-report-cache-v4";
/** 口径固定全量后缓存为单条目，scope 恒为该常量。 */
const REPORT_CACHE_SCOPE = "all";
/** 最多保留的范围口径数（超出滚动淘汰最旧）。 */
const REPORT_CACHE_LIMIT = 4;
/** 单条缓存序列化体积上限：超出先裁剪趋势点的按模型明细（体积大头），
 *  仍超限则放弃本条写入——避免大范围数据反复触发 QuotaExceededError
 *  被 saveCache 静默吞掉、缓存整体失效且每轮重复序列化拖慢加载。 */
const REPORT_CACHE_ENTRY_LIMIT = 500_000;

/** 报告页缓存条目。 */
interface ReportCacheEntry {
  from_ms: number;
  to_ms: number;
  bucket: TrendBucket;
  /** zai/codex/claude/kimi/dsh/opencodego/minimax 已合并来源（stats + trend） */
  sources: Partial<Record<Exclude<ReportAgentSourceId, "cursor">, ReportSource>>;
  /** Cursor 原始快照（每日明细，重建时再折叠） */
  cursor: CursorSnapshot | null;
  /** 生成时的汇率（重建 cost_cny 用，刷新完成后自动纠正） */
  fxRate: number;
  ts: number;
}

type ReportCacheStore = Record<string, ReportCacheEntry>;

function readReportCache(scope: string): ReportCacheEntry | null {
  const store = loadCache<ReportCacheStore>(REPORT_CACHE_KEY);
  const entry = store?.[scope];
  if (!entry || !Number.isFinite(entry.ts) || !Number.isFinite(entry.fxRate)) {
    return null;
  }
  if (!entry.sources || typeof entry.sources !== "object") {
    return null;
  }
  // 半截/损坏结构防御：任何字段形状不符则整条作废（恢复在 effect 内，
  // 抛错会中断组件树，必须在这里全部拦下）
  if (entry.cursor) {
    if (
      !Array.isArray(entry.cursor.daily) ||
      !Array.isArray(entry.cursor.by_model)
    ) {
      return null;
    }
  }
  for (const id of Object.keys(entry.sources) as Exclude<
    ReportAgentSourceId,
    "cursor"
  >[]) {
    const source = entry.sources[id];
    if (
      !source?.stats?.overall ||
      !Array.isArray(source.stats.by_model) ||
      !Array.isArray(source.trend)
    ) {
      return null;
    }
  }
  return entry;
}

/** 裁掉趋势点里的按模型明细（缓存条目的体积大头），只保留总量序列。 */
function slimCacheEntry(entry: ReportCacheEntry): ReportCacheEntry {
  const sources: ReportCacheEntry["sources"] = {};
  for (const [id, source] of Object.entries(entry.sources) as [
    Exclude<ReportAgentSourceId, "cursor">,
    ReportSource,
  ][]) {
    sources[id] = {
      stats: source.stats,
      trend: source.trend.map(({ by_model: _byModel, ...rest }) => rest),
    };
  }
  return { ...entry, sources };
}

let legacyCacheCleaned = false;

function writeReportCache(scope: string, entry: ReportCacheEntry): void {
  // 结构升级（v2 含已删除的额度快照字段）：首次写入新键时顺带清理旧键
  if (!legacyCacheCleaned) {
    removeCache(REPORT_CACHE_KEY_LEGACY);
    legacyCacheCleaned = true;
  }
  const store = loadCache<ReportCacheStore>(REPORT_CACHE_KEY) ?? {};
  const next = { ...store, [scope]: entry };
  const keys = Object.keys(next);
  if (keys.length > REPORT_CACHE_LIMIT) {
    keys
      .sort((a, b) => next[a].ts - next[b].ts)
      .slice(0, keys.length - REPORT_CACHE_LIMIT)
      .forEach((key) => delete next[key]);
  }
  // 超限先降级为「无按模型明细」的瘦身条目；仍超限则主动跳过本条
  let payload: ReportCacheStore = next;
  if (JSON.stringify(entry).length > REPORT_CACHE_ENTRY_LIMIT) {
    const slim = slimCacheEntry(entry);
    if (JSON.stringify(slim).length > REPORT_CACHE_ENTRY_LIMIT) return;
    payload = { ...next, [scope]: slim };
  }
  saveCache(REPORT_CACHE_KEY, payload);
}

/** 缓存条目 → 两阶段装配的「本地」口径（花费在装配时按缓存汇率重算）。 */
function localResultsFromEntry(entry: ReportCacheEntry): LocalResults {
  const loaded = (id: Exclude<ReportAgentSourceId, "cursor">): LoadedSource | null => {
    const source = entry.sources[id];
    if (!source) return null;
    return { source };
  };
  return {
    zai: entry.sources.zai ?? null,
    codex: loaded("codex"),
    claude: loaded("claude"),
    kimi: loaded("kimi"),
    dsh: loaded("dsh"),
    opencodego: loaded("opencodego"),
    minimax: loaded("minimax"),
    cursor: entry.cursor,
    fxRate: entry.fxRate,
  };
}

// ============================================================
// 两阶段装配：本地命令先渲染、远端返回后合并，两个阶段与缓存重建
// 共用同一段组装逻辑，保证三种来源口径一致。
// ============================================================

/** 本地阶段（含缓存重建）已就绪的数据：远端合并前的口径。 */
interface LocalResults {
  zai: ReportSource | null;
  codex: LoadedSource | null;
  claude: LoadedSource | null;
  kimi: LoadedSource | null;
  /** DSH（本地 ~/.dsh 会话数据）：取数走 fetchDshUsage，键是 UI 侧 agent id
   *  `dsh`（远端来源名同字面，见注册表 remoteSource） */
  dsh: LoadedSource | null;
  /** OpenCode（本地 opencode.db 采集型）：取数走 fetchOpencodeUsage，
   *  键是 UI 侧 agent id `opencodego`（远端来源名字面见注册表 remoteSource） */
  opencodego: LoadedSource | null;
  /** MiniMax Code（本地 ~/.minimax 采集型）：取数走 fetchMinimaxcodeUsage，
   *  键是 UI 侧 agent id `minimax`（后端来源名是 `minimaxcode`，两个域异名） */
  minimax: LoadedSource | null;
  cursor: CursorSnapshot | null;
  fxRate: number;
}

/** 远端阶段返回的数据（同步未启用 / 筛选仅本地时各字段为空）。
 *  仅登记有远端同步来源的 agent：Cursor 明细只在本机。 */
interface RemoteResults {
  zai: RemoteUsage | null;
  codex: RemoteUsage | null;
  claude: RemoteUsage | null;
  /** Kimi Code 明细已在 sync.rs 里按 source="kimi" 上传（server 白名单同值） */
  kimi: RemoteUsage | null;
  /** DSH 明细已在 sync.rs 里按 source="dsh" 上传（server 白名单同值） */
  dsh: RemoteUsage | null;
  /** OpenCode 明细已在 sync.rs 里按 source="opencode" 上传（server 白名单同值） */
  opencodego: RemoteUsage | null;
  /** MiniMax Code 明细已在 sync.rs 里按 source="minimaxcode" 上传（白名单同值） */
  minimax: RemoteUsage | null;
}

interface Assembled {
  data: ReportData;
  /** 可落盘的缓存载荷（已合并口径） */
  cache: {
    sources: Partial<Record<Exclude<ReportAgentSourceId, "cursor">, ReportSource>>;
    cursor: CursorSnapshot | null;
  };
}

function assembleReport(args: {
  fromMs: number;
  toMs: number;
  bucket: TrendBucket;
  agentVisibility: AgentVisibility;
  pricing: PricingConfig;
  t: TFn;
  local: LocalResults;
  remote: RemoteResults | null;
}): Assembled {
  const { fromMs, toMs, bucket, agentVisibility, pricing, t, local, remote } =
    args;
  const fxRate = local.fxRate;
  const agents: ReportAgent[] = [];
  const cacheSources: Assembled["cache"]["sources"] = {};

  const addSourceAgent = (
    id: Exclude<ReportAgentSourceId, "cursor">,
    localSource: ReportSource | null,
    remoteUsageData: RemoteUsage | null
  ) => {
    if (!agentVisibility[id]) return;
    const source = combineSource(
      localSource,
      remoteUsageData,
      pricing,
      fxRate,
      bucket
    );
    if (!source) return;
    if (hasUsageInRange(source, fromMs, toMs, bucket)) {
      agents.push(makeAgent(id, source, pricing, fxRate));
    }
    // 合并后来源无条件入缓存（无用量重建后同样不出卡，口径一致）
    cacheSources[id] = source;
  };

  addSourceAgent("zai", local.zai, remote?.zai ?? null);
  addSourceAgent("codex", local.codex?.source ?? null, remote?.codex ?? null);
  addSourceAgent("claude", local.claude?.source ?? null, remote?.claude ?? null);
  // Kimi：本机快照 + 远端（source="kimi"）；订阅额度仍只来自本机
  addSourceAgent("kimi", local.kimi?.source ?? null, remote?.kimi ?? null);
  // dsh（DeepSeek Harness）：本机快照 + 远端（source="dsh"），同款合并口径
  addSourceAgent("dsh", local.dsh?.source ?? null, remote?.dsh ?? null);
  // OpenCode：本机快照 + 远端（source="opencode"，与 zai/codex/claude 同款
  // 合并口径）；同步未启用时 remote 为 null，等价于纯本机
  addSourceAgent(
    "opencodego",
    local.opencodego?.source ?? null,
    remote?.opencodego ?? null
  );
  // MiniMax Code：本机快照 + 远端（source="minimaxcode"，与 zai/codex/claude/
  // opencodego 同款合并口径）；同步未启用时 remote 为 null，等价于纯本机。
  // 套餐额度走 providerQuota 另一条链路，报告页额度区块本就不含该源
  addSourceAgent(
    "minimax",
    local.minimax?.source ?? null,
    remote?.minimax ?? null
  );
  const cursorAgent =
    agentVisibility.cursor && local.cursor
      ? makeCursorAgent(local.cursor, fromMs, toMs, fxRate)
      : null;
  if (
    cursorAgent &&
    (hasUsageInRange(
      {
        stats: {
          from_ms: fromMs,
          to_ms: toMs,
          overall: {
            requests: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: 0,
          },
          by_model: [],
          earliest_ms: null,
          latest_ms: null,
        },
        trend: cursorAgent.trend,
      },
      fromMs,
      toMs,
      bucket
    ))
  ) {
    agents.push(cursorAgent);
  }

  // Cursor 官方明细按日返回，小时桶趋势无法混入，需剔除并加说明；
  // 判断依据是 bucket 而非 kind（自定义短跨度同样走小时桶）。
  const trendAgents =
    bucket === "hour"
      ? agents.filter((agent) => agent.id !== "cursor")
      : agents;
  const notes: string[] = [];
  if (bucket === "hour" && cursorAgent) {
    notes.push(t("report.noteCursorDaily"));
  }

  return {
    data: {
      from_ms: fromMs,
      to_ms: toMs,
      bucket,
      fxRate,
      agents,
      trend: mergeReportTrends(trendAgents, bucket, fromMs, toMs),
      warnings: [],
      notes,
    },
    cache: {
      sources: cacheSources,
      cursor: local.cursor,
    },
  };
}

// ============================================================
// 图表数据工具：日聚合（热力图 / KPI / 活跃度）与按模型日序列（趋势柱图）
// ============================================================

/** 单日聚合（本地时区自然日）。成本随日聚合，供 KPI 在趋势查询口径
 *  扩到 89 天后仍按报告范围切片展示。 */
interface DayTotals {
  tokens: number;
  requests: number;
  costCny: number;
  costUsd: number;
}

/** 空聚合起点。 */
const emptyDayTotals = (): DayTotals => ({
  tokens: 0,
  requests: 0,
  costCny: 0,
  costUsd: 0,
});

/** 趋势点标签 → 所在自然日 0 点（本地时区）。
 *  - hour 桶标签 "HH:00" 无日期，全部归属范围起始日（hour 桶仅单日跨度）；
 *  - 日桶本地标签 "MM-DD" 不带年份，在范围内顺序定位（≤89 天窗口内唯一）；
 *  - Cursor 明细为 "YYYY-MM-DD"，直接解析。无法识别返回 null。 */
function labelToDayStart(
  label: string,
  fromMs: number,
  toMs: number,
  bucket: TrendBucket
): number | null {
  if (bucket === "hour") return startOfLocalDay(fromMs);
  if (/^\d{4}-\d{2}-\d{2}$/.test(label)) return parseLocalDate(label);
  if (/^\d{2}-\d{2}$/.test(label)) {
    const cursor = new Date(startOfLocalDay(fromMs));
    const end = startOfLocalDay(toMs);
    while (cursor.getTime() <= end) {
      if (localDateStr(cursor.getTime()).slice(5) === label) {
        return cursor.getTime();
      }
      cursor.setDate(cursor.getDate() + 1);
    }
    return null;
  }
  return null;
}

/** 连续活跃天数：逐自然日检查 tokens>0。
 *  current 从范围末日（≤今天）往前数（GitHub 口径：末日无消耗从倒数第二
 *  天起算），遇到断档即止；longest 为范围内最长。 */
function computeStreaks(
  daily: Map<string, DayTotals>,
  fromMs: number,
  toMs: number
): { current: number; longest: number } {
  const startDay = startOfLocalDay(fromMs);
  const endDay = startOfLocalDay(toMs);
  const days: string[] = [];
  const cursor = new Date(startDay);
  while (cursor.getTime() <= endDay && days.length < 400) {
    days.push(localDateStr(cursor.getTime()));
    cursor.setDate(cursor.getDate() + 1);
  }
  let longest = 0;
  let run = 0;
  for (const day of days) {
    run = (daily.get(day)?.tokens ?? 0) > 0 ? run + 1 : 0;
    if (run > longest) longest = run;
  }
  let current = 0;
  let start = days.length - 1;
  if (
    days.length > 0 &&
    (daily.get(days[days.length - 1])?.tokens ?? 0) <= 0
  ) {
    start = days.length - 2;
  }
  for (let i = start; i >= 0; i--) {
    if ((daily.get(days[i])?.tokens ?? 0) > 0) current += 1;
    else break;
  }
  return { current, longest };
}

/** ms 所在自然周（本地时区）的周一 0 点（与 ComparePanel 同款实现）。 */
function weekStartOf(ms: number): number {
  const d = new Date(ms);
  const day = d.getDay(); // 周日为 0，转换成周一基数
  d.setDate(d.getDate() - (day === 0 ? 6 : day - 1));
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 热力图一列（一个自然周）；cells 固定 7 槽（周一~周日），范围外为 null。 */
interface HeatColumn {
  startMs: number;
  cells: ({ dayMs: number; key: string; tokens: number; requests: number } | null)[];
}

/** 生成范围覆盖的热力图列：从范围起始日所在周的周一到末日，逐周推进。 */
function buildHeatColumns(fromMs: number, toMs: number): HeatColumn[] {
  const startDay = startOfLocalDay(fromMs);
  const endDay = startOfLocalDay(toMs);
  const columns: HeatColumn[] = [];
  const cursor = new Date(weekStartOf(startDay));
  while (cursor.getTime() <= endDay) {
    const colStart = cursor.getTime();
    const cells: HeatColumn["cells"] = [];
    for (let i = 0; i < 7; i++) {
      const day = new Date(colStart);
      day.setDate(day.getDate() + i);
      const dayMs = day.getTime();
      if (dayMs < startDay || dayMs > endDay) {
        cells.push(null);
      } else {
        cells.push({ dayMs, key: localDateStr(dayMs), tokens: 0, requests: 0 });
      }
    }
    columns.push({ startMs: colStart, cells });
    cursor.setDate(cursor.getDate() + 7);
  }
  return columns;
}

/** token 量 → 亮度档位（0 = 无数据空态，1~4 递亮）。
 *  相对窗口单日峰值的比例阈值：≥75% 峰值档、≥40% 高活、≥15% 日常、
 *  其余低活——比 P50 分位层次更分明（中档不再占大半图）。 */
function heatLevel(tokens: number, max: number): number {
  if (tokens <= 0 || max <= 0) return 0;
  if (tokens >= max * 0.75) return 4;
  if (tokens >= max * 0.4) return 3;
  if (tokens >= max * 0.15) return 2;
  return 1;
}

/** 模型系列色：ZCode usage-chart 六色板（CSS 类双主题：亮色 -600 深档、
 *  暗色 -400 亮档，见 index.css .series-color-*），按 token 量依序分配；
 *  「其他」固定中性灰。 */
const SERIES_CLASSES = [
  "series-color-0",
  "series-color-1",
  "series-color-2",
  "series-color-3",
  "series-color-4",
  "series-color-5",
] as const;
const OTHER_SERIES_CLASS = "series-color-other";

interface TrendSeries {
  key: string;
  label: string;
  colorCls: string;
  total: number;
}

interface TrendGroup {
  dayMs: number;
  key: string;
  values: Record<string, number>;
  total: number;
}

/** 热力图 / 趋势 tooltip 日期：zh "2026年10月5日"，en "Oct 5, 2026"。 */
function formatHeatDate(ms: number, locale: Locale): string {
  const d = new Date(ms);
  if (locale === "zh") {
    return `${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日`;
  }
  return d.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

/** 热力图底部月份短标签：zh "10月"，en "Oct"。 */
function monthShortLabel(month: number, locale: Locale): string {
  if (locale === "zh") return `${month + 1}月`;
  return new Date(2020, month, 1).toLocaleString("en-US", { month: "short" });
}

/**
 * 用量报告内容区：供 ReportsPanel 壳按标签复用。内容区自成 ZCode 深色
 * 玻璃体系（固定深蓝→紫渐变底 + 白字透明度体系，不随应用亮暗主题变化），
 * 分区为 agent 筛选行、KPI 行、活跃度五项、Token 活动热力图与用量趋势。
 * 加载为两阶段：本地命令先并发先渲染，远端同步独立异步返回后合并。
 */
export function ReportContent({
  pricing,
  currency,
  agentVisibility,
}: Props) {
  const { locale, t } = useI18n();
  const [report, setReport] = useState<ReportData | null>(null);
  // 分项加载状态：本地命令 / 远端同步分别提示（慢在哪个环节可见）
  const [localLoading, setLocalLoading] = useState(true);
  const [syncing, setSyncing] = useState(false);
  // 当前展示的是否为缓存快照（后台刷新完成后自动解除）
  const [stale, setStale] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [doneFlash, setDoneFlash] = useState<string | null>(null);
  // 分享卡片弹层（默认生成近 7 天卡片，数据由弹层自拉）
  const [shareOpen, setShareOpen] = useState(false);

  // Agent 筛选（"all" = 现有合并口径）；切换纯前端 useMemo 过滤，不发请求
  const [agentScope, setAgentScope] = useState<"all" | ReportAgentSourceId>("all");
  // 热力图粒度（每日 / 每周 / 累计）
  const [heatGranularity, setHeatGranularity] = useState<
    "day" | "week" | "cum"
  >("day");
  // 热力图横向滚动容器：挂载/粒度切换/数据就绪后默认滚到最右端（今天）
  const heatScrollRef = useRef<HTMLDivElement | null>(null);
  // 两侧渐隐遮罩可见性：按滚动位置条件渲染（滚到边即消失，不盖真实格子）
  const [heatCanLeft, setHeatCanLeft] = useState(false);
  const [heatCanRight, setHeatCanRight] = useState(false);
  // 用量趋势柱状图窗口（近 7 / 30 天，只影响本图表；全量数据末尾切片）
  const [trendWindow, setTrendWindow] = useState<number>(7);
  // 趋势图图例勾选（隐藏的系列 key）
  const [hiddenSeries, setHiddenSeries] = useState<Set<string>>(new Set());

  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const [deviceFilter, setDeviceFilter] = useState("all");
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;
  const loadReqId = useRef(0);

  useEffect(() => {
    getSyncConfig()
      .then((config) => {
        setSyncConfig(config);
        if (config.enabled && config.device_token) {
          listRemoteDevices()
            .then(setRemoteDevices)
            .catch(() => {});
        }
      })
      .catch(() => {});
  }, []);

  // 口径固定为全量后缓存为单条目（scope 不再携带范围/设备/可见性指纹，
  // 重建时按当前 agentVisibility 过滤，可见性变化由刷新补齐）。
  useEffect(() => {
    const entry = readReportCache(REPORT_CACHE_SCOPE);
    if (entry) {
      setReport(
        assembleReport({
          fromMs: entry.from_ms,
          toMs: entry.to_ms,
          bucket: entry.bucket,
          agentVisibility,
          pricing,
          t,
          local: localResultsFromEntry(entry),
          remote: null,
        }).data
      );
      setStale(true);
    } else {
      setReport(null);
      setStale(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 缓存快照仅在挂载时应用；刷新由 load 负责
  }, []);

  const load = useCallback(async () => {
    const reqId = ++loadReqId.current;
    // 口径固定全量：from = 89 天收敛下限、to = 现在（热力图与报告共用同一次查询）
    const { fromMs, toMs, bucket } = fixedReportRange();
    const wantLocal = deviceFilter === "all" || deviceFilter === "local";
    const wantRemote =
      syncEnabled &&
      deviceFilter !== "local" &&
      !!syncConfig?.device_token;
    const safe = async <T,>(task: () => Promise<T>): Promise<T | null> => {
      try {
        return await task();
      } catch {
        // 可选 Agent 未安装、未登录或没有会话时按空数据处理，不打扰页面。
        return null;
      }
    };

    // —— 本地阶段：本地命令先并发，先渲染（远端 10-15s 超时不再拖死首屏）——
    const localZaiPromise: Promise<ReportSource | null> =
      agentVisibility.zai && wantLocal
        ? safe(async () => {
            const [stats, trend] = await Promise.all([
              fetchStats(fromMs, toMs),
              fetchTrend(fromMs, toMs, bucket),
            ]);
            return { stats, trend };
          })
        : Promise.resolve(null);
    const localCodexPromise: Promise<LoadedSource | null> =
      agentVisibility.codex && wantLocal
        ? safe(async () => {
            const snapshot: CodexSnapshot = await fetchCodexUsage(
              fromMs,
              toMs,
              bucket
            );
            return { source: { stats: snapshot.stats, trend: snapshot.trend } };
          })
        : Promise.resolve(null);
    const localClaudePromise: Promise<LoadedSource | null> =
      agentVisibility.claude && wantLocal
        ? safe(async () => {
            const snapshot: ClaudeSnapshot = await fetchClaudeUsage(
              fromMs,
              toMs,
              bucket
            );
            return { source: { stats: snapshot.stats, trend: snapshot.trend } };
          })
        : Promise.resolve(null);
    const localCursorPromise: Promise<CursorSnapshot | null> =
      agentVisibility.cursor && wantLocal
        ? safe(() => fetchCursorUsage(fromMs, toMs))
        : Promise.resolve(null);
    // Kimi 本机快照（未安装时 safe 兜底为 null）；远端部分见下面的
    // remoteUsageFor（source="kimi"），额度字段不进报告页
    const localKimiPromise: Promise<LoadedSource | null> =
      agentVisibility.kimi && wantLocal
        ? safe(async () => {
            const snapshot: KimiSnapshot = await fetchKimiUsage(
              fromMs,
              toMs,
              bucket
            );
            return { source: { stats: snapshot.stats, trend: snapshot.trend } };
          })
        : Promise.resolve(null);
    // dsh（DeepSeek Harness）本机快照（~/.dsh 会话数据，快照无 rate_limits）；
    // 未安装 / 无会话目录时 safe 兜底为 null，远端部分见 remoteUsageFor
    const localDshPromise: Promise<LoadedSource | null> =
      agentVisibility.dsh && wantLocal
        ? safe(async () => {
            const snapshot: DshSnapshot = await fetchDshUsage(
              fromMs,
              toMs,
              bucket
            );
            return { source: { stats: snapshot.stats, trend: snapshot.trend } };
          })
        : Promise.resolve(null);
    // OpenCode：本地取数命令是 get_opencode_usage（模块名 "opencode"），
    // 快照只有 stats/trend（Go 套餐额度走 providerQuota 另一条链路，报告页
    // 不展示）；未安装 / 本地库无数据时 safe 兜底为 null
    const localOpencodePromise: Promise<LoadedSource | null> =
      agentVisibility.opencodego && wantLocal
        ? safe(async () => {
            const snapshot: OpencodeSnapshot = await fetchOpencodeUsage(
              fromMs,
              toMs,
              bucket
            );
            return { source: { stats: snapshot.stats, trend: snapshot.trend } };
          })
        : Promise.resolve(null);
    // MiniMax Code：本地取数命令是 get_minimaxcode_usage（后端来源名
    // "minimaxcode"，UI 侧 agent id 仍是 minimax），快照只有 stats/trend
    // （套餐额度走 providerQuota 另一条链路，报告页不展示）；未安装 / 本地库
    // 无数据时 safe 兜底为 null
    const localMinimaxPromise: Promise<LoadedSource | null> =
      agentVisibility.minimax && wantLocal
        ? safe(async () => {
            const snapshot: MinimaxcodeSnapshot = await fetchMinimaxcodeUsage(
              fromMs,
              toMs,
              bucket
            );
            return { source: { stats: snapshot.stats, trend: snapshot.trend } };
          })
        : Promise.resolve(null);
    const fxRatePromise = getCursorConfig()
      .then((config) =>
        config.usd_cny_rate > 0 ? config.usd_cny_rate : 7.2
      )
      .catch(() => 7.2);

    const remoteOptions = (source: string) =>
      deviceFilter === "all"
        ? { excludeDevice: syncConfig?.device_id ?? "", source }
        : { devices: deviceFilter, source };
    /** 有远端同步来源的 agent 统一走这里（source 字符串取自注册表）。 */
    const remoteUsageFor = (
      id: "zai" | "codex" | "claude" | "kimi" | "dsh" | "opencodego" | "minimax"
    ) =>
      agentVisibility[id] && wantRemote
        ? safe(() =>
            remoteUsage(fromMs, toMs, bucket, {
              ...remoteOptions(REPORT_SOURCE_REGISTRY[id].remoteSource),
            })
          )
        : Promise.resolve(null);
    const remoteZaiPromise = remoteUsageFor("zai");
    const remoteCodexPromise = remoteUsageFor("codex");
    const remoteClaudePromise = remoteUsageFor("claude");
    // Kimi 的远端 source 是 "kimi"（注册表 remoteSource），与 UI id 同字面
    const remoteKimiPromise = remoteUsageFor("kimi");
    // DSH 的远端 source 是 "dsh"（注册表 remoteSource），与 UI id 同字面
    const remoteDshPromise = remoteUsageFor("dsh");
    // OpenCode 的远端 source 是 "opencode"（注册表 remoteSource），非 UI id
    const remoteOpencodePromise = remoteUsageFor("opencodego");
    // MiniMax Code 的远端 source 是 "minimaxcode"（注册表 remoteSource），非 UI id
    const remoteMinimaxPromise = remoteUsageFor("minimax");

    setLocalLoading(true);
    setSyncing(wantRemote);
    setError(null);

    let localResults: LocalResults | null = null;
    // 本地阶段是否已渲染出内容（决定远端失败时是否提示错误）
    let localRendered = false;
    try {
      const [lz, lc, ll, lcur, lk, ld, lo, lm, fx] = await Promise.all([
        localZaiPromise,
        localCodexPromise,
        localClaudePromise,
        localCursorPromise,
        localKimiPromise,
        localDshPromise,
        localOpencodePromise,
        localMinimaxPromise,
        fxRatePromise,
      ]);
      if (reqId !== loadReqId.current) return;
      localResults = {
        zai: lz,
        codex: lc,
        claude: ll,
        kimi: lk,
        dsh: ld,
        opencodego: lo,
        minimax: lm,
        cursor: lcur,
        fxRate: fx,
      };
      const built = assembleReport({
        fromMs,
        toMs,
        bucket,
        agentVisibility,
        pricing,
        t,
        local: localResults,
        remote: null,
      });
      // 仅本地即可先渲染；启用远端但本地无任何数据时保持加载态等远端合并
      if (built.data.agents.length > 0 || !wantRemote) {
        setReport(built.data);
        setStale(false);
        localRendered = true;
      }
      setLocalLoading(false);
      if (!wantRemote) {
        // 无远端参与时本阶段即终态，直接落缓存
        writeReportCache(REPORT_CACHE_SCOPE, {
          ...built.cache,
          from_ms: fromMs,
          to_ms: toMs,
          bucket,
          fxRate: fx,
          ts: Date.now(),
        });
      }
    } catch (e) {
      if (reqId === loadReqId.current) {
        setError(shortError(e));
        setLocalLoading(false);
        setSyncing(false);
      }
      return;
    }
    if (!wantRemote) return;

    // —— 远端阶段：独立异步，返回后合并进状态（不阻塞本地首屏）——
    try {
      const [rz, rc, rcl, rk, rd, ro, rm] = await Promise.all([
        remoteZaiPromise,
        remoteCodexPromise,
        remoteClaudePromise,
        remoteKimiPromise,
        remoteDshPromise,
        remoteOpencodePromise,
        remoteMinimaxPromise,
      ]);
      if (reqId !== loadReqId.current || !localResults) return;
      const built = assembleReport({
        fromMs,
        toMs,
        bucket,
        agentVisibility,
        pricing,
        t,
        local: localResults,
        remote: {
          zai: rz,
          codex: rc,
          claude: rcl,
          kimi: rk,
          dsh: rd,
          opencodego: ro,
          minimax: rm,
        },
      });
      setReport(built.data);
      setStale(false);
      writeReportCache(REPORT_CACHE_SCOPE, {
        ...built.cache,
        from_ms: fromMs,
        to_ms: toMs,
        bucket,
        fxRate: localResults.fxRate,
        ts: Date.now(),
      });
    } catch (e) {
      // 远端失败不推翻已渲染的本地数据，仅在本地阶段也没有内容时提示
      if (reqId === loadReqId.current && !localRendered) {
        setError(shortError(e));
      }
    } finally {
      if (reqId === loadReqId.current) setSyncing(false);
    }
  }, [agentVisibility, deviceFilter, pricing, syncConfig, syncEnabled, t]);

  useEffect(() => {
    load();
  }, [load]);

  // ===== Agent 筛选（纯前端 useMemo，切换不发请求） =====
  // 筛选胶囊列表：agentVisibility 开启 ∩ 本次加载实际有数据——
  // 没数据的 agent 不出现胶囊，避免点开全 0
  const activeReportAgents = useMemo(() => {
    if (!report) return [];
    return REPORT_AGENT_IDS.filter(
      (id) =>
        agentVisibility[id] &&
        report.agents.some(
          (agent) => agent.id === id && agentHasDataInRange(agent)
        )
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps -- agentHasDataInRange 为组件内函数声明，随 report 闭包更新
  }, [report, agentVisibility]);

  const filteredAgents = useMemo(() => {
    if (!report) return [];
    if (agentScope === "all" || !agentVisibility[agentScope]) {
      return report.agents;
    }
    return report.agents.filter((agent) => agent.id === agentScope);
  }, [report, agentScope, agentVisibility]);

  // 选中的 agent 被设置页关闭（visibility 隐藏）或本次数据为空时
  // 回退「全部」，胶囊高亮同步
  useEffect(() => {
    if (agentScope === "all") return;
    const hasData = (report?.agents ?? []).some(
      (agent) => agent.id === agentScope && agentHasDataInRange(agent)
    );
    if (!agentVisibility[agentScope] || !hasData) {
      setAgentScope("all");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 同上
  }, [agentVisibility, agentScope, report]);

  // ===== 按日聚合（所选 agent，全部=合并） =====
  // dailyMap = 报告范围切片（KPI / 活跃度 / 趋势图共用）；
  // dailyAll = 89 天热力图窗口（趋势查询扩到 89 天后独立于报告范围）。
  const dailyMap = useMemo(() => {
    const map = new Map<string, DayTotals>();
    if (!report) return map;
    for (const agent of filteredAgents) {
      for (const point of agent.trend) {
        const dayStart = labelToDayStart(
          point.label,
          report.from_ms,
          report.to_ms,
          report.bucket
        );
        if (dayStart == null) continue;
        const key = localDateStr(dayStart);
        const previous = map.get(key) ?? emptyDayTotals();
        previous.tokens += point.total_tokens;
        previous.requests += point.requests;
        previous.costCny += point.cost_cny;
        previous.costUsd += point.cost_usd;
        map.set(key, previous);
      }
    }
    return map;
  }, [report, filteredAgents]);

  const dailyAll = useMemo(() => {
    const map = new Map<string, DayTotals>();
    if (!report) return map;
    // 一年热力图窗口（与 heat memo 同款：52 周前的周一 → 今天）
    const heatTo = startOfLocalDay(Date.now());
    const anchor = new Date(heatTo);
    anchor.setDate(anchor.getDate() - 52 * 7);
    const heatFrom = weekStartOf(anchor.getTime());
    for (const agent of filteredAgents) {
      for (const point of agent.trend) {
        // hour 桶标签无日期，归属报告起始日（该情形下热力图隐藏，防御兜底）
        const dayStart =
          report.bucket === "hour"
            ? startOfLocalDay(report.from_ms)
            : labelToDayStart(point.label, heatFrom, heatTo, report.bucket);
        if (dayStart == null) continue;
        const key = localDateStr(dayStart);
        const previous = map.get(key) ?? emptyDayTotals();
        previous.tokens += point.total_tokens;
        previous.requests += point.requests;
        previous.costCny += point.cost_cny;
        previous.costUsd += point.cost_usd;
        map.set(key, previous);
      }
    }
    return map;
  }, [report, filteredAgents]);

  /** 单 agent 在报告范围内是否有用量（胶囊过滤与回退共用，范围切片口径）。
   *  函数声明式：允许在先于定义的 useMemo 工厂内引用。 */
  function agentHasDataInRange(agent: ReportAgent): boolean {
    if (!report) return false;
    return agent.trend.some((point) => {
      const day = labelToDayStart(
        point.label,
        report.from_ms,
        report.to_ms,
        report.bucket
      );
      return day != null && (point.total_tokens > 0 || point.requests > 0);
    });
  }

  // ===== KPI 行（趋势查询扩到 89 天后，总量/花费一律从报告范围切片的
  // 日聚合求和，与页面所选范围严格一致） =====
  const kpi = useMemo(() => {
    let tokens = 0;
    let costCny = 0;
    let costUsd = 0;
    let requests = 0;
    let activeDays = 0;
    for (const totals of dailyMap.values()) {
      tokens += totals.tokens;
      costCny += totals.costCny;
      costUsd += totals.costUsd;
      requests += totals.requests;
      if (totals.tokens > 0) activeDays += 1;
    }
    return { tokens, costCny, costUsd, requests, activeDays };
  }, [dailyMap]);

  // ===== 活跃度汇总（累计 / 峰值 / 日均 / 连续天数） =====
  const activity = useMemo(() => {
    const startDay = report ? startOfLocalDay(report.from_ms) : 0;
    const endDay = report ? startOfLocalDay(report.to_ms) : 0;
    let total = 0;
    let activeDays = 0;
    let peak: { key: string; tokens: number } | null = null;
    for (const [key, totals] of dailyMap) {
      total += totals.tokens;
      if (totals.tokens > 0) activeDays += 1;
      if (!peak || totals.tokens > peak.tokens) peak = { key, tokens: totals.tokens };
    }
    const streaks = computeStreaks(dailyMap, startDay, endDay);
    return {
      total,
      peak,
      dailyAvg: activeDays > 0 ? total / activeDays : 0,
      current: streaks.current,
      longest: streaks.longest,
    };
  }, [dailyMap, report]);

  // ===== Token 活动热力图：固定一年全量视图（52 周前的周一 → 今天，
  //       约 53 列），独立于报告范围；每日 / 每周 / 累计 三档粒度都在
  //       一年窗口上计算，数据保留期外（>89 天）为 0 档空档格 =====
  const heat = useMemo(() => {
    // 一年窗口：52 周前的同星期回退到周一，到今天恰好约 53 列
    const heatTo = startOfLocalDay(Date.now());
    const anchor = new Date(heatTo);
    anchor.setDate(anchor.getDate() - 52 * 7);
    const heatFrom = weekStartOf(anchor.getTime());
    const columns = buildHeatColumns(heatFrom, heatTo);
    for (const column of columns) {
      for (const cell of column.cells) {
        if (!cell) continue;
        const totals = dailyAll.get(cell.key);
        cell.tokens = totals?.tokens ?? 0;
        cell.requests = totals?.requests ?? 0;
      }
    }
    // 每周粒度按 ISO 周（列）聚合列内求和
    const weekly = columns.map((column) => {
      let tokens = 0;
      let requests = 0;
      let firstDayMs = 0;
      for (const cell of column.cells) {
        if (!cell) continue;
        tokens += cell.tokens;
        requests += cell.requests;
        if (!firstDayMs) firstDayMs = cell.dayMs;
      }
      return { startMs: column.startMs, tokens, requests, firstDayMs };
    });
    // 累计粒度：每格 = 89 天窗口起始到该日的累计总量（单调递增，呈现增长过程）
    const cum = new Map<string, { tokens: number; requests: number }>();
    let accTokens = 0;
    let accRequests = 0;
    for (const column of columns) {
      for (const cell of column.cells) {
        if (!cell) continue;
        accTokens += cell.tokens;
        accRequests += cell.requests;
        cum.set(cell.key, { tokens: accTokens, requests: accRequests });
      }
    }
    const dayValues: number[] = [];
    const weekValues: number[] = [];
    const cumValues: number[] = [];
    for (const column of columns) {
      let weekTokens = 0;
      for (const cell of column.cells) {
        if (!cell) continue;
        if (cell.tokens > 0) dayValues.push(cell.tokens);
        weekTokens += cell.tokens;
        const cumulative = cum.get(cell.key);
        if (cumulative && cumulative.tokens > 0) cumValues.push(cumulative.tokens);
      }
      if (weekTokens > 0) weekValues.push(weekTokens);
    }
    // 相对峰值分档：max 取当前粒度的峰值（每日=单日、每周=单周、累计=累计值）
    const max =
      heatGranularity === "week"
        ? Math.max(0, ...weekValues)
        : heatGranularity === "cum"
          ? Math.max(0, ...cumValues)
          : Math.max(0, ...dayValues);
    return { columns, weekly, cum, max, hasData: dayValues.length > 0 };
  }, [dailyAll, heatGranularity]);
  const heatHasData = !!heat?.hasData;

  // 月份轴标签（一年约 12 个）：月首列间隔不足 2 列时跳过防文字重叠
  // （列宽 14px < 标签宽），自然月距 4-5 周时首尾标签不受影响
  const monthLabels: (string | null)[] = (() => {
    const shown: (string | null)[] = heat
      ? heat.columns.map(() => null)
      : [];
    if (!heat) return shown;
    let lastShown = -3;
    heat.columns.forEach((_column, ci) => {
      const firstMs = heat.weekly[ci].firstDayMs;
      if (!firstMs) return;
      const month = new Date(firstMs).getMonth();
      const prevFirstMs = ci > 0 ? heat.weekly[ci - 1].firstDayMs : 0;
      const prevMonth = prevFirstMs ? new Date(prevFirstMs).getMonth() : -1;
      if (month !== prevMonth && ci - lastShown >= 2) {
        shown[ci] = monthShortLabel(month, locale);
        lastShown = ci;
      }
    });
    return shown;
  })();

  // 热力图横向滚动：挂载/数据就绪（含缓存秒显）/粒度切换后默认停在最右端
  // （今天），并按位置同步两侧遮罩；后台刷新不改变 hasData 时不打断用户
  // 当前滚动位置
  const updateHeatScroll = useCallback(() => {
    const el = heatScrollRef.current;
    if (!el) return;
    setHeatCanLeft(el.scrollLeft > 4);
    setHeatCanRight(el.scrollWidth - el.clientWidth - el.scrollLeft > 4);
  }, []);
  useEffect(() => {
    const el = heatScrollRef.current;
    if (el) {
      el.scrollLeft = el.scrollWidth;
      updateHeatScroll();
    }
  }, [heatGranularity, heatHasData, updateHeatScroll]);

  // ===== 用量趋势：按模型 × 按日 token 序列（by_model 桶聚合） =====
  const modelDaily = useMemo(() => {
    const perDay = new Map<string, Map<string, number>>();
    // key → 模型/来源元数据（同 model_id 不同 provider 时图例需加前缀区分；
    // agentId 供 provider 未登记时回退来源名）
    const names = new Map<
      string,
      { modelId: string; providerId: string; agentId: ReportAgentSourceId }
    >();
    if (!report) return { perDay, names };
    for (const agent of filteredAgents) {
      for (const point of agent.trend) {
        if (!point.by_model || point.by_model.length === 0) continue;
        const dayStart = labelToDayStart(
          point.label,
          report.from_ms,
          report.to_ms,
          report.bucket
        );
        if (dayStart == null) continue;
        const dayKey = localDateStr(dayStart);
        let dayMap = perDay.get(dayKey);
        if (!dayMap) {
          dayMap = new Map();
          perDay.set(dayKey, dayMap);
        }
        for (const model of point.by_model) {
          const key = model.provider_id + "|" + model.model_id;
          dayMap.set(key, (dayMap.get(key) ?? 0) + model.total_tokens);
          if (!names.has(key)) {
            names.set(key, {
              modelId: model.model_id,
              providerId: model.provider_id,
              agentId: agent.id,
            });
          }
        }
      }
    }
    return { perDay, names };
  }, [report, filteredAgents]);

  // 趋势图窗口：报告范围内最后 N 天（切换只影响本图表）
  const trendDays = useMemo(() => {
    if (!report) return [];
    const start = startOfLocalDay(report.from_ms);
    const end = startOfLocalDay(report.to_ms);
    const days: number[] = [];
    const cursor = new Date(start);
    while (cursor.getTime() <= end && days.length < 400) {
      days.push(cursor.getTime());
      cursor.setDate(cursor.getDate() + 1);
    }
    return days.slice(-trendWindow);
  }, [report, trendWindow]);

  // 系列：窗口内 token 总量 Top N + 「其他」（7 日 Top6 / 30 日 Top3，
  // 窄窗下 30 天 × 多系列会把柱压成不可读的细丝）
  const trendSeries = useMemo<TrendSeries[]>(() => {
    if (!report || trendDays.length === 0) return [];
    const totals = new Map<string, number>();
    for (const dayMs of trendDays) {
      const dayMap = modelDaily.perDay.get(localDateStr(dayMs));
      if (!dayMap) continue;
      for (const [key, tokens] of dayMap) {
        totals.set(key, (totals.get(key) ?? 0) + tokens);
      }
    }
    const maxSeries = trendWindow <= 7 ? 6 : 3;
    const top = [...totals.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, maxSeries);
    const topKeys = new Set(top.map(([key]) => key));
    let otherTokens = 0;
    for (const [key, tokens] of totals) {
      if (!topKeys.has(key)) otherTokens += tokens;
    }
    // 同一 model_id 来自多个来源（如 zai 与 codex 都用 GLM-5.3）时图例
    // 会重名——重复的模型名加 agent 品牌前缀区分（Z.ai GLM-5.3 / Codex GLM-5.3）。
    // 按**展示名**统计：展示名已去掉第三方 CLI 拼进 id 的 provider 配置名前缀
    // （zhipuai-coding-plan/glm-5.3-flash → glm-5.3-flash），不同完整 id 也可能
    // 折叠成同一个标签，那种情况同样要靠来源前缀区分。
    const dupModels = new Set<string>();
    {
      const labelCounts = new Map<string, number>();
      for (const [key] of top) {
        const modelId = modelDaily.names.get(key)?.modelId ?? key;
        const label = displayModelName(modelId);
        labelCounts.set(label, (labelCounts.get(label) ?? 0) + 1);
      }
      for (const [label, count] of labelCounts) {
        if (count > 1) dupModels.add(label);
      }
    }
    const series: TrendSeries[] = top.map(([key, total], index) => {
      const meta = modelDaily.names.get(key);
      const modelLabel = meta ? displayModelName(meta.modelId) : key;
      const label =
        meta && dupModels.has(modelLabel)
          ? `${sourcePrefixOf(meta.agentId, meta.providerId)} ${modelLabel}`
          : modelLabel;
      return {
        key,
        label,
        colorCls: SERIES_CLASSES[index % SERIES_CLASSES.length],
        total,
      };
    });
    if (otherTokens > 0) {
      series.push({
        key: "__other__",
        label: t("report.trend.other"),
        colorCls: OTHER_SERIES_CLASS,
        total: otherTokens,
      });
    }
    return series;
  }, [report, trendDays, trendWindow, modelDaily, t]);
  const topModelKeys = useMemo(
    () =>
      new Set(
        trendSeries.filter((s) => s.key !== "__other__").map((s) => s.key)
      ),
    [trendSeries]
  );

  // 按可见系列展开每日分组数据（勾选图例即时重算，不发请求）
  const trend = useMemo(() => {
    const visible = trendSeries.filter((s) => !hiddenSeries.has(s.key));
    const visibleKeys = new Set(visible.map((s) => s.key));
    const groups: TrendGroup[] = trendDays.map((dayMs) => {
      const dayMap = modelDaily.perDay.get(localDateStr(dayMs));
      const values: Record<string, number> = {};
      let total = 0;
      for (const s of trendSeries) {
        if (!visibleKeys.has(s.key)) continue;
        let value = 0;
        if (s.key === "__other__") {
          if (dayMap) {
            for (const [key, tokens] of dayMap) {
              if (!topModelKeys.has(key)) value += tokens;
            }
          }
        } else {
          value = dayMap?.get(s.key) ?? 0;
        }
        values[s.key] = value;
        total += value;
      }
      return { dayMs, key: localDateStr(dayMs), values, total };
    });
    const maxValue = Math.max(
      1,
      ...groups.flatMap((group) => Object.values(group.values))
    );
    return { groups, visible, maxValue };
  }, [trendDays, trendSeries, hiddenSeries, modelDaily, topModelKeys]);

  // 分享卡所需的按日序列（dayMs 升序）：热力图与活跃天数共用同一聚合口径
  const dailySeries = useMemo(() => {
    const out: {
      dayMs: number;
      tokens: number;
      requests: number;
      costCny: number;
      costUsd: number;
    }[] = [];
    for (const [key, totals] of dailyMap) {
      const dayMs = parseLocalDate(key);
      if (Number.isFinite(dayMs)) {
        out.push({
          dayMs,
          tokens: totals.tokens,
          requests: totals.requests,
          costCny: totals.costCny,
          costUsd: totals.costUsd,
        });
      }
    }
    return out.sort((a, b) => a.dayMs - b.dayMs);
  }, [dailyMap]);

  const toggleSeries = (key: string) => {
    setHiddenSeries((previous) => {
      const next = new Set(previous);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  // 导出 Markdown（useMemo：不随无关渲染重复生成全文；全量口径）
  const markdown = useMemo(
    () => buildMarkdown(report, currency, pricing, t, locale),
    [report, currency, pricing, t, locale]
  );
  // 文件名拼入实际数据范围（以已加载 report 为准，未加载时退固定口径）
  const fallbackRange = useMemo(() => fixedReportRange(), []);
  const rangeFromMs = report?.from_ms ?? fallbackRange.fromMs;
  const rangeToMs = report?.to_ms ?? fallbackRange.toMs;
  const rangeText = localDateStr(rangeFromMs) + "~" + localDateStr(rangeToMs);
  const filename = t("report.file.custom") + rangeText + ".md";

  const handleCopy = async () => {
    try {
      await writeText(markdown);
      setDoneFlash(t("report.copied"));
      setTimeout(() => setDoneFlash(null), 1800);
    } catch (e) {
      setError(shortError(e));
    }
  };

  const handleSave = async () => {
    setError(null);
    try {
      await saveReport(markdown, filename);
      setDoneFlash(t("report.savedOpened"));
      setTimeout(() => setDoneFlash(null), 1800);
    } catch (e) {
      setError(shortError(e));
    }
  };

  return (
    <div className="report-scope flex-1 min-h-0 flex flex-col">
      {/* 工具区：设备筛选 + 分享/刷新（时间范围选择已按需求移除，
          报告口径固定为 89 天全量，趋势图自带近 7/30 天切换）。
          近实底面板色阻断滚动内容从半透明 header 后穿出（KPI 卡断片观感）；
          header 非 sticky，随面板固定仅因 flex 布局。 */}
      <div className="px-3 pt-2.5 pb-2 border-b border-slate-900/8 shrink-0 bg-[rgb(var(--panel-bg)/0.92)] backdrop-blur-sm">
        <div className="flex items-center justify-between gap-2">
          {syncEnabled ? (
            <select
              value={deviceFilter}
              onChange={(event) => setDeviceFilter(event.target.value)}
              className="input-box num flex-1 min-w-0 text-[10px] py-1"
              title={t("stats.deviceFilter")}
            >
              <option value="all">{t("report.allDevices")}</option>
              <option value="local">
                {syncConfig?.device_name
                  ? t("stats.deviceLocalName", { name: syncConfig.device_name })
                  : t("stats.deviceLocal")}
              </option>
              {remoteDevices.filter((device) => device.device_id !== syncConfig?.device_id).map((device) => (
                <option key={device.device_id} value={device.device_id}>
                  {t("common.deviceOption", {
                    name: device.device_name,
                    id: device.device_id.slice(0, 6),
                  })}
                </option>
              ))}
            </select>
          ) : (
            <span />
          )}
          <div className="flex items-center gap-0.5 shrink-0">
            <button
              onClick={() => setShareOpen(true)}
              className="toolbar-btn"
              title={t("share.button")}
              aria-label={t("share.button")}
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinejoin="round"
                className="h-3 w-3"
                aria-hidden
              >
                <path d="M12 3l2.2 5.8L20 11l-5.8 2.2L12 19l-2.2-5.8L4 11l5.8-2.2L12 3z" />
              </svg>
            </button>
            <button
              onClick={load}
              disabled={localLoading}
              className="toolbar-btn"
              title={t("report.refresh")}
              aria-label={t("report.refresh")}
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                className={`h-3 w-3 ${localLoading ? "animate-spin" : ""}`}
                aria-hidden
              >
                <path d="M21 12a9 9 0 1 1-2.64-6.36" />
                <path d="M21 3v6h-6" />
              </svg>
            </button>
          </div>
        </div>
      </div>

      {(error || (report?.warnings.length ?? 0) > 0) && (
        <div className="px-3 pt-2 space-y-1">
          {error && <AlertBanner>{error}</AlertBanner>}
          {report?.warnings.length ? (
            <AlertBanner type="warning">
              {report.warnings.join(locale === "zh" ? "；" : "; ")}
            </AlertBanner>
          ) : null}
        </div>
      )}

      <PageBody className="pb-3">
        {report === null && (localLoading || syncing) ? (
          <LoadingState
            text={
              localLoading
                ? t("report.loadingLocal")
                : t("report.loadingRemote")
            }
          />
        ) : !report || report.agents.length === 0 ? (
          <SectionCard className="report-card">
            <div className="py-6 text-center">
              <div className="text-sm text-slate-800/80">{t("report.emptyTitle")}</div>
              <div className="text-[10px] report-sub leading-relaxed mt-1.5">{t("report.emptyHint")}</div>
            </div>
          </SectionCard>
        ) : (
          <div className="page-stack">
            {/* 分项加载提示：固定高度占位，状态出现/消失不引起下方内容跳动 */}
            <div className="min-h-[14px] flex items-center gap-2.5 flex-wrap text-[9px] report-status">
              {stale && <span>{t("report.cachedHint")}</span>}
              {localLoading && <span>{t("report.loadingLocal")}</span>}
              {syncing && (
                <span className="flex items-center gap-1">
                  <span className="inline-block w-1 h-1 rounded-full bg-sky-500 animate-pulse" />
                  {t("report.loadingRemote")}
                </span>
              )}
            </div>

            {/* Agent 筛选行：仅列出实际有数据的 agent（0/1 个时筛选无意义隐藏）；
                纯前端过滤（useMemo），切换不发请求 */}
            {activeReportAgents.length > 1 && (
              <div className="flex items-center gap-0.5 flex-nowrap overflow-x-auto">
                <button
                  onClick={() => setAgentScope("all")}
                  aria-pressed={agentScope === "all"}
                  className={`shrink-0 px-1.5 py-0.5 rounded-full text-[9px] font-medium transition-all duration-150 ${
                    agentScope === "all"
                      ? "pill-active"
                      : "report-pill hover:text-slate-800 hover:bg-slate-900/5"
                  }`}
                >
                  {t("report.filter.all")}
                </button>
                {activeReportAgents.map((id) => {
                  const active = agentScope === id;
                  return (
                    <button
                      key={id}
                      onClick={() => setAgentScope(id)}
                      aria-pressed={active}
                      className={`shrink-0 flex items-center gap-0.5 px-1.5 py-0.5 rounded-full text-[9px] font-medium transition-all duration-150 ${
                        active
                          ? "pill-active"
                          : "report-pill hover:text-slate-800 hover:bg-slate-900/5"
                      }`}
                    >
                      <BrandIcon
                        brand={REPORT_SOURCE_REGISTRY[id].brand}
                        className="h-2.5 w-2.5 shrink-0"
                        style={{ color: AGENT_COLOR[id] }}
                      />
                      {REPORT_SOURCE_REGISTRY[id].label}
                    </button>
                  );
                })}
              </div>
            )}

            {/* KPI 行：总消耗 Token / 总花费 / 总请求数 / 活跃天数（值加 title 展示完整值） */}
            <div className="grid grid-cols-4 gap-1.5">
              <KpiCard
                value={formatTokensParts(kpi.tokens, locale).num}
                unit={formatTokensParts(kpi.tokens, locale).unit}
                title={kpi.tokens.toLocaleString()}
                label={t("report.kpi.tokens")}
              />
              <KpiCard
                value={formatCost(
                  currency === "cny" ? kpi.costCny : kpi.costUsd,
                  currency
                )}
                title={formatCost(
                  currency === "cny" ? kpi.costCny : kpi.costUsd,
                  currency
                )}
                label={t("report.kpi.cost")}
              />
              <KpiCard
                value={formatCountLocale(kpi.requests, locale)}
                title={kpi.requests.toLocaleString()}
                label={t("report.kpi.requests")}
              />
              <KpiCard
                value={String(kpi.activeDays)}
                title={String(kpi.activeDays)}
                label={t("report.kpi.activeDays")}
              />
            </div>

            {/* 活跃度汇总条：五项布局，1px 竖分隔线
                （累计使用时长无会话数据，以日均 Token 替代） */}
            <SectionCard className="report-card" title={t("report.activity")}>
              <div className="flex items-stretch">
                {[
                  {
                    label: t("report.act.total"),
                    num: formatTokensParts(activity.total, locale).num,
                    unit: formatTokensParts(activity.total, locale).unit,
                    tip: undefined,
                    info: false,
                  },
                  {
                    label: t("report.act.peakDay"),
                    num: formatTokensParts(activity.peak?.tokens ?? 0, locale)
                      .num,
                    unit: formatTokensParts(activity.peak?.tokens ?? 0, locale)
                      .unit,
                    tip: activity.peak
                      ? t("report.act.peakTip") +
                        " · " +
                        formatHeatDate(parseLocalDate(activity.peak.key), locale)
                      : undefined,
                    info: true,
                  },
                  {
                    label: t("report.act.dailyAvg"),
                    num: formatTokensParts(activity.dailyAvg, locale).num,
                    unit: formatTokensParts(activity.dailyAvg, locale).unit,
                    tip: undefined,
                    info: false,
                  },
                  {
                    label: t("report.act.streakCurrent"),
                    num: String(activity.current),
                    unit: t("report.act.daysUnit"),
                    tip: undefined,
                    info: false,
                  },
                  {
                    label: t("report.act.streakLongest"),
                    num: String(activity.longest),
                    unit: t("report.act.daysUnit"),
                    tip: undefined,
                    info: false,
                  },
                ].map((item, index) => (
                  <div
                    key={item.label}
                    title={item.tip}
                    className={`flex-1 min-w-0 text-center py-0.5 px-0.5 ${
                      index > 0 ? "border-l border-slate-900/8" : ""
                    }`}
                  >
                    <div className="flex items-baseline justify-center gap-px min-w-0">
                      <span className="num text-[13px] font-semibold text-slate-900/85 truncate">
                        {item.num}
                      </span>
                      {item.unit && (
                        <span className="num text-[10px] font-medium text-slate-900/60 shrink-0">
                          {item.unit}
                        </span>
                      )}
                      {item.info && (
                        <svg
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          strokeWidth="2"
                          className="h-2 w-2 shrink-0 report-sub"
                          aria-hidden
                        >
                          <circle cx="12" cy="12" r="9" />
                          <path strokeLinecap="round" d="M12 11v5M12 7.8v.2" />
                        </svg>
                      )}
                    </div>
                    <div className="text-[9px] report-sub mt-0.5 truncate">
                      {item.label}
                    </div>
                  </div>
                ))}
              </div>
            </SectionCard>

            {/* Token 活动热力图：固定一年全量视图（约 53 列，列=周、行=周一~周日），
                窄窗横向滚动、默认停在最右端（今天）；每日/每周/累计三档粒度 */}
            <SectionCard
              className="report-card"
              title={t("report.tokenActivity")}
              action={
                <div className="flex items-center gap-1.5">
                  {/* 热力图固定一年视图，与报告范围无关——明示消除误读 */}
                  <span className="text-[10px] report-dim whitespace-nowrap">
                    {t("report.heat.window")}
                  </span>
                  <Segmented<"day" | "week" | "cum">
                    options={[
                      { key: "day", label: t("report.heat.daily") },
                      { key: "week", label: t("report.heat.weekly") },
                      { key: "cum", label: t("report.heat.cum") },
                    ]}
                    value={heatGranularity}
                    onChange={setHeatGranularity}
                  />
                </div>
              }
            >
              {!heat || !heat.hasData ? (
                <div className="text-[10px] report-sub py-5 text-center">
                  {t("report.heat.empty")}
                </div>
              ) : (
                <>
                  {/* 横向滚动（隐藏滚动条），格网 + 月份轴一起滚；两侧渐隐
                      遮罩按滚动位置条件渲染（滚到边即消失，不盖真实格子） */}
                  <div className="relative">
                    <div
                      ref={heatScrollRef}
                      className="hide-scrollbar overflow-x-auto"
                      onScroll={updateHeatScroll}
                    >
                      <div className="w-max pr-2">
                        {/* 14px 小方格细密网格（53 列 ≈ 900px，初始视口
                            停在今天） */}
                        <div className="flex gap-[3px]">
                          {heat.columns.map((column, ci) => (
                            <div
                              key={column.startMs}
                              className="w-[14px] flex flex-col gap-[3px]"
                            >
                              {column.cells.map((cell, ri) => {
                                if (!cell) {
                                  return (
                                    <div
                                      key={ri}
                                      className="w-[14px] h-[14px]"
                                    />
                                  );
                                }
                                // 每周=列内合计统一分档；累计=一年窗口起始到该日的累计值
                                const weekly = heatGranularity === "week";
                                const view = weekly
                                  ? heat.weekly[ci]
                                  : heatGranularity === "cum"
                                    ? heat.cum.get(cell.key)!
                                    : cell;
                                const line1 = weekly
                                  ? t("report.heat.tooltipWeek", {
                                      date: formatHeatDate(
                                        heat.weekly[ci].firstDayMs,
                                        locale
                                      ),
                                    })
                                  : heatGranularity === "cum"
                                    ? t("report.heat.tooltipCum", {
                                        date: formatHeatDate(cell.dayMs, locale),
                                      })
                                    : formatHeatDate(cell.dayMs, locale);
                                return (
                                  <HeatCell
                                    key={cell.key}
                                    level={heatLevel(view.tokens, heat.max)}
                                    tokens={view.tokens}
                                    requests={view.requests}
                                    line1={line1}
                                    rowIndex={ri}
                                    scrollElRef={heatScrollRef}
                                  />
                                );
                              })}
                            </div>
                          ))}
                        </div>
                        {/* 底部月份轴：随列横向滚动，标签锚在月份首列内左侧 */}
                        <div className="flex gap-[3px] mt-1">
                          {heat.columns.map((column, ci) => (
                            <div
                              key={column.startMs}
                              className="w-[14px] text-[8px] num report-dim whitespace-nowrap text-left"
                            >
                              {monthLabels[ci] ?? ""}
                            </div>
                          ))}
                        </div>
                      </div>
                    </div>
                    {/* 两侧渐隐遮罩：仅对应方向仍有可滚动内容时显示 */}
                    {heatCanLeft && (
                      <div className="heat-fade-left pointer-events-none absolute inset-y-0 left-0 w-4" />
                    )}
                    {heatCanRight && (
                      <div className="heat-fade-right pointer-events-none absolute inset-y-0 right-0 w-4" />
                    )}
                  </div>
                  <div className="flex items-center justify-end gap-1 mt-1.5 text-[9px] report-dim">
                    <span>{t("report.heat.less")}</span>
                    {[0, 1, 2, 3, 4].map((level) => (
                      <span
                        key={level}
                        className={`w-2 h-2 rounded-[2px] heat-level-${level} ${
                          level === 0 ? "heat-legend-empty" : ""
                        }`}
                      />
                    ))}
                    <span>{t("report.heat.more")}</span>
                  </div>
                </>
              )}
            </SectionCard>

            {/* 用量趋势：按天分组柱状图（每模型一柱）+ 可勾选图例（内联汇总值）；
                窗口档位在全量数据的末尾切片（近 7 / 30 天切换只影响本图表） */}
            <SectionCard
              className="report-card"
              title={t("common.usageTrend")}
              action={
                <Segmented<number>
                  options={[
                    { key: 7, label: t("report.trend.days7") },
                    { key: 30, label: t("report.trend.days30") },
                  ]}
                  value={trendWindow}
                  onChange={setTrendWindow}
                />
              }
            >
              {trendSeries.length === 0 ? (
                <div className="text-[10px] report-sub py-5 text-center">
                  {modelDaily.perDay.size === 0 && kpi.tokens <= 0
                    ? t("report.noTrend")
                    : t("report.trend.noDaily")}
                </div>
              ) : trend.visible.length === 0 ? (
                /* 全部系列被取消勾选：给提示而不是留一块空白图区 */
                <div className="text-[10px] report-sub py-5 text-center">
                  {t("report.trend.allHidden")}
                </div>
              ) : (
                <>
                  <TrendGroupChart
                    groups={trend.groups}
                    series={trend.visible}
                    maxValue={trend.maxValue}
                    locale={locale}
                  />
                  <div className="flex flex-wrap gap-x-2.5 gap-y-1 mt-2">
                    {trendSeries.map((s) => {
                      const on = !hiddenSeries.has(s.key);
                      return (
                        <button
                          key={s.key}
                          onClick={() => toggleSeries(s.key)}
                          title={s.label}
                          role="checkbox"
                          aria-checked={on}
                          className={`flex items-center gap-1 text-[9px] min-w-0 report-sub ${
                            on ? "" : "opacity-75"
                          }`}
                        >
                          <span
                            className={`w-2 h-2 rounded-[3px] shrink-0 ${s.colorCls} ${
                              on ? "" : "opacity-30"
                            }`}
                          />
                          <span className="truncate">{s.label}</span>
                          <span className="num shrink-0">
                            : {formatTokensLocale(s.total, locale)} Token
                          </span>
                        </button>
                      );
                    })}
                  </div>
                </>
              )}
            </SectionCard>

            {report.notes.map((note) => (
              <div
                key={note}
                className="text-[9px] report-sub leading-relaxed"
              >
                {note}
              </div>
            ))}
          </div>
        )}

        {/* 导出栏：随文档流渲染在滚动内容末尾（不用 PageShell 的固定底栏
            插槽）——滚动中间态不再被栏上缘横切内容；滚动到底时导出栏自然
            出现在趋势卡下方，无任何穿层遮挡 */}
        <div className="mt-2 pt-2 border-t border-slate-900/8 flex items-center justify-between gap-2 text-[10px]">
          <span className="report-footer-text truncate">{doneFlash || filename}</span>
          <div className="flex gap-1.5 shrink-0">
            <BtnSecondary onClick={handleCopy} disabled={!report || localLoading}>{t("report.copy")}</BtnSecondary>
            <BtnPrimary onClick={handleSave} disabled={!report || localLoading}>{t("common.save")}</BtnPrimary>
          </div>
        </div>
      </PageBody>

      {/* 生成分享卡片弹层：复用报告页已加载数据（不再自行拉取数据源） */}
      {shareOpen && report && (
        <ShareCardModal
          onClose={() => setShareOpen(false)}
          pricing={pricing}
          report={{
            fromMs: report.from_ms,
            toMs: report.to_ms,
            bucket: report.bucket,
            fxRate: report.fxRate,
            currency,
            agents: report.agents,
            daily: dailySeries,
          }}
        />
      )}
    </div>
  );
}

/** 热力图列 tooltip 水平对齐：首尾两列内收避免浮层溢出卡片。 */
function heatAlign(columnIndex: number, columnCount: number): "start" | "center" | "end" {
  if (columnIndex < 2) return "start";
  if (columnIndex >= columnCount - 2) return "end";
  return "center";
}

/** KPI 指标卡：大号粗体数值 + 小一档半透明的单位 + 下方小号灰字标签
 *  （title 悬浮展示完整值，窄窗内紧凑数值截断时不丢信息）。 */
function KpiCard({
  value,
  unit = "",
  label,
  title,
}: {
  value: string;
  unit?: string;
  label: string;
  title?: string;
}) {
  return (
    <div
      className="report-card px-1.5 py-2.5 text-center min-w-0"
      title={title}
    >
      <div className="flex items-baseline justify-center gap-px leading-none min-w-0">
        <span className="num text-[15px] font-bold text-slate-900/90 truncate">
          {value}
        </span>
        {unit && (
          <span className="num text-[12px] font-semibold text-slate-900/60 shrink-0">
            {unit}
          </span>
        )}
      </div>
      <div className="text-[9px] report-sub mt-1.5 truncate">{label}</div>
    </div>
  );
}

/** 胶囊分段控件：亮色 sky 实底白字、暗色白色磨砂（.pill-active 双主题），
 *  支持 disabled 档位（如范围不足 30 天时禁「近 30 日」）。 */
function Segmented<T extends string | number>({
  options,
  value,
  onChange,
}: {
  options: { key: T; label: string; disabled?: boolean }[];
  value: T;
  onChange: (next: T) => void;
}) {
  return (
    <div className="flex gap-0.5 p-0.5 rounded-full bg-slate-900/4 shrink-0">
      {options.map((option) => (
        <button
          key={String(option.key)}
          onClick={() => onChange(option.key)}
          disabled={option.disabled}
          aria-pressed={value === option.key}
          className={`px-2 py-0.5 rounded-full text-[9px] font-medium transition-all duration-150 whitespace-nowrap disabled:opacity-40 disabled:cursor-not-allowed ${
            value === option.key
              ? "pill-active"
              : "report-pill hover:text-slate-800"
          }`}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

/** 热力图单元格：固定 14px 小方格、圆角 3px（细密数据格，与 ZCode 一致，
 *  不随卡片宽度拉伸）；分档色走 .heat-level-0~4 主题类；hover 深色气泡
 *  （紧凑两行；行在上半部时向下弹、下半部向上弹；水平方向按锚点在滚动
 *  容器可视区的左右半决定展开方向——右半向左展开、左半向右展开，跟随
 *  滚动位置，保证任何格子 hover 气泡都不越卡片边缘）。 */
function HeatCell({
  level,
  line1,
  tokens,
  requests,
  rowIndex,
  scrollElRef,
}: {
  level: number;
  line1: string;
  tokens: number;
  requests: number;
  /** 行号（0=周一 … 6=周日），决定 tooltip 垂直弹出方向 */
  rowIndex: number;
  /** 横向滚动容器 ref：测量可视区中线用 */
  scrollElRef: RefObject<HTMLDivElement | null>;
}) {
  const { t, locale } = useI18n();
  const [hover, setHover] = useState(false);
  // 第 1-3 行向下弹，第 4-7 行向上弹
  const flipDown = rowIndex <= 2;
  // 锚点在可视区右半 → 气泡向左展开（右缘贴格子右缘）
  const [flipLeft, setFlipLeft] = useState(false);
  const handleEnter = (event: ReactMouseEvent<HTMLDivElement>) => {
    const wrap = scrollElRef.current;
    if (wrap) {
      const cellRect = event.currentTarget.getBoundingClientRect();
      const wrapRect = wrap.getBoundingClientRect();
      const cellCenterX = cellRect.left + cellRect.width / 2;
      const viewMidX = wrapRect.left + wrapRect.width / 2;
      setFlipLeft(cellCenterX > viewMidX);
    }
    setHover(true);
  };
  return (
    <div
      className="relative w-[14px] h-[14px]"
      onMouseEnter={handleEnter}
      onMouseLeave={() => setHover(false)}
    >
      <div
        className={`h-full w-full rounded-[3px] heat-level-${level} ${
          hover ? "ring-1 ring-slate-900/30" : ""
        }`}
      />
      {hover && (
        <div
          className={`absolute z-20 whitespace-nowrap rounded-lg bg-[rgba(10,12,20,0.88)] px-1.5 py-1 text-[10px] leading-tight text-[#e5eaf0] pointer-events-none shadow-lg ${
            flipLeft ? "right-0" : "left-0"
          } ${flipDown ? "top-full mt-1" : "bottom-full mb-1"}`}
        >
          <div className="num font-medium">{line1}</div>
          <div className="num opacity-75">
            {t("report.heat.tooltipLine", {
              tokens: formatTokensLocale(tokens, locale),
              n: requests,
            })}
          </div>
        </div>
      )}
    </div>
  );
}

/** 用量趋势分组柱状图：按天分组、每天并列多柱（每系列一柱），纯 CSS 实现。
 *  X 轴日期标签用 M/D 短格式（7 日逐日标，30 日隔约 5 日，首中尾恒显）。 */
function TrendGroupChart({
  groups,
  series,
  maxValue,
  locale,
}: {
  groups: TrendGroup[];
  series: TrendSeries[];
  maxValue: number;
  locale: Locale;
}) {
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const n = groups.length;
  // 标签步长：目标约显示 6~8 个标签，避免文字重叠。
  const labelStep = n <= 8 ? 1 : Math.max(2, Math.ceil(n / 7));
  return (
    <div>
      <div className="flex items-end gap-px h-24">
        {groups.map((group, gi) => {
          const isHover = hoverIdx === gi;
          const align = heatAlign(gi, n);
          return (
            <div
              key={group.key}
              className="flex-1 h-full flex items-end justify-center gap-px min-w-0 relative"
              onMouseEnter={() => setHoverIdx(gi)}
              onMouseLeave={() => setHoverIdx(null)}
            >
              {isHover && (
                <div
                  className={`absolute top-0 z-20 whitespace-nowrap rounded-lg bg-[rgba(10,12,20,0.88)] px-1.5 py-1 text-[10px] leading-tight text-[#e5eaf0] pointer-events-none shadow-lg ${
                    align === "start"
                      ? "left-0"
                      : align === "end"
                        ? "right-0"
                        : "left-1/2 -translate-x-1/2"
                  }`}
                >
                  <div className="num font-medium">
                    {formatHeatDate(group.dayMs, locale)}
                  </div>
                  <div className="num">
                    {formatTokensLocale(group.total, locale)} Token
                  </div>
                  {series.map((s) => (
                    <div
                      key={s.key}
                      className="num opacity-75 flex items-center gap-1"
                    >
                      <span
                        className={`inline-block w-1.5 h-1.5 rounded-full shrink-0 ${s.colorCls}`}
                      />
                      <span className="truncate max-w-[110px]">{s.label}</span>
                      <span>
                        {formatTokensLocale(group.values[s.key] ?? 0, locale)}
                      </span>
                    </div>
                  ))}
                </div>
              )}
              {series.map((s) => {
                const value = group.values[s.key] ?? 0;
                const h = maxValue > 0 ? (value / maxValue) * 100 : 0;
                return (
                  <div
                    key={s.key}
                    className={`flex-1 min-w-px rounded-t-[2px] transition-all duration-300 ${s.colorCls}`}
                    style={{
                      height: `${Math.max(h, value > 0 ? 3 : 0)}%`,
                      maxWidth: n <= 8 ? "12px" : undefined,
                      opacity: isHover ? 1 : 0.88,
                    }}
                  />
                );
              })}
            </div>
          );
        })}
      </div>
      {/* X 轴日期标签：M/D 短格式，隔步显示，首尾恒显 */}
      <div className="flex gap-px mt-1">
        {groups.map((group, gi) => {
          const isFirst = gi === 0;
          const isLast = gi === n - 1;
          const farFromLast = n - 1 - gi >= Math.max(1, Math.ceil(labelStep / 2));
          const showLabel =
            isFirst || isLast || (gi % labelStep === 0 && farFromLast);
          // "YYYY-MM-DD" → "M/D"（去前导零，跟随数字习惯的本地化短格式）
          const [, mm, dd] = group.key.split("-");
          return (
            <span
              key={group.key}
              className={`flex-1 text-[8px] num min-w-0 whitespace-nowrap text-center ${
                isLast ? "text-sky-600 font-medium" : "report-dim"
              } ${showLabel ? "" : "invisible"}`}
            >
              {Number(mm)}/{Number(dd)}
            </span>
          );
        })}
      </div>
    </div>
  );
}

/** 报告范围内按模型切片聚合的 TOP 行（导出 Markdown 与分享卡共用；
 *  agents.trend 为查询口径，可能大于报告范围，必须逐点切片）。
 *  无有效价格的模型 cost 为 null（展示侧不标注花费）。 */
export interface RangeModelRow {
  agentId: ReportAgentSourceId;
  agentLabel: string;
  modelId: string;
  tokens: number;
  cost: number | null;
}

export function rangeModelRows(
  data: ShareReportData,
  pricing: PricingConfig,
  currency: Currency,
  limit: number
): RangeModelRow[] {
  const perModel = new Map<
    string,
    {
      agentId: ReportAgentSourceId;
      modelId: string;
      tokens: number;
      input: number;
      output: number;
      cacheRead: number;
    }
  >();
  for (const agent of data.agents) {
    for (const point of agent.trend) {
      const day = labelToDayStart(
        point.label,
        data.fromMs,
        data.toMs,
        data.bucket
      );
      if (day == null) continue;
      for (const m of point.by_model ?? []) {
        const key = m.provider_id + "|" + m.model_id;
        const existing = perModel.get(key);
        if (existing) {
          existing.tokens += m.total_tokens;
          existing.input += m.input_tokens ?? 0;
          existing.output += m.output_tokens ?? 0;
          existing.cacheRead += m.cache_read_tokens ?? 0;
        } else {
          perModel.set(key, {
            agentId: agent.id,
            modelId: m.model_id,
            tokens: m.total_tokens,
            input: m.input_tokens ?? 0,
            output: m.output_tokens ?? 0,
            cacheRead: m.cache_read_tokens ?? 0,
          });
        }
      }
    }
  }
  return [...perModel.values()]
    .sort((a, b) => b.tokens - a.tokens)
    .slice(0, limit)
    .map((m) => {
      const priced =
        m.tokens > 0 && hasPositivePrice(m.modelId, pricing);
      return {
        agentId: m.agentId,
        agentLabel: REPORT_SOURCE_REGISTRY[m.agentId].label,
        modelId: m.modelId,
        tokens: m.tokens,
        cost: priced
          ? modelCost(
              m.modelId,
              m.input,
              m.output,
              m.cacheRead,
              pricing,
              currency,
              data.fxRate
            )
          : null,
      };
    });
}

/** 报告范围内按 agent 切片聚合的用量行（分享卡 Agent 分布横条用）。 */
export interface RangeAgentRow {
  id: ReportAgentSourceId;
  name: string;
  color: string;
  tokens: number;
  /** 占报告范围 token 总量的比例 0~1 */
  share: number;
}

export function rangeAgentRows(data: ShareReportData): RangeAgentRow[] {
  const totalTokens = data.daily.reduce((s, d) => s + d.tokens, 0);
  return data.agents
    .map((agent) => {
      let tokens = 0;
      for (const point of agent.trend) {
        const day = labelToDayStart(
          point.label,
          data.fromMs,
          data.toMs,
          data.bucket
        );
        if (day == null) continue;
        tokens += point.total_tokens;
      }
      return {
        id: agent.id,
        name: agent.label,
        color: agent.color,
        tokens,
      };
    })
    .filter((row) => row.tokens > 0)
    .sort((a, b) => b.tokens - a.tokens)
    .map((row) => ({
      ...row,
      share: totalTokens > 0 ? row.tokens / totalTokens : 0,
    }));
}

/** 生成报告 Markdown 全文（模式 B：接收 t / locale，报告语言跟随 UI 语言；
 *  口径固定为 89 天全量，汇总全部基于该范围）。 */
function buildMarkdown(
  report: ReportData | null,
  currency: Currency,
  pricing: PricingConfig,
  t: TFn,
  locale: Locale
): string {
  const now = Date.now();
  const title = t("report.md.custom");
  if (!report || report.agents.length === 0) {
    return (
      "📊 ZBar " + title + " · " + localDateStr(now) + "\n\n" + t("report.md.noData")
    );
  }

  const inRange = (point: TrendPoint) =>
    labelToDayStart(
      point.label,
      report.from_ms,
      report.to_ms,
      report.bucket
    ) != null;
  const rangePoints = report.trend.filter(inRange);
  const totalTokens = rangePoints.reduce((s, p) => s + p.total_tokens, 0);
  const totalRequests = rangePoints.reduce((s, p) => s + p.requests, 0);
  const totalCost = rangePoints.reduce(
    (s, p) => s + selectedCost(p, currency),
    0
  );

  const range =
    localDateStr(report.from_ms) + " ~ " + localDateStr(report.to_ms);
  const models = rangeModelRows(
    {
      fromMs: report.from_ms,
      toMs: report.to_ms,
      bucket: report.bucket,
      fxRate: report.fxRate,
      currency,
      agents: report.agents,
      // rangeModelRows 不消费 daily，占位空数组即可
      daily: [],
    },
    pricing,
    currency,
    5
  ).filter((m) => m.tokens > 0);
  const lines: string[] = [];
  lines.push("📊 ZBar " + title + " · " + range);
  lines.push("");
  lines.push(
    t("report.md.summaryLine", {
      cost: formatCost(totalCost, currency),
      tokens: formatTokens(totalTokens),
      requests: totalRequests.toLocaleString(),
    })
  );
  lines.push("");
  lines.push(t("report.agentDist"));
  for (const agent of report.agents) {
    let agentTokens = 0;
    let agentRequests = 0;
    let agentCost = 0;
    for (const point of agent.trend) {
      if (!inRange(point)) continue;
      agentTokens += point.total_tokens;
      agentRequests += point.requests;
      agentCost += selectedCost(point, currency);
    }
    if (agentTokens <= 0 && agentRequests <= 0) continue;
    lines.push(
      "- " +
        t("report.md.agentLine", {
          label: agent.label,
          cost: formatCost(agentCost, currency),
          tokens: formatTokens(agentTokens),
          requests: agentRequests.toLocaleString(),
        })
    );
  }
  if (models.length > 0) {
    lines.push("");
    lines.push(t("report.md.top5"));
    for (const model of models) {
      lines.push(
        "- " +
          t("report.md.modelLine", {
            agent: model.agentLabel,
            model: displayModelName(model.modelId),
            cost:
              model.cost != null ? formatCost(model.cost, currency) : "—",
            tokens: formatTokens(model.tokens),
          })
      );
    }
  }
  if (rangePoints.length > 0) {
    const peak = rangePoints.reduce((best, point) =>
      point.total_tokens > best.total_tokens ? point : best
    );
    lines.push("");
    lines.push(
      t("report.md.tokenPeak", {
        label: peak.label,
        tokens: formatTokens(peak.total_tokens),
      })
    );
  }
  if (report.warnings.length > 0 || report.notes.length > 0) {
    lines.push("");
    lines.push(t("report.md.notes"));
    for (const note of [...report.notes, ...report.warnings]) {
      lines.push("- " + note);
    }
  }
  lines.push("");
  lines.push(
    "> " +
      t("report.md.footer", {
        date: new Date(now).toLocaleString(dateLocale(locale)),
      })
  );
  return lines.join("\n");
}
