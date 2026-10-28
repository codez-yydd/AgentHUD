/**
 * 界面布局配置（卡片高度 / 折叠 / 展开 / 显示方案）的单一数据源。
 *
 * 设计约束（与产品要求一一对应）：
 *  - 每张卡片有稳定 ID，按「窗口 + 页面」分别落盘，互不影响；
 *  - 高度区分「自适应」（height = null）与「用户指定高度」（逻辑 px）；
 *  - 配置带版本号；读取时做白名单校验，未知卡片 / 损坏值 / 越界值一律回退
 *    默认，不因一份坏配置让页面打不开；
 *  - 经典 / 简洁 / 自定义三种方案共用同一份卡片状态；切换方案只改 scheme，
 *    用户自定义过的布局另存一份快照（custom），不会被方案切换销毁；
 *  - 新增卡片追加在顺序末尾，已定制过的简洁布局不会擅自多出卡片
 *    （customized 时新卡默认隐藏，由用户自行添加）。
 *
 * 本文件保持纯函数 / localStorage 读写（不依赖 React、不直接依赖 Tauri），
 * 便于用 node --experimental-strip-types 做迁移与校验逻辑的单元验证。
 */

/** 窗口类型：悬浮面板 / 主窗口（大窗） */
export type UiWindow = "panel" | "dashboard";

/** 显示方案：经典（升级默认）/ 简洁（三块核心）/ 自定义（用户自由编排） */
export type LayoutScheme = "classic" | "simple" | "custom";

/** 卡片展示级别：主要信息（精简）/ 完整信息（展开次要字段） */
export type CardDetail = "primary" | "full";

/** 卡片能力：决定该卡在「调整显示」和卡片本身上提供哪些操作 */
export interface CardCapabilities {
  /** 拖动底边改高度 / 恢复自适应 */
  resize: boolean;
  /** 折叠为标题 + 摘要 */
  collapse: boolean;
  /** 列表卡片「展开全部 / 收起」（取消内部高度限制，改页面整体滚动） */
  expandList: boolean;
  /** 主要信息 / 完整信息 */
  detail: boolean;
  /** 允许在自定义方案中隐藏 */
  hide: boolean;
}

/** 卡片注册项：页面渲染与「调整显示」共用同一份定义 */
export interface CardDefinition {
  id: string;
  window: UiWindow;
  page: string;
  /** i18n 键（「调整显示」列表里的名称） */
  labelKey: string;
  /** 默认顺序（升序） */
  order: number;
  /** 经典方案默认显示 */
  classic: boolean;
  /** 简洁方案默认显示 */
  simple: boolean;
  /** 合理最小高度（逻辑 px；字号变大后由组件按真实标题高度再抬升） */
  minHeight: number;
  caps: CardCapabilities;
}

const ALL_CAPS: CardCapabilities = {
  resize: true,
  collapse: true,
  expandList: false,
  detail: false,
  hide: true,
};

const LIST_CAPS: CardCapabilities = {
  ...ALL_CAPS,
  expandList: true,
};

/**
 * 统一卡片注册表：ID / 默认顺序 / 适用页面 / 简洁预设状态 / 最小高度 / 支持操作。
 * 新增卡片只改这里 + 页面渲染；旧配置里没有的 ID 由 readPageLayout 补齐。
 */
