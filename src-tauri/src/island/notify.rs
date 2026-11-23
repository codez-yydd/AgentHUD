//! 灵动岛消息：只读轮询 Windows 通知数据库 `wpndatabase.db`，
//! 并识别飞书 / 钉钉右下角自绘的通知浮窗。
//!
//! 通知库部分采用只读轮询：不申请通知监听权限、不读聊天内容存储，
//! 只把系统通知里的 toast XML 文本按应用映射成 QQ / 微信 / 邮件 / 飞书 /
//! 钉钉 五类。首次启动读最近 50 条作为历史（标记已读），之后新增的条目
//! 计入未读数并广播 `zbar://island-notify`。
//!
//! 两个配置开关（`IslandConfig.notify_modules` / `notify_body_preview`）：
//! - `notifyModules`：消息页启用的模块。未启用的 kind 直接丢弃（toast 增量、
//!   初始历史、飞书/钉钉浮窗都一样），不进列表、不计未读；读取/广播/清理等
//!   视图路径也按当前配置过滤已有条目。空数组 = 一个都不显示。
//! - `notifyBodyPreview`：额外读取正文预览的模块。toast 的 sender/body 文本
//!   规则、飞书/钉钉浮窗的 UI Automation 读取都以它为准（见 `toast_item_texts`
//!   与 `win::handle_popup`）；空数组 = 不读取正文。
//! 两者都每拍/每个浮窗事件重读配置缓存（`island_set_config` 后立即生效），
//! 只影响「是否投递/是否读正文」，不改 items 的字段结构。
//!
//! 飞书 / 钉钉默认不写 Windows 系统通知、也不闪任务栏，消息到达时只会创建
//! 一个贴在工作区右下角的小浮窗（无边框 + 置顶 + 工具窗口）。这里每秒
//! `EnumWindows` 找一次这种窗口，发送人 / 正文用 UI Automation 读（读不到
//! 就只提醒「新消息」），条目同样进消息列表。`island_notify_open` 供消息页
//! 点按：优先聚焦已运行的来源应用，没运行才按常见安装路径启动。
//!
//! 任务栏闪烁（B 路）：独立线程建一个**隐藏的顶层窗口**，用
//! `RegisterShellHookWindow` + `RegisterWindowMessageW("SHELLHOOK")` 接收
//! `HSHELL_FLASH`；按闪烁窗口的进程名映射 kind（qq/wechat/feishu/dingtalk/mail），
//! 同 kind 8 秒内去重，再走与 toast 完全相同的 `push_item`/`emit` 与
//! `notifyModules` 过滤链路（id 用负数合成，sender/body 为空）。
//! DeepSeek 桌面版会话日志和 IMAP 收件箱也通过同一条消息链路投递。

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

mod deepseek;
pub(super) mod mail;

pub const NOTIFY_EVENT: &str = "zbar://island-notify";
const HISTORY_MAX: usize = 50;
const _FILETIME_DIFF: i64 = 116_444_736_000_000_000;
const POLL_MS: u64 = 2000;

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static INBOX: Mutex<NotifyInbox> = Mutex::new(NotifyInbox {
    items: Vec::new(),
    wechat_unread: 0,
    latest_wechat: None,
});
static EMIT_LOCK: Mutex<()> = Mutex::new(());
static MAX_ID: AtomicI64 = AtomicI64::new(-1);
// 浮窗、任务栏、IMAP 和会话日志共用负数 ID，避免与通知数据库或彼此冲突。
static SYNTH_ID: AtomicI64 = AtomicI64::new(-1);

fn app_slot() -> &'static Mutex<Option<AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
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
    /// 邮件验证码（由运行时填充；空串表示没有验证码）。只在前端 chip /
    /// 收起态展示与「点击复制」，不参与去重、未读或打开应用。
    pub code: String,
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
    pub wechat_unread: usize,
}

#[derive(Default)]
struct NotifyInbox {
    items: Vec<NotifyItem>,
    wechat_unread: usize,
    latest_wechat: Option<NotifyItem>,
}

impl NotifyInbox {
    fn push(&mut self, item: NotifyItem) {
        if item.kind == "wechat" {
            if item.read {
                return;
            }
            self.wechat_unread = self.wechat_unread.saturating_add(1);
            self.latest_wechat = Some(item.clone());
        }
        self.items.insert(0, item);
        self.items.truncate(HISTORY_MAX);
    }

    fn mark_read(&mut self, kind: Option<&str>) {
        self.items.retain_mut(|item| {
            if kind.is_some_and(|kind| kind != item.kind) {
                return true;
            }
            item.read = true;
            item.kind != "wechat"
        });
        self.clear_wechat_unread(kind);
    }

    fn clear(&mut self, kind: Option<&str>) {
        self.items.retain(|item| kind.is_some_and(|kind| kind != item.kind));
        self.clear_wechat_unread(kind);
    }

    fn clear_wechat_unread(&mut self, kind: Option<&str>) {
        if kind.is_none_or(|kind| kind == "wechat") {
            self.wechat_unread = 0;
            self.latest_wechat = None;
        }
    }

    fn snapshot(&self, prefs: &NotifyPrefs) -> NotifyState {
        let mut items = visible_items(&self.items, prefs);
        let wechat_unread = if prefs.module_enabled("wechat") {
            if !items.iter().any(|item| item.kind == "wechat") {
                items.extend(self.latest_wechat.iter().cloned());
            }
            self.wechat_unread
        } else {
            0
        };
        let unread = items
            .iter()
            .filter(|item| !item.read && item.kind != "wechat")
            .count();
        NotifyState {
            items,
            unread: unread.saturating_add(wechat_unread),
            wechat_unread,
        }
    }
}

/// `map_primary` 实际能产出的 kind 并集，也就是「Windows 通知库 toast」的
/// 全部来源。只有这些模块至少启用一个时，`poll_new` 才值得去读 SQLite。
///
/// 注意飞书 / 钉钉虽然主要走浮窗识别，但通知库里的 toast 同样会被
/// `map_primary` 映射成这两种 kind（见 `map_primary` 与
/// `通知应用标识映射到消息类型` 单测），所以不能只算 qq / wechat / mail。
const TOAST_SOURCE_KINDS: [&str; 5] = ["qq", "wechat", "feishu", "dingtalk", "mail"];

/// 消息模块开关快照（`notifyModules` / `notifyBodyPreview`）。
///
/// 轮询每拍、浮窗事件各读一次 `load_config()`（进程内缓存，不会重建整库）；
/// 逐条通知只查这个快照，避免每条都 clone 一次配置。
struct NotifyPrefs {
    modules: Vec<String>,
    body_preview: Vec<String>,
}

impl NotifyPrefs {
    fn from_config() -> Self {
        let cfg = super::load_config();
        Self {
            modules: cfg.notify_modules,
            body_preview: cfg.notify_body_preview,
        }
    }
    fn module_enabled(&self, kind: &str) -> bool {
        self.modules.iter().any(|enabled| enabled == kind)
    }
    fn body_preview(&self, kind: &str) -> bool {
        self.body_preview.iter().any(|enabled| enabled == kind)
    }
    /// 是否有任何「通知库 toast 来源」模块启用；没有就跳过 `load_rows`。
    fn toast_sources_enabled(&self) -> bool {
        TOAST_SOURCE_KINDS
            .iter()
            .any(|kind| self.module_enabled(kind))
    }
}

/// 按当前模块开关过滤要展示的条目：存储里只保留「投递时模块已启用」的条目，
/// 这里仅在视图/广播路径再按当前配置过滤（用于模块被关掉后立即隐藏其历史；
/// 重新开启时，仍在 50 条历史内的旧条目会重新出现——模块关闭期间新到的消息
/// 在投递入口 `push_item` 就被丢弃，不会补回）。
fn visible_items(items: &[NotifyItem], prefs: &NotifyPrefs) -> Vec<NotifyItem> {
    items
        .iter()
        .filter(|item| prefs.module_enabled(&item.kind))
        .cloned()
        .collect()
}

