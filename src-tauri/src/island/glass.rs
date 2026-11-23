//! Hands the island's desktop capture from the WebView fallback to the native
//! renderer. Only one DXGI duplication session per output can run in a process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

use super::native_glass::{NativeGlass, Surface};
use super::{backdrop, capture_guard, ISLAND_WINDOW_LABEL};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_DELAY: Duration = Duration::from_secs(8);
/// 玻璃渲染拍的节奏：与旧穿透轮询的 16ms 一致（一次光标刷新）。
const TICKER_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Deserialize)]
pub(crate) struct Geometry {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radii: [f32; 4],
}

impl Geometry {
    fn surface(self) -> Option<Surface> {
        if ![self.x, self.y, self.width, self.height]
            .into_iter()
            .chain(self.radii)
            .all(f32::is_finite)
            || !(1.0..=super::ISLAND_WINDOW_W as f32).contains(&self.width)
            || !(1.0..=super::ISLAND_WINDOW_H as f32).contains(&self.height)
            || self.x.abs() > super::ISLAND_WINDOW_W as f32
            || self.y.abs() > super::ISLAND_WINDOW_H as f32
        {
            return None;
        }
        let max_radius = self.width.min(self.height) / 2.0;
        Some(Surface {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            radii: self.radii.map(|r| r.clamp(0.0, max_radius)),
        })
    }
}

struct Glass {
    allowed: bool,
    requested: Option<(isize, Surface)>,
    renderer: Option<NativeGlass>,
    unready_since: Option<Instant>,
    retry_at: Option<Instant>,
    ready: bool,
    last_luma: Option<f32>,
}

static GLASS: Mutex<Glass> = Mutex::new(Glass {
    allowed: false,
    requested: None,
    renderer: None,
    unready_since: None,
    retry_at: None,
    ready: false,
    last_luma: None,
});

/// `Glass.allowed` 的原子镜像：穿透轮询每拍都要问「玻璃在不在跑」，锁
/// `GLASS` 会和渲染拍争锁；这里只在 [`set_allowed`] 里同步写。
static ALLOWED: AtomicBool = AtomicBool::new(false);
/// 专用 ticker 线程句柄（只在允许时工作，不允许时 park 等待唤醒）。
static TICKER: OnceLock<thread::Thread> = OnceLock::new();
static TICKER_STARTED: AtomicBool = AtomicBool::new(false);

impl Glass {
    fn stop(&mut self, app: &AppHandle) {
        if let Some(renderer) = self.renderer.take() {
            drop(renderer);
            backdrop::release_native_capture();
        }
        self.unready_since = None;
        self.publish(app, false, None);
    }

    fn publish(&mut self, app: &AppHandle, ready: bool, luma: Option<f32>) {
        if self.ready != ready {
            self.ready = ready;
            let _ = app.emit_to(ISLAND_WINDOW_LABEL, "zbar://island-native-glass", ready);
        }
        if !ready {
            self.last_luma = None;
        } else if let Some(value) = luma.filter(|value| {
            self.last_luma
                .is_none_or(|last| (last - value).abs() >= 1.0 / 255.0)
        }) {
            self.last_luma = Some(value);
            let _ = app.emit_to(
                ISLAND_WINDOW_LABEL,
                "zbar://island-native-glass-luma",
                value,
            );
        }
    }
}

/// 允许/禁止玻璃渲染的**唯一**入口。重复调用幂等：`true` 只拉起一条
/// `zbar-island-glass` 线程，`false` 不会反复停/起。
///
/// 允许状态同时写入 [`ALLOWED`]，供穿透轮询无锁读取（见 [`allowed`]）。
pub(super) fn set_allowed(app: &AppHandle, allowed: bool) {
    if let Ok(mut glass) = GLASS.lock() {
        glass.allowed = allowed;
        if !allowed {
            glass.requested = None;
            glass.retry_at = None;
            glass.stop(app);
        }
        ALLOWED.store(allowed, Ordering::SeqCst);
    }
    if allowed {
        start_ticker(app);
        // ticker 可能已经停在 park 上（先 true → false → true）：ensure 之后
        // 必须再唤醒一次；线程起床后以 ALLOWED 为准，重复 set 不会起第二条。
        if let Some(ticker) = TICKER.get() {
            ticker.unpark();
        }
    }
}

