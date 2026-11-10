//! 截图工作台：屏幕/窗口捕获、区域选择、标注编辑器、滚动长截图、贴图、
//! 本地 OCR/二维码识别、剪贴板与历史记录。
//!
//! 设计上把"抓屏"和"编辑"拆开：
//! - 抓屏在 Rust 的阻塞线程里完成（GDI/PrintWindow），产物是 PNG data URL；
//! - 抓屏前隐藏 AgentHUD 自己的窗口，避免把岛/贴图/主窗口拍进去；
//! - 抓完立即创建一个覆盖当前显示器的无边框窗口（`shot.html`），
//!   平时它保持隐藏，前端把图片绘制好后再调用 `shot_ready` 显示，
//!   避免 WebView 首帧白屏闪一下；
//! - 前端负责区域选择、标注、裁剪和导出 PNG，Rust 只做保存/复制/贴图/
//!   OCR/二维码识别与滚动拼接。
//!
//! 滚动截图先让用户框选区域，然后隐藏编辑器，向区域中心下的窗口模拟
//! 滚轮，逐帧抓取并用行签名搜索相邻帧重叠区，拼成一张长图。它不依赖
//! 浏览器扩展，适合网页/文档/聊天记录这类常规滚动容器。

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::ImageEncoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

/// 截图选择/编辑器窗口 label（capabilities/shot.json 同步）
pub const SHOT_WINDOW_LABEL: &str = "shot-overlay";
/// 贴图窗口 label 前缀，后缀为 pin id
pub const PIN_WINDOW_PREFIX: &str = "shot-pin-";
/// 新截图保存完成事件（HUD 截图页刷新历史）
pub const SHOT_SAVED_EVENT: &str = "zbar://shot-saved";

const MAX_HISTORY: usize = 40;
const THUMB_WIDTH: u32 = 168;

static SESSION: OnceLock<Mutex<Option<ShotSession>>> = OnceLock::new();
static PINS: OnceLock<Mutex<HashMap<u64, PinEntry>>> = OnceLock::new();
static HIDDEN_WINDOWS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static SESSION_SEQ: AtomicU64 = AtomicU64::new(1);
static PIN_SEQ: AtomicU64 = AtomicU64::new(1);
static STARTING: AtomicBool = AtomicBool::new(false);

/// macOS 抓屏比例：Tauri 的显示器几何是物理像素，`screencapture -R` 收的是
/// 逻辑点，二者相差一个 backingScaleFactor。抓屏前由调用方写入。
#[cfg(target_os = "macos")]
static MAC_SCALE_BITS: AtomicU64 = AtomicU64::new(0);
/// 屏幕录制权限只提示一次，避免反复弹窗
#[cfg(target_os = "macos")]
static MAC_PERMISSION_ASKED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
fn set_mac_scale(scale: f64) {
    if scale > 0.0 {
        MAC_SCALE_BITS.store(scale.to_bits(), Ordering::Relaxed);
    }
}

#[cfg(target_os = "macos")]
fn mac_scale() -> f64 {
    let bits = MAC_SCALE_BITS.load(Ordering::Relaxed);
    if bits == 0 {
        2.0
    } else {
        f64::from_bits(bits)
    }
}

fn session_slot() -> &'static Mutex<Option<ShotSession>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

fn pins_slot() -> &'static Mutex<HashMap<u64, PinEntry>> {
    PINS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn hidden_slot() -> &'static Mutex<Vec<String>> {
    HIDDEN_WINDOWS.get_or_init(|| Mutex::new(Vec::new()))
}

// ============================================================
// 数据契约
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotMonitor {
    pub index: usize,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 覆盖层里可点选的窗口：坐标与 `shot_session.width/height` 同一套像素
///（抓屏图上的像素，Retina 上是物理像素），前端直接拿来当选区用。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotWindow {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// 应用名，如 "Safari"
    pub app: String,
    /// 窗口标题（没有屏幕录制权限时可能为空）
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotSession {
    pub id: u64,
    /// region = 先框选；edit = 直接进入标注
    pub mode: String,
    /// PNG 原始字节只经 `shot_session_image` 以 ArrayBuffer 下发，不塞进
    /// 会话 JSON（4K 截图的 base64 会让 IPC payload 膨胀到几十 MB）。
    #[serde(skip)]
    pub image_png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// 图片在显示器物理坐标中的位置；窗口截图/滚动结果为 None，由前端居中
    pub origin: Option<ShotRect>,
    pub monitor: ShotMonitor,
    pub title: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinData {
    pub id: u64,
    pub image: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotHistoryItem {
    pub name: String,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub created_at: i64,
    pub thumb: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub ok: bool,
    pub text: String,
    pub lines: Vec<String>,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrResult {
    pub ok: bool,
    pub texts: Vec<String>,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollResult {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordResult {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub duration_ms: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShotSettings {
    pub auto_copy: bool,
    pub auto_pin: bool,
    pub play_sound: bool,
    pub format: String,
    pub delay_sec: u64,
    /// 唯一的截图快捷键：呼出覆盖层后在里面切换区域/窗口/全屏。
    /// 旧版按模式分了三个快捷键，读取时由 store 迁移到这一个。
    pub hotkey: String,
}

impl Default for ShotSettings {
    fn default() -> Self {
        Self {
            auto_copy: true,
            auto_pin: false,
            play_sound: false,
            format: "png".into(),
            delay_sec: 0,
            hotkey: DEFAULT_SHOT_HOTKEY.into(),
        }
    }
}

pub const DEFAULT_SHOT_HOTKEY: &str = "Ctrl+Alt+A";

impl ShotSettings {
    pub(crate) fn clamped(mut self) -> Self {
        if self.format != "png" && self.format != "jpg" {
            self.format = "png".into();
        }
        self.delay_sec = self.delay_sec.min(30);
        self
    }
}

#[derive(Debug, Clone)]
struct PinEntry {
    image: String,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone)]
struct Captured {
    png: Vec<u8>,
    width: u32,
    height: u32,
    title: String,
}

// ============================================================
// 基础工具
// ============================================================

fn encode_png(image: &image::RgbaImage) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    // 默认的 Adaptive 过滤在 5MP 截图上要 2 秒以上（debug 构建更慢），
    // 换成 Sub + Fast：编码快一个数量级，体积只大一点。
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Sub,
    )
    .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("编码 PNG 失败: {e}"))?;
    Ok(png)
}

fn png_data_url(image: &image::RgbaImage) -> Result<String, String> {
    Ok(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(encode_png(image)?)
    ))
}

fn decode_data_url(data: &str) -> Result<image::RgbaImage, String> {
    let body = data
        .split_once(',')
        .map(|(_, body)| body)
        .unwrap_or(data)
        .trim();
    if body.is_empty() {
        return Err("图片数据为空".into());
    }
    let bytes = STANDARD
        .decode(body)
        .map_err(|e| format!("图片 base64 解码失败: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("图片解码失败: {e}"))?;
    Ok(img.to_rgba8())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn on_main_thread_blocking<T, F>(app: &AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<T, String>>();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| format!("投递主线程失败: {e}"))?;
    rx.recv_timeout(Duration::from_secs(8))
        .map_err(|_| "等待主线程超时".to_string())?
}

async fn on_main_thread<T, F>(app: &AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || on_main_thread_blocking(&app, f))
        .await
        .map_err(|e| format!("等待主线程任务失败: {e}"))?
}

fn monitor_list(app: &AppHandle) -> Vec<ShotMonitor> {
    let primary = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| (m.position().x, m.position().y));
    let mut out = Vec::new();
    if let Ok(monitors) = app.available_monitors() {
        for (index, mon) in monitors.into_iter().enumerate() {
            let pos = mon.position();
            out.push(ShotMonitor {
                index,
                name: mon
                    .name()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format!("显示器 {}", index + 1)),
                x: pos.x,
                y: pos.y,
                width: mon.size().width,
                height: mon.size().height,
                scale_factor: mon.scale_factor(),
                primary: primary == Some((pos.x, pos.y)),
            });
        }
    }
    out.sort_by_key(|m| (m.x, m.y));
    for (index, mon) in out.iter_mut().enumerate() {
        mon.index = index;
    }
    out
}

fn monitor_for_point(app: &AppHandle, x: i32, y: i32) -> Option<ShotMonitor> {
    monitor_list(app)
        .into_iter()
        .find(|m| {
            x >= m.x && x < m.x + m.width as i32 && y >= m.y && y < m.y + m.height as i32
        })
        .or_else(|| monitor_list(app).into_iter().find(|m| m.primary))
}

fn cursor_position() -> Option<(i32, i32)> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p).ok()? };
        return Some((p.x, p.y));
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

// ============================================================
// Windows GDI 抓屏
// ============================================================

#[cfg(target_os = "windows")]
fn capture_rect(x: i32, y: i32, width: u32, height: u32) -> Result<image::RgbaImage, String> {
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
    };

    if width < 2 || height < 2 {
        return Err("截图区域过小".into());
    }
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err("获取屏幕 DC 失败".into());
        }
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            ReleaseDC(None, screen);
            return Err("创建内存 DC 失败".into());
        }
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(e) => {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return Err(format!("创建截图位图失败: {e}"));
            }
        };
        let old = SelectObject(mem, HGDIOBJ(bitmap.0));
        let ok = BitBlt(
            mem,
            0,
            0,
            width as i32,
            height as i32,
            Some(screen),
            x,
            y,
            SRCCOPY | CAPTUREBLT,
        );
        let image = if ok.is_ok() && !bits.is_null() {
            let len = (width as usize) * (height as usize) * 4;
            let raw = std::slice::from_raw_parts(bits as *const u8, len);
            let mut rgba = Vec::with_capacity(len);
            for px in raw.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
            image::RgbaImage::from_raw(width, height, rgba)
        } else {
            None
        };
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        image.ok_or_else(|| "抓取屏幕失败（BitBlt）".to_string())
    }
}

/// macOS 抓屏：走系统 `screencapture`，一次调用约 100~200ms。
/// 入参是物理像素（与 Tauri 显示器几何一致），换算成逻辑点后交给系统命令，
/// 输出仍是 Retina 物理像素图，和覆盖窗口尺寸 1:1 对齐。
#[cfg(target_os = "macos")]
fn capture_rect(x: i32, y: i32, width: u32, height: u32) -> Result<image::RgbaImage, String> {
    let scale = mac_scale().max(1.0);
    mac_screencapture(
        x as f64 / scale,
        y as f64 / scale,
        (width as f64 / scale).max(1.0),
        (height as f64 / scale).max(1.0),
    )
}

