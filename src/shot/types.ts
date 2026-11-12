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
  /** "mp4" | "gif" */
  format: string;
  recordId: number;
  /** MP4 是否带音轨（GIF 恒为 false；预览窗据此显示声音标识） */
  hasAudio: boolean;
}

/** 开始录制后返回的信息（录制条据此显示上限和格式） */
export interface RecordStarted {
  recordId: number;
  format: string;
  width: number;
  height: number;
  maxSeconds: number;
  /** 非致命提示（如「未检测到系统声设备」），调用方 toast 一次 */
  notice: string | null;
}

/** 录制中/结束后的状态，录制条轮询它 */
export interface RecordState {
  running: boolean;
  elapsedMs: number;
  format: string;
  result: RecordResult | null;
  error: string | null;
  /** GIF 当前帧率与重编码进度（MP4 为 0） */
  gifFps: number;
  progress: number;
}

export interface RecordOptions {
  format: "mp4" | "gif";
  /** 是否把鼠标指针画进画面 */
  cursor: boolean;
  /** 麦克风设备（仅 Windows MP4；null = 不录麦克风） */
  mic?: string | null;
}

/** 音频输入设备：name 传给后端，label 用于显示 */
export interface AudioDevice {
  name: string;
  label: string;
}

/** ffmpeg 就位状态 */
export interface FfmpegStatus {
  ready: boolean;
  path: string | null;
}

/** ffmpeg 下载进度事件（zbar://ffmpeg-download） */
export interface FfmpegDownloadProgress {
  /** download | extract | verify | done | error */
  stage: string;
  sourceIndex: number;
  sourceTotal: number;
  downloaded: number;
  total: number | null;
  message: string;
}

export interface ShotSettings {
  autoCopy: boolean;
  autoPin: boolean;
  playSound: boolean;
  format: "png" | "jpg";
  delaySec: number;
  /** 唯一截图快捷键：呼出覆盖层后在里面切换区域/窗口/全屏 */
  hotkey: string;
  /** 录屏麦克风开关（仅 Windows MP4 生效；GIF 无音轨） */
  micEnabled: boolean;
  /** 上次选择的麦克风设备（跨重启保持选择） */
  micDevice: string;
}
