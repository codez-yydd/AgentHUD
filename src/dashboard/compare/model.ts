/**
 * 周额度对比页（主窗口大窗版）的纯数据模型与聚合工具：无 React、无 IPC。
 *
 * 从面板 ComparePanel（300px 窄窗版）复制改造而来：系列的建立口径（Z.ai 按
 * 账号指纹分组、其余 Agent 各一条）、远端快照的合并去重规则、Token 归集来源
 * 与窄窗完全一致——两版共存，窄窗原件不动，不发明新口径。
 * 大窗额外做的只是「同口径数据重新组织」：额度矩阵（各系列当前剩余）与
 * 时间轴折线（各系列采样点）直接消费这里输出的系列数据。
 */
import type {
  AccountMeta,
  AgentQuotaSnapshot,
  AgentQuotaWindowKey,
  QuotaSnapshot,
  RemoteSnapshot,
  RemoteUsage,
  WeeklyTokenBucket,
} from "../../types";
import type { AgentId } from "../../agentVisibility";
import { AGENT_COLOR, AGENT_COLOR_SCALE } from "../../agentVisibility";
import type { BrandIconName } from "../../BrandIcon";
import type { CompareTokenSource } from "../../api";

/** 对比页参与的 Agent（与窄窗版 COMPARE_AGENTS 同集同序）。
 *  kimi/dsh/opencodego/minimax 为本地采集型：纳入 Token 用量对比；因无订阅
 *  额度窗口快照（后端 agent_quota_history 只写 cursor/codex/claude/kimi），
 *  不登记 AGENT_WINDOW_KEY，故不出现在周额度系列里。 */
export type CompareAgent =
  | "zai"
  | "codex"
  | "claude"
  | "cursor"
  | "kimi"
  | "dsh"
  | "opencodego"
  | "minimax";

export const COMPARE_AGENTS: CompareAgent[] = [
  "zai",
  "codex",
  "claude",
  "cursor",
  "kimi",
  "dsh",
  "opencodego",
  "minimax",
];

const AGENT_META: Record<
  AgentId,
  { label: string; brand: BrandIconName; remoteSource?: string }
> = {
  zai: { label: "Z.ai", brand: "zai", remoteSource: "zcode" },
  codex: { label: "Codex", brand: "codex", remoteSource: "codex" },
  claude: { label: "Claude", brand: "claude", remoteSource: "claude" },
  cursor: { label: "Cursor", brand: "cursor" },
  kimi: { label: "Kimi Code", brand: "kimi", remoteSource: "kimi" },
  // remoteSource 是**后端来源名 "dsh"**（sync.rs 增量上传与 server 白名单同值）
  dsh: { label: "DeepSeek Harness", brand: "dsh", remoteSource: "dsh" },
  // 凭证驱动的新 provider（对比页暂未纳入，元数据先补齐保证类型完整）
  gemini: { label: "Gemini", brand: "gemini" },
  grok: { label: "Grok", brand: "grok" },
  qoder: { label: "Qoder", brand: "qoder" },
  // remoteSource 是后端来源名 "opencode"（不是 UI id `opencodego`）
  opencodego: {
    label: "OpenCode",
    brand: "opencodego",
    remoteSource: "opencode",
  },
  // remoteSource 是后端来源名 "minimaxcode"（不是 UI id `minimax`）
  minimax: {
    label: "MiniMax Code",
    brand: "minimax",
    remoteSource: "minimaxcode",
  },
  moonshot: { label: "Kimi", brand: "moonshot" },
  deepseek: { label: "DeepSeek", brand: "deepseek" },
  longcat: { label: "LongCat", brand: "longcat" },
  mimo: { label: "MiMo", brand: "mimo" },
  alibaba: { label: "通义灵码", brand: "alibaba" },
  alibabatoken: { label: "百炼Token包", brand: "alibabatoken" },
  stepfun: { label: "StepFun", brand: "stepfun" },
  doubao: { label: "火山引擎", brand: "doubao" },
};

/** 各 Agent 参与对比的周额度窗口（Z.ai 走 QuotaSnapshot.weekly_pct，不走这里）。
 *  dsh / opencodego / minimax 故意不登记：它们没有订阅额度窗口快照。 */
const AGENT_WINDOW_KEY: Partial<Record<AgentId, AgentQuotaWindowKey>> = {
  codex: "weekly",
  claude: "weekly",
  cursor: "cursor_auto",
  kimi: "weekly",
};