/// 调用 `/usr/sbin/screencapture` 截取逻辑点矩形（全局坐标，原点在主屏左上），
/// 直接返回 PNG 原始字节与像素尺寸——覆盖窗口只需要字节，省掉一次解码+重编码。
#[cfg(target_os = "macos")]
fn mac_screencapture_png(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(Vec<u8>, u32, u32), String> {
    use std::process::Command;

    if width < 1.0 || height < 1.0 {
        return Err("截图区域过小".into());
    }
    if !mac_screen_recording_ready() {
        return Err("没有屏幕录制权限：系统设置 → 隐私与安全性 → 屏幕录制，勾选 AgentHUD 后重试".into());
    }
    let path = std::env::temp_dir().join(format!(
        "agenthud-shot-{}-{}.png",
        std::process::id(),
        now_ms()
    ));
    let rect = format!(
        "{},{},{},{}",
        x.round() as i64,
        y.round() as i64,
        (width.round() as i64).max(1),
        (height.round() as i64).max(1)
    );
    let output = Command::new("/usr/sbin/screencapture")
        .args(["-x", "-o", "-t", "png", "-R", &rect])
        .arg(&path)
        .output()
        .map_err(|e| format!("调用 screencapture 失败: {e}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&path);
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "截屏命令执行失败".into()
        } else {
            format!("截屏失败: {err}")
        });
    }
    let bytes = std::fs::read(&path).map_err(|e| {
        let _ = std::fs::remove_file(&path);
        format!("读取截图失败: {e}")
    });
    let _ = std::fs::remove_file(&path);
    let bytes = bytes?;
    if bytes.len() < 128 {
        return Err("截屏返回空图：请确认已授予 AgentHUD 屏幕录制权限".into());
    }
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| format!("读取截图尺寸失败: {e}"))?
        .into_dimensions()
        .map_err(|e| format!("读取截图尺寸失败: {e}"))?;
    Ok((bytes, w, h))
}

/// 需要像素的调用方（滚动截图、GIF 录制）在这里解码。
#[cfg(target_os = "macos")]
fn mac_screencapture(x: f64, y: f64, width: f64, height: f64) -> Result<image::RgbaImage, String> {
    let (bytes, _, _) = mac_screencapture_png(x, y, width, height)?;
    image::load_from_memory(&bytes)
        .map_err(|e| format!("解码截图失败: {e}"))
        .map(|img| img.to_rgba8())
}

/// 抓一整块显示器：macOS 走原始 PNG 快路径，其它平台退回抓像素 + 编码。
fn capture_monitor_captured(mon: &ShotMonitor) -> Result<Captured, String> {
    #[cfg(target_os = "macos")]
    {
        let scale = mac_scale().max(1.0);
        let (png, width, height) = mac_screencapture_png(
            mon.x as f64 / scale,
            mon.y as f64 / scale,
            (mon.width as f64 / scale).max(1.0),
            (mon.height as f64 / scale).max(1.0),
        )?;
        return Ok(Captured {
            png,
            width,
            height,
            title: String::new(),
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let image = capture_rect(mon.x, mon.y, mon.width, mon.height)?;
        Ok(Captured {
            png: encode_png(&image)?,
            width: image.width(),
            height: image.height(),
            title: String::new(),
        })
    }
}

/// 检查（必要时请求）屏幕录制权限。首次调用会触发系统授权弹窗。
#[cfg(target_os = "macos")]
fn mac_screen_recording_ready() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    unsafe {
        if CGPreflightScreenCaptureAccess() {
            return true;
        }
        if !MAC_PERMISSION_ASKED.swap(true, Ordering::SeqCst) {
            // 触发一次系统授权弹窗；用户同意后需要重启应用才生效。
            let _ = CGRequestScreenCaptureAccess();
        }
        false
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn capture_rect(_x: i32, _y: i32, _width: u32, _height: u32) -> Result<image::RgbaImage, String> {
    Err("截图目前仅支持 Windows / macOS".into())
}

#[cfg(target_os = "windows")]
fn is_blankish(image: &image::RgbaImage) -> bool {
    let mut dark = 0usize;
    let mut total = 0usize;
    let step_x = (image.width() / 24).max(1);
    let step_y = (image.height() / 24).max(1);
    for y in (0..image.height()).step_by(step_y as usize) {
        for x in (0..image.width()).step_by(step_x as usize) {
            let p = image.get_pixel(x, y);
            if p[0] < 6 && p[1] < 6 && p[2] < 6 {
                dark += 1;
            }
            total += 1;
        }
    }
    total > 0 && dark * 100 / total > 92
}

#[cfg(target_os = "windows")]
fn window_title(hwnd: windows::Win32::Foundation::HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

#[cfg(target_os = "windows")]
fn window_bounds(hwnd: windows::Win32::Foundation::HWND) -> Option<(i32, i32, u32, u32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    let mut rect = RECT::default();
    let mut ok = false;
    unsafe {
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
        {
            ok = true;
        }
    }
    if !ok {
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
        unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    }
    let w = (rect.right - rect.left).max(0) as u32;
    let h = (rect.bottom - rect.top).max(0) as u32;
    if w < 2 || h < 2 {
        None
    } else {
        Some((rect.left, rect.top, w, h))
    }
}

#[cfg(target_os = "windows")]
fn blend_own_process(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid == unsafe { GetCurrentProcessId() }
}

#[cfg(target_os = "windows")]
fn active_window() -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindowTextLengthW, IsIconic, IsWindowVisible,
    };
    use std::ffi::c_void;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, TRUE};

    unsafe {
        let fg = GetForegroundWindow();
        if !fg.is_invalid() && !blend_own_process(fg) {
            let len = GetWindowTextLengthW(fg);
            if len > 0 && !IsIconic(fg).as_bool() {
                return Some(fg);
            }
        }
    }

    struct State {
        found: Option<windows::Win32::Foundation::HWND>,
        best_area: u64,
    }
    unsafe extern "system" fn enum_proc(
        hwnd: windows::Win32::Foundation::HWND,
        lparam: LPARAM,
    ) -> BOOL {
        let state = &mut *(lparam.0 as *mut State);
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return TRUE;
            }
            if blend_own_process(hwnd) {
                return TRUE;
            }
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return TRUE;
            }
            if let Some((_, _, w, h)) = window_bounds(hwnd) {
                if w >= 240 && h >= 120 {
                    let area = w as u64 * h as u64;
                    if area > state.best_area {
                        state.best_area = area;
                        state.found = Some(hwnd);
                    }
                }
            }
        }
        TRUE
    }

    let mut state = State {
        found: None,
        best_area: 0,
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut state as *mut State as *mut c_void as isize),
        );
    }
    state.found
}

#[cfg(target_os = "windows")]
fn capture_window(hwnd: windows::Win32::Foundation::HWND) -> Result<Captured, String> {
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetWindowDC,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::UI::WindowsAndMessaging::PW_RENDERFULLCONTENT;

    #[link(name = "user32")]
    extern "system" {
        fn PrintWindow(hwnd: HWND, hdc: windows::Win32::Graphics::Gdi::HDC, flags: u32) -> i32;
    }

    let Some((x, y, w, h)) = window_bounds(hwnd) else {
        return Err("无法读取窗口大小".into());
    };
    if w < 4 || h < 4 {
        return Err("窗口尺寸过小".into());
    }
    let title = window_title(hwnd);
    let mut rendered = None;

    unsafe {
        let screen = GetDC(None);
        let window_dc = GetWindowDC(Some(hwnd));
        let mem = CreateCompatibleDC(Some(screen));
        if !mem.is_invalid() {
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            if let Ok(bitmap) =
                CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                let old = SelectObject(mem, HGDIOBJ(bitmap.0));
                let ok = PrintWindow(hwnd, mem, PW_RENDERFULLCONTENT);
                if ok != 0 && !bits.is_null() {
                    let len = (w as usize) * (h as usize) * 4;
                    let raw = std::slice::from_raw_parts(bits as *const u8, len);
                    let mut rgba = Vec::with_capacity(len);
                    for px in raw.chunks_exact(4) {
                        rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    }
                    if let Some(img) = image::RgbaImage::from_raw(w, h, rgba) {
                        if !is_blankish(&img) {
                            rendered = Some(img);
                        }
                    }
                }
                SelectObject(mem, old);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
            let _ = DeleteDC(mem);
        }
        if !window_dc.is_invalid() {
            ReleaseDC(Some(hwnd), window_dc);
        }
        ReleaseDC(None, screen);
    }

    // 部分 Chromium/WebView2 窗口对 PrintWindow 返回黑图，退回屏幕区域抓取。
    let image = rendered
        .map(Ok)
        .unwrap_or_else(|| capture_rect(x, y, w, h))?;
    Ok(Captured {
        png: encode_png(&image)?,
        width: image.width(),
        height: image.height(),
        title: if title.is_empty() {
            "窗口截图".into()
        } else {
            title
        },
    })
}

