/**
 * 设置窗的「静态结构 + 校验规则」。
 */

import { NOTIFY_MODULE_KEYS } from "../../../island/notifyBrand";

export { NOTIFY_MODULE_KEYS };

/** 14 个导航页（顺序即侧栏顺序，文案固定） */
export type PageKey =
  | "look"
  | "order"
  | "hotkeys"
  | "actions"
  | "media"
  | "notify"
  | "stocks"
  | "weather"
  | "translate"
  | "live"
  | "launcher"
  | "system"
  | "advanced"
  | "about";

export interface PageMeta {
  key: PageKey;
  title: string;
  /** 页头说明（接线后的页沿用同一份） */
  desc: string;
}

export const PAGES: PageMeta[] = [
  {
    key: "look",
    title: "外观与交互",
    desc: "位置上贴哪条边、展开方式、主题与收起态内容。拖动灵动岛也会自动记下位置。",
  },
  {
    key: "order",
    title: "排序",
    desc: "收起胶囊里的同心额度环和主页额度卡按这里的顺序排列；新出现的账号排到最后。",
  },
  {
    key: "hotkeys",
    title: "全局热键",
    desc: "在任何窗口下都能触发的快捷键。修改后立即重新注册。",
  },
  {
    key: "actions",
    title: "快捷操作",
    desc: "全局热键状态与常用动作；这里的按钮立即执行，不受「保存并应用」影响。",
  },
  {
    key: "media",
    title: "媒体与歌词",
    desc: "正在播放的识别方式、自动切换、歌词源与滚动歌词。",
  },
  {
    key: "notify",
    title: "消息与邮件",
    desc: "消息页显示的模块、提醒正文、桌面弹窗，以及邮件未读检查。",
  },
  {
    key: "stocks",
    title: "股票",
    desc: "自选股、行情刷新间隔与涨跌幅提醒。",
  },
  {
    key: "weather",
    title: "天气",
    desc: "天气数据源、绑定城市与自定义城市列表。",
  },
  {
    key: "translate",
    title: "翻译",
    desc: "选择翻译平台和模型，或用自定义接口。",
  },
  {
    key: "live",
    title: "实时字幕",
    desc: "系统声音 → 日语识别 → 中文翻译的实时字幕参数。修改后重新开启实时字幕生效。",
  },
  {
    key: "launcher",
    title: "剪贴板与启动器",
    desc: "剪贴板保留条数与主页底部的应用启动器。",
  },
  {
    key: "system",
    title: "系统与控制",
    desc: "Clash Verge 与亮度控制（Twinkle）的本地路径。",
  },
  {
    key: "advanced",
    title: "高级",
    desc: "配置文件的位置、导入导出，以及恢复默认设置。",
  },
  {
    key: "about",
    title: "关于",
    desc: "应用信息、版本更新与开源说明。",
  },
];

/** 「板块」可显隐项：配置里存 hidden_tabs，控件显示的是「已显示」 */
export const HIDDEN_TAB_OPTIONS = [
  { value: "msg", label: "消息" },
  { value: "stock", label: "股票" },
  { value: "weather", label: "天气" },
  { value: "ctl", label: "控制" },
  { value: "clip", label: "剪贴板" },
  { value: "shot", label: "截图" },
  { value: "tool", label: "工具" },
  { value: "translate", label: "翻译" },
] as const;

/** 板块顺序（保存 hidden_tabs 时按这份顺序取差集，保证结果稳定） */
export const HIDDEN_TAB_KEYS: string[] = HIDDEN_TAB_OPTIONS.map((item) => item.value);

/** 正文预览只对读 toast 的来源有意义（DeepSeek 的正文来自会话日志，始终显示） */
export const NOTIFY_BODY_KEYS = ["qq", "wechat", "mail", "feishu", "dingtalk"] as const;