export const CARD_REGISTRY: readonly CardDefinition[] = [
  // ---- 悬浮面板 / 汇总页 ----
  {
    id: "summary.agents",
    window: "panel",
    page: "summary",
    labelKey: "customize.card.agents",
    order: 10,
    classic: true,
    simple: false,
    minHeight: 96,
    caps: ALL_CAPS,
  },
  {
    id: "summary.quota",
    window: "panel",
    page: "summary",
    labelKey: "customize.card.quota",
    order: 20,
    classic: true,
    simple: true,
    minHeight: 110,
    caps: ALL_CAPS,
  },
  {
    id: "summary.trend",
    window: "panel",
    page: "summary",
    labelKey: "customize.card.trend",
    order: 30,
    classic: true,
    simple: false,
    minHeight: 150,
    caps: ALL_CAPS,
  },
  {
    id: "summary.models",
    window: "panel",
    page: "summary",
    labelKey: "customize.card.models",
    order: 40,
    classic: true,
    simple: false,
    minHeight: 150,
    caps: LIST_CAPS,
  },
  // ---- 主窗口 / 用量总览 ----
  {
    id: "overview.metrics",
    window: "dashboard",
    page: "overview",
    labelKey: "customize.card.metrics",
    order: 10,
    classic: true,
    simple: false,
    minHeight: 96,
    caps: { ...ALL_CAPS, resize: false },
  },
  {
    id: "overview.trend",
    window: "dashboard",
    page: "overview",
    labelKey: "customize.card.trend",
    order: 20,
    classic: true,
    simple: false,
    minHeight: 220,
    caps: ALL_CAPS,
  },
  {
    id: "overview.heatmap",
    window: "dashboard",
    page: "overview",
    labelKey: "customize.card.heatmap",
    order: 30,
    classic: true,
    simple: false,
    minHeight: 150,
    caps: ALL_CAPS,
  },
  {
    id: "overview.quota",
    window: "dashboard",
    page: "overview",
    labelKey: "customize.card.quota",
    order: 40,
    classic: true,
    simple: true,
    minHeight: 150,
    caps: ALL_CAPS,
  },
  {
    id: "overview.models",
    window: "dashboard",
    page: "overview",
    labelKey: "customize.card.models",
    order: 50,
    classic: true,
    simple: false,
    minHeight: 200,
    caps: LIST_CAPS,
  },
];

/** 单卡状态（存储值） */
export interface CardLayoutState {
  /** 用户指定高度（逻辑 CSS px）；null = 自适应 */
  height: number | null;
  /** 折叠为标题 + 摘要 */
  collapsed: boolean;
  /** 列表卡展开全部（取消内部限高，页面整体滚动） */
  listExpanded: boolean;
  /** 隐藏（不渲染；采集与统计照常） */
  hidden: boolean;
  /** 展示级别 */
  detail: CardDetail;
}

/** 单页布局状态（存储值 / 解析后的完整值） */
export interface PageLayoutState {
  scheme: LayoutScheme;
  /** 卡片展示顺序（ID 列表；缺失的新卡由解析函数追加到末尾） */
  order: string[];
  cards: Record<string, CardLayoutState>;
  /** 用户是否手动改过本页布局（新卡不再自动挤进简洁布局） */
  customized: boolean;
}

interface StoredRoot {
  v: number;
  pages: Record<string, unknown>;
  /** 用户自定义布局快照（切走方案不销毁） */
  custom: Record<string, unknown>;
}

/** 配置版本：v2 起含方案 / 顺序 / 显示级别；v1 仅高度与折叠，读取时迁移 */
export const LAYOUT_VERSION = 2;

/** 布局持久化键 */
export const UI_LAYOUT_KEY = "zbar-ui-layout";

/** 同源窗口内的布局变更事件（Tauri 跨窗口事件见 notifyLayoutChanged） */
export const UI_LAYOUT_LOCAL_EVENT = "zbar-layout-local";

/** Tauri 跨窗口布局变更广播（两个窗口若打开同一页面时保持同步） */
export const UI_LAYOUT_EVENT = "zbar://ui-layout-changed";

/** 用户指定高度的安全上限：不把卡片最大高度限制为一屏，允许完整查看 */
export const MAX_CARD_HEIGHT = 5000;

/** 卡片最小高度下限（逻辑 px） */
export const MIN_CARD_HEIGHT = 56;

/** 读取卡片定义（未知 ID 返回 undefined） */
export function cardDefinition(id: string): CardDefinition | undefined {
  return CARD_REGISTRY.find((d) => d.id === id);
}

/** 某窗口 + 页面的全部卡片定义（按默认顺序） */
export function cardDefinitions(
  window: UiWindow,
  page: string
): CardDefinition[] {
  return CARD_REGISTRY.filter((d) => d.window === window && d.page === page).sort(
    (a, b) => a.order - b.order
  );
}

/** 页面存储键：窗口与页面分别保存 */
export function pageKey(window: UiWindow, page: string): string {
  return `${window}:${page}`;
}

/** 默认卡片状态：经典显示所有 classic 卡，简洁只显示 simple 卡 */
export function defaultCardState(
  def: CardDefinition,
  scheme: LayoutScheme
): CardLayoutState {
  return {
    height: null,
    collapsed: false,
    listExpanded: false,
    hidden:
      scheme === "simple"
        ? !def.simple
        : scheme === "custom"
          ? !def.classic
          : !def.classic,
    detail: "primary",
  };
}

