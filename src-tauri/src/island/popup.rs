//! 消息玻璃提示框（label `notify-popup`）：新消息到桌面时在角落浮出的玻璃小卡。
//!
//! 原型 360×84、圆角 18、六种位置、最多 5 条、每条独立 3s、hover 暂停、
//! 点击秒关并打开来源应用，但落地方式必须改：
//!
//! ## 为什么是独立窗口
//! 岛窗口是**固定 780×632 的透明画布**，画布上胶囊以外的区域由 Rust 的
//! 16ms 轮询（`start_mouse_through_watcher`）设为点击穿透。浮窗必须能被
//! 鼠标命中（hover 暂停 / 点击打开），所以不能画在岛画布里，只能另开窗口。
//!
//! ## 窗口形态与「不吃点击」
//! `transparent + decorations(false) + always_on_top + skip_taskbar +
//! focusable(false) + shadow(false) + resizable(false)`，**不做**
//! ignore_cursor_events（要收鼠标）。代价是窗口矩形内的像素都会被命中，
//! 因此窗口尺寸必须**紧贴当前卡片堆叠**：空闲时 `hide()`，前端每次堆叠
//! 变化都调 `notify_popup_resize` 让 Rust 按当前位置规则重新定位 + set_size
//! （贴下边保持底边不动、贴上边保持顶边不动，水平按 left / right / center）。
//!
//! ## 时序（与岛端编排共用常量）
//! 收到新消息后**先等 [`POPUP_DELAY_MS`]**（岛端先播惊讶表情，再呼吸灯 +
//! 活动卡），把这段等待窗口内到达的同类消息**合并成一批**统一出卡，然后由
//! 前端播 180ms 淡入。等待结束时重新读一次开关：期间关掉 `notify.popup`
//! 就不出卡。窗口在消息到达那一刻就以隐藏态创建，WebView2 冷启动与这段
//! 等待重叠，第一波不至于比后面几波更晚。
//!
//! ## 顺序语义（与参考实现一致，容易读反，写清楚）
//! 浮窗内部数组 `cards[0]` 是**最靠屏幕边缘**的那条 = 最早到达的那条；
//! 新卡永远追加到数组末尾（最靠屏幕内侧）。超过上限时**直接秒删
//! `cards[0]`**（贴边最旧），不播退场。
//!
//! ## 职责划分
//! - Rust：波次合并 + 1.2s 延迟 + 窗口生命周期/几何 + 配置快照；
//! - 前端（`src/island/NotifyPopup.tsx`）：卡片渲染、5 条与高度上限裁剪、
//!   180ms 淡入、3s 独立计时、hover 暂停/重置、260ms 退场、180ms 让位、
//!   点击调既有的 `island_notify_open`。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{
    window::Color, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use super::notify::{NotifyItem, NotifyState};

/// 浮窗 label（与 `capabilities/notify-popup.json` 的 windows 白名单、
/// `notify-popup.html` 入口同名，改名要三处同步）。
pub const POPUP_WINDOW_LABEL: &str = "notify-popup";
/// 出卡广播（Rust → 浮窗页面）：`PopupShow { items, config }`。
pub const POPUP_SHOW_EVENT: &str = "zbar://notify-popup-show";
/// 配置广播：`PopupConfig`（enabled / position / size / theme / modules / 上限）。
pub const POPUP_CONFIG_EVENT: &str = "zbar://notify-popup-config";

/// 出卡延迟（毫秒，**单一权威常量**）：等岛端「惊讶表情 → 呼吸灯 + 活动卡」
/// 的编排先动起来，再浮出玻璃框。岛端前端用同名常量，两边必须一起改。
pub const POPUP_DELAY_MS: u64 = 1200;

/// 设计稿卡片尺寸（逻辑 px，`notify.popup.size` = 100% 时）
pub const POPUP_CARD_W: f64 = 360.0;
pub const POPUP_CARD_H: f64 = 84.0;
/// 卡片间距（逻辑 px）。**不随 size 缩放**（参考实现同样固定 4px）。
pub const POPUP_GAP: f64 = 4.0;
/// 与屏幕工作区边缘的安全边距（逻辑 px）
pub const POPUP_EDGE: f64 = 16.0;
/// 退场时卡片会朝贴边方向位移 10px：窗口在**贴边一侧**多留这么宽的一条透明
/// 带，否则最后那 10px 会被窗口自己裁掉（参考实现用的是 10px 投影留白）。
/// 其余三边仍然紧贴卡片；实际宽度由前端按 `size` 缩放后作为 `pad` 传进来。
pub const POPUP_EXIT_PAD: f64 = 10.0;
/// 同屏最多同时显示的卡片数
pub const POPUP_MAX: usize = 5;

/// 待出卡队列（1.2s 波次窗口内累积；等待结束一次性取空）
fn queue_slot() -> &'static Mutex<Vec<NotifyItem>> {
    static QUEUE: Mutex<Vec<NotifyItem>> = Mutex::new(Vec::new());
    &QUEUE
}

/// 页面尚未挂载时到点的波次先攒在这里，由 `notify_popup_ready` 交出去
fn pending_slot() -> &'static Mutex<Vec<NotifyItem>> {
    static PENDING: Mutex<Vec<NotifyItem>> = Mutex::new(Vec::new());
    &PENDING
}

