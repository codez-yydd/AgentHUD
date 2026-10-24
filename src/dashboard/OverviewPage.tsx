/**
 * 用量总览页（主窗口首页）：跨 Agent 的用量 / 花费 / 趋势汇总。
 *
 * 结构：页头（数据范围 + 时间范围触发按钮 + 刷新）→ 指标卡行 → 趋势图 →
 * 「热力图 + Agent 统计表」行（默认上下堆叠，仅大窗 ≥1340px 并排两列利用
 * 横向空间，见 .db-overview-duo）→ 模型用量表。范围与数据范围切换只重新聚合（本地来源
 * 一次取满 365 天，指标卡 / 趋势 / 两张表按 90 天切片、热力图铺 52 周），
 * 刷新按钮才重新取数；切换页面重进命中模块级缓存即刻渲染；不做自动轮询（一期）。
 * 时间范围预设档与自定义区间都收进页头单个触发按钮的弹出面板（预设档在面板
 * 顶部快捷行，点一下即生效；见 CustomRangeMenu）：自定义区间在 365 天取数
 * 窗口内按 [fromMs, toMs) 切片（derive.rangeKeys），同样不重新取数；
 * 热力图固定 52 周窗口，不受时间范围影响（保持现状）。
 * Agent 筛选（页头下拉）同理是纯视图切片（derive.agentView）：指标卡 / 趋势
 * 图 / 热力图 / 模型用量表收窄到所选 Agent，Agent 统计表保留跨 Agent 分解
 * 对比口径不跟随；空态 / 硬错误判定也用未过滤口径，不受筛选影响。
 */
import { useMemo, useState } from "react";
import { useI18n, type MessageKey } from "../i18n";
import { AlertBanner, EmptyState } from "../layout";
import { RefreshIcon } from "./icons";
import { DropdownMenu, type DropdownOption } from "../DropdownMenu";
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
  agentView,
  buildAgentRows,
  buildModelRows,
  heatSeries,
  scopeView,
  summarize,
  trendSeries,
} from "./overview/derive";
import { useCurrencyPref } from "./settings/currencyPref";
import { useOverviewData } from "./overview/useOverviewData";
import {
  USAGE_AGENT_IDS,
  WINDOW_DAYS,
  addLocalDays,
  agentLabel,
  loadOverviewAgentFilter,
  saveOverviewAgentFilter,
  startOfLocalDay,
  type OverviewAgentFilter,
  type OverviewRange,
  type OverviewScope,
} from "./overview/model";

