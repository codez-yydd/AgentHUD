/**
 * 额度趋势卡（大窗版独有）：所选范围内各额度系列的周额度「已用 %」随时间
 * 变化的 SVG 多序列折线图。
 *
 * 视觉沿用总览页 TrendCard 的语言（卡片头 + 图例 + 深色悬停浮层 + 底部
 * 脚注），但不引图表库：坐标系用固定 viewBox（960×280）手写，容器宽度变化
 * 时整体等比缩放。悬停取值为「阶梯语义」——显示该时刻之前最近一次采样值
 * （额度是采样量，两点之间沿用上一次读数才是真实含义，不做线性插值）。
 * 图例两种交互：点击开关系列显隐（aria-pressed，系列多（Z.ai 多账号）时
 * 避免互相压线，可经「显示全部」重置）；悬停 / 键盘聚焦某图例项临时聚焦
 * ——该系列满不透明度、其余降到 0.35，六条线挤在一起时也能读出单条。
 * 采样点默认不画圆点，只在悬停竖线处由实心大点标出读数位置。
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
/** 图例聚焦时其余系列的折线不透明度（聚焦系列保持满值 1） */
const DIM_OPACITY = 0.35;

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
  // 图例两级交互：hidden 为点击显隐（点击切换、可重置）；hoveredId 为
  // 悬停 / 键盘聚焦的临时聚焦（聚焦系列满不透明度、其余降 DIM_OPACITY），
  // 被隐藏的系列不参与绘制也不受聚焦影响。聚焦只对可见系列生效：悬停 /
  // 键盘聚焦已隐藏的图例项时不算聚焦，否则 focusId 对所有可见系列都不
  // 成立，可见折线会被整体降到 DIM_OPACITY
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const [hoverRatio, setHoverRatio] = useState<number | null>(null);

  const visible = series.filter((item) => !hidden.has(item.id));
  const hasPoints = visible.some((item) => item.points.length > 0);
  const focusId = hoveredId != null && !hidden.has(hoveredId) ? hoveredId : null;
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
      className="db-card--legend-row"
      title={t("dashboard.compare.trendTitle")}
      sub={t("dashboard.compare.trendSub", { range: rangeLabel })}
      action={
        <div className="db-legend db-legend--wrap">
          {series.map((item) => (
            <button
              key={item.id}
              type="button"
              onClick={() => toggle(item.id)}
              onMouseEnter={() => setHoveredId(item.id)}
              onMouseLeave={() => setHoveredId(null)}
              onFocus={() => setHoveredId(item.id)}
              onBlur={() => setHoveredId(null)}
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

              {/* 各系列折线（1.5px 线；默认不画采样点圆点——六线同图时
                  逐点圆点太花，读数位置由悬停竖线处的实心大点标出）。
                  图例聚焦时聚焦系列满不透明度、其余降到 DIM_OPACITY */}
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
                  <path
                    key={item.id}
                    d={path}
                    fill="none"
                    stroke={item.color}
                    strokeWidth={1.5}
                    strokeLinejoin="round"
                    strokeLinecap="round"
                    strokeOpacity={
                      focusId == null || focusId === item.id ? 1 : DIM_OPACITY
                    }
                  />
                );
              })}

              {/* 悬停读数层：竖线位置各系列的实心大点（阶梯取值落到
                  最近一次采样点，读数与大点一一对应；非聚焦系列同步降
                  不透明度，与折线保持同一层级） */}
              {hoverRatio != null &&
                hoverTs != null &&
                visible.map((item) => {
                  const snapped = lastAt(item.points, hoverTs);
                  if (!snapped) return null;
                  return (
                    <circle
                      key={`hover:${item.id}`}
                      cx={xOf(snapped.ts)}
                      cy={yOf(snapped.usedPct)}
                      r={3.2}
                      fill={item.color}
                      stroke="var(--db-card-bg)"
                      strokeWidth={1.2}
                      strokeOpacity={
                        focusId == null || focusId === item.id ? 1 : DIM_OPACITY
                      }
                    />
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
