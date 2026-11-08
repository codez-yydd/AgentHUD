//! 自定义托盘右键菜单窗口（macOS 与 Windows 编译，lib.rs 里以 cfg 挂载）。
//!
//! 两平台启用本模块的动机不同：
//! - macOS 是被迫摘菜单：macOS 26 起菜单栏由 ControlCenter 托管，tray-icon
//!   crate 靠覆盖在 NSStatusBarButton 上的子视图拦截 mouseDown 区分左右键
//!   的做法失效——只要给 NSStatusItem 挂了原生 NSMenu，系统直接把左键当
//!   "打开菜单"处理，应用收不到任何 TrayIconEvent::Click（上游
//!   tauri-apps/tray-icon#355 已验证：去掉 setMenu 后左右键事件全部正常
//!   送达）。因此 macOS 上托盘不再挂原生菜单（见 lib.rs setup_tray 的
//!   cfg 分叉注释），左键恢复切换悬浮面板，右键改为弹出本模块创建的
//!   自定义 HTML 菜单窗口。
//! - Windows 是主动采用同一面板：Windows 托盘不挂菜单时左右键 Click 事件
//!   均正常送达，无 macOS 的 NSMenu 问题；改用本模块是纯产品决策——与
//!   macOS 统一"快捷/用量"面板体验。与 macOS 的差异两点：定位锚定点击点
//!   而非菜单栏底部（Windows 托盘图标在任务栏上，任务栏可停靠屏幕四边，
//!   见 show_tray_menu 的平台分支）；不挂系统材质——Windows 材质不裁
//!   圆角，四角会露方角残影，透明窗口直接用页面自身半透明底（见
//!   ensure_tray_menu_window 的平台分支）。
//! - Linux 保留原生菜单与既有行为，本模块不参与编译，保证 Linux 零行为
//!   变化。
//!
//! 菜单窗口本体（tray-menu.html，零 React 轻壳，照 session-hud-main.ts 的
//! 架构哲学）两平台完全共用。
//!
//! 职责：
//! - ensure_tray_menu_window：懒建隐藏窗口（应用 setup 时预热调用一次，
//!   右键时秒开——建窗 ~300ms 不能落在右键路径上）。菜单窗口保持
//!   focusable（默认，不设 focusable(false)）：要接收菜单项点击与 Esc，
//!   "点外部自动收起"依赖 Focused(false) 事件。
//! - show_tray_menu：定位 + show + set_focus。setup_tray 传入托盘事件的
//!   物理坐标，本函数用 monitor_from_point 找到点击所在屏幕、按该屏的
//!   scale_factor 换算逻辑坐标后定位（y 按平台分叉：macOS 贴菜单栏底，
//!   Windows 锚定点击点）；已可见时再按右键 = 收起。窗口高度按屏幕
//!   可用高度夹取（小屏上内容区滚动，底栏始终可见）。
//! - tray_menu_state：轻量状态（版本 / HUD 停靠条开关 / 展示币种）。只做
//!   文件读，秒回；重查询全部放 tray_menu_usage。
//! - tray_menu_usage：用量页一次取数（本机 7 个请求来源 + 同步服务上的
//!   其他设备的汇总/趋势 + 失败来源），内部 spawn_blocking 跑 SQLite 与
//!   远端 HTTP，不在主线程阻塞。
//! - tray_menu_action：菜单项动作分发。同步命令（项目惯例：同步命令在
//!   主线程执行），open-main / open-panel / hud-toggle 要求主线程上下文
//!   （建窗铁律，见 main_window.rs 模块注释与 hud_bar.rs
//!   toggle_from_tray 注释），直接调用即可——与 lib.rs on_menu_event
//!   原生菜单回调同款上下文。
//!
//! 尺寸栅格：窗口 340×580（逻辑 px），与 tray-menu.html 的内容栅格
//! 一一对应：头部 54、标签页 42、内容区自适应滚动（设计值 394）、底栏 90。窗口内边距
//! 为 0（系统材质铺满窗口（仅 macOS Popover；Windows 不挂材质、页面底色即背景），圆角 12），
//! 改任何一端必须同步另一端（见 tray-menu.html 顶部注释）。

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};

