//! 灵动岛 HUD（island）：顶部居中、贴近屏幕上缘的液态毛玻璃胶囊。
//!
//! 视觉与交互：
//! - 收起态是一枚贴着屏幕上缘的胶囊，默认只显示 **Agent 额度 + 天气**，
//!   额度沿用旧版 HUD 的同心圆环（环心品牌图标，每圈一个窗口）；
//! - 悬停约 220ms 后向两侧撑开、向下长成一块深色毛玻璃面板，顶部标签栏
//!   切换 主页 / 消息 / 股票 / 天气 / 控制 / 剪贴板，齿轮打开独立的
//!   「灵动岛设置」窗口（1100×720 无边框大窗，见 crate::island_settings_window；
//!   首开懒创建、关闭=隐藏，岛总开关关闭时该窗口一并收起）；
//! - 只有一套深色毛玻璃配色，不再提供主题切换；`transparency` 控制玻璃
//!   色层与桌面底图的合成程度；
//! - 右键菜单、滚轮切换收起内容、全屏时自动隐藏。
//!
//! 窗口形态：透明、无边框、置顶、不抢启动焦点，但**可聚焦**——设置输入框
//! 需要键盘。窗口始终贴在屏幕工作区上缘居中，展开/收起同时改宽高，
//! 左上角不动；前端等窗口实际 resize 完成后再启动 CSS 形变，避免 WebView2
//! 在改几何时闪出没有玻璃底的一帧。
//!
//! 数据链路全部复用现有后端能力：
//! - 额度：复用 `hud_bar` 的数据线程与 `zbar://hud-bar-usage` 事件；
//! - 天气/股票/媒体/系统/剪贴板/通知：本模块的子模块各自维护后台
//!   线程与 `zbar://island-*` 事件，前端只负责渲染与交互。

pub mod app_paths;
pub mod audio_peak;
pub mod backdrop;
pub mod capture_guard;
pub mod clash;
pub mod clipboard;
pub mod live;
pub mod lyrics;
pub mod media;
pub mod meeting;
pub mod notify;
pub mod passthrough;
pub mod popup;
pub mod screenshot;
pub mod settings;
pub mod stocks;
pub mod store;
pub mod system;
pub mod translate;
pub mod weather;

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::webview_args::MergedRendererBuilder;

/// 灵动岛窗口 label（capabilities/island.json 白名单同步）
pub const ISLAND_WINDOW_LABEL: &str = "island";
/// 配置变更广播（透明度 / 板块显隐 / 城市 / 自选股等）
pub const ISLAND_CONFIG_EVENT: &str = "zbar://island-config";
/// Rust 侧强制收起（全屏隐藏 / 关闭灵动岛）时通知前端收敛本地 open 态
pub const ISLAND_COLLAPSE_EVENT: &str = "zbar://island-collapse";
/// 设置和快捷键触发的展开事件。
pub const ISLAND_EXPAND_EVENT: &str = "zbar://island-expand";
/// macOS 上由 Rust 轮询光标后广播的悬停事件。
const ISLAND_HOVER_EXPAND_EVENT: &str = "zbar://island-hover-expand";

/// 液态画布窗口尺寸（逻辑 px）：780×632。主体贴边侧内缩 `BODY_INSET`，
/// 其余空间留给液桥/拉丝/残滴的绘制。**窗口尺寸固定**——开合、切页、主体尺寸
/// 上报都只改画布内布局，不动窗口（避免 DWM 重建合成面抖动）。
const ISLAND_WINDOW_W: f64 = 780.0;
const ISLAND_WINDOW_H: f64 = 632.0;
/// 主体到窗口「贴边侧」的内缩（逻辑 px）。
const BODY_INSET: f64 = 70.0;
/// 贴边两侧内凹液脚 / 圆角半径（FILLET，与前端形状一致）。
const BODY_FILLET: f64 = 13.0;
/// 主体沿边摆放时额外预留的安全边距（液脚 + 视觉留白）。
const BODY_EDGE_MARGIN: f64 = 6.0;
/// 前端未上报主体尺寸时的回退值（收起态胶囊设计值）。
const BODY_FALLBACK_W: f64 = 300.0;
const BODY_FALLBACK_H: f64 = 54.0;
/// 主体设计上限：展开面板 640×492。
const BODY_MAX_W: f64 = 640.0;
const BODY_MAX_H: f64 = 492.0;

/// 主体（岛本体）在画布里的矩形（逻辑 px，相对 webview 左上角）。
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BodyRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BodyRect {
    fn from_tuple((x, y, width, height): (f64, f64, f64, f64)) -> Self {
        Self { x, y, width, height }
    }
}

/// 前端上报的主体尺寸（宽, 高），未上报时回退 300×54。
static BODY_SIZE: std::sync::OnceLock<std::sync::Mutex<(f64, f64)>> = std::sync::OnceLock::new();
/// 最近一次上报的「是否展开态」提示（仅用于调试/状态展示，窗口几何与它无关）。
static BODY_EXPANDED_HINT: AtomicBool = AtomicBool::new(false);

fn body_size_slot() -> &'static std::sync::Mutex<(f64, f64)> {
    BODY_SIZE.get_or_init(|| std::sync::Mutex::new((BODY_FALLBACK_W, BODY_FALLBACK_H)))
}

/// 归一化主体尺寸：夹进 1..=设计上限，非法值回退默认。
fn clamp_body_size(width: f64, height: f64) -> (f64, f64) {
    let w = if width.is_finite() && width > 0.0 { width.clamp(1.0, BODY_MAX_W) } else { BODY_FALLBACK_W };
    let h = if height.is_finite() && height > 0.0 { height.clamp(1.0, BODY_MAX_H) } else { BODY_FALLBACK_H };
    (w, h)
}

fn current_body_size() -> (f64, f64) {
    body_size_slot().lock().map(|g| *g).unwrap_or((BODY_FALLBACK_W, BODY_FALLBACK_H))
}

static SERVICES_STARTED: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static FULLSCREEN_WATCHER_STARTED: AtomicBool = AtomicBool::new(false);
static FULLSCREEN_HIDDEN: AtomicBool = AtomicBool::new(false);
static LAST_EXPANDED: AtomicBool = AtomicBool::new(false);
/// 收起态胶囊的悬停热区（CSS px，相对 webview 左上角），由前端上报。
/// macOS 的悬停展开判定、Windows 的透明留白穿透都按这块矩形算。
static HOVER_RECT: std::sync::OnceLock<std::sync::Mutex<Option<(f64, f64, f64, f64)>>> =
    std::sync::OnceLock::new();

fn hover_rect_slot() -> &'static std::sync::Mutex<Option<(f64, f64, f64, f64)>> {
    HOVER_RECT.get_or_init(|| std::sync::Mutex::new(None))
}

/// 收起态右键菜单是否打开（前端 `island_set_context_menu_open` 上报）。
///
/// 菜单是浮在胶囊热区之外的绝对定位浮层：打开期间必须冻结「透明留白自动
/// 穿透」，否则光标一移到菜单上就被判在热区外、窗口被设成
/// `WS_EX_TRANSPARENT`，菜单随即收不到点击。关闭菜单后下一拍恢复原判定，
/// 不是永久关闭穿透。
static CONTEXT_MENU_OPEN: AtomicBool = AtomicBool::new(false);

/// 前端把胶囊元素的实际矩形上报给 Rust：macOS 用它判悬停展开，Windows 用它
/// 判「光标是不是落在透明留白里」。
#[tauri::command]
pub fn island_set_hover_rect(x: f64, y: f64, width: f64, height: f64) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    if std::env::var_os("ZBAR_ISLAND_DEBUG").is_some() {
        eprintln!("[agenthud-island] hover rect {x:.0},{y:.0} {width:.0}x{height:.0}");
    }
    if let Ok(mut slot) = hover_rect_slot().lock() {
        *slot = Some((x, y, width, height));
    }
}

/// 前端汇报收起态右键菜单的开/关：打开期间冻结透明留白穿透，保证浮在胶囊
/// 热区外的菜单能收到 hover / 点击。只写一个原子，不需要 spawn_blocking。
#[tauri::command]
pub fn island_set_context_menu_open(open: bool) {
    if std::env::var_os("ZBAR_ISLAND_DEBUG").is_some() {
        eprintln!("[zbar-island] context menu open={open}");
    }
    CONTEXT_MENU_OPEN.store(open, Ordering::Relaxed);
}

/// 灵动岛窗口的逻辑尺寸。液态画布在 Windows/macOS 上都是固定 `780×632`，
/// 开合/切页/主体尺寸变化都不改窗口（`edge` 参数保留给旧调用点，忽略）。
fn window_size_logical(_edge: &str) -> (f64, f64) {
    (ISLAND_WINDOW_W, ISLAND_WINDOW_H)
}

/// 把前端上报的热区（相对窗口左上角的逻辑 px）裁进窗口内。
///
/// 收起动画刚开始时 `.island` 仍然带着展开时的尺寸（约 500×450），如果原样
/// 采信，鼠标离胶囊还有几百 px 就会触发展开。裁进窗口后最坏情况也只是整扇
/// 窗口大小，不会再“提前亮”。裁完太小（数据明显不对）返回 None，调用方回退
/// 到整窗判定。
fn clamp_hover_rect(rect: (f64, f64, f64, f64), edge: &str) -> Option<(f64, f64, f64, f64)> {
    let (cw, ch) = window_size_logical(edge);
    if !rect.0.is_finite() || !rect.1.is_finite() || !rect.2.is_finite() || !rect.3.is_finite() {
        return None;
    }
    let x0 = rect.0.clamp(0.0, cw);
    let y0 = rect.1.clamp(0.0, ch);
    let x1 = (rect.0 + rect.2).clamp(x0, cw);
    let y1 = (rect.1 + rect.3).clamp(y0, ch);
    if x1 - x0 < 8.0 || y1 - y0 < 8.0 {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}

// ============================================================
// 拖拽
// ============================================================

/// Drag and snap use 240 Hz substeps and a shared rest criterion.
/// Position values can be in either physical or logical units.
#[derive(Debug, Clone, Copy)]
struct MotionSpring {
    value: f64,
    velocity: f64,
    target: f64,
    stiffness: f64,
    damping: f64,
    epsilon: f64,
}

impl MotionSpring {
    fn new(value: f64, stiffness: f64, ratio: f64) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            stiffness,
            damping: 2.0 * ratio * stiffness.sqrt(),
            epsilon: 0.2,
        }
    }

    fn at_rest(&self) -> bool {
        (self.value - self.target).abs() < self.epsilon && self.velocity.abs() < self.epsilon * 8.0
    }

    fn step(&mut self, dt: f64) {
        if self.at_rest() {
            self.value = self.target;
            self.velocity = 0.0;
            return;
        }
        let steps = (dt * 240.0).ceil().max(1.0) as usize;
        let h = dt / steps as f64;
        for _ in 0..steps {
            let acceleration =
                -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += acceleration * h;
            self.value += self.velocity * h;
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct IslandDragSize {
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct IslandDragSizes {
    horizontal: IslandDragSize,
    vertical: IslandDragSize,
}

impl IslandDragSizes {
    fn for_edge(self, edge: &str) -> (f64, f64) {
        let size = if is_horizontal(edge) {
            self.horizontal
        } else {
            self.vertical
        };
        clamp_body_size(size.width, size.height)
    }
}

#[derive(Debug, Clone)]
struct MotionWindow {
    position: PhysicalPosition<i32>,
    scale: f64,
    local: (f64, f64, f64, f64),
}

#[derive(Debug, Clone)]
struct DragSession {
    id: u64,
    edge: String,
    press: (f64, f64),
    grab: (f64, f64),
    expanded: bool,
    navigation: bool,
    started: bool,
    released: bool,
    x: MotionSpring,
    y: MotionSpring,
    width: MotionSpring,
    height: MotionSpring,
    sizes: IslandDragSizes,
    window: MotionWindow,
    last_tick: Instant,
}

#[derive(Debug, Clone)]
struct SnapSession {
    id: u64,
    edge: String,
    area: MonitorArea,
    along: MotionSpring,
    gap: MotionSpring,
    width: MotionSpring,
    height: MotionSpring,
    pos_frac: f64,
    target_win: (f64, f64),
    window: MotionWindow,
    last_tick: Instant,
}

const MOTION_WATCHDOG_INTERVAL: Duration = Duration::from_millis(100);
const MOTION_WATCHDOG_TIMEOUT: Duration = Duration::from_millis(500);

static DRAG: std::sync::OnceLock<std::sync::Mutex<Option<DragSession>>> =
    std::sync::OnceLock::new();
static SNAP: std::sync::OnceLock<std::sync::Mutex<Option<SnapSession>>> =
    std::sync::OnceLock::new();
static MOTION_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 刚结束一次按压/拖拽：等光标离开胶囊再恢复悬停展开。
static DRAG_RELEASED: AtomicBool = AtomicBool::new(false);

fn drag_slot() -> &'static std::sync::Mutex<Option<DragSession>> {
    DRAG.get_or_init(|| std::sync::Mutex::new(None))
}

fn snap_slot() -> &'static std::sync::Mutex<Option<SnapSession>> {
    SNAP.get_or_init(|| std::sync::Mutex::new(None))
}

fn motion_in_progress() -> bool {
    drag_slot()
        .lock()
        .map(|slot| slot.as_ref().is_some_and(|drag| drag.started))
        .unwrap_or(false)
        || snap_slot()
            .lock()
            .map(|slot| slot.is_some())
            .unwrap_or(false)
}

fn clear_motion() {
    if let Ok(mut slot) = drag_slot().lock() {
        *slot = None;
    }
    if let Ok(mut slot) = snap_slot().lock() {
        *slot = None;
    }
}

/// macOS 全局光标位置（逻辑点，原点在主屏左上）。窗口几何是物理像素，
/// 调用方需要乘 `scale_factor` 后再比较。
#[cfg(target_os = "macos")]
fn cursor_points_macos() -> Option<(f64, f64)> {
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const std::ffi::c_void);
    }
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let point = CGEventGetLocation(event);
        CFRelease(event as *const std::ffi::c_void);
        Some((point.x, point.y))
    }
}

#[cfg(not(target_os = "macos"))]
fn cursor_points_macos() -> Option<(f64, f64)> {
    None
}

/// 顶栏浮层（悬浮面板 / 托盘菜单）是否可见。
///
/// 两者与灵动岛共用屏幕上缘那一块区域：浮层打开后光标落在浮层上，坐标却
/// 可能仍压在（被浮层盖住的）岛的胶囊矩形里。不把这种情况排除，用户在面板
/// 顶部移动鼠标就会触发岛的悬停展开 / 收起，表现为「岛在乱闪」。
fn top_popup_visible(app: &AppHandle) -> bool {
    ["panel", "tray-menu"].iter().any(|label| {
        let win = app.get_webview_window(label);
        // Windows：is_visible() 是主线程同步往返（hover watcher 每 120ms
        // 调一次），主线程挂死时这里全部卡死并往死队列堆积闭包——改 Win32
        // 直判（同下方 start_mouse_through_watcher 的先例），零主线程往返。
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
            win.and_then(|win| win.hwnd().ok())
                .map(|hwnd| unsafe { IsWindowVisible(hwnd) }.as_bool())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "windows"))]
        {
            win.and_then(|win| win.is_visible().ok()).unwrap_or(false)
        }
    })
}

/// 毫秒 → 轮询拍数（至少 1 拍）。macOS 悬停守护的展开/收起延迟按配置换算成
/// 120ms 一拍；Windows 的悬停延迟由前端 CSS/JS 负责，这里只服务 macOS 兜底。
fn ticks_for_ms(ms: u64, tick_ms: u64) -> u8 {
    if tick_ms == 0 {
        return 1;
    }
    ms.saturating_add(tick_ms - 1)
        .checked_div(tick_ms)
        .unwrap_or(1)
        .clamp(1, u8::MAX as u64) as u8
}

/// 悬停兜底：macOS 上灵动岛是不可聚焦窗口，WKWebView 的 tracking area 只对
/// key window 生效，pointerenter / pointerleave 一律收不到。这里用 150ms 轮询
/// 光标位置双向补齐：进入胶囊热区展开，离开面板收起。
///
/// **只在 `expandMode == "hover"` 时生效**：always（常驻展开）与 never（长关）
/// 都不参与，鼠标离开自然也不会把岛收起来。
fn start_hover_watcher(app: AppHandle) {
    static HOVER_WATCHER_STARTED: AtomicBool = AtomicBool::new(false);
    if HOVER_WATCHER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::Builder::new()
        .name("zbar-island-hoverwatch".into())
        .spawn(move || {
            const TICK_MS: u64 = 120;
            let mut outside = 0u8;
            let mut inside = 0u8;
            let mut entered_fired = false;
            let mut cooldown_ms = 0u64;
            // ZBAR_ISLAND_DEBUG=1 时把「为什么没判」打出来，排查「岛收不起来」用
            let debug = std::env::var_os("ZBAR_ISLAND_DEBUG").is_some();
            let mut tick_no = 0u64;
            loop {
                thread::sleep(Duration::from_millis(TICK_MS));
                tick_no += 1;
                let trace = debug && tick_no % 16 == 0;
                if cooldown_ms > 0 {
                    cooldown_ms = cooldown_ms.saturating_sub(TICK_MS);
                    continue;
                }
                let cfg = load_config();
                if !cfg.enabled || cfg.expand_mode != "hover" {
                    outside = 0;
                    inside = 0;
                    continue;
                }
                // 展开 / 收起延迟按配置换算成轮询拍数（0ms = 下一拍就生效）。
                let expand_ticks = ticks_for_ms(cfg.hover_expand_ms, TICK_MS);
                let collapse_ticks = ticks_for_ms(cfg.collapse_delay_ms, TICK_MS);
                // 面板 / 托盘菜单可见时暂停悬停判定：它们与岛在屏幕上缘
                // 重叠，光标在浮层上移动不应把岛撑开（见 top_popup_visible）
                if top_popup_visible(&app) {
                    outside = 0;
                    inside = 0;
                    entered_fired = false;
                    continue;
                }
                if motion_in_progress() {
                    if trace {
                        eprintln!("[agenthud-island] hover skip: drag session");
                    }
                    outside = 0;
                    inside = 0;
                    continue;
                }
                // 按住左键（准备拖拽）时不要悬停展开/收起：展开会把按住的
                // 胶囊换掉，拖拽因此中断。
                if left_button_down() {
                    if trace {
                        eprintln!("[agenthud-island] hover skip: left button down");
                    }
                    outside = 0;
                    inside = 0;
                    continue;
                }
                let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) else {
                    continue;
                };
                // Windows：is_visible() 走主线程同步往返（同 top_popup_visible
                // 的说明），改 Win32 直判，悬停守护在主线程挂死时不再被拖死。
                #[cfg(target_os = "windows")]
                let visible = {
                    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
                    win.hwnd()
                        .ok()
                        .map(|hwnd| unsafe { IsWindowVisible(hwnd) }.as_bool())
                        .unwrap_or(false)
                };
                #[cfg(not(target_os = "windows"))]
                let visible = win.is_visible().unwrap_or(false);
                if !visible {
                    outside = 0;
                    inside = 0;
                    continue;
                }
                let Some((cx, cy)) = cursor_points_macos() else {
                    continue;
                };
                let Ok(pos) = win.outer_position() else {
                    continue;
                };
                let Ok(size) = win.outer_size() else {
                    continue;
                };
                let scale = win.scale_factor().unwrap_or(1.0);
                let cx = cx * scale;
                let cy = cy * scale;

                let expanded = LAST_EXPANDED.load(Ordering::Relaxed);
                if trace {
                    eprintln!(
                        "[agenthud-island] trace pos=({},{}) size={}x{} scale={scale} cursor=({cx:.0},{cy:.0}) expanded={expanded}",
                        pos.x, pos.y, size.width, size.height
                    );
                }
                // 展开态用整扇窗判定；收起态优先用前端上报的胶囊矩形，
                // 没上报过就退回整扇窗（透明边距会略大一点）。
                let reported = hover_rect_slot().lock().ok().and_then(|g| *g);
                let (rx, ry, rw, rh) = match (expanded, reported) {
                    (true, _) | (false, None) => (
                        pos.x as f64,
                        pos.y as f64,
                        size.width as f64,
                        size.height as f64,
                    ),
                    (false, Some(rect)) => match clamp_hover_rect(rect, &cfg.edge) {
                        Some((x, y, w, h)) => (
                            pos.x as f64 + x * scale,
                            pos.y as f64 + y * scale,
                            w * scale,
                            h * scale,
                        ),
                        None => {
                            let (cw, ch) = window_size_logical(&cfg.edge);
                            (pos.x as f64, pos.y as f64, cw * scale, ch * scale)
                        }
                    },
                };
                let margin = if expanded { 6.0 } else { 2.0 };
                let is_inside = cx >= rx - margin
                    && cx <= rx + rw + margin
                    && cy >= ry - margin
                    && cy <= ry + rh + margin;

                if expanded {
                    inside = 0;
                    entered_fired = false;
                    if win.is_focused().unwrap_or(false) {
                        if trace {
                            eprintln!("[agenthud-island] hover skip: window focused");
                        }
                        outside = 0; // 正在输入，别收
                        continue;
                    }
                    if is_inside {
                        outside = 0;
                        continue;
                    }
                    outside = outside.saturating_add(1);
                    if outside >= collapse_ticks {
                        outside = 0;
                        cooldown_ms = 700;
                        let _ = app.emit(ISLAND_COLLAPSE_EVENT, ());
                    }
                } else {
                    outside = 0;
                    if !is_inside {
                        inside = 0;
                        entered_fired = false;
                        // 光标已经离开热区，松手保护可以解除
                        DRAG_RELEASED.store(false, Ordering::Relaxed);
                        continue;
                    }
                    if !cfg.hover_expand {
                        inside = 0;
                        entered_fired = false;
                        continue;
                    }
                    if DRAG_RELEASED.load(Ordering::Relaxed) {
                        // 刚点过/拖过：光标还停在胶囊上，先别展开（点胶囊收起
                        // 不该被悬停立刻重新撑开）；等离开一次再恢复悬停。
                        inside = 0;
                        entered_fired = false;
                        continue;
                    }
                    inside = inside.saturating_add(1);
                    // 停够 hoverExpandMs（默认 120ms，至少 1 拍）再展开
                    if inside >= expand_ticks && !entered_fired {
                        entered_fired = true;
                        cooldown_ms = 400;
                        let _ = app.emit(ISLAND_HOVER_EXPAND_EVENT, ());
                    } else if entered_fired && inside >= 12 {
                        // 事件可能丢了（前端还没就绪 / 状态不同步）：约 1.5s 后允许重试
                        inside = 0;
                        entered_fired = false;
                    }
                }
            }
        })
        .ok();
}

#[cfg(target_os = "windows")]
fn cursor_physical(_app: &AppHandle) -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

