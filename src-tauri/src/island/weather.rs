//! 灵动岛天气：多城市当前天气 + 12 小时 + 6 天。
//!
//! 数据源：设置文档 `weather.api_key` 非空时走 OpenWeather（current + 3 小时
//! 预报，`units` / `lang=zh_cn` 生效），网络或 Key 失败自动回退同城 Open-Meteo；
//! 留空则只用免费的 Open-Meteo（`units=imperial` 时传 fahrenheit / mph）。
//!
//! 主城市优先级：绑定城市（`weather.city` 非空）> 公网 IP 定位城市 > 自定义
//! 城市列表第一个 > 无。显示列表 = [主城市] +（绑定时）[IP 城市，若与主城市
//! 不同] + 其余自定义城市（按 `weather.cities` 顺序、去重）。顶层字段始终等于
//! 主城市详情，收起态继续只读顶层字段，不受多城市改动影响。
//!
//! 线程模型：启用时启动一条后台线程，立即取一拍后按 `weather.interval_min`
//! （5–180 分钟，默认 15）刷新；另每 5 分钟重新定位一次公网 IP，城市变化时
//! 自动整拍刷新。数据源参数与刷新间隔在每拍发起取数时读一次 settings.json
//! （防重入早退之前就读，保证保存设置后的新间隔立即生效），500ms 循环只读
//! 内存里的间隔原子量，不反复读盘。前端主动刷新走 `refresh()`（置 `FORCE` +
//! spawn 一次性线程，`FETCHING` 防重入）：一旦被正在跑的取数挡掉，后台循环
//! 下一拍（≤500ms）凭 `FORCE` 补取，不会丢弃。数据变化/刷新完成通过
//! `zbar://island-weather` 广播。IP 定位与 geocode 结果都有进程内
//! 缓存，避免巡检与取数的重复请求。

use serde::Serialize;
use std::collections::HashMap;

// 复用 codex 的系统代理探测（Windows 注册表 / macOS scutil）。天气数据请求经
// shared_agent() 挂系统代理，确保 Clash 等只设系统代理的环境也能命中；IP 定位
// 必须直连（否则按代理出口 IP 定位，拿到的是代理出口位置），走 direct_agent()。
use crate::codex::system_proxy;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use super::{IslandConfig, WeatherCityCfg};

pub const WEATHER_EVENT: &str = "zbar://island-weather";
/// 默认刷新间隔（秒）：设置文档缺失 / 首次启动时用；实际值见 `INTERVAL_SECS`。
const REFRESH_SECS: u64 = 15 * 60;
/// `weather.interval_min` 的合法范围（与 settings.rs clamp 同口径）。
const MIN_REFRESH_MIN: i64 = 5;
const MAX_REFRESH_MIN: i64 = 180;
const IP_CHECK_SECS: u64 = 5 * 60;
const GEO_CACHE_MAX: usize = 64;

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static FETCHING: AtomicBool = AtomicBool::new(false);
/// 有一次主动刷新（`refresh()`）还没兑现：撞上正在跑的取数被防重入挡掉时保持
/// true，后台循环下一拍补取；只有真正抢到取数权的那次取数才清零。
static FORCE: AtomicBool = AtomicBool::new(false);
/// 当前生效的刷新间隔（秒）：每次发起取数时（含被防重入挡掉的那次）用读到的
/// `weather.interval_min` 更新，线程循环只读这个原子量，避免 500ms 一拍地读
/// settings.json。
static INTERVAL_SECS: AtomicU64 = AtomicU64::new(REFRESH_SECS);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static CACHE: OnceLock<Mutex<Option<WeatherState>>> = OnceLock::new();
/// 每座城市最近一次取数成功的快照（进程内，city_key → 快照 + 时间戳）。
/// 请求失败时回填它并标 `stale`，让页面展示最近一次成功数据而不是整页红错。
static SNAPSHOTS: OnceLock<Mutex<HashMap<String, WeatherCityState>>> = OnceLock::new();
static GEO_CACHE: OnceLock<Mutex<HashMap<String, GeoHit>>> = OnceLock::new();
/// 最近一次广播用的 IP 定位结果（`None` = 定位失败或还没跑过），
/// 5 分钟 IP 巡检用它判断城市是否变化。
static LAST_IP: OnceLock<Mutex<Option<IpSnapshot>>> = OnceLock::new();

fn app_slot() -> &'static Mutex<Option<AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
}

fn cache_slot() -> &'static Mutex<Option<WeatherState>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn snapshots() -> &'static Mutex<HashMap<String, WeatherCityState>> {
    SNAPSHOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn geo_cache() -> &'static Mutex<HashMap<String, GeoHit>> {
    GEO_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn last_ip_slot() -> &'static Mutex<Option<IpSnapshot>> {
    LAST_IP.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WeatherHour {
    pub time: String,
    pub label: String,
    pub temp: f64,
    pub code: i32,
    pub icon: String,
    pub desc: String,
    pub precip: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WeatherDay {
    pub date: String,
    pub weekday: String,
    pub code: i32,
    pub icon: String,
    pub desc: String,
    pub max: f64,
    pub min: f64,
}

/// 显示列表里的单个城市：`cities` 的元素，字段与主城市顶层字段同口径。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WeatherCityState {
    /// 用于 geocode 的市名（IP 城市同样取定位返回的 city）。
    pub name: String,
    /// 显示名：IP 城市用「省 · 市」，绑定 / 自定义城市用名字。
    pub label: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub is_main: bool,
    pub is_ip: bool,
    pub is_user: bool,
    pub ok: bool,
    pub error: String,
    /// 本次取数失败但已回填最近成功快照：true 表示展示的是旧数据。
    pub stale: bool,
    /// 简短错误摘要（失败时用于顶部小提示；成功时为空）。完整错误仍在 `error`。
    pub error_brief: String,
    pub updated_at: i64,
    pub temp: f64,
    pub feels_like: f64,
    pub humidity: f64,
    pub wind: f64,
    pub code: i32,
    pub desc: String,
    pub icon: String,
    pub is_day: bool,
    pub sunrise: String,
    pub sunset: String,
    pub hourly: Vec<WeatherHour>,
    pub daily: Vec<WeatherDay>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WeatherState {
    pub ok: bool,
    pub error: String,
    /// 主城市本次取数失败但已回填最近成功快照：true 表示展示的是旧数据。
    pub stale: bool,
    /// 简短错误摘要（失败时用于顶部小提示；成功时为空）。
    pub error_brief: String,
    pub city: String,
    pub updated_at: i64,
    pub temp: f64,
    pub feels_like: f64,
    pub humidity: f64,
    pub wind: f64,
    pub code: i32,
    pub desc: String,
    pub icon: String,
    pub is_day: bool,
    pub sunrise: String,
    pub sunset: String,
    pub hourly: Vec<WeatherHour>,
    pub daily: Vec<WeatherDay>,
    /// 显示列表：[主城市] +（绑定时）[IP 城市] + 其余自定义城市。
    pub cities: Vec<WeatherCityState>,
    /// 主城市在 `cities` 里的下标；无主城市 = -1。
    pub main_index: i64,
    /// IP 城市在 `cities` 里的下标；主城市就是 IP 城市时为 None。
    pub ip_index: Option<usize>,
    /// 没有绑定城市 = 跟随 IP。
    pub follows_ip: bool,
    /// 温度单位：`metric` / `imperial`，由本次取数的数据源实际单位决定
    /// （OpenWeather 随 `units` 参数，Open-Meteo 随 temperature_unit）。
    pub units: String,
}

/// 一次取数统一使用的数据源参数：api_key 非空 → OpenWeather，否则 Open-Meteo。
/// 只在「该取数」的这一拍从 settings.json 读一次，随参数传入各城市取数。
#[derive(Debug, Clone)]
struct WeatherProvider {
    api_key: String,
    units: String,
    /// `weather.interval_min`（由调用方 clamp 后换算成线程刷新间隔）。
    interval_min: i64,
}

impl WeatherProvider {
    fn from_settings() -> Self {
        let weather = super::settings::load().weather;
        Self {
            api_key: weather.api_key.trim().to_string(),
            units: if weather.units == "imperial" {
                "imperial".to_string()
            } else {
                "metric".to_string()
            },
            interval_min: weather.interval_min,
        }
    }

    fn uses_openweather(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

/// `weather.interval_min`（分钟）→ 线程刷新间隔（秒）；与设置文档 clamp 同口径。
fn refresh_secs(interval_min: i64) -> u64 {
    interval_min.clamp(MIN_REFRESH_MIN, MAX_REFRESH_MIN) as u64 * 60
}

fn wmo(code: i32, day: bool) -> (String, &'static str) {
    let (desc, icon) = match code {
        0 => ("晴", if day { "sun" } else { "moon" }),
        1 => ("大部晴朗", if day { "sun" } else { "moon" }),
        2 => ("多云", "partly"),
        3 => ("阴", "cloudy"),
        45 | 48 => ("雾", "fog"),
        51 | 53 | 55 => ("毛毛雨", "drizzle"),
        56 | 57 => ("冻毛毛雨", "drizzle"),
        61 => ("小雨", "rain"),
        63 => ("中雨", "rain"),
        65 => ("大雨", "rain"),
        66 | 67 => ("冻雨", "rain"),
        71 => ("小雪", "snow"),
        73 => ("中雪", "snow"),
        75 => ("大雪", "snow"),
        77 => ("米雪", "snow"),
        80 => ("阵雨", "rain"),
        81 => ("强阵雨", "rain"),
        82 => ("暴雨", "rain"),
        85 => ("阵雪", "snow"),
        86 => ("强阵雪", "snow"),
        95 => ("雷阵雨", "thunder"),
        96 | 99 => ("雷暴冰雹", "storm"),
        _ => ("未知", "cloudy"),
    };
    (desc.to_string(), icon)
}

fn weekday_zh(date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|d| {
            use chrono::Datelike;
            ["周一", "周二", "周三", "周四", "周五", "周六", "周日"]
                [d.weekday().num_days_from_monday() as usize]
                .to_string()
        })
        .unwrap_or_default()
}

fn num(v: &serde_json::Value, key: &str) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0)
}
fn int(v: &serde_json::Value, key: &str) -> i32 {
    v.get(key).and_then(|x| x.as_i64()).unwrap_or(0) as i32
}
fn arr<'a>(v: &'a serde_json::Value, key: &str) -> &'a [serde_json::Value] {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

/// 把展示用市名转成 Open-Meteo 能命中的查询名：
/// “杭州市” → “杭州”，“阿坝藏族羌族自治州” → “阿坝藏族羌族”（完整名不命中时
/// 才去掉行政后缀；先试原名可避免误伤本身带“市”的海外地名）。
fn geocode_query(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut out = trimmed.to_string();
    for suffix in ["自治州", "地区", "市", "省", "县", "区"] {
        if out.ends_with(suffix) && out.chars().count() > suffix.chars().count() + 1 {
            out.truncate(out.len() - suffix.len());
            break;
        }
    }
    if out.is_empty() {
        trimmed.to_string()
    } else {
        out
    }
}

/// 逆地理查询：按公网出口 IP 取当前位置。优先 ip-api（中文），失败回退
/// ipwho.is（同样带中文参数；某些线路下仍可能返回英文名，由 `localize_place`
/// 再用 geocoding（language=zh）反查中文名）。前端用它定位到三级联动里对应的
/// 城市，同时保留经纬度，省掉一次 Open-Meteo geocoding。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocatedPlace {
    pub ok: bool,
    pub error: String,
    pub country: String,
    pub province: String,
    pub city: String,
    pub district: String,
    pub lat: f64,
    pub lon: f64,
    pub label: String,
}

