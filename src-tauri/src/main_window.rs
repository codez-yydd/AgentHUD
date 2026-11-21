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
//!   **关闭即销毁**（lib.rs 的 CloseRequested 挂点不拦截本窗口）——
//!   dashboard 的 WebView 渲染进程随之释放，第二次及以后的打开走现场
//!   建窗；应用退出
//!   仍由托盘"退出 AgentHUD"完成。
//! - 启动预热（v2.0.4 恢复）：应用 setup 阶段即预建隐藏的主窗口
//!   （prebuild_main_window，visible(false)），dashboard.html 重页面的
//!   加载藏进启动期，首次打开只剩 show + focus。恢复原因：v2.0.2 省内存
//!   改造删除预热后改为点击时冷建，dev 模式 vite 按需编译 dashboard.html
//!   首次加载 8-10 秒，点击打开时主线程同步冷建重页面 WebView 撞上
//!   初始化死锁窗口 → 白屏 + 窗口消息泵挂死（生产版同样受首次冷建拖累，
//!   PrintWindow 对挂死窗口阻塞 2 分钟以上是硬证据）；v2.0.1 预建模式
//!   无此问题。冷建兜底路径保留（prebuild 失败 / 关闭销毁后的再次打开）。
//! - 跳页：open_main_window 可带 page（Dashboard 侧栏页 id，白名单见
//!   DASHBOARD_PAGES），开窗/聚焦后经 zbar://dashboard-navigate 事件让
//!   DashboardApp 切到对应页（面板价格入口 → 主窗口定价页走此通道）。
//!   冷建路径事件可能赶在前端监听注册前被丢弃，配套 pending page 补发
//!   机制（见 PENDING_PAGE 与 take_pending_dashboard_page）。
//!
//! 启动后的打开请求统一在后台线程调用 WebviewWindowBuilder，由 Tauri 将
//! 建窗投递给窗口事件循环。Windows 上从同步命令或窗口事件回调直接建窗
//! 可能触发 WebView2 死锁（见 Tauri 的 WebviewWindowBuilder::new 文档）。

use crate::webview_args::MergedRendererBuilder;
use std::sync::Mutex;
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

const MAIN_WINDOW_OP_TIMEOUT: Duration = Duration::from_secs(8);

// 串行处理后台打开请求，避免多个入口同时发现窗口缺失而重复建窗。
static MAIN_WINDOW_OPEN: Mutex<()> = Mutex::new(());

/// 面板 → 主窗口的跳页事件（风格沿 zbar://appearance-changed）：
/// open_main_window 带 page 参数时，开窗/聚焦后 emit_to 主窗口 WebView，
/// 由 DashboardApp.tsx 监听并 setView（payload 为下方白名单内的页 id）
const DASHBOARD_NAVIGATE_EVENT: &str = "zbar://dashboard-navigate";

