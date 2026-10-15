/**
 * 用量总览页（主窗口首页）：跨 Agent 的用量 / 花费 / 趋势汇总。
 *
 * 结构：页头（数据范围 + 时间范围触发按钮 + 刷新）→ 指标卡行 → 趋势图 →
 * 热力图 → Agent 统计表 → 模型用量表。范围与数据范围切换只重新聚合（本地来源
 * 一次取满 365 天，指标卡 / 趋势 / 两张表按 90 天切片、热力图铺 52 周），
 * 刷新按钮才重新取数；切换页面重进命中模块级缓存即刻渲染；不做自动轮询（一期）。
 * 时间范围预设档与自定义区间都收进页头单个触发按钮的弹出面板（预设档在面板
 * 顶部快捷行，点一下即生效；见 CustomRangeMenu）：自定义区间在 365 天取数
 * 窗口内按 [fromMs, toMs) 切片（derive.rangeKeys），同样不重新取数；
 * 热力图固定 52 周窗口，不受时间范围影响（保持现状）。
 */
import { useMemo, useState } from "react";
import { useI18n, type MessageKey } from "../i18n";
import { AlertBanner, EmptyState } from "../layout";
import { RefreshIcon } from "./icons";
import {
  CustomRangeMenu,
  formatRangeSummary,
  type CustomRangeValue,
} from "./CustomRangeMenu";
import { AgentTable } from "./overview/AgentTable";
import { HeatmapCard } from "./overview/HeatmapCard";
import { MetricCards } from "./overview/MetricCards";
import { ModelTable } from "./overview/ModelTable";
import { OverviewSkeleton } from "./overview/primitives";
import { TrendCard } from "./overview/TrendCard";
import {
  buildAgentRows,
  buildModelRows,
  currencyOf,
  heatSeries,
  scopeView,
  summarize,
  trendSeries,
} from "./overview/derive";
import { useOverviewData } from "./overview/useOverviewData";
import {
  WINDOW_DAYS,
  addLocalDays,
  startOfLocalDay,
  type OverviewRange,
  type OverviewScope,
} from "./overview/model";

/** 页头范围分段（顺序即展示顺序；默认近 30 天，兼顾趋势可读性与跨度） */
const RANGES: { range: OverviewRange; labelKey: MessageKey }[] = [
  { range: "today", labelKey: "dashboard.overview.range.today" },
  { range: "7d", labelKey: "dashboard.overview.range.7d" },
  { range: "30d", labelKey: "dashboard.overview.range.30d" },
  { range: "90d", labelKey: "dashboard.overview.range.90d" },
];

/** 页头数据范围分段（顺序即展示顺序；默认「全部设备」，与菜单栏标题口径对齐） */
const SCOPES: { scope: OverviewScope; labelKey: MessageKey }[] = [
  { scope: "local", labelKey: "dashboard.overview.scope.local" },
  { scope: "all", labelKey: "dashboard.overview.scope.all" },
];