/** 桌面弹窗位置（顺序 = Rust 的 settings::POPUP_POSITIONS） */
export const POPUP_POSITION_KEYS = [
  "center_bottom",
  "center_top",
  "bottom_left",
  "bottom_right",
  "top_left",
  "top_right",
] as const;

/**
 * 每页参与草稿态的点路径（顺序 = 行顺序，校验按它跑）。
 * 未接线的页面留空数组，接线时把键补进来即可（错误定位 / 恢复本页默认值自动生效）。
 */
export const PAGE_FIELDS: Record<PageKey, string[]> = {
  look: [
    "island.edge",
    "island.pos",
    "island.screen",
    "island.keep_on_top",
    "island.hug_fullscreen",
    "island.theme",
    "island.font_family",
    "island.exclude_from_capture",
    "island.breath_enabled",
    "island.blur_fps",
    "island.transparency",
    "island.font_size",
    "island.expand_mode",
    "island.hover_expand",
    "island.hover_expand_ms",
    "island.collapse_delay_ms",
    "island.passthrough",
    "island.hide_fullscreen",
    "island.compact_view",
    "island.quota_display",
    "island.quota_ring_style",
    "island.auto_cycle_s",
    "island.hidden_tabs",
  ],
  order: ["agenthud.agent_order"],
  hotkeys: ["hotkeys.toggle", "hotkeys.translate", "hotkeys.clipboard"],
  actions: [],
  media: [
    "media.auto_show",
    "media.lyrics",
    "media.netease_log",
    "media.playback_logs",
    "media.lyrics_sources",
    "media.lyrics_custom_url",
    "media.priority",
  ],
  notify: [
    "notify.modules",
    "notify.body_preview",
    "dsh.enabled",
    "dsh.include_subagents",
    "dsh.skip_when_active",
    "dsh.body_chars",
    "dsh.sessions_dir",
    "notify.popup.enabled",
    "notify.popup.position",
    "notify.popup.size",
    "notify.open.qq",
    "notify.open.wechat",
    "notify.open.mail",
    "notify.open.feishu",
    "notify.open.dingtalk",
    "notify.open.dsh",
    "mail.interval_s",
    "mail.imap",
  ],
  stocks: ["stocks.watchlist", "stocks.interval", "stocks.pct_alert"],
  weather: ["weather.api_key", "weather.units", "weather.interval_min", "weather.city", "weather.cities"],
  translate: [
    "translate.custom_enabled",
    "translate.platform",
    "translate.platform_model",
    "translate.api_key",
    "translate.base_url",
    "translate.model",
    "translate.import_from",
  ],
  live: [
    "live.audio.source",
    "live.audio.device",
    "live.vad.threshold",
    "live.vad.min_silence_ms",
    "live.vad.min_speech_ms",
    "live.models_dir",
    "live.asr.partial",
    "live.asr.draft_interval_ms",
    "live.asr.final_threads",
    "live.asr.max_segment_s",
    "live.asr.context_prompt",
    "live.translate.enabled",
    "live.translate.live",
    "live.translate.live_min_interval_ms",
    "live.translate.live_min_new_chars",
    "live.translate.context_lines",
  ],
  launcher: ["clipboard.max", "launcher"],
  system: ["clash.config_dir", "clash.proxy_port", "clash.app", "brightness.twinkle"],
  advanced: [],
  about: [],
};

/** 点路径 → 所属页（错误定位用；未登记的键返回 null） */
const KEY_PAGE = new Map<string, PageKey>();
for (const page of PAGES) {
  for (const key of PAGE_FIELDS[page.key]) KEY_PAGE.set(key, page.key);
}

export function pageOfKey(key: string): PageKey | null {
  return KEY_PAGE.get(key) ?? null;
}

/** 行 DOM id（跳转错误 / 滚动定位用；点路径里的符号统一换成 -） */
export function rowDomId(key: string): string {
  return `wset-row-${key.replace(/[^a-zA-Z0-9]+/g, "-")}`;
}

