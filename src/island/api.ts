/** 灵动岛 API 封装：Tauri 环境走 invoke；浏览器预览回退到 mock.ts */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { CANVAS_H, CANVAS_W, INSET } from "./liquid";
import { advanceSpring, hasDragMovement, nearestScreenEdge, screenGap, springAt, type Spring } from "./motion";
import type {
  ClashMode,
  ClashStatus,
  ClashToggleKind,
  ClipItem,
  HudBarUsage,
  IslandConfig,
  IslandDragBegin,
  IslandDragEnd,
  IslandDragSizes,
  IslandDragStep,
  IslandEdge,
  IslandQuotaDisplay,
  IslandState,
  LauncherProject,
  LiveCaption,
  LiveDevices,
  LiveStatus,
  LocatedPlace,
  LyricsState,
  MediaState,
  MeetingAudioDevice,
  MeetingItem,
  MeetingMark,
  MeetingPrivacy,
  MeetingSessionBrief,
  MeetingSessionInfo,
  MeetingSettings,
  MeetingStopResult,
  NotifyItem,
  NotifyModules,
  NotifyState,
  Snippet,
  StockMinute,
  StockSearchItem,
  StocksState,
  SystemState,
  TabKey,
  TodayUsageTotals,
  ToolboxImageInfo,
  ToolboxImageOut,
  ToolboxQrPng,
  ToolboxQrSaved,
  WeatherState,
} from "./types";
import { LIVE_CAPTION_LIMIT } from "./types";

export const IS_TAURI =
  typeof window !== "undefined" &&
  Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);

function delay<T>(value: T, ms = 120): Promise<T> {
  return new Promise((resolve) => window.setTimeout(() => resolve(value), ms));
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

// ===== mock 模块动态加载（内存压降专项）=====
// mock.ts 只有浏览器预览用（真机全部走 invoke）。原先静态 import 会把它
// 打进 island 主 chunk 且常驻内存；改为动态 import 后 Vite 拆独立 chunk，
// 真机运行永远不会触发下载。所有回退分支先 `await mocks()` 再取种子数据。
type MockModule = typeof import("./mock");
type MockConfig = MockModule["mockConfig"];
let mockModulePromise: Promise<MockModule> | null = null;
// 浏览器预览配置持久化：真机由 Rust SQLite 落盘；这里只服务预览验证。
const MOCK_CONFIG_KEY = "agenthud.mock.islandConfig";
// 与 mock.ts 的 MOCK_SETTINGS_STORE_KEY 同值：不能静态 import（否则 mock.ts
// 会重新进主 chunk），两处改值需同步。
const MOCK_SETTINGS_STORE_KEY = "agenthud.mock.islandSettings.v1";
// mock 模块里 mockConfig.transparency 的初始值（首次加载时读一次，供预览的
// settings 变更事件在缺省透明度时回落）。
let mockDefaultTransparency = 0;

/**
 * 首次真正需要 mock 种子数据时加载模块，并完成一次性水合（原先的模块级
 * `try { ... }` 静态块）。sessionStorage 的预览配置 / 展开态与 localStorage
 * 的设置窗写回都必须等 mock 模块就绪后再叠加，所以统一放进 boot，只跑一次。
 */
async function bootMock(): Promise<MockModule> {
  const m = await import("./mock");
  mockDefaultTransparency = m.mockConfig.transparency;
  try {
    const raw =
      typeof sessionStorage === "undefined" ? null : sessionStorage.getItem(MOCK_CONFIG_KEY);
    if (raw) Object.assign(m.mockConfig, JSON.parse(raw) as Partial<MockConfig>);
    if (!IS_TAURI) {
      const { quotaDisplay, hiddenTabs, notifyModules, transparency } =
        readMockIslandPreferences(localStorage.getItem(MOCK_SETTINGS_STORE_KEY));
      if (quotaDisplay) m.mockConfig.quotaDisplay = quotaDisplay;
      if (hiddenTabs) m.mockConfig.hiddenTabs = hiddenTabs;
      if (notifyModules) m.mockConfig.notifyModules = notifyModules;
      if (transparency !== undefined) m.mockConfig.transparency = transparency;
    }
  } catch {
    /* 隐私模式 / 存储不可用时退回内存 mock */
  }
  return m;
}

function mocks(): Promise<MockModule> {
  if (!mockModulePromise) mockModulePromise = bootMock();
  return mockModulePromise;
}

// 浏览器预览用的可变状态（真机上不参与：全部走 invoke）。种子数据依赖
// mock 模块，首次进入预览回退分支时才惰性初始化。
let mockClipState: ClipItem[] | null = null;
let mockSnippetState: Snippet[] | null = null;

// ===== 实时字幕：浏览器预览用的空闲快照 / 空字幕 / 示例设备 =====
/** 预览里的实时字幕状态（空闲；调 live_start 后转为 listening，便于验证开关态） */
let mockLiveStatus: LiveStatus = {
  running: false,
  phase: "idle",
  error: "",
  modelReady: false,
  source: "loopback",
  device: "",
  level: 0,
  download: { state: "idle", done: 0, total: 0, label: "", message: "" },
};
let mockLiveCaptions: LiveCaption[] = [];
/** 预览示例设备（与 Rust `live_devices()` 同结构：loopback / mic 两组名字） */
const MOCK_LIVE_DEVICES: LiveDevices = {
  loopback: ["扬声器 (Realtek High Definition Audio)", "耳机 (USB Audio Device)"],
  mic: ["麦克风阵列 (Realtek High Definition Audio)", "麦克风 (USB Audio Device)"],
};
const mockClashStatus: ClashStatus = {
  supported: true,
  running: true,
  installed: true,
  mode: "rule",
  mixedPort: 7897,
  tun: false,
  systemProxy: true,
  canSetSystemProxy: true,
  canSetTun: true,
  canSetMode: true,
  traffic: { uploadSpeed: 65_536, downloadSpeed: 1_572_864 },
  subscription: { used: 33_500_745_728, total: 107_374_182_400 },
  ipLocation: { ip: "203.0.113.18", region: "示例地区" },
};

/** 钉住的排前面（与 Rust 侧 sort_for_display 同语义的稳定分区） */
function sortPinnedFirst(items: ClipItem[]): ClipItem[] {
  return [
    ...items.filter((item) => item.pinned),
    ...items.filter((item) => !item.pinned),
  ];
}

const MOCK_DEFAULT_NOTIFY_MODULES = ["qq", "wechat", "mail"];

/** 浏览器预览：按 mockConfig.notifyModules 过滤通知（与 Rust 的可见列表同口径） */
async function mockNotifySnapshot(): Promise<NotifyState> {
  const m = await mocks();
  const modules = m.mockConfig.notifyModules ?? MOCK_DEFAULT_NOTIFY_MODULES;
  const items = m.mockNotify.items.filter((item) =>
    modules.includes(item.kind) && (item.kind !== "wechat" || !item.read)
  );
  return {
    items: clone(items),
    unread: items.filter((item) => !item.read).length,
    wechatUnread: items.filter((item) => item.kind === "wechat").length,
  };
}

// 浏览器预览：把 mock 配置持久化到 sessionStorage，刷新后仍能验证
// 「切页 → 改设置 → 重开仍停在该页」这类跨重启行为（真机由 Rust SQLite 落盘）。
// MOCK_CONFIG_KEY 定义见上方动态加载区。
function mockStringList(value: unknown): string[] | undefined {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : undefined;
}

function readMockIslandPreferences(raw: string | null): {
  quotaDisplay?: IslandQuotaDisplay;
  hiddenTabs?: string[];
  notifyModules?: string[];
  transparency?: number;
} {
  try {
    const settings = raw ? JSON.parse(raw) : null;
    const data = settings?.data?.island;
    const value = data?.quota_display;
    return {
      quotaDisplay: value === "rings" || value === "percent" ? value : undefined,
      hiddenTabs: mockStringList(data?.hidden_tabs),
      notifyModules: mockStringList(settings?.data?.notify?.modules),
      transparency: Number.isFinite(data?.transparency)
        ? Math.round(Math.max(0, Math.min(90, data.transparency)))
        : undefined,
    };
  } catch {
    return {};
  }
}

// 模块级预热已随动态加载挪进 bootMock（首次 await mocks() 时执行一次）。
async function persistMockConfig() {
  try {
    sessionStorage.setItem(MOCK_CONFIG_KEY, JSON.stringify((await mocks()).mockConfig));
  } catch {
    /* 忽略：预览不需要强一致 */
  }
}

// ===== 窗口 / 配置 =====

/** 浏览器预览：记录最后一次 island_set_expanded，刷新后模拟 WebView 重载恢复 */
let mockExpanded = false;
try {
  mockExpanded =
    typeof sessionStorage !== "undefined" &&
    sessionStorage.getItem("agenthud.mock.expanded") === "1";
} catch {
  mockExpanded = false;
}

export async function getIslandState(): Promise<IslandState> {
  if (!IS_TAURI) {
    const m = await mocks();
    return delay({
      enabled: true,
      config: clone(m.mockConfig),
      expanded: mockExpanded,
    });
  }
  return invoke<IslandState>("island_state");
}

export async function getIslandConfig(): Promise<IslandConfig> {
  if (!IS_TAURI) return delay(clone((await mocks()).mockConfig));
  return invoke<IslandConfig>("island_config");
}

export async function setIslandConfig(config: IslandConfig): Promise<IslandConfig> {
  if (!IS_TAURI) {
    const m = await mocks();
    const live = m.mockConfig;
    const hiddenTabsChanged = live.hiddenTabs.join() !== config.hiddenTabs.join();
    const modulesChanged = live.notifyModules?.join() !== config.notifyModules?.join();
    const transparencyChanged = live.transparency !== config.transparency;
    if (hiddenTabsChanged || modulesChanged || transparencyChanged) {
      try {
        const raw = localStorage.getItem(MOCK_SETTINGS_STORE_KEY);
        const data = (raw && JSON.parse(raw)?.data) || {};
        localStorage.setItem(MOCK_SETTINGS_STORE_KEY, JSON.stringify({
          data: {
            ...data,
            ...(hiddenTabsChanged || transparencyChanged ? {
              island: {
                ...data.island,
                ...(hiddenTabsChanged && { hidden_tabs: config.hiddenTabs }),
                ...(transparencyChanged && { transparency: config.transparency }),
              },
            } : {}),
            ...(modulesChanged && { notify: { ...data.notify, modules: config.notifyModules } }),
          },
        }));
      } catch {
        /* 存储不可用时仍更新本窗口。 */
      }
    }
    Object.assign(live, clone(config));
    await persistMockConfig();
    return delay(clone(live));
  }
  return invoke<IslandConfig>("island_set_config", { config });
}

export async function setIslandEnabled(enabled: boolean): Promise<boolean> {
  if (!IS_TAURI) return delay(enabled);
  return invoke<boolean>("island_set_enabled", { enabled });
}

export async function toggleIsland(): Promise<boolean> {
  if (!IS_TAURI) return delay(true);
  return invoke<boolean>("island_toggle");
}

export async function setIslandExpanded(expanded: boolean): Promise<void> {
  if (!IS_TAURI) {
    mockExpanded = expanded;
    try {
      sessionStorage.setItem("agenthud.mock.expanded", expanded ? "1" : "0");
    } catch {
      /* 预览会话存储不可用：仅内存 */
    }
    console.info("[island mock] set expanded", expanded);
    return delay(undefined, 20);
  }
  return invoke("island_set_expanded", { expanded });
}

/** 可穿透：窗口整体鼠标穿透（右键仍可用）。Rust 侧读 config.passthrough 落盘。 */
export async function islandSetPassthrough(on: boolean): Promise<void> {
  if (!IS_TAURI) {
    (await mocks()).mockConfig.passthrough = Boolean(on);
    await persistMockConfig();
    return delay(undefined, 20);
  }
  return invoke("island_set_passthrough", { on });
}

/** 记住上次停留的标签：独立命令落盘，避免每次切页都整份写回 IslandConfig。 */
/**
 * 可穿透右键菜单关闭后调用：立刻结束 Rust 侧的「临时可点击」窗口期（把
 * WS_EX_TRANSPARENT 加回去）。调用必须幂等；失败静默（Rust 侧还有 10s 兜底）。
 */
export async function islandPassthroughRestore(): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] passthrough restore");
    return delay(undefined, 20);
  }
  return invoke("island_passthrough_restore");
}

