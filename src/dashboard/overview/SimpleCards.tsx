/**
 * 简洁方案的核心卡片（主窗口总览页）：当前速度 + Token 用量。
 *
 * 速度卡按「今日」窗口查询 get_model_speed（与模型速度页同一后端口径），
 * 展示平均速度；request_average 口径保留 ≈ 标记。没有可信样本时显示真实
 * 空态，不填 0。用量卡复用总览页已算好的 MetricSummary，范围切换走页头
 * 同一个 range state，不额外取数。
 */
import { useEffect, useState } from "react";
import { getModelSpeed } from "../../api";
import { sourceMeta } from "../../agentVisibility";
import { formatCost, formatMs, formatTokensLocale, formatTps } from "../../format";
import { useI18n } from "../../i18n";
import { dateLocale } from "../../i18n/locale";
import { displayModelName } from "../../modelName";
import type {
  Currency,
  ModelSpeedResponse,
  ModelSpeedScope,
} from "../../types";
import { costOf, type MetricSummary } from "./derive";
import type { OverviewRange, OverviewRangeChoice } from "./model";
import { Card } from "./primitives";

const SPEED_RANGES: { value: OverviewRange; labelKey: "range.today" | "range.7d" | "range.30d" }[] = [
  { value: "today", labelKey: "range.today" },
  { value: "7d", labelKey: "range.7d" },
  { value: "30d", labelKey: "range.30d" },
];

export function OverviewUsageCard({
  summary,
  currency,
  range,
  rangeLabel,
  onRangeChange,
}: {
  summary: MetricSummary;
  currency: Currency;
  range: OverviewRangeChoice;
  rangeLabel: string;
  onRangeChange: (range: OverviewRange) => void;
}) {
  const { locale, t } = useI18n();
  const rangeCost = costOf(summary.rangeCostUsd, summary.rangeCostCny, currency);
  const todayCost = costOf(summary.todayCostUsd, summary.todayCostCny, currency);
  const todayTokensLabel = formatTokensLocale(summary.todayTokens, locale);

  return (
    <Card
      title={t("customize.card.usage")}
      frame={{ window: "dashboard", page: "overview", id: "overview.usage" }}
      summary={`${rangeLabel} · ${formatTokensLocale(summary.rangeTokens, locale)} tokens`}
      action={
        <div className="db-seg db-seg--sm" role="group" aria-label={t("customize.usage.rangeHint")}>
          {SPEED_RANGES.map((item) => (
            <button
              key={item.value}
              type="button"
              className={`db-seg__btn ${
                range === item.value ? "db-seg__btn--active" : ""
              }`}
              aria-pressed={range === item.value}
              onClick={() => onRangeChange(item.value)}
            >
              {t(item.labelKey)}
            </button>
          ))}
        </div>
      }
    >
      {({ detail }) => (
        <div className="min-w-0">
          <div className="text-[11px] text-slate-500">
            {t("customize.usage.rangeTokens")} · {rangeLabel}
          </div>
          <div className="num mt-1 text-[34px] font-bold leading-none text-slate-900/90 truncate">
            {formatTokensLocale(summary.rangeTokens, locale)}
          </div>
          <div className="mt-2 text-[11px] text-slate-500">
            {t("customize.usage.todayTokens")}{" "}
            <span className="num font-medium text-slate-700">{todayTokensLabel}</span>
          </div>
          {detail === "full" && (
            <div className="mt-3 grid grid-cols-2 gap-2 border-t border-slate-900/6 pt-2">
              <div>
                <div className="text-[10px] text-slate-500">
                  {t("customize.usage.rangeCost")}
                </div>
                <div className="num text-[13px] font-semibold text-slate-800">
                  {formatCost(rangeCost, currency)}
                </div>
              </div>
              <div>
                <div className="text-[10px] text-slate-500">
                  {t("customize.usage.requests")}
                </div>
                <div className="num text-[13px] font-semibold text-slate-800">
                  {summary.rangeRequests.toLocaleString(dateLocale(locale))}
                </div>
              </div>
              <div>
                <div className="text-[10px] text-slate-500">
                  {t("customize.usage.todayCost")}
                </div>
                <div className="num text-[13px] font-semibold text-slate-800">
                  {formatCost(todayCost, currency)}
                </div>
              </div>
              <div>
                <div className="text-[10px] text-slate-500">
                  {t("customize.usage.requests")}
                </div>
                <div className="num text-[13px] font-semibold text-slate-800">
                  {summary.todayRequests.toLocaleString(dateLocale(locale))}
                </div>
              </div>
            </div>
          )}
        </div>
      )}
    </Card>
  );
}

