//! 主窗口（Dashboard / label "main-window"）：无边框常规大窗（1100×720、
//! 前端自绘标题栏、可自由缩放、任务栏可见），承载用量总览 / 周额度对比 /
//! 价格设置等管理与分析功能（第一阶段为应用壳 + 三个占位页）。
//!
//! 与其余三个窗口的形态差异：
//! - panel（300×500 悬浮面板）：无边框透明置顶 + 失焦自动隐藏，托盘左键
//!   切换显示；
//! - pet / session-hud：透明无边框置顶、不抢焦点的小窗；
//! - main-window：无边框窗（decorations(false)、不置顶、不 skip_taskbar、
//!   阴影默认），标题栏由 dashboard 前端自绘（拖拽 / 最大化 / 关闭见
//!   src/dashboard/TitleBar.tsx），边缘缩放走 tao 在不透明窗上的
//!   WM_NCHITTEST 系统热区（前端不挂 resize 热区）；用户点标题栏"×"时
//!   **关闭即销毁**（省内存改造，lib.rs 的 CloseRequested 挂点已不再拦截
//!   本窗口）——dashboard 的 WebView 渲染进程随之释放，下次打开（托盘
//!   右键"打开主窗口" / 悬浮窗统计页按钮）走现场建窗，接受首次几百 ms
//!   冷启动；应用退出仍由托盘"退出 AgentHUD"完成。
//! - 启动预热（已取消）：原先应用 setup 阶段即预建隐藏的主窗口
//!   （prebuild_main_window，visible(false)），首次打开只剩 show + focus；
//!   省内存改造删除该预热——主窗口属于"用户没打开就不该占内存"的窗体，
//!   冷建路径成为主路径（本文件下方 open_main_window_on_main_thread）。
//! - 跳页：open_main_window 可带 page（Dashboard 侧栏页 id，白名单见
//!   DASHBOARD_PAGES），开窗/聚焦后经 zbar://dashboard-navigate 事件让
//!   DashboardApp 切到对应页（面板价格入口 → 主窗口定价页走此通道）。
//!   冷建路径事件可能赶在前端监听注册前被丢弃，配套 pending page 补发
//!   机制（见 PENDING_PAGE 与 take_pending_dashboard_page）。
//!
//! 建窗铁律（照搬 pet.rs / session_hud.rs）：
//! - WebviewWindowBuilder 必须在主线程事件循环上下文调用。合法调用点：
//!   run_on_main_thread 投递的闭包、托盘菜单回调（on_menu_event 本身在
//!   主线程执行，可直接调 open_main_window_on_main_thread）；不能在同步
//!   invoke 命令主体里直接建窗——同步命令占用主线程，建窗等不到事件循环
//!   处理消息，自等待死锁（pet.rs 有事故记载）。
//! - 命令侧统一 async + run_on_main_thread + channel + 超时等待（同
//!   session_hud::apply_session_hud_config）：把"已存在则显示聚焦"与
//!   "首次建窗"都放进主线程闭包执行，窗口 API 调用串行落在事件循环线程，
//!   同时把建窗错误（中文）经 channel 回传给前端。

use std::sync::mpsc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use crate::webview_args::MergedRendererBuilder;

/// 主窗口 label（与 capabilities/dashboard.json 的 windows 白名单、
/// dashboard.html 入口一致）
pub const MAIN_WINDOW_LABEL: &str = "main-window";

/// 主窗口默认尺寸（逻辑 px）：1100×720 管理/分析大窗
pub const MAIN_WINDOW_WIDTH: f64 = 1100.0;
pub const MAIN_WINDOW_HEIGHT: f64 = 720.0;
/// 主窗口最小尺寸（逻辑 px）：再小则侧栏 + 主区并排布局挤不下
pub const MAIN_WINDOW_MIN_WIDTH: f64 = 900.0;
pub const MAIN_WINDOW_MIN_HEIGHT: f64 = 640.0;

/// 主窗口操作（建窗/显示聚焦）的等待超时：仅事件循环异常退出时兜底，
/// 正常路径毫秒级返回
const MAIN_WINDOW_OP_TIMEOUT: Duration = Duration::from_secs(8);

/// 面板 → 主窗口的跳页事件（风格沿 zbar://appearance-changed）：
/// open_main_window 带 page 参数时，开窗/聚焦后 emit_to 主窗口 WebView，
/// 由 DashboardApp.tsx 监听并 setView（payload 为下方白名单内的页 id）
const DASHBOARD_NAVIGATE_EVENT: &str = "zbar://dashboard-navigate";

