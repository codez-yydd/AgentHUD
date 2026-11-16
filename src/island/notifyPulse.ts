/**
 * 新消息「岛端先动」用到的时序常量与纯函数（无 DOM / 无 React 依赖，可直接 node 单测）。
 *
 * 数值按界面实测标定：
 *   - 呼吸灯：5s 总时长；BREATH_STOPS 的渐变剖面只落在 island.css（本文件不重复声明）。
 *   - 浅色品牌色单独取值，比品牌主色更亮。
 *   - 1.2s 的「惊讶先行、呼吸灯与活动卡后到」编排按本次任务约定。
 */

import type { NotifyItem } from "./types";

/** 静默窗口：同一波次里相邻两条消息的间隔上限（含边界，毫秒）。 */
export const NOTIFY_WAVE_MS = 1200;
/** 惊讶到呼吸灯 / 活动卡的提前量：t=0 惊讶，t=1200 亮灯 + 弹卡。 */
export const NOTIFY_LEAD_MS = 1200;
/** 呼吸灯总时长（参考 breath(glow, 5.0)）。 */
export const NOTIFY_PULSE_MS = 5000;
/** 活动卡时长（沿用 showActivity 既有的 4000ms）。 */
export const NOTIFY_ACTIVITY_MS = 4000;
/** 呼吸频率 0.5Hz：一个呼吸周期 2s（参考 elapsed * tau * 0.5）。 */
export const NOTIFY_PULSE_PERIOD_MS = 2000;
/** 整体透明度包络：起始 450ms / 结束前 1100ms 走 smoothstep（参考 0.45 / 1.1s）。 */
export const NOTIFY_PULSE_FADE_IN_MS = 450;
export const NOTIFY_PULSE_FADE_OUT_MS = 1100;

/** 呼吸振幅 = 0.26 + 0.74 × 波形（参考 env * (0.26 + 0.74 * wave)）。 */
const NOTIFY_PULSE_FLOOR = 0.26;
const NOTIFY_PULSE_SWING = 0.74;

/** 新消息呼吸灯用的浅色品牌色（参考 KIND_LIGHT_COLORS，比品牌色更亮）。 */
export const NOTIFY_LIGHT_COLORS: Record<string, string> = {
  qq: "#8fd3ff",
  wechat: "#8ff0dd",
  mail: "#ffc58a",
  feishu: "#c9b8ff",
  dingtalk: "#ffe08a",
  dsh: "#b9c6ff",
};

/** 一波新消息：首条到达时间固定成呼吸灯的相位起点，续期只改 latestAt。 */
export interface NotifyWave {
  /** 波次首条到达时间（performance.now 基准） */
  startedAt: number;
  /** 波次里最后一条的到达时间（滑动窗口与呼吸灯续期按它算） */
  latestAt: number;
  /** 波次里最新的一条（活动卡内容） */
  latest: NotifyItem;
}

export function clamp01(value: number): number {
  return value < 0 ? 0 : value > 1 ? 1 : value;
}

/** 与参考 anim.py 的 smooth() 同一曲线：t²(3-2t)。 */
export function smoothstep(value: number): number {
  const t = clamp01(value);
  return t * t * (3 - 2 * t);
}

/**
 * 新消息是否并入当前波次：
 * - 通知卡还在屏上（activityKind === "notify" 且有波次记录）：并入——卡片盖着胶囊，
 *   重播惊讶也看不见；
 * - 否则看静默窗口：距上一条不超过 NOTIFY_WAVE_MS（含边界）就并入。
 * 并入时 startedAt 不变，呼吸灯相位因此连续。
 */
export function joinNotifyWave(
  current: NotifyWave | null,
  item: NotifyItem,
  now: number
): { wave: NotifyWave; coalesced: boolean } {
  if (current && now - current.latestAt <= NOTIFY_WAVE_MS) {
    return { wave: { ...current, latestAt: now, latest: item }, coalesced: true };
  }
  return { wave: { startedAt: now, latestAt: now, latest: item }, coalesced: false };
}

/** 收到新消息后的编排分支。 */
export type NotifyReactionMode = "ignore" | "replace" | "continue" | "start";

