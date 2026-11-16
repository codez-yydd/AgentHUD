/**
 * 收起态「小形象」：用 SVG + rAF 实现。
 *
 * 设计要点：
 * - 全部状态用阻尼弹簧（视线 ex/ey 140/0.8、下眼睑 lh/rh 160/0.75、困倦
 *   tired 60/1.0、放大 wide 220/0.45、特殊形状 morph 200/0.7、腮红
 *   blush 120/0.9），弹簧积分用 1/240s 固定子步长（见 motion.ts 的
 *   advanceSpring）：rAF 帧时长只决定子步数量，不直接进积分，掉帧也不发散。
 * - 动画只在挂载且 active=true 时跑：展开面板 / 页面 hidden 时组件卸载或
 *   active=false，rAF 立即 cancel；收起态不在后台空转。
 * - 不用 CSS 关键帧拼表情：每一帧都由弹簧值算 SVG 几何。
 *
 * 表情与触发（同参考）：
 * - 常态东张西望；好奇时看哪边哪只眼变高；60s 没人理变困倦；随机眨眼。
 * - 新消息：惊讶 1.4s（wide 弹簧给速度冲量）。
 * - 鼠标靠近：随机 7 种表情（happy/wink/love/star/shy/pout/squint，不与上次重复）。
 * - 按压：两眼外扩 + 向外倾斜、本体压扁并向没被按的一侧偏。
 */

import { useEffect, useMemo, useRef, useState } from "react";

/** 参考 anim.py：阻尼弹簧，固定 1/240s 子步长积分。 */
class Spring {
  x: number;
  v = 0;
  target: number;
  private k: number;
  private c: number;
  private eps: number;

  constructor(value = 0, stiffness = 300, ratio = 0.8, eps = 0.05) {
    this.x = value;
    this.target = value;
    this.k = stiffness;
    this.c = 2 * ratio * Math.sqrt(stiffness);
    this.eps = eps;
  }

  to(target: number) {
    this.target = target;
  }

  step(dt: number) {
    if (this.rest) {
      this.x = this.target;
      this.v = 0;
      return;
    }
    const n = Math.max(1, Math.ceil(dt / (1 / 240)));
    const h = dt / n;
    for (let i = 0; i < n; i += 1) {
      const a = -this.k * (this.x - this.target) - this.c * this.v;
      this.v += a * h;
      this.x += this.v * h;
    }
  }

  get rest() {
    return Math.abs(this.x - this.target) < this.eps && Math.abs(this.v) < this.eps * 8;
  }
}

function clamp(value: number, min: number, max: number): number {
  return value < min ? min : value > max ? max : value;
}

/** 收起态鼠标采样：相对胶囊中心的像素偏移 + 指针是否在窗口内 */
export interface CompactPointerSample {
  x: number;
  y: number;
  inside: boolean;
}

type Mood = "default" | "happy" | "wink" | "love" | "star" | "shy" | "pout" | "squint" | "surprise";
const MOODS: Mood[] = ["happy", "wink", "love", "star", "shy", "pout", "squint"];

interface Frame {
  mood: Mood;
  tired: number;
  wide: number;
  morph: number;
  blush: number;
  lh: number;
  rh: number;
  lx: number;
  ly: number;
  blink: number;
  bounce: number;
  now: number;
  lean: number;
  spread: number;
  tilt: number;
  squashX: number;
  squashY: number;
}

const INITIAL_FRAME: Frame = {
  mood: "default",
  tired: 0,
  wide: 0,
  morph: 0,
  blush: 0,
  lh: 0,
  rh: 0,
  lx: 0,
  ly: 0,
  blink: 0,
  bounce: 0,
  now: 0,
  lean: 0,
  spread: 0,
  tilt: 0,
  squashX: 1,
  squashY: 1,
};

/** 爱心路径（同参考 _heart） */
function heartPath(cx: number, cy: number, size: number): string {
  const w = size;
  return [
    `M ${cx} ${cy + w * 0.42}`,
    `C ${cx - w * 0.9} ${cy - w * 0.05} ${cx - w * 0.45} ${cy - w * 0.75} ${cx} ${cy - w * 0.28}`,
    `C ${cx + w * 0.45} ${cy - w * 0.75} ${cx + w * 0.9} ${cy - w * 0.05} ${cx} ${cy + w * 0.42}`,
    "Z",
  ].join(" ");
}