#[cfg(target_os = "windows")]
fn capture_active_window() -> Result<Captured, String> {
    let hwnd = active_window().ok_or_else(|| "没有找到可截图的窗口".to_string())?;
    capture_window(hwnd)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn capture_active_window() -> Result<Captured, String> {
    Err("窗口截图目前仅支持 Windows / macOS".into())
}

// ============================================================
// 窗口列表（覆盖层里的"悬停高亮 / 单击选窗"，飞书式）
// ============================================================

/// macOS：用 CoreGraphics 列出屏幕上的普通窗口（layer 0）。返回顺序是
/// 从最前面到最后面，前端命中测试取第一个包含鼠标的窗口即可。
#[cfg(target_os = "macos")]
mod mac_windows {
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFStringRef = *const c_void;

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

    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const CF_NUMBER_SINT32: i32 = 3;
    const CF_NUMBER_DOUBLE: i32 = 13;
    const ON_SCREEN_ONLY: u32 = 1;
    const EXCLUDE_DESKTOP_ELEMENTS: u32 = 16;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        static kCGWindowBounds: CFStringRef;
        static kCGWindowName: CFStringRef;
        static kCGWindowOwnerName: CFStringRef;
        static kCGWindowLayer: CFStringRef;
        static kCGWindowOwnerPID: CFStringRef;
        static kCGWindowAlpha: CFStringRef;
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(array: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, index: isize) -> *const c_void;
        fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
        fn CFNumberGetValue(number: CFTypeRef, the_type: i32, value: *mut c_void) -> bool;
        fn CFStringGetCString(s: CFStringRef, buffer: *mut i8, size: isize, encoding: u32) -> bool;
        fn CFRelease(cf: CFTypeRef);
    }

    pub struct MacWindow {
        /// 全局逻辑点（主屏左上为原点，和 screencapture 一致）
        pub x: f64,
        pub y: f64,
        pub w: f64,
        pub h: f64,
        pub layer: i32,
        pub pid: i32,
        pub alpha: f64,
        pub app: String,
        pub title: String,
    }

    unsafe fn number(dict: CFDictionaryRef, key: CFStringRef) -> Option<i32> {
        let value = CFDictionaryGetValue(dict, key);
        if value.is_null() {
            return None;
        }
        let mut out: i32 = 0;
        if CFNumberGetValue(value, CF_NUMBER_SINT32, &mut out as *mut i32 as *mut c_void) {
            Some(out)
        } else {
            None
        }
    }

    unsafe fn double(dict: CFDictionaryRef, key: CFStringRef) -> Option<f64> {
        let value = CFDictionaryGetValue(dict, key);
        if value.is_null() {
            return None;
        }
        let mut out: f64 = 0.0;
        if CFNumberGetValue(value, CF_NUMBER_DOUBLE, &mut out as *mut f64 as *mut c_void) {
            Some(out)
        } else {
            None
        }
    }

    unsafe fn string(dict: CFDictionaryRef, key: CFStringRef) -> String {
        let value = CFDictionaryGetValue(dict, key);
        if value.is_null() {
            return String::new();
        }
        let mut buf = [0i8; 512];
        if !CFStringGetCString(value, buf.as_mut_ptr(), buf.len() as isize, CF_STRING_ENCODING_UTF8)
        {
            return String::new();
        }
        let bytes: Vec<u8> = buf
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).to_string()
    }

    pub fn list() -> Vec<MacWindow> {
        let mut out = Vec::new();
        unsafe {
            let array = CGWindowListCopyWindowInfo(
                ON_SCREEN_ONLY | EXCLUDE_DESKTOP_ELEMENTS,
                0, // kCGNullWindowID
            );
            if array.is_null() {
                return out;
            }
            let count = CFArrayGetCount(array);
            for index in 0..count.max(0) {
                let item = CFArrayGetValueAtIndex(array, index);
                if item.is_null() {
                    continue;
                }
                let dict = item as CFDictionaryRef;
                let mut rect = CGRect::default();
                let bounds = CFDictionaryGetValue(dict, kCGWindowBounds);
                if bounds.is_null()
                    || !CGRectMakeWithDictionaryRepresentation(bounds as CFDictionaryRef, &mut rect)
                {
                    continue;
                }
                out.push(MacWindow {
                    x: rect.origin.x,
                    y: rect.origin.y,
                    w: rect.size.width,
                    h: rect.size.height,
                    layer: number(dict, kCGWindowLayer).unwrap_or(0),
                    pid: number(dict, kCGWindowOwnerPID).unwrap_or(0),
                    alpha: double(dict, kCGWindowAlpha).unwrap_or(1.0),
                    app: string(dict, kCGWindowOwnerName),
                    title: string(dict, kCGWindowName),
                });
            }
            CFRelease(array as CFTypeRef);
        }
        out
    }
}

/// 把系统窗口坐标换算到抓屏图上，并裁掉屏幕外的部分。
#[cfg(target_os = "macos")]
fn screen_windows(monitor: &ShotMonitor, image_w: u32, image_h: u32) -> Vec<ShotWindow> {
    let scale = monitor.scale_factor.max(1.0);
    let mon_x = monitor.x as f64 / scale;
    let mon_y = monitor.y as f64 / scale;
    let own_pid = std::process::id() as i32;
    let mut out = Vec::new();
    for win in mac_windows::list() {
        // layer 0 = 普通应用窗口；再排掉自己（岛/覆盖层）和全透明窗口。
        if win.layer != 0 || win.alpha <= 0.01 || win.pid == own_pid {
            continue;
        }
        if win.w < 24.0 || win.h < 24.0 {
            continue;
        }
        let x = (win.x - mon_x) * scale;
        let y = (win.y - mon_y) * scale;
        let left = x.max(0.0);
        let top = y.max(0.0);
        let right = (x + win.w * scale).min(image_w as f64);
        let bottom = (y + win.h * scale).min(image_h as f64);
        if right - left < 16.0 || bottom - top < 16.0 {
            continue;
        }
        out.push(ShotWindow {
            x: left.round(),
            y: top.round(),
            width: (right - left).round(),
            height: (bottom - top).round(),
            app: win.app,
            title: win.title,
        });
    }
    out
}

/// 非 macOS 暂不枚举窗口（Windows 的"窗口"入口还是抓当前活动窗口）。
#[cfg(not(target_os = "macos"))]
fn screen_windows(_monitor: &ShotMonitor, _image_w: u32, _image_h: u32) -> Vec<ShotWindow> {
    Vec::new()
}

/// 覆盖层初始化后拉一次窗口列表，用于悬停高亮/单击选窗。
#[tauri::command]
pub fn shot_windows(id: u64) -> Vec<ShotWindow> {
    // 只取几何信息，别把几 MB 的 PNG 一起 clone 出来。
    let info = session_slot().lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .filter(|s| s.id == id)
            .map(|s| (s.monitor.clone(), s.width, s.height))
    });
    match info {
        Some((monitor, width, height)) => screen_windows(&monitor, width, height),
        None => Vec::new(),
    }
}

// ============================================================
// 窗口管理：隐藏自身窗口 / 创建编辑器
// ============================================================

fn hide_own_windows(app: &AppHandle) -> Vec<String> {
    let mut hidden = Vec::new();
    for (label, win) in app.webview_windows() {
        if label == SHOT_WINDOW_LABEL {
            continue;
        }
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
            hidden.push(label);
        }
    }
    hidden.sort();
    hidden
}

fn restore_own_windows(app: &AppHandle) {
    let labels = hidden_slot()
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default();
    let island_was_hidden = labels
        .iter()
        .any(|label| label == super::ISLAND_WINDOW_LABEL);
    for label in &labels {
        if label == super::ISLAND_WINDOW_LABEL {
            // 截图期间收不到 mouseleave，恢复时先把窗口缩回悬浮条再显示，
            // 否则会原样贴回展开态，用户鼠标不在岛上也收不起来。
            // 注意 ensure_window 只复用不显示，这里必须显式 show 回来。
            let _ = super::set_expanded_on_main(app, false);
            if let Some(win) = app.get_webview_window(label) {
                let _ = win.show();
            }
            continue;
        }
        if let Some(win) = app.get_webview_window(label) {
            let _ = win.show();
        }
    }
    if island_was_hidden {
        // 同步前端状态（open/键盘态），避免 React 仍以为处于展开态。
        let _ = app.emit(super::ISLAND_COLLAPSE_EVENT, ());
    }
}

fn destroy_overlay(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(SHOT_WINDOW_LABEL) {
        let _ = win.destroy();
    }
}

