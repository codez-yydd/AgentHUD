/**
 * 用量总览页的派生视图：把取数结果（OverviewRaw）按当前「数据范围」
 * （本机 / 全部设备）合并，再按时间范围切片成指标卡 / 趋势图 / 热力图 /
 * Agent 表要用的结构。全部是纯函数，页面在范围、数据范围或语言变化时重算
 * （预设档 O(90 × agent 数)，自定义区间最多 O(365 × agent 数)，均不再取数）。
 *
 * 数据范围语义（与菜单栏标题对齐）：local = 仅本机；all = 本机 + 其他设备
 * （同步开启时；未启用 / 远端失败时退化为仅本机，远端贡献为 0）。
 */
import type { Currency, PricingConfig } from "../../types";
import { AGENT_COLOR } from "../../agentVisibility";
import { modelCost } from "../../merge";
import {
  canonicalModelId,
  displayModelName,
  type MergedModelVariant,
} from "../../modelName";
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
  type DayModelAgg,
  type HeatMonthSpan,
  type OverviewAgentFilter,
  type OverviewRange,
  type OverviewScope,
  type UsageAgentId,
} from "./model";
import type {
  CursorModelAgg,
  OverviewRaw,
  QuotaInfo,
} from "./useOverviewData";

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
  /** Cursor 按「日 + 模型」聚合（仅本机口径：Cursor 明细不上传，不随数据范围变化） */
  cursorByModelByDay: Map<string, Map<string, CursorModelAgg>>;
  quotas: Map<UsageAgentId, QuotaInfo>;
  fxRate: number;
  /** 本机价格表（模型成本折算用；null = 读不到，成本按 0） */
  pricing: PricingConfig | null;
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
      cursorByModelByDay: raw.cursorByModelByDay,
      quotas: raw.quotas,
      fxRate: raw.fxRate,
      pricing: raw.pricing,
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
    cursorByModelByDay: raw.cursorByModelByDay,
    quotas: raw.quotas,
    fxRate: raw.fxRate,
    pricing: raw.pricing,
    includeRemote: true,
  };
}

// ===== Agent 筛选视图切片 =====

/**
 * 把范围视图收窄到单个 Agent（纯视图切片，不重新取数、不拷贝日聚合——
 * merged / perAgent 直接引用该 Agent 已合并好的 days）。
 *
 * 跟随筛选的区块：merged（指标卡 / 趋势图 / 热力图的数据源）与 perAgent
 * （模型用量表按 Agent 取 byModel 的数据源）；dayKeys / windowStartMs /
 * quotas / fxRate / pricing / includeRemote 原样保留（热力图窗口、成本折算
 * 等口径不随筛选变化）。Agent 统计表**不跟随**——它是跨 Agent 的分解对比
 * 表，是否切片由调用方决定（OverviewPage 里 Agent 表仍取范围视图的结果）。
 *
 * cursorByModelByDay 特判：Cursor 的按模型聚合是独立旁路（不随 Agent 日聚合
 * 走），仅筛选 cursor 时保留原表，其余 Agent 置空 Map——否则模型用量表会把
 * Cursor 用量混进其他 Agent 的行。
 *
 * 找不到该 Agent（当前数据范围下无数据，如仅本机用过的 Agent）时：merged
 * 用空 Map、perAgent 空数组，dayKeys 仍保留——热力图照常铺满 52 周空格子、
 * 指标卡 / 趋势 / 模型表如实显示 0，而不是误入整页空态。
 */
