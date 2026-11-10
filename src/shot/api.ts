import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { IS_TAURI } from "../island/api";
import {
  mockDesktop,
  mockHistory,
  mockMonitor,
  mockOcr,
  mockPin,
  mockQr,
  mockRecordExport,
  mockRecordMedia,
  mockRecordStart,
  mockRecordState,
  mockScroll,
  mockSession,
  mockShotSettings,
  mockWindows,
} from "./mock";
import type {
  OcrResult,
  PinData,
  QrResult,
  RecordOptions,
  RecordStarted,
  RecordState,
  ScrollResult,
  ShotHistoryItem,
  ShotMonitor,
  ShotRect,
  ShotSession,
  ShotSettings,
  ShotWindow,
} from "./types";

function delay<T>(value: T, ms = 80): Promise<T> {
  return new Promise((resolve) => window.setTimeout(() => resolve(value), ms));
}

export async function listShotMonitors(): Promise<ShotMonitor[]> {
  if (!IS_TAURI) return delay([mockMonitor]);
  return invoke<ShotMonitor[]>("shot_monitors");
}

export async function getShotSettings(): Promise<ShotSettings> {
  if (!IS_TAURI) return delay(mockShotSettings);
  return invoke<ShotSettings>("shot_settings");
}

export async function setShotSettings(settings: ShotSettings): Promise<ShotSettings> {
  if (!IS_TAURI) {
    Object.assign(mockShotSettings, settings);
    return delay(settings);
  }
  return invoke<ShotSettings>("shot_set_settings", { settings });
}

export async function startShot(
  mode: "region" | "full" | "window" | "scroll" | "record",
  delaySec: number,
  monitor?: number
): Promise<number> {
  if (!IS_TAURI) return delay(1);
  return invoke<number>("shot_start", { mode, delaySec, monitor: monitor ?? null });
}

export async function editShotFile(path: string): Promise<number> {
  if (!IS_TAURI) return delay(1);
  return invoke<number>("shot_edit_file", { path });
}

export async function getShotSession(id: number): Promise<ShotSession> {
  if (!IS_TAURI) {
    const params = new URLSearchParams(window.location.search);
    const mode =
      params.get("edit") === "1" ? "edit" : params.get("window") === "1" ? "window" : "region";
    return delay(mockSession(mode), 30);
  }
  return invoke<ShotSession>("shot_session", { id });
}

/** 覆盖层里可点选的窗口列表（macOS 走 CoreGraphics，其它平台为空）。 */
export async function getShotWindows(id: number): Promise<ShotWindow[]> {
  if (!IS_TAURI) return delay(mockWindows, 20);
  return invoke<ShotWindow[]>("shot_windows", { id });
}

/** 原始 PNG 字节；Tauri raw IPC 返回 ArrayBuffer。 */
export async function getSessionImage(id: number): Promise<ArrayBuffer> {
  if (!IS_TAURI) {
    const session = mockSession("edit");
    return fetch(session.image ?? "").then((response) => response.arrayBuffer());
  }
  return invoke<ArrayBuffer>("shot_session_image", { id });
}

export async function shotReady(id: number): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("shot_ready", { id });
}

export async function markPainted(id: number): Promise<void> {
  if (!IS_TAURI) return;
  return invoke("shot_mark_painted", { id });
}

/** 打开截图页时预热覆盖窗口（WebView 提前建好，首次截图不再冷启动）。 */
export async function prewarmShot(): Promise<void> {
  if (!IS_TAURI) return;
  try {
    await invoke("shot_prewarm");
  } catch {
    /* 预热失败不影响后续正常截图 */
  }
}

export async function closeShot(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 10);
  return invoke("shot_close");
}

export async function scrollShot(id: number, rect: ShotRect): Promise<ScrollResult> {
  if (!IS_TAURI) {
    return delay(
      mockScroll(mockDesktop(), Math.round(rect.width), Math.round(rect.height)),
      500
    );
  }
  return invoke<ScrollResult>("shot_scroll", { id, rect });
}

/** 开始录屏：覆盖层会被挪出屏幕，右下角浮出录制条。 */
export async function startRecord(
  id: number,
  rect: ShotRect,
  options: RecordOptions
): Promise<RecordStarted> {
  if (!IS_TAURI) return mockRecordStart(options.format);
  return invoke<RecordStarted>("shot_record_start", { id, rect, options });
}

/** 录制条轮询：已录时长 / 是否结束 / 结果。 */
export async function getRecordState(recordId: number): Promise<RecordState> {
  if (!IS_TAURI) return mockRecordState(recordId);
  return invoke<RecordState>("shot_record_state", { recordId });
}

export async function stopRecord(recordId: number): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 10);
  return invoke("shot_record_stop", { recordId });
}

/** 录制结束后把录制条原地放大成预览窗。 */
export async function previewRecord(recordId: number): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 10);
  return invoke("shot_record_preview", { recordId });
}

/** 预览窗要播放的媒体路径（已放进 asset 协议白名单）。 */
export async function recordSrc(recordId: number): Promise<string> {
  if (!IS_TAURI) return delay("");
  return invoke<string>("shot_record_src", { recordId });
}

