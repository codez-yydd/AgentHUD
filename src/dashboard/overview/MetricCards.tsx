/**
 * 指标卡行：范围内 Token（含输入/输出分项）、估算成本、请求数、今日维度卡。
 *
 * 第 4 卡随页头范围变化：非今日范围下是「今日 Token」（独立于范围的口径，必须
 * 保留）；今日范围下「范围内 Token」与「今日 Token」是同一份数据，第 4 卡换成
 * 「较昨日」——昨日 token 与环比，数据在 90 天日桶里现成（derive.summarize）。
 *
 * 配色：只有主指标（范围内 Token）用强调色，其余数值走中性文字色；四卡四色在
 * 4 列并排时主次不分、噪声大。数值色由 dashboard.css 的 --db-* 变量负责翻转。
 */
import type { ReactNode } from "react";
import { dateLocale } from "../../i18n/locale";
import { formatCost, formatTokensLocale, formatTokensParts } from "../../format";
import { useI18n } from "../../i18n";
import type { Currency } from "../../types";
import { costOf, type MetricSummary } from "./derive";
import type { OverviewRangeChoice } from "./model";

/** 副行双列：不再把「输入 X · 输出 Y」拼成整行长串（窄窗 + 大字号档下必然
 *  折行）；两列各自 min-w-0 + 单行截断，截断后的完整文本由 title 兜底 */
function SubPair({ left, right }: { left: string; right: string }) {
  return (
    <div className="db-metric__subgrid">
      <span className="db-metric__subcell num" title={left}>
        {left}
      </span>
      <span className="db-metric__subcell num" title={right}>
        {right}
      </span>
    </div>
  );
}

function MetricCard({
  label,
  value,
  unit,
  sub,
  primary = false,
}: {
  label: string;
  value: string;
  unit?: string;
  sub: ReactNode;
  /** 主指标卡：数值走强调色（其余走中性色） */
  primary?: boolean;
}) {
  return (
    <div className="db-card db-metric">
      <div className="db-metric__label">{label}</div>
      <div
        className={`db-metric__value ${primary ? "db-metric__value--primary" : ""}`}
      >
        <span className="num min-w-0 truncate" title={value}>
          {value}
        </span>
        {unit && <span className="db-metric__unit num shrink-0">{unit}</span>}
      </div>
      <div className="db-metric__sub">{sub}</div>
    </div>
  );
}

export function MetricCards({
  summary,
  currency,
  range,
  rangeLabel,
}: {
  summary: MetricSummary;
  currency: Currency;
  /** 当前页头范围：今日档下第 4 卡换成「较昨日」；自定义区间按非今日处理
   *  （「范围内」与「今日」不是同一份数据，仍展示今日 Token 卡） */
  range: OverviewRangeChoice;
  /** 自定义区间摘要（MM-DD ~ MM-DD；预设档不传）：标在「范围内 Token」卡副行
   *  下方，说明卡片数值覆盖的是哪一段自然日（自定义范围读不出档位名） */
  rangeLabel?: string;
}) {
  const { locale, t } = useI18n();
  const intlLocale = dateLocale(locale);
  const rangeCost = costOf(summary.rangeCostUsd, summary.rangeCostCny, currency);
  const todayCost = costOf(summary.todayCostUsd, summary.todayCostCny, currency);
  const avgCost = summary.days > 0 ? rangeCost / summary.days : 0;
  const avgRequests = summary.days > 0 ? summary.rangeRequests / summary.days : 0;
  const tokens = formatTokensParts(summary.rangeTokens, locale);
  const todayTokens = formatTokensParts(summary.todayTokens, locale);

  // 第 4 卡（今日范围）：今日 vs 昨日环比。措辞与面板 TrendChart 的涨跌角标一致
  // （持平 / 新增 / 百分比），只是这里按 token 口径；两边都为 0 时不出涨跌（"—"）
  let deltaText = "—";
  if (summary.todayTokens > 0 || summary.yesterdayTokens > 0) {
    if (summary.yesterdayTokens > 0) {
      const pct =
        ((summary.todayTokens - summary.yesterdayTokens) / summary.yesterdayTokens) *
        100;
      deltaText =
        Math.abs(pct) < 0.5
          ? t("common.trendFlat")
          : `${pct > 0 ? "↑" : "↓"}${Math.abs(pct).toFixed(0)}%`;
    } else {
      // 昨日无用量、今日有：没有可比的基数，按面板同款「新增」表达
      deltaText = t("common.trendNew");
    }
  }

  return (
    <div className="db-metrics">
      <MetricCard
        label={t("dashboard.overview.rangeTokens")}
        value={tokens.num}
        unit={tokens.unit}
        primary
        sub={
          <>
            <SubPair
              left={t("dashboard.overview.subInput", {
                value: formatTokensLocale(summary.rangeInput, locale),
              })}
              right={t("dashboard.overview.subOutput", {
                value: formatTokensLocale(summary.rangeOutput, locale),
              })}
            />
            {rangeLabel != null && (
              <div className="db-metric__range num" title={rangeLabel}>
                {rangeLabel}
              </div>
            )}
          </>
        }
      />
      {/* 「估算花费」标签已表达估算语义，不再重复挂「估算」角标 */}
      <MetricCard
        label={t("dashboard.overview.estCost")}
        value={formatCost(rangeCost, currency)}
        sub={
          <SubPair
            left={t("dashboard.overview.subAvg", {
              value: formatCost(avgCost, currency),
            })}
            right={t("dashboard.overview.subToday", {
              value: formatCost(todayCost, currency),
            })}
          />
        }
      />
      <MetricCard
        label={t("dashboard.overview.requestCount")}
        value={summary.rangeRequests.toLocaleString(intlLocale)}
        sub={t("dashboard.overview.dailyAvgRequests", {
          n: Math.round(avgRequests).toLocaleString(intlLocale),
        })}
      />
      {range === "today" ? (
        <MetricCard
          label={t("dashboard.overview.vsYesterday")}
          value={deltaText}
          sub={t("dashboard.overview.yesterdayTokens", {
            n: formatTokensLocale(summary.yesterdayTokens, locale),
          })}
        />
      ) : (
        <MetricCard
          label={t("dashboard.overview.todayTokens")}
          value={todayTokens.num}
          unit={todayTokens.unit}
          sub={t("dashboard.overview.todayRequests", {
            n: summary.todayRequests.toLocaleString(intlLocale),
          })}
        />
      )}
    </div>
  );
}