/** 页头范围分段（顺序即展示顺序；默认「今日」——当天用量是最常看的口径，
 *  更长跨度由其余预设档与自定义区间覆盖） */
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
  const [range, setRange] = useState<OverviewRange>("today");
  /** 自定义区间（非空 = 自定义模式，优先于预设档；选预设即清空） */
  const [customRange, setCustomRange] = useState<CustomRangeValue | null>(null);
  /** Agent 筛选（"all" 或具体 agent id）：偏好持久化，切换只重新聚合 */
  const [agentFilter, setAgentFilter] = useState<OverviewAgentFilter>(
    () => loadOverviewAgentFilter()
  );

  /** 切换 Agent 筛选：选项值必为 "all" 或 USAGE_AGENT_IDS 内的 id（见
      agentOptions），未知值直接忽略；与数据范围偏好同款即时持久化 */
  function changeAgentFilter(next: string) {
    if (next !== "all" && !(USAGE_AGENT_IDS as readonly string[]).includes(next)) {
      return;
    }
    const value = next as OverviewAgentFilter;
    setAgentFilter(value);
    saveOverviewAgentFilter(value);
  }

  // Agent 筛选下拉：全部 + 8 个 Agent（显示名与 Agent 表同一份品牌元数据）
  const agentOptions: DropdownOption[] = useMemo(
    () => [
      { value: "all", label: t("dashboard.overview.agent.all") },
      ...USAGE_AGENT_IDS.map((id) => ({ value: id, label: agentLabel(id) })),
    ],
    [t]
  );

  // 同步未启用时只能看本机口径（远端无从取数）：偏好仍保留，控件禁用并脚注说明
  const effectiveScope: OverviewScope = syncEnabled ? scope : "local";
  const remoteDegraded = data?.remoteDegraded ?? false;

  // 币种：读设置（默认跟随语言 = 升级前 currencyOf(locale) 同一口径；
  // 设置页手动指定后全页跟随）
  const { currency } = useCurrencyPref();
  const view = useMemo(() => {
    if (!data) return null;
    const scoped = scopeView(data, effectiveScope);
    // Agent 筛选视图切片：指标卡 / 趋势 / 热力图 / 模型表跟随；Agent 表保留
    // 跨 Agent 分解对比口径，仍取 scoped
    const filtered = agentView(scoped, agentFilter);
    return {
      summary: summarize(filtered, range, customRange),
      trend: trendSeries(filtered, range, customRange),
      heat: heatSeries(filtered),
      rows: buildAgentRows(scoped, range, customRange),
      modelRows: buildModelRows(filtered, range, customRange),
    };
  }, [data, range, customRange, effectiveScope, agentFilter]);

  // 时间范围展示名（趋势 / 表格卡副标题共用）：自定义时显示区间摘要
  const rangeLabel = customRange
    ? formatRangeSummary(customRange.fromMs, customRange.toMs, locale)
    : t(RANGES.find((item) => item.range === range)!.labelKey);
  // 筛选中的 Agent 品牌名（未筛选为 undefined）：只下发给跟随筛选的
  // TrendCard / ModelTable 作副标题口径标注；MetricCards / HeatmapCard 不带
  //（逐卡标注对它们是噪音），AgentTable 不跟随筛选也不能带
  const agentName =
    agentFilter === "all" ? undefined : agentLabel(agentFilter);
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
          {/* Agent 筛选：指标卡 / 趋势 / 热力图 / 模型表切片到所选 Agent
              （Agent 统计表保留跨 Agent 对比，不跟随）；偏好持久化，
              切换只重新聚合、不重新取数 */}
          <DropdownMenu
            label={t("dashboard.overview.agentLabel")}
            ariaLabel={t("dashboard.overview.agentLabel")}
            options={agentOptions}
            value={agentFilter}
            onChange={changeAgentFilter}
            /* 筛选生效（偏好持久化，重启即单 Agent 口径）时触发器带激活态：
               主色描边 + 浅主色底，提示「当前不是默认的合并口径」（与自定义
               区间生效时的 db-range-trigger--active 同一先例） */
            active={agentFilter !== "all"}
          />
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
            /* Agent 筛选生效时副标题标注数据归属（合计非跨 Agent 口径） */
            agentName={agentName}
          />
          {/* 热力图 + Agent 表：默认上下堆叠（与页面其余卡片同节奏）；仅大窗
              （≥1340px，内容区 ≥1080px）并排两列，减少大窗下的纵向滚动。
              断点依据：52 列热力图的格子边长随列宽线性变化——默认 1100 窗若
              强行并排，热力列仅约 419px，扣除 51×2px 列间隙与 26px 星期标签
              列后格子约 5.4px，hover 命中区过小、四档色阶不可辨，且 Agent 表
              右列任何宽度都横向滚动、额度列默认屏外；1340 起热力列约 585px、
              格子约 8px（1280 内容列上限下约 9.5px），并排才划算。并排列宽
              11fr : 9fr——热力图对宽度最敏感，分略宽一列；Agent 表列挤由
              .db-table-wrap 横向滚动兜底。行内 align-items: stretch，矮的
              热力图卡拉伸至与表卡同高（内容保持自然高度、底部留白），两卡
              上下边对齐比高低参差整齐 */}
          <div className="db-overview-duo">
            <HeatmapCard series={view.heat} />
            <AgentTable
              rows={view.rows}
              currency={currency}
              rangeLabel={customRange ? rangeLabel : undefined}
            />
          </div>
          {/* 模型用量表：与 Agent 表同一份范围口径（范围内切片），成本按本机
              价格表折算（Cursor 用其后端成本），默认折叠前 10 行 */}
          <ModelTable
            rows={view.modelRows}
            currency={currency}
            rangeLabel={customRange ? rangeLabel : undefined}
            /* 同 TrendCard：筛选生效时副标题标注 Agent 口径（走独立 prop，
                不拼进 rangeLabel——AgentTable 不跟随筛选但共用该参数） */
            agentName={agentName}
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