/// 波次计时线程是否已经在跑（并发到达只起一个计时器）
static TIMER_ARMED: AtomicBool = AtomicBool::new(false);
/// 浮窗页面是否已经 `listen` 完并调用过 `notify_popup_ready`
static PAGE_READY: AtomicBool = AtomicBool::new(false);
/// 浮窗自己的「已见」基线（纯数据，便于单测）。
///
/// 两类 id 的语义不同，必须分开记：
/// - **数据库 toast**（`id >= 0`）：`wpndatabase.db` 的 id 单调递增；
/// - **合成来源**（`id < 0`，飞书 / 钉钉浮窗 UIA、任务栏 `HSHELL_FLASH`）：
///   id 逐条递减，只能记「见过哪些」。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SeenState {
    /// 已见过的最大数据库 id（`-1` = 还没见过任何数据库条目）
    pub toast_max: i64,
    /// 新鲜度窗口内已见过的合成 id 和到达时间。隐藏模块时也保留这段基线。
    pub synth: BTreeMap<i64, i64>,
}

impl Default for SeenState {
    fn default() -> Self {
        Self {
            toast_max: -1,
            synth: BTreeMap::new(),
        }
    }
}

/// 消息广播共享的已见基线。
fn seen_slot() -> &'static Mutex<SeenState> {
    static SEEN: Mutex<SeenState> = Mutex::new(SeenState {
        toast_max: -1,
        synth: BTreeMap::new(),
    });
    &SEEN
}

/// 负数合成条目的新鲜度窗口（毫秒）：模块开关来回切时，重新出现的旧条目
/// 不能当成新消息再弹一次
const SYNTH_FRESH_MS: u64 = 15_000;
/// 隐藏代次：`notify_popup_hide` 每调用一次 +1。resize 投递主线程前先记下代次，
/// 闭包里再比一次——点最后一张卡（hide）与 resize 几乎同时发生时，迟到的
/// resize 不会把「已经没有卡片」的透明置顶窗又 show 出来。
static HIDE_GEN: AtomicU64 = AtomicU64::new(0);

// ============================================================
// 纯函数：位置 / 尺寸 / 裁剪（全部有单测）
// ============================================================

/// `notify.popup.size`（70..=160 百分比）→ 卡片缩放比（0.7..=1.6）。
pub(crate) fn popup_scale(size: i64) -> f64 {
    size.clamp(70, 160) as f64 / 100.0
}

/// 位置是否贴工作区**下边**（决定退场位移方向与缩放锚点）。
pub(crate) fn popup_is_bottom(position: &str) -> bool {
    position.starts_with("center_bottom") || position.starts_with("bottom_")
}

/// 横向锚点：`left` / `right` / `center`（center_* 是水平居中）。
pub(crate) fn popup_h_anchor(position: &str) -> &'static str {
    if position.ends_with("_left") {
        "left"
    } else if position.ends_with("_right") {
        "right"
    } else {
        "center"
    }
}

/// 堆叠尺寸（逻辑 px）：宽 = 设计宽 × scale；高 = n × 卡高 × scale + (n-1) × 间距。
pub(crate) fn popup_stack_size(scale: f64, count: usize) -> (f64, f64) {
    let s = scale.clamp(0.7, 1.6);
    let w = POPUP_CARD_W * s;
    let h = if count == 0 {
        0.0
    } else {
        count as f64 * POPUP_CARD_H * s + (count - 1) as f64 * POPUP_GAP
    };
    (w, h)
}

/// 位置 + 窗口尺寸（**含贴边侧透明带 `pad`**）+ 工作区（逻辑 px，`area` 为
/// 工作区左上角与宽高）→ 窗口左上角。
///
/// 贴边语义：`bottom_*` / `center_bottom` 的**卡片box**下沿、`top_*` /
/// `center_top` 的卡片box 上沿都距工作区边 [`POPUP_EDGE`]；`pad` 那一条透明
/// 带永远长在贴边一侧（退场位移住在这里），所以窗口外沿会再多出去 `pad`。
/// `center_*` 只做水平居中，**不是屏幕正中**。
pub(crate) fn popup_window_xy(
    position: &str,
    width: f64,
    height: f64,
    pad: f64,
    area: (f64, f64, f64, f64),
) -> (f64, f64) {
    let (ax, ay, aw, ah) = area;
    let pad = pad.clamp(0.0, 64.0);
    let x = match popup_h_anchor(position) {
        "left" => ax + POPUP_EDGE,
        "right" => ax + aw - POPUP_EDGE - width,
        _ => ax + (aw - width) / 2.0,
    };
    // 卡片box（= 窗口去掉贴边侧那条 pad）贴边放，窗口再朝贴边方向让出 pad
    let card = (height - pad).max(0.0);
    let y = if popup_is_bottom(position) {
        ay + ah - POPUP_EDGE - card
    } else {
        ay + POPUP_EDGE - pad
    };
    (x, y)
}

/// 当前屏幕高度 + 当前 size 下一屏最多能放几张卡（1..=[`POPUP_MAX`]）。
///
/// 高度不够时前端按「贴边最旧先删」的顺序秒删，删到 `max_cards` 为止；
/// 至少保留 1 张，避免屏幕上什么都不剩。
pub(crate) fn max_cards(scale: f64, max_height: f64) -> usize {
    let card = POPUP_CARD_H * scale.clamp(0.7, 1.6);
    if !max_height.is_finite() || max_height <= 0.0 {
        return 1;
    }
    let mut count = 1;
    while count < POPUP_MAX {
        let next = count + 1;
        let needed = next as f64 * card + (next - 1) as f64 * POPUP_GAP;
        if needed > max_height {
            break;
        }
        count = next;
    }
    count
}

