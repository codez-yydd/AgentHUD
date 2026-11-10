export interface ShotMonitor {
  index: number;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  scaleFactor: number;
  primary: boolean;
}

export interface ShotRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ShotSession {
  id: number;
  mode: "region" | "edit";
  /** 前端从 shot_session_image 取回字节后创建的 blob URL（不在会话 JSON 里） */
  image?: string;
  backdrop?: string;
  width: number;
  height: number;
  origin: ShotRect | null;
  monitor: ShotMonitor;
  title: string;
  createdAt: number;
}

export interface ShotHistoryItem {
  name: string;
  path: string;
  width: number;
  height: number;
  createdAt: number;
  thumb: string;
  size: number;
}

export interface PinData {
  id: number;
  image: string;
  width: number;
  height: number;
}

export interface OcrResult {
  ok: boolean;
  text: string;
  lines: string[];
  error: string;
}

export interface QrResult {
  ok: boolean;
  texts: string[];
  error: string;
}

export interface ScrollResult {
  width: number;
  height: number;
  frames: number;
}

export interface RecordResult {
  path: string;
  width: number;
  height: number;
  frames: number;
  durationMs: number;
  size: number;
}

export interface ShotSettings {
  autoCopy: boolean;
  autoPin: boolean;
  playSound: boolean;
  format: "png" | "jpg";
  delaySec: number;
  hotkeyRegion: string;
  hotkeyWindow: string;
  hotkeyFull: string;
}