/**
 * AgentId → 本地取数用的**后端来源名**（get_compare_tokens_for_agent 的 source）。
 * 两个域有两个 agent 异名：UI 侧 `opencodego` → 后端 `opencode`，
 * UI 侧 `minimax` → 后端 `minimaxcode`。全量列出而不是回落，避免把 UI id
 * 原样发给后端（后端不认，报「未知的对比 Agent」）。
 */
export const LOCAL_COMPARE_SOURCE: Record<CompareAgent, CompareTokenSource> = {
  zai: "zai",
  codex: "codex",
  claude: "claude",
  cursor: "cursor",
  kimi: "kimi",
  dsh: "dsh",
  opencodego: "opencode",
  minimax: "minimaxcode",
};

// ===== 页头时间范围 =====

/** 页头时间范围预设档（趋势切片 + Token 统计周期） */
export type ComparePresetRange = "today" | "7d" | "30d" | "90d";

/** 页头时间范围选择：预设档或自定义区间（"custom" 只在页面选中自定义档时使用） */
export type CompareRange = ComparePresetRange | "custom";

/** 各预设覆盖的自然日数（含今天） */
export const COMPARE_RANGE_DAYS: Record<ComparePresetRange, number> = {
  today: 1,
  "7d": 7,
  "30d": 30,
  "90d": 90,
};

/** 全部范围预设（顺序即页头分段顺序）。
 *  Token 取数一次覆盖这里的全部预设（+ 可选自定义区间），范围切换只做视图选择。 */
export const COMPARE_RANGES: ComparePresetRange[] = [
  "today",
  "7d",
  "30d",
  "90d",
];

/** 时间窗口 [fromMs, toMs)：预设档 toMs = 取数时刻（本代际的 nowMs）；
 *  自定义区间为所选自然日半开区间（fromMs = 起始日 0 点，toMs = 结束日次日 0 点）。 */
export interface CompareWindow {
  fromMs: number;
  toMs: number;
}

/** 窗口取数缓存键（含起止毫秒）：不同自定义区间、不同代际的预设窗口互不命中 */
export function windowKeyOf(win: CompareWindow): string {
  return `${win.fromMs}|${win.toMs}`;
}

/** 额度历史固定取数窗口（天）：矩阵的「当前剩余」不随页头范围缩水，
 *  范围切换只重新切片折线，与总览页的「90 天窗口一次取数」同一思路。
 *  也是自定义区间可选最早日期与额度趋势数据的保留期（见 ComparePage）。 */
export const COMPARE_HISTORY_DAYS = 90;

