//! 灵动岛歌词：网易云 / lrclib / 自定义接口三种歌词源。
//!
//! 源顺序由 `IslandConfig.media_lyrics_sources` 决定（逗号分隔，默认
//! `netease,lrclib,custom`）：按用户写的顺序逐个尝试，任一源拿到歌词即返回，
//! 全失败返回可读错误。结果缓存 30 分钟，缓存键包含源顺序与自定义 URL，
//! 改设置后不会命中旧结果。

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::normalize_lyrics_sources;

pub const LYRICS_TTL_SECS: u64 = 30 * 60;

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LyricsState {
    pub ok: bool,
    pub error: String,
    pub title: String,
    pub artist: String,
    pub lrc: String,
    pub translation: String,
    pub source: String,
}

/// 一次查询的入参（`album` / `duration` 只用于 lrclib 与自定义占位符）。
#[derive(Debug, Clone, Default)]
struct LyricsRequest {
    title: String,
    artist: String,
    album: String,
    duration: Option<f64>,
}

static CACHE: OnceLock<Mutex<HashMap<String, (Instant, LyricsState)>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, (Instant, LyricsState)>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AgentHUD-Island/1.0";

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

/// 尝试顺序（已归一化；空配置回退默认三源）。
fn source_order(raw: &str) -> Vec<String> {
    normalize_lyrics_sources(raw)
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// GET 文本；HTTP 4xx/5xx 也返回 `(status, body)` 由调用方决定怎么处理，
/// 只有传输层错误（超时 / DNS / TLS）才走 `Err`。
fn http_text(url: &str, referer: Option<&str>) -> Result<(u16, String), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .build();
    let mut request = agent.get(url).set("User-Agent", UA);
    if let Some(referer) = referer {
        request = request.set("Referer", referer);
    }
    match request.call() {
        Ok(resp) => {
            let status = resp.status();
            let text = resp
                .into_string()
                .map_err(|e| format!("读取歌词响应失败: {e}"))?;
            Ok((status, text))
        }
        Err(ureq::Error::Status(code, resp)) => Ok((code, resp.into_string().unwrap_or_default())),
        Err(e) => Err(format!("歌词请求失败: {e}")),
    }
}

/// lrclib `/api/get`：按 API 约定顺序拼参数，缺失项整段省略。
fn build_lrclib_get_url(title: &str, artist: &str, album: &str, duration: Option<f64>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !artist.trim().is_empty() {
        parts.push(format!("artist_name={}", encode(artist.trim())));
    }
    if !title.trim().is_empty() {
        parts.push(format!("track_name={}", encode(title.trim())));
    }
    if !album.trim().is_empty() {
        parts.push(format!("album_name={}", encode(album.trim())));
    }
    if let Some(seconds) = duration.filter(|d| d.is_finite() && *d > 0.0) {
        parts.push(format!("duration={}", seconds.round() as i64));
    }
    format!("https://lrclib.net/api/get?{}", parts.join("&"))
}

/// lrclib `/api/search`：`track_name` 必有（曲名缺失时不请求），`artist_name` 可选。
fn build_lrclib_search_url(title: &str, artist: &str) -> String {
    let mut parts = vec![format!("track_name={}", encode(title.trim()))];
    if !artist.trim().is_empty() {
        parts.push(format!("artist_name={}", encode(artist.trim())));
    }
    format!("https://lrclib.net/api/search?{}", parts.join("&"))
}

