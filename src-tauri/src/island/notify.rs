//! 灵动岛消息：只读轮询 Windows 通知数据库 `wpndatabase.db`，
//! 并识别飞书 / 钉钉右下角自绘的通知浮窗。
//!
//! 通知库部分沿用 wisland 的做法：不申请通知监听权限、不读聊天内容存储，
//! 只把系统通知里的 toast XML 文本按应用映射成 QQ / 微信 / 邮件 / 飞书 /
//! 钉钉 五类。首次启动读最近 50 条作为历史（标记已读），之后新增的条目
//! 计入未读数并广播 `zbar://island-notify`。
//!
//! 飞书 / 钉钉默认不写 Windows 系统通知、也不闪任务栏，消息到达时只会创建
//! 一个贴在工作区右下角的小浮窗（无边框 + 置顶 + 工具窗口）。这里每秒
//! `EnumWindows` 找一次这种窗口，发送人 / 正文用 UI Automation 读（读不到
//! 就只提醒「新消息」），条目同样进消息列表。`island_notify_open` 供消息页
//! 点按：优先聚焦已运行的来源应用，没运行才按常见安装路径启动。

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
const POLL_MS: u64 = 2000;

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

/// 消息线程的存活条件：未 stop，且没有被新一代 `start()` 顶掉。
fn running(gen: u64) -> bool {
    !STOP.load(Ordering::Relaxed) && GEN.load(Ordering::SeqCst) == gen
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
    /// 来源进程名（小写，可能为空）。消息页点按时优先用它定位应用。
    pub exe: String,
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

fn push_item(item: NotifyItem) {
    if let Ok(mut guard) = items_slot().lock() {
        guard.insert(0, item);
        guard.truncate(HISTORY_MAX);
    }
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

/// 通知库里的应用标识 → 可执行文件名（点消息时优先打开这个应用）。
fn exe_for_primary(primary: &str) -> &'static str {
    let p = primary.to_ascii_lowercase();
    if p.contains("thunderbird") {
        "thunderbird.exe"
    } else if p.contains("windowscommunicationsapps") {
        "hxoutlook.exe"
    } else if p.contains("outlookforwindows") || p.contains("olk") {
        // 新版 Outlook（olk.exe）的 AUMID 是 Microsoft.OutlookForWindows
        "olk.exe"
    } else if p.contains("outlook") {
        "outlook.exe"
    } else if p.contains("weixin") || p.contains("wechat") {
        "weixin.exe"
    } else if p.contains("qq") {
        "qq.exe"
    } else if p.contains("feishu") || p.contains("lark") {
        "feishu.exe"
    } else if p.contains("dingtalk") || p.contains("dingding") {
        "dingtalk.exe"
    } else {
        ""
    }
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
            exe: exe_for_primary(&primary).to_string(),
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
    let new_rows: Vec<_> = rows.into_iter().filter(|r| r.0 > last).collect();
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
    for item in fresh {
        push_item(item);
    }
    emit(app);
}

pub fn start(app: AppHandle) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(app.clone());
    }
    STOP.store(false, Ordering::SeqCst);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let popup_app = app.clone();
    thread::Builder::new()
        .name("zbar-island-notify".into())
        .spawn(move || {
            init_history(&app);
            while running(gen) {
                poll_new(&app);
                thread::sleep(Duration::from_millis(POLL_MS));
            }
        })
        .ok();
    start_popup_watch(popup_app, gen);
}

#[cfg(target_os = "windows")]
fn start_popup_watch(app: AppHandle, gen: u64) {
    win::start_watch(app, gen);
}

