/**
 * 额度驾驶舱页（主窗口第五页）的纯数据模型与聚合工具：无 React、无 IPC。
 *
 * 三个本地历史表 → 统一「系列 × 窗口」模型：
 * - zai：quota_history（QuotaSnapshot），**账号指纹维度**（每账号一个系列，
 *   窗口 = 周额度 weekly / 5 小时窗 hour5 / MCP 月度 mcp）；
 * - codex / claude / cursor / kimi：agent_quota_history（CLI 登录态通道），
 *   源级粒度（每源一个系列，窗口键 hour5 / weekly / cursor_auto / cursor_api）；
 * - 其余 11 个凭证型 provider：provider_quota_history，**凭证 label 维度**
 *   （每凭证一个系列，窗口键为各 provider 归一后的稳定键：hour5 / weekly /
 *   monthly / credits 等）。
 *
 * 口径纪律（重要）：三张表的百分比字段（weekly_pct / hour5_pct / mcp_pct /
 * used_pct）全部是**已用**口径；本页对外统一暴露**剩余**（= 100 − 已用），
 * 与总览页 Agent 表的 remainingPct 口径一致（HUD 停靠条才是已用口径）。
 * 系列构建阶段就完成换算，视图层不再碰已用值。
 */
import type {
  AccountMeta,
  AgentQuotaSnapshot,
  ProviderQuotaHistorySnapshot,
  QuotaHistoryWindow,
  QuotaSnapshot,
} from "../../types";
import { AGENT_COLOR, type AgentId } from "../../agentVisibility";
import type { BrandIconName } from "../../BrandIcon";

/** 本页参与的来源（顺序即区块① 的分组展示顺序）。全部是 AgentId 域的
 *  合法值（BrandIcon 品牌名同字面、AGENT_COLOR 有对应色）。
 *  gemini / dsh / doubao 没有额度历史通道，不列——它们不会出现在
 *  「暂无历史数据」名单里（本页只谈有额度采样通道的源）。 */
export type QuotaCockpitSource =
  | "zai"
  | "codex"
  | "claude"
  | "cursor"
  | "kimi"
  | "deepseek"
  | "moonshot"
  | "minimax"
  | "opencodego"
  | "alibaba"
  | "qoder"
  | "longcat"
  | "mimo"
  | "stepfun"
  | "grok"
  | "alibabatoken";

export const QUOTA_COCKPIT_SOURCES: readonly QuotaCockpitSource[] = [
  "zai",
  "codex",
  "claude",
  "cursor",
  "kimi",
  "deepseek",
  "moonshot",
  "minimax",
  "opencodego",
  "alibaba",
  "qoder",
  "longcat",
  "mimo",
  "stepfun",
  "grok",
  "alibabatoken",
];

/** CLI 通道来源（agent_quota_history 只写这四个；provider 通道刻意不含） */
const CLI_SOURCES: readonly QuotaCockpitSource[] = [
  "codex",
  "claude",
  "cursor",
  "kimi",
];

/** 来源展示元数据（label 与对比页 compare/model.ts 的 AGENT_META 同名同值，
 *  该表未导出，此处为本页独立登记；品牌名与 AgentId 同字面） */
const SOURCE_META: Record<
  QuotaCockpitSource,
  { label: string; brand: BrandIconName }
> = {
  zai: { label: "Z.ai", brand: "zai" },
  codex: { label: "Codex", brand: "codex" },
  claude: { label: "Claude", brand: "claude" },
  cursor: { label: "Cursor", brand: "cursor" },
  kimi: { label: "Kimi Code", brand: "kimi" },
  deepseek: { label: "DeepSeek", brand: "deepseek" },
  moonshot: { label: "Kimi", brand: "moonshot" },
  minimax: { label: "MiniMax Code", brand: "minimax" },
  opencodego: { label: "OpenCode", brand: "opencodego" },
  alibaba: { label: "通义灵码", brand: "alibaba" },
  qoder: { label: "Qoder", brand: "qoder" },
  longcat: { label: "LongCat", brand: "longcat" },
  mimo: { label: "MiMo", brand: "mimo" },
  stepfun: { label: "StepFun", brand: "stepfun" },
  grok: { label: "Grok", brand: "grok" },
  alibabatoken: { label: "百炼Token包", brand: "alibabatoken" },
};

