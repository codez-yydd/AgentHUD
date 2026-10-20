/**
 * HUD 停靠条宿主壳（hud-bar）。
 *
 * 与会话悬浮窗宿主壳（session-hud-main.ts）同款的零 React 薄壳：不引入
 * panel 的组件体系，条目渲染在壳内直接完成。职责：
 * - 数据流：监听 zbar://hud-bar-usage（hud_bar.rs 数据线程每 30s 组装、
 *   内容变化才推的各 Agent 额度），每个 Agent 一个条目——"语义环 +
 *   品牌图标 + 已用百分比"（参照 jazzenchen/agent-hud-open Simple 模式：
 *   环画已用比例、旁边直接写数字，抬眼可读）；环与数字取该 Agent 全部
 *   窗口中已用% 最高（= 余量最紧）的窗口，颜色按已用阈值分档：<70% 绿
 *   / 70–89% 黄 / ≥90% 红（与注入版统计条的三档阈值同源：已用 70% =
 *   剩余 30%；数值色 = 环色，健康态也用绿）。设置里已开启但本轮没取到
 *   额度的 Agent 也保留弱化条目（「—」），不再整条消失；
 * - 布局流：监听 zbar://hud-bar-layout（payload = { orientation, edge }，
 *   贴边吸附换边后 Rust 推送）切换竖排 flex column（贴左/右）与横排
 *   flex row（贴顶），并记住停靠边（.edge-right 类让面板贴窗口右缘，
 *   卡片按边定位方向）；启动时按窗口横竖比自判兜底（窗口尺寸由 Rust
 *   bar_metrics 按停靠边预置：竖排恒 128 逻辑宽、横排恒 92 逻辑高，条长
 *   下限 120 保证两者可分；layout 事件在页面未就绪时丢失也不怕）；
 * - 悬浮抽屉 + 详情卡：pointerenter 条 → 90ms 后给 #hud-root 加 .hud-open，
 *   面板宽度 70 → 112（横排按 50 → 100/条计算），百分比由 overflow 裁切后
 *   随宽度过渡拉出（0.34s cubic-bezier，类似 element-ui 抽屉）；同时悬浮
 *   到某个条目就直接显示该 Agent 的详情卡，不用先点击。卡片内容：品牌
 *   头像 + 名称/套餐/健康态 + 每窗口两行式额度行（状态点/标签/已用%、
 *   进度轨/剩余/倒计时）+ Token 区（24h 输入/输出/缓存堆叠直方图、
 *   今日/请求/峰值）+ 底部「打开主窗口」按钮 → invoke
 *   open_main_window("overview")。指针可移到卡上操作，离开条/卡约 260ms
 *   后收卡；点击条目仍可打开，点击其他区域收起。卡片按边定位：贴左卡在
 *   条右侧、贴右卡在条左侧、贴顶卡在条下方；纵向跟随 hover 的条目居中
 *   （Rust 扩窗时在**上下两侧对称**补画布，条在画布内居中，位置与扩窗
 *   无关，卡片因此能贴着条目而不是被顶到画布边缘），入场是从条那一
 *   侧滑出的抽屉式动画；
 * - 扩窗态同步：监听 zbar://hud-bar-popover（Rust 侧吸附强制缩回/托盘
 *   关闭时单方面缩窗后广播 { open:false }），收敛本地扩窗态并收卡（详情
 *   开着也收），防两端状态失步；扩窗命令返回 {width,height}（目标视口
 *   尺寸），卡片定位前先等视口到位；
 * - 首帧保障：三路 listen 全部注册完成后 invoke hud_bar_refresh（数据
 *   线程按"内容未变"静默跳过时，refresh 强制 200ms 内重推一帧，销毁重开
 *   /页面重载不丢首帧）；
 * - 空态：无数据时显示一条占位小字（"暂无数据"），不闪窗；
 * - 拖拽：条目 .hud-item 不带原生拖拽属性（会吞点击），改为阈值自定义
 *   拖拽——越过 4px 后先把抽屉/卡片/扩窗画布全部收掉，再由前端逐帧把
 *   指针逻辑坐标发给 Rust（hud_bar_drag_move）：远离屏幕边缘时跟手，
 *   靠近左/右/上边 ≤26px 时吸附成该边并切换横竖排（拖到顶部变横排），
 *   松手时必定吸附落盘；拖完吞掉一次 click，并暂停悬停 600ms；
 * - 语言：文案为轻量双语词典（zh/en），语言偏好读主面板写入的
 *   localStorage 键（同源 WebView 共享；缺失/异常回退 zh）。主题不跟随
 *   ——停靠条恒定深色（hud.html 无亮色分支与 .dark 切换），外观监听只
 *   保留语言同步（语言变化全量重渲染）。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { APPEARANCE_CHANGED_EVENT } from "./appearance";
import { getAgentTokenSummary, type AgentTokenSummary } from "./api";
import { loadAgentVisibility } from "./agentVisibility";
import { LOCALE_KEY } from "./i18n/locale";

// ============================================================
// 数据契约（Rust hud_bar.rs 的 HudBarUsage，camelCase 序列化）
// ============================================================

/** 单个额度窗口（usedPct 为已用百分比；环与数字都直接展示已用比例） */

interface HudBarQuota {
  /** 稳定窗口键：hour5 / weekly / monthly / provider 原生 key（interval） */
  key: string;
  labelZh: string;
  labelEn: string;
  usedPct: number;
  /** 下次重置时刻（毫秒）；无则不显示倒计时 */
  resetAt: number | null;
}

/** 单个 Agent 条目（source 与图标键一致；plan 为后端可选下发的套餐名） */
interface HudBarAgent {
  source: string;
  plan?: string;
  /** 后端状态：缺省 = 实时额度正常；"unavailable" = 已开启但暂无额度 */
  status?: "unavailable";
  quotas: HudBarQuota[];
}

/** 开启但没取到额度的条目（未登录 / 凭证过期 / 网络失败） */
function isOffline(agent: HudBarAgent): boolean {
  return agent.status === "unavailable" || agent.quotas.length === 0;
}

/** 数据事件 payload（内容变化才推；倒计时由本地基于 resetAt 计算） */
interface HudBarUsage {
  ts: number;
  agents: HudBarAgent[];
}

/** 布局事件 payload（吸附换边后推送；edge 供卡片按边定位方向） */
interface HudBarLayout {
  orientation: "vertical" | "horizontal";
  edge: "left" | "right" | "top";
}

/** Rust hud_bar_set_popover 的返回：扩窗后的**目标视口尺寸**（逻辑 px，
 *  等于 window.innerWidth/innerHeight；0 = 取不到）。前端等视口到位后
 *  才给卡片定位——扩窗由 OS 异步完成，只等"两帧稳定"会把还在路上的
 *  resize 当成已稳定。 */
interface HudPopoverState {
  width: number;
  height: number;
}

/** Rust hud_bar_pointer_state：光标与窗口左上角的全局物理坐标 */
interface HudBarPointerState {
  cursorX: number;
  cursorY: number;
  windowX: number;
  windowY: number;
  scale: number;
}

/** Rust 拖拽回执：当前停靠边/朝向 + 窗口逻辑几何（屏幕坐标） */
interface HudDragState {
  edge: "left" | "right" | "top";
  orientation: "vertical" | "horizontal";
  x: number;
  y: number;
  w: number;
  h: number;
  snapped: boolean;
}

// ============================================================
// Agent 图标（图形数据复用 src/BrandIcon.tsx 同款来源，零 React 壳内以
// SVG 字符串内联——BrandIcon 是 React 组件无法在此引用，不新引图标库。
// 注意：原生 innerHTML 下 SVG 属性用连字符形态（fill-rule / clip-rule），
// camelCase（fillRule/clipRule）是 React 的属性映射，原生不生效。
// 18px 小尺寸下的取舍（宁可简洁可辨，不要糊成色块）：
// - zai：外框 currentColor（白）+ Z 字改胶囊近黑色（BrandIcon 原版 Z 为
//   白色，白框白字在单色白图标体系下会糊成"白色方块认不出"）；
// - minimax：小怪兽多段轮廓降级为首字母徽标（深色圆底 + 白首字母），
//   不再走 path（18px 下细节糊死，用户实测截图中已是不可辨色块）；
// - 其余四个（codex/claude/kimi/opencodego）保持 path。
// ============================================================

/** 各源图标：source → (viewBox, path 元素串)。path 数据与 BrandIcon.tsx
 *  逐字节一致（zai 30 视口双子路径但内层 Z 换深色、kimi/nonzero 挖空、
 *  opencodego 环 + 内块、codex/claude 单 path）；minimax 已降级为首字母
 *  徽标（见 letterBadgeSvg），不在本表 */
