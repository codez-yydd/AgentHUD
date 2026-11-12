/** 灵动岛 API 封装：Tauri 环境走 invoke；浏览器预览回退到 mock.ts */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  mockClipboardWithImage,
  mockConfig,
  mockMedia,
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
  if (!IS_TAURI) return delay({ active: true, dx: 0, dy: 0, started: true }, 10);
  return invoke<IslandDragStep>("island_drag_step");
}

export async function endIslandDrag(): Promise<IslandDragEnd> {
  if (!IS_TAURI) return delay({ edge: "top", x: 0, y: 0, width: 552, height: 70 }, 10);
  return invoke<IslandDragEnd>("island_drag_end");
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