/** asset 协议播不了时的兜底：直接拿字节做 blob URL。 */
export async function recordMedia(recordId: number): Promise<ArrayBuffer> {
  if (!IS_TAURI) return mockRecordMedia();
  return invoke<ArrayBuffer>("shot_record_media", { recordId });
}

/** GIF 改帧率：后端按新帧率重新编码，返回最新状态。 */
export async function setGifFps(recordId: number, fps: number): Promise<RecordState> {
  if (!IS_TAURI) return delay({ ...(await mockRecordState(recordId)), gifFps: fps }, 60);
  return invoke<RecordState>("shot_record_gif_fps", { recordId, fps });
}

/** 复制 / 下载，`startMs`~`endMs` 是裁剪区间。 */
export async function exportRecord(
  recordId: number,
  startMs: number,
  endMs: number,
  action: "copy" | "save"
): Promise<string | null> {
  if (!IS_TAURI) return mockRecordExport(action);
  return invoke<string | null>("shot_record_export", { recordId, startMs, endMs, action });
}

/** 关闭预览窗并收掉截图会话。 */
export async function closeRecord(recordId: number): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 10);
  return invoke("shot_record_close", { recordId });
}

/** 取消录制（或录制出错）：收起录制窗，覆盖层回到原位并提示原因。 */
export async function cancelRecord(recordId: number, message = ""): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 10);
  return invoke("shot_record_cancel", { recordId, message });
}

/** 录制中出错：后端把原因推回覆盖层，由它弹提示。 */
export function onRecordError(handler: (message: string) => void): () => void {
  if (!IS_TAURI) return () => {};
  let unlisten: UnlistenFn | null = null;
  let dead = false;
  listen<string>("zbar://shot-record-error", (event) => handler(event.payload))
    .then((fn) => {
      if (dead) fn();
      else unlisten = fn;
    })
    .catch(() => {});
  return () => {
    dead = true;
    if (unlisten) unlisten();
  };
}

export async function saveShot(dataUrl: string, format?: string): Promise<ShotHistoryItem> {
  if (!IS_TAURI) {
    const item = mockHistory()[0];
    return delay(item, 220);
  }
  return invoke<ShotHistoryItem>("shot_save", { dataUrl, format: format ?? null });
}

/** 另存为：弹系统保存对话框；用户取消返回 null。 */
export async function saveShotAs(
  dataUrl: string,
  format?: string
): Promise<ShotHistoryItem | null> {
  if (!IS_TAURI) {
    return delay(mockHistory()[0], 220);
  }
  return invoke<ShotHistoryItem | null>("shot_save_as", { dataUrl, format: format ?? null });
}

export async function copyShot(dataUrl: string): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 100);
  return invoke("shot_copy", { dataUrl });
}

export async function copyText(text: string): Promise<void> {
  if (!IS_TAURI) {
    await navigator.clipboard.writeText(text);
    return delay(undefined, 30);
  }
  return invoke("shot_copy_text", { text });
}

export async function listShotHistory(limit = 12): Promise<ShotHistoryItem[]> {
  if (!IS_TAURI) return delay(mockHistory());
  return invoke<ShotHistoryItem[]>("shot_history", { limit });
}

export async function copyShotFile(path: string): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 100);
  return invoke("shot_copy_file", { path });
}

export async function pinShotFile(path: string): Promise<PinData> {
  if (!IS_TAURI) return delay(mockPin(mockDesktop(), 1600, 1000), 150);
  return invoke<PinData>("shot_pin_file", { path });
}

export async function deleteShot(path: string): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 100);
  return invoke("shot_delete", { path });
}

export async function openShot(path: string): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 50);
  return invoke("shot_open", { path });
}

export async function openShotFolder(): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("shot_open_folder");
}

export async function pinShot(dataUrl: string): Promise<PinData> {
  if (!IS_TAURI) {
    return delay(mockPin(dataUrl, 1600, 1000), 150);
  }
  return invoke<PinData>("shot_pin", { dataUrl });
}

export async function getPinData(id: number): Promise<PinData> {
  if (!IS_TAURI) {
    return delay(mockPin(mockDesktop(), 1600, 1000), 30);
  }
  return invoke<PinData>("shot_pin_data", { id });
}

export async function closePin(id: number): Promise<void> {
  if (!IS_TAURI) return delay(undefined, 20);
  return invoke("shot_pin_close", { id });
}

export async function ocrShot(dataUrl: string): Promise<OcrResult> {
  if (!IS_TAURI) return delay(mockOcr(), 700);
  return invoke<OcrResult>("shot_ocr", { dataUrl });
}

export async function qrShot(dataUrl: string): Promise<QrResult> {
  if (!IS_TAURI) return delay(mockQr(), 400);
  return invoke<QrResult>("shot_qr", { dataUrl });
}

export function onShotSaved(handler: (item: ShotHistoryItem) => void): () => void {
  if (!IS_TAURI) return () => {};
  let unlisten: UnlistenFn | null = null;
  let dead = false;
  listen<ShotHistoryItem>("zbar://shot-saved", (event) => handler(event.payload))
    .then((fn) => {
      if (dead) fn();
      else unlisten = fn;
    })
    .catch(() => {});
  return () => {
    dead = true;
    if (unlisten) unlisten();
  };
}
