/** 灵动岛 API 封装：Tauri 环境走 invoke；浏览器预览回退到 mock.ts */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  mockClipboardWithImage,
  mockConfig,
  mockMedia,
  mockMeetingDevices,
  mockMeetings,
  mockMinute,
  mockNotify,
  mockProjects,
  mockSnippets,
  mockStocks,
  mockSystem,
  mockToolboxImagePreview,
  mockToolboxQr,
  mockUsage,
  mockWeather,
} from "./mock";
import type {
  ClipItem,
  HudBarUsage,
  IslandConfig,
  IslandDragBegin,
  IslandDragEnd,
  IslandDragStep,
  IslandState,
  LauncherProject,
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
  NotifyState,
  Snippet,
  StockMinute,
  StockSearchItem,
  StocksState,
  SystemState,
  TodayUsageTotals,
  ToolboxImageInfo,
  ToolboxImageOut,
  ToolboxQrPng,
  ToolboxQrSaved,
  WeatherState,
} from "./types";

export const IS_TAURI =
  typeof window !== "undefined" &&
  Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);

function delay<T>(value: T, ms = 120): Promise<T> {
  return new Promise((resolve) => window.setTimeout(() => resolve(value), ms));
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

// 浏览器预览用的可变状态（真机上不参与：全部走 invoke）
let mockClipState: ClipItem[] = clone(mockClipboardWithImage());
let mockSnippetState: Snippet[] = clone(mockSnippets);

/** 钉住的排前面（与 Rust 侧 sort_for_display 同语义的稳定分区） */
function sortPinnedFirst(items: ClipItem[]): ClipItem[] {
  return [
    ...items.filter((item) => item.pinned),
    ...items.filter((item) => !item.pinned),
  ];
}

// ===== 窗口 / 配置 =====

export async function getIslandState(): Promise<IslandState> {
  if (!IS_TAURI) {
    return delay({
      enabled: true,
      config: clone(mockConfig),
      expanded: false,
    });
  }
  return invoke<IslandState>("island_state");
}

export async function getIslandConfig(): Promise<IslandConfig> {
  if (!IS_TAURI) return delay(clone(mockConfig));
  return invoke<IslandConfig>("island_config");
}

export async function setIslandConfig(config: IslandConfig): Promise<IslandConfig> {
  if (!IS_TAURI) {
    Object.assign(mockConfig, clone(config));
    return delay(clone(mockConfig));
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
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_set_expanded", { expanded });
}

/** macOS 悬停展开用：把收起态胶囊的实际矩形（CSS px）告诉 Rust。 */
export async function islandSetHoverRect(
  x: number,
  y: number,
  width: number,
  height: number
): Promise<void> {
  if (!IS_TAURI) return;
  try {
    await invoke("island_set_hover_rect", { x, y, width, height });
  } catch {
    /* 上报失败只影响 mac 悬停手感，不影响其它功能 */
  }
}

// ===== 拖拽 / 吸附 =====

export async function beginIslandDrag(): Promise<IslandDragBegin> {
  if (!IS_TAURI) return delay({ active: true }, 10);
  return invoke<IslandDragBegin>("island_drag_begin");
}

export async function stepIslandDrag(): Promise<IslandDragStep> {
  // 浏览器预览没有真实光标位移：一律按「只是按住没拖」处理（started=false）。
  // 之前恒回 started=true，预览里点任何按钮都可能被判成拖拽而收起面板。
  if (!IS_TAURI) return delay({ active: true, dx: 0, dy: 0, started: false }, 10);
  return invoke<IslandDragStep>("island_drag_step");
}

export async function endIslandDrag(started = true): Promise<IslandDragEnd> {
  if (!IS_TAURI) return delay({ edge: "top", x: 0, y: 0, width: 552, height: 70 }, 10);
  // Rust 的 drag_step 会在松手时先清掉会话，等淡出 130ms 后再 end 时
  // 槽里可能已经空了。显式告诉 Rust 这次是真拖拽，才能保证左/顶也吸附。
  return invoke<IslandDragEnd>("island_drag_end", { started });
}

/** 窗口身后的桌面缩略图（data URL）；浏览器预览返回空，让 CSS 渐变兜底。 */
export async function getIslandBackdrop(): Promise<string> {
  if (!IS_TAURI) return "";
  return invoke<string>("island_backdrop");
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

// ===== 额度 =====

export async function getQuotaSnapshot(): Promise<HudBarUsage | null> {
  if (!IS_TAURI) return delay(clone(mockUsage));
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
  if (!IS_TAURI) return delay(clone(mockWeather));
  return invoke<WeatherState | null>("island_weather");
}

export async function refreshWeather(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_weather_refresh");
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
  if (!IS_TAURI) return delay(clone(mockStocks));
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
  if (!IS_TAURI) return delay({ ...clone(mockMinute), code });
  return invoke<StockMinute>("island_stock_minute", { code });
}

// ===== 媒体 =====

export async function getMedia(): Promise<MediaState> {
  if (!IS_TAURI) return delay(clone(mockMedia), 40);
  return invoke<MediaState>("island_media");
}

export async function mediaAction(action: "toggle" | "next" | "prev"): Promise<void> {
  if (!IS_TAURI) {
    if (action === "toggle") mockMedia.playing = !mockMedia.playing;
    return delay(undefined, 40);
  }
  return invoke("island_media_action", { action });
}

export async function getLyrics(title: string, artist: string): Promise<LyricsState> {
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
  return invoke<LyricsState>("island_lyrics", { title, artist });
}

// ===== 系统 =====

export async function getSystem(): Promise<SystemState> {
  if (!IS_TAURI) return delay(clone(mockSystem));
  return invoke<SystemState>("island_system");
}

export async function setVolume(volume: number): Promise<SystemState> {
  if (!IS_TAURI) {
    mockSystem.volume = Math.min(1, Math.max(0, volume));
    return delay(clone(mockSystem), 20);
  }
  return invoke<SystemState>("island_set_volume", { volume });
}

export async function toggleMute(): Promise<SystemState> {
  if (!IS_TAURI) {
    mockSystem.muted = !mockSystem.muted;
    return delay(clone(mockSystem), 20);
  }
  return invoke<SystemState>("island_toggle_mute");
}

export async function setBrightness(value: number): Promise<SystemState> {
  if (!IS_TAURI) {
    mockSystem.brightness = value;
    return delay(clone(mockSystem), 20);
  }
  return invoke<SystemState>("island_set_brightness", { value });
}

export async function toggleRadio(
  kind: "wifi" | "bluetooth",
  on: boolean
): Promise<SystemState> {
  if (!IS_TAURI) {
    if (kind === "wifi") mockSystem.wifi = on;
    else mockSystem.bluetooth = on;
    return delay(clone(mockSystem), 20);
  }
  return invoke<SystemState>("island_toggle_radio", { kind, on });
}

export async function openSystemSettings(kind: string): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_open_system_settings", { kind });
}

// ===== 剪贴板 =====

export async function getClipboard(): Promise<ClipItem[]> {
  if (!IS_TAURI) return delay(clone(mockClipState));
  return invoke<ClipItem[]>("island_clipboard");
}

export async function copyClipboard(id: number): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_clipboard_copy", { id });
}

export async function clearClipboard(): Promise<void> {
  if (!IS_TAURI) {
    // 与 Rust 侧一致：清空保留钉住的条目
    mockClipState = mockClipState.filter((item) => item.pinned);
    return delay(undefined, 20);
  }
  return invoke("island_clipboard_clear");
}

// ===== 消息 =====

export async function getNotify(): Promise<NotifyState> {
  if (!IS_TAURI) return delay(clone(mockNotify));
  return invoke<NotifyState>("island_notify");
}

export async function markNotifyRead(): Promise<NotifyState> {
  if (!IS_TAURI) {
    mockNotify.items.forEach((i) => {
      i.read = true;
    });
    mockNotify.unread = 0;
    return delay(clone(mockNotify), 20);
  }
  return invoke<NotifyState>("island_notify_mark_read");
}

export async function clearNotify(kind?: string): Promise<NotifyState> {
  if (!IS_TAURI) {
    mockNotify.items = kind
      ? mockNotify.items.filter((i) => i.kind !== kind)
      : [];
    mockNotify.unread = mockNotify.items.filter((i) => !i.read).length;
    return delay(clone(mockNotify), 20);
  }
  return invoke<NotifyState>("island_notify_clear", { kind: kind ?? null });
}

export async function openNotifyApp(kind: string, exe?: string): Promise<NotifyState> {
  if (!IS_TAURI) return delay(clone(mockNotify), 20);
  return invoke<NotifyState>("island_notify_open", { kind, exe: exe || null });
}

export async function openNotifySettings(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("island_notify_open_settings");
}

// ===== 事件 =====

export function onEvent<T>(name: string, handler: (payload: T) => void): () => void {
  if (!IS_TAURI) return () => {};
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

// ===== 工具箱（二维码 / 图片压缩转换）=====

/** 生成二维码 PNG（data URL，不落盘）。空内容 / 超容量由后端给中文错误。 */
export async function toolboxQrPng(
  text: string,
  ecLevel: string,
  scale: number,
  quiet: number
): Promise<ToolboxQrPng> {
  if (!IS_TAURI) return delay(mockToolboxQr(text, scale, quiet));
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
    const png = mockToolboxQr(text, scale, quiet);
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
      preview: mockToolboxImagePreview(),
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
    mockClipState = mockClipState.filter((item) => item.id !== id);
    return delay(mockClipState);
  }
  return invoke<ClipItem[]>("island_clipboard_delete", { id });
}

/** 调整条数上限（后端夹到 5–100 并立即淘汰） */
export async function clipboardSetLimit(limit: number): Promise<ClipItem[]> {
  if (!IS_TAURI) {
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
  if (!IS_TAURI) return delay(mockSnippetState);
  return invoke<Snippet[]>("list_snippets");
}

export async function saveSnippet(
  id: string | null,
  name: string,
  body: string
): Promise<Snippet> {
  if (!IS_TAURI) {
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
    mockSnippetState = mockSnippetState.filter((item) => item.id !== id);
    return delay(undefined, 40);
  }
  return invoke("delete_snippet", { id });
}

// ===== 启动器 =====

/** 近 30 天的项目清单（只取展示字段） */
export async function launcherProjects(): Promise<LauncherProject[]> {
  if (!IS_TAURI) return delay(mockProjects);
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

let mockMeetingItems: MeetingItem[] = clone(mockMeetings);
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
  if (!IS_TAURI) return delay(clone(mockMeetingDevices));
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
  if (!IS_TAURI) return delay(clone(mockMeetingItems));
  return invoke<MeetingItem[]>("meeting_list");
}

export async function meetingDelete(id: string): Promise<void> {
  if (!IS_TAURI) {
    mockMeetingItems = mockMeetingItems.filter((item) => item.id !== id);
    return delay(undefined, 40);
  }
  return invoke("meeting_delete", { id });
}

/** 重命名（wav 与侧车 json 一起改名），返回改名后的条目 */
export async function meetingRename(id: string, newName: string): Promise<MeetingItem> {
  if (!IS_TAURI) {
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