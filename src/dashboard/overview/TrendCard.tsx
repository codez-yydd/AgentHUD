/**
 * 趋势图卡片：按天聚合的柱状图，支持 Token / 花费 / 请求 三指标切换，
 * 悬停显示当日明细。
 *
 * 视觉沿用面板 TrendChart（widgets.tsx）的语言——圆角柱、圆角分段控件、
 * 深色 tooltip、标签按步长抽稀——但按大窗重排：图高随窗口自适应
 * （clamp 190px..320px，面板是 64px 迷你图）、纵轴带横向网格线与数值刻度、
 * 指标切换用「花费 / Token / 请求」同一套措辞。数据侧不需要图表库：
 * 柱体是 div + 百分比高度，与 TrendChart 同款做法。
 *
 * 指标口径（数据都来自 DayAgg，不额外取数）：
 *  - token：输入侧 / 输出双段堆叠（面板同款双色），图例只在这一指标下展示；
 *  - cost：按当前币种口径选列（costCny / costUsd，币种从属语言）；
 *  - requests：请求次数单段柱。
 * 刻度、峰值与 tooltip 都跟随当前指标格式化（token 走 万/亿 · K/M/B，
 * 金额走 formatCost，请求数走 locale 千分位）。
 */
import { useState } from "react";
import { formatCost, formatTokensLocale } from "../../format";
import { useI18n } from "../../i18n";
import { dateLocale } from "../../i18n/locale";
import type { Currency } from "../../types";
import type { TrendMetric } from "../../widgets";
import { costOf, type TrendDay } from "./derive";
import { Card } from "./primitives";

/** 指标切换顺序（与面板 TrendChart 的 metrics 数组同序：花费在前） */
const METRICS: TrendMetric[] = ["cost", "token", "requests"];

/** 纵轴刻度比例：柱高百分比与刻度值一一对应（1 = 当日峰值） */
const GRID_LEVELS = [1, 0.75, 0.5, 0.25];

/** 当日某指标的数值（花费按当前币种选列） */
function metricValueOf(day: TrendDay, metric: TrendMetric, currency: Currency): number {
  return metric === "cost"
    ? costOf(day.costUsd, day.costCny, currency)
    : metric === "token"
      ? day.total
      : day.requests;
}

