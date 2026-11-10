//! 灵动岛歌词：网易云音乐公开歌词接口。
//!
//! 只做「按曲名 + 歌手搜索第一首 → 取 LRC + 翻译」，结果缓存 30 分钟；
//! 没有版权 / 接口失败时返回可读错误，前端静默不显示歌词即可。

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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

static CACHE: OnceLock<Mutex<HashMap<String, (Instant, LyricsState)>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, (Instant, LyricsState)>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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

fn http_json(url: &str) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .build();
    let resp = agent
        .get(url)
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AgentHUD-Island/1.0")
        .set("Referer", "https://music.163.com/")
        .set("Cookie", "appver=2.0.2; os=pc")
        .call()
        .map_err(|e| format!("歌词请求失败: {e}"))?;
    let text = resp
        .into_string()
        .map_err(|e| format!("读取歌词响应失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("解析歌词响应失败: {e}"))
}

fn fetch_lyrics(title: &str, artist: &str) -> Result<LyricsState, String> {
    let query = format!("{} {}", title.trim(), artist.trim())
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("没有正在播放的曲目".into());
    }
    let search = http_json(&format!(
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
    let lyric = http_json(&format!(
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

#[tauri::command]
pub async fn island_lyrics(title: String, artist: String) -> Result<LyricsState, String> {
    let key = format!("{}|{}", title.trim().to_lowercase(), artist.trim().to_lowercase());
    if let Ok(cache) = cache().lock() {
        if let Some((at, value)) = cache.get(&key) {
            if at.elapsed() < Duration::from_secs(30 * 60) {
                return Ok(value.clone());
            }
        }
    }
    let result = tauri::async_runtime::spawn_blocking(move || fetch_lyrics(&title, &artist))
        .await
        .map_err(|e| format!("歌词任务失败: {e}"))?;
    if let Ok(mut cache) = cache().lock() {
        if cache.len() > 60 {
            cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(30 * 60));
        }
        if let Ok(value) = &result {
            cache.insert(key, (Instant::now(), value.clone()));
        }
    }
    result
}
