//! 灵动岛 HUD（island）：顶部居中、贴近屏幕上缘的液态毛玻璃胶囊。
//!
//! 视觉与交互参考 Misaka273/wisland（PySide6 版 Windows 灵动岛，MIT）：
//! - 收起态是一枚贴着屏幕上缘的胶囊，默认只显示 **Agent 额度 + 天气**，
//!   额度沿用旧版 HUD 的同心圆环（环心品牌图标，每圈一个窗口）；
//! - 悬停约 220ms 后向两侧撑开、向下长成一块深色毛玻璃面板，顶部标签栏
//!   切换 主页 / 消息 / 股票 / 天气 / 控制 / 剪贴板，齿轮进入设置；
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

pub mod backdrop;
pub mod capture_guard;
pub mod clipboard;
pub mod lyrics;
pub mod media;
pub mod meeting;
pub mod notify;
pub mod screenshot;
pub mod stocks;
pub mod store;
pub mod system;
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

/// 灵动岛窗口 label（capabilities/island.json 白名单同步）
pub const ISLAND_WINDOW_LABEL: &str = "island";
/// 配置变更广播（透明度 / 板块显隐 / 城市 / 自选股等）
pub const ISLAND_CONFIG_EVENT: &str = "zbar://island-config";
/// Rust 侧强制收起（全屏隐藏 / 关闭灵动岛）时通知前端收敛本地 open 态
pub const ISLAND_COLLAPSE_EVENT: &str = "zbar://island-collapse";
/// macOS 上由 Rust 轮询光标后广播的展开事件（WebView 收不到 mouseenter）
pub const ISLAND_EXPAND_EVENT: &str = "zbar://island-expand";

/// 窗口宽（逻辑 px）。面板 520 + 左右各 16 的投影留白。
const ISLAND_WINDOW_W: f64 = 552.0;
/// 拖拽时临时用的「条」高：顶部 6 屏缘间隙 + 胶囊 48 + 底部 16 投影留白。
/// 只是拖拽期间的窗口高度（为了能贴到屏幕下缘），松手/开合都用展开态高。
const ISLAND_COLLAPSED_H: f64 = 70.0;
/// 常驻窗口高（内容面板 492 + 顶部间隙 9 + 底部 16 投影留白）。开合不改窗口。
const ISLAND_EXPANDED_H: f64 = 520.0;

static SERVICES_STARTED: AtomicBool = AtomicBool::new(false);
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

/// 前端把胶囊元素的实际矩形上报给 Rust：macOS 用它判悬停展开，Windows 用它
/// 判「光标是不是落在透明留白里」。
#[tauri::command]
pub fn island_set_hover_rect(x: f64, y: f64, width: f64, height: f64) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    if std::env::var_os("ZBAR_ISLAND_DEBUG").is_some() {
        eprintln!("[zbar-island] hover rect {x:.0},{y:.0} {width:.0}x{height:.0}");
    }
    if let Ok(mut slot) = hover_rect_slot().lock() {
        *slot = Some((x, y, width, height));
    }
}

/// 灵动岛窗口的逻辑尺寸（前端上报的热区按此裁剪）。
///
/// **窗口永远是展开态尺寸**（见 `set_expanded_on_main` 的注释）：透明窗口每改
/// 一次尺寸，DWM 都会重建它的合成面，WebView2 重绘前的那 1~2 帧是空的——实机上
/// 展开时表现为「整块岛先横向跳一下、再消失一瞬」，收起后表现为「动画都结束了
/// 竖条才闪一下冒出来」。所以尺寸一次定死，开合只由 CSS 负责。
fn window_size_logical(_edge: &str) -> (f64, f64) {
    (ISLAND_WINDOW_W, ISLAND_EXPANDED_H)
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

/// 拖拽弹簧：参考 wisland `Spring(900, 0.78, 0.2)`。窗口不是硬贴着
/// 光标走，而是欠阻尼地追过去，快速甩动时会有轻微滞后和回弹。
const DRAG_SPRING_K: f64 = 900.0;
const DRAG_SPRING_RATIO: f64 = 0.78;
const DRAG_SUBSTEP: f64 = 1.0 / 240.0;

#[derive(Debug, Clone, Copy)]
struct DragSession {
    /// 光标到窗口左上角的物理像素偏移
    offset_x: i32,
    offset_y: i32,
    /// 弹簧当前窗口位置 / 速度（物理 px）
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    last_x: i32,
    last_y: i32,
    last_tick: Instant,
    /// 是否已越过启动阈值（只按不拖时保持 false，点击语义不受影响）
    started: bool,
    /// 按下时的光标位置，用于判断是否真的在拖
    press_x: i32,
    press_y: i32,
    /// 启动时是否需要先收成横条
    need_bar: bool,
}

/// 单轴欠阻尼弹簧积分；dt 内按 240Hz 子步进，避免大步长在高速时抖。
fn spring_axis(x: &mut f64, v: &mut f64, target: f64, dt: f64) {
    let c = 2.0 * DRAG_SPRING_RATIO * DRAG_SPRING_K.sqrt();
    let steps = (dt / DRAG_SUBSTEP).ceil().max(1.0) as usize;
    let h = dt / steps as f64;
    for _ in 0..steps {
        let a = -DRAG_SPRING_K * (*x - target) - c * *v;
        *v += a * h;
        *x += *v * h;
    }
}

static DRAG: std::sync::OnceLock<std::sync::Mutex<Option<DragSession>>> =
    std::sync::OnceLock::new();
/// 刚结束一次按压/拖拽：悬停守卫先别急着展开，等光标离开胶囊热区再恢复。
/// 否则「点胶囊收起」会被下一拍悬停立刻重新撑开。
static DRAG_RELEASED: AtomicBool = AtomicBool::new(false);

fn drag_slot() -> &'static std::sync::Mutex<Option<DragSession>> {
    DRAG.get_or_init(|| std::sync::Mutex::new(None))
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
        app.get_webview_window(label)
            .and_then(|win| win.is_visible().ok())
            .unwrap_or(false)
    })
}

