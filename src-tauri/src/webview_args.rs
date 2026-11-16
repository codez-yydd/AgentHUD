//! WebView2 建窗参数统一辅助（内存压降专项）。
//!
//! ⚠️ v2.0.2 事故回滚记录（2026-11-15）：本模块曾给所有窗口统一追加
//! `--process-per-site`（同源窗口共享 1 个渲染进程，常驻内存收益明显）。
//! 发布后用户实测：打开主窗口（dashboard 重页面冷建进共享渲染进程）
//! 直接白屏且整个应用无响应、须任务管理器强杀——v2.0.1（独立渲染进程
//! + 预建窗口）无此问题，托盘菜单等轻窗口冷建也正常，指向共享渲染
//! 进程下重页面初始化死锁并拖死等待中的主线程。v2.0.3 起移除该开关，
//! 恢复 WebView2 默认的每窗口独立渲染进程。窗口按需建/用完销毁的改造
//! 保留（关闭即释放渲染进程），常驻进程数不受影响。
//!
//! ⚠️ additionalBrowserArgs 是整体替换语义（wry 0.55.1 webview2/mod.rs
//! 中 `unwrap_or_else` 只在未设置时才拼默认串）：一旦设置，wry 自带的
//! `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`
//! （关闭 mini menu / PDF OOUI / SmartScreen）与 autoplay / proxy 追加
//! 都不会再拼接。Wry 默认启用 autoplay，会在未配置参数的窗口里额外添加
//! `--autoplay-policy=no-user-gesture-required`。共用数据目录的 Windows 窗口
//! 应统一使用本辅助函数，避免 WebView2 因浏览器参数不一致而拒绝创建环境。
//!
//! 保留本模块与 merged_renderer() 调用点的意义：WebView2 浏览器参数
//! 的唯一注入点，未来需要增删开关只改这里，不会各建窗点漂移。

use tauri::{Manager, Runtime, WebviewWindowBuilder};

/// wry 0.55.1 WebView2 默认浏览器参数（原样保留，见模块注释）。
/// 默认串来源：wry-0.55.1/src/webview2/mod.rs `default_args`。
pub(crate) const WEBVIEW2_BROWSER_ARGS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

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
