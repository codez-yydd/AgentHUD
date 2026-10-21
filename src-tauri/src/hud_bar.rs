//! HUD 停靠条（hud-bar）：贴屏幕边缘常驻的小条，每 Agent 一个"语义环 +
//! 品牌图标"条目（默认不显示百分比数字——环即比例，数字进悬浮
//! tooltip/详情卡）；悬浮/点击弹出该 Agent 的全部额度窗口；整条可拖拽，
//! 松手自动吸附最近的屏幕边（左/右/顶）并切换横竖布局（交互与视觉参考
//! jazzenchen/agent-hud-open 的 Simple 模式——近纯黑胶囊 + 大语义环 +
//! 品牌图标，仅参考不搬代码）。
//!
//! 一期范围（明确不做）：审批/通知、余额、24h token 柱、会话列表、活动
//! 呼吸动画、多显示器独立配置、底部贴边（避免与任务栏重叠）。
//!
//! 数据口径：与总览页 Agent 表额度列完全一致（余量口径 = 100 − 已用%），
//! 复用同一批 Rust 底层函数，不新造取数链路：
//! - zai：quota::query_quota（注入版 usage_feed 额度线程同款查询通道；
//!   不走 fetch_quota——写历史采样是前端额度命令的职责，停靠条只读）；
//! - codex / claude / kimi：各自 fetch_live_rate_limits（内部已有批次⑤的
//!   成功 60s / 失败 15s 双 TTL + single-flight 缓存，30s 拍节下大概率
//!   命中缓存，不额外新发 HTTP）；kimi 额外带月窗口（monthly_pct 预埋
//!   字段，服务端返回后自动出现）；
//! - opencodego / minimax：provider_quota::query_provider_quota（凭证/
//!   本地登录混合链路；本模块再加 60s 结果缓存限频，与面板 120s 轮询
//!   互不打扰）。窗口挑选与总览页 providerQuotaFrom 同规则：第一个
//!   非错误态且有百分比窗口的条目，全部百分比窗口整表透出。
//! 某源查询失败 → 该源本轮跳过（不阻塞其他源）；全部失败 → 空态占位。
//!
//! 窗口形态与铁律（照搬 session_hud.rs / pet.rs）：
//! - 透明、无边框、置顶、skipTaskbar、shadow 关闭、不可 resize、
//!   focusable(false)+focused(false)（不抢编辑器焦点，鼠标交互不受影响）；
//! - Windows 隐藏建窗 → set_size → show 的钳宽顺序（68 逻辑宽远小于
//!   WM_GETMINMAXINFO 默认最小跟踪宽度，min_inner_size(1,1) 覆盖，
//!   见 pet.rs 的事故记载）；
//! - 建窗/窗口操作必须在主线程事件循环上下文：合法调用点是 setup 阶段
//!   （start_if_enabled）与托盘菜单回调（on_menu_event 在主线程执行，
//!   见 main_window.rs 模块 doc 的建窗铁律）；命令侧统一 async +
//!   run_on_main_thread + channel + 超时等待。
//!
//! 贴边吸附：Moved 事件（lib.rs 按 label 分发）只刷新内存里的最新位置与
//! 时刻；独立吸附线程（与数据线程分离，见 snap_loop——数据拍最坏 ~68s 的
//! 串行 HTTP 不能阻塞吸附）以快循环检测"事件静默 ≥ 300ms"视为拖放结束，
//! 经 run_on_main_thread 投递吸附闭包：**全程物理坐标**——窗口
//! outer_position 是虚拟桌面全局物理坐标，必须先减 current_monitor 的
//! position 换算到本屏局部再做最近边判定（副屏在主屏右侧时全局 x 直接
//! 与本屏宽比较会把 d_right 钳 0 误判、贴右落点把窗口弹回主屏），落点
//! 再加回 mon.position 偏移成全局坐标 set_position；nearest_edge /
//! snap_target 纯函数收本屏局部几何（含跨屏单测）。吸附闭包按最近边
//! set_size 切换横竖排尺寸 + set_position 贴边，{edge,pos} 原子落盘并向前
//! 端推 zbar://hud-bar-layout 通知切布局。程序侧 set_position 的 Moved
//! 回声由 SNAP_POS 位置比对拦截（吸附/扩窗闭包末尾写入，闭包在事件循环
//! 线程同步执行完毕后回声才分发，时序安全）。
//!
//! 详情卡：条窗口厚度只有 68 逻辑 px（贴顶 70），卡片画不下也不能溢出
//! 窗口边界——所以卡片有**自己的窗口**（"hud-card"，透明置顶、不抢焦点、
//! 常驻隐藏预热）。条前端 hover 条目时只 invoke hud_card_open（带 hover
//! 条目中心的物理坐标），Rust 记下锚点通知卡片页；卡片页按自身实测高度
//! 回报尺寸，Rust 算屏幕落点（贴着条的展开面板、纵向上对条目中心、钳进
//! 工作区）后改尺寸+移位+show——**此时卡片内容还是 measuring 隐藏态**，
//! 所以这些窗口操作一次都不会被看见；卡片页确认视口到位后再露出并播
//! 抽屉动画。
//!
//! 为什么不再"扩窗"：卡片在条的外侧，而条贴着屏幕边缘，扩窗必然要 OS
//! 改条窗口的位置与尺寸。WebView2 的渲染子窗口锚定父窗口左上角，父窗口
//! 先变大/移位、渲染进程还没按新视口重排的那一帧，整条会被画到新窗口的
//! 左上角（实机录屏逐帧拍到：条瞬移到屏幕左上方一帧再弹回——这正是
//! "第一次悬浮时卡片闪现移动"）。条窗口全程不动就没有这一帧；卡片窗口
//! 只在自身内容不可见时变几何，同样不会有可见的错位。Rust 侧单方面收卡
//!（吸附强制回位 / 托盘关闭）会广播 zbar://hud-bar-popover {open:false}，
//! 条前端据此收敛本地详情态（防两端状态失步）。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::claude::ClaudeRateLimits;
use crate::codex::CodexRateLimits;
use crate::kimi::KimiRateLimits;
use crate::provider_quota::{ProviderQuotaBalance, ProviderQuotaEntry};
use crate::quota::QuotaResult;

// ============================================================
// 常量与窗口 label / 事件
// ============================================================

/// 停靠条窗口 label（与 capabilities/hud-bar.json 的 windows 白名单、
/// 事件目标一致）
pub const HUD_BAR_WINDOW_LABEL: &str = "hud-bar";
/// 额度数据事件（前端 listen，payload = HudBarUsage，内容变化才推）
pub const HUD_BAR_USAGE_EVENT: &str = "zbar://hud-bar-usage";
/// 布局切换事件（吸附换边后推送，payload = { orientation, edge }）
pub const HUD_BAR_LAYOUT_EVENT: &str = "zbar://hud-bar-layout";
/// 详情态单方面变更通知（Rust 侧吸附强制回位 / 托盘关闭后已隐藏卡片
/// 窗口并广播 { open: false }，条前端据此收敛本地详情态，防两端失步）
pub const HUD_BAR_POPOVER_EVENT: &str = "zbar://hud-bar-popover";

/// 条厚度（逻辑 px）：竖排 = 窗口宽（贴左/右）。几何分解与 hud.html
/// 栅格一一对应：透明屏缘间隙 6×2（#hud-root 的 padding）+ 面板内边距
/// 8×2 + 条目宽 40（环 32 + 条目内边距 4×2）= 68。两种朝向都**不做抽屉
/// 展开**（条上只有环与图标，百分比/金额全进详情卡），所以窗口可以贴着
/// 面板收死，不留透明画布。改任一侧必须同步另一侧，否则出现裁切或空白
const BAR_THICK: f64 = 68.0;
/// 横排（贴顶）窗口高：屏缘间隙 6×2 + 面板内边距 8×2 + 条目高 42 = 70
const BAR_THICK_TOP: f64 = 70.0;
/// 竖排单条目长度（逻辑 px）：条目盒高 42（环 32 上下各 5），与 hud.html
/// .hud-item 尺寸同步
const ITEM_LEN: f64 = 42.0;
/// 单条目宽度（逻辑 px）：只露环 + 内边距（hud.html .hud-item width 40
/// 同步）。横竖排共用同一宽度——没有展开态
const ITEM_W_COLLAPSED: f64 = 40.0;
/// 条目间距（逻辑 px）：hud.html #hud-strip 的 gap 同步
const ITEM_GAP: f64 = 2.0;
/// 条与屏缘的视觉间隙（逻辑 px）：hud.html #hud-root padding 同步；吸附
/// 把窗口贴到屏幕边 x=0/y=0，面板离缘全靠它
const BAR_PAD: f64 = 6.0;
/// 面板内边距（逻辑 px）：hud.html #hud-strip padding 同步
const PANEL_PAD: f64 = 8.0;
/// 条长度下限（逻辑 px）：单条目时也保持该长度
const BAR_MIN_LEN: f64 = 96.0;
/// 详情卡窗口 label（独立窗口，见 hud_card 段）与事件名
pub const HUD_CARD_WINDOW_LABEL: &str = "hud-card";
/// 打开一张卡（payload = { source, edge, animate }）
pub const HUD_CARD_OPEN_EVENT: &str = "zbar://hud-card-open";
/// 收回一张卡（payload = { animate }）
pub const HUD_CARD_CLOSE_EVENT: &str = "zbar://hud-card-close";
/// 窗口已按卡片尺寸落位（前端据此确认视口到位再露出卡片）
pub const HUD_CARD_PLACED_EVENT: &str = "zbar://hud-card-placed";
/// 详情卡宽度（逻辑 px）：hud.html .hud-card width 同步
const CARD_W: f64 = 304.0;
/// 卡片窗口四周留白（逻辑 px）：只服务两件事——① 卡片投影必须落在窗口
/// 内（hud.html .hud-card 的 blur+offset 之和 ≤ 本值，否则阴影被窗口边缘
/// 硬裁成一条可见的矩形描边）；② 抽屉入场位移不被裁切。**值越小越好**：
/// 这圈透明留白会连同背景一起被看见（就是"卡片四周多出来的透明边框"），
/// 还会吃掉鼠标事件——16 时肉眼已能明显看到一圈，收到 8
const CARD_PAD: f64 = 8.0;
/// 卡片与条之间的间隙（逻辑 px）：hud.html #hud-pop 侧留白一致
const CARD_GAP: f64 = 8.0;
/// 面板宽度（逻辑 px）：条目 40 + 面板内边距 8×2。卡片横向位置以面板
/// 边缘为基准（卡片挂在条外侧）
const PANEL_W: f64 = ITEM_W_COLLAPSED + PANEL_PAD * 2.0;
/// 窗口操作（建窗/吸附/拖拽/卡片落位）的等待超时：仅事件循环异常时兜底
const HUD_BAR_OP_TIMEOUT: Duration = Duration::from_secs(8);
/// 数据拍周期（毫秒）：各源额度组装与推送节拍（zai/provider 查询另有
/// 60s 结果缓存，实际 HTTP 频率 ≤ 60s 一次）
const FEED_TICK_MS: u64 = 30_000;
/// 快循环步长（毫秒）：吸附静默检测与强制推送的响应粒度（分段睡眠可
/// 及时感知 stop）
const FAST_TICK_MS: u64 = 200;
/// 拖放结束判定（毫秒）：Moved 事件静默超过该时长视为松手，触发贴边吸附
const SNAP_IDLE_MS: u64 = 300;
/// 卡片悬浮守护的离开宽限（毫秒）：真实光标离开条/卡超过该时长才由 Rust
/// 兜底收卡。取 500 = 条页判定 220ms + 卡片退出动画 150ms 之后再留余量：
/// 正常路径（条页收起 + 动画 + hud_card_hide）先走完，CARD_VISIBLE 已置
/// false，这里的兜底根本不会触发；只有条页轮询停摆、卡片变成孤儿时才生效。
const CARD_CLOSE_GRACE_MS: u64 = 500;
/// zai / provider 额度查询的结果缓存时长（成功与失败同 TTL：失败也短暂
/// 缓存防 30s 拍重试风暴；codex/claude/kimi 用各自模块内建的双 TTL 缓存）
const QUERY_CACHE_TTL: Duration = Duration::from_secs(60);

// ============================================================
// 停靠边（纯数据，供吸附判定与配置序列化）
// ============================================================

/// 停靠边。底部不做（任务栏区域，一期明确排除）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarEdge {
    Left,
    Right,
    Top,
}

impl BarEdge {
    /// 配置文件里的序列化键（小写字符串）
    fn key(self) -> &'static str {
        match self {
            BarEdge::Left => "left",
            BarEdge::Right => "right",
            BarEdge::Top => "top",
        }
    }

    /// 配置字符串解析（非法值回 None，由 clamped 回退 Left）
    fn parse(s: &str) -> Option<Self> {
        match s {
            "left" => Some(BarEdge::Left),
            "right" => Some(BarEdge::Right),
            "top" => Some(BarEdge::Top),
            _ => None,
        }
    }

    /// 前端布局方向：左右 = 竖排、顶 = 横排
    fn orientation(self) -> &'static str {
        match self {
            BarEdge::Left | BarEdge::Right => "vertical",
            BarEdge::Top => "horizontal",
        }
    }

    /// AtomicUsize 存取用的小整数编码
    fn index(self) -> usize {
        match self {
            BarEdge::Left => 0,
            BarEdge::Right => 1,
            BarEdge::Top => 2,
        }
    }

    fn from_index(i: usize) -> Self {
        match i {
            1 => BarEdge::Right,
            2 => BarEdge::Top,
            _ => BarEdge::Left,
        }
    }
}

/// 条窗口尺寸（宽, 高）：竖排 = 68 × 条长，横排 = 条长 ×
/// 70。条长 = 两端留白 + 面板内边距 + 条目数 × 条目长 + 条目间距，下限
/// BAR_MIN_LEN（单条目也给悬浮卡留纵向空间）。返回**逻辑 px**（建窗
/// builder.position / inner_size 走逻辑坐标）。吸附/拖拽闭包与建窗共
/// 包与建窗共用。
fn bar_metrics(edge: BarEdge, items: usize) -> (f64, f64) {
    let n = items.max(1) as f64;
    let gaps = (n - 1.0).max(0.0) * ITEM_GAP;
    match edge {
        BarEdge::Left | BarEdge::Right => {
            let len = (BAR_PAD * 2.0 + PANEL_PAD * 2.0 + n * ITEM_LEN + gaps)
                .max(BAR_MIN_LEN);
            (BAR_THICK, len)
        }
        BarEdge::Top => {
            // 条目只有 40 宽（两种朝向都不出抽屉，数字全进详情卡）：
            // 窗口必须与可见面板等宽，否则条两侧挂着一圈透明画布
            // （挡点击，拖拽时指针与条目也对不上）
            let len = (BAR_PAD * 2.0 + PANEL_PAD * 2.0 + n * ITEM_W_COLLAPSED + gaps)
                .max(BAR_MIN_LEN);
            (len, BAR_THICK_TOP)
        }
    }
}

/// 收缩态窗口尺寸的物理像素版（吸附/拖拽闭包的 set_size 走物理坐标，
/// 与 outer_size 同口径可直接比对）
fn bar_metrics_physical(edge: BarEdge, items: usize, scale: f64) -> (f64, f64) {
    let (w, h) = bar_metrics(edge, items);
    (w * scale, h * scale)
}

