//! 灵动岛剪贴板：最近若干条文本 / 图片（**内存历史，退出即清空**）。
//!
//! 用 arboard 轮询系统剪贴板；同一内容不重复入栈。点击历史项经
//! `island_clipboard_copy` 写回剪贴板。
//!
//! 刻意不落盘：剪贴板里经常是密码 / token / 私密内容，写进
//! `~/.agenthud/` 属于凭空多出来的泄漏面。条数上限可调（5–100，默认 10），
//! 钉住的条目不会被新内容挤出、也不会被「清空」清掉；图片条目可转文字
//! （前端复用截图 OCR 命令，本模块不管）。

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
/// 条数上限：默认 10，可在设置里调到 5–100
const DEFAULT_LIMIT: usize = 10;
const MIN_LIMIT: usize = 5;
const MAX_LIMIT: usize = 100;

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static ITEMS: OnceLock<Mutex<Vec<ClipItem>>> = OnceLock::new();
static LAST_HASH: AtomicU64 = AtomicU64::new(0);
/// 当前条数上限（AtomicUsize 读多写少：轮询线程每拍都要读）
static LIMIT: AtomicUsize = AtomicUsize::new(DEFAULT_LIMIT);
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
    /// 钉住：不会被新内容挤出、也不会被「清空」清掉
    pub pinned: bool,
    #[serde(skip)]
    pub png: Vec<u8>,
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// 展示顺序：钉住的在前，其余保持插入顺序（新的在前）。
/// 用稳定分区，避免交换法把未钉住条目之间的先后顺序打乱。
fn sort_for_display(items: &mut Vec<ClipItem>) {
    let (pinned, rest): (Vec<ClipItem>, Vec<ClipItem>) =
        items.drain(..).partition(|item| item.pinned);
    items.extend(pinned);
    items.extend(rest);
}

/// 超限淘汰：只淘汰最旧的**未钉住**条目；全钉住时宁可超出上限也不丢。
fn prune_to_limit(items: &mut Vec<ClipItem>, limit: usize) {
    while items.len() > limit {
        match items.iter().rposition(|item| !item.pinned) {
            Some(index) => {
                items.remove(index);
            }
            None => break,
        }
    }
}

fn clamp_limit(limit: usize) -> usize {
    limit.clamp(MIN_LIMIT, MAX_LIMIT)
}

fn emit(app: &AppHandle) {
    let mut items = items_slot().lock().map(|g| g.clone()).unwrap_or_default();
    sort_for_display(&mut items);
    let _ = app.emit(CLIPBOARD_EVENT, items);
}

fn push_item(item: ClipItem) {
    let mut guard = match items_slot().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    guard.retain(|i| i.preview != item.preview || i.kind != item.kind);
    guard.insert(0, item);
    prune_to_limit(&mut guard, LIMIT.load(Ordering::Relaxed));
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
                            pinned: false,
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
                    pinned: false,
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
    let mut items = items_slot().lock().map(|g| g.clone()).unwrap_or_default();
    // 与事件推送同一口径：钉住的在前，保证启动后首次拉取顺序一致
    sort_for_display(&mut items);
    items
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

/// 清空历史：钉住的条目保留（用户明确钉住的东西不该被「清空」顺手带走）。
#[tauri::command]
pub fn island_clipboard_clear() -> Result<(), String> {
    if let Ok(mut guard) = items_slot().lock() {
        guard.retain(|item| item.pinned);
    }
    LAST_HASH.store(0, Ordering::SeqCst);
    notify_changed();
    Ok(())
}

/// 钉住 / 取消钉住，返回调整后的完整列表（前端直接替换，省一次全量拉取）。
#[tauri::command]
pub fn island_clipboard_pin(id: u64, pinned: bool) -> Result<Vec<ClipItem>, String> {
    let list = {
        let mut guard = items_slot()
            .lock()
            .map_err(|_| "剪贴板历史不可用".to_string())?;
        let item = guard
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| "剪贴板记录不存在".to_string())?;
        item.pinned = pinned;
        guard.clone()
    };
    notify_changed();
    Ok(display_list(list))
}

/// 删除单条（钉住的也能删——这是显式操作）。
#[tauri::command]
pub fn island_clipboard_delete(id: u64) -> Result<Vec<ClipItem>, String> {
    let list = {
        let mut guard = items_slot()
            .lock()
            .map_err(|_| "剪贴板历史不可用".to_string())?;
        guard.retain(|item| item.id != id);
        guard.clone()
    };
    notify_changed();
    Ok(display_list(list))
}

/// 调整条数上限（5–100）：立即按新上限淘汰最旧的未钉住条目。
#[tauri::command]
pub fn island_clipboard_set_limit(limit: u32) -> Result<Vec<ClipItem>, String> {
    let next = clamp_limit(limit as usize);
    LIMIT.store(next, Ordering::Relaxed);
    let list = {
        let mut guard = items_slot()
            .lock()
            .map_err(|_| "剪贴板历史不可用".to_string())?;
        prune_to_limit(&mut guard, next);
        guard.clone()
    };
    notify_changed();
    Ok(display_list(list))
}

/// 当前条数上限（页面首次渲染读一次）。
#[tauri::command]
pub fn island_clipboard_limit() -> u32 {
    LIMIT.load(Ordering::Relaxed) as u32
}

fn display_list(mut items: Vec<ClipItem>) -> Vec<ClipItem> {
    sort_for_display(&mut items);
    items
}

fn notify_changed() {
    if let Some(app) = app_slot().lock().ok().and_then(|slot| slot.clone()) {
        emit(&app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, pinned: bool) -> ClipItem {
        ClipItem {
            id,
            kind: "text".into(),
            text: format!("t{id}"),
            preview: format!("t{id}"),
            image: String::new(),
            time: id as i64,
            width: 0,
            height: 0,
            pinned,
            png: Vec::new(),
        }
    }

    fn ids(items: &[ClipItem]) -> Vec<u64> {
        items.iter().map(|item| item.id).collect()
    }

    /// 超限时淘汰最旧的未钉住条目，钉住的必须活着
    #[test]
    fn prune_drops_oldest_unpinned_only() {
        let mut items = vec![item(4, false), item(3, true), item(2, false), item(1, false)];
        prune_to_limit(&mut items, 3);
        assert_eq!(ids(&items), vec![4, 3, 2]);
    }

    /// 全部钉住时宁可超出上限，也不能悄悄丢用户钉住的东西
    #[test]
    fn prune_keeps_all_when_everything_pinned() {
        let mut items = vec![item(3, true), item(2, true), item(1, true)];
        prune_to_limit(&mut items, 2);
        assert_eq!(ids(&items), vec![3, 2, 1]);
    }

    /// 展示顺序：钉住的在前，未钉住的相对顺序不变（稳定分区）
    #[test]
    fn display_order_is_stable_partition() {
        let mut items = vec![item(4, false), item(3, true), item(2, false), item(1, true)];
        sort_for_display(&mut items);
        assert_eq!(ids(&items), vec![3, 1, 4, 2]);
    }

    /// 上限夹到 5..=100
    #[test]
    fn limit_is_clamped_to_sane_range() {
        assert_eq!(clamp_limit(1), MIN_LIMIT);
        assert_eq!(clamp_limit(1000), MAX_LIMIT);
        assert_eq!(clamp_limit(42), 42);
    }
}
