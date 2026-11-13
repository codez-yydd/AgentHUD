//! 录屏：框选之后按 MP4 / GIF 录制，右下角录制条，结束后原地变成预览窗。
//!
//! 交互对齐飞书：
//! 1. 覆盖层里框好区域 → 选格式（MP4 / GIF）+ 是否录鼠标 → 点「开始录制」；
//! 2. 覆盖层挪出屏幕，屏幕右下角浮出录制条（`rec.html`），显示已录时长和
//!    「结束录制」，最长 5 分钟自动停；
//! 3. 结束后录制条原地放大成预览窗：能播放、左右拖动裁剪、复制 / 下载。
//!
//! 实现：
//! - 抓帧走 CoreGraphics 的 `CGWindowListCreateImage`（dlsym，老系统/新系统
//!   都在），抓不到时退回 `screencapture -R`；
//! - MP4 用 VideoToolbox 硬编 H.264，再用纯 Rust 的 `mp4` 写封装，边录边写，
//!   内存占用与时长无关；
//! - GIF 先把 JPEG 帧落到临时目录，停止时统一编码，这样裁剪可以按帧重编；
//! - 录制条窗口由于有独立标题，抓帧时用 `OnScreenBelowWindow` 把它自己排除
//!   在画面之外，录出来的区域里不会出现录制条。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

#[cfg(target_os = "macos")]
mod sck;

mod ffmpeg;

use super::{
    close_shot, hide_island_for_capture, monitor_list, on_main_thread, park_overlay,
    restore_overlay, shot_dir, shot_session, show_island_for_capture, RecordResult, ShotMonitor,
    ShotRect,
};

/// 录制条 / 预览窗的窗口 label（capabilities/rec.json 同步）
pub const REC_WINDOW_LABEL: &str = "shot-recorder";
const REC_WINDOW_TITLE: &str = "AgentHUD 录制";
/// 飞书同款上限：5 分钟
pub const MAX_RECORD_MS: u64 = 300_000;
const MP4_FPS: u64 = 60;
/// GIF 抓帧节奏：先按 30fps 抓，录久了逐级降到 15/10fps，
/// 免得五分钟的 GIF 在临时目录里堆出几百 MB 的 JPEG。
const GIF_INTERVAL_MS: u64 = 33;

fn gif_interval(frames: u32) -> u64 {
    match frames {
        0..=1799 => GIF_INTERVAL_MS,
        1800..=3599 => 66,
        _ => 100,
    }
}
const GIF_MAX_WIDTH: u32 = 1000;
/// GIF 默认帧率；录制完可以在预览窗里改
const DEFAULT_GIF_FPS: u32 = 10;
const MP4_MAX_WIDTH: u32 = 2560;

#[cfg(target_os = "macos")]
const MP4_SUPPORTED: bool = true;
#[cfg(not(target_os = "macos"))]
const MP4_SUPPORTED: bool = false;

