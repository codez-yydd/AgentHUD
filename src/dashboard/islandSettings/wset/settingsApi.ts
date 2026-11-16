/**
 * 灵动岛设置窗（wset）的数据接口层：只封装「settings.json 文档 + 保存 / 重载 /
 * 导入导出 / 显示器 / 字体 / 热键状态 / 窗口收起」这一个窗口用到的命令。
 */

import { invoke } from "@tauri-apps/api/core";
import { MOCK_SETTINGS_STORE_KEY } from "../../../island/mock";

/** 是否运行在 Tauri WebView 里（与 src/island/api.ts 的 IS_TAURI 同口径） */
export const IS_TAURI =
  typeof window !== "undefined" &&
  Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);

/** 设置文档（data / defaults 都是嵌套 snake_case 对象） */
export interface SettingsDoc {
  data: Record<string, unknown>;
  defaults: Record<string, unknown>;
  path: string;
  data_dir: string;
  /** 后端尚未接入的点路径：对应行显示「未接入」小标签 */
  unsupported: string[];
}

/** 保存结果（applied = 本次真正同步给岛运行时的点路径列表） */
export interface SettingsSaveResult {
  data: Record<string, unknown>;
  unsupported: string[];
  applied: string[];
}

export interface SettingsPaths {
  path: string;
  data_dir: string;
}

export interface SettingsFont {
  family: string;
  has_chinese: boolean;
}

export interface SettingsMonitor {
  index: number;
  name: string;
  primary: boolean;
  width: number;
  height: number;
  scale_factor: number;
}

export interface SettingsHotkeyStatus {
  toggle: boolean;
  translate: boolean;
  clipboard: boolean;
}

/** `island_translate` 的返回体（model 是实际使用的模型） */
export interface TranslateResult {
  text: string;
  translation: string;
  model: string;
}

export interface TranslationPlatform {
  id: string;
  label: string;
}

// ============================================================ 默认文档（mock）

/** 浏览器预览的默认文档：形状 / 默认值与 Rust 侧 `SettingsDoc::default()` 对齐
 *  （src-tauri/src/island/settings.rs 与 island/mod.rs 的 default_*；
 *   mock 只需要本窗会读写的块，未列出的键由后端 defaults 兜底） */
const MOCK_DEFAULTS: Record<string, unknown> = {
  island: {
    edge: "top",
    pos: 0.5,
    screen: 0,
    keep_on_top: true,
    hug_fullscreen: true,
    theme: "dark",
    font_family: "",
    exclude_from_capture: false,
    blur_fps: 8,
    expand_mode: "hover",
    hover_expand: true,
    hover_expand_ms: 120,
    collapse_delay_ms: 320,
    passthrough: false,
    hide_fullscreen: false,
    compact_view: "rings",
    quota_display: "percent",
    auto_cycle_s: 30,
    hidden_tabs: [],
    transparency: 0,
    font_size: 13,
  },
  agenthud: {
    agent_order: [],
  },
  hotkeys: {
    toggle: "Ctrl+Alt+Shift+I",
    translate: "Ctrl+Alt+J",
    clipboard: "Ctrl+Alt+V",
  },
  media: {
    auto_show: true,
    lyrics: true,
    netease_log: true,
    playback_logs: [],
    lyrics_sources: "netease,lrclib,custom",
    lyrics_custom_url: "",
    priority: ["cloudmusic", "bilibili", "chrome", "zen", "msedge", "firefox"],
  },
  notify: {
    modules: ["qq", "wechat", "mail"],
    body_preview: ["feishu", "dingtalk"],
    popup: { enabled: true, position: "bottom_right", size: 100 },
    open: { qq: "", wechat: "", mail: "", feishu: "", dingtalk: "", dsh: "" },
  },
  dsh: {
    enabled: true,
    sessions_dir: "",
    include_subagents: false,
    skip_when_active: true,
    body_chars: 120,
  },
  mail: {
    interval_s: 120,
    imap: [],
  },
  stocks: {
    watchlist: ["sh000001", "sz399001", "sz399006", "hk00700", "usAAPL", "usNVDA", "usQQQ"],
    interval: 5,
    pct_alert: 3,
  },
  weather: {
    api_key: "",
    // 空 = 跟随公网 IP 定位（与 Rust IslandConfig / settings.json 默认一致）
    city: "",
    lat: null,
    lon: null,
    units: "metric",
    interval_min: 15,
    cities: [],
  },
  translate: {
    custom_enabled: false,
    platform: "",
    platform_model: "",
    api_key: "",
    base_url: "",
    model: "",
    import_from: "",
  },
  live: {
    models_dir: "models",
    audio: { source: "loopback", device: "" },
    vad: { threshold: 0.4, min_silence_ms: 350, min_speech_ms: 200 },
    asr: {
      partial: true,
      draft_interval_ms: 500,
      final_threads: 6,
      max_segment_s: 10,
      context_prompt: false,
    },
    translate: {
      enabled: true,
      live: true,
      live_min_interval_ms: 900,
      live_min_new_chars: 4,
      context_lines: 3,
    },
  },
  clipboard: { max: 10 },
  launcher: [
    { name: "资源管理器", path: "explorer.exe", icon: "folder" },
    { name: "浏览器", path: "https://www.bing.com", icon: "browser" },
    { name: "设置", path: "ms-settings:", icon: "settings" },
    { name: "任务管理器", path: "taskmgr.exe", icon: "task" },
    { name: "记事本", path: "notepad.exe", icon: "note" },
  ],
  clash: {
    config_dir: "%APPDATA%/io.github.clash-verge-rev.clash-verge-rev",
    proxy_port: 7897,
    app: "",
  },
  brightness: { twinkle: "" },
};