/// macOS：CGEventGetLocation 给的是逻辑点，按光标所在屏的 scale 换成物理像素
/// （tao 的窗口几何就是「点 × 该屏 scale」）。
#[cfg(target_os = "macos")]
fn cursor_physical(app: &AppHandle) -> Option<(i32, i32)> {
    let (px, py) = cursor_points_macos()?;
    let scale = monitor_areas(app)
        .into_iter()
        .find(|area| {
            let (left, top, right, bottom) = area.bounds;
            px >= left as f64 / area.scale
                && px < right as f64 / area.scale
                && py >= top as f64 / area.scale
                && py < bottom as f64 / area.scale
        })
        .map(|area| area.scale)
        .or_else(|| {
            app.primary_monitor()
                .ok()
                .flatten()
                .map(|m| m.scale_factor())
        })
        .unwrap_or(1.0);
    Some(((px * scale).round() as i32, (py * scale).round() as i32))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn cursor_physical(_app: &AppHandle) -> Option<(i32, i32)> {
    None
}

#[cfg(target_os = "windows")]
fn left_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    unsafe { (GetAsyncKeyState(0x01) & 0x8000u16 as i16) != 0 }
}

/// macOS：按 CGEventSource 的按键状态判断左键是否仍按着。
#[cfg(target_os = "macos")]
fn left_button_down() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceButtonState(state_id: i32, button: u32) -> bool;
    }
    const COMBINED_SESSION_STATE: i32 = 0;
    const LEFT_BUTTON: u32 = 0;
    unsafe { CGEventSourceButtonState(COMBINED_SESSION_STATE, LEFT_BUTTON) }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn left_button_down() -> bool {
    false
}

// ============================================================
// 透明留白穿透（Windows）
// ============================================================

/// 「这一刻是否要让灵动岛窗口对鼠标透明」——纯函数，单测覆盖。
///
/// 液态画布窗口固定 780×632，绝大多数像素是透明的；Win32 的命中测试按整扇
/// 窗口算，这圈留白会把点击吃掉。这里只按「主体矩形 + 液脚 13px」判定可交互区：
/// **收起态与展开态同一套规则**（展开面板也被视为可见主体），落在外面就穿透。
/// 是否收起由前端/悬停逻辑决定——本函数只返回「要不要切 WS_EX_TRANSPARENT」。
///
/// `menu_open`：收起态右键菜单打开期间返回 false（保持可交互）。菜单是绝对
/// 定位浮层、位置在胶囊热区之外，照常判定会让它一画出来就点不动；菜单关闭后
/// 调用方传 false，立即回到上面这套几何口径。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn should_mouse_through(
    window: (i32, i32, u32, u32),
    scale: f64,
    body: Option<(f64, f64, f64, f64)>,
    cursor: (i32, i32),
    menu_open: bool,
) -> bool {
    if menu_open {
        return false;
    }
    if !scale.is_finite() || scale <= 0.0 {
        return false;
    }
    let (wx, wy, ww, wh) = window;
    // 窗口尺寸对不上（正在拖拽/重建）就不动样式，避免误穿透。
    if (ww as f64 / scale - ISLAND_WINDOW_W).abs() > 1.5
        || (wh as f64 / scale - ISLAND_WINDOW_H).abs() > 1.5
    {
        return false;
    }
    let Some((rx, ry, rw, rh)) = body else {
        return false;
    };
    if !rx.is_finite() || !ry.is_finite() || !rw.is_finite() || !rh.is_finite() || rw < 1.0 || rh < 1.0
    {
        return false;
    }
    // 液脚（13px）也算岛的一部分：贴边/按压形变时点击不会漏给下面。
    let x0 = wx as f64 + (rx - BODY_FILLET) * scale;
    let y0 = wy as f64 + (ry - BODY_FILLET) * scale;
    let x1 = wx as f64 + (rx + rw + BODY_FILLET) * scale;
    let y1 = wy as f64 + (ry + rh + BODY_FILLET) * scale;
    let (cx, cy) = (cursor.0 as f64, cursor.1 as f64);
    !(cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1)
}

/// 按光标位置动态开关 `WS_EX_TRANSPARENT`：光标在胶囊上可交互，落在透明留白
/// 或窗口外时点击穿到下面的应用。
///
/// Windows 需要保持分层窗口以支持跨进程鼠标穿透。与显式穿透共用原生样式
/// 修改，让光标进出时只切换 WS_EX_TRANSPARENT，避免 Tao 重建窗口边框。
///
/// 轮询里除「解析句柄 / 取缩放因子」外全部直接调 Win32：tauri 的
/// outer_position / is_visible / scale_factor 都是同步派发到事件循环再等回复
/// 的，16ms 一拍地调会把主线程吵死。
#[cfg(target_os = "windows")]
fn start_mouse_through_watcher(app: AppHandle) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindow, IsWindowVisible};

    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // 16ms ≈ 一次光标刷新的节奏：移开就能点到下面的应用，移回胶囊也能立刻
    // 恢复可交互；几十 px 的判定不值得更密。
    const TICK_MS: u64 = 16;
    // 缩放因子只在窗口尺寸变化或约 1s 后重取（DPI 只随窗口换屏而变）。
    const SCALE_TICKS: u32 = 60;

    thread::Builder::new()
        .name("zbar-island-passthrough".into())
        .spawn(move || {
            let debug = std::env::var_os("ZBAR_ISLAND_DEBUG").is_some();
            let mut hwnd: Option<HWND> = None;
            let mut scale = 0.0f64;
            let mut rect = (0i32, 0i32, 0u32, 0u32);
            let mut scale_age = SCALE_TICKS;
            let mut outside_ticks = 0u8;

            loop {
                thread::sleep(Duration::from_millis(TICK_MS));

                // 句柄失效（关掉灵动岛 / 窗口重建）就重新解析一次
                let alive = hwnd
                    .map(|h| unsafe { IsWindow(Some(h)) }.as_bool())
                    .unwrap_or(false);
                if !alive {
                    hwnd = app
                        .get_webview_window(ISLAND_WINDOW_LABEL)
                        .and_then(|win| win.hwnd().ok());
                    outside_ticks = 0;
                    scale = 0.0;
                    scale_age = SCALE_TICKS;
                    if hwnd.is_none() {
                        continue;
                    }
                }
                let Some(h) = hwnd else { continue };

                if !unsafe { IsWindowVisible(h) }.as_bool() {
                    continue;
                }
                let mut raw = RECT::default();
                if unsafe { GetWindowRect(h, &mut raw) }.is_err() {
                    continue;
                }
                let now_rect = (
                    raw.left,
                    raw.top,
                    (raw.right - raw.left).max(0) as u32,
                    (raw.bottom - raw.top).max(0) as u32,
                );
                let resized = now_rect.2 != rect.2 || now_rect.3 != rect.3;
                rect = now_rect;
                if scale <= 0.0 || resized || scale_age >= SCALE_TICKS {
                    if let Some(value) = app
                        .get_webview_window(ISLAND_WINDOW_LABEL)
                        .and_then(|win| win.scale_factor().ok())
                        .filter(|value| value.is_finite() && *value > 0.0)
                    {
                        scale = value;
                    }
                    scale_age = 0;
                } else {
                    scale_age = scale_age.saturating_add(1);
                }
                if scale <= 0.0 {
                    continue;
                }

                let cfg = load_config();
                if !cfg.enabled {
                    continue;
                }
                // 显式可穿透期间由 passthrough 模块的钩子接管。
                if cfg.passthrough {
                    outside_ticks = 0;
                    continue;
                }
                let dragging = drag_slot()
                    .lock()
                    .map(|slot| slot.is_some())
                    .unwrap_or(false);
                let mouse_down = left_button_down();
                // 只按不拖的点击也会建立会话，但前端在 begin 回执前松手时
                // 可能来不及调用 drag_step/end。这里兜底清掉「已松手且从未
                // 越过阈值」的会话；已经 started 的真拖拽必须留给
                // island_drag_end 做吸附落盘，不能在这里清。
                if dragging && !mouse_down {
                    if let Ok(mut slot) = drag_slot().lock() {
                        if slot.as_ref().map(|s| !s.started).unwrap_or(false) {
                            *slot = None;
                        }
                    }
                }
                // 左键按着时不切换穿透：用户可能正在胶囊上按下准备拖拽，
                // 中途把窗口切成 WS_EX_TRANSPARENT 会让 WebView 丢掉
                // pointerup/pointermove，看起来就是「按住了却拖不动」。
                // （拖拽会话建立前同样要拦住——从按下到 begin 回执之间
                // 还有一次 IPC 往返。）
                let Some(cursor) = cursor_physical(&app) else {
                    continue;
                };
                // 主体矩形（画布本地逻辑 px）：收起/展开同一套规则，外面穿透。
                let body = current_body_rect();
                // 收起态右键菜单打开期间一律不穿透（等价于 cfg.passthrough 的
                // 跳过分支）：菜单浮在胶囊热区之外，照常判定会把它切成穿透、
                // 菜单点不动；这时 want 恒为 false，窗口保持可交互。菜单一关，
                // 前端报 false，下一拍就回到原来的矩形判定。
                let menu_open = CONTEXT_MENU_OPEN.load(Ordering::Relaxed);
                // 拖拽期间保持可交互：拖拽由 Rust 轮询光标推进，但松手和点击
                // 语义还留在窗口上，中间切成穿透会丢掉 pointerup。
                let outside = !dragging
                    && !mouse_down
                    && should_mouse_through(now_rect, scale, Some(body), cursor, menu_open);

                // 光标移进胶囊：立刻恢复可交互（宁可慢一拍穿透，也不能让用户
                // 点胶囊时把点击漏给下面的应用）。移出去要连续两拍才真正穿透，
                // 快速掠过留白时不至于一闪一闪。
                outside_ticks = if outside {
                    outside_ticks.saturating_add(1)
                } else {
                    0
                };
                let want = outside && outside_ticks >= 2;
                match passthrough::set_automatic_mouse_through(h, want) {
                    Ok(true) if debug => eprintln!("[zbar-island] mouse-through {want}"),
                    Err(error) if debug => eprintln!("[zbar-island] mouse-through: {error}"),
                    _ => {}
                }
            }
        })
        .ok();
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandDragBegin {
    pub active: bool,
}

/// 拖拽/吸附帧数据（前端按此画液桥、拉丝、断桥、残滴）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandDragStep {
    pub active: bool,
    pub dx: f64,
    pub dy: f64,
    pub started: bool,
    pub edge: String,
    pub gap: f64,
    pub edge_line: f64,
    pub edge_lines: EdgeLines,
    pub origin: CanvasOrigin,
    pub monitor: String,
    pub vx: f64,
    pub vy: f64,
    pub body_rect: BodyRect,
    pub squash: f64,
    pub settled: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct EdgeLines {
    top: f64,
    bottom: f64,
    left: f64,
    right: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CanvasOrigin {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandDragEnd {
    pub edge: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub body_rect: BodyRect,
}

impl MotionWindow {
    fn origin(&self) -> (f64, f64) {
        (
            self.position.x as f64 / self.scale,
            self.position.y as f64 / self.scale,
        )
    }

    fn step(&self, area: &MonitorArea, dock_edge: &str, edge: &str) -> IslandDragStep {
        let area_l = dock_area_logical(area, dock_edge);
        let origin = self.origin();
        let line = |edge| edge_line_canvas(edge, area_l, origin);
        IslandDragStep {
            active: false,
            dx: 0.0,
            dy: 0.0,
            started: false,
            edge: edge.to_owned(),
            gap: gap_to_edge(edge, self.local, area_l, origin),
            edge_line: line(edge),
            edge_lines: EdgeLines {
                top: line("top"),
                bottom: line("bottom"),
                left: line("left"),
                right: line("right"),
            },
            origin: CanvasOrigin {
                x: origin.0,
                y: origin.1,
            },
            monitor: format!(
                "{}@{}:{},{}",
                area.name.as_deref().unwrap_or(""),
                area.scale,
                area.x,
                area.y,
            ),
            vx: 0.0,
            vy: 0.0,
            body_rect: BodyRect::from_tuple(self.local),
            squash: 0.0,
            settled: true,
        }
    }

    fn place(
        &mut self,
        win: &WebviewWindow,
        origin: (f64, f64),
        scale: f64,
        body: (f64, f64, f64, f64),
    ) -> Result<(f64, f64), String> {
        let position = PhysicalPosition::new(
            (origin.0 * scale).round() as i32,
            (origin.1 * scale).round() as i32,
        );
        if self.scale != scale {
            apply_window_frame_scaled(
                win,
                origin.0,
                origin.1,
                ISLAND_WINDOW_W,
                ISLAND_WINDOW_H,
                scale,
            )?;
        } else if position != self.position {
            win.set_position(position)
                .map_err(|e| format!("移动灵动岛失败: {e}"))?;
        }
        let delta = (
            (position.x as f64 - self.position.x as f64) / scale,
            (position.y as f64 - self.position.y as f64) / scale,
        );
        self.position = position;
        self.scale = scale;
        let origin = self.origin();
        self.local = (body.0 - origin.0, body.1 - origin.1, body.2, body.3);
        Ok(delta)
    }
}

fn drag_idle(app: &AppHandle, win: &WebviewWindow) -> Result<IslandDragStep, String> {
    let cfg = load_config();
    let area = monitor_area_for(app, cfg.monitor.as_deref());
    let (width, height) = current_body_size();
    let window = MotionWindow {
        position: win
            .outer_position()
            .map_err(|e| format!("读取灵动岛位置失败: {e}"))?,
        scale: win.scale_factor().unwrap_or(area.scale).max(0.1),
        local: body_local_rect(&cfg.edge, width, height),
    };
    Ok(window.step(&area, &cfg.edge, &cfg.edge))
}

fn drag_threshold_crossed(delta: (f64, f64), navigation: bool) -> bool {
    if navigation {
        // Empty navigation uses a smaller Manhattan threshold.
        delta.0.abs() + delta.1.abs() > 4.0
    } else {
        // Qt's default startDragDistance is ten logical pixels.
        delta.0.hypot(delta.1) > 10.0
    }
}

fn motion_dt(now: Instant, last_tick: Instant) -> f64 {
    (now - last_tick)
        .as_secs_f64()
        .clamp(1.0 / 240.0, 1.0 / 30.0)
}

fn is_horizontal(edge: &str) -> bool {
    matches!(edge, "top" | "bottom")
}

fn centered_rect(cx: f64, cy: f64, width: f64, height: f64) -> (f64, f64, f64, f64) {
    (cx - width / 2.0, cy - height / 2.0, width, height)
}

/// The canvas follows the body's center, with overscan for fillets and drops at the edge.
/// The screen-space body keeps moving independently as the canvas reaches its bounds.
fn moving_canvas_origin(
    center: (f64, f64),
    (x, y, width, height): (f64, f64, f64, f64),
) -> (f64, f64) {
    let place = |center, start, extent, canvas| {
        let min: f64 = start - BODY_INSET;
        let max: f64 = start + extent + BODY_INSET - canvas;
        if min > max {
            start + (extent - canvas) / 2.0
        } else {
            let centered: f64 = center - canvas / 2.0;
            centered.clamp(min, max)
        }
    };
    (
        place(center.0, x, width, ISLAND_WINDOW_W),
        place(center.1, y, height, ISLAND_WINDOW_H),
    )
}

#[tauri::command]
pub async fn island_drag_begin(
    app: AppHandle,
    sizes: IslandDragSizes,
    navigation: Option<bool>,
    expanded: Option<bool>,
    pointer: Option<CanvasOrigin>,
) -> Result<IslandDragBegin, String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        let cfg = load_config();
        if cfg.passthrough || motion_in_progress() || !left_button_down() {
            return Ok(IslandDragBegin { active: false });
        }
        let win = ensure_window(&app_main)?;
        let (cx, cy) = cursor_physical(&app_main).ok_or_else(|| "无法读取光标位置".to_string())?;
        let cursor = (cx as f64, cy as f64);
        let area = monitor_area_at(&app_main, cursor);
        let scale = win.scale_factor().unwrap_or(area.scale).max(0.1);
        let position = win
            .outer_position()
            .map_err(|e| format!("读取灵动岛位置失败: {e}"))?;
        let press = pointer
            .filter(|p| {
                (0.0..=ISLAND_WINDOW_W).contains(&p.x) && (0.0..=ISLAND_WINDOW_H).contains(&p.y)
            })
            .map(|p| {
                (
                    position.x as f64 + p.x * scale,
                    position.y as f64 + p.y * scale,
                )
            })
            .unwrap_or(cursor);
        let (width, height) = current_body_size();
        let local = body_local_rect(&cfg.edge, width, height);
        let (local_x, local_y) = rect_center(local);
        let center = (
            position.x as f64 + local_x * scale,
            position.y as f64 + local_y * scale,
        );
        let id = MOTION_ID.fetch_add(1, Ordering::Relaxed) + 1;
        let session = DragSession {
            id,
            edge: cfg.edge,
            press,
            grab: (0.0, 0.0),
            expanded: expanded.unwrap_or_else(|| LAST_EXPANDED.load(Ordering::Relaxed)),
            navigation: navigation.unwrap_or(false),
            started: false,
            released: false,
            x: MotionSpring::new(center.0, 900.0, 0.78),
            y: MotionSpring::new(center.1, 900.0, 0.78),
            width: MotionSpring::new(width, 300.0, 0.55),
            height: MotionSpring::new(height, 300.0, 0.55),
            sizes,
            window: MotionWindow {
                position,
                scale,
                local,
            },
            last_tick: Instant::now(),
        };
        *drag_slot().lock().map_err(|_| "无法开始灵动岛拖拽")? = Some(session);
        Ok(IslandDragBegin { active: true })
    })
    .await
}

#[tauri::command]
pub async fn island_drag_step(app: AppHandle) -> Result<IslandDragStep, String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        let win = ensure_window(&app_main)?;
        if let Some(snap) = snap_slot().lock().ok().and_then(|slot| slot.clone()) {
            return advance_snap(&app_main, &win, snap);
        }
        let Some(mut drag) = drag_slot().lock().ok().and_then(|slot| slot.clone()) else {
            return drag_idle(&app_main, &win);
        };
        if !left_button_down() || drag.released {
            DRAG_RELEASED.store(true, Ordering::Relaxed);
            if !drag.started {
                *drag_slot().lock().map_err(|_| "无法结束灵动岛按压")? = None;
                return drag_idle(&app_main, &win);
            }
            let area = monitor_area_at(&app_main, (drag.x.value, drag.y.value));
            let (edge, _) = nearest_edge(
                dock_area_logical(&area, &drag.edge),
                drag.window.origin(),
                drag.window.local,
            );
            let mut frame = drag.window.step(&area, &drag.edge, edge);
            frame.started = true;
            frame.settled = false;
            frame.vx = drag.x.velocity / drag.window.scale;
            frame.vy = drag.y.velocity / drag.window.scale;
            drag.released = true;
            *drag_slot().lock().map_err(|_| "无法结束灵动岛拖拽")? = Some(drag);
            return Ok(frame);
        }
        let (cx, cy) = cursor_physical(&app_main).ok_or_else(|| "无法读取光标位置".to_string())?;
        let cursor = (cx as f64, cy as f64);
        let press_scale = drag.window.scale;
        let now = Instant::now();
        let dt = motion_dt(now, drag.last_tick);
        drag.last_tick = now;
        if !drag.started {
            let delta = (
                (cursor.0 - drag.press.0) / press_scale,
                (cursor.1 - drag.press.1) / press_scale,
            );
            if !drag_threshold_crossed(delta, drag.navigation) {
                let area = monitor_area_at(&app_main, (drag.x.value, drag.y.value));
                let mut frame = drag.window.step(&area, &drag.edge, &drag.edge);
                frame.active = true;
                frame.settled = false;
                return Ok(frame);
            }
            drag.started = true;
            if drag.expanded {
                // Expanded content moves out immediately and shrinks around the pointer.
                drag.x.value = cursor.0;
                drag.y.value = cursor.1;
                drag.grab = (0.0, 0.0);
                LAST_EXPANDED.store(false, Ordering::Relaxed);
            } else {
                let grab = (
                    (drag.x.value - cursor.0) / press_scale,
                    (drag.y.value - cursor.1) / press_scale,
                );
                if grab.0.abs() <= drag.width.value / 2.0 && grab.1.abs() <= drag.height.value / 2.0
                {
                    drag.grab = grab;
                }
            }
            (drag.width.target, drag.height.target) = drag.sizes.for_edge(&drag.edge);
            spawn_motion_watchdog(app_main.clone(), drag.id);
        }
        let pointer_area = monitor_area_at(&app_main, cursor);
        drag.x.target = cursor.0 + drag.grab.0 * pointer_area.scale;
        drag.y.target = cursor.1 + drag.grab.1 * pointer_area.scale;
        drag.x.step(dt);
        drag.y.step(dt);
        drag.width.step(dt);
        drag.height.step(dt);
        let area = monitor_area_at(&app_main, (drag.x.value, drag.y.value));
        let center = (drag.x.value / area.scale, drag.y.value / area.scale);
        let area_l = dock_area_logical(&area, &drag.edge);
        let body = centered_rect(center.0, center.1, drag.width.value, drag.height.value);
        let new_origin = moving_canvas_origin(center, area_l);
        let delta = drag.window.place(&win, new_origin, area.scale, body)?;
        let (edge, _) = nearest_edge(area_l, drag.window.origin(), drag.window.local);
        let mut frame = drag.window.step(&area, &drag.edge, edge);
        frame.active = true;
        frame.started = true;
        frame.settled = false;
        (frame.dx, frame.dy) = delta;
        frame.vx = drag.x.velocity / area.scale;
        frame.vy = drag.y.velocity / area.scale;
        *drag_slot().lock().map_err(|_| "无法更新灵动岛拖拽")? = Some(drag);
        Ok(frame)
    })
    .await
}

