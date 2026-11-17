/** 灵动岛外壳：收起胶囊 / 展开面板 / 标签导航 / 右键菜单 / 主题 / 事件桥 */

import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  addWeatherCity,
  addWeatherCityExact,
  bindWeatherCity,
  clearClipboard,
  clearNotify,
  clipboardDelete,
  clipboardLimit,
  clipboardPin,
  clipboardSetLimit,
  copyClipboard,
  copyText,
  ocrImage,
  beginIslandDrag,
  endIslandDrag,
  getClipboard,
  getIslandBackdrop,
  getIslandConfig,
  getIslandState,
  getLyrics,
  getMedia,
  getNotify,
  getQuotaSnapshot,
  getStocks,
  getSystem,
  getTodayUsage,
  getWeather,
  IS_TAURI,
  islandSetLastTab,
  islandSetPassthrough,
  islandPassthroughRestore,
  markNotifyReadKind,
  mediaAction,
  meetingState,
  onEvent,
  onLiveCaption,
  onLiveStatus,
  openIslandSettingsWindow,
  openMainWindow,
  openNotifyApp,
  openNotifySettings,
  openPath,
  openSystemSettings,
  quitApp,
  refreshQuota,
  refreshStocks,
  refreshWeather,
  removeWeatherCity,
  setBrightness,
  setIslandConfig,
  islandSetHoverRect,
  islandSetBodySize,
  islandSetContextMenuOpen,
  liveCaptions,
  liveClearCaptions,
  liveStart,
  liveStatus as getLiveStatus,
  liveStop,
  setIslandEnabled,
  setIslandExpanded,
  setKeyboard,
  setVolume,
  stepIslandDrag,
  toggleMute,
  toggleRadio,
  unbindWeatherCity,
} from "./api";
const DEFAULT_QUOTA_SIZES: IslandDragSizes = {
  horizontal: { width: 300, height: 54 },
  vertical: { width: 64, height: 160 },
};

/** 展开面板宽度（逻辑 px）。P4 画布 780×632：主体 640 居中，吸附边内缩 70。 */
const PANEL_W = 640;
const COMPACT_QUOTA_CELL = 36;
const COMPACT_QUOTA_PADDING = 10;
/** 大画布在吸附边一侧留出的离屏余量（液桥 / 残滴 / 投影），Rust INSET 同名。 */
const CANVAS_INSET = INSET;
const STATIC_EDGE_LINES = {
  top: CANVAS_INSET,
  bottom: CANVAS_H - CANVAS_INSET,
  left: CANVAS_INSET,
  right: CANVAS_W - CANVAS_INSET,
};

/** liquid 引擎空闲轮询间隔：稳定休眠后按此频率只读比较输入指纹，输入变化即唤醒；
 *  有唤醒钩子漏挂时最坏延迟即此值，可自愈。 */
const LIQUID_IDLE_POLL_MS = 500;

/**
 * 每个标签的展开面板高度（常量表，含 42px 导航 + 内容区上下内边距）。
 * 数值按 640 宽下各页真实内容高度实测（Chrome 672×520 预览，mock 数据）：
 *   home 492（主页按可用高度自动缩放）、msg 380、stock 440、weather 440、
 *   ctl 464、clip 424、shot 448、translate 492、tool 440。
 * translate 取到上限 492：翻译页多了一块「实时字幕」（状态 + 电平条 + 历史列表），
 * 412 装不下，会把整页顶出滚动条。
 * 不做「一律 492」的固定高度：页面短就长得短，切页时高度由 440ms 接近
 * Spring(440, 0.6) 的曲线补间。设置已迁到独立窗口，岛内有 9 个标签
 *（与 Rust ISLAND_TAB_KEYS 一致，不含 settings）。
 */
const PANEL_HEIGHT: Record<TabKey, number> = {
  home: 492,
  msg: 380,
  stock: 440,
  weather: 440,
  ctl: 464,
  clip: 424,
  shot: 448,
  translate: 492,
  tool: 440,
};

/** left/right 是竖胶囊；top/bottom 是横条（bottom 时窗口贴工作区下沿）。 */
const isSideEdge = (edge: IslandEdge) => edge === "left" || edge === "right";

/** P4：主体在 780×632 画布里的 anchored 位置（吸附边内缩 70，沿边居中）。 */
function anchoredBodyPos(edge: IslandEdge, width: number, height: number) {
  if (edge === "top") return { x: (CANVAS_W - width) / 2, y: CANVAS_INSET };
  if (edge === "bottom")
    return { x: (CANVAS_W - width) / 2, y: CANVAS_H - CANVAS_INSET - height };
  if (edge === "left") return { x: CANVAS_INSET, y: (CANVAS_H - height) / 2 };
  return { x: CANVAS_W - CANVAS_INSET - width, y: (CANVAS_H - height) / 2 };
}

/** P4 预览钩子：?liquid=top:40 / ?liquid=bottom:70&drip=1，用于 CDP 拍拖拽/断桥。 */
function parseLiquidPreview(): { edge: LiquidEdge; gap: number; drip: boolean } | null {
  if (typeof window === "undefined") return null;
  const params = new URLSearchParams(window.location.search);
  const raw = params.get("liquid");
  if (!raw) return null;
  const [edgeRaw, gapRaw] = raw.split(":");
  const edge: LiquidEdge =
    edgeRaw === "bottom" || edgeRaw === "left" || edgeRaw === "right" ? edgeRaw : "top";
  const gap = Math.max(0, Math.min(BREAK + 12, Number(gapRaw) || 0));
  return { edge, gap, drip: params.get("drip") === "1" };
}

const isIslandEdge = (value: string | null | undefined): value is IslandEdge =>
  value === "top" || value === "bottom" || value === "left" || value === "right";

/** 面板高度硬上限（窗口常驻 520 高：上 6 + 面板 492 + 下 16 + 余量）。 */
const MAX_PANEL_H = 492;

/**
 * 浏览器预览使用 300ms 双击间隔。原生窗口启动后读取系统间隔。
 */
const SYSTEM_DOUBLE_CLICK_MS = 300;
/** 单击展开后 600ms 内不因 hover-out 立即收起（pin 锁）。 */
const CLICK_PIN_MS = 600;

interface IslandPress {
  id: number;
  time: number;
  x: number;
  y: number;
  expanded: boolean;
  navigation: boolean;
  moved: boolean;
}

/** 主题菜单项：值与 Rust config.theme 对齐。 */
const THEME_MODES: { key: IslandThemeMode; label: string }[] = [
  { key: "auto", label: "跟随背景" },
  { key: "light", label: "浅色毛玻璃" },
  { key: "dark", label: "深色毛玻璃" },
  { key: "rouge", label: "胭脂撞色" },
  { key: "sky", label: "晴空渐变" },
];

/** 展开方式菜单项。 */
const EXPAND_MODES: { key: ExpandMode; label: string }[] = [
  { key: "hover", label: "悬停展开" },
  { key: "always", label: "常驻展开" },
  { key: "never", label: "长关" },
];

/** 「显示板块」可勾选的板块（只有主页固定显示，不在此列）。 */
const SECTION_TABS: { key: TabKey; label: string; icon: IconName }[] = [
  { key: "msg", label: "消息", icon: "message" },
  { key: "stock", label: "股票", icon: "chart" },
  { key: "weather", label: "天气", icon: "weather" },
  { key: "ctl", label: "控制", icon: "sliders" },
  { key: "clip", label: "剪贴板", icon: "clipboard" },
  { key: "shot", label: "截图", icon: "camera" },
  { key: "tool", label: "工具", icon: "toolbox" },
  { key: "translate", label: "翻译", icon: "translate" },
];

/** 收起态视图序列（滚轮切换：rings → clock → pet → music → rings）。 */
const COMPACT_VIEWS: CompactView[] = ["rings", "clock", "pet", "music"];
/** 通知 kind → 图标（复用项目现有图标名；颜色走 KIND_COLORS）。 */
const KIND_ICONS: Record<string, IconName> = {
  qq: "message",
  wechat: "message",
  mail: "note",
  feishu: "message",
  dingtalk: "message",
  stock: "chart",
  other: "message",
};

/** auto 主题的亮度阈值与滞回时间。 */
const AUTO_LIGHT_LUM = 0.64;
const AUTO_DARK_LUM = 0.42;
const AUTO_SWITCH_MS = 3000;

function clampNumber(value: unknown, min: number, max: number, fallback: number): number {
  const num = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(num)) return fallback;
  return Math.max(min, Math.min(max, num));
}

function normalizeExpandMode(value: unknown): ExpandMode {
  return value === "always" || value === "never" ? value : "hover";
}

function normalizeThemeMode(value: unknown): IslandThemeMode {
  return value === "light" || value === "dark" || value === "rouge" || value === "sky" || value === "auto"
    ? value
    : "dark";
}

function normalizeCompactView(value: unknown): CompactView {
  return value === "clock" || value === "pet" || value === "music" || value === "rings"
    ? value
    : "rings";
}

function normalizeTab(value: unknown): TabKey | null {
  return TABS.some((item) => item.key === value) ? (value as TabKey) : null;
}

/** 配置相等判断（纯 JSON 结构；键序由 {...current,...patch} 保持稳定） */
function sameIslandConfig(a: IslandConfig, b: IslandConfig): boolean {
  try {
    return JSON.stringify(a) === JSON.stringify(b);
  } catch {
    return false;
  }
}

/** 本批配置写完后，从 pendingPatch 里清掉「值仍等于本批提交值」的键。 */
function clearSettledConfigPatch(
  pending: Partial<IslandConfig>,
  batch: Partial<IslandConfig>
): void {
  for (const key of Object.keys(batch) as Array<keyof IslandConfig>) {
    let same = false;
    try {
      same = JSON.stringify(pending[key]) === JSON.stringify(batch[key]);
    } catch {
      same = false;
    }
    if (same) delete pending[key];
  }
}

/** 复用已解码的底板，按参考主题的亮度权重取样。
 *
 * 仅用于 Aligned 帧上屏路径（excludeFromCapture 开启时的全窗折射帧）：
 * 解码本就必须发生（折射上屏），在其上追加 32×32 采样不新增解码位图的
 * 生命周期。Neighbor 帧（未开排除）的亮度由 Rust 随帧算好下发
 * （captured.luminance），前端不再为采样而解码——那是渲染进程原生内存
 * 无上限堆积的旧通路。 */
function sampleBackdropLuminance(image: HTMLImageElement): number | null {
  try {
    const canvas = document.createElement("canvas");
    canvas.width = 32;
    canvas.height = 32;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) return null;
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
    let total = 0;
    let count = 0;
    for (let i = 0; i < data.length; i += 4) {
      if (data[i + 3] < 8) continue;
      total += (0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2]) / 255;
      count += 1;
    }
    return count ? total / count : null;
  } catch {
    return null;
  }
}

import type {
  ClipItem,
  CompactView,
  ExpandMode,
  HudBarAgent,
  HudBarQuota,
  HudBarUsage,
  IslandBodyRect,
  IslandConfig,
  IslandDragStep,
  IslandDragSizes,
  IslandEdge,
  IslandState,
  IslandThemeMode,
  IslandThemePalette,
  LauncherItem,
  LiveCaption,
  LiveStatus,
  LyricsState,
  MediaState,
  MeetingSessionBrief,
  NotifyItem,
  NotifyState,
  StockAlert,
  StocksState,
  SystemState,
  TabKey,
  TodayUsageTotals,
  WeatherState,
} from "./types";
import { LIVE_CAPTION_HOLD_MS, LIVE_CAPTION_LIMIT, tempUnitSuffix } from "./types";
import {
  agentLabel,
  BrandMark,
  currentLyricLine,
  fmtClock,
  fmtMoney,
  Icon,
  type IconName,
  NowProvider,
  parseLyrics,
  quotaTone,
  remainingPct,
  Slider,
  WeatherGlyph,
} from "./ui";
import {
  COMPACT_HEIGHT,
  LIVE_CAPTION_HEIGHT,
  CompactView as CompactViewSurface,
  activityWidthFor,
  badgeList,
  compactMusicHeightFor,
  compactText,
  compactWidthFor,
  liveCaptionWidthFor,
  longestLyricText,
  type BadgeCounts,
  type CompactActivity,
  type CompactLiveCaption,
  type CompactViewKey,
} from "./CompactView";
import { fontFamilyWithFallback, ISLAND_FONT_STACK } from "./fontFamily";
import { KIND_COLORS, NOTIFY_BRAND_ICONS, NOTIFY_BRAND_LABELS, NotifyBrandIcon } from "./notifyBrand";
import { createGlassBackdrop, type GlassBackdrop, type GlassBackdropGeometry } from "./glassBackdrop";
import { useNativeGlass } from "./nativeGlass";
import {
  NOTIFY_ACTIVITY_MS,
  NOTIFY_LEAD_MS,
  decideNotifyReaction,
  notifyPulseColor,
  notifyPulseIntensity,
  notifyPulseWindow,
  type NotifyWave,
} from "./notifyPulse";
import {
  BREAK,
  CANVAS_H,
  CANVAS_W,
  FILLET,
  INSET,
  JOIN,
  bodyRadius,
  cssPath,
  liquidPath,
  nativeGlassBody,
  liquidRadii,
  type LiquidDrip,
  type LiquidEdge,
} from "./liquid";
import {
  advanceSpring,
  hasDragMovement,
  LONG_PRESS_MS,
  nearestScreenEdge,
  screenGap,
  springAt,
  type Spring,
} from "./motion";
import type { CompactPointerSample } from "./PetFace";
import {
  ClipboardPage,
  ControlsPage,
  HomePage,
  MessagesPage,
  StocksPage,
  WeatherPage,
} from "./pages";
import { TranslatePanel } from "./TranslatePanel";
// 重页拆独立 chunk（内存压降专项 5）：截图编辑器与工具箱（连带其内部的
// 会议录音面板）代码量大且非首屏，React.lazy 拆出去后 island 主 chunk 不
// 再包含；首次点击对应 tab 时按需加载（挂载后有空闲期预取，见下方 effect）。
// 首页保持同步 import，保证展开首帧零延迟。
const ScreenshotPage = lazy(() =>
  import("./ShotPage").then((m) => ({ default: m.ScreenshotPage }))
);
const ToolboxPage = lazy(() =>
  import("./ToolboxPage").then((m) => ({ default: m.ToolboxPage }))
);

const WINDOWS_TAURI = IS_TAURI && typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
/** Windows 采集桌面，浏览器预览可用 ?backdrop=1 查看折射。 */
const CAN_CAPTURE_BACKDROP = WINDOWS_TAURI ||
  (!IS_TAURI &&
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("backdrop") === "1");

/**
 * 收起态面板子树延时卸载（内存压降专项 3a，保守方案）：收起持续 10 分钟
 * 后卸载 island-panel 子树（nav + 8 个 tab 页 + 图表），保留胶囊常驻。
 *
 * 不采用「open 即条件渲染」的原因：面板常驻 DOM 是展开/收起 CSS 过渡
 * （opacity/visibility/transform，见 island.css .island-panel）工作的前提，
 * 收起即卸载会让面板在展开首帧无过渡地"瞬间出现"——灵动岛是最高频可见
 * UI，动画回归不可接受。改用延时卸载：expand() 入口同步预挂（距
 * setOpen(true) 还有窗口视口等待 + 双 rAF 的帧余量，一次 commit 完成），
 * 展开时面板已在 DOM，过渡与原行为完全一致。卸载会丢失面板内未提交的
 * 输入态（搜索词、设置项草稿），10 分钟无交互后可接受。
 */
const PANEL_UNMOUNT_DELAY_MS = 10 * 60_000;

const TABS: { key: TabKey; label: string; icon: IconName }[] = [
  { key: "home", label: "主页", icon: "home" },
  { key: "msg", label: "消息", icon: "message" },
  { key: "stock", label: "股票", icon: "chart" },
  { key: "weather", label: "天气", icon: "weather" },
  { key: "ctl", label: "控制", icon: "sliders" },
  { key: "clip", label: "剪贴板", icon: "clipboard" },
  { key: "shot", label: "截图", icon: "camera" },
  { key: "translate", label: "翻译", icon: "translate" },
  { key: "tool", label: "工具", icon: "toolbox" },
];

const EMPTY_MEDIA: MediaState = {
  hasSession: false,
  canToggle: false,
  appId: "",
  appName: "",
  title: "",
  artist: "",
  album: "",
  playing: false,
  position: 0,
  duration: 0,
  canPrev: false,
  canNext: false,
  cover: "",
  source: "",
};

