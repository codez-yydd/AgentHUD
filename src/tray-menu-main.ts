/**
 * 托盘右键菜单壳（tray-menu，仅 macOS）。
 *
 * 零 React 轻壳（照 session-hud-main.ts 的架构哲学）：菜单是静态少量
 * 单项 + 一块用量面板，无需 React 运行时。职责：
 * - 状态流：启动与每次窗口重新显示时（window focus 事件——Rust 侧
 *   show_tray_menu 在 show 后必调 set_focus，focus 事件必达）invoke
 *   tray_menu_state（轻量文件读）拉版本 / HUD 开关 / 币种；用量页的
 *   汇总与趋势走 tray_menu_usage（后端 async + spawn_blocking，一次
 *   打开 7 个来源库）。拉取失败保持当前显示（防御式，与 pet-main 同
 *   风格）。
 * - 动作流：快捷入口与底栏 invoke tray_menu_action（open-main /
 *   open-panel / hud-toggle / quit），后端执行后统一 hide 菜单窗口；
 *   开机自启直接走官方 autostart 插件（本地 LaunchAgent/注册表，无需
 *   自定义命令）；Esc 直接 getCurrentWindow().hide() 兜底（失焦自动
 *   收起由 Rust 侧完成）。
 * - 用量页：档位（当天 / 7 天 / 30 天 / 全部）每次显示默认回到「当天」
 *   （不落 localStorage，打开期间可自由切换）；指标（Tokens / 请求 /
 *   成本）仍记忆在 localStorage。趋势图为纯 DOM/CSS 柱状图（不引入图表
 *   库），空桶补齐、超过 31 根自动合并相邻桶；悬停显示 tooltip。
 * - 语言/主题同步：轻量双语词典（zh/en），语言偏好读主面板写入的
 *   localStorage 键（同源 WebView 共享；缺失/异常回退 zh），不引入
 *   i18n 运行时；主题读 zbar-theme 切 <html>.dark（仅 "dark" 视为暗色，
 *   对齐 appearance.ts loadTheme 语义）。两者启动各读一次，随后监听
 *   zbar://appearance-changed（APPEARANCE_CHANGED_EVENT，主面板
 *   setLocale/applyTheme 发出，无 payload）即时跟随：语言变化全量重
 *   渲染文案，主题变化纯 CSS 生效无需重渲染。
 * - 菜单透明度：快捷页「透明度」滑块调节窗口底色不透明度（40%~100%，
 *   只调 --m-bg-alpha 玻璃底、文字不受影响），拖动即时生效、松手写
 *   localStorage（zbar-tray-menu-alpha，读写在 appearance.ts），亮/暗
 *   两套缺省值分别在 tray-menu.html 的 --m-bg-alpha 里，用户值落成
 *   #menu-root 的内联变量压住两者。
 *
 * 取舍说明：菜单窗口每次显示都要最新数据（用量与 HUD 开关态随时在变），
 * 后端 show 前没有专门的"即将显示"事件，改用 window focus——set_focus
 * 是 show_tray_menu 的固定收尾动作，比 visibilitychange 更可靠（Tauri
 * hide/show 不一定派发 visibilitychange）。每次 focus 清空用量缓存并
 * 强制重取当前档位；打开菜单期间每 60s 静默刷新一次。
 *
 * 尺寸栅格：Rust 侧窗口 340×600（逻辑 px），页面内容栅格见
 * tray-menu.html 顶部注释（两端账目一一对应：头部 54、标签页 42、内容
 * 区自适应、底栏 90；小屏由 Rust 缩窗，内容区滚动、底栏常驻）。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import {
  APPEARANCE_CHANGED_EVENT,
  THEME_KEY,
  applyTrayMenuAlpha,
  loadTrayMenuAlpha,
  persistTrayMenuAlpha,
} from "./appearance";
import { LOCALE_KEY } from "./i18n/locale";

// ===== 后端契约（与 src-tauri/src/tray_menu.rs 同构，camelCase） =====

/** tray_menu_state 返回：轻量状态 */
interface TrayMenuState {
  version: string;
  hudBarEnabled: boolean;
  isUsd: boolean;
}

/** 汇总四格的数值（costUsd / costCny 二选一展示） */
interface TrayUsageSummary {
  requests: number;
  costUsd: number;
  costCny: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  totalTokens: number;
  cacheHitRate: number | null;
}

/** 趋势桶：bucket 为本地时间刻度 `YYYY-MM-DDTHH` / `YYYY-MM-DD` */
interface TrayUsagePoint {
  bucket: string;
  requests: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  tokens: number;
  costUsd: number;
  costCny: number;
}

/** tray_menu_usage 返回 */
interface TrayMenuUsage {
  range: string;
  fromMs: number;
  toMs: number;
  bucket: "hour" | "day";
  isUsd: boolean;
  summary: TrayUsageSummary;
  trend: TrayUsagePoint[];
  failedSources: string[];
}

// ===== 文案 =====

type Tab = "actions" | "usage";
type RangeKey = "today" | "7d" | "30d" | "all";
type MetricKey = "tokens" | "requests" | "cost";
type Locale = "zh" | "en";

const TABS: readonly Tab[] = ["actions", "usage"];
const METRICS: readonly MetricKey[] = ["tokens", "requests", "cost"];