/** 刷新时刻（本地时区 HH:mm:ss） */
function timeText(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export function OverviewPage() {
  const { locale, t } = useI18n();
  const {
    data,
    loading,
    error,
    lastUpdatedAt,
    reload,
    scope,
    setScope,
    syncEnabled,
    syncReady,
    remotePending,
  } = useOverviewData();
  const [range, setRange] = useState<OverviewRange>("30d");
  /** 自定义区间（非空 = 自定义模式，优先于预设档；选预设即清空） */
  const [customRange, setCustomRange] = useState<CustomRangeValue | null>(null);

  // 同步未启用时只能看本机口径（远端无从取数）：偏好仍保留，控件禁用并脚注说明
  const effectiveScope: OverviewScope = syncEnabled ? scope : "local";
  const remoteDegraded = data?.remoteDegraded ?? false;

  const currency = currencyOf(locale);
  const view = useMemo(() => {
    if (!data) return null;
    const scoped = scopeView(data, effectiveScope);
    return {
      summary: summarize(scoped, range, customRange),
      trend: trendSeries(scoped, range, customRange),
      heat: heatSeries(scoped),
      rows: buildAgentRows(scoped, range, customRange),
      modelRows: buildModelRows(scoped, range, customRange),
    };
  }, [data, range, customRange, effectiveScope]);

  // 时间范围展示名（趋势 / 表格卡副标题共用）：自定义时显示区间摘要
  const rangeLabel = customRange
    ? formatRangeSummary(customRange.fromMs, customRange.toMs, locale)
    : t(RANGES.find((item) => item.range === range)!.labelKey);
  // 自定义区间可选最早日期：取数窗口（365 天）起点——更早的日期没有数据，
  // 面板不放开（切范围只重新聚合，不会为此扩大取数窗口）
  const minMs = useMemo(
    () => startOfLocalDay(addLocalDays(Date.now(), -(WINDOW_DAYS - 1))),
    []
  );
  // 空态：没有任何 Agent 有用量或额度可展示（未启用 / 全部无数据）
  const isEmpty = view != null && view.rows.length === 0;
  // 硬错误：全部 Agent 取数失败（error 有值）且没有任何可渲染数据——
  // OverviewPage 拿到的 data 在致命失败时仍是「空壳」（perAgent 为空），
  // 因此这里用 error + 空行集判定，走专用错误态而不是采集引导空态
  const hardError =
    error != null && (view == null || view.rows.length === 0);

  return (
    <div className="db-page">
      <header className="db-page__header">
        <div>
          <h1 className="db-page__title">{t("dashboard.overview")}</h1>
          <p className="db-page__desc">{t("dashboard.overview.desc")}</p>
        </div>
        <div className="db-page__actions">
          <div
            className="db-seg"
            role="group"
            aria-label={t("dashboard.overview.scopeLabel")}
          >
            {SCOPES.map(({ scope: value, labelKey }) => (
              <button
                key={value}
                type="button"
                onClick={() => setScope(value)}
                disabled={!syncEnabled}
                title={
                  syncEnabled ? undefined : t("dashboard.overview.syncDisabledHint")
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
          {/* 时间范围：单个触发按钮 + 弹出面板（预设档收进面板顶部快捷行，
              可选日期下限 = 365 天取数窗口起点） */}
          <CustomRangeMenu
            value={customRange}
            onChange={setCustomRange}
            minMs={minMs}
            triggerLabel={rangeLabel}
            ariaLabel={t("dashboard.overview.rangeLabel")}
            presets={RANGES.map(({ range: presetRange, labelKey }) => ({
              label: t(labelKey),
              active: customRange == null && range === presetRange,
              onClick: () => {
                setRange(presetRange);
                setCustomRange(null);
              },
            }))}
          />
          <button
            type="button"
            className="db-btn"
            onClick={reload}
            disabled={loading}
          >
            <RefreshIcon className={`h-3.5 w-3.5 ${loading ? "db-spin" : ""}`} />
            {t("common.refresh")}
          </button>
          {/* 远端合并中：本机数据已可见，其他设备贡献仍在后台合并（弱标记，
              不阻塞界面；仅「全部设备」范围可能出现） */}
          {effectiveScope === "all" && remotePending && (
            <span className="db-page__merging">
              {t("dashboard.overview.remoteMerging")}
            </span>
          )}
          <span className="db-page__updated num">
            {lastUpdatedAt != null
              ? t("dashboard.overview.lastUpdated", { time: timeText(lastUpdatedAt) })
              : ""}
          </span>
        </div>
      </header>

      {/* 同步未启用：说明为何范围控件固定在「本机」（弱提示，不阻断展示）。
          等同步配置读完（或超时回退）再显示：同步配置与本地取数并行读，
          配置未返回前不闪提示 */}
      {data != null && syncReady && !syncEnabled && !hardError && (
        <div className="db-hintbar">
          {t("dashboard.overview.syncDisabledHint")}
        </div>
      )}
      {error && !hardError && (
        <AlertBanner type="error">
          {t("dashboard.overview.errorBanner", { error })}
        </AlertBanner>
      )}
      {data != null && data.failures.length > 0 && !hardError && (
        <AlertBanner type="warning">
          {t("dashboard.overview.partialFailures", {
            names: data.failures.join(" · "),
          })}
        </AlertBanner>
      )}
      {/* 「全部设备」下远端取数失败：未清数据、只是降级为本机口径（弱提示条） */}
      {effectiveScope === "all" && remoteDegraded && !error && !hardError && (
        <div className="db-hintbar">
          {t("dashboard.overview.remoteDegraded")}
        </div>
      )}

      {hardError ? (
        /* 专用错误态：全部 Agent 取数失败，此时没有任何可用数据，
           不复用「暂无用量数据」的采集引导空态（那会把失败说成没数据） */
        <div className="db-errorstate">
          <div className="db-errorstate__title">
            {t("dashboard.overview.errorTitle")}
          </div>
          <div className="db-errorstate__reason">
            {t("dashboard.overview.errorReason", { error })}
          </div>
          <button type="button" className="db-btn" onClick={reload}>
            <RefreshIcon className="h-3.5 w-3.5" />
            {t("dashboard.overview.retry")}
          </button>
        </div>
      ) : !view ? (
        loading ? (
          <OverviewSkeleton />
        ) : (
          <EmptyState
            title={t("dashboard.overview.emptyTitle")}
            hint={t("dashboard.overview.emptyHint")}
            action={
              <button type="button" className="db-btn mt-1" onClick={reload}>
                {t("dashboard.overview.retry")}
              </button>
            }
          />
        )
      ) : isEmpty ? (
        <EmptyState
          title={t("dashboard.overview.emptyTitle")}
          hint={t("dashboard.overview.emptyHint")}
          action={
            <button type="button" className="db-btn mt-1" onClick={reload}>
              {t("dashboard.overview.retry")}
            </button>
          }
        />
      ) : (
        <>
          <MetricCards
            summary={view.summary}
            currency={currency}
            /* 自定义区间按非今日处理（范围内 ≠ 今日，第 4 卡仍展示今日 Token） */
            range={customRange ? "custom" : range}
            /* 自定义范围下把区间摘要标在「范围内 Token」卡上（预设档不显示，
               卡片标题保持原有简洁） */
            rangeLabel={customRange ? rangeLabel : undefined}
          />
          <TrendCard
            series={view.trend}
            currency={currency}
            rangeLabel={rangeLabel}
          />
          <HeatmapCard series={view.heat} />
          <AgentTable
            rows={view.rows}
            currency={currency}
            rangeLabel={customRange ? rangeLabel : undefined}
          />
          {/* 模型用量表：与 Agent 表同一份范围口径（范围内切片），成本按本机
              价格表折算（Cursor 用其后端成本），默认折叠前 10 行 */}
          <ModelTable
            rows={view.modelRows}
            currency={currency}
            rangeLabel={customRange ? rangeLabel : undefined}
          />
          {/* 口径脚注：本机成本由后端按定价表 × 汇率折算，远端成本用同一份
              价格表在前端折算（与菜单栏标题同源），非账单金额 */}
          <p className="db-page__footnote">
            {t("dashboard.overview.costFootnote")}
          </p>
        </>
      )}
    </div>
  );
}
