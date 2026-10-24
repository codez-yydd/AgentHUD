/**
 * 额度圆形仪表（驾驶舱视觉核心）：270° 扫角、底部开口的 SVG 表盘，
 * 剩余百分比用同源剩余色着色（深色控制台用提亮版 remainingAccentColor，
 * 亮色卡片用 remainingTextColor），中心大号数字 + 环形刻度。
 *
 * 口径：传入的 pct 一律是**剩余 %**（0-100），与额度驾驶舱整页一致。
 * 本组件只负责画表盘，不做风险分级（风险标签由调用方按 model.riskOf 渲染）。
 */
import type { CSSProperties } from "react";

const CX = 60;
const CY = 60;
const R = 48;
/** 表盘从底部左侧 135° 起顺时针扫 270°，底部留 90° 开口 */
const START_ANGLE = -135;
const SWEEP = 270;

/** 极坐标 → SVG 坐标（0° 在正上方，顺时针为正） */
function polar(angle: number, radius: number): [number, number] {
  const rad = (angle * Math.PI) / 180;
  return [CX + Math.sin(rad) * radius, CY - Math.cos(rad) * radius];
}

/** 0-100 的百分比 → 表盘圆弧路径；from/to 相同（0 段）时返回空串 */
function arcPath(from: number, to: number, radius = R): string {
  const clampedFrom = Math.min(Math.max(from, 0), 100);
  const clampedTo = Math.min(Math.max(to, 0), 100);
  if (clampedTo - clampedFrom < 0.01) return "";
  const a0 = START_ANGLE + (SWEEP * clampedFrom) / 100;
  const a1 = START_ANGLE + (SWEEP * clampedTo) / 100;
  const [x0, y0] = polar(a0, radius);
  const [x1, y1] = polar(a1, radius);
  const largeArc = Math.abs(a1 - a0) > 180 ? 1 : 0;
  return `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${radius} ${radius} 0 ${largeArc} 1 ${x1.toFixed(
    2
  )} ${y1.toFixed(2)}`;
}

const TRACK_PATH = arcPath(0, 100);

/** 环形刻度：21 个点（每 5% 一个），5 的倍数为主刻度 */
const TICKS = Array.from({ length: 21 }, (_, index) => {
  const pct = index * 5;
  const angle = START_ANGLE + (SWEEP * pct) / 100;
  const major = index % 5 === 0;
  const [x0, y0] = polar(angle, R - (major ? 12 : 10));
  const [x1, y1] = polar(angle, R - 6);
  return { pct, major, x0, y0, x1, y1 };
});

export interface QuotaDialProps {
  /** 剩余百分比（0-100） */
  pct: number;
  /** 显示尺寸（px，正方形） */
  size: number;
  /** 仪表色（调用方按主题/深色底选 remainingTextColor / remainingAccentColor） */
  tone: string;
  /** 无障碍名称（如「Codex 周额度 67%」） */
  label: string;
  /** 深色控制台大表盘：更粗的弧 + 刻度 + 20% 警戒刻度 */
  hero?: boolean;
}

export function QuotaDial({ pct, size, tone, label, hero = false }: QuotaDialProps) {
  const clamped = Math.min(Math.max(pct, 0), 100);
  const valuePath = arcPath(0, clamped);
  const thresholdAngle = START_ANGLE + (SWEEP * 20) / 100;
  const [tx0, ty0] = polar(thresholdAngle, R - 13);
  const [tx1, ty1] = polar(thresholdAngle, R + 1);

  return (
    <div
      className={`db-qc-dial ${hero ? "db-qc-dial--hero" : ""}`}
      style={{ width: size, height: size, "--qc-tone": tone } as CSSProperties}
      role="img"
      aria-label={label}
    >
      <svg viewBox="0 0 120 120" className="db-qc-dial__svg" aria-hidden>
        <path className="db-qc-dial__track" d={TRACK_PATH} />
        {valuePath && <path className="db-qc-dial__arc" d={valuePath} />}

        {hero && (
          <>
            {TICKS.map((tick) => (
              <line
                key={tick.pct}
                x1={tick.x0}
                y1={tick.y0}
                x2={tick.x1}
                y2={tick.y1}
                className={`db-qc-dial__tick ${
                  tick.major ? "db-qc-dial__tick--major" : ""
                }`}
              />
            ))}
            {/* 20% 警戒刻度：比普通刻度更醒目的一小段红色标记 */}
            <line
              x1={tx0}
              y1={ty0}
              x2={tx1}
              y2={ty1}
              className="db-qc-dial__threshold"
            />
          </>
        )}
      </svg>

      <div className="db-qc-dial__center">
        <span
          className="db-qc-dial__value num"
          style={{ fontSize: Math.round(size * (hero ? 0.26 : 0.28)) }}
        >
          {Math.round(clamped)}
          <small>%</small>
        </span>
      </div>
    </div>
  );
}