/// 天气数据（Open-Meteo forecast/geocoding、OpenWeather）出站请求共用的 ureq
/// agent 构建：统一挂系统代理，失败静默回落直连。
///
/// ureq 2.x 默认只读环境变量代理（`proxy-from-env`），不读 Windows 系统代理；
/// 用户开着 Clash Verge 但只设了系统代理时，`api.open-meteo.com` 会直连超时。
/// 这里在 `AgentBuilder::new()` 基础上补一次系统代理探测：`system_proxy()` 返回
/// 了 URL 就 `Proxy::new` 挂上，解析失败（配置异常）则不带 proxy 走 ureq 默认
/// 行为（环境变量代理或直连），绝不因代理配置异常让天气模块崩掉。
fn shared_agent(timeout: Duration) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new().timeout(timeout);
    if let Some(url) = system_proxy() {
        match ureq::Proxy::new(&url) {
            Ok(proxy) => builder = builder.proxy(proxy),
            Err(error) => eprintln!("[zbar-island-weather] 系统代理「{url}」解析失败，回落直连: {error}"),
        }
    }
    builder.build()
}

/// IP 定位（ip-api / ipwho.is）专用的直连 agent：刻意不挂系统代理。
///
/// 定位服务按「公网出口 IP」返回地理位置，国内可直连；只有直连才能拿到用户
/// 真实位置（如西安）。一旦走代理，接口看到的是代理出口 IP，会定位到代理
/// 出口所在地（实测东京），属于功能语义错误。`try_proxy_from_env(false)`
/// 显式关闭环境变量代理，避免将来启用 ureq 的 `proxy-from-env` feature 后
/// 又悄悄把定位请求挂上代理。天气数据请求仍走 `shared_agent`（带代理）。
fn direct_agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .try_proxy_from_env(false)
        .build()
}

fn location_json(url: &str) -> Result<serde_json::Value, String> {
    let agent = direct_agent(Duration::from_secs(10));
    let resp = agent
        .get(url)
        .set("User-Agent", "AgentHUD-Island/1.0")
        .call()
        .map_err(|e| format!("定位请求失败: {e}"))?;
    let text = resp
        .into_string()
        .map_err(|e| format!("读取定位响应失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("解析定位响应失败: {e}"))
}

fn locate_place() -> Result<LocatedPlace, String> {
    if let Ok(v) = location_json("http://ip-api.com/json/?lang=zh-CN") {
        if v.get("status").and_then(|s| s.as_str()) == Some("success") {
            let province = v
                .get("regionName")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let city = v
                .get("city")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let lat = num(&v, "lat");
            let lon = num(&v, "lon");
            if !city.is_empty() && lat.is_finite() && lon.is_finite() {
                return Ok(localize_place(LocatedPlace {
                    ok: true,
                    country: v
                        .get("country")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                    label: if province.is_empty() {
                        city.clone()
                    } else {
                        format!("{province} · {city}")
                    },
                    province,
                    city,
                    district: String::new(),
                    lat,
                    lon,
                    ..Default::default()
                }));
            }
        }
    }
    // ip-api 走明文 HTTP，容易被代理 / 网络策略阻断；回退 ipwho.is 时必须带
    // `lang=zh-CN`，否则接口返回英文城市名（这正是中文环境下显示英文名的根因）。
    let v = location_json("https://ipwho.is/?lang=zh-CN")?;
    if v.get("success").and_then(|s| s.as_bool()) == Some(false) {
        return Err(v
            .get("message")
            .and_then(|s| s.as_str())
            .unwrap_or("定位失败")
            .to_string());
    }
    let province = v
        .get("region")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let city = v
        .get("city")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let lat = num(&v, "latitude");
    let lon = num(&v, "longitude");
    if city.is_empty() || !lat.is_finite() || !lon.is_finite() {
        return Err("定位服务没有返回有效城市".into());
    }
    Ok(localize_place(LocatedPlace {
        ok: true,
        country: v
            .get("country")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        label: if province.is_empty() {
            city.clone()
        } else {
            format!("{province} · {city}")
        },
        province,
        city,
        district: String::new(),
        lat,
        lon,
        ..Default::default()
    }))
}

#[tauri::command]
pub async fn island_locate() -> Result<LocatedPlace, String> {
    tauri::async_runtime::spawn_blocking(locate_place)
        .await
        .map_err(|e| format!("定位任务失败: {e}"))?
}

/// 城市选择器「使用当前位置」：手动触发一次 IP 定位（复用后台巡检同一
/// `locate_place`），返回含 province / city / lat / lon 的定位结果供前端直加。
#[tauri::command]
pub async fn weather_locate_now() -> Result<LocatedPlace, String> {
    tauri::async_runtime::spawn_blocking(locate_place)
        .await
        .map_err(|e| format!("定位任务失败: {e}"))?
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 带 HTTP 状态码的请求错误：OpenWeather 的 Key 无效（401）/ 限流（429）
/// 要能与网络错误区分，日志和错误文案才有意义。
#[derive(Debug, Clone)]
struct HttpError {
    status: Option<u16>,
    reason: String,
}

impl HttpError {
    /// OpenWeather 失败原因（不含「天气请求」前缀，由调用方拼上下文）。
    fn weather_error(&self) -> String {
        match self.status {
            Some(401) => "API Key 无效（HTTP 401）".to_string(),
            Some(429) => "请求过于频繁（HTTP 429）".to_string(),
            Some(code) => format!("HTTP {code}"),
            None => self.reason.clone(),
        }
    }
}

fn http_json_detailed(url: &str) -> Result<serde_json::Value, HttpError> {
    let agent = shared_agent(Duration::from_secs(12));
    let resp = agent
        .get(url)
        .set("User-Agent", "AgentHUD-Island/1.0")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => HttpError {
                status: Some(code),
                reason: format!("天气请求失败: HTTP {code}"),
            },
            other => HttpError {
                status: None,
                reason: format!("天气请求失败: {other}"),
            },
        })?;
    let text = resp.into_string().map_err(|e| HttpError {
        status: None,
        reason: format!("读取天气响应失败: {e}"),
    })?;
    serde_json::from_str(&text).map_err(|e| HttpError {
        status: None,
        reason: format!("解析天气响应失败: {e}"),
    })
}

fn http_json(url: &str) -> Result<serde_json::Value, String> {
    http_json_detailed(url).map_err(|error| error.reason)
}

// ============================================================
// geocode（进程内缓存）
// ============================================================

#[derive(Debug, Clone)]
struct GeoHit {
    name: String,
    lat: f64,
    lon: f64,
    /// 一级行政区（geocoding 的 admin1，如「浙江省」），用于中文展示名。
    admin1: String,
    /// 国家（geocoding 的 country，如「中国」），用于中文展示名。
    country: String,
}

fn cached_geocode(query: &str) -> Option<GeoHit> {
    geo_cache()
        .lock()
        .ok()?
        .get(&query.trim().to_lowercase())
        .cloned()
}

fn cache_geocode(query: &str, hit: GeoHit) {
    if let Ok(mut guard) = geo_cache().lock() {
        if guard.len() >= GEO_CACHE_MAX {
            guard.clear();
        }
        guard.insert(query.trim().to_lowercase(), hit);
    }
}

/// 按城市名 geocode（Open-Meteo geocoding）。会先试原名，再试去掉行政后缀
/// 的形态；结果按查询名缓存，避免 5 分钟巡检反复打接口。
fn geocode_lookup(query: &str) -> Result<GeoHit, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("城市名为空".into());
    }
    if let Some(hit) = cached_geocode(query) {
        return Ok(hit);
    }
    let stripped = geocode_query(query);
    let mut attempts = vec![query.to_string()];
    if !stripped.is_empty() && stripped != query {
        attempts.push(stripped);
    }
    let mut last_err = String::new();
    for attempt in attempts {
        let url = format!(
            "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=zh&format=json",
            encode(&attempt)
        );
        match http_json(&url) {
            Ok(geo) => {
                if let Some(first) = geo
                    .get("results")
                    .and_then(|r| r.as_array())
                    .and_then(|r| r.first())
                {
                    let lat = num(first, "latitude");
                    let lon = num(first, "longitude");
                    if lat.is_finite() && lon.is_finite() {
                        let field = |key: &str| {
                            first
                                .get(key)
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .unwrap_or("")
                                .to_string()
                        };
                        let name = first
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|v| !v.is_empty())
                            .unwrap_or(query)
                            .to_string();
                        let hit = GeoHit {
                            name,
                            lat,
                            lon,
                            admin1: field("admin1"),
                            country: field("country"),
                        };
                        cache_geocode(query, hit.clone());
                        return Ok(hit);
                    }
                }
            }
            Err(error) => last_err = error,
        }
    }
    Err(if last_err.is_empty() {
        format!("找不到城市「{query}」")
    } else {
        format!("找不到城市「{query}」: {last_err}")
    })
}

/// 城市坐标：已有合法坐标直接用，缺坐标才 geocode。
fn resolve_coords(name: &str, lat: Option<f64>, lon: Option<f64>) -> Result<(f64, f64), String> {
    if let (Some(lat), Some(lon)) = super::normalize_weather_coord_pair(lat, lon) {
        return Ok((lat, lon));
    }
    let hit = geocode_lookup(name)?;
    Ok((hit.lat, hit.lon))
}

// ============================================================
// 中文城市名修复（IP 定位回退到英文名时反查中文）
// ============================================================

/// 纯 ASCII 名称（ipwho.is 在部分线路下仍返回英文城市名）。
fn is_ascii_text(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.is_ascii()
}

/// 名称里是否含 CJK 汉字（判断反查结果是否已经是中文名）。
fn has_cjk(value: &str) -> bool {
    value
        .chars()
        .any(|c| matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF))
}

