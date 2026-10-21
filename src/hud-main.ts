/**
 * HUD 停靠条宿主壳（hud-bar）。
 *
 * 与会话悬浮窗宿主壳（session-hud-main.ts）同款的零 React 薄壳：不引入
 * panel 的组件体系，条目渲染在壳内直接完成。职责：
 * - 数据流：监听 zbar://hud-bar-usage（hud_bar.rs 数据线程每 30s 组装、
 *   内容变化才推的各 Agent 额度），每个 Agent 一个条目——"语义环 +
 *   品牌图标"（参照 jazzenchen/agent-hud-open Simple 模式；**条上不出
 *   数字**，抬眼从环的比例与颜色读健康度，具体数值全在悬浮详情卡里）。
 *   **每个额度窗口一圈**（同心：外圈 5h/Auto/interval、往里周、月，最多
 *   三圈——见 RING_LAYOUTS）：每个环按自己的已用% 画、颜色按已用阈值分档
 *   （<70% 绿 / 70–89% 黄 / ≥90% 红，与注入版统计条的三档阈值同源：已用
 *   70% = 剩余 30%）；窗口多时中心品牌图标收小给内环让位。设置里已开启但
 *   本轮没取到额度的 Agent 也保留弱化条目（单圈底轨），不再整条消失；
 * - 布局流：监听 zbar://hud-bar-layout（payload = { orientation, edge }，
 *   贴边吸附换边后 Rust 推送）切换竖排 flex column（贴左/右）与横排
 *   flex row（贴顶），并记住停靠边（.edge-right 类让面板贴窗口右缘，
 *   卡片按边定位方向）；启动时按窗口横竖比自判兜底（窗口尺寸由 Rust
 *   bar_metrics 按停靠边预置：竖排恒 68 逻辑宽、横排恒 70 逻辑高，条长
 *   下限 96 保证两者可分；layout 事件在页面未就绪时丢失也不怕）；
 * - 悬浮详情卡：pointerenter 条 → 70ms 后给 #hud-root 加 .hud-open
 *   （只驱动悬停高亮，两种朝向都不做抽屉展开——窗口宽度按收起态收死），
 *   悬浮到某个条目就直接显示该 Agent 的详情卡，不用先点击。
 *   卡片内容：品牌头像 + 名称/套餐/健康态 + 每窗口两行式额度行（状态
 *   点/标签/已用%、进度轨/剩余/倒计时）+ Token 区（24h 输入/输出/缓存堆叠
 *   直方图、今日/请求/峰值）+ 底部「打开主窗口」按钮 → invoke
 *   open_main_window("overview")。余额型条目（dsh = DeepSeek 官方余额）
 *   没有百分比窗口：额度区换成余额行（金额 + 赠送/充值拆分）。指针可移到
 *   卡上操作，离开条/卡约 260ms
 *   后收卡。**卡片渲染在独立的 hud-card 窗口里**（同一份 bundle 按窗口
 *   label 分派成"卡模式"，见文件末尾）：条窗口全程不改几何，所以悬浮时
 *   不会出现"扩窗那一帧整条瞬移"的闪现（WebView2 的渲染子窗口锚定父窗口
 *   左上角，父窗口一改尺寸/位置，重排完成前那一帧就会把整条画到新窗口
 *   左上角，实机录屏逐帧抓到过）；卡片窗口在自己内容 measuring 隐藏时改
 *   尺寸/移位，摆好视口再露出并播抽屉动画；
 * - 详情态同步：监听 zbar://hud-bar-popover（Rust 侧吸附强制回位/托盘关闭
 *   时单方面收卡后广播 { open:false }），只收敛本地状态——卡片窗口已被
 *   Rust 隐藏，条页不回发 close（防两个方向来回乒乓）；
 * - 首帧保障：三路 listen 全部注册完成后 invoke hud_bar_refresh（数据
 *   线程按"内容未变"静默跳过时，refresh 强制 200ms 内重推一帧，销毁重开
 *   /页面重载不丢首帧）；
 * - 空态：无数据时显示一条占位小字（"暂无数据"），不闪窗；
 * - 拖拽：条目 .hud-item 不带原生拖拽属性（会吞点击），改为阈值自定义
 *   拖拽——越过 4px 后先把抽屉/详情卡全部收掉，再由前端逐帧把
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
import { getCurrentWindow } from "@tauri-apps/api/window";
import { APPEARANCE_CHANGED_EVENT } from "./appearance";
import { getAgentTokenSummary, type AgentTokenSummary } from "./api";
import { loadAgentVisibility } from "./agentVisibility";
import { LOCALE_KEY } from "./i18n/locale";

/** 详情卡窗口 label（与 Rust hud_bar::HUD_CARD_WINDOW_LABEL 同步）：同一份
 *  bundle 按窗口 label 决定跑"条"还是"卡"——卡片是独立窗口，见 Rust 侧
 *  的建窗注释（条窗口一旦为卡片改几何，WebView2 会在那一帧把整条画到
 *  新窗口左上角，就是"首次悬浮闪现移动"） */