/** 四角星（同参考 _sparkle） */
function sparklePath(cx: number, cy: number, r: number): string {
  return [
    `M ${cx} ${cy - r}`,
    `Q ${cx} ${cy} ${cx + r} ${cy}`,
    `Q ${cx} ${cy} ${cx} ${cy + r}`,
    `Q ${cx} ${cy} ${cx - r} ${cy}`,
    `Q ${cx} ${cy} ${cx} ${cy - r}`,
    "Z",
  ].join(" ");
}

export function PetFace({
  active,
  pointerRef,
  surpriseToken = 0,
  pressed = false,
}: {
  /** false = 暂停 rAF（展开 / 页面隐藏 / 切到其它视图） */
  active: boolean;
  pointerRef?: React.MutableRefObject<CompactPointerSample>;
  /** 每 +1 表示一条新消息：惊讶 1.4s */
  surpriseToken?: number;
  /** 按住胶囊：两眼外扩倾斜、本体压扁 */
  pressed?: boolean;
}) {
  const [, setTick] = useState(0);
  const frameRef = useRef<Frame>(INITIAL_FRAME);
  const pressedRef = useRef(pressed);
  const pressRelRef = useRef(0);
  const surpriseUntilRef = useRef(0);
  const surpriseTokenRef = useRef(surpriseToken);

  const springs = useMemo(
    () => ({
      ex: new Spring(0, 140, 0.8, 0.01),
      ey: new Spring(0, 140, 0.8, 0.01),
      lh: new Spring(0, 160, 0.75, 0.003),
      rh: new Spring(0, 160, 0.75, 0.003),
      tired: new Spring(0, 60, 1.0, 0.003),
      wide: new Spring(0, 220, 0.45, 0.003),
      morph: new Spring(0, 200, 0.7, 0.003),
      blush: new Spring(0, 120, 0.9, 0.003),
      press: new Spring(0, 220, 0.7, 0.004),
    }),
    []
  );

  const meta = useMemo(
    () => ({
      near: false,
      chosenMood: "default" as Mood,
      lastMood: "" as Mood | "",
      blinkAt: performance.now() / 1000 + 2.5,
      lastAttention: performance.now() / 1000,
    }),
    []
  );

  pressedRef.current = pressed;

  // 新消息 → 惊讶 1.4s：wide 弹簧给速度冲量（同参考 react("surprise")）
  useEffect(() => {
    if (surpriseToken === surpriseTokenRef.current) return;
    surpriseTokenRef.current = surpriseToken;
    if (surpriseToken === 0) return;
    springs.wide.v += 9;
    surpriseUntilRef.current = performance.now() / 1000 + 1.4;
    meta.lastAttention = performance.now() / 1000;
  }, [surpriseToken, springs, meta]);

  useEffect(() => {
    if (!active) return;
    let raf = 0;
    let alive = true;
    let last = performance.now();
    let previousPress = false;

    const stepFrame = (nowMs: number) => {
      const now = nowMs / 1000;
      const dt = clamp((nowMs - last) / 1000, 1 / 240, 0.05);
      last = nowMs;
      const ptr = pointerRef?.current ?? { x: 9999, y: 9999, inside: false };
      // 「靠近」判定同参考：胶囊半宽 + 40 / 半高 + 40（以中心为原点）
      const near = ptr.inside && Math.abs(ptr.x) < 90 && Math.abs(ptr.y) < 55;

      if (near && !meta.near) {
        const choices = MOODS.filter((mood) => mood !== meta.lastMood);
        meta.chosenMood = choices[Math.floor(Math.random() * choices.length)];
        meta.lastMood = meta.chosenMood;
      } else if (!near && meta.near) {
        meta.chosenMood = "default";
      }
      meta.near = near;
      if (near || pressedRef.current) meta.lastAttention = now;
      if (pressedRef.current && !previousPress) {
        pressRelRef.current = clamp(ptr.x / 75, -1, 1);
      }
      previousPress = pressedRef.current;

      // 有效表情：惊讶优先级最高；鼠标远离时回到默认
      let mood: Mood = near ? meta.chosenMood : "default";
      if (now < surpriseUntilRef.current) mood = "surprise";

      // 视线：鼠标近 → 看鼠标；远 → 东张西望（同参考的正弦漂移）
      let tx: number;
      let ty: number;
      if (!near && Math.abs(ptr.x) > 420) {
        tx = Math.sin(now * 0.55) * 0.8 + Math.sin(now * 1.7) * 0.15;
        ty = Math.sin(now * 0.37) * 0.35;
      } else {
        tx = clamp(ptr.x / 90, -1, 1);
        ty = clamp(ptr.y / 60, -1, 1);
      }
      if (mood === "shy") {
        tx = tx * 0.3 + 0.5;
        ty = 0.75;
      }
      springs.ex.to(tx);
      springs.ey.to(ty);
      springs.lh.to(mood === "wink" ? 0 : mood === "happy" ? 1 : mood === "shy" ? 0.55 : 0);
      springs.rh.to(mood === "happy" ? 1 : mood === "shy" ? 0.55 : 0);
      springs.tired.to(now - meta.lastAttention > 60 && mood === "default" ? 0.55 : 0);
      springs.wide.to(mood === "star" ? 0.3 : mood === "surprise" ? 0.35 : 0);
      springs.morph.to(mood === "love" || mood === "squint" || mood === "pout" ? 1 : 0);
      springs.blush.to(
        mood === "shy" ? 1 : mood === "love" ? 0.8 : mood === "happy" ? 0.5 : mood === "wink" ? 0.5 : 0
      );
      springs.press.to(pressedRef.current ? 1 : 0);

      for (const spring of [
        springs.ex,
        springs.ey,
        springs.lh,
        springs.rh,
        springs.tired,
        springs.wide,
        springs.morph,
        springs.blush,
        springs.press,
      ]) {
        spring.step(dt);
      }

      // 眨眼：特殊表情时不眨（同参考）
      const blinkDur = 0.16 + 0.18 * springs.tired.x;
      if (now >= meta.blinkAt + blinkDur) {
        const gap = 2.4 + (Math.sin(now * 12.9898) * 0.5 + 0.5) * 3.2;
        meta.blinkAt = now + gap * (springs.tired.x > 0.3 ? 1.6 : 1.0);
      }
      let blink = 0;
      if (
        meta.blinkAt <= now &&
        now < meta.blinkAt + blinkDur &&
        (mood === "default" || mood === "pout" || mood === "shy" || mood === "star")
      ) {
        blink = Math.sin(((now - meta.blinkAt) / blinkDur) * Math.PI);
      }

      const press = clamp(springs.press.x, 0, 1);
      const sideW = Math.abs(pressRelRef.current);
      frameRef.current = {
        mood,
        tired: clamp(springs.tired.x, 0, 1),
        wide: clamp(springs.wide.x, -0.3, 1),
        morph: clamp(springs.morph.x, 0, 1),
        blush: clamp(springs.blush.x, 0, 1),
        lh: clamp(springs.lh.x, 0, 1),
        rh: clamp(springs.rh.x, 0, 1),
        lx: springs.ex.x,
        ly: springs.ey.x,
        blink,
        bounce: mood === "happy" ? Math.sin(now * 16) * 1.6 : 0,
        now,
        lean: -pressRelRef.current * Math.min(1, press * 1.6),
        spread: press * (1 - sideW) * 9,
        tilt: press * (1 - sideW) * 14,
        squashX: 1 + 0.05 * press,
        squashY: 1 - 0.16 * press,
      };
      setTick((value) => (value + 1) % 1_000_000);
      if (alive) raf = requestAnimationFrame(stepFrame);
    };

    raf = requestAnimationFrame(stepFrame);
    return () => {
      alive = false;
      cancelAnimationFrame(raf);
    };
  }, [active, meta, pointerRef, springs]);

  const f = frameRef.current;
  const cx = 32;
  const cy = 17;
  const baseW = 11;
  const baseH = 15;
  const gap = 15;
  const pink = "#ff5c8a";
  const glowClass = "pet-glowable";

  const eyes = ([-1, 1] as const).map((side) => {
    const hap = side < 0 ? f.lh : f.rh;
    const curious = 1 + 0.22 * Math.max(0, f.lx * side);
    const w = baseW * (1 + 0.12 * f.wide) * (1 + 0.25 * f.blink);
    const h = Math.max(1.6, baseH * curious * (1 + 0.18 * f.wide) * (1 - 0.88 * f.blink));
    const ex = cx + side * (gap / 2 + baseW / 2 + f.spread / 2) + f.lx * 5;
    const ey = cy + f.ly * 3 + f.bounce;
    const tilt = Math.abs(f.tilt) > 0.05 ? f.tilt : 0;
    return { side, ex, ey, w, h, hap, tilt };
  });

  return (
    <svg
      className="pet-face"
      viewBox="0 0 64 34"
      width={56}
      height={34}
      role="img"
      aria-label="灵动岛小形象"
    >
      <g
        transform={`translate(${cx + f.lean * 24} ${cy}) rotate(${f.lean * 18}) scale(${f.squashX} ${f.squashY}) translate(${-cx} ${-cy})`}
      >
        {eyes.map(({ side, ex, ey, w, h, hap, tilt }) => {
          // 按压时每只眼以自己中心向外倾斜
          const rotate = tilt ? `rotate(${side * tilt} ${ex} ${ey})` : undefined;
          if (f.mood === "love" && f.morph > 0.05) {
            const pulse = 1 + 0.08 * Math.sin(f.now * 7);
            return (
              <path
                key={side}
                d={heartPath(ex, ey, 15 * pulse * (0.6 + 0.4 * f.morph))}
                fill={pink}
                opacity={f.morph}
                className={glowClass}
              />
            );
          }
          if (f.mood === "squint" && f.morph > 0.05) {
            const d = -side;
            const k = 5.5 * f.morph;
            return (
              <polyline
                key={side}
                points={`${ex - d * k},${ey - k} ${ex + d * k},${ey} ${ex - d * k},${ey + k}`}
                fill="none"
                stroke="currentColor"
                strokeWidth={2.6}
                strokeLinecap="round"
                strokeLinejoin="round"
                className={glowClass}
              />
            );
          }
          const rx = Math.min(w, h) * 0.42;
          const maskId = `pet-eye-mask-${side}`;
          const left = ex - w / 2;
          const top = ey - h / 2;
          const cutRadius = w * 0.9;
          return (
            <g key={side} transform={rotate}>
              <defs>
                <mask
                  id={maskId}
                  maskUnits="userSpaceOnUse"
                  x={left - 12}
                  y={top - 12}
                  width={w + 24}
                  height={h + 24}
                >
                  <rect x={left} y={top} width={w} height={h} rx={rx} fill="#fff" />
                  {f.tired > 0.01 ? (
                    <rect
                      x={left - 2}
                      y={top - 2}
                      width={w + 4}
                      height={2 + h * 0.5 * f.tired}
                      fill="#000"
                    />
                  ) : null}
                  {f.mood === "pout" && f.morph > 0.05 ? (
                    <rect
                      x={-w}
                      y={-h}
                      width={w * 2.2}
                      height={h}
                      fill="#000"
                      transform={`translate(${ex - side * w * 0.1} ${top + h * 0.32 * f.morph}) rotate(${side * -24 * f.morph})`}
                    />
                  ) : null}
                  {hap > 0.01 ? (
                    <circle cx={ex} cy={top + h + cutRadius - h * 0.62 * hap} r={cutRadius} fill="#000" />
                  ) : null}
                </mask>
              </defs>
              <rect
                x={left}
                y={top}
                width={w}
                height={h}
                rx={rx}
                ry={rx}
                fill="currentColor"
                mask={`url(#${maskId})`}
                className={glowClass}
              />
              {f.mood === "star" && h > 6 ? (
                <>
                  <path d={sparklePath(ex + 1.4, ey - 2.2, 3.4)} fill="var(--pet-sparkle, #14141a)" />
                  <path d={sparklePath(ex - 2.2, ey + 3.0, 1.6)} fill="var(--pet-sparkle, #14141a)" />
                </>
              ) : null}
            </g>
          );
        })}

        {/* 腮红 */}
        {f.blush > 0.02
          ? eyes.map(({ side, ex, ey }) => (
              <ellipse
                key={`blush-${side}`}
                cx={ex + side * 7.5}
                cy={ey + baseH * 0.55}
                rx={4.4}
                ry={2.4}
                fill={pink}
                opacity={0.59 * f.blush}
              />
            ))
          : null}

        {/* 嘴（同参考各表情） */}
        {(() => {
          const mx = cx + f.lx * 5;
          const my = cy + baseH * 0.66 + f.ly * 3 + f.bounce;
          const common = {
            fill: "none",
            stroke: "currentColor",
            strokeWidth: 1.8,
            strokeLinecap: "round" as const,
            opacity: 0.9,
          };
          if (f.mood === "happy" || f.mood === "wink" || f.mood === "love" || f.mood === "shy") {
            return <path d={`M ${mx - 3.5} ${my} Q ${mx} ${my + 4.2} ${mx + 3.5} ${my}`} {...common} />;
          }
          if (f.mood === "pout") {
            return <line x1={mx - 3} y1={my + 1.5} x2={mx + 3} y2={my + 1.5} {...common} />;
          }
          if (f.mood === "squint") {
            return <path d={`M ${mx - 3} ${my} Q ${mx} ${my + 3.5} ${mx + 3} ${my}`} {...common} />;
          }
          if (f.mood === "star" || f.mood === "surprise") {
            return <ellipse cx={mx} cy={my + 1.5} rx={2.4} ry={2.8} fill="currentColor" opacity={0.9} />;
          }
          return null;
        })()}
      </g>
    </svg>
  );
}
