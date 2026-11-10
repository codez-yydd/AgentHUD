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
  mockScroll,
  mockSession,
  mockShotSettings,
} from "./mock";
import type {
  OcrResult,
  PinData,
  QrResult,
  RecordResult,
  ScrollResult,
  ShotHistoryItem,
  ShotMonitor,
  ShotRect,
  ShotSession,
  ShotSettings,
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
  mode: "region" | "full" | "window",
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
    return delay(mockSession(params.get("edit") === "1"), 30);
  }
  return invoke<ShotSession>("shot_session", { id });
}

/** 原始 PNG 字节；Tauri raw IPC 返回 ArrayBuffer。 */
export async function getSessionImage(id: number): Promise<ArrayBuffer> {
  if (!IS_TAURI) {
    const session = mockSession(true);
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

export async function recordGif(id: number, rect: ShotRect, seconds = 8): Promise<RecordResult> {
  if (!IS_TAURI) {
    return delay({
      path: "C:\\Users\\admin\\Pictures\\AgentHUD Screenshots\\Recording_preview.gif",
      width: Math.min(1000, Math.round(rect.width)),
      height: Math.round(rect.height),
      frames: Math.round(seconds * 8),
      durationMs: seconds * 1000,
      size: 2_400_000,
    }, 900);
  }
  return invoke<RecordResult>("shot_record_gif", { id, rect, seconds });
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
