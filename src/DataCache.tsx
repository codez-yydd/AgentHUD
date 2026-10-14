import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ReactNode } from "react";
import type {
  AgentQuotaDeltaMap,
  AccountQuotaEntry,
  ClaudeSnapshot,
  CostResult,
  CodexSnapshot,
  CursorSnapshot,
  DeviceInfo,
  DshSnapshot,
  KimiSnapshot,
  MinimaxcodeSnapshot,
  ModelStat,
  OverallStat,
  OpencodeSnapshot,
  PricingConfig,
  ProviderCredentialMeta,
  ProviderQuotaEntry,
  QuotaResult,
  QuotaSnapshot,
  RangePreset,
  RemoteUsage,
  Stats,
  SyncConfig,
  TrendBucket,
  TrendPoint,
} from "./types";
import {
  accountQuotas as fetchAccountQuotas,
  computeCost,
  fetchClaudeUsage,
  fetchCodexUsage,
  fetchCursorUsage,
  fetchDshUsage,
  fetchKimiUsage,
  fetchMinimaxcodeUsage,
  fetchOpencodeUsage,
  fetchQuota,
  getAgentQuotaHistory,
  fetchStats,
  fetchTrend,
  getCursorConfig,
  getQuotaHistory,
  getSyncConfig,
  getTodayDelta,
  fetchProviderQuota,
  listProviderCredentials,
  listRemoteDevices,
  remoteUsage,
  remoteAgentQuotaSnapshots,
  remoteSnapshots,
  showNotification,
  switchAccount,
} from "./api";
import {
  AUTO_SWITCH_DONE_EVENT,
  pickAutoSwitchTarget,
  readAutoSwitchEnabled,
  writeAutoSwitchLog,
} from "./autoSwitch";
import { resolveRange } from "./RangePicker";
import { dateStr } from "./format";
import {
  computeRemoteCost,
  mergeCost,
  mergeStats,
  mergeTrend,
  remoteToStats,
  remoteTrendToLocal,
} from "./merge";
import { loadCache, saveCache } from "./cache";
import { useI18n } from "./i18n";
import { CREDENTIALS_CHANGED_EVENT } from "./agentVisibility";
import {
  calculateAgentQuotaDeltas,
  filterInflatedRemoteCursorSnapshots,
  mergeAgentQuotaSnapshots,
  todayStartMs,
} from "./agentQuota";
import { foldByModelStats, foldCursorModelRows } from "./modelName";

/**
 * 全局数据缓存层（v2：按范围缓存 + 后台定时刷新 + 展示只读）。
 *
 * 核心思路：
 *  - 展示层「只读缓存」：切换时间范围 = 改当前 key → 直接读对应缓存条目，
 *    零请求、秒显。stats/cost/trend/codex/claude/cursor 全部取自当前 key 的缓存。
 *  - 后台独立定时任务：定期把各预设范围（today/1d/7d/30d）的数据刷新进缓存，
 *    与范围切换完全解耦——切回来时数据已在缓存里；用户停留在自定义视图时，
 *    当前 custom 范围（z.ai + Codex + Claude + Cursor）也顺带刷新，保证 lastUpdate 常新。
 *  - 按需补刷：切到无缓存/过期范围（如全新 custom、首次进入、数据老化、配置
 *    就绪）才触发一次加载，之后也进缓存。
 *  - 刷新期间不清空旧值（只设 refreshing），新数据到达后平滑替换 → 无闪烁。
 *  - pricing 未就绪时保留旧 cost，避免空价格表把花费覆盖成 0。
 *  - localStorage 持久化：进程重启后各范围仍秒显。
 */

// 需要后台持续刷新的预设范围（custom 无法预缓存，仅当用户停留在自定义视图时
// 由后台 tick / visibilitychange 顺带刷新当前 custom 范围）
const PRESET_RANGES: RangePreset[] = ["today", "1d", "7d", "30d"];

// custom 缓存条目上限：超过则按 ts 淘汰最老的，避免长期累积撑爆 localStorage
const MAX_CUSTOM_ENTRIES = 8;

/** 由 preset 派生趋势分桶：今日/24h 按小时，更长范围按日 */
function bucketOf(preset: RangePreset): TrendBucket {
  return preset === "today" || preset === "1d" ? "hour" : "day";
}

/** 范围缓存 key：预设用 preset 名，custom 用日期区间 */
function rangeKey(
  preset: RangePreset,
  custom: { from: string; to: string }
): string {
  return preset === "custom" ? `custom:${custom.from}~${custom.to}` : preset;
}

/** 价格表是否为空（未加载占位）——空时不覆盖 cost，避免把花费算成 0 */
function isPricingEmpty(p: PricingConfig): boolean {
  return Object.keys(p.usd).length === 0;
}

/**
 * 淘汰超额的 custom 缓存条目（按 ts 升序删除最老的）。
 * key 形如 `${df}|custom:...` 或 `custom:...`，均含 "custom:"。
 * 预设范围 key（today/1d/7d/30d）固定 4 个，不参与淘汰。
 */
function trimCustomEntries<T extends { ts: number }>(
  map: Record<string, T>,
  max: number
): Record<string, T> {
  const customKeys = Object.keys(map).filter((k) => k.includes("custom:"));
  if (customKeys.length <= max) return map;
  customKeys.sort((a, b) => map[a].ts - map[b].ts);
  const next = { ...map };
  for (const k of customKeys.slice(0, customKeys.length - max)) delete next[k];
  return next;
}

// 缓存过期阈值：超过则按需补刷（略大于后台刷新间隔，避免边界抖动）
const ZAI_STALE_MS = 60_000;
const CURSOR_STALE_MS = 240_000;
const CODEX_STALE_MS = 60_000;
const CLAUDE_STALE_MS = 60_000;
const KIMI_STALE_MS = 60_000;
const DSH_STALE_MS = 60_000;
const OPENCODE_STALE_MS = 60_000;
const MINIMAXCODE_STALE_MS = 60_000;

// ===== 速度口径缓存迁移（V24.1 契约：可加总分子/分母 + speedQuality 标记）=====
// 旧缓存（无版本标记）的 avg_tps/max_tps 由旧算法聚合（逐行速度算术平均、
// 500 t/s 封顶），缺新契约字段时不能冒充 generation 口径。冷启动恢复时对
// zai/claude/kimi 三个含速度的缓存做**针对性失效**：剥离旧口径速度字段
//（TTFT 口径未变，保留 avg_ttft_ms/ttftSampleCount）并把条目 ts 归零，
// 按需补刷与后台 500ms 首刷会立即重拉；trend/cost/token 等无关统计保留，
// 期间速度显示 —（宁缺毋滥，不用旧值顶替）。codex/cursor 无速度字段不涉及。
// 迁移结果落盘后（版本标记 effect 置于各持久化 effect 之后）写入版本标记，
// 下次冷启动不再重复处理。
//
// V3（本机速度权重快照）：V2 时代 mergeStats 合并远端 Token 时未冻结
// localSpeedWeightTokens，合并结果（持久缓存存的就是合并结果）里
// request_average 行的 output_tokens 已混入远端 Token——折叠加权会被远端
// 设备用量污染（远端输出越多，本机 avg_tps 被拉得越偏）。对带
// request_average 速度但无权重快照的 by_model 行剥离速度字段并 ts 归零
// 触发重刷；重刷后的行由 mergeStats 写入快照，本迁移不再命中。overall 行
// 无折叠权重问题、TTFT 族为纯本机口径，均不受污染，不剥离。版本升级为
// 一次性迁移：V2 已处理过的旧口径行（无速度值）对新判定是无操作，幂等。
const SPEED_CONTRACT_VERSION = 3;
const SPEED_VERSION_CACHE_KEY = "zbar-speed-cache-version";

/** 行级判断：有速度值但无任何新契约字段 → 旧口径行（需剥离） */
function isLegacySpeedRow(row: {
  avg_tps?: number | null;
  max_tps?: number | null;
  speedQuality?: string | null;
  speedOutputTokens?: number | null;
  speedGenerationMs?: number | null;
}): boolean {
  if (row.avg_tps == null && row.max_tps == null) return false;
  return (
    row.speedQuality == null &&
    row.speedOutputTokens == null &&
    row.speedGenerationMs == null
  );
}

/** V3 行级判断：request_average 行带速度但无本机权重快照 → 该行
 *  output_tokens 可能已混入远端 Token（V2 时代合并输出），折叠加权会被
 *  污染，需剥离速度触发重刷。仅用于 by_model 行（overall 不折叠）。 */
function isUnweightedRequestAvgRow(row: {
  speedQuality?: string | null;
  localSpeedWeightTokens?: number | null;
}): boolean {
  return (
    row.speedQuality === "request_average" &&
    row.localSpeedWeightTokens == null
  );
}

/** 剥离旧口径速度字段（保留 TTFT 族字段与全部数值统计）。是否需要剥离由
 *  调用方按行级判定决定（overall → isLegacySpeedRow；by_model → 再叠加
 *  isUnweightedRequestAvgRow），本函数只负责剥离本身。 */
function stripLegacySpeedRow<T extends ModelStat | OverallStat>(row: T): T {
  const {
    avg_tps: _avgTps,
    max_tps: _maxTps,
    speedOutputTokens: _num,
    speedGenerationMs: _den,
    speedSampleCount: _cnt,
    speedQuality: _q,
    ...rest
  } = row;
  return rest as T;
}

/** stats 迁移：overall 行按 V1 旧口径判定剥离；by_model 行叠加 V2 无快照
 *  request_average 判定（折叠权重可能被远端 Token 污染） */
function migrateLegacySpeedStats<S extends Stats>(s: S): S {
  return {
    ...s,
    overall: isLegacySpeedRow(s.overall) ? stripLegacySpeedRow(s.overall) : s.overall,
    by_model: s.by_model.map((row) =>
      isLegacySpeedRow(row) || isUnweightedRequestAvgRow(row)
        ? stripLegacySpeedRow(row)
        : row
    ),
  };
}

/** 缓存条目迁移：剥离旧速度并把 ts 归零（触发按需补刷立即重拉） */
function migrateSpeedEntries<
  T extends { ts: number } & (
    | { stats?: Stats | null }
    | { snapshot?: { stats: Stats } | null }
  ),
>(map: Record<string, T>): Record<string, T> {
  const out: Record<string, T> = {};
  for (const [k, e] of Object.entries(map)) {
    if ("stats" in e && e.stats != null) {
      out[k] = { ...e, stats: migrateLegacySpeedStats(e.stats), ts: 0 };
    } else if ("snapshot" in e && e.snapshot?.stats != null) {
      out[k] = {
        ...e,
        snapshot: { ...e.snapshot, stats: migrateLegacySpeedStats(e.snapshot.stats) },
        ts: 0,
      };
    } else {
      out[k] = e;
    }
  }
  return out;
}

/** 模块级一次性判断：当前 localStorage 缓存是否为速度契约升级前的旧数据 */
const LEGACY_SPEED_CACHE =
  (loadCache<number>(SPEED_VERSION_CACHE_KEY) ?? 1) < SPEED_CONTRACT_VERSION;

/** 迁移轮同步写开关：模块加载时判定为旧契约缓存时置 true，版本标记 effect
 *  写完标记后复位。挂载轮的 zai/claude/kimi 持久化 effect 据此改用同步
 *  saveCache，维持「迁移后的缓存先落盘 → 版本标记后落盘」的时序语义（进程
 *  中途被杀时靠「版本=旧 + 缓存=已剥离」幂等重跑迁移，见版本标记 effect 注释）。
 *  标记落盘后本会话的后续更新即恢复防抖异步写；正常路径（版本已最新）恒为
 *  false，不受影响。 */
let speedMigrationSyncWrite = LEGACY_SPEED_CACHE;

/** provider 额度缓存老化阈值（与 120s 通用轮询同频）：面板挂载补刷判定
 *  「无缓存或缓存已老化」用（冷启动 localStorage 恢复的旧缓存也视为旧）。 */
export const PROVIDER_QUOTA_STALE_MS = 120_000;

interface ZaiEntry {
  stats: Stats | null;
  cost: CostResult | null;
  trend: TrendPoint[];
  error: string | null;
  refreshing: boolean;
  ts: number;
}

interface CursorEntry {
  snapshot: CursorSnapshot | null;
  error: string | null;
  refreshing: boolean;
  ts: number;
}

/** Cursor 锚点新鲜度门槛：localStorage 恢复的陈旧缓存不能当锚点——锚点偏低
 *  会把锚点滞后期间真实增长的更高快照误删；实时拉取通常几分钟一轮，10 分钟
 *  足以覆盖正常间隔，同时挡住隔夜/离线恢复的过期缓存。 */
const CURSOR_ANCHOR_MAX_AGE_MS = 10 * 60_000;

/** 从本机 Cursor 缓存构造远端失真快照的过滤锚点：取 ts 最新条目的
 *  plan 百分比与计费周期结束时间。auto/api 两侧锚点独立取值（任一侧可能
 *  无值，如未使用 API 时 api_pct 为 null）；条目过期、plan 缺失或两侧
 *  百分比都无效时返回 null（锚点缺失不过滤，宁缺毋滥）。 */
