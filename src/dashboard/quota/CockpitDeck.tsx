/**
 * 驾驶舱控制台（页面顶部深色面板）：整体余量仪表 + 风险分布 +
 * 三个态势读数（最紧窗口 / 待重置 / 数据覆盖）。
 *
 * 纯展示组件：聚合口径来自 model.summarizeCockpit，时间由页面传入
 * （页面每 30s 刷新一次 now，倒计时跟着走）。深色面板在亮暗主题下都保持
 * 同一套“仪表台”观感，色彩不进入 --db-* 变量体系，避免亮色主题下失去
 * 控制台对比度。
 */
import { remainingAccentColor } from "../../widgets";
import type { TFn } from "../../i18n";
import { QuotaDial } from "./QuotaDial";
import { countdownCore, riskLabel, windowLabel } from "./format";
import {
  QUOTA_COCKPIT_SOURCES,
  type CockpitSummary,
  type QuotaRisk,
} from "./model";

const RISKS: readonly QuotaRisk[] = ["healthy", "warning", "critical"];

export function CockpitDeck({
  summary,
  nowMs,
  t,
}: {
  summary: CockpitSummary;
  nowMs: number;
  t: TFn;
}) {
  const average = summary.averageRemaining;
  const averageRounded = average == null ? null : Math.round(average);
  // 主状态按「最严重的窗口」而不是平均值：平均 89% 也可能藏着 1 个告急窗口，
  // 控制台必须先说最危险的那件事（仪表本身仍显示平均值）
  const statusKey =
    summary.riskCounts.critical > 0
      ? "critical"
      : summary.riskCounts.warning > 0
        ? "warning"
        : "healthy";
  const statusText =
    statusKey === "critical"
      ? t("dashboard.quota.deckStatusCriticalCount", {
          count: summary.riskCounts.critical,
        })
      : statusKey === "warning"
        ? t("dashboard.quota.deckStatusWarningCount", {
            count: summary.riskCounts.warning,
          })
        : t("dashboard.quota.deckStatusHealthy");

  const worst = summary.worstRow;
  const worstTone = worst ? remainingAccentColor(worst.remainingPct) : "#94a3b8";
  const next = summary.nextReset;
  const nextIn = next ? countdownCore(next.resetAt, nowMs) : null;
  const coveragePct =
    (summary.sourceCount / QUOTA_COCKPIT_SOURCES.length) * 100;

  return (
    <section className="db-qc-console" aria-label={t("dashboard.quota")}>
      <div className="db-qc-hero">
        <QuotaDial
          pct={average ?? 0}
          size={150}
          tone={average == null ? "#94a3b8" : remainingAccentColor(average)}
          label={`${t("dashboard.quota.deckEyebrow")} ${averageRounded ?? 0}%`}
          hero
        />
        <div className="db-qc-hero__copy">
          <div className="db-qc-eyebrow">{t("dashboard.quota.deckEyebrow")}</div>
          <div className={`db-qc-hero__status db-qc-hero__status--${statusKey}`}>
            {statusText}
          </div>
          <p className="db-qc-hero__sub">
            {t("dashboard.quota.deckSub", { count: summary.windowCount })}
          </p>
          <div className="db-qc-legend">
            {RISKS.map((risk) => (
              <span
                key={risk}
                className={`db-qc-legend__item db-qc-legend__item--${risk}`}
              >
                <i className="db-qc-legend__dot" />
                {riskLabel(t, risk)}
                <b className="num">{summary.riskCounts[risk]}</b>
              </span>
            ))}
          </div>
        </div>
      </div>

      <div className="db-qc-console__stats">
        <div className="db-qc-stat">
          <span className="db-qc-stat__label">{t("dashboard.quota.deckWorst")}</span>
          <div className="db-qc-stat__value" style={{ color: worstTone }}>
            {worst ? (
              <>
                {Math.round(worst.remainingPct)}
                <small>%</small>
              </>
            ) : (
              "—"
            )}
          </div>
          <span className="db-qc-stat__meta" title={worst?.series.label}>
            {worst
              ? `${worst.series.label} · ${windowLabel(t, worst.windowKey)}`
              : t("dashboard.quota.deckNoSamples")}
          </span>
          <div className="db-qc-stat__bar">
            <span
              style={{
                width: `${worst ? Math.min(Math.max(worst.remainingPct, 0), 100) : 0}%`,
                background: worstTone,
              }}
            />
          </div>
        </div>

        <div className="db-qc-stat">
          <span className="db-qc-stat__label">
            {t("dashboard.quota.deckResets")}
          </span>
          <div className="db-qc-stat__value num">{summary.resets.length}</div>
          <span className="db-qc-stat__meta">
            {nextIn
              ? t("dashboard.quota.deckNextReset", { time: nextIn })
              : t("dashboard.quota.deckResetsEmpty")}
          </span>
          <div className="db-qc-stat__pips" aria-hidden>
            {summary.resets.slice(0, 12).map((item) => (
              <i key={`${item.series.id}:${item.windowKey}`} />
            ))}
            {summary.resets.length > 12 && (
              <span className="db-qc-stat__pip-more">
                +{summary.resets.length - 12}
              </span>
            )}
          </div>
        </div>

        <div className="db-qc-stat">
          <span className="db-qc-stat__label">
            {t("dashboard.quota.deckCoverage")}
          </span>
          <div className="db-qc-stat__value num">
            {summary.sourceCount}
            <small> / {QUOTA_COCKPIT_SOURCES.length}</small>
          </div>
          <span className="db-qc-stat__meta">
            {t("dashboard.quota.deckCoverageHint", { series: summary.seriesCount })}
          </span>
          <div className="db-qc-stat__bar">
            <span
              style={{
                width: `${coveragePct}%`,
                background: "linear-gradient(90deg, #0ea5e9, #22d3ee)",
              }}
            />
          </div>
        </div>
      </div>
    </section>
  );
}
