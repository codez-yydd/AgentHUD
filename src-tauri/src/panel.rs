//! panel 悬浮窗（label "panel"）：用量监控主面板（300×500 无边框透明置顶
//! 小窗，失焦自动隐藏，index.html 入口），承载统计总览 / 设置页 / 皮肤页
//! 等面板内功能。
//!
//! 省内存改造（本模块诞生的原因）：panel 原先定义在 tauri.conf.json 的
//! app.windows 静态数组里——应用一启动 Tauri 就无条件创建 WebView，即使
//! 用户从不想用它。现在静态定义移除，改为**按设置按需建窗**：
//! - 启动：setup 里读 panel.json 的 enabled（缺省 true，保持老用户体验），
//!   开启才建窗（visible(false) 隐藏出生，首次托盘左键 / 快捷键呼出才
//!   show，行为与原静态定义完全一致）；
//! - 关闭：设置页「悬浮窗」开关置 false → 销毁窗口（WebView 渲染进程
//!   随之释放），用户没开启的功能一个 WebView 都不留；
//! - 重开入口：托盘左键 / 全局快捷键 / 托盘右键菜单「打开悬浮窗」（三处
//!   都收敛到 lib.rs 的 toggle_panel，panel 缺失时现场重建，见该函数注释）。
//!
//! 建窗参数从 tauri.conf.json 的静态定义**逐项迁移**（尺寸 / min / max /
//! transparent / 装饰 / windowEffects 等，改任一侧必须同步另一侧）：
//! - windowEffects（popover / active / radius 16）：popover 材质仅 macOS
//!   支持，Windows 本就被 Tauri 忽略，故按 tray_menu.rs 同款只在 macOS
//!   分支挂 EffectsBuilder，行为与原静态配置等价；
//! - macOSPrivateApi（transparent 依赖）是 app 级配置，仍留在
//!   tauri.conf.json，不受本次改动影响。
//!
//! 建窗铁律（照搬 main_window.rs / session_hud.rs）：WebviewWindowBuilder
//! 必须在主线程事件循环上下文调用。合法调用点：setup 闭包、托盘事件回调、
//! 全局快捷键回调、run_on_main_thread 投递的闭包（set_panel_enabled 命令
//! 用 async + 主线程投递 + channel + 超时等待，同
//! session_hud::apply_session_hud_config 口径），不在非主线程直接建窗。
//!
//! 顺序依赖（改 setup 时注意）：Windows 置顶恢复与 macOS vibrancy 调整都
//! 用 get_webview_window("panel") 取窗——本模块的启动建窗必须先于这两段
//! 执行；窗口不存在时两处 if let Some 自然跳过（panel 关闭态零开销）。

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use crate::webview_args::MergedRendererBuilder;

/// panel 窗口 label（与 capabilities/default.json 的 windows 白名单、
/// index.html 入口一致；lib.rs 失焦自动隐藏、单实例唤醒等挂点按本 label
/// 匹配，改任一侧必须同步）
pub const PANEL_WINDOW_LABEL: &str = "panel";

/// panel 默认尺寸（逻辑 px，与原 tauri.conf.json 的 width/height 一致）
pub const PANEL_WIDTH: f64 = 300.0;
pub const PANEL_HEIGHT: f64 = 500.0;
/// panel 最小 / 最大尺寸（逻辑 px，与原 tauri.conf.json 的 minWidth 等
/// 四项一致；maxInnerSize 决定拖拽上限，与 ResizeHandles 的窗口大小档位
/// 体系配套，勿单独调整）
pub const PANEL_MIN_WIDTH: f64 = 260.0;
pub const PANEL_MIN_HEIGHT: f64 = 430.0;
pub const PANEL_MAX_WIDTH: f64 = 420.0;
pub const PANEL_MAX_HEIGHT: f64 = 760.0;

/// panel 开关操作的等待超时：仅事件循环异常退出时兜底，正常路径毫秒级返回
const PANEL_OP_TIMEOUT: Duration = Duration::from_secs(8);

/// panel 开关配置文件：~/.agenthud/panel.json（缺省 enabled=true，保持
/// 老用户体验；与 pin.json / shortcut.json 同款「缺省字段按默认处理」口径）
fn panel_config_path() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join("panel.json"))
}

/// 读取悬浮窗启用状态。文件不存在或解析失败时默认返回 true（开启），
/// 保证首次运行与异常情况下与原静态定义行为一致。
pub fn load_panel_enabled() -> bool {
    let path = match panel_config_path() {
        Ok(p) => p,
        Err(_) => return true,
    };
    if !path.exists() {
        return true;
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return true,
    };
    serde_json::from_str::<serde_json::Value>(&data)
        .ok()
        .and_then(|v| v.get("enabled").and_then(|b| b.as_bool()))
        .unwrap_or(true)
}

/// 写入悬浮窗启用状态到 panel.json，持久化以便下次启动按需建窗。
fn save_panel_enabled(enabled: bool) -> Result<(), String> {
    let dir = crate::pricing::config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = panel_config_path()?;
    let data = serde_json::json!({ "enabled": enabled }).to_string();
    std::fs::write(&path, data).map_err(|e| format!("写入悬浮窗配置失败: {e}"))
}

