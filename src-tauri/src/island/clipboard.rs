//! 灵动岛剪贴板：最近 10 条文本 / 图片（内存历史，退出即清空）。
//!
//! 用 arboard 轮询系统剪贴板；同一内容不重复入栈。点击历史项经
//! `island_clipboard_copy` 写回剪贴板。

use base64::Engine;
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub const CLIPBOARD_EVENT: &str = "zbar://island-clipboard";
const MAX_ITEMS: usize = 10;

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static ITEMS: OnceLock<Mutex<Vec<ClipItem>>> = OnceLock::new();
static LAST_HASH: AtomicU64 = AtomicU64::new(0);
/// 正在写剪贴板的并发计数（截图复制 / 历史回写）：大于 0 时后台轮询跳过
/// 本拍。Windows 的 OpenClipboard 是独占的，轮询读一张大图要卡住剪贴板
/// 几十毫秒，正好赶上用户点「复制」就会 `ClipboardOccupied`。
static WRITING: AtomicUsize = AtomicUsize::new(0);

/// 写剪贴板前调用，返回的 guard drop 时计数减一；轮询线程只在计数为 0
/// 时读剪贴板，避免自己抢自己。
pub(crate) struct ClipboardWriteGuard;

impl Drop for ClipboardWriteGuard {
    fn drop(&mut self) {
        WRITING.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) fn begin_write() -> ClipboardWriteGuard {
    WRITING.fetch_add(1, Ordering::SeqCst);
    ClipboardWriteGuard
}

fn is_writing() -> bool {
    WRITING.load(Ordering::SeqCst) > 0
}

fn app_slot() -> &'static Mutex<Option<AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
}
fn items_slot() -> &'static Mutex<Vec<ClipItem>> {
    ITEMS.get_or_init(|| Mutex::new(Vec::new()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipItem {
    pub id: u64,
    pub kind: String,
    pub text: String,
    pub preview: String,
    pub image: String,
    pub time: i64,
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    pub png: Vec<u8>,
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

fn emit(app: &AppHandle) {
    let items = items_slot().lock().map(|g| g.clone()).unwrap_or_default();
    let _ = app.emit(CLIPBOARD_EVENT, items);
}

fn push_item(item: ClipItem) {
    let mut guard = match items_slot().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    guard.retain(|i| i.preview != item.preview || i.kind != item.kind);
    guard.insert(0, item);
    guard.truncate(MAX_ITEMS);
}

fn png_data_url(png: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

fn poll_once(clip: &mut arboard::Clipboard, next_id: &mut u64) -> bool {
    // 图片优先：部分播放器复制图片时同时带一个空文本
    if let Ok(img) = clip.get_image() {
        let rgba = img.bytes.to_vec();
        if !rgba.is_empty() {
            let hash = hash_bytes(&rgba);
            if LAST_HASH.swap(hash, Ordering::SeqCst) != hash {
                let width = img.width as u32;
                let height = img.height as u32;
                if let Some(buffer) =
                    image::RgbaImage::from_raw(width, height, rgba.clone())
                        .map(image::DynamicImage::ImageRgba8)
                {
                    let mut png = Cursor::new(Vec::new());
                    if buffer.write_to(&mut png, image::ImageFormat::Png).is_ok() {
                        let bytes = png.into_inner();
                        *next_id += 1;
                        let item = ClipItem {
                            id: *next_id,
                            kind: "image".into(),
                            text: String::new(),
                            preview: format!("图片 {width}×{height}"),
                            image: png_data_url(&bytes),
                            time: chrono::Utc::now().timestamp_millis(),
                            width,
                            height,
                            png: bytes,
                        };
                        push_item(item);
                        return true;
                    }
                }
            }
        }
    }
    if let Ok(text) = clip.get_text() {
        let text = text.trim_end().to_string();
        if !text.trim().is_empty() {
            let hash = hash_bytes(text.as_bytes());
            if LAST_HASH.swap(hash, Ordering::SeqCst) != hash {
                *next_id += 1;
                let preview = text
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(80)
                    .collect::<String>();
                let item = ClipItem {
                    id: *next_id,
                    kind: "text".into(),
                    text,
                    preview,
                    image: String::new(),
                    time: chrono::Utc::now().timestamp_millis(),
                    width: 0,
                    height: 0,
                    png: Vec::new(),
                };
                push_item(item);
                return true;
            }
        }
    }
    false
}

pub fn start(app: AppHandle) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(app.clone());
    }
    STOP.store(false, Ordering::SeqCst);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    thread::Builder::new()
        .name("zbar-island-clipboard".into())
        .spawn(move || {
            let mut clip = match arboard::Clipboard::new() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[zbar-island] 剪贴板不可用: {e}");
                    return;
                }
            };
            let mut next_id = 0u64;
            while !STOP.load(Ordering::Relaxed) && GEN.load(Ordering::SeqCst) == gen {
                // 首拍也捕获当前剪贴板，页面打开就有内容可点；正在写剪贴板时
                // 跳拍，下一拍再读——否则会把用户刚复制的内容卡在
                // ClipboardOccupied 上。
                let changed = if is_writing() {
                    false
                } else {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        poll_once(&mut clip, &mut next_id)
                    }))
                    .unwrap_or(false)
                };
                if changed {
                    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
                        emit(&app);
                    }
                }
                thread::sleep(Duration::from_millis(800));
            }
        })
        .ok();
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    GEN.fetch_add(1, Ordering::SeqCst);
}