export async function islandSetLastTab(key: TabKey): Promise<void> {
  if (!IS_TAURI) {
    (await mocks()).mockConfig.lastTab = key;
    await persistMockConfig();
    return delay(undefined, 10);
  }
  return invoke("island_set_last_tab", { key });
}

/** macOS 悬停展开用：把收起态胶囊的实际矩形（CSS px）告诉 Rust。 */
export async function islandSetHoverRect(
  x: number,
  y: number,
  width: number,
  height: number
): Promise<void> {
  if (!IS_TAURI) {
    console.info(
      "[island mock] hover rect",
      Math.round(x),
      Math.round(y),
      Math.round(width),
      Math.round(height)
    );
    return;
  }
  try {
    await invoke("island_set_hover_rect", { x, y, width, height });
  } catch {
    /* 上报失败只影响 mac 悬停手感，不影响其它功能 */
  }
}

/**
 * 收起态右键菜单开/关：菜单是浮在胶囊热区之外的绝对定位浮层，打开期间要让
 * Rust 冻结「透明留白自动穿透」，否则光标一移到菜单上就会被判在热区外、窗口
 * 被设成 WS_EX_TRANSPARENT，菜单随即点不动。关闭后下一拍恢复原判定。
 */
export async function islandSetContextMenuOpen(open: boolean): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] context menu", open);
    return;
  }
  try {
    await invoke("island_set_context_menu_open", { open });
  } catch {
    /* 失败只影响菜单期间的可点击性，不打断菜单交互 */
  }
}

// ===== 拖拽 / 吸附 =====

// 浏览器没有全局桌面坐标，以画布内缩线模拟工作区，使用同一组弹簧参数。
const mockEdgeLines = { top: INSET, bottom: CANVAS_H - INSET, left: INSET, right: CANVAS_W - INSET };
const mockHorizontal = (edge: IslandEdge) => edge === "top" || edge === "bottom";
let mockDrag: {
  edge: IslandEdge;
  sizes: IslandDragSizes;
  expanded: boolean;
  navigation: boolean;
  press: { x: number; y: number };
  grab: { x: number; y: number };
  x: Spring;
  y: Spring;
  w: Spring;
  h: Spring;
  last: number;
  started: boolean;
  snap?: { edge: IslandEdge; along: Spring; gap: Spring };
} | null = null;
let mockBodySize = { width: 300, height: 54, expanded: false };
let mockCursor = { x: 0, y: 0 };
let mockPointerDown = false;

if (!IS_TAURI && typeof window !== "undefined") {
  const updateCursor = (event: PointerEvent) => {
    mockCursor = { x: event.clientX, y: event.clientY };
  };
  window.addEventListener("pointermove", updateCursor, true);
  window.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    updateCursor(event);
    mockPointerDown = true;
  }, true);
  const release = () => { mockPointerDown = false; };
  window.addEventListener("pointerup", release, true);
  window.addEventListener("pointercancel", release, true);
  window.addEventListener("blur", release);
}

