/**
 * 界面字号配置：以「基准字号（逻辑 px）」驱动一套有层级的文字大小。
 *
 * 引擎选择：沿用面板既有的 #root transform: scale(--ui-scale)。原因：
 *  - 项目历史样式大量写死 px（text-[9px] / .section-title 11px / 卡片内 8~26px
 *    等），只改 html { font-size } 对它们完全无效；transform 会等比缩放**所有**
 *    已写死的字号、行高、按钮与必要间距，标题 / 核心数字 / 辅助文字的相对层级
 *    保持不变；
 *  - 主窗口此前未接入缩放，本版把同一套 --ui-scale 消费点补到 dashboard.css，
 *    两窗同源；
 *  - 缩放后布局宽度按 1/scale 补偿，视觉窗口尺寸不变，文字变大的同时可用宽度
 *    变窄，因而长标题会让位换行、网格按更窄断点重排（而不是把内容顶出窗口）。
 *
 * 基准字号与老档位的迁移（老用户升级后视觉不变）：
 *  - 旧 zbar-ui-scale（0.9 / 1 / 1.1 / 1.25）按 13px 基准折算为
 *    11.7 / 13 / 14.3 / 16.25 逻辑 px，scale = px / 13 与原值逐值相等；
 *  - 主窗口旧版没有缩放，默认固定 13px（scale = 1），不会被面板历史档位
 *    连带放大；需要一致时打开「与悬浮面板使用相同字号」。
 *
 * 范围：10~24 逻辑 px，按 0.5px 调整；ZCode 注入统计条独立保存（9~24px）。
 * 配置异常（非数字 / 越界 / localStorage 不可用）一律回退默认 13px。
 */
import { emit, listen } from "@tauri-apps/api/event";

/** 面板 / 主窗口的字号目标 */
export type FontTarget = "panel" | "dashboard";

/** 基准字号对应 13px 时的 scale = 1（历史整体缩放基准） */
export const FONT_BASE_PX = 13;
/** 应用界面字号范围（逻辑 px） */
export const FONT_MIN_PX = 10;
export const FONT_MAX_PX = 24;
/** 增减步进（逻辑 px） */
export const FONT_STEP_PX = 0.5;
/** 默认基准字号 */
export const DEFAULT_FONT_PX = 13;

/** 字号持久化键（两窗独立） */
export const PANEL_FONT_KEY = "zbar-font-panel";
export const DASHBOARD_FONT_KEY = "zbar-font-dashboard";
/** 主窗口跟随面板字号（默认关闭：保持升级前主窗口 13px 观感） */
export const DASHBOARD_FONT_LINK_KEY = "zbar-font-dashboard-link";
/** 旧版整体缩放键（与 appearance.ts 的 UI_SCALE_KEY 同值；仅迁移读取，
 *  不再写入。此处独立声明以保持本模块可被 node 单测直接加载） */
const LEGACY_UI_SCALE_KEY = "zbar-ui-scale";

/** 字号变更广播：其他已打开窗口重读并应用 */
export const FONT_CHANGED_EVENT = "zbar://ui-font-changed";
/** 同窗口内字号变更事件（Settings 页预览 / 提交后即时刷新控件） */
export const FONT_LOCAL_EVENT = "zbar-font-local";

/** 档位：数值与旧整体缩放档位一一对应，保证升级后视觉不变 */
export const FONT_PRESETS: ReadonlyArray<{
  id: "small" | "standard" | "large" | "xl";
  px: number;
  labelKey:
    | "settings.fontSmall"
    | "settings.fontStandard"
    | "settings.fontLarge"
    | "settings.fontXl";
}> = [
  { id: "small", px: 11.7, labelKey: "settings.fontSmall" },
  { id: "standard", px: 13, labelKey: "settings.fontStandard" },
  { id: "large", px: 14.3, labelKey: "settings.fontLarge" },
  { id: "xl", px: 16.25, labelKey: "settings.fontXl" },
];

/** 数值 → 展示文本：整数不带小数，最多两位（16.25 这类旧档位必须原样显示） */
export function formatFontPx(px: number): string {
  if (Number.isInteger(px)) return String(px);
  const one = Math.round(px * 10) / 10;
  return Math.abs(one - px) < 0.001 ? one.toFixed(1) : px.toFixed(2);
}

