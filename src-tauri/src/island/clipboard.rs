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
use serde::{Serialize, Serializer};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const CLIPBOARD_EVENT: &str = "zbar://island-clipboard";
/// 条数上限：默认 10，可在设置里调到 5–100
const DEFAULT_LIMIT: usize = 10;
const MIN_LIMIT: usize = 5;
const MAX_LIMIT: usize = 100;

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
/// 历史容器存 Arc 条目：emit / 命令拉取只克隆指针列表（条数 ≤ 上限），
/// png 大缓冲与文本长串不再随每次推送/拉取整份复制。
static ITEMS: OnceLock<Mutex<Vec<Arc<ClipItem>>>> = OnceLock::new();
static LAST_HASH: AtomicU64 = AtomicU64::new(0);
/// 当前条数上限（AtomicUsize 读多写少：轮询线程每拍都要读）
static LIMIT: AtomicUsize = AtomicUsize::new(DEFAULT_LIMIT);
/// 正在写剪贴板的并发计数（截图复制 / 历史回写）：大于 0 时后台轮询跳过
/// 本拍。Windows 的 OpenClipboard 是独占的，轮询读一张大图要卡住剪贴板
/// 几十毫秒，正好赶上用户点「复制」就会 `ClipboardOccupied`。
static WRITING: AtomicUsize = AtomicUsize::new(0);
/// 写剪贴板前的「让行」请求：置位后轮询线程从下一拍起直接跳过，直到
/// 恢复。WRITING 只能拦住「下一拍」，拦不住正在进行中的一拍（拉全尺寸
/// RGBA 持锁几十毫秒），这里补上这一层。
static PAUSED: AtomicBool = AtomicBool::new(false);
/// 轮询线程正在读剪贴板（poll_once 进行中）；让行方据此等待当前拍让出。
static POLLING: AtomicBool = AtomicBool::new(false);

/// 写剪贴板前调用，返回的 guard drop 时计数减一；轮询线程只在计数为 0
/// 时读剪贴板，避免自己抢自己。
pub(crate) struct ClipboardWriteGuard;