function mockCenter(edge: IslandEdge, along: number, gap: number, width: number, height: number) {
  switch (edge) {
    case "top": return { x: along, y: mockEdgeLines.top + gap + height / 2 };
    case "bottom": return { x: along, y: mockEdgeLines.bottom - gap - height / 2 };
    case "left": return { x: mockEdgeLines.left + gap + width / 2, y: along };
    case "right": return { x: mockEdgeLines.right - gap - width / 2, y: along };
  }
}

/** P4：把当前本体（640×H / 收起胶囊）的实际尺寸上报给 Rust。
 * Rust 用它在 780×632 大画布内计算 bodyRect / 液桥几何；失败静默。 */
export async function islandSetBodySize(
  width: number,
  height: number,
  expanded: boolean
): Promise<void> {
  if (!IS_TAURI) {
    mockBodySize = { width: Math.round(width), height: Math.round(height), expanded };
    console.info(
      "[island mock] body size",
      Math.round(width),
      Math.round(height),
      expanded ? "expanded" : "collapsed"
    );
    return delay(undefined, 10);
  }
  try {
    await invoke("island_set_body_size", {
      width: Math.round(width),
      height: Math.round(height),
      expanded,
    });
  } catch {
    /* Rust 任务尚未落地时静默：预览 / 旧后端仍能显示外壳 */
  }
}

export async function beginIslandDrag(
  sizes: IslandDragSizes,
  navigation: boolean,
  expanded: boolean,
  edge: IslandEdge,
  pointer: { x: number; y: number },
): Promise<IslandDragBegin> {
  if (!IS_TAURI) {
    const { width, height } = mockBodySize;
    const center = mockCenter(edge, mockHorizontal(edge) ? CANVAS_W / 2 : CANVAS_H / 2, 0, width, height);
    mockDrag = {
      edge, sizes, expanded, navigation,
      press: mockCursor,
      grab: { x: center.x - mockCursor.x, y: center.y - mockCursor.y },
      x: springAt(center.x),
      y: springAt(center.y),
      w: springAt(width),
      h: springAt(height),
      last: performance.now(),
      started: false,
    };
    return { active: true };
  }
  return invoke<IslandDragBegin>("island_drag_begin", { sizes, navigation, expanded, pointer });
}

export async function stepIslandDrag(): Promise<IslandDragStep> {
  if (IS_TAURI) return invoke<IslandDragStep>("island_drag_step");

  const motion = mockDrag;
  if (!motion) return { active: false, dx: 0, dy: 0, started: false, settled: true };
  if (!motion.started) {
    const dx = mockCursor.x - motion.press.x;
    const dy = mockCursor.y - motion.press.y;
    const moved = hasDragMovement(dx, dy, motion.navigation);
    if (!mockPointerDown || !moved) {
      if (!mockPointerDown) mockDrag = null;
      return { active: mockPointerDown, dx: 0, dy: 0, started: false };
    }
    motion.started = true;
    if (motion.expanded) {
      motion.grab = { x: 0, y: 0 };
      motion.x = springAt(mockCursor.x);
      motion.y = springAt(mockCursor.y);
    } else {
      motion.grab = { x: motion.x.value - mockCursor.x, y: motion.y.value - mockCursor.y };
    }
  }
  const now = performance.now();
  const dt = Math.max(1 / 240, Math.min(1 / 30, (now - motion.last) / 1000));
  motion.last = now;
  const oldX = motion.x.value;
  const oldY = motion.y.value;
  let squash = 0;
  let active = mockPointerDown;
  let settled = false;

  if (motion.snap) {
    const { edge, along, gap } = motion.snap;
    const horizontal = mockHorizontal(edge);
    const size = horizontal ? motion.sizes.horizontal : motion.sizes.vertical;
    advanceSpring(motion.w, size.width, 460, 0.64, dt, 0.2);
    advanceSpring(motion.h, size.height, 460, 0.64, dt, 0.2);
    const targetAlong = horizontal ? CANVAS_W / 2 : CANVAS_H / 2;
    advanceSpring(along, targetAlong, 240, 0.74, dt, 0.2);
    advanceSpring(gap, 0, 300, 0.38, dt, 0.2);
    squash = Math.max(0, -gap.value);
    const center = mockCenter(edge, along.value, Math.max(0, gap.value), motion.w.value, motion.h.value);
    const normalVelocity = edge === "top" || edge === "left" ? gap.velocity : -gap.velocity;
    motion.x = springAt(center.x, horizontal ? along.velocity : normalVelocity);
    motion.y = springAt(center.y, horizontal ? normalVelocity : along.velocity);
    settled = along.value === targetAlong && gap.value === 0 && motion.w.value === size.width && motion.h.value === size.height;
    active = !settled;
    if (settled) mockDrag = null;
  } else if (mockPointerDown) {
    const size = mockHorizontal(motion.edge) ? motion.sizes.horizontal : motion.sizes.vertical;
    advanceSpring(motion.w, size.width, 300, 0.55, dt, 0.2);
    advanceSpring(motion.h, size.height, 300, 0.55, dt, 0.2);
    advanceSpring(motion.x, mockCursor.x + motion.grab.x, 900, 0.78, dt, 0.2);
    advanceSpring(motion.y, mockCursor.y + motion.grab.y, 900, 0.78, dt, 0.2);
  }

  const width = motion.w.value;
  const height = motion.h.value;
  const bodyRect = { x: motion.x.value - width / 2, y: motion.y.value - height / 2, width, height };
  const edge = motion.snap?.edge ?? nearestScreenEdge(bodyRect, mockEdgeLines);
  return {
    active, started: true, settled, edge, bodyRect, squash,
    dx: motion.x.value - oldX,
    dy: motion.y.value - oldY,
    vx: motion.x.velocity,
    vy: motion.y.velocity,
    edgeLines: mockEdgeLines,
    origin: { x: 0, y: 0 },
  };
}

export async function endIslandDrag(): Promise<IslandDragEnd> {
  if (IS_TAURI) return invoke<IslandDragEnd>("island_drag_end", { started: true });

  const motion = mockDrag;
  if (!motion?.started) throw new Error("拖动会话已结束");
  const size = mockHorizontal(motion.edge) ? motion.sizes.horizontal : motion.sizes.vertical;
  const body = {
    x: motion.x.value - size.width / 2,
    y: motion.y.value - size.height / 2,
    ...size,
  };
  const edge = nearestScreenEdge(body, mockEdgeLines, 1.35);
  const horizontal = mockHorizontal(edge);
  const normal = horizontal ? motion.y.velocity : motion.x.velocity;
  motion.snap = {
    edge,
    along: springAt(horizontal ? motion.x.value : motion.y.value, horizontal ? motion.x.velocity : motion.y.velocity),
    gap: springAt(Math.max(-4, screenGap(edge, body, mockEdgeLines[edge])),
      edge === "top" || edge === "left" ? normal : -normal),
  };
  (await mocks()).mockConfig.edge = edge;
  await persistMockConfig();
  return { edge, x: 0, y: 0, width: CANVAS_W, height: CANVAS_H };
}

/** 只接收前端尚未确认的桌面纹理。 */
export async function getIslandBackdrop(knownRevision?: number): Promise<{ revision: number; url: string } | null> {
  if (IS_TAURI) return invoke("island_backdrop", { knownRevision });
  if (knownRevision === 1 || new URLSearchParams(window.location.search).get("backdrop") !== "1") return null;

  // 预览中的条纹便于观察边缘对采样坐标的位移。
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="780" height="632">' +
    '<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">' +
    '<stop offset="0" stop-color="#223a72"/><stop offset="0.55" stop-color="#4a2f6e"/>' +
    '<stop offset="1" stop-color="#7a3357"/></linearGradient>' +
    '<pattern id="lines" width="42" height="42" patternUnits="userSpaceOnUse">' +
    '<path d="M0 0H42M0 0V42" stroke="#a7d8ff" stroke-opacity=".65" stroke-width="3"/>' +
    '</pattern></defs><rect width="780" height="632" fill="url(#g)"/>' +
    '<rect width="780" height="632" fill="url(#lines)"/></svg>';
  return delay({ revision: 1, url: "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg) }, 35);
}

/** 输入框聚焦时临时激活窗口；false = 恢复不可聚焦，不抢编辑器焦点 */
export async function setKeyboard(active: boolean): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 10);
  return invoke("island_set_keyboard", { active });
}

