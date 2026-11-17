//! Hands the island's desktop capture from the WebView fallback to the native
//! renderer. Only one DXGI duplication session per output can run in a process.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

use super::native_glass::{NativeGlass, Surface};
use super::{backdrop, capture_guard, ISLAND_WINDOW_LABEL};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_DELAY: Duration = Duration::from_secs(8);

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

pub(super) fn set_allowed(app: &AppHandle, allowed: bool) {
    if let Ok(mut glass) = GLASS.lock() {
        glass.allowed = allowed;
        if !allowed {
            glass.requested = None;
            glass.retry_at = None;
            glass.stop(app);
        }
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

/// Called from the existing mouse-through worker. The JavaScript paint loop
/// only updates the requested shape, including the animated drag/snap frames.
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