const HUD_CARD_WINDOW_LABEL = "hud-card";
const CARD_MODE = getCurrentWindow().label === HUD_CARD_WINDOW_LABEL;
/** 卡片窗口四周留白（逻辑 px）：卡片在窗口内的偏移 = 投影可用的全部
 *  空间；留白越小，卡片四周那圈"透明边框"越窄。与 Rust hud_bar::CARD_PAD
 *  同步（改一处必须改另一处，否则卡片会被窗口裁切） */
const CARD_PAD = 8;
const CARD_OPEN_EVENT = "zbar://hud-card-open";
const CARD_CLOSE_EVENT = "zbar://hud-card-close";

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

/** 按量计费余额（后端 ProviderQuotaBalance 同字段；当前只有 dsh =
 *  DeepSeek 开放平台官方余额） */
interface HudBarBalance {
  amount: number;
  currency: string;
  granted?: number;
  toppedUp?: number;
}

/** 单个 Agent 条目（source 与图标键一致；plan 为后端可选下发的套餐名） */
interface HudBarAgent {
  source: string;
  plan?: string;
  /** 后端状态：缺省 = 实时额度正常；"unavailable" = 已开启但暂无额度 */
  status?: "unavailable";
  quotas: HudBarQuota[];
  /** 余额型条目（dsh）：有值时 quotas 恒空，条目显示官方余额而不是百分比 */
  balance?: HudBarBalance;
}

/** 余额型条目（有余额、且没有百分比窗口） */
function isBalanceAgent(agent: HudBarAgent): boolean {
  return agent.balance != null && agent.quotas.length === 0;
}

/** 开启但没取到额度的条目（未登录 / 凭证过期 / 网络失败）；余额型条目
 *  有余额即在线，不算离线 */