/** 词典行结构（以 zh 为基准，en 必须同构） */
interface Msg {
  tabActions: string;
  tabUsage: string;
  quickActions: string;
  general: string;
  openMain: string;
  openPanel: string;
  logs: string;
  pricing: string;
  settings: string;
  launchAtLogin: string;
  opacity: string;
  about: string;
  hudBar: string;
  quit: string;
  ranges: Record<RangeKey, string>;
  metrics: Record<MetricKey, string>;
  totalCost: string;
  totalRequests: string;
  tokens: string;
  cacheHit: string;
  costDetail: string;
  requestsDetail: string;
  input: string;
  output: string;
  cacheWrite: string;
  cacheRead: string;
  trend: string;
  noData: string;
  noDataSub: string;
  errorTitle: string;
  errorSub: string;
  retry: string;
  partial: string;
  settingsTitle: string;
}

/** 轻量双语词典（仅菜单内文案；语言偏好与主面板同键共享） */
const MESSAGES: Record<Locale, Msg> = {
  zh: {
    tabActions: "快捷",
    tabUsage: "用量",
    quickActions: "快捷操作",
    general: "通用",
    openMain: "打开主窗口",
    openPanel: "打开悬浮面板",
    logs: "请求日志",
    pricing: "价格设置",
    settings: "设置",
    launchAtLogin: "开机自启",
    opacity: "透明度",
    about: "本地优先 · 多 Agent 用量监控",
    hudBar: "灵动岛",
    quit: "退出",
    ranges: { today: "当天", "7d": "近 7 天", "30d": "近 30 天", all: "全部" },
    metrics: { tokens: "Tokens", requests: "请求", cost: "成本" },
    totalCost: "总成本",
    totalRequests: "总请求数",
    tokens: "真实消耗 Tokens",
    cacheHit: "缓存命中率",
    costDetail: "按本机价格折算",
    requestsDetail: "本机 + 同步设备",
    input: "输入",
    output: "输出",
    cacheWrite: "创建",
    cacheRead: "命中",
    trend: "使用趋势",
    noData: "暂无用量数据",
    noDataSub: "使用 Agent 后，这里会出现本机请求统计。",
    errorTitle: "读取用量失败",
    errorSub: "稍后重试，或打开主窗口查看请求日志。",
    retry: "重试",
    partial: "部分来源读取失败",
    settingsTitle: "设置",
  },
  en: {
    tabActions: "Actions",
    tabUsage: "Usage",
    quickActions: "Quick actions",
    general: "General",
    openMain: "Open main window",
    openPanel: "Open floating panel",
    logs: "Request logs",
    pricing: "Pricing",
    settings: "Settings",
    launchAtLogin: "Launch at login",
    opacity: "Opacity",
    about: "Local-first · multi-agent usage",
    hudBar: "Dynamic island",
    quit: "Quit",
    ranges: { today: "Today", "7d": "7 days", "30d": "30 days", all: "All" },
    metrics: { tokens: "Tokens", requests: "Requests", cost: "Cost" },
    totalCost: "Total cost",
    totalRequests: "Requests",
    tokens: "Tokens used",
    cacheHit: "Cache hit",
    costDetail: "local pricing",
    requestsDetail: "local + synced",
    input: "In",
    output: "Out",
    cacheWrite: "Write",
    cacheRead: "Read",
    trend: "Usage trend",
    noData: "No usage yet",
    noDataSub: "Stats appear here after your agents start working.",
    errorTitle: "Failed to load usage",
    errorSub: "Try again, or open the main window for request logs.",
    retry: "Retry",
    partial: "Some sources failed",
    settingsTitle: "Settings",
  },
};

// ===== 偏好读写（失败静默：记忆仅锦上添花，不影响菜单使用） =====

const TAB_KEY = "agenthud-tray-tab";
const METRIC_KEY = "agenthud-tray-usage-metric";
const CACHE_TTL_MS = 30_000;
const REFRESH_MS = 60_000;
const MAX_BARS = 31;
const MAX_SERIES_BUCKETS = 10_000;

function readPref<T extends string>(
  key: string,
  options: readonly T[],
  fallback: T
): T {
  try {
    const value = localStorage.getItem(key);
    if (value && (options as readonly string[]).includes(value)) {
      return value as T;
    }
  } catch {
    /* localStorage 不可用：用默认值 */
  }
  return fallback;
}

function writePref(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* 记不住不影响使用 */
  }
}

function detectLocale(): Locale {
  try {
    return localStorage.getItem(LOCALE_KEY) === "en" ? "en" : "zh";
  } catch {
    return "zh";
  }
}

/** 应用主题（主面板 appearance.ts loadTheme 同语义）：仅 "dark" 视为
 *  暗色，其余（含无值/损坏值/读取异常）一律亮色。纯 CSS 生效，无需重渲染 */
function applyHudTheme(): void {
  let dark = false;
  try {
    dark = localStorage.getItem(THEME_KEY) === "dark";
  } catch {
    dark = false;
  }
  document.documentElement.classList.toggle("dark", dark);
}

// ===== 运行时状态 =====

let locale: Locale = detectLocale();
let msg: Msg = MESSAGES[locale];
let activeTab: Tab = readPref(TAB_KEY, TABS, "usage");
let range: RangeKey = "today";
let metric: MetricKey = readPref(METRIC_KEY, METRICS, "tokens");
let trayState: TrayMenuState | null = null;
let hudEnabled = false;
let autostartEnabled = false;
/** 最近一次成功/失败后保留的用量数据（刷新失败时不清空） */
let usage: TrayMenuUsage | null = null;
let usageError: string | null = null;
let usageBusy = false;
/** 请求序号守卫：切档位/重新显示时只认最新一次请求 */
let usageReqId = 0;
/** 档位 → 最近一次成功数据（菜单打开期间切档位秒回；focus 时清空） */
const usageCache = new Map<string, { data: TrayMenuUsage; at: number }>();
let refreshTimer: number | null = null;

