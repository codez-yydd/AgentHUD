/** 灵动岛前后端数据契约（与 src-tauri/src/island/* 的 serde camelCase 对齐） */

/**
 * 展开方式三态：
 * - hover：悬停展开、移出收起（默认）
 * - always：常驻展开，不因移出收起；点击收起后切回 hover
 * - never：不自动展开（「长关」），只能手动点击 / 热键展开
 */
export type ExpandMode = "hover" | "always" | "never";

/** 主题模式：auto = 按身后桌面平均亮度自动在 dark / light 间切换 */
export type IslandThemeMode = "auto" | "light" | "dark" | "rouge" | "sky";

/** 实际生效的色板（auto 解析后只会落到这四套之一） */
export type IslandThemePalette = Exclude<IslandThemeMode, "auto">;

/**
 * 收起态胶囊的视图序列。视图内容由 P1 批次实现，本轮只维护配置值与序号：
 * 滚轮在 rings（额度环）/ clock（时钟）/ pet（宠物）/ music（音乐）之间循环。
 */
export type CompactView = "rings" | "clock" | "pet" | "music";

export type IslandQuotaDisplay = "percent" | "rings";

/** 纯额度环的配色方式：period 按周期固定分色，dynamic 按已用额度动态绿黄红 */
export type IslandQuotaRingStyle = "period" | "dynamic";

/** 吸附边：top/bottom 横条（bottom 时窗口贴工作区下沿、条在窗口底部）；left/right 竖胶囊 */
export type IslandEdge = "top" | "bottom" | "left" | "right";
/**
 * 岛内页面标签。设置已迁到独立窗口（island-settings.html，齿轮打开），
 * 不再占用标签位——因此这里没有 "settings"，岛内也没有对应的页面状态。
 */
export type TabKey =
  | "home"
  | "msg"
  | "stock"
  | "weather"
  | "ctl"
  | "clip"
  | "translate"
  | "shot"
  | "tool";

export interface LauncherItem {
  name: string;
  path: string;
  icon: string;
}

export interface IslandConfig {
  enabled: boolean;
  /** 0 = 底色最浓，90 = 尽量透出桌面。 */
  transparency: number;
  /** 界面基准字号（逻辑 px，13 = 标准）；只缩放文字，不改窗口几何 */
  fontSize: number;
  expandMode: ExpandMode;
  /** 置顶窗口（旧响应缺省为 true）。 */
  keepOnTop?: boolean;
  /** Windows 将窗口从截图和录屏中排除，允许取得对齐的桌面底板。 */
  excludeFromCapture?: boolean;
  /** 收起态空闲呼吸动画开关（默认开启）；关闭时圆角裁剪照常，仅无动画。 */
  breathEnabled?: boolean;
  // ===== P0 新增配置（Rust 侧并行提供；旧配置缺字段时前端兜底）=====
  /** 主题：auto 跟随背景亮度，light/dark/rouge/sky 为固定色板；缺省按 dark */
  theme?: IslandThemeMode;
  /** 悬停多少 ms 后展开（默认 120） */
  hoverExpand?: boolean;
  hoverExpandMs?: number;
  /** 移出多少 ms 后收起（默认 320） */
  collapseDelayMs?: number;
  /** 收起态胶囊当前视图（默认 rings；视图内容 P1 实现） */
  compactView?: CompactView;
  /** 收起态额度的显示方式，缺省显示百分比。 */
  quotaDisplay?: IslandQuotaDisplay;
  /** 纯额度环（quotaDisplay = rings）的配色方式，缺省按周期固定分色。 */
  quotaRingStyle?: IslandQuotaRingStyle;
  /** 收起态视图自动轮换秒数（30；0 = 不轮换。P1 内容落地后生效） */
  autoCycleSec?: number;
  /** 鼠标可穿透（点击落到身后的窗口，只保留右键入口） */
  passthrough?: boolean;
  /** 上次停留的标签；null = 没记录过，回退 defaultTab */
  lastTab?: TabKey | null;
  /** 毛玻璃底图采样帧率（8；越高越流畅也越费电） */
  blurFps?: number;
  /** 自定义字体 family；空 = 内置字体栈 */
  fontFamily?: string;
  // ===== P5：媒体 /「打开应用」接线（Rust 侧 clamp 后广播）=====
  /** 媒体开始播放时自动把收起胶囊切到 music 视图（默认 true） */
  mediaAutoShow?: boolean;
  /** 是否拉取歌词（默认 true；关掉后不再请求 island_lyrics） */
  mediaLyrics?: boolean;
  /** Windows 网易云本地播放日志（默认 true） */
  mediaNeteaseLog?: boolean;
  /** Windows 播放状态 JSON / JSONL 文件路径 */
  mediaPlaybackLogs?: string[];
  /** 歌词源顺序（逗号分隔，白名单 netease/lrclib/custom） */
  mediaLyricsSources?: string;
  /** 自定义歌词接口 URL 模板（占位符 {title}{artist}{album}{duration}{song_id}） */
  mediaLyricsCustomUrl?: string;
  /** SMTC 播放器优先级（子串匹配 appId / 友好名） */
  mediaPriority?: string[];
  /** 「打开应用」自定义路径（kind → exe 路径，支持 %VAR%） */
  notifyOpen?: Record<string, string>;
  /** 首次打开停留的标签（默认 home） */
  defaultTab?: TabKey;
  replaceLegacyBar: boolean;
  weatherCity: string;
  weatherLabel: string;
  weatherLat: number | null;
  weatherLon: number | null;
  /** 天气页自定义城市列表（主城市单独存 weatherCity；空 = 跟随 IP） */
  weatherCities?: WeatherCityConfig[];
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
  // ===== P2：消息页模块开关（Rust 侧白名单 qq/wechat/mail/feishu/dingtalk，
  // 归一化后按固定顺序输出；允许空数组 = 消息功能整体关闭） =====
  /** 消息页启用的模块；缺省（旧配置）按 ["qq","wechat","mail"] */
  notifyModules?: string[];
  /** 哪些模块把正文放进描述（默认 ["feishu","dingtalk"]） */
  notifyBodyPreview?: string[];
  launcher: LauncherItem[];
  /** 收起胶囊环 / 主页额度卡的顺序（agent id）；空 = 数据源默认顺序 */
  agentOrder?: string[];
}