/** 某个方案的出厂布局 */
export function defaultPageLayout(
  window: UiWindow,
  page: string,
  scheme: LayoutScheme = "classic"
): PageLayoutState {
  const defs = cardDefinitions(window, page);
  const cards: Record<string, CardLayoutState> = {};
  for (const def of defs) cards[def.id] = defaultCardState(def, scheme);
  return {
    scheme,
    order: defs.map((d) => d.id),
    cards,
    customized: false,
  };
}

/** 数值校验：有限数字、夹到 [MIN, MAX]，保留整数逻辑 px */
function sanitizeHeight(value: unknown): number | null {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  if (value < MIN_CARD_HEIGHT) return null;
  return Math.min(Math.round(value), MAX_CARD_HEIGHT);
}

function asBool(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}

function asScheme(value: unknown): LayoutScheme | null {
  return value === "classic" || value === "simple" || value === "custom"
    ? value
    : null;
}

/**
 * 把任意来源的页面配置解析为合法状态：
 *  - scheme / order / cards / customized 逐项校验；
 *  - 未知卡片 ID 丢弃；注册表里新增的卡片追加到末尾；
 *  - 卡片能力不支持的状态复位（如非列表卡 listExpanded 恒 false）。
 */
export function normalizePageLayout(
  raw: unknown,
  window: UiWindow,
  page: string
): PageLayoutState {
  const defs = cardDefinitions(window, page);
  const base = defaultPageLayout(window, page, "classic");
  if (raw == null || typeof raw !== "object") return base;
  const src = raw as Record<string, unknown>;
  const scheme = asScheme(src.scheme) ?? "classic";
  const customized = src.customized === true;

  const valid = new Set(defs.map((d) => d.id));
  const rawOrder = Array.isArray(src.order)
    ? src.order.filter((id): id is string => typeof id === "string" && valid.has(id))
    : [];
  const seen = new Set<string>();
  const order: string[] = [];
  for (const id of rawOrder) {
    if (seen.has(id)) continue;
    seen.add(id);
    order.push(id);
  }
  // 新卡追加在末尾（已定制布局里默认隐藏，见下方 hidden 默认）
  for (const def of defs) {
    if (!seen.has(def.id)) order.push(def.id);
  }

  const rawCards =
    src.cards != null && typeof src.cards === "object"
      ? (src.cards as Record<string, unknown>)
      : {};
  const cards: Record<string, CardLayoutState> = {};
  for (const def of defs) {
    const explicit = rawCards[def.id] as Record<string, unknown> | undefined;
    const fallback = defaultCardState(def, scheme);
    if (explicit == null || typeof explicit !== "object") {
      // 新增卡片：已定制的简洁 / 自定义布局不擅自显示
      const isNew = !(def.id in rawCards) && customized;
      cards[def.id] = {
        ...fallback,
        hidden: isNew ? true : fallback.hidden,
      };
      continue;
    }
    cards[def.id] = {
      height: def.caps.resize ? sanitizeHeight(explicit.height) : null,
      collapsed: def.caps.collapse
        ? asBool(explicit.collapsed, fallback.collapsed)
        : false,
      listExpanded: def.caps.expandList
        ? asBool(explicit.listExpanded, fallback.listExpanded)
        : false,
      hidden: def.caps.hide
        ? asBool(explicit.hidden, fallback.hidden)
        : fallback.hidden,
      detail: def.caps.detail
        ? explicit.detail === "full"
          ? "full"
          : "primary"
        : "primary",
    };
  }

  return { scheme, order, cards, customized };
}

/** 安全读取 localStorage 根配置（损坏 / 不可用时返回空） */
function readRoot(): StoredRoot {
  try {
    const raw = localStorage.getItem(UI_LAYOUT_KEY);
    if (!raw) return { v: LAYOUT_VERSION, pages: {}, custom: {} };
    const parsed = JSON.parse(raw) as Partial<StoredRoot> | null;
    if (parsed == null || typeof parsed !== "object") {
      return { v: LAYOUT_VERSION, pages: {}, custom: {} };
    }
    return {
      v: typeof parsed.v === "number" ? parsed.v : 1,
      pages:
        parsed.pages != null && typeof parsed.pages === "object"
          ? (parsed.pages as Record<string, unknown>)
          : {},
      custom:
        parsed.custom != null && typeof parsed.custom === "object"
          ? (parsed.custom as Record<string, unknown>)
          : {},
    };
  } catch {
    return { v: LAYOUT_VERSION, pages: {}, custom: {} };
  }
}