fn state_with(prefs: &NotifyPrefs) -> NotifyState {
    INBOX
        .lock()
        .map(|inbox| inbox.snapshot(prefs))
        .unwrap_or_default()
}

fn emit(app: &AppHandle) {
    let Ok(_guard) = EMIT_LOCK.lock() else {
        return;
    };
    let state = state_with(&NotifyPrefs::from_config());
    // 桌面玻璃浮窗：复用同一份最新状态挑「本次新到」的条目，攒够一个
    // 1.2s 波次再出卡（见 island::popup）。这里只读状态，不碰采集/去重。
    super::popup::on_notify_state(app, &state);
    let _ = app.emit(NOTIFY_EVENT, state);
}

/// 配置变更后立即按新开关重算视图并广播（`island_set_config` 调用），
/// 前端勾选模块磁贴后不用等下一拍轮询。
pub fn refresh() {
    mail::settings_changed();
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        emit(&app);
    }
}

pub(super) fn mail_settings_changed() {
    mail::settings_changed();
}

/// 入队一条新通知：未启用的 kind 直接丢弃（不进列表、不计未读）。
/// 返回是否真正入队。
fn push_item(item: NotifyItem, prefs: &NotifyPrefs) -> bool {
    if !prefs.module_enabled(&item.kind) {
        return false;
    }
    let Ok(mut inbox) = INBOX.lock() else {
        return false;
    };
    inbox.push(item);
    true
}

/// toast 文本规则：
/// - sender：仅 qq/wechat/feishu/dingtalk，且（有第二条 或 该 kind 未开正文
///   预览）时取 `texts[0]` 前 40 字；
/// - body：仅该 kind 开了正文预览时取 `texts[1]`（没有第二条退回 `texts[0]`）
///   前 120 字；
/// - 其余（含邮件默认）为空串。
fn toast_item_texts(kind: &str, texts: &[String], body_preview: &[String]) -> (String, String) {
    let preview = body_preview.iter().any(|enabled| enabled == kind);
    let sender = if texts.is_empty() {
        String::new()
    } else if matches!(kind, "qq" | "wechat" | "feishu" | "dingtalk")
        && (texts.len() > 1 || !preview)
    {
        texts[0].chars().take(40).collect()
    } else {
        String::new()
    };
    let body = if preview {
        texts
            .get(1)
            .or_else(|| texts.first())
            .map(|text| text.chars().take(120).collect())
            .unwrap_or_default()
    } else {
        String::new()
    };
    (sender, body)
}

// ============================================================
// 任务栏闪烁（HSHELL_FLASH）来源
// ============================================================

/// 任务栏闪烁（`FlashWindowEx`）的进程名 → (kind, 显示名)。
///
/// 映射如下（额外含 QQNT）：
/// - `qq.exe / qqnt.exe` → qq；`weixin.exe / wechat.exe` → wechat；
/// - `feishu.exe / lark.exe` → feishu；`dingtalk.exe` → dingtalk；
/// - `thunderbird.exe / outlook.exe / olk.exe / hxoutlook.exe` → mail。
///
/// 进程名统一小写比较，未知进程返回 None（不投递）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn flash_source(exe: &str) -> Option<(&'static str, &'static str)> {
    match exe.trim().to_ascii_lowercase().as_str() {
        "qq.exe" | "qqnt.exe" => Some(("qq", "QQ")),
        "weixin.exe" | "wechat.exe" => Some(("wechat", "微信")),
        "feishu.exe" | "lark.exe" => Some(("feishu", "飞书")),
        "dingtalk.exe" => Some(("dingtalk", "钉钉")),
        "thunderbird.exe" | "outlook.exe" | "olk.exe" | "hxoutlook.exe" => Some(("mail", "邮件")),
        _ => None,
    }
}

/// 同 kind 闪烁去重：距离上一次投递不足 `dedupe` 就丢弃（参考 8s）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn flash_allowed(since_last: Option<Duration>, dedupe: Duration) -> bool {
    since_last.map(|elapsed| elapsed >= dedupe).unwrap_or(true)
}

/// 构造一条闪烁来源的 NotifyItem；kind 未启用（notifyModules 过滤）返回 None。
///
/// 与 toast 共用 items 结构：sender/body 为空、`name` 为 kind 显示名、
/// `exe` 填真实进程名（点按消息时可聚焦/启动对应应用）、id 用负数合成。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn flash_item(exe: &str, prefs: &NotifyPrefs, id: i64, now_ms: i64) -> Option<NotifyItem> {
    let (kind, name) = flash_source(exe)?;
    if !prefs.module_enabled(kind) {
        return None;
    }
    Some(NotifyItem {
        id,
        kind: kind.to_string(),
        name: name.to_string(),
        sender: String::new(),
        body: String::new(),
        code: String::new(),
        time: now_ms,
        read: false,
        exe: exe.trim().to_ascii_lowercase(),
    })
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

fn rows_to_items(
    rows: Vec<(i64, String, i64, String, Vec<u8>)>,
    prefs: &NotifyPrefs,
) -> Vec<NotifyItem> {
    let mut out = Vec::new();
    for (id, primary, arrival, ntype, payload) in rows {
        if !ntype.is_empty() && ntype.to_ascii_lowercase() != "toast" {
            continue;
        }
        let Some((kind, name)) = map_primary(&primary) else {
            continue;
        };
        // 未启用的模块直接丢弃：不进 items、不计未读
        if !prefs.module_enabled(kind) {
            continue;
        }
        let texts = toast_texts(&payload);
        if texts.is_empty() {
            continue;
        }
        let (sender, body) = toast_item_texts(kind, &texts, &prefs.body_preview);
        // 系统通知来源的邮件（没有 IMAP）：验证码就在 toast 文本里，就地用同一套
        // 规则抽，不额外读取通知正文；其它来源不抽，避免误报。
        let code = if kind == "mail" {
            mail::extract_code(&texts.join(" ")).unwrap_or_default()
        } else {
            String::new()
        };
        out.push(NotifyItem {
            id,
            kind: kind.to_string(),
            name: name.to_string(),
            sender,
            body,
            code,
            time: filetime_to_ms(arrival),
            read: false,
            exe: exe_for_primary(&primary).to_string(),
        });
    }
    out
}

fn init_history(app: &AppHandle, gen: u64) {
    let prefs = NotifyPrefs::from_config();
    // MAX_ID 按**全部**通知推进（不管模块开关），关掉模块期间的历史通知不会
    // 在重新打开时被当成新消息补进来。
    let rows = load_rows(HISTORY_MAX as i64);
    if !running(gen) {
        return;
    }
    // 干净通知库也建立游标和历史，清除上一次监听留下的合成条目。
    let max_id = rows.iter().map(|r| r.0).max().unwrap_or(0);
    MAX_ID.store(max_id, Ordering::SeqCst);
    let mut inbox = NotifyInbox {
        items: rows_to_items(rows, &prefs),
        ..NotifyInbox::default()
    };
    inbox.mark_read(None);
    if let Ok(mut guard) = INBOX.lock() {
        *guard = inbox;
    }
    emit(app);
}

