//! 灵动岛「可穿透」（passthrough）的 Windows 实现。
//!
//! 配置项 `IslandConfig.passthrough`（`island_set_passthrough` 命令写入）打开后：
//!
//! 1. 在灵动岛的分层窗口上切换 `WS_EX_TRANSPARENT`，鼠标点击和移动
//!    穿透到下面的应用。关闭时还原透明位，分层模式在窗口生命周期内保持；
//! 2. 独立线程安装 `WH_MOUSE_LL` 低级鼠标钩子，**只在「可穿透开启 + 岛可见」
//!    时安装**，配置热更新/岛关闭/窗口销毁都会自动卸载；
//! 3. 钩子回调只做「命中判定 + 写原子」，真正的窗口/事件动作都由 50ms 的
//!    协调线程执行（钩子回调必须尽快返回，不能持锁、不能碰窗口样式）：
//!    - 右键（`WM_RBUTTONDOWN`）：登记请求；协调线程清 `WS_EX_TRANSPARENT`、
//!      广播 [`ISLAND_PASSTHROUGH_MENU_EVENT`]、并开 2s grace 兜底，超时或前端
//!      调用 [`restore_after_menu`]（命令 `island_passthrough_restore`）就恢复；
//!    - grace 期内左键点在岛可见矩形**之外**：立刻取消 grace，恢复穿透（点
//!      外部即关菜单）；
//!    - 滚轮（`WM_MOUSEWHEEL`）：广播 [`ISLAND_WHEEL_EVENT`] 并**吞掉**该
//!      滚轮事件（岛下方应用不会同时滚动）。
//!
//! 为什么右键选按下（DOWN）而不是抬起（UP）：按下那一刻就登记请求，协调线程
//! 在 ≤50ms 内清掉透明位，同一次右键的 UP 以及随后鼠标捕获内的消息才有机会
//! 落回岛窗口，WebView 原生 `contextmenu` 也可能照常弹出——在前端接上事件
//! 之前右键不会变成「毫无反应」。右键**不吞**（只有滚轮吞）。
//!
//! 线程模型：协调和自动穿透共用窗口样式锁；钩子回调只读原子 + 本地
//! Win32（`GetWindowRect`），hover rect 用 `try_lock`（拿不到就跳过这一拍）。
//! 钩子安装后若超过 30s 一次回调都没有，协调线程会重装一次自愈。
//! 配置读取复用 `island::load_config` 的进程内缓存。

// 纯函数在非 Windows 构建里只被单测使用；Windows 上由 imp 模块调用。
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use tauri::AppHandle;

/// 右键菜单事件：payload `{ x: number, y: number }`，相对 webview 左上角的
/// 逻辑 px（CSS px），供前端在光标位置打开右键菜单。
pub const ISLAND_PASSTHROUGH_MENU_EVENT: &str = "zbar://island-passthrough-menu";
/// 滚轮事件：payload `{ delta: number }`，±1（正 = 向上滚，负 = 向下滚）。
pub const ISLAND_WHEEL_EVENT: &str = "zbar://island-wheel";

/// 右键菜单期间临时恢复可点击的兜底时长（毫秒）。前端在菜单关闭时会调用
/// `island_passthrough_restore` 立即恢复；2s 只是兜底（也覆盖「点外部关闭菜单」
/// 没走命令时的左键取消路径）。
pub const MENU_GRACE_MS: u64 = 2_000;

/// 钩子健康检查：超过 30s 没有任何鼠标钩子回调就认为钩子被系统摘掉/异常失效，
/// 协调线程重装一次。
pub const HOOK_STALE_MS: u64 = 30_000;

// ============================================================
// 纯函数（跨平台，单测覆盖）
// ============================================================

/// 屏幕物理坐标 → webview 逻辑坐标（相对 webview/窗口左上角）。
/// `scale` 非法（NaN / 0 / 负数）时按 1.0 处理。
pub fn screen_to_webview(cursor: (f64, f64), outer: (f64, f64), scale: f64) -> (f64, f64) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    ((cursor.0 - outer.0) / scale, (cursor.1 - outer.1) / scale)
}

