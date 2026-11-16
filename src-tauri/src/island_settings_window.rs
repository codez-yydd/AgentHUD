//! 灵动岛设置窗口（label "island-settings"）：灵动岛 HUD 的专属设置面板。
//!
//! 入口与定位：岛展开面板右上角的齿轮（island.html / src/island 前端自绘）
//! 调用 `open_island_settings_window` 打开本窗口。设置项体量已经超出岛内
//! 展开面板能承载的舒适范围（板块显隐 / 天气城市 / 自选股 / 剪贴板与截图
//! 参数 / 热键 / 通知等，还要放长表单与说明文字），因此独立成窗：
//! **920×660 透明无边框小窗**（最小 760×520），圆角 24 的
//! 玻璃卡由前端 CSS 画（标题栏 / 侧栏 / 页脚全部前端自绘，见
//! island-settings.html 与 src/dashboard/islandSettings/SettingsWindow.tsx）。
//!
//! 与 main_window.rs 的复用点：
//! - **建窗铁律逐条照搬**（见下方"建窗铁律"）：命令侧 async +
//!   run_on_main_thread + mpsc channel + 超时等待；WebviewWindowBuilder 只在
//!   主线程事件循环上下文调用；
//! - 窗口形态：decorations(false)（前端自绘标题栏）、resizable(true)、center()、
//!   不置顶、任务栏可见；
//! - **关闭 ≠ 退出**：用户点自绘标题栏"×"（前端请求收起）只隐藏窗口，挂点
//!   在 lib.rs 的 WindowEvent::CloseRequested 分支（prevent_close + hide，
//!   与 main-window 同款写法）——WebView 与页面状态原样保留，再次打开走
//!   show + unminimize + set_focus 秒开，应用退出仍由托盘"退出 AgentHUD"
//!   完成。
//!
//! 与 main_window.rs 的差异（刻意为之，改任何一条都要先看这里的理由）：
//! - **透明窗**：`.transparent(true)` + `.shadow(false)` + 背景色全透明，圆角与
//!   毛玻璃由前端 CSS 承担。因此**不能**沿用主窗口的可见描边 / 系统阴影，
//!   否则圆角外会露出一圈方角底色；
//! - **只做懒创建，不做启动预热**：主窗口是应用主入口，setup 阶段就
//!   prebuild 隐藏窗把 WebView 冷启动挪出首次点击路径；本窗口是低频的
//!   附属设置页，多数用户可能从不打开，预热等于白吃一份 WebView2 进程与
//!   内存（项目对常驻窗口内存有实际压力）。因此这里**没有** `visible(false)`
//!   的预建路径，首次点齿轮才建窗，第二次起只剩 show + focus；
//! - label / 入口页面不同：label "island-settings"（与 capabilities/
//!   island-settings.json 的 windows 白名单同步），页面
//!   island-settings.html（仓库根目录的多入口页面，vite.config.ts 的
//!   rollupOptions.input 同步登记）；
//! - 没有跨窗跳页：主窗口有 zbar://dashboard-navigate（面板价格入口 →
//!   定价页），设置窗口是单页应用，页内标签由前端自己切，Rust 侧不需要
//!   对应事件；
//! - **生命周期跟随灵动岛**：岛被总开关关闭时（island::set_enabled_on_main
//!   的 enabled=false 路径，含 island_set_enabled / island_toggle /
//!   island_set_config 三条命令触发）调
//!   hide_island_settings_window_on_main 一并收起本窗口，避免岛消失后留下
//!   一个没有入口的孤儿设置窗。
//!
//! 建窗铁律（照搬 pet.rs / session_hud.rs / main_window.rs，事故记载见
//! pet.rs）：
//! - WebviewWindowBuilder 必须在主线程事件循环上下文调用。合法调用点：
//!   run_on_main_thread 投递的闭包；**不能**在同步 invoke 命令主体里直接
//!   建窗——同步命令占用主线程，建窗等不到事件循环处理消息，自等待死锁。
//! - 命令侧统一 async + run_on_main_thread + channel + 超时等待（同
//!   main_window::open_main_window / session_hud::apply_session_hud_config）：
//!   把"已存在则显示聚焦"与"首次建窗"都放进主线程闭包执行，窗口 API 调用
//!   串行落在事件循环线程，同时把建窗错误（中文）经 channel 回传前端。