/// 推进基线并返回本次「新到」的条目（纯函数，便于单测）。
///
/// 判新规则：
/// - **数据库 toast**（`id >= 0`）：比已见最大值更大**且未读**。历史回填
///   （`notify::init_history`）会把读到的条目全部标成 `read = true`，所以
///   「岛关了再开」重新读入的那批历史天然被挡掉，不会补弹；而新到的消息一定
///   是 `read = false`（`push_item` 只投递当期新行）。
/// - **合成来源**（`id < 0`）：没见过这个 id，且在 [`SYNTH_FRESH_MS`] 之内。
///
/// 也会把基线推进到本次状态（无论最终是否出卡），这样关掉浮窗期间到达的
/// 消息不会被攒下来，重新打开时一次性补弹。
pub(crate) fn take_fresh(
    seen: &mut SeenState,
    items: &[NotifyItem],
    now_ms: i64,
) -> Vec<NotifyItem> {
    let toast_max_now = items
        .iter()
        .filter(|item| item.id >= 0)
        .map(|item| item.id)
        .max()
        .unwrap_or(-1);
    let is_recent = |time| time > 0 && now_ms.abs_diff(time) <= SYNTH_FRESH_MS;
    seen.synth.retain(|_, time| is_recent(*time));
    let mut fresh: Vec<NotifyItem> = items
        .iter()
        .filter(|item| {
            if item.id >= 0 {
                item.id > seen.toast_max && !item.read
            } else {
                is_recent(item.time)
                    && seen.synth.insert(item.id, item.time).is_none()
                    && !item.read
            }
        })
        .cloned()
        .collect();
    seen.toast_max = toast_max_now.max(seen.toast_max);
    fresh.sort_by_key(|item| (item.time, item.id));
    fresh
}

/// 一批新到条目按「最多 max 条、贴边最旧先丢」裁剪，保持到达顺序。
///
/// 跨波次的活卡片上限由前端按当前堆叠维护（只有它知道哪条已经退场），
/// 这里负责单波次内的裁剪顺序，保证「超过时丢的是最早到达的那些」。
pub(crate) fn cap_wave(mut items: Vec<NotifyItem>, max: usize) -> Vec<NotifyItem> {
    // 正负 id 混在一批里时不能再按 id 排：统一按到达时间（同毫秒再看 id）
    items.sort_by_key(|item| (item.time, item.id));
    let skip = items.len().saturating_sub(max.max(1));
    items.into_iter().skip(skip).collect()
}

// ============================================================
// 配置快照
// ============================================================

/// 浮窗需要的配置快照（`notify_popup_ready` 返回值 + 每次配置变更广播）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupConfig {
    /// `notify.popup.enabled`
    pub enabled: bool,
    /// 岛总开关：关岛时浮窗一并作废，页面要清掉手上的卡片，否则快速
    /// 关→开之后旧卡会跟着新卡一起重新显示出来
    pub island_enabled: bool,
    pub position: String,
    pub size: i64,
    /// 岛当前主题（auto / light / dark / rouge / sky），浮窗据此取玻璃色板
    pub theme: String,
    /// 已启用的消息模块（前端再兜一层过滤）
    pub modules: Vec<String>,
    /// 可用堆叠高度（逻辑 px）= 工作区高 - 2 × 边距
    pub max_height: f64,
    /// 当前屏 + 当前 size 下最多显示几条（1..=5，高度不够时小于 5）
    pub max_cards: usize,
}

/// 浮窗出卡载荷。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupShow {
    /// 同一波次的条目，**按到达顺序**（第 0 条最先到，出卡后最贴屏幕边缘）
    pub items: Vec<NotifyItem>,
    pub config: PopupConfig,
}

/// `notify_popup_ready` 返回值：配置快照 + 冷启动期间攒下的第一批。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupReady {
    pub config: PopupConfig,
    pub pending: Vec<NotifyItem>,
}

/// 岛所在那块屏的工作区（逻辑 px）：优先 Tauri `Monitor::work_area`，
/// 取不到时 `MonitorArea` 自己会退化为 size（见 `area_of_monitor`）。
fn work_area_logical(app: &AppHandle) -> (f64, f64, f64, f64) {
    let cfg = super::load_config();
    let area = super::monitor_area_for(app, cfg.monitor.as_deref());
    super::area_logical(&area)
}

fn config_snapshot(app: &AppHandle) -> PopupConfig {
    let cfg = super::load_config();
    let scale = popup_scale(cfg.notify_popup_size);
    let max_height = (work_area_logical(app).3 - 2.0 * POPUP_EDGE).max(0.0);
    PopupConfig {
        enabled: cfg.notify_popup_enabled,
        island_enabled: cfg.enabled,
        position: cfg.notify_popup_position.clone(),
        size: cfg.notify_popup_size,
        theme: cfg.theme.clone(),
        modules: cfg.notify_modules.clone(),
        max_height,
        max_cards: max_cards(scale, max_height),
    }
}

// ============================================================
// 窗口
// ============================================================