/// 悬停兜底：macOS 上灵动岛是不可聚焦窗口，WKWebView 的 tracking area 只对
/// key window 生效，pointerenter / pointerleave 一律收不到。这里用 150ms 轮询
/// 光标位置双向补齐：进入胶囊热区展开，离开面板收起。
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
                // 面板 / 托盘菜单可见时暂停悬停判定：它们与岛在屏幕上缘
                // 重叠，光标在浮层上移动不应把岛撑开（见 top_popup_visible）
                if top_popup_visible(&app) {
                    outside = 0;
                    inside = 0;
                    entered_fired = false;
                    continue;
                }
                if drag_slot().lock().map(|g| g.is_some()).unwrap_or(false) {
                    if trace {
                        eprintln!("[zbar-island] hover skip: drag session");
                    }
                    outside = 0;
                    inside = 0;
                    continue;
                }
                // 按住左键（准备拖拽）时不要悬停展开/收起：展开会把按住的
                // 胶囊换掉，拖拽因此中断。
                if left_button_down() {
                    if trace {
                        eprintln!("[zbar-island] hover skip: left button down");
                    }
                    outside = 0;
                    inside = 0;
                    continue;
                }
                let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) else {
                    continue;
                };
                if !win.is_visible().unwrap_or(false) {
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
                        "[zbar-island] trace pos=({},{}) size={}x{} scale={scale} cursor=({cx:.0},{cy:.0}) expanded={expanded}",
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
                            eprintln!("[zbar-island] hover skip: window focused");
                        }
                        outside = 0; // 正在输入，别收
                        continue;
                    }
                    if is_inside {
                        outside = 0;
                        continue;
                    }
                    outside = outside.saturating_add(1);
                    if outside >= 2 {
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
                    if DRAG_RELEASED.load(Ordering::Relaxed) {
                        // 刚点过/拖过：光标还停在胶囊上，先别展开（点胶囊收起
                        // 不该被悬停立刻重新撑开）；等离开一次再恢复悬停。
                        inside = 0;
                        entered_fired = false;
                        continue;
                    }
                    inside = inside.saturating_add(1);
                    // 停 2 拍（约 240ms）再展开，和 Windows 上 220ms 的手感一致
                    if inside >= 2 && !entered_fired {
                        entered_fired = true;
                        cooldown_ms = 400;
                        let _ = app.emit(ISLAND_EXPAND_EVENT, ());
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
            let lx = area.x / area.scale;
            let ly = area.y / area.scale;
            px >= lx && px < lx + area.w / area.scale && py >= ly && py < ly + area.h / area.scale
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
/// 灵动岛窗口按「展开面板 + 投影留白」建窗（顶部吸附 552×70），可见胶囊只占
/// 中间一块，四周是纯透明像素。Win32 的命中测试按窗口矩形算，这圈留白会把
/// 点击吃掉：看着是空的，点下面的应用却没反应。
///
/// 只有「窗口正好是收起态几何 + 有可信的胶囊矩形 + 光标不在胶囊内」三条同时
/// 成立才穿透；窗口尺寸对不上（展开态 / 展开收起动画途中）一律保持整窗可交互
/// ——那 500ms 里热区必然滞后，宁可多挡十几 px 也不能让面板上的点击漏到下面。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn should_mouse_through(
    edge: &str,
    window: (i32, i32, u32, u32),
    scale: f64,
    pill: Option<(f64, f64, f64, f64)>,
    cursor: (i32, i32),
) -> bool {
    if !scale.is_finite() || scale <= 0.0 {
        return false;
    }
    let (wx, wy, ww, wh) = window;
    // 展开态整扇窗（含面板）都要能点：只有收起态才按胶囊热区穿透。
    if LAST_EXPANDED.load(Ordering::Relaxed) {
        return false;
    }
    let (cw, ch) = window_size_logical(edge);
    if (ww as f64 / scale - cw).abs() > 1.5 || (wh as f64 / scale - ch).abs() > 1.5 {
        return false;
    }
    let Some((rx, ry, rw, rh)) = pill.and_then(|rect| clamp_hover_rect(rect, edge)) else {
        return false;
    };
    // 胶囊边缘给几 px 容错：光标贴着边抖动不会反复切样式，贴边点击也仍会
    // 落回胶囊上。留 10px 而不是只留 1~2px：用户经常从胶囊的圆角/描边上
    // 按下去拖拽，判定太紧会出现「明明点到了却穿透给下层应用」。
    const MARGIN: f64 = 10.0;
    let x0 = wx as f64 + (rx - MARGIN) * scale;
    let y0 = wy as f64 + (ry - MARGIN) * scale;
    let x1 = wx as f64 + (rx + rw + MARGIN) * scale;
    let y1 = wy as f64 + (ry + rh + MARGIN) * scale;
    let (cx, cy) = (cursor.0 as f64, cursor.1 as f64);
    !(cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1)
}

/// 按光标位置动态开关 `WS_EX_TRANSPARENT`：光标在胶囊上可交互，落在透明留白
/// 或窗口外时点击穿到下面的应用。
///
/// Tauri 的 set_ignore_cursor_events 在 Windows 上会连带置上 WS_EX_LAYERED
/// （与 Electron setIgnoreMouseEvents 同款），实测渲染不受影响；但穿不穿透
/// 只由这个 ex-style 位决定，所以只能自己轮询光标来算——窗口一旦穿透，
/// WebView 就再也收不到鼠标事件了。
///
/// 轮询里除「解析句柄 / 取缩放因子」外全部直接调 Win32：tauri 的
/// outer_position / is_visible / scale_factor 都是同步派发到事件循环再等回复
/// 的，16ms 一拍地调会把主线程吵死。
#[cfg(target_os = "windows")]
fn start_mouse_through_watcher(app: AppHandle) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, GetWindowRect, IsWindow, IsWindowVisible, GWL_EXSTYLE, WS_EX_TRANSPARENT,
    };

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
                let pill = hover_rect_slot().lock().ok().and_then(|slot| *slot);
                // 拖拽期间保持可交互：拖拽由 Rust 轮询光标推进，但松手和点击
                // 语义还留在窗口上，中间切成穿透会丢掉 pointerup。
                let outside = !dragging
                    && !mouse_down
                    && should_mouse_through(&cfg.edge, now_rect, scale, pill, cursor);

                // 光标移进胶囊：立刻恢复可交互（宁可慢一拍穿透，也不能让用户
                // 点胶囊时把点击漏给下面的应用）。移出去要连续两拍才真正穿透，
                // 快速掠过留白时不至于一闪一闪。
                outside_ticks = if outside {
                    outside_ticks.saturating_add(1)
                } else {
                    0
                };
                let want = outside && outside_ticks >= 2;
                // 外部路径（show/hide、全屏守护、截图恢复）也可能改过窗口的
                // transparent 位。每拍都和真实 ex-style 比一次，不维护本地
                // 缓存——缓存一旦和实际状态分叉，窗口要么一直吃不到点击，
                // 要么一直挡着下面的应用。
                let actual_through =
                    unsafe { GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0 };
                if want == actual_through {
                    continue;
                }
                let applied = app
                    .get_webview_window(ISLAND_WINDOW_LABEL)
                    .map(|win| win.set_ignore_cursor_events(want).is_ok())
                    .unwrap_or(false);
                if applied && debug {
                    eprintln!("[zbar-island] mouse-through {want}");
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandDragStep {
    pub active: bool,
    pub dx: f64,
    pub dy: f64,
    /// 是否已经真正开始拖动（越过阈值）。只有按没有拖时为 false，
    /// 前端据此把这次交互留给 click。
    pub started: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandDragEnd {
    pub edge: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 开始拖拽：记录光标与窗口左上角的偏移。展开态或左右竖排胶囊会先被
/// 收成 552×70 的横条，并以光标为中心放到屏幕上再开始拖——前端在按住
/// 任意非表单区域移动超过阈值后调用，面板会在这一拍收掉。
#[tauri::command]
pub async fn island_drag_begin(app: AppHandle) -> Result<IslandDragBegin, String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        let win = ensure_window(&app_main)?;
        let (cx, cy) = cursor_physical(&app_main).ok_or_else(|| "无法读取光标位置".to_string())?;
        let scale = win.scale_factor().unwrap_or(1.0);
        let cfg = load_config();
        let size = win
            .outer_size()
            .map_err(|e| format!("读取窗口尺寸失败: {e}"))?;
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;
        let collapsed_side = cfg.edge == "left" || cfg.edge == "right";
        // 展开态（任意边）、左右竖胶囊、或几何不是横条时，先统一切成
        // 横条。这样从面板任意位置开始拖，视觉上都是“收成 HUD 后被拖走”。
        let need_bar = LAST_EXPANDED.load(Ordering::Relaxed)
            || collapsed_side
            || (logical_w - ISLAND_WINDOW_W).abs() > 1.5
            || (logical_h - ISLAND_COLLAPSED_H).abs() > 1.5;
        // 这里只记录，不动窗口：按一下没拖动时应该保持点击语义，
        // 真正越过阈值（island_drag_step）才收条并跟手。
        let pos = win
            .outer_position()
            .map_err(|e| format!("读取窗口位置失败: {e}"))?;
        if let Ok(mut slot) = drag_slot().lock() {
            *slot = Some(DragSession {
                offset_x: cx - pos.x,
                offset_y: cy - pos.y,
                x: pos.x as f64,
                y: pos.y as f64,
                vx: 0.0,
                vy: 0.0,
                last_x: pos.x,
                last_y: pos.y,
                last_tick: Instant::now(),
                started: false,
                press_x: cx,
                press_y: cy,
                need_bar,
            });
        }
        Ok(IslandDragBegin { active: true })
    })
    .await
}

