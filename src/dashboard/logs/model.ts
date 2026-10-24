/**
 * 请求日志页的纯数据模型与展示工具（无 React / 无 IPC，便于单测与复用）。
 *
 * 口径说明：
 *  - 时间是**本地时区**自然日口径（今天 = 本地 0 点起），与总览页共用
 *    overview/model.ts 的 startOfLocalDay / addLocalDays，不再自造日期工具；
 *  - 来源名是**后端来源名**（RequestLogSource），与 UI 侧 AgentId 两个域，
 *    显示名复用总览页 agentLabel 的同一份品牌元数据（Z.ai / Kimi Code / ...），
 *    表格列窄，另给一套短名（ZCode / Kimi / ...）；
 *  - 花费口径与总览页一致：后端按本机价格表逐行折算好美元与人民币，前端只按
 *    展示币种取一列（costOf）。
 */
import type { RequestLogRow, RequestLogSource } from "../../types";
import type { MessageKey } from "../../i18n";
import { formatSeconds } from "../../format";
import {
  addLocalDays,
  agentLabel,
  startOfLocalDay,
  type UsageAgentId,
} from "../overview/model";

/** 每页条数（与后端缺省一致；后端会夹在 [10, 100]） */
export const LOG_PAGE_SIZE = 20;

/** 页面来源顺序（与后端 request_logs::SOURCES 同序）：筛选项与列展示顺序 */
export const LOG_SOURCES: RequestLogSource[] = [
  "zcode",
  "codex",
  "claude",
  "kimi",
  "dsh",
  "opencode",
  "minimaxcode",
];

/** 来源 → UI 侧 AgentId：显示名与品牌元数据复用总览页同一份（速度页
 *  SpeedPage 也复用本映射渲染来源列 BrandIcon——Agent id 即品牌名，
 *  见 overview/AgentTable 的同款约定） */
export const SOURCE_AGENT: Record<RequestLogSource, UsageAgentId> = {
  zcode: "zai",
  codex: "codex",
  claude: "claude",
  kimi: "kimi",
  dsh: "dsh",
  opencode: "opencodego",
  minimaxcode: "minimax",
};

/** 来源展示名（筛选下拉）：与总览页 agentLabel 完全一致；未知来源原样回显 */
export function sourceLabel(source: string): string {
  const agent = SOURCE_AGENT[source as RequestLogSource];
  return agent ? agentLabel(agent) : source;
}

/** 来源短名（表格来源列，列宽受限）：与下拉的全名区分开 */
const SOURCE_SHORT: Record<RequestLogSource, string> = {
  zcode: "ZCode",
  codex: "Codex",
  claude: "Claude",
  kimi: "Kimi",
  dsh: "DSH",
  opencode: "OpenCode",
  minimaxcode: "MiniMax",
};

export function sourceShortLabel(source: string): string {
  return SOURCE_SHORT[source as RequestLogSource] ?? source;
}

/** 时间范围预设（顺序即展示顺序；默认「今天」——排查当次请求是最常见用法）。
 *  四档与其他主窗口页统一，**复用总览页同一套 key**：同款分段控件的文案
 *  必须一致（总览用「今日」）；更长历史由自定义区间覆盖（不限制最早日期）。 */
export type LogsRange = "today" | "7d" | "30d" | "90d";

export const LOGS_RANGES: { range: LogsRange; labelKey: MessageKey }[] = [
  { range: "today", labelKey: "dashboard.overview.range.today" },
  { range: "7d", labelKey: "dashboard.overview.range.7d" },
  { range: "30d", labelKey: "dashboard.overview.range.30d" },
  { range: "90d", labelKey: "dashboard.overview.range.90d" },
];

/** 预设覆盖的自然日数（含今天） */
const RANGE_DAYS: Record<LogsRange, number> = {
  today: 1,
  "7d": 7,
  "30d": 30,
  "90d": 90,
};

/** 范围 → 查询窗口 [fromMs, toMs)（本地自然日口径）。
 *  to = now + 1：与后端 `started_at < to` 的半开区间配合，把「此刻」含进来。 */