/// 玻璃渲染是否处于允许状态。廉价原子读，**不要**为这个判定去锁 `GLASS`
/// （穿透轮询每拍都会问；旧的 `tick` 自己会再确认一次 `glass.allowed`）。
pub(super) fn allowed() -> bool {
    ALLOWED.load(Ordering::Relaxed)
}

/// 懒启动唯一的玻璃 ticker 线程。线程只在允许时按 16ms 调 [`tick`]；不允许
/// 时阻塞在 `thread::park` 上（空闲零周期唤醒），由 `set_allowed(true)` 唤醒。
fn start_ticker(app: &AppHandle) {
    if TICKER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let spawned = thread::Builder::new()
        .name("zbar-island-glass".into())
        .spawn(move || {
            let _ = TICKER.set(thread::current());
            loop {
                while !allowed() {
                    thread::park();
                }
                tick(&app);
                thread::sleep(TICKER_INTERVAL);
            }
        });
    if spawned.is_err() {
        // 起线程失败就允许下一次 set_allowed(true) 重试，避免永久失去刷新。
        TICKER_STARTED.store(false, Ordering::SeqCst);
    }
}

pub(super) fn sync(app: &AppHandle, anchor: isize, geometry: Option<Geometry>) -> bool {
    let Ok(mut glass) = GLASS.lock() else {
        return false;
    };
    let Some(surface) = geometry.and_then(Geometry::surface) else {
        glass.requested = None;
        glass.retry_at = None;
        glass.stop(app);
        return false;
    };
    if glass
        .requested
        .is_some_and(|(previous, _)| previous != anchor)
    {
        glass.stop(app);
        glass.retry_at = None;
    }
    // The WebView can send its first geometry before the main thread applies
    // display affinity. Retain it so a stationary island can start afterward.
    glass.requested = Some((anchor, surface));
    if let Some(renderer) = &glass.renderer {
        renderer.set_surface(Some(surface));
    }
    glass.ready
}

pub(super) fn sync_capture_guard() {
    if let Ok(glass) = GLASS.lock() {
        if let Some(renderer) = &glass.renderer {
            renderer.set_paused(capture_guard::active());
        }
    }
}

/// Called from the dedicated `zbar-island-glass` ticker (only while [`allowed`]).
/// The JavaScript paint loop only updates the requested shape, including the
/// animated drag/snap frames.
pub(super) fn tick(app: &AppHandle) {
    let Ok(mut glass) = GLASS.lock() else {
        return;
    };
    if !glass.allowed {
        return;
    }
    let Some((anchor, surface)) = glass.requested else {
        return;
    };
    let visible = unsafe { IsWindowVisible(HWND(anchor as *mut _)) }.as_bool();
    let paused = capture_guard::active();
    let now = Instant::now();

    if glass.renderer.is_none() {
        if !visible || paused || glass.retry_at.is_some_and(|until| now < until) {
            return;
        }
        if !backdrop::reserve_native_capture() {
            return;
        }
        let Some(renderer) = NativeGlass::start(anchor) else {
            backdrop::release_native_capture();
            glass.retry_at = Some(now + RETRY_DELAY);
            return;
        };
        renderer.set_surface(Some(surface));
        glass.renderer = Some(renderer);
        glass.retry_at = None;
    }

    let Some(renderer) = &glass.renderer else {
        return;
    };
    let ready = visible && renderer.is_ready();
    let luma = if ready { renderer.luma() } else { None };
    glass.publish(app, ready, luma);
    if ready || paused || !visible {
        glass.unready_since = None;
    } else if now.duration_since(*glass.unready_since.get_or_insert(now)) >= STARTUP_TIMEOUT {
        glass.stop(app);
        glass.retry_at = Some(now + RETRY_DELAY);
    }
}
