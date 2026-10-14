/**
 * 额度趋势卡（大窗版独有）：所选范围内各额度系列的周额度「已用 %」随时间
 * 变化的 SVG 多序列折线图。
 *
 * 视觉沿用总览页 TrendCard 的语言（卡片头 + 图例 + 深色悬停浮层 + 底部
 * 脚注），但不引图表库：坐标系用固定 viewBox（960×280）手写，容器宽度变化
 * 时整体等比缩放。悬停取值为「阶梯语义」——显示该时刻之前最近一次采样值
 * （额度是采样量，两点之间沿用上一次读数才是真实含义，不做线性插值）。
 * 图例可点击开关系列显隐，系列多（Z.ai 多账号）时避免互相压线。
 */
import { useState } from "react";
import { formatResetStamp } from "../../format";
import { useI18n } from "../../i18n";
import { Card, Skeleton } from "../primitives";
import { dateLabel, type QuotaSample } from "./model";

/** 折线图单系列视图（颜色已在模型层按系列档位算好） */
export interface TrendSeriesView {
  id: string;
  label: string;
  color: string;
  /** 已按范围切片的升序采样点 */
  points: QuotaSample[];
}

// 画布常量：viewBox 固定尺寸，实际显示宽度由 CSS 撑满卡片
const W = 960;
const H = 280;
const PAD_L = 46;
const PAD_R = 14;
const PAD_T = 12;
const PAD_B = 26;
const PLOT_W = W - PAD_L - PAD_R;
const PLOT_H = H - PAD_T - PAD_B;
const GRID_PCTS = [0, 25, 50, 75, 100];
/** 采样点数超过该值时不再逐点画圆（避免密集成团） */
const DOT_LIMIT = 60;