/// 主窗口可跳转页白名单（与 src/dashboard/DashboardApp.tsx 的 View 枚举
/// 一致，改动任一侧需同步）。不在名单内的 page 只开窗不导航
const DASHBOARD_PAGES: [&str; 14] = [
    "overview",
    "logs",
    "sessions",
    "speed",
    "compare",
    "quota",
    "report",
    "sync",
    "accounts",
    "pricing",
    "agents",
    "skin",
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
/// 收到，导航意图都不会丢。静态 Mutex 即可：page 是单值低频写入，后台开窗路径写入、
/// 前端命令线程读取，锁内无任何阻塞操作。
static PENDING_PAGE: Mutex<Option<String>> = Mutex::new(None);

/// 记录最近一次打开的页面，未指定页面时清空旧值。
fn set_pending_page(page: Option<&str>) {
    if let Ok(mut guard) = PENDING_PAGE.lock() {
        *guard = page.map(str::to_owned);
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
/// - 不存在 → 由 Tauri 投递建窗（dashboard.html）。
/// - page（可选）：合法页 id 时开窗/聚焦后向主窗口发 zbar://dashboard-navigate
///   跳页（如面板价格入口传 "pricing"）；缺省/非法只开窗不导航。
#[tauri::command]
pub async fn open_main_window(app: AppHandle, page: Option<String>) -> Result<(), String> {
    let task = tauri::async_runtime::spawn_blocking(move || {
        open_main_window_from_worker(&app, page.as_deref())
    });
    tokio::time::timeout(MAIN_WINDOW_OP_TIMEOUT, task)
        .await
        .map_err(|_| "主窗口操作超时".to_string())?
        .map_err(|e| format!("主窗口任务失败: {e}"))?
}

/// 托盘回调提交打开请求后立即返回，避免在回调中等待 WebView2 初始化。
pub fn open_main_window_from_tray(app: &AppHandle, page: Option<&str>) {
    let app = app.clone();
    let page = page.map(str::to_owned);
    tauri::async_runtime::spawn(async move {
        if let Err(e) = open_main_window(app, page).await {
            eprintln!("[agenthud-main-window] 托盘打开主窗口失败: {e}");
        }
    });
}

fn open_main_window_from_worker(app: &AppHandle, page: Option<&str>) -> Result<(), String> {
    let _open = MAIN_WINDOW_OPEN
        .lock()
        .map_err(|e| format!("无法锁定主窗口打开流程: {e}"))?;
    set_pending_page(normalize_dashboard_page(page));
    if let Some(win) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        // 秒开路径：显示 → 取消最小化 → 聚焦（顺序固定，最小化状态下
        // 直接 set_focus 在 Windows 上不会还原窗口）。预建的隐藏窗口与
        // 已打开的窗口都走这条分支。
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        emit_dashboard_navigate(app, page);
        return Ok(());
    }
    // 冷建时前端可能还未注册事件监听，挂载完成后会主动领取待跳转页。
    let outcome = ensure_main_window(app, true);
    if outcome.is_ok() {
        emit_dashboard_navigate(app, page);
    } else {
        // 建窗失败：清掉待补发页，避免残留值被下次打开的前端误消费
        let _ = take_pending_page();
    }
    outcome
}

/// 应用启动时预建主窗口（隐藏，v2.0.4 恢复，省内存改造曾删除）：把
/// dashboard.html 重页面的加载成本从「首次打开」的等待路径挪到启动阶段
/// ——首次点击「打开主窗口」（托盘菜单 / 悬浮窗统计页按钮）时窗口已
/// 存在，只剩 show + focus。恢复原因：点击时冷建在 dev 模式下撞上 vite
/// 按需编译（dashboard.html 首次加载 8-10s），主线程同步冷建重页面
/// WebView 撞上初始化死锁窗口 → 白屏 + 窗口消息泵挂死（详见模块 doc）。
///
/// 由 setup 阶段调用，此时托盘打开请求尚未开始处理。
/// 隐藏窗口不出现在任务栏：Windows 下任务栏按钮由窗口的 WS_VISIBLE 决定，
/// `visible(false)` 建出的窗口没有任务栏按钮，`show()` 之后才出现（本机
/// 实测类推）；因此不需要额外调 skip_taskbar，形态与正常打开时完全一致。
/// 失败只记日志，首次打开时由后台流程重新创建可见窗口。
pub fn prebuild_main_window(app: &AppHandle) -> Result<(), String> {
    ensure_main_window(app, false)
}

/// 创建 "main-window" 主窗口（由 setup 或后台打开流程调用）。
/// 无边框 + 前端自绘标题栏（用户可拖拽 /
/// 最大化 / 关闭，见 src/dashboard/TitleBar.tsx）、可自由缩放（边缘缩放
/// 走 tao 的 WM_NCHITTEST 系统热区，前端不挂 resize 热区）、居中出生、
/// 任务栏可见（默认，不调 skip_taskbar）、不置顶（默认）、阴影默认（不调
/// shadow，交给系统），保证是标准桌面大窗而非悬浮工具窗。已存在时提前
/// 返回 Ok（幂等，避免同名重复建窗报错）。
///
/// `visible` 区分两种创建路径：启动预热用 false（隐藏建窗，首次打开时
/// show + focus，见 prebuild_main_window）；打开命令在窗口缺失时用 true
/// （冷建兜底路径，行为与直接建窗一致）。
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
            "report",
            "sync",
            "accounts",
            "pricing",
            "agents",
            "skin",
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