/** 股票代码比较：腾讯行情的键大小写不固定（usAAPL / usaapl），统一忽略大小写。 */
function sameStock(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

const EMPTY_SYSTEM: SystemState = {
  volume: 0,
  muted: false,
  brightness: null,
  brightnessSupported: false,
  wifi: null,
  wifiAvailable: false,
  bluetooth: null,
  bluetoothAvailable: false,
  error: "",
  updatedAt: 0,
};

/** 胶囊上的会议录音计时：mm:ss 且分钟恒补零。字符数不随时间变化，
 *  tabular-nums 下宽度稳定，胶囊的自适应测量不会因计时跳秒而变形。 */
function fmtMeetClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

export default function IslandApp() {
  const [config, setConfig] = useState<IslandConfig | null>(null);
  const [enabled, setEnabled] = useState(true);
  const [captureExcluded, setCaptureExcluded] = useState(false);
  const [open, setOpen] = useState(
    () => !IS_TAURI && new URLSearchParams(window.location.search).get("open") === "1"
  );
  const [tab, setTab] = useState<TabKey>("home");
  const [usage, setUsage] = useState<HudBarUsage | null>(null);
  const [todayUsage, setTodayUsage] = useState<TodayUsageTotals | null>(null);
  const [weather, setWeather] = useState<WeatherState | null>(null);
  const [stocks, setStocks] = useState<StocksState | null>(null);
  const [media, setMedia] = useState<MediaState>(EMPTY_MEDIA);
  const [lyrics, setLyrics] = useState<LyricsState | null>(null);
  const [system, setSystem] = useState<SystemState>(EMPTY_SYSTEM);
  const [clipboard, setClipboard] = useState<ClipItem[]>([]);
  /** 剪贴板保留条数上限（后端状态，页面首次渲染读一次） */
  const [clipLimit, setClipLimit] = useState(10);
  const [notify, setNotify] = useState<NotifyState>({ items: [], unread: 0 });
  /** 会议录音会话快照（null = 空闲）：收起胶囊上的录音指示数据源 */
  const [meetingBrief, setMeetingBrief] = useState<MeetingSessionBrief | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [toast, setToast] = useState("");
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; launcher?: LauncherItem } | null>(null);
  const launcherPickerOpen = useRef(false);
  /** 右键菜单当前展开的子菜单（主题 / 展开方式 / 显示板块） */
  const [contextSubmenu, setContextSubmenu] = useState<"theme" | "mode" | "tabs" | null>(null);
  const [edge, setEdge] = useState<IslandEdge>("top");
  const [dragging, setDragging] = useState(false);
  /** 主题模式（config.theme）；auto 的实际色板由 autoDark 决定 */
  const [themeMode, setThemeMode] = useState<IslandThemeMode>("dark");
  const [autoDark, setAutoDark] = useState(true);
  // ===== P1 收起态四视图 / activity =====
  /** 运行时的视图覆盖（滚轮、音乐自动切换、自动轮换用；不改配置） */
  const [viewOverride, setViewOverride] = useState<CompactView | null>(null);
  /** 收起态临时提醒（优先级最高，到时自动消失） */
  const [activity, setActivity] = useState<CompactActivity | null>(null);
  const [compactPressed, setCompactPressed] = useState(false);
  /** 新消息计数器：pet 收到 +1 就惊讶 1.4s */
  const [surpriseToken, setSurpriseToken] = useState(0);
  const [pageVisible, setPageVisible] = useState(() => !document.hidden);
  /** 交互计数：滚轮 / 点击后重置 autoCycle 计时 */
  const [interactSeq, setInteractSeq] = useState(0);
  // ===== 实时字幕（收起态字幕层 + 翻译页历史）=====
  /** 运行快照（命令返回 + `zbar://island-live` 事件；null = 后端还没就绪） */
  const [liveStatus, setLiveStatus] = useState<LiveStatus | null>(null);
  /** 最新一条字幕（收起态胶囊只显示它） */
  const [liveCaption, setLiveCaption] = useState<LiveCaption | null>(null);
  /** 最新字幕的本地到达时间（决定「最后一句后 7 秒回原视图」） */
  const [liveCaptionAt, setLiveCaptionAt] = useState(0);
  /** 是否仍在 7 秒字幕窗口内（定时器驱动；胶囊宽高跟它走） */
  const [liveCaptionOn, setLiveCaptionOn] = useState(false);
  /** 最近 80 条字幕历史（旧 → 新，最新在末尾） */
  const [liveHistory, setLiveHistory] = useState<LiveCaption[]>([]);
  /** 开始 / 停止请求进行中 */
  const [liveBusy, setLiveBusy] = useState(false);
  const liveRunning = Boolean(liveStatus?.running);
  // 面板子树挂载开关（内存压降专项 3a）：收起 10 分钟后卸载、expand 入口
  // 预挂，见 PANEL_UNMOUNT_DELAY_MS 注释。初始挂载保持首帧行为与原版一致。
  const [panelMounted, setPanelMounted] = useState(true);

  // 用户自定义顺序（设置页里上下挪）：不在表里的条目保持数据源原顺序排在后面。
  // 必须放在 config 的早退分支之前——hooks 不能有条件地跳过。
  const orderedUsage = useMemo(() => {
    if (!usage?.agents?.length) return usage;
    const order = config?.agentOrder ?? [];
    if (!order.length) return usage;
    const rank = new Map(order.map((id, index) => [id, index]));
    const agents = [...usage.agents].sort((a, b) => {
      const ra = rank.get(a.id);
      const rb = rank.get(b.id);
      if (ra == null && rb == null) return 0;
      if (ra == null) return 1;
      if (rb == null) return -1;
      return ra - rb;
    });
    return { ...usage, agents };
  }, [usage, config]);

  const rootRef = useRef<HTMLDivElement | null>(null);
  const islandRef = useRef<HTMLDivElement | null>(null);
  /** 面板滚动容器：剪贴板热键切页后滚回顶部用 */
  const contentRef = useRef<HTMLDivElement | null>(null);
  const [quotaSizes, setQuotaSizes] = useState(DEFAULT_QUOTA_SIZES);
  const resizeQuota = useCallback((
    orientation: keyof IslandDragSizes,
    size: IslandDragSizes[keyof IslandDragSizes],
  ) => {
    setQuotaSizes((current) =>
      current[orientation].width === size.width && current[orientation].height === size.height
        ? current
        : { ...current, [orientation]: size }
    );
  }, []);
  const collapseTimer = useRef<number | null>(null);
  const hoverTimer = useRef<number | null>(null);
  const shrinkTimer = useRef<number | null>(null);
  const toastTimer = useRef<number | null>(null);
  const configTimer = useRef<number | null>(null);
  const configRef = useRef<IslandConfig | null>(null);
  // 连续快速改配置（例如连点删除多只股票）时，串行落盘并忽略旧响应，
  // 否则后返回的旧快照会把刚删掉的股票又写回去。
  const configRevision = useRef(0);
  const configSaveQueue = useRef<Promise<void>>(Promise.resolve());
  /** 尚未确认落盘的累计配置改动：写前重读权威配置后叠加它再整份写回（P1-3） */
  const configPendingRef = useRef<Partial<IslandConfig>>({});
  const openRef = useRef(open);
  const bootRef = useRef(false);
  /** island_state.expanded：挂载 / WebView 重载后按后端真实展开态恢复（P2-12） */
  const backendExpandedRef = useRef(false);
  /** boot 时从 lastTab/defaultTab 解析出的初始页；持久化 effect 等它生效再写（P2-11） */
  const bootTabRef = useRef<TabKey | null>(null);
  /** 当前标签：事件回调（剪贴板热键切页）里读，避免闭包拿到旧的 tab state */
  const tabRef = useRef<TabKey>(tab);
  /** 常驻展开的延迟计时器（boot 用），卸载/关闭要清 */
  const bootExpandTimer = useRef<number | null>(null);
  const backdropCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const backdropFallbackRef = useRef<HTMLImageElement | null>(null);
  const glassBackdropRef = useRef<GlassBackdrop | null>(null);
  const glassBackdropGeometryRef = useRef<GlassBackdropGeometry | null>(null);
  const refreshBackdropRef = useRef<(() => void) | null>(null);
  // ===== P4 液态外壳（单一外轮廓：glass clip + SVG stroke） =====
  const bodyFrameRef = useRef<HTMLDivElement | null>(null);
  const liquidGlassRef = useRef<HTMLDivElement | null>(null);
  const liquidRimRef = useRef<SVGPathElement | null>(null);
  const liquidRimGradientRef = useRef<SVGLinearGradientElement | null>(null);
  const liquidFlowRef = useRef<SVGPathElement | null>(null);
  const liquidFlowGradientRef = useRef<SVGRadialGradientElement | null>(null);
  /** liquid 引擎唤醒钩子：输入在事件回调 / 渲染期变化时调用，休眠中立即恢复 rAF。
   *  主循环 effect 内赋真实实现（见该处事故注释），未挂载时是空操作。 */
  const liquidWakeRef = useRef(() => {});
  const bodyTargetRef = useRef({ ...DEFAULT_QUOTA_SIZES.horizontal, open: false });
  const liquidPokeRef = useRef({ x: CANVAS_W / 2, y: CANVAS_INSET });
  const liquidPreviewRef = useRef(parseLiquidPreview());
  const dragFrameRef = useRef<IslandDragStep | null>(null);
  const neckRef = useRef({ edge: "top" as IslandEdge | null, spring: springAt(1) });
  const dripsRef = useRef<{ edge: IslandEdge; line: number; center: number; radius: Spring }[]>([]);
  const canvasOriginRef = useRef({ x: 0, y: 0 });
  const dragMonitorRef = useRef<string | null | undefined>(undefined);
  const snapEdgeRef = useRef<IslandEdge | null>(null);
  const pendingExpand = useRef(false);
  const openMenuRef = useRef(false);
  const edgeRef = useRef<IslandEdge>("top");
  const pointerRef = useRef<IslandPress | null>(null);
  const finishPressRef = useRef<(event: PointerEvent | null, cancelled?: boolean) => void>(() => {});
  const blockHoverUntilLeave = useRef(false);
  const dragActiveRef = useRef(false);
  const dragStartingRef = useRef(false);
  const dragEpochRef = useRef(0);
  const pokeState = useRef({ ...springAt(0), target: 0, pressing: false, stiffness: 380, ratio: 0.55 });
  const resetPokeRef = useRef<() => void>(() => {});
  const suppressClickRef = useRef(false);
  // ===== P0 新交互用的计时器 / 引用 =====
  /** 单击延迟计时（等系统双击间隔，双击时取消） */
  const clickTimer = useRef<number | null>(null);
  const doubleClickMs = useRef(SYSTEM_DOUBLE_CLICK_MS);
  const lastPillClick = useRef<{ x: number; y: number } | null>(null);
  /** 单击展开后的 pin 截止时间（performance.now）；防止 hover-out 立刻收起 */
  const pinUntil = useRef(0);
  /** 导航栏滚轮节流 */
  const navWheelAt = useRef(0);
  /** 收起态滚轮切视图节流（250ms，P1-4） */
  const compactWheelAt = useRef(0);
  /** 最近一次上报的胶囊矩形：相同尺寸不重复 IPC（P1-2） */
  const hoverRectRef = useRef("");
  const navRef = useRef<HTMLElement | null>(null);
  /** 主题相关 ref（事件 / 轮询回调里读最新值，避免重复注册监听） */
  const themeModeRef = useRef<IslandThemeMode>("dark");
  const autoDarkRef = useRef(true);
  /** auto 主题的平滑亮度（指数平滑，0.8 旧值 + 0.2 新值） */
  const luminanceRef = useRef(0.5);
  /** 越过亮度阈值后开始持续计时的时间戳；持续 3s 才真正切换 */
  const autoFlipSince = useRef<number | null>(null);
  /** 滚轮回调（挂在原生 wheel 监听上，用 ref 保证读到最新闭包） */
  const cycleCompactRef = useRef<(dir: number) => void>(() => {});
  const cycleTabRef = useRef<(dir: number) => void>(() => {});
  // ===== P1 收起态：activity / 事件比较 / pet 指针采样 =====
  const activityRef = useRef<CompactActivity | null>(null);
  const activityTimer = useRef<number | null>(null);
  const activitySeq = useRef(0);
  const seenNotifyIds = useRef(new Set<number>());
  const notifyBaselineReady = useRef(false);
  const systemRef = useRef<SystemState | null>(null);
  const mediaRef = useRef<MediaState>(EMPTY_MEDIA);
  const viewOverrideRef = useRef<CompactView | null>(null);
  /** 播放开始时记住的原视图（暂停 8s 后回到这里，参考 app.py） */
  const musicReturnView = useRef<CompactView | null>(null);
  const musicPauseTimer = useRef<number | null>(null);
  // ===== 新消息「岛端先动」：波次 / 临时 pet 覆盖 / 贴边呼吸灯 =====
  /** 当前新消息波次（同一波次只排一次惊讶与活动卡，呼吸灯按它续期） */
  const notifyWaveRef = useRef<NotifyWave | null>(null);
  /** 呼吸灯 DOM（绝对定位在胶囊内侧，pointer-events:none） */
  const notifyPulseRef = useRef<HTMLSpanElement | null>(null);
  /** 已亮起的呼吸灯：startAt 固定住呼吸相位，续期只改 endAt（参考 island.breath） */
  const notifyPulseState = useRef<{ startAt: number; endAt: number; raf: number } | null>(null);
  /** 本次波次最新的呼吸灯颜色（同一波次后续消息换成最新来源色） */
  const notifyPulseColorRef = useRef("");
  /** 惊讶用的临时 pet 视图覆盖：记住覆盖前的运行时视图，1.2s 后（或用户交互）释放 */
  const notifyPetRef = useRef<{ active: boolean; original: CompactView | null }>({
    active: false,
    original: null,
  });
  const notifyPetRaf = useRef<number | null>(null);
  /** t=1200 的编排计时器（亮灯 + 弹卡） */
  const notifyStageTimer = useRef<number | null>(null);
  /** 鼠标相对胶囊中心的采样（pet 视线的数据源，pointermove 直接写 ref 不触发渲染）。
   *  初始坐标故意放到很远：指针没进过窗口时 pet 走「东张西望」而不是盯着正中。 */
  const liquidAudioLevelRef = useRef(0);
  const compactPointerRef = useRef<CompactPointerSample>({ x: 9999, y: 9999, inside: false });
  // ===== P2：可穿透右键菜单（Rust 钩子广播坐标 → 前端开同一份菜单） =====
  /** 本次菜单是否由 zbar://island-passthrough-menu 打开（关闭时要 restore） */
  const passthroughMenuOpen = useRef(false);
  /** 菜单打开前的面板展开状态：关闭时回到这个状态（P1-6） */
  const passthroughMenuWasOpen = useRef(false);
  /** 本次菜单里是否点了菜单项（用户显式动作优先，不做状态回滚） */
  const passthroughMenuAction = useRef(false);
  /** 前端 9.5s 主动收菜单的兜底计时（Rust 侧另有 10s 兜底） */
  const passthroughMenuTimer = useRef<number | null>(null);
  /** 关闭菜单的唯一出口（Esc / 卸载等 hook 通过 ref 调用，避免依赖顺序问题） */
  const closeMenuRef = useRef<() => void>(() => {});
  const passthroughMenuHandlerRef = useRef<(payload: { x?: number; y?: number }) => void>(() => {});
  const wheelEventHandlerRef = useRef<(payload: { delta?: number }) => void>(() => {});

  configRef.current = config;
  openMenuRef.current = contextMenu !== null;
  edgeRef.current = edge;
  themeModeRef.current = themeMode;
  autoDarkRef.current = autoDark;
  activityRef.current = activity;
  viewOverrideRef.current = viewOverride;
  systemRef.current = system;
  mediaRef.current = media;

  const previewParams = useMemo(() => new URLSearchParams(window.location.search), []);
  const previewOpen = !IS_TAURI && previewParams.get("open") === "1";
  const previewTab = previewParams.get("tab") as TabKey | null;

  useEffect(() => {
    if (IS_TAURI) return;
    const wanted = previewParams.get("edge");
    if (isIslandEdge(wanted)) setEdge(wanted);
  }, [previewParams]);

  const showToast = (message: string) => {
    setToast(message);
    if (toastTimer.current != null) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(""), 2000);
  };

  // ===== 实时字幕：事件收敛 =====
  // 同一个 id 的草稿会被反复推送（覆盖同一行），定稿后不再变化；历史最多 80 条。
  // 事件回调闭包只捕获 setState，不会读到旧的 state。
  const applyLiveCaption = (caption: LiveCaption) => {
    if (!caption || typeof caption.id !== "number") return;
    setLiveCaption(caption);
    setLiveCaptionAt(Date.now());
    setLiveHistory((current) => {
      const index = current.findIndex((item) => item.id === caption.id);
      const next =
        index >= 0
          ? current.map((item, i) => (i === index ? caption : item))
          : [...current, caption];
      return next.length > LIVE_CAPTION_LIMIT ? next.slice(next.length - LIVE_CAPTION_LIMIT) : next;
    });
  };

  // 最后一句后 7 秒回原视图。停止聆听时后端会补推最后一句的定稿字幕，
  // 所以这里只看 7 秒保鲜期、不要求 liveRunning：停止后 running 立即为 false，
  // 若继续依赖它，补推的定稿句就永远上不了胶囊。
  useEffect(() => {
    if (!liveCaptionAt) {
      setLiveCaptionOn(false);
      return;
    }
    const left = LIVE_CAPTION_HOLD_MS - (Date.now() - liveCaptionAt);
    if (left <= 0) {
      setLiveCaptionOn(false);
      return;
    }
    setLiveCaptionOn(true);
    const timer = window.setTimeout(() => setLiveCaptionOn(false), left + 20);
    return () => window.clearTimeout(timer);
  }, [liveCaptionAt]);

  /** 开始 / 停止实时字幕：失败只 toast，不打断岛上其它交互。 */
  const toggleLive = () => {
    if (liveBusy) return;
    setLiveBusy(true);
    const action = liveRunning ? liveStop() : liveStart();
    void action
      .then((next) => {
        if (!next) return;
        setLiveStatus(next);
        // 后端以「返回快照 + phase=error」表示启动失败（例如模型未下载）
        if (next.phase === "error") showToast(next.error || "实时字幕启动失败");
      })
      .catch((error) => showToast(`实时字幕操作失败：${String(error)}`))
      .finally(() => setLiveBusy(false));
  };

  /** 清空字幕历史（本地先清，后端返回值不作约定）。 */
  const clearLiveCaptions = () => {
    setLiveHistory([]);
    setLiveCaption(null);
    setLiveCaptionAt(0);
    setLiveCaptionOn(false);
    void liveClearCaptions().catch((error) => showToast(`清空字幕失败：${String(error)}`));
  };

  // ===== 启动加载 =====
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const state: IslandState = await getIslandState();
        if (!alive) return;
        if (state.doubleClickMs && Number.isFinite(state.doubleClickMs)) {
          doubleClickMs.current = state.doubleClickMs;
        }
        setConfig(state.config);
        configRef.current = state.config;
        setCaptureExcluded(state.config.excludeFromCapture === true);
        backendExpandedRef.current = Boolean(state.expanded);
        const wantedEdge = previewParams.get("edge");
        setEdge(isIslandEdge(wantedEdge) ? wantedEdge : state.config.edge ?? "top");
        setEnabled(state.enabled);
        void getQuotaSnapshot().then((data) => alive && data && setUsage(data));
        void getTodayUsage().then((data) => alive && data && setTodayUsage(data));
        void getWeather().then((data) => alive && data && setWeather(data));
        void getStocks().then((data) => alive && data && setStocks(data));
        // 首屏数据的 ref 也要同步：后续事件处理要靠它比较「变化前」的值，
        // 否则第一帧就把初始音量/通知当成新变化弹一次 activity。
        void getMedia().then((data) => {
          if (!alive || !data) return;
          handleMedia(data, true);
        });
        void getSystem().then((data) => {
          if (!alive || !data) return;
          systemRef.current = data;
          setSystem(data);
        });
        void getClipboard().then((data) => alive && setClipboard(data));
        void clipboardLimit().then((value) => alive && setClipLimit(value));
        void getNotify().then((data) => {
          if (!alive || !data || notifyBaselineReady.current) return;
          applyNotifyState(data);
        });
        // 实时字幕：首帧先拉运行快照与历史；后端（并行开发中）还没注册命令时
        // invoke 会 reject，这里静默降级成「未开始」，不影响灵动岛其它功能。
        void getLiveStatus()
          .then((data) => {
            if (alive && data) setLiveStatus(data);
          })
          .catch(() => {});
        void liveCaptions()
          .then((data) => {
            if (!alive || !Array.isArray(data)) return;
            setLiveHistory(data.slice(-LIVE_CAPTION_LIMIT));
            const last = data.length ? data[data.length - 1] : null;
            if (last) {
              setLiveCaption(last);
              // 历史最后一条可能已经是很旧的句子：用后端毫秒时间戳回填「到达时间」，
              // 否则重载后会被当成刚到达而白占胶囊 7 秒（胶囊只认 7 秒保鲜期）。
              const ts = Number(last.ts);
              setLiveCaptionAt(Number.isFinite(ts) && ts > 0 ? ts : Date.now());
            }
          })
          .catch(() => {});
      } catch (e) {
        if (alive) showToast(`灵动岛加载失败: ${String(e)}`);
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // ===== auto 主题亮度反馈 =====
  // 指数平滑（0.8 旧 + 0.2 新），越过 0.64 / 0.42 阈值后还要持续 3s 才切换，
  // 避免滚动的网页 / 视频让玻璃来回闪。只维护 autoDark，固定主题不受影响。
  const feedLuminance = useCallback((lum: number) => {
    const next = luminanceRef.current * 0.8 + Math.max(0, Math.min(1, lum)) * 0.2;
    luminanceRef.current = next;
    if (themeModeRef.current !== "auto") {
      autoFlipSince.current = null;
      return;
    }
    const dark = autoDarkRef.current;
    // 深色时亮度持续高于 0.64 → 切浅色；浅色时低于 0.42 → 切深色。
    const wantDark = dark ? next <= AUTO_LIGHT_LUM : next < AUTO_DARK_LUM;
    if (wantDark === dark) {
      autoFlipSince.current = null;
      return;
    }
    const now = performance.now();
    if (autoFlipSince.current == null) {
      autoFlipSince.current = now;
      return;
    }
    if (now - autoFlipSince.current < AUTO_SWITCH_MS) return;
    autoFlipSince.current = null;
    autoDarkRef.current = wantDark;
    setAutoDark(wantDark);
  }, []);

  // 开启时等后端应用窗口采集标志。用户关闭时立即回退。
  const nativeGlassEnabled = enabled && captureExcluded && config?.excludeFromCapture === true &&
    (config?.transparency ?? 0) > 0;
  const {
    active: nativeGlassActive,
    luminance: nativeGlassLuminance,
    updateGeometry: updateNativeGlassGeometry,
  } = useNativeGlass(WINDOWS_TAURI, nativeGlassEnabled);
  useEffect(() => {
    if (!nativeGlassActive || themeMode !== "auto") return;
    // 静止桌面也要让明暗切换的平滑和 3 秒去抖继续收敛。
    const sample = () => {
      if (nativeGlassLuminance.current != null) feedLuminance(nativeGlassLuminance.current);
    };
    sample();
    const timer = window.setInterval(sample, 250);
    return () => window.clearInterval(timer);
  }, [feedLuminance, nativeGlassActive, nativeGlassLuminance, themeMode]);

  // 只有窗口正下方的桌面帧参与折射，相邻帧仅用于自动主题的明暗判断。
  const wantsRefraction = !IS_TAURI || config?.excludeFromCapture === true;
  const sampleNeighbor = themeMode === "auto";
  const backdropReady = Boolean(config) && (wantsRefraction || sampleNeighbor);
  useEffect(() => {
    if (!CAN_CAPTURE_BACKDROP || !enabled || !backdropReady || nativeGlassActive) return;
    const canvas = backdropCanvasRef.current;
    const fallback = backdropFallbackRef.current;
    if (!canvas || !fallback) return;
    let alive = true;
    let timer: number | undefined;
    let capturing = false;
    let refreshOnFinish = false;
    let frame: HTMLImageElement | null = null;
    let knownRevision: number | undefined;
    let luminance: number | null = null;
    glassBackdropRef.current = wantsRefraction ? createGlassBackdrop(canvas) : null;

    const clearBackdrop = () => {
      canvas.classList.remove("is-ready");
      fallback.removeAttribute("src");
    };

    const uploadFrame = () => {
      if (!frame) return;
      const renderer = glassBackdropRef.current;
      if (renderer) {
        try {
          renderer.setFrame(frame);
          const geometry = glassBackdropGeometryRef.current;
          if (geometry) renderer.draw(geometry);
          canvas.classList.add("is-ready");
          fallback.removeAttribute("src");
          return;
        } catch {
          renderer.dispose();
          glassBackdropRef.current = null;
        }
      }
      canvas.classList.remove("is-ready");
      // 同值跳过：Rust revision 协议已保证 url 变化才下发，这里再防御一层，
      // 避免异常重放时每拍把同一 data URL 重新解码成光栅（旧光栅释放靠
      // GC，正是原生内存堆积的通路之一）。
      if (fallback.getAttribute("src") !== frame.src) fallback.src = frame.src;
    };
    const onLost = (event: Event) => {
      event.preventDefault();
      glassBackdropRef.current?.dispose();
      glassBackdropRef.current = null;
      uploadFrame();
    };
    const onRestored = () => {
      if (!wantsRefraction) return;
      glassBackdropRef.current = createGlassBackdrop(canvas);
      uploadFrame();
    };
    canvas.addEventListener("webglcontextlost", onLost);
    canvas.addEventListener("webglcontextrestored", onRestored);

    const interval = () => {
      const fps = clampNumber(configRef.current?.blurFps, 2, 30, 8);
      const delay = Math.max(60, Math.round(1000 / fps));
      return openRef.current || dragActiveRef.current ? delay : delay * 3;
    };
    const canCapture = () => !document.hidden;
    const schedule = (ms: number) => {
      if (!alive || timer !== undefined) return;
      timer = window.setTimeout(() => {
        timer = undefined;
        void tick();
      }, ms);
    };

    const tick = async () => {
      if (!alive || capturing) return;
      if (!canCapture()) {
        schedule(250);
        return;
      }

      capturing = true;
      const start = performance.now();
      try {
        const captured = await getIslandBackdrop(knownRevision, sampleNeighbor);
        if (!alive || !canCapture()) return;
        if (captured) {
          const showFrame = wantsRefraction && captured.aligned && Boolean(captured.url);
          let sampled: number | null = null;
          if (showFrame && captured.url) {
            // 唯一保留的解码路径：Aligned 帧真要上屏折射。Neighbor 帧的
            // 亮度由 Rust 随帧算好（captured.luminance），不再
            // new Image()+decode()——那条链每拍丢弃解码位图、释放全靠
            // GC，页面空闲时 GC 极少触发，是渲染进程原生内存无上限堆积
            // （6.4GB 事故）的来源。
            const incoming = new Image();
            incoming.src = captured.url;
            await incoming.decode();
            if (!alive || !canCapture()) return;
            frame = incoming;
            uploadFrame();
            // Aligned 帧 + auto 主题：复用这张已解码的图采样正下方画面
            // 亮度（见 sampleBackdropLuminance 的说明），保持旧版语义。
            if (sampleNeighbor) sampled = sampleBackdropLuminance(incoming);
          } else {
            frame = null;
            clearBackdrop();
          }
          // Neighbor 帧优先用 Rust 算好的值；Aligned 帧（luminance=null）
          // 落到上面的采样值，两条通路互补，auto 主题在任何采集模式下
          // 都有输入。
          luminance = typeof captured.luminance === "number" ? captured.luminance : sampled;
          knownRevision = captured.revision;
        }
        if (themeModeRef.current === "auto" && luminance != null) feedLuminance(luminance);
      } catch {
        // 捕获和解码失败时保留当前底板。
      } finally {
        capturing = false;
        const delay = refreshOnFinish ? 0 : Math.max(0, interval() - (performance.now() - start));
        refreshOnFinish = false;
        schedule(delay);
      }
    };

    const refreshBackdrop = () => {
      if (!alive || !canCapture()) return;
      if (capturing) {
        refreshOnFinish = true;
        return;
      }
      window.clearTimeout(timer);
      timer = undefined;
      schedule(0);
    };
    refreshBackdropRef.current = refreshBackdrop;

    const onVisible = () => {
      if (!canCapture()) return;
      uploadFrame();
      refreshBackdrop();
    };
    document.addEventListener("visibilitychange", onVisible);

    schedule(0);
    return () => {
      alive = false;
      window.clearTimeout(timer);
      refreshBackdropRef.current = null;
      document.removeEventListener("visibilitychange", onVisible);
      canvas.removeEventListener("webglcontextlost", onLost);
      canvas.removeEventListener("webglcontextrestored", onRestored);
      glassBackdropRef.current?.dispose();
      glassBackdropRef.current = null;
      clearBackdrop();
    };
  }, [backdropReady, enabled, feedLuminance, nativeGlassActive, sampleNeighbor, wantsRefraction]);

  // ===== 外部事件 =====
  useEffect(() => {
    const unsubs = [
      onEvent<HudBarUsage>("zbar://hud-bar-usage", setUsage),
      onEvent<TodayUsageTotals>("zbar://island-today-usage", setTodayUsage),
      onEvent<WeatherState>("zbar://island-weather", setWeather),
      onEvent<StocksState>("zbar://island-stocks", setStocks),
      onEvent<MediaState>("zbar://island-media", handleMedia),
      onEvent<SystemState>("zbar://island-system", handleSystem),
      onEvent<ClipItem[]>("zbar://island-clipboard", setClipboard),
      onEvent<NotifyState>("zbar://island-notify", handleNotify),
      // 实时字幕：状态快照 + 单条字幕（草稿覆盖同 id）
      onLiveStatus((next) => {
        if (!next || typeof next !== "object") return;
        setLiveStatus(next);
      }),
      onLiveCaption((caption) => applyLiveCaption(caption)),
      onEvent<IslandConfig>("zbar://island-config", (next) => {
        // island_set_last_tab / island_set_passthrough / 设置窗写回成功后 Rust
        // 都会广播完整配置。这里只 merge 到本地状态，绝不调用 updateConfig
        // （否则形成保存回环）；等值事件直接跳过，避免反复渲染。
        // P1-3：广播到达时可能还有本地未确认的改动，必须把它们叠回广播值，
        // 否则「后发先至」的旧广播会把刚滚出来的 compactView 打回旧值、触发
        // viewOverride 清空造成视图回跳闪烁；写前重读见 updateConfig.commit。
        if (!next || next.enabled === undefined) return;
        setCaptureExcluded(next.excludeFromCapture === true);
        const merged = { ...next, ...configPendingRef.current };
        const current = configRef.current;
        if (current && sameIslandConfig(current, merged)) return;
        setConfig(merged);
        configRef.current = merged;
        if (merged.edge && merged.edge !== current?.edge) setEdge(merged.edge);
        if (!IS_TAURI && merged.notifyModules?.join() !== current?.notifyModules?.join()) {
          void getNotify().then(applyNotifyState).catch(() => {});
        }
      }),
      onEvent<{ enabled?: boolean; expanded?: boolean }>("zbar://island-state", (payload) => {
        if (typeof payload?.enabled === "boolean") setEnabled(payload.enabled);
        if (typeof payload?.expanded === "boolean") {
          backendExpandedRef.current = payload.expanded;
          openRef.current = payload.expanded;
          setOpen(payload.expanded);
        }
      }),
      onEvent<StockAlert>("zbar://island-stock-alert", (alert) => {
        // 收起态有 activity 视图承载异动提示；只有展开态（看不见胶囊）才用 toast。
        if (alert?.name && openRef.current) showToast(`${alert.name} ${alert.text}`);
        // 收起态「股票异动」提醒：红涨绿跌，5s（参考 app.py 的 on_stock_alert）
        if (alert?.name) {
          showActivity(
            {
              kind: "stock",
              title: alert.name,
              sub: alert.text,
              icon: "chart",
              tone: alert.pct >= 0 ? "up" : "down",
            },
            5000
          );
        }
      }),
      onEvent("zbar://island-hover-expand", () => {
        if (canHoverExpand()) expand();
      }),
      onEvent("zbar://island-expand", () => {
        // 剪贴板热键（Rust 按下时已把 lastTab 置为 clip 并广播配置，随后广播
        // 本事件）：配置里的 lastTab 与当前页不同就切过去并回到顶部。
        // 回调闭包不随 config 重新绑定，必须读 ref；只认岛内合法标签。
        const wanted = normalizeTab(configRef.current?.lastTab);
        if (wanted && wanted !== tabRef.current) {
          tabRef.current = wanted;
          setTab(wanted);
          // 等 React 把新页面挂上去再滚，否则滚的是旧页面。
          window.requestAnimationFrame(() => {
            contentRef.current?.scrollTo({ top: 0 });
          });
        }
        // expandMode→always 时 Rust 只广播事件、不自己改 LAST_EXPANDED，所以
        // 即使面板已经展开也要补一次 island_set_expanded(true) 收敛后端状态。
        if (openRef.current) {
          void setIslandExpanded(true).catch(() => {});
          return;
        }
        expand();
      }),
      onEvent<{ x?: number; y?: number }>("zbar://island-passthrough-menu", (payload) =>
        passthroughMenuHandlerRef.current(payload)
      ),
      onEvent<{ delta?: number }>("zbar://island-wheel", (payload) =>
        wheelEventHandlerRef.current(payload)
      ),
      onEvent("zbar://island-collapse", () => {
        setOpen(false);
        openRef.current = false;
        closeMenuRef.current();
        void setKeyboard(false);
        // 窗口也要真的缩回悬浮条：截图恢复、全屏隐藏等路径只会发事件，
        // 不缩窗口的话会停在展开尺寸，且透明区域仍然算 :hover。
        if (shrinkTimer.current != null) window.clearTimeout(shrinkTimer.current);
        shrinkTimer.current = window.setTimeout(() => {
          void setIslandExpanded(false).catch(() => {});
        }, 30);
      }),
    ];
    return () => unsubs.forEach((fn) => fn());
  }, []);

  // 把 `.island`（本体）的实际矩形报给 Rust：收起态是胶囊外轮廓（macOS 悬停
  // 展开热区 + Windows 可穿透钩子命中判定），展开态是面板主体（macOS 悬停
  // 守护据此判「离开面板即收起」——若展开态不报，Rust 只能拿整扇 780×632
  // 画布判定，鼠标必须移出整个画布才收缩，表现为面板外画布留白区迟迟不收）。
  // Windows 穿透钩子的展开态命中走 Rust 侧 intercept_rect 的整窗分支，不受
  // 这份上报影响。
  //
  // 关键：`width/height` 有 440–500ms 过渡，收起/展开动画期间量到的是中间值；
  // 另外 clock/pet/music/activity 切换或未读角标变化都会改胶囊宽高。这里用
  // ResizeObserver 监听 `.island`，每次尺寸变化后等矩形连续 2 帧不动再上报，
  // 既避免动画中间值，也保证 P1-2：视图/角标尺寸变化后热区同步更新。
  // （open 切换会重建 observer 并重新等稳定帧，动画收敛后自然换报新矩形；
  // 期间 Rust 沿用上一次的矩形，clamp 后最坏整窗，与既有收起行为一致。）
  useEffect(() => {
    // 预览也跑：mock 命令是 no-op，但能用日志/尺寸验证 ResizeObserver 重报逻辑；
    // 真机上这条上报决定 macOS 悬停热区与 Windows 穿透钩子命中区域。
    const element = islandRef.current;
    if (!element) return;
    let alive = true;
    let raf = 0;
    let last = "";
    let stable = 0;
    const push = () => {
      const rect = element.getBoundingClientRect();
      if (rect.width < 1 || rect.height < 1) return;
      // P4：视觉外轮廓比 body 沿吸附边多 26px（两侧各 13px 内凹液脚），
      // Windows 钩子 / macOS 悬停热区按外轮廓上报。
      const horizontal = element.closest(".island-root")?.getAttribute("data-edge") === "top" ||
        element.closest(".island-root")?.getAttribute("data-edge") === "bottom";
      const padAlong = 13;
      const x = horizontal ? rect.left - padAlong : rect.left;
      const y = horizontal ? rect.top : rect.top - padAlong;
      const width = horizontal ? rect.width + padAlong * 2 : rect.width;
      const height = horizontal ? rect.height : rect.height + padAlong * 2;
      const key = `${x.toFixed(1)},${y.toFixed(1)},${width.toFixed(1)},${height.toFixed(1)}`;
      if (key === hoverRectRef.current) return;
      hoverRectRef.current = key;
      void islandSetHoverRect(x, y, width, height);
    };
    const tick = () => {
      raf = 0;
      if (!alive) return;
      const rect = element.getBoundingClientRect();
      if (rect.width >= 1 && rect.height >= 1) {
        const key = `${rect.left.toFixed(1)},${rect.top.toFixed(1)},${rect.width.toFixed(1)},${rect.height.toFixed(1)}`;
        if (key === last) {
          stable += 1;
        } else {
          last = key;
          stable = 0;
        }
        if (stable >= 2) {
          push();
          return;
        }
      }
      raf = window.requestAnimationFrame(tick);
    };
    const schedule = () => {
      last = "";
      stable = 0;
      if (!raf) raf = window.requestAnimationFrame(tick);
    };
    const observer = new ResizeObserver(schedule);
    observer.observe(element);
    schedule();
    return () => {
      alive = false;
      if (raf) window.cancelAnimationFrame(raf);
      observer.disconnect();
    };
  }, [open, edge, Boolean(config)]);

  // 失去系统焦点（点其它应用）后恢复「不可聚焦」，下次启动/点击不再抢焦点
  useEffect(() => {
    const onBlur = () => {
      if (!IS_TAURI) return;
      window.setTimeout(() => {
        void setKeyboard(false);
        if (
          configRef.current?.expandMode === "hover" &&
          !rootRef.current?.matches(":hover")
        ) {
          collapse();
        }
      }, 80);
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // ===== 面板子树延时卸载（内存压降专项 3a）=====
  // 进入收起态后启动 10 分钟倒计时，到期卸载 island-panel 子树省内存；
  // 展开立即清表（expand() 里还有同步预挂，二者叠加保证展开时已挂载）。
  useEffect(() => {
    if (open) return;
    const timer = window.setTimeout(() => setPanelMounted(false), PANEL_UNMOUNT_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [open]);

  // ===== lazy 页 chunk 空闲期预取（内存压降专项 5）=====
  // ShotPage / ToolboxPage 拆独立 chunk 后，首次点击对应 tab 才加载会有
  // 一帧 Suspense 占位；启动后空闲期预取（只下载执行模块顶层，不渲染
  // 组件）让首次点击零等待。
  useEffect(() => {
    const warm = () => {
      void import("./ShotPage");
      void import("./ToolboxPage");
    };
    if (typeof window.requestIdleCallback === "function") {
      const id = window.requestIdleCallback(warm, { timeout: 5000 });
      return () => window.cancelIdleCallback(id);
    }
    const t = window.setTimeout(warm, 1500);
    return () => window.clearTimeout(t);
  }, []);

  // ===== 会议录音指示（收起胶囊）=====
  // 只在收起态轮询：展开时工具页自己的 meet-tab-live 红点与提示行已经承担
  // 感知职责，本层再轮询就是重复请求；收起后胶囊是用户唯一能看到的入口，
  // 指示只能由这里提供。2.5s 一拍足够——胶囊上的计时按轮询粒度跳秒，
  // 不需要每秒刷新，也不值得为一个红点常驻更高频的 IPC。
  // 会议功能唯一入口在「工具」页内（ToolboxPage 的会议子页），配置把 tool
  // tab 隐藏后胶囊指示无从进入会议页，轮询就是纯空转——跳过（内存压降
  // 专项 3c；IslandConfig 无独立"会议启用"开关，hiddenTabs 是唯一可达性
  // 字段）。config 未就绪时保持轮询（与原行为一致，boot 期只有几百 ms）。
  const meetingEntryAvailable = config
    ? !config.hiddenTabs.includes("tool")
    : true;
  useEffect(() => {
    if (open || !meetingEntryAvailable) return;
    let cancelled = false;
    const check = () => {
      meetingState()
        .then((brief) => {
          if (!cancelled) setMeetingBrief(brief);
        })
        .catch(() => {
          /* 状态查询失败只影响红点显示，不打扰用户 */
        });
    };
    check();
    const timer = window.setInterval(check, 2500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [open, meetingEntryAvailable]);

  // ===== 歌词：换曲后按「曲名 + 歌手（+ 专辑 / 时长）」取一次，失败静默 =====
  // media.lyrics 关掉时一次都不请求（省掉网易云 / lrclib 的网络往返）。
  useEffect(() => {
    if (configRef.current?.mediaLyrics === false) {
      setLyrics(null);
      return;
    }
    if (!media.hasSession || !media.title.trim()) {
      setLyrics(null);
      return;
    }
    setLyrics(null);
    let alive = true;
    const timer = window.setTimeout(() => {
      // 专辑 / 时长只作为命中辅助，用 ref 读最新值：它们不算换曲信号，
      // 放进依赖会让同一首歌在进度刷新时反复请求。
      const latest = mediaRef.current;
      getLyrics(media.title, media.artist, latest?.album, latest?.duration)
        .then((value) => {
          if (alive) setLyrics(value);
        })
        .catch(() => {
          if (alive) setLyrics(null);
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [media.hasSession, media.title, media.artist, config?.mediaLyrics]);

  // ===== 主题跟随配置 =====
  // 首次加载、外部（设置窗 / 主窗口 / 菜单命令）广播 zbar://island-config /
  // 主进程回写都会走到这里；缺字段（旧配置）按 dark 兜底。
  useEffect(() => {
    if (!config) return;
    setThemeMode(normalizeThemeMode(config.theme));
  }, [config?.theme]);

  // ===== 字体：自定义 family 写到文档根节点的 CSS 变量 =====
  // island.css 的 body 用 `var(--island-font-family, 内置字体栈)`；变量挂在
  // <html> 上 body 才继承得到（挂 .island-root 只影响岛内部）。自定义 family
  // 用安全引号并保留完整回退栈，避免含空格的字体名被拆成多个 family，或中文
  // 缺字时失去系统回退。空串 / 未设置移除变量，回退内置字体栈。
  useEffect(() => {
    const family = (config?.fontFamily ?? "").trim();
    const root = document.documentElement;
    if (family) {
      root.style.setProperty("--island-font-family", fontFamilyWithFallback(family, ISLAND_FONT_STACK));
    } else {
      root.style.removeProperty("--island-font-family");
    }
    return () => {
      root.style.removeProperty("--island-font-family");
    };
  }, [config?.fontFamily]);

  // ===== 首次展开策略（预览参数 / lastTab / defaultTab）=====
  // 预览参数 ?tab= 只接受岛内标签（TabKey）；设置已迁到独立窗口，没有
  // "settings" 这个岛内页，非法值一律回落到主页。
  useEffect(() => {
    if (!config || bootRef.current) return;
    bootRef.current = true;
    // 上次停留的标签优先（Rust 的 island_set_last_tab 落盘），其次是配置里的
    // defaultTab（缺省 home）。两者都必须能在当前可见标签里找到。
    const remembered = normalizeTab(config.lastTab) ?? normalizeTab(config.defaultTab) ?? "home";
    const initialTab =
      previewTab && TABS.some((t) => t.key === previewTab) ? previewTab : remembered;
    bootTabRef.current = initialTab;
    setTab(initialTab);
    if (!enabled) return;
    // mounted / WebView 重载：后端 LAST_EXPANDED 为 true 时直接恢复展开态，
    // 不只是预览参数和 always 配置（P2-12）。
    if (previewOpen || backendExpandedRef.current) {
      // 预览 / 重载恢复都不经过 expand()：同样先预挂面板子树（对称 expand 入口）
      setPanelMounted(true);
      setOpen(true);
      openRef.current = true;
      void setIslandExpanded(true).catch(() => {});
      return;
    }
    // 常驻展开：配置生效即展开，不等悬停。160ms 让首帧的窗口尺寸/样式先落定。
    if (normalizeExpandMode(config.expandMode) === "always") {
      if (bootExpandTimer.current != null) window.clearTimeout(bootExpandTimer.current);
      bootExpandTimer.current = window.setTimeout(() => {
        bootExpandTimer.current = null;
        if (configRef.current && normalizeExpandMode(configRef.current.expandMode) === "always") {
          expand();
        }
      }, 160);
    }
  }, [config, enabled, previewOpen, previewTab]);

  // ===== 展开 / 收起（expandMode 三态 + 可配置延迟）=====
  // hover 悬停停留 hoverExpandMs 后展开、移出 collapseDelayMs 后
  // 收起；always 常驻展开、移出不收；never（长关）由双击退出。
  const expandModeOf = () => normalizeExpandMode(configRef.current?.expandMode);
  const hoverExpandMsOf = () => clampNumber(configRef.current?.hoverExpandMs, 0, 4000, 120);
  const collapseDelayMsOf = () => clampNumber(configRef.current?.collapseDelayMs, 0, 4000, 320);

  const cancelClick = () => {
    lastPillClick.current = null;
    if (clickTimer.current != null) {
      window.clearTimeout(clickTimer.current);
      clickTimer.current = null;
    }
  };

  const cancelHover = () => {
    if (hoverTimer.current != null) {
      window.clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
  };

  const cancelCollapse = () => {
    if (collapseTimer.current != null) {
      window.clearTimeout(collapseTimer.current);
      collapseTimer.current = null;
    }
    if (shrinkTimer.current != null) {
      window.clearTimeout(shrinkTimer.current);
      shrinkTimer.current = null;
    }
  };

  const showExpanded = () => {
    setOpen(true);
  };

  const expand = (immediate = false) => {
    // 面板子树预挂（内存压降专项 3a）：收起 10 分钟后面板被卸载，展开入口
    // 先同步挂回，距 showExpanded() 还有 setIslandExpanded → 视口等待 →
    // 双 rAF 的帧余量，面板已在 DOM，展开 CSS 过渡与常驻 DOM 时代完全一致
    setPanelMounted(true);
    cancelHover();
    cancelCollapse();
    if (shrinkTimer.current != null) {
      window.clearTimeout(shrinkTimer.current);
      shrinkTimer.current = null;
    }
    if (openRef.current) return;
    openRef.current = true;
    resetPokeRef.current();
    closeMenuRef.current();
    // 窗口已常驻 672×520（面板 640 + 投影留白），开合只剩 CSS 尺寸动画；
    // 仍等视口达到面板尺寸再开始，浏览器预览 / 极端缩放下也不会长到一半。
    if (IS_TAURI && !immediate) {
      void setIslandExpanded(true)
        .catch(() => {})
        .then(() => {
          const started = performance.now();
          const waitViewport = () => {
            if (!openRef.current) return;
            const settled =
              (window.innerWidth >= PANEL_W && window.innerHeight >= 500) ||
              performance.now() - started > 360;
            if (!settled) {
              window.requestAnimationFrame(waitViewport);
              return;
            }
            window.requestAnimationFrame(() => {
              window.requestAnimationFrame(() => {
                if (openRef.current) showExpanded();
              });
            });
          };
          window.requestAnimationFrame(waitViewport);
        });
    } else {
      void setIslandExpanded(true).catch(() => {});
      showExpanded();
    }
  };

  const closeNow = () => {
    cancelHover();
    cancelCollapse();
    cancelClick();
    pinUntil.current = 0;
    if (!openRef.current) return;
    openRef.current = false;
    closeMenuRef.current();
    setOpen(false);
    void setKeyboard(false);
    shrinkTimer.current = window.setTimeout(() => {
      void setIslandExpanded(false).catch(() => {});
    }, 560);
  };

  /** 收起：默认延迟读 config.collapseDelayMs；常驻展开 / 指针在岛上 / pin 期都不收。 */
  const collapse = (delay = collapseDelayMsOf()) => {
    cancelHover();
    if (expandModeOf() === "always" || launcherPickerOpen.current) return;
    if (collapseTimer.current != null) {
      window.clearTimeout(collapseTimer.current);
      collapseTimer.current = null;
      if (launcherPickerOpen.current) return;
    }
    collapseTimer.current = window.setTimeout(() => {
      collapseTimer.current = null;
      const pinLeft = pinUntil.current - performance.now();
      if (pinLeft > 0) {
        // 单击展开后的 600ms 保护期：到点再判定，避免鼠标擦过边缘就缩回去。
        collapse(pinLeft + 30);
        return;
      }
      if (expandModeOf() === "always") return;
      // 指针仍停在岛上（含右键菜单）或输入框有焦点时不收。
      if (rootRef.current?.matches(":hover")) return;
      const active = document.activeElement as HTMLElement | null;
      if (
        active &&
        rootRef.current?.contains(active) &&
        (active.tagName === "INPUT" ||
          active.tagName === "TEXTAREA" ||
          active.tagName === "SELECT")
      ) {
        return;
      }
      closeNow();
    }, delay);
  };

  const canHoverExpand = () =>
    !openRef.current && !pointerRef.current && !dragActiveRef.current &&
    !blockHoverUntilLeave.current && !openMenuRef.current &&
    !configRef.current?.passthrough && configRef.current?.hoverExpand !== false &&
    expandModeOf() === "hover";

  const scheduleExpand = () => {
    if (!enabled || !canHoverExpand() || hoverTimer.current != null) return;
    hoverTimer.current = window.setTimeout(() => {
      hoverTimer.current = null;
      if (canHoverExpand() && !islandRef.current?.querySelector(".compact-play:hover") &&
          rootRef.current?.matches(":hover")) {
        expand();
      }
    }, hoverExpandMsOf());
  };

  const toggleOpen = () => {
    cancelClick();
    if (openRef.current) {
      closeNow();
      // 常驻展开时点击收起 → 切回 hover，否则之后就再没有自动展开的入口。
      if (expandModeOf() === "always") applyExpandMode("hover");
      return;
    }
    expand();
  };

  // ===== 配置保存（稍作防抖，滑杆拖动不会打爆 IPC）=====
  // P1-3（参考设置窗 useIslandConfig）：配置是整份对象，直接拿本地快照写回
  // 会覆盖 island_set_last_tab / island_set_passthrough 等并发写入的字段。
  // 因此累计 pendingPatch，真正落盘前先重读权威配置再叠加本批改动写回；
  // revision + pendingPatch 负责丢弃过期响应、保留在途新改动。
  const updateConfig = (patch: Partial<IslandConfig>, immediate = false) => {
    const current = configRef.current;
    if (!current) return;
    configPendingRef.current = { ...configPendingRef.current, ...patch };
    const next = { ...current, ...patch };
    setConfig(next);
    configRef.current = next;
    const revision = ++configRevision.current;
    if (configTimer.current != null) window.clearTimeout(configTimer.current);
    const commit = () => {
      configTimer.current = null;
      const batch = { ...configPendingRef.current };
      if (Object.keys(batch).length === 0) return;
      configSaveQueue.current = configSaveQueue.current
        .catch(() => {
          /* 上一次失败不能卡住后续保存 */
        })
        .then(async () => {
          try {
            const fresh = await getIslandConfig();
            const saved = await setIslandConfig({ ...fresh, ...batch });
            clearSettledConfigPatch(configPendingRef.current, batch);
            // 期间又改过配置：这是旧批次的响应，不能回写覆盖新状态；
            // 新批次自己的 commit 排在队列后面，会带来正确结果。
            if (revision === configRevision.current && saved) {
              const merged = { ...saved, ...configPendingRef.current };
              setConfig(merged);
              configRef.current = merged;
            }
          } catch (e) {
            clearSettledConfigPatch(configPendingRef.current, batch);
            // 回滚用「重拉权威 + 保留在途新改动」，而不是本地旧快照
            try {
              const fresh = await getIslandConfig();
              const rolledBack = { ...fresh, ...configPendingRef.current };
              setConfig(rolledBack);
              configRef.current = rolledBack;
            } catch {
              /* 二次读取失败：保持乐观值，下一次操作仍会尝试写入 */
            }
            showToast(`保存配置失败: ${String(e)}`);
          }
        });
    };
    if (immediate) commit();
    else configTimer.current = window.setTimeout(commit, 220);
  };

  // ===== 展开方式 / 单击 / 双击 / 滚轮 =====
  /** 应用展开方式（菜单用）：always 立即展开；never 等鼠标移出再收。 */
  const applyExpandMode = (mode: ExpandMode) => {
    updateConfig({ expandMode: mode }, true);
    cancelClick();
    pinUntil.current = 0;
    if (mode === "always") {
      expand();
      return;
    }
    if (openRef.current && !rootRef.current?.matches(":hover")) collapse();
  };

  /** 双击：hover ↔ never（长关）。常驻展开时双击也切到长关并立即收起。 */
  const toggleLongOff = () => {
    const next: ExpandMode = expandModeOf() === "never" ? "hover" : "never";
    updateConfig({ expandMode: next }, true);
    cancelClick();
    pinUntil.current = 0;
    if (next === "never" && openRef.current) closeNow();
    showToast(
      next === "never" ? "长关：双击退出长关" :
        configRef.current?.hoverExpand === false ? "已恢复单击展开" : "已恢复悬停展开"
    );
  };

  // 只用通过手势判定的短按形成双击，长按和拖拽不会影响下一次短按。
  const queuePillClick = (point?: { x: number; y: number }) => {
    const previous = lastPillClick.current;
    if (point && previous && !hasDragMovement(point.x - previous.x, point.y - previous.y, false)) {
      toggleLongOff();
      return;
    }
    cancelClick();
    lastPillClick.current = point ?? null;
    clickTimer.current = window.setTimeout(() => {
      clickTimer.current = null;
      lastPillClick.current = null;
      if (openRef.current || pointerRef.current || expandModeOf() === "never") return;
      if (snapEdgeRef.current) {
        pendingExpand.current = true;
        return;
      }
      if (dragActiveRef.current) return;
      expand();
      pinUntil.current = performance.now() + CLICK_PIN_MS;
    }, doubleClickMs.current);
  };

  const handlePillClick = (event: ReactMouseEvent<HTMLElement>) => {
    // 鼠标手势在全局 pointerup 处理。保留键盘生成的按钮点击。
    if (event.detail === 0) queuePillClick();
  };

  /** 双击面板空白处：同样切 hover ↔ never；按钮 / 输入框等交互元素不响应。 */
  const handleIslandDoubleClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement | null;
    if (!target || target.closest(".pill-wrap, .liquid-glass")) return;
    if (
      target.closest(
        "button, a, input, textarea, select, [contenteditable='true'], .ctl-range, " +
          "[role='button'], .agent-card, .island-context-menu, [data-no-dblclick]"
      )
    ) {
      return;
    }
    if (dragActiveRef.current || pointerRef.current || suppressClickRef.current) return;
    cancelClick();
    toggleLongOff();
  };

  // ===== P1：收起态 activity / 事件处理 =====
  /**
   * 弹一条收起态提醒。updateInPlace=true 时（音量连拖）沿用同一个 id，
   * 不重新播入场动画，只刷新数值和倒计时（参考 compact.update_activity）。
   */
  const showActivity = (
    patch: Omit<CompactActivity, "id" | "until">,
    duration: number,
    updateInPlace = false
  ) => {
    // 展开态不展示收起提醒：面板盖住胶囊，弹了也看不见，还会打断展开动画。
    if (openRef.current) return;
    const previous = activityRef.current;
    const id =
      updateInPlace && previous && previous.kind === patch.kind
        ? previous.id
        : ++activitySeq.current;
    const next: CompactActivity = { ...patch, id, until: Date.now() + duration };
    activityRef.current = next;
    setActivity(next);
    if (activityTimer.current != null) window.clearTimeout(activityTimer.current);
    activityTimer.current = window.setTimeout(() => {
      activityTimer.current = null;
      if (activityRef.current?.id === id) {
        activityRef.current = null;
        setActivity(null);
      }
    }, duration);
  };

  /** 通知类活动卡的字段（来源 kind → 图标 / 品牌色，与既有 showActivity 一致） */
  const notifyActivityFor = (item: NotifyItem): Omit<CompactActivity, "id" | "until"> => {
    const kind = item.kind || "other";
    return {
      kind: "notify",
      title: (item.sender || item.name || "").trim() || "新消息",
      sub: (item.body || "").trim() || "新消息",
      icon: KIND_ICONS[kind] ?? "message",
      sourceKind: kind,
      color: KIND_COLORS[kind] ?? KIND_COLORS.other,
    };
  };

  // ===== 新消息「岛端先动」：t=0 惊讶 → t=1200 贴边呼吸灯 + 活动卡 =====
  // 时序常量见 notifyPulse.ts（NOTIFY_LEAD_MS / NOTIFY_PULSE_MS / NOTIFY_ACTIVITY_MS）。
  // 事件订阅 effect 依赖是 []，这里所有可变状态一律走 ref 读最新值。

  /** 关掉呼吸灯：停 rAF、透明度归零（展开 / 放弃编排 / 新波次重置时用） */
  const stopNotifyPulse = () => {
    const state = notifyPulseState.current;
    if (state && state.raf) window.cancelAnimationFrame(state.raf);
    notifyPulseState.current = null;
    if (notifyPulseRef.current) notifyPulseRef.current.style.opacity = "0";
  };

  /** 把当前波次的颜色写到呼吸灯元素（同一波次后续消息换成最新来源色） */
  const applyNotifyPulseColor = () => {
    notifyPulseRef.current?.style.setProperty(
      "--notify-pulse-color",
      notifyPulseColorRef.current
    );
  };

  /** 同一波次续期：只推结束时间，startAt 不动 → 呼吸相位连续，不会重新淡入 */
  const extendNotifyPulse = (wave: NotifyWave) => {
    const state = notifyPulseState.current;
    if (!state) return;
    const { endAt } = notifyPulseWindow(wave);
    if (endAt > state.endAt) state.endAt = endAt;
  };

  /** 点亮贴边呼吸灯：rAF 每帧只写 opacity（亮度曲线见 notifyPulseIntensity） */
  const startNotifyPulse = (wave: NotifyWave) => {
    const element = notifyPulseRef.current;
    if (!element) return;
    stopNotifyPulse();
    applyNotifyPulseColor();
    const { startAt, endAt } = notifyPulseWindow(wave);
    const state = { startAt, endAt, raf: 0 };
    notifyPulseState.current = state;
    const tick = () => {
      // 续期只改 state.endAt；新波次会换掉整个 state，旧循环自然退出。
      if (notifyPulseState.current !== state) return;
      const now = performance.now();
      if (now >= state.endAt) {
        element.style.opacity = "0";
        notifyPulseState.current = null;
        return;
      }
      const intensity = notifyPulseIntensity(now - state.startAt, state.endAt - state.startAt);
      element.style.opacity = intensity > 0 ? intensity.toFixed(3) : "0";
      state.raf = window.requestAnimationFrame(tick);
    };
    tick();
  };

  /**
   * 释放惊讶用的临时 pet 覆盖；返回释放后生效的运行时覆盖（null = 跟配置走）。
   * 同一 tick 里的滚轮要按「用户原视图」继续翻页，所以恢复值同步写回 ref。
   */
  const releaseNotifyPet = (): CompactView | null => {
    const state = notifyPetRef.current;
    if (!state.active) return viewOverrideRef.current;
    state.active = false;
    if (notifyPetRaf.current != null) {
      window.cancelAnimationFrame(notifyPetRaf.current);
      notifyPetRaf.current = null;
    }
    if (viewOverrideRef.current !== "pet") return viewOverrideRef.current;
    const configView = normalizeCompactView(configRef.current?.compactView);
    const restored = state.original === configView ? null : state.original;
    viewOverrideRef.current = restored;
    setViewOverride(restored);
    return restored;
  };

  /** t=0：惊讶。当前视图不是 pet 时先临时覆盖成 pet，等 PetFace 挂载后再 +1 才会播 */
  const stageNotifySurprise = () => {
    // 上一波覆盖还没释放（主线程卡顿让 1.2s 回调排到了下一波 start 之后）：
    // 沿用第一次记下的原视图；否则 original 会被写成 "pet"，释放后永久停在 pet。
    if (!notifyPetRef.current.active) {
      notifyPetRef.current.original = viewOverrideRef.current;
      notifyPetRef.current.active = true;
    }
    // 上一波「等 PetFace 挂载」的 rAF 还没跑：先取消，避免这次再 +1 造成双份惊讶。
    if (notifyPetRaf.current != null) {
      window.cancelAnimationFrame(notifyPetRaf.current);
      notifyPetRaf.current = null;
    }
    const current =
      viewOverrideRef.current ?? normalizeCompactView(configRef.current?.compactView);
    if (current === "pet") {
      setSurpriseToken((value) => value + 1);
      return;
    }
    setViewOverride("pet");
    // PetFace 的 surpriseTokenRef 在挂载时按当时的 prop 初始化，同帧 +1 会被当成初值
    // 忽略；等视图换过去（下一帧）再 +1，惊讶才会真正播放。
    notifyPetRaf.current = window.requestAnimationFrame(() => {
      notifyPetRaf.current = null;
      setSurpriseToken((value) => value + 1);
    });
  };

  /** t=1200：收起 pet 覆盖，弹活动卡（波次里最新一条），点亮呼吸灯 */
  const scheduleNotifyStage = () => {
    if (notifyStageTimer.current != null) window.clearTimeout(notifyStageTimer.current);
    notifyStageTimer.current = window.setTimeout(() => {
      notifyStageTimer.current = null;
      const wave = notifyWaveRef.current;
      if (!wave) return;
      releaseNotifyPet();
      if (openRef.current) {
        // 这 1.2s 里用户把面板拉开了：胶囊被面板盖住，整段编排放弃。
        notifyWaveRef.current = null;
        stopNotifyPulse();
        return;
      }
      showActivity(notifyActivityFor(wave.latest), NOTIFY_ACTIVITY_MS);
      startNotifyPulse(wave);
    }, NOTIFY_LEAD_MS);
  };

  /** 放弃本轮编排（展开面板 / 更高优先级 activity 抢走胶囊时） */
  const cancelNotifyChain = () => {
    if (notifyStageTimer.current != null) {
      window.clearTimeout(notifyStageTimer.current);
      notifyStageTimer.current = null;
    }
    notifyWaveRef.current = null;
    releaseNotifyPet();
    stopNotifyPulse();
  };

  /**
   * 新消息的岛端反应链（收起态）：
   *   t=0     惊讶（必要时把胶囊临时切到 pet 1.2s）
   *   t=1200  贴边呼吸灯 5s + 活动卡 4s
   * 同一波次的后续消息只续期：不重播惊讶、不重排活动卡、呼吸灯相位连续。
   * 展开态 / 已有 activity 时维持既有语义（只刷新角标，或直接覆盖活动卡）。
   */
  const reactToNewMessages = (fresh: NotifyItem[]) => {
    const newest = fresh[0];
    if (!newest) return;

    const decision = decideNotifyReaction({
      open: openRef.current,
      activityKind: activityRef.current?.kind ?? null,
      wave: notifyWaveRef.current,
      item: newest,
      now: performance.now(),
    });
    // 展开态：角标已由 notify.items 派生，不做收起编排。
    if (decision.mode === "ignore") return;

    if (decision.mode === "replace") {
      // 音量 / 股票等更高优先级 activity（或没有本波次记录的通知卡）正占用胶囊：
      // 沿用既有覆盖语义，不插惊讶与呼吸灯（惊讶会被 activity 视图盖住）。
      cancelNotifyChain();
      showActivity(notifyActivityFor(newest), NOTIFY_ACTIVITY_MS);
      return;
    }

    const wave = decision.wave;
    if (!wave) return;
    notifyWaveRef.current = wave;
    notifyPulseColorRef.current = notifyPulseColor(newest.kind, KIND_COLORS, "var(--accent)");

    if (decision.mode === "continue") {
      // 同一波次：不重播惊讶、不重排活动卡；呼吸灯换最新来源色并续期（相位连续）。
      if (notifyPulseState.current) applyNotifyPulseColor();
      extendNotifyPulse(wave);
      // 通知卡还在屏上就地换内容（沿用同一个 id，不重播入场动画）。
      if (activityRef.current?.kind === "notify") {
        showActivity(notifyActivityFor(newest), NOTIFY_ACTIVITY_MS, true);
      }
      return;
    }

    // 新波次：t=0 惊讶，t=NOTIFY_LEAD_MS 呼吸灯 + 活动卡。
    stageNotifySurprise();
    scheduleNotifyStage();
  };

  /** 新通知：角标照旧由 notify.items 派生；收起态再走一遍「岛端先动」编排 */
  const handleNotify = (next: NotifyState) => {
    const fresh = notifyBaselineReady.current
      ? next.items.filter((item) => !item.read && !seenNotifyIds.current.has(item.id))
      : [];
    applyNotifyState(next);
    reactToNewMessages(fresh);
  };

  /** 音量 / 静音变化：1.8s 提醒；初始值和展开态不弹（参考 app.py on_volume） */
  const handleSystem = (next: SystemState) => {
    const previous = systemRef.current;
    systemRef.current = next;
    setSystem(next);
    if (!previous || openRef.current) return;
    const volumeChanged = Math.abs((previous.volume ?? 0) - (next.volume ?? 0)) > 0.004;
    const mutedChanged = previous.muted !== next.muted;
    if (!volumeChanged && !mutedChanged) return;
    showActivity({ kind: "volume", value: next.volume, muted: next.muted }, 1800, true);
  };

  /** 媒体：开始播放（或换曲）自动切 music 并记住原视图；暂停 8s 后回原视图。
   *  `media.auto_show` 关掉时不再自动切视图（手动滚轮切换不受影响）。 */
  const handleMedia = (next: MediaState, initial = false) => {
    const previous = mediaRef.current;
    mediaRef.current = next;
    setMedia(next);
    const currentConfig = configRef.current;
    if (!currentConfig) return;
    const autoShow = currentConfig.mediaAutoShow !== false;
    const playing = Boolean(next.hasSession && next.playing && next.title);
    const wasPlaying = Boolean(previous.hasSession && previous.playing && previous.title);
    const trackChanged = next.title !== previous.title || next.artist !== previous.artist;
    const currentView =
      viewOverrideRef.current ?? normalizeCompactView(currentConfig.compactView);
    if (playing && (initial || !wasPlaying || trackChanged)) {
      if (musicPauseTimer.current != null) {
        window.clearTimeout(musicPauseTimer.current);
        musicPauseTimer.current = null;
      }
      // 自动切页关掉时不抢占用户当前视图；因为没切过，暂停时也不会回退。
      if (autoShow && currentView !== "music") {
        musicReturnView.current = currentView;
        setViewOverride("music");
      }
    } else if (!playing && wasPlaying) {
      const back = musicReturnView.current;
      if (back) {
        if (musicPauseTimer.current != null) window.clearTimeout(musicPauseTimer.current);
        musicPauseTimer.current = window.setTimeout(() => {
          musicPauseTimer.current = null;
          const latest = mediaRef.current;
          if (latest.hasSession && latest.playing && latest.title) return;
          // 期间用户已滚轮切到别的视图：尊重用户选择，不再抢回原视图。
          const currentViewNow =
            viewOverrideRef.current ?? normalizeCompactView(configRef.current?.compactView);
          if (currentViewNow !== "music") {
            musicReturnView.current = null;
            return;
          }
          setViewOverride(
            back === normalizeCompactView(configRef.current?.compactView) ? null : back
          );
          musicReturnView.current = null;
        }, 8000);
      }
    }
  };

  /** 收起态胶囊滚轮：activity 期间不切；顺序 rings→clock→pet→music→rings。 */
  const cycleCompactView = (direction: number) => {
    // 用户主动滚轮：立刻放弃「新消息临时切 pet」，并从释放后的视图继续翻页。
    // pet 覆盖的恢复是排队到下一次 render 的，不能直接读 viewOverrideRef，
    // 否则惊讶期间滚轮会从 "pet" 算下一页，还把 pet 写进配置。
    const baseView = releaseNotifyPet();
    if (activityRef.current) return; // activity 优先级最高，等它跑完
    // P1-4：250ms 节流，避免高精度滚轮/钩子高频事件连翻好几个视图
    const nowMs = Date.now();
    if (nowMs - compactWheelAt.current < 250) return;
    compactWheelAt.current = nowMs;
    // 用户手动切视图后，音乐暂停的 8s 自动回退不应再把视图抢走（P2-9）
    if (musicPauseTimer.current != null) {
      window.clearTimeout(musicPauseTimer.current);
      musicPauseTimer.current = null;
    }
    const current = baseView ?? normalizeCompactView(configRef.current?.compactView);
    const index = COMPACT_VIEWS.indexOf(current);
    const next =
      COMPACT_VIEWS[(index + direction + COMPACT_VIEWS.length) % COMPACT_VIEWS.length];
    musicReturnView.current = null;
    setViewOverride(next);
    updateConfig({ compactView: next }, true);
    setInteractSeq((value) => value + 1);
  };
  cycleCompactRef.current = cycleCompactView;

  /** 导航栏滚轮：在可见标签间上一页 / 下一页（到边界停住）。 */
  const cycleTab = (direction: number) => {
    const list = TABS.filter((item) => item.key === "home" || !hidden.has(item.key));
    const index = list.findIndex((item) => item.key === activeTab);
    if (index < 0) return;
    const target = list[Math.max(0, Math.min(list.length - 1, index + direction))];
    if (target && target.key !== activeTab) setTab(target.key);
  };
  cycleTabRef.current = cycleTab;

  // ===== P1：页面可见性（pet rAF / 律动条暂停）=====
  useEffect(() => {
    const onVisibility = () => setPageVisible(!document.hidden);
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, []);

  // 展开面板：胶囊被面板接管，立刻放弃「新消息临时切 pet」与呼吸灯，不等 1.2s。
  useEffect(() => {
    if (!open) return;
    cancelNotifyChain();
  }, [open]);

  // 已加载的设置发生变化才清掉覆盖，保留启动时已在播放的音乐视图。
  const configuredCompactView = config ? normalizeCompactView(config.compactView) : null;
  const previousConfiguredCompactView = useRef<CompactView | null>(null);
  useEffect(() => {
    const previous = previousConfiguredCompactView.current;
    previousConfiguredCompactView.current = configuredCompactView;
    if (previous !== null && previous !== configuredCompactView) setViewOverride(null);
  }, [configuredCompactView]);

  // ===== P1：自动轮换（只 clock ↔ pet；交互后重新计时）=====
  useEffect(() => {
    if (!config || open || !enabled || activity) return;
    const seconds = clampNumber(config.autoCycleSec, 0, 600, 30);
    if (!seconds) return;
    const view = viewOverride ?? normalizeCompactView(config.compactView);
    if (view !== "clock" && view !== "pet") return;
    const timer = window.setTimeout(() => {
      const current =
        viewOverrideRef.current ?? normalizeCompactView(configRef.current?.compactView);
      if (openRef.current || activityRef.current) return;
      if (current === "clock") setViewOverride("pet");
      else if (current === "pet") setViewOverride("clock");
      setInteractSeq((value) => value + 1);
    }, seconds * 1000);
    return () => window.clearTimeout(timer);
  }, [config, open, enabled, activity, viewOverride, interactSeq]);

  // ===== P2：可穿透右键菜单的生命周期 =====
  // Esc：关闭菜单（唯一出口 closeMenuRef 会负责 island_passthrough_restore）。
  useEffect(() => {
    if (!contextMenu) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeMenuRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [contextMenu]);

  // ===== P2：右键菜单期间冻结「透明留白自动穿透」=====
  // 收起态菜单是浮在胶囊热区之外的绝对定位浮层。不冻结的话，光标一移到菜单上
  // 就被判在热区外，Rust 的 16ms 轮询立刻把窗口设成 WS_EX_TRANSPARENT，菜单
  // 收不到 hover/click（这正是不动点不动的根因）。依赖 contextMenu 覆盖所有
  // 关闭路径——菜单项动作、backdrop、Esc、收起事件、passthrough 兜底计时器
  // 都走 closeContextMenu 把 state 置空；卸载时补一次 false，避免窗口重载后
  // 一直按「菜单打开」处理。
  const contextMenuOpen = contextMenu !== null;
  useEffect(() => {
    if (!contextMenuOpen) return;
    void islandSetContextMenuOpen(true);
    return () => {
      void islandSetContextMenuOpen(false);
    };
  }, [contextMenuOpen]);

  // 组件卸载：如果菜单还开着（可穿透 grace 未释放），补一次 restore（幂等）；
  // 同时清掉所有未卸载的交互计时器，避免 WebView 重载后旧计时器继续跑。
  useEffect(
    () => () => {
      dragEpochRef.current += 1;
      if (passthroughMenuOpen.current) {
        passthroughMenuOpen.current = false;
        if (passthroughMenuTimer.current != null) {
          window.clearTimeout(passthroughMenuTimer.current);
          passthroughMenuTimer.current = null;
        }
        void islandPassthroughRestore().catch(() => {});
      }
      const timers = [
        clickTimer,
        hoverTimer,
        collapseTimer,
        shrinkTimer,
        activityTimer,
        musicPauseTimer,
        configTimer,
        toastTimer,
        bootExpandTimer,
        notifyStageTimer,
      ];
      for (const timer of timers) {
        if (timer.current != null) {
          window.clearTimeout(timer.current);
          timer.current = null;
        }
      }
      // 新消息编排的 rAF（临时 pet 覆盖 / 呼吸灯）也要停，WebView 重载后不留旧循环。
      if (notifyPetRaf.current != null) {
        window.cancelAnimationFrame(notifyPetRaf.current);
        notifyPetRaf.current = null;
      }
      const pulse = notifyPulseState.current;
      if (pulse && pulse.raf) window.cancelAnimationFrame(pulse.raf);
      notifyPulseState.current = null;
    },
    []
  );

  // pet / 收起动画用的指针采样：直接写 ref，不参与 React 渲染。
  const trackCompactPointer = (clientX: number, clientY: number) => {
    const rect = islandRef.current?.getBoundingClientRect();
    if (!rect) return;
    compactPointerRef.current = {
      x: clientX - (rect.left + rect.width / 2),
      y: clientY - (rect.top + rect.height / 2),
      inside: true,
    };
  };

  // 原生 wheel 监听（React 的 onWheel 在部分 WebView 上是 passive，拦不住滚动）：
  // - 收起态：整个胶囊上滚轮切 compactView
  // - 展开态：只有导航栏上滚轮切页，页面内容（如主页额度分页）保持原有滚轮语义
  useEffect(() => {
    const element = rootRef.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (event.ctrlKey || event.metaKey || dragActiveRef.current || event.deltaY === 0) return;
      const target = event.target as Element | null;
      if (!target?.closest(".island, .liquid-glass")) return;
      const direction = event.deltaY > 0 ? 1 : -1;
      if (!openRef.current) {
        const quotaList = target.closest<HTMLElement>(".pill-quota .pill-agents, .compact-quotas");
        if (quotaList) {
          if (quotaList.scrollHeight > quotaList.clientHeight + 1) return;
          if (quotaList.scrollWidth > quotaList.clientWidth + 1) {
            event.preventDefault();
            quotaList.scrollBy({ left: event.deltaY });
            return;
          }
        }
        event.preventDefault();
        event.stopPropagation();
        cycleCompactRef.current(direction);
        return;
      }
      if (!target.closest(".island-nav")) return;
      event.preventDefault();
      event.stopPropagation();
      const now = Date.now();
      if (now - navWheelAt.current < 220) return;
      navWheelAt.current = now;
      cycleTabRef.current(direction);
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
    // 首帧 config 还没到 → 渲染的是 island-boot，islandRef 为空；配置到达后
    // 必须重新挂一次监听（依赖 config 而不是空数组）。
  }, [config]);

  // ===== P2：消息页动作 =====
  const applyNotifyState = (next: NotifyState | null | undefined) => {
    if (!next) return;
    const seen = seenNotifyIds.current;
    for (const item of next.items) seen.add(item.id);
    // 留出超过后端历史上限的空间，临时隐藏的模块恢复后也不会重放提醒。
    while (seen.size > 128) {
      const oldest = seen.values().next();
      if (oldest.done) break;
      seen.delete(oldest.value);
    }
    notifyBaselineReady.current = true;
    setNotify(next);
  };

  const markNotifyKindRead = (kind: string) => {
    void markNotifyReadKind(kind).then(applyNotifyState).catch(() => {});
  };

  /** 全部已读：对每个显示模块调 island_notify_mark_read_kind（只清未读） */
  const markNotifyAllRead = (kinds: string[]) => {
    void Promise.all(kinds.map((kind) => markNotifyReadKind(kind)))
      .then(() => getNotify())
      .then(applyNotifyState)
      .catch(() => {});
  };

  const clearNotifyKind = (kind?: string) => {
    void clearNotify(kind).then(applyNotifyState).catch((e) => showToast(String(e)));
  };

  /** 写 config.notifyModules；Rust 按新模块过滤通知流，这里主动补拉一次状态 */
  const changeNotifyModules = (modules: string[]) => {
    updateConfig({ notifyModules: modules }, true);
    window.setTimeout(() => {
      void getNotify().then(applyNotifyState).catch(() => {});
    }, 150);
  };

  const openNotifyModule = (kind: string, exe?: string) => {
    void openNotifyApp(kind, exe).then(applyNotifyState).catch((e) => showToast(String(e)));
  };

  const refreshAll = () => {
    setRefreshing(true);
    void refreshQuota();
    void refreshWeather();
    void refreshStocks();
    // 统一走事件同款 handler，保证同步 ref、activity / 自动切视图不重复触发
    void getMedia().then(handleMedia).catch(() => {});
    void getSystem().then(handleSystem).catch(() => {});
    void getClipboard().then(setClipboard).catch(() => {});
    void getNotify().then(applyNotifyState).catch(() => {});
    void getTodayUsage().then((data) => data && setTodayUsage(data)).catch(() => {});
    window.setTimeout(() => setRefreshing(false), 900);
  };

  /**
   * 天气城市绑定 / 增删后回填状态：Rust 的四个天气城市命令内部已经 refresh()
   * 并广播 zbar://island-weather；这里再补拉一次，浏览器预览没有事件桥时也能
   * 看到列表变化（Tauri 下与事件刷新结果一致）。
   */
  const reloadWeather = async () => {
    const next = await getWeather().catch(() => null);
    if (next) setWeather(next);
  };

  // 悬停挤压与拖拽共用同一个液态轮廓。
  const updateHoverPoke = (clientX: number, clientY: number) => {
    const island = islandRef.current;
    const root = rootRef.current;
    const poke = pokeState.current;
    if (!island || !root || openRef.current || dragActiveRef.current || poke.pressing) return;
    const rect = island.getBoundingClientRect();
    const rootRect = root.getBoundingClientRect();
    liquidPokeRef.current = { x: clientX - rootRect.left, y: clientY - rootRect.top };
    const nx = (clientX - (rect.left + rect.width / 2)) / Math.max(1, rect.width / 2);
    const ny = (clientY - (rect.top + rect.height / 2)) / Math.max(1, rect.height / 2);
    poke.stiffness = 260;
    poke.ratio = 0.75;
    poke.target = (1 - Math.min(1, Math.hypot(nx, ny))) ** 1.4;
    // 悬停挤压目标变化：liquid 引擎可能正在休眠，即时唤醒保证形变跟手。
    liquidWakeRef.current();
  };

  const releasePoke = () => {
    Object.assign(pokeState.current, { pressing: false, stiffness: 300, ratio: 0.22, target: 0 });
    liquidWakeRef.current();
  };

  const resetPoke = () => {
    Object.assign(pokeState.current, { ...springAt(0), target: 0, pressing: false });
    liquidWakeRef.current();
  };
  resetPokeRef.current = resetPoke;

  const clearDrag = () => {
    const wasDragging = dragActiveRef.current;
    dragStartingRef.current = false;
    dragActiveRef.current = false;
    dragFrameRef.current = null;
    snapEdgeRef.current = null;
    // 拖拽结束：motion 输入清空，唤醒引擎补间回锚定几何。
    liquidWakeRef.current();
    setDragging(false);
    if (wasDragging) refreshBackdropRef.current?.();
  };

  const runDragLoop = (epoch: number) => {
    const tick = async () => {
      if (epoch !== dragEpochRef.current) return;
      try {
        const step = await stepIslandDrag();
        if (epoch !== dragEpochRef.current) return;
        if (!step.started && dragActiveRef.current && !snapEdgeRef.current) {
          clearDrag();
          pendingExpand.current = false;
          return;
        }
        if (!step.started && !dragActiveRef.current) {
          if (step.active) window.requestAnimationFrame(tick);
          else dragStartingRef.current = false;
          return;
        }
        const justStarted = !dragActiveRef.current;
        if (justStarted) {
          dragActiveRef.current = true;
          dragStartingRef.current = false;
          suppressClickRef.current = true;
          blockHoverUntilLeave.current = true;
          if (pointerRef.current) pointerRef.current.moved = true;
          liquidPreviewRef.current = null;
          neckRef.current = { edge: edgeRef.current, spring: springAt(1) };
          dragMonitorRef.current = step.monitor;
          cancelClick();
          cancelCollapse();
          resetPoke();
          setDragging(true);
          setCompactPressed(false);
          if (openRef.current) {
            openRef.current = false;
            pinUntil.current = 0;
            setOpen(false);
            closeMenuRef.current();
            void setKeyboard(false);
            void setIslandExpanded(false).catch(() => {});
          }
        }

        dragFrameRef.current = step;
        if (step.origin) canvasOriginRef.current = step.origin;
        // 拖拽帧到达：即使引擎在休眠也立即回 rAF 模式逐帧跟画。
        liquidWakeRef.current();
        if (justStarted) refreshBackdropRef.current?.();
        if (step.settled && snapEdgeRef.current) {
          const expandAfter = pendingExpand.current && expandModeOf() !== "never";
          pendingExpand.current = false;
          clearDrag();
          if (!rootRef.current?.matches(":hover")) blockHoverUntilLeave.current = false;
          if (expandAfter || expandModeOf() === "always") {
            window.setTimeout(() => {
              if (epoch !== dragEpochRef.current) return;
              if (expandModeOf() === "never") return;
              expand();
              if (expandAfter) pinUntil.current = performance.now() + CLICK_PIN_MS;
            }, 80);
          }
          return;
        }
        if (!step.active && !snapEdgeRef.current) {
          const result = await endIslandDrag();
          if (epoch !== dragEpochRef.current) return;
          snapEdgeRef.current = result.edge;
          edgeRef.current = result.edge;
          setEdge(result.edge);
          if (!IS_TAURI && configRef.current) {
            const next = { ...configRef.current, edge: result.edge };
            configRef.current = next;
            setConfig(next);
          }
        }
        window.requestAnimationFrame(tick);
      } catch (error) {
        if (epoch !== dragEpochRef.current) return;
        clearDrag();
        pendingExpand.current = false;
        showToast(`拖拽失败: ${String(error)}`);
      }
    };
    window.requestAnimationFrame(tick);
  };

  const beginPointerDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !event.isPrimary || configRef.current?.passthrough) return;
    if (dragActiveRef.current && !snapEdgeRef.current) return;
    suppressClickRef.current = false;
    const target = event.target instanceof Element ? event.target : null;
    if (!target || target.closest(
      "input, textarea, select, a, [contenteditable='true'], .ctl-range, .compact-play, " +
      "[role='button'], .agent-card, .island-context-menu, [data-no-dblclick]"
    )) return;
    const expanded = openRef.current;
    const onGlass = Boolean(target.closest(".liquid-glass"));
    if (!onGlass && (expanded
      ? !target.closest(".island-panel") || target.closest("button")
      : !target.closest(".pill-wrap"))) return;
    const navigation = Boolean(target.closest(".island-nav"));
    cancelHover();
    cancelCollapse();
    pointerRef.current = {
      id: event.pointerId,
      time: event.timeStamp,
      x: event.screenX,
      y: event.screenY,
      expanded,
      navigation,
      moved: false,
    };
    if (snapEdgeRef.current) return;

    const epoch = ++dragEpochRef.current;
    dragStartingRef.current = true;
    void beginIslandDrag({
      horizontal: { width: compactWidth, height: compactHeight },
      vertical: { width: sideCompactWidth, height: sideCompactHeight },
    }, navigation, expanded, edgeRef.current, { x: event.clientX, y: event.clientY }).then((result) => {
      if (epoch !== dragEpochRef.current) return;
      if (result.active) runDragLoop(epoch);
      else dragStartingRef.current = false;
    }).catch((error) => {
      if (epoch !== dragEpochRef.current) return;
      clearDrag();
      showToast(`拖拽失败: ${String(error)}`);
    });

    if (!expanded) {
      const root = rootRef.current?.getBoundingClientRect();
      if (root) liquidPokeRef.current = { x: event.clientX - root.left, y: event.clientY - root.top };
      Object.assign(pokeState.current, { pressing: true, stiffness: 380, ratio: 0.55, target: 1 });
      // 按压起跳：收起态静止时引擎在休眠，必须即时唤醒出下压形变。
      liquidWakeRef.current();
      setCompactPressed(true);
      setInteractSeq((value) => value + 1);
    }
  };

  const finishPointerPress = (event: PointerEvent | null, cancelled = false) => {
    const pointer = pointerRef.current;
    if (!pointer || (event && pointer.id !== event.pointerId)) return;

    pointerRef.current = null;
    setCompactPressed(false);
    releasePoke();
    const target = event?.target instanceof Element ? event.target : null;
    const inside = Boolean(target && rootRef.current?.contains(target));
    const longPress = event && event.timeStamp - pointer.time >= LONG_PRESS_MS;
    const moved = pointer.moved || (event && hasDragMovement(
      event.screenX - pointer.x, event.screenY - pointer.y, pointer.navigation
    ));
    if (cancelled || longPress || moved) {
      suppressClickRef.current = true;
      blockHoverUntilLeave.current = inside || (cancelled && Boolean(rootRef.current?.matches(":hover")));
      cancelClick();
      return;
    }

    if (event && inside && !pointer.expanded && !openRef.current &&
        target?.closest(".pill-wrap, .liquid-glass") && !target.closest(".compact-play")) {
      queuePillClick({ x: event.screenX, y: event.screenY });
    }
  };
  finishPressRef.current = finishPointerPress;

  useEffect(() => {
    const move = (event: PointerEvent) => {
      const pointer = pointerRef.current;
      if (!pointer || pointer.id !== event.pointerId) return;
      if ((event.buttons & 1) === 0) {
        finishPressRef.current(event, true);
        return;
      }
      pointer.moved ||= hasDragMovement(
        event.screenX - pointer.x, event.screenY - pointer.y, pointer.navigation
      );
    };
    const up = (event: PointerEvent) => {
      if (event.button === 0) finishPressRef.current(event);
    };
    const cancel = (event: PointerEvent) => finishPressRef.current(event, true);
    const blur = () => finishPressRef.current(null, true);
    window.addEventListener("pointermove", move, true);
    window.addEventListener("pointerup", up, true);
    window.addEventListener("pointercancel", cancel, true);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("pointermove", move, true);
      window.removeEventListener("pointerup", up, true);
      window.removeEventListener("pointercancel", cancel, true);
      window.removeEventListener("blur", blur);
    };
  }, []);

  const handlePointerDownCapture = (event: ReactPointerEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement | null;
    const needsKeyboard = Boolean(
      target?.closest(
        "textarea, select, [contenteditable='true'], input:not([type='range']):not([type='checkbox']):not([type='radio']):not([type='button'])"
      )
    );
    void setKeyboard(needsKeyboard).then(() => {
      if (needsKeyboard) {
        window.setTimeout(() => target?.focus?.(), 30);
      }
    });
  };

  // ⚠️ 原生内存泄漏事故记录（2026-11-17，参照 webview_args.rs 的事故回滚记录风格）：
  // liquid 主循环曾是 60fps 永动 rAF——稳定快路径带 isOpen && 前置条件，收起态
  // （岛约 95% 的存在时间）永远为 false，每帧都走全量路径；收起态「呼吸」又用
  // 正弦逐帧扰动 fillet，令 liquidPath 每帧产出不同路径，于是 --liquid-clip
  // （巨型 path 字符串）、两条 SVG path 的 d、--body-r-* 圆角与 flow 渐变每帧
  // 重写，780×632 全画布（clip-path 变化使 blur 层全部重光栅）随之逐帧重绘。
  // 用户实测渲染进程 8 分钟 2.5GB→6.0GB（≈440MB/min，20 分钟 6.7GB），页面
  // JS 堆全程仅 1-12MB——纯原生内存泄漏；CDP A/B 验证：页面内置空
// requestAnimationFrame 后增长立即停止并回落。修复三件套（1~3 为前两轮）：
//   1) 稳定判定不再带 isOpen：全部输入静止即休眠（收起态同样适用）；
//   2) 呼吸最终删除（第二轮 CDP 复测：矢量 clip-path 子树内连 transform
//      动画也无法合成器化，三个常驻 CSS 动画叠加泄漏更凶，收起态静止
//      即静止，见 island.css P4 段落的事故记录）；
//   3) 休眠期以 500ms 低频轮询只读比较输入指纹兜底，事件侧即时唤醒，
//      漏挂唤醒钩子时最坏 500ms 自愈。
//   4) 第三轮（同日，呼吸回归·结构性修复）：静止不是永久答案——休眠帧
//      用最终 body rect + 四角 radii 换算 --idle-clip（inset round 圆角
//      矩形裁剪，合成器原生支持）并挂 is-idle，三层裁剪临时切换过去，
//      liquid-breath 仅在 is-idle 下做 CSS transform 呼吸（圆角矩形裁剪
//      下 transform 真正跑 GPU 合成器，零重绘）；唤醒即摘类回到矢量
//      path()。形变期与前两轮的行为完全不变，取舍详见 island.css P4
//      事故记录第三轮结论。
  useLayoutEffect(() => {
    if (!config) return;
    const root = rootRef.current;
    const island = islandRef.current;
    const glass = liquidGlassRef.current;
    const frame = bodyFrameRef.current;
    if (!root || !island || !glass || !frame) return;
    let raf = 0;
    let lastNow = performance.now();
    let lastPath = "";
    let previousTarget = bodyTargetRef.current;
    const widthSpring = springAt(previousTarget.width);
    const heightSpring = springAt(previousTarget.height);
    const panelAlpha = springAt(previousTarget.open ? 1 : 0);
    const compactAlpha = springAt(previousTarget.open ? 0 : 1);
    let widthFloor = previousTarget.open;
    let heightFloor = previousTarget.open;
    let panelReady = previousTarget.open;
    let sizeStiffness = previousTarget.open ? 420 : 460;
    let sizeDamping = previousTarget.open ? 0.62 : 0.64;
    let lastWidth = widthSpring.value;
    let lastHeight = heightSpring.value;
    let smoothedPoke = { ...liquidPokeRef.current };
    // 空闲休眠：稳定帧写完最终几何后不再排 rAF，改由低频轮询只读比较输入指纹。
    let sleeping = false;
    let pollTimer = 0;
    let sleepSnapshot = "";

    // 输入指纹：轮询时只读比较这些 ref，不写任何 DOM。覆盖 busy 判定的全部
    // 外部输入——几何目标（开合/尺寸）、吸附边、预览、拖拽帧、按压弹簧状态、
    // 平滑按压点来源、残滴数量、音频电平。
    const inputFingerprint = () => {
      const target = bodyTargetRef.current;
      const poke = pokeState.current;
      const preview = liquidPreviewRef.current;
      const pokePoint = liquidPokeRef.current;
      const audioLevel = !dragFrameRef.current && !preview && !target.open
        ? liquidAudioLevelRef.current
        : 0;
      return [
        target.open ? 1 : 0,
        target.width.toFixed(2),
        target.height.toFixed(2),
        edgeRef.current,
        preview ? `${preview.edge}:${preview.gap.toFixed(2)}:${preview.drip ? 1 : 0}` : "-",
        dragFrameRef.current ? String(dragFrameRef.current.monitor ?? "drag") : "-",
        poke.target.toFixed(4),
        poke.value.toFixed(4),
        poke.velocity.toFixed(4),
        poke.pressing ? 1 : 0,
        pokePoint.x.toFixed(1),
        pokePoint.y.toFixed(1),
        dripsRef.current.length,
        audioLevel.toFixed(4),
      ].join("|");
    };

    const wake = () => {
      if (!sleeping) return;
      sleeping = false;
      // 摘除 is-idle：--liquid-clip（休眠时未被清除，仍持有最终路径）立即
      // 重新生效，几何相同无缝；rAF 回调先于本帧 paint 执行，中间不会闪出
      // 矢量/圆角裁剪交替的一帧。随后的 tick 按新输入接管重写路径。
      root.classList.remove("is-idle");
      window.clearTimeout(pollTimer);
      raf = window.requestAnimationFrame(tick);
    };

    const poll = () => {
      if (inputFingerprint() !== sleepSnapshot) {
        wake();
        return;
      }
      pollTimer = window.setTimeout(poll, LIQUID_IDLE_POLL_MS);
    };

    const tick = (now: number) => {
      const dt = Math.max(1 / 240, Math.min(1 / 30, (now - lastNow) / 1000));
      const poke = pokeState.current;
      const motion = dragFrameRef.current;
      const preview = liquidPreviewRef.current;
      const target = bodyTargetRef.current;
      const isOpen = target.open;
      const edge = preview?.edge ?? edgeRef.current;
      const lines = motion?.edgeLines ?? STATIC_EDGE_LINES;
      const origin = canvasOriginRef.current;
      const neck = neckRef.current;
      const audioLevel = !motion && !preview && !isOpen ? liquidAudioLevelRef.current : 0;
      const changingSize = widthSpring.value !== target.width || heightSpring.value !== target.height ||
        widthSpring.velocity !== 0 || heightSpring.velocity !== 0;
      const fading = panelAlpha.value !== Number(isOpen) || compactAlpha.value !== Number(!isOpen);
      // 活跃判定（事故修复点：不再含 isOpen，收起态静止同样是稳定态）。按压按
      // 「弹簧值到位且速度为零」判稳——悬停挤压保持在目标形变时几何静止，平滑
      // 按压点收敛到指针（≤0.5px，形变中心跟手）后即可休眠，指针一动即被唤醒。
      const pokeGoal = isOpen || motion ? 0 : poke.target;
      const pokeSettled = poke.value === pokeGoal && poke.velocity === 0 && (
        pokeGoal === 0 ||
        Math.abs(smoothedPoke.x - liquidPokeRef.current.x) <= 0.5 &&
        Math.abs(smoothedPoke.y - liquidPokeRef.current.y) <= 0.5
      );
      const busy = Boolean(motion) || Boolean(preview) || audioLevel > 0 ||
        !pokeSettled || changingSize || fading || dripsRef.current.length > 0;
      advanceSpring(poke, isOpen || motion ? 0 : poke.target, poke.stiffness, poke.ratio, dt, 0.004);

      if (motion?.bodyRect) {
        Object.assign(widthSpring, springAt(motion.bodyRect.width));
        Object.assign(heightSpring, springAt(motion.bodyRect.height));
      } else {
        const opening = isOpen && !previousTarget.open;
        if (!isOpen) {
          sizeStiffness = 460;
          sizeDamping = 0.64;
        } else if (opening) {
          sizeStiffness = 420;
          sizeDamping = 0.62;
        } else if (previousTarget.width !== target.width || previousTarget.height !== target.height) {
          sizeStiffness = 440;
          sizeDamping = 0.6;
        }
        if (opening || previousTarget.width !== target.width) widthFloor = widthSpring.value >= target.width;
        if (opening || previousTarget.height !== target.height) heightFloor = heightSpring.value >= target.height;
        advanceSpring(widthSpring, target.width, sizeStiffness, sizeDamping, dt, 0.2);
        advanceSpring(heightSpring, target.height, sizeStiffness, sizeDamping, dt, 0.2);
        if (isOpen) {
          if (widthSpring.value >= target.width) widthFloor = true;
          else if (widthFloor) Object.assign(widthSpring, springAt(target.width));
          if (heightSpring.value >= target.height) heightFloor = true;
          else if (heightFloor) Object.assign(heightSpring, springAt(target.height));
        }
      }

      const width = Math.max(40, widthSpring.value);
      const height = Math.max(24, heightSpring.value);
      if (!isOpen) {
        panelReady = false;
        if (panelAlpha.value !== 0) Object.assign(panelAlpha, springAt(0));
      } else if (width >= target.width - 1 && height >= target.height - 1) {
        panelReady = true;
      }
      if (panelReady) advanceSpring(panelAlpha, 1, 700, 1, dt, 0.01);
      advanceSpring(compactAlpha, Number(!isOpen), isOpen ? 900 : 420, 1, dt, 0.01);
      island.classList.toggle("panel-ready", panelReady);
      island.style.setProperty("--panel-opacity", panelAlpha.value.toFixed(3));
      island.style.setProperty("--compact-opacity", compactAlpha.value.toFixed(3));
      island.style.setProperty("--liquid-w", `${width}px`);
      island.style.setProperty("--liquid-h", `${height}px`);
      previousTarget = target;

      const pos = motion?.bodyRect ?? anchoredBodyPos(edge, width, height);
      const body: IslandBodyRect = { x: pos.x, y: pos.y, width, height };

      if (preview) {
        if (edge === "top") body.y += preview.gap;
        if (edge === "bottom") body.y -= preview.gap;
        if (edge === "left") body.x += preview.gap;
        if (edge === "right") body.x -= preview.gap;
      }
      frame.style.transform = `translate3d(${body.x}px, ${body.y}px, 0)`;

      let sx = 1;
      let sy = 1;
      let contentX = 0;
      let contentY = 0;
      const squash = motion?.squash ?? 0;
      if (squash > 0 || (!motion && !preview && !isOpen)) {
        const pk = Math.max(-0.5, Math.min(1.2, poke.value));
        const side = isSideEdge(edge);
        const along = side ? height : width;
        const depth = side ? width : height;
        const nextAlong = squash > 0 ? along + squash * 1.1 : along * (1 + 0.05 * pk);
        const nextDepth = squash > 0 ? Math.max(depth * 0.62, depth - squash) : depth * (1 - 0.16 * pk);
        sx = (side ? nextDepth : nextAlong) / Math.max(1, width);
        sy = (side ? nextAlong : nextDepth) / Math.max(1, height);
        const shiftX = (width - width * sx) / 2;
        const shiftY = (height - height * sy) / 2;
        contentX = edge === "left" ? -shiftX : edge === "right" ? shiftX : 0;
        contentY = edge === "top" ? -shiftY : edge === "bottom" ? shiftY : 0;
        body.x += shiftX + contentX;
        body.y += shiftY + contentY;
        body.width *= sx;
        body.height *= sy;
      }
      island.style.setProperty("--poke-sx", String(sx));
      island.style.setProperty("--poke-sy", String(sy));
      island.style.setProperty("--poke-x", `${contentX}px`);
      island.style.setProperty("--poke-y", `${contentY}px`);

      const horizontal = (e: IslandEdge) => !isSideEdge(e);
      const alongCenter = (e: IslandEdge) => horizontal(e) ? body.x + body.width / 2 : body.y + body.height / 2;
      const edgeOrigin = (e: IslandEdge) => horizontal(e) ? origin.y : origin.x;
      const alongOrigin = (e: IslandEdge) => horizontal(e) ? origin.x : origin.y;
      const breakNeck = () => {
        if (neck.edge === null) return;
        const e = neck.edge;
        dripsRef.current.push({
          edge: e, line: lines[e] + edgeOrigin(e), center: alongCenter(e) + alongOrigin(e),
          radius: springAt(7.5),
        });
        neck.edge = null;
        neck.spring = springAt(0);
      };

      if (motion) {
        if (dragMonitorRef.current !== motion.monitor) {
          dragMonitorRef.current = motion.monitor;
          neck.edge = null;
          neck.spring = springAt(0);
          dripsRef.current = [];
        }
        if (neck.edge !== null && (
          (snapEdgeRef.current !== null && neck.edge !== snapEdgeRef.current) ||
          screenGap(neck.edge, body, lines[neck.edge]) > BREAK
        )) breakNeck();
        if (neck.edge === null) {
          const nearest = snapEdgeRef.current ?? nearestScreenEdge(body, lines);
          if (screenGap(nearest, body, lines[nearest]) < JOIN) {
            neck.edge = nearest;
            neck.spring = springAt(0);
          }
        }
        advanceSpring(neck.spring, 1, 520, 1, dt, 0.01);
      } else {
        neck.edge = edge;
        neck.spring = springAt(preview && preview.gap > BREAK ? 0 : 1);
      }

      const drips: LiquidDrip[] = [];
      dripsRef.current = dripsRef.current.filter((drip) => {
        advanceSpring(drip.radius, 0, 160, 0.45, dt);
        if (drip.radius.value < 0.3 && drip.radius.velocity <= 0) return false;
        drips.push({
          edge: drip.edge,
          line: drip.line - edgeOrigin(drip.edge),
          center: drip.center - alongOrigin(drip.edge),
          radius: drip.radius.value,
        });
        return true;
      });
      if (preview?.drip && preview.gap > BREAK) {
        drips.push({ edge, line: lines[edge], center: alongCenter(edge), radius: 7.5 });
      }

      const neckEdge = neck.edge ?? edge;
      const neckStrength = neck.edge === null ? 0 : neck.spring.value;
      const gap = Math.max(0, screenGap(neckEdge, body, lines[neckEdge]));
      // 呼吸：JS 正弦扰动 fillet 已删（事故修复点：矢量 clip 子树内逐帧
      // 重绘引发原生内存泄漏）；静止后的呼吸改由 CSS 在 is-idle 圆角矩形
      // 裁剪下承担（liquid-breath，见 island.css 的事故记录）。路径在本帧
      // 与后续静止帧保持确定不变，主循环才有可能休眠。
      const shapeSpeed = (Math.abs(width - lastWidth) + Math.abs(height - lastHeight)) / dt;
      const fillet = FILLET + Math.min(12, shapeSpeed * 0.012);
      lastWidth = width;
      lastHeight = height;
      lastNow = now;
      smoothedPoke.x += (liquidPokeRef.current.x - smoothedPoke.x) * 0.3;
      smoothedPoke.y += (liquidPokeRef.current.y - smoothedPoke.y) * 0.3;

      const flow = liquidFlowGradientRef.current;
      if (flow && !isOpen && !motion) {
        const phase = 0.5 + 0.5 * Math.sin(now / 1000 * 0.55);
        const cx = edge === "left" ? body.x + body.width : edge === "right" ? body.x : body.x + body.width * phase;
        const cy = edge === "top" ? body.y + body.height : edge === "bottom" ? body.y : body.y + body.height * phase;
        flow.setAttribute("cx", String(cx));
        flow.setAttribute("cy", String(cy));
        flow.setAttribute("r", String(Math.max(60, body.width * 0.35)));
      }
      const radii = liquidRadii(body, neckEdge, gap, neckStrength);
      const shape = {
        edge: neckEdge,
        line: lines[neckEdge],
        body,
        gap,
        neck: neckStrength,
        fillet,
        drips,
        poke: motion || isOpen ? 0 : poke.value,
        pokePoint: smoothedPoke,
        vx: motion?.vx ?? 0,
        vy: motion?.vy ?? 0,
        wave: audioLevel > 0 ? { amplitude: audioLevel, time: now / 1000 } : undefined,
      };
      const d = liquidPath(shape);
      if (d && d !== lastPath) {
        const geometry = {
          width: CANVAS_W, height: CANVAS_H,
          x: body.x, y: body.y, bodyWidth: body.width, bodyHeight: body.height,
          radii,
        };
        glassBackdropGeometryRef.current = geometry;
        glassBackdropRef.current?.draw(geometry);
        if (WINDOWS_TAURI) updateNativeGlassGeometry(nativeGlassBody(shape, radii));
        glass.style.setProperty("--liquid-body-top", `${body.y}px`);
        glass.style.setProperty("--liquid-body-bottom", `${body.y + body.height}px`);
        liquidRimGradientRef.current?.setAttribute("y1", String(body.y));
        liquidRimGradientRef.current?.setAttribute("y2", String(body.y + body.height));
        const tintX = body.width / 4;
        const tintY = body.height;
        const tintLength = Math.hypot(tintX, tintY);
        const tintStart = (body.x * tintX + body.y * tintY) / Math.max(1, tintLength);
        glass.style.setProperty("--glass-tint-angle", `${Math.atan2(tintX, -tintY)}rad`);
        glass.style.setProperty("--glass-tint-start", `${tintStart}px`);
        glass.style.setProperty("--glass-tint-end", `${tintStart + tintLength}px`);
        lastPath = d;
        root.style.setProperty("--liquid-clip", cssPath(d));
        liquidRimRef.current?.setAttribute("d", d);
        liquidFlowRef.current?.setAttribute("d", d);
      }
      ["tl", "tr", "br", "bl"].forEach((corner, i) => {
        island.style.setProperty(`--body-r-${corner}`, `${radii[i]}px`);
      });
      if (busy) {
        raf = window.requestAnimationFrame(tick);
        return;
      }
      // 进入稳定休眠：本帧已写入「无呼吸扰动、无音频波」的最终几何；之后
      // 不排 rAF，改由 500ms 轮询只读比较输入指纹，变化即唤醒回 rAF 模式。
      // 音频律动期间 audioLevel > 0 属真实业务渲染，保持活跃；归零后即回到
      // 这里休眠。
      sleeping = true;
      sleepSnapshot = inputFingerprint();
      pollTimer = window.setTimeout(poll, LIQUID_IDLE_POLL_MS);
      // 第三轮修复（呼吸回归）：静止且无按压形变时，把三层裁剪从矢量
      // path() 切到合成器友好的圆角矩形（inset round），CSS transform
      // 呼吸才能真正跑 GPU 合成器。两个前置条件缺一不可：
      //   a) !isOpen——展开态不呼吸（island.css 选择器同样限定 is-collapsed）；
      //   b) poke.value === 0——悬停挤压静止时路径是 polyPath 形变 blob，
      //      不是圆角矩形，硬套 inset 会跳变；此时保持矢量裁剪原样休眠。
      // 已知取舍：贴边静止轮廓带液桥肩角（gap=0 时比 body 宽约 fillet=13px，
      // 见 liquid.ts 的 neckPath），圆角矩形表达不了，休眠/唤醒瞬间肩角会
      // 各跳变一次；详见 island.css P4 事故记录第三轮。
      if (!isOpen && poke.value === 0) {
        // inset(top right bottom left round tl tr br bl)：四边 = body 距画布
        // 四边的距离；圆角 = 本帧最终 radii（liquidRadii 顺序恰为 tl,tr,br,bl，
        // 与 inset round 一致），与最后一帧 bodyPath 逐点相同（肩角除外）。
        const idleTop = Math.max(0, body.y);
        const idleRight = Math.max(0, CANVAS_W - body.x - body.width);
        const idleBottom = Math.max(0, CANVAS_H - body.y - body.height);
        const idleLeft = Math.max(0, body.x);
        root.style.setProperty(
          "--idle-clip",
          `inset(${idleTop.toFixed(2)}px ${idleRight.toFixed(2)}px ${idleBottom.toFixed(2)}px ${idleLeft.toFixed(2)}px round ${radii[0].toFixed(2)}px ${radii[1].toFixed(2)}px ${radii[2].toFixed(2)}px ${radii[3].toFixed(2)}px)`
        );
        root.classList.add("is-idle");
      }
    };
    liquidWakeRef.current = wake;
    tick(lastNow + 30);
    return () => {
      liquidWakeRef.current = () => {};
      root.classList.remove("is-idle");
      window.cancelAnimationFrame(raf);
      window.clearTimeout(pollTimer);
    };
  }, [Boolean(config), updateNativeGlassGeometry]);

  // 渲染期写入的 liquid 输入（bodyTargetRef 在 render 里赋值、edgeRef 跟随
  // edge state）不经过事件回调，任何重渲染后都唤醒一次引擎：已活跃时是空
  // 操作，休眠中则立即恢复 rAF——展开/收起、切标签改面板高度、收起视图换
  // 宽度、媒体起播/暂停（media state）等都走这条路。
  useLayoutEffect(() => {
    liquidWakeRef.current();
  });

  // 静止时上报当前本体尺寸，拖拽与吸附期间由原生帧提供尺寸。
  useEffect(() => {
    if (!config || dragging) return;
    const element = islandRef.current;
    if (!element) return;
    let last = "";
    const report = () => {
      if (dragActiveRef.current) return;
      const { offsetWidth: width, offsetHeight: height } = element;
      const key = `${width}x${height}:${openRef.current}`;
      if (key === last) return;
      last = key;
      void islandSetBodySize(width, height, openRef.current).catch(() => {});
    };
    report();
    const observer = new ResizeObserver(report);
    observer.observe(element);
    window.addEventListener("resize", report);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", report);
    };
  }, [config, open, dragging]);

  // 可见标签 / 当前标签在早退前算好：切页持久化 effect、滚轮回调、菜单都要用。
  const hidden = useMemo(() => new Set(config?.hiddenTabs ?? []), [config?.hiddenTabs]);
  const tabs = useMemo(
    () => TABS.filter((t) => t.key === "home" || !hidden.has(t.key)),
    [hidden]
  );
  // 当前标签不在可见标签里（例如刚被「隐藏板块」移除）时回落到主页；
  // 设置不是岛内标签（独立窗口承载），这里不再有 settings 分支。
  const activeTab: TabKey = tabs.some((t) => t.key === tab) ? tab : "home";
  // 事件回调里读当前页（见 zbar://island-expand 的剪贴板热键切页）
  tabRef.current = activeTab;

  // 切页后记住 lastTab：独立命令落盘（不整份写配置），失败静默。
  // 本地 config 也要同步 lastTab：否则之后任何 updateConfig 整份保存都会把
  // 旧 lastTab 回写给 Rust，「记住上次标签」失效（code-reviewer P1-2）。
  // updateConfig 的 commit 读的是 configRef.current，这里先写 ref 再写 state，
  // 已排队但尚未执行的整份保存也会自然带上新值。
  useEffect(() => {
    if (!config) return;
    // 首帧 activeTab 恒为 home，而 remembered 还没解析完；此刻写 island_set_last_tab
    // 会把用户上次的页面覆盖成 home（P2-11）。等 boot 的 setTab 生效后第一个
    // activeTab 与 bootTabRef 相同的那一轮再写。
    if (bootTabRef.current && bootTabRef.current !== activeTab) return;
    bootTabRef.current = null;
    const current = configRef.current;
    if (current && current.lastTab !== activeTab) {
      const next: IslandConfig = { ...current, lastTab: activeTab };
      configRef.current = next;
      setConfig(next);
    }
    void islandSetLastTab(activeTab).catch(() => {});
  }, [activeTab, Boolean(config)]);

  // 面板高度：每个标签一张常量表（按 640 宽下内容实际高度），最大 492。
  const panelHeight = Math.min(
    MAX_PANEL_H,
    Math.max(300, PANEL_HEIGHT[activeTab] ?? MAX_PANEL_H)
  );

  // ===== P1：收起态数据（歌词当前行 / 未读角标）=====
  const lyricLines = useMemo(
    () => (lyrics?.ok ? parseLyrics(lyrics.lrc, lyrics.translation) : []),
    [lyrics]
  );
  const lyricNow = useMemo(
    () => currentLyricLine(lyricLines, media.position),
    [lyricLines, media.position]
  );
  const lyricKey = lyricNow?.text
    ? `${lyricNow.time}|${lyricNow.text}`
    : `${media.title}|${media.artist}`;
  const badgeCounts = useMemo<BadgeCounts>(() => {
    const counts: BadgeCounts = {};
    for (const item of notify.items) {
      if (item.read) continue;
      counts[item.kind] = (counts[item.kind] ?? 0) + 1;
    }
    if (notify.wechatUnread !== undefined) counts.wechat = notify.wechatUnread;
    return counts;
  }, [notify.items, notify.wechatUnread]);

  if (!config) {
    return <div className="island-boot" />;
  }

  // 透光值只参与玻璃合成。
  const transparency = Number.isFinite(config.transparency)
    ? Math.max(0, Math.min(90, config.transparency))
    : 0;
  // 主题色板：auto 由身后桌面平均亮度决定 dark / light，固定主题直接用配置值。
  const themePalette: IslandThemePalette =
    themeMode === "auto" ? (autoDark ? "dark" : "light") : themeMode;
  // 与面板 / 主窗口同口径：13px 是原始设计基准，自定义值按比例缩放全部文字。
  const fontSize = Number.isFinite(config.fontSize)
    ? Math.max(10, Math.min(18, config.fontSize))
    : 13;
  const fontScale = fontSize / 13;

  // ===== P1：收起态视图与玻璃尺寸 =====
  // 视图解析顺序：activity（最高） > 运行时覆盖（滚轮 / 音乐自动切 / 自动轮换）
  // > config.compactView。玻璃高度随视图变，圆角 = min(30, h/2)（参考 base_r）。
  const configView = normalizeCompactView(config.compactView);
  const compactViewBase: CompactView = viewOverride ?? configView;
  const compactViewKey: CompactViewKey | "activity" = activity ? "activity" : compactViewBase;
  const side = isSideEdge(edge);
  // 实时字幕层：最后一句未超过 7 秒 + 横条（竖胶囊 48 宽放不下两行）。
  // 不要求聆听中：停止时后端补推的定稿句同样在保鲜期内上胶囊。
  // 优先级照参考项目 compact.py：临时提醒（activity：音量 / 通知 / 股票异动）最高，
  // 其次才是字幕；字幕盖住音乐 / 时间 / 小形象 / 额度环，宽度取 min(640, 可用宽度)。
  const liveCaptionLayer: CompactLiveCaption | null =
    liveCaptionOn && liveCaption && !side && activity === null
      ? { on: true, jp: liveCaption.jp, zh: liveCaption.zh, at: liveCaptionAt }
      : null;
  const showLiveCaption = liveCaptionLayer !== null;
  const showCompactQuotas = !showLiveCaption
    && compactViewKey !== "rings"
    && (compactViewBase === "music" || (media.hasSession && Boolean(media.title)));
  const compactQuotaAgents = orderedUsage?.agents ?? [];
  const compactQuotaExtent = showCompactQuotas
    ? COMPACT_QUOTA_PADDING + (compactQuotaAgents.length === 0
      ? side ? COMPACT_QUOTA_CELL : COMPACT_QUOTA_CELL * 2
      : Math.min(side ? 4 : 6, compactQuotaAgents.length) * COMPACT_QUOTA_CELL)
    : 0;
  const compactHeight = showLiveCaption
    ? LIVE_CAPTION_HEIGHT
    : compactViewKey === "rings"
      ? quotaSizes.horizontal.height
      : COMPACT_HEIGHT[compactViewKey];
  const sideCompactWidth = compactViewKey === "rings" ? quotaSizes.vertical.width : 48;
  // 按整首最长歌词留出空间，换行时胶囊大小保持稳定。
  const musicWidthText = longestLyricText(lyricLines, config.fontFamily) || compactText(media, lyricNow);
  const compactOptions = {
    badges: badgeCounts,
    hasWeather: Boolean(weather),
    media,
    lyricLine: lyricNow,
    musicText: musicWidthText,
    fontScale,
    fontFamily: config.fontFamily,
    maxWidth: PANEL_W - compactQuotaExtent,
  };
  const sideCompactHeight = compactViewKey === "rings"
    ? quotaSizes.vertical.height
    : compactViewKey === "music"
      ? compactMusicHeightFor(compactOptions, MAX_PANEL_H - compactQuotaExtent) + compactQuotaExtent
      : 440;
  const compactContentWidth = showLiveCaption
    ? liveCaptionWidthFor(PANEL_W - compactQuotaExtent)
    : activity !== null
      ? activityWidthFor(activity, compactOptions)
      : compactViewBase === "rings"
        ? quotaSizes.horizontal.width
        : compactWidthFor(compactViewBase, compactOptions);
  const compactWidth = compactContentWidth + compactQuotaExtent;
  // ===== P4：主体尺寸（画布里的 body rect） =====
  const bodyW = open ? PANEL_W : isSideEdge(edge) ? sideCompactWidth : compactWidth;
  const bodyH = open ? panelHeight : isSideEdge(edge) ? sideCompactHeight : compactHeight;
  bodyTargetRef.current = { width: bodyW, height: bodyH, open };
  const baseBodyR = bodyRadius({ x: 0, y: 0, width: bodyW, height: bodyH });
  const cornerVars =
    edge === "top"
      ? { tl: 0, tr: 0, br: baseBodyR, bl: baseBodyR }
      : edge === "bottom"
        ? { tl: baseBodyR, tr: baseBodyR, br: 0, bl: 0 }
        : edge === "left"
          ? { tl: 0, tr: baseBodyR, br: baseBodyR, bl: 0 }
          : { tl: baseBodyR, tr: 0, br: 0, bl: baseBodyR };
  const islandStyle = {
    "--body-r-tl": `${cornerVars.tl}px`,
    "--body-r-tr": `${cornerVars.tr}px`,
    "--body-r-br": `${cornerVars.br}px`,
    "--body-r-bl": `${cornerVars.bl}px`,
    // 面板宽高由每个 tab 的常量表驱动（见 PANEL_HEIGHT）；收起态宽高由当前
    // 收起视图决定（rings 宽高按内容实测）。
    "--panel-w": `${PANEL_W}px`,
    "--panel-h": `${panelHeight}px`,
    "--compact-h": `${compactHeight}px`,
    "--side-compact-w": `${sideCompactWidth}px`,
    "--side-compact-h": `${sideCompactHeight}px`,
    "--quota-max-w": `${PANEL_W}px`,
    "--quota-max-h": `${MAX_PANEL_H}px`,
    "--compact-quota-cell": `${COMPACT_QUOTA_CELL}px`,
    "--compact-quota-extent": `${compactQuotaExtent}px`,
    "--compact-content-extent": `${side ? sideCompactHeight - compactQuotaExtent : compactContentWidth}px`,
    // 收起态圆角随视图高度变化（rings 48→24 / clock 34→17）；贴边侧的内凹角
    // 由 island.css 的 data-edge 规则覆盖成 2px + mask 凹陷。
    "--compact-r": `${Math.min(30, compactHeight / 2)}px`,
    width: `var(--liquid-w, ${bodyW}px)`,
    height: `var(--liquid-h, ${bodyH}px)`,
  } as CSSProperties;
  // 字号倍率挂在最外层，右键菜单 / toast 也一起缩放；面板和胶囊都继承。
  const rootFontStyle = {
    "--fs-scale": fontScale.toFixed(3),
    "--glass-transmission": (transparency / 90).toFixed(4),
  } as CSSProperties;
  /** 切换主题：写配置 + 立即生效；auto 时重置滞回计时，让采样重新判断。 */
  const applyTheme = (mode: IslandThemeMode) => {
    setThemeMode(mode);
    autoFlipSince.current = null;
    updateConfig({ theme: mode }, true);
    closeMenuRef.current();
  };

  /** 可穿透：先乐观更新本地，再叫 Rust 改窗口样式；失败回滚并 toast。 */
  const togglePassthrough = () => {
    const next = !Boolean(configRef.current?.passthrough);
    const current = configRef.current;
    if (current) {
      const merged = { ...current, passthrough: next };
      setConfig(merged);
      configRef.current = merged;
    }
    void islandSetPassthrough(next).catch((error) => {
      showToast(`设置穿透失败: ${String(error)}`);
      void getIslandState()
        .then((state) => {
          setConfig(state.config);
          configRef.current = state.config;
        })
        .catch(() => {});
    });
    // 穿透后鼠标不再落到岛上：面板必须收起，否则看着还在却点不动。
    // always 模式同样要收：此时窗口已穿透，再保持展开就是「看得见点不到」。
    if (next && openRef.current) closeNow();
    closeMenuRef.current();
  };

  /** 「显示板块」勾选：写 hiddenTabs（只有主页固定显示，不参与）。 */
  const toggleSection = (key: TabKey, visible: boolean) => {
    const current = configRef.current;
    if (!current) return;
    const next = visible
      ? current.hiddenTabs.filter((item) => item !== key)
      : Array.from(new Set([...current.hiddenTabs, key]));
    updateConfig({ hiddenTabs: next }, true);
  };

  /** 打开独立设置窗口：失败只 toast，不影响岛内其它交互 */
  const openSettingsWindow = () => {
    void openIslandSettingsWindow().catch((error) =>
      showToast(`打开设置窗口失败: ${String(error)}`)
    );
  };

  const renderPage = () => {
    switch (activeTab) {
      case "home":
        return (
          <HomePage
            usage={orderedUsage}
            weather={weather}
            media={media}
            muted={system.muted}
            config={config}
            expandMode={normalizeExpandMode(config.expandMode)}
            passthrough={Boolean(config.passthrough)}
            active={open && pageVisible}
            refreshing={refreshing}
            onRefresh={refreshAll}
            onToggleMute={() => {
              void toggleMute().then(setSystem).catch((e) => showToast(String(e)));
            }}
            onToggleAlways={() =>
              applyExpandMode(expandModeOf() === "always" ? "hover" : "always")
            }
            onTogglePassthrough={togglePassthrough}
            onMediaAction={(action) => {
              void mediaAction(action, media.appId)
                .then(() =>
                  getMedia()
                    .then((next) => next && handleMedia(next))
                    .catch(() => {})
                )
                .catch((e) => showToast(`媒体控制失败: ${String(e)}`));
            }}
            onOpenMain={() => void openMainWindow().catch((e) => showToast(String(e)))}
            onLaunch={(path) => void openPath(path).catch((e) => showToast(String(e)))}
            onAddLauncher={() => void addLauncherFiles()}
            onLauncherContextMenu={openLauncherMenu}
            liveOn={liveRunning}
            liveBusy={liveBusy}
            onToggleLive={toggleLive}
          />
        );
      case "msg":
        return (
          <MessagesPage
            notify={notify}
            config={config}
            onMarkReadAll={markNotifyAllRead}
            onMarkReadKind={markNotifyKindRead}
            onClear={clearNotifyKind}
            onModulesChange={changeNotifyModules}
            onOpenItem={(item) => openNotifyModule(item.kind, item.exe)}
            onOpenModule={openNotifyModule}
            onOpenSettings={() => {
              void openNotifySettings().catch(() => showToast("无法打开通知设置"));
            }}
          />
        );
      case "stock":
        return (
          <StocksPage
            stocks={stocks}
            config={config}
            onAdd={(code) => {
              // 用 ref 而不是本次渲染的 config：连续快速增删时不会拿旧列表覆盖。
              const current = configRef.current;
              if (!current) return;
              if (current.stocks.some((c) => sameStock(c, code))) {
                showToast("已在自选里");
                return;
              }
              updateConfig(
                { stocks: [...current.stocks.filter((c) => !sameStock(c, code)), code] },
                true
              );
              showToast("已添加自选");
              window.setTimeout(() => void refreshStocks(), 600);
            }}
            onRemove={(code) => {
              const current = configRef.current;
              if (!current) return;
              const next = current.stocks.filter((c) => !sameStock(c, code));
              if (next.length === current.stocks.length) {
                showToast("自选里没有这只");
                return;
              }
              updateConfig({ stocks: next }, true);
              showToast("已移除自选");
              window.setTimeout(() => void refreshStocks(), 350);
            }}
            onRefresh={() => void refreshStocks()}
          />
        );
      case "weather":
        return (
          <WeatherPage
            weather={weather}
            onRefresh={() => void refreshWeather()}
            onBind={(city) => {
              void bindWeatherCity({ name: city.name, lat: city.lat, lon: city.lon })
                .then(() => {
                  showToast(`已绑定「${city.label}」为主城市`);
                  return reloadWeather();
                })
                .catch((e) => showToast(String(e)));
            }}
            onUnbind={() => {
              void unbindWeatherCity()
                .then(() => {
                  showToast("已取消绑定，主城市跟随公网 IP");
                  return reloadWeather();
                })
                .catch((e) => showToast(String(e)));
            }}
            onRemoveCity={(city) => {
              void removeWeatherCity({ name: city.name, lat: city.lat, lon: city.lon })
                .then(() => {
                  showToast(`已删除「${city.label}」`);
                  return reloadWeather();
                })
                .catch((e) => showToast(String(e)));
            }}
            onAddCity={async (query) => {
              try {
                const message = await addWeatherCity(query);
                showToast(message);
                await reloadWeather();
                return message;
              } catch (e) {
                showToast(String(e));
                throw e;
              }
            }}
            onAddCityExact={async (city) => {
              try {
                const message = await addWeatherCityExact(city);
                showToast(message);
                await reloadWeather();
                return message;
              } catch (e) {
                showToast(String(e));
                throw e;
              }
            }}
          />
        );
      case "ctl":
        return (
          <ControlsPage
            system={system}
            active={open && pageVisible}
            onVolume={(v) => void setVolume(v).then(setSystem).catch((e) => showToast(String(e)))}
            onMute={() => void toggleMute().then(setSystem).catch((e) => showToast(String(e)))}
            onBrightness={(v) => void setBrightness(v).then(setSystem).catch((e) => showToast(String(e)))}
            onRadio={(kind, on) =>
              void toggleRadio(kind, on)
                .then(setSystem)
                .catch((e) => showToast(String(e)))
            }
            onOpenSettings={(kind) => void openSystemSettings(kind).catch(() => {})}
            onOpenMain={() => void openMainWindow().catch((e) => showToast(String(e)))}
            onConfigureClash={openSettingsWindow}
            onClashError={showToast}
          />
        );
      case "clip":
        return (
          <ClipboardPage
            items={clipboard}
            limit={clipLimit}
            onCopy={(id) => {
              void copyClipboard(id).catch((e) => showToast(String(e)));
            }}
            onPin={(id, pinned) => {
              void clipboardPin(id, pinned)
                .then(setClipboard)
                .catch((e) => showToast(String(e)));
            }}
            onDelete={(id) => {
              void clipboardDelete(id)
                .then(setClipboard)
                .catch((e) => showToast(String(e)));
            }}
            onClear={() => {
              void clearClipboard()
                .then(() => getClipboard().then(setClipboard).catch(() => {}))
                .catch((e) => showToast(String(e)));
            }}
            onLimit={(limit) => {
              void clipboardSetLimit(limit)
                .then((next) => {
                  setClipboard(next);
                  setClipLimit(limit);
                })
                .catch((e) => showToast(String(e)));
            }}
            onOcr={(item) => {
              // 图片转文字：走截图 OCR，识别结果直接写回剪贴板
              void ocrImage(item.image)
                .then((result) => {
                  if (!result.ok || !result.text.trim()) {
                    showToast(result.error || "没识别到文字");
                    return;
                  }
                  return copyText(result.text).then(() =>
                    showToast(`已识别 ${result.text.length} 字并复制`)
                  );
                })
                .catch((e) => showToast(String(e)));
            }}
            onToast={showToast}
          />
        );
      case "shot":
        return <ScreenshotPage onToast={showToast} />;
      case "translate":
        return (
          <TranslatePanel
            onOpenSettings={openSettingsWindow}
            liveStatus={liveStatus}
            liveCaptions={liveHistory}
            liveBusy={liveBusy}
            onToggleLive={toggleLive}
            onClearLive={clearLiveCaptions}
          />
        );
      case "tool":
        return (
          <ToolboxPage
            onToast={showToast}
            onOpenTab={(next) => setTab(next)}
            onOpenMainWindow={(page) =>
              void openMainWindow(page).catch((e) => showToast(String(e)))
            }
            onOpenPath={(path) =>
              void openPath(path).catch((e) => showToast(String(e)))
            }
          />
        );
      default:
        return null;
    }
  };

  // ===== 右键菜单 =====
  // 面板 640 宽、当前 tab 高度可能只有 ~380，菜单整体按面板尺寸夹住；
  // 子菜单（主题 / 展开方式 / 显示板块）超出右边界时翻到左侧展开。
  /**
   * P1-7：右键菜单坐标按「当前边 + 实际可见区域」夹边，不能浮在透明区。
   * - 展开态：按当前吸附边换算出面板在窗口里的矩形（bottom 时面板贴窗口底部），
   *   菜单整体夹在面板内；
   * - 收起态（可穿透菜单不再强制展开）：菜单贴着胶囊所在边摆放，并夹在窗口内。
   * 返回的是相对 `.island-root` 的坐标（菜单本身 position:absolute 在 root 内）。
   */
  const positionContextMenu = (
    clientX: number,
    clientY: number,
    expanded: boolean,
    menuHeight = WINDOWS_TAURI ? 416 : 372,
  ): { x: number; y: number } => {
    const MW = 196;
    const MH = menuHeight;
    const M = 4;
    const clamp = (value: number, min: number, max: number) =>
      Math.max(min, Math.min(max, Math.max(min, value)));
    const rootRect = rootRef.current?.getBoundingClientRect();
    const island = islandRef.current?.getBoundingClientRect();
    const rootLeft = rootRect?.left ?? 0;
    const rootTop = rootRect?.top ?? 0;
    const width = rootRect?.width ?? CANVAS_W;
    const height = rootRect?.height ?? CANVAS_H;
    const localX = clientX - rootLeft;
    const localY = clientY - rootTop;
    const body = island
      ? {
          left: island.left - rootLeft,
          top: island.top - rootTop,
          right: island.right - rootLeft,
          bottom: island.bottom - rootTop,
        }
      : {
          left: INSET,
          top: INSET,
          right: width - INSET,
          bottom: height - INSET,
        };
    if (expanded) {
      // P4：菜单夹在实际主体矩形内（展开面板 640×H，四边吸附均适用）
      return {
        x: clamp(localX, body.left + M, body.right - MW - M),
        y: clamp(localY, body.top + M, body.bottom - MH - M),
      };
    }
    // 收起态：贴着胶囊所在边弹出，并夹在画布内
    let left = clamp(localX, 8, width - MW - 8);
    let top = clamp(localY, 8, height - MH - 8);
    if (edge === "bottom") top = clamp(body.top - MH - 8, 8, height - MH - 8);
    else if (edge === "top") top = clamp(body.bottom + 8, 8, height - MH - 8);
    else if (edge === "left") left = clamp(body.right + 8, 8, width - MW - 8);
    else left = clamp(body.left - MW - 8, 8, width - MW - 8);
    return { x: left, y: top };
  };
  const menuLeft = contextMenu?.x ?? 120;
  const menuTop = contextMenu?.y ?? 60;
  const submenuSide =
    menuLeft + 188 + 4 + 158 > (rootRef.current?.clientWidth ?? CANVAS_W) - 8
      ? "context-submenu-left"
      : "";
  const enumThemeLabel = THEME_MODES.find((item) => item.key === themeMode)?.label ?? "";
  const enumModeLabel =
    EXPAND_MODES.find((item) => item.key === normalizeExpandMode(config.expandMode))?.label ?? "";
  const passthroughOn = Boolean(config.passthrough);
  const desktopRefractionOn = config.excludeFromCapture === true && transparency > 0;
  const visibleSections = SECTION_TABS.filter((item) => !hidden.has(item.key)).length;

  /**
   * 关闭右键菜单的唯一出口：普通菜单只关状态；若本次菜单来自可穿透事件，
   * 必须调用 island_passthrough_restore（失败静默，Rust 侧 10s 兜底）。
   */
  const closeContextMenu = () => {
    setContextMenu(null);
    setContextSubmenu(null);
    if (!passthroughMenuOpen.current) return;
    passthroughMenuOpen.current = false;
    if (passthroughMenuTimer.current != null) {
      window.clearTimeout(passthroughMenuTimer.current);
      passthroughMenuTimer.current = null;
    }
    void islandPassthroughRestore().catch(() => {});
    // 菜单打开期间可能被 hover / 事件展开过：回到打开前的状态，避免
    // restore 后留下一个透明但仍显示的展开面板。用户点过菜单项（例如
    // 「展开灵动岛」）时以动作结果为准，不回滚。
    if (!passthroughMenuAction.current && !passthroughMenuWasOpen.current && openRef.current) {
      closeNow();
    }
    passthroughMenuAction.current = false;
  };
  closeMenuRef.current = closeContextMenu;

  /**
   * 可穿透模式下 Rust 在光标处临时恢复可点击并广播 {x,y}：在这一坐标打开
   * 已有的右键菜单（client 坐标），并准备关闭时的 restore。
   */
  const openPassthroughMenu = (x: number, y: number) => {
    closeContextMenu(); // 上一轮 grace 先释放（幂等）
    // P1-6：不再为了菜单强制展开面板。可穿透下窗口临时可点击，菜单打开后
    // 若鼠标划过胶囊触发 hover 展开，关闭 restore 后会留下「展开但点不动」；
    // 因此记住打开前状态、菜单期间抑制 hover，关闭时恢复到打开前。
    passthroughMenuWasOpen.current = openRef.current;
    passthroughMenuAction.current = false;
    cancelHover();
    cancelCollapse();
    setContextSubmenu(null);
    setContextMenu(positionContextMenu(x, y, Boolean(openRef.current)));
    passthroughMenuOpen.current = true;
    if (passthroughMenuTimer.current != null) {
      window.clearTimeout(passthroughMenuTimer.current);
    }
    // Rust 侧 grace 只有 2s：前端略早于它主动关闭并 restore，避免菜单还画着
    // 但窗口已经重新透明、点不动。
    passthroughMenuTimer.current = window.setTimeout(() => closeContextMenu(), 1900);
  };
  passthroughMenuHandlerRef.current = (payload) => {
    const x = Number(payload?.x);
    const y = Number(payload?.y);
    if (!Number.isFinite(x) || !Number.isFinite(y)) return;
    openPassthroughMenu(x, y);
  };
  wheelEventHandlerRef.current = (payload) => {
    if (openRef.current) return; // P1-4：展开态直接忽略可穿透钩子的滚轮
    const delta = Number(payload?.delta);
    if (!delta) return;
    // Rust 契约：正 = 向上滚；DOM 里 deltaY > 0 是向下 = 下一个视图
    cycleCompactRef.current(delta > 0 ? -1 : 1);
  };

  const closeMenu = closeContextMenu;

  const addLauncherFiles = async () => {
    if (launcherPickerOpen.current) return;
    if (!IS_TAURI) {
      showToast("请在桌面应用中选择本地文件");
      return;
    }
    launcherPickerOpen.current = true;
    cancelCollapse();
    try {
      const { open: pickFiles } = await import("@tauri-apps/plugin-dialog");
      const paths = await pickFiles({ title: "选择应用或文件", multiple: true });
      if (!paths?.length || !configRef.current) return;

      const current = configRef.current.launcher;
      const seen = new Set(current.map((item) => item.path));
      const additions: LauncherItem[] = [];
      for (const path of paths) {
        if (seen.has(path)) continue;
        seen.add(path);
        additions.push({ name: path.split(/[\\/]/).pop() || path, path, icon: "note" });
      }
      const available = Math.max(0, 64 - current.length);
      if (additions.length > available) showToast("启动器最多保留 64 个快捷按钮");
      if (available && additions.length) {
        updateConfig({ launcher: [...current, ...additions.slice(0, available)] }, true);
      }
    } catch (error) {
      showToast(`无法添加快捷按钮：${String(error)}`);
    } finally {
      launcherPickerOpen.current = false;
    }
  };

  const openLauncherMenu = (item: LauncherItem, x: number, y: number) => {
    closeContextMenu();
    cancelCollapse();
    setContextMenu({ ...positionContextMenu(x, y, true, 44), launcher: item });
  };

  const removeLauncher = (item: LauncherItem) => {
    closeContextMenu();
    const current = configRef.current?.launcher;
    if (!current) return;
    const currentIndex = current.indexOf(item);
    const index = currentIndex >= 0 ? currentIndex : current.findIndex(
      (entry) => entry.name === item.name && entry.path === item.path,
    );
    if (index >= 0) {
      updateConfig({ launcher: current.filter((_, entryIndex) => entryIndex !== index) }, true);
    }
  };

  const launcherMenuItem = contextMenu?.launcher;

  const defaultContextItems = (
    <>
      <button
        type="button"
        onClick={() => {
          toggleOpen();
          closeMenu();
        }}
      >
        <Icon name={open ? "chevron" : "external"} size={13} />
        {open ? "收起灵动岛" : "展开灵动岛"}
      </button>
      <div className="context-divider" />

      {/* 主题 ▸ */}
      <div className="context-group">
        <button
          type="button"
          className={`context-parent ${contextSubmenu === "theme" ? "is-open" : ""}`}
          onPointerEnter={() => setContextSubmenu("theme")}
          onClick={() => setContextSubmenu((current) => (current === "theme" ? null : "theme"))}
        >
          <span className="context-check" />
          <Icon name="spark" size={13} /> 主题
          <span className="context-hint">{enumThemeLabel}</span>
          <Icon name="chevron" size={12} className="context-arrow" />
        </button>
        {contextSubmenu === "theme" ? (
          <div
            className={`context-submenu ${submenuSide}`}
            onPointerEnter={() => setContextSubmenu("theme")}
          >
            {THEME_MODES.map((item) => (
              <button
                key={item.key}
                type="button"
                className={themeMode === item.key ? "selected" : ""}
                onClick={() => applyTheme(item.key)}
              >
                <span className="context-check">
                  {themeMode === item.key ? <Icon name="check" size={11} /> : null}
                </span>
                {item.label}
              </button>
            ))}
          </div>
        ) : null}
      </div>

      {/* 展开方式 ▸ */}
      <div className="context-group">
        <button
          type="button"
          className={`context-parent ${contextSubmenu === "mode" ? "is-open" : ""}`}
          onPointerEnter={() => setContextSubmenu("mode")}
          onClick={() => setContextSubmenu((current) => (current === "mode" ? null : "mode"))}
        >
          <span className="context-check" />
          <Icon name="eye" size={13} /> 展开方式
          <span className="context-hint">{enumModeLabel}</span>
          <Icon name="chevron" size={12} className="context-arrow" />
        </button>
        {contextSubmenu === "mode" ? (
          <div
            className={`context-submenu ${submenuSide}`}
            onPointerEnter={() => setContextSubmenu("mode")}
          >
            {EXPAND_MODES.map((item) => (
              <button
                key={item.key}
                type="button"
                className={normalizeExpandMode(config.expandMode) === item.key ? "selected" : ""}
                onClick={() => {
                  applyExpandMode(item.key);
                  closeMenu();
                }}
              >
                <span className="context-check">
                  {normalizeExpandMode(config.expandMode) === item.key ? (
                    <Icon name="check" size={11} />
                  ) : null}
                </span>
                {item.label}
              </button>
            ))}
          </div>
        ) : null}
      </div>

      {/* 显示板块 ▸（勾选写 hiddenTabs；只有主页固定显示） */}
      <div className="context-group">
        <button
          type="button"
          className={`context-parent ${contextSubmenu === "tabs" ? "is-open" : ""}`}
          onPointerEnter={() => setContextSubmenu("tabs")}
          onClick={() => setContextSubmenu((current) => (current === "tabs" ? null : "tabs"))}
        >
          <span className="context-check" />
          <Icon name="toolbox" size={13} /> 显示板块
          <span className="context-hint">
            {visibleSections}/{SECTION_TABS.length}
          </span>
          <Icon name="chevron" size={12} className="context-arrow" />
        </button>
        {contextSubmenu === "tabs" ? (
          <div
            className={`context-submenu ${submenuSide}`}
            onPointerEnter={() => setContextSubmenu("tabs")}
          >
            {SECTION_TABS.map((item) => {
              const visible = !hidden.has(item.key);
              return (
                <button
                  key={item.key}
                  type="button"
                  className={visible ? "selected" : ""}
                  onClick={() => toggleSection(item.key, !visible)}
                >
                  <span className="context-check">
                    {visible ? <Icon name="check" size={11} /> : null}
                  </span>
                  {item.label}
                </button>
              );
            })}
          </div>
        ) : null}
      </div>

      <button
        type="button"
        className={`context-check-row ${passthroughOn ? "selected" : ""}`}
        onClick={togglePassthrough}
      >
        <span className="context-check">
          {passthroughOn ? <Icon name="check" size={11} /> : null}
        </span>
        <Icon name="monitor" size={13} /> 可穿透
      </button>

      <div className="context-divider" />
      {WINDOWS_TAURI && (
        <button
          type="button"
          aria-label="实时桌面折射"
          aria-describedby="desktop-refraction-help"
          aria-pressed={desktopRefractionOn}
          className={`context-refraction context-check-row ${desktopRefractionOn ? "selected" : ""}`}
          onClick={() => updateConfig(desktopRefractionOn
            ? { excludeFromCapture: false }
            : { excludeFromCapture: true, ...(transparency === 0 ? { transparency: 60 } : {}) }, true)}
        >
          <span className="context-check">
            {desktopRefractionOn ? <Icon name="check" size={11} /> : null}
          </span>
          <span className="context-refraction-copy">
            <span>实时桌面折射</span>
            <span id="desktop-refraction-help">启用时，灵动岛会从截图和录屏中隐藏</span>
          </span>
        </button>
      )}
      <div className="context-glass">
        <div className="context-glass-head">
          <span>玻璃透明度</span>
          <span>{transparency}%</span>
        </div>
        <Slider
          value={transparency}
          min={0}
          max={90}
          step={1}
          onChange={(value) => updateConfig({ transparency: value })}
          ariaLabel="玻璃透明度"
        />
      </div>
      <div className="context-divider" />
      <button
        type="button"
        onClick={() => {
          closeMenu();
          openSettingsWindow();
        }}
      >
        <Icon name="settings" size={13} /> 设置…
      </button>
      <button
        type="button"
        onClick={() => {
          refreshAll();
          closeMenu();
        }}
      >
        <Icon name="refresh" size={13} /> 刷新数据
      </button>
      <button
        type="button"
        onClick={() => {
          void openMainWindow();
          closeMenu();
        }}
      >
        <Icon name="external" size={13} /> 打开主窗口
      </button>
      <div className="context-divider" />
      <button
        type="button"
        onClick={() => {
          closeMenu();
          void setIslandEnabled(false);
        }}
      >
        <Icon name="eyeOff" size={13} /> 关闭灵动岛
      </button>
      <button type="button" className="danger" onClick={() => void quitApp()}>
        <Icon name="power" size={13} /> 退出 AgentHUD
      </button>
    </>
  );

  const contextItems = (
    <div
      className="island-context-menu"
      style={{ left: menuLeft, top: menuTop }}
      onPointerDown={(e) => e.stopPropagation()}
      onPointerEnter={() => cancelCollapse()}
      onClickCapture={() => {
        passthroughMenuAction.current = true;
      }}
      onClick={(e) => e.stopPropagation()}
    >
      {launcherMenuItem ? (
        <button type="button" className="danger" onClick={() => removeLauncher(launcherMenuItem)}>
          <Icon name="trash" size={13} /> 移除快捷按钮
        </button>
      ) : defaultContextItems}
    </div>
  );

  // 收起态冻结整岛 1s 时钟（内存压降专项 3b）：胶囊不消费 now，展开解冻
  // 时 NowProvider 会立即重新同步，倒计时/相对时间无陈旧残留
  return (
    <NowProvider frozen={!open}>
      <div
        ref={rootRef}
        className={`island-root edge-${edge} ${open ? "is-open" : "is-collapsed"} ${
          dragging ? "is-dragging" : ""
        } ${config.breathEnabled === false ? "no-breath" : ""}`}
        style={rootFontStyle}
        data-theme={themePalette}
        data-theme-mode={themeMode}
        data-native-glass={nativeGlassActive ? "" : undefined}
        data-edge={edge}
        onPointerDownCapture={handlePointerDownCapture}
        onPointerEnter={() => {
          if (dragActiveRef.current || dragStartingRef.current) return;
          if (openRef.current) cancelCollapse();
        }}
        onPointerLeave={() => {
          if (!dragActiveRef.current && !pointerRef.current) blockHoverUntilLeave.current = false;
          if (dragActiveRef.current || dragStartingRef.current) return;
          if (passthroughMenuOpen.current) return;
          collapse();
        }}
        onDoubleClick={(event) => {
          const target = event.target as HTMLElement | null;
          if (!target?.closest(".island, .liquid-glass")) return;
          handleIslandDoubleClick(event);
        }}
        onContextMenu={(e) => {
          const target = e.target as HTMLElement | null;
          if (!target?.closest(".island, .liquid-glass")) return;
          if (target.closest("input, textarea, [contenteditable='true']")) {
            return;
          }
          if (dragActiveRef.current || dragStartingRef.current) return;
          e.preventDefault();
          closeMenuRef.current();
          cancelHover();
          cancelCollapse();
          setContextSubmenu(null);
          setContextMenu(positionContextMenu(e.clientX, e.clientY, openRef.current));
        }}
      >
        <div className="liquid-stage">
        {/* P4 单一外轮廓：玻璃层按同一 path 裁剪，SVG 描边用同一 path；
            内层不再有 border/阴影，避免出现内部接缝线。 */}
        <div
          className="liquid-glass"
          ref={liquidGlassRef}
          aria-hidden="true"
          onPointerDown={beginPointerDrag}
          onPointerMove={(event) => {
            updateHoverPoke(event.clientX, event.clientY);
            if (event.buttons === 0) scheduleExpand();
          }}
          onPointerEnter={(event) => {
            if (openRef.current) cancelCollapse();
            else if (event.buttons === 0) scheduleExpand();
          }}
          onPointerLeave={() => {
            if (!dragActiveRef.current && !dragStartingRef.current) {
              releasePoke();
              cancelHover();
            }
          }}
        >
          <div className="liquid-backdrops">
            <canvas ref={backdropCanvasRef} className="glass-refraction" />
            <img ref={backdropFallbackRef} className="island-backdrop" alt="" />
          </div>
          <span className="liquid-tint" />
          <span className="liquid-gloss" />
        </div>
        <svg
          className="liquid-outline"
          viewBox={`0 0 ${CANVAS_W} ${CANVAS_H}`}
          width={CANVAS_W}
          height={CANVAS_H}
          aria-hidden="true"
        >
          <defs>
            <linearGradient ref={liquidRimGradientRef} id="liquid-rim-gradient"
              gradientUnits="userSpaceOnUse" x1="0" x2="0" y1="0" y2="1">
              <stop className="liquid-rim-top" offset="0" />
              <stop className="liquid-rim-bottom" offset="1" />
            </linearGradient>
            <radialGradient ref={liquidFlowGradientRef} id="liquid-flow-gradient"
              gradientUnits="userSpaceOnUse" cx="0" cy="0" r="60">
              <stop className="liquid-flow-center" offset="0" />
              <stop stopColor="white" stopOpacity="0" offset="1" />
            </radialGradient>
          </defs>
          <path ref={liquidRimRef} className="liquid-stroke-rim" d="" />
          <path ref={liquidFlowRef} className="liquid-stroke-flow" d="" />
        </svg>
        <div className="liquid-content-clip">
        <div
          ref={bodyFrameRef}
          className="liquid-body"
        >
        <div
          ref={islandRef}
          className={`island ${open ? "open" : "collapsed"}`}
          style={islandStyle}
          onPointerDown={beginPointerDrag}
          onPointerMove={(event) => {
            trackCompactPointer(event.clientX, event.clientY);
            updateHoverPoke(event.clientX, event.clientY);
            if ((event.target as Element).closest(".compact-play")) cancelHover();
            else if (event.buttons === 0) scheduleExpand();
          }}
          onClickCapture={(e) => {
            if (!suppressClickRef.current || e.detail === 0) return;
            e.preventDefault();
            e.stopPropagation();
          }}
          onPointerEnter={(event) => {
            trackCompactPointer(event.clientX, event.clientY);
            updateHoverPoke(event.clientX, event.clientY);
            if (dragActiveRef.current || dragStartingRef.current) return;
            if (openRef.current) cancelCollapse();
            else if (event.buttons === 0 && !(event.target as Element).closest(".compact-play")) {
              scheduleExpand();
            }
          }}
          onPointerLeave={() => {
            compactPointerRef.current = { x: 9999, y: 9999, inside: false };
            if (dragActiveRef.current || dragStartingRef.current) return;
            setCompactPressed(false);
            releasePoke();
            cancelHover();
            collapse();
          }}
        >
          {/* 新消息贴边呼吸灯：绝对定位在胶囊内侧，pointer-events:none，
              不参与布局也不改变 .island 外接矩形（hover 热区仍按胶囊本体上报）。
              亮灭与颜色由 handleNotify 的 rAF 写 opacity / --notify-pulse-color。 */}
          <span className="notify-pulse" ref={notifyPulseRef} aria-hidden="true" />
          <div className="pill-wrap" aria-hidden={open} inert={open}>
            {showLiveCaption || activity || compactViewBase !== "rings" ? (
                <CompactViewSurface
                  view={compactViewBase}
                  activity={activity}
                  media={media}
                  lyricLine={lyricNow}
                  lyricKey={lyricKey}
                  weather={weather}
                  badges={badgeCounts}
                  live={liveCaptionLayer}
                  fontScale={fontScale}
                  quota={showCompactQuotas ? (
                    <div className="compact-quotas" role="group" aria-label="额度环">
                      {compactQuotaAgents.length ? compactQuotaAgents.map((agent) => (
                        <PillAgent
                          key={agent.id}
                          agent={agent}
                          plan=""
                          periods={ringPeriods(agent.quotas)}
                          ringsOnly
                          dynamicColor={config.quotaRingStyle === "dynamic"}
                        />
                      )) : (
                        <span className="pill-agent-empty">额度查询中</span>
                      )}
                    </div>
                  ) : undefined}
                  vertical={side}
                  active={pageVisible && !open}
                  pressed={compactPressed}
                  surpriseToken={surpriseToken}
                  pointerRef={compactPointerRef}
                  audioLevelRef={liquidAudioLevelRef}
                  onTogglePlay={() => {
                    setInteractSeq((value) => value + 1);
                    void mediaAction("toggle", media.appId)
                      .then(() =>
                        getMedia()
                          .then((next) => next && handleMedia(next))
                          .catch(() => {})
                      )
                      .catch((error) => showToast(`媒体控制失败: ${String(error)}`));
                  }}
                  onActivate={handlePillClick}
                />
              ) : (
                <CollapsedPill
                  config={config}
                  usage={orderedUsage}
                  todayUsage={todayUsage}
                  weather={weather}
                  system={system}
                  meeting={meetingBrief}
                  unread={badgeCounts}
                  edge={edge}
                  onOpen={handlePillClick}
                  onResize={resizeQuota}
                />
              )}
          </div>
          {/* 面板子树：收起 10 分钟后卸载、展开入口预挂（内存压降专项 3a，
              见 PANEL_UNMOUNT_DELAY_MS 注释；胶囊 pill-wrap 常驻不卸载） */}
          {panelMounted ? (
          <div className="island-panel" aria-hidden={!open}>
            <nav ref={navRef} className="island-nav" title={edge === "bottom" ? "按住底部空白处拖动 HUD" : "按住顶部空白处拖动 HUD"}>
              {tabs.map((item) => (
                <button
                  key={item.key}
                  type="button"
                  className={`nav-tab ${activeTab === item.key ? "active" : ""}`}
                  onClick={() => setTab(item.key)}
                  title={item.label}
                >
                  <Icon name={item.icon} size={15} />
                  <span className="nav-label">{item.label}</span>
                  {item.key === "msg" && notify.unread > 0 ? (
                    <span className="nav-badge">{notify.unread > 9 ? "9+" : notify.unread}</span>
                  ) : null}
                </button>
              ))}
              <span className="nav-spacer" />
              {/* 齿轮 = 打开独立设置窗口（island-settings，1100×720 自绘标题栏
                  大窗）。设置项已经从岛内迁出，齿轮不再是岛内标签，故没有
                  active 态，也不改变当前标签；失败只 toast 不打断岛内交互。 */}
              <button
                type="button"
                className="nav-tab gear"
                onClick={openSettingsWindow}
                title="灵动岛设置"
                aria-label="打开灵动岛设置窗口"
              >
                <Icon name="settings" size={15} />
              </button>
            </nav>
            <div ref={contentRef} className={`island-content tab-${activeTab}`}>
              {/* lazy 页（截图 / 工具箱）的 Suspense 边界：chunk 毫秒级加载 +
                  空闲期预取都靠它兜底，fallback 保持空占位不引入额外样式 */}
              <Suspense fallback={null}>{renderPage()}</Suspense>
            </div>
          </div>
          ) : null}
        </div>
        </div>
        </div>
        </div>
        {contextMenu ? (
          <>
            <div className="context-backdrop" onClick={closeContextMenu} />
            {contextItems}
          </>
        ) : null}
        {toast ? <div className="island-toast">{toast}</div> : null}
      </div>
    </NowProvider>
  );
}

function fmtTokens(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0";
  // 顶部横排同样要自动升级单位，否则 1.3B 会一直显示成 1341.4M
  if (value >= 1_000_000_000) {
    const billions = value / 1_000_000_000;
    // 和竖排 fmtTokensSide 保持同样的精度（1.33B，而不是 1.3B）
    return `${billions >= 100 ? Math.round(billions) : Number(billions.toFixed(2))}B`;
  }
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(value >= 10_000 ? 0 : 1)}K`;
  return String(Math.round(value));
}

/** 竖排花费：减少小数位，把字号让出来；百元/千元级直接取整。 */
function fmtCostSide(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0.00";
  if (value >= 100) return String(Math.round(value));
  if (value >= 10) return value.toFixed(1);
  return value.toFixed(2);
}
/** 竖排 48px 胶囊专用的更短 token：1091.1M → 1091M、1.234B → 1.23B。
 *  保留 M/B/K 单位语义，靠减少小数位给字号让空间。 */
function fmtTokensSide(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0";
  const trim = (n: number, digits: number) =>
    n.toFixed(digits).replace(/\.?0+$/, "");  if (value >= 1_000_000_000) return `${trim(value / 1_000_000_000, 2)}B`;
  if (value >= 1_000_000) {
    const millions = value / 1_000_000;
    return `${millions >= 100 ? Math.round(millions) : trim(millions, 1)}M`;
  }
  if (value >= 1_000) {
    const thousands = value / 1_000;
    return `${thousands >= 100 ? Math.round(thousands) : trim(thousands, 1)}K`;
  }
  return String(Math.round(value));
}

interface CollapsedPillProps {
  config: IslandConfig;
  usage: HudBarUsage | null;
  todayUsage: TodayUsageTotals | null;
  weather: WeatherState | null;
  system: SystemState;
  meeting: MeetingSessionBrief | null;
  /** 各消息模块未读数：activity 结束后常驻在胶囊右侧的品牌图标来源 */
  unread: BadgeCounts;
  edge: IslandEdge;
  onOpen: (event: ReactMouseEvent<HTMLButtonElement>) => void;
  onResize: (orientation: keyof IslandDragSizes, size: IslandDragSizes[keyof IslandDragSizes]) => void;
}

function CollapsedPill({ edge, ...props }: CollapsedPillProps) {
  const side = isSideEdge(edge);
  return (
    <>
      <QuotaPill {...props} side={false} probe={side} />
      <QuotaPill {...props} side probe={!side} />
    </>
  );
}

function QuotaPill({
  config,
  usage,
  todayUsage,
  weather,
  system,
  meeting,
  unread,
  side,
  probe,
  onOpen,
  onResize,
}: Omit<CollapsedPillProps, "edge"> & { side: boolean; probe: boolean }) {
  const ringsOnly = config.quotaDisplay === "rings";
  // 纯额度环的配色：dynamic 按该窗口已用阈值动态绿黄红，否则按周期固定分色。
  const dynamicColor = config.quotaRingStyle === "dynamic";
  const entries = (usage?.agents ?? []).map((agent) => ({
    agent,
    plan: planTag(agent),
    periods: ringPeriods(agent.quotas),
  }));
  const unreadEntries = badgeList(unread);
  // 会议录音进行中的胶囊指示：闪烁红点 + mm:ss 计时（暂停时红点常亮不闪）。
  // 与工具页 meet-tab-live 同色（#ff5d6c）同动画（meet-pulse）。
  const meetIndicator = meeting ? (
    <span
      className="pill-meet"
      title={meeting.paused ? "会议录音已暂停" : "会议录音进行中"}
    >
      <span className={`pill-meet-dot ${meeting.paused ? "is-paused" : ""}`} />
      <span className="pill-meet-time">{fmtMeetClock(meeting.effective_ms)}</span>
    </span>
  ) : null;
  const pillRef = useRef<HTMLButtonElement | null>(null);
  const agentsRef = useRef<HTMLSpanElement | null>(null);
  const showPlans = entries.some(({ plan }) => Boolean(plan));
  // 温度单位跟随数据源（imperial → °F），其余按摄氏度 °。
  const unit = tempUnitSuffix(weather);
  const weatherCity = weather?.city ?? config.weatherCity;
  const todayLabel = todayUsage
    ? `${todayUsage.currency === "USD" ? "$" : "¥"}${
        side ? fmtCostSide(todayUsage.cost) : todayUsage.cost.toFixed(2)
      }`
    : "--";
  const tokensLabel = todayUsage
    ? side
      ? fmtTokensSide(todayUsage.tokens)
      : fmtTokens(todayUsage.tokens)
    : "--";

  useLayoutEffect(() => {
    const pill = pillRef.current;
    const agentList = agentsRef.current;
    if (!pill || !agentList) return;
    const rings = agentList.querySelectorAll<HTMLElement>(".pill-ring");
    const fixed = Array.from(pill.children).filter((el) => el !== agentList);
    const px = (value: string) => parseFloat(value) || 0;
    const sync = () => {
      if (side) {
        const style = getComputedStyle(pill);
        const fixedHeight = fixed.reduce((total, item) => total + px(getComputedStyle(item).height), 0)
          + px(style.paddingTop) + px(style.paddingBottom) + fixed.length * px(style.rowGap);
        const ringHeight = Math.max(1, ...Array.from(rings, (ring) => px(getComputedStyle(ring).height)));
        const rowGap = px(getComputedStyle(agentList).rowGap);
        const rows = Math.max(1, Math.min(
          rings.length,
          Math.floor((MAX_PANEL_H - Math.ceil(fixedHeight) + rowGap) / (ringHeight + rowGap)),
        ));
        const next = String(rows);
        if (agentList.style.getPropertyValue("--quota-side-rows") !== next) {
          agentList.style.setProperty("--quota-side-rows", next);
        }
      }
      // 用布局尺寸上报，避免贴边动画的临时缩放影响目标宽高。
      const style = getComputedStyle(pill);
      onResize(side ? "vertical" : "horizontal", {
        width: Math.ceil(px(style.width)),
        height: Math.ceil(px(style.height)),
      });
    };
    sync();
    const observer = new ResizeObserver(sync);
    observer.observe(pill);
    if (side) {
      fixed.forEach((item) => observer.observe(item));
      rings.forEach((ring) => observer.observe(ring));
    }
    return () => observer.disconnect();
  }, [side, usage, ringsOnly, onResize]);

  return (
    <button
      type="button"
      ref={pillRef}
      className={`pill quota-presentation ${side ? "is-vertical" : ""} ${probe ? "is-size-probe" : "pill-quota"}`}
      aria-hidden={probe || undefined}
      inert={probe}
      onClick={probe ? undefined : onOpen}
    >
      <span className="pill-weather">
        <span className="pill-weather-current">
          <WeatherGlyph icon={weather?.icon ?? "cloudy"} size={side ? 22 : 24} />
          <span className="pill-weather-temp">{weather ? `${Math.round(weather.temp)}${unit}` : "--°"}</span>
        </span>
        {weather?.stale ? (
          <span
            className="pill-weather-stale"
            title={`更新失败，显示 ${fmtClock(weather.updatedAt)} 数据${weather.errorBrief ? `（${weather.errorBrief}）` : ""}`}
          >
            !
          </span>
        ) : null}
        {weatherCity && <span className="pill-weather-city" title={weatherCity}>{weatherCity}</span>}
      </span>
      <span className="pill-divider" />
      <span className={`pill-agents ${showPlans ? "has-plans" : ""}`} ref={agentsRef}>
        {entries.length === 0 ? (
          <span className="pill-agent-empty">
            <span className="pill-pulse" /> 额度查询中
          </span>
        ) : (
          entries.map(({ agent, plan, periods }) => (
            <PillAgent
              key={agent.id}
              agent={agent}
              plan={plan}
              periods={periods}
              ringsOnly={ringsOnly}
              dynamicColor={dynamicColor}
            />
          ))
        )}
      </span>
      {/* 用量块前的竖线：与天气后那条同参数（1px × 18px、沿用 .pill 的 7px
          间距、align-items: center 垂直居中）。竖排 48px 宽没有横向空间，
          直接不渲染，竖排结构保持原样。 */}
      {side ? null : <span className="pill-divider" aria-hidden="true" />}
      <span className="pill-usage">
        <span className="pill-usage-tokens">{tokensLabel}</span>
        <span className="pill-usage-cost">≈{todayLabel}</span>
      </span>
      {/* 消息簇前竖线：同样复用 .pill-divider，作为 .pill-usage 的兄弟节点
          放在 .pill-tail 之前，间距走 .pill 的 7px（不再受 .pill-tail 的
          4px gap 影响）。竖排由 CSS 隐藏未读簇，这里也不渲染。 */}
      {!side && unreadEntries.length ? (
        <span className="pill-divider" aria-hidden="true" />
      ) : null}
      <span className="pill-tail">
        {meetIndicator}
        {system.muted ? <Icon name="volumeOff" size={12} /> : null}
        {/* 未读消息模块：品牌图标常驻到对应模块已读；activity 期间由
            CompactViewSurface 接管胶囊，这里天然不渲染。竖排 48px 宽由 CSS
            隐藏未读簇；没有未读时未读簇和它前面那条线都不存在。 */}
        {unreadEntries.length ? (
          <span className="pill-unread" aria-label={`${unreadEntries.length} 类未读消息`}>
            {unreadEntries.map(({ kind, color }) => (
              <span
                key={kind}
                className="pill-unread-item"
                title={`${NOTIFY_BRAND_LABELS[kind] ?? kind} · ${unread[kind]} 条未读`}
              >
                {NOTIFY_BRAND_ICONS[kind] ? (
                  <NotifyBrandIcon kind={kind} size={20} />
                ) : (
                  <i className="pill-unread-dot" style={{ background: color }} />
                )}
              </span>
            ))}
          </span>
        ) : null}
      </span>
    </button>
  );
}

const RING_PERIODS = [
  { key: "hour5", suffix: "5h", color: "var(--ok)" },
  { key: "weekly", suffix: "周", color: "var(--quota-ring-week)" },
  { key: "monthly", suffix: "月", color: "var(--quota-ring-month)" },
] as const;

const RING_RADII = [18, 13.5, 9];

function ringPeriods(quotas: HudBarQuota[]) {
  const valid = quotas.filter(({ usedPct }) => Number.isFinite(usedPct));
  return RING_PERIODS.flatMap(({ key, suffix, color }) => {
    const quota =
      valid.find((item) => item.key === key) ??
      (key === "hour5" ? valid.find((item) => item.key === "interval") : undefined);
    if (!quota) return [];
    const remaining = remainingPct(quota);
    return [{ quota, key, suffix, color, remaining, text: `${Math.round(remaining)}% ${suffix}` }];
  });
}

/** 环下标：订阅档位。OpenCode Go 的档位就是产品名；DSH 没有订阅，
 *  只有开放平台余额；其它无 plan 的余额型条目标「余额」。 */
function planTag(agent: HudBarAgent): string {
  const plan = agent.plan?.trim();
  if (plan) return plan;
  if (agent.source === "opencodego") return "Go";
  if (agent.source === "dsh" || agent.balance) return "余额";
  return "";
}

function PillAgent({ agent, plan, periods, ringsOnly, dynamicColor }: {
  agent: HudBarAgent;
  plan: string;
  periods: ReturnType<typeof ringPeriods>;
  ringsOnly: boolean;
  dynamicColor: boolean;
}) {
  const quota =
    periods[0]?.quota ?? agent.quotas.find(({ usedPct }) => Number.isFinite(usedPct));
  const used = quota ? Math.max(0, Math.min(100, quota.usedPct)) : 0;
  // 仅可用周期占用半径；弧长统一表示「已用」百分比（与旧版语义一致），
  // 配色按配置在固定周期色与动态额度色之间切换。
  const tracks = ringsOnly
    ? periods.map(({ key, color, quota: periodQuota }, index) => ({
        key,
        color: dynamicColor ? `var(--${quotaTone(periodQuota)})` : color,
        progress: Math.max(0, Math.min(100, periodQuota.usedPct)),
        radius: RING_RADII[index],
        strokeWidth: 2.6,
      }))
    : quota ? [{
        key: "primary",
        color: `var(--${quotaTone(quota)})`,
        progress: used,
        radius: 17.2,
        strokeWidth: 3.7,
      }] : [];
  const balance = agent.balance;
  const offline = !quota && !balance && agent.status === "unavailable";
  const ariaBits: string[] = [agentLabel(agent.source)];
  if (agent.account) ariaBits.push(`· ${agent.account}`);
  if (agent.plan) ariaBits.push(`(${agent.plan})`);
  if (periods.length) {
    ariaBits.push(`剩余 ${periods.map(({ text }) => text).join("、")}`);
  } else if (quota) {
    ariaBits.push(quota.labelZh);
  } else if (balance) {
    ariaBits.push(`余额 ${fmtMoney(balance.amount, balance.currency)}`);
  } else {
    ariaBits.push(offline ? "额度暂不可用" : "暂无额度");
  }
  const ariaLabel = ariaBits.join(" ");

  return (
    <span
      className={`pill-ring ${offline ? "is-offline" : ""} ${
        balance && !quota ? "is-balance" : ""
      }`}
      aria-label={ariaLabel}
      title={ariaLabel}
    >
      <span className="pill-ring-main" aria-hidden="true">
        <span className={ringsOnly ? "pill-ring-ball is-concentric" : "pill-ring-ball"}>
          <svg viewBox="0 0 42 42">
            {tracks.map(({ key, color, progress, radius, strokeWidth }) => (
              <g key={key} data-quota-period={ringsOnly ? key : undefined}>
                <circle
                  cx="21"
                  cy="21"
                  r={radius}
                  fill="none"
                  stroke="var(--ring-track)"
                  strokeWidth={strokeWidth}
                />
                {progress > 0 ? (
                  <circle
                    className="pill-ring-arc"
                    cx="21"
                    cy="21"
                    r={radius}
                    fill="none"
                    stroke={color}
                    strokeWidth={strokeWidth}
                    strokeLinecap="round"
                    pathLength={100}
                    strokeDasharray={`${progress} 100`}
                    transform="rotate(-90 21 21)"
                  />
                ) : null}
              </g>
            ))}
            {!ringsOnly && !quota && (
              <circle
                cx="21"
                cy="21"
                r="17.2"
                fill="none"
                stroke="var(--accent)"
                strokeOpacity={balance ? 0.62 : 0.26}
                strokeWidth={balance ? 2.4 : 2}
                strokeLinecap="round"
                strokeDasharray={balance ? "1.5 5.2" : undefined}
                transform="rotate(-90 21 21)"
              />
            )}
          </svg>
          <span className="pill-ring-icon">
            <BrandMark source={agent.source} size={ringsOnly && periods.length === 3 ? 10 : 13} />
          </span>
        </span>
        {!ringsOnly && periods.length > 0 ? (
          <span className="pill-ring-periods">
            {periods.map(({ suffix, text }) => (
              <span key={suffix} className="pill-ring-pct">{text}</span>
            ))}
          </span>
        ) : null}
      </span>
      {plan ? <span className="pill-ring-plan" aria-hidden="true">{plan}</span> : null}
    </span>
  );
}

export { IslandApp };