const MOCK_DATA_DIR = "~/.agenthud";
const MOCK_CONFIG_PATH = `${MOCK_DATA_DIR}/settings.json`;
/** 浏览器预览专用：想在界面上看「未接入」标签时，往这个键塞点路径数组 */
const MOCK_UNSUPPORTED_KEY = "agenthud.mock.islandSettings.unsupported";

/** 深拷贝（文档里全是 JSON 值） */
function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/** 深合并：patch 里出现的键覆盖 data，对象递归、数组整体替换 */
function applyPatch(
  data: Record<string, unknown>,
  patch: Record<string, unknown>
): Record<string, unknown> {
  const out = clone(data);
  for (const [key, value] of Object.entries(patch)) {
    const parts = key.split(".");
    let cur: Record<string, unknown> = out;
    for (const part of parts.slice(0, -1)) {
      const next = cur[part];
      if (!next || typeof next !== "object" || Array.isArray(next)) {
        cur[part] = {};
      }
      cur = cur[part] as Record<string, unknown>;
    }
    cur[parts[parts.length - 1]] = clone(value);
  }
  return out;
}

/** 深合并嵌套对象（localStorage 里的文档可能只存了部分块，缺的用默认值补齐） */
function mergeDeep(
  base: Record<string, unknown>,
  over: Record<string, unknown>
): Record<string, unknown> {
  const out = clone(base);
  for (const [key, value] of Object.entries(over)) {
    const prev = out[key];
    if (
      value &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      prev &&
      typeof prev === "object" &&
      !Array.isArray(prev)
    ) {
      out[key] = mergeDeep(prev as Record<string, unknown>, value as Record<string, unknown>);
    } else {
      out[key] = clone(value);
    }
  }
  return out;
}

/** 读取 mock 文档数据（首次打开用默认值，之后读 localStorage） */
function mockLoadData(): Record<string, unknown> {
  try {
    const raw = localStorage.getItem(MOCK_SETTINGS_STORE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as { data?: Record<string, unknown> };
      if (parsed?.data && typeof parsed.data === "object") {
        return mergeDeep(clone(MOCK_DEFAULTS), parsed.data);
      }
    }
  } catch {
    /* 隐私模式 / 存储不可用：退回内存默认值 */
  }
  return clone(MOCK_DEFAULTS);
}

function mockPersistData(data: Record<string, unknown>) {
  try {
    localStorage.setItem(MOCK_SETTINGS_STORE_KEY, JSON.stringify({ data }));
  } catch {
    /* 忽略：预览不要求强一致 */
  }
}

function mockDoc(data = mockLoadData()): SettingsDoc {
  return {
    data,
    defaults: clone(MOCK_DEFAULTS),
    path: MOCK_CONFIG_PATH,
    data_dir: MOCK_DATA_DIR,
    unsupported: mockUnsupported(),
  };
}