function buildCursorQuotaAnchors(
  cache: Record<string, CursorEntry>
): {
  autoPct: number | null;
  apiPct: number | null;
  resetAtMs: number | null;
} | null {
  const now = Date.now();
  let latest: CursorEntry | null = null;
  for (const entry of Object.values(cache)) {
    if (!entry.snapshot || entry.ts <= 0) continue;
    if (!latest || entry.ts > latest.ts) latest = entry;
  }
  if (!latest || now - latest.ts > CURSOR_ANCHOR_MAX_AGE_MS) return null;
  const snapshot = latest.snapshot;
  if (!snapshot) return null;
  const plan = snapshot.plan;
  if (!plan) return null;
  const validPct = (value: number | null): number | null =>
    value !== null && Number.isFinite(value) && value > 0 ? value : null;
  const autoPct = validPct(plan.auto_pct);
  const apiPct = validPct(plan.api_pct);
  if (autoPct === null && apiPct === null) return null;
  const resetAtMs = snapshot.billing_cycle_end
    ? Date.parse(snapshot.billing_cycle_end)
    : null;
  return {
    autoPct,
    apiPct,
    resetAtMs: resetAtMs !== null && Number.isFinite(resetAtMs) ? resetAtMs : null,
  };
}

/** Codex / Claude 快照的公共形状（两者结构完全一致，TS 结构化类型互通，
 *  各自的名义类型可直接赋给本接口）。 */
interface AgentSnapshot {
  stats: Stats;
  trend: TrendPoint[];
  rate_limits: {
    plan_type: string | null;
    primary_pct: number | null;
    primary_reset_at: number | null;
    secondary_pct: number | null;
    secondary_reset_at: number | null;
  } | null;
}

interface AgentEntry {
  snapshot: AgentSnapshot | null;
  error: string | null;
  refreshing: boolean;
  ts: number;
}

/** 「本地采集 + 远端合并」四源（Kimi / DSH / OpenCode / MiniMax Code）的
 *  公共快照形状：只有 stats / trend，**不含**订阅额度窗口（各自的额度走
 *  独立链路，见各来源注释）。 */
interface LocalCollectSnapshot {
  stats: Stats;
  trend: TrendPoint[];
}

/** 四源缓存条目：四个来源结构完全同构，只有快照类型不同（Kimi 另带
 *  rate_limits / rate_limits_error），故共用同一泛型条目——刷新逻辑也
 *  统一走 refreshLocalCollectRange，新增同类来源只需再写一个类型别名。 */
interface LocalCollectEntry<T extends LocalCollectSnapshot> {
  snapshot: T | null;
  error: string | null;
  refreshing: boolean;
  ts: number;
}

/** 空条目工厂：泛型函数内引用不到某一源的 EMPTY_* 常量，统一由这里构造
 *  （形状与各 EMPTY_* 完全一致）。 */
function emptyLocalCollectEntry<
  T extends LocalCollectSnapshot,
>(): LocalCollectEntry<T> {
  return { snapshot: null, error: null, refreshing: false, ts: 0 };
}

/** 一个「本地采集型」来源的取数与缓存句柄（refreshLocalCollectRangeImpl 用）。
 *  四源共用：新增同类来源只需在 DataCache 里再声明一次这组字段。 */
interface LocalCollectAgent<T extends LocalCollectSnapshot> {
  /** 远端同步协议里的来源名（server 白名单同值，**不是** UI 侧 AgentId） */
  source: "kimi" | "dsh" | "opencode" | "minimaxcode";
  /** 完全无数据时的错误文案填充名（与各 EMPTY_* 的展示名一致） */
  label: string;
  fetchLocal: (from: number, to: number, bucket: TrendBucket) => Promise<T>;
  inflight: Set<string>;
  setCache: React.Dispatch<
    React.SetStateAction<Record<string, LocalCollectEntry<T>>>
  >;
  /** 仅远端有数据（本机未安装 / 无会话）时构造快照：额度字段按各源语义
   *  置空（Kimi 的 rate_limits 是本机 OAuth 查询结果；DSH 无额度窗口；
   *  OpenCode / MiniMax Code 的套餐额度走 providerQuota 另一条链路——
   *  远端只有明细，本来就没有这些数据）。 */
  makeRemoteOnly: (
    remote: RemoteUsage,
    pricing: PricingConfig,
    fxRate: number,
    bucket: TrendBucket
  ) => T;
  /** 成功后是否重读 Agent 额度增量（只有带额度窗口的 Kimi 需要） */
  reloadAgentQuota: boolean;
}

/** refreshLocalCollectRangeImpl 需要的外部依赖（组件闭包里的同步状态）。 */
interface LocalCollectCtx {
  syncConfig: SyncConfig | null;
  syncEnabled: boolean;
  pricing: PricingConfig;
  fxRate: number;
  /** 无数据错误文案（key + 名称 → 本地化文本） */
  noDataText: (label: string) => string;
  /** 查询收尾钩子（Kimi 用它重读 Agent 额度增量） */
  onSettled: () => void;
}

/**
 * 刷新单个「本地采集型」来源的范围（Kimi / DSH / OpenCode / MiniMax Code 四源
 * 同构：快照只有 stats / trend）。这四个来源的明细都已在 sync.rs 里按各自的
 * source 名上传（server 白名单同值），故取数与合并口径与 refreshAgentRange
 * 完全一致：
 * - df === "all"：本机 + 远端其他设备（exclude_device = 本机设备）；
 * - df === "local"：仅本机，不发远端请求；
 * - df === <device_id>：仅远端该设备（本机命令不发）。
 * 合并时 spread 本机快照以保留其额外字段（Kimi 的 rate_limits /
 * rate_limits_error；其余三源只有 stats / trend）。本机命令失败（未安装 /
 * 无会话）不整体失败：远端有数据仍展示远端，完全无数据才把本机错误透出
 * ——与 refreshAgentRange 同款软失败语义。
 * 定义为模块级 async 函数而非组件内泛型回调：泛型箭头不能声明 async
 * （返回类型会变成 Promise，与刷新函数签名不符），且重入保护 / refreshing
 * 置位 / 异常兜底全部同步执行到第一个 await 之前，语义与组件内写法一致。
 */
async function refreshLocalCollectRangeImpl<T extends LocalCollectSnapshot>(
  agent: LocalCollectAgent<T>,
  df: string,
  key: string,
  from: number,
  to: number,
  bucket: TrendBucket,
  ctx: LocalCollectCtx
): Promise<void> {
  const {
    source,
    label,
    fetchLocal,
    inflight,
    setCache,
    makeRemoteOnly,
    reloadAgentQuota,
  } = agent;
  // 同步段（await 之前）：同 key 已在刷新则直接返回，并立刻置 refreshing
  if (inflight.has(key)) return;
  inflight.add(key);
  setCache((prev) => ({
    ...prev,
    [key]: {
      ...(prev[key] ?? emptyLocalCollectEntry<T>()),
      refreshing: true,
    },
  }));

  // 本机命令错误单独记录（未安装等）：远端有数据时仍可展示，不整体失败
  let localError: string | null = null;
  // 用对象盒子承接 then 回调里的赋值：回调内赋值不参与控制流分析，
  // 直接用 let 会被 TS 收窄成初始 null，后面无法安全解引用
  const localBox: { snapshot: T | null } = { snapshot: null };
  try {
    let remote: RemoteUsage | null = null;

    const wantLocal = df === "all" || df === "local";
    const wantRemote = ctx.syncEnabled && df !== "local";

    const tasks: Promise<unknown>[] = [];
    if (wantLocal) {
      tasks.push(
        fetchLocal(from, to, bucket)
          .then((s) => (localBox.snapshot = s))
          .catch((e) => {
            localError = String(e);
          })
      );
    }
    if (wantRemote && ctx.syncConfig) {
      const opts =
        df === "all"
          ? { excludeDevice: ctx.syncConfig.device_id, source }
          : { devices: df, source };
      const isSpecificRemote = df !== "all";
      tasks.push(
        remoteUsage(from, to, bucket, opts)
          .then((r) => (remote = r))
          .catch((e) => {
            if (isSpecificRemote) throw e;
            // "全部"模式：远端失败静默，仅用本地数据
          })
      );
    }

    await Promise.all(tasks);
    const local = localBox.snapshot;

    // 合并本地 + 远端（与 refreshAgentRange 同款三分支；spread 本机快照以
    // 保留 rate_limits 等非合并字段）
    let snapshot: T | null;
    if (df === "local") {
      snapshot = local;
    } else if (remote && !local) {
      snapshot = makeRemoteOnly(remote, ctx.pricing, ctx.fxRate, bucket);
    } else if (local && remote) {
      snapshot = {
        ...local,
        stats: mergeStats(local.stats, remote),
        trend: mergeTrend(
          local.trend,
          remote,
          ctx.pricing,
          ctx.fxRate,
          bucket
        ),
      };
    } else {
      snapshot = local;
    }

    setCache((prev) =>
      trimCustomEntries(
        {
          ...prev,
          [key]: {
            snapshot,
            // 有数据（含仅远端）即清错误；完全无数据时透出本机错误
            error: snapshot ? null : localError ?? ctx.noDataText(label),
            refreshing: false,
            ts: Date.now(),
          },
        },
        MAX_CUSTOM_ENTRIES
      )
    );
  } catch (e) {
    setCache((prev) =>
      trimCustomEntries(
        {
          ...prev,
          [key]: {
            ...(prev[key] ?? emptyLocalCollectEntry<T>()),
            error: String(e),
            refreshing: false,
            ts: Date.now(),
          },
        },
        MAX_CUSTOM_ENTRIES
      )
    );
  } finally {
    inflight.delete(key);
    if (reloadAgentQuota) ctx.onSettled();
  }
}

/** Kimi 快照条目：与 AgentEntry 同款形状，但 snapshot 携带加油包余额与
 *  额度失败原因（rate_limits_error），展示层需要这两个字段。 */
type KimiEntry = LocalCollectEntry<KimiSnapshot>;

const EMPTY_AGENT: AgentEntry = {
  snapshot: null,
  error: null,
  refreshing: false,
  ts: 0,
};

const EMPTY_ZAI: ZaiEntry = {
  stats: null,
  cost: null,
  trend: [],
  error: null,
  refreshing: false,
  ts: 0,
};

const EMPTY_CURSOR: CursorEntry = {
  snapshot: null,
  error: null,
  refreshing: false,
  ts: 0,
};

const EMPTY_CODEX: AgentEntry = EMPTY_AGENT;
const EMPTY_CLAUDE: AgentEntry = EMPTY_AGENT;
const EMPTY_KIMI: KimiEntry = emptyLocalCollectEntry();

/** DSH（DeepSeek Harness）快照条目：快照**不含** rate_limits（本地采集型
 *  无订阅额度窗口），故不能复用 AgentEntry（其 snapshot 类型要求
 *  rate_limits 字段）。 */
type DshEntry = LocalCollectEntry<DshSnapshot>;

const EMPTY_DSH: DshEntry = emptyLocalCollectEntry();

/** OpenCode 快照条目：与 DshEntry 同款形状同款理由——本地采集型快照
 *  **不含** rate_limits（Go 套餐额度是并列的另一条来源，走 providerQuota），
 *  故不能复用 AgentEntry（其 snapshot 类型要求 rate_limits 字段）。 */
type OpencodeEntry = LocalCollectEntry<OpencodeSnapshot>;

const EMPTY_OPENCODE: OpencodeEntry = emptyLocalCollectEntry();

/** MiniMax Code 快照条目：与 OpencodeEntry 同款形状同款理由——本地采集型
 *  快照**不含** rate_limits（套餐额度是并列的另一条来源，走 providerQuota
 *  的 minimax 条目），故不能复用 AgentEntry（其 snapshot 类型要求
 *  rate_limits 字段）。 */
type MinimaxcodeEntry = LocalCollectEntry<MinimaxcodeSnapshot>;

const EMPTY_MINIMAXCODE: MinimaxcodeEntry = emptyLocalCollectEntry();

/** 通用 provider 额度缓存条目（provider → {entries, refreshing, ts}）。
 *  entries 为每条凭证一个 ProviderQuotaEntry（单凭证失败以条目内
 *  status/message 表达，无全局 error）；不含 secret，可安全持久化。 */
export interface ProviderQuotaCacheEntry {
  entries: ProviderQuotaEntry[];
  refreshing: boolean;
  ts: number;
}

/** 冷启动恢复缓存时清掉 refreshing 标志（与 stripRefreshing 同款，但
 *  条目缺 entries 字段的旧数据一并兜底为空数组）。 */
function normalizeQuotaCache(
  map: Record<string, ProviderQuotaCacheEntry>
): Record<string, ProviderQuotaCacheEntry> {
  for (const k of Object.keys(map)) {
    map[k] = {
      entries: Array.isArray(map[k]?.entries) ? map[k].entries : [],
      refreshing: false,
      ts: map[k]?.ts ?? 0,
    };
  }
  return map;
}

/** 冷启动加载缓存时清掉可能被持久化的 refreshing 标志（崩溃恢复场景）。 */
function stripRefreshing<T extends { refreshing: boolean }>(
  map: Record<string, T>
): Record<string, T> {
  for (const k of Object.keys(map)) map[k] = { ...map[k], refreshing: false };
  return map;
}

export interface DataCacheValue {
  // ===== 查询参数（UI 可修改，变化时只切当前 key，不触发请求）=====
  preset: RangePreset;
  custom: { from: string; to: string };
  /** 由 preset 派生：今日/24h 按小时，更长范围按日 */
  trendBucket: TrendBucket;
  deviceFilter: string;
  setPreset: (p: RangePreset) => void;
  setCustom: (c: { from: string; to: string }) => void;
  setDeviceFilter: (d: string) => void;

  // ===== z.ai 数据（当前范围，读缓存）=====
  stats: Stats | null;
  cost: CostResult | null;
  trend: TrendPoint[];

  // ===== Codex 数据（当前范围，读缓存；stats/trend 与 z.ai 同构）=====
  codex: CodexSnapshot | null;
  /** Codex 错误信息（如未安装，不阻塞其他来源展示） */
  codexError: string | null;

  // ===== Claude 数据（当前范围，读缓存；stats/trend 与 z.ai 同构）=====
  claude: ClaudeSnapshot | null;
  /** Claude 错误信息（如未安装，不阻塞其他来源展示） */
  claudeError: string | null;