use std::sync::mpsc;
use std::time::Duration;
use tauri::{window::Color, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// 灵动岛设置窗口 label（与 capabilities/island-settings.json 的 windows
/// 白名单、island-settings.html 入口一致）
pub const ISLAND_SETTINGS_WINDOW_LABEL: &str = "island-settings";

/// 窗口默认尺寸（逻辑 px）：920×660
pub const ISLAND_SETTINGS_WINDOW_WIDTH: f64 = 920.0;
pub const ISLAND_SETTINGS_WINDOW_HEIGHT: f64 = 660.0;
/// 窗口最小尺寸（逻辑 px）：再小则 208px 分类导航 + 右侧表单挤不下，
/// 侧栏 14 项也会压到 24px 的最小行高（见 island-settings.css 的 flex 收缩）
pub const ISLAND_SETTINGS_WINDOW_MIN_WIDTH: f64 = 760.0;
pub const ISLAND_SETTINGS_WINDOW_MIN_HEIGHT: f64 = 520.0;

/// 窗口操作（建窗 / 显示聚焦）的等待超时：仅事件循环异常退出时兜底，
/// 正常路径毫秒级返回（与 MAIN_WINDOW_OP_TIMEOUT 同口径）
const ISLAND_SETTINGS_OP_TIMEOUT: Duration = Duration::from_secs(8);

/// 打开灵动岛设置窗口命令（岛展开面板齿轮入口）。
/// - 窗口已存在（含"关闭=隐藏"后仍存活、或被岛关闭流程收起）→
///   show + unminimize + set_focus；
/// - 不存在 → 主线程懒建窗（island-settings.html）并显示聚焦。
///
/// async + channel + 超时等待：见模块 doc 的建窗铁律。返回 Err 时前端只需
/// 静默（建窗失败由后端日志与错误串表达），不阻断岛内其它交互。
#[tauri::command]
pub async fn open_island_settings_window(app: AppHandle) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let app_main = app.clone();
    // 命令无参数：闭包直接 move app 副本进去即可，无需像
    // open_main_window 那样再搬 page
    app.run_on_main_thread(move || {
        let _ = tx.send(open_island_settings_window_on_main_thread(&app_main));
    })
    .map_err(|e| format!("投递灵动岛设置窗口操作到主线程失败: {e}"))?;

    match rx.recv_timeout(ISLAND_SETTINGS_OP_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(e) => Err(format!("灵动岛设置窗口操作超时: {e}")),
    }
}

