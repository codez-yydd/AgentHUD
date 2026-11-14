//! WebView2 建窗参数统一辅助（内存压降专项）。
//!
//! 背景：WebView2 默认每个 WebView 各起一个渲染进程（--process-per-
//! WebView2 语义），本应用窗口多（panel / island / hud / pet / shot /
//! rec / pin / tray-menu / dashboard……），渲染进程数随窗口数线性增长，
//! 常驻内存被撑大。给所有窗口统一追加 `--process-per-site` 后，同源
//! （http://tauri.localhost）的全部窗口共享同一个渲染进程，渲染进程数
//! 从"每窗一个"收敛为全局 1 个。
//!
//! ⚠️ additionalBrowserArgs 是整体替换语义（wry 0.55.1 webview2/mod.rs
//! 中 `unwrap_or_else` 只在未设置时才拼默认串）：一旦设置，wry 自带的
//! `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`
//! （关闭 mini menu / PDF OOUI / SmartScreen）与 autoplay / proxy 追加
//! 都不会再拼接，因此这里必须手动保留默认串再追加，否则会改变 WebView2
//! 既有行为（如右键 mini menu 复现）。本项目未使用 webview 层 autoplay
//! / proxy，故仅保留默认串 + process-per-site。
//!
//! 取舍记录（实测）：dashboard 重页面与 HUD 等轻窗口共进程后，dashboard
//! 打开时同进程的灵动岛动画可能掉帧——接受该取舍：dashboard 是按需窗，
//! 常驻收益（每窗 ~30-80MB 渲染进程）远大于偶发掉帧。

use tauri::{Manager, Runtime, WebviewWindowBuilder};

/// wry 0.55.1 WebView2 默认浏览器参数（必须原样保留）+ 进程合并参数。
/// 默认串来源：wry-0.55.1/src/webview2/mod.rs `default_args`。
pub(crate) const WEBVIEW2_BROWSER_ARGS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --process-per-site";

/// 为 `WebviewWindowBuilder` 统一追加 WebView2 合并渲染进程参数的扩展
/// trait：所有建窗点在链式调用中插一个 `.merged_renderer()` 即可，保证
/// 参数串只有这一处定义、不会各点漂移。
pub(crate) trait MergedRendererBuilder<R: Runtime> {
    fn merged_renderer(self) -> Self;
}

impl<'a, R: Runtime, M: Manager<R>> MergedRendererBuilder<R> for WebviewWindowBuilder<'a, R, M> {
    fn merged_renderer(self) -> Self {
        self.additional_browser_args(WEBVIEW2_BROWSER_ARGS)
    }
}