// ===== DOM 引用 =====

function byId<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) {
    throw new Error(`tray-menu 缺少元素 #${id}`);
  }
  return el as T;
}

const tabActionsEl = byId<HTMLButtonElement>("tab-actions");
const tabUsageEl = byId<HTMLButtonElement>("tab-usage");
const panelActionsEl = byId<HTMLElement>("panel-actions");
const panelUsageEl = byId<HTMLElement>("panel-usage");
const contentEl = byId<HTMLElement>("content");

const btnSettingsEl = byId<HTMLButtonElement>("btn-settings");
const btnOpenMainEl = byId<HTMLButtonElement>("btn-open-main");
const btnQuitEl = byId<HTMLButtonElement>("btn-quit");
const rowAutostartEl = byId<HTMLButtonElement>("row-autostart");
const rowHudEl = byId<HTMLButtonElement>("row-hud");
const alphaRangeEl = byId<HTMLInputElement>("alpha-range");
const alphaValueEl = byId<HTMLElement>("alpha-value");

const rangeGroupEl = byId<HTMLElement>("range-group");
const metricGroupEl = byId<HTMLElement>("metric-group");
const rangeButtons = Array.from(
  rangeGroupEl.querySelectorAll<HTMLButtonElement>("[data-range]")
);
const metricButtons = Array.from(
  metricGroupEl.querySelectorAll<HTMLButtonElement>("[data-metric]")
);

const aboutVersionEl = byId<HTMLElement>("about-version");

const usageBodyEl = byId<HTMLElement>("usage-body");
const usageLoadingEl = byId<HTMLElement>("usage-loading");
const usageEmptyEl = byId<HTMLElement>("usage-empty");
const usageErrorEl = byId<HTMLElement>("usage-error");
const usageErrorSubEl = byId<HTMLElement>("usage-error-sub");
const usageRetryEl = byId<HTMLButtonElement>("usage-retry");

const statCostEl = byId<HTMLElement>("stat-cost");
const statRequestsEl = byId<HTMLElement>("stat-requests");
const statTokensEl = byId<HTMLElement>("stat-tokens");
const statCacheEl = byId<HTMLElement>("stat-cache");
const statTokensDetailEl = byId<HTMLElement>("stat-tokens-detail");
const statCacheDetailEl = byId<HTMLElement>("stat-cache-detail");
const chartTitleEl = byId<HTMLElement>("chart-title");
const chartWarningEl = byId<HTMLElement>("chart-warning");
const chartGridEl = byId<HTMLElement>("chart-grid");
const chartBarsEl = byId<HTMLElement>("chart-bars");
const chartYLabelsEl = byId<HTMLElement>("chart-ylabels");
const chartXLabelsEl = byId<HTMLElement>("chart-xlabels");
const chartTooltipEl = byId<HTMLElement>("chart-tooltip");
const chartEl = byId<HTMLElement>("chart");

// ===== 小工具 =====

function setText(id: string, text: string): void {
  const el = document.getElementById(id);
  if (el) el.textContent = text;
}

function setSwitch(el: HTMLElement, on: boolean): void {
  el.setAttribute("aria-checked", on ? "true" : "false");
}

function shortError(error: unknown): string {
  const text = String(error).replace(/^Error:\s*/, "");
  return text.length > 160 ? text.slice(0, 157) + "…" : text;
}

function localeTag(): string {
  return locale === "zh" ? "zh-CN" : "en-US";
}

const intlFormatters: Partial<Record<Locale, Intl.NumberFormat>> = {};

function formatInt(n: number): string {
  const formatter =
    intlFormatters[locale] ??
    (intlFormatters[locale] = new Intl.NumberFormat(localeTag()));
  return formatter.format(Math.round(n));
}

/** 紧凑数量：1.2K / 13.8M / 1.4B（与主面板 formatTokens 的 K/M/B 台阶一致） */
function compact(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0";
  const trim = (value: number) => {
    const text = value.toFixed(1);
    return text.endsWith(".0") ? text.slice(0, -2) : text;
  };
  if (n >= 1_000_000_000) return `${trim(n / 1_000_000_000)}B`;
  if (n >= 1_000_000) return `${trim(n / 1_000_000)}M`;
  if (n >= 1_000) return `${trim(n / 1_000)}K`;
  return String(Math.round(n));
}

/** 金额（汇总卡）：与主面板 formatCost 同口径，极小值保留 4 位，大额带千分位 */
function money(value: number, isUsd: boolean, digits = 2): string {
  const symbol = isUsd ? "$" : "¥";
  if (!Number.isFinite(value)) return "—";
  if (value === 0) return `${symbol}0.00`;
  const fractionDigits = value < 0.01 ? 4 : digits;
  return `${symbol}${new Intl.NumberFormat(localeTag(), {
    minimumFractionDigits: fractionDigits,
    maximumFractionDigits: fractionDigits,
  }).format(value)}`;
}

