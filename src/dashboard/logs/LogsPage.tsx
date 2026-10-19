/**
 * 请求日志页（主窗口第四页）：7 个本地来源的请求级明细，按时间倒序分页；
 * 数据范围「全部设备」时后端再并入同步服务端上其他设备的明细（远端行带
 * device 字段，耗时 / 首字 / 速度恒为「—」）。
 *
 * 结构：页头（数据范围分段 + 时间范围触发按钮 + 自动刷新间隔 + 刷新）→ 筛选行
 * （来源 / 模型下拉，模型选项带请求条数）→ 汇总指标行（总请求数 / Token 总量 /
 * 输入 / 输出 / 总成本 / 缓存命中率，当前筛选条件下的**全范围**聚合）→
 * 明细表（10 列）→ 分页条 → 详情抽屉。
 *
 * 版面：本页是唯一「不整页滚动」的页（.db-logs-page：flex 列 + 内部滚动），
 * 表格卡 flex:1、表体 .db-table-wrap 纵向滚动 + sticky 表头，分页条固定卡底
 * ——长列表不会把分页条推出屏幕（900×640 最小窗口同样可用）。
 *
 * 数据事实：Cursor 没有本地逐条库（只有远端接口的日粒度汇总），本页不含它
 * （「全部设备」口径下远端 cursor 明细同样不进表格，与本地口径一致）；数据
 * 范围分段与总览页共用同一份偏好（localStorage 键 zbar-dashboard-scope），
 * 未启用多设备同步时控件禁用、固定本机口径；未安装 / 未用过的来源由后端按
 * 0 条返回，页面静默显示空态，不报错。
 *
 * 交互约定：
 *  - 筛选（数据范围 / 来源 / 模型 / 范围）变化即回到第 1 页——否则会停在
 *    「第 3 页但只有 1 页数据」的空结果上；
 *  - 首发射数与同步配置读取并行：配置读毕（成功或失败）才发首个请求，避免
 *    「先按本机查、配置到达后又按全部设备全量重取」的双取（与总览页同款
 *    规避，等待期骨架本就在展示）；
 *  - 时间范围收进页头单个触发按钮（预设档在弹出面板顶部快捷行，点一下即生效
 *    并收起；见 CustomRangeMenu）：自定义与预设互斥，选预设即清自定义；
 *    自定义区间（起止自然日）直接作为后端查询窗口，不按「此刻」推算；
 *    本页不限制最早日期（后端存有全部历史记录）；
 *  - 模型筛选**不自动清空**：换来源 / 范围后该模型可能没有请求，此时空态给出
 *    说明与「清除模型筛选」按钮，选中值保留在控件里由用户决定；
 *  - 页码文案与分页按钮基于「已生效页码」（rowsPage，即当前表格里那批数据所属
 *    的页），翻页等待期不会出现「第 3 页」配第 2 页数据的错位；
 *  - 「刷新」按钮重取明细 + 清单（不做模块级缓存，见 useLogsData）；
 *  - 页头「自动刷新」下拉选间隔（关闭 / 5 / 10 / 30 / 60 秒，默认 30 秒，
 *    localStorage 键 zbar-logs-refresh-ms 持久化）：间隔 > 0 时定时走
 *    useLogsData 的**静默**刷新（不置 loading——刷新按钮不转圈、表格与指标
 *    不压暗、不闪骨架），保持当前筛选与页码；document.hidden（主窗口被关闭
 *    隐藏）时跳过本次，避免后台空转查询；
 *  - 远端不可达 / 未启用同步 / 旧服务端 404（scope=all 时）：后端自动降级为
 *    纯本机口径（failed_sources 里给 "remote"），有数据才用一行 .db-hintbar
 *    弱提示，不清空表格、不打断。
 */