/// geocoding 的 admin1 常见全称（「浙江省」）；展示用去掉「省」后缀，与
/// ip-api 的 regionName（「浙江」）风格一致。
fn short_admin1(admin1: &str) -> String {
    let trimmed = admin1.trim();
    trimmed
        .strip_suffix('省')
        .filter(|rest| !rest.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// 定位结果本地化的纯判定：city / province 是纯 ASCII 且反查命中了中文名时，
/// 返回新的 (city, province, country, label)；否则 None（保留英文，不阻断取数）。
fn localized_place_names(
    place: &LocatedPlace,
    hit: &GeoHit,
) -> Option<(String, String, String, String)> {
    if !is_ascii_text(&place.city) && !is_ascii_text(&place.province) {
        return None;
    }
    if !has_cjk(&hit.name) {
        return None;
    }
    let city = hit.name.trim().to_string();
    let admin1 = short_admin1(&hit.admin1);
    let province = if has_cjk(&admin1) {
        admin1
    } else if has_cjk(&place.province) {
        place.province.trim().to_string()
    } else {
        String::new()
    };
    let country = if has_cjk(&hit.country) {
        hit.country.trim().to_string()
    } else {
        place.country.clone()
    };
    let label = if province.is_empty() {
        city.clone()
    } else {
        format!("{province} · {city}")
    };
    Some((city, province, country, label))
}

/// 用 geocoding（language=zh）反查中文名；失败 / 没有中文名时原样返回。
fn localize_place(mut place: LocatedPlace) -> LocatedPlace {
    if !is_ascii_text(&place.city) && !is_ascii_text(&place.province) {
        return place;
    }
    let Ok(hit) = geocode_lookup(&place.city) else {
        return place;
    };
    let Some((city, province, country, label)) = localized_place_names(&place, &hit) else {
        return place;
    };
    place.city = city;
    place.province = province;
    place.country = country;
    place.label = label;
    place
}

/// 取数时的同一判定：plan 名字是 ASCII 且 geocode 命中中文名 → 用中文名覆盖
/// 展示名。IP 城市沿用「省 · 市」风格，绑定 / 自定义城市直接用名字。
fn localized_city_plan_names(city: &CityPlan, hit: &GeoHit) -> Option<(String, String)> {
    if !is_ascii_text(&city.name) || !has_cjk(&hit.name) {
        return None;
    }
    let name = hit.name.trim().to_string();
    let label = if city.is_ip {
        let admin1 = short_admin1(&hit.admin1);
        if has_cjk(&admin1) {
            format!("{admin1} · {name}")
        } else {
            name.clone()
        }
    } else {
        name.clone()
    };
    Some((name, label))
}

/// 中文名覆盖后同一城市可能出现「英文名 + 中文名」两个条目：按最终 name 去重
/// （保留先出现者，主城市在最前），并同步主城市 / IP 城市下标。
fn dedupe_localized_cities(
    cities: Vec<WeatherCityState>,
    main_index: i64,
    ip_index: Option<usize>,
) -> (Vec<WeatherCityState>, i64, Option<usize>) {
    let mut seen = std::collections::HashSet::new();
    let mut remap: Vec<Option<usize>> = vec![None; cities.len()];
    let mut out: Vec<WeatherCityState> = Vec::with_capacity(cities.len());
    for (index, city) in cities.into_iter().enumerate() {
        if !seen.insert(city_key(&city.name)) {
            continue;
        }
        remap[index] = Some(out.len());
        out.push(city);
    }
    let new_main = remap
        .get(main_index.max(0) as usize)
        .copied()
        .flatten()
        .map(|index| index as i64)
        .unwrap_or(if out.is_empty() { -1 } else { 0 });
    let new_ip = ip_index.and_then(|index| remap.get(index).copied().flatten());
    (out, new_main, new_ip)
}

/// 「纬度,经度」输入解析（纬度 -90..90、经度 -180..180）。
fn parse_coord_query(query: &str) -> Option<(f64, f64)> {
    let (lat, lon) = query.split_once(',')?;
    let lat: f64 = lat.trim().parse().ok()?;
    let lon: f64 = lon.trim().parse().ok()?;
    if lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
    {
        Some((lat, lon))
    } else {
        None
    }
}

// ============================================================
// 单城市天气：数据源选择 + Open-Meteo + OpenWeather
// ============================================================

/// 按设置选数据源：api_key 非空走 OpenWeather，失败回退同城 Open-Meteo；
/// 回退也失败才把错误返回给 `fetch_city_state`（写 ok=false + error）。
fn fetch_forecast(
    lat: f64,
    lon: f64,
    provider: &WeatherProvider,
) -> Result<WeatherCityState, String> {
    if provider.uses_openweather() {
        match fetch_openweather(lat, lon, provider) {
            Ok(state) => return Ok(state),
            Err(error) => {
                eprintln!("[zbar-island-weather] OpenWeather 取数失败，回退 Open-Meteo: {error}");
            }
        }
    }
    fetch_open_meteo(lat, lon, &provider.units)
}

/// Open-Meteo 单位参数：imperial 要显式传华氏度 + mph，metric 用默认（°C + km/h）。
fn open_meteo_unit_params(units: &str) -> &'static str {
    if units == "imperial" {
        "&temperature_unit=fahrenheit&wind_speed_unit=mph"
    } else {
        ""
    }
}

fn open_meteo_forecast_url(lat: f64, lon: f64, units: &str) -> String {
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,precipitation,weather_code,wind_speed_10m\
         &hourly=temperature_2m,weather_code,precipitation_probability,is_day\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset\
         &timezone=auto&forecast_days=6{}",
        open_meteo_unit_params(units)
    )
}

fn fetch_open_meteo(lat: f64, lon: f64, units: &str) -> Result<WeatherCityState, String> {
    let j = http_json(&open_meteo_forecast_url(lat, lon, units))?;
    let current = j.get("current").cloned().unwrap_or(serde_json::Value::Null);
    let code = int(&current, "weather_code");
    let is_day = int(&current, "is_day") != 0;
    let (desc, icon) = wmo(code, is_day);
    let mut state = WeatherCityState {
        ok: true,
        error: String::new(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        temp: num(&current, "temperature_2m"),
        feels_like: num(&current, "apparent_temperature"),
        humidity: num(&current, "relative_humidity_2m"),
        wind: num(&current, "wind_speed_10m"),
        code,
        desc,
        icon: icon.to_string(),
        is_day,
        sunrise: String::new(),
        sunset: String::new(),
        hourly: Vec::new(),
        daily: Vec::new(),
        ..Default::default()
    };

    let hourly = j.get("hourly").cloned().unwrap_or(serde_json::Value::Null);
    let times = arr(&hourly, "time");
    let temps = arr(&hourly, "temperature_2m");
    let codes = arr(&hourly, "weather_code");
    let precips = arr(&hourly, "precipitation_probability");
    let days = arr(&hourly, "is_day");
    let now_hour = chrono::Local::now().format("%Y-%m-%dT%H:00").to_string();
    let start = times
        .iter()
        .position(|t| t.as_str().is_some_and(|s| s >= now_hour.as_str()))
        .unwrap_or(0);
    for i in start..times.len().min(start + 12) {
        let t = times[i].as_str().unwrap_or("");
        let hcode = codes.get(i).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let hday = days
            .get(i)
            .and_then(|v| v.as_i64())
            .map(|v| v != 0)
            .unwrap_or(true);
        let (hdesc, hicon) = wmo(hcode, hday);
        state.hourly.push(WeatherHour {
            time: t.to_string(),
            label: t.get(11..16).unwrap_or("").to_string(),
            temp: temps.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0),
            code: hcode,
            icon: hicon.to_string(),
            desc: hdesc,
            precip: precips.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0),
        });
    }

    let daily = j.get("daily").cloned().unwrap_or(serde_json::Value::Null);
    let ddates = arr(&daily, "time");
    let dcodes = arr(&daily, "weather_code");
    let dmax = arr(&daily, "temperature_2m_max");
    let dmin = arr(&daily, "temperature_2m_min");
    let dsunrise = arr(&daily, "sunrise");
    let dsunset = arr(&daily, "sunset");
    for i in 0..ddates.len().min(6) {
        let date = ddates[i].as_str().unwrap_or("").to_string();
        let dcode = dcodes.get(i).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let (ddesc, dicon) = wmo(dcode, true);
        state.daily.push(WeatherDay {
            date: date.clone(),
            weekday: weekday_zh(&date),
            code: dcode,
            icon: dicon.to_string(),
            desc: ddesc,
            max: dmax.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0),
            min: dmin.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0),
        });
    }
    if !state.daily.is_empty() {
        state.sunrise = dsunrise
            .first()
            .and_then(|v| v.as_str())
            .and_then(|s| s.get(11..16))
            .unwrap_or("")
            .to_string();
        state.sunset = dsunset
            .first()
            .and_then(|v| v.as_str())
            .and_then(|s| s.get(11..16))
            .unwrap_or("")
            .to_string();
    }
    Ok(state)
}

/// OpenWeather `weather[0].id` → WMO code：图标 / 中文描述统一复用 `wmo()`。
/// 按 OWM 文档的 condition codes 分组覆盖 2xx 雷暴 / 3xx 毛毛雨 / 5xx 雨 /
/// 6xx 雪与冻雨 / 7xx 雾霾沙尘；未收录的 id 兜底 3（阴）。
fn owm_to_wmo(id: i32, is_day: bool) -> i32 {
    match id {
        800 => {
            if is_day {
                0
            } else {
                1
            }
        }
        801 => 2,
        802 | 803 | 804 => 3,
        // 2xx 雷暴：95 雷阵雨 / 96 雷暴冰雹 / 99 强雷暴（202 沿用原来的 99）
        200 | 210 | 211 | 221 => 95,
        201 | 212 | 230 | 231 | 232 => 96,
        202 => 99,
        // 3xx 毛毛雨：51 小 / 53 中 / 55 大（313、314 为阵雨型毛毛雨）
        300 | 310 => 51,
        301 | 311 | 321 => 53,
        302 | 312 | 313 | 314 => 55,
        // 5xx 雨：511 为冻雨
        500 => 61,
        501 => 63,
        502 | 503 | 504 => 65,
        511 => 66,
        520 => 80,
        521 => 81,
        522 | 531 => 82,
        // 6xx 雪：611–616 为冻雨 / 雨夹雪，620–622 为阵雪
        600 => 71,
        601 => 73,
        602 => 75,
        611 | 612 | 613 | 615 | 616 => 66,
        620 | 621 => 85,
        622 => 86,
        // 7xx 雾 / 霾 / 沙尘 / 火山灰 / 龙卷风
        701 | 711 | 721 | 731 | 741 | 751 | 761 | 762 => 45,
        771 => 95,
        781 => 99,
        _ => 3,
    }
}

/// OpenWeather 天气图标的昼夜标记：结尾 `n` 为夜间，其余（`d` / 缺失）按白天。
fn owm_is_day(icon: &str) -> bool {
    !icon.trim().ends_with('n')
}

fn owm_url(endpoint: &str, lat: f64, lon: f64, provider: &WeatherProvider) -> String {
    format!(
        "https://api.openweathermap.org/data/2.5/{endpoint}?lat={lat}&lon={lon}&appid={}&units={}&lang=zh_cn",
        encode(&provider.api_key),
        provider.units
    )
}

/// Unix 秒 + 城市时区偏移（秒）→ 本地 (YYYY-MM-DD, HH:MM)。
fn local_parts(dt: i64, tz: i64) -> Option<(String, String)> {
    let local = dt.checked_add(tz)?;
    let moment = chrono::DateTime::from_timestamp(local, 0)?;
    Some((
        moment.format("%Y-%m-%d").to_string(),
        moment.format("%H:%M").to_string(),
    ))
}

fn owm_clock(dt: i64, tz: i64) -> String {
    local_parts(dt, tz)
        .map(|(_, time)| time)
        .unwrap_or_default()
}