export function agentView(
  view: OverviewView,
  agent: OverviewAgentFilter
): OverviewView {
  if (agent === "all") return view;
  const hit = view.perAgent.find((entry) => entry.id === agent);
  return {
    ...view,
    merged: hit ? hit.days : new Map<string, DayAgg>(),
    perAgent: hit ? [hit] : [],
    cursorByModelByDay:
      agent === "cursor"
        ? view.cursorByModelByDay
        : new Map<string, Map<string, CursorModelAgg>>(),
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

/**
 * 模型用量表的折叠键（**仅本页模型表使用**）：
 * canonicalModelId(displayModelName(model_id)) —— 在通用身份归一之上再按
 * 「展示名」折叠，把带 provider 前缀的完整 id 与裸模型名视为同一个模型
 * （如 zhipuai-coding-plan/glm-5.3-flash 与 glm-5.3-flash）。
 *
 * 为什么本表做 display 级折叠：本表是**跨来源合并视图**——把各 Agent 的用量
 * 按用户认知的模型名汇总成一行，两行显示名完全相同时分成两行只会让用户困惑
 * （用户反馈的正是这种情形）。报告页的 foldModelStatRows 刻意不做这一层：
 * 它按来源分行展示，同名模型在不同来源本应各占一行，口径不同不冲突。
 *
 * 属于**展示层折叠**：不改写、不写回任何数据，也不影响计费（modelCost 仍按各
 * 写法的原始 id 分别折算，见 buildModelRows）、价格配置页、同步与模型徽标的
 * 身份判定（那些场景仍用完整 id + canonicalModelId / lookupPrice 的既有口径，
 * 见 modelName.ts）。点号同样不参与身份判定（glm-5.3 ≠ glm-5-3——点号归一
 * 只用于价格查找域）。
 */
function modelRowKey(modelId: string): string {
  return canonicalModelId(displayModelName(modelId));
}

/** 模型用量表行（按范围内总 token 降序） */
export interface ModelRow {
  /** 模型折叠键（modelRowKey：展示名级身份，见其注释）：React key 用——
   *  rep 会随数据变化，折叠键更稳定；「未拆分」占位行为空串 */
  key: string;
  /** 展示用原始写法（组内累计 total 最大的写法，与 foldModelStatRows 的 rep
   *  策略一致，保留用户熟悉的形态；渲染标签再经 displayModelName 去前缀）。
   *  「未拆分」占位行为空串。 */
  modelId: string;
  /** 组内被折叠的原始写法与请求数（仅多于一个写法时给出，模型名 tooltip 用；
   *  同一写法多次出现已按写法累加请求数；带 provider 前缀的写法同样列出，
   *  用户可据此看出该行是折叠结果） */
  variants?: MergedModelVariant[];
  /** 是否「未拆分」占位行：by_model 缺失的旧数据 / 无逐模型拆分的来源汇总 */
  unsplit: boolean;
  requests: number;
  /** 输入侧 token（= 总 token − 输出，含缓存读与缓存写，与指标卡「输入侧」
   *  同口径；未拆分行整份计入输入侧） */
  input: number;
  output: number;
  total: number;
  costUsd: number;
  costCny: number;
  /** 占比（0-1，分母 = 范围内全部行 total 之和，含未拆分行） */
  share: number;
}

/** 单个原始写法的累计（组内 rep 与 variants 都从这里派生，成本也按写法折算） */
interface SpellingAgg {
  /** 累计 total token（rep 选它最大者） */
  total: number;
  requests: number;
  input: number;
  output: number;
  cacheRead: number;
  /** 该写法来自 Cursor 的贡献（三段 token 与后端成本）：折算价格表时要扣除，
   *  否则会连同后端成本一起被价格表再折算一次（双计）；无贡献为 null */
  cursor: {
    input: number;
    output: number;
    cacheRead: number;
    costUsd: number;
  } | null;
}

/**
 * 模型用量表行：按当前时间范围切片各 Agent 日聚合的 byModel，跨 Agent 按
 * **模型折叠键**（modelRowKey：展示名级身份）合并，按总 token 降序。
 *
 * 身份折叠：
 *  - 聚合键 = modelRowKey(model_id) = canonicalModelId(displayModelName(model_id))：
 *    既折叠「GLM-5.3 / glm-5.3」这类只在字节层面有差异的写法（canonical 域，
 *    与 foldModelStatRows / 价格查找同域），也折叠显示名相同的带前缀写法与
 *    裸名（如 zhipuai-coding-plan/glm-5.3-flash 与 glm-5.3-flash）——本表是
 *    跨来源合并视图，理由见 modelRowKey 注释；
 *  - 展示名 rep = 组内累计 total 最大的原始写法（并列取先出现），与
 *    foldModelStatRows 的 rep 策略一致；界面渲染再由 displayModelName 去前缀
 *    （组内各写法的 display 名相同，显示 rep 的 display 名即整组的名字）；
 *  - 点号不参与身份判定（glm-5.3 ≠ glm-5-3，点号归一只用于价格查找域）；
 *  - 组内多于一个写法时输出 variants（原始写法 + 请求数）供模型名 tooltip
 *    说明「这一行是折叠结果」（含带前缀的写法）。
 *
 * 口径与数据来源：
 *  - 本机与远端（「全部设备」时已在 scopeView 合并进同一份 DayAgg）逐模型相加；
 *  - Cursor 的 events 由后端按「日 + 模型」聚合（cursorByModelByDay），这里
 *    随所选范围逐日并入；今日没有 Cursor 用量时不会带出历史模型；
 *  - 成本**按写法分别折算**（而非按组一次折算）：组成本 = Σ(非 Cursor 写法)
 *    modelCost(该写法的原始 id, 该写法三段 token) + Σ(Cursor 写法) 该写法的
 *    后端 cost_usd。按写法折算既让每个写法都能命中自己的价格条目
 *    （lookupPrice 对带前缀 id 有去前缀候选兜底），也天然消除「Cursor 贡献要
 *    从组总量里扣除」的需求（Cursor 的 token 根本不进 modelCost 基数），
 *    与「Cursor 用后端成本、不重复按价格表折算」的口径一致；
 *    价格表读不到时非 Cursor 写法成本按 0（显示「未定价」）；
 *  - 「未拆分」行：by_model 整日缺失的用量（旧数据 / 无拆分来源）汇总成一行，
 *    token 全部计入输入侧（与趋势图 splitNote 同口径），不参与成本折算。
 *    Cursor 的 daily 同样整日计入 unsplit，但其模型数据走 cursorByModelByDay
 *    旁路，这里按 Agent 排除，避免同一份用量计两次。
 *  - 占比分母 = 全部行（含未拆分行）的 total 之和：与指标卡的「范围内 Token」
 *    对齐。
 */
export function buildModelRows(
  raw: OverviewView,
  range: OverviewRange,
  custom: CustomRangeValue | null = null
): ModelRow[] {
  const keys = rangeKeys(raw, range, custom);
  /** 一个模型折叠键（modelRowKey）的聚合组 */
  interface ModelGroup {
    costUsd: number;
    agg: DayModelAgg;
    /** 原始写法 → 该写法的累计（rep / variants / 成本都从这里派生） */
    spellings: Map<string, SpellingAgg>;
    /** 组内累计 total 最大的原始写法（并列取先出现） */
    rep: string;
  }
  const groups = new Map<string, ModelGroup>();
  /** 未拆分行（requests 只在整日未拆分时可归属，见下） */
  let unsplitTotal = 0;
  let unsplitRequests = 0;

  /** 把一条模型聚合并入其折叠组（cursorCostUsd 非空时同时登记该写法的
   *  Cursor 贡献，供成本段扣除） */
  const absorb = (
    modelId: string,
    src: DayModelAgg,
    cursorCostUsd: number | null = null,
    billingSource = "zcode"
  ): ModelGroup => {
    const key = modelRowKey(modelId);
    let group = groups.get(key);
    if (!group) {
      group = { costUsd: 0, agg: { ...src }, spellings: new Map(), rep: modelId };
      groups.set(key, group);
    } else {
      group.agg.requests += src.requests;
      group.agg.input += src.input;
      group.agg.output += src.output;
      group.agg.cacheRead += src.cacheRead;
      group.agg.total += src.total;
    }
    const writes = billingSource === "claude" || billingSource === "kimi" ? Math.max(0, src.total - src.input - src.output - src.cacheRead) : 0;
    group.costUsd += cursorCostUsd ?? (raw.pricing ? modelCost(modelId, src.input, src.output, src.cacheRead, raw.pricing, "usd", raw.fxRate, billingSource, writes) : 0);
    const spelling = group.spellings.get(modelId) ?? {
      total: 0,
      requests: 0,
      input: 0,
      output: 0,
      cacheRead: 0,
      cursor: null,
    };
    spelling.total += src.total;
    spelling.requests += src.requests;
    spelling.input += src.input;
    spelling.output += src.output;
    spelling.cacheRead += src.cacheRead;
    if (cursorCostUsd != null) {
      const cursor =
        spelling.cursor ??
        (spelling.cursor = { input: 0, output: 0, cacheRead: 0, costUsd: 0 });
      cursor.input += src.input;
      cursor.output += src.output;
      cursor.cacheRead += src.cacheRead;
      cursor.costUsd += cursorCostUsd;
    }
    group.spellings.set(modelId, spelling);
    // rep 取组内累计 total 最大的原始写法（并列取先出现：只有严格大于才换）
    if (spelling.total > (group.spellings.get(group.rep)?.total ?? 0)) {
      group.rep = modelId;
    }
    return group;
  };

  for (const { id, days } of raw.perAgent) {
    const isCursor = id === "cursor";
    for (const key of keys) {
      const day = days.get(key);
      if (!day) continue;
      for (const [modelId, model] of day.byModel) absorb(modelId, model, null, id);
      // Cursor 的未拆分用量由 cursorByModelByDay 顶替（见函数注释），不在此累计
      if (isCursor || day.unsplit <= 0) continue;
      unsplitTotal += day.unsplit;
      // 整日都无 by_model 时（旧数据 / 无拆分来源）整日的请求数都属于未拆分行；
      // 部分缺失（本机 + 远端同日合并、远端缺 by_model）无法归属请求数，只计 token
      if (day.byModel.size === 0) unsplitRequests += day.requests;
    }
  }

  // Cursor：按所选范围的每个日键并入当日按模型聚合。Cursor 的 daily 整日
  // 计入 unsplit（无输入/输出拆分），模型数据来自独立的 cursorByModelByDay，
  // 逐日切片后不会把 90 天历史模型带进今日视图。
  for (const key of keys) {
    const byModel = raw.cursorByModelByDay.get(key);
    if (!byModel) continue;
    for (const [modelId, model] of byModel) {
      absorb(
        modelId,
        {
          requests: model.requests,
          input: model.input,
          output: model.output,
          cacheRead: model.cacheRead,
          total: model.total,
        },
        model.costUsd
      );
    }
  }

  const rows: ModelRow[] = [];
  for (const [key, group] of groups) {
    const model = group.agg;
    // 0 token 且 0 请求的模型不出行（异常 / 空模型名占位不进表）
    if (model.total <= 0 && model.requests <= 0) continue;
    // 组成本按写法分别折算：非 Cursor 写法用本机价格表（modelCost，与菜单栏
    // 标题 / 面板汇总页同款口径；用该写法自己的原始 id 调用，带前缀的写法由
    // lookupPrice 的去前缀候选兜底），扣掉该写法里的 Cursor 贡献后折算，
    // 再叠加 Cursor 写法的后端 cost_usd——两来源互不重复计价（见函数注释）。
    const costUsd = group.costUsd;
    // 折叠明细：同一写法多次出现已按写法累加（tooltip 读作「各写法的请求数」）；
    // 只有一个写法时与 foldModelStatRows 一致不携带（行没被折叠过）
    const variants = [...group.spellings].map(([model_id, spelling]) => ({
      model_id,
      requests: spelling.requests,
    }));
    rows.push({
      key,
      modelId: group.rep,
      variants: variants.length > 1 ? variants : undefined,
      unsplit: false,
      requests: model.requests,
      input: Math.max(0, model.total - model.output),
      output: model.output,
      total: model.total,
      costUsd,
      costCny: costUsd * raw.fxRate,
      share: 0,
    });
  }
  if (unsplitTotal > 0) {
    rows.push({
      key: "",
      modelId: "",
      unsplit: true,
      requests: unsplitRequests,
      // 无输入/输出拆分：整份计入输入侧（与趋势图 / 指标卡同口径）
      input: unsplitTotal,
      output: 0,
      total: unsplitTotal,
      costUsd: 0,
      costCny: 0,
      share: 0,
    });
  }

  const totalAll = rows.reduce((sum, row) => sum + row.total, 0);
  for (const row of rows) {
    row.share = totalAll > 0 ? row.total / totalAll : 0;
  }
  // 并列时按身份键定序（与 rep 写法无关，排序结果不随数据写法漂移）
  rows.sort((a, b) => b.total - a.total || a.key.localeCompare(b.key));
  return rows;
}
