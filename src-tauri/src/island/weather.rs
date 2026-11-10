//! 灵动岛天气：Open-Meteo（免费、无需 Key）当前天气 + 12 小时 + 5 天。
//!
//! 线程模型：启用时启动一条后台线程，立即取一拍后每 15 分钟刷新；前端
//! 主动刷新走 `refresh()`（spawn 一次性线程，`FETCHING` 防重入）。数据
//! 变化/刷新完成通过 `zbar://island-weather` 广播。

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const WEATHER_EVENT: &str = "zbar://island-weather";
const REFRESH_SECS: u64 = 15 * 60;

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static FETCHING: AtomicBool = AtomicBool::new(false);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static CACHE: OnceLock<Mutex<Option<WeatherState>>> = OnceLock::new();

fn app_slot() -> &'static Mutex<Option<AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
}

fn cache_slot() -> &'static Mutex<Option<WeatherState>> {
    CACHE.get_or_init(|| Mutex::new(None))
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

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WeatherState {
    pub ok: bool,
    pub error: String,
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
        return "上海".into();
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
/// ipwho.is（英文仍可用于天气查询）。前端用它定位到三级联动里对应的城市，
/// 同时保留经纬度，省掉一次 Open-Meteo geocoding。
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

fn location_json(url: &str) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();
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
                return Ok(LocatedPlace {
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
                });
            }
        }
    }
    let v = location_json("https://ipwho.is/")?;
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
    Ok(LocatedPlace {
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
    })
}

#[tauri::command]
pub async fn island_locate() -> Result<LocatedPlace, String> {
    tauri::async_runtime::spawn_blocking(locate_place)
        .await
        .map_err(|e| format!("定位任务失败: {e}"))?
}

fn encode(s: &str) -> String {    let mut out = String::new();
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

fn http_json(url: &str) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .build();
    let resp = agent
        .get(url)
        .set("User-Agent", "AgentHUD-Island/1.0")
        .call()
        .map_err(|e| format!("天气请求失败: {e}"))?;
    let text = resp
        .into_string()
        .map_err(|e| format!("读取天气响应失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("解析天气响应失败: {e}"))
}

fn fetch_weather() -> Result<WeatherState, String> {
    let mut cfg = super::load_config();
    let city = cfg.weather_city.clone();
    let display_city = if cfg.weather_label.trim().is_empty() {
        city.clone()
    } else {
        cfg.weather_label.trim().to_string()
    };
    let (lat, lon) = match (cfg.weather_lat, cfg.weather_lon) {
        (Some(lat), Some(lon)) if lat.is_finite() && lon.is_finite() => (lat, lon),
        _ => {
            // 先按完整市名查；Open-Meteo 对“杭州市”这类带行政后缀的名字
            // 经常无结果，再退回去后缀形态（“杭州”）试一次。
            let stripped = geocode_query(&city);
            let mut attempts = vec![city.clone()];
            if stripped != city {
                attempts.push(stripped);
            }
            let mut found: Option<(f64, f64)> = None;
            let mut last_err = String::new();
            for query in attempts {
                let geo_url = format!(
                    "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=zh&format=json",
                    encode(&query)
                );
                match http_json(&geo_url) {
                    Ok(geo) => {
                        if let Some(first) = geo
                            .get("results")
                            .and_then(|r| r.as_array())
                            .and_then(|r| r.first())
                        {
                            found = Some((num(first, "latitude"), num(first, "longitude")));
                            break;
                        }
                    }
                    Err(e) => last_err = e,
                }
            }
            let (lat, lon) = found.ok_or_else(|| {
                if last_err.is_empty() {
                    format!("找不到城市「{city}」")
                } else {
                    format!("找不到城市「{city}」: {last_err}")
                }
            })?;
            cfg.weather_lat = Some(lat);
            cfg.weather_lon = Some(lon);
            let _ = super::save_config(&cfg);
            (lat, lon)
        }
    };

    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,precipitation,weather_code,wind_speed_10m\
         &hourly=temperature_2m,weather_code,precipitation_probability,is_day\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset\
         &timezone=auto&forecast_days=6"
    );
    let j = http_json(&url)?;
    let current = j.get("current").cloned().unwrap_or(serde_json::Value::Null);
    let code = int(&current, "weather_code");
    let is_day = int(&current, "is_day") != 0;
    let (desc, icon) = wmo(code, is_day);
    let mut state = WeatherState {
        ok: true,
        error: String::new(),
        city: display_city,
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

fn publish(app: &AppHandle, state: WeatherState) {
    if let Ok(mut guard) = cache_slot().lock() {
        *guard = Some(state.clone());
    }
    let _ = app.emit(WEATHER_EVENT, state);
}

fn fetch_and_publish() {
    if STOP.load(Ordering::Relaxed) {
        return;
    }
    if FETCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let result = fetch_weather();
    FETCHING.store(false, Ordering::SeqCst);
    let app = app_slot().lock().ok().and_then(|g| g.clone());
    let Some(app) = app else { return };
    match result {
        Ok(state) => publish(&app, state),
        Err(error) => {
            let city = super::load_config().weather_city;
            let old = cache_slot().lock().ok().and_then(|g| g.clone());
            let mut state = old.unwrap_or_default();
            state.ok = false;
            state.error = error;
            state.city = city;
            publish(&app, state);
        }
    }
}

/// 启动后台刷新线程（幂等）
pub fn start(app: AppHandle) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(app.clone());
    }
    STOP.store(false, Ordering::SeqCst);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    thread::Builder::new()
        .name("zbar-island-weather".into())
        .spawn(move || {
            fetch_and_publish();
            let mut last = Instant::now();
            while !STOP.load(Ordering::Relaxed) && GEN.load(Ordering::SeqCst) == gen {
                thread::sleep(Duration::from_millis(500));
                if last.elapsed() >= Duration::from_secs(REFRESH_SECS) {
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

/// 立即刷新（配置变更 / 用户点刷新）
pub fn refresh() {
    if STOP.load(Ordering::Relaxed) {
        return;
    }
    thread::Builder::new()
        .name("zbar-island-weather-refresh".into())
        .spawn(fetch_and_publish)
        .ok();
}

#[tauri::command]
pub fn island_weather() -> Option<WeatherState> {
    cache_slot().lock().ok().and_then(|g| g.clone())
}

#[tauri::command]
pub fn island_weather_refresh() -> Result<(), String> {
    refresh();
    Ok(())
}
