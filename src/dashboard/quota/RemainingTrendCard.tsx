/**
 * 剩余额度趋势图（额度驾驶舱区块②的曲线主体）：单系列 SVG 折线，
 * 视觉沿用对比页 QuotaTrendCard 的语言（固定 viewBox 960×280 手写坐标系、
 * 横向网格 + 纵轴刻度、深色悬停浮层、采样点超限不画圆），复用其
 * db-linechart / db-tip 样式类。
 *
 * 与对比页的两个口径差异：
 * - 纵轴是**剩余 %**（= 100 − 已用）：窗口重置时曲线跳回 100、随消耗下行，
 *   锯齿形态自然呈现（本页统一剩余口径，见 model.ts 头注释）；
 * - 单系列展示（本页曲线跟随「系列 + 窗口」选择器），无图例开关。
 *
 * 悬停取值为「阶梯语义」——显示该时刻之前最近一次采样值（额度是采样量，
 * 两点之间沿用上一次读数才是真实含义，不做线性插值），与对比页一致。
 */
import { useId, useMemo, useState } from "react";
import { formatResetStamp } from "../../format";
import { useI18n } from "../../i18n";
import { Skeleton } from "../primitives";
import { dateLabel } from "../compare/model";
import { decimateRemainingPoints, type RemainingPoint } from "./model";

// 画布常量：viewBox 固定尺寸，实际显示宽度由 CSS 撑满卡片（与对比页同款）
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
/** 折线绘制点数上限：90 天全量采样可能上万点，等间隔抽稀后路径仍保留形态 */
const DRAW_LIMIT = 800;