// ============================================================ 热键规范（与 Rust 同语义）

const MOD_ALIASES: Record<string, string> = {
  ctrl: "ctrl",
  control: "ctrl",
  alt: "alt",
  option: "alt",
  shift: "shift",
  win: "win",
  meta: "win",
  cmd: "win",
  super: "win",
};

const MOD_ORDER = ["ctrl", "alt", "shift", "win"] as const;
const MOD_LABELS: Record<string, string> = {
  ctrl: "Ctrl",
  alt: "Alt",
  shift: "Shift",
  win: "Win",
};

/** 单个主键的规范写法；不认识的键返回 null（与 Rust `_norm_key` 一致） */
function normalizeKeyToken(token: string): string | null {
  const text = token.trim().toLowerCase();
  if (text.length === 1 && /[a-z0-9]/.test(text)) return text.toUpperCase();
  if (text === "space") return "Space";
  const fn = /^f(\d{1,2})$/.exec(text);
  if (fn && Number(fn[1]) >= 1 && Number(fn[1]) <= 24) return `F${Number(fn[1])}`;
  return null;
}

/**
 * 热键格式校验（空串 = 不启用，合法）。
 * 规则同 Rust：至少一个 Ctrl / Alt / Shift / Win，F1–F24 可以单独用。
 */
export function hotkeyProblem(spec: string): string | null {
  const text = String(spec ?? "").trim();
  if (!text) return null;
  const mods = new Set<string>();
  let main: string | null = null;
  for (const raw of text.split("+")) {
    const part = raw.trim();
    const lower = part.toLowerCase();
    if (MOD_ALIASES[lower]) {
      mods.add(MOD_ALIASES[lower]);
      continue;
    }
    const key = normalizeKeyToken(part);
    if (!key) return `无法识别的按键：${part || "空"}`;
    if (main !== null) return "只能有一个主按键，例如 Ctrl+Alt+I";
    main = key;
  }
  if (main === null) return "还缺少一个主按键";
  if (mods.size === 0 && !main.startsWith("F")) {
    return "至少需要一个 Ctrl / Alt / Shift / Win 修饰键";
  }
  return null;
}

/** 规范化热键写法：修饰键顺序 Ctrl→Alt→Shift→Win，主键大写 */
export function normalizeHotkey(spec: string): string {
  const text = String(spec ?? "").trim();
  if (!text) return "";
  const mods = new Set<string>();
  let main = "";
  for (const raw of text.split("+")) {
    const part = raw.trim();
    const lower = part.toLowerCase();
    if (MOD_ALIASES[lower]) {
      mods.add(MOD_ALIASES[lower]);
      continue;
    }
    const key = normalizeKeyToken(part);
    if (key) main = key;
  }
  const labels = MOD_ORDER.filter((name) => mods.has(name)).map((name) => MOD_LABELS[name]);
  return [...labels, main].filter(Boolean).join("+");
}

// ============================================================ 值校验

export type FieldKind = "hotkey" | "url" | "stock" | "text";

export interface FieldRule {
  /** 允许的枚举值 */
  allow?: readonly string[];
  /** 数值下限 / 上限（含端点） */
  min?: number;
  max?: number;
  /** 格式类校验 */
  kind?: FieldKind;
  /** 必填（去空白后不能为空） */
  required?: boolean;
  /** 覆盖默认的范围提示（例如百分比按 0~100 显示） */
  rangeMessage?: string;
}