export function sourceLabelOf(source: QuotaCockpitSource): string {
  return SOURCE_META[source].label;
}

export function sourceBrandOf(source: QuotaCockpitSource): BrandIconName {
  return SOURCE_META[source].brand;
}

export function sourceColorOf(source: QuotaCockpitSource): string {
  return AGENT_COLOR[source as AgentId];
}

/** 窗口展示顺序（区块① 行序 / 趋势窗口选择器排序共用）；未列出的键
 *  （如各 provider 特有的 credits 等）排在其后按字典序。 */
const WINDOW_ORDER: readonly string[] = [
  "weekly",
  "hour5",
  "monthly",
  "cursor_auto",
  "cursor_api",
  "mcp",
];

export function windowOrderOf(key: string): number {
  const index = WINDOW_ORDER.indexOf(key);
  return index >= 0 ? index : WINDOW_ORDER.length;
}

/** 系列的主窗口（趋势卡默认窗口）：周额度优先，其次月度、Cursor Auto、
 *  5 小时窗；其余按展示序取第一个。 */
const PRIMARY_WINDOW_PREFERENCE: readonly string[] = [
  "weekly",
  "monthly",
  "cursor_auto",
  "hour5",
  "cursor_api",
];

export function primaryWindowOf(series: CockpitSeries): string {
  for (const key of PRIMARY_WINDOW_PREFERENCE) {
    if (series.windows.some((w) => w.key === key)) return key;
  }
  return series.windows[0]?.key ?? "";
}

// ===== 系列与采样 =====

/** 单个窗口的一条采样（usedPct 为已用 0-100，已收敛；对外换算剩余） */
export interface CockpitSample {
  ts: number;
  usedPct: number;
  resetAt: number | null;
}

/** 一个窗口的升序采样序列 */
export interface CockpitWindowTimeline {
  key: string;
  samples: CockpitSample[];
}

/** 一个额度数据系列：zai 每账号一个、CLI 每源一个、provider 每凭证一个 */
export interface CockpitSeries {
  /** 稳定 id：zai:<指纹> | <cli 源名> | <provider 源名>:<label> */
  id: string;
  source: QuotaCockpitSource;
  /** 展示名：单系列时为源名；同源多系列时「源名·账号/凭证名」 */
  label: string;
  /** 账号维度子标签（指纹显示名 / 凭证备注名；源级粒度为 null） */
  accountName: string | null;
  /** 套餐标识（最新采样的 plan_type / plan_name / level；无则 null） */
  plan: string | null;
  windows: CockpitWindowTimeline[];
}

/** 快照百分比统一按 0-100 收敛；非法值视为无效采样（与对比页同款） */
function clampUsedPct(value: number | null | undefined): number | null {
  if (value == null || !Number.isFinite(value)) return null;
  return Math.min(100, Math.max(0, value));
}

function validResetAt(value: number | null | undefined): number | null {
  return value != null && Number.isFinite(value) ? value : null;
}

/** 构建系列输入（useQuotaData 三路本地历史 + 账号列表组装后传入） */
export interface CockpitBuildInput {
  zai: QuotaSnapshot[];
  agent: AgentQuotaSnapshot[];
  provider: ProviderQuotaHistorySnapshot[];
  accounts: AccountMeta[] | null;
}

/**
 * 把三张历史表的快照聚合为系列列表（按 QUOTA_COCKPIT_SOURCES 顺序）。
 *
 * zai 指纹分组规则与对比页 buildQuotaTimelines 一致：存在带指纹的采样时
 * 无指纹旧快照整体丢弃（归属无法分辨，混入即跨账号失真）；全部都是无指纹
 * 的纯旧版客户端才保留一条不区分账号的 "Z.ai" 系列。
 */