const AGENT_ICONS: Record<string, { viewBox: string; body: string }> = {
  // 智谱 mark：外框 currentColor 白 + 近黑 Z 字（单色体系下保 Z 可辨）
  zai: {
    viewBox: "0 0 30 30",
    body:
      '<path fill="currentColor" d="M24.51 28.51H5.49c-2.21 0-4-1.79-4-4V5.49c0-2.21 1.79-4 4-4h19.03c2.21 0 4 1.79 4 4v19.03c-.01 2.2-1.8 3.99-4.01 3.99Z"/>' +
      '<path fill="#0e0f12" d="m15.47 7.1-1.3 1.85c-.2.29-.54.47-.9.47h-7.1V7.09c-.01.01 9.3.01 9.3.01Zm8.83 0-11.16 15.81H5.7L16.86 7.1h7.44Zm-9.77 15.81 1.31-1.86c.2-.29.54-.47.9-.47h7.09v2.33h-9.3Z"/>',
  },
  // OpenAI mark（Codex 使用 OpenAI 品牌标识）
  codex: {
    viewBox: "0 0 24 24",
    body:
      '<path fill="currentColor" d="M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z"/>',
  },
  // Anthropic mark（Claude 使用 Anthropic 品牌标识）
  claude: {
    viewBox: "0 0 24 24",
    body:
      '<path fill="currentColor" d="M17.3041 3.541h-3.6718l6.696 16.918H24Zm-10.6082 0L0 20.459h3.7442l1.3693-3.5527h7.0052l1.3693 3.5528h3.7442L10.5363 3.5409Zm-.3712 10.2232 2.2914-5.9456 2.2914 5.9456Z"/>',
  },
  // Cursor mark（与 BrandIcon.tsx 的 cursor 路径一致）
  cursor: {
    viewBox: "0 0 24 24",
    body:
      '<path fill="currentColor" d="M11.503.131 1.891 5.678a.84.84 0 0 0-.42.726v11.188c0 .3.162.575.42.724l9.609 5.55a1 1 0 0 0 .998 0l9.61-5.55a.84.84 0 0 0 .42-.724V6.404a.84.84 0 0 0-.42-.726L12.497.131a1.01 1.01 0 0 0-.996 0M2.657 6.338h18.55c.263 0 .43.287.297.515L12.23 22.918c-.062.107-.229.064-.229-.06V12.335a.59.59 0 0 0-.295-.51l-9.11-5.257c-.109-.063-.064-.23.061-.23"/>',
  },
  // Kimi Code 官方 mark（nonzero 挖空，与 BrandIcon kimi 分支同款；
  // 原生 innerHTML 用连字符 fill-rule）
  kimi: {
    viewBox: "0 0 24 24",
    body:
      '<path fill="currentColor" fill-rule="nonzero" d="M1.48 11.88A9.62 9.62 0 1 1 20.72 11.88A9.62 9.62 0 1 1 1.48 11.88Z' +
      "M1.78 14.58H22.22A0.72 0.72 0 0 1 22.94 15.31V20.96A0.72 0.72 0 0 1 22.22 21.68" +
      "H1.78A0.72 0.72 0 0 1 1.06 20.96V15.31A0.72 0.72 0 0 1 1.78 14.58Z" +
      "M19.33 3.83A1.62 1.62 0 1 1 22.58 3.83A1.62 1.62 0 1 1 19.33 3.83Z" +
      "M19.33 3.46L20.72 5.45L19.33 5.45Z" +
      "M9.9 8.27L9.9 10.2A1.26 1.26 0 0 0 12.42 10.2L12.42 8.27A1.26 1.26 0 0 0 9.9 8.27Z" +
      "M14.92 7.58L14.92 9.45A1.17 1.17 0 0 0 17.26 9.45L17.26 7.58A1.17 1.17 0 0 0 14.92 7.58Z" +
      "M2.61 17.34L4.53 18.54A0.48 0.48 0 0 0 5.04 17.72L3.12 16.52A0.48 0.48 0 0 0 2.61 17.34Z" +
      "M3.12 19.74L5.04 18.54A0.48 0.48 0 0 0 4.53 17.72L2.61 18.93A0.48 0.48 0 0 0 3.12 19.74Z" +
      'M7.01 20.23L8.93 20.23A0.48 0.48 0 0 0 8.93 19.27L7.01 19.27A0.48 0.48 0 0 0 7.01 20.23Z"/>',
  },
  // OpenCode 官方 mark：方环 + 0.55 透明内块（与 BrandIcon opencodego
  // 同款；evenodd 挖空在原生 innerHTML 下用连字符 fill-rule/clip-rule，
  // camelCase 不生效会把方环渲染成实心方块）
  opencodego: {
    viewBox: "0 0 24 24",
    body:
      '<path fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" d="M3.28 1.1H20.72V22.9H3.28V1.1ZM7.64 5.46H16.36V18.54H7.64V5.46Z"/>' +
      '<path fill="currentColor" fill-opacity="0.55" d="M7.64 9.82H16.36V18.54H7.64V9.82Z"/>',
  },
};

/** source → 显示名（卡片标题用，与总览页 agentLabel 同名口径） */
const SOURCE_LABELS: Record<string, string> = {
  zai: "ZCode",
  codex: "Codex",
  claude: "Claude",
  cursor: "Cursor",
  kimi: "Kimi Code",
  opencodego: "OpenCode Go",
  minimax: "MiniMax Code",
};

/** XML 文本转义（首字母可能来自后端 source 字符串，进 innerHTML 前兜底） */
function escapeXmlText(s: string): string {
  return s.replace(/[&<>"']/g, (ch) => {
    switch (ch) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case '"':
        return "&quot;";
      default:
        return "&apos;";
    }
  });
}

/** 首字母徽标（深色圆底 + 白首字母）：minimax 小怪兽 path 在 18px 下糊成
 *  色块的降级方案，兼作未识别 source 的兜底（比裸圆点可辨识） */
function letterBadgeSvg(letter: string): string {
  return (
    `<svg viewBox="0 0 24 24" aria-hidden="true">` +
    `<circle cx="12" cy="12" r="12" fill="#33373c"/>` +
    `<text x="12" y="12.6" text-anchor="middle" dominant-baseline="central" ` +
    `fill="#fff" font-size="12" font-weight="700" ` +
    `font-family="inherit">${escapeXmlText(letter)}</text></svg>`
  );
}

/** source → 首字母（显示名优先，取首个字母/数字并大写；空串回 "?"） */
function initialOf(source: string): string {
  const label = SOURCE_LABELS[source] ?? source ?? "";
  const ch = (label.trim().charAt(0) || "?").toUpperCase();
  return /[A-Za-z0-9]/.test(ch) ? ch : "?";
}

/** 图标 SVG 字符串（viewBox + 单色 path；minimax 与未识别 source 回落
 *  首字母徽标） */
function iconSvg(source: string): string {
  const icon = AGENT_ICONS[source];
  if (icon) {
    return `<svg viewBox="${icon.viewBox}" aria-hidden="true">${icon.body}</svg>`;
  }
  return letterBadgeSvg(initialOf(source));
}

// ============================================================
// 轻量双语词典（仅停靠条内文案；语言偏好与主面板同键共享）
// ============================================================

interface Msg {
  empty: string;
  openMain: string;
  /** 详情卡副标题（卡头第二行弱色说明） */
  subtitle: string;
  /** 详情卡底部窗口数 */
  windows: (n: number) => string;
  /** 详情卡 Token 区文案 */
  tokens: string;
  today: string;
  day24h: string;
  /** 卡头右侧健康状态 */
  status: (used: number) => string;
  /** 额度行底部「剩余 x%」（中英语序不同） */
  remaining: (remain: number) => string;
  input: string;
  output: string;
  cache: string;
  requests: string;
  peak: string;
  /** 本地库全时段累计 */
  total: string;
  /** 底栏刷新说明 */
  live: string;
  /** 无小时分布时的占位 */
  noTrend: string;
  /** 有本地库、但最近 24h 没有调用 */
  noRecent: string;
  /** 已开启但暂无额度数据 */
  offline: string;
  offlineHint: string;
  /** 行 aria-label：完整口径（已用/剩余/重置） */
  rowTitle: (
    label: string,
    used: number,
    remain: number,
    reset: string
  ) => string;
}

const MESSAGES: Record<"zh" | "en", Msg> = {
  zh: {
    empty: "暂无数据",
    openMain: "打开主窗口",
    subtitle: "订阅额度与用量",
    windows: (n) => `${n} 个窗口`,
    tokens: "Token 用量",
    today: "今日",
    day24h: "24h",
    status: (used) => (used >= 90 ? "紧张" : used >= 70 ? "注意" : "正常"),
    remaining: (remain) => `剩余 ${remain}%`,
    input: "输入",
    output: "输出",
    cache: "缓存",
    requests: "24h 请求",
    peak: "峰值",
    total: "累计",
    live: "30s 刷新",
    noTrend: "暂无本地 Token 数据",
    noRecent: "24h 无本地调用",
    offline: "未连接",
    offlineHint: "未获取到实时额度，登录对应客户端后自动出现",
    rowTitle: (label, used, remain, reset) =>
      `${label} 已用 ${used}% · 剩余 ${remain}%${reset ? ` · ${reset} 后重置` : ""}`,
  },
  en: {
    empty: "No data",
    openMain: "Open Dashboard",
    subtitle: "Subscription quota & usage",
    windows: (n) => `${n} window${n === 1 ? "" : "s"}`,
    tokens: "Tokens",
    today: "Today",
    day24h: "24h",
    status: (used) => (used >= 90 ? "Tight" : used >= 70 ? "Watch" : "Healthy"),
    remaining: (remain) => `${remain}% left`,
    input: "Input",
    output: "Output",
    cache: "Cache",
    requests: "24h requests",
    peak: "Peak",
    total: "Total",
    live: "30s refresh",
    noTrend: "No local token data",
    noRecent: "No local calls in 24h",
    offline: "Offline",
    offlineHint: "No live quota yet. Sign in to the agent to populate it.",
    rowTitle: (label, used, remain, reset) =>
      `${label} used ${used}% · ${remain}% left${reset ? ` · resets in ${reset}` : ""}`,
  },
};

function detectMsg(): Msg {
  try {
    const v = localStorage.getItem(LOCALE_KEY);
    return v === "en" ? MESSAGES.en : MESSAGES.zh;
  } catch {
    return MESSAGES.zh;
  }
}
// 主题不跟随：停靠条恒定深色（hud.html 无亮色分支与 .dark 切换），
// 外观监听只保留语言同步，无 applyHudTheme

// ============================================================
// 展示辅助（口径与注入版 usage.js 对齐）
// ============================================================

/** 已用阈值分档（与注入版三档阈值同源：已用 <70 绿 / 70–89 黄 /
 *  ≥90 红，等价于剩余 >30 / 10–30 / <10）。环色、百分比、额度行与
 *  迷你轨道都用同一档色 */
function levelOf(usedPct: number): "hud-ok" | "hud-warn" | "hud-low" {
  if (usedPct >= 90) return "hud-low";
  if (usedPct >= 70) return "hud-warn";
  return "hud-ok";
}

/** 已用百分比取整并钳入 [0,100]（后端是 f64，环/数字/轨道宽度共用） */
function usedPctOf(quota: HudBarQuota): number {
  if (!Number.isFinite(quota.usedPct)) return 0;
  return Math.min(100, Math.max(0, Math.round(quota.usedPct)));
}