/// 抓屏/录屏前把编辑器窗口移出所有显示器。WebView2 的 `hide()` 在某些
/// 情况下仍会被 `WindowFromPoint` 命中，挪走是更可靠的兜底。
async fn park_overlay(app: &AppHandle) -> Result<(), String> {
    let app_main = app.clone();
    on_main_thread(app, move || {
        if let Some(win) = app_main.get_webview_window(SHOT_WINDOW_LABEL) {
            let _ = win.hide();
            #[cfg(target_os = "windows")]
            if let Ok(hwnd) = win.hwnd() {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
                let hwnd = HWND(hwnd.0 as *mut _);
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            let _ = win.set_position(PhysicalPosition::new(-10_000, -10_000));
        }
        Ok(())
    })
    .await
}

async fn restore_overlay(app: &AppHandle, monitor_x: i32, monitor_y: i32) -> Result<(), String> {
    let app_main = app.clone();
    on_main_thread(app, move || {
        if let Some(win) = app_main.get_webview_window(SHOT_WINDOW_LABEL) {
            let _ = win.set_position(PhysicalPosition::new(monitor_x, monitor_y));
            let _ = win.show();
            let _ = win.set_focus();
        }
        Ok(())
    })
    .await
}

/// 预先建好隐藏的覆盖窗口：截图页一打开就调用，首次点击截图时 WebView 已经是热的。
#[tauri::command]
pub async fn shot_prewarm(app: AppHandle) -> Result<(), String> {
    let app_inner = app.clone();
    on_main_thread(&app, move || {
        if app_inner.get_webview_window(SHOT_WINDOW_LABEL).is_some() {
            return Ok(()); // 已经有窗口（复用中），无需预热
        }
        let monitor = monitor_list(&app_inner)
            .into_iter()
            .find(|m| m.primary)
            .or_else(|| monitor_list(&app_inner).into_iter().next())
            .ok_or_else(|| "没有检测到显示器".to_string())?;
        let win = WebviewWindowBuilder::new(
            &app_inner,
            SHOT_WINDOW_LABEL,
            WebviewUrl::App("shot.html?warm=1".into()),
        )
        .title("AgentHUD Shot")
        .inner_size(2.0, 2.0)
        .decorations(false)
        .transparent(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .visible(false)
        .focused(false)
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .build()
        .map_err(|e| format!("预热截图窗口失败: {e}"))?;
        let _ = win.set_background_color(Some(tauri::window::Color(9, 11, 17, 255)));
        let _ = win.set_position(PhysicalPosition::new(monitor.x, monitor.y));
        let _ = win.set_size(PhysicalSize::new(monitor.width, monitor.height));
        Ok(())
    })
    .await
}

/// 覆盖窗口在多次截图之间复用：WebView 进程是热的，比每次重建快 ~100ms。
/// 复用走 `navigate` 到带新 id 的 `shot.html`，前端照旧按 id 拉会话。
fn reuse_overlay(app: &AppHandle, monitor: &ShotMonitor, id: u64) -> Result<(), String> {
    let Some(win) = app.get_webview_window(SHOT_WINDOW_LABEL) else {
        build_overlay(app, monitor, id)?;
        return Ok(());
    };
    let _ = win.hide();
    win.set_position(PhysicalPosition::new(monitor.x, monitor.y))
        .map_err(|e| format!("定位截图窗口失败: {e}"))?;
    win.set_size(PhysicalSize::new(monitor.width, monitor.height))
        .map_err(|e| format!("调整截图窗口尺寸失败: {e}"))?;
    let mut url = win
        .url()
        .map_err(|e| format!("读取截图窗口地址失败: {e}"))?;
    url.set_query(Some(&format!("id={id}")));
    win.navigate(url)
        .map_err(|e| format!("刷新截图窗口失败: {e}"))?;
    let _ = win.show();
    let _ = win.set_focus();
    let _ = win.set_position(PhysicalPosition::new(monitor.x, monitor.y));
    Ok(())
}

/// 抓屏前把覆盖窗口收起来（复用模式下不能 destroy，否则又变成冷启动）。
fn hide_overlay(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(SHOT_WINDOW_LABEL) {
        let _ = win.hide();
    }
}

fn build_overlay(app: &AppHandle, monitor: &ShotMonitor, id: u64) -> Result<WebviewWindow, String> {
    if let Some(old) = app.get_webview_window(SHOT_WINDOW_LABEL) {
        let _ = old.destroy();
    }
    let builder = WebviewWindowBuilder::new(
        app,
        SHOT_WINDOW_LABEL,
        WebviewUrl::App(format!("shot.html?id={id}").into()),
    )
    .title("AgentHUD Shot")
    .inner_size(2.0, 2.0)
    .decorations(false)
    .transparent(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .closable(true)
    .visible(false)
    .focusable(true)
    .focused(false)
    .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled);

    let win = builder
        .build()
        .map_err(|e| format!("创建截图窗口失败: {e}"))?;
    let _ = win.set_background_color(Some(tauri::window::Color(9, 11, 17, 255)));
    win.set_position(PhysicalPosition::new(monitor.x, monitor.y))
        .map_err(|e| format!("定位截图窗口失败: {e}"))?;
    win.set_size(PhysicalSize::new(monitor.width, monitor.height))
        .map_err(|e| format!("调整截图窗口尺寸失败: {e}"))?;
    let _ = win.set_always_on_top(true);
    // macOS 会把高于可视区（菜单栏以下）的窗口往下顶 30pt，显示后再钉一次位置，
    // 让冻结帧和真实屏幕 1:1 对齐。
    let _ = win.set_position(PhysicalPosition::new(monitor.x, monitor.y));
    // 抓屏此时已经结束，先显示出去让用户马上看到“正在加载”的暗色遮罩，
    // 比等图片解码完再显示（shot_ready）快一截。
    let _ = win.show();
    let _ = win.set_focus();
    Ok(win)
}

// ============================================================
// 启动抓屏流程
// ============================================================

pub async fn start_capture(
    app: AppHandle,
    mode: String,
    delay_sec: u64,
    monitor_index: Option<usize>,
) -> Result<u64, String> {
    if STARTING.swap(true, Ordering::SeqCst) {
        return Err("截图正在进行中".into());
    }
    let app_restore = app.clone();
    let result = start_capture_inner(app, mode, delay_sec, monitor_index).await;
    if result.is_err() {
        // 任何一步失败（抓屏/建窗）都要把刚才隐藏的岛与贴图放回来。
        let app_inner = app_restore.clone();
        let _ = on_main_thread(&app_restore, move || {
            destroy_overlay(&app_inner);
            if let Ok(mut guard) = session_slot().lock() {
                *guard = None;
            }
            restore_own_windows(&app_inner);
            Ok(())
        })
        .await;
    }
    STARTING.store(false, Ordering::SeqCst);
    result
}

async fn start_capture_inner(
    app: AppHandle,
    mode: String,
    delay_sec: u64,
    monitor_index: Option<usize>,
) -> Result<u64, String> {
    if session_slot()
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false)
    {
        return Err("截图窗口已经打开".into());
    }
    let mode = match mode.as_str() {
        "region" => "region",
        "full" | "monitor" => "full",
        "window" => "window",
        other => return Err(format!("未知截图模式: {other}")),
    };

    let started = std::time::Instant::now();
    let app_main = app.clone();
    let hidden = on_main_thread(&app, move || {
        hide_overlay(&app_main);
        let hidden = hide_own_windows(&app_main);
        if let Ok(mut guard) = hidden_slot().lock() {
            *guard = hidden.clone();
        }
        Ok(hidden)
    })
    .await?;
    let _ = hidden;
    let wait_ms = if delay_sec > 0 {
        delay_sec.min(30) * 1000
    } else if cfg!(target_os = "windows") {
        // 给 DWM 一拍把岛/贴图真正移出合成画面。
        140
    } else {
        // macOS 上窗口隐藏是同步提交的，只留一帧给合成器。
        32
    };
    tauri::async_runtime::spawn_blocking(move || std::thread::sleep(Duration::from_millis(wait_ms)))
        .await
        .map_err(|e| format!("等待截图延时失败: {e}"))?;

    let app_cap = app.clone();
    let (captured, monitor, origin, final_mode, title) =
        tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
            let monitors = monitor_list(&app_cap);
            if monitors.is_empty() {
                return Err("没有检测到显示器".into());
            }
            // Windows 没有覆盖层选窗，直接抓当前活动窗口进标注器；
            // macOS 则统一抓整屏，由覆盖层里的"窗口"模式选窗。
            #[cfg(not(target_os = "macos"))]
            if mode == "window" {
                let captured = capture_active_window()?;
                // 窗口中心所在显示器
                let cursor = cursor_position();
                let mon = cursor
                    .and_then(|(x, y)| monitor_for_point(&app_cap, x, y))
                    .or_else(|| monitors.iter().find(|m| m.primary).cloned())
                    .ok_or_else(|| "无法确定窗口所在显示器".to_string())?;
                return Ok((
                    captured,
                    mon,
                    None,
                    "edit".to_string(),
                    "窗口截图".to_string(),
                ));
            }
            let mon = match monitor_index {
                Some(index) => monitors.get(index).cloned(),
                None => cursor_position()
                    .and_then(|(x, y)| monitor_for_point(&app_cap, x, y))
                    .or_else(|| monitors.iter().find(|m| m.primary).cloned()),
            }
            .ok_or_else(|| "无法确定截图显示器".to_string())?;
            #[cfg(target_os = "macos")]
            set_mac_scale(mon.scale_factor);
            let captured = capture_monitor_captured(&mon)?;
            let origin = ShotRect {
                x: 0.0,
                y: 0.0,
                width: mon.width as f64,
                height: mon.height as f64,
            };
            let final_mode = match mode {
                "region" => "region",
                "window" => "window",
                _ => "edit",
            };
            let title = match final_mode {
                "region" => "区域截图",
                "window" => "窗口截图",
                _ => "全屏截图",
            };
            Ok((captured, mon, Some(origin), final_mode.to_string(), title.to_string()))
        })
        .await
        .map_err(|e| format!("截图任务失败: {e}"))??;

    eprintln!(
        "[zbar-shot] 抓屏+编码完成 {}×{} @({}, {})，耗时 {}ms",
        captured.width,
        captured.height,
        monitor.x,
        monitor.y,
        started.elapsed().as_millis()
    );

    let id = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    let session = ShotSession {
        id,
        mode: final_mode,
        image_png: captured.png.clone(),
        width: captured.width,
        height: captured.height,
        origin,
        monitor: monitor.clone(),
        title: if captured.title.is_empty() {
            title
        } else {
            captured.title.clone()
        },
        created_at: now_ms(),
    };
    if let Ok(mut guard) = session_slot().lock() {
        *guard = Some(session);
    }

    let app_build = app.clone();
    let mon_build = monitor.clone();
    let id_build = id;
    if let Err(e) = on_main_thread(&app, move || {
        reuse_overlay(&app_build, &mon_build, id_build)?;
        Ok(())
    })
    .await
    {
        if let Ok(mut guard) = session_slot().lock() {
            *guard = None;
        }
        // 错误路径由 start_capture 的外层统一在主线程恢复窗口。
        return Err(e);
    }

    // 前端如果因为 WebView 加载失败/图片过大而没有调用 shot_ready，窗口会
    // 永远隐藏、HUD 也一直不回来。2.5s 后兜底显示，至少让用户能取消。
    let app_watch = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2500));
        let alive = session_slot()
            .lock()
            .map(|g| g.as_ref().map(|s| s.id == id).unwrap_or(false))
            .unwrap_or(false);
        if !alive {
            return;
        }
        let app_inner = app_watch.clone();
        let _ = on_main_thread_blocking(&app_watch, move || {
            if let Some(win) = app_inner.get_webview_window(SHOT_WINDOW_LABEL) {
                if !win.is_visible().unwrap_or(false) {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            Ok(())
        });
    });

    eprintln!(
        "[zbar-shot] 截图窗口就绪，端到端 {}ms",
        started.elapsed().as_millis()
    );
    Ok(id)
}

// ============================================================
// 命令：启动/会话/关闭
// ============================================================

#[tauri::command]
pub fn shot_monitors(app: AppHandle) -> Vec<ShotMonitor> {
    monitor_list(&app)
}

#[tauri::command]
pub fn shot_settings() -> ShotSettings {
    super::store::load_shot_settings()
}

#[tauri::command]
pub fn shot_set_settings(settings: ShotSettings) -> Result<ShotSettings, String> {
    let next = settings.clamped();
    super::store::save_shot_settings(&next)?;
    Ok(next)
}

#[tauri::command]
pub async fn shot_start(
    app: AppHandle,
    mode: String,
    delay_sec: Option<u64>,
    monitor: Option<usize>,
) -> Result<u64, String> {
    let delay = delay_sec.unwrap_or_else(|| shot_settings().delay_sec);
    start_capture(app, mode, delay, monitor).await
}

/// 从历史截图重新进入标注器（Snow Shot 的“编辑已有图片”）。
#[tauri::command]
pub async fn shot_edit_file(app: AppHandle, path: String) -> Result<u64, String> {
    if STARTING.swap(true, Ordering::SeqCst) {
        return Err("截图正在进行中".into());
    }
    let result = async {
        let p = PathBuf::from(&path);
        if !is_inside_shot_dir(&p)? {
            return Err("只能编辑截图目录中的文件".into());
        }
        if session_slot()
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
        {
            return Err("截图窗口已经打开".into());
        }
        let image = tauri::async_runtime::spawn_blocking(move || {
            image::open(&p)
                .map(|img| img.to_rgba8())
                .map_err(|e| format!("读取截图失败: {e}"))
        })
        .await
        .map_err(|e| format!("读取截图任务失败: {e}"))??;
        let png = encode_png(&image)?;
        let monitors = monitor_list(&app);
        let monitor = cursor_position()
            .and_then(|(x, y)| monitor_for_point(&app, x, y))
            .or_else(|| monitors.into_iter().find(|m| m.primary))
            .ok_or_else(|| "无法确定显示器".to_string())?;
        let app_main = app.clone();
        on_main_thread(&app, move || {
            destroy_overlay(&app_main);
            let hidden = hide_own_windows(&app_main);
            if let Ok(mut guard) = hidden_slot().lock() {
                *guard = hidden;
            }
            Ok(())
        })
        .await?;
        let id = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
        let title = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "编辑截图".into());
        let session = ShotSession {
            id,
            mode: "edit".into(),
            image_png: png,
            width: image.width(),
            height: image.height(),
            origin: None,
            monitor: monitor.clone(),
            title,
            created_at: now_ms(),
        };
        if let Ok(mut guard) = session_slot().lock() {
            *guard = Some(session);
        }
        let app_build = app.clone();
        let mon_build = monitor.clone();
        on_main_thread(&app, move || {
            build_overlay(&app_build, &mon_build, id)?;
            Ok(())
        })
        .await?;
        let app_watch = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2500));
            let alive = session_slot()
                .lock()
                .map(|g| g.as_ref().map(|s| s.id == id).unwrap_or(false))
                .unwrap_or(false);
            if !alive {
                return;
            }
            let app_inner = app_watch.clone();
            let _ = on_main_thread_blocking(&app_watch, move || {
                if let Some(win) = app_inner.get_webview_window(SHOT_WINDOW_LABEL) {
                    if !win.is_visible().unwrap_or(false) {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
                Ok(())
            });
        });
        Ok(id)
    }
    .await;
    if result.is_err() {
        let app_restore = app.clone();
        let _ = on_main_thread(&app, move || {
            destroy_overlay(&app_restore);
            if let Ok(mut guard) = session_slot().lock() {
                *guard = None;
            }
            restore_own_windows(&app_restore);
            Ok(())
        })
        .await;
    }
    STARTING.store(false, Ordering::SeqCst);
    result
}