export function buildCockpitSeries(input: CockpitBuildInput): CockpitSeries[] {
  const series: CockpitSeries[] = [];

  // ---- zai：账号指纹维度 ----
  {
    const hasFingerprint = input.zai.some((s) => s.account);
    const usable = hasFingerprint
      ? input.zai.filter((s) => s.account)
      : input.zai;
    const groups = new Map<string, QuotaSnapshot[]>();
    for (const snap of usable) {
      if (!Number.isFinite(snap.ts)) continue;
      const key = snap.account ?? "";
      const list = groups.get(key) ?? [];
      list.push(snap);
      groups.set(key, list);
    }
    const keys = [...groups.keys()].sort((a, b) => a.localeCompare(b));
    const single = keys.length <= 1;
    for (const key of keys) {
      const snaps = groups.get(key) ?? [];
      const windows: CockpitWindowTimeline[] = [];
      const weekly = collectSamples(
        "weekly",
        snaps,
        (s) => clampUsedPct(s.weekly_pct),
        (s) => validResetAt(s.weekly_reset)
      );
      if (weekly) windows.push(weekly);
      // 5 小时窗快照无重置时刻字段（QuotaSnapshot 只记录已用百分比）
      const hour5 = collectSamples(
        "hour5",
        snaps,
        (s) => clampUsedPct(s.hour5_pct),
        () => null
      );
      if (hour5) windows.push(hour5);
      // MCP 月度：只有确实带 MCP 信息（有次数额度或非零百分比）才建窗口，
      // 避免给未开通 MCP 的账号渲染一条恒 0 的噪音行
      const hasMcp = snaps.some(
        (s) => (s.mcp_total ?? null) != null || (s.mcp_used ?? null) != null || s.mcp_pct > 0
      );
      if (hasMcp) {
        const mcp = collectSamples(
          "mcp",
          snaps,
          (s) => clampUsedPct(s.mcp_pct),
          () => null
        );
        if (mcp) windows.push(mcp);
      }
      const latest = snaps[snaps.length - 1];
      const name = single
        ? null
        : input.accounts?.find((a) => a.fingerprint === key)?.display_name?.trim() ||
          key.slice(0, 6);
      series.push({
        id: `zai:${key || "history"}`,
        source: "zai",
        label: name ? `${SOURCE_META.zai.label}·${name}` : SOURCE_META.zai.label,
        accountName: name,
        plan: latest ? latest.level || null : null,
        windows: sortWindows(windows),
      });
    }
  }

  // ---- CLI 通道：源级粒度（每源一个系列）----
  for (const source of CLI_SOURCES) {
    const snaps = input.agent.filter((s) => s.source === source);
    if (snaps.length === 0) continue;
    const seriesWindows = windowsFromAgentSnaps(snaps);
    if (seriesWindows.length === 0) continue;
    series.push({
      id: source,
      source,
      label: SOURCE_META[source].label,
      accountName: null,
      plan: latestPlanOf(snaps),
      windows: seriesWindows,
    });
  }

  // ---- provider 通道：凭证 label 维度 ----
  {
    const groups = new Map<string, ProviderQuotaHistorySnapshot[]>();
    for (const snap of input.provider) {
      const source = snap.source as QuotaCockpitSource;
      if (!QUOTA_COCKPIT_SOURCES.includes(source) || CLI_SOURCES.includes(source)) {
        continue; // 防御：只认 provider 通道的 11 个来源
      }
      if (!Number.isFinite(snap.ts)) continue;
      const key = `${snap.source}|${snap.label ?? ""}`;
      const list = groups.get(key) ?? [];
      list.push(snap);
      groups.set(key, list);
    }
    // 分组键显式用 "|" 分隔；拆分时只切第一个分隔符，凭证备注名本身含 "|"
    // 不会被截断
    for (const [key, snaps] of groups) {
      const sep = key.indexOf("|");
      const source = key.slice(0, sep) as QuotaCockpitSource;
      const label = sep >= 0 ? key.slice(sep + 1) : "";
      const seriesWindows = windowsFromAgentSnaps(snaps);
      if (seriesWindows.length === 0) continue;
      const name = label || null;
      series.push({
        id: `${source}:${label ?? ""}`,
        source,
        label: name
          ? `${SOURCE_META[source].label}·${name}`
          : SOURCE_META[source].label,
        accountName: name,
        plan: latestPlanOf(snaps),
        windows: seriesWindows,
      });
    }
  }

  // 输出按来源展示序排列（同源内保持构建序：zai 指纹序 / provider 分组序）
  return series.sort((a, b) => {
    const order =
      QUOTA_COCKPIT_SOURCES.indexOf(a.source) -
      QUOTA_COCKPIT_SOURCES.indexOf(b.source);
    return order !== 0 ? order : a.id.localeCompare(b.id);
  });
}