/// 打开设置窗口主体，**必须在主线程事件循环上下文调用**（模块 doc 的建窗
/// 铁律）：经 run_on_main_thread 投递的闭包满足；托盘菜单回调等主线程回调
/// 也可直接调用。窗口已存在时只显示/聚焦，不重复建窗
/// （WebviewWindowBuilder 对同名 label 会直接报错）。
pub fn open_island_settings_window_on_main_thread(app: &AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(ISLAND_SETTINGS_WINDOW_LABEL) {
        // 关闭=隐藏后的秒开路径：显示 → 取消最小化 → 聚焦（顺序固定，
        // 最小化状态下直接 set_focus 在 Windows 上不会还原窗口；
        // 同 main_window::open_main_window_on_main_thread）
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    ensure_island_settings_window(app)?;
    // 懒建路径：builder 本身按可见+聚焦建窗，这里仍显式 show + unminimize +
    // set_focus 兜底——万一今后有人给 builder 补上 visible(false)（例如学主
    // 窗口改预热），首次点击也不会出现"命令成功但窗口看不见"。三步均幂等，
    // 失败静默（窗口已建出来，聚焦失败不该让前端弹出错误）
    if let Some(win) = app.get_webview_window(ISLAND_SETTINGS_WINDOW_LABEL) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
    Ok(())
}

/// 创建 "island-settings" 设置窗口（必须在主线程事件循环上下文调用，见模块
/// doc）。形态：920×660 小窗、无边框（标题栏由前端
/// 自绘，拖拽走 startDragging）、可自由缩放、居中出生、任务栏可见（默认，不调
/// skip_taskbar）、不置顶（默认，设置页不该盖在别的窗口上）。
///
/// **透明窗要点**（三条必须同时成立，缺一条圆角外就会露出方角底色）：
/// - `transparent(true)`：窗口本身支持逐像素 alpha；
/// - `shadow(false)`：Windows 上透明无边框窗的系统阴影会沿矩形边画，圆角
///   外侧会透出一圈灰边；
/// - `background_color(Color(0,0,0,0))`：WebView 绘制前的底色也透明；前端
///   body / .wset 自己画半径 24 的玻璃卡（见 src/island-settings.css）。
///
/// 已存在时提前返回 Ok（幂等，避免同名重复建窗报错；正常路径由
/// open_island_settings_window_on_main_thread 先查再决定，这里只是兜底）。
/// 私有：唯一调用点是本模块的打开主体，没有启动预热路径（差异说明见模块
/// doc），所以不需要 main_window::ensure_main_window 那样的 visible 参数。
fn ensure_island_settings_window(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(ISLAND_SETTINGS_WINDOW_LABEL).is_some() {
        return Ok(());
    }

    WebviewWindowBuilder::new(
        app,
        ISLAND_SETTINGS_WINDOW_LABEL,
        // 只按 dist 根相对路径加载；页面本体见仓库根的 island-settings.html
        WebviewUrl::App("island-settings.html".into()),
    )
    .title("AgentHUD 灵动岛设置")
    .inner_size(ISLAND_SETTINGS_WINDOW_WIDTH, ISLAND_SETTINGS_WINDOW_HEIGHT)
    .min_inner_size(
        ISLAND_SETTINGS_WINDOW_MIN_WIDTH,
        ISLAND_SETTINGS_WINDOW_MIN_HEIGHT,
    )
    // 无系统标题栏：拖拽 / 关闭由设置页前端自绘标题栏承担（GetCurrentWindow
    // .startDragging + island_settings_window_hide）。decorations(false) 的窗在
    // Windows 上仍保留 tao 的系统边缘缩放热区，前端无需挂 resize 热区。
    // 已知取舍：无边框后 Win11 悬停最大化按钮的"贴靠布局"浮出与 Alt+Space
    // 系统菜单不再可用；本窗刻意不提供最小化 / 最大化按钮。
    .decorations(false)
    .resizable(true)
    // 透明 + 无系统阴影 + 全透明底色：圆角 24 由 CSS 画，圆角外必须完全透明
    .transparent(true)
    .shadow(false)
    .background_color(Color(0, 0, 0, 0))
    .center()
    .visible(true)
    .build()
    .map_err(|e| format!("创建灵动岛设置窗口失败: {e}"))?;

    Ok(())
}

/// 灵动岛总开关被关闭时收起本窗口（island::set_enabled_on_main 的
/// enabled=false 分支调用）。设置窗口是岛的附属窗口：岛隐藏后它既没有入口
/// 也不再有用，必须一并收起，否则用户会看到"岛没了、设置窗还开着"的孤儿
/// 窗口（且关掉它之后还得再从托盘重新开岛才能再见）。
///
/// **必须在主线程事件循环上下文调用**（与建窗铁律同源：窗口 API 串行落在
/// 事件循环线程；set_enabled_on_main 的全部调用点——命令侧 on_main_thread、
/// 托盘 on_menu_event、热键 run_on_main_thread、setup start_if_enabled——
/// 都满足）。首次打开前窗口可能从未创建，此时静默返回；hide 失败也静默：
/// 收起附属窗是收尾动作，不能反过来阻断岛的关闭流程。
pub(crate) fn hide_island_settings_window_on_main(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(ISLAND_SETTINGS_WINDOW_LABEL) {
        let _ = win.hide();
    }
}