import { useEffect, useMemo, useState } from "react";
import { useI18n, type MessageKey } from "../../i18n";
import { EmptyState } from "../../layout";
import { DropdownMenu, type DropdownOption } from "../../DropdownMenu";
import type { SyncConfig } from "../../types";
import {
  formatCost,
  formatCountLocale,
  formatPct,
  formatTokensLocale,
} from "../../format";
import { RefreshIcon } from "../icons";
import { Card, Skeleton } from "../primitives";
import {
  CustomRangeMenu,
  formatRangeSummary,
  type CustomRangeValue,
} from "../CustomRangeMenu";
import { currencyOf, costOf } from "../overview/derive";
import {
  loadOverviewScope,
  saveOverviewScope,
  type OverviewScope,
} from "../overview/model";
import {
  peekSyncConfigCached,
  readSyncConfigCached,
} from "../syncConfig";
import { LogTable, LogTableSkeleton } from "./LogTable";
import { RequestDetailDrawer } from "./RequestDetailDrawer";
import { useLogsData } from "./useLogsData";
import {
  LOG_PAGE_SIZE,
  LOG_SOURCES,
  LOGS_RANGES,
  exactCount,
  sourceLabel,
  type LogsRange,
} from "./model";
import type { Currency, RequestLogRow, RequestLogSummary } from "../../types";

