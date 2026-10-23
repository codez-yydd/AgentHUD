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
//!   **隐藏而不销毁**（挂点见 lib.rs 的 WindowEvent::CloseRequested 分支）
//!   ——webview 与页面状态保留，再次打开（托盘右键"打开主窗口" / 悬浮窗
//!   统计页按钮）走 show + focus 秒开，应用退出仍由托盘"退出 AgentHUD"完成。
//! - 启动预热：应用 setup 阶段（托盘初始化后）即预建隐藏的主窗口
//!   （visible(false)，见 prebuild_main_window），dashboard.html 的 WebView
//!   提前加载，首次打开只剩 show + focus，不再等冷启动。
//! - 跳页：open_main_window 可带 page（Dashboard 侧栏页 id，白名单见
//!   DASHBOARD_PAGES），开窗/聚焦后经 zbar://dashboard-navigate 事件让
//!   DashboardApp 切到对应页（面板价格入口 → 主窗口定价页走此通道）。
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
const DASHBOARD_PAGES: [&str; 5] = ["overview", "logs", "speed", "compare", "pricing"];

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

/// 打开主窗口命令（悬浮窗统计页按钮与托盘菜单共用同一主体）：
/// - 窗口已存在（含"关闭=隐藏"后仍存活）→ show + unminimize + set_focus；
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
        // 关闭=隐藏后的秒开路径：显示 → 取消最小化 → 聚焦（顺序固定，
        // 最小化状态下直接 set_focus 在 Windows 上不会还原窗口）。
        // 启动期预建的隐藏窗口（prebuild_main_window）也走这条分支：
        // WebView 已预热，只剩 show + focus
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        // 已存在窗口的 WebView 与页面监听均已就绪，跳页事件必达
        emit_dashboard_navigate(app, page);
        return Ok(());
    }
    let outcome = ensure_main_window(app, true);
    // 兜底新建路径：WebView 尚未加载完 dashboard.html，事件可能赶在监听
    // 注册前被丢弃——启动预热让该路径几乎不会走到（prebuild 失败才出现），
    // 仍尽力发送；丢失时主窗口落在默认总览页，用户可侧栏手动切换
    if outcome.is_ok() {
        emit_dashboard_navigate(app, page);
    }
    outcome
}

/// 应用启动时预建主窗口（隐藏）：把 WebView 的冷启动成本从「首次打开」
/// 的等待路径挪到启动阶段——首次点击「打开主窗口」（托盘菜单 / 悬浮窗
/// 统计页按钮）时窗口已存在，只剩 show + focus。
///
/// 调用时机与线程：与建窗铁律同源，必须在 setup 阶段的主线程事件循环
/// 上下文调用（setup 闭包本身在主线程执行，可直接调用）。
/// 隐藏窗口不出现在任务栏：Windows 下任务栏按钮由窗口的 WS_VISIBLE 决定，
/// `visible(false)` 建出的窗口没有任务栏按钮，`show()` 之后才出现（本机
/// 实测类推）；因此不需要额外调 skip_taskbar，形态与正常打开时完全一致。
/// 失败只记日志、不阻断启动：首次打开会走 open_main_window_on_main_thread
/// 的「窗口不存在」分支按可见窗口重新创建。
pub fn prebuild_main_window(app: &AppHandle) -> Result<(), String> {
    ensure_main_window(app, false)
}

/// 创建 "main-window" 主窗口（必须在主线程事件循环上下文调用，见模块
/// doc）。形态刻意与悬浮窗相反：无边框 + 前端自绘标题栏（用户可拖拽 /
/// 最大化 / 关闭，见 src/dashboard/TitleBar.tsx）、可自由缩放（边缘缩放
/// 走 tao 的 WM_NCHITTEST 系统热区，前端不挂 resize 热区）、居中出生、
/// 任务栏可见（默认，不调 skip_taskbar）、不置顶（默认）、阴影默认（不调
/// shadow，交给系统），保证是标准桌面大窗而非悬浮工具窗。已存在时提前
/// 返回 Ok（幂等，避免同名重复建窗报错）。
///
/// `visible` 区分两种创建路径：启动预热用 false（隐藏建窗，首次打开时
/// show + focus，见 prebuild_main_window）；打开命令在窗口缺失时用 true
/// （兜底路径，行为与预建之前的直接建窗一致）。
fn ensure_main_window(app: &AppHandle, visible: bool) -> Result<(), String> {
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
    .visible(visible)
    .build()
    .map_err(|e| format!("创建主窗口失败: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_dashboard_page;

    #[test]
    fn 合法页id原样放行() {
        for page in ["overview", "logs", "compare", "pricing"] {
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