export async function openPath(path: string): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] open", path);
    return delay(undefined, 20);
  }
  return invoke("island_open_path", { path });
}

export async function quitApp(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_quit");
}

export async function openMainWindow(page?: string): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] open main window", page);
    return delay(undefined, 20);
  }
  return invoke("open_main_window", { page: page ?? null });
}

/** 打开独立的「灵动岛设置」窗口（label island-settings，页面
 *  island-settings.html，见 src-tauri/src/island_settings_window.rs）。
 *
 * 窗口是懒创建的：首次调用建窗并聚焦，之后（含关闭=隐藏后再开）只做
 * show + unminimize + set_focus，秒开。命令无参数、无返回值，失败串由
 * Rust 侧给出中文原因；调用方只需 catch 后 toast，不阻断岛内其它交互。
 * 浏览器预览没有真实窗口，按本文件 mock 分支的既有风格打日志即可。 */
export async function openIslandSettingsWindow(): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] open island settings window");
    return delay(undefined, 20);
  }
  return invoke("open_island_settings_window");
}

/** 全局开关热键当前是否已成功注册（只读查询，不改注册）。
 *
 * 语义来自 Rust 侧 island_hotkey_registered：读配置里的 hotkeyToggle 再问
 * global-shortcut 插件「是否注册着这个组合」。被其它软件占用、组合非法或
 * 空串时返回 false。设置窗口据此显示「已生效 / 未生效」提示；保存热键后
 * 注册发生在主线程（island_set_config 的命令返回之后），调用方需要延迟
 * 一拍再查询才能读到新状态。
 * 浏览器预览没有全局热键，统一返回 true（与 mockConfig 的默认组合自洽）。 */
export async function islandHotkeyRegistered(): Promise<boolean> {
  if (!IS_TAURI) return delay(true);
  return invoke<boolean>("island_hotkey_registered");
}

// ===== 额度 =====

export async function getQuotaSnapshot(): Promise<HudBarUsage | null> {
  if (!IS_TAURI) return delay(clone((await mocks()).mockUsage));
  return invoke<HudBarUsage | null>("island_quota_snapshot");
}

export async function refreshQuota(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_refresh_quota");
}

export async function getTodayUsage(): Promise<TodayUsageTotals | null> {
  if (!IS_TAURI) {
    return delay({
      tokens: 1_284_300,
      cost: 3.86,
      currency: "CNY",
      updatedAt: Date.now(),
    });
  }
  return invoke<TodayUsageTotals | null>("island_today_usage");
}

// ===== 天气 =====

export async function getWeather(): Promise<WeatherState | null> {
  if (!IS_TAURI) return delay(clone((await mocks()).mockWeather));
  return invoke<WeatherState | null>("island_weather");
}

export async function refreshWeather(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_weather_refresh");
}

/** 绑定主城市：把 name/lat/lon 写进 weather.city/lat/lon（settings.json 同步） */
export async function bindWeatherCity(city: {
  name: string;
  lat: number | null;
  lon: number | null;
}): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_weather_bind", {
    name: city.name,
    lat: city.lat,
    lon: city.lon,
  });
}

/** 取消绑定：清空主城市，被解绑城市补回自定义列表并立即重新定位 IP */
export async function unbindWeatherCity(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_weather_unbind");
}

/** 添加自定义城市：城市名 geocode 或「纬度,经度」；成功返回可展示提示文案 */
export async function addWeatherCity(query: string): Promise<string> {
  if (!IS_TAURI) return delay(`已添加城市「${query.trim()}」`, 20);
  return invoke<string>("island_weather_add_city", { query });
}

/** 删除自定义城市：只从 weather.cities 删除，主城市 / IP 城市不可删 */
export async function removeWeatherCity(city: {
  name: string;
  lat: number | null;
  lon: number | null;
}): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_weather_remove_city", {
    name: city.name,
    lat: city.lat,
    lon: city.lon,
  });
}

export async function locateCurrentPlace(): Promise<LocatedPlace> {
  if (!IS_TAURI) {
    return delay({
      ok: true,
      error: "",
      country: "中国",
      province: "陕西",
      city: "西安",
      district: "",
      lat: 34.2635,
      lon: 108.9246,
      label: "陕西 · 西安",
    });
  }
  return invoke<LocatedPlace>("island_locate");
}

// ===== 股票 =====

export async function getStocks(): Promise<StocksState | null> {
  if (!IS_TAURI) return delay(clone((await mocks()).mockStocks));
  return invoke<StocksState | null>("island_stocks");
}

export async function refreshStocks(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_stocks_refresh");
}

export async function searchStock(query: string): Promise<StockSearchItem[]> {
  if (!IS_TAURI) {
    const q = query.trim().toLowerCase();
    const pool = [
      { code: "sh600519", name: "贵州茅台" },
      { code: "sz000858", name: "五粮液" },
      { code: "sh601318", name: "中国平安" },
      { code: "hk09988", name: "阿里巴巴-W" },
      { code: "usTSLA", name: "特斯拉" },
      { code: "usMSFT", name: "微软" },
      { code: "sz300750", name: "宁德时代" },
      { code: "sh588000", name: "科创50ETF" },
    ];
    return delay(
      pool.filter(
        (i) => !q || i.code.toLowerCase().includes(q) || i.name.toLowerCase().includes(q)
      )
    );
  }
  return invoke<StockSearchItem[]>("island_stock_search", { query });
}

export async function getStockMinute(code: string): Promise<StockMinute> {
  if (!IS_TAURI) return delay({ ...clone((await mocks()).mockMinute), code });
  return invoke<StockMinute>("island_stock_minute", { code });
}

// ===== 媒体 =====

export async function getMedia(): Promise<MediaState> {
  if (!IS_TAURI) return delay(clone((await mocks()).mockMedia), 40);
  return invoke<MediaState>("island_media");
}

export async function mediaAction(action: "toggle" | "next" | "prev", appId: string): Promise<void> {
  if (!IS_TAURI) {
    const m = await mocks();
    if (action === "toggle") m.mockMedia.playing = !m.mockMedia.playing;
    return delay(undefined, 40);
  }
  return invoke("island_media_action", { action, appId });
}

/**
 * 歌词：按后端配置的源顺序（`media.lyrics_sources`）逐个尝试。
 * `album` / `duration` 供 lrclib 精确匹配与自定义接口占位符使用；缺失传 null
 * （后端按 `Option` 处理）。
 */
export async function getLyrics(
  title: string,
  artist: string,
  album?: string,
  duration?: number
): Promise<LyricsState> {
  if (!IS_TAURI) {
    return delay({
      ok: true,
      error: "",
      title,
      artist,
      lrc: "[00:00.00]示例歌词\n[00:12.00]把额度与天气轻轻收进屏幕边缘\n[00:24.00]玻璃小岛贴着上缘呼吸\n[00:36.00]悬停时展开一整片天空",
      translation: "",
      source: "mock",
    });
  }
  return invoke<LyricsState>("island_lyrics", {
    title,
    artist,
    album: album ?? null,
    duration: duration ?? null,
  });
}

// ===== 实时字幕 =====
// 后端：系统声音 / 麦克风 → VAD 断句 → 日语 ASR（定稿 + 草稿）→ 流式翻译。
// 状态快照走 `zbar://island-live`，单条字幕走 `zbar://island-live-caption`。
// 浏览器预览全部返回空闲快照 / 空字幕 / 示例设备，避免预览报错。

