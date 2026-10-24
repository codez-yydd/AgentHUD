/**
 * 额度驾驶舱页（主窗口第五页）：额度专页——当前额度全景 + 90 天历史趋势 +
 * 重置窗口时间线。与总览页的区别：总览 Agent 表只有「当前时点」一列且是
 * Agent 级聚合；本页做到账号/凭证级明细（zai 按账号指纹、provider 按凭证
 * label、CLI 通道为源级粒度）+ 90 天历史曲线 + 重置时间线。
 *
 * 口径纪律：本页百分比、仪表与曲线统一为**剩余额度**（= 100 − 已用），
 * 与总览页 Agent 表口径一致；历史表字段全部是已用口径，换算在 quota/model.ts
 * 完成。页头说明文案写明该口径。
 *
 * 驾驶舱布局（本页重构）：
 * ① 顶部深色控制台——整体余量仪表（全部窗口均值）+ 风险分布 + 三个态势
 *    读数（最紧窗口 / 待重置 / 数据覆盖），一眼回答「现在安不安全」；
 * ② 左主区——按最紧剩余排序的仪表阵列（每个账号/凭证一张卡，2–3 个圆形
 *    窗口仪表 + 90 天迷你曲线），下方是选中系列的 90 天剩余曲线；
 * ③ 右侧栏——重置序列（未来 resetAt 升序，最近一条标「最近」）。
 * 窄窗（≤1080px）时右栏落到阵列与趋势之间，仪表阵列保持两列。
 *
 * 数据获取（零自动 HTTP 铁律）：页面加载纯读三张本地历史表（最新快照即
 * 「当前值」，逐行标注快照时间让用户知道新鲜度）；只有页头「刷新」按钮
 * 触发各源 live 查询（Rust 侧有 TTL 缓存），live 成功自然写入历史采样后
 * 重读本地刷新整页。见 quota/useQuotaData.ts 头注释。
 *
 * 页面内每 30s 刷新一次 nowMs，让重置倒计时 / 「x 分钟前」跟着走，
 * 不触发任何取数。
 */
import { useEffect, useMemo, useState } from "react";
import { useI18n } from "../i18n";
import { AlertBanner, EmptyState } from "../layout";
import { DropdownMenu } from "../DropdownMenu";
import { Card, Skeleton, useDarkTheme } from "./primitives";
import { RefreshIcon } from "./icons";
import { RemainingTrendCard } from "./quota/RemainingTrendCard";
import { CockpitDeck } from "./quota/CockpitDeck";
import { InstrumentCard } from "./quota/InstrumentCard";
import { ResetRail } from "./quota/ResetRail";
import { useQuotaData, QUOTA_COCKPIT_DAYS } from "./quota/useQuotaData";
import {
  QUOTA_COCKPIT_SOURCES,
  primaryWindowOf,
  remainingPointsOf,
  sortSeriesByRisk,
  sourceLabelOf,
  summarizeCockpit,
  type RemainingPoint,
} from "./quota/model";
import {
  formatDateTime,
  timeAgo,
  windowLabel,
} from "./quota/format";
import { quotaTone } from "./quota/tone";

/** 页面内计时器：每 intervalMs 更新一次 nowMs（倒计时 / 相对时间跟随） */
function useNow(intervalMs: number): number {
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);
  return nowMs;
}

/** 趋势图的四点摘要（当前 / 区间最低 / 区间最高 / 采样点数） */
interface TrendStats {
  current: number;
  low: number;
  high: number;
  count: number;
}

function trendStatsOf(points: RemainingPoint[]): TrendStats | null {
  if (points.length === 0) return null;
  let low = points[0].remaining;
  let high = points[0].remaining;
  for (const point of points) {
    if (point.remaining < low) low = point.remaining;
    if (point.remaining > high) high = point.remaining;
  }
  return {
    current: points[points.length - 1].remaining,
    low,
    high,
    count: points.length,
  };
}

