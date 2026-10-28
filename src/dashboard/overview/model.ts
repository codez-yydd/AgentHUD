/**
 * 用量总览页的纯数据模型与聚合工具（无 React / 无 IPC，便于单测与复用）。
 *
 * 统一按「本地自然日」聚合：后端 get_trend 与各 Agent 的 usage 命令都下发
 * day 桶，标签由 Rust 侧 db::bucket_label 生成 "%m-%d"（本地时区，90 天
 * 窗口内唯一），因此可以直接用 "MM-DD" 跨 Agent 对齐到同一天——指标卡、
 * 趋势图、热力图、Agent 表全部由这一份按日聚合派生，不再解析时间戳。
 *
 * 远端（其他设备）用量单独走 hour 桶再归到本地自然日（见 addRemoteTrendDays）：
 * 服务端 day 桶按 UTC 零点对齐，非 UTC 时区会与本地自然日错位，用小时桶
 * 才能与菜单栏标题的「本地自然日」口径对齐。
 *
 * 除总量与输入/输出外，各来源的 by_model 也逐日并入 DayAgg.byModel（模型
 * 用量表的数据源，成本在派生层用本机价格表统一折算）；Cursor 的按模型聚合
 * 是「请求范围整段」、无按日拆分，走 useOverviewData 的 cursorByModel 旁路。
 */
import type {
  CursorDailyEntry,
  PricingConfig,
  RemoteUsage,
  TrendPoint,
} from "../../types";
import { modelCost } from "../../merge";
import { AGENT_VISIBILITY_OPTIONS } from "../../agentVisibility";

/** 支持用量统计的 Agent（顺序即默认展示顺序）。
 *  这 8 个 agent id 同时是 BrandIcon 品牌名与 AGENT_COLOR / AGENT_GROUP 的
 *  键，展示时无需再做域映射；opencodego / minimax 的后端来源名异名
 *  （opencode / minimaxcode）只在需要传 source 的接口上才关心。 */
export const USAGE_AGENT_IDS = [
  "zai",
  "codex",
  "claude",
  "cursor",
  "kimi",
  "dsh",
  "opencodego",
  "minimax",
] as const;

export type UsageAgentId = (typeof USAGE_AGENT_IDS)[number];

/** Agent id → 品牌展示名：取设置页「统计展示来源」的同一份元数据
 *  （品牌名不进词典，与面板各处显示一致），未登记时回退 id 本身。 */
export function agentLabel(id: UsageAgentId): string {
  return AGENT_VISIBILITY_OPTIONS.find((option) => option.id === id)?.label ?? id;
}

/** 页头时间范围预设档（自定义区间不在此列：由 [fromMs, toMs) 直接决定，
 *  见 derive.rangeKeys 与 CustomRangeMenu） */
export type OverviewRange = "today" | "7d" | "30d" | "90d";

/** 页头时间范围选择：预设档或自定义区间（"custom" 只在选中自定义档时出现，
 *  供卡片判断「范围内是否等于今日」这类档位语义） */
export type OverviewRangeChoice = OverviewRange | "custom";

/** 各预设覆盖的自然日数（含今天；趋势图与指标卡按此在取数窗口内切片） */
export const RANGE_DAYS: Record<OverviewRange, number> = {
  today: 1,
  "7d": 7,
  "30d": 30,
  "90d": 90,
};

/** 页面统一取数窗口（天）：本地 SQLite 型来源一次取满一年，指标卡 / 趋势图 /
 *  Agent 表按 USAGE_WINDOW_DAYS 从这个窗口里切片（口径不变，避免重复请求），
 *  热力图则铺满最近 52 个自然周。「切换范围」仍然只重新聚合、不重新取数。 */
export const WINDOW_DAYS = 365;

/** 指标卡 / 趋势图 / Agent 表的使用窗口（天）：口径与改动前一致（近 90 天），
 *  数据从 365 天取数结果里切片。 */
export const USAGE_WINDOW_DAYS = 90;