/** 模型总大小的兜底估算（约 630MB；后端拿不到 total 时 UI 用它算百分比） */
export const LIVE_MODEL_TOTAL_FALLBACK = 630 * 1024 * 1024;

export async function liveStatus(): Promise<LiveStatus> {
  if (!IS_TAURI) return delay(clone(mockLiveStatus), 40);
  return invoke<LiveStatus>("live_status");
}

/** 开始聆听：模型未下载 / 设备不可用时后端返回 phase=error 的快照（不抛异常）。 */
export async function liveStart(): Promise<LiveStatus> {
  if (!IS_TAURI) {
    // 预览不做真实采集：直接切到 listening，便于验证胶囊字幕层与开始/停止态。
    mockLiveStatus = { ...mockLiveStatus, running: true, phase: "listening", error: "" };
    return delay(clone(mockLiveStatus), 60);
  }
  return invoke<LiveStatus>("live_start");
}

export async function liveStop(): Promise<LiveStatus> {
  if (!IS_TAURI) {
    mockLiveStatus = { ...mockLiveStatus, running: false, phase: "idle", level: 0 };
    return delay(clone(mockLiveStatus), 60);
  }
  return invoke<LiveStatus>("live_stop");
}

/** 可选音频设备：按 source 分 loopback（系统声音）/ mic（麦克风）两组。 */
export async function liveDevices(): Promise<LiveDevices> {
  if (!IS_TAURI) return delay(clone(MOCK_LIVE_DEVICES), 40);
  return invoke<LiveDevices>("live_devices");
}

/** 下载 parakeet-ja + silero_vad 模型；进度通过 `zbar://island-live` 推送。 */
export async function liveDownloadModels(): Promise<LiveStatus> {
  if (!IS_TAURI) {
    // 预览直接当作下载完成（约 630MB），便于验证「已下载」态。
    mockLiveStatus = {
      ...mockLiveStatus,
      modelReady: true,
      download: {
        state: "done",
        done: LIVE_MODEL_TOTAL_FALLBACK,
        total: LIVE_MODEL_TOTAL_FALLBACK,
        label: "模型已就绪",
        message: "",
      },
    };
    return delay(clone(mockLiveStatus), 80);
  }
  return invoke<LiveStatus>("live_download_models");
}

export async function liveCancelDownload(): Promise<LiveStatus> {
  if (!IS_TAURI) {
    mockLiveStatus = {
      ...mockLiveStatus,
      download: { state: "cancelled", done: 0, total: 0, label: "", message: "" },
    };
    return delay(clone(mockLiveStatus), 60);
  }
  return invoke<LiveStatus>("live_cancel_download");
}

/** 当前会话的字幕历史（旧 → 新；最多 80 条）。 */
export async function liveCaptions(): Promise<LiveCaption[]> {
  if (!IS_TAURI) return delay(clone(mockLiveCaptions), 40);
  return invoke<LiveCaption[]>("live_captions");
}

/** 清空字幕历史（后端没有返回值约定，只用本地清空 + 事件收敛）。 */
export async function liveClearCaptions(): Promise<void> {
  if (!IS_TAURI) {
    mockLiveCaptions = [];
    return delay(undefined, 40);
  }
  return invoke("live_clear_captions");
}

// ===== 系统 =====

export async function getSystem(): Promise<SystemState> {
  if (!IS_TAURI) return delay(clone((await mocks()).mockSystem));
  return invoke<SystemState>("island_system");
}

export async function setVolume(volume: number): Promise<SystemState> {
  if (!IS_TAURI) {
    const m = await mocks();
    m.mockSystem.volume = Math.min(1, Math.max(0, volume));
    return delay(clone(m.mockSystem), 20);
  }
  return invoke<SystemState>("island_set_volume", { volume });
}

export async function toggleMute(): Promise<SystemState> {
  if (!IS_TAURI) {
    const m = await mocks();
    m.mockSystem.muted = !m.mockSystem.muted;
    return delay(clone(m.mockSystem), 20);
  }
  return invoke<SystemState>("island_toggle_mute");
}

export async function setBrightness(value: number): Promise<SystemState> {
  if (!IS_TAURI) {
    const m = await mocks();
    m.mockSystem.brightness = value;
    return delay(clone(m.mockSystem), 20);
  }
  return invoke<SystemState>("island_set_brightness", { value });
}

export async function toggleRadio(
  kind: "wifi" | "bluetooth",
  on: boolean
): Promise<SystemState> {
  if (!IS_TAURI) {
    const m = await mocks();
    if (kind === "wifi") m.mockSystem.wifi = on;
    else m.mockSystem.bluetooth = on;
    return delay(clone(m.mockSystem), 20);
  }
  return invoke<SystemState>("island_toggle_radio", { kind, on });
}

export async function openSystemSettings(kind: string): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_open_system_settings", { kind });
}

export async function getClashStatus(): Promise<ClashStatus> {
  if (!IS_TAURI) return delay(clone(mockClashStatus));
  return invoke<ClashStatus>("island_clash_status");
}

export async function setClashToggle(kind: ClashToggleKind, enabled: boolean): Promise<ClashStatus> {
  if (!IS_TAURI) {
    mockClashStatus[kind] = enabled;
    return delay(clone(mockClashStatus));
  }
  return invoke<ClashStatus>("island_clash_set", { kind, enabled });
}

export async function setClashMode(mode: ClashMode): Promise<ClashStatus> {
  if (!IS_TAURI) {
    mockClashStatus.mode = mode;
    return delay(clone(mockClashStatus));
  }
  return invoke<ClashStatus>("island_clash_set_mode", { mode });
}

export async function openClashVerge(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_clash_open");
}

// ===== 剪贴板 =====

export async function getClipboard(): Promise<ClipItem[]> {
  if (!IS_TAURI) {
    mockClipState ??= clone((await mocks()).mockClipboardWithImage());
    return delay(clone(mockClipState));
  }
  return invoke<ClipItem[]>("island_clipboard");
}

export async function copyClipboard(id: number): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_clipboard_copy", { id });
}

export async function clearClipboard(): Promise<void> {
  if (!IS_TAURI) {
    mockClipState ??= clone((await mocks()).mockClipboardWithImage());
    // 与 Rust 侧一致：清空保留钉住的条目
    mockClipState = mockClipState.filter((item) => item.pinned);
    return delay(undefined, 20);
  }
  return invoke("island_clipboard_clear");
}

// ===== 消息 =====

export async function getNotify(): Promise<NotifyState> {
  if (!IS_TAURI) return delay(await mockNotifySnapshot());
  return invoke<NotifyState>("island_notify");
}

export async function markNotifyRead(): Promise<NotifyState> {
  if (!IS_TAURI) {
    const m = await mocks();
    m.mockNotify.items.forEach((i) => {
      i.read = true;
    });
    m.mockNotify.unread = 0;
    return delay(await mockNotifySnapshot(), 20);
  }
  return invoke<NotifyState>("island_notify_mark_read");
}

export async function clearNotify(kind?: string): Promise<NotifyState> {
  if (!IS_TAURI) {
    const m = await mocks();
    m.mockNotify.items = kind
      ? m.mockNotify.items.filter((i) => i.kind !== kind)
      : [];
    m.mockNotify.unread = m.mockNotify.items.filter((i) => !i.read).length;
    return delay(await mockNotifySnapshot(), 20);
  }
  return invoke<NotifyState>("island_notify_clear", { kind: kind ?? null });
}