/// 取 `weather[0]` 的 (condition id, icon)；缺失时 (0, "")。
fn owm_weather_of(item: &serde_json::Value) -> (i32, &str) {
    let first = item
        .get("weather")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first());
    let id = first
        .and_then(|w| w.get("id"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;
    let icon = first
        .and_then(|w| w.get("icon"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    (id, icon)
}

/// OpenWeather current（`/data/2.5/weather`）→ 城市状态（当前天气 + 日出日落）。
fn owm_current_state(j: &serde_json::Value, tz: i64) -> Result<WeatherCityState, String> {
    let item = j
        .get("weather")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| "OpenWeather 当前天气缺少 weather 字段".to_string())?;
    let id = item.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let icon_name = item.get("icon").and_then(|v| v.as_str()).unwrap_or("");
    let is_day = owm_is_day(icon_name);
    let code = owm_to_wmo(id, is_day);
    let (desc, icon) = wmo(code, is_day);
    let main = j.get("main").cloned().unwrap_or(serde_json::Value::Null);
    let wind = j
        .get("wind")
        .and_then(|w| w.get("speed"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let sys = j.get("sys").cloned().unwrap_or(serde_json::Value::Null);
    let sunrise = sys
        .get("sunrise")
        .and_then(|v| v.as_i64())
        .map(|dt| owm_clock(dt, tz))
        .unwrap_or_default();
    let sunset = sys
        .get("sunset")
        .and_then(|v| v.as_i64())
        .map(|dt| owm_clock(dt, tz))
        .unwrap_or_default();
    Ok(WeatherCityState {
        ok: true,
        error: String::new(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        temp: num(&main, "temp"),
        feels_like: num(&main, "feels_like"),
        humidity: num(&main, "humidity"),
        wind,
        code,
        desc,
        icon: icon.to_string(),
        is_day,
        sunrise,
        sunset,
        hourly: Vec::new(),
        daily: Vec::new(),
        ..Default::default()
    })
}

/// OpenWeather forecast 的 `list` 前 8 条（3 小时一条 = 24 小时）。
fn owm_hourly(j: &serde_json::Value, tz: i64) -> Vec<WeatherHour> {
    let mut hours = Vec::new();
    for item in arr(j, "list").iter().take(8) {
        let Some(dt) = item.get("dt").and_then(|v| v.as_i64()) else {
            continue;
        };
        let Some((_, time)) = local_parts(dt, tz) else {
            continue;
        };
        let (id, icon_name) = owm_weather_of(item);
        let is_day = owm_is_day(icon_name);
        let code = owm_to_wmo(id, is_day);
        let (desc, icon) = wmo(code, is_day);
        let temp = item.get("main").map(|m| num(m, "temp")).unwrap_or(0.0);
        let precip = item.get("pop").and_then(|v| v.as_f64()).unwrap_or(0.0) * 100.0;
        hours.push(WeatherHour {
            time: time.clone(),
            label: time,
            temp,
            code,
            icon: icon.to_string(),
            desc,
            precip,
        });
    }
    hours
}

#[derive(Debug)]
struct OwmDayGroup {
    date: String,
    max: f64,
    min: f64,
    /// 当天各 WMO code 出现次数（保持首次出现顺序，便于取「出现最多的 code」）。
    codes: Vec<(i32, usize)>,
}

fn bump_owm_code(codes: &mut Vec<(i32, usize)>, code: i32) {
    match codes.iter_mut().find(|(existing, _)| *existing == code) {
        Some((_, count)) => *count += 1,
        None => codes.push((code, 1)),
    }
}

/// OpenWeather forecast（3 小时间隔 / 5 天）→ 按本地日期分组的 6 天预报，
/// 每天取最高 / 最低温与出现最多的 WMO code。
fn owm_daily(j: &serde_json::Value, tz: i64) -> Vec<WeatherDay> {
    let mut groups: Vec<OwmDayGroup> = Vec::new();
    for item in arr(j, "list") {
        let Some(dt) = item.get("dt").and_then(|v| v.as_i64()) else {
            continue;
        };
        let Some((date, _)) = local_parts(dt, tz) else {
            continue;
        };
        let temp = item.get("main").map(|m| num(m, "temp")).unwrap_or(0.0);
        let (id, _) = owm_weather_of(item);
        let code = owm_to_wmo(id, true);
        match groups.iter_mut().find(|group| group.date == date) {
            Some(group) => {
                group.max = group.max.max(temp);
                group.min = group.min.min(temp);
                bump_owm_code(&mut group.codes, code);
            }
            None => {
                let mut codes = Vec::new();
                bump_owm_code(&mut codes, code);
                groups.push(OwmDayGroup {
                    date,
                    max: temp,
                    min: temp,
                    codes,
                });
            }
        }
    }
    groups
        .into_iter()
        .take(6)
        .map(|group| {
            let code = group
                .codes
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(code, _)| code)
                .unwrap_or(3);
            let (desc, icon) = wmo(code, true);
            WeatherDay {
                date: group.date.clone(),
                weekday: weekday_zh(&group.date),
                code,
                icon: icon.to_string(),
                desc,
                max: group.max,
                min: group.min,
            }
        })
        .collect()
}

/// OpenWeather 的 `wind.speed` 单位随 `units` 变：metric 是 m/s、imperial 是
/// mph。前端风速展示沿用 Open-Meteo 的口径（metric = km/h），这里把 metric
/// 换算成 km/h；imperial 两边都是 mph，保持原值。
fn owm_wind(speed: f64, units: &str) -> f64 {
    if units == "imperial" {
        speed
    } else {
        speed * 3.6
    }
}

/// OpenWeather 取数：current（当前天气 + 日出日落）+ forecast（3 小时 × 5 天）。
/// 任一请求失败都把错误交给 `fetch_forecast` 做 Open-Meteo 回退。
fn fetch_openweather(
    lat: f64,
    lon: f64,
    provider: &WeatherProvider,
) -> Result<WeatherCityState, String> {
    let current = http_json_detailed(&owm_url("weather", lat, lon, provider))
        .map_err(|e| format!("当前天气失败（{}）", e.weather_error()))?;
    let forecast = http_json_detailed(&owm_url("forecast", lat, lon, provider))
        .map_err(|e| format!("预报失败（{}）", e.weather_error()))?;
    let tz = current
        .get("timezone")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let mut state = owm_current_state(&current, tz)?;
    state.wind = owm_wind(state.wind, &provider.units);
    state.hourly = owm_hourly(&forecast, tz);
    state.daily = owm_daily(&forecast, tz);
    Ok(state)
}

/// 拉一个城市的完整天气；坐标缺失时先 geocode。失败时返回带 `ok=false`
/// 与可展示错误的城市状态，不影响其它城市。
fn fetch_city_state(city: &CityPlan, provider: &WeatherProvider) -> WeatherCityState {
    let mut state = WeatherCityState {
        name: city.name.clone(),
        label: city.label.clone(),
        lat: city.lat,
        lon: city.lon,
        is_ip: city.is_ip,
        is_user: city.is_user,
        ..Default::default()
    };
    let coords = resolve_coords(&city.name, city.lat, city.lon);
    let (lat, lon) = match coords {
        Ok((lat, lon)) => (lat, lon),
        Err(error) => {
            state.ok = false;
            state.error = error;
            return state;
        }
    };
    state.lat = Some(lat);
    state.lon = Some(lon);
    // 城市名是英文（IP 服务偶发返回英文 / 历史绑定英文名）时，用 geocode 的
    // 中文结果覆盖展示名；反查失败保留英文，不影响取数。
    if is_ascii_text(&city.name) {
        if let Ok(hit) = geocode_lookup(&city.name) {
            if let Some((name, label)) = localized_city_plan_names(city, &hit) {
                state.name = name;
                state.label = label;
            }
        }
    }
    match fetch_forecast(lat, lon, provider) {
        Ok(forecast) => {
            state.ok = true;
            state.updated_at = forecast.updated_at;
            state.temp = forecast.temp;
            state.feels_like = forecast.feels_like;
            state.humidity = forecast.humidity;
            state.wind = forecast.wind;
            state.code = forecast.code;
            state.desc = forecast.desc;
            state.icon = forecast.icon;
            state.is_day = forecast.is_day;
            state.sunrise = forecast.sunrise;
            state.sunset = forecast.sunset;
            state.hourly = forecast.hourly;
            state.daily = forecast.daily;
        }
        Err(error) => {
            state.ok = false;
            state.error = error;
        }
    }
    state
}

// ============================================================
// 主城市 / 显示列表
// ============================================================

#[derive(Debug, Clone, PartialEq)]
struct CityPlan {
    name: String,
    label: String,
    lat: Option<f64>,
    lon: Option<f64>,
    is_ip: bool,
    is_user: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct CityPlanList {
    cities: Vec<CityPlan>,
    main_index: i64,
    ip_index: Option<usize>,
    follows_ip: bool,
}

impl CityPlanList {
    fn empty() -> Self {
        Self {
            cities: Vec::new(),
            main_index: -1,
            ip_index: None,
            follows_ip: true,
        }
    }
}

fn city_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn ip_plan(ip: &LocatedPlace) -> CityPlan {
    let city = ip.city.trim();
    CityPlan {
        name: city.to_string(),
        label: if ip.label.trim().is_empty() {
            city.to_string()
        } else {
            ip.label.trim().to_string()
        },
        lat: Some(ip.lat),
        lon: Some(ip.lon),
        is_ip: true,
        is_user: false,
    }
}

fn user_plan(city: &WeatherCityCfg) -> CityPlan {
    let name = city.name.trim().to_string();
    CityPlan {
        label: name.clone(),
        name,
        lat: city.lat,
        lon: city.lon,
        is_ip: false,
        is_user: true,
    }
}

/// 主城市优先级 + 显示列表：
/// 1. `weather.city` 非空 → 绑定城市（`follows_ip = false`）；
/// 2. 否则 IP 定位城市；
/// 3. 否则 `weather.cities` 第一个；
/// 4. 都没有 → 空列表（空态）。
/// 显示顺序：[主城市] +（绑定时）[IP 城市，若与主城市不同] + 其余自定义
/// 城市（按 `weather.cities` 顺序，按 name 去重；IP 城市不会被重复追加）。
fn plan_cities(cfg: &IslandConfig, ip: Option<&LocatedPlace>) -> CityPlanList {
    let bound = cfg.weather_city.trim();
    let follows_ip = bound.is_empty();
    let ip = ip.filter(|p| !p.city.trim().is_empty());

    let main = if !bound.is_empty() {
        let (lat, lon) = super::normalize_weather_coord_pair(cfg.weather_lat, cfg.weather_lon);
        Some(CityPlan {
            name: bound.to_string(),
            label: bound.to_string(),
            lat,
            lon,
            is_ip: false,
            is_user: false,
        })
    } else if let Some(ip) = ip {
        Some(ip_plan(ip))
    } else {
        cfg.weather_cities
            .iter()
            .find(|city| !city.name.trim().is_empty())
            .map(user_plan)
    };
    let Some(main) = main else {
        return CityPlanList::empty();
    };

    let mut seen = std::collections::HashSet::new();
    seen.insert(city_key(&main.name));
    let mut cities = vec![main];

    // 绑定时 IP 城市始终作为第二项（与主城市同名/同城时不再重复）。
    let mut ip_index = None;
    if !follows_ip {
        if let Some(ip) = ip {
            if seen.insert(city_key(&ip.city)) {
                ip_index = Some(cities.len());
                cities.push(ip_plan(ip));
            }
        }
    }

    for city in &cfg.weather_cities {
        let name = city.name.trim();
        if name.is_empty() {
            continue;
        }
        if !seen.insert(city_key(name)) {
            continue;
        }
        cities.push(user_plan(city));
    }

    CityPlanList {
        cities,
        main_index: 0,
        ip_index,
        follows_ip,
    }
}

fn empty_state() -> WeatherState {
    WeatherState {
        ok: false,
        error: "还没有城市，点 + 添加城市".into(),
        city: String::new(),
        follows_ip: true,
        main_index: -1,
        ip_index: None,
        ..Default::default()
    }
}

/// 请求失败的错误摘要：收敛 ureq 的冗长技术文本，供前端顶部小提示使用。
fn brief_error(error: &str) -> String {
    let trimmed = error.trim();
    if trimmed.is_empty() {
        return "网络错误".into();
    }
    let lower = trimmed.to_lowercase();
    if lower.contains("connection failed")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("os error")
    {
        return "网络不可达".into();
    }
    if lower.contains("http 429") {
        return "请求过于频繁".into();
    }
    if lower.contains("http 401") {
        return "接口鉴权失败".into();
    }
    let first_line = trimmed.lines().next().unwrap_or(trimmed);
    first_line.chars().take(30).collect()
}

/// 取数成功：把当前城市状态存进快照，供后续失败时回填。
/// `key` 用计划里的市名（city_key 归一化），与失败回填时使用同一 key；不依赖
/// 取数后被本地化改写的 state.name，避免展示名变化导致快照错配。
fn remember_snapshot(key: &str, state: &WeatherCityState) {
    if !state.ok {
        return;
    }
    if let Ok(mut guard) = snapshots().lock() {
        guard.insert(key.to_string(), state.clone());
    }
}

/// 取数失败：回填最近一次成功快照并标记 `stale`；从未成功过（无快照 /
/// 坐标变化太大）则保持失败态，由前端展示完整错误。同城不同坐标（重新绑定 /
/// IP 变化）不复用，避免展示错误城市的旧数据。
fn merge_stale(key: &str, state: &mut WeatherCityState) {
    if state.ok {
        return;
    }
    let snapshot = {
        let guard = match snapshots().lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        match guard.get(key) {
            Some(snapshot) => snapshot.clone(),
            None => return,
        }
    };
    if let (Some((lat, lon)), Some((old_lat, old_lon))) =
        (state.lat.zip(state.lon), snapshot.lat.zip(snapshot.lon))
    {
        if (lat - old_lat).abs() > 0.5 || (lon - old_lon).abs() > 0.5 {
            return;
        }
    }
    // 数据字段用快照回填，身份字段（name / label / is_main 等）保持本次结果。
    state.lat = state.lat.or(snapshot.lat);
    state.lon = state.lon.or(snapshot.lon);
    state.updated_at = snapshot.updated_at;
    state.temp = snapshot.temp;
    state.feels_like = snapshot.feels_like;
    state.humidity = snapshot.humidity;
    state.wind = snapshot.wind;
    state.code = snapshot.code;
    state.desc = snapshot.desc.clone();
    state.icon = snapshot.icon.clone();
    state.is_day = snapshot.is_day;
    state.sunrise = snapshot.sunrise.clone();
    state.sunset = snapshot.sunset.clone();
    state.hourly = snapshot.hourly.clone();
    state.daily = snapshot.daily.clone();
    state.stale = true;
    state.error_brief = brief_error(&state.error);
}

fn build_state(
    cfg: &IslandConfig,
    ip: Option<&LocatedPlace>,
    provider: &WeatherProvider,
) -> WeatherState {
    let plan = plan_cities(cfg, ip);
    if plan.cities.is_empty() {
        let mut state = empty_state();
        state.units = provider.units.clone();
        return state;
    }
    let mut cities: Vec<WeatherCityState> = Vec::with_capacity(plan.cities.len());
    for (index, city) in plan.cities.iter().enumerate() {
        let mut state = fetch_city_state(city, provider);
        state.is_main = index as i64 == plan.main_index;
        let key = city_key(&city.name);
        remember_snapshot(&key, &state);
        merge_stale(&key, &mut state);
        cities.push(state);
    }
    // 中文名覆盖后可能和 IP 城市变成同一个名字：按最终 name 再去重一次。
    let (cities, main_index, ip_index) =
        dedupe_localized_cities(cities, plan.main_index, plan.ip_index);
    let main = cities
        .get(main_index.max(0) as usize)
        .cloned()
        .unwrap_or_default();
    WeatherState {
        ok: main.ok,
        error: main.error.clone(),
        stale: main.stale,
        error_brief: main.error_brief.clone(),
        city: main.label.clone(),
        updated_at: main.updated_at,
        temp: main.temp,
        feels_like: main.feels_like,
        humidity: main.humidity,
        wind: main.wind,
        code: main.code,
        desc: main.desc.clone(),
        icon: main.icon.clone(),
        is_day: main.is_day,
        sunrise: main.sunrise.clone(),
        sunset: main.sunset.clone(),
        hourly: main.hourly.clone(),
        daily: main.daily.clone(),
        cities,
        main_index,
        ip_index,
        follows_ip: plan.follows_ip,
        units: provider.units.clone(),
    }
}

// ============================================================
// 线程 / 广播
// ============================================================

#[derive(Debug, Clone)]
struct IpSnapshot {
    place: Option<LocatedPlace>,
}

fn same_ip(left: Option<&LocatedPlace>, right: Option<&LocatedPlace>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.city.trim() == right.city.trim()
                && (left.lat - right.lat).abs() < 0.05
                && (left.lon - right.lon).abs() < 0.05
        }
        _ => false,
    }
}

fn publish(app: &AppHandle, state: WeatherState) {
    if let Ok(mut guard) = cache_slot().lock() {
        *guard = Some(state.clone());
    }
    let _ = app.emit(WEATHER_EVENT, state);
}

fn fetch_and_publish_with(ip: Option<LocatedPlace>) {
    if STOP.load(Ordering::Relaxed) {
        return;
    }
    let cfg = super::load_config();
    // 数据源（api_key / units）与刷新间隔在这一拍发起时就读一次 settings.json，
    // 且放在防重入早退之前：即使本拍随后被挡掉，保存设置后的新间隔也已生效，
    // 用户把 180 分钟改成 5 分钟不用等旧间隔到点。500ms 循环仍只读内存原子量。
    let provider = WeatherProvider::from_settings();
    INTERVAL_SECS.store(refresh_secs(provider.interval_min), Ordering::Relaxed);
    if FETCHING.swap(true, Ordering::SeqCst) {
        // 已有一拍在跑：本次丢弃，但 `FORCE` 保持为 true，后台循环下一拍补取，
        // 主动刷新不会被吞掉。
        return;
    }
    // 抢到取数权：这次确实会取数，兑现主动刷新标记。
    FORCE.store(false, Ordering::SeqCst);
    let state = build_state(&cfg, ip.as_ref(), &provider);
    FETCHING.store(false, Ordering::SeqCst);
    let Some(app) = app_slot().lock().ok().and_then(|guard| guard.clone()) else {
        return;
    };
    if let Ok(mut slot) = last_ip_slot().lock() {
        *slot = Some(IpSnapshot { place: ip.clone() });
    }
    publish(&app, state);
}

fn fetch_and_publish() {
    fetch_and_publish_with(locate_place().ok());
}

/// 每 5 分钟的 IP 巡检：城市/坐标变化（含定位从失败恢复）才刷新整拍。
fn check_ip_changed() {
    let fresh = locate_place().ok();
    let previous = last_ip_slot().lock().ok().and_then(|guard| guard.clone());
    let changed = match &previous {
        None => true,
        Some(snapshot) => !same_ip(snapshot.place.as_ref(), fresh.as_ref()),
    };
    if changed {
        fetch_and_publish_with(fresh);
    }
}

/// 启动后台刷新线程（幂等）
pub fn start(app: AppHandle) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(app.clone());
    }
    STOP.store(false, Ordering::SeqCst);
    FORCE.store(false, Ordering::SeqCst);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    thread::Builder::new()
        .name("zbar-island-weather".into())
        .spawn(move || {
            fetch_and_publish();
            let mut last = Instant::now();
            let mut last_ip_check = Instant::now();
            while !STOP.load(Ordering::Relaxed) && GEN.load(Ordering::SeqCst) == gen {
                thread::sleep(Duration::from_millis(500));
                if last_ip_check.elapsed() >= Duration::from_secs(IP_CHECK_SECS) {
                    last_ip_check = Instant::now();
                    check_ip_changed();
                }
                // 间隔由最近一拍取数时读到的 `weather.interval_min` 决定；
                // 外部 refresh() 也会在那一拍顺带更新它。`FORCE` 表示有主动
                // 刷新被防重入挡掉（取数入口在真正开取时才清零），这里下一拍
                // （≤500ms）补取，保证保存设置后的刷新不会被吞。
                let forced = FORCE.load(Ordering::SeqCst);
                let due = last.elapsed() >= Duration::from_secs(INTERVAL_SECS.load(Ordering::Relaxed));
                if forced || due {
                    last = Instant::now();
                    fetch_and_publish();
                }
            }
        })
        .ok();
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    GEN.fetch_add(1, Ordering::SeqCst);
}

/// 立即刷新（配置变更 / 用户点刷新 / 绑定命令）。
pub fn refresh() {
    if STOP.load(Ordering::Relaxed) {
        return;
    }
    // 先置 FORCE 再 spawn 一次性线程：没在取数时线程立刻刷新（行为同以前）；
    // 撞上正在跑的取数被防重入挡掉时，标记留给后台循环下一拍补取。
    FORCE.store(true, Ordering::SeqCst);
    thread::Builder::new()
        .name("zbar-island-weather-refresh".into())
        .spawn(fetch_and_publish)
        .ok();
}

#[tauri::command]
pub fn island_weather() -> Option<WeatherState> {
    cache_slot().lock().ok().and_then(|guard| guard.clone())
}

#[tauri::command]
pub fn island_weather_refresh() -> Result<(), String> {
    refresh();
    Ok(())
}

// ============================================================
// 绑定 / 添加 / 删除城市（settings.json 为权威，统一走 apply_patch）
// ============================================================

/// 取消绑定时要补回 `weather.cities` 的城市：被解绑城市不在列表里才追加，
/// 保证它仍可被删除 / 重新绑定。
fn unbind_city_list(cfg: &IslandConfig) -> Vec<WeatherCityCfg> {
    let mut cities = cfg.weather_cities.clone();
    let name = cfg.weather_city.trim();
    if name.is_empty() {
        return cities;
    }
    if cities
        .iter()
        .any(|city| city.name.trim().eq_ignore_ascii_case(name))
    {
        return cities;
    }
    let (lat, lon) = super::normalize_weather_coord_pair(cfg.weather_lat, cfg.weather_lon);
    cities.push(WeatherCityCfg {
        name: name.to_string(),
        lat,
        lon,
    });
    cities
}

/// 添加城市时的重名 / 重复坐标检查。
fn duplicate_city_error(cfg: &IslandConfig, city: &WeatherCityCfg) -> Option<String> {
    let name = city.name.trim();
    let key = name.to_lowercase();
    let bound = cfg.weather_city.trim();
    if !bound.is_empty() && bound.to_lowercase() == key {
        return Some(format!("「{name}」已经是绑定的主城市"));
    }
    for existing in &cfg.weather_cities {
        if existing.name.trim().to_lowercase() == key {
            return Some(format!("「{name}」已在城市列表里"));
        }
        if let (Some(lat), Some(lon), Some(old_lat), Some(old_lon)) =
            (city.lat, city.lon, existing.lat, existing.lon)
        {
            if (lat - old_lat).abs() < 0.01 && (lon - old_lon).abs() < 0.01 {
                return Some(format!("坐标与「{}」重复", existing.name));
            }
        }
    }
    if let (Some(lat), Some(lon), Some(bound_lat), Some(bound_lon)) =
        (city.lat, city.lon, cfg.weather_lat, cfg.weather_lon)
    {
        if !bound.is_empty() && (lat - bound_lat).abs() < 0.01 && (lon - bound_lon).abs() < 0.01 {
            return Some(format!("坐标与主城市「{bound}」重复"));
        }
    }
    None
}

/// 解析「添加城市」输入：城市名走 geocode，`纬度,经度` 直接落坐标。
fn resolve_add_city(query: &str) -> Result<WeatherCityCfg, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("请输入城市名或「纬度,经度」".into());
    }
    if query.contains(',') {
        let (lat, lon) = parse_coord_query(query).ok_or_else(|| {
            "坐标格式应为「纬度,经度」（纬度 -90..90、经度 -180..180，如 30.27,120.15）".to_string()
        })?;
        return Ok(WeatherCityCfg {
            name: format!("{lat:.2},{lon:.2}"),
            lat: Some(lat),
            lon: Some(lon),
        });
    }
    let hit = geocode_lookup(query)?;
    let (lat, lon) = super::normalize_weather_coord_pair(Some(hit.lat), Some(hit.lon));
    let (Some(lat), Some(lon)) = (lat, lon) else {
        return Err(format!("「{query}」的坐标不合法"));
    };
    Ok(WeatherCityCfg {
        name: hit.name,
        lat: Some(lat),
        lon: Some(lon),
    })
}