fn make_snap(app: &AppHandle, drag: &DragSession) -> SnapSession {
    let area = monitor_area_at(app, (drag.x.value, drag.y.value));
    let center = (drag.x.value / area.scale, drag.y.value / area.scale);
    let (width, height) = (drag.width.target, drag.height.target);
    let body = centered_rect(center.0, center.1, width, height);
    let (edge, _) = nearest_snap_edge(dock_area_logical(&area, &drag.edge), body);
    let area_l = dock_area_logical(&area, edge);
    let gap = gap_to_edge(edge, body, area_l, (0.0, 0.0));
    let horizontal = is_horizontal(edge);
    let (along0, v_along, area_start, area_len, gap_velocity) = match edge {
        "top" => (
            center.0,
            drag.x.velocity,
            area_l.0,
            area_l.2,
            drag.y.velocity,
        ),
        "bottom" => (
            center.0,
            drag.x.velocity,
            area_l.0,
            area_l.2,
            -drag.y.velocity,
        ),
        "left" => (
            center.1,
            drag.y.velocity,
            area_l.1,
            area_l.3,
            drag.x.velocity,
        ),
        _ => (
            center.1,
            drag.y.velocity,
            area_l.1,
            area_l.3,
            -drag.x.velocity,
        ),
    };
    let pos_frac = ((along0 - area_start) / area_len.max(1.0)).clamp(0.0, 1.0);
    // AgentHUD reserves the expanded panel's footprint when anchored, so opening
    // at the ends of an edge does not move or clip the native window.
    let target_win = anchored_window_pos(edge, pos_frac, area_l);
    let mut along = MotionSpring::new(along0, 240.0, 0.74);
    along.velocity = v_along / area.scale;
    along.target = if horizontal {
        target_win.0 + ISLAND_WINDOW_W / 2.0
    } else {
        target_win.1 + ISLAND_WINDOW_H / 2.0
    };
    let mut gap = MotionSpring::new(gap.max(-4.0), 300.0, 0.38);
    gap.velocity = gap_velocity / area.scale;
    gap.target = 0.0;
    let (target_w, target_h) = drag.sizes.for_edge(edge);
    let mut width = drag.width;
    let mut height = drag.height;
    if (width.target - target_w).abs() > 0.5 {
        let velocity = width.velocity;
        width = MotionSpring::new(width.value, 460.0, 0.64);
        width.velocity = velocity;
    }
    if (height.target - target_h).abs() > 0.5 {
        let velocity = height.velocity;
        height = MotionSpring::new(height.value, 460.0, 0.64);
        height.velocity = velocity;
    }
    (width.target, height.target) = (target_w, target_h);
    SnapSession {
        id: drag.id,
        edge: edge.to_owned(),
        area,
        along,
        gap,
        width,
        height,
        pos_frac: ((along.target - area_start) / area_len.max(1.0)).clamp(0.0, 1.0),
        target_win,
        window: drag.window.clone(),
        last_tick: Instant::now(),
    }
}

impl SnapSession {
    fn target(&self) -> IslandDragEnd {
        let local = body_local_rect(&self.edge, self.width.target, self.height.target);
        IslandDragEnd {
            edge: self.edge.clone(),
            x: self.target_win.0,
            y: self.target_win.1,
            width: ISLAND_WINDOW_W,
            height: ISLAND_WINDOW_H,
            body_rect: BodyRect::from_tuple(local),
        }
    }

    fn place(&mut self, win: &WebviewWindow, settled: bool) -> Result<IslandDragStep, String> {
        let (width, height, along, gap) = if settled {
            (
                self.width.target,
                self.height.target,
                self.along.target,
                0.0,
            )
        } else {
            (
                self.width.value,
                self.height.value,
                self.along.value,
                self.gap.value.max(0.0),
            )
        };
        let area = dock_area_logical(&self.area, &self.edge);
        let line = edge_line_canvas(&self.edge, area, (0.0, 0.0));
        let center = match self.edge.as_str() {
            "top" => (along, line + gap + height / 2.0),
            "bottom" => (along, line - gap - height / 2.0),
            "left" => (line + gap + width / 2.0, along),
            _ => (line - gap - width / 2.0, along),
        };
        let body = centered_rect(center.0, center.1, width, height);
        let origin = if settled {
            self.target_win
        } else {
            moving_canvas_origin(center, area)
        };
        let delta = self.window.place(win, origin, self.area.scale, body)?;
        let mut frame = self.window.step(&self.area, &self.edge, &self.edge);
        let va = self.along.velocity;
        let vg = self.gap.velocity;
        (frame.vx, frame.vy) = match self.edge.as_str() {
            "top" => (va, vg),
            "bottom" => (va, -vg),
            "left" => (vg, va),
            _ => (-vg, va),
        };
        frame.active = !settled;
        frame.started = true;
        frame.settled = settled;
        (frame.dx, frame.dy) = delta;
        frame.squash = if settled {
            0.0
        } else {
            (-self.gap.value).max(0.0)
        };
        Ok(frame)
    }
}

fn finish_snap(
    app: &AppHandle,
    win: &WebviewWindow,
    mut snap: SnapSession,
) -> Result<IslandDragStep, String> {
    let frame = snap.place(win, true)?;
    let mut cfg = load_config();
    cfg.edge = snap.edge;
    cfg.pos_frac = snap.pos_frac;
    cfg.monitor = snap.area.name;
    *body_size_slot().lock().map_err(|_| "无法记录灵动岛尺寸")? =
        (snap.width.target, snap.height.target);
    *snap_slot().lock().map_err(|_| "无法结束灵动岛吸附")? = None;
    if let Err(error) = save_config(&cfg) {
        let previous = load_config();
        let (x, y, w, h) = island_geometry(app, &previous.edge, false, previous.pos_frac);
        let _ = apply_window_frame_scaled(win, x, y, w, h, island_scale(app));
        emit_config(app, &previous);
        return Err(error);
    }
    emit_config(app, &cfg);
    Ok(frame)
}

fn advance_snap(
    app: &AppHandle,
    win: &WebviewWindow,
    mut snap: SnapSession,
) -> Result<IslandDragStep, String> {
    let now = Instant::now();
    let dt = motion_dt(now, snap.last_tick);
    snap.last_tick = now;
    snap.along.step(dt);
    snap.gap.step(dt);
    snap.width.step(dt);
    snap.height.step(dt);
    if snap.along.at_rest() && snap.gap.at_rest() && snap.width.at_rest() && snap.height.at_rest() {
        return finish_snap(app, win, snap);
    }
    let frame = snap.place(win, false)?;
    *snap_slot().lock().map_err(|_| "无法更新灵动岛吸附")? = Some(snap);
    Ok(frame)
}

#[tauri::command]
pub async fn island_drag_end(
    app: AppHandle,
    started: Option<bool>,
) -> Result<IslandDragEnd, String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        if let Some(snap) = snap_slot().lock().ok().and_then(|slot| slot.clone()) {
            return Ok(snap.target());
        }
        let win = ensure_window(&app_main)?;
        let mut slot = drag_slot().lock().map_err(|_| "无法结束灵动岛拖拽")?;
        if slot
            .as_ref()
            .is_some_and(|drag| drag.started && (drag.released || !left_button_down()))
        {
            let drag = slot.as_ref().ok_or("灵动岛拖拽已结束")?;
            let snap = make_snap(&app_main, drag);
            let target = snap.target();
            *snap_slot().lock().map_err(|_| "无法开始灵动岛吸附")? = Some(snap);
            *slot = None;
            DRAG_RELEASED.store(true, Ordering::Relaxed);
            return Ok(target);
        }
        if started == Some(false) && !left_button_down() {
            *slot = None;
        }
        drop(slot);
        let current = drag_idle(&app_main, &win)?;
        Ok(IslandDragEnd {
            edge: current.edge,
            x: current.origin.x,
            y: current.origin.y,
            width: ISLAND_WINDOW_W,
            height: ISLAND_WINDOW_H,
            body_rect: current.body_rect,
        })
    })
    .await
}

/// A stalled or reloaded webview must not leave the native window mid-drag.
/// The watchdog checks the same generation again on the main thread before committing.
fn spawn_motion_watchdog(app: AppHandle, id: u64) {
    let _ = thread::Builder::new()
        .name("zbar-island-motion-watchdog".into())
        .spawn(move || loop {
            thread::sleep(MOTION_WATCHDOG_INTERVAL);
            let drag = drag_slot().lock().ok().and_then(|slot| slot.clone());
            let snap = snap_slot().lock().ok().and_then(|slot| slot.clone());
            let stale_drag = drag.as_ref().is_some_and(|drag| {
                drag.id == id
                    && drag.started
                    && drag.last_tick.elapsed() > MOTION_WATCHDOG_TIMEOUT
                    && !left_button_down()
            });
            let stale_snap = snap.as_ref().is_some_and(|snap| {
                snap.id == id && snap.last_tick.elapsed() > MOTION_WATCHDOG_TIMEOUT
            });
            if stale_drag || stale_snap {
                let app_main = app.clone();
                let _ = app.run_on_main_thread(move || {
                    let Some(win) = app_main.get_webview_window(ISLAND_WINDOW_LABEL) else {
                        return;
                    };
                    let snap = snap_slot().lock().ok().and_then(|slot| slot.clone());
                    let snap = snap
                        .filter(|snap| {
                            snap.id == id && snap.last_tick.elapsed() > MOTION_WATCHDOG_TIMEOUT
                        })
                        .or_else(|| {
                            let mut slot = drag_slot().lock().ok()?;
                            let same_drag = slot.as_ref().is_some_and(|drag| {
                                drag.id == id
                                    && drag.started
                                    && drag.last_tick.elapsed() > MOTION_WATCHDOG_TIMEOUT
                                    && !left_button_down()
                            });
                            if !same_drag {
                                return None;
                            }
                            let drag = slot.take()?;
                            drop(slot);
                            DRAG_RELEASED.store(true, Ordering::Relaxed);
                            Some(make_snap(&app_main, &drag))
                        });
                    if let Some(snap) = snap {
                        if let Err(error) = finish_snap(&app_main, &win, snap) {
                            eprintln!("[zbar-island] 恢复灵动岛吸附失败: {error}");
                        }
                    }
                });
            } else if !drag.is_some_and(|drag| drag.id == id)
                && !snap.is_some_and(|snap| snap.id == id)
            {
                break;
            }
        });
}

// ============================================================
// 配置
// ============================================================

/// 启动器条目（主页底部快捷入口；默认给 Windows 常用位置）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LauncherItem {
    pub name: String,
    pub path: String,
    /// 前端图标键：folder / browser / settings / task / note / terminal
    #[serde(default)]
    pub icon: String,
}

impl Default for LauncherItem {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: String::new(),
            icon: String::new(),
        }
    }
}

fn default_transparency() -> u8 {
    0
}
fn default_font_size() -> f64 {
    13.0
}
fn default_expand_mode() -> String {
    "hover".into()
}
fn default_theme() -> String {
    "dark".into()
}
fn default_hover_expand_ms() -> u64 {
    120
}
fn default_collapse_delay_ms() -> u64 {
    320
}
fn default_compact_view() -> String {
    "rings".into()
}
fn default_quota_display() -> String {
    "percent".into()
}
fn normalize_quota_display(value: &str) -> String {
    if value.trim().eq_ignore_ascii_case("rings") {
        "rings".into()
    } else {
        default_quota_display()
    }
}
fn default_auto_cycle_sec() -> u64 {
    30
}
fn default_blur_fps() -> u64 {
    8
}
fn default_tab_key() -> String {
    "home".into()
}

/// 消息模块 kind 全集（顺序 = 前端消息页磁贴顺序）。
pub const NOTIFY_KINDS: [&str; 6] = ["qq", "wechat", "mail", "feishu", "dingtalk", "dsh"];

/// 归一化消息模块列表：trim + 小写、白名单过滤、去重，按 `NOTIFY_KINDS`
/// 固定顺序输出。允许返回空数组（= 该功能整体关闭）。
pub fn normalize_notify_kinds(list: &[String]) -> Vec<String> {
    let mut wanted = std::collections::HashSet::new();
    for kind in list {
        let kind = kind.trim().to_ascii_lowercase();
        if NOTIFY_KINDS.contains(&kind.as_str()) {
            wanted.insert(kind);
        }
    }
    NOTIFY_KINDS
        .iter()
        .filter(|kind| wanted.contains(**kind))
        .map(|kind| (*kind).to_string())
        .collect()
}

fn normalize_notify_body_preview(list: &[String]) -> Vec<String> {
    let mut kinds = normalize_notify_kinds(list);
    kinds.retain(|kind| kind != "dsh");
    kinds
}

/// 歌词源白名单。与消息模块不同，**用户配置的顺序就是尝试顺序**，归一化时
/// 不去重排序，只做 trim / 小写 / 白名单 / 去重。
pub const LYRICS_SOURCES: [&str; 3] = ["netease", "lrclib", "custom"];

fn default_media_lyrics_sources() -> String {
    "netease,lrclib,custom".into()
}

/// 归一化歌词源串：逗号拆分 → trim / 小写 → 白名单 → 去重（保持用户顺序）；
/// 一个都不剩（空串 / 全非法）时回退默认 `netease,lrclib,custom`。
pub fn normalize_lyrics_sources(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split(',') {
        let key = part.trim().to_ascii_lowercase();
        if LYRICS_SOURCES.contains(&key.as_str()) && !out.iter().any(|item| *item == key) {
            out.push(key);
        }
    }
    if out.is_empty() {
        return default_media_lyrics_sources();
    }
    out.join(",")
}

/// SMTC 播放器优先级默认值（子串匹配 appId / 友好名）。
fn default_media_priority() -> Vec<String> {
    ["cloudmusic", "bilibili", "chrome", "zen", "msedge", "firefox"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 播放器优先级：trim、去空、大小写不敏感去重（保留首次出现的写法），最多
/// 16 项。允许为空（= 用系统当前会话的原逻辑）。
pub(crate) fn normalize_media_priority(items: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty() && seen.insert(item.to_ascii_lowercase()))
        .take(16)
        .map(|item| item.to_string())
        .collect()
}

pub(crate) fn normalize_media_playback_logs(items: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .iter()
        .map(|item| item.trim())
        .filter(|item| {
            !item.is_empty()
                && seen.insert(if cfg!(windows) {
                    item.to_lowercase()
                } else {
                    (*item).to_owned()
                })
        })
        .take(16)
        .map(str::to_owned)
        .collect()
}

/// 归一化「打开应用」路径表：只保留白名单 key（小写）、值 trim、空值删除。
pub fn normalize_notify_open(
    map: std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    map.into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            (NOTIFY_KINDS.contains(&key.as_str()) && !value.is_empty()).then_some((key, value))
        })
        .collect()
}

fn default_notify_modules() -> Vec<String> {
    vec!["qq".into(), "wechat".into(), "mail".into()]
}

fn default_notify_body_preview() -> Vec<String> {
    vec!["feishu".into(), "dingtalk".into()]
}

/// 灵动岛标签页 key 全集（与前端 `TabKey` 保持一致；`settings` 已迁到
/// 独立设置窗口，不再占用岛内标签位）。
pub const ISLAND_TAB_KEYS: [&str; 9] = [
    "home", "msg", "stock", "weather", "ctl", "clip", "shot", "translate", "tool",
];

/// 合法标签页 key（lastTab / defaultTab 共用）。
fn valid_tab_key(key: &str) -> bool {
    ISLAND_TAB_KEYS.contains(&key)
}

/// 主页固定显示（岛的根页面），其余板块都可在岛内菜单和设置窗口里切换；
/// 工具板块没有常驻后台线程，隐藏后前端不再挂载页面即可；正在进行的会议录音
/// 由 `sync_tab_services` 兜底结束（正常保存，见 `meeting::stop_on_panel_close`）。
fn can_hide_tab(key: &str) -> bool {
    valid_tab_key(key) && key != "home"
}

/// 坐标对归一化：任一缺失 / 非有限 / 越界都当成「没有坐标」。
pub(crate) fn normalize_weather_coord_pair(
    lat: Option<f64>,
    lon: Option<f64>,
) -> (Option<f64>, Option<f64>) {
    match (lat, lon) {
        (Some(lat), Some(lon))
            if lat.is_finite()
                && lon.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lon) =>
        {
            (Some(lat), Some(lon))
        }
        _ => (None, None),
    }
}

/// 自定义城市归一化：trim 名字、跳过空名、坐标非法降级为 `None`、
/// 按 name + 坐标去重、最多 20 条（保持用户添加顺序）。
fn normalize_weather_cities(items: Vec<WeatherCityCfg>) -> Vec<WeatherCityCfg> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let name = item.name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let (lat, lon) = normalize_weather_coord_pair(item.lat, item.lon);
        let key = (
            name.to_lowercase(),
            lat.map(f64::to_bits),
            lon.map(f64::to_bits),
        );
        if !seen.insert(key) {
            continue;
        }
        out.push(WeatherCityCfg { name, lat, lon });
        if out.len() >= 20 {
            break;
        }
    }
    out
}

fn default_edge() -> String {
    "top".into()
}
fn default_pos_frac() -> f64 {
    0.5
}
fn default_stocks() -> Vec<String> {
    [
        "sh000001", "sz399001", "sz399006", "hk00700", "usAAPL", "usNVDA", "usQQQ",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
fn default_stock_interval() -> u64 {
    5
}
fn default_stock_alert() -> f64 {
    3.0
}
fn default_hotkey() -> String {
    // Ctrl+Alt+I 在不少输入法/截图工具里已被占用；用带 Shift 的组合降低冲突
    "Ctrl+Alt+Shift+I".into()
}
/// 翻译剪贴板热键（注册后读剪贴板并翻译写回）
fn default_hotkey_translate() -> String {
    "Ctrl+Alt+J".into()
}
/// 打开剪贴板热键
fn default_hotkey_clipboard() -> String {
    "Ctrl+Alt+V".into()
}
fn default_launcher() -> Vec<LauncherItem> {
    vec![
        LauncherItem {
            name: "资源管理器".into(),
            path: "explorer.exe".into(),
            icon: "folder".into(),
        },
        LauncherItem {
            name: "浏览器".into(),
            path: "https://www.bing.com".into(),
            icon: "browser".into(),
        },
        LauncherItem {
            name: "设置".into(),
            path: "ms-settings:".into(),
            icon: "settings".into(),
        },
        LauncherItem {
            name: "任务管理器".into(),
            path: "taskmgr.exe".into(),
            icon: "task".into(),
        },
        LauncherItem {
            name: "记事本".into(),
            path: "notepad.exe".into(),
            icon: "note".into(),
        },
    ]
}

/// 天气自定义城市（运行态与 `settings.json` 的 `weather.cities` 双向同步）。
///
/// 只保留 geocode / Open-Meteo 需要的字段；`settings.json` 里的 `country`
/// 等额外字段由 `island::settings` 的 `WeatherCity::extra` 保留，不进运行态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WeatherCityCfg {
    pub name: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

/// 灵动岛配置（内部 SQLite 状态库，不再暴露 JSON 文件）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IslandConfig {
    pub enabled: bool,
    /// 毛玻璃的通透程度：0 = 几乎实心，90 = 尽量透出桌面。
    /// 只影响玻璃色层与模糊背景的合成，不改变文字和图标的不透明度。
    pub transparency: u8,
    /// 界面基准字号（逻辑 px，13 = 原始设计）：只缩放文字，不缩放图标和窗口几何。
    pub font_size: f64,
    /// hover / always / never（hover = 悬停展开、离开收起；always = 常驻展开；
    /// never = 保持收起）。只有 hover 由后端悬停守护自动收起。
    pub expand_mode: String,
    /// 主题：auto / light / dark / rouge / sky（非法回退 dark）
    pub theme: String,
    /// 是否让悬停展开。关闭后仍可单击展开，并在移出时收起。
    pub hover_expand: bool,
    /// 悬停展开延迟（毫秒，clamp 0..=2000）
    pub hover_expand_ms: u64,
    /// 鼠标离开后的收起延迟（毫秒，clamp 0..=4000）
    pub collapse_delay_ms: u64,
    /// 收起态视图：rings / clock / pet / music（非法回退 rings）
    pub compact_view: String,
    /// 收起态额度显示：percent / rings。
    pub quota_display: String,
    /// 收起态视图自动轮换间隔（秒，clamp 0..=600，0 = 关闭）
    pub auto_cycle_sec: u64,
    /// 毛玻璃背景采样帧率（clamp 1..=60）
    pub blur_fps: u64,
    /// 可穿透：窗口对鼠标透明（Windows 上真正加 WS_EX_TRANSPARENT，
    /// 右键菜单/滚轮由 island::passthrough 的 WH_MOUSE_LL 钩子处理）。
    pub passthrough: bool,
    /// 上次停留的标签页；值必须属于 ISLAND_TAB_KEYS，非法时置 None。
    pub last_tab: Option<String>,
    /// 默认标签页（同 last_tab 值域，非法回退 home）
    pub default_tab: String,
    /// 首次启用时自动关掉旧的贴边 HUD 停靠条，避免两套 HUD 叠在一起
    pub replace_legacy_bar: bool,
    /// 绑定的主城市；空串 = 跟随公网 IP 定位城市。
    pub weather_city: String,
    /// 旧版三级联动选中的展示名（如“杭州 · 西湖区”）；空串时显示 weather_city。
    /// 新逻辑里展示名改由天气服务按城市计算；设置页 / 绑定命令保存城市时会
    /// 回填为城市名，用来区分「显式绑定」与旧版本默认值（见 clamped 的迁移）。
    pub weather_label: String,
    pub weather_lat: Option<f64>,
    pub weather_lon: Option<f64>,
    /// 天气页自定义城市列表（按用户添加顺序；主城市不在这里，见 weather_city）。
    pub weather_cities: Vec<WeatherCityCfg>,
    /// 吸附边：top / bottom / left / right
    pub edge: String,
    /// 吸附边上的位置比例 0..1（top/bottom 按横坐标，左右按纵坐标）
    pub pos_frac: f64,
    /// 上次所在显示器（Tauri Monitor::name，如 `\\.\DISPLAY2`）。None/找不到
    /// 时回退主屏；多屏拖拽后展开、重启都靠它留在同一块屏。
    pub monitor: Option<String>,
    pub stocks: Vec<String>,
    pub stock_interval_sec: u64,
    pub stock_alert_pct: f64,
    pub hotkey_toggle: String,
    /// 翻译剪贴板热键：读剪贴板文本 → DeepSeek 兼容接口 → 译文写回剪贴板
    pub hotkey_translate: String,
    /// 打开剪贴板热键：展开灵动岛并记住剪贴板页
    pub hotkey_clipboard: String,
    /// 字体 family（空 = 内置字体栈）；只存盘 + 随配置广播
    pub font_family: String,
    /// 始终置顶（false = 允许被其他窗口盖住）
    pub keep_on_top: bool,
    /// 全屏时贴到物理屏幕边缘（本期只存盘）
    pub hug_fullscreen: bool,
    /// 启用时通过 WDA_EXCLUDEFROMCAPTURE 将灵动岛排除出截图和录屏。
    pub exclude_from_capture: bool,
    pub hide_fullscreen: bool,
    pub hidden_tabs: Vec<String>,
    /// 媒体开始播放时自动把收起态切到 music 视图（false = 只保留手动滚轮切换）
    pub media_auto_show: bool,
    /// 是否拉取并显示歌词（false = 前端不请求 `island_lyrics`）
    pub media_lyrics: bool,
    /// Windows 网易云本地播放日志
    pub media_netease_log: bool,
    /// Windows 播放状态 JSON / JSONL 文件路径
    pub media_playback_logs: Vec<String>,
    /// 歌词源顺序（逗号分隔，白名单 `netease` / `lrclib` / `custom`）
    pub media_lyrics_sources: String,
    /// 自定义歌词接口 URL 模板（支持 `{title}` `{artist}` `{album}` `{duration}`
    /// `{song_id}` 占位符，值会被 URL 编码）
    pub media_lyrics_custom_url: String,
    /// 播放器优先级：按顺序子串匹配 appId / 友好名，挑第一个正在播放的
    pub media_priority: Vec<String>,
    /// 消息页启用的模块（`qq/wechat/mail/feishu/dingtalk`）。空数组 = 一个都
    /// 不显示：对应 kind 的通知直接丢弃，不进列表、不计未读。
    pub notify_modules: Vec<String>,
    /// 额外读取正文预览的模块（同一白名单）。空数组 = 不读正文/sender 预览。
    pub notify_body_preview: Vec<String>,
    /// 「打开应用」自定义路径（kind → exe 路径，支持 `%VAR%` 展开）；空 = 走
    /// 内置「聚焦已运行窗口 → 常见安装路径」逻辑。
    pub notify_open: std::collections::BTreeMap<String, String>,
    /// 新消息桌面玻璃浮窗总开关（`notify.popup.enabled`）。浮窗是独立窗口
    /// label `notify-popup`，见子模块 `popup`。
    pub notify_popup_enabled: bool,
    /// 浮窗位置六选一（`notify.popup.position`，值域同 settings 的
    /// `POPUP_POSITIONS`）：center_bottom / center_top / bottom_left /
    /// bottom_right / top_left / top_right
    pub notify_popup_position: String,
    /// 浮窗尺寸百分比（`notify.popup.size`，70..=160，100 = 360×84 设计稿）
    pub notify_popup_size: i64,
    pub launcher: Vec<LauncherItem>,
    /// 收起胶囊的环 / 主页额度卡的展示顺序（agent id 列表）。空数组 =
    /// 按数据源默认顺序；不在这里的条目按原顺序排在后面。
    #[serde(default)]
    pub agent_order: Vec<String>,
}

impl Default for IslandConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            transparency: default_transparency(),
            font_size: default_font_size(),
            expand_mode: default_expand_mode(),
            theme: default_theme(),
            hover_expand: true,
            hover_expand_ms: default_hover_expand_ms(),
            collapse_delay_ms: default_collapse_delay_ms(),
            compact_view: default_compact_view(),
            quota_display: default_quota_display(),
            auto_cycle_sec: default_auto_cycle_sec(),
            blur_fps: default_blur_fps(),
            passthrough: false,
            last_tab: None,
            default_tab: default_tab_key(),
            replace_legacy_bar: true,
            weather_city: String::new(),
            weather_label: String::new(),
            weather_lat: None,
            weather_lon: None,
            weather_cities: Vec::new(),
            edge: default_edge(),
            pos_frac: default_pos_frac(),
            monitor: None,
            stocks: default_stocks(),
            stock_interval_sec: default_stock_interval(),
            stock_alert_pct: default_stock_alert(),
            hotkey_toggle: default_hotkey(),
            hotkey_translate: default_hotkey_translate(),
            hotkey_clipboard: default_hotkey_clipboard(),
            font_family: String::new(),
            keep_on_top: true,
            hug_fullscreen: true,
            exclude_from_capture: false,
            hide_fullscreen: false,
            hidden_tabs: Vec::new(),
            media_auto_show: true,
            media_lyrics: true,
            media_netease_log: true,
            media_playback_logs: Vec::new(),
            media_lyrics_sources: default_media_lyrics_sources(),
            media_lyrics_custom_url: String::new(),
            media_priority: default_media_priority(),
            notify_modules: default_notify_modules(),
            notify_body_preview: default_notify_body_preview(),
            notify_open: std::collections::BTreeMap::new(),
            notify_popup_enabled: true,
            notify_popup_position: "bottom_right".into(),
            notify_popup_size: 100,
            launcher: default_launcher(),
            agent_order: Vec::new(),
        }
    }
}