/// 卡片在屏幕上的落点（**物理 px，全局虚拟桌面坐标**）：纯函数，便于单测。
///
/// 基准是**条的窗口矩形**（不是条目/面板）：竖排贴着屏幕左右缘，卡片挂在
/// 窗口外侧、与展开态面板留 CARD_GAP；横排贴在下方、水平跟随 hover 条目
/// 的中心。纵向竖排对条目中心，横排挂在条下方（与旧的前端 placeCard 同一
/// 套观感）。最后钳进工作区（4px 安全边距），保证不越出屏幕（含任务栏）。
#[allow(clippy::too_many_arguments)]
fn card_placement(
    edge: BarEdge,
    bar: (f64, f64, f64, f64),
    anchor_center_x: f64,
    anchor_center_y: f64,
    card_w: f64,
    card_h: f64,
    area: (f64, f64, f64, f64),
    scale: f64,
) -> (f64, f64) {
    let (bx, by, _bw, _bh) = bar;
    let gap = CARD_GAP * scale;
    let x = match edge {
        // 贴右：面板钉在窗口右缘，卡片在面板左侧（窗口整体左侧延展）
        BarEdge::Right => bx + (BAR_THICK - BAR_PAD - PANEL_W) * scale - gap - card_w,
        // 贴左：卡片在面板右侧
        BarEdge::Left => bx + (BAR_PAD + PANEL_W) * scale + gap,
        // 贴顶：水平跟随条目中心
        BarEdge::Top => anchor_center_x - card_w / 2.0,
    };
    let y = match edge {
        BarEdge::Left | BarEdge::Right => anchor_center_y - card_h / 2.0,
        BarEdge::Top => by + (BAR_THICK_TOP - BAR_PAD) * scale + gap,
    };
    let (ax, ay, aw, ah) = area;
    let margin = 4.0 * scale;
    let x = x.clamp(ax + margin, (ax + aw - card_w - margin).max(ax + margin));
    let y = y.clamp(ay + margin, (ay + ah - card_h - margin).max(ay + margin));
    (x, y)
}

/// 拖拽中的停靠边判定（逻辑 px）：与 agent-hud-open 的 dragPlacement 同一个
/// 思路——**拖动过程中条始终停在某条边上**，边跟着指针走，而不是"松手才
/// 吸附"。但那边有四条边、top/bottom 天然把中间分掉；这里只有左/右/顶，
/// 直接用"到各边距离取最近"会让顶边吃掉大半个屏幕，所以改成：
/// - 指针进顶部吸附带（工作区顶部 ~14%，56–150px，并留 40px 迟滞）→ 顶；
/// - 否则按水平中线分左右（带 45%/55% 迟滞，中线附近来回移动不抖）。
fn drag_edge(
    pointer: (f64, f64),
    area: (f64, f64, f64, f64),
    current: BarEdge,
) -> BarEdge {
    let (ax, ay, aw, ah) = area;
    let top_zone = (ah * 0.14).clamp(56.0, 150.0);
    // 迟滞：已经在顶部时多留 40px 才离开，避免贴边抖动
    let top_limit = ay + if current == BarEdge::Top { top_zone + 40.0 } else { top_zone };
    if pointer.1 <= top_limit {
        return BarEdge::Top;
    }
    match current {
        BarEdge::Left => {
            if pointer.0 > ax + aw * 0.55 {
                BarEdge::Right
            } else {
                BarEdge::Left
            }
        }
        BarEdge::Right => {
            if pointer.0 < ax + aw * 0.45 {
                BarEdge::Left
            } else {
                BarEdge::Right
            }
        }
        // 从横排下来：按中线决定落左还是落右
        BarEdge::Top => {
            if pointer.0 < ax + aw * 0.5 {
                BarEdge::Left
            } else {
                BarEdge::Right
            }
        }
    }
}

/// 拖拽落点几何（**物理 px**，屏幕坐标）：每一步都吸在 `drag_edge` 选出的
/// 边上（条永远贴边，不存在"浮在屏幕中间"的中间态），沿边方向按
/// `grab_frac`（按下时指针在条上的比例位置）停放，指针因此始终压在同一
/// 条目上。换边时按该边重算横竖排尺寸（`scale` 为指针所在屏的缩放比，
/// 条厚/条目宽等逻辑常量在此换算）。
///
/// 全程物理像素是跨屏拖拽的关键：多显示器下各屏的 work_area / 缩放比不同，
/// 用逻辑坐标混算会把窗口落回主屏或错位。
fn drag_geometry(
    edge: BarEdge,
    items: usize,
    pointer: (f64, f64),
    grab_frac: f64,
    area: (f64, f64, f64, f64),
    scale: f64,
) -> (BarEdge, f64, f64, f64, f64) {
    let (ax, ay, aw, ah) = area;
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let next = drag_edge(pointer, area, edge);
    let (lw, lh) = bar_metrics(next, items);
    let (nw, nh) = (lw * scale, lh * scale);
    let new_run = if next == BarEdge::Top { nw } else { nh };
    let area_len = if next == BarEdge::Top { aw } else { ah };
    let pointer_along = if next == BarEdge::Top {
        pointer.0 - ax
    } else {
        pointer.1 - ay
    };
    let frac = grab_frac.clamp(0.0, 1.0);
    let along = (pointer_along - frac * new_run).clamp(0.0, (area_len - new_run).max(0.0));
    let x = match next {
        BarEdge::Left => ax,
        BarEdge::Right => ax + aw - nw,
        BarEdge::Top => ax + along,
    };
    let y = if next == BarEdge::Top {
        ay
    } else {
        ay + along
    };
    (next, x, y, nw, nh)
}

/// 吸附目标：窗口外框中心到左/右/顶三边取最近（底部不参与，避免任务栏；
/// 距离下限 0，容忍拖出屏幕一半的拖放）。**全部为本屏局部物理像素**
///（调用方已把全局坐标减去 mon.position()；屏幕高度不参与判定——底边
/// 不吸，垂直距离只需 cy 到顶），保留 mon_h 参数与 snap_target 的几何
/// 签名对称。
fn nearest_edge(mon_w: f64, _mon_h: f64, cx: f64, cy: f64) -> BarEdge {
    let d_left = cx.max(0.0);
    let d_right = (mon_w - cx).max(0.0);
    let d_top = cy.max(0.0);
    // 依序比较取最小；并列时左 > 右 > 顶（用户把条丢在屏幕正中时优先贴左）
    let mut best = BarEdge::Left;
    let mut best_d = d_left;
    if d_right < best_d {
        best = BarEdge::Right;
        best_d = d_right;
    }
    if d_top < best_d {
        best = BarEdge::Top;
    }
    best
}

/// 吸附落点：按边切换横竖排尺寸并贴边（左/右 x 贴 0 / 屏宽−条厚，顶 y 贴
/// 0）；非贴边轴保持拖放落点（垂直方向不强制居中，用户拖到哪就贴哪）。
/// **全部为物理像素、cur 与返回值均为"本屏局部"坐标**——调用方把窗口
/// 全局坐标减 mon.position() 后传入，拿到落点再加回 mon.position() 得
/// 全局坐标（多显示器下全局坐标与本屏分辨率直接运算会把窗口弹回主屏）。
/// 条厚等逻辑常量经 scale 换算成物理。mon_h 当前不参与（底部不做、垂直
/// 轴保持落点），保留参数与 nearest_edge 对称，便于后续接多显示器几何。
#[allow(clippy::too_many_arguments)]
fn snap_target(
    edge: BarEdge,
    mon_w: f64,
    _mon_h: f64,
    cur_x: f64,
    cur_y: f64,
    items: usize,
    scale: f64,
) -> (f64, f64, f64, f64) {
    let (w, h) = bar_metrics_physical(edge, items, scale);
    match edge {
        BarEdge::Left => (w, h, 0.0, cur_y),
        BarEdge::Right => (w, h, (mon_w - BAR_THICK * scale).max(0.0), cur_y),
        BarEdge::Top => (w, h, cur_x, 0.0),
    }
}

// ============================================================
// 配置（~/.agenthud/hud-bar.json，结构体 + 原子写，照 session_hud 范式）
// ============================================================

/// 停靠条配置。enabled 默认 false（首开零开销）；edge 默认贴左；pos 为
/// 收缩态吸附落点（逻辑坐标）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HudBarConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_edge")]
    pub edge: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<(f64, f64)>,
}

fn default_edge() -> String {
    BarEdge::Left.key().to_string()
}

impl Default for HudBarConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            edge: default_edge(),
            pos: None,
        }
    }
}

impl HudBarConfig {
    /// 脏值收敛：edge 非法回 left；pos 非有限值丢弃
    fn clamped(mut self) -> Self {
        if BarEdge::parse(&self.edge).is_none() {
            self.edge = default_edge();
        }
        if let Some((x, y)) = self.pos {
            if !x.is_finite() || !y.is_finite() {
                self.pos = None;
            }
        }
        self
    }

    fn edge_of(&self) -> BarEdge {
        BarEdge::parse(&self.edge).unwrap_or(BarEdge::Left)
    }
}

fn hud_bar_config_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join("hud-bar.json"))
}

/// 读取停靠条配置；文件不存在或损坏时静默返回默认值（首开无文件）。
pub fn load_hud_bar_config() -> HudBarConfig {
    let Ok(path) = hud_bar_config_path() else {
        return HudBarConfig::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<HudBarConfig>(&text).ok())
        .unwrap_or_default()
        .clamped()
}

/// 保存停靠条配置：clamp 后原子写（先写临时文件再改名，与 session_hud
/// 同口径）。
pub fn save_hud_bar_config(config: &HudBarConfig) -> Result<(), String> {
    let path = hud_bar_config_path()?;
    let json = serde_json::to_string_pretty(&config.clone().clamped())
        .map_err(|e| format!("序列化停靠条配置失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("写入停靠条配置失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("保存停靠条配置失败: {e}"))
}

// ============================================================
// 额度 payload（口径 = 总览页 Agent 表额度列：余量 = 100 − 已用%）
// ============================================================

/// 单个额度窗口（key 为稳定窗口键：hour5 / weekly / monthly / provider
/// 原生 key 如 minimax 的 interval；label 双语由后端给出，前端按语言取）
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudBarQuota {
    pub key: String,
    pub label_zh: String,
    pub label_en: String,
    /// 已用百分比 0-100（环与悬浮行都按 100 − used 展示余量）
    pub used_pct: f64,
    /// 下次重置时刻（毫秒）；无则 None（前端不显示倒计时）
    pub reset_at: Option<i64>,
}

/// 单个 Agent 条目（source 与前端 BrandIcon 键一致：zai/codex/claude/
/// kimi/dsh/opencodego/minimax/cursor）
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudBarAgent {
    pub source: String,
    /// 套餐/订阅名（zai "max"、codex "plus"、provider "Pro" 等）；None =
    /// 未知或该来源不提供。详情卡在 Agent 名旁展示为档位徽标。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// 数据状态：None = 实时额度正常；"unavailable" = 该 Agent 已开启但
    /// 本轮没取到额度（未登录/凭证过期/网络失败），前端渲染成弱化的占位
    /// 条目；"usage" = 没有订阅额度证据、但今日有用量（`today_usage` 快照
    /// 里有 token），图标按用量动态保留，详情卡说明"无订阅、仅按用量显示"。
    /// 两种占位都不随查询失败/跨日忽隐忽现：是否出现由 fill_placeholders
    /// 的准入证据决定（见其注释）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub quotas: Vec<HudBarQuota>,
    /// 按量计费余额（当前只有 dsh = DeepSeek 开放平台余额：DSH 是纯本地
    /// 采集型 agent，没有 5h/周等订阅窗口，唯一可展示的"额度"就是官方
    /// 余额）。有余额时 quotas 恒空，前端把条目渲染成余额型（环内不画
    /// 进度弧，展开位显示金额而不是百分比）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<ProviderQuotaBalance>,
}

/// 套餐字符串收敛：去空白，空串视为未知（避免详情卡出现空徽标）。
fn plan_of(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// 追加一个条目（无窗口数据不出现，与"查询失败跳过该源"口径一致）。
fn push_agent(
    agents: &mut Vec<HudBarAgent>,
    source: &str,
    plan: Option<String>,
    quotas: Vec<HudBarQuota>,
) {
    if quotas.is_empty() {
        return;
    }
    agents.push(HudBarAgent {
        source: source.to_string(),
        plan: plan.filter(|value| !value.trim().is_empty()),
        status: None,
        quotas,
        balance: None,
    });
}

/// DeepSeek 平台余额 → HUD 条目（source="dsh"）：DeepSeek Harness 自身
/// 没有订阅额度窗口，它的"额度"就是开放平台余额（凭证通道 provider id
/// 仍是 deepseek）。取第一条 ok 且带余额的凭证条目；没有凭证/查询失败
/// 返回 None，由 fill_placeholders 按「订阅/今日用量」证据补占位。
fn dsh_agent(entries: &[ProviderQuotaEntry]) -> Option<HudBarAgent> {
    let entry = entries
        .iter()
        .find(|e| e.status == "ok" && e.balance.is_some())?;
    Some(HudBarAgent {
        source: "dsh".into(),
        plan: plan_of(entry.plan_name.as_deref()),
        status: None,
        quotas: Vec::new(),
        balance: entry.balance.clone(),
    })
}

/// HUD 固定展示顺序（也是设置页 agent 顺序）：实时额度源与"已开启但
/// 暂无数据"的占位条目都按这个顺序排，避免占位全被追加到末尾。
const HUD_SOURCES: &[&str] = &[
    "zai",
    "codex",
    "claude",
    "cursor",
    "kimi",
    "dsh",
    "opencodego",
    "minimax",
];

/// 占位条目的准入证据（纯数据，单测不必碰真实设置文件/系统时钟）。
///
/// 规则（用户口径）：**既没有订阅额度、今日也没有用量**的 agent 不出现在
/// 停靠条上；有订阅（本会话取到过该来源的额度/余额）或今日有 token 用量
/// 任一条成立，条目就动态加回来。
///
/// - `visible`：设置页开启的来源（Rust 侧镜像，见 agent_visibility）；
/// - `subscribed`：本会话内取到过额度/余额的来源（**粘滞**——额度查询偶发
///   失败/网络抖动时，已确认存在的订阅不该忽隐忽现）；
/// - `today`：今日各来源 token（`today_usage` 快照，已含其他设备）。`None`
///   = 快照不可用（托盘线程还没跑完/已停摆）：此时**保守放行**，退回旧行为
///   （设置里开启就出占位），绝不把「不知道」当成「没用量」把图标藏掉。
struct PlaceholderEvidence<'a> {
    visible: &'a BTreeSet<String>,
    subscribed: &'a HashSet<String>,
    today: Option<&'a crate::today_usage::TodayUsage>,
}

impl PlaceholderEvidence<'_> {
    /// 该来源有没有"该出现在条上"的证据（不含今日用量/订阅的展示语义）。
    fn allows(&self, source: &str) -> bool {
        if !self.visible.contains(source) {
            return false;
        }
        if self.subscribed.contains(source) {
            return true;
        }
        match self.today {
            Some(today) => today.used(source),
            None => true, // 今日用量未知：保守放行（见结构体注释）
        }
    }

    /// 占位条目的状态：有订阅证据（或用量未知）= `unavailable`（额度暂不可
    /// 用）；只有用量证据 = `usage`（没有订阅额度，靠今日用量保留在条上）。
    fn status(&self, source: &str) -> &'static str {
        if self.subscribed.contains(source) || self.today.is_none() {
            "unavailable"
        } else {
            "usage"
        }
    }
}