/** Cursor 与远端（其他设备）合并的取数窗口（天）：cursor events 全年分页
 *  太慢、远端 hour 桶 × 全年体积太大，维持 90 天。90 天之前的格子 = 纯本机
 *  口径（不含 Cursor、不含其他设备）。 */
export const REMOTE_WINDOW_DAYS = 90;

/** 热力图显示的自然周数（52 周 ≈ 一年，与 365 天取数窗口同量级；
 *  52 周最多覆盖 364 天，恒落在取数窗口内）。 */
export const HEAT_WEEKS = 52;

/** 底部月份标签行的最大标签数（52 周可能跨 13 个月，超出时合并最短的段） */
export const HEAT_MONTH_LABEL_MAX = 12;

/** 额度快照回看天数：额度剩余只反映「当前」状态，固定取近 8 天的最新采样，
 *  避免随页面范围把 90 天历史全量读进来（get_quota_history 是文件全量读）。 */
export const QUOTA_LOOKBACK_DAYS = 8;

/** 单模型日聚合（DayAgg.byModel 的值，模型用量表用）。
 *  input/output/cacheRead 与后端 by_model 契约一致（input 不含缓存读），
 *  旧数据 / 远端旧服务缺省时按 0 处理；成本刻意不放这一层——本机与远端
 *  来源的模型成本统一在派生层用同一份价格表折算（单一口径，见
 *  derive.buildModelRows），避免「按日成本」与「按模型成本」两个口径并存。 */
export interface DayModelAgg {
  requests: number;
  /** 输入 token（不含缓存读；缓存读单列，供 modelCost 扣减） */
  input: number;
  output: number;
  cacheRead: number;
  total: number;
}

/** 单日聚合（跨 Agent 合并或单 Agent 各自一份） */
export interface DayAgg {
  /** 总 token（后端 computed_total_tokens 口径） */
  total: number;
  /** 已知输出 token 合计；无逐日拆分的来源（Cursor daily / 旧数据）计入 unsplit */
  output: number;
  requests: number;
  costUsd: number;
  costCny: number;
  /** 缺少输入/输出拆分的 token：渲染时并入「输入侧」并在 tooltip 标注 ≈ */
  unsplit: number;
  /** 按模型拆分（key = model_id）：模型用量表用。by_model 缺失的来源
   *  （Cursor daily / 旧数据）为空表，其用量只体现在 total / unsplit 上 */
  byModel: Map<string, DayModelAgg>;
}

export function emptyModelAgg(): DayModelAgg {
  return { requests: 0, input: 0, output: 0, cacheRead: 0, total: 0 };
}

/** 把 src 模型聚合累加进 target（原地修改） */
export function addModelInto(target: DayModelAgg, src: DayModelAgg): void {
  target.requests += src.requests;
  target.input += src.input;
  target.output += src.output;
  target.cacheRead += src.cacheRead;
  target.total += src.total;
}

/** 深拷贝一张「模型 → 聚合」表（合并结果与来源解耦：合并后的表可能继续被
 *  addDayInto 原地累加，浅拷贝会让来源表跟着变） */
function cloneModelMap(
  map: Map<string, DayModelAgg>
): Map<string, DayModelAgg> {
  const out = new Map<string, DayModelAgg>();
  for (const [modelId, agg] of map) out.set(modelId, { ...agg });
  return out;
}

/** 深拷贝单日聚合（含 byModel 表） */
export function cloneDay(day: DayAgg): DayAgg {
  return { ...day, byModel: cloneModelMap(day.byModel) };
}

/** 输入侧 token = 总量 - 输出（含缓存读，与项目「输入侧」口径一致） */
export function inputSideOf(day: DayAgg): number {
  return Math.max(0, day.total - day.output);
}

export function emptyDay(): DayAgg {
  return {
    total: 0,
    output: 0,
    requests: 0,
    costUsd: 0,
    costCny: 0,
    unsplit: 0,
    byModel: new Map(),
  };
}