/** 刷新时刻（本地时区 HH:mm:ss；与总览页页头同款） */
function timeText(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/**
 * 页头数据范围分段（顺序即展示顺序）：本机 / 全部设备。与总览页共用同一份
 * 偏好（loadOverviewScope / saveOverviewScope，localStorage 键
 * zbar-dashboard-scope）：任一页切换，另一页下次进入即生效；词条复用
 * dashboard.overview.scope.*（同款分段控件文案必须一致）。
 */
const SCOPES: { scope: OverviewScope; labelKey: MessageKey }[] = [
  { scope: "local", labelKey: "dashboard.overview.scope.local" },
  { scope: "all", labelKey: "dashboard.overview.scope.all" },
];

/**
 * 自动刷新间隔档位（页头下拉）：关闭 / 5 / 10 / 30 / 60 秒。取值即毫秒数
 * （0 = 关闭），数组顺序即下拉展示顺序；默认 30 秒。
 */
const REFRESH_MS_OPTIONS = [0, 5000, 10000, 30000, 60000];
const REFRESH_MS_DEFAULT = 30000;

/** 间隔偏好持久化键（与热力图模式 zbar-heat-mode 同款：只记用户选择，不参与
 *  取数语义；重启后停留在上次选的档位，不被重置回默认） */
const REFRESH_MS_KEY = "zbar-logs-refresh-ms";

/** 读取间隔偏好：缺省 / 空串 / 非法值一律回退 30 秒（注意 "0" 是合法档位） */
function loadRefreshMs(): number {
  try {
    const saved = localStorage.getItem(REFRESH_MS_KEY);
    if (saved) {
      const ms = Number(saved);
      if (REFRESH_MS_OPTIONS.includes(ms)) return ms;
    }
  } catch {
    /* 存储不可用：回退默认间隔 */
  }
  return REFRESH_MS_DEFAULT;
}

/** 写入间隔偏好（失败静默：记忆仅锦上添花，不影响刷新行为） */
function saveRefreshMs(ms: number): void {
  try {
    localStorage.setItem(REFRESH_MS_KEY, String(ms));
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等 */
  }
}

/**
 * 汇总指标行：总请求数 / Token 总量 / 输入 / 输出 / 总成本 / 缓存命中率
 * （当前筛选条件下的全范围聚合，后端一次返回，翻页不变）。
 *
 * 状态降级：
 *  - 首载（无旧值）→ 六个骨架块（形状与真实值同宽，不跳版）；
 *  - 刷新 / 翻页（有旧值）→ 数值压暗（db-logs-metrics--busy），不闪空；
 *  - 错误 / 无数据（summary 为 null）→ 全部显示灰化「—」；
 *  - token 三格：有请求即有值，0 是有效值（与请求数格一致直接显示 0）；
 *  - 命中率 null（分母为 0）→ 「—」；总成本无任何价格数据（折算为 0）→ 「—」
 *    并给 title 说明（与表格「未定价」同一套口径）。
 *
 * token 口径：总量 = total_tokens（输入 + 输出 + 缓存等全部口径的总量）；
 * 输入的跨源口径差异（zcode / codex / dsh / opencode / minimaxcode 的
 * input 已含缓存读，claude / kimi 不含缓存读写）写进 title 提示，详见
 * src-tauri/src/request_logs.rs 文件头「汇总指标」注释。数值格用
 * formatTokensLocale（与表格逐行同一格式化函数）；title = 精确值
 * （exactCount）· 口径说明（与表格缓存列 cacheCellTitle「精确值在 title」
 * 同一先例），无值（「—」态）只留口径说明。
 */
function LogsMetrics({
  summary,
  loading,
  currency,
}: {
  summary: RequestLogSummary | null;
  loading: boolean;
  currency: Currency;
}) {
  const { locale, t } = useI18n();
  const placeholder = loading && summary == null;
  const busy = loading && summary != null;
  const cost = summary ? costOf(summary.cost_usd, summary.cost_cny, currency) : 0;
  const costText = summary != null && cost > 0 ? formatCost(cost, currency) : null;
  const hitRate = summary?.cache_hit_rate ?? null;
  // token 三格 title：有值 = 精确值（exactCount）· 口径说明；「—」态退回
  // 纯口径说明（hint 词条本身不含数值，与表格缓存列「精确值在 title」同一先例）
  const tokenTitle = (hintKey: MessageKey, n: number | null | undefined): string =>
    n == null ? t(hintKey) : `${exactCount(n, locale)} · ${t(hintKey)}`;
  // value = null → 灰化「—」；title 只在该格确实没有值（或有口径说明）时给
  const cells: { key: string; label: string; value: string | null; title?: string }[] = [
    {
      key: "requests",
      label: t("dashboard.logs.metricsRequests"),
      value: summary ? exactCount(summary.requests, locale) : null,
    },
    {
      key: "tokensTotal",
      label: t("dashboard.logs.metricsTokensTotal"),
      value:
        summary != null ? formatTokensLocale(summary.total_tokens, locale) : null,
      title: tokenTitle(
        "dashboard.logs.metricsTokensTotalHint",
        summary?.total_tokens
      ),
    },
    {
      key: "inputTokens",
      label: t("dashboard.logs.metricsInputTokens"),
      value:
        summary != null ? formatTokensLocale(summary.input_tokens, locale) : null,
      title: tokenTitle(
        "dashboard.logs.metricsInputTokensHint",
        summary?.input_tokens
      ),
    },
    {
      key: "outputTokens",
      label: t("dashboard.logs.metricsOutputTokens"),
      value:
        summary != null ? formatTokensLocale(summary.output_tokens, locale) : null,
      title: tokenTitle(
        "dashboard.logs.metricsOutputTokensHint",
        summary?.output_tokens
      ),
    },
    {
      key: "cost",
      label: t("dashboard.logs.metricsCost"),
      value: costText,
      // 展示「—」时才解释（有值时 hover 提示「没有可计价数据」会误导）
      title: costText == null ? t("dashboard.logs.metricsCostHint") : undefined,
    },
    {
      key: "cache",
      label: t("dashboard.logs.metricsCacheHit"),
      value: hitRate != null ? formatPct(hitRate) : null,
      title: t("dashboard.logs.metricsCacheHint"),
    },
  ];
  return (
    <div
      className={`db-logs-metrics${busy ? " db-logs-metrics--busy" : ""}`}
      aria-busy={busy || undefined}
    >
      {cells.map((cell) => (
        <div className="db-logs-metric" key={cell.key} title={cell.title}>
          <span className="db-logs-metric__label">{cell.label}</span>
          {placeholder ? (
            <Skeleton className="db-logs-metric__skeleton" />
          ) : (
            <span
              className={`db-logs-metric__value num${
                cell.value == null ? " db-logs-metric__value--dash" : ""
              }`}
            >
              {cell.value ?? "—"}
            </span>
          )}
        </div>
      ))}
    </div>
  );
}

export function LogsPage() {
  const { locale, t } = useI18n();
  const [range, setRange] = useState<LogsRange>("today");
  /** 自定义区间（非空 = 自定义模式，与五档预设互斥：预设档选中时清空） */
  const [customRange, setCustomRange] = useState<CustomRangeValue | null>(null);
  /** 后端来源名；空串 = 全部来源 */
  const [source, setSource] = useState("");
  /** 模型 id；空串 = 不筛（换来源 / 范围不自动清空，失效时由空态提示处理） */
  const [model, setModel] = useState("");
  /** 数据范围偏好（与总览页共用 zbar-dashboard-scope）；懒初始化只读一次，
   *  变更即时写入（与总览页 loadOverviewScope / saveOverviewScope 同一套） */
  const [scope, setScope] = useState<OverviewScope>(() => loadOverviewScope());
  /** 同步配置（null = 尚未读到；读到前范围控件短暂禁用，与总览页同款）。
   *  初值取两页共享的模块级缓存：总览页已读过时本页首帧即为就绪态，
   *  不再多等一次 IPC */
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(
    () => peekSyncConfigCached()
  );
  /** 同步配置是否已读毕（成功或失败）：首发射数挂起在它之后——scope 偏好
   *  可能是「全部设备」（缺省即 all），配置到位前按本机先查一遍再全量重取
   *  会双取 + 闪数据（总览页 useOverviewData 的反 pattern 注记，同样采用
   *  「配置与取数并行」规避）。共享缓存命中时初值即为 true（不发 IPC） */
  const [syncReady, setSyncReady] = useState(
    () => peekSyncConfigCached() != null
  );
  /** 请求页码（1 基，页面语义） */
  const [page, setPage] = useState(1);
  /** 详情抽屉当前行（null = 关闭） */
  const [active, setActive] = useState<RequestLogRow | null>(null);
  /** 自动刷新间隔（毫秒，0 = 关闭）；偏好懒初始化，只在挂载时读一次 */
  const [refreshMs, setRefreshMs] = useState<number>(() => loadRefreshMs());

  // 同步配置读一次（走两页共享缓存：总览页已读过则立即 resolve、不发 IPC；
  // 未命中发 IPC，失败 / 超时按未启用处理并置 ready——口径固定本机，不卡
  // 骨架）。本页不订阅同步面板的实时变化：开关同步后重进本页，命中缓存时
  // 按上次配置渲染，下一次总览页取数（refresh 读取）会刷新共享缓存
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
  // 同步未启用时只能看本机口径（远端无从取数）：偏好仍保留，控件禁用并提示
  const effectiveScope: OverviewScope = syncEnabled ? scope : "local";

  const {
    rows,
    rowsPage,
    total,
    summary,
    failedSources,
    loading,
    error,
    lastUpdatedAt,
    models,
    reload,
  } = useLogsData({
    range,
    custom: customRange,
    source,
    model,
    scope: effectiveScope,
    ready: syncReady,
    page,
  });

  const currency = currencyOf(locale);
  const pages = Math.max(1, Math.ceil(total / LOG_PAGE_SIZE));
  // 表内仍是旧行时的取数中状态：压暗 + 不可点开详情（见 LogTable 的 busy）
  const busy = loading && rows.length > 0;
  // 时间列「今天」判定与「上次刷新」共用的基准时刻：取最近一次成功取数时刻
  // （取数后必然重渲染，跨日时也会随下一次刷新自动修正）
  const nowMs = lastUpdatedAt ?? Date.now();

  // 筛选变化一律回第 1 页（见文件头交互约定）；模型选中值保留。
  // 预设档与自定义互斥：选预设清自定义，应用自定义则自定义生效（预设值保留
  // 在 state 里，仅作为「取消自定义」后的回退档）
  function changeRange(next: LogsRange) {
    setRange(next);
    setCustomRange(null);
    setPage(1);
  }
  function changeCustom(next: CustomRangeValue) {
    setCustomRange(next);
    setPage(1);
  }
  function changeSource(next: string) {
    setSource(next);
    setPage(1);
  }
  function changeModel(next: string) {
    setModel(next);
    setPage(1);
  }
  /** 切换数据范围：与总览页共用同一份偏好（即时持久化），并回第 1 页 */
  function changeScope(next: OverviewScope) {
    setScope(next);
    saveOverviewScope(next);
    setPage(1);
  }
  function clearModel() {
    setModel("");
    setPage(1);
  }

  /** 切换自动刷新间隔：立即生效（定时器由下方 effect 按新间隔重建）并持久化 */
  function changeRefreshMs(next: number) {
    if (!REFRESH_MS_OPTIONS.includes(next)) return;
    setRefreshMs(next);
    saveRefreshMs(next);
  }

  // 页码越界回收：数据变少（刷新 / 清筛选后总条数下降）时把页码收进范围。
  // 判定基于「已生效页码」与已到手的 total，避免用待请求的页码做递归。
  useEffect(() => {
    if (!loading && total > 0 && rowsPage > pages) setPage(pages);
  }, [loading, total, rowsPage, pages]);

  // 自动刷新：间隔 > 0 时定时重取「当前筛选 + 当前页码」的明细。
  //  - 走静默刷新（reload({silent:true})）：不置 loading——刷新按钮不转圈、
  //    表格与指标不压暗，也不会闪骨架；
  //  - 主窗口关闭只是 hide（WebView 仍存活），document.hidden 时跳过本次，
  //    避免后台空转查询（重新显示后按剩余间隔继续）；
  //  - 间隔变化即重建定时器（依赖 refreshMs），切「关闭」由 cleanup 清掉，
  //    页面卸载（切到其他页）时同样清理。
  useEffect(() => {
    if (refreshMs <= 0) return;
    const timer = window.setInterval(() => {
      if (document.hidden) return;
      reload({ silent: true });
    }, refreshMs);
    return () => window.clearInterval(timer);
  }, [refreshMs, reload]);

  // 间隔下拉选项：关闭 / 5 / 10 / 30 / 60 秒（选项顺序即 REFRESH_MS_OPTIONS；
  // 选中项由 value 匹配，组件自动带 ✓）
  const refreshOptions: DropdownOption[] = useMemo(
    () =>
      REFRESH_MS_OPTIONS.map((ms) => ({
        value: String(ms),
        label:
          ms === 0
            ? t("dashboard.logs.autoRefreshOff")
            : t("dashboard.logs.autoRefreshSec", { n: ms / 1000 }),
      })),
    [t]
  );
  // 当前档位文案（触发器与 title / aria 共用；值必在选项里，兜底为空串）
  const refreshLabel =
    refreshOptions.find((option) => option.value === String(refreshMs))?.label ??
    "";

  // 来源下拉：全部 + 7 个来源（显示名与总览页 agentLabel 同一份品牌元数据）
  const sourceOptions: DropdownOption[] = useMemo(
    () => [
      { value: "", label: t("dashboard.logs.sourceAll") },
      ...LOG_SOURCES.map((item) => ({ value: item, label: sourceLabel(item) })),
    ],
    [t]
  );

  // 模型下拉：全部 + 清单（带条数）；选中项若尚未进清单（清单还在取 / 该模型
  // 在当前条件下无请求），先补进选项，保证触发器不显示空串
  const modelOptions: DropdownOption[] = useMemo(() => {
    const options: DropdownOption[] = [
      { value: "", label: t("dashboard.logs.modelAll") },
      ...(models ?? []).map((item) => ({
        value: item.model_id,
        label: `${item.model_id} · ${formatCountLocale(item.count, locale)}`,
      })),
    ];
    if (model !== "" && !options.some((option) => option.value === model)) {
      options.push({ value: model, label: model });
    }
    return options;
  }, [models, model, t, locale]);

  // 当前时间范围展示名（分段高亮与卡片副标题共用）：自定义时显示区间摘要
  // 「MM-DD ~ MM-DD」，预设档取词典（前四项与总览页同一套 key）
  const rangeLabel = customRange
    ? formatRangeSummary(customRange.fromMs, customRange.toMs, locale)
    : t(LOGS_RANGES.find((item) => item.range === range)!.labelKey);

  // 卡片副标题：当前条件摘要（数据范围文案随口径变化 + 点击行的说明，
  // 行内不再逐行提示）
  const cardSub = [
    rangeLabel,
    source ? sourceLabel(source) : t("dashboard.logs.sourceAll"),
    model || t("dashboard.logs.modelAll"),
    effectiveScope === "all"
      ? t("dashboard.overview.scope.all")
      : t("dashboard.logs.localOnly"),
    t("dashboard.logs.rowHint"),
  ].join(" · ");

  const isEmpty = !loading && error == null && total === 0;
  const filteredByModel = model !== "";
  // 部分来源取数失败（库读失败 / 表缺失）但仍有数据：弱提示，不清空、不报错
  const showFailedHint =
    error == null && total > 0 && failedSources.length > 0;

  /** 分页条：页码文案 / 按钮均以「已生效页码」为准（翻页等待期不领先数据） */
  const pager = (
    <div className="db-pager">
      <button
        type="button"
        className="db-btn db-btn--sm"
        disabled={rowsPage <= 1 || loading}
        onClick={() => setPage(Math.max(1, rowsPage - 1))}
      >
        ‹ {t("dashboard.logs.pagePrev")}
      </button>
      <span className="db-pager__info num">
        {t("dashboard.logs.pageInfo", { page: rowsPage, total: pages })}
      </span>
      <button
        type="button"
        className="db-btn db-btn--sm"
        disabled={rowsPage >= pages || loading}
        onClick={() => setPage(Math.min(pages, rowsPage + 1))}
      >
        {t("dashboard.logs.pageNext")} ›
      </button>
      <span className="db-pager__total num">
        {t("dashboard.logs.pageTotal", {
          n: formatCountLocale(total, locale),
        })}
      </span>
    </div>
  );

  return (
    <div className="db-page db-logs-page">
      <header className="db-page__header">
        <div>
          <h1 className="db-page__title">{t("dashboard.logs")}</h1>
          <p className="db-page__desc">{t("dashboard.logs.desc")}</p>
        </div>
        <div className="db-page__actions">
          {/* 数据范围：本机 / 全部设备（与总览页同一套分段控件与偏好键；
              未启用多设备同步时禁用并提示原因） */}
          <div
            className="db-seg"
            role="group"
            aria-label={t("dashboard.overview.scopeLabel")}
          >
            {SCOPES.map(({ scope: value, labelKey }) => (
              <button
                key={value}
                type="button"
                onClick={() => changeScope(value)}
                disabled={!syncEnabled}
                title={
                  syncEnabled
                    ? undefined
                    : t("dashboard.overview.syncDisabledHint")
                }
                className={`db-seg__btn ${
                  effectiveScope === value ? "db-seg__btn--active" : ""
                }`}
                aria-pressed={effectiveScope === value}
              >
                {t(labelKey)}
              </button>
            ))}
          </div>
          {/* 时间范围：单个触发按钮 + 弹出面板（五档预设收进面板顶部快捷行；
              本页不限制最早日期——后端存有全部历史请求记录） */}
          <CustomRangeMenu
            value={customRange}
            onChange={changeCustom}
            triggerLabel={rangeLabel}
            ariaLabel={t("dashboard.logs.rangeLabel")}
            presets={LOGS_RANGES.map(({ range: presetRange, labelKey }) => ({
              label: t(labelKey),
              active: customRange == null && range === presetRange,
              onClick: () => changeRange(presetRange),
            }))}
          />
          {/* 自动刷新间隔：触发按钮显示「自动刷新 + 当前档」，选择即生效并持久化
              （长文案 / 完整语义放 title 与 aria-label，页头保持紧凑） */}
          <DropdownMenu
            label={t("dashboard.logs.autoRefresh")}
            ariaLabel={t("dashboard.logs.autoRefreshHint", { value: refreshLabel })}
            title={t("dashboard.logs.autoRefreshHint", { value: refreshLabel })}
            options={refreshOptions}
            value={String(refreshMs)}
            onChange={(next) => changeRefreshMs(Number(next))}
          />
          <button
            type="button"
            className="db-btn"
            onClick={() => reload()}
            disabled={loading}
          >
            <RefreshIcon className={`h-3.5 w-3.5 ${loading ? "db-spin" : ""}`} />
            {t("common.refresh")}
          </button>
          <span className="db-page__updated num">
            {lastUpdatedAt != null
              ? t("dashboard.overview.lastUpdated", { time: timeText(lastUpdatedAt) })
              : ""}
          </span>
        </div>
      </header>

      {/* 筛选行：来源 + 模型（模型选项带条数，随来源 / 范围刷新） */}
      <div className="db-logs-filters">
        <DropdownMenu
          label={t("dashboard.logs.sourceLabel")}
          ariaLabel={t("dashboard.logs.sourceLabel")}
          options={sourceOptions}
          value={source}
          onChange={changeSource}
        />
        <DropdownMenu
          label={t("dashboard.logs.modelLabel")}
          ariaLabel={t("dashboard.logs.modelLabel")}
          options={modelOptions}
          value={model}
          onChange={changeModel}
        />
      </div>

      {/* 汇总指标行：当前筛选条件下的全范围聚合（与表格同一次请求返回） */}
      <LogsMetrics summary={summary} loading={loading} currency={currency} />

      {/* 单源 / 远端取数失败弱提示（对齐总览页 partialFailures 惯例：有数据才
          提示，不打断；"remote" = 「全部设备」口径下远端一路未取到，已降级为
          纯本机口径展示） */}
      {showFailedHint && (
        <div className="db-hintbar">
          {t("dashboard.logs.partialFailures", {
            names: failedSources
              .map((item) =>
                item === "remote"
                  ? t("dashboard.logs.remoteSourceName")
                  : sourceLabel(item)
              )
              .join(" · "),
          })}
        </div>
      )}

      {error != null ? (
        /* 错误态：单一提示块（原因 + 重试），不再叠加 AlertBanner 重复同一句
           错误；形态与总览页的 .db-errorstate 一致 */
        <div className="db-errorstate">
          <div className="db-errorstate__title">
            {t("dashboard.logs.errorTitle")}
          </div>
          <div className="db-errorstate__reason">
            {t("dashboard.logs.errorReason", { error })}
          </div>
          <button
            type="button"
            className="db-btn"
            onClick={() => reload()}
            disabled={loading}
          >
            <RefreshIcon className={`h-3.5 w-3.5 ${loading ? "db-spin" : ""}`} />
            {t("dashboard.logs.retry")}
          </button>
        </div>
      ) : loading && rows.length === 0 ? (
        /* 首载 / 筛选切换后无旧数据：表格骨架（行数与每页条数一致） */
        <Card
          title={t("dashboard.logs.tableTitle")}
          sub={cardSub}
          className="db-logs-card"
        >
          <LogTableSkeleton />
        </Card>
      ) : isEmpty ? (
        /* 空态与总览页同款：作为页面的直接子元素（大窗字号覆盖只作用于
           .db-page 的直接子元素），不套卡片 */
        filteredByModel ? (
          <EmptyState
            title={t("dashboard.logs.emptyModelTitle")}
            hint={t("dashboard.logs.emptyModelHint", { model })}
            action={
              <button type="button" className="db-btn mt-1" onClick={clearModel}>
                {t("dashboard.logs.clearModel")}
              </button>
            }
          />
        ) : (
          <EmptyState
            title={t("dashboard.logs.emptyTitle")}
            hint={t("dashboard.logs.emptyHint")}
            action={
              <button
                type="button"
                className="db-btn mt-1"
                onClick={() => reload()}
              >
                {t("dashboard.logs.retry")}
              </button>
            }
          />
        )
      ) : rows.length === 0 ? (
        /* total > 0 但当前页没有行：深页 / 页码越界回收帧 / 归并切片的边界，
           给一行行内提示（分页条保留，可直接退回上一页） */
        <Card
          title={t("dashboard.logs.tableTitle")}
          sub={cardSub}
          className="db-logs-card"
        >
          <div className="db-empty">{t("dashboard.logs.pageEmptyHint")}</div>
          {pager}
        </Card>
      ) : (
        <Card
          title={t("dashboard.logs.tableTitle")}
          sub={cardSub}
          className="db-logs-card"
        >
          <LogTable
            rows={rows}
            nowMs={nowMs}
            currency={currency}
            busy={busy}
            onOpen={setActive}
          />
          {pager}
        </Card>
      )}

      {active && (
        <RequestDetailDrawer
          row={active}
          currency={currency}
          onClose={() => setActive(null)}
        />
      )}
    </div>
  );
}