/// 给"该出现但本轮没有额度数据"的来源补占位条目，并按固定顺序排序。
///
/// 占位条目 `quotas` 为空、`status` 见 [`PlaceholderEvidence::status`]：
/// 前端据此渲染弱化图标与「—」，悬浮卡说明是"额度未连接"还是"无订阅、
/// 仅按今日用量保留"。没有订阅也没有用量的来源直接不出现（不再常驻）。
fn fill_placeholders(agents: &mut Vec<HudBarAgent>, evidence: &PlaceholderEvidence<'_>) {
    for source in HUD_SOURCES {
        if agents.iter().any(|agent| agent.source == *source) {
            continue;
        }
        if !evidence.allows(source) {
            continue;
        }
        agents.push(HudBarAgent {
            source: (*source).to_string(),
            plan: None,
            status: Some(evidence.status(source).to_string()),
            quotas: Vec::new(),
            balance: None,
        });
    }
    agents.sort_by_key(|agent| {
        HUD_SOURCES
            .iter()
            .position(|source| *source == agent.source)
            .unwrap_or(usize::MAX)
    });
}

/// 数据事件 payload（内容变化才 emit；倒计时由前端基于 reset_at 本地计算）
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudBarUsage {
    pub ts: i64,
    pub agents: Vec<HudBarAgent>,
}

/// 窗口键 → 双语短标签（行形与注入版 V32 定稿一致："5h 65% 3h02m" /
/// "每周 93% 6d21h"；minimax 的 interval 即 5 小时会话窗口，同 5h 文案）。
/// 未识别 key 返回 None（provider 未来新增窗口原样透出 key，不丢数据）。
fn quota_label(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        "hour5" | "interval" => Some(("5h", "5h")),
        "weekly" => Some(("每周", "wk")),
        "monthly" => Some(("每月", "mo")),
        "cursor_auto" => Some(("Auto", "Auto")),
        "cursor_api" => Some(("API", "API")),
        _ => None,
    }
}

/// 已用百分比 → 窗口条目（百分比非法/缺失返回 None，该窗口不入列表——
/// 与总览页"取不到则单元格 --"的缺失语义一致：停靠条直接不显示该窗口）
fn quota_window(key: &str, used_pct: Option<f64>, reset_at: Option<i64>) -> Option<HudBarQuota> {
    let used = used_pct?;
    if !used.is_finite() {
        return None;
    }
    let (label_zh, label_en) = match quota_label(key) {
        Some((zh, en)) => (zh.to_string(), en.to_string()),
        None => (key.to_string(), key.to_string()),
    };
    Some(HudBarQuota {
        key: key.to_string(),
        label_zh,
        label_en,
        used_pct: used,
        reset_at,
    })
}

/// zai 查询结果 → 窗口列表（hour5 + weekly；MCP 月度是次数型额度、无剩余
/// 百分比语义，总览页 zai 的 QuotaInfo 同样只取 weekly + hour5，口径一致）
fn zai_windows(result: &QuotaResult) -> Vec<HudBarQuota> {
    let mut out = Vec::new();
    if let Some(w) = quota_window(
        "hour5",
        result.hour5.as_ref().map(|h| h.percentage as f64),
        result.hour5.as_ref().and_then(|h| h.next_reset_time),
    ) {
        out.push(w);
    }
    if let Some(w) = quota_window(
        "weekly",
        result.weekly.as_ref().map(|v| v.percentage as f64),
        result.weekly.as_ref().and_then(|v| v.next_reset_time),
    ) {
        out.push(w);
    }
    out
}

/// codex / claude / kimi 的实时额度结构 → 窗口列表。三个结构字段口径一致
/// （primary = 5h、secondary = 周；codex 的 to_rate_limits 已把 primary/
/// secondary 归一到 5h/weekly 语义），kimi 额外带预埋月窗口。
fn rate_limit_windows(
    primary: Option<f64>,
    primary_reset: Option<i64>,
    secondary: Option<f64>,
    secondary_reset: Option<i64>,
    monthly: Option<(f64, Option<i64>)>,
) -> Vec<HudBarQuota> {
    let mut out = Vec::new();
    if let Some(w) = quota_window("hour5", primary, primary_reset) {
        out.push(w);
    }
    if let Some(w) = quota_window("weekly", secondary, secondary_reset) {
        out.push(w);
    }
    if let Some((pct, reset)) = monthly {
        if let Some(w) = quota_window("monthly", Some(pct), reset) {
            out.push(w);
        }
    }
    out
}

/// provider 额度条目里挑展示条目（opencodego / minimax 共用）：与总览页
/// providerQuotaFrom 同规则——第一个非 error/expired 且带百分比窗口的条目。
fn provider_entry(entries: &[ProviderQuotaEntry]) -> Option<&ProviderQuotaEntry> {
    entries.iter().find(|e| {
        e.status != "error"
            && e.status != "expired"
            && e.windows
                .iter()
                .any(|w| w.used_percent.is_some_and(|p| p.is_finite()))
    })
}

/// 展示条目的全部百分比窗口（key 原样，含 minimax 的 interval）。
fn provider_entry_windows(entry: &ProviderQuotaEntry) -> Vec<HudBarQuota> {
    entry
        .windows
        .iter()
        .filter_map(|w| quota_window(&w.key, w.used_percent, w.resets_at))
        .collect()
}

/// provider 额度条目 → 窗口列表（单测用的薄封装）。
#[cfg(test)]
fn provider_windows(entries: &[ProviderQuotaEntry]) -> Option<Vec<HudBarQuota>> {
    Some(provider_entry_windows(provider_entry(entries)?))
}

/// 组装一拍数据：只含有窗口数据的源（查询失败/无额度数据的源本轮直接
/// 不出现，不阻塞其他源）。ts 由调用方取当下时间。
#[allow(clippy::too_many_arguments)]
fn build_usage(
    zai: Option<&QuotaResult>,
    codex: Option<&CodexRateLimits>,
    claude: Option<&ClaudeRateLimits>,
    kimi: Option<&KimiRateLimits>,
    opencodego: Option<&[ProviderQuotaEntry]>,
    minimax: Option<&[ProviderQuotaEntry]>,
    dsh: Option<&[ProviderQuotaEntry]>,
) -> Vec<HudBarAgent> {
    let mut agents = Vec::new();
    if let Some(result) = zai {
        push_agent(
            &mut agents,
            "zai",
            plan_of(Some(result.level.as_str())),
            zai_windows(result),
        );
    }
    if let Some(limits) = codex {
        push_agent(
            &mut agents,
            "codex",
            plan_of(limits.plan_type.as_deref()),
            rate_limit_windows(
                limits.primary_pct,
                limits.primary_reset_at,
                limits.secondary_pct,
                limits.secondary_reset_at,
                None,
            ),
        );
    }
    if let Some(limits) = claude {
        push_agent(
            &mut agents,
            "claude",
            plan_of(limits.plan_type.as_deref()),
            rate_limit_windows(
                limits.primary_pct,
                limits.primary_reset_at,
                limits.secondary_pct,
                limits.secondary_reset_at,
                None,
            ),
        );
    }
    if let Some(limits) = kimi {
        push_agent(
            &mut agents,
            "kimi",
            plan_of(limits.plan_type.as_deref()),
            rate_limit_windows(
                limits.primary_pct,
                limits.primary_reset_at,
                limits.secondary_pct,
                limits.secondary_reset_at,
                // 月窗口已用百分比存在即显示；重置时刻可选（服务端预埋字段
                // 可能只有百分比）
                limits.monthly_pct.map(|p| (p, limits.monthly_reset_at)),
            ),
        );
    }
    for (source, entries) in [("opencodego", opencodego), ("minimax", minimax)] {
        if let Some(entry) = entries.and_then(provider_entry) {
            push_agent(
                &mut agents,
                source,
                entry.plan_name.clone(),
                provider_entry_windows(entry),
            );
        }
    }
    // dsh 走余额口径（不是百分比窗口），单独组装：只含官方余额 + 套餐名
    if let Some(agent) = dsh.and_then(dsh_agent) {
        agents.push(agent);
    }
    agents
}

/// Cursor 订阅额度（Auto / API 两窗口）：额度线程与 HUD 数据线程共用。
/// Cursor 没有 5h/周窗口，只有套餐 Auto/API 百分比；billing_cycle_end 是
/// RFC3339 字符串，这里转成毫秒时间戳供注入条/HUD 倒计时。未登录/无套餐
/// 返回 None（该 Agent 本轮不出现）。
pub(crate) fn cursor_agent() -> Option<HudBarAgent> {
    let now = chrono::Utc::now().timestamp_millis();
    let from = now - 30 * 24 * 3600 * 1000;
    let snapshot = crate::cursor::fetch_cursor_snapshot(from, now).ok()?;
    let plan = snapshot.plan.as_ref()?;
    let reset = snapshot
        .billing_cycle_end
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis());
    let mut quotas = Vec::new();
    if let Some(window) = quota_window("cursor_auto", plan.auto_pct, reset) {
        quotas.push(window);
    }
    if let Some(window) = quota_window("cursor_api", plan.api_pct, reset) {
        quotas.push(window);
    }
    if quotas.is_empty() {
        None
    } else {
        Some(HudBarAgent {
            source: "cursor".into(),
            plan: plan_of(snapshot.membership_type.as_deref()),
            status: None,
            quotas,
            balance: None,
        })
    }
}

/// 只返回设置里开启的 Cursor 条目（注入额度线程用）。
pub(crate) fn enabled_cursor_agent() -> Option<HudBarAgent> {
    if !crate::agent_visibility::is_visible("cursor") {
        return None;
    }
    cursor_agent()
}

// ============================================================
// 运行期共享状态（普通原子/锁，沿用 session_hud 的全局槽模式）
// ============================================================

/// 最后一次 Moved 事件的逻辑位置（吸附与扩窗都消费；吸附闭包在主线程
/// 读取窗口实际几何，此槽主要服务调试与兜底）
static LAST_POS: OnceLock<Mutex<Option<(f64, f64)>>> = OnceLock::new();
/// 最后一次 Moved 事件时刻（毫秒，0 = 从未）：吸附守护的静默判定锚点
static LAST_MOVED_AT: AtomicU64 = AtomicU64::new(0);
/// 程序侧（吸附/扩窗/缩窗）落定的逻辑位置：随后到达的同位置 Moved 事件
/// 是回声，拦截不触发吸附（容差 1px）
static SNAP_POS: OnceLock<Mutex<Option<(f64, f64)>>> = OnceLock::new();
/// 当前停靠边（AtomicUsize 存 BarEdge::index）：吸附闭包更新，扩窗与
/// 尺寸自适应读取
static CURRENT_EDGE: AtomicUsize = AtomicUsize::new(0);
/// 当前条目数（含空态占位 1）：数据线程更新，建窗/吸附/扩窗算尺寸用
static ITEM_COUNT: AtomicUsize = AtomicUsize::new(1);
/// 详情卡窗口当前是否可见（true = 已按卡片尺寸落位并 show）
static CARD_VISIBLE: AtomicBool = AtomicBool::new(false);
/// 本次卡片请求的锚点（物理 px：hover 条目中心的 x/y）与来源，供
/// hud_card_measure 落位时用
static CARD_ANCHOR: OnceLock<Mutex<Option<CardAnchor>>> = OnceLock::new();
/// 卡片窗口当前占用的屏幕矩形（物理 px）：前端悬浮守护据此判断"指针还在
/// 卡上"，Rust 侧关窗时清空
static CARD_RECT: OnceLock<Mutex<Option<HudCardRect>>> = OnceLock::new();

/// 卡片锚点（物理 px）：竖排用 center_y、横排用 center_x
#[derive(Debug, Clone, Copy)]
struct CardAnchor {
    center_x: f64,
    center_y: f64,
}

/// 卡片屏幕矩形（物理 px，全局虚拟桌面坐标）
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudCardRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

fn card_anchor_slot() -> &'static Mutex<Option<CardAnchor>> {
    CARD_ANCHOR.get_or_init(|| Mutex::new(None))
}

fn card_rect_slot() -> &'static Mutex<Option<HudCardRect>> {
    CARD_RECT.get_or_init(|| Mutex::new(None))
}
/// 强制下一拍立即查询并推送（托盘开启后 0.2s 内出首帧数据）
static FORCE_PUSH: AtomicBool = AtomicBool::new(false);
/// 自定义拖拽进行中：Moved 挂点与吸附守护全部让位（拖拽位置由前端
/// 16ms 一拍驱动，程序侧 set_position 的 Moved 回声不能触发吸附）
static DRAGGING: AtomicBool = AtomicBool::new(false);
/// 拖拽会话（begin 登记抓取偏移与抓取比例；move/end 只读）
static DRAG_SESSION: OnceLock<Mutex<Option<HudDragSession>>> = OnceLock::new();

/// 拖拽会话：指针在条长方向的比例位置（换边换横竖排时保持同一条目停在
/// 指针下）
#[derive(Debug, Clone, Copy)]
struct HudDragSession {
    frac: f64,
}

fn drag_session_slot() -> &'static Mutex<Option<HudDragSession>> {
    DRAG_SESSION.get_or_init(|| Mutex::new(None))
}

fn last_pos_slot() -> &'static Mutex<Option<(f64, f64)>> {
    LAST_POS.get_or_init(|| Mutex::new(None))
}

fn snap_pos_slot() -> &'static Mutex<Option<(f64, f64)>> {
    SNAP_POS.get_or_init(|| Mutex::new(None))
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn current_edge() -> BarEdge {
    BarEdge::from_index(CURRENT_EDGE.load(Ordering::Relaxed))
}

fn item_count() -> usize {
    ITEM_COUNT.load(Ordering::Relaxed).max(1)
}

/// Moved 事件位置是否为程序侧落点的回声（容差 1px）
fn is_snap_echo(pos: (f64, f64)) -> bool {
    snap_pos_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_some_and(|s| (s.0 - pos.0).abs() < 1.0 && (s.1 - pos.1).abs() < 1.0)
}

// ============================================================
// 数据线程（普通 thread + flag 模式，沿用 session_hud 惯例）
// ============================================================

static FEED_STOP: AtomicBool = AtomicBool::new(false);
static FEED_HANDLE: OnceLock<Mutex<Option<thread::JoinHandle<()>>>> = OnceLock::new();
static SNAP_HANDLE: OnceLock<Mutex<Option<thread::JoinHandle<()>>>> = OnceLock::new();

fn feed_handle() -> &'static Mutex<Option<thread::JoinHandle<()>>> {
    FEED_HANDLE.get_or_init(|| Mutex::new(None))
}

fn snap_handle() -> &'static Mutex<Option<thread::JoinHandle<()>>> {
    SNAP_HANDLE.get_or_init(|| Mutex::new(None))
}

/// 启动数据线程与吸附守护线程（托盘开启/启动恢复挂点调用）。已在运行时
/// 幂等 no-op（各自独立判定）。
pub fn start_feed(app: AppHandle) {
    // 吸附线程投递主线程闭包用的 AppHandle 槽（先于两个线程存入）
    if let Ok(mut slot) = snap_app_slot().lock() {
        *slot = Some(app.clone());
    }
    // 数据线程
    let mut guard = match feed_handle().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !guard.as_ref().is_some_and(|h| !h.is_finished()) {
        if let Ok(h) = thread::Builder::new()
            .name("zbar-hud-bar-feed".into())
            .spawn(move || feed_loop(app))
        {
            *guard = Some(h);
        }
    }
    // 吸附守护线程（独立于数据线程：数据拍最坏 ~68s 的串行 HTTP 不能
    // 阻塞 300ms 静默检测）
    drop(guard);
    let mut snap_guard = match snap_handle().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !snap_guard.as_ref().is_some_and(|h| !h.is_finished()) {
        if let Ok(h) = thread::Builder::new()
            .name("zbar-hud-bar-snap".into())
            .spawn(snap_loop)
        {
            *snap_guard = Some(h);
        }
    }
}