/** 从快照数组收集一个窗口的采样序列；全部无效时返回 null（不建空窗口） */
function collectSamples(
  key: string,
  snaps: QuotaSnapshot[],
  usedPctOf: (s: QuotaSnapshot) => number | null,
  resetAtOf: (s: QuotaSnapshot) => number | null
): CockpitWindowTimeline | null {
  const samples: CockpitSample[] = [];
  for (const snap of snaps) {
    const usedPct = usedPctOf(snap);
    if (usedPct == null) continue;
    samples.push({ ts: snap.ts, usedPct, resetAt: resetAtOf(snap) });
  }
  if (samples.length === 0) return null;
  samples.sort((a, b) => a.ts - b.ts);
  return { key, samples };
}

/** AgentQuotaSnapshot / ProviderQuotaHistorySnapshot 形状相同（后者是
 *  同一 Rust 结构的宽类型），统一按窗口键分组构建窗口时间线。 */
function windowsFromAgentSnaps(
  snaps: readonly (AgentQuotaSnapshot | ProviderQuotaHistorySnapshot)[]
): CockpitWindowTimeline[] {
  const byKey = new Map<string, CockpitSample[]>();
  for (const snap of snaps) {
    for (const win of snap.windows as QuotaHistoryWindow[]) {
      const usedPct = clampUsedPct(win.used_pct);
      if (usedPct == null) continue;
      const list = byKey.get(win.key) ?? [];
      list.push({ ts: snap.ts, usedPct, resetAt: validResetAt(win.reset_at) });
      byKey.set(win.key, list);
    }
  }
  const windows: CockpitWindowTimeline[] = [];
  for (const [key, samples] of byKey) {
    samples.sort((a, b) => a.ts - b.ts);
    windows.push({ key, samples });
  }
  return sortWindows(windows);
}

/** 窗口内排序：weekly → hour5 → monthly → cursor_* → mcp → 其余字典序 */
function sortWindows(windows: CockpitWindowTimeline[]): CockpitWindowTimeline[] {
  return [...windows].sort((a, b) => {
    const order = windowOrderOf(a.key) - windowOrderOf(b.key);
    return order !== 0 ? order : a.key.localeCompare(b.key);
  });
}

/** 最新一条快照的套餐标识：provider 通道优先 plan_name，CLI 通道用 plan_type
 *  （AgentQuotaSnapshot 的窄类型没有 plan_name 字段——那是 provider 通道
 *  同构快照才携带的扩展字段，这里收窄到宽类型读取） */
function latestPlanOf(
  snaps: readonly (AgentQuotaSnapshot | ProviderQuotaHistorySnapshot)[]
): string | null {
  let latest: (AgentQuotaSnapshot | ProviderQuotaHistorySnapshot) | null = null;
  for (const snap of snaps) {
    if (!latest || snap.ts > latest.ts) latest = snap;
  }
  if (!latest) return null;
  return (
    latest.plan_type || (latest as ProviderQuotaHistorySnapshot).plan_name || null
  );
}

// ===== 当前值（区块①）=====