#[cfg(not(target_os = "windows"))]
fn start_popup_watch(app: AppHandle, gen: u64) {
    let _ = (app, gen);
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

fn mark_kind_read(kind: &str) {
    if let Ok(mut guard) = items_slot().lock() {
        for item in guard.iter_mut() {
            if item.kind == kind {
                item.read = true;
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn open_message_app(kind: &str, exe: Option<&str>) -> Result<(), String> {
    win::open_app(kind, exe)
}

#[cfg(not(target_os = "windows"))]
fn open_message_app(kind: &str, exe: Option<&str>) -> Result<(), String> {
    let _ = (kind, exe);
    Err("当前平台暂不支持从消息打开应用".to_string())
}

/// 消息页点按：聚焦 / 启动来源应用，并把该类消息标记为已读。
#[tauri::command]
pub fn island_notify_open(kind: String, exe: Option<String>) -> Result<NotifyState, String> {
    open_message_app(&kind, exe.as_deref())?;
    mark_kind_read(&kind);
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        emit(&app);
    }
    Ok(island_notify())
}

#[tauri::command]
pub fn island_notify_open_settings() -> Result<(), String> {
    open::that("ms-settings:notifications").map_err(|e| format!("打开通知设置失败: {e}"))
}

#[cfg(target_os = "windows")]
mod win {
    //! 飞书 / 钉钉通知浮窗的识别、正文读取，以及「点消息打开应用」。

    use super::{emit, push_item, running, NotifyItem};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, TRUE};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{
        AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
        UIA_TextControlTypeId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW,
        GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        SetForegroundWindow, ShowWindow, GWL_EXSTYLE, GWL_STYLE, SW_RESTORE, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST, WS_POPUP,
    };

    /// 只有这些 Electron 应用会把新消息画成右下角浮窗；QQ / 微信仍走 toast / 任务栏闪烁。
    const POPUP_APPS: [&str; 3] = ["feishu.exe", "lark.exe", "dingtalk.exe"];
    const POPUP_MIN_W: i32 = 160;
    const POPUP_MAX_W: i32 = 900;
    const POPUP_MIN_H: i32 = 40;
    const POPUP_MAX_H: i32 = 600;
    const POPUP_EDGE_PX: i32 = 300;
    const POPUP_EDGE_SLACK: i32 = 8;
    const POPUP_POLL_MS: u64 = 1000;
    const POPUP_SEEN_MS: u128 = 10_000;
    const POPUP_DEDUPE_MS: u128 = 1_500;
    const POPUP_TEXT_DELAY_MS: u64 = 400;
    const POPUP_TEXT_TRIES: usize = 3;

    static POPUP_ID: AtomicI64 = AtomicI64::new(-1);

    fn seen_slot() -> &'static Mutex<HashMap<isize, Instant>> {
        static SEEN: OnceLock<Mutex<HashMap<isize, Instant>>> = OnceLock::new();
        SEEN.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn last_slot() -> &'static Mutex<HashMap<&'static str, Instant>> {
        static LAST: OnceLock<Mutex<HashMap<&'static str, Instant>>> = OnceLock::new();
        LAST.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn start_watch(app: super::AppHandle, gen: u64) {
        thread::Builder::new()
            .name("zbar-island-popup".into())
            .spawn(move || {
                // 启动时先记下已经存在的浮窗，避免把启动前的旧消息当成新消息。
                let now = Instant::now();
                if let Ok(mut seen) = seen_slot().lock() {
                    for (hwnd, _) in scan() {
                        seen.insert(hwnd_key(hwnd), now);
                    }
                }
                while running(gen) {
                    for (hwnd, exe) in scan() {
                        handle_popup(&app, hwnd, exe, gen);
                    }
                    thread::sleep(Duration::from_millis(POPUP_POLL_MS));
                }
            })
            .ok();
    }

    struct ScanState {
        found: Vec<(HWND, &'static str)>,
    }

    fn scan() -> Vec<(HWND, &'static str)> {
        let mut state = ScanState { found: Vec::new() };
        unsafe {
            let _ = EnumWindows(
                Some(enum_popup),
                LPARAM(&mut state as *mut ScanState as *mut c_void as isize),
            );
        }
        state.found
    }

    unsafe extern "system" fn enum_popup(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = unsafe { &mut *(lparam.0 as *mut ScanState) };
        if let Some(hit) = unsafe { popup_of(hwnd) } {
            state.found.push(hit);
        }
        TRUE
    }

    /// 这个窗口像不像飞书 / 钉钉的新消息浮窗。
    unsafe fn popup_of(hwnd: HWND) -> Option<(HWND, &'static str)> {
        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if style & WS_POPUP.0 == 0
            || ex & WS_EX_TOOLWINDOW.0 == 0
            || ex & WS_EX_TOPMOST.0 == 0
        {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..MONITORINFO::default()
        };
        if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
            return None;
        }
        if !popup_geometry_ok(
            rect.right - rect.left,
            rect.bottom - rect.top,
            rect.right,
            rect.bottom,
            mi.rcWork.right,
            mi.rcWork.bottom,
        ) {
            return None;
        }
        let exe = exe_name(hwnd)?;
        let app = POPUP_APPS.iter().copied().find(|a| *a == exe.as_str())?;
        // 飞书的其它弹窗（更新 / 设置）通常有标题；Electron 顶层窗口类固定是 Chrome_WidgetWin_*。
        if matches!(app, "feishu.exe" | "lark.exe") && (!class_is_chromium(hwnd) || !title_is_empty(hwnd))
        {
            return None;
        }
        Some((hwnd, app))
    }

    /// 尺寸 160–900 × 40–600，且右 / 下边贴在工作区右下角附近。
    fn popup_geometry_ok(
        w: i32,
        h: i32,
        right: i32,
        bottom: i32,
        work_right: i32,
        work_bottom: i32,
    ) -> bool {
        (POPUP_MIN_W..=POPUP_MAX_W).contains(&w)
            && (POPUP_MIN_H..=POPUP_MAX_H).contains(&h)
            && right >= work_right - POPUP_EDGE_PX
            && right <= work_right + POPUP_EDGE_SLACK
            && bottom >= work_bottom - POPUP_EDGE_PX
            && bottom <= work_bottom + POPUP_EDGE_SLACK
    }

    fn hwnd_key(hwnd: HWND) -> isize {
        hwnd.0 as isize
    }

    /// 窗口句柄 → 小写 exe 文件名（失败返回 None）。
    unsafe fn exe_name(hwnd: HWND) -> Option<String> {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        res.ok()?;
        if len == 0 || len as usize > buf.len() {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_ascii_lowercase())
    }

    unsafe fn class_is_chromium(hwnd: HWND) -> bool {
        let mut buf = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut buf);
        if len <= 0 {
            return false;
        }
        String::from_utf16_lossy(&buf[..len as usize])
            .to_ascii_lowercase()
            .starts_with("chrome_widgetwin")
    }

    unsafe fn title_is_empty(hwnd: HWND) -> bool {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return true;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        n <= 0 || String::from_utf16_lossy(&buf[..n as usize]).trim().is_empty()
    }

    fn popup_kind(exe: &str) -> (&'static str, &'static str) {
        if exe == "dingtalk.exe" {
            ("dingtalk", "钉钉")
        } else {
            ("feishu", "飞书")
        }
    }

    fn handle_popup(app: &super::AppHandle, hwnd: HWND, exe: &'static str, gen: u64) {
        let (kind, name) = popup_kind(exe);
        if recently_seen(hwnd, kind) {
            return;
        }
        // HWND 不是 Send：跨线程只传句柄数值，进线程后再拼回 HWND。
        let raw = hwnd_key(hwnd);
        let app = app.clone();
        thread::Builder::new()
            .name("zbar-island-popup-text".into())
            .spawn(move || {
                let hwnd = HWND(raw as *mut c_void);
                let (sender, body) = read_text(hwnd).unwrap_or_default();
                if !running(gen) {
                    return; // 消息监听已停 / 已重启，旧线程不再写入
                }
                push_item(NotifyItem {
                    id: POPUP_ID.fetch_sub(1, Ordering::SeqCst),
                    kind: kind.to_string(),
                    name: name.to_string(),
                    sender,
                    body,
                    time: chrono::Utc::now().timestamp_millis(),
                    read: false,
                    exe: exe.to_string(),
                });
                emit(&app);
            })
            .ok();
    }

    /// 同一个浮窗句柄 10 秒内只算一条；同一来源 1.5 秒内合并（浮窗换句柄重建）。
    fn recently_seen(hwnd: HWND, kind: &'static str) -> bool {
        let now = Instant::now();
        let key = hwnd_key(hwnd);
        if let Ok(mut seen) = seen_slot().lock() {
            if let Some(prev) = seen.get(&key) {
                if now.duration_since(*prev).as_millis() < POPUP_SEEN_MS {
                    return true;
                }
            }
            seen.insert(key, now);
            if seen.len() > 64 {
                seen.retain(|_, t| now.duration_since(*t).as_millis() < 60_000);
            }
        }
        if let Ok(mut last) = last_slot().lock() {
            if let Some(prev) = last.get(kind) {
                if now.duration_since(*prev).as_millis() < POPUP_DEDUPE_MS {
                    return true;
                }
            }
            last.insert(kind, now);
        }
        false
    }

    /// 读浮窗里的文字：返回 (发送人, 正文)，读不到给 ("", "")。
    fn read_text(hwnd: HWND) -> Option<(String, String)> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            let result = read_text_inner(hwnd);
            if hr.is_ok() {
                CoUninitialize();
            }
            result
        }
    }

    unsafe fn read_text_inner(hwnd: HWND) -> Option<(String, String)> {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL).ok()?;
        let walker = uia.RawViewWalker().ok()?;
        let mut best: Vec<String> = Vec::new();
        // 正文有时比发送人晚一两帧才画出来，多试几次；读不到不影响提醒本身。
        for _ in 0..POPUP_TEXT_TRIES {
            thread::sleep(Duration::from_millis(POPUP_TEXT_DELAY_MS));
            let Ok(root) = uia.ElementFromHandle(hwnd) else {
                break; // 浮窗已经关掉
            };
            let mut texts = Vec::new();
            collect_text(&root, 0, &walker, &mut texts);
            texts.retain(|t| {
                let t = t.trim();
                !t.is_empty() && t != "[Generating...]" && t != "Generating..."
            });
            texts.truncate(3);
            if texts.len() > best.len() {
                best = texts;
            }
            if best.len() >= 2 {
                break; // 发送人 + 正文都拿到了
            }
        }
        let sender: String = best.first()?.trim().chars().take(40).collect();
        if sender.is_empty() {
            return None;
        }
        let body: String = best
            .get(1)
            .map(|b| b.trim().chars().take(120).collect())
            .unwrap_or_default();
        Some((sender, body))
    }

    unsafe fn collect_text(
        el: &IUIAutomationElement,
        depth: usize,
        walker: &IUIAutomationTreeWalker,
        out: &mut Vec<String>,
    ) {
        if depth > 6 || out.len() >= 3 {
            return;
        }
        if let Ok(name) = el.CurrentName() {
            let name = name.to_string();
            let is_text = el
                .CurrentControlType()
                .map(|t| t.0 == UIA_TextControlTypeId.0)
                .unwrap_or(false);
            let has_size = el
                .CurrentBoundingRectangle()
                .map(|r| r.right - r.left > 0)
                .unwrap_or(false);
            if !name.trim().is_empty() && is_text && has_size {
                out.push(name.chars().take(200).collect());
            }
        }
        let Ok(first) = walker.GetFirstChildElement(el) else {
            return;
        };
        let mut child = first;
        while out.len() < 3 {
            collect_text(&child, depth + 1, walker, out);
            let Ok(next) = walker.GetNextSiblingElement(&child) else {
                break;
            };
            child = next;
        }
    }

    /// 点消息：先找已运行的来源窗口并切前台，没找到再按常见安装路径启动。
    pub fn open_app(kind: &str, exe: Option<&str>) -> Result<(), String> {
        let candidates = candidates_for(kind, exe);
        if candidates.is_empty() {
            return Err(format!("未知的消息来源：{kind}"));
        }
        if focus_running(&candidates) {
            return Ok(());
        }
        let hint = exe
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty());
        for path in known_paths(kind, hint.as_deref()) {
            // 指定了来源 exe 时只启动同一个应用，避免「找不到 Outlook 就打开 Thunderbird」。
            if let Some(hint) = &hint {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if &name != hint {
                    continue;
                }
            }
            if open::that(&path).is_ok() {
                return Ok(());
            }
        }
        if kind == "mail" {
            for proto in ["outlook:", "mailto:"] {
                if open::that(proto).is_ok() {
                    return Ok(());
                }
            }
        }
        for candidate in &candidates {
            if open::that(candidate).is_ok() {
                return Ok(());
            }
        }
        Err(format!(
            "没找到可打开的应用：{}（可先手动打开一次再试）",
            candidates.join(" / ")
        ))
    }

    fn candidates_for(kind: &str, exe: Option<&str>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(exe) = exe {
            let exe = exe.trim().to_ascii_lowercase();
            if !exe.is_empty() {
                out.push(exe);
            }
        }
        let defaults: &[&str] = match kind {
            "qq" => &["qq.exe", "qqnt.exe"],
            "wechat" => &["weixin.exe", "wechat.exe"],
            "feishu" => &["feishu.exe", "lark.exe"],
            "dingtalk" => &["dingtalk.exe"],
            "mail" => &["thunderbird.exe", "olk.exe", "outlook.exe", "hxoutlook.exe"],
            _ => &[],
        };
        for item in defaults {
            if !out.iter().any(|existing| existing == item) {
                out.push((*item).to_string());
            }
        }
        out
    }

    fn known_paths(kind: &str, exe_hint: Option<&str>) -> Vec<PathBuf> {
        let specs: Vec<(&str, &[&str])> = match kind {
            "qq" => vec![
                ("ProgramFiles", &["Tencent", "QQNT", "QQ.exe"][..]),
                ("ProgramFiles(x86)", &["Tencent", "QQNT", "QQ.exe"][..]),
                ("ProgramFiles", &["Tencent", "QQ", "Bin", "QQ.exe"][..]),
                ("ProgramFiles(x86)", &["Tencent", "QQ", "Bin", "QQ.exe"][..]),
                ("LocalAppData", &["Tencent", "QQNT", "QQ.exe"][..]),
            ],
            "wechat" => vec![
                ("ProgramFiles", &["Tencent", "Weixin", "Weixin.exe"][..]),
                ("ProgramFiles(x86)", &["Tencent", "Weixin", "Weixin.exe"][..]),
                ("ProgramFiles", &["Tencent", "WeChat", "WeChat.exe"][..]),
                ("ProgramFiles(x86)", &["Tencent", "WeChat", "WeChat.exe"][..]),
                ("LocalAppData", &["Tencent", "Weixin", "Weixin.exe"][..]),
            ],
            "feishu" => vec![
                ("ProgramFiles", &["ByteDance", "Feishu", "Feishu.exe"][..]),
                ("ProgramFiles(x86)", &["ByteDance", "Feishu", "Feishu.exe"][..]),
                ("LocalAppData", &["Feishu", "Feishu.exe"][..]),
                ("LocalAppData", &["ByteDance", "Feishu", "Feishu.exe"][..]),
            ],
            "dingtalk" => vec![
                ("ProgramFiles", &["DingTalk", "DingTalk.exe"][..]),
                ("ProgramFiles(x86)", &["DingTalk", "DingTalk.exe"][..]),
                ("LocalAppData", &["DingTalk", "DingTalk.exe"][..]),
            ],
            "mail" => vec![
                ("LocalAppData", &["Microsoft", "WindowsApps", "olk.exe"][..]),
                ("ProgramFiles", &["Microsoft Office", "root", "Office16", "OUTLOOK.EXE"][..]),
                (
                    "ProgramFiles(x86)",
                    &["Microsoft Office", "root", "Office16", "OUTLOOK.EXE"][..],
                ),
                ("ProgramFiles", &["Mozilla Thunderbird", "thunderbird.exe"][..]),
                (
                    "ProgramFiles(x86)",
                    &["Mozilla Thunderbird", "thunderbird.exe"][..],
                ),
            ],
            _ => Vec::new(),
        };
        let mut out: Vec<PathBuf> = specs
            .iter()
            .filter_map(|(root, parts)| env_path(root, parts))
            .filter(|path| path.is_file())
            .collect();
        if let Some(hint) = exe_hint {
            let hint = hint.to_ascii_lowercase();
            out.sort_by_key(|path| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if name == hint {
                    0
                } else {
                    1
                }
            });
        }
        out
    }

    fn env_path(root: &str, parts: &[&str]) -> Option<PathBuf> {
        let base = std::env::var_os(root)?;
        let mut path = PathBuf::from(base);
        for part in parts {
            path.push(part);
        }
        Some(path)
    }

    struct WindowList {
        hwnds: Vec<HWND>,
    }

    unsafe extern "system" fn enum_all(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = unsafe { &mut *(lparam.0 as *mut WindowList) };
        state.hwnds.push(hwnd);
        TRUE
    }

    fn all_windows() -> Vec<HWND> {
        let mut state = WindowList { hwnds: Vec::new() };
        unsafe {
            let _ = EnumWindows(
                Some(enum_all),
                LPARAM(&mut state as *mut WindowList as *mut c_void as isize),
            );
        }
        state.hwnds
    }

    fn focus_running(candidates: &[String]) -> bool {
        let mut best: Option<(HWND, i64)> = None;
        let mut fallback: Option<HWND> = None;
        for hwnd in all_windows() {
            unsafe {
                // 来源窗口都有标题；没有标题的辅助窗口 / 浮窗本身跳过。
                if GetWindowTextLengthW(hwnd) <= 0 {
                    continue;
                }
                let Some(exe) = exe_name(hwnd) else {
                    continue;
                };
                if !candidates.iter().any(|c| c == &exe) {
                    continue;
                }
                fallback.get_or_insert(hwnd);
                // 同类窗口挑面积最大的可见窗口，避开提示条 / 隐藏助手窗口。
                let mut rect = RECT::default();
                if GetWindowRect(hwnd, &mut rect).is_ok() {
                    let area = (rect.right - rect.left) as i64 * (rect.bottom - rect.top) as i64;
                    if IsWindowVisible(hwnd).as_bool() && area > 0 {
                        if best.map(|(_, a)| area > a).unwrap_or(true) {
                            best = Some((hwnd, area));
                        }
                    }
                }
            }
        }
        match best.map(|(hwnd, _)| hwnd).or(fallback) {
            Some(hwnd) => {
                unsafe { activate(hwnd) };
                true
            }
            None => false,
        }
    }

    unsafe fn activate(hwnd: HWND) {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = BringWindowToTop(hwnd);
        // 从非前台进程调 SetForegroundWindow 常被系统拒绝，先把自己挂到
        // 当前前台线程的输入队列上再切。
        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let current_thread = GetCurrentThreadId();
        let attached = foreground_thread != 0
            && foreground_thread != current_thread
            && AttachThreadInput(current_thread, foreground_thread, true).as_bool();
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn 右下角浮窗几何判断() {
            // 飞书浮窗 360x80，贴在工作区右下角 16px 处
            assert!(popup_geometry_ok(360, 80, 984, 784, 1000, 800));
            // 主窗口：尺寸超范围
            assert!(!popup_geometry_ok(1200, 800, 1000, 800, 1000, 800));
            // 左上角弹窗：不在右下角附近
            assert!(!popup_geometry_ok(360, 80, 400, 200, 1000, 800));
            // 太扁 / 太窄
            assert!(!popup_geometry_ok(120, 80, 984, 784, 1000, 800));
            assert!(!popup_geometry_ok(360, 24, 984, 784, 1000, 800));
            // 允许 8px 的工作区外溢
            assert!(popup_geometry_ok(360, 80, 1006, 806, 1000, 800));
        }

        #[test]
        fn 浮窗进程映射到消息类型() {
            assert_eq!(popup_kind("feishu.exe"), ("feishu", "飞书"));
            assert_eq!(popup_kind("lark.exe"), ("feishu", "飞书"));
            assert_eq!(popup_kind("dingtalk.exe"), ("dingtalk", "钉钉"));
        }

        #[test]
        fn 点消息的应用候选顺序() {
            assert_eq!(
                candidates_for("feishu", Some("lark.exe")),
                vec!["lark.exe", "feishu.exe"]
            );
            assert_eq!(candidates_for("mail", None)[0], "thunderbird.exe");
            assert!(candidates_for("unknown", None).is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_xml_按顺序取文本节点() {
        let payload = r#"<toast><visual><binding template="ToastText02"><text id="1">张三</text><text id="2">在吗？</text></binding></visual></toast>"#;
        assert_eq!(toast_texts(payload.as_bytes()), vec!["张三", "在吗？"]);
    }

    #[test]
    fn 通知应用标识映射到消息类型() {
        assert_eq!(map_primary("QQ"), Some(("qq", "QQ")));
        assert_eq!(map_primary("Weixin"), Some(("wechat", "微信")));
        assert_eq!(map_primary("electron.app.Feishu"), Some(("feishu", "飞书")));
        assert_eq!(map_primary("com.alibaba.dingtalk"), Some(("dingtalk", "钉钉")));
        assert_eq!(
            map_primary("Microsoft.OutlookForWindows"),
            Some(("mail", "邮件"))
        );
        // 音乐类通知不能算成 QQ 消息
        assert_eq!(map_primary("CloudMusic"), None);
        assert_eq!(map_primary("QQMusic"), None);
    }

    #[test]
    fn 通知来源映射到可执行文件() {
        assert_eq!(exe_for_primary("electron.app.Feishu"), "feishu.exe");
        assert_eq!(exe_for_primary("com.alibaba.dingtalk"), "dingtalk.exe");
        assert_eq!(
            exe_for_primary("microsoft.windowscommunicationsapps_8wekyb3d8bbwe!microsoft.windowslive.mail"),
            "hxoutlook.exe"
        );
        assert_eq!(exe_for_primary("ThunderbirdToast"), "thunderbird.exe");
        assert_eq!(exe_for_primary("Microsoft.OutlookForWindows"), "olk.exe");
        assert_eq!(exe_for_primary("未知应用"), "");
    }
}