#[tauri::command]
pub fn shot_session(id: u64) -> Result<ShotSession, String> {
    session_slot()
        .lock()
        .map_err(|_| "截图会话不可用".to_string())?
        .as_ref()
        .filter(|s| s.id == id)
        .cloned()
        .ok_or_else(|| "截图会话已失效".into())
}

/// 原始 PNG 字节：经 Tauri 的 raw IPC 通道下发，避免大图 base64 撑爆
/// 会话 JSON。前端拿 ArrayBuffer 后创建 blob URL 供 img/canvas 使用。
#[tauri::command]
pub fn shot_session_image(id: u64) -> Result<tauri::ipc::Response, String> {
    let bytes = session_slot()
        .lock()
        .map_err(|_| "截图会话不可用".to_string())?
        .as_ref()
        .filter(|s| s.id == id)
        .map(|s| s.image_png.clone())
        .ok_or_else(|| "截图会话已失效".to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn shot_ready(app: AppHandle, id: u64) -> Result<(), String> {
    let valid = session_slot()
        .lock()
        .map(|g| g.as_ref().map(|s| s.id == id).unwrap_or(false))
        .unwrap_or(false);
    if !valid {
        return Err("截图会话已失效".into());
    }
    if let Some(created) = session_slot()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.created_at))
    {
        eprintln!(
            "[zbar-shot] 前端就绪（图片解码+首帧），距抓屏开始 {}ms",
            (now_ms() - created).max(0)
        );
    }
    let app_main = app.clone();
    on_main_thread(&app, move || {
        if let Some(win) = app_main.get_webview_window(SHOT_WINDOW_LABEL) {
            win.show().map_err(|e| format!("显示截图窗口失败: {e}"))?;
            let _ = win.set_focus();
        }
        Ok(())
    })
    .await
}

/// 前端把冻结帧画到屏幕后回调一次，用来量化“点击到真的能框选”的耗时。
#[tauri::command]
pub fn shot_mark_painted(id: u64) {
    if let Some(created) = session_slot()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().filter(|s| s.id == id).map(|s| s.created_at))
    {
        eprintln!(
            "[zbar-shot] 首帧已绘制，距抓屏开始 {}ms",
            (now_ms() - created).max(0)
        );
    }
}

#[tauri::command]
pub async fn shot_close(app: AppHandle) -> Result<(), String> {
    close_shot(&app).await
}

pub async fn close_shot(app: &AppHandle) -> Result<(), String> {
    let app_main = app.clone();
    on_main_thread(app, move || {
        // 只隐藏不销毁：下次截图复用同一个 WebView，省掉冷启动。
        hide_overlay(&app_main);
        if let Ok(mut guard) = session_slot().lock() {
            *guard = None;
        }
        restore_own_windows(&app_main);
        Ok(())
    })
    .await
}

/// 窗口被 Alt+F4 / 系统关闭时调用：清理会话并恢复 HUD。
pub fn on_overlay_close_requested(app: &AppHandle) {
    // 该函数由 lib.rs 的 WindowEvent::CloseRequested 在主线程直接调用；
    // 不能再用 run_on_main_thread + 阻塞等待，否则会自己等自己。
    destroy_overlay(app);
    if let Ok(mut guard) = session_slot().lock() {
        *guard = None;
    }
    restore_own_windows(app);
}

// ============================================================
// 保存 / 剪贴板 / 历史记录
// ============================================================

fn shot_dir() -> Result<PathBuf, String> {
    let base = dirs::picture_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Pictures")))
        .ok_or_else(|| "无法定位图片目录".to_string())?;
    let dir = base.join("AgentHUD Screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建截图目录失败: {e}"))?;
    Ok(dir)
}

fn is_inside_shot_dir(path: &Path) -> Result<bool, String> {
    let dir = shot_dir()?;
    let dir = dir.canonicalize().unwrap_or(dir);
    let path = path.canonicalize().map_err(|e| format!("截图不存在: {e}"))?;
    Ok(path.starts_with(&dir))
}

fn timestamp_name(format: &str) -> String {
    let now = chrono::Local::now();
    let ext = if format == "jpg" { "jpg" } else { "png" };
    format!("Shot_{}.{}", now.format("%Y%m%d_%H%M%S_%3f"), ext)
}

fn write_image_file(image: &image::RgbaImage, format: &str) -> Result<PathBuf, String> {
    let dir = shot_dir()?;
    let mut path = dir.join(timestamp_name(format));
    let mut attempt = 1;
    while path.exists() {
        let ext = if format == "jpg" { "jpg" } else { "png" };
        path = dir.join(format!(
            "Shot_{}_{}.{}",
            chrono::Local::now().format("%Y%m%d_%H%M%S_%3f"),
            attempt,
            ext
        ));
        attempt += 1;
    }
    if format == "jpg" {
        let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
        let file = std::fs::File::create(&path).map_err(|e| format!("创建截图文件失败: {e}"))?;
        let mut writer = std::io::BufWriter::new(file);
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 92)
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("编码 JPG 失败: {e}"))?;
    } else {
        image
            .save_with_format(&path, image::ImageFormat::Png)
            .map_err(|e| format!("保存 PNG 失败: {e}"))?;
    }
    Ok(path)
}

/// 把 RGBA 图编码成目标格式的字节（写文件/另存为共用）。
fn encode_image_bytes(image: &image::RgbaImage, format: &str) -> Result<Vec<u8>, String> {
    if format == "jpg" {
        let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
        let mut buf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92)
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("编码 JPG 失败: {e}"))?;
        Ok(buf)
    } else {
        encode_png(image)
    }
}

fn history_item(path: &Path) -> Result<ShotHistoryItem, String> {
    let image = image::open(path).map_err(|e| format!("读取截图失败: {e}"))?;
    let rgba = image.to_rgba8();
    let thumb = image::imageops::thumbnail(&rgba, THUMB_WIDTH, THUMB_WIDTH);
    let thumb_url = png_data_url(&thumb)?;
    let meta = std::fs::metadata(path).map_err(|e| format!("读取截图信息失败: {e}"))?;
    let created = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or_else(now_ms);
    Ok(ShotHistoryItem {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "截图".into()),
        path: path.to_string_lossy().to_string(),
        width: rgba.width(),
        height: rgba.height(),
        created_at: created,
        thumb: thumb_url,
        size: meta.len(),
    })
}

fn list_history(limit: usize) -> Vec<ShotHistoryItem> {
    let Ok(dir) = shot_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<(i64, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && matches!(
                    p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()),
                    Some(ext) if ext == "png" || ext == "jpg" || ext == "jpeg"
                )
        })
        .map(|p| {
            let ts = std::fs::metadata(&p)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            (ts, p)
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .into_iter()
        .take(limit.clamp(1, MAX_HISTORY))
        .filter_map(|(_, path)| history_item(&path).ok())
        .collect()
}

fn write_clipboard(image: &image::RgbaImage) -> Result<(), String> {
    let data = arboard::ImageData {
        width: image.width() as usize,
        height: image.height() as usize,
        bytes: std::borrow::Cow::Owned(image.clone().into_raw()),
    };
    let mut last_err = String::new();
    for attempt in 0..3 {
        match arboard::Clipboard::new() {
            Ok(mut clip) => match clip.set_image(data.clone()) {
                Ok(()) => return Ok(()),
                Err(e) => last_err = format!("写入剪贴板失败: {e}"),
            },
            Err(e) => last_err = format!("剪贴板不可用: {e}"),
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(60));
        }
    }
    Err(last_err)
}

#[tauri::command]
pub async fn shot_save(
    app: AppHandle,
    data_url: String,
    format: Option<String>,
) -> Result<ShotHistoryItem, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = decode_data_url(&data_url)?;
        let fmt = format.unwrap_or_else(|| shot_settings().format);
        let path = write_image_file(&image, &fmt)?;
        let item = history_item(&path)?;
        let _ = app.emit(SHOT_SAVED_EVENT, item.clone());
        Ok(item)
    })
    .await
    .map_err(|e| format!("保存截图任务失败: {e}"))?
}

/// 另存为：弹系统保存对话框，用户选路径后写盘。返回 `Ok(None)` 表示取消。
/// 与「保存」不同，它不改截图目录里的历史，适合把挑好的图放到项目目录。
#[tauri::command]
pub async fn shot_save_as(
    app: AppHandle,
    data_url: String,
    format: Option<String>,
) -> Result<Option<ShotHistoryItem>, String> {
    let fmt = match format.as_deref() {
        Some("jpg") | Some("jpeg") => "jpg".to_string(),
        Some("png") | None => "png".to_string(),
        Some(other) => other.to_string(),
    };
    tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;

        let image = decode_data_url(&data_url)?;
        let ext = if fmt == "jpg" { "jpg" } else { "png" };
        let filter = if fmt == "jpg" { "JPEG 图片" } else { "PNG 图片" };
        let picked = app
            .dialog()
            .file()
            .set_title("另存为")
            .add_filter(filter, &[ext])
            .set_file_name(timestamp_name(&fmt))
            .blocking_save_file();
        let Some(picked) = picked else {
            return Ok(None); // 用户取消
        };
        let path = picked
            .into_path()
            .map_err(|e| format!("解析保存路径失败: {e}"))?;
        let bytes = encode_image_bytes(&image, &fmt)?;
        std::fs::write(&path, bytes).map_err(|e| format!("写入图片失败: {e}"))?;
        Ok(Some(history_item(&path)?))
    })
    .await
    .map_err(|e| format!("另存为任务失败: {e}"))?
}