/** 每个键的校验规则（没有登记的键不校验） */
export const FIELD_RULES: Record<string, FieldRule> = {
  "island.edge": { allow: ["top", "bottom", "left", "right"] },
  "island.pos": { min: 0, max: 1, rangeMessage: "取值范围是 0 ~ 100%" },
  "island.theme": { allow: ["auto", "light", "dark", "rouge", "sky"] },
  "island.blur_fps": { min: 2, max: 30 },
  "island.transparency": { min: 0, max: 90 },
  "island.font_size": { min: 10, max: 18 },
  "island.expand_mode": { allow: ["hover", "always", "never"] },
  "island.hover_expand_ms": { min: 0, max: 1000 },
  "island.collapse_delay_ms": { min: 0, max: 2000 },
  "island.compact_view": { allow: ["rings", "clock", "pet", "music"] },
  "island.quota_display": { allow: ["percent", "rings"] },
  "island.quota_ring_style": { allow: ["period", "dynamic"] },
  "island.auto_cycle_s": { min: 0, max: 600 },
  "hotkeys.toggle": { kind: "hotkey" },
  "hotkeys.translate": { kind: "hotkey" },
  "hotkeys.clipboard": { kind: "hotkey" },
  // 媒体与歌词
  "media.lyrics_custom_url": { kind: "url" },
  // 消息与邮件
  "dsh.body_chars": { min: 40, max: 240 },
  "notify.popup.position": { allow: POPUP_POSITION_KEYS },
  "notify.popup.size": { min: 70, max: 160 },
  "mail.interval_s": { min: 30, max: 3600 },
  // 股票
  "stocks.interval": { min: 2, max: 60 },
  "stocks.pct_alert": { min: 0, max: 20 },
  // 天气
  "weather.units": { allow: ["metric", "imperial"] },
  "weather.interval_min": { min: 5, max: 180 },
  // 翻译
  "translate.base_url": { kind: "url" },
  // 实时字幕
  "live.audio.source": { allow: ["loopback", "mic"] },
  "live.vad.threshold": { min: 0.1, max: 0.9 },
  "live.vad.min_silence_ms": { min: 100, max: 1500 },
  "live.vad.min_speech_ms": { min: 100, max: 1000 },
  "live.asr.draft_interval_ms": { min: 200, max: 2000 },
  "live.asr.final_threads": { min: 1, max: 16 },
  "live.asr.max_segment_s": { min: 3, max: 30 },
  "live.translate.live_min_interval_ms": { min: 300, max: 3000 },
  "live.translate.live_min_new_chars": { min: 1, max: 20 },
  "live.translate.context_lines": { min: 0, max: 3 },
  // 剪贴板与启动器
  "clipboard.max": { min: 1, max: 100 },
  // 系统与控制
  "clash.proxy_port": { min: 1, max: 65535 },
};

/** 范围文案：整数不带小数点，非整数最多 4 位小数 */
export function formatRange(lo: number, hi: number): string {
  const fmt = (v: number) => (Number.isInteger(v) ? String(v) : String(Number(v.toFixed(4))));
  return `${fmt(lo)} ~ ${fmt(hi)}`;
}

/** 接口地址校验：空串合法，否则必须是 http(s):// 开头 */
export function urlProblem(text: string): string | null {
  const value = String(text ?? "").trim();
  if (!value) return null;
  if (!/^https?:\/\/[^\s/]+/.test(value)) {
    return "接口地址需要以 http:// 或 https:// 开头";
  }
  return null;
}

/** 股票代码校验：A 股 sh/sz/bj + 6 位，港股 hk + 5 位，美股 us + 代码 */
export function stockCodeProblem(code: string): string | null {
  const value = String(code ?? "").trim();
  if (!value) return "股票代码不能为空";
  if (/^(sh|sz|bj)\d{6}$/i.test(value)) return null;
  if (/^hk\d{5}$/i.test(value)) return null;
  if (/^us[A-Za-z0-9.-]+$/.test(value)) return null;
  return `代码格式不对：${value}（A 股 sh/sz/bj + 6 位，港股 hk + 5 位，美股 us + 代码）`;
}

export interface ValidationError {
  key: string;
  message: string;
}

