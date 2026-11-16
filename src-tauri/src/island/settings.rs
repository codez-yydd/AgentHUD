//! 灵动岛设置文档（`~/.agenthud/settings.json`）。
//!
//! 设置窗按 14 页 / 84 项组织，而岛运行时配置是内部
//! SQLite（`island.db`）里的 `IslandConfig`。两者字段有大量重叠、又各自
//! 有对方没有的项，所以这里单独维护一份 **唯一权威文档**：
//!
//! - `settings.json`：snake_case 嵌套 JSON，顶层按分组嵌套
//!   （`island` / `hotkeys` / `media` / `notify` / `dsh` / `mail` /
//!   `stocks` / `weather` / `translate` / `live` / `clipboard` /
//!   `launcher` / `clash` / `brightness` / `agenthud`），未知额外字段
//!   （launcher 的 args、weather.cities[].country 等）用 `#[serde(flatten)]`
//!   原样保留，方便导入既有配置或手改文件；
//! - `IslandConfig`：岛运行时只认这份 SQLite 配置，所以与运行时重叠的
//!   字段在保存文档后经 `overlay_from_island` / `sync_to_island` 双向同步。
//!
//! ## 读写纪律
//!
//! - 保存走 `apply_patch`：先把岛实时值 overlay 进文档，再叠加 **扁平点
//!   路径 patch**（如 `{"island.edge":"top"}`），只写 patch 里出现的键，
//!   绝不用整份表单覆盖未提交字段；
//! - 落盘是 tmp + rename 的原子写，pretty JSON + 末尾换行；
//! - 同步岛配置必须走 `island::apply_config`（clamped → SQLite → 广播
//!   `zbar://island-config` → 天气 / 股票 / 消息 / 热键副作用），不能绕开
//!   广播直接 `save_config`；
//! - 尚未接线的项只存盘，由 `unsupported()` 把点路径告诉前端做「未接线」提示。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{IslandConfig, LauncherItem, WeatherCityCfg};

/// 文档文件名（与 `island.db` 同目录：`~/.agenthud/`）。
pub const SETTINGS_FILE: &str = "settings.json";

const EDGES: [&str; 4] = ["top", "bottom", "left", "right"];
const THEMES: [&str; 5] = ["auto", "light", "dark", "rouge", "sky"];
const EXPAND_MODES: [&str; 3] = ["hover", "always", "never"];
const COMPACT_VIEWS: [&str; 4] = ["rings", "clock", "pet", "music"];
pub(crate) const POPUP_POSITIONS: [&str; 6] = [
    "center_bottom",
    "center_top",
    "bottom_left",
    "bottom_right",
    "top_left",
    "top_right",
];
const TEMPERATURE_UNITS: [&str; 2] = ["metric", "imperial"];
const AUDIO_SOURCES: [&str; 2] = ["loopback", "mic"];

/// 设置文档点路径里**已经接到岛运行时**的项；`island_settings_save` 用它
/// 计算 `applied`（本次同步到运行时配置或由服务直接读取的字段）。
const SYNCED_PATHS: [&str; 80] = [
    "island.edge",
    "island.pos",
    "island.screen",
    "island.theme",
    "island.font_family",
    "island.font_size",
    "island.transparency",
    "island.expand_mode",
    "island.compact_view",
    "island.quota_display",
    "island.quota_ring_style",
    "island.auto_cycle_s",
    "island.hover_expand",
    "island.hover_expand_ms",
    "island.collapse_delay_ms",
    "island.blur_fps",
    "island.passthrough",
    "island.hide_fullscreen",
    "island.hidden_tabs",
    "island.keep_on_top",
    "island.hug_fullscreen",
    "island.exclude_from_capture",
    "hotkeys.toggle",
    "hotkeys.translate",
    "hotkeys.clipboard",
    "media.auto_show",
    "media.lyrics",
    "media.netease_log",
    "media.playback_logs",
    "media.lyrics_sources",
    "media.lyrics_custom_url",
    "media.priority",
    "notify.modules",
    "notify.body_preview",
    "notify.popup.enabled",
    "notify.popup.position",
    "notify.popup.size",
    "notify.open.qq",
    "notify.open.wechat",
    "notify.open.mail",
    "notify.open.feishu",
    "notify.open.dingtalk",
    "notify.open.dsh",
    "dsh.enabled",
    "dsh.sessions_dir",
    "dsh.include_subagents",
    "dsh.skip_when_active",
    "dsh.body_chars",
    "mail.interval_s",
    "mail.imap",
    "stocks.watchlist",
    "stocks.interval",
    "stocks.pct_alert",
    "weather.api_key",
    "weather.city",
    "weather.lat",
    "weather.lon",
    "weather.units",
    "weather.interval_min",
    "weather.cities",
    "launcher",
    "agenthud.agent_order",
    "clipboard.max",
    "brightness.twinkle",
    // 实时字幕：island::live 的 live_start 启动时快照这些参数（命令 / 事件消费），
    // 16 个点路径与设置窗 LivePage 一一对应。
    "live.models_dir",
    "live.audio.source",
    "live.audio.device",
    "live.vad.threshold",
    "live.vad.min_silence_ms",
    "live.vad.min_speech_ms",
    "live.asr.partial",
    "live.asr.draft_interval_ms",
    "live.asr.final_threads",
    "live.asr.max_segment_s",
    "live.asr.context_prompt",
    "live.translate.enabled",
    "live.translate.live",
    "live.translate.live_min_interval_ms",
    "live.translate.live_min_new_chars",
    "live.translate.context_lines",
];

/// 本期尚未接线的点路径：这些键会存进 `settings.json` / 能往返，但不会
/// 影响岛运行时。前端据此在对应设置行上打「未接线」标记。
///
/// `translate.*` 已由 `island::translate` 的命令与翻译热键消费，不在此列。
/// `live.*` 已由 `island::live`（实时字幕命令 / 事件 / 启动快照）消费，16 个
/// 点路径全部接线，不在此列。
/// `weather.*` 已由 `island::weather` 消费（保存后触发一次刷新），也不在此列。
/// `brightness.twinkle` 已由 `island::system` 的亮度设置消费（只有 Windows
/// 分支），Windows 上不在此列；其它平台由 `unsupported()` 动态补上「未接线」。
const UNSUPPORTED_PATHS: &[&str] = &[];

// ============================================================
// 通用小工具
// ============================================================

/// 未知额外字段的落点（`#[serde(flatten)]`）：导入既有配置或手改
/// 文件时，没建模的键不会被悄悄丢掉。
type Extra = Map<String, Value>;

fn extra() -> Extra {
    Extra::new()
}

fn trimmed(value: &str) -> String {
    value.trim().to_string()
}

fn clamp_int(value: i64, lo: i64, hi: i64) -> i64 {
    value.clamp(lo, hi)
}

fn clamp_float(value: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(lo, hi)
    } else {
        fallback
    }
}

fn one_of(value: &str, allowed: &[&str], fallback: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if allowed.contains(&normalized.as_str()) {
        normalized
    } else {
        fallback.to_string()
    }
}

/// trim + 去空 + 去重（可选大小写不敏感）+ 截断，保持原顺序。
fn normalize_list(items: &[String], max: usize, case_insensitive: bool) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let value = item.trim();
        if value.is_empty() {
            continue;
        }
        let key = if case_insensitive {
            value.to_ascii_lowercase()
        } else {
            value.to_string()
        };
        if !seen.insert(key) {
            continue;
        }
        out.push(value.to_string());
        if out.len() >= max {
            break;
        }
    }
    out
}

fn normalize_hidden_tabs(items: &[String]) -> Vec<String> {
    let mut out: Vec<String> = items
        .iter()
        .map(|key| key.trim().to_ascii_lowercase())
        .filter(|key| super::can_hide_tab(key))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// 经纬度只接受「同时存在且都在合法范围」；否则成对清空。
fn normalize_coord_pair(lat: Option<f64>, lon: Option<f64>) -> (Option<f64>, Option<f64>) {
    match (lat, lon) {
        (Some(lat), Some(lon))
            if lat.is_finite()
                && lon.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lon) =>
        {
            (Some(lat), Some(lon))
        }
        _ => (None, None),
    }
}