/// 停止数据线程与吸附守护线程（托盘关闭挂点，两线程共用停止标志）。仅
/// 置位标记，线程完成当前查询周期/检测拍后退出。
pub fn stop_feed() {
    FEED_STOP.store(true, Ordering::Relaxed);
}

/// zai / provider 查询的 60s 结果缓存条目（成功与失败同 TTL，见
/// QUERY_CACHE_TTL 说明；codex/claude/kimi 走各自模块内建缓存，不进此结构）
#[derive(Clone)]
struct QueryCache<T> {
    at: Instant,
    value: Option<T>,
}

impl<T> QueryCache<T> {
    fn fresh(&self) -> bool {
        self.at.elapsed() < QUERY_CACHE_TTL
    }
}

/// 吸附/收卡守护线程主循环：拖放结束检测（Moved 静默 ≥ SNAP_IDLE_MS
/// 且非回声 → 投递 snap_to_edge）+ 卡片悬浮守护（见 card_guard_tick）。
fn snap_loop() {
    let mut outside_since: Option<Instant> = None;
    loop {
        if FEED_STOP.load(Ordering::Relaxed) {
            return;
        }
        let app = snap_app_slot().lock().ok().and_then(|g| g.clone());
        if let Some(app) = app.as_ref() {
            card_guard_tick(app, &mut outside_since);
        }
        let moved_at = LAST_MOVED_AT.load(Ordering::Relaxed);
        // 自定义拖拽期间不做静默吸附：位置由 hud_bar_drag_move 逐步接管
        if !DRAGGING.load(Ordering::Relaxed)
            && moved_at > 0
            && now_ms().saturating_sub(moved_at) >= SNAP_IDLE_MS
        {
            // 置零防重复投递（用户再拖会重新刷新）
            LAST_MOVED_AT.store(0, Ordering::Relaxed);
            // snap_to_edge 需要 AppHandle 投递主线程闭包；本线程没有 handle，
            // 经 SNAP_APP 全局槽取（start_feed 时存入）
            if let Some(app) = app {
                let app_snap = app.clone();
                let _ = app.run_on_main_thread(move || snap_to_edge(&app_snap));
            }
        }
        thread::sleep(Duration::from_millis(FAST_TICK_MS));
    }
}

/// 卡片悬浮守护（Rust 侧权威兜底）：卡片可见期间，真实光标连续
/// CARD_CLOSE_GRACE_MS 不在「条窗口 ∪ 卡片窗口」内就收卡。
///
/// 条页自己有一套 150ms 的 checkPointer（还要维护 .hud-open 高亮与卡片锚点），
/// 但那套判定依赖条页本地状态：页面重载/状态失步/某次 invoke 卡住时轮询会
/// 停摆，卡片窗口就成了关不掉的孤儿（实测：点开卡片 → 移到卡上 → 移走，
/// 偶尔不复位）。这里只认光标与两个窗口的屏幕矩形，状态只有"离开起始时刻"，
/// 不受前端影响；命中后投递主线程 force_close_card（隐藏窗口 + 两端收敛）。
fn card_guard_tick(app: &AppHandle, outside_since: &mut Option<Instant>) {
    if !CARD_VISIBLE.load(Ordering::Relaxed) {
        *outside_since = None;
        return;
    }
    // 用 AppHandle 的 cursor_position（走事件循环消息）而不是 Window 的同名
    // 方法：后者在 Linux 上是直接调 GDK，从守护线程调不安全；AppHandle 版本
    // 会把读取投递回主线程。
    let Ok(cursor) = app.cursor_position() else {
        // 读不到光标：本拍跳过但不清零，别把"刚离开"误判成"还在"
        return;
    };
    let mut inside = false;
    for win in [
        app.get_webview_window(HUD_BAR_WINDOW_LABEL),
        app.get_webview_window(HUD_CARD_WINDOW_LABEL),
    ]
    .into_iter()
    .flatten()
    {
        let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else {
            continue;
        };
        // 2px 容差：圆角/缩放边缘不误判；卡片窗口自带 CARD_PAD 留白，
        // 用窗口矩形而不是卡片内容矩形——指针落在留白上也算"在卡上"
        let left = pos.x as f64 - 2.0;
        let top = pos.y as f64 - 2.0;
        let right = (pos.x + size.width as i32) as f64 + 2.0;
        let bottom = (pos.y + size.height as i32) as f64 + 2.0;
        if cursor.x >= left && cursor.x <= right && cursor.y >= top && cursor.y <= bottom {
            inside = true;
            break;
        }
    }
    if inside {
        *outside_since = None;
        return;
    }
    let now = Instant::now();
    let since = *outside_since.get_or_insert(now);
    if now.duration_since(since) < Duration::from_millis(CARD_CLOSE_GRACE_MS) {
        return;
    }
    *outside_since = None;
    let app_close = app.clone();
    let _ = app.run_on_main_thread(move || force_close_card(&app_close));
}

/// 吸附线程的 AppHandle 槽（snap_loop 投递主线程闭包需要；start_feed
/// 启动线程前存入）
static SNAP_APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();

fn snap_app_slot() -> &'static Mutex<Option<AppHandle>> {
    SNAP_APP.get_or_init(|| Mutex::new(None))
}

/// 数据线程主循环：200ms 快循环承载两件事——
/// 1) 数据拍：距上拍 ≥ FEED_TICK_MS 或 FORCE_PUSH 置位 → 组装各源额度并
///    推送（内容变化才 emit；变化检测保证 30s 拍下无变化时不刷 IPC）；
/// 2) 托盘开启/页面就绪的强推响应（FORCE_PUSH，200ms 内出数据）。
/// 吸附守护已拆到独立线程（见 snap_loop）。任何源查询失败静默跳过该源
/// 本轮（不 panic 不刷日志），下轮重试。
fn feed_loop(app: AppHandle) {
    // 变化检测缓存（agents 序列化字节，ts 不参与比对）
    let mut cache: Option<String> = None;
    // 数据拍节拍锚点：远过去，保证首拍立即执行（启动恢复/托盘开启秒出数据）
    let mut last_tick = Instant::now() - Duration::from_secs(FEED_TICK_MS + 1);
    // zai / opencodego / minimax / dsh 的查询结果缓存（单线程访问，局部
    // 变量即可）
    let mut zai_cache: Option<QueryCache<QuotaResult>> = None;
    // Cursor 套餐额度单独缓存：fetch_cursor_snapshot 可能拉 events（较慢），
    // 60s 内复用，且只在该 agent 开启时查询
    let mut cursor_cache: Option<QueryCache<HudBarAgent>> = None;
    let mut ocg_cache: Option<QueryCache<Vec<ProviderQuotaEntry>>> = None;
    let mut mmx_cache: Option<QueryCache<Vec<ProviderQuotaEntry>>> = None;
    let mut dsh_cache: Option<QueryCache<Vec<ProviderQuotaEntry>>> = None;
    // 窗口尺寸已同步到的条目数（条目数变化才经主线程 set_size）
    let mut synced_items = 0usize;
    // 本会话内取到过额度/余额的来源 = 订阅存在的证据（粘滞）。额度查询偶发
    // 失败时，已确认的订阅不该从条上消失（见 PlaceholderEvidence）
    let mut subscribed: HashSet<String> = HashSet::new();
    loop {
        if FEED_STOP.load(Ordering::Relaxed) {
            return;
        }
        // ---- 数据拍 ----
        let forced = FORCE_PUSH.load(Ordering::Relaxed);
        if forced || last_tick.elapsed() >= Duration::from_millis(FEED_TICK_MS) {
            FORCE_PUSH.store(false, Ordering::Relaxed);
            last_tick = Instant::now();
            // 各源独立容错：失败源缓存 60s（防重试风暴），本轮跳过
            if !zai_cache.as_ref().is_some_and(|c| c.fresh()) {
                zai_cache = crate::quota::query_quota().ok().map(|v| QueryCache {
                    at: Instant::now(),
                    value: Some(v),
                }).or_else(|| Some(QueryCache { at: Instant::now(), value: None }));
            }
            if !ocg_cache.as_ref().is_some_and(|c| c.fresh()) {
                ocg_cache =
                    crate::provider_quota::query_provider_quota("opencodego")
                        .ok()
                        .map(|v| QueryCache { at: Instant::now(), value: Some(v) })
                        .or_else(|| Some(QueryCache { at: Instant::now(), value: None }));
            }
            if !mmx_cache.as_ref().is_some_and(|c| c.fresh()) {
                mmx_cache =
                    crate::provider_quota::query_provider_quota("minimax")
                        .ok()
                        .map(|v| QueryCache { at: Instant::now(), value: Some(v) })
                        .or_else(|| Some(QueryCache { at: Instant::now(), value: None }));
            }
            // dsh：DeepSeek 平台余额（provider id 仍是 deepseek，与主面板
            // DSH 面板底部的余额区同一通道）。未录 Key 时 query 返回空数组，
            // dsh 由 fill_placeholders 按「订阅/今日用量」证据补占位；同样
            // 60s 缓存限频
            if !dsh_cache.as_ref().is_some_and(|c| c.fresh()) {
                dsh_cache = Some(QueryCache {
                    at: Instant::now(),
                    value: crate::provider_quota::query_provider_quota("deepseek").ok(),
                });
            }
            // codex/claude/kimi：fetch_live_rate_limits 内建 60s/15s 双 TTL +
            // single-flight 缓存，直接调用即可（大概率命中缓存零 HTTP）
            let codex = crate::codex::fetch_live_rate_limits().ok().flatten();
            let claude = crate::claude::fetch_live_rate_limits().ok().flatten();
            let kimi = crate::kimi::fetch_live_rate_limits().ok().flatten();
            if !cursor_cache.as_ref().is_some_and(|c| c.fresh()) {
                cursor_cache = Some(QueryCache {
                    at: Instant::now(),
                    value: enabled_cursor_agent(),
                });
            }
            let mut agents = build_usage(
                zai_cache.as_ref().and_then(|c| c.value.as_ref()),
                codex.as_ref(),
                claude.as_ref(),
                kimi.as_ref(),
                ocg_cache.as_ref().and_then(|c| c.value.as_deref()),
                mmx_cache.as_ref().and_then(|c| c.value.as_deref()),
                dsh_cache.as_ref().and_then(|c| c.value.as_deref()),
            );
            if let Some(cursor) = cursor_cache.as_ref().and_then(|c| c.value.as_ref()) {
                agents.push(cursor.clone());
            }
            // 订阅证据：本拍真取到额度/余额的来源记进粘滞集合
            for agent in &agents {
                if agent.status.is_none() {
                    subscribed.insert(agent.source.clone());
                }
            }
            // 设置页关掉的 agent 不进 payload（前端 renderItems 仍按同一份
            // 偏好过滤，但 Rust 侧先滤掉才能让 ITEM_COUNT / 窗口尺寸与实际
            // 渲染的条目数一致——否则隐藏一个有额度的 agent 会让条多出一格
            // 空位）。可见性镜像一次读文件，本拍内反复用。
            let visibility = crate::agent_visibility::load();
            agents.retain(|agent| {
                crate::agent_visibility::is_visible_in(&visibility, &agent.source)
            });
            let visible_sources: BTreeSet<String> = HUD_SOURCES
                .iter()
                .filter(|source| crate::agent_visibility::is_visible_in(&visibility, source))
                .map(|source| (*source).to_string())
                .collect();
            // 今日用量快照（托盘标题线程每 30s 写一次，已含其他设备），
            // 用来决定"既没订阅、今日也没用量"的来源不出现。
            let today_snapshot = crate::today_usage::snapshot().filter(|s| s.fresh());
            let evidence = PlaceholderEvidence {
                visible: &visible_sources,
                subscribed: &subscribed,
                today: today_snapshot.as_ref(),
            };
            fill_placeholders(&mut agents, &evidence);
            let ts = chrono::Utc::now().timestamp_millis();
            // 变化检测：agents 序列化字节不变则不 emit（倒计时由前端本地
            // 计算，无需按秒推送）
            let key = serde_json::to_string(&agents).unwrap_or_default();
            if forced || cache.as_deref() != Some(key.as_str()) {
                cache = Some(key);
                // 广播（不是 emit_to 条窗口）：额度数据两个窗口都要——条页画
                // 条目，卡片窗口画详情卡（卡片是独立 webview，拿不到 emit_to
                // 只发给条的 payload，卡里就会是空数据）
                let _ = app.emit(
                    HUD_BAR_USAGE_EVENT,
                    HudBarUsage { ts, agents: agents.clone() },
                );
            }
            // 条目数变化 → 窗口尺寸自适应（拖拽中由 drag_geometry 接管，
            // 这里不插手）。空态占 1 条。
            let items = agents.len().max(1);
            ITEM_COUNT.store(items, Ordering::Relaxed);
            if items != synced_items {
                synced_items = items;
                let app_size = app.clone();
                let _ = app.run_on_main_thread(move || sync_bar_size(&app_size));
            }
        }
        // 分段睡眠：及时感知 stop
        thread::sleep(Duration::from_millis(FAST_TICK_MS));
    }
}

/// 把窗口尺寸同步到当前条目数（主线程闭包体，**物理坐标**——set_size 与
/// outer_size 同口径）。位置不动（左上角锚定：贴左/贴顶时贴边坐标不变，
/// 贴右时条厚不变 x 仍贴右缘——set_size 保持左上角，右缘会暂时内缩，下一
/// 次吸附/拖放会重新贴齐，可接受的一期简化）。
fn sync_bar_size(app: &AppHandle) {
    let Some(win) = app.get_webview_window(HUD_BAR_WINDOW_LABEL) else {
        return;
    };
    let Ok(scale) = win.scale_factor() else { return };
    if scale <= 0.0 {
        return;
    }
    if DRAGGING.load(Ordering::Relaxed) {
        // 拖拽中条的形状由 drag_geometry 每帧接管，这里别插手
        return;
    }
    let (w, h) = bar_metrics_physical(current_edge(), item_count(), scale);
    let _ = win.set_size(PhysicalSize::new(w.round().max(1.0) as u32, h.round().max(1.0) as u32));
}