/**
 * 新消息的编排决策（纯函数，便于单测）：
 * - ignore：展开态，角标由 notify.items 派生，不弹任何收起提醒；
 * - replace：已有其它 activity（音量 / 股票），或通知卡还在但本波次已无记录
 *   （卡片是被既有覆盖语义弹出来的）→ 沿用 showActivity 的覆盖语义；
 * - continue：同一波次（通知卡还在屏上，或距上一条不超过 NOTIFY_WAVE_MS）→
 *   不重播惊讶、不重排活动卡，只续期；
 * - start：新波次 → t=0 惊讶，t=1200 呼吸灯 + 活动卡。
 */
export function decideNotifyReaction(input: {
  open: boolean;
  /** 当前 activity 的 kind；null = 屏上没有 activity */
  activityKind: string | null;
  wave: NotifyWave | null;
  item: NotifyItem;
  now: number;
}): { mode: NotifyReactionMode; wave: NotifyWave | null } {
  if (input.open) return { mode: "ignore", wave: input.wave };
  if (input.activityKind) {
    if (input.activityKind === "notify" && input.wave) {
      return {
        mode: "continue",
        wave: { ...input.wave, latestAt: input.now, latest: input.item },
      };
    }
    return { mode: "replace", wave: input.wave };
  }
  const joined = joinNotifyWave(input.wave, input.item, input.now);
  return { mode: joined.coalesced ? "continue" : "start", wave: joined.wave };
}

/**
 * 呼吸灯窗口：首条 + NOTIFY_LEAD_MS 亮起，至少亮 NOTIFY_PULSE_MS；
 * 同一波次后续消息把结束时间推到「最后一条 + NOTIFY_PULSE_MS」，相位起点不动。
 */
export function notifyPulseWindow(wave: NotifyWave): { startAt: number; endAt: number } {
  const startAt = wave.startedAt + NOTIFY_LEAD_MS;
  return { startAt, endAt: Math.max(startAt, wave.latestAt) + NOTIFY_PULSE_MS };
}

/**
 * 某一时刻的呼吸灯强度 k（0 = 熄灭，1 = 最亮）：
 * env = smoothstep(已亮 / 450ms) × smoothstep(剩余 / 1100ms)，
 * wave = 0.5 - 0.5cos(2π × 已亮 / 2000ms)（0.5Hz），k = env × (0.26 + 0.74 × wave)。
 * 入参用「已亮毫秒 + 总时长毫秒」，续期只改总时长，已亮部分不变 → 相位连续。
 */
export function notifyPulseIntensity(elapsedMs: number, totalMs: number): number {
  if (!(totalMs > 0)) return 0;
  const elapsed = Math.max(0, Math.min(elapsedMs, totalMs));
  const remain = totalMs - elapsed;
  const env =
    smoothstep(elapsed / NOTIFY_PULSE_FADE_IN_MS) *
    smoothstep(remain / NOTIFY_PULSE_FADE_OUT_MS);
  if (env <= 0.01) return 0;
  const wave = 0.5 - 0.5 * Math.cos((elapsed / NOTIFY_PULSE_PERIOD_MS) * Math.PI * 2);
  const intensity = env * (NOTIFY_PULSE_FLOOR + NOTIFY_PULSE_SWING * wave);
  return intensity <= 0.01 ? 0 : intensity;
}

/** 把 #rrggbb 往白色混 amount（0..1），用于给其它 kind 的品牌色做「同样浅化」。 */
export function lightenHex(color: string, amount = 0.55): string {
  const hex = color.trim().replace(/^#/, "");
  if (!/^[0-9a-fA-F]{6}$/.test(hex)) return color;
  const mix = (index: number) => {
    const start = parseInt(hex.slice(index, index + 2), 16);
    const value = start + (255 - start) * clamp01(amount);
    return Math.round(Math.max(0, Math.min(255, value)))
      .toString(16)
      .padStart(2, "0");
  };
  return `#${mix(0)}${mix(2)}${mix(4)}`;
}

/**
 * 呼吸灯颜色：已知来源用参考项目的浅色品牌色；其它 kind 把 KIND_COLORS 的同名颜色
 * 往白色浅化；都没有就回退主题 accent（由调用方传入，例如 "var(--accent)"）。
 */
export function notifyPulseColor(
  kind: string | undefined,
  brandColors: Record<string, string>,
  accent: string
): string {
  const key = kind || "other";
  const light = NOTIFY_LIGHT_COLORS[key];
  if (light) return light;
  const brand = brandColors[key];
  if (brand) return lightenHex(brand);
  return accent;
}