/** 把 src 累加进 target（跨 Agent 合并用，原地修改） */
export function addDayInto(target: DayAgg, src: DayAgg): void {
  target.total += src.total;
  target.output += src.output;
  target.requests += src.requests;
  target.costUsd += src.costUsd;
  target.costCny += src.costCny;
  target.unsplit += src.unsplit;
  for (const [modelId, agg] of src.byModel) {
    const hit = target.byModel.get(modelId);
    if (hit) addModelInto(hit, agg);
    else target.byModel.set(modelId, { ...agg });
  }
}

/** 多个「日期 → 日聚合」表合并成一张（同键逐字段求和，byModel 逐模型求和） */
export function mergeDayMaps(
  maps: Iterable<Map<string, DayAgg>>
): Map<string, DayAgg> {
  const out = new Map<string, DayAgg>();
  for (const map of maps) {
    for (const [key, day] of map) {
      const target = out.get(key);
      if (target) addDayInto(target, day);
      else out.set(key, cloneDay(day));
    }
  }
  return out;
}

/** 取某日聚合（键缺失/为空返回全零，避免各处重复做空值兜底） */
export function dayOf(map: Map<string, DayAgg>, key: string | undefined): DayAgg {
  return (key ? map.get(key) : undefined) ?? emptyDay();
}

/** 按日键列表求和（缺失日期按 0），用于指标卡与 Agent 表。 */
export function sumDays(map: Map<string, DayAgg>, keys: string[]): DayAgg {
  const out = emptyDay();
  for (const key of keys) {
    const day = map.get(key);
    if (day) addDayInto(out, day);
  }
  return out;
}

/** 后端 day 桶标签（"MM-DD"）校验：hour 桶、远端 ISO 标签、脏值一律跳过 */
const DAY_LABEL_RE = /^\d{2}-\d{2}$/;

/**
 * TrendPoint[]（day 桶）→ 按日聚合，写入 into。
 * by_model 缺失（旧数据）时整桶计入 unsplit，避免把未知拆分伪装成「全是输入」。
 * by_model 存在时逐模型并入 byModel（模型用量表的数据源；缺 token 字段的
 * 旧服务按 0 处理）。
 */
export function addTrendDays(
  trend: TrendPoint[],
  into: Map<string, DayAgg>
): void {
  for (const point of trend) {
    if (!DAY_LABEL_RE.test(point.label)) continue;
    const models = point.by_model ?? [];
    const output = models.reduce((sum, m) => sum + (m.output_tokens ?? 0), 0);
    const day = into.get(point.label) ?? emptyDay();
    day.total += point.total_tokens;
    day.output += output;
    day.requests += point.requests;
    day.costUsd += point.cost_usd;
    day.costCny += point.cost_cny;
    for (const m of models) {
      const agg = day.byModel.get(m.model_id) ?? emptyModelAgg();
      agg.requests += m.requests;
      agg.input += m.input_tokens ?? 0;
      agg.output += m.output_tokens ?? 0;
      agg.cacheRead += m.cache_read_tokens ?? 0;
      agg.total += m.total_tokens;
      day.byModel.set(m.model_id, agg);
    }
    if (models.length === 0 && point.total_tokens > 0) {
      day.unsplit += point.total_tokens;
    }
    into.set(point.label, day);
  }
}

/**
 * Cursor 每日明细 → 按日聚合。daily 无输入/输出拆分（只有总量），
 * 整日 token 计入 unsplit；人民币成本按本机汇率折算（与面板 Cursor 页同款）。
 * daily 也没有模型拆分：byModel 留空，模型用量表走 cursorByModel 旁路
 * （Cursor events 的按模型聚合是「请求范围整段」，无按日拆分）。
 */
export function addCursorDays(
  daily: CursorDailyEntry[],
  fxRate: number,
  into: Map<string, DayAgg>
): void {
  for (const entry of daily) {
    if (!DAY_LABEL_RE.test(entry.date)) continue;
    const day = into.get(entry.date) ?? emptyDay();
    day.total += entry.total_tokens;
    day.requests += entry.requests;
    day.costUsd += entry.cost_usd;
    day.costCny += entry.cost_usd * fxRate;
    day.unsplit += entry.total_tokens;
    into.set(entry.date, day);
  }
}