/// 贴边吸附（主线程闭包体，**全程物理坐标**）：扩窗态先缩回（保条可见
/// 位置），窗口全局坐标减 mon.position() 换算本屏局部 → 最近边判定 →
/// 落点加回 mon.position() 得全局坐标 set_position（多显示器下虚拟桌面
/// 全局坐标与本屏分辨率直接运算会把窗口弹回主屏），set_size 切横竖排，
/// 落盘 {edge,pos}（逻辑坐标）并推布局事件。任何一步失败静默（下次拖放
/// 重试）。
fn snap_to_edge(app: &AppHandle) {
    // 自定义拖拽进行中：落点由 hud_bar_drag_end 负责，静默吸附不插手
    if DRAGGING.load(Ordering::Relaxed) {
        return;
    }
    let Some(win) = app.get_webview_window(HUD_BAR_WINDOW_LABEL) else {
        return;
    };
    let Ok(scale) = win.scale_factor() else { return };
    if scale <= 0.0 {
        return;
    }
    // 收起态的卡片先收掉：吸附几何按条自身的矩形计算（卡片是独立窗口，
    // 不影响几何判定，但程序侧回位后卡片位置就过期了）
    force_close_card(app);
    let Ok(pos) = win.outer_position() else { return };
    let Ok(size) = win.outer_size() else { return };
    let Ok(mon) = win.current_monitor() else { return };
    let Some(mon) = mon else { return };
    // 本屏局部物理坐标（全局 − 本屏原点）：nearest_edge / snap_target 的
    // 输入契约
    let m_pos = mon.position();
    let mon_w_px = mon.size().width as f64;
    let mon_h_px = mon.size().height as f64;
    let x_local = (pos.x - m_pos.x) as f64;
    let y_local = (pos.y - m_pos.y) as f64;
    let w_px = size.width as f64;
    let h_px = size.height as f64;
    let edge = nearest_edge(mon_w_px, mon_h_px, x_local + w_px / 2.0, y_local + h_px / 2.0);
    let (tw, th, tx_local, ty_local) =
        snap_target(edge, mon_w_px, mon_h_px, x_local, y_local, item_count(), scale);
    // 本屏局部 → 全局物理坐标（加回本屏原点）
    let tx_global = m_pos.x as f64 + tx_local;
    let ty_global = m_pos.y as f64 + ty_local;
    let _ = win.set_size(PhysicalSize::new(tw.round().max(1.0) as u32, th.round().max(1.0) as u32));
    let _ = win.set_position(PhysicalPosition::new(
        tx_global.round() as i32,
        ty_global.round() as i32,
    ));
    // 程序侧落点登记（逻辑坐标，与 Moved 挂点同口径）：随后的 Moved 回声
    // 被拦截，不再触发吸附
    let snap_logical = (tx_global / scale, ty_global / scale);
    if let Ok(mut guard) = snap_pos_slot().lock() {
        *guard = Some(snap_logical);
    }
    if let Ok(mut guard) = last_pos_slot().lock() {
        *guard = Some(snap_logical);
    }
    CURRENT_EDGE.store(edge.index(), Ordering::Relaxed);
    // 落盘 {edge,pos}（吸附是停靠位置的主要持久化路径；拖动中不落盘；
    // pos 为逻辑坐标，与建窗 position 同口径）
    let mut cfg = load_hud_bar_config();
    if cfg.edge_of() != edge || cfg.pos != Some(snap_logical) {
        cfg.edge = edge.key().to_string();
        cfg.pos = Some(snap_logical);
        let _ = save_hud_bar_config(&cfg);
    }
    // 通知前端切布局（orientation + edge，edge 供卡片按边定位方向）；
    // 页面未就绪时丢失无妨：前端按窗口横竖比自判兜底
    push_layout(app, edge);
}

// ============================================================
// 窗口事件挂点（lib.rs 的 on_window_event 按 label 分发）
// ============================================================

/// Moved 挂点：物理坐标转逻辑写内存并刷新静默判定锚点。程序侧落点回声
/// （吸附/扩窗自身的 set_position 触发）被 SNAP_POS 比对拦截。
pub fn handle_hud_bar_window_moved(win: &tauri::Window, pos: tauri::PhysicalPosition<i32>) {
    // 自定义拖拽期间的位置全由前端 16ms 一拍下发，Moved 是程序侧回声：
    // 不刷新静默锚点，避免松手瞬间被吸附守护二次挪动
    if DRAGGING.load(Ordering::Relaxed) {
        return;
    }
    let scale = win.scale_factor().unwrap_or(1.0);
    if scale <= 0.0 {
        return;
    }
    let logical = (pos.x as f64 / scale, pos.y as f64 / scale);
    if is_snap_echo(logical) {
        return;
    }
    if let Ok(mut guard) = last_pos_slot().lock() {
        *guard = Some(logical);
    }
    LAST_MOVED_AT.store(now_ms(), Ordering::Relaxed);
}

/// Destroyed 挂点：停线程 + 开关复位落盘后重建托盘菜单刷新勾选态（正常
/// 关闭路径只隐藏不销毁，本挂点兜底异常销毁，照 session_hud 同款幂等
/// 语义；勾选刷新是 P2-8：落盘后菜单若不重建会残留开启勾）。
pub fn handle_hud_bar_window_destroyed(app: &AppHandle) {
    stop_feed();
    // 卡片是独立窗口，条销毁后它必须一起销毁（否则留一块点不到的透明窗）
    if let Some(card) = app.get_webview_window(HUD_CARD_WINDOW_LABEL) {
        let _ = card.destroy();
    }
    if let Ok(mut guard) = card_rect_slot().lock() {
        *guard = None;
    }
    CARD_VISIBLE.store(false, Ordering::Relaxed);
    let mut cfg = load_hud_bar_config();
    if cfg.enabled {
        cfg.enabled = false;
        let _ = save_hud_bar_config(&cfg);
        crate::refresh_tray_menu(app);
    }
    LAST_MOVED_AT.store(0, Ordering::Relaxed);
}

// ============================================================
// 窗口管理
// ============================================================

/// 确保 "hud-bar" 窗口存在并显示（托盘开启 / 启动恢复共用）；已存在时仅
/// show + 推一次布局事件。必须在主线程事件循环上下文调用（setup 阶段的
/// start_if_enabled 与托盘菜单回调，建窗铁律见模块 doc）。
pub fn ensure_hud_bar_window(app: &AppHandle) -> Result<(), String> {
    let cfg = load_hud_bar_config();
    let edge = cfg.edge_of();
    if let Some(win) = app.get_webview_window(HUD_BAR_WINDOW_LABEL) {
        let _ = win.show();
        push_layout(app, edge);
        ensure_hud_card_window(app)?;
        return Ok(());
    }
    CURRENT_EDGE.store(edge.index(), Ordering::Relaxed);
    let (w, h) = bar_metrics(edge, item_count());
    let mut builder = WebviewWindowBuilder::new(
        app,
        HUD_BAR_WINDOW_LABEL,
        WebviewUrl::App("hud.html".into()),
    )
    .title("AgentHUD Bar")
    .inner_size(w, h)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    // 双击拖拽区不能把 68×条长 的小窗最大化（undecorated 窗在 Windows
    // 上双击标题区默认触发最大化，小窗被拉满整屏）
    .maximizable(false)
    // Windows 钳宽修复（照搬 pet.rs / session_hud.rs）：tao 的
    // WM_GETMINMAXINFO 默认最小跟踪宽度会把 68 逻辑宽的小窗出生即钳宽，
    // min_inner_size 覆盖该值
    .min_inner_size(1.0, 1.0)
    // 后台节流必须关掉：HUD 窗永不抢焦点（focusable(false)），macOS 会把
    // "非活跃 App 的 webview"判成后台并按 WKInactiveSchedulingPolicy 暂停
    // JS 调度 —— 悬浮守护的 150ms 轮询一停，指针移出/点桌面后详情卡就再也
    // 收不掉（卡片退出动画、30s 倒计时同理）。Disabled 让定时器在 App 失活
    // 时照常跑；Windows/Linux 上 wry 忽略该属性。
    .background_throttling(BackgroundThrottlingPolicy::Disabled)
    // 不抢焦点（照搬 pet.rs）：focusable(false) 映射 WS_EX_NOACTIVATE 从
    // 根上不激活；停靠条无键盘交互，点击条目/拖动都不需要激活
    .focusable(false)
    .focused(false);

    // 位置：持久化吸附落点优先；无记录默认贴主显示器对应边缘垂直居中
    // （贴右时 x = 屏宽 − 条厚，否则左边缘 x=0；不再一律按贴左落点）
    let mon = app.primary_monitor().ok().flatten();
    let (x, y) = match cfg.pos {
        Some(p) => p,
        None => {
            let scale = mon.as_ref().map(|m| m.scale_factor()).unwrap_or(1.0);
            let mon_w = mon
                .as_ref()
                .map(|m| m.size().width as f64 / scale)
                .unwrap_or(1920.0);
            let mon_h = mon
                .as_ref()
                .map(|m| m.size().height as f64 / scale)
                .unwrap_or(1080.0);
            let x = if edge == BarEdge::Right {
                (mon_w - BAR_THICK).max(0.0)
            } else {
                0.0
            };
            (x, ((mon_h - h) / 2.0).max(0.0))
        }
    };
    builder = builder.position(x, y);

    // 平台差异（照搬 session_hud.rs）：macOS 显式清空背景防白底；
    // Windows 隐藏建窗 → set_size 收回请求尺寸（钳宽顺序）→ 显示
    #[cfg(target_os = "macos")]
    {
        let win = builder
            .build()
            .map_err(|e| format!("创建停靠条窗口失败: {e}"))?;
        let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let win = builder
            .visible(false)
            .build()
            .map_err(|e| format!("创建停靠条窗口失败: {e}"))?;
        let _ = win.set_size(LogicalSize::new(w, h));
        let _ = win.show();
    }
    if let Ok(mut guard) = snap_pos_slot().lock() {
        *guard = Some((x, y));
    }
    // 卡片窗口同时预热（隐藏态）：第一次 hover 才能秒开（建窗 ~300ms 不能
    // 落在 hover 路径上）
    ensure_hud_card_window(app)?;
    // 首帧布局事件（页面未就绪时丢失无妨，前端按窗口横竖比自判兜底）
    push_layout(app, edge);
    Ok(())
}

/// 推布局方向事件（payload 含 orientation + edge：orientation 切横竖
/// 排，edge 供前端卡片按边定位方向——贴左卡在条右侧、贴右卡在条左侧）
fn push_layout(app: &AppHandle, edge: BarEdge) {
    let _ = app.emit_to(
        HUD_BAR_WINDOW_LABEL,
        HUD_BAR_LAYOUT_EVENT,
        serde_json::json!({
            "orientation": edge.orientation(),
            "edge": edge.key(),
        }),
    );
}

/// 应用启动挂点：开关开启时恢复窗口与数据线程（未开启零开销，照
/// session_hud::start_if_enabled）。
pub fn start_if_enabled(app: &AppHandle) {
    let cfg = load_hud_bar_config();
    if !cfg.enabled {
        return;
    }
    CURRENT_EDGE.store(cfg.edge_of().index(), Ordering::Relaxed);
    if let Err(e) = ensure_hud_bar_window(app) {
        eprintln!("[zbar-hud-bar] 恢复停靠条失败: {e}");
        return;
    }
    start_feed(app.clone());
    FORCE_PUSH.store(true, Ordering::Relaxed);
}

/// 托盘开关切换（on_menu_event 主线程回调内直接调用，建窗合法上下文）。
/// 返回切换后的开启态（托盘侧据此更新菜单勾选）。开：落盘 + 建窗/显示 +
/// 启线程 + 强推首帧；关：隐藏窗口（保留隐藏态不销毁，页面状态原样保留
/// 秒开）+ 停线程 + 落盘。
pub fn toggle_from_tray(app: &AppHandle) -> Result<bool, String> {
    let mut cfg = load_hud_bar_config();
    if cfg.enabled {
        cfg.enabled = false;
        save_hud_bar_config(&cfg)?;
        if let Some(win) = app.get_webview_window(HUD_BAR_WINDOW_LABEL) {
            // 隐藏条之前先把卡片收掉（卡片是独立窗口，不跟着条一起隐藏会
            // 留一块悬浮在黑边上的卡）；广播 popover 事件让条前端收敛状态
            force_close_card(app);
            let _ = win.hide();
        }
        hide_card_window(app);
        stop_feed();
        Ok(false)
    } else {
        cfg.enabled = true;
        save_hud_bar_config(&cfg)?;
        ensure_hud_bar_window(app)?;
        ensure_hud_card_window(app)?;
        start_feed(app.clone());
        FORCE_PUSH.store(true, Ordering::Relaxed);
        Ok(true)
    }
}

// ============================================================
// 详情卡独立窗口
// ============================================================
//
// 卡片为什么必须独立成窗：卡片要挂在条的**外侧**，而条贴着屏幕边缘，所以
// 承载卡片的窗口只能向屏幕内侧长出去——那必然要 OS 改窗口位置与尺寸。而
// WebView2 的渲染子窗口锚定在父窗口左上角：父窗口先变大/移位、渲染进程还
// 没按新视口重排的那一帧，整条会被画到新窗口的左上角（实机录屏逐帧抓到过：
// 条瞬移到屏幕左上方 336×274 处一帧再弹回——"第一次悬浮时卡片闪现移动"）。
// 改成"条窗口永远不动、卡片自己一个窗口"后，hover 全程没有任何窗口几何
// 变化，这一帧错位从根上消失；卡片窗口只在**自己的内容不可见**时改尺寸/
// 移位（页面侧先 .measuring 藏起来量，量完由 Rust 摆位 + show，再露出），
// 因此它自己的重排也永远不会被看见。

/// 确保 "hud-card" 窗口存在（隐藏态预热，第一次 hover 才能秒开）。
/// 必须在主线程事件循环上下文调用（建窗铁律，同 ensure_hud_bar_window）。
pub fn ensure_hud_card_window(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(HUD_CARD_WINDOW_LABEL).is_some() {
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(
        app,
        HUD_CARD_WINDOW_LABEL,
        WebviewUrl::App("hud.html".into()),
    )
    .title("AgentHUD Card")
    // 占位尺寸：每次开卡都按卡片实测高度重设（内容此刻不可见，重设不会被看见）
    .inner_size(CARD_W + CARD_PAD * 2.0, CARD_W + CARD_PAD * 2.0)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .min_inner_size(1.0, 1.0)
    // 卡片窗同样关后台节流：收卡靠卡片页 150ms 退出动画后的 hud_card_hide，
    // 定时器被暂停会让窗口停在"已经该隐藏"的状态（同条窗的理由，见上）
    .background_throttling(BackgroundThrottlingPolicy::Disabled)
    // 不抢焦点（同停靠条）：卡片只做悬浮展示与点击，"打开主窗口"按钮走 IPC
    .focusable(false)
    .focused(false)
    .visible(false)
    .build()
    .map_err(|e| format!("创建详情卡窗口失败: {e}"))?;
    #[cfg(target_os = "macos")]
    let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    let _ = win;
    Ok(())
}

/// 隐藏卡片窗口并清掉"卡在哪"的记忆（Rust 侧单方面收卡时的公共尾段）
fn hide_card_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(HUD_CARD_WINDOW_LABEL) {
        let _ = win.hide();
    }
    CARD_VISIBLE.store(false, Ordering::Relaxed);
    if let Ok(mut guard) = card_rect_slot().lock() {
        *guard = None;
    }
}

/// 程序侧收卡（吸附强制回位 / 托盘关闭条）：隐藏卡片窗口 + 广播 popover
/// 事件让条前端收敛本地详情态（条页不再回发 close，避免来回乒乓）。
/// 卡片页也要收一次 close（animate=false）：它的 30s 重绘定时器在窗口隐藏后
/// 仍在跑（卡片窗也关掉了后台节流），不通知就会继续按 cardSource 重建 DOM，
/// 万一行数变化触发 measure → 已隐藏的卡片窗会自己再 show 出来。
fn force_close_card(app: &AppHandle) {
    if !CARD_VISIBLE.swap(false, Ordering::Relaxed) {
        return;
    }
    hide_card_window(app);
    let _ = app.emit_to(
        HUD_CARD_WINDOW_LABEL,
        HUD_CARD_CLOSE_EVENT,
        serde_json::json!({ "animate": false }),
    );
    let _ = app.emit_to(
        HUD_BAR_WINDOW_LABEL,
        HUD_BAR_POPOVER_EVENT,
        serde_json::json!({ "open": false }),
    );
}

/// 主线程闭包投递 + 超时等待（窗口操作必须落在事件循环线程；同步命令占
/// 主线程会自等待死锁——同 hud_bar_set_popover 的建窗铁律）
fn on_main_thread<T, F>(app: &AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<T, String>>();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| format!("投递卡片窗口操作到主线程失败: {e}"))?;
    match rx.recv_timeout(HUD_BAR_OP_TIMEOUT) {
        Ok(outcome) => outcome,
        Err(e) => Err(format!("卡片窗口操作超时: {e}")),
    }
}