/** N 天前（含今天共 N 天）的本地自然日 0 点 */
function startOfDaysAgo(days: number, nowMs: number): number {
  const d = new Date(nowMs);
  d.setDate(d.getDate() - (days - 1));
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 预设档的范围起点（本地自然日 0 点，含今天共 N 天） */
export function rangeStartMs(
  range: ComparePresetRange,
  nowMs = Date.now()
): number {
  return startOfDaysAgo(COMPARE_RANGE_DAYS[range], nowMs);
}

/** 额度历史取数起点（固定 90 天窗口） */
export function historyStartMs(nowMs = Date.now()): number {
  return startOfDaysAgo(COMPARE_HISTORY_DAYS, nowMs);
}

/**
 * 一次取数覆盖的全部窗口：四个预设档 + 可选自定义区间（顺序即展示顺序）。
 * 预设窗口的起止都取本代际的 nowMs，与 publish / 远端懒取的窗口键保持一致
 * （否则命不中同一份缓存）。
 */
export function compareWindows(
  custom: CompareWindow | null,
  nowMs = Date.now()
): { key: CompareRange; win: CompareWindow }[] {
  const list: { key: CompareRange; win: CompareWindow }[] = COMPARE_RANGES.map(
    (range) => ({
      key: range,
      win: { fromMs: rangeStartMs(range, nowMs), toMs: nowMs },
    })
  );
  if (custom) {
    list.push({
      key: "custom",
      win: { fromMs: custom.fromMs, toMs: custom.toMs },
    });
  }
  return list;
}

/** 页头当前选择 → 窗口（自定义优先；两者互斥由页面保证） */
export function selectedWindow(
  range: ComparePresetRange,
  custom: CompareWindow | null,
  nowMs = Date.now()
): CompareWindow {
  if (custom) return { fromMs: custom.fromMs, toMs: custom.toMs };
  return { fromMs: rangeStartMs(range, nowMs), toMs: nowMs };
}

/**
 * 把若干窗口的并集拆成互不重叠的相邻段（升序），只保留真正落在某个窗口内的段。
 *
 * 仅 Cursor 需要（见 useCompareData.loadLocalTokenTotals）：其后端把事件归属到
 * 「首个命中的区间」（cursor.rs 按 periods 顺序 position 匹配），重叠区间会互相
 * 截断。用互斥段一次取回后，按「段完整落在窗口内」累加即可还原各窗口合计。
 * 四个预设档全是「以此刻结尾的嵌套窗口」（today ⊂ 7d ⊂ 30d ⊂ 90d），本函数
 * 输出的分段与改动前的手工相邻区间写法完全一致。
 */
export function disjointSegments(windows: CompareWindow[]): CompareWindow[] {
  const points = [
    ...new Set(windows.flatMap((win) => [win.fromMs, win.toMs])),
  ].sort((a, b) => a - b);
  const segments: CompareWindow[] = [];
  for (let i = 0; i + 1 < points.length; i++) {
    const fromMs = points[i];
    const toMs = points[i + 1];
    if (toMs <= fromMs) continue;
    if (windows.some((win) => win.fromMs <= fromMs && toMs <= win.toMs)) {
      segments.push({ fromMs, toMs });
    }
  }
  return segments;
}

// ===== 额度系列与采样 =====

/** 一个额度数据系列：Z.ai 每账号一个，其余 Agent 各一个（与窄窗同结构） */
export interface QuotaSeries {
  id: string;
  agent: AgentId;
  /** Z.ai 账号指纹（纯旧版客户端的历史采样为 null） */
  account: string | null;
  label: string;
}

/** 单个额度采样点（窄窗只取「周内峰值」，大窗保留完整时间线画折线） */
export interface QuotaSample {
  ts: number;
  /** 周额度已用 %（0-100，已收敛） */
  usedPct: number;
  /** 下次重置时刻（毫秒；快照未提供为 null） */
  resetAt: number | null;
  /** 5 小时窗口已用 %（无该窗口为 null） */
  hour5UsedPct: number | null;
}

export interface QuotaTimeline {
  series: QuotaSeries;
  /** 升序采样点（仅保留百分比有效的样本） */
  samples: QuotaSample[];
}

/**
 * 暗色主题下的系列色阶提亮表（只登记在 slate-900 底上对比度不足的深档色系：
 * indigo / 深蓝；其余 Agent 的色阶本身在暗底可读，未登记即沿用原色阶）。
 * 不修改 agentVisibility 的原始色阶常量——面板（深色玻璃底）仍在用同一份，
 * 这里只做主窗口对比页的内联取色兜底。
 */
const AGENT_COLOR_SCALE_DARK: Partial<Record<AgentId, readonly string[]>> = {
  kimi: ["#818cf8", "#a5b4fc", "#c7d2fe", "#e0e7ff"],
  moonshot: ["#918ce9", "#b5b1f0", "#cfccf6", "#e4e2fb"],
  deepseek: ["#7a90fe", "#a6b4fe", "#c3cefe", "#dfe5ff"],
  dsh: ["#7a90fe", "#a6b4fe", "#c3cefe", "#dfe5ff"],
  qoder: ["#8d7ded", "#ae9ff2", "#c9c0f7", "#e0dafb"],
};

/**
 * 主窗口对比页的智谱（zai）多账号系列专用分类色板（跨色相区分，首档保留品牌
 * 天蓝）。原 AGENT_COLOR_SCALE.zai 是同色相 4 档天蓝只差深浅，而智谱是唯一
 * 会按账号指纹拆成多系列的 Agent——多条折线 / 色点叠在一张图上无法分辨；
 * 这里改用跨色相分类色。仅主窗口对比页生效（窄窗 ComparePanel 沿用
 * agentVisibility 原色阶，本文件不修改那份常量）。暗色档为对应亮色在
 * slate-900 底上的提亮变体。
 */
const ZAI_SERIES_COLORS = {
  light: ["#0ea5e9", "#d97706", "#db2777", "#0d9488"],
  dark: ["#38bdf8", "#fbbf24", "#f472b6", "#2dd4bf"],
} as const;

/** 取系列颜色：单系列用品牌色，同一 Agent 的多账号系列取色阶档位（窄窗同款）。
 *  dark=true（暗色主题）时优先取提亮色阶（见 AGENT_COLOR_SCALE_DARK）。
 *  智谱多账号例外：走 ZAI_SERIES_COLORS 跨色相分类色（同色相深浅在折线图上
 *  不可辨），其余 Agent 维持原色阶逻辑。 */
export function seriesColorOf(
  series: QuotaSeries[],
  id: string,
  dark = false
): string {
  const target = series.find((item) => item.id === id);
  if (!target) return AGENT_COLOR_SCALE.zai[0]; // 正常不可达的兜底
  const scale =
    target.agent === "zai"
      ? ZAI_SERIES_COLORS[dark ? "dark" : "light"]
      : (dark ? AGENT_COLOR_SCALE_DARK[target.agent] : undefined) ??
        AGENT_COLOR_SCALE[target.agent];
  const index = series
    .filter((item) => item.agent === target.agent)
    .indexOf(target);
  return scale[Math.min(Math.max(index, 0), scale.length - 1)];
}

/** 快照百分比统一按 0-100 收敛；非法值视为无效采样。 */
function clampUsedPct(value: number | null | undefined): number | null {
  if (value == null || !Number.isFinite(value)) return null;
  return Math.min(100, Math.max(0, value));
}

/**
 * 从合并后的额度快照构建各系列的时间线。
 * Z.ai 按账号指纹分组（每账号一个系列）；Codex/Claude/Cursor/Kimi 各一个系列。
 * 无账号指纹的旧快照无法归属，只要同期存在带指纹的采样就整体丢弃（否则会
 * 在对比页多出一条"旧快照"系列）；全部都是无指纹的纯旧版客户端才保留一条
 * 不区分账号的 "Z.ai" 系列，避免曲线整个消失。
 */
export function buildQuotaTimelines(
  zaiSnapshots: QuotaSnapshot[],
  agentSnapshots: AgentQuotaSnapshot[],
  enabledAgents: CompareAgent[],
  accounts: AccountMeta[] | null
): QuotaTimeline[] {
  const timelines: QuotaTimeline[] = [];

  if (enabledAgents.includes("zai")) {
    const hasFingerprint = zaiSnapshots.some((snapshot) => snapshot.account);
    const usable = hasFingerprint
      ? zaiSnapshots.filter((snapshot) => snapshot.account)
      : zaiSnapshots;
    const groups = new Map<string, QuotaSnapshot[]>();
    for (const snapshot of usable) {
      if (!Number.isFinite(snapshot.ts)) continue;
      const key = snapshot.account ?? "";
      const list = groups.get(key) ?? [];
      list.push(snapshot);
      groups.set(key, list);
    }
    const keys = [...groups.keys()];
    const single = keys.length <= 1;
    // 指纹组在前（按指纹排序），（纯旧版客户端时的）无指纹组最后
    keys.sort((a, b) => {
      if (a === "") return 1;
      if (b === "") return -1;
      return a.localeCompare(b);
    });
    for (const key of keys) {
      const id = `zai:${key || "history"}`;
      let label: string;
      if (key === "") {
        label = AGENT_META.zai.label;
      } else if (single) {
        label = AGENT_META.zai.label;
      } else {
        const name = accounts
          ?.find((account) => account.fingerprint === key)
          ?.display_name?.trim();
        label = `${AGENT_META.zai.label}·${name || key.slice(0, 6)}`;
      }
      const timeline: QuotaTimeline = {
        series: { id, agent: "zai", account: key || null, label },
        samples: [],
      };
      for (const snapshot of groups.get(key) ?? []) {
        const usedPct = clampUsedPct(snapshot.weekly_pct);
        if (usedPct == null) continue;
        timeline.samples.push({
          ts: snapshot.ts,
          usedPct,
          resetAt:
            snapshot.weekly_reset != null && Number.isFinite(snapshot.weekly_reset)
              ? snapshot.weekly_reset
              : null,
          hour5UsedPct: clampUsedPct(snapshot.hour5_pct),
        });
      }
      timeline.samples.sort((a, b) => a.ts - b.ts);
      timelines.push(timeline);
    }
  }

  for (const agent of COMPARE_AGENTS) {
    const windowKey = AGENT_WINDOW_KEY[agent];
    if (!windowKey || !enabledAgents.includes(agent)) continue;
    const snapshots = agentSnapshots.filter(
      (snapshot) => snapshot.source === agent
    );
    if (snapshots.length === 0) continue; // 无采样的 Agent 不建空系列
    const timeline: QuotaTimeline = {
      series: { id: agent, agent, account: null, label: AGENT_META[agent].label },
      samples: [],
    };
    for (const snapshot of snapshots) {
      const window = snapshot.windows.find((item) => item.key === windowKey);
      if (!window) continue;
      const usedPct = clampUsedPct(window.used_pct);
      if (usedPct == null) continue;
      const hour5 = snapshot.windows.find((item) => item.key === "hour5");
      timeline.samples.push({
        ts: snapshot.ts,
        usedPct,
        resetAt:
          window.reset_at != null && Number.isFinite(window.reset_at)
            ? window.reset_at
            : null,
        hour5UsedPct: hour5 ? clampUsedPct(hour5.used_pct) : null,
      });
    }
    timeline.samples.sort((a, b) => a.ts - b.ts);
    timelines.push(timeline);
  }

  return timelines;
}

// ===== 额度矩阵（各系列当前剩余） =====

export interface MatrixCell {
  id: string;
  label: string;
  /** 系列颜色（同一 Agent 多账号取色阶档位） */
  color: string;
  /** 周额度剩余 %（= 100 − 已用），来自最近一次采样 */
  remainingPct: number;
  /** 下次重置时刻（毫秒，可能为 null） */
  resetAt: number | null;
  /** 5 小时窗口剩余 %（无该窗口为 null） */
  hour5RemainingPct: number | null;
  /** 窗口语义：weekly = 周额度；billing = 账单周期（Cursor） */
  windowKind: "weekly" | "billing";
  /** 最近一次采样时刻（毫秒） */
  sampledAt: number;
}

export interface MatrixRow {
  agent: AgentId;
  label: string;
  brand: BrandIconName;
  color: string;
  cells: MatrixCell[];
}

/**
 * 矩阵行：每个 Agent 一行，行内每个「系列」一个额度块。
 * 无任何有效采样的系列不进矩阵（矩阵表达的是「当前剩余」，没有采样就没有当前值）；
 * 趋势卡仍会在图例里保留该系列（折线为空），两处口径一致。
 * dark：暗色主题下系列色点走提亮色阶（见 seriesColorOf）。
 */
export function buildQuotaMatrix(
  timelines: QuotaTimeline[],
  dark = false
): MatrixRow[] {
  const seriesList = timelines.map((item) => item.series);
  const rows: MatrixRow[] = [];
  for (const agent of COMPARE_AGENTS) {
    const cells: MatrixCell[] = [];
    for (const timeline of timelines) {
      if (timeline.series.agent !== agent) continue;
      const latest = timeline.samples[timeline.samples.length - 1];
      if (!latest) continue;
      cells.push({
        id: timeline.series.id,
        label: timeline.series.label,
        color: seriesColorOf(seriesList, timeline.series.id, dark),
        remainingPct: 100 - latest.usedPct,
        resetAt: latest.resetAt,
        hour5RemainingPct:
          latest.hour5UsedPct == null ? null : 100 - latest.hour5UsedPct,
        // 与总览页额度列同口径：Cursor 的 cursor_auto 是账单周期，其余为周窗口
        windowKind: agent === "cursor" ? "billing" : "weekly",
        sampledAt: latest.ts,
      });
    }
    if (cells.length === 0) continue;
    rows.push({
      agent,
      label: AGENT_META[agent].label,
      brand: AGENT_META[agent].brand,
      // 行色与面板/总览页的 AGENT_COLOR 惯例一致（系列色才取色阶档位）
      color: AGENT_COLOR[agent],
      cells,
    });
  }
  return rows;
}

// ===== Token 对比（本机 + 远端合并） =====

export interface TokenComparison {
  agent: CompareAgent;
  label: string;
  color: string;
  localTokens: number;
  remoteTokens: number;
  localRequests: number;
  remoteRequests: number;
}

/** 本地「按 Agent 聚合」结果（单周期调用时只有一个桶）求和。 */
export function sumBuckets(buckets: WeeklyTokenBucket[]): {
  tokens: number;
  requests: number;
} {
  let tokens = 0;
  let requests = 0;
  for (const bucket of buckets) {
    tokens += bucket.total_tokens;
    requests += bucket.requests;
  }
  return { tokens, requests };
}

/** 本地「多窗口一次取数」结果 → 窗口键（`${起点}|${终点}`）→ 合计。
 *  后端把入参的 reset_at / end_at 原样回填到每个桶（见 db::query_period_buckets
 *  及各来源实现），因此按起止对号入座即可，不依赖返回顺序；键必须含 end_at：
 *  自定义区间可能与预设档共享同一个起点（只是终点不同），只按起点会串桶。 */
export function bucketTotalsByWindow(
  buckets: WeeklyTokenBucket[]
): Map<string, { tokens: number; requests: number }> {
  const out = new Map<string, { tokens: number; requests: number }>();
  for (const bucket of buckets) {
    const key = windowKeyOf({ fromMs: bucket.reset_at, toMs: bucket.end_at });
    const previous = out.get(key);
    out.set(key, {
      tokens: (previous?.tokens ?? 0) + bucket.total_tokens,
      requests: (previous?.requests ?? 0) + bucket.requests,
    });
  }
  return out;
}

/** 远端逐小时桶宽度（远端取数固定用 hour 桶）：切片求和时判定区间重叠 */
const REMOTE_BUCKET_MS = 3_600_000;

/** 远端桶 label → 桶起始毫秒。服务端下发的是桶起始 ms 字符串
 *  （server/db.py 里 label = str(start)），ISO 串作为兜底格式解析。 */
function bucketStartOf(label: string): number | null {
  const numeric = Number(label);
  if (label.trim() !== "" && Number.isFinite(numeric)) return numeric;
  const parsed = Date.parse(label);
  return Number.isFinite(parsed) ? parsed : null;
}

/**
 * 远端用量里落在 [fromMs, toMs] 的桶求和（范围切换的视图切片）。
 *
 * 判定用「桶区间与范围重叠」而不是「桶起点落在范围内」：服务端把起点按桶宽
 * 向下对齐（_align_bucket_start_utc），首个桶可能跨越范围起点、末个桶可能
 * 跨越当前时刻——重叠判定与原来「直接按范围请求」的取数结果一致。
 * 无法解析的时间标签按「计入」处理（宁可多算不可静默丢数据；当前服务端
 * 与兜底格式都能解析，正常不会走到）。
 */
export function sumRemoteUsageIn(
  remote: RemoteUsage,
  fromMs: number,
  toMs: number
): { tokens: number; requests: number } {
  let tokens = 0;
  let requests = 0;
  for (const point of remote.trend) {
    const start = bucketStartOf(point.label);
    if (start != null && (start + REMOTE_BUCKET_MS <= fromMs || start > toMs)) {
      continue;
    }
    tokens += point.total_tokens;
    requests += point.requests;
  }
  return { tokens, requests };
}

/** 远端来源名（AGENT_META.remoteSource）：无来源的 Agent 只按本机口径对比。 */
export function remoteSourceOf(agent: CompareAgent): string | undefined {
  return AGENT_META[agent].remoteSource;
}

export function agentLabelOf(agent: CompareAgent): string {
  return AGENT_META[agent].label;
}

export function agentBrandOf(agent: CompareAgent): BrandIconName {
  return AGENT_META[agent].brand;
}

export function agentColorOf(agent: CompareAgent): string {
  return AGENT_COLOR[agent];
}

// ===== 快照合并（窄窗同款，逐字段对齐） =====

/** 远端快照去掉设备字段，转换为按指纹分组所需的本地结构。 */
export function toQuotaSnapshot(snapshot: RemoteSnapshot): QuotaSnapshot {
  const { device_id: _deviceId, ...quotaSnapshot } = snapshot;
  return quotaSnapshot;
}

/** 合并本机与远端额度采样；按「时间 + 账号指纹」去重，同一时刻多设备
 *  重复采样时保留已用比例更高的一条，避免汇总视图低估额度峰值。 */
export function mergeQuotaSnapshots(
  local: QuotaSnapshot[],
  remote: QuotaSnapshot[]
): QuotaSnapshot[] {
  const byKey = new Map<string, QuotaSnapshot>();
  for (const snapshot of [...local, ...remote]) {
    if (!Number.isFinite(snapshot.ts)) continue;
    const key = `${snapshot.ts}:${snapshot.account ?? ""}`;
    const previous = byKey.get(key);
    if (
      !previous ||
      snapshot.weekly_pct > previous.weekly_pct ||
      (snapshot.weekly_pct === previous.weekly_pct &&
        snapshot.hour5_pct > previous.hour5_pct)
    ) {
      byKey.set(key, snapshot);
    }
  }
  return [...byKey.values()].sort((a, b) => a.ts - b.ts);
}

/** 毫秒 → "MM-DD" label（趋势轴标签） */
export function dateLabel(ms: number): string {
  const d = new Date(ms);
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${mm}-${dd}`;
}