/// 菜单窗口 label（Rust 侧定位/hide 与命令 hide 共用）
pub(crate) const TRAY_MENU_WINDOW_LABEL: &str = "tray-menu";

/// 窗口宽度（逻辑 px，与 tray-menu.html 的 #menu-root 宽度一致）
const TRAY_MENU_WIDTH: f64 = 340.0;
/// 窗口总高（逻辑 px）：54(header) + 42(tabs) + 394(content) + 90(footer)
/// = 580（与 tray-menu.html 的尺寸栅格注释一一对应）。小屏显示时
/// show_tray_menu 会按屏幕可用高度夹取，内容区滚动、底栏常驻。
const TRAY_MENU_HEIGHT: f64 = 580.0;
/// 菜单窗口顶部起点（逻辑 px，macOS 专用口径）：菜单栏底部约 25pt，与
/// lib.rs toggle_panel 的 macOS y=25 同口径。Windows 走点击点锚定，
/// 不以本常量为顶部起点（仅无显示器信息的兜底分支沿用）
const TRAY_MENU_TOP_Y: f64 = 25.0;
/// Windows：托盘图标中心到任务栏上沿的近似距离（任务栏逻辑半高：Win11
/// 任务栏 48 逻辑px 的一半；Win10 40px 时误差 4px，视觉无感）
const TRAY_MENU_TASKBAR_HALF_Y: f64 = 24.0;
/// 小屏夹取后的最小窗口高：再矮就连标题 + 底栏都放不下，不如让内容区
/// 从极窄开始滚动
const TRAY_MENU_MIN_HEIGHT: f64 = 360.0;
/// 失焦自动收起后的保护窗口（毫秒）：期间托盘图标的点击不再重新弹出
const AUTO_HIDE_GUARD_MS: u64 = 250;

/// 失焦自动收起（Focused(false) → hide）的时间戳（Unix 毫秒）：托盘图标
/// 点击本身也可能让本窗口失焦，没有时间戳保护会出现"失焦先隐藏 → 托盘
/// 事件又 show"的闪烁往返
static LAST_AUTO_HIDE_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 确保 "tray-menu" 窗口存在（隐藏态，首次显示前 WebView 已就绪）。
/// 必须在主线程事件循环上下文调用（建窗铁律，同 ensure_hud_bar_window）：
/// setup 预热与托盘事件回调都满足。pub(crate)：lib.rs 的 setup 里预热调用。
pub(crate) fn ensure_tray_menu_window(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(TRAY_MENU_WINDOW_LABEL).is_some() {
        return Ok(());
    }
    let builder = WebviewWindowBuilder::new(
        app,
        TRAY_MENU_WINDOW_LABEL,
        WebviewUrl::App("tray-menu.html".into()),
    )
    .title("AgentHUD Menu")
    // 尺寸栅格见模块注释：340×580 与 tray-menu.html 一一对应
    .inner_size(TRAY_MENU_WIDTH, TRAY_MENU_HEIGHT)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    // 双击等场景不能把小窗最大化（同 hud_bar.rs 停靠条）
    .maximizable(false)
    // 菜单第一下点击就要生效（原生菜单的直觉），不要先"激活窗口"吞掉
    .accept_first_mouse(true)
    // Windows 钳宽修复（照搬 hud_bar.rs / session_hud.rs）：tao 的
    // WM_GETMINMAXINFO 默认最小跟踪宽度会把 340 逻辑宽的小窗出生即钳宽
    .min_inner_size(1.0, 1.0)
    .visible(false);
    // 不设 focusable(false)：菜单要接收点击、Esc，且失焦自动收起依赖
    // key 状态变化（与停靠条"永不抢焦点"相反的有意取舍）

    // 窗口材质平台分叉（Linux 不编译本模块，无需分支）：
    // - macOS：popover 材质 + 圆角 12，与 panel 窗口的 windowEffects 同款
    //   做法（tauri.conf.json panel 配 popover/active/16，菜单圆角与前端
    //   CSS 的 12px 一致），毛玻璃质感由系统材质提供。
    // - Windows：不挂系统材质——Mica/Acrylic 是矩形材质不裁圆角（macOS
    //   Popover 带 radius 才会裁），CSS 把内容裁成 12px 圆角后四个角外侧
    //   会露出材质方角残影（2026-11-08 Windows 实测反馈）；透明窗口 +
    //   页面自身半透明实底（tray-menu.html --m-bg）即得干净圆角，观感
    //   与系统材质几乎一致。
    #[cfg(target_os = "macos")]
    let builder = builder.effects(
        tauri::window::EffectsBuilder::new()
            .effect(tauri::window::Effect::Popover)
            .state(tauri::window::EffectState::Active)
            .radius(12.0)
            .build(),
    );

    let win = builder
        .build()
        .map_err(|e| format!("创建托盘菜单窗口失败: {e}"))?;
    // 显式清空背景防 WebView 白底盖住系统材质（macOS/Windows 都需要，
    // 照抄 hud_bar.rs macOS 分支 / panel 的同款处理）
    let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));

    // 失焦自动收起：点窗口外任意区域系统会释放本窗口的 key 状态，
    // Focused(false) → hide（原生菜单"点外面就关"的等价行为）。
    // on_window_event 闭包拿不到 AppHandle，克隆一份捕获
    let app_ev = app.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            LAST_AUTO_HIDE_MS.store(now_ms(), Ordering::Relaxed);
            if let Some(w) = app_ev.get_webview_window(TRAY_MENU_WINDOW_LABEL) {
                let _ = w.hide();
            }
        }
    });
    Ok(())
}