#[tauri::command]
pub fn island_clipboard() -> Vec<ClipItem> {
    items_slot().lock().map(|g| g.clone()).unwrap_or_default()
}

#[tauri::command]
pub fn island_clipboard_copy(id: u64) -> Result<(), String> {
    let item = items_slot()
        .lock()
        .map_err(|_| "剪贴板历史不可用".to_string())?
        .iter()
        .find(|i| i.id == id)
        .cloned()
        .ok_or_else(|| "剪贴板记录不存在".to_string())?;
    // 回写期间让轮询线程停一拍，避免它正读着剪贴板时这里打不开。
    let _writing = begin_write();
    // 图片只解码一次，重试时只重写剪贴板
    let rgba = if item.kind == "image" {
        Some(
            image::load_from_memory(&item.png)
                .map_err(|e| format!("图片解码失败: {e}"))?
                .to_rgba8(),
        )
    } else {
        None
    };
    let mut last_err = String::new();
    for attempt in 0..3 {
        match write_clip_item(&item, rgba.as_ref()) {
            Ok(()) => {
                let hash = match rgba.as_ref() {
                    Some(img) => hash_bytes(img.as_raw()),
                    None => hash_bytes(item.text.as_bytes()),
                };
                LAST_HASH.store(hash, Ordering::SeqCst);
                return Ok(());
            }
            Err(e) => last_err = e,
        }
        if attempt < 2 {
            thread::sleep(Duration::from_millis(50));
        }
    }
    Err(last_err)
}

/// 单次回写系统剪贴板（图片 / 文本）。
fn write_clip_item(item: &ClipItem, rgba: Option<&image::RgbaImage>) -> Result<(), String> {
    let mut clip = arboard::Clipboard::new().map_err(|e| format!("剪贴板不可用: {e}"))?;
    match rgba {
        Some(img) => {
            let data = arboard::ImageData {
                width: img.width() as usize,
                height: img.height() as usize,
                bytes: std::borrow::Cow::Borrowed(img.as_raw()),
            };
            clip.set_image(data)
                .map_err(|e| format!("写入图片失败: {e}"))
        }
        None => clip
            .set_text(item.text.as_str())
            .map_err(|e| format!("写入文本失败: {e}")),
    }
}

#[tauri::command]
pub fn island_clipboard_clear() -> Result<(), String> {
    if let Ok(mut guard) = items_slot().lock() {
        guard.clear();
    }
    LAST_HASH.store(0, Ordering::SeqCst);
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        emit(&app);
    }
    Ok(())
}
