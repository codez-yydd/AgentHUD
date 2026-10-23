/**
 * 模型速度页（主窗口第五页）：跨来源、跨渠道的模型输出速度排行 + 总体概览。
 *
 * 数据契约：与窄浮窗速度页（src/ModelSpeedPanel.tsx）**同一后端两种呈现**——
 * getModelSpeed(days, scope) → ModelSpeedResponse { stats, remoteFailed }，
 * 分组键 (source, provider, model_id)：同名模型跨来源 / 跨渠道绝不合并
 * （deepseek 走 OpenCode Go 订阅与走官方 key 是两行，GLM 在不同中转站同理）。
 *
 * 口径分层（后端 model_speed.rs 的两池设计）：speedQuality = "generation"
 * （zcode 事件时刻的精确生成窗口）/ "request_average"（导入库与远端行按
 * 请求总耗时折算的近似，**含首字延迟**）/ null（无速度样本，速度列「—」）。
 * 近似口径行的 P10/P50/P90 数值前加 ≈（对齐日志页与窄窗的 ≈ 约定，大窗
 * 列宽足够直接前缀），title 挂 speed.approxHint 说明口径。
 *
 * 偏好独立：zbar-speed-dash-range（预设档，默认 7d；旧键 zbar-speed-dash-days
 * 读取时自动迁移）/ zbar-speed-dash-custom（自定义区间 JSON）/ zbar-speed-dash-
 * scope（默认 local）——与窄浮窗的 zbar-speed-range / zbar-speed-scope
 * **完全独立**：两窗可能同时开，互不影响口径与档位（LogsPage 的 DEVICE_KEY
 * 注释先例）。
 *
 * 版面：对齐 LogsPage 的「不整页滚动」模式——布局类直接复用 db-logs-page /
 * db-logs-card / db-logs-filters / db-logs-metrics（它们只含 flex 列 + 表体
 * 内滚 + sticky 表头的通用版面规则，无日志专属样式）：表格卡 flex:1、表体
 * .db-table-wrap 滚动，900×640 最小窗口可用。**筛选行在指标行之前**（本页
 * 指标随来源筛选变化，先筛选后看数因果更清晰，日志页同款次序）。
 *
 * 交互：
 *  - 时间范围 = CustomRangeMenu（总览 / 日志 / 对比三页共用件）：三档预设
 *    （今日 / 近 7 天 / 近 30 天）+ 自定义起止自然日历，两模式互斥（选预设
 *    清自定义，应用自定义后预设失效仅作回退档）。预设档前端折算窗口
 *    （今日 = 本地零点、7d / 30d = 滚动窗口，与后端 days 口径一致）；自定义
 *    区间固定透传（历史区间不按此刻推算）。30 天窗口 zcode 约 4 万行进内存
 *    聚合仍可控——近 7 天 <1 万行的既有评估 ×4，逐行累加 + 组内几十级分位
 *    排序毫秒级完成；
 *  - 数据范围（本机 / 全部设备，db-seg）：scope=local 不触网。同步未启用时
 *    「全部设备」禁用并固定本机口径（总览页 / 日志页同款先例）；首发射数
 *    挂在同步配置读毕之后（缓存命中即瞬时，避免「先按本机查、配置到位后
 *    再按全部设备全量重取」的双取，LogsPage 同款规避）。远端失败
 *    （remoteFailed=true）不清数据——后端已按纯本机口径降级返回，页面走
 *    db-hintbar 弱提示；
 *  - 来源筛选**纯前端过滤**（分组粒度几十级，不重查）；排序亦纯前端，
 *    排序键只存组件 state（不写 localStorage）；默认 P50 降序、null 恒排尾；
 *  - **排名列恒显速度名次**（按 P50 降序对当前筛选集预计算的稳定名次，
 *    P50 缺失的行无名次）：切到其他列排序时名次乱序出现是有效信息（该行
 *    在速度维度的位置）；第一名高亮只属于速度第一，不随排序漂移；
 *  - 30 秒自动刷新 + 手动刷新；document.hidden（主窗口关闭 = 隐藏）时跳过
 *    本轮。定时刷新走**静默**通道（不置 loading——刷新按钮不转圈、表格与
 *    指标不压暗不闪骨架，LogsPage 同款约定）；静默刷新失败保留已展示数据
 *    不打断（下一次刷新自愈），只有手动 / 切档位触发的查询失败才切整页
 *    错误态；
 *  - 请求序号守卫（窄窗 P1 教训）：useRef 递增序号，then / catch / finally
 *    三路径只有最新序号才落状态——stats / remoteFailed / updatedAt / error /
 *    loading 五个 setter 全覆盖，切档位时慢的旧响应不会覆盖新数据。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useI18n, type MessageKey } from "../i18n";
import { EmptyState } from "../layout";
import { DropdownMenu, type DropdownOption } from "../DropdownMenu";
import { getModelSpeed } from "../api";
import type {
  ModelSpeedScope,
  ModelSpeedStat,
  RequestLogSource,
  SyncConfig,
} from "../types";
import {
  formatCountLocale,
  formatPct,
  formatSeconds,
  formatTokensLocale,
  formatTps,
} from "../format";
import { RefreshIcon } from "./icons";
import { Card, Skeleton } from "./primitives";
import {
  CustomRangeMenu,
  formatRangeSummary,
  type CustomRangeValue,
} from "./CustomRangeMenu";
import { displayModelName } from "../modelName";
import { BrandIcon } from "../BrandIcon";
import {
  LOG_SOURCES,
  SOURCE_AGENT,
  exactCount,
  sourceLabel,
  sourceShortLabel,
  ttftText,
} from "./logs/model";
import {
  peekSyncConfigCached,
  readSyncConfigCached,
} from "./syncConfig";

/** 时间档位（后端 days 参数：今日 = 0 自然日零点起，其余 = 滚动窗口） */
type SpeedDays = "today" | "7d" | "30d";

