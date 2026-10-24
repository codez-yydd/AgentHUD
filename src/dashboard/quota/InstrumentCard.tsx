/**
 * 仪表阵列中的单张系列卡（一个账号 / 凭证）：品牌徽标 + 套餐 + 各额度窗口
 * 的圆形仪表 + 重置倒计时 + 主窗口 90 天迷你曲线。
 *
 * 口径：仪表与曲线都是剩余 %（= 100 − 已用）。卡片按「最紧窗口」着色，
 * 并把风险标签放在迷你曲线标题行，颜色之外还有文字兜底。
 */
import { useMemo } from "react";
import type { CSSProperties } from "react";
import { BrandIcon } from "../../BrandIcon";
import { levelLabel } from "../../format";
import type { TFn } from "../../i18n";
import { QuotaDial } from "./QuotaDial";
import {
  countdownCore,
  formatDateTime,
  riskLabel,
  timeAgo,
  windowLabel,
} from "./format";
import {
  currentRowsInOrderOf,
  decimateRemainingPoints,
  latestSampleAtOf,
  primaryWindowOf,
  remainingPointsOf,
  riskOf,
  sourceBrandOf,
  sourceColorOf,
  worstRemainingOf,
  type CockpitSeries,
  type RemainingPoint,
} from "./model";
import { quotaTone } from "./tone";

/** 系列卡里的仪表尺寸：窗口越少表盘越大；4 个以上允许换行 */
function dialSizeOf(windowCount: number): number {
  if (windowCount <= 1) return 96;
  if (windowCount === 2) return 84;
  if (windowCount === 3) return 68;
  return 58;
}

/** 迷你剩余曲线：无坐标轴、无交互，只表达 90 天走势与当前值 */
function Sparkline({ points, tone }: { points: RemainingPoint[]; tone: string }) {
  if (points.length < 2) {
    return <div className="db-qc-spark db-qc-spark--empty" aria-hidden />;
  }
  const W = 100;
  const H = 26;
  const PAD = 1.5;
  const from = points[0].ts;
  const to = points[points.length - 1].ts;
  const span = Math.max(1, to - from);
  const xOf = (ts: number) => PAD + ((ts - from) / span) * (W - PAD * 2);
  const yOf = (pct: number) =>
    PAD + (1 - Math.min(Math.max(pct, 0), 100) / 100) * (H - PAD * 2);

  const line = points
    .map(
      (point, index) =>
        `${index === 0 ? "M" : "L"}${xOf(point.ts).toFixed(2)} ${yOf(
          point.remaining
        ).toFixed(2)}`
    )
    .join(" ");
  const area = `${line} L ${xOf(to).toFixed(2)} ${H - PAD} L ${xOf(from).toFixed(
    2
  )} ${H - PAD} Z`;

  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="none"
      className="db-qc-spark"
      aria-hidden
    >
      <path d={area} fill={tone} fillOpacity={0.12} />
      <path
        d={line}
        fill="none"
        stroke={tone}
        strokeWidth={1.3}
        strokeLinecap="round"
        strokeLinejoin="round"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

export function InstrumentCard({
  series,
  nowMs,
  dark,
  t,
}: {
  series: CockpitSeries;
  nowMs: number;
  dark: boolean;
  t: TFn;
}) {
  const rows = currentRowsInOrderOf(series);
  const worst = worstRemainingOf(series);
  const risk = riskOf(worst ?? 100);
  const primaryKey = primaryWindowOf(series);
  // 曲线数据按 series 身份缓存：页面每 30s 只更新 nowMs，不该为此重新映射
  // 整个 90 天采样序列（可能上万点 × 每张卡）
  const primaryPoints = useMemo(
    () => remainingPointsOf(series, primaryKey),
    [series, primaryKey]
  );
  // 迷你曲线最多画 96 个点：避免 90 天全量采样撑大 SVG
  const sparkPoints = useMemo(
    () => decimateRemainingPoints(primaryPoints, 96),
    [primaryPoints]
  );
  const primaryLatest = primaryPoints[primaryPoints.length - 1]?.remaining ?? worst;
  const latestAt = latestSampleAtOf(series);
  // 超过 24h 未采样的卡在页头元信息里转琥珀色并标注「数据较旧」：
  // 仪表读数仍然照常展示，但用户不该把陈旧读数当成实时状态
  const stale = latestAt != null && nowMs - latestAt > 24 * 60 * 60 * 1000;
  const size = dialSizeOf(rows.length);
  // 套餐标识：zai 走统一的 lite/pro/max 词条；其它通道的 plan_type 多为小写
  // 英文（plus/free/team），这里统一首字母大写，避免卡片右上角出现小写胶囊
  const plan = series.plan
    ? series.source === "zai"
      ? levelLabel(series.plan)
      : series.plan.charAt(0).toUpperCase() + series.plan.slice(1)
    : null;
  const sourceColor = sourceColorOf(series.source);

  return (
    <article
      className={`db-qc-instrument db-qc-instrument--${risk}`}
      style={{ "--qc-source": sourceColor } as CSSProperties}
    >
      <header className="db-qc-instrument__head">
        <span className="db-qc-instrument__brand" style={{ color: sourceColor }}>
          <BrandIcon brand={sourceBrandOf(series.source)} className="h-4 w-4" />
        </span>
        <div className="db-qc-instrument__titles">
          <h3 className="db-qc-instrument__title" title={series.label}>
            {series.label}
          </h3>
          <span
            className={`db-qc-instrument__meta ${
              stale ? "db-qc-instrument__meta--stale" : ""
            }`}
            title={latestAt != null ? formatDateTime(latestAt) : undefined}
          >
            {t("dashboard.quota.updatedAgo", {
              time: timeAgo(t, latestAt, nowMs),
            })}
            {stale ? ` · ${t("dashboard.quota.staleData")}` : ""}
          </span>
        </div>
        {plan && <span className="db-qc-plan">{plan}</span>}
      </header>

      <div
        className={`db-qc-gauges ${
          rows.length <= 1 ? "db-qc-gauges--single" : ""
        }`}
      >
        {rows.map((row) => {
          const tone = quotaTone(row.remainingPct, dark);
          const countdown =
            row.resetAt != null ? countdownCore(row.resetAt, nowMs) : null;
          return (
            <div className="db-qc-gauge" key={`${series.id}:${row.windowKey}`}>
              <QuotaDial
                pct={row.remainingPct}
                size={size}
                tone={tone}
                label={`${series.label} · ${windowLabel(t, row.windowKey)} · ${Math.round(
                  row.remainingPct
                )}%`}
              />
              <span className="db-qc-gauge__label">
                <span className="db-qc-gauge__window">
                  {windowLabel(t, row.windowKey)}
                </span>
                <span
                  className="db-qc-gauge__reset"
                  title={
                    row.resetAt != null ? formatDateTime(row.resetAt) : undefined
                  }
                >
                  {countdown
                    ? `↻ ${t("dashboard.quota.resetIn", { time: countdown })}`
                    : t("dashboard.quota.windowNoReset")}
                </span>
              </span>
            </div>
          );
        })}
      </div>

      <footer className="db-qc-instrument__foot">
        <div className="db-qc-instrument__spark-head">
          <span className="db-qc-instrument__spark-label">
            {windowLabel(t, primaryKey)} · {t("dashboard.overview.range.90d")}
          </span>
          <span className={`db-qc-risk db-qc-risk--${risk}`}>
            {riskLabel(t, risk)}
          </span>
        </div>
        <Sparkline
          points={sparkPoints}
          tone={quotaTone(primaryLatest ?? 100, dark)}
        />
      </footer>
    </article>
  );
}
