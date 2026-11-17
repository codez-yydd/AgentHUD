//! Windows companion window for desktop-backed island refraction.

use std::ffi::c_void;
use std::ptr::{self, NonNull};

/// Client-local CSS pixels. Corners run clockwise from the top-left.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Surface {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radii: [f32; 4],
}

extern "C" {
    fn agenthud_glass_start(anchor: isize) -> *mut c_void;
    fn agenthud_glass_stop(handle: *mut c_void);
    fn agenthud_glass_set_surface(handle: *mut c_void, surface: *const Surface);
    fn agenthud_glass_set_paused(handle: *mut c_void, paused: i32);
    fn agenthud_glass_is_ready(handle: *const c_void) -> i32;
    fn agenthud_glass_luma(handle: *const c_void) -> i32;
}

pub(super) struct NativeGlass(NonNull<c_void>);

// The native handle owns shared state. All access is synchronized, and the
// desktop and window resources belong exclusively to the native worker.
unsafe impl Send for NativeGlass {}
unsafe impl Sync for NativeGlass {}

impl NativeGlass {
    pub fn start(anchor_hwnd: isize) -> Option<Self> {
        NonNull::new(unsafe { agenthud_glass_start(anchor_hwnd) }).map(Self)
    }

    pub fn set_surface(&self, surface: Option<Surface>) {
        let surface = surface.as_ref().map_or(ptr::null(), ptr::from_ref);
        unsafe { agenthud_glass_set_surface(self.0.as_ptr(), surface) }
    }

    pub fn set_paused(&self, paused: bool) {
        unsafe { agenthud_glass_set_paused(self.0.as_ptr(), i32::from(paused)) }
    }

    pub fn is_ready(&self) -> bool {
        unsafe { agenthud_glass_is_ready(self.0.as_ptr()) != 0 }
    }

    pub fn luma(&self) -> Option<f32> {
        let luma = unsafe { agenthud_glass_luma(self.0.as_ptr()) };
        (luma >= 0).then_some(luma as f32 / 255.0)
    }
}

impl Drop for NativeGlass {
    fn drop(&mut self) {
        unsafe { agenthud_glass_stop(self.0.as_ptr()) }
    }
}