// ============================================================
// 数据范围（本机 / 全部设备）
// ============================================================

/** 数据范围：local = 仅本机；all = 本机 + 其他设备（与菜单栏标题同口径） */
export type OverviewScope = "local" | "all";

/** 范围偏好持久化键（损坏值回退默认 all，与菜单栏标题口径对齐） */
const SCOPE_KEY = "zbar-dashboard-scope";

/** 读取范围偏好：无值 / 损坏值一律回退 all（默认与托盘同口径） */
export function loadOverviewScope(): OverviewScope {
  try {
    const saved = localStorage.getItem(SCOPE_KEY);
    if (saved === "local" || saved === "all") return saved;
  } catch {
    /* 存储不可用：回退默认范围 */
  }
  return "all";
}

/** 写入范围偏好（失败静默：记忆仅锦上添花，不影响取数） */
export function saveOverviewScope(scope: OverviewScope): void {
  try {
    localStorage.setItem(SCOPE_KEY, scope);
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等 */
  }
}

/** Agent 筛选：all = 全部 Agent 合并口径（默认）；否则为单个 agent id */
export type OverviewAgentFilter = "all" | UsageAgentId;

/** Agent 筛选偏好持久化键（损坏 / 未知值回退默认 all，与范围偏好同一套范式） */
const AGENT_FILTER_KEY = "zbar-dashboard-agent";

/** 读取 Agent 筛选偏好：无值 / 损坏值 / 已不再支持的 agent id 一律回退 all */
export function loadOverviewAgentFilter(): OverviewAgentFilter {
  try {
    const saved = localStorage.getItem(AGENT_FILTER_KEY);
    if (
      saved === "all" ||
      (saved != null && (USAGE_AGENT_IDS as readonly string[]).includes(saved))
    ) {
      return saved as OverviewAgentFilter;
    }
  } catch {
    /* 存储不可用：回退默认 */
  }
  return "all";
}

/** 写入 Agent 筛选偏好（失败静默：与范围偏好同款，筛选仅影响展示切片） */
export function saveOverviewAgentFilter(agent: OverviewAgentFilter): void {
  try {
    localStorage.setItem(AGENT_FILTER_KEY, agent);
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等 */
  }
}

// ============================================================
// 远端（其他设备）用量：hour 桶 → 本地自然日
// ============================================================

/** 远端桶 label → 桶起始毫秒。服务端下发「桶起始 ms」字符串
 *  （server/db.py 里 label = str(start)），ISO 串作为兜底格式解析。
 *  与对比页 compare/model.ts 的 bucketStartOf 同款口径，此处独立小实现，
 *  避免总览页为一个 5 行解析函数依赖整个对比页模型。 */
function remoteBucketStart(label: string): number | null {
  const numeric = Number(label);
  if (label.trim() !== "" && Number.isFinite(numeric)) {
    // 兼容旧服务端的秒级时间戳（当前下发毫秒）
    return numeric < 1_000_000_000_000 ? numeric * 1000 : numeric;
  }
  const parsed = Date.parse(label);
  return Number.isFinite(parsed) ? parsed : null;
}

/**
 * 远端趋势（hour 桶）→ 按本地自然日聚合，写入 into（「全部设备」合并用）。
 *
 * 为什么用 hour 而不是 day 桶：服务端 day 桶按 UTC 零点对齐，非 UTC 时区下
 * 每个桶跨越两个本地自然日，「今日」会取到前一天的数据，无法与菜单栏标题
 * （本地自然日 0 点起）对齐；hour 桶按 UTC 整点对齐，整点时区与本地整点
 * 完全一致，按桶起点的本地日期归日即与本地 day 桶严格对齐（非整点时区
 * 仅窗口边界有 ≤1 桶偏差，总量仍精确）。
 *
 * 成本：远端不下发成本，用本机价格表按模型逐条折算（modelCost，与菜单栏
 * 标题 / 面板汇总页同款口径）；价格表读不到时远端成本按 0，token 照常统计。
 * 落在窗口日键之外的桶（非整点时区窗口起点之前的一桶）直接跳过，避免窗口
 * 外的用量把 Agent 误标成「有用量」。
 *
 * by_model 存在时逐模型并入 byModel（模型用量表的数据源，成本同样在派生层
 * 用本机价格表折算，与这里的按日成本同源），缺失时整桶计入 unsplit。
 */