/// 开卡：登记锚点（hover 条目中心的物理坐标）后通知卡片页渲染。
/// 条前端只负责"哪一条 Agent、锚在哪"，尺寸与屏幕落点由 Rust + 卡片页决定。
#[tauri::command]
pub async fn hud_card_open(
    source: String,
    center_x: f64,
    center_y: f64,
    animate: bool,
    app: AppHandle,
) -> Result<(), String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        ensure_hud_card_window(&app_main)?;
        if let Ok(mut guard) = card_anchor_slot().lock() {
            *guard = Some(CardAnchor {
                center_x,
                center_y,
            });
        }
        CARD_VISIBLE.store(true, Ordering::Relaxed);
        app_main
            .emit_to(
                HUD_CARD_WINDOW_LABEL,
                HUD_CARD_OPEN_EVENT,
                serde_json::json!({
                    "source": source,
                    "edge": current_edge().key(),
                    "animate": animate,
                    "pad": CARD_PAD,
                }),
            )
            .map_err(|e| format!("通知卡片窗口失败: {e}"))
    })
}

/// 卡片页量好了自身尺寸（CSS px）→ 摆窗口。**此刻卡片内容还是隐藏态**
/// （页面侧 .measuring），所以 set_size/set_position/show 都不会被看见；
/// 回执目标视口尺寸（CSS px），前端等视口真正到位后再露出卡片并入场。
#[tauri::command]
pub async fn hud_card_measure(
    width: f64,
    height: f64,
    app: AppHandle,
) -> Result<HudCardViewport, String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        let Some(win) = app_main.get_webview_window(HUD_CARD_WINDOW_LABEL) else {
            return Err("详情卡窗口不存在".into());
        };
        let scale = win
            .scale_factor()
            .map_err(|e| format!("读取卡片缩放比失败: {e}"))?;
        if scale <= 0.0 {
            return Err("卡片缩放比非法".into());
        }
        let anchor = card_anchor_slot()
            .lock()
            .ok()
            .and_then(|g| *g)
            .ok_or_else(|| "缺少卡片锚点".to_string())?;
        let bar_win = app_main
            .get_webview_window(HUD_BAR_WINDOW_LABEL)
            .ok_or_else(|| "停靠条窗口不存在".to_string())?;
        let bar_pos = bar_win
            .outer_position()
            .map_err(|e| format!("读取停靠条位置失败: {e}"))?;
        let (bw, bh) = bar_metrics_physical(current_edge(), item_count(), scale);
        // 工作区按**条所在的屏**取（卡片永远跟着条；指针可能已经飘到别的屏）
        let (area, _) = monitor_area_under(
            &bar_win,
            (bar_pos.x as f64 + bw / 2.0, bar_pos.y as f64 + bh / 2.0),
        );
        let cw = width * scale;
        let ch = height * scale;
        let (cx, cy) = card_placement(
            current_edge(),
            (bar_pos.x as f64, bar_pos.y as f64, bw, bh),
            anchor.center_x,
            anchor.center_y,
            cw,
            ch,
            area,
            scale,
        );
        let pad = CARD_PAD * scale;
        win.set_size(PhysicalSize::new(
            (cw + pad * 2.0).round().max(1.0) as u32,
            (ch + pad * 2.0).round().max(1.0) as u32,
        ))
        .map_err(|e| format!("卡片窗口改尺寸失败: {e}"))?;
        win.set_position(PhysicalPosition::new(
            (cx - pad).round() as i32,
            (cy - pad).round() as i32,
        ))
        .map_err(|e| format!("卡片窗口定位失败: {e}"))?;
        win.show()
            .map_err(|e| format!("显示卡片窗口失败: {e}"))?;
        CARD_VISIBLE.store(true, Ordering::Relaxed);
        if let Ok(mut guard) = card_rect_slot().lock() {
            *guard = Some(HudCardRect {
                x: cx,
                y: cy,
                w: cw,
                h: ch,
            });
        }
        let viewport = HudCardViewport {
            width: width + CARD_PAD * 2.0,
            height: height + CARD_PAD * 2.0,
        };
        app_main
            .emit_to(HUD_CARD_WINDOW_LABEL, HUD_CARD_PLACED_EVENT, viewport)
            .map_err(|e| format!("通知卡片落位失败: {e}"))?;
        Ok(viewport)
    })
}

/// 收卡：先通知卡片页播退出动画（它播完会回 hud_card_hide）。屏幕矩形
/// 立刻作废，避免悬浮守护还把指针当成"在卡上"而不收卡。
#[tauri::command]
pub async fn hud_card_close(animate: bool, app: AppHandle) -> Result<(), String> {
    if let Ok(mut guard) = card_rect_slot().lock() {
        *guard = None;
    }
    app.emit_to(
        HUD_CARD_WINDOW_LABEL,
        HUD_CARD_CLOSE_EVENT,
        serde_json::json!({ "animate": animate }),
    )
    .map_err(|e| format!("通知卡片窗口收卡失败: {e}"))
}

/// 卡片页播完退出动画后真正隐藏窗口
#[tauri::command]
pub async fn hud_card_hide(app: AppHandle) -> Result<(), String> {
    let app_main = app.clone();
    on_main_thread(&app, move || {
        hide_card_window(&app_main);
        Ok(())
    })
}

/// 卡片窗口的目标视口尺寸（CSS px）
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudCardViewport {
    pub width: f64,
    pub height: f64,
}


/// 前端悬浮守护用的光标/窗口几何（**物理像素**，与 cursor_position /
/// outer_position 同口径）。DOM 的 pointerleave 只在指针真的离开窗口时可靠，
/// 而卡片是另一个窗口——指针从条移到卡上时条的 pointerleave 会先到，光靠
/// DOM 事件会把卡收掉。前端按 150ms 轮询本命令，用真实光标位置判断"是否
/// 还在条/卡上"，彻底脱离 DOM 事件。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudBarPointerState {
    /// 光标全局物理坐标
    pub cursor_x: f64,
    pub cursor_y: f64,
    /// 窗口左上角全局物理坐标
    pub window_x: i32,
    pub window_y: i32,
    /// 逻辑/物理缩放比（客户端 CSS px = (物理 − 窗口原点) / scale）
    pub scale: f64,
    /// 当前详情卡的屏幕矩形（物理 px）；None = 没有可见的卡
    pub card: Option<HudCardRect>,
}

/// 读取停靠条当前光标与窗口几何（供前端悬浮守护轮询）。
#[tauri::command]
pub fn hud_bar_pointer_state(app: AppHandle) -> Result<HudBarPointerState, String> {
    let win = app
        .get_webview_window(HUD_BAR_WINDOW_LABEL)
        .ok_or_else(|| "停靠条窗口不存在".to_string())?;
    let cursor = win
        .cursor_position()
        .map_err(|e| format!("读取光标位置失败: {e}"))?;
    let pos = win
        .outer_position()
        .map_err(|e| format!("读取停靠条位置失败: {e}"))?;
    let scale = win
        .scale_factor()
        .map_err(|e| format!("读取停靠条缩放比失败: {e}"))?;
    Ok(HudBarPointerState {
        cursor_x: cursor.x,
        cursor_y: cursor.y,
        window_x: pos.x,
        window_y: pos.y,
        scale: if scale > 0.0 { scale } else { 1.0 },
        card: card_rect_slot().lock().ok().and_then(|g| *g),
    })
}

/// 拖拽状态回执（逻辑 px，屏幕坐标）。前端据此切换横竖排/停靠边，
/// `buttonDown` 只作诊断（拖拽结束由前端 pointerup 驱动）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudDragState {
    pub edge: String,
    pub orientation: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// 是否吸附在工作区边缘（false = 跟手浮拽）
    pub snapped: bool,
}

fn drag_state_of(edge: BarEdge, geo: (f64, f64, f64, f64), snapped: bool) -> HudDragState {
    HudDragState {
        edge: edge.key().to_string(),
        orientation: edge.orientation().to_string(),
        x: geo.0,
        y: geo.1,
        w: geo.2,
        h: geo.3,
        snapped,
    }
}

/// 指针（物理 px）落在第几块屏：先按包含关系找，找不到（拖出边界/屏间空隙）
/// 取**中心距离最近**的一块。纯函数，便于单测；`monitors` 元素为 (x, y, w, h)。
fn monitor_index_at(monitors: &[(f64, f64, f64, f64)], point: (f64, f64)) -> Option<usize> {
    if monitors.is_empty() {
        return None;
    }
    for (i, m) in monitors.iter().enumerate() {
        if point.0 >= m.0 && point.0 < m.0 + m.2 && point.1 >= m.1 && point.1 < m.1 + m.3 {
            return Some(i);
        }
    }
    let dist = |m: &(f64, f64, f64, f64)| {
        let cx = m.0 + m.2 / 2.0;
        let cy = m.1 + m.3 / 2.0;
        (cx - point.0) * (cx - point.0) + (cy - point.1) * (cy - point.1)
    };
    monitors
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| dist(a).partial_cmp(&dist(b)).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
}

/// 指针所在屏的工作区（**物理 px**）与缩放比：跨屏拖拽时窗口要落到指针
/// 那一块屏上，而不是窗口当前所在屏（窗口一直贴边，永远追不上指针）。
/// 自己按 `available_monitors` 的物理矩形做包含判定，不用
/// `monitor_from_point`——后者入参口径在不同 DPI 下不明确，实测会把副屏上
/// 的点判成主屏。
fn monitor_area_under(win: &WebviewWindow, point: (f64, f64)) -> ((f64, f64, f64, f64), f64) {
    let monitors = win.available_monitors().unwrap_or_default();
    let rects: Vec<(f64, f64, f64, f64)> = monitors
        .iter()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            (p.x as f64, p.y as f64, s.width as f64, s.height as f64)
        })
        .collect();
    let picked = monitor_index_at(&rects, point)
        .and_then(|i| monitors.get(i).cloned())
        .or_else(|| win.current_monitor().ok().flatten());
    match picked {
        Some(m) => {
            let a = m.work_area();
            (
                (
                    a.position.x as f64,
                    a.position.y as f64,
                    a.size.width as f64,
                    a.size.height as f64,
                ),
                m.scale_factor(),
            )
        }
        None => (
            (0.0, 0.0, 1920.0, 1080.0),
            win.scale_factor().unwrap_or(1.0).max(0.001),
        ),
    }
}

/// 拖拽用的指针（**物理 px**）：默认取系统光标；测试/调试可显式传点
/// （多屏 + 全屏游戏会 ClipCursor/抢光标，自动化只能靠显式点复现跨屏）。
fn drag_pointer(
    win: &WebviewWindow,
    override_pt: Option<(f64, f64)>,
) -> Result<(f64, f64), String> {
    match override_pt {
        Some(p) => Ok(p),
        None => win
            .cursor_position()
            .map(|c| (c.x, c.y))
            .map_err(|e| format!("读取光标位置失败: {e}")),
    }
}

fn opt_point(x: Option<f64>, y: Option<f64>) -> Option<(f64, f64)> {
    match (x, y) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => None,
    }
}

/// 拖拽开始：登记指针在条长方向的比例位置，并置 DRAGGING（Moved 挂点/
/// 吸附守护让位）。前端在收起抽屉与画布之后调用，保证拖的是 68×条长 的
/// 收缩窗而不是整块透明画布。
/// 指针默认取系统光标（物理坐标）：跨屏拖拽时前端事件的 screenX 在各屏
/// 缩放比不同时口径不一，只有 GetCursorPos 是全局一致的。
#[tauri::command]
pub async fn hud_bar_drag_begin(
    pointer_x: Option<f64>,
    pointer_y: Option<f64>,
    app: AppHandle,
) -> Result<HudDragState, String> {
    let win = app
        .get_webview_window(HUD_BAR_WINDOW_LABEL)
        .ok_or_else(|| "停靠条窗口不存在".to_string())?;
    let (px, py) = drag_pointer(&win, opt_point(pointer_x, pointer_y))?;
    let pos = win
        .outer_position()
        .map_err(|e| format!("读取停靠条位置失败: {e}"))?;
    let size = win
        .outer_size()
        .map_err(|e| format!("读取停靠条尺寸失败: {e}"))?;
    let scale = win.scale_factor().unwrap_or(1.0).max(0.001);
    let edge = current_edge();
    // 指针在条长方向的比例位置（换边后同一条目仍停在指针下）
    let frac = match edge {
        BarEdge::Top if size.width > 0 => {
            ((px - pos.x as f64) / size.width as f64).clamp(0.0, 1.0)
        }
        BarEdge::Top => 0.5,
        _ if size.height > 0 => ((py - pos.y as f64) / size.height as f64).clamp(0.0, 1.0),
        _ => 0.5,
    };
    *drag_session_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(HudDragSession { frac });
    DRAGGING.store(true, Ordering::Relaxed);
    LAST_MOVED_AT.store(0, Ordering::Relaxed);
    Ok(drag_state_of(
        edge,
        (
            pos.x as f64 / scale,
            pos.y as f64 / scale,
            size.width as f64 / scale,
            size.height as f64 / scale,
        ),
        true,
    ))
}

/// 拖拽一步：读系统光标 → 取**光标所在屏**的工作区 → 算落点（贴边/换向）。
/// 窗口几何只在变化时下发，16ms 一拍也不会引发无谓重排。
#[tauri::command]
pub async fn hud_bar_drag_move(
    pointer_x: Option<f64>,
    pointer_y: Option<f64>,
    app: AppHandle,
) -> Result<HudDragState, String> {
    let session = *drag_session_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .ok_or_else(|| "没有进行中的拖拽".to_string())?;
    if !DRAGGING.load(Ordering::Relaxed) {
        return Err("没有进行中的拖拽".into());
    }
    let win = app
        .get_webview_window(HUD_BAR_WINDOW_LABEL)
        .ok_or_else(|| "停靠条窗口不存在".to_string())?;
    let (px, py) = drag_pointer(&win, opt_point(pointer_x, pointer_y))?;
    let pos = win
        .outer_position()
        .map_err(|e| format!("读取停靠条位置失败: {e}"))?;
    let size = win
        .outer_size()
        .map_err(|e| format!("读取停靠条尺寸失败: {e}"))?;
    let (area, scale) = monitor_area_under(&win, (px, py));
    let edge = current_edge();
    let (next_edge, x, y, w, h) =
        drag_geometry(edge, item_count(), (px, py), session.frac, area, scale);
    let moved = (x - pos.x as f64).abs() > 0.5 || (y - pos.y as f64).abs() > 0.5;
    let resized = (w - size.width as f64).abs() > 0.5 || (h - size.height as f64).abs() > 0.5;
    if resized {
        let _ = win.set_size(PhysicalSize::new(
            w.round().max(1.0) as u32,
            h.round().max(1.0) as u32,
        ));
    }
    if moved || resized {
        let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    }
    if next_edge != edge {
        CURRENT_EDGE.store(next_edge.index(), Ordering::Relaxed);
        push_layout(&app, next_edge);
    }
    Ok(drag_state_of(
        next_edge,
        (x / scale, y / scale, w / scale, h / scale),
        true,
    ))
}

