//! 灵动岛股票：腾讯财经公开接口（行情 / 分时 / 搜索）。
//!
//! 接口返回 GBK 编码文本，用 encoding_rs 解码；后台线程按配置间隔轮询，
//! 只在内容变化时广播。A 股 / 港股 / 美股统一以 `sh/sz/bj/hk/us` 前缀标识。

use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const STOCKS_EVENT: &str = "zbar://island-stocks";
pub const STOCK_ALERT_EVENT: &str = "zbar://island-stock-alert";

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static FORCE: AtomicBool = AtomicBool::new(true);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static CACHE: OnceLock<Mutex<Option<StocksState>>> = OnceLock::new();

fn app_slot() -> &'static Mutex<Option<AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
}
fn cache_slot() -> &'static Mutex<Option<StocksState>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StockQuote {
    pub code: String,
    pub name: String,
    pub price: f64,
    pub prev_close: f64,
    pub open: f64,
    pub chg: f64,
    pub pct: f64,
    pub high: f64,
    pub low: f64,
    pub time: String,
    pub market: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StocksState {
    pub ok: bool,
    pub error: String,
    pub updated_at: i64,
    pub quotes: Vec<StockQuote>,
}

/// 规范化代码：SH600519 → sh600519；usAAPL.OQ → usAAPL
pub fn norm_code(code: &str) -> String {
    let mut c = code.trim().to_string();
    if let Some(prefix) = c.get(..2) {
        if matches!(prefix.to_ascii_lowercase().as_str(), "sh" | "sz" | "bj" | "hk" | "us") {
            c = format!("{}{}", prefix.to_ascii_lowercase(), &c[2..]);
        }
    }
    if c.starts_with("us") {
        c = format!("us{}", c[2..].split('.').next().unwrap_or("").to_uppercase());
    }
    c
}

pub fn valid_code(code: &str) -> bool {
    let c = norm_code(code);
    let Some(prefix) = c.get(..2) else {
        return false;
    };
    if !matches!(prefix, "sh" | "sz" | "bj" | "hk" | "us") {
        return false;
    }
    let rest = &c[2..];
    !rest.is_empty()
        && rest.len() <= 12
        && rest.chars().all(|ch| ch.is_ascii_alphanumeric())
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

fn http_text(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();
    let resp = agent
        .get(url)
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AgentHUD-Island/1.0")
        .set("Referer", "https://gu.qq.com/")
        .call()
        .map_err(|e| format!("行情请求失败: {e}"))?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("读取行情响应失败: {e}"))?;
    let (text, _, _) = encoding_rs::GBK.decode(&bytes);
    Ok(text.to_string())
}

fn num(v: &str) -> f64 {
    v.trim().parse::<f64>().unwrap_or(0.0)
}

fn parse_quotes(text: &str) -> Vec<StockQuote> {
    let mut out = Vec::new();
    for line in text.split(';') {
        let line = line.trim();
        if !line.starts_with("v_") {
            continue;
        }
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let fields: Vec<&str> = rest.trim().trim_matches('"').split('~').collect();
        if fields.len() < 35 {
            continue;
        }
        // 腾讯返回的键大小写不固定（美股常见 v_usAAPL）；统一走 norm_code，
        // 避免缓存里的 usAAPL 与配置里的 usAAPL 因大小写对不上。
        let code = norm_code(key.trim_start_matches("v_"));
        let market = code.get(..2).unwrap_or("").to_string();
        out.push(StockQuote {
            name: fields[1].to_string(),
            price: num(fields[3]),
            prev_close: num(fields[4]),
            open: num(fields[5]),
            chg: num(fields[31]),
            pct: num(fields[32]),
            high: num(fields[33]),
            low: num(fields[34]),
            time: fields[30].to_string(),
            market: market.clone(),
            status: market_status(&market),
            code,
        });
    }
    out
}

fn in_window(hm: u32, start: u32, end: u32) -> bool {
    if start <= end {
        (start..end).contains(&hm)
    } else {
        hm >= start || hm < end
    }
}