/** 浏览器预览的「未接入」点路径（真机由后端 payload 给出，mock 默认空） */
function mockUnsupported(): string[] {
  try {
    const raw = localStorage.getItem(MOCK_UNSUPPORTED_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (Array.isArray(parsed)) {
      return parsed.filter((item): item is string => typeof item === "string");
    }
  } catch {
    /* 无效 JSON / 隐私模式：按「全部已接入」处理 */
  }
  return [];
}

const MOCK_FONTS: SettingsFont[] = [
  { family: "HarmonyOS Sans", has_chinese: true },
  { family: "MiSans", has_chinese: true },
  { family: "Segoe UI Variable Text", has_chinese: false },
  { family: "Segoe UI", has_chinese: false },
  { family: "Microsoft YaHei UI", has_chinese: true },
  { family: "Microsoft YaHei", has_chinese: true },
  { family: "SimSun", has_chinese: true },
  { family: "SimHei", has_chinese: true },
  { family: "KaiTi", has_chinese: true },
  { family: "DengXian", has_chinese: true },
  { family: "Source Han Sans SC", has_chinese: true },
  { family: "Noto Sans SC", has_chinese: true },
  { family: "Consolas", has_chinese: false },
  { family: "Cascadia Code", has_chinese: false },
  { family: "JetBrains Mono", has_chinese: false },
  { family: "Arial", has_chinese: false },
  { family: "Calibri", has_chinese: false },
  { family: "Georgia", has_chinese: false },
];

const MOCK_MONITORS: SettingsMonitor[] = [
  { index: 0, name: "显示器 1", primary: true, width: 2560, height: 1440, scale_factor: 1.25 },
  { index: 1, name: "显示器 2", primary: false, width: 1920, height: 1080, scale_factor: 1 },
];

/** 浏览器预览的假延迟：让加载态 / 保存态一眼可见（真机命令是毫秒级） */
function delay<T>(value: T, ms = 110): Promise<T> {
  return new Promise((resolve) => window.setTimeout(() => resolve(value), ms));
}

// ============================================================ 文档 / 保存

/** 读取设置文档（首帧与重载共用） */
export async function getSettingsDoc(): Promise<SettingsDoc> {
  if (!IS_TAURI) return delay(mockDoc());
  return invoke<SettingsDoc>("island_settings_doc");
}

/** 保存改动：patch 只带差异键（扁平点路径） */
export async function saveSettings(
  patch: Record<string, unknown>
): Promise<SettingsSaveResult> {
  if (!IS_TAURI) {
    const data = applyPatch(mockLoadData(), patch);
    mockPersistData(data);
    return delay({
      data,
      unsupported: mockUnsupported(),
      applied: Object.keys(patch),
    });
  }
  return invoke<SettingsSaveResult>("island_settings_save", { patch });
}

/** 重新读取文件（外部改动后刷新） */
export async function reloadSettings(): Promise<SettingsDoc> {
  if (!IS_TAURI) return delay(mockDoc());
  return invoke<SettingsDoc>("island_settings_reload");
}

/** 全部恢复默认 */
export async function resetSettings(): Promise<SettingsSaveResult> {
  if (!IS_TAURI) {
    mockPersistData(clone(MOCK_DEFAULTS));
    return delay({ data: clone(MOCK_DEFAULTS), unsupported: mockUnsupported(), applied: [] });
  }
  return invoke<SettingsSaveResult>("island_settings_reset");
}

/** 从文件导入 */
export async function importSettings(path: string): Promise<SettingsSaveResult> {
  if (!IS_TAURI) {
    console.info("[island mock] import settings", path);
    return delay({ data: mockLoadData(), unsupported: mockUnsupported(), applied: [] });
  }
  return invoke<SettingsSaveResult>("island_settings_import", { path });
}

/** 导出到文件 */
export async function exportSettings(path: string): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] export settings", path);
    return delay(undefined, 20);
  }
  return invoke("island_settings_export", { path });
}

/** 用系统默认程序打开 settings.json（标题栏「打开配置文件」） */
export async function openSettingsFile(): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] open settings file", MOCK_CONFIG_PATH);
    return delay(undefined, 20);
  }
  return invoke("island_settings_open_file");
}

/** 在资源管理器里定位 settings.json */
export async function revealSettingsFile(): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] reveal settings file", MOCK_CONFIG_PATH);
    return delay(undefined, 20);
  }
  return invoke("island_settings_reveal_file");
}

/** 配置文件与数据目录（快捷操作页脚注） */
export async function getSettingsPaths(): Promise<SettingsPaths> {
  if (!IS_TAURI) return delay({ path: MOCK_CONFIG_PATH, data_dir: MOCK_DATA_DIR });
  return invoke<SettingsPaths>("island_settings_paths");
}

// ============================================================ 显示器 / 字体 / 热键

export async function getMonitors(): Promise<SettingsMonitor[]> {
  if (!IS_TAURI) return delay(clone(MOCK_MONITORS));
  return invoke<SettingsMonitor[]>("island_monitors");
}

export async function getFonts(): Promise<SettingsFont[]> {
  if (!IS_TAURI) return delay(clone(MOCK_FONTS));
  return invoke<SettingsFont[]>("island_fonts");
}

/** 三个全局热键当前是否注册成功（只读，保存后延迟复查） */
export async function getHotkeyStatus(): Promise<SettingsHotkeyStatus> {
  if (!IS_TAURI) {
    const data = mockLoadData();
    const hotkeys = (data.hotkeys ?? {}) as Record<string, unknown>;
    return delay({
      toggle: Boolean(String(hotkeys.toggle ?? "").trim()),
      translate: Boolean(String(hotkeys.translate ?? "").trim()),
      clipboard: Boolean(String(hotkeys.clipboard ?? "").trim()),
    });
  }
  return invoke<SettingsHotkeyStatus>("island_hotkey_status");
}