const DAYS_VALUE: Record<SpeedDays, number> = { today: 0, "7d": 7, "30d": 30 };

/** 天数档位（顺序即分段按钮展示顺序；文案复用总览页同一套 range key） */
const DAYS_OPTIONS: { days: SpeedDays; labelKey: MessageKey }[] = [
  { days: "today", labelKey: "dashboard.overview.range.today" },
  { days: "7d", labelKey: "dashboard.overview.range.7d" },
  { days: "30d", labelKey: "dashboard.overview.range.30d" },
];

/** 偏好持久化键（大窗独立于窄浮窗，见文件头说明；失败静默——记忆仅锦上添花）。
 *  预设档 zbar-speed-dash-range 与自定义区间 zbar-speed-dash-custom（JSON
 *  {fromMs,toMs}）分开存：自定义被取消（选回预设）时回退到上次预设档。
 *  旧键 zbar-speed-dash-days（只有预设档的年代）读取时迁移到新键后写回 */
const RANGE_KEY = "zbar-speed-dash-range";
const CUSTOM_KEY = "zbar-speed-dash-custom";
const LEGACY_DAYS_KEY = "zbar-speed-dash-days";
const SCOPE_KEY = "zbar-speed-dash-scope";

/** 常见 provider slug 前缀 → 官方产品名（渠道列展示；项目无既有 plan 名
 *  静态映射——QuotaEntryCard 的 planName 是后端快照动态字段，故此处自持
 *  一份速度页映射）。真实 slug 形如 `account:bigmodel-individual-coding-plan`
 *  （后端剥 account: 后仍带 -coding-plan 等档位后缀），故按**前缀**匹配；
 *  key 不含后缀，各自命中变体。**品牌名不进词典**（GLM Coding Plan 是智谱
 *  官方产品名，翻译反而两头不讨好——中文用户不认自造译名、英文用户看不懂
 *  中文，zh/en 统一显示）。映射不到的渠道原样显示；渠道 title 恒给原始
 *  provider_id 全量，友好名只管展示可读性 */
const PROVIDER_LABEL: Record<string, string> = {
  // 智谱 GLM Coding Plan 的账号 slug（zcode 渠道剥 account: 前缀后的值，
  // 真实形态如 bigmodel-individual-coding-plan / bigmodel-max-...）
  "bigmodel-individual": "GLM Coding Plan",
  "bigmodel-max": "GLM Coding Plan Max",
};

/** 前缀匹配的遍历序：长 key 优先（防未来多档 slug 前缀重叠时误配短 key，
 *  如 bigmodel-max-team 与 bigmodel-max 并存时先撞长 key） */
const PROVIDER_KEYS_DESC: string[] = Object.keys(PROVIDER_LABEL).sort(
  (a, b) => b.length - a.length
);

function loadDays(): SpeedDays {
  try {
    const saved =
      localStorage.getItem(RANGE_KEY) ?? localStorage.getItem(LEGACY_DAYS_KEY);
    if (saved === "today" || saved === "7d" || saved === "30d") {
      // 旧键存量迁移：写入新键（旧键保留无害，读取侧此后只认新键）
      if (localStorage.getItem(RANGE_KEY) == null) {
        localStorage.setItem(RANGE_KEY, saved);
      }
      return saved;
    }
  } catch {
    /* 存储不可用：回退默认档 */
  }
  return "7d";
}

/** 读取自定义区间偏好：JSON 形态校验（数值 + to > from），损坏值回退 null
 *  （预设档兜底），不阻塞页面 */
function loadCustom(): CustomRangeValue | null {
  try {
    const raw = localStorage.getItem(CUSTOM_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (
      typeof parsed === "object" &&
      parsed != null &&
      typeof (parsed as { fromMs?: unknown }).fromMs === "number" &&
      typeof (parsed as { toMs?: unknown }).toMs === "number" &&
      (parsed as { toMs: number }).toMs > (parsed as { fromMs: number }).fromMs
    ) {
      return parsed as CustomRangeValue;
    }
  } catch {
    /* 损坏值：回退预设档 */
  }
  return null;
}

/** 预设档窗口折算（与后端 days 口径一致：今日 = 本地时区自然日零点、
 *  7d / 30d = 滚动窗口 now − N×24h）。自定义区间不走这里——原样透传，
 *  历史区间固定、不按此刻推算（日志页先例） */
function daysWindow(days: SpeedDays): { fromMs: number; toMs: number } {
  const now = Date.now();
  if (days === "today") {
    const midnight = new Date();
    midnight.setHours(0, 0, 0, 0);
    return { fromMs: midnight.getTime(), toMs: now };
  }
  return { fromMs: now - DAYS_VALUE[days] * 86_400_000, toMs: now };
}

function loadScope(): ModelSpeedScope {
  try {
    const saved = localStorage.getItem(SCOPE_KEY);
    if (saved === "local" || saved === "all") return saved;
  } catch {
    /* 存储不可用：回退默认口径（本机，不触网） */
  }
  return "local";
}

function savePref(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等 */
  }
}