/// 简单交易时段判断（北京时间；美股按北京时间近似，未处理夏令时细节）
fn market_status(market: &str) -> String {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let wd = now.weekday().num_days_from_monday();
    let hm = now.hour() * 100 + now.minute();
    match market {
        "sh" | "sz" | "bj" => {
            if wd >= 5 {
                "休市".into()
            } else if in_window(hm, 930, 1130) || in_window(hm, 1300, 1500) {
                "交易中".into()
            } else if in_window(hm, 1130, 1300) {
                "午间休市".into()
            } else {
                "已收盘".into()
            }
        }
        "hk" => {
            if wd >= 5 {
                "休市".into()
            } else if in_window(hm, 930, 1200) || in_window(hm, 1300, 1600) {
                "交易中".into()
            } else if in_window(hm, 1200, 1300) {
                "午间休市".into()
            } else {
                "已收盘".into()
            }
        }
        "us" => {
            let (start, end) = if (3..=10).contains(&now.month()) {
                (2130, 400)
            } else {
                (2230, 500)
            };
            if in_window(hm, start, end) && wd < 5 {
                "交易中".into()
            } else {
                "已收盘".into()
            }
        }
        _ => "已收盘".into(),
    }
}

fn unescape_unicode(s: &str) -> String {
    if !s.contains("\\u") {
        return s.to_string();
    }
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'u') {
            chars.next();
            let mut hex = String::new();
            for _ in 0..4 {
                match chars.peek() {
                    Some(c) if c.is_ascii_hexdigit() => {
                        hex.push(*c);
                        chars.next();
                    }
                    _ => break,
                }
            }
            if hex.len() == 4 {
                if let Ok(code) = u32::from_str_radix(&hex, 16) {
                    if let Some(c) = char::from_u32(code) {
                        out.push(c);
                        continue;
                    }
                }
            }
            out.push_str("\\u");
            out.push_str(&hex);
            continue;
        }
        out.push(ch);
    }
    out
}

fn fetch_quotes(codes: &[String]) -> Result<Vec<StockQuote>, String> {
    if codes.is_empty() {
        return Ok(Vec::new());
    }
    let list = codes.join(",");
    let text = http_text(&format!("https://qt.gtimg.cn/q={list}"))?;
    let quotes = parse_quotes(&text);
    if quotes.is_empty() {
        return Err("行情接口未返回数据".into());
    }
    Ok(quotes)
}

fn fingerprint(quotes: &[StockQuote]) -> u64 {
    let mut h = DefaultHasher::new();
    for q in quotes {
        q.code.hash(&mut h);
        q.price.to_bits().hash(&mut h);
        q.pct.to_bits().hash(&mut h);
        q.status.hash(&mut h);
    }
    h.finish()
}

fn check_alerts(app: &AppHandle, quotes: &[StockQuote], threshold: f64, seen: &mut HashSet<String>) {
    for q in quotes {
        if q.pct.abs() < threshold {
            seen.remove(&q.code);
            continue;
        }
        if seen.insert(q.code.clone()) {
            let _ = app.emit(
                STOCK_ALERT_EVENT,
                serde_json::json!({
                    "code": q.code,
                    "name": q.name,
                    "pct": q.pct,
                    "price": q.price,
                    "text": format!("{} {:.2}%", if q.pct >= 0.0 { "涨" } else { "跌" }, q.pct.abs()),
                }),
            );
        }
    }
}

fn fetch_and_publish(force_alert: bool) {
    let cfg = super::load_config();
    let codes = cfg.stocks.clone();
    let threshold = cfg.stock_alert_pct;
    let result = fetch_quotes(&codes);
    let app = app_slot().lock().ok().and_then(|g| g.clone());
    let Some(app) = app else { return };
    let old = cache_slot().lock().ok().and_then(|g| g.clone());
    let old_fp = old
        .as_ref()
        .map(|s| {
            let mut h = DefaultHasher::new();
            for q in &s.quotes {
                q.code.hash(&mut h);
                q.price.to_bits().hash(&mut h);
                q.pct.to_bits().hash(&mut h);
                q.status.hash(&mut h);
            }
            h.finish()
        })
        .unwrap_or(0);
    match result {
        Ok(quotes) => {
            let fp = fingerprint(&quotes);
            let state = StocksState {
                ok: true,
                error: String::new(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                quotes,
            };
            if let Ok(mut guard) = cache_slot().lock() {
                *guard = Some(state.clone());
            }
            if fp != old_fp || force_alert {
                let _ = app.emit(STOCKS_EVENT, state.clone());
            }
            // 提醒每条只弹一次（涨/跌回到阈值内后重新武装）
            static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
            if let Ok(mut seen) = SEEN.get_or_init(|| Mutex::new(HashSet::new())).lock() {
                check_alerts(&app, &state.quotes, threshold, &mut seen);
            }
        }
        Err(error) => {
            let mut state = old.unwrap_or_default();
            state.ok = false;
            state.error = error;
            if let Ok(mut guard) = cache_slot().lock() {
                *guard = Some(state.clone());
            }
            let _ = app.emit(STOCKS_EVENT, state);
        }
    }
}

/// 启动轮询线程
pub fn start(app: AppHandle) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(app.clone());
    }
    STOP.store(false, Ordering::SeqCst);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    thread::Builder::new()
        .name("zbar-island-stocks".into())
        .spawn(move || {
            let mut last = Instant::now() - Duration::from_secs(3600);
            while !STOP.load(Ordering::Relaxed) && GEN.load(Ordering::SeqCst) == gen {
                let cfg = super::load_config();
                let interval = Duration::from_secs(cfg.stock_interval_sec.clamp(3, 60));
                if FORCE.swap(false, Ordering::SeqCst) || last.elapsed() >= interval {
                    last = Instant::now();
                    fetch_and_publish(false);
                }
                thread::sleep(Duration::from_millis(400));
            }
        })
        .ok();
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    GEN.fetch_add(1, Ordering::SeqCst);
}