export function rangeWindow(
  range: LogsRange,
  nowMs: number
): { fromMs: number; toMs: number } {
  const toMs = nowMs + 1;
  const todayStart = startOfLocalDay(nowMs);
  return { fromMs: addLocalDays(todayStart, -(RANGE_DAYS[range] - 1)), toMs };
}

const pad = (n: number) => String(n).padStart(2, "0");

/** 完整时间 "YYYY-MM-DD HH:mm:ss"（本地时区；单元格 title 与详情抽屉用） */
export function logTimeFull(ms: number): string {
  const d = new Date(ms);
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  );
}

/**
 * 单元格时间：今天只显 "HH:mm:ss"，其余显 "MM-DD HH:mm"；跨年的行补年份
 * （90 天窗口跨年时同月同日会混淆，完整时间恒在 title 与详情里）。
 */
export function logTimeShort(ms: number, nowMs: number): string {
  const d = new Date(ms);
  const now = new Date(nowMs);
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  if (
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  ) {
    return `${time}:${pad(d.getSeconds())}`;
  }
  if (d.getFullYear() !== now.getFullYear()) {
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${time}`;
  }
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${time}`;
}

/** 行内耗时文本（复用 format.ts 的秒单位格式："3.2s"；>100s 取整秒防长串）；
 *  缺失（null 或 <= 0）返回 null，由调用方显示占位 */
export function durationText(ms: number | null): string | null {
  return ms != null && ms > 0 ? formatSeconds(ms) : null;
}

/** 首字延迟文本：固定 2 位小数秒（百毫秒量级指标，1 位小数会把 0.35s 与
 *  0.42s 抹平成同值）；耗时列仍用一位小数的 durationText。
 *  缺失（null 或 <= 0）返回 null，由调用方显示占位。 */
export function ttftText(ms: number | null): string | null {
  return ms != null && ms > 0 ? `${(ms / 1000).toFixed(2)}s` : null;
}

/** 精确整数（千分位，跟随语言）：紧凑值之外的 title / 抽屉明细用 */
export function exactCount(n: number, locale: "zh" | "en"): string {
  return Math.round(n).toLocaleString(locale === "en" ? "en-US" : "zh-CN");
}

/**
 * 请求输出速度（tok/s），两层结构：
 *  1. **后端 speed_tps 非 null 时直接采用**——口径唯一真源 = Rust token_speed
 *     （行级判定天然含思考感知窗口：思考型请求 output 已含思考 token、生成
 *     窗口用 completed − started 全程，避免思考时长漏出分母导致速率虚高；
 *     以及 [200ms, 1h] 样本窗口边界与时序校验）。approximate =
 *     speed_quality === "request_average"（沿用 ≈ 前缀语义，标请求平均近似）；
 *  2. **后端为 null 时回退本地公式**（平滑兼容：其他来源行 / 旧版本后端未
 *     下发速度字段）：生成窗口 = 耗时 − 首字；无首字或首字异常（>= 耗时）
 *     退回总耗时，标近似（≈）。此时本地公式不做 token_speed 的窗口边界
 *     拒收——它只是旧口径的兼容回退，正常链路不该走到。
 * 两层都算不出（无耗时 / 无速度原料）返回 null。
 */
export function speedOf(
  row: RequestLogRow
): { tps: number; approximate: boolean } | null {
  // 第一层：后端真源（token_speed 判定结果原样透传，不二次加工）
  if (row.speed_tps != null) {
    return {
      tps: row.speed_tps,
      approximate: row.speed_quality === "request_average",
    };
  }
  // 第二层：本地公式回退（旧版后端 / 无速度字段的来源）
  const duration = row.duration_ms;
  if (duration == null || duration <= 0) return null;
  const ttft = row.ttft_ms;
  const generation =
    ttft != null && ttft > 0 && ttft < duration ? duration - ttft : null;
  const windowMs = generation ?? duration;
  if (windowMs <= 0) return null;
  return {
    tps: row.output_tokens / (windowMs / 1000),
    approximate: generation == null,
  };
}

/** 速度列的近似说明 key：只有「无首字、按总耗时估算」一种近似，非近似返回 null */
export function speedApproxHintKey(approximate: boolean): MessageKey | null {
  return approximate ? "dashboard.logs.speedApproxHint" : null;
}
