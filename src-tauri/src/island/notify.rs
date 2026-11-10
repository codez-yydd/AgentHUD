//! 灵动岛消息：只读轮询 Windows 通知数据库 `wpndatabase.db`。
//!
//! 这是 wisland 的同一套做法：不申请通知监听权限、不读聊天内容存储，
//! 只把系统通知里的 toast XML 文本按应用映射成 QQ / 微信 / 邮件 / 飞书 /
//! 钉钉 五类。首次启动读最近 30 条作为历史（标记已读），之后新增的条目
//! 计入未读数并广播 `zbar://island-notify`。

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub const NOTIFY_EVENT: &str = "zbar://island-notify";
const HISTORY_MAX: usize = 50;
const _FILETIME_DIFF: i64 = 116_444_736_000_000_000;

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static ITEMS: OnceLock<Mutex<Vec<NotifyItem>>> = OnceLock::new();
static MAX_ID: AtomicI64 = AtomicI64::new(-1);

fn app_slot() -> &'static Mutex<Option<AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
}
fn items_slot() -> &'static Mutex<Vec<NotifyItem>> {
    ITEMS.get_or_init(|| Mutex::new(Vec::new()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyItem {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub sender: String,
    pub body: String,
    pub time: i64,
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NotifyState {
    pub items: Vec<NotifyItem>,
    pub unread: usize,
}

fn emit(app: &AppHandle) {
    let items = items_slot().lock().map(|g| g.clone()).unwrap_or_default();
    let unread = items.iter().filter(|i| !i.read).count();
    let _ = app.emit(
        NOTIFY_EVENT,
        NotifyState {
            items,
            unread,
        },
    );
}

fn db_path() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local)
            .join("Microsoft")
            .join("Windows")
            .join("Notifications")
            .join("wpndatabase.db"),
    )
}

fn filetime_to_ms(ft: i64) -> i64 {
    if ft <= 0 {
        return chrono::Utc::now().timestamp_millis();
    }
    (ft - _FILETIME_DIFF) / 10_000
}

fn map_primary(primary: &str) -> Option<(&'static str, &'static str)> {
    let p = primary.to_ascii_lowercase();
    let hit = |needles: &[&str]| needles.iter().any(|n| p.contains(n));
    if p.contains("qqmusic") || p.contains("kugou") || p.contains("cloudmusic") {
        return None;
    }
    if hit(&["weixin", "wechat"]) {
        return Some(("wechat", "微信"));
    }
    if p.contains("qq") {
        return Some(("qq", "QQ"));
    }
    if hit(&["feishu", "lark"]) {
        return Some(("feishu", "飞书"));
    }
    if p.contains("dingtalk") || p.contains("dingding") {
        return Some(("dingtalk", "钉钉"));
    }
    if hit(&[
        "thunderbird",
        "outlook",
        "windowscommunicationsapps",
        "windowslive.mail",
        "hxoutlook",
    ]) {
        return Some(("mail", "邮件"));
    }
    None
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

/// 从 toast XML 里按顺序提取 `<text>` 节点文本（最多 4 条）
fn toast_texts(payload: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(payload);
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut pos = 0usize;
    while out.len() < 4 {
        let Some(open_rel) = lower[pos..].find("<text") else {
            break;
        };
        let open = pos + open_rel;
        let Some(gt_rel) = lower[open..].find('>') else {
            break;
        };
        let start = open + gt_rel + 1;
        let Some(close_rel) = lower[start..].find("</text>") else {
            break;
        };
        let end = start + close_rel;
        let value = strip_tags(&text[start..end]);
        if !value.is_empty() {
            out.push(value.chars().take(200).collect());
        }
        pos = end + 7;
    }
    out
}

fn load_rows(limit: i64) -> Vec<(i64, String, i64, String, Vec<u8>)> {
    let Some(path) = db_path() else {
        return Vec::new();
    };
    if !path.exists() {
        return Vec::new();
    }
    let conn = match rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let _ = conn.busy_timeout(Duration::from_millis(1200));
    let mut stmt = match conn.prepare(
        "SELECT n.Id, h.PrimaryId, n.ArrivalTime, n.Type, n.Payload \
         FROM Notification n JOIN NotificationHandler h ON h.RecordId = n.HandlerId \
         ORDER BY n.Id DESC LIMIT ?1",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([limit], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3).unwrap_or_default(),
            row.get::<_, Vec<u8>>(4).unwrap_or_default(),
        ))
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.flatten().collect()
}