/** 坐标轴金额：千位以上用 1.2K / 3.4M 紧凑写法，窄轴标签才放得下 */
function moneyCompact(value: number, isUsd: boolean): string {
  const symbol = isUsd ? "$" : "¥";
  if (!Number.isFinite(value)) return "—";
  if (Math.abs(value) >= 1_000) return `${symbol}${compact(value)}`;
  if (value === 0) return `${symbol}0`;
  return money(value, isUsd, value < 0.01 ? 4 : value < 1 ? 2 : 0);
}

function percent(rate: number | null): string {
  if (rate == null || !Number.isFinite(rate)) return "—";
  return `${(rate * 100).toFixed(1)}%`;
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

// ===== 静态文案 / 标签页 / 档位 =====

function applyStaticText(): void {
  document.documentElement.lang = localeTag();
  msg = MESSAGES[locale];

  setText("tab-actions", msg.tabActions);
  setText("tab-usage", msg.tabUsage);
  setText("label-quick", msg.quickActions);
  setText("label-general", msg.general);
  setText("act-main-label", msg.openMain);
  setText("act-panel-label", msg.openPanel);
  setText("act-logs-label", msg.logs);
  setText("act-pricing-label", msg.pricing);
  setText("act-settings-label", msg.settings);
  setText("row-autostart-label", msg.launchAtLogin);
  setText("row-alpha-label", msg.opacity);
  setText("row-hud-label", msg.hudBar);
  setText("btn-open-main-label", msg.openMain);
  setText("btn-quit-label", msg.quit);
  setText("about-text", msg.about);
  setText("stat-cost-label", msg.totalCost);
  setText("stat-requests-label", msg.totalRequests);
  setText("stat-tokens-label", msg.tokens);
  setText("stat-cache-label", msg.cacheHit);
  setText("stat-cost-detail", msg.costDetail);
  setText("stat-requests-detail", msg.requestsDetail);
  setText("usage-empty-title", msg.noData);
  setText("usage-empty-sub", msg.noDataSub);
  setText("usage-error-title", msg.errorTitle);
  setText("usage-error-sub", msg.errorSub);
  setText("usage-retry", msg.retry);
  btnSettingsEl.title = msg.settingsTitle;
  btnSettingsEl.setAttribute("aria-label", msg.settingsTitle);

  for (const button of rangeButtons) {
    const key = button.dataset.range as RangeKey;
    button.textContent = msg.ranges[key];
  }
  for (const button of metricButtons) {
    const key = button.dataset.metric as MetricKey;
    button.textContent = msg.metrics[key];
  }
  applyTabState();
  applyRangeState();
  applyMetricState();
  renderState();
  renderUsage();
}

/**
 * 切成"下拉面板 + 内容区"的两段式：切页只改 hidden/aria-selected，不做
 * 路由；记忆偏好，下次打开直接落在上次的页。
 */
function applyTabState(): void {
  const isActions = activeTab === "actions";
  tabActionsEl.setAttribute("aria-selected", isActions ? "true" : "false");
  tabUsageEl.setAttribute("aria-selected", isActions ? "false" : "true");
  panelActionsEl.hidden = !isActions;
  panelUsageEl.hidden = isActions;
}

function applyRangeState(): void {
  for (const button of rangeButtons) {
    const selected = button.dataset.range === range;
    button.setAttribute("aria-selected", selected ? "true" : "false");
  }
}

function applyMetricState(): void {
  for (const button of metricButtons) {
    const selected = button.dataset.metric === metric;
    button.setAttribute("aria-selected", selected ? "true" : "false");
  }
}

function selectTab(next: Tab): void {
  if (activeTab === next) return;
  activeTab = next;
  writePref(TAB_KEY, next);
  applyTabState();
  hideChartTooltip();
  if (next === "usage") {
    if (!usage || usage.range !== range) {
      usage = usageCache.get(range)?.data ?? null;
    }
    renderUsage();
    void fetchUsage(false);
  }
}

function selectRange(next: RangeKey): void {
  if (range === next) return;
  range = next;
  applyRangeState();
  hideChartTooltip();
  usage = usageCache.get(next)?.data ?? null;
  usageError = null;
  renderUsage();
  void fetchUsage(false);
}

function selectMetric(next: MetricKey): void {
  if (metric === next) return;
  metric = next;
  writePref(METRIC_KEY, next);
  applyMetricState();
  hideChartTooltip();
  renderChart();
}

// ===== 菜单透明度（快捷页 · 通用）=====

/** 滑块位置 + 百分比读数：range 刻度是 40~100 的整数（见 tray-menu.html，
 *  与 appearance.ts 的 MIN_TRAY_MENU_ALPHA = 0.4 一一对应） */
function paintMenuAlpha(alpha: number): void {
  const pct = Math.round(alpha * 100);
  alphaRangeEl.value = String(pct);
  alphaValueEl.textContent = `${pct}%`;
  alphaRangeEl.setAttribute("aria-valuetext", `${pct}%`);
}

/** 读偏好 → 写 --m-bg-alpha + 刷滑块。启动 / 每次显示 / 主题切换后各调一次
 *  （主题切换会换缺省值：亮 76% / 暗 82%，用户存过值则两套都用用户值） */
function syncMenuAlpha(): void {
  const alpha = loadTrayMenuAlpha();
  applyTrayMenuAlpha(alpha);
  paintMenuAlpha(alpha);
}

// ===== 状态 / 开关 =====

function renderState(): void {
  if (trayState) {
    aboutVersionEl.textContent = `v${trayState.version}`;
  }
  setSwitch(rowHudEl, hudEnabled);
  setSwitch(rowAutostartEl, autostartEnabled);
}

async function refreshState(): Promise<void> {
  try {
    trayState = await invoke<TrayMenuState>("tray_menu_state");
    hudEnabled = trayState.hudBarEnabled;
    renderState();
  } catch {
    /* 静默：保持上次显示（版本显示 v– 也不影响操作） */
  }
}

async function refreshAutostart(): Promise<void> {
  try {
    autostartEnabled = await isAutostartEnabled();
    renderState();
  } catch {
    /* 插件不可用/无权限：保持关闭显示，不阻断菜单 */
  }
}

async function toggleAutostart(): Promise<void> {
  const next = !autostartEnabled;
  autostartEnabled = next;
  renderState();
  try {
    if (next) {
      await enableAutostart();
    } else {
      await disableAutostart();
    }
  } catch {
    // 失败回滚：菜单不弹额外错误层，保持与设置页相同的"只显示最终真态"
    autostartEnabled = !next;
    renderState();
  }
}

function toggleHud(): void {
  const next = !hudEnabled;
  hudEnabled = next;
  renderState();
  // 后端切换成功会广播 zbar://hud-bar-state；失败则回滚本地态
  invoke("tray_menu_action", { action: "hud-toggle" }).catch(() => {
    hudEnabled = !next;
    renderState();
  });
}

function runAction(action: string): void {
  invoke("tray_menu_action", { action }).catch(() => {
    /* 后端失败只记日志；菜单已由后端 hide，不阻塞下一次操作 */
  });
}

// ===== 用量取数 =====

async function fetchUsage(force: boolean): Promise<void> {
  const requested = range;
  const cached = usageCache.get(requested);
  if (!force && cached && Date.now() - cached.at < CACHE_TTL_MS) {
    usage = cached.data;
    usageError = null;
    usageBusy = false;
    renderUsage();
    return;
  }

  const reqId = ++usageReqId;
  usageBusy = true;
  usageError = null;
  if (usage && usage.range !== requested) {
    usage = cached?.data ?? null;
  }
  renderUsage();

  try {
    const data = await invoke<TrayMenuUsage>("tray_menu_usage", {
      range: requested,
    });
    if (reqId !== usageReqId) return;
    usage = data;
    usageError = null;
    usageCache.set(requested, { data, at: Date.now() });
  } catch (error) {
    if (reqId !== usageReqId) return;
    usageError = shortError(error);
    // 有上次成功值就继续展示（静默刷新失败不清空）；否则落到错误态
    usage = cached?.data ?? (usage?.range === requested ? usage : null);
  } finally {
    if (reqId === usageReqId) {
      usageBusy = false;
      renderUsage();
    }
  }
}

// ===== 用量渲染 =====

function renderUsage(): void {
  const hasData = usage != null;
  const noRequests = usage != null && usage.summary.requests === 0;
  const showBody = usage != null && !noRequests;
  const showPlaceholder = !hasData && usageError == null;

  usageBodyEl.hidden = !showBody;
  usageBodyEl.classList.toggle("is-busy", usageBusy && showBody);
  usageLoadingEl.hidden = !showPlaceholder;
  usageEmptyEl.hidden = !noRequests;
  usageErrorEl.hidden = !(usageError != null && !hasData);

  if (usageError && !hasData) {
    usageErrorSubEl.textContent = usageError;
  }

  if (showBody && usage) {
    renderStats(usage);
    renderChart();
  } else if (!hasData) {
    clearChart();
  }
  renderWarning();
}

/** 失败来源的展示名：remote 对用户说「同步设备」，其余来源名本身可读 */
function sourceLabel(source: string): string {
  if (source === "remote") {
    return locale === "zh" ? "同步设备" : "synced devices";
  }
  return source;
}

function renderWarning(): void {
  let message = "";
  if (usage?.failedSources.length) {
    message = `${msg.partial}: ${usage.failedSources
      .map(sourceLabel)
      .join(", ")}`;
  } else if (usageError && usage) {
    message = usageError;
  }
  chartWarningEl.hidden = !message;
  chartWarningEl.title = message;
}

function renderStats(data: TrayMenuUsage): void {
  const summary = data.summary;
  const cost = data.isUsd ? summary.costUsd : summary.costCny;
  statCostEl.textContent = money(cost, data.isUsd);
  statRequestsEl.textContent =
    summary.requests >= 1_000_000
      ? compact(summary.requests)
      : formatInt(summary.requests);
  statTokensEl.textContent = compact(summary.totalTokens);
  statCacheEl.textContent = percent(summary.cacheHitRate);
  statTokensDetailEl.textContent = `${msg.input} ${compact(
    summary.inputTokens
  )} · ${msg.output} ${compact(summary.outputTokens)}`;
  statCacheDetailEl.textContent = `${msg.cacheWrite} ${compact(
    summary.cacheWriteTokens
  )} · ${msg.cacheRead} ${compact(summary.cacheReadTokens)}`;
}

// ===== 趋势图（纯 DOM/CSS 柱状图） =====

interface ChartPoint {
  key: string;
  label: string;
  fullLabel: string;
  tokens: number;
  requests: number;
  costUsd: number;
  costCny: number;
}

interface ChartSlot extends ChartPoint {
  value: number;
}

/** 解析后端桶键（本地时间，不带时区后缀） */
function parseBucket(key: string, bucket: "hour" | "day"): Date | null {
  if (bucket === "hour") {
    const [datePart, hourPart] = key.split("T");
    if (!datePart) return null;
    const [year, month, day] = datePart.split("-").map(Number);
    const hour = Number(hourPart ?? 0);
    if (!year || !month || !day || Number.isNaN(hour)) return null;
    return new Date(year, month - 1, day, hour, 0, 0, 0);
  }
  const [year, month, day] = key.split("-").map(Number);
  if (!year || !month || !day) return null;
  return new Date(year, month - 1, day, 0, 0, 0, 0);
}

function bucketKey(date: Date, bucket: "hour" | "day"): string {
  const day = `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(
    date.getDate()
  )}`;
  return bucket === "hour" ? `${day}T${pad2(date.getHours())}` : day;
}

function floorBucket(date: Date, bucket: "hour" | "day"): Date {
  const next = new Date(date);
  if (bucket === "hour") {
    next.setMinutes(0, 0, 0);
  } else {
    next.setHours(0, 0, 0, 0);
  }
  return next;
}

const MONTHS_EN = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

function fullBucketLabel(date: Date, bucket: "hour" | "day"): string {
  if (locale === "zh") {
    const day = `${date.getMonth() + 1}月${date.getDate()}日`;
    return bucket === "hour"
      ? `${day} ${pad2(date.getHours())}:00`
      : `${date.getFullYear()}年${day}`;
  }
  const day = `${MONTHS_EN[date.getMonth()]} ${date.getDate()}`;
  return bucket === "hour"
    ? `${day}, ${pad2(date.getHours())}:00`
    : `${day}, ${date.getFullYear()}`;
}

/** 补齐空桶：从区间起点到终点按小时/日逐格走，缺数据的桶补 0 */
function buildSeries(data: TrayMenuUsage): ChartPoint[] {
  if (data.trend.length === 0) return [];
  const first = parseBucket(data.trend[0].bucket, data.bucket);
  if (!first) return [];
  const start = floorBucket(
    data.fromMs > 0 ? new Date(data.fromMs) : first,
    data.bucket
  );
  const end = floorBucket(new Date(data.toMs), data.bucket);
  if (start.getTime() > end.getTime()) return [];

  const byBucket = new Map(
    data.trend.map((point) => [point.bucket, point] as const)
  );
  // 跨半年以上的日序列在 X 轴用「年-月」，避免「全部」档 05-23 到底
  // 是哪一年看不出来、也避免 8 位日期标签互相重叠；7/30 天档仍用
  // MM-DD，保持标签短。
  const longSpan =
    data.bucket === "day" &&
    end.getTime() - start.getTime() > 180 * 86_400_000;
  const points: ChartPoint[] = [];
  const cursor = new Date(start);
  for (let guard = 0; guard < MAX_SERIES_BUCKETS; guard += 1) {
    if (cursor.getTime() > end.getTime()) break;
    const key = bucketKey(cursor, data.bucket);
    const item = byBucket.get(key);
    points.push({
      key,
      label:
        data.bucket === "hour"
          ? `${pad2(cursor.getHours())}:00`
          : longSpan
            ? `${String(cursor.getFullYear()).slice(2)}-${pad2(
                cursor.getMonth() + 1
              )}`
            : `${pad2(cursor.getMonth() + 1)}-${pad2(cursor.getDate())}`,
      fullLabel: fullBucketLabel(cursor, data.bucket),
      tokens: item?.tokens ?? 0,
      requests: item?.requests ?? 0,
      costUsd: item?.costUsd ?? 0,
      costCny: item?.costCny ?? 0,
    });
    if (data.bucket === "hour") {
      cursor.setHours(cursor.getHours() + 1);
    } else {
      cursor.setDate(cursor.getDate() + 1);
    }
  }
  return points;
}

/** 超过 31 根就把相邻桶并成一根（求和），与 CC Switch 的 31 根上限同思路 */
function mergeSeries(points: ChartPoint[]): ChartPoint[] {
  if (points.length <= MAX_BARS) return points;
  const size = Math.ceil(points.length / MAX_BARS);
  const merged: ChartPoint[] = [];
  for (let i = 0; i < points.length; i += size) {
    const chunk = points.slice(i, i + size);
    const first = chunk[0];
    const last = chunk[chunk.length - 1];
    if (!first || !last) continue;
    merged.push({
      key: first.key,
      label: first.label,
      fullLabel:
        chunk.length > 1
          ? `${first.fullLabel} – ${last.fullLabel}`
          : first.fullLabel,
      tokens: chunk.reduce((sum, point) => sum + point.tokens, 0),
      requests: chunk.reduce((sum, point) => sum + point.requests, 0),
      costUsd: chunk.reduce((sum, point) => sum + point.costUsd, 0),
      costCny: chunk.reduce((sum, point) => sum + point.costCny, 0),
    });
  }
  return merged;
}

function niceMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1;
  const exponent = Math.floor(Math.log10(value));
  const base = 10 ** exponent;
  const normalized = value / base;
  const factor =
    normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10;
  return factor * base;
}