/** 一行「系列 × 窗口」的当前值（各取该窗口最新采样） */
export interface CockpitCurrentRow {
  series: CockpitSeries;
  windowKey: string;
  /** 剩余 %（= 100 − 已用，0-100） */
  remainingPct: number;
  resetAt: number | null;
  /** 该窗口最新采样时刻 */
  sampledAt: number;
}

/** 系列的全部当前行；按剩余升序（最紧的在前，页面分组内的行排序口径） */
export function currentRowsOf(series: CockpitSeries): CockpitCurrentRow[] {
  const rows: CockpitCurrentRow[] = [];
  for (const win of series.windows) {
    const latest = win.samples[win.samples.length - 1];
    if (!latest) continue;
    rows.push({
      series,
      windowKey: win.key,
      remainingPct: 100 - latest.usedPct,
      resetAt: latest.resetAt,
      sampledAt: latest.ts,
    });
  }
  rows.sort((a, b) => a.remainingPct - b.remainingPct);
  return rows;
}

/** 系列的最紧剩余（组内系列排序用）：各窗口最新剩余的最小值；无窗口为 null */
export function worstRemainingOf(series: CockpitSeries): number | null {
  let worst: number | null = null;
  for (const row of currentRowsOf(series)) {
    if (worst == null || row.remainingPct < worst) worst = row.remainingPct;
  }
  return worst;
}

/** 系列的最新采样时刻（跨窗口取最大）；无窗口为 null */
export function latestSampleAtOf(series: CockpitSeries): number | null {
  let latest: number | null = null;
  for (const win of series.windows) {
    const ts = win.samples[win.samples.length - 1]?.ts;
    if (ts != null && (latest == null || ts > latest)) latest = ts;
  }
  return latest;
}

/** 当前系列按窗口展示序（weekly → hour5 → ...）的最新行，供仪表卡渲染。
 *  与 currentRowsOf 的「剩余升序」不同：卡片内仪表按固定窗口顺序排列，
 *  用户在不同账号之间扫视时列位稳定。 */
export function currentRowsInOrderOf(series: CockpitSeries): CockpitCurrentRow[] {
  const rows: CockpitCurrentRow[] = [];
  for (const win of series.windows) {
    const latest = win.samples[win.samples.length - 1];
    if (!latest) continue;
    rows.push({
      series,
      windowKey: win.key,
      remainingPct: 100 - latest.usedPct,
      resetAt: latest.resetAt,
      sampledAt: latest.ts,
    });
  }
  return rows;
}

// ===== 额度风险分级与总览摘要（驾驶舱控制台）=====

/** 风险分级（剩余口径）：≤20% 告急、20–50% 偏低、>50% 正常。
 *  分级只用于「态势」标签与图例计数，不影响仪表颜色的连续渐变。 */
export type QuotaRisk = "critical" | "warning" | "healthy";

export const QUOTA_RISK_CRITICAL_PCT = 20;
export const QUOTA_RISK_WARNING_PCT = 50;

export function riskOf(remainingPct: number): QuotaRisk {
  if (remainingPct <= QUOTA_RISK_CRITICAL_PCT) return "critical";
  if (remainingPct <= QUOTA_RISK_WARNING_PCT) return "warning";
  return "healthy";
}

/** 驾驶舱控制台的汇总口径：全部当前窗口（系列 × 窗口）。
 *  平均剩余 = 全部窗口剩余百分比的算术平均（不是按 token 加权）。 */
export interface CockpitSummary {
  /** 全部当前行（剩余升序，最紧的在前） */
  rows: CockpitCurrentRow[];
  windowCount: number;
  seriesCount: number;
  sourceCount: number;
  averageRemaining: number | null;
  worstRow: CockpitCurrentRow | null;
  latestSampleAt: number | null;
  riskCounts: Record<QuotaRisk, number>;
  resets: CockpitResetItem[];
  nextReset: CockpitResetItem | null;
}