#[tauri::command]
pub async fn shot_copy(data_url: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = decode_data_url(&data_url)?;
        write_clipboard(&image)
    })
    .await
    .map_err(|e| format!("复制截图任务失败: {e}"))?
}

#[tauri::command]
pub fn shot_copy_text(text: String) -> Result<(), String> {
    let mut clip = arboard::Clipboard::new().map_err(|e| format!("剪贴板不可用: {e}"))?;
    clip.set_text(text)
        .map_err(|e| format!("复制文字失败: {e}"))
}

#[tauri::command]
pub async fn shot_history(limit: Option<usize>) -> Result<Vec<ShotHistoryItem>, String> {
    tauri::async_runtime::spawn_blocking(move || list_history(limit.unwrap_or(20)))
        .await
        .map_err(|e| format!("读取截图历史失败: {e}"))
}

#[tauri::command]
pub async fn shot_delete(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = PathBuf::from(path);
        if !is_inside_shot_dir(&p)? {
            return Err("只能删除截图目录中的文件".into());
        }
        std::fs::remove_file(&p).map_err(|e| format!("删除截图失败: {e}"))
    })
    .await
    .map_err(|e| format!("删除截图任务失败: {e}"))?
}

#[tauri::command]
pub fn shot_open(path: String) -> Result<(), String> {
    let p = PathBuf::from(path);
    if !is_inside_shot_dir(&p)? {
        return Err("只能打开截图目录中的文件".into());
    }
    open::that(&p).map_err(|e| format!("打开截图失败: {e}"))
}

#[tauri::command]
pub fn shot_open_folder() -> Result<(), String> {
    let dir = shot_dir()?;
    open::that(dir).map_err(|e| format!("打开截图目录失败: {e}"))
}

// ============================================================
// 贴图窗口
// ============================================================

fn pin_id_from_label(label: &str) -> Option<u64> {
    label
        .strip_prefix(PIN_WINDOW_PREFIX)
        .and_then(|s| s.parse::<u64>().ok())
}

fn build_pin_window(app: &AppHandle, id: u64) -> Result<WebviewWindow, String> {
    let (image, width, height) = {
        let pins = pins_slot().lock().map_err(|_| "贴图数据不可用".to_string())?;
        let entry = pins.get(&id).ok_or_else(|| "贴图不存在".to_string())?;
        (entry.image.clone(), entry.width, entry.height)
    };
    let _ = image;
    let cursor = cursor_position();
    let mon = cursor
        .and_then(|(x, y)| monitor_for_point(app, x, y))
        .or_else(|| monitor_list(app).into_iter().find(|m| m.primary))
        .ok_or_else(|| "无法确定贴图位置".to_string())?;
    let scale = mon.scale_factor.max(0.5);
    let logical_w = width as f64 / scale;
    let logical_h = height as f64 / scale;
    let fit = (520.0 / logical_w.max(1.0))
        .min(380.0 / logical_h.max(1.0))
        .min(1.0)
        .max(0.05);
    let w = (logical_w * fit).max(120.0);
    let h = (logical_h * fit).max(80.0);
    let label = format!("{PIN_WINDOW_PREFIX}{id}");
    if let Some(old) = app.get_webview_window(&label) {
        let _ = old.destroy();
    }
    let builder = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::App(format!("pin.html?id={id}").into()),
    )
    .title("AgentHUD 贴图")
    .inner_size(w, h)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .visible(false)
    .focusable(true)
    .focused(false)
    .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled);
    let win = builder
        .build()
        .map_err(|e| format!("创建贴图窗口失败: {e}"))?;
    let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    let x = mon.x + ((mon.width as f64 - w * scale) / 2.0).max(0.0) as i32;
    let y = mon.y + ((mon.height as f64 - h * scale) / 2.0).max(0.0) as i32;
    win.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| format!("定位贴图失败: {e}"))?;
    Ok(win)
}

async fn pin_image(app: AppHandle, image: image::RgbaImage) -> Result<PinData, String> {
    let id = PIN_SEQ.fetch_add(1, Ordering::Relaxed);
    let data = PinData {
        id,
        width: image.width(),
        height: image.height(),
        image: png_data_url(&image)?,
    };
    if let Ok(mut pins) = pins_slot().lock() {
        pins.insert(
            id,
            PinEntry {
                image: data.image.clone(),
                width: data.width,
                height: data.height,
            },
        );
    }
    let app_build = app.clone();
    let result = on_main_thread(&app, move || build_pin_window(&app_build, id)).await;
    if let Err(e) = result {
        if let Ok(mut pins) = pins_slot().lock() {
            pins.remove(&id);
        }
        return Err(e);
    }
    if let Some(win) = app.get_webview_window(&format!("{PIN_WINDOW_PREFIX}{id}")) {
        let _ = win.show();
        let _ = win.set_focus();
    }
    Ok(data)
}

#[tauri::command]
pub async fn shot_pin(app: AppHandle, data_url: String) -> Result<PinData, String> {
    let image = tauri::async_runtime::spawn_blocking(move || decode_data_url(&data_url))
        .await
        .map_err(|e| format!("贴图任务失败: {e}"))??;
    pin_image(app, image).await
}

#[tauri::command]
pub async fn shot_pin_file(app: AppHandle, path: String) -> Result<PinData, String> {
    let p = PathBuf::from(path);
    if !is_inside_shot_dir(&p)? {
        return Err("只能贴出截图目录中的文件".into());
    }
    let image = tauri::async_runtime::spawn_blocking(move || {
        image::open(&p)
            .map(|img| img.to_rgba8())
            .map_err(|e| format!("读取截图失败: {e}"))
    })
    .await
    .map_err(|e| format!("读取截图任务失败: {e}"))??;
    pin_image(app, image).await
}

#[tauri::command]
pub async fn shot_copy_file(path: String) -> Result<(), String> {
    let p = PathBuf::from(path);
    if !is_inside_shot_dir(&p)? {
        return Err("只能复制截图目录中的文件".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let image = image::open(&p)
            .map(|img| img.to_rgba8())
            .map_err(|e| format!("读取截图失败: {e}"))?;
        write_clipboard(&image)
    })
    .await
    .map_err(|e| format!("复制截图任务失败: {e}"))?
}

#[tauri::command]
pub fn shot_pin_data(id: u64) -> Result<PinData, String> {
    let pins = pins_slot().lock().map_err(|_| "贴图数据不可用".to_string())?;
    let entry = pins.get(&id).ok_or_else(|| "贴图不存在".to_string())?;
    Ok(PinData {
        id,
        image: entry.image.clone(),
        width: entry.width,
        height: entry.height,
    })
}

#[tauri::command]
pub async fn shot_pin_close(app: AppHandle, id: u64) -> Result<(), String> {
    if let Ok(mut pins) = pins_slot().lock() {
        pins.remove(&id);
    }
    let app_main = app.clone();
    on_main_thread(&app, move || {
        if let Some(win) = app_main.get_webview_window(&format!("{PIN_WINDOW_PREFIX}{id}")) {
            let _ = win.destroy();
        }
        Ok(())
    })
    .await
}

/// 贴图窗口关闭（Alt+F4 / 自绘关闭按钮）时清理数据。
pub fn on_pin_close_requested(app: &AppHandle, label: &str) {
    if let Some(id) = pin_id_from_label(label) {
        if let Ok(mut pins) = pins_slot().lock() {
            pins.remove(&id);
        }
    }
    let _ = app;
}

// ============================================================
// 本地 OCR / 二维码
// ============================================================

fn ocr_image(image: &image::RgbaImage) -> Result<OcrResult, String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
        use windows::Media::Ocr::OcrEngine;
        use windows::Storage::Streams::DataWriter;
        use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let max_dim = OcrEngine::MaxImageDimension().unwrap_or(2600).max(512);
        let src = if image.width() > max_dim || image.height() > max_dim {
            let ratio = (max_dim as f64 / image.width().max(image.height()) as f64).min(1.0);
            let nw = (image.width() as f64 * ratio).round().max(1.0) as u32;
            let nh = (image.height() as f64 * ratio).round().max(1.0) as u32;
            image::imageops::resize(image, nw, nh, image::imageops::FilterType::Triangle)
        } else {
            image.clone()
        };
        let mut bgra = Vec::with_capacity((src.width() * src.height() * 4) as usize);
        for px in src.pixels() {
            bgra.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
            format!("Windows OCR 不可用（请先在系统设置中安装语言识别组件）: {e}")
        })?;
        let writer = DataWriter::new().map_err(|e| format!("创建 OCR 缓冲区失败: {e}"))?;
        writer
            .WriteBytes(&bgra)
            .map_err(|e| format!("写入 OCR 缓冲区失败: {e}"))?;
        let buffer = writer
            .DetachBuffer()
            .map_err(|e| format!("读取 OCR 缓冲区失败: {e}"))?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            src.width() as i32,
            src.height() as i32,
        )
        .map_err(|e| format!("创建 OCR 位图失败: {e}"))?;
        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| format!("启动 OCR 失败: {e}"))?
            .get()
            .map_err(|e| format!("OCR 识别失败: {e}"))?;
        let text = result
            .Text()
            .map(|t| t.to_string_lossy())
            .unwrap_or_default();
        let mut lines = Vec::new();
        if let Ok(list) = result.Lines() {
            for line in list {
                if let Ok(t) = line.Text() {
                    let s = t.to_string_lossy();
                    if !s.trim().is_empty() {
                        lines.push(s);
                    }
                }
            }
        }
        let clean = text.trim().to_string();
        return Ok(OcrResult {
            ok: !clean.is_empty(),
            text: clean.clone(),
            lines,
            error: if clean.is_empty() {
                "未识别到文字".into()
            } else {
                String::new()
            },
        });
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = image;
        Err("OCR 目前仅支持 Windows".into())
    }
}

#[tauri::command]
pub async fn shot_ocr(data_url: String) -> Result<OcrResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = decode_data_url(&data_url)?;
        ocr_image(&image)
    })
    .await
    .map_err(|e| format!("OCR 任务失败: {e}"))?
}

fn qr_image(image: &image::RgbaImage) -> Result<QrResult, String> {
    let gray = image::DynamicImage::ImageRgba8(image.clone()).to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(gray);
    let grids = prepared.detect_grids();
    let mut texts = Vec::new();
    for grid in grids {
        if let Ok((_meta, content)) = grid.decode() {
            if !content.trim().is_empty() && !texts.contains(&content) {
                texts.push(content);
            }
        }
    }
    Ok(QrResult {
        ok: !texts.is_empty(),
        error: if texts.is_empty() {
            "未识别到二维码".into()
        } else {
            String::new()
        },
        texts,
    })
}