function metricValue(point: ChartPoint, isUsd: boolean): number {
  if (metric === "tokens") return point.tokens;
  if (metric === "requests") return point.requests;
  return isUsd ? point.costUsd : point.costCny;
}

function formatMetricValue(value: number, isUsd: boolean): string {
  if (metric === "tokens") return compact(value);
  if (metric === "requests") return formatInt(value);
  return money(value, isUsd);
}

function formatAxisValue(value: number, isUsd: boolean): string {
  if (metric === "tokens" || metric === "requests") return compact(value);
  return moneyCompact(value, isUsd);
}

function clearChart(): void {
  chartBarsEl.replaceChildren();
  chartGridEl.replaceChildren();
  chartYLabelsEl.replaceChildren();
  chartXLabelsEl.replaceChildren();
  hideChartTooltip();
}

function hideChartTooltip(): void {
  chartTooltipEl.hidden = true;
}

function showChartTooltip(
  slot: HTMLElement,
  point: ChartSlot,
  isUsd: boolean
): void {
  const bar = slot.firstElementChild;
  if (!bar) return;
  const chartRect = chartEl.getBoundingClientRect();
  const slotRect = slot.getBoundingClientRect();
  const barRect = bar.getBoundingClientRect();
  const left = slotRect.left - chartRect.left + slotRect.width / 2;
  const top = barRect.top - chartRect.top - 5;
  chartTooltipEl.style.left = `${Math.min(
    Math.max(left, 46),
    Math.max(46, chartRect.width - 8)
  )}px`;
  chartTooltipEl.style.top = `${Math.max(top, 18)}px`;
  const label = document.createElement("div");
  label.className = "tt-label";
  label.textContent = point.fullLabel;
  const value = document.createElement("div");
  value.className = "tt-value";
  value.textContent = formatMetricValue(point.value, isUsd);
  chartTooltipEl.replaceChildren(label, value);
  chartTooltipEl.hidden = false;
}