/// 建窗 / 取窗。**必须在主线程事件循环上下文调用**（建窗铁律见
/// `island_settings_window.rs` 模块 doc：同步命令里直接建窗会自等待死锁）。
fn ensure_window_on_main(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(win) = app.get_webview_window(POPUP_WINDOW_LABEL) {
        return Ok(win);
    }
    // 建窗时先按「一张卡」的尺寸，真正几何由第一次 notify_popup_resize 摆好
    let (start_w, start_h) =
        popup_stack_size(popup_scale(super::load_config().notify_popup_size), 1);
    let builder = WebviewWindowBuilder::new(
        app,
        POPUP_WINDOW_LABEL,
        WebviewUrl::App("notify-popup.html".into()),
    )
    .title("AgentHUD Notify Popup")
    .inner_size(start_w, start_h)
    .position(0.0, 0.0)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .min_inner_size(1.0, 1.0)
    // 后台节流只对 Linux（WebKitGTK）生效；Windows 上 Tauri 这个开关是
    // Unsupported / no-op，拿不到「计时器不会被压」的保证。因此 3s 停留
    // 不能当成准的：前端在 visibilitychange 时用 performance.now() 校正剩余
    // 时间（见 NotifyPopup.tsx 的 resyncHolds）。这里保留开关只是与岛窗口
    // 同口径、跨平台不倒退。
    .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
    // 不抢焦点：WS_EX_NOACTIVATE（Windows），显示时不会把前台应用顶掉
    .focusable(false)
    .focused(false)
    // 先以隐藏态建窗，几何由 notify_popup_resize 一次 set 好再 show，
    // 规避 tao 对极窄窗口的默认钳宽，也避免建窗瞬间闪一块空窗
    .visible(false);

    let win = builder
        .build()
        .map_err(|e| format!("创建消息浮窗失败: {e}"))?;
    // 双保险：系统/WebView2 默认底色的白底会让圆角外露出一块矩形
    let _ = win.set_background_color(Some(Color(0, 0, 0, 0)));
    // 新窗口立刻跟上「截图 / 录屏排除」，否则共享画面里会先漏出通知正文
    apply_capture_flag_on_main(app);
    Ok(win)
}

/// 把「截图 / 录屏排除」同步到浮窗（`island.exclude_from_capture`）。
///
/// 与岛窗口同一套 Win32 调用（见 `island::apply_window_flags_on_main`）：
/// 用户开了排除，通知正文就不该出现在共享画面 / 录屏里。非 Windows 无等价
/// 能力，直接跳过。
#[cfg(target_os = "windows")]
fn apply_capture_flag_on_main(app: &AppHandle) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };

    let Some(win) = app.get_webview_window(POPUP_WINDOW_LABEL) else {
        return;
    };
    let excluded = super::load_config().exclude_from_capture;
    let Ok(hwnd) = win.hwnd() else {
        return;
    };
    let affinity = if excluded {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    if let Err(e) = unsafe { SetWindowDisplayAffinity(HWND(hwnd.0), affinity) } {
        eprintln!("[zbar-island-popup] 设置消息浮窗采集排除失败: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_capture_flag_on_main(_app: &AppHandle) {}

/// 配置变化后在主线程同步一次采集排除（`apply_config` 的窗口标志分支调用）。
pub(crate) fn sync_capture_flag(app: &AppHandle) {
    let app_main = app.clone();
    let _ = app.run_on_main_thread(move || apply_capture_flag_on_main(&app_main));
}

/// 在后台线程（通知轮询）里请求建窗：投递到主线程，失败只记日志。
///
/// 提前建窗是**故意**的：WebView2 冷启动（几百毫秒到 1 秒）与
/// [`POPUP_DELAY_MS`] 的等待重叠，第一波卡片不会比后面几波晚一个 WebView
/// 启动时间。
pub(crate) fn ensure_window(app: &AppHandle) {
    let app_main = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(e) = ensure_window_on_main(&app_main) {
            eprintln!("[zbar-island-popup] {e}");
        }
    });
}

/// 主线程上下文里同步隐藏（岛总开关关闭 / 配置关掉弹窗时调用）。
pub(crate) fn hide_on_main(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(POPUP_WINDOW_LABEL) {
        let _ = win.hide();
    }
}

// ============================================================
// 波次：新消息 → 1.2s 合并 → 出卡广播
// ============================================================

/// 消息状态广播钩子（由 `notify::emit` 调用）。
///
/// 挑「本次新到」的规则见 [`take_fresh`]：数据库条目要「id 更大且未读」（历史
/// 回填全是已读，岛关了再开不会补弹），合成负 id 靠集合 + 新鲜度窗口。
/// 岛开 / 关时 `notify::start`、`notify::stop` 会调 [`reset_baseline`] 清基线。
pub(crate) fn on_notify_state(app: &AppHandle, state: &NotifyState) {
    // 先推进基线再判开关：关掉浮窗期间到达的消息不算新消息，重新打开时
    // 不会把攒下的历史一次性补弹出来
    let fresh = match seen_slot().lock() {
        Ok(mut seen) => take_fresh(&mut seen, &state.items, now_ms()),
        Err(_) => return,
    };
    if fresh.is_empty() {
        return;
    }

    let cfg = super::load_config();
    if !cfg.enabled || !cfg.notify_popup_enabled {
        return;
    }
    let fresh = filter_popup_items(fresh, &cfg.notify_modules);
    if fresh.is_empty() {
        return;
    }
    let fresh = cap_wave(fresh, POPUP_MAX);
    if let Ok(mut queue) = queue_slot().lock() {
        queue.extend(fresh);
    }
    // 先请求建窗（隐藏），再起 1.2s 计时：两者的开销重叠
    ensure_window(app);
    schedule(app.clone());
}

fn filter_popup_items(mut items: Vec<NotifyItem>, modules: &[String]) -> Vec<NotifyItem> {
    let suppress_deepseek = items.iter().any(|item| item.kind == "dsh")
        && super::notify::deepseek_popup_suppressed();
    items.retain(|item| modules.contains(&item.kind) && !(suppress_deepseek && item.kind == "dsh"));
    items
}

/// 重置浮窗差分基线 + 清掉未出卡的波次/冷启动暂存。
///
/// 岛总开关关闭 / 重新开启时调用（`notify::start` / `notify::stop`）：关岛期间
/// 到达的消息会留在通知库里，重开时 `init_history` 会把它们当历史重新读入并
/// 标成已读；基线一起清掉，既不会漏判也不会补弹，排队中的旧卡也不会在关岛
/// 之后突然冒出来。
pub(crate) fn reset_baseline() {
    if let Ok(mut seen) = seen_slot().lock() {
        *seen = SeenState::default();
    }
    if let Ok(mut queue) = queue_slot().lock() {
        queue.clear();
    }
    if let Ok(mut pending) = pending_slot().lock() {
        pending.clear();
    }
}

/// 当前时间（UNIX 毫秒）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_millis() as i64)
        .unwrap_or(0)
}