export interface IslandState {
  doubleClickMs?: number;
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

export interface IslandBodyRect {
  /** 画布坐标（780×632），不是窗口/屏幕坐标 */
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface IslandDragSizes {
  horizontal: Pick<IslandBodyRect, "width" | "height">;
  vertical: Pick<IslandBodyRect, "width" | "height">;
}

export interface IslandDragStep {
  active: boolean;
  dx: number;
  dy: number;
  /** 是否已越过拖动阈值；false 表示只是按住（点击语义交给 click） */
  started: boolean;
  /** P4：当前最近吸附边 / 本体到边的 gap / 吸附线在画布里的位置 */
  edge?: IslandEdge;
  gap?: number;
  edgeLine?: number;
  edgeLines?: Record<IslandEdge, number>;
  origin?: { x: number; y: number };
  monitor?: string | null;
  squash?: number;
  /** 本体画布矩形（新 Rust 契约；预览可能缺省） */
  bodyRect?: IslandBodyRect;
  /** 速度方向（画布 px/s），用于拉丝泪滴形变 */
  vx?: number;
  vy?: number;
  /** 是否已进入吸附静置收尾 */
  settled?: boolean;
}

export interface IslandDragEnd {
  edge: IslandEdge;
  x: number;
  y: number;
  width: number;
  height: number;
  /** P4：目标边 anchored 模型下的主体画布矩形 */
  bodyRect?: IslandBodyRect;
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

/** 天气页自定义城市配置（Rust WeatherCityCfg / settings.json weather.cities） */
export interface WeatherCityConfig {
  name: string;
  lat: number | null;
  lon: number | null;
}

/** 多城市显示列表里的单个城市详情（与主城市顶层字段同口径） */
export interface WeatherCityState {
  /** 用于 geocode 的市名 */
  name: string;
  /** 显示名：IP 城市「省 · 市」，绑定 / 自定义城市用名字 */
  label: string;
  lat: number | null;
  lon: number | null;
  isMain: boolean;
  isIp: boolean;
  isUser: boolean;
  ok: boolean;
  error: string;
  /** 本次取数失败但已回填最近成功快照：展示的是旧数据 */
  stale?: boolean;
  /** 失败时的简短错误摘要（用于顶部小提示） */
  errorBrief?: string;
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

export interface WeatherState {
  ok: boolean;
  error: string;
  /** 主城市取数失败但已回填最近成功快照：展示的是旧数据 */
  stale?: boolean;
  /** 失败时的简短错误摘要（用于顶部小提示） */
  errorBrief?: string;
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
  /** 显示列表：[主城市] +（绑定时）[IP 城市] + 其余自定义城市 */
  cities: WeatherCityState[];
  /** 主城市在 cities 里的下标；无主城市 = -1 */
  mainIndex: number;
  /** IP 城市在 cities 里的下标；主城市就是 IP 时为 null */
  ipIndex: number | null;
  /** 没有绑定城市（主城市跟随 IP） */
  followsIp: boolean;
  /** 温度单位：metric（°C）/ imperial（°F），由本次取数的数据源决定；空 = 按摄氏度 */
  units?: string;
}

/** 温度单位后缀：imperial → "°F"，其余（含空 / 未知）→ "°" */
export function tempUnitSuffix(weather: Pick<WeatherState, "units"> | null | undefined): string {
  return weather?.units === "imperial" ? "°F" : "°";
}

/** 风速单位后缀：imperial → "mph"（OpenWeather / Open-Meteo 均按 mph），其余 → "km/h" */
export function windUnitSuffix(weather: Pick<WeatherState, "units"> | null | undefined): string {
  return weather?.units === "imperial" ? "mph" : "km/h";
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
  canToggle: boolean;
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

export type ClashMode = "rule" | "global" | "direct";

export interface ClashStatus {
  supported: boolean;
  running: boolean;
  installed: boolean;
  mode?: ClashMode | "script" | null;
  mixedPort?: number | null;
  tun?: boolean | null;
  systemProxy?: boolean | null;
  canSetSystemProxy?: boolean;
  canSetTun?: boolean;
  canSetMode?: boolean;
  traffic?: { uploadSpeed: number; downloadSpeed: number } | null;
  subscription?: { used: number; total: number } | null;
  ipLocation?: { ip: string; region: string } | null;
}

export type ClashToggleKind = "systemProxy" | "tun";

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
  wechatUnread?: number;
}

/** `island_notify_modules` 返回的模块开关快照 */
export interface NotifyModules {
  modules: string[];
  bodyPreview: string[];
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

// ===== 实时字幕（`live_*` 命令 / `zbar://island-live*` 事件）=====
// 与 src-tauri/src/island/live/* 的 serde camelCase 对齐：系统声音 / 麦克风 →
// Silero VAD 断句 → Parakeet 日语 ASR（定稿 + 滚动草稿）→ 流式翻译。

/** 字幕历史最多保留 / 展示的条数（收起态只取最新一条） */
export const LIVE_CAPTION_LIMIT = 80;

/** 最后一句字幕之后，收起态胶囊保持字幕的时间（之后回到原视图） */
export const LIVE_CAPTION_HOLD_MS = 7000;

/** 模型下载状态：done / total 为字节数（拿不到总大小时 total = 0） */
export interface LiveDownloadState {
  /** idle | downloading | verifying | done | error | cancelled */
  state: string;
  done: number;
  total: number;
  /** 当前正在下载的文件名 / 阶段名 */
  label: string;
  /** 失败原因（state = error 时非空） */
  message: string;
}

/** 实时字幕运行快照（命令返回 + `zbar://island-live` 事件载荷） */
export interface LiveStatus {
  running: boolean;
  /** idle | starting | listening | stopping | error */
  phase: string;
  /** 失败原因（phase = error 时非空） */
  error: string;
  /** 模型文件是否已就绪（false 时「开始聆听」会被后端拒绝） */
  modelReady: boolean;
  /** 采集来源：loopback（系统声音）/ mic（麦克风） */
  source: string;
  device: string;
  /** 最近一次音频电平 0..1（电平条用） */
  level: number;
  download: LiveDownloadState;
}

/** 一条字幕：草稿（draft）会滚动刷新，定稿（final）不再变化 */
export interface LiveCaption {
  id: number;
  jp: string;
  zh: string;
  draft: boolean;
  final: boolean;
  ts: number;
}

/** 可选音频设备列表（按采集来源分组；id 即设备名） */
export interface LiveDevices {
  loopback: string[];
  mic: string[];
}