fn poll_new(app: &AppHandle, gen: u64) {
    // 每拍读一次最新模块开关（配置热更新，无需重启监听线程）。
    let prefs = NotifyPrefs::from_config();
    // 通知库门控：所有 toast 来源模块都关掉时不碰 SQLite（连历史初始化也
    // 推迟——否则那一次 load_rows 仍会每拍发生）。重新启用后下一拍自然恢复，
    // 首次恢复时 init_history 照旧把最近 50 条当历史标已读，不会补发。
    if !prefs.toast_sources_enabled() {
        return;
    }
    let last = MAX_ID.load(Ordering::SeqCst);
    if last < 0 {
        init_history(app, gen);
        return;
    }
    let rows = load_rows(120);
    if rows.is_empty() || !running(gen) {
        return;
    }
    let new_rows: Vec<_> = rows.into_iter().filter(|r| r.0 > last).collect();
    if new_rows.is_empty() {
        return;
    }
    // 同样按全部通知推进游标，未启用模块的通知不会反复重扫。
    let max_id = new_rows.iter().map(|r| r.0).max().unwrap_or(last);
    MAX_ID.store(max_id, Ordering::SeqCst);
    let mut fresh = rows_to_items(new_rows, &prefs);
    fresh.sort_by_key(|i| i.id);
    if fresh.is_empty() {
        return;
    }
    for item in fresh {
        push_item(item, &prefs);
    }
    emit(app);
}

/// 启动消息服务：读一次通知库最近 50 条作为历史（全部标已读），之后轮询增量。
pub fn start(app: AppHandle) {
    start_inner(app, true);
}

/// 板块从隐藏→重新显示时拉起消息服务：保留内存里的 items / MAX_ID /
/// 浮窗基线，只把 `stop` 停掉的线程重新拉起。
///
/// **不能**再走 `init_history`——它会把列表整体替换成全已读，未读徽标凭空清零。
pub fn resume(app: AppHandle) {
    start_inner(app, false);
}

fn start_inner(app: AppHandle, reset_history: bool) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(app.clone());
    }
    // 浮窗差分基线跟着一起重置：关岛期间进库的消息会被下面 init_history
    // 重新当历史读入，基线留着会让它们被误判成新消息（或漏判）。
    // resume 不重置：items / MAX_ID / 基线都保留，接着按增量 poll_new。
    if reset_history {
        super::popup::reset_baseline();
    }
    STOP.store(false, Ordering::SeqCst);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let popup_app = app.clone();
    let flash_app = app.clone();
    thread::Builder::new()
        .name("zbar-island-notify".into())
        .spawn(move || {
            if !running(gen) {
                return;
            }
            // 从未初始化过（MAX_ID = -1，比如冷启动直接走 resume）时仍要读一次
            // 历史建立游标，否则第一条消息会被当成新消息补进来。
            if reset_history || MAX_ID.load(Ordering::SeqCst) < 0 {
                init_history(&app, gen);
                if !running(gen) {
                    return;
                }
            }
            mail::start(app.clone(), gen);
            start_deepseek_watch(app.clone(), gen);
            while running(gen) {
                poll_new(&app, gen);
                thread::sleep(Duration::from_millis(POLL_MS));
            }
        })
        .ok();
    start_popup_watch(popup_app, gen);
    // 任务栏闪烁（HSHELL_FLASH）：独立线程 + 隐藏窗口收 shell hook 消息
    start_flash_watch(flash_app, gen);
}

fn start_deepseek_watch(app: AppHandle, gen: u64) {
    thread::Builder::new()
        .name("zbar-island-deepseek".into())
        .spawn(move || {
            let mut watcher = deepseek::Watcher::default();
            // 上一拍是否启用：关闭后只复位一次 Watcher，之后不再读配置/扫日志。
            let mut was_enabled = false;
            while running(gen) {
                let enabled = NotifyPrefs::from_config().module_enabled("dsh");
                if !enabled {
                    // 与旧实现（每拍 poll(&DshDoc::default(), false)）等价的
                    // 状态复位：把 root 置回 None、清空游标与已投递表，避免重新
                    // 启用时把关闭期间的会话完成当新消息补发。只在翻转那一拍做，
                    // 之后纯睡 POLL_MS，不读 settings.dsh、不扫会话目录。
                    if was_enabled {
                        watcher.poll(&super::settings::DshDoc::default(), false);
                        was_enabled = false;
                    }
                    thread::sleep(Duration::from_millis(POLL_MS));
                    continue;
                }
                was_enabled = true;
                let config = super::settings::load().dsh;
                let completions = watcher.poll(&config, true);
                if !completions.is_empty() && running(gen) {
                    let prefs = NotifyPrefs::from_config();
                    let mut changed = false;
                    for completion in completions {
                        changed |= push_item(
                            NotifyItem {
                                id: SYNTH_ID.fetch_sub(1, Ordering::Relaxed),
                                kind: "dsh".into(),
                                name: "DeepSeek".into(),
                                sender: completion.title,
                                body: completion.body,
                                code: String::new(),
                                time: completion.time,
                                read: false,
                                exe: deepseek::EXE.into(),
                            },
                            &prefs,
                        );
                    }
                    if changed && running(gen) {
                        emit(&app);
                    }
                }
                thread::sleep(Duration::from_millis(POLL_MS));
            }
        })
        .ok();
}

pub(super) fn deepseek_popup_suppressed() -> bool {
    #[cfg(windows)]
    {
        win::deepseek_foreground() && super::settings::load().dsh.skip_when_active
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(target_os = "windows")]
fn start_flash_watch(app: AppHandle, gen: u64) {
    win::start_flash_watch(app, gen);
}

#[cfg(not(target_os = "windows"))]
fn start_flash_watch(_app: AppHandle, _gen: u64) {}

#[cfg(target_os = "windows")]
fn stop_flash_watch() {
    win::stop_flash_watch();
}

#[cfg(not(target_os = "windows"))]
fn stop_flash_watch() {}

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
    mail::settings_changed();
    // 关岛即作废浮窗的未出卡波次 / 暂存 / 基线，避免关岛后还冒出一批旧卡
    super::popup::reset_baseline();
    // 注销任务栏闪烁的 shell hook（窗口销毁 + 线程退出）
    stop_flash_watch();
}

/// 当前可见消息列表（按 `notifyModules` 过滤）+ 未读数。
#[tauri::command]
pub fn island_notify() -> NotifyState {
    state_with(&NotifyPrefs::from_config())
}

#[tauri::command]
pub fn island_notify_mark_read() -> NotifyState {
    if let Ok(mut inbox) = INBOX.lock() {
        inbox.mark_read(None);
    }
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        emit(&app);
    }
    island_notify()
}

/// 只把某一个模块（kind）的消息标记为已读。
#[tauri::command]
pub fn island_notify_mark_read_kind(kind: String) -> NotifyState {
    let kind = kind.trim().to_ascii_lowercase();
    if !kind.is_empty() {
        mark_kind_read(&kind);
    }
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        emit(&app);
    }
    island_notify()
}

/// 清空消息：`kind` 有值 = 只清该模块，None = 全清。清完广播最新 state。
#[tauri::command]
pub fn island_notify_clear(kind: Option<String>) -> NotifyState {
    let target = kind
        .as_deref()
        .map(|kind| kind.trim().to_ascii_lowercase())
        .filter(|kind| !kind.is_empty());
    if let Ok(mut inbox) = INBOX.lock() {
        inbox.clear(target.as_deref());
    }
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        emit(&app);
    }
    island_notify()
}

/// 当前消息模块开关快照，供消息页渲染模块磁贴（等价于读
/// `island_config().notifyModules` / `.notifyBodyPreview`，这里给单次调用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyModules {
    pub modules: Vec<String>,
    pub body_preview: Vec<String>,
}

#[tauri::command]
pub fn island_notify_modules() -> NotifyModules {
    let cfg = super::load_config();
    NotifyModules {
        modules: cfg.notify_modules,
        body_preview: cfg.notify_body_preview,
    }
}