/// 主窗口可跳转页白名单（与 src/dashboard/DashboardApp.tsx 的 View 枚举
/// 一致，改动任一侧需同步）。不在名单内的 page 只开窗不导航
const DASHBOARD_PAGES: [&str; 12] = [
    "overview",
    "logs",
    "sessions",
    "speed",
    "compare",
    "quota",
    "sync",
    "accounts",
    "pricing",
    "agents",
    "settings",
    "about",
];

/// page 参数收敛：合法页 id 原样放行，缺省/非法一律 None（只开窗不导航）
fn normalize_dashboard_page(page: Option<&str>) -> Option<&str> {
    match page {
        Some(p) if DASHBOARD_PAGES.contains(&p) => Some(p),
        _ => None,
    }
}

/// 向主窗口 WebView 发跳页事件（emit_to 定向 main-window，不广播到面板/
/// 宠物等其余窗口）。发送失败静默：导航是锦上添花，窗口本身已打开聚焦
fn emit_dashboard_navigate(app: &AppHandle, page: Option<&str>) {
    if let Some(page) = normalize_dashboard_page(page) {
        let _ = app.emit_to(MAIN_WINDOW_LABEL, DASHBOARD_NAVIGATE_EVENT, page);
    }
}

/// 冷建窗口的待补发跳页（省内存改造配套）：冷建路径下 dashboard.html 要
/// 重新加载，emit 出去的跳页事件可能赶在 DashboardApp 注册监听前被丢弃。
/// 建窗时把 page 存到这里；前端 DashboardApp 挂载完成后主动 invoke
/// [`take_pending_dashboard_page`] 原子取走并清空——无论 emit 是否被前端
/// 收到，导航意图都不会丢；emit 先生效的场景 take 返回 None，不会重复
/// 切页。静态 Mutex 即可：page 是单值低频写入，主线程建窗路径写入、
/// 前端命令线程读取，锁内无任何阻塞操作。
static PENDING_PAGE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 存入待补发跳页（覆盖旧值：后一次打开意图优先）
fn set_pending_page(page: &str) {
    if let Ok(mut guard) = PENDING_PAGE.lock() {
        *guard = Some(page.to_string());
    }
}

/// 取走待补发跳页（原子取出并清空，避免重复消费）
fn take_pending_page() -> Option<String> {
    PENDING_PAGE
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
}

/// 前端启动补发命令：DashboardApp 挂载完成后调用，取走冷建时暂存的跳页
/// 意图（None = 无待补发，emit 路径已生效或本次打开不带 page）。
#[tauri::command]
pub fn take_pending_dashboard_page() -> Option<String> {
    take_pending_page()
}

/// 打开主窗口命令（悬浮窗统计页按钮与托盘菜单共用同一主体）：
/// - 窗口已存在（用户没关）→ show + unminimize + set_focus；
/// - 不存在 → 主线程建窗（dashboard.html）。
/// - page（可选）：合法页 id 时开窗/聚焦后向主窗口发 zbar://dashboard-navigate
///   跳页（如面板价格入口传 "pricing"）；缺省/非法只开窗不导航。
///
/// async + channel + 超时等待：见模块 doc 的死锁防护说明。返回 Err 时
/// 前端只需静默（窗口创建失败由后端日志与错误串表达），不阻断交互。
#[tauri::command]
pub async fn open_main_window(
    app: AppHandle,
    page: Option<String>,
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let app_main = app.clone();
    // page 整体 move 进闭包（'static 要求），闭包体内再 as_deref 借用
    app.run_on_main_thread(move || {
        let _ = tx.send(open_main_window_on_main_thread(&app_main, page.as_deref()));
    })
    .map_err(|e| format!("投递主窗口操作到主线程失败: {e}"))?;

    match rx.recv_timeout(MAIN_WINDOW_OP_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(e) => Err(format!("主窗口操作超时: {e}")),
    }
}