/** 校验并夹取字号：非数字 / 越界分别回退或夹到边界 */
export function sanitizeFontPx(value: unknown, fallback = DEFAULT_FONT_PX): number {
  const n = typeof value === "number" ? value : parseFloat(String(value));
  if (!Number.isFinite(n)) return fallback;
  if (n < FONT_MIN_PX) return FONT_MIN_PX;
  if (n > FONT_MAX_PX) return FONT_MAX_PX;
  return Math.round(n * 100) / 100;
}

/** 是否在合法范围内（用于输入校验；越界给提示而不是静默夹取） */
export function isValidFontPx(value: number): boolean {
  return (
    Number.isFinite(value) && value >= FONT_MIN_PX && value <= FONT_MAX_PX
  );
}

/** 基准字号 → 整体缩放（保留 4 位，避免 16.25/13 的浮点尾差） */
export function fontScaleOf(px: number): number {
  return Math.round((px / FONT_BASE_PX) * 10000) / 10000;
}

/** 步进：在 0.5px 网格上跳一档（老档位的 11.7 等先吸附到最近网格） */
export function stepFontPx(px: number, direction: 1 | -1): number {
  const snapped = Math.round(px / FONT_STEP_PX) * FONT_STEP_PX;
  const base = Math.abs(snapped - px) < 0.001 ? px : snapped;
  return sanitizeFontPx(base + direction * FONT_STEP_PX);
}

/** 当前宿主窗口类型（dashboard.html 为主窗口，其余是悬浮面板） */
export function detectFontTarget(): FontTarget {
  try {
    return /(^|\/)dashboard\.html$/.test(location.pathname)
      ? "dashboard"
      : "panel";
  } catch {
    return "panel";
  }
}

function readItem(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeItem(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等 */
  }
}

/**
 * 读取面板基准字号：优先新键；旧版仅有 zbar-ui-scale 时按 13px 折算迁移。
 * 迁移结果不主动落盘——用户未改字号前始终按旧档位等比还原，改档后才写新键。
 */
export function loadPanelFontPx(): number {
  const raw = readItem(PANEL_FONT_KEY);
  if (raw != null) {
    const parsed = parseFloat(raw);
    if (Number.isFinite(parsed) && isValidFontPx(parsed)) {
      return Math.round(parsed * 100) / 100;
    }
  }
  const legacy = parseFloat(readItem(LEGACY_UI_SCALE_KEY) ?? "");
  if (Number.isFinite(legacy) && legacy > 0) {
    return sanitizeFontPx(Math.round(legacy * FONT_BASE_PX * 100) / 100);
  }
  return DEFAULT_FONT_PX;
}

/** 读取主窗口基准字号：旧版主窗口无缩放，默认 13px */
export function loadDashboardFontPx(): number {
  const raw = readItem(DASHBOARD_FONT_KEY);
  if (raw != null) {
    const parsed = parseFloat(raw);
    if (Number.isFinite(parsed) && isValidFontPx(parsed)) {
      return Math.round(parsed * 100) / 100;
    }
  }
  return DEFAULT_FONT_PX;
}

/** 主窗口是否跟随面板字号（默认关，保持升级前主窗口观感） */
export function loadDashboardFontLink(): boolean {
  return readItem(DASHBOARD_FONT_LINK_KEY) === "1";
}

/** 解析某窗口最终生效的基准字号 */
export function resolveFontPx(target: FontTarget): number {
  if (target === "panel") return loadPanelFontPx();
  return loadDashboardFontLink()
    ? loadPanelFontPx()
    : loadDashboardFontPx();
}

/** 应用某窗口的字号（写 --ui-scale，纯 DOM，不落盘） */
export function applyFontPx(target: FontTarget, px: number): void {
  const clean = sanitizeFontPx(px);
  try {
    const root = document.documentElement;
    root.style.setProperty("--ui-scale", String(fontScaleOf(clean)));
    root.setAttribute("data-font-px", formatFontPx(clean));
    root.setAttribute("data-font-target", target);
    // 布局分档：窄面板在基准字号 ≥14px 后需要让工具条/时间范围换行，
    // 避免 11 个 22px 图标按钮按 1/scale 补偿后溢出无法点击
    root.setAttribute(
      "data-font-tier",
      clean >= 20 ? "xl" : clean >= 16 ? "lg" : clean >= 14 ? "md" : "sm"
    );
  } catch {
    /* 忽略：非浏览器环境 */
  }
}

/** 按当前窗口 + 已保存配置应用字号（启动首帧调用） */
export function applyFontSettings(): void {
  const target = detectFontTarget();
  applyFontPx(target, resolveFontPx(target));
}