// ============================================================ 翻译测试

const MOCK_TRANSLATION_PLATFORMS = [
  { id: "deepseek:demo", label: "DeepSeek · 示例账号", models: ["deepseek-chat", "deepseek-reasoner"] },
  { id: "kimi:demo", label: "Kimi · 示例账号", models: ["kimi-k2.5", "moonshot-v1-8k"] },
];

export async function listTranslationPlatforms(): Promise<TranslationPlatform[]> {
  if (!IS_TAURI) {
    return delay(MOCK_TRANSLATION_PLATFORMS.map(({ id, label }) => ({ id, label })));
  }
  return invoke<TranslationPlatform[]>("island_translation_platforms");
}

export async function listTranslationModels(platform: string): Promise<string[]> {
  if (!IS_TAURI) {
    const source = MOCK_TRANSLATION_PLATFORMS.find(({ id }) => id === platform);
    if (!source) throw new Error("所选平台已不可用");
    return delay([...source.models]);
  }
  return invoke<string[]>("island_translation_models", { platform });
}

/**
 * 用**已保存**的 `translate.*` 配置翻译一段文本（设置页「测试翻译」用，
 * 不读取、也不写回草稿）。返回译文；失败时抛出后端的中文错误。
 */
export async function translateText(text: string, target = "zh"): Promise<string> {
  if (!IS_TAURI) {
    if (!text.trim()) throw new Error("没有需要翻译的文本");
    const config = mockLoadData().translate as Record<string, unknown>;
    if (config.custom_enabled === true) {
      if (!String(config.api_key ?? "").trim() && !String(config.import_from ?? "").trim()) {
        throw new Error("请填写自定义 API Key 并保存");
      }
      if (!String(config.base_url ?? "").trim() || !String(config.model ?? "").trim()) {
        throw new Error("请填写自定义接口地址和模型并保存");
      }
    } else {
      const platform = MOCK_TRANSLATION_PLATFORMS.find(({ id }) => id === config.platform);
      if (!platform?.models.some((model) => model === config.platform_model)) {
        throw new Error("请选择翻译平台和模型并保存");
      }
    }
    return delay("你好，世界", 420);
  }
  const result = await invoke<TranslateResult>("island_translate", { text, target });
  return result.translation;
}

// ============================================================ 窗口 / 文件对话框

/** 收起设置窗（关闭 = 隐藏，见 src-tauri 侧 prevent_close 逻辑）。
 *
 * 优先走专属命令 island_settings_window_hide（后端接线后由它负责隐藏）；
 * 命令还没接线 / 调用失败时退回窗口自身的 hide()（capabilities 里的
 * core:window:allow-hide 就是为这条兜底路径加的），保证「关闭 / Esc」在
 * 前后端接线期间也不会变成一个点了没反应的按钮。 */
export async function hideSettingsWindow(): Promise<void> {
  if (!IS_TAURI) {
    console.info("[island mock] hide settings window");
    return delay(undefined, 10);
  }
  try {
    await invoke("island_settings_window_hide");
  } catch {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().hide();
    } catch {
      /* 两个路径都失败：窗口保持打开，用户在界面上仍可继续操作 */
    }
  }
}

/** 拖动窗口（标题栏拖拽，按钮区除外）。浏览器预览无窗口可拖，静默忽略。 */
export async function startWindowDrag(): Promise<void> {
  if (!IS_TAURI) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().startDragging();
  } catch {
    /* 拖拽失败（快速点击 / ACL）不影响其它交互 */
  }
}

export interface PickFileOptions {
  mode: "open" | "save";
  title?: string;
  /** 默认文件名 / 目录，按后端给的路径预填 */
  defaultPath?: string;
  filters?: { name: string; extensions: string[] }[];
  /** 选择文件夹而不是文件（目录类路径行） */
  directory?: boolean;
}

/** 系统文件对话框（导入 / 导出用）。浏览器预览返回固定的假路径。 */
export async function pickFile(options: PickFileOptions): Promise<string | null> {
  if (!IS_TAURI) {
    return delay(options.mode === "save" ? "C:\\Users\\Administrator\\Desktop\\settings.json" : MOCK_CONFIG_PATH);
  }
  const { open, save } = await import("@tauri-apps/plugin-dialog");
  if (options.mode === "save") {
    return save({
      title: options.title,
      defaultPath: options.defaultPath,
      filters: options.filters,
    });
  }
  const picked = await open({
    title: options.title,
    defaultPath: options.defaultPath,
    multiple: false,
    directory: Boolean(options.directory),
    filters: options.directory ? undefined : options.filters,
  });
  return typeof picked === "string" ? picked : null;
}