#[tauri::command]
pub async fn shot_qr(data_url: String) -> Result<QrResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = decode_data_url(&data_url)?;
        qr_image(&image)
    })
    .await
    .map_err(|e| format!("二维码识别任务失败: {e}"))?
}

// ============================================================
// 滚动长截图
// ============================================================

fn row_signature(image: &image::RgbaImage, y: u32, samples: u32) -> Vec<u8> {
    let w = image.width().max(1);
    let mut out = Vec::with_capacity(samples as usize);
    for i in 0..samples {
        let x = ((i as u64 * (w as u64 - 1)) / samples.max(1) as u64) as u32;
        let px = image.get_pixel(x, y);
        out.push(((px[0] as u16 + px[1] as u16 + px[2] as u16) / 3) as u8);
    }
    out
}

fn frames_differ(a: &image::RgbaImage, b: &image::RgbaImage) -> bool {
    if a.width() != b.width() || a.height() != b.height() {
        return true;
    }
    let rows = 24u32;
    let cols = 32u32;
    let mut diff = 0u64;
    let mut n = 0u64;
    for ry in 0..rows {
        let y = (ry * (a.height().max(2) - 1)) / (rows - 1).max(1);
        for cx in 0..cols {
            let x = (cx * (a.width().max(2) - 1)) / (cols - 1).max(1);
            let pa = a.get_pixel(x, y);
            let pb = b.get_pixel(x, y);
            diff += (pa[0] as i32 - pb[0] as i32).unsigned_abs() as u64
                + (pa[1] as i32 - pb[1] as i32).unsigned_abs() as u64
                + (pa[2] as i32 - pb[2] as i32).unsigned_abs() as u64;
            n += 3;
        }
    }
    n > 0 && diff as f64 / n as f64 > 1.2
}

fn find_overlap(prev: &image::RgbaImage, next: &image::RgbaImage) -> Option<u32> {
    let h = prev.height().min(next.height());
    let w = prev.width().min(next.width());
    if h < 80 || w < 60 {
        return None;
    }
    let min_overlap = (h as f64 * 0.12).max(40.0) as u32;
    let max_overlap = h.saturating_sub(24);
    if min_overlap >= max_overlap {
        return None;
    }
    let samples = 24u32;
    let mut best = (u32::MAX, f64::MAX);
    // 仅搜索可能的滚动重叠，按 2px 步进降低长图搜索成本。
    let mut k = min_overlap;
    while k <= max_overlap {
        let mut diff = 0.0f64;
        let mut count = 0.0f64;
        for s in 0..samples {
            let y = (s * (k.saturating_sub(1))) / samples.max(1);
            let py = prev.height() - k + y;
            if py >= prev.height() {
                continue;
            }
            let sa = row_signature(prev, py, 32);
            let sb = row_signature(next, y, 32);
            for i in 0..sa.len() {
                diff += (sa[i] as i32 - sb[i] as i32).unsigned_abs() as f64;
                count += 1.0;
            }
        }
        if count > 0.0 {
            let score = diff / count;
            if score < best.1 {
                best = (k, score);
            }
        }
        k += 2;
    }
    if best.1 <= 26.0 {
        Some(best.0)
    } else {
        None
    }
}

fn stitch_frames(frames: &[image::RgbaImage]) -> Result<image::RgbaImage, String> {
    if frames.is_empty() {
        return Err("没有抓到可拼接的画面".into());
    }
    let width = frames[0].width();
    let mut rows: Vec<Vec<u8>> = Vec::with_capacity(frames[0].height() as usize);
    for y in 0..frames[0].height() {
        let mut row = Vec::with_capacity(width as usize * 4);
        for x in 0..width {
            row.extend_from_slice(&frames[0].get_pixel(x, y).0);
        }
        rows.push(row);
    }
    let mut stitched_height = frames[0].height();
    let mut last = frames[0].clone();
    for frame in frames.iter().skip(1) {
        if frame.width() != width {
            continue;
        }
        if !frames_differ(&last, frame) {
            continue;
        }
        let overlap = find_overlap(&last, frame).unwrap_or(0);
        let start = overlap.min(frame.height().saturating_sub(2));
        for y in start..frame.height() {
            let mut row = Vec::with_capacity(width as usize * 4);
            for x in 0..width {
                row.extend_from_slice(&frame.get_pixel(x, y).0);
            }
            rows.push(row);
            stitched_height += 1;
            if stitched_height >= 18_000 {
                break;
            }
        }
        last = frame.clone();
        if stitched_height >= 18_000 {
            break;
        }
    }
    let mut out = Vec::with_capacity(width as usize * stitched_height as usize * 4);
    for row in rows.into_iter().take(stitched_height as usize) {
        out.extend_from_slice(&row);
    }
    image::RgbaImage::from_raw(width, stitched_height, out)
        .ok_or_else(|| "拼接长图失败（尺寸不匹配）".into())
}

#[cfg(target_os = "windows")]
fn scroll_once(screen_x: i32, screen_y: i32, amount: i32) -> Result<(), String> {
    use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{mouse_event, MOUSEEVENTF_WHEEL};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, PostMessageW, SetCursorPos, WindowFromPoint, GA_ROOT,
    };
    const WM_MOUSEWHEEL: u32 = 0x020A;
    let delta = -(amount * 120);
    unsafe {
        let target = WindowFromPoint(POINT {
            x: screen_x,
            y: screen_y,
        });
        if !target.is_invalid() {
            let root = GetAncestor(target, GA_ROOT);
            // 优先把滚轮消息直接投递给鼠标下的子窗口。SetForegroundWindow
            // 在后台进程里可能被 Windows 拒绝，只靠全局滚轮会把内容滚到
            // 别的窗口里。
            let wheel_wparam = ((delta as u32) << 16) as usize;
            let wheel_lparam =
                (((screen_y as u32) << 16) | (screen_x as u32 & 0xffff)) as isize;
            let _ = PostMessageW(
                Some(target),
                WM_MOUSEWHEEL,
                WPARAM(wheel_wparam),
                LPARAM(wheel_lparam),
            );
            if !root.is_invalid() {
                let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(root);
            }
        }
        SetCursorPos(screen_x, screen_y).map_err(|e| format!("移动光标失败: {e}"))?;
        // 再补发一次输入队列滚轮，兼容只处理真实输入事件的 Chromium。
        mouse_event(MOUSEEVENTF_WHEEL, 0, 0, delta, 0);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn scroll_once(_x: i32, _y: i32, _amount: i32) -> Result<(), String> {
    Err("滚动截图目前仅支持 Windows".into())
}

#[tauri::command]
pub async fn shot_scroll(
    app: AppHandle,
    id: u64,
    rect: ShotRect,
) -> Result<ScrollResult, String> {
    let session = shot_session(id)?;
    #[cfg(target_os = "macos")]
    set_mac_scale(session.monitor.scale_factor);
    let x = rect.x.round() as i32;
    let y = rect.y.round() as i32;
    let w = rect.width.round().max(2.0) as u32;
    let h = rect.height.round().max(2.0) as u32;
    let monitor_x = session.monitor.x;
    let monitor_y = session.monitor.y;
    if x < 0 || y < 0 || x as u32 + w > session.width || y as u32 + h > session.height {
        return Err("滚动区域超出屏幕范围".into());
    }
    park_overlay(&app).await?;
    tauri::async_runtime::spawn_blocking(move || std::thread::sleep(Duration::from_millis(650)))
        .await
        .map_err(|e| format!("准备滚动截图失败: {e}"))?;

    let screen_x = session.monitor.x + x + w as i32 / 2;
    let screen_y = session.monitor.y + y + h as i32 / 2;
    let original_cursor = cursor_position();
    let captured = tauri::async_runtime::spawn_blocking(
        move || -> Result<Vec<image::RgbaImage>, String> {
            let mut frames = Vec::new();
            let first = capture_rect(session.monitor.x + x, session.monitor.y + y, w, h)?;
            frames.push(first);
            for _ in 0..24 {
                scroll_once(screen_x, screen_y, 3)?;
                std::thread::sleep(Duration::from_millis(300));
                let frame = capture_rect(session.monitor.x + x, session.monitor.y + y, w, h)?;
                let last = frames.last().unwrap();
                if !frames_differ(last, &frame) {
                    // 有的容器要等动画结束才动，多等一拍再判定。
                    std::thread::sleep(Duration::from_millis(260));
                    let retry = capture_rect(session.monitor.x + x, session.monitor.y + y, w, h)?;
                    if !frames_differ(last, &retry) {
                        break;
                    }
                    frames.push(retry);
                } else {
                    frames.push(frame);
                }
                if frames.len() >= 18 {
                    break;
                }
            }
            Ok(frames)
        },
    )
    .await;

    if let Some((cx, cy)) = original_cursor {
        #[cfg(target_os = "windows")]
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(cx, cy);
        }
    }

    // 无论拼接成功还是失败，都要把编辑器重新显示出来；否则用户会只看到
    // 桌面，既没有截图窗口也没有 HUD。
    let outcome = (|| -> Result<(ScrollResult, u32, u32, Vec<u8>), String> {
        let frames = match captured {
            Ok(Ok(frames)) => frames,
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(format!("滚动截图任务失败: {e}")),
        };
        let stitched = stitch_frames(&frames)?;
        let png = encode_png(&stitched)?;
        let new_width = stitched.width();
        let new_height = stitched.height();
        let frame_count = frames.len() as u32;
        Ok((
            ScrollResult {
                width: new_width,
                height: new_height,
                frames: frame_count,
            },
            new_width,
            new_height,
            png,
        ))
    })();

    if let Ok((_result, new_width, new_height, png)) = &outcome {
        if let Ok(mut guard) = session_slot().lock() {
            if let Some(session) = guard.as_mut().filter(|s| s.id == id) {
                session.mode = "edit".into();
                session.image_png = png.clone();
                session.width = *new_width;
                session.height = *new_height;
                session.origin = None;
                session.title = "滚动长截图".into();
            }
        }
    }

    restore_overlay(&app, monitor_x, monitor_y).await?;
    outcome.map(|(result, _, _, _)| result)
}

// ============================================================
// GIF 录屏
// ============================================================

fn gif_filter(image: &image::RgbaImage, max_width: u32) -> image::RgbaImage {
    if image.width() <= max_width {
        return image.clone();
    }
    let ratio = max_width as f64 / image.width() as f64;
    let h = (image.height() as f64 * ratio).round().max(2.0) as u32;
    image::imageops::resize(image, max_width, h, image::imageops::FilterType::Triangle)
}