/// 在托盘图标正下方弹出/收起菜单窗口。click_pos 为托盘事件给的**物理**
/// 坐标（多显示器/混合 DPI 下由本函数按目标屏幕的 scale_factor 换算逻辑
/// 坐标，不再借 panel 的缩放系数）。从 on_tray_icon_event（主线程）调用，
/// 建窗合法上下文。
pub(crate) fn show_tray_menu(app: &AppHandle, click_pos: (f64, f64)) {
    if let Err(e) = ensure_tray_menu_window(app) {
        eprintln!("[zbar-tray-menu] {e}");
        return;
    }
    let Some(win) = app.get_webview_window(TRAY_MENU_WINDOW_LABEL) else {
        return;
    };

    // 已可见 → 再按右键 = 收起（与左键切面板的"再点一次隐藏"习惯一致）
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }
    // 刚因失焦自动收起（保护窗口内）的托盘点击不再重新弹出：托盘图标
    // 点击可能先触发本窗口失焦（Focused(false) → hide），随后的托盘
    // Click 事件若无条件 show 会把刚收起的菜单立刻弹回，形成闪烁往返
    let last = LAST_AUTO_HIDE_MS.load(Ordering::Relaxed);
    if last > 0 && now_ms().saturating_sub(last) < AUTO_HIDE_GUARD_MS {
        return;
    }

    // 菜单落点与悬浮面板重叠：弹出菜单前先把可见的 panel 收起
    // （只做 hide，不改 toggle_panel 内部的定位/重绘逻辑）
    if let Some(panel) = app.get_webview_window("panel") {
        if panel.is_visible().unwrap_or(false) {
            let _ = panel.hide();
        }
    }

    // 定位：水平居中对齐托盘图标 x，按**点击所在屏幕**的工作区夹紧
    // （monitor_from_point 拿到的 position 是全局物理坐标，换算成该屏
    // 逻辑坐标后再夹，副屏在主屏右侧时不会把窗口弹回主屏）。y 按平台
    // 分叉：macOS 顶部贴菜单栏底（TRAY_MENU_TOP_Y，同 toggle_panel 口径）；
    // Windows 托盘在任务栏上（任务栏可停靠屏幕底部/顶部/左右侧），菜单
    // 锚定点击点弹出（见下方 if let Some(mon) 内的平台分支）。
    let (px, py) = click_pos;
    let target_monitor = win
        .monitor_from_point(px, py)
        .ok()
        .flatten()
        .or_else(|| win.current_monitor().ok().flatten());
    let scale = target_monitor
        .as_ref()
        .map(|mon| mon.scale_factor())
        .unwrap_or_else(|| win.scale_factor().unwrap_or(1.0));
    let cx = px / scale;
    let mut x = cx - TRAY_MENU_WIDTH / 2.0;
    let mut height = TRAY_MENU_HEIGHT;
    // Windows 额外需要点击点的逻辑 y（菜单锚定点击点）；macOS 恒贴菜单栏
    // 底用不到换算值，不计算以免未使用告警
    #[cfg(target_os = "windows")]
    let cy = py / scale;
    // macOS 的 y 恒定（保持原口径）；Windows 分支内会按点击点改写
    #[cfg(target_os = "macos")]
    let y = TRAY_MENU_TOP_Y;
    #[cfg(target_os = "windows")]
    let mut y = TRAY_MENU_TOP_Y;
    if let Some(mon) = target_monitor {
        // 该屏自己的 scale：混合 DPI 下 position/size 都是物理像素
        let mon_scale = mon.scale_factor();
        let mon_left = mon.position().x as f64 / mon_scale;
        let mon_w = mon.size().width as f64 / mon_scale;
        let mon_h = mon.size().height as f64 / mon_scale;
        let min_x = mon_left + 4.0;
        let max_x = (mon_left + mon_w - TRAY_MENU_WIDTH - 4.0).max(min_x);
        x = x.clamp(min_x, max_x);
        #[cfg(target_os = "macos")]
        {
            // 屏幕高度不够时同步缩窗，内容区滚动、底栏常驻
            height = (mon_h - TRAY_MENU_TOP_Y - 8.0)
                .clamp(TRAY_MENU_MIN_HEIGHT, TRAY_MENU_HEIGHT);
        }
        #[cfg(target_os = "windows")]
        {
            let mon_top = mon.position().y as f64 / mon_scale;
            // 锚定缝 = 图标中心到任务栏贴屏沿的近似距离 24（见
            // TRAY_MENU_TASKBAR_HALF_Y 的 why）+ 6px 呼吸缝，上下半屏
            // 对称使用：菜单贴任务栏边弹出时不整缝贴死，视觉更自然
            let anchor_gap = TRAY_MENU_TASKBAR_HALF_Y + 6.0;
            if cy - mon_top < mon_h / 2.0 {
                // 点击在屏幕上半（任务栏停靠顶部的少数布局）：改在点击点
                // 下方弹出。cy 是托盘图标中心、距任务栏下沿约 24，再叠
                // anchor_gap 的 6px 缝，菜单顶边落在任务栏下沿之上留缝
                y = cy + anchor_gap;
                height = (mon_top + mon_h - y - 8.0)
                    .clamp(TRAY_MENU_MIN_HEIGHT, TRAY_MENU_HEIGHT);
            } else {
                // 常见布局（任务栏在底部/左右侧）：菜单整体收进任务栏上方。
                // cy 是托盘图标中心（约在任务栏半高处、距任务栏上沿约 24），
                // 锚点必须上移半个任务栏高：y = cy - height - anchor_gap 让
                // 菜单底边贴任务栏上沿之上再留缝，不再像旧口径 cy - 2 那样
                // 越过任务栏上沿约 22px 压在任务栏上（2026-11-08 Windows
                // 实测反馈）。height 同步多减 4.0 顶部屏缘安全余量（与
                // macOS 分支 -8 同口径）；小屏时 height 被夹小，菜单自动
                // 缩高（内容区滚动、底栏常驻）
                height = (cy - mon_top - anchor_gap - 4.0)
                    .clamp(TRAY_MENU_MIN_HEIGHT, TRAY_MENU_HEIGHT);
                y = cy - height - anchor_gap;
            }
        }
    } else {
        x = x.max(4.0);
    }
    let _ = win.set_position(LogicalPosition::new(x, y));
    let _ = win.set_size(LogicalSize::new(TRAY_MENU_WIDTH, height));
    let _ = win.show();
    let _ = win.set_focus();
}