function renderChart(): void {
  const data = usage;
  if (!data) {
    clearChart();
    return;
  }
  const rangeLabel =
    msg.ranges[data.range as RangeKey] ?? msg.ranges[range];
  chartTitleEl.textContent = `${msg.trend} · ${rangeLabel}`;
  renderWarning();

  const points = mergeSeries(buildSeries(data));
  const isUsd = data.isUsd;
  const slots: ChartSlot[] = points.map((point) => ({
    ...point,
    value: metricValue(point, isUsd),
  }));
  const maxRaw = slots.reduce((max, point) => Math.max(max, point.value), 0);
  const max = maxRaw > 0 ? niceMax(maxRaw) : 1;

  // 网格 + Y 轴：0 / 50% / 100% 三条线，刻度用紧凑数量
  chartGridEl.replaceChildren();
  chartYLabelsEl.replaceChildren();
  const ticks = maxRaw > 0 ? [0, 0.5, 1] : [];
  for (const frac of ticks) {
    const line = document.createElement("div");
    line.className = "chart-grid-line";
    line.style.bottom = `${frac * 100}%`;
    chartGridEl.append(line);
    const label = document.createElement("span");
    label.className = "chart-ylabel";
    label.style.bottom = `${frac * 100}%`;
    label.textContent = formatAxisValue(max * frac, isUsd);
    chartYLabelsEl.append(label);
  }

  // 柱体
  chartBarsEl.replaceChildren();
  for (const point of slots) {
    const slot = document.createElement("div");
    slot.className = "chart-bar-slot";
    const bar = document.createElement("div");
    bar.className = "chart-bar";
    const ratio = max > 0 ? point.value / max : 0;
    if (point.value <= 0) {
      bar.classList.add("is-zero");
      bar.style.height = "1px";
    } else {
      bar.style.height = `${Math.min(100, Math.max(ratio * 100, 1.5))}%`;
    }
    slot.append(bar);
    slot.addEventListener("pointerenter", () => {
      showChartTooltip(slot, point, isUsd);
    });
    slot.addEventListener("pointerleave", hideChartTooltip);
    chartBarsEl.append(slot);
  }

  // X 轴：固定 5 个位置（首/尾对齐，避免文字出界）
  chartXLabelsEl.replaceChildren();
  if (slots.length > 0) {
    // 小时桶按 4 小时间隔取标签（00/04/08/12 + 末尾当前时刻），比均分
    // 4 段更符合看时间的直觉；日桶仍按首尾 + 均分 4 段。
    const indices = Array.from(
      new Set(
        data.bucket === "hour" && slots.length >= 8
          ? [0, 4, 8, 12, slots.length - 1].filter(
              (index) => index < slots.length
            )
          : [0, 0.25, 0.5, 0.75, 1].map((frac) =>
              Math.round((slots.length - 1) * frac)
            )
      )
    );
    indices.forEach((index, order) => {
      const point = slots[index];
      if (!point) return;
      const label = document.createElement("span");
      label.className = "chart-xlabel";
      if (order === 0) label.classList.add("is-first");
      if (index === slots.length - 1) label.classList.add("is-last");
      label.style.left =
        slots.length > 1 ? `${(index / (slots.length - 1)) * 100}%` : "0%";
      label.textContent = point.label;
      chartXLabelsEl.append(label);
    });
  }
  hideChartTooltip();
}