impl IslandConfig {
    fn clamped(self) -> Self {
        let mut c = self;
        c.transparency = c.transparency.min(90);
        c.font_size = if c.font_size.is_finite() {
            c.font_size.clamp(10.0, 18.0)
        } else {
            default_font_size()
        };
        c.expand_mode = c.expand_mode.trim().to_ascii_lowercase();
        if !matches!(c.expand_mode.as_str(), "hover" | "always" | "never") {
            c.expand_mode = default_expand_mode();
        }
        c.theme = c.theme.trim().to_ascii_lowercase();
        if !matches!(
            c.theme.as_str(),
            "auto" | "light" | "dark" | "rouge" | "sky"
        ) {
            c.theme = default_theme();
        }
        c.compact_view = c.compact_view.trim().to_ascii_lowercase();
        if !matches!(c.compact_view.as_str(), "rings" | "clock" | "pet" | "music") {
            c.compact_view = default_compact_view();
        }
        c.quota_display = normalize_quota_display(&c.quota_display);
        // 天气主城市：空 = 跟随公网 IP。旧版本默认写过「上海」，升级时
        // 若没有坐标也没有展示名，则按旧默认值清空，让主城市跟随 IP。
        c.weather_city = c.weather_city.trim().to_string();
        c.weather_label = c.weather_label.trim().to_string();
        if c.weather_city == "上海"
            && c.weather_lat.is_none()
            && c.weather_lon.is_none()
            && c.weather_label.is_empty()
        {
            c.weather_city.clear();
        }
        // 未绑定主城市时坐标/展示名一起清掉，避免旧坐标残留。
        if c.weather_city.is_empty() {
            c.weather_label.clear();
            c.weather_lat = None;
            c.weather_lon = None;
        }
        c.weather_cities = normalize_weather_cities(std::mem::take(&mut c.weather_cities));
        c.hover_expand_ms = c.hover_expand_ms.min(2000);
        c.collapse_delay_ms = c.collapse_delay_ms.min(4000);
        c.auto_cycle_sec = c.auto_cycle_sec.min(600);
        c.blur_fps = c.blur_fps.clamp(1, 60);
        c.last_tab = c
            .last_tab
            .map(|key| key.trim().to_ascii_lowercase())
            .filter(|key| valid_tab_key(key));
        let default_tab = c.default_tab.trim().to_ascii_lowercase();
        c.default_tab = if valid_tab_key(&default_tab) {
            default_tab
        } else {
            default_tab_key()
        };
        c.hotkey_toggle = c.hotkey_toggle.trim().to_string();
        c.hotkey_translate = c.hotkey_translate.trim().to_string();
        c.hotkey_clipboard = c.hotkey_clipboard.trim().to_string();
        c.font_family = c.font_family.trim().to_string();
        if c.edge != "top" && c.edge != "bottom" && c.edge != "left" && c.edge != "right" {
            c.edge = default_edge();
        }
        c.pos_frac = if c.pos_frac.is_finite() {
            c.pos_frac.clamp(0.0, 1.0)
        } else {
            default_pos_frac()
        };
        c.monitor = c
            .monitor
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty());
        c.stock_interval_sec = c.stock_interval_sec.clamp(3, 60);
        c.stock_alert_pct = if c.stock_alert_pct.is_finite() {
            c.stock_alert_pct.clamp(0.5, 20.0)
        } else {
            default_stock_alert()
        };
        let mut seen = std::collections::HashSet::new();
        c.stocks = c
            .stocks
            .iter()
            .map(|s| stocks::norm_code(s))
            .filter(|s| stocks::valid_code(s) && seen.insert(s.clone()))
            .collect();
        // 隐藏板块白名单：与前端 SECTION_TABS 对齐（只有 home 不参与隐藏）。
        // 空数组 = 全部显示；过滤后排序去重。
        c.hidden_tabs.retain(|key| can_hide_tab(key));
        c.hidden_tabs.sort();
        c.hidden_tabs.dedup();
        // 消息模块开关 / 正文预览开关：白名单 + 固定顺序 + 去重，允许为空
        c.notify_modules = normalize_notify_kinds(&c.notify_modules);
        c.notify_body_preview = normalize_notify_body_preview(&c.notify_body_preview);
        // 歌词源：逗号拆分 + 白名单 + 去重，保持用户顺序；空回默认顺序
        c.media_lyrics_sources = normalize_lyrics_sources(&c.media_lyrics_sources);
        // 自定义歌词接口：trim + 限长（URL 只会更长不会有意义，2048 足够）
        let custom_url: String = c
            .media_lyrics_custom_url
            .trim()
            .chars()
            .take(2048)
            .collect();
        c.media_lyrics_custom_url = custom_url;
        c.media_priority = normalize_media_priority(&c.media_priority);
        c.media_playback_logs = normalize_media_playback_logs(&c.media_playback_logs);
        // 「打开应用」路径表：白名单 key + trim + 空值删除
        c.notify_open = normalize_notify_open(std::mem::take(&mut c.notify_open));
        // 桌面浮窗：位置白名单（非法回退右下）、尺寸夹到 70..=160
        c.notify_popup_position = {
            let wanted = c.notify_popup_position.trim().to_ascii_lowercase();
            if settings::POPUP_POSITIONS.contains(&wanted.as_str()) {
                wanted
            } else {
                "bottom_right".to_string()
            }
        };
        c.notify_popup_size = c.notify_popup_size.clamp(70, 160);
        c.launcher
            .retain(|i| !i.name.trim().is_empty() && !i.path.trim().is_empty());
        // 排序表：去空、去重、限长（防止前端手滑塞进成千上万个 id）
        let mut seen_agents = std::collections::HashSet::new();
        c.agent_order = c
            .agent_order
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty() && seen_agents.insert(id.clone()))
            .take(64)
            .collect();
        c
    }
}

/// 读取配置：内部库不存在时回退默认值（并自动迁移旧版 JSON）。
pub fn load_config() -> IslandConfig {
    store::load()
}

/// 写配置：内部 SQLite 单行覆盖，读路径有进程内缓存。
pub fn save_config(config: &IslandConfig) -> Result<(), String> {
    store::save(config)
}

/// 启动前迁移：灵动岛默认接管 HUD，把旧的贴边停靠条关掉（只写配置，
/// 不建窗/不碰已有窗口；随后 lib.rs 的 hud_bar::start_if_enabled 会读到 false）。
pub fn prepare_legacy_bar() {
    let cfg = load_config();
    if !cfg.enabled || !cfg.replace_legacy_bar {
        return;
    }
    let mut bar = crate::hud_bar::load_hud_bar_config();
    if bar.enabled {
        bar.enabled = false;
        if let Err(e) = crate::hud_bar::save_hud_bar_config(&bar) {
            eprintln!("[agenthud-island] 关闭旧 HUD 停靠条失败: {e}");
        }
    }
}

// ============================================================
// 窗口
// ============================================================

/// 配置所指那块屏的缩放比（逻辑 px → 物理 px 用）。
///
/// 不能用 `win.scale_factor()`：多屏不同缩放时，配置指向副屏而窗口还挂在主屏
/// 上的那一次会算错；换算必须和 `island_geometry` 用同一块屏的比例。
fn island_scale(app: &AppHandle) -> f64 {
    let cfg = load_config();
    monitor_area_for(app, cfg.monitor.as_deref()).scale.max(0.5)
}

/// 一块显示器的物理工作区 + 缩放比 + 设备名（跨屏拖拽/吸附用）。
#[derive(Debug, Clone)]
struct MonitorArea {
    name: Option<String>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    scale: f64,
    bounds: ScreenBounds,
}

impl Default for MonitorArea {
    fn default() -> Self {
        Self {
            name: None,
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
            scale: 1.0,
            bounds: (0, 0, 1920, 1080),
        }
    }
}

fn area_of_monitor(mon: &tauri::Monitor) -> MonitorArea {
    let a = mon.work_area();
    MonitorArea {
        name: mon.name().map(|n| n.to_string()),
        x: a.position.x as f64,
        y: a.position.y as f64,
        w: a.size.width as f64,
        h: a.size.height as f64,
        scale: mon.scale_factor().max(0.5),
        bounds: (
            mon.position().x,
            mon.position().y,
            mon.position().x + mon.size().width as i32,
            mon.position().y + mon.size().height as i32,
        ),
    }
}

/// 全部显示器的工作区和屏幕边界（物理 px）。多屏拖拽应按屏幕边界命中，
/// 不能只看 primary_monitor，否则副屏会被夹回主屏。
fn monitor_areas(app: &AppHandle) -> Vec<MonitorArea> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(area_of_monitor)
        .collect()
}

/// 指针（物理 px）落在第几块屏：先按包含关系找，落在屏间空隙时
/// 取中心最近的一块。纯函数，方便单测。
fn monitor_index_at(rects: &[(f64, f64, f64, f64)], point: (f64, f64)) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }
    for (i, m) in rects.iter().enumerate() {
        if point.0 >= m.0 && point.0 < m.0 + m.2 && point.1 >= m.1 && point.1 < m.1 + m.3 {
            return Some(i);
        }
    }
    let dist = |m: &(f64, f64, f64, f64)| {
        let cx = m.0 + m.2 / 2.0;
        let cy = m.1 + m.3 / 2.0;
        (cx - point.0) * (cx - point.0) + (cy - point.1) * (cy - point.1)
    };
    rects
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            dist(a)
                .partial_cmp(&dist(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

fn monitor_area_at(app: &AppHandle, point: (f64, f64)) -> MonitorArea {
    let areas = monitor_areas(app);
    let rects: Vec<_> = areas
        .iter()
        .map(|area| {
            let (left, top, right, bottom) = area.bounds;
            (
                left as f64,
                top as f64,
                (right - left) as f64,
                (bottom - top) as f64,
            )
        })
        .collect();
    monitor_index_at(&rects, point)
        .and_then(|i| areas.get(i).cloned())
        .unwrap_or_default()
}

/// 配置里保存的显示器（按设备名匹配），找不到时回退主屏/第一块屏。
fn monitor_area_for(app: &AppHandle, name: Option<&str>) -> MonitorArea {
    find_monitor_area(app, name).unwrap_or_default()
}

fn find_monitor_area(app: &AppHandle, name: Option<&str>) -> Option<MonitorArea> {
    let areas = monitor_areas(app);
    name.and_then(|wanted| {
        areas
            .iter()
            .find(|a| a.name.as_deref() == Some(wanted))
            .cloned()
    })
    .or_else(|| {
        app.primary_monitor()
            .ok()
            .flatten()
            .map(|m| area_of_monitor(&m))
    })
    .or_else(|| areas.first().cloned())
}

/// 矩形中心。
fn rect_center((x, y, w, h): (f64, f64, f64, f64)) -> (f64, f64) {
    (x + w / 2.0, y + h / 2.0)
}

/// 主体在画布里的本地矩形（**逻辑 px**，相对 webview 左上角）。
/// 贴边一侧内缩 `BODY_INSET=70`，沿边轴居中（画布中心 390 / 316）。
fn body_local_rect(edge: &str, width: f64, height: f64) -> (f64, f64, f64, f64) {
    let w = width.max(1.0);
    let h = height.max(1.0);
    let cx = ISLAND_WINDOW_W / 2.0;
    let cy = ISLAND_WINDOW_H / 2.0;
    match edge {
        "bottom" => (cx - w / 2.0, ISLAND_WINDOW_H - BODY_INSET - h, w, h),
        "left" => (BODY_INSET, cy - h / 2.0, w, h),
        "right" => (ISLAND_WINDOW_W - BODY_INSET - w, cy - h / 2.0, w, h),
        _ => (cx - w / 2.0, BODY_INSET, w, h),
    }
}

/// 沿边轴主体中心的可用半程：按**主体设计上限** + 液脚 + 安全边距算，
/// 与实际主体尺寸无关——展开/收起改尺寸时窗口位置保持不动。
fn along_half(edge: &str) -> f64 {
    if matches!(edge, "left" | "right") {
        BODY_MAX_H / 2.0 + BODY_FILLET + BODY_EDGE_MARGIN
    } else {
        BODY_MAX_W / 2.0 + BODY_FILLET + BODY_EDGE_MARGIN
    }
}

/// anchored 窗口位置（逻辑 px）：沿边中心 = 边线 + pos_frac×边长（夹取后），
/// 窗口 = 沿边中心 - 画布中心；贴边轴固定内缩 70。
/// **只依赖 边/pos_frac/工作区，与主体尺寸无关**（窗口在开合/切页时不移动）。
fn anchored_window_pos(
    edge: &str,
    pos_frac: f64,
    (ax, ay, aw, ah): (f64, f64, f64, f64),
) -> (f64, f64) {
    let frac = if pos_frac.is_finite() {
        pos_frac.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let half = along_half(edge);
    let cx = ISLAND_WINDOW_W / 2.0;
    let cy = ISLAND_WINDOW_H / 2.0;
    match edge {
        "left" | "right" => {
            let along = (ay + frac * ah).clamp(ay + half, (ay + ah - half).max(ay + half));
            let x = if edge == "left" {
                ax - BODY_INSET
            } else {
                ax + aw + BODY_INSET - ISLAND_WINDOW_W
            };
            (x, along - cy)
        }
        _ => {
            let along = (ax + frac * aw).clamp(ax + half, (ax + aw - half).max(ax + half));
            let y = if edge == "bottom" {
                ay + ah + BODY_INSET - ISLAND_WINDOW_H
            } else {
                ay - BODY_INSET
            };
            (along - cx, y)
        }
    }
}

/// 工作区（物理）→ 逻辑 px。
fn area_logical(area: &MonitorArea) -> (f64, f64, f64, f64) {
    let scale = area.scale.max(0.1);
    (area.x / scale, area.y / scale, area.w / scale, area.h / scale)
}

/// 某条边在画布坐标里的坐标：top/bottom 返回 y，left/right 返回 x。
fn edge_line_canvas(edge: &str, area: (f64, f64, f64, f64), window: (f64, f64)) -> f64 {
    let (ax, ay, aw, ah) = area;
    match edge {
        "bottom" => ay + ah - window.1,
        "left" => ax - window.0,
        "right" => ax + aw - window.0,
        _ => ay - window.1,
    }
}

/// 主体到某条边的有向距离（逻辑 px）：正 = 在屏幕内侧，负 = 冲过边线。
fn gap_to_edge(
    edge: &str,
    local: (f64, f64, f64, f64),
    area: (f64, f64, f64, f64),
    window: (f64, f64),
) -> f64 {
    let line = edge_line_canvas(edge, area, window);
    let (x, y, w, h) = local;
    match edge {
        "bottom" => line - (y + h),
        "left" => x - line,
        "right" => line - (x + w),
        _ => y - line,
    }
}

/// 最近边 + 该边有向距离：取四条边里 gap 最小的（最贴近或冲得最远的边）。
/// 平手顺序 top → bottom → left → right，保证结果确定。
fn nearest_edge(
    area: (f64, f64, f64, f64),
    window: (f64, f64),
    local: (f64, f64, f64, f64),
) -> (&'static str, f64) {
    let mut best = ("top", gap_to_edge("top", local, area, window));
    for edge in ["bottom", "left", "right"] {
        let gap = gap_to_edge(edge, local, area, window);
        if gap < best.1 {
            best = (edge, gap);
        }
    }
    best
}

/// Prefer the horizontal edges when the drag is released.
fn nearest_snap_edge(
    area: (f64, f64, f64, f64),
    body: (f64, f64, f64, f64),
) -> (&'static str, f64) {
    let gap = |edge| gap_to_edge(edge, body, area, (0.0, 0.0));
    let mut nearest = ("top", gap("top"));
    let mut best = nearest.1;
    for (edge, weight) in [("bottom", 1.0), ("left", 1.35), ("right", 1.35)] {
        let distance = gap(edge);
        if distance * weight < best {
            nearest = (edge, distance);
            best = distance * weight;
        }
    }
    nearest
}

fn island_position(app: &AppHandle) -> (f64, f64) {
    let cfg = load_config();
    let (x, y, _, _) = island_geometry(app, &cfg.edge, false, cfg.pos_frac);
    (x, y)
}

/// 当前主体在画布里的本地矩形（拖拽/吸附中取会话里的实时值，
/// 静止时取当前边的 anchored 模型）。穿透判定、`island_state` 都用它。
fn current_body_rect() -> (f64, f64, f64, f64) {
    if let Some(snap) = snap_slot().lock().ok().and_then(|slot| slot.clone()) {
        let (x, y, width, height) = snap.window.local;
        let squash = (-snap.gap.value).max(0.0);
        let growth = squash * 1.1;
        if is_horizontal(&snap.edge) {
            let h = (height - squash).max(height * 0.62);
            let y = if snap.edge == "bottom" { y + height - h } else { y };
            return (x - growth / 2.0, y, width + growth, h);
        }
        let w = (width - squash).max(width * 0.62);
        let x = if snap.edge == "right" { x + width - w } else { x };
        return (x, y - growth / 2.0, w, height + growth);
    }
    if let Some(local) = drag_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().filter(|drag| drag.started).map(|drag| drag.window.local))
    {
        return local;
    }
    let cfg = load_config();
    let (w, h) = current_body_size();
    body_local_rect(&cfg.edge, w, h)
}

/// 灵动岛窗口几何（逻辑 px）：**固定 780×632**，位置由边/pos_frac 决定。
/// 开合/切页/主体尺寸变化都不改窗口（`expanded` 只为兼容旧调用点，忽略）。
fn island_geometry(
    app: &AppHandle,
    edge: &str,
    _expanded: bool,
    pos_frac: f64,
) -> (f64, f64, f64, f64) {
    let cfg = load_config();
    let area = monitor_area_for(app, cfg.monitor.as_deref());
    let (x, y) = anchored_window_pos(edge, pos_frac, dock_area_logical(&area, edge));
    (x, y, ISLAND_WINDOW_W, ISLAND_WINDOW_H)
}

/// 确保窗口存在（隐藏态预热）。必须在主线程事件循环上下文调用。
///
/// 注意：这里**不再挂 Windows Acrylic 系统背板**。Windows 11 浅色模式下
/// Acrylic 会把整个窗口（包括胶囊四周的透明留白）铺成一块浅灰矩形，
/// WebView 再叠加半透明 CSS 玻璃后就出现截图里的白边。现在窗口保持
/// 全透明，WebView 将折射底板和染色裁剪到当前胶囊或面板的轮廓内。
pub fn ensure_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) {
        return Ok(win);
    }
    let (x, y) = island_position(app);
    let cfg = load_config();
    let (_, _, start_w, start_h) = island_geometry(app, &cfg.edge, false, cfg.pos_frac);
    let builder = WebviewWindowBuilder::new(
        app,
        ISLAND_WINDOW_LABEL,
        WebviewUrl::App("island.html".into()),
    )
    .title("AgentHUD Island")
    .inner_size(start_w, start_h)
    .position(x, y)
    .decorations(false)
    .transparent(true)
    .always_on_top(cfg.keep_on_top)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .min_inner_size(1.0, 1.0)
    .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
    .focusable(false)
    .focused(false)
    // 合并 WebView2 渲染进程（内存压降专项，见 webview_args.rs 模块注释）
    .merged_renderer();

    // Windows：隐藏建窗 → 修正尺寸 → 显示，规避 tao 对极窄窗口的默认钳宽。
    #[cfg(target_os = "windows")]
    let win = {
        let win = builder
            .visible(false)
            .build()
            .map_err(|e| format!("创建灵动岛窗口失败: {e}"))?;
        // 双保险：即使系统主题/WebView2 默认色是白的，也把窗口与 WebView
        // 的底层背景置成全透明；CSS 玻璃才有正确的透明底。
        let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        let _ = win.set_size(LogicalSize::new(start_w, start_h));
        let _ = win.show();
        win
    };
    #[cfg(not(target_os = "windows"))]
    let win = {
        let win = builder
            .build()
            .map_err(|e| format!("创建灵动岛窗口失败: {e}"))?;
        let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        let _ = win.show();
        win
    };
    // 窗口创建时已按配置选定置顶层级，此处同步截图排除等窗口属性。
    let _ = apply_window_flags_on_main(app, &cfg);
    Ok(win)
}