/// tray_menu_state 命令返回：轻量、秒回的状态（camelCase 契约）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayMenuState {
    /// 应用版本（「快捷」页底部版本行）
    pub version: String,
    /// HUD 停靠条当前开启态（底栏开关）
    pub hud_bar_enabled: bool,
    /// 当前展示币种是否美元（true = $ / false = ¥），与菜单栏标题同源
    pub is_usd: bool,
}

/// 菜单页状态快照：窗口每次从 hidden → shown 时前端会重新 invoke。
/// 只做本地文件读（版本 / hud-bar.json / currency.json），不碰 SQLite，
/// 保证底栏开关与版本号不被用量查询拖慢。
#[tauri::command]
pub fn tray_menu_state(app: AppHandle) -> TrayMenuState {
    TrayMenuState {
        version: app.package_info().version.to_string(),
        hud_bar_enabled: crate::hud_bar::load_hud_bar_config().enabled,
        is_usd: crate::pricing::load_currency() == "usd",
    }
}

/// 用量页汇总（camelCase 契约，与 tray_menu_usage 同构）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayUsageSummary {
    pub requests: i64,
    pub cost_usd: f64,
    pub cost_cny: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub cache_hit_rate: Option<f64>,
}

/// 用量趋势桶（camelCase 契约）：bucket 为本地时间刻度
/// `YYYY-MM-DDTHH`（小时）/ `YYYY-MM-DD`（日），前端补齐空桶后画图。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayUsagePoint {
    pub bucket: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    pub cost_cny: f64,
}