export function RemainingTrendCard({
  points,
  fromMs,
  toMs,
  color,
  pending = false,
}: {
  /** 升序采样点（剩余 % 口径，model.remainingPointsOf 的输出） */
  points: RemainingPoint[];
  fromMs: number;
  toMs: number;
  /** 系列色（AGENT_COLOR 的源色） */
  color: string;
  /** 本地历史未就绪（首屏骨架） */
  pending?: boolean;
}) {
  const { t } = useI18n();
  const [hoverRatio, setHoverRatio] = useState<number | null>(null);
  // SVG 渐变 id：useId 自带冒号，url(#…) 会解析失败，先滤成合法字符
  const rawId = useId();
  const fillId = `db-qc-area-${rawId.replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const drawPoints = useMemo(
    () => decimateRemainingPoints(points, DRAW_LIMIT),
    [points]
  );

  const span = Math.max(1, toMs - fromMs);
  const xOf = (ts: number) => PAD_L + ((ts - fromMs) / span) * PLOT_W;
  const yOf = (pct: number) =>
    PAD_T + (1 - Math.min(Math.max(pct, 0), 100) / 100) * PLOT_H;

  const hoverTs = hoverRatio == null ? null : fromMs + hoverRatio * span;
  // 阶梯取值：该时刻之前最近一次采样（无则 null，不做事后回填）。
  // 用抽稀后的序列做悬停查找，避免上万点序列在每次 mousemove 上线性扫描。
  let hoverPoint: RemainingPoint | null = null;
  if (hoverTs != null) {
    for (const point of drawPoints) {
      if (point.ts > hoverTs) break;
      hoverPoint = point;
    }
  }

  if (pending && points.length === 0) {
    // 本地历史未就绪：卡片内骨架（形状与折线图近似，减少布局跳动）
    return (
      <div className="db-skeleton-bars">
        {[42, 66, 35, 78, 52, 88, 61, 46, 72, 58, 40, 84].map((h, i) => (
          <div key={i} className="db-skeleton-bars__col">
            <Skeleton style={{ height: `${h}%` }} />
          </div>
        ))}
      </div>
    );
  }
  if (points.length === 0) {
    return <div className="db-empty">{t("dashboard.quota.trendEmpty")}</div>;
  }

  const path = drawPoints
    .map(
      (point, index) =>
        `${index === 0 ? "M" : "L"}${xOf(point.ts).toFixed(1)} ${yOf(
          point.remaining
        ).toFixed(1)}`
    )
    .join(" ");
  const last = drawPoints[drawPoints.length - 1];
  const areaPath = `${path} L ${xOf(last.ts).toFixed(1)} ${(
    PAD_T + PLOT_H
  ).toFixed(1)} L ${xOf(drawPoints[0].ts).toFixed(1)} ${(
    PAD_T + PLOT_H
  ).toFixed(1)} Z`;

  return (
    <div className="db-linechart db-qc-linechart">
      <div className="db-linechart__plot">
        <svg
          viewBox={`0 0 ${W} ${H}`}
          className="db-linechart__svg"
          role="img"
          aria-label={t("dashboard.quota.trendTitle")}
        >
          <defs>
            <linearGradient id={fillId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={color} stopOpacity={0.24} />
              <stop offset="100%" stopColor={color} stopOpacity={0} />
            </linearGradient>
          </defs>

          {/* 告急带（剩余 0–20%）与 20% 警戒线：驾驶舱的“红区”语义 */}
          <rect
            x={PAD_L}
            y={yOf(20)}
            width={PLOT_W}
            height={yOf(0) - yOf(20)}
            className="db-linechart__danger"
          />
          <line
            x1={PAD_L}
            y1={yOf(20)}
            x2={W - PAD_R}
            y2={yOf(20)}
            className="db-linechart__threshold"
          />
          <text
            x={W - PAD_R - 4}
            y={yOf(20) - 4}
            textAnchor="end"
            className="db-linechart__threshold-label"
          >
            {t("dashboard.quota.trendDanger")}
          </text>

          {/* 横向网格与纵轴刻度（剩余 %：100 在上、0 在下） */}
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

          {/* 悬停竖线（在折线之下，不遮挡） */}
          {hoverRatio != null && (
            <line
              x1={PAD_L + hoverRatio * PLOT_W}
              y1={PAD_T}
              x2={PAD_L + hoverRatio * PLOT_W}
              y2={PAD_T + PLOT_H}
              className="db-linechart__hover-line"
            />
          )}

          {/* 剩余曲线：渐变面积 + 折线 + 采样点 + 当前值端点 */}
          <g>
            <path d={areaPath} fill={`url(#${fillId})`} />
            <path
              d={path}
              fill="none"
              stroke={color}
              strokeWidth={1.8}
              strokeLinejoin="round"
              strokeLinecap="round"
            />
            {drawPoints.length <= DOT_LIMIT &&
              drawPoints.map((point) => (
                <circle
                  key={point.ts}
                  cx={xOf(point.ts)}
                  cy={yOf(point.remaining)}
                  r={2.4}
                  fill={color}
                />
              ))}
            {/* 最新采样端点：外圈半透明光晕 + 实心点（仪表盘的“当前读数”） */}
            <circle
              cx={xOf(last.ts)}
              cy={yOf(last.remaining)}
              r={6}
              fill={color}
              opacity={0.16}
            />
            <circle
              cx={xOf(last.ts)}
              cy={yOf(last.remaining)}
              r={2.8}
              fill={color}
              stroke="var(--db-card-bg)"
              strokeWidth={1.2}
            />
          </g>

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
          // 浮层定位与对比页同款：中段跟随鼠标，两端贴边
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
            <div className="db-tip__title num">{formatResetStamp(hoverTs)}</div>
            <div className="db-tip__row db-tip__row--strong">
              <span className="db-tip__label">{t("dashboard.quota.remaining")}</span>
              <span className="num">
                {hoverPoint == null
                  ? t("dashboard.sync.noData")
                  : `${Math.round(hoverPoint.remaining)}%`}
              </span>
            </div>
          </div>
        )}
      </div>

      <p className="db-linechart__foot">{t("dashboard.quota.trendFootnote")}</p>
    </div>
  );
}