/// 把模板里的 `{title}` / `{artist}` / `{album}` / `{duration}` / `{song_id}`
/// 换成 URL 编码后的值；未知占位符原样保留（例如别人的 `{id}`）。
///
/// 单趟扫描而不是逐个 `replace`：曲名里如果正好带 `{artist}` 字样，逐个替换
/// 会被二次展开。
fn fill_url_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match tail.find('}') {
            Some(end) => {
                let name = &tail[1..end];
                match vars.iter().find(|(key, _)| *key == name) {
                    Some((_, value)) => out.push_str(&encode(value)),
                    None => out.push_str(&tail[..=end]),
                }
                rest = &tail[end + 1..];
            }
            None => {
                out.push_str(tail);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 纯文本歌词 → 伪 LRC：按行均分整首歌时长（缺时长时每行 3s），让前端现有的
/// 时间轴解析器也能显示「没有时间戳」的歌词。
fn plain_to_lrc(plain: &str, duration: Option<f64>) -> String {
    let lines: Vec<&str> = plain
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let span = duration
        .filter(|d| d.is_finite() && *d >= 10.0)
        .map(|d| (d / lines.len() as f64).clamp(1.5, 8.0))
        .unwrap_or(3.0);
    let mut out = String::new();
    for (index, text) in lines.iter().enumerate() {
        let at = span * index as f64;
        let minutes = (at / 60.0).floor() as i64;
        let seconds = at - minutes as f64 * 60.0;
        out.push_str(&format!("[{minutes:02}:{seconds:05.2}]{text}\n"));
    }
    out
}

fn json_str(value: &serde_json::Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(|v| v.as_str()) {
            if !text.trim().is_empty() {
                return text.to_string();
            }
        }
        // 允许一层 `{"data": {...}}` 包裹
        if let Some(text) = value
            .get("data")
            .and_then(|data| data.get(*key))
            .and_then(|v| v.as_str())
        {
            if !text.trim().is_empty() {
                return text.to_string();
            }
        }
    }
    String::new()
}

/// 自定义接口响应：先按 JSON 取 `lyrics` / `lrc` / `syncedLyrics`（可带
/// `translation` / `tlyric`），否则整段当 LRC 文本；纯文本再补伪时间轴。
fn parse_custom_body(text: &str) -> Result<(String, String), String> {
    let body = text.trim();
    if body.is_empty() {
        return Err("自定义接口返回空内容".into());
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        let mut lrc = json_str(&value, &["lyrics", "lrc", "syncedLyrics", "synced_lyrics"]);
        let translation = json_str(&value, &["translation", "tlyric", "trans"]);
        if lrc.trim().is_empty() {
            let plain = json_str(&value, &["plainLyrics", "plain_lyrics", "text"]);
            lrc = plain_to_lrc(&plain, None);
        }
        if lrc.trim().is_empty() {
            return Err("自定义接口没有歌词字段".into());
        }
        return Ok((lrc, translation));
    }
    if body.contains('[') {
        return Ok((body.to_string(), String::new()));
    }
    Ok((plain_to_lrc(body, None), String::new()))
}

fn lrclib_state_from(value: &serde_json::Value, req: &LyricsRequest) -> Option<LyricsState> {
    let synced = value
        .get("syncedLyrics")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let lrc = if synced.is_empty() {
        let plain = value
            .get("plainLyrics")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        plain_to_lrc(plain, req.duration)
    } else {
        synced
    };
    if lrc.trim().is_empty() {
        return None;
    }
    let title = value
        .get("trackName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(req.title.as_str())
        .to_string();
    let artist = value
        .get("artistName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(req.artist.as_str())
        .to_string();
    Some(LyricsState {
        ok: true,
        error: String::new(),
        title,
        artist,
        lrc,
        translation: String::new(),
        source: "lrclib".into(),
    })
}

// ============================================================
// 源 1：网易云（公开搜索 + 歌词接口）
// ============================================================

fn netease_json(url: &str) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .build();
    let resp = agent
        .get(url)
        .set("User-Agent", UA)
        .set("Referer", "https://music.163.com/")
        .set("Cookie", "appver=2.0.2; os=pc")
        .call()
        .map_err(|e| format!("网易云请求失败: {e}"))?;
    let text = resp
        .into_string()
        .map_err(|e| format!("读取网易云响应失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("解析网易云响应失败: {e}"))
}

/// 搜索第一首：`(song_id, 曲名, 歌手)`。
fn netease_search(title: &str, artist: &str) -> Result<(i64, String, String), String> {
    let query = format!("{} {}", title.trim(), artist.trim())
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("没有正在播放的曲目".into());
    }
    let search = netease_json(&format!(
        "https://music.163.com/api/search/get/web?s={}&type=1&offset=0&limit=1",
        encode(&query)
    ))?;
    let song = search
        .get("result")
        .and_then(|r| r.get("songs"))
        .and_then(|s| s.as_array())
        .and_then(|s| s.first())
        .ok_or_else(|| "网易云没有找到这首歌".to_string())?;
    let id = song
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "歌词歌曲 ID 缺失".to_string())?;
    let found_title = song
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(title)
        .to_string();
    let found_artist = song
        .get("artists")
        .or_else(|| song.get("ar"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or(artist)
        .to_string();
    Ok((id, found_title, found_artist))
}

fn fetch_netease(req: &LyricsRequest) -> Result<LyricsState, String> {
    let (id, found_title, found_artist) = netease_search(&req.title, &req.artist)?;
    let lyric = netease_json(&format!(
        "https://music.163.com/api/song/lyric?id={id}&lv=-1&kv=-1&tv=-1"
    ))?;
    let lrc = lyric
        .get("lrc")
        .and_then(|l| l.get("lyric"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if lrc.trim().is_empty() {
        return Err("这首歌暂时没有歌词".into());
    }
    let translation = lyric
        .get("tlyric")
        .and_then(|l| l.get("lyric"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(LyricsState {
        ok: true,
        error: String::new(),
        title: found_title,
        artist: found_artist,
        lrc,
        translation,
        source: "netease".into(),
    })
}

/// 只为了自定义接口的 `{song_id}` 占位符：网易云搜索一次拿 id，失败给 None。
fn netease_song_id(title: &str, artist: &str) -> Option<i64> {
    netease_search(title, artist).ok().map(|(id, _, _)| id)
}

// ============================================================
// 源 2：lrclib
// ============================================================

fn fetch_lrclib(req: &LyricsRequest) -> Result<LyricsState, String> {
    if req.title.trim().is_empty() {
        return Err("曲名缺失，无法查询 lrclib".into());
    }
    let url = build_lrclib_get_url(&req.title, &req.artist, &req.album, req.duration);
    let (status, text) = http_text(&url, None)?;
    if status == 200 {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(state) = lrclib_state_from(&value, req) {
                return Ok(state);
            }
        }
    }
    // `/api/get` 要求曲名 + 歌手精确命中，匹配不上时（404 或字段为空）再搜一次。
    let search_url = build_lrclib_search_url(&req.title, &req.artist);
    let (search_status, body) = http_text(&search_url, None)?;
    if search_status == 200 {
        if let Ok(serde_json::Value::Array(items)) =
            serde_json::from_str::<serde_json::Value>(&body)
        {
            // 优先真正带时间轴的，其次接受纯文本（内部补伪时间轴）。
            let mut fallback: Option<LyricsState> = None;
            for item in &items {
                let synced = item
                    .get("syncedLyrics")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if let Some(state) = lrclib_state_from(item, req) {
                    if !synced.is_empty() {
                        return Ok(state);
                    }
                    fallback.get_or_insert(state);
                }
            }
            if let Some(state) = fallback {
                return Ok(state);
            }
        }
    }
    Err(format!(
        "lrclib 没有这首歌的歌词（HTTP {status}/{}）",
        search_status
    ))
}

// ============================================================
// 源 3：自定义接口（URL 模板）
// ============================================================

fn fetch_custom(
    req: &LyricsRequest,
    template: &str,
    song_id: Option<i64>,
) -> Result<LyricsState, String> {
    let template = template.trim();
    if template.is_empty() {
        return Err("没有配置自定义歌词接口".into());
    }
    let duration = req
        .duration
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| format!("{}", d.round() as i64))
        .unwrap_or_default();
    let song_id = song_id.map(|id| id.to_string()).unwrap_or_default();
    let url = fill_url_template(
        template,
        &[
            ("title", req.title.trim()),
            ("artist", req.artist.trim()),
            ("album", req.album.trim()),
            ("duration", duration.as_str()),
            ("song_id", song_id.as_str()),
        ],
    );
    let (status, text) = http_text(&url, None)?;
    if status != 200 {
        return Err(format!("自定义歌词接口返回 HTTP {status}"));
    }
    let (lrc, translation) = parse_custom_body(&text)?;
    Ok(LyricsState {
        ok: true,
        error: String::new(),
        title: req.title.clone(),
        artist: req.artist.clone(),
        lrc,
        translation,
        source: "custom".into(),
    })
}

// ============================================================
// 调度 / 缓存 / 命令
// ============================================================

fn fetch_lyrics(
    req: &LyricsRequest,
    sources: &str,
    custom_url: &str,
) -> Result<LyricsState, String> {
    if req.title.trim().is_empty() {
        return Err("没有正在播放的曲目".into());
    }
    let order = source_order(sources);
    // 自定义接口的 {song_id} 只能问网易云要；只有真用到时才多打一次搜索。
    let wants_song_id =
        order.iter().any(|source| source == "custom") && custom_url.contains("{song_id}");
    let song_id = if wants_song_id {
        netease_song_id(&req.title, &req.artist)
    } else {
        None
    };
    let mut errors: Vec<String> = Vec::new();
    for source in &order {
        let attempt = match source.as_str() {
            "netease" => fetch_netease(req),
            "lrclib" => fetch_lrclib(req),
            "custom" => fetch_custom(req, custom_url, song_id),
            other => Err(format!("未知歌词源: {other}")),
        };
        match attempt {
            Ok(state) => return Ok(state),
            Err(error) => errors.push(format!("{source}: {error}")),
        }
    }
    Err(format!("歌词获取失败（{}）", errors.join("；")))
}

fn cache_key(req: &LyricsRequest, sources: &str, custom_url: &str) -> String {
    let duration = req
        .duration
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| (d.round() as i64).to_string())
        .unwrap_or_default();
    format!(
        "{}|{}|{}|{}|{}|{}",
        sources.trim().to_lowercase(),
        custom_url.trim(),
        req.title.trim().to_lowercase(),
        req.artist.trim().to_lowercase(),
        req.album.trim().to_lowercase(),
        duration
    )
}

fn get_lyrics(req: LyricsRequest) -> Result<LyricsState, String> {
    let cfg = super::load_config();
    let sources = normalize_lyrics_sources(&cfg.media_lyrics_sources);
    let custom_url = cfg.media_lyrics_custom_url.clone();
    let key = cache_key(&req, &sources, &custom_url);
    if let Ok(cache) = cache().lock() {
        if let Some((at, value)) = cache.get(&key) {
            if at.elapsed() < Duration::from_secs(LYRICS_TTL_SECS) {
                return Ok(value.clone());
            }
        }
    }
    let result = fetch_lyrics(&req, &sources, &custom_url);
    if let Ok(mut cache) = cache().lock() {
        if cache.len() > 60 {
            cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(LYRICS_TTL_SECS));
        }
        if let Ok(value) = &result {
            cache.insert(key, (Instant::now(), value.clone()));
        }
    }
    result
}

/// 按 `media.lyrics_sources` 的顺序取歌词（`album` / `duration` 供 lrclib 与
/// 自定义占位符使用，前端拿不到时传 null）。
#[tauri::command]
pub async fn island_lyrics(
    title: String,
    artist: String,
    album: Option<String>,
    duration: Option<f64>,
) -> Result<LyricsState, String> {
    let req = LyricsRequest {
        title,
        artist,
        album: album.unwrap_or_default(),
        duration,
    };
    tauri::async_runtime::spawn_blocking(move || get_lyrics(req))
        .await
        .map_err(|e| format!("歌词任务失败: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LyricsRequest {
        LyricsRequest {
            title: "夜曲".into(),
            artist: "周杰伦".into(),
            album: "十一月的萧邦".into(),
            duration: Some(226.5),
        }
    }

    #[test]
    fn source_order_follows_config_and_falls_back() {
        assert_eq!(
            source_order("netease,lrclib,custom"),
            ["netease", "lrclib", "custom"]
        );
        // 用户顺序优先，不按白名单固定排序
        assert_eq!(source_order(" custom , netease "), ["custom", "netease"]);
        // 去重 / 小写 / 非法丢弃
        assert_eq!(
            source_order("CUSTOM,custom,bogus,lrclib"),
            ["custom", "lrclib"]
        );
        // 全非法回默认三源顺序
        assert_eq!(source_order("bogus"), ["netease", "lrclib", "custom"]);
        assert_eq!(source_order(""), ["netease", "lrclib", "custom"]);
    }

    #[test]
    fn custom_url_placeholders_are_encoded() {
        let req = request();
        let duration = "226";
        let url = fill_url_template(
            "https://ly.example/api?t={title}&a={artist}&al={album}&d={duration}&id={song_id}",
            &[
                ("title", req.title.trim()),
                ("artist", req.artist.trim()),
                ("album", req.album.trim()),
                ("duration", duration),
                ("song_id", ""),
            ],
        );
        assert!(url.contains("%E5%A4%9C%E6%9B%B2"), "曲名要 URL 编码: {url}");
        assert!(url.contains("&d=226"));
        assert!(url.contains("&id="), "缺 song_id 时留空值: {url}");
        // 未知占位符原样保留
        assert_eq!(
            fill_url_template("https://x/{unknown}/{title}", &[("title", "a b")]),
            "https://x/{unknown}/a%20b"
        );
        // 曲名里带占位符字样不会被二次展开
        assert_eq!(
            fill_url_template(
                "https://x/{title}",
                &[("title", "{artist}"), ("artist", "nope")]
            ),
            "https://x/%7Bartist%7D"
        );
        // 未闭合的大括号不 panic
        assert_eq!(
            fill_url_template("https://x/{title", &[("title", "a")]),
            "https://x/{title"
        );
    }

    #[test]
    fn lrclib_urls_skip_missing_params() {
        assert_eq!(
            build_lrclib_get_url("夜曲", "周杰伦", "十一月的萧邦", Some(226.5)),
            "https://lrclib.net/api/get?artist_name=%E5%91%A8%E6%9D%B0%E4%BC%A6&track_name=%E5%A4%9C%E6%9B%B2&album_name=%E5%8D%81%E4%B8%80%E6%9C%88%E7%9A%84%E8%90%A7%E9%82%A6&duration=227"
        );
        // 缺歌手 / 专辑 / 时长时整段省略
        assert_eq!(
            build_lrclib_get_url("Night", "", "", None),
            "https://lrclib.net/api/get?track_name=Night"
        );
        // 非法时长（NaN / 负 / 0）不参与
        for bad in [f64::NAN, -1.0, 0.0] {
            assert_eq!(
                build_lrclib_get_url("A", "B", "C", Some(bad)),
                "https://lrclib.net/api/get?artist_name=B&track_name=A&album_name=C"
            );
        }
        assert_eq!(
            build_lrclib_search_url("Night", "Zhou"),
            "https://lrclib.net/api/search?track_name=Night&artist_name=Zhou"
        );
        assert_eq!(
            build_lrclib_search_url("Night", " "),
            "https://lrclib.net/api/search?track_name=Night"
        );
    }

    #[test]
    fn lrclib_json_prefers_synced_then_plain() {
        let req = request();
        let synced = serde_json::json!({
            "trackName": "夜曲",
            "artistName": "周杰伦",
            "syncedLyrics": "[00:01.00]一群嗜血的蚂蚁",
            "plainLyrics": "一群嗜血的蚂蚁"
        });
        let state = lrclib_state_from(&synced, &req).expect("应有歌词");
        assert_eq!(state.source, "lrclib");
        assert_eq!(state.lrc, "[00:01.00]一群嗜血的蚂蚁");

        // 只有 plainLyrics：补伪时间轴
        let plain = serde_json::json!({
            "syncedLyrics": null,
            "plainLyrics": "第一行\n第二行"
        });
        let state = lrclib_state_from(&plain, &req).expect("纯文本也要可用");
        assert!(state.lrc.starts_with("[00:00.00]第一行"));
        assert!(state.lrc.contains("第二行"));
        // 没有曲名 / 歌手时回退请求参数
        assert_eq!(state.title, "夜曲");
        assert_eq!(state.artist, "周杰伦");

        // 两个字段都空 → 不算命中
        let empty = serde_json::json!({ "syncedLyrics": "", "plainLyrics": "" });
        assert!(lrclib_state_from(&empty, &req).is_none());
    }

    #[test]
    fn custom_body_accepts_json_and_plain_text() {
        // 标准 JSON：lyrics 字段
        let (lrc, translation) =
            parse_custom_body(r#"{"lyrics":"[00:01.00]hello","translation":"[00:01.00]你好"}"#)
                .expect("JSON");
        assert_eq!(lrc, "[00:01.00]hello");
        assert_eq!(translation, "[00:01.00]你好");
        // lrclib 风格：syncedLyrics
        let (lrc, _) = parse_custom_body(r#"{"syncedLyrics":"[00:02.00]x"}"#).expect("synced");
        assert_eq!(lrc, "[00:02.00]x");
        // 一层 data 包裹
        let (lrc, _) = parse_custom_body(r#"{"data":{"lrc":"[00:03.00]y"}}"#).expect("data");
        assert_eq!(lrc, "[00:03.00]y");
        // 只有纯文本字段 → 补伪时间轴
        let (lrc, _) = parse_custom_body(r#"{"plainLyrics":"a\nb"}"#).expect("plain");
        assert!(lrc.starts_with("[00:00.00]a"));
        // 非 JSON 且带时间戳 → 原样
        let (lrc, _) = parse_custom_body("[00:09.00]直接给 LRC").expect("raw lrc");
        assert_eq!(lrc, "[00:09.00]直接给 LRC");
        // 非 JSON 纯文本 → 伪时间轴
        let (lrc, _) = parse_custom_body("第一句\n第二句").expect("raw plain");
        assert!(lrc.starts_with("[00:00.00]第一句"));
        // 空响应 / 没有可用字段都要报错
        assert!(parse_custom_body("   ").is_err());
        assert!(parse_custom_body(r#"{"foo":1}"#).is_err());
    }

    #[test]
    fn plain_text_gets_even_timeline() {
        let lrc = plain_to_lrc("a\nb\nc", Some(30.0));
        // 30s / 3 行 = 每行 10s，被 clamp 到 8s
        assert!(lrc.contains("[00:00.00]a"));
        assert!(lrc.contains("[00:08.00]b"));
        assert!(lrc.contains("[00:16.00]c"));
        // 缺时长时每行 3s
        let lrc = plain_to_lrc("a\nb", None);
        assert!(lrc.contains("[00:03.00]b"));
        // 空文本给空串（调用方据此判失败）
        assert!(plain_to_lrc("  \n\n ", None).is_empty());
    }

    #[test]
    fn cache_key_includes_sources_and_custom_url() {
        let req = request();
        let base = cache_key(&req, "netease", "https://x/{title}");
        assert_ne!(base, cache_key(&req, "lrclib", "https://x/{title}"));
        assert_ne!(base, cache_key(&req, "netease", "https://y/{title}"));
        // 大小写 / 首尾空格不影响命中
        assert_eq!(
            cache_key(&req, " Netease ", " https://x/{title} "),
            cache_key(&req, "netease", "https://x/{title}")
        );
        assert_ne!(base, cache_key(&req, "netease,lrclib", "https://x/{title}"));
    }

    #[test]
    fn empty_track_short_circuits() {
        let req = LyricsRequest {
            title: "  ".into(),
            ..LyricsRequest::default()
        };
        let err = fetch_lyrics(&req, "netease,lrclib,custom", "").expect_err("空曲名要报错");
        assert!(err.contains("没有正在播放的曲目"), "{err}");
    }
}
