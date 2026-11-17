//! 本应用截图 / 录屏时暂停背景采样，供 WebView 底板和原生玻璃共用。
//!
//! 计数而不是布尔：录屏线程和截图命令可能同时持有。

use std::sync::atomic::{AtomicUsize, Ordering};

static DEPTH: AtomicUsize = AtomicUsize::new(0);

/// 进入一次「屏幕正在被采集」的区间，`drop` 时退出。
pub struct CaptureGuard(());

impl CaptureGuard {
    pub fn enter() -> Self {
        DEPTH.fetch_add(1, Ordering::SeqCst);
        #[cfg(windows)]
        super::glass::sync_capture_guard();
        CaptureGuard(())
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        DEPTH.fetch_sub(1, Ordering::SeqCst);
        #[cfg(windows)]
        super::glass::sync_capture_guard();
    }
}

/// 当前是否有录屏 / 截图正在采集屏幕。
pub fn active() -> bool {
    DEPTH.load(Ordering::SeqCst) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_is_reentrant_and_releases_on_drop() {
        assert!(!active());
        let outer = CaptureGuard::enter();
        assert!(active());
        {
            let _inner = CaptureGuard::enter();
            assert!(active());
        }
        assert!(active(), "外层还没释放");
        drop(outer);
        assert!(!active());
    }
}
