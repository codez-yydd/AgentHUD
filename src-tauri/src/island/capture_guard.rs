//! 屏幕采集守卫：本应用自己的截图 / 录屏正在抓屏时，暂停毛玻璃底板采样。
//!
//! `backdrop::capture` 每 650ms 会做一次 GDI 桌面采样（见 backdrop.rs 模块
//! 注释）。本应用自己截图/录屏期间再并发一次底板采样没有意义，还会和抓屏
//! 抢桌面 DC，所以用计数守卫让 `backdrop` 直接复用上一张图。
//!
//! 计数而不是布尔：录屏线程和截图命令可能同时持有。

use std::sync::atomic::{AtomicUsize, Ordering};

static DEPTH: AtomicUsize = AtomicUsize::new(0);

/// 进入一次「屏幕正在被采集」的区间，`drop` 时退出。
pub struct CaptureGuard(());

impl CaptureGuard {
    pub fn enter() -> Self {
        DEPTH.fetch_add(1, Ordering::SeqCst);
        CaptureGuard(())
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        DEPTH.fetch_sub(1, Ordering::SeqCst);
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