export function addRemoteTrendDays(
  remote: RemoteUsage,
  pricing: PricingConfig | null,
  fxRate: number,
  windowKeys: Set<string>,
  into: Map<string, DayAgg>
): void {
  for (const bucket of remote.trend) {
    const start = remoteBucketStart(bucket.label);
    if (start == null) continue;
    const key = dayKeyOf(start);
    if (!windowKeys.has(key)) continue;
    const day = into.get(key) ?? emptyDay();
    let output = 0;
    let costUsd = 0;
    for (const m of bucket.by_model) {
      output += m.output_tokens ?? 0;
      if (pricing) {
        costUsd += modelCost(
          m.model_id,
          m.input_tokens ?? 0,
          m.output_tokens ?? 0,
          m.cache_read_tokens ?? 0,
          pricing,
          "usd",
          fxRate, m.source, m.cache_write_tokens ?? ((m.source === "claude" || m.source === "kimi") ? Math.max(0, m.total_tokens - m.input_tokens - m.output_tokens - m.cache_read_tokens) : 0)
        );
      }
      const agg = day.byModel.get(m.model_id) ?? emptyModelAgg();
      agg.requests += m.requests;
      agg.input += m.input_tokens ?? 0;
      agg.output += m.output_tokens ?? 0;
      agg.cacheRead += m.cache_read_tokens ?? 0;
      agg.total += m.total_tokens;
      day.byModel.set(m.model_id, agg);
    }
    day.total += bucket.total_tokens;
    day.output += output;
    day.requests += bucket.requests;
    day.costUsd += costUsd;
    day.costCny += costUsd * fxRate;
    // by_model 缺失（异常响应）时整桶计入输入侧，与本地 addTrendDays 同款兜底
    if (bucket.by_model.length === 0 && bucket.total_tokens > 0) {
      day.unsplit += bucket.total_tokens;
    }
    into.set(key, day);
  }
}

// ============================================================
// 本地日期工具（与 format.ts 的 dateStr 同口径：本地时区，不用 toISOString，
// 避免东八区凌晨错一天）
// ============================================================

/** 本地自然日 0 点 */
export function startOfLocalDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 加减自然日（跨月/跨年/夏令时由 Date 自行处理） */
export function addLocalDays(ms: number, days: number): number {
  const d = new Date(ms);
  d.setDate(d.getDate() + days);
  return d.getTime();
}