/** 只把某个模块（kind）的消息标记为已读；返回最新可见状态。 */
export async function markNotifyReadKind(kind: string): Promise<NotifyState> {
  if (!IS_TAURI) {
    const m = await mocks();
    m.mockNotify.items.forEach((item) => {
      if (item.kind === kind) item.read = true;
    });
    m.mockNotify.unread = m.mockNotify.items.filter((item) => !item.read).length;
    return delay(await mockNotifySnapshot(), 20);
  }
  return invoke<NotifyState>("island_notify_mark_read_kind", { kind });
}

/** 消息模块开关快照（消息页磁贴据 config.notifyModules 渲染；这里单次读）。 */
export async function getNotifyModules(): Promise<NotifyModules> {
  if (!IS_TAURI) {
    const m = await mocks();
    return delay({
      modules: [...(m.mockConfig.notifyModules ?? MOCK_DEFAULT_NOTIFY_MODULES)],
      bodyPreview: [...(m.mockConfig.notifyBodyPreview ?? ["feishu", "dingtalk"])],
    });
  }
  return invoke<NotifyModules>("island_notify_modules");
}

/** 系统音频峰值（0..1，Rust 微秒级读取，无设备/无会话返回 0）；预览合成。 */
export async function islandAudioPeak(): Promise<number> {
  if (!IS_TAURI) {
    const m = await mocks();
    if (!m.mockMedia.hasSession || !m.mockMedia.playing) return delay(0, 5);
    const t = performance.now() / 1000;
    const value =
      0.44 +
      0.28 * Math.sin(t * 7.7) * Math.sin(t * 3.1 + 1.2) +
      0.12 * Math.sin(t * 23.0 + 0.4);
    return delay(Math.max(0, Math.min(1, value)), 5);
  }
  return invoke<number>("island_audio_peak");
}

export async function openNotifyApp(kind: string, exe?: string): Promise<NotifyState> {
  if (!IS_TAURI) return markNotifyReadKind(kind);
  return invoke<NotifyState>("island_notify_open", { kind, exe: exe || null });
}

export async function openNotifySettings(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_notify_open_settings");
}

// ===== 事件 =====

export function onEvent<T>(name: string, handler: (payload: T) => void): () => void {
  if (!IS_TAURI) {
    // 浏览器预览：用 window CustomEvent 模拟 Tauri 事件总线。这样没有 Rust 也能
    // 调收起态 / activity，并可在 Chrome headless 里用 CDP 注入事件做交互验证。
    const listener = (event: Event) => {
      const detail = (event as CustomEvent<T>).detail as unknown;
      // 通知事件同步回写 mock 状态：页面注入/真实 Tauri 事件后，再调
      // island_notify_* mock 命令时返回值仍是同一份数据（预览更接近真机）。
      if (name === "zbar://island-notify" && detail && typeof detail === "object") {
        const payload = detail as { items?: unknown; unread?: number };
        if (Array.isArray(payload.items)) {
          // mock 模块可能还没加载完：事件先到也无所谓，拿到模块后按同一份
          // payload 回写，保持「注入事件 → 再调 mock 命令」数据一致。
          void mocks().then((m) => {
            m.mockNotify.items = clone(payload.items as NotifyItem[]);
            m.mockNotify.unread =
              typeof payload.unread === "number"
                ? payload.unread
                : m.mockNotify.items.filter((item) => !item.read).length;
          });
        }
      }
      // 实时字幕：注入的状态 / 字幕事件同步回写 mock，之后调 live_status() /
      // live_captions() 返回的还是同一份数据（预览更接近真机）。
      if (name === "zbar://island-live" && detail && typeof detail === "object") {
        mockLiveStatus = { ...mockLiveStatus, ...clone(detail as Partial<LiveStatus>) };
      }
      if (name === "zbar://island-live-caption" && detail && typeof detail === "object") {
        const caption = clone(detail as LiveCaption);
        const index = mockLiveCaptions.findIndex((item) => item.id === caption.id);
        if (index >= 0) mockLiveCaptions[index] = caption;
        else mockLiveCaptions = [...mockLiveCaptions, caption].slice(-LIVE_CAPTION_LIMIT);
      }
      handler(detail as T);
    };
    window.addEventListener(name, listener);
    const onSettingsSaved = (event: StorageEvent) => {
      if (event.key !== MOCK_SETTINGS_STORE_KEY && event.key !== null) return;
      // storage 事件是同步回调，而 mock 配置要等动态模块：整段包进 IIFE，
      // 比较 / 回写 / 回调都落在同一份配置上（预览不要求同步送达）。
      void (async () => {
        const next = readMockIslandPreferences(event.newValue);
        const quotaDisplay = next.quotaDisplay ?? "percent";
        const hiddenTabs = next.hiddenTabs ?? [];
        const notifyModules = next.notifyModules ?? MOCK_DEFAULT_NOTIFY_MODULES;
        const m = await mocks();
        const live = m.mockConfig;
        const transparency = next.transparency ?? mockDefaultTransparency;
        if (
          live.quotaDisplay === quotaDisplay &&
          live.hiddenTabs.join() === hiddenTabs.join() &&
          live.notifyModules?.join() === notifyModules.join() &&
          live.transparency === transparency
        ) return;
        live.quotaDisplay = quotaDisplay;
        live.hiddenTabs = hiddenTabs;
        live.notifyModules = notifyModules;
        live.transparency = transparency;
        await persistMockConfig();
        handler(clone(live) as T);
      })();
    };
    if (name === "zbar://island-config") window.addEventListener("storage", onSettingsSaved);
    return () => {
      window.removeEventListener(name, listener);
      if (name === "zbar://island-config") window.removeEventListener("storage", onSettingsSaved);
    };
  }
  let unlisten: UnlistenFn | null = null;
  let dead = false;
  listen<T>(name, (event) => handler(event.payload))
    .then((fn) => {
      if (dead) fn();
      else unlisten = fn;
    })
    .catch(() => {
      /* 窗口卸载 / ACL 拒绝：无副作用 */
    });
  return () => {
    dead = true;
    if (unlisten) unlisten();
  };
}

/** 实时字幕状态快照（运行 / 模型 / 下载进度 / 电平），对应 `zbar://island-live`。 */
export function onLiveStatus(handler: (status: LiveStatus) => void): () => void {
  return onEvent<LiveStatus>("zbar://island-live", handler);
}

/**
 * 单条字幕更新，对应 `zbar://island-live-caption`。
 * 同一个 id 的草稿会反复推送（draft=true），定稿推送后不再变化。
 */
export function onLiveCaption(handler: (caption: LiveCaption) => void): () => void {
  return onEvent<LiveCaption>("zbar://island-live-caption", handler);
}

// ===== 工具箱（二维码 / 图片压缩转换）=====

/** 生成二维码 PNG（data URL，不落盘）。空内容 / 超容量由后端给中文错误。 */
export async function toolboxQrPng(
  text: string,
  ecLevel: string,
  scale: number,
  quiet: number
): Promise<ToolboxQrPng> {
  if (!IS_TAURI) return delay((await mocks()).mockToolboxQr(text, scale, quiet));
  return invoke<ToolboxQrPng>("toolbox_qr_png", { text, ecLevel, scale, quiet });
}

/** 生成二维码并写盘（路径由文件对话框给出） */
export async function toolboxQrSave(
  text: string,
  ecLevel: string,
  scale: number,
  quiet: number,
  outPath: string
): Promise<ToolboxQrSaved> {
  if (!IS_TAURI) {
    const png = (await mocks()).mockToolboxQr(text, scale, quiet);
    return delay({ path: outPath, bytes: png.bytes, size: png.size });
  }
  return invoke<ToolboxQrSaved>("toolbox_qr_save", {
    text,
    ecLevel,
    scale,
    quiet,
    outPath,
  });
}

