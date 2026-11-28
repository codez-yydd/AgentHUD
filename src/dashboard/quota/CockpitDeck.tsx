/**
 * 驾驶舱控制台（页面顶部中性面板）：整体余量分段圆环 + 风险计数 +
 * 三个态势读数（最紧窗口 / 待重置 / 数据覆盖）。
 *
 * 视觉与整页同一套语言：白底（暗色卡片底色）发丝描边面板，不再有深色
 * 「仪表台」底、渐变、水印与顶部亮线。聚合口径来自 model.summarizeCockpit，
 * 时间由页面传入（页面每 30s 刷新一次 now，倒计时跟着走）。
 *
 * 左侧一组：72px 分段圆环按「正常 / 偏低 / 告急」的窗口数量占比分段着色
 * （段间约 2px 缺口，从正上方顺时针依次是正常、偏低、告急；数量为 0 的
 * 段不画），圆环正中是全部窗口的平均剩余百分数。右侧一组：三列读数，
 * 列间竖向发丝线；只有带状态含义的数值（最紧张窗口的剩余）上状态色，
 * 其余数值用主文字色。
 */
import type { CSSProperties } from "react";
import type { TFn } from "../../i18n";
import { countdownCore, riskLabel, windowLabel } from "./format";
import {
  QUOTA_COCKPIT_SOURCES,
  type CockpitSummary,
  type QuotaRisk,
} from "./model";
import { quotaTone } from "./tone";

const RISKS: readonly QuotaRisk[] = ["healthy", "warning", "critical"];

/** 三档风险 → 语义色令牌（与 quota/tone.ts 同一套，圆环 / 图例 / 圆点共用） */
const RISK_TONE: Record<QuotaRisk, string> = {
  healthy: "var(--db-quota-ok)",
  warning: "var(--db-quota-warn)",
  critical: "var(--db-quota-danger)",
};

// 分段圆环几何：viewBox 72×72、半径 33、线宽 6（CSS 负责），圆心正中数字
const RING_SIZE = 72;
const RING_R = 33;
const RING_CX = RING_SIZE / 2;
const RING_GAP_PX = 2;

/** 极坐标（0° 正上方、顺时针）→ viewBox 坐标 */
function ringPoint(angleDeg: number): [number, number] {
  const rad = (angleDeg * Math.PI) / 180;
  return [
    RING_CX + Math.sin(rad) * RING_R,
    RING_CX - Math.cos(rad) * RING_R,
  ];
}

/** 一段弧的路径；sweep 覆盖整圈（单段无缺口）时返回 null（用 circle 画） */
function ringArc(fromDeg: number, toDeg: number): string | null {
  if (toDeg - fromDeg >= 359.999) return null;
  const [x0, y0] = ringPoint(fromDeg);
  const [x1, y1] = ringPoint(toDeg);
  const largeArc = toDeg - fromDeg > 180 ? 1 : 0;
  return `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${RING_R} ${RING_R} 0 ${largeArc} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
}

/**
 * 分段圆环：按风险计数占比从正上方顺时针画「正常 → 偏低 → 告急」，
 * 段间留约 2px 缺口（单段 / 全 0 时不留）。中心是平均剩余百分数，
 * 无障碍名称由调用方用现有词条拼（整体余量 + 百分数）。
 */
function SegmentRing({
  counts,
  averagePct,
  label,
}: {
  counts: Record<QuotaRisk, number>;
  /** 平均剩余百分数（圆环正中数字；null 显示 —） */
  averagePct: number | null;
  label: string;
}) {
  const total = counts.healthy + counts.warning + counts.critical;
  const gapDeg = total > 1 ? (RING_GAP_PX / (2 * Math.PI * RING_R)) * 360 : 0;
  const segments: { risk: QuotaRisk; d: string | null }[] = [];
  if (total > 0) {
    let cursor = 0;
    for (const risk of RISKS) {
      const count = counts[risk];
      if (count <= 0) continue;
      const sweep = (count / total) * 360;
      const from = cursor + gapDeg / 2;
      const to = cursor + sweep - gapDeg / 2;
      segments.push({ risk, d: to > from ? ringArc(from, to) : null });
      cursor += sweep;
    }
  }

  return (
    <div
      className="db-qc-ring"
      style={{ width: RING_SIZE, height: RING_SIZE }}
      role="img"
      aria-label={label}
    >
      <svg viewBox={`0 0 ${RING_SIZE} ${RING_SIZE}`} aria-hidden>
        {/* 轨道整圈（段间缺口处露出轨道，视觉一致） */}
        <circle
          cx={RING_CX}
          cy={RING_CX}
          r={RING_R}
          className="db-qc-ring__track"
        />
        {segments.map(({ risk, d }) =>
          d ? (
            <path
              key={risk}
              d={d}
              className="db-qc-ring__seg"
              style={{ stroke: RISK_TONE[risk] } as CSSProperties}
            />
          ) : (
            <circle
              key={risk}
              cx={RING_CX}
              cy={RING_CX}
              r={RING_R}
              className="db-qc-ring__seg"
              style={{ stroke: RISK_TONE[risk] } as CSSProperties}
            />
          )
        )}
      </svg>
      <span className="db-qc-ring__value num">
        {averagePct == null ? "—" : Math.round(averagePct)}
        {averagePct != null && <small>%</small>}
      </span>
    </div>
  );
}

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
  // 控制台必须先说最危险的那件事（圆环本身仍显示平均值）
  const statusKey =
    summary.riskCounts.critical > 0
      ? "critical"
      : summary.riskCounts.warning > 0
        ? "warning"
        : "healthy";
  // 数量 = 1 走 …One 单数键（英文形态不同；t() 只有 {name} 插值，没有复数）
  const statusText =
    statusKey === "critical"
      ? t(
          summary.riskCounts.critical === 1
            ? "dashboard.quota.deckStatusCriticalCountOne"
            : "dashboard.quota.deckStatusCriticalCount",
          {
            count: summary.riskCounts.critical,
          }
        )
      : statusKey === "warning"
        ? t(
            summary.riskCounts.warning === 1
              ? "dashboard.quota.deckStatusWarningCountOne"
              : "dashboard.quota.deckStatusWarningCount",
            {
              count: summary.riskCounts.warning,
            }
          )
        : t("dashboard.quota.deckStatusHealthy");

  const worst = summary.worstRow;
  const worstTone = worst ? quotaTone(worst.remainingPct) : undefined;
  const next = summary.nextReset;
  const nextIn = next ? countdownCore(next.resetAt, nowMs) : null;
  const coveragePct =
    (summary.sourceCount / QUOTA_COCKPIT_SOURCES.length) * 100;

  return (
    <section className="db-qc-console" aria-label={t("dashboard.quota")}>
      {/* 左侧一组：分段圆环 + 标题 / 副标题 / 状态计数行 */}
      <div className="db-qc-hero">
        <SegmentRing
          counts={summary.riskCounts}
          averagePct={average}
          label={`${t("dashboard.quota.deckEyebrow")} · ${
            averageRounded == null ? "—" : `${averageRounded}%`
          }`}
        />
        <div className="db-qc-hero__copy">
          <div className="db-qc-hero__status">{statusText}</div>
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

      {/* 右侧一组：三列态势读数（列间竖向发丝线；窄面板换到左侧下方） */}
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
                background: "var(--db-accent)",
              }}
            />
          </div>
        </div>
      </div>
    </section>
  );
}
