/** 灵动岛前后端数据契约（与 src-tauri/src/island/* 的 serde camelCase 对齐） */

export type ExpandMode = "hover" | "never";
export type IslandEdge = "top" | "left" | "right";
export type TabKey =
  | "home"
  | "msg"
  | "stock"
  | "weather"
  | "ctl"
  | "clip"
  | "shot"
  | "tool"
  | "settings";

export interface LauncherItem {
  name: string;
  path: string;
  icon: string;
}

export interface IslandConfig {
  enabled: boolean;
  /** 0 = 完全不透明，90 = 尽量透出桌面；只影响玻璃色层，不影响文字 */
  transparency: number;
  /** 界面基准字号（逻辑 px，13 = 标准）；只缩放文字，不改窗口几何 */
  fontSize: number;
  expandMode: ExpandMode;
  replaceLegacyBar: boolean;
  weatherCity: string;
  weatherLabel: string;
  weatherLat: number | null;
  weatherLon: number | null;
  edge: IslandEdge;
  posFrac: number;
  /** 上次吸附的显示器设备名；空/不存在的显示器回退主屏 */
  monitor?: string | null;
  stocks: string[];
  stockIntervalSec: number;
  stockAlertPct: number;
  hotkeyToggle: string;
  hideFullscreen: boolean;
  hiddenTabs: string[];
  launcher: LauncherItem[];
  /** 收起胶囊环 / 主页额度卡的顺序（agent id）；空 = 数据源默认顺序 */
  agentOrder?: string[];
}

export interface IslandState {
  enabled: boolean;
  config: IslandConfig;
  expanded: boolean;
}

export interface HudBarQuota {
  key: string;
  labelZh: string;
  labelEn: string;
  usedPct: number;
  resetAt: number | null;
}

export interface HudBarBalance {
  amount: number;
  currency: string;
  granted?: number;
  toppedUp?: number;
}

export interface HudBarAgent {
  id: string;
  source: string;
  account?: string;
  plan?: string;
  status?: "unavailable" | "usage";
  quotas: HudBarQuota[];
  balance?: HudBarBalance;
}

export interface HudBarUsage {
  ts: number;
  agents: HudBarAgent[];
}

export interface TodayUsageTotals {
  tokens: number;
  cost: number;
  currency: string;
  updatedAt: number;
}

export interface LocatedPlace {
  ok: boolean;
  error: string;
  country: string;
  province: string;
  city: string;
  district: string;
  lat: number;
  lon: number;
  label: string;
}

export interface IslandDragBegin {
  active: boolean;
}

export interface IslandDragStep {
  active: boolean;
  dx: number;
  dy: number;
  /** 是否已越过拖动阈值；false 表示只是按住（点击语义交给 click） */
  started: boolean;
}