/** 读取图片元信息 + 720px 预览 */
export async function toolboxImageInspect(
  path: string
): Promise<ToolboxImageInfo> {
  if (!IS_TAURI) {
    return delay({
      path,
      width: 2560,
      height: 1440,
      bytes: 3_912_448,
      format: "png",
      preview: (await mocks()).mockToolboxImagePreview(),
    });
  }
  return invoke<ToolboxImageInfo>("toolbox_image_inspect", { path });
}

/** 按目标格式 / 质量 / 最长边重编码并写盘 */
export async function toolboxImageSave(
  inputPath: string,
  format: string,
  quality: number | null,
  maxEdge: number,
  outPath: string
): Promise<ToolboxImageOut> {
  if (!IS_TAURI) {
    const ratio = maxEdge > 0 ? maxEdge / 2560 : 1;
    return delay({
      path: outPath,
      width: Math.round(2560 * ratio),
      height: Math.round(1440 * ratio),
      bytes:
        format === "jpeg" ? 412_000 : format === "webp" ? 602_000 : 1_820_000,
      format,
    });
  }
  return invoke<ToolboxImageOut>("toolbox_image_save", {
    inputPath,
    format,
    quality,
    maxEdge,
    outPath,
  });
}

// ===== 剪贴板增强 =====

/** 钉住 / 取消钉住（返回调整后的完整列表） */
export async function clipboardPin(
  id: number,
  pinned: boolean
): Promise<ClipItem[]> {
  if (!IS_TAURI) {
    mockClipState ??= clone((await mocks()).mockClipboardWithImage());
    const next = mockClipState.map((item) =>
      item.id === id ? { ...item, pinned } : item
    );
    mockClipState = sortPinnedFirst(next);
    return delay(mockClipState);
  }
  return invoke<ClipItem[]>("island_clipboard_pin", { id, pinned });
}

/** 删除单条（钉住的也能删） */
export async function clipboardDelete(id: number): Promise<ClipItem[]> {
  if (!IS_TAURI) {
    mockClipState ??= clone((await mocks()).mockClipboardWithImage());
    mockClipState = mockClipState.filter((item) => item.id !== id);
    return delay(mockClipState);
  }
  return invoke<ClipItem[]>("island_clipboard_delete", { id });
}

/** 调整条数上限（后端夹到 5–100 并立即淘汰） */
export async function clipboardSetLimit(limit: number): Promise<ClipItem[]> {
  if (!IS_TAURI) {
    mockClipState ??= clone((await mocks()).mockClipboardWithImage());
    mockClipState = mockClipState.slice(0, Math.max(5, Math.min(100, limit)));
    return delay(mockClipState);
  }
  return invoke<ClipItem[]>("island_clipboard_set_limit", { limit });
}

/** 当前条数上限（页面首次渲染读一次） */
export async function clipboardLimit(): Promise<number> {
  if (!IS_TAURI) return delay(10, 0);
  return invoke<number>("island_clipboard_limit");
}

/** 写文本剪贴板（OCR 结果回写等场景） */
export async function copyText(text: string): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] copy text", text.slice(0, 40));
    return delay(undefined, 20);
  }
  return invoke("shot_copy_text", { text });
}

/** 图片 → 文字（复用截图 OCR 命令，入参是 data URL） */
export async function ocrImage(dataUrl: string): Promise<{
  ok: boolean;
  text: string;
  lines: string[];
  error: string;
}> {
  if (!IS_TAURI) {
    const text = "AgentHUD · 剪贴板 OCR 预览文本\n第二行示例内容";
    return delay({ ok: true, text, lines: text.split("\n"), error: "" }, 400);
  }
  return invoke("shot_ocr", { dataUrl });
}

// ===== 片段库 =====

export async function listSnippets(): Promise<Snippet[]> {
  if (!IS_TAURI) {
    mockSnippetState ??= clone((await mocks()).mockSnippets);
    return delay(mockSnippetState);
  }
  return invoke<Snippet[]>("list_snippets");
}

export async function saveSnippet(
  id: string | null,
  name: string,
  body: string
): Promise<Snippet> {
  if (!IS_TAURI) {
    mockSnippetState ??= clone((await mocks()).mockSnippets);
    const now = Date.now();
    const existing = id ? mockSnippetState.find((item) => item.id === id) : null;
    const next: Snippet = existing
      ? { ...existing, name: name || existing.name, body, updated_at: now }
      : {
          id: `mock-${now}`,
          name: name || body.split("\n")[0].slice(0, 40) || "未命名片段",
          body,
          created_at: now,
          updated_at: now,
        };
    mockSnippetState = [next, ...mockSnippetState.filter((item) => item.id !== next.id)];
    return delay(next);
  }
  return invoke<Snippet>("save_snippet", { id, name, body });
}

export async function deleteSnippet(id: string): Promise<void> {
  if (!IS_TAURI) {
    mockSnippetState ??= clone((await mocks()).mockSnippets);
    mockSnippetState = mockSnippetState.filter((item) => item.id !== id);
    return delay(undefined, 40);
  }
  return invoke("delete_snippet", { id });
}

// ===== 启动器 =====

/** 近 30 天的项目清单（只取展示字段） */
export async function launcherProjects(): Promise<LauncherProject[]> {
  if (!IS_TAURI) return delay((await mocks()).mockProjects);
  const to = Date.now();
  const from = to - 30 * 86_400_000;
  const rows = await invoke<
    { key: string; display_path: string | null; sessions: number }[]
  >("get_projects", { fromMs: from, toMs: to });
  return rows.map((row) => ({
    key: row.key,
    display_path: row.display_path,
    sessions: row.sessions,
  }));
}

// ===== 会议录音 =====

let mockMeetingItems: MeetingItem[] | null = null;
let mockPrivacyConfirmed = false;
let mockAutoGain = true;
const MOCK_MEETING_DIR = "C:\\Users\\demo\\.agenthud\\meetings";
/** 预览用的假录音地址（懒生成一次，60 秒近静音 WAV Blob） */
let mockMeetingAudioUrl = "";