/// 起一个波次计时器（已有计时器在跑时直接返回：那一波会把这批一起带走）。
fn schedule(app: AppHandle) {
    if TIMER_ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(POPUP_DELAY_MS));
        let batch = queue_slot()
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default();
        TIMER_ARMED.store(false, Ordering::SeqCst);
        if !batch.is_empty() {
            deliver(&app, batch);
        }
        // 收尾竞态：清标志之后又进来的条目再起一波
        let stragglers = queue_slot()
            .lock()
            .map(|queue| !queue.is_empty())
            .unwrap_or(false);
        if stragglers {
            schedule(app);
        }
    });
}

/// 页面就绪判定 + 冷启动暂存（与 `notify_popup_ready` 抢同一把锁）。
///
/// 就绪位必须在 `pending_slot` 的锁内读/写：否则会出现「deliver 看到未就绪
/// → ready 置位并取空 pending → deliver 才把这一批塞进 pending」，这一批就
/// 永远没人取了。返回 `Some` = 直接广播，`None` = 已攒着等页面来取。
fn gate_for_page(items: Vec<NotifyItem>) -> Option<Vec<NotifyItem>> {
    let mut pending = pending_slot().lock().ok()?;
    if PAGE_READY.load(Ordering::SeqCst) {
        return Some(items);
    }
    pending.extend(items);
    let overflow = pending.len().saturating_sub(POPUP_MAX);
    if overflow > 0 {
        pending.drain(..overflow);
    }
    None
}

/// 把一批卡片交给浮窗页面；页面还没就绪时先攒着等 `notify_popup_ready`。
fn deliver(app: &AppHandle, items: Vec<NotifyItem>) {
    // 1.2s 等待期间岛总开关 / 浮窗开关都可能被关掉：投递前再读一次，
    // 关岛瞬间排队的那一波不会漏出来
    let cfg = super::load_config();
    if !cfg.enabled || !cfg.notify_popup_enabled {
        return;
    }
    let items = filter_popup_items(items, &cfg.notify_modules);
    if items.is_empty() {
        return;
    }
    let Some(items) = gate_for_page(items) else {
        return;
    };
    let payload = PopupShow {
        items,
        config: config_snapshot(app),
    };
    let _ = app.emit_to(POPUP_WINDOW_LABEL, POPUP_SHOW_EVENT, payload);
}

/// 配置变更后的收敛：关掉开关就清空并隐藏，其余情况广播新快照
/// （position / size / theme / modules / 显示器 变化都要让前端重新量尺寸）。
pub(crate) fn on_config_changed(app: &AppHandle) {
    let cfg = config_snapshot(app);
    if !cfg.enabled || !cfg.island_enabled {
        if let Ok(mut queue) = queue_slot().lock() {
            queue.clear();
        }
        if let Ok(mut pending) = pending_slot().lock() {
            pending.clear();
        }
        let app_main = app.clone();
        let _ = app.run_on_main_thread(move || hide_on_main(&app_main));
    }
    let _ = app.emit_to(POPUP_WINDOW_LABEL, POPUP_CONFIG_EVENT, cfg);
}

// ============================================================
// 命令
// ============================================================

/// 浮窗页面挂载完成（`listen` 已注册）：置就绪位并交出冷启动期间攒下的条目。
#[tauri::command]
pub fn notify_popup_ready(app: AppHandle) -> PopupReady {
    let pending = {
        // 就绪位在同一把锁里置位再取空：见 gate_for_page 的竞态说明
        let mut slot = match pending_slot().lock() {
            Ok(slot) => slot,
            Err(_) => return PopupReady { config: config_snapshot(&app), pending: Vec::new() },
        };
        PAGE_READY.store(true, Ordering::SeqCst);
        std::mem::take(&mut *slot)
    };
    PopupReady {
        config: config_snapshot(&app),
        pending,
    }
}