/// 一次调用同时提交窗口的位置和尺寸（逻辑 px 进，物理 px 出）。
///
/// Windows 上 `set_size` / `set_position` 是两次 `SetWindowPos`：先改尺寸再挪
/// 位置时，合成器会抓到「新尺寸 + 旧位置」的中间帧。向右吸附的岛以前那个瞬间
/// 会整块往左闪 480px（实机录屏每次展开都能抓到一帧）。这里改用一次
/// `SetWindowPos` 同时提交两者，不留中间态；目标几何没变时直接跳过，避免
/// DWM 重建合成面。
///
/// `scale` 由调用方传入（通常是配置那块屏的比例，见 `island_scale`）：逻辑 px
/// 的设计稿尺寸因此能落到任意缩放/分辨率的屏上。
fn apply_window_frame_scaled(
    win: &WebviewWindow,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    scale: f64,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

        let hwnd = win
            .hwnd()
            .map_err(|e| format!("读取灵动岛窗口句柄失败: {e}"))?;
        // SetWindowPos 收的是外框尺寸，Tauri 的 set_size 是内容区尺寸；
        // 无边框窗口两者相等，这里仍按实际差值补偿，避免以后加边框时错位。
        let outer = win
            .outer_size()
            .map_err(|e| format!("读取灵动岛外框失败: {e}"))?;
        let inner = win
            .inner_size()
            .map_err(|e| format!("读取灵动岛内容区失败: {e}"))?;
        let dw = outer.width as i32 - inner.width as i32;
        let dh = outer.height as i32 - inner.height as i32;
        let px = (x * scale).round() as i32;
        let py = (y * scale).round() as i32;
        let pw = (w * scale).round() as i32 + dw;
        let ph = (h * scale).round() as i32 + dh;
        // 已经是目标几何就别再发 SetWindowPos：每一次都会让 DWM 重建合成面。
        if let Ok(pos) = win.outer_position() {
            if pos.x == px && pos.y == py && outer.width as i32 == pw && outer.height as i32 == ph {
                return Ok(());
            }
        }
        unsafe {
            SetWindowPos(
                HWND(hwnd.0),
                None,
                px,
                py,
                pw,
                ph,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
        .map_err(|e| format!("调整灵动岛窗口失败: {e}"))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        win.set_size(LogicalSize::new(w, h))
            .map_err(|e| format!("调整灵动岛尺寸失败: {e}"))?;
        win.set_position(PhysicalPosition::new(
            (x * scale).round() as i32,
            (y * scale).round() as i32,
        ))
        .map_err(|e| format!("移动灵动岛失败: {e}"))?;
        Ok(())
    }
}

pub(crate) fn set_expanded_on_main(app: &AppHandle, expanded: bool) -> Result<(), String> {
    let win = ensure_window(app)?;
    if motion_in_progress() {
        LAST_EXPANDED.store(expanded, Ordering::Relaxed);
        return Ok(());
    }
    let cfg = load_config();
    if expanded {
        raise_topmost_on_main(&win, cfg.keep_on_top);
    }
    // **窗口尺寸/位置固定不动**，开合只切 CSS 与主体布局。
    //
    // 透明窗口一改尺寸，DWM 会丢掉旧的合成面重建，WebView2 重绘之前的那 1~2
    // 帧窗口是空的：展开时看着像「整块岛先横向跳一下 → 消失 → 动画才开始」，
    // 收起时看着像「收缩动画演完了，竖条才闪一下冒出来」。
    //
    // 液态画布固定 780×632、anchored 位置只由 边/pos_frac/工作区 决定，
    // 主体尺寸变化只改画布内布局；这里调 apply 通常是空操作（几何没变会跳过）。
    // 透明留白的点击穿透由 start_mouse_through_watcher 按主体矩形 + 液脚判定。
    let (x, y, w, h) = island_geometry(app, &cfg.edge, expanded, cfg.pos_frac);
    apply_window_frame_scaled(&win, x, y, w, h, island_scale(app))?;
    LAST_EXPANDED.store(expanded, Ordering::Relaxed);
    Ok(())
}

/// 在展开及重新显示时把置顶窗口移到该层最前面，保持当前焦点。
fn raise_topmost_on_main(win: &WebviewWindow, keep_on_top: bool) {
    if !keep_on_top || !win.is_visible().unwrap_or(false) {
        return;
    }

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        };

        // Tauri 会跳过未变化的置顶标志。原生请求可调整同层 Z 序，也不会重建边框。
        if let Ok(hwnd) = win.hwnd() {
            if let Err(error) = unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                )
            } {
                eprintln!("[zbar-island] 提升灵动岛置顶顺序失败: {error}");
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(window) = win.ns_window() {
        // 调用方在主线程，且 `window` 是该 WebviewWindow 当前持有的 NSWindow。
        unsafe {
            let window: &objc2_app_kit::NSWindow = &*window.cast();
            let _: () = objc2::msg_send![window, orderFrontRegardless];
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = win.set_always_on_top(true);
}

/// 建窗/重新显示时保持不可聚焦，避免 WebView2 建控件的副作用抢走用户
/// 正在输入的编辑器焦点。只有用户点进设置输入框时，前端才调用
/// island_set_keyboard(true) 临时激活并聚焦窗口；点其它位置/收起后恢复。
fn arm_window_after_show(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) {
        let _ = win.set_focusable(false);
    }
}

/// 把配置里的「窗口级」标志落到岛窗口（主线程）：始终置顶、截图/录屏排除。
///
/// - `keep_on_top`：Tauri 的 `set_always_on_top`（Windows 走 HWND_TOPMOST）；
/// - `exclude_from_capture`：Windows 用 `SetWindowDisplayAffinity(
///   WDA_EXCLUDEFROMCAPTURE)`，其它平台没有等价能力，静默忽略（字段仍会
///   随配置广播）。`WDA_EXCLUDEFROMCAPTURE` 需要 Windows 10 2004+，老系统
///   API 会返回失败，只记日志不阻断保存。
///
/// `backdrop` 根据窗口当前的采集排除状态选择正下方或附近的桌面取样。
/// 采集标志由此处一次性应用，与「出现在截图和录屏里」同步。
fn apply_window_flags_on_main(app: &AppHandle, cfg: &IslandConfig) -> Result<(), String> {
    let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) else {
        return Ok(());
    };
    win.set_always_on_top(cfg.keep_on_top)
        .map_err(|e| format!("设置灵动岛置顶失败: {e}"))?;
    raise_topmost_on_main(&win, cfg.keep_on_top);
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
        };
        let hwnd = win
            .hwnd()
            .map_err(|e| format!("读取灵动岛窗口句柄失败: {e}"))?;
        let affinity = if cfg.exclude_from_capture {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            WDA_NONE
        };
        unsafe { SetWindowDisplayAffinity(HWND(hwnd.0), affinity) }
            .map_err(|e| format!("设置灵动岛采集排除失败: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = cfg.exclude_from_capture;
    Ok(())
}

// ============================================================
// 开关 / 服务
// ============================================================

fn emit_config(app: &AppHandle, config: &IslandConfig) {
    let _ = app.emit(ISLAND_CONFIG_EVENT, config.clone());
}

fn start_services(app: &AppHandle) {
    if SERVICES_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    weather::start(app.clone());
    stocks::start(app.clone());
    media::start(app.clone());
    system::start(app.clone());
    clipboard::start(app.clone());
    notify::start(app.clone());
    start_fullscreen_watcher(app.clone());
    start_hover_watcher(app.clone());
    // 可穿透：WS_EX_TRANSPARENT + WH_MOUSE_LL（配置热更新/岛隐藏自动收敛）
    passthrough::start(app.clone());
    // 透明留白穿透：不启动的话胶囊四周那圈透明像素会吃掉下面的点击
    #[cfg(target_os = "windows")]
    start_mouse_through_watcher(app.clone());
    // 额度复用 hud_bar 数据线程；start_feed 幂等，首次会立即推送一拍。
    crate::hud_bar::start_feed(app.clone());
    crate::hud_bar::hud_bar_refresh(app.clone());
    // 启动时就按当前 hidden_tabs 收敛一次：上面无条件拉起的服务里，被隐藏
    // 板块对应的要立刻停掉（截图/录屏预热窗口的销毁在 sync 内部异步做），
    // 否则「关掉的板块」在应用重启后会重新跑起来。prev=None 表示可见板块
    // 刚被拉起，sync 只处理隐藏方向。
    let cfg = load_config();
    sync_tab_services(app, &cfg, None);
}

fn stop_services() {
    if !SERVICES_STARTED.swap(false, Ordering::SeqCst) {
        return;
    }
    weather::stop();
    stocks::stop();
    media::stop();
    system::stop();
    clipboard::stop();
    notify::stop();
    // 音频峰值采样线程由前端首次 invoke 懒启动（之后 16ms 一拍常驻），与上面
    // 六个服务的生命周期不同，这里一并挂起；下次岛打开、音乐视图再次 invoke
    // 时由命令自行重启（见 audio_peak::suspend）。
    audio_peak::suspend();
    // 实时字幕：岛总开关关闭 = 面板整体消失，正在聆听的字幕会话停掉采集，
    // 并尽力补最后一句话（失败静默）；没有活动会话时是幂等空操作。
    live::stop();
    // 旧停靠条若仍开启，额度线程需要继续供它使用；否则一起停掉。
    if !crate::hud_bar::load_hud_bar_config().enabled {
        crate::hud_bar::stop_feed();
    }
    // 会议录音：岛总开关关闭 = 整个面板消失，正在录的会议按正常停止流程收尾
    // （WAV finalize + 侧车 JSON 落盘，keeper 线程停流退出），不保留后台录音。
    // 收尾要整段读写 WAV（自动音量），长录音可能耗时数秒，放后台线程避免阻塞
    // 主线程的窗口 / 设置保存操作；没有活动会话时 stop_on_panel_close 返回 None。
    std::thread::spawn(|| {
        if let Some(result) = meeting::stop_on_panel_close() {
            match result {
                Ok(saved) => {
                    eprintln!("[zbar-island] 灵动岛关闭，会议录音已保存: {}", saved.path)
                }
                Err(e) => eprintln!("[zbar-island] 灵动岛关闭后停止会议录音失败: {e}"),
            }
        }
    });
}

/// 板块是否在 `hidden_tabs` 里（配置已在 `clamped()` 里做过白名单/排序/去重）。
fn tab_hidden(cfg: &IslandConfig, key: &str) -> bool {
    cfg.hidden_tabs.iter().any(|item| item == key)
}

/// 按 `hidden_tabs` 收敛后台服务：隐藏的板块停掉对应线程，从隐藏重新显示的
/// 板块再拉起；可见性没变化的板块一律不动。
///
/// 同步执行：`stocks`/`clipboard`/`system`/`notify` 的 start/stop 只翻原子标志
/// 并起线程，毫秒级返回；截图窗口的销毁/预热较重（主线程窗口 API + WebView），
/// 在内部另起线程做，本函数不等待（见 `screenshot::set_tab_visible`）。
/// 工具板块的会议录音停止也另起线程：`meeting::stop_session` 的 finalize /
/// 自动音量要整段读写 WAV（长录音可达数秒），不能阻塞配置应用（`apply_config`）。
///
/// `prev`：上一份配置。`None` = [`start_services`] 启动路径——六个服务刚被
/// 无条件拉起，可见板块已在线，这里只处理「隐藏」方向。这样既不会在启动时
/// 重复 start 出两套线程（重复抓行情 / 重读通知库 / 重复 PowerShell 亮度
/// 回读），也不会在热更新时把本来就在跑的服务重启一遍（通知线程重启会重读
/// 历史，把未读态刷成已读；消息板块从隐藏→显示已改走 `notify::resume`，
/// 只重启线程、不重读历史）。
fn sync_tab_services(app: &AppHandle, cfg: &IslandConfig, prev: Option<&IslandConfig>) {
    // 岛总开关关着时全部服务都已停：只允许 stop 方向，不能把可见板块拉起来。
    let services_running = SERVICES_STARTED.load(Ordering::SeqCst);
    let hidden = |key: &str| tab_hidden(cfg, key);
    // 只有真的发生显隐翻转才动手（stop/start 本身幂等，但重启线程有实际代价：
    // 重抓行情、重读通知库把未读态刷成已读）；启动路径（prev=None）的无条件
    // 拉起算「刚变成显示」的反面，这里只处理隐藏方向。
    let was_hidden = |key: &str| prev.map(|p| tab_hidden(p, key)).unwrap_or(false);
    let newly_hidden = |key: &str| hidden(key) && (prev.is_none() || !was_hidden(key));
    let should_start =
        |key: &str| !hidden(key) && services_running && was_hidden(key);
    // 股票：隐藏即停掉 400ms 唤醒 + 按 interval 的行情抓取
    if newly_hidden("stock") {
        stocks::stop();
    } else if should_start("stock") {
        stocks::start(app.clone());
    }
    // 剪贴板：隐藏即停掉 800ms 的剪贴板监听
    if newly_hidden("clip") {
        clipboard::stop();
    } else if should_start("clip") {
        clipboard::start(app.clone());
    }
    // 控制：隐藏即停掉系统状态轮询（Windows 上还省掉每 10s 一次的
    // PowerShell 亮度回读）
    if newly_hidden("ctl") {
        system::stop();
    } else if should_start("ctl") {
        system::start(app.clone());
    }
    // 消息：隐藏即停掉 toast 轮询，同时停掉它带起的飞书/钉钉浮窗扫描、
    // DeepSeek 完成监听、IMAP 邮件轮询与任务栏闪烁 hook（它们都挂在
    // notify 的线程组里，由同一个 STOP/GEN 控制）。重新显示走 `resume`：
    // 只重启线程，不重读历史，否则内存里的未读会被 `init_history` 整体
    // 刷成已读（未读徽标凭空清零）。
    if newly_hidden("msg") {
        notify::stop();
    } else if should_start("msg") {
        notify::resume(app.clone());
    }
    // 工具：隐藏即兜底结束正在进行的会议录音。走正常停止流程（finalize 回填
    // WAV 头 → 自动音量 → rename 正式文件 → 写侧车 JSON），keeper 线程收到
    // ControlMsg::Stop 后停流退出，不留下后台录音。finalize / 自动音量会整段
    // 读写 WAV，长录音可能耗时数秒，必须放后台线程，不能阻塞配置应用；没有
    // 活动会话时 stop_on_panel_close 直接返回 None。
    if newly_hidden("tool") {
        std::thread::spawn(|| {
            if let Some(result) = meeting::stop_on_panel_close() {
                match result {
                    Ok(saved) => {
                        eprintln!("[zbar-island] 工具板块关闭，会议录音已保存: {}", saved.path)
                    }
                    Err(e) => eprintln!("[zbar-island] 工具板块关闭后停止会议录音失败: {e}"),
                }
            }
        });
    }
    // 天气 / 媒体不随板块停：主页天气 hero、收起态 rings/clock/music 仍在用
    // 它们的数据，隐藏标签只是不显示对应页面，数据要继续刷新。
    // translate 没有后台线程（翻译是命令式调用），隐藏后前端不再挂载页面即可；
    // tool 的会议录音兜底停止见上面的分支。
    // 截图：隐藏 → 销毁隐藏的全屏覆盖窗与录制条待命窗（全屏隐藏 WebView，
    // 常驻会一直占渲染进程内存/GPU）；显示 → 延迟 1.5s 再预热。内部自带
    // 「截图/录屏进行中不销毁」守卫，放后台线程避免阻塞配置保存。
    let shot_hidden = tab_hidden(cfg, "shot");
    let shot_needs_sync = match prev {
        // 启动路径：隐藏的要清掉残留窗口；可见的仍交给 lib.rs 的 2.5s / +1.5s
        // 启动预热，不提前抢 WebView 资源。
        None => shot_hidden,
        // 配置热更新：只有显隐真的翻转才动手（预热虽幂等，但没必要每次改
        // 其它板块都起一个延迟线程）。
        Some(p) => tab_hidden(p, "shot") != shot_hidden,
    };
    if shot_needs_sync {
        let app_shot = app.clone();
        std::thread::spawn(move || {
            let _ = tauri::async_runtime::block_on(screenshot::set_tab_visible(
                app_shot, !shot_hidden,
            ));
        });
    }
}

fn set_enabled_on_main(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let mut cfg = load_config();
    cfg.enabled = enabled;
    cfg = cfg.clamped();
    save_config(&cfg)?;
    if enabled {
        ensure_window(app)?;
        if let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) {
            let cfg = load_config();
            // always 常驻展开：窗口几何直接按展开态摆（macOS 收起窗更小），
            // 否则重新开启后的第一帧会先露出收起条再被前端撑开。
            let expanded = cfg.expand_mode == "always";
            let (x, y, w, h) = island_geometry(app, &cfg.edge, expanded, cfg.pos_frac);
            let _ = apply_window_frame_scaled(&win, x, y, w, h, island_scale(app));
            LAST_EXPANDED.store(expanded, Ordering::Relaxed);
            let _ = win.show();
        }
        arm_window_after_show(app);
        let _ = apply_window_flags_on_main(app, &cfg);
        start_services(app);
        // 窗口可能被全屏守护隐藏过，重新显示时清掉标记
        FULLSCREEN_HIDDEN.store(false, Ordering::Relaxed);
    } else {
        clear_motion();
        if let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) {
            // 关闭时把窗口摆回常驻几何再隐藏：页面 WebView 保留，重新开启时
            // 直接以同一尺寸恢复，不需要再改窗口。
            let cfg = load_config();
            let (x, y, w, h) = island_geometry(app, &cfg.edge, false, cfg.pos_frac);
            let _ = apply_window_frame_scaled(&win, x, y, w, h, island_scale(app));
            let _ = win.hide();
        }
        LAST_EXPANDED.store(false, Ordering::Relaxed);
        let _ = app.emit(ISLAND_COLLAPSE_EVENT, ());
        // 灵动岛设置窗口是岛的附属窗口（岛屿展开面板齿轮打开的独立大窗）：
        // 岛关了它既没有入口也不再有用，必须一并收起，否则留下"岛已消失、
        // 设置窗还开着"的孤儿窗口。首开懒创建，窗口可能从未建过，隐藏函数
        // 内部静默跳过；set_enabled_on_main 的全部调用点都在主线程，满足
        // 窗口 API 的线程要求（见 island_settings_window.rs 模块 doc）
        crate::island_settings_window::hide_island_settings_window_on_main(app);
        // 桌面消息浮窗同理：岛总开关关掉后不再有新消息来源（stop_services
        // 会停掉通知线程），窗口若还留在屏幕上就是一块剥不掉的孤儿玻璃。
        // 用 on_config_changed 而不是只 hide：它同时把「岛已关闭」推给浮窗
        // 页面，页面据此清掉手上的卡片——否则快速关→开之后旧卡会跟着新卡
        // 一起重新显示出来。
        popup::on_config_changed(app);
        stop_services();
    }
    let _ = app.emit(
        "zbar://island-state",
        serde_json::json!({ "enabled": enabled }),
    );
    emit_config(app, &cfg);
    Ok(enabled)
}

/// 托盘菜单开关（on_menu_event / tray_menu_action 主线程回调内直接调用）。
/// 返回切换后的开启态。
pub fn toggle_from_tray(app: &AppHandle) -> Result<bool, String> {
    let enabled = !load_config().enabled;
    set_enabled_on_main(app, enabled)
}

/// 注册灵动岛全局快捷键（默认 Ctrl+Alt+Shift+I 开关岛、Ctrl+Alt+J 翻译
/// 剪贴板、Ctrl+Alt+V 打开剪贴板）。
///
/// 与主面板快捷键共用 tauri-plugin-global-shortcut：lib.rs 的
/// apply_shortcut 每次都会先 unregister_all，再依次注册面板快捷键与这里，
/// 因此两边不会互相覆盖。三个岛热键按「开关 → 剪贴板 → 翻译」的顺序注册，
/// 组合与前面重复的直接跳过，避免插件报「已注册」以及两个动作互相覆盖。
/// 注册失败（被占用/格式非法）只记日志，不阻断应用启动；设置页改热键后由
/// lib.rs 主动重新应用。
pub fn register_hotkeys(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let cfg = load_config();
    let gs = app.global_shortcut();
    if !cfg.hotkey_toggle.trim().is_empty() {
        if let Err(e) = gs.on_shortcut(cfg.hotkey_toggle.as_str(), move |app, _shortcut, event| {
            // 同截图热键：macOS 按下/松开各派发一次，只认按下，否则会“开→关”。
            if !matches!(
                event.state(),
                tauri_plugin_global_shortcut::ShortcutState::Pressed
            ) {
                return;
            }
            let app_main = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(e) = toggle_from_tray(&app_main) {
                    eprintln!("[zbar-island] 热键切换失败: {e}");
                }
            });
        }) {
            eprintln!(
                "[zbar-island] 注册灵动岛热键 {} 失败（可能被占用或格式非法）: {e}",
                cfg.hotkey_toggle
            );
        }
    }
    // 剪贴板热键：展开岛并记住剪贴板页。与开关热键相同时跳过，避免插件
    // 报「已注册」以及两个动作互相覆盖。
    let clipboard_key = cfg.hotkey_clipboard.trim().to_string();
    if !clipboard_key.is_empty() && clipboard_key != cfg.hotkey_toggle.trim() {
        if let Err(e) = gs.on_shortcut(clipboard_key.as_str(), move |app, _shortcut, event| {
            if !matches!(
                event.state(),
                tauri_plugin_global_shortcut::ShortcutState::Pressed
            ) {
                return;
            }
            let app_main = app.clone();
            // 只投递、不等待：回调可能在主线程上派发，同步等待会自锁。
            let _ = app.run_on_main_thread(move || open_clipboard_tab_on_main(&app_main));
        }) {
            eprintln!(
                "[zbar-island] 注册剪贴板热键 {clipboard_key} 失败（可能被占用或格式非法）: {e}"
            );
        }
    }
    // 翻译剪贴板热键：读文本 → 后台翻译 → 译文写回剪贴板。与开关 / 剪贴板
    // 热键重复时跳过，避免插件报「已注册」以及两个动作互相覆盖。
    let translate_key = cfg.hotkey_translate.trim().to_string();
    if !translate_key.is_empty()
        && translate_key != cfg.hotkey_toggle.trim()
        && translate_key != clipboard_key
    {
        if let Err(e) = gs.on_shortcut(translate_key.as_str(), move |_app, _shortcut, event| {
            // 同其它热键：macOS 按下/松开各派发一次，只认按下。
            if !matches!(
                event.state(),
                tauri_plugin_global_shortcut::ShortcutState::Pressed
            ) {
                return;
            }
            // 网络请求与 arboard 读写都在后台线程，绝不阻塞热键回调 / 主线程。
            std::thread::spawn(translate_clipboard_on_worker);
        }) {
            eprintln!(
                "[zbar-island] 注册翻译热键 {translate_key} 失败（可能被占用或格式非法）: {e}"
            );
        }
    }
}