/// 拖拽步进：前端以约 60fps 调用。位置由 Rust 读光标计算，WebView 的
/// pointer 事件离开窗口也不会中断；左键松开时返回 active=false，由前端
/// 再调用 island_drag_end 完成吸附。
#[tauri::command]
pub async fn island_drag_step(app: AppHandle) -> Result<IslandDragStep, String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        let Some(mut session) = drag_slot().lock().ok().and_then(|g| *g) else {
            return Ok(IslandDragStep {
                active: false,
                dx: 0.0,
                dy: 0.0,
                started: false,
            });
        };
        if !left_button_down() {
            // 松手就清会话：只按不拖的点击也要清，否则会话永久残留，
            // 悬停守卫会一直以为还在拖，岛再也收不起来。
            if let Ok(mut slot) = drag_slot().lock() {
                *slot = None;
            }
            DRAG_RELEASED.store(true, Ordering::Relaxed);
            return Ok(IslandDragStep {
                active: false,
                dx: 0.0,
                dy: 0.0,
                started: false,
            });
        }
        let Some((cx, cy)) = cursor_physical(&app_main) else {
            return Ok(IslandDragStep {
                active: false,
                dx: 0.0,
                dy: 0.0,
                started: false,
            });
        };
        // 只按下没拖动 → 保持点击语义，窗口一动不动。
        if !session.started {
            let moved = ((cx - session.press_x).pow(2) + (cy - session.press_y).pow(2)) as f64;
            if moved < 25.0 {
                if let Ok(mut slot) = drag_slot().lock() {
                    *slot = Some(session);
                }
                return Ok(IslandDragStep {
                    active: true,
                    dx: 0.0,
                    dy: 0.0,
                    started: false,
                });
            }
            // 真正开始拖：此刻才把面板收成横条并挪到光标下。
            if session.need_bar {
                let win = ensure_window(&app_main)?;
                let area = monitor_area_at(&app_main, (cx as f64, cy as f64));
                let (ax, ay, aw, ah) = (
                    area.x / area.scale,
                    area.y / area.scale,
                    area.w / area.scale,
                    area.h / area.scale,
                );
                let lx = cx as f64 / area.scale;
                let ly = cy as f64 / area.scale;
                // 窗口尺寸保持常驻展开态，**拖拽期间也不再改**：改尺寸会让 DWM
                // 重建合成面，看起来就是「一开始拖，岛闪一下」。
                // 条画在窗口顶部，所以窗口顶边 = 条顶边；把「条」的落点 clamp 在
                // 工作区内即可，窗口多出来的透明部分伸出屏幕边缘也无所谓。
                let x = (lx - ISLAND_WINDOW_W / 2.0).clamp(ax, (ax + aw - ISLAND_WINDOW_W).max(ax));
                let y = (ly - ISLAND_COLLAPSED_H / 2.0)
                    .clamp(ay, (ay + ah - ISLAND_COLLAPSED_H).max(ay));
                // Windows：窗口常驻展开态尺寸，拖拽期间也不改（改尺寸会闪一下）；
                // macOS 的透明窗口不经过 DWM 这条合成路径，仍缩成「条」。
                #[cfg(target_os = "windows")]
                let drag_window_h = ISLAND_EXPANDED_H;
                #[cfg(not(target_os = "windows"))]
                let drag_window_h = ISLAND_COLLAPSED_H;
                apply_window_frame_scaled(&win, x, y, ISLAND_WINDOW_W, drag_window_h, area.scale)?;
                LAST_EXPANDED.store(false, Ordering::Relaxed);
                session.need_bar = false;
                // 收条后重算抓取偏移，保证跟手不跳
                session.offset_x = cx - (x * area.scale).round() as i32;
                session.offset_y = cy - (y * area.scale).round() as i32;
                session.x = x * area.scale;
                session.y = y * area.scale;
                session.last_x = session.x.round() as i32;
                session.last_y = session.y.round() as i32;
            }
            session.started = true;
            session.last_tick = Instant::now();
        }
        let win = ensure_window(&app_main)?;
        // 多屏：夹到所有显示器工作区的并集，而不是主屏。这样窗口可以
        // 从左屏跨到负坐标的副屏（或右侧副屏），不会被 primary 边界卡住。
        let bounds = virtual_work_bounds(&app_main);
        let size = win
            .outer_size()
            .map_err(|e| format!("读取窗口尺寸失败: {e}"))?;
        // Windows 上窗口常驻 552×520，但可见的只有顶部 70 高的横条；拖拽
        // 范围必须按「条」算，不能按整扇透明窗算——否则用户把岛往屏幕下方
        // 拖，窗口底边一碰到工作区下沿（1080 屏约 y=560）就再也拖不动了，
        // 看起来就是「拖到一半卡住 / 根本拖不走」。窗口多出来的透明部分
        // 允许伸出屏幕边缘。
        let bar_h =
            (ISLAND_COLLAPSED_H * win.scale_factor().unwrap_or(1.0)).min(size.height as f64);
        let (target_x, target_y) = clamp_drag_target(
            cx as f64,
            cy as f64,
            session.offset_x,
            session.offset_y,
            (size.width as f64, bar_h),
            bounds,
        );
        let max_x = bounds.2 - size.width as f64;
        let max_y = bounds.3 - bar_h;
        let lo_x = bounds.0.min(max_x);
        let hi_x = max_x;
        let lo_y = bounds.1.min(max_y);
        let hi_y = max_y;

        let now = Instant::now();
        let dt = (now - session.last_tick)
            .as_secs_f64()
            .clamp(1.0 / 240.0, 1.0 / 30.0);
        session.last_tick = now;
        spring_axis(&mut session.x, &mut session.vx, target_x, dt);
        spring_axis(&mut session.y, &mut session.vy, target_y, dt);
        // 弹簧可以越过目标，但不能把窗口甩出工作区。
        if session.x < lo_x {
            session.x = lo_x;
            session.vx = 0.0;
        } else if session.x > hi_x {
            session.x = hi_x;
            session.vx = 0.0;
        }
        if session.y < lo_y {
            session.y = lo_y;
            session.vy = 0.0;
        } else if session.y > hi_y {
            session.y = hi_y;
            session.vy = 0.0;
        }

        let px = session.x.round() as i32;
        let py = session.y.round() as i32;
        if px != session.last_x || py != session.last_y {
            let _ = win.set_position(PhysicalPosition::new(px, py));
        }
        let dx = session.x - session.last_x as f64;
        let dy = session.y - session.last_y as f64;
        session.last_x = px;
        session.last_y = py;
        if let Ok(mut slot) = drag_slot().lock() {
            *slot = Some(session);
        }
        Ok(IslandDragStep {
            active: true,
            dx,
            dy,
            started: true,
        })
    })
    .await
}