/** 单键校验（不含跨行重复）；返回错误说明或 null */
export function validateValue(key: string, value: unknown): string | null {
  const rule = FIELD_RULES[key];
  if (!rule) return null;
  const text = typeof value === "string" ? value.trim() : "";
  if (rule.required && !text) return "不能为空";
  if (rule.kind === "hotkey") return hotkeyProblem(String(value ?? ""));
  if (rule.kind === "url") return urlProblem(String(value ?? ""));
  if (rule.kind === "stock") return stockCodeProblem(String(value ?? ""));
  if (rule.allow && !rule.allow.includes(String(value))) return "选项已失效，请重新选择";
  if (rule.min != null || rule.max != null) {
    const numeric = typeof value === "number" ? value : Number(String(value ?? "").trim());
    if (!Number.isFinite(numeric)) return "请输入一个数字";
    if (rule.min != null && numeric < rule.min) {
      return rule.rangeMessage ?? `请输入 ${formatRange(rule.min, rule.max ?? Infinity)} 之间的数字`;
    }
    if (rule.max != null && numeric > rule.max) {
      return rule.rangeMessage ?? `请输入 ${formatRange(rule.min ?? -Infinity, rule.max)} 之间的数字`;
    }
  }
  return null;
}

/** 字符串数组 + 白名单校验（消息模块 / 正文预览） */
function kindListProblem(
  value: unknown,
  allowed: readonly string[],
  message: string
): string | null {
  if (!Array.isArray(value)) return message;
  for (const item of value) {
    if (typeof item !== "string" || !allowed.includes(item)) return message;
  }
  return null;
}

/** 列表类值的额外校验（键 → 校验器），行内红字显示在对应 Row 上 */
const LIST_VALIDATORS: Record<string, (value: unknown) => string | null> = {
  "media.playback_logs": (value) => {
    if (!Array.isArray(value)) return "播放日志路径格式错误";
    if (value.length > 16) return "最多可添加 16 个播放日志";
    const seen = new Set<string>();
    for (const item of value) {
      const path = String(item ?? "").trim();
      if (!/^(?:[a-z]:[\\/]|[\\/]{2}[^\\/]+[\\/][^\\/]+)/i.test(path) || !/\.jsonl?$/i.test(path)) {
        return "请输入完整的 Windows .json 或 .jsonl 文件路径";
      }
      const key = path.toLowerCase();
      if (seen.has(key)) return `播放日志路径重复：${path}`;
      seen.add(key);
    }
    return null;
  },
  "notify.modules": (value) => kindListProblem(value, NOTIFY_MODULE_KEYS, "消息模块格式错误"),
  "notify.body_preview": (value) =>
    kindListProblem(value, NOTIFY_BODY_KEYS, "正文预览格式错误"),
  "media.priority": (value) => {
    if (!Array.isArray(value)) return "播放器优先级格式错误";
    const seen = new Set<string>();
    for (const item of value) {
      const text = String(item ?? "").trim();
      if (!text) return "播放器列表里有空白项，请填写或删除";
      if (seen.has(text.toLowerCase())) return `播放器重复：${text}`;
      seen.add(text.toLowerCase());
    }
    return null;
  },
  "stocks.watchlist": (value) => {
    if (!Array.isArray(value)) return "自选股格式错误";
    const seen = new Set<string>();
    for (const item of value) {
      const code = String(item ?? "").trim();
      if (!code) return "自选股里有空白项，请填写或删除";
      const problem = stockCodeProblem(code);
      if (problem) return problem;
      if (seen.has(code.toLowerCase())) return `自选股重复：${code}`;
      seen.add(code.toLowerCase());
    }
    return null;
  },
  "mail.imap": (value) => {
    if (!Array.isArray(value)) return "IMAP 账户格式错误";
    for (let index = 0; index < value.length; index += 1) {
      const account = value[index];
      if (!account || typeof account !== "object") return `第 ${index + 1} 个账户格式错误`;
      const row = account as Record<string, unknown>;
      const host = String(row.host ?? "").trim();
      const user = String(row.user ?? "").trim();
      if (!host || !user) return `第 ${index + 1} 个账户缺少服务器或用户名`;
      if (/\s/.test(host)) return `第 ${index + 1} 个账户的服务器地址有空格`;
      const port = Number(row.port);
      if (!Number.isInteger(port) || port < 1 || port > 65535) {
        return `第 ${index + 1} 个账户的端口无效（1 ~ 65535）`;
      }
    }
    return null;
  },
  "weather.cities": (value) => {
    if (!Array.isArray(value)) return "城市列表格式错误";
    for (let index = 0; index < value.length; index += 1) {
      const city = value[index];
      if (!city || typeof city !== "object") return `第 ${index + 1} 个城市格式错误`;
      const row = city as Record<string, unknown>;
      const name = String(row.name ?? "").trim();
      if (!name) return `第 ${index + 1} 个城市缺少名字`;
      const hasLat = row.lat !== null && row.lat !== undefined && String(row.lat).trim() !== "";
      const hasLon = row.lon !== null && row.lon !== undefined && String(row.lon).trim() !== "";
      if (hasLat !== hasLon) return `${name} 的经纬度需要同时填写或同时留空`;
      if (hasLat) {
        const lat = Number(row.lat);
        const lon = Number(row.lon);
        if (!Number.isFinite(lat) || !Number.isFinite(lon)) return `${name} 的经纬度不是数字`;
        if (lat < -90 || lat > 90 || lon < -180 || lon > 180) {
          return `${name} 的经纬度超出范围`;
        }
      }
    }
    return null;
  },
  launcher: (value) => {
    if (!Array.isArray(value)) return "启动器格式错误";
    for (let index = 0; index < value.length; index += 1) {
      const entry = value[index];
      if (!entry || typeof entry !== "object") return `第 ${index + 1} 个启动项格式错误`;
      const row = entry as Record<string, unknown>;
      if (!String(row.name ?? "").trim()) return `第 ${index + 1} 个启动项缺少名称`;
      if (!String(row.path ?? "").trim()) return `第 ${index + 1} 个启动项缺少路径`;
    }
    return null;
  },
};

