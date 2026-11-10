//! 灵动岛媒体：Windows SMTC（GlobalSystemMediaTransportControls）会话。
//!
//! 支持读取当前播放器 / 标题 / 歌手 / 专辑 / 封面 / 进度 / 播放状态，并
//! 下发 播放暂停 / 上一首 / 下一首 / 跳转。非 Windows 平台返回空状态，
//! 保证跨平台编译。

use base64::Engine;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub const MEDIA_EVENT: &str = "zbar://island-media";

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static CACHE: OnceLock<Mutex<Option<MediaState>>> = OnceLock::new();
static COVER_CACHE: OnceLock<Mutex<(String, String)>> = OnceLock::new();

fn app_slot() -> &'static Mutex<Option<AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
}
fn cache_slot() -> &'static Mutex<Option<MediaState>> {
    CACHE.get_or_init(|| Mutex::new(None))
}
fn cover_slot() -> &'static Mutex<(String, String)> {
    COVER_CACHE.get_or_init(|| Mutex::new((String::new(), String::new())))
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaState {
    pub has_session: bool,
    pub app_id: String,
    pub app_name: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub can_prev: bool,
    pub can_next: bool,
    pub cover: String,
    pub source: String,
}

#[cfg(target_os = "windows")]
fn friendly_name(app_id: &str) -> String {
    let lower = app_id.to_ascii_lowercase();
    for (needle, name) in [
        ("cloudmusic", "网易云音乐"),
        ("spotify", "Spotify"),
        ("qqmusic", "QQ音乐"),
        ("kugou", "酷狗音乐"),
        ("bilibili", "哔哩哔哩"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("firefox", "Firefox"),
        ("foobar", "foobar2000"),
        ("potplayer", "PotPlayer"),
        ("vlc", "VLC"),
        ("music.ui", "音乐"),
    ] {
        if lower.contains(needle) {
            return name.to_string();
        }
    }
    app_id
        .rsplit('.')
        .next()
        .unwrap_or(app_id)
        .trim_end_matches(".exe")
        .to_string()
}

#[cfg(target_os = "windows")]
fn ticks_to_secs(ticks: i64) -> f64 {
    ticks.max(0) as f64 / 10_000_000.0
}

#[cfg(target_os = "windows")]
fn read_cover(
    props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
    key: &str,
) -> String {
    let mut cache = match cover_slot().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if cache.0 == key {
        return cache.1.clone();
    }
    let mut cover = String::new();
    if let Ok(thumb) = props.Thumbnail() {
        if let Ok(op) = thumb.OpenReadAsync() {
            if let Ok(stream) = op.get() {
                if let Ok(size) = stream.Size() {
                    let size = size.min(8 * 1024 * 1024) as u32;
                    if let Ok(reader) = windows::Storage::Streams::DataReader::CreateDataReader(&stream) {
                        if let Ok(load) = reader.LoadAsync(size) {
                            if load.get().is_ok() {
                                let mut buf = vec![0u8; size as usize];
                                if reader.ReadBytes(&mut buf).is_ok() {
                                    let mime = if buf.starts_with(&[0x89, b'P', b'N', b'G']) {
                                        "image/png"
                                    } else if buf.starts_with(&[0xFF, 0xD8]) {
                                        "image/jpeg"
                                    } else if buf.starts_with(b"GIF8") {
                                        "image/gif"
                                    } else if buf.starts_with(b"RIFF") {
                                        "image/webp"
                                    } else {
                                        "image/jpeg"
                                    };
                                    cover = format!(
                                        "data:{mime};base64,{}",
                                        base64::engine::general_purpose::STANDARD.encode(&buf)
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    cache.0 = key.to_string();
    cache.1 = cover.clone();
    cover
}

#[cfg(target_os = "windows")]
fn snapshot() -> MediaState {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };
    let mut state = MediaState::default();
    let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|op| op.get())
    {
        Ok(m) => m,
        Err(_) => return state,
    };
    let session = match manager.GetCurrentSession() {
        Ok(s) => s,
        Err(_) => return state,
    };
    state.has_session = true;
    state.source = "smtc".into();
    state.app_id = session
        .SourceAppUserModelId()
        .map(|s| s.to_string())
        .unwrap_or_default();
    state.app_name = friendly_name(&state.app_id);
    if let Ok(props) = session.TryGetMediaPropertiesAsync().and_then(|op| op.get()) {
        state.title = props.Title().map(|s| s.to_string()).unwrap_or_default();
        state.artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
        state.album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
        let key = format!("{}|{}|{}", state.app_id, state.title, state.artist);
        state.cover = read_cover(&props, &key);
    }
    if let Ok(info) = session.GetPlaybackInfo() {
        state.playing = info
            .PlaybackStatus()
            .map(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
            .unwrap_or(false);
        if let Ok(controls) = info.Controls() {
            state.can_next = controls.IsNextEnabled().unwrap_or(true);
            state.can_prev = controls.IsPreviousEnabled().unwrap_or(true);
        }
    }
    if let Ok(tl) = session.GetTimelineProperties() {
        let start = tl.StartTime().map(|t| t.Duration).unwrap_or(0);
        let pos = tl.Position().map(|t| t.Duration).unwrap_or(0);
        let end = tl.EndTime().map(|t| t.Duration).unwrap_or(0);
        state.position = ticks_to_secs(pos - start);
        state.duration = ticks_to_secs(end - start);
    }
    state
}

#[cfg(not(target_os = "windows"))]
fn snapshot() -> MediaState {
    MediaState::default()
}

fn publish(app: &AppHandle, state: MediaState) {
    let changed = {
        let guard = cache_slot().lock().ok().and_then(|g| g.clone());
        match guard {
            Some(old) => {
                old.title != state.title
                    || old.artist != state.artist
                    || old.playing != state.playing
                    || old.cover != state.cover
                    || (old.position - state.position).abs() > 1.5
                    || old.duration != state.duration
            }
            None => true,
        }
    };
    if let Ok(mut guard) = cache_slot().lock() {
        *guard = Some(state.clone());
    }
    if changed {
        let _ = app.emit(MEDIA_EVENT, state);
    }
}

/// 启动媒体轮询线程
pub fn start(app: AppHandle) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(app.clone());
    }
    STOP.store(false, Ordering::SeqCst);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    thread::Builder::new()
        .name("zbar-island-media".into())
        .spawn(move || {
            while !STOP.load(Ordering::Relaxed) && GEN.load(Ordering::SeqCst) == gen {
                let state = snapshot();
                if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
                    publish(&app, state);
                }
                thread::sleep(Duration::from_millis(1500));
            }
        })
        .ok();
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    GEN.fetch_add(1, Ordering::SeqCst);
}

#[tauri::command]
pub fn island_media() -> MediaState {
    snapshot()
}

#[cfg(target_os = "windows")]
fn apply_action(action: &str) -> Result<(), String> {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|op| op.get())
        .map_err(|e| format!("媒体会话不可用: {e}"))?;
    let session = manager
        .GetCurrentSession()
        .map_err(|_| "当前没有媒体会话".to_string())?;
    match action {
        "toggle" => {
            let playing = session
                .GetPlaybackInfo()
                .and_then(|i| i.PlaybackStatus())
                .map(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
                .unwrap_or(false);
            let op = if playing {
                session.TryPauseAsync()
            } else {
                session.TryPlayAsync()
            }
            .map_err(|e| e.to_string())?;
            op.get().map_err(|e| e.to_string())?;
        }
        "next" => {
            session
                .TrySkipNextAsync()
                .map_err(|e| e.to_string())?
                .get()
                .map_err(|e| e.to_string())?;
        }
        "prev" => {
            session
                .TrySkipPreviousAsync()
                .map_err(|e| e.to_string())?
                .get()
                .map_err(|e| e.to_string())?;
        }
        _ => return Err("未知媒体动作".into()),
    }
    // 动作后立即刷新一次，前端不用等下一拍
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        publish(&app, snapshot());
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn apply_action(_action: &str) -> Result<(), String> {
    Err("当前平台不支持系统媒体控制".into())
}

#[tauri::command]
pub async fn island_media_action(action: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || apply_action(&action))
        .await
        .map_err(|e| format!("媒体任务失败: {e}"))?
}