/// Windows 的 MP4 录制依赖 ffmpeg：ffmpeg 就位才算支持，否则回落 GIF。
/// macOS 编译期恒为 true（VideoToolbox / ScreenCaptureKit）。
fn mp4_supported() -> bool {
    if MP4_SUPPORTED {
        return true;
    }
    #[cfg(windows)]
    {
        ffmpeg::Ffmpeg::locate().is_ok()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

static REC_SEQ: AtomicU64 = AtomicU64::new(1);
static RECORDS: OnceLock<Mutex<HashMap<u64, Arc<Recording>>>> = OnceLock::new();

fn records() -> &'static Mutex<HashMap<u64, Arc<Recording>>> {
    RECORDS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordOptions {
    /// "mp4" | "gif"
    pub format: Option<String>,
    /// 是否把鼠标指针画进画面
    pub cursor: Option<bool>,
    /// MP4 是否录系统声音（GIF 没有声音，忽略）。Windows 依赖机器上有
    /// 「立体声混音」类 loopback 设备，探测不到只降级为无系统声，不失败。
    pub audio: Option<bool>,
    /// 麦克风设备名（None = 不录麦克风）；Windows 走 ffmpeg dshow，
    /// macOS 目前忽略（ScreenCaptureKit 麦克风未接入，见交接文档）
    pub mic: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStarted {
    pub record_id: u64,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub max_seconds: u64,
    /// 非致命提示（如「未检测到系统声设备」），录制条/覆盖层 toast 一次
    pub notice: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecordState {
    pub running: bool,
    pub elapsed_ms: u64,
    pub format: String,
    pub result: Option<RecordResult>,
    pub error: Option<String>,
    /// GIF 当前帧率与重编码进度（MP4 为 0）
    pub gif_fps: u32,
    pub progress: u32,
}

struct Inner {
    running: bool,
    result: Option<RecordResult>,
    error: Option<String>,
}

pub struct Recording {
    id: u64,
    format: String,
    stop: AtomicBool,
    /// 抓帧线程真正开始抓的时间（epoch ms）。命令入口到首帧之间要挪窗口、
    /// 预滚 420ms，用命令时间当起点会让录制条比视频长出 1 秒多。
    started_ms: AtomicU64,
    max_ms: u64,
    session_id: u64,
    /// 会话所在显示器的物理位置（恢复覆盖层用）
    monitor_x: i32,
    monitor_y: i32,
    inner: Mutex<Inner>,
    /// GIF 每帧的绝对时间戳（ms），裁剪时按时间挑帧
    frames: Mutex<Vec<u64>>,
    /// GIF 目标帧率（预览/导出共用，改帧率后重新编码）
    gif_fps: AtomicU64,
    /// GIF 重编码进度（0~100），录制条/预览窗据此显示百分比
    progress: AtomicU64,
    temp: Option<PathBuf>,
    media: Mutex<Option<PathBuf>>,
}

impl Recording {
    fn finish(self: &Arc<Self>, outcome: Result<RecordResult, String>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.running = false;
            match outcome {
                Ok(mut result) => {
                    result.record_id = self.id;
                    if let Ok(mut media) = self.media.lock() {
                        *media = Some(PathBuf::from(&result.path));
                    }
                    inner.result = Some(result);
                }
                Err(error) => inner.error = Some(error),
            }
        }
    }

    fn elapsed_ms(&self) -> u64 {
        let started = self.started_ms.load(Ordering::Relaxed);
        if started == 0 {
            return 0;
        }
        (super::now_ms() - started as i64).max(0) as u64
    }
}

fn normalize_format(raw: Option<&str>, mp4_ok: bool) -> String {
    match raw {
        Some("gif") => "gif".to_string(),
        _ if mp4_ok => "mp4".to_string(),
        _ => "gif".to_string(),
    }
}

fn output_size(format: &str, width: u32, height: u32) -> (u32, u32) {
    let (mut w, mut h) = (width.max(2), height.max(2));
    if format == "mp4" {
        if w > MP4_MAX_WIDTH {
            let ratio = MP4_MAX_WIDTH as f64 / w as f64;
            h = (h as f64 * ratio).round() as u32;
            w = MP4_MAX_WIDTH;
        }
        // H.264 要求宽高为偶数
        (w & !1, h & !1)
    } else {
        if w > GIF_MAX_WIDTH {
            let ratio = GIF_MAX_WIDTH as f64 / w as f64;
            h = (h as f64 * ratio).round() as u32;
            w = GIF_MAX_WIDTH;
        }
        (w.max(2), h.max(2))
    }
}

fn recording_name(format: &str) -> String {
    format!(
        "Recording_{}.{}",
        chrono::Local::now().format("%Y%m%d_%H%M%S_%3f"),
        format
    )
}

// ============================================================
// Tauri 命令
// ============================================================

/// 开始录制：校验会话/区域 → 挪走覆盖层 → 右下角弹出录制条 → 后台线程抓帧。
#[tauri::command]
pub async fn shot_record_start(
    app: AppHandle,
    id: u64,
    rect: ShotRect,
    options: RecordOptions,
) -> Result<RecordStarted, String> {
    let outcome = start_recording(app, id, rect, options).await;
    match &outcome {
        Ok(started) => eprintln!(
            "[zbar-rec] 开始录制 id={} 格式={} {}×{}",
            started.record_id, started.format, started.width, started.height
        ),
        Err(e) => eprintln!("[zbar-rec] 开始录制失败: {e}"),
    }
    outcome
}

async fn start_recording(
    app: AppHandle,
    id: u64,
    rect: ShotRect,
    options: RecordOptions,
) -> Result<RecordStarted, String> {
    let busy = records()
        .lock()
        .map(|map| {
            map.values()
                .any(|rec| rec.inner.lock().map(|i| i.running).unwrap_or(false))
        })
        .unwrap_or(false);
    if busy {
        return Err("已有录屏在进行中".into());
    }

    let session = shot_session(id)?;
    if session.origin.is_none() {
        return Err("当前截图不是屏幕区域，无法录制：请框选屏幕区域后再录".into());
    }
    // Windows 上 MP4 依赖 ffmpeg：没装就回落 GIF，并系统通知一次
    let mp4_ok = mp4_supported();
    let format = normalize_format(options.format.as_deref(), mp4_ok);
    if !mp4_ok && options.format.as_deref() != Some("gif") {
        crate::notify(
            "录屏格式".into(),
            "未找到 ffmpeg，本次录制降级为 GIF（无音轨）。可在录屏条里下载 ffmpeg 后使用 MP4".into(),
        );
    }
    let cursor = options.cursor.unwrap_or(true);
    let audio = options.audio.unwrap_or(true) && format == "mp4";

    // Windows MP4：ffmpeg 定位 + 麦克风/系统声设备预校验。子进程枚举要
    // 几百毫秒，放阻塞线程池，别卡异步执行器。
    #[cfg(windows)]
    let (audio_spec, notice) = if format == "mp4" {
        let mic = options
            .mic
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        probe_windows_audio(mic, audio).await?
    } else {
        (ffmpeg::RecordAudioSpec::default(), None)
    };
    #[cfg(not(windows))]
    let notice: Option<String> = None;

    #[cfg(target_os = "macos")]
    if !super::mac_screen_recording_ready() {
        return Err("没有屏幕录制权限：系统设置 → 隐私与安全性 → 屏幕录制，勾选 AgentHUD 后重试".into());
    }

    let scale = session.monitor.scale_factor.max(0.5);
    #[cfg(target_os = "macos")]
    super::set_mac_scale(scale);
    let x = rect.x.round() as i32;
    let y = rect.y.round() as i32;
    let w = rect.width.round().max(2.0) as u32;
    let h = rect.height.round().max(2.0) as u32;
    if x < 0 || y < 0 || x as u32 + w > session.width || y as u32 + h > session.height {
        return Err("录制区域超出屏幕范围".into());
    }
    let screen_x = session.monitor.x + x;
    let screen_y = session.monitor.y + y;
    let (out_w, out_h) = output_size(&format, w, h);

    let temp = if format == "gif" {
        let dir = std::env::temp_dir().join(format!(
            "agenthud-recording-{}-{}",
            std::process::id(),
            chrono::Local::now().format("%Y%m%d%H%M%S%3f")
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建录屏临时目录失败: {e}"))?;
        Some(dir)
    } else {
        None
    };

    prune_temp_media();
    let record_id = REC_SEQ.fetch_add(1, Ordering::Relaxed);
    let rec = Arc::new(Recording {
        id: record_id,
        format: format.clone(),
        stop: AtomicBool::new(false),
        started_ms: AtomicU64::new(0),
        max_ms: MAX_RECORD_MS,
        session_id: id,
        monitor_x: session.monitor.x,
        monitor_y: session.monitor.y,
        inner: Mutex::new(Inner {
            running: true,
            result: None,
            error: None,
        }),
        frames: Mutex::new(Vec::new()),
        gif_fps: AtomicU64::new(DEFAULT_GIF_FPS as u64),
        progress: AtomicU64::new(0),
        temp,
        media: Mutex::new(None),
    });
    if let Ok(mut map) = records().lock() {
        map.insert(record_id, rec.clone());
    }

    // 覆盖层挪出屏幕：录制期间用户看到的应该是真实桌面。
    park_overlay(&app).await?;
    show_island_for_capture(&app).await;

    let monitor = session.monitor.clone();
    let app_window = app.clone();
    let window = on_main_thread(&app, move || {
        build_recorder_window(&app_window, record_id, &monitor)
    })
    .await;
    if let Err(e) = window {
        if let Ok(mut map) = records().lock() {
            map.remove(&record_id);
        }
        let _ = restore_overlay(&app, session.monitor.x, session.monitor.y).await;
        return Err(e);
    }

    // Windows 的 MP4 走 ffmpeg 抓屏（gdigrab），没有 macOS 那种「排除本进程
    // 窗口」的机制：录制条若压在选区里会入画，此时把它藏起来，改用全局
    // 快捷键（Ctrl+Shift+G，与「开始录制」同键）结束录制；选区不含录制条
    // 时保持显示，用户照常点「结束」或按 Esc。
    // 顺序必须是「先注册快捷键、成功后才隐藏」：注册失败时保持录制条
    // 可见，用户仍可点「结束」/按 Esc 停止，不至于只能等 5 分钟自动停。
    #[cfg(windows)]
    let hide_recorder_bar = format == "mp4"
        && recorder_overlaps_selection(&session.monitor, screen_x, screen_y, w, h);
    #[cfg(windows)]
    let mut stop_hotkey_registered = false;
    #[cfg(windows)]
    if hide_recorder_bar && register_record_stop_hotkey(&app) {
        stop_hotkey_registered = true;
        let app_hide = app.clone();
        let hidden = on_main_thread(&app, move || {
            if let Some(win) = app_hide.get_webview_window(REC_WINDOW_LABEL) {
                let _ = win.hide();
                Ok(())
            } else {
                Err("录制窗口不存在".to_string())
            }
        })
        .await
        .is_ok();
        // hide 失败（窗口异常）时快捷键反而是唯一停止手段，注册保留到收尾
        if hidden {
            crate::notify(
                "录屏已开始".into(),
                "录制条在录制区域内已隐藏：按 Ctrl+Shift+G 结束录制（最长 5 分钟自动停）".into(),
            );
        }
    }

    #[cfg(windows)]
    let (mic_in_spec, system_in_spec) = (audio_spec.mic.clone(), audio_spec.system.clone());
    #[cfg(not(windows))]
    let (mic_in_spec, system_in_spec): (Option<String>, Option<String>) = (None, None);

    let spec = CaptureSpec {
        screen_x,
        screen_y,
        width: w,
        height: h,
        out_w,
        out_h,
        scale,
        format: format.clone(),
        cursor,
        audio,
        fps: MP4_FPS as u32,
        cursor_origin: (screen_x, screen_y),
        max_ms: MAX_RECORD_MS,
        mic: mic_in_spec,
        system_audio: system_in_spec,
    };
    let rec_worker = rec.clone();
    let dir = shot_dir()?;

    // Windows MP4：单独的 ffmpeg 录制线程（子进程收尾 + 快捷键按需注销）
    #[cfg(windows)]
    if format == "mp4" {
        let worker_app = app.clone();
        let hotkey_registered = stop_hotkey_registered;
        std::thread::spawn(move || {
            run_ffmpeg_recording(rec_worker, spec, dir, worker_app, hotkey_registered);
        });
        return Ok(RecordStarted {
            record_id,
            format,
            width: out_w,
            height: out_h,
            max_seconds: MAX_RECORD_MS / 1000,
            notice,
        });
    }

    std::thread::spawn(move || {
        run_recording(rec_worker, spec, dir);
    });

    Ok(RecordStarted {
        record_id,
        format,
        width: out_w,
        height: out_h,
        max_seconds: MAX_RECORD_MS / 1000,
        notice,
    })
}

/// 录制条轮询：已录时长 / 是否结束 / 结果或错误。
#[tauri::command]
pub fn shot_record_state(record_id: u64) -> Result<RecordState, String> {
    let rec = get_record(record_id)?;
    let inner = rec
        .inner
        .lock()
        .map_err(|_| "录屏状态不可用".to_string())?;
    let elapsed = if inner.running {
        rec.elapsed_ms()
    } else {
        inner
            .result
            .as_ref()
            .map(|r| r.duration_ms)
            .unwrap_or_else(|| rec.elapsed_ms())
    };
    Ok(RecordState {
        running: inner.running,
        elapsed_ms: elapsed.min(rec.max_ms),
        format: rec.format.clone(),
        result: inner.result.clone(),
        error: inner.error.clone(),
        gif_fps: rec.gif_fps.load(Ordering::Relaxed) as u32,
        progress: rec.progress.load(Ordering::Relaxed) as u32,
    })
}

/// GIF 改帧率：按目标帧率重新编码，原子替换原文件，返回最新状态。
/// 帧率越高越流畅、文件越大；预览窗里改完即时生效。
#[tauri::command]
pub async fn shot_record_gif_fps(record_id: u64, fps: u32) -> Result<RecordState, String> {
    let rec = get_record(record_id)?;
    if rec.format != "gif" {
        return Err("只有 GIF 可以调整帧率".into());
    }
    let fps = fps.clamp(1, 30);
    eprintln!("[zbar-rec] 改 GIF 帧率 id={record_id} -> {fps}fps");
    let result = {
        let inner = rec
            .inner
            .lock()
            .map_err(|_| "录屏状态不可用".to_string())?;
        inner
            .result
            .clone()
            .ok_or_else(|| "录制还没结束".to_string())?
    };
    let dir = rec
        .temp
        .clone()
        .ok_or_else(|| "录制帧已清理，无法重新生成".to_string())?;
    rec.gif_fps.store(fps as u64, Ordering::Relaxed);
    rec.progress.store(0, Ordering::Relaxed);
    let rec_worker = rec.clone();
    let result_worker = result.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let times = rec_worker
            .frames
            .lock()
            .map_err(|_| "录制帧不可用".to_string())?
            .clone();
        let picked = sample_frames(&times, 0.0, result_worker.duration_ms as f64, fps);
        let temp = std::env::temp_dir().join(format!(
            "agenthud-gif-{}-{}.gif",
            std::process::id(),
            rec_worker.id
        ));
        encode_gif_frames(
            &dir,
            &picked,
            result_worker.width,
            result_worker.height,
            &temp,
            Some(&rec_worker.progress),
        )?;
        let target = PathBuf::from(&result_worker.path);
        std::fs::rename(&temp, &target).map_err(|e| format!("替换 GIF 失败: {e}"))?;
        let _ = std::fs::remove_file(&temp);
        let mut inner = rec_worker
            .inner
            .lock()
            .map_err(|_| "录屏状态不可用".to_string())?;
        if let Some(current) = inner.result.as_mut() {
            current.frames = picked.len() as u32;
            current.size = std::fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("重新生成 GIF 失败: {e}"))??;
    eprintln!("[zbar-rec] GIF 重编码完成 id={record_id}");
    shot_record_state(record_id)
}

/// 结束录制：只置停止位，抓帧线程自己收尾（MP4 flush / GIF 编码）。
#[tauri::command]
pub fn shot_record_stop(record_id: u64) -> Result<(), String> {
    let rec = get_record(record_id)?;
    rec.stop.store(true, Ordering::Relaxed);
    Ok(())
}

/// 枚举音频输入设备（Windows MP4 的麦克风下拉）。ffmpeg 未就位时返回
/// 中文错误，前端据此展示下载引导。
#[tauri::command]
pub async fn shot_record_audio_devices() -> Result<Vec<ffmpeg::AudioDevice>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let ff = ffmpeg::Ffmpeg::locate()?;
        ff.audio_devices()
    })
    .await
    .map_err(|e| format!("枚举音频设备任务失败: {e}"))?
}

/// ffmpeg 是否就位（前端决定显示下载引导还是麦克风下拉）。
#[tauri::command]
pub fn shot_record_ffmpeg_status() -> ffmpeg::FfmpegStatus {
    ffmpeg::Ffmpeg::status()
}

/// 确保 ffmpeg 就位：缺失时按需下载（多源依次降级），进度经
/// `zbar://ffmpeg-download` 事件推给前端。下载上百 MB，全程在阻塞线程。
#[tauri::command]
pub async fn shot_record_ensure_ffmpeg(app: AppHandle) -> Result<ffmpeg::FfmpegStatus, String> {
    tauri::async_runtime::spawn_blocking(move || ffmpeg::ensure_ffmpeg(&app))
        .await
        .map_err(|e| format!("ffmpeg 下载任务失败: {e}"))?
}

/// 把录制条原地放大成预览窗（播放 / 裁剪 / 复制 / 下载）。
#[tauri::command]
pub async fn shot_record_preview(app: AppHandle, record_id: u64) -> Result<(), String> {
    let rec = get_record(record_id)?;
    let (w, h) = {
        let inner = rec
            .inner
            .lock()
            .map_err(|_| "录屏状态不可用".to_string())?;
        let result = inner.result.clone().ok_or_else(|| "录制还没结束".to_string())?;
        (result.width, result.height)
    };
    let Some(monitor) = monitor_list(&app)
        .into_iter()
        .find(|m| m.x == rec.monitor_x && m.y == rec.monitor_y)
        .or_else(|| monitor_list(&app).into_iter().find(|m| m.primary))
    else {
        return Err("没有检测到显示器".into());
    };
    let app_main = app.clone();
    on_main_thread(&app, move || {
        let Some(win) = app_main.get_webview_window(REC_WINDOW_LABEL) else {
            return Err("录制窗口不存在".to_string());
        };
        let (logical_w, logical_h) = preview_size(&monitor, w, h);
        let _ = win.set_size(PhysicalSize::new(
            (logical_w * monitor.scale_factor) as u32,
            (logical_h * monitor.scale_factor) as u32,
        ));
        let x = monitor.x
            + ((monitor.width as f64 - logical_w * monitor.scale_factor) / 2.0).max(0.0) as i32;
        let y = monitor.y
            + ((monitor.height as f64 - logical_h * monitor.scale_factor) / 2.0).max(0.0) as i32;
        let _ = win.set_position(PhysicalPosition::new(x, y));
        let _ = win.show();
        let _ = win.set_focus();
        Ok(())
    })
    .await
}

/// 预览窗要播放的媒体路径：顺手把文件放进 asset 协议白名单。
#[tauri::command]
pub fn shot_record_src(app: AppHandle, record_id: u64) -> Result<String, String> {
    let rec = get_record(record_id)?;
    let path = rec
        .media
        .lock()
        .map_err(|_| "录屏数据不可用".to_string())?
        .clone()
        .ok_or_else(|| "录制文件还没生成".to_string())?;
    let _ = app.asset_protocol_scope().allow_file(&path);
    Ok(path.to_string_lossy().to_string())
}

/// 媒体原始字节：asset 协议不可用时前端退回 blob URL 播放。
#[tauri::command]
pub async fn shot_record_media(record_id: u64) -> Result<tauri::ipc::Response, String> {
    let rec = get_record(record_id)?;
    let path = rec
        .media
        .lock()
        .map_err(|_| "录屏数据不可用".to_string())?
        .clone()
        .ok_or_else(|| "录制文件还没生成".to_string())?;
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        std::fs::read(&path).map_err(|e| format!("读取录制文件失败: {e}"))
    })
    .await
    .map_err(|e| format!("读取录制文件任务失败: {e}"))??;
    Ok(tauri::ipc::Response::new(bytes))
}