/** 持久化面板字号 */
export function persistPanelFontPx(px: number): void {
  writeItem(PANEL_FONT_KEY, String(sanitizeFontPx(px)));
}

/** 持久化主窗口字号 */
export function persistDashboardFontPx(px: number): void {
  writeItem(DASHBOARD_FONT_KEY, String(sanitizeFontPx(px)));
}

/** 持久化「主窗口跟随面板」开关 */
export function persistDashboardFontLink(linked: boolean): void {
  writeItem(DASHBOARD_FONT_LINK_KEY, linked ? "1" : "0");
}

/**
 * 应用并保存字号（本窗口立即生效），并广播给其他已打开窗口。
 * 主窗口联动开启时只保存面板值，主窗口按面板值重新解析。
 */
export function commitPanelFontPx(px: number): number {
  const clean = sanitizeFontPx(px);
  persistPanelFontPx(clean);
  const target = detectFontTarget();
  if (target === "panel" || loadDashboardFontLink()) {
    applyFontPx(target, target === "panel" ? clean : loadPanelFontPx());
  }
  broadcastFontChange();
  return clean;
}

/** 应用并保存主窗口字号 */
export function commitDashboardFontPx(px: number): number {
  const clean = sanitizeFontPx(px);
  persistDashboardFontPx(clean);
  if (detectFontTarget() === "dashboard" && !loadDashboardFontLink()) {
    applyFontPx("dashboard", clean);
  }
  broadcastFontChange();
  return clean;
}

/** 应用并保存联动开关 */
export function commitDashboardFontLink(linked: boolean): void {
  persistDashboardFontLink(linked);
  if (detectFontTarget() === "dashboard") {
    applyFontPx("dashboard", resolveFontPx("dashboard"));
  }
  broadcastFontChange();
}

/** 恢复默认：面板 / 主窗口各自回到 13px；联动保持原值 */
export function resetFontPx(target: FontTarget): number {
  if (target === "panel") {
    persistPanelFontPx(DEFAULT_FONT_PX);
    if (detectFontTarget() === "panel" || loadDashboardFontLink()) {
      applyFontPx(detectFontTarget(), DEFAULT_FONT_PX);
    }
  } else {
    persistDashboardFontPx(DEFAULT_FONT_PX);
    if (detectFontTarget() === "dashboard") {
      applyFontPx("dashboard", resolveFontPx("dashboard"));
    }
  }
  broadcastFontChange();
  return DEFAULT_FONT_PX;
}

/** 预览：只写 DOM 不落盘（输入过程中实时看效果） */
export function previewFontPx(target: FontTarget, px: number): void {
  applyFontPx(target, px);
}

/** 取消预览 / 输入非法时恢复已保存值 */
export function revertFontPreview(target: FontTarget): void {
  applyFontPx(target, resolveFontPx(target));
}

/** 广播字号变更：同窗口自定义事件 + Tauri 跨窗口事件（失败静默） */
export function broadcastFontChange(): void {
  try {
    window.dispatchEvent(new CustomEvent(FONT_LOCAL_EVENT));
  } catch {
    /* 忽略 */
  }
  emit(FONT_CHANGED_EVENT).catch(() => {
    /* 非 Tauri 宿主 / 广播失败：本窗口已生效 */
  });
}

/**
 * 监听其他窗口 / 其他设置页的字号变更并重新应用（启动时挂一次）。
 * 返回取消函数；Tauri 监听失败静默（单窗口场景不受影响）。
 */
export function watchFontChanges(): () => void {
  const reapply = () => {
    // 保存值才是最终事实：其他窗口只广播「变了」，本窗自己重读配置
    applyFontSettings();
  };
  try {
    window.addEventListener(FONT_LOCAL_EVENT, reapply);
  } catch {
    /* 忽略 */
  }
  let dispose: (() => void) | undefined;
  let cancelled = false;
  listen(FONT_CHANGED_EVENT, reapply)
    .then((unlisten) => {
      if (cancelled) unlisten();
      else dispose = unlisten;
    })
    .catch(() => {
      /* 非 Tauri 宿主：只用同窗口事件 */
    });
  return () => {
    cancelled = true;
    try {
      window.removeEventListener(FONT_LOCAL_EVENT, reapply);
    } catch {
      /* 忽略 */
    }
    dispose?.();
  };
}