  // ===== Kimi 数据（当前范围，读缓存；stats/trend 与 z.ai 同构）=====
  kimi: KimiSnapshot | null;
  /** Kimi 错误信息（如未安装，不阻塞其他来源展示） */
  kimiError: string | null;

  // ===== DSH（DeepSeek Harness）数据（当前范围，读缓存；纯本地采集型，
  //      快照只有 stats/trend，无订阅额度窗口）=====
  dsh: DshSnapshot | null;
  /** DSH 错误信息（如未安装 / 无 ~/.dsh 会话目录，不阻塞其他来源展示） */
  dshError: string | null;

  // ===== OpenCode 数据（当前范围，读缓存；纯本地采集型，快照只有
  //      stats/trend；Go 套餐额度在 providerQuota["opencodego"] 里）=====
  opencode: OpencodeSnapshot | null;
  /** OpenCode 错误信息（如未安装 / 本地库无数据，不阻塞其他来源展示） */
  opencodeError: string | null;

  // ===== MiniMax Code 数据（当前范围，读缓存；与 OpenCode 同款本地采集型，
  //      快照只有 stats/trend；套餐额度在 providerQuota["minimax"] 里，
  //      agent id 仍是 UI 侧的 minimax）=====
  minimaxcode: MinimaxcodeSnapshot | null;
  /** MiniMax Code 错误信息（如未安装 / 本地库无数据，不阻塞其他来源展示） */
  minimaxcodeError: string | null;

  // ===== Cursor 数据（当前范围，读缓存）=====
  cursor: CursorSnapshot | null;
  cursorError: string | null;
  /** USD→CNY 汇率（汇总页合并花费用） */
  fxRate: number;

  // ===== Quota 数据（与范围无关）=====
  quota: QuotaResult | null;
  /** 当前账号今日增量 [增量百分比, 采样数]（本机+远端多端合并口径） */
  todayDelta: [number, number] | null;
  /** Codex / Claude / Cursor 今日额度增量，按来源和窗口索引。 */
  agentQuotaDeltas: AgentQuotaDeltaMap;
  quotaError: string | null;
  /** 全部账号快照的订阅额度（当前账号条目已用 30s live quota 覆盖，见 value 派生） */
  accountQuotas: AccountQuotaEntry[];
  /** 手动刷新多账号额度（账号捕获/切换/删除/重命名后调用） */
  refreshAccountQuotas: () => void;

  // ===== 同步配置（启动时加载一次）=====
  syncConfig: SyncConfig | null;
  remoteDevices: DeviceInfo[];
  syncEnabled: boolean;

  // ===== 通用凭证缓存（provider → 元数据列表；不落盘、不轮询，
  //      仅按需加载 + 凭证增删改事件后失效刷新）=====
  credentials: Record<string, ProviderCredentialMeta[]>;
  /** 拉取某 provider 的凭证列表（CredentialsCard 首挂与操作后刷新共用） */
  refreshCredentials: (provider: string) => Promise<void>;

  // ===== 通用 provider 额度缓存（provider → {entries, refreshing, ts}；
  //      120s 轮询仅覆盖已有凭证的 provider，手动刷新不受此限制）=====
  providerQuota: Record<string, ProviderQuotaCacheEntry>;
  /** 手动刷新某 provider 的额度（面板刷新按钮用；force 供凭证增删改事件
   *  路径使用：inflight 存在时登记补刷而非丢弃）；查询结果不含 secret，
   *  完成后顺带刷新该 provider 凭证列表以同步 last_check 徽章 */
  refreshProviderQuota: (
    provider: string,
    opts?: { force?: boolean }
  ) => Promise<void>;

  // ===== 元信息 =====
  /** 当前各数据源中最旧一次成功刷新的时间（最保守的新鲜度下限） */
  lastUpdate: number;
  /** 当前范围是否在后台拉取（仅用于刷新按钮转圈，不阻塞渲染） */
  refreshing: boolean;
  /** z.ai 错误信息（不阻塞已有数据展示） */
  error: string | null;
  /** 手动强制刷新当前范围（z.ai + Codex + Claude + Cursor） */
  refresh: () => void;
  /** 手动强制刷新 Coding Plan 额度 */
  refreshQuota: () => void;
}

// ===== 大缓存写盘防抖调度器 =====
// 后台 30s 刷新轮里 zai/codex/claude/kimi/dsh/opencode/minimaxcode/cursor/accountQuotas/providerQuota
// 等大缓存陆续到达，若各自 state 变化即同步 JSON.stringify 全量写 localStorage，
// 主线程会被反复阻塞（挂载/窗口返回时卡顿的来源之一）。这里把写盘请求合并进
// pending Map（同 key 只保留最新值），500ms 防抖后在 requestIdleCallback 空闲期
// 逐项落盘；页面隐藏/关闭与组件卸载时同步刷写全部 pending，保证进程被杀前
// 不丢最近一次更新。序列化仍走 saveCache（配额超限等静默降级，行为不变），
// 读路径不经过本调度器，恢复逻辑不受影响。
const PERSIST_DEBOUNCE_MS = 500;

const persistPending = new Map<string, unknown>();
let persistDebounceTimer: number | null = null;
let persistIdleHandle: number | null = null;

/** 立即同步刷写全部 pending 写盘请求（兜底：hidden / beforeunload / 卸载） */
function flushPersistNow(): void {
  if (persistDebounceTimer !== null) {
    clearTimeout(persistDebounceTimer);
    persistDebounceTimer = null;
  }
  if (persistIdleHandle !== null) {
    window.cancelIdleCallback(persistIdleHandle);
    persistIdleHandle = null;
  }
  if (persistPending.size === 0) return;
  const entries = Array.from(persistPending.entries());
  persistPending.clear();
  for (const [key, value] of entries) {
    saveCache(key, value);
  }
}

/** 大缓存写盘调度入口：500ms 防抖合并（同 key 只保留最新值），空闲期落盘 */
function schedulePersist(key: string, value: unknown): void {
  persistPending.set(key, value);
  // 已有待触发的防抖定时器时只合并值，不重建定时器
  if (persistDebounceTimer !== null) return;
  persistDebounceTimer = window.setTimeout(() => {
    persistDebounceTimer = null;
    if (typeof window.requestIdleCallback === "function") {
      persistIdleHandle = window.requestIdleCallback(
        () => {
          persistIdleHandle = null;
          flushPersistNow();
        },
        // 主线程持续繁忙（如后台刷新轮的响应解析处理）时 idle 回调可能长期
        // 不触发，timeout 保证至多 2s 强制执行一次，pending 不再完全依赖
        // hidden/beforeunload 兜底落盘
        { timeout: 2000 }
      );
    } else {
      // 不支持 requestIdleCallback 的环境退化为宏任务，同样不阻塞渲染
      window.setTimeout(flushPersistNow, 0);
    }
  }, PERSIST_DEBOUNCE_MS);
}

/** 今日增量持久化载荷：带本地日期戳（dateStr），跨天冷启动据此丢弃昨日值 */
interface DatedTodayDelta {
  date: string;
  delta: [number, number];
}

/** 今日增量回落切段上限（与后端 quota_history::MAX_DROP_SEGMENTS 同值）：
 *  回落次数超过它即判定该账号今日序列数据源异常，整体回退"峰值 − 首条"。
 *  真实重置（自然周重置 + 少量重置券/补差价）一天至多 1-3 次，5 已宽裕；
 *  前后端两个常量必须同值，两侧勿单独改动。 */
const MAX_DROP_SEGMENTS = 5;

/** 冷启动恢复今日增量缓存：日期非本地"今天"（含旧版无日期的裸数组格式）一律
 *  返回 null——昨日增量被当成今日展示会一直停在旧值，直到 60s 一轮的
 *  多端合并计算返回；丢弃最多让今日徽标晚一轮出现（期间按 null 不显示）。 */
function loadTodayDeltaCache(): [number, number] | null {
  const cached = loadCache<Partial<DatedTodayDelta>>("zbar-today-delta");
  if (!cached || cached.date !== dateStr(Date.now())) return null;
  const d = cached.delta;
  return Array.isArray(d) && typeof d[0] === "number" && typeof d[1] === "number"
    ? [d[0], d[1]]
    : null;
}

const Ctx = createContext<DataCacheValue | null>(null);

interface ProviderProps {
  /** 价格表（合并远程花费时需要） */
  pricing: PricingConfig;
  /** 凭证驱动 provider 的「已有凭证」状态（App 层探测；120s 额度轮询
   *  只查 presence 为 true 的 provider，经 ref 读取避免定时器重建） */
  credentialPresence: Record<string, boolean>;
  children: ReactNode;
}