/// 创建 "panel" 悬浮窗（必须在主线程事件循环上下文调用，见模块 doc）。
/// 已存在时提前返回 Ok（幂等，避免同名重复建窗报错）。隐藏出生
/// （visible(false)）：启动恢复与设置页开启共用本函数，首次显示统一交给
/// 呼出路径（托盘左键 / 快捷键 / 开关开启分支的 show），与原静态定义
/// visible:false + 按需 show 的时序完全一致。
pub fn ensure_panel_window(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(PANEL_WINDOW_LABEL).is_some() {
        return Ok(());
    }
    let builder = WebviewWindowBuilder::new(
        app,
        PANEL_WINDOW_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .title("AgentHUD")
    .inner_size(PANEL_WIDTH, PANEL_HEIGHT)
    .min_inner_size(PANEL_MIN_WIDTH, PANEL_MIN_HEIGHT)
    .max_inner_size(PANEL_MAX_WIDTH, PANEL_MAX_HEIGHT)
    .resizable(true)
    // 无边框透明置顶小窗：拖拽 / 关闭由前端自绘（StatsPanel 标题栏）
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    // 不调用 center()：与原静态定义 center:false 一致（出生位置交给系统
    // 默认；首次显示前 toggle_panel 会按托盘 / 屏幕重新定位）
    // 合并 WebView2 渲染进程（内存压降专项，见 webview_args.rs 模块注释）
    .merged_renderer()
    .visible(false);
    // windowEffects：popover 材质仅 macOS 支持（radius 与前端面板圆角
    // 16px 配套），Windows 本就被 Tauri 忽略，原静态配置在 Windows 上
    // 无实际效果，故只在 macOS 分支挂载（与 tray_menu.rs 同款做法）
    #[cfg(target_os = "macos")]
    let builder = builder.effects(
        tauri::window::EffectsBuilder::new()
            .effect(tauri::window::Effect::Popover)
            .state(tauri::window::EffectState::Active)
            .radius(16.0)
            .build(),
    );

    let win = builder
        .build()
        .map_err(|e| format!("创建悬浮窗失败: {e}"))?;
    // macOS：显式清空窗口背景，避免 WebView 默认背景把原生
    // NSVisualEffectView 盖成纯白；真正的底色和模糊由 windowEffects 的
    // popover 材质提供（与原 setup 的 macOS 段同款处理）
    #[cfg(target_os = "macos")]
    {
        let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        crate::tune_panel_vibrancy(&win);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = win;
    Ok(())
}

/// 应用启动挂点：开关开启时按需建窗（隐藏出生，首次呼出才 show）；
/// 关闭零开销（不建窗，一个 WebView 都不留）。必须在 setup 的主线程上下文
/// 调用，且先于 lib.rs 里依赖 get_webview_window("panel") 的平台段执行
/// （Windows 置顶恢复 / macOS vibrancy，见模块 doc 的顺序依赖）。
pub fn start_if_enabled(app: &AppHandle) {
    if !load_panel_enabled() {
        return;
    }
    if let Err(e) = ensure_panel_window(app) {
        eprintln!("[zbar-panel] 恢复悬浮窗失败: {e}");
    }
}

/// get_panel_enabled：读取悬浮窗启用状态（设置页开关初始值）
#[tauri::command]
pub fn get_panel_enabled() -> bool {
    load_panel_enabled()
}

/// set_panel_enabled：保存悬浮窗开关并立即应用。
/// - enabled=false：先落盘再主线程销毁 panel 窗口（WebView 渲染进程随之
///   释放；销毁而非隐藏——开关关掉的窗口不该占内存）。
/// - enabled=true：先落盘再主线程建窗（隐藏出生）→ 立即 show + focus，
///   让用户切换后即刻看到面板回来了（显式意图，不等托盘呼出）；置顶状态
///   跟随建窗默认（always_on_top(true)，与原静态定义一致），失焦自动隐藏
///   由 lib.rs 既有挂点接管。
///
/// async + channel + 超时等待：见模块 doc 的建窗铁律说明。先落盘再动窗口
/// （session_hud::apply_session_hud_config 同款顺序）：落盘失败直接报错
/// 不动窗口，状态可恢复；建窗 / 销毁失败时配置已是新值，下次启动生效。
#[tauri::command]
pub async fn set_panel_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    save_panel_enabled(enabled)?;

    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let app_main = app.clone();
    app.run_on_main_thread(move || {
        let outcome = if enabled {
            ensure_panel_window(&app_main).and_then(|()| {
                if let Some(win) = app_main.get_webview_window(PANEL_WINDOW_LABEL) {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
                Ok(())
            })
        } else if let Some(win) = app_main.get_webview_window(PANEL_WINDOW_LABEL) {
            win.destroy().map_err(|e| format!("销毁悬浮窗失败: {e}"))
        } else {
            Ok(())
        };
        let _ = tx.send(outcome);
    })
    .map_err(|e| {
        // 投递失败时配置已落盘（save 失败走不到这里），窗口动作未执行；
        // 提示以后次启动的实际状态为准，前端据此展示
        format!("投递悬浮窗操作到主线程失败: {e}（开关已保存，状态以下次启动为准）")
    })?;

    match rx.recv_timeout(PANEL_OP_TIMEOUT) {
        Ok(outcome) => outcome,
        // 超时：配置已先落盘（save 失败不会走到投递），仅窗口动作未确认
        //（事件循环异常时才会发生，正常路径毫秒级返回）。提示用户以后次
        // 启动的实际状态为准，避免「开关显示已回滚、重启后却是新状态」
        Err(e) => Err(format!(
            "悬浮窗操作超时: {e}（开关已保存，本次窗口动作未确认，状态以下次启动为准）"
        )),
    }
}