fn weather_patch(
    pairs: &[(&str, serde_json::Value)],
) -> serde_json::Map<String, serde_json::Value> {
    let mut patch = serde_json::Map::new();
    for (key, value) in pairs {
        patch.insert((*key).to_string(), value.clone());
    }
    patch
}

fn cities_value(cities: &[WeatherCityCfg]) -> Result<serde_json::Value, String> {
    serde_json::to_value(cities).map_err(|e| format!("序列化城市列表失败: {e}"))
}

/// 绑定主城市：把 name / lat / lon 写进 `weather.city/lat/lon`（settings.json
/// 同步），随后刷新天气。
#[tauri::command]
pub async fn island_weather_bind(
    name: String,
    lat: Option<f64>,
    lon: Option<f64>,
    app: AppHandle,
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("城市名为空".into());
    }
    let (lat, lon) = super::normalize_weather_coord_pair(lat, lon);
    let patch = weather_patch(&[
        ("weather.city", serde_json::Value::String(name)),
        (
            "weather.lat",
            serde_json::to_value(lat).map_err(|e| e.to_string())?,
        ),
        (
            "weather.lon",
            serde_json::to_value(lon).map_err(|e| e.to_string())?,
        ),
    ]);
    super::settings::apply_patch(&app, &patch).await?;
    refresh();
    Ok(())
}