/** 毫秒 → 后端同款日标签 "MM-DD"（本地时区，月/日补零） */
export function dayKeyOf(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** 从起始日 0 点连续生成 count 天的日键（含起始日，升序） */
export function buildDayKeys(fromMs: number, count: number): string[] {
  const keys: string[] = [];
  for (let i = 0; i < count; i++) keys.push(dayKeyOf(addLocalDays(fromMs, i)));
  return keys;
}

/**
 * 两个本地自然日 0 点相差的自然日数（to − from；可为负）。
 *
 * 按 Y/M/D 经 UTC 归一后相减：直接拿毫秒差除以 86400000 会在夏令时切换那天
 * 少算/多算一天（本地日长 23/25 小时）。
 */
export function localDaysBetween(fromDay: number, toDay: number): number {
  const utcMidnight = (ms: number) => {
    const d = new Date(ms);
    return Date.UTC(d.getFullYear(), d.getMonth(), d.getDate());
  };
  return Math.round((utcMidnight(toDay) - utcMidnight(fromDay)) / 86_400_000);
}

/** 周一为首行的星期序号（周一 0 … 周日 6），热力图行序用 */
export function weekdayMonFirst(ms: number): number {
  return (new Date(ms).getDay() + 6) % 7;
}

/**
 * 热力图列布局：最近 HEAT_WEEKS 个自然周（列 = 周，行 = 周一…周日）。
 * 首列 = (HEAT_WEEKS-1) 周前那周的周一，末列 = 本周；每列固定 7 格。
 * 未来日期与取数窗口之外的日期为 null（空格子，占位不渲染色块）。
 *
 * 52 周最多覆盖 364 天，恒在 365 天取数窗口内；因此「MM-DD 日键在窗口内
 * 唯一」成立，用集合判断日期是否落在窗口内即可（跨年不会误命中同月同日）。
 */
export function buildHeatColumns(
  nowMs: number,
  dayKeySet: Set<string>
): (string | null)[][] {
  const todayStart = startOfLocalDay(nowMs);
  const thisMonday = addLocalDays(todayStart, -weekdayMonFirst(todayStart));
  const firstMonday = addLocalDays(thisMonday, -(HEAT_WEEKS - 1) * 7);
  const columns: (string | null)[][] = [];
  for (let col = 0; col < HEAT_WEEKS; col++) {
    const column: (string | null)[] = [];
    for (let row = 0; row < 7; row++) {
      const ms = addLocalDays(firstMonday, col * 7 + row);
      const key = dayKeyOf(ms);
      column.push(ms > todayStart || !dayKeySet.has(key) ? null : key);
    }
    columns.push(column);
  }
  return columns;
}

/** 底部月份标签的一段（连续同月列合并 + 超上限合并后的结果） */
export interface HeatMonthSpan {
  /** 起始列序号（0 基） */
  start: number;
  /** 跨列数（≥1） */
  span: number;
  /** 月份 1-12 */
  month: number;
}

/**
 * 底部月份标签行：每列取「该列第一个有效日期」的月份，连续同月合并为一段。
 * 52 周最多跨 13 个月，段数超过 HEAT_MONTH_LABEL_MAX 时反复把最短的一段并进
 * 前一段（标签更少但不丢月份语义），保证底部标签行不挤成一片。
 */
export function buildMonthSpans(
  columns: (string | null)[][]
): HeatMonthSpan[] {
  const months: (number | null)[] = [];
  let lastMonth: number | null = null;
  for (const column of columns) {
    const first = column.find((key) => key != null);
    const month: number | null = first ? monthOfDayKey(first) : lastMonth;
    months.push(month);
    if (month != null) lastMonth = month;
  }
  const spans: HeatMonthSpan[] = [];
  for (let i = 0; i < months.length; i++) {
    const month = months[i];
    if (month == null) continue;
    const prev = spans[spans.length - 1];
    if (prev && prev.month === month) prev.span += 1;
    else spans.push({ start: i, span: 1, month });
  }
  while (spans.length > HEAT_MONTH_LABEL_MAX) {
    // 从第 2 段起找最短的一段并进前一段（第 1 段没有前一段可并）
    let minIndex = 1;
    for (let i = 2; i < spans.length; i++) {
      if (spans[i].span < spans[minIndex].span) minIndex = i;
    }
    spans[minIndex - 1].span += spans[minIndex].span;
    spans.splice(minIndex, 1);
  }
  return spans;
}

/** 日键 "MM-DD" → 月份数字（热力图月度标签用） */
export function monthOfDayKey(key: string): number {
  return Number(key.slice(0, 2));
}

/** 色阶分档（0 = 无数据，1..4 由浅到深）：按当日 / 窗口峰值的比例四等分 */
export function heatLevel(value: number, max: number): 0 | 1 | 2 | 3 | 4 {
  if (!(value > 0) || !(max > 0)) return 0;
  const ratio = value / max;
  if (ratio <= 0.25) return 1;
  if (ratio <= 0.5) return 2;
  if (ratio <= 0.75) return 3;
  return 4;
}

/** 热力图图例的色阶档位（不含 0：0 = 无数据，图例里单独用文字标注，不参与「少→多」） */
export const HEAT_LEGEND_LEVELS = [1, 2, 3, 4] as const;