fn rows_to_items(rows: Vec<(i64, String, i64, String, Vec<u8>)>) -> Vec<NotifyItem> {
    let mut out = Vec::new();
    for (id, primary, arrival, ntype, payload) in rows {
        if !ntype.is_empty() && ntype.to_ascii_lowercase() != "toast" {
            continue;
        }
        let Some((kind, name)) = map_primary(&primary) else {
            continue;
        };
        let texts = toast_texts(&payload);
        if texts.is_empty() {
            continue;
        }
        let sender = texts.first().cloned().unwrap_or_default();
        let body = if matches!(kind, "feishu" | "dingtalk" | "mail") {
            texts.get(1).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        out.push(NotifyItem {
            id,
            kind: kind.to_string(),
            name: name.to_string(),
            sender: sender.chars().take(60).collect(),
            body: body.chars().take(160).collect(),
            time: filetime_to_ms(arrival),
            read: false,
        });
    }
    out
}

fn init_history(app: &AppHandle) {
    let rows = load_rows(HISTORY_MAX as i64);
    if rows.is_empty() {
        return;
    }
    let max_id = rows.iter().map(|r| r.0).max().unwrap_or(0);
    MAX_ID.store(max_id, Ordering::SeqCst);
    let mut items = rows_to_items(rows);
    for item in &mut items {
        item.read = true;
    }
    if let Ok(mut guard) = items_slot().lock() {
        *guard = items;
    }
    emit(app);
}

fn poll_new(app: &AppHandle) {
    let last = MAX_ID.load(Ordering::SeqCst);
    if last < 0 {
        init_history(app);
        return;
    }
    let rows = load_rows(120);
    if rows.is_empty() {
        return;
    }
    let new_rows: Vec<_> = rows
        .into_iter()
        .filter(|r| r.0 > last)
        .collect();
    if new_rows.is_empty() {
        return;
    }
    let max_id = new_rows.iter().map(|r| r.0).max().unwrap_or(last);
    MAX_ID.store(max_id, Ordering::SeqCst);
    let mut fresh = rows_to_items(new_rows);
    fresh.sort_by_key(|i| i.id);
    if fresh.is_empty() {
        return;
    }
    if let Ok(mut guard) = items_slot().lock() {
        for item in fresh.drain(..) {
            guard.insert(0, item);
        }
        guard.truncate(HISTORY_MAX);
    }
    emit(app);
}

pub fn start(app: AppHandle) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(app.clone());
    }
    STOP.store(false, Ordering::SeqCst);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    thread::Builder::new()
        .name("zbar-island-notify".into())
        .spawn(move || {
            init_history(&app);
            while !STOP.load(Ordering::Relaxed) && GEN.load(Ordering::SeqCst) == gen {
                poll_new(&app);
                thread::sleep(Duration::from_millis(2000));
            }
        })
        .ok();
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    GEN.fetch_add(1, Ordering::SeqCst);
}

#[tauri::command]
pub fn island_notify() -> NotifyState {
    let items = items_slot().lock().map(|g| g.clone()).unwrap_or_default();
    let unread = items.iter().filter(|i| !i.read).count();
    NotifyState { items, unread }
}

#[tauri::command]
pub fn island_notify_mark_read() -> NotifyState {
    if let Ok(mut guard) = items_slot().lock() {
        for item in guard.iter_mut() {
            item.read = true;
        }
    }
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        emit(&app);
    }
    island_notify()
}

#[tauri::command]
pub fn island_notify_clear(kind: Option<String>) -> NotifyState {
    if let Ok(mut guard) = items_slot().lock() {
        if let Some(kind) = kind.as_deref() {
            guard.retain(|i| i.kind != kind);
        } else {
            guard.clear();
        }
    }
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        emit(&app);
    }
    island_notify()
}

#[tauri::command]
pub fn island_notify_open_settings() -> Result<(), String> {
    open::that("ms-settings:notifications").map_err(|e| format!("打开通知设置失败: {e}"))
}