/// 取消绑定：清空 `weather.city/lat/lon`，被解绑城市不在 `weather.cities`
/// 里时追加进去（仍可删除 / 重新绑定）；随后强制重新定位 IP 并刷新。
#[tauri::command]
pub async fn island_weather_unbind(app: AppHandle) -> Result<(), String> {
    let cfg = super::load_config();
    let name = cfg.weather_city.trim();
    if name.is_empty() {
        return Err("当前没有绑定城市".into());
    }
    let cities = unbind_city_list(&cfg);
    let patch = weather_patch(&[
        ("weather.city", serde_json::Value::String(String::new())),
        ("weather.lat", serde_json::Value::Null),
        ("weather.lon", serde_json::Value::Null),
        ("weather.cities", cities_value(&cities)?),
    ]);
    super::settings::apply_patch(&app, &patch).await?;
    refresh();
    Ok(())
}

/// 添加城市的共用落库逻辑：数量上限 / 重名与坐标去重 / 持久化 / 广播刷新。
/// 两条入口（在线 geocode 与带坐标直加）都走这里，保证行为一致。
async fn commit_add_city(city: WeatherCityCfg, app: &AppHandle) -> Result<String, String> {
    let cfg = super::load_config();
    if cfg.weather_cities.len() >= 20 {
        return Err("最多添加 20 个城市".into());
    }
    if let Some(error) = duplicate_city_error(&cfg, &city) {
        return Err(error);
    }
    let mut cities = cfg.weather_cities.clone();
    let label = city.name.clone();
    cities.push(city);
    let patch = weather_patch(&[("weather.cities", cities_value(&cities)?)]);
    super::settings::apply_patch(app, &patch).await?;
    refresh();
    Ok(format!("已添加城市「{label}」"))
}

/// 添加自定义城市：城市名 geocode 或「纬度,经度」；重名 / 重复坐标拒绝。
/// 成功返回可展示提示文案。
#[tauri::command]
pub async fn island_weather_add_city(query: String, app: AppHandle) -> Result<String, String> {
    // geocode 是阻塞网络请求，放到 blocking 线程，避免占住 async 运行时。
    let city = tauri::async_runtime::spawn_blocking(move || resolve_add_city(&query))
        .await
        .map_err(|e| format!("添加城市任务失败: {e}"))??;
    commit_add_city(city, &app).await
}

/// 带坐标直加城市（省市联动 / 当前位置入口）：跳过在线 geocode，直接用
/// 前端给定的市名、省名与经纬度构造配置入列；复用与 `island_weather_add_city`
/// 相同的数量上限 / 去重 / 持久化 / 广播逻辑。
#[tauri::command]
pub async fn island_weather_add_city_exact(
    name: String,
    province: String,
    lat: f64,
    lon: f64,
    app: AppHandle,
) -> Result<String, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("城市名为空".into());
    }
    let (lat, lon) = super::normalize_weather_coord_pair(Some(lat), Some(lon));
    let (Some(lat), Some(lon)) = (lat, lon) else {
        return Err("城市坐标不合法".into());
    };
    // 省名仅用于拼接去重更稳的展示名（同名市在不同省时区分）；为空则只用市名。
    let province = province.trim();
    let display = if province.is_empty() || name.contains(province) {
        name.clone()
    } else {
        format!("{province}·{name}")
    };
    commit_add_city(
        WeatherCityCfg {
            name: display,
            lat: Some(lat),
            lon: Some(lon),
        },
        &app,
    )
    .await
}

/// 删除自定义城市：只从 `weather.cities` 删除；绑定的主城市不可删。
#[tauri::command]
pub async fn island_weather_remove_city(
    name: String,
    lat: Option<f64>,
    lon: Option<f64>,
    app: AppHandle,
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("城市名为空".into());
    }
    let cfg = super::load_config();
    if cfg.weather_city.trim().eq_ignore_ascii_case(&name) {
        return Err("主城市不能删除，请先取消绑定".into());
    }
    let (lat, lon) = super::normalize_weather_coord_pair(lat, lon);
    let before = cfg.weather_cities.len();
    let cities: Vec<WeatherCityCfg> = cfg
        .weather_cities
        .iter()
        .filter(|city| {
            if !city.name.trim().eq_ignore_ascii_case(&name) {
                return true;
            }
            match (lat, lon, city.lat, city.lon) {
                (Some(lat), Some(lon), Some(old_lat), Some(old_lon)) => {
                    (lat - old_lat).abs() >= 0.01 || (lon - old_lon).abs() >= 0.01
                }
                _ => false,
            }
        })
        .cloned()
        .collect();
    if cities.len() == before {
        return Ok(());
    }
    let patch = weather_patch(&[("weather.cities", cities_value(&cities)?)]);
    super::settings::apply_patch(&app, &patch).await?;
    refresh();
    Ok(())
}