/// 用量页一次取数的完整返回。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayMenuUsage {
    /// 归一后的档位："today" | "7d" | "30d" | "all"
    pub range: String,
    /// 查询区间（本地自然日边界，半开 [fromMs, toMs)）；all 时 fromMs = 0，
    /// 前端以第一个趋势桶为起点
    pub from_ms: i64,
    pub to_ms: i64,
    /// "hour" | "day"
    pub bucket: String,
    pub is_usd: bool,
    pub summary: TrayUsageSummary,
    pub trend: Vec<TrayUsagePoint>,
    /// 打开 / 查询失败的来源（库不存在不算）；前端弱提示，不清空数据
    pub failed_sources: Vec<String>,
}

/// 本地自然日零点（days_ago = 0 即今天）。按日历日回退而不是直接减
/// 毫秒，跨夏令时不会偏一小时；极端时区歧义回退到毫秒近似。
fn local_day_start(days_ago: i64) -> i64 {
    let now = chrono::Local::now();
    let date = (now - chrono::Duration::days(days_ago)).date_naive();
    date.and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .earliest()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(|| now.timestamp_millis() - days_ago * 86_400_000)
}

/// 档位 → (归一档位, from_ms, to_ms, bucket)。「当天」按小时分桶，其余按
/// 自然日分桶；「全部」从 0 开始，由趋势点决定实际起点。
fn resolve_range(range: &str) -> (&'static str, i64, i64, &'static str) {
    let now = chrono::Local::now().timestamp_millis();
    match range {
        "7d" => ("7d", local_day_start(6), now, "day"),
        "30d" => ("30d", local_day_start(29), now, "day"),
        "all" => ("all", 0, now, "day"),
        _ => ("today", local_day_start(0), now, "hour"),
    }
}

/// tray_menu_usage：用量页的汇总 + 趋势一次取数。async + spawn_blocking：
/// 7 个来源库的 SQLite 查询与价格文件读不能跑在主线程（与 get_trend /
/// get_request_logs 同款卸载方式）。
#[tauri::command]
pub async fn tray_menu_usage(range: String) -> Result<TrayMenuUsage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (range, from_ms, to_ms, bucket) = resolve_range(&range);
        let overview = crate::request_logs::collect_usage_overview(from_ms, to_ms, bucket);
        let summary = overview.summary;
        Ok(TrayMenuUsage {
            range: range.to_string(),
            from_ms,
            to_ms,
            bucket: bucket.to_string(),
            is_usd: crate::pricing::load_currency() == "usd",
            summary: TrayUsageSummary {
                requests: summary.requests,
                cost_usd: summary.cost_usd,
                cost_cny: summary.cost_cny,
                input_tokens: summary.input_tokens,
                output_tokens: summary.output_tokens,
                cache_read_tokens: summary.cache_read_tokens,
                cache_write_tokens: summary.cache_write_tokens,
                total_tokens: summary.total_tokens,
                cache_hit_rate: summary.cache_hit_rate,
            },
            trend: overview
                .trend
                .into_iter()
                .map(|item| TrayUsagePoint {
                    bucket: item.bucket,
                    requests: item.requests,
                    input_tokens: item.input_tokens,
                    output_tokens: item.output_tokens,
                    cache_read_tokens: item.cache_read_tokens,
                    cache_write_tokens: item.cache_write_tokens,
                    tokens: item.total_tokens,
                    cost_usd: item.cost_usd,
                    cost_cny: item.cost_cny,
                })
                .collect(),
            failed_sources: overview.failed_sources,
        })
    })
    .await
    .map_err(|e| format!("托盘菜单用量查询任务失败: {e}"))?
}

