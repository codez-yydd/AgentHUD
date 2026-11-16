//! 灵动岛媒体：Windows SMTC 会话和本地播放日志。
//!
//! 支持读取当前播放器 / 标题 / 歌手 / 专辑 / 封面 / 进度 / 播放状态，并
//! 下发 播放暂停 / 上一首 / 下一首 / 跳转。非 Windows 平台返回空状态，
//! 保证跨平台编译。

#[cfg(target_os = "windows")]
#[path = "media/netease.rs"]
mod netease;
#[cfg(target_os = "windows")]
#[path = "media/playback_log.rs"]
mod playback_log;

#[cfg(target_os = "windows")]
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
    pub can_toggle: bool,
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
        ("kuwo", "酷我音乐"),
        ("applemusic", "Apple Music"),
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
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(app_id)
        .trim_end_matches(".exe")
        .to_string()
}

#[cfg(target_os = "windows")]
fn priority_rank(priority: &[String], app_id: &str, name: &str) -> usize {
    let app_id = app_id.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    priority
        .iter()
        .position(|item| {
            let item = item.to_ascii_lowercase();
            app_id.contains(&item) || name.contains(&item)
        })
        .unwrap_or(usize::MAX)
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
                    if let Ok(reader) =
                        windows::Storage::Streams::DataReader::CreateDataReader(&stream)
                    {
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
fn session_playing(
    session: &windows::Media::Control::GlobalSystemMediaTransportControlsSession,
) -> bool {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus;
    session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus())
        .map(|status| status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
        .unwrap_or(false)
}

/// 按 `media_priority` 挑一个会话：按配置顺序找第一个「正在播放且 appId /
/// 友好名命中」的；配置为空、没有会话或都匹配不上时回退系统当前会话（原行为）。
#[cfg(target_os = "windows")]
fn pick_session(
    manager: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager,
    priority: &[String],
) -> Option<windows::Media::Control::GlobalSystemMediaTransportControlsSession> {
    if !priority.is_empty() {
        if let Ok(sessions) = manager.GetSessions() {
            let preferred = (0..sessions.Size().unwrap_or(0))
                .filter_map(|index| sessions.GetAt(index).ok())
                .filter(session_playing)
                .filter_map(|session| {
                    let app_id = session.SourceAppUserModelId().ok()?.to_string();
                    let rank = priority_rank(priority, &app_id, &friendly_name(&app_id));
                    (rank != usize::MAX).then_some((rank, session))
                })
                .min_by_key(|(rank, _)| *rank);
            if let Some((_, session)) = preferred {
                return Some(session);
            }
        }
    }
    manager.GetCurrentSession().ok()
}

#[cfg(target_os = "windows")]
fn snapshot_from(
    session: windows::Media::Control::GlobalSystemMediaTransportControlsSession,
) -> MediaState {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus;
    let mut state = MediaState::default();
    state.has_session = true;
    state.can_toggle = true;
    state.source = "smtc".into();
    state.app_id = session
        .SourceAppUserModelId()
        .map(|s| s.to_string())
        .unwrap_or_default();
    state.app_name = friendly_name(&state.app_id);
    if let Ok(props) = session.TryGetMediaPropertiesAsync().and_then(|op| op.get()) {
        state.title = props.Title().map(|s| s.to_string()).unwrap_or_default();
        state.artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
        state.album = props
            .AlbumTitle()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let key = format!("{}|{}|{}", state.app_id, state.title, state.artist);
        state.cover = read_cover(&props, &key);
    }
    if let Ok(info) = session.GetPlaybackInfo() {
        state.playing = info
            .PlaybackStatus()
            .map(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
            .unwrap_or(false);
        if let Ok(controls) = info.Controls() {
            state.can_toggle = controls.IsPlayPauseToggleEnabled().unwrap_or(false)
                || if state.playing {
                    controls.IsPauseEnabled().unwrap_or(true)
                } else {
                    controls.IsPlayEnabled().unwrap_or(true)
                };
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

#[cfg(target_os = "windows")]
fn snapshot() -> MediaState {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;
    let config = super::load_config();
    let mut system = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|op| op.get())
        .ok()
        .and_then(|manager| pick_session(&manager, &config.media_priority))
        .map(snapshot_from)
        .unwrap_or_default();

    let mut netease = if config.media_netease_log {
        netease::snapshot()
    } else {
        netease::reset();
        None
    };
    if let Some(log) = &netease {
        if system.app_id.to_ascii_lowercase().contains("cloudmusic")
            && (system.title.trim().is_empty() || system.title.trim() == log.title)
        {
            if system.duration == 0.0 || (system.position == 0.0 && log.position > 1.5) {
                system.playing = log.playing;
                system.position = log.position;
                if log.duration > 0.0 {
                    system.duration = log.duration;
                }
            }
            if system.title.trim().is_empty() {
                system.title.clone_from(&log.title);
            }
            if system.artist.is_empty() {
                system.artist.clone_from(&log.artist);
            }
            if system.album.is_empty() {
                system.album.clone_from(&log.album);
            }
            if system.cover.is_empty() {
                system.cover.clone_from(&log.cover);
            }
            netease = None;
        }
    }

    let now = chrono::Utc::now().timestamp_millis();
    let logs = netease.into_iter().chain(
        config
            .media_playback_logs
            .iter()
            .filter_map(|path| playback_log::snapshot(std::path::Path::new(path), now)),
    );
    select_state(system, logs, &config.media_priority)
}

#[cfg(target_os = "windows")]
fn select_state(
    system: MediaState,
    logs: impl Iterator<Item = MediaState>,
    priority: &[String],
) -> MediaState {
    let rank = |state: &MediaState| priority_rank(priority, &state.app_id, &state.app_name);
    let best = logs.min_by_key(|state| (!state.playing, rank(state)));
    match best {
        Some(log)
            if system.title.trim().is_empty()
                || (log.playing && (!system.playing || rank(&log) < rank(&system))) =>
        {
            log
        }
        _ => system,
    }
}

#[cfg(not(target_os = "windows"))]
fn snapshot() -> MediaState {
    MediaState::default()
}

fn publish(app: &AppHandle, state: MediaState) {
    let changed = if let Ok(mut guard) = cache_slot().lock() {
        let changed = guard.as_ref().is_none_or(|old| {
            old.has_session != state.has_session
                || old.app_id != state.app_id
                || old.app_name != state.app_name
                || old.source != state.source
                || old.title != state.title
                || old.artist != state.artist
                || old.album != state.album
                || old.playing != state.playing
                || old.cover != state.cover
                || old.can_toggle != state.can_toggle
                || old.can_prev != state.can_prev
                || old.can_next != state.can_next
                || (old.position - state.position).abs() > 0.5
                || old.duration != state.duration
        });
        *guard = Some(state.clone());
        changed
    } else {
        true
    };
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
    #[cfg(target_os = "windows")]
    netease::reset();
}

#[tauri::command]
pub async fn island_media() -> Result<MediaState, String> {
    tauri::async_runtime::spawn_blocking(snapshot)
        .await
        .map_err(|e| format!("媒体任务失败: {e}"))
}

#[cfg(target_os = "windows")]
fn apply_action(action: &str, app_id: &str) -> Result<(), String> {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|op| op.get())
        .map_err(|e| format!("媒体会话不可用: {e}"))?;
    // 使用用户点击时显示的来源，避免切换播放器后控制到另一会话。
    let matches_app =
        |session: &windows::Media::Control::GlobalSystemMediaTransportControlsSession| {
            session
                .SourceAppUserModelId()
                .is_ok_and(|id| id.to_string() == app_id)
        };
    let session = manager
        .GetCurrentSession()
        .ok()
        .filter(matches_app)
        .or_else(|| {
            let sessions = manager.GetSessions().ok()?;
            (0..sessions.Size().ok()?)
                .filter_map(|index| sessions.GetAt(index).ok())
                .find(matches_app)
        })
        .ok_or_else(|| "播放器的系统媒体会话已不可用".to_string())?;
    let accepted = match action {
        "toggle" => {
            let info = session.GetPlaybackInfo().map_err(|e| e.to_string())?;
            let playing = info.PlaybackStatus().is_ok_and(|s| {
                s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
            });
            let toggle = info
                .Controls()
                .and_then(|c| c.IsPlayPauseToggleEnabled())
                .unwrap_or(false);
            if toggle {
                session.TryTogglePlayPauseAsync()
            } else if playing {
                session.TryPauseAsync()
            } else {
                session.TryPlayAsync()
            }
            .and_then(|op| op.get())
            .map_err(|e| e.to_string())?
        }
        "next" => session
            .TrySkipNextAsync()
            .and_then(|op| op.get())
            .map_err(|e| e.to_string())?,
        "prev" => session
            .TrySkipPreviousAsync()
            .and_then(|op| op.get())
            .map_err(|e| e.to_string())?,
        _ => return Err("未知媒体动作".into()),
    };
    if !accepted {
        return Err("播放器未接受此媒体操作".into());
    }
    // 动作后立即刷新一次，前端不用等下一拍
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        publish(&app, snapshot());
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn apply_action(_action: &str, _app_id: &str) -> Result<(), String> {
    Err("当前平台不支持系统媒体控制".into())
}

#[tauri::command]
pub async fn island_media_action(action: String, app_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || apply_action(&action, &app_id))
        .await
        .map_err(|e| format!("媒体任务失败: {e}"))?
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn playing_log_obeys_priority_and_does_not_take_controls_from_system_on_tie() {
        let smtc = MediaState {
            app_id: "Spotify.exe".into(),
            app_name: "Spotify".into(),
            title: "Song".into(),
            playing: true,
            can_toggle: true,
            source: "smtc".into(),
            ..MediaState::default()
        };
        let log = MediaState {
            app_id: "cloudmusic.exe".into(),
            app_name: "网易云音乐".into(),
            title: "歌曲".into(),
            playing: true,
            source: "netease-log".into(),
            ..MediaState::default()
        };
        let chosen = select_state(smtc.clone(), [log.clone()].into_iter(), &[]);
        assert_eq!(chosen.source, "smtc");
        assert!(chosen.can_toggle);
        let priority = ["cloudmusic".into(), "spotify".into()];
        let chosen = select_state(smtc, [log].into_iter(), &priority);
        assert_eq!(chosen.source, "netease-log");
        assert!(!chosen.can_toggle);
    }
}