/// 翻译剪贴板热键的后台动作：读文本 → 翻译 → 译文写回剪贴板并通知。
///
/// 全程在热键回调 spawn 出来的线程里执行（arboard 读写与网络请求都不允许
/// 阻塞热键回调 / 主线程）；失败只发通知，不弹错误对话框、不写日志。
fn translate_clipboard_on_worker() {
    let text = match read_clipboard_text() {
        Ok(Some(text)) if !text.trim().is_empty() => text,
        Ok(_) => {
            crate::notify("翻译".into(), "剪贴板里没有文本".into());
            return;
        }
        Err(error) => {
            crate::notify("翻译失败".into(), error);
            return;
        }
    };
    let translation = match translate::translate_saved(&text) {
        Ok(result) => result.translation,
        Err(error) => {
            crate::notify("翻译失败".into(), error);
            return;
        }
    };
    if let Err(error) = write_clipboard_text(&translation) {
        crate::notify("翻译失败".into(), error);
        return;
    }
    let preview: String = translation.chars().take(120).collect();
    crate::notify("翻译完成".into(), preview);
}

/// 读剪贴板文本：`Ok(None)` = 当前内容不是文本；瞬时占用最多重试 3 次。
fn read_clipboard_text() -> Result<Option<String>, String> {
    let mut last_error = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(80));
        }
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => match clipboard.get_text() {
                Ok(text) => return Ok(Some(text)),
                Err(arboard::Error::ContentNotAvailable) => return Ok(None),
                Err(error) => last_error = format!("读取剪贴板失败: {error}"),
            },
            Err(error) => last_error = format!("剪贴板不可用: {error}"),
        }
    }
    Err(last_error)
}

/// 写剪贴板文本：与剪贴板轮询线程互斥，避免「正在读大图」时抢不到剪贴板。
fn write_clipboard_text(text: &str) -> Result<(), String> {
    let _writing = clipboard::begin_write();
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("剪贴板不可用: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("写回剪贴板失败: {e}"))
}

/// 剪贴板热键动作：展开岛并记住剪贴板页。
///
/// 前端没有独立的「切页」Rust 事件，这里复用现有路径：把 `last_tab` 置为
/// `clip` 落盘并广播完整配置，再广播 `ISLAND_EXPAND_EVENT` 让前端走统一
/// 展开流程（前端收到后调用 `island_set_expanded` 收敛后端状态）。全程在
/// 主线程同步完成，不等待任何跨线程结果，避免自等待死锁。
fn open_clipboard_tab_on_main(app: &AppHandle) {
    let mut cfg = load_config();
    if !cfg.enabled {
        // 热键是全局的：岛关着时按剪贴板热键，顺带把岛重新打开。
        if let Err(e) = set_enabled_on_main(app, true) {
            eprintln!("[zbar-island] 剪贴板热键唤起灵动岛失败: {e}");
            return;
        }
        cfg = load_config();
    }
    cfg.last_tab = Some("clip".into());
    let next = cfg.clamped();
    if let Err(e) = save_config(&next) {
        eprintln!("[zbar-island] 剪贴板热键记住标签页失败: {e}");
        return;
    }
    emit_config(app, &next);
    if let Err(e) = ensure_window(app) {
        eprintln!("[zbar-island] 剪贴板热键恢复窗口失败: {e}");
        return;
    }
    if let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) {
        let _ = win.show();
    }
    let _ = app.emit(ISLAND_EXPAND_EVENT, ());
}

/// 应用启动挂点：开关开启时恢复窗口与所有数据线程。
pub fn start_if_enabled(app: &AppHandle) {
    let cfg = load_config();
    if !cfg.enabled {
        return;
    }
    if let Err(e) = ensure_window(app) {
        eprintln!("[agenthud-island] 恢复灵动岛失败: {e}");
        return;
    }
    arm_window_after_show(app);
    let _ = apply_window_flags_on_main(app, &cfg);
    // always 常驻展开：启动就按展开态摆好窗口（macOS 收起窗只有条高，
    // 不主动改几何会先闪一条再被撑开）。hover/never 保持收起几何。
    if cfg.expand_mode == "always" {
        if let Err(e) = set_expanded_on_main(app, true) {
            eprintln!("[zbar-island] 恢复常驻展开失败: {e}");
        }
    } else {
        LAST_EXPANDED.store(false, Ordering::Relaxed);
    }
    start_services(app);
}

/// 主线程投递 + 等待结果（命令侧统一走这个，避免同步命令自等待死锁）
async fn on_main_thread<T, F>(app: &AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<T, String>>();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| format!("投递主线程失败: {e}"))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "等待主线程超时".to_string())?
}

// ============================================================
// 全屏隐藏
// ============================================================

type ScreenBounds = (i32, i32, i32, i32);

#[cfg(windows)]
fn foreground_fullscreen() -> Option<ScreenBounds> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowLongW, GetWindowRect,
        GetWindowThreadProcessId, GWL_STYLE, WS_CAPTION, WS_MAXIMIZE,
    };

    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() || foreground == unsafe { GetShellWindow() } {
        return None;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(foreground, Some(&mut pid)) };
    if pid == std::process::id() {
        return None;
    }
    let mut class_name = [0u16; 80];
    let name_len = unsafe { GetClassNameW(foreground, &mut class_name) }.max(0) as usize;
    if matches!(
        String::from_utf16_lossy(&class_name[..name_len]).as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    ) {
        return None;
    }
    let style = unsafe { GetWindowLongW(foreground, GWL_STYLE) } as u32;
    if (style & WS_MAXIMIZE.0) != 0 && (style & WS_CAPTION.0) == WS_CAPTION.0 {
        return None;
    }
    let mut window = RECT::default();
    unsafe { GetWindowRect(foreground, &mut window) }.ok()?;
    let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    let screen = info.rcMonitor;
    (window.left <= screen.left
        && window.top <= screen.top
        && window.right >= screen.right
        && window.bottom >= screen.bottom)
        .then_some((screen.left, screen.top, screen.right, screen.bottom))
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct FullscreenHug {
    monitor: ScreenBounds,
    spring: MotionSpring,
    last_tick: Instant,
}

#[cfg(windows)]
static FULLSCREEN_HUG: std::sync::Mutex<Option<FullscreenHug>> = std::sync::Mutex::new(None);

#[cfg(any(windows, test))]
fn extend_work_area(
    (x, y, width, height): (f64, f64, f64, f64),
    (left, top, right, bottom): (f64, f64, f64, f64),
    edge: &str,
    amount: f64,
) -> (f64, f64, f64, f64) {
    let amount = amount.clamp(0.0, 1.0);
    match edge {
        "top" => {
            let y1 = y + (top - y) * amount;
            (x, y1, width, y + height - y1)
        }
        "bottom" => (x, y, width, height + (bottom - y - height) * amount),
        "left" => {
            let x1 = x + (left - x) * amount;
            (x1, y, x + width - x1, height)
        }
        _ => (x, y, width + (right - x - width) * amount, height),
    }
}

fn dock_area_logical(area: &MonitorArea, _edge: &str) -> (f64, f64, f64, f64) {
    let work = area_logical(area);
    #[cfg(windows)]
    if let Some(hug) = FULLSCREEN_HUG
        .lock()
        .ok()
        .and_then(|state| *state)
        .filter(|hug| hug.monitor == area.bounds)
    {
        let (left, top, right, bottom) = area.bounds;
        let bounds = (
            left as f64 / area.scale,
            top as f64 / area.scale,
            right as f64 / area.scale,
            bottom as f64 / area.scale,
        );
        return extend_work_area(work, bounds, _edge, hug.spring.value);
    }
    work
}

#[cfg(windows)]
fn update_fullscreen_on_main(
    app: &AppHandle,
    foreground: Option<ScreenBounds>,
) -> Result<bool, String> {
    let cfg = load_config();
    if !cfg.enabled {
        return Ok(false);
    }
    let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) else {
        return Ok(false);
    };
    let hidden = cfg.hide_fullscreen && foreground.is_some();
    let was_hidden = FULLSCREEN_HIDDEN.load(Ordering::Relaxed);
    if hidden && !was_hidden {
        clear_motion();
        win.hide().map_err(|e| format!("隐藏灵动岛失败: {e}"))?;
        FULLSCREEN_HIDDEN.store(true, Ordering::Relaxed);
        LAST_EXPANDED.store(false, Ordering::Relaxed);
        let _ = app.emit(ISLAND_COLLAPSE_EVENT, ());
    }
    if !hidden && (motion_in_progress() || left_button_down()) {
        return Ok(false);
    }

    // 显示模式切换期间系统可能暂时没有工作区，此时等下一轮再定位。
    let Some(area) = find_monitor_area(app, cfg.monitor.as_deref())
        .filter(|area| area.w > 0.0 && area.h > 0.0)
    else {
        return Ok(false);
    };
    let now = Instant::now();
    let target = if !hidden && cfg.hug_fullscreen && foreground == Some(area.bounds) {
        1.0
    } else {
        0.0
    };
    let mut state = FULLSCREEN_HUG.lock().map_err(|_| "无法获取全屏贴边状态")?;
    let hug = match state.as_mut().filter(|hug| hug.monitor == area.bounds) {
        Some(hug) => hug,
        None => state.insert(FullscreenHug {
            monitor: area.bounds,
            spring: MotionSpring {
                epsilon: 0.004,
                ..MotionSpring::new(target, 320.0, 0.9)
            },
            last_tick: now,
        }),
    };
    hug.spring.target = target;
    hug.spring.step(motion_dt(now, hug.last_tick));
    hug.last_tick = now;
    let needs_frame = hug.spring.value != hug.spring.target || hug.spring.velocity != 0.0;
    drop(state);

    // Windows 可能在显示模式更新后再次移动窗口，工作区与 DPI 也可在屏幕边界
    // 不变时更新。每次检查可见窗口的实际位置与尺寸，已贴边时跳过 SetWindowPos。
    if !hidden && (was_hidden || win.is_visible().unwrap_or(false)) {
        let (x, y) =
            anchored_window_pos(&cfg.edge, cfg.pos_frac, dock_area_logical(&area, &cfg.edge));
        apply_window_frame_scaled(&win, x, y, ISLAND_WINDOW_W, ISLAND_WINDOW_H, area.scale)?;
        if was_hidden {
            win.show().map_err(|e| format!("显示灵动岛失败: {e}"))?;
            raise_topmost_on_main(&win, cfg.keep_on_top);
            FULLSCREEN_HIDDEN.store(false, Ordering::Relaxed);
        }
    }
    Ok(needs_frame)
}

#[cfg(windows)]
fn start_fullscreen_watcher(app: AppHandle) {
    if FULLSCREEN_WATCHER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = thread::Builder::new()
        .name("zbar-island-fullscreen".into())
        .spawn(move || {
            // 空闲拍 3s：全屏应用切换是分钟级事件，原先 1s 一拍的
            // block_on 主线程同步往返（on_main_thread 内含 5s 超时）在
            // 主线程挂死/繁忙时每拍白等满超时，还往主线程死队列堆闭包
            // ——这是岛失效的传导路径之一（见 popup.rs 事故记录）。
            let interval = Duration::from_secs(3);
            let mut last_check = Instant::now() - interval;
            let mut foreground = None;
            // 连续失败退避（3s→6s→12s→24s，封顶 30s）：任何一次成功即复原。
            let mut failures: u32 = 0;
            loop {
                if last_check.elapsed() >= interval {
                    foreground = foreground_fullscreen();
                    last_check = Instant::now();
                }
                let app_main = app.clone();
                let animated = match tauri::async_runtime::block_on(on_main_thread(&app, move || {
                    update_fullscreen_on_main(&app_main, foreground)
                })) {
                    Ok(animated) => {
                        failures = 0;
                        animated
                    }
                    Err(error) => {
                        failures += 1;
                        eprintln!("[zbar-island] 更新灵动岛全屏状态失败(连续 {failures}): {error}");
                        false
                    }
                };
                thread::sleep(if animated {
                    Duration::from_millis(16)
                } else if failures > 0 {
                    Duration::from_secs((3u64 << (failures - 1).min(4)).min(30))
                } else {
                    interval.saturating_sub(last_check.elapsed())
                });
            }
        });
}

#[cfg(not(windows))]
fn start_fullscreen_watcher(_app: AppHandle) {}

// ============================================================
// 命令
// ============================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandState {
    pub enabled: bool,
    pub config: IslandConfig,
    pub expanded: bool,
    /// 主体在画布里的当前矩形（逻辑 px，相对 webview 左上角）
    pub body_rect: BodyRect,
    /// 系统双击间隔（毫秒），用于在单击展开之前等待第二次点击。
    pub double_click_ms: u32,
}

#[cfg(target_os = "windows")]
fn double_click_time_ms() -> u32 {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
    unsafe { GetDoubleClickTime() }
}

#[cfg(not(target_os = "windows"))]
fn double_click_time_ms() -> u32 {
    300
}

#[tauri::command]
pub fn island_state() -> IslandState {
    let cfg = load_config();
    IslandState {
        enabled: cfg.enabled,
        config: cfg,
        expanded: LAST_EXPANDED.load(Ordering::Relaxed),
        body_rect: BodyRect::from_tuple(current_body_rect()),
        double_click_ms: double_click_time_ms(),
    }
}

/// 应用一份完整配置：clamped → SQLite 落盘 → 广播 → 相关服务 / 窗口副作用。
///
/// `island_set_config` 命令与 `island::settings` 的设置文档同步共用这一条
/// 路径：广播、天气 / 股票 / 消息刷新、热键重注册、窗口置顶 / 采集排除都
/// 只有一份实现。只有真正变化的项才触发重注册 / 线程刷新，不会每次保存
/// 都把后台线程全量重启（`enabled` 不变时不会走 `set_enabled_on_main`）。
pub(crate) async fn apply_config(
    app: &AppHandle,
    config: IslandConfig,
) -> Result<IslandConfig, String> {
    let next = config.clamped();
    let prev = load_config();
    save_config(&next)?;

    let weather_changed = next.weather_city != prev.weather_city
        || next.weather_lat != prev.weather_lat
        || next.weather_lon != prev.weather_lon
        || next.weather_cities != prev.weather_cities;
    let stocks_changed = next.stocks != prev.stocks
        || next.stock_interval_sec != prev.stock_interval_sec
        || next.stock_alert_pct != prev.stock_alert_pct;
    // 三个岛热键都真正参与注册：任一变化都要 unregister_all + 重注册。
    let hotkey_changed = next.hotkey_toggle != prev.hotkey_toggle
        || next.hotkey_translate != prev.hotkey_translate
        || next.hotkey_clipboard != prev.hotkey_clipboard;
    let notify_prefs_changed = next.notify_modules != prev.notify_modules
        || next.notify_body_preview != prev.notify_body_preview;
    // 板块显隐：隐藏 / 重新显示的板块要立即收敛对应后台服务（见下方
    // sync_tab_services；不新增轮询，只按需 start/stop 已有线程）。
    let hidden_tabs_changed = next.hidden_tabs != prev.hidden_tabs;
    // 桌面浮窗（notify.popup.*）：开关 / 位置 / 尺寸、玻璃色板（主题）、模块
    // 过滤、所在显示器任一变化，都要把新快照推给浮窗（前端据此重新量尺寸并
    // 让 Rust 重新定位）。不新增轮询，全部走事件。
    let popup_cfg_changed = next.enabled != prev.enabled
        || next.notify_popup_enabled != prev.notify_popup_enabled
        || next.notify_popup_position != prev.notify_popup_position
        || next.notify_popup_size != prev.notify_popup_size
        || next.theme != prev.theme
        || next.notify_modules != prev.notify_modules
        || next.monitor != prev.monitor;
    let window_flags_changed = next.keep_on_top != prev.keep_on_top
        || next.exclude_from_capture != prev.exclude_from_capture;
    let enabled = next.enabled;
    if enabled != prev.enabled {
        let app_main = app.clone();
        on_main_thread(app, move || set_enabled_on_main(&app_main, enabled)).await?;
    }
    // 板块显隐收敛：按 prev → next 的差集停/起对应线程，可见性没变的板块
    // 不动（不会因为隐藏消息而重启股票/通知线程）。总开关同时变化时，
    // 上面的 set_enabled_on_main 已经整体启停过全部服务（start_services
    // 末尾也按最新 hidden_tabs 同步过一次），这里不再重复 sync。
    if hidden_tabs_changed && enabled == prev.enabled {
        sync_tab_services(app, &next, Some(&prev));
    }
    // 置顶 / 截图排除是窗口 API，必须在主线程；失败只记日志（老系统
    // 没有 WDA_EXCLUDEFROMCAPTURE 时 SetWindowDisplayAffinity 会失败，
    // 不能让设置保存因此失败）。
    if window_flags_changed {
        let app_main = app.clone();
        let flags = next.clone();
        if let Err(e) = on_main_thread(app, move || {
            apply_window_flags_on_main(&app_main, &flags)
        })
        .await
        {
            eprintln!("[zbar-island] 应用窗口标志失败: {e}");
        }
        // 桌面消息浮窗跟随同一个「截图 / 录屏排除」开关：共享画面里不该
        // 出现通知正文（浮窗可能是懒创建的，这里按当前窗口存在与否处理）
        popup::sync_capture_flag(app);
    }
    // 展开方式三态变化：后端不直接改 LAST_EXPANDED / 窗口几何，只广播
    // 展开 / 收起事件，由前端调用 island_set_expanded 收敛——
    // 避免「面板仍显示展开、后端已按收起态判定穿透」这类错位。
    if next.enabled && next.expand_mode != prev.expand_mode {
        match next.expand_mode.as_str() {
            "always" => {
                let _ = app.emit(ISLAND_EXPAND_EVENT, ());
            }
            "never" => {
                let _ = app.emit(ISLAND_COLLAPSE_EVENT, ());
            }
            _ => {}
        }
    }
    emit_config(app, &next);
    if popup_cfg_changed {
        popup::on_config_changed(app);
    }
    if weather_changed {
        weather::refresh();
    }
    if stocks_changed {
        stocks::refresh_now();
    }
    // 消息模块开关/正文预览变化：立刻按新配置重算可见列表并广播一次，
    // 前端磁贴勾选后无需等下一拍轮询，也不用重启。
    if notify_prefs_changed {
        notify::refresh();
    }
    if hotkey_changed {
        let app_hotkey = app.clone();
        let _ = app.run_on_main_thread(move || crate::refresh_global_shortcuts(&app_hotkey));
    }
    Ok(next)
}

#[tauri::command]
pub async fn island_set_config(
    config: IslandConfig,
    app: AppHandle,
) -> Result<IslandConfig, String> {
    apply_config(&app, config).await
}

#[tauri::command]
pub async fn island_set_enabled(enabled: bool, app: AppHandle) -> Result<bool, String> {
    let app_main = app.clone();
    on_main_thread(&app, move || set_enabled_on_main(&app_main, enabled)).await
}

#[tauri::command]
pub async fn island_toggle(app: AppHandle) -> Result<bool, String> {
    let enabled = !load_config().enabled;
    let app_main = app.clone();
    on_main_thread(&app, move || set_enabled_on_main(&app_main, enabled)).await
}

#[tauri::command]
pub async fn island_set_expanded(expanded: bool, app: AppHandle) -> Result<(), String> {
    let app_main = app.clone();
    on_main_thread(&app, move || set_expanded_on_main(&app_main, expanded)).await
}

/// 前端上报主体（岛本体）尺寸：`width/height` 逻辑 px，`expanded` 表示是否
/// 展开态（用于状态提示；窗口位置与主体尺寸无关，开合不会移动窗口）。
/// 未上报时 Rust 回退 300×54；尺寸会被夹进设计上限 640×492。
#[tauri::command]
pub fn island_set_body_size(width: f64, height: f64, expanded: bool) {
    let size = clamp_body_size(width, height);
    if let Ok(mut slot) = body_size_slot().lock() {
        *slot = size;
    }
    BODY_EXPANDED_HINT.store(expanded, Ordering::Relaxed);
}

/// 可穿透开关（Windows 真实生效）：写配置、广播最新配置、回传新配置。
///
/// 实际的 `WS_EX_TRANSPARENT` 与 `WH_MOUSE_LL` 钩子由 `passthrough` 模块的
/// 常驻协调线程在约 50ms 内收敛（配置热更新、岛隐藏/关闭自动卸载还原），
/// 本命令保持同步、不做窗口 API 调用，避免在命令线程上碰窗口。
#[tauri::command]
pub fn island_set_passthrough(on: bool, app: AppHandle) -> Result<IslandConfig, String> {
    let mut cfg = load_config();
    cfg.passthrough = on;
    let next = cfg.clamped();
    save_config(&next)?;
    // 协调线程若曾因 spawn 失败没起来，这里补一次（start 幂等；失败会回滚标记）。
    passthrough::start(app.clone());
    // 广播给所有窗口（岛面板 / 设置窗），让「可穿透」勾选状态同步。
    emit_config(&app, &next);
    Ok(next)
}

/// 右键菜单关闭后前端调用：立刻结束「临时可点击」窗口期，把
/// `WS_EX_TRANSPARENT` 加回去（不等 2s 兜底）。幂等。
#[tauri::command]
pub fn island_passthrough_restore() {
    passthrough::restore_after_menu();
}

/// 记住当前标签页 key（home/msg/stock/weather/ctl/clip/shot/tool）。
/// 非法 key 会被 `clamped()` 置为 None，下次打开回退 `defaultTab`。
#[tauri::command]
pub fn island_set_last_tab(key: String, app: AppHandle) -> Result<IslandConfig, String> {
    let mut cfg = load_config();
    cfg.last_tab = Some(key);
    let next = cfg.clamped();
    save_config(&next)?;
    // 广播完整最新配置：设置窗 / 托盘等其它窗口的「上次页面」状态同步。
    emit_config(&app, &next);
    Ok(next)
}