/// 配置变化后立即刷新
pub fn refresh_now() {
    FORCE.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn island_stocks() -> Option<StocksState> {
    cache_slot().lock().ok().and_then(|g| g.clone())
}

#[tauri::command]
pub fn island_stocks_refresh() -> Result<(), String> {
    refresh_now();
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockSearchItem {
    pub code: String,
    pub name: String,
}

#[tauri::command]
pub async fn island_stock_search(query: String) -> Result<Vec<StockSearchItem>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let text = http_text(&format!(
            "https://smartbox.gtimg.cn/s3/?v=2&q={}&t=all",
            encode(q)
        ))?;
        let body = text
            .split_once('=')
            .map(|(_, v)| v)
            .unwrap_or("")
            .trim()
            .trim_end_matches(';')
            .trim_matches('"');
        let mut out = Vec::new();
        for item in body.split('^') {
            let f: Vec<&str> = item.split('~').collect();
            if f.len() < 3 || !matches!(f[0], "sh" | "sz" | "bj" | "hk" | "us") {
                continue;
            }
            let code = norm_code(&format!("{}{}", f[0], f[1]));
            if valid_code(&code) && !out.iter().any(|i: &StockSearchItem| i.code == code) {
                out.push(StockSearchItem {
                    code,
                    name: unescape_unicode(f[2]),
                });
            }
            if out.len() >= 12 {
                break;
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("搜索任务失败: {e}"))?
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StockMinute {
    pub code: String,
    pub prev_close: f64,
    pub points: Vec<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn us_codes_keep_canonical_case() {
        // 腾讯行情键大小写不固定，配置和缓存必须归一到同一形态。
        assert_eq!(norm_code("usaapl"), "usAAPL");
        assert_eq!(norm_code("usAAPL.OQ"), "usAAPL");
        assert_eq!(norm_code("SH600519"), "sh600519");
    }

    #[test]
    fn normed_codes_are_stable() {
        for raw in ["usNVDA", "usnvda", "usNVDA.OQ"] {
            assert_eq!(norm_code(&norm_code(raw)), "usNVDA");
        }
    }
}

#[tauri::command]
pub async fn island_stock_minute(code: String) -> Result<StockMinute, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let code = norm_code(&code);
        if !valid_code(&code) {
            return Err("股票代码非法".into());
        }
        let text = http_text(&format!(
            "https://web.ifzq.gtimg.cn/appstock/app/minute/query?code={code}"
        ))?;
        let j: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("解析分时失败: {e}"))?;
        let node = j
            .get("data")
            .and_then(|d| d.get(&code))
            .and_then(|d| d.get("data"))
            .ok_or_else(|| "分时接口未返回数据".to_string())?;
        let prev_close = j
            .get("data")
            .and_then(|d| d.get(&code))
            .and_then(|d| d.get("qt"))
            .and_then(|q| q.get(&code))
            .and_then(|q| q.as_array())
            .and_then(|a| a.get(4))
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let mut points = Vec::new();
        if let Some(rows) = node.get("data").and_then(|d| d.as_array()) {
            for row in rows {
                let Some(s) = row.as_str() else { continue };
                if let Some(price) = s.split_whitespace().nth(1) {
                    if let Ok(v) = price.parse::<f64>() {
                        points.push(v);
                    }
                }
            }
        }
        Ok(StockMinute {
            code,
            prev_close,
            points,
        })
    })
    .await
    .map_err(|e| format!("分时任务失败: {e}"))?
}
