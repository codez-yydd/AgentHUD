/**
 * 用量总览页的派生视图：把取数结果（OverviewRaw）按当前「数据范围」
 * （本机 / 全部设备）合并，再按时间范围切片成指标卡 / 趋势图 / 热力图 /
 * Agent 表要用的结构。全部是纯函数，页面在范围、数据范围或语言变化时重算
 * （预设档 O(90 × agent 数)，自定义区间最多 O(365 × agent 数)，均不再取数）。
 *
 * 数据范围语义（与菜单栏标题对齐）：local = 仅本机；all = 本机 + 其他设备
 * （同步开启时；未启用 / 远端失败时退化为仅本机，远端贡献为 0）。
 */
import type { Currency } from "../../types";
import { AGENT_COLOR } from "../../agentVisibility";
import type { CustomRangeValue } from "../CustomRangeMenu";
import {
  RANGE_DAYS,
  USAGE_AGENT_IDS,
  agentLabel,
  dayOf,
  inputSideOf,
  addDayInto,
  addLocalDays,
  emptyDay,
  localDaysBetween,
  mergeDayMaps,
  startOfLocalDay,
  sumDays,
  buildHeatColumns,
  buildMonthSpans,
  type DayAgg,
  type HeatMonthSpan,
  type OverviewRange,
  type OverviewScope,
  type UsageAgentId,
} from "./model";
import type { OverviewRaw, QuotaInfo } from "./useOverviewData";

/** 展示币种：与面板一致，币种从属语言（中文=人民币、英文=美元） */
export function currencyOf(locale: string): Currency {
  return locale === "en" ? "usd" : "cny";
}

/** 按展示币种取对应金额（成本两种货币后端都已算好） */
export function costOf(costUsd: number, costCny: number, currency: Currency): number {
  return currency === "cny" ? costCny : costUsd;
}

// ===== 数据范围视图（本机 / 全部设备）=====

/** 单 Agent 的范围视图：合并后的日聚合 + 远端单独贡献（供「含远端」标记） */
export interface ViewAgent {
  id: UsageAgentId;
  /** 本机 + 远端合并后的按日聚合（local 范围下即本机） */
  days: Map<string, DayAgg>;
  /** 远端单独贡献（无远端贡献为 null） */
  remoteDays: Map<string, DayAgg> | null;
}

/** 页面聚合视图：把方向性口径（范围 / 配额）与合并后的日聚合放在一起 */
export interface OverviewView {
  dayKeys: string[];
  windowStartMs: number;
  /** 跨 Agent 合并后的按日聚合（当前数据范围口径） */
  merged: Map<string, DayAgg>;
  /** 各 Agent 的合并日聚合（顺序同 USAGE_AGENT_IDS） */
  perAgent: ViewAgent[];
  quotas: Map<UsageAgentId, QuotaInfo>;
  fxRate: number;
  /** 当前是否含远端（Agent 表的「含远端」标记只在此为 true 时可能展示） */
  includeRemote: boolean;
}

/**
 * 组装当前数据范围下的聚合视图。
 *
 * - local（或远端无任何贡献）：直接复用本机结果，零拷贝；
 * - all：本机与远端逐日相加（每个 Agent 一份 + 跨 Agent 合并一份），
 *   远端单独贡献保留给 Agent 表的「含远端」标记；
 * - Agent 顺序固定按 USAGE_AGENT_IDS：本机某 Agent 取数失败但远端有数据时
 *   该行仍应出现（反之亦然）。
 */
export function scopeView(raw: OverviewRaw, scope: OverviewScope): OverviewView {
  const includeRemote = scope === "all" && raw.remotePerAgent.length > 0;
  if (!includeRemote) {
    return {
      dayKeys: raw.dayKeys,
      windowStartMs: raw.windowStartMs,
      merged: raw.merged,
      perAgent: raw.perAgent.map((entry) => ({
        id: entry.id,
        days: entry.days,
        remoteDays: null,
      })),
      quotas: raw.quotas,
      fxRate: raw.fxRate,
      includeRemote: false,
    };
  }

  const localOf = new Map<UsageAgentId, Map<string, DayAgg>>(
    raw.perAgent.map((entry) => [entry.id, entry.days])
  );
  const remoteOf = new Map<UsageAgentId, Map<string, DayAgg>>(
    raw.remotePerAgent.map((entry) => [entry.id, entry.days])
  );
  const perAgent: ViewAgent[] = [];
  for (const id of USAGE_AGENT_IDS) {
    const local = localOf.get(id);
    const remote = remoteOf.get(id);
    if (!local && !remote) continue;
    const days = mergeDayMaps(
      [local, remote].filter((map): map is Map<string, DayAgg> => map != null)
    );
    perAgent.push({ id, days, remoteDays: remote ?? null });
  }
  return {
    dayKeys: raw.dayKeys,
    windowStartMs: raw.windowStartMs,
    merged: mergeDayMaps(perAgent.map((entry) => entry.days)),
    perAgent,
    quotas: raw.quotas,
    fxRate: raw.fxRate,
    includeRemote: true,
  };
}