/// 松手吸附：按窗口矩形到三条边的距离选最近边，落盘 edge/pos_frac，
/// 立即切到该边的窗口尺寸与位置。前端负责在调用前把胶囊淡出，避免
/// WebView 在窗口改几何的那一帧露出没有玻璃底的横条。
#[tauri::command]
pub async fn island_drag_end(
    app: AppHandle,
    started: Option<bool>,
) -> Result<IslandDragEnd, String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        // 只按不拖（纯点击）时 Rust 侧会话已经建立但 started 仍为 false：
        // 这里清掉会话即可，绝不能走吸附/落盘——否则点一下胶囊岛就会被
        // 弹到最近的屏幕边。前端在 pointerup 早于 begin 回执时也会调用
        // 本命令收尾，语义必须保持幂等。
        let session_started = drag_slot()
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .map(|session| session.started)
            .unwrap_or(false);
        // 前端在真正拖动过（收到过 started=true）后会显式传 started=true。
        // 不能只信 drag_slot：island_drag_step 在检测到左键松开时会先清掉
        // 会话，等 130ms 淡出后再调用本命令时槽里已经空了。只按会话判断会
        // 得到 false，于是拖到左边/顶部也不会吸附，配置一直停在旧边。
        let started = started.unwrap_or(session_started);
        DRAG_RELEASED.store(true, Ordering::Relaxed);
        let win = ensure_window(&app_main)?;
        let pos = win
            .outer_position()
            .map_err(|e| format!("读取窗口位置失败: {e}"))?;
        let size = win
            .outer_size()
            .map_err(|e| format!("读取窗口尺寸失败: {e}"))?;
        let drag_scale = win.scale_factor().unwrap_or(1.0);
        let bar_h = ISLAND_COLLAPSED_H * drag_scale;
        if !started {
            return Ok(IslandDragEnd {
                edge: load_config().edge,
                x: pos.x as f64 / drag_scale,
                y: pos.y as f64 / drag_scale,
                width: size.width as f64 / drag_scale,
                height: size.height as f64 / drag_scale,
            });
        }
        // 以窗口中心所在的那块屏做吸附判定：拖到副屏后，距离和比例都
        // 相对副屏工作区计算，不能再用主屏的边。
        // 注意窗口是常驻的 552×520，**条只占窗口顶部 70 高**：吸附距离和比例
        // 都必须按条算，否则条会被算在窗口垂直中心上，贴左右边时整体下坠 260px。
        let center = (
            pos.x as f64 + size.width as f64 / 2.0,
            pos.y as f64 + bar_h / 2.0,
        );
        let area = monitor_area_at(&app_main, center);
        let (ax, ay, aw, ah) = (area.x, area.y, area.w, area.h);
        let x = pos.x as f64;
        let y = pos.y as f64;
        let w = size.width as f64;
        let h = bar_h;
        let (edge, pos_frac) = snap_to_edge(x, y, w, h, (ax, ay, aw, ah));
        let cfg = {
            let mut c = load_config();
            c.edge = edge.to_string();
            c.pos_frac = pos_frac;
            c.monitor = area.name.clone();
            c
        };
        save_config(&cfg)?;
        // 落点按「条」算；窗口尺寸由 island_geometry 决定（Windows 上恒为展开态，
        // 不再改窗口尺寸，否则又要闪一下）。
        let (nx, ny, nw, nh) = island_geometry(&app_main, edge, false, cfg.pos_frac);
        let scale = area.scale;
        apply_window_frame_scaled(&win, nx, ny, nw, nh, scale)?;
        LAST_EXPANDED.store(false, Ordering::Relaxed);
        emit_config(&app_main, &cfg);
        Ok(IslandDragEnd {
            edge: edge.to_string(),
            x: nx,
            y: ny,
            width: nw,
            height: nh,
        })
    })
    .await
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
    40
}
fn default_font_size() -> f64 {
    13.0
}
fn default_expand_mode() -> String {
    "hover".into()
}
fn default_city() -> String {
    "上海".into()
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
    /// hover / never（常驻展开已移除）
    pub expand_mode: String,
    /// 首次启用时自动关掉旧的贴边 HUD 停靠条，避免两套 HUD 叠在一起
    pub replace_legacy_bar: bool,
    pub weather_city: String,
    /// 三级联动选中的展示名（如“杭州 · 西湖区”）；空串时显示 weather_city。
    pub weather_label: String,
    pub weather_lat: Option<f64>,
    pub weather_lon: Option<f64>,
    /// 吸附边：top / left / right
    pub edge: String,
    /// 吸附边上的位置比例 0..1（顶部按横坐标，左右按纵坐标）
    pub pos_frac: f64,
    /// 上次所在显示器（Tauri Monitor::name，如 `\\.\DISPLAY2`）。None/找不到
    /// 时回退主屏；多屏拖拽后展开、重启都靠它留在同一块屏。
    pub monitor: Option<String>,
    pub stocks: Vec<String>,
    pub stock_interval_sec: u64,
    pub stock_alert_pct: f64,
    pub hotkey_toggle: String,
    pub hide_fullscreen: bool,
    pub hidden_tabs: Vec<String>,
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
            replace_legacy_bar: true,
            weather_city: default_city(),
            weather_label: String::new(),
            weather_lat: None,
            weather_lon: None,
            edge: default_edge(),
            pos_frac: default_pos_frac(),
            monitor: None,
            stocks: default_stocks(),
            stock_interval_sec: default_stock_interval(),
            stock_alert_pct: default_stock_alert(),
            hotkey_toggle: default_hotkey(),
            hide_fullscreen: true,
            hidden_tabs: Vec::new(),
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
        if c.expand_mode != "hover" && c.expand_mode != "never" {
            c.expand_mode = default_expand_mode();
        }
        if c.weather_city.trim().is_empty() {
            c.weather_city = default_city();
        }
        c.weather_city = c.weather_city.trim().to_string();
        c.weather_label = c.weather_label.trim().to_string();
        if c.edge != "top" && c.edge != "left" && c.edge != "right" {
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
        c.hidden_tabs.retain(|k| {
            matches!(
                k.as_str(),
                "msg" | "stock" | "weather" | "ctl" | "clip" | "shot"
            )
        });
        c.hidden_tabs.sort();
        c.hidden_tabs.dedup();
        c.launcher
            .retain(|i| !i.name.trim().is_empty() && !i.path.trim().is_empty());
        if c.launcher.is_empty() {
            c.launcher = default_launcher();
        }
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
            eprintln!("[zbar-island] 关闭旧 HUD 停靠条失败: {e}");
        }
    }
}