/// 抓到的帧 → 固定尺寸的 GIF 帧。帧尺寸必须和编码器完全一致，否则
/// `from_rgba_speed` 会断言 panic；而 macOS 的 `screencapture -R` 会向外
/// 取整（请求 401×301 实际拿到 402×302），所以这里统一缩放。
fn gif_frame(
    image: image::RgbaImage,
    width: u32,
    height: u32,
    delay: u16,
) -> gif::Frame<'static> {
    let fitted = if image.width() == width && image.height() == height {
        image
    } else {
        image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle)
    };
    let mut rgba = fitted.into_raw();
    let mut frame = gif::Frame::from_rgba_speed(width as u16, height as u16, &mut rgba, 10);
    frame.delay = delay;
    frame
}

/// 框选区域后录制固定时长的 GIF。录屏期间编辑器移出屏幕，HUD 保持隐藏；
/// 完成后保存到截图目录并把路径返回给前端，由前端提供“打开/打开文件夹”。
#[tauri::command]
pub async fn shot_record_gif(
    app: AppHandle,
    id: u64,
    rect: ShotRect,
    seconds: u64,
) -> Result<RecordResult, String> {
    let session = shot_session(id)?;
    // 只有"屏幕上的某块区域"才能按屏幕坐标重录；窗口/历史图片会话的
    // rect 是图片内坐标，硬录会拍到不相干的地方。
    if session.origin.is_none() {
        return Err("当前截图不是屏幕区域，无法录制：请用区域或窗口模式重新框选后再录".into());
    }
    #[cfg(target_os = "macos")]
    set_mac_scale(session.monitor.scale_factor);
    let x = rect.x.round() as i32;
    let y = rect.y.round() as i32;
    let w = rect.width.round().max(2.0) as u32;
    let h = rect.height.round().max(2.0) as u32;
    if x < 0 || y < 0 || x as u32 + w > session.width || y as u32 + h > session.height {
        return Err("录制区域超出屏幕范围".into());
    }
    let monitor_x = session.monitor.x;
    let monitor_y = session.monitor.y;
    let screen_x = session.monitor.x + x;
    let screen_y = session.monitor.y + y;
    let seconds = seconds.clamp(3, 20);
    park_overlay(&app).await?;
    tauri::async_runtime::spawn_blocking(move || std::thread::sleep(Duration::from_millis(550)))
        .await
        .map_err(|e| format!("准备录屏失败: {e}"))?;

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<RecordResult, String> {
            let dir = shot_dir()?;
            let path = dir.join(format!(
                "Recording_{}.gif",
                chrono::Local::now().format("%Y%m%d_%H%M%S_%3f")
            ));
            let started = std::time::Instant::now();
            let deadline = Duration::from_secs(seconds);
            let interval = Duration::from_millis(125);
            // 先抓第一帧，用真实像素尺寸建编码器：macOS 的 screencapture
            // 会把逻辑矩形向外取整，实际像素常比请求的多 1px。
            let first = gif_filter(&capture_rect(screen_x, screen_y, w, h)?, 1000);
            let out_w = first.width().max(2);
            let out_h = first.height().max(2);
            let mut gif_buf: Vec<u8> = Vec::new();
            let mut encoder = gif::Encoder::new(&mut gif_buf, out_w as u16, out_h as u16, &[])
                .map_err(|e| format!("初始化 GIF 失败: {e}"))?;
            encoder
                .set_repeat(gif::Repeat::Infinite)
                .map_err(|e| format!("设置 GIF 循环失败: {e}"))?;
            let mut encoded = 0u32;
            let mut last_at = started;
            let mut next_image = Some(first);
            while started.elapsed() < deadline {
                let image = match next_image.take() {
                    Some(image) => image,
                    None => gif_filter(&capture_rect(screen_x, screen_y, w, h)?, 1000),
                };
                let now = std::time::Instant::now();
                // 大区域抓一帧要 1s+，帧延时按真实节奏走；固定 12（120ms）
                // 会把 8 秒录屏回放成 0.6 秒。
                let delay = (((now - last_at).as_millis() / 10) as u32).clamp(4, 200) as u16;
                last_at = now;
                let frame = gif_frame(image, out_w, out_h, delay);
                encoder
                    .write_frame(&frame)
                    .map_err(|e| format!("写入 GIF 帧失败: {e}"))?;
                encoded += 1;
                let elapsed = now - started;
                if elapsed < interval {
                    std::thread::sleep(interval - elapsed);
                }
            }
            drop(encoder);
            if encoded == 0 {
                return Err("没有抓到任何录制帧".into());
            }
            std::fs::write(&path, &gif_buf).map_err(|e| format!("保存 GIF 失败: {e}"))?;
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            Ok(RecordResult {
                path: path.to_string_lossy().to_string(),
                width: out_w,
                height: out_h,
                frames: encoded,
                duration_ms: seconds * 1000,
                size,
            })
        },
    )
    .await;

    let _ = restore_overlay(&app, monitor_x, monitor_y).await;
    match result {
        Ok(Ok(record)) => Ok(record),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(format!("录制任务失败: {e}")),
    }
}

// ============================================================
// 全局快捷键 / 注册挂点
// ============================================================

/// 注册截图快捷键（与面板/灵动岛共用 global-shortcut 插件）。
/// 只注册一个键：呼出覆盖层后，区域/窗口/全屏都在覆盖层里切换。
/// 被占用只记日志，不影响应用启动；设置页修改快捷键后由
/// `refresh_global_shortcuts` 统一重注册。
pub fn register_hotkeys(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let settings = super::store::load_shot_settings();
    let key = settings.hotkey;
    if key.trim().is_empty() {
        return;
    }
    let gs = app.global_shortcut();
    if let Err(e) = gs.on_shortcut(key.as_str(), move |app, _shortcut, event| {
        // macOS 上 global-hotkey 按下/松开各派发一次，这里只认按下，
        // 否则第二次会撞上 STARTING 守卫，报“截图正在进行中”。
        if !matches!(
            event.state(),
            tauri_plugin_global_shortcut::ShortcutState::Pressed
        ) {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            // 统一以区域模式呼出：覆盖层里可以切到窗口/全屏。
            if let Err(e) = start_capture(app, "region".into(), 0, None).await {
                eprintln!("[zbar-shot] 快捷键截图失败: {e}");
            }
        });
    }) {
        eprintln!("[zbar-shot] 注册截图快捷键 {key} 失败（可能被占用）: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_frame(shift: u32) -> image::RgbaImage {
        let mut img = image::RgbaImage::new(120, 220);
        for y in 0..220u32 {
            for x in 0..120u32 {
                let v = ((y + shift) * 7 + x * 3) as u8;
                img.put_pixel(x, y, image::Rgba([v, v.wrapping_add(41), v.wrapping_add(83), 255]));
            }
        }
        img
    }

    #[test]
    fn overlap_finds_vertical_shift() {
        let first = test_frame(0);
        // 下一帧相当于页面上滚 60px：顶部对应上一帧第 60 行。
        let mut second = image::RgbaImage::new(120, 220);
        for y in 0..220u32 {
            for x in 0..120u32 {
                second.put_pixel(x, y, *first.get_pixel(x, (y + 60).min(219)));
            }
        }
        let overlap = find_overlap(&first, &second).expect("应找到重叠");
        assert!((overlap as i32 - 160).abs() <= 4, "overlap={overlap}");
    }

    #[test]
    fn stitch_appends_only_new_rows() {
        let first = test_frame(0);
        let second = test_frame(60);
        let stitched = stitch_frames(&[first.clone(), second.clone()]).unwrap();
        assert_eq!(stitched.width(), 120);
        // 重叠 160 行，只新增 60 行。
        assert!((stitched.height() as i32 - 280).abs() <= 5, "height={}", stitched.height());
        for x in [0u32, 37, 119] {
            assert_eq!(stitched.get_pixel(x, 220).0, second.get_pixel(x, 160).0);
        }
    }

    #[test]
    fn png_roundtrip_preserves_size() {
        let img = test_frame(0);
        let url = png_data_url(&img).unwrap();
        let decoded = decode_data_url(&url).unwrap();
        assert_eq!(decoded.dimensions(), img.dimensions());
        assert_eq!(decoded.get_pixel(12, 34), img.get_pixel(12, 34));
    }

    #[test]
    fn gif_filter_limits_width() {
        let img = test_frame(0);
        let small = gif_filter(&img, 60);
        assert_eq!(small.width(), 60);
        assert_eq!(small.height(), 110);
    }

    #[test]
    fn gif_frame_accepts_off_by_one_capture() {
        // macOS 上请求 60×110 逻辑矩形可能抓到 61×111 像素，
        // 直接按 60×110 建帧会 panic，gif_frame 必须自己缩放。
        let odd = image::RgbaImage::from_pixel(61, 111, image::Rgba([9, 8, 7, 255]));
        let frame = gif_frame(odd, 60, 110, 12);
        assert_eq!(frame.width, 60);
        assert_eq!(frame.height, 110);
        assert_eq!(frame.buffer.len(), 60 * 110);
        assert_eq!(frame.delay, 12);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod mac_capture_tests {
    /// 真机冒烟：走一次 screencapture 并解码，确保命令参数与坐标换算可用。
    #[test]
    fn screencapture_roundtrip() {
        let image = super::mac_screencapture(0.0, 0.0, 120.0, 80.0).expect("capture failed");
        assert!(image.width() >= 100, "宽度异常: {}", image.width());
        assert!(image.height() >= 60, "高度异常: {}", image.height());
    }

    /// 物理像素 → 逻辑点：Retina 2x 时 2880×1800 应换算成 1440×900。
    #[test]
    fn scale_conversion() {
        super::set_mac_scale(2.0);
        assert_eq!(super::mac_scale(), 2.0);
    }

    /// 窗口列表换算到抓屏图像素后必须都落在图内，且是正整数尺寸。
    #[test]
    fn window_list_stays_inside_capture() {
        let monitor = super::ShotMonitor {
            index: 0,
            name: "test".into(),
            x: 0,
            y: 0,
            width: 2880,
            height: 1800,
            scale_factor: 2.0,
            primary: true,
        };
        let wins = super::screen_windows(&monitor, 2880, 1800);
        // 无窗口的机器（CI/无头）返回空列表也算通过，这里只校验换算结果。
        eprintln!("[test] 窗口列表 {} 个", wins.len());
        for win in &wins {
            assert!(win.x >= 0.0 && win.y >= 0.0);
            assert!(win.width >= 16.0 && win.height >= 16.0);
            assert!(win.x + win.width <= 2881.0);
            assert!(win.y + win.height <= 1801.0);
        }
    }
}