export function summarizeCockpit(
  series: CockpitSeries[],
  nowMs: number
): CockpitSummary {
  const rows = series
    .flatMap(currentRowsOf)
    .sort((a, b) => a.remainingPct - b.remainingPct);
  const riskCounts: Record<QuotaRisk, number> = {
    critical: 0,
    warning: 0,
    healthy: 0,
  };
  let sum = 0;
  let latestSampleAt: number | null = null;
  for (const row of rows) {
    sum += row.remainingPct;
    riskCounts[riskOf(row.remainingPct)] += 1;
    if (latestSampleAt == null || row.sampledAt > latestSampleAt) {
      latestSampleAt = row.sampledAt;
    }
  }
  const resets = resetTimeline(series, nowMs);
  return {
    rows,
    windowCount: rows.length,
    seriesCount: series.length,
    sourceCount: new Set(series.map((s) => s.source)).size,
    averageRemaining: rows.length > 0 ? sum / rows.length : null,
    worstRow: rows[0] ?? null,
    latestSampleAt,
    riskCounts,
    resets,
    nextReset: resets[0] ?? null,
  };
}

/** 仪表阵列排序：最紧的系列在前（同样紧时按来源展示序、再按展示名），
 *  让用户先看到需要处理的额度。 */
export function sortSeriesByRisk(series: CockpitSeries[]): CockpitSeries[] {
  return [...series].sort((a, b) => {
    const worstA = worstRemainingOf(a) ?? 101;
    const worstB = worstRemainingOf(b) ?? 101;
    if (worstA !== worstB) return worstA - worstB;
    const sourceOrder =
      QUOTA_COCKPIT_SOURCES.indexOf(a.source) -
      QUOTA_COCKPIT_SOURCES.indexOf(b.source);
    if (sourceOrder !== 0) return sourceOrder;
    return a.label.localeCompare(b.label);
  });
}

// ===== 重置时间线（区块③）=====

/** 一条重置窗口项：最新采样带未来 resetAt 的窗口 */
export interface CockpitResetItem {
  series: CockpitSeries;
  windowKey: string;
  resetAt: number;
  remainingPct: number;
}

/** 收集全部系列当前窗口里 resetAt 在未来的项，按重置时间升序（近期在前）。
 *  resetAt 已过期（快照停在旧周期）的窗口不收——它们不是待重置窗口。 */
export function resetTimeline(
  series: CockpitSeries[],
  nowMs: number
): CockpitResetItem[] {
  const items: CockpitResetItem[] = [];
  for (const item of series) {
    for (const row of currentRowsOf(item)) {
      if (row.resetAt != null && row.resetAt > nowMs) {
        items.push({
          series: item,
          windowKey: row.windowKey,
          resetAt: row.resetAt,
          remainingPct: row.remainingPct,
        });
      }
    }
  }
  items.sort((a, b) => a.resetAt - b.resetAt);
  return items;
}

// ===== 历史趋势（区块②）=====

/** 趋势图的剩余百分比采样点（口径换算在此完成，视图层不再碰已用值） */
export interface RemainingPoint {
  ts: number;
  /** 剩余 %（0-100） */
  remaining: number;
}

/** 某系列某窗口的 90 天剩余曲线数据 */
export function remainingPointsOf(
  series: CockpitSeries,
  windowKey: string
): RemainingPoint[] {
  const win = series.windows.find((w) => w.key === windowKey);
  if (!win) return [];
  return win.samples.map((s) => ({ ts: s.ts, remaining: 100 - s.usedPct }));
}

/** 绘图用降采样：等间隔保留 limit 个点（含真实首尾），避免 90 天全量
 *  采样生成超长 SVG path（仪表卡迷你曲线 / 趋势折线共用）。
 *  只影响绘制密度，统计数字仍由调用方用原序列计算。 */
export function decimateRemainingPoints(
  points: RemainingPoint[],
  limit: number
): RemainingPoint[] {
  if (limit < 2 || points.length <= limit) return points;
  const out: RemainingPoint[] = [];
  const step = (points.length - 1) / (limit - 1);
  for (let index = 0; index < limit; index += 1) {
    out.push(points[Math.round(index * step)]);
  }
  return out;
}