/// 拖拽结束：按最后光标位置吸附落盘（`edge` + 逻辑坐标 pos）并推布局事件。
#[tauri::command]
pub async fn hud_bar_drag_end(
    pointer_x: Option<f64>,
    pointer_y: Option<f64>,
    app: AppHandle,
) -> Result<HudDragState, String> {
    let session = drag_session_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    DRAGGING.store(false, Ordering::Relaxed);
    LAST_MOVED_AT.store(0, Ordering::Relaxed);
    let win = app
        .get_webview_window(HUD_BAR_WINDOW_LABEL)
        .ok_or_else(|| "停靠条窗口不存在".to_string())?;
    let (px, py) = drag_pointer(&win, opt_point(pointer_x, pointer_y)).unwrap_or((0.0, 0.0));
    let pos = win
        .outer_position()
        .map_err(|e| format!("读取停靠条位置失败: {e}"))?;
    let size = win
        .outer_size()
        .map_err(|e| format!("读取停靠条尺寸失败: {e}"))?;
    let (area, scale) = monitor_area_under(&win, (px, py));
    let edge = current_edge();
    // 结束按最后一次光标位置再算一遍并落盘（拖动中每一步已经贴边，这里
    // 主要是把最终几何与配置对齐）
    let (next_edge, x, y, w, h) = match session {
        Some(s) => drag_geometry(edge, item_count(), (px, py), s.frac, area, scale),
        None => {
            // 无会话（异常路径）：按当前几何落盘
            (
                edge,
                pos.x as f64,
                pos.y as f64,
                size.width as f64,
                size.height as f64,
            )
        }
    };
    let _ = win.set_size(PhysicalSize::new(
        w.round().max(1.0) as u32,
        h.round().max(1.0) as u32,
    ));
    let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    CURRENT_EDGE.store(next_edge.index(), Ordering::Relaxed);
    // 程序侧落点登记（逻辑坐标，与 Moved 挂点同口径）：Moved 回声不触发吸附
    if let Ok(mut guard) = snap_pos_slot().lock() {
        *guard = Some((x / scale, y / scale));
    }
    let mut cfg = load_hud_bar_config();
    cfg.edge = next_edge.key().to_string();
    // 配置里的 pos 是逻辑坐标（建窗 builder.position 同口径）
    cfg.pos = Some((x / scale, y / scale));
    let _ = save_hud_bar_config(&cfg);
    push_layout(&app, next_edge);
    Ok(drag_state_of(
        next_edge,
        (x / scale, y / scale, w / scale, h / scale),
        true,
    ))
}

/// 强制数据线程立即重推一帧（页面三路 listen 注册完成后 invoke 一次：
/// 窗口销毁重开/页面重载时数据线程按"内容未变"静默跳过，重推保证首帧
/// 必达——线程 200ms 内响应 FORCE_PUSH，数据已有缓存时零 HTTP）。同时
/// 补推一次布局事件：建窗时 push_layout 的 layout 事件可能早于前端
/// listen 注册而丢失（refresh 只补数据不补布局的话，右贴边恢复的窗口
/// 前端 barEdge 会以默认 left 运行、hover 卡放错边直到下次吸附）。
/// 同步命令只写原子标志 + 纯 emit（无窗口操作），无死锁风险；app 参数
/// 由 Tauri 自动注入，前端 invoke 无参不变。
#[tauri::command]
pub fn hud_bar_refresh(app: AppHandle) {
    FORCE_PUSH.store(true, Ordering::Relaxed);
    push_layout(&app, current_edge());
}