// ===== 事件绑定 =====

function visibleNavItems(): HTMLElement[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>("[data-nav]")
  ).filter(
    (el) =>
      !el.hasAttribute("disabled") &&
      el.closest("[hidden]") == null &&
      el.getClientRects().length > 0
  );
}

function bindEvents(): void {
  tabActionsEl.addEventListener("click", () => selectTab("actions"));
  tabUsageEl.addEventListener("click", () => selectTab("usage"));

  for (const button of rangeButtons) {
    button.addEventListener("click", () => {
      selectRange(button.dataset.range as RangeKey);
    });
  }
  for (const button of metricButtons) {
    button.addEventListener("click", () => {
      selectMetric(button.dataset.metric as MetricKey);
    });
  }

  for (const row of document.querySelectorAll<HTMLElement>("[data-action]")) {
    row.addEventListener("click", () => {
      const action = row.dataset.action;
      if (action) runAction(action);
    });
  }

  btnSettingsEl.addEventListener("click", () => runAction("open-main-settings"));
  btnOpenMainEl.addEventListener("click", () => runAction("open-main"));
  btnQuitEl.addEventListener("click", () => runAction("quit"));
  usageRetryEl.addEventListener("click", () => void fetchUsage(true));

  rowAutostartEl.addEventListener("click", () => void toggleAutostart());
  rowHudEl.addEventListener("click", toggleHud);

  // 透明度滑块：拖动即时生效（input），松手才落盘（change，一次拖动只写一次）
  alphaRangeEl.addEventListener("input", () => {
    const alpha = Number(alphaRangeEl.value) / 100;
    applyTrayMenuAlpha(alpha);
    paintMenuAlpha(alpha);
  });
  alphaRangeEl.addEventListener("change", () => {
    persistTrayMenuAlpha(Number(alphaRangeEl.value) / 100);
  });

  contentEl.addEventListener("scroll", hideChartTooltip, { passive: true });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      hideWindow();
      return;
    }
    // 透明度滑块自己吃方向键（range 原生调值），不参与菜单的焦点导航
    if (event.target instanceof HTMLInputElement) return;
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const items = visibleNavItems();
    if (items.length === 0) return;
    event.preventDefault();
    const current = document.activeElement as HTMLElement | null;
    const index = current ? items.indexOf(current) : -1;
    const step = event.key === "ArrowDown" ? 1 : -1;
    const next =
      index < 0
        ? step > 0
          ? 0
          : items.length - 1
        : (index + step + items.length) % items.length;
    items[next]?.focus();
  });

  window.addEventListener("focus", onShown);
  window.addEventListener("blur", stopRefreshTimer);
}