/** 写入根配置（失败静默：布局记忆失败不影响本次使用） */
function writeRoot(root: StoredRoot): void {
  try {
    localStorage.setItem(UI_LAYOUT_KEY, JSON.stringify(root));
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等 */
  }
}

/** 读取某页解析后的布局（无配置 = 经典出厂布局） */
export function readPageLayout(
  window: UiWindow,
  page: string
): PageLayoutState {
  const root = readRoot();
  const key = pageKey(window, page);
  // 兼容 v1：高度 / 折叠写在扁平 pages[key] 里，normalize 会按缺省补齐其余项
  return normalizePageLayout(root.pages[key], window, page);
}

/** 读取单卡状态 */
export function readCardState(
  window: UiWindow,
  page: string,
  cardId: string
): CardLayoutState {
  const def = cardDefinition(cardId);
  const layout = readPageLayout(window, page);
  return (
    layout.cards[cardId] ??
    (def ? defaultCardState(def, layout.scheme) : defaultPageLayout(window, page).cards[cardId])
  );
}

/** 保存整页布局（自动做一次解析校验，避免把坏值写盘） */
export function writePageLayout(
  window: UiWindow,
  page: string,
  layout: PageLayoutState
): PageLayoutState {
  const root = readRoot();
  const clean = normalizePageLayout(layout, window, page);
  const key = pageKey(window, page);
  root.v = LAYOUT_VERSION;
  root.pages[key] = clean;
  writeRoot(root);
  notifyLayoutChanged(window, page);
  return clean;
}

/**
 * 更新单卡状态并落盘。返回最新单页状态（供调用方同步 UI）。
 * 注意：drag 过程中的中间高度不落盘，只在 pointerup 时调用一次。
 */
export function writeCardState(
  window: UiWindow,
  page: string,
  cardId: string,
  patch: Partial<CardLayoutState>
): CardLayoutState {
  const layout = readPageLayout(window, page);
  const current =
    layout.cards[cardId] ??
    (cardDefinition(cardId)
      ? defaultCardState(cardDefinition(cardId)!, layout.scheme)
      : null);
  if (!current) return layout.cards[cardId];
  const next: PageLayoutState = {
    ...layout,
    customized: true,
    cards: { ...layout.cards, [cardId]: { ...current, ...patch } },
  };
  writePageLayout(window, page, next);
  return next.cards[cardId];
}

/** 保存 / 读取用户自定义布局快照（切换方案不销毁） */
export function saveCustomSnapshot(
  window: UiWindow,
  page: string,
  layout: PageLayoutState
): void {
  const root = readRoot();
  root.v = LAYOUT_VERSION;
  root.custom[pageKey(window, page)] = normalizePageLayout(layout, window, page);
  writeRoot(root);
}

export function readCustomSnapshot(
  window: UiWindow,
  page: string
): PageLayoutState | null {
  const root = readRoot();
  const raw = root.custom[pageKey(window, page)];
  if (raw == null) return null;
  return normalizePageLayout(raw, window, page);
}

/** 恢复某页出厂布局（经典方案） */
export function resetPageLayout(window: UiWindow, page: string): PageLayoutState {
  const root = readRoot();
  const key = pageKey(window, page);
  delete root.pages[key];
  delete root.custom[key];
  writeRoot(root);
  notifyLayoutChanged(window, page);
  return defaultPageLayout(window, page, "classic");
}

/** 通知布局变更：同窗口自定义事件 + Tauri 跨窗口广播（失败静默） */
export function notifyLayoutChanged(window: UiWindow, page: string): void {
  if (typeof window !== "undefined") {
    // 参数名 window 遮蔽了全局 window，这里通过 globalThis 取宿主窗口
    const host = globalThis.window;
    host?.dispatchEvent?.(
      new CustomEvent(UI_LAYOUT_LOCAL_EVENT, { detail: { windowKind: window, page } })
    );
  }
  // 动态引入：本模块被 Node 单测直接加载时不会因 Tauri API 环境失败
  void import("@tauri-apps/api/event")
    .then(({ emit }) => emit(UI_LAYOUT_EVENT, { windowKind: window, page }))
    .catch(() => {
      /* 非 Tauri 宿主 / 广播失败：本窗口已生效 */
    });
}