// ============================================================
// 窗口
// ============================================================

fn work_area_logical(app: &AppHandle) -> (f64, f64, f64, f64) {
    let cfg = load_config();
    let area = monitor_area_for(app, cfg.monitor.as_deref());
    (
        area.x / area.scale,
        area.y / area.scale,
        area.w / area.scale,
        area.h / area.scale,
    )
}

/// 配置所指那块屏的缩放比（逻辑 px → 物理 px 用）。
///
/// 不能用 `win.scale_factor()`：多屏不同缩放时，配置指向副屏而窗口还挂在主屏
/// 上的那一次会算错；换算必须和 `work_area_logical` 用同一块屏的比例。
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
    }
}

/// 全部显示器的工作区（物理 px）。多屏拖拽必须在这里做并集/命中判定，
/// 不能只看 primary_monitor，否则副屏会被夹回主屏。
fn monitor_areas(app: &AppHandle) -> Vec<MonitorArea> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(area_of_monitor)
        .collect()
}

/// 指针（物理 px）落在第几块屏：先按包含关系找，找不到（屏间空隙/任务栏）
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
    let rects: Vec<(f64, f64, f64, f64)> = areas.iter().map(|a| (a.x, a.y, a.w, a.h)).collect();
    monitor_index_at(&rects, point)
        .and_then(|i| areas.get(i).cloned())
        .or_else(|| areas.first().cloned())
        .unwrap_or(MonitorArea {
            name: None,
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
            scale: 1.0,
        })
}

/// 配置里保存的显示器（按设备名匹配），找不到时回退主屏/第一块屏。
fn monitor_area_for(app: &AppHandle, name: Option<&str>) -> MonitorArea {
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
    .unwrap_or(MonitorArea {
        name: None,
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
        scale: 1.0,
    })
}

/// 所有显示器工作区的并集（物理 px）。拖拽时只夹到这个虚拟桌面边界，
/// 允许指针跨过屏间分界；具体吸附到哪块屏由 island_drag_end 判定。
fn virtual_work_bounds(app: &AppHandle) -> (f64, f64, f64, f64) {
    let areas = monitor_areas(app);
    if areas.is_empty() {
        return (0.0, 0.0, 1920.0, 1080.0);
    }
    let min_x = areas.iter().map(|a| a.x).fold(f64::INFINITY, f64::min);
    let min_y = areas.iter().map(|a| a.y).fold(f64::INFINITY, f64::min);
    let max_x = areas
        .iter()
        .map(|a| a.x + a.w)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = areas
        .iter()
        .map(|a| a.y + a.h)
        .fold(f64::NEG_INFINITY, f64::max);
    (min_x, min_y, max_x, max_y)
}

/// 拖拽目标点：窗口左上角 = 光标 - 抓取偏移，再夹进虚拟桌面并集。
/// 纯函数，单测覆盖副屏在负坐标的情况。
fn clamp_drag_target(
    cx: f64,
    cy: f64,
    offset_x: i32,
    offset_y: i32,
    size: (f64, f64),
    bounds: (f64, f64, f64, f64),
) -> (f64, f64) {
    let (vx0, vy0, vx1, vy1) = bounds;
    let max_x = vx1 - size.0;
    let max_y = vy1 - size.1;
    let lo_x = vx0.min(max_x);
    let lo_y = vy0.min(max_y);
    (
        (cx - offset_x as f64).clamp(lo_x, max_x),
        (cy - offset_y as f64).clamp(lo_y, max_y),
    )
}

/// 松手后按「条」到三条边的距离选最近边。左右距离乘 0.82：同样距离下
/// 优先贴左右，符合“拖到两边变竖”的主诉求；顶边仍保留，用于回到顶部横排。
/// 纯函数，单测覆盖左/右/顶三种落点。
fn pick_snap_edge(d_top: f64, d_left: f64, d_right: f64) -> &'static str {
    let score_left = d_left * 0.82;
    let score_right = d_right * 0.82;
    if d_top <= score_left && d_top <= score_right {
        "top"
    } else if score_left <= score_right {
        "left"
    } else {
        "right"
    }
}

/// 松手后的吸附结果：边 + 该边上的位置比例（top 用横轴，左右用纵轴）。
/// 入参都是物理 px，区域是该窗口中心所在显示器的工作区；条高只取顶部
/// 可见的 70，不按整扇 520 高窗口算。
fn snap_to_edge(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    (ax, ay, aw, ah): (f64, f64, f64, f64),
) -> (&'static str, f64) {
    let d_top = (y - ay).max(0.0);
    let d_left = (x - ax).max(0.0);
    let d_right = (ax + aw - (x + w)).max(0.0);
    let edge = pick_snap_edge(d_top, d_left, d_right);
    let frac = match edge {
        "top" => ((x + w / 2.0 - ax) / aw.max(1.0)).clamp(0.0, 1.0),
        _ => ((y + h / 2.0 - ay) / ah.max(1.0)).clamp(0.0, 1.0),
    };
    (edge, frac)
}

fn island_position(app: &AppHandle) -> (f64, f64) {
    let cfg = load_config();
    let (x, y, _, _) = island_geometry(app, &cfg.edge, false, cfg.pos_frac);
    (x, y)
}