/// 光标（屏幕物理 px）是否落在 `rect`（相对 webview 左上角的逻辑 px）内；
/// 命中时同时返回换算后的 webview 逻辑坐标（事件 payload 用）。
pub fn hit_webview_rect(
    cursor: (f64, f64),
    outer: (f64, f64),
    scale: f64,
    rect: (f64, f64, f64, f64),
) -> Option<(f64, f64)> {
    let (x, y) = screen_to_webview(cursor, outer, scale);
    let (rx, ry, rw, rh) = rect;
    let inside = x >= rx && x <= rx + rw && y >= ry && y <= ry + rh;
    inside.then_some((x, y))
}

/// 钩子要拦截的「岛可见矩形」（相对 webview 左上角逻辑 px）。
///
/// - 展开态：整扇窗口（面板 + 投影留白，与旧穿透判定一致）；
/// - 收起态：前端上报的胶囊矩形并裁进窗口；没上报过（或数据显然无效）
///   返回 None——此时不拦截任何右键/滚轮，宁可漏响应也不误吞屏幕上的事件。
pub fn intercept_rect(
    expanded: bool,
    window_logical: (f64, f64),
    reported: Option<(f64, f64, f64, f64)>,
) -> Option<(f64, f64, f64, f64)> {
    if expanded {
        if window_logical.0 < 1.0 || window_logical.1 < 1.0 {
            return None;
        }
        return Some((0.0, 0.0, window_logical.0, window_logical.1));
    }
    let (x, y, w, h) = reported?;
    if !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() || w < 1.0 || h < 1.0 {
        return None;
    }
    let x0 = x.max(0.0);
    let y0 = y.max(0.0);
    let x1 = (x + w).min(window_logical.0.max(1.0));
    let y1 = (y + h).min(window_logical.1.max(1.0));
    (x1 - x0 >= 1.0 && y1 - y0 >= 1.0).then_some((x0, y0, x1 - x0, y1 - y0))
}

/// 滚轮方向：Windows 的 `WHEEL_DELTA` 以 120 为一步，正 = 向上/向前。
/// 这里只保留符号（payload 契约是 ±1）。
pub fn wheel_direction(raw_delta: i32) -> i32 {
    raw_delta.signum()
}

/// 右键菜单临时恢复可点击的截止时刻（进程启动起的毫秒；0 = 不在 grace 期）。
pub fn grace_deadline(now_ms: u64) -> u64 {
    now_ms.saturating_add(MENU_GRACE_MS).max(1)
}

/// 是否仍处于「右键菜单临时可点击」窗口期。
pub fn grace_active(now_ms: u64, until_ms: u64) -> bool {
    until_ms != 0 && now_ms < until_ms
}

/// grace 是否仍然生效：被「左键点外部 / 前端 restore」取消后立即失效
/// （取消只清截止时刻 + 置取消标记，下一拍协调线程立刻恢复穿透）。
pub fn grace_holds(cancelled: bool, now_ms: u64, until_ms: u64) -> bool {
    !cancelled && grace_active(now_ms, until_ms)
}

/// ex-style 位操作：置位 / 清位 / 查询（保留其它位）。
pub fn style_with_bit(style: isize, bit: isize) -> isize {
    style | bit
}
pub fn style_without_bit(style: isize, bit: isize) -> isize {
    style & !bit
}
#[cfg_attr(not(test), allow(dead_code))]
pub fn style_has_bit(style: isize, bit: isize) -> bool {
    style & bit != 0
}

// ============================================================
// Windows 实现
// ============================================================

/// 启动可穿透协调线程（幂等；非 Windows 空实现）。线程常驻到进程退出，
/// 每 50ms 对一次配置/窗口状态：开关热更新、窗口重建、岛隐藏都在这里收敛。
pub fn start(app: AppHandle) {
    imp::start(app);
}

/// 右键菜单关闭后由前端调用（命令 `island_passthrough_restore`）：立刻结束
/// grace，把 `WS_EX_TRANSPARENT` 加回去。非 Windows 空实现。
pub fn restore_after_menu() {
    imp::restore_after_menu();
}