/// 按当前堆叠尺寸（逻辑 px，前端量出来的）重新定位 + 缩放窗口并显示。
///
/// 贴边锚定全部在 [`popup_window_xy`] 里：`bottom_*` 保持底边、`top_*` 保持
/// 顶边、`left/right` 贴对应边、`center_*` 水平居中——窗口随堆叠高度伸缩时
/// 贴边那一侧不动，视觉上就是「从内侧长出来 / 往内侧收回去」。
#[tauri::command]
pub async fn notify_popup_resize(
    app: AppHandle,
    width: u32,
    height: u32,
    pad: Option<u32>,
) -> Result<(), String> {
    let cfg = super::load_config();
    if !cfg.enabled || !cfg.notify_popup_enabled || width < 1 || height < 1 {
        return hide(app).await;
    }
    let (w, h) = (width as f64, height as f64);
    // pad 由前端按 size 缩放后传进来（= 10px × size/100，最大 16px）；这里再
    // 夹一层硬上限，脏值不会把卡片推离屏幕边
    let pad = (pad.unwrap_or(0) as f64).clamp(0.0, POPUP_EXIT_PAD * 1.6 + 1.0);
    let (x, y) = popup_window_xy(
        &cfg.notify_popup_position,
        w,
        h,
        pad,
        work_area_logical(&app),
    );
    let scale = super::island_scale(&app);
    // 记下当前隐藏代次：闭包真正执行时若已经被要求隐藏（或开关已关掉），
    // 就不要再 show —— 否则会留下「没有卡片但可见」的透明置顶窗
    let gen = HIDE_GEN.load(Ordering::SeqCst);
    let app_main = app.clone();
    super::on_main_thread(&app, move || {
        if HIDE_GEN.load(Ordering::SeqCst) != gen {
            return Ok(());
        }
        let cfg_now = super::load_config();
        if !cfg_now.enabled || !cfg_now.notify_popup_enabled {
            return Ok(());
        }
        let win = ensure_window_on_main(&app_main)?;
        super::apply_window_frame_scaled(&win, x, y, w, h, scale)?;
        if !win.is_visible().unwrap_or(false) {
            win.show().map_err(|e| format!("显示消息浮窗失败: {e}"))?;
        }
        Ok(())
    })
    .await
}

/// 清空堆叠后隐藏窗口（前端在最后一张卡片离开时调用）。
#[tauri::command]
pub async fn notify_popup_hide(app: AppHandle) -> Result<(), String> {
    hide(app).await
}

async fn hide(app: AppHandle) -> Result<(), String> {
    // 先推进代次：并发在途的 resize 闭包看到代次变了就不会再 show
    HIDE_GEN.fetch_add(1, Ordering::SeqCst);
    let app_main = app.clone();
    super::on_main_thread(&app, move || {
        if let Some(win) = app_main.get_webview_window(POPUP_WINDOW_LABEL) {
            // 窗口首次建出来还没显示过就 hide 也安全（Tauri 幂等）
            win.hide().map_err(|e| format!("隐藏消息浮窗失败: {e}"))?;
        }
        Ok(())
    })
    .await
}