export function OverviewSpeedCard({ scope }: { scope: ModelSpeedScope }) {
  const { t } = useI18n();
  const [data, setData] = useState<ModelSpeedResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setData(null);
    setError(null);
    getModelSpeed(0, scope)
      .then((res) => {
        if (!cancelled) setData(res);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [scope]);

  const rows = (data?.stats ?? [])
    .filter((row) => row.avgTps != null && Number.isFinite(row.avgTps))
    .sort((a, b) => (b.avgTps ?? 0) - (a.avgTps ?? 0));
  const top = rows[0] ?? null;
  const approximate = top?.speedQuality === "request_average";
  const speedText = top?.avgTps != null ? `${approximate ? "≈" : ""}${formatTps(top.avgTps)} t/s` : null;
  const summary = top
    ? `${displayModelName(top.modelId)} · ${speedText}`
    : t("customize.speed.none");

  return (
    <Card
      title={t("customize.card.speed")}
      frame={{ window: "dashboard", page: "overview", id: "overview.speed" }}
      summary={summary}
    >
      {({ detail }) =>
        top ? (
          <div className="flex min-w-0 flex-col gap-2">
            <div className="flex items-baseline justify-between gap-3 min-w-0">
              <span
                className="num truncate text-[32px] font-bold leading-none text-sky-700"
                title={speedText ?? undefined}
              >
                {speedText}
              </span>
              <span className="shrink-0 text-[11px] text-slate-500">
                {t("customize.speed.today")}
              </span>
            </div>
            <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[12px]">
              <span className="text-slate-500">{t("customize.speed.model")}</span>
              <span className="min-w-0 truncate font-medium text-slate-900/85">
                {displayModelName(top.modelId)}
              </span>
              <span className="text-slate-400">·</span>
              <span className="text-slate-500">{sourceMeta(top.source).label}</span>
            </div>
            <div className="text-[11px] text-slate-500">
              {t("customize.speed.label")}
              {top.speedQuality === "request_average"
                ? ` · ${t("customize.speed.today")} ≈`
                : ` · ${t("customize.speed.today")}`}
            </div>
            {detail === "full" && (
              <div className="mt-1 flex flex-col gap-2 border-t border-slate-900/6 pt-2">
                {top.ttftAvgMs != null && (
                  <div className="text-[11px] text-slate-600">
                    {t("common.ttft")} <span className="num">{formatMs(top.ttftAvgMs)}</span>
                  </div>
                )}
                {rows.slice(1, 4).map((row) => (
                  <div
                    key={`${row.source}:${row.provider}:${row.modelId}`}
                    className="flex min-w-0 items-center justify-between gap-3 text-[11px] text-slate-600"
                  >
                    <span className="min-w-0 truncate">
                      {displayModelName(row.modelId)}
                      <span className="text-slate-400"> · {sourceMeta(row.source).label}</span>
                    </span>
                    <span className="num shrink-0">
                      {row.speedQuality === "request_average" ? "≈" : ""}
                      {formatTps(row.avgTps!)} t/s
                    </span>
                  </div>
                ))}
                <div className="text-[10px] text-slate-400">
                  {t("customize.speed.fullHint")}
                </div>
              </div>
            )}
          </div>
        ) : (
          <div className="py-2">
            <div className="text-[13px] font-medium text-slate-700">
              {t("customize.speed.none")}
            </div>
            <div className="mt-1 text-[11px] leading-relaxed text-slate-500">
              {error ?? t("customize.speed.noneHint")}
            </div>
          </div>
        )
      }
    </Card>
  );
}