/// 菜单项动作分发。同步命令（项目惯例：同步命令在主线程执行）：
/// open-main / open-panel / hud-toggle 要求主线程上下文（建窗铁律，见
/// main_window.rs / hud_bar.rs 的注释），直接调用即可——与 lib.rs
/// on_menu_event 原生菜单回调同款上下文。除 HUD 开关（原地切换、菜单
/// 保持打开）外，动作执行后都把菜单收起。
#[tauri::command]
pub fn tray_menu_action(app: AppHandle, action: String) -> Result<(), String> {
    // HUD 开关是"原地切换"：菜单保持打开，前端按事件把开关态翻到新值
    // （与 CC Switch 托盘面板里切轻量模式后不收起面板同款）。其余动作
    // 都会打开别的窗口 / 退出，先把菜单收起，避免压在新窗口上面。
    let keep_open = action == "hud-toggle";
    if !keep_open {
        if let Some(win) = app.get_webview_window(TRAY_MENU_WINDOW_LABEL) {
            let _ = win.hide();
        }
    }
    match action.as_str() {
        // 打开主窗口（默认停留在上次页面）
        "open-main" => {
            if let Err(e) = crate::main_window::open_main_window_on_main_thread(&app, None) {
                eprintln!("[zbar-tray-menu] 打开主窗口失败: {e}");
            }
        }
        // 主窗口定向页：白名单见 main_window.rs DASHBOARD_PAGES
        "open-main-logs" => {
            if let Err(e) =
                crate::main_window::open_main_window_on_main_thread(&app, Some("logs"))
            {
                eprintln!("[zbar-tray-menu] 打开请求日志页失败: {e}");
            }
        }
        "open-main-pricing" => {
            if let Err(e) =
                crate::main_window::open_main_window_on_main_thread(&app, Some("pricing"))
            {
                eprintln!("[zbar-tray-menu] 打开定价页失败: {e}");
            }
        }
        "open-main-settings" => {
            if let Err(e) =
                crate::main_window::open_main_window_on_main_thread(&app, Some("settings"))
            {
                eprintln!("[zbar-tray-menu] 打开设置页失败: {e}");
            }
        }
        // 悬浮面板：show_tray_menu 弹出前已把可见 panel 收起，这里的
        // toggle 等价于「打开」（无点击位置路径，落到屏幕右上角，同全局
        // 快捷键行为）
        "open-panel" => crate::toggle_panel(&app, None),
        // HUD 停靠条开关：原生菜单 "hud-bar-toggle" 分支的同款调用
        // （toggle_from_tray 内部按新开关落盘并启停数据线程，并广播
        // zbar://hud-bar-state，前端据此同步开关态）
        "hud-toggle" => {
            if let Err(e) = crate::hud_bar::toggle_from_tray(&app) {
                eprintln!("[zbar-tray-menu] 切换停靠条失败: {e}");
                return Err(format!("切换停靠条失败: {e}"));
            }
        }
        // 退出应用：原生菜单 "quit" 分支的同款语义（app.exit(0)）
        "quit" => app.exit(0),
        // Esc / 显式关闭：只收起菜单
        "close" => {}
        _ => {}
    }
    if !keep_open {
        if let Some(win) = app.get_webview_window(TRAY_MENU_WINDOW_LABEL) {
            let _ = win.hide();
        }
    }
    Ok(())
}