// ============================================================
// 单测：位置 / 堆叠 / 裁剪
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::settings::POPUP_POSITIONS;

    fn item(id: i64) -> NotifyItem {
        NotifyItem {
            id,
            kind: "qq".into(),
            name: "QQ".into(),
            sender: String::new(),
            body: String::new(),
            time: 0,
            read: false,
            exe: String::new(),
        }
    }

    /// 六种位置 × 工作区 → 窗口左上角（工作区原点、任务栏让出的高度都要算进去）
    #[test]
    fn six_positions_map_onto_the_work_area() {
        // 主屏 1920×1080，底部 40px 任务栏 → 工作区 1920×1040
        let area = (0.0, 0.0, 1920.0, 1040.0);
        let (w, h) = popup_stack_size(1.0, 2); // 360 × 172
        assert_eq!((w, h), (360.0, 172.0));

        assert_eq!(popup_window_xy("top_left", w, h, 0.0, area), (16.0, 16.0));
        assert_eq!(popup_window_xy("top_right", w, h, 0.0, area), (1544.0, 16.0));
        // center_* 只水平居中，纵向仍贴上下边（不是屏幕正中）
        assert_eq!(popup_window_xy("center_top", w, h, 0.0, area), (780.0, 16.0));
        assert_eq!(
            popup_window_xy("center_bottom", w, h, 0.0, area),
            (780.0, 852.0)
        );
        assert_eq!(popup_window_xy("bottom_left", w, h, 0.0, area), (16.0, 852.0));
        assert_eq!(
            popup_window_xy("bottom_right", w, h, 0.0, area),
            (1544.0, 852.0)
        );

        // 副屏：工作区原点在负坐标，位置必须跟着那块屏走
        let left_of_primary = (-1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(
            popup_window_xy("bottom_right", w, h, 0.0, left_of_primary),
            (-376.0, 892.0)
        );
        assert_eq!(
            popup_window_xy("top_left", w, h, 0.0, left_of_primary),
            (-1904.0, 16.0)
        );
    }

    /// 贴边侧那条退场透明带：卡片 box 仍与屏幕边差 16px，窗口外沿再多出 pad
    #[test]
    fn exit_gutter_keeps_the_card_edge_margin() {
        let area = (0.0, 0.0, 1920.0, 1040.0);
        let (w, h) = popup_stack_size(1.0, 2); // 360 × 172
        let pad = POPUP_EXIT_PAD; // 10

        let (x, y) = popup_window_xy("bottom_right", w, h + pad, pad, area);
        assert_eq!(x, 1544.0);
        // 卡片底边 = 工作区下沿 - 16（与无 pad 时一致），窗口下沿再下 10
        assert_eq!(y + h, 1040.0 - POPUP_EDGE);
        assert_eq!(y + h + pad, 1040.0 - POPUP_EDGE + pad);

        let (_, top_y) = popup_window_xy("center_top", w, h + pad, pad, area);
        // 卡片顶边 = 工作区上沿 + 16，窗口顶边向上多 10（不裁退场位移）
        assert_eq!(top_y + pad, POPUP_EDGE);
        assert_eq!(top_y, POPUP_EDGE - pad);

        // pad 越界夹到 0..=64，脏值不会把窗口甩到屏幕外
        let (_, clamped) = popup_window_xy("top_right", w, h, 1000.0, area);
        assert_eq!(clamped, POPUP_EDGE - 64.0);
    }

    /// 窗口伸缩时贴边那一侧不动（前端每次堆叠变化都会重新调 resize）
    #[test]
    fn edge_side_stays_pinned_when_window_grows() {
        let area = (0.0, 0.0, 1920.0, 1040.0);
        let (_, y_small) = popup_window_xy("bottom_right", 360.0, 84.0, 10.0, area);
        let (_, y_big) = popup_window_xy("bottom_right", 360.0, 436.0, 10.0, area);
        // 底边固定：窗口高多少，顶边就往下让多少
        assert_eq!(y_small + 84.0, y_big + 436.0);

        let (_, top_small) = popup_window_xy("center_top", 360.0, 84.0, 10.0, area);
        let (_, top_big) = popup_window_xy("center_top", 360.0, 436.0, 10.0, area);
        // 顶边固定
        assert_eq!(top_small, top_big);

        let (x_small, _) = popup_window_xy("bottom_left", 360.0, 84.0, 10.0, area);
        let (x_big, _) = popup_window_xy("bottom_left", 576.0, 84.0, 10.0, area);
        // 左边固定
        assert_eq!(x_small, x_big);

        let (r_small, _) = popup_window_xy("top_right", 360.0, 84.0, 10.0, area);
        let (r_big, _) = popup_window_xy("top_right", 576.0, 84.0, 10.0, area);
        // 右边固定
        assert_eq!(r_small + 360.0, r_big + 576.0);
    }

    #[test]
    fn stack_size_scales_card_but_not_gap() {
        assert_eq!(popup_stack_size(1.0, 0), (360.0, 0.0));
        assert_eq!(popup_stack_size(1.0, 1), (360.0, 84.0));
        // 5 条：5×84 + 4×4 = 436
        assert_eq!(popup_stack_size(1.0, 5), (360.0, 436.0));
        // 160%：卡片等比放大，间距仍是 4px
        let (w, h) = popup_stack_size(1.6, 3);
        assert!((w - 576.0).abs() < 1e-9);
        assert!((h - (84.0 * 1.6 * 3.0 + 8.0)).abs() < 1e-9);
        // 越界缩放夹到 70%..160%
        assert_eq!(popup_stack_size(0.1, 1).0, 360.0 * 0.7);
        assert_eq!(popup_stack_size(9.0, 1).0, 360.0 * 1.6);
    }

    #[test]
    fn scale_follows_settings_range() {
        assert_eq!(popup_scale(70), 0.7);
        assert_eq!(popup_scale(100), 1.0);
        assert_eq!(popup_scale(160), 1.6);
        assert_eq!(popup_scale(10), 0.7);
        assert_eq!(popup_scale(999), 1.6);
    }

    #[test]
    fn max_cards_bounded_by_height_and_cap() {
        // 1080 屏扣掉 2×16 边距：100% 与 160% 都能放满 5 条
        assert_eq!(max_cards(1.0, 1048.0), 5);
        assert_eq!(max_cards(1.6, 1048.0), 5);
        // 高度只够两张（2×84 + 4 = 172 ≤ 200 < 3×84 + 8 = 260）
        assert_eq!(max_cards(1.0, 200.0), 2);
        // 连两张都放不下时也至少留一张
        assert_eq!(max_cards(1.0, 84.0), 1);
        assert_eq!(max_cards(1.0, 0.0), 1);
        // 永远不超过硬上限
        assert_eq!(max_cards(0.7, 1e9), POPUP_MAX);
    }

    fn ids(items: Vec<NotifyItem>) -> Vec<i64> {
        items.iter().map(|item| item.id).collect()
    }

    fn item_at(id: i64, read: bool, time: i64) -> NotifyItem {
        let mut item = item(id);
        item.read = read;
        item.time = time;
        item
    }

    /// 数据库 toast：未读 + 比已见最大值更大才算新到
    #[test]
    fn fresh_items_uses_monotonic_toast_ids() {
        let items = vec![item(7), item(6), item(3)];
        // 已见过 5：只有 6、7 算新到（3 是历史重放）
        let mut seen = SeenState {
            toast_max: 5,
            ..SeenState::default()
        };
        assert_eq!(ids(take_fresh(&mut seen, &items, 0)), vec![6, 7]);
        // 同样的状态再来一次：都见过了
        assert!(take_fresh(&mut seen, &items, 0).is_empty());
        // 只出现更小的 id（历史重放）也不弹
        assert!(take_fresh(&mut seen, &items[2..], 0).is_empty());
        assert_eq!(seen.toast_max, 7);
    }

    /// P1：历史回填（init_history 把读到的条目全标成已读）绝不能弹
    #[test]
    fn history_backfill_marked_read_is_never_fresh() {
        let history: Vec<NotifyItem> = (10..=14).map(|id| item_at(id, true, 1_000)).collect();
        let mut seen = SeenState::default();
        assert!(take_fresh(&mut seen, &history, 2_000).is_empty());
        // 基线照样推进到历史最大值
        assert_eq!(seen.toast_max, 14);
        // 已读条目即使 id 超过基线也不弹（防脏数据 / 手工改库）
        let mut other = SeenState::default();
        assert!(take_fresh(&mut other, &[item_at(99, true, 3_000)], 3_000).is_empty());
        // 真正的新消息（未读）照常出卡
        let fresh = take_fresh(&mut seen, &[item_at(15, false, 4_000)], 4_000);
        assert_eq!(ids(fresh), vec![15]);
    }

    /// P2-1：干净通知库（没有历史）不吞第一条真实消息——基线从空状态起步，
    /// 首个未读条目就是新到，不需要「先来一发空回填」才能开始
    #[test]
    fn empty_history_keeps_first_real_message_fresh() {
        let mut seen = SeenState::default();
        // 第一拍：库是空的（init_history 只建立游标，不 emit）
        assert!(take_fresh(&mut seen, &[], 1_000).is_empty());
        assert_eq!(seen.toast_max, -1);
        // 第一条真实通知（id 从 1 起、未读）
        let fresh = take_fresh(&mut seen, &[item_at(1, false, 2_000)], 2_000);
        assert_eq!(ids(fresh), vec![1]);
        assert_eq!(seen.toast_max, 1);
    }

    /// P1：岛关 → 重开。关岛期间的消息在重开时被 init_history 当历史读入
    /// （已读），不能补弹；重开之后的新消息照常弹。
    #[test]
    fn island_restart_does_not_replay_offline_messages() {
        let mut seen = SeenState::default();
        let before = take_fresh(&mut seen, &[item_at(5, false, 1_000)], 1_000);
        assert_eq!(ids(before), vec![5]);
        // notify::stop → popup::reset_baseline()（等价于换回默认状态）
        seen = SeenState::default();
        // notify::start → init_history 重读最近 50 条并全部标成已读，
        // 其中 6、7 是关岛期间到达的
        let backfill = vec![
            item_at(7, true, 9_000),
            item_at(6, true, 8_000),
            item_at(5, true, 1_000),
        ];
        assert!(take_fresh(&mut seen, &backfill, 10_000).is_empty());
        assert_eq!(seen.toast_max, 7);
        // 重开之后的新消息仍然出卡
        let fresh = take_fresh(&mut seen, &[item_at(8, false, 11_000)], 11_000);
        assert_eq!(ids(fresh), vec![8]);
    }

    /// 合成负 id（飞书 / 钉钉浮窗、任务栏闪烁）：只能靠「没见过」+ 新鲜度窗口
    #[test]
    fn synthetic_ids_use_seen_set_and_freshness_window() {
        let mut newer = item(-3);
        newer.time = 1_000;
        let mut older = item(-1);
        older.time = 900;
        let items = vec![newer.clone(), older.clone()];
        let mut seen = SeenState::default();
        // 没见过 → 都算新到，按到达时间排（-1 先到）
        assert_eq!(ids(take_fresh(&mut seen, &items, 1_000)), vec![-1, -3]);
        // 见过 → 不重复
        assert!(take_fresh(&mut seen, &items, 1_000).is_empty());
        // 关掉模块后再重新显示，最近看过的提醒也不能重复弹。
        assert!(take_fresh(&mut seen, &[], 1_100).is_empty());
        assert!(take_fresh(&mut seen, &items, 1_200).is_empty());
        assert!(take_fresh(&mut seen, &[item_at(-4, true, 1_100)], 1_200).is_empty());
        // 超出新鲜度窗口的旧条目重新出现 → 不弹
        let mut seen = SeenState::default();
        assert!(take_fresh(&mut seen, &[newer], 30_000).is_empty());
    }

    #[test]
    fn wave_keeps_newest_and_drops_edge_oldest() {
        let wave: Vec<NotifyItem> = (1..=7).map(item).collect();
        let capped = cap_wave(wave, POPUP_MAX);
        // 丢掉最早到达的 1、2，保留 3..7 且顺序不变
        assert_eq!(
            capped.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![3, 4, 5, 6, 7]
        );
        // 未超上限时原样（仍按到达顺序）
        let small: Vec<NotifyItem> = (1..=3).map(item).collect();
        assert_eq!(cap_wave(small, POPUP_MAX).len(), 3);
        // 乱序输入也要按到达顺序排
        let mixed = vec![item(9), item(4), item(7)];
        assert_eq!(
            cap_wave(mixed, POPUP_MAX)
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![4, 7, 9]
        );
    }

    #[test]
    fn position_keys_are_classified_consistently() {
        let expect = [
            ("center_bottom", true, "center"),
            ("center_top", false, "center"),
            ("bottom_left", true, "left"),
            ("bottom_right", true, "right"),
            ("top_left", false, "left"),
            ("top_right", false, "right"),
        ];
        assert_eq!(expect.len(), POPUP_POSITIONS.len());
        for (key, bottom, anchor) in expect {
            assert!(POPUP_POSITIONS.contains(&key), "设置文档缺少位置 {key}");
            assert_eq!(popup_is_bottom(key), bottom, "{key} 的贴边方向");
            assert_eq!(popup_h_anchor(key), anchor, "{key} 的横向锚点");
        }
    }
}