/**
 * 当前范围在取数窗口内的日键切片（升序，末位为今天）。
 *
 * - 预设档：取窗口末尾 N 天（RANGE_DAYS），口径与改动前一致；
 * - 自定义区间（custom 非空时优先，忽略 range）：按「与窗口末位（今天）相差的
 *   自然日数」定位下标后切片。**不能**改用「按日键（MM-DD）集合过滤」：MM-DD
 *   只在 365 天窗口内唯一，窗口起点之前的日期会与窗口内某天同月同日（如窗口
 *   起点前 30 天 = 窗口内第 335 天），过滤会误命中窗口外的同月同日。
 *   区间只与窗口部分相交时取交集；完全落在窗口外（早于窗口起点或晚于今天）
 *   返回空切片——页面已用 min/max 限制可选日期，这里只是诚实的防御。
 */
export function rangeKeys(
  raw: OverviewView,
  range: OverviewRange,
  custom: CustomRangeValue | null = null
): string[] {
  if (custom) {
    const total = raw.dayKeys.length;
    if (total === 0) return [];
    // 窗口末位（今天）的 0 点：由窗口起点 + 天数推出，不依赖切片时刻的 Date.now()
    const lastDayStart = addLocalDays(raw.windowStartMs, total - 1);
    const startIndex =
      total - 1 - localDaysBetween(startOfLocalDay(custom.fromMs), lastDayStart);
    const endIndex =
      total -
      1 -
      localDaysBetween(startOfLocalDay(custom.toMs - 1), lastDayStart);
    if (endIndex < 0 || startIndex > total - 1) return [];
    return raw.dayKeys.slice(
      Math.max(0, startIndex),
      Math.min(total - 1, endIndex) + 1
    );
  }
  return raw.dayKeys.slice(-RANGE_DAYS[range]);
}

/** 指标卡汇总（范围内 + 今日独立口径） */
export interface MetricSummary {
  /** 范围天数（日均用） */
  days: number;
  rangeTokens: number;
  rangeInput: number;
  rangeOutput: number;
  rangeRequests: number;
  rangeCostUsd: number;
  rangeCostCny: number;
  todayTokens: number;
  todayRequests: number;
  todayCostUsd: number;
  todayCostCny: number;
  /** 昨日 token（今日范围下第 4 卡做环比用；恒为窗口倒数第二天） */
  yesterdayTokens: number;
}

export function summarize(
  raw: OverviewView,
  range: OverviewRange,
  custom: CustomRangeValue | null = null
): MetricSummary {
  const keys = rangeKeys(raw, range, custom);
  const sum = sumDays(raw.merged, keys);
  // 今日恒为窗口最后一天（不随范围变化）
  const today = dayOf(raw.merged, raw.dayKeys[raw.dayKeys.length - 1]);
  // 昨日 = 窗口倒数第二天（窗口固定 90 天，越界时 dayOf 回落全零）
  const yesterday = dayOf(raw.merged, raw.dayKeys[raw.dayKeys.length - 2]);
  return {
    days: keys.length,
    rangeTokens: sum.total,
    rangeInput: inputSideOf(sum),
    rangeOutput: sum.output,
    rangeRequests: sum.requests,
    rangeCostUsd: sum.costUsd,
    rangeCostCny: sum.costCny,
    todayTokens: today.total,
    todayRequests: today.requests,
    todayCostUsd: today.costUsd,
    todayCostCny: today.costCny,
    yesterdayTokens: yesterday.total,
  };
}

/** 趋势图单日数据点（输入侧 / 输出堆叠） */
export interface TrendDay extends DayAgg {
  key: string;
  input: number;
}

export function trendSeries(
  raw: OverviewView,
  range: OverviewRange,
  custom: CustomRangeValue | null = null
): TrendDay[] {
  const keys = rangeKeys(raw, range, custom);
  return keys.map((key) => {
    const day = dayOf(raw.merged, key);
    return { key, ...day, input: inputSideOf(day) };
  });
}

/** 热力图单格 */
export interface HeatCell {
  key: string | null;
  day: DayAgg | null;
}

/** 热力图图例模式：每日 / 每周 / 累计（纯客户端聚合切换） */
export type HeatMode = "daily" | "weekly" | "cumulative";

/** 单周聚合（周模式与累计模式共用） */
export interface HeatWeek {
  /** 该周（列）内有效日期数：本周为已过天数，整周为 7（填充行数的上限） */
  days: number;
  /** 周合计 */
  total: number;
  requests: number;
  /** 截至本周（含）的累计 */
  cumTotal: number;
  cumRequests: number;
  /** 该列首个 / 末个有效日键（列 tooltip 标题用；无有效日期为 null） */
  startKey: string | null;
  endKey: string | null;
}

export interface HeatSeries {
  /** 52 列 × 7 行（周一…周日），空位为 null（未来日期 / 窗口外） */
  columns: HeatCell[][];
  /** 底部月份标签段（连续同月合并，最多 12 段） */
  monthSpans: HeatMonthSpan[];
  /** 日模式的色阶分母（窗口内单日峰值） */
  dayMax: number;
  /** 周模式的色阶分母（窗口内单周峰值） */
  weekMax: number;
  /** 累计模式的色阶分母（末周累计值） */
  cumMax: number;
  /** 各周聚合（与列一一对应） */
  weeks: HeatWeek[];
  /** 展示窗口（52 周）内的合计与活跃度 */
  totalTokens: number;
  activeDays: number;
  /** 活跃日日均 token（无活跃日为 0） */
  activeDailyAvg: number;
}