/// 采集液态玻璃纹理。版本未变化时省略回应；纹理不可用时返回没有图片的版本。
#[tauri::command]
pub async fn island_backdrop(
    app: AppHandle,
    known_revision: Option<u32>,
    sample_neighbor: Option<bool>,
) -> Result<Option<backdrop::BackdropFrame>, String> {
    #[cfg(target_os = "windows")]
    {
        let win = app
            .get_webview_window(ISLAND_WINDOW_LABEL)
            .ok_or_else(|| "灵动岛窗口不存在".to_string())?;
        if !win.is_visible().unwrap_or(false) {
            return Err("灵动岛不可见".into());
        }
        let pos = win
            .outer_position()
            .map_err(|e| format!("读取窗口位置失败: {e}"))?;
        let size = win
            .outer_size()
            .map_err(|e| format!("读取窗口尺寸失败: {e}"))?;
        let hwnd = win.hwnd().map_err(|e| format!("读取窗口句柄失败: {e}"))?.0 as isize;
        return tauri::async_runtime::spawn_blocking(move || {
            backdrop::capture(
                hwnd,
                pos.x,
                pos.y,
                size.width,
                size.height,
                known_revision,
                sample_neighbor.unwrap_or(true),
            )
        })
        .await
        .map_err(|e| format!("背景抓取任务失败: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, known_revision, sample_neighbor);
        Err("桌面毛玻璃采样仅支持 Windows".into())
    }
}

/// 额度首帧快照：前端窗口可能晚于数据线程启动，单靠事件会错过首拍。
#[tauri::command]
pub fn island_quota_snapshot() -> Option<crate::hud_bar::HudBarUsage> {
    crate::hud_bar::latest_usage()
}

/// 今日 token / 预估花费首帧快照（托盘标题线程算完后缓存）。
#[tauri::command]
pub fn island_today_usage() -> Option<crate::today_usage::TodayUsageTotals> {
    crate::today_usage::totals()
}

#[tauri::command]
pub async fn island_refresh_quota(app: AppHandle) -> Result<(), String> {
    crate::hud_bar::hud_bar_refresh(app);
    Ok(())
}

#[tauri::command]
pub fn island_open_path(path: String) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("路径为空".into());
    }
    open::that(trimmed).map_err(|e| format!("打开失败: {e}"))
}

#[tauri::command]
pub async fn island_quit(app: AppHandle) {
    app.exit(0);
}