/** 某源全部窗口中已用% 最高的窗口（= 余量最紧；环色取该窗口的已用档） */
function tightestWindow(quotas: HudBarQuota[]): HudBarQuota | null {
  let best: HudBarQuota | null = null;
  for (const q of quotas) {
    if (!best || q.usedPct > best.usedPct) best = q;
  }
  return best;
}

/**
 * 重置倒计时格式化（口径与注入版 V31/V32 的额度倒计时一致）：
 * <1h "35m" / <24h "2h35m" / ≥24h "5d2h"；已过期（<=0）按 0 计。
 */
function fmtCountdown(resetAt: number | null): string {
  if (resetAt == null) return "";
  const ms = Math.max(0, resetAt - Date.now());
  const mins = Math.floor(ms / 60_000);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h${mins % 60}m`;
  return `${Math.floor(hours / 24)}d${hours % 24}h`;
}

/** 套餐名展示化：max → Max、LEVEL_BASIC → Basic、pro_plus → Pro Plus */
function planLabel(plan?: string): string {
  const raw = (plan ?? "").trim();
  if (!raw) return "";
  const value = raw
    .replace(/^level[_-]?/i, "")
    .replace(/[_-]+/g, " ")
    .toLowerCase();
  return value.replace(/(^|\s)([a-z0-9])/g, (_m, p: string, c: string) => p + c.toUpperCase());
}

/** 千分位计数（请求数用） */
function fmtCount(n: number): string {
  const v = Math.max(0, Number.isFinite(n) ? n : 0);
  try {
    return Math.round(v).toLocaleString(msg === MESSAGES.zh ? "zh-CN" : "en-US");
  } catch {
    return String(Math.round(v));
  }
}

/** 详情卡额度行（两行式）：
 *  头行 = 状态点 + 标签 + 已用%（同档语义色）；
 *  体行 = 迷你进度轨 + 「剩余 x% · 倒计时」。
 *  DOM API 构建：labelZh/labelEn/key 是后端字符串，textContent 天然转义；
 *  行 title 给出已用/剩余/重置的完整口径与绝对重置时刻。 */
function appendQuotaRow(parent: HTMLElement, q: HudBarQuota, msg: Msg): void {
  const used = usedPctOf(q);
  const remain = Math.max(0, 100 - used);
  const lv = levelOf(used);
  const labelText = msg === MESSAGES.zh ? q.labelZh : q.labelEn;
  const countdown = fmtCountdown(q.resetAt);
  const row = document.createElement("div");
  row.className = "hud-qrow";
  // aria-label 而非 title：卡片内不用原生黄底 tooltip（与整体视觉冲突），
  // 完整口径留给读屏
  row.setAttribute("aria-label", msg.rowTitle(labelText, used, remain, countdown));

  const head = document.createElement("div");
  head.className = "hud-qhead";
  const dot = document.createElement("i");
  dot.className = `hud-qdot ${lv}`;
  const label = document.createElement("span");
  label.className = "hud-qlabel";
  label.textContent = labelText;
  const value = document.createElement("span");
  value.className = `hud-qval ${lv}`;
  value.textContent = `${used}%`;
  head.append(dot, label, value);

  const body = document.createElement("div");
  body.className = "hud-qbody";
  const track = document.createElement("span");
  track.className = "hud-qtrack";
  const fill = document.createElement("span");
  fill.className = `hud-qfill ${lv}`;
  fill.style.width = `${used}%`;
  track.appendChild(fill);
  const meta = document.createElement("span");
  meta.className = "hud-qmeta";
  meta.textContent = `${msg.remaining(remain)}${countdown ? ` · ${countdown}` : ""}`;
  body.append(track, meta);

  row.append(head, body);
  parent.appendChild(row);
}

/** Token 数缩写（HUD 详情卡用；与注入版 fmtTokensShort 同口径） */
function fmtTokensShort(n: number): string {
  const v = Math.max(0, Number.isFinite(n) ? n : 0);
  if (v >= 1e9) return `${(v / 1e9).toFixed(v >= 1e10 ? 0 : 1)}B`;
  if (v >= 1e6) return `${(v / 1e6).toFixed(v >= 1e7 ? 0 : 1)}M`;
  if (v >= 1e3) return `${(v / 1e3).toFixed(v >= 1e4 ? 0 : 1)}k`;
  return String(Math.round(v));
}

/** Token 汇总按 agent 缓存 60s：详情卡 30s 重建/反复点击不重复扫库 */
const TOKEN_CACHE = new Map<string, { at: number; data: AgentTokenSummary }>();
const TOKEN_CACHE_TTL = 60_000;
/** 直方图柱数/绘图高度（逻辑 px）：固定尺寸，异步回填不改变卡片高度 */
const CHART_BARS = 24;
const CHART_H = 56;

/** 详情卡 Token 区：标题 + 24h 堆叠直方图 + 三段图例 + 今日/请求/峰值。
 *  先建固定高度占位再异步回填——卡片定位后若长高会被扩窗画布裁切。 */
function buildTokenSection(source: string): HTMLElement {
  const sec = document.createElement("div");
  sec.className = "hud-tokens";

  const head = document.createElement("div");
  head.className = "hud-tokens-head";
  const title = document.createElement("span");
  title.className = "hud-tokens-title";
  title.textContent = msg.tokens;
  const total = document.createElement("span");
  total.className = "hud-tokens-total";
  total.textContent = "—";
  head.append(title, total);
  sec.appendChild(head);

  const legend = document.createElement("div");
  legend.className = "hud-legend";
  for (const [cls, label, key] of [
    ["hud-seg-in", msg.input, "input"],
    ["hud-seg-out", msg.output, "output"],
    ["hud-seg-cache", msg.cache, "cache"],
  ] as const) {
    const item = document.createElement("span");
    item.className = "hud-legend-item";
    const swatch = document.createElement("i");
    swatch.className = cls;
    const text = document.createElement("span");
    text.textContent = label;
    const value = document.createElement("b");
    value.className = "hud-legend-value";
    value.dataset.seg = key;
    value.textContent = "—";
    item.append(swatch, text, value);
    legend.appendChild(item);
  }
  sec.appendChild(legend);

  const chart = document.createElement("div");
  chart.className = "hud-chart";
  for (let i = 0; i < CHART_BARS; i++) {
    const bar = document.createElement("span");
    bar.className = "hud-bar empty";
    chart.appendChild(bar);
  }
  const empty = document.createElement("div");
  empty.className = "hud-chart-empty";
  empty.textContent = msg.noTrend;
  chart.appendChild(empty);
  sec.appendChild(chart);

  const stats = document.createElement("div");
  stats.className = "hud-stats";
  for (const key of ["today", "requests", "peak"] as const) {
    const cell = document.createElement("div");
    cell.className = "hud-stat";
    cell.dataset.key = key;
    const label = document.createElement("span");
    label.className = "hud-stat-label";
    label.textContent =
      key === "today"
        ? msg.today
        : key === "requests"
          ? msg.requests
          : msg.peak;
    const value = document.createElement("b");
    value.className = "hud-stat-value";
    value.textContent = "—";
    cell.append(label, value);
    stats.appendChild(cell);
  }
  sec.appendChild(stats);

  hydrateTokenSection(sec, source);
  return sec;
}

/** 画直方图：每根柱按输入/输出/缓存三段堆叠；高度按三段之和归一到
 *  CHART_H。蓝天/紫/绿三色与图例一致，柱顶 tooltip 给出整点、总量与
 *  三段明细。 */
function paintChart(sec: HTMLElement, buckets: AgentTokenSummary["buckets"]): void {
  const chart = sec.querySelector<HTMLElement>(".hud-chart");
  if (!chart) return;
  chart.querySelectorAll(".hud-bar").forEach((el) => el.remove());
  chart.querySelector(".hud-chart-empty")?.classList.remove("on");
  const rows = buckets.slice(-CHART_BARS);
  if (rows.length === 0) {
    chart.querySelector(".hud-chart-empty")?.classList.add("on");
    return;
  }
  const maxTokens = Math.max(0, ...rows.map((b) => b.tokens));
  if (maxTokens <= 0) {
    chart.querySelector(".hud-chart-empty")?.classList.add("on");
    return;
  }
  for (let i = 0; i < rows.length; i++) {
    const b = rows[i];
    const total = Math.max(0, b.tokens);
    const bar = document.createElement("span");
    bar.className = "hud-bar";
    if (total <= 0) {
      bar.classList.add("empty");
      bar.style.height = "3px";
      chart.insertBefore(bar, chart.querySelector(".hud-chart-empty"));
      continue;
    }
    // 用「总 token」做权威分母，再按缓存/输出切出不重叠的三段：
    // 各来源的 input_tokens 是否含 cache 不一致（ZCode/Codex 含、
    // Claude 不含），直接相加会双计；这里令「输入 = 总量 − 缓存 − 输出」，
    // 三段之和恒等于总 token，柱子高度也与真实用量一致。
    const cache = Math.min(total, Math.max(0, b.cacheTokens));
    const output = Math.min(total - cache, Math.max(0, b.outputTokens));
    const input = Math.max(0, total - cache - output);
    const parts: Array<[string, number]> = [
      ["hud-seg-out", output],
      ["hud-seg-in", input],
      ["hud-seg-cache", cache],
    ];
    // aria-label 而非 title：卡片内不弹原生黄底 tooltip（与额度行同口径）
    bar.setAttribute(
      "aria-label",
      `${b.label} · ${fmtTokensShort(total)} tok · ${fmtCount(b.requests)} req · ` +
        `${msg.input} ${fmtTokensShort(input)} · ${msg.output} ${fmtTokensShort(
          output
        )} · ${msg.cache} ${fmtTokensShort(cache)}`
    );
    const barH = Math.max(3, (total / Math.max(1, maxTokens)) * CHART_H);
    const stack = document.createElement("span");
    stack.className = "hud-bar-stack";
    stack.style.height = `${barH}px`;
    stack.style.animationDelay = `${i * 9}ms`;
    for (const [cls, v] of parts) {
      if (v <= 0) continue;
      const seg = document.createElement("span");
      seg.className = `hud-bar-seg ${cls}`;
      seg.style.height = `${(v / total) * barH}px`;
      stack.appendChild(seg);
    }
    bar.appendChild(stack);
    chart.insertBefore(bar, chart.querySelector(".hud-chart-empty"));
  }
}

function paintTokenSection(sec: HTMLElement, data: AgentTokenSummary | null): void {
  const totalEl = sec.querySelector<HTMLElement>(".hud-tokens-total");
  const empty = sec.querySelector<HTMLElement>(".hud-chart-empty");
  const values = sec.querySelectorAll<HTMLElement>(".hud-stat-value");
  if (!data) {
    if (totalEl) totalEl.textContent = "—";
    values.forEach((el) => (el.textContent = "—"));
    empty?.classList.add("on");
    return;
  }
  const buckets = data.buckets ?? [];
  // 没有任何小时桶 = 该来源没有本地 token 库（Cursor 的用量在远端
  // events 接口），此时显示「—」而不是误导性的 0。有桶但 24h 全空
  // （如 OpenCode 只剩历史会话）则回落到"累计"，仍给出实据。
  const hasLibrary = buckets.length > 0;
  const recent = buckets.slice(-CHART_BARS);
  if (totalEl) {
    if (!hasLibrary) {
      totalEl.textContent = "—";
    } else {
      // 头部一行同时给出 24h 与累计：24h 为空时累计是唯一实据，
      // 不再占一个统计格（4 格在 304 宽下会把峰值挤成省略号）
      const parts: string[] = [];
      if (data.dayTokens > 0) {
        parts.push(`${msg.day24h} ${fmtTokensShort(data.dayTokens)}`);
      }
      if (
        data.totalTokens > 0 &&
        (data.dayTokens === 0 || data.totalTokens > data.dayTokens)
      ) {
        parts.push(`${msg.total} ${fmtTokensShort(data.totalTokens)}`);
      }
      totalEl.textContent = parts.length > 0 ? parts.join(" · ") : `${msg.day24h} 0`;
    }
  }
  if (empty) {
    empty.textContent =
      data.totalTokens > 0 ? msg.noRecent : msg.noTrend;
  }
  // 图例右侧补 24h 三段总量（输入/输出/缓存），与柱体同口径（不重叠、
  // 三段之和 = 总量；input 取「总量 − 缓存 − 输出」的剩余部分）
  const sums = { input: 0, output: 0, cache: 0 };
  for (const b of recent) {
    const bucketTotal = Math.max(0, b.tokens);
    const cache = Math.min(bucketTotal, Math.max(0, b.cacheTokens));
    const output = Math.min(bucketTotal - cache, Math.max(0, b.outputTokens));
    sums.input += Math.max(0, bucketTotal - cache - output);
    sums.output += output;
    sums.cache += cache;
  }
  sec.querySelectorAll<HTMLElement>(".hud-legend-value").forEach((el) => {
    const key = el.dataset.seg as keyof typeof sums | undefined;
    const v = key ? sums[key] : 0;
    el.textContent = v > 0 ? fmtTokensShort(v) : "—";
  });
  const peak = recent.reduce<AgentTokenSummary["buckets"][number] | null>(
    (best, b) => (!best || b.tokens > best.tokens ? b : best),
    null
  );
  const today = sec.querySelector<HTMLElement>('[data-key="today"] .hud-stat-value');
  const requests = sec.querySelector<HTMLElement>('[data-key="requests"] .hud-stat-value');
  const peakEl = sec.querySelector<HTMLElement>('[data-key="peak"] .hud-stat-value');
  if (today) {
    today.textContent = hasLibrary ? `${fmtTokensShort(data.todayTokens)} tok` : "—";
  }
  if (requests) {
    requests.textContent = hasLibrary ? fmtCount(data.dayRequests) : "—";
  }
  if (peakEl) {
    peakEl.textContent =
      peak && peak.tokens > 0 ? `${peak.label} · ${fmtTokensShort(peak.tokens)}` : "—";
  }
  if (buckets.length === 0) {
    empty?.classList.add("on");
    return;
  }
  paintChart(sec, buckets);
}

function hydrateTokenSection(sec: HTMLElement, source: string): void {
  const cached = TOKEN_CACHE.get(source);
  if (cached && Date.now() - cached.at < TOKEN_CACHE_TTL) {
    paintTokenSection(sec, cached.data);
    return;
  }
  sec.classList.add("loading");
  void getAgentTokenSummary(source)
    .then((data) => {
      TOKEN_CACHE.set(source, { at: Date.now(), data });
      paintTokenSection(sec, data);
    })
    .catch(() => {
      paintTokenSection(sec, null);
    })
    .finally(() => {
      sec.classList.remove("loading");
    });
}

// ============================================================
// 渲染与交互
// ============================================================

let msg: Msg = detectMsg();
let usage: HudBarUsage | null = null;
/** 当前布局：vertical（贴左/右）/ horizontal（贴顶） */
let orientation: "vertical" | "horizontal" =
  window.innerWidth > window.innerHeight ? "horizontal" : "vertical";
/** 当前停靠边（layout 事件携带；初始按 orientation 推断，事件到达后校正） */
let barEdge: HudBarLayout["edge"] = orientation === "horizontal" ? "top" : "left";
/** 详情卡打开的 source（null = 无详情） */
let detailSource: string | null = null;
/** 扩窗态（与 Rust EXPANDED 对应；invoke 失败按本地记忆幂等收敛） */
let expanded = false;
/** 鼠标是否在停靠条内（收起计时器的判定依据） */
let pointerInside = false;
/** 鼠标是否在详情卡内（hover 开卡后允许移到卡上操作，不立即收） */
let cardHovered = false;
/** hover 展开的短延迟（防扫过时误触发抽屉） */
let expandTimer: ReturnType<typeof setTimeout> | null = null;
/** hover 离开后的收起延迟（让指针在条/卡之间移动时不闪烁） */
let collapseTimer: ReturnType<typeof setTimeout> | null = null;
/** 指针离开条/卡后的收卡延迟 */
let cardCloseTimer: ReturnType<typeof setTimeout> | null = null;
/** 在条目间移动时的开卡防抖（避免扫过一排条目连续换卡闪动） */
let hoverSwitchTimer: ReturnType<typeof setTimeout> | null = null;
/** 已在排队等待开卡的条目（同一条目内的子元素切换不再重排计时器） */
let hoverPending: string | null = null;
/** 弹出层收起动画的清理计时器（动画播完再清 DOM/缩窗） */
let hideTimer: ReturnType<typeof setTimeout> | null = null;
/** 悬浮守护轮询：真实光标位置（DOM pointerleave 在透明扩窗画布上不可靠） */
let pointerWatch: ReturnType<typeof setInterval> | null = null;
let pointerCheckInflight = false;
/** 指针连续在条/卡之外的起始时刻（0 = 当前在范围内） */
let pointerOutsideSince = 0;
/** 详情卡打开序号：异步扩窗/等待动画期间被收起或切换时，旧流程作废 */
let showSeq = 0;
/** 扩窗请求序号：open/close 快速交错时只让最后一次结果落地 */
let popoverSeq = 0;
/** 扩窗后的目标视口尺寸（Rust 回执，逻辑 px；null = 未知/收缩态）。
 *  卡片定位前用它确认"扩窗真的落地了"，而不是靠肉眼帧数猜。 */
let popoverTarget: { w: number; h: number } | null = null;
/** 卡片入场前等几何收敛的上限（毫秒）：超过就按当下几何露出，
 *  宁可差一点也不能让卡迟迟不出现 */
const CARD_SETTLE_MS = 320;
/** 上一次卡片入场的 source/时刻：短时间内同源重建不再重放入场动画
 *  （动画从 0 重播看起来就是"卡片又冒出来一次"） */
let lastMount: { source: string; at: number } | null = null;
/** 抽屉展开动画的起始时刻（详情卡要等宽度过渡结束再锚定） */
let dockOpenedAt = 0;
/** 阈值拖拽会话（pointerdown 记录起点；null = 无拖拽会话） */
let drag: { x: number; y: number; started: boolean } | null = null;
/** 拖拽启动后需吞掉一次 click（pointerup 后 click 仍会派发——不吞会在
 *  松手时误开详情卡） */
let suppressClick = false;
/** 自定义拖拽进行中：越过阈值后接管，悬停/抽屉/卡片全部让位 */
let dragging = false;
/** 拖拽轮询定时器（16ms 一拍，Rust 侧按系统光标算落点） */
let dragTimer: ReturnType<typeof setInterval> | null = null;
/** 上一次 drag_move 还没回来（丢弃重复帧，下一帧自然重发） */
let dragInflight = false;
/** 拖拽后暂不响应悬停：松手时指针仍压在条上，立刻展开会很突兀；
 *  指针离开条或 600ms 后恢复 */
let hoverSuspended = false;
let hoverResumeTimer: ReturnType<typeof setTimeout> | null = null;

const rootEl = document.getElementById("hud-root")!;
const stripEl = document.getElementById("hud-strip")!;
const emptyEl = document.getElementById("hud-empty")!;
const popEl = document.getElementById("hud-pop")!;

function applyOrientation(next: "vertical" | "horizontal"): void {
  orientation = next;
  rootEl.classList.toggle("horizontal", next === "horizontal");
}

/** 停靠边 → 面板贴边方向：贴右时窗口扩窗向左平移，面板必须钉在窗口
 *  右缘（.edge-right），否则会随窗口一起左移离开屏缘 */
function applyEdge(next: HudBarLayout["edge"]): void {
  barEdge = next;
  rootEl.classList.toggle("edge-right", next === "right");
}

/** 单条目环 SVG：42px 语义圆环（外径 42、stroke 4、圆头、12 点起顺时
 *  针——rotate(-90) 把 dash 起点从 3 点挪到 12 点、底轨 --hud-ring-track）
 *  + 中心品牌图标（20px，见 .hud-agent-icon）。stroke 按"已用%"画
 *  （dasharray），颜色按已用档（<70 绿 / 70–89 黄 / ≥90 红） */
function ringSvg(usedPct: number, source: string, offline = false): string {
  const r = 18;
  const c = 2 * Math.PI * r;
  const used = Math.min(100, Math.max(0, usedPct));
  const dash = `${(used / 100) * c} ${c}`;
  const lv = levelOf(used);
  const strokeVar =
    lv === "hud-ok" ? "--hud-ok" : lv === "hud-warn" ? "--hud-warn" : "--hud-low";
  // 0% / 未连接不画进度弧：round linecap 对 0 长度 dash 会渲染成一个
  // 绿点，看起来像"在线健康"，与「—」语义冲突
  const arc =
    !offline && used > 0
      ? `<circle cx="21" cy="21" r="${r}" fill="none" stroke="var(${strokeVar})" stroke-width="4" ` +
        `stroke-linecap="round" stroke-dasharray="${dash}" transform="rotate(-90 21 21)"/>`
      : "";
  return (
    `<div class="hud-ring">` +
    `<svg viewBox="0 0 42 42" aria-hidden="true">` +
    `<circle cx="21" cy="21" r="${r}" fill="none" stroke="var(--hud-ring-track)" stroke-width="4"/>` +
    arc +
    `</svg>` +
    `<span class="hud-agent-icon">${iconSvg(source)}</span>` +
    `</div>`
  );
}

/** 重建条目列表（数据/语言/布局变化时全量重建，条目数 ≤ 6，开销可忽略）。
 *  条目 = 环 + 品牌图标 + 已用百分比（数字与环同档色）。空态占位节点常驻
 *  胶囊面板内部（#hud-empty 挂在 #hud-strip 下，空态撑起面板高度且自带
 *  拖拽属性可拖动），重建前先摘下、清空后再挂回 */
function renderItems(): void {
  // 与主面板设置页同一份可见性偏好（同源 localStorage）：设置里关掉的
  // agent 不在停靠条出现，开启的才渲染
  const visibility = loadAgentVisibility() as Record<string, boolean>;
  const agents = (usage?.agents ?? []).filter(
    (agent) => visibility[agent.source] !== false
  );
  emptyEl.remove();
  stripEl.innerHTML = "";
  let shown = 0;
  for (const agent of agents) {
    const tight = tightestWindow(agent.quotas);
    const offline = isOffline(agent);
    const used = tight ? usedPctOf(tight) : 0;
    const lv = levelOf(used);
    const item = document.createElement("div");
    item.className = offline ? "hud-item offline" : "hud-item";
    item.dataset.source = agent.source;
    item.setAttribute(
      "aria-label",
      offline
        ? `${SOURCE_LABELS[agent.source] ?? agent.source} · ${msg.offline}`
        : `${SOURCE_LABELS[agent.source] ?? agent.source} · ${used}%`
    );
    item.innerHTML =
      ringSvg(used, agent.source, offline) +
      `<span class="hud-pct ${offline ? "hud-off" : lv}">${
        offline ? "—" : `${used}%`
      }</span>`;
    stripEl.appendChild(item);
    shown++;
  }
  // 抽屉两态宽度：竖排只变宽（68 → 102），横排按条目数计算
  // （条目 50 → 84，条内 gap 恒 2）。宽度经 CSS 变量交给样式表过渡，
  // 窗口本身尺寸不变（Rust 侧仍按展开态条目数建窗）。
  const count = Math.max(1, shown);
  const gaps = (count - 1) * 2;
  const collapsedW = orientation === "horizontal" ? 18 + count * 50 + gaps : 68;
  const expandedW = orientation === "horizontal" ? 18 + count * 84 + gaps : 102;
  stripEl.style.setProperty("--hud-w-collapsed", `${collapsedW}px`);
  stripEl.style.setProperty("--hud-w-expanded", `${expandedW}px`);
  stripEl.appendChild(emptyEl);
  emptyEl.textContent = msg.empty;
  emptyEl.classList.toggle("on", shown === 0);
  // 重建后详情锚点可能失效，统一收起（数据 30s 一拍，罕见路径）
  if (detailSource && !agents.some((a) => a.source === detailSource)) {
    closeDetail();
  } else if (detailSource) {
    // 详情仍有效：补挂高亮（重建即丢）并重新锚定卡片
    const item = stripEl.querySelector<HTMLElement>(
      `.hud-item[data-source="${detailSource}"]`
    );
    if (item) {
      item.classList.add("active");
      const existing = popEl.querySelector<HTMLElement>(".hud-card");
      if (cardHovered && existing) {
        // 指针正在卡上：只重锚，不换 DOM，避免数据刷新把卡抽走导致闪
        placeCard(existing, item);
      } else {
        void showDetailCard(detailSource, item, true);
      }
    }
  }
}

function findAgent(source: string | null): HudBarAgent | null {
  if (!source) return null;
  return usage?.agents.find((a) => a.source === source) ?? null;
}

/** 数值钳入闭区间 [min(4, lo), hi]（4px 安全边距；窗口极窄时下限让位） */
function clampTo(v: number, hi: number): number {
  return Math.min(Math.max(4, v), Math.max(4, hi));
}

/** 卡片与面板之间的间隙（逻辑 px）：面板离屏缘也是 8，视觉上连成一体 */
const CARD_GAP = 8;

/** 卡片抽屉方向类：起点压回条的那一侧，条 z-index 更高 → 看起来像从
 *  面板里抽出来（贴左从左推、贴右从右推、贴顶从上推） */
function drawerClass(): string {
  if (orientation === "horizontal") return "drawer-t";
  return barEdge === "right" ? "drawer-r" : "drawer-l";
}

/**
 * 定位卡片（详情卡与 hover 卡共用），**按停靠边定向**：
 * - 竖排贴左（edge=left）：卡在面板右侧（left = strip.right + 8）；
 * - 竖排贴右（edge=right）：卡在面板左侧（left = strip.left − 8 − 卡宽）；
 * - 横排贴顶（edge=top）：卡在面板下方（top = strip.bottom + 8）。
 * 水平方向以**面板**边缘为基准（不是条目边缘——条目距面板内边距
 * 9px，按条目算会让卡片叠进面板 1px）；纵向跟随 hover 条目的中心，
 * 再钳进窗口（Rust 扩窗已按工作区余量在上下两侧补出画布，卡片贴条目
 * 显示不会被裁）。
 *
 * **纯定位**：要求卡已挂在 #hud-pop 内且画布已 .open（否则量不到卡宽/
 * 卡高）。**幂等**：重复调用只更新 left/top，不重挂节点、不重放动画
 * ——pointerover 在条目内部随子元素切换连续派发，重挂会让入场动画
 * 反复从头播（同一图标内持续移动时"闪动"的根因）。
 */
function placeCard(card: HTMLElement, item: HTMLElement): void {
  cancelHide();
  card.classList.remove("leaving");
  popEl.classList.add("open");
  const rect = item.getBoundingClientRect();
  const strip = stripEl.getBoundingClientRect();
  const cardW = card.offsetWidth;
  const cardH = card.offsetHeight;
  const maxX = window.innerWidth - cardW - 4;
  const maxY = window.innerHeight - cardH - 4;
  // 横向：贴左卡在条右侧、贴右卡在条左侧、贴顶跟随条目中心
  const left =
    orientation === "vertical"
      ? barEdge === "right"
        ? strip.left - CARD_GAP - cardW
        : strip.right + CARD_GAP
      : rect.left + rect.width / 2 - cardW / 2;
  // 纵向：竖排对条目中心，横排挂在条下方
  const top =
    orientation === "vertical"
      ? rect.top + rect.height / 2 - cardH / 2
      : strip.bottom + CARD_GAP;
  card.style.left = `${clampTo(left, maxX)}px`;
  card.style.top = `${clampTo(top, maxY)}px`;
}

/** 一帧后继续（rAF：与绘制节拍对齐，避免同一帧内连测多次） */
function nextFrame(): Promise<void> {
  return new Promise<void>((resolve) => {
    requestAnimationFrame(() => resolve());
  });
}

/**
 * 挂载 + 入场：先以 `.measuring`（visibility:hidden、无动画）挂进画布，
 * 再逐帧收敛 left/top，直到「几何连续两帧一致 + 扩窗后的视口尺寸到位」，
 * 最后撤掉 .measuring 露出并起抽屉动画。
 *
 * 为什么要先藏起来量：扩窗（OS）与视口 relayout（WebView）都是异步的，
 * setPopover 返回时视口可能还要变一两帧；若卡片这时已经可见，它会先按
 * 旧几何画出来再被挪走——看起来就是"卡片错乱地出现、像瞬间移动了几下"。
 * 隐藏态测量让"第一帧可见"必然是最终位置，代价只是多等一两帧
 * （16–32ms），肉眼不可见。
 */
async function mountCard(
  card: HTMLElement,
  item: HTMLElement,
  animate: boolean
): Promise<void> {
  cancelHide();
  popEl.classList.add("open");
  if (card.parentElement !== popEl) {
    card.classList.add(drawerClass());
    if (!animate) card.classList.add("no-anim");
    card.classList.add("measuring");
    popEl.appendChild(card);
  }
  const deadline = performance.now() + CARD_SETTLE_MS;
  let prev = "";
  for (;;) {
    if (!card.isConnected) return; // 收敛期间被收起/换卡
    placeCard(card, item);
    const cur = `${card.style.left}|${card.style.top}`;
    const stable = cur === prev;
    prev = cur;
    if (stable && viewportSettled()) break;
    if (performance.now() > deadline) break;
    await nextFrame();
  }
  card.classList.remove("measuring");
}

/** 扩窗后的目标视口是否已到位（Rust 回执的尺寸；取不到时视为已到位） */
function viewportSettled(): boolean {
  const t = popoverTarget;
  if (!t || !(t.w > 0) || !(t.h > 0)) return true;
  return (
    Math.abs(window.innerWidth - t.w) <= 2 &&
    Math.abs(window.innerHeight - t.h) <= 2
  );
}

/** 卡片骨架：品牌头像（38px 圆角瓦片）+ Agent 名/套餐徽标 + 副标题 +
 *  右侧健康状态胶囊。显示名/套餐/状态都走 textContent（后端字符串与
 *  用户套餐名不进 innerHTML）。 */
function buildCardShell(
  source: string,
  className: string,
  plan: string | undefined,
  tightestUsed: number,
  offline = false
): HTMLElement {
  const card = document.createElement("div");
  card.className = className;

  const head = document.createElement("div");
  head.className = "hud-card-head";

  const avatar = document.createElement("span");
  avatar.className = "hud-card-avatar";
  avatar.innerHTML = iconSvg(source);

  const title = document.createElement("div");
  title.className = "hud-card-title";
  const nameRow = document.createElement("div");
  nameRow.className = "hud-card-name-row";
  const name = document.createElement("span");
  name.className = "hud-card-name";
  name.textContent = SOURCE_LABELS[source] ?? source;
  nameRow.appendChild(name);
  const planText = planLabel(plan);
  if (planText) {
    const badge = document.createElement("span");
    badge.className = "hud-plan";
    badge.textContent = planText;
    nameRow.appendChild(badge);
  }
  const sub = document.createElement("div");
  sub.className = "hud-card-sub";
  sub.textContent = msg.subtitle;
  title.append(nameRow, sub);

  const lv = offline ? "hud-off" : levelOf(tightestUsed);
  const status = document.createElement("span");
  status.className = `hud-status ${lv}`;
  const dot = document.createElement("i");
  const statusText = document.createElement("span");
  statusText.textContent = offline ? msg.offline : msg.status(tightestUsed);
  status.append(dot, statusText);

  head.append(avatar, title, status);
  card.appendChild(head);
  return card;
}

/** 「打开主窗口」按钮（详情卡专用，带右箭头） */
function buildOpenMainBtn(): HTMLButtonElement {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "hud-open-btn";
  const label = document.createElement("span");
  label.textContent = msg.openMain;
  const arrow = document.createElement("svg");
  arrow.setAttribute("viewBox", "0 0 12 12");
  arrow.setAttribute("aria-hidden", "true");
  arrow.innerHTML =
    '<path d="M4.2 2.4 7.8 6l-3.6 3.6" fill="none" stroke="currentColor" ' +
    'stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>';
  btn.append(label, arrow);
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    // open_main_window 带 page 白名单（overview = 用量总览页）
    invoke("open_main_window", { page: "overview" }).catch(() => {});
  });
  return btn;
}

/** 组装详情卡：Agent 头（含套餐/健康态）+ 各窗口两行式额度行 + Token
 *  直方图区 + 底栏（窗口数/刷新说明 + 打开主窗口）。 */
function buildDetailCard(source: string): HTMLElement | null {
  const agent = findAgent(source);
  if (!agent) return null;
  const offline = isOffline(agent);
  const tight = tightestWindow(agent.quotas);
  const card = buildCardShell(
    source,
    "hud-card",
    agent.plan,
    tight ? usedPctOf(tight) : 0,
    offline
  );
  if (offline) {
    const empty = document.createElement("div");
    empty.className = "hud-offline";
    const title = document.createElement("b");
    title.textContent = msg.offline;
    const hint = document.createElement("span");
    hint.textContent = msg.offlineHint;
    empty.append(title, hint);
    card.appendChild(empty);
  } else {
    for (const q of agent.quotas) {
      appendQuotaRow(card, q, msg);
    }
  }
  card.appendChild(buildTokenSection(source));
  const foot = document.createElement("div");
  foot.className = "hud-card-foot";
  const meta = document.createElement("span");
  meta.className = "hud-foot-meta";
  meta.textContent = offline
    ? `${msg.offline} · ${msg.live}`
    : `${msg.windows(agent.quotas.length)} · ${msg.live}`;
  foot.append(meta, buildOpenMainBtn());
  card.appendChild(foot);
  return card;
}

/** 等视口与条几何真正落定：扩窗是 OS 异步完成的，WebView 的 relayout 还会
 *  再晚一两帧。判据是「Rust 回执的目标视口尺寸已到位 + 条的位置/尺寸连续
 *  两帧不变」，最多等 CARD_SETTLE_MS（取不到目标尺寸时退化为只等稳定）。 */
async function settleLayout(): Promise<void> {
  const deadline = performance.now() + CARD_SETTLE_MS;
  let prev = "";
  for (let i = 0; i < 24; i++) {
    const r = stripEl.getBoundingClientRect();
    const sig =
      `${window.innerWidth}x${window.innerHeight}|` +
      `${r.left},${r.top},${r.width},${r.height}`;
    if (sig === prev && viewportSettled()) return;
    prev = sig;
    if (performance.now() > deadline) return;
    await nextFrame();
  }
}

/** 扩窗/缩窗 invoke。**每次都真发**（不再按本地 expanded 短路）：页面在
 *  扩窗态被热更新/重载后，本地记忆归零而 Rust 仍展开，短路会让卡片在
 *  收缩窗里渲染（被裁/被顶到窗口外）——Rust 侧对同态请求本就是 O(1)
 *  早返回，多一次命令往返换取两端永不失步。返回值是扩窗后的目标视口
 *  尺寸，前端据此确认尺寸到位后再给卡片定位。 */
async function setPopover(open: boolean): Promise<void> {
  const seq = ++popoverSeq;
  expanded = open;
  try {
    const state = await invoke<HudPopoverState>("hud_bar_set_popover", {
      open,
    });
    // open/close 快速交错：旧请求的返回值不再落地，避免窗口被过期的
    // 扩窗结果留在展开态
    if (seq !== popoverSeq) return;
    popoverTarget = open ? { w: state.width, h: state.height } : null;
    // 首次打开：等扩窗后的视口真正落定，卡片再按最终几何定位/入场
    if (open) await settleLayout();
  } catch {
    /* 失败回退本地态：下一次操作重试（卡可能被窗口边界裁切，交互仍可用） */
    if (seq === popoverSeq) {
      expanded = !open;
      popoverTarget = null;
    }
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function clearExpandTimer(): void {
  if (expandTimer) {
    clearTimeout(expandTimer);
    expandTimer = null;
  }
}

function clearCollapseTimer(): void {
  if (collapseTimer) {
    clearTimeout(collapseTimer);
    collapseTimer = null;
  }
}

function clearCardCloseTimer(): void {
  if (cardCloseTimer) {
    clearTimeout(cardCloseTimer);
    cardCloseTimer = null;
  }
}

function clearHoverSwitchTimer(): void {
  if (hoverSwitchTimer) {
    clearTimeout(hoverSwitchTimer);
    hoverSwitchTimer = null;
  }
}

/** 真实光标是否落在某个客户端矩形内（2px 容差，缩放/圆角边缘不误判） */
function pointerInRect(
  x: number,
  y: number,
  rect: DOMRect | null
): boolean {
  if (!rect) return false;
  return (
    x >= rect.left - 2 &&
    x <= rect.right + 2 &&
    y >= rect.top - 2 &&
    y <= rect.bottom + 2
  );
}

function stopPointerWatch(): void {
  if (pointerWatch) {
    clearInterval(pointerWatch);
    pointerWatch = null;
  }
  pointerOutsideSince = 0;
}

/**
 * 悬浮守护：每 150ms 用 Rust GetCursorPos 的真实屏幕坐标判断指针是否
 * 还在条/卡上。DOM pointerleave 在"扩窗后整块透明画布都参与命中测试"
 * 的场景下并不可靠（指针停在空白区时不一定派发 leave），这是"移出去
 * 不折叠"的根因；真实坐标不依赖 WebView 的事件派发，也能识别贴顶扩宽
 * 时条布局位移导致的假 enter/leave 抖动。
 */
async function checkPointer(): Promise<void> {
  // 拖拽中：指针被窗口带着走，任何"离开条/卡"的判定都不成立，直接让位
  if (dragging) return;
  const dockOpen = rootEl.classList.contains("hud-open");
  if (!detailSource && !dockOpen && !expanded) {
    // 抽屉/卡片还在排队（hover 70ms 防扫过、条目切换 60ms 防抖、收起动画
    // 150ms）时别停守护：ensurePointerWatch 会立即跑一次本函数，那一刻
    // .hud-open 还没加上，一停就会让"移出去不折叠"的兜底整段失效
    if (
      !expandTimer &&
      !hoverPending &&
      !hideTimer &&
      !popEl.firstChild
    ) {
      stopPointerWatch();
    }
    return;
  }
  if (pointerCheckInflight) return;
  pointerCheckInflight = true;
  let state: HudBarPointerState;
  try {
    state = await invoke<HudBarPointerState>("hud_bar_pointer_state");
  } catch {
    pointerCheckInflight = false;
    return;
  }
  pointerCheckInflight = false;
  if (!Number.isFinite(state.scale) || state.scale <= 0) return;
  const x = (state.cursorX - state.windowX) / state.scale;
  const y = (state.cursorY - state.windowY) / state.scale;
  // 正在播收回动画的卡不算命中（它已不接鼠标，且即将被清掉）
  const card = popEl.querySelector<HTMLElement>(".hud-card:not(.leaving)");
  const overStrip = pointerInRect(x, y, stripEl.getBoundingClientRect());
  const overCard = pointerInRect(x, y, card?.getBoundingClientRect() ?? null);

  if (overStrip || overCard) {
    pointerOutsideSince = 0;
    clearCardCloseTimer();
    clearCollapseTimer();
    if (overStrip) {
      pointerInside = true;
      cardHovered = false;
      if (!rootEl.classList.contains("hud-open")) openDock();
    } else {
      pointerInside = false;
      cardHovered = true;
      if (!rootEl.classList.contains("hud-open")) {
        rootEl.classList.add("hud-open");
        dockOpenedAt = performance.now();
      }
    }
    return;
  }

  // 指针在窗口内的透明区（扩窗画布）或窗口外：都算离开条/卡
  pointerInside = false;
  cardHovered = false;
  if (pointerOutsideSince === 0) pointerOutsideSince = performance.now();
  // 卡片重建/异步测量时可能短暂取不到 rect，多给 400ms 宽限
  const grace = detailSource && !card ? 650 : 220;
  if (performance.now() - pointerOutsideSince < grace) return;
  pointerOutsideSince = 0;
  if (detailSource) {
    closeDetail();
    return;
  }
  if (dockOpen) closeDock(0);
  // 收起动画进行中：缩窗交给 hidePop 的计时器（立刻缩会把退出动画裁掉）
  if (expanded && !hideTimer) void setPopover(false);
  if (!detailSource && !rootEl.classList.contains("hud-open")) stopPointerWatch();
}

function ensurePointerWatch(): void {
  if (pointerWatch) return;
  pointerOutsideSince = 0;
  pointerWatch = setInterval(() => {
    void checkPointer();
  }, 150);
  void checkPointer();
}

/** 指针离开条/卡后延迟收卡；仍在条内或卡上则取消。
 *  260ms 足够从条目跨过 8px 间隙移到卡上，又不会让卡滞留。 */
function scheduleCardClose(waitMs = 260): void {
  clearCardCloseTimer();
  if (!detailSource || pointerInside || cardHovered) return;
  cardCloseTimer = setTimeout(() => {
    cardCloseTimer = null;
    if (pointerInside || cardHovered) return;
    closeDetail();
  }, waitMs);
}

/** 抽屉推入：条目宽度从环的位置向外过渡，数值由 overflow 裁切后
 *  拉出。hover 给 90ms 防扫过误触；点击详情立即展开 */
function openDock(immediate = false): void {
  if (dragging) return; // 拖拽中保持默认的收起态
  clearCollapseTimer();
  ensurePointerWatch();
  if (rootEl.classList.contains("hud-open")) return;
  if (immediate) {
    clearExpandTimer();
    rootEl.classList.add("hud-open");
    dockOpenedAt = performance.now();
    return;
  }
  if (expandTimer) return;
  expandTimer = setTimeout(() => {
    expandTimer = null;
    if (rootEl.classList.contains("hud-open")) return;
    rootEl.classList.add("hud-open");
    dockOpenedAt = performance.now();
  }, 70);
}

/** 抽屉收出：详情打开或指针仍在条内时不收 */
function closeDock(waitMs = 220): void {
  clearCollapseTimer();
  if (detailSource || pointerInside) return;
  collapseTimer = setTimeout(() => {
    collapseTimer = null;
    if (detailSource || pointerInside) return;
    rootEl.classList.remove("hud-open");
    dockOpenedAt = 0;
  }, waitMs);
}

/** 详情卡必须在抽屉宽度动画结束后再测量锚点，否则卡片会按半展开
 *  的条右缘定位、与最终面板重叠 */
async function ensureDockOpen(): Promise<void> {
  clearExpandTimer();
  clearCollapseTimer();
  if (!rootEl.classList.contains("hud-open")) {
    rootEl.classList.add("hud-open");
    dockOpenedAt = performance.now();
  }
  let reduce = false;
  try {
    reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    /* 老内核无 matchMedia：按有动画处理 */
  }
  const wait = reduce ? 0 : 300 - (performance.now() - dockOpenedAt);
  if (wait > 0) await delay(wait);
}

/** 取消仍在进行的收起动画（新卡要进来时调用：否则旧计时器到点后会把
 *  新卡一起清掉并缩窗） */
function cancelHide(): void {
  if (hideTimer) {
    clearTimeout(hideTimer);
    hideTimer = null;
  }
}

/** 收起弹出层：卡片先按抽屉方向推回条里（0.14s），动画播完再清节点、
 *  缩窗——先缩窗会把卡片瞬间裁掉，看不到退出动画。期间 .leaving 关闭
 *  pointer-events，避免残影挡住鼠标或被指针守护误判成"还在卡上" */
function hidePop(): void {
  if (hideTimer) return; // 已在收起动画中
  const card = popEl.querySelector<HTMLElement>(".hud-card");
  if (!card || !popEl.classList.contains("open")) {
    popEl.classList.remove("open");
    popEl.innerHTML = "";
    void setPopover(false);
    return;
  }
  card.classList.add("leaving");
  hideTimer = setTimeout(() => {
    hideTimer = null;
    popEl.innerHTML = "";
    popEl.classList.remove("open");
    void setPopover(false);
  }, 150);
}

/** 详情卡：品牌头（套餐/健康态）+ 逐窗口额度行 + 24h Token 直方图 +
 *  底栏（窗口数/刷新说明 + 打开主窗口）。
 *  悬浮条目即打开（同源重复 pointerover 只重定位、不重建）；force 供
 *  数据刷新时强制换内容。先等抽屉展开、再等 Rust 扩窗落地（含视口尺寸
 *  到位），最后才在隐藏态测量、收敛后露出并入场。 */
async function showDetailCard(
  source: string,
  item: HTMLElement,
  force = false
): Promise<void> {
  clearCardCloseTimer();
  ensurePointerWatch();
  cancelHide();
  const seq = ++showSeq;
  const existing = popEl.querySelector<HTMLElement>(".hud-card:not(.leaving)");
  if (!force && detailSource === source && existing) {
    placeCard(existing, item);
    return;
  }
  // 数据刷新（同一 Agent 重建卡）：内容变、位置不变，不重放抽屉动画
  const refreshing = existing !== null && detailSource === source;
  const card = buildDetailCard(source);
  if (!card) return;
  detailSource = source;
  // 条目高亮标明详情归属（closeDetail / 重建时移除）
  stripEl
    .querySelectorAll(".hud-item.active")
    .forEach((el) => el.classList.remove("active"));
  item.classList.add("active");
  await ensureDockOpen();
  // 等待动画/扩窗期间被收起或切到别的条目：本次流程作废
  if (seq !== showSeq || detailSource !== source) return;
  await setPopover(true);
  if (seq !== showSeq || detailSource !== source) return;
  popEl.innerHTML = "";
  // 刚入场过的同源卡重建（数据刷新/扩窗重试）：不重放推入动画
  const recent =
    lastMount !== null &&
    lastMount.source === source &&
    performance.now() - lastMount.at < 600;
  await mountCard(card, item, !refreshing && !recent);
  if (seq === showSeq && detailSource === source) {
    lastMount = { source, at: performance.now() };
  }
}

function closeDetail(): void {
  clearCardCloseTimer();
  clearHoverSwitchTimer();
  hoverPending = null;
  showSeq++; // 作废仍在等待动画/扩窗的打开流程
  cardHovered = false;
  detailSource = null;
  stripEl
    .querySelectorAll(".hud-item.active")
    .forEach((el) => el.classList.remove("active"));
  hidePop();
  closeDock(120);
}

/** 事件目标所在条目（closest 返回 Element，统一转回 HTMLElement 供锚定） */
function hitItem(e: Event): HTMLElement | null {
  const target = e.target as HTMLElement | null;
  return (target?.closest(".hud-item") as HTMLElement | null) ?? null;
}

// ---- 阈值拖拽（.hud-item 不带 drag-region 属性，手动接管）----
// 越过 4px 即接管为**自定义拖拽**：先把抽屉/详情卡/扩窗画布全部收掉
// （拖的是 118×条长 的收缩窗，不是整块透明画布），再由前端每帧把指针的
// 逻辑屏幕坐标发给 Rust（hud_bar_drag_move），由 drag_geometry 决定跟手
// 还是吸附到左/右/顶并切换横竖排——这就是"拖到边缘自动变形"。
// 松手后吞掉一次 click（pointerup 之后仍会派发）。
const DRAG_THRESHOLD_PX = 4;

/** 拖拽态复位：卡片/抽屉/画布/高亮一次性回到"无悬浮"的默认样子 */
function collapseForDrag(): void {
  clearCardCloseTimer();
  clearHoverSwitchTimer();
  clearExpandTimer();
  clearCollapseTimer();
  hoverPending = null;
  showSeq++;
  cardHovered = false;
  pointerInside = false;
  if (hoverResumeTimer) {
    clearTimeout(hoverResumeTimer);
    hoverResumeTimer = null;
  }
  detailSource = null;
  stripEl
    .querySelectorAll(".hud-item.active")
    .forEach((el) => el.classList.remove("active"));
  cancelHide();
  popEl.classList.remove("open");
  popEl.innerHTML = "";
  rootEl.classList.remove("hud-open");
  dockOpenedAt = 0;
}

/** 把 Rust 回执落到本地布局：朝向/停靠边变化时重渲染（条目宽度变量
 *  按横竖排重算），条会立刻变成该边的样子 */
function applyDragState(st: HudDragState): void {
  const nextOrientation: "vertical" | "horizontal" =
    st.orientation === "horizontal" ? "horizontal" : "vertical";
  const nextEdge = st.edge === "right" ? "right" : st.edge === "top" ? "top" : "left";
  if (nextOrientation === orientation && nextEdge === barEdge) return;
  applyOrientation(nextOrientation);
  applyEdge(nextEdge);
  renderItems();
}

/** 拖拽开始：先缩回画布（等 Rust 真正把窗口改回条尺寸），再交给 Rust
 *  按系统光标驱动。指针位置不再从前端事件取——跨屏时各屏缩放比不同，
 *  只有 GetCursorPos 是全局一致的；前端只负责"开始/结束"两个时机。 */
async function beginDrag(): Promise<void> {
  dragging = true;
  hoverSuspended = true;
  suppressClick = true;
  collapseForDrag();
  try {
    await setPopover(false);
    const st = await invoke<HudDragState>("hud_bar_drag_begin");
    // 松手发生在 begin 落地之前（快速拖一下又放开）：立刻结束会话，
    // 否则 Rust 侧 DRAGGING 会一直挂着、后续吸附全被挡掉
    if (!dragging) {
      await invoke("hud_bar_drag_end").catch(() => {});
      return;
    }
    applyDragState(st);
    if (dragTimer !== null) clearInterval(dragTimer);
    // 16ms 一拍轮询落点：不依赖 pointermove 是否送达（指针离开窗口后
    // 事件可能被系统吞掉，轮询系统光标永远准）
    dragTimer = setInterval(() => {
      void pollDrag();
    }, 16);
  } catch {
    // 失败退回普通点击语义：不吞 click、不挂起悬停；顺手收敛 Rust 会话
    dragging = false;
    suppressClick = false;
    hoverSuspended = false;
    void invoke("hud_bar_drag_end").catch(() => {});
  }
}

/** 轮询一帧落点（同一时刻只允许一个在途请求） */
async function pollDrag(): Promise<void> {
  if (!dragging || dragInflight) return;
  dragInflight = true;
  try {
    const st = await invoke<HudDragState>("hud_bar_drag_move");
    if (dragging) applyDragState(st);
  } catch {
    /* 单帧失败静默：下一帧重试 */
  }
  dragInflight = false;
}

/** 松手：Rust 做最终吸附与落盘，然后暂停悬停 350ms（避免拖完立刻弹卡） */
async function endDrag(): Promise<void> {
  if (!dragging) return;
  dragging = false;
  if (dragTimer !== null) {
    clearInterval(dragTimer);
    dragTimer = null;
  }
  try {
    const st = await invoke<HudDragState>("hud_bar_drag_end");
    applyDragState(st);
  } catch {
    /* 静默：位置已由最后一帧落地 */
  }
  if (hoverResumeTimer) clearTimeout(hoverResumeTimer);
  hoverResumeTimer = setTimeout(() => {
    hoverResumeTimer = null;
    hoverSuspended = false;
  }, 350);
}

stripEl.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  const item = hitItem(e);
  if (!item) return;
  // 新会话：清掉上一会话残留的吞 click 标志（陈旧 suppressClick 会把
  // 本次真实点击吞掉，要点两次才开详情）
  suppressClick = false;
  hoverSuspended = false;
  drag = { x: e.clientX, y: e.clientY, started: false };
});

document.addEventListener("pointermove", (e) => {
  if (!drag || drag.started) return;
  const dx = e.clientX - drag.x;
  const dy = e.clientY - drag.y;
  if (dx * dx + dy * dy >= DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX) {
    drag.started = true;
    void beginDrag();
  }
});

document.addEventListener("pointerup", () => {
  drag = null;
  void endDrag();
});
// 指针被系统收走 / 窗口失焦：兜底结束拖拽，避免卡在拖拽态
document.addEventListener("pointercancel", () => {
  drag = null;
  void endDrag();
});
window.addEventListener("blur", () => {
  drag = null;
  void endDrag();
});

stripEl.addEventListener("click", (e) => {
  const item = hitItem(e);
  if (!item) return;
  e.stopPropagation();
  // 阈值拖拽刚启动过：吞掉这一次 click（pointerup 后仍会派发）
  if (suppressClick) {
    suppressClick = false;
    return;
  }
  const source = item.dataset.source ?? "";
  if (source) {
    clearHoverSwitchTimer();
    void showDetailCard(source, item);
  }
});

// ---- hover：展开抽屉 + 直接显示该条目的详情卡；指针可移到卡上操作，
//      离开条/卡约 260ms 后收卡（不再需要先点击才出详情）----
stripEl.addEventListener("pointerover", (e) => {
  const item = hitItem(e);
  if (!item) return;
  if (drag && !drag.started) return; // 按下未超阈值：让位给点击/拖拽判定
  // 拖拽中/刚松手：不展开、不换卡（松手 600ms 后或指针离开再恢复）
  if (dragging || hoverSuspended) return;
  const source = item.dataset.source ?? "";
  if (!source) return;
  // 同一条目：已在显示或正在展开 → 直接返回。条目内部子元素（环/SVG/
  // 数字）之间移动会连续派发 pointerover，重排计时器会让卡迟迟不出现，
  // 重挂节点则会重放入场动画——这是"同一图标内移动时闪动"的根因。
  // 注意这里**不看卡是否已挂载**：showDetailCard 先置 detailSource 再
  // await 抽屉/扩窗，期间卡还不存在；若此时重排计时器，每次移动都会
  // 用新流程作废旧流程，卡会被无限推迟。
  if (detailSource === source) return;
  if (hoverPending === source) return;
  // 条目间移动防抖：指针在同一目标停留 60ms 才换卡，扫过整排时不闪
  hoverPending = source;
  clearHoverSwitchTimer();
  hoverSwitchTimer = setTimeout(() => {
    hoverSwitchTimer = null;
    hoverPending = null;
    if (!item.isConnected) return;
    void showDetailCard(source, item);
  }, 60);
});
stripEl.addEventListener("pointerenter", () => {
  if (dragging) return; // 拖拽中进入不算悬停
  pointerInside = true;
  clearCollapseTimer();
  clearCardCloseTimer();
  openDock();
});
stripEl.addEventListener("pointerleave", () => {
  pointerInside = false;
  hoverPending = null;
  hoverSuspended = false; // 离开条即恢复悬停（拖拽后不必再等计时器）
  clearExpandTimer();
  clearHoverSwitchTimer();
  scheduleCardClose(260);
  closeDock();
});
popEl.addEventListener("pointerover", () => {
  cardHovered = true;
  clearCardCloseTimer();
  clearCollapseTimer();
});
popEl.addEventListener("pointerout", (e) => {
  const to = e.relatedTarget as Node | null;
  if (to && popEl.contains(to)) return; // 卡片内部子元素之间移动
  cardHovered = false;
  if (to && stripEl.contains(to)) return; // 回到条上：由 pointerover 接管
  scheduleCardClose(220);
});
// 点击条目与卡片之外的区域：收起详情卡（缩窗）
document.addEventListener("pointerdown", (e) => {
  const t = e.target as HTMLElement;
  if (t.closest(".hud-card") || t.closest(".hud-item")) return;
  if (detailSource) closeDetail();
});

// 可见性偏好（主面板设置页）同源 localStorage：HUD 是独立 webview，
// 不保证能收到主面板的 window 事件，低频比对一次签名即可（3s、纯内存读，
// 开销可忽略）；变化时重渲染，隐藏/显示即时生效
let lastVisibilitySig = "";
function visibilitySignature(): string {
  try {
    return JSON.stringify(loadAgentVisibility());
  } catch {
    return "";
  }
}
lastVisibilitySig = visibilitySignature();
setInterval(() => {
  const next = visibilitySignature();
  if (next !== lastVisibilitySig) {
    lastVisibilitySig = next;
    renderItems();
  }
}, 3000);

// 倒计时不随数据事件刷新（Rust 只在额度变化时推），本地定时重绘打开的
// 详情卡；抽屉是否展开由 hover/详情态决定，不在这里改
setInterval(() => {
  if (!detailSource || cardHovered) return; // 指针在卡上时别换 DOM，防闪
  const source = detailSource;
  const card = buildDetailCard(source);
  if (!card) return;
  const item = stripEl.querySelector<HTMLElement>(
    `.hud-item[data-source="${source}"]`
  );
  if (!item) return;
  popEl.innerHTML = "";
  // 只是刷新倒计时：不重放抽屉动画（mountCard 隐藏态量一次即露出）
  void mountCard(card, item, false);
}, 30_000);

/** 视口变化（扩窗/缩窗落地、DPI 切换）后重锚当前卡：扩窗是 OS 异步完成
 *  的，resize 事件可能晚于挂载；这时卡片会停在按旧视口算出的位置。幂等
 *  重锚只改 left/top、不重放动画，位置没变时等于空操作。 */
let replantRaf = 0;
function replantCard(): void {
  if (replantRaf) return;
  replantRaf = requestAnimationFrame(() => {
    replantRaf = 0;
    if (!detailSource) return;
    const card = popEl.querySelector<HTMLElement>(
      ".hud-card:not(.leaving):not(.measuring)"
    );
    const item = stripEl.querySelector<HTMLElement>(
      `.hud-item[data-source="${detailSource}"]`
    );
    if (card && item) placeCard(card, item);
  });
}
window.addEventListener("resize", replantCard);

// ============================================================
// 主流程：语言初始化 + 事件监听（主题不跟随：恒定深色）
// ============================================================

const main = async (): Promise<void> => {
  applyOrientation(orientation);
  applyEdge(barEdge);
  renderItems();

  // 数据流：数据线程推送的额度（内容变化才推，事件停流保持最后内容）
  try {
    await listen<HudBarUsage>("zbar://hud-bar-usage", (e) => {
      usage = e.payload;
      renderItems();
    });
  } catch {
    /* 监听失败保持空态（与 pet/session-hud 同款降级） */
  }

  // 布局流：贴边吸附换边后的横竖排切换 + 停靠边（卡片定向依据）
  try {
    await listen<HudBarLayout>("zbar://hud-bar-layout", (e) => {
      const { orientation: nextOrientation, edge } = e.payload;
      if (nextOrientation === "vertical" || nextOrientation === "horizontal") {
        applyOrientation(nextOrientation);
        if (edge === "left" || edge === "right" || edge === "top") {
          applyEdge(edge);
        }
        // 布局翻转后条目几何/抽屉宽度变量都变了：重渲染并重锚详情卡
        renderItems();
      }
    });
  } catch {
    /* 静默：保持启动时按窗口横竖比自判的布局 */
  }

  // 扩窗态同步流：Rust 侧单方面缩窗（吸附强制缩回 / 托盘关闭）后广播
  // { open:false }——收敛本地扩窗态并收卡（详情开着也收），防失步
  try {
    await listen<{ open: boolean }>("zbar://hud-bar-popover", (e) => {
      if (!e.payload.open) {
        expanded = false;
        popoverTarget = null;
        if (detailSource) closeDetail();
        else hidePop();
        if (!pointerInside) closeDock(0);
      }
    });
  } catch {
    /* 静默：失去同步时本地态由下一次 hover/点击的幂等 setPopover 收敛 */
  }

  // 外观同步流：主面板语言切换后广播（重读 localStorage 对比；语言变化
  // 全量重渲染。主题不跟随——停靠条恒定深色，hud.html 无亮色分支）
  try {
    await listen(APPEARANCE_CHANGED_EVENT, () => {
      const next = detectMsg();
      if (next !== msg) {
        msg = next;
        renderItems();
      }
    });
  } catch {
    /* 静默：保持启动时读取的语言 */
  }

  // 自愈：热更新/页面重载可能发生在扩窗态（Rust 仍展开、本页记忆归零），
  // 启动时先收敛到收缩态，避免卡片渲染在收缩窗里被裁
  await setPopover(false);

  // 首帧保障：三路数据/布局/扩窗 listen 全部注册完成后强制数据线程重推
  // 一帧（数据线程按"内容未变"静默跳过时销毁重开/页面重载会永远空窗；
  // refresh 只置标志，线程 200ms 内响应，数据已有缓存时零 HTTP）
  invoke("hud_bar_refresh").catch(() => {});
};

void main();