export function TrendCard({
  series,
  currency,
  rangeLabel,
  agentName,
}: {
  series: TrendDay[];
  currency: Currency;
  /** 页头当前范围文案（"近 30 天"），用于卡片副标题 */
  rangeLabel: string;
  /** Agent 筛选生效时的品牌名（未筛选不传）：副标题是本卡的口径说明位，
   *  筛选后合计已不是跨 Agent 口径，须标注数据归属；Agent 名插在范围与
   *  合计之间（语序由词条控制，中英各自组织） */
  agentName?: string;
}) {
  const { locale, t } = useI18n();
  const [hover, setHover] = useState<number | null>(null);
  const [metric, setMetric] = useState<TrendMetric>("token");
  const intlLocale = dateLocale(locale);

  const n = series.length;
  const hasUnsplit = series.some((d) => d.unsplit > 0);
  // 标签抽稀：约 8 个刻度，首尾必显（与 TrendChart 同思路）
  const labelStep = n <= 8 ? 1 : Math.max(2, Math.ceil(n / 8));
  // 柱间距/柱宽随天数自适应（与面板 TrendChart 的 barGap 同思路，大窗放宽）
  const plotMod = n > 30 ? "db-chart__plot--dense" : n <= 7 ? "db-chart__plot--sparse" : "";

  // 当前指标下的格式化（刻度 / 峰值 / tooltip / 合计共用同一套）
  const formatMetric = (value: number): string =>
    metric === "cost"
      ? formatCost(value, currency)
      : metric === "token"
        ? formatTokensLocale(value, locale)
        : value.toLocaleString(intlLocale);
  const metricLabel = (m: TrendMetric): string =>
    m === "cost" ? t("common.cost") : m === "token" ? "Token" : t("common.requests");

  const values = series.map((day) => metricValueOf(day, metric, currency));
  const metricMax = values.reduce((max, value) => (value > max ? value : max), 0);
  const scaleMax = Math.max(metricMax, 1);
  const rangeTotal = values.reduce((sum, value) => sum + value, 0);

  // 峰值日（跟随当前指标）：无用量时不出图脚信息
  let peak: { key: string; value: number } | null = null;
  for (let i = 0; i < series.length; i += 1) {
    if (!peak || values[i] > peak.value) peak = { key: series[i].key, value: values[i] };
  }
  if (peak != null && peak.value <= 0) peak = null;

  return (
    <Card
      title={t("dashboard.overview.trendTitle")}
      frame={{ window: "dashboard", page: "overview", id: "overview.trend" }}
      summary={t("customize.summaryTotal", { value: formatMetric(rangeTotal) })}
      bodyClassName="db-card__body--chart"
      // 副标题 = 范围口径说明位：Agent 筛选生效时换带 {agent} 的词条
      //（「近 30 天 · Claude Code · 合计 …」），未筛选维持原词条
      sub={
        agentName != null
          ? t("dashboard.overview.trendSubAgent", {
              range: rangeLabel,
              agent: agentName,
              total: formatMetric(rangeTotal),
            })
          : t("dashboard.overview.trendSub", {
              range: rangeLabel,
              total: formatMetric(rangeTotal),
            })
      }
      action={
        <>
          <div
            className="db-seg db-seg--sm"
            role="group"
            aria-label={t("dashboard.overview.metricLabel")}
          >
            {METRICS.map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMetric(m)}
                className={`db-seg__btn ${metric === m ? "db-seg__btn--active" : ""}`}
                aria-pressed={metric === m}
              >
                {metricLabel(m)}
              </button>
            ))}
          </div>
          {/* 图例只对 Token 指标（输入侧 / 输出堆叠）有意义：
              花费 / 请求是单一序列，留着会误导成分段口径 */}
          {metric === "token" && (
            <div className="db-legend">
              <span className="db-legend__item">
                <i className="db-swatch db-swatch--input" />
                {t("dashboard.overview.seriesInput")}
              </span>
              <span className="db-legend__item">
                <i className="db-swatch db-swatch--output" />
                {t("dashboard.overview.seriesOutput")}
              </span>
            </div>
          )}
        </>
      }
    >
      <div className="db-chart">
        <div className={`db-chart__plot ${plotMod}`}>
          {/* 纵轴：4 条横向网格线 + 数值刻度（绝对定位铺在柱体后，
              刻度格式随当前指标与单位体系） */}
          {metricMax > 0 && (
            <div className="db-chart__grid" aria-hidden>
              {GRID_LEVELS.map((level) => (
                <div
                  key={level}
                  className="db-chart__grid-line"
                  style={{ bottom: `${level * 100}%` }}
                >
                  <span className="db-chart__grid-label num">
                    {formatMetric(metricMax * level)}
                  </span>
                </div>
              ))}
            </div>
          )}

          {series.map((day, i) => {
            const value = values[i];
            const heightPct = value > 0 ? Math.max((value / scaleMax) * 100, 1.5) : 0;
            const outputShare = day.total > 0 ? (day.output / day.total) * 100 : 0;
            const inputShare = 100 - outputShare;
            const isHover = hover === i;
            // tooltip 行序跟随当前指标：选中的指标置顶（并加粗），其余照旧
            const tipRows: { key: string; label: string; value: string }[] = [
              {
                key: "token",
                label: t("dashboard.overview.ttTotal"),
                value: formatTokensLocale(day.total, locale),
              },
              {
                key: "cost",
                label: t("dashboard.overview.ttCost"),
                value: formatCost(costOf(day.costUsd, day.costCny, currency), currency),
              },
              {
                key: "requests",
                label: t("dashboard.overview.ttRequests"),
                value: day.requests.toLocaleString(intlLocale),
              },
            ];
            const primary = tipRows.find((row) => row.key === metric)!;
            const rest = tipRows.filter((row) => row.key !== metric);
            return (
              <div
                key={day.key}
                className={`db-chart__col ${isHover ? "db-chart__col--hover" : ""}`}
                onMouseEnter={() => setHover(i)}
                onMouseLeave={() => setHover(null)}
              >
                {isHover && (
                  <div
                    className={`db-tip ${
                      i < 2 ? "db-tip--left" : i >= n - 2 ? "db-tip--right" : ""
                    }`}
                  >
                    <div className="db-tip__title num">{day.key}</div>
                    <TipRow label={primary.label} value={primary.value} strong />
                    {/* Token 指标下补输入 / 输出拆分（其余指标无拆分口径） */}
                    {metric === "token" && (
                      <>
                        <TipRow
                          label={t("dashboard.overview.ttOutput")}
                          value={`${day.unsplit > 0 ? "≈" : ""}${formatTokensLocale(
                            day.output,
                            locale
                          )}`}
                        />
                        <TipRow
                          label={t("dashboard.overview.ttInput")}
                          value={formatTokensLocale(day.input, locale)}
                        />
                      </>
                    )}
                    {rest.map((row) => (
                      <TipRow key={row.key} label={row.label} value={row.value} />
                    ))}
                  </div>
                )}
                <div className="db-bar" style={{ height: `${heightPct}%` }}>
                  {metric === "token" ? (
                    <>
                      <div className="db-bar__input" style={{ height: `${inputShare}%` }} />
                      <div
                        className="db-bar__output"
                        style={{ height: `${outputShare}%` }}
                      />
                    </>
                  ) : (
                    <div
                      className={metric === "cost" ? "db-bar__cost" : "db-bar__requests"}
                      style={{ height: "100%" }}
                    />
                  )}
                </div>
              </div>
            );
          })}
        </div>

        <div className={`db-chart__axis ${plotMod}`}>
          {series.map((day, i) => {
            const isFirst = i === 0;
            const isLast = i === n - 1;
            const show = isLast || isFirst || i % labelStep === 0;
            // 单日范围（今日）只有一个刻度，居中即可，不做首/尾贴边
            const edge =
              n === 1
                ? ""
                : isFirst
                  ? "db-chart__tick--first"
                  : isLast
                    ? "db-chart__tick--last"
                    : "";
            return (
              <span
                key={day.key}
                className={`db-chart__tick num ${edge} ${
                  show ? "" : "db-chart__tick--hidden"
                }`}
              >
                {day.key}
              </span>
            );
          })}
        </div>

        <div className="db-chart__foot">
          {peak && (
            <span className="db-chart__peak num">
              {t("dashboard.overview.peak", {
                date: peak.key,
                value: formatMetric(peak.value),
              })}
            </span>
          )}
          {hasUnsplit && metric === "token" && (
            <span className="db-chart__note">{t("dashboard.overview.splitNote")}</span>
          )}
        </div>
      </div>
    </Card>
  );
}

function TipRow({
  label,
  value,
  strong = false,
}: {
  label: string;
  value: string;
  /** 当前所选指标行：加粗突出（其余行为对照信息） */
  strong?: boolean;
}) {
  return (
    <div className={`db-tip__row ${strong ? "db-tip__row--strong" : ""}`}>
      <span className="db-tip__label">{label}</span>
      <span className="num">{value}</span>
    </div>
  );
}