export function DataProvider({ pricing, credentialPresence, children }: ProviderProps) {
  // ===== i18n：t 走 ref 镜像读取，避免把 t 列进刷新回调依赖导致
  //      语言切换重建全部定时器 / 触发全范围刷新 =====
  const { t } = useI18n();
  const tRef = useRef(t);
  useEffect(() => {
    tRef.current = t;
  }, [t]);

  // ===== 查询参数（持久化，冷启动恢复上次视图，与上次缓存匹配）=====
  const [preset, setPreset] = useState<RangePreset>(
    () => loadCache<RangePreset>("zbar-preset") ?? "today"
  );
  const [custom, setCustom] = useState<{ from: string; to: string }>(() => {
    const saved = loadCache<{ from: string; to: string }>("zbar-custom");
    if (saved) return saved;
    // 本地时区日期（dateStr）；不能用 toISOString（UTC），东八区凌晨会错一天
    const today = dateStr(Date.now());
    const week = dateStr(Date.now() - 6 * 86400000);
    return { from: week, to: today };
  });
  const [deviceFilter, setDeviceFilter] = useState<string>(
    () => loadCache<string>("zbar-device") ?? "all"
  );

  const trendBucket = bucketOf(preset);

  // ===== 按范围缓存（持久化，key: zai/codex=`${df}|${rangeKey}`，cursor=rangeKey）=====
  // zai/claude/kimi 含速度数据：旧契约缓存冷启动恢复时先剥离旧口径速度字段
  // 并把 ts 归零（详见 migrateSpeedEntries 注释），刷新到达后自动恢复新口径。
  const [zaiCache, setZaiCache] = useState<Record<string, ZaiEntry>>(
    () => {
      const raw = stripRefreshing(loadCache<Record<string, ZaiEntry>>("zbar-zai-cache") ?? {});
      return LEGACY_SPEED_CACHE ? migrateSpeedEntries(raw) : raw;
    }
  );
  const [cursorCache, setCursorCache] = useState<Record<string, CursorEntry>>(
    () => stripRefreshing(loadCache<Record<string, CursorEntry>>("zbar-cursor-cache") ?? {})
  );
  const [codexCache, setCodexCache] = useState<Record<string, AgentEntry>>(
    () => stripRefreshing(loadCache<Record<string, AgentEntry>>("zbar-codex-cache") ?? {})
  );
  const [claudeCache, setClaudeCache] = useState<Record<string, AgentEntry>>(
    () => {
      const raw = stripRefreshing(loadCache<Record<string, AgentEntry>>("zbar-claude-cache") ?? {});
      return LEGACY_SPEED_CACHE ? migrateSpeedEntries(raw) : raw;
    }
  );
  const [kimiCache, setKimiCache] = useState<Record<string, KimiEntry>>(
    () => {
      const raw = stripRefreshing(loadCache<Record<string, KimiEntry>>("zbar-kimi-cache") ?? {});
      return LEGACY_SPEED_CACHE ? migrateSpeedEntries(raw) : raw;
    }
  );
  // dsh 缓存：速度契约迁移不适用——zbar-dsh-cache 是本版本全新引入的键，
  // 磁盘上不可能存在旧口径速度条目（其余处理与 claude/kimi 完全一致）
  const [dshCache, setDshCache] = useState<Record<string, DshEntry>>(
    () => stripRefreshing(loadCache<Record<string, DshEntry>>("zbar-dsh-cache") ?? {})
  );
  // opencode 缓存：与 dsh 同款（zbar-opencode-cache 同为全新键，无旧口径速度
  // 条目需要迁移，速度字段照旧由后端下发）
  const [opencodeCache, setOpencodeCache] = useState<Record<string, OpencodeEntry>>(
    () => stripRefreshing(loadCache<Record<string, OpencodeEntry>>("zbar-opencode-cache") ?? {})
  );
  // minimaxcode 缓存：与 opencode 同款（zbar-minimaxcode-cache 同为全新键，
  // 无旧口径速度条目需要迁移，速度字段照旧由后端下发）
  const [minimaxcodeCache, setMinimaxcodeCache] = useState<
    Record<string, MinimaxcodeEntry>
  >(
    () =>
      stripRefreshing(
        loadCache<Record<string, MinimaxcodeEntry>>("zbar-minimaxcode-cache") ?? {}
      )
  );

  // ===== 其他数据（持久化）=====
  const [fxRate, setFxRate] = useState<number>(
    () => loadCache<number>("zbar-fxrate") ?? 7.2
  );
  const [quota, setQuota] = useState<QuotaResult | null>(
    () => loadCache<QuotaResult>("zbar-quota")
  );
  // 今日增量（本机 30s 路径的全局值；多端合并值在下方派生层优先覆盖）。
  // 恢复走 loadTodayDeltaCache：带日期校验，旧版无日期的裸数组与昨日值
  // 一律丢弃，避免跨天冷启动把昨天的增量当作今天展示
  const [todayDelta, setTodayDelta] = useState<[number, number] | null>(
    loadTodayDeltaCache
  );
  const [agentQuotaDeltas, setAgentQuotaDeltas] = useState<AgentQuotaDeltaMap>({});
  const [quotaError, setQuotaError] = useState<string | null>(null);
  // 多账号额度（低频 60s 一轮 + 账号操作后手动刷，持久化供冷启动秒显）。
  // 恢复时规范化：旧版本缓存条目无 fingerprint，补空串默认值；
  // today_delta 无日期戳、跨天会残留，一律清掉（见下）
  const [accountQuotas, setAccountQuotas] = useState<AccountQuotaEntry[]>(
    () =>
      (loadCache<AccountQuotaEntry[]>("zbar-account-quotas") ?? []).map((e) => ({
        ...e,
        fingerprint: e.fingerprint ?? "",
        // 今日增量只是展示加速字段（下一轮 60s 多端合并会重算），冷启动一律
        // 清掉：缓存里该值无日期戳，跨天残留会把昨日增量显示在账号行上；
        // 清掉最多让账号行今日徽标晚一轮（60s）出现
        today_delta: null,
      }))
  );
  // 各账号今日增量（fingerprint → [增量百分比, 采样数]）：本机全部快照 +
  // 远端其他设备快照合并计算（覆盖本机离线时段其他设备的消耗）
  const [accountTodayDeltas, setAccountTodayDeltas] = useState<
    Record<string, [number, number]>
  >({});

  // ===== 同步配置 =====
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // ===== 通用凭证缓存（挂点：内存缓存 + 事件失效刷新，不实现轮询）=====
  // 只缓存掩码元数据且不进 localStorage——凭证属敏感数据，按需从后端拉取。
  const [credentials, setCredentials] = useState<
    Record<string, ProviderCredentialMeta[]>
  >({});
  const credentialsInflight = useRef<Set<string>>(new Set());

  const refreshCredentials = useCallback((provider: string): Promise<void> => {
    if (credentialsInflight.current.has(provider)) return Promise.resolve();
    credentialsInflight.current.add(provider);
    return listProviderCredentials(provider)
      .then((entries) => {
        setCredentials((prev) => ({ ...prev, [provider]: entries }));
      })
      .finally(() => {
        credentialsInflight.current.delete(provider);
      });
  }, []);

  // ===== 通用 provider 额度缓存（120s 独立轮询 + 手动刷新）=====
  // 条目为每凭证一条 ProviderQuotaEntry（无 secret），可安全持久化：
  // 冷启动先用缓存渲染配额卡，后台轮询刷新后覆盖。
  const [providerQuota, setProviderQuota] = useState<
    Record<string, ProviderQuotaCacheEntry>
  >(() => normalizeQuotaCache(loadCache<Record<string, ProviderQuotaCacheEntry>>("zbar-provider-quota") ?? {}));
  const providerQuotaInflight = useRef<Set<string>>(new Set());
  // 事件驱动 force 刷新的补刷登记：inflight 存在时登记，当前查询完成后
  // 消费（再查一轮）并从集合移除——一次性登记，天然防无限循环
  const providerQuotaPendingForce = useRef<Set<string>>(new Set());
  // providerQuota 的 ref 镜像：presence 补刷判断"是否已有缓存"用
  const providerQuotaRef = useRef(providerQuota);
  useEffect(() => {
    providerQuotaRef.current = providerQuota;
  }, [providerQuota]);
  // credentialPresence 的 ref 镜像：120s 定时器 / visibilitychange 读最新值，
  // 避免把 presence 列入依赖导致定时器反复重建
  const credentialPresenceRef = useRef(credentialPresence);
  useEffect(() => {
    credentialPresenceRef.current = credentialPresence;
  }, [credentialPresence]);

  /** 刷新某 provider 的额度（120s 轮询与手动刷新共用）：inflight 去重，
   *  刷新期间保留旧 entries（不清空、无闪烁）；command 整体失败保留旧值
   *  （单凭证失败已在条目 status/message 里表达，无全局 error 可设）。
   *  完成后顺带刷新该 provider 凭证列表，让凭证卡的 last_check 徽章
   *  与本轮回写结论同步。
   *  force（事件驱动路径专用）：inflight 存在时不丢弃本次刷新——飞行中的
   *  旧查询持有删除/新增前的凭证列表快照，直接丢弃会短暂留下幽灵额度卡
   *  （≤120s）；改为登记补刷，当前查询完成后再补一轮（补刷由链内一次性
   *  消费登记项触发，不再登记，防无限循环）。 */
  const refreshProviderQuota = useCallback(
    (provider: string, opts?: { force?: boolean }): Promise<void> => {
      // 单轮查询（去重检查在外层）：inflight 标记 → 查询 → 回写/保旧
      const runRound = (): Promise<void> => {
        providerQuotaInflight.current.add(provider);
        setProviderQuota((prev) => ({
          ...prev,
          [provider]: {
            entries: prev[provider]?.entries ?? [],
            refreshing: true,
            ts: prev[provider]?.ts ?? 0,
          },
        }));
        return fetchProviderQuota(provider)
          .then((entries) => {
            setProviderQuota((prev) => ({
              ...prev,
              [provider]: { entries, refreshing: false, ts: Date.now() },
            }));
          })
          .catch(() => {
            setProviderQuota((prev) => ({
              ...prev,
              [provider]: {
                entries: prev[provider]?.entries ?? [],
                refreshing: false,
                ts: prev[provider]?.ts ?? 0,
              },
            }));
          })
          .finally(() => {
            providerQuotaInflight.current.delete(provider);
            // 后端查询完成时已回写 last_check，同步凭证列表（本地文件读，开销可忽略）
            refreshCredentials(provider).catch(() => {});
          });
      };
      if (providerQuotaInflight.current.has(provider)) {
        if (opts?.force === true) {
          providerQuotaPendingForce.current.add(provider);
        }
        return Promise.resolve();
      }
      return runRound().then(() => {
        // 补刷登记的消费点：本轮结束后立即再查一轮（拿事件后的新凭证列表）；
        // delete 返回 false 表示无登记，普通路径零开销。补刷轮不触发登记，
        // 链在此收敛，不会循环
        if (providerQuotaPendingForce.current.delete(provider)) {
          return runRound();
        }
        return undefined;
      });
    },
    [refreshCredentials]
  );

  // presence 变化补刷：某 provider 首次探测到有凭证（启动探测完成 / 添加
  // 首条凭证）时立即查一轮，不等下一个 120s tick
  useEffect(() => {
    for (const [provider, has] of Object.entries(credentialPresence)) {
      if (has && !providerQuotaRef.current[provider]) {
        void refreshProviderQuota(provider);
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [credentialPresence, refreshProviderQuota]);

  // 凭证增删改事件 → 失效刷新该 provider（App 层另行处理 visibility 联动）；
  // 额度同步补刷一轮：添加凭证后立即可见查询结果，形成操作闭环。
  // 事件路径带 force：触发时恰有飞行中的旧查询（持有旧凭证列表快照）时，
  // 完成后补一轮，删除的凭证不会残留幽灵额度卡
  useEffect(() => {
    const onCredentialsChanged = (e: Event) => {
      const provider = (e as CustomEvent<{ provider: string }>).detail?.provider;
      if (typeof provider === "string" && provider) {
        refreshCredentials(provider).catch(() => {});
        void refreshProviderQuota(provider, { force: true });
      }
    };
    window.addEventListener(CREDENTIALS_CHANGED_EVENT, onCredentialsChanged);
    return () =>
      window.removeEventListener(CREDENTIALS_CHANGED_EVENT, onCredentialsChanged);
  }, [refreshCredentials, refreshProviderQuota]);

  // ===== 并发保护：同范围正在刷新则跳过，避免重复请求 =====
  const zaiInflight = useRef<Set<string>>(new Set());
  const cursorInflight = useRef<Set<string>>(new Set());
  const codexInflight = useRef<Set<string>>(new Set());
  const claudeInflight = useRef<Set<string>>(new Set());
  const kimiInflight = useRef<Set<string>>(new Set());
  const dshInflight = useRef<Set<string>>(new Set());
  const opencodeInflight = useRef<Set<string>>(new Set());
  const minimaxcodeInflight = useRef<Set<string>>(new Set());
  const quotaReqId = useRef(0);
  const accountQuotasInflight = useRef(false);
  // 两路额度数据各自的最近成功时刻：切换账号后 refreshQuota（live，读 ~/.zcode
  // 实时凭证）与 refreshAccountQuotas（读快照 + 实时指纹）并行、到达顺序不定，
  // 合并时只允许较新一路覆盖当前账号条目，避免把旧账号额度贴到新账号名下
  const quotaTsRef = useRef(0);
  const accountQuotasTsRef = useRef(0);
  const agentQuotaReqId = useRef(0);
  const agentQuotaDeltaRefreshRef = useRef<() => void>(() => {});
  const agentQuotaReloadTimer = useRef<number | null>(null);

  // ===== 额度用满自动切换（无人值守）=====
  // loadQuota 的 useCallback([]) 闭包无法直接引用后定义的检查函数，走 ref 转发
  // （与 agentQuotaDeltaRefreshRef 同模式）
  const maybeAutoSwitchRef = useRef<(q: QuotaResult) => void>(() => {});
  // 切换执行中标志：切换含退出/重启 ZCode 的长事务（约 8-15s），期间 30s 一轮
  // 的 loadQuota 可能再进成功回调，重入直接跳过
  const autoSwitchingRef = useRef(false);
  // 检查冷却截止时间戳：无可用目标/切换失败后一段时间内不再重复检查与通知，
  // 避免额度轮询每 30s 反复打扰；切换成功后也短暂冷却（见下方注释）
  const autoSwitchCooldownUntilRef = useRef(0);
  // "无可切换账号"类通知的上次发出时间：满额窗口可持续约 5h，30 分钟冷却
  // 一轮仍会重复通知约 10 次，该类通知单独 2 小时节流（检查冷却不受影响）
  const lastNoTargetNotifyTsRef = useRef(0);
  // accountQuotasMerged 的 ref 镜像：检查触发时读最新值，避免闭包拿到旧列表
  const accountQuotasMergedRef = useRef<AccountQuotaEntry[]>([]);

  // 各 Agent 的额度查询完成后立即重读历史，避免网络较慢时首轮 3 秒读取
  // 先于快照写入，用户要等到下一轮 30 秒定时器才看到今日增量。
  const scheduleAgentQuotaReload = useCallback(() => {
    if (agentQuotaReloadTimer.current != null) return;
    agentQuotaReloadTimer.current = window.setTimeout(() => {
      agentQuotaReloadTimer.current = null;
      agentQuotaDeltaRefreshRef.current();
    }, 200);
  }, []);

  // ===== refs：按需补刷 effect 读取最新缓存，避免闭包过期 / 反复触发 =====
  const zaiCacheRef = useRef(zaiCache);
  useEffect(() => {
    zaiCacheRef.current = zaiCache;
  }, [zaiCache]);
  const cursorCacheRef = useRef(cursorCache);
  useEffect(() => {
    cursorCacheRef.current = cursorCache;
  }, [cursorCache]);
  const codexCacheRef = useRef(codexCache);
  useEffect(() => {
    codexCacheRef.current = codexCache;
  }, [codexCache]);
  const claudeCacheRef = useRef(claudeCache);
  useEffect(() => {
    claudeCacheRef.current = claudeCache;
  }, [claudeCache]);
  const kimiCacheRef = useRef(kimiCache);
  useEffect(() => {
    kimiCacheRef.current = kimiCache;
  }, [kimiCache]);
  const dshCacheRef = useRef(dshCache);
  useEffect(() => {
    dshCacheRef.current = dshCache;
  }, [dshCache]);
  const opencodeCacheRef = useRef(opencodeCache);
  useEffect(() => {
    opencodeCacheRef.current = opencodeCache;
  }, [opencodeCache]);
  const minimaxcodeCacheRef = useRef(minimaxcodeCache);
  useEffect(() => {
    minimaxcodeCacheRef.current = minimaxcodeCache;
  }, [minimaxcodeCache]);

  // ===== refs：后台 tick 读取最新 preset/custom（用户停留在自定义视图时把当前
  //      custom 范围纳入刷新用）。经 ref 读取而不是列入定时器 effect 依赖，
  //      避免每次切换范围都重建 30s 定时器 =====
  const presetRef = useRef(preset);
  useEffect(() => {
    presetRef.current = preset;
  }, [preset]);
  const customRef = useRef(custom);
  useEffect(() => {
    customRef.current = custom;
  }, [custom]);

  /**
   * 刷新单个 z.ai 范围（范围无关，参数化 df/from/to/bucket）。
   * 保留原 local/remote/merge 合并逻辑；刷新期间不清空旧值，只设 refreshing。
   * pricing 未就绪时保留旧 cost，避免空价格表把花费覆盖成 0（冷启动保护）。
   */
  const refreshZaiRange = useCallback(
    async (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) => {
      if (zaiInflight.current.has(key)) return; // 同范围已在刷新，跳过
      zaiInflight.current.add(key);
      setZaiCache((prev) => ({
        ...prev,
        [key]: { ...(prev[key] ?? EMPTY_ZAI), refreshing: true },
      }));

      try {
        let localStats: Stats | null = null;
        let localCost: CostResult | null = null;
        let localTrend: TrendPoint[] = [];
        let remote: RemoteUsage | null = null;

        const wantLocal = df === "all" || df === "local";
        // all 或指定远端设备时拉远程数据；选 local 时不拉
        const wantRemote = syncEnabled && df !== "local";

        const tasks: Promise<unknown>[] = [];
        if (wantLocal) {
          tasks.push(
            fetchStats(from, to).then((s) => (localStats = s)),
            computeCost(from, to).then((c) => (localCost = c)),
            fetchTrend(from, to, bucket).then((t) => (localTrend = t))
          );
        }
        if (wantRemote && syncConfig) {
          // source 固定 zcode：服务端不传 source 会合并全部来源，
          // 其他设备上传的 Codex 数据会混入 Z.ai 口径并在汇总页重复计数
          const opts =
            df === "all"
              ? { excludeDevice: syncConfig.device_id, source: "zcode" }
              : { devices: df, source: "zcode" };
          const isSpecificRemote = df !== "all";
          tasks.push(
            remoteUsage(from, to, bucket, opts)
              .then((r) => (remote = r))
              .catch((e) => {
                if (isSpecificRemote) throw e;
                // "全部"模式：远端失败静默，仅用本地数据
              })
          );
        }

        await Promise.all(tasks);

        // 合并本地 + 远端
        let stats: Stats | null;
        let cost: CostResult | null;
        let trend: TrendPoint[];
        if (df === "local") {
          stats = localStats;
          cost = localCost;
          trend = localTrend;
        } else if (remote && !localStats) {
          stats = remoteToStats(remote);
          cost = computeRemoteCost(remote, pricing, fxRate);
          trend = remoteTrendToLocal(remote, pricing, fxRate, bucket);
        } else if (localStats && remote) {
          stats = mergeStats(localStats, remote);
          cost = mergeCost(localCost, remote, pricing, fxRate);
          trend = mergeTrend(localTrend, remote, pricing, fxRate, bucket);
        } else {
          stats = localStats;
          cost = localCost;
          trend = localTrend;
        }

        setZaiCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                stats,
                // pricing 未就绪时保留旧 cost，避免空价格表覆盖成 0
                cost: isPricingEmpty(pricing) ? (prev[key]?.cost ?? null) : cost,
                trend,
                error: null,
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      } catch (e) {
        setZaiCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                ...(prev[key] ?? EMPTY_ZAI),
                error: String(e),
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      } finally {
        zaiInflight.current.delete(key);
      }
    },
    // 注意不含 currency：花费是双货币一次算齐的，切货币只需展示层换字段，
    // 不需要重刷数据；若把 currency 列进依赖会导致切货币触发全范围刷新风暴（UI 卡顿）
    [syncConfig, syncEnabled, pricing, fxRate]
  );

  /** 刷新单个 Cursor 范围（账号级，不受设备筛选影响）。刷新期间不清空旧值。 */
  const refreshCursorRange = useCallback((key: string, from: number, to: number) => {
    if (cursorInflight.current.has(key)) return;
    cursorInflight.current.add(key);
    setCursorCache((prev) => ({
      ...prev,
      [key]: { ...(prev[key] ?? EMPTY_CURSOR), refreshing: true },
    }));

    fetchCursorUsage(from, to)
      .then((data) => {
        setCursorCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: { snapshot: data, error: null, refreshing: false, ts: Date.now() },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      })
      .catch((e) => {
        setCursorCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                ...(prev[key] ?? EMPTY_CURSOR),
                error: String(e),
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      })
      .finally(() => {
        cursorInflight.current.delete(key);
        scheduleAgentQuotaReload();
      });
  }, [scheduleAgentQuotaReload]);

  /**
   * 刷新单个派生来源（Codex / Claude）范围。两者数据链路完全同构，仅数据源
   * 不同，抽成参数化实现（fetchLocal + 远端 source 参数 + 各自缓存）。
   * 复用 z.ai 的 Stats/RemoteUsage 合并函数——快照的 stats/trend 与 z.ai 同构。
   * 远端合并开关与 z.ai 一致：syncEnabled 且设备筛选非 "local" 时拉
   * remote_usage(source=...)；本地未安装时软失败，远端有数据仍展示远端；
   * 完全无数据才记录 error。rate_limits 始终保持本地值。
   */
  const refreshAgentRange = useCallback(
    async (
      agent: {
        source: "codex" | "claude";
        label: string;
        fetchLocal: (
          from: number,
          to: number,
          bucket: TrendBucket
        ) => Promise<AgentSnapshot>;
        inflight: Set<string>;
        setCache: React.Dispatch<
          React.SetStateAction<Record<string, AgentEntry>>
        >;
      },
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) => {
      const { source, label, fetchLocal, inflight, setCache } = agent;
      if (inflight.has(key)) return;
      inflight.add(key);
      setCache((prev) => ({
        ...prev,
        [key]: { ...(prev[key] ?? EMPTY_AGENT), refreshing: true },
      }));

      // 本地命令错误单独记录（未安装等）：远端有数据时仍可展示，不整体失败
      let localError: string | null = null;
      // 用对象盒子承接 then 回调里的赋值：回调内赋值不参与控制流分析，
      // 直接用 let 会被 TS 收窄成初始 null，后面无法安全解引用 stats/trend
      const localBox: { snapshot: AgentSnapshot | null } = { snapshot: null };
      try {
        let remote: RemoteUsage | null = null;

        const wantLocal = df === "all" || df === "local";
        const wantRemote = syncEnabled && df !== "local";

        const tasks: Promise<unknown>[] = [];
        if (wantLocal) {
          tasks.push(
            fetchLocal(from, to, bucket)
              .then((s) => (localBox.snapshot = s))
              .catch((e) => {
                localError = String(e);
              })
          );
        }
        if (wantRemote && syncConfig) {
          const opts =
            df === "all"
              ? { excludeDevice: syncConfig.device_id, source }
              : { devices: df, source };
          const isSpecificRemote = df !== "all";
          tasks.push(
            remoteUsage(from, to, bucket, opts)
              .then((r) => (remote = r))
              .catch((e) => {
                if (isSpecificRemote) throw e;
                // "全部"模式：远端失败静默，仅用本地数据
              })
          );
        }

        await Promise.all(tasks);
        const local = localBox.snapshot;

        // 合并本地 + 远端（与 z.ai 同款三分支）
        let snapshot: AgentSnapshot | null;
        if (df === "local") {
          snapshot = local;
        } else if (remote && !local) {
          snapshot = {
            stats: remoteToStats(remote),
            trend: remoteTrendToLocal(remote, pricing, fxRate, bucket),
            rate_limits: null,
          };
        } else if (local && remote) {
          snapshot = {
            stats: mergeStats(local.stats, remote),
            trend: mergeTrend(local.trend, remote, pricing, fxRate, bucket),
            rate_limits: local.rate_limits,
          };
        } else {
          snapshot = local;
        }

        setCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                snapshot,
                // 有数据（含仅远端）即清错误；完全无数据时透出本地错误
                error: snapshot
                  ? null
                  : localError ?? tRef.current("stats.noDataFor", { name: label }),
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      } catch (e) {
        setCache((prev) =>
          trimCustomEntries(
            {
              ...prev,
              [key]: {
                ...(prev[key] ?? EMPTY_AGENT),
                error: String(e),
                refreshing: false,
                ts: Date.now(),
              },
            },
            MAX_CUSTOM_ENTRIES
          )
        );
      } finally {
        inflight.delete(key);
        scheduleAgentQuotaReload();
      }
    },
    [syncConfig, syncEnabled, pricing, fxRate, scheduleAgentQuotaReload]
  );

  const refreshCodexRange = useCallback(
    (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) =>
      refreshAgentRange(
        {
          source: "codex",
          label: "Codex",
          fetchLocal: fetchCodexUsage,
          inflight: codexInflight.current,
          setCache: setCodexCache,
        },
        df,
        key,
        from,
        to,
        bucket
      ),
    [refreshAgentRange]
  );

  const refreshClaudeRange = useCallback(
    (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) =>
      refreshAgentRange(
        {
          source: "claude",
          label: "Claude",
          fetchLocal: fetchClaudeUsage,
          inflight: claudeInflight.current,
          setCache: setClaudeCache,
        },
        df,
        key,
        from,
        to,
        bucket
      ),
    [refreshAgentRange]
  );

  /**
   * 刷新单个「本地采集型」来源的范围（Kimi / DSH / OpenCode / MiniMax Code
   * 四源同构：快照只有 stats / trend）。主体是模块级
   * refreshLocalCollectRangeImpl（异步泛型函数不能直接当 useCallback 回调），
   * 这里只负责把当前闭包里的同步配置/价格/汇率/文案绑进去。
   * 取数与合并口径与 refreshAgentRange 完全一致：
   * - df === "all"：本机 + 远端其他设备（exclude_device = 本机设备）；
   * - df === "local"：仅本机，不发远端请求；
   * - df === <device_id>：仅远端该设备（本机命令不发）。
   * 这四个来源的明细都已在 sync.rs 里按各自的 source 名上传（server 白名单
   * 同值）：kimi / dsh / opencode / minimaxcode——后两个与 UI 侧 AgentId
   * （opencodego / minimax）异名，别写错。
   */
  const refreshLocalCollectRange = useCallback(
    <T extends LocalCollectSnapshot,>(
      agent: LocalCollectAgent<T>,
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) => {
      void refreshLocalCollectRangeImpl(agent, df, key, from, to, bucket, {
        syncConfig,
        syncEnabled,
        pricing,
        fxRate,
        noDataText: (label) =>
          tRef.current("stats.noDataFor", { name: label }),
        onSettled: scheduleAgentQuotaReload,
      });
    },
    [syncConfig, syncEnabled, pricing, fxRate, scheduleAgentQuotaReload]
  );

  /**
   * 刷新单个 Kimi 范围：本机 ~/.kimi-code 会话 + 远端同来源（source="kimi"）
   * 合并，形态与 refreshAgentRange 一致。key 带 df 前缀（与 codex/claude
   * 同款调用），df 同时决定远端取数语义（见 refreshLocalCollectRange）。
   * 成功后照旧重读 Agent 额度增量：Kimi 是四个来源里唯一有额度窗口
   * （rate_limits）的，其余三源该参数为 false。
   */
  const refreshKimiRange = useCallback(
    (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) =>
      refreshLocalCollectRange(
        {
          source: "kimi",
          label: "Kimi Code",
          fetchLocal: fetchKimiUsage,
          inflight: kimiInflight.current,
          setCache: setKimiCache,
          // 订阅额度来自本机 OAuth 查询（远端只有明细）→ 仅远端时置空
          makeRemoteOnly: (remote, pricing, fxRate, bucket) => ({
            stats: remoteToStats(remote),
            trend: remoteTrendToLocal(remote, pricing, fxRate, bucket),
            rate_limits: null,
            rate_limits_error: null,
          }),
          reloadAgentQuota: true,
        },
        df,
        key,
        from,
        to,
        bucket
      ),
    [refreshLocalCollectRange]
  );

  /**
   * 刷新单个 DSH（DeepSeek Harness）范围：本机 ~/.dsh 会话 + 远端同来源
   * （source="dsh"）合并，形态与 refreshKimiRange 完全一致（走同一个
   * refreshLocalCollectRange），df 同时决定远端取数语义。
   * 与 Kimi 的唯一差异：reloadAgentQuota 为 false——dsh 没有任何额度窗口
   * （无 rate_limits），其查询不写入 agent 额度快照，重读历史纯属空转。
   */
  const refreshDshRange = useCallback(
    (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) =>
      refreshLocalCollectRange(
        {
          source: "dsh",
          label: "DeepSeek Harness",
          fetchLocal: fetchDshUsage,
          inflight: dshInflight.current,
          setCache: setDshCache,
          // 无额度字段可保留：仅远端时就是纯 stats/trend 快照
          makeRemoteOnly: (remote, pricing, fxRate, bucket) => ({
            stats: remoteToStats(remote),
            trend: remoteTrendToLocal(remote, pricing, fxRate, bucket),
          }),
          reloadAgentQuota: false,
        },
        df,
        key,
        from,
        to,
        bucket
      ),
    [refreshLocalCollectRange]
  );

  /**
   * 刷新单个 OpenCode 范围：本机 opencode.db + 远端同来源（source="opencode"，
   * 注意不是 UI 侧 AgentId `opencodego`）合并，形态与 refreshDshRange 完全一致
   * （走同一个 refreshLocalCollectRange）。reloadAgentQuota 为 false：本地用量
   * 查询不写 agent 额度快照（Go 套餐额度走 providerQuota 那条独立链路）。
   */
  const refreshOpencodeRange = useCallback(
    (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) =>
      refreshLocalCollectRange(
        {
          source: "opencode",
          label: "OpenCode",
          fetchLocal: fetchOpencodeUsage,
          inflight: opencodeInflight.current,
          setCache: setOpencodeCache,
          makeRemoteOnly: (remote, pricing, fxRate, bucket) => ({
            stats: remoteToStats(remote),
            trend: remoteTrendToLocal(remote, pricing, fxRate, bucket),
          }),
          reloadAgentQuota: false,
        },
        df,
        key,
        from,
        to,
        bucket
      ),
    [refreshLocalCollectRange]
  );

  /**
   * 刷新单个 MiniMax Code 范围：本机 ~/.minimax + 远端同来源
   * （source="minimaxcode"）合并，形态与 refreshOpencodeRange 完全一致（走同一
   * 个 refreshLocalCollectRange）。reloadAgentQuota 为 false：本地用量查询不写
   * agent 额度快照（套餐额度走 providerQuota["minimax"] 那条独立链路）。
   */
  const refreshMinimaxcodeRange = useCallback(
    (
      df: string,
      key: string,
      from: number,
      to: number,
      bucket: TrendBucket
    ) =>
      refreshLocalCollectRange(
        {
          source: "minimaxcode",
          label: "MiniMax Code",
          fetchLocal: fetchMinimaxcodeUsage,
          inflight: minimaxcodeInflight.current,
          setCache: setMinimaxcodeCache,
          makeRemoteOnly: (remote, pricing, fxRate, bucket) => ({
            stats: remoteToStats(remote),
            trend: remoteTrendToLocal(remote, pricing, fxRate, bucket),
          }),
          reloadAgentQuota: false,
        },
        df,
        key,
        from,
        to,
        bucket
      ),
    [refreshLocalCollectRange]
  );

  // Quota 数据加载（与范围无关，每次调用会采样写入 quota_history）。
  const loadQuota = useCallback(() => {
    const reqId = ++quotaReqId.current;
    fetchQuota()
      .then((r) => {
        if (reqId !== quotaReqId.current) return;
        quotaTsRef.current = Date.now();
        setQuota(r);
        setQuotaError(null);
        // 额度刷新成功后顺带读今日增量（快照由 fetch_quota 采样写入）
        getTodayDelta().then(setTodayDelta).catch(() => {});
        // 额度用满自动切换只挂在成功路径：自动切换执行期间（约 8-15s）live
        // 查询可能超时失败，失败路径不检查，天然不会误触发
        maybeAutoSwitchRef.current(r);
      })
      .catch((e) => {
        if (reqId !== quotaReqId.current) return;
        setQuotaError(String(e));
      });
  }, []);

  // 多账号额度加载（读各账号快照里的凭证并行查询，查询成功会写带指纹的
  // 历史采样——各账号"今日增量"的数据源，读路径按指纹过滤互不污染）。
  // 失败静默保留旧值：账号级失败原因在各条目 error 字段里，不设全局 error。
  const loadAccountQuotas = useCallback(() => {
    if (accountQuotasInflight.current) return;
    accountQuotasInflight.current = true;
    fetchAccountQuotas()
      .then((r) => {
        accountQuotasTsRef.current = Date.now();
        setAccountQuotas(r);
      })
      .catch(() => {})
      .finally(() => {
        accountQuotasInflight.current = false;
      });
  }, []);

  /**
   * 额度用满自动切换检查（无人值守，只在 loadQuota 成功路径调用）：
   * 当前账号 5h 额度用满 → 按算法挑一个「立即可用」的其他账号切换并系统通知。
   * 防抖：autoSwitching 重入保护 + 冷却时间戳 + 多账号数据未就绪直接跳过
   * （不算失败、不进冷却）。与 AccountsCard 手动切换的并发靠事务内前提校验
   * 兜底：请求携带触发时的当前指纹（expectFingerprint），等锁期间用户先手动
   * 切换了账号则后端取消本次自动切换，不会覆盖用户的选择。
   */
  const maybeAutoSwitch = useCallback(
    (fresh: QuotaResult) => {
      // 开关现读（不缓存）：设置页切换开关后即时生效，与 locale 读取同模式
      if (!readAutoSwitchEnabled()) return;
      if (autoSwitchingRef.current) return;
      // 5h 窗口缺失（接口异常）或未用满（<100%）不触发
      const h5 = fresh.hour5;
      if (!h5 || h5.percentage < 100) return;
      // 多账号额度未就绪（冷启动首轮未跑，ts=0）或数据已过期（面板长期隐藏、
      // 轮询被节流）：直接返回，不算失败不冷却——拿旧数据挑目标会误切
      if (
        accountQuotasMergedRef.current.length === 0 ||
        accountQuotasTsRef.current === 0 ||
        Date.now() - accountQuotasTsRef.current > 5 * 60_000
      ) {
        return;
      }
      if (Date.now() < autoSwitchCooldownUntilRef.current) return;

      // 无人值守只切「立即可用」账号（readyOnly）：切过去还要等 5h 重置没有意义
      const pick = pickAutoSwitchTarget(accountQuotasMergedRef.current, {
        readyOnly: true,
      });
      if (!pick) {
        autoSwitchCooldownUntilRef.current = Date.now() + 30 * 60_000;
        writeAutoSwitchLog({
          ts: Date.now(),
          to: null,
          ok: false,
          reasonKey: "quotaFull",
        });
        window.dispatchEvent(new Event(AUTO_SWITCH_DONE_EVENT));
        // 通知单独 2 小时节流（见 lastNoTargetNotifyTsRef 注释）；冷却仍 30
        // 分钟，继续挡住 30s 轮询的重复触发
        if (Date.now() - lastNoTargetNotifyTsRef.current >= 2 * 3_600_000) {
          lastNoTargetNotifyTsRef.current = Date.now();
          void showNotification(
            "ZBar",
            tRef.current("settings.accountsAutoNotifyFail")
          ).catch(() => {});
        }
        return;
      }

      // 触发时观察到的当前登录指纹：随请求带给后端做事务内前提校验。找不到
      // 当前账号条目（数据异常）时不传，退化为无前提校验的普通切换
      const expectFingerprint =
        accountQuotasMergedRef.current.find((e) => e.is_current)
          ?.fingerprint ?? null;

      autoSwitchingRef.current = true;
      void (async () => {
        try {
          await switchAccount(
            pick.entry.id,
            expectFingerprint ?? undefined
          );
          // 成功后短暂冷却：给刚触发的 loadAccountQuotas 一点时间刷新各条目
          // is_current，避免下一轮 30s 检查在未更新的列表里把刚切走的旧账号
          // 又当成候选目标
          autoSwitchCooldownUntilRef.current = Date.now() + 90_000;
          writeAutoSwitchLog({
            ts: Date.now(),
            to: pick.entry.display_name,
            ok: true,
            reasonKey: "quotaFull",
          });
          window.dispatchEvent(new Event(AUTO_SWITCH_DONE_EVENT));
          void showNotification(
            "ZBar",
            tRef.current("settings.accountsAutoNotifyOk", {
              name: pick.entry.display_name,
            })
          ).catch(() => {});
          // 立即刷新多账号额度（新当前账号条目与 is_current 标记）
          loadAccountQuotas();
        } catch (e) {
          const msg = String(e);
          if (msg.includes("登录态已变化")) {
            // 前提校验取消：本请求等锁期间登录态已被其他切换（多为用户手动
            // 切换）改变，本次自动切换作废——不通知、不写失败日志、不冷却。
            // 文案判断与 Rust 侧 accounts.rs switch_account 的固定错误文案
            // 约定，两侧勿单独改动
            return;
          }
          autoSwitchCooldownUntilRef.current = Date.now() + 30 * 60_000;
          writeAutoSwitchLog({
            ts: Date.now(),
            to: null,
            ok: false,
            reasonKey: "quotaFull",
          });
          window.dispatchEvent(new Event(AUTO_SWITCH_DONE_EVENT));
          // 通知直接透传后端错误（多为中文事务错误），比固定文案更可诊断
          void showNotification("ZBar", msg).catch(() => {});
        } finally {
          autoSwitchingRef.current = false;
        }
      })();
    },
    [loadAccountQuotas]
  );
  maybeAutoSwitchRef.current = maybeAutoSwitch;

  /** 各账号今日增量（多端合并）：本机全部账号快照 + 远端其他设备的快照
   *  （排除本机设备防重复），按账号指纹分组、时间升序分段累计今日增量——
   *  遇 weekly_pct 回落（周重置/重置券/补差价等重置事件）即结算当前段并开
   *  新段，增量 = Σ(段末 − 段首)，多段累计可超过 100。之所以不用"峰值 −
   *  首条"：回落会让峰值冻结在重置前的最高点，用到 100% 后用重置券后徽标
   *  从此恒定不动；当天周重置还会让首条与峰值分属两个额度周期。
   *  额度百分比是账号服务端总量而非设备增量，多设备采样互补合并后
   *  分段语义仍正确——本机离线时段其他设备的消耗由此补全。
   *  数据源异常防护：回落次数超过 MAX_DROP_SEGMENTS 的账号判定多设备数据
   *  打架（"回落 = 重置"只在数据源可信时成立），此时**改用仅本机序列重算**；
   *  本机今天没采样该账号才退回合并序列"峰值 − 首条"有界兜底。取舍：有界
   *  回退的峰值同样可能取自脏值（线上实测：本机实际已用 43%，合并序列的
   *  脏峰值却给出 87%），所以正确做法是换数据源而不是换公式——本机是用户
   *  当前查看的设备、凭证链路已在 1.5.5 收紧，可信度高于来源不明的远端
   *  批量数据；本机恰是脏源时不完美，属可接受取舍（详见循环内说明）。
   *  与后端 quota_history::today_deltas_from 同口径（同一分段算法、同一
   *  回落上限）；差异：后端只读本机 quota_history、没有远端合并，前端在
   *  异常路径多了"换回本机源"这一步，两端算法本身勿单独改动。
   *  失败静默保留旧值（后端 account_quotas 的本机 today_delta 仍在兜底）。 */
  const loadAccountTodayDeltas = useCallback(() => {
    const from = todayStartMs();
    const to = Date.now() + 1;
    const localPromise = getQuotaHistory(true, from);
    const wantRemote = syncEnabled && !!syncConfig;
    const remotePromise = wantRemote
      ? remoteSnapshots(from, to, {
          excludeDevice: syncConfig!.device_id,
        }).catch(() => [])
      : Promise.resolve([]);
    Promise.all([localPromise, remotePromise])
      .then(([local, remote]) => {
        // 只取今日 + 带账号指纹的采样（无指纹的旧版快照归属不明，不参与）
        const isTodayOfAccount = (
          s: QuotaSnapshot
        ): s is QuotaSnapshot & { account: string } =>
          typeof s.account === "string" && !!s.account && s.ts >= from;
        // 本机序列：独立保留、不与远端 concat（合并序列数据源打架时要用它
        // 重算）。后端 load_all 已按 ts 升序返回，过滤不改变顺序，可直接计算
        const localList = local.filter(isTodayOfAccount);
        // 合并序列：本机 + 远端其他设备，按 (ts, account) 去重后时间升序
        const seen = new Set<string>();
        const mergedList = [...local, ...remote]
          .filter(isTodayOfAccount)
          .sort((a, b) => a.ts - b.ts)
          .filter((s) => {
            const key = `${s.ts}|${s.account}`;
            if (seen.has(key)) return false;
            seen.add(key);
            return true;
          });
        // 两个序列各自按账号分组（组内已按 ts 升序）
        const groupByAccount = (
          list: (QuotaSnapshot & { account: string })[]
        ) => {
          const groups = new Map<
            string,
            (QuotaSnapshot & { account: string })[]
          >();
          for (const s of list) {
            const g = groups.get(s.account);
            if (g) g.push(s);
            else groups.set(s.account, [s]);
          }
          return groups;
        };
        const mergedGroups = groupByAccount(mergedList);
        const localGroups = groupByAccount(localList);
        // 回落统计（回落 = cur < prev）：回落次数判定与峰值回退共用一次遍历
        const statsOf = (snaps: (QuotaSnapshot & { account: string })[]) => {
          let drops = 0;
          const first = snaps[0].weekly_pct;
          let peak = first;
          for (let i = 1; i < snaps.length; i += 1) {
            const pct = snaps[i].weekly_pct;
            if (pct < snaps[i - 1].weekly_pct) drops += 1;
            if (pct > peak) peak = pct;
          }
          return { drops, first, peak };
        };
        // 分段累计：segStart = 当前段首值，last = 上一条值。
        // 低于上一条 = 一次回落（调用方已保证回落次数 ≤ MAX_DROP_SEGMENTS，
        // 即每次回落都可信为真实重置事件）：先结算上一段再从此条开新段；
        // 段内单调不减，差值天然非负。遍历结束结算最后一段。
        const segmentedOf = (
          snaps: (QuotaSnapshot & { account: string })[]
        ): [number, number] => {
          let segStart = snaps[0].weekly_pct;
          let last = segStart;
          let total = 0;
          for (let i = 1; i < snaps.length; i += 1) {
            const pct = snaps[i].weekly_pct;
            if (pct < last) {
              total += Math.max(0, last - segStart);
              segStart = pct;
            }
            last = pct;
          }
          // 单段序列即 末值 − 首值，与旧公式一致
          total += Math.max(0, last - segStart);
          return [Math.max(0, total), snaps.length];
        };
        const out: Record<string, [number, number]> = {};
        for (const [acc, snaps] of mergedGroups) {
          // 合并序列可信（回落次数是真实重置量级）→ 用合并结果，
          // 多设备互补语义不变
          const ms = statsOf(snaps);
          if (ms.drops <= MAX_DROP_SEGMENTS) {
            out[acc] = segmentedOf(snaps);
            continue;
          }
          // 数据源打架：合并序列不可信。有界 ≠ 正确——回退公式的峰值同样
          // 取自脏值（线上实测：本机实际已用 43%，合并序列的脏峰值给出 87%），
          // 正确做法是**换数据源**：改用仅本机序列重算，同样走"回落统计 +
          // 分段累计/超限回退"完整算法。
          // 取舍：本机是用户当前查看的设备、凭证链路已在 1.5.5 收紧，
          // 可信度高于来源不明的远端批量数据；本机恰是脏源时不完美，
          // 属可接受取舍（详见函数文档注释）
          const localSnaps = localGroups.get(acc);
          if (localSnaps && localSnaps.length >= 2) {
            const ls = statsOf(localSnaps);
            if (ls.drops <= MAX_DROP_SEGMENTS) {
              out[acc] = segmentedOf(localSnaps);
            } else {
              // 本机序列自己也异常：无更可信的源可用，只能取有界值
              out[acc] = [Math.max(0, ls.peak - ls.first), localSnaps.length];
            }
            continue;
          }
          // 本机今天没采样该账号（非当前账号且 5 分钟轮询未覆盖等）：
          // 退回合并序列"峰值 − 首条"有界兜底（≤100，宁可少算不显示爆炸值）
          out[acc] = [Math.max(0, ms.peak - ms.first), snaps.length];
        }
        setAccountTodayDeltas(out);
      })
      .catch(() => {});
  }, [syncEnabled, syncConfig]);

  /** 读取并合并今日 Agent 额度快照，供详情页与汇总页共用。 */
  const loadAgentQuotaDeltas = useCallback(() => {
    const reqId = ++agentQuotaReqId.current;
    const from = todayStartMs();
    const to = Date.now() + 1;
    const localPromise = getAgentQuotaHistory(from, to);
    const wantRemote = syncEnabled && deviceFilter !== "local" && !!syncConfig;
    const remotePromise = wantRemote
      ? remoteAgentQuotaSnapshots(from, to, deviceFilter === "all"
          ? { excludeDevice: syncConfig!.device_id }
          : { devices: deviceFilter })
      : Promise.resolve([]);

    Promise.all([
      localPromise,
      remotePromise.catch(() => []),
    ])
      .then(([local, remote]) => {
        if (reqId !== agentQuotaReqId.current) return;
        const localSelected = deviceFilter === "local" || deviceFilter === "all"
          ? local
          : [];
        // 旧版本设备可能仍在上传「美元/百分比口径混用」的失真 Cursor 快照
        // （如 cursor_auto=65.02），合并前先用本机最新实时 plan 百分比作锚点过滤。
        // cursorCacheRef 读最新值，避免把 cache 加进本回调的依赖。
        const filteredRemote = remote.length > 0
          ? filterInflatedRemoteCursorSnapshots(
              remote,
              buildCursorQuotaAnchors(cursorCacheRef.current)
            )
          : remote;
        const merged = mergeAgentQuotaSnapshots(localSelected, filteredRemote);
        setAgentQuotaDeltas(calculateAgentQuotaDeltas(merged, from, to));
      })
      .catch(() => {
        if (reqId === agentQuotaReqId.current) setAgentQuotaDeltas({});
      });
  }, [deviceFilter, syncConfig, syncEnabled]);

  // 刷新函数定义在数据源刷新函数之后，因此用 ref 让前面的完成回调
  // 能调用到最新的设备筛选与同步配置。
  agentQuotaDeltaRefreshRef.current = loadAgentQuotaDeltas;

  // 设备或同步身份切换时立即作废上一轮结果，避免新筛选器短暂显示旧设备的增量。
  useEffect(() => {
    agentQuotaReqId.current += 1;
    setAgentQuotaDeltas({});
  }, [deviceFilter, syncConfig?.device_id, syncEnabled]);

  // 初次加载：同步配置 + 设备列表（仅一次）
  useEffect(() => {
    getSyncConfig()
      .then((cfg) => {
        setSyncConfig(cfg);
        if (cfg.enabled && cfg.device_token) {
          listRemoteDevices().then(setRemoteDevices).catch(() => {});
        }
      })
      .catch(() => {});
  }, []);

  // ===== 后台定时刷新 z.ai + Codex + Claude：当前 deviceFilter 的所有预设范围（与 preset/custom 解耦）。
  //      每 30s 刷一遍 today/1d/7d/30d → 切换预设范围时缓存命中、秒显。
  //      用户停留在自定义视图时，当前 custom 范围也顺带刷一轮（复用既有
  //      refreshZaiRange/refreshAgentRange 路径与 inflight 去重；custom 缓存 key 独立，
  //      不混淆预设缓存）。Codex/Claude 与 z.ai 同一轮刷新，无需单独定时器错峰。
  //      依赖不含 preset/custom：预设范围的 resolveRange 忽略 custom，custom 经 ref 读最新值。 =====
  useEffect(() => {
    const df = deviceFilter;
    const tick = () => {
      for (const p of PRESET_RANGES) {
        const [f, t] = resolveRange(p, custom);
        refreshZaiRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshCodexRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshClaudeRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshKimiRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshDshRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshOpencodeRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshMinimaxcodeRange(df, `${df}|${p}`, f, t, bucketOf(p));
      }
      // 自定义视图当前展示的范围纳入本轮刷新：custom 无法预缓存（日期区间任意），
      // 不顺带刷的话该视图只能靠按需补刷，lastUpdate 会长期显示旧时间
      if (presetRef.current === "custom") {
        const key = `${df}|${rangeKey("custom", customRef.current)}`;
        const [f, t] = resolveRange("custom", customRef.current);
        refreshZaiRange(df, key, f, t, bucketOf("custom"));
        refreshCodexRange(df, key, f, t, bucketOf("custom"));
        refreshClaudeRange(df, key, f, t, bucketOf("custom"));
        refreshKimiRange(df, key, f, t, bucketOf("custom"));
        refreshDshRange(df, key, f, t, bucketOf("custom"));
        refreshOpencodeRange(df, key, f, t, bucketOf("custom"));
        refreshMinimaxcodeRange(df, key, f, t, bucketOf("custom"));
      }
    };
    // 延后首刷（500ms）：WebView 重载（首次打开 / 长期隐藏后被系统回收）后，
    // 首屏先靠 localStorage 缓存渲染；挂载即并发发起十几个数据命令的话，
    // 响应陆续回来的主线程处理（IPC 解析 + 全量缓存写盘）会推迟首帧，造成白屏
    const first = setTimeout(tick, 500);
    const timer = setInterval(tick, 30_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
    // 故意不把 preset/custom 列入依赖：后台刷预设范围与二者无关（custom 仅在
    // 自定义视图时经 ref 顺带刷新），列入会导致每次切范围重建定时器
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceFilter, syncConfig, syncEnabled, pricing, refreshZaiRange, refreshCodexRange, refreshClaudeRange, refreshKimiRange, refreshDshRange, refreshOpencodeRange, refreshMinimaxcodeRange]);

  // ===== 后台定时刷新 Cursor：所有预设范围，降频 180s（网络慢，4 范围并行）。
  //      账号级，不受 deviceFilter 影响。汇率每 tick 只读一次（与范围循环解耦）。
  //      用户停留在自定义视图时，当前 custom 范围也顺带刷一轮（与 z.ai 后台
  //      tick 同款 ref 模式；cursorCache 的 inflight 去重复用）。 =====
  useEffect(() => {
    const tick = () => {
      // 每 tick 读一次汇率（用户可能在设置页改过），避免每个范围重复读。
      // >0 校验：非法值（手改文件等）保持现值，与后端 load_fx_rate 的 7.2 兜底口径一致
      getCursorConfig()
        .then((cfg) => {
          if (cfg.usd_cny_rate > 0) setFxRate(cfg.usd_cny_rate);
        })
        .catch(() => {});
      for (const p of PRESET_RANGES) {
        const [f, t] = resolveRange(p, custom);
        refreshCursorRange(p, f, t);
      }
      // 自定义视图当前范围纳入本轮：lastUpdate 取 zai/cursor 两者较旧值，
      // 只刷 z.ai 不刷 cursor 的话仍会被旧 cursor 条目拉低
      if (presetRef.current === "custom") {
        const [f, t] = resolveRange("custom", customRef.current);
        refreshCursorRange(rangeKey("custom", customRef.current), f, t);
      }
    };
    // 延后首刷（1.5s）错峰：避免与 z.ai 首刷、首屏渲染竞争主线程
    const first = setTimeout(tick, 1_500);
    const timer = setInterval(tick, 180_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshCursorRange]);

  // Quota 定时 30s（与范围无关）。延后首刷（2.5s）错峰，避免与首屏渲染竞争
  useEffect(() => {
    const first = setTimeout(loadQuota, 2_500);
    const timer = setInterval(loadQuota, 30_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
  }, [loadQuota]);

  // 多账号额度定时 60s：非当前账号的额度查询兼今日增量采样，5 分钟一轮
  // 会漏掉轻度账号两次采样间的百分比变化（增量恒为 0 不显示）；当前账号
  // 条目展示时仍由 30s 的 quota 实时覆盖（见下方 value 派生）。
  // 延后首刷（3.5s）错峰。
  useEffect(() => {
    const first = setTimeout(loadAccountQuotas, 3_500);
    const timer = setInterval(loadAccountQuotas, 60_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
  }, [loadAccountQuotas]);

  // 各账号今日增量（本机+远端合并）与多账号额度同频刷新；依赖 syncConfig，
  // 配置异步就绪后本函数重建、effect 重跑，远端部分自动生效。
  useEffect(() => {
    const first = setTimeout(loadAccountTodayDeltas, 4_000);
    const timer = setInterval(loadAccountTodayDeltas, 60_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
  }, [loadAccountTodayDeltas]);

  // Agent 额度快照在各数据源首刷后读取，后续与额度查询同频刷新。
  useEffect(() => {
    const first = setTimeout(loadAgentQuotaDeltas, 3_000);
    const timer = setInterval(loadAgentQuotaDeltas, 30_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
  }, [loadAgentQuotaDeltas]);

  // ===== 通用 provider 额度轮询（120s，独立于上面各循环）：只查已有凭证的
  //      provider（presence 经 ref 读最新值，定时器不因 presence 变化重建）。
  //      未接入的 provider 即使有凭证，后端也返回空数组不打网络，轮询无害。 =====
  useEffect(() => {
    const tick = () => {
      for (const [provider, has] of Object.entries(
        credentialPresenceRef.current
      )) {
        if (has) void refreshProviderQuota(provider);
      }
    };
    // 延后首刷（5.5s）错峰：等 App 层 presence 批量探测完成，首轮才有目标
    const first = setTimeout(tick, 5_500);
    const timer = setInterval(tick, 120_000);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
  }, [refreshProviderQuota]);

  // 窗口恢复可见时补刷全部已有凭证的 provider 额度：隐藏期间 setInterval
  // 常被节流，恢复后立即补齐（与现有 visibilitychange 补刷同款思路，独立监听）
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      for (const [provider, has] of Object.entries(
        credentialPresenceRef.current
      )) {
        if (has) void refreshProviderQuota(provider);
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [refreshProviderQuota]);

  // ===== 按需补刷：切到无缓存/过期范围，或配置就绪后补一次。
  //      预设范围通常已被后台任务刷新 → 命中新鲜缓存 → 不请求、秒显。
  //      依赖含 refreshZaiRange/refreshCursorRange/refreshCodexRange：pricing/syncConfig
  //      就绪后函数重建，本 effect 重跑 → custom 范围用就绪配置重刷自愈（修复冷启动覆盖问题）。 =====
  useEffect(() => {
    const [f, t] = resolveRange(preset, custom);
    const zKey = `${deviceFilter}|${rangeKey(preset, custom)}`;
    const zEntry = zaiCacheRef.current[zKey];
    if (!zEntry || Date.now() - zEntry.ts > ZAI_STALE_MS) {
      refreshZaiRange(deviceFilter, zKey, f, t, trendBucket);
    }
    const cKey = rangeKey(preset, custom);
    const cEntry = cursorCacheRef.current[cKey];
    if (!cEntry || Date.now() - cEntry.ts > CURSOR_STALE_MS) {
      refreshCursorRange(cKey, f, t);
    }
    const xEntry = codexCacheRef.current[zKey];
    if (!xEntry || Date.now() - xEntry.ts > CODEX_STALE_MS) {
      refreshCodexRange(deviceFilter, zKey, f, t, trendBucket);
    }
    const aEntry = claudeCacheRef.current[zKey];
    if (!aEntry || Date.now() - aEntry.ts > CLAUDE_STALE_MS) {
      refreshClaudeRange(deviceFilter, zKey, f, t, trendBucket);
    }
    const kEntry = kimiCacheRef.current[zKey];
    if (!kEntry || Date.now() - kEntry.ts > KIMI_STALE_MS) {
      refreshKimiRange(deviceFilter, zKey, f, t, trendBucket);
    }
    const dEntry = dshCacheRef.current[zKey];
    if (!dEntry || Date.now() - dEntry.ts > DSH_STALE_MS) {
      refreshDshRange(deviceFilter, zKey, f, t, trendBucket);
    }
    const oEntry = opencodeCacheRef.current[zKey];
    if (!oEntry || Date.now() - oEntry.ts > OPENCODE_STALE_MS) {
      refreshOpencodeRange(deviceFilter, zKey, f, t, trendBucket);
    }
    const mEntry = minimaxcodeCacheRef.current[zKey];
    if (!mEntry || Date.now() - mEntry.ts > MINIMAXCODE_STALE_MS) {
      refreshMinimaxcodeRange(deviceFilter, zKey, f, t, trendBucket);
    }
    // 仅在范围/设备/刷新函数变化时触发；不依赖 cache 内容（靠 ref 读最新值）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preset, custom, deviceFilter, trendBucket, refreshZaiRange, refreshCursorRange, refreshCodexRange, refreshClaudeRange, refreshKimiRange, refreshDshRange, refreshOpencodeRange, refreshMinimaxcodeRange]);

  // ===== 窗口恢复可见时主动补刷：隐藏期间 setInterval 常被节流，恢复后立即补齐
  //      当前 deviceFilter 的预设范围，保证"常驻新鲜"。
  //      用户停留在自定义视图时，当前 custom 范围（z.ai + Codex + Cursor）同样补刷。 =====
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      const df = deviceFilter;
      for (const p of PRESET_RANGES) {
        const [f, t] = resolveRange(p, custom);
        refreshZaiRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshCodexRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshClaudeRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshKimiRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshDshRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshOpencodeRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshMinimaxcodeRange(df, `${df}|${p}`, f, t, bucketOf(p));
        refreshCursorRange(p, f, t);
      }
      if (presetRef.current === "custom") {
        const cKey = rangeKey("custom", customRef.current);
        const [f, t] = resolveRange("custom", customRef.current);
        refreshZaiRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshCodexRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshClaudeRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshKimiRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshDshRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshOpencodeRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshMinimaxcodeRange(df, `${df}|${cKey}`, f, t, bucketOf("custom"));
        refreshCursorRange(cKey, f, t);
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceFilter, refreshZaiRange, refreshCursorRange, refreshCodexRange, refreshClaudeRange, refreshKimiRange, refreshDshRange, refreshOpencodeRange, refreshMinimaxcodeRange]);

  // ===== 持久化：小对象（体积小，序列化无感知）state 变化即同步落盘；
  //      大缓存（zai/cursor/codex/claude/kimi/dsh/opencode/minimaxcode/
  //      todayDelta/accountQuotas/providerQuota）走 500ms 防抖 + 空闲期写盘
  //      调度器（见 schedulePersist），
  //      避免后台刷新轮里陆续到达的各路数据反复全量序列化阻塞主线程。
  //      供下次冷启动各范围秒显 =====
  useEffect(() => {
    saveCache("zbar-preset", preset);
  }, [preset]);
  useEffect(() => {
    saveCache("zbar-custom", custom);
  }, [custom]);
  useEffect(() => {
    saveCache("zbar-device", deviceFilter);
  }, [deviceFilter]);
  // 大缓存防抖写盘。zai/claude/kimi 在速度契约迁移轮（speedMigrationSyncWrite）
  // 改用同步 saveCache：迁移结果必须先于下方版本标记落盘（同一次 commit 的
  // effect 按声明顺序同步执行，不受防抖影响），时序语义与既有约定一致；
  // 开关仅在迁移会话为 true 且写完标记即复位，正常路径恒走防抖调度器。
  useEffect(() => {
    if (speedMigrationSyncWrite) saveCache("zbar-zai-cache", zaiCache);
    else schedulePersist("zbar-zai-cache", zaiCache);
  }, [zaiCache]);
  useEffect(() => {
    schedulePersist("zbar-cursor-cache", cursorCache);
  }, [cursorCache]);
  useEffect(() => {
    schedulePersist("zbar-codex-cache", codexCache);
  }, [codexCache]);
  useEffect(() => {
    if (speedMigrationSyncWrite) saveCache("zbar-claude-cache", claudeCache);
    else schedulePersist("zbar-claude-cache", claudeCache);
  }, [claudeCache]);
  useEffect(() => {
    if (speedMigrationSyncWrite) saveCache("zbar-kimi-cache", kimiCache);
    else schedulePersist("zbar-kimi-cache", kimiCache);
  }, [kimiCache]);
  useEffect(() => {
    schedulePersist("zbar-dsh-cache", dshCache);
  }, [dshCache]);
  useEffect(() => {
    schedulePersist("zbar-opencode-cache", opencodeCache);
  }, [opencodeCache]);
  useEffect(() => {
    schedulePersist("zbar-minimaxcode-cache", minimaxcodeCache);
  }, [minimaxcodeCache]);
  // 版本标记写入：必须位于 zai/claude/kimi 三个持久化 effect 之后。同一次
  // commit 的 effect 按声明顺序同步执行，挂载轮先落盘迁移后的缓存、再写
  // 版本标记；若两步之间进程被杀，只会残留「版本=旧 + 缓存=已剥离」，
  // 下次冷启动幂等重跑迁移（isLegacySpeedRow 对新契约行是无操作）。反之
  // 若先写标记，极端窗口会留下「版本=新 + localStorage 仍为旧口径速度
  // 条目」，旧 avg_tps 会被当新契约显示。
  // 大缓存改防抖异步写后该时序仍成立：迁移轮下三个缓存 effect 改用同步
  // saveCache（见上方各 effect），与本 effect 的先后顺序不受防抖影响；标记
  // 落盘后立即复位开关，本会话后续更新恢复防抖写（迁移时序使命已完成）。
  // 版本已最新时本 effect 幂等空操作（StrictMode 双跑无害）
  useEffect(() => {
    if (speedMigrationSyncWrite) {
      saveCache(SPEED_VERSION_CACHE_KEY, SPEED_CONTRACT_VERSION);
      speedMigrationSyncWrite = false;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    saveCache("zbar-fxrate", fxRate);
  }, [fxRate]);
  useEffect(() => {
    if (quota) saveCache("zbar-quota", quota);
  }, [quota]);
  useEffect(() => {
    // 带日期戳持久化：冷启动由 loadTodayDeltaCache 校验日期，跨天残留不恢复
    if (todayDelta) {
      schedulePersist("zbar-today-delta", {
        date: dateStr(Date.now()),
        delta: todayDelta,
      } satisfies DatedTodayDelta);
    }
  }, [todayDelta]);
  useEffect(() => {
    schedulePersist("zbar-account-quotas", accountQuotas);
  }, [accountQuotas]);
  useEffect(() => {
    // ProviderQuotaEntry 不含 secret（凭证明文永不下发前端），可安全持久化
    schedulePersist("zbar-provider-quota", providerQuota);
  }, [providerQuota]);
  // 大缓存防抖写盘的兜底防丢：页面隐藏/关闭时同步刷写全部 pending（进程可能
  // 随即被挂起/杀死，空闲回调不再执行）；组件卸载同理。挂载轮的迁移同步写
  // 不经过 pending，不受影响。
  useEffect(() => {
    const onHidden = () => {
      if (document.visibilityState === "hidden") flushPersistNow();
    };
    const onBeforeUnload = () => flushPersistNow();
    document.addEventListener("visibilitychange", onHidden);
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => {
      document.removeEventListener("visibilitychange", onHidden);
      window.removeEventListener("beforeunload", onBeforeUnload);
      // 卸载兜底：刷写尚未落盘的 pending
      flushPersistNow();
    };
  }, []);

  // 手动刷新：刷新当前范围（z.ai + Codex + Claude + Cursor + Kimi + DSH + OpenCode
  // + MiniMax Code）一次
  const refresh = useCallback(() => {
    const [f, t] = resolveRange(preset, custom);
    const zKey = `${deviceFilter}|${rangeKey(preset, custom)}`;
    refreshZaiRange(deviceFilter, zKey, f, t, trendBucket);
    refreshCodexRange(deviceFilter, zKey, f, t, trendBucket);
    refreshClaudeRange(deviceFilter, zKey, f, t, trendBucket);
    refreshKimiRange(deviceFilter, zKey, f, t, trendBucket);
    refreshDshRange(deviceFilter, zKey, f, t, trendBucket);
    refreshOpencodeRange(deviceFilter, zKey, f, t, trendBucket);
    refreshMinimaxcodeRange(deviceFilter, zKey, f, t, trendBucket);
    refreshCursorRange(rangeKey(preset, custom), f, t);
    // 额度采样由上述后台命令写入，稍后读取以覆盖本次手动刷新产生的快照。
    window.setTimeout(loadAgentQuotaDeltas, 1_000);
  }, [preset, custom, deviceFilter, trendBucket, refreshZaiRange, refreshCodexRange, refreshClaudeRange, refreshKimiRange, refreshDshRange, refreshOpencodeRange, refreshMinimaxcodeRange, refreshCursorRange, loadAgentQuotaDeltas]);

  const refreshQuota = useCallback(() => loadQuota(), [loadQuota]);
  const refreshAccountQuotas = useCallback(
    () => loadAccountQuotas(),
    [loadAccountQuotas]
  );

  // ===== 当前范围数据（展示层只读这些）=====
  const curZaiKey = `${deviceFilter}|${rangeKey(preset, custom)}`;
  const curCursorKey = rangeKey(preset, custom);
  const curZai = zaiCache[curZaiKey] ?? EMPTY_ZAI;
  const curCursor = cursorCache[curCursorKey] ?? EMPTY_CURSOR;
  const curCodex = codexCache[curZaiKey] ?? EMPTY_CODEX;
  const curClaude = claudeCache[curZaiKey] ?? EMPTY_CLAUDE;
  const curKimi = kimiCache[curZaiKey] ?? EMPTY_KIMI;
  const curDsh = dshCache[curZaiKey] ?? EMPTY_DSH;
  const curOpencode = opencodeCache[curZaiKey] ?? EMPTY_OPENCODE;
  const curMinimaxcode = minimaxcodeCache[curZaiKey] ?? EMPTY_MINIMAXCODE;

  // lastUpdate 取当前范围各数据源里最旧的成功时间（保守的新鲜度下限）
  const updateTs = [
    curZai.ts,
    curCursor.ts,
    curCodex.ts,
    curClaude.ts,
    curKimi.ts,
    curDsh.ts,
    curOpencode.ts,
    curMinimaxcode.ts,
  ].filter((t) => t > 0);
  const lastUpdate = updateTs.length ? Math.min(...updateTs) : 0;
  const refreshing =
    curZai.refreshing ||
    curCursor.refreshing ||
    curCodex.refreshing ||
    curClaude.refreshing ||
    curKimi.refreshing ||
    curDsh.refreshing ||
    curOpencode.refreshing ||
    curMinimaxcode.refreshing;

  // 展示派生 1：各账号今日增量优先用"本机+远端合并"计算值（多端同步，
  // 覆盖本机离线时段），未就绪时退回后端 account_quotas 的本机计算值。
  // 展示派生 2：当前账号条目用 30s live quota 覆盖（用量实时变化，60s 一轮的
  // 多账号查询跟不上）。仅当 live 一路更新（时间戳较新）时才覆盖——切换账号后
  // 两路数据到达顺序不定，旧一路的额度可能是另一个账号的；live 查询失败
  // （quota=null 或时间戳较旧）时保留后端值，避免闪空或错配
  const accountQuotasMerged = useMemo(
    () =>
      accountQuotas.map((e) => {
        const delta = accountTodayDeltas[e.fingerprint] ?? e.today_delta ?? null;
        return e.is_current && quota && quotaTsRef.current >= accountQuotasTsRef.current
          ? { ...e, quota, today_delta: delta }
          : { ...e, today_delta: delta };
      }),
    [accountQuotas, quota, accountTodayDeltas]
  );

  // ref 镜像同步：自动切换检查（loadQuota 闭包）经 ref 读最新合并列表，
  // 避免闭包拿到旧值（accountQuotasMerged 定义在 loadQuota 之后，无法直接引用）
  useEffect(() => {
    accountQuotasMergedRef.current = accountQuotasMerged;
  }, [accountQuotasMerged]);

  // 当前账号今日增量：多端合并值优先（口径与各账号行一致），未就绪时退回
  // 本机 30s 路径的全局值（get_today_delta 按当前指纹过滤）
  const currentAccountFp =
    accountQuotas.find((e) => e.is_current)?.fingerprint ?? "";
  const todayDeltaMerged =
    (currentAccountFp ? accountTodayDeltas[currentAccountFp] : undefined) ??
    todayDelta;

  const value = useMemo<DataCacheValue>(
    () => ({
      preset,
      custom,
      trendBucket,
      deviceFilter,
      setPreset,
      setCustom,
      setDeviceFilter,
      // 展示前把仅大小写/不可见字符/provider 差异的模型行折叠为一行（fold 幂等）。
      // 必须在派生层做：本页数据含 localStorage 冷启动旧缓存，不经 mergeStats。
      stats: foldByModelStats(curZai.stats),
      cost: curZai.cost,
      trend: curZai.trend,
      error: curZai.error,
      codex: curCodex.snapshot
        ? {
            ...curCodex.snapshot,
            stats: foldByModelStats(curCodex.snapshot.stats),
          }
        : curCodex.snapshot,
      codexError: curCodex.error,
      claude: curClaude.snapshot
        ? {
            ...curClaude.snapshot,
            stats: foldByModelStats(curClaude.snapshot.stats),
          }
        : curClaude.snapshot,
      claudeError: curClaude.error,
      kimi: curKimi.snapshot
        ? {
            ...curKimi.snapshot,
            stats: foldByModelStats(curKimi.snapshot.stats),
          }
        : curKimi.snapshot,
      kimiError: curKimi.error,
      dsh: curDsh.snapshot
        ? {
            ...curDsh.snapshot,
            stats: foldByModelStats(curDsh.snapshot.stats),
          }
        : curDsh.snapshot,
      dshError: curDsh.error,
      opencode: curOpencode.snapshot
        ? {
            ...curOpencode.snapshot,
            stats: foldByModelStats(curOpencode.snapshot.stats),
          }
        : curOpencode.snapshot,
      opencodeError: curOpencode.error,
      minimaxcode: curMinimaxcode.snapshot
        ? {
            ...curMinimaxcode.snapshot,
            stats: foldByModelStats(curMinimaxcode.snapshot.stats),
          }
        : curMinimaxcode.snapshot,
      minimaxcodeError: curMinimaxcode.error,
      cursor: curCursor.snapshot
        ? {
            ...curCursor.snapshot,
            by_model: foldCursorModelRows(curCursor.snapshot.by_model),
          }
        : curCursor.snapshot,
      cursorError: curCursor.error,
      fxRate,
      quota,
      todayDelta: todayDeltaMerged,
      agentQuotaDeltas,
      quotaError,
      accountQuotas: accountQuotasMerged,
      refreshAccountQuotas,
      syncConfig,
      remoteDevices,
      syncEnabled,
      credentials,
      refreshCredentials,
      providerQuota,
      refreshProviderQuota,
      lastUpdate,
      refreshing,
      refresh,
      refreshQuota,
    }),
    [
      preset,
      custom,
      trendBucket,
      deviceFilter,
      curZai,
      curCodex,
      curClaude,
      curCursor,
      curKimi,
      curDsh,
      curOpencode,
      curMinimaxcode,
      fxRate,
      quota,
      todayDeltaMerged,
      agentQuotaDeltas,
      quotaError,
      accountQuotasMerged,
      refreshAccountQuotas,
      syncConfig,
      remoteDevices,
      syncEnabled,
      credentials,
      refreshCredentials,
      providerQuota,
      refreshProviderQuota,
      lastUpdate,
      refreshing,
      refresh,
      refreshQuota,
    ],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** 读取全局数据缓存。必须在 <DataProvider> 内使用。 */
export function useDataCache(): DataCacheValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useDataCache must be used within a DataProvider");
  return v;
}