function hideWindow(): void {
  void getCurrentWindow().hide().catch(() => {
    /* 权限/窗口异常：忽略，Rust 侧还会按失焦收起 */
  });
}

// ===== 显示生命周期 =====

function stopRefreshTimer(): void {
  if (refreshTimer != null) {
    window.clearInterval(refreshTimer);
    refreshTimer = null;
  }
}

function startRefreshTimer(): void {
  stopRefreshTimer();
  refreshTimer = window.setInterval(() => {
    if (activeTab === "usage") void fetchUsage(true);
  }, REFRESH_MS);
}

function onShown(): void {
  // 事件可能因窗口隐藏/页面重载错过：每次显示再校一次主题与语言偏好
  applyHudTheme();
  // 主题可能换了（亮/暗缺省透明度不同），同时兜住其他窗口写入的新值
  syncMenuAlpha();
  const nextLocale = detectLocale();
  if (nextLocale !== locale) {
    locale = nextLocale;
    msg = MESSAGES[locale];
    applyStaticText();
  }
  // 每次右键显示都回到「当天」档（用户偏好：默认先看今天）；档位不落
  // localStorage，本次菜单打开期间仍可自由切到 7/30/全部。
  range = "today";
  applyRangeState();
  hideChartTooltip();
  // stale-while-revalidate：先把上次缓存立刻画出来（再次右键时不再先闪
  // 骨架屏），同时 force 重取；「全部」档要串行查多段远端，这一点对
  // 体感尤其重要。拉取失败时缓存数据继续展示，由 chart 警示角标提示。
  usageReqId += 1;
  usageBusy = false;
  usageError = null;
  usage = usageCache.get(range)?.data ?? null;
  renderUsage();
  if (activeTab === "usage") {
    void fetchUsage(true);
  }
  void refreshState();
  void refreshAutostart();
  startRefreshTimer();
}

// ===== 启动 =====

function main(): void {
  applyHudTheme();
  syncMenuAlpha();
  applyStaticText();
  bindEvents();

  // 外观同步流：主面板语言/主题切换后广播 zbar://appearance-changed
  // （无 payload，接收方重读 localStorage 自行对比）。语言变化：重读词典
  // + 全量重渲染；主题变化：applyHudTheme 切 .dark，纯 CSS 生效。
  void listen(APPEARANCE_CHANGED_EVENT, () => {
    applyHudTheme();
    // 主题切换会换裸缺省透明度（亮 76% / 暗 82%），同步重算一次
    syncMenuAlpha();
    const next = detectLocale();
    if (next !== locale) {
      locale = next;
      msg = MESSAGES[locale];
      applyStaticText();
    }
  }).catch(() => {
    /* 静默：保持启动时读取的语言与主题 */
  });

  // 灵动岛开关事件：面板顶部按钮 / 托盘 / 异常销毁都会广播，
  // 打开着菜单时实时同步底栏开关
  void listen<{ enabled: boolean }>("zbar://island-state", (event) => {
    if (typeof event.payload?.enabled === "boolean") {
      hudEnabled = event.payload.enabled;
      renderState();
    }
  }).catch(() => {
    /* 静默：开关仍可通过菜单操作，事件只是被动同步 */
  });

  // 首次渲染（窗口可能是预热隐藏的，也可能是首次右键才建出来的）
  void refreshState();
  void refreshAutostart();
  renderUsage();

  // 页面在窗口已显示后才加载时 focus 事件可能已错过：补一次
  if (document.hasFocus()) {
    window.setTimeout(onShown, 0);
  }
}

void main();