fn mark_kind_read(kind: &str) {
    if let Ok(mut inbox) = INBOX.lock() {
        inbox.mark_read(Some(kind));
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

/// 展开路径里的 `%VAR%`（变量名大小写敏感，Windows 环境变量普遍大写写法）。
/// 未定义的变量原样保留，用户能一眼看出自己写错了哪个变量。
#[cfg(any(target_os = "windows", test))]
pub(super) fn expand_env_vars<F>(raw: &str, lookup: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        match tail.find('%') {
            Some(end) => {
                let name = &tail[..end];
                let value = if name.trim().is_empty() {
                    None
                } else {
                    lookup(name)
                };
                match value {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &tail[end + 1..];
            }
            None => {
                // 只有一个 % ：不是占位符，原样输出剩余部分
                out.push('%');
                out.push_str(tail);
                return out;
            }
        }
    }
    out.push_str(&rest);
    out
}

/// 设置窗「打开应用」里给该 kind 配的可执行文件路径：trim 掉空白与首尾引号
/// （从资源管理器复制路径时常见 `"C:\...\QQ.exe"`），再展开 `%VAR%`。
/// `None` = 没配，调用方回退内置逻辑。
#[cfg(target_os = "windows")]
fn configured_open_app(kind: &str) -> Option<String> {
    let configured = super::load_config().notify_open.get(kind).cloned()?;
    let raw = configured.trim().trim_matches('"').trim();
    if raw.is_empty() {
        return None;
    }
    let expanded = expand_env_vars(raw, |name| std::env::var(name).ok());
    let expanded = expanded.trim().to_string();
    (!expanded.is_empty()).then_some(expanded)
}

#[cfg(target_os = "windows")]
pub(super) fn focus_running_app(exe: &str) -> bool {
    win::focus_running(&[exe.to_owned()])
}

/// 启动自动探测：「打开应用」里该项为空时用来填空（见
/// `island::settings::seed_detected_paths`）。先复用内置常见安装路径，再按
/// 运行进程 / 卸载注册表 / 常见目录探测。非 Windows / 探测失败都返回 None。
#[cfg(target_os = "windows")]
pub(super) fn detect_open_path(kind: &str) -> Option<String> {
    win::detect_open_path(kind)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn detect_open_path(kind: &str) -> Option<String> {
    let _ = kind;
    None
}

#[cfg(target_os = "windows")]
mod win {
    //! 飞书 / 钉钉通知浮窗的识别、正文读取，以及「点消息打开应用」。

    use super::{emit, flash_allowed, push_item, running, NotifyItem, NotifyPrefs, SYNTH_ID};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use windows::core::{BOOL, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, RECT, TRUE, WPARAM,
    };
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, HBRUSH, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{
        AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
        UIA_TextControlTypeId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, CreateWindowExW, DefWindowProcW, DeregisterShellHookWindow,
        DestroyWindow, DispatchMessageW, EnumWindows, GetClassNameW, GetForegroundWindow,
        GetMessageW, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, PostMessageW, PostQuitMessage, RegisterClassW,
        RegisterShellHookWindow, RegisterWindowMessageW, SetForegroundWindow, ShowWindow,
        TranslateMessage, GWL_EXSTYLE, GWL_STYLE, HCURSOR, HICON, MSG, SW_RESTORE, WINDOW_EX_STYLE,
        WM_CLOSE, WM_DESTROY, WNDCLASSW, WNDCLASS_STYLES, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_POPUP,
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
                // 启动基线推迟到第一次「确实要扫描」时：飞书/钉钉都关着时
                // 不跑 EnumWindows；等第一次启用时再记下当时已在屏上的旧浮窗，
                // 语义与旧实现「启动先扫一遍避免把旧消息当新消息」一致。
                let mut baselined = false;
                while running(gen) {
                    let prefs = NotifyPrefs::from_config();
                    // 飞书（含 lark）与钉钉都没启用：没有要识别的浮窗来源，
                    // 跳过每秒一次的全窗口枚举（配置热更新下一拍生效）。
                    if !prefs.module_enabled("feishu") && !prefs.module_enabled("dingtalk") {
                        thread::sleep(Duration::from_millis(POPUP_POLL_MS));
                        continue;
                    }
                    if !baselined {
                        // 启动时先记下已经存在的浮窗，避免把启动前的旧消息当成新消息。
                        let now = Instant::now();
                        if let Ok(mut seen) = seen_slot().lock() {
                            for (hwnd, _) in scan() {
                                seen.insert(hwnd_key(hwnd), now);
                            }
                        }
                        baselined = true;
                    }
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
        if style & WS_POPUP.0 == 0 || ex & WS_EX_TOOLWINDOW.0 == 0 || ex & WS_EX_TOPMOST.0 == 0 {
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
        let exe = exe_of_hwnd(hwnd)?;
        let app = POPUP_APPS.iter().copied().find(|a| *a == exe.as_str())?;
        // 飞书的其它弹窗（更新 / 设置）通常有标题；Electron 顶层窗口类固定是 Chrome_WidgetWin_*。
        if matches!(app, "feishu.exe" | "lark.exe")
            && (!class_is_chromium(hwnd) || !title_is_empty(hwnd))
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
    unsafe fn exe_of_hwnd(hwnd: HWND) -> Option<String> {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
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
        n <= 0
            || String::from_utf16_lossy(&buf[..n as usize])
                .trim()
                .is_empty()
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
        // 每个浮窗事件读一次最新模块开关：配置热更新即时生效。
        let prefs = NotifyPrefs::from_config();
        if !prefs.module_enabled(kind) {
            return; // 模块未启用：不进 items、不计未读
        }
        if recently_seen(hwnd, kind) {
            return;
        }
        // 未开启正文预览：不做 UIA 读取，直接出一条「新消息」（sender/body 空）。
        if !prefs.body_preview(kind) {
            let new_item = NotifyItem {
                id: SYNTH_ID.fetch_sub(1, Ordering::SeqCst),
                kind: kind.to_string(),
                name: name.to_string(),
                sender: String::new(),
                body: String::new(),
                code: String::new(),
                time: chrono::Utc::now().timestamp_millis(),
                read: false,
                exe: exe.to_string(),
            };
            if running(gen) && push_item(new_item, &prefs) {
                emit(app);
            }
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
                // 读取期间配置可能被改：再确认一次模块开关（关掉就丢弃）。
                let prefs = NotifyPrefs::from_config();
                let new_item = NotifyItem {
                    id: SYNTH_ID.fetch_sub(1, Ordering::SeqCst),
                    kind: kind.to_string(),
                    name: name.to_string(),
                    sender,
                    body,
                    code: String::new(),
                    time: chrono::Utc::now().timestamp_millis(),
                    read: false,
                    exe: exe.to_string(),
                };
                if push_item(new_item, &prefs) {
                    emit(&app);
                }
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

    // ============================================================
    // 任务栏闪烁：隐藏顶层窗口 + shell hook 消息回路
    // ============================================================

    /// `HSHELL_FLASH`：Windows 在任务栏闪烁/请求注意时发给 shell hook 窗口的
    /// 事件码（windows crate 未导出该常量，按 WinUser.h 取 0x8006）。
    const HSHELL_FLASH: u32 = 0x8006;
    /// 同 kind 闪烁 8s 内只投递一次（参考 `FLASH_DEDUPE_S = 8.0`）。
    const FLASH_DEDUPE_MS: u64 = 8_000;

    static FLASH_HWND: AtomicIsize = AtomicIsize::new(0);
    static FLASH_GEN: AtomicU64 = AtomicU64::new(0);
    static FLASH_MSG: AtomicU32 = AtomicU32::new(0);
    static FLASH_APP: OnceLock<super::AppHandle> = OnceLock::new();
    /// 类名要在进程生命周期内保持有效（RegisterClass 不保证复制字符串）。
    static FLASH_CLASS: OnceLock<Box<[u16]>> = OnceLock::new();

    fn hwnd_of(raw: isize) -> HWND {
        HWND(raw as *mut c_void)
    }

    /// UTF-16 + NUL。
    fn wide_null(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn flash_class_name() -> PCWSTR {
        let wide = FLASH_CLASS.get_or_init(|| wide_null("AgentHUDFlashWatch").into_boxed_slice());
        PCWSTR::from_raw(wide.as_ptr())
    }

    fn flash_last_slot() -> &'static Mutex<HashMap<String, Instant>> {
        static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
        LAST.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// 启动闪烁监听：独立线程建隐藏顶层窗口 → 注册 `SHELLHOOK` → `GetMessage` 循环。
    pub fn start_flash_watch(app: super::AppHandle, gen: u64) {
        // 上一代窗口先关掉（stop 已关过；这里兜底避免重复注册）。
        stop_flash_watch();
        let _ = FLASH_APP.set(app);
        FLASH_GEN.store(gen, Ordering::SeqCst);
        let _ = thread::Builder::new()
            .name("zbar-island-flash".into())
            .spawn(run_flash_watch);
    }

    /// 注销闪烁监听：向窗口投递 WM_CLOSE（窗口过程里 DeregisterShellHookWindow +
    /// DestroyWindow → PostQuitMessage），线程随即退出。幂等。
    pub fn stop_flash_watch() {
        FLASH_GEN.store(0, Ordering::SeqCst);
        let raw = FLASH_HWND.swap(0, Ordering::SeqCst);
        if raw != 0 {
            unsafe {
                let _ = PostMessageW(Some(hwnd_of(raw)), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// 监听线程主体：窗口不可见（WS_POPUP、0×0、不 show），只用来收消息。
    fn run_flash_watch() {
        unsafe {
            let Ok(module) = GetModuleHandleW(None) else {
                return;
            };
            let instance = HINSTANCE(module.0);
            let class_name = flash_class_name();
            let wc = WNDCLASSW {
                style: WNDCLASS_STYLES::default(),
                lpfnWndProc: Some(flash_wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: HICON(ptr::null_mut()),
                hCursor: HCURSOR(ptr::null_mut()),
                hbrBackground: HBRUSH(ptr::null_mut()),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: class_name,
            };
            // 类可能由上一代线程注册过：重复注册返回 0 无妨（同名类仍可用）。
            let _ = RegisterClassW(&wc);
            let title = wide_null("AgentHUD Flash Watch");
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                PCWSTR::from_raw(title.as_ptr()),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            ) {
                Ok(hwnd) => hwnd,
                Err(_) => return,
            };
            let shell_msg =
                RegisterWindowMessageW(PCWSTR::from_raw(wide_null("SHELLHOOK").as_ptr()));
            FLASH_MSG.store(shell_msg, Ordering::SeqCst);
            // 只有槽为空才登记（旧一代线程可能晚到，不能覆盖新一代的窗口）。
            let key = hwnd_key(hwnd);
            if FLASH_HWND
                .compare_exchange(0, key, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
                || !running(FLASH_GEN.load(Ordering::SeqCst))
            {
                let _ = DestroyWindow(hwnd);
                let _ = FLASH_HWND.compare_exchange(key, 0, Ordering::SeqCst, Ordering::SeqCst);
                return;
            }
            let _ = RegisterShellHookWindow(hwnd);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = FLASH_HWND.compare_exchange(key, 0, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    unsafe extern "system" fn flash_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let shell_msg = FLASH_MSG.load(Ordering::SeqCst);
        if shell_msg != 0 && msg == shell_msg {
            // wParam = shell 事件码；HSHELL_FLASH 时 lParam = 闪烁的窗口句柄。
            if wparam.0 as u32 == HSHELL_FLASH && lparam.0 != 0 {
                handle_flash(lparam.0 as isize);
            }
            return LRESULT(0);
        }
        match msg {
            WM_CLOSE => {
                // 先注销 shell hook，再销毁窗口；WM_DESTROY 里退出消息循环。
                let _ = DeregisterShellHookWindow(hwnd);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    /// 处理一条闪烁：exe → kind → 8s 去重 → 模块过滤 → 走 toast 同一条 push/emit 链路。
    fn handle_flash(flash_hwnd: isize) {
        let Some(app) = FLASH_APP.get() else {
            return;
        };
        let gen = FLASH_GEN.load(Ordering::SeqCst);
        if !running(gen) {
            return;
        }
        // 复用浮窗的 exe 解析逻辑（窗口句柄 → 小写进程名）。
        let Some(exe) = (unsafe { exe_of_hwnd(hwnd_of(flash_hwnd)) }) else {
            return;
        };
        let prefs = NotifyPrefs::from_config();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let id = SYNTH_ID.fetch_sub(1, Ordering::SeqCst);
        // 模块未启用时直接 None（不进 items、不计未读），也不占用 8s 去重窗口。
        let Some(item) = super::flash_item(&exe, &prefs, id, now_ms) else {
            return;
        };
        let now = Instant::now();
        let kind = item.kind.clone();
        let allowed = {
            let Ok(guard) = flash_last_slot().lock() else {
                return;
            };
            flash_allowed(
                guard.get(&kind).map(|last| now.duration_since(*last)),
                Duration::from_millis(FLASH_DEDUPE_MS),
            )
        };
        if !allowed {
            return;
        }
        if let Ok(mut guard) = flash_last_slot().lock() {
            guard.insert(kind, now);
        }
        if push_item(item, &prefs) {
            emit(app);
        }
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

    /// 配置的「打开应用」路径是否与消息来源 exe 一致：`hint` 为空时维持旧行为
    /// （不校验，配了就用）；有 hint 时要求文件名大小写不敏感相等。
    fn configured_path_matches(path: &Path, hint: Option<&str>) -> bool {
        match hint {
            Some(hint) => path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(hint)),
            None => true,
        }
    }

    /// 启动自动探测的候选过滤：邮件来源不能用「应用执行别名」
    /// （`%LOCALAPPDATA%\Microsoft\WindowsApps\*.exe`）当默认值——别名对任何
    /// 来源都启动同一个应用（如 `olk.exe`），优先经典 Outlook / Thunderbird。
    /// 只影响自动探测，用户手动配置的路径不受限制（见 `configured_path_matches`
    /// 与设置窗的输入项）。
    fn auto_detect_candidate_ok(kind: &str, path: &Path) -> bool {
        !(kind == "mail" && path.to_string_lossy().to_ascii_lowercase().contains("windowsapps"))
    }

    /// 点消息：先看设置窗里有没有为这个来源配可执行文件路径，再找已运行的
    /// 来源窗口并切前台，最后按常见安装路径启动。
    pub fn open_app(kind: &str, exe: Option<&str>) -> Result<(), String> {
        // 来源 exe（小写、可能为空）：配置路径必须与它同名才可信——自动探测
        // 会把新版 Outlook 的别名 `olk.exe` 填进邮件项，若拿它去打开
        // Thunderbird 的邮件就会开错应用。
        let hint = exe
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty());
        // 设置窗「打开应用」：配了具体路径且文件存在就直接启动它（支持
        // `%ProgramFiles%` 之类的变量）；文件不在、来源不一致或启动失败都
        // 继续走内置逻辑。
        if let Some(path) = super::configured_open_app(kind) {
            if configured_path_matches(Path::new(&path), hint.as_deref())
                && Path::new(&path).is_file()
                && open::that(&path).is_ok()
            {
                return Ok(());
            }
        }
        let candidates = candidates_for(kind, exe);
        if candidates.is_empty() {
            return Err(format!("未知的消息来源：{kind}"));
        }
        if focus_running(&candidates) {
            return Ok(());
        }
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
        if kind == "dsh" && detect_open_path(kind).is_some_and(|path| open::that(path).is_ok()) {
            return Ok(());
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
            "dsh" => &[super::deepseek::EXE],
            _ => &[],
        };
        for item in defaults {
            if !out.iter().any(|existing| existing == item) {
                out.push((*item).to_string());
            }
        }
        out
    }

    /// 各类消息来源的常见安装目录（环境变量根 + 相对段，最后一段是 exe）。
    /// `known_paths` 与「启动自动探测」共用同一份候选。
    fn common_specs(kind: &str) -> Vec<(&'static str, &'static [&'static str])> {
        match kind {
            "qq" => vec![
                ("ProgramFiles", &["Tencent", "QQNT", "QQ.exe"][..]),
                ("ProgramFiles(x86)", &["Tencent", "QQNT", "QQ.exe"][..]),
                ("ProgramFiles", &["Tencent", "QQ", "Bin", "QQ.exe"][..]),
                ("ProgramFiles(x86)", &["Tencent", "QQ", "Bin", "QQ.exe"][..]),
                ("LocalAppData", &["Tencent", "QQNT", "QQ.exe"][..]),
            ],
            "wechat" => vec![
                ("ProgramFiles", &["Tencent", "Weixin", "Weixin.exe"][..]),
                (
                    "ProgramFiles(x86)",
                    &["Tencent", "Weixin", "Weixin.exe"][..],
                ),
                ("ProgramFiles", &["Tencent", "WeChat", "WeChat.exe"][..]),
                (
                    "ProgramFiles(x86)",
                    &["Tencent", "WeChat", "WeChat.exe"][..],
                ),
                ("LocalAppData", &["Tencent", "Weixin", "Weixin.exe"][..]),
            ],
            "feishu" => vec![
                ("ProgramFiles", &["ByteDance", "Feishu", "Feishu.exe"][..]),
                (
                    "ProgramFiles(x86)",
                    &["ByteDance", "Feishu", "Feishu.exe"][..],
                ),
                ("LocalAppData", &["Feishu", "Feishu.exe"][..]),
                ("LocalAppData", &["ByteDance", "Feishu", "Feishu.exe"][..]),
            ],
            "dingtalk" => vec![
                ("ProgramFiles", &["DingTalk", "DingTalk.exe"][..]),
                ("ProgramFiles(x86)", &["DingTalk", "DingTalk.exe"][..]),
                ("LocalAppData", &["DingTalk", "DingTalk.exe"][..]),
            ],
            "dsh" => vec![
                ("LocalAppData", &["Programs", "DeepSeek Harness", "DeepSeek Harness.exe"][..]),
                ("LocalAppData", &["Programs", "deepseek-harness", "DeepSeek Harness.exe"][..]),
                ("ProgramFiles", &["DeepSeek Harness", "DeepSeek Harness.exe"][..]),
            ],
            "mail" => vec![
                ("LocalAppData", &["Microsoft", "WindowsApps", "olk.exe"][..]),
                (
                    "ProgramFiles",
                    &["Microsoft Office", "root", "Office16", "OUTLOOK.EXE"][..],
                ),
                (
                    "ProgramFiles(x86)",
                    &["Microsoft Office", "root", "Office16", "OUTLOOK.EXE"][..],
                ),
                (
                    "ProgramFiles",
                    &["Mozilla Thunderbird", "thunderbird.exe"][..],
                ),
                (
                    "ProgramFiles(x86)",
                    &["Mozilla Thunderbird", "thunderbird.exe"][..],
                ),
            ],
            _ => Vec::new(),
        }
    }

    fn known_paths(kind: &str, exe_hint: Option<&str>) -> Vec<PathBuf> {
        let specs = common_specs(kind);
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

    /// 启动自动探测「打开应用」的空项：先看内置常见安装路径，再走通用探测
    /// （运行进程 → 卸载注册表 → 常见目录）。返回能直接写进设置文档的路径。
    ///
    /// 邮件项会跳过 WindowsApps 下的应用执行别名（如新版 Outlook 的
    /// `olk.exe`）：那是别人的应用别名，和消息来源 exe 未必是同一个应用。
    pub(super) fn detect_open_path(kind: &str) -> Option<String> {
        if let Some(path) = known_paths(kind, None)
            .into_iter()
            .find(|path| auto_detect_candidate_ok(kind, path))
        {
            return path.to_str().map(str::to_string);
        }
        let (exe_names, keywords): (&[&str], &[&str]) = match kind {
            "qq" => (
                &["qq.exe", "qqnt.exe"][..],
                // QQNT 的卸载项 DisplayName 常只写「QQ」/「QQNT」。
                &["Tencent QQ", "QQNT", "QQ"][..],
            ),
            "wechat" => (
                &["weixin.exe", "wechat.exe"][..],
                &["微信", "WeChat", "Tencent WeChat"][..],
            ),
            "feishu" => (
                &["feishu.exe", "lark.exe"][..],
                &["飞书", "Feishu", "Lark"][..],
            ),
            "dingtalk" => (&["dingtalk.exe"][..], &["钉钉", "DingTalk"][..]),
            "mail" => (
                &["thunderbird.exe", "olk.exe", "outlook.exe", "hxoutlook.exe"][..],
                &["Thunderbird", "Outlook"][..],
            ),
            "dsh" => (&[super::deepseek::EXE][..], &["DeepSeek Harness"][..]),
            _ => return None,
        };
        super::super::app_paths::detect_exe(exe_names, keywords, &common_specs(kind))
            .filter(|path| auto_detect_candidate_ok(kind, path))
            .and_then(|path| path.to_str().map(str::to_string))
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

    pub(super) fn focus_running(candidates: &[String]) -> bool {
        let mut best: Option<(HWND, i64)> = None;
        let mut fallback: Option<HWND> = None;
        for hwnd in all_windows() {
            unsafe {
                // 来源窗口都有标题；没有标题的辅助窗口 / 浮窗本身跳过。
                if GetWindowTextLengthW(hwnd) <= 0 {
                    continue;
                }
                let Some(exe) = exe_of_hwnd(hwnd) else {
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

    pub(super) fn deepseek_foreground() -> bool {
        let Some(exe) = (unsafe { exe_of_hwnd(GetForegroundWindow()) }) else {
            return false;
        };
        exe == super::deepseek::EXE
            || super::configured_open_app("dsh").is_some_and(|path| {
                Path::new(&path)
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(&exe))
            })
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

        /// P2-7：配置的「打开应用」路径必须和消息来源 exe 同名（自动探测
        /// 填的 olk.exe 不能拿去打开 Thunderbird 的邮件）；邮件自动探测
        /// 还要跳过 WindowsApps 下的应用执行别名。
        #[test]
        fn 邮件配置路径匹配来源且探测跳过别名() {
            let alias = Path::new(r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\olk.exe");
            let classic = Path::new(r"C:\Program Files\Mozilla Thunderbird\thunderbird.exe");
            // 配置路径：同名才用（大小写不敏感），不同名跳过；没 hint 维持旧行为
            assert!(configured_path_matches(alias, Some("olk.exe")));
            assert!(configured_path_matches(alias, Some("OLK.EXE")));
            assert!(!configured_path_matches(alias, Some("thunderbird.exe")));
            assert!(configured_path_matches(alias, None));
            // 自动探测：邮件项跳过 WindowsApps 别名，经典安装路径照常可用
            assert!(!auto_detect_candidate_ok("mail", alias));
            assert!(auto_detect_candidate_ok("mail", classic));
            // 其它来源不受该过滤影响
            assert!(auto_detect_candidate_ok("qq", alias));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 打开应用路径展开环境变量() {
        let lookup = |name: &str| match name {
            "ProgramFiles" => Some(r"C:\Program Files".to_string()),
            "LocalAppData" => Some(r"C:\Users\me\AppData\Local".to_string()),
            _ => None,
        };
        assert_eq!(
            expand_env_vars(r"%ProgramFiles%\Tencent\QQNT\QQ.exe", lookup),
            r"C:\Program Files\Tencent\QQNT\QQ.exe"
        );
        // 多个占位符 + 前后普通文本
        assert_eq!(
            expand_env_vars(r"x %LocalAppData%/y/%ProgramFiles%/z", lookup),
            r"x C:\Users\me\AppData\Local/y/C:\Program Files/z"
        );
        // 未定义变量原样保留（方便用户排查拼写）
        assert_eq!(expand_env_vars(r"%NOPE%\a.exe", lookup), r"%NOPE%\a.exe");
        // 单独一个 % / 空占位符都不 panic 且不吞字符
        assert_eq!(expand_env_vars("100%", lookup), "100%");
        assert_eq!(expand_env_vars(r"%%\a", lookup), r"%%\a");
        assert_eq!(expand_env_vars("", lookup), "");
    }

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
        assert_eq!(
            map_primary("com.alibaba.dingtalk"),
            Some(("dingtalk", "钉钉"))
        );
        assert_eq!(
            map_primary("Microsoft.OutlookForWindows"),
            Some(("mail", "邮件"))
        );
        // 音乐类通知不能算成 QQ 消息
        assert_eq!(map_primary("CloudMusic"), None);
        assert_eq!(map_primary("QQMusic"), None);
    }

    /// 通知库门控：只有 `map_primary` 能产出的 toast 来源启用时才读 SQLite；
    /// 全关 / 只开 dsh 等非通知库模块时 `poll_new` 直接空转。纯函数，不连库。
    #[test]
    fn 通知库轮询仅在toast来源启用时进行() {
        let prefs = |modules: &[&str]| NotifyPrefs {
            modules: modules.iter().map(|kind| kind.to_string()).collect(),
            body_preview: Vec::new(),
        };
        assert!(!prefs(&[]).toast_sources_enabled());
        // 只开 DeepSeek（走会话日志，不走通知库）→ 不读 SQLite
        assert!(!prefs(&["dsh"]).toast_sources_enabled());
        // 每一种 map_primary 可产出的 kind 都算 toast 来源
        for kind in TOAST_SOURCE_KINDS {
            assert!(prefs(&[kind]).toast_sources_enabled(), "{kind}");
        }
        // 飞书 / 钉钉的 toast 同样在通知库里，只开浮窗模块也必须继续读库
        assert!(prefs(&["feishu"]).toast_sources_enabled());
        assert!(prefs(&["dingtalk"]).toast_sources_enabled());
        assert!(prefs(&["dsh", "mail"]).toast_sources_enabled());
        // 未知 kind 不算来源（前端白名单之外的脏数据不能撬开轮询）
        assert!(!prefs(&["stock", "unknown"]).toast_sources_enabled());
    }

    #[test]
    fn 通知来源映射到可执行文件() {
        assert_eq!(exe_for_primary("electron.app.Feishu"), "feishu.exe");
        assert_eq!(exe_for_primary("com.alibaba.dingtalk"), "dingtalk.exe");
        assert_eq!(
            exe_for_primary(
                "microsoft.windowscommunicationsapps_8wekyb3d8bbwe!microsoft.windowslive.mail"
            ),
            "hxoutlook.exe"
        );
        assert_eq!(exe_for_primary("ThunderbirdToast"), "thunderbird.exe");
        assert_eq!(exe_for_primary("Microsoft.OutlookForWindows"), "olk.exe");
        assert_eq!(exe_for_primary("未知应用"), "");
    }

    fn prefs(modules: &[&str], preview: &[&str]) -> NotifyPrefs {
        NotifyPrefs {
            modules: modules.iter().map(|k| k.to_string()).collect(),
            body_preview: preview.iter().map(|k| k.to_string()).collect(),
        }
    }

    fn item(kind: &str, read: bool) -> NotifyItem {
        NotifyItem {
            id: 1,
            kind: kind.to_string(),
            name: kind.to_string(),
            sender: String::new(),
            body: String::new(),
            code: String::new(),
            time: 0,
            read,
            exe: String::new(),
        }
    }

    fn texts(items: &[&str]) -> Vec<String> {
        items.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn 模块开关过滤可见列表且不影响未读统计口径() {
        let items = vec![
            item("qq", false),
            item("wechat", true),
            item("feishu", false),
            item("mail", true),
        ];
        // 默认只开 qq/wechat/mail：feishu 的条目不可见
        let visible = visible_items(&items, &prefs(&["qq", "wechat", "mail"], &[]));
        let kinds: Vec<&str> = visible.iter().map(|i| i.kind.as_str()).collect();
        assert_eq!(kinds, vec!["qq", "wechat", "mail"]);
        assert_eq!(visible.iter().filter(|i| !i.read).count(), 1);
        // 全关：一条都不显示
        assert!(visible_items(&items, &prefs(&[], &[])).is_empty());
        // 只开 feishu
        let visible = visible_items(&items, &prefs(&["feishu"], &[]));
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].kind, "feishu");
    }

    #[test]
    fn push_item_drops_disabled_kind() {
        let off = prefs(&["wechat"], &[]);
        let before = INBOX.lock().map(|g| g.items.len()).unwrap_or(0);
        assert!(!push_item(item("qq", false), &off));
        assert_eq!(
            INBOX.lock().map(|g| g.items.len()).unwrap_or(0),
            before,
            "未启用模块不能进 items"
        );
        assert!(push_item(item("wechat", false), &off));
        assert_eq!(
            INBOX.lock().map(|g| g.items.len()).unwrap_or(0),
            before + 1
        );
        // 清掉本测试写入的条目，避免影响同进程其它单测
        if let Ok(mut inbox) = INBOX.lock() {
            inbox.clear(Some("wechat"));
        }
    }

    #[test]
    fn wechat_unread_survives_history_limit_and_is_removed_on_read() {
        let mut inbox = NotifyInbox::default();
        let total = HISTORY_MAX + 3;
        for _ in 0..total {
            inbox.push(item("wechat", false));
        }
        for _ in 0..HISTORY_MAX {
            inbox.push(item("mail", false));
        }
        let visible = prefs(&["wechat", "mail"], &[]);
        let snapshot = inbox.snapshot(&visible);
        assert_eq!(snapshot.wechat_unread, total);
        assert_eq!(snapshot.unread, total + HISTORY_MAX);
        assert_eq!(snapshot.items.iter().filter(|item| item.kind == "wechat").count(), 1);
        assert_eq!(inbox.snapshot(&prefs(&["mail"], &[])).wechat_unread, 0);

        inbox.mark_read(Some("wechat"));
        let snapshot = inbox.snapshot(&visible);
        assert_eq!(snapshot.wechat_unread, 0);
        assert_eq!(snapshot.unread, HISTORY_MAX);
        assert!(snapshot.items.iter().all(|item| item.kind == "mail"));
    }

    #[test]
    fn toast文本规则跟随正文预览开关() {
        // 默认：qq 不在正文预览里 → 有 sender 无 body
        let (sender, body) = toast_item_texts(
            "qq",
            &texts(&["张三", "在吗？"]),
            &texts(&["feishu", "dingtalk"]),
        );
        assert_eq!(sender, "张三");
        assert_eq!(body, "");
        // feishu 在正文预览里 → sender + body
        let (sender, body) = toast_item_texts(
            "feishu",
            &texts(&["李四", "周会改到三点"]),
            &texts(&["feishu", "dingtalk"]),
        );
        assert_eq!(sender, "李四");
        assert_eq!(body, "周会改到三点");
        // feishu 只有一条文本且开了预览：sender 为空，那条当正文（对齐参考实现）
        let (sender, body) =
            toast_item_texts("feishu", &texts(&["周会改到三点"]), &texts(&["feishu"]));
        assert_eq!(sender, "");
        assert_eq!(body, "周会改到三点");
        // feishu 只有一条且未开预览：那条是 sender，body 为空
        let (sender, body) = toast_item_texts("feishu", &texts(&["李四"]), &texts(&[]));
        assert_eq!(sender, "李四");
        assert_eq!(body, "");
        // qq 被加进正文预览：单条文本时 sender 让位给 body
        let (sender, body) = toast_item_texts("qq", &texts(&["在吗？"]), &texts(&["qq"]));
        assert_eq!(sender, "");
        assert_eq!(body, "在吗？");
        // 邮件默认不在正文预览且不在 sender 白名单 → 都为空
        let (sender, body) =
            toast_item_texts("mail", &texts(&["通知", "新邮件"]), &texts(&["feishu"]));
        assert_eq!(sender, "");
        assert_eq!(body, "");
        // 邮件被加进正文预览：sender 仍为空，body 取第二条
        let (sender, body) =
            toast_item_texts("mail", &texts(&["通知", "新邮件"]), &texts(&["mail"]));
        assert_eq!(sender, "");
        assert_eq!(body, "新邮件");
        // kind 不在预览里同时 sender 白名单里也不在（如 dingtalk 未开预览）→ sender 仍可显示
        let (sender, body) = toast_item_texts("dingtalk", &texts(&["王五", "看下"]), &texts(&[]));
        assert_eq!(sender, "王五");
        assert_eq!(body, "");
    }

    #[test]
    fn flash进程映射到消息类型() {
        // 任务要求的所有 exe（含 QQNT）
        assert_eq!(flash_source("qq.exe"), Some(("qq", "QQ")));
        assert_eq!(flash_source("qqnt.exe"), Some(("qq", "QQ")));
        assert_eq!(flash_source("weixin.exe"), Some(("wechat", "微信")));
        assert_eq!(flash_source("wechat.exe"), Some(("wechat", "微信")));
        assert_eq!(flash_source("feishu.exe"), Some(("feishu", "飞书")));
        assert_eq!(flash_source("lark.exe"), Some(("feishu", "飞书")));
        assert_eq!(flash_source("dingtalk.exe"), Some(("dingtalk", "钉钉")));
        assert_eq!(flash_source("thunderbird.exe"), Some(("mail", "邮件")));
        assert_eq!(flash_source("outlook.exe"), Some(("mail", "邮件")));
        assert_eq!(flash_source("olk.exe"), Some(("mail", "邮件")));
        assert_eq!(flash_source("hxoutlook.exe"), Some(("mail", "邮件")));
        // 大小写 / 前后空白不影响
        assert_eq!(flash_source(" Weixin.EXE "), Some(("wechat", "微信")));
        assert_eq!(flash_source("QQNT.Exe"), Some(("qq", "QQ")));
        // 未知/空进程名不投递
        assert_eq!(flash_source("chrome.exe"), None);
        assert_eq!(flash_source(""), None);
        assert_eq!(flash_source("qq.exe.bak"), None);
    }

    #[test]
    fn flash同类8秒去重() {
        let dedupe = Duration::from_millis(8_000);
        assert!(flash_allowed(None, dedupe), "第一次必须放行");
        assert!(!flash_allowed(Some(Duration::from_millis(7_999)), dedupe));
        assert!(flash_allowed(Some(Duration::from_millis(8_000)), dedupe));
        assert!(flash_allowed(Some(Duration::from_millis(9_000)), dedupe));
        // 刚投递过（0ms）必须被 8s 窗口挡住
        assert!(!flash_allowed(Some(Duration::ZERO), dedupe));
    }

    #[test]
    fn flash条目遵守模块开关() {
        // qq 未启用 → 不投递（不进 items、不计未读）
        assert!(flash_item("qq.exe", &prefs(&["wechat"], &[]), -3, 1_700_000_000_000).is_none());
        // 全关 → 不投递
        assert!(flash_item("weixin.exe", &prefs(&[], &[]), -4, 0).is_none());
        // 未知进程 → 不投递（与模块无关）
        assert!(flash_item(
            "chrome.exe",
            &prefs(&["qq", "wechat", "mail", "feishu", "dingtalk"], &[]),
            -5,
            0
        )
        .is_none());
        // qq 已启用 → 投递一条与 toast 同结构的条目：sender/body 空、id/time 原样、
        // exe 为小写进程名（点按消息时可定位应用）
        let item = flash_item(" QQ.EXE ", &prefs(&["qq"], &[]), -6, 1_700_000_000_000)
            .expect("qq 已启用应投递");
        assert_eq!(item.id, -6);
        assert_eq!(item.kind, "qq");
        assert_eq!(item.name, "QQ");
        assert_eq!(item.sender, "");
        assert_eq!(item.body, "");
        assert_eq!(item.time, 1_700_000_000_000);
        assert!(!item.read);
        assert_eq!(item.exe, "qq.exe");
    }

    #[test]
    fn toast文本按40和120字截断() {
        let long_sender: String = "发".repeat(80);
        let long_body: String = "正".repeat(300);
        let (sender, body) = toast_item_texts(
            "qq",
            &texts(&[long_sender.as_str(), long_body.as_str()]),
            &texts(&[]),
        );
        assert_eq!(sender.chars().count(), 40);
        assert_eq!(body, "");
        let (sender, body) = toast_item_texts(
            "feishu",
            &texts(&[long_sender.as_str(), long_body.as_str()]),
            &texts(&["feishu"]),
        );
        assert_eq!(sender.chars().count(), 40);
        assert_eq!(body.chars().count(), 120);
        assert_eq!(body, "正".repeat(120));
    }

    fn toast_payload(texts: &[&str]) -> Vec<u8> {
        let nodes: String = texts.iter().map(|text| format!("<text>{text}</text>")).collect();
        format!(
            "<toast><visual><binding template=\"ToastGeneric\">{nodes}</binding></visual></toast>"
        )
        .into_bytes()
    }

    /// 系统通知来源的邮件：toast 文本里带验证码时就地抽出；其它 kind 不抽。
    #[test]
    fn toast_mail_items_extract_code_but_other_kinds_do_not() {
        let prefs = prefs(&["mail", "qq"], &["mail"]);
        let rows = vec![
            (
                11,
                "Microsoft.OutlookForWindows".into(),
                0,
                "toast".into(),
                toast_payload(&["登录验证码", "您的验证码是 123456"]),
            ),
            (
                12,
                "QQ".into(),
                0,
                "toast".into(),
                toast_payload(&["登录验证码", "您的验证码是 123456"]),
            ),
        ];
        let items = rows_to_items(rows, &prefs);
        assert_eq!(items.len(), 2);
        let mail = items.iter().find(|item| item.kind == "mail").expect("邮件条目");
        assert_eq!(mail.code, "123456");
        // 邮件正文预览规则不变：prefs 开了 mail 预览 → body 取第二条
        assert_eq!(mail.body, "您的验证码是 123456");
        let qq = items.iter().find(|item| item.kind == "qq").expect("QQ 条目");
        assert_eq!(qq.code, "");
    }

    /// 系统通知里没有验证码时保持空串，不影响既有 sender/body 规则。
    #[test]
    fn toast_mail_without_code_keeps_empty_code() {
        let prefs = prefs(&["mail"], &["mail"]);
        let rows = vec![(
            13,
            "Microsoft.OutlookForWindows".into(),
            0,
            "toast".into(),
            toast_payload(&["邮件", "2 封新邮件"]),
        )];
        let items = rows_to_items(rows, &prefs);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].code, "");
    }
}