// ============================================================
// 单元测试（纯函数部分：吸附几何 / payload 组装 / 配置收敛）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_quota::{ProviderQuotaEntry, ProviderQuotaWindow};
    use crate::quota::{QuotaLimit, QuotaResult};

    /// zai 窗口构造辅助（percentage 为已用百分比）
    fn zai_limit(pct: u32, reset: Option<i64>) -> Option<QuotaLimit> {
        Some(QuotaLimit {
            kind: "TOKENS_LIMIT".into(),
            unit: 3,
            number: 5,
            percentage: pct,
            next_reset_time: reset,
            current_value: None,
            usage: None,
            usage_details: None,
        })
    }

    // ---- 吸附最近边判定 ----

    #[test]
    fn nearest_edge_明显靠左不吸顶() {
        // 窗口中心 (40, 100)：到左 40 < 到顶 100 < 到右，应贴左而非顶
        assert_eq!(nearest_edge(1920.0, 1080.0, 40.0, 100.0), BarEdge::Left);
    }

    #[test]
    fn nearest_edge_靠右() {
        // 中心距右缘 60，距顶 300：贴右
        assert_eq!(nearest_edge(1920.0, 1080.0, 1860.0, 300.0), BarEdge::Right);
    }

    #[test]
    fn nearest_edge_靠顶() {
        // 中心距顶 30，距左 400：贴顶
        assert_eq!(nearest_edge(1920.0, 1080.0, 400.0, 30.0), BarEdge::Top);
    }

    #[test]
    fn nearest_edge_底部不参与() {
        // 中心贴屏幕底缘（cy=1070）：底部（距离 10）即使最近也不吸——
        // 枚举里没有底边，按左/右/顶三边中最近者（左 500 < 顶 1070）贴左
        assert_eq!(
            nearest_edge(1920.0, 1080.0, 500.0, 1070.0),
            BarEdge::Left
        );
    }

    #[test]
    fn nearest_edge_屏幕中心左右并列取左() {
        // 到左/右距离相等（500）且都小于到顶（600）：并列优先左
        assert_eq!(nearest_edge(1000.0, 1200.0, 500.0, 600.0), BarEdge::Left);
    }

    #[test]
    fn nearest_edge_拖出屏幕下限为0() {
        // 中心拖到屏幕左外（cx=-30）：左距离钳 0，贴左
        assert_eq!(nearest_edge(1920.0, 1080.0, -30.0, 500.0), BarEdge::Left);
    }

    // ---- 吸附落点 ----

    #[test]
    fn snap_target_左右贴边竖排顶贴边横排() {
        // scale=1 时物理 == 逻辑。贴左：x=0、竖排尺寸（宽 = 条厚）；
        // 1 条目长度 12+16+42 = 70 < 下限 96 取下限
        let (w, h, x, y) =
            snap_target(BarEdge::Left, 1920.0, 1080.0, 333.0, 222.0, 1, 1.0);
        assert_eq!((x, y), (0.0, 222.0)); // 垂直位置保持拖放落点
        assert_eq!(w, BAR_THICK);
        assert_eq!(h, BAR_MIN_LEN);
        // 3 条目长度 158 > 下限：留白 12 + 面板内边距 16 + 3×42 + 2×2
        let (_, h3, _, _) = snap_target(BarEdge::Left, 1920.0, 1080.0, 0.0, 0.0, 3, 1.0);
        let h3_expect =
            BAR_PAD * 2.0 + PANEL_PAD * 2.0 + 3.0 * ITEM_LEN + 2.0 * ITEM_GAP;
        assert!((h3 - h3_expect).abs() < f64::EPSILON);
        // 贴右：x = 屏宽 − 条厚
        let (_, _, x, _) =
            snap_target(BarEdge::Right, 1920.0, 1080.0, 1500.0, 222.0, 2, 1.0);
        assert_eq!(x, 1920.0 - BAR_THICK);
        // 贴顶：y=0、横排高 = BAR_THICK_TOP；宽按收起态条目宽（40）计算
        let (w, h, x, y) = snap_target(BarEdge::Top, 1920.0, 1080.0, 333.0, 5.0, 3, 1.0);
        assert_eq!((y, h), (0.0, BAR_THICK_TOP));
        let w_expect = BAR_PAD * 2.0
            + PANEL_PAD * 2.0
            + 3.0 * ITEM_W_COLLAPSED
            + 2.0 * ITEM_GAP;
        assert!((w - w_expect).abs() < f64::EPSILON);
        assert_eq!(x, 333.0); // 水平位置保持拖放落点
    }

    #[test]
    fn snap_target_高dpi按scale换算物理尺寸() {
        // 150% DPI：条厚/条长 ×1.5，贴右 x = 屏宽物理 − 条厚物理
        let (w, h, x, _) =
            snap_target(BarEdge::Right, 1920.0, 1080.0, 1500.0, 222.0, 3, 1.5);
        let h_expect =
            BAR_PAD * 2.0 + PANEL_PAD * 2.0 + 3.0 * ITEM_LEN + 2.0 * ITEM_GAP;
        assert_eq!(w, BAR_THICK * 1.5);
        assert_eq!(h, h_expect * 1.5);
        assert_eq!(x, 1920.0 - BAR_THICK * 1.5);
    }

    /// 跨屏吸附数值流（snap_to_edge 闭包内的坐标平移步骤逐段复现，纯函数
    /// 层面验证"全局→本屏局部→判定→落点加回偏移"的多显示器路径）：
    /// 主屏 1920×1080 在左，副屏 1280×720 在其右侧（origin x=1920）。
    #[test]
    fn 吸附几何_跨屏窗口应贴所在屏而非主屏() {
        let mon_x = 1920.0;
        let mon_w = 1280.0;
        let mon_h = 720.0;
        // 用例 1：窗口中心在副屏左半偏下（全局 cx=2500 → 局部 580），距顶
        // 最近 → 贴副屏顶（若错误地按全局坐标与主屏宽比较，cx=2500 > 1920
        // 会被钳 0 误判贴右并弹回主屏右缘）
        let cx_local = 2500.0 - mon_x; // 580
        let edge = nearest_edge(mon_w, mon_h, cx_local, 150.0);
        assert_eq!(edge, BarEdge::Top);
        let (_tw, th, tx, ty) = snap_target(edge, mon_w, mon_h, cx_local, 300.0, 2, 1.0);
        assert_eq!((tx, ty), (cx_local, 0.0)); // 水平保持落点、贴本屏顶
        // 落点加回副屏 origin：仍在副屏范围内（不弹回主屏）
        let gx = tx + mon_x;
        assert!((mon_x..mon_x + mon_w).contains(&gx));
        assert_eq!(th, BAR_THICK_TOP);
        // 用例 2：窗口中心在副屏右缘附近（局部 cx=1200，距右 80）→ 贴副屏
        // 右缘，全局落点 = 1920 + (1280 − 100)，不会落在主屏
        let edge = nearest_edge(mon_w, mon_h, 1200.0, 400.0);
        assert_eq!(edge, BarEdge::Right);
        let (_, _, tx, _) = snap_target(edge, mon_w, mon_h, 1200.0, 400.0, 2, 1.0);
        assert_eq!(tx + mon_x, mon_x + mon_w - BAR_THICK);
        // 用例 3：窗口中心在副屏左缘附近（局部 cx=60）→ 贴副屏左缘（全局
        // x = 1920，即两屏交界处，副屏自己的左缘）
        let edge = nearest_edge(mon_w, mon_h, 60.0, 400.0);
        assert_eq!(edge, BarEdge::Left);
        let (tw, _, tx, _) = snap_target(edge, mon_w, mon_h, 60.0, 400.0, 2, 1.0);
        assert_eq!(tx, 0.0);
        assert_eq!(tx + mon_x, mon_x);
        assert_eq!(tw, BAR_THICK);
    }

    #[test]
    fn bar_metrics_单条目取下限与横竖排栅格() {
        // 单条目长度 12+16+42 = 70 < 下限 96：竖排取 96 高
        let (w, h) = bar_metrics(BarEdge::Left, 1);
        assert_eq!((w, h), (BAR_THICK, BAR_MIN_LEN));
        // 零条目按 1 条（空态占位）处理
        let (_, h0) = bar_metrics(BarEdge::Left, 0);
        assert_eq!(h0, BAR_MIN_LEN);
        // 4 条目：留白 12 + 内边距 16 + 4×42 + 3×2 = 202
        let (_, h4) = bar_metrics(BarEdge::Left, 4);
        assert!((h4 - 202.0).abs() < f64::EPSILON);
        // 横排收起态 3 条目：留白 12 + 内边距 16 + 3×40 + 2×2 = 152，高 70
        let (wt, ht) = bar_metrics(BarEdge::Top, 3);
        assert!((wt - 152.0).abs() < f64::EPSILON);
        assert_eq!(ht, BAR_THICK_TOP);
    }

    #[test]
    fn card_placement_按停靠边挂到条外侧并钳进工作区() {
        // scale=1：条窗口 100×334 落在 (1000,300)，工作区 1920×1080
        let area = (0.0, 0.0, 1920.0, 1080.0);
        let bar = (1000.0, 300.0, 68.0, 334.0);
        // 贴右：卡片在面板左侧，右缘贴条窗口左缘（与面板留 8 间隙）
        assert_eq!(
            card_placement(BarEdge::Right, bar, 0.0, 500.0, 304.0, 300.0, area, 1.0),
            (694.0, 350.0)
        );
        // 贴左：卡片在面板右侧 = 窗口右缘外 8px
        assert_eq!(
            card_placement(BarEdge::Left, bar, 0.0, 500.0, 304.0, 300.0, area, 1.0),
            (1070.0, 350.0)
        );
        // 贴顶：卡片挂在条下方，水平跟随 hover 条目中心（400 → 卡片居中）
        let top_bar = (100.0, 0.0, 320.0, 70.0);
        assert_eq!(
            card_placement(BarEdge::Top, top_bar, 400.0, 40.0, 304.0, 300.0, area, 1.0),
            (248.0, 72.0)
        );
        // 纵向钳位：条目贴底时卡片不出工作区（1080 − 300 − 4 = 776）
        let low_bar = (1000.0, 900.0, 68.0, 334.0);
        assert_eq!(
            card_placement(BarEdge::Right, low_bar, 0.0, 1105.0, 304.0, 300.0, area, 1.0).1,
            776.0
        );
        // 跨屏（工作区原点为负）+ 125% 缩放：贴右时卡片落在副屏内
        let sub = (-3200.0, -103.0, 3200.0, 1720.0);
        let sub_bar = (-85.0, 600.0, 85.0, 417.5);
        let (cx, cy) = card_placement(
            BarEdge::Right,
            sub_bar,
            0.0,
            856.0,
            380.0,
            400.0,
            sub,
            1.25,
        );
        assert!((cx - (-467.5)).abs() < 0.01);
        assert!((cy - 656.0).abs() < 0.01);
    }

    #[test]
    fn drag_geometry_拖动中始终贴最近的边() {
        // 工作区 1920×1080；顶带 = clamp(1080×0.14)=150；7 条目竖排
        // 68×334、横排 320×70
        let area = (0.0, 0.0, 1920.0, 1080.0);
        // 左半边中部：贴左，沿边按抓取比例 0.5 停放
        assert_eq!(
            drag_geometry(BarEdge::Left, 7, (300.0, 500.0), 0.5, area, 1.0),
            (BarEdge::Left, 0.0, 333.0, 68.0, 334.0)
        );
        // 同一次拖动（不松手）到右半边：立刻变右边布局
        assert_eq!(
            drag_geometry(BarEdge::Left, 7, (1500.0, 500.0), 0.5, area, 1.0),
            (BarEdge::Right, 1852.0, 333.0, 68.0, 334.0)
        );
        // 继续拖到顶部：变横排贴顶，沿 x 按比例停放
        assert_eq!(
            drag_geometry(BarEdge::Left, 7, (900.0, 40.0), 0.5, area, 1.0),
            (BarEdge::Top, 740.0, 0.0, 320.0, 70.0)
        );
        // 从顶部拖下来（离开迟滞带）：按中线落左/落右
        assert_eq!(
            drag_geometry(BarEdge::Top, 7, (800.0, 300.0), 0.5, area, 1.0),
            (BarEdge::Left, 0.0, 133.0, 68.0, 334.0)
        );
        assert_eq!(
            drag_geometry(BarEdge::Top, 7, (1200.0, 300.0), 0.5, area, 1.0),
            (BarEdge::Right, 1852.0, 133.0, 68.0, 334.0)
        );
        // 左右迟滞：左在 55% 内保持，越过才换右
        assert_eq!(
            drag_geometry(BarEdge::Left, 7, (1000.0, 500.0), 0.5, area, 1.0).0,
            BarEdge::Left
        );
        assert_eq!(
            drag_geometry(BarEdge::Left, 7, (1100.0, 500.0), 0.5, area, 1.0).0,
            BarEdge::Right
        );
        // 顶部迟滞：已在顶部时 150+40=190 内不离开
        assert_eq!(
            drag_geometry(BarEdge::Top, 7, (900.0, 180.0), 0.5, area, 1.0).0,
            BarEdge::Top
        );
        // 沿边钳位：指针远在右侧时条尾贴工作区右缘（1920−320=1600）
        assert_eq!(
            drag_geometry(BarEdge::Top, 7, (1900.0, 10.0), 0.5, area, 1.0),
            (BarEdge::Top, 1600.0, 0.0, 320.0, 70.0)
        );
    }

    #[test]
    fn monitor_index_at_按物理矩形归属屏幕() {
        // 主屏 0,0 3440×1440；副屏 -3200,-103 3200×1800（物理坐标）
        let mons = [
            (0.0, 0.0, 3440.0, 1440.0),
            (-3200.0, -103.0, 3200.0, 1800.0),
        ];
        assert_eq!(monitor_index_at(&mons, (100.0, 100.0)), Some(0));
        // 负 x 属于副屏（主屏左边界是 0）
        assert_eq!(monitor_index_at(&mons, (-1000.0, 500.0)), Some(1));
        assert_eq!(monitor_index_at(&mons, (-50.0, 500.0)), Some(1));
        assert_eq!(monitor_index_at(&mons, (3439.0, 1439.0)), Some(0));
        // 越界：取中心最近的一块
        assert_eq!(monitor_index_at(&mons, (-4000.0, 500.0)), Some(1));
        assert_eq!(monitor_index_at(&mons, (4000.0, 500.0)), Some(0));
        assert_eq!(monitor_index_at(&[], (0.0, 0.0)), None);
    }

    #[test]
    fn drag_geometry_跨屏拖到副屏贴边() {
        // 副屏工作区（物理）：-3200,-103 3200×1720；scale 1.25
        let area = (-3200.0, -103.0, 3200.0, 1720.0);
        // 7 条目 ×1.25：竖排 85×417.5、横排 400×87.5
        // 指针在副屏右半边 → 贴副屏右边（x = -3200+3200-85）
        let (e, x, y, w, h) = drag_geometry(BarEdge::Left, 7, (-1000.0, 500.0), 0.5, area, 1.25);
        assert_eq!(e, BarEdge::Right);
        assert_eq!((w, h), (85.0, 417.5));
        assert_eq!(x, -85.0);
        assert!((y - 291.25).abs() < 1e-9);
        // 指针在副屏顶部 → 横排贴副屏上沿
        let (e2, x2, y2, w2, h2) =
            drag_geometry(BarEdge::Left, 7, (-1600.0, -80.0), 0.5, area, 1.25);
        assert_eq!(e2, BarEdge::Top);
        assert_eq!((y2, w2, h2), (-103.0, 400.0, 87.5));
        assert_eq!(x2, -3200.0 + 1600.0 - 200.0);
    }

    // ---- payload 组装 ----
    #[test]
    fn zai_windows_小时与周窗口() {
        let result = QuotaResult {
            level: "pro".into(),
            hour5: zai_limit(35, Some(1_700_000_000_000)),
            weekly: zai_limit(7, None),
            mcp: None,
        };
        let ws = zai_windows(&result);
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].key, "hour5");
        assert_eq!(ws[0].used_pct, 35.0);
        assert_eq!(ws[0].reset_at, Some(1_700_000_000_000));
        assert_eq!(ws[0].label_zh, "5h");
        assert_eq!(ws[1].key, "weekly");
        assert_eq!(ws[1].label_zh, "每周");
        assert_eq!(ws[1].label_en, "wk");
    }

    #[test]
    fn zai_windows_缺失窗口跳过() {
        let result = QuotaResult {
            level: String::new(),
            hour5: None,
            weekly: zai_limit(50, None),
            mcp: None,
        };
        let ws = zai_windows(&result);
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].key, "weekly");
    }

    #[test]
    fn rate_limit_windows_三源共用含kimi月窗() {
        // codex/claude：无月窗口
        let ws = rate_limit_windows(Some(10.0), None, Some(93.0), Some(123), None);
        assert_eq!(
            ws.iter().map(|w| w.key.as_str()).collect::<Vec<_>>(),
            vec!["hour5", "weekly"]
        );
        // kimi：月窗口存在时附加在最后
        let ws = rate_limit_windows(
            Some(10.0),
            None,
            Some(93.0),
            None,
            Some((40.0, Some(456))),
        );
        assert_eq!(
            ws.iter().map(|w| w.key.as_str()).collect::<Vec<_>>(),
            vec!["hour5", "weekly", "monthly"]
        );
        assert_eq!(ws[2].label_zh, "每月");
        // 百分比缺失（None）的窗口不入列表
        let ws = rate_limit_windows(None, None, Some(5.0), None, None);
        assert_eq!(ws.len(), 1);
    }

    fn provider_entry(status: &str, windows: Vec<ProviderQuotaWindow>) -> ProviderQuotaEntry {
        ProviderQuotaEntry {
            credential_id: "c1".into(),
            label: "cred".into(),
            status: status.into(),
            windows,
            balance: None,
            plan_name: None,
            message: None,
            updated_at: 0,
        }
    }

    fn provider_window(key: &str, pct: Option<f64>) -> ProviderQuotaWindow {
        ProviderQuotaWindow {
            key: key.into(),
            title: String::new(),
            used_percent: pct,
            used: None,
            total: None,
            unit: None,
            resets_at: Some(99),
        }
    }

    #[test]
    fn provider_windows_跳过错误条目取首个可用() {
        // 首条目 error：跳过；第二条目 ok：全部百分比窗口透出（interval =
        // minimax 的 5 小时会话窗口，label 与 hour5 同文案）
        let entries = vec![
            provider_entry("error", vec![provider_window("weekly", Some(50.0))]),
            provider_entry(
                "ok",
                vec![
                    provider_window("weekly", Some(20.0)),
                    provider_window("interval", Some(60.0)),
                    provider_window("balance_only", None), // 无百分比：过滤
                ],
            ),
        ];
        let ws = provider_windows(&entries).expect("应有窗口");
        assert_eq!(
            ws.iter().map(|w| w.key.as_str()).collect::<Vec<_>>(),
            vec!["weekly", "interval"]
        );
        assert_eq!(ws[1].label_zh, "5h");
        // 全部条目无可用窗口：None（该源本轮不出现）
        let bad = vec![provider_entry("expired", vec![provider_window("weekly", Some(1.0))])];
        assert!(provider_windows(&bad).is_none());
    }

    #[test]
    fn build_usage_只含有数据的源_顺序稳定() {
        let zai = QuotaResult {
            level: String::new(),
            hour5: None,
            weekly: zai_limit(12, None),
            mcp: None,
        };
        let kimi = KimiRateLimits {
            plan_type: None,
            primary_pct: Some(50.0),
            primary_reset_at: None,
            secondary_pct: Some(2.0),
            secondary_reset_at: None,
            booster_balance: None,
            booster_monthly_used: None,
            monthly_pct: None,
            monthly_reset_at: None,
        };
        // codex/claude/opencodego/minimax 全 None（查询失败）：不出现
        let agents = build_usage(Some(&zai), None, None, Some(&kimi), None, None, None);
        assert_eq!(
            agents.iter().map(|a| a.source.as_str()).collect::<Vec<_>>(),
            vec!["zai", "kimi"]
        );
        assert_eq!(agents[1].quotas.len(), 2); // kimi 月窗口未填：仅 5h + 周
        // 全部失败：空列表（前端空态占位）
        assert!(build_usage(None, None, None, None, None, None, None).is_empty());
    }

    #[test]
    fn dsh_agent_取官方余额_无余额不出现() {
        let mut entry = provider_entry("ok", vec![]);
        entry.balance = Some(ProviderQuotaBalance {
            amount: 54.64,
            currency: "CNY".into(),
            granted: Some(4.64),
            topped_up: Some(50.0),
        });
        let agent = dsh_agent(std::slice::from_ref(&entry)).expect("有余额应出现");
        assert_eq!(agent.source, "dsh");
        assert!(agent.quotas.is_empty(), "余额型条目不带百分比窗口");
        let balance = agent.balance.expect("余额透传");
        assert_eq!(balance.amount, 54.64);
        assert_eq!(balance.currency, "CNY");
        assert_eq!(balance.granted, Some(4.64));
        // 失败条目 / 无余额条目：不出条目（由 fill_placeholders 补占位）
        assert!(dsh_agent(&[provider_entry("expired", vec![])]).is_none());
        assert!(dsh_agent(&[provider_entry("ok", vec![])]).is_none());
        assert!(dsh_agent(&[]).is_none());
        // build_usage 里同样只透出一个余额型条目
        let agents = build_usage(
            None,
            None,
            None,
            None,
            None,
            None,
            Some(std::slice::from_ref(&entry)),
        );
        assert_eq!(
            agents.iter().map(|a| a.source.as_str()).collect::<Vec<_>>(),
            vec!["dsh"]
        );
        assert!(agents[0].balance.is_some());
        assert!(agents[0].quotas.is_empty());
    }

    // ---- 配置收敛 ----

    #[test]
    fn config_clamped_边非法回退左_坐标脏值丢弃() {
        let cfg = HudBarConfig {
            enabled: true,
            edge: "bottom".into(),
            pos: Some((f64::NAN, 3.0)),
        }
        .clamped();
        assert_eq!(cfg.edge, "left");
        assert!(cfg.pos.is_none());
        assert!(cfg.enabled);
        // 合法值原样保留
        let cfg = HudBarConfig {
            enabled: false,
            edge: "top".into(),
            pos: Some((10.0, 20.0)),
        }
        .clamped();
        assert_eq!(cfg.edge, "top");
        assert_eq!(cfg.pos, Some((10.0, 20.0)));
    }

    #[test]
    fn bar_edge_往返序列化() {
        for edge in [BarEdge::Left, BarEdge::Right, BarEdge::Top] {
            assert_eq!(BarEdge::parse(edge.key()), Some(edge));
            assert_eq!(BarEdge::from_index(edge.index()), edge);
        }
        assert_eq!(BarEdge::parse("bottom"), None);
        assert_eq!(BarEdge::Left.orientation(), "vertical");
        assert_eq!(BarEdge::Top.orientation(), "horizontal");
    }
    // ---- 占位准入：没有订阅也没有今日用量的 agent 不出现 ----

    /// 造一个"实时额度正常"的条目（用于验证占位不会和真条目重复）。
    fn quota_agent(source: &str) -> HudBarAgent {
        HudBarAgent {
            source: source.into(),
            plan: None,
            status: None,
            quotas: vec![HudBarQuota {
                key: "hour5".into(),
                label_zh: "5h".into(),
                label_en: "5h".into(),
                used_pct: 10.0,
                reset_at: None,
            }],
            balance: None,
        }
    }

    fn names(agents: &[HudBarAgent]) -> Vec<&str> {
        agents.iter().map(|a| a.source.as_str()).collect()
    }

    /// 造一份"今日用量"快照（准入判定只看 tokens，不校验新鲜度）。
    fn today_usage_with(pairs: &[(&str, i64)]) -> crate::today_usage::TodayUsage {
        crate::today_usage::TodayUsage {
            day_start: 0,
            at: 0,
            tokens: pairs
                .iter()
                .map(|(source, tokens)| ((*source).to_string(), *tokens))
                .collect(),
        }
    }

    /// 今日有用量但没订阅：条目出现，状态是 usage（不是 unavailable）。
    #[test]
    fn 有用量无订阅的条目按用量出现() {
        let visible = BTreeSet::from(["zai".to_string(), "codex".to_string()]);
        let subscribed = HashSet::new();
        let today = today_usage_with(&[("zai", 123)]);
        let mut agents = Vec::new();
        let evidence = PlaceholderEvidence {
            visible: &visible,
            subscribed: &subscribed,
            today: Some(&today),
        };
        fill_placeholders(&mut agents, &evidence);
        assert_eq!(names(&agents), vec!["zai"]);
        assert_eq!(agents[0].status.as_deref(), Some("usage"));
        assert!(agents[0].quotas.is_empty());
    }

    /// 既没订阅也没有今日用量：不出现（旧行为是常驻占位）。
    #[test]
    fn 无订阅且无今日用量不出现() {
        let visible = BTreeSet::from(["zai".to_string(), "codex".to_string()]);
        let subscribed = HashSet::new();
        let today = today_usage_with(&[("zai", 0)]);
        let mut agents = Vec::new();
        let evidence = PlaceholderEvidence {
            visible: &visible,
            subscribed: &subscribed,
            today: Some(&today),
        };
        fill_placeholders(&mut agents, &evidence);
        assert!(agents.is_empty(), "零用量 + 零订阅的来源必须隐藏");
    }

    /// 订阅证据粘滞：本轮额度查询失败（不在 agents 里）也要保留占位，
    /// 状态仍是 unavailable（不是 usage），避免把"订阅在、额度暂时查不到"
    /// 误标成"没有订阅"。
    #[test]
    fn 订阅证据粘滞抵消额度查询失败() {
        let visible = BTreeSet::from(["codex".to_string()]);
        let subscribed = HashSet::from(["codex".to_string()]);
        let today = today_usage_with(&[]);
        let mut agents = Vec::new();
        let evidence = PlaceholderEvidence {
            visible: &visible,
            subscribed: &subscribed,
            today: Some(&today),
        };
        fill_placeholders(&mut agents, &evidence);
        assert_eq!(names(&agents), vec!["codex"]);
        assert_eq!(agents[0].status.as_deref(), Some("unavailable"));
    }

    /// 今日用量快照不可用（托盘线程还没跑完/停摆）：保守放行，退回旧行为，
    /// 不能把「不知道」当成「没用量」把图标藏掉。
    #[test]
    fn 今日用量未知时保守显示占位() {
        let visible = BTreeSet::from(["kimi".to_string()]);
        let subscribed = HashSet::new();
        let mut agents = Vec::new();
        let evidence = PlaceholderEvidence {
            visible: &visible,
            subscribed: &subscribed,
            today: None,
        };
        fill_placeholders(&mut agents, &evidence);
        assert_eq!(names(&agents), vec!["kimi"]);
        assert_eq!(agents[0].status.as_deref(), Some("unavailable"));
    }

    /// 设置页关掉的来源一律不出占位（即使今日有用量）。
    #[test]
    fn 设置关闭的来源不出占位() {
        let visible = BTreeSet::new();
        let subscribed = HashSet::from(["zai".to_string()]);
        let today = today_usage_with(&[("zai", 9)]);
        let mut agents = Vec::new();
        let evidence = PlaceholderEvidence {
            visible: &visible,
            subscribed: &subscribed,
            today: Some(&today),
        };
        fill_placeholders(&mut agents, &evidence);
        assert!(agents.is_empty());
    }

    /// 真额度条目不会被占位重复，且整体按 HUD_SOURCES 固定顺序排
    /// （占位不能全被追加到末尾）。
    #[test]
    fn 占位与真条目不重复且按固定顺序排() {
        let visible = BTreeSet::from([
            "zai".to_string(),
            "codex".to_string(),
            "cursor".to_string(),
        ]);
        let subscribed = HashSet::new();
        let today = today_usage_with(&[("zai", 5)]);
        let mut agents = vec![quota_agent("cursor")];
        let evidence = PlaceholderEvidence {
            visible: &visible,
            subscribed: &subscribed,
            today: Some(&today),
        };
        fill_placeholders(&mut agents, &evidence);
        assert_eq!(names(&agents), vec!["zai", "cursor"]);
    }


}