/**
 * 全量校验：按 PAGE_FIELDS 的页面 / 行顺序走，结果顺序稳定（首个错误优先），
 * 末尾追加热键跨行重复错误（保持结果顺序稳定）。
 */
export function validateDraft(getValue: (key: string) => unknown): ValidationError[] {
  const errors: ValidationError[] = [];
  const seen = new Set<string>();
  const hotkeys: { key: string; spec: string }[] = [];

  for (const page of PAGES) {
    for (const key of PAGE_FIELDS[page.key]) {
      if (seen.has(key)) continue;
      seen.add(key);
      if (key === "translate.base_url" && getValue("translate.custom_enabled") !== true) continue;
      const value = getValue(key);
      const message = validateValue(key, value) ?? LIST_VALIDATORS[key]?.(value) ?? null;
      if (message) errors.push({ key, message });
      if (isHotkeyRule(key)) {
        const spec = normalizeHotkey(String(value ?? ""));
        if (spec) hotkeys.push({ key, spec });
      }
    }
  }

  const marked = new Set(errors.map((item) => item.key));
  const bySpec = new Map<string, { key: string; spec: string }[]>();
  for (const item of hotkeys) {
    const list = bySpec.get(item.spec.toLowerCase()) ?? [];
    list.push(item);
    bySpec.set(item.spec.toLowerCase(), list);
  }
  for (const [spec, group] of bySpec) {
    if (group.length < 2) continue;
    for (const item of group) {
      if (marked.has(item.key)) continue;
      errors.push({ key: item.key, message: `热键 ${spec} 和另一项重复` });
      marked.add(item.key);
    }
  }
  return errors;
}

function isHotkeyRule(key: string): boolean {
  return FIELD_RULES[key]?.kind === "hotkey";
}