// ============================================================
// 测试（只测纯逻辑，不联网）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn place(city: &str, province: &str, lat: f64, lon: f64) -> LocatedPlace {
        LocatedPlace {
            ok: true,
            error: String::new(),
            country: "中国".into(),
            province: province.into(),
            city: city.into(),
            district: String::new(),
            lat,
            lon,
            label: format!("{province} · {city}"),
        }
    }

    fn weather_cfg(
        bound: Option<&str>,
        cities: &[(&str, Option<f64>, Option<f64>)],
    ) -> IslandConfig {
        let mut cfg = IslandConfig::default();
        if let Some(name) = bound {
            cfg.weather_city = name.into();
        }
        cfg.weather_cities = cities
            .iter()
            .map(|(name, lat, lon)| WeatherCityCfg {
                name: (*name).into(),
                lat: *lat,
                lon: *lon,
            })
            .collect();
        cfg
    }

    fn names(plan: &CityPlanList) -> Vec<String> {
        plan.cities.iter().map(|city| city.label.clone()).collect()
    }

    #[test]
    fn main_city_priority_bound_over_ip_over_first_custom_over_none() {
        let ip = place("杭州", "浙江", 30.27, 120.15);

        // 绑定 > IP：主城市是绑定城市，IP 作为第二项
        let bound = weather_cfg(Some("西安"), &[("上海", None, None), ("杭州", None, None)]);
        let plan = plan_cities(&bound, Some(&ip));
        assert_eq!(plan.main_index, 0);
        assert!(!plan.follows_ip);
        assert_eq!(names(&plan), vec!["西安", "浙江 · 杭州", "上海"]);
        assert_eq!(plan.ip_index, Some(1));

        // IP > 首个自定义：主城市是 IP，且 IP 不重复显示
        let unbound = weather_cfg(None, &[("上海", None, None), ("杭州", None, None)]);
        let plan = plan_cities(&unbound, Some(&ip));
        assert_eq!(plan.main_index, 0);
        assert!(plan.follows_ip);
        assert!(plan.cities[0].is_ip);
        assert_eq!(names(&plan), vec!["浙江 · 杭州", "上海"]);
        assert_eq!(plan.ip_index, None, "主城市就是 IP 时 ipIndex = null");

        // IP 失败 → 首个自定义
        let plan = plan_cities(&unbound, None);
        assert_eq!(plan.main_index, 0);
        assert!(plan.follows_ip);
        assert_eq!(names(&plan), vec!["上海", "杭州"]);
        assert_eq!(plan.ip_index, None);

        // 无绑定 / 无 IP / 无自定义 → 空列表
        let plan = plan_cities(&weather_cfg(None, &[]), None);
        assert!(plan.cities.is_empty());
        assert_eq!(plan.main_index, -1);
        assert_eq!(plan.ip_index, None);
        assert!(plan.follows_ip);
    }

    #[test]
    fn display_list_dedupes_custom_cities_and_never_repeats_ip() {
        let ip = place("杭州", "浙江", 30.27, 120.15);
        // 绑定的主城市与 IP 同名：不再重复添加 IP 项
        let bound_same = weather_cfg(Some("杭州"), &[("上海", None, None)]);
        let plan = plan_cities(&bound_same, Some(&ip));
        assert_eq!(names(&plan), vec!["杭州", "上海"]);
        assert_eq!(plan.ip_index, None);

        // 自定义列表里已有 IP 城市：IP 项在前，自定义重复项被丢掉
        let bound = weather_cfg(
            Some("西安"),
            &[
                ("上海", None, None),
                ("杭州", None, None),
                ("上海", None, None),
            ],
        );
        let plan = plan_cities(&bound, Some(&ip));
        assert_eq!(names(&plan), vec!["西安", "浙江 · 杭州", "上海"]);
        assert_eq!(plan.ip_index, Some(1));

        // 主城市本身来自自定义列表时，后续不再重复
        let unbound = weather_cfg(
            None,
            &[
                ("上海", None, None),
                ("上海", None, None),
                ("杭州", None, None),
            ],
        );
        let plan = plan_cities(&unbound, None);
        assert_eq!(names(&plan), vec!["上海", "杭州"]);
        assert!(plan.cities[0].is_user);
    }

    #[test]
    fn ip_failure_falls_back_to_custom_city_or_empty_state() {
        let ip = place("杭州", "浙江", 30.27, 120.15);
        let bound = weather_cfg(Some("西安"), &[("上海", None, None)]);
        // 绑定城市在 IP 定位失败时仍是主城市，列表里没有 IP 项
        let plan = plan_cities(&bound, None);
        assert_eq!(names(&plan), vec!["西安", "上海"]);
        assert_eq!(plan.ip_index, None);

        // 未绑定 + IP 失败 + 有自定义 → 第一个自定义
        let unbound = weather_cfg(None, &[("上海", None, None), ("杭州", None, None)]);
        let plan = plan_cities(&unbound, None);
        assert_eq!(plan.main_index, 0);
        assert_eq!(names(&plan), vec!["上海", "杭州"]);

        // 未绑定 + IP 失败 + 无自定义 → 空态契约
        let state = empty_state();
        assert!(!state.ok);
        assert_eq!(state.error, "还没有城市，点 + 添加城市");
        assert_eq!(state.city, "");
        assert!(state.cities.is_empty());
        assert_eq!(state.main_index, -1);
        assert_eq!(state.ip_index, None);
        assert!(state.follows_ip);
        assert!(state.hourly.is_empty());
        assert!(state.daily.is_empty());
        let plan = plan_cities(&weather_cfg(None, &[]), None);
        assert!(plan.cities.is_empty());
        assert_eq!(plan.main_index, -1);

        // 同一份空列表在 IP 成功时主城市就是 IP（不请求自定义城市）
        let plan = plan_cities(&weather_cfg(None, &[]), Some(&ip));
        assert_eq!(names(&plan), vec!["浙江 · 杭州"]);
        assert!(plan.cities[0].is_ip);
        assert_eq!(plan.ip_index, None);
    }

    #[test]
    fn unbind_appends_the_bound_city_once() {
        let mut cfg = weather_cfg(Some("杭州"), &[("上海", Some(31.23), Some(121.47))]);
        cfg.weather_lat = Some(30.27);
        cfg.weather_lon = Some(120.15);
        let cities = unbind_city_list(&cfg);
        assert_eq!(cities.len(), 2);
        assert_eq!(cities[0].name, "上海");
        assert_eq!(cities[1].name, "杭州");
        assert_eq!(cities[1].lat, Some(30.27));

        // 已在列表里时不重复追加（大小写 / 空白不敏感）
        let mut cfg = weather_cfg(Some(" 杭州 "), &[("杭州", Some(30.27), Some(120.15))]);
        cfg.weather_city = " 杭州 ".into();
        assert_eq!(unbind_city_list(&cfg).len(), 1);

        // 没有绑定城市时原样返回
        let cfg = weather_cfg(None, &[("上海", None, None)]);
        assert_eq!(unbind_city_list(&cfg).len(), 1);
    }

    #[test]
    fn weather_state_serializes_camel_case_contract() {
        let state = WeatherState {
            ok: true,
            city: "西安".into(),
            cities: vec![WeatherCityState {
                name: "西安".into(),
                label: "陕西 · 西安".into(),
                is_main: true,
                is_ip: true,
                ..Default::default()
            }],
            main_index: 0,
            ip_index: None,
            follows_ip: true,
            ..Default::default()
        };
        let raw = serde_json::to_value(&state).expect("可序列化");
        for key in [
            "ok",
            "error",
            "stale",
            "errorBrief",
            "city",
            "updatedAt",
            "temp",
            "feelsLike",
            "humidity",
            "wind",
            "code",
            "desc",
            "icon",
            "isDay",
            "sunrise",
            "sunset",
            "hourly",
            "daily",
            "cities",
            "mainIndex",
            "ipIndex",
            "followsIp",
            "units",
        ] {
            assert!(raw.get(key).is_some(), "WeatherState 缺字段 {key}");
        }
        assert!(raw["ipIndex"].is_null(), "无 IP 项时 ipIndex = null");
        assert_eq!(raw["mainIndex"], 0);
        let city = &raw["cities"][0];
        for key in [
            "name",
            "label",
            "lat",
            "lon",
            "isMain",
            "isIp",
            "isUser",
            "ok",
            "error",
            "stale",
            "errorBrief",
            "updatedAt",
            "temp",
            "feelsLike",
            "humidity",
            "wind",
            "code",
            "desc",
            "icon",
            "isDay",
            "sunrise",
            "sunset",
            "hourly",
            "daily",
        ] {
            assert!(city.get(key).is_some(), "WeatherCityState 缺字段 {key}");
        }
        assert_eq!(city["isMain"], true);
        assert_eq!(city["isIp"], true);
    }

    #[test]
    fn add_city_parses_coordinates_and_rejects_duplicates() {
        assert_eq!(parse_coord_query("30.27,120.15"), Some((30.27, 120.15)));
        assert_eq!(parse_coord_query(" 30.27 , 120.15 "), Some((30.27, 120.15)));
        assert_eq!(parse_coord_query("91,120"), None, "纬度越界");
        assert_eq!(parse_coord_query("-31,-120.5"), Some((-31.0, -120.5)));
        assert_eq!(parse_coord_query("杭州"), None);
        assert_eq!(parse_coord_query("abc,120"), None);

        let cfg = weather_cfg(Some("西安"), &[("上海", Some(31.23), Some(121.47))]);
        let duplicate = |name: &str, lat: f64, lon: f64| {
            duplicate_city_error(
                &cfg,
                &WeatherCityCfg {
                    name: name.into(),
                    lat: Some(lat),
                    lon: Some(lon),
                },
            )
        };
        assert!(duplicate("上海", 30.0, 120.0).is_some(), "重名拒绝");
        assert!(duplicate("杭州", 31.23, 121.47).is_some(), "重复坐标拒绝");
        assert!(duplicate("西安", 34.0, 108.0).is_some(), "与主城市重名拒绝");
        assert!(duplicate("杭州", 30.27, 120.15).is_none());
    }

    // ============================================================
    // ⑤ 中文城市名 / OpenWeather / 单位 / 刷新间隔（全部纯函数，不联网）
    // ============================================================

    fn geo_hit(name: &str, admin1: &str, country: &str) -> GeoHit {
        GeoHit {
            name: name.into(),
            lat: 30.27,
            lon: 120.15,
            admin1: admin1.into(),
            country: country.into(),
        }
    }

    fn ipwho_english_place() -> LocatedPlace {
        let mut place = place("Hangzhou", "Zhejiang", 30.27, 120.15);
        place.country = "China".into();
        place.label = "Zhejiang · Hangzhou".into();
        place
    }

    #[test]
    fn ascii_ip_place_names_are_localized_to_chinese() {
        assert!(is_ascii_text("Hangzhou"));
        assert!(!is_ascii_text("杭州"));
        assert!(!is_ascii_text("  "));
        assert!(has_cjk("杭州"));
        assert!(!has_cjk("Hangzhou"));
        assert_eq!(short_admin1("浙江省"), "浙江");
        assert_eq!(short_admin1("浙江"), "浙江");
        assert_eq!(short_admin1(""), "");

        let hit = geo_hit("杭州", "浙江省", "中国");
        let (city, province, country, label) =
            localized_place_names(&ipwho_english_place(), &hit).expect("英文定位结果应被本地化");
        assert_eq!(city, "杭州");
        assert_eq!(
            province, "浙江",
            "admin1 去掉「省」后缀，与 ip-api 口径一致"
        );
        assert_eq!(country, "中国");
        assert_eq!(label, "浙江 · 杭州");

        // 已经是中文名：不需要反查替换
        assert!(localized_place_names(&place("杭州", "浙江", 30.27, 120.15), &hit).is_none());
        // geocoding 也只回英文：保留原文，不阻断天气取数
        assert!(localized_place_names(
            &ipwho_english_place(),
            &geo_hit("Hangzhou", "Zhejiang", "China")
        )
        .is_none());
        // 没有 admin1 时退化成「市」名，不出现「 · 」空省
        assert_eq!(
            localized_place_names(&ipwho_english_place(), &geo_hit("杭州", "", "")),
            Some(("杭州".into(), String::new(), "China".into(), "杭州".into()))
        );

        // 取数路径：IP 城市「省 · 市」，绑定 / 自定义城市只留名字
        let ip_plan = CityPlan {
            name: "Hangzhou".into(),
            label: "Hangzhou".into(),
            lat: Some(30.27),
            lon: Some(120.15),
            is_ip: true,
            is_user: false,
        };
        assert_eq!(
            localized_city_plan_names(&ip_plan, &hit),
            Some(("杭州".to_string(), "浙江 · 杭州".to_string()))
        );
        let user_plan = CityPlan {
            is_ip: false,
            is_user: true,
            ..ip_plan
        };
        assert_eq!(
            localized_city_plan_names(&user_plan, &hit),
            Some(("杭州".to_string(), "杭州".to_string()))
        );
        // plan 名字已是中文：不覆盖
        assert!(localized_city_plan_names(
            &CityPlan {
                name: "杭州".into(),
                label: "杭州".into(),
                ..user_plan
            },
            &hit
        )
        .is_none());
    }

    #[test]
    fn localized_duplicate_cities_are_merged_and_indexes_remapped() {
        let city = |name: &str, label: &str, is_ip: bool| WeatherCityState {
            name: name.into(),
            label: label.into(),
            is_ip,
            ..Default::default()
        };
        // 主城市「Shanghai」被本地化成「上海」，IP 项恰好也是「上海」
        let cities = vec![
            city("上海", "上海", false),
            city("上海", "浙江 · 上海", true),
            city("杭州", "杭州", true),
        ];
        let (merged, main_index, ip_index) = dedupe_localized_cities(cities, 0, Some(1));
        assert_eq!(merged.len(), 2, "同城中英文条目合并");
        assert_eq!(merged[0].name, "上海");
        assert_eq!(main_index, 0);
        assert_eq!(ip_index, None, "被合并的 IP 下标置空");

        // 没有重复时原样返回，下标不变
        let cities = vec![city("上海", "上海", false), city("杭州", "杭州", true)];
        let (merged, main_index, ip_index) = dedupe_localized_cities(cities, 0, Some(1));
        assert_eq!(merged.len(), 2);
        assert_eq!(main_index, 0);
        assert_eq!(ip_index, Some(1));

        // 空列表：主城市 -1
        let (merged, main_index, ip_index) = dedupe_localized_cities(Vec::new(), 0, None);
        assert!(merged.is_empty());
        assert_eq!(main_index, -1);
        assert_eq!(ip_index, None);
    }

    #[test]
    fn owm_condition_ids_map_to_wmo_codes() {
        assert_eq!(owm_to_wmo(800, true), 0, "晴天 → 晴");
        assert_eq!(owm_to_wmo(800, false), 1, "晴夜 → 大部晴朗（夜间图标）");
        assert_eq!(owm_to_wmo(801, true), 2);
        assert_eq!(owm_to_wmo(802, true), 3);
        assert_eq!(owm_to_wmo(803, true), 3);
        assert_eq!(owm_to_wmo(804, true), 3);
        assert_eq!(owm_to_wmo(500, true), 61);
        assert_eq!(owm_to_wmo(501, true), 63);
        assert_eq!(owm_to_wmo(502, true), 65);
        assert_eq!(owm_to_wmo(520, true), 80);
        assert_eq!(owm_to_wmo(521, true), 81);
        assert_eq!(owm_to_wmo(522, true), 82);
        assert_eq!(owm_to_wmo(600, true), 71);
        assert_eq!(owm_to_wmo(601, true), 73);
        assert_eq!(owm_to_wmo(602, true), 75);
        assert_eq!(owm_to_wmo(701, true), 45);
        assert_eq!(owm_to_wmo(741, true), 45);
        assert_eq!(owm_to_wmo(711, true), 45);
        assert_eq!(owm_to_wmo(762, true), 45);
        assert_eq!(owm_to_wmo(771, true), 95);
        assert_eq!(owm_to_wmo(781, true), 99, "龙卷风 → 强雷暴");
        assert_eq!(owm_to_wmo(200, true), 95);
        assert_eq!(owm_to_wmo(201, true), 96);
        assert_eq!(owm_to_wmo(202, true), 99);
        assert_eq!(owm_to_wmo(999, true), 3, "未知 id 兜底阴天");
        assert_eq!(owm_to_wmo(0, true), 3);
        assert!(owm_is_day("01d"));
        assert!(!owm_is_day("01n"));
        assert!(owm_is_day(""), "icon 缺失按白天处理");
        // 映射后的 code 都能被 wmo() 解释出中文描述 + 图标
        assert_eq!(wmo(0, true), ("晴".to_string(), "sun"));
        assert_eq!(wmo(71, true), ("小雪".to_string(), "snow"));
    }

    /// 补全后的 OWM condition id 映射：雷暴 / 毛毛雨 / 雨 / 雪与冻雨 / 雾霾沙尘
    /// 都要落到具体 WMO code，不能掉进兜底的 3（阴）。
    #[test]
    fn owm_extended_condition_ids_map_to_wmo_codes() {
        // 2xx 雷暴：95 / 96 / 99
        for (id, code) in [
            (200, 95),
            (210, 95),
            (211, 95),
            (221, 95),
            (201, 96),
            (212, 96),
            (230, 96),
            (231, 96),
            (232, 96),
            (202, 99),
        ] {
            assert_eq!(owm_to_wmo(id, true), code, "雷暴 id {id}");
        }
        // 3xx 毛毛雨：51 / 53 / 55
        for (id, code) in [
            (300, 51),
            (310, 51),
            (301, 53),
            (311, 53),
            (321, 53),
            (302, 55),
            (312, 55),
            (313, 55),
            (314, 55),
        ] {
            assert_eq!(owm_to_wmo(id, true), code, "毛毛雨 id {id}");
        }
        // 5xx 雨
        for (id, code) in [
            (500, 61),
            (501, 63),
            (502, 65),
            (503, 65),
            (504, 65),
            (511, 66),
            (520, 80),
            (521, 81),
            (522, 82),
            (531, 82),
        ] {
            assert_eq!(owm_to_wmo(id, true), code, "雨 id {id}");
        }
        // 6xx 雪 / 冻雨
        for (id, code) in [
            (600, 71),
            (601, 73),
            (602, 75),
            (611, 66),
            (612, 66),
            (613, 66),
            (615, 66),
            (616, 66),
            (620, 85),
            (621, 85),
            (622, 86),
        ] {
            assert_eq!(owm_to_wmo(id, true), code, "雪 / 冻雨 id {id}");
        }
        // 7xx 雾霾沙尘
        for id in [701, 711, 721, 731, 741, 751, 761, 762] {
            assert_eq!(owm_to_wmo(id, true), 45, "雾霾 id {id}");
        }
        assert_eq!(owm_to_wmo(771, true), 95, "飑 → 雷阵雨");
        assert_eq!(owm_to_wmo(781, true), 99, "龙卷风 → 强雷暴");
        // 这些 code 都能被 wmo() 解释出中文描述和图标（不落到「未知」兜底）
        for id in [202, 300, 313, 511, 522, 531, 602, 613, 622, 771, 781] {
            let (desc, icon) = wmo(owm_to_wmo(id, true), true);
            assert_ne!(desc, "未知", "id {id} 应映射到可解释的 WMO code");
            assert!(!icon.is_empty());
        }
    }

    /// OpenWeather `/data/2.5/weather` 样本（timezone = UTC+8）。
    const OWM_CURRENT: &str = r#"{
        "timezone": 28800,
        "name": "杭州",
        "weather": [{"id": 800, "icon": "01d", "description": "clear sky"}],
        "main": {"temp": 18.5, "feels_like": 17.2, "humidity": 52},
        "wind": {"speed": 3.4},
        "sys": {"sunrise": 1700000000, "sunset": 1700032400}
    }"#;

    #[test]
    fn owm_current_json_parses_into_city_state() {
        let j: serde_json::Value = serde_json::from_str(OWM_CURRENT).expect("样本 JSON 合法");
        let tz = j["timezone"].as_i64().unwrap_or(0);
        let state = owm_current_state(&j, tz).expect("current 解析成功");
        assert!(state.ok);
        assert_eq!(state.temp, 18.5);
        assert_eq!(state.feels_like, 17.2);
        assert_eq!(state.humidity, 52.0);
        assert_eq!(state.wind, 3.4, "解析层保留 OpenWeather 原始 wind.speed");
        assert!(
            (owm_wind(3.4, "metric") - 12.24).abs() < 1e-9,
            "metric m/s → km/h"
        );
        assert_eq!(owm_wind(3.4, "imperial"), 3.4, "imperial 两边都是 mph");
        assert_eq!(state.code, 0);
        assert_eq!(state.desc, "晴");
        assert_eq!(state.icon, "sun");
        assert!(state.is_day);
        assert_eq!(state.sunrise, "06:13", "Unix 秒 + UTC+8 偏移换成本地 HH:MM");
        assert_eq!(state.sunset, "15:13");

        // 夜间图标：is_day=false，800 映射成 1（夜间晴天）
        let night: serde_json::Value = serde_json::from_str(
            r#"{"timezone": 0, "weather": [{"id": 800, "icon": "01n"}],
                "main": {"temp": 60.0, "feels_like": 58.0, "humidity": 70},
                "wind": {"speed": 2.0}, "sys": {}}"#,
        )
        .expect("样本 JSON 合法");
        let night = owm_current_state(&night, 0).expect("current 解析成功");
        assert!(!night.is_day);
        assert_eq!(night.code, 1);
        assert_eq!(night.desc, "大部晴朗");
        assert_eq!(night.icon, "moon");
        assert_eq!(night.sunrise, "");
        assert_eq!(night.sunset, "");

        // 没有 weather 字段：明确报错，交给上层回退
        let broken: serde_json::Value =
            serde_json::from_str(r#"{"main":{}}"#).expect("样本 JSON 合法");
        assert!(owm_current_state(&broken, 0).is_err());
    }

    /// OpenWeather `/data/2.5/forecast` 样本：3 小时间隔，跨 2023-11-15 / 16 两天。
    const OWM_FORECAST: &str = r#"{
        "timezone": 28800,
        "list": [
            {"dt": 1700000000, "main": {"temp": 18.5}, "weather": [{"id": 800, "icon": "01d"}], "pop": 0.1},
            {"dt": 1700010800, "main": {"temp": 17.0}, "weather": [{"id": 803, "icon": "04d"}], "pop": 0.2},
            {"dt": 1700021600, "main": {"temp": 16.0}, "weather": [{"id": 500, "icon": "10d"}], "pop": 0.5},
            {"dt": 1700032400, "main": {"temp": 15.0}, "weather": [{"id": 501, "icon": "10n"}], "pop": 0.7},
            {"dt": 1700043200, "main": {"temp": 14.0}, "weather": [{"id": 802, "icon": "03d"}], "pop": 0.0},
            {"dt": 1700054000, "main": {"temp": 13.0}, "weather": [{"id": 741, "icon": "50d"}], "pop": 0.3},
            {"dt": 1700064800, "main": {"temp": 12.5}, "weather": [{"id": 800, "icon": "01n"}], "pop": 0.0},
            {"dt": 1700075600, "main": {"temp": 11.0}, "weather": [{"id": 600, "icon": "13n"}], "pop": 0.4},
            {"dt": 1700079200, "main": {"temp": 10.0}, "weather": [{"id": 600, "icon": "13d"}], "pop": 0.6}
        ]
    }"#;

    #[test]
    fn owm_forecast_json_parses_hourly_and_groups_daily() {
        let j: serde_json::Value = serde_json::from_str(OWM_FORECAST).expect("样本 JSON 合法");
        let tz = j["timezone"].as_i64().unwrap_or(0);

        let hourly = owm_hourly(&j, tz);
        assert_eq!(hourly.len(), 8, "只取前 8 条（24 小时）");
        assert_eq!(hourly[0].time, "06:13");
        assert_eq!(hourly[0].label, "06:13");
        assert_eq!(hourly[0].temp, 18.5);
        assert_eq!(hourly[0].code, 0);
        assert_eq!(hourly[0].icon, "sun");
        assert_eq!(hourly[0].precip, 10.0, "pop 0.1 → 10%");
        assert_eq!(hourly[3].time, "15:13");
        assert_eq!(hourly[3].code, 63);
        assert_eq!(hourly[3].icon, "rain");
        assert_eq!(hourly[3].precip, 70.0);
        assert_eq!(hourly[6].time, "00:13", "跨本地日期仍取 dt + 时区偏移");
        assert_eq!(hourly[6].code, 1, "夜间 800 → 1");

        let daily = owm_daily(&j, tz);
        assert_eq!(daily.len(), 2, "样本跨两个本地日期");
        assert_eq!(daily[0].date, "2023-11-15");
        assert_eq!(daily[0].weekday, "周三");
        assert_eq!(daily[0].max, 18.5);
        assert_eq!(daily[0].min, 13.0);
        assert_eq!(daily[0].code, 3, "当天出现最多的 code（阴 2 次）");
        assert_eq!(daily[0].desc, "阴");
        assert_eq!(daily[1].date, "2023-11-16");
        assert_eq!(daily[1].weekday, "周四");
        assert_eq!(daily[1].max, 12.5);
        assert_eq!(daily[1].min, 10.0);
        assert_eq!(daily[1].code, 71, "小雪出现 2 次");
        assert_eq!(daily[1].desc, "小雪");
    }

    #[test]
    fn data_source_urls_carry_units_and_language() {
        assert_eq!(open_meteo_unit_params("metric"), "");
        assert!(open_meteo_unit_params("imperial").contains("temperature_unit=fahrenheit"));
        assert!(open_meteo_unit_params("imperial").contains("wind_speed_unit=mph"));

        let metric_url = open_meteo_forecast_url(30.27, 120.15, "metric");
        assert!(metric_url.contains("latitude=30.27"));
        assert!(metric_url.contains("longitude=120.15"));
        assert!(
            !metric_url.contains("temperature_unit"),
            "metric 走 Open-Meteo 默认单位"
        );
        let imperial_url = open_meteo_forecast_url(30.27, 120.15, "imperial");
        assert!(imperial_url.contains("temperature_unit=fahrenheit"));
        assert!(imperial_url.contains("wind_speed_unit=mph"));

        let provider = WeatherProvider {
            api_key: "abc123".into(),
            units: "imperial".into(),
            interval_min: 30,
        };
        assert!(provider.uses_openweather());
        let url = owm_url("weather", 30.27, 120.15, &provider);
        assert!(url.starts_with("https://api.openweathermap.org/data/2.5/weather?"));
        assert!(url.contains("lat=30.27"));
        assert!(url.contains("lon=120.15"));
        assert!(url.contains("appid=abc123"));
        assert!(url.contains("units=imperial"));
        assert!(url.contains("lang=zh_cn"));
        assert!(owm_url("forecast", 30.27, 120.15, &provider)
            .starts_with("https://api.openweathermap.org/data/2.5/forecast?"));

        let free = WeatherProvider {
            api_key: "  ".into(),
            units: "metric".into(),
            interval_min: 15,
        };
        assert!(!free.uses_openweather(), "Key 留空 / 仅空白 → Open-Meteo");
    }

    #[test]
    fn refresh_interval_follows_settings_and_clamps() {
        assert_eq!(refresh_secs(15), 900);
        assert_eq!(refresh_secs(5), 300);
        assert_eq!(refresh_secs(180), 10_800);
        assert_eq!(refresh_secs(1), 300, "低于 5 分钟按 5 分钟");
        assert_eq!(refresh_secs(999), 10_800, "高于 180 分钟按 180 分钟");
    }

    #[test]
    fn http_errors_distinguish_key_and_rate_limit() {
        let unauthorized = HttpError {
            status: Some(401),
            reason: "天气请求失败: HTTP 401".into(),
        };
        assert_eq!(unauthorized.weather_error(), "API Key 无效（HTTP 401）");
        let limited = HttpError {
            status: Some(429),
            reason: "天气请求失败: HTTP 429".into(),
        };
        assert_eq!(limited.weather_error(), "请求过于频繁（HTTP 429）");
        let network = HttpError {
            status: None,
            reason: "天气请求失败: io error".into(),
        };
        assert_eq!(network.weather_error(), "天气请求失败: io error");
    }

    #[test]
    fn brief_error_summarizes_network_failures() {
        assert_eq!(
            brief_error("天气请求失败: Connection Failed: tls connection init failed os error 10060"),
            "网络不可达"
        );
        assert_eq!(brief_error("天气请求失败: HTTP 429"), "请求过于频繁");
        assert_eq!(brief_error("失败: 找不到城市"), "失败: 找不到城市");
        assert_eq!(brief_error("   "), "网络错误");
    }

    #[test]
    fn stale_snapshot_backfills_failed_fetch() {
        let key = "测试快照城市";
        // 成功快照入栈。
        let good = WeatherCityState {
            name: key.into(),
            ok: true,
            updated_at: 111,
            temp: 26.0,
            desc: "晴".into(),
            icon: "sun".into(),
            lat: Some(30.0),
            lon: Some(120.0),
            hourly: vec![WeatherHour {
                label: "12时".into(),
                temp: 26.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        remember_snapshot(key, &good);

        // 失败时同名同坐标回填：数据来自快照，标记 stale + 简短错误。
        let mut failed = WeatherCityState {
            name: key.into(),
            ok: false,
            error: "天气请求失败: Connection Failed os error 10060".into(),
            lat: Some(30.0),
            lon: Some(120.0),
            ..Default::default()
        };
        merge_stale(key, &mut failed);
        assert!(failed.stale, "失败但命中快照 → stale");
        assert_eq!(failed.temp, 26.0, "温度回填");
        assert_eq!(failed.updated_at, 111, "时间戳回填");
        assert_eq!(failed.error_brief, "网络不可达");
        assert_eq!(failed.hourly.len(), 1);
        assert_eq!(failed.name, key, "身份字段不被快照覆盖");

        // 坐标变化太大（重新绑定）不复用。
        let mut moved = WeatherCityState {
            name: key.into(),
            ok: false,
            error: "失败".into(),
            lat: Some(40.0),
            lon: Some(130.0),
            ..Default::default()
        };
        merge_stale(key, &mut moved);
        assert!(!moved.stale, "坐标变化大 → 不回填");
    }
}