/// 复制 / 下载（下载=另存为）。`start_ms`/`end_ms` 是裁剪区间，整段时直接用原文件。
#[tauri::command]
pub async fn shot_record_export(
    app: AppHandle,
    record_id: u64,
    start_ms: f64,
    end_ms: f64,
    action: String,
) -> Result<Option<String>, String> {
    let rec = get_record(record_id)?;
    let result = {
        let inner = rec
            .inner
            .lock()
            .map_err(|_| "录屏状态不可用".to_string())?;
        inner
            .result
            .clone()
            .ok_or_else(|| "录制还没结束".to_string())?
    };
    let duration = result.duration_ms as f64;
    let start = start_ms.clamp(0.0, duration);
    let end = end_ms.clamp(0.0, duration).max(start + 1.0);
    let trimmed = start > 60.0 || end < duration - 60.0;
    eprintln!(
        "[zbar-rec] 导出请求 id={record_id} {:.0}ms~{:.0}ms action={action} 裁剪={trimmed}",
        start, end
    );
    tauri::async_runtime::spawn_blocking(move || {
        let source = PathBuf::from(&result.path);
        let path = if trimmed {
            let trimmed_path = trim_recording(&rec, &source, &result, start, end)?;
            eprintln!("[zbar-rec] 裁剪完成 {}", trimmed_path.display());
            trimmed_path
        } else {
            source
        };
        match action.as_str() {
            "copy" => {
                if let Err(e) = clipboard_file(&path) {
                    eprintln!("[zbar-rec] 复制失败: {e}");
                    return Err(e);
                }
                eprintln!("[zbar-rec] 已复制 {}", path.display());
                Ok(Some(path.to_string_lossy().to_string()))
            }
            "save" => {
                use tauri_plugin_dialog::DialogExt;
                let ext = result.format.clone();
                // 默认落到「视频/AgentHUD」，而不是让对话框停在临时目录或
                // 上次的任意位置；文件名用人类可读的时间戳，不暴露
                // agenthud-recording-* / agenthud-trim-* 这种内部名字。
                let dir = dirs::video_dir()
                    .or_else(|| dirs::home_dir().map(|home| home.join("Videos")))
                    .unwrap_or_else(|| std::env::temp_dir())
                    .join("AgentHUD");
                let _ = std::fs::create_dir_all(&dir);
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let name = format!(
                    "录屏-{stamp}{}.{ext}",
                    if trimmed { "-裁剪" } else { "" }
                );
                let filter = if ext == "gif" {
                    ("GIF 动图", "gif")
                } else {
                    ("MP4 视频", "mp4")
                };
                let picked = app
                    .dialog()
                    .file()
                    .set_title("保存录屏")
                    .set_directory(&dir)
                    .add_filter(filter.0, &[filter.1])
                    .set_file_name(name)
                    .blocking_save_file();
                let Some(picked) = picked else {
                    return Ok(None);
                };
                let target = picked
                    .into_path()
                    .map_err(|e| format!("解析保存路径失败: {e}"))?;
                std::fs::copy(&path, &target).map_err(|e| format!("写入录屏文件失败: {e}"))?;
                eprintln!("[zbar-rec] 已保存 {}", target.display());
                Ok(Some(target.to_string_lossy().to_string()))
            }
            other => Err(format!("未知操作: {other}")),
        }
    })
    .await
    .map_err(|e| format!("导出录屏任务失败: {e}"))?
}

/// 关闭预览窗：停掉线程、删临时帧、收起截图会话并把 HUD 恢复。
#[tauri::command]
pub async fn shot_record_close(app: AppHandle, record_id: u64) -> Result<(), String> {
    stop_and_wait(record_id);
    remove_record(record_id);
    let app_main = app.clone();
    let _ = on_main_thread(&app, move || {
        if let Some(win) = app_main.get_webview_window(REC_WINDOW_LABEL) {
            let _ = win.destroy();
        }
        Ok(())
    })
    .await;
    close_shot(&app).await
}

/// 出错 / 用户取消：收起录制窗，把覆盖层放回原位让用户重选。
#[tauri::command]
pub async fn shot_record_cancel(
    app: AppHandle,
    record_id: u64,
    message: Option<String>,
) -> Result<(), String> {
    let monitor = get_record(record_id).ok().and_then(|rec| {
        // 会话被别的入口清掉时就不用把覆盖层放回去了
        super::shot_session(rec.session_id)
            .ok()
            .map(|_| (rec.monitor_x, rec.monitor_y))
    });
    stop_and_wait(record_id);
    remove_record(record_id);
    let app_main = app.clone();
    let _ = on_main_thread(&app, move || {
        if let Some(win) = app_main.get_webview_window(REC_WINDOW_LABEL) {
            let _ = win.destroy();
        }
        Ok(())
    })
    .await;
    if let Some(text) = message.filter(|text| !text.trim().is_empty()) {
        let _ = app.emit_to(super::SHOT_WINDOW_LABEL, super::SHOT_RECORD_ERROR_EVENT, text);
    }
    hide_island_for_capture(&app).await;
    if let Some((x, y)) = monitor {
        let _ = restore_overlay(&app, x, y).await;
    }
    Ok(())
}

/// 是否有录屏进行中（截图入口据此拦截，避免边录边截）。
pub fn is_recording() -> bool {
    records()
        .lock()
        .map(|map| {
            map.values()
                .any(|rec| rec.inner.lock().map(|i| i.running).unwrap_or(false))
        })
        .unwrap_or(false)
}

fn get_record(record_id: u64) -> Result<Arc<Recording>, String> {
    records()
        .lock()
        .map_err(|_| "录屏状态不可用".to_string())?
        .get(&record_id)
        .cloned()
        .ok_or_else(|| "录屏会话已失效".into())
}