/// 输入框聚焦前后的窗口激活开关：不可聚焦窗口不抢焦点，只有真正要打字
/// 时才临时设为可聚焦并 focus；点击其它区域/收起后恢复不可聚焦。
#[tauri::command]
pub async fn island_set_keyboard(active: bool, app: AppHandle) -> Result<(), String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        let win = ensure_window(&app_main)?;
        win.set_focusable(active)
            .map_err(|e| format!("切换窗口键盘状态失败: {e}"))?;
        if active {
            let _ = win.set_focus();
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn island_config() -> IslandConfig {
    load_config()
}

/// 快捷键盘点：设置页可据此提示「热键被占用」。只读查询，不修改注册。
#[tauri::command]
pub fn island_hotkey_registered(app: AppHandle) -> bool {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let key = load_config().hotkey_toggle;
    !key.trim().is_empty() && app.global_shortcut().is_registered(key.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stock_list_is_kept() {
        // 删光自选是合法状态；不能像旧实现那样在 clamped() 里偷偷补回默认股。
        let cfg = IslandConfig {
            stocks: Vec::new(),
            ..IslandConfig::default()
        }
        .clamped();
        assert!(cfg.stocks.is_empty());
    }

    #[test]
    fn font_size_is_clamped() {
        let low = IslandConfig {
            font_size: 1.0,
            ..IslandConfig::default()
        }
        .clamped();
        let high = IslandConfig {
            font_size: 99.0,
            ..IslandConfig::default()
        }
        .clamped();
        let bad = IslandConfig {
            font_size: f64::NAN,
            ..IslandConfig::default()
        }
        .clamped();
        assert!((low.font_size - 10.0).abs() < 1e-9);
        assert!((high.font_size - 18.0).abs() < 1e-9);
        assert!((bad.font_size - 13.0).abs() < 1e-9);
    }

    #[test]
    fn monitor_index_at_handles_negative_secondary() {
        let mons = [(0.0, 0.0, 2752.0, 1152.0), (-2560.0, -82.0, 2560.0, 1440.0)];
        assert_eq!(monitor_index_at(&mons, (100.0, 500.0)), Some(0));
        assert_eq!(monitor_index_at(&mons, (-100.0, 500.0)), Some(1));
        assert_eq!(monitor_index_at(&mons, (-2560.0, -82.0)), Some(1));
        assert_eq!(monitor_index_at(&[], (0.0, 0.0)), None);
    }

    #[test]
    fn empty_monitor_name_is_dropped() {
        let cfg = IslandConfig {
            monitor: Some("   ".into()),
            ..IslandConfig::default()
        }
        .clamped();
        assert!(cfg.monitor.is_none());
    }

    #[test]
    fn drag_spring_converges_without_blowing_up() {
        let mut axis = MotionSpring::new(0.0, 900.0, 0.78);
        axis.target = 240.0;
        for _ in 0..120 {
            axis.step(1.0 / 60.0);
        }
        assert_eq!(axis.value, 240.0);
        assert!(axis.at_rest());
    }

    #[test]
    fn agent_order_is_trimmed_and_deduped() {
        let cfg = IslandConfig {
            agent_order: vec![
                " zai:1 ".into(),
                "".into(),
                "zai:1".into(),
                "dsh".into(),
                "  ".into(),
            ],
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(
            cfg.agent_order,
            vec!["zai:1".to_string(), "dsh".to_string()]
        );
        // 默认（空数组）= 数据源自己的顺序，不写死任何账号
        assert!(IslandConfig::default().clamped().agent_order.is_empty());
    }

    #[test]
    fn theme_and_compact_view_fall_back_to_defaults() {
        // 合法值（含大小写/空白归一）原样保留。
        let cfg = IslandConfig {
            theme: " SKY ".into(),
            compact_view: "Pet".into(),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.theme, "sky");
        assert_eq!(cfg.compact_view, "pet");
        // auto / light / dark / rouge 都合法。
        for theme in ["auto", "light", "dark", "rouge"] {
            let cfg = IslandConfig {
                theme: theme.into(),
                ..IslandConfig::default()
            }
            .clamped();
            assert_eq!(cfg.theme, theme);
        }
        // 非法主题回退 dark。
        let cfg = IslandConfig {
            theme: "neon".into(),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.theme, "dark");
        let cfg = IslandConfig {
            theme: String::new(),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.theme, "dark");
        // 非法收起态视图回退 rings；rings/clock/music 合法。
        for view in ["rings", "clock", "music"] {
            let cfg = IslandConfig {
                compact_view: view.into(),
                ..IslandConfig::default()
            }
            .clamped();
            assert_eq!(cfg.compact_view, view);
        }
        let cfg = IslandConfig {
            compact_view: "chart".into(),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.compact_view, "rings");
    }

    #[test]
    fn quota_display_runtime_uses_camel_case_and_defaults_to_percent() {
        let legacy: IslandConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.quota_display, "percent");
        for (raw, expected) in [
            (" RiNgS ", "rings"),
            ("percent", "percent"),
            ("invalid", "percent"),
        ] {
            let cfg: IslandConfig = serde_json::from_value(serde_json::json!({
                "quotaDisplay": raw
            }))
            .unwrap();
            let saved = serde_json::to_value(cfg.clamped()).unwrap();
            assert_eq!(saved["quotaDisplay"], expected);
        }
    }

    #[test]
    fn new_timing_and_blur_fields_are_clamped() {
        let cfg = IslandConfig {
            hover_expand_ms: 9_999,
            collapse_delay_ms: 9_999,
            auto_cycle_sec: 9_999,
            blur_fps: 0,
            passthrough: true,
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.hover_expand_ms, 2_000);
        assert_eq!(cfg.collapse_delay_ms, 4_000);
        assert_eq!(cfg.auto_cycle_sec, 600);
        assert_eq!(cfg.blur_fps, 1);
        assert!(cfg.passthrough);
        // 0 是合法值（0ms = 立刻；0 秒 = 关闭自动轮换）。
        let cfg = IslandConfig {
            hover_expand_ms: 0,
            collapse_delay_ms: 0,
            auto_cycle_sec: 0,
            blur_fps: 120,
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.hover_expand_ms, 0);
        assert_eq!(cfg.collapse_delay_ms, 0);
        assert_eq!(cfg.auto_cycle_sec, 0);
        assert_eq!(cfg.blur_fps, 60);
        // 默认值就是契约值。
        let default = IslandConfig::default();
        assert_eq!(default.hover_expand_ms, 120);
        assert_eq!(default.collapse_delay_ms, 320);
        assert_eq!(default.auto_cycle_sec, 30);
        assert_eq!(default.blur_fps, 8);
        assert!(!default.passthrough);
        assert_eq!(default.theme, "dark");
        assert_eq!(default.compact_view, "rings");
        assert_eq!(default.default_tab, "home");
        assert!(default.last_tab.is_none());
    }

    #[test]
    fn media_and_open_fields_default_clamp_and_roundtrip() {
        let default = IslandConfig::default();
        assert!(default.media_auto_show);
        assert!(default.media_lyrics);
        assert!(default.media_netease_log);
        assert_eq!(default.media_lyrics_sources, "netease,lrclib,custom");
        assert_eq!(default.media_lyrics_custom_url, "");
        assert_eq!(
            default.media_priority,
            vec![
                "cloudmusic".to_string(),
                "bilibili".to_string(),
                "chrome".to_string(),
                "zen".to_string(),
                "msedge".to_string(),
                "firefox".to_string()
            ]
        );
        assert!(default.notify_open.is_empty());

        // 歌词源：保留用户顺序 + 小写 + 白名单 + 去重；全非法回默认
        assert_eq!(
            normalize_lyrics_sources(" LRCLIB , netease ,lrclib,bogus"),
            "lrclib,netease"
        );
        assert_eq!(normalize_lyrics_sources("custom"), "custom");
        assert_eq!(normalize_lyrics_sources(""), "netease,lrclib,custom");
        assert_eq!(normalize_lyrics_sources(" , , "), "netease,lrclib,custom");
        assert_eq!(normalize_lyrics_sources("bogus"), "netease,lrclib,custom");

        // 优先级：trim / 去空 / 大小写不敏感去重 / 最多 16
        let many: Vec<String> = (0..20).map(|i| format!("p{i}")).collect();
        let mut with_dups = vec![" Chrome ".to_string(), String::new(), "chrome".into()];
        with_dups.extend(many);
        let priority = normalize_media_priority(&with_dups);
        assert_eq!(priority.len(), 16);
        assert_eq!(priority[0], "Chrome");
        assert_eq!(priority[1], "p0");
        assert!(normalize_media_priority(&[]).is_empty());

        // notify_open：白名单 key、trim 值、空值删除
        let raw = std::collections::BTreeMap::from([
            ("QQ".to_string(), "  C:\\QQ\\QQ.exe  ".to_string()),
            ("dsh".to_string(), "  ".to_string()),
            ("bogus".to_string(), "x".to_string()),
        ]);
        let open = normalize_notify_open(raw);
        assert_eq!(open.len(), 1);
        assert_eq!(open.get("qq").map(String::as_str), Some("C:\\QQ\\QQ.exe"));

        // 自定义接口 URL：trim + 限长 2048
        let long = format!("  https://x/{}  ", "a".repeat(3000));
        let cfg = IslandConfig {
            media_lyrics_sources: "custom,custom,netease".into(),
            media_lyrics_custom_url: long,
            media_priority: with_dups,
            notify_open: std::collections::BTreeMap::from([
                ("wechat".to_string(), " D:\\WeChat.exe ".to_string()),
                ("bogus".to_string(), "nope".to_string()),
            ]),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.media_lyrics_sources, "custom,netease");
        assert_eq!(cfg.media_lyrics_custom_url.chars().count(), 2048);
        assert!(cfg.media_lyrics_custom_url.starts_with("https://x/"));
        assert_eq!(cfg.media_priority.len(), 16);
        assert_eq!(
            cfg.notify_open.get("wechat").map(String::as_str),
            Some("D:\\WeChat.exe")
        );
        assert!(!cfg.notify_open.contains_key("bogus"));
    }

    #[test]
    fn last_tab_and_default_tab_validate_tab_keys() {
        // 合法 key 全部接受（含 trim / 大小写归一）。
        assert_eq!(ISLAND_TAB_KEYS.len(), 9, "与前端 TabKey 一致");
        assert!(
            !ISLAND_TAB_KEYS.contains(&"settings"),
            "settings 已迁到独立窗口，不再是岛内标签"
        );
        for key in ISLAND_TAB_KEYS {
            let cfg = IslandConfig {
                last_tab: Some(format!(" {key} ")),
                default_tab: key.to_string(),
                ..IslandConfig::default()
            }
            .clamped();
            assert_eq!(cfg.last_tab.as_deref(), Some(key));
            assert_eq!(cfg.default_tab, key);
        }
        // lastTab 非法 → None（不偷偷回退 defaultTab）。
        // "settings" 已迁到独立设置窗口，不再是岛内合法标签。
        for bad in ["bogus", "", "  ", "HOME!", "setting", "settings"] {
            let cfg = IslandConfig {
                last_tab: Some(bad.into()),
                ..IslandConfig::default()
            }
            .clamped();
            assert!(cfg.last_tab.is_none(), "last_tab {bad:?} 应置 None");
        }
        // defaultTab 非法 → home（settings 同样不合法）
        for bad in ["bogus", "settings"] {
            let cfg = IslandConfig {
                default_tab: bad.into(),
                ..IslandConfig::default()
            }
            .clamped();
            assert_eq!(cfg.default_tab, "home");
        }
        let cfg = IslandConfig {
            default_tab: String::new(),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.default_tab, "home");
    }

    #[test]
    fn expand_mode_and_edge_accept_new_values() {
        // expandMode：hover / always / never 三态；always 不再被回退成 hover。
        for mode in ["hover", "always", "never"] {
            let cfg = IslandConfig {
                expand_mode: mode.into(),
                ..IslandConfig::default()
            }
            .clamped();
            assert_eq!(cfg.expand_mode, mode);
        }
        let cfg = IslandConfig {
            expand_mode: " ALWAYS ".into(),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.expand_mode, "always");
        let cfg = IslandConfig {
            expand_mode: "pinned".into(),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.expand_mode, "hover");
        // edge：top / bottom / left / right 四边；bottom 不再被回退成 top。
        for edge in ["top", "bottom", "left", "right"] {
            let cfg = IslandConfig {
                edge: edge.into(),
                ..IslandConfig::default()
            }
            .clamped();
            assert_eq!(cfg.edge, edge);
        }
        let cfg = IslandConfig {
            edge: "center".into(),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.edge, "top");
    }

    #[test]
    fn notify_module_lists_are_normalized() {
        // 乱序 / 重复 / 大小写 / 非法值 → 白名单 + 固定顺序 + 去重
        let input = vec![
            " DingTalk ".to_string(),
            " DSH ".into(),
            "QQ".into(),
            "wechat".into(),
            "qq".into(),
            "telegram".into(),
            "".into(),
        ];
        assert_eq!(
            normalize_notify_kinds(&input),
            vec![
                "qq".to_string(),
                "wechat".to_string(),
                "dingtalk".to_string(),
                "dsh".to_string(),
            ]
        );
        // 允许为空（= 该功能整体关闭）
        assert!(normalize_notify_kinds(&[]).is_empty());
        assert!(normalize_notify_kinds(&["bogus".into()]).is_empty());
        // clamped：保留合法项、非法项丢弃（不偷偷回退默认）
        let cfg = IslandConfig {
            notify_modules: vec!["DSH".into(), "MAIL".into(), "x".into()],
            notify_body_preview: vec!["dsh".into(), "mail".into()],
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.notify_modules, vec!["mail", "dsh"]);
        assert_eq!(cfg.notify_body_preview, vec!["mail"]);
        // 默认值契约：modules = qq/wechat/mail，bodyPreview = feishu/dingtalk
        let default = IslandConfig::default();
        assert_eq!(
            default.notify_modules,
            vec!["qq".to_string(), "wechat".to_string(), "mail".to_string()]
        );
        assert_eq!(
            default.notify_body_preview,
            vec!["feishu".to_string(), "dingtalk".to_string()]
        );
        let clamped = default.clamped();
        assert_eq!(
            clamped.notify_modules,
            vec!["qq".to_string(), "wechat".to_string(), "mail".to_string()]
        );
        assert_eq!(
            clamped.notify_body_preview,
            vec!["feishu".to_string(), "dingtalk".to_string()]
        );
    }

    #[test]
    fn hidden_tabs_are_whitelisted() {
        // 只保留可隐藏板块的 key：
        // home 与未知值一律丢弃，重复去重、顺序稳定。
        // 工具板块已变为可隐藏（tool 保留），旧配置里被丢弃的 tool 从现在起生效。
        let cfg = IslandConfig {
            hidden_tabs: vec![
                "stock".into(),
                "home".into(),
                "translate".into(),
                "settings".into(),
                "msg".into(),
                "tool".into(),
                "bogus".into(),
                "stock".into(),
            ],
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.hidden_tabs, vec!["msg", "stock", "tool", "translate"]);
        // 全非法 → 空（= 全部显示）
        let cfg = IslandConfig {
            hidden_tabs: vec!["home".into(), "settings".into()],
            ..IslandConfig::default()
        }
        .clamped();
        assert!(cfg.hidden_tabs.is_empty());
    }

    #[test]
    fn tool_tab_is_hideable_for_meeting_fallback() {
        // 工具板块必须留在可隐藏白名单里：隐藏后由 sync_tab_services 兜底
        // 结束正在进行的会议录音（正常保存，keeper 线程停流退出）。
        assert!(valid_tab_key("tool"));
        assert!(can_hide_tab("tool"));
        let cfg = IslandConfig {
            hidden_tabs: vec!["tool".into()],
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.hidden_tabs, vec!["tool".to_string()]);
    }

    #[test]
    fn serde_uses_camel_case_and_fills_missing_fields() {
        // 落盘用 camelCase（前端契约）。
        let value = serde_json::to_value(IslandConfig::default()).unwrap();
        assert_eq!(value["keepOnTop"], true);
        assert_eq!(value["hoverExpand"], true);
        assert_eq!(value["hoverExpandMs"], 120);
        assert_eq!(value["collapseDelayMs"], 320);
        assert_eq!(value["compactView"], "rings");
        assert_eq!(value["autoCycleSec"], 30);
        assert_eq!(value["blurFps"], 8);
        assert_eq!(value["passthrough"], false);
        assert_eq!(value["defaultTab"], "home");
        assert_eq!(value["expandMode"], "hover");
        assert_eq!(value["theme"], "dark");
        // 天气：默认不绑定主城市（跟随 IP），自定义城市列表为空
        assert_eq!(value["weatherCity"], "");
        assert!(value["weatherCities"].as_array().unwrap().is_empty());
        assert!(value["lastTab"].is_null());
        assert_eq!(
            value["notifyModules"],
            serde_json::json!(["qq", "wechat", "mail"])
        );
        assert_eq!(
            value["notifyBodyPreview"],
            serde_json::json!(["feishu", "dingtalk"])
        );
        // 媒体 /「打开应用」新字段：camelCase + 默认值
        assert_eq!(value["mediaAutoShow"], true);
        assert_eq!(value["mediaLyrics"], true);
        assert_eq!(value["mediaNeteaseLog"], true);
        assert_eq!(value["mediaLyricsSources"], "netease,lrclib,custom");
        assert_eq!(value["mediaLyricsCustomUrl"], "");
        assert_eq!(
            value["mediaPriority"],
            serde_json::json!(["cloudmusic", "bilibili", "chrome", "zen", "msedge", "firefox"])
        );
        assert!(value["notifyOpen"].is_object());
        // 旧库 payload（缺所有新字段）必须能反序列化并回退默认值。
        let legacy = r#"{"enabled":true,"transparency":40,"fontSize":13.0,
            "expandMode":"hover","weatherCity":"上海","edge":"top","posFrac":0.5}"#;
        let cfg: IslandConfig = serde_json::from_str(legacy).expect("旧库 payload 必须兼容");
        assert_eq!(cfg.transparency, 40);
        assert_eq!(cfg.theme, "dark");
        assert!(cfg.keep_on_top);
        assert!(cfg.hover_expand);
        assert_eq!(cfg.hover_expand_ms, 120);
        assert_eq!(cfg.collapse_delay_ms, 320);
        assert_eq!(cfg.compact_view, "rings");
        assert_eq!(cfg.auto_cycle_sec, 30);
        assert_eq!(cfg.blur_fps, 8);
        assert!(!cfg.passthrough);
        assert!(cfg.last_tab.is_none());
        assert_eq!(cfg.default_tab, "home");
        assert!(cfg.hidden_tabs.is_empty(), "旧配置默认显示翻译板块");
        // 消息模块开关缺字段时回退默认（旧库兼容）
        assert_eq!(
            cfg.notify_modules,
            vec!["qq".to_string(), "wechat".to_string(), "mail".to_string()]
        );
        assert_eq!(
            cfg.notify_body_preview,
            vec!["feishu".to_string(), "dingtalk".to_string()]
        );
        // 媒体 /「打开应用」缺字段同样回退默认
        assert!(cfg.media_auto_show);
        assert!(cfg.media_lyrics);
        assert!(cfg.media_netease_log);
        assert_eq!(cfg.media_lyrics_sources, "netease,lrclib,custom");
        assert_eq!(cfg.media_priority.len(), 6);
        assert!(cfg.notify_open.is_empty());
        // 新字段可以按 camelCase 读回。
        let cfg: IslandConfig = serde_json::from_str(
            r#"{"edge":"bottom","expandMode":"always","theme":"rouge","keepOnTop":false,
                "compactView":"music","lastTab":"stock","defaultTab":"weather",
                "hoverExpandMs":200,"collapseDelayMs":500,"autoCycleSec":0,
                "blurFps":12,"passthrough":true,
                "notifyModules":["feishu","qq","feishu"],"notifyBodyPreview":[]}"#,
        )
        .unwrap();
        let cfg = cfg.clamped();
        assert_eq!(cfg.edge, "bottom");
        assert_eq!(cfg.expand_mode, "always");
        assert!(!cfg.keep_on_top);
        assert_eq!(cfg.theme, "rouge");
        assert_eq!(cfg.compact_view, "music");
        assert_eq!(cfg.last_tab.as_deref(), Some("stock"));
        assert_eq!(cfg.default_tab, "weather");
        assert_eq!(cfg.hover_expand_ms, 200);
        assert_eq!(cfg.collapse_delay_ms, 500);
        assert_eq!(cfg.auto_cycle_sec, 0);
        assert_eq!(cfg.blur_fps, 12);
        assert!(cfg.passthrough);
        // 归一化：固定顺序 + 去重；空数组保持为空（= 关闭）
        assert_eq!(
            cfg.notify_modules,
            vec!["qq".to_string(), "feishu".to_string()]
        );
        assert!(cfg.notify_body_preview.is_empty());
    }

    #[test]
    fn weather_city_list_normalizes_and_legacy_default_migrates() {
        // 旧版本默认「上海」且无坐标/展示名 → 清空（升级后跟随公网 IP）
        let legacy: IslandConfig = serde_json::from_str(r#"{"weatherCity":"上海"}"#).unwrap();
        let legacy = legacy.clamped();
        assert_eq!(legacy.weather_city, "");

        // 带坐标的「上海」是显式绑定，不能被旧默认迁移清掉
        let bound: IslandConfig = serde_json::from_str(
            r#"{"weatherCity":"上海","weatherLat":31.23,"weatherLon":121.47}"#,
        )
        .unwrap();
        let bound = bound.clamped();
        assert_eq!(bound.weather_city, "上海");

        // 绑定「上海」但坐标留空：sync_to_island 会回填 label=城市名，
        // 非空 label 必须让绑定存活（否则设置页 / 绑定命令无法绑定上海）
        let bound = IslandConfig {
            weather_city: "上海".into(),
            weather_label: "上海".into(),
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(bound.weather_city, "上海");
        assert_eq!(bound.weather_label, "上海");
        assert_eq!(bound.weather_lat, None);

        // 空主城市时 label / 坐标一起清掉，避免旧坐标残留
        let mut cfg = IslandConfig::default();
        cfg.weather_label = "上海 · 徐汇区".into();
        cfg.weather_lat = Some(31.23);
        cfg.weather_lon = Some(121.47);
        let cfg = cfg.clamped();
        assert_eq!(cfg.weather_label, "");
        assert_eq!(cfg.weather_lat, None);
        assert_eq!(cfg.weather_lon, None);

        // 自定义城市：trim、跳过空名、坐标非法降级 None、name+坐标去重、上限 20
        let mut cities = vec![
            WeatherCityCfg {
                name: " 杭州 ".into(),
                lat: Some(30.27),
                lon: Some(120.15),
            },
            WeatherCityCfg {
                name: "   ".into(),
                lat: Some(1.0),
                lon: Some(1.0),
            },
            WeatherCityCfg {
                name: "杭州".into(),
                lat: Some(30.27),
                lon: Some(120.15),
            },
            WeatherCityCfg {
                name: "杭州".into(),
                lat: Some(31.0),
                lon: Some(121.0),
            },
            WeatherCityCfg {
                name: "越界".into(),
                lat: Some(120.0),
                lon: Some(30.0),
            },
        ];
        for index in 0..30 {
            cities.push(WeatherCityCfg {
                name: format!("城市{index}"),
                lat: Some(20.0 + index as f64 * 0.1),
                lon: Some(100.0),
            });
        }
        let cfg = IslandConfig {
            weather_cities: cities,
            ..IslandConfig::default()
        }
        .clamped();
        assert_eq!(cfg.weather_cities.len(), 20, "最多 20 个城市");
        assert_eq!(cfg.weather_cities[0].name, "杭州");
        assert_eq!(cfg.weather_cities[0].lat, Some(30.27));
        assert_eq!(cfg.weather_cities[1].name, "杭州");
        assert_eq!(
            cfg.weather_cities[1].lat,
            Some(31.0),
            "同名不同坐标保留（去重按 name+坐标）"
        );
        assert_eq!(cfg.weather_cities[2].name, "越界");
        assert_eq!(cfg.weather_cities[2].lat, None, "越界坐标降级为 None");
        assert_eq!(cfg.weather_cities[2].lon, None);
    }

    #[test]
    fn hover_ticks_are_never_zero() {
        // 悬停守护按 120ms 一拍换算：0ms 也必须至少 1 拍，避免永远不触发。
        assert_eq!(ticks_for_ms(0, 120), 1);
        assert_eq!(ticks_for_ms(120, 120), 1);
        assert_eq!(ticks_for_ms(121, 120), 2);
        assert_eq!(ticks_for_ms(320, 120), 3);
        assert_eq!(ticks_for_ms(2000, 120), 17);
        assert_eq!(ticks_for_ms(4000, 120), 34);
        assert_eq!(ticks_for_ms(500, 0), 1);
    }

    fn area_primary() -> (f64, f64, f64, f64) {
        (0.0, 0.0, 1920.0, 1080.0)
    }

    #[test]
    fn anchored_placements_for_four_edges() {
        let a = area_primary();
        // top：窗口 y = 边线 - 70；主体本地 y=70；窗口 x = 沿边中心 - 390
        assert_eq!(anchored_window_pos("top", 0.5, a), (570.0, -70.0));
        assert_eq!(body_local_rect("top", 300.0, 54.0), (240.0, 70.0, 300.0, 54.0));
        // bottom：窗口 y = 边线 + 70 - 632；主体本地 y = 632 - 70 - h
        assert_eq!(anchored_window_pos("bottom", 0.5, a), (570.0, 518.0));
        assert_eq!(
            body_local_rect("bottom", 300.0, 54.0),
            (240.0, 508.0, 300.0, 54.0)
        );
        // left：窗口 x = 边线 - 70；主体本地 x=70；窗口 y = 沿边中心 - 316
        assert_eq!(anchored_window_pos("left", 0.5, a), (-70.0, 224.0));
        assert_eq!(body_local_rect("left", 300.0, 54.0), (70.0, 289.0, 300.0, 54.0));
        // right：窗口 x = 边线 + 70 - 780；主体本地 x = 780 - 70 - w
        assert_eq!(anchored_window_pos("right", 0.5, a), (1210.0, 224.0));
        assert_eq!(
            body_local_rect("right", 300.0, 54.0),
            (410.0, 289.0, 300.0, 54.0)
        );
        // 主体屏幕中心 = 窗口 + 本地中心：四边分别贴住对应边线
        let centers: Vec<(f64, f64)> = ["top", "bottom", "left", "right"]
            .iter()
            .map(|edge| {
                let (wx, wy) = anchored_window_pos(edge, 0.5, a);
                let (lcx, lcy) = rect_center(body_local_rect(edge, 300.0, 54.0));
                (wx + lcx, wy + lcy)
            })
            .collect();
        assert_eq!(centers[0], (960.0, 27.0));
        assert_eq!(centers[1], (960.0, 1053.0));
        assert_eq!(centers[2], (150.0, 540.0));
        assert_eq!(centers[3], (1770.0, 540.0));
    }

    #[test]
    fn anchored_window_is_stable_across_body_size_changes() {
        let a = area_primary();
        // 窗口位置只由 边/pos_frac/工作区 决定：主体尺寸 300×54 ↔ 640×492 不变
        for edge in ["top", "bottom", "left", "right"] {
            let before = anchored_window_pos(edge, 0.5, a);
            let after = anchored_window_pos(edge, 0.5, a);
            assert_eq!(before, after);
            // 主体中心沿边方向保持不动（只有垂直贴边方向随尺寸变）
            let small = body_local_rect(edge, 300.0, 54.0);
            let large = body_local_rect(edge, 640.0, 492.0);
            let (scx, scy) = rect_center(small);
            let (lcx, lcy) = rect_center(large);
            if matches!(edge, "top" | "bottom") {
                assert_eq!(before.0 + scx, after.0 + lcx);
            } else {
                assert_eq!(before.1 + scy, after.1 + lcy);
            }
        }
    }

    #[test]
    fn anchored_body_tracks_resolution_scale_and_work_area_changes() {
        let original = MonitorArea {
            x: -2560.0,
            y: -100.0,
            w: 2560.0,
            h: 1400.0,
            bounds: (-2560, -100, 0, 1340),
            ..MonitorArea::default()
        };
        let resized = MonitorArea {
            x: -1920.0,
            w: 1920.0,
            h: 1040.0,
            bounds: (-1920, -100, 0, 980),
            ..original.clone()
        };
        let scaled = MonitorArea { scale: 1.25, ..resized.clone() };
        let new_work_area = MonitorArea {
            x: -1880.0,
            w: 1880.0,
            h: 1000.0,
            ..scaled.clone()
        };
        let fraction = 0.6;
        for area in [original, resized, scaled, new_work_area] {
            for edge in ["top", "bottom", "left", "right"] {
                let (x, y) = anchored_window_pos(edge, fraction, dock_area_logical(&area, edge));
                let (body_x, body_y, body_w, body_h) = body_local_rect(edge, 300.0, 54.0);
                let left = (x * area.scale).round() + body_x * area.scale;
                let top = (y * area.scale).round() + body_y * area.scale;
                let width = body_w * area.scale;
                let height = body_h * area.scale;
                let (actual_edge, expected_edge) = match edge {
                    "top" => (top, area.y),
                    "bottom" => (top + height, area.y + area.h),
                    "left" => (left, area.x),
                    _ => (left + width, area.x + area.w),
                };
                let (along, expected_along) = if matches!(edge, "left" | "right") {
                    (top + height / 2.0, area.y + area.h * fraction)
                } else {
                    (left + width / 2.0, area.x + area.w * fraction)
                };
                assert!((actual_edge - expected_edge).abs() <= 0.5, "{edge} {area:?}");
                assert!((along - expected_along).abs() <= 0.5, "{edge} {area:?}");
            }
        }
    }

    #[test]
    fn anchored_clamps_along_edge_and_negative_secondary() {
        let a = area_primary();
        // 0% / 100%：夹到「主体上限半宽 + 13 液脚 + 6 边距」= 339
        assert_eq!(anchored_window_pos("top", 0.0, a).0, 339.0 - 390.0);
        assert_eq!(anchored_window_pos("top", 1.0, a).0, 1920.0 - 339.0 - 390.0);
        // 左右夹到 492/2 + 19 = 265
        assert_eq!(anchored_window_pos("left", 0.0, a).1, 265.0 - 316.0);
        assert_eq!(anchored_window_pos("left", 1.0, a).1, 1080.0 - 265.0 - 316.0);
        // 非法 pos_frac 回退中位
        assert_eq!(
            anchored_window_pos("top", f64::NAN, a),
            anchored_window_pos("top", 0.5, a)
        );
        // 副屏在负坐标：仍是「相对该屏工作区」贴边
        let secondary = (-2560.0, -82.0, 2560.0, 1440.0);
        assert_eq!(
            anchored_window_pos("top", 0.5, secondary),
            (-1280.0 - 390.0, -82.0 - 70.0)
        );
        assert_eq!(
            anchored_window_pos("left", 0.5, secondary),
            (-2560.0 - 70.0, -82.0 + 720.0 - 316.0)
        );
    }

    #[test]
    fn drag_window_placement_from_each_edge_avoids_jump() {
        // 拖拽期间主体本地矩形固定为起始边的 anchored 模型：
        // 窗口 = 主体中心（逻辑）- 本地中心，四边公式与契约一致。
        let w = 300.0;
        let h = 54.0;
        let (bcx, bcy) = (900.0, 500.0);
        let place = |edge: &str| {
            let (lcx, lcy) = rect_center(body_local_rect(edge, w, h));
            (bcx - lcx, bcy - lcy)
        };
        assert_eq!(place("top"), (510.0, 500.0 - 27.0 - 70.0));
        assert_eq!(place("bottom"), (510.0, 500.0 + 27.0 - (632.0 - 70.0)));
        assert_eq!(place("left"), (900.0 - 150.0 - 70.0, 184.0));
        assert_eq!(place("right"), (900.0 + 150.0 - (780.0 - 70.0), 184.0));
        // 原点相对：拖拽开始那一帧窗口位置与 anchored 时完全一致（不会跳）
        let a = area_primary();
        for edge in ["top", "bottom", "left", "right"] {
            let local = body_local_rect(edge, w, h);
            let (lcx, lcy) = rect_center(local);
            let anchored = anchored_window_pos(edge, 0.5, a);
            let body_center = (anchored.0 + lcx, anchored.1 + lcy);
            assert_eq!(
                (body_center.0 - lcx, body_center.1 - lcy),
                anchored,
                "{edge} 起始拖拽位置不能跳"
            );
        }
    }

    #[test]
    fn gap_and_edge_line_follow_window_position() {
        let a = area_primary();
        // top：edgeLine = 边线y - 窗口y；gap = 主体本地 y - edgeLine
        let local = body_local_rect("top", 300.0, 54.0);
        assert_eq!(edge_line_canvas("top", a, (570.0, -70.0)), 70.0);
        assert_eq!(gap_to_edge("top", local, a, (570.0, -70.0)), 0.0);
        // 往下拖 → gap 变大；冲过边线 → gap 为负
        assert_eq!(gap_to_edge("top", local, a, (570.0, -40.0)), 30.0);
        assert_eq!(gap_to_edge("top", local, a, (570.0, -100.0)), -30.0);
        // bottom
        let local = body_local_rect("bottom", 300.0, 54.0);
        assert_eq!(edge_line_canvas("bottom", a, (570.0, 518.0)), 562.0);
        assert_eq!(gap_to_edge("bottom", local, a, (570.0, 518.0)), 0.0);
        assert_eq!(gap_to_edge("bottom", local, a, (570.0, 470.0)), 48.0);
        // left / right
        let local = body_local_rect("left", 300.0, 54.0);
        assert_eq!(edge_line_canvas("left", a, (-70.0, 224.0)), 70.0);
        assert_eq!(gap_to_edge("left", local, a, (-70.0, 224.0)), 0.0);
        let local = body_local_rect("right", 300.0, 54.0);
        assert_eq!(edge_line_canvas("right", a, (1210.0, 224.0)), 710.0);
        assert_eq!(gap_to_edge("right", local, a, (1210.0, 224.0)), 0.0);
        // 负坐标副屏：edgeLine 仍是画布坐标
        let secondary = (-2560.0, -82.0, 2560.0, 1440.0);
        assert_eq!(edge_line_canvas("top", secondary, (0.0, -152.0)), 70.0);
        let local = body_local_rect("top", 300.0, 54.0);
        assert_eq!(gap_to_edge("top", local, secondary, (0.0, -152.0)), 0.0);
    }

    #[test]
    fn nearest_edge_picks_smallest_signed_gap() {
        let a = area_primary();
        let local = body_local_rect("top", 300.0, 54.0);
        assert_eq!(nearest_edge(a, (570.0, -70.0), local).0, "top");
        let local = body_local_rect("bottom", 300.0, 54.0);
        assert_eq!(nearest_edge(a, (570.0, 518.0), local).0, "bottom");
        let local = body_local_rect("left", 300.0, 54.0);
        assert_eq!(nearest_edge(a, (-70.0, 224.0), local).0, "left");
        let local = body_local_rect("right", 300.0, 54.0);
        assert_eq!(nearest_edge(a, (1210.0, 224.0), local).0, "right");
        // 冲过顶边：gap 为负，仍选 top（液桥/断桥判定用得到负值）
        let local = body_local_rect("top", 300.0, 54.0);
        let (edge, gap) = nearest_edge(a, (570.0, -200.0), local);
        assert_eq!(edge, "top");
        assert!(gap < -100.0);
    }

    #[test]
    fn snap_prefers_horizontal_edges_and_projects_the_release_velocity() {
        let a = area_primary();
        let body = (40.0, 50.0, 300.0, 54.0);
        assert_eq!(nearest_edge(a, (0.0, 0.0), body), ("left", 40.0));
        assert_eq!(nearest_snap_edge(a, body), ("top", 50.0));

        let mut gap = MotionSpring::new(50.0, 300.0, 0.38);
        gap.target = 0.0;
        gap.velocity = -350.0;
        let mut overshot = false;
        for _ in 0..120 {
            gap.step(1.0 / 60.0);
            overshot |= gap.value < 0.0;
        }
        assert!(overshot);
        assert!(gap.at_rest());
        assert_eq!(gap.value, 0.0);
    }

    #[test]
    fn motion_canvas_retains_room_for_nearby_edge_bridges() {
        let area = area_primary();
        assert_eq!(moving_canvas_origin((960.0, 40.0), area), (570.0, -70.0));
        assert_eq!(moving_canvas_origin((30.0, 540.0), area), (-70.0, 224.0));
        assert_eq!(moving_canvas_origin((960.0, 540.0), area), (570.0, 224.0));
        assert!(!drag_threshold_crossed((6.0, 8.0), false));
        assert!(drag_threshold_crossed((7.0, 8.0), false));
        assert!(!drag_threshold_crossed((2.0, -2.0), true));
        assert!(drag_threshold_crossed((3.0, -2.0), true));
    }

    #[test]
    fn fullscreen_hug_moves_only_the_docked_edge() {
        let work = (-2460.0, -80.0, 1900.0, 1020.0);
        let bounds = (-2560.0, -100.0, -500.0, 980.0);
        assert_eq!(extend_work_area(work, bounds, "top", 0.5), (-2460.0, -90.0, 1900.0, 1030.0));
        assert_eq!(extend_work_area(work, bounds, "bottom", 1.0), (-2460.0, -80.0, 1900.0, 1060.0));
        assert_eq!(extend_work_area(work, bounds, "left", 1.0), (-2560.0, -80.0, 2000.0, 1020.0));
        assert_eq!(extend_work_area(work, bounds, "right", 1.0), (-2460.0, -80.0, 1960.0, 1020.0));
        assert_eq!(extend_work_area(work, bounds, "bottom", 0.0), work);
    }

    #[test]
    fn body_rect_and_window_position_roundtrip() {
        let a = area_primary();
        let w = 300.0;
        let h = 54.0;
        for edge in ["top", "bottom", "left", "right"] {
            let win = anchored_window_pos(edge, 0.5, a);
            let local = body_local_rect(edge, w, h);
            let (lcx, lcy) = rect_center(local);
            let body_center = (win.0 + lcx, win.1 + lcy);
            let expected = match edge {
                "bottom" => (960.0, 1080.0 - h / 2.0),
                "left" => (w / 2.0, 540.0),
                "right" => (1920.0 - w / 2.0, 540.0),
                _ => (960.0, h / 2.0),
            };
            assert_eq!(body_center, expected, "{edge}");
        }
    }

    #[test]
    fn body_size_is_clamped_with_fallback() {
        assert_eq!(clamp_body_size(300.0, 54.0), (300.0, 54.0));
        assert_eq!(clamp_body_size(9999.0, 9999.0), (BODY_MAX_W, BODY_MAX_H));
        assert_eq!(clamp_body_size(0.0, 0.0), (BODY_FALLBACK_W, BODY_FALLBACK_H));
        assert_eq!(
            clamp_body_size(f64::NAN, -5.0),
            (BODY_FALLBACK_W, BODY_FALLBACK_H)
        );
        assert_eq!(clamp_body_size(1.0, 1.0), (1.0, 1.0));
        // 默认未上报：300×54（收起态胶囊设计值）
        assert_eq!(
            body_local_rect("top", BODY_FALLBACK_W, BODY_FALLBACK_H),
            (240.0, 70.0, 300.0, 54.0)
        );
    }

    #[test]
    fn mouse_through_uses_body_rect_with_fillet_margin() {
        let s = 1.25;
        // 780×632 逻辑 = 975×790 物理
        let win = (1000, 0, 975, 790);
        let body = Some((240.0, 70.0, 300.0, 54.0));
        // 主体正中（可交互）
        assert!(!should_mouse_through(win, s, body, (1400, 120), false));
        // 左右透明留白（穿透）
        assert!(should_mouse_through(win, s, body, (1100, 120), false));
        // 主体下方透明区域（穿透）
        assert!(should_mouse_through(win, s, body, (1400, 300), false));
        // 液脚 13px 内仍算岛（不穿透）；再往外穿透
        let margin_x = (1000.0 + (240.0 - 13.0) * s) as i32;
        assert!(!should_mouse_through(win, s, body, (margin_x + 1, 120), false));
        assert!(should_mouse_through(win, s, body, (margin_x - 6, 120), false));
        // 未上报主体 / 非法缩放 / 窗口尺寸对不上 → 保持可交互（不穿透）
        assert!(!should_mouse_through(win, s, None, (1100, 120), false));
        assert!(!should_mouse_through(win, f64::NAN, body, (1100, 120), false));
        assert!(!should_mouse_through((1000, 0, 690, 650), s, body, (1100, 120), false));
    }

    /// 收起态右键菜单是浮在胶囊热区之外的浮层：菜单打开期间不穿透（窗口保持
    /// 可交互，菜单才能 hover/点击/展开子菜单）；菜单关闭后立刻回到原矩形判定。
    #[test]
    fn context_menu_holds_mouse_through_off() {
        let s = 1.25;
        let win = (1000, 0, 975, 790);
        let body = Some((240.0, 70.0, 300.0, 54.0));
        // 同样的透明留白坐标：菜单关着要穿透，菜单打开时保持可交互
        assert!(should_mouse_through(win, s, body, (1100, 120), false));
        assert!(!should_mouse_through(win, s, body, (1100, 120), true));
        // 胶囊下方（收起态菜单的常驻位置）同样：关着穿透、开着不穿透
        assert!(should_mouse_through(win, s, body, (1400, 300), false));
        assert!(!should_mouse_through(win, s, body, (1400, 300), true));
        // 菜单打开时即便几何数据无效也保持可交互
        assert!(!should_mouse_through(win, s, None, (1100, 120), true));
        assert!(!should_mouse_through((1000, 0, 690, 650), s, body, (1100, 120), true));
        // 菜单关闭（menu_open=false）后同一坐标恢复原判定：没有永久关掉穿透
        assert!(should_mouse_through(win, s, body, (1100, 120), false));
    }

    #[test]
    fn hover_rect_is_clamped_to_canvas() {
        // 画布 780×632：收起态胶囊 300×54 在 (240,70) 原样保留
        assert_eq!(
            clamp_hover_rect((240.0, 70.0, 300.0, 54.0), "top"),
            Some((240.0, 70.0, 300.0, 54.0))
        );
        // bottom 收起态在画布底部
        assert_eq!(
            clamp_hover_rect((240.0, 508.0, 300.0, 54.0), "bottom"),
            Some((240.0, 508.0, 300.0, 54.0))
        );
        // 越界裁进画布；整个越界返回 None；非法值 None
        assert_eq!(
            clamp_hover_rect((-10.0, 500.0, 800.0, 200.0), "top"),
            Some((0.0, 500.0, 780.0, 132.0))
        );
        assert_eq!(clamp_hover_rect((900.0, 70.0, 100.0, 54.0), "top"), None);
        assert_eq!(clamp_hover_rect((f64::NAN, 70.0, 300.0, 54.0), "top"), None);
    }
}