/** 趋势图横轴起点：90 天窗口的本地自然日 0 点（与取数窗口同口径） */
function startOfTrendWindow(nowMs: number): number {
  const d = new Date(nowMs);
  d.setDate(d.getDate() - (QUOTA_COCKPIT_DAYS - 1));
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

export function QuotaPage() {
  const { t } = useI18n();
  const state = useQuotaData();
  const dark = useDarkTheme();
  const nowMs = useNow(30_000);

  // ---- 趋势选择器状态（系列 + 窗口；数据刷新后按 id 尽量保序保持选中） ----
  const [seriesId, setSeriesId] = useState("");
  const [windowKey, setWindowKey] = useState("");

  const data = state.data;
  const series = data?.series ?? [];
  const summary = summarizeCockpit(series, nowMs);
  // 仪表阵列与趋势下拉共用「最紧的在前」顺序：默认聚焦最需要处理的系列
  const orderedSeries = useMemo(() => sortSeriesByRisk(series), [series]);
  const selected =
    orderedSeries.find((s) => s.id === seriesId) ?? orderedSeries[0] ?? null;
  const selectedWindows = selected?.windows ?? [];
  const effWindowKey = selectedWindows.some((w) => w.key === windowKey)
    ? windowKey
    : selected
      ? primaryWindowOf(selected)
      : "";

  // 暂无历史数据的来源名单（全集 − 有数据的；弱提示行，不占卡片）
  const emptySources = QUOTA_COCKPIT_SOURCES.filter(
    (s) => !data?.sourcesWithData.has(s)
  );

  const points = useMemo(
    () => (selected ? remainingPointsOf(selected, effWindowKey) : []),
    [selected, effWindowKey]
  );
  const stats = useMemo(() => trendStatsOf(points), [points]);

  const hasAnyData = series.length > 0;

  return (
    <div className="db-page db-qc-page">
      <header className="db-page__header db-page__header--sticky db-qc-header">
        <div>
          <h1 className="db-page__title">{t("dashboard.quota")}</h1>
          <p className="db-page__desc">{t("dashboard.quota.desc")}</p>
        </div>
        <div className="db-page__actions">
          {/* 全局新鲜度：全部采样里最近的一次（live 刷新后自然前移） */}
          {data?.latestSampleAt != null && (
            <span
              className="db-qc-live"
              title={formatDateTime(data.latestSampleAt)}
            >
              <span className="db-qc-live__dot" aria-hidden />
              <span className="db-qc-live__label">
                {t("dashboard.quota.liveLabel")}
              </span>
              <span className="db-qc-live__time num">
                {timeAgo(t, data.latestSampleAt, nowMs)}
              </span>
            </span>
          )}
          <button
            type="button"
            className="db-btn"
            onClick={state.refreshLive}
            disabled={state.refreshing}
            title={t("dashboard.quota.refreshHint")}
          >
            <RefreshIcon
              className={`h-3.5 w-3.5 ${state.refreshing ? "db-spin" : ""}`}
            />
            {state.refreshing
              ? t("dashboard.quota.refreshing")
              : t("common.refresh")}
          </button>
        </div>
      </header>

      {/* 单通道本地历史读取失败：弱提示（该通道按暂无历史数据降级）；
          整页硬错误态下不重复显示（错误态已说明读取失败） */}
      {state.partialFailures != null &&
        !(state.error != null && !hasAnyData) && (
          <AlertBanner type="error">
            {t("dashboard.quota.partialFailures", {
              names: state.partialFailures.join("、"),
            })}
          </AlertBanner>
        )}

      {/* live 刷新部分源失败：弱提示（不影响已读到的本地数据） */}
      {state.refreshFailures != null && (
        <AlertBanner type="error">
          {t("dashboard.quota.refreshFailed", {
            names: state.refreshFailures.join("、"),
          })}
        </AlertBanner>
      )}

      {state.error != null && !hasAnyData ? (
        /* 整页硬错误：三路本地历史全部失败且无任何数据（照 SyncPage 的
           db-errorstate 重试态；「重新加载」纯重读本地，不发 HTTP） */
        <div className="db-errorstate">
          <div className="db-errorstate__title">
            {t("dashboard.quota.errorTitle")}
          </div>
          <div className="db-errorstate__reason">
            {t("dashboard.quota.errorReason", { error: state.error })}
          </div>
          <button type="button" className="db-btn" onClick={state.reload}>
            <RefreshIcon className="h-3.5 w-3.5" />
            {t("dashboard.quota.retry")}
          </button>
        </div>
      ) : state.loading && !hasAnyData ? (
        /* 首屏骨架：控制台一条 + 仪表阵列四张 + 右栏（形状与真实布局一致） */
        <>
          <Skeleton className="h-[296px] w-full db-qc-skeleton-hero" />
          <div className="db-qc-skeleton-workspace">
            <div className="db-qc-skeleton-fleet">
              {[0, 1, 2, 3].map((index) => (
                <Skeleton key={index} className="h-[218px] w-full" />
              ))}
            </div>
            <Skeleton className="db-qc-skeleton-rail" />
          </div>
        </>
      ) : !hasAnyData ? (
        /* 空态：还没有任何额度记录（不是错误）。用通用 EmptyState 引导 +
           刷新按钮触发 live 查询（与页头刷新同一逻辑，含 busy 防重） */
        <EmptyState
          title={t("dashboard.quota.emptyTitle")}
          hint={t("dashboard.quota.emptyHint")}
          action={
            <button
              type="button"
              className="db-btn mt-1"
              onClick={state.refreshLive}
              disabled={state.refreshing}
            >
              <RefreshIcon
                className={`h-3.5 w-3.5 ${state.refreshing ? "db-spin" : ""}`}
              />
              {state.refreshing
                ? t("dashboard.quota.refreshing")
                : t("common.refresh")}
            </button>
          }
        />
      ) : (
        <>
          {/* ---- 控制台：整体余量 / 风险分布 / 态势读数 ---- */}
          <CockpitDeck summary={summary} nowMs={nowMs} t={t} />

          <div className="db-qc-workspace">
            {/* ---- 仪表阵列（最紧的系列在前） ---- */}
            <section className="db-qc-fleet" aria-labelledby="db-qc-fleet-title">
              <div className="db-qc-section-head">
                <div>
                  <h2 id="db-qc-fleet-title" className="db-qc-section-title">
                    {t("dashboard.quota.fleetTitle")}
                  </h2>
                  <p className="db-qc-section-sub">
                    {t("dashboard.quota.fleetSub", {
                      count: summary.seriesCount,
                    })}
                  </p>
                </div>
                <span className="db-qc-section-badge num">
                  {summary.windowCount}
                  <small>{t("dashboard.quota.windowUnit")}</small>
                </span>
              </div>

              <div className="db-qc-fleet-grid">
                {orderedSeries.map((item) => (
                  <InstrumentCard
                    key={item.id}
                    series={item}
                    nowMs={nowMs}
                    dark={dark}
                    t={t}
                  />
                ))}
              </div>

              {/* 无历史数据的来源：紧凑胶囊行（不逐源渲染空卡片） */}
              {emptySources.length > 0 && (
                <div className="db-qc-empty-sources">
                  <span className="db-qc-empty-sources__label">
                    {t("dashboard.quota.sourcesEmptyLabel")}
                  </span>
                  {emptySources.map((source) => (
                    <span key={source} className="db-qc-empty-sources__chip">
                      {sourceLabelOf(source)}
                    </span>
                  ))}
                </div>
              )}
            </section>

            {/* ---- 历史趋势：系列 + 窗口选择器 + 90 天剩余曲线 ---- */}
            <section className="db-qc-trend">
              <Card
                title={t("dashboard.quota.trendTitle")}
                sub={t("dashboard.quota.trendRangeSub", {
                  range: t("dashboard.overview.range.90d"),
                })}
              >
                {selected ? (
                  <>
                    <div className="db-qc-trend-toolbar">
                      <DropdownMenu
                        className="db-qc-select"
                        ariaLabel={t("dashboard.quota.seriesLabel")}
                        label={t("dashboard.quota.seriesLabel")}
                        options={orderedSeries.map((s) => ({
                          value: s.id,
                          label: s.label,
                        }))}
                        value={selected.id}
                        onChange={(value) => {
                          setSeriesId(value);
                          setWindowKey(""); // 换系列后窗口回落到该系列主窗口
                        }}
                      />
                      <DropdownMenu
                        className="db-qc-select"
                        ariaLabel={t("dashboard.quota.windowLabel")}
                        label={t("dashboard.quota.windowLabel")}
                        options={selected.windows.map((w) => ({
                          value: w.key,
                          label: windowLabel(t, w.key),
                        }))}
                        value={effWindowKey}
                        onChange={setWindowKey}
                      />
                      <span className="db-qc-trend-range num">
                        {t("dashboard.overview.range.90d")}
                      </span>
                    </div>

                    {stats && (
                      <div className="db-qc-trend-stats">
                        <div className="db-qc-trend-stat">
                          <span className="db-qc-trend-stat__label">
                            {t("dashboard.quota.trendCurrent")}
                          </span>
                          <span
                            className="db-qc-trend-stat__value num"
                            style={{
                              color: quotaTone(stats.current, dark),
                            }}
                          >
                            {Math.round(stats.current)}%
                          </span>
                        </div>
                        <div className="db-qc-trend-stat">
                          <span className="db-qc-trend-stat__label">
                            {t("dashboard.quota.trendLow")}
                          </span>
                          <span
                            className="db-qc-trend-stat__value num"
                            style={{ color: quotaTone(stats.low, dark) }}
                          >
                            {Math.round(stats.low)}%
                          </span>
                        </div>
                        <div className="db-qc-trend-stat">
                          <span className="db-qc-trend-stat__label">
                            {t("dashboard.quota.trendHigh")}
                          </span>
                          <span
                            className="db-qc-trend-stat__value num"
                            style={{ color: quotaTone(stats.high, dark) }}
                          >
                            {Math.round(stats.high)}%
                          </span>
                        </div>
                        <div className="db-qc-trend-stat">
                          <span className="db-qc-trend-stat__label">
                            {t("dashboard.quota.trendSamples")}
                          </span>
                          <span className="db-qc-trend-stat__value num">
                            {stats.count}
                          </span>
                        </div>
                      </div>
                    )}

                    <RemainingTrendCard
                      points={points}
                      fromMs={startOfTrendWindow(nowMs)}
                      toMs={nowMs}
                      color={quotaTone(stats?.current ?? 100, dark)}
                    />
                  </>
                ) : (
                  <div className="db-empty">
                    {t("dashboard.quota.trendEmpty")}
                  </div>
                )}
              </Card>
            </section>

            {/* ---- 右侧栏：重置序列 ---- */}
            <aside className="db-qc-rail">
              <ResetRail
                items={summary.resets}
                nowMs={nowMs}
                dark={dark}
                t={t}
              />
            </aside>
          </div>

          <p className="db-page__footnote">{t("dashboard.quota.footnote")}</p>
        </>
      )}
    </div>
  );
}
