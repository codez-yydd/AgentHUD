/**
 * 额度圆形仪表（仪表阵列卡内）：SVG 弧线表盘——从底部左侧 135° 起顺时针
 * 扫 270°、底部留 90° 开口（无整圈形态，开口下方是窗口标签与倒计时），
 * 剩余百分数居中显示，弧线按状态档位着色（tone 由调用方经 quota/tone.ts
 * 的三档语义令牌传入）。
 *
 * 口径：传入的 pct 一律是**剩余 %**（0-100），与额度驾驶舱整页一致。
 * 本组件只负责画表盘，不做风险分级（风险标签由调用方按 model.riskOf 渲染）。
 * （控制台旧 150px 刻度表盘的 hero 形态已随深色控制台一并移除——顶栏
 * 改用 CockpitDeck 内的中性分段圆环。）
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

export interface QuotaDialProps {
  /** 剩余百分比（0-100） */
  pct: number;
  /** 显示尺寸（px，正方形） */
  size: number;
  /** 弧线色（调用方经 quota/tone.ts 按风险档位取语义令牌） */
  tone: string;
  /** 无障碍名称（如「Codex 周额度 67%」） */
  label: string;
}

export function QuotaDial({ pct, size, tone, label }: QuotaDialProps) {
  const clamped = Math.min(Math.max(pct, 0), 100);
  const valuePath = arcPath(0, clamped);

  return (
    <div
      className="db-qc-dial"
      style={{ width: size, height: size, "--qc-tone": tone } as CSSProperties}
      role="img"
      aria-label={label}
    >
      <svg viewBox="0 0 120 120" className="db-qc-dial__svg" aria-hidden>
        <path className="db-qc-dial__track" d={TRACK_PATH} />
        {valuePath && <path className="db-qc-dial__arc" d={valuePath} />}
      </svg>

      <div className="db-qc-dial__center">
        <span className="db-qc-dial__value num">
          {Math.round(clamped)}
          <small>%</small>
        </span>
      </div>
    </div>
  );
}