/// 纯计算版窗口矩形（**逻辑 px**），方便单测。
///
/// 尺寸是「设计稿的工作面」而不是「物理像素」：逻辑 px 经 `scale_factor()`
/// 换算成物理像素后才交给 `SetWindowPos`——100% 屏是 552×520，125% 是 690×650，
/// 150% 是 828×780，换分辨率/缩放都不用改代码。
///
/// 552 = 面板 520 + 左右各 16 的投影留白；520 = 上 6 + 面板 492 + 下 16 + 余量。
/// 这两个数只跟 CSS 设计稿（`.island.open` 520×492）绑定，改面板尺寸时要一起改。
/// 工作面比设计稿还小时（高 DPI 的小笔记本）取小值，宁可窗口窄一点也不能让
/// 面板跑到屏幕外。
fn window_rect_in_area(
    edge: &str,
    expanded: bool,
    frac: f64,
    (ax, ay, aw, ah): (f64, f64, f64, f64),
) -> (f64, f64, f64, f64) {
    #[cfg(target_os = "windows")]
    let (base_w, base_h) = {
        let _ = expanded;
        (ISLAND_WINDOW_W, ISLAND_EXPANDED_H)
    };
    #[cfg(not(target_os = "windows"))]
    let (base_w, base_h) = if expanded {
        (ISLAND_WINDOW_W, ISLAND_EXPANDED_H)
    } else {
        // 48px 竖胶囊 + 左右各 11px 留白 / 胶囊 48 + 上 6 下 16
        match edge {
            "left" | "right" => (70.0, 520.0),
            _ => (ISLAND_WINDOW_W, ISLAND_COLLAPSED_H),
        }
    };
    let w = base_w.min(aw.max(1.0));
    let h = base_h.min(ah.max(1.0));
    match edge {
        "left" | "right" => {
            let x = if edge == "left" { ax } else { ax + aw - w };
            let cy = ay + frac * ah;
            let y = (cy - h / 2.0).clamp(ay, (ay + ah - h).max(ay));
            (x, y, w, h)
        }
        _ => {
            let cx = ax + frac * aw;
            let x = (cx - w / 2.0).clamp(ax, (ax + aw - w).max(ax));
            (x, ay, w, h)
        }
    }
}

/// 灵动岛窗口几何（**逻辑 px**）。三种吸附边：
/// - top：552×520，沿工作区横轴按 pos_frac 定位；
/// - left/right：552×520（面板向内展开），贴住对应屏幕边，
///   沿工作区纵轴按 pos_frac 定位。
///
/// **Windows 上窗口恒为展开态尺寸**：透明窗口每改一次尺寸，DWM 都会重建合成面
/// 并空出 1~2 帧（见 `set_expanded_on_main` 的注释）；`expanded` 于是在 Windows
/// 上只影响窗口内岛元素的布局（CSS 的事）。macOS 的透明窗口不经过这条合成路径，
/// 保持原来的「收起态窗口更小」，避免常驻大窗口挡住点击。
fn island_geometry(
    app: &AppHandle,
    edge: &str,
    expanded: bool,
    pos_frac: f64,
) -> (f64, f64, f64, f64) {
    let area = work_area_logical(app);
    let frac = if pos_frac.is_finite() {
        pos_frac.clamp(0.0, 1.0)
    } else {
        0.5
    };
    window_rect_in_area(edge, expanded, frac, area)
}

/// 确保窗口存在（隐藏态预热）。必须在主线程事件循环上下文调用。
///
/// 注意：这里**不再挂 Windows Acrylic 系统背板**。Windows 11 浅色模式下
/// Acrylic 会把整个窗口（包括胶囊四周的透明留白）铺成一块浅灰矩形，
/// WebView 再叠加半透明 CSS 玻璃后就出现截图里的白边。现在窗口保持
/// 全透明，玻璃只由 CSS 画在胶囊/面板形状内；代价是桌面壁纸不会被
/// 真实模糊，但不会再出现矩形底色。
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
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .min_inner_size(1.0, 1.0)
    .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
    .focusable(false)
    .focused(false);

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
    let cfg = load_config();
    // **窗口尺寸永远按展开态**，开合只切 CSS。
    //
    // 透明窗口一改尺寸，DWM 会丢掉旧的合成面重建，WebView2 重绘之前的那 1~2
    // 帧窗口是空的：展开时看着像「整块岛先横向跳一下 → 消失 → 动画才开始」，
    // 收起时看着像「收缩动画演完了，竖条才闪一下冒出来」。以前收起 70×520、
    // 展开 552×520，每次开合都要改窗口尺寸，正好踩中这个。
    //
    // 现在窗口固定 552×520，收起态只是把胶囊画在边缘、其余透明——透明区域
    // 的点击穿透由 start_mouse_through_watcher 按胶囊热区处理。
    let (x, y, w, h) = island_geometry(app, &cfg.edge, expanded, cfg.pos_frac);
    apply_window_frame_scaled(&win, x, y, w, h, island_scale(app))?;
    LAST_EXPANDED.store(expanded, Ordering::Relaxed);
    Ok(())
}