/** 刷新时刻（本地时区 HH:mm:ss；与日志页页头同款） */
function timeText(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** 可排序列键（排名列恒显速度名次不参与排序；渠道列文本短、按需求不排序） */
type SpeedSortKey =
  | "model"
  | "source"
  | "p10"
  | "p50"
  | "p90"
  | "ttft"
  | "dur"
  | "in"
  | "out"
  | "requests"
  | "samples"
  | "success";

/** 排序取值器：null = 无值（恒排尾部）；文本列 localeCompare，数值列比较 */
const SORT_ACCESSORS: Record<
  SpeedSortKey,
  (row: ModelSpeedStat) => string | number | null
> = {
  model: (row) => row.modelId,
  source: (row) => row.source,
  p10: (row) => row.p10Tps,
  p50: (row) => row.p50Tps,
  p90: (row) => row.p90Tps,
  ttft: (row) => row.ttftAvgMs,
  dur: (row) => row.durAvgMs,
  in: (row) => row.inAvg,
  out: (row) => row.outAvg,
  requests: (row) => row.requests,
  samples: (row) => row.speedSampleCount,
  success: (row) => row.successRate,
};

/** 文本列点击默认升序、数值列默认降序（再次点击同列切换方向） */
function defaultDir(key: SpeedSortKey): "asc" | "desc" {
  return key === "model" || key === "source" ? "asc" : "desc";
}

/** 稳定次序（排序值相等时）：请求数降序 → 模型名 → 来源（与后端组内序一致） */
function tiebreak(a: ModelSpeedStat, b: ModelSpeedStat): number {
  return (
    b.requests - a.requests ||
    a.modelId.localeCompare(b.modelId) ||
    a.source.localeCompare(b.source)
  );
}

/** 排序比较：null 恒排尾部（无论方向——无值不参与名次语义），再按方向比较 */
function compareRows(
  a: ModelSpeedStat,
  b: ModelSpeedStat,
  key: SpeedSortKey,
  dir: "asc" | "desc"
): number {
  const va = SORT_ACCESSORS[key](a);
  const vb = SORT_ACCESSORS[key](b);
  if (va == null && vb == null) return tiebreak(a, b);
  if (va == null) return 1;
  if (vb == null) return -1;
  let cmp: number;
  if (typeof va === "string" || typeof vb === "string") {
    cmp = String(va).localeCompare(String(vb));
  } else {
    cmp = va - vb;
  }
  if (dir === "desc") cmp = -cmp;
  return cmp || tiebreak(a, b);
}

/** 行唯一键（React key 与速度名次 map 共用）：分组键三元组串接 */
function rowKey(row: ModelSpeedStat): string {
  return `${row.source}:${row.provider}:${row.modelId}`;
}

/** 灰化占位（与日志表同款） */
function Dash({ title }: { title?: string }) {
  return (
    <span className="db-table__dash" title={title}>
      —
    </span>
  );
}

/** 可排序表头：按钮占满 th（继承对齐与字号），激活列高亮 + 方向箭头；
 *  aria-sort 供读屏；title 挂该列口径说明（速度分位等） */
function SortTh({
  label,
  sortKey,
  sort,
  onSort,
  num,
  className,
  title,
}: {
  label: string;
  sortKey: SpeedSortKey;
  sort: { key: SpeedSortKey; dir: "asc" | "desc" };
  onSort: (key: SpeedSortKey) => void;
  num?: boolean;
  className?: string;
  title?: string;
}) {
  const active = sort.key === sortKey;
  return (
    <th
      className={`${num ? "db-table__num " : ""}${className ?? ""}`.trim()}
      aria-sort={
        active ? (sort.dir === "desc" ? "descending" : "ascending") : "none"
      }
    >
      <button
        type="button"
        className={`db-speed__th-sort${active ? " db-speed__th-sort--active" : ""}`}
        onClick={() => onSort(sortKey)}
        title={title}
      >
        {label}
        {active && (
          <span className="db-speed__th-arrow" aria-hidden>
            {sort.dir === "desc" ? "▼" : "▲"}
          </span>
        )}
      </button>
    </th>
  );
}

/** 表头（真实表格与骨架共用同一份，保证形状一致） */
function SpeedTableHead({
  sort,
  onSort,
}: {
  sort: { key: SpeedSortKey; dir: "asc" | "desc" };
  onSort: (key: SpeedSortKey) => void;
}) {
  const { t } = useI18n();
  return (
    <thead>
      <tr>
        <th className="db-speed__col-rank" title={t("dashboard.speed.rankHint")}>
          {t("dashboard.speed.colRank")}
        </th>
        <SortTh label={t("dashboard.logs.colModel")} sortKey="model" sort={sort} onSort={onSort} className="db-speed__col-model" />
        <SortTh label={t("dashboard.logs.colSource")} sortKey="source" sort={sort} onSort={onSort} className="db-speed__col-source" />
        <th className="db-speed__col-provider">{t("dashboard.speed.colProvider")}</th>
        <SortTh label={t("speed.p10")} sortKey="p10" sort={sort} onSort={onSort} num className="db-speed__col-p10" title={t("dashboard.speed.p10Hint")} />
        <SortTh label={t("speed.p50")} sortKey="p50" sort={sort} onSort={onSort} num className="db-speed__col-p50" title={t("dashboard.speed.p50Hint")} />
        <SortTh label={t("speed.p90")} sortKey="p90" sort={sort} onSort={onSort} num className="db-speed__col-p90" title={t("dashboard.speed.p90Hint")} />
        <SortTh label={t("speed.ttft")} sortKey="ttft" sort={sort} onSort={onSort} num className="db-speed__col-ttft" />
        <SortTh label={t("speed.latencyShort")} sortKey="dur" sort={sort} onSort={onSort} num className="db-speed__col-dur" />
        <SortTh label={t("speed.inputShort")} sortKey="in" sort={sort} onSort={onSort} num className="db-speed__col-in" title={t("dashboard.speed.inTotalHint")} />
        <SortTh label={t("speed.outputShort")} sortKey="out" sort={sort} onSort={onSort} num className="db-speed__col-out" title={t("dashboard.speed.outTotalHint")} />
        <SortTh label={t("speed.samplesShort")} sortKey="requests" sort={sort} onSort={onSort} num className="db-speed__col-requests" />
        <SortTh label={t("dashboard.speed.colSamples")} sortKey="samples" sort={sort} onSort={onSort} num className="db-speed__col-samples" />
        <SortTh label={t("speed.overviewSuccess")} sortKey="success" sort={sort} onSort={onSort} num className="db-speed__col-success" />
      </tr>
    </thead>
  );
}

/** 骨架（行数固定 12：分组粒度几十级，形状对齐即可；14 列与表头恒一致，
 *  响应式隐藏机制已随 1199px 断点整体删除，无需列类对齐） */
function SpeedTableSkeleton({
  sort,
  onSort,
}: {
  sort: { key: SpeedSortKey; dir: "asc" | "desc" };
  onSort: (key: SpeedSortKey) => void;
}) {
  return (
    <div className="db-table-wrap">
      <table className="db-table db-speed-table">
        <SpeedTableHead sort={sort} onSort={onSort} />
        <tbody>
          {Array.from({ length: 12 }, (_, rowIndex) => (
            <tr key={rowIndex}>
              {Array.from({ length: 14 }, (_, colIndex) => (
                <td key={colIndex}>
                  <Skeleton
                    className={colIndex === 1 ? "h-3.5 w-32" : "h-3.5 w-10"}
                  />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** 单行：口径标识（≈）+ 各列格式化（locale 系函数与日志页同一套纪律；
 *  TTFT 用 logs/model 的 ttftText 两位小数——首字是百毫秒量级指标，
 *  一位小数会把 0.35s 与 0.42s 抹平成同值，速度页正是对比首字的场景） */
function SpeedTableRow({
  row,
  speedRank,
  isSpeedTop,
}: {
  row: ModelSpeedStat;
  /** 速度名次（按 P50 降序的恒定名次，与当前排序列无关；null = 无 P50） */
  speedRank: number | null;
  /** 是否速度第一（P50 最高者——名次高亮只属于它，不随排序漂移） */
  isSpeedTop: boolean;
}) {
  const { locale, t } = useI18n();
  // 近似口径（导入库 / 远端行）：速度三列加 ≈ + title（大窗列宽足够直接
  // 前缀，无窄窗的截断问题）；null 口径速度列照常「—」
  const approximate = row.speedQuality === "request_average";
  const approxTitle = approximate ? t("speed.approxHint") : undefined;
  const tpsText = (value: number | null) =>
    value == null ? null : `${approximate ? "≈" : ""}${formatTps(value)}`;
  const agent = SOURCE_AGENT[row.source as RequestLogSource];
  // 渠道友好名：按 key 前缀匹配（长 key 优先）命中已知 slug（真实形态如
  // bigmodel-individual-coding-plan）→ 官方产品名（GLM Coding Plan 等，
  // 不翻译），其余原样；title 恒给原始 provider_id 全量（悬停可对回）
  const providerKey = PROVIDER_KEYS_DESC.find((key) =>
    row.provider.startsWith(key)
  );
  const providerText =
    providerKey != null ? PROVIDER_LABEL[providerKey] : row.provider;
  return (
    <tr>
      <td className="db-speed__col-rank num">
        {speedRank != null ? (
          <span className={isSpeedTop ? "db-speed__rank--top" : ""}>{speedRank}</span>
        ) : (
          <Dash title={t("speed.samplesNone")} />
        )}
      </td>
      <td className="db-speed__col-model">
        <span className="db-speed__model" title={row.modelId}>
          {displayModelName(row.modelId)}
        </span>
      </td>
      {/* 来源：品牌小图标 + 短名（title 给下拉一致的全名；agent id 即
          BrandIcon 品牌名，AgentTable 同款约定） */}
      <td className="db-speed__col-source" title={sourceLabel(row.source)}>
        <span className="db-speed__source">
          {agent && <BrandIcon brand={agent} className="h-3 w-3 db-speed__source-icon" />}
          {sourceShortLabel(row.source)}
        </span>
      </td>
      {/* 渠道：已知 slug 显示官方产品名（GLM Coding Plan 等，前缀匹配），
          title 恒给原始 provider_id 全量（悬停可对回） */}
      <td className="db-speed__col-provider">
        {row.provider ? (
          <span className="db-speed__provider" title={row.provider}>
            {providerText}
          </span>
        ) : (
          <Dash />
        )}
      </td>
      <td className="db-table__num db-speed__col-p10 num" title={approxTitle}>
        {row.p10Tps != null ? tpsText(row.p10Tps) : <Dash />}
      </td>
      {/* P50 主列：数值带小号 t/s 单位（日志页速度列同款 db-logs__speed-unit） */}
      <td className="db-table__num db-speed__col-p50 num" title={approxTitle}>
        {row.p50Tps != null ? (
          <>
            {tpsText(row.p50Tps)}
            <span className="db-logs__speed-unit">t/s</span>
          </>
        ) : (
          <Dash />
        )}
      </td>
      <td className="db-table__num db-speed__col-p90 num" title={approxTitle}>
        {row.p90Tps != null ? tpsText(row.p90Tps) : <Dash />}
      </td>
      <td className="db-table__num db-speed__col-ttft num">
        {ttftText(row.ttftAvgMs) ?? <Dash />}
      </td>
      <td className="db-table__num db-speed__col-dur num">
        {row.durAvgMs != null ? formatSeconds(row.durAvgMs) : <Dash />}
      </td>
      {/* 输入 / 输出：窗口内累计总量口径（列头 title 说明含缓存读），均值
          进数值格 title（每请求口径与总量口径并存的完整信息） */}
      <td
        className="db-table__num db-speed__col-in num"
        title={`${t("dashboard.speed.inTotalHint")} · ${t("dashboard.speed.avgPerRequest", {
          v: formatTokensLocale(row.inAvg, locale),
        })}`}
      >
        {formatTokensLocale(row.inTotal, locale)}
      </td>
      <td
        className="db-table__num db-speed__col-out num"
        title={`${t("dashboard.speed.outTotalHint")} · ${t("dashboard.speed.avgPerRequest", {
          v: formatTokensLocale(row.outAvg, locale),
        })}`}
      >
        {formatTokensLocale(row.outTotal, locale)}
      </td>
      {/* 精确值在 title（日志页惯例：紧凑值 + 千分位精确数悬停可见） */}
      <td
        className="db-table__num db-speed__col-requests num"
        title={exactCount(row.requests, locale)}
      >
        {formatCountLocale(row.requests, locale)}
      </td>
      <td
        className="db-table__num db-speed__col-samples num"
        title={
          row.speedSampleCount != null
            ? exactCount(row.speedSampleCount, locale)
            : t("speed.samplesNone")
        }
      >
        {row.speedSampleCount != null ? (
          formatCountLocale(row.speedSampleCount, locale)
        ) : (
          <Dash title={t("speed.samplesNone")} />
        )}
      </td>
      <td className="db-table__num db-speed__col-success num">
        {row.successRate != null ? formatPct(row.successRate) : <Dash />}
      </td>
    </tr>
  );
}

export function SpeedPage() {
  const { locale, t } = useI18n();
  const [days, setDays] = useState<SpeedDays>(loadDays);
  /** 自定义区间（非空 = 自定义模式，与三档预设互斥：选预设清空、应用
   *  自定义后预设值保留在 state 里作为「取消自定义」的回退档） */
  const [custom, setCustom] = useState<CustomRangeValue | null>(loadCustom);
  const [scope, setScope] = useState<ModelSpeedScope>(loadScope);
  const [stats, setStats] = useState<ModelSpeedStat[] | null>(null);
  const [remoteFailed, setRemoteFailed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  /** 来源筛选（后端来源名；空串 = 全部）：纯前端过滤，不持久化 */
  const [sourceFilter, setSourceFilter] = useState("");
  /** 排序键（默认 P50 降序）：组件 state，不写 localStorage */
  const [sort, setSort] = useState<{ key: SpeedSortKey; dir: "asc" | "desc" }>({
    key: "p50",
    dir: "desc",
  });
  /** 同步配置（null = 尚未读到）：初值取两页共享的模块级缓存（总览页 /
   *  日志页已读过时首帧即就绪，不发 IPC）。只用于「全部设备」可用性判断 */
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(
    () => peekSyncConfigCached()
  );
  /** 配置是否读毕（成功或失败）：首发射数挂在它之后——scope 偏好可能是
   *  「全部设备」，配置到位前按本机先查再全量重取会双取 + 闪数据
   *  （LogsPage 同款规避，见其文件头交互约定） */
  const [syncReady, setSyncReady] = useState(
    () => peekSyncConfigCached() != null
  );

  // 请求序号守卫（窄窗 P1 教训）：切档位 / 手动刷新与在途请求并发时，慢的
  // 旧响应后到不得覆盖新数据。then / catch / finally 三路径都比对最新序号，
  // 五个 setter（stats / remoteFailed / updatedAt / error / loading）全覆盖
  const requestSeq = useRef(0);

  // 同步配置读一次（共享缓存，失败静默按未启用处理）；本页不订阅实时变化
  useEffect(() => {
    let cancelled = false;
    readSyncConfigCached().then((config) => {
      if (!cancelled) {
        setSyncConfig(config);
        setSyncReady(true);
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;
  // 同步未启用时「全部设备」无从取数：固定本机口径（偏好保留，重开同步后
  // 自动恢复；总览页 effectiveScope 同款处理），分段按钮的「全部」禁用
  const effectiveScope: ModelSpeedScope = syncEnabled ? scope : "local";

  const load = useCallback(
    (resetError: boolean, silent = false) => {
      const seq = ++requestSeq.current;
      if (resetError) setError(null);
      // 静默刷新（30 秒定时）不置 loading：刷新按钮不转圈、表格与指标不压
      // 暗、不闪骨架（LogsPage 明文先例）
      if (!silent) setLoading(true);
      // 窗口四态折算：自定义区间固定透传（历史区间不按此刻推算）；预设档
      // 按当前时刻折算（今日零点 / 滚动窗口，与后端 days 口径一致——后端
      // 收到 from+to 后走区间模式并忽略 days，days 仅供窄窗兼容继续下发）
      const win = custom ?? daysWindow(days);
      getModelSpeed(DAYS_VALUE[days], effectiveScope, win)
        .then((resp) => {
          if (seq !== requestSeq.current) return;
          setStats(resp.stats);
          setRemoteFailed(resp.remoteFailed);
          setUpdatedAt(Date.now());
          setError(null);
        })
        .catch((e) => {
          if (seq !== requestSeq.current) return;
          // 静默刷新失败：保留已展示数据不打断（下一次刷新自愈），只有
          // 手动 / 切档位触发的查询失败才切整页错误态
          if (!silent) setError(String(e));
        })
        .finally(() => {
          if (seq !== requestSeq.current) return;
          // 无条件清 loading：静默路径从不置 true（不转圈不闪骨架），但任何
          // 「仍是最新的在途请求」完成都要把转圈态收掉——非静默请求被后续
          // 静默请求取代序号后，loading 若只由非静默路径清理会永久卡 true
          //（刷新按钮禁用、压暗不解除；LogsPage useLogsData 同款防御）
          setLoading(false);
        });
    },
    [days, custom, effectiveScope]
  );

  // 首帧 + 档位切换即重查（load 身份随 days / effectiveScope 变化）；首帧
  // 挂在同步配置读毕之后（见 syncReady 注释）
  useEffect(() => {
    if (syncReady) load(true);
  }, [load, syncReady]);

  // 30 秒自动刷新（静默）：document.hidden（主窗口关闭 = 隐藏）时跳过本轮，
  // 避免后台空转查询（LogsPage 同款约定）
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.hidden) return;
      load(false, true);
    }, 30_000);
    return () => window.clearInterval(timer);
  }, [load]);

  /** 表头点击：同列切方向，新列取该列默认方向 */
  function changeSort(key: SpeedSortKey) {
    setSort((prev) =>
      prev.key === key
        ? { key, dir: prev.dir === "desc" ? "asc" : "desc" }
        : { key, dir: defaultDir(key) }
    );
  }

  /** 切预设档：与自定义互斥（清空自定义，回退档生效）+ 即时持久化并重查
   *  （load 身份随 [days, custom, effectiveScope] 变化触发） */
  function changeDays(next: SpeedDays) {
    setDays(next);
    setCustom(null);
    savePref(RANGE_KEY, next);
    savePref(CUSTOM_KEY, "");
  }

  /** 应用自定义区间：进入自定义模式（预设高亮失效），JSON 持久化 */
  function changeCustom(next: CustomRangeValue) {
    setCustom(next);
    savePref(CUSTOM_KEY, JSON.stringify(next));
  }

  function changeScope(next: ModelSpeedScope) {
    setScope(next);
    savePref(SCOPE_KEY, next);
  }

  // 来源筛选：纯前端过滤（分组粒度几十级，不重查）
  const filtered = useMemo(
    () =>
      (stats ?? []).filter(
        (row) => sourceFilter === "" || row.source === sourceFilter
      ),
    [stats, sourceFilter]
  );
  const sorted = useMemo(
    () => [...filtered].sort((a, b) => compareRows(a, b, sort.key, sort.dir)),
    [filtered, sort]
  );

  // 速度名次（P1-4）：对当前筛选集按 P50 降序预计算稳定名次——排名列恒显
  // **速度名次**，切到其他列排序时名次乱序出现是有效信息（该行在速度维度
  // 的位置）；P50 缺失的行无名次（排在有名次行之后，名次列「—」）；第一名
  // 高亮只属于速度第一，不随排序漂移
  const speedRanking = useMemo(() => {
    const ordered = [...filtered]
      .filter((row) => row.p50Tps != null)
      .sort((a, b) => (b.p50Tps ?? 0) - (a.p50Tps ?? 0) || tiebreak(a, b));
    const ranks = new Map<string, number>();
    ordered.forEach((row, index) => ranks.set(rowKey(row), index + 1));
    return { ranks, topKey: ordered.length > 0 ? rowKey(ordered[0]) : null };
  }, [filtered]);

  // 汇总指标（当前筛选集上的总体概览）：
  //  - 最快 P50：取该行 speedQuality 联动 ≈（与窄窗 hero 同一口径处理）；
  //  - 平均 P50：跨口径混均，title 挂 speed.overviewMeanHint（不标 ≈——
  //    混合口径不是纯近似）；两个速度值都带小号 t/s 单位；
  //  - 总成功率：按 localRequests 加权（后端 successRate 分母只算本机行，
  //    权重用 requests 会在全部设备口径下被远端行稀释失真），title 说明
  const fastestRow = filtered.reduce<ModelSpeedStat | null>((best, row) => {
    if (row.p50Tps == null) return best;
    if (best?.p50Tps == null || row.p50Tps > best.p50Tps) return row;
    return best;
  }, null);
  const speedRows = filtered.filter((row) => row.p50Tps != null);
  const meanP50 =
    speedRows.length > 0
      ? speedRows.reduce((sum, row) => sum + (row.p50Tps ?? 0), 0) /
        speedRows.length
      : null;
  const successRows = filtered.filter((row) => row.successRate != null);
  const successBase = successRows.reduce(
    (sum, row) => sum + row.localRequests,
    0
  );
  const success =
    successBase > 0
      ? successRows.reduce(
          (sum, row) => sum + row.localRequests * (row.successRate ?? 0),
          0
        ) / successBase
      : null;
  const fastestApprox = fastestRow?.speedQuality === "request_average";
  const totalRequests = filtered.reduce((sum, row) => sum + row.requests, 0);

  const placeholder = loading && stats == null;
  const busy = loading && stats != null;
  // 查询失败（非静默错误态）指标行降级：与错误块形态统一显示「—」，不
  // 残留上一次口径的旧数值（切档位失败时旧窗口的数字仍挂在错误块上方会
  // 误导；LogsPage 失败清 summary 的降级先例）
  const metricsDegrade = error != null;

  const metrics: {
    key: string;
    label: string;
    value: string | null;
    title?: string;
    unit?: string;
  }[] = [
    // stats == null（冷缓存首帧：syncReady 未就绪、尚未发请求）时组数 /
    // 请求数给 null 走灰化「—」，与表格区骨架形态一致——空数组直接
    // String(0)/formatCountLocale(0) 会显示 "0"，两区形态割裂
    //（LogsPage summary==null 时 value 给 null 的降级先例）
    {
      key: "groups",
      label: t("speed.overviewModels"),
      value: stats == null ? null : String(filtered.length),
    },
    {
      key: "requests",
      label: t("speed.overviewRequests"),
      value: stats == null ? null : formatCountLocale(totalRequests, locale),
      title: t("dashboard.speed.metricRequestsHint"),
    },
    {
      key: "fastest",
      label: t("speed.fastest"),
      value:
        fastestRow?.p50Tps != null
          ? `${fastestApprox ? "≈" : ""}${formatTps(fastestRow.p50Tps)}`
          : null,
      title: fastestApprox ? t("speed.approxHint") : undefined,
      unit: "t/s",
    },
    {
      key: "mean",
      label: t("speed.overviewMean"),
      value: meanP50 == null ? null : formatTps(meanP50),
      title: t("speed.overviewMeanHint"),
      unit: "t/s",
    },
    {
      key: "success",
      label: t("speed.overviewSuccess"),
      value: success == null ? null : formatPct(success),
      title: t("dashboard.speed.metricSuccessHint"),
    },
  ];

  // 来源下拉：全部 + 7 来源（显示名与日志页同一份品牌元数据）
  const sourceOptions: DropdownOption[] = useMemo(
    () => [
      { value: "", label: t("dashboard.logs.sourceAll") },
      ...LOG_SOURCES.map((item) => ({
        value: item,
        label: sourceLabel(item),
      })),
    ],
    [t]
  );

  // 当前时间范围展示名（触发按钮与卡片副标题共用）：自定义时显示区间
  // 摘要「MM-DD ~ MM-DD」，预设档取词典（与总览 / 日志 / 对比页同款形态）
  const rangeLabel = custom
    ? formatRangeSummary(custom.fromMs, custom.toMs, locale)
    : t(DAYS_OPTIONS.find((item) => item.days === days)!.labelKey);
  // 卡片副标题：当前条件摘要（口径 · 范围 · 来源）
  const cardSub = [
    t(
      effectiveScope === "all"
        ? "dashboard.overview.scope.all"
        : "dashboard.overview.scope.local"
    ),
    rangeLabel,
    sourceFilter ? sourceLabel(sourceFilter) : t("dashboard.logs.sourceAll"),
  ].join(" · ");

  // 远端失败弱提示（db-hintbar 同款）：仅全部设备口径、有数据、无整体错误
  const showRemoteHint =
    effectiveScope === "all" && remoteFailed && stats != null && error == null;
  const isEmpty = !loading && error == null && stats != null && filtered.length === 0;

  return (
    <div className="db-page db-logs-page">
      <header className="db-page__header">
        <div>
          <h1 className="db-page__title">{t("dashboard.speed")}</h1>
          <p className="db-page__desc">{t("dashboard.speed.desc")}</p>
        </div>
        <div className="db-page__actions">
          {/* 时间范围：CustomRangeMenu（总览 / 日志 / 对比三页共用件）——
              触发按钮显示预设名或区间摘要（自定义生效时日历图标高亮），
              面板预设快捷行 = 今日 / 近 7 天 / 近 30 天（点选立即生效并收起、
              清空自定义），起止自然日历应用后进入自定义模式（历史区间固定，
              30 秒静默刷新重查同区间，不按此刻推算） */}
          <CustomRangeMenu
            value={custom}
            onChange={changeCustom}
            triggerLabel={rangeLabel}
            ariaLabel={t("dashboard.logs.rangeLabel")}
            presets={DAYS_OPTIONS.map(({ days: preset, labelKey }) => ({
              label: t(labelKey),
              active: custom == null && days === preset,
              onClick: () => changeDays(preset),
            }))}
          />
          {/* 数据范围：本机 / 全部设备。scope=local 不触网；同步未启用时
              「全部」禁用 + 说明（总览页 / 日志页同款先例），口径固定本机；
              远端失败走下方弱提示条（后端已按纯本机口径降级返回） */}
          <div className="db-seg" role="group" aria-label={t("dashboard.overview.scopeLabel")}>
            {(["local", "all"] as const).map((value) => (
              <button
                key={value}
                type="button"
                onClick={() => changeScope(value)}
                disabled={value === "all" && !syncEnabled}
                title={
                  value === "all" && !syncEnabled
                    ? t("dashboard.overview.syncDisabledHint")
                    : undefined
                }
                className={`db-seg__btn ${effectiveScope === value ? "db-seg__btn--active" : ""}`}
                aria-pressed={effectiveScope === value}
              >
                {t(
                  value === "local"
                    ? "dashboard.overview.scope.local"
                    : "dashboard.overview.scope.all"
                )}
              </button>
            ))}
          </div>
          {/* 手动刷新；title 顺带告知自动刷新节奏（speed.autoRefresh 词条） */}
          <button
            type="button"
            className="db-btn"
            onClick={() => load(true)}
            disabled={loading}
            title={t("speed.autoRefresh")}
          >
            <RefreshIcon className={`h-3.5 w-3.5 ${loading ? "db-spin" : ""}`} />
            {t("common.refresh")}
          </button>
          <span className="db-page__updated num">
            {updatedAt != null
              ? t("dashboard.overview.lastUpdated", { time: timeText(updatedAt) })
              : ""}
          </span>
        </div>
      </header>

      {/* 筛选行（在指标行之前：指标随筛选变化，先筛选后看数因果更清晰）：
          来源下拉（纯前端过滤；模型不筛——表格可排序自查） */}
      <div className="db-logs-filters">
        <DropdownMenu
          label={t("dashboard.logs.sourceLabel")}
          ariaLabel={t("dashboard.logs.sourceLabel")}
          options={sourceOptions}
          value={sourceFilter}
          onChange={setSourceFilter}
          active={sourceFilter !== ""}
        />
      </div>

      {/* 汇总指标行（复用 db-logs-metrics 样式族：首载骨架 / 刷新压暗 /
          无值灰化「—」与日志页同款降级形态）；速度值带小号 t/s 单位 */}
      <div
        className={`db-logs-metrics${busy ? " db-logs-metrics--busy" : ""}`}
        aria-busy={busy || undefined}
      >
        {metrics.map((cell) => (
          <div className="db-logs-metric" key={cell.key} title={cell.title}>
            <span className="db-logs-metric__label">{cell.label}</span>
            {placeholder ? (
              <Skeleton className="db-logs-metric__skeleton" />
            ) : (
              <span
                className={`db-logs-metric__value num${
                  metricsDegrade || cell.value == null
                    ? " db-logs-metric__value--dash"
                    : ""
                }`}
              >
                {metricsDegrade || cell.value == null ? "—" : cell.value}
                {!metricsDegrade && cell.value != null && cell.unit && (
                  <span className="db-logs__speed-unit">{cell.unit}</span>
                )}
              </span>
            )}
          </div>
        ))}
      </div>

      {/* 远端失败弱提示：全部设备口径下拉取失败（含未启用同步），后端已降级
          为纯本机数据，不清空不打断（对齐日志页 db-hintbar 惯例） */}
      {showRemoteHint && (
        <div className="db-hintbar">{t("speed.remoteUnavailable")}</div>
      )}

      {error != null ? (
        /* 错误态：单一提示块 + 重试（形态与日志页 db-errorstate 一致；只有
           手动 / 切档位触发的查询失败会走到这里，静默刷新失败不打断） */
        <div className="db-errorstate">
          <div className="db-errorstate__title">{t("speed.loadFailed")}</div>
          <div className="db-errorstate__reason">
            {t("speed.fail", { msg: error })}
          </div>
          <button
            type="button"
            className="db-btn"
            onClick={() => load(true)}
            disabled={loading}
          >
            <RefreshIcon className={`h-3.5 w-3.5 ${loading ? "db-spin" : ""}`} />
            {t("dashboard.logs.retry")}
          </button>
        </div>
      ) : stats == null ? (
        /* 首载骨架（表头复用真实表头保证形状一致） */
        <Card title={t("speed.ranking")} sub={cardSub} className="db-logs-card">
          <SpeedTableSkeleton sort={sort} onSort={changeSort} />
        </Card>
      ) : isEmpty ? (
        /* 空态：来源筛选专属文案 + 一键清除（LogsPage 模型空态同款先例）；
           通用空态说明窗口内无请求数据。均作为页面直接子元素（大窗字号
           覆盖只作用于 .db-page 直接子元素，与日志页同款） */
        sourceFilter !== "" ? (
          <EmptyState
            title={t("dashboard.speed.emptySourceTitle")}
            hint={t("dashboard.speed.emptySourceHint", {
              source: sourceLabel(sourceFilter),
            })}
            action={
              <button
                type="button"
                className="db-btn mt-1"
                onClick={() => setSourceFilter("")}
              >
                {t("dashboard.speed.clearSource")}
              </button>
            }
          />
        ) : (
          <EmptyState
            title={t("speed.empty")}
            hint={t("speed.emptyHint")}
            action={
              <button type="button" className="db-btn mt-1" onClick={() => load(true)}>
                {t("common.refresh")}
              </button>
            }
          />
        )
      ) : (
        <Card title={t("speed.ranking")} sub={cardSub} className="db-logs-card">
          <div
            className={`db-table-wrap${busy ? " db-table-wrap--busy" : ""}`}
            aria-busy={busy || undefined}
          >
            <table className="db-table db-speed-table">
              <SpeedTableHead sort={sort} onSort={changeSort} />
              <tbody>
                {sorted.map((row) => {
                  const key = rowKey(row);
                  return (
                    <SpeedTableRow
                      key={key}
                      row={row}
                      speedRank={speedRanking.ranks.get(key) ?? null}
                      isSpeedTop={speedRanking.topKey === key}
                    />
                  );
                })}
              </tbody>
            </table>
          </div>
        </Card>
      )}
    </div>
  );
}