/**
 * 热力图序列：最近 52 个自然周（列 = 周、行 = 周一…周日），三种模式的色阶
 * 分母与周聚合一次算好；数据取自统一的 365 天按日聚合（缺失日期按 0）。
 */
export function heatSeries(raw: OverviewView): HeatSeries {
  const keySet = new Set(raw.dayKeys);
  const columns: HeatCell[][] = buildHeatColumns(Date.now(), keySet).map(
    (column) =>
      column.map((key) =>
        key ? { key, day: dayOf(raw.merged, key) } : { key: null, day: null }
      )
  );
  const monthSpans = buildMonthSpans(columns.map((column) => column.map((cell) => cell.key)));

  const weeks: HeatWeek[] = [];
  let dayMax = 0;
  let weekMax = 0;
  let totalTokens = 0;
  let activeDays = 0;
  let cumulative = 0;
  let cumRequests = 0;
  for (const column of columns) {
    let days = 0;
    let total = 0;
    let requests = 0;
    let startKey: string | null = null;
    let endKey: string | null = null;
    for (const cell of column) {
      if (!cell.key || !cell.day) continue;
      days += 1;
      startKey ??= cell.key;
      endKey = cell.key;
      const dayTotal = cell.day.total;
      totalTokens += dayTotal;
      if (dayTotal > 0) activeDays += 1;
      if (dayTotal > dayMax) dayMax = dayTotal;
      total += dayTotal;
      requests += cell.day.requests;
    }
    cumulative += total;
    cumRequests += requests;
    if (total > weekMax) weekMax = total;
    weeks.push({
      days,
      total,
      requests,
      cumTotal: cumulative,
      cumRequests,
      startKey,
      endKey,
    });
  }

  return {
    columns,
    monthSpans,
    dayMax,
    weekMax,
    cumMax: cumulative,
    weeks,
    totalTokens,
    activeDays,
    activeDailyAvg: activeDays > 0 ? totalTokens / activeDays : 0,
  };
}

/** Agent 统计表行 */
export interface AgentRow {
  id: UsageAgentId;
  label: string;
  color: string;
  todayTokens: number;
  rangeTokens: number;
  rangeRequests: number;
  rangeCostUsd: number;
  rangeCostCny: number;
  /** 范围内远端（其他设备）贡献的 token（> 0 时行内加「含远端」标记） */
  remoteRangeTokens: number;
  quota: QuotaInfo | null;
}

/**
 * Agent 表行：按范围内 token 降序。
 * 行列取自「当前所选范围（rangeKeys）内有 token 或 今日有 token 或 有额度
 * 快照」的 Agent——三者在当前范围口径下都为空说明该行所有列都是占位符
 * （如仅更早之前用过、当前范围没用且无额度），不占一行（避免空白噪音）；
 * 无额度快照的行额度列显示 "--"。保留条件随所选时间范围变化，与列内展示的
 * 今日 / 范围内 / 额度三个来源一一对应。
 * remoteRangeTokens 只在「全部设备」且该行确有远端贡献时大于 0。
 */
export function buildAgentRows(
  raw: OverviewView,
  range: OverviewRange,
  custom: CustomRangeValue | null = null
): AgentRow[] {
  const keys = rangeKeys(raw, range, custom);
  const rows: AgentRow[] = [];
  for (const { id, days, remoteDays } of raw.perAgent) {
    const rangeSum = sumDays(days, keys);
    const todayKey = raw.dayKeys[raw.dayKeys.length - 1];
    const todayTokens = todayKey ? days.get(todayKey)?.total ?? 0 : 0;
    const quota = raw.quotas.get(id) ?? null;
    // 保留条件：范围内有 token、今日有 token 或有额度快照；都空则整行都是
    // "—"，不出行
    if (rangeSum.total <= 0 && todayTokens <= 0 && !quota) continue;
    let remoteRangeTokens = 0;
    if (remoteDays) {
      const remoteSum = emptyDay();
      for (const key of keys) {
        const day = remoteDays.get(key);
        if (day) addDayInto(remoteSum, day);
      }
      remoteRangeTokens = remoteSum.total;
    }
    rows.push({
      id,
      label: agentLabel(id),
      color: AGENT_COLOR[id],
      todayTokens,
      rangeTokens: rangeSum.total,
      rangeRequests: rangeSum.requests,
      rangeCostUsd: rangeSum.costUsd,
      rangeCostCny: rangeSum.costCny,
      remoteRangeTokens,
      quota,
    });
  }
  rows.sort(
    (a, b) =>
      b.rangeTokens - a.rangeTokens ||
      USAGE_AGENT_IDS.indexOf(a.id) - USAGE_AGENT_IDS.indexOf(b.id)
  );
  return rows;
}