/// 打开主窗口主体，**必须在主线程事件循环上下文调用**（模块 doc 的建窗
/// 铁律）：托盘菜单回调可直接调用（on_menu_event 在主线程执行），命令与
/// 其他上下文须经 run_on_main_thread 投递。窗口已存在时只显示/聚焦，
/// 不重复建窗（WebviewWindowBuilder 对同名 label 会直接报错）。
/// page 语义同 open_main_window 命令（None = 只开窗不导航，托盘入口用）。
pub fn open_main_window_on_main_thread(
    app: &AppHandle,
    page: Option<&str>,
) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        // 已存在（用户没关 / 关闭后又被别的路径重建）的秒开路径：显示 →
        // 取消最小化 → 聚焦（顺序固定，最小化状态下直接 set_focus 在
        // Windows 上不会还原窗口）。WebView 与页面监听均已就绪，跳页
        // 事件必达，不需要 pending 补发；但 PENDING_PAGE 要同步为
        // 「最近一次打开意图」：带 page 写入新值、不带 page 清空——
        // 否则更早一次冷开残留的待补发页，会被后续某次窗口重建后的
        // 前端挂载误消费，落到旧意图的页
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        match normalize_dashboard_page(page) {
            Some(p) => set_pending_page(p),
            None => {
                let _ = take_pending_page();
            }
        }
        emit_dashboard_navigate(app, page);
        return Ok(());
    }
    // 冷建路径（省内存改造后的主路径）：关闭即销毁后，每次打开都是现场
    // 建窗。带 page 时先把跳页意图存入 PENDING_PAGE 再建窗——dashboard
    // 前端挂载完成后会主动 invoke take_pending_dashboard_page 取走，emit
    // 只是尽力而为的双保险（此时监听多半未注册，事件可能被丢弃）。
    // 丢失时主窗口落在默认总览页的老问题由 pending 补发兜住
    if let Some(p) = normalize_dashboard_page(page) {
        set_pending_page(p);
    }
    let outcome = ensure_main_window(app);
    if outcome.is_ok() {
        emit_dashboard_navigate(app, page);
    } else {
        // 建窗失败：清掉待补发页，避免残留值被下次打开的前端误消费
        let _ = take_pending_page();
    }
    outcome
}

/// 创建 "main-window" 主窗口（必须在主线程事件循环上下文调用，见模块
/// doc）。形态刻意与悬浮窗相反：无边框 + 前端自绘标题栏（用户可拖拽 /
/// 最大化 / 关闭，见 src/dashboard/TitleBar.tsx）、可自由缩放（边缘缩放
/// 走 tao 的 WM_NCHITTEST 系统热区，前端不挂 resize 热区）、居中出生、
/// 任务栏可见（默认，不调 skip_taskbar）、不置顶（默认）、阴影默认（不调
/// shadow，交给系统），保证是标准桌面大窗而非悬浮工具窗。已存在时提前
/// 返回 Ok（幂等，避免同名重复建窗报错）。
///
/// 省内存改造后本函数只有一种创建形态：可见直建（关闭即销毁，每次打开
/// 都走这里），原先隐藏预建的 visible(false) 分支随 prebuild_main_window
/// 一并删除。
fn ensure_main_window(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(MAIN_WINDOW_LABEL).is_some() {
        return Ok(());
    }

    WebviewWindowBuilder::new(
        app,
        MAIN_WINDOW_LABEL,
        WebviewUrl::App("dashboard.html".into()),
    )
    .title("AgentHUD")
    .inner_size(MAIN_WINDOW_WIDTH, MAIN_WINDOW_HEIGHT)
    .min_inner_size(MAIN_WINDOW_MIN_WIDTH, MAIN_WINDOW_MIN_HEIGHT)
    // 无系统标题栏：拖拽 / 最大化 / 关闭由 dashboard 前端自绘标题栏承担
    // （decorations(false) 的不透明窗在 Windows 上仍保留 tao 的系统边缘
    // 缩放热区，前端无需挂 resize 热区，见 ResizeHandles.tsx 头注释）。
    // 已知取舍：无边框后 Win11 悬停最大化按钮的"贴靠布局"浮出与
    // Alt+Space 系统菜单不再可用——如需恢复需自定义 WM_NCHITTEST 返回
    // HTMINBUTTON/HTMAXBUTTON/HTCLOSE，暂不实现
    .decorations(false)
    .resizable(true)
    .center()
    .visible(true)
    // 合并 WebView2 渲染进程（内存压降专项，见 webview_args.rs 模块注释）
    .merged_renderer()
    .build()
    .map_err(|e| format!("创建主窗口失败: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_dashboard_page;

    #[test]
    fn 合法页id原样放行() {
        for page in [
            "overview",
            "logs",
            "sessions",
            "speed",
            "compare",
            "quota",
            "sync",
            "accounts",
            "pricing",
            "agents",
            "settings",
            "about",
        ] {
            assert_eq!(normalize_dashboard_page(Some(page)), Some(page));
        }
    }

    #[test]
    fn 缺省与非法页id只开窗不导航() {
        assert_eq!(normalize_dashboard_page(None), None);
        // 大小写敏感：与 DashboardApp 的 View 值字面不一致即非法
        assert_eq!(normalize_dashboard_page(Some("Pricing")), None);
        // 未知页 / 空串 / 事件名仿冒一律忽略
        assert_eq!(normalize_dashboard_page(Some("stats")), None);
        assert_eq!(normalize_dashboard_page(Some("")), None);
        assert_eq!(
            normalize_dashboard_page(Some("zbar://dashboard-navigate")),
            None
        );
    }
}
