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
  /** region/window/scroll/record = 覆盖层选区分支；edit = 直接进入标注 */
  mode: "region" | "window" | "scroll" | "record" | "edit";
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

/** 覆盖层里可点选的窗口，坐标与截图会话的像素坐标一致。 */
export interface ShotWindow {
  x: number;
  y: number;
  width: number;
  height: number;
  app: string;
  title: string;
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
  /** 唯一截图快捷键：呼出覆盖层后在里面切换区域/窗口/全屏 */
  hotkey: string;
}