/// 建窗/重新显示时保持不可聚焦，避免 WebView2 建控件的副作用抢走用户
/// 正在输入的编辑器焦点。只有用户点进设置输入框时，前端才调用
/// island_set_keyboard(true) 临时激活并聚焦窗口；点其它位置/收起后恢复。
fn arm_window_after_show(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) {
        let _ = win.set_focusable(false);
    }
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
    // 透明留白穿透：不启动的话胶囊四周那圈透明像素会吃掉下面的点击
    #[cfg(target_os = "windows")]
    start_mouse_through_watcher(app.clone());
    // 额度复用 hud_bar 数据线程；start_feed 幂等，首次会立即推送一拍。
    crate::hud_bar::start_feed(app.clone());
    crate::hud_bar::hud_bar_refresh(app.clone());
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
    // 旧停靠条若仍开启，额度线程需要继续供它使用；否则一起停掉。
    if !crate::hud_bar::load_hud_bar_config().enabled {
        crate::hud_bar::stop_feed();
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
            let (x, y, w, h) = island_geometry(app, &cfg.edge, false, cfg.pos_frac);
            let _ = apply_window_frame_scaled(&win, x, y, w, h, island_scale(app));
            let _ = win.show();
        }
        arm_window_after_show(app);
        start_services(app);
        // 窗口可能被全屏守护隐藏过，重新显示时清掉标记
        FULLSCREEN_HIDDEN.store(false, Ordering::Relaxed);
    } else {
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

/// 注册灵动岛全局快捷键（默认 Ctrl+Alt+I 开关岛）。
///
/// 与主面板快捷键共用 tauri-plugin-global-shortcut：lib.rs 的
/// apply_shortcut 每次都会先 unregister_all，再依次注册面板快捷键与这里，
/// 因此两边不会互相覆盖。注册失败（被占用/格式非法）只记日志，不阻断
/// 应用启动；设置页改热键后由 lib.rs 主动重新应用。
pub fn register_hotkeys(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let cfg = load_config();
    if cfg.hotkey_toggle.trim().is_empty() {
        return;
    }
    let gs = app.global_shortcut();
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

/// 应用启动挂点：开关开启时恢复窗口与所有数据线程。
pub fn start_if_enabled(app: &AppHandle) {
    let cfg = load_config();
    if !cfg.enabled {
        return;
    }
    if let Err(e) = ensure_window(app) {
        eprintln!("[zbar-island] 恢复灵动岛失败: {e}");
        return;
    }
    arm_window_after_show(app);
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

fn foreground_fullscreen() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Shell::{
            SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN,
        };
        if let Ok(state) = unsafe { SHQueryUserNotificationState() } {
            return state == QUNS_RUNNING_D3D_FULL_SCREEN;
        }
    }
    false
}

fn start_fullscreen_watcher(app: AppHandle) {
    if FULLSCREEN_WATCHER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::Builder::new()
        .name("zbar-island-fullscreen".into())
        .spawn(move || loop {
            thread::sleep(Duration::from_millis(1200));
            let cfg = load_config();
            if !cfg.enabled {
                continue;
            }
            if !cfg.hide_fullscreen {
                if FULLSCREEN_HIDDEN.swap(false, Ordering::Relaxed) {
                    if let Some(win) = app.get_webview_window(ISLAND_WINDOW_LABEL) {
                        let _ = win.show();
                    }
                }
                continue;
            }
            let full = foreground_fullscreen();
            let app_main = app.clone();
            if full && !FULLSCREEN_HIDDEN.load(Ordering::Relaxed) {
                FULLSCREEN_HIDDEN.store(true, Ordering::Relaxed);
                let _ = app.run_on_main_thread(move || {
                    if let Some(win) = app_main.get_webview_window(ISLAND_WINDOW_LABEL) {
                        let cfg = load_config();
                        let (x, y, w, h) =
                            island_geometry(&app_main, &cfg.edge, false, cfg.pos_frac);
                        let _ =
                            apply_window_frame_scaled(&win, x, y, w, h, island_scale(&app_main));
                        let _ = win.hide();
                    }
                    LAST_EXPANDED.store(false, Ordering::Relaxed);
                    let _ = app_main.emit(ISLAND_COLLAPSE_EVENT, ());
                });
            } else if !full && FULLSCREEN_HIDDEN.load(Ordering::Relaxed) {
                FULLSCREEN_HIDDEN.store(false, Ordering::Relaxed);
                let _ = app.run_on_main_thread(move || {
                    if let Some(win) = app_main.get_webview_window(ISLAND_WINDOW_LABEL) {
                        let _ = win.show();
                    }
                });
            }
        })
        .ok();
}

// ============================================================
// 命令
// ============================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandState {
    pub enabled: bool,
    pub config: IslandConfig,
    pub expanded: bool,
}

#[tauri::command]
pub fn island_state() -> IslandState {
    let cfg = load_config();
    IslandState {
        enabled: cfg.enabled,
        config: cfg,
        expanded: LAST_EXPANDED.load(Ordering::Relaxed),
    }
}