// ============================================================
// 文档结构（snake_case，按分组嵌套）
// ============================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct SettingsDoc {
    pub island: IslandDoc,
    pub hotkeys: HotkeysDoc,
    pub media: MediaDoc,
    pub notify: NotifyDoc,
    pub dsh: DshDoc,
    pub mail: MailDoc,
    pub stocks: StocksDoc,
    pub weather: WeatherDoc,
    pub translate: TranslateDoc,
    pub live: LiveDoc,
    pub clipboard: ClipboardDoc,
    pub launcher: Vec<LauncherEntry>,
    pub clash: ClashDoc,
    pub brightness: BrightnessDoc,
    pub agenthud: AgentHudDoc,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct IslandDoc {
    /// top / bottom / left / right
    pub edge: String,
    /// 沿边位置 0..1
    pub pos: f64,
    /// 显示器索引（`island_monitors` 返回的稳定顺序）
    pub screen: i64,
    /// auto / light / dark / rouge / sky
    pub theme: String,
    /// 自定义字体 family；空 = 内置字体栈
    pub font_family: String,
    pub keep_on_top: bool,
    pub hug_fullscreen: bool,
    /// 悬停自动展开（false = 单击展开）；AgentHUD 暂未接线
    pub hover_expand: bool,
    pub hover_expand_ms: i64,
    pub collapse_delay_ms: i64,
    pub exclude_from_capture: bool,
    pub blur_fps: i64,
    /// hover / always / never
    pub expand_mode: String,
    pub passthrough: bool,
    pub hide_fullscreen: bool,
    /// rings / clock / pet / music
    pub compact_view: String,
    /// percent / rings
    pub quota_display: String,
    /// period / dynamic
    pub quota_ring_style: String,
    pub auto_cycle_s: i64,
    pub hidden_tabs: Vec<String>,
    /// AgentHUD 专属：玻璃透明程度 0..90
    pub transparency: i64,
    /// AgentHUD 专属：界面基准字号 10..18
    pub font_size: f64,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct HotkeysDoc {
    pub toggle: String,
    pub translate: String,
    pub clipboard: String,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct MediaDoc {
    pub auto_show: bool,
    pub lyrics: bool,
    pub netease_log: bool,
    pub playback_logs: Vec<String>,
    pub lyrics_sources: String,
    pub lyrics_custom_url: String,
    pub priority: Vec<String>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct NotifyDoc {
    pub modules: Vec<String>,
    pub body_preview: Vec<String>,
    pub popup: PopupDoc,
    pub open: OpenDoc,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct PopupDoc {
    pub enabled: bool,
    pub position: String,
    pub size: i64,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct OpenDoc {
    pub qq: String,
    pub wechat: String,
    pub mail: String,
    pub feishu: String,
    pub dingtalk: String,
    pub dsh: String,
    /// 用户手动保存过（**包括显式清空**）的 kind：启动自动探测只填空项，
    /// 这些 kind 不会再被自动填充。白名单 / trim / 去重见 `clamped`。
    #[serde(default)]
    pub open_manual: Vec<String>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct DshDoc {
    pub enabled: bool,
    /// 空 = `%USERPROFILE%\.dsh\sessions`
    pub sessions_dir: String,
    pub include_subagents: bool,
    pub skip_when_active: bool,
    pub body_chars: i64,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct MailDoc {
    pub interval_s: i64,
    pub imap: Vec<ImapAccount>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct ImapAccount {
    pub host: String,
    pub port: i64,
    pub user: String,
    pub password: String,
    pub enabled: bool,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct StocksDoc {
    pub watchlist: Vec<String>,
    pub interval: i64,
    pub pct_alert: f64,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct WeatherDoc {
    pub api_key: String,
    pub city: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub units: String,
    pub interval_min: i64,
    pub cities: Vec<WeatherCity>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct WeatherCity {
    pub name: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct TranslateDoc {
    pub custom_enabled: bool,
    pub platform: String,
    pub platform_model: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub import_from: String,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct LiveDoc {
    pub models_dir: String,
    pub audio: LiveAudioDoc,
    pub vad: LiveVadDoc,
    pub asr: LiveAsrDoc,
    pub translate: LiveTranslateDoc,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct LiveAudioDoc {
    /// loopback / mic
    pub source: String,
    pub device: String,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct LiveVadDoc {
    pub threshold: f64,
    pub min_silence_ms: i64,
    pub min_speech_ms: i64,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct LiveAsrDoc {
    pub partial: bool,
    pub draft_interval_ms: i64,
    pub final_threads: i64,
    pub max_segment_s: i64,
    pub context_prompt: bool,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct LiveTranslateDoc {
    pub enabled: bool,
    pub live: bool,
    pub live_min_interval_ms: i64,
    pub live_min_new_chars: i64,
    pub context_lines: i64,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct ClipboardDoc {
    pub max: i64,
    #[serde(flatten, default)]
    pub extra: Extra,
}

/// 启动器条目：保留 AgentHUD 的 `icon`（前端图标键），其余字段
/// （`args` 等）走 `extra` 原样保留。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct LauncherEntry {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct ClashDoc {
    pub config_dir: String,
    pub proxy_port: i64,
    pub app: String,
    /// 用户手动保存过 `app`（**包括显式清空**）：启动自动探测不再覆盖。
    #[serde(default)]
    pub app_manual: bool,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct BrightnessDoc {
    pub twinkle: String,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct AgentHudDoc {
    pub agent_order: Vec<String>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

// ============================================================
// 默认值
// ============================================================

fn default_launcher_entries() -> Vec<LauncherEntry> {
    super::default_launcher()
        .into_iter()
        .map(LauncherEntry::from)
        .collect()
}

impl From<LauncherItem> for LauncherEntry {
    fn from(item: LauncherItem) -> Self {
        Self {
            name: item.name,
            path: item.path,
            icon: if item.icon.trim().is_empty() {
                None
            } else {
                Some(item.icon)
            },
            extra: extra(),
        }
    }
}

impl Default for IslandDoc {
    fn default() -> Self {
        Self {
            edge: super::default_edge(),
            pos: super::default_pos_frac(),
            screen: 0,
            // 与 AgentHUD 现有默认一致
            theme: super::default_theme(),
            font_family: String::new(),
            keep_on_top: true,
            hug_fullscreen: true,
            hover_expand: true,
            hover_expand_ms: super::default_hover_expand_ms() as i64,
            collapse_delay_ms: super::default_collapse_delay_ms() as i64,
            exclude_from_capture: false,
            blur_fps: super::default_blur_fps() as i64,
            expand_mode: super::default_expand_mode(),
            passthrough: false,
            // 默认关闭全屏隐藏（旧配置显式写了 true 的仍然隐藏）
            hide_fullscreen: false,
            compact_view: super::default_compact_view(),
            quota_display: super::default_quota_display(),
            quota_ring_style: super::default_quota_ring_style(),
            auto_cycle_s: super::default_auto_cycle_sec() as i64,
            hidden_tabs: Vec::new(),
            transparency: super::default_transparency() as i64,
            font_size: super::default_font_size(),
            extra: extra(),
        }
    }
}

impl Default for HotkeysDoc {
    fn default() -> Self {
        Self {
            toggle: super::default_hotkey(),
            translate: "Ctrl+Alt+J".into(),
            clipboard: "Ctrl+Alt+V".into(),
            extra: extra(),
        }
    }
}

impl Default for MediaDoc {
    fn default() -> Self {
        Self {
            auto_show: true,
            lyrics: true,
            netease_log: true,
            playback_logs: Vec::new(),
            lyrics_sources: "netease,lrclib,custom".into(),
            lyrics_custom_url: String::new(),
            priority: ["cloudmusic", "bilibili", "chrome", "zen", "msedge", "firefox"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            extra: extra(),
        }
    }
}

impl Default for PopupDoc {
    fn default() -> Self {
        Self {
            enabled: true,
            position: "bottom_right".into(),
            size: 100,
            extra: extra(),
        }
    }
}

impl Default for OpenDoc {
    fn default() -> Self {
        Self {
            qq: String::new(),
            wechat: String::new(),
            mail: String::new(),
            feishu: String::new(),
            dingtalk: String::new(),
            dsh: String::new(),
            open_manual: Vec::new(),
            extra: extra(),
        }
    }
}

impl Default for NotifyDoc {
    fn default() -> Self {
        Self {
            modules: super::default_notify_modules(),
            body_preview: super::default_notify_body_preview(),
            popup: PopupDoc::default(),
            open: OpenDoc::default(),
            extra: extra(),
        }
    }
}

impl Default for DshDoc {
    fn default() -> Self {
        Self {
            enabled: true,
            sessions_dir: String::new(),
            include_subagents: false,
            skip_when_active: true,
            body_chars: 120,
            extra: extra(),
        }
    }
}

impl Default for ImapAccount {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 993,
            user: String::new(),
            password: String::new(),
            enabled: true,
            extra: extra(),
        }
    }
}

impl Default for MailDoc {
    fn default() -> Self {
        Self {
            interval_s: 120,
            imap: Vec::new(),
            extra: extra(),
        }
    }
}

impl Default for StocksDoc {
    fn default() -> Self {
        Self {
            watchlist: super::default_stocks(),
            interval: super::default_stock_interval() as i64,
            pct_alert: super::default_stock_alert(),
            extra: extra(),
        }
    }
}

impl Default for WeatherCity {
    fn default() -> Self {
        Self {
            name: String::new(),
            lat: None,
            lon: None,
            extra: extra(),
        }
    }
}

impl Default for WeatherDoc {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            // 空 = 跟随公网 IP 定位；绑定城市后由天气页 / 设置页写入。
            city: String::new(),
            lat: None,
            lon: None,
            units: "metric".into(),
            interval_min: 15,
            cities: Vec::new(),
            extra: extra(),
        }
    }
}

impl Default for TranslateDoc {
    fn default() -> Self {
        Self {
            custom_enabled: false,
            platform: String::new(),
            platform_model: String::new(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            import_from: String::new(),
            extra: extra(),
        }
    }
}

impl Default for LiveAudioDoc {
    fn default() -> Self {
        Self {
            source: "loopback".into(),
            device: String::new(),
            extra: extra(),
        }
    }
}

impl Default for LiveVadDoc {
    fn default() -> Self {
        Self {
            threshold: 0.4,
            min_silence_ms: 350,
            min_speech_ms: 200,
            extra: extra(),
        }
    }
}

impl Default for LiveAsrDoc {
    fn default() -> Self {
        Self {
            partial: true,
            draft_interval_ms: 500,
            final_threads: 6,
            max_segment_s: 10,
            context_prompt: false,
            extra: extra(),
        }
    }
}

impl Default for LiveTranslateDoc {
    fn default() -> Self {
        Self {
            enabled: true,
            live: true,
            live_min_interval_ms: 900,
            live_min_new_chars: 4,
            context_lines: 3,
            extra: extra(),
        }
    }
}

impl Default for LiveDoc {
    fn default() -> Self {
        Self {
            models_dir: "models".into(),
            audio: LiveAudioDoc::default(),
            vad: LiveVadDoc::default(),
            asr: LiveAsrDoc::default(),
            translate: LiveTranslateDoc::default(),
            extra: extra(),
        }
    }
}

impl Default for ClipboardDoc {
    fn default() -> Self {
        Self {
            max: 10,
            extra: extra(),
        }
    }
}

impl Default for ClashDoc {
    fn default() -> Self {
        Self {
            config_dir: "%APPDATA%/io.github.clash-verge-rev.clash-verge-rev".into(),
            proxy_port: 7897,
            app: String::new(),
            app_manual: false,
            extra: extra(),
        }
    }
}

impl Default for BrightnessDoc {
    fn default() -> Self {
        Self {
            twinkle: String::new(),
            extra: extra(),
        }
    }
}

impl Default for AgentHudDoc {
    fn default() -> Self {
        Self {
            agent_order: Vec::new(),
            extra: extra(),
        }
    }
}

impl Default for LauncherEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: String::new(),
            icon: None,
            extra: extra(),
        }
    }
}

impl Default for SettingsDoc {
    fn default() -> Self {
        Self {
            island: IslandDoc::default(),
            hotkeys: HotkeysDoc::default(),
            media: MediaDoc::default(),
            notify: NotifyDoc::default(),
            dsh: DshDoc::default(),
            mail: MailDoc::default(),
            stocks: StocksDoc::default(),
            weather: WeatherDoc::default(),
            translate: TranslateDoc::default(),
            live: LiveDoc::default(),
            clipboard: ClipboardDoc::default(),
            launcher: default_launcher_entries(),
            clash: ClashDoc::default(),
            brightness: BrightnessDoc::default(),
            agenthud: AgentHudDoc::default(),
            extra: extra(),
        }
    }
}

// ============================================================
// 归一化（clamp）
// ============================================================

impl IslandDoc {
    fn clamped(mut self) -> Self {
        self.edge = one_of(&self.edge, &EDGES, "top");
        self.pos = clamp_float(self.pos, 0.0, 1.0, 0.5);
        self.screen = clamp_int(self.screen, 0, 63);
        self.theme = one_of(&self.theme, &THEMES, "dark");
        self.font_family = trimmed(&self.font_family);
        self.hover_expand_ms = clamp_int(self.hover_expand_ms, 0, 1000);
        self.collapse_delay_ms = clamp_int(self.collapse_delay_ms, 0, 2000);
        self.blur_fps = clamp_int(self.blur_fps, 2, 30);
        self.expand_mode = one_of(&self.expand_mode, &EXPAND_MODES, "hover");
        self.compact_view = one_of(&self.compact_view, &COMPACT_VIEWS, "rings");
        self.quota_display = super::normalize_quota_display(&self.quota_display);
        self.quota_ring_style = super::normalize_quota_ring_style(&self.quota_ring_style);
        self.auto_cycle_s = clamp_int(self.auto_cycle_s, 0, 600);
        self.hidden_tabs = normalize_hidden_tabs(&self.hidden_tabs);
        self.transparency = clamp_int(self.transparency, 0, 90);
        self.font_size = clamp_float(self.font_size, 10.0, 18.0, 13.0);
        self
    }
}

impl HotkeysDoc {
    fn clamped(mut self) -> Self {
        self.toggle = trimmed(&self.toggle);
        self.translate = trimmed(&self.translate);
        self.clipboard = trimmed(&self.clipboard);
        self
    }
}

impl MediaDoc {
    fn clamped(mut self) -> Self {
        // 与 IslandConfig::clamped 同口径：源顺序保留、自定义接口限长、优先级去重。
        self.lyrics_sources = super::normalize_lyrics_sources(&self.lyrics_sources);
        self.lyrics_custom_url = trimmed(&self.lyrics_custom_url)
            .chars()
            .take(2048)
            .collect();
        self.priority = super::normalize_media_priority(&self.priority);
        self.playback_logs = super::normalize_media_playback_logs(&self.playback_logs);
        self
    }
}

impl PopupDoc {
    fn clamped(mut self) -> Self {
        self.position = one_of(&self.position, &POPUP_POSITIONS, "bottom_right");
        self.size = clamp_int(self.size, 70, 160);
        self
    }
}

impl OpenDoc {
    fn clamped(mut self) -> Self {
        self.qq = trimmed(&self.qq);
        self.wechat = trimmed(&self.wechat);
        self.mail = trimmed(&self.mail);
        self.feishu = trimmed(&self.feishu);
        self.dingtalk = trimmed(&self.dingtalk);
        self.dsh = trimmed(&self.dsh);
        // 手动标记只留 `NOTIFY_KINDS` 白名单：trim + 小写 + 去重（顺序按
        // NOTIFY_KINDS 固定，与消息模块开关同口径）。
        self.open_manual = super::normalize_notify_kinds(&self.open_manual);
        self
    }
}

/// 文档 `notify.open` → `IslandConfig.notify_open`（只收非空值，key 固定白名单）。
pub(crate) fn open_map(open: &OpenDoc) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for (kind, value) in [
        ("qq", &open.qq),
        ("wechat", &open.wechat),
        ("mail", &open.mail),
        ("feishu", &open.feishu),
        ("dingtalk", &open.dingtalk),
        ("dsh", &open.dsh),
    ] {
        let value = value.trim();
        if !value.is_empty() {
            map.insert(kind.to_string(), value.to_string());
        }
    }
    super::normalize_notify_open(map)
}

/// `IslandConfig.notify_open` → 文档 `notify.open`（缺失的 kind 清空）。
pub(crate) fn apply_open_map(
    open: &mut OpenDoc,
    map: &std::collections::BTreeMap<String, String>,
) {
    let value = |kind: &str| map.get(kind).cloned().unwrap_or_default();
    open.qq = value("qq");
    open.wechat = value("wechat");
    open.mail = value("mail");
    open.feishu = value("feishu");
    open.dingtalk = value("dingtalk");
    open.dsh = value("dsh");
}

impl NotifyDoc {
    fn clamped(mut self) -> Self {
        self.modules = super::normalize_notify_kinds(&self.modules);
        self.body_preview = super::normalize_notify_body_preview(&self.body_preview);
        self.popup = self.popup.clamped();
        self.open = self.open.clamped();
        self
    }
}

impl DshDoc {
    fn clamped(mut self) -> Self {
        self.sessions_dir = trimmed(&self.sessions_dir);
        self.body_chars = clamp_int(self.body_chars, 40, 240);
        self
    }
}

impl ImapAccount {
    fn clamped(mut self) -> Option<Self> {
        self.host = trimmed(&self.host);
        self.user = trimmed(&self.user);
        self.password = trimmed(&self.password);
        if self.host.is_empty() || self.user.is_empty() {
            return None;
        }
        self.port = clamp_int(self.port, 1, 65535);
        Some(self)
    }
}

impl MailDoc {
    fn clamped(mut self) -> Self {
        self.interval_s = clamp_int(self.interval_s, 30, 3600);
        self.imap = self
            .imap
            .into_iter()
            .filter_map(ImapAccount::clamped)
            .take(16)
            .collect();
        self
    }
}

impl StocksDoc {
    fn clamped(mut self) -> Self {
        // 与 IslandConfig::clamped 完全同口径：规范代码、白名单、去重。
        let mut seen = std::collections::HashSet::new();
        self.watchlist = self
            .watchlist
            .iter()
            .map(|code| super::stocks::norm_code(code))
            .filter(|code| super::stocks::valid_code(code) && seen.insert(code.clone()))
            .collect();
        self.interval = clamp_int(self.interval, 2, 60);
        self.pct_alert = clamp_float(self.pct_alert, 0.0, 20.0, 3.0);
        self
    }
}

impl WeatherCity {
    fn clamped(mut self) -> Option<Self> {
        self.name = trimmed(&self.name);
        if self.name.is_empty() {
            return None;
        }
        let (lat, lon) = normalize_coord_pair(self.lat, self.lon);
        self.lat = lat;
        self.lon = lon;
        Some(self)
    }
}

impl WeatherDoc {
    fn clamped(mut self) -> Self {
        self.api_key = trimmed(&self.api_key);
        self.city = trimmed(&self.city);
        let (lat, lon) = normalize_coord_pair(self.lat, self.lon);
        self.lat = lat;
        self.lon = lon;
        // 主城市为空 = 跟随 IP，坐标不留在文档里。
        if self.city.is_empty() {
            self.lat = None;
            self.lon = None;
        }
        self.units = one_of(&self.units, &TEMPERATURE_UNITS, "metric");
        self.interval_min = clamp_int(self.interval_min, 5, 180);
        // 与 IslandConfig 同口径：name + 坐标去重，最多 20 条。
        let mut seen = std::collections::HashSet::new();
        self.cities = self
            .cities
            .into_iter()
            .filter_map(WeatherCity::clamped)
            .filter(|city| {
                seen.insert((
                    city.name.to_lowercase(),
                    city.lat.map(f64::to_bits),
                    city.lon.map(f64::to_bits),
                ))
            })
            .take(20)
            .collect();
        self
    }

    /// 旧默认迁移（只在读盘 / 导入路径执行）：老版本默认主城市「上海」且
    /// 没有坐标时视为未绑定，升级后跟随 IP。
    ///
    /// 不能放进 `clamped()`：`patched_doc` 对每次 patch 都会 clamp，用户显式
    /// 绑定「上海」（经纬度留空）时会被这条规则误清空。
    fn migrate_legacy_default(mut self) -> Self {
        if self.city == "上海" && self.lat.is_none() && self.lon.is_none() {
            self.city.clear();
            self.lat = None;
            self.lon = None;
        }
        self
    }
}

impl TranslateDoc {
    fn clamped(mut self) -> Self {
        self.platform = trimmed(&self.platform);
        self.platform_model = trimmed(&self.platform_model);
        self.api_key = trimmed(&self.api_key);
        self.base_url = trimmed(&self.base_url);
        self.model = trimmed(&self.model);
        self.import_from = trimmed(&self.import_from);
        self
    }
}

impl LiveAudioDoc {
    fn clamped(mut self) -> Self {
        self.source = one_of(&self.source, &AUDIO_SOURCES, "loopback");
        self.device = trimmed(&self.device);
        self
    }
}

impl LiveVadDoc {
    fn clamped(mut self) -> Self {
        self.threshold = clamp_float(self.threshold, 0.1, 0.9, 0.4);
        self.min_silence_ms = clamp_int(self.min_silence_ms, 100, 1500);
        self.min_speech_ms = clamp_int(self.min_speech_ms, 100, 1000);
        self
    }
}

impl LiveAsrDoc {
    fn clamped(mut self) -> Self {
        self.draft_interval_ms = clamp_int(self.draft_interval_ms, 200, 2000);
        self.final_threads = clamp_int(self.final_threads, 1, 16);
        self.max_segment_s = clamp_int(self.max_segment_s, 3, 30);
        self
    }
}

impl LiveTranslateDoc {
    fn clamped(mut self) -> Self {
        self.live_min_interval_ms = clamp_int(self.live_min_interval_ms, 300, 3000);
        self.live_min_new_chars = clamp_int(self.live_min_new_chars, 1, 20);
        self.context_lines = clamp_int(self.context_lines, 0, 3);
        self
    }
}

impl LiveDoc {
    fn clamped(mut self) -> Self {
        self.models_dir = trimmed(&self.models_dir);
        self.audio = self.audio.clamped();
        self.vad = self.vad.clamped();
        self.asr = self.asr.clamped();
        self.translate = self.translate.clamped();
        self
    }
}

impl ClipboardDoc {
    fn clamped(mut self) -> Self {
        self.max = clamp_int(self.max, 1, 100);
        self
    }
}

impl LauncherEntry {
    fn clamped(mut self) -> Option<Self> {
        self.name = trimmed(&self.name);
        self.path = trimmed(&self.path);
        if self.name.is_empty() || self.path.is_empty() {
            return None;
        }
        self.icon = self
            .icon
            .map(|icon| trimmed(&icon))
            .filter(|icon| !icon.is_empty());
        Some(self)
    }
}

impl ClashDoc {
    fn clamped(mut self) -> Self {
        self.config_dir = trimmed(&self.config_dir);
        self.proxy_port = clamp_int(self.proxy_port, 1, 65535);
        self.app = trimmed(&self.app);
        self
    }
}

impl BrightnessDoc {
    fn clamped(mut self) -> Self {
        self.twinkle = trimmed(&self.twinkle);
        self
    }
}

impl AgentHudDoc {
    fn clamped(mut self) -> Self {
        self.agent_order = normalize_list(&self.agent_order, 64, false);
        self
    }
}

impl SettingsDoc {
    /// 枚举白名单、数值夹取、数组去重 / 白名单、字符串 trim。
    /// 取值范围与设置窗表单一致（AgentHUD 专属项另注）。
    pub fn clamped(mut self) -> Self {
        self.island = self.island.clamped();
        self.hotkeys = self.hotkeys.clamped();
        self.media = self.media.clamped();
        self.notify = self.notify.clamped();
        self.dsh = self.dsh.clamped();
        self.mail = self.mail.clamped();
        self.stocks = self.stocks.clamped();
        self.weather = self.weather.clamped();
        self.translate = self.translate.clamped();
        self.live = self.live.clamped();
        self.clipboard = self.clipboard.clamped();
        self.launcher = self
            .launcher
            .into_iter()
            .filter_map(LauncherEntry::clamped)
            .take(64)
            .collect();
        self.clash = self.clash.clamped();
        self.brightness = self.brightness.clamped();
        self.agenthud = self.agenthud.clamped();
        self
    }
}

// ============================================================
// 文件读写（tmp + rename）
// ============================================================

/// `~/.agenthud/settings.json`
pub fn doc_path() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join(SETTINGS_FILE))
}

pub(super) fn saved_hover_expand() -> Option<bool> {
    let doc = read_doc_file(&doc_path().ok()?).ok()?;
    Some(doc.island.hover_expand)
}

fn merge_json(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(base_map), Value::Object(over_map)) => {
            for (key, value) in over_map {
                match base_map.get_mut(&key) {
                    Some(slot) => merge_json(slot, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

/// 默认值 + 磁盘 JSON 的深合并（对象递归合并，数组整体替换）。
fn merge_defaults(raw: Value) -> Value {
    let mut base = serde_json::to_value(SettingsDoc::default())
        .unwrap_or_else(|_| Value::Object(Map::new()));
    merge_json(&mut base, raw);
    base
}

fn from_value(raw: Value) -> Result<SettingsDoc, String> {
    let merged = merge_defaults(raw);
    let doc: SettingsDoc = serde_json::from_value(merged)
        .map_err(|e| format!("设置文档结构不合法: {e}"))?;
    let mut doc = doc.clamped();
    // 旧默认迁移只在读盘 / 导入路径执行，见 WeatherDoc::migrate_legacy_default。
    doc.weather = doc.weather.migrate_legacy_default();
    Ok(doc)
}

fn read_doc_file(path: &Path) -> Result<SettingsDoc, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let raw: Value = serde_json::from_str(&text)
        .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
    if !raw.is_object() {
        return Err(format!(
            "{} 的顶层必须是 JSON 对象（当前是 {}）",
            path.display(),
            json_type_name(&raw)
        ));
    }
    from_value(raw)
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "布尔值",
        Value::Number(_) => "数字",
        Value::String(_) => "字符串",
        Value::Array(_) => "数组",
        Value::Object(_) => "对象",
    }
}

fn save_to_path(path: &Path, doc: &SettingsDoc) -> Result<(), String> {
    let doc = doc.clone().clamped();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建配置目录 {} 失败: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&doc)
        .map_err(|e| format!("序列化设置文档失败: {e}"))?;
    // 每次用唯一临时文件：两个保存并发时不会互相写坏对方的 tmp，最后由
    // rename 的原子性决定谁生效（last-writer-wins）。
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "json.tmp{}",
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, format!("{text}\n"))
        .map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入 {} 失败: {e}", path.display()));
    }
    Ok(())
}

/// 从指定路径读文档；文件不存在时用 `seed`（岛运行时配置）+ `monitors`
/// （显示器稳定顺序）生成并落盘。
fn load_from_path(
    path: &Path,
    seed: &IslandConfig,
    monitors: &[MonitorEntry],
) -> Result<SettingsDoc, String> {
    match read_doc_file(path) {
        Ok(doc) => Ok(doc),
        Err(_) if !path.exists() => {
            // 首次运行：defaults + 现有 IslandConfig 重叠字段 → 落盘
            let doc = seeded_doc(seed, monitors);
            save_to_path(path, &doc)?;
            Ok(doc)
        }
        Err(message) => Err(message),
    }
}

/// 首次生成用的种子文档：默认值叠加岛运行时已生效的重叠字段。
fn seeded_doc(seed: &IslandConfig, monitors: &[MonitorEntry]) -> SettingsDoc {
    let mut doc = SettingsDoc::default();
    overlay_from_island(&mut doc, seed, monitors);
    doc.clamped()
}

fn load_doc(seed: &IslandConfig, monitors: &[MonitorEntry]) -> SettingsDoc {
    let path = match doc_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("[zbar-island-settings] {e}");
            return seeded_doc(seed, monitors);
        }
    };
    match load_from_path(&path, seed, monitors) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("[zbar-island-settings] {e}");
            seeded_doc(seed, monitors)
        }
    }
}

/// 读设置文档（不存在则生成）。任何 IO / 解析失败都回退到「defaults +
/// 岛实时值」，只记日志，不覆盖磁盘上的坏文件，避免一次手滑清空设置。
pub fn load() -> SettingsDoc {
    load_doc(&super::load_config(), &[])
}

/// 命令侧读取：首次生成时能把 `monitor` 设备名映射成显示器索引。
fn load_for(app: &tauri::AppHandle) -> SettingsDoc {
    load_doc(&super::load_config(), &monitor_entries(app))
}

/// 整份原子落盘（先 clamp）。settings 路径之外的调用方（导入 / 恢复默认）
/// 也走这里。
pub fn save_atomic(doc: &SettingsDoc) -> Result<(), String> {
    save_to_path(&doc_path()?, doc)
}

/// 启动挂点：首次运行生成 `settings.json`，并把文档里的剪贴板条数上限
/// 应用到内存态（该值不在 `IslandConfig` 里，只能单独补一次）。
pub(crate) fn ensure_document(app: &tauri::AppHandle) {
    let mut doc = load_for(app);
    let imported = "thunderbird_imported";
    if doc.mail.imap.is_empty()
        && doc.mail.extra.get(imported).and_then(Value::as_bool) != Some(true)
        && doc_path().is_ok_and(|path| read_doc_file(&path).is_ok())
    {
        let accounts = super::notify::mail::discover_thunderbird_accounts();
        if !accounts.is_empty() {
            doc.mail.imap = accounts;
            doc.mail.extra.insert(imported.into(), Value::Bool(true));
            if let Err(e) = save_atomic(&doc) {
                eprintln!("[zbar-island-settings] 导入 Thunderbird 账户失败: {e}");
            }
        }
    }
    let want = doc.clipboard.max.clamp(1, 100) as u32;
    if super::clipboard::island_clipboard_limit() != want {
        if let Err(e) = super::clipboard::island_clipboard_set_limit(want) {
            eprintln!("[zbar-island-settings] 应用剪贴板条数上限失败: {e}");
        }
    }
}

// ============================================================
// 启动自动探测「打开应用 / Clash Verge 程序 / Twinkle」路径
// ============================================================

/// `kind` → 「打开应用」六字段的可写槽；未知 kind 返回 None。
fn open_slot<'a>(open: &'a mut OpenDoc, kind: &str) -> Option<&'a mut String> {
    Some(match kind {
        "qq" => &mut open.qq,
        "wechat" => &mut open.wechat,
        "mail" => &mut open.mail,
        "feishu" => &mut open.feishu,
        "dingtalk" => &mut open.dingtalk,
        "dsh" => &mut open.dsh,
        _ => return None,
    })
}

/// 启动探测的纯逻辑：`detect_open(kind)` / `detect_app()` / `detect_twinkle()`
/// 给候选路径，只填「值为空且没被用户手动保存过」的项，返回是否有改动。
///
/// - 已有值不覆盖（用户填过的、上次探测填过的都算）；
/// - `open_manual` / `app_manual` 命中时跳过——用户显式清空也算「设置过」；
/// - `brightness.twinkle` 没有 manual 字段，只有「空才填」；
/// - 探测结果 trim 后为空也跳过（不写空值）。
fn fill_detected_paths(
    doc: &mut SettingsDoc,
    detect_open: impl Fn(&str) -> Option<String>,
    detect_app: impl Fn() -> Option<String>,
    detect_twinkle: impl Fn() -> Option<String>,
) -> bool {
    let manual = doc.notify.open.open_manual.clone();
    let mut changed = false;
    for kind in super::NOTIFY_KINDS {
        if manual.iter().any(|item| item == kind) {
            continue;
        }
        let Some(slot) = open_slot(&mut doc.notify.open, kind) else {
            continue;
        };
        if !slot.trim().is_empty() {
            continue;
        }
        let Some(path) = detect_open(kind) else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        *slot = path.to_string();
        changed = true;
    }
    if !doc.clash.app_manual && doc.clash.app.trim().is_empty() {
        if let Some(path) = detect_app() {
            let path = path.trim();
            if !path.is_empty() {
                doc.clash.app = path.to_string();
                changed = true;
            }
        }
    }
    // Twinkle 便携版路径只有「空才填」：非空一律不覆盖用户填的值。
    if doc.brightness.twinkle.trim().is_empty() {
        if let Some(path) = detect_twinkle() {
            let path = path.trim();
            if !path.is_empty() {
                doc.brightness.twinkle = path.to_string();
                changed = true;
            }
        }
    }
    changed
}

/// Twinkle（亮度）程序自动探测：运行中进程 → 卸载表 → 常见安装目录。
///
/// `app_paths::detect_exe` 的常见目录规则是「最后一段必须是 exe 文件名」，
/// 所以这里每个候选目录都带上文件名尾巴（只给目录永远匹配不上）。
fn detect_twinkle_path() -> Option<String> {
    super::app_paths::detect_exe(
        &["Twinkle Tray.exe", "TwinkleTray.exe", "Twinkle.exe"],
        &["Twinkle Tray", "Twinkle"],
        &[
            (
                "LOCALAPPDATA",
                &["Programs", "twinkle-tray", "Twinkle Tray.exe"],
            ),
            ("ProgramFiles", &["Twinkle Tray", "Twinkle Tray.exe"]),
            ("ProgramFiles(x86)", &["Twinkle Tray", "Twinkle Tray.exe"]),
        ],
    )
    .map(|path| path.display().to_string())
}

/// 启动探测的完整流程：先把岛实时值叠加进文档，再填空项。
///
/// 顺序不能颠倒：岛内右键菜单 / 拖动只写 `IslandConfig`、不写 settings.json，
/// 磁盘文档可能落后；少了 overlay，后面的 `persist_and_sync → sync_to_island`
/// 会把文档里的旧 edge/pos/monitor/theme/transparency/hidden_tabs/
/// notify_modules/hotkeys/stocks/weather 写回岛配置，等于「探测到新路径」
/// 顺手把用户刚在岛内改的配置静默回退（`apply_patch` 也是先 overlay 再改）。
/// 抽成独立函数只为了单测能锁住这个顺序。
fn seed_with_overlay(
    doc: &mut SettingsDoc,
    cfg: &IslandConfig,
    monitors: &[MonitorEntry],
    detect_open: impl Fn(&str) -> Option<String>,
    detect_app: impl Fn() -> Option<String>,
    detect_twinkle: impl Fn() -> Option<String>,
) -> bool {
    overlay_from_island(doc, cfg, monitors);
    fill_detected_paths(doc, detect_open, detect_app, detect_twinkle)
}

/// 启动挂点：把本机已安装程序的路径填进 `settings.json` 里从没设置过的
/// 空项（`notify.open.*` / `clash.app` / `brightness.twinkle`），再同步岛运行时。
///
/// 每次应用启动都会跑一遍（含开机自启——两者走同一条 setup 路径）：用户
/// 手动保存过的项（`open_manual` / `app_manual`，包括显式清空）不覆盖；
/// 本次没有新填入时不动磁盘。探测 / 落盘失败只记日志，不影响启动，也
/// 不会设置手动标记。
pub(crate) async fn seed_detected_paths(app: &tauri::AppHandle) {
    let mut doc = load_for(app);
    let changed = seed_with_overlay(
        &mut doc,
        &super::load_config(),
        &monitor_entries(app),
        super::notify::detect_open_path,
        super::clash::detect_app_path,
        detect_twinkle_path,
    );
    if !changed {
        return;
    }
    if let Err(e) = persist_and_sync(app, doc).await {
        eprintln!("[zbar-island-settings] 自动探测程序路径失败: {e}");
    }
}

// ============================================================
// 双向同步
// ============================================================

/// `IslandConfig` → `SettingsDoc`（只覆盖重叠字段，未知额外字段不动）。
///
/// 设置页每次读盘前都会先做一次：UI 显示的是岛实时值（比如拖动吸附后
/// 的 edge/pos、岛内改过的透明度），而不是磁盘上可能落后的文档。
pub fn overlay_from_island(
    doc: &mut SettingsDoc,
    cfg: &IslandConfig,
    monitors: &[MonitorEntry],
) {
    doc.island.edge = cfg.edge.clone();
    doc.island.pos = cfg.pos_frac;
    doc.island.screen = screen_for_monitor(monitors, cfg.monitor.as_deref());
    doc.island.theme = cfg.theme.clone();
    doc.island.font_family = cfg.font_family.clone();
    doc.island.keep_on_top = cfg.keep_on_top;
    doc.island.hug_fullscreen = cfg.hug_fullscreen;
    doc.island.hover_expand = cfg.hover_expand;
    doc.island.hover_expand_ms = cfg.hover_expand_ms as i64;
    doc.island.collapse_delay_ms = cfg.collapse_delay_ms as i64;
    doc.island.exclude_from_capture = cfg.exclude_from_capture;
    doc.island.blur_fps = cfg.blur_fps as i64;
    doc.island.expand_mode = cfg.expand_mode.clone();
    doc.island.passthrough = cfg.passthrough;
    doc.island.hide_fullscreen = cfg.hide_fullscreen;
    doc.island.compact_view = cfg.compact_view.clone();
    doc.island.quota_display = cfg.quota_display.clone();
    doc.island.quota_ring_style = cfg.quota_ring_style.clone();
    doc.island.auto_cycle_s = cfg.auto_cycle_sec as i64;
    doc.island.hidden_tabs = cfg.hidden_tabs.clone();
    doc.island.transparency = cfg.transparency as i64;
    doc.island.font_size = cfg.font_size;
    doc.hotkeys.toggle = cfg.hotkey_toggle.clone();
    doc.hotkeys.translate = cfg.hotkey_translate.clone();
    doc.hotkeys.clipboard = cfg.hotkey_clipboard.clone();
    doc.media.auto_show = cfg.media_auto_show;
    doc.media.lyrics = cfg.media_lyrics;
    doc.media.netease_log = cfg.media_netease_log;
    doc.media.playback_logs = cfg.media_playback_logs.clone();
    doc.media.lyrics_sources = cfg.media_lyrics_sources.clone();
    doc.media.lyrics_custom_url = cfg.media_lyrics_custom_url.clone();
    doc.media.priority = cfg.media_priority.clone();
    doc.notify.modules = cfg.notify_modules.clone();
    doc.notify.body_preview = cfg.notify_body_preview.clone();
    doc.notify.popup.enabled = cfg.notify_popup_enabled;
    doc.notify.popup.position = cfg.notify_popup_position.clone();
    doc.notify.popup.size = cfg.notify_popup_size.clamp(70, 160);
    apply_open_map(&mut doc.notify.open, &cfg.notify_open);
    doc.stocks.watchlist = cfg.stocks.clone();
    doc.stocks.interval = cfg.stock_interval_sec as i64;
    doc.stocks.pct_alert = cfg.stock_alert_pct;
    doc.weather.city = cfg.weather_city.clone();
    doc.weather.lat = cfg.weather_lat;
    doc.weather.lon = cfg.weather_lon;
    doc.weather.cities =
        merge_weather_cities(std::mem::take(&mut doc.weather.cities), &cfg.weather_cities);
    doc.launcher = merge_launcher(std::mem::take(&mut doc.launcher), &cfg.launcher);
    doc.agenthud.agent_order = cfg.agent_order.clone();
}

/// 岛启动器 → 文档启动器：名字/路径没变时保留原条目的 `extra`（args 等），
/// 只刷新 icon，避免「设置里改一下别的项就把启动器参数吃掉」。
fn merge_launcher(existing: Vec<LauncherEntry>, items: &[LauncherItem]) -> Vec<LauncherEntry> {
    let mut existing: Vec<_> = existing.into_iter().map(Some).collect();
    items
        .iter()
        .map(|item| {
            let matched = existing
                .iter_mut()
                .find(|entry| {
                    entry
                        .as_ref()
                        .is_some_and(|prev| prev.name == item.name && prev.path == item.path)
                })
                .and_then(Option::take);
            match matched {
                Some(mut next) => {
                    if !item.icon.trim().is_empty() {
                        next.icon = Some(item.icon.clone());
                    }
                    next
                }
                None => LauncherEntry::from(item.clone()),
            }
        })
        .collect()
}

/// 岛自定义城市 → 文档城市：同名（忽略首尾空白与大小写）的旧条目保留
/// `extra`（country 等），只刷新名字与坐标。
fn merge_weather_cities(existing: Vec<WeatherCity>, items: &[WeatherCityCfg]) -> Vec<WeatherCity> {
    let mut existing: Vec<_> = existing.into_iter().map(Some).collect();
    items
        .iter()
        .map(|city| {
            let matched = existing
                .iter_mut()
                .find(|entry| {
                    entry
                        .as_ref()
                        .is_some_and(|prev| prev.name.trim().eq_ignore_ascii_case(city.name.trim()))
                })
                .and_then(Option::take);
            match matched {
                Some(mut next) => {
                    next.name = city.name.clone();
                    next.lat = city.lat;
                    next.lon = city.lon;
                    next
                }
                None => WeatherCity {
                    name: city.name.clone(),
                    lat: city.lat,
                    lon: city.lon,
                    extra: extra(),
                },
            }
        })
        .collect()
}

/// 文档城市 → 岛自定义城市（`extra` 留在文档里，不进运行态）。
fn weather_cities_from_doc(cities: &[WeatherCity]) -> Vec<WeatherCityCfg> {
    cities
        .iter()
        .map(|city| WeatherCityCfg {
            name: city.name.clone(),
            lat: city.lat,
            lon: city.lon,
        })
        .collect()
}

/// 文档天气主城市 → 运行态。
///
/// 设置页「默认城市」只 patch `weather.city`，坐标仍是从运行态 overlay 进来的
/// 旧值；此时若直接沿用会把「北京」按上海坐标拉天气。所以城市名变了、而坐标
/// 与运行态完全相同时，把坐标清成 `None`，交给天气服务按新城市名 geocode；
/// 绑定命令同时带新坐标时（坐标也变了）保留新坐标。
fn apply_weather_main_city(next: &mut IslandConfig, prev: &IslandConfig, doc: &WeatherDoc) {
    next.weather_city = doc.city.clone();
    next.weather_lat = doc.lat;
    next.weather_lon = doc.lon;
    if next.weather_city != prev.weather_city
        && next.weather_lat == prev.weather_lat
        && next.weather_lon == prev.weather_lon
    {
        next.weather_lat = None;
        next.weather_lon = None;
    }
}

/// `SettingsDoc` → `IslandConfig`：写回重叠字段并经 `apply_config` 落盘 +
/// 广播 + 触发相关副作用（只有真正变化的项才刷新 / 重注册）。
pub(crate) async fn sync_to_island(
    app: &tauri::AppHandle,
    doc: &SettingsDoc,
) -> Result<IslandConfig, String> {
    let prev = super::load_config();
    let mut next = prev.clone();

    next.edge = doc.island.edge.clone();
    next.pos_frac = doc.island.pos;
    next.monitor = monitor_for_screen(app, doc.island.screen);
    next.theme = doc.island.theme.clone();
    next.font_family = doc.island.font_family.clone();
    next.font_size = doc.island.font_size;
    next.transparency = doc.island.transparency as u8;
    next.expand_mode = doc.island.expand_mode.clone();
    next.compact_view = doc.island.compact_view.clone();
    next.quota_display = doc.island.quota_display.clone();
    next.quota_ring_style = doc.island.quota_ring_style.clone();
    next.auto_cycle_sec = doc.island.auto_cycle_s as u64;
    next.hover_expand = doc.island.hover_expand;
    next.hover_expand_ms = doc.island.hover_expand_ms as u64;
    next.collapse_delay_ms = doc.island.collapse_delay_ms as u64;
    next.blur_fps = doc.island.blur_fps as u64;
    next.passthrough = doc.island.passthrough;
    next.hide_fullscreen = doc.island.hide_fullscreen;
    next.hidden_tabs = doc.island.hidden_tabs.clone();
    next.keep_on_top = doc.island.keep_on_top;
    next.hug_fullscreen = doc.island.hug_fullscreen;
    next.exclude_from_capture = doc.island.exclude_from_capture;
    next.hotkey_toggle = doc.hotkeys.toggle.clone();
    next.hotkey_translate = doc.hotkeys.translate.clone();
    next.hotkey_clipboard = doc.hotkeys.clipboard.clone();
    next.media_auto_show = doc.media.auto_show;
    next.media_lyrics = doc.media.lyrics;
    next.media_netease_log = doc.media.netease_log;
    next.media_playback_logs = doc.media.playback_logs.clone();
    next.media_lyrics_sources = doc.media.lyrics_sources.clone();
    next.media_lyrics_custom_url = doc.media.lyrics_custom_url.clone();
    next.media_priority = doc.media.priority.clone();
    next.notify_modules = doc.notify.modules.clone();
    next.notify_body_preview = doc.notify.body_preview.clone();
    next.notify_popup_enabled = doc.notify.popup.enabled;
    next.notify_popup_position = doc.notify.popup.position.clone();
    next.notify_popup_size = doc.notify.popup.size.clamp(70, 160);
    next.notify_open = open_map(&doc.notify.open);
    next.stocks = doc.stocks.watchlist.clone();
    next.stock_interval_sec = doc.stocks.interval as u64;
    next.stock_alert_pct = doc.stocks.pct_alert;
    next.weather_cities = weather_cities_from_doc(&doc.weather.cities);
    apply_weather_main_city(&mut next, &prev, &doc.weather);
    next.launcher = doc
        .launcher
        .iter()
        .enumerate()
        .map(|(index, item)| LauncherItem {
            name: item.name.clone(),
            path: item.path.clone(),
            // 文档没写 icon（前端只提交 name/path）时沿用岛端同位置同名的
            // 图标键，避免一次保存把所有启动器图标打回默认。
            icon: item
                .icon
                .clone()
                .or_else(|| {
                    prev.launcher.get(index).and_then(|old| {
                        (old.name == item.name && old.path == item.path)
                            .then(|| old.icon.clone())
                    })
                })
                .unwrap_or_default(),
        })
        .collect();
    next.agent_order = doc.agenthud.agent_order.clone();

    // 城市/坐标变了：旧的三级联动展示名会指错地方，用城市名回填展示名。
    // 这样用户显式绑定「上海」时 label 非空，不会被 mod.rs 的旧默认迁移
    // （要求 label 为空）误清空；解绑 / 空城市时 label 一起清空。
    if next.weather_city != prev.weather_city
        || next.weather_lat != prev.weather_lat
        || next.weather_lon != prev.weather_lon
    {
        next.weather_label = next.weather_city.trim().to_string();
    }

    let saved = super::apply_config(app, next).await?;

    // 剪贴板条数上限不在 IslandConfig：单独应用到内存态（5–100，模块内夹取）。
    let want = doc.clipboard.max.clamp(1, 100) as u32;
    if super::clipboard::island_clipboard_limit() != want {
        if let Err(e) = super::clipboard::island_clipboard_set_limit(want) {
            eprintln!("[zbar-island-settings] 应用剪贴板条数上限失败: {e}");
        }
    }
    Ok(saved)
}

// ============================================================
// 点路径 patch
// ============================================================

/// 把扁平点路径（`a.b.c`）写进 JSON 对象；中间层不存在时补空对象。
fn set_path(root: &mut Value, path: &str, value: Value) -> Result<(), String> {
    let segments: Vec<&str> = path.split('.').filter(|seg| !seg.is_empty()).collect();
    if segments.is_empty() {
        return Err("patch 里出现空路径".into());
    }
    let mut node = root;
    let last = segments.len() - 1;
    for (index, segment) in segments.iter().enumerate() {
        let Some(object) = node.as_object_mut() else {
            return Err(format!("patch 路径 {path} 的 {segment} 上级不是对象"));
        };
        if index == last {
            object.insert((*segment).to_string(), value);
            return Ok(());
        }
        if !object.contains_key(*segment) {
            object.insert((*segment).to_string(), Value::Object(Map::new()));
        }
        node = object
            .get_mut(*segment)
            .ok_or_else(|| format!("patch 路径 {path} 无法写入 {segment}"))?;
    }
    Ok(())
}

/// 文档 + 点路径 patch → 新文档（clamp 后）。只改 patch 里出现的键。
fn patched_doc(doc: SettingsDoc, patch: &Map<String, Value>) -> Result<SettingsDoc, String> {
    let mut value =
        serde_json::to_value(&doc).map_err(|e| format!("序列化设置文档失败: {e}"))?;
    for (path, next) in patch {
        set_path(&mut value, path, next.clone())?;
    }
    let next: SettingsDoc = serde_json::from_value(value)
        .map_err(|e| format!("patch 应用后的设置结构不合法: {e}"))?;
    // `island.transparency` / `island.font_size` 是外观页唯一权威键，直接映射
    // `IslandConfig.transparency` / `fontSize`；clamped() 负责夹取范围。
    Ok(next.clamped())
}

/// patch 里命中的、已接到运行时的点路径（canonical 形式）。
fn applied_paths(patch_keys: &[String]) -> Vec<String> {
    SYNCED_PATHS
        .iter()
        .filter(|path| {
            patch_keys.iter().any(|key| {
                key == *path || path.starts_with(&format!("{key}."))
            })
        })
        .map(|path| (*path).to_string())
        .collect()
}

fn all_applied_paths() -> Vec<String> {
    SYNCED_PATHS.iter().map(|path| (*path).to_string()).collect()
}

/// 本期尚未接线的点路径。
pub fn unsupported() -> Vec<String> {
    let mut paths: Vec<String> = UNSUPPORTED_PATHS
        .iter()
        .map(|path| (*path).to_string())
        .collect();
    // 采集排除只有 Windows 有等价能力（WDA_EXCLUDEFROMCAPTURE）。
    if !cfg!(target_os = "windows") {
        paths.push("island.exclude_from_capture".to_string());
        paths.push("island.hug_fullscreen".to_string());
        paths.push("media.netease_log".to_string());
        paths.push("media.playback_logs".to_string());
        paths.push("dsh.skip_when_active".to_string());
        paths.extend(["clash.config_dir", "clash.proxy_port", "clash.app"].map(str::to_owned));
        // 亮度 Twinkle 命令行只有 Windows 分支消费（`island::system` 的
        // `cfg(windows)` 分支）；其它平台继续按「未接入」提示，与上面
        // clash.* 同口径。
        paths.push("brightness.twinkle".to_string());
    }
    paths
}

// ============================================================
// 显示器 / 字体
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct MonitorEntry {
    pub index: i64,
    pub name: String,
    pub primary: bool,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
}

/// 显示器列表：按位置排序后重新编号（与设置页「显示器 1/2/…」一致，
/// 副屏坐标是负数也能稳定排序）。
fn monitor_entries(app: &tauri::AppHandle) -> Vec<MonitorEntry> {
    let primary_pos = app.primary_monitor().ok().flatten().map(|m| {
        let p = m.position();
        (p.x, p.y)
    });
    let mut rows: Vec<(i32, i32, String, u32, u32, f64)> = Vec::new();
    if let Ok(monitors) = app.available_monitors() {
        for (index, monitor) in monitors.iter().enumerate() {
            let pos = monitor.position();
            rows.push((
                pos.x,
                pos.y,
                monitor
                    .name()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| format!("显示器 {}", index + 1)),
                monitor.size().width,
                monitor.size().height,
                monitor.scale_factor(),
            ));
        }
    }
    rows.sort_by_key(|row| (row.0, row.1));
    rows.into_iter()
        .enumerate()
        .map(|(index, (x, y, name, width, height, scale_factor))| MonitorEntry {
            index: index as i64,
            name,
            primary: primary_pos == Some((x, y)),
            width,
            height,
            scale_factor,
        })
        .collect()
}

fn screen_for_monitor(monitors: &[MonitorEntry], monitor: Option<&str>) -> i64 {
    if let Some(name) = monitor {
        if let Some(entry) = monitors.iter().find(|entry| entry.name == name) {
            return entry.index;
        }
    }
    monitors
        .iter()
        .find(|entry| entry.primary)
        .map(|entry| entry.index)
        .unwrap_or(0)
}

fn monitor_for_screen(app: &tauri::AppHandle, screen: i64) -> Option<String> {
    if screen < 0 {
        return None;
    }
    monitor_entries(app)
        .get(screen as usize)
        .map(|entry| entry.name.clone())
}

fn monitor_names(app: &tauri::AppHandle) -> Vec<MonitorEntry> {
    monitor_entries(app)
}

#[derive(Debug, Clone, Serialize)]
pub struct FontEntry {
    pub family: String,
    pub has_chinese: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HotkeyStatus {
    pub toggle: bool,
    pub translate: bool,
    pub clipboard: bool,
}

/// 字体名是否大概率带中文字形（注册表没有语言元数据，只能按常见字体名
/// 关键字 + CJK 字符近似判断；前端只拿它做排序 / 分组提示）。
#[cfg(target_os = "windows")]
fn looks_chinese_font(family: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "雅黑", "微软", "宋体", "黑体", "楷体", "仿宋", "幼圆", "隶书", "等线", "华文", "方正",
        "思源", "苹方", "新宋", "圆体", "明体", "细明", "中易", "汉仪", "造字", "文泉", "霞鹜",
        "鸿蒙", "澎湃", "阿里巴巴",
        "SimSun", "SimHei", "KaiTi", "FangSong", "YaHei", "DengXian", "PingFang", "Noto Sans CJK",
        "Noto Sans SC", "Noto Serif SC", "Noto Sans TC", "Source Han", "JhengHei", "MingLiU",
        "YouYuan", "LiSu", "HarmonyOS Sans", "MiSans", "Alibaba PuHuiTi", "OPPO Sans",
    ];
    let lower = family.to_ascii_lowercase();
    if KEYWORDS
        .iter()
        .any(|keyword| family.contains(*keyword) || lower.contains(&keyword.to_ascii_lowercase()))
    {
        return true;
    }
    // 字体名本身就是中文的（如「优设鲨鱼菲特健康体」）时关键字表永远盖不
    // 全，直接按 CJK 统一表意文字范围判断。
    family
        .chars()
        .any(|ch| matches!(ch as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF))
}

/// 注册表值名 → 字体 family：注册表里是「微软雅黑 & Microsoft YaHei UI
/// (TrueType)」这种组合，去掉格式后缀、竖排 `@` 前缀，再按 `&` 拆开。
#[cfg(target_os = "windows")]
fn font_families_from_registry_value(value_name: &str) -> Vec<String> {
    let mut base = value_name.trim();
    for suffix in [
        " (TrueType)",
        " (OpenType)",
        " (All res)",
        "(TrueType)",
        "(OpenType)",
        "(All res)",
    ] {
        if let Some(stripped) = base.strip_suffix(suffix) {
            base = stripped.trim();
            break;
        }
    }
    base.split('&')
        .map(|part| part.trim().trim_start_matches('@').trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// 注册表原始值名 → 最终 family 列表：解析（拆 `&`、剥格式后缀与竖排 `@`
/// 前缀）→ 大小写不敏感去重（保留首个出现的写法）→ 按 lowercase 排序。
/// 不依赖 winreg，便于单测。
#[cfg(target_os = "windows")]
fn normalize_font_families<I: IntoIterator<Item = String>>(names: I) -> Vec<String> {
    let mut families: Vec<String> = names
        .into_iter()
        .flat_map(|name| font_families_from_registry_value(&name))
        .collect();
    families.sort_by_key(|family| family.to_lowercase());
    let mut seen = std::collections::HashSet::new();
    families.retain(|family| seen.insert(family.to_lowercase()));
    families
}

/// 读一个字体注册表键下的全部值名（值名即字体名，如「微软雅黑 (TrueType)」）；
/// 键不存在或没权限时返回空，由调用方决定跳过哪个来源。
#[cfg(target_os = "windows")]
fn read_font_registry_value_names(root: winreg::HKEY, subkey: &str) -> Vec<String> {
    use winreg::enums::KEY_READ;
    use winreg::RegKey;

    let mut names = Vec::new();
    if let Ok(key) = RegKey::predef(root).open_subkey_with_flags(subkey, KEY_READ) {
        for (name, _value) in key.enum_values().flatten() {
            names.push(name);
        }
    }
    names
}

/// 本机已安装字体：合并 HKLM 64 位、HKLM WOW6432Node 32 位与 HKCU 三个
/// 字体键——用户「为当前用户安装」的字体只登记在 HKCU，以前只读 HKLM 会
/// 漏掉；任一来源打不开只跳过它，不影响其它来源。
#[cfg(target_os = "windows")]
fn read_installed_fonts() -> Vec<FontEntry> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let raw_names = read_font_registry_value_names(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts",
    )
    .into_iter()
    .chain(read_font_registry_value_names(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\\WOW6432Node\\Microsoft\\Windows NT\\CurrentVersion\\Fonts",
    ))
    .chain(read_font_registry_value_names(
        HKEY_CURRENT_USER,
        r"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts",
    ));

    normalize_font_families(raw_names)
        .into_iter()
        .map(|family| {
            let has_chinese = looks_chinese_font(&family);
            FontEntry { family, has_chinese }
        })
        .collect()
}

// ============================================================
// 命令：数据契约
// ============================================================

#[derive(Debug, Clone, Serialize)]
pub struct SettingsPayload {
    pub data: SettingsDoc,
    pub defaults: SettingsDoc,
    pub path: String,
    pub data_dir: String,
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsSaveResult {
    pub data: SettingsDoc,
    pub unsupported: Vec<String>,
    pub applied: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsPaths {
    pub path: String,
    pub data_dir: String,
}

// ============================================================
// 命令
// ============================================================

fn paths_or_empty() -> (String, String) {
    match doc_path() {
        Ok(path) => {
            let data_dir = path
                .parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_default();
            (path.display().to_string(), data_dir)
        }
        Err(_) => (String::new(), String::new()),
    }
}

/// 设置页首帧：文档（叠加岛实时值）+ 出厂默认值 + 文件路径 + 未接线项。
#[tauri::command]
pub fn island_settings_doc(app: tauri::AppHandle) -> SettingsPayload {
    payload(&app)
}

#[tauri::command]
pub fn island_settings_reload(app: tauri::AppHandle) -> SettingsPayload {
    let data = payload(&app);
    super::notify::mail_settings_changed();
    data
}

/// 剪贴板条数上限不在 `IslandConfig` 里（模块内存态），读文档前单独 overlay
/// 一次实时值；只影响 UI 显示，保存时仍只写 patch 里出现的键。
fn overlay_clipboard_limit(doc: &mut SettingsDoc) {
    doc.clipboard.max = super::clipboard::island_clipboard_limit() as i64;
}

fn payload(app: &tauri::AppHandle) -> SettingsPayload {
    let mut data = load_for(app);
    overlay_from_island(&mut data, &super::load_config(), &monitor_names(app));
    overlay_clipboard_limit(&mut data);
    let data = data.clamped();
    let (path, data_dir) = paths_or_empty();
    SettingsPayload {
        data,
        defaults: SettingsDoc::default().clamped(),
        path,
        data_dir,
        unsupported: unsupported(),
    }
}

#[tauri::command]
pub fn island_settings_paths() -> SettingsPaths {
    let (path, data_dir) = paths_or_empty();
    SettingsPaths { path, data_dir }
}

/// 保存时标记「用户手动设置过」的项：之后启动自动探测不再覆盖，包括用户
/// 显式清空（patch 值为空）的情况。标记只进 `settings.json`，不进
/// `IslandConfig`（岛运行时不需要知道它是怎么来的）。
fn mark_manual_edits(doc: &mut SettingsDoc, patch: &Map<String, Value>) {
    for key in patch.keys() {
        if let Some(kind) = key.strip_prefix("notify.open.") {
            let kind = kind.trim().to_ascii_lowercase();
            if !super::NOTIFY_KINDS.contains(&kind.as_str())
                || doc.notify.open.open_manual.iter().any(|item| item == &kind)
            {
                continue;
            }
            doc.notify.open.open_manual.push(kind);
        } else if key == "clash.app" {
            doc.clash.app_manual = true;
        }
    }
}

/// 文档 + 点路径 patch 的核心实现：load → overlay 岛实时值 → 叠 patch →
/// clamp → 原子落盘 → 同步岛配置。**只写 patch 里出现的键**。
pub(crate) async fn apply_patch(
    app: &tauri::AppHandle,
    patch: &Map<String, Value>,
) -> Result<SettingsDoc, String> {
    let mut doc = load_for(app);
    overlay_from_island(&mut doc, &super::load_config(), &monitor_names(app));
    overlay_clipboard_limit(&mut doc);
    let mut next = patched_doc(doc, patch)?;
    mark_manual_edits(&mut next, patch);
    let data = persist_and_sync(app, next).await?;
    // 天气三项（api_key / units / interval_min）已接线：保存后立即按新配置
    // 重新拉一次，不用等下一次轮询。
    if patch.keys().any(|key| key.starts_with("weather.")) {
        super::weather::refresh();
    }
    Ok(data)
}

/// 恢复全部默认值并落盘 + 同步。
pub(crate) async fn reset_all(app: &tauri::AppHandle) -> Result<SettingsDoc, String> {
    persist_and_sync(app, SettingsDoc::default()).await
}

/// 导入 JSON 文件（顶层必须是对象）：与默认值深合并、clamp 后落盘 + 同步。
pub(crate) async fn import(
    app: &tauri::AppHandle,
    path: &str,
) -> Result<SettingsDoc, String> {
    let target = path.trim();
    if target.is_empty() {
        return Err("导入路径为空".into());
    }
    let text = std::fs::read_to_string(target)
        .map_err(|e| format!("读取导入文件失败: {e}"))?;
    let raw: Value = serde_json::from_str(&text)
        .map_err(|e| format!("解析导入文件失败: {e}"))?;
    if !raw.is_object() {
        return Err("这个文件不是一份灵动岛配置（顶层需要是 JSON 对象）".into());
    }
    let doc = from_value(raw)?;
    persist_and_sync(app, doc).await
}

/// 导出当前文档（先叠加岛实时值）为 pretty JSON。
pub(crate) fn export(app: &tauri::AppHandle, path: &str) -> Result<(), String> {
    let target = path.trim();
    if target.is_empty() {
        return Err("导出路径为空".into());
    }
    let mut doc = load_for(app);
    overlay_from_island(&mut doc, &super::load_config(), &monitor_names(app));
    overlay_clipboard_limit(&mut doc);
    let doc = doc.clamped();
    let text = serde_json::to_string_pretty(&doc)
        .map_err(|e| format!("序列化设置文档失败: {e}"))?;
    let target_path = Path::new(target);
    if let Some(parent) = target_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建导出目录失败: {e}"))?;
    }
    std::fs::write(target_path, format!("{text}\n"))
        .map_err(|e| format!("写入 {target} 失败: {e}"))?;
    Ok(())
}

/// 保存扁平点路径 patch：overlay 岛实时值 → 叠加 patch → clamp → 原子落盘
/// → 同步岛配置（clamped / 落盘 / 广播 / 副作用）→ 返回同步后的文档。
#[tauri::command]
pub async fn island_settings_save(
    patch: Value,
    app: tauri::AppHandle,
) -> Result<SettingsSaveResult, String> {
    let Value::Object(patch) = patch else {
        return Err("patch 必须是 JSON 对象（扁平点路径 → 值）".into());
    };
    let patch_keys: Vec<String> = patch.keys().cloned().collect();
    let data = apply_patch(&app, &patch).await?;
    Ok(SettingsSaveResult {
        data,
        unsupported: unsupported(),
        applied: applied_paths(&patch_keys),
    })
}

/// 恢复全部默认值并立即同步。
#[tauri::command]
pub async fn island_settings_reset(
    app: tauri::AppHandle,
) -> Result<SettingsSaveResult, String> {
    let data = reset_all(&app).await?;
    Ok(SettingsSaveResult {
        data,
        unsupported: unsupported(),
        applied: all_applied_paths(),
    })
}

/// 导入一份 JSON（顶层对象）覆盖当前设置；缺的项用默认值补齐，额外的
/// 未知字段原样保留。
#[tauri::command]
pub async fn island_settings_import(
    path: String,
    app: tauri::AppHandle,
) -> Result<SettingsSaveResult, String> {
    let data = import(&app, &path).await?;
    Ok(SettingsSaveResult {
        data,
        unsupported: unsupported(),
        applied: all_applied_paths(),
    })
}

/// 导出当前文档（先叠加岛实时值）为 pretty JSON。
#[tauri::command]
pub fn island_settings_export(
    path: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    export(&app, &path)
}

#[tauri::command]
pub fn island_settings_open_file() -> Result<(), String> {
    let path = doc_path()?;
    if !path.exists() {
        save_to_path(&path, &load())?;
    }
    open::that(&path).map_err(|e| format!("打开设置文件失败: {e}"))
}

#[tauri::command]
pub fn island_settings_reveal_file() -> Result<(), String> {
    let path = doc_path()?;
    if !path.exists() {
        save_to_path(&path, &load())?;
    }
    #[cfg(target_os = "windows")]
    {
        // explorer 退出码即使成功也可能是 1，只看能否拉起进程。
        std::process::Command::new("explorer")
            .arg("/select,")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("在资源管理器中定位设置文件失败: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let dir = path.parent().ok_or("设置文件没有父目录")?;
        open::that(dir).map_err(|e| format!("打开配置目录失败: {e}"))
    }
}

/// 隐藏设置窗（前端「有未保存修改时确认关闭」走这里，窗口 API 必须在
/// 主线程，所以投递 + 等待）。
#[tauri::command]
pub async fn island_settings_window_hide(app: tauri::AppHandle) -> Result<(), String> {
    let app_main = app.clone();
    super::on_main_thread(&app, move || {
        crate::island_settings_window::hide_island_settings_window_on_main(&app_main);
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn island_fonts() -> Vec<FontEntry> {
    #[cfg(target_os = "windows")]
    {
        read_installed_fonts()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

#[tauri::command]
pub fn island_monitors(app: tauri::AppHandle) -> Vec<MonitorEntry> {
    monitor_entries(&app)
}

/// 三个全局热键的注册状态。
#[tauri::command]
pub fn island_hotkey_status(app: tauri::AppHandle) -> HotkeyStatus {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let cfg = super::load_config();
    let shortcuts = app.global_shortcut();
    let registered = |key: &str| {
        let key = key.trim();
        !key.is_empty() && shortcuts.is_registered(key)
    };
    let toggle_key = cfg.hotkey_toggle.trim();
    let clipboard_key = cfg.hotkey_clipboard.trim();
    let translate_key = cfg.hotkey_translate.trim();
    HotkeyStatus {
        toggle: registered(toggle_key),
        // register_hotkeys 会跳过与开关 / 剪贴板重复的组合；这里按同一规则
        // 判断，字符串相同也不会谎报「已注册」。
        translate: translate_key != toggle_key
            && translate_key != clipboard_key
            && registered(translate_key),
        // 与开关热键相同时 register_hotkeys 会跳过剪贴板注册，避免覆盖。
        clipboard: clipboard_key != toggle_key && registered(clipboard_key),
    }
}

/// 落盘 + 同步 + 把运行时归一化结果回写文档（例如股票刷新 2s 会被岛端
/// clamp 成 3s、自选股代码会大小写归一），保证文件与运行时一致。
async fn persist_and_sync(
    app: &tauri::AppHandle,
    doc: SettingsDoc,
) -> Result<SettingsDoc, String> {
    let next = doc.clamped();
    save_atomic(&next)?;
    super::notify::mail_settings_changed();
    let synced = sync_to_island(app, &next).await?;
    let mut data = next.clone();
    overlay_from_island(&mut data, &synced, &monitor_names(app));
    let data = data.clamped();
    if data != next {
        save_atomic(&data)?;
    }
    Ok(data)
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 每个测试一个临时目录，Drop 时清理（不碰真实 `~/.agenthud`）。
    struct TempFixture {
        dir: PathBuf,
    }

    impl TempFixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "agenthud-settings-test-{}-{tag}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("创建测试目录");
            Self { dir }
        }

        fn path(&self) -> PathBuf {
            self.dir.join(SETTINGS_FILE)
        }
    }

    impl Drop for TempFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn monitor(index: i64, name: &str, primary: bool) -> MonitorEntry {
        MonitorEntry {
            index,
            name: name.to_string(),
            primary,
            width: 1920,
            height: 1080,
            scale_factor: 1.0,
        }
    }

    #[test]
    fn defaults_match_agenthud_runtime() {
        let doc = SettingsDoc::default();
        // 出厂默认值
        assert_eq!(doc.island.edge, "top");
        assert!((doc.island.pos - 0.5).abs() < 1e-9);
        assert_eq!(doc.island.screen, 0);
        assert_eq!(doc.island.font_family, "");
        assert!(doc.island.keep_on_top);
        assert!(doc.island.hug_fullscreen);
        assert!(doc.island.hover_expand);
        assert_eq!(doc.island.hover_expand_ms, 120);
        assert_eq!(doc.island.collapse_delay_ms, 320);
        assert!(!doc.island.exclude_from_capture);
        assert_eq!(doc.island.blur_fps, 8);
        assert_eq!(doc.island.expand_mode, "hover");
        assert!(!doc.island.passthrough);
        assert_eq!(doc.island.auto_cycle_s, 30);
        assert!(doc.island.hidden_tabs.is_empty());
        // AgentHUD 现状覆盖项
        assert_eq!(doc.island.theme, "dark");
        // 默认关闭全屏隐藏，与 IslandConfig::default 保持同一口径
        assert!(!doc.island.hide_fullscreen);
        assert_eq!(doc.island.compact_view, "rings");
        assert_eq!(doc.island.font_size, 13.0);
        assert_eq!(doc.island.transparency, 0);
        assert_eq!(doc.hotkeys.toggle, "Ctrl+Alt+Shift+I");
        assert_eq!(doc.hotkeys.translate, "Ctrl+Alt+J");
        assert_eq!(doc.hotkeys.clipboard, "Ctrl+Alt+V");
        assert_eq!(doc.notify.modules, vec!["qq", "wechat", "mail"]);
        assert_eq!(doc.notify.body_preview, vec!["feishu", "dingtalk"]);
        assert_eq!(
            doc.stocks.watchlist,
            vec!["sh000001", "sz399001", "sz399006", "hk00700", "usAAPL", "usNVDA", "usQQQ"]
        );
        assert_eq!(doc.stocks.interval, 5);
        assert!((doc.stocks.pct_alert - 3.0).abs() < 1e-9);
        assert_eq!(doc.clipboard.max, 10);
        assert_eq!(doc.launcher.len(), super::super::default_launcher().len());
        // 网络 / 媒体 / 系统等默认值
        assert_eq!(doc.media.lyrics_sources, "netease,lrclib,custom");
        assert_eq!(doc.media.priority.len(), 6);
        assert!(doc.notify.popup.enabled);
        assert_eq!(doc.notify.popup.position, "bottom_right");
        assert_eq!(doc.notify.popup.size, 100);
        assert_eq!(doc.dsh.body_chars, 120);
        assert_eq!(doc.mail.interval_s, 120);
        assert_eq!(doc.weather.city, "", "默认不绑定城市，主城市跟随 IP");
        assert!(doc.weather.cities.is_empty());
        assert_eq!(doc.weather.units, "metric");
        assert_eq!(doc.weather.interval_min, 15);
        assert!(!doc.translate.custom_enabled);
        assert!(doc.translate.base_url.is_empty());
        assert_eq!(doc.live.models_dir, "models");
        assert!((doc.live.vad.threshold - 0.4).abs() < 1e-9);
        assert_eq!(doc.live.asr.final_threads, 6);
        assert_eq!(doc.live.translate.context_lines, 3);
        assert_eq!(doc.clash.proxy_port, 7897);
        assert!(doc.notify.open.open_manual.is_empty());
        assert!(!doc.clash.app_manual);
        assert!(doc.brightness.twinkle.is_empty());
        assert!(doc.agenthud.agent_order.is_empty());
    }

    #[test]
    fn first_load_seeds_complete_json_file() {
        let fixture = TempFixture::new("seed");
        let path = fixture.path();
        let seed = IslandConfig::default();
        let doc = load_from_path(&path, &seed, &[]).expect("首次生成");
        assert!(path.exists(), "首次 load 必须落盘 settings.json");

        let text = std::fs::read_to_string(&path).expect("读文件");
        let raw: Value = serde_json::from_str(&text).expect("JSON 可解析");
        for section in [
            "island",
            "hotkeys",
            "media",
            "notify",
            "dsh",
            "mail",
            "stocks",
            "weather",
            "translate",
            "live",
            "clipboard",
            "launcher",
            "clash",
            "brightness",
            "agenthud",
        ] {
            assert!(raw.get(section).is_some(), "settings.json 缺 {section}");
        }
        // 嵌套结构完整（不是只有顶层壳）
        assert_eq!(raw["notify"]["popup"]["position"], "bottom_right");
        assert_eq!(raw["notify"]["open"]["dsh"], "");
        assert_eq!(raw["live"]["audio"]["source"], "loopback");
        assert_eq!(raw["live"]["vad"]["min_silence_ms"], 350);
        assert_eq!(raw["live"]["asr"]["partial"], true);
        assert_eq!(raw["live"]["translate"]["live_min_interval_ms"], 900);
        assert_eq!(raw["mail"]["imap"], json!([]));
        assert_eq!(raw["weather"]["cities"], json!([]));
        assert_eq!(raw["launcher"].as_array().map(|a| a.len()), Some(5));
        assert!(raw["launcher"][0]["icon"].is_string(), "启动器保留 AgentHUD icon");
        assert_eq!(raw["island"]["theme"], "dark");
        assert_eq!(raw["island"]["transparency"], 0);
        assert_eq!(raw["island"]["font_size"], 13.0);
        assert_eq!(raw["island"]["compact_view"], "rings");
        assert_eq!(raw["stocks"]["interval"], 5);
        assert_eq!(raw["clipboard"]["max"], 10);

        // 幂等：再读一次得到同一份文档
        let again = load_from_path(&path, &seed, &[]).expect("二次读取");
        assert_eq!(doc, again);

        // 手改文件只写一部分时，缺失项用默认值补齐但不丢已有值
        let partial = json!({ "island": { "theme": "sky" } });
        std::fs::write(&path, serde_json::to_string_pretty(&partial).unwrap()).unwrap();
        let merged = load_from_path(&path, &seed, &[]).expect("读取部分文件");
        assert_eq!(merged.island.theme, "sky");
        assert_eq!(merged.island.transparency, 0);
        assert_eq!(merged.notify.modules, vec!["qq", "wechat", "mail"]);
    }

    fn assert_json_path(raw: &Value, path: &str) {
        let mut node = raw;
        for segment in path.split('.') {
            node = node
                .get(segment)
                .unwrap_or_else(|| panic!("settings.json 缺字段 {path}"));
        }
    }

    /// 首次生成的 JSON 必须包含任务约定的全部点路径（不只是顶层壳）。
    #[test]
    fn payload_field_names_match_settings_window_contract() {
        // 前端 wset/settingsApi.ts 按 snake_case 读取（data_dir / has_chinese /
        // scale_factor）；这些 DTO 不能加 camelCase rename。
        let payload = SettingsPayload {
            data: SettingsDoc::default(),
            defaults: SettingsDoc::default(),
            path: "p".into(),
            data_dir: "d".into(),
            unsupported: vec!["x".into()],
        };
        let value = serde_json::to_value(&payload).expect("payload 可序列化");
        assert!(value.get("data_dir").is_some());
        assert!(value.get("dataDir").is_none());

        let save = SettingsSaveResult {
            data: SettingsDoc::default(),
            unsupported: Vec::new(),
            applied: vec!["island.edge".into()],
        };
        let value = serde_json::to_value(&save).expect("save result 可序列化");
        assert_eq!(value["applied"], json!(["island.edge"]));

        let font = FontEntry {
            family: "SimSun".into(),
            has_chinese: true,
        };
        let value = serde_json::to_value(&font).expect("font 可序列化");
        assert_eq!(value["has_chinese"], json!(true));

        let monitor = MonitorEntry {
            index: 0,
            name: "显示器 1".into(),
            primary: true,
            width: 1920,
            height: 1080,
            scale_factor: 1.0,
        };
        let value = serde_json::to_value(&monitor).expect("monitor 可序列化");
        assert_eq!(value["scale_factor"], json!(1.0));
    }

    #[test]
    fn seeded_file_contains_every_documented_path() {
        let fixture = TempFixture::new("paths");
        let path = fixture.path();
        load_from_path(&path, &IslandConfig::default(), &[]).expect("首次生成");
        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for point in [
            "island.edge",
            "island.pos",
            "island.screen",
            "island.theme",
            "island.font_family",
            "island.keep_on_top",
            "island.hug_fullscreen",
            "island.hover_expand",
            "island.hover_expand_ms",
            "island.collapse_delay_ms",
            "island.exclude_from_capture",
            "island.blur_fps",
            "island.expand_mode",
            "island.passthrough",
            "island.hide_fullscreen",
            "island.compact_view",
            "island.quota_display",
            "island.quota_ring_style",
            "island.auto_cycle_s",
            "island.hidden_tabs",
            "island.transparency",
            "island.font_size",
            "hotkeys.toggle",
            "hotkeys.translate",
            "hotkeys.clipboard",
            "media.auto_show",
            "media.lyrics",
            "media.netease_log",
            "media.playback_logs",
            "media.lyrics_sources",
            "media.lyrics_custom_url",
            "media.priority",
            "notify.modules",
            "notify.body_preview",
            "notify.popup.enabled",
            "notify.popup.position",
            "notify.popup.size",
            "notify.open.qq",
            "notify.open.wechat",
            "notify.open.mail",
            "notify.open.feishu",
            "notify.open.dingtalk",
            "notify.open.dsh",
            "dsh.enabled",
            "dsh.sessions_dir",
            "dsh.include_subagents",
            "dsh.skip_when_active",
            "dsh.body_chars",
            "mail.interval_s",
            "mail.imap",
            "stocks.watchlist",
            "stocks.interval",
            "stocks.pct_alert",
            "weather.api_key",
            "weather.city",
            "weather.lat",
            "weather.lon",
            "weather.units",
            "weather.interval_min",
            "weather.cities",
            "translate.custom_enabled",
            "translate.platform",
            "translate.platform_model",
            "translate.api_key",
            "translate.base_url",
            "translate.model",
            "translate.import_from",
            "live.models_dir",
            "live.audio.source",
            "live.audio.device",
            "live.vad.threshold",
            "live.vad.min_silence_ms",
            "live.vad.min_speech_ms",
            "live.asr.partial",
            "live.asr.draft_interval_ms",
            "live.asr.final_threads",
            "live.asr.max_segment_s",
            "live.asr.context_prompt",
            "live.translate.enabled",
            "live.translate.live",
            "live.translate.live_min_interval_ms",
            "live.translate.live_min_new_chars",
            "live.translate.context_lines",
            "clipboard.max",
            "launcher",
            "clash.config_dir",
            "clash.proxy_port",
            "clash.app",
            "brightness.twinkle",
            "agenthud.agent_order",
        ] {
            assert_json_path(&raw, point);
        }
        // 空数组 / null 也要保留键（前端据此渲染空态）
        assert!(raw["island"]["hidden_tabs"].is_array());
        assert!(raw["mail"]["imap"].is_array());
        assert!(raw["weather"]["cities"].is_array());
        assert!(raw["weather"]["lat"].is_null());
        assert!(raw["weather"]["lon"].is_null());
        assert!(raw["launcher"].is_array());
        // 启动自动探测的「手动设置过」标记也要落盘（前端不渲染，但必须往返）
        assert_eq!(raw["notify"]["open"]["open_manual"], json!([]));
        assert_eq!(raw["clash"]["app_manual"], json!(false));
    }

    #[test]
    fn first_load_seeds_screen_index_from_monitor_name() {
        let fixture = TempFixture::new("screen-seed");
        let mut seed = IslandConfig::default();
        seed.monitor = Some("\\\\.\\DISPLAY2".into());
        seed.edge = "left".into();
        let monitors = vec![
            monitor(0, "\\\\.\\DISPLAY1", true),
            monitor(1, "\\\\.\\DISPLAY2", false),
        ];
        let doc = load_from_path(&fixture.path(), &seed, &monitors).expect("首次生成");
        assert_eq!(doc.island.screen, 1, "设备名要落成稳定索引");
        assert_eq!(doc.island.edge, "left");
        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(fixture.path()).unwrap()).unwrap();
        assert_eq!(raw["island"]["screen"], 1);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let mut doc = SettingsDoc::default();
        doc.island.edge = " LEFT ".into();
        doc.island.pos = 1.7;
        doc.island.screen = -3;
        doc.island.theme = "NEON".into();
        doc.island.hover_expand_ms = 9_000;
        doc.island.collapse_delay_ms = 9_000;
        doc.island.blur_fps = 99;
        doc.island.auto_cycle_s = -5;
        doc.island.expand_mode = "sometimes".into();
        doc.island.compact_view = "".into();
        doc.island.hidden_tabs = vec![
            " CLIP ".into(),
            "bogus".into(),
            "clip".into(),
            " TrAnSlAtE ".into(),
        ];
        doc.island.transparency = 300;
        doc.island.font_size = 99.0;
        doc.hotkeys.toggle = "  Ctrl+Alt+K  ".into();
        doc.notify.popup.position = "middle".into();
        doc.notify.popup.size = 10;
        doc.dsh.body_chars = 1;
        doc.mail.interval_s = 5;
        doc.stocks.interval = 1;
        doc.stocks.pct_alert = 99.0;
        doc.weather.interval_min = 999;
        doc.weather.lat = Some(120.0);
        doc.weather.lon = Some(30.0);
        doc.weather.units = "kelvin".into();
        doc.live.audio.source = "mic".into();
        doc.live.vad.threshold = 2.0;
        doc.live.vad.min_silence_ms = 1;
        doc.live.vad.min_speech_ms = 99_999;
        doc.live.asr.draft_interval_ms = 1;
        doc.live.asr.final_threads = 99;
        doc.live.asr.max_segment_s = 300;
        doc.live.translate.live_min_interval_ms = 1;
        doc.live.translate.live_min_new_chars = 0;
        doc.live.translate.context_lines = 99;
        doc.clipboard.max = 0;
        doc.clash.proxy_port = 70_000;

        let doc = doc.clamped();
        assert_eq!(doc.island.edge, "left");
        assert!((doc.island.pos - 1.0).abs() < 1e-9);
        assert_eq!(doc.island.screen, 0);
        assert_eq!(doc.island.theme, "dark");
        assert_eq!(doc.island.hover_expand_ms, 1000);
        assert_eq!(doc.island.collapse_delay_ms, 2000);
        assert_eq!(doc.island.blur_fps, 30);
        assert_eq!(doc.island.auto_cycle_s, 0);
        assert_eq!(doc.island.expand_mode, "hover");
        assert_eq!(doc.island.compact_view, "rings");
        assert_eq!(doc.island.hidden_tabs, vec!["clip", "translate"]);
        assert_eq!(doc.island.transparency, 90);
        assert!((doc.island.font_size - 18.0).abs() < 1e-9);
        assert_eq!(doc.hotkeys.toggle, "Ctrl+Alt+K");
        assert_eq!(doc.notify.popup.position, "bottom_right");
        assert_eq!(doc.notify.popup.size, 70);
        assert_eq!(doc.dsh.body_chars, 40);
        assert_eq!(doc.mail.interval_s, 30);
        assert_eq!(doc.stocks.interval, 2);
        assert!((doc.stocks.pct_alert - 20.0).abs() < 1e-9);
        assert_eq!(doc.weather.interval_min, 180);
        assert_eq!(doc.weather.lat, None, "越界经纬度成对清空");
        assert_eq!(doc.weather.lon, None);
        assert_eq!(doc.weather.units, "metric");
        assert_eq!(doc.live.audio.source, "mic");
        assert!((doc.live.vad.threshold - 0.9).abs() < 1e-9);
        assert_eq!(doc.live.vad.min_silence_ms, 100);
        assert_eq!(doc.live.vad.min_speech_ms, 1000);
        assert_eq!(doc.live.asr.draft_interval_ms, 200);
        assert_eq!(doc.live.asr.final_threads, 16);
        assert_eq!(doc.live.asr.max_segment_s, 30);
        assert_eq!(doc.live.translate.live_min_interval_ms, 300);
        assert_eq!(doc.live.translate.live_min_new_chars, 1);
        assert_eq!(doc.live.translate.context_lines, 3);
        assert_eq!(doc.clipboard.max, 1);
        assert_eq!(doc.clash.proxy_port, 65535);

        // 低端同样夹取；NaN 回退默认
        let mut low = SettingsDoc::default();
        low.island.font_size = 1.0;
        low.island.pos = f64::NAN;
        low.island.hover_expand_ms = -10;
        let low = low.clamped();
        assert!((low.island.font_size - 10.0).abs() < 1e-9);
        assert!((low.island.pos - 0.5).abs() < 1e-9);
        assert_eq!(low.island.hover_expand_ms, 0);
    }

    #[test]
    fn quota_display_patches_and_round_trips_through_settings() {
        let path = "island.quota_display";
        let legacy = from_value(json!({"island": {"theme": "sky"}})).unwrap();
        assert_eq!(legacy.island.quota_display, "percent");
        assert_eq!(applied_paths(&[path.into()]), vec![path]);

        let patch = Map::from_iter([(path.into(), json!(" RiNgS "))]);
        let next = patched_doc(legacy, &patch).unwrap();
        assert_eq!(next.island.quota_display, "rings");

        let fixture = TempFixture::new("quota-display");
        save_to_path(&fixture.path(), &next).unwrap();
        let loaded = read_doc_file(&fixture.path()).unwrap();
        assert_eq!(loaded.island.quota_display, "rings");
        assert_eq!(loaded.island.theme, "sky");

        let cfg = IslandConfig {
            quota_display: loaded.island.quota_display,
            ..IslandConfig::default()
        };
        let mut runtime_doc = SettingsDoc::default();
        overlay_from_island(&mut runtime_doc, &cfg, &[]);
        assert_eq!(runtime_doc.island.quota_display, "rings");

        let patch = Map::from_iter([(path.into(), json!("invalid"))]);
        let reset = patched_doc(runtime_doc, &patch).unwrap();
        assert_eq!(reset.island.quota_display, "percent");
    }

    #[test]
    fn quota_ring_style_patches_and_round_trips_through_settings() {
        let path = "island.quota_ring_style";
        let legacy = from_value(json!({"island": {"theme": "sky"}})).unwrap();
        assert_eq!(legacy.island.quota_ring_style, "period");
        assert_eq!(applied_paths(&[path.into()]), vec![path]);

        let patch = Map::from_iter([(path.into(), json!(" DyNaMiC "))]);
        let next = patched_doc(legacy, &patch).unwrap();
        assert_eq!(next.island.quota_ring_style, "dynamic");

        let fixture = TempFixture::new("quota-ring-style");
        save_to_path(&fixture.path(), &next).unwrap();
        let loaded = read_doc_file(&fixture.path()).unwrap();
        assert_eq!(loaded.island.quota_ring_style, "dynamic");
        assert_eq!(loaded.island.theme, "sky");

        let cfg = IslandConfig {
            quota_ring_style: loaded.island.quota_ring_style,
            ..IslandConfig::default()
        };
        let mut runtime_doc = SettingsDoc::default();
        overlay_from_island(&mut runtime_doc, &cfg, &[]);
        assert_eq!(runtime_doc.island.quota_ring_style, "dynamic");

        let patch = Map::from_iter([(path.into(), json!("invalid"))]);
        let reset = patched_doc(runtime_doc, &patch).unwrap();
        assert_eq!(reset.island.quota_ring_style, "period");
    }

    #[test]
    fn translation_settings_preserve_both_sources_when_switched() {
        let legacy = from_value(json!({
            "translate": {
                "api_key": "test-custom-key",
                "base_url": "http://localhost:5000/v1",
                "model": "custom-model"
            }
        }))
        .unwrap();
        assert!(!legacy.translate.custom_enabled);

        let patch = Map::from_iter([
            ("translate.platform".into(), json!("test:credential")),
            ("translate.platform_model".into(), json!(" example-model ")),
            ("translate.custom_enabled".into(), json!(true)),
        ]);
        let next = patched_doc(legacy, &patch).unwrap();
        let fixture = TempFixture::new("translation-sources");
        save_to_path(&fixture.path(), &next).unwrap();
        let loaded = read_doc_file(&fixture.path()).unwrap();
        assert!(loaded.translate.custom_enabled);
        let disabled = patched_doc(
            loaded,
            &Map::from_iter([("translate.custom_enabled".into(), json!(false))]),
        )
        .unwrap();
        assert!(!disabled.translate.custom_enabled);
        assert_eq!(disabled.translate.platform, "test:credential");
        assert_eq!(disabled.translate.platform_model, "example-model");
        assert_eq!(disabled.translate.api_key, "test-custom-key");
        assert_eq!(disabled.translate.base_url, "http://localhost:5000/v1");
        assert_eq!(disabled.translate.model, "custom-model");
    }

    #[test]
    fn point_path_patch_only_touches_given_keys() {
        let doc = SettingsDoc::default();
        let mut patch = Map::new();
        patch.insert("island.edge".into(), json!("bottom"));
        patch.insert("notify.popup.enabled".into(), json!(false));
        patch.insert(
            "launcher".into(),
            json!([
                { "name": "计算器", "path": "calc.exe", "icon": "task", "args": ["-x"] }
            ]),
        );
        let next = patched_doc(doc.clone(), &patch).expect("patch 可用");
        assert_eq!(next.island.edge, "bottom");
        assert_eq!(next.island.pos, doc.island.pos, "未提交字段保持不变");
        assert_eq!(next.island.theme, doc.island.theme);
        assert!(!next.notify.popup.enabled);
        assert_eq!(next.notify.popup.position, "bottom_right");
        assert_eq!(next.launcher.len(), 1, "数组整体替换");
        assert_eq!(next.launcher[0].name, "计算器");
        assert_eq!(next.launcher[0].icon.as_deref(), Some("task"));
        assert_eq!(next.launcher[0].extra["args"], json!(["-x"]));

        // 深层新键也能写（未知额外字段留给 flatten）
        let mut patch = Map::new();
        patch.insert("island.screen".into(), json!(1));
        let next = patched_doc(next, &patch).expect("深层 patch");
        assert_eq!(next.island.screen, 1);

        // 外观页的玻璃透明度 / 字号走 island.transparency / island.font_size
        let mut appearance = Map::new();
        appearance.insert("island.transparency".into(), json!(60));
        appearance.insert("island.font_size".into(), json!(15.5));
        let styled = patched_doc(SettingsDoc::default(), &appearance).expect("外观 patch");
        assert_eq!(styled.island.transparency, 60);
        assert_eq!(styled.island.font_size, 15.5);

        // 非法路径要报错，而不是静默丢 patch
        let mut bad_patch = Map::new();
        bad_patch.insert("island.edge".into(), json!("top"));
        bad_patch.insert("island.edge.deep".into(), json!(1));
        assert!(patched_doc(SettingsDoc::default(), &bad_patch).is_err());
        // `a..b` 会被 split 过滤成 a.b；这里只断言不会 panic 且写入成功
        let mut broken = Map::new();
        broken.insert("island..edge".into(), json!("top"));
        let fixed = patched_doc(SettingsDoc::default(), &broken).expect("空段被忽略");
        assert_eq!(fixed.island.edge, "top");
    }

    /// 媒体与「打开应用」：patch → 归一化 → IslandConfig 字段的往返。
    #[test]
    fn media_and_open_patch_normalize_like_island_config() {
        let mut patch = Map::new();
        patch.insert("media.auto_show".into(), json!(false));
        patch.insert("media.lyrics".into(), json!(false));
        patch.insert(
            "media.lyrics_sources".into(),
            json!(" LRCLIB , bogus , netease ,lrclib"),
        );
        patch.insert(
            "media.lyrics_custom_url".into(),
            json!("  https://x/{title}  "),
        );
        patch.insert(
            "media.priority".into(),
            json!([
                " Chrome ", "", "chrome", "QQMusic", "a", "b", "c", "d", "e", "f", "g", "h", "i",
                "j", "k", "l", "m", "n", "o", "p"
            ]),
        );
        patch.insert("notify.open.qq".into(), json!("  C:\\QQ\\QQ.exe  "));
        patch.insert("notify.open.dsh".into(), json!("   "));
        let doc = patched_doc(SettingsDoc::default(), &patch).expect("媒体 patch 可用");

        assert!(!doc.media.auto_show);
        assert!(!doc.media.lyrics);
        // 顺序保留用户所写、去重、白名单外丢弃
        assert_eq!(doc.media.lyrics_sources, "lrclib,netease");
        assert_eq!(doc.media.lyrics_custom_url, "https://x/{title}");
        // 大小写不敏感去重 + 最多 16 项
        assert_eq!(doc.media.priority.len(), 16);
        assert_eq!(doc.media.priority[0], "Chrome");
        assert_eq!(doc.media.priority[1], "QQMusic");
        assert_eq!(doc.media.priority[15], "n");
        assert_eq!(doc.notify.open.qq, "C:\\QQ\\QQ.exe");
        assert_eq!(doc.notify.open.dsh, "", "空路径不写进文档");

        // 文档 → IslandConfig：open_map 只收非空值
        let map = open_map(&doc.notify.open);
        assert_eq!(map.get("qq").map(String::as_str), Some("C:\\QQ\\QQ.exe"));
        assert!(!map.contains_key("dsh"));

        // IslandConfig → 文档：同一批字段回填，旧值被覆盖
        let cfg = IslandConfig {
            media_auto_show: false,
            media_lyrics: false,
            media_lyrics_sources: "custom,netease".into(),
            media_lyrics_custom_url: "https://y/{artist}".into(),
            media_priority: vec!["spotify".into(), "qqmusic".into()],
            notify_open: std::collections::BTreeMap::from([(
                "wechat".to_string(),
                "D:\\WeChat.exe".to_string(),
            )]),
            ..IslandConfig::default()
        };
        let mut doc = SettingsDoc::default();
        doc.notify.open.qq = "old".into();
        overlay_from_island(&mut doc, &cfg, &[]);
        assert!(!doc.media.auto_show);
        assert!(!doc.media.lyrics);
        assert_eq!(doc.media.lyrics_sources, "custom,netease");
        assert_eq!(doc.media.lyrics_custom_url, "https://y/{artist}");
        assert_eq!(doc.media.priority, vec!["spotify", "qqmusic"]);
        assert_eq!(doc.notify.open.wechat, "D:\\WeChat.exe");
        assert_eq!(doc.notify.open.qq, "", "旧值没有对应 cfg 时清空");
    }

    /// 保存 patch 时打「手动设置过」标记：之后启动自动探测不再覆盖，包括
    /// 用户显式清空（值为空）的情况。
    #[test]
    fn manual_markers_are_recorded_from_save_patch() {
        let patch = Map::from_iter([
            ("notify.open.qq".to_string(), json!(" C:/QQ/QQ.exe ")),
            ("notify.open.dsh".to_string(), json!("   ")),
            ("clash.app".to_string(), json!("")),
            ("island.edge".to_string(), json!("bottom")),
        ]);
        let mut doc = patched_doc(SettingsDoc::default(), &patch).expect("patch 可用");
        mark_manual_edits(&mut doc, &patch);
        let doc = doc.clamped();
        assert_eq!(doc.notify.open.qq, "C:/QQ/QQ.exe");
        assert_eq!(doc.notify.open.open_manual, vec!["qq", "dsh"]);
        assert!(doc.clash.app_manual);

        // 标记要能随 settings.json 往返（不依赖岛运行时）
        let fixture = TempFixture::new("manual-markers");
        save_to_path(&fixture.path(), &doc).expect("落盘");
        let loaded = read_doc_file(&fixture.path()).expect("读回");
        assert_eq!(loaded.notify.open.open_manual, vec!["qq", "dsh"]);
        assert!(loaded.clash.app_manual);

        // 非白名单 kind / 其他 clash 键都不打标记
        let patch = Map::from_iter([
            ("notify.open.bogus".to_string(), json!("x")),
            ("clash.proxy_port".to_string(), json!(7890)),
        ]);
        let mut doc = patched_doc(SettingsDoc::default(), &patch).expect("patch 可用");
        mark_manual_edits(&mut doc, &patch);
        assert!(doc.notify.open.open_manual.is_empty());
        assert!(!doc.clash.app_manual);
    }

    /// `open_manual` 归一化：trim / 小写 / 白名单 / 去重，顺序按 NOTIFY_KINDS。
    #[test]
    fn open_manual_normalizes_to_notify_kind_whitelist() {
        let mut doc = SettingsDoc::default();
        doc.notify.open.open_manual = vec![
            " DSH ".into(),
            "qq".into(),
            "WeChat".into(),
            "bogus".into(),
            String::new(),
            "qq".into(),
        ];
        assert_eq!(
            doc.clamped().notify.open.open_manual,
            vec!["qq", "wechat", "dsh"]
        );
    }

    /// 启动探测只填「空且没被手动设置过」的项；已有值 / 手动清空都不覆盖，
    /// 探测本身也不写手动标记。
    #[test]
    fn seed_only_fills_empty_slots_without_manual_marker() {
        let mut doc = SettingsDoc::default();
        doc.notify.open.wechat = "D:/wechat.exe".into();
        doc.notify.open.open_manual = vec!["qq".into()];
        doc.clash.app_manual = true;
        let changed = fill_detected_paths(
            &mut doc,
            |kind| Some(format!("D:/{kind}.exe")),
            || Some("D:/Clash Verge/clash-verge.exe".into()),
            || Some("D:/Twinkle/Twinkle Tray.exe".into()),
        );
        assert!(changed);
        assert_eq!(doc.notify.open.qq, "", "手动清空过的项不再被填充");
        assert_eq!(doc.notify.open.wechat, "D:/wechat.exe", "已有值不覆盖");
        assert_eq!(doc.notify.open.mail, "D:/mail.exe");
        assert_eq!(doc.notify.open.feishu, "D:/feishu.exe");
        assert_eq!(doc.notify.open.dingtalk, "D:/dingtalk.exe");
        assert_eq!(doc.notify.open.dsh, "D:/dsh.exe");
        assert_eq!(doc.clash.app, "", "手动设置过 Clash 路径不再被填充");
        assert_eq!(doc.notify.open.open_manual, vec!["qq"], "探测不写手动标记");
        assert_eq!(
            doc.brightness.twinkle, "D:/Twinkle/Twinkle Tray.exe",
            "Twinkle 空项由启动探测填上"
        );

        // 全部填满 / 有标记后再跑一遍：探测闭包一次都不该被调用
        doc.brightness.twinkle = "D:/custom/Twinkle.exe".into();
        let calls = std::cell::Cell::new(0u32);
        let changed = fill_detected_paths(
            &mut doc,
            |_| {
                calls.set(calls.get() + 1);
                None
            },
            || {
                calls.set(calls.get() + 1);
                None
            },
            || {
                calls.set(calls.get() + 1);
                None
            },
        );
        assert!(!changed);
        assert_eq!(calls.get(), 0);
        assert_eq!(
            doc.brightness.twinkle, "D:/custom/Twinkle.exe",
            "Twinkle 已有值不覆盖"
        );
    }

    /// P1-1：启动探测必须先叠加岛实时值再填空项——否则落盘 + 同步时会把
    /// settings.json 里的旧配置写回 `IslandConfig`，把用户在岛内改过的项
    /// （拖动 / 右键菜单只写 IslandConfig）静默回退。
    #[test]
    fn seed_overlays_island_runtime_before_detection() {
        // 磁盘文档落后：边沿 / 隐藏板块还是旧值
        let mut doc = SettingsDoc::default();
        doc.island.edge = "left".into();
        doc.island.hidden_tabs = vec!["stock".into()];
        // 岛运行时已被用户改过
        let runtime = IslandConfig {
            edge: "top".into(),
            hidden_tabs: Vec::new(),
            ..IslandConfig::default()
        };
        let changed = seed_with_overlay(
            &mut doc,
            &runtime,
            &[monitor(0, "主屏", true)],
            |_| None,
            || None,
            || None,
        );
        // 没有新填入 → 不落盘；但文档已按岛实时值修正，后续同步不会回退
        assert!(!changed);
        assert_eq!(doc.island.edge, "top");
        assert!(doc.island.hidden_tabs.is_empty());
    }

    /// 岛运行时已有「打开应用」路径时，探测不能拿自动探测到的路径覆盖它。
    #[test]
    fn seed_keeps_runtime_open_path_over_detected() {
        let mut doc = SettingsDoc::default();
        let mut runtime = IslandConfig::default();
        runtime
            .notify_open
            .insert("mail".into(), r"D:\Runtime\thunderbird.exe".into());
        let changed = seed_with_overlay(
            &mut doc,
            &runtime,
            &[],
            |kind| Some(format!("D:/detected/{kind}.exe")),
            || None,
            || None,
        );
        assert!(changed, "其它空项仍要被填");
        assert_eq!(
            doc.notify.open.mail, r"D:\Runtime\thunderbird.exe",
            "岛运行时路径优先，探测不得覆盖"
        );
        assert_eq!(doc.notify.open.qq, "D:/detected/qq.exe");
        assert!(doc.notify.open.open_manual.is_empty(), "探测不写手动标记");
    }

    /// 自定义歌词接口：2048 上限 & 非法源回退默认。
    #[test]
    fn media_custom_url_is_capped_and_sources_fall_back() {
        let mut doc = SettingsDoc::default();
        doc.media.lyrics_sources = "  ,bogus, ".into();
        doc.media.lyrics_custom_url = format!("  https://x/{}  ", "a".repeat(3000));
        let doc = doc.clamped();
        assert_eq!(doc.media.lyrics_sources, "netease,lrclib,custom");
        assert_eq!(doc.media.lyrics_custom_url.chars().count(), 2048);
        assert!(doc.media.lyrics_custom_url.starts_with("https://x/"));
    }

    #[test]
    fn import_deep_merges_defaults_and_keeps_unknown_fields() {
        let raw = json!({
            "island": { "theme": "sky", "scale": 1.25 },
            "notify": { "toast_apps": { "QQ": { "name": "QQ", "kind": "qq" } } },
            "stocks": { "alerts": { "sh688256": { "above": 1500 } } },
            "launcher": [
                { "name": "Leet", "path": "leet.exe", "args": ["--fast"] }
            ],
            "weather": {
                "cities": [
                    { "name": "杭州", "lat": 30.27, "lon": 120.15, "country": "CN" }
                ]
            }
        });
        let doc = from_value(raw).expect("导入结构可解析");
        assert_eq!(doc.island.theme, "sky", "导入值覆盖默认");
        assert!((doc.island.pos - 0.5).abs() < 1e-9, "缺失项用默认值补齐");
        assert_eq!(doc.media.priority.len(), 6);
        assert_eq!(doc.island.extra["scale"], json!(1.25));
        assert_eq!(doc.notify.extra["toast_apps"]["QQ"]["kind"], json!("qq"));
        assert_eq!(doc.stocks.extra["alerts"]["sh688256"]["above"], json!(1500));
        assert_eq!(doc.launcher.len(), 1);
        assert_eq!(doc.launcher[0].extra["args"], json!(["--fast"]));
        assert_eq!(doc.weather.cities.len(), 1);
        assert_eq!(doc.weather.cities[0].extra["country"], json!("CN"));
        assert_eq!(doc.weather.cities[0].lat, Some(30.27));

        // 序列化回去仍在（深合并 + flatten 的完整往返）
        let round = serde_json::to_value(&doc).expect("可序列化");
        assert_eq!(round["island"]["scale"], json!(1.25));
        assert_eq!(round["notify"]["toast_apps"]["QQ"]["name"], json!("QQ"));
        assert_eq!(round["launcher"][0]["args"], json!(["--fast"]));
        assert_eq!(round["weather"]["cities"][0]["country"], json!("CN"));
        assert_eq!(round["island"]["theme"], json!("sky"));

        // 导入文件顶层不是对象要拒绝
        assert!(from_value(json!([1, 2, 3])).is_err());
    }

    /// 旧 `settings.json` 里可能留有已被删掉的顶层分节。`SettingsDoc` 用
    /// `#[serde(flatten)] extra` 收口未知字段，所以必须保证：未知分节不会让
    /// 加载失败，`clamped()` 后再序列化仍原样保留，且不干扰其它分节。
    #[test]
    fn unknown_top_level_section_survives_round_trip() {
        let raw = json!({
            "obsolete_section": { "legacy_key": "value", "flag": true },
            "translate": { "custom_enabled": true, "model": "deepseek-chat" },
            "island": { "theme": "rouge" },
        });
        let doc = from_value(raw).expect("含未知顶层分节的旧文档必须能加载");
        assert_eq!(doc.island.theme, "rouge");
        assert!(doc.translate.custom_enabled);
        assert_eq!(doc.translate.model, "deepseek-chat");

        // 保存链路 = save_to_path 的 clone + clamped + to_string_pretty，
        // 这里验证序列化结果：未知分节原样保留，其它分节解析出的值不受影响。
        let round = serde_json::to_value(doc.clamped()).expect("可序列化");
        assert_eq!(
            round["obsolete_section"],
            json!({ "legacy_key": "value", "flag": true })
        );
        assert_eq!(round["island"]["theme"], json!("rouge"));
        assert_eq!(round["translate"]["model"], json!("deepseek-chat"));
        assert!(round["hotkeys"].is_object(), "未涉及的默认分节仍在");
    }

    /// weather.cities 双向同步：文档 → 运行态只取 name/lat/lon；运行态 → 文档
    /// 保留同名旧条目的 extra（country 等），不会因为每次 overlay 被吃掉。
    #[test]
    fn weather_cities_sync_both_directions_and_keep_extra() {
        fn with_country() -> Extra {
            let mut map = extra();
            map.insert("country".into(), json!("CN"));
            map
        }

        // 文档 → IslandConfig
        let mut doc = SettingsDoc::default();
        doc.weather.cities = vec![WeatherCity {
            name: "杭州".into(),
            lat: Some(30.27),
            lon: Some(120.15),
            extra: with_country(),
        }];
        let runtime = weather_cities_from_doc(&doc.weather.cities);
        assert_eq!(
            runtime,
            vec![WeatherCityCfg {
                name: "杭州".into(),
                lat: Some(30.27),
                lon: Some(120.15),
            }]
        );

        // IslandConfig → 文档：同名条目保留 extra，坐标刷新
        let cfg = IslandConfig {
            weather_cities: vec![
                WeatherCityCfg {
                    name: "杭州".into(),
                    lat: Some(30.27),
                    lon: Some(120.15),
                },
                WeatherCityCfg {
                    name: "西安".into(),
                    lat: Some(34.34),
                    lon: Some(108.94),
                },
            ],
            ..IslandConfig::default()
        };
        let mut doc = SettingsDoc::default();
        doc.weather.cities = vec![WeatherCity {
            name: "杭州".into(),
            lat: Some(0.0),
            lon: Some(0.0),
            extra: with_country(),
        }];
        overlay_from_island(&mut doc, &cfg, &[]);
        assert_eq!(doc.weather.cities.len(), 2);
        assert_eq!(doc.weather.cities[0].name, "杭州");
        assert_eq!(doc.weather.cities[0].lat, Some(30.27));
        assert_eq!(doc.weather.cities[1].name, "西安");
        assert_eq!(
            doc.weather.cities[0].extra["country"],
            json!("CN"),
            "同名城市的额外字段不能在 overlay 里丢掉"
        );

        // 文档序列化后 weather.cities / country 都在（settings.json 权威）
        let raw = serde_json::to_value(&doc).expect("可序列化");
        assert_eq!(raw["weather"]["cities"][0]["name"], json!("杭州"));
        assert_eq!(raw["weather"]["cities"][0]["country"], json!("CN"));
        assert_eq!(raw["weather"]["cities"][1]["lat"], json!(34.34));
    }

    /// 设置页 patch 写入 weather.cities 后，文档可序列化、运行态可读到。
    #[test]
    fn weather_cities_patch_flows_into_runtime_values() {
        let patch: Map<String, Value> = serde_json::from_value(json!({
            "weather.city": "杭州",
            "weather.lat": 30.27,
            "weather.lon": 120.15,
            "weather.cities": [
                { "name": "杭州", "lat": 30.27, "lon": 120.15, "country": "CN" },
                { "name": "西安", "lat": null, "lon": null }
            ]
        }))
        .expect("patch 结构");
        let doc = patched_doc(SettingsDoc::default(), &patch).expect("patch 可用");
        assert_eq!(doc.weather.city, "杭州");
        assert_eq!(doc.weather.lat, Some(30.27));
        assert_eq!(doc.weather.cities.len(), 2);
        assert_eq!(doc.weather.cities[0].name, "杭州");
        assert_eq!(
            doc.weather.cities[0].extra["country"],
            json!("CN"),
            "settings.json 里的额外字段随 patch 保留"
        );
        let runtime = weather_cities_from_doc(&doc.weather.cities);
        assert_eq!(runtime.len(), 2);
        assert_eq!(runtime[0].name, "杭州");
        assert_eq!(runtime[0].lat, Some(30.27));
        assert_eq!(runtime[1].name, "西安");
        assert_eq!(runtime[1].lat, None);
    }

    /// 旧默认「上海」无坐标：读盘 / 导入时迁移成空（跟随 IP）；但设置页 /
    /// 绑定命令的 patch 路径不能误清显式绑定，否则「上海」无法绑定。
    #[test]
    fn weather_city_legacy_default_migrates_only_on_load() {
        assert_eq!(WeatherDoc::default().city, "");
        assert!(WeatherDoc::default().cities.is_empty());

        // 读盘 / 导入：旧默认值迁移为空
        let doc = from_value(json!({ "weather": { "city": "上海" } })).expect("旧文档可解析");
        assert_eq!(doc.weather.city, "", "旧默认「上海」且无坐标 → 跟随 IP");

        // patch 路径：显式绑定「上海」（坐标留空）不能被清掉
        let patch: Map<String, Value> = serde_json::from_value(json!({
            "weather.city": "上海",
            "weather.lat": null,
            "weather.lon": null,
        }))
        .expect("patch 结构");
        let doc = patched_doc(SettingsDoc::default(), &patch).expect("patch 可用");
        assert_eq!(doc.weather.city, "上海", "显式绑定不能被旧默认迁移清掉");
        assert_eq!(doc.weather.lat, None);

        // 带坐标的显式绑定（读盘路径）同样保留
        let doc = from_value(json!({
            "weather": { "city": "上海", "lat": 31.23, "lon": 121.47 }
        }))
        .expect("可解析");
        assert_eq!(doc.weather.city, "上海");
        assert_eq!(doc.weather.lat, Some(31.23));

        let mut doc = SettingsDoc::default();
        doc.weather.lat = Some(31.23);
        doc.weather.lon = Some(121.47);
        let doc = doc.clamped();
        assert_eq!(doc.weather.lat, None, "空主城市时坐标一起清空");
        assert_eq!(doc.weather.lon, None);
    }

    /// 只改城市名（坐标与运行态相同）时丢弃旧坐标；带新坐标 / 只改坐标时保留。
    #[test]
    fn weather_main_city_rename_drops_stale_coordinates() {
        let prev = IslandConfig {
            weather_city: "上海".into(),
            weather_lat: Some(31.2),
            weather_lon: Some(121.4),
            ..IslandConfig::default()
        };
        let mapped = |doc: &WeatherDoc| {
            let mut next = prev.clone();
            apply_weather_main_city(&mut next, &prev, doc);
            next
        };

        // 设置页只改城市名：坐标还是运行态旧值 → 清空，后续按城市名 geocode
        let mut doc = WeatherDoc::default();
        doc.city = "北京".into();
        doc.lat = Some(31.2);
        doc.lon = Some(121.4);
        let next = mapped(&doc);
        assert_eq!(next.weather_city, "北京");
        assert_eq!(next.weather_lat, None);
        assert_eq!(next.weather_lon, None);

        // 绑定命令带新坐标：保留
        let mut doc = WeatherDoc::default();
        doc.city = "北京".into();
        doc.lat = Some(39.90);
        doc.lon = Some(116.40);
        let next = mapped(&doc);
        assert_eq!(next.weather_city, "北京");
        assert_eq!(next.weather_lat, Some(39.90));
        assert_eq!(next.weather_lon, Some(116.40));

        // 城市没变、坐标变了：保留新坐标
        let mut doc = WeatherDoc::default();
        doc.city = "上海".into();
        doc.lat = Some(31.23);
        doc.lon = Some(121.47);
        let next = mapped(&doc);
        assert_eq!(next.weather_city, "上海");
        assert_eq!(next.weather_lat, Some(31.23));
        assert_eq!(next.weather_lon, Some(121.47));

        // 城市没变、坐标没变：原样保留
        let mut doc = WeatherDoc::default();
        doc.city = "上海".into();
        doc.lat = Some(31.2);
        doc.lon = Some(121.4);
        let next = mapped(&doc);
        assert_eq!(next.weather_city, "上海");
        assert_eq!(next.weather_lat, Some(31.2));
        assert_eq!(next.weather_lon, Some(121.4));
    }

    #[test]
    fn overlay_from_island_seeds_runtime_fields() {
        let cfg = IslandConfig {
            edge: "bottom".into(),
            pos_frac: 0.25,
            monitor: Some("\\\\.\\DISPLAY2".into()),
            theme: "sky".into(),
            font_family: "Microsoft YaHei".into(),
            font_size: 15.5,
            transparency: 65,
            expand_mode: "always".into(),
            compact_view: "clock".into(),
            auto_cycle_sec: 45,
            hover_expand: false,
            hover_expand_ms: 300,
            collapse_delay_ms: 500,
            blur_fps: 12,
            passthrough: true,
            hide_fullscreen: false,
            keep_on_top: false,
            hug_fullscreen: false,
            exclude_from_capture: true,
            hidden_tabs: vec!["clip".into(), "msg".into()],
            hotkey_toggle: "Ctrl+Alt+K".into(),
            hotkey_translate: "Ctrl+Alt+L".into(),
            hotkey_clipboard: "Ctrl+Alt+V".into(),
            notify_modules: vec!["qq".into()],
            notify_body_preview: vec![],
            stocks: vec!["sh600519".into()],
            stock_interval_sec: 10,
            stock_alert_pct: 5.5,
            weather_city: "杭州".into(),
            weather_lat: Some(30.27),
            weather_lon: Some(120.15),
            launcher: vec![LauncherItem {
                name: "资源管理器".into(),
                path: "explorer.exe".into(),
                icon: "folder".into(),
            }],
            agent_order: vec!["dsh".into()],
            ..IslandConfig::default()
        };
        let monitors = vec![monitor(0, "\\\\.\\DISPLAY1", true), monitor(1, "\\\\.\\DISPLAY2", false)];
        let mut doc = SettingsDoc::default();
        // 默认启动器第 0 项与 cfg 同名同理，这里验证 extra 保留
        doc.launcher[0].extra.insert("args".into(), json!(["--x"]));
        overlay_from_island(&mut doc, &cfg, &monitors);

        assert_eq!(doc.island.edge, "bottom");
        assert!((doc.island.pos - 0.25).abs() < 1e-9);
        assert_eq!(doc.island.screen, 1, "monitor 名映射到稳定索引");
        assert_eq!(doc.island.theme, "sky");
        assert_eq!(doc.island.font_family, "Microsoft YaHei");
        assert_eq!(doc.island.font_size, 15.5);
        assert_eq!(doc.island.transparency, 65);
        assert_eq!(doc.island.expand_mode, "always");
        assert_eq!(doc.island.compact_view, "clock");
        assert_eq!(doc.island.auto_cycle_s, 45);
        assert!(!doc.island.hover_expand);
        assert_eq!(doc.island.hover_expand_ms, 300);
        assert_eq!(doc.island.collapse_delay_ms, 500);
        assert_eq!(doc.island.blur_fps, 12);
        assert!(doc.island.passthrough);
        assert!(!doc.island.hide_fullscreen);
        assert!(!doc.island.keep_on_top);
        assert!(!doc.island.hug_fullscreen);
        assert!(doc.island.exclude_from_capture);
        assert_eq!(doc.island.hidden_tabs, vec!["clip", "msg"]);
        assert_eq!(doc.hotkeys.toggle, "Ctrl+Alt+K");
        assert_eq!(doc.hotkeys.translate, "Ctrl+Alt+L");
        assert_eq!(doc.notify.modules, vec!["qq"]);
        assert!(doc.notify.body_preview.is_empty());
        assert_eq!(doc.stocks.watchlist, vec!["sh600519"]);
        assert_eq!(doc.stocks.interval, 10);
        assert!((doc.stocks.pct_alert - 5.5).abs() < 1e-9);
        assert_eq!(doc.weather.city, "杭州");
        assert_eq!(doc.weather.lat, Some(30.27));
        assert_eq!(doc.launcher.len(), 1);
        assert_eq!(doc.launcher[0].icon.as_deref(), Some("folder"));
        assert_eq!(
            doc.launcher[0].extra["args"],
            json!(["--x"]),
            "启动器额外字段不能在双向同步里被吃掉"
        );
        assert_eq!(doc.agenthud.agent_order, vec!["dsh"]);

        // 主屏回退：monitor 为 None 时 screen 指向 primary
        let fallback = IslandConfig::default();
        let mut doc = SettingsDoc::default();
        overlay_from_island(&mut doc, &fallback, &monitors);
        assert_eq!(doc.island.screen, 0);
    }

    #[test]
    fn launcher_defaults_only_fill_missing_configuration() {
        assert_eq!(IslandConfig::default().launcher.len(), 5);
        assert_eq!(from_value(json!({})).unwrap().launcher.len(), 5);

        let runtime: IslandConfig = serde_json::from_value(json!({"launcher": []})).unwrap();
        let runtime = runtime.clamped();
        assert!(runtime.launcher.is_empty());
        assert!(from_value(json!({"launcher": []})).unwrap().launcher.is_empty());

        let mut doc = SettingsDoc::default();
        overlay_from_island(&mut doc, &runtime, &[]);
        assert!(doc.launcher.is_empty());
    }

    #[test]
    fn launcher_preserves_surviving_entry_metadata_after_removal() {
        let mut doc = SettingsDoc::default();
        doc.launcher[1].extra.insert("args".into(), json!(["--example"]));
        let mut runtime = IslandConfig::default();
        runtime.launcher.remove(0);

        overlay_from_island(&mut doc, &runtime, &[]);
        assert_eq!(doc.launcher[0].name, runtime.launcher[0].name);
        assert_eq!(doc.launcher[0].extra["args"], json!(["--example"]));
    }

    #[test]
    fn applied_paths_match_synced_subset() {
        let keys = vec![
            "island.edge".to_string(),
            "notify.popup.enabled".to_string(),
            "launcher".to_string(),
            "media.lyrics".to_string(),
            "media.netease_log".to_string(),
            "media.playback_logs".to_string(),
            "notify.open.qq".to_string(),
            "clipboard.max".to_string(),
            "brightness.twinkle".to_string(),
        ];
        assert_eq!(
            applied_paths(&keys),
            vec![
                "island.edge".to_string(),
                "media.lyrics".to_string(),
                "media.netease_log".to_string(),
                "media.playback_logs".to_string(),
                // 桌面浮窗三项已接线（island::popup 消费），必须算 applied
                "notify.popup.enabled".to_string(),
                "notify.open.qq".to_string(),
                "launcher".to_string(),
                "clipboard.max".to_string(),
                // 亮度 Twinkle 路径已由 island::system 消费，必须算 applied
                "brightness.twinkle".to_string()
            ]
        );
        // 整块对象 patch 能覆盖其下所有已接线子路径
        let keys = vec!["island".to_string()];
        let applied = applied_paths(&keys);
        assert!(applied.contains(&"island.edge".to_string()));
        assert!(applied.contains(&"island.transparency".to_string()));
        assert!(applied.contains(&"island.font_family".to_string()));
        assert!(applied.contains(&"island.blur_fps".to_string()));
        assert!(applied.contains(&"island.hover_expand".to_string()));
        // media / notify.open 整块 patch 同样覆盖其子路径
        let applied = applied_paths(&["media".into(), "notify".into(), "dsh".into(), "mail".into()]);
        for path in [
            "media.auto_show",
            "media.lyrics",
            "media.netease_log",
            "media.playback_logs",
            "media.lyrics_sources",
            "media.lyrics_custom_url",
            "media.priority",
            "notify.open.qq",
            "notify.open.dsh",
            "dsh.enabled",
            "dsh.sessions_dir",
            "dsh.include_subagents",
            "dsh.skip_when_active",
            "dsh.body_chars",
            "mail.interval_s",
            "mail.imap",
        ] {
            assert!(applied.contains(&path.to_string()), "缺 {path}");
        }
        // live 整块 patch：16 个点路径都已接线（island::live 消费）
        let live = applied_paths(&["live".to_string()]);
        for path in [
            "live.models_dir",
            "live.audio.source",
            "live.audio.device",
            "live.vad.threshold",
            "live.vad.min_silence_ms",
            "live.vad.min_speech_ms",
            "live.asr.partial",
            "live.asr.draft_interval_ms",
            "live.asr.final_threads",
            "live.asr.max_segment_s",
            "live.asr.context_prompt",
            "live.translate.enabled",
            "live.translate.live",
            "live.translate.live_min_interval_ms",
            "live.translate.live_min_new_chars",
            "live.translate.context_lines",
        ] {
            assert!(live.contains(&path.to_string()), "live 缺 {path}");
        }
        assert_eq!(live.len(), 16, "live 只应有 16 个已接线点路径");
    }

    #[test]
    fn synced_and_unsupported_paths_are_disjoint() {
        // 不变式：一个点路径不能既算「已接线」又标「未接线」（前端两种标记互斥）。
        let unsupported = unsupported();
        for path in SYNCED_PATHS {
            assert!(
                !unsupported.iter().any(|item| item == path),
                "{path} 同时出现在 SYNCED_PATHS 与 unsupported"
            );
        }
        let applied = all_applied_paths();
        for path in &unsupported {
            assert!(
                !applied.contains(path),
                "{path} 已接线，不应再标未接线"
            );
        }
    }

    #[test]
    fn unsupported_lists_only_unwired_paths() {
        let list = unsupported();
        assert_eq!(
            list.iter().any(|path| path == "island.hug_fullscreen"),
            !cfg!(target_os = "windows"),
        );
        for path in [
            "media.netease_log",
            "media.playback_logs",
            "dsh.skip_when_active",
            "clash.config_dir",
            "clash.proxy_port",
            "clash.app",
        ] {
            assert_eq!(list.iter().any(|item| item == path), !cfg!(windows));
        }
        for path in [
            "live.models_dir",
            "live.audio.source",
            "live.audio.device",
            "live.vad.threshold",
            "live.vad.min_silence_ms",
            "live.vad.min_speech_ms",
            "live.asr.partial",
            "live.asr.draft_interval_ms",
            "live.asr.final_threads",
            "live.asr.max_segment_s",
            "live.asr.context_prompt",
            "live.translate.enabled",
            "live.translate.live",
            "live.translate.live_min_interval_ms",
            "live.translate.live_min_new_chars",
            "live.translate.context_lines",
        ] {
            assert!(
                !list.iter().any(|item| item == path),
                "{path} 已由 island::live 消费，不应出现在 unsupported"
            );
        }
        // 已接线的不要列（翻译命令与翻译热键会直接消费这些键）
        for path in [
            "island.edge",
            "island.pos",
            "island.screen",
            "island.theme",
            "island.transparency",
            "island.font_size",
            "island.font_family",
            "island.blur_fps",
            "island.hover_expand",
            "island.keep_on_top",
            "hotkeys.toggle",
            "hotkeys.translate",
            "hotkeys.clipboard",
            "translate.custom_enabled",
            "translate.platform",
            "translate.platform_model",
            "translate.api_key",
            "translate.base_url",
            "translate.model",
            "translate.import_from",
            "media.auto_show",
            "media.lyrics",
            "media.lyrics_sources",
            "media.lyrics_custom_url",
            "media.priority",
            "notify.modules",
            "notify.body_preview",
            // 桌面浮窗三项已由 island::popup 消费，不能再标「未接入」
            "notify.popup.enabled",
            "notify.popup.position",
            "notify.popup.size",
            "notify.open.qq",
            "notify.open.wechat",
            "notify.open.mail",
            "notify.open.feishu",
            "notify.open.dingtalk",
            "notify.open.dsh",
            "dsh.enabled",
            "dsh.sessions_dir",
            "dsh.include_subagents",
            "dsh.body_chars",
            "mail.interval_s",
            "mail.imap",
            "stocks.watchlist",
            "stocks.interval",
            "weather.api_key",
            "weather.city",
            "weather.lat",
            "weather.lon",
            "weather.units",
            "weather.interval_min",
            "weather.cities",
            "launcher",
            "agenthud.agent_order",
            "clipboard.max",
        ] {
            assert!(
                !list.iter().any(|item| item == path),
                "{path} 已接线，不应出现在 unsupported"
            );
        }
        // Windows 才有 WDA_EXCLUDEFROMCAPTURE；其它平台应标未接线。
        assert_eq!(
            list.iter().any(|item| item == "island.exclude_from_capture"),
            !cfg!(target_os = "windows")
        );
        // 亮度 Twinkle 命令行只有 Windows 分支消费；其它平台仍标未接线。
        assert_eq!(
            list.iter().any(|item| item == "brightness.twinkle"),
            !cfg!(target_os = "windows")
        );
    }

    /// Windows 字体列表：注册表路径、去重排序与中文字体启发式都要能跑通。
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_font_registry_list_is_sorted() {
        let fonts = read_installed_fonts();
        assert!(fonts.len() > 10, "字体库条目过少: {}", fonts.len());
        assert!(
            fonts.iter().any(|entry| entry.has_chinese),
            "应能按关键字识别出中文字体"
        );
        assert!(
            fonts
                .windows(2)
                .all(|pair| pair[0].family.to_lowercase() <= pair[1].family.to_lowercase()),
            "字体列表必须按名称排序"
        );
        assert!(
            fonts.iter().all(|entry| !entry.family.starts_with('@')),
            "竖排 @ 前缀字体应被去掉"
        );
    }

    /// 注册表值名解析：`&` 组合拆分、竖排 `@` 前缀与格式后缀剥离。
    #[cfg(target_os = "windows")]
    #[test]
    fn font_registry_value_parses_family_names() {
        assert_eq!(
            font_families_from_registry_value("微软雅黑 & Microsoft YaHei UI (TrueType)"),
            vec!["微软雅黑".to_string(), "Microsoft YaHei UI".to_string()]
        );
        assert_eq!(
            font_families_from_registry_value("@宋体 (OpenType)"),
            vec!["宋体".to_string()]
        );
        assert_eq!(
            font_families_from_registry_value("优设鲨鱼菲特健康体 (TrueType)"),
            vec!["优设鲨鱼菲特健康体".to_string()]
        );
        assert_eq!(
            font_families_from_registry_value("Arial(All res)"),
            vec!["Arial".to_string()]
        );
    }

    /// 多来源合并：同一 family 大小写不同只保留首次出现的写法。
    #[cfg(target_os = "windows")]
    #[test]
    fn normalize_font_families_dedups_across_sources() {
        let families = normalize_font_families(vec![
            "Microsoft YaHei (TrueType)".to_string(),
            "microsoft yahei".to_string(),
        ]);
        assert_eq!(families, vec!["Microsoft YaHei".to_string()]);
    }

    /// 合并结果按 lowercase 非降序排序。
    #[cfg(target_os = "windows")]
    #[test]
    fn normalize_font_families_sorts_by_lowercase() {
        let families = normalize_font_families(vec![
            "Zebra (TrueType)".to_string(),
            "apple (TrueType)".to_string(),
            "Banana & Cherry (TrueType)".to_string(),
        ]);
        assert_eq!(
            families,
            vec![
                "apple".to_string(),
                "Banana".to_string(),
                "Cherry".to_string(),
                "Zebra".to_string()
            ]
        );
        assert!(families
            .windows(2)
            .all(|pair| pair[0].to_lowercase() <= pair[1].to_lowercase()));
    }

    /// 中文字体启发式：CJK 字符与厂商关键字都算，纯拉丁名不算。
    #[cfg(target_os = "windows")]
    #[test]
    fn looks_chinese_font_recognizes_cjk_names() {
        assert!(
            looks_chinese_font("优设鲨鱼菲特健康体"),
            "中文命名字体应判定为含中文"
        );
        assert!(
            looks_chinese_font("HarmonyOS Sans SC"),
            "HarmonyOS Sans 关键字应命中"
        );
        assert!(!looks_chinese_font("Arial"), "Arial 不应判定为含中文");
    }
}