#[cfg(target_os = "windows")]
pub(super) use imp::set_automatic_mouse_through;

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    use tauri::{Emitter, Manager};
    use windows::Win32::Foundation::{
        GetLastError, SetLastError, COLORREF, ERROR_SUCCESS, HWND, LPARAM, LRESULT, RECT, WPARAM,
    };
    use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, LOGPIXELSX};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, GetWindowLongPtrW, GetWindowRect, IsWindow, IsWindowVisible,
        KillTimer, PeekMessageW, SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW,
        SetWindowsHookExW, UnhookWindowsHookEx, GWL_EXSTYLE, HHOOK, LWA_ALPHA, MSG, MSLLHOOKSTRUCT,
        PM_NOREMOVE, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_TIMER,
        WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };

    use super::*;

    /// `WS_EX_TRANSPARENT` 的数值位（0x20）；钩子回调和纯函数共用。
    const TRANSPARENT_BIT: isize = WS_EX_TRANSPARENT.0 as isize;
    const LAYERED_BIT: isize = WS_EX_LAYERED.0 as isize;

    /// 原始句柄（isize，跨线程存在原子里）→ `HWND`。
    fn hwnd_of(raw: isize) -> HWND {
        HWND(raw as *mut core::ffi::c_void)
    }

    static STARTED: AtomicBool = AtomicBool::new(false);
    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    static START_INST: OnceLock<Instant> = OnceLock::new();
    /// 当前岛窗口句柄（0 = 没有/无效），钩子回调只读。
    static HOOK_HWND: AtomicIsize = AtomicIsize::new(0);
    /// 已解析的窗口句柄缓存：只在句柄失效/窗口重建时才会再走一次 Tauri 派发。
    static CACHED_HWND: AtomicIsize = AtomicIsize::new(0);
    /// 当前窗口缩放因子（f64 bits），钩子回调按它把 hover rect 换成物理 px。
    static SCALE_BITS: AtomicU64 = AtomicU64::new(0);
    /// 可穿透当前是否生效（配置开 + 岛可见 + 钩子已装）；钩子回调的快速开关。
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static WINDOW_STYLE: Mutex<()> = Mutex::new(());
    /// 右键菜单 grace 截止时刻（0 = 不在 grace）。
    static GRACE_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
    /// 收到「左键点在岛可见矩形之外」：下一拍立刻结束 grace、恢复穿透。
    static GRACE_CANCEL: AtomicBool = AtomicBool::new(false);
    /// 钩子回调请求打开右键菜单：回调里只写原子，清透明位 / emit 由协调线程做。
    static MENU_REQUEST: AtomicBool = AtomicBool::new(false);
    static MENU_X_BITS: AtomicU64 = AtomicU64::new(0);
    static MENU_Y_BITS: AtomicU64 = AtomicU64::new(0);
    /// 最近一次钩子回调的进程时间（ms）；>30s 没有回调就认为钩子已被系统摘掉，
    /// 下一拍重装（低级钩子超时/被异常卸载时的自愈）。
    static LAST_CALLBACK_MS: AtomicU64 = AtomicU64::new(0);

    pub(super) fn start(app: tauri::AppHandle) {
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = APP.set(app);
        let _ = START_INST.set(Instant::now());
        let spawned = std::thread::Builder::new()
            .name("zbar-island-mousehook".into())
            .spawn(run)
            .is_ok();
        if !spawned {
            // 起线程失败：回滚标记，让下一次命令 / start 还能重试（静默降级）。
            STARTED.store(false, Ordering::SeqCst);
        }
    }

    /// 前端右键菜单关闭时调用（左键点外部也会走同样的取消路径）。幂等：
    /// 不在 grace 期时无副作用；会丢弃还没被协调线程消费的菜单请求。
    pub(super) fn restore_after_menu() {
        GRACE_UNTIL_MS.store(0, Ordering::SeqCst);
        GRACE_CANCEL.store(false, Ordering::SeqCst);
        MENU_REQUEST.store(false, Ordering::SeqCst);
        if debug() {
            eprintln!("[zbar-island] passthrough menu closed: 恢复 WS_EX_TRANSPARENT");
        }
    }

    fn app() -> Option<&'static tauri::AppHandle> {
        APP.get()
    }

    fn now_ms() -> u64 {
        START_INST
            .get()
            .map(|start| start.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    fn grace_on() -> bool {
        super::grace_holds(
            GRACE_CANCEL.load(Ordering::SeqCst),
            now_ms(),
            GRACE_UNTIL_MS.load(Ordering::SeqCst),
        )
    }

    /// 工作线程：`GetMessage` 消息泵 + 50ms 线程定时器做状态协调。
    ///
    /// `WH_MOUSE_LL` 的回调由「安装钩子的线程」在取消息时派发，所以这里必须
    /// 常驻消息循环；系统会把钩子通知投递到本线程队列，`GetMessageW` 一取就
    /// 触发回调。线程定时器（`SetTimer(None, 0, 50, None)`）负责周期性重读
    /// 配置/窗口状态，让开关热更新、岛隐藏、窗口重建都能自动收敛。
    fn run() {
        // 先把线程消息队列建出来，再挂线程定时器（无窗口也需要队列）。
        unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        }
        let timer = unsafe { SetTimer(None, 0, 50, None) };
        let mut guard = StyleGuard::default();
        let mut hook: Option<HHOOK> = None;
        reconcile(&mut guard, &mut hook);
        loop {
            let mut msg = MSG::default();
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if ret.0 <= 0 {
                break;
            }
            // 钩子的通知消息在 GetMessage 内部已触发回调，这里只需处理定时器。
            if msg.message == WM_TIMER {
                reconcile(&mut guard, &mut hook);
            }
        }
        if timer != 0 {
            unsafe {
                let _ = KillTimer(None, timer);
            }
        }
    }

    fn reconcile(guard: &mut StyleGuard, hook: &mut Option<HHOOK>) {
        let Ok(_style) = WINDOW_STYLE.lock() else { return };
        let cfg = super::super::load_config();
        let hwnd = resolve_hwnd();
        // 窗口换代（重建/销毁）：先把旧句柄上的样式还原掉。
        if guard.hwnd != 0 && guard.hwnd != hwnd {
            guard.restore();
        }
        let visible = hwnd != 0 && unsafe { IsWindowVisible(hwnd_of(hwnd)) }.as_bool();
        // DPI 每拍现算（纯 Win32，不经过主线程）：窗口被拖到另一块缩放不同的
        // 屏上时 hover rect 的换算要跟着变。
        if let Some(scale) = dpi_scale(hwnd) {
            SCALE_BITS.store(scale.to_bits(), Ordering::SeqCst);
        }
        HOOK_HWND.store(hwnd, Ordering::SeqCst);
        let active = cfg.enabled && cfg.passthrough && visible;
        if active {
            // 1) 左键点外部 / 前端 restore 取消 grace：本拍立刻恢复透明样式
            if GRACE_CANCEL.swap(false, Ordering::SeqCst) {
                GRACE_UNTIL_MS.store(0, Ordering::SeqCst);
            }
            // 2) 钩子请求的右键：现在才清透明位并广播事件（回调里只写原子）
            if MENU_REQUEST.swap(false, Ordering::SeqCst) && !grace_on() {
                let x = f64::from_bits(MENU_X_BITS.load(Ordering::SeqCst));
                let y = f64::from_bits(MENU_Y_BITS.load(Ordering::SeqCst));
                GRACE_UNTIL_MS.store(super::grace_deadline(now_ms()), Ordering::SeqCst);
                if let Some(app) = app() {
                    let _ = app.emit(
                        ISLAND_PASSTHROUGH_MENU_EVENT,
                        serde_json::json!({ "x": x, "y": y }),
                    );
                }
                if debug() {
                    eprintln!("[zbar-island] passthrough right-click at {x:.0},{y:.0}");
                }
            }
            let want_transparent = !grace_on();
            guard.ensure(hwnd, want_transparent);
            // 3) 钩子健康检查：超过 30s 没有任何回调说明钩子已被系统摘掉（或
            //    线程被卡住），立刻卸掉重装一次，避免可穿透直接失效。
            let last_callback = LAST_CALLBACK_MS.load(Ordering::SeqCst);
            if hook.is_some()
                && last_callback != 0
                && now_ms().saturating_sub(last_callback) > super::HOOK_STALE_MS
            {
                if let Some(stale) = hook.take() {
                    unsafe {
                        let _ = UnhookWindowsHookEx(stale);
                    }
                }
                if debug() {
                    eprintln!("[zbar-island] WH_MOUSE_LL 超过 30s 无回调，重装钩子");
                }
            }
            if hook.is_none() {
                match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) } {
                    Ok(installed) => {
                        *hook = Some(installed);
                        LAST_CALLBACK_MS.store(now_ms(), Ordering::SeqCst);
                        if debug() {
                            eprintln!(
                                "[zbar-island] passthrough on: WS_EX_TRANSPARENT + WH_MOUSE_LL"
                            );
                        }
                    }
                    Err(err) => {
                        if debug() {
                            eprintln!("[zbar-island] 安装 WH_MOUSE_LL 失败: {err}");
                        }
                    }
                }
            }
        } else {
            guard.restore();
            if let Some(h) = hook.take() {
                unsafe {
                    let _ = UnhookWindowsHookEx(h);
                }
                if debug() {
                    eprintln!("[zbar-island] passthrough off: 钩子已卸载、ex-style 已还原");
                }
            }
            GRACE_UNTIL_MS.store(0, Ordering::SeqCst);
            GRACE_CANCEL.store(false, Ordering::SeqCst);
            MENU_REQUEST.store(false, Ordering::SeqCst);
        }
        ACTIVE.store(active, Ordering::SeqCst);
    }

    fn debug() -> bool {
        std::env::var_os("ZBAR_ISLAND_DEBUG").is_some()
    }

    /// 解析岛窗口句柄：正常情况下只读缓存 + `IsWindow`（纯 Win32，微秒级）；
    /// 只有首次/窗口重建/销毁后才走一次 Tauri 派发。低级钩子线程上不能每拍
    /// 都向主线程派发，否则钩子回调可能超时（系统会静默摘掉钩子）。
    fn resolve_hwnd() -> isize {
        let cached = CACHED_HWND.load(Ordering::SeqCst);
        if cached != 0 && unsafe { IsWindow(Some(hwnd_of(cached))) }.as_bool() {
            return cached;
        }
        let found = app()
            .and_then(|app| app.get_webview_window(super::super::ISLAND_WINDOW_LABEL))
            .and_then(|win| win.hwnd().ok())
            .map(|h| h.0 as isize)
            .filter(|raw| *raw != 0 && unsafe { IsWindow(Some(hwnd_of(*raw))) }.as_bool())
            .unwrap_or(0);
        CACHED_HWND.store(found, Ordering::SeqCst);
        found
    }

    /// 窗口所在显示器的 DPI 缩放（96 = 1.0）。GDI 直接读，不经过主线程。
    fn dpi_scale(hwnd: isize) -> Option<f64> {
        if hwnd == 0 {
            return None;
        }
        unsafe {
            let screen = hwnd_of(hwnd);
            let hdc = GetDC(Some(screen));
            if hdc.is_invalid() {
                return None;
            }
            let dpi = GetDeviceCaps(Some(hdc), LOGPIXELSX);
            let _ = ReleaseDC(Some(screen), hdc);
            (dpi > 0).then(|| dpi as f64 / 96.0)
        }
    }

    fn window_style(hwnd: HWND) -> windows::core::Result<isize> {
        unsafe {
            SetLastError(ERROR_SUCCESS);
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if style == 0 && GetLastError() != ERROR_SUCCESS {
                return Err(windows::core::Error::from_win32());
            }
            Ok(style)
        }
    }

    fn write_window_style(hwnd: HWND, style: isize) -> windows::core::Result<()> {
        unsafe {
            SetLastError(ERROR_SUCCESS);
            let old_style = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
            if old_style == 0 && GetLastError() != ERROR_SUCCESS {
                return Err(windows::core::Error::from_win32());
            }
            Ok(())
        }
    }

    fn set_mouse_through(hwnd: HWND, through: bool) -> windows::core::Result<bool> {
        let current = window_style(hwnd)?;
        let layered = style_with_bit(current, LAYERED_BIT);
        let target = if through {
            style_with_bit(layered, TRANSPARENT_BIT)
        } else {
            style_without_bit(layered, TRANSPARENT_BIT)
        };
        if current == target {
            return Ok(false);
        }

        write_window_style(hwnd, target)?;
        if current != layered {
            // 创建分层表面一次。255 保持 WebView 原有的透明像素合成。
            if let Err(error) =
                unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA) }
            {
                let _ = write_window_style(hwnd, current);
                return Err(error);
            }
        }
        Ok(true)
    }

    pub(in crate::island) fn set_automatic_mouse_through(
        hwnd: HWND,
        through: bool,
    ) -> windows::core::Result<bool> {
        let Ok(_style) = WINDOW_STYLE.lock() else { return Ok(false) };
        // 在同一把锁内复查，避免配置切换时覆盖显式模式的菜单 grace。
        if ACTIVE.load(Ordering::SeqCst) || super::super::load_config().passthrough {
            return Ok(false);
        }
        set_mouse_through(hwnd, through)
    }

    #[derive(Default)]
    struct StyleGuard {
        hwnd: isize,
        original_through: bool,
    }

    impl StyleGuard {
        fn ensure(&mut self, hwnd: isize, through: bool) {
            if self.hwnd != hwnd {
                self.restore();
                let Ok(style) = window_style(hwnd_of(hwnd)) else { return };
                self.hwnd = hwnd;
                self.original_through = style_has_bit(style, TRANSPARENT_BIT);
            }
            if let Err(error) = set_mouse_through(hwnd_of(hwnd), through) {
                if debug() {
                    eprintln!("[zbar-island] 设置可穿透样式失败: {error}");
                }
            }
        }

        fn restore(&mut self) {
            if self.hwnd != 0 && unsafe { IsWindow(Some(hwnd_of(self.hwnd))) }.as_bool() {
                if let Err(error) = set_mouse_through(hwnd_of(self.hwnd), self.original_through) {
                    if debug() {
                        eprintln!("[zbar-island] 恢复可穿透样式失败: {error}");
                    }
                }
            }
            self.hwnd = 0;
            self.original_through = false;
        }
    }

    /// 命中判定的三态：`Unknown` = 这一拍读不到可信几何（锁忙/没上报过），
    /// 调用方据此「不处理也不取消」，避免误吞/误关菜单。
    enum HitTest {
        Inside(f64, f64),
        Outside,
        Unknown,
    }

    /// 命中判定：用窗口外框 + 缓存缩放 + 前端上报的胶囊矩形算「可见矩形」。
    /// 全部是本地 Win32 调用/原子读，不经过 Tauri 事件循环（低级钩子回调里
    /// 不能做会阻塞主线程的事）；hover rect 用 `try_lock`，拿不到就跳过这一拍。
    fn hit_test(hwnd: isize, cursor: (f64, f64)) -> HitTest {
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd_of(hwnd), &mut rect) }.is_err() {
            return HitTest::Unknown;
        }
        let scale = f64::from_bits(SCALE_BITS.load(Ordering::SeqCst));
        if !scale.is_finite() || scale <= 0.0 {
            return HitTest::Unknown;
        }
        let outer = (rect.left as f64, rect.top as f64);
        let window_logical = (
            (rect.right - rect.left) as f64 / scale,
            (rect.bottom - rect.top) as f64 / scale,
        );
        // 钩子回调里不能阻塞等锁：主线程正在更新 hover rect 就跳过这一拍。
        let Ok(reported) = super::super::hover_rect_slot().try_lock() else {
            return HitTest::Unknown;
        };
        let reported = *reported;
        let Some(target) = super::intercept_rect(
            super::super::LAST_EXPANDED.load(Ordering::SeqCst),
            window_logical,
            reported,
        ) else {
            return HitTest::Unknown;
        };
        match super::hit_webview_rect(cursor, outer, scale, target) {
            Some((x, y)) => HitTest::Inside(x, y),
            None => HitTest::Outside,
        }
    }

    /// 低级鼠标钩子：**只读原子、只写原子**，不持锁、不碰窗口样式、不 emit。
    /// （emit、清透明度、grace 计时都在协调线程里做，回调必须尽快返回。）
    unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code < 0 || !ACTIVE.load(Ordering::SeqCst) {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // 健康检查活动时间：任何一条钩子回调都算「钩子还活着」。
        LAST_CALLBACK_MS.store(now_ms(), Ordering::SeqCst);
        let msg = wparam.0 as u32;
        if msg != WM_MOUSEWHEEL && msg != WM_RBUTTONDOWN && msg != WM_LBUTTONDOWN {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let hwnd = HOOK_HWND.load(Ordering::SeqCst);
        if hwnd == 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let cursor = (info.pt.x as f64, info.pt.y as f64);
        // 菜单 grace 期：窗口临时可点击，事件交还窗口原生处理。左键点在岛
        // 可见矩形之外时置取消标记，由协调线程立刻恢复穿透（点外部关菜单）。
        if grace_on() {
            if msg == WM_LBUTTONDOWN && matches!(hit_test(hwnd, cursor), HitTest::Outside) {
                GRACE_CANCEL.store(true, Ordering::SeqCst);
            }
            return CallNextHookEx(None, code, wparam, lparam);
        }
        match msg {
            WM_MOUSEWHEEL => {
                if !matches!(hit_test(hwnd, cursor), HitTest::Inside(_, _)) {
                    return CallNextHookEx(None, code, wparam, lparam);
                }
                // mouseData 高 16 位是带符号的滚轮增量（WHEEL_DELTA=120 一步）
                let raw = ((info.mouseData >> 16) & 0xffff) as u16 as i16;
                let delta = super::wheel_direction(raw as i32);
                if delta != 0 {
                    if let Some(app) = app() {
                        let _ = app.emit(ISLAND_WHEEL_EVENT, serde_json::json!({ "delta": delta }));
                    }
                }
                // 吞掉：岛下面的应用不应该跟着滚
                LRESULT(1)
            }
            WM_RBUTTONDOWN => {
                // 只登记请求（坐标 + 标记），清透明位 / 开 grace / emit 由
                // 协调线程在 ≤50ms 内完成；回调本身不能碰窗口样式。
                if let HitTest::Inside(x, y) = hit_test(hwnd, cursor) {
                    MENU_X_BITS.store(x.to_bits(), Ordering::SeqCst);
                    MENU_Y_BITS.store(y.to_bits(), Ordering::SeqCst);
                    MENU_REQUEST.store(true, Ordering::SeqCst);
                }
                // 不吞：窗口恢复可点击后事件继续派发，原生 contextmenu 仍可用
                CallNextHookEx(None, code, wparam, lparam)
            }
            _ => CallNextHookEx(None, code, wparam, lparam),
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    pub(super) fn start(_app: tauri::AppHandle) {}
    pub(super) fn restore_after_menu() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_to_webview_respects_dpi_scale() {
        // 125%：窗口外框 (1000,200) 物理 px，光标 (1100,450) → 逻辑 (80,200)
        assert_eq!(
            screen_to_webview((1100.0, 450.0), (1000.0, 200.0), 1.25),
            (80.0, 200.0)
        );
        // 100%：直接相减
        assert_eq!(
            screen_to_webview((86.0, 60.0), (0.0, 0.0), 1.0),
            (86.0, 60.0)
        );
        // 150%
        let (x, y) = screen_to_webview((1500.0, 900.0), (0.0, 0.0), 1.5);
        assert!((x - 1000.0).abs() < 1e-9);
        assert!((y - 600.0).abs() < 1e-9);
        // 负外框（副屏在左侧）
        assert_eq!(
            screen_to_webview((-3200.0 + 125.0, 10.0), (-3200.0, 0.0), 1.0),
            (125.0, 10.0)
        );
        // 非法 scale 按 1.0
        assert_eq!(
            screen_to_webview((10.0, 20.0), (0.0, 0.0), f64::NAN),
            (10.0, 20.0)
        );
        assert_eq!(
            screen_to_webview((10.0, 20.0), (0.0, 0.0), 0.0),
            (10.0, 20.0)
        );
    }

    #[test]
    fn hit_webview_rect_checks_pill_bounds() {
        let outer = (1000.0, 200.0);
        let scale = 1.25;
        // 125% 屏：胶囊 500×48 逻辑，在 webview (86,6)
        let rect = (86.0, 6.0, 500.0, 48.0);
        // 物理命中区：x 1000+86*1.25 .. 1000+586*1.25 = 1107.5..1732.5
        //              y 200+7.5 .. 200+67.5 = 207.5..267.5
        let hit = hit_webview_rect((1107.5, 207.5), outer, scale, rect).unwrap();
        assert!((hit.0 - 86.0).abs() < 1e-9);
        assert!((hit.1 - 6.0).abs() < 1e-9);
        assert!(hit_webview_rect((1500.0, 240.0), outer, scale, rect).is_some());
        // 左/上边界外
        assert_eq!(hit_webview_rect((1107.4, 240.0), outer, scale, rect), None);
        assert_eq!(hit_webview_rect((1500.0, 207.4), outer, scale, rect), None);
        // 右/下边界外
        assert_eq!(hit_webview_rect((1732.6, 240.0), outer, scale, rect), None);
        assert_eq!(hit_webview_rect((1500.0, 267.6), outer, scale, rect), None);
        // 展开态整扇窗口（672×520）也命中同一套换算
        let full = (0.0, 0.0, 672.0, 520.0);
        assert!(hit_webview_rect((1005.0, 205.0), outer, scale, full).is_some());
        assert_eq!(hit_webview_rect((999.0, 205.0), outer, scale, full), None);
    }

    #[test]
    fn intercept_rect_prefers_reported_pill_and_clamps() {
        // 收起态：用上报的胶囊矩形
        assert_eq!(
            intercept_rect(false, (672.0, 520.0), Some((86.0, 456.0, 500.0, 48.0))),
            Some((86.0, 456.0, 500.0, 48.0))
        );
        // 越界部分裁进窗口
        assert_eq!(
            intercept_rect(false, (672.0, 520.0), Some((-10.0, 500.0, 700.0, 60.0))),
            Some((0.0, 500.0, 672.0, 20.0))
        );
        // 没上报过 / 数据非法 → 不拦截
        assert_eq!(intercept_rect(false, (672.0, 520.0), None), None);
        assert_eq!(
            intercept_rect(false, (672.0, 520.0), Some((f64::NAN, 0.0, 10.0, 10.0))),
            None
        );
        assert_eq!(
            intercept_rect(false, (672.0, 520.0), Some((0.0, 0.0, 0.0, 0.0))),
            None
        );
        // 整个落在窗口外 → 不拦截
        assert_eq!(
            intercept_rect(false, (672.0, 520.0), Some((700.0, 0.0, 100.0, 100.0))),
            None
        );
        // 展开态：整扇窗口（忽略过期的收起胶囊矩形）
        assert_eq!(
            intercept_rect(true, (672.0, 520.0), Some((86.0, 456.0, 500.0, 48.0))),
            Some((0.0, 0.0, 672.0, 520.0))
        );
        // 展开态但窗口尺寸未知 → 不拦截
        assert_eq!(intercept_rect(true, (0.0, 0.0), None), None);
    }

    #[test]
    fn wheel_direction_keeps_only_sign() {
        // 标准一步 / 多步 / 反向
        assert_eq!(wheel_direction(120), 1);
        assert_eq!(wheel_direction(360), 1);
        assert_eq!(wheel_direction(-120), -1);
        assert_eq!(wheel_direction(-1), -1);
        // 0 增量（部分鼠标的水平/异常事件）不产生 payload
        assert_eq!(wheel_direction(0), 0);
        // 高 16 位解出 i16 后仍保持符号
        assert_eq!(wheel_direction(i16::MIN as i32), -1);
    }

    #[test]
    fn style_bit_ops_preserve_other_bits() {
        let transparent = 0x20isize;
        let orig = 0x0008_0100isize; // 任意其它位组合（TOPMOST | WINDOWEDGE）
                                     // 置位
        let on = style_with_bit(orig, transparent);
        assert!(style_has_bit(on, transparent));
        assert_eq!(on & !transparent, orig, "其它位必须原样保留");
        // 清位
        let off = style_without_bit(on, transparent);
        assert!(!style_has_bit(off, transparent));
        assert_eq!(off, orig);
        // 本来就没有该位时清位是幂等的
        assert_eq!(style_without_bit(orig, transparent), orig);
        // 负值/高位组合也不能破坏其它位
        let layered = orig | 0x0008_0000;
        let on2 = style_with_bit(layered, transparent);
        assert_eq!(style_without_bit(on2, transparent), layered);
    }

    #[test]
    fn menu_grace_expires_after_two_seconds() {
        assert_eq!(MENU_GRACE_MS, 2_000, "菜单 grace 兜底为 2s");
        assert!(!grace_active(0, 0), "0 = 不在 grace");
        assert!(grace_active(0, 1));
        assert!(grace_active(1_999, 2_000));
        assert!(!grace_active(2_000, 2_000), "到点即失效");
        assert!(!grace_active(2_001, 2_000));
        assert_eq!(grace_deadline(0), MENU_GRACE_MS);
        assert_eq!(grace_deadline(500), 500 + MENU_GRACE_MS);
        // 极端 now 不溢出
        assert_eq!(grace_deadline(u64::MAX), u64::MAX);
        // 恢复命令把截止时刻清 0 后立即生效
        assert!(!grace_active(1, 0));
    }

    #[test]
    fn grace_can_be_cancelled_by_outside_click_or_restore() {
        // 未取消且在窗口期内 → 仍生效
        assert!(grace_holds(false, 100, 2_000));
        // 左键点外部 / 前端 restore 置取消标记 → 即使没到期也立即失效
        assert!(!grace_holds(true, 100, 2_000));
        // 已过期 → 失效
        assert!(!grace_holds(false, 2_000, 2_000));
        // 未进入 grace（until=0）→ 失效
        assert!(!grace_holds(false, 0, 0));
    }
}