#[tauri::command]
pub async fn island_set_config(
    config: IslandConfig,
    app: AppHandle,
) -> Result<IslandConfig, String> {
    let next = config.clamped();
    let prev = load_config();
    save_config(&next)?;

    let weather_changed = next.weather_city != prev.weather_city
        || next.weather_lat != prev.weather_lat
        || next.weather_lon != prev.weather_lon;
    let stocks_changed = next.stocks != prev.stocks
        || next.stock_interval_sec != prev.stock_interval_sec
        || next.stock_alert_pct != prev.stock_alert_pct;
    let hotkey_changed = next.hotkey_toggle != prev.hotkey_toggle;
    let enabled = next.enabled;
    let app_main = app.clone();
    if enabled != prev.enabled {
        on_main_thread(&app, move || set_enabled_on_main(&app_main, enabled)).await?;
    }
    emit_config(&app, &next);
    if weather_changed {
        weather::refresh();
    }
    if stocks_changed {
        stocks::refresh_now();
    }
    if hotkey_changed {
        let app_hotkey = app.clone();
        let _ = app.run_on_main_thread(move || crate::refresh_global_shortcuts(&app_hotkey));
    }
    Ok(next)
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

/// 抓取灵动岛窗口身后的桌面缩略图（CSS 深色毛玻璃的底层）。
///
/// 前端在展开时约 7fps、收起时约 1fps 轮询；抓屏是阻塞的 GDI 操作，
/// 放到 blocking 线程池里，避免卡住 Tauri 的异步运行时和 UI。
#[tauri::command]
pub async fn island_backdrop(app: AppHandle) -> Result<String, String> {
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
            backdrop::capture(hwnd, pos.x, pos.y, size.width, size.height)
        })
        .await
        .map_err(|e| format!("背景抓取任务失败: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
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
    fn window_rect_is_logical_and_fits_small_work_area() {
        // 工作区按逻辑 px 传入（3440×1440 @125% → 2752×1152）。
        let big = (0.0, 0.0, 2752.0, 1152.0);
        // Windows：窗口常驻「设计稿」尺寸 552×520，跟缩放/分辨率无关——
        // 物理尺寸由 apply_window_frame_scaled 乘那块屏的 scale 得到。
        #[cfg(target_os = "windows")]
        {
            let r = window_rect_in_area("right", true, 0.5, big);
            assert_eq!((r.2, r.3), (552.0, 520.0));
            assert_eq!(r.0, 2752.0 - 552.0);
            assert_eq!(r.1, 1152.0 / 2.0 - 260.0);
            // 收起态几何不变：窗口不跟着开合改尺寸。
            assert_eq!(r, window_rect_in_area("right", false, 0.5, big));
            // 左吸附贴左边。
            assert_eq!(window_rect_in_area("left", true, 0.5, big).0, 0.0);
            // 工作面比设计稿还小（1366×768 @150% → 911×512）：取小，别出屏。
            let small = window_rect_in_area("top", true, 0.5, (0.0, 0.0, 911.0, 512.0));
            assert_eq!((small.2, small.3), (552.0, 512.0));
            assert_eq!(small.1, 0.0);
        }
        // 非法 pos_frac 由调用方归一到 0.5；这里只验证吸附轴仍贴边。
        let r = window_rect_in_area("right", true, 0.0, big);
        assert_eq!(r.0, 2752.0 - r.2);
    }

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
    fn drag_target_can_cross_to_negative_secondary() {
        // 主屏 0..2752，副屏在左侧 -2560..0；窗口 552×70。
        let bounds = (-2560.0, -82.0, 2752.0, 1310.0);
        assert_eq!(
            clamp_drag_target(-1000.0, 400.0, 276, 35, (552.0, 70.0), bounds),
            (-1276.0, 365.0)
        );
        // 拖到副屏最左/主屏最右时仍夹在虚拟桌面内。
        assert_eq!(
            clamp_drag_target(-4000.0, 400.0, 276, 35, (552.0, 70.0), bounds).0,
            -2560.0
        );
        assert_eq!(
            clamp_drag_target(4000.0, 400.0, 276, 35, (552.0, 70.0), bounds).0,
            2200.0
        );
    }

    #[test]
    fn snap_edge_prefers_the_nearest_side() {
        // 左边缘中部：到左边 0，顶边 400，右边 2200 → left。
        assert_eq!(pick_snap_edge(400.0, 0.0, 2200.0), "left");
        // 顶边中部：到顶 0，左右都 1400 → top。
        assert_eq!(pick_snap_edge(0.0, 1400.0, 1400.0), "top");
        // 右边缘中部 → right。
        assert_eq!(pick_snap_edge(400.0, 2200.0, 0.0), "right");
        // 左上角同时贴两边时选 top（横排更自然）。
        assert_eq!(pick_snap_edge(0.0, 0.0, 2200.0), "top");
        // 离左边稍远但仍在左侧：左右权重不能把 left 抢成 top。
        assert_eq!(pick_snap_edge(100.0, 0.0, 2200.0), "left");
    }

    #[test]
    fn snap_to_edge_handles_left_top_right_and_negative_secondary() {
        let primary = (0.0, 0.0, 3440.0, 1440.0);
        // 左边中部：窗口 690×87.5 的左上角贴在 (0, 600)。
        let (edge, frac) = snap_to_edge(0.0, 600.0, 690.0, 87.5, primary);
        assert_eq!(edge, "left");
        assert!((frac - (600.0 + 43.75) / 1440.0).abs() < 1e-9);
        // 顶部中间：横轴比例按窗口中心算。
        let (edge, frac) = snap_to_edge(1375.0, 0.0, 690.0, 87.5, primary);
        assert_eq!(edge, "top");
        assert!((frac - 1720.0 / 3440.0).abs() < 1e-9);
        // 右边中部。
        let (edge, _) = snap_to_edge(2750.0, 600.0, 690.0, 87.5, primary);
        assert_eq!(edge, "right");
        // 副屏在负坐标时，左边仍然是副屏自己的工作区。
        let secondary = (-3200.0, 0.0, 3200.0, 1800.0);
        let (edge, frac) = snap_to_edge(-3200.0, 900.0, 690.0, 87.5, secondary);
        assert_eq!(edge, "left");
        assert!((frac - (900.0 + 43.75) / 1800.0).abs() < 1e-9);
    }

    #[test]
    fn drag_target_uses_bar_height_not_tall_window() {
        // Windows 常驻窗口 552×520，但可见横条只有 70 高：拖拽 clamp 必须
        // 用条的尺寸，窗口多出来的透明部分允许伸出屏幕下沿。否则 1080 屏
        // 上窗口 top 最高只能到 560，岛拖不到下半屏。
        let bounds = (0.0, 0.0, 1920.0, 1080.0);
        let (_, y_bar) = clamp_drag_target(960.0, 900.0, 276, 35, (552.0, 70.0), bounds);
        assert_eq!(y_bar, 865.0);
        let (_, y_window) = clamp_drag_target(960.0, 900.0, 276, 35, (552.0, 520.0), bounds);
        assert_eq!(y_window, 560.0);
        assert!(y_bar > y_window);
    }

    #[test]
    fn drag_spring_converges_without_blowing_up() {
        let mut x = 0.0;
        let mut v = 0.0;
        for _ in 0..120 {
            spring_axis(&mut x, &mut v, 240.0, 1.0 / 60.0);
        }
        assert!((x - 240.0).abs() < 1.0, "x={x}");
        assert!(v.abs() < 20.0, "v={v}");
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
    fn collapsed_hot_zone_keeps_pill_rect() {
        // 顶部吸附：胶囊 500×48，位于窗口 (26,6)。
        let r = clamp_hover_rect((26.0, 6.0, 500.0, 48.0), "top").unwrap();
        assert_eq!(r, (26.0, 6.0, 500.0, 48.0));
        // 左右吸附：竖胶囊 48×440，窗口 70×520。
        let r = clamp_hover_rect((11.0, 40.0, 48.0, 440.0), "right").unwrap();
        assert_eq!(r, (11.0, 40.0, 48.0, 440.0));
    }

    #[test]
    fn expanded_size_report_is_clamped() {
        // 窗口恒为展开态 552×520：报告只按窗口边界裁剪，不再被收起尺寸压扁
        // （收起动画期间的误判由 LAST_EXPANDED 和前端重新上报兜底）。
        let r = clamp_hover_rect((26.0, 6.0, 494.0, 450.0), "top").unwrap();
        assert_eq!(r, (26.0, 6.0, 494.0, 450.0));
        // 越界部分裁进窗口。
        let r = clamp_hover_rect((-10.0, 500.0, 700.0, 60.0), "top").unwrap();
        assert_eq!(r, (0.0, 500.0, 552.0, 20.0));
        // 整个越界的报告回退给调用方处理。
        assert_eq!(clamp_hover_rect((600.0, 6.0, 500.0, 48.0), "top"), None);
        assert_eq!(clamp_hover_rect((f64::NAN, 6.0, 500.0, 48.0), "top"), None);
    }

    #[test]
    fn mouse_through_follows_pill_rect() {
        // 窗口常驻展开态尺寸（552×520 逻辑 = 690×650 物理），顶部吸附时
        // 胶囊 500×48 在 (26,6)。
        let win = (1000, 0, 690, 650);
        let pill = Some((26.0, 6.0, 500.0, 48.0));
        let s = 1.25;
        // 胶囊正中可交互；左右留白、胶囊下方的留白穿透。
        assert!(!should_mouse_through("top", win, s, pill, (1345, 37)));
        assert!(should_mouse_through("top", win, s, pill, (1010, 40)));
        assert!(should_mouse_through("top", win, s, pill, (1345, 300)));
        // 没上报过胶囊矩形：保持整窗可交互（宁可挡着也别乱穿透）。
        assert!(!should_mouse_through("top", win, s, None, (1010, 40)));
        // 展开态：整窗可交互，绝不穿透。
        LAST_EXPANDED.store(true, Ordering::Relaxed);
        assert!(!should_mouse_through("top", win, s, pill, (1010, 40)));
        LAST_EXPANDED.store(false, Ordering::Relaxed);
        // 窗口尺寸对不上（拖拽期间的「条」）：不动样式。
        assert!(!should_mouse_through(
            "top",
            (1000, 0, 690, 88),
            s,
            pill,
            (1010, 40)
        ));
        // 左右吸附的竖胶囊：同样在常驻窗口里，48×440 贴右边。
        let side = (0, 0, 690, 650);
        let side_pill = Some((493.0, 40.0, 48.0, 440.0));
        assert!(!should_mouse_through(
            "right",
            side,
            s,
            side_pill,
            (640, 300)
        ));
        assert!(should_mouse_through(
            "right",
            side,
            s,
            side_pill,
            (500, 300)
        ));
        // 非法缩放：不动样式。
        assert!(!should_mouse_through(
            "top",
            win,
            f64::NAN,
            pill,
            (1010, 40)
        ));
    }
}