export function QuotaTrendCard({
  series,
  fromMs,
  toMs,
  rangeLabel,
  note,
  pending = false,
}: {
  series: TrendSeriesView[];
  fromMs: number;
  toMs: number;
  /** 页头当前范围文案（卡片副标题） */
  rangeLabel: string;
  /** 补充说明（自定义区间下说明额度快照保留期；预设档不传） */
  note?: string;
  /** 本机额度快照未就绪（首屏卡片骨架） */
  pending?: boolean;
}) {
  const { t } = useI18n();
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [hoverRatio, setHoverRatio] = useState<number | null>(null);

  const visible = series.filter((item) => !hidden.has(item.id));
  const hasPoints = visible.some((item) => item.points.length > 0);
  const span = Math.max(1, toMs - fromMs);
  const xOf = (ts: number) => PAD_L + ((ts - fromMs) / span) * PLOT_W;
  const yOf = (pct: number) => PAD_T + (1 - Math.min(Math.max(pct, 0), 100) / 100) * PLOT_H;

  const toggle = (id: string) => {
    setHidden((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  // 悬停时刻与各可见系列的阶梯取值（该时刻之前最近一次采样）
  const hoverTs = hoverRatio == null ? null : fromMs + hoverRatio * span;
  const hoverRows =
    hoverTs == null
      ? []
      : visible.map((item) => ({
          id: item.id,
          label: item.label,
          color: item.color,
          value: lastAt(item.points, hoverTs),
        }));

  return (
    <Card
      title={t("dashboard.compare.trendTitle")}
      sub={t("dashboard.compare.trendSub", { range: rangeLabel })}
      action={
        <div className="db-legend db-legend--wrap">
          {series.map((item) => (
            <button
              key={item.id}
              type="button"
              onClick={() => toggle(item.id)}
              aria-pressed={!hidden.has(item.id)}
              title={t("dashboard.compare.trendLegendHint")}
              className={`db-legend__btn ${
                hidden.has(item.id) ? "db-legend__btn--off" : ""
              }`}
            >
              <i className="db-swatch" style={{ background: item.color }} />
              <span className="db-legend__btn-label">{item.label}</span>
            </button>
          ))}
          {hidden.size > 0 && (
            <button
              type="button"
              className="db-legend__reset"
              onClick={() => setHidden(new Set())}
            >
              {t("dashboard.compare.trendShowAll")}
            </button>
          )}
        </div>
      }
    >
      {pending && !hasPoints ? (
        // 本机额度历史未就绪：卡片内骨架（形状与折线图近似，减少布局跳动）
        <div className="db-skeleton-bars">
          {[42, 66, 35, 78, 52, 88, 61, 46, 72, 58, 40, 84].map((h, i) => (
            <div key={i} className="db-skeleton-bars__col">
              <Skeleton style={{ height: `${h}%` }} />
            </div>
          ))}
        </div>
      ) : series.length === 0 || !hasPoints ? (
        <div className="db-empty">{t("dashboard.compare.trendEmpty")}</div>
      ) : (
        <div className="db-linechart">
          <div className="db-linechart__plot">
            <svg
              viewBox={`0 0 ${W} ${H}`}
              className="db-linechart__svg"
              role="img"
              aria-label={t("dashboard.compare.trendTitle")}
            >
              {/* 横向网格与纵轴刻度（已用 %） */}
              {GRID_PCTS.map((pct) => (
                <g key={pct}>
                  <line
                    x1={PAD_L}
                    y1={yOf(pct)}
                    x2={W - PAD_R}
                    y2={yOf(pct)}
                    className="db-linechart__grid"
                  />
                  <text
                    x={PAD_L - 8}
                    y={yOf(pct) + 3.5}
                    textAnchor="end"
                    className="db-linechart__label num"
                  >
                    {pct}
                  </text>
                </g>
              ))}

              {/* 横轴刻度：5 等分时间点 */}
              {[0, 1, 2, 3, 4].map((i) => {
                const ts = fromMs + (span * i) / 4;
                const isEdge = i === 0 || i === 4;
                return (
                  <text
                    key={i}
                    x={xOf(ts)}
                    y={H - 8}
                    textAnchor={isEdge ? (i === 0 ? "start" : "end") : "middle"}
                    className="db-linechart__label num"
                  >
                    {dateLabel(ts)}
                  </text>
                );
              })}

              {/* 悬停竖线（在各序列之下，不遮挡折线） */}
              {hoverRatio != null && (
                <line
                  x1={PAD_L + hoverRatio * PLOT_W}
                  y1={PAD_T}
                  x2={PAD_L + hoverRatio * PLOT_W}
                  y2={PAD_T + PLOT_H}
                  className="db-linechart__hover-line"
                />
              )}

              {/* 各系列折线 + 采样点 */}
              {visible.map((item) => {
                if (item.points.length === 0) return null;
                const path = item.points
                  .map(
                    (point, index) =>
                      `${index === 0 ? "M" : "L"}${xOf(point.ts).toFixed(1)} ${yOf(
                        point.usedPct
                      ).toFixed(1)}`
                  )
                  .join(" ");
                return (
                  <g key={item.id}>
                    <path
                      d={path}
                      fill="none"
                      stroke={item.color}
                      strokeWidth={1.8}
                      strokeLinejoin="round"
                      strokeLinecap="round"
                    />
                    {item.points.length <= DOT_LIMIT &&
                      item.points.map((point) => (
                        <circle
                          key={point.ts}
                          cx={xOf(point.ts)}
                          cy={yOf(point.usedPct)}
                          r={2.4}
                          fill={item.color}
                        />
                      ))}
                  </g>
                );
              })}

              {/* 事件层：只覆盖绘图区，换算比例即为时间比例 */}
              <rect
                x={PAD_L}
                y={PAD_T}
                width={PLOT_W}
                height={PLOT_H}
                fill="transparent"
                onMouseMove={(e) => {
                  const rect = e.currentTarget.getBoundingClientRect();
                  setHoverRatio(
                    Math.min(
                      1,
                      Math.max(0, (e.clientX - rect.left) / Math.max(1, rect.width))
                    )
                  );
                }}
                onMouseLeave={() => setHoverRatio(null)}
              />
            </svg>

            {hoverRatio != null && hoverTs != null && (
              // 浮层横向定位：中段用 left:% + translateX(-50%) 跟随鼠标；
              // 两端改由 db-tip--left/--right 贴边（此时不写行内 left，
              // 否则行内优先级会压掉类里的 left:auto/right:0 造成右缘溢出）。
              // 纵向由 .db-tip 默认规则贴绘图区顶部（不向上压卡片头）
              <div
                className={`db-tip ${
                  hoverRatio < 0.12
                    ? "db-tip--left"
                    : hoverRatio > 0.88
                      ? "db-tip--right"
                      : ""
                }`}
                style={
                  hoverRatio < 0.12 || hoverRatio > 0.88
                    ? undefined
                    : {
                        left: `${(
                          ((PAD_L + hoverRatio * PLOT_W) / W) *
                          100
                        ).toFixed(2)}%`,
                      }
                }
              >
                <div className="db-tip__title num">
                  {formatResetStamp(hoverTs)}
                </div>
                {hoverRows.map((row) => (
                  <div key={row.id} className="db-tip__row">
                    <span className="db-tip__label">{row.label}</span>
                    <span className="num">
                      {row.value == null
                        ? t("dashboard.compare.matrixNoData")
                        : `${Math.round(row.value.usedPct)}%`}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>

          <p className="db-linechart__foot">{t("dashboard.compare.trendFootnote")}</p>
          {note != null && <p className="db-linechart__foot">{note}</p>}
        </div>
      )}
    </Card>
  );
}

/** 阶梯取值：该时刻之前最近一次采样（无则 null，不做事后回填）。 */
function lastAt(points: QuotaSample[], ts: number): QuotaSample | null {
  let found: QuotaSample | null = null;
  for (const point of points) {
    if (point.ts > ts) break;
    found = point;
  }
  return found;
}