impl Drop for ClipboardWriteGuard {
    fn drop(&mut self) {
        WRITING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 请求轮询线程暂停，并等正在进行的读拍让出剪贴板（最多 ~150ms：一拍
/// 读大图的持锁时间在几十毫秒量级，150ms 足够它跑完，不至于白等）。
/// 恢复交给 `begin_write` 收尾——届时 WRITING 计数已生效，轮询自然跳拍。
fn suspend_polling_while_reading() {
    PAUSED.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_millis(150);
    while POLLING.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
}

pub(crate) fn begin_write() -> ClipboardWriteGuard {
    // 先让轮询停拍、等当前读拍退出，再置写计数；恢复暂停标志放在写计数
    // 之后，两个状态无缝衔接，轮询全程不会撞上这次写入。
    suspend_polling_while_reading();
    WRITING.fetch_add(1, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
    ClipboardWriteGuard
}

fn is_writing() -> bool {
    WRITING.load(Ordering::SeqCst) > 0
}

fn app_slot() -> &'static Mutex<Option<AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
}
fn items_slot() -> &'static Mutex<Vec<Arc<ClipItem>>> {
    ITEMS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 剪贴板历史条目。图片只存一份 PNG 原始字节（png 字段）；此前的 base64
/// data URL 副本（1.33 倍膨胀）已移除，改为序列化出口按需生成——emit /
/// 命令响应是人工触发的低频路径，几毫秒编码换掉常驻的双份大缓冲。
/// 历史容器以 Arc 共享（ITEMS），emit / 拉取只克隆指针列表，不复制大缓冲。
#[derive(Debug, Clone)]
pub struct ClipItem {
    pub id: u64,
    pub kind: String,
    pub text: String,
    pub preview: String,
    pub time: i64,
    pub width: u32,
    pub height: u32,
    /// 钉住：不会被新内容挤出、也不会被「清空」清掉
    pub pinned: bool,
    /// 图片条目的 PNG 原始字节（唯一内存副本；文本条目为空）
    pub png: Vec<u8>,
}

impl Serialize for ClipItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        // 字段名与前端既有契约一致（原 serde rename_all = "camelCase" 下的字段）
        let image = if self.kind == "image" {
            png_data_url(&self.png)
        } else {
            String::new()
        };
        let mut s = serializer.serialize_struct("ClipItem", 9)?;
        s.serialize_field("id", &self.id)?;
        s.serialize_field("kind", &self.kind)?;
        s.serialize_field("text", &self.text)?;
        s.serialize_field("preview", &self.preview)?;
        s.serialize_field("image", &image)?;
        s.serialize_field("time", &self.time)?;
        s.serialize_field("width", &self.width)?;
        s.serialize_field("height", &self.height)?;
        s.serialize_field("pinned", &self.pinned)?;
        s.end()
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// 展示顺序：钉住的在前，其余保持插入顺序（新的在前）。
/// 用稳定分区，避免交换法把未钉住条目之间的先后顺序打乱。
fn sort_for_display(items: &mut Vec<Arc<ClipItem>>) {
    let (pinned, rest): (Vec<Arc<ClipItem>>, Vec<Arc<ClipItem>>) =
        items.drain(..).partition(|item| item.pinned);
    items.extend(pinned);
    items.extend(rest);
}

/// 超限淘汰：只淘汰最旧的**未钉住**条目；全钉住时宁可超出上限也不丢。
fn prune_to_limit(items: &mut Vec<Arc<ClipItem>>, limit: usize) {
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
    // 锁内只克隆 Arc 指针列表（≤ 上限 100 条），png / 文本大缓冲零复制；
    // 排序在克隆列表上进行，不改动存储顺序（与事件推送同口径）
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
    guard.insert(0, Arc::new(item));
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
                        // data URL 不再在此生成常驻（旧实现与 png 双份存储），
                        // 序列化出口按需编码，见 ClipItem 的 Serialize
                        let item = ClipItem {
                            id: *next_id,
                            kind: "image".into(),
                            text: String::new(),
                            preview: format!("图片 {width}×{height}"),
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
                // 首拍也捕获当前剪贴板，页面打开就有内容可点；正在写剪贴板
                // 或有写入方请求让行时跳拍，下一拍再读——否则会把用户刚复制
                // 的内容卡在 ClipboardOccupied 上。
                let changed = if is_writing() || PAUSED.load(Ordering::SeqCst) {
                    false
                } else {
                    POLLING.store(true, Ordering::SeqCst);
                    let changed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        poll_once(&mut clip, &mut next_id)
                    }))
                    .unwrap_or(false);
                    POLLING.store(false, Ordering::SeqCst);
                    changed
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
pub fn island_clipboard() -> Vec<Arc<ClipItem>> {
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
pub fn island_clipboard_pin(id: u64, pinned: bool) -> Result<Vec<Arc<ClipItem>>, String> {
    let list = {
        let mut guard = items_slot()
            .lock()
            .map_err(|_| "剪贴板历史不可用".to_string())?;
        let item = guard
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| "剪贴板记录不存在".to_string())?;
        // 修改共享条目走写时复制：只有该条目被克隆，其余条目仍共享
        Arc::make_mut(item).pinned = pinned;
        guard.clone()
    };
    notify_changed();
    Ok(display_list(list))
}

/// 删除单条（钉住的也能删——这是显式操作）。
#[tauri::command]
pub fn island_clipboard_delete(id: u64) -> Result<Vec<Arc<ClipItem>>, String> {
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
pub fn island_clipboard_set_limit(limit: u32) -> Result<Vec<Arc<ClipItem>>, String> {
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

fn display_list(mut items: Vec<Arc<ClipItem>>) -> Vec<Arc<ClipItem>> {
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
            time: id as i64,
            width: 0,
            height: 0,
            pinned,
            png: Vec::new(),
        }
    }

    fn ids(items: &[Arc<ClipItem>]) -> Vec<u64> {
        items.iter().map(|item| item.id).collect()
    }

    /// 超限时淘汰最旧的未钉住条目，钉住的必须活着
    #[test]
    fn prune_drops_oldest_unpinned_only() {
        let mut items = vec![
            Arc::new(item(4, false)),
            Arc::new(item(3, true)),
            Arc::new(item(2, false)),
            Arc::new(item(1, false)),
        ];
        prune_to_limit(&mut items, 3);
        assert_eq!(ids(&items), vec![4, 3, 2]);
    }

    /// 全部钉住时宁可超出上限，也不能悄悄丢用户钉住的东西
    #[test]
    fn prune_keeps_all_when_everything_pinned() {
        let mut items = vec![
            Arc::new(item(3, true)),
            Arc::new(item(2, true)),
            Arc::new(item(1, true)),
        ];
        prune_to_limit(&mut items, 2);
        assert_eq!(ids(&items), vec![3, 2, 1]);
    }

    /// 展示顺序：钉住的在前，未钉住的相对顺序不变（稳定分区）
    #[test]
    fn display_order_is_stable_partition() {
        let mut items = vec![
            Arc::new(item(4, false)),
            Arc::new(item(3, true)),
            Arc::new(item(2, false)),
            Arc::new(item(1, true)),
        ];
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

    /// begin_write 全程置写计数、不残留暂停标志：写期间轮询必须跳拍
    /// （is_writing 为真），guard drop 后一切复原。
    #[test]
    fn begin_write_marks_writing_and_resumes_polling() {
        assert!(!is_writing(), "测试前提：没有并发写入");
        {
            let _guard = begin_write();
            assert!(is_writing(), "写入期间写计数必须生效");
            assert!(!PAUSED.load(Ordering::SeqCst), "让行标志用完即清");
        }
        assert!(!is_writing(), "guard drop 后写计数必须归零");
    }

    /// 图片的 base64 data URL 只在序列化出口按需生成：内存条目只存一份
    /// png 原始字节；文本条目 image 序列化为空串（前端契约不变）。
    #[test]
    fn serialization_builds_image_data_url_on_demand() {
        let png = b"not-a-real-png".to_vec();
        let image_item = ClipItem {
            id: 1,
            kind: "image".into(),
            text: String::new(),
            preview: "图片".into(),
            time: 1,
            width: 2,
            height: 3,
            pinned: false,
            png: png.clone(),
        };
        let json = serde_json::to_string(&image_item).unwrap();
        let expected = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );
        assert!(json.contains(&expected), "序列化应输出按需生成的 data URL");

        let text_item = item(2, false);
        let json = serde_json::to_string(&text_item).unwrap();
        assert!(json.contains("\"image\":\"\""), "文本条目 image 应为空串");
    }
}