export interface IslandDragEnd {
  edge: IslandEdge;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface WeatherHour {
  time: string;
  label: string;
  temp: number;
  code: number;
  icon: string;
  desc: string;
  precip: number;
}

export interface WeatherDay {
  date: string;
  weekday: string;
  code: number;
  icon: string;
  desc: string;
  max: number;
  min: number;
}

export interface WeatherState {
  ok: boolean;
  error: string;
  city: string;
  updatedAt: number;
  temp: number;
  feelsLike: number;
  humidity: number;
  wind: number;
  code: number;
  desc: string;
  icon: string;
  isDay: boolean;
  sunrise: string;
  sunset: string;
  hourly: WeatherHour[];
  daily: WeatherDay[];
}

export interface StockQuote {
  code: string;
  name: string;
  price: number;
  prevClose: number;
  open: number;
  chg: number;
  pct: number;
  high: number;
  low: number;
  time: string;
  market: string;
  status: string;
}

export interface StocksState {
  ok: boolean;
  error: string;
  updatedAt: number;
  quotes: StockQuote[];
}

export interface StockSearchItem {
  code: string;
  name: string;
}

export interface StockMinute {
  code: string;
  prevClose: number;
  points: number[];
}

export interface MediaState {
  hasSession: boolean;
  appId: string;
  appName: string;
  title: string;
  artist: string;
  album: string;
  playing: boolean;
  position: number;
  duration: number;
  canPrev: boolean;
  canNext: boolean;
  cover: string;
  source: string;
}

export interface SystemState {
  volume: number;
  muted: boolean;
  brightness: number | null;
  brightnessSupported: boolean;
  wifi: boolean | null;
  wifiAvailable: boolean;
  bluetooth: boolean | null;
  bluetoothAvailable: boolean;
  error: string;
  updatedAt: number;
}

export interface ClipItem {
  id: number;
  kind: "text" | "image";
  text: string;
  preview: string;
  image: string;
  time: number;
  width: number;
  height: number;
  /** 钉住：不会被新内容挤出、也不会被「清空」清掉 */
  pinned: boolean;
}

/** 提示词片段（~/.agenthud/snippets.json；wire 为 snake_case） */
export interface Snippet {
  id: string;
  name: string;
  body: string;
  created_at: number;
  updated_at: number;
}

/** 启动器用到的项目摘要（只取展示所需字段，接口与 get_projects 同源） */
export interface LauncherProject {
  key: string;
  display_path: string | null;
  sessions: number;
}

// ===== 会议录音（工具箱「会议」子页；wire 为 snake_case）=====

/** 音频输入设备（下拉用；cpal 无跨平台稳定 id，id 即设备名） */
export interface MeetingAudioDevice {
  id: string;
  name: string;
}

/** 时间标记：ms 为有效时间轴（扣除暂停段）上的位置 */
export interface MeetingMark {
  ms: number;
  label: string;
}

/** 开始录音后的会话信息 */
export interface MeetingSessionInfo {
  id: string;
  name: string;
  path: string;
  dir: string;
  started_at: number;
  device: string;
  sample_rate: number;
}

/** 录制中的会话快照（子页重挂载后恢复录制视图用） */
export interface MeetingSessionBrief {
  id: string;
  name: string;
  device: string;
  sample_rate: number;
  started_at: number;
  paused: boolean;
  effective_ms: number;
  marks: MeetingMark[];
  dir: string;
}

/** 结束录音的结果 */
export interface MeetingStopResult {
  id: string;
  name: string;
  path: string;
  duration_ms: number;
  size: number;
  marks: number;
  sample_rate: number;
  /** 非 null = 文件已保存但录制期间有过写入告警；null = 全程无告警 */
  warning: string | null;
}

/** 最近录音列表条目 */
export interface MeetingItem {
  id: string;
  name: string;
  started_at: number;
  duration_ms: number;
  size: number;
  sample_rate: number;
  device: string;
  marks: MeetingMark[];
}

/** 首次使用隐私确认状态 + 保存目录 */
export interface MeetingPrivacy {
  confirmed: boolean;
  dir: string;
}

/** 会议录音设置（camelCase wire）：自动音量默认开 */
export interface MeetingSettings {
  privacyConfirmed: boolean;
  autoGain: boolean;
}

export interface NotifyItem {
  id: number;
  kind: string;
  name: string;
  sender: string;
  body: string;
  time: number;
  read: boolean;
  /** 来源进程名（小写，可能为空），点消息时优先用它定位应用 */
  exe: string;
}

export interface NotifyState {
  items: NotifyItem[];
  unread: number;
}

export interface LyricsState {
  ok: boolean;
  error: string;
  title: string;
  artist: string;
  lrc: string;
  translation: string;
  source: string;
}

export interface StockAlert {
  code: string;
  name: string;
  pct: number;
  price: number;
  text: string;
}

// ===== 工具箱（二维码 / 图片压缩转换）=====
// wire 字段是 snake_case（Rust 侧没有 rename_all），与用量系契约一致。

/** 二维码生成结果：data_url 可直接给 <img src> */
export interface ToolboxQrPng {
  data_url: string;
  /** 逻辑模块数（不含静区） */
  modules: number;
  /** 生成图边长（像素，含静区） */
  size: number;
  bytes: number;
}

/** 二维码落盘结果 */
export interface ToolboxQrSaved {
  path: string;
  bytes: number;
  size: number;
}

/** 选中图片的元信息 + 720px 预览（preview 为 JPEG data URL） */
export interface ToolboxImageInfo {
  path: string;
  width: number;
  height: number;
  bytes: number;
  /** 按扩展名粗分：png / jpeg / webp / gif / bmp / 其他 */
  format: string;
  preview: string;
}

/** 图片转换落盘结果 */
export interface ToolboxImageOut {
  path: string;
  width: number;
  height: number;
  bytes: number;
  format: string;
}
