//! 屏幕采集守卫：用户正在录屏 / 截图时，别让灵动岛把自己标成「对采集隐藏」。
//!
//! `backdrop::capture` 为了不让玻璃把岛自己拍进去，每次采样都会临时给岛窗口
//! 设置 `WDA_EXCLUDEFROMCAPTURE`，采完再恢复（见 backdrop.rs 模块注释）。这个
//! 标志对屏幕上的显示没有影响，**但会让所有截屏/录屏 API 看不见这个窗口**：
//! 如果用户此刻正在用本应用录屏，录出来的画面里岛会每隔一次采样就闪掉 1~2 帧
//! （实机录屏：233 帧里 6 帧整块消失）。
//!
//! 所以采集期间用这里的计数守卫把「排除自己」这一步跳过：`backdrop` 直接回
//! 上一次采到的图（毛玻璃本来就是 k 级模糊的壁纸，冻结几秒无感），窗口的标志
//! 保持 `WDA_NONE`，录屏/截图里岛就是连续可见的。
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