function isOffline(agent: HudBarAgent): boolean {
  if (agent.status === "unavailable") return true;
  return agent.quotas.length === 0 && !isBalanceAgent(agent);
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

/** 卡片在屏幕上的矩形（**物理 px**，与光标同口径）：Rust 摆好卡片窗口后
 *  回传，条页的悬浮守护据此判断"指针还在卡上"（卡片是独立窗口，DOM 事件
 *  看不到另一边） */
interface HudCardRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Rust hud_bar_pointer_state：光标与窗口左上角的全局物理坐标 + 卡片矩形 */
interface HudBarPointerState {
  cursorX: number;
  cursorY: number;
  windowX: number;
  windowY: number;
  scale: number;
  card: HudCardRect | null;
}

/** zbar://hud-card-open 的 payload（Rust 推给卡片窗口） */
interface HudCardOpenPayload {
  source: string;
  edge: "left" | "right" | "top";
  animate: boolean;
}

/** hud_card_measure 的返回：卡片窗口的目标视口尺寸（CSS px） */
interface HudCardViewport {
  width: number;
  height: number;
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
  // DeepSeek Harness mark：鲸鱼（与 BrandIcon.tsx 的 dsh 路径逐字节一致；
  // 眼睛是反向绕行的子路径，nonzero 填充规则下挖空）
  dsh: {
    viewBox: "0 0 24 24",
    body:
      '<path fill="currentColor" d="M7.9 8.4h7.8a4.3 4.3 0 0 1 0 8.6H7.9a4.3 4.3 0 0 1 0-8.6ZM17.5 10 22.8 6.2v6.4ZM9.9 3.6a1.2 1.2 0 1 1-2.4 0 1.2 1.2 0 0 1 2.4 0ZM13.9 5.4a.9.9 0 1 1-1.8 0 .9.9 0 0 1 1.8 0ZM7.5 12.2a1.1 1.1 0 1 0 2.2 0 1.1 1.1 0 0 0-2.2 0Z"/>',
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
  dsh: "DeepSeek Harness",
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
  /** 余额型条目（dsh = DeepSeek 官方余额）：区块标题与拆分标签 */
  balance: string;
  granted: string;
  toppedUp: string;
  /** 余额型条目的副标题（替代"订阅额度与用量"） */
  balanceSubtitle: string;
  /** 已开启但没有配置 DeepSeek Key 时的提示 */
  balanceHint: string;
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
    balance: "官方余额",
    granted: "赠送",
    toppedUp: "充值",
    balanceSubtitle: "DeepSeek 官方余额与用量",
    balanceHint: "未配置 DeepSeek API Key，在主窗口 DSH 面板里添加后自动显示余额",
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
    balance: "Balance",
    granted: "Granted",
    toppedUp: "Top-up",
    balanceSubtitle: "DeepSeek balance & usage",
    balanceHint: "No DeepSeek API key yet. Add one in the DSH panel to show the balance.",
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

/** 币种 → 显示符号（后端给 "CNY"/"USD" 或已是符号；未知币种原样前缀） */
function currencySymbol(currency: string): string {
  const c = (currency ?? "").trim();
  const upper = c.toUpperCase();
  if (upper === "CNY" || upper === "RMB" || c === "¥" || c === "￥") return "¥";
  if (upper === "USD" || c === "$") return "$";
  if (upper === "EUR" || c === "€") return "€";
  if (upper === "GBP" || c === "£") return "£";
  return upper ? `${upper} ` : "¥";
}

/** 余额完整值（详情卡）：两位小数 + 千分位 */
function fmtMoneyFull(amount: number, currency: string): string {
  const sym = currencySymbol(currency);
  const v = Number.isFinite(amount) ? Math.max(0, amount) : 0;
  try {
    return `${sym}${v.toLocaleString(msg === MESSAGES.zh ? "zh-CN" : "en-US", {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    })}`;
  } catch {
    return `${sym}${v.toFixed(2)}`;
  }
}

/** 详情卡余额行（dsh = DeepSeek 官方余额）：头行「官方余额 + 金额」，
 *  体行「赠送 x · 充值 y」（后端有拆分时才出）。 */
function appendBalanceRow(
  parent: HTMLElement,
  balance: HudBarBalance,
  t: Msg
): void {
  const wrap = document.createElement("div");
  wrap.className = "hud-balance";
  const head = document.createElement("div");
  head.className = "hud-balance-head";
  const label = document.createElement("span");
  label.className = "hud-balance-label";
  label.textContent = t.balance;
  const value = document.createElement("span");
  value.className = "hud-balance-value";
  value.textContent = fmtMoneyFull(balance.amount, balance.currency);
  head.append(label, value);
  wrap.appendChild(head);
  const parts: string[] = [];
  if (balance.granted != null) {
    parts.push(`${t.granted} ${fmtMoneyFull(balance.granted, balance.currency)}`);
  }
  if (balance.toppedUp != null) {
    parts.push(`${t.toppedUp} ${fmtMoneyFull(balance.toppedUp, balance.currency)}`);
  }
  if (parts.length > 0) {
    const sub = document.createElement("div");
    sub.className = "hud-balance-sub";
    sub.textContent = parts.join(" · ");
    wrap.appendChild(sub);
  }
  parent.appendChild(wrap);
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
 *  先建固定高度占位再异步回填——卡片定位后若长高会把窗口撑大（独立窗
 *  口按卡片实测尺寸落位，占位先撑住高度就不会出现"露出后再长高"）。 */
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
/** 详情卡打开的 source（null = 无详情）。卡片本体在**另一个窗口**，这里
 *  只保留"哪一条开着"用于高亮与收卡判定 */
let detailSource: string | null = null;
/** 鼠标是否在停靠条内（收起计时器的判定依据） */
let pointerInside = false;
/** 鼠标是否在详情卡内（hover 开卡后允许移到卡上操作，不立即收） */
let cardHovered = false;
/** hover 的短延迟（防扫过时误触发 .hud-open 标记与详情卡） */
let expandTimer: ReturnType<typeof setTimeout> | null = null;
/** hover 离开后的收起延迟（让指针在条/卡之间移动时不闪烁） */
let collapseTimer: ReturnType<typeof setTimeout> | null = null;
/** 指针离开条/卡后的收卡延迟 */
let cardCloseTimer: ReturnType<typeof setTimeout> | null = null;
/** 在条目间移动时的开卡防抖（避免扫过一排条目连续换卡闪动） */
let hoverSwitchTimer: ReturnType<typeof setTimeout> | null = null;
/** 已在排队等待开卡的条目（同一条目内的子元素切换不再重排计时器） */
let hoverPending: string | null = null;
/** 悬浮守护轮询：真实光标位置（卡片在另一个窗口，DOM 事件看不到） */
let pointerWatch: ReturnType<typeof setInterval> | null = null;
let pointerCheckInflight = false;
/** 指针连续在条/卡之外的起始时刻（0 = 当前在范围内） */
let pointerOutsideSince = 0;
/** 详情卡打开序号：异步流程（等 Rust 回执/卡片落位）期间被收起或切换时，旧流程作废 */
let showSeq = 0;
/** 上一次卡片入场的 source/时刻：短时间内同源重建不再重放入场动画
 *  （动画从 0 重播看起来就是"卡片又冒出来一次"） */
let lastMount: { source: string; at: number } | null = null;
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

/** 停靠边 → 面板贴边方向：贴右时面板必须钉在窗口
 *  右缘（.edge-right），否则会随窗口一起左移离开屏缘 */
function applyEdge(next: HudBarLayout["edge"]): void {
  barEdge = next;
  rootEl.classList.toggle("edge-right", next === "right");
}

/** 多窗口同心环的几何档位：[窗口数 − 1] → 半径（外→内）/ 统一描边 /
 *  中心图标边长。窗口越多环越靠里，必须把图标收小给内环让位——32px 的
 *  环里三圈 + 15px 图标会互相压住（42 视口下 15px 图标对角线半径 ≈13.9，
 *  正好撞上第二圈）。数值以 viewBox 42 为单位，渲染统一 32px。 */
const RING_LAYOUTS: { radii: number[]; stroke: number; icon: number }[] = [
  { radii: [18], stroke: 4, icon: 15 },
  // 双环：内环内缘 ≈ 12.1 视口单位（9.2px 半径），11.5px 图标对角线半径
  // 8.1px，留 1.1px 余量——满底品牌块（zai 的白色圆角方）也不会贴住内环
  { radii: [18, 13.5], stroke: 2.8, icon: 11.5 },
  { radii: [18, 14.4, 10.8], stroke: 2.3, icon: 10 },
];

/** 单条目环 SVG：**每个额度窗口一圈**（同心，外圈 = 后端窗口顺序里的第一个，
 *  即 5h / Auto / interval，往里依次是周、月）。12 点起顺时针（rotate(-90)
 *  把 dash 起点从 3 点挪到 12 点），底轨 --hud-ring-track，进度弧按"已用%"
 *  画（dasharray）、颜色按各自已用档（<70 绿 / 70–89 黄 / ≥90 红）。
 *  余额型条目没有窗口：只留一圈底轨；未连接同理（不画弧）。 */
function ringSvg(
  quotas: HudBarQuota[],
  source: string,
  offline = false
): string {
  const wins = quotas.slice(0, RING_LAYOUTS.length);
  const layout = RING_LAYOUTS[Math.max(0, wins.length - 1)];
  const { stroke } = layout;
  let arcs = "";
  if (wins.length === 0) {
    // 余额型 / 空数据：一圈底轨，图标按单环尺寸
    arcs = `<circle cx="21" cy="21" r="18" fill="none" stroke="var(--hud-ring-track)" stroke-width="4"/>`;
  }
  wins.forEach((q, i) => {
    const r = layout.radii[i];
    const c = 2 * Math.PI * r;
    const used = offline ? 0 : usedPctOf(q);
    const lv = levelOf(used);
    const strokeVar =
      lv === "hud-ok"
        ? "--hud-ok"
        : lv === "hud-warn"
          ? "--hud-warn"
          : "--hud-low";
    arcs +=
      `<circle cx="21" cy="21" r="${r}" fill="none" stroke="var(--hud-ring-track)" stroke-width="${stroke}"/>` +
      // 0% / 未连接不画进度弧：round linecap 对 0 长度 dash 会渲染成一个
      // 绿点，看起来像"在线健康"，与「—」语义冲突
      (used > 0
        ? `<circle cx="21" cy="21" r="${r}" fill="none" stroke="var(${strokeVar})" stroke-width="${stroke}" ` +
          `stroke-linecap="round" stroke-dasharray="${(used / 100) * c} ${c}" transform="rotate(-90 21 21)"/>`
        : "");
  });
  return (
    `<div class="hud-ring" style="--hud-icon-size:${layout.icon}px">` +
    `<svg viewBox="0 0 42 42" aria-hidden="true">${arcs}</svg>` +
    `<span class="hud-agent-icon">${iconSvg(source)}</span>` +
    `</div>`
  );
}

/** 重建条目列表（数据/语言/布局变化时全量重建，条目数 ≤ 8，开销可忽略）。
 *  条目 = 环 + 品牌图标，**条上不出数字**（百分比/余额全进悬浮详情卡，
 *  两种朝向一致）；余额型条目（dsh）环内不画进度弧。空态占位节点常驻
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
    const balance = isBalanceAgent(agent) ? agent.balance! : null;
    const offline = isOffline(agent);
    const label = SOURCE_LABELS[agent.source] ?? agent.source;
    // 条的环面：每个窗口一圈（最多三圈，与 RING_LAYOUTS 对齐）；余额型
    // 条目 windows 为空，只剩单圈底轨
    const quotas = offline ? [] : agent.quotas.slice(0, RING_LAYOUTS.length);
    const quotaText = quotas
      .map(
        (q) =>
          `${msg === MESSAGES.en ? q.labelEn : q.labelZh} ${usedPctOf(q)}%`
      )
      .join(" · ");
    const item = document.createElement("div");
    item.className = offline ? "hud-item offline" : "hud-item";
    item.dataset.source = agent.source;
    // 完整口径留给读屏（条上不显示文字，数字只在详情卡里）
    item.setAttribute(
      "aria-label",
      offline
        ? `${label} · ${msg.offline}`
        : balance
          ? `${label} · ${msg.balance} ${fmtMoneyFull(balance.amount, balance.currency)}`
          : `${label} · ${quotaText}`
    );
    item.innerHTML = ringSvg(quotas, agent.source, offline);
    stripEl.appendChild(item);
    shown++;
  }
  // 面板宽度：**两种朝向都不展开**（横排按条目数撑开、竖排恒 56），
  // 窗口尺寸由 Rust bar_metrics 维护，这里只把宽度写进 CSS 变量。
  const count = Math.max(1, shown);
  const gaps = (count - 1) * 2;
  const collapsedW = orientation === "horizontal" ? 16 + count * 40 + gaps : 56;
  stripEl.style.setProperty("--hud-w-collapsed", `${collapsedW}px`);
  stripEl.appendChild(emptyEl);
  emptyEl.textContent = msg.empty;
  emptyEl.classList.toggle("on", shown === 0);
  // 重建后详情锚点可能失效，统一收起（数据 30s 一拍，罕见路径）
  if (detailSource && !agents.some((a) => a.source === detailSource)) {
    closeDetail();
  } else if (detailSource) {
    // 详情仍有效：补挂高亮（重建即丢）并按新条目几何重开一次卡（卡片页
    // 按新数据重渲染、不重放入场动画，位置跟着条目走）
    const item = stripEl.querySelector<HTMLElement>(
      `.hud-item[data-source="${detailSource}"]`
    );
    if (item) {
      item.classList.add("active");
      void showDetailCard(detailSource, item, true);
    }
  }
}

function findAgent(source: string | null): HudBarAgent | null {
  if (!source) return null;
  return usage?.agents.find((a) => a.source === source) ?? null;
}

/** 卡片骨架：品牌头像（34px 圆角瓦片）+ Agent 名/套餐徽标 + 副标题 +
 *  右侧健康状态胶囊。显示名/套餐/状态都走 textContent（后端字符串与
 *  用户套餐名不进 innerHTML）。余额型条目（dsh）没有百分比健康度，
 *  右侧胶囊改显示「官方余额」。 */
function buildCardShell(
  source: string,
  className: string,
  plan: string | undefined,
  tightestUsed: number,
  offline = false,
  balance: HudBarBalance | null = null
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
  sub.textContent = balance ? msg.balanceSubtitle : msg.subtitle;
  title.append(nameRow, sub);

  const lv = offline ? "hud-off" : balance ? "hud-ok" : levelOf(tightestUsed);
  const status = document.createElement("span");
  status.className = `hud-status ${lv}`;
  const dot = document.createElement("i");
  const statusText = document.createElement("span");
  statusText.textContent = offline
    ? msg.offline
    : balance
      ? msg.balance
      : msg.status(tightestUsed);
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
  const balance = isBalanceAgent(agent) ? agent.balance! : null;
  const tight = tightestWindow(agent.quotas);
  const card = buildCardShell(
    source,
    "hud-card",
    agent.plan,
    tight ? usedPctOf(tight) : 0,
    offline,
    balance
  );
  if (offline) {
    const empty = document.createElement("div");
    empty.className = "hud-offline";
    const title = document.createElement("b");
    title.textContent = msg.offline;
    const hint = document.createElement("span");
    // 余额型条目（dsh）的"未连接"是没录 DeepSeek API Key，不是客户端登录态
    hint.textContent = source === "dsh" ? msg.balanceHint : msg.offlineHint;
    empty.append(title, hint);
    card.appendChild(empty);
  } else if (balance) {
    appendBalanceRow(card, balance, msg);
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
    : balance
      ? `${msg.balance} · ${msg.live}`
      : `${msg.windows(agent.quotas.length)} · ${msg.live}`;
  foot.append(meta, buildOpenMainBtn());
  card.appendChild(foot);
  return card;
}

/** 卡片抽屉方向类：起点压回条的那一侧，条 z-index 更高 → 看起来像从
 *  面板里抽出来（贴左从左推、贴右从右推、贴顶从上推） */
function drawerClass(): string {
  if (orientation === "horizontal") return "drawer-t";
  return barEdge === "right" ? "drawer-r" : "drawer-l";
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
 * 还在条/卡上。卡片在**另一个窗口**，DOM 事件天然跨不过去（指针移到卡上
 * 时条只会收到 pointerleave），所以"指针还在卡上就别收卡"必须靠真实
 * 坐标 + Rust 记录的卡片屏幕矩形；真实坐标也不受 WebView 事件派发抖动
 * 影响，是"移出去不折叠/移上去不误收"的统一兜底。
 */
async function checkPointer(): Promise<void> {
  // 拖拽中：指针被窗口带着走，任何"离开条/卡"的判定都不成立，直接让位
  if (dragging) return;
  const dockOpen = rootEl.classList.contains("hud-open");
  if (!detailSource && !dockOpen) {
    // 抽屉/卡片还在排队（hover 70ms 防扫过、条目切换 60ms 防抖）时别停
    // 守护：ensurePointerWatch 会立即跑一次本函数，那一刻 .hud-open 还没
    // 加上，一停就会让"移出去不折叠"的兜底整段失效
    if (!expandTimer && !hoverPending) stopPointerWatch();
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
  const overStrip = pointerInRect(x, y, stripEl.getBoundingClientRect());
  const overCard = cardContains(state.card, state.cursorX, state.cursorY);

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
      }
    }
    return;
  }

  // 指针既不在条上也不在卡上：超宽限就收
  pointerInside = false;
  cardHovered = false;
  if (pointerOutsideSince === 0) pointerOutsideSince = performance.now();
  // 卡片还没落位（刚 open、窗口尺寸在途）：多给一段宽限，别把刚开的卡收掉
  const grace = detailSource && !state.card ? 650 : 220;
  if (performance.now() - pointerOutsideSince < grace) return;
  pointerOutsideSince = 0;
  if (detailSource) {
    closeDetail();
    return;
  }
  if (dockOpen) closeDock(0);
  if (!rootEl.classList.contains("hud-open")) stopPointerWatch();
}

/** 光标（物理 px）是否落在卡片矩形内（2px 容差，圆角/边框不误判） */
function cardContains(
  card: HudCardRect | null,
  px: number,
  py: number
): boolean {
  if (!card || !(card.w > 0) || !(card.h > 0)) return false;
  return (
    px >= card.x - 2 &&
    px <= card.x + card.w + 2 &&
    py >= card.y - 2 &&
    py <= card.y + card.h + 2
  );
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

/** 悬停标记：.hud-open 已不改变布局（条不做抽屉展开），只作为"指针在
 *  条上"的状态位给收起逻辑与调试用。hover 给 70ms 防扫过误触；点击详情
 *  立即置位 */
function openDock(immediate = false): void {
  if (dragging) return; // 拖拽中保持默认的收起态
  clearCollapseTimer();
  ensurePointerWatch();
  if (rootEl.classList.contains("hud-open")) return;
  if (immediate) {
    clearExpandTimer();
    rootEl.classList.add("hud-open");
    return;
  }
  if (expandTimer) return;
  expandTimer = setTimeout(() => {
    expandTimer = null;
    if (rootEl.classList.contains("hud-open")) return;
    rootEl.classList.add("hud-open");
  }, 70);
}

/** 悬停标记复位：详情打开或指针仍在条内时不收 */
function closeDock(waitMs = 220): void {
  clearCollapseTimer();
  if (detailSource || pointerInside) return;
  collapseTimer = setTimeout(() => {
    collapseTimer = null;
    if (detailSource || pointerInside) return;
    rootEl.classList.remove("hud-open");
  }, waitMs);
}

/** 悬停标记（.hud-open）与卡片锚点的准备：条**不再做抽屉展开**（两种
 *  朝向都只露环、宽度恒定），所以这里没有任何过渡要等——只保证标记已
 *  落到当前帧，卡片立即锚定（以前要等 300ms 宽度动画，否则卡片会按
 *  半展开的面板边缘定位；现在展开态不存在了，再等就是白等） */
async function ensureDockOpen(): Promise<void> {
  clearExpandTimer();
  clearCollapseTimer();
  if (!rootEl.classList.contains("hud-open")) {
    rootEl.classList.add("hud-open");
  }
}

/** 详情卡：品牌头（套餐/健康态）+ 逐窗口额度行 + 24h Token 直方图 +
 *  底栏（窗口数/刷新说明 + 打开主窗口）。卡片本体在**独立窗口**里渲染
 *  （见 Rust hud_bar.rs 的建窗注释）：本函数把 hover 条目中心的物理坐标
 *  交给 Rust，摆位与入场由 Rust + 卡片窗自己完成。
 *  force 供数据刷新时重开一次（卡片页按新数据重渲染、不重放入场动画）。 */
async function showDetailCard(
  source: string,
  item: HTMLElement,
  force = false
): Promise<void> {
  clearCardCloseTimer();
  ensurePointerWatch();
  const seq = ++showSeq;
  // 数据刷新（同一 Agent 重开）：内容变、位置不变，不重放抽屉动画
  const refreshing = detailSource === source;
  if (!force && refreshing && lastMount && lastMount.source === source) {
    // 同一条目重复 hover（条目内部子元素切换）：刚开过的卡直接复用，
    // 不重发 open（重发会让卡片页重建 DOM，看起来就是"卡片又闪一下"）
    if (performance.now() - lastMount.at < 600) return;
  }
  detailSource = source;
  // 条目高亮标明详情归属（closeDetail / 重建时移除）
  stripEl
    .querySelectorAll(".hud-item.active")
    .forEach((el) => el.classList.remove("active"));
  item.classList.add("active");
  await ensureDockOpen();
  // 异步准备期间被收起或切到别的条目：本次流程作废
  if (seq !== showSeq || detailSource !== source) return;
  // 条目中心 → 物理屏幕坐标（Rust 侧几何全用物理 px）
  const rect = item.getBoundingClientRect();
  let st: HudBarPointerState;
  try {
    st = await invoke<HudBarPointerState>("hud_bar_pointer_state");
  } catch {
    return;
  }
  if (seq !== showSeq || detailSource !== source) return;
  const scale = Number.isFinite(st.scale) && st.scale > 0 ? st.scale : 1;
  const centerX = st.windowX + (rect.left + rect.width / 2) * scale;
  const centerY = st.windowY + (rect.top + rect.height / 2) * scale;
  const recent =
    lastMount !== null &&
    lastMount.source === source &&
    performance.now() - lastMount.at < 600;
  try {
    await invoke("hud_card_open", {
      source,
      centerX,
      centerY,
      animate: !refreshing && !recent,
    });
  } catch {
    /* 静默：卡片窗口预热失败时下一次 hover 重试 */
    return;
  }
  if (seq === showSeq && detailSource === source) {
    lastMount = { source, at: performance.now() };
  }
}

function closeDetail(): void {
  const had = detailSource !== null;
  clearCardCloseTimer();
  clearHoverSwitchTimer();
  hoverPending = null;
  showSeq++; // 作废仍在等待抽屉动画的打开流程
  cardHovered = false;
  detailSource = null;
  stripEl
    .querySelectorAll(".hud-item.active")
    .forEach((el) => el.classList.remove("active"));
  if (had) void invoke("hud_card_close", { animate: true }).catch(() => {});
  closeDock(120);
}

/** 事件目标所在条目（closest 返回 Element，统一转回 HTMLElement 供锚定） */
function hitItem(e: Event): HTMLElement | null {
  const target = e.target as HTMLElement | null;
  return (target?.closest(".hud-item") as HTMLElement | null) ?? null;
}

// ---- 阈值拖拽（.hud-item 不带 drag-region 属性，手动接管）----
// 越过 4px 即接管为**自定义拖拽**：先把抽屉/详情卡全部收掉
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
  // 卡片是独立窗口：拖拽前必须先收掉（拖动中窗口跟着指针跑，卡片留在原地
  // 会变成一张飘在桌面上的孤儿卡）
  void invoke("hud_card_close", { animate: false }).catch(() => {});
  rootEl.classList.remove("hud-open");
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

/** 拖拽开始：直接交给 Rust 按系统光标驱动（条窗口本身就是收缩尺寸，不再
 *  需要"先缩回画布"）。指针位置不再从前端事件取——跨屏时各屏缩放比不同，
 *  只有 GetCursorPos 是全局一致的；前端只负责"开始/结束"两个时机。 */
async function beginDrag(): Promise<void> {
  dragging = true;
  hoverSuspended = true;
  suppressClick = true;
  collapseForDrag();
  try {
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
  // await ensureDockOpen，期间卡还不存在；若此时重排计时器，每次移动都会
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


// ============================================================
// 主流程：语言初始化 + 事件监听（主题不跟随：恒定深色）
// ============================================================

const mainBar = async (): Promise<void> => {
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

  // 详情态失同步流：Rust 侧单方面收卡（吸附强制回位 / 托盘关闭）后广播
  // { open:false }——卡片窗口已被 Rust 隐藏，这里只收敛本地状态，别回发
  // close（两边来回乒乓会把刚开的卡立刻收掉）
  try {
    await listen<{ open: boolean }>("zbar://hud-bar-popover", (e) => {
      if (e.payload.open) return;
      showSeq++;
      detailSource = null;
      cardHovered = false;
      stripEl
        .querySelectorAll(".hud-item.active")
        .forEach((el) => el.classList.remove("active"));
      if (!pointerInside) closeDock(0);
    });
  } catch {
    /* 静默：失去同步时本地态由下一次 hover/点击的幂等 open 收敛 */
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

  // 首帧保障：数据/布局两路 listen 全部注册完成后强制数据线程重推一帧
  //（数据线程按"内容未变"静默跳过时销毁重开/页面重载会永远空窗；
  // refresh 只置标志，线程 200ms 内响应，数据已有缓存时零 HTTP）
  invoke("hud_bar_refresh").catch(() => {});
};

// ============================================================
// 详情卡窗口（label = hud-card，同一份 bundle 的另一种模式）
// ============================================================
//
// 卡片为什么独立成窗见 Rust hud_bar.rs 的建窗注释。这里只有一条纪律：
// **改窗口尺寸/位置的那几帧，卡片内容必须是 .measuring（不可见）**，否则
// WebView2 会把旧内容按左上角锚定画一帧（条页当年"首次悬浮闪现移动"
// 就是同一个机制）。所以流程固定为：挂隐藏卡 → 量尺寸 → 交给 Rust 摆窗口
// + show → 等视口到位 → 撤 .measuring 露出并播抽屉动画。

/** 卡片页当前展示的 source（null = 无卡） */
let cardSource: string | null = null;
/** 开卡序号：快速换卡/收起时，旧流程作废 */
let cardSeq = 0;
/** 退出动画的收尾计时器 */
let cardExitTimer: ReturnType<typeof setTimeout> | null = null;
/** 当前卡片节点（用于播退出动画） */
let cardEl: HTMLElement | null = null;

/** 等卡片窗口视口到位：Rust 摆好窗口后 innerWidth/innerHeight 才等于回执
 *  的目标尺寸。隐藏窗口里 rAF 不跑，用 setTimeout 轮询（最多 320ms）。 */
async function waitCardViewport(target: HudCardViewport): Promise<void> {
  const deadline = performance.now() + 320;
  for (;;) {
    if (
      Math.abs(window.innerWidth - target.width) <= 2 &&
      Math.abs(window.innerHeight - target.height) <= 2
    ) {
      return;
    }
    if (performance.now() > deadline) return;
    await delay(8);
  }
}

/** 开卡（含数据刷新时的重绘）：先隐藏量尺寸，交给 Rust 摆位，再露出入场 */
async function openCard(payload: HudCardOpenPayload): Promise<void> {
  const seq = ++cardSeq;
  if (cardExitTimer) {
    clearTimeout(cardExitTimer);
    cardExitTimer = null;
  }
  // 抽屉方向跟着停靠边走（贴左从左推、贴右从右推、贴顶从上展开）
  applyOrientation(payload.edge === "top" ? "horizontal" : "vertical");
  applyEdge(payload.edge);
  const card = buildDetailCard(payload.source);
  if (!card) {
    closeCard(false);
    return;
  }
  const prev = cardEl;
  const prevHeight = prev ? Math.ceil(prev.getBoundingClientRect().height) : -1;
  card.classList.add(drawerClass());
  if (!payload.animate) card.classList.add("no-anim");
  card.classList.add("measuring");
  card.style.left = `${CARD_PAD}px`;
  card.style.top = `${CARD_PAD}px`;
  popEl.classList.add("open");
  popEl.appendChild(card);
  const box = card.getBoundingClientRect();
  const width = Math.ceil(box.width) || 304;
  const height = Math.ceil(box.height);
  const sameSource = cardSource === payload.source;
  cardEl = card;
  cardSource = payload.source;
  // 尺寸没变（同源重绘、倒计时刷新）：直接换 DOM，不动窗口也就不会有一帧
  // 半透明的"卡片消失再出现"
  if (prev && prev !== card && sameSource && prevHeight === height) {
    prev.remove();
    card.classList.remove("measuring");
    return;
  }
  // 尺寸变了（换 Agent / 行数变化）：旧卡一起藏起来，等窗口按新尺寸摆好
  // 再露出——窗口改尺寸那一帧的内容必须是不可见的（见文件末尾说明）
  if (prev && prev !== card) prev.classList.add("measuring");
  if (prev && prev !== card) prev.remove();
  let target: HudCardViewport;
  try {
    target = await invoke<HudCardViewport>("hud_card_measure", {
      width,
      height,
    });
  } catch {
    return;
  }
  if (seq !== cardSeq) return; // 期间被收起或换了卡
  await waitCardViewport(target);
  if (seq !== cardSeq) return;
  card.classList.remove("measuring"); // 第一帧可见即在最终位置
}

/** 收卡：播完退出动画再让 Rust 真正隐藏窗口（窗口尺寸保持不变） */
function closeCard(animate: boolean): void {
  cardSeq++;
  const card = cardEl;
  cardEl = null;
  cardSource = null;
  const finish = (): void => {
    popEl.innerHTML = "";
    popEl.classList.remove("open");
    void invoke("hud_card_hide").catch(() => {});
  };
  if (!card || !animate) {
    finish();
    return;
  }
  card.classList.add("leaving");
  cardExitTimer = setTimeout(() => {
    cardExitTimer = null;
    finish();
  }, 150);
}

/** 卡片窗口冷启动兜底：确保窗口此刻是隐藏态（Rust 建窗即 visible(false)，
 *  这里只是热重载后自愈） */
const mainCard = async (): Promise<void> => {
  document.documentElement.dataset.mode = "card";
  try {
    await listen<HudBarUsage>("zbar://hud-bar-usage", (e) => {
      usage = e.payload;
      // 卡开着就按新数据重绘（不重放入场动画）
      if (cardSource) {
        void openCard({ source: cardSource, edge: barEdge, animate: false });
      }
    });
  } catch {
    /* 静默：保持空态 */
  }
  try {
    await listen<HudCardOpenPayload>(CARD_OPEN_EVENT, (e) => {
      void openCard(e.payload);
    });
  } catch {
    /* 静默：条侧 open 失败会由下一次 hover 重试 */
  }
  try {
    await listen<{ animate?: boolean }>(CARD_CLOSE_EVENT, (e) => {
      closeCard(e.payload?.animate !== false);
    });
  } catch {
    /* 静默 */
  }
  try {
    await listen(APPEARANCE_CHANGED_EVENT, () => {
      const next = detectMsg();
      if (next !== msg) {
        msg = next;
        if (cardSource) {
          void openCard({ source: cardSource, edge: barEdge, animate: false });
        }
      }
    });
  } catch {
    /* 静默 */
  }
  // 倒计时/剩余时间不随数据事件刷新：卡开着时定时重绘（同旧版 30s 节拍）
  setInterval(() => {
    if (cardSource) {
      void openCard({ source: cardSource, edge: barEdge, animate: false });
    }
  }, 30_000);
  invoke("hud_bar_refresh").catch(() => {});
  void invoke("hud_card_hide").catch(() => {});
};

if (CARD_MODE) {
  void mainCard();
} else {
  void mainBar();
}

// 开发态状态快照：逐帧验证脚本（.preview/）据此判断"到底是谁把卡收了"。
// 生产包被 tree-shake 掉（import.meta.env.DEV 为 false）
if (import.meta.env.DEV) {
  (window as unknown as Record<string, unknown>).__hudDebug = () => ({
    mode: CARD_MODE ? "card" : "bar",
    detailSource,
    cardSource,
    dock: rootEl.classList.contains("hud-open"),
    pointerInside,
    cardHovered,
    showSeq,
    cardSeq,
    lastMount,
    usageAgents: usage?.agents.length ?? -1,
    t: Math.round(performance.now()),
  });
}