fn stop_and_wait(record_id: u64) {
    let Some(rec) = get_record(record_id).ok() else {
        return;
    };
    rec.stop.store(true, Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let running = rec.inner.lock().map(|i| i.running).unwrap_or(false);
        if !running {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn remove_record(record_id: u64) {
    let removed = records().lock().ok().and_then(|mut map| map.remove(&record_id));
    if let Some(rec) = removed {
        if let Some(dir) = &rec.temp {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// 清理系统临时目录里的旧录屏产物：
/// - `agenthud-recording-*`：GIF 的 JPEG 帧（只在录制/编码期间有用）；
/// - `agenthud-trim-*`：裁剪产物。**不能立刻删**——「复制」放的是文件
///   URL，用户关掉预览窗之后才去聊天窗口粘贴，删早了就粘不出来，
///   所以只在下次开录前清理一天以前的；
/// - `agenthud-ffmpeg-*`：ffmpeg 录制/裁剪的 stderr 日志（正常路径
///   用完即删，这里兜底清残留）。
fn prune_temp_media() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let stale = name.starts_with("agenthud-trim-")
            || name.starts_with("agenthud-recording-")
            || name.starts_with("agenthud-ffmpeg-");
        if !stale {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .map(|age| age.as_secs() > 24 * 60 * 60)
            .unwrap_or(false);
        if old {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

fn preview_size(monitor: &ShotMonitor, media_w: u32, media_h: u32) -> (f64, f64) {
    let scale = monitor.scale_factor.max(0.5);
    let mon_w = monitor.width as f64 / scale;
    let mon_h = monitor.height as f64 / scale;
    let content_w = (mon_w * 0.6).clamp(520.0, 820.0);
    let aspect = if media_w == 0 {
        0.6
    } else {
        media_h as f64 / media_w as f64
    };
    let content_h = (content_w * aspect).clamp(180.0, mon_h * 0.52);
    let win_w = content_w + 48.0;
    let win_h = content_h + 178.0;
    (win_w, win_h.min(mon_h - 80.0))
}

// ============================================================
// 录制窗口
// ============================================================

fn build_recorder_window(
    app: &AppHandle,
    record_id: u64,
    monitor: &ShotMonitor,
) -> Result<WebviewWindow, String> {
    if let Some(old) = app.get_webview_window(REC_WINDOW_LABEL) {
        let _ = old.destroy();
    }
    let scale = monitor.scale_factor.max(0.5);
    let (w, h) = (238.0, 54.0);
    let win = WebviewWindowBuilder::new(
        app,
        REC_WINDOW_LABEL,
        WebviewUrl::App(format!("rec.html?id={record_id}").into()),
    )
    .title(REC_WINDOW_TITLE)
    .inner_size(w, h)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .closable(false)
    .visible(false)
    .focused(false)
    .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
    .build()
    .map_err(|e| format!("创建录制窗口失败: {e}"))?;
    let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    let x = monitor.x + monitor.width as i32 - (w * scale) as i32 - (24.0 * scale) as i32;
    let y = monitor.y + monitor.height as i32 - (h * scale) as i32 - (24.0 * scale) as i32;
    win.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| format!("定位录制窗口失败: {e}"))?;
    let _ = win.show();
    Ok(win)
}

// ============================================================
// 抓帧 + 编码
// ============================================================

#[derive(Clone)]
struct CaptureSpec {
    /// 物理像素（Tauri 显示器坐标系）
    screen_x: i32,
    screen_y: i32,
    width: u32,
    height: u32,
    out_w: u32,
    out_h: u32,
    scale: f64,
    format: String,
    cursor: bool,
    audio: bool,
    fps: u32,
    cursor_origin: (i32, i32),
    max_ms: u64,
    /// 麦克风设备（仅 Windows ffmpeg 分支使用；None = 不录）
    mic: Option<String>,
    /// 系统声 loopback 设备（仅 Windows ffmpeg 分支使用）
    system_audio: Option<String>,
}

// ============================================================
// Windows：ffmpeg 录制（MP4 + 麦克风 / 系统声）
// ============================================================

/// Windows MP4 的音频预校验：麦克风必须在设备列表里（被拔出/占用就明确
/// 失败，不产出静默无音轨的文件）；系统声按「立体声混音」关键词探测，
/// 探测不到只降级并带一句提示，不阻塞录制。
#[cfg(windows)]
async fn probe_windows_audio(
    mic: Option<String>,
    want_system: bool,
) -> Result<(ffmpeg::RecordAudioSpec, Option<String>), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut spec = ffmpeg::RecordAudioSpec::default();
        let mut notice: Option<String> = None;
        if mic.is_none() && !want_system {
            return Ok((spec, notice));
        }
        let ff = ffmpeg::Ffmpeg::locate()?;
        let devices = ff.audio_devices()?;
        if let Some(mic) = &mic {
            let found = devices
                .iter()
                .find(|device| device.name == *mic || device.label == *mic)
                .ok_or_else(|| {
                    format!("找不到麦克风设备「{mic}」：设备可能已被拔出或占用，请在录制条里重新选择")
                })?;
            // 统一改用 dshow 备用名：与本地代码页无关，比显示名稳
            spec.mic = Some(found.name.clone());
        }
        if want_system {
            match ffmpeg::find_loopback_device(&devices) {
                Some(loopback) => spec.system = Some(loopback.name),
                None => {
                    notice = Some(
                        "未检测到系统声音设备（立体声混音），本条录制不含系统声；如需系统声请在系统声音设置里启用「立体声混音」".into(),
                    );
                }
            }
        }
        Ok((spec, notice))
    })
    .await
    .map_err(|e| format!("音频设备检查任务失败: {e}"))?
}

/// 录制条（右下角 238×54 逻辑像素 + 24 边距，见 build_recorder_window）
/// 是否与选区相交——相交则 gdigrab 会把它拍进画面。
#[cfg(windows)]
fn recorder_overlaps_selection(
    monitor: &ShotMonitor,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) -> bool {
    let scale = monitor.scale_factor.max(0.5);
    let bar_w = (238.0 * scale) as i32;
    let bar_h = (54.0 * scale) as i32;
    let margin = (24.0 * scale) as i32;
    let bar_x = monitor.x + monitor.width as i32 - bar_w - margin;
    let bar_y = monitor.y + monitor.height as i32 - bar_h - margin;
    x < bar_x + bar_w && bar_x < x + w as i32 && y < bar_y + bar_h && bar_y < y + h as i32
}

/// 录制条被隐藏时，临时注册「停止录制」全局快捷键（与「开始录制」同键
/// Ctrl+Shift+G，语义连续：按下开始，录制中再按结束）。返回是否注册成功。
#[cfg(windows)]
fn register_record_stop_hotkey(app: &AppHandle) -> bool {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    match gs.on_shortcut("ctrl+shift+g", move |_app, _shortcut, event| {
        if !matches!(
            event.state(),
            tauri_plugin_global_shortcut::ShortcutState::Pressed
        ) {
            return;
        }
        // 同一时刻最多一条录制；直接把所有进行中的都置停止位
        if let Ok(map) = records().lock() {
            for rec in map.values() {
                if rec.inner.lock().map(|i| i.running).unwrap_or(false) {
                    rec.stop.store(true, Ordering::Relaxed);
                }
            }
        }
    }) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("[zbar-rec] 注册停止录制快捷键失败（可能被占用）: {e}");
            false
        }
    }
}

/// 录制结束后注销「停止录制」快捷键（成败路径都要走）。
#[cfg(windows)]
fn unregister_record_stop_hotkey(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    let _ = gs.unregister("ctrl+shift+g");
}

/// Windows MP4 录制主循环：spawn ffmpeg → 轮询停止位 → 停止收尾（CTRL_BREAK
/// 优先、kill 兜底，fMP4 保证落盘文件可用，详见 ffmpeg.rs 模块注释）→
/// 校验产物 → 通知前端。
#[cfg(windows)]
fn run_ffmpeg_recording(
    rec: Arc<Recording>,
    spec: CaptureSpec,
    dir: PathBuf,
    app: AppHandle,
    stop_hotkey_registered: bool,
) {
    /// gdigrab 帧率：30fps 是 Windows 屏幕抓取与体积的平衡点（mac SCK 是 60）
    const GRAB_FPS: u32 = 30;
    // 录制期间暂停底板采样，避免和录屏抓屏抢 GDI 桌面（见 island::capture_guard）。
    let _capture_guard = crate::island::capture_guard::CaptureGuard::enter();
    let outcome = (|| -> Result<RecordResult, String> {
        let ff = ffmpeg::Ffmpeg::locate()?;
        let path = dir.join(recording_name("mp4"));
        let audio = ffmpeg::RecordAudioSpec {
            mic: spec.mic.clone(),
            system: spec.system_audio.clone(),
        };
        let args = ffmpeg::build_record_args(
            spec.screen_x,
            spec.screen_y,
            spec.width,
            spec.height,
            spec.out_w,
            spec.out_h,
            GRAB_FPS,
            spec.cursor,
            &audio,
            &path,
        );
        // 对齐 run_capture 的 420ms 预滚：等覆盖层挪走、窗口系统画稳
        std::thread::sleep(Duration::from_millis(420));
        let mut proc = ffmpeg::FfmpegRecording::spawn(&ff, &args)?;
        rec.started_ms
            .store(super::now_ms() as u64, Ordering::Relaxed);
        // 提前失败探测：设备名错 / 抓屏区域非法会让 ffmpeg 立即退出。
        // 给它 600ms，把「启动即失败」在录制条还醒着时就暴露出来。
        std::thread::sleep(Duration::from_millis(600));
        if let Some(status) = proc.try_finish() {
            let tail = proc.stderr_tail(400);
            proc.cleanup();
            let _ = std::fs::remove_file(&path);
            return Err(describe_ffmpeg_exit(status, &tail, "启动"));
        }
        loop {
            if rec.stop.load(Ordering::Relaxed) {
                break;
            }
            if rec.elapsed_ms() >= spec.max_ms {
                break;
            }
            if let Some(status) = proc.try_finish() {
                // 录制中途 ffmpeg 自己退出（磁盘满 / 进程被杀）
                let tail = proc.stderr_tail(400);
                proc.cleanup();
                let _ = std::fs::remove_file(&path);
                return Err(describe_ffmpeg_exit(status, &tail, "录制"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let duration = rec.elapsed_ms().min(spec.max_ms);
        let status = proc
            .stop_graceful()
            .map_err(|e| format!("结束 ffmpeg 失败: {e}"))?;
        proc.cleanup();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size == 0 || (!status.success() && size < 1024) {
            let _ = std::fs::remove_file(&path);
            return Err("录制没有产出可用的 MP4 文件".into());
        }
        Ok(RecordResult {
            path: path.to_string_lossy().to_string(),
            width: spec.out_w,
            height: spec.out_h,
            frames: ((duration as f64 / 1000.0) * GRAB_FPS as f64) as u32,
            duration_ms: duration,
            size,
            format: "mp4".into(),
            record_id: 0,
            has_audio: audio.has_audio(),
        })
    })();
    // 录制条被隐藏时注册过停止快捷键，结束时注销。只有本次注册成功过才
    // 注销：用户可能把自定义截图快捷键（可配置）设成同键，无条件注销
    // 会把用户的快捷键一起拆掉。预览窗的 show 由 shot_record_preview
    // 负责（它本来就带 win.show()）。
    if stop_hotkey_registered {
        unregister_record_stop_hotkey(&app);
    }
    rec.finish(outcome);
}

/// 把 ffmpeg 退出状态 + stderr 尾部转成可读中文错误。
#[cfg(windows)]
fn describe_ffmpeg_exit(status: std::process::ExitStatus, tail: &str, phase: &str) -> String {
    if tail.trim().is_empty() {
        format!(
            "ffmpeg {phase}失败（退出码 {}）",
            status.code().unwrap_or(-1)
        )
    } else {
        format!("ffmpeg {phase}失败: {tail}")
    }
}

/// 录制入口：macOS 15+ 的 MP4 优先走 ScreenCaptureKit（60fps + 系统声音），
/// 起流失败再退回逐帧抓屏 + VideoToolbox 编码（无声音、帧率受抓屏速度限制）。
fn run_recording(rec: Arc<Recording>, spec: CaptureSpec, dir: PathBuf) {
    // 逐帧抓屏（GIF 兜底 / 非 ffmpeg 分支）期间同样暂停底板采样。
    let _capture_guard = crate::island::capture_guard::CaptureGuard::enter();
    #[cfg(target_os = "macos")]
    if spec.format == "mp4" {
        let path = dir.join(recording_name("mp4"));
        let rect = (
            spec.screen_x as f64 / spec.scale,
            spec.screen_y as f64 / spec.scale,
            (spec.width as f64 / spec.scale).max(1.0),
            (spec.height as f64 / spec.scale).max(1.0),
        );
        let started = sck::Session::start(
            rect,
            spec.out_w,
            spec.out_h,
            spec.fps,
            spec.cursor,
            spec.audio,
            &path,
        );
        match started {
            Ok(session) => {
                rec.started_ms
                    .store(super::now_ms() as u64, Ordering::Relaxed);
                loop {
                    if rec.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if rec.elapsed_ms() >= spec.max_ms {
                        break;
                    }
                    if session.finished() {
                        // 输出自己结束了（编码器出错/磁盘问题），走收尾拿错误
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(40));
                }
                let duration = rec.elapsed_ms().min(spec.max_ms);
                let outcome = session.stop().and_then(|_| {
                    let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                    if size == 0 {
                        return Err("录屏文件为空，可能是录制权限被收回".to_string());
                    }
                    Ok(RecordResult {
                        path: path.to_string_lossy().to_string(),
                        width: spec.out_w,
                        height: spec.out_h,
                        frames: ((duration as f64 / 1000.0) * spec.fps as f64) as u32,
                        duration_ms: duration,
                        size,
                        format: "mp4".into(),
                        record_id: 0,
                        has_audio: spec.audio,
                    })
                });
                rec.finish(outcome);
                return;
            }
            Err(error) => {
                eprintln!("[zbar-rec] ScreenCaptureKit 起流失败，回退逐帧抓屏: {error}");
            }
        }
    }
    let outcome = run_capture(&rec, spec);
    rec.finish(outcome);
}

fn run_capture(rec: &Arc<Recording>, spec: CaptureSpec) -> Result<RecordResult, String> {
    let dir = shot_dir()?;
    let path = dir.join(recording_name(&spec.format));
    let mut sink = match spec.format.as_str() {
        #[cfg(target_os = "macos")]
        "mp4" => Sink::Mp4(Mp4Sink::new(&path, spec.out_w, spec.out_h)?),
        _ => {
            let temp = rec
                .temp
                .clone()
                .ok_or_else(|| "录屏临时目录不可用".to_string())?;
            Sink::Gif(GifSink::new(temp, spec.out_w, spec.out_h)?)
        }
    };
    let mut below = recorder_window_number();
    // 岛/桌面刚切回来时首帧容易是旧画面，先让窗口系统画稳再抓。
    std::thread::sleep(Duration::from_millis(420));
    let started = Instant::now();
    rec.started_ms
        .store(super::now_ms() as u64, Ordering::Relaxed);
    let mut frames = 0u32;
    let mut last_at = 0u64;
    let mut last_error = String::new();

    loop {
        if rec.stop.load(Ordering::Relaxed) {
            break;
        }
        if below == 0 {
            // 录制条刚显示时窗口号可能还查不到，前几帧补查一次。
            below = recorder_window_number();
        }
        let elapsed = started.elapsed().as_millis() as u64;
        if elapsed >= spec.max_ms {
            break;
        }
        match capture_frame(&spec, below) {
            Ok(mut frame) => {
                let at = started.elapsed().as_millis() as u64;
                if spec.cursor {
                    draw_cursor(&mut frame, &spec);
                }
                if let Err(e) = sink.push(frame, at) {
                    last_error = e;
                } else {
                    last_at = at;
                    if spec.format == "gif" {
                        if let Ok(mut times) = rec.frames.lock() {
                            times.push(at);
                        }
                    }
                    frames += 1;
                }
            }
            Err(e) => last_error = e,
        }
        let interval = if spec.format == "mp4" {
            1000 / spec.fps.max(1) as u64
        } else {
            gif_interval(frames)
        };
        let target = last_at + interval;
        let used = started.elapsed().as_millis() as u64;
        if used < target {
            std::thread::sleep(Duration::from_millis(target - used));
        }
    }

    if frames == 0 {
        return Err(if last_error.is_empty() {
            "没有抓到任何录制帧".into()
        } else {
            last_error
        });
    }
    let elapsed = started.elapsed().as_millis() as u64;
    let gif_fps = rec.gif_fps.load(Ordering::Relaxed) as u32;
    sink.finish(elapsed, frames, &path, gif_fps, &rec.progress)
}

/// 一帧原始像素。macOS 直接抓成 BGRA（和 VideoToolbox 的 32BGRA 一致，
/// 省掉一次全图逐像素换道——debug 构建下这个换道能吃掉 150ms/帧），
/// 兜底的 screencapture 分支是 RGBA。
struct Frame {
    data: Vec<u8>,
    width: u32,
    height: u32,
    bgra: bool,
}

impl Frame {
    fn from_rgba(image: image::RgbaImage) -> Self {
        Self {
            width: image.width(),
            height: image.height(),
            data: image.into_raw(),
            bgra: false,
        }
    }

    /// 转成 `image`，GIF 分支缩放/编码用。
    fn into_rgba(self) -> image::RgbaImage {
        let mut data = self.data;
        if self.bgra {
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        image::RgbaImage::from_raw(self.width, self.height, data)
            .unwrap_or_else(|| image::RgbaImage::new(self.width, self.height))
    }

    /// 直接送 VideoToolbox 的 BGRA。
    fn into_bgra(self) -> (Vec<u8>, u32, u32) {
        let mut data = self.data;
        if !self.bgra {
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        (data, self.width, self.height)
    }
}

fn capture_frame(spec: &CaptureSpec, below: u32) -> Result<Frame, String> {
    #[cfg(target_os = "macos")]
    {
        let x = spec.screen_x as f64 / spec.scale;
        let y = spec.screen_y as f64 / spec.scale;
        let w = (spec.width as f64 / spec.scale).max(1.0);
        let h = (spec.height as f64 / spec.scale).max(1.0);
        let want_bgra = spec.format != "gif";
        if let Some(frame) =
            mac_capture::capture_bgra(x, y, w, h, spec.out_w, spec.out_h, below, want_bgra)
        {
            return Ok(frame);
        }
    }
    let image = super::capture_rect(spec.screen_x, spec.screen_y, spec.width, spec.height)?;
    let image = fit_image(image, spec.out_w, spec.out_h);
    Ok(Frame::from_rgba(image))
}

/// 录制条自己的窗口号：抓帧时以它为界取「它下面的窗口」，录制条就不会入画。
fn recorder_window_number() -> u32 {
    #[cfg(target_os = "macos")]
    {
        super::mac_windows::window_number(std::process::id() as i32, REC_WINDOW_TITLE)
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

enum Sink {
    Gif(GifSink),
    #[cfg(target_os = "macos")]
    Mp4(Mp4Sink),
}

impl Sink {
    fn push(&mut self, frame: Frame, at_ms: u64) -> Result<(), String> {
        match self {
            Sink::Gif(sink) => sink.push(frame, at_ms),
            #[cfg(target_os = "macos")]
            Sink::Mp4(sink) => sink.push(frame, at_ms),
        }
    }

    fn finish(
        self,
        elapsed_ms: u64,
        frames: u32,
        path: &Path,
        gif_fps: u32,
        progress: &AtomicU64,
    ) -> Result<RecordResult, String> {
        match self {
            Sink::Gif(sink) => sink.finish(elapsed_ms, frames, path, gif_fps, progress),
            #[cfg(target_os = "macos")]
            Sink::Mp4(sink) => {
                let _ = (gif_fps, progress);
                sink.finish(elapsed_ms, frames, path)
            }
        }
    }
}

struct GifSink {
    dir: PathBuf,
    width: u32,
    height: u32,
    times: Vec<u64>,
    index: u32,
}

impl GifSink {
    fn new(dir: PathBuf, width: u32, height: u32) -> Result<Self, String> {
        Ok(Self {
            dir,
            width,
            height,
            times: Vec::new(),
            index: 0,
        })
    }

    fn push(&mut self, frame: Frame, at_ms: u64) -> Result<(), String> {
        let fitted = fit_image(frame.into_rgba(), self.width, self.height);
        let path = self.dir.join(format!("f{:05}.jpg", self.index));
        let file = std::fs::File::create(&path).map_err(|e| format!("写入录制帧失败: {e}"))?;
        let mut writer = std::io::BufWriter::new(file);
        let rgb = image::DynamicImage::ImageRgba8(fitted).to_rgb8();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 82);
        encoder
            .encode_image(&rgb)
            .map_err(|e| format!("编码录制帧失败: {e}"))?;
        self.index += 1;
        self.times.push(at_ms);
        Ok(())
    }

    fn finish(
        self,
        elapsed_ms: u64,
        _frames: u32,
        path: &Path,
        gif_fps: u32,
        progress: &AtomicU64,
    ) -> Result<RecordResult, String> {
        let picked = sample_frames(&self.times, 0.0, elapsed_ms as f64, gif_fps);
        let count = encode_gif_frames(
            &self.dir,
            &picked,
            self.width,
            self.height,
            path,
            Some(progress),
        )?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Ok(RecordResult {
            path: path.to_string_lossy().to_string(),
            width: self.width,
            height: self.height,
            frames: count,
            duration_ms: elapsed_ms,
            size,
            format: "gif".into(),
            record_id: 0,
            has_audio: false,
        })
    }
}

fn fit_image(image: image::RgbaImage, width: u32, height: u32) -> image::RgbaImage {
    if image.width() == width && image.height() == height {
        return image;
    }
    image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle)
}

/// 按目标帧率挑帧：在 `[start, end)` 上按 `1000/fps` 取点，每个点要
/// 「时间上不晚于它」的最后一帧，同一帧不重复取。
fn sample_frames(times: &[u64], start_ms: f64, end_ms: f64, fps: u32) -> Vec<(usize, u64)> {
    if times.is_empty() {
        return Vec::new();
    }
    let fps = fps.clamp(1, 30) as f64;
    let step = 1000.0 / fps;
    let mut picked: Vec<(usize, u64)> = Vec::new();
    let mut cursor = 0usize;
    let mut t = start_ms.max(0.0);
    while t < end_ms.max(start_ms + step) {
        while cursor + 1 < times.len() && (times[cursor + 1] as f64) <= t + 0.5 {
            cursor += 1;
        }
        if (times[cursor] as f64) <= t + 0.5 || cursor == 0 {
            if picked.last().map(|(index, _)| *index) != Some(cursor) {
                picked.push((cursor, times[cursor]));
            }
        }
        t += step;
    }
    if picked.is_empty() {
        picked.push((0, times[0]));
    }
    picked
}

/// 单帧解码 + 量化；放线程里跑。
fn render_gif_frame(
    dir: &Path,
    index: usize,
    width: u32,
    height: u32,
) -> Result<gif::Frame<'static>, String> {
    let frame_path = dir.join(format!("f{index:05}.jpg"));
    let image = image::open(&frame_path)
        .map_err(|e| format!("读取录制帧失败: {e}"))?
        .to_rgba8();
    let image = fit_image(image, width, height);
    let mut rgba = image.into_raw();
    Ok(gif::Frame::from_rgba_speed(
        width as u16,
        height as u16,
        &mut rgba,
        24,
    ))
}

/// 把临时目录里的 JPEG 帧编成 GIF。量化是最费 CPU 的一步，按机器核数
/// 并行做（每批 `核数×2` 帧），主线程只负责按顺序写文件 + 报进度。
fn encode_gif_frames(
    dir: &Path,
    frames: &[(usize, u64)],
    width: u32,
    height: u32,
    path: &Path,
    progress: Option<&AtomicU64>,
) -> Result<u32, String> {
    if frames.is_empty() {
        return Err("没有抓到任何录制帧".into());
    }
    let file = std::fs::File::create(path).map_err(|e| format!("创建 GIF 失败: {e}"))?;
    let writer = std::io::BufWriter::new(file);
    let mut encoder = gif::Encoder::new(writer, width as u16, height as u16, &[])
        .map_err(|e| format!("初始化 GIF 失败: {e}"))?;
    encoder
        .set_repeat(gif::Repeat::Infinite)
        .map_err(|e| format!("设置 GIF 循环失败: {e}"))?;

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let total = frames.len();
    let batch = (threads * 2).max(2);
    let mut written = 0u32;
    let mut at = 0usize;
    while at < total {
        let end = (at + batch).min(total);
        let slice = &frames[at..end];
        let mut rendered: Vec<Option<Result<gif::Frame<'static>, String>>> =
            (0..slice.len()).map(|_| None).collect();
        std::thread::scope(|scope| {
            let chunk = (slice.len() + threads - 1) / threads;
            let mut base = 0usize;
            let mut rest: &mut [Option<Result<gif::Frame<'static>, String>>] = &mut rendered;
            while base < slice.len() && !rest.is_empty() {
                let take = chunk.min(rest.len());
                let (head, tail) = rest.split_at_mut(take);
                let items = &slice[base..base + take];
                scope.spawn(move || {
                    for (slot, (index, _)) in head.iter_mut().zip(items.iter()) {
                        *slot = Some(render_gif_frame(dir, *index, width, height));
                    }
                });
                base += take;
                rest = tail;
            }
        });
        for (offset, item) in rendered.into_iter().enumerate() {
            let mut frame = item
                .ok_or_else(|| "GIF 帧编码中断".to_string())??;
            let at_ms = frames[at + offset].1;
            let next = frames.get(at + offset + 1).map(|(_, t)| *t).unwrap_or(at_ms + 100);
            frame.delay = (((next.saturating_sub(at_ms)) / 10) as u16).clamp(2, 200);
            encoder
                .write_frame(&frame)
                .map_err(|e| format!("写入 GIF 帧失败: {e}"))?;
            written += 1;
        }
        if let Some(slot) = progress {
            let percent = ((written as u64 * 100) / total as u64).min(100);
            slot.store(percent, Ordering::Relaxed);
        }
        at = end;
    }
    drop(encoder);
    if let Some(slot) = progress {
        slot.store(100, Ordering::Relaxed);
    }
    Ok(written)
}

// ============================================================
// macOS：CGWindowListCreateImage 抓帧（比 screencapture 快一个数量级）
// ============================================================

#[cfg(target_os = "macos")]
mod mac_capture {
    use std::ffi::{c_char, c_void, CStr};
    use std::sync::OnceLock;

    type CGImageRef = *mut c_void;
    type CGContextRef = *mut c_void;
    type CGColorSpaceRef = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    type CreateImageFn = unsafe extern "C" fn(CGRect, u32, u32, u32) -> CGImageRef;

    const ON_SCREEN_ONLY: u32 = 1;
    const ON_SCREEN_BELOW_WINDOW: u32 = 4;
    const IMAGE_DEFAULT: u32 = 0;
    const IMAGE_SHOULD_BE_OPAQUE: u32 = 2;
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    // premultipliedFirst | byteorder32Little，内存里就是 B,G,R,A
    const BITMAP_BGRA: u32 = 2 | (2 << 12);
    // premultipliedLast | byteorder32Big，内存里就是 R,G,B,A（GIF 用）
    const BITMAP_RGBA: u32 = 1 | (4 << 12);
    const INTERPOLATION_HIGH: u32 = 3;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGImageGetWidth(image: CGImageRef) -> usize;
        fn CGImageGetHeight(image: CGImageRef) -> usize;
        fn CGImageRelease(image: CGImageRef);
        fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
        fn CGColorSpaceRelease(space: CGColorSpaceRef);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits: usize,
            row_bytes: usize,
            space: CGColorSpaceRef,
            bitmap_info: u32,
        ) -> CGContextRef;
        fn CGContextDrawImage(context: CGContextRef, rect: CGRect, image: CGImageRef);
        fn CGContextSetInterpolationQuality(context: CGContextRef, quality: u32);
        fn CGContextRelease(context: CGContextRef);
    }

    extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    /// 直接要一块未初始化的内存：每帧 16MB 的清零也是实打实的开销。
    fn uninit_bytes(len: usize) -> Vec<u8> {
        let mut data = Vec::<u8>::with_capacity(len);
        // SAFETY: 马上会被 CGContextDrawImage 整块覆写，不会读到未初始化值。
        unsafe { data.set_len(len) };
        data
    }

    fn create_image() -> Option<CreateImageFn> {
        static FN: OnceLock<Option<usize>> = OnceLock::new();
        let address = *FN.get_or_init(|| unsafe {
            let name = CStr::from_bytes_with_nul(b"CGWindowListCreateImage\0").ok()?;
            let ptr = dlsym(RTLD_DEFAULT, name.as_ptr());
            (!ptr.is_null()).then_some(ptr as usize)
        });
        address.map(|addr| unsafe { std::mem::transmute::<usize, CreateImageFn>(addr) })
    }

    /// 抓 `[x, y, w, h]`（全局逻辑点）并直接缩放到 `out_w × out_h` 的 BGRA。
    /// `below` 非 0 时只取该窗口下方的画面，用来把录制条本身排除在录制之外。
    pub fn capture_bgra(
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        out_w: u32,
        out_h: u32,
        below: u32,
        want_bgra: bool,
    ) -> Option<super::Frame> {
        if w < 1.0 || h < 1.0 {
            return None;
        }
        let create = create_image()?;
        let rect = CGRect {
            origin: CGPoint { x, y },
            size: CGSize {
                width: w,
                height: h,
            },
        };
        let (option, relative) = if below > 0 {
            (ON_SCREEN_BELOW_WINDOW, below)
        } else {
            (ON_SCREEN_ONLY, 0)
        };
        unsafe {
            // ShouldBeOpaque：抓屏幕不需要 alpha，实测快 30% 左右。
            let image = create(
                rect,
                option,
                relative,
                IMAGE_DEFAULT | IMAGE_SHOULD_BE_OPAQUE,
            );
            if image.is_null() {
                return None;
            }
            let width = out_w.max(2) as usize;
            let height = out_h.max(2) as usize;
            let mut data = uninit_bytes(width * height * 4);
            let space = CGColorSpaceCreateDeviceRGB();
            let context = CGBitmapContextCreate(
                data.as_mut_ptr() as *mut c_void,
                width,
                height,
                8,
                width * 4,
                space,
                if want_bgra { BITMAP_BGRA } else { BITMAP_RGBA },
            );
            CGColorSpaceRelease(space);
            if context.is_null() {
                CGImageRelease(image);
                return None;
            }
            // 缩放交给 CoreGraphics（比在 Rust 里做快得多）
            CGContextSetInterpolationQuality(context, INTERPOLATION_HIGH);
            CGContextDrawImage(
                context,
                CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: width as f64,
                        height: height as f64,
                    },
                },
                image,
            );
            CGContextRelease(context);
            CGImageRelease(image);
            // CGBitmapContext 的第一行就是画面顶行，直接用，不要再翻一次。
            Some(super::Frame {
                data,
                width: width as u32,
                height: height as u32,
                bgra: want_bgra,
            })
        }
    }
}

// ============================================================
// 鼠标指针
// ============================================================

/// 白色箭头 + 深色描边，按输出图比例画在当前光标位置。
fn draw_cursor(frame: &mut Frame, spec: &CaptureSpec) {
    let Some((cx, cy)) = super::cursor_position() else {
        return;
    };
    // 抓取区域（物理像素）→ 输出图（可能缩到 2560 / 1000 宽）
    let ratio = frame.width as f64 / spec.width.max(1) as f64;
    let px = (cx - spec.cursor_origin.0) as f64 * ratio;
    let py = (cy - spec.cursor_origin.1) as f64 * ratio;
    if px < -40.0
        || py < -40.0
        || px > frame.width as f64 + 40.0
        || py > frame.height as f64 + 40.0
    {
        return;
    }
    let shape: [(f64, f64); 7] = [
        (0.0, 0.0),
        (0.0, 15.0),
        (3.8, 11.4),
        (6.2, 16.8),
        (8.6, 15.6),
        (6.3, 10.4),
        (11.2, 10.4),
    ];
    let k = (spec.scale * ratio).clamp(1.0, 3.0);
    let points: Vec<(f64, f64)> = shape
        .iter()
        .map(|(sx, sy)| (px + sx * k, py + sy * k))
        .collect();
    let shadow: Vec<(f64, f64)> = points
        .iter()
        .map(|(x, y)| (x + 1.2 * k, y + 1.2 * k))
        .collect();
    let order: [usize; 3] = if frame.bgra { [2, 1, 0] } else { [0, 1, 2] };
    fill_polygon(
        &mut frame.data,
        frame.width,
        frame.height,
        &shadow,
        [0, 0, 0, 170],
        order,
    );
    fill_polygon(
        &mut frame.data,
        frame.width,
        frame.height,
        &points,
        [255, 255, 255, 255],
        order,
    );
}

fn fill_polygon(
    data: &mut [u8],
    width: u32,
    height: u32,
    points: &[(f64, f64)],
    color: [u8; 4],
    order: [usize; 3],
) {
    if points.len() < 3 || width < 2 || height < 2 {
        return;
    }
    let min_y = points
        .iter()
        .map(|p| p.1)
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(0.0) as i32;
    let max_y = points
        .iter()
        .map(|p| p.1)
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .min(height as f64 - 1.0) as i32;
    for y in min_y..=max_y {
        let scan = y as f64 + 0.5;
        let mut crossings: Vec<f64> = Vec::with_capacity(4);
        for index in 0..points.len() {
            let (x1, y1) = points[index];
            let (x2, y2) = points[(index + 1) % points.len()];
            if (y1 <= scan && y2 > scan) || (y2 <= scan && y1 > scan) {
                crossings.push(x1 + (scan - y1) * (x2 - x1) / (y2 - y1));
            }
        }
        if crossings.len() < 2 {
            continue;
        }
        crossings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut index = 0;
        while index + 1 < crossings.len() {
            let from = crossings[index].ceil().max(0.0) as i32;
            let to = crossings[index + 1].floor().min(width as f64 - 1.0) as i32;
            for x in from..=to {
                blend_pixel(data, width, height, x, y, color, order);
            }
            index += 2;
        }
    }
}

fn blend_pixel(
    data: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    color: [u8; 4],
    order: [usize; 3],
) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let index = (y as usize * width as usize + x as usize) * 4;
    if index + 3 >= data.len() {
        return;
    }
    let alpha = color[3] as f32 / 255.0;
    for (channel, &slot) in order.iter().enumerate() {
        let old = data[index + slot] as f32;
        data[index + slot] = (color[channel] as f32 * alpha + old * (1.0 - alpha)).round() as u8;
    }
    data[index + 3] = 255;
}

// ============================================================
// macOS：VideoToolbox H.264 + MP4 封装
// ============================================================

#[cfg(target_os = "macos")]
mod h264 {
    use std::ffi::c_void;
    use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
    use std::time::Duration;
    use std::sync::Mutex;

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type CVPixelBufferRef = *mut c_void;
    type CMSampleBufferRef = *mut c_void;
    type CMBlockBufferRef = *mut c_void;
    type CMFormatDescriptionRef = *mut c_void;
    type VTCompressionSessionRef = *mut c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct CMTime {
        pub value: i64,
        pub timescale: i32,
        pub flags: u32,
        pub epoch: i64,
    }

    const CM_TIME_VALID: u32 = 1;
    const CODEC_H264: u32 = 0x6176_6331; // 'avc1'
    const PIXEL_BGRA: u32 = 0x4247_5241; // 'BGRA'

    type VTOutputCallback =
        extern "C" fn(*mut c_void, *mut c_void, i32, u32, CMSampleBufferRef);

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        static kCFBooleanTrue: CFTypeRef;
        static kCFBooleanFalse: CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFDictionaryCreate(
            allocator: CFAllocatorRef,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFDictionaryRef;
        fn CFNumberCreate(allocator: CFAllocatorRef, kind: i32, value: *const c_void) -> CFTypeRef;
        fn CFArrayGetCount(array: CFArrayRef) -> isize;
    }

    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;
        fn CVPixelBufferCreate(
            allocator: CFAllocatorRef,
            width: usize,
            height: usize,
            format: u32,
            attributes: CFDictionaryRef,
            out: *mut CVPixelBufferRef,
        ) -> i32;
        fn CVPixelBufferLockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
        fn CVPixelBufferGetBaseAddress(buffer: CVPixelBufferRef) -> *mut c_void;
        fn CVPixelBufferGetBytesPerRow(buffer: CVPixelBufferRef) -> usize;
    }

    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {
        static kCMSampleAttachmentKey_NotSync: CFStringRef;
        fn CMSampleBufferGetDataBuffer(buffer: CMSampleBufferRef) -> CMBlockBufferRef;
        fn CMBlockBufferGetDataLength(buffer: CMBlockBufferRef) -> usize;
        fn CMBlockBufferCopyDataBytes(
            buffer: CMBlockBufferRef,
            offset: usize,
            length: usize,
            destination: *mut c_void,
        ) -> i32;
        fn CMSampleBufferGetFormatDescription(buffer: CMSampleBufferRef) -> CMFormatDescriptionRef;
        fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            description: CMFormatDescriptionRef,
            index: usize,
            parameter_set: *mut *const u8,
            parameter_set_size: *mut usize,
            parameter_set_count: *mut usize,
            nal_header_length: *mut i32,
        ) -> i32;
        fn CMSampleBufferGetSampleAttachmentsArray(
            buffer: CMSampleBufferRef,
            create: bool,
        ) -> CFArrayRef;
        fn CMSampleBufferGetPresentationTimeStamp(buffer: CMSampleBufferRef) -> CMTime;
        fn CFDictionaryContainsKey(dict: CFDictionaryRef, key: *const c_void) -> bool;
        fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, index: isize) -> *const c_void;
    }

    #[link(name = "VideoToolbox", kind = "framework")]
    extern "C" {
        static kVTCompressionPropertyKey_RealTime: CFStringRef;
        static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
        static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
        static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
        static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
        static kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: CFStringRef;
        fn VTCompressionSessionCreate(
            allocator: CFAllocatorRef,
            width: i32,
            height: i32,
            codec: u32,
            encoder_specification: CFDictionaryRef,
            source_attributes: CFDictionaryRef,
            compressed_allocator: CFAllocatorRef,
            callback: VTOutputCallback,
            refcon: *mut c_void,
            out: *mut VTCompressionSessionRef,
        ) -> i32;
        fn VTSessionSetProperty(
            session: VTCompressionSessionRef,
            key: CFStringRef,
            value: CFTypeRef,
        ) -> i32;
        fn VTCompressionSessionEncodeFrame(
            session: VTCompressionSessionRef,
            image: CVPixelBufferRef,
            pts: CMTime,
            duration: CMTime,
            frame_properties: CFDictionaryRef,
            refcon: *mut c_void,
            flags: *mut u32,
        ) -> i32;
        fn VTCompressionSessionCompleteFrames(
            session: VTCompressionSessionRef,
            until: CMTime,
        ) -> i32;
        fn VTCompressionSessionInvalidate(session: VTCompressionSessionRef);
    }

    pub struct EncodedSample {
        pub pts_ms: i64,
        pub sync: bool,
        pub data: Vec<u8>,
    }

    struct Context {
        tx: Sender<EncodedSample>,
        params: Mutex<Option<(Vec<u8>, Vec<u8>)>>,
    }

    extern "C" fn on_sample(
        refcon: *mut c_void,
        _source: *mut c_void,
        status: i32,
        _flags: u32,
        sample: CMSampleBufferRef,
    ) {
        if status != 0 || sample.is_null() || refcon.is_null() {
            return;
        }
        unsafe {
            let context = &*(refcon as *const Context);
            let description = CMSampleBufferGetFormatDescription(sample);
            if !description.is_null() && context.params.lock().map(|p| p.is_none()).unwrap_or(false)
            {
                let mut sps: Option<Vec<u8>> = None;
                let mut pps: Option<Vec<u8>> = None;
                for index in 0..2usize {
                    let mut pointer: *const u8 = std::ptr::null();
                    let mut size = 0usize;
                    let mut count = 0usize;
                    let mut nal = 0i32;
                    let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                        description,
                        index,
                        &mut pointer,
                        &mut size,
                        &mut count,
                        &mut nal,
                    );
                    if status == 0 && !pointer.is_null() && size > 0 {
                        let bytes = std::slice::from_raw_parts(pointer, size).to_vec();
                        if index == 0 {
                            sps = Some(bytes);
                        } else {
                            pps = Some(bytes);
                        }
                    }
                }
                if let (Some(sps), Some(pps)) = (sps, pps) {
                    if let Ok(mut slot) = context.params.lock() {
                        *slot = Some((sps, pps));
                    }
                }
            }
            let block = CMSampleBufferGetDataBuffer(sample);
            if block.is_null() {
                return;
            }
            let length = CMBlockBufferGetDataLength(block);
            if length == 0 {
                return;
            }
            let mut data = vec![0u8; length];
            if CMBlockBufferCopyDataBytes(block, 0, length, data.as_mut_ptr() as *mut c_void) != 0 {
                return;
            }
            let mut sync = true;
            let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, false);
            if !attachments.is_null() && CFArrayGetCount(attachments) > 0 {
                let dict = CFArrayGetValueAtIndex(attachments, 0) as CFDictionaryRef;
                if CFDictionaryContainsKey(dict, kCMSampleAttachmentKey_NotSync) {
                    let value = CFDictionaryGetValue(dict, kCMSampleAttachmentKey_NotSync);
                    sync = value.is_null() || value == kCFBooleanTrue;
                }
            }
            let pts = CMSampleBufferGetPresentationTimeStamp(sample);
            let pts_ms = if pts.timescale > 0 {
                pts.value * 1000 / pts.timescale as i64
            } else {
                0
            };
            let _ = context.tx.send(EncodedSample { pts_ms, sync, data });
        }
    }

    pub struct Encoder {
        session: VTCompressionSessionRef,
        context: *mut Context,
        rx: Receiver<EncodedSample>,
        width: u32,
        height: u32,
        pixel_attributes: CFDictionaryRef,
    }

    fn cm_time(ms: i64) -> CMTime {
        CMTime {
            value: ms,
            timescale: 1000,
            flags: CM_TIME_VALID,
            epoch: 0,
        }
    }

    impl Encoder {
        pub fn new(width: u32, height: u32, fps: u32, bitrate: i32) -> Result<Self, String> {
            unsafe {
                let (tx, rx) = channel();
                let context = Box::into_raw(Box::new(Context {
                    tx,
                    params: Mutex::new(None),
                }));
                let mut spec_keys = [kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder];
                let mut spec_values = [kCFBooleanTrue];
                let spec = CFDictionaryCreate(
                    std::ptr::null(),
                    spec_keys.as_mut_ptr(),
                    spec_values.as_mut_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks as *const c_void,
                    &kCFTypeDictionaryValueCallBacks as *const c_void,
                );
                let mut session: VTCompressionSessionRef = std::ptr::null_mut();
                let status = VTCompressionSessionCreate(
                    std::ptr::null(),
                    width as i32,
                    height as i32,
                    CODEC_H264,
                    spec,
                    std::ptr::null(),
                    std::ptr::null(),
                    on_sample,
                    context as *mut c_void,
                    &mut session,
                );
                if !spec.is_null() {
                    CFRelease(spec);
                }
                if status != 0 || session.is_null() {
                    drop(Box::from_raw(context));
                    return Err(format!("创建视频编码器失败（{status}）"));
                }
                let set_number = |key: CFStringRef, value: i32| {
                    let number = CFNumberCreate(
                        std::ptr::null(),
                        3, // kCFNumberSInt32Type
                        &value as *const i32 as *const c_void,
                    );
                    if number.is_null() {
                        return;
                    }
                    let _ = VTSessionSetProperty(session, key, number);
                    CFRelease(number);
                };
                // 不用 RealTime：RT 模式在编码跟不上时会主动丢帧，录出来
                // 只剩零星几帧（表现为“录了 8 秒预览只有 3 秒”）。
                let _ = VTSessionSetProperty(
                    session,
                    kVTCompressionPropertyKey_RealTime,
                    kCFBooleanFalse,
                );
                let _ = VTSessionSetProperty(
                    session,
                    kVTCompressionPropertyKey_AllowFrameReordering,
                    kCFBooleanFalse,
                );
                set_number(kVTCompressionPropertyKey_AverageBitRate, bitrate.max(800_000));
                set_number(kVTCompressionPropertyKey_ExpectedFrameRate, fps.max(1) as i32);
                set_number(
                    kVTCompressionPropertyKey_MaxKeyFrameInterval,
                    (fps.max(1) * 4) as i32,
                );

                let mut keys = [kCVPixelBufferIOSurfacePropertiesKey];
                let mut empty = [CFDictionaryCreate(
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    &kCFTypeDictionaryKeyCallBacks as *const c_void,
                    &kCFTypeDictionaryValueCallBacks as *const c_void,
                )];
                let attributes = CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_mut_ptr(),
                    empty.as_mut_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks as *const c_void,
                    &kCFTypeDictionaryValueCallBacks as *const c_void,
                );
                if !empty[0].is_null() {
                    CFRelease(empty[0]);
                }
                Ok(Self {
                    session,
                    context,
                    rx,
                    width,
                    height,
                    pixel_attributes: attributes,
                })
            }
        }

        pub fn params(&self) -> Option<(Vec<u8>, Vec<u8>)> {
            unsafe {
                let context = &*self.context;
                context.params.lock().ok().and_then(|slot| slot.clone())
            }
        }

        /// 送一帧 BGRA（`bytes` 的 stride 是 `width * 4`）。
        pub fn encode_bgra(&self, bytes: &[u8], pts_ms: i64) -> Result<(), String> {
            unsafe {
                let mut buffer: CVPixelBufferRef = std::ptr::null_mut();
                let status = CVPixelBufferCreate(
                    std::ptr::null(),
                    self.width as usize,
                    self.height as usize,
                    PIXEL_BGRA,
                    self.pixel_attributes,
                    &mut buffer,
                );
                if status != 0 || buffer.is_null() {
                    return Err(format!("创建视频帧失败（{status}）"));
                }
                if CVPixelBufferLockBaseAddress(buffer, 0) != 0 {
                    CFRelease(buffer as CFTypeRef);
                    return Err("锁定视频帧失败".into());
                }
                let base = CVPixelBufferGetBaseAddress(buffer) as *mut u8;
                let stride = CVPixelBufferGetBytesPerRow(buffer);
                let row = self.width as usize * 4;
                for y in 0..self.height as usize {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr().add(y * row),
                        base.add(y * stride),
                        row,
                    );
                }
                CVPixelBufferUnlockBaseAddress(buffer, 0);
                let pts = cm_time(pts_ms);
                let flags = VTCompressionSessionEncodeFrame(
                    self.session,
                    buffer,
                    pts,
                    cm_time(1000 / 20),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                CFRelease(buffer as CFTypeRef);
                if flags != 0 {
                    return Err(format!("编码视频帧失败（{flags}）"));
                }
                Ok(())
            }
        }

        pub fn drain(&self) -> Vec<EncodedSample> {
            let mut out = Vec::new();
            loop {
                match self.rx.try_recv() {
                    Ok(sample) => out.push(sample),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
            out
        }

        /// 收尾：等编码器把这批帧吐完，回调里送出来的样本全部收进 `out`。
        pub fn flush(&self, out: &mut Vec<EncodedSample>) {
            unsafe {
                let _ = VTCompressionSessionCompleteFrames(
                    self.session,
                    CMTime {
                        value: 0,
                        timescale: 1000,
                        flags: 0,
                        epoch: 0,
                    },
                );
            }
            std::thread::sleep(Duration::from_millis(120));
            out.extend(self.drain());
        }
    }

    impl Drop for Encoder {
        fn drop(&mut self) {
            unsafe {
                if !self.session.is_null() {
                    VTCompressionSessionInvalidate(self.session);
                    CFRelease(self.session as CFTypeRef);
                }
                if !self.pixel_attributes.is_null() {
                    CFRelease(self.pixel_attributes);
                }
                drop(Box::from_raw(self.context));
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod mp4_track {
    use std::fs::File;
    use std::io::BufWriter;
    use std::path::Path;

    pub struct Mp4Writer {
        writer: mp4::Mp4Writer<BufWriter<File>>,
        track_ready: bool,
        pending: Option<(i64, bool, Vec<u8>)>,
        last_duration: u32,
        /// 最后一个样本的结束时间（ms），结果时长以它为准
        pub last_end_ms: u64,
    }

    impl Mp4Writer {
        pub fn new(path: &Path) -> Result<Self, String> {
            let file = File::create(path).map_err(|e| format!("创建视频文件失败: {e}"))?;
            let config = mp4::Mp4Config {
                major_brand: str::parse("isom").map_err(|e| format!("MP4 品牌无效: {e}"))?,
                minor_version: 512,
                compatible_brands: vec![
                    str::parse("isom").map_err(|e| format!("MP4 品牌无效: {e}"))?,
                    str::parse("iso2").map_err(|e| format!("MP4 品牌无效: {e}"))?,
                    str::parse("avc1").map_err(|e| format!("MP4 品牌无效: {e}"))?,
                    str::parse("mp41").map_err(|e| format!("MP4 品牌无效: {e}"))?,
                ],
                timescale: 1000,
            };
            let writer =
                mp4::Mp4Writer::write_start(BufWriter::new(file), &config).map_err(|e| {
                    format!("初始化 MP4 失败: {e}")
                })?;
            Ok(Self {
                writer,
                track_ready: false,
                pending: None,
                last_duration: 50,
                last_end_ms: 0,
            })
        }

        pub fn write(
            &mut self,
            params: Option<(Vec<u8>, Vec<u8>)>,
            width: u32,
            height: u32,
            sample: super::h264::EncodedSample,
        ) -> Result<(), String> {
            if !self.track_ready {
                let Some((sps, pps)) = params else {
                    return Ok(());
                };
                let config = mp4::TrackConfig {
                    track_type: mp4::TrackType::Video,
                    timescale: 1000,
                    language: "und".into(),
                    media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                        width: width as u16,
                        height: height as u16,
                        seq_param_set: sps,
                        pic_param_set: pps,
                    }),
                };
                self.writer
                    .add_track(&config)
                    .map_err(|e| format!("写入视频轨道失败: {e}"))?;
                self.track_ready = true;
            }
            if let Some((pts, sync, data)) = self.pending.take() {
                let duration = (sample.pts_ms - pts).clamp(1, 60_000) as u32;
                self.last_duration = duration;
                self.write_one(pts, duration, sync, data)?;
            }
            self.pending = Some((sample.pts_ms, sample.sync, sample.data));
            Ok(())
        }

        fn write_one(
            &mut self,
            pts: i64,
            duration: u32,
            sync: bool,
            data: Vec<u8>,
        ) -> Result<(), String> {
            self.last_end_ms = pts.max(0) as u64 + duration as u64;
            let sample = mp4::Mp4Sample {
                start_time: pts.max(0) as u64,
                duration,
                rendering_offset: 0,
                is_sync: sync,
                bytes: mp4::Bytes::from(data),
            };
            self.writer
                .write_sample(1, &sample)
                .map_err(|e| format!("写入视频帧失败: {e}"))
        }

        pub fn finish(mut self) -> Result<(), String> {
            if let Some((pts, sync, data)) = self.pending.take() {
                let duration = self.last_duration.max(1);
                self.write_one(pts, duration, sync, data)?;
            }
            self.writer
                .write_end()
                .map_err(|e| format!("收尾 MP4 失败: {e}"))?;
            use std::io::Write;
            let mut writer = self.writer.into_writer();
            writer.flush().map_err(|e| format!("刷新视频文件失败: {e}"))?;
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
struct Mp4Sink {
    encoder: h264::Encoder,
    writer: mp4_track::Mp4Writer,
    width: u32,
    height: u32,
    frames: u32,
    first_pts: i64,
    last_pts: i64,
    /// 逐帧抓屏回退路径没有音频（纯 VideoToolbox 视频轨）
    has_audio: bool,
}

#[cfg(target_os = "macos")]
impl Mp4Sink {
    fn new(path: &Path, width: u32, height: u32) -> Result<Self, String> {
        let encoder = h264::Encoder::new(width, height, MP4_FPS as u32, bitrate_for(width, height))?;
        Ok(Self {
            encoder,
            writer: mp4_track::Mp4Writer::new(path)?,
            width,
            height,
            frames: 0,
            first_pts: -1,
            last_pts: -1,
            has_audio: false,
        })
    }

    fn push(&mut self, frame: Frame, at_ms: u64) -> Result<(), String> {
        let (bgra, width, height) = frame.into_bgra();
        if width != self.width || height != self.height {
            return Err(format!(
                "录制帧尺寸不一致: {width}×{height} != {}×{}",
                self.width, self.height
            ));
        }
        // PTS 必须严格递增：同一毫秒抓到两帧时不能出现相同时间戳，
        // 否则 MP4 的 stts 会抖出重复 dts（播放器会报 non-monotonic）。
        let raw = at_ms as i64;
        if self.first_pts < 0 {
            self.first_pts = raw;
        }
        let pts = (raw - self.first_pts).max(self.last_pts + 1);
        self.last_pts = pts;
        self.encoder.encode_bgra(&bgra, pts)?;
        self.frames += 1;
        self.flush_samples()
    }

    fn flush_samples(&mut self) -> Result<(), String> {
        for sample in self.encoder.drain() {
            let params = self.encoder.params();
            self.writer.write(params, self.width, self.height, sample)?;
        }
        Ok(())
    }

    fn finish(mut self, elapsed_ms: u64, frames: u32, path: &Path) -> Result<RecordResult, String> {
        let mut tail = Vec::new();
        self.encoder.flush(&mut tail);
        for sample in tail {
            let params = self.encoder.params();
            self.writer.write(params, self.width, self.height, sample)?;
        }
        let last_end = self.writer.last_end_ms;
        self.writer.finish()?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        // 时长以视频实际最后一帧为准：抓帧慢/掉帧时预览时间轴才不会和
        // 播放时长对不上（之前用墙钟时长，8 秒的录制可能只播 3 秒）。
        let duration_ms = if last_end > 0 { last_end } else { elapsed_ms };
        Ok(RecordResult {
            path: path.to_string_lossy().to_string(),
            width: self.width,
            height: self.height,
            frames: frames.max(self.frames),
            duration_ms,
            size,
            format: "mp4".into(),
            record_id: 0,
            has_audio: self.has_audio,
        })
    }
}

#[cfg(target_os = "macos")]
fn bitrate_for(width: u32, height: u32) -> i32 {
    let pixels = width as i64 * height as i64;
    ((pixels / 1000) * 4).clamp(1_500_000, 12_000_000) as i32
}

// ============================================================
// 裁剪 / 导出
// ============================================================

/// 把录制文件裁剪成 `[start_ms, end_ms)`；GIF 按帧重编，MP4 走系统 avconvert。
fn trim_recording(
    rec: &Arc<Recording>,
    source: &Path,
    result: &RecordResult,
    start_ms: f64,
    end_ms: f64,
) -> Result<PathBuf, String> {
    let out = std::env::temp_dir().join(format!(
        "agenthud-trim-{}-{}.{}",
        std::process::id(),
        rec.id,
        result.format
    ));
    if result.format == "gif" {
        let dir = rec
            .temp
            .clone()
            .ok_or_else(|| "录制帧已清理，无法裁剪 GIF".to_string())?;
        let times = rec
            .frames
            .lock()
            .map_err(|_| "录制帧不可用".to_string())?
            .clone();
        let times = if times.is_empty() {
            source_frame_times(&dir)?
        } else {
            times
        };
        let fps = rec.gif_fps.load(Ordering::Relaxed) as u32;
        let picked = sample_frames(&times, start_ms, end_ms, fps);
        if picked.is_empty() {
            return Err("裁剪区间里没有录制帧".into());
        }
        encode_gif_frames(&dir, &picked, result.width, result.height, &out, None)?;
        return Ok(out);
    }
    #[cfg(target_os = "macos")]
    {
        let start = start_ms / 1000.0;
        let duration = ((end_ms - start_ms) / 1000.0).max(0.1);
        let run = |preset: &str| -> bool {
            std::process::Command::new("/usr/bin/avconvert")
                .args([
                    "-s",
                    &source.to_string_lossy(),
                    "-p",
                    preset,
                    "-o",
                    &out.to_string_lossy(),
                    "--start",
                    &format!("{start:.3}"),
                    "--duration",
                    &format!("{duration:.3}"),
                    "--replace",
                    "--disableMetadataFilter",
                ])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        };
        if run("PresetPassthrough") || run("PresetHighestQuality") {
            return Ok(out);
        }
        Err("裁剪视频失败（avconvert 未能完成）".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows：ffmpeg 重编码裁剪（精确到 0.1s 级；不做 -c copy 快速模式，
        // 关键帧对齐的误差对预览里的精确裁剪体验不友好）。
        let ff = ffmpeg::Ffmpeg::locate()?;
        let args = ffmpeg::build_trim_args(source, &out, start_ms, end_ms);
        ffmpeg::run_to_done(&ff, &args)?;
        let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        if size == 0 {
            return Err("裁剪没有产出可用的视频文件".into());
        }
        Ok(out)
    }
}

fn source_frame_times(dir: &Path) -> Result<Vec<u64>, String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("读取录制帧失败: {e}"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".jpg"))
        .collect();
    names.sort();
    Ok((0..names.len() as u64).map(|i| i * GIF_INTERVAL_MS).collect())
}

fn clipboard_file(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let escaped = path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let output = std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                &format!("set the clipboard to (POSIX file \"{escaped}\")"),
            ])
            .output()
            .map_err(|e| format!("调用剪贴板失败: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "复制失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        use clipboard_win::formats::FileList;
        use clipboard_win::{Clipboard, Setter};
        // 直接写 CF_HDROP 文件列表：不再 spawn powershell（会弹一个黑框，
        // 而且 Set-Clipboard -Path 在部分机器上直接失败）。粘贴到资源管理器
        // / 微信 / 飞书都是一个文件。
        let _clip = Clipboard::new_attempts(10)
            .map_err(|e| format!("剪贴板被占用，复制失败: {e}"))?;
        let files = [path.to_string_lossy().to_string()];
        FileList
            .write_clipboard(&files)
            .map_err(|e| format!("写入剪贴板失败: {e}"))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
        Err("当前系统暂不支持复制录制文件".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_output_size_is_capped() {
        assert_eq!(output_size("gif", 400, 300), (400, 300));
        assert_eq!(output_size("gif", 2000, 1000), (1000, 500));
    }

    #[test]
    fn mp4_output_size_is_even() {
        assert_eq!(output_size("mp4", 1001, 701), (1000, 700));
        assert_eq!(output_size("mp4", 600, 300), (600, 300));
    }

    #[test]
    fn format_defaults_to_mp4_on_macos() {
        #[cfg(target_os = "macos")]
        assert_eq!(normalize_format(None, true), "mp4");
        assert_eq!(normalize_format(Some("gif"), true), "gif");
        // ffmpeg 缺席的 Windows：请求 mp4 / 不指定都降级 gif，显式 gif 不变
        assert_eq!(normalize_format(Some("mp4"), false), "gif");
        assert_eq!(normalize_format(None, false), "gif");
        assert_eq!(normalize_format(Some("gif"), false), "gif");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn encodes_frames_into_playable_mp4() {
        let dir = std::env::temp_dir().join(format!("agenthud-mp4-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("clip.mp4");
        let mut sink = Mp4Sink::new(&path, 320, 200).expect("sink");
        for index in 0..24u32 {
            let mut image = image::RgbaImage::new(320, 200);
            for (x, y, pixel) in image.enumerate_pixels_mut() {
                *pixel = image::Rgba([
                    ((x * 2 + index * 6) % 256) as u8,
                    ((y * 2 + index * 3) % 256) as u8,
                    140,
                    255,
                ]);
            }
            sink.push(Frame::from_rgba(image), index as u64 * 50)
                .expect("encode frame");
        }
        let result = sink.finish(1200, 24, &path).expect("finish");
        let bytes = std::fs::read(&path).expect("read mp4");
        assert!(result.size > 2000, "mp4 too small: {}", result.size);
        assert_eq!(&bytes[4..8], b"ftyp", "missing mp4 ftyp box");
        assert!(result.path.ends_with(".mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gif_encoder_writes_animated_file() {
        let dir = std::env::temp_dir().join(format!("agenthud-gif-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        for index in 0..6u32 {
            let image = image::RgbaImage::from_pixel(64, 48, image::Rgba([index as u8 * 30, 80, 160, 255]));
            let file = std::fs::File::create(dir.join(format!("f{index:05}.jpg"))).unwrap();
            let mut writer = std::io::BufWriter::new(file);
            let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 80)
                .encode_image(&rgb)
                .expect("jpeg");
        }
        let times: Vec<u64> = (0..6).map(|i| i * 100).collect();
        let picked: Vec<(usize, u64)> = times.iter().copied().enumerate().map(|(i, t)| (i, t)).collect();
        let out = dir.join("clip.gif");
        let frames = encode_gif_frames(&dir, &picked, 64, 48, &out, None).expect("gif");
        assert_eq!(frames, 6);
        let bytes = std::fs::read(&out).expect("read gif");
        assert!(bytes.starts_with(b"GIF89a") || bytes.starts_with(b"GIF87a"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gif_trim_reads_frames_by_absolute_index() {
        // 帧 i 画成红色渐变色，裁剪从第 3 帧开始时必须读到 f00003.jpg
        let dir = std::env::temp_dir().join(format!("agenthud-gif-trim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        for index in 0..6u32 {
            let image = image::RgbaImage::from_pixel(8, 8, image::Rgba([index as u8 * 40, 0, 0, 255]));
            let file = std::fs::File::create(dir.join(format!("f{index:05}.jpg"))).unwrap();
            let mut writer = std::io::BufWriter::new(file);
            let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 90)
                .encode_image(&rgb)
                .expect("jpeg");
        }
        let all: Vec<u64> = (0..6u64).map(|i| i * 100).collect();
        let times = &all[3..6];
        let picked: Vec<(usize, u64)> = times.iter().enumerate().map(|(offset, t)| (3 + offset, *t)).collect();
        let out = dir.join("trim.gif");
        let frames = encode_gif_frames(&dir, &picked, 8, 8, &out, None).expect("gif");
        assert_eq!(frames, 3);

        let bytes = std::fs::read(&out).expect("read");
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options
            .read_info(std::io::Cursor::new(bytes))
            .expect("decode");
        let frame = decoder.read_next_frame().expect("frame").expect("frame");
        let red = frame.buffer[0];
        assert!(
            (100..=150).contains(&red),
            "裁剪后的首帧应该来自第 3 帧（红≈120），实际 {red}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn bench_large_frames() {
        let dir = std::env::temp_dir().join(format!("agenthud-bench-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let (w, h) = (2560u32, 1540u32);
        let spec = CaptureSpec {
            screen_x: 300,
            screen_y: 200,
            width: w,
            height: h,
            out_w: w,
            out_h: h,
            scale: 2.0,
            format: "mp4".into(),
            cursor: false,
            audio: false,
            fps: 60,
            cursor_origin: (300, 200),
            max_ms: 10_000,
            mic: None,
            system_audio: None,
        };
        let mut sink = Mp4Sink::new(&dir.join("bench.mp4"), w, h).expect("sink");
        let mut cap_total = std::time::Duration::ZERO;
        let mut push_total = std::time::Duration::ZERO;
        for index in 0..20u64 {
            let t0 = std::time::Instant::now();
            let image = capture_frame(&spec, 0).expect("capture");
            cap_total += t0.elapsed();
            let t1 = std::time::Instant::now();
            sink.push(image, index * 50).expect("push");
            push_total += t1.elapsed();
        }
        sink.finish(1000, 20, &dir.join("bench.mp4")).ok();
        println!(
            "BENCH 2560x1540: capture avg {:.1}ms, encode avg {:.1}ms",
            cap_total.as_secs_f64() * 1000.0 / 20.0,
            push_total.as_secs_f64() * 1000.0 / 20.0
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_window_fits_inside_monitor() {
        let monitor = ShotMonitor {
            index: 0,
            name: "main".into(),
            x: 0,
            y: 0,
            width: 2880,
            height: 1800,
            scale_factor: 2.0,
            primary: true,
        };
        let (w, h) = preview_size(&monitor, 800, 600);
        assert!(w <= 1440.0 && h <= 900.0);
        assert!(w > 400.0 && h > 300.0);
    }

    /// 录制条右下角矩形：scale=1 时是 x∈[2878-24-238?]…… 直接按公式算。
    /// 1920×1080 屏幕：bar = (1920-238-24, 1080-54-24) = (1658, 1002) 起，
    /// 尺寸 238×54。
    #[cfg(windows)]
    #[test]
    fn recorder_overlap_follows_corner_geometry() {
        let monitor = ShotMonitor {
            index: 0,
            name: "main".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1.0,
            primary: true,
        };
        // 选区在屏幕中间：不重叠
        assert!(!recorder_overlaps_selection(&monitor, 100, 100, 800, 600));
        // 选区盖住右下角（录制条位置）：重叠
        assert!(recorder_overlaps_selection(&monitor, 1650, 990, 300, 100));
        // 全屏录制：必然重叠
        assert!(recorder_overlaps_selection(&monitor, 0, 0, 1920, 1080));
        // 刚好贴着录制条上边缘之外：不重叠
        assert!(!recorder_overlaps_selection(&monitor, 1658, 0, 238, 1002));
    }
}