/** 浏览器预览没有真实 wav：合成一段 60 秒、极轻的 220Hz 音，播放器 UI 可走通 */
function mockMeetingTone(): string {
  if (mockMeetingAudioUrl) return mockMeetingAudioUrl;
  const rate = 16_000;
  const seconds = 60;
  const count = rate * seconds;
  const buffer = new ArrayBuffer(44 + count * 2);
  const view = new DataView(buffer);
  const writeText = (offset: number, text: string) => {
    for (let i = 0; i < text.length; i += 1) view.setUint8(offset + i, text.charCodeAt(i));
  };
  writeText(0, "RIFF");
  view.setUint32(4, 36 + count * 2, true);
  writeText(8, "WAVEfmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, rate, true);
  view.setUint32(28, rate * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  writeText(36, "data");
  view.setUint32(40, count * 2, true);
  for (let i = 0; i < count; i += 1) {
    view.setInt16(44 + i * 2, Math.round(Math.sin((i / rate) * 2 * Math.PI * 220) * 40), true);
  }
  mockMeetingAudioUrl = URL.createObjectURL(new Blob([buffer], { type: "audio/wav" }));
  return mockMeetingAudioUrl;
}

/** 浏览器预览用的假录制会话（真机不参与：全部走 invoke） */
interface MockMeetingSession {
  id: string;
  device: string;
  sampleRate: number;
  startWall: number;
  pausedAccumMs: number;
  pauseAt: number | null;
  marks: MeetingMark[];
}

let mockMeetingSession: MockMeetingSession | null = null;

function mockMeetingElapsedMs(session: MockMeetingSession): number {
  const pausedNow =
    session.pauseAt != null ? Date.now() - session.pauseAt : 0;
  return Math.max(0, Date.now() - session.startWall - session.pausedAccumMs - pausedNow);
}

function mockMeetingStamp(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(
    d.getHours()
  )}${pad(d.getMinutes())}`;
}

/** 枚举音频输入设备（下拉用） */
export async function meetingDevices(): Promise<MeetingAudioDevice[]> {
  if (!IS_TAURI) return delay(clone((await mocks()).mockMeetingDevices));
  return invoke<MeetingAudioDevice[]>("meeting_devices");
}

/** 播放地址：后端把 wav 放行进 asset 协议，这里转成 `<audio>` 能用的 URL */
export async function meetingAudio(id: string): Promise<string> {
  if (!IS_TAURI) return mockMeetingTone();
  const path = await invoke<string>("meeting_audio", { id });
  return convertFileSrc(path);
}

/** 会议录音设置（目前只有「自动音量」） */
export async function meetingSettings(): Promise<MeetingSettings> {
  if (!IS_TAURI) {
    return delay({ privacyConfirmed: mockPrivacyConfirmed, autoGain: mockAutoGain });
  }
  return invoke<MeetingSettings>("meeting_settings");
}

/** 开关「保存时自动提升音量」 */
export async function meetingSetAutoGain(enabled: boolean): Promise<MeetingSettings> {
  if (!IS_TAURI) {
    mockAutoGain = enabled;
    return delay({ privacyConfirmed: mockPrivacyConfirmed, autoGain: mockAutoGain });
  }
  return invoke<MeetingSettings>("meeting_set_auto_gain", { enabled });
}

/** 首次使用隐私确认状态 + 保存目录 */
export async function meetingPrivacy(): Promise<MeetingPrivacy> {
  if (!IS_TAURI) {
    return delay({ confirmed: mockPrivacyConfirmed, dir: MOCK_MEETING_DIR });
  }
  return invoke<MeetingPrivacy>("meeting_privacy");
}

/** 用户确认「录音保存到本机磁盘」后持久化 */
export async function meetingConfirmPrivacy(): Promise<boolean> {
  if (!IS_TAURI) {
    mockPrivacyConfirmed = true;
    return delay(true);
  }
  return invoke<boolean>("meeting_confirm_privacy");
}

/** 开始录音；device 传 null 用默认输入设备 */
export async function meetingStart(device: string | null): Promise<MeetingSessionInfo> {
  if (!IS_TAURI) {
    const stamp = mockMeetingStamp();
    mockMeetingSession = {
      id: stamp,
      device: device ?? "麦克风阵列 (Realtek Audio)",
      sampleRate: 16_000,
      startWall: Date.now(),
      pausedAccumMs: 0,
      pauseAt: null,
      marks: [],
    };
    return delay({
      id: stamp,
      name: stamp,
      path: `${MOCK_MEETING_DIR}\\${stamp}.wav`,
      dir: MOCK_MEETING_DIR,
      started_at: Date.now(),
      device: mockMeetingSession.device,
      sample_rate: 16_000,
    });
  }
  return invoke<MeetingSessionInfo>("meeting_start", { device });
}

export async function meetingPause(): Promise<void> {
  if (!IS_TAURI) {
    if (mockMeetingSession && mockMeetingSession.pauseAt == null) {
      mockMeetingSession.pauseAt = Date.now();
    }
    return delay(undefined, 20);
  }
  return invoke("meeting_pause");
}

export async function meetingResume(): Promise<void> {
  if (!IS_TAURI) {
    if (mockMeetingSession && mockMeetingSession.pauseAt != null) {
      mockMeetingSession.pausedAccumMs += Date.now() - mockMeetingSession.pauseAt;
      mockMeetingSession.pauseAt = null;
    }
    return delay(undefined, 20);
  }
  return invoke("meeting_resume");
}

/** 录制中打时间标记（label 可选），返回打在有效时间轴上的标记 */
export async function meetingAddMark(label: string | null): Promise<MeetingMark> {
  if (!IS_TAURI) {
    const session = mockMeetingSession;
    if (!session) throw new Error("当前没有正在进行的会议录音");
    const mark: MeetingMark = { ms: mockMeetingElapsedMs(session), label: (label ?? "").trim() };
    session.marks.push(mark);
    return delay(mark, 20);
  }
  return invoke<MeetingMark>("meeting_add_mark", { label });
}

/** 录制中的会话快照（null = 空闲）；子页重挂载后恢复录制视图用 */
export async function meetingState(): Promise<MeetingSessionBrief | null> {
  if (!IS_TAURI) {
    const session = mockMeetingSession;
    if (!session) return delay(null, 0);
    return delay({
      id: session.id,
      name: session.id,
      device: session.device,
      sample_rate: session.sampleRate,
      started_at: session.startWall,
      paused: session.pauseAt != null,
      effective_ms: mockMeetingElapsedMs(session),
      marks: clone(session.marks),
      dir: MOCK_MEETING_DIR,
    });
  }
  return invoke<MeetingSessionBrief | null>("meeting_state");
}

/** 结束录音并落盘（后端回填 WAV 头 + 写侧车 + rename） */
export async function meetingStop(): Promise<MeetingStopResult> {
  if (!IS_TAURI) {
    const session = mockMeetingSession;
    if (!session) throw new Error("当前没有正在进行的会议录音");
    const durationMs = mockMeetingElapsedMs(session);
    const result: MeetingStopResult = {
      id: session.id,
      name: session.id,
      path: `${MOCK_MEETING_DIR}\\${session.id}.wav`,
      duration_ms: durationMs,
      size: Math.max(44, Math.round(durationMs * 32)),
      marks: session.marks.length,
      sample_rate: session.sampleRate,
      warning: null,
    };
    mockMeetingItems ??= clone((await mocks()).mockMeetings);
    mockMeetingItems = [
      {
        id: session.id,
        name: session.id,
        started_at: session.startWall,
        duration_ms: durationMs,
        size: result.size,
        sample_rate: session.sampleRate,
        device: session.device,
        marks: clone(session.marks),
      },
      ...mockMeetingItems,
    ];
    mockMeetingSession = null;
    return delay(result, 60);
  }
  return invoke<MeetingStopResult>("meeting_stop");
}

/** 最近录音列表（started_at 倒序） */
export async function meetingList(): Promise<MeetingItem[]> {
  if (!IS_TAURI) {
    mockMeetingItems ??= clone((await mocks()).mockMeetings);
    return delay(clone(mockMeetingItems));
  }
  return invoke<MeetingItem[]>("meeting_list");
}

export async function meetingDelete(id: string): Promise<void> {
  if (!IS_TAURI) {
    mockMeetingItems ??= clone((await mocks()).mockMeetings);
    mockMeetingItems = mockMeetingItems.filter((item) => item.id !== id);
    return delay(undefined, 40);
  }
  return invoke("meeting_delete", { id });
}

/** 重命名（wav 与侧车 json 一起改名），返回改名后的条目 */
export async function meetingRename(id: string, newName: string): Promise<MeetingItem> {
  if (!IS_TAURI) {
    mockMeetingItems ??= clone((await mocks()).mockMeetings);
    const target = mockMeetingItems.find((item) => item.id === id);
    if (!target) throw new Error("录音不存在或已被删除");
    // 按显示名查重（真实后端按文件主干查，主干恒等于名称；mock 种子数据的
    // id 可以与 name 不同，按 id 查重会漏掉「评审会」这类手工命名的种子项）
    if (mockMeetingItems.some((item) => item.name === newName && item.id !== id)) {
      throw new Error(`已存在同名录音「${newName}」`);
    }
    const next: MeetingItem = { ...target, id: newName, name: newName };
    mockMeetingItems = [next, ...mockMeetingItems.filter((item) => item.id !== id)];
    return delay(next, 40);
  }
  return invoke<MeetingItem>("meeting_rename", { id, newName });
}

/** 打开该录音所在文件夹 */
export async function meetingOpenFolder(id: string): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] open meeting folder", id);
    return delay(undefined, 20);
  }
  return invoke("meeting_open_folder", { id });
}
