//! asar 注入物：主题模板（全部自研）、注入块幂等插入与 variables.css 生成。
//!
//! 注入方式：在 ZCode 的 asar 内 out/renderer/index.html 中，
//! `</head>` 前插入两个外链样式（variables.css + theme.css）、
//! `</body>` 前插入 defer 脚本（effects.js + usage.js + pet.js），全部指向
//! ~/.agenthud/agent-themes/ 下的主题文件（file:// URL，不改动应用其它资源）。
//! 注入的外链带 data-zbar-variables / data-zbar-theme / data-zbar-effects /
//! data-zbar-usage / data-zbar-pet 标记：effects.js 热重载靠 data-zbar-variables 定位
//! variables.css（旧版注入行无 data 属性时回退按 href 匹配，同样可热重载）；
//! usage.js 靠自身 src 推导同目录 usage-data.js 的地址；pet.js 消费
//! usage.js 重载的 window.__ZBAR_USAGE__ 全局数据（只读不重复请求）。
//! 整段引用包裹在 <!--ZBAR-THEME-BEGIN--> … <!--ZBAR-THEME-END--> 标记内，
//! 重复安装时先剥离旧标记块再插入新块，保证幂等。

use crate::agent_theme::store::ThemeParams;
use std::fs;
use std::path::{Path, PathBuf};

/// 注入块起止标记（head 与 body 两处各一个块，剥离时全部移除）
pub const INJECT_BEGIN: &str = "<!--ZBAR-THEME-BEGIN-->";
pub const INJECT_END: &str = "<!--ZBAR-THEME-END-->";

// ============================================================
// 内置模板
// ============================================================

/// 主题样式模板：消费 variables.css 中的 --zbar-* 变量。
/// 版本化落盘（头部 ZBAR-THEME-V 标记，见 store::ensure_versioned_template）：
/// 旧版本文件由 ensure_theme_assets 自动覆盖升级；已是当前版本的文件
/// （可能被实机调优过）不会被覆盖。
pub const THEME_CSS: &str = r#"/* ============================================================
 * ZBAR-THEME-V12
 * ZBar Agent 动态壁纸主题样式（由 ZBar 落盘并随版本升级覆盖）
 * ============================================================
 * V12 变更（壳透明规则变量驱动，壁纸关闭恢复原始整窗底）：V11 的
 * 壳规则原为硬编码 background: transparent !important——壁纸开关的
 * 卸载路径（effects.js）与 variables.css 的空转变量都覆盖不到它，
 * 关闭壁纸后壳底仍被挖空、透出窗口原生底色，用户看到的是"透明"而
 * 非原始外观。V12 把该规则改为消费新变量 --zbar-shell-uncover
 * （variables.css 渲染 1/0：开启壁纸=0 壳全透明，与 V11 行为完全
 * 一致；关闭=1 壳底恢复 --color-background——空转下该 token 已被
 * --zbar-base-alpha=1 还原为原色实色，深浅色自动跟随 .dark 的
 * token 重定义，无需两套规则）。变量缺失兜底 0（透明）：兼容尚未
 * 重渲的旧 variables.css，已装用户升级 theme.css 后保持 V11 观感，
 * 不会被提前挖穿；改规则必须升版（THEME_CSS_VERSION 同步 11→12），
 * 已装用户经 ensure_versioned_template 自动覆盖升级，重启 ZCode 后
 * 生效。
 *
 * V11 变更（透出 Windows 端全视口应用壳容器）：Windows 实机
 * （ZCode 3.10.1.6272 / Electron 41）确认应用壳的全视口 flex 容器
 * （div.flex.h-dvh.flex-col.overflow-hidden）自带不透明底色
 * rgb(43 43 43)（浅色主题为对应浅色值），处于普通文档流层，会把
 * 全部负 z-index 壁纸层（视频 -2 / 压暗遮罩 -1）整体盖住——这是
 * Windows 端"不透明度等参数生效但壁纸永不显示"的直接原因。规则
 * 见文件末尾"V11：透出全视口应用壳容器"；macOS 端该容器本就
 * 透明（或不存在），规则无副作用。
 *
 * 实机结论（解剖 ZCode 自身样式）：界面容器普遍通过工具类
 * （.bg-background / .bg-panel 等）消费设计 token
 * --color-background / --color-background-alt / --color-panel，
 * body 无全局背景。因此本文件不再猜测 DOM 选择器，而是直接把
 * 这些 token 重定义为"原色 × 透明度"的 color-mix 半透明色，
 * 所有消费方一次性透出底下的壁纸视频。
 *
 * V3 变更：侧栏改由专用 token --color-sidebar 承载（ZCode 的
 * .bg-sidebar 工具类引用）。
 *
 * V4 变更：实机进一步确认，主侧栏容器与主内容列自身无背景工具
 * 类（透明，透出全局底色），并不消费上述 token，token 覆盖对
 * 它们无效。故追加元素选择器规则（!important）为这两块主区域
 * 直接刷半透明背景。
 *
 * V5 变更（主题分层重构）：V4 及此前全局 token 由滑块变量驱动，
 * 拖任一滑块会牵连顶栏、右侧面板、卡片等一切全局底消费方；且
 * 侧栏底下透出的也是全局底，两个滑块必然联动。V5 起全部 8 条
 * 全局底色 token（:root 与 .dark 的 background / background-alt /
 * panel / sidebar）改由固定氛围变量 --zbar-base-alpha（variables.css
 * 恒定输出 0.25）驱动，与滑块彻底解绑。
 *
 * V6 变更（对话区作用区修正 + 右栏独立滑块）：实测对话列与右侧
 * 面板同属一个整体主内容容器，V5 把对话区滑块刷在该容器上会牵连
 * 右侧面板。V6 删除该容器的元素规则，改按 ZCode 内
 * react-resizable-panels 的面板属性定位（面板无原生 DOM id，属性名
 * data-panel-id，对话列属性值为 conversation-column）：对话列唯一
 * 作用区为按该属性值反查的元素规则（消费对话区滑块变量）；新增
 * 右栏独立滑块变量 --zbar-sidebar-right-opacity，作用于除对话列外
 * 的全部面板（面板属性 :not 反选，自动覆盖将来新增的面板）。
 * 自此三区域分层：左栏侧栏容器 / 对话列 / 右栏其余面板，三个滑块
 * 各管各的容器、互不牵连。
 *
 * V7 变更（选择器属性名修正）：实机解剖面板组件渲染代码确认，
 * 面板容器 DOM 属性名为 data-pane-id（V6 误写为 data-panel-id，
 * 致对话列与右栏两处选择器全部落空：右栏滑块无效果、对话滑块
 * 作用区丢失）。V7 仅把四处选择器的属性名改为 data-pane-id，
 * 规则结构、滑块变量与三区域分层不变。
 *
 * V8 变更（选择器归属修正）：实机解剖确认对话列面板（面板属性
 * 值 conversation-column）内部还嵌套一层子面板组，子面板同样带
 * 面板属性（对话内容 conversation、终端 terminal）。V7 的右栏
 * :not 反选只排除了外层对话列，把这两个子面板也捞进右栏作用区
 * （右栏滑块实际控制了对话区）；而对话区规则只刷外层面板，内层
 * 子面板的背景盖在其上，看似无效果。V8 把对话区作用区扩为三面
 * 板组（conversation-column / conversation / terminal，消费对话
 * 区滑块变量），右栏 :not 链同步排除这三个面板值，三区域归属
 * 恢复各管各的容器。
 *
 * V9 变更（运行时实测选择器终版修正）：真机运行时检查确认——
 * 当前 ZCode 常驻 UI 中唯一带面板属性的面板是 workspace-main
 * （代码常量之值，覆盖中间对话+输入区主区域）；V8 枚举的对话列
 * 三面板组（conversation-column / conversation / terminal）仅在
 * 多面板视图（repo-wiki 等）展开时才挂载。V7/V8 症状根因：右栏
 * :not 反选把唯一的 workspace-main 捞走（右栏滑块实际控制了对
 * 话区），而对话区规则的 conversation-column 选择器在常驻 UI 无
 * 命中（对话滑块看似无效果）。V9 把对话区作用区扩为四面板组
 * （常驻 workspace-main + 多面板视图的对话列三面板组），右栏
 * :not 链同步排除这四个面板值；右栏"打开标签页"空态选择面板
 * 无面板属性（容器类 side-pane-open-tab-shell），单独并入右栏
 * 作用区，三区域归属彻底归位。
 *
 * V10 变更（文字可读性增强 + 氛围底可调）：--zbar-base-alpha 由
 * 固定常量升级为用户可调参数 base_alpha（variables.css 按参数渲染
 * 真值并随热重载即时生效，本文件中原有的 0.25 兜底写法保留，作
 * 旧 variables.css 的容错）；新增文字描边强度参数 text_shadow，
 * 消费变量 --zbar-text-shadow，文件末尾追加三区域主容器（与上方
 * 主区域背景规则同批选择器）的前景文字描边规则——深色主题补黑色
 * 描边、浅色主题补白色描边，把文字从过亮/过暗的壁纸里托出来；
 * 强度 0 时 alpha 为 0 描边自然不可见，默认观感与 V9 完全一致。
 * 既有 V9 选择器与规则一字未动。
 *
 * 生效前提：本文件 link 晚于 ZCode 自身样式（</head> 前注入，
 * 源顺序占优）；壁纸视频层与压暗遮罩层由 effects.js 挂载。
 * ============================================================ */

/* 根背景透明化兜底：ZCode body 本无全局背景，防止个别版本铺底色 */
html,
body {
  background: transparent !important;
}

/* ---- 浅色 token 半透明化（V5：全局底固定氛围值） ----
 * 引用 neutral 原色而非硬编码色值，跟随 ZCode 自身配色演进；
 * 全局底色 token 统一消费 variables.css 固定输出的氛围变量
 * --zbar-base-alpha（隐约透出壁纸的一层氛围底），与两个滑块
 * 彻底解绑：拖任一滑块都不会再牵连顶栏、右侧面板、卡片等
 * 全局底消费方。 */
:root {
  --color-background: color-mix(
    in oklab,
    var(--color-neutral-50) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  --color-background-alt: color-mix(
    in oklab,
    var(--color-neutral-100) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  --color-panel: color-mix(
    in oklab,
    #fff calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  /* 侧栏专用 token（浅色取 neutral-100，同 ZCode 原始定义）：
     V5 起与其余全局底色 token 一并由固定氛围变量驱动，
     不再消费任何滑块变量 */
  --color-sidebar: color-mix(
    in oklab,
    var(--color-neutral-100) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
}

/* ---- 深色 token 半透明化 ----
 * .dark 与 :root 同特异性 (0,1,0)，本文件靠源顺序靠后胜出。 */
.dark {
  --color-background: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  --color-background-alt: color-mix(
    in oklab,
    var(--color-neutral-800) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  --color-panel: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  /* 侧栏专用 token（深色取 neutral-950，同 ZCode .dark 原始定义），
     同样由固定氛围变量驱动，不再消费任何滑块变量 */
  --color-sidebar: color-mix(
    in oklab,
    var(--color-neutral-950) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
}

/* ---- V6 三区域分层说明 ----
 * 全局底色 token（background / background-alt / panel / sidebar）
 * 与滑块彻底解绑，统一由固定氛围变量提供"隐约透出壁纸"的一层
 * 氛围底；左栏 / 对话列 / 右栏三个滑块各自只驱动一个主容器的
 * 元素规则（见下方主区域规则），互不牵连。顶栏、内容卡片等
 * 其余区域消费全局 token，永远保持固定的氛围透明度。 */

/* ---- 主区域元素级半透明化（V6：三区域分层，滑块的唯一作用区） ----
 * 左栏与主内容区内的各面板原本自身无背景（透明，透出全局底色），
 * 全局 token 覆盖对它们无效，这里按元素直接刷半透明背景。V6 起按
 * 面板属性定位（react-resizable-panels 实例，无原生 DOM id），三区域
 * 各只消费自己的滑块变量、互不串味：
 * - 左栏侧栏容器 → 侧栏滑块变量；
 * - 对话区四面板组（V9：常驻主面板 workspace-main + 多面板视图
 *   才挂载的对话列面板组 conversation-column / conversation /
 *   terminal，子面板同带面板属性，须一并归入对话区）→ 对话区
 *   滑块变量；
 * - 右栏其余面板（面板属性 :not 反选排除上述四面板）与右栏"打开
 *   标签页"空态选择面板（无面板属性，按容器类定位）→ 右栏滑块
 *   变量，反选自动覆盖将来新增的面板，无需枚举。
 * 基准原色与全局底色一致（浅色 neutral-50、深色 neutral-900），保持
 * 区域间色彩连贯。深浅两套均加 !important 压过 ZCode 源样式。 */
#sidebar {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-50) calc(var(--zbar-sidebar-opacity, 0) * 100%),
    transparent
  ) !important;
}

html.dark #sidebar {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-sidebar-opacity, 0) * 100%),
    transparent
  ) !important;
}

/* V9：对话区作用区为四面板组——常驻主面板 workspace-main（当前
 * 常驻 UI 中唯一带面板属性的面板，覆盖中间对话+输入区主区域）+
 * 多面板视图展开时才挂载的对话列三面板组（外层 conversation-column
 * 与其内部子面板 conversation / terminal）。缺一则其余面板会被
 * 下方右栏 :not 反选捞走（归属串味） */
[data-pane-id="workspace-main"],
[data-pane-id="conversation-column"],
[data-pane-id="conversation"],
[data-pane-id="terminal"] {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-50) calc(var(--zbar-panel-opacity, 0) * 100%),
    transparent
  ) !important;
}

html.dark [data-pane-id="workspace-main"],
html.dark [data-pane-id="conversation-column"],
html.dark [data-pane-id="conversation"],
html.dark [data-pane-id="terminal"] {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-panel-opacity, 0) * 100%),
    transparent
  ) !important;
}

/* V9：:not 链排除对话区四面板组（常驻主面板 + 对话列三面板组），
 * 其余面板全部归右栏滑块；右栏"打开标签页"空态选择面板无面板
 * 属性，按其容器类并入右栏作用区 */
[data-pane-id]:not([data-pane-id="workspace-main"]):not([data-pane-id="conversation-column"]):not([data-pane-id="conversation"]):not([data-pane-id="terminal"]),
.side-pane-open-tab-shell {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-50) calc(var(--zbar-sidebar-right-opacity, 0) * 100%),
    transparent
  ) !important;
}

html.dark [data-pane-id]:not([data-pane-id="workspace-main"]):not([data-pane-id="conversation-column"]):not([data-pane-id="conversation"]):not([data-pane-id="terminal"]),
html.dark .side-pane-open-tab-shell {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-sidebar-right-opacity, 0) * 100%),
    transparent
  ) !important;
}

/* ---- 文字可读性 ----
 * 深色壁纸上浅色文字发虚时，优先在 ZBar 面板调大遮罩强度
 * （--zbar-mask-strength，遮罩层由 effects.js 挂载并热重载更新），
 * 压暗壁纸提升前景对比度。
 * V10：已追加独立于遮罩的文字描边能力（强度变量 --zbar-text-shadow，
 * 用户可调），规则见文件末尾"文字可读性：壁纸过亮/过暗时给前景
 * 文字补描边"块。 */

/* ---- 文字可读性：壁纸过亮/过暗时给前景文字补描边（V10） ----
 * 壁纸以氛围底透出后，亮色壁纸配深色主题时白色文字、深色壁纸配
 * 浅色主题时深色文字都可能失去对比度；遮罩强度只能整体压暗/提亮，
 * 这里按主题方向对称补描边，把文字从背景里"托"出来：
 * - 深色主题（html.dark）给三区域主容器补黑色描边，浅色主题对称
 *   补白色描边；
 * - 强度全部消费 --zbar-text-shadow（用户可调 0~1，0=关闭；
 *   variables.css 按参数渲染真值并随热重载即时生效），alpha=0 时
 *   描边自然不可见，无需条件分支；
 * - 模糊半径随强度温和缩放（3px 起步，最高 +5px），低强度下边缘
 *   柔和不产生图标毛边；
 * - 选择器组与上方主区域背景规则同批（侧栏容器 + 对话区四面板组
 *   + 右栏其余面板与空态选择面板），描边设在容器上由文字继承，
 *   与背景三区域分层一一对应，不影响任何 V9 规则。 */
#sidebar,
[data-pane-id="workspace-main"],
[data-pane-id="conversation-column"],
[data-pane-id="conversation"],
[data-pane-id="terminal"],
[data-pane-id]:not([data-pane-id="workspace-main"]):not([data-pane-id="conversation-column"]):not([data-pane-id="conversation"]):not([data-pane-id="terminal"]),
.side-pane-open-tab-shell {
  text-shadow: 0 0 calc(3px + (var(--zbar-text-shadow, 0) * 5px))
    rgba(255, 255, 255, var(--zbar-text-shadow, 0));
}

html.dark #sidebar,
html.dark [data-pane-id="workspace-main"],
html.dark [data-pane-id="conversation-column"],
html.dark [data-pane-id="conversation"],
html.dark [data-pane-id="terminal"],
html.dark [data-pane-id]:not([data-pane-id="workspace-main"]):not([data-pane-id="conversation-column"]):not([data-pane-id="conversation"]):not([data-pane-id="terminal"]),
html.dark .side-pane-open-tab-shell {
  text-shadow: 0 0 calc(3px + (var(--zbar-text-shadow, 0) * 5px))
    rgba(0, 0, 0, var(--zbar-text-shadow, 0));
}

/* ---- V12：全视口应用壳容器（壳底透明/恢复由变量驱动） ----
 * 见文件头部 V12 变更说明。该容器铺满整窗且自带不透明底色，V11 起
 * 必须由本规则决定壳底去向；V12 改为消费 --zbar-shell-uncover 的
 * color-mix 形式：开启壁纸（变量=0，缺失兜底 0 兼容旧 variables.css）
 * 时透明全量为 100%，壳整体透明、负 z-index 的壁纸视频/压暗遮罩层
 * 按原设计垫底透出（与 V11 硬编码 transparent 行为完全一致）；关闭
 * 壁纸（=1）时壳底恢复 --color-background 不透明底——空转组合下该
 * token 已被 --zbar-base-alpha=1 还原为原色实色，整体回到原始整窗
 * 观感。氛围底与面板半透明质感仍由上方既有 token/元素规则负责。 */
div.flex.h-dvh.flex-col.overflow-hidden {
  background: color-mix(in oklab, var(--color-background, #fff) calc(var(--zbar-shell-uncover, 0) * 100%), transparent) !important;
}
"#;

/// 壁纸运行时脚本模板：读取 --zbar-* CSS 变量，在 body 上挂黑底占位层、
/// 壁纸媒体层（视频或图片，按壁纸扩展名二选一）与压暗遮罩层，并每秒
/// 热重载 variables.css——AgentHUD 面板改参数/换壁纸无需重启 ZCode 即时生效。
/// theme.css 为静态 link 不做周期热重载（模板升级场景由面板"重启 ZCode"
/// 按钮冷启动完全重载，见模板内 V5 说明）。
/// V6 起新增壁纸层总开关（消费 --zbar-wallpaper-enabled，默认开启）：
/// 关闭时移除注入三层、背景回到主题纯色底，重新开启按最新指向恢复；
/// off 只拦壁纸相关应用，variables.css 的每秒热重载照常（见模板内
/// V6 说明）。
/// 版本化落盘（头部 ZBAR-THEME-V 标记，见 store::ensure_versioned_template）。
pub const EFFECTS_JS: &str = r#"// ============================================================
// ZBAR-THEME-V6
// ZBar Agent 动态壁纸运行时（由 ZBar 落盘并随版本升级覆盖）
// ============================================================
// 读取 variables.css 注入的 --zbar-* CSS 变量，在 body 上创建：
//   - 黑底占位层（z-index:-2）：媒体就绪前垫底，防止加载期闪白
//   - 壁纸媒体层（z-index:-2）：按壁纸 URL 扩展名二选一——
//       .mp4/.webm/.mov → video（muted/loop/playsinline/autoplay，
//         object-fit:cover，canplay 后淡入）
//       .jpg/.jpeg/.png/.webp → img（object-fit:cover，onload 后淡入）
//     视频↔图片切换类型时移除旧元素重建；加载失败静默移除全部注入
//     元素，退回原生观感
//   - 压暗遮罩层（z-index:-1）：rgba(0,0,0,强度)，保证前景可读
// 滤镜（亮度/饱和/模糊）与遮罩对视频和图片同样生效；播放速率仅视频。
// V6 变更（壁纸层总开关）：消费新增变量 --zbar-wallpaper-enabled
// （variables.css 渲染 1/0，用户参数 wallpaper_enabled，默认 1）——显式
// "0" 时移除占位/媒体/遮罩三层并记住 off 态，背景回到 theme.css 的
// base_alpha 纯色氛围底（面板不透明度等参数照常生效，用量统计条等其余
// 注入功能完全不受影响）；重新置 1 后按快照里的最新指向恢复挂载（显式
// 开启视为一次显式重试：坏源也重建重载，自动自愈路径仍受 dead 约束），
// 壁纸指向与滤镜参数原样保留。off 是用户意图，与"加载失败 dead"态严格
// 区分：dead 需换源才复位、期间不自愈重挂；off 期间同样不自愈重挂（用户
// 意图优先，自愈分支以 wallpaperOn && !wallpaperOff 双前提判定——只看
// wallpaperOn 会被每秒 reloadVarsLink 的失效窗口读到空串误判为开启而抢先
// 重挂），但重新开启即恢复。开关判定
// 置于快照与 reloadVarsLink 之后（off 只拦壁纸相关应用，不拦 variables.css
// 每秒 cache-bust——那是 usage.js 等消费方热重载的唯一通道，其余参数的
// 热重载在关闭壁纸期间照常）。变量缺失/空串（旧 variables.css）一律视为
// 开启，且该变量被排除出 poll 的"任一变量空串即视为失效窗口"防御——
// 旧文件无此变量恒为空串，若参与判定会让整轮轮询永久跳过，面板其余
// 参数的热重载会一并停摆。
// 热重载：每 1000ms 强制重读 variables.css（link href 追加时间戳，
// Chromium 对 file:// 的常用强制重读手段），比对 --zbar-* 变量快照，
// 壁纸 URL / 滤镜三参 / 播放速率 / 遮罩强度变化即时应用。
// theme.css 不做周期热重载（V4 曾与 variables.css 同款每秒 cache-bust，
// V5 撤销）：样式表 href 变更会经历"旧样式表卸载失效 → 异步加载解析
// → 恢复"窗口，失效窗口内三区域背景/文字描边规则整体失效，背景闪回
// ZCode 原生底色，系统忙时窗口拉长到肉眼可见的周期性闪烁。theme.css
// 模板升级（版本化覆盖落盘）改由面板"重启 ZCode"按钮冷启动完全重载。
// 兼容：定位 variables.css 优先 link[data-zbar-variables]（新版注入行），
// 旧版注入行（无 data 属性）回退按 href 含 "variables.css" 匹配——
// 旧 asar 注入无需重装主题也能热重载。
// ============================================================
(function () {
  "use strict";

  if (!document.body) return;

  var VAR_NAMES = [
    "--zbar-wallpaper-url",
    "--zbar-wp-brightness",
    "--zbar-wp-saturate",
    "--zbar-wp-blur",
    "--zbar-mask-strength",
    "--zbar-playback-rate",
    "--zbar-wallpaper-enabled"
  ];

  /* 壁纸层总开关变量名（V6 新增）：取值语义与 usage_* 开关同惯例——
   * 仅显式 "0" 视为关闭，空串/缺失（旧 variables.css）一律按开启处理；
   * 单独持有常量，供 poll 的空值防御排除判定与专项读取使用 */
  var VAR_WALLPAPER_ENABLED = "--zbar-wallpaper-enabled";

  function cssVar(name, fallback) {
    var v = getComputedStyle(document.documentElement).getPropertyValue(name);
    v = (v || "").trim();
    return v || fallback;
  }

  function num(value, fallback) {
    var n = parseFloat(value);
    return isFinite(n) ? n : fallback;
  }

  /* 从 url("file://…") 形式的变量值中提取纯地址再交给媒体元素 src */
  function urlOf(value) {
    var m = /^\s*url\(\s*(['"]?)(.*?)\1\s*\)\s*$/.exec(value || "");
    return m ? m[2] : "";
  }

  /* 定位注入的 variables.css link：新版注入行带 data-zbar-variables；
   * 旧版注入行（无 data 属性）靠 href 含 variables.css 兜底匹配 */
  function findVarsLink() {
    return (
      document.querySelector("link[data-zbar-variables]") ||
      document.querySelector('link[href*="variables.css"]')
    );
  }

  /* 壁纸类型：按 URL 扩展名判定（file_url 的 percent-encoding 不影响
   * 扩展名字符）；未知扩展按视频兜底（与旧版行为一致） */
  function kindOf(url) {
    var u = (url || "").toLowerCase().split("?")[0];
    if (/\.(mp4|webm|mov)$/.test(u)) return "video";
    if (/\.(jpe?g|png|webp)$/.test(u)) return "image";
    return "video";
  }

  /* ---- 注入的常驻层：黑底占位与压暗遮罩 ---- */
  var placeholder = document.createElement("div");
  placeholder.setAttribute("data-zbar-wallpaper", "placeholder");
  placeholder.style.cssText =
    "position:fixed;top:0;left:0;width:100%;height:100%;" +
    "z-index:-2;background:#000;pointer-events:none;";

  var mask = document.createElement("div");
  mask.setAttribute("data-zbar-wallpaper", "mask");
  mask.style.cssText =
    "position:fixed;top:0;left:0;width:100%;height:100%;" +
    "z-index:-1;pointer-events:none;";

  /* ---- 壁纸媒体层（video 或 img，按类型二选一） ---- */
  var media = null; /* 当前媒体元素 */
  var mediaKind = ""; /* "video" | "image" */
  var currentUrl = "";
  var dead = false; /* 当前壁纸加载失败标记：不自动重试同源，换源后复位 */
  var wallpaperOff = false; /* 壁纸层总开关关闭态（V6 用户意图）：与 dead
    （加载失败）独立——off 期间不自愈重挂，重新开启即按已知指向恢复
    （显式开启视为显式重试，坏源也重建重载；自动自愈仍受 dead 约束） */
  var placeholderTimer = 0;

  /* 就绪（video canplay / img load）：应用速率（仅视频）、淡入并撤黑底 */
  function onReady() {
    if (mediaKind === "video") {
      applyRate();
      var p = media.play();
      if (p && p.catch) {
        p.catch(function () {
          /* 自动播放被拦截时保持首帧，不报错 */
        });
      }
    }
    media.style.opacity = "1"; /* CSS 过渡淡入（可重复调用） */
    /* 淡入完成后再撤黑底，全程不闪白 */
    if (placeholderTimer) clearTimeout(placeholderTimer);
    placeholderTimer = setTimeout(function () {
      if (placeholder.parentNode) placeholder.parentNode.removeChild(placeholder);
      placeholderTimer = 0;
    }, 500);
  }

  /* 加载失败：静默移除全部注入元素，页面退回原生观感；
   * 热重载轮询继续，壁纸指向换到新地址后自动重建 */
  function onDead() {
    unmount();
    dead = true;
  }

  function createVideo() {
    var v = document.createElement("video");
    v.setAttribute("data-zbar-wallpaper", "video");
    v.muted = true;
    v.loop = true;
    v.playsInline = true;
    v.autoplay = true;
    v.style.cssText =
      "position:fixed;top:0;left:0;width:100%;height:100%;" +
      "object-fit:cover;z-index:-2;pointer-events:none;" +
      "opacity:0;transition:opacity .35s ease;";
    v.addEventListener("canplay", onReady);
    v.addEventListener("error", onDead);
    return v;
  }

  function createImage() {
    var i = document.createElement("img");
    i.setAttribute("data-zbar-wallpaper", "image");
    i.alt = "";
    i.style.cssText =
      "position:fixed;top:0;left:0;width:100%;height:100%;" +
      "object-fit:cover;z-index:-2;pointer-events:none;" +
      "opacity:0;transition:opacity .35s ease;";
    i.addEventListener("load", onReady);
    i.addEventListener("error", onDead);
    return i;
  }

  function mounted() {
    return !!(media && media.parentNode);
  }

  /* 挂载三层：媒体层先入，占位层插到媒体层之前（同 z-index:-2 时
   * DOM 靠后者居上，占位层必须先于媒体层插入才会被媒体盖住） */
  function mount() {
    if (!media) return;
    if (!media.parentNode) document.body.appendChild(media);
    if (!placeholder.parentNode) {
      document.body.insertBefore(placeholder, media);
    }
    if (!mask.parentNode) document.body.appendChild(mask);
  }

  function unmount() {
    if (placeholder.parentNode) placeholder.parentNode.removeChild(placeholder);
    if (media && media.parentNode) media.parentNode.removeChild(media);
    if (mask.parentNode) mask.parentNode.removeChild(mask);
  }

  /* ---- 变量应用 ---- */
  function applyFilter() {
    var b = num(cssVar("--zbar-wp-brightness", "1.1"), 1.1);
    var s = num(cssVar("--zbar-wp-saturate", "1.4"), 1.4);
    var l = num(cssVar("--zbar-wp-blur", "0"), 0);
    if (media) {
      media.style.filter =
        "brightness(" + b + ") saturate(" + s + ") blur(" + l + "px)";
    }
  }

  function applyRate() {
    /* 播放速率仅视频有意义，图片直接跳过 */
    if (mediaKind !== "video" || !media) return;
    var r = num(cssVar("--zbar-playback-rate", "1"), 1);
    try {
      media.playbackRate = r;
    } catch (e) {
      /* 个别编码下设置速率可能受限，忽略即可 */
    }
  }

  function applyMask() {
    var m = num(cssVar("--zbar-mask-strength", "0"), 0);
    mask.style.background = "rgba(0,0,0," + m + ")";
  }

  /* ---- 壁纸切换（首建与热重载换源共用） ---- */
  function setWallpaper(url) {
    var kind = kindOf(url);
    currentUrl = url;
    dead = false;
    /* 类型变化（视频↔图片）时移除旧元素重建，避免元素属性串味 */
    if (media && mediaKind !== kind) {
      if (media.parentNode) media.parentNode.removeChild(media);
      media = null;
      mediaKind = "";
    }
    if (!media) {
      mediaKind = kind;
      media = kind === "image" ? createImage() : createVideo();
    }
    mount();
    media.style.opacity = "0"; /* 重置为占位态，就绪后再淡入 */
    media.src = url;
    if (kind === "video") media.load();
    applyFilter();
    applyMask();
  }

  /* ---- 热重载：每 1000ms 强制重读 variables.css 并比对快照 ---- */
  var snapshot = {};

  function snapshotOf() {
    var cs = getComputedStyle(document.documentElement);
    var o = {};
    for (var i = 0; i < VAR_NAMES.length; i++) {
      o[VAR_NAMES[i]] = (cs.getPropertyValue(VAR_NAMES[i]) || "").trim();
    }
    return o;
  }

  function sameSnapshot(a, b) {
    for (var i = 0; i < VAR_NAMES.length; i++) {
      if (a[VAR_NAMES[i]] !== b[VAR_NAMES[i]]) return false;
    }
    return true;
  }

  /* href 追加时间戳强制重读文件；percent-encoded 路径不含裸 "?"，
   * split 取基址安全 */
  function reloadVarsLink() {
    var link = findVarsLink();
    if (!link) return;
    var base = (link.getAttribute("href") || "").split("?")[0];
    link.setAttribute("href", base + "?t=" + Date.now());
  }

  function poll() {
    try {
      /* 壁纸层总开关（V6）：仅显式 "0" 视为关闭；空串/缺失（旧
       * variables.css）按开启处理——升级后默认行为与旧版一致 */
      var wallpaperOn = cssVar(VAR_WALLPAPER_ENABLED, "1") !== "0";

      /* 注入层被页面意外清掉时自愈（加载失败 dead 态与总开关关闭态
       * 除外——off 是用户意图，不得重新挂载）。!wallpaperOff 必须显式
       * 判定，不能只看 wallpaperOn：poll 每秒 reloadVarsLink() 会开
       * "样式表卸载 → 异步解析 → 恢复"窗口，off 稳态下若某轮起点恰在
       * 窗口内，enabled 读到空串回退 "1" 会误判为开启——本分支位于空
       * 值防御之前（防御拦不住），抢先挂载后稳态快照不再变化、卸载分支
       * 不可达，开关已关但壁纸持续显示；同时它也会与下方"由关切开"的
       * 恢复分支重复挂载（off→on 时二次 setWallpaper 重置淡入与占位层）。 */
      if (wallpaperOn && !wallpaperOff && !dead && currentUrl && !mounted()) {
        setWallpaper(currentUrl);
        return;
      }
      /* 先取快照，后重读 variables.css：reloadVarsLink() 改 href 会让
       * 样式表立即进入"卸载失效 → 异步加载解析 → 恢复"窗口。若沿用
       * 旧顺序（先重读后快照），快照正好撞上本轮重载的失效窗口读到
       * 空值；先取快照则读到的恒为上一轮重载稳定后的值（距本轮 href
       * 变更已隔一个轮询周期）。 */
      var now = snapshotOf();
      /* 空值防御：任一变量读到空串，说明 variables.css 仍处于上一轮
       * href 变更的重载窗口（旧样式表已卸载、新规则未解析完），或页面
       * 初载样式表尚未首次解析完成——本轮快照不可信，视为失效窗口直接
       * 返回：不 diff、不应用滤镜/遮罩、不切壁纸，避免把全部变量误判为
       * "被重置为默认值"而清掉用户参数。正常落盘的 variables.css 恒定
       * 渲染全部变量，空串只会出现在失效窗口，判定无歧义。
       * 唯一例外 --zbar-wallpaper-enabled（V6）：旧 variables.css 无此
       * 变量恒为空串，若参与本判定会让旧主题用户升级后每轮都提前返回、
       * 其余参数的热重载一并停摆；该变量缺失本就等价于"开启"（上面已按
       * fallback "1" 读取），无需防御 */
      for (var i = 0; i < VAR_NAMES.length; i++) {
        if (VAR_NAMES[i] === VAR_WALLPAPER_ENABLED) continue;
        if (now[VAR_NAMES[i]] === "") return;
      }
      reloadVarsLink();
      if (sameSnapshot(snapshot, now)) return;
      snapshot = now;
      /* 总开关关闭：移除注入三层并记住 off（unmount 幂等，重复轮次零
       * 成本），背景回到 theme.css 的 base_alpha 纯色氛围底；不置 dead、
       * 不动 currentUrl——重新开启后按原壁纸指向与滤镜参数恢复。本分支
       * 位于快照与 reloadVarsLink 之后：关闭壁纸不影响其余参数的热重载
       * （本脚本每秒 cache-bust 是 variables.css 唯一的强制重读通道，
       * usage.js 的开关/字号读数同样依赖它） */
      if (!wallpaperOn) {
        if (!wallpaperOff) {
          unmount();
          wallpaperOff = true;
        }
        return;
      }
      /* 由关切开：按当前快照的指向恢复挂载——用户显式开启等价于一次显式
       * 重试，即使该源此前加载失败（dead）也照常重建重载（再失败会重新
       * 置 dead，不形成自动循环：自动自愈路径仍受 dead 约束，只有换源才
       * 复位）。取快照里的最新指向，关闭期间换过壁纸不会停留在旧源 */
      if (wallpaperOff) {
        wallpaperOff = false;
        var resumeUrl = urlOf(now["--zbar-wallpaper-url"]) || currentUrl;
        if (resumeUrl) {
          setWallpaper(resumeUrl);
        }
        return;
      }
      var url = urlOf(now["--zbar-wallpaper-url"]);
      if (url && url !== currentUrl) {
        setWallpaper(url); /* 内部已应用滤镜与遮罩 */
      } else {
        applyFilter();
        applyRate();
        applyMask();
      }
    } catch (e) {
      /* 单轮失败静默，下一轮重试 */
    }
  }

  poll(); /* 立即建层，不等首个周期 */
  setInterval(poll, 1000);
})();
"#;

/// 对话页用量统计条模板：在每轮对话（section[data-turn-id]，值实为
/// msg_ 前缀的用户消息 id）同轮 DOM 顺序最后一个单元节点末尾渲染一行
/// 小字统计条（↑ 非缓存输入 ↓ 输出 ⟲ 缓存读 · × 请求数 · 输出速度 ·
/// TTFT 首字延迟，V5 格式）；并在对话输入框上方固定悬浮会话级实时
/// 统计条（Σ 会话累计）。V6 起轮次完成后（turn_usage 落库）显示最终
/// 真实值，生成过程中经 runs 数组（model_usage 已确认的请求、turn_usage
/// 尚未写入的进行中轮聚合，usage_feed 每 2 秒随 turns 一并导出）更新；
/// 不从 DOM 文本推算 Token 或速度。V8 起活动轮判定为 DOM 驱动（会话
/// DOM 最后 umid 节点不在 index 即活动轮），首笔模型请求完成前的启动
/// 窗口也即时显示等待占位。V10 起三态（启动窗口/live/完成）统一为同一
/// 固定结构、字段等宽补位，任何状态只更新数值不改变结构（行格式
/// 与枯萎清理见模板头 V10 变更说明）。V13 起会话条定位为零测量的布局
/// 方案：ensureStyle 给输入区容器 .chat-composer-region 注入 relative +
/// padding-top:26px 顶部留白，会话条（absolute top:4px 居中）由
/// renderSessionBar 幂等挂载进容器、住进留白内，输入框单行/多行切换、
/// 窗口缩放均随文档流自动跟随，零坐标测量、无 resize 监听；region 缺
/// 失时退回旧路径（挂 body + fixed + SESSION_BAR_BOTTOM_PX 96px 兜底）。
/// V12 的动态测量方案（getBoundingClientRect().top → fixed bottom）因
/// 实机会压住输入框内第一行文字废弃（测量目标与可见输入卡片边缘不一
/// 致 + 输入框单行/多行切换的时序窗口）。
/// V20 起数据端为每条已完成子代理轮额外导出"自身视图行"（sess 为子代
/// 理会话 id、带 subagent:1 标记）：子轮 umid 进入完成索引后，已完成子
/// 代理面板轮走完成态显示真实 token 值（此前永无该行，误判活动轮渲染
/// 全 0 占位并枯萎移除）；会话累计按 sess 精确匹配无任何双计路径，渲染
/// 管线零改动。
/// V25 起每轮条与会话条追加"平均 / 快 / 慢"聚合段、会话条下方追加模型
/// 分组速度行（数据端 usage_feed 的 turns[].sa / sess[].sa / 顶层
/// models[]，只用精确生成样本，随 2 秒大文件发布；1 秒速度旁路不变）：
/// 样本 < 2 时隐藏聚合段，模型行按最近值三档变色并带全量口径 title，
/// 模型行显示时动态抬高输入区顶部留白。
/// V26 起速度展示视觉与语言修复（仅渲染端）：语言三信号判定（DOM
/// lang/CJK 文本 → navigator.language → 默认 zh）+ WORDS 词表统一标签
/// （均/快/慢 ↔ avg/fast/slow）；模型区改带底板组件并与会话行同左缘/
/// 宽度锚定，行内弹性布局（截断只发生在模型名，数值永不截断）；留白
/// 按实际字号与底板几何精确计算（修复压输入面板）；tooltip 固定两行
/// 模板（见模板头 V26 变更）。
/// V27 起会话条尾部追加订阅额度段、缓存维度升级——a) 数据端 usage_feed
/// 新增独立额度线程（60s 节拍、首拍立即查询 BigModel 额度接口，HTTP 15s
/// 超时绝不进 2s 导出线程与 1s 速度旁路），成功后原子写出同目录
/// usage-quota.js 小文件（window.__ZBAR_QUOTA__ = {v:1,ts,h5,wk}，剩余 =
/// 100 − 已用百分比；失败静默保留上次文件），本脚本 30s 低频重载，会话
/// 条尾部追加 "5h余22% · 周余7%"（数据缺失/查询未成功时对应段不入行，
/// 无空白分隔符残留；剩余 <30% 黄、<10% 红，其余继承行色）；b) 每轮条
/// 三态 ⟲ 缓存读段后补 ✚ 缓存写段（turns/runs 既有 cw 字段，数据端零
/// 改动）；c) 会话条 ⟲ 段升级为 token + 缓存命中率（cr / (cr + 非缓存
/// 输入)，分母 0 显示 —），新增 ✚ 缓存写段——sess 行新增 cw 附加字段，
/// V21 双口径回退路径 sessionTotals/sessionRunTotals 同步补 cw 累计；
/// Σ = ↑+↓+⟲ 口径不变，usage-data.js 契约 v 保持 2。
/// V28 起额度展示改版（仅渲染端，usage_feed 额度线程与 usage-quota.js
/// 契约 {v:1,ts,h5,wk} 零改动）：额度从会话条尾部移出，改为输入框卡片
/// 底部外侧的独立一行（renderQuotaBar：右对齐小号灰字 "额度 5h 余78% ·
/// 周余93% · 18:18 更新"，尾段消费 quotaData.ts 格式化的本地 HH:mm 更
/// 新时间），absolute 定位不占布局空间（region 缺失回退 fixed 贴底右
/// 侧）；新增独立开关参数 usage_quota_bar（默认 true），经 variables.css
/// 的 --zbar-usage-quota-bar（1/0）热重载生效，变量缺失视为开启；三档
/// 配色、30s 低频重载（隐藏降频 60s）与缺失段隐藏机制原样保留。
/// V29 起注入统计条实机反馈三修复（仅渲染端，usage_feed 与三个数据文
/// 件契约零改动）：a) 额度行样式修正——字号升回 --zbar-usage-font-size
/// 基准（取消 ×0.9 缩小）、整行继承灰白行色（仅百分比数值保留三档变
/// 色）、删除更新时间戳尾段（行内容精简为 "额度 5h 余89% · 周余93%"，
/// bottom 负偏移微调 -16、行高压缩 1.4）；b) 空会话隐藏额度行与模型
/// 行——currentSessionHasData 会话级判定（提炼 renderSessionBar
/// removeBar 同款条件；实机复检后取代初版全局判定 sessionDataEmpty，
/// 后者在多会话保活下任一旧会话有数据即放行、新建任务页仍出现孤立额
/// 度行）+ renderAll 空轮分支双重闸，新建任务页不再出现孤立悬浮的额
/// 度行/模型行；c) 模型速度行从会话条内
/// 拆出为独立容器（data-zbar-usage-models）并新增独立开关
/// usage_model_rows（默认 true，经 --zbar-usage-model-rows 热重载生
/// 效）——会话条开关关闭时模型行照常显示，输入区顶部留白由
/// syncComposerPad 按两个开关独立叠加（全关还原基数 26px）。
/// V30 起额度行 bottom 负偏移再加深至 -20，缓存命中率自会话条 ⟲ 段挪
/// 至底部额度行「缓存 88.7%」段（分母 0 隐藏，数据端零改动）。
/// V31 起底部额度行追加窗口重置倒计时（额度契约 v:1 → v:2）：
/// usage-quota.js 新增 h5rs/wkrs（5h/周窗口下一次重置时刻 ms，数据端取
/// 接口 nextResetTime 直取无换算，窗口缺失 null），渲染端 5h/周余段后追
/// 加括号纯时长（"额度 5h 余89%（2h35m） · 周余93%（5d2h） · 缓存
/// 88.7%"），格式 <1h "35m" / <24h "2h35m" / ≥24h "5d2h"（天/时/分英文
/// 单位中英文通用）；倒计时段以无 class 独立 span 恒继承灰白行色，阈值
/// 变色只属于剩余百分比（<30 黄 / <10 红）不扩展到倒计时；版本闸放行
/// v:1/v:2（升级过渡期残留旧文件无倒计时段自然过渡）；倒计时基于
/// Date.now() 随 renderAll 每拍重算、全串比对零 DOM 机制不变；无新增开
/// 关（随 usage_quota_bar 显隐），V29 删除的时间戳尾段不复活。
/// V32 起底部额度行扁平化去括号（纯渲染端微调，数据契约与 V31 一
/// 致）：zh 词表额度段「5h 余」→「5h 」、周段「周余」→「每周 」
/// （与 en 同款尾随空格），倒计时段去括号直接返回纯时长、由渲染端前
/// 置半角空格紧随百分比，行形 "额度 5h 65% 3h02m · 每周 93% 6d21h
/// · 缓存 88.7%"（en "Quota 5h 65% 3h02m · wk 93% 6d21h · cache
/// 88.7%"）；seg.sub 通道、rs 缺失/过期不加段与阈值变色边界原样保留。
/// 数据源为本目录下 usage-data.js（键名契约见 usage_feed 模块头；每轮
/// 条匹配键为 umid 字段）与 usage-quota.js（V27 额度旁路）。版本化落盘
/// （头部 ZBAR-THEME-V 标记，见 store::ensure_versioned_template）。风格
/// 与 effects.js 同款：自愈、静默失败、空值防御；DOM 选择器集中在头部
/// 常量，便于实机比对调整。
pub const USAGE_JS: &str = r#"// ============================================================
// ZBAR-THEME-V47
// ZBar Agent 对话页用量统计条（由 ZBar 落盘并随版本升级覆盖）
// ============================================================
// V47: request-average markers propagate through per-turn, session and model aggregates.
// V46 变更（底部状态条去中文文字标签，纯渲染端微调、数据契约零改动——
//   各段本就带原生 title 悬浮说明该列口径，段内中文标签（本轮/会话/
//   工具/今日/子代理/缓存）删除省横向空间，title 承担全部口径说明；额
//   度两段（5h/每周）原本无图标、删"额度"前导后无标识，补 battery（剩
//   余电量隐喻）/repeat（周期重置隐喻）行内 SVG 图标；词条删除仅限失去
//   消费方的标签词，title 词条与 5h 前缀词保留）：
// V45 变更（实机反馈：V44 隐藏滚动条失效，横向滚动时顶出默认粗滚动条——
//   ZCode 全局标准 scrollbar-width/scrollbar-color 可继承，Chromium 121+
//   元素上取非 auto 值（含继承）即整体禁用 webkit 伪元素。改为显式细滚
//   动条：qb-line 声明 scrollbar-width:auto;scrollbar-color:auto 重置继
//   承值恢复伪元素生效权，滚动条从隐藏改 4px 细条（thumb 中性半透明灰 +
//   透明 track）；滚轮横滚与左右缘渐隐零改动）：
// V44 变更（底部状态条溢出适配机制退役重做——实机反馈：分段串长随对话
//   天然变化（工具/子代理/缓存段条件性出现、token 数字随会话增长变
//   宽），且右侧计划列表/子代理面板/浏览器等侧面板压缩对话区宽度，
//   V34 优先级整段隐藏导致今日/缓存段频繁被整段隐藏、缓存段在临界宽
//   度下随数据每拍隐藏↔恢复闪烁。改为横向滚动 + 滚轮，隐藏机制整体退
//   役、闪烁随之根治；单行保持不变，不做字号自适应。只动渲染端，数据
//   契约零改动）：
//   a) .zbar-usage-qb-line overflow:hidden → overflow-x:auto
//      （::-webkit-scrollbar 隐藏保持胶囊视觉干净），justify-content:
//      center → flex-start + .zbar-usage-qfull 补 margin:0 auto——
//      flex 容器溢出时居中对齐会让 scrollLeft=0 左端内容不可达（经典
//      冲突），auto margin 溢出时归零、内容锚左可完整滚到两端，不溢出
//      时仍视觉居中；
//   b) 普通滚轮直接横向滚动：qb-line 挂非 passive wheel 监听
//      （deltaX/deltaY 累加进 scrollLeft，触控板 deltaX 天然支持），仅
//      存在横向溢出且未滚到对应边界时 preventDefault，滚到头放行不吞
//      页面纵向滚动；监听随 qb-line 元素创建挂载、随条移除一并销毁
//      （元素即生命周期），不重复挂载；
//   c) 左右缘渐隐提示：syncQuotaFade 按滚动状态在 qb-line 上切
//      qfade-l/qfade-r class，mask 渐隐挂本体（纯绘制层不拦指针、不裁
//      可滚动内容，随胶囊透明度/主题色自动一致）——未滚动右缘提示还
//      有内容、滚动位置>0 加左缘、滚到最右右缘消失，无溢出两侧全无；
//   d) fitQuotaSegments/quotaFitHidden/seg.h/qseg-off/data-prio 全部
//      退役删除，签名（quotaSigOf）保留但不再含隐藏位（内容不变零
//      DOM 语义不变）。
// V43 变更（实机反馈 V42 条仍略高，再收一档；只动渲染端几何）：
//   a) 条高 max(36px, 字号×2.6) → max(32px, 字号×2.3)，圆角 12 → 10；
//   b) 留白带 31 → 27px，输入框再回落 4px；条底仍 −13、顶距 8px、
//      条底离软件底部约 14px 的贴底关系不变。
// V42 变更（实机反馈 V41 条偏高、条底离软件底部约 37px 太空，输入框被
//   留白带顶得偏高；只动渲染端几何）：
//   a) 条高 max(46px, 字号×3.2) → max(36px, 字号×2.6)，圆角 14 → 12；
//   b) 条底 bottom 10 → −13（伸到 region 下缘之外 13px），条顶距输入框
//      卡片下缘 8px；留白带高 = 条高 + 8 − 13 = 31px（14px 字号档），
//      输入框随之回落，条底离软件底部约 14px；
//   c) 字段集、卡片同宽对齐、可见性窗口不变（仍为 V41 的 V36 字段）。
// V41 变更（撤回 V40 注入条的多 Agent 订阅额度与抽屉两态——用户实机反
//   馈：抽屉动效是给 HUD 的，ZCode 注入条仍应保持 V36 的字段；只需要
//   调与对话框的间距、保持输入框宽度、再加一点高度）：
//   a) 底部状态条恢复原字段集：速度 / 本轮 / 会话 / 工具 / 今日 /
//      额度 5h / 每周 / 缓存，不再加载 usage-agents.js，也不再渲染
//      Codex/OpenCode Go/Cursor 等分段；
//   b) 恢复按输入框卡片对齐（applyCardAlign：left=card.offsetLeft、
//      width=card.offsetWidth），条与对话框严格同宽同起点；
//   c) 高度提高到 max(46px, 字号×3.2)，上下留白带 = 实测条高 + 2×10，
//      条与卡片底边/输入区底边各留 10px；
//   d) 留白带仍只在条可见时生效（data-zbar-usage-quota-band），关闭开
//      关/空会话不再给输入区垫底；COMPOSER_PAD_BOTTOM_PX 66 兜底。
// V37 变更（底部额度行降行高 + 与输入框卡片/状态栏双侧恒定间隔 +
//   卡片对齐修复——实机用户反馈：额度行高度偏高、与输入框卡片贴得太
//   近要"间隔一点点"，复检确认要上下双侧都有间隔。仅渲染端改动，数
//   据契约与 V36 完全一致）：
//   a) 废弃负偏移挂卡片下边缘外侧方案（单边加大与卡片间距、下方几乎
//      贴死状态栏，方向不对），改预留留白带——照会话条用 padding-top
//      预留空间的同款模式，ensureStyle 给 .chat-composer-region 追加
//      padding-bottom:var(--zbar-usage-pad-bottom,24px)（新常量
//      COMPOSER_PAD_BOTTOM_PX = 24，仅作 CSS 兜底）；
//   b) 留白带高度改动态实测（V37 初版静态预算实机失衡：假设行高 16px
//      实测约 26px——含胶囊背景条/边框/分段纵向 padding，行顶被顶出
//      24px 带外与输入框卡片贴死重叠、带下却空置，上下间隔 1:15）：
//      行渲染后 syncQuotaBand 实测 bar.offsetHeight，带高 = 行高 +
//      2×QUOTA_BAR_GAP_PX（=5，行 bottom 同值）经 --zbar-usage-pad-
//      bottom 行内变量写入（setRegionPadBottom 照 setRegionPad 惯例：
//      region 迁移还原旧 region、不高于基数回落 CSS 兜底；行隐藏/空
//      会话保留最近一次写入值不新增移除路径；region 缺失 fixed 兜底
//      路径不施加）——任何行高下上下间隔恒定 5px；
//   c) 额度行行高 line-height 1.4 → 1.25（行盒收窄）；
//   d) 胶囊纵向 padding 同步收紧：.zbar-usage-qb-line 1px 2px →
//      0 2px、.zbar-usage-qseg 2px 6px → 1px 5px（字号与配色不动）；
//   e) 卡片对齐修复（实机复检：底部条深灰背景横贯全屏、卡片左缘缩进
//      约 48px 未对齐——V35"region 直接子元素首中即用"在卡片非直接
//      子元素/前面有满宽兄弟元素两类结构下取到满宽容器）：
//      locateComposerCard 增语义锚点路径（region 内 textarea/
//      [contenteditable] 向上取最外层非满宽可见祖先，通用 HTML 语义
//      非 ZCode 类名），V35 直接子元素扫描保留为兜底；applyCardAlign
//      改 getBoundingClientRect 相对换算（锚点可命中深层元素，
//      offsetLeft 不再与 region 内 absolute 同坐标系），卡片水平越界
//      时返回 false 回退现状。
// V36 变更（浅色主题文字可读性——用户实测反馈：ZCode 浅色主题白底
//   下注入统计条文字太淡看不清。仅渲染端样式改动，数据契约与 V35 完
//   全一致；深色主题渲染零变化——新增规则全部带 html:not(.dark) 前缀
//   （:not(.dark) 计入特异性，均高于对应原规则），浅色主题命中覆盖、
//   深色主题不命中）：
//   a) 透明度下限提升：行透明消费的 --zbar-usage-opacity（用户参数，
//      默认 .55）按"深色壁纸背景下弱化小字"设计，白底下 55% 透明灰字
//      对比度最多约 3.6:1（即使基色全黑），数学上必然不可读——只能提
//      高浅色下的实际透明度，加深颜色无效。三处行透明消费点（每轮条
//      容器/会话条行 .zbar-usage-sb-line/底部条行 .zbar-usage-qb-line）
//      以 opacity:max(var(--zbar-usage-opacity,.55),.82) 兜下限（用户
//      调高 >.82 仍取用户值）；标签弱色 dim .66→.82、底部条倒计时弱
//      色 qfaint .72→.85（弱色与主文字仍乘法叠加，层级次序不变）；
//   b) 语义色浅色加深变体：三档速度色（fast #34d399 / mid #fbbf24 /
//      slow #f87171）与额度阈值色（quota-mid #fbbf24 / quota-low
//      #f87171）是深色底专用亮色，白底上（尤其黄 #fbbf24）不可读。
//      为四组容器（模型底板/每轮条/会话条/底部条）tps 三档 + 会话条/
//      底部条 quota 两档追加 html:not(.dark) 前缀的加深版
//      （#059669/#d97706/#dc2626，白底对比度达标；模型底板 V33 起底
//      色消费 --color-card 随主题翻转，浅色下恰为浅底）；深色规则原
//      样保留；
//   c) 明确不动：模型底板容器自供卡底+前景色（V26 起不吃低透明）、
//      qdim（无自身透明，父级底部条行提升已覆盖）、qdot 小蓝点、等
//      待态 0.636 系数（防御性规则，无渲染路径）。
// V35 变更（注入条与输入框卡片左右缘对齐 + 每轮条/会话条缓存命中率回
//   归 + ✚ 缓存写 0 值隐藏——实机用户反馈。全部为渲染端改动，数据契约
//   与 V34 完全一致）：
//   a) 输入框卡片定位 locateComposerCard(region)：遍历 region 直接子元
//      素，跳过带 data-zbar-usage-* 属性（自家注入元素）或绝对定位的
//      元素，取第一个可见且 offsetWidth>100 的元素为"输入框卡片"——
//      零选择器假设（不依赖 ZCode 内部类名，仅按几何特征与自家标记排
//      除），找不到返回 null；
//   b) 底部胶囊条（renderQuotaBar）：卡片命中时 left=card.offsetLeft、
//      width=card.offsetWidth（两缘精确对齐卡片——右对齐自然宽的遗留
//      左缘悬空约 145px 问题），行内 right:auto 让位样式表右对齐；未
//      命中回退现状（右对齐自然宽）。每渲染拍重算、值变才写 style；
//      溢出优先级隐藏照常（fitQuotaSegments 按新宽度自然生效）；
//   c) 会话条（renderSessionBar）：居中（样式表 left:50%+translateX）
//      改为左缘对齐卡片（行内 left + transform:none，自然宽），未命
//      中回退居中；顶部 padding 定位机制（V13 红线）不动；
//   d) 模型行底板（renderModelRows）：左缘/宽度对齐卡片（V26 的"与会
//      话行同左缘"锚定升级为与输入框卡片对齐），未命中回退居中；
//   e) 对齐经 applyCardAlign（值变才写；命中前提 card.offsetParent ===
//      region——V13 注入的 relative 规则生效时 offsetLeft 与 region 内
//      absolute left 同一坐标系）/clearCardAlign（回退清理，fixed 兜
//      底路径亦清残留行内对齐）两个 helper；region 缺失的 fixed 兜底
//      路径原样；窗口缩放不触发渲染拍，由下一渲染拍/15s 兜底拍自愈修
//      正（沿用文件既有哲学，不做 resize 监听）；
//   f) 每轮条/会话条 ⟲ 段后追加弱色缓存命中率段 "(NN%)"（cacheHitText
//      共用：cr ÷ (cr + 非缓存输入)；每轮条非缓存输入 = max(0, in − cr)，
//      in 含缓存读，live 态 runs 同口径；会话条 sess 口径 cr/(cr+up)，
//      与底部条「缓存」段同源同拍；分母 0 整段不入行）——V30 曾把命中
//      率自会话条挪至底部条，本次按实机反馈回归两条，底部条段落保留
//      （三处并存）；
//   g) ✚ 缓存写段 cw 为 0 时整段隐藏（含启动窗口占位态的 "✚ 0"——
//      "缓存读后面的+0 看不懂"），cw>0 显示并带 title（zh"缓存写
//      token"/en"Cache write tokens"）；新段进全串比对串（隐藏段不进
//      ——条件段不 push 即不入比对）；renderRowParts/renderBarLine 补
//      part/seg 级 title 通道（title 不进比对串）；词表补 cwTitle 双
//      语条目。
// V34 变更（底部状态条实机反馈修复——挂具页 vision 审查三 P1 + 两
//   P2，全部为渲染端改动，数据契约与 V33 完全一致）：
//   a) 分段优先级隐藏（P1：条宽不足时右缘 mask 渐隐曾把额度/每周/缓
//      存三段视觉吃掉，相比 V32 纯额度行永远可见是体验倒退）：每段带
//      data-prio——速度/额度两段 prio 0 永不隐藏；溢出时按优先级从低
//      到高整段隐藏（缓存 1 → 今日 2 → 工具 3 → 本轮 4 → 子代理 5 →
//      会话 6），每步复测 scrollWidth>clientWidth，不再溢出即停；全部
//      可见时不做任何隐藏，mask 渐隐仅保留为最终兜底。隐藏经
//      .zbar-usage-qseg-off class（DOM 常驻、恢复零重建），隐藏态计
//      入渲染签名（seg.h）：fitQuotaSegments 每拍在渲染后执行——常规
//      拍（内容/宽度均不变）只做两次布局读取零 DOM 写；修正发生时同
//      步 quotaFitHidden 与签名，下一拍构建态与 DOM 一致、签名命中依
//      旧零 DOM。窗口宽度变化不触发渲染拍，由下一个数据拍/15s 兜底拍
//      自愈修正。同时段内水平 padding 9px → 6px、图标-文字与段内
//      parts 间距统一 3px flex gap，降低触发频率。
//   b) 模型速度行浅色主题对比度修复（P1：浅色卡片上恒浅灰文字几乎
//      不可见）：底板文字改消费会翻转的客户端语义变量——容器
//      var(--color-foreground, #5b6577)、单位/聚合小字
//      var(--color-foreground-subtle, #5b6577)；兜底取中灰（深浅两底
//      均可读），不再用 rgba(241,245,249,*) 深色主题专用浅灰。三档速
//      度色保留硬编码（数据语义色，与主题无关）。
//   c) 会话条速度值三档色（P1：白/灰色与每轮条/底部条不一致）：速度
//      值 span 补 tpsClassOf 三档 class（新增 ATTR_SESSION_BAR scoped
//      规则），无效快照/等待占位不着色；可见文本拼接串不变，全串比
//      对零 DOM 机制保持。
//   d) 额度/缓存段口径说明并入原生 title（P2）：额度 5h/每周/缓存三
//      段各自独立分段 + 发丝分隔（V33 已成立），本次补 title 词表
//      条目（quota5hTitle/quotaWkTitle/cacheTitle），段内无任何 "·"
//      分隔符。
//   e) 段内 parts 间距改 flex gap 承担（P2：数值与 "N req" 次级信息
//      近乎粘连）：.zbar-usage-qseg gap 3px，parts 文本不再携带语义
//      间距空格（标签尾空格删除、底部条数值走 fmtTokens(...).trim()
//      去等宽补位空格——底部条各段独立无跨段对齐需求；词间空格如
//      "额度 5h 65%"、"N req" 内部保留）。每轮条/会话条不受影响。
// V33 变更（底部额度行演进为分段胶囊状态条 + 全组件设计语言统一——
//   对 ByteRax/zcode-token-usage-statusbar 参考样式的自研实现，不搬代
//   码。数据端零改动：usage-data.js 仍为 v:2，V33 起消费其两个既有可
//   选附加字段 sess[].tl（会话树工具调用数，缺失省略键）与顶层 today
//   {tt,rq}（今日全库合计，查询失败整键省略）——缺键分段整段隐藏，
//   与逐段隐藏机制同构）：
//   a) 设计 tokens（全组件统一）：
//      · 字体跟随客户端：font-family 改
//        var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Consolas,
//        monospace)——ZCode 客户端在 :root 暴露 --font-mono，缺失时回
//        退既有等宽栈；
//      · 颜色跟随客户端语义变量 + 兜底：弱文字
//        var(--color-foreground-subtle, rgba(120,130,145,.95))、边框
//        var(--color-border, rgba(127,127,127,.22))、卡片底
//        var(--color-card, rgba(28,30,36,.72))（客户端变量随 .dark 翻
//        转）；速度/额度三档语义色保留硬编码（数据语义与主题无关）：
//        fast #34d399 / mid #fbbf24 / slow #f87171（阈值 70/40 不变）；
//      · 文字三级层级靠亮度+字重不靠字号：主数值 font-weight:600 + 前
//        景色、标签/单位弱色、次要信息更弱（.zbar-usage-dim 相对透明
//        / .zbar-usage-strong 加粗 / 底部条 .zbar-usage-qdim/qfaint 弱
//        色两级）；
//      · 每轮条/会话条/模型行底板全部换用上述 tokens（信息集与机制零
//        改动）。
//   b) 底部状态条（renderQuotaBar 演进为完整状态条，容器即玻璃拟态胶
//      囊：backdrop-filter blur(14px) saturate(1.3) 带前缀、卡片底/边
//      框走客户端变量、圆角 10px、内高光+双层投影、右缘 mask 线性渐隐
//      截断兜底）。分段从左到右，缺数据的分段整段省略（相邻兄弟选择
//      器产发丝分隔线，无空分隔残留）：
//      ① ⚡ 速度：最近生成速度——活动轮 runs 行速度（1s 旁路
//        applySpeedData 每秒覆盖）优先，空闲回退该会话 turns 最新完成
//        轮稳定速度（latestSpeedOf，沿用既有取值链路）；数值三档色 +
//        " t/s" 弱色单位，title 注明口径与三档规则；
//      ② 本轮：↑非缓存输入+↓输出合计（fmtTokens）+ " N req"——当前
//        会话活动轮 runs 行（含并入 sub）优先，无则该会话最新完成
//        turn 行（activeRunOf/latestTurnOf）；title 带输入/输出/缓存
//        读/缓存写明细；
//      ③ 会话 Σ：当前会话 tt（sessionTotals 双口径同款合计，即会话条
//        Σ 口径）+ " N req"；title 注明会话树口径；
//      ④ 🔧 工具：sess[].tl（V33 新消费字段；缺失隐藏本段）；title
//        "会话树全部工具调用"；
//      ⑤ 📅 今日：顶层 today.tt + " N req"（today.rq；整键缺失隐藏本
//        段）；title "今日全部会话合计（本地自然日）"；
//      ⑥ 🌿 子代理：Σ 当前会话 turns[].sub.n；任一 runs 行
//        psess=当前会话时图标旁 2px 呼吸蓝点（opacity 脉动，
//        prefers-reduced-motion 关闭）；无子代理隐藏本段；
//      ⑦ 额度：V32 的 5h/每周两段原样迁入（三档变色 + 倒计时逻辑不
//        动，倒计时由 qfaint 显式回灰——阈变色仍只属于剩余百分比）；
//      ⑧ 缓存：V32 的「缓存 NN%」段迁入（分母 0 不入段），加仪表图
//        标。
//      分段行内 SVG 图标经 createElementNS 构建（stroke=currentColor、
//      stroke-width 1.7、14px 随字号缩放、fill none、round cap/join），
//      随文字颜色不单独上色；分段悬停极轻高亮 + 原生 title 提示（容
//      器 pointer-events:none、分段 auto——只有分段捕获指针，不挡输入
//      区；不引入自绘 tooltip）。开关 usage_quota_bar 语义升级为整条
//      开关（--zbar-usage-quota-bar 控制整个底部状态条显隐）。
//   c) 比对机制分级说明：每轮条/会话条保持「全串比对零 DOM 机制」——
//      标签/单位/主数值拆为逐段 span 后，比对串仍为全部字段文本的拼
//      接串（与渲染后 textContent 逐字一致），V10「任何状态只更新数
//      值不改变结构」原则不变；底部条分段携带行内 SVG（无法进
//      textContent 比对），改用 renderModelRows 同款签名比对
//      （data-zbar-usage-qsig，全部分段数据的拼接签名），无变化零
//      DOM 操作语义不变，倒计时分钟级变化之间仍零开销。
//   d) 定位机制零改动（V12/V13 红线）：bottom 负偏移
//     （QUOTA_BAR_BOTTOM_PX = -20）、region 挂载/缺失 fixed 兜底、
//      currentSessionHasData 空会话闸 + renderAll 空轮分支双闸、30s
//      额度重载节奏全部原样；条宽右对齐自然宽 + max-width 100%。
// V32 变更（底部额度行扁平化去括号，纯渲染端微调——数据契约与 V31
//   完全一致，usage-quota.js v:2 不变）：
//   a) 词表精简：zh 额度段「5h 余」→「5h 」、周段「周余」→「每周 」
//      （与 en 词表同款尾随空格，对齐参考样式的「标签 + 裸数值」扁平
//      风格；en 词表不变），行形 "额度 5h 65% 3h02m · 每周 93% 6d21h
//      · 缓存 88.7%"（en "Quota 5h 65% 3h02m · wk 93% 6d21h · cache
//      88.7%"）；
//   b) 倒计时段去括号：quotaCountdownOf 不再按语言包全角/半角括号，
//      直接返回纯时长裸值紧随百分比；
//   c) 渲染同步：renderQuotaLine 在 sub span 之前插入半角空格 text
//      node，renderQuotaBar 全串比对拼接同步为 t + " " + sub——DOM
//      与比对串逐字一致，零 DOM 机制不变；seg.sub 通道、rs 缺失/过期
//      不加段、倒计时不参与阈值变色等 V31 机制原样保留。
// V31 变更（底部额度行追加窗口重置倒计时 + 额度契约 v:1 → v:2）：
//   a) 数据契约升级：usage-quota.js 由 {v:1,ts,h5,wk} 升至
//      {v:2,ts,h5,wk,h5rs,wkrs}——h5rs/wkrs 为 5h/周窗口的下一次重置
//      时刻（毫秒，与 ts 同单位，数据端取自接口 nextResetTime 直取无
//      换算；窗口缺失为 null）；
//   b) 版本兼容放宽：loadQuota 与 renderQuotaBar 的版本闸同时放行
//      v:1/v:2——升级过渡期本地残留的 v:1 文件照常消费（无 h5rs/wkrs，
//      不出现倒计时段，行为与升级前完全一致），额度线程 60s 内写出
//      v:2 后倒计时自然生效；
//   c) 倒计时渲染：5h/周余段后追加括号包裹的纯时长——"额度 5h 余
//      89%（2h35m） · 周余93%（5d2h） · 缓存 88.7%"。fmtResetCountdown
//      格式化剩余毫秒（<1h → "35m"；<24h → "2h35m"；≥24h → "5d2h"，
//      天/时/分英文单位中英文 locale 通用，不进词表）；括号随行内既有
//      语言判定（zh 全角 / en 半角）。倒计时段进 seg.sub，由
//      renderQuotaLine 以无 class 独立 span 渲染、恒继承灰白行色——阈
//      值变色只属于剩余百分比（<30 黄 / <10 红），绝不扩展到倒计时；
//      rs 缺失/非法/已过期（<= now）该窗口不加括号段；
//   d) 每拍新鲜与零开销：倒计时基于 Date.now() 随 renderAll 主路径每
//      拍重算（额度数据仍 60s 写盘 / 30s 重载、隐藏降频 60s 不变），
//      「全串比对无变化零 DOM 操作」机制原样保留（倒计时分钟级变化之
//      间零开销）；V29 删除的时间戳尾段不复活、无新增开关（倒计时随
//      usage_quota_bar 开关显隐），行保持单行右对齐小字。
// V30 变更（额度行微调 + 缓存命中率挪位，数据端零改动）：
//   a) 额度行 bottom 负偏移 -16 → -20（QUOTA_BAR_BOTTOM_PX），贴卡观
//      感再加深；
//   b) 缓存命中率由会话条 ⟲ 段后缀挪至底部额度行「缓存 88.7%」段——
//      renderQuotaBar 新增 anchor 入参、按 renderSessionBar 同款双口
//      径算当前会话命中率（cacheHitText 复用，分母 0 不入段），会话条
//      ⟲ 段回归纯 token，行形 "额度 5h 余89% · 周余93% · 缓存 88.7%"。
// V29 变更（注入统计条实机反馈三修复：额度行样式、空会话隐藏（实机
//   复检升级为会话级口径）、模型速度行独立开关。数据端零改动——
//   usage-data.js 契约 v:2、usage-speed.js v:1、usage-quota.js 契约
//   {v:1,ts,h5,wk} 不变）：
//   a) 额度行样式修正：字号取消 ×0.9 缩小（此前是全页最小字、灰字对
//      比度也低），与统计元素同级消费 --zbar-usage-font-size 基准；整
//      行继承灰白行色——删除「额度」前缀与时间戳尾段的专用着色（此前
//      偏橙黄，与统计条灰白风格不统一），仅百分比数值保留三档阈值变
//      色（<30 黄 / <10 红，quotaClassOf 保留）；删除更新时间戳尾段
//      （fmtClock、词表 updated、时间戳专用样式一并移除——
//      实机上增加行宽与视觉噪音），行内容精简为 "额度 5h 余89% ·
//      周余93%"；bottom 负偏移 -14 → -16、行高 1.5 → 1.4（贴卡片更
//      自然且不超窗口底边）。
//   b) 空会话隐藏额度行（实机复检后口径从全局级升级为会话级）：初版
//      sessionDataEmpty 只判 turns/runs/sess 全局全空——多会话保活下
//      旧会话容器仍挂载 DOM、任一旧会话有数据即放行，新建任务页仍出
//      现孤立额度行（实机复现），已删除。改为会话级判定
//      currentSessionHasData：提炼 renderSessionBar「无数据即
//      removeBar」的同款条件（当前会话 id 无 sess 全量合计行、无归属
//      完成轮/进行中 run/活动轮，锚点或会话 id 缺失同判无数据），
//      renderAll 主路径求值一次（sessHasData）作为模型行与额度行的渲
//      染闸——多会话保活的新建任务页三元素（会话条/模型行/额度行）一
//      并隐藏，切回有数据会话由既有 MutationObserver/兜底渲染拍自愈
//      恢复；renderAll 空轮分支（整页无轮节点）的三清理保留不动。第
//      一条消息发出、数据就位后恢复显示，开关 usage_quota_bar 逻辑不
//      变。
//   c) 模型速度行独立开关与容器（本次重点）：模型行容器从会话条 bar
//      内拆出为独立元素（ATTR_MODELS = data-zbar-usage-models，独立
//      ensureModelsBox/removeModelsBox 与挂载兜底标记
//      ATTR_MODELS_FIXED），挂进 .chat-composer-region（会话条已挂进
//      region 时跟随同一容器共享留白几何，否则取第一个可见 region；
//      region 缺失回退 body + fixed 贴底，照会话条/额度行既有模式）；
//      渲染从 renderSessionBar 内部移出、renderAll 独立调用——会话条
//      开关关闭时模型行照常显示（用户核心诉求），当前会话无数据时随
//      会话条一并隐藏（b 项 sessHasData 渲染闸）；新增独立开关
//      参数 usage_model_rows（默认 true），经 variables.css 的
//      --zbar-usage-model-rows（1/0）热重载生效（变量缺失视为开启，
//      与 session/turn/quota 开关同惯例）。留白几何拆分：padForModels
//      的「会话条行 + 模型行」合计改为 syncComposerPad 统一计算（每
//      拍渲染尾部求一次值，防两条渲染路径互相覆盖）——会话条行
//      （SESSION_BAR_TOP_PX + 字号×1.5）随会话条挂载态计入，模型底板
//      高（modelsPlateHeight）随模型行开关与数据计入，两者独立叠加，
//      全关还原基数 26px（COMPOSER_PAD_TOP_PX，V13 起既有基础留白，
//      CSS 规则静态生效，语义不变）；模型底板 top 恒定在会话行下方
//      MODELS_MARGIN_TOP_PX 处，会话条开关切换不引起模型行跳位。
// V28 变更（额度展示改版：从会话条尾部移出，改为输入框卡片底部外侧
//   的独立一行。数据端零改动——usage_feed 额度线程与 usage-quota.js
//   契约 {v:1,ts,h5,wk} 不变，usage-data.js 契约 v:2 不变）：
//   a) 独立额度行：新增 renderQuotaBar，把额度渲染为 .chat-composer-
//      region 容器内 absolute 定位、贴输入卡片下边缘外侧（bottom 负偏
//      移 QUOTA_BAR_BOTTOM_PX）右对齐的单行小字——"额度 5h 余78% ·
//      周余93% · 18:18 更新"（en 同构 Quota 5h 78% · wk 93% · 18:18
//      updated）；不占布局空间、pointer-events:none，与输入区顶部留白
//      几何（COMPOSER_PAD_TOP_PX/padForModels）无联动；region 缺失回
//      退挂 body + fixed 贴底右侧（标记属性切换定位，同会话条兜底模
//      式）。
//   b) 内容分段：h5/wk 缺失段照旧隐藏（逐段机制无分隔符残留），查询
//      未成功/两窗口全缺失整行隐藏（不渲染空行）；新增更新时间戳尾
//      段——消费 quotaData.ts（毫秒）格式化本地 HH:mm + 词表「更新」，
//      ts 缺失/无效时尾段不出现；三档配色保留（<30 黄 #fbbf24、<10
//      红 #f87171、充足继承行色灰白）。
//   c) 独立开关：新增参数 usage_quota_bar（默认 true），经 variables.css
//      的 --zbar-usage-quota-bar（1/0）热重载生效，变量缺失视为开启
//      （与 session/turn 开关同惯例）；额度行在 renderAll 每拍渲染——
//      额度数据 onload 与开关切换直接生效，不依赖会话条渲染节奏（会
//      话条关闭/无数据时额度行照常独立渲染；页面隐藏降频 60s 保留）。
// V27 变更（额度段 + 缓存写段 + 缓存命中率。usage-data.js 契约 v:2 键名
//   不变，仅 sess 行附加 cw 字段，旧脚本忽略未知字段平滑兼容）：
//   a) 额度段：数据端 usage_feed 新增独立额度线程（60s 节拍，首拍启动
//      后立即查询）调 BigModel 额度接口，写出同目录 usage-quota.js 小
//      文件（window.__ZBAR_QUOTA__ = {v:1, ts, h5, wk}，h5/wk = 5 小时/
//      周窗口剩余百分比 = 100 − 已用；查询失败静默保留上次文件）。本
//      脚本以独立 30s 低频重载（页面隐藏降频 60s），会话条尾部追加
//      "5h余22% · 周余7%" 两段；数据缺失/查询未成功（无全局对象或
//      h5/wk 为 null）时对应段不入行，无空白分隔符残留；剩余 <30% 黄、
//      <10% 红，其余继承行色灰白（醒目不刺眼）。额度绝不进 2s 大文件
//      与 1s 速度旁路。
//   b) 每轮条（三态统一结构）⟲ 缓存读段后补 ✚ 缓存写段（数据端
//      turns/runs 既有 cw 字段，零数据改动；live 行含并入的 sub.cw）。
//   c) 会话条 ⟲ 段升级为 "⟲ <token>·<命中率>"：命中率 = cr / (cr + 非缓
//      存输入)，保留一位小数，分母为 0 显示 —；新增 ✚ 缓存写段（sess
//      行新增 cw 附加字段；V21 双口径回退路径 sessionTotals/
//      sessionRunTotals 同步补 cw 累计，sess 命中/回退两分支都覆盖）。
//      Σ = ↑+↓+⟲ 口径不变（✚ 段与命中率只是新增展示，不改变 Σ 定义）。
// V26 变更（速度展示视觉与语言修复。用户实测反馈：中文界面全部显示英文
//   标签（navigator.language 在 ZCode 的 WebView 里返回英文，判定不可
//   靠）、模型速度行无底板直接叠在动态壁纸上混成一片、模型区与会话行
//   左右边界不齐、行尾 worst 数值被整段截断、标签词三套混用（avg/fast/
//   min 与 avg/best/worst 与 recent）、tooltip 换行参差、第三行模型行
//   压到输入面板、行距过小。数据端 usage_feed 零改动，v:2 / v:1 契约
//   不变）：
//   a) 语言判定改三信号优先级（任一命中即定，结果缓存一次）：
//      ① documentElement.lang 以 zh 开头，或页面可见文本前 2000 字符
//         含 CJK 字符——直接反映用户所见 ZCode 界面语言；
//      ② navigator.language 以 zh 开头；
//      ③ 默认 zh（本项目用户主体中文，且中文标签更短更整齐）。
//      词表集中到 WORDS 常量对象（中英两套），标签统一 均/快/慢 ↔
//      avg/fast/slow，彻底废弃 best/worst/min/recent 混用；tooltip 的
//      "最近/latest" 与 "笔/requests" 同表。
//   b) 模型速度区改带底板组件：半透明深色底（亮暗 prefers-color-scheme
//      两档）+ 1px 细边框 + 8px 圆角 + backdrop-blur + 内边距，任何壁纸
//      上可读。底板恒为深色，最近值三档变色恒用亮色系（删除原亮暗两套
//      媒体查询——深底上暗色系对比度不足）；底板自供对比度，整条低透
//      明（--zbar-usage-opacity）只保留在会话行上，不再压暗模型区。
//   c) 对齐与弹性：会话条容器 align-items:center → stretch——模型底板
//      与会话行同一左边界、同一宽度锚定（复用会话条定位锚点与宽度，不
//      各自为政）；每行改 flex：模型名 flex:1 1 auto + min-width:0 +
//      ellipsis（截断只发生在模型名），数值区（最近值/单位/跟随小字）
//      flex:none + nowrap 永不截断；删除旧行 70vw/420px 双上限的独立
//      宽度（此前与居中会话行错位、行尾 worst 被裁即源于 mextra
//      flex:0 1 auto 可收缩）。
//   d) 交叠修复：V25 留白增量 = 行数 × 固定 14px，用户调大字号或底板
//      自身增高（内边距/行距/边框）后即压到输入面板。改按与 CSS 同源
//      的几何常量精确计算（实际字号 × 行高系数 + 底板内边距 + 行间距 +
//      边框 + 安全间隙，取 max(26px 基数, 需求值)），字号经
//      --zbar-usage-font-size 实际值读取；行高 1.4× → 1.6×、行间 3px、
//      底板与会话行间 5px，层级与呼吸感同步修正（用户反馈行距过小）。
//   e) tooltip 改固定两行模板："模型 · 最近 71.6 t/s ↵ 均 38.4 · 快
//      71.6 · 慢 12.4 · 5 笔"（en 同构 latest/avg/fast/slow/requests），
//      显式换行消除悬挂分隔符开头与自动换行参差，删除 "颜色按最近值
//      分档" 冗余说明。
// V25 变更（速度聚合 + 模型分组速度：每轮条与会话条追加"平均 / 快 / 慢"
//   聚合段，会话条下方追加模型分组速度行；口径对齐免注入悬浮窗模型速度
//   区，两处共用 token_speed::generation_sample 的行级唯一真源）：
//   a) 数据端 usage_feed 新增三处附加字段（usage-data.js 的 v 保持 2、
//      usage-speed.js 的 v 保持 1，旧脚本忽略未知字段平滑兼容；1 秒速度
//      旁路内容零变化——聚合只在 2 秒大文件拍产出）：
//      turns[].sa 与 sess[].sa = {avg,max,min,n}：avg = Σ输出 ÷ Σ生成毫秒
//      × 1000（加权均速），max/min = 样本池单笔最快/最慢，n = 精确生成
//      样本笔数（只用 first_token_at/completed_at 齐全的精确样本，
//      request_average 近似样本不进池）；无精确样本时省略该键。
//      顶层 models[] = 窗口级按 model_id 分组的 [{model,tps,avg,max,min,n}]
//      （最近使用降序至多 3 组，对齐免注入悬浮窗模型速度区；老库缺
//      model_id 列时整个键省略 → 本脚本不渲染模型行）。
//   b) 每轮条（完成态）：速度位之后追加紧凑聚合段 "· 均38 快71 慢12"
//      （整数，单位沿用该行已有的 t/s 不重复标）；样本 n < 2 时隐藏该段
//      （单笔样本 均=快=慢 无信息量）；进行中轮（live 行）与启动窗口占位
//      不加聚合段（进行中轮数据端不产出 sa）。子代理每轮条走同一
//      renderOne 路径天然获得聚合，无需单独处理。
//   c) 会话条：速度段后追加同构聚合段（同样 n < 2 隐藏）；条下方追加模型
//      行容器（最多 3 行：模型名 truncate + 最近值等宽三档变色（≥70 绿 /
//      ≥40 黄 / <40 红，与免注入悬浮窗同阈值）+ 小号灰 "· 均 x · 快 y ·
//      慢 z"，行 title 给模型全名与全量口径）。无样本或会话条开关关闭时
//      不渲染模型行。
//   d) 会话条 DOM 由单行改为两段式（容器 + 逐段 span 行 + 模型行容器）：
//      逐段 span 渲染结构与无变化零 DOM 操作策略保留（行与模型行各自
//      用自己的内容比对，元素重建时自然重渲）；模型行显示时动态加大输入
//      区容器顶部留白（--zbar-usage-pad-top，基数 26px + 每行 14px），
//      隐藏时移除变量还原 26px；region 缺失的 fixed 兜底路径不施加留白，
//      挂载自愈逻辑不变。
//   e) 语言：本脚本无 i18n 通道，按 navigator.language 判定——zh 开头用
//      "均/快/慢"（与免注入悬浮窗中文一致），其余用 "avg/fast/min"。
// V24 变更（速度快照拆出小文件，速度与用量只消费后端确认数据）：
//   a) 每轮、进行中轮与会话条均显示 model_usage 已确认请求的计数；
//      DOM 文本不再参与 token 或速度计算。
//   b) 速度改为最近一笔完成请求的请求级快照；可信生成速度显示精确值，
//      只有总耗时的请求平均值带 ≈ 前缀，缺失数据显示 –。
//   c) 速度快照从 usage-data.js 拆到同目录 usage-speed.js；历史用量仍
//      每 2 秒刷新，速度小文件独立轮询，数据文件未出现时只显示等待占位。
// V22 变更（删除会话条 CTX 上下文占用段。用户改主意：CTX 百分比展示
//   下线（悬浮窗版已先行删除），注入版一并删除， sess 数组保留
//   model_usage 全量合计口径不变）：
//   a) 会话条删除 "CTX NN%" 段与三档变色 class（黄/红两条样式规则一并
//      移除）；逐段 span 渲染结构保留（V21 引入，未来段级配色零成本
//      复用，当前全部段无配色 class，行为与单串 textContent 等价）。
//   b) 数据端 usage_feed 同步删除 sess 行的 cp/cu/cw 三字段与 CTX 查询
//      （context_window 模块整体移除）；渲染端 cp 消费点删除，旧数据
//      文件残留 cp 字段被忽略（未知字段零影响）。
// V21 变更（数据端新增 sess 会话级统计数组，渲染端会话累计条优先消费
//   新口径 + 追加 CTX 段。数据仍为 v2 附加字段，旧脚本忽略未知字段）：
//   a) 会话累计口径修正：turn_usage 覆盖不全（失败/中断轮不落库），按
//      轮聚合的旧会话累计系统性偏小。数据端新增 sess 数组（每会话一条
//      model_usage 全量合计：tt 总量 / up 非缓存输入 / down 输出 / cr
//      缓存读 / rq 请求数，会话树口径含子代理与失败轮）。渲染端
//      sessionTotals 优先读 sess 行（sessIndex 命中且 tt != null）；
//      数据里无该字段（旧数据文件/查询降级）回退旧 turns 遍历口径——
//      两套口径并存，无缝降级。sess 命中时不再叠加 sessionRunTotals
//      （runs 行即 model_usage 已落库行的聚合，全量合计已含，叠加即
//      双计）；逐轮显示（renderOne 完成态）仍用 turn_usage 不变。
//      （V21 b) 的 CTX 段已被 V22 删除，见上）
// V20 变更（配合数据端 V20 双修复，渲染管线零改动即天然正确）：
//   a) turns 新增子代理"自身视图行"（每条已完成子代理轮额外导出一条
//      行：sess 为子代理会话 id、umid 为子轮自己的用户消息 id、数值与
//      dur/ttft 均为子轮自身口径，带 subagent:1 标记——标记仅为数据
//      侧自描述字段（调试/未来消费预留），前端匹配不读取该键；主轮
//      行仍照常含并入的 sub 数值，两行并存）。子轮 umid 由此进入完成
//      索引 index → findLiveNodes 不再把已完成子代理轮误判为活动轮
//      （此前进入已完成子代理详情面板时，该轮 umid 永不在
//      index/runIndex，renderOne 走启动窗口分支渲染全 0 占位条、
//      90 秒后被枯萎判定移除，面板 token 显示全 0），renderOne 按
//      index 命中走完成态显示真实 token 值，枯萎移除逻辑不再作用于
//      它。会话累计无任何双计路径：sessionTotals 按 t.sess 精确匹配，
//      主会话视图不命中子代理自身行、子代理视图只命中自身行；runs
//      侧该轮行落库后即被后端 done 集合过滤，与自身行不同通道不重叠。
//      数据格式仍为 v2 附加字段（旧脚本忽略未知字段）。
//   b) 数据端父会话保活（usage_feed collect_runs）：主轮派发子代理后
//      自身静默等待不再满 10 分钟被踢出 runs——主轮行、会话累计条与
//      子代理孤儿并入均保持实时值（此前主轮行消失导致每轮条跌回全 0
//      启动窗口占位、Σ 丢失该轮，孤儿数值蒸发）。本文件渲染逻辑不变。
// V19 变更（新增每轮统计条开关参数 usage_turn_bar，默认开启）：renderAll
//   第二遍渲染前统一读 --zbar-usage-turn-bar（variables.css 渲染 1/0，
//   变量缺失视为开启，兼容旧 variables.css），关闭时对全部轮节点
//   removeRow 并跳过 renderOne（removeRow 幂等，已渲染行随关闭清掉、
//   开启后自动恢复）；速度快照与会话条不受影响。
// V18 变更（修复新建任务后（空会话）会话累计条停留在上一个会话数据：
//   renderAll 开头无 [data-turn-id] 节点时直接早退，renderSessionBar
//   永不执行，V17 修复的容器判定本身正确但该分支根本到不了会话条渲
//   染，条停留在上一个会话的累计值永不消失。实机复现：新任务容器
//   data-session-id="draft" 可见且含焦点）：
//   a) renderAll 空轮分支在清理旧状态之后补 removeBar()：会话内无任何
//      轮节点即移除会话条。其余逻辑零改动。
// V17 变更（修复新建任务后会话累计条不消失、数据不重置：多会话保活
//   下旧会话容器在任务切换后仍挂载在 DOM 且通常排在前面，原
//   currentSessionId 用 querySelector 首中即取，读到保活的旧会话 id，
//   条继续渲染旧会话累计，应读到新任务的 draft/无数据并随之消失）：
//   a) 新增 visibleEl：元素当前是否可见（保活面板隐藏时 rect 归零或
//      display:none）。
//   b) currentSessionId 重写为容器遍历两级优先：包含
//      document.activeElement 的容器（用户正在输入的会话）优先，其次
//      第一个可见容器；锚点内优先、无候选再全文档。都拿不到返回空
//      串，renderSessionBar 既有逻辑随之 removeBar。
//   c) composer 挂载点新增 pickComposerRegion：可见且属于当前会话容
//      器（closest 命中会话容器属性值）优先，其次第一个可见的，都无
//      返回 null 走既有 body 兜底分支。渲染与数据管线零改动。
// V16 变更（整体移除每轮统计行（完成态/进行中/启动窗口）的鼠标悬浮
//   title 提示。用户反馈：悬浮信息无必要，且原生 title 提示框样式丑）：
//   a) 删除三态行的 title 赋值，以及仅为其服务的明细构造函数与口径
//      常量（悬浮明细文案随之下线，标识符全文零残留）。行可见内容与
//      数据管线零改动；fmtSeconds 因 lineOf 的 TTFT 字段仍在使用而
//      保留。
// V15 变更（会话累计条显示格式调整。用户反馈：动态段 ⋯ 前缀与流式
//   估算 ↓ ~ 前缀语义不明，且 Σ 段缺少会话总量数字）：
//   a) Σ 段新增会话总 Token（tsum = 输入+输出+缓存读之和，真实数据
//      不含估算），原 Σ ↑ 输入改为独立 ↑ 明细段。
//   b) 速度段去掉 ⋯ 前缀（与每轮条速度段一致）。
//   c) 流式估算段 ↓ ~ 改 ≈ 前缀（生成中未落库的输出估算，不计入
//      累计）。每轮条（renderOne/live 行）格式零改动。
// V14 变更（修复 V13 遗漏 SEL_COMPOSER 定义：renderSessionBar 挂载块
//   每次执行 querySelector(SEL_COMPOSER) 读到未声明变量抛
//   ReferenceError，被挂载 catch 静默吞掉，会话条永远走 body + fixed
//   bottom 兜底、压住输入区内部的引用条/附件行）：
//   a) 常量区补回 SEL_COMPOSER 定义（值为 ".chat-composer-region"）。
//   b) 挂载 catch 增加一次性 console.warn（mountWarned 标志），便于实
//      机 DevTools 排查兜底态；渲染与数据管线零改动。
// V13 变更（会话条 DOM 挂载进输入区容器 + CSS 顶部留白定位，废弃动态
//   测量。实机缺陷：V12 的"测 region.top → fixed bottom 计算"会压住
//   输入框内第一行文字——测量目标（region 容器上沿）与可见输入卡片边
//   缘不一致，且输入框单行/多行切换存在时序窗口，测量值滞后于实际布
//   局；DevTools 调试通道本次不可用，直接改为无测量的布局方案）：
//   a) ensureStyle 注入 .chat-composer-region{position:relative
//      !important;padding-top:26px !important}：输入区容器顶部留白
//      26px，会话条住进留白内。relative 风险说明：region 类名自带
//      z-20（设计上已是定位元素），补 relative 通常无副作用；若
//      ZCode 后续改用其它定位方案，本 !important 规则有覆盖风险，
//      实机回访时留意。
//   b) 会话条样式 fixed/bottom → absolute（top:4px;left:50%;
//      translateX(-50%) 居中）：输入框单行/多行增高、窗口缩放时留白
//      随文档流自动跟随，条恒在容器顶部，零坐标测量、零事件监听。
//   c) 挂载进容器：renderSessionBar 幂等迁移（bar.parentElement !==
//      region 才 appendChild；React 不主动清理非自身创建的子节点，
//      此前消息 section 内手插行长期存活已验证；region 被重建时条
//      断连，ensureBar 的 isConnected 检查自愈重建）。
//   d) region 缺失退回旧路径：挂 body + fixed + bottom:
//      SESSION_BAR_BOTTOM_PX（96 兜底），经 data-zbar-usage-session-
//      fixed 属性选择器切换样式，迁回 region 时移除标记自动还原。
//   e) 删除 getBoundingClientRect 测量、SESSION_BAR_ABOVE_PX 常量、
//      window resize 监听（absolute 随文档流自适应，无需重定位）。
//      渲染与数据管线零改动。
// V12 变更（会话条动态测量定位到输入框上沿之上、还原输入框位置、
//   resize 自适应。用户反馈：V11 的"输入框上移 22px + 会话条贴窗底"
//   观感差——统计条贴死窗口边缘、与 ZCode 留白风格不协调）：
//   改回 CLI 惯例——输入框还原原位（删除 V11 的
//   .chat-composer-region padding-bottom 规则与 COMPOSER_GAP_PX 常量），
//   会话条每次渲染动态测量输入区容器（SEL_COMPOSER =
//   .chat-composer-region，常量保留改为定位测量用）
//   getBoundingClientRect().top，style.bottom =
//   window.innerHeight - top + SESSION_BAR_ABOVE_PX（条底距输入区上沿
//   6px），彻底解决 V5 写死 bottom:96 落入输入框内部的问题（输入框
//   实际顶部距底约 180px，写死值不可靠）；region 不存在 / 测量异常时
//   退回固定 SESSION_BAR_BOTTOM_PX = 96（兜底值）。初始化追加
//   window resize 监听（scheduleRender），窗口缩放后下帧自动重定位。
//   渲染与数据管线零改动。
// V10 变更（统计条显示稳定性：结构整体固定只更新数值）：
//   - 每轮条三态（启动窗口/live/完成）统一为同一固定结构，任何状态都
//     渲染全部字段位、只更新数值。原三态各一种结构（启动窗口"逐段省
//     略"极简行 / live 数字段 + 行尾 … 标记 / 完成完整格式）导致段数
//     与左右宽度随状态持续变化，观感差。统一格式（barLineOf）：
//     "↑ <in> ↓ <out> ⟲ <cr> · × <req> · <speed> t/s · TTFT <ttft>"。
//     启动窗口态与未确认速度统一显示等待占位；live/完成态只显示后端
//     确认的请求聚合与请求级速度。TTFT 缺失时显示 "–"。
//   - 数字等宽补位（等宽字体 + tabular-nums 下各字段占位恒定，整行宽
//     度恒定）：token 值经 fmtTokens 恒定 5 字符（"  998"/" 1.2k"/
//     "10.5M"，超 999.9M 自然溢出）；req padStart(3)；速度 toFixed(1)
//     后 padStart(4)，dur 缺失（老库）显示 4 字符占位 "  - "；TTFT
//     padStart(4)（"x.xs" 恒 4 字符），进行中/缺失显示 "–"。title
//     hover 明细不受等宽约束（补位结果在 title 消费端 trim）。
//   - 会话条速度段固定显示，缺少确认速度时显示等待占位；Σ 数字段同步
//     补位（req padStart(3)，token 经 fmtTokens）。
//   - 顺带修复已完成子代理面板的残留占位：已完成的子代理轮（值已并入
//     主轮 sub，turns/runs 永无该 umid 行）此前在子代理详情面板永久
//     显示启动占位行。现在仅由数据索引与请求确认状态决定行内容，不读取
//     页面文本，也不维护基于文本增长的临时速度状态。
// V9 变更（子代理消耗实时化：主轮条/会话条随子代理消耗动态更新 + 子代
//   理详情面板自身统计）。实机核实：子代理详情面板与主对话同 document
//   （无 iframe），面板有自己的 [data-session-id] 容器（值为子代理会
//   话 id sess_subagent_*）与 [data-turn-id] 节点（值为子代理会话自己
//   的 umid，与数据 runs 子代理行完全一致），面板默认关闭、点开才挂
//   载；数据侧 runs 新增主会话行 sub 聚合（并入的子代理实时消耗，含
//   游离子代理完成轮）与子代理行 m:1 防双计标记（usage-data.js 仍为
//   v2 附加字段，向后兼容）。V8 的三个缺陷与修复：
//   - 扫描范围 document 级：V8 的 renderAll 与活动轮判定限定在
//     workspace-main 锚点内，扫不到锚点外的子代理面板。V9 改
//     document.querySelectorAll（35 节点量级，性能无虞），锚点仅保留
//     给会话条定位。子代理面板内的每轮条随 document 扫描按子代理行
//     umid 命中渲染 live 条（子轮完成后 runs 行退出、turns 无子代理
//     行，条自然消失——消耗并入主轮完成值，面板条不残留）。
//   - 多容器活动轮（findLiveNodes 取代 findLiveNode）：遍历所有
//     [data-session-id] 容器，每容器取 DOM 顺序最后一个 data-turn-id
//     节点，umid 不在 index（完成轮）即为该会话的活动轮——启动窗口
//     与 runs 阶段同一节点（V8 口径不变，判定仍不依赖 runs 数据）。
//     返回 Map：会话 id → 活动轮节点；主对话与并行多个子代理面板各
//     有各的活动轮，互不影响。
//   - 主轮条 live 态显示 sub：runIndex 命中的行若带 sub（数据侧并入
//     的子代理实时聚合），数字段按合计显示（↑↓⟲× 加子代理部分），
//     速度仍只消费请求级字段——子代理消耗实时反映在主对话每轮条上。
//   - 会话条 Σ 口径修正：runs 行合计跳过 m:1 的子代理行（其值已并入
//     主轮行 sub，随主轮行一并计入——原 V8 按 psess 裸命中，子代理行
//     与主轮行并存时双计、主轮行缺失时又丢游离完成轮）；父会话暂无
//     主轮行的子代理行不带 m，仍按 psess 并入，无缝衔接。会话条仍只
//     渲染主窗口（锚点定位 + 优先取锚点内会话容器 id，子代理面板无
//     输入区不渲染会话条）。
//   - 各会话活动轮独立判定，renderAll 只在数据变化时刷新确认值；不创建
//     基于 DOM 文本差分的采样目标或定时器。
// V8 变更（启动窗口实时渲染：活动轮判定从数据驱动改为 DOM 驱动）：
//   实机缺陷：发消息后 agent 已开始思考输出，但每轮统计条不显示、会话
//   累计条一动不动，直到第一笔模型请求完成后才有内容。根因：V6/V7 的
//   实时渲染以数据驱动——live 每轮条与估算目标（findLiveNode）都依赖
//   runIndex（runs = model_usage 已完成请求的聚合），而轮次开始到第一
//   笔请求完成之间（思考阶段可达几十秒）model_usage 无行 → runs 无该
//   轮 → live 条不渲染、估算无目标、会话条 sessionRunTotals 为空——
//   启动窗口全空白；data-running 兜底实机不可靠（该属性取值存疑）。
//   V8 活动轮判定改为 DOM 驱动：会话内 DOM 顺序最后一个 data-turn-id
//   （umid）节点，其 umid 不在 index（完成轮）即为活动轮。启动窗口只
//   显示确认计数的等待占位；runIndex 命中后显示已完成请求的实时聚合。
//   data-running 不再是渲染必要条件，仅保留在 MutationObserver 的刷新
//   信号中。每轮完成后 turn_usage 到达即切换最终真实值，多会话各自独立。
// V7 变更：请求次数图标 ⟳ → ×（原 ⟳ 与缓存读 ⟲ 仅箭头方向之差，过于
//   相似易混淆；× 读作"共 N 次"，缓存读 ⟲ 保持不变）
// V6 变更（生成过程实时跳动）：
//   - 数据源 usage-data.js 追加 runs 数组（v2 契约不变，runs 为附加
//     字段，旧脚本忽略未知字段平滑兼容）：model_usage 已落库、
//     turn_usage 尚未写入的进行中轮实时聚合——工具循环每完成一步模型
//     请求即落一行 model_usage（2 秒轮询内可见），无需等整轮结束。行
//     形如 { umid, sess, psess, in/out/cr/cw/rt, req, start }（psess
//     仅子代理会话有值，指回父会话）。
//   - 每轮条渲染优先级：完成数据（index 命中，最终真实值，V2 起逻辑
//     不变）> 进行中 run（runIndex 命中）> 活动轮等待态 > 不渲染。
//     进行中轮行只显示 run 中已完成请求的确认聚合与请求级速度；轮完成
//     后 turn_usage 到达即切最终真实值。
//   - 会话条 Σ = 完成轮合计 + runs 中 sess 或 psess 命中当前会话的行
//     合计；生成期间不把 DOM 文本或未确认输出加入累计，新会话首轮
//     （无任何完成轮）也可显示等待态。修复 V5 动态段不生效的两个根因：
//     a) renderSessionBar 在 sessionTotals 为 null（会话尚无完成轮，
//        新会话首轮必现）时提前返回，动态定时器从未启动——生成期间
//        会话条完全静止；V6 改为完成合计与 run 合计均为空才放弃。
//     b) 动态目标节点检测依赖 data-running="true"，实机不可靠（等待
//        态同样受影响，见用户反馈"完成后才显示数字"）。V6 目标节点改
//        由 runIndex 数据驱动（run 命中的 DOM 节点，2 秒内必达），
//        data-running 仅作数据未达头 2 秒的兜底。
//   - 更新节奏：确认数据随数据轮询（2 秒）刷新；活动轮只在收到新数据时
//     更新等待/实时行，不维护文本差分或独立采样定时器。
// V5 变更（行格式图标化 + 会话级实时统计条）：
//   - 每轮行格式调整（数据与渲染管线零改动，仅显示层）："N req" →
//     "× N"（× = 模型请求次数）、"tok/s" → "t/s"、"首字 X.Xs" →
//     "TTFT X.Xs"（time to first token 公认缩写）。新格式示例：
//     ↑ 1.2k ↓ 3.4k ⟲ 89k · × 25 · 90.0 t/s · TTFT 5.8s
//   - 新增会话级实时统计条（renderSessionBar，独立管线）：fixed 悬浮于
//     对话输入框上方（锚点 workspace-main，bottom 由 SESSION_BAR_BOTTOM_PX
//     常量控制便于实机调参），显示当前会话全部轮次的真实累计
//     （Σ ↑非缓存输入 ↓输出 ⟲缓存读 ×请求数，按 sess 过滤 turns 聚合，
//     已含并入的子代理部分），随 2 秒数据轮询刷新。找不到锚点/会话 id/
//     会话无任何轮次（draft 空会话）时不渲染，静默降级。
//   - 流式生成期间只显示已完成请求的确认聚合；没有请求级速度时显示
//     等待占位。轮完成后数据轮询切回数据库最终值，真实优先。
//   - 会话条开关：ThemeParams.usage_session_bar 经 variables.css 的
//     --zbar-usage-session-bar（1/0）透出（effects.js 每秒热重载，改
//     开关约 1 秒生效）；变量缺失（旧 variables.css）时视为开启（默认
//     true）。字号/不透明度复用 --zbar-usage-font-size/opacity。
// V4 变更（样式参数化）：统计条字号与不透明度改为消费 variables.css 的
//   --zbar-usage-font-size / --zbar-usage-opacity（皮肤页新增"用量统计条"
//   滑块可调，variables.css 由 effects.js 每秒热重载，拖动约 1 秒生效，
//   无需重启）；模板内仅保留 V3 写死值（10px / .55）兜底。原 font 简写
//   `400 10px/1.5` 拆为 font-family/size/weight/line-height 独立声明以
//   参数化字号；等待态低透明由正常态乘 0.636 系数得出（默认
//   0.55×0.636≈0.35，与 V3 写死 .35 观感一致）。渲染/匹配/死锁防护
//   逻辑零改动。
// V3 变更（实机缺陷修复）：scheduleRender 的 rAF 回调改为 try/finally
//   结构——scheduled 复位移入 finally，renderAll 抛异常不再导致 scheduled
//   永久为 true 卡死渲染管线；另加 15 秒低频兜底渲染（不经过 scheduled
//   检查直接调用 renderAll），未来出现新的意外状态也能在一个周期内自愈。
// V2 变更（实机缺陷修复）：实测 DOM data-turn-id 的值是 msg_ 前缀的
//   用户消息 id（等于主库 turn_usage.user_message_id）而非 turn id，
//   匹配键从 turn 改为数据 v2 新增的 umid 字段；同一轮被虚拟列表拆成
//   多个单元节点时，统计条只渲染在 DOM 顺序最后一个节点（回复结束处），
//   其余节点旧行移除，"最后一个"随节点集合变化正确迁移；数据 ts 与
//   上次相同时跳过索引重建与重渲染（常态每 2 秒重载零成本）。
// 数据源：ZBar 后台任务从 ZCode 主库 turn_usage（官方每轮聚合表，轮次
// 完成时才落库）+ model_usage（每步模型请求完成即落一行，V6 起取其
// 进行中轮）导出的本目录 usage-data.js，内容形如：
//   window.__ZBAR_USAGE__ = { v:2, ts:<最后数据变化时刻ms>, turns:[{
//     umid: "msg_xxx"   用户消息 id（turn_usage.user_message_id，与 DOM
//                       data-turn-id 同值，即本脚本匹配键；可 null，
//                       null 轮无法与 DOM 匹配，仅透出）
//     turn: "turn_xxx"  轮 id（保留透出，不用于 DOM 匹配）
//     sess / status / start / end   会话、状态、起止毫秒
//     in / out / cr / cw / rt       输入(含缓存读)/输出/缓存读/缓存写/推理
//                                   ——均已并入该轮覆盖的子代理聚合
//     req / retry / tool            模型请求数 / 重试数 / 工具调用数（同上）
//     dur / ttft                    主轮自身总耗时 / 首字延迟毫秒（可 null）
//     sub: { n, req, in, out, cr, cw, rt }  并入的子代理聚合（可 null）
//     models: "GLM-5.3,..."         该轮模型（去重逗号拼接，含子代理）
//     sa: { avg, max, min, n }      V25 该轮精确生成样本聚合（无样本则
//                                   省略）：加权均速 / 单笔最快 / 单笔最慢
//                                   （t/s）+ 样本笔数；n < 2 时本脚本隐藏
//                                   聚合段（数据端照常导出原值）
//   }],
//     runs: [{           进行中轮实时聚合（V6 附加字段；空数组也输出）
//     umid / sess        匹配键与会话（同上；子代理轮 umid 指向子代理
//                        会话自己的消息，V9 起子代理详情面板按它匹配）
//     psess              父会话 id（仅子代理会话有值，主会话 null）
//     m                  V9：子代理行且父会话存在进行中主轮行时为 1
//                        ——数值已并入主轮行 sub，会话累计跳过防双计
//     sub                V9：主会话行并入的子代理实时聚合（并行子代理
//                        runs + 游离子代理完成轮），每轮条与会话条按
//                        合计口径显示，title 分解明细
//     in / out / cr / cw / rt / req  该轮已完成请求的聚合
//     start              首个请求开始毫秒
//   }],
//     sess: [{ s, tt, up, down, cr, cw, rq, tl, speed, speedState, sa }] 会话级
//     统计（V21 附加数组，model_usage 全量合计；V25 起带会话树口径的
//     sa 聚合；V27 起带 ✚ 缓存写 cw——不参与 tt 口径；V33 起带 tl 会话
//     树工具调用数——列/表缺失或查询失败省略本键，本脚本缺字段隐藏
//     「工具」分段）
//     models: [{ model, tps, avg, max, min, n }]  V25 窗口级模型分组速度
//     （按最近使用降序至多 3 组；model_id 列缺失的老版本库整个键省略 →
//     本脚本不渲染模型行）
//     today: { tt, rq }  V33 顶层今日合计（本地自然日 0 点起全库
//     model_usage 全量：tt = Σ(up+down+cr) 与 sess 行 tt 同口径、
//     rq = model_usage 行数；查询失败/表缺失整个键省略 → 「今日」分段
//     隐藏；只随 2 秒大文件发布）
//   } }
// 额度源（V27）：同目录 usage-quota.js——数据端独立额度线程 60s 查询
//   写盘（window.__ZBAR_QUOTA__ = { v:1, ts, h5, wk }，h5/wk = 5 小时/周
//   窗口剩余百分比；查询失败保留上次文件），本脚本 30s 低频重载，V28
//   起渲染为输入框底部外侧的独立额度行（"额度 5h 余78% · 周余93%"，
//   见 renderQuotaBar；V29 删除时间戳尾段，且空会话——turns/runs/sess
//   全空或页面无轮节点——整行隐藏），数据缺失/查询未成功时隐藏对应段；
//   V33 起该行升级为分段胶囊状态条，额度/缓存为其中尾部两段（见 V33
//   变更 b，开关 usage_quota_bar 语义随之升级为整条开关）。
// 导出窗口：turns 最近 7 天、至多 3000 轮；runs 近 10 分钟内有请求的
//   进行中轮（turn_usage 已有行的完成轮不进 runs）。
// 展示口径：↑ = in − cr（非缓存输入），↓ = out，⟲ = cr，✚ = cw（V27
//   起新增；V35 起 cw 为 0 整段隐藏——启动窗口占位不再出现 "✚ 0"），
//   × = req；V35 起每轮条/会话条 ⟲ 段后补弱色缓存命中率段 (NN%)
//   （cr/(cr+非缓存输入)，cacheHitText 共用，分母 0 不入段；V27–V29 曾
//   挂会话条 ⟲ 后缀、V30 曾移至底部条「缓存」段，V35 起三处并存——
//   每轮条/会话条为 (NN%) 后缀、底部条为「缓存 NN%」段）；
//   速度来自最近一笔完成 model_usage 请求的 speed 快照。generation 为
//   首字到完成的精确速度，request_average 为仅有总耗时的近似速度并带 ≈；
//   缺失/无效快照显示 "–"，TTFT 缺失同样显示 "–"。
// V25 聚合口径：sa 与 models 的 avg/max/min 只取精确生成样本（见文件头
//   V25 变更 a），与上方的"最近速度"是两条独立通道（后者可能是近似
//   样本）；聚合段在样本 n < 2 时隐藏，模型行则始终显示"最近值 + 聚合"
//   （对齐免注入悬浮窗模型速度区）。
// 行格式（V10 起三态统一固定结构；V27 ⟲ 后补 ✚ 缓存写段；V35 起 ⟲ 后
//   补条件命中率段 (NN%)、✚ 段改条件段——分母 0 / cw 0 整段不入行）：
//   每轮条任何状态都渲染
//   "↑ <in> ↓ <out> ⟲ <cr> [(NN%)] [✚ <cw>] · × <req> · <speed> t/s ·
//   TTFT <ttft>"（方括号段按数据条件出现），
//   各字段等宽补位（token 5 字符 / req 3 字符 / 速度与 TTFT 各 4 字
//   符），等宽字体 + tabular-nums 下整行宽度恒定，只更新字段数值不改变
//   结构；request_average 的 ≈ 前缀由速度字段自身表达近似质量。
//   V33 起行内标签/单位拆为弱色 span、速度数值带三档色 class——可见
//   文本拼接串与字段补位规则不变（全串比对串逐字一致）。
// V25 起完成态在速度位后追加 "· 均38 快71 慢12"（整数，不重复标单位；
//   样本 n < 2 整体不出现），live 行与启动窗口占位不追加（见 V25 变更 b）。
// 行为：usage-data.js 每 2 秒以 script 标签重载（只承载历史计数，先删旧
//   再插新，加载失败保留上次数据；页面隐藏降频 10 秒）；速度则以独立的
//   usage-speed.js 小文件每 1 秒重载，速度变更不会触发大文件重写或历史
//   索引重建。V27 起额度以独立的 usage-quota.js 小文件每 30 秒低频重载
//   （页面隐藏降频 60 秒），只承载剩余百分比，不触发历史索引重建。
//   v !== 2 / speed v !== 1 / quota v !== 1 视为无效并保留上次数据；各自
//   ts 与上次相同则跳过对应的索引/渲染。
//   MutationObserver + 首次全量扫描定位 [data-turn-id]（V9 起扫描
//   document 级，覆盖主对话与子代理详情面板）；每轮条按优先级渲染：
//   完成数据按 umid 命中（同 umid 多节点只在 DOM 顺序最后一个节点出）
//   > 进行中 runIndex 命中（确认聚合）> 启动窗口活动轮（DOM 驱动判定：
//   本会话容器 DOM 最后 umid 节点且数据未达，统一固定结构渲染确认等待
//   占位）> 不渲染；虚拟列表回收重挂按 umid 幂等重渲，节点消失不残留
//   状态。renderAll 尾部同步驱动会话级统计条，15 秒兜底渲染覆盖自愈。
// ============================================================
(function () {
  "use strict";

  if (!document.body) return;

  /* ---- 选择器与常量（集中此处，便于实机比对调整） ---- */
  /* 对话区锚点：V9 起仅用于会话条定位与会话 id 收敛（每轮条/活动轮
   * 判定改 document 级扫描，覆盖锚点外的子代理详情面板） */
  var SEL_PANE_ANCHOR = '[data-pane-id="workspace-main"]';
  var SEL_TURN = "[data-turn-id]"; /* 每轮对话节点 */
  var ATTR_TURN_ID = "data-turn-id";
  var ATTR_RUNNING = "data-running"; /* 轮进行中属性（实机取值存疑）：
    V8 起不参与任何活动轮/渲染判定，仅保留在 MutationObserver
    attributeFilter 作为属性变化的额外刷新信号 */
  var ATTR_ROW = "data-zbar-usage-row"; /* 统计条标记（防重复） */
  var STYLE_ID = "zbar-usage-style";
  var LOADER_ID = "zbar-usage-data-loader";
  var SPEED_LOADER_ID = "zbar-usage-speed-loader";
  var QUOTA_LOADER_ID = "zbar-usage-quota-loader";
  var POLL_MS = 2000; /* 历史用量大文件重载周期 */
  var SPEED_POLL_MS = 1000; /* 速度小文件重载周期；不重载大文件 */
  var POLL_HIDDEN_MS = 10000; /* 页面隐藏时降频 */
  var QUOTA_POLL_MS = 30000; /* V27 额度小文件重载周期（数据端 60s 节拍
    写盘；低频——额度是慢变量，重载只解析几十字节） */
  var QUOTA_POLL_HIDDEN_MS = 60000; /* 页面隐藏时额度重载降频 */
  var FALLBACK_RENDER_MS = 15000; /* 低频兜底渲染周期（死锁/漏渲染自愈） */
  /* ---- 会话级实时统计条常量（V5，实机调参集中在此处） ---- */
  var SEL_SESSION_ID = "[data-session-id]"; /* 当前会话锚点（属性值为会话 id） */
  var ATTR_SESSION_ID = "data-session-id";
  var ATTR_SESSION_BAR = "data-zbar-usage-session"; /* 会话条标记（防重复挂载） */
  var ATTR_SESSION_BAR_FIXED = "data-zbar-usage-session-fixed"; /* 会话条兜底
    定位标记（V13）：region 缺失退回 fixed 时打上，样式表据此把 absolute
    切换为 fixed + bottom 兜底；迁回 region 时移除自动还原 */
  var SESSION_BAR_BOTTOM_PX = 96; /* 会话条 bottom（px）：仅 region 缺失的
    兜底路径使用（V13 正常定位为挂进容器的 absolute，见 renderSessionBar），
    实机可调 */
  /* V13 选择器记录（集中在常量区便于实机比对）：SEL_COMPOSER =
   * ".chat-composer-region" —— ZCode 输入区容器稳定语义类名（DevTools
   * 实测确认），renderSessionBar 把会话条幂等挂载进该容器（absolute
   * top:4px 住进 CSS 注入的顶部留白），不再做任何坐标测量 */
  var SEL_COMPOSER = ".chat-composer-region"; /* 输入区容器选择器，会话条
    挂载目标（V14 补回 V13 遗漏的定义，此前挂载块读未声明变量抛
    ReferenceError 被静默吞掉） */
  var COMPOSER_PAD_TOP_PX = 26; /* 输入区容器顶部留白基数（px）：ensureStyle
    的 padding-top 为 var(--zbar-usage-pad-top,26px) !important（CSS 内无法
    引用 JS 变量，故基数写死在兜底值里），调整须同步本常量与 CSS 兜底 */
  var COMPOSER_PAD_BOTTOM_PX = 27; /* 输入区容器底部留白带兜底（px，V43）：
    仅作 CSS 静态兜底（页面首帧/行内变量写入失败时的落点，约等于 14px 字
    号档条高 32 + 顶部 8 − 条底外扩 13）；运行期由 setRegionPadBottom 按
    「实测条高 + QUOTA_BAR_GAP_PX + QUOTA_BAR_BOTTOM_PX」经
    --zbar-usage-pad-bottom 行内变量覆盖（照 --zbar-usage-pad-top 同款
    模式），调整须同步本常量与 CSS 兜底 */
  var VAR_PAD_BOTTOM = "--zbar-usage-pad-bottom"; /* 输入区容器底部留白带
    变量（V37）：由 setRegionPadBottom 按实测条高动态写入；V40 起只在条
    可见时随 ATTR_QUOTA_BAND 属性生效，条移除时连同属性一起清除（不再像
    V37 那样常驻 24px 兜底、条关闭仍给输入区垫底）；syncComposerPad 只维
    护 --zbar-usage-pad-top，不触碰本变量 */
  var SESSION_BAR_Z = 30; /* 会话条 z-index：适度抬高，不遮挡弹层 */
  var VAR_SESSION_BAR = "--zbar-usage-session-bar"; /* 开关变量（variables.css 渲染 1/0） */
  var VAR_TURN_BAR = "--zbar-usage-turn-bar"; /* 每轮统计条开关变量（variables.css 渲染 1/0） */
  /* ---- V28：独立额度行常量（V33 起演进为分段胶囊状态条：开关语义
   *      升级为整条开关，定位/兜底机制不变）---- */
  var VAR_QUOTA_BAR = "--zbar-usage-quota-bar"; /* 底部状态条开关变量
    （variables.css 渲染 1/0；缺失视为开启，与 session/turn 开关同惯
    例）；V28 为额度行专用开关，V33 起控制整条分段状态条显隐 */
  var ATTR_QUOTA_BAR = "data-zbar-usage-quota"; /* 额度行标记（防重复挂载） */
  var ATTR_QUOTA_BAR_FIXED = "data-zbar-usage-quota-fixed"; /* 额度行兜底
    定位标记：region 缺失退回 fixed 时打上，样式表据此把 absolute 切换
    为 fixed 贴底居中；迁回 region 时移除自动还原（同会话条
    ATTR_SESSION_BAR_FIXED 模式） */
  var ATTR_QUOTA_BAND = "data-zbar-usage-quota-band"; /* 输入区底部留白带
    生效标记（V40）：打在 .chat-composer-region 上时 CSS 才注入
    padding-bottom（值读 VAR_PAD_BOTTOM），条不可见即移除——避免关闭
    统计条/空会话时输入区永久多出 27px 底垫 */
  var QUOTA_BAR_GAP_PX = 8; /* 底部状态条与输入框卡片下缘的间隔（px，
    V42）：留白带高 = 条高 + 本值 + QUOTA_BAR_BOTTOM_PX（后者为负 =
    条底伸到 region 下缘之外，吃掉状态栏上方的空白）——条顶距卡片恒
    定本值；字号（9~24px）/条高变化时带高随之实测调整 */
  var QUOTA_BAR_BOTTOM_PX = -13; /* 额度行 bottom 偏移（px，V43）：负值
    让条底伸到 region 下缘之外 13px，贴近软件底部；V41 为 10（完全
    在带内，条底离软件底部约 37px，实机反馈太空）。V37 初版为 4px 的静态预算方案——带
    固定 24px + 假设行高 16px，实机行高约 26px（含背景条与 padding）远
    超假设，行顶被顶出带外与输入框卡片贴死/重叠、带下却空置，上下间隔
    1:15 失衡，改 syncQuotaBand 动态实测 + 恒定间隔）；V29~V36 曾为负偏
    移方案——V29 由 -14 微调并把行高压缩到 1.4；V30 再由 -16 加深至 -20
    （行尾新增「缓存」段后行内容更宽，保持贴卡观感且不超窗口底边）；V37
    初版加深至 -24 应"间隔一点点"反馈，实机复检方向不对（只单边加大与
    卡片的间距、下方几乎贴死状态栏）；实机可调 */
  var QUOTA_BAR_FALLBACK_RIGHT_PX = 18; /* region 缺失兜底 fixed 的右边距
    （与宠物默认位置边距同款） */
  var QUOTA_BAR_FALLBACK_BOTTOM_PX = 10; /* region 缺失兜底 fixed 的下边距 */
  /* ---- V29：独立模型行容器常量（模型分组速度行自会话条拆出，独立
   * 开关控制；挂载/兜底照会话条与额度行既有模式）---- */
  var VAR_MODEL_ROWS = "--zbar-usage-model-rows"; /* 模型速度行开关变量
    （variables.css 渲染 1/0；缺失视为开启，与 session/turn/quota 开关
    同惯例） */
  var ATTR_MODELS = "data-zbar-usage-models"; /* 模型行容器标记（防重复
    挂载；容器同时保留 .zbar-usage-models 类名供样式定位） */
  var ATTR_MODELS_FIXED = "data-zbar-usage-models-fixed"; /* 模型行容器
    兜底定位标记：region 缺失退回 fixed 时打上，迁回 region 时移除自
    动还原（同会话条 ATTR_SESSION_BAR_FIXED 模式） */
  var MODELS_FALLBACK_BOTTOM_PX = 120; /* 模型行容器兜底 fixed 的
    bottom：位于会话条兜底（SESSION_BAR_BOTTOM_PX = 96）之上，留出会
    话行高与底板间距 */
  /* ---- V25：速度聚合段与模型分组速度行常量 ---- */
  var VAR_PAD_TOP = "--zbar-usage-pad-top"; /* 输入区容器顶部留白（px）：
    syncComposerPad 按会话条与模型行两个开关的挂载态统一计算后施加
    （V29 起两个注入层独立叠加，见几何常量注释块；基数 26px 兜底），
    全关/迁移/移除时 removeProperty 还原 CSS 兜底 26px */
  var MODEL_ROWS_MAX = 3; /* 模型行上限（数据端已截断到 3，渲染端再兜一刀） */
  /* ---- V26/V29 模型区底板几何常量（留白计算的唯一依据，与
   * ensureStyle 里 .zbar-usage-mrow / ATTR_MODELS 容器的 CSS 数值一
   * 一同步，改任一侧必须同步另一侧）。V25 的留白增量 = 行数 × 固定
   * 14px，字号调大或底板自带内边距/行距/边框增高后，第三行即压到输
   * 入面板（用户实测截图）；V26 改按实际渲染几何求和，并以
   * --zbar-usage-font-size 的实际值（而非写死 10px）参与计算，字号
   * 调大留白同步放大。V29 留白拆分——padForModels 的「会话条行 +
   * 模型行」合计改为 syncComposerPad 统一计算（每拍渲染尾部求一次
   * 值，防会话条/模型行两条渲染路径互相覆盖），两个开关独立叠加：
   *   会话条开（挂进 region）→ need = SESSION_BAR_TOP_PX + 字号×1.5
   *     （会话行底缘，行高系数与容器 line-height 同源）
   *   模型行开（容器挂进 region 且有数据）→ need = SESSION_BAR_TOP_PX
   *     + 字号×1.5 + MODELS_MARGIN_TOP_PX + modelsPlateHeight(有效行
   *     数, 字号) + MODELS_CLEARANCE_PX（底板 top 恒定在会话行下方，
   *     见 ATTR_MODELS 定位样式——会话条关闭时底板位置不变，故两种
   *     组合的需求值相同，取前者即被后者覆盖）
   *   两者全关 → 还原基数 COMPOSER_PAD_TOP_PX（26px：V13 起输入区顶
   *     部留白基数，CSS 规则静态生效、与会话条 top:4px + 默认字号行
   *     高的原始组合对应；两者全关保持 V13 以来行为，不新增"移除留
   *     白"路径）
   *   结果取 max(26, need) 后经 --zbar-usage-pad-top 行内变量施加 ---- */
  var SESSION_BAR_TOP_PX = 4; /* 会话条容器 top（CSS 内写死 top:4px 须与
    本常量同步，同 COMPOSER_PAD_TOP_PX 模式；V29 起同时参与模型底板
    的 top calc 与留白计算） */
  var MODEL_ROW_H_FACTOR = 1.6; /* 模型行高系数（.zbar-usage-mrow height
    calc(字号 * 1.6) 同源；V25 为 1.4，行距过小一并放宽） */
  var MODEL_ROW_GAP_PX = 3; /* 模型行间距（.zbar-usage-models 的 gap 同步） */
  var MODELS_PAD_V_PX = 5; /* 底板纵向内边距（padding:5px 9px 纵向值同步） */
  var MODELS_MARGIN_TOP_PX = 5; /* 底板与会话行的间距（V25~V28 为容器
    margin-top；V29 起消费方为独立容器的 top calc 与留白计算，数值
    语义不变；V35 起容器水平几何在卡片命中时改由行内 left/width 对齐
    输入框卡片接管，样式表居中退为未命中回退） */
  var MODELS_CLEARANCE_PX = 6; /* 底板下沿与输入卡片的安全间隙 */
  /* 模型行三档变色阈值（与免注入悬浮窗模型速度区、主面板速度卡同阈值） */
  var TPS_FAST_MIN = 70; /* ≥ 绿 */
  var TPS_MID_MIN = 40; /* ≥ 黄，其余红 */
  /* ---- V27：额度三档阈值（剩余百分比；V28 起段文本 "额度 5h 余78%"/
   * "周余7%" 于独立额度行渲染）----
   * 剩余充足（≥30）继承行颜色（灰白，不抢视觉）；偏低（<30）黄、
   * 紧张（<10）红——与模型行三档同色系（#fbbf24/#f87171），醒目不刺眼 */
  var QUOTA_MID_REMAIN = 30;
  var QUOTA_LOW_REMAIN = 10;
  /* ---- V33 亮度层级 class（全组件共用，文字三级层级靠亮度+字重不靠
   *      字号）---- */
  var CLS_DIM = "zbar-usage-dim"; /* 标签/单位弱色：相对透明度 .66（随行
    透明 --zbar-usage-opacity 比例衰减，每轮条/会话条继承行色可用） */
  var CLS_STRONG = "zbar-usage-strong"; /* 主数值加粗一档（font-weight
    600 + 前景色，会话条 Σ 数值） */
  /* ---- V26：语言判定（三信号优先级，任一命中即定，缓存一次） ----
   * V25 只按 navigator.language 判定，在 ZCode 的 WebView 里返回英文，
   * 中文界面全部显示英文标签（用户实测反馈）。V26 改：
   * ① DOM 探测（最优先，直接反映用户所见界面语言）：documentElement
   *    的 lang 以 zh 开头，或页面可见文本前 2000 字符含 CJK 字符；
   * ② navigator.language 以 zh 开头；
   * ③ 默认 zh（本项目用户主体中文，且中文标签更短更整齐）。
   * SPA 首帧尚未渲染时文本探测可能拿不到 CJK，由 ②/③ 兜底，判定结果
   * 仍为 zh；不在渲染路径重复探测 */
  var LANG_ZH = (function () {
    try {
      var docLang =
        document.documentElement.lang ||
        document.documentElement.getAttribute("lang") ||
        "";
      if (/^zh/i.test(String(docLang))) return true;
      var text =
        (document.body &&
          (document.body.innerText || document.body.textContent)) ||
        "";
      if (/[\u4e00-\u9fff]/.test(String(text).slice(0, 2000))) return true;
    } catch (e) {
      /* DOM 探测失败继续下一信号 */
    }
    try {
      var navLang = navigator.language || navigator.userLanguage || "";
      if (/^zh/i.test(String(navLang))) return true;
    } catch (e2) {
      /* 读不到语言走默认 */
    }
    return true;
  })();
  /* ---- V26 语言词典（集中一个常量对象，彻底废弃 V25 的 avg/fast/min、
   * avg/best/worst 与 recent 三套混用）：
   * - avg/fast/slow：紧凑聚合段（"均38 快72 慢12"）与模型行跟随小字
   *   （"· 均 38.4 · 快 71.6 · 慢 12.4"）共用同一组标签；
   * - sep：标签与数值间是否插空格（中文紧邻"均38"，英文插空格"avg 38"）；
   * - latest / n：tooltip 的"最近/latest"与样本数单位"笔/requests"；
   * - quota5h：V28 独立额度行（zh 初版 "额度 5h 余78% · 周余93%"，en
   *   "Quota 5h 78% · wk 93%"）；V29 删除 updated 更新时间戳尾段词（时
   *   间戳整体下线）；V30 起行尾再补 cachePct「缓存」段（当前会话命中
   *   率，zh "缓存" / en "cache"）；V32 扁平化去括号，zh 改 "5h " / "每
   *   周 "（与 en 同款尾随空格，「标签 + 裸数值」），行形 "额度 5h 65%
   *   3h02m · 每周 93% 6d21h · 缓存 88.7%"；V46 删除失去消费方的标签词
   *   quotaLead/quotaWk/cachePct（底部状态条去文字标签，quotaLead 前导
   *   与每周/缓存标签删除、配 battery/repeat 图标，5h 前缀保留——"5h
   *   65%" 中 5h 是窗口标识而非列标签），口径说明全部由各段 title 承担。
   * en 词表保留为完整对照（判定三信号全为 zh 命中，见 LANG_ZH），后续
   * 若需放开英文界面或测试驱动可直接切换 */
  var WORDS = {
    zh: {
      avg: "均",
      fast: "快",
      slow: "慢",
      sep: "",
      latest: "最近",
      n: "笔",
      quota5h: "5h ",
      /* V33 底部状态条分段词条与 title 文案（V46 去文字标签：sess/tools/
       * today/subs 标签词删除，仅 title 词条保留） */
      turn: "本轮",
      req: "req",
      tIn: "输入",
      tOut: "输出",
      tCr: "缓存读",
      tCw: "缓存写",
      cwTitle: "缓存写 token",
      speedTitle: "最近一笔请求的生成速度；≥70 绿 / ≥40 黄 / <40 红（t/s）",
      sessTitle: "当前会话累计 Σ=非缓存输入+输出+缓存读（会话树口径，含子代理与失败轮）",
      toolsTitle: "会话树全部工具调用",
      todayTitle: "今日全部会话合计（本地自然日）",
      subsTitle: "并入主轮的子代理轮数；蓝点=子代理运行中",
      quota5hTitle: "5 小时窗口剩余；<30% 黄 / <10% 红",
      quotaWkTitle: "每周窗口剩余；<30% 黄 / <10% 红",
      cacheTitle: "当前会话缓存命中率 = 缓存读 ÷（缓存读 + 非缓存输入）"
    },
    en: {
      avg: "avg",
      fast: "fast",
      slow: "slow",
      sep: " ",
      latest: "latest",
      n: "requests",
      quota5h: "5h ",
      /* V33 底部状态条分段词条与 title 文案（V46 去文字标签，同 zh） */
      turn: "turn",
      req: "req",
      tIn: "in",
      tOut: "out",
      tCr: "cache rd",
      tCw: "cache wr",
      cwTitle: "Cache write tokens",
      speedTitle: "Latest generation speed; >=70 green / >=40 yellow / <40 red (t/s)",
      sessTitle: "Current session totals = plain input + output + cache read (session tree incl. subagents)",
      toolsTitle: "All tool calls in the session tree",
      todayTitle: "All sessions today (local calendar day)",
      subsTitle: "Subagent turns merged into main turns; dot = subagent running",
      quota5hTitle: "5-hour window remaining; <30% yellow / <10% red",
      quotaWkTitle: "Weekly window remaining; <30% yellow / <10% red",
      cacheTitle: "Session cache-hit rate = cache read / (cache read + plain input)"
    }
  };
  var W = WORDS[LANG_ZH ? "zh" : "en"];

  /* ---- 定位同目录 usage-data.js：由注入行自身 src 推导目录 ---- */
  var dataUrl = "";
  var speedUrl = "";
  var quotaUrl = "";
  try {
    var tag =
      document.currentScript ||
      document.querySelector("script[data-zbar-usage]");
    if (tag && tag.src) {
      /* percent-encoded 路径不含裸 "?"，split 取基址安全（同 effects.js） */
      var baseUrl = tag.src.split("?")[0].replace(/[^/]*$/, "");
      dataUrl = baseUrl + "usage-data.js";
      speedUrl = baseUrl + "usage-speed.js";
      quotaUrl = baseUrl + "usage-quota.js"; /* V27 额度旁路小文件 */
    }
  } catch (e) {
    dataUrl = "";
  }
  if (!dataUrl || !speedUrl) return; /* 拿不到自身地址就无法定位数据文件 */

  /* ---- 样式：内联创建样式表，不依赖 ZCode 类名；前景继承 + 半透明
   *      深浅主题均可读。V4 起字号/不透明度消费 variables.css 的
   *      --zbar-usage-font-size / --zbar-usage-opacity（兜底为 V3 写死值
   *      10px / .55；variables.css 由 effects.js 每秒热重载，var() 取值
   *      随变量自动重算，本样式表无需重建）。等待态低透明 = 正常态 ×
   *      0.636（默认 0.55×0.636≈0.35，与 V3 写死 .35 观感一致）。
   *      V5 追加会话累计条样式：pointer-events:none 防挡输入区交互、
   *      tabular-nums 防数字跳动宽度抖动，字号/不透明度与每轮条共用
   *      同一批变量。V13 会话条定位：absolute 挂进输入区容器（挂载见
   *      renderSessionBar）顶部留白（本函数注入的 region 规则，高度见
   *      COMPOSER_PAD_TOP_PX 注释，CSS 内写死 26px 须与常量同步）；
   *      region 缺失的兜底路径经 data-zbar-usage-session-fixed 属性切
   *      换为 fixed + bottom（SESSION_BAR_BOTTOM_PX，旧 V5 写法）---- */
  function ensureStyle() {
    if (document.getElementById(STYLE_ID)) return;
    var st = document.createElement("style");
    st.id = STYLE_ID;
    st.textContent =
      "[" + ATTR_ROW + "]{" +
      "display:block;margin:2px 0 0 1px;" +
      "font-family:var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace);" +
      "font-size:var(--zbar-usage-font-size,10px);" +
      "font-weight:400;line-height:1.5;" +
      "letter-spacing:.02em;color:inherit;opacity:var(--zbar-usage-opacity,.55);" +
      "font-variant-numeric:tabular-nums;" +
      "user-select:none;-webkit-user-select:none;" +
      "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}" +
      /* V8：等待态渲染分支已删除（活动轮判定不再需要 data-running），
       * 本样式保留作防御；当前无渲染路径产出 waiting 态 */
      "[" + ATTR_ROW + "][data-zbar-usage-row-state=waiting]{opacity:calc(var(--zbar-usage-opacity,.55)*.636)}" +
      /* V13：输入区容器顶部留白（基数 26px，与 COMPOSER_PAD_TOP_PX 同步）
       * ——会话条 DOM 挂进容器、住进留白，输入框多行/窗口缩放随文档流
       * 自动跟随。V25：留白改为读 --zbar-usage-pad-top（兜底 26px），
       * renderSessionBar 在模型行显示时以行内变量把留白抬到 26 + 14×行数
       * px（模型行住进多出来的留白里），隐藏/迁移时 removeProperty 还原。
       * V37：追加底部留白带 padding-bottom:var(--zbar-usage-pad-bottom,
       * 24px)（基数与 COMPOSER_PAD_BOTTOM_PX 同步，照本条 padding-top 预
       * 留空间的同款模式）——底部统计行住进带内（bottom 5px = QUOTA_BAR_
       * GAP_PX，见 QUOTA_BAR_BOTTOM_PX），与输入框卡片、宿主状态栏上下
       * 双侧各留间隔；V37R 起带高由 syncQuotaBand 按实测行高动态写入变
       * 量（setRegionPadBottom），本兜底值只覆盖页面首帧/写入失败窗口，
       * 行隐藏时保留（同顶部 26px 基数惯例，syncComposerPad 只维护
       * --zbar-usage-pad-top）。
       * relative 通常无副作用（region 自带 z-20，设计上已是定位元素）；
       * 若 ZCode 改用其它定位方案，本 !important 规则有覆盖风险，实机
       * 回访留意 */
      ".chat-composer-region{position:relative !important;padding-top:var(" +
      VAR_PAD_TOP + "," + COMPOSER_PAD_TOP_PX + "px) !important;}" +
      /* V40：底部留白带只在统计条实际可见时生效（属性由
       * setRegionPadBottom 打上、removeQuotaBar/固定兜底路径清除）——
       * 条关闭/空会话不再永久给输入区垫底 */
      ".chat-composer-region[" + ATTR_QUOTA_BAND + "]{" +
      "padding-bottom:var(" + VAR_PAD_BOTTOM + "," +
      COMPOSER_PAD_BOTTOM_PX + "px) !important;}" +
      /* 会话累计条（V5）：不占消息布局。V13 定位改 absolute：住进输入
       * 区容器顶部留白（left + translateX 水平居中），不再 fixed/bottom。
       * V35：卡片命中时行内 left + transform:none 覆盖为左缘对齐输入框
       * 卡片（applyCardAlign），本居中规则退为未命中回退。
       * V25：容器改为纵向 flex（逐段 span 行在上、模型行容器在下）。
       * V26：align-items:center → stretch——模型底板与会话行同一左边
       * 界、同一宽度锚定（条宽由最宽子元素撑起，两段都拉满，左缘天然
       * 对齐；绝对定位的 left/translateX 仍按条宽水平居中不变）。
       * V26：整条低透明（--zbar-usage-opacity）从容器下移到会话行——
       * 模型底板自供对比度（深色半透明底 + 近白文字），不吃低透明，
       * 否则底板与文字同被压暗、壁纸上仍不可读（用户实测反馈主因）。
       * V29：模型行容器拆出为独立元素（ATTR_MODELS，独立开关），本容
       * 器现仅承载逐段 span 行；纵向 flex/stretch 结构保留（单行场景
       * 行为与块级等价，避免无谓改写定位几何） */
      "[" + ATTR_SESSION_BAR + "]{" +
      "position:absolute;top:4px;left:50%;transform:translateX(-50%);" +
      "z-index:" + SESSION_BAR_Z + ";" +
      "display:flex;flex-direction:column;align-items:stretch;" +
      "font-family:var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace);" +
      "font-size:var(--zbar-usage-font-size,10px);" +
      "font-weight:400;line-height:1.5;" +
      "letter-spacing:.02em;color:inherit;" +
      "font-variant-numeric:tabular-nums;" +
      "user-select:none;-webkit-user-select:none;" +
      "pointer-events:none;white-space:nowrap;}" +
      /* V26：会话行承接原容器低透明（观感与 V25 及更早版本一致）。
       * 本行是 V25 会话条总量行的逐段 span 宿主——旧实现直接写在
       * 会话条容器上，行内文本比对与重建只在行元素上进行（V29 起模
       * 型行容器已拆出，行元素是本容器唯一子节点） */
      "[" + ATTR_SESSION_BAR + "] .zbar-usage-sb-line{" +
      "white-space:nowrap;opacity:var(--zbar-usage-opacity,.55);}" +
      /* V13 兜底：region 缺失时条挂 body 并打上标记，退回旧 V5 行为
       * （fixed 贴窗底，bottom 见 SESSION_BAR_BOTTOM_PX） */
      "[" + ATTR_SESSION_BAR + "][" + ATTR_SESSION_BAR_FIXED + "]{" +
      "position:fixed;top:auto;bottom:" + SESSION_BAR_BOTTOM_PX + "px;}" +
      /* V29 独立模型行容器（自会话条拆出）：absolute 挂进输入区容器顶
       * 部留白、住进会话行下方——top = 会话条 top + 会话行高（字号 ×
       * 1.5，与容器 line-height 同源）+ 底板间距，常量经字符串拼接与
       * SESSION_BAR_TOP_PX / MODELS_MARGIN_TOP_PX 一一同步；top 恒定，
       * 会话条开关切换（会话行出现/消失）不引起模型行跳位。left +
       * translateX 按自身宽度水平居中（与上方会话行同一轴线）。容器
       * pointer-events:none 不挡输入区，行内 auto 保 title 悬浮（与会
       * 话条容器同款语义）。底板组件（V26）：半透明深色底 + 细边框 +
       * 圆角 + 毛玻璃 + 内边距，任何壁纸上可读；亮暗两档背景随
       * prefers-color-scheme 适配；:empty 整体隐藏（无模型行时不得露
       * 出空底板），行距 gap 见几何常量（MODEL_ROW_GAP_PX 同步） */
      "[" + ATTR_MODELS + "]{" +
      "position:absolute;" +
      "top:calc(" + SESSION_BAR_TOP_PX + "px + var(--zbar-usage-font-size,10px) * 1.5 + " + MODELS_MARGIN_TOP_PX + "px);" +
      "left:50%;transform:translateX(-50%);" +
      "z-index:" + SESSION_BAR_Z + ";" +
      "display:flex;flex-direction:column;align-items:stretch;" +
      "gap:" + MODEL_ROW_GAP_PX + "px;" +
      "font-family:var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace);" +
      "font-size:var(--zbar-usage-font-size,10px);" +
      "line-height:1;" +
      "padding:" + MODELS_PAD_V_PX + "px 9px;box-sizing:border-box;" +
      /* V33：底板背景/边框改消费客户端语义变量（随 .dark 翻转），缺失
       * 回退 V26 恒深底；亮暗 prefers-color-scheme 两档媒体查询随之下
       * 线（变量存在时由客户端接管，回退值保持原深底观感） */
      "border-radius:8px;border:1px solid var(--color-border, rgba(148,163,184,.28));" +
      "background:var(--color-card, rgba(2,6,23,.72));" +
      "-webkit-backdrop-filter:blur(6px);backdrop-filter:blur(6px);" +
      /* V34：文字改消费会翻转的客户端语义变量（P1 实机反馈：恒浅灰
       * rgba(241,245,249,*) 在浅色卡片上几乎不可见）；兜底取中灰
       * #5b6577——变量缺失时恒深底（回退值）上可读、极端浅底上也可读，
       * 不再用任何深色主题专用浅灰。三档速度色保留硬编码（数据语义色） */
      "color:var(--color-foreground, #5b6577);opacity:1;" +
      "pointer-events:none;white-space:nowrap;}" +
      /* V29 兜底：region 缺失时挂 body 并打上标记，退回 fixed 贴底
       * （bottom 见 MODELS_FALLBACK_BOTTOM_PX，位于会话条兜底之上；
       * left/translateX 居中沿用基础规则） */
      "[" + ATTR_MODELS + "][" + ATTR_MODELS_FIXED + "]{" +
      "position:fixed;top:auto;bottom:" + MODELS_FALLBACK_BOTTOM_PX + "px;}" +
      /* V33：V26 的亮色媒体查询已删（底板背景/边框改消费客户端
       * --color-card/--color-border 语义变量，浅色主题由客户端变量自
       * 动翻转，缺失回退值保持原深底观感） */
      "[" + ATTR_MODELS + "]:empty{display:none;}" +
      /* V25 模型分组速度行（对齐免注入悬浮窗模型速度区：小号、等宽、
       * 最近值三档变色 + 单位 + 跟随灰字）。V26：行内改弹性——模型名
       * flex:1 1 auto 占满余量（min-width:0 + ellipsis，截断只发生在
       * 模型名），数值区 flex:none + nowrap 永不截断（用户实测行尾
       * worst 值被整段裁掉，根因是 mextra flex:0 1 auto 可收缩）；删除
       * 旧 70vw/420px 双上限的独立宽度（宽度随底板自适应）；行高 =
       * 字号 × 1.6（与 MODEL_ROW_H_FACTOR 同源，须同步），
       * pointer-events:auto 让行可悬浮出 title（容器为
       * pointer-events:none 不挡输入区，模型行住在上方留白里、不与输入
       * 卡片重叠） */
      "[" + ATTR_MODELS + "] .zbar-usage-mrow{" +
      "display:flex;align-items:center;gap:6px;" +
      "height:calc(var(--zbar-usage-font-size,10px) * " + MODEL_ROW_H_FACTOR + ");" +
      "font-size:calc(var(--zbar-usage-font-size,10px) * .9);" +
      "line-height:1;pointer-events:auto;cursor:default;}" +
      /* 行间细分割线（V33 改消费客户端 --color-border 语义变量；行距由
       * 容器 gap 承担） */
      "[" + ATTR_MODELS + "] .zbar-usage-mrow + .zbar-usage-mrow{" +
      "border-top:1px solid var(--color-border, rgba(148,163,184,.22));}" +
      /* 模型名：唯一可收缩截断的段（flex:1 1 auto 吃掉余量，数值区恒
       * 右缘对齐；名字过长时 ellipsis，行 title 兜底全名） */
      "[" + ATTR_MODELS + "] .zbar-usage-mname{" +
      "flex:1 1 auto;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;}" +
      /* 数值区（最近值三档变色）：永不收缩、永不截断 */
      "[" + ATTR_MODELS + "] .zbar-usage-mval{" +
      "flex:none;white-space:nowrap;font-variant-numeric:tabular-nums;}" +
      /* 单位后缀：独立静态节点，不参与三档变色（同免注入模型速度区）。
       * V34：弱色改消费客户端 --color-foreground-subtle（随主题翻转，
       * 中灰兜底），替代相对透明度衰减（浅底上不可见的根因之一） */
      "[" + ATTR_MODELS + "] .zbar-usage-munit{" +
      "flex:none;white-space:nowrap;font-size:calc(var(--zbar-usage-font-size,10px) * .8);" +
      "color:var(--color-foreground-subtle, #5b6577);}" +
      /* 均 / 快 / 慢跟随小字：弱色一档表达层级；flex:none 保证数值永不
       * 截断（V26 修复，原 flex:0 1 auto 会被行尾裁切）。V34 同单位后
       * 缀改客户端弱色变量 */
      "[" + ATTR_MODELS + "] .zbar-usage-mextra{" +
      "flex:none;white-space:nowrap;" +
      "font-size:calc(var(--zbar-usage-font-size,10px) * .8);" +
      "color:var(--color-foreground-subtle, #5b6577);}" +
      /* 三档变色：V26 起恒用亮色系——底板恒为深色（见 ATTR_MODELS 容
       * 器背景），原亮色主题的暗色系（#059669/#d97706/#dc2626）在深底
       * 上对比度不足，删除原亮暗两套媒体查询 */
      "[" + ATTR_MODELS + "] .zbar-usage-tps-fast{color:#34d399;}" +
      "[" + ATTR_MODELS + "] .zbar-usage-tps-mid{color:#fbbf24;}" +
      "[" + ATTR_MODELS + "] .zbar-usage-tps-slow{color:#f87171;}" +
      /* V27 额度段三档配色：剩余充足（≥30）继承会话行颜色（无 class，
       * 灰白不抢视觉），偏低（<30）黄、紧张（<10）红——与模型行三档
       * 同色系（醒目不刺眼，底色随壁纸但行色已由主题保证可读）。
       * V28 起该配色由独立额度行消费（会话条不再携带额度段，规则保留
       * 供旧落盘脚本过渡期渲染，随版本覆盖自然消失） */
      "[" + ATTR_SESSION_BAR + "] .zbar-usage-quota-mid{color:#fbbf24;}" +
      "[" + ATTR_SESSION_BAR + "] .zbar-usage-quota-low{color:#f87171;}" +
      /* V33 全组件亮度层级 class（文字三级层级靠亮度+字重不靠字号）：
       * dim = 标签/单位弱色（相对透明度，随行透明比例衰减），
       * strong = 主数值加粗一档 */
      ".zbar-usage-dim{opacity:.66;}" +
      ".zbar-usage-strong{font-weight:600;}" +
      /* V33 每轮条速度三档色（与模型行同阈值同色系，class 由 tpsClassOf
       * 产出；模型行 scoped 规则保留，两条渲染路径互不影响）。V34：会
       * 话条速度值同款三档色（P1 实机反馈：白/灰与每轮条/底部条不一致） */
      "[" + ATTR_ROW + "] .zbar-usage-tps-fast{color:#34d399;}" +
      "[" + ATTR_ROW + "] .zbar-usage-tps-mid{color:#fbbf24;}" +
      "[" + ATTR_ROW + "] .zbar-usage-tps-slow{color:#f87171;}" +
      "[" + ATTR_SESSION_BAR + "] .zbar-usage-tps-fast{color:#34d399;}" +
      "[" + ATTR_SESSION_BAR + "] .zbar-usage-tps-mid{color:#fbbf24;}" +
      "[" + ATTR_SESSION_BAR + "] .zbar-usage-tps-slow{color:#f87171;}" +
      /* V41：底部状态条恢复 V36 的字段集（速度 / 本轮 / 会话 / 工具 /
       * 今日 / 额度 5h / 每周 / 缓存），不再把其它 Agent 的订阅额度
       * 注入这里。容器按输入框卡片对齐（renderQuotaBar 里
       * applyCardAlign 写 left/width），高度 46px，上下各留
       * QUOTA_BAR_GAP_PX 的呼吸位。条本身不占布局空间（留白带由
       * region 的 padding-bottom 参与），外层 pointer-events:none、
       * 胶囊本体 auto，不挡输入区交互。字号/高度基准由
       * --zbar-usage-font-size 控制 */
      "[" + ATTR_QUOTA_BAR + "]{" +
      "position:absolute;bottom:" + QUOTA_BAR_BOTTOM_PX + "px;left:0;right:0;" +
      "z-index:" + SESSION_BAR_Z + ";" +
      "max-width:100%;" +
      "font-family:var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace);" +
      "font-size:var(--zbar-usage-font-size,10px);" +
      "font-weight:400;line-height:1.2;" +
      "letter-spacing:.015em;color:inherit;" +
      "font-variant-numeric:tabular-nums;" +
      "user-select:none;-webkit-user-select:none;" +
      "pointer-events:none;}" +
      /* V44 胶囊本体：占满容器（容器已与输入框卡片同宽），固定
       * max(32px, 字号×2.3) 高；溢出适配由 V34 优先级整段隐藏改为横向
       * 滚动——qb-line 自身成为滚动容器（overflow-x:auto），居中让位
       * 给 qfull 的 margin:0 auto（justify-content:center 在溢出时
       * scrollLeft=0 左端内容不可达，flex-start + 子项 auto margin 溢
       * 出时归零、内容锚左可完整滚到两端）。滚动条 V45 起显式矮化为 4px
       * 细条，qb-line 重置标准 scrollbar-width/scrollbar-color 防 ZCode
       * 全局样式禁用 webkit 伪元素（ZCode 为 Electron/Chromium） */
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qb-line{" +
      "position:relative;display:flex;align-items:center;justify-content:flex-start;" +
      "box-sizing:border-box;width:100%;max-width:100%;" +
      "height:max(32px, calc(var(--zbar-usage-font-size,10px) * 2.3));" +
      "padding:0 calc(var(--zbar-usage-font-size,10px) * .9);" +
      "white-space:nowrap;overflow-x:auto;overflow-y:hidden;pointer-events:auto;" +
      "background:var(--color-card, rgba(28,30,36,.92));" +
      "border:1px solid var(--color-border, rgba(127,127,127,.22));" +
      "border-radius:10px;" +
      "box-shadow:inset 0 1px 0 rgba(255,255,255,.05), 0 8px 22px rgba(0,0,0,.26), 0 1px 3px rgba(0,0,0,.18);" +
      "-webkit-backdrop-filter:blur(12px) saturate(1.2);backdrop-filter:blur(12px) saturate(1.2);" +
      "opacity:var(--zbar-usage-opacity,.55);scrollbar-width:auto;scrollbar-color:auto;}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qb-line::-webkit-scrollbar{height:4px;}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qb-line::-webkit-scrollbar-thumb{background:rgba(127,127,127,.4);border-radius:2px;}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qb-line::-webkit-scrollbar-track{background:transparent;}" +
      /* 完整分段容器：flex:none 保证内容不被压扁不换行；margin:0 auto
       * 在 flex 容器里吸收剩余空间——不溢出时视觉居中，溢出时归零锚左
       * 交给滚动（V44 居中与滚动的两全解） */
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qfull{" +
      "display:flex;align-items:center;flex:none;margin:0 auto;" +
      "gap:calc(var(--zbar-usage-font-size,10px) * .55);" +
      "white-space:nowrap;pointer-events:auto;}" +
      /* V41 分段：弹性盒项；容器 pointer-events:none、分段 auto——只有
       * 分段本身捕获指针出 title/悬停，不挡输入区交互（同模型行语义）。
       * 段内水平 padding/间距/图标尺寸随字号缩放，发丝分隔线 = 相邻
       * 兄弟 ::before，分段整段省略时相邻选择器自动无残留；悬停极轻
       * 高亮 */
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qseg{" +
      "position:relative;display:flex;align-items:center;flex:none;" +
      "gap:calc(var(--zbar-usage-font-size,10px) * .34);white-space:nowrap;" +
      "padding:0 calc(var(--zbar-usage-font-size,10px) * .36);" +
      "pointer-events:auto;cursor:default;border-radius:8px;}" +
      /* V44 左右缘渐隐提示：mask 挂 qb-line 本体（滚动容器）——mask 只
       * 作用于绘制层，不拦截指针（分段 title/悬停照常）、不裁掉可滚
       * 动内容（滚动可达范围不变，渐隐纯视觉），且随胶囊自身透明度与
       * 主题色自动一致（对比覆盖层方案无需浅色主题配套规则）。状态
       * class 由 syncQuotaFade 按滚动位置切换：未滚动仅右缘提示还有
       * 内容、滚动位置>0 加左缘、滚到最右右缘消失，无溢出两侧全无。
       * 双侧同时命中走单独的四停靠渐变规则——两条单侧 mask 若同时生
       * 效会以后者覆盖前者（同属性层叠，非合成），不能叠用 */
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qb-line.zbar-usage-qfade-r{" +
      "-webkit-mask-image:linear-gradient(to left, transparent, #000 calc(var(--zbar-usage-font-size,10px) * 1.4));" +
      "mask-image:linear-gradient(to left, transparent, #000 calc(var(--zbar-usage-font-size,10px) * 1.4));}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qb-line.zbar-usage-qfade-l{" +
      "-webkit-mask-image:linear-gradient(to right, transparent, #000 calc(var(--zbar-usage-font-size,10px) * 1.4));" +
      "mask-image:linear-gradient(to right, transparent, #000 calc(var(--zbar-usage-font-size,10px) * 1.4));}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qb-line.zbar-usage-qfade-l.zbar-usage-qfade-r{" +
      "-webkit-mask-image:linear-gradient(to right, transparent, #000 calc(var(--zbar-usage-font-size,10px) * 1.4), #000 calc(100% - var(--zbar-usage-font-size,10px) * 1.4), transparent);" +
      "mask-image:linear-gradient(to right, transparent, #000 calc(var(--zbar-usage-font-size,10px) * 1.4), #000 calc(100% - var(--zbar-usage-font-size,10px) * 1.4), transparent);}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qseg + .zbar-usage-qseg::before{" +
      "content:\"\";position:absolute;left:0;top:20%;bottom:20%;width:1px;" +
      "background:var(--color-border, rgba(127,127,127,.22));}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qseg:hover{" +
      "background:rgba(127,127,127,.07);" +
      "background:color-mix(in srgb, currentColor 6%, transparent);}" +
      /* V33 文字三级层级（底部条侧）：主数值 600 + 前景色（qval）、标
       * 签/单位弱色（qdim = 客户端 --color-foreground-subtle 兜底灰）、
       * 次要信息更弱一档（qfaint = 同弱色 + 0.72 透明，重置倒计时）。
       * 额度段三档变色挂在分段 span 上——主数值部分继承变色，倒计时
       * 子 span 由 qfaint 显式回灰，阈变色仍只属于剩余百分比 */
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qval{font-weight:600;}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qdim{color:var(--color-foreground-subtle, rgba(120,130,145,.95));}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qfaint{color:var(--color-foreground-subtle, rgba(120,130,145,.95));opacity:.72;}" +
      /* 分段行内 SVG 图标（createElementNS 构建，stroke 随 currentColor
       * 不单独上色；尺寸 = 字号 × 1.2；fill none + round cap/join + 线宽
       * 1.7）；与文字间距由段容器 flex gap 统一承担 */
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qico{" +
      "width:calc(var(--zbar-usage-font-size,10px) * 1.2);" +
      "height:calc(var(--zbar-usage-font-size,10px) * 1.2);" +
      "flex:none;fill:none;stroke:currentColor;stroke-width:1.7;" +
      "stroke-linecap:round;stroke-linejoin:round;opacity:.9;}" +
      /* 子代理呼吸蓝点（2px，轻量 opacity 脉动；动画禁用偏好下静态常亮） */
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qdot{" +
      "width:2px;height:2px;border-radius:50%;background:#60a5fa;margin-left:4px;" +
      "flex:none;animation:zbarUsagePulse 2s ease-in-out infinite;}" +
      "@keyframes zbarUsagePulse{0%,100%{opacity:.25;}50%{opacity:1;}}" +
      "@media (prefers-reduced-motion: reduce){" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-qdot{animation:none;opacity:.85;}}" +
      /* 速度段三档色（与模型行/每轮条同阈值同色系） */
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-tps-fast{color:#34d399;}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-tps-mid{color:#fbbf24;}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-tps-slow{color:#f87171;}" +
      /* V28 兜底：region 缺失时挂 body 并打上标记，退回 fixed 贴底；V41
       * 起整条占满视口宽度（正常路径由 applyCardAlign 对齐输入框卡片） */
      "[" + ATTR_QUOTA_BAR + "][" + ATTR_QUOTA_BAR_FIXED + "]{" +
      "position:fixed;top:auto;left:0;right:0;transform:none;" +
      "bottom:" + QUOTA_BAR_FALLBACK_BOTTOM_PX + "px;}" +
      /* V29 起额度三档配色（充足继承行色灰白、<30 黄、<10 红）；V33 起
       * 类挂在分段 span 上（倒计时子 span 经 qfaint 显式回灰，见上） */
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-quota-mid{color:#fbbf24;}" +
      "[" + ATTR_QUOTA_BAR + "] .zbar-usage-quota-low{color:#f87171;}" +
      /* V36 浅色主题覆盖（白底下注入条文字可读性）：规则全部带
       * html:not(.dark) 前缀（:not(.dark) 计入特异性，均高于对应原规
       * 则），浅色主题命中覆盖、深色主题不命中（深色渲染零变化，上方
       * 原规则原样保留）。a) 透明度下限：三处行透明消费点（每轮条容
       * 器/会话条行/底部条行）以 max() 兜 .82 底（用户调高 >.82 仍取
       * 用户值），dim/qfaint 弱色同步提亮；b) 语义色浅色加深变体：四
       * 组容器 tps 三档 + 会话条/底部条 quota 两档改用白底可读的加深
       * 版（#059669/#d97706/#dc2626；模型底板 V33 起底色消费
       * --color-card 随主题翻转，浅色下恰为浅底） */
      "html:not(.dark) [" + ATTR_ROW + "]{opacity:max(var(--zbar-usage-opacity,.55),.82);}" +
      "html:not(.dark) [" + ATTR_SESSION_BAR + "] .zbar-usage-sb-line{opacity:max(var(--zbar-usage-opacity,.55),.82);}" +
      "html:not(.dark) [" + ATTR_QUOTA_BAR + "] .zbar-usage-qb-line{opacity:max(var(--zbar-usage-opacity,.55),.82);}" +
      "html:not(.dark) .zbar-usage-dim{opacity:.82;}" +
      "html:not(.dark) [" + ATTR_QUOTA_BAR + "] .zbar-usage-qfaint{opacity:.85;}" +
      "html:not(.dark) [" + ATTR_MODELS + "] .zbar-usage-tps-fast{color:#059669;}" +
      "html:not(.dark) [" + ATTR_MODELS + "] .zbar-usage-tps-mid{color:#d97706;}" +
      "html:not(.dark) [" + ATTR_MODELS + "] .zbar-usage-tps-slow{color:#dc2626;}" +
      "html:not(.dark) [" + ATTR_ROW + "] .zbar-usage-tps-fast{color:#059669;}" +
      "html:not(.dark) [" + ATTR_ROW + "] .zbar-usage-tps-mid{color:#d97706;}" +
      "html:not(.dark) [" + ATTR_ROW + "] .zbar-usage-tps-slow{color:#dc2626;}" +
      "html:not(.dark) [" + ATTR_SESSION_BAR + "] .zbar-usage-tps-fast{color:#059669;}" +
      "html:not(.dark) [" + ATTR_SESSION_BAR + "] .zbar-usage-tps-mid{color:#d97706;}" +
      "html:not(.dark) [" + ATTR_SESSION_BAR + "] .zbar-usage-tps-slow{color:#dc2626;}" +
      "html:not(.dark) [" + ATTR_QUOTA_BAR + "] .zbar-usage-tps-fast{color:#059669;}" +
      "html:not(.dark) [" + ATTR_QUOTA_BAR + "] .zbar-usage-tps-mid{color:#d97706;}" +
      "html:not(.dark) [" + ATTR_QUOTA_BAR + "] .zbar-usage-tps-slow{color:#dc2626;}" +
      "html:not(.dark) [" + ATTR_SESSION_BAR + "] .zbar-usage-quota-mid{color:#d97706;}" +
      "html:not(.dark) [" + ATTR_SESSION_BAR + "] .zbar-usage-quota-low{color:#dc2626;}" +
      "html:not(.dark) [" + ATTR_QUOTA_BAR + "] .zbar-usage-quota-mid{color:#d97706;}" +
      "html:not(.dark) [" + ATTR_QUOTA_BAR + "] .zbar-usage-quota-low{color:#dc2626;}";
      /* V21 的 CTX 三档变色样式（黄/红两个 class 规则）已随 V22 CTX 段
       * 删除（无渲染路径再产出这两个 class） */
    (document.head || document.documentElement).appendChild(st);
  }

  /* ---- 数据索引：umid（用户消息 id，与 DOM data-turn-id 同值）→ 轮对象 ---- */
  var index = {};
  /* 进行中轮索引（V6）：umid → run 行（model_usage 已落库、turn_usage
   * 尚未写入的轮实时聚合）。完成轮由 index 接管（优先级更高），2 秒
   * 竞态窗口内同 umid 两边并存时按完成数据渲染 */
  var runIndex = {};
  var lastTs = null; /* 上次加载的数据 ts（未变则本轮零成本跳过） */
  /* 最近一次数据的 turns / runs 原始数组：index/runIndex 丢弃了 umid
   * 为 null 的行，而会话累计必须覆盖会话内全部行（含无法与 DOM 匹配
   * 的子代理轮——经 psess 并入），故聚合时遍历原始数组而非索引 */
  var lastTurns = [];
  var lastRuns = [];
  /* V21 会话级统计索引：会话 id → sess 行（数据端 model_usage 全量合计；
   * V22 起 cp/cu/cw 已删，仅余合计字段）。旧数据文件无 sess 数组：索引
   * 为空对象，会话累计回退旧口径 */
  var sessIndex = {};
  /* V25 模型分组速度（数据端顶层 models[]，窗口级、最近使用降序至多 3
   * 组）：旧数据文件无该键 → 空数组，模型行区不渲染（行为与 V24 一致）；
   * 老库缺 model_id 列时数据端整个键省略，同样落在这里 */
  var lastModels = [];
  /* V33 顶层今日合计（本地自然日 0 点起全库 model_usage 全量
   * {tt, rq}）：查询失败/表缺失时数据端整键省略 → null，底部状态条
   * 「今日」分段随之隐藏；只随 2 秒大文件发布，applySpeedData 不触碰 */
  var lastToday = null;
  var lastSpeedTs = null;
  var lastSpeedData = null;

  function applySpeedData(data) {
    if (!data || data.v !== 1) return;
    var turns = data.turns || [];
    var runs = data.runs || [];
    var sessions = data.sess || [];
    var byUmid = {};
    var byTurn = {};
    var byRunUmid = {};
    var bySession = {};
    for (var i = 0; i < turns.length; i++) {
      var t = turns[i];
      if (!t) continue;
      if (t.umid) byUmid[t.umid] = t.speed || null;
      if (t.turn && t.sess) byTurn[t.sess + "\u0000" + t.turn] = t.speed || null;
    }
    for (var j = 0; j < runs.length; j++) {
      var r = runs[j];
      if (r && r.umid) byRunUmid[r.umid] = r.speed || null;
    }
    for (var k = 0; k < sessions.length; k++) {
      var s = sessions[k];
      if (s && s.s) bySession[s.s] = s;
    }
    for (var ti = 0; ti < lastTurns.length; ti++) {
      var lt = lastTurns[ti];
      if (!lt) continue;
      var turnSpeed = null;
      var foundTurn = false;
      if (lt.umid && Object.prototype.hasOwnProperty.call(byUmid, lt.umid)) {
        turnSpeed = byUmid[lt.umid];
        foundTurn = true;
      } else if (lt.turn && lt.sess &&
        Object.prototype.hasOwnProperty.call(byTurn, lt.sess + "\u0000" + lt.turn)) {
        turnSpeed = byTurn[lt.sess + "\u0000" + lt.turn];
        foundTurn = true;
      }
      if (foundTurn) lt.speed = turnSpeed;
    }
    for (var ri = 0; ri < lastRuns.length; ri++) {
      var lr = lastRuns[ri];
      if (lr && lr.umid && Object.prototype.hasOwnProperty.call(byRunUmid, lr.umid)) {
        lr.speed = byRunUmid[lr.umid];
      }
    }
    for (var sk = 0; sk < sessions.length; sk++) {
      var ss = sessions[sk];
      if (!ss || !ss.s) continue;
      if (sessIndex[ss.s]) {
        sessIndex[ss.s].speed = ss.speed || null;
        sessIndex[ss.s].speedState = ss.speedState;
      }
    }
  }

  function rebuildIndex(data) {
    index = {};
    runIndex = {};
    sessIndex = {};
    lastTurns = (data && data.turns) || [];
    lastRuns = (data && data.runs) || [];
    if (data && data.turns && data.turns.length) {
      for (var i = 0; i < data.turns.length; i++) {
        var t = data.turns[i];
        /* umid 为 null 的轮（老版本库缺列/无关联消息）无法与 DOM 匹配 */
        if (t && t.umid) index[t.umid] = t;
      }
    }
    if (data && data.runs && data.runs.length) {
      for (var j = 0; j < data.runs.length; j++) {
        var r = data.runs[j];
        /* 旧导出文件无 runs 数组：lastRuns 兜底为空数组，零影响 */
        if (r && r.umid) runIndex[r.umid] = r;
      }
    }
    if (data && data.sess && data.sess.length) {
      for (var k = 0; k < data.sess.length; k++) {
        var sv = data.sess[k];
        if (sv && sv.s) sessIndex[sv.s] = sv;
      }
    }
    /* V25 模型分组速度：旧数据文件/老库缺 model_id 列时缺键或空数组 →
     * 模型行区不渲染；数据端已按最近使用降序截断，这里再兜一刀上限 */
    lastModels =
      data && data.models && data.models.length
        ? data.models.slice(0, MODEL_ROWS_MAX)
        : [];
    /* V33 今日合计：旧数据文件无 today 键 → null（「今日」分段隐藏）；
     * 字段形态由消费端再校验（tt 非有限数同样隐藏） */
    lastToday = (data && data.today) || null;
    applySpeedData(lastSpeedData);
  }

  /* ---- 历史数据轮询：script 标签方式重载（onload/onerror 均静默） ---- */
  var loading = false;

  function loadData() {
    if (loading || !dataUrl) return;
    loading = true;
    var old = document.getElementById(LOADER_ID);
    if (old && old.parentNode) old.parentNode.removeChild(old);
    var s = document.createElement("script");
    s.id = LOADER_ID;
    s.onload = function () {
      loading = false;
      try {
        var data = window.__ZBAR_USAGE__;
        /* 版本不符视为无效数据，走静默路径（保留上次数据） */
        if (!data || data.v !== 2) return;
        /* ts 未变：Rust 侧仅在内容变化时重写文件，常态重载零成本跳过 */
        if (data.ts === lastTs) return;
        lastTs = data.ts;
        rebuildIndex(data);
        scheduleRender();
      } catch (e) {
        /* 单轮失败静默 */
      }
    };
    s.onerror = function () {
      loading = false; /* 加载失败保留上次数据 */
    };
    s.src = dataUrl + "?t=" + Date.now();
    (document.head || document.documentElement).appendChild(s);
  }

  function pollLoop() {
    try {
      loadData();
    } catch (e) {
      /* 静默 */
    }
    setTimeout(pollLoop, document.hidden ? POLL_HIDDEN_MS : POLL_MS);
  }

  /* ---- 速度小文件轮询：独立于大文件，完成后约一秒内只传输小快照 ---- */
  var speedLoading = false;

  function loadSpeed() {
    if (speedLoading || !speedUrl) return;
    speedLoading = true;
    var old = document.getElementById(SPEED_LOADER_ID);
    if (old && old.parentNode) old.parentNode.removeChild(old);
    var s = document.createElement("script");
    s.id = SPEED_LOADER_ID;
    s.onload = function () {
      speedLoading = false;
      try {
        var data = window.__ZBAR_USAGE_SPEED__;
        if (!data || data.v !== 1) return;
        if (data.ts === lastSpeedTs) return;
        lastSpeedTs = data.ts;
        lastSpeedData = data;
        applySpeedData(data);
        scheduleRender();
      } catch (e) {
        /* 速度旁路失败不影响历史计数 */
      }
    };
    s.onerror = function () {
      speedLoading = false;
    };
    s.src = speedUrl + "?t=" + Date.now();
    (document.head || document.documentElement).appendChild(s);
  }

  function speedPollLoop() {
    try {
      loadSpeed();
    } catch (e) {
      /* 静默 */
    }
    setTimeout(speedPollLoop, document.hidden ? POLL_HIDDEN_MS : SPEED_POLL_MS);
  }

  /* ---- V27 额度小文件轮询：独立低频节拍（数据端 60s 写盘、本端 30s
   *      重载），只承载 5h/周窗口剩余百分比，绝不进历史大文件与速度
   *      旁路；查询未成功（数据端从未写出/文件缺失）时 quotaData 保持
   *      null，V28 起独立额度行整行隐藏（无内容可显示），加载失败保
   *      留上次额度 ---- */
  var quotaLoading = false;
  var quotaData = null; /* { v:1|2, ts, h5, wk, h5rs, wkrs }；h5/wk 与
      h5rs/wkrs 可为 null（窗口缺失）；V31 契约 v:2 新增重置时间戳，
      v:1 为升级过渡期残留旧文件（无倒计时段） */

  function loadQuota() {
    if (quotaLoading || !quotaUrl) return;
    quotaLoading = true;
    var old = document.getElementById(QUOTA_LOADER_ID);
    if (old && old.parentNode) old.parentNode.removeChild(old);
    var s = document.createElement("script");
    s.id = QUOTA_LOADER_ID;
    s.onload = function () {
      quotaLoading = false;
      try {
        var q = window.__ZBAR_QUOTA__;
        /* 版本不符视为无效数据，保留上次额度（与数据/速度旁路同款）；
         * V31 放行 v:1/v:2——升级过渡期残留 v:1 照常消费（无倒计时段），
         * 额度线程写出 v:2 后自然生效 */
        if (!q || (q.v !== 1 && q.v !== 2)) return;
        quotaData = q;
        scheduleRender();
      } catch (e) {
        /* 额度旁路失败不影响统计条 */
      }
    };
    s.onerror = function () {
      quotaLoading = false; /* 加载失败保留上次额度 */
    };
    s.src = quotaUrl + "?t=" + Date.now();
    (document.head || document.documentElement).appendChild(s);
  }

  function quotaPollLoop() {
    try {
      loadQuota();
    } catch (e) {
      /* 静默 */
    }
    setTimeout(quotaPollLoop, document.hidden ? QUOTA_POLL_HIDDEN_MS : QUOTA_POLL_MS);
  }

  /* ---- 格式化（速度口径见文件头说明） ---- */
  function trimTail(s) {
    return s.replace(/\.0$/, "");
  }

  /* V10：token 值等宽补位——恒定 5 字符（"  998"/" 1.2k"/"10.5M"，超过
   * 999.9M 自然溢出），等宽字体 + tabular-nums 下字段占位恒定 */
  function fmtTokens(n) {
    n = Number(n) || 0;
    var sign = n < 0 ? "-" : "";
    n = Math.abs(n);
    var s;
    if (n < 1000) s = String(Math.round(n));
    else if (n < 1000000) s = trimTail((n / 1000).toFixed(1)) + "k";
    else s = trimTail((n / 1000000).toFixed(1)) + "M";
    return (sign + s).padStart(5, " ");
  }

  /* V34 底部条数值格式：K/M 缩写与一位小数同 fmtTokens，去前导等宽
   * 补位空格——底部条各段独立、无跨段对齐需求，段内间距由 flex gap
   * 承担，补位空格会叠加 gap 造成过宽间距（P2 实机反馈近乎粘连）；每
   * 轮条/会话条仍用 fmtTokens 等宽补位不变 */
  function fmtTokensShort(n) {
    return fmtTokens(n).replace(/^ +/, "");
  }

  function fmtSeconds(ms) {
    return (Math.round(ms / 100) / 10).toFixed(1) + "s";
  }

  /* 速度字段只消费 Rust 确认的请求级快照；请求平均值显式带 ≈，缺失
   * 或无效值统一显示等待占位。 */
  function padSpeed(s) {
    if (!s || typeof s.value !== "number" || !isFinite(s.value) || s.value <= 0) {
      return "  – ";
    }
    var prefix = s.quality === "request_average" ? "≈" : "";
    return (prefix + s.value.toFixed(1)).padStart(4, " ");
  }

  /* ---- V25 速度聚合段（数据端 sa 字段，见文件头 V25 变更 a/b）----
   * 口径：avg = 加权均速、max/min = 样本池单笔极值，n = 精确样本笔数。
   * 渲染规则：整数取整（单位沿用该行已有的 t/s 不重复标）；n < 2 时
   * 整段隐藏（单笔样本 均=快=慢 无信息量）；数据缺失/非有限数同样隐藏
   * （旧数据文件无 sa 字段 → 整段不出现，行为与 V24 一致）。 */
  function aggTextOf(sa) {
    if (!sa || !(sa.n >= 2)) return "";
    var vals = [sa.avg, sa.max, sa.min];
    for (var i = 0; i < vals.length; i++) {
      if (typeof vals[i] !== "number" || !isFinite(vals[i])) return "";
    }
    return (
      W.avg + W.sep + (sa.approximate ? "≈" : "") + Math.round(vals[0]) + " " +
      W.fast + W.sep + (sa.approximate ? "≈" : "") + Math.round(vals[1]) + " " +
      W.slow + W.sep + (sa.approximate ? "≈" : "") + Math.round(vals[2])
    );
  }

  /* 模型行与 title 用的全量口径小字（一位小数，不取整；中间点分隔，
   * 与免注入悬浮窗 modelSpeedExtra 同形） */
  function fmtTps(v) {
    return typeof v === "number" && isFinite(v) ? v.toFixed(1) : "–";
  }

  /* V26：跟随小字统一走 WORDS 词表（"· 均 38.4 · 快 71.6 · 慢 12.4" /
   * "· avg 38.4 · fast 71.6 · slow 12.4"），废弃 best/worst 混用 */
  function modelExtraOf(m) {
    return (
      "· " + W.avg + " " + (m.approximate ? "≈" : "") + fmtTps(m.avg) +
      " · " + W.fast + " " + (m.approximate ? "≈" : "") + fmtTps(m.max) +
      " · " + W.slow + " " + (m.approximate ? "≈" : "") + fmtTps(m.min)
    );
  }

  /* V26 模型行 title：固定两行模板——
   *   "GLM-5.3 · 最近 71.6 t/s"
   *   "均 38.4 · 快 71.6 · 慢 12.4 · 5 笔"
   * （en："GLM-5.3 · latest 71.6 t/s" / "avg 38.4 · fast 71.6 ·
   *   slow 12.4 · 5 requests"）。显式 \n 换行：原生 title 按空白自动
   * 折行时会出现 "·" 悬挂行首、语句被裁参差不齐（用户实测），固定
   * 断点后两行均短于折行阈值，不再触发自动换行；V25 尾部的"颜色按
   * 最近值分档"冗余说明删除。name 截断时第一行兜底模型全名 */
  function modelTitleOf(m) {
    return (
      m.model + " · " + W.latest + " " + (m.approximate ? "≈" : "") + fmtTps(m.tps) + " t/s\n" +
      W.avg + " " + (m.approximate ? "≈" : "") + fmtTps(m.avg) +
      " · " + W.fast + " " + (m.approximate ? "≈" : "") + fmtTps(m.max) +
      " · " + W.slow + " " + (m.approximate ? "≈" : "") + fmtTps(m.min) +
      " · " + (m.n || 0) + " " + W.n
    );
  }

  /* 模型行最近值三档变色 class（≥70 绿 / ≥40 黄 / 其余红，与免注入悬浮窗
   * 及主面板速度卡同阈值） */
  function tpsClassOf(tps) {
    if (typeof tps !== "number" || !isFinite(tps)) return "";
    if (tps >= TPS_FAST_MIN) return "zbar-usage-tps-fast";
    if (tps >= TPS_MID_MIN) return "zbar-usage-tps-mid";
    return "zbar-usage-tps-slow";
  }

  /* V27 额度三档配色 class（剩余百分比）：紧张（<10）红、偏低（<30）
   * 黄、充足继承行颜色（返回空 class，灰白不抢视觉）。V28 起消费方为
   * 独立额度行（会话条不再携带额度段） */
  function quotaClassOf(remain) {
    if (typeof remain !== "number" || !isFinite(remain)) return "";
    if (remain < QUOTA_LOW_REMAIN) return "zbar-usage-quota-low";
    if (remain < QUOTA_MID_REMAIN) return "zbar-usage-quota-mid";
    return "";
  }

  /* V31 窗口重置倒计时格式化（输入剩余毫秒差）：纯时长、天/时/分英文
   * 单位中英文 locale 通用（不进词表）——<1h → "35m"；<24h → "2h35m"；
   * ≥24h → "5d2h"。仅由 quotaCountdownOf 消费（rs <= now 的窗口不入段，
   * 不会出现 0 值形态，防御性返回空串） */
  function fmtResetCountdown(msLeft) {
    if (typeof msLeft !== "number" || !isFinite(msLeft) || msLeft <= 0) return "";
    var mins = Math.floor(msLeft / 60000);
    if (mins < 60) return mins + "m";
    var hours = Math.floor(mins / 60);
    if (hours < 24) return hours + "h" + (mins % 60) + "m";
    return Math.floor(hours / 24) + "d" + (hours % 24) + "h";
  }

  /* V31 重置倒计时段（结果进 seg.sub，由 renderQuotaLine 以无 class
   * 独立 span 渲染、恒继承灰白行色——阈值变色只属于剩余百分比，绝不扩
   * 展到倒计时）：rs 缺失/非法/已过期（<= now）返回空串（该窗口不加
   * 倒计时段），有效返回纯时长（不进词表）；V32 起扁平化去括号——
   * 直接返回裸时长紧随百分比，前置半角空格由渲染端补 */
  function quotaCountdownOf(rs, now) {
    if (typeof rs !== "number" || !isFinite(rs) || rs <= now) return "";
    var body = fmtResetCountdown(rs - now);
    if (!body) return "";
    return body;
  }

  /* V27 缓存命中率：cr / (cr + 非缓存输入)，保留一位小数；分母为 0
   * （该会话无任何输入）显示 —。V27–V29 曾作会话条 ⟲ 段后缀，V30–V34
   * 消费方为底部额度行「缓存」段（renderQuotaBar 调用方判 "—" 不入
   * 段）；V35 起三处共用——底部条「缓存」段 + 每轮条/会话条 ⟲ 段后的
   * 弱色 (NN%) 段（调用方判 "—" 不入段），Σ 与 ⟲ 的 token 数值口径
   * 不受影响 */
  function cacheHitText(cr, plainIn) {
    var denom = (cr || 0) + (plainIn || 0);
    if (denom <= 0) return "—";
    return ((cr / denom) * 100).toFixed(1) + "%";
  }

  /* ---- 统一行格式（V10）：三态共用同一固定结构，只更新字段数值 ----
   * "↑ <in> ↓ <out> ⟲ <cr> [(NN%)] [✚ <cw>] · × <req> · <speed> t/s ·
   * TTFT <ttft>"（方括号段按数据条件出现，见下）
   * 各字段等宽补位（等宽字体 + tabular-nums 下整行宽度恒定）：
   * - token 字段经 fmtTokens 恒定 5 字符；
   * - req padStart(3)；
   * - 速度经 padSpeed 恒定占位（null = 尚无确认速度）；
   * - TTFT padStart(4)（"x.xs" 恒 4 字符），null（进行中未定/老库缺
   *   失）显示 "–" 占位；
   * - 速度快照的 request_average 质量由 padSpeed 添加 ≈ 前缀；没有快照
   *   时使用等待占位，计数位始终只来自确认数据。
   * V27：⟲ 缓存读段后补 ✚ 缓存写段（数据端 turns/runs 既有 cw 字段）。
   * V25：v.agg（数据端 sa）存在且样本 ≥ 2 时，速度位之后追加紧凑聚合段
   * "· 均38 快71 慢12"；三处调用点差异——完成态传聚合、live 行与启动
   * 窗口占位不传（进行中轮数据端不产出 sa，见文件头 V25 变更 b）。
   * 启动窗口只显示确认计数的等待占位；live/完成态均只显示数据库
   * 已确认的请求聚合与速度。
   * V33：返回值由拼接串改为字段段数组（{t: 文本, c: 可选 class}）——
   * 标签/单位弱色（CLS_DIM）、速度数值三档色（tpsClassOf，仅确认数值
   * 有效时）；各段文本按序拼接的结果与行串逐字一致（全串比对
   * 机制不变，见 renderRowParts），等宽补位规则原样保留。
   * V35：⟲ 段后追加弱色缓存命中率段 " (NN%)"（cacheHitText 共用：
   * cr / (cr + 非缓存输入)，v.inp 即逐轮 clamp 后的非缓存输入；分母 0
   * ——该轮无任何输入——整段不入行）；✚ 缓存写段改条件段：cw 为 0
   * （启动窗口占位/无缓存写）整段不入行，cw > 0 显示并带词表 title
   * （"缓存读后面的+0 看不懂"实机反馈）。条件段不 push 即不进比对串，
   * 全串比对零 DOM 机制保持；title 不进比对串（见 renderRowParts）。 */
  function barLineOf(v) {
    var agg = v.agg ? aggTextOf(v.agg) : "";
    var spd = v.speed;
    var spdCls =
      spd && typeof spd.value === "number" && isFinite(spd.value) && spd.value > 0
        ? tpsClassOf(spd.value)
        : "";
    var hit = cacheHitText(v.cr, v.inp);
    var parts = [
      { t: "↑ ", c: CLS_DIM },
      { t: fmtTokens(v.inp) },
      { t: " ↓ ", c: CLS_DIM },
      { t: fmtTokens(v.out) },
      { t: " ⟲ ", c: CLS_DIM },
      { t: fmtTokens(v.cr) }
    ];
    if (hit !== "—") {
      parts.push({ t: " (" + hit + ")", c: CLS_DIM });
    }
    if (v.cw > 0) {
      parts.push({ t: " ✚ ", c: CLS_DIM, title: W.cwTitle });
      parts.push({ t: fmtTokens(v.cw), title: W.cwTitle });
    }
    parts.push(
      { t: " · × ", c: CLS_DIM },
      { t: String(v.req).padStart(3, " ") },
      { t: " · ", c: CLS_DIM },
      { t: padSpeed(v.speed), c: spdCls },
      { t: " t/s", c: CLS_DIM }
    );
    if (agg) {
      parts.push({ t: " · " + agg, c: CLS_DIM });
    }
    parts.push({ t: " · TTFT ", c: CLS_DIM });
    parts.push({ t: (v.ttft == null ? "–" : v.ttft).padStart(4, " ") });
    return parts;
  }

  /* V33 字段段数组的比对串：各段文本按序拼接（与渲染后元素的
   * textContent 逐字一致）——每轮条/会话条的「全串比对零 DOM 机制」
   * 以此串为基准，V10 语义不变 */
  function partsTextOf(parts) {
    var s = "";
    for (var i = 0; i < parts.length; i++) s += parts[i].t;
    return s;
  }

  /* V33 每轮条渲染入口：全串比对（partsTextOf 结果 vs 现有
   * textContent）不命中才重建——逐段 span（弱色标签/单位、三档色速度
   * 值）；行元素被重建时 textContent 为空，比对自然不命中 → 自动重渲
   * （「元素即状态」自愈语义不变）。V35：part 支持 title 通道（✚ 缓存
   * 写段口径说明）——title 是属性不是文本，不进比对串，比对零 DOM
   * 机制保持 */
  function renderRowParts(row, parts) {
    var text = partsTextOf(parts);
    if (row.textContent === text) return;
    while (row.firstChild) row.removeChild(row.firstChild);
    for (var i = 0; i < parts.length; i++) {
      var sp = document.createElement("span");
      if (parts[i].c) sp.className = parts[i].c;
      if (parts[i].title) sp.title = parts[i].title;
      sp.textContent = parts[i].t;
      row.appendChild(sp);
    }
  }

  function lineOf(t) {
    return barLineOf({
      inp: Math.max(0, (t["in"] || 0) - (t.cr || 0)),
      out: t.out || 0,
      cr: t.cr || 0,
      cw: t.cw || 0,
      req: t.req || 0,
      speed: t.speed || null,
      agg: t.sa || null,
       ttft: t.ttft != null && t.ttft >= 0 ? fmtSeconds(t.ttft) : null
    });
  }

  /* ---- 进行中轮行：只显示已完成 model_usage 请求的确认聚合；尚未有
   *      请求级速度时保留 "–"，不从 DOM 文本推算 token 或 t/s。
   *      V25：进行中轮不显示聚合段（数据端 runs 行不产出 sa，聚合属于
   *      完成态口径）。V27：✚ 段同口径并入子代理 sub.cw。 ---- */
  function liveLineOf(r) {
    var s = r.sub;
    var inp = (r["in"] || 0) + (s ? s["in"] || 0 : 0);
    var out = (r.out || 0) + (s ? s.out || 0 : 0);
    var cr = (r.cr || 0) + (s ? s.cr || 0 : 0);
    var cw = (r.cw || 0) + (s ? s.cw || 0 : 0);
    var req = (r.req || 0) + (s ? s.req || 0 : 0);
    return barLineOf({
      inp: Math.max(0, inp - cr),
      out: out,
      cr: cr,
      cw: cw,
      req: req,
      speed: r.speed || null,
       ttft: null
    });
  }

  /* ---- 渲染：按 umid（= data-turn-id 值）匹配；同一轮被虚拟列表拆成
   *      多个单元节点时，统计条只渲染在 DOM 顺序最后一个节点（回复结束
   *      处），其余节点旧行移除；幂等（已存在则更新内容） ---- */
  var rendered = new Map(); /* umid → 统计条元素（虚拟列表回收时清） */

  function ensureRow(node, turnId) {
    var row = node.querySelector(":scope > [" + ATTR_ROW + "]");
    if (!row) {
      row = document.createElement("div");
      row.setAttribute(ATTR_ROW, "");
      node.appendChild(row); /* section 内末尾 append */
    }
    rendered.set(turnId, row);
    return row;
  }

  function removeRow(node, turnId) {
    var row = node.querySelector(":scope > [" + ATTR_ROW + "]");
    if (row && row.parentNode) row.parentNode.removeChild(row);
    rendered.delete(turnId);
  }

  function renderOne(node, turnId, isLast, activeMap) {
    var t = index[turnId];
    if (!t) {
      /* V6：进行中 run 命中——已确认聚合 + 请求级速度（只在最后一个
       * 节点渲染）。2 秒竞态窗口内 index 与 runIndex 并存时走上方完成
       * 分支（完成数据优先），此处天然只在"确实未完成"时到达。
       * V9：数字段含数据侧并入的子代理聚合（见 liveLineOf）。V16 起不再有悬浮 title */
      var r = runIndex[turnId];
      if (r) {
        if (!isLast) {
          removeRow(node, turnId);
          return;
        }
        var live = ensureRow(node, turnId);
        /* V33：liveLineOf 返回字段段数组，经 renderRowParts 全串比对后
         * 逐段 span 渲染（弱色标签/三档色速度值），比对机制不变 */
        renderRowParts(live, liveLineOf(r));
        live.setAttribute("data-zbar-usage-row-state", "live");
        return;
      }
      /* 启动窗口活动轮：消息节点已经存在于 DOM，但尚无确认请求时只
       * 渲染等待占位；首笔 model_usage 请求出现后自动切换 live 态。 */
      if (activeMap.get(sessOf(node)) === node) {
        var startRow = ensureRow(node, turnId);
        renderRowParts(startRow, barLineOf({
          inp: 0,
          out: 0,
          cr: 0,
          cw: 0,
          req: 0,
          speed: null,
          ttft: null,
        }));
        startRow.setAttribute("data-zbar-usage-row-state", "live");
        return;
      }
      /* 未命中且非活动轮：不渲染（避免脏数据），清掉可能的残留条 */
      removeRow(node, turnId);
      return;
    }
    if (!isLast) {
      /* 数据命中但本节点不是该轮最后一个节点：清掉本节点上的行
       *（节点集合变化导致"最后一个"迁移时，旧节点上的旧行在此清理） */
      removeRow(node, turnId);
      return;
    }
    var row = ensureRow(node, turnId);
    /* V33：lineOf 返回字段段数组，经 renderRowParts 全串比对后逐段
     * span 渲染（弱色标签/三档色速度值），比对机制不变 */
    renderRowParts(row, lineOf(t));
    row.setAttribute("data-zbar-usage-row-state", "data");
  }

  /* ---- 会话级统计条（V5 引入，V6 进行中确认数据，V8 启动窗口化，V9 子代
   *      理实时化）：fixed 悬浮于对话输入框上方，Σ = 完成轮合计（sess
   *      过滤 turns 原始数组，已含并入的子代理部分）+ 进行中 run 合计
   *      （本会话行含并入的子代理 sub + 未打 m 标记的子代理行按 psess
   *      命中，见 sessionRunTotals），生成期间只显示已确认数字；无确认
   *      数据时显示等待占位。活动轮判定从 DOM 读取，但仅用于确定渲染位置，
   *      不读取回复文字或启动任何速度采样器。独立管线：renderAll 尾部调用
   *      （活动轮判定由 renderAll 统一计算传入，V9 起为多容器 Map），任何
   *      DOM/数据异常都 try 静默，不影响每轮条。
   *      开关经 --zbar-usage-session-bar（variables.css 渲染 1/0，热重
   *      载生效）读取，变量缺失视为开启（默认 true，与 ThemeParams 默
   *      认值一致） ---- */
  var sessionBar = null; /* 已挂载的会话条元素（跨渲染复用，幂等更新） */
  var mountWarned = false; /* 挂载异常一次性告警标志（V14）：此前挂载
    catch 静默吞掉异常（含 V13 的 ReferenceError），兜底态无法被发现 */
  function sessionBarEnabled() {
    try {
      var v = (
        getComputedStyle(document.documentElement).getPropertyValue(
          VAR_SESSION_BAR
        ) || ""
      ).trim();
      return v !== "0";
    } catch (e) {
      return true; /* 读不到变量（旧 variables.css）按默认开启 */
    }
  }

  /* V19：每轮统计条开关（镜像 sessionBarEnabled），renderAll 第二遍渲染
   * 前统一读取（单次 getComputedStyle，避免逐节点重复读） */
  function turnBarEnabled() {
    try {
      var v = (
        getComputedStyle(document.documentElement).getPropertyValue(
          VAR_TURN_BAR
        ) || ""
      ).trim();
      return v !== "0";
    } catch (e) {
      return true; /* 读不到变量（旧 variables.css）按默认开启 */
    }
  }

  /* V17：元素当前是否可见（保活面板隐藏时 rect 归零或 display:none）*/
  function visibleEl(el) {
    try {
      var r = el.getBoundingClientRect();
      var s = window.getComputedStyle(el);
      return r.width > 0 && r.height > 0 &&
        s.display !== "none" && s.visibility !== "hidden";
    } catch (e) {
      return false;
    }
  }

  /* 当前会话 id（V17 重写）：旧实现 querySelector 首中即取，多会话保
   * 活时首中的是任务切换后仍挂载在 DOM 的旧会话容器（通常排在前面），
   * 导致新建任务后条继续渲染旧会话累计、永不消失。改为遍历候选容器
   * 按两级优先取值：① 包含 document.activeElement 的容器（用户正在
   * 输入的会话，含焦点时通常即当前任务）；② 第一个可见容器
   * （visibleEl，保活面板隐藏时 rect 归零或 display:none）。锚点内优
   * 先——子代理详情面板容器与主对话同 document，须先限定主对话锚点
   * 范围；锚点内无候选再退化为全文档。都不满足返回空串（renderSessionBar
   * 既有逻辑随之 removeBar） */
  function currentSessionId(anchor) {
    try {
      var scopes = anchor ? [anchor, document] : [document];
      for (var s = 0; s < scopes.length; s++) {
        var list = scopes[s].querySelectorAll(SEL_SESSION_ID);
        var firstVisible = null;
        for (var i = 0; i < list.length; i++) {
          var el = list[i];
          /* ① 焦点所在容器（用户正在输入的会话）优先即取 */
          try {
            if (document.activeElement &&
              el.contains(document.activeElement)) {
              return el.getAttribute(ATTR_SESSION_ID) || "";
            }
          } catch (e2) {}
          /* ② 记录第一个可见容器，扫完无焦点命中再取 */
          if (firstVisible === null && visibleEl(el)) {
            firstVisible = el.getAttribute(ATTR_SESSION_ID) || "";
          }
        }
        if (firstVisible !== null) return firstVisible;
      }
      return "";
    } catch (e) {
      return "";
    }
  }

  /* 会话累计：V21 起双口径并存——
   * ① 新口径（优先）：数据端 sess 数组的 model_usage 全量合计（含
   *    失败/中断轮——turn_usage 覆盖不全，轮正常结束才落库，按轮聚合
   *    的旧口径会话累计系统性偏小；sess 行为会话树口径，含子代理与
   *    CLI 后台请求）。命中且 tt != null 即采用，调用方跳过 runs 叠加
   *    （runs 行即 model_usage 已落库行的聚合，全量合计已含）；
   * ② 旧口径（回退）：数据无 sess 字段（旧数据文件/查询降级）时按
   *    sess 过滤 turns 原始数组聚合完成轮真实值 + 调用方叠加
   *    sessionRunTotals 的进行中合计（V6/V9 既有行为）。
   * 两套口径返回同构 {tin, tout, tcr, tcw, treq}（↑ 均为逐笔/逐轮 clamp
   * 的非缓存输入；tcw = ✚ 缓存写合计，V27 起两口径同步累计），
   * 渲染端无感切换。
   * V20：turns 含子代理自身视图行（sess 为子代理会话 id、带
   * subagent:1），按 sess 精确匹配天然隔离——主会话视图不命中子代理
   * 自身行（其数值经主轮行的并入值计入），子代理视图（面板锚点外通常
   * 不渲染会话条）只命中自身行，无任何双计路径 */
  function sessionTotals(sessId) {
    /* ① 新口径：sess 全量合计行 */
    var sv = sessIndex[sessId];
    if (sv && sv.tt != null) {
      return {
        tin: sv.up || 0,
        tout: sv.down || 0,
        tcr: sv.cr || 0,
        tcw: sv.cw || 0,
        treq: sv.rq || 0
      };
    }
    /* ② 旧口径回退：该会话无任何完成轮返回 null——V6 起不再据此直接
     * 放弃渲染（V5 根因 a)：新会话首轮生成期间 totals 恒为 null，动态
     * 段从未启动），改由 renderSessionBar 以 run 合计兜底 */
    var tin = 0,
      tout = 0,
      tcr = 0,
      tcw = 0,
      treq = 0,
      found = false;
    for (var i = 0; i < lastTurns.length; i++) {
      var t = lastTurns[i];
      if (!t || t.sess !== sessId) continue;
      found = true;
      /* 与每轮条同口径（保守取小值）逐轮累计 */
      tin += Math.max(0, (t["in"] || 0) - (t.cr || 0));
      tout += t.out || 0;
      tcr += t.cr || 0;
      tcw += t.cw || 0;
      treq += t.req || 0;
    }
    return found ? { tin: tin, tout: tout, tcr: tcr, tcw: tcw, treq: treq } : null;
  }

  /* 进行中 run 合计（V6 引入，V9 口径修正）：本会话行（sess 命中，含
   * 并入的子代理 sub）+ 未打 m 标记的子代理行（psess 命中）。V9 修正：
   * a) 跳过 m:1 的子代理行——其数值已并入父会话主轮行 sub，随主轮行
   *    一并计入（原 psess 裸命中在子代理行与主轮行并存时双计）；
   * b) 主会话行携带 sub（并行子代理 runs + 游离子代理完成轮）时按行
   *    数值同口径合计计入——子代理消耗实时反映在会话累计条上；
   * c) 父会话暂无主轮行（主轮首笔请求未完成）的子代理行不带 m，仍按
   *    psess 在此并入，主轮行出现后自动切换口径，无缝衔接。
   * V27：✚ 缓存写按同口径累计（r.cw 与 r.sub.cw）。无命中返回 null */
  function sessionRunTotals(sessId) {
    var tin = 0,
      tout = 0,
      tcr = 0,
      tcw = 0,
      treq = 0,
      found = false;
    for (var i = 0; i < lastRuns.length; i++) {
      var r = lastRuns[i];
      if (!r || r.m) continue;
      if (r.sess !== sessId && r.psess !== sessId) continue;
      found = true;
      tin += Math.max(0, (r["in"] || 0) - (r.cr || 0));
      tout += r.out || 0;
      tcr += r.cr || 0;
      tcw += r.cw || 0;
      treq += r.req || 0;
      if (r.sub) {
        tin += Math.max(0, (r.sub["in"] || 0) - (r.sub.cr || 0));
        tout += r.sub.out || 0;
        tcr += r.sub.cr || 0;
        tcw += r.sub.cw || 0;
        treq += r.sub.req || 0;
      }
    }
    return found ? { tin: tin, tout: tout, tcr: tcr, tcw: tcw, treq: treq } : null;
  }

  /* 活动轮判定（V8 DOM 驱动，V9 多容器化）：遍历 document 上所有
   * [data-session-id] 容器（主对话 + 子代理详情面板同 document 共存，
   * 各有独立容器，并行多个子代理各有面板），每容器取 DOM 顺序最后一个
   * data-turn-id（umid）节点，umid 不在 index（完成轮）即为该会话的
   * 活动轮。修复 V8 锚点扫描扫不到子代理面板的缺陷（面板在
   * workspace-main 锚点之外）。判定不依赖 runs 数据：既不在 index 也
   * 不在 runIndex = 启动窗口活动轮；在 runIndex = runs 阶段活动轮（同
   * 一节点继续显示已确认聚合），V8 口径不变。返回 Map：会话 id → 活动轮
   * 节点（每轮条启动窗口分支、live 行与会话条共用同一次判定）。
   * 嵌套容器防御：只认属主为本容器的节点（最近 [data-session-id] 祖先
   * 必须是本容器），防把子容器（子代理面板）的轮算进父容器。零容器时
   * 退化为 document 末节点（键取空串，与 sessOf 无容器返回值一致） */
  function findLiveNodes() {
    var map = new Map();
    try {
      var containers = document.querySelectorAll(SEL_SESSION_ID);
      for (var i = 0; i < containers.length; i++) {
        var sess = (containers[i].getAttribute(ATTR_SESSION_ID) || "");
        if (!sess || map.has(sess)) continue;
        var scoped = containers[i].querySelectorAll(SEL_TURN);
        var last = null;
        for (var j = scoped.length - 1; j >= 0; j--) {
          if (scoped[j].closest(SEL_SESSION_ID) === containers[i]) {
            last = scoped[j];
            break;
          }
        }
        if (!last) continue;
        var id = last.getAttribute(ATTR_TURN_ID);
        if (!id || index[id]) continue; /* 已落库 = 已完成 */
        map.set(sess, last);
      }
      if (!containers.length) {
        var nodes = document.querySelectorAll(SEL_TURN);
        if (nodes.length) {
          var lastNode = nodes[nodes.length - 1];
          var lastId = lastNode.getAttribute(ATTR_TURN_ID);
          if (lastId && !index[lastId]) map.set("", lastNode);
        }
      }
    } catch (e) {
      /* 静默，返回已收集部分 */
    }
    return map;
  }

  /* V13：ensureBar 只负责单例创建与断连自愈，挂载位置（region 正常路
   * 径 / body 兜底路径）由 renderSessionBar 依 region 是否存在决定。
   * V25：条容器改为两段式（行 + 模型行容器），两者各自按内容比对重建；
   * V29：模型行容器拆出为独立元素（ensureModelsBox 不再挂在条内），
   * 本容器现仅承载逐段 span 行 */
  function ensureBar() {
    if (sessionBar && sessionBar.isConnected) {
      return sessionBar;
    }
    removeBar(); /* 断连后重建（页面清掉注入层/region 被重建时自愈） */
    var bar = document.createElement("div");
    bar.setAttribute(ATTR_SESSION_BAR, "");
    sessionBar = bar;
    return bar;
  }

  /* V25 行元素（总量 · 速度 · 聚合段）：条的直属子节点，按类名幂等取
   * 用（条被重建时元素随之新建，比对自然不命中 → 重渲；这正是既有的
   * "元素即状态"自愈策略） */
  function ensureBarLine(bar) {
    var line = bar.querySelector(":scope > .zbar-usage-sb-line");
    if (!line) {
      line = document.createElement("div");
      line.className = "zbar-usage-sb-line";
      bar.appendChild(line);
    }
    return line;
  }

  /* ---- V26 动态留白精确计算（修复 V25 模型行压到输入面板）----
   * V25 增量 = 行数 × 固定 14px：字号被调大（行高随 --zbar-usage-font-
   * size 缩放而留白不随）或底板自带内边距/行距/边框增高后，最后一行
   * 即与输入卡片上缘交叠（用户实测截图第三行压面板）。V26 按与 CSS
   * 同源的几何常量逐项求和（见常量区注释），字号读变量实际值 */
  function usageFontSize() {
    try {
      var v = parseFloat(
        (
          getComputedStyle(document.documentElement).getPropertyValue(
            "--zbar-usage-font-size"
          ) || ""
        ).trim()
      );
      return isFinite(v) && v > 0 ? v : 10; /* 与 CSS 兜底 10px 同步 */
    } catch (e) {
      return 10;
    }
  }

  /* 有效模型行数：与 renderModelRows 的过滤条件一致（无 model 的行不
   * 渲染，留白也不该为它预留），避免 V25 按 lastModels.length 高估 */
  function validModelCount() {
    var n = 0;
    for (var i = 0; i < lastModels.length && i < MODEL_ROWS_MAX; i++) {
      if (lastModels[i] && lastModels[i].model) n++;
    }
    return n;
  }

  /* 模型底板高度（行数 × 行高系数 × 字号 + 行距 × (行数-1) + 纵向内
   * 边距 × 2 + 边框 1px × 2），与 ATTR_MODELS 容器 / .zbar-usage-mrow
   * 的 CSS 数值一一同步（见常量区注释块）。rows 恒 > 0（调用方以
   * validModelCount() > 0 为前提） */
  function modelsPlateHeight(rows, fs) {
    return (
      rows * fs * MODEL_ROW_H_FACTOR +
      (rows - 1) * MODEL_ROW_GAP_PX +
      MODELS_PAD_V_PX * 2 +
      2
    );
  }

  /* V29 留白统一写入口：会话条行与模型底板两个成分按各自开关/挂载态
   * 独立叠加（四种组合见常量区注释块），每拍渲染只求一次值——
   * renderAll 主路径在会话条/模型行/额度行渲染完成后调用，空轮分支在
   * 三层注入清干净后调用。region 归属：会话条挂载的 region 优先（会
   * 条是留白的原始宿主，模型行挂载时也跟随它），会话条不在 region 时
   * 用模型底板挂载的 region；两者都不在 region（含兜底 fixed 路径）
   * 时不施加变量，旧 region 的变量经 setRegionPad 迁移逻辑还原。
   * V37 起底部留白带（--zbar-usage-pad-bottom）由 setRegionPadBottom
   * 独立维护（syncQuotaBand 实测行高后写入，照本函数/setRegionPad 同款
   * 惯例）——本函数与 setRegionPad 仍只维护 --zbar-usage-pad-top 变量
   * （region.style.setProperty/removeProperty），不整体重写
   * style.padding，不触碰 padding-bottom */
  function syncComposerPad() {
    var sessIn =
      sessionBarEnabled() &&
      !!sessionBar &&
      sessionBar.isConnected &&
      !!sessionBar.parentElement &&
      !sessionBar.hasAttribute(ATTR_SESSION_BAR_FIXED);
    var modelsIn =
      modelRowsEnabled() &&
      validModelCount() > 0 &&
      !!modelsBox &&
      modelsBox.isConnected &&
      !!modelsBox.parentElement &&
      !modelsBox.hasAttribute(ATTR_MODELS_FIXED);
    var region = null;
    if (sessIn) {
      region = sessionBar.parentElement;
    } else if (modelsIn) {
      region = modelsBox.parentElement;
    }
    var px = COMPOSER_PAD_TOP_PX;
    if (region) {
      var fs = usageFontSize();
      /* 会话条行底缘 = top + 行高（字号 × 1.5，与容器 line-height 同源）；
       * 模型行开时底板需求（含会话行槽位）覆盖该值 */
      var need = SESSION_BAR_TOP_PX + fs * 1.5;
      if (modelsIn) {
        need =
          SESSION_BAR_TOP_PX +
          fs * 1.5 +
          MODELS_MARGIN_TOP_PX +
          modelsPlateHeight(validModelCount(), fs) +
          MODELS_CLEARANCE_PX;
      }
      px = Math.max(COMPOSER_PAD_TOP_PX, Math.round(need));
    }
    setRegionPad(region, px);
  }

  /* V25 动态留白：模型行住进输入区容器顶部留白的增量部分。变量只施加在
   * 当前挂载的 region 上——注入层迁移到另一个 region（多会话保活切换）
   * 或隐藏时，旧 region 的变量必须移除，否则旧输入区永久多出几十像素。
   * px <= 基数时不设变量（回落到 CSS 兜底 26px）。V29 起唯一调用方为
   * syncComposerPad（统一计算，防两条渲染路径互相覆盖） */
  var paddedRegion = null; /* 已施加增量留白的 region（迁移/隐藏时还原） */
  function setRegionPad(region, px) {
    if (paddedRegion && paddedRegion !== region) {
      try {
        paddedRegion.style.removeProperty(VAR_PAD_TOP);
      } catch (e) {}
      paddedRegion = null;
    }
    if (!region) return;
    try {
      if (px > COMPOSER_PAD_TOP_PX) {
        region.style.setProperty(VAR_PAD_TOP, px + "px");
        paddedRegion = region;
      } else {
        region.style.removeProperty(VAR_PAD_TOP);
        paddedRegion = null;
      }
    } catch (e) {
      /* 内联样式不可写（极端只读样式环境）时静默：留白退回 26px，
       * 模型行可能被输入区遮住，但不影响输入区本身可用 */
    }
  }

  /* V40 底部留白带写入：只有统计条实际可见时才给 region 打属性 + 写变
   * 量（CSS 选择器 .chat-composer-region[data-zbar-usage-quota-band] 才注
   * 入 padding-bottom）；注入层迁移 region 时旧 region 的属性/变量必须
   * 一起清——否则旧输入区永久多出一截底垫。条移除/开关关闭/空会话时由
   * clearQuotaBand 显式清除，不再保留"最近一次写入值"（V37 的常驻兜底
   * 会让关闭状态也垫 24px）。与 setRegionPad 相互独立（顶部/底部各一个
   * 变量、各自记忆 region，同一 region 上互不干扰） */
  var paddedBottomRegion = null; /* 已施加底部留白带的 region（迁移时还原） */
  function setRegionPadBottom(region, px) {
    if (paddedBottomRegion && paddedBottomRegion !== region) {
      try {
        paddedBottomRegion.style.removeProperty(VAR_PAD_BOTTOM);
        paddedBottomRegion.removeAttribute(ATTR_QUOTA_BAND);
      } catch (e) {}
      paddedBottomRegion = null;
    }
    if (!region) return;
    try {
      /* 属性先于变量写入：同一拍内浏览器只做一次样式计算，不会先按兜底
       * 27px 垫一下再改为实测值 */
      region.setAttribute(ATTR_QUOTA_BAND, "");
      region.style.setProperty(VAR_PAD_BOTTOM, px + "px");
      paddedBottomRegion = region;
    } catch (e) {
      /* 内联样式不可写时静默：属性仍在、变量缺失 → CSS 兜底 27px，条
       * 可能被多垫一点，但不影响输入区本身可用 */
    }
  }

  /* V40 清除底部留白带（条移除 / 固定兜底路径 / 空会话）：属性与变量
   * 一起清，CSS 选择器不再命中，region 恢复自身 padding-bottom */
  function clearQuotaBand() {
    if (!paddedBottomRegion) return;
    try {
      paddedBottomRegion.style.removeProperty(VAR_PAD_BOTTOM);
      paddedBottomRegion.removeAttribute(ATTR_QUOTA_BAND);
    } catch (e) {}
    paddedBottomRegion = null;
  }

  /* V43 底部留白带动态实测：条渲染后实测 bar.offsetHeight（条高随
   * --zbar-usage-font-size 缩放，14px 档约 32px），带高 = 条高 +
   * QUOTA_BAR_GAP_PX + QUOTA_BAR_BOTTOM_PX（后者为负，条底伸到 region
   * 下缘之外贴近软件底部）——条顶距输入框卡片下缘恒定 GAP。region
   * 缺失（fixed 兜底路径）不施加；条未挂进 region/量不到高度（0）跳
   * 过。每渲染拍重测（字号热重载/分段增减引起的条高变化自然跟随），
   * 写入幂等、开销可忽略（同 setRegionPad 节奏） */
  function syncQuotaBand(bar, region) {
    if (!region || !bar || bar.parentElement !== region) return;
    var h = bar.offsetHeight;
    if (!(h > 0)) return;
    setRegionPadBottom(region, h + QUOTA_BAR_GAP_PX + QUOTA_BAR_BOTTOM_PX);
  }

  function removeBar() {
    if (sessionBar && sessionBar.parentNode) {
      sessionBar.parentNode.removeChild(sessionBar);
    }
    sessionBar = null;
    /* V29：留白不再在此处还原——syncComposerPad 按会话条/模型行两个
     * 注入层的挂载态统一计算（renderAll 尾部/空轮分支调用），避免会
     * 条先清理时把模型行需要的留白一并抹掉 */
  }

  /* 节点所属会话 id（V9）：向上取最近 [data-session-id] 容器属性值；
   * 无容器返回空串（与 findLiveNodes 的零容器兜底键一致） */
  function sessOf(node) {
    try {
      var c = node.closest(SEL_SESSION_ID);
      return (c && c.getAttribute(ATTR_SESSION_ID)) || "";
    } catch (e) {
      return "";
    }
  }

  /* V17：会话条挂载点选择。多会话保活时 document 首个
   * .chat-composer-region 可能属于隐藏的旧会话容器（首中即用会把条挂
   * 错位置），改为两级选择：① 可见且属于当前会话容器（closest 命中
   * 会话容器属性值 === sessId，分栏视图下跟随当前会话）；② 第一个可
   * 见的。都无返回 null，走 renderSessionBar 既有 body 兜底分支 */
  function pickComposerRegion(sessId) {
    try {
      var list = document.querySelectorAll(SEL_COMPOSER);
      var firstVisible = null;
      for (var i = 0; i < list.length; i++) {
        var el = list[i];
        if (!visibleEl(el)) continue;
        if (firstVisible === null) firstVisible = el;
        var owner = el.closest ? el.closest(SEL_SESSION_ID) : null;
        if (sessId && owner &&
          (owner.getAttribute(ATTR_SESSION_ID) || "") === sessId) {
          return el;
        }
      }
      return firstVisible;
    } catch (e) {
      return null;
    }
  }

  /* ---- V35 输入框卡片定位（注入条与输入框卡片左右缘对齐的唯一测量
   *      入口）---- 零选择器假设：不依赖 ZCode 内部类名（选择器仍只有
   * 既有 SEL_* 常量 + 本函数内的通用 HTML 语义锚点 textarea/
   * [contenteditable]），只按"几何特征 + 自家标记排除"推断。
   * V37R 双路径修订（实机复检：底部条深灰背景横贯全屏、输入框卡片左缘
   * 缩进约 48px 未对齐——V35"region 直接子元素首中即用"在两类真实结构
   * 下都会取错：① 输入框卡片不是 region 直接子元素（外层还有满宽的
   * 布局行/居中列容器，首中即用取到满宽容器，条宽被写成 region 全宽）；
   * ② 直接子元素里卡片之前还有别的满宽可见元素（建议条/工具行）。
   * 修复：① 语义锚点优先——region 内的可编辑输入元素（textarea /
   * contenteditable，通用 HTML 语义非 ZCode 类名，任何聊天输入框必有）
   * 自它向上取"最外层非满宽可见祖先"为卡片/列容器（列容器与卡片同宽
   * 同左缘——居中列、缩进列都成立；全链满宽 = 卡片铺满 region 无缩进，
   * 对齐 region 即正确，自然落到 ② 的首中路径）；② V35 原直接子元素
   * 扫描保留为兜底（锚点找不到时维持 V35 既有语义与现状回退） ---- */
  function locateComposerCard(region) {
    if (!region) return null;
    try {
      /* ① 语义锚点路径：可编辑元素 → 最外层非满宽可见祖先 */
      var editable = null;
      try {
        editable = region.querySelector("textarea, [contenteditable=true]");
      } catch (e0) {}
      if (editable) {
        var rw = region.clientWidth;
        var best = null;
        var anc = editable.parentElement;
        while (anc && anc !== region) {
          /* 非满宽判定 rw-4 容忍盒模型取整噪声；可见 + 宽 >100 沿用
           * V35 纪律（跳过装饰性窄元素），持续上溯取离 region 最近的一
           * 个（外层嵌套多个同宽容器时取最外层——与卡片视觉左右缘一致 */
          if (
            visibleEl(anc) &&
            anc.offsetWidth > 100 &&
            anc.offsetWidth < rw - 4
          ) {
            best = anc;
          }
          anc = anc.parentElement;
        }
        if (best) return best;
        /* 全链满宽：不返回锚点路径结果，落 ② 首个合格直接子元素（满
         * 宽场景对齐它即对齐 region，视觉正确） */
      }
      /* ② V35 原路径（兜底）：遍历 region 直接子元素，跳过带
       * data-zbar-usage-* 属性（自家注入元素：每轮条不会出现在这里，
       * 会话条/模型行/底部条会）或绝对定位（装饰层与自家条，positioned
       * 子元素也不是文档流里的卡片）的元素，取第一个可见且
       * offsetWidth > 100 的元素视为"输入框卡片" */
      var kids = region.children;
      for (var i = 0; i < kids.length; i++) {
        var el = kids[i];
        var skip = false;
        try {
          var attrs = el.attributes;
          for (var a = 0; a < attrs.length; a++) {
            if (attrs[a].name.indexOf("data-zbar-usage-") === 0) {
              skip = true;
              break;
            }
          }
        } catch (e1) {}
        if (skip) continue;
        try {
          var pos = getComputedStyle(el).position;
          if (pos === "absolute" || pos === "fixed") continue;
        } catch (e2) {}
        if (!visibleEl(el)) continue;
        if (el.offsetWidth > 100) return el;
      }
    } catch (e) {
      return null;
    }
    return null;
  }

  /* V35 对齐写入口 + V37R 坐标系修订：卡片命中时把条的水平几何改为与
   * 卡片对齐。V35 以 offsetLeft（card.offsetParent === region 时与
   * region 内 absolute left 同一坐标系）直接取值；V37R 锚点路径可命中
   * region 深层元素（offsetParent 是中间定位元素而非 region，
   * offsetLeft 不再同坐标系），改 getBoundingClientRect 相对换算——
   * 行内 left 相对 region 的 padding box（绝对定位包含块），region 计
   * 算边框宽（通常 0）先扣除；卡片水平范围须落在 region 内（±1px 容
   * 差，坐标系一致性防御的等价替代）。withWidth 时再令 width = 卡片
   * 宽，右缘同步对齐（底部条行内 right:auto 显式让位样式表 right:0；
   * 模型底板 box-sizing:border-box，宽度即底板总宽；会话条自然宽不传
   * width）。每渲染拍重算、值变才写 style（沿用文件既有低写入纪律）。
   * 未命中（含几何越界的坐标系防御）返回 false，调用方 clearCardAlign
   * 回退现状。窗口缩放不触发渲染拍：由下一渲染拍/15s 兜底拍自愈修正
   * （同底部条横向滚动/渐隐状态的自愈节奏，不做 resize 监听——V13 起
   * 既有哲学） */
  function applyCardAlign(bar, region, withWidth) {
    var card = locateComposerCard(region);
    if (!card) return false;
    try {
      var cr = card.getBoundingClientRect();
      var rr = region.getBoundingClientRect();
      if (!(cr.width > 100)) return false; /* 复核宽度下限（锚点祖先已过滤，防御） */
      if (cr.left < rr.left - 1 || cr.right > rr.right + 1) return false;
      var bw = parseFloat(getComputedStyle(region).borderLeftWidth) || 0;
      var l = Math.round(cr.left - rr.left - bw) + "px";
      if (bar.style.left !== l) bar.style.left = l;
      /* 会话条/模型行样式表的 left:50%+translateX(-50%) 居中须一并失
       * 效（底部条无 transform，写 none 为无害幂等） */
      if (bar.style.transform !== "none") bar.style.transform = "none";
      if (withWidth) {
        var w = Math.round(cr.width) + "px";
        if (bar.style.width !== w) bar.style.width = w;
        if (bar.style.right !== "auto") bar.style.right = "auto";
      }
    } catch (e) {
      return false;
    }
    return true;
  }

  /* V35 回退清理：清掉行内对齐，样式表规则自动接管——会话条/模型行
   * 回居中、底部条回右对齐自然宽；region 缺失的 fixed 兜底路径亦清理
   * （fixed 贴底定位不受行内 left/width/transform 残留干扰）。逐属性
   * 判空再清，常态（无残留）零 DOM 写 */
  function clearCardAlign(bar) {
    try {
      if (bar.style.left) bar.style.left = "";
      if (bar.style.width) bar.style.width = "";
      if (bar.style.transform) bar.style.transform = "";
      if (bar.style.right) bar.style.right = "";
    } catch (e) {}
  }

  /* V29（P1 修复）：当前会话数据判定——renderSessionBar「无数据即
   * removeBar」条件的同口径提炼（勿另造口径）：当前渲染目标会话
   * （currentSessionId：焦点容器优先，退第一个可见容器）满足以下任一
   * 即有数据——
   * ① sess 全量合计行命中（sv.tt != null，V21 新口径）；
   * ② 该会话存在完成轮（sessionTotals 旧口径归集 turns）；
   * ③ 该会话存在进行中 run/未并入子代理行（sessionRunTotals）；
   * ④ 该会话存在活动轮（activeMap.has，DOM 驱动，覆盖启动窗口）。
   * 全不满足、或无锚点/无会话 id（renderSessionBar 同款 removeBar 分
   * 支）即「当前会话无数据」返回 false。与被取代的全局级判定
   * sessionDataEmpty（turns/runs/sess 任一非空即有数据）的本质区别：
   * 多会话保活下旧会话容器仍挂载 DOM、其数据不再作为当前 draft/新建
   * 任务页的渲染依据——判定只看当前会话自身的四路数据源。会话条开
   * 关不参与判定（开关关闭时模型行/额度行照常显示的 V29 解耦语义不
   * 变）。纯计算无 DOM 写操作（currentSessionId 内部自含 try/catch），
   * renderAll 主路径每拍求值一次后传给 renderModelRows/renderQuotaBar */
  function currentSessionHasData(anchor, activeMap) {
    var sessId = currentSessionId(anchor);
    if (!anchor || !sessId) return false;
    var sv = sessIndex[sessId];
    if (sv && sv.tt != null) return true; /* ① V21 新口径命中 */
    /* ②③④：即 renderSessionBar 的
     * !useSess && !totals && !runTotals && !active → removeBar 取反 */
    return !!(sessionTotals(sessId) || sessionRunTotals(sessId) ||
      activeMap.has(sessId));
  }

  function renderSessionBar(anchor, activeMap) {
    if (!sessionBarEnabled()) {
      /* 会话条独立开关；每轮统计条仍由 turnBarEnabled 单独控制。 */
      removeBar();
      return;
    }
    var sessId = currentSessionId(anchor);
    /* 找不到锚点/会话 id：不渲染（静默降级）。锚点定位天然只在主窗口
     * 出会话条（子代理面板在锚点外，无输入区不渲染会话条） */
    if (!anchor || !sessId) {
      removeBar();
      return;
    }
    /* V21 双口径判定：sess 全量合计命中（新口径）时 runs 不再叠加
     * （runs 行即 model_usage 已落库行的聚合，全量合计已含，叠加即
     * 双计）；未命中（旧数据文件/查询降级）回退旧口径 totals +
     * runTotals 叠加（V6/V9 既有行为）。速度只消费 sess 行快照。 */
    var sv = sessIndex[sessId];
    var useSess = !!(sv && sv.tt != null);
    var totals = sessionTotals(sessId);
    var runTotals = useSess ? null : sessionRunTotals(sessId);
    /* V8：完成合计、run 合计均空且无活动轮（DOM 驱动判定）才放弃——
     * 启动窗口活动轮同样支撑会话条即时显示（新会话首轮发消息即出现，
     * 修复 V6/V7 首笔请求完成前会话条全空白的启动窗口）。draft 空会话
     * 仍不渲染。V9：活动轮按本会话 id 从 activeMap 取。V21：sess 行
     * 命中同样支撑（全量合计非空） */
    var active = activeMap.has(sessId);
    if (!useSess && !totals && !runTotals && !active) {
      removeBar();
      return;
    }
    /* Σ 口径：新数据使用 model_usage 全量会话树合计；旧数据文件回退
     * 完成轮 + 已确认进行中请求。DOM 文本永远不进入这些数字。
     * V27：tcw（✚ 缓存写）两口径同步累计——sess 行 cw / 回退路径
     * totals.tcw + runTotals.tcw（V21 防双计判定不变）。 */
    var tin = useSess ? (sv.up || 0) : (totals ? totals.tin : 0) + (runTotals ? runTotals.tin : 0);
    var tout = useSess
      ? sv.down || 0
      : (totals ? totals.tout : 0) + (runTotals ? runTotals.tout : 0);
    var tcr = useSess
      ? sv.cr || 0
      : (totals ? totals.tcr : 0) + (runTotals ? runTotals.tcr : 0);
    var tcw = useSess
      ? sv.cw || 0
      : (totals ? totals.tcw : 0) + (runTotals ? runTotals.tcw : 0);
    var treq = useSess
      ? sv.rq || 0
      : (totals ? totals.treq : 0) + (runTotals ? runTotals.treq : 0);
    /* V10：Σ 数字段等宽补位（token 经 fmtTokens 恒定 5 字符、req
     * padStart(3)），行宽不随数值位数跳动。V15：Σ 段为会话总 Token
     * （tsum = 输入+输出+缓存读之和，真实数据不含估算），明细段依次
     * 为 ↑ 输入 / ↓ 输出 / ⟲ 缓存读 / × 请求数。
     * V21：parts 改段对象 {t: 文本, c: 可选配色 class}——会话条由单
     * textContent 改为逐段 span 渲染（当时为 CTX 段级变色引入）。
     * V27 曾新增 ✚ 缓存写段——Σ = ↑+↓+⟲ 口径不变（tsum 不含 tcw）。
     * V30：命中率后缀曾挪至底部额度行「缓存 NN%」段（cacheHitText 由
     * renderQuotaBar 复用），⟲ 段一度回归纯 token。
     * V35：⟲ 段升级为 parts 复合段——token 后补弱色缓存命中率段
     * " (NN%)"（cacheHitText(tcr, tin) 同口径：sess 行 tin 即 up 非缓
     * 存输入，与底部条「缓存」段同源同拍；分母 0 该段不入行），实机
     * 反馈回归；✚ 缓存写段改条件段——tcw 为 0 整段不入行，>0 显示并
     * 带词表 title（seg 级 title 通道，见 renderBarLine）。条件段不
     * push 即不进 segsTextOf 比对串，全串比对零 DOM 机制保持。 */
    var tsum = tin + tout + tcr;
    var sessHit = cacheHitText(tcr, tin);
    var crSeg = { parts: [{ t: "⟲ " + fmtTokens(tcr) }] };
    if (sessHit !== "—") {
      crSeg.parts.push({ t: " (" + sessHit + ")", c: CLS_DIM });
    }
    /* V33：Σ 段拆为 parts——Σ 标签继承行色、数值加粗一档
     * （CLS_STRONG，亮度+字重层级不靠字号）；其余明细段结构不变 */
    var segs = [
      { parts: [{ t: "Σ " }, { t: fmtTokens(tsum), c: CLS_STRONG }] },
      { t: "↑ " + fmtTokens(tin) },
      { t: "↓ " + fmtTokens(tout) },
      crSeg
    ];
    if (tcw > 0) {
      segs.push({ t: "✚ " + fmtTokens(tcw), title: W.cwTitle });
    }
    segs.push({ t: "× " + String(treq).padStart(3, " ") });
    /* 速度字段来自会话级 model_usage 请求快照；新一轮尚未出现确认请求
     * 时清掉上一轮的展示值。activeMap 只负责识别当前 DOM 轮，runIndex
     * 告诉我们是否已经有本轮确认请求；它不读取回复文字。request_average
     * 已由 padSpeed 带 ≈ 前缀，绝不把 DOM 文本当 token 或 t/s。 */
    var sessionSpeed = sv && sv.speed ? sv.speed : null;
    if (active) {
      var activeNode = activeMap.get(sessId);
      var activeId = activeNode && activeNode.getAttribute
        ? activeNode.getAttribute(ATTR_TURN_ID)
        : "";
      if (!activeId || !runIndex[activeId] || (sv && sv.speedState === "measuring")) {
        sessionSpeed = null;
      }
    }
    /* V34：速度值补三档色 class（P1 实机反馈：白/灰与每轮条/底部条的
     * 速度三档色不一致）；无效快照/等待占位不着色。可见文本拼接串不
     * 变（padSpeed 等宽补位与 " t/s" 原样），全串比对零 DOM 机制保持 */
    var sessionSpdCls =
      sessionSpeed && typeof sessionSpeed.value === "number" &&
      isFinite(sessionSpeed.value) && sessionSpeed.value > 0
        ? tpsClassOf(sessionSpeed.value)
        : "";
    segs.push({
      parts: [
        { t: padSpeed(sessionSpeed), c: sessionSpdCls },
        { t: " t/s" }
      ]
    });
    /* V25：会话条速度段后追加会话树口径的聚合段（"均38 快71 慢12"，
     * 样本 < 2 隐藏——aggTextOf 返回空串时不入段）。sa 只随 2 秒大文件
     * 发布，1 秒旁路不携带，故它随大文件拍自然刷新。 */
    var aggText = aggTextOf(sv && sv.sa);
    /* V33：聚合段（均/快/慢）弱色（CLS_DIM 次要信息层级） */
    if (aggText) segs.push({ t: aggText, c: CLS_DIM });
    /* V27 曾在此追加额度段（会话条尾部 "5h余/周余"），V28 随额度展示
     * 改版移出——额度行独立渲染（renderQuotaBar，输入框卡片底部外侧），
     * 不再随会话条渲染节奏/开关联动；V21 曾在此追加 "CTX NN%" 上下文
     * 占用段（三档变色，消费 sess 行的 cp 字段），V22 随展示下线删除
     * （数据端 cp/cu/cw 字段一并移除，旧数据文件残留值被忽略；V27 的
     * cw 是缓存写合计，与旧 CTX cw 无关） */
    /* V33：比对串经 segsTextOf 求值——各段（含 Σ 段 parts）文本按序
     * 拼接，与渲染后行 textContent 逐字一致（全串比对零 DOM 机制不
     * 变）；分隔符仍为 " · " */
    var text = segsTextOf(segs);
    var bar = ensureBar();
    /* V13：挂载进输入区容器（幂等迁移）——条 absolute 住进 CSS 注入的
     * 顶部留白，零坐标测量、随文档流自适应。V17：挂载点经
     * pickComposerRegion 选择（可见优先并跟随当前会话容器，多会话保
     * 活时不再首中隐藏旧会话的输入区容器）。region 缺失（选择器失效/
     * 结构变更）退回旧路径：挂 body、打兜底标记切 fixed +
     * SESSION_BAR_BOTTOM_PX（迁回 region 时移除标记自动还原）。
     * V29：留白不再在此处施加——renderAll 尾部 syncComposerPad 按会
     * 话条/模型行两个注入层的挂载态统一计算（本函数只负责条的挂载与
     * 清理） */
    try {
      var region = pickComposerRegion(sessId);
      if (region) {
        if (bar.parentElement !== region) region.appendChild(bar);
        if (bar.hasAttribute(ATTR_SESSION_BAR_FIXED)) {
          bar.removeAttribute(ATTR_SESSION_BAR_FIXED);
        }
        /* V35：水平对齐输入框卡片（左缘 = 卡片左缘、自然宽），未命中
         * 清掉行内对齐回退样式表居中。顶部 padding 定位机制不动 */
        if (!applyCardAlign(bar, region, false)) clearCardAlign(bar);
      } else {
        if (bar.parentElement !== document.body) {
          document.body.appendChild(bar);
        }
        bar.setAttribute(ATTR_SESSION_BAR_FIXED, "");
        /* V35：兜底 fixed 路径清掉行内对齐——left/transform 残留会干
         * 扰 fixed + bottom 兜底定位 */
        clearCardAlign(bar);
      }
    } catch (e) {
      /* 挂载异常：一次性告警（V14 前此处静默吞掉了 ReferenceError，
       * 导致兜底态无法被发现），之后每周期静默重试 */
      if (!mountWarned) {
        mountWarned = true;
        try { console.warn("[ZBar] usage session bar mount error:", e); } catch (e2) {}
      }
    }
    /* V21：内容变化时逐段重建（行元素 textContent 读值含全部段文本，
     * 拼接串比对与旧单串比对等价）；无变化零 DOM 操作。V25：行与模型
     * 行容器分离，行元素只承载总量/速度/聚合段；V29：模型行容器拆出
     * 为独立元素，由 renderAll 独立渲染（会话条开关关闭时模型行照常
     * 显示；当前会话无数据时模型行经 sessHasData 闸随会话条一并隐藏，
     * 见独立容器一节与 currentSessionHasData） */
    renderBarLine(bar, text, segs);
  }

  /* V33 段数组比对串：各段文本按序拼接、" · " 分隔——段可为 {t} 单串
   * 或 {parts:[{t,c}]} 复合段（Σ 段数值加粗），结果与渲染后行
   * textContent 逐字一致（全串比对零 DOM 机制的基准串） */
  function segsTextOf(segs) {
    var text = "";
    for (var si = 0; si < segs.length; si++) {
      if (si) text += " · ";
      text += segs[si].parts ? partsTextOf(segs[si].parts) : segs[si].t;
    }
    return text;
  }

  /* V25 会话条行重建：与旧实现同款"逐段 span + 全串比对"（textContent
   * 读值含全部段文本）；行元素被重建时其 textContent 为空，比对自然不
   * 命中 → 自动重渲（元素即状态的自愈语义不变）。V33：段支持可选
   * parts（复合段内部再拆 span，供 Σ 数值加粗等亮度层级），段级 class
   * 与 parts 级 class 互不干扰。V35：段支持可选 title（✚ 缓存写段口
   * 径说明）——title 是属性不是文本，不进比对串，零 DOM 机制保持 */
  function renderBarLine(bar, text, segs) {
    var line = ensureBarLine(bar);
    if (line.textContent === text) return;
    while (line.firstChild) line.removeChild(line.firstChild);
    for (var sj = 0; sj < segs.length; sj++) {
      if (sj) line.appendChild(document.createTextNode(" · "));
      var sp = document.createElement("span");
      if (segs[sj].c) sp.className = segs[sj].c;
      if (segs[sj].title) sp.title = segs[sj].title;
      if (segs[sj].parts) {
        for (var pj = 0; pj < segs[sj].parts.length; pj++) {
          var pp = document.createElement("span");
          if (segs[sj].parts[pj].c) pp.className = segs[sj].parts[pj].c;
          pp.textContent = segs[sj].parts[pj].t;
          sp.appendChild(pp);
        }
      } else {
        sp.textContent = segs[sj].t;
      }
      line.appendChild(sp);
    }
  }

  /* ---- V29 独立模型行容器（模型分组速度行，最多 3 行）----
   * V25 时容器挂在会话条 bar 内、随会话条开关联动渲染；V29 拆出为独
   * 立元素（ATTR_MODELS）——数据（lastModels）/容器/渲染三重解耦，
   * renderAll 独立调用，会话条开关关闭时模型行照常显示（用户核心诉
   * 求）；当前会话无数据时经 sessHasData 闸随会话条一并隐藏（P1 修
   * 复——多会话保活下新建任务页不再出现孤立模型行，见
   * currentSessionHasData）。开关经 --zbar-usage-model-rows
   * （variables.css 渲染 1/0，缺失视为开启，与 session/turn/quota 开
   * 关同惯例） */
  var modelsBox = null; /* 已挂载的模型行容器（跨渲染复用，幂等更新） */
  var modelsMountWarned = false; /* 挂载异常一次性告警标志（同会话条
    mountWarned 的 V14 模式） */

  function modelRowsEnabled() {
    try {
      var v = (
        getComputedStyle(document.documentElement).getPropertyValue(
          VAR_MODEL_ROWS
        ) || ""
      ).trim();
      return v !== "0";
    } catch (e) {
      return true; /* 读不到变量（旧 variables.css）按默认开启 */
    }
  }

  /* 单例创建与断连自愈（同 ensureBar 的"元素即状态"策略）：挂载位置
   * 由 renderModelRows 依会话条挂载态与 region 是否存在决定 */
  function ensureModelsBox() {
    if (modelsBox && modelsBox.isConnected) {
      return modelsBox;
    }
    removeModelsBox(); /* 断连后重建（页面清掉注入层时自愈） */
    var box = document.createElement("div");
    box.className = "zbar-usage-models";
    box.setAttribute(ATTR_MODELS, "");
    modelsBox = box;
    return box;
  }

  function removeModelsBox() {
    if (modelsBox && modelsBox.parentNode) {
      modelsBox.parentNode.removeChild(modelsBox);
    }
    modelsBox = null;
  }

  /* V25 模型分组速度行渲染（V29 起独立管线；对齐免注入悬浮窗模型速度
   * 区：模型名 truncate + 最近值等宽三档变色 + 单位小字 + 跟随灰字）。
   * V26：行内弹性布局——模型名是唯一可截断段（ellipsis），数值区
   * flex:none 永不截断；聚合小字与 title 统一走 WORDS 词表（均/快/慢
   * ↔ avg/fast/slow），title 为固定两行模板。各节点一律用 textContent
   * 写外部字符串（无 HTML 注入面）；内容比对用行容器的整体签名属性
   * （容器被重建时属性缺失 → 自动重渲），无变化零 DOM 操作。V29 P1
   * 修复：入参 sessHasData 为会话级判定闸（renderAll 主路径经
   * currentSessionHasData 求值，会话条 removeBar 同口径）——当前会话
   * 无数据（多会话保活下的新建任务/draft 页）时移除容器不渲染，切换
   * 回有数据会话后由既有 MutationObserver/兜底渲染拍自愈恢复。开关关
   * 闭或数据端 models 为空（旧数据文件/老库缺列/无样本）时同样移除容
   * 器，不渲染任何行；留白由 renderAll 尾部 syncComposerPad 统一计
   * 算 */
  function renderModelRows(sessHasData) {
    if (!sessHasData || !modelRowsEnabled() || !validModelCount()) {
      removeModelsBox();
      return;
    }
    var box = ensureModelsBox();
    try {
      /* 挂载（幂等迁移，照会话条/额度行既有模式）：会话条已挂进
       * region 时跟随同一容器（两注入层共享同一留白几何，V26 的
       * padForModels 语义在独立容器下延续）；会话条关闭/兜底/未渲染
       * 时取第一个可见 region（模型行是窗口级数据，无会话归属）。
       * region 缺失（选择器失效/结构变更）退回 body + fixed 贴底（标
       * 记属性切换定位，迁回自动还原，同会话条兜底模式） */
      var region =
        sessionBar && sessionBar.isConnected &&
        !sessionBar.hasAttribute(ATTR_SESSION_BAR_FIXED)
          ? sessionBar.parentElement
          : pickComposerRegion("");
      if (region) {
        if (box.parentElement !== region) region.appendChild(box);
        if (box.hasAttribute(ATTR_MODELS_FIXED)) {
          box.removeAttribute(ATTR_MODELS_FIXED);
        }
        /* V35：底板左缘/宽度对齐输入框卡片（V26 的"与会话行同左缘"
         * 锚定升级为与卡片两缘对齐），未命中清掉行内对齐回退居中 */
        if (!applyCardAlign(box, region, true)) clearCardAlign(box);
      } else {
        if (box.parentElement !== document.body) {
          document.body.appendChild(box);
        }
        box.setAttribute(ATTR_MODELS_FIXED, "");
        /* V35：兜底 fixed 路径清掉行内对齐（同会话条兜底分支） */
        clearCardAlign(box);
      }
    } catch (e) {
      /* 挂载异常：一次性告警（同会话条 V14 模式），之后每周期静默重试 */
      if (!modelsMountWarned) {
        modelsMountWarned = true;
        try { console.warn("[ZBar] usage model rows mount error:", e); } catch (e2) {}
      }
    }
    var rows = [];
    var sig = "";
    for (var mi = 0; mi < lastModels.length && mi < MODEL_ROWS_MAX; mi++) {
      var m = lastModels[mi];
      if (!m || !m.model) continue;
      sig += String(m.approximate) + m.model + "\u0000" + m.tps + "\u0000" + m.avg + "\u0000" +
        m.max + "\u0000" + m.min + "\u0000" + m.n + "\u0001";
    }
    if (box.getAttribute("data-zbar-usage-msig") === sig) return;
    while (box.firstChild) box.removeChild(box.firstChild);
    for (var i = 0; i < lastModels.length && i < MODEL_ROWS_MAX; i++) {
      var mm = lastModels[i];
      if (!mm || !mm.model) continue;
      var row = document.createElement("div");
      row.className = "zbar-usage-mrow";
      /* 行 title 同时兜底模型全名（名字截断时）与速度口径 */
      row.title = modelTitleOf(mm);
      var name = document.createElement("span");
      name.className = "zbar-usage-mname";
      name.textContent = mm.model;
      var val = document.createElement("span");
      val.className = "zbar-usage-mval " + tpsClassOf(mm.tps);
      val.textContent = (mm.approximate ? "≈" : "") + fmtTps(mm.tps);
      var unit = document.createElement("span");
      unit.className = "zbar-usage-munit";
      unit.textContent = " t/s";
      var extra = document.createElement("span");
      extra.className = "zbar-usage-mextra";
      extra.textContent = modelExtraOf(mm);
      row.appendChild(name);
      row.appendChild(val);
      row.appendChild(unit);
      row.appendChild(extra);
      rows.push(row);
    }
    for (var ri = 0; ri < rows.length; ri++) box.appendChild(rows[ri]);
    box.setAttribute("data-zbar-usage-msig", sig);
  }

  /* ---- V28/V29 独立额度行（region 底部留白带内，右对齐单行小字）----
   * V27 的额度段曾拼在会话条 segs 尾部；V28 移出为独立一行：额度是账
   * 号级慢变量，不随会话切换/清空变化，也不该被会话条开关连带隐藏。
   * V29 补充空会话隐藏（实机复检后从全局级升级为会话级口径）：
   * renderQuotaBar 新增 sessHasData 入参闸（renderAll 主路径经
   * currentSessionHasData 求值，与会话条 removeBar 同口径）——当前会
   * 话无数据整行隐藏；多会话保活下旧会话有数据不再放行新建任务页
   * （初版全局判定 sessionDataEmpty 在该场景失效——任一旧会话有数据
   * 即放行，实机复现新建任务页孤立额度行，已删除）；整页无 DOM 轮节
   * 点的清理由 renderAll 空轮分支的 removeQuotaBar 承担。该页没有会
   * 话条承载视觉上下文，孤立额度行压着输入框下边框（实机反馈），与
   * 「会话条/每轮条无数据不显示」的自然逻辑对齐，第一条消息发出后恢
   * 复。挂载照会话条 V13
 * 模式——absolute 挂进 .chat-composer-region（复用
 * pickComposerRegion，额度无会话归属、传空 id 即取第一个可见容
 * 器），bottom 偏移（QUOTA_BAR_BOTTOM_PX，V37 起带内 bottom 5px =
 * QUOTA_BAR_GAP_PX）住进 region padding-bottom 预留的留白带（带高
 * V37R 起由 syncQuotaBand 实测行高动态写入），行本身不占布局（留白
 * 带由 region 的 padding-bottom 参与布局）、不与顶部留白计算联动；region
 * 缺失回退挂 body +
 * fixed 贴底右侧（标记属性切换定位，迁回自动还原，同会话条兜底模
 * 式）。渲染节拍：renderAll 主路径每拍调用——额度数据 onload 与开
 * 关热重载都直接生效，不依赖会话条渲染节奏。开关经
 * --zbar-usage-quota-bar（variables.css 渲染 1/0，缺失视为开启，与
 * session/turn/model 开关同惯例）。页面隐藏降频 60s 的额度重载节拍
 * （QUOTA_POLL_HIDDEN_MS）不变。V30：新增 anchor 入参（renderAll 主
 * 路径把锚点一并传入）——周段之后追加当前会话缓存命中率「缓存
 * NN%」段（renderSessionBar 同款双口径自算，不依赖会话条；分母 0
 * 不入段），额度行不再只承载账号级额度。V31：5h/周段追加重置倒计
 * 时段（seg.sub → quotaCountdownOf，无 class 独立 span 灰白继承
 * 行色；rs 缺失/已过期不加段），版本闸放行 v:1/v:2。V32：扁平化去
 * 括号，行形 "额度 5h 65% 3h02m · 每周 93% 6d21h · 缓存 88.7%"
 * （en "Quota 5h 65% 3h02m · wk 93% 6d21h · cache 88.7%"）。
 * V33：演进为分段胶囊状态条（完整分段清单/取值链路/签名比对机制见
 * 文件头 V33 变更 b/c）——新增 activeMap 入参（renderAll 主路径与
 * sessHasData/anchor 同源传入，供「速度」「本轮」分段取活动轮
 * runs 行）；额度/缓存为其中尾部两段（V32 文本与阈变色逻辑原样），
 * 开关 usage_quota_bar 语义升级为整条开关。V35：水平几何改为与输入
 * 框卡片对齐（locateComposerCard/applyCardAlign，两缘精确对齐——右
 * 对齐自然宽的遗留左缘悬空问题），未命中回退右对齐自然宽；每渲染拍
 * 重算、值变才写 style，窗口缩放由下一渲染拍/兜底拍自愈（不做
 * resize 监听）。V37R：对齐取值改 rect 相对换算 + 锚点路径（卡片可
 * 为 region 深层元素，见 locateComposerCard 注释），底部留白带改
 * syncQuotaBand 动态实测（见上）---- */
  var quotaBar = null; /* 已挂载的额度行元素（跨渲染复用，幂等更新） */
  var quotaMountWarned = false; /* 挂载异常一次性告警标志（同会话条
    mountWarned 的 V14 模式：此前类问题曾被 catch 静默吞掉无从发现） */

  function quotaBarEnabled() {
    try {
      var v = (
        getComputedStyle(document.documentElement).getPropertyValue(
          VAR_QUOTA_BAR
        ) || ""
      ).trim();
      return v !== "0";
    } catch (e) {
      return true; /* 读不到变量（旧 variables.css）按默认开启 */
    }
  }

  /* V29 初版的全局级空判定 sessionDataEmpty（turns/runs/sess 任一非
   * 空即有数据）已删除：多会话保活下旧会话容器仍挂载 DOM、任一旧会话
   * 有数据即放行，新建任务页仍出现孤立额度行（实机复现）。空会话隐
   * 藏由会话级判定 currentSessionHasData（renderSessionBar removeBar
   * 同口径）经 renderAll 求值后以 sessHasData 入参承担，见上方段注释 */

  /* 单例创建与断连自愈（同 ensureBar 的"元素即状态"策略）：挂载位置
   * （region 正常路径 / body 兜底路径）由 renderQuotaBar 依 region 是
   * 否存在决定 */
  function ensureQuotaBar() {
    if (quotaBar && quotaBar.isConnected) {
      return quotaBar;
    }
    removeQuotaBar(); /* 断连后重建（页面清掉注入层时自愈） */
    var bar = document.createElement("div");
    bar.setAttribute(ATTR_QUOTA_BAR, "");
    quotaBar = bar;
    return bar;
  }

  function removeQuotaBar() {
    if (quotaBar && quotaBar.parentNode) {
      quotaBar.parentNode.removeChild(quotaBar);
    }
    quotaBar = null;
    /* V40：条不可见 → 底部留白带一并撤掉（关闭开关/空会话不再垫底） */
    clearQuotaBand();
  }

  /* ---- V33 底部状态条取值链路（数据全部来自既有全局状态，沿用既有
   *      通道，不新增轮询/不读 DOM 文本）---- */

  /* 活动轮 runs 行（当前会话）：activeMap 命中的活动轮 umid → runIndex
   * 行（主会话行含并入的 sub，V9 口径）。无活动轮/未进 runs 返回 null */
  function activeRunOf(sessId, activeMap) {
    if (!sessId || !activeMap) return null;
    var node = activeMap.get(sessId);
    var umid = node && node.getAttribute ? node.getAttribute(ATTR_TURN_ID) : "";
    return umid ? runIndex[umid] || null : null;
  }

  /* 该会话最新完成轮（按 end/start 取最大；turns 含子代理自身视图行，
   * 按 sess 精确匹配天然隔离——V20 口径）。无完成轮返回 null */
  function latestTurnOf(sessId) {
    var best = null;
    var bestEnd = -1;
    for (var i = 0; i < lastTurns.length; i++) {
      var t = lastTurns[i];
      if (!t || t.sess !== sessId) continue;
      var end = typeof t.end === "number" ? t.end : t.start || 0;
      if (end >= bestEnd) {
        bestEnd = end;
        best = t;
      }
    }
    return best;
  }

  /* 最近生成速度（「速度」分段取值链路）：① 当前会话活动轮 runs 行速
   * 度优先（1s 旁路 applySpeedData 每秒覆盖 lastRuns，生成期实时值）；
   * ② 空闲回退该会话 turns 最新完成轮稳定速度（turns 行速度 = 该轮最
   * 新请求快照，轮完成后不再变化，大文件与 1s 旁路双通道覆盖）。仅返
   * 回数值有效的快照——无效/缺失返回 null，「速度」分段整段省略 */
  function latestSpeedOf(sessId, activeMap) {
    function valid(s) {
      return (
        !!s && typeof s.value === "number" && isFinite(s.value) && s.value > 0
      );
    }
    var r = activeRunOf(sessId, activeMap);
    if (r && valid(r.speed)) return r.speed;
    var t = sessId ? latestTurnOf(sessId) : null;
    if (t && valid(t.speed)) return t.speed;
    return null;
  }

  /* ---- V33 分段行内 SVG 图标（feather 风格 24 viewBox 线性图标，全
   *      部以 path 表达；stroke=currentColor 随文字颜色、不单独上色，
   *      线宽/圆帽由 .zbar-usage-qico 统一）。经 createElementNS 构建
   *      ——不拼接 HTML 字符串建节点，与全文件 DOM 写入纪律一致 ---- */
  var ICON_PATHS = {
    bolt: ["M13 2 3 14h9l-1 8 10-12h-9l1-8z"],
    hourglass: [
      "M5 22h14",
      "M5 2h14",
      "M7 2v4.2a2 2 0 0 0 .6 1.4L12 12l4.4-4.4a2 2 0 0 0 .6-1.4V2",
      "M7 22v-4.2a2 2 0 0 1 .6-1.4L12 12l4.4 4.4a2 2 0 0 1 .6 1.4V22"
    ],
    bubble: [
      "M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"
    ],
    wrench: [
      "M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"
    ],
    calendar: [
      "M8 2v4",
      "M16 2v4",
      "M3 10h18",
      "M5 4h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z"
    ],
    branch: [
      "M6 3v12",
      "M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
      "M6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
      "M18 9a9 9 0 0 1-9 9"
    ],
    /* V46 额度两段去文字标签后补标识图标（feather 同款纯 path）：
     * battery=剩余电量隐喻配 5h 窗口、repeat=周期重置隐喻配每周窗口 */
    battery: [
      "M3 6h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z",
      "M23 13v-2"
    ],
    repeat: [
      "M17 1l4 4-4 4",
      "M3 11V9a4 4 0 0 1 4-4h14",
      "M7 23l-4-4 4-4",
      "M21 13v2a4 4 0 0 1-4 4H3"
    ],
    gauge: ["M4.9 19a9 9 0 1 1 14.2 0", "M12 13l3.5-3.5"]
  };

  function makeIcon(name) {
    var ds = ICON_PATHS[name];
    if (!ds || !document.createElementNS) return null;
    var svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("class", "zbar-usage-qico");
    svg.setAttribute("aria-hidden", "true");
    for (var i = 0; i < ds.length; i++) {
      var p = document.createElementNS("http://www.w3.org/2000/svg", "path");
      p.setAttribute("d", ds[i]);
      svg.appendChild(p);
    }
    return svg;
  }

  function renderQuotaBar(sessHasData, anchor, activeMap) {
    if (!sessHasData || !quotaBarEnabled()) {
      /* 当前会话无数据（会话级判定闸，见 currentSessionHasData——多
       * 会话保活下的新建任务/draft 页同样命中，V29 P1 修复），或开关
       * 关闭（变量缺失视为开启，与 session/turn 开关同惯例）：移除整
       * 条状态条 */
      removeQuotaBar();
      return;
    }
    /* V31 版本闸放行 v:1/v:2：升级过渡期本地残留 v:1 文件（无 h5rs/wkrs）
     * 照常显示、无倒计时段，额度线程 60s 内写出 v:2 自然生效 */
    var qd =
      quotaData && (quotaData.v === 1 || quotaData.v === 2) ? quotaData : null;
    /* 双口径（renderSessionBar 同款）：sess 全量合计行命中即消费 sess
     * 行，否则 totals + runTotals 回退叠加——会话 Σ 段与缓存命中率段
     * 共用同一组口径值，与 sessHasData 同源同拍。 */
    var sessId = currentSessionId(anchor);
    var sv = sessId ? sessIndex[sessId] : null;
    var useSess = !!(sv && sv.tt != null);
    var totals = sessId ? sessionTotals(sessId) : null;
    var runTotals = useSess ? null : (sessId ? sessionRunTotals(sessId) : null);
    var tcr = useSess
      ? sv.cr || 0
      : (totals ? totals.tcr : 0) + (runTotals ? runTotals.tcr : 0);
    var tin = useSess ? (sv.up || 0) : (totals ? totals.tin : 0) + (runTotals ? runTotals.tin : 0);
    /* V33 分段容器：{ico: 图标, title: 原生提示, c: 段级配色 class,
     * dot: 呼吸蓝点, parts: [{t: 文本, c: 段内层级 class}]}——缺数据的
     * 分段整段省略（push 侧过滤；条宽不足由 V44 横向滚动承担，不再隐
     * 藏分段），发丝分隔线由 CSS 相邻兄弟选择器承担、无空分隔残留；
     * 段内间距由 flex gap 承担，文本不带语义间距空格（P2-5） */
    var segs = [];
    var nowMs = Date.now();
    /* ① ⚡ 速度：最近生成速度（latestSpeedOf：活动轮 runs 行 1s 旁路
     * 实时值优先，空闲回退该会话最新完成轮稳定速度）；数值三档色 +
     * "t/s" 弱色单位，request_average 的 ≈ 前缀保留（近似质量不丢） */
    var spd = latestSpeedOf(sessId, activeMap);
    if (spd) {
      segs.push({
        ico: "bolt",
        title: W.speedTitle,
        parts: [
          {
            t: (spd.quality === "request_average" ? "≈" : "") + spd.value.toFixed(1),
            c: tpsClassOf(spd.value)
          },
          { t: "t/s", c: "zbar-usage-qdim" }
        ]
      });
    }
    /* ② 本轮：↑非缓存输入+↓输出合计（fmtTokensShort，去等宽补位）+
     * "N req"——活动轮 runs 行（含并入 sub，liveLineOf 同口径）优先，
     * 无则该会话最新完成 turn 行（turns 行数值已并入子代理聚合）；
     * title 带输入/输出/缓存读/缓存写明细（原始数值非缩写） */
    var tr = activeRunOf(sessId, activeMap);
    var tt = tr ? null : (sessId ? latestTurnOf(sessId) : null);
    if (tr || tt) {
      var tIn2, tOut2, tCr2, tCw2, tReq2;
      if (tr) {
        var sb = tr.sub;
        var trIn = (tr["in"] || 0) + (sb ? sb["in"] || 0 : 0);
        var trCr = (tr.cr || 0) + (sb ? sb.cr || 0 : 0);
        tIn2 = Math.max(0, trIn - trCr);
        tOut2 = (tr.out || 0) + (sb ? sb.out || 0 : 0);
        tCr2 = trCr;
        tCw2 = (tr.cw || 0) + (sb ? sb.cw || 0 : 0);
        tReq2 = (tr.req || 0) + (sb ? sb.req || 0 : 0);
      } else {
        tIn2 = Math.max(0, (tt["in"] || 0) - (tt.cr || 0));
        tOut2 = tt.out || 0;
        tCr2 = tt.cr || 0;
        tCw2 = tt.cw || 0;
        tReq2 = tt.req || 0;
      }
      segs.push({
        ico: "hourglass",
        title:
          W.turn + " · " + W.tIn + " " + tIn2 + " · " + W.tOut + " " + tOut2 +
          " · " + W.tCr + " " + tCr2 + " · " + W.tCw + " " + tCw2 +
          " · " + tReq2 + " " + W.n,
        parts: [
          { t: fmtTokensShort(tIn2 + tOut2), c: "zbar-usage-qval" },
          { t: tReq2 + " " + W.req, c: "zbar-usage-qdim" }
        ]
      });
    }
    /* ③ 会话 Σ：当前会话 tt（sess 行 tt 或 totals+runTotals 同口径合
     * 计 = ↑+↓+⟲，即会话条 Σ 口径）+ "N req"（rq/rreq 同口径） */
    if (sessId) {
      var sTt = useSess
        ? sv.tt || 0
        : (totals ? totals.tin + totals.tout + totals.tcr : 0) +
          (runTotals ? runTotals.tin + runTotals.tout + runTotals.tcr : 0);
      var sRq = useSess
        ? sv.rq || 0
        : (totals ? totals.treq : 0) + (runTotals ? runTotals.treq : 0);
      segs.push({
        ico: "bubble",
        title: W.sessTitle,
        parts: [
          { t: fmtTokensShort(sTt), c: "zbar-usage-qval" },
          { t: sRq + " " + W.req, c: "zbar-usage-qdim" }
        ]
      });
    }
    /* ④ 工具：sess[].tl（V33 新消费字段，会话树工具调用数；字段缺失
     * ——旧数据文件/查询降级——整段隐藏） */
    if (sv && typeof sv.tl === "number" && isFinite(sv.tl)) {
      segs.push({
        ico: "wrench",
        title: W.toolsTitle,
        parts: [{ t: String(sv.tl), c: "zbar-usage-qval" }]
      });
    }
    /* ⑤ 今日：顶层 today.tt + "N req"（today.rq；V33 新消费字段，本
     * 地自然日全库合计——整键缺失/非法隐藏本段） */
    if (lastToday && typeof lastToday.tt === "number" && isFinite(lastToday.tt)) {
      segs.push({
        ico: "calendar",
        title: W.todayTitle,
        parts: [
          { t: fmtTokensShort(lastToday.tt), c: "zbar-usage-qval" },
          { t: (lastToday.rq || 0) + " " + W.req, c: "zbar-usage-qdim" }
        ]
      });
    }
    /* ⑥ 子代理：Σ 当前会话 turns[].sub.n（并入主轮的子代理轮数）；
     * 任一 runs 行 psess 命中当前会话 → 图标旁呼吸蓝点（子代理运行中
     * 信号，轻量 opacity 脉动）；无子代理隐藏本段 */
    if (sessId) {
      var subN = 0;
      for (var si = 0; si < lastTurns.length; si++) {
        var st = lastTurns[si];
        if (st && st.sess === sessId && st.sub && st.sub.n) subN += st.sub.n;
      }
      if (subN > 0) {
        var subRunning = false;
        for (var ri = 0; ri < lastRuns.length; ri++) {
          var rr = lastRuns[ri];
          if (rr && rr.psess === sessId) {
            subRunning = true;
            break;
          }
        }
        segs.push({
          ico: "branch",
          title: W.subsTitle,
          dot: subRunning,
          parts: [{ t: String(subN), c: "zbar-usage-qval" }]
        });
      }
    }
    /* ⑦ 额度（V32 原样迁入 + V34 补 title + V46 补图标去"额度"前导，
     * 口径说明由 title 承担）：h5/wk 缺失只隐对应段；三档变色挂段级
     * class（主数值部分继承变色）；倒计时段由 quotaCountdownOf 生成
     * （rs 缺失/非法/已过期 → 无倒计时段），qfaint 显式回灰——阈变色
     * 仍只属于剩余百分比（V31 界线不变） */
    if (qd && typeof qd.h5 === "number" && isFinite(qd.h5)) {
      var h5cd = quotaCountdownOf(qd.h5rs, nowMs);
      var h5Parts = [
        { t: W.quota5h + Math.round(qd.h5) + "%", c: quotaClassOf(qd.h5) }
      ];
      if (h5cd) h5Parts.push({ t: h5cd, c: "zbar-usage-qfaint" });
      segs.push({ ico: "battery", title: W.quota5hTitle, parts: h5Parts });
    }
    if (qd && typeof qd.wk === "number" && isFinite(qd.wk)) {
      var wkcd = quotaCountdownOf(qd.wkrs, nowMs);
      var wkParts = [{ t: Math.round(qd.wk) + "%", c: quotaClassOf(qd.wk) }];
      if (wkcd) wkParts.push({ t: wkcd, c: "zbar-usage-qfaint" });
      segs.push({ ico: "repeat", title: W.quotaWkTitle, parts: wkParts });
    }
    /* ⑧ 缓存（V30 逻辑原样迁入 + V33 仪表图标 + V34 title + V46 去文字
     * 标签，口径说明由 title 承担）：当前会话命中率 cr / (cr + 非缓存
     * 输入)，分母 0（cacheHitText 返回 "—"）不入段不留残留 */
    var hit = cacheHitText(tcr, tin);
    if (hit !== "—") {
      segs.push({
        ico: "gauge",
        title: W.cacheTitle,
        parts: [{ t: hit, c: "" }]
      });
    }
    if (!segs.length) {
      removeQuotaBar();
      return;
    }
    var bar = ensureQuotaBar();
    /* V37R：挂载成功的 region 提升到外层——渲染尾部 syncQuotaBand 要
     * 用它实测行高、写底部留白带（fixed 兜底路径 region 为 null，不施
     * 加，见 syncQuotaBand） */
    var quotaRegion = null;
    try {
      /* 挂载（V28 原样）：额度/今日无会话归属——pickComposerRegion 传
       * 空 id 即取第一个可见容器（多会话保活下同样成立）。region 缺失
       * （选择器失效/结构变更）退回旧路径：挂 body、打兜底标记切
       * fixed 贴底右侧 */
      var region = pickComposerRegion("");
      if (region) {
        quotaRegion = region;
        if (bar.parentElement !== region) region.appendChild(bar);
        if (bar.hasAttribute(ATTR_QUOTA_BAR_FIXED)) {
          bar.removeAttribute(ATTR_QUOTA_BAR_FIXED);
        }
        /* V41：恢复按输入框卡片对齐——left = card.offsetLeft、width =
         * card.offsetWidth，条与对话框严格同宽同起点；未命中回退整条
         * 占满 region（left/right:0），条宽不足由 V44 横向滚动适配
         * （qb-line 即滚动容器，宽度变化随下一渲染拍自愈） */
        if (!applyCardAlign(bar, region, true)) clearCardAlign(bar);
      } else {
        if (bar.parentElement !== document.body) {
          document.body.appendChild(bar);
        }
        bar.setAttribute(ATTR_QUOTA_BAR_FIXED, "");
        /* V40：兜底 fixed 路径清掉行内对齐与残留底部留白带（fixed 居中
         * 贴底不受旧 left/width 干扰） */
        clearCardAlign(bar);
        clearQuotaBand();
      }
    } catch (e) {
      /* 挂载异常：一次性告警（同会话条 V14 模式），之后每周期静默重试 */
      if (!quotaMountWarned) {
        quotaMountWarned = true;
        try { console.warn("[ZBar] usage quota bar mount error:", e); } catch (e2) {}
      }
    }
    /* V33 签名比对（renderModelRows 同款「元素即状态」策略）：分段携
     * 带行内 SVG 图标无法进 textContent 比对，改以全部分段数据（段级
     * class/图标/标题/蓝点/各段文本）拼接签名存于容器
     * data-zbar-usage-qsig——无变化零 DOM 操作，倒计时分钟级变化之间
     * 仍零开销；行/容器被重建时属性缺失 → 自动全量重渲（自愈语义不
     * 变）。V44：签名经 quotaSigOf 统一构建（隐藏机制退役后不含隐藏
     * 位），渲染后由 syncQuotaFade 同步左右缘渐隐状态 class */
    renderQuotaLine(bar, quotaSigOf(segs), segs);
    syncQuotaFade(bar);
    /* V37R：渲染完成后实测行高、同步底部留白带（fixed 兜底路径
     * quotaRegion 为 null 不施加） */
    syncQuotaBand(bar, quotaRegion);
  }

  /* V33 底部状态条渲染：签名比对（renderModelRows 同款「元素即状态」
   * 策略）——分段携带行内 SVG 图标无法进 textContent 比对，改以全部
   * 分段数据（class/图标/标题/蓝点/各段文本）拼接的签名字符串存于容
   * 器 data-zbar-usage-qsig 属性，无变化零 DOM 操作；行/容器被重建时
   * 属性缺失 → 自动全量重渲（自愈语义不变）。段结构：
   * span.zbar-usage-qseg（可选 seg.c 段级配色 class + 原生 title 提示
   * + 前置 SVG 图标 + 内部 parts 层级 span + 可选呼吸蓝点），发丝分
   * 隔线由 CSS 相邻兄弟 ::before 承担（分段整段省略无残留）；各节点
   * 一律 createElement/createElementNS + textContent 写入（与全文件
   * 纪律一致：不拼接 HTML 字符串建节点）。V31 的「倒计时独立无 class
   * span」语义由 parts 内 qfaint class 显式回灰等价承载（阈变色仍只
   * 属于剩余百分比） */
  function renderQuotaLine(bar, sig, segs) {
    var line = bar.querySelector(":scope > .zbar-usage-qb-line");
    if (
      line &&
      line.querySelector(":scope > .zbar-usage-qfull") &&
      bar.getAttribute("data-zbar-usage-qsig") === sig
    ) {
      return;
    }
    if (!line) {
      line = document.createElement("div");
      line.className = "zbar-usage-qb-line";
      /* V44 滚动交互监听随行元素创建挂载一次（wheel 非 passive 才能
       * preventDefault，scroll 同步渐隐 class）：行元素与条同生共死
       * （removeQuotaBar 卸条时监听随元素一并释放，无全局引用可泄
       * 漏），断连重建时走本分支自然重挂，不会重复挂载 */
      line.addEventListener("wheel", onQuotaWheel, { passive: false });
      line.addEventListener("scroll", onQuotaFadeScroll, false);
      bar.appendChild(line);
    }
    while (line.firstChild) line.removeChild(line.firstChild);
    /* V41 完整分段容器：条与输入框卡片同宽；不溢出时 margin:0 auto 视
     * 觉居中，溢出时锚左交给 qb-line 横向滚动（V44 滚动适配） */
    var full = document.createElement("span");
    full.className = "zbar-usage-qfull";
    for (var qj = 0; qj < segs.length; qj++) {
      var seg = segs[qj];
      var sp = document.createElement("span");
      sp.className = "zbar-usage-qseg" + (seg.c ? " " + seg.c : "");
      if (seg.title) sp.title = seg.title;
      var ico = seg.ico ? makeIcon(seg.ico) : null;
      if (ico) sp.appendChild(ico);
      for (var pj = 0; pj < seg.parts.length; pj++) {
        var pp = document.createElement("span");
        if (seg.parts[pj].c) pp.className = seg.parts[pj].c;
        pp.textContent = seg.parts[pj].t;
        sp.appendChild(pp);
      }
      if (seg.dot) {
        var sdot = document.createElement("span");
        sdot.className = "zbar-usage-qdot";
        sp.appendChild(sdot);
      }
      full.appendChild(sp);
    }
    line.appendChild(full);
    bar.setAttribute("data-zbar-usage-qsig", sig);
  }

  /* V33 签名构建（renderQuotaBar 渲染前统一拼装）：全部分段渲染语义
   * 位（段级 class/图标/标题/蓝点/各段文本）计入签名——内容不变签名
   * 不变、零 DOM；V44 起不再含隐藏位（优先级隐藏机制整体退役，滚动
   * 态/渐隐态属滚动容器自身状态、不进内容签名） */
  function quotaSigOf(segs) {
    var sig = "";
    for (var qi = 0; qi < segs.length; qi++) {
      sig +=
        (segs[qi].c || "") + "\u0000" + (segs[qi].ico || "") + "\u0000" +
        (segs[qi].title || "") + "\u0000" + (segs[qi].dot ? "1" : "0") +
        "\u0000";
      for (var qpi = 0; qpi < segs[qi].parts.length; qpi++) {
        sig += "<" + (segs[qi].parts[qpi].c || "") + "\u0001" + segs[qi].parts[qpi].t;
      }
    }
    return sig;
  }

  /* ---- V44 溢出适配：横向滚动 + 滚轮（取代 V34 优先级整段隐藏）----
   * 动机（实机反馈）：分段串长随对话天然变化（工具/子代理/缓存段条件
   * 性出现、token 数字随会话增长变宽），且右侧计划列表/子代理面板/浏
   * 览器等侧面板会压缩对话区宽度——整段隐藏导致今日/缓存段频繁消失，
   * 缓存段在临界宽度下随数据每拍隐藏↔恢复闪烁。滚动适配内容恒全量在
   * 场、无临界翻转，闪烁随隐藏机制退役根治。宽度变化不触发渲染拍：
   * 渐隐/居中随下一渲染拍自愈（同 applyCardAlign 节奏，不做 resize
   * 监听）。 */

  /* 滚轮横向滚动：触控板 deltaX（天然横滚）优先，普通滚轮取 deltaY
   * 转横向；仅存在横向溢出且未滚到对应边界时 preventDefault（监听非
   * passive 的意义所在）——滚到头或本无溢出即放行，不吞页面纵向滚
   * 动。deltaMode 不归一化：Chromium 鼠标/触控板均像素模式 */
  function onQuotaWheel(e) {
    var line = e.currentTarget;
    var max = line.scrollWidth - line.clientWidth;
    if (!(max > 0)) return;
    var d = Math.abs(e.deltaX) >= Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    if (!d) return;
    var to = Math.max(0, Math.min(max, line.scrollLeft + d));
    if (to === line.scrollLeft) return;
    e.preventDefault();
    line.scrollLeft = to;
  }

  /* scroll 事件入口（监听挂在 qb-line 上，经 currentTarget 取行） */
  function onQuotaFadeScroll(e) {
    syncQuotaFadeLine(e.currentTarget);
  }

  /* 左右缘渐隐状态同步（每次渲染拍后 + 每次 scroll）：无溢出双侧全
   * 无；有溢出时未滚到左缘亮左渐隐、未滚到右缘亮右渐隐（1px 容差防
   * 亚像素悬崖）。class 全量重建、值变才写（沿用文件低写入纪律）；
   * 内容重渲后 scrollLeft 由浏览器自动收敛到合法区间，本函数随后把
   * 渐隐态收敛到一致 */
  function syncQuotaFade(bar) {
    var line = bar.querySelector(":scope > .zbar-usage-qb-line");
    if (!line) return;
    syncQuotaFadeLine(line);
  }

  function syncQuotaFadeLine(line) {
    var max = line.scrollWidth - line.clientWidth;
    var wl = max > 0 && line.scrollLeft > 0;
    var wr = max > 0 && line.scrollLeft < max - 1;
    var has = " " + line.className + " ";
    if (
      wl === (has.indexOf(" zbar-usage-qfade-l ") >= 0) &&
      wr === (has.indexOf(" zbar-usage-qfade-r ") >= 0)
    ) {
      return;
    }
    line.className =
      "zbar-usage-qb-line" +
      (wl ? " zbar-usage-qfade-l" : "") +
      (wr ? " zbar-usage-qfade-r" : "");
  }

  function renderAll() {
    /* V9：document 级扫描——子代理详情面板与主对话同 document 且在
     * workspace-main 锚点之外，每轮条扫描不再限定锚点（35 节点量级，
     * 性能无虞）；锚点仅保留给会话条定位 */
    var anchor = document.querySelector(SEL_PANE_ANCHOR);
    var nodes = document.querySelectorAll(SEL_TURN);
    if (!nodes.length) {
      /* V18：空会话（新建任务/清空对话）同样要推进会话条——此前此处
       * 早退导致 renderSessionBar 永不执行，条停留在上一个会话的
       * 累计值永不消失（实机复现：新任务容器 data-session-id="draft"
       * 可见且含焦点，V17 的会话判定本身正确，缺的就是这一步） */
      removeBar();
      /* V29：空会话页同步清理额度行与模型行容器——额度行 V28 曾在此
       * 照常渲染，是新建任务页额度行孤立悬浮在输入框与快捷建议之间、
       * 压着输入框下边框的来源（该页没有会话条承载视觉上下文）；模型
       * 行容器自会话条拆出后同样在此清理（保持 V28 及之前空会话页无
       * 模型行的观感）。与「会话条/每轮条无数据不显示」的自然逻辑一
       * 致，第一条消息发出后由主路径恢复渲染 */
      removeQuotaBar();
      removeModelsBox();
      /* 拆出后的留白由 syncComposerPad 统一计算：两个注入层清干净后
       * 在此同步一次，输入区还原基础留白（26px） */
      syncComposerPad();
      return;
    }
    /* 第一遍：按 DOM 顺序确定每个 umid 的最后一个节点（querySelectorAll
     * 保证 DOM 顺序，后者覆盖前者即"最后一个"；同一 umid 可能命中多个
     * 虚拟列表单元节点） */
    var lastOf = {};
    var seen = {};
    for (var i = 0; i < nodes.length; i++) {
      var id = nodes[i].getAttribute(ATTR_TURN_ID);
      if (!id) continue;
      seen[id] = true;
      lastOf[id] = nodes[i];
    }
    /* 多容器活动轮判定（每会话一个活动轮，Map：sess → 节点）；它只
     * 决定启动窗口等待行，不采样 DOM 文本。 */
    var activeMap = findLiveNodes();
    /* V29（P1 修复）：当前会话数据判定（renderSessionBar「无数据即
     * removeBar」条件的同口径提炼，见 currentSessionHasData）——多会
     * 话保活下主路径因旧会话轮节点仍挂载 DOM 而照常执行，模型行/额度
     * 行改以本判定作渲染闸：当前会话（draft/新建任务）无数据时三元素
     * （会话条/模型行/额度行）一并隐藏，切回有数据会话后由既有
     * MutationObserver/兜底渲染拍自愈恢复 */
    var sessHasData = currentSessionHasData(anchor, activeMap);
    /* 第二遍：逐节点渲染（仅"最后一个"节点出内容，其余节点清旧行）。
     * V19：每轮统计条开关——循环前读一次 --zbar-usage-turn-bar（变量
     * 缺失视为开启，兼容旧 variables.css），关闭时对全部轮节点
     * removeRow 并跳过 renderOne（幂等清掉已渲染行，开启后自动恢复）。 */
    var turnBarOn = turnBarEnabled();
    for (var i = 0; i < nodes.length; i++) {
      var id = nodes[i].getAttribute(ATTR_TURN_ID);
      if (!id) continue;
      if (!turnBarOn) {
        removeRow(nodes[i], id);
        continue;
      }
      renderOne(nodes[i], id, lastOf[id] === nodes[i], activeMap);
    }
    /* 虚拟列表回收：节点从 DOM 消失（连带统计条断连）即清缓存不残留 */
    rendered.forEach(function (rowEl, id) {
      if (!seen[id] || !rowEl.isConnected) rendered.delete(id);
    });
    /* 会话级统计条独立渲染，失败不影响每轮条。 */
    try {
      renderSessionBar(anchor, activeMap);
    } catch (e) {
      /* 会话条失败静默，下个变更/兜底周期重试 */
    }
    /* V29：模型速度行独立渲染（容器/开关/管线均已从会话条解耦）——
     * 有模型数据且开关开启才渲染容器；会话条开关关闭时模型行照常显示
     * （这是拆出的核心诉求），当前会话无数据时经 sessHasData 闸随会
     * 话条一并隐藏（P1 修复）。失败静默，下个周期重试。 */
    try {
      renderModelRows(sessHasData);
    } catch (e) {
      /* 模型行失败静默，下个变更/兜底周期重试 */
    }
    /* V28：独立额度行与会话条解耦渲染——会话条开关关闭/数据缺失/挂
     * 载失败都不影响额度行（V29 P1 修复：当前会话无数据时经
     * sessHasData 闸隐藏，取代初版全局级 sessionDataEmpty 判定）；额
     * 度数据 onload 与开关热重载随本渲染节拍直接生效，不依赖会话条渲
     * 染节奏。V30：调用再传 anchor，额度行自算当前会话缓存命中率
     * （「缓存 NN%」段，与会话条同拍同口径）。V33：再传 activeMap，
     * 「速度」「本轮」分段取当前会话活动轮 runs 行（1s 旁路实时值优
     * 先），与 sessHasData/anchor 同源同拍。 */
    try {
      renderQuotaBar(sessHasData, anchor, activeMap);
    } catch (e) {
      /* 额度行失败静默，下个变更/兜底周期重试 */
    }
    /* V29：输入区顶部留白统一计算——会话条与模型行两个注入层状态就
     * 位后求一次值（syncComposerPad 按两个开关的挂载态独立叠加，防两
     * 条渲染路径互相覆盖）。失败静默（内联样式异常已在其内部自愈）。 */
    try {
      syncComposerPad();
    } catch (e) {
      /* 留白计算失败静默，下个变更/兜底周期重试 */
    }
  }

  var scheduled = false;

  function scheduleRender() {
    if (scheduled) return;
    scheduled = true;
    var raf = window.requestAnimationFrame || function (f) { setTimeout(f, 50); };
    raf(function () {
      try {
        renderAll();
      } catch (e) {
        /* 单轮失败静默，下次变更重试 */
      } finally {
        /* 复位必须在 finally：若 renderAll 抛异常导致复位被跳过，
         * scheduled 会永久为 true，此后所有 scheduleRender 被拦截，
         * 渲染管线死锁 */
        scheduled = false;
      }
    });
  }

  /* ---- DOM 扫描：属性变化（虚拟列表复用节点改 data-turn-id /
   *      data-running）与子树增删统一走全量重渲（对话区节点量小，
   *      单次遍历开销可忽略，rAF 合并避免逐帧重复渲染） ---- */
  try {
    new MutationObserver(scheduleRender).observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: [ATTR_TURN_ID, ATTR_RUNNING]
    });
  } catch (e) {
    /* 观察器不可用时仅失去动态刷新，静默 */
  }

  ensureStyle();
  /* V13：会话条 absolute 挂进输入区容器、随文档流自适应，无需 V12 的
   * window resize 重定位监听（已删除） */
  scheduleRender(); /* 立即渲染已挂载的轮次，不等首个数据周期 */
  pollLoop();
  speedPollLoop();
  quotaPollLoop(); /* V27：额度旁路低频重载（独立于大文件/速度节拍） */
  /* 死锁/漏渲染兜底：低频定时器不经过 scheduled 检查直接调用一次
   * renderAll——即使未来出现新的意外状态（调度标志卡死、事件丢失等）
   * 也能在一个兜底周期内自愈，恢复统计条显示 */
  setInterval(function () {
    try {
      renderAll();
    } catch (e) {
      /* 兜底轮失败静默，下个周期再试 */
    }
  }, FALLBACK_RENDER_MS);
})();
"#;

/// 对话页桌面像素宠物脚本（版本化落盘模板）。
/// V2 起拆为两段拼接，宠物核心单源化（第二阶段独立悬浮窗宠物与注入版
/// 共用同一份形象库/状态机/渲染器，防两份帧数据漂移）：
/// - 核心段 = public/pet-core.js（include_str! 编译期引入，文件即唯一
///   真相源：PET_STYLES 帧数据 + 状态机 + canvas 渲染器 + ZBarPet 工厂
///   接口 create(container, opts) / feed(data) / heartbeat(ms) /
///   setParams / destroy，无环境假设——参数与数据一律由宿主喂入）；
/// - 宿主壳段（下方内联，普通 r#"…"# 原始字符串，正文无 "# 序列）=
///   注入版专属：每秒读 variables.css 的 --zbar-pet-* 变量（开关/形象/
///   大小，随热重载约 1 秒生效）调 create/destroy/setParams；读
///   usage.js 每 2 秒重载的 window.__ZBAR_USAGE__ 喂数据给 feed；每
///   2 秒经 script 时间戳重载心跳小文件 usage-data-hb.js 喂 heartbeat
///   （大文件保持内容不变跳写，存活信号由独立小文件承担，见
///   usage_feed 模块头）；形象值为 custom:<id>（V3 自定义宠物，V8 起
///   含内置智谱娘的全部形象）时按需加载同目录物化文件 pet-custom.js
///   取 window.__ZBAR_PET_CUSTOM__ 作为 customAsset 传入核心（静态资产
///   不轮询，V8 起资产缺失不再回退内建形象——核心进入空态，宠物暂隐，
///   资产就位后下个 tick 自动热切换）；并注入对话页右下角的 fixed
///   定位样式（pointer-events:none 不捕获鼠标）。独立悬浮窗版宿主
///   （AgentHUD 透明宠物窗口页面）同样加载 pet-core.js，参数直接调
///   setParams、数据监听 Tauri 事件喂 feed。
/// 版本头（ZBAR-THEME-V 标记）写在 pet-core.js 首行，两段拼接后位于
/// 落盘文件头部，ensure_versioned_template 的提取器无感知。
pub const PET_JS: &str = concat!(
    include_str!("../../../public/pet-core.js"),
    "\n",
    r#"// ============================================================
// ZBAR 宠物注入版宿主壳（拼接在 pet-core.js 之后落盘为 pet.js）
// ============================================================
// 皮肤注入环境专属逻辑：核心（上方的 ZBarPet 工厂）不做任何环境假设，
// 本壳负责把 ZCode 对话页的环境接入核心——
// - 参数来源：每 tick 读 variables.css 的 --zbar-pet-* 变量（effects.js
//   每秒重载该文件，皮肤页改参数约 1 秒内生效：关闭销毁宠物实例、
//   开启重建、形象热切换重建、大小经 setParams 即时生效）；
// - 数据来源：usage.js 每 2 秒把 usage-data.js 重载到
//   window.__ZBAR_USAGE__，本壳只读全局对象不重复请求大文件；
// - 心跳来源：每 2 个 tick 经 script 时间戳重载同目录心跳小文件
//   usage-data-hb.js（几十字节，ZBar 侧仅注入版宠物开启时写出），
//   读 window.__ZBAR_USAGE_HB__ 喂给核心 heartbeat 接口——大文件保持
//   "内容不变跳写"（防写放大），存活信号由独立小文件承担；心跳文件
//   缺失（旧版 ZBar / 宠物关闭中）时加载失败静默，核心回退用数据 ts
//   判陈旧（V1 行为）；
// - 自定义形象（V3 引入，V8 起为全部形象的唯一通道）：--zbar-pet-style
//   形如 custom:<id> 时，按需加载同目录 pet-custom.js（静态资产，不
//   轮询——ZBar 侧仅选中变更/导入删除时重写该文件），校验
//   window.__ZBAR_PET_CUSTOM__.meta.id 与选中 id 一致后作为
//   customAsset 传入核心；资产缺失/不匹配（加载未完成/文件缺失/选中
//   已删除）时核心进入空态——宠物暂隐（V8 起无内建回退，不再闪猫脸），
//   资产就绪后下个 tick 自动热切换；
// - 定位与拖拽（V7）：宠物 fixed 悬浮于对话页内（默认右下角，边距与
//   旧版 right:18px/bottom:14px 观感一致），支持鼠标拖拽移动，位置经
//   localStorage（__zbarPetPos）持久化、每 tick 轻量 clamp 进视口
//   （窗口 resize 后拉回可见区）；容器接收鼠标事件（旧的鼠标事件遮蔽
//   已移除），z-index 20 低于会话累计条（usage.js 为 30）与弹窗层。
//   拖拽状态三重收尾：容器自身 pointerup/cancel、容器重建时
//   ensurePetContainer 的同步复位、window 级兜底监听——防止 dragging
//   标志卡死挡死后续拖拽。
// ============================================================
(function () {
  "use strict";

  if (!document.body) return;
  if (!window.ZBarPet) return; /* 核心缺失（异常环境）静默退出 */

  var STYLE_ID = "zbar-pet-style";
  var ATTR_SELF = "data-zbar-pet"; /* 本壳注入行的标记（推导心跳文件地址） */
  var HB_FILE = "usage-data-hb.js";
  var CUSTOM_FILE = "pet-custom.js";
  var CUSTOM_PREFIX = "custom:";
  var VAR_ENABLED = "--zbar-pet-enabled"; /* 开关变量（variables.css 渲染 1/0） */
  var VAR_STYLE = "--zbar-pet-style"; /* 形象 id 变量（custom:<id> 原样透传） */
  var VAR_SIZE = "--zbar-pet-size"; /* 像素边长变量（壳读取后喂给核心） */
  var VAR_ASSET_VER = "--zbar-pet-asset-ver"; /* 物化资产内容戳（重复导入
    同 id 的热刷新信号，见下方资产同步说明） */
  var TICK_MS = 1000; /* 参数与数据轮询周期（不做页面隐藏降频，见 tick） */
  var HB_TICKS = 2; /* 心跳重载周期 = 2 个 tick（与数据重载同节奏） */
  var ATTR_DRAGGING = "data-zbar-pet-dragging"; /* 拖拽进行中标记（光标态） */
  var POS_KEY = "__zbarPetPos"; /* 位置持久化键（{"l":n,"t":n} 视口 px） */
  var POS_MARGIN_R = 18; /* 默认位置右边距（px，与旧版 right:18px 一致） */
  var POS_MARGIN_B = 14; /* 默认位置下边距（px，与旧版 bottom:14px 一致） */

  function readVar(name) {
    try {
      return (
        getComputedStyle(document.documentElement).getPropertyValue(name) ||
        ""
      ).trim();
    } catch (e) {
      return "";
    }
  }

  function petEnabled() {
    /* 值非 "1" 一律视为关闭（含变量缺失的旧 variables.css）：宠物默认
     * 关闭，不惊扰未开启过的用户；与 usage.js 的开关语义（缺失视为
     * 开启）相反，因二者默认值不同 */
    return readVar(VAR_ENABLED) === "1";
  }

  function styleIdOf() {
    var id = readVar(VAR_STYLE);
    return id && window.ZBarPet ? id : ""; /* 空串 = 资产未就位（V8 起
      无内建回退：变量缺失/非法值一律视为未渲染，核心空态暂隐） */
  }

  /* ---- 自定义形象资产（V3）：校验 window.__ZBAR_PET_CUSTOM__ 的 id 与
   *      选中一致（ZBar 侧仅在选中变更时重写 pet-custom.js，页面已加载
   *      的副本可能滞后于热切换，按 id 比对保证不渲染错宠物） ---- */
  function customAssetFor(styleId) {
    if (styleId.indexOf(CUSTOM_PREFIX) !== 0) return null;
    var a = window.__ZBAR_PET_CUSTOM__;
    if (
      a &&
      a.meta &&
      a.dataUri &&
      String(a.meta.id || "") === styleId.slice(CUSTOM_PREFIX.length)
    ) {
      return a;
    }
    return null;
  }

  /* ---- 自定义资产懒加载（不轮询）：缺失/不匹配时经 script 时间戳重载
   *      同目录 pet-custom.js 一次，onload/onerror 后由主循环的下一 tick
   *      消费（加载完成 → 自动热切换到自定义形象）；in-flight 防重入。
   *      连续失败（文件确实不在）退避为每 10 tick 重试一次，防每秒空转 ---- */
  var customLoading = false;
  var customFails = 0;
  function reloadCustomAsset() {
    if (customLoading) return;
    if (customFails >= 3 && tickCount % 10 !== 0) return;
    try {
      var self = document.querySelector("script[" + ATTR_SELF + "]");
      if (!self || !self.src) return;
      customLoading = true;
      var url = self.src.replace(/[^/]*$/, CUSTOM_FILE) + "?t=" + Date.now();
      var s = document.createElement("script");
      var done = function () {
        if (s.parentNode) s.parentNode.removeChild(s);
        customLoading = false;
      };
      s.onload = function () {
        customFails = 0; /* 成功复位（下次缺失再即时加载） */
        done();
      };
      s.onerror = function () {
        customFails++; /* 文件缺失（未物化/被清理）静默，核心空态暂隐 */
        done();
      };
      s.src = url;
      (document.head || document.documentElement).appendChild(s);
    } catch (e) {
      customLoading = false;
      /* 静默 */
    }
  }

  function sizeOf() {
    var n = parseInt(readVar(VAR_SIZE), 10);
    return isFinite(n) && n > 0 ? n : 64;
  }

  /* 对话页定位样式：fixed + 可拖拽（V7）。选择器精确到 div——本壳注入
   * 行的 script 标签同样带 data-zbar-pet 属性（ATTR_SELF），不带 div
   * 限定会误命中。容器需要接收鼠标事件（移除了旧的鼠标事件遮蔽），
   * touch-action:none 阻止触摸拖拽触发滚动，user-select:none 防拖拽
   * 选中页面文字 */
  function ensureStyle() {
    if (document.getElementById(STYLE_ID)) return;
    var st = document.createElement("style");
    st.id = STYLE_ID;
    st.textContent =
      "div[data-zbar-pet]{position:fixed;z-index:20;" +
      "cursor:grab;touch-action:none;" +
      "user-select:none;-webkit-user-select:none;}" +
      "div[data-zbar-pet][" + ATTR_DRAGGING + "]{cursor:grabbing;}";
    (document.head || document.documentElement).appendChild(st);
  }

  /* ---- 位置管理与拖拽（V7）：位置存 localStorage（读写 try/catch 静默，
   *      隐私模式/配额满不炸）；坐标实时 clamp 进视口，窗口 resize 后由
   *      tick 的轻量 clamp 拉回可见区。核心的 create 与 setParams 形象
   *      切换都会重建容器 div——容器获取 + 位置应用 + 拖拽绑定封装为
   *      ensurePetContainer，每 tick 比对容器身份，变化即重挂 ---- */
  var petEl = null; /* 当前已应用位置并绑定拖拽的容器（div[data-zbar-pet]） */
  var dragging = false; /* 拖拽进行中（视口 clamp 让位，不抢实时位置） */
  var dragEl = null; /* 拖拽进行中的容器（window 级兜底复位用；容器重建
    /指针捕获失败时 el 自身的 up 监听不再触达，靠全局兜底收尾） */

  function readPos() {
    try {
      var raw = window.localStorage.getItem(POS_KEY);
      if (!raw) return null;
      var v = JSON.parse(raw);
      if (
        v &&
        typeof v.l === "number" &&
        isFinite(v.l) &&
        typeof v.t === "number" &&
        isFinite(v.t)
      ) {
        return { l: v.l, t: v.t };
      }
    } catch (e) {
      /* 静默 */
    }
    return null;
  }

  function savePos(l, t) {
    try {
      window.localStorage.setItem(POS_KEY, JSON.stringify({ l: l, t: t }));
    } catch (e) {
      /* 静默 */
    }
  }

  /* 坐标 clamp 进视口（负值/超出均拉回；视口小于容器时夹到 0 起点） */
  function clampPos(l, t, w, h) {
    var vw = window.innerWidth || 0;
    var vh = window.innerHeight || 0;
    return {
      l: Math.min(Math.max(l, 0), Math.max(0, vw - w)),
      t: Math.min(Math.max(t, 0), Math.max(0, vh - h)),
    };
  }

  function applyPos(el, pos) {
    el.style.left = Math.round(pos.l) + "px";
    el.style.top = Math.round(pos.t) + "px";
  }

  function elSize(el) {
    return {
      w: el.offsetWidth || sizeOf(),
      h: el.offsetHeight || sizeOf(),
    };
  }

  /* 默认位置：视口右下角留边距（与旧版 right:18px/bottom:14px 观感一致；
   * 高度取容器实际值——自定义形象帧为 192×208 非正方形） */
  function defaultPos(el) {
    var s = elSize(el);
    var vw = window.innerWidth || 0;
    var vh = window.innerHeight || 0;
    return {
      l: Math.max(0, vw - s.w - POS_MARGIN_R),
      t: Math.max(0, vh - s.h - POS_MARGIN_B),
    };
  }

  /* 容器重挂（create/setParams 重建后调用）：应用持久化位置（clamp 进
   * 视口；无记录用默认右下角）并绑定拖拽。幂等——同一容器不重复绑。
   * 容器身份变化说明旧容器已被核心移除：若其上有进行中的拖拽，旧容器
   * 不再派发 pointerup/pointercancel，必须在此复位 dragging——否则
   * dragging 永真会挡死新容器的 pointerdown 与每 tick 的视口 clamp */
  function ensurePetContainer() {
    if (!pet) return;
    var el = null;
    try {
      el = document.querySelector("div[data-zbar-pet]");
    } catch (e) {
      /* 静默 */
    }
    if (!el || el === petEl) return;
    if (petEl && dragging) {
      /* 拖拽中容器被重建：按旧容器最后位置兜底持久化一次（用户松手前
       * 的落点），再复位拖拽状态 */
      savePos(parseFloat(petEl.style.left) || 0, parseFloat(petEl.style.top) || 0);
    }
    dragging = false;
    dragEl = null;
    petEl = el;
    var s = elSize(el);
    var pos = readPos();
    applyPos(el, pos ? clampPos(pos.l, pos.t, s.w, s.h) : defaultPos(el));
    bindDrag(el);
  }

  /* 鼠标/触摸拖拽：pointerdown 捕获 → move 实时更新（clamp 视口内）→
   * up/cancel 释放并把最终位置持久化。拖拽期间打 ATTR_DRAGGING 标记
   * （光标态切换），动画由核心的 interval 驱动不受影响。up 处理器与
   * window 级兜底（globalDragEnd）双保险：正常路径 el 的 up 先触发并
   * 置 dragEl = null，兜底随后空转（幂等） */
  function bindDrag(el) {
    el.addEventListener("pointerdown", function (ev) {
      if (dragging || ev.button !== 0) return; /* 仅主键，防重入 */
      dragging = true;
      dragEl = el;
      el.setAttribute(ATTR_DRAGGING, "");
      try {
        el.setPointerCapture(ev.pointerId);
      } catch (e) {
        /* 捕获失败仍可拖（事件不离开元素区间即继续；up 若只到达
         * window，由 globalDragEnd 兜底收尾） */
      }
      var baseL = parseFloat(el.style.left) || 0;
      var baseT = parseFloat(el.style.top) || 0;
      var ox = ev.clientX;
      var oy = ev.clientY;
      var move = function (e) {
        var s = elSize(el);
        applyPos(
          el,
          clampPos(baseL + (e.clientX - ox), baseT + (e.clientY - oy), s.w, s.h)
        );
      };
      var up = function (e) {
        el.removeEventListener("pointermove", move);
        el.removeEventListener("pointerup", up);
        el.removeEventListener("pointercancel", up);
        try {
          el.releasePointerCapture(e.pointerId);
        } catch (err) {
          /* 静默 */
        }
        el.removeAttribute(ATTR_DRAGGING);
        dragging = false;
        dragEl = null; /* 先于 window 兜底收尾（冒泡到达时已空转） */
        savePos(parseFloat(el.style.left) || 0, parseFloat(el.style.top) || 0);
      };
      el.addEventListener("pointermove", move);
      el.addEventListener("pointerup", up);
      el.addEventListener("pointercancel", up);
      ev.preventDefault(); /* 防拖拽触发原生选中文本/触摸滚动 */
    });
  }

  /* window 级拖拽兜底：指针捕获失败或拖拽容器被移出 DOM（形象热切换
   * 重建等）时，pointerup/pointercancel 可能不再触达容器自身的监听——
   * 全局兜底复位 dragging，防止标志永久卡死挡死后续拖拽与视口 clamp。
   * 初始化时一次性注册、宠物销毁不解绑（handler 按 dragging/dragEl
   * 状态判定，无拖拽时为空转，幂等）。容器重建路径另有 ensurePetContainer
   * 的同步复位（tick 内即时），本兜底覆盖事件丢失的残余窗口 */
  function globalDragEnd() {
    if (!dragging || !dragEl) return;
    dragging = false;
    var el = dragEl;
    dragEl = null;
    try {
      el.removeAttribute(ATTR_DRAGGING);
    } catch (e) {
      /* 静默（容器已 detach 也可安全操作 style/属性） */
    }
    savePos(parseFloat(el.style.left) || 0, parseFloat(el.style.top) || 0);
  }
  try {
    window.addEventListener("pointerup", globalDragEnd);
    window.addEventListener("pointercancel", globalDragEnd);
  } catch (e) {
    /* 注册失败退化为仅容器自身监听（现有行为） */
  }

  /* 轻量视口 clamp（每 tick 一次）：仅越界才改 style（resize 后拉回可见
   * 区；拖拽中与宠物未挂载时让位）。尺寸热切换（curSize 变化）后同样
   * 经本函数把放大的容器拉回视口 */
  function clampPetIntoView() {
    if (!pet || !petEl || dragging) return;
    var l = parseFloat(petEl.style.left);
    var t = parseFloat(petEl.style.top);
    if (!isFinite(l) || !isFinite(t)) return;
    var s = elSize(petEl);
    var p = clampPos(l, t, s.w, s.h);
    if (p.l !== l || p.t !== t) applyPos(petEl, p);
  }

  var pet = null;
  var curStyle = ""; /* 核心当前生效形象（V8：资产未就位时为空串 = 空态暂隐） */
  var seenStyle = ""; /* 上次消费的原始选中值（含 custom:<id>，变更检测） */
  var curAsset = null; /* 上次消费的自定义资产对象（身份比对：重复导入同 id
    后物化文件重载产生新对象，触发核心按资产身份重建） */
  var assetVer = ""; /* 上次消费的 --zbar-pet-asset-ver 内容戳（P1-2） */
  var curSize = 0;
  var tickCount = 0;

  /* ---- 心跳重载：按本壳注入行的 src 推导同目录心跳小文件地址，时间戳
   *      cache-bust 强制重读（Chromium 对 file:// 的常用手段），脚本执行
   *      后读全局心跳喂给核心并移除节点（防 DOM 堆积）；加载失败（旧版
   *      ZBar 未写心跳 / 文件被清理）静默，核心保持上一次心跳或回退 ts ---- */
  function reloadHeartbeat() {
    try {
      var self = document.querySelector("script[" + ATTR_SELF + "]");
      if (!self || !self.src) return;
      var url = self.src.replace(/[^/]*$/, HB_FILE) + "?t=" + Date.now();
      var s = document.createElement("script");
      s.src = url;
      var done = function () {
        if (s.parentNode) s.parentNode.removeChild(s);
      };
      s.onload = function () {
        try {
          if (pet) pet.heartbeat(window.__ZBAR_USAGE_HB__);
        } catch (e) {
          /* 静默 */
        }
        done();
      };
      s.onerror = done;
      (document.head || document.documentElement).appendChild(s);
    } catch (e) {
      /* 静默 */
    }
  }

  /* ---- 主循环：每 tick 读参数（热重载）→ 同步核心实例 → 喂数据；
   *      每 HB_TICKS 个 tick 重载一次心跳文件。不做页面隐藏降频（V4，
   *      与核心同步移除）：ZCode 注入环境的 visibility 判定与视觉可见
   *      性脱节，降频会造成参数热切换滞后与心跳断流（心跳必须守住核
   *      心 DATA_FRESH_MS 新鲜度，否则宠物误沉睡）；隐藏时的省电交给
   *      浏览器对隐藏页定时器的限流。
   *      V3 自定义形象：want 为 custom:<id> 且资产有效时把资产随
   *      create/setParams 传入核心；V8 起资产缺失（加载中/物化失败/
   *      旧 variables.css 的 cat 残留值）不再回退内建形象——effective
   *      为空串，核心进入空态宠物暂隐（约 1 秒的瞬态，优于闪错形象），
   *      资产就绪后下一 tick effective 变化触发 setParams 热切换。
   *      资产同步（P1-2）：--zbar-pet-asset-ver 内容戳随 variables.css
   *      每秒热重载到达（ZBar 侧仅在物化内容变化时改写它），值变化
   *      说明 pet-custom.js 已更新（重复导入同 id）——强制重载一次，
   *      重载后全局对象被替换为新对象，经 curAsset 身份比对把新资产
   *      传给 setParams，核心按资产对象身份重建画布（与独立版按对象
   *      身份重建的行为对齐） ---- */
  function tick() {
    try {
      if (petEnabled()) {
        var want = styleIdOf();
        var isCustom = want.indexOf(CUSTOM_PREFIX) === 0;
        var asset = customAssetFor(want);
        var effective = asset ? want : "";
        var size = sizeOf();
        if (isCustom) {
          var ver = readVar(VAR_ASSET_VER);
          /* 重载条件：资产缺失/不匹配，或内容戳变化（重复导入同 id）；
           * 变量缺失（旧版 variables.css / 未物化）视为空串，仅按资产
           * 缺失路径走（懒加载 + 失败退避），不额外触发 */
          if (!asset || (ver && ver !== assetVer)) {
            assetVer = ver;
            reloadCustomAsset();
          } else if (ver) {
            assetVer = ver;
          }
        }
        if (!pet) {
          ensureStyle();
          pet = window.ZBarPet.create(document.body, {
            style: effective,
            size: size,
            customAsset: asset
          });
          curStyle = effective;
          curAsset = asset;
          seenStyle = want;
          curSize = size;
          ensurePetContainer(); /* 应用持久化位置并绑定拖拽（V7） */
          reloadHeartbeat(); /* 创建即取一次心跳（下个整周期再刷新） */
        } else {
          if (
            want !== seenStyle ||
            effective !== curStyle ||
            (asset && curAsset !== asset)
          ) {
            /* 选中变化（custom 间切换）、资产就绪（空态 → 渲染）热
             * 切换、或资产对象被替换（重复导入）都走 setParams 重建
             * （核心对同 id 的新资产对象按身份重建），容器 div 随之
             * 重建——下一 tick 的 ensurePetContainer 重挂 */
            pet.setParams({ style: effective, customAsset: asset });
            seenStyle = want;
            curStyle = effective;
            curAsset = asset;
            ensurePetContainer();
          }
          if (size !== curSize) {
            pet.setParams({ size: size }); /* 大小热切换（内联画布尺寸） */
            curSize = size;
            clampPetIntoView(); /* 档位变化后按新尺寸 clamp 回视口 */
          }
          ensurePetContainer(); /* 容器被核心重建（防御）时重挂 */
        }
      } else if (pet) {
        pet.destroy(); /* 开关关闭：销毁宠物实例并移除 DOM */
        pet = null;
        petEl = null;
        seenStyle = "";
        curStyle = "";
        curAsset = null;
        assetVer = "";
      }
      if (pet && window.__ZBAR_USAGE__) {
        pet.feed(window.__ZBAR_USAGE__);
      }
      clampPetIntoView(); /* 每秒轻量 clamp：resize 后拉回可见区 */
      tickCount++;
      if (pet && tickCount % HB_TICKS === 0) {
        reloadHeartbeat();
      }
    } catch (e) {
      /* 静默 */
    }
    setTimeout(tick, TICK_MS);
  }

  tick();
})();
"#
);

// ============================================================
// URL 工具
// ============================================================

/// 文件路径 → file:// URL 的 percent-encoding。
/// 保留 RFC3986 unreserved（A-Za-z0-9-._~）与路径分隔符 `/`，
/// 其余字节（空格、中文等）按 %XX 编码，避免 Electron 加载外链失败。
pub fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(*b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// 剥离 Windows `Path::canonicalize` 返回的 verbatim 前缀：
/// `\\?\C:\dir\…` → `C:\dir\…`，`\\?\UNC\server\share\…` → `\\server\share\…`；
/// 非 Windows 或无前缀时原样返回。canonicalize 的结果统一过一遍，
/// 避免 verbatim 路径流入 params.json 与 file:// URL（verbatim 斜杠化后
/// 形如 `//?/C:/…`，Chromium 无法加载）。
pub(crate) fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest.to_string());
        }
        path
    }
    #[cfg(not(windows))]
    {
        path
    }
}

/// 绝对路径 → file:// URL（Unix 形如 file:///Users/…，Windows 形如 file:///C:/…）。
/// Windows 分支拼接前剥离可能残留的 `\\?\` verbatim 前缀，兜住历史脏数据；
/// POSIX 绝对路径（"/" 开头，如从 mac 迁移的 params.json）自身已含根斜杠，
/// 固定前缀降为 file:// 避免拼出 file://// 四斜杠畸形 URL。
pub fn file_url(path: &Path) -> String {
    #[cfg(windows)]
    {
        let unified = strip_verbatim_prefix(path.to_path_buf())
            .to_string_lossy()
            .replace('\\', "/");
        let prefix = if unified.starts_with('/') {
            "file://"
        } else {
            "file:///"
        };
        format!("{}{}", prefix, percent_encode_path(&unified))
    }
    #[cfg(not(windows))]
    {
        format!("file://{}", percent_encode_path(&path.to_string_lossy()))
    }
}

// ============================================================
// variables.css 生成
// ============================================================

/// 由主题参数渲染 variables.css 内容。
/// 壁纸 URL 为 file:// 绝对地址（已 percent-encoding）。
/// --zbar-wallpaper-enabled 为壁纸层总开关（由用户参数 wallpaper_enabled
/// 渲染 1/0，默认 1）：effects.js 读它决定是否挂载壁纸媒体层——显式 "0"
/// 时移除注入三层；变量缺失/空串视为开启，与默认值 true 一致，改开关约
/// 1 秒随热重载生效。
/// 关闭态同时进入"主题视觉空转"：theme.css 对 ZCode 自身的视觉改造不只
/// 壁纸层一处（全局底色 token 半透明化、三区域主容器半透明背景、文字
/// 描边），只卸载壁纸层会留下"透明模糊"的中间态（背景 25% 半透明直接
/// 透出窗口底，与用户预期"关闭 = ZCode 原本的样子"不符）。故关闭时把
/// 视觉类变量渲染为空转组合（theme.css 规则本身无开关分支，靠变量取值
/// 整体失效，等效 ZCode 原始外观）：
/// - --zbar-base-alpha = 1：8 条全局底色 token 的 color-mix 取 100% 原色
///   = ZCode 原始实色底（顶栏/卡片等全部消费方一并恢复）；
/// - --zbar-sidebar-opacity / --zbar-panel-opacity /
///   --zbar-sidebar-right-opacity = 0：三区域主容器强加的 !important 背景
///   全透明，回到 ZCode 原始"面板自身无背景、透全局底"结构（必须用 0
///   而非 1：1 会把 #sidebar 等刷成 neutral-50/900 实色，与原始全局
///   --color-sidebar 的 neutral-100/950 差一档色）；
/// - --zbar-text-shadow = 0：描边 alpha 归零自然不可见；
/// 滤镜/遮罩/速率照常输出参数真值（壁纸层已卸载无消费方，保持渲染简单，
/// 重新开启即按原值恢复）；usage 类变量（字号/不透明度/四开关）完全不受
/// 影响——"关闭壁纸只关视觉皮肤，注入的用量统计等信息照常显示"。
/// --zbar-shell-uncover 为 theme.css V12 壳规则的开关（V12 新增，与
/// --zbar-wallpaper-enabled 同源自 wallpaper_enabled 渲染反相值 1/0——
/// 开启壁纸=0、关闭=1）：theme.css 的全视口应用壳容器规则消费它决定
/// 壳底去向——0 时 color-mix 全量透明（V11 行为，壁纸垫底透出）、1 时
/// 壳底恢复 --color-background 不透明底（叠加空转组合即原始整窗观感）；
/// 变量缺失在 theme.css 侧兜底 0（透明），兼容尚未重渲的旧 variables.css。
/// --zbar-base-alpha 为全局氛围底透明度（V10 起由用户参数 base_alpha
/// 渲染真值，默认 0.25）：theme.css V5 起全部全局底色 token 由它驱动，
/// 与滑块解绑；壁纸背景关闭时按上文渲染空转值 1。
/// --zbar-text-shadow 为文字描边强度（V10 新增，由用户参数
/// text_shadow 渲染真值，0=关闭）：theme.css V10 起三区域主容器的
/// 前景文字描边由它驱动；壁纸背景关闭时按上文渲染空转值 0。两者均随
/// variables.css 每秒热重载即时生效。
/// --zbar-usage-font-size / --zbar-usage-opacity 为对话内用量统计条的
/// 字号与文字不透明度（V4 新增，由用户参数 usage_font_size /
/// usage_opacity 渲染真值）：usage.js V4 起统计条样式消费这两个变量
/// （模板内仅保留原写死值兜底），同样随每秒热重载即时生效。
/// --zbar-usage-session-bar 为会话级实时统计条的开关（V5 新增，由用户
/// 参数 usage_session_bar 渲染 1/0）：usage.js V5 起读它决定是否渲染
/// 会话条（变量缺失视为开启，与默认值 true 一致），改开关约 1 秒随
/// 热重载生效。
/// --zbar-usage-turn-bar 为每轮末尾统计条的开关（V19 新增，由用户参数
/// usage_turn_bar 渲染 1/0）：usage.js V19 起 renderAll 第二遍渲染前读它
/// 决定是否渲染每轮条（变量缺失视为开启，与默认值 true 一致），同样随
/// 每秒热重载即时生效。
/// --zbar-usage-quota-bar 为输入框额度行的开关（V28 新增，由用户参数
/// usage_quota_bar 渲染 1/0）：usage.js V28 起读它决定是否渲染输入框
/// 卡片底部外侧的独立额度行（额度 5h/周剩余百分比，V29 起空会话隐藏、
/// 时间戳尾段删除；变量缺失视为开启，与默认值 true 一致），随每秒热
/// 重载即时生效。
/// --zbar-usage-model-rows 为输入框上方模型速度行的开关（V29 新增，由
/// 用户参数 usage_model_rows 渲染 1/0）：usage.js V29 起读它决定是否
/// 渲染独立挂载的模型分组速度行（自会话条拆出的独立容器，与会话累计
/// 条开关相互独立——会话条关闭时模型行照常显示；变量缺失视为开启，与
/// 默认值 true 一致），随每秒热重载即时生效。
/// --zbar-pet-enabled 为桌面像素宠物的开关（由 PetConfig 的
/// enabled && mode==injected 渲染 1/0，默认 0——宠物配置统一收敛到
/// pet.json 后，ThemeParams 不再承载宠物参数）：pet.js 读它决定渲染
/// （值非 "1" 一律视为关闭——宠物默认关闭不惊扰现有用户，与 usage_*
/// 开关的"缺失视为开启"语义相反），关闭移除宠物 DOM、开启重建，随
/// 每秒热重载约 1 秒生效。
/// --zbar-pet-style 为宠物形象 id（由 PetConfig.style 渲染，V8 起合法
/// 值恒为 custom:<id>——默认内置智谱娘，原样透传）：pet.js 读它选形象
/// （未知值/缺资产均进入空态暂隐，资产就位后热切换），形象变化热切换
/// （重建画布）。
/// --zbar-pet-asset-ver 为自定义宠物物化资产的内容戳（pets::sync_theme_
/// custom_pet 物化/清除后重渲本文件时计算，非用户参数）：pet.js 壳每秒
/// 随热重载读取，值变化（重复导入同 id 更新了 pet-custom.js）即重载资产
/// 并按资产对象身份热重建（P1-2，无此变量时壳按缺失处理、不触发）。
/// --zbar-pet-size 为宠物显示边长（PetConfig.size 档位由调用方按屏幕
/// 逻辑高换算好的整数 px，如 81px）：pet.js 的画布 CSS 尺寸直接消费
/// 该变量（image-rendering:pixelated 最近邻放大），改档位零 JS 成本、
/// 随热重载即时生效。兼容硬约束：值恒为整数 px——ZCode 页面可能仍运行
/// 旧版 pet.js（sizeOf 用 parseInt 解析），写 vh/百分比等非 px 值会被
/// 解析出错误小数值把宠物缩成蚂蚁。
pub fn render_variables_css(
    params: &ThemeParams,
    wallpaper_url: &str,
    pet_asset_ver: &str,
    pet_enabled: u8,
    pet_style: &str,
    pet_size_px: u32,
) -> String {
    // 壁纸背景关闭时取"主题视觉空转"组合（语义见函数文档）：全局底
    // base_alpha 归 1（原色实色）、三区域主容器不透明度归 0（回透明
    // 结构）、文字描边归 0（不可见），theme.css 对 ZCode 自身的视觉
    // 改造整体失效，等效原始外观；开启则一律输出参数真值。取值在
    // 渲染处覆写而不动 ThemeParams——参数真值原样留在 params.json，
    // 重新开启即按原参数恢复。滤镜/遮罩/速率不在空转之列：壁纸层已
    // 卸载时无消费方，照常输出真值保持渲染简单；usage 类变量与四个
    // 开关也不受影响——关闭壁纸只关视觉皮肤，统计条等信息照常显示。
    let (panel, sidebar, sidebar_right, base_alpha, text_shadow) = if params.wallpaper_enabled {
        (
            params.panel_opacity,
            params.sidebar_opacity,
            params.sidebar_right_opacity,
            params.base_alpha,
            params.text_shadow,
        )
    } else {
        (0.0, 0.0, 0.0, 1.0, 0.0)
    };
    format!(
        "/* ZBar 自动生成的主题变量，请勿手工编辑 */\n\
         /* 修改参数请在 ZBar 面板的动态壁纸设置中进行 */\n\
         :root {{\n\
         \x20 --zbar-wallpaper-url: url(\"{url}\");\n\
         \x20 --zbar-wallpaper-enabled: {wallpaper_enabled};\n\
         \x20 --zbar-shell-uncover: {shell_uncover};\n\
         \x20 --zbar-wp-brightness: {brightness};\n\
         \x20 --zbar-wp-saturate: {saturate};\n\
         \x20 --zbar-wp-blur: {blur}px;\n\
         \x20 --zbar-mask-strength: {mask};\n\
         \x20 --zbar-panel-opacity: {panel};\n\
         \x20 --zbar-sidebar-opacity: {sidebar};\n\
         \x20 --zbar-sidebar-right-opacity: {sidebar_right};\n\
         \x20 --zbar-base-alpha: {base_alpha};\n\
         \x20 --zbar-text-shadow: {text_shadow};\n\
         \x20 --zbar-usage-font-size: {usage_font_size}px;\n\
         \x20 --zbar-usage-opacity: {usage_opacity};\n\
         \x20 --zbar-usage-session-bar: {usage_session_bar};\n\
         \x20 --zbar-usage-turn-bar: {usage_turn_bar};\n\
         \x20 --zbar-usage-quota-bar: {usage_quota_bar};\n\
         \x20 --zbar-usage-model-rows: {usage_model_rows};\n\
         \x20 --zbar-pet-enabled: {pet_enabled};\n\
         \x20 --zbar-pet-style: {pet_style};\n\
         \x20 --zbar-pet-size: {pet_size}px;\n\
         \x20 --zbar-pet-asset-ver: {pet_asset_ver};\n\
         \x20 --zbar-playback-rate: {rate};\n\
         }}\n",
        url = wallpaper_url,
        wallpaper_enabled = if params.wallpaper_enabled { 1 } else { 0 },
        // V12 壳规则开关：与 wallpaper_enabled 同源取反相值（开启壁纸=0，
        // theme.css 壳容器 color-mix 全量透明、壁纸垫底透出即 V11 行为；
        // 关闭=1，壳底恢复 --color-background 不透明底，与空转组合叠加
        // 还原原始整窗观感）。不受视觉空转覆写影响——它本身就是空转的
        // 一部分，随开关同源翻转
        shell_uncover = if params.wallpaper_enabled { 0 } else { 1 },
        brightness = params.wp_brightness,
        saturate = params.wp_saturate,
        blur = params.wp_blur,
        mask = params.mask_strength,
        // 视觉五变量取上方解构出的局部值（开启 = 参数真值，关闭 = 空转
        // 组合），不得直写 params.* 真值——否则关闭态空转失效、ZCode 停
        // 留在半透明中间态；滤镜/遮罩/速率与 usage 类仍按参数真值渲染
        // （同名的 `x = x` 显式形式：宏内裸标识符会被当作位置参数，与
        // 命名参数混用直接编译报错，不能写成简写）
        panel = panel,
        sidebar = sidebar,
        sidebar_right = sidebar_right,
        base_alpha = base_alpha,
        text_shadow = text_shadow,
        usage_font_size = params.usage_font_size,
        usage_opacity = params.usage_opacity,
        usage_session_bar = if params.usage_session_bar { 1 } else { 0 },
        usage_turn_bar = if params.usage_turn_bar { 1 } else { 0 },
        usage_quota_bar = if params.usage_quota_bar { 1 } else { 0 },
        usage_model_rows = if params.usage_model_rows { 1 } else { 0 },
        pet_enabled = pet_enabled,
        pet_style = pet_style,
        pet_size = pet_size_px,
        pet_asset_ver = pet_asset_ver,
        rate = params.playback_rate,
    )
}

// ============================================================
// 注入块操作
// ============================================================

/// html 中是否已含注入标记（用于安装后抽检与状态检测）
pub fn has_inject(html: &str) -> bool {
    html.contains(INJECT_BEGIN)
}

/// 剥离所有旧的注入标记块（BEGIN…END 之间的全部内容连同标记，
/// 及块尾紧随的换行），保证重复安装幂等且还原后无残留空行；
/// 孤立的 BEGIN 标记（无 END 闭合）也一并清除。
pub fn strip_inject_blocks(html: &str) -> String {
    let mut out = html.to_string();
    loop {
        let Some(b) = out.find(INJECT_BEGIN) else {
            break;
        };
        match out[b..].find(INJECT_END) {
            Some(rel) => {
                let mut e = b + rel + INJECT_END.len();
                // 块尾紧随的换行一并移除，避免剥离后残留空行
                if out[e..].starts_with('\n') {
                    e += 1;
                }
                out.replace_range(b..e, "");
            }
            None => {
                // 不完整的残留块：仅移除标记本身
                out.replace_range(b..b + INJECT_BEGIN.len(), "");
            }
        }
    }
    out
}

/// 大小写不敏感查找（仅用于 ASCII 标签 </head> / </body>；
/// to_ascii_lowercase 字节一一对应，索引可直接用于原串）
fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

/// 在 tag 前插入 block
fn insert_before(html: &str, tag: &str, block: &str, tag_desc: &str) -> Result<String, String> {
    match find_ignore_case(html, tag) {
        Some(i) => Ok(format!("{}{}{}", &html[..i], block, &html[i..])),
        None => Err(format!("index.html 结构异常：未找到 {tag_desc}（{tag}），已中止注入")),
    }
}

/// 向解包后的 index.html 注入主题外链引用。
/// 幂等：先剥离旧注入块，再插入新块：
///   - variables.css + theme.css（带 data-zbar-variables / data-zbar-theme
///     标记，供 effects.js 定位热重载目标）于 `</head>` 前
///   - effects.js（defer，带 data-zbar-effects 标记）+
///     usage.js（defer，带 data-zbar-usage 标记，对话页用量统计条）+
///     pet.js（defer，带 data-zbar-pet 标记，对话页桌面像素宠物，消费
///     usage.js 重载的 window.__ZBAR_USAGE__ 数据）于 `</body>` 前
/// 返回写回后的完整 html。
pub fn apply_inject(staging_index_html: &Path, theme_dir: &Path) -> Result<String, String> {
    let raw = fs::read_to_string(staging_index_html)
        .map_err(|e| format!("读取 index.html 失败: {e}"))?;
    let cleaned = strip_inject_blocks(&raw);

    let vars_url = file_url(&theme_dir.join(crate::agent_theme::store::VARIABLES_CSS));
    let theme_url = file_url(&theme_dir.join(crate::agent_theme::store::THEME_CSS));
    let effects_url = file_url(&theme_dir.join(crate::agent_theme::store::EFFECTS_JS));
    let usage_url = file_url(&theme_dir.join(crate::agent_theme::store::USAGE_JS));
    let pet_url = file_url(&theme_dir.join(crate::agent_theme::store::PET_JS));

    let head_block = format!(
        "{INJECT_BEGIN}\n<link rel=\"stylesheet\" href=\"{vars_url}\" data-zbar-variables=\"\">\n<link rel=\"stylesheet\" href=\"{theme_url}\" data-zbar-theme=\"\">\n{INJECT_END}\n"
    );
    let body_block = format!(
        "{INJECT_BEGIN}\n<script defer src=\"{effects_url}\" data-zbar-effects=\"\"></script>\n<script defer src=\"{usage_url}\" data-zbar-usage=\"\"></script>\n<script defer src=\"{pet_url}\" data-zbar-pet=\"\"></script>\n{INJECT_END}\n"
    );

    let mut html = insert_before(&cleaned, "</head>", &head_block, "文档头结束标签")?;
    html = insert_before(&html, "</body>", &body_block, "文档体结束标签")?;

    fs::write(staging_index_html, &html).map_err(|e| format!("写回 index.html 失败: {e}"))?;
    Ok(html)
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "zbar-agent-theme-inject-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    const SAMPLE_HTML: &str = "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>ZCode</title>\n</head>\n<body>\n<div id=\"root\"></div>\n</body>\n</html>\n";

    #[test]
    fn 注入幂等_二次注入不多插() {
        let dir = test_dir("idempotent");
        let index = dir.join("index.html");
        fs::write(&index, SAMPLE_HTML).unwrap();

        // 第一次注入：head + body 各一个标记块
        let html1 = apply_inject(&index, &dir).unwrap();
        assert!(has_inject(&html1));
        assert_eq!(html1.matches(INJECT_BEGIN).count(), 2);
        assert!(html1.contains("</head>"));
        assert!(html1.contains("</body>"));

        // 第二次注入（对同一文件重复执行）：仍是 2 个标记块，不多插
        let html2 = apply_inject(&index, &dir).unwrap();
        assert_eq!(html2.matches(INJECT_BEGIN).count(), 2, "二次注入不应产生多余标记块");
        assert_eq!(html2.matches(INJECT_END).count(), 2);
        assert_eq!(html2.matches("variables.css").count(), 1);
        assert_eq!(html2.matches("effects.js").count(), 1);
        assert_eq!(html2.matches("usage.js").count(), 1, "二次注入不应重复 usage.js");
        assert_eq!(html2.matches("pet.js").count(), 1, "二次注入不应重复 pet.js");

        // 引用位置：样式在 </head> 之前、脚本在 </body> 之前
        let head_pos = html2.to_ascii_lowercase().find("</head>").unwrap();
        let vars_pos = html2.find("variables.css").unwrap();
        assert!(vars_pos < head_pos, "样式链接应位于 </head> 之前");
        let body_pos = html2.to_ascii_lowercase().find("</body>").unwrap();
        let js_pos = html2.find("effects.js").unwrap();
        assert!(js_pos < body_pos, "脚本应位于 </body> 之前（defer）");
        let usage_pos = html2.find("usage.js").unwrap();
        assert!(usage_pos < body_pos, "usage.js 应位于 </body> 之前（defer）");
        let pet_pos = html2.find("pet.js").unwrap();
        assert!(pet_pos < body_pos, "pet.js 应位于 </body> 之前（defer）");
        assert!(html2.contains("<script defer src="));
        // pet.js 注入行带 data-zbar-pet 标记（清理与防重复注入定位用）
        let pet_line = html2
            .lines()
            .find(|l| l.contains("pet.js"))
            .expect("pet.js 注入行应存在");
        assert!(pet_line.contains("data-zbar-pet"), "pet.js 行应带标记：{pet_line}");

        // 剥离后应还原为无标记的原貌
        let stripped = strip_inject_blocks(&html2);
        assert!(!has_inject(&stripped));
        assert_eq!(stripped, SAMPLE_HTML);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 注入幂等_历史残留块被清理() {
        // 预置含旧注入块的 html（模拟升级重装场景）
        let dirty = format!(
            "<html><head><title>t</title>{BEGIN}<link rel=\"stylesheet\" href=\"file:///old.css\">{END}</head><body>{BEGIN}<script src=\"file:///old.js\"></script>{END}</body></html>",
            BEGIN = INJECT_BEGIN,
            END = INJECT_END
        );
        let dir = test_dir("legacy");
        let index = dir.join("index.html");
        fs::write(&index, dirty).unwrap();

        let html = apply_inject(&index, &dir).unwrap();
        assert_eq!(html.matches(INJECT_BEGIN).count(), 2);
        assert!(!html.contains("old.css"));
        assert!(!html.contains("old.js"));
        assert!(html.contains("variables.css"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 注入_缺少闭合标签时报中文错误() {
        let dir = test_dir("malformed");
        let index = dir.join("index.html");
        fs::write(&index, "<html><head></head><body>no tail</body>").unwrap();
        // 该 html 缺 </body>？——上面其实含 </body>，去掉它构造异常结构
        fs::write(&index, "<html><head></head><body>broken").unwrap();
        let err = apply_inject(&index, &dir).unwrap_err();
        assert!(err.contains("index.html 结构异常"), "错误信息应说明结构异常：{err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn percent_编码_空格与中文() {
        assert_eq!(
            percent_encode_path("/Users/a b/中文.mp4"),
            "/Users/a%20b/%E4%B8%AD%E6%96%87.mp4"
        );
        // unreserved 与路径分隔符保持原样
        assert_eq!(percent_encode_path("/opt/homebrew/bin/npx-v1.2_3~"), "/opt/homebrew/bin/npx-v1.2_3~");
        // file_url 形态
        let url = file_url(Path::new("/Users/z/我的壁纸.mp4"));
        assert!(url.starts_with("file:///Users/z/"));
        assert!(url.contains("%E6%88%91%E7%9A%84%E5%A3%81%E7%BA%B8.mp4"));
    }

    #[cfg(windows)]
    #[test]
    fn verbatim前缀剥离_盘符UNC与无前缀形态() {
        // 盘符 verbatim：\\?\C:\… → C:\…
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\Users\z\w.mp4")),
            PathBuf::from(r"C:\Users\z\w.mp4")
        );
        // UNC verbatim：\\?\UNC\srv\share → \\srv\share
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\srv\share\w.mp4")),
            PathBuf::from(r"\\srv\share\w.mp4")
        );
        // 无前缀原样返回（普通绝对路径与相对文件名都不受影响）
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"C:\Users\z\w.mp4")),
            PathBuf::from(r"C:\Users\z\w.mp4")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from("a.mp4")),
            PathBuf::from("a.mp4")
        );
    }

    #[cfg(windows)]
    #[test]
    fn file_url_verbatim路径兜底剥前缀() {
        // 历史 params.json 存过 \\?\C:\… 形态，重渲时必须自愈为合法 URL
        let url = file_url(Path::new(r"\\?\C:\Users\z\wp\my wall.mp4"));
        assert_eq!(url, "file:///C%3A/Users/z/wp/my%20wall.mp4");
    }

    #[test]
    fn variables_css_渲染含全部变量与壁纸地址() {
        let params = ThemeParams::default();
        let url = "file:///Users/z/.zbar/agent-themes/zcode/wallpapers/my%20wall.mp4";
        // 宠物三变量改由显式入参渲染（PetConfig 统一收敛后 ThemeParams
        // 不再承载宠物参数；此处模拟注入版开启：cat / 档 3@1080 = 108px）
        let css = render_variables_css(&params, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        for var in [
            "--zbar-wallpaper-url",
            "--zbar-wallpaper-enabled",
            "--zbar-wp-brightness",
            "--zbar-wp-saturate",
            "--zbar-wp-blur",
            "--zbar-mask-strength",
            "--zbar-panel-opacity",
            "--zbar-sidebar-opacity",
            "--zbar-sidebar-right-opacity",
            "--zbar-base-alpha",
            "--zbar-text-shadow",
            "--zbar-usage-font-size",
            "--zbar-usage-opacity",
            "--zbar-usage-session-bar",
            "--zbar-usage-turn-bar",
            "--zbar-usage-quota-bar",
            "--zbar-usage-model-rows",
            "--zbar-pet-enabled",
            "--zbar-pet-style",
            "--zbar-pet-size",
            "--zbar-pet-asset-ver",
            "--zbar-playback-rate",
        ] {
            assert!(css.contains(var), "variables.css 缺少变量 {var}");
        }
        // P1-2：资产内容戳变量按入参渲染（空 = 未物化；壳按缺失处理）
        assert!(css.contains("--zbar-pet-asset-ver: ;"), "{css}");
        let stamped = render_variables_css(&params, url, "0123456789abcdef", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(
            stamped.contains("--zbar-pet-asset-ver: 0123456789abcdef;"),
            "{stamped}"
        );
        // 壁纸地址以 url("…") 形式写入
        assert!(css.contains(&format!("url(\"{url}\")")));
        // 默认参数值渲染（V6 默认：亮度/饱和度拉满、对话列/侧栏/右栏
        // 归零、氛围底默认 0.25、V10 描边默认关闭、速率 1；用量条字号
        // 10px / 不透明度 0.55 与 V3 usage.js 模板写死观感一致）
        assert!(css.contains("--zbar-wp-brightness: 1.1;"));
        assert!(css.contains("--zbar-wp-saturate: 1.4;"));
        assert!(css.contains("--zbar-wp-blur: 0px;"));
        assert!(css.contains("--zbar-mask-strength: 0;"));
        assert!(css.contains("--zbar-panel-opacity: 0;"));
        assert!(css.contains("--zbar-sidebar-opacity: 0;"));
        assert!(css.contains("--zbar-sidebar-right-opacity: 0;"));
        assert!(css.contains("--zbar-base-alpha: 0.25;"));
        assert!(css.contains("--zbar-text-shadow: 0;"));
        assert!(css.contains("--zbar-usage-font-size: 10px;"));
        assert!(css.contains("--zbar-usage-opacity: 0.55;"));
        assert!(css.contains("--zbar-usage-session-bar: 1;"), "{css}");
        assert!(css.contains("--zbar-usage-turn-bar: 1;"), "{css}");
        assert!(css.contains("--zbar-usage-quota-bar: 1;"), "{css}");
        assert!(css.contains("--zbar-usage-model-rows: 1;"), "{css}");
        assert!(css.contains("--zbar-wallpaper-enabled: 1;"), "{css}");
        assert!(css.contains("--zbar-playback-rate: 1;"));
        // 非 ASCII 壁纸名在 url 里必须已编码（不出现裸中文）
        assert!(!css.contains("我的壁纸"));

        // V10：两个新参数按参数渲染真值（不再写死常量），改参后
        // variables.css 跟随变化（热重载即时生效的前提）
        let mut p = ThemeParams::default();
        p.base_alpha = 0.4;
        p.text_shadow = 0.8;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-base-alpha: 0.4;"), "{css}");
        assert!(css.contains("--zbar-text-shadow: 0.8;"), "{css}");

        // V4：用量统计条参数同样按参数渲染真值（皮肤页滑块改参后
        // 随 variables.css 热重载即时生效的前提）
        let mut p = ThemeParams::default();
        p.usage_font_size = 13.0;
        p.usage_opacity = 0.8;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-usage-font-size: 13px;"), "{css}");
        assert!(css.contains("--zbar-usage-opacity: 0.8;"), "{css}");

        // V5：会话累计条开关按参数渲染 1/0（关闭时 usage.js 读到 0
        // 即不渲染会话条，随热重载约 1 秒生效）
        let mut p = ThemeParams::default();
        p.usage_session_bar = false;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-usage-session-bar: 0;"), "{css}");
        p.usage_session_bar = true;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-usage-session-bar: 1;"), "{css}");

        // V19：每轮统计条开关同样按参数渲染 1/0（关闭时 renderAll 对
        // 全部轮节点 removeRow 并跳过 renderOne，随热重载约 1 秒生效）
        let mut p = ThemeParams::default();
        p.usage_turn_bar = false;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-usage-turn-bar: 0;"), "{css}");
        p.usage_turn_bar = true;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-usage-turn-bar: 1;"), "{css}");

        // V28：输入框额度行开关同样按参数渲染 1/0（关闭时 usage.js 移
        // 除独立额度行，随热重载约 1 秒生效）
        let mut p = ThemeParams::default();
        p.usage_quota_bar = false;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-usage-quota-bar: 0;"), "{css}");
        p.usage_quota_bar = true;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-usage-quota-bar: 1;"), "{css}");

        // V29：模型速度行开关同样按参数渲染 1/0（关闭时 usage.js 移除
        // 独立模型行容器，会话条开关不受影响，随热重载约 1 秒生效）
        let mut p = ThemeParams::default();
        p.usage_model_rows = false;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-usage-model-rows: 0;"), "{css}");
        p.usage_model_rows = true;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-usage-model-rows: 1;"), "{css}");

        // 壁纸层总开关按参数渲染 1/0（effects.js V6 读到 0 即移除注入
        // 三层、背景回纯色底；缺失视为开启，随热重载约 1 秒生效）
        let mut p = ThemeParams::default();
        p.wallpaper_enabled = false;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-wallpaper-enabled: 0;"), "{css}");
        p.wallpaper_enabled = true;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-wallpaper-enabled: 1;"), "{css}");

        // 桌面宠物三变量按显式入参渲染真值（PetConfig 统一后 store.rs
        // 传入 pet_enabled/style/size_px）：默认关闭渲染 0（pet.js 读值
        // 非 "1" 一律视为关闭，含变量缺失的旧 variables.css），开启后
        // 形象与大小随配置透出（热重载约 1 秒生效的前提）
        let closed = render_variables_css(&params, url, "", 0, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(closed.contains("--zbar-pet-enabled: 0;"), "{closed}");
        assert!(
            closed.contains(&format!(
                "--zbar-pet-style: {};",
                crate::pet::DEFAULT_PET_STYLE
            )),
            "{closed}"
        );
        assert!(closed.contains("--zbar-pet-size: 108px;"), "{closed}");
        let css = render_variables_css(&params, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 162);
        assert!(css.contains("--zbar-pet-enabled: 1;"), "{css}");
        assert!(
            css.contains(&format!(
                "--zbar-pet-style: {};",
                crate::pet::DEFAULT_PET_STYLE
            )),
            "{css}"
        );
        assert!(css.contains("--zbar-pet-size: 162px;"), "{css}");
        // V3：自定义形象值 custom:<id> 原样透传（冒号为 CSS 自定义属性的
        // 合法值字符，注入壳按前缀识别）
        let css = render_variables_css(&params, url, "", 1, "custom:boba", 108);
        assert!(css.contains("--zbar-pet-style: custom:boba;"), "{css}");
    }

    #[test]
    fn variables_css_壁纸关闭态渲染空转组合且usage保持真值() {
        // 关闭态"主题视觉空转"回归：视觉五变量必须渲染空转组合而非参数
        // 真值（base_alpha 归 1 恢复实色底、三区域不透明度归 0 回透明结
        // 构、描边归 0 不可见），否则关壁纸后 ZCode 停留在半透明中间态；
        // usage 类变量与滤镜/速率不受开关影响，照常渲染参数真值。
        // 参数全部取非默认值——默认 panel/sidebar/right 本就是 0、与空转
        // 值恰好重合，会掩盖"渲染了真值还是空转值"的区分（format! 命名
        // 参数误写 params.* 时默认参数下测试照样绿）；开启态用同参数断言
        // 真值，与关闭态逐项互为对照。
        let url = "file:///x/y.mp4";
        let mut p = ThemeParams::default();
        p.panel_opacity = 0.6;
        p.sidebar_opacity = 0.5;
        p.sidebar_right_opacity = 0.4;
        p.base_alpha = 0.3;
        p.text_shadow = 0.7;
        p.usage_font_size = 13.0;
        p.usage_opacity = 0.8;
        p.usage_session_bar = false;

        p.wallpaper_enabled = false;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-wallpaper-enabled: 0;"), "{css}");
        // V12 壳规则开关随壁纸开关反相（关闭=1）：theme.css 壳容器据此
        // 恢复 --color-background 不透明底，与空转组合叠加还原原始整窗观感
        assert!(css.contains("--zbar-shell-uncover: 1;"), "{css}");
        // 空转组合五项逐项断言（缺一即残留半透明中间态）
        assert!(css.contains("--zbar-panel-opacity: 0;"), "{css}");
        assert!(css.contains("--zbar-sidebar-opacity: 0;"), "{css}");
        assert!(css.contains("--zbar-sidebar-right-opacity: 0;"), "{css}");
        assert!(css.contains("--zbar-base-alpha: 1;"), "{css}");
        assert!(css.contains("--zbar-text-shadow: 0;"), "{css}");
        // usage 类变量不受开关影响，仍渲染参数真值（关壁纸只关视觉皮肤，
        // 统计条照常显示，字号/不透明度/开关也照常可调）
        assert!(css.contains("--zbar-usage-font-size: 13px;"), "{css}");
        assert!(css.contains("--zbar-usage-opacity: 0.8;"), "{css}");
        assert!(css.contains("--zbar-usage-session-bar: 0;"), "{css}");
        // 滤镜/速率照常真值（壁纸层已卸载无消费方，重新开启按原值恢复）
        assert!(css.contains("--zbar-wp-brightness: 1.1;"), "{css}");
        assert!(css.contains("--zbar-playback-rate: 1;"), "{css}");

        // 开启态：同参数原样输出真值
        p.wallpaper_enabled = true;
        let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, 108);
        assert!(css.contains("--zbar-wallpaper-enabled: 1;"), "{css}");
        // V12 壳规则开关反相（开启=0）：壳容器保持全透明、壁纸垫底透出，
        // 与 V11 硬编码 transparent 行为一致
        assert!(css.contains("--zbar-shell-uncover: 0;"), "{css}");
        assert!(css.contains("--zbar-panel-opacity: 0.6;"), "{css}");
        assert!(css.contains("--zbar-sidebar-opacity: 0.5;"), "{css}");
        assert!(css.contains("--zbar-sidebar-right-opacity: 0.4;"), "{css}");
        assert!(css.contains("--zbar-base-alpha: 0.3;"), "{css}");
        assert!(css.contains("--zbar-text-shadow: 0.7;"), "{css}");

        // 空转覆写只发生在渲染处：ThemeParams 本体未被改写，参数真值
        // 原样留在 params.json，重新开启即按原值恢复
        assert_eq!(p.panel_opacity, 0.6);
        assert_eq!(p.base_alpha, 0.3);
        assert_eq!(p.text_shadow, 0.7);
    }

    #[test]
    fn variables_css_pet_size档位换算px断言() {
        // 兼容硬约束回归：--zbar-pet-size 恒为「整数 + px」——旧版 pet.js
        // 的 sizeOf 用 parseInt 解析该值，任何非整数 px 形态（小数、
        // 无单位、vh/百分比）都会把宠物缩成蚂蚁。档位换算已上移到
        // store.rs（PetConfig.size → pet_size_px），此处断言入参 px
        // 原样整数落盘
        let url = "file:///x/y.mp4";
        for screen_h in [800.0, 1080.0, 1440.0, 2160.0] {
            for level in 1..=crate::pet::PET_SIZE_LEVEL_PCT.len() as u32 {
                let p = ThemeParams::default();
                let expect = crate::pet::pet_size_px(level, screen_h);
                let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, expect);
                let want = format!("--zbar-pet-size: {expect}px;");
                assert!(css.contains(&want), "档{level}@{screen_h} 应渲染 {want}：{css}");
                // 值主体必为纯整数（无小数点、无 vh/% 等单位变体）
                let line = css
                    .lines()
                    .find(|l| l.contains("--zbar-pet-size:"))
                    .unwrap();
                let v = line
                    .trim()
                    .trim_start_matches("--zbar-pet-size:")
                    .trim()
                    .trim_end_matches(';');
                assert_eq!(v, format!("{expect}px"), "值形态必须为整数px：{line}");
                assert!(v.trim_end_matches("px").parse::<u32>().is_ok(), "去掉 px 后必须是纯整数：{v}");
            }
        }
        // 屏高非法兜底 1080：默认档 3 → 108px（换算由 pet::pet_size_px
        // 保证，渲染端只消费换算结果）
        for bad in [0.0, f64::NAN, f64::INFINITY] {
            let p = ThemeParams::default();
            let expect = crate::pet::pet_size_px(3, bad);
            let css = render_variables_css(&p, url, "", 1, crate::pet::DEFAULT_PET_STYLE, expect);
            assert!(css.contains("--zbar-pet-size: 108px;"), "{css}");
        }
    }

    #[test]
    fn pet_js_模板契约与帧数据完整性() {
        // 头部版本标记（store::ensure_versioned_template 的升级判据，
        // 与 theme/effects/usage 共用同一提取器）
        assert!(PET_JS.contains("ZBAR-THEME-V9"));
        assert!(
            !PET_JS.contains("ZBAR-THEME-V8"),
            "版本头应已升到 V9（动作语义细分：thinking/walking）"
        );
        // 单源拼接契约：PET_JS = pet-core.js（唯一真相源）+ 注入版宿主壳，
        // 核心段必须完整位于头部（版本标记提取器只看文件头部）
        assert!(
            PET_JS.starts_with(include_str!("../../../public/pet-core.js")),
            "PET_JS 应以 public/pet-core.js 内容开头（include_str! 单源拼接）"
        );
        // 核心工厂契约：ZBarPet 工厂 + 完整实例接口（独立悬浮窗宿主与
        // 注入版宿主共用同一接口；heartbeat 为 V2 心跳契约的可选入口）
        assert!(PET_JS.contains("window.ZBarPet = { create: create }"));
        assert!(PET_JS.contains("ZBarPet.create(document.body"));
        assert!(PET_JS.contains("feed: feed,"));
        assert!(PET_JS.contains("heartbeat: heartbeat,"));
        assert!(PET_JS.contains("setParams: function (params)"));
        assert!(PET_JS.contains("destroy: function ()"));
        // 严格模式 IIFE（风格与 usage.js/effects.js 一致）
        assert!(PET_JS.contains("\"use strict\";"));
        // 参数热重载契约：消费 variables.css 的三个宠物变量
        assert!(PET_JS.contains("\"--zbar-pet-enabled\""));
        assert!(PET_JS.contains("\"--zbar-pet-style\""));
        assert!(PET_JS.contains("\"--zbar-pet-size\""));
        // 开关语义：值非 "1" 一律视为关闭（默认 false，变量缺失的旧
        // variables.css 下不显示宠物）
        assert!(PET_JS.contains("readVar(VAR_ENABLED) === \"1\""));
        // 数据契约：大文件只读 usage.js 重载的全局数据（壳不重载
        // usage-data.js），心跳小文件由壳每 2 秒重载喂 heartbeat——
        // 切出壳段（拼接边界之后）断言职责划分
        assert!(PET_JS.contains("window.__ZBAR_USAGE__"));
        assert!(PET_JS.contains("d.v !== 2"));
        let shell_lo = PET_JS
            .find("ZBAR 宠物注入版宿主壳")
            .expect("注入版宿主壳应存在");
        let shell = &PET_JS[shell_lo..];
        assert!(
            shell.contains("usage-data-hb.js"),
            "壳应重载心跳小文件（存活信号由独立小文件承担）：{shell}"
        );
        assert!(
            shell.contains("pet.heartbeat(window.__ZBAR_USAGE_HB__)"),
            "壳应把心跳全局喂给核心 heartbeat 接口：{shell}"
        );
        assert!(
            !shell.contains("usage-data.js\"") && !shell.contains("usage-data.js`"),
            "壳不应自行重载大文件 usage-data.js（usage.js 的职责）：{shell}"
        );
        // V7 拖拽与位置持久化契约：localStorage 持久化键 + 指针捕获拖拽
        // + 容器重挂（create/setParams 重建后）+ 视口 clamp
        assert!(
            shell.contains("\"__zbarPetPos\""),
            "壳应以 localStorage 键 __zbarPetPos 持久化位置：{shell}"
        );
        assert!(
            shell.contains("setPointerCapture"),
            "拖拽应使用 pointer capture（移出容器仍跟随）：{shell}"
        );
        assert!(
            shell.contains("releasePointerCapture"),
            "拖拽结束应释放指针捕获：{shell}"
        );
        assert!(
            shell.contains("ensurePetContainer"),
            "容器重挂入口应存在（开关开合/形象切换重建容器后重绑拖拽）：{shell}"
        );
        assert!(
            shell.contains("clampPetIntoView"),
            "每 tick 的视口 clamp 应存在（resize 后拉回可见区）：{shell}"
        );
        // 拖拽状态防卡死契约（P1-2）：容器重建时同步复位 dragging +
        // window 级 pointerup/cancel 兜底（容器 detach/捕获失败时自救）
        assert!(
            shell.contains("dragging = false") && shell.contains("dragEl = null"),
            "容器重挂与拖拽收尾应复位 dragging/dragEl：{shell}"
        );
        assert!(
            shell.contains("window.addEventListener(\"pointerup\"")
                && shell.contains("window.addEventListener(\"pointercancel\""),
            "window 级拖拽兜底监听应存在（pointerup/cancel 双通道）：{shell}"
        );
        assert!(
            shell.contains("globalDragEnd"),
            "兜底处理器应具名存在且幂等（无拖拽时空转）：{shell}"
        );
        assert!(
            shell.contains("div[data-zbar-pet]"),
            "定位样式选择器应精确到 div（壳注入行 script 同带 data-zbar-pet 属性）：{shell}"
        );
        assert!(
            !shell.contains("pointer-events:none"),
            "V7 容器需接收鼠标事件，不应再写 pointer-events:none：{shell}"
        );
        // 核心（壳之前的公共段）：心跳回退 ts（旧数据文件 V1 兼容）+
        // 闲置判定消费 la（缺失回退 ts，防窗口滑动误弹闲置）
        let core = &PET_JS[..shell_lo];
        assert!(
            core.contains("if (!hbSeen) lastHb = d.ts;"),
            "核心应在心跳缺失时回退 ts 判陈旧（V1 兼容）：{core}"
        );
        assert!(
            core.contains("typeof d.la === \"number\" ? d.la : d.ts"),
            "核心应消费 la 判闲置（缺失回退 ts）：{core}"
        );
        assert!(
            core.contains("now - s.lastActivity < IDLE_SLEEP_MS"),
            "闲置判定应按最后活动时刻 la 计算（V5 纯函数化后经侧写字段消费）：{core}"
        );
        // 状态机九状态（V6：五状态 + tool_running/failed；V9：细分
        // thinking/walking，working 降为缺键回退目标）与关键判定常量
        // （心跳新鲜度 / 闲置入睡 / 庆祝时长）
        for state in [
            "sleeping",
            "idle",
            "working",
            "typing",
            "celebrating",
            "tool_running",
            "failed",
            "thinking",
            "walking",
        ] {
            assert!(PET_JS.contains(&format!("\"{state}\"")), "缺状态 {state}");
        }
        assert!(PET_JS.contains("DATA_FRESH_MS = 10000"));
        assert!(PET_JS.contains("IDLE_SLEEP_MS = 60000"));
        assert!(PET_JS.contains("CELEBRATE_MS = 3000"));
        // V5 新常量：pu 预判窗口（实测首请求 30~70s + 余量）与 working
        // 迟滞窗口（抹平轮间隙回落感）
        assert!(PET_JS.contains("PENDING_TURN_MS = 90000"));
        assert!(PET_JS.contains("WORKING_LINGER_MS = 45000"));
        // V6 新常量：ta 工具活跃窗口（30s 兜轮询间隙与异常残留，正常
        // 结束 ta 立即归 null）与 fe 沮丧时长（与 CELEBRATE_MS 同款口径）
        assert!(PET_JS.contains("TOOL_ACTIVE_MS = 30000"));
        assert!(PET_JS.contains("FAILED_MS = 3000"));
        // V5 pu 契约：核心消费 d.pu（缺失与 null 归 0，向后兼容旧数据
        // 文件），预判按 PENDING_TURN_MS 窗口判定；迟滞基准 lastWorkT
        // 只随真实工作信号前进（迟滞自身不回写，防窗口无限续期）
        assert!(
            core.contains("typeof d.pu === \"number\""),
            "核心应消费 pu 字段（缺失兼容）：{core}"
        );
        assert!(
            core.contains("now - pendingUser < PENDING_TURN_MS"),
            "预判应按 PENDING_TURN_MS 窗口判定：{core}"
        );
        assert!(
            core.contains("now - s.lastWorkT < WORKING_LINGER_MS"),
            "迟滞应按 WORKING_LINGER_MS 窗口保持动身观感（V9 输出 walking）：{core}"
        );
        assert!(
            core.contains("if (s.pending) {"),
            "预判命中的优先级应在 celebrating 之后、idle/sleeping 之前：{core}"
        );
        assert!(
            core.contains("function decideState"),
            "状态机判定应为模块级纯函数 decideState（单测直测）：{core}"
        );
        // V6 ta/fe 契约：核心消费 d.ta / d.fe（缺失与 null 归 0，向后兼容
        // 旧数据文件）；ta 按 TOOL_ACTIVE_MS 窗口判定 tool_active；fe 驱动
        // 沮丧窗口并与庆祝成败互斥（feed 的轮完成分支按 fe 新鲜度二选一）
        assert!(
            core.contains("typeof d.ta === \"number\""),
            "核心应消费 ta 字段（缺失兼容）：{core}"
        );
        assert!(
            core.contains("typeof d.fe === \"number\""),
            "核心应消费 fe 字段（缺失兼容）：{core}"
        );
        assert!(
            core.contains("now - toolActiveAt < TOOL_ACTIVE_MS"),
            "tool_active 应按 TOOL_ACTIVE_MS 窗口判定：{core}"
        );
        assert!(
            core.contains("if (s.toolActive) return \"tool_running\";"),
            "工具执行期应优先于 typing/working 输出 tool_running：{core}"
        );
        assert!(
            core.contains("if (s.failedUntil > now) return \"failed\";"),
            "沮丧窗口内应输出 failed（优先级最高）：{core}"
        );
        assert!(
            core.contains("failedUntil = now + FAILED_MS;"),
            "轮完成且 fe 新鲜时应置位沮丧截止时刻：{core}"
        );
        // V6 状态回退契约（V8 起仅剩自定义形象的缺键回退）：旧五状态
        // pet.json 缺 tool_running/failed 键时回退相近状态帧
        //（tool_running → typing 忙碌执行；failed → sleeping 蔫了），
        // 渲染路径统一经 customStateDef 消费 CUSTOM_STATE_FALLBACK；
        // V9 细分键缺键（未随细分映射升级的形象）回退 working 帧
        //（thinking/walking 的场景来源即原 working，观感与 V8 一致）；
        // 内建形象的 BUILTIN_STATE_FALLBACK / builtinRenderState 已随
        // V8 字符网格形象移除（负向断言防回归）
        assert!(
            core.contains("tool_running: \"typing\""),
            "缺键回退应将 tool_running 映射到 typing 帧：{core}"
        );
        assert!(
            core.contains("failed: \"sleeping\""),
            "缺键回退应将 failed 映射到 sleeping 帧：{core}"
        );
        assert!(
            core.contains("thinking: \"working\""),
            "缺键回退应将 thinking 映射到 working 帧（V9）：{core}"
        );
        assert!(
            core.contains("walking: \"working\""),
            "缺键回退应将 walking 映射到 working 帧（V9）：{core}"
        );
        assert!(
            !core.contains("builtinRenderState"),
            "内建渲染回退函数应已移除（V8 无字符网格形象）：{core}"
        );
        // V6 迟滞跨工作态：lastWorkT 随 tool_running 前进（迟滞覆盖
        // 「工作类状态 → 非工作状态」的过渡）
        assert!(
            core.contains("st === \"tool_running\""),
            "迟滞基准 lastWorkT 应覆盖 tool_running（V6 工作类状态）：{core}"
        );
        // V4 动画驱动契约：不依赖 rAF（ZCode 注入环境视觉可见但无
        // beginFrame，rAF 全暂停 → 冻结首帧），setInterval 轮询驱动
        // （帧推进语义不变）；应用层不做 document.hidden 省电判断
        // （visibility 判定与视觉可见性脱节时降频会造成同样的冻结/
        // 状态滞后）。负向断言带调用/三元特征，头部 V4 变更注释中的
        // 说明字样不受影响
        assert!(
            PET_JS.contains("setInterval(loop"),
            "核心动画应由 setInterval 轮询驱动（V4）：{core}"
        );
        assert!(
            !PET_JS.contains("requestAnimationFrame("),
            "宠物脚本不应调用 rAF（注入环境 beginFrame 缺失会冻结）"
        );
        assert!(
            !PET_JS.contains("cancelAnimationFrame("),
            "宠物脚本不应残留 rAF 取消调用（V4 已无 rAF 生命周期）"
        );
        assert!(
            !PET_JS.contains("document.hidden ?"),
            "宠物脚本不应做 hidden 降频判断（visibility 与视觉可见性脱节）"
        );
        // 形象库：V8 起内建字符网格形象（cat/bot）与 PET_STYLES 常量已
        // 整体移除——全部形象统一走 Petdex 图集渲染器（负向断言防回归；
        // 帧数据的合法性由 pets.rs 的内嵌资产单测把关）
        assert!(
            !PET_JS.contains("var PET_STYLES"),
            "字符网格形象库应已移除（V8 渲染收敛 customAsset-only）"
        );
        assert!(!PET_JS.contains("cat: {"));
        assert!(!PET_JS.contains("bot: {"));
        // 渲染契约：防重复挂载标记 + 像素风放大（V7 起容器可拖拽、
        // 需接收鼠标事件，不再要求 pointer-events 遮蔽样式）
        assert!(PET_JS.contains("\"data-zbar-pet\""));
        assert!(PET_JS.contains("image-rendering:pixelated"));
        // V3 自定义形象契约（Petdex 图集渲染器）：style 前缀 custom: +
        // customAsset 资产 + drawImage 按网格切帧（最近邻采样保锐利）
        // + 缺资产进入空态（V8 起无内建回退）
        assert!(
            PET_JS.contains("CUSTOM_PREFIX = \"custom:\""),
            "核心应识别 custom:<id> 形象前缀"
        );
        assert!(PET_JS.contains("customAsset"), "create/setParams 应支持 customAsset");
        assert!(PET_JS.contains("imageSmoothingEnabled = false"));
        assert!(PET_JS.contains("ctx.drawImage("));
        assert!(
            PET_JS.contains("function effectiveStyleOf"),
            "custom 缺资产应进入空态不渲染（effectiveStyleOf）"
        );
        // 注入壳（V3）：custom 选中时按需加载同目录物化文件
        // pet-custom.js（静态资产不轮询），按 meta.id 比对选中 id
        let shell_v3 = &PET_JS[PET_JS
            .find("ZBAR 宠物注入版宿主壳")
            .expect("注入版宿主壳应存在")..];
        assert!(
            shell_v3.contains("window.__ZBAR_PET_CUSTOM__"),
            "壳应消费物化资产全局：{shell_v3}"
        );
        assert!(
            shell_v3.contains("\"pet-custom.js\""),
            "壳应加载同目录 pet-custom.js：{shell_v3}"
        );
        assert!(
            shell_v3.contains("function customAssetFor"),
            "壳应按选中 id 校验资产归属"
        );
        assert!(
            shell_v3.contains("function reloadCustomAsset"),
            "壳应支持资产缺失时懒加载一次（不轮询）"
        );
        assert!(
            shell_v3.contains("customAsset: asset"),
            "壳应把资产传给核心 create/setParams"
        );
        // P1-2：资产内容戳热刷新——壳读 --zbar-pet-asset-ver，值变化强制
        // 重载资产，重载后经 curAsset 身份比对把新对象传给核心重建
        assert!(
            shell_v3.contains("VAR_ASSET_VER = \"--zbar-pet-asset-ver\""),
            "壳应读资产内容戳变量：{shell_v3}"
        );
        assert!(
            shell_v3.contains("ver !== assetVer"),
            "壳应按内容戳变化触发重载"
        );
        assert!(
            shell_v3.contains("curAsset !== asset"),
            "壳应按资产对象身份变化触发核心重建（重复导入同 id）"
        );
        // V8 壳无内建回退契约：styleIdOf/effective 的回退值为空串（资产
        // 未就位 → 核心空态暂隐），壳全文不应再出现 "cat" 回退字面量——
        // 升级窗口期（新版壳 + 旧 variables.css 的 cat 残留值）表现为
        // 宠物暂隐而非闪猫脸
        assert!(
            !shell_v3.contains("\"cat\""),
            "壳不应再回退内建 cat 形象（V8 无内建渲染路径）：{shell_v3}"
        );
        assert!(
            shell_v3.contains("var effective = asset ? want : \"\";"),
            "壳的 effective 回退应为空串（资产未就位 = 空态暂隐）：{shell_v3}"
        );
    }

    #[test]
    fn usage_js_v34_分段状态条与设计语言() {
        // ---- 版本标记（V44 起头部升级，V34 特征除溢出隐藏外保留）----
        assert!(USAGE_JS.contains("ZBAR-THEME-V47"));
        assert!(!USAGE_JS.contains("ZBAR-THEME-V45"), "版本头应已升到 V47");
        // ---- 字体跟随客户端（任务红线：--font-mono 引用）：四个注入层
        // （每轮条/会话条/模型底板/底部条）各一条 font-family 声明，缺失
        // 回退既有等宽栈
        assert_eq!(
            USAGE_JS
                .matches("font-family:var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace);")
                .count(),
            4,
            "每轮条/会话条/模型底板/底部条应各消费一次 --font-mono"
        );
        // ---- 颜色跟随客户端语义变量 + 兜底（速度/额度三档语义色保留硬编码）----
        assert!(USAGE_JS.contains("var(--color-foreground-subtle, rgba(120,130,145,.95))"));
        assert!(USAGE_JS.contains("var(--color-border, rgba(127,127,127,.22))"));
        assert!(USAGE_JS.contains("var(--color-card, rgba(28,30,36,.92))"));
        assert!(
            USAGE_JS.contains("background:var(--color-card, rgba(2,6,23,.72))")
                && USAGE_JS
                    .contains("border:1px solid var(--color-border, rgba(148,163,184,.28))"),
            "模型底板背景/边框应对齐客户端 tokens（回退原深底/原边框）"
        );
        assert!(
            !USAGE_JS.contains("prefers-color-scheme: light"),
            "底板配色改由客户端语义变量接管，V26 亮色媒体查询应删除"
        );
        // ---- 胶囊容器（任务红线：backdrop-filter 存在）：blur+saturate
        // 带前缀、内高光+双层投影、圆角、V44 横向滚动容器 + V45 细滚动条 ----
        assert!(
            USAGE_JS
                .contains("-webkit-backdrop-filter:blur(12px) saturate(1.2);backdrop-filter:blur(12px) saturate(1.2);"),
            "胶囊应带前缀的 blur(12px) saturate(1.2) 毛玻璃"
        );
        assert!(
            USAGE_JS.contains(
                "box-shadow:inset 0 1px 0 rgba(255,255,255,.05), 0 8px 22px rgba(0,0,0,.26), 0 1px 3px rgba(0,0,0,.18);"
            ),
            "胶囊应有内高光 + 双层投影"
        );
        assert!(
            USAGE_JS.contains("white-space:nowrap;overflow-x:auto;overflow-y:hidden;pointer-events:auto;")
                && USAGE_JS
                    .contains(".zbar-usage-qb-line::-webkit-scrollbar{height:4px;}")
                && USAGE_JS.contains(
                    ".zbar-usage-qb-line::-webkit-scrollbar-thumb{background:rgba(127,127,127,.4);border-radius:2px;}",
                )
                && USAGE_JS.contains(".zbar-usage-qb-line::-webkit-scrollbar-track{background:transparent;}")
                && USAGE_JS.contains("scrollbar-width:auto;scrollbar-color:auto;")
                && !USAGE_JS.contains("::-webkit-scrollbar{display:none;}")
                && USAGE_JS.contains("justify-content:flex-start;")
                && USAGE_JS.contains("flex:none;margin:0 auto;"),
            "V45 胶囊滚动条显式矮化：overflow-x:auto + 4px 细条 + 标准属性重置（flex-start + qfull margin:0 auto 兼顾不溢出时居中与溢出时可完整滚动）"
        );
        assert!(USAGE_JS.contains("border-radius:10px;"), "状态条圆角 10px");
        assert!(
            USAGE_JS.contains("width:100%;max-width:100%;")
                && USAGE_JS.contains("height:max(32px, calc(var(--zbar-usage-font-size,10px) * 2.3));")
                && !USAGE_JS.contains("zbar-usage-qb-line:hover")
                && !USAGE_JS.contains("zbar-usage-qmini"),
            "V43 底栏恢复全宽常显：与卡片同宽、32px 高，不再有窄态/悬浮抽屉"
        );
        assert!(
            USAGE_JS.contains("zbar-usage-qfull")
                && !USAGE_JS.contains("zbar-usage-qmini-dot"),
            "V41 只保留完整分段容器"
        );
        // ---- 文字三级层级（亮度+字重，不靠字号）----
        assert!(USAGE_JS.contains(".zbar-usage-dim{opacity:.66;}"));
        assert!(USAGE_JS.contains(".zbar-usage-strong{font-weight:600;}"));
        assert!(USAGE_JS.contains(".zbar-usage-qval{font-weight:600;}"));
        assert!(
            USAGE_JS.contains(".zbar-usage-qdim{color:var(--color-foreground-subtle")
                && USAGE_JS.contains(".zbar-usage-qfaint{color:var(--color-foreground-subtle"),
            "底部条标签/单位弱色两级（qdim/qfaint）应消费 --color-foreground-subtle"
        );
        // 每轮条速度三档色 scoped 规则（模型行 scoped 规则保留并存）
        assert!(
            USAGE_JS.contains("\"[\" + ATTR_ROW + \"] .zbar-usage-tps-fast{color:#34d399;}\"")
                && USAGE_JS.contains("\"[\" + ATTR_ROW + \"] .zbar-usage-tps-mid{color:#fbbf24;}\"")
                && USAGE_JS.contains("\"[\" + ATTR_ROW + \"] .zbar-usage-tps-slow{color:#f87171;}\"")
                && USAGE_JS
                    .contains("\"[\" + ATTR_QUOTA_BAR + \"] .zbar-usage-tps-fast{color:#34d399;}\""),
            "每轮条/底部条速度三档色应与模型行同阈值同色系"
        );
        // ---- 分段清单与行内 SVG 图标（createElementNS，无 innerHTML；
        // V46 额度 5h/每周段去文字标签后补 battery/repeat 图标）----
        for ico in [
            "bolt", "hourglass", "bubble", "wrench", "calendar", "branch",
            "battery", "repeat", "gauge",
        ] {
            assert!(
                USAGE_JS.contains(&format!("ico: \"{ico}\"")),
                "分段图标 {ico} 应在 renderQuotaBar 分段清单中"
            );
            assert!(
                USAGE_JS.contains(&format!("{}: [", ico)),
                "分段图标 {ico} 应在 ICON_PATHS 定义"
            );
        }
        assert!(
            USAGE_JS.contains("var ICON_PATHS = {")
                && USAGE_JS.contains("function makeIcon(name)")
                && USAGE_JS.contains("document.createElementNS(\"http://www.w3.org/2000/svg\", \"svg\")")
                && USAGE_JS.contains("svg.setAttribute(\"class\", \"zbar-usage-qico\");")
                && USAGE_JS.contains("stroke-width:1.7;")
                && USAGE_JS.contains("stroke:currentColor;"),
            "图标应经 createElementNS 构建（stroke=currentColor、线宽 1.7、fill none）"
        );
        // ---- 取值链路 helper（数据全部来自既有全局状态）----
        assert!(
            USAGE_JS.contains("function activeRunOf(sessId, activeMap)")
                && USAGE_JS.contains("function latestTurnOf(sessId)")
                && USAGE_JS.contains("function latestSpeedOf(sessId, activeMap)"),
            "速度/本轮分段取值应集中 helper（活动轮 runs 行优先，空闲回退最新完成轮）"
        );
        assert!(
            USAGE_JS.contains("var spd = latestSpeedOf(sessId, activeMap);")
                && USAGE_JS.contains("var tr = activeRunOf(sessId, activeMap);")
                && USAGE_JS.contains("var tt = tr ? null : (sessId ? latestTurnOf(sessId) : null);"),
            "速度/本轮分段应按 activeRunOf → latestTurnOf 优先级取数"
        );
        // ---- 任务红线：today / tl 消费 ----
        assert!(
            USAGE_JS.contains("lastToday = (data && data.today) || null;"),
            "rebuildIndex 应捕获顶层 today 键（V33 新消费字段，缺失整段隐藏）"
        );
        assert!(
            USAGE_JS.contains("typeof sv.tl === \"number\" && isFinite(sv.tl)"),
            "工具分段应消费 sess[].tl（V33 新消费字段，缺失整段隐藏）"
        );
        assert!(
            USAGE_JS.contains("lastToday && typeof lastToday.tt === \"number\" && isFinite(lastToday.tt)")
                && USAGE_JS.contains("(lastToday.rq || 0) + \" \" + W.req")
                && !USAGE_JS.contains("\" \" + (lastToday.rq || 0)"),
            "今日分段应消费 today.tt / today.rq（V34 起数值与 req 间距由 gap 承担、文本无前导空格）"
        );
        // ---- 子代理分段：Σ turns[].sub.n + psess 呼吸蓝点 ----
        assert!(
            USAGE_JS.contains("if (st && st.sess === sessId && st.sub && st.sub.n) subN += st.sub.n;"),
            "子代理分段应累计当前会话 turns[].sub.n"
        );
        assert!(
            USAGE_JS.contains("if (rr && rr.psess === sessId) {"),
            "呼吸蓝点应按任一 runs 行 psess 命中当前会话判定"
        );
        assert!(
            USAGE_JS.contains("dot: subRunning,")
                && USAGE_JS.contains("dot.className = \"zbar-usage-qdot\";")
                && USAGE_JS.contains("@keyframes zbarUsagePulse{0%,100%{opacity:.25;}50%{opacity:1;}}")
                && USAGE_JS.contains("@media (prefers-reduced-motion: reduce){"),
            "呼吸蓝点应为 2px opacity 脉动且在 prefers-reduced-motion 下关闭"
        );
        // ---- 签名比对（底部条；每轮条/会话条保持全串比对）----
        assert!(
            USAGE_JS.contains("data-zbar-usage-qsig"),
            "底部条应以 data-zbar-usage-qsig 签名比对（分段携带 SVG 无法进 textContent 比对）"
        );
        assert!(
            USAGE_JS.contains("function renderRowParts(row, parts)")
                && USAGE_JS.contains("if (row.textContent === text) return;")
                && USAGE_JS.contains("function partsTextOf(parts)")
                && USAGE_JS.contains("function segsTextOf(segs)"),
            "每轮条/会话条应保持全串比对零 DOM 机制（比对串=字段段文本拼接）"
        );
        // ---- 词表新词条（zh/en 双语；V46 去文字标签后标签词删除，
        // turn（title 用）/req/title 词条保留）----
        assert!(
            USAGE_JS.contains("turn: \"本轮\"")
                && USAGE_JS.contains("req: \"req\"")
                && USAGE_JS.contains("speedTitle: \"最近一笔请求的生成速度；≥70 绿 / ≥40 黄 / <40 红（t/s）\"")
                && USAGE_JS.contains("turn: \"turn\"")
                && USAGE_JS.contains("todayTitle: \"All sessions today (local calendar day)\""),
            "WORDS 应含底部条分段与 title 词条（zh/en 双语）"
        );
        // V46 防回退：失去消费方的标签词条应整体删除（zh/en 两侧均无残留）
        assert!(
            !USAGE_JS.contains("sess: \"")
                && !USAGE_JS.contains("tools: \"")
                && !USAGE_JS.contains("today: \"")
                && !USAGE_JS.contains("subs: \"")
                && !USAGE_JS.contains("quotaLead: \"")
                && !USAGE_JS.contains("quotaWk: \"")
                && !USAGE_JS.contains("cachePct: \""),
            "V46 去文字标签后失去消费方的词条应删除"
        );
        // ---- 红线复核：定位/挂载/闸门/轮询机制零改动 ----
        assert!(
            USAGE_JS.contains("var QUOTA_BAR_BOTTOM_PX = -13")
                && USAGE_JS.contains(
                    "position:absolute;bottom:\" + QUOTA_BAR_BOTTOM_PX + \"px;left:0;right:0;"
                )
                && !USAGE_JS.contains("function loadAgents()")
                && !USAGE_JS.contains("__ZBAR_AGENTS__"),
            "V43 底部条容器定位（负 bottom 贴近软件底部 + 左右 0），不再加载多 Agent 额度旁路"
        );
        assert!(
            USAGE_JS.contains("var QUOTA_POLL_MS = 30000") && USAGE_JS.contains("function loadQuota"),
            "额度 30s 低频重载节拍零改动"
        );
        assert!(
            USAGE_JS.contains("var sessHasData = currentSessionHasData(anchor, activeMap);")
                && USAGE_JS.contains("if (!sessHasData || !quotaBarEnabled()) {"),
            "空会话会话级闸 + 开关闸（整条开关语义）零改动"
        );
        // ---- V44：溢出适配改横向滚动 + 滚轮（V34 优先级整段隐藏退役）----
        // 退役负向断言：隐藏机制的代码形态全部消失（头部 V34 变更注释
        // 是历史记录保留，故只断言代码级特征串）
        assert!(
            !USAGE_JS.contains("function fitQuotaSegments")
                && !USAGE_JS.contains("var quotaFitHidden")
                && !USAGE_JS.contains("segs[hqi].h = ")
                && !USAGE_JS.contains("qseg-off{display:none;}")
                && !USAGE_JS.contains("setAttribute(\"data-prio\"")
                && !USAGE_JS.contains("prio: "),
            "V34 优先级隐藏机制应整体退役（fitQuotaSegments 函数/quotaFitHidden 状态/seg.h 隐藏位/qseg-off 规则/data-prio 标记/prio 字段）"
        );
        assert!(
            USAGE_JS.contains("function onQuotaWheel(e)")
                && USAGE_JS.contains("if (to === line.scrollLeft) return;")
                && USAGE_JS.contains("e.preventDefault();")
                && USAGE_JS.contains("var d = Math.abs(e.deltaX) >= Math.abs(e.deltaY) ? e.deltaX : e.deltaY;"),
            "V44 滚轮横向滚动：deltaX/deltaY 转横向，仅在存在溢出且未到边界时 preventDefault（滚到头放行页面纵向滚动）"
        );
        assert!(
            USAGE_JS.contains("function syncQuotaFade(bar)")
                && USAGE_JS.contains("function syncQuotaFadeLine(line)")
                && USAGE_JS.contains("zbar-usage-qfade-l")
                && USAGE_JS.contains("zbar-usage-qfade-r")
                && USAGE_JS.contains("-webkit-mask-image:linear-gradient(to left, transparent, #000")
                && USAGE_JS.contains("mask-image:linear-gradient(to right, transparent, #000"),
            "V44 左右缘渐隐：滚动状态 class 切换 + mask 渐隐挂 qb-line 本体（不拦指针、不裁可滚动内容）"
        );
        assert!(
            USAGE_JS.contains("function quotaSigOf(segs)")
                && USAGE_JS.contains(
                    "renderQuotaLine(bar, quotaSigOf(segs), segs);\n    syncQuotaFade(bar);"
                )
                && !USAGE_JS.contains("? \"H\" : \"V\""),
            "签名机制保留（内容不变零 DOM），但不再含隐藏位；渲染拍后同步渐隐状态（每拍复测，值变才写）"
        );
        // ---- V34 P1-2：模型行浅色对比度（客户端前景变量 + 中灰兜底）----
        assert!(
            USAGE_JS.contains("color:var(--color-foreground, #5b6577);")
                && USAGE_JS.matches("color:var(--color-foreground-subtle, #5b6577);").count() >= 2,
            "模型行文字应消费 --color-foreground/-subtle（容器 + munit/mextra），兜底中灰不再用深色主题专用浅灰"
        );
        assert!(
            !USAGE_JS.contains("rgba(241,245,249,.92)"),
            "模型行恒浅灰文字应删除（浅色卡片上不可见）"
        );
        // ---- V34 P1-3：会话条速度三档色 scoped 规则 ----
        assert!(
            USAGE_JS
                .contains("\"[\" + ATTR_SESSION_BAR + \"] .zbar-usage-tps-fast{color:#34d399;}\"")
                && USAGE_JS
                    .contains("\"[\" + ATTR_SESSION_BAR + \"] .zbar-usage-tps-mid{color:#fbbf24;}\"")
                && USAGE_JS
                    .contains("\"[\" + ATTR_SESSION_BAR + \"] .zbar-usage-tps-slow{color:#f87171;}\""),
            "会话条速度值应有三档色 scoped 规则（与每轮条/底部条/模型行一致）"
        );
        // ---- V34 P2-4/P2-5：额度/缓存 title + 段内间距 ----
        assert!(
            USAGE_JS.contains("quota5hTitle: \"5 小时窗口剩余；<30% 黄 / <10% 红\"")
                && USAGE_JS.contains("quotaWkTitle: \"每周窗口剩余；<30% 黄 / <10% 红\"")
                && USAGE_JS
                    .contains("cacheTitle: \"当前会话缓存命中率 = 缓存读 ÷（缓存读 + 非缓存输入）\""),
            "额度/缓存段口径说明应入词表并挂到分段 title"
        );
        assert!(
            USAGE_JS.contains("\"position:relative;display:flex;align-items:center;flex:none;\"")
                && USAGE_JS
                    .contains("gap:calc(var(--zbar-usage-font-size,10px) * .34);white-space:nowrap;")
                && USAGE_JS
                    .contains("padding:0 calc(var(--zbar-usage-font-size,10px) * .36);")
                && USAGE_JS.contains("pointer-events:auto;cursor:default;border-radius:8px;}"),
            "V40 分段间距/padding 随字号缩放，分隔线由相邻兄弟承担"
        );
        assert!(
            USAGE_JS.contains("function fmtTokensShort(n)")
                && USAGE_JS.contains("fmtTokensShort(tIn2 + tOut2)")
                && USAGE_JS.contains("fmtTokensShort(sTt)")
                && USAGE_JS.contains("fmtTokensShort(lastToday.tt)"),
            "底部条数值应走 fmtTokensShort（去等宽补位空格，段内间距由 gap 承担）"
        );
        assert!(
            !USAGE_JS.contains("{ t: W.turn,")
                && !USAGE_JS.contains("{ t: W.sess,")
                && !USAGE_JS.contains("{ t: W.tools,")
                && !USAGE_JS.contains("{ t: W.today,")
                && !USAGE_JS.contains("{ t: W.subs,"),
            "V46 去文字标签：底部条分段 parts 不应再含中文标签 part（悬浮 title 承担口径说明）"
        );
        assert!(
            !USAGE_JS.contains("W.cachePct")
                && !USAGE_JS.contains("W.quotaLead")
                && !USAGE_JS.contains("W.quotaWk +"),
            "V46 去文字标签：额度前导/每周/缓存标签词条应零消费（W.quotaWkTitle 保留，故按消费形态匹配防回退）"
        );
    }

    #[test]
    fn usage_js_v35_卡片对齐与缓存率() {
        // ---- 版本标记 ----
        assert!(USAGE_JS.contains("ZBAR-THEME-V47"));
        assert!(!USAGE_JS.contains("ZBAR-THEME-V45"), "版本头应已升到 V47");
        // ---- 任务 A：输入框卡片定位（零选择器假设 + V37R 语义锚点）----
        assert!(
            USAGE_JS.contains("function locateComposerCard(region)")
                && USAGE_JS.contains("indexOf(\"data-zbar-usage-\") === 0")
                && USAGE_JS.contains("el.offsetWidth > 100")
                && USAGE_JS
                    .contains("region.querySelector(\"textarea, [contenteditable=true]\")")
                && USAGE_JS.contains("anc.offsetWidth < rw - 4"),
            "V35 locateComposerCard 直接子元素扫描应保留；V37R 增语义锚点路径（textarea/contenteditable 向上取最外层非满宽可见祖先——实机卡片非直接子元素/前面有满宽兄弟时首中即用取错）"
        );
        // 对齐写入口 + 回退清理 helper
        assert!(
            USAGE_JS.contains("function applyCardAlign(bar, region, withWidth)")
                && USAGE_JS.contains("function clearCardAlign(bar)"),
            "V35 应有对齐写入口（值变才写）与回退清理 helper"
        );
        // V37R 坐标系修订：锚点路径可命中 region 深层元素（offsetParent
        // 是中间定位元素而非 region，offsetLeft 不再同坐标系），对齐改
        // rect 相对换算；保留卡片水平范围越界防御（offsetParent ===
        // region 一致性防御的等价替代）
        assert!(
            USAGE_JS.contains("var cr = card.getBoundingClientRect();")
                && USAGE_JS.contains("var rr = region.getBoundingClientRect();")
                && USAGE_JS
                    .contains("if (cr.left < rr.left - 1 || cr.right > rr.right + 1) return false;"),
            "对齐应改 rect 相对换算并保留卡片水平越界防御（V37R 深层卡片坐标系）"
        );
        // 三个注入层挂载块接线：会话条自然宽（不带宽度），底部条与模
        // 型行底板带宽度；各命中/兜底分支都有对齐或清理
        assert_eq!(
            USAGE_JS
                .matches("if (!applyCardAlign(bar, region, false)) clearCardAlign(bar);")
                .count(),
            1,
            "会话条应对齐卡片左缘（自然宽，不带宽度）"
        );
        assert_eq!(
            USAGE_JS
                .matches("if (!applyCardAlign(bar, region, true)) clearCardAlign(bar);")
                .count(),
            1,
            "V41 底部状态条恢复写卡片宽度（与对话框同宽）"
        );
        assert_eq!(
            USAGE_JS
                .matches("if (!applyCardAlign(box, region, true)) clearCardAlign(box);")
                .count(),
            1,
            "模型行底板仍对齐卡片"
        );
        assert_eq!(
            USAGE_JS.matches("clearCardAlign(bar);").count()
                + USAGE_JS.matches("clearCardAlign(box);").count(),
            6,
            "三个注入层的命中回退与兜底 fixed 分支都应接线（2×3 处清理）"
        );
        // 红线：不引入 resize 监听（窗口缩放由下一渲染拍/兜底拍自愈）
        assert!(
            !USAGE_JS.contains("addEventListener(\"resize\""),
            "V35 不做 resize 监听（沿用文件既有哲学）"
        );
        // ---- 任务 B：每轮条/会话条缓存命中率段 + ✚0 隐藏 ----
        // 每轮条：barLineOf 条件段——命中率分母 0 与 cw 0 整段不入行
        //（不 push 即不进全串比对串），✚ 段带词表 title
        let blo = USAGE_JS.find("function barLineOf").expect("barLineOf 应存在");
        let bhi = USAGE_JS.find("function lineOf").expect("lineOf 应存在");
        let bar_body = &USAGE_JS[blo..bhi];
        assert!(
            bar_body.contains("var hit = cacheHitText(v.cr, v.inp);")
                && bar_body.contains("if (hit !== \"—\") {")
                && bar_body.contains("parts.push({ t: \" (\" + hit + \")\", c: CLS_DIM });"),
            "每轮条应在 ⟲ 段后追加弱色命中率段 (NN%)（cacheHitText 同口径，分母 0 隐藏）: {bar_body}"
        );
        assert!(
            bar_body.contains("if (v.cw > 0) {"),
            "每轮条 ✚ 缓存写段应在 cw 为 0 时整段隐藏（含启动窗口占位态）: {bar_body}"
        );
        // 会话条：⟲ 段 parts 化补 (NN%)（sess 口径与底部条「缓存」段同
        // 源）、✚ 段条件化带 title
        let slo = USAGE_JS
            .find("function renderSessionBar")
            .expect("renderSessionBar 应存在");
        let shi = USAGE_JS
            .find("function renderBarLine")
            .expect("renderBarLine 应存在");
        let sess_body = &USAGE_JS[slo..shi];
        assert!(
            sess_body.contains("var sessHit = cacheHitText(tcr, tin);")
                && sess_body
                    .contains("crSeg.parts.push({ t: \" (\" + sessHit + \")\", c: CLS_DIM });"),
            "会话条应在 ⟲ 段后追加弱色命中率段 (NN%)（sess 口径 cr/(cr+up)，与底部条「缓存」段同源）: {sess_body}"
        );
        assert!(
            sess_body.contains("if (tcw > 0) {")
                && sess_body
                    .contains("segs.push({ t: \"✚ \" + fmtTokens(tcw), title: W.cwTitle });"),
            "会话条 ✚ 段应 cw 0 隐藏、>0 带词表 title: {sess_body}"
        );
        // title 通道：renderRowParts（part 级）与 renderBarLine（seg 级）
        // ——title 是属性不是文本，不进比对串
        assert!(
            USAGE_JS.contains("if (parts[i].title) sp.title = parts[i].title;")
                && USAGE_JS.contains("if (segs[sj].title) sp.title = segs[sj].title;"),
            "全串比对渲染应补 part/seg 级 title 通道（title 不进比对串）"
        );
        // 词表双语条目
        assert!(
            USAGE_JS.contains("cwTitle: \"缓存写 token\"")
                && USAGE_JS.contains("cwTitle: \"Cache write tokens\""),
            "WORDS 应补 ✚ 缓存写段 title 词条（zh/en 双语）"
        );
        // 底部条「缓存 NN%」段保留（V35 起三处并存，口径同源）
        assert!(
            USAGE_JS.contains("var hit = cacheHitText(tcr, tin);")
                && USAGE_JS.contains("if (hit !== \"—\") {"),
            "底部条「缓存 NN%」段应保留（与每轮条/会话条命中率段并存）"
        );
    }

    #[test]
    fn usage_js_v36_浅色主题可读性() {
        // ---- 版本标记 ----
        assert!(USAGE_JS.contains("ZBAR-THEME-V47"));
        assert!(!USAGE_JS.contains("ZBAR-THEME-V45"), "版本头应已升到 V47");
        // ---- a) 透明度下限：三处行透明消费点（每轮条容器/会话条行/
        // 底部条行）以 max() 兜 .82 底（用户调高 >.82 仍取用户值），
        // dim/qfaint 弱色同步提亮；规则全部带 html:not(.dark) 前缀 ----
        assert!(
            USAGE_JS.contains(
                "\"html:not(.dark) [\" + ATTR_ROW + \"]{opacity:max(var(--zbar-usage-opacity,.55),.82);}\""
            ) && USAGE_JS.contains(
                "\"html:not(.dark) [\" + ATTR_SESSION_BAR + \"] .zbar-usage-sb-line{opacity:max(var(--zbar-usage-opacity,.55),.82);}\""
            ) && USAGE_JS.contains(
                "\"html:not(.dark) [\" + ATTR_QUOTA_BAR + \"] .zbar-usage-qb-line{opacity:max(var(--zbar-usage-opacity,.55),.82);}\""
            ),
            "三处行透明消费点应有 html:not(.dark) 的 opacity:max(…,.82) 下限规则"
        );
        assert!(
            USAGE_JS.contains("\"html:not(.dark) .zbar-usage-dim{opacity:.82;}\"")
                && USAGE_JS.contains(
                    "\"html:not(.dark) [\" + ATTR_QUOTA_BAR + \"] .zbar-usage-qfaint{opacity:.85;}\""
                ),
            "浅色弱色应提亮（dim .82 / qfaint .85）"
        );
        // 深色原规则保留（红线：深色主题渲染零变化）
        assert!(USAGE_JS.contains(".zbar-usage-dim{opacity:.66;}"));
        // ---- b) 语义色浅色加深变体：四组容器（模型底板/每轮条/会话
        // 条/底部条）tps 三档 + 会话条/底部条 quota 两档，白底可读加
        // 深版 #059669/#d97706/#dc2626；深色亮色原规则原样保留 ----
        for attr in ["ATTR_MODELS", "ATTR_ROW", "ATTR_SESSION_BAR", "ATTR_QUOTA_BAR"] {
            for (cls, light, dark) in [
                ("zbar-usage-tps-fast", "#059669", "#34d399"),
                ("zbar-usage-tps-mid", "#d97706", "#fbbf24"),
                ("zbar-usage-tps-slow", "#dc2626", "#f87171"),
            ] {
                assert!(
                    USAGE_JS.contains(&format!(
                        "\"html:not(.dark) [\" + {attr} + \"] .{cls}{{color:{light};}}\""
                    )),
                    "浅色加深变体应覆盖 {attr} 的 {cls}"
                );
                assert!(
                    USAGE_JS.contains(&format!(
                        "\"[\" + {attr} + \"] .{cls}{{color:{dark};}}\""
                    )),
                    "深色亮色原规则应保留：{attr} 的 {cls}"
                );
            }
        }
        for attr in ["ATTR_SESSION_BAR", "ATTR_QUOTA_BAR"] {
            for (cls, light, dark) in [
                ("zbar-usage-quota-mid", "#d97706", "#fbbf24"),
                ("zbar-usage-quota-low", "#dc2626", "#f87171"),
            ] {
                assert!(
                    USAGE_JS.contains(&format!(
                        "\"html:not(.dark) [\" + {attr} + \"] .{cls}{{color:{light};}}\""
                    )),
                    "浅色加深变体应覆盖 {attr} 的 {cls}"
                );
                assert!(
                    USAGE_JS.contains(&format!(
                        "\"[\" + {attr} + \"] .{cls}{{color:{dark};}}\""
                    )),
                    "深色亮色原规则应保留：{attr} 的 {cls}"
                );
            }
        }
    }

    #[test]
    fn has_inject_判定() {
        assert!(!has_inject(SAMPLE_HTML));
        assert!(has_inject(&format!("x{INJECT_BEGIN}y{INJECT_END}z")));
    }

    #[test]
    fn 模板_版本头与token覆盖与图片支持() {
        // 头部版本标记（store::ensure_versioned_template 的升级判据）：
        // theme.css 升 V12（壳透明规则变量驱动，壁纸关闭恢复原始整窗底），
        // effects.js V6（壁纸层总开关）
        assert!(THEME_CSS.contains("ZBAR-THEME-V12"));
        assert!(!THEME_CSS.contains("ZBAR-THEME-V11"), "版本头应已升到 V12");
        assert!(EFFECTS_JS.contains("ZBAR-THEME-V6"));
        assert!(
            !EFFECTS_JS.contains("ZBAR-THEME-V5"),
            "版本头应已升到 V6"
        );
        // V12 壳规则契约：全视口应用壳容器选择器必须消费
        // --zbar-shell-uncover（color-mix 形式，变量缺失兜底 0 保持 V11
        // 透明观感）——不得回退为紧跟选择器的硬编码
        // background: transparent !important（旧形态让壁纸开关的卸载路径
        // 与空转变量都覆盖不到壳底，关闭壁纸后壳仍被挖空、回不到原始
        // 整窗底色）。contains 断言 color-mix 形式存在即可锁定新契约
        assert!(
            THEME_CSS.contains("div.flex.h-dvh.flex-col.overflow-hidden")
                && THEME_CSS.contains(
                    "color-mix(in oklab, var(--color-background, #fff) calc(var(--zbar-shell-uncover, 0) * 100%), transparent) !important;"
                ),
            "V12 壳规则应为消费 --zbar-shell-uncover（缺失兜底 0）的 color-mix 形式，不得硬编码透明"
        );
        // usage.js V24（速度快照拆出 usage-speed.js 小文件；历史用量大文件
        // 仍按 2 秒节拍更新，注入版独立轮询速度小文件）+ V23（删除会话条
        // CTX 上下文占用段：用户改主意，CTX 百分
        // 比展示下线——渲染端删除 "CTX NN%" 段与三档变色样式，数据端
        // usage_feed 同步删除 sess 行 cp/cu/cw 字段与 CTX 查询，逐段
        // span 渲染结构保留）；V21（数据端新增 sess 会话级统计数组：
        // model_usage 全量合计修正会话累计口径（turn_usage 覆盖不全）；
        // 渲染端 sessionTotals 优先读 sessIndex、命中时不叠加 runs 防双
        // 计、数据缺失回退旧口径，会话条逐段 span 化（V21 的 CTX 段已
        // 被 V22 删除）；
        // V20（配合数据端 V20 双修复：turns 新增子代理自身视图
        // 行（subagent:1），子轮 umid 进入完成索引后已完成子代理轮不再
        // 被误判为活动轮、面板按 index 命中显示真实 token，会话累计按
        // sess 精确匹配无双计路径，渲染管线零改动；数据端父会话保活修
        // 复主轮行被子代理运行静默踢出 runs 的缺陷）；V19 新增每轮统计
        // 条开关参数 usage_turn_bar（默认开启）：renderAll 第二遍渲染前
        // 统一读 --zbar-usage-turn-bar（变量缺失视为开启），关闭时对全
        // 部轮节点 removeRow 并跳过 renderOne，速度快照与会话条不受影
        // 响；V18 修复新建任务后（空会话）会话累计条停留在上一个
        // 会话数据：renderAll 无轮节点分支早退导致 renderSessionBar 永
        // 不执行，该分支补 removeBar；V17 修复新建任务后会话累计条不消
        // 失、数据不重置：currentSessionId 改为焦点优先+可见优先的容器
        // 遍历选择（旧
        // querySelector 首中在多会话保活时拿到旧会话），composer 挂载
        // 点经 pickComposerRegion 可见优先并跟随当前会话容器；V16 整体
        // 移除每轮统计行的悬浮 title 提示，titleOf/
        // liveTitleOf/LIVE_TITLE/LIVE_START_TITLE 一并删除；V15 会话条
        // Σ 段新增会话总 Token + 速度段去 ⋯ 前缀 +
        // 流式估算段改 ≈ 前缀；V14 修复 V13 遗漏 SEL_COMPOSER 定义 + 挂
        // 载异常一次性告警；V13 会话条挂载进输入区容器 + CSS 留白定位，
        // V12 动态测量废弃；V10 统计条显示稳定性：三态统一固定
        // 结构 + 数字等宽补位 + 会话条动态段固定 + 完成子代理面板残留
        // 占位枯萎清理；V9 子代理消耗实时化：document 级扫描 + 多容器
        // 活动轮 + 主轮 live 行 sub 合计 + 会话条 Σ 跳过 m 行；V8 启动
        // 窗口实时渲染；V7 请求图标 ⟳ → ×；V6 生成过程实时跳动）
        // V25（速度聚合 + 模型分组速度：每轮条/会话条追加 均/快/慢 聚合段，
        // 会话条下方追加模型分组速度行；数据端 usage_feed 同步新增
        // turns[].sa / sess[].sa / 顶层 models[]，随 2 秒大文件发布，
        // usage-data.js 的 v 保持 2、usage-speed.js 内容不变）
        // V28（额度展示改版：额度从会话条 segs 移出，改为输入框卡片底
        // 部外侧的独立额度行 renderQuotaBar + 独立开关
        // --zbar-usage-quota-bar；数据端零改动，usage-quota.js 契约与
        // V27 额度线程不变）
        // V29（实机反馈三修复：额度行样式修正（字号取消 ×0.9、整行灰
        // 白、删除更新时间戳尾段）；空会话隐藏额度行与模型行
        // （currentSessionHasData 会话级判定 + renderAll 空轮分支双重
        // 闸，实机复检后取代全局级 sessionDataEmpty）；模型速度行拆出
        // 为独立容器（data-zbar-usage-models）+ 独立开关
        // usage_model_rows / --zbar-usage-model-rows + 留白几何拆分
        // syncComposerPad；数据端零改动，三个数据文件契约不变）
        // V30（额度行微调 + 缓存命中率挪位：bottom 负偏移 -16 → -20；
        // 会话条 ⟲ 段命中率后缀移除，改由额度行新增「缓存 NN%」段承载
        // ——renderQuotaBar 接收 anchor、按 renderSessionBar 同款双口
        // 径算当前会话命中率，分母 0 不入段；数据端零改动）
        // V31（底部额度行追加窗口重置倒计时，额度契约 v:1 → v:2：
        // usage-quota.js 新增 h5rs/wkrs 重置时刻 ms；5h/周余段追加括号
        // 纯时长 "（2h35m）"，fmtResetCountdown/quotaCountdownOf + seg.sub
        // 无 class 独立 span 灰白继承行色；loadQuota 与 renderQuotaBar
        // 版本闸放行 v:1/v:2 平滑过渡；倒计时 Date.now() 每拍重算，全串
        // 比对零 DOM 机制不变）
        // V32（底部额度行扁平化去括号，纯渲染端微调、数据契约零改动：
        // zh 词表「5h 余」→ "5h "、「周余」→ "每周 "；quotaCountdownOf
        // 去括号返回裸时长，renderQuotaLine 在 sub span 前插半角空格
        // text node、全串比对同步 t + " " + sub；seg.sub 通道与阈值变
        // 色边界不变）
        // V33（底部额度行演进为分段胶囊状态条 + 全组件设计语言统一：
        // 字体 var(--font-mono)、颜色 var(--color-*) 兜底、亮度+字重
        // 三级层级、发丝分隔、悬停高亮、行内 SVG 图标、呼吸蓝点；新消
        // 费 sess[].tl 与顶层 today{tt,rq}（数据端 V33 附加字段，缺键
        // 整段隐藏）；每轮条/会话条保持全串比对零 DOM 机制、底部条改
        // renderModelRows 同款签名比对；定位/测量/留白与三个 poll 节拍
        // 零改动）
        // V34（底部状态条实机反馈修复：a) 分段优先级隐藏——data-prio
        // 标级、溢出时按 缓存→今日→工具→本轮→子代理→会话 逐级隐藏再
        // 复测，速度/额度 prio 0 永不隐藏，fitQuotaSegments 渲染后适
        // 配、隐藏态计入签名（quotaSigOf/quotaFitHidden），mask 仅兜
        // 底，段 padding 9→6px、段内间距改 flex gap 3px；b) 模型行文
        // 字改 --color-foreground/--color-foreground-subtle + 中灰兜底
        // （浅色卡片对比度修复）；c) 会话条速度值补三档色；d) 额度/每
        // 周/缓存段补 title、段内无 "·"；e) 底部条数值走 fmtTokensShort
        // 去等宽补位，文本不带语义间距空格）
        // V35（注入条与输入框卡片左右缘对齐 + 每轮条/会话条缓存命中率
        // 回归 + ✚ 缓存写 0 值隐藏，数据契约零改动：a) 新增
        // locateComposerCard（遍历 region 直接子元素，跳过带
        // data-zbar-usage-* 属性或绝对定位的自家注入元素，取第一个可
        // 见且 offsetWidth>100 的元素为输入框卡片，零选择器假设）；
        // b) 底部胶囊条卡片命中时 left/width 两缘精确对齐（未命中回退
        // 右对齐自然宽）、会话条左缘对齐（未命中回退居中）、模型行底
        // 板左缘/宽度对齐（未命中回退居中）——applyCardAlign 值变才
        // 写 / clearCardAlign 回退清理，窗口缩放由下一渲染拍/兜底拍自
        // 愈（不做 resize 监听）；c) 每轮条/会话条 ⟲ 段后补弱色命中率
        // 段 (NN%)（cacheHitText 共用口径，分母 0 不入段；底部条「缓
        // 存」段并存）；d) ✚ 缓存写段 cw 0 整段隐藏（含启动窗口占位
        // 态）、>0 带 title（词表新增 cwTitle 双语条目，
        // renderRowParts/renderBarLine 补 title 通道））
        assert!(USAGE_JS.contains("ZBAR-THEME-V47"));
        assert!(!USAGE_JS.contains("ZBAR-THEME-V45"), "版本头应已升到 V47");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V43"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V33"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V31"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V30"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V29"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V27"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V26"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V25"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V24"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V23"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V19"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V18"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V10"), "版本头不应回退");
        // V24 速度旁路：速度数据有独立小文件、独立 loader 与独立节拍；
        // 大文件仍按 2 秒历史节拍，不因速度变化重载。
        assert!(USAGE_JS.contains("usage-speed.js"), "应定位速度小文件");
        assert!(USAGE_JS.contains("var SPEED_LOADER_ID = \"zbar-usage-speed-loader\""));
        assert!(USAGE_JS.contains("var POLL_MS = 2000"));
        assert!(USAGE_JS.contains("var SPEED_POLL_MS = 1000"));
        assert!(USAGE_JS.contains("function loadSpeed"));
        assert!(USAGE_JS.contains("function speedPollLoop"));
        let speed_lo = USAGE_JS.find("function loadSpeed").expect("loadSpeed 应存在");
        let speed_hi = USAGE_JS
            .find("function speedPollLoop")
            .expect("speedPollLoop 应存在");
        let speed_loader_body = &USAGE_JS[speed_lo..speed_hi];
        assert!(speed_loader_body.contains("SPEED_LOADER_ID"));
        assert!(speed_loader_body.contains("speedUrl"));
        assert!(!speed_loader_body.contains("dataUrl"));
        // V27 额度旁路：独立小文件 + 独立 loader + 独立低频节拍（30s，
        // 隐藏降频 60s），绝不进大文件/速度旁路；消费 window.__ZBAR_QUOTA__
        // 且版本不符保留上次额度；V31 起版本闸放行 v:1/v:2（升级过渡期
        // 残留 v:1 照常消费，写出 v:2 后自然生效）
        assert!(USAGE_JS.contains("usage-quota.js"), "应定位额度小文件");
        assert!(USAGE_JS.contains("var QUOTA_LOADER_ID = \"zbar-usage-quota-loader\""));
        assert!(USAGE_JS.contains("var QUOTA_POLL_MS = 30000"));
        assert!(USAGE_JS.contains("var QUOTA_POLL_HIDDEN_MS = 60000"));
        assert!(USAGE_JS.contains("function loadQuota"));
        assert!(USAGE_JS.contains("function quotaPollLoop"));
        let quota_lo = USAGE_JS.find("function loadQuota").expect("loadQuota 应存在");
        let quota_hi = USAGE_JS
            .find("function quotaPollLoop")
            .expect("quotaPollLoop 应存在");
        let quota_loader_body = &USAGE_JS[quota_lo..quota_hi];
        assert!(
            quota_loader_body.contains("window.__ZBAR_QUOTA__")
                && quota_loader_body.contains("(q.v !== 1 && q.v !== 2)")
                && quota_loader_body.contains("quotaData = q;"),
            "额度 loader 应消费 __ZBAR_QUOTA__ 并做版本校验（V31 放行 v:1/v:2）：{quota_loader_body}"
        );
        assert!(
            !quota_loader_body.contains("dataUrl") && !quota_loader_body.contains("speedUrl"),
            "额度 loader 不得触碰大文件/速度旁路地址：{quota_loader_body}"
        );
        // V28/V29 独立额度行特征（V27 额度段从会话条 segs 移出改版；V29
        // 样式修正）：独立渲染/挂载/移除函数 + 开关读取函数；右对齐 +
        // 底部外侧负偏移挂载；region 缺失兜底 fixed 标记；词表双语标签
        // （V29 删除更新时间词）；三档配色阈值与 class 保留
        assert!(
            USAGE_JS.contains("function renderQuotaBar(sessHasData, anchor, activeMap)")
                && USAGE_JS.contains("function renderQuotaLine")
                && USAGE_JS.contains("function quotaBarEnabled")
                && USAGE_JS.contains("function ensureQuotaBar")
                && USAGE_JS.contains("function removeQuotaBar"),
            "V28 应新增独立额度行的渲染/挂载/移除与开关读取函数（V33 起 renderQuotaBar 增 activeMap 入参）"
        );
        assert!(
            USAGE_JS.contains("ATTR_QUOTA_BAR = \"data-zbar-usage-quota\"")
                && USAGE_JS.contains("VAR_QUOTA_BAR = \"--zbar-usage-quota-bar\""),
            "额度行应有防重复标记属性与独立开关变量（variables.css 渲染 1/0，缺失视为开启）"
        );
        assert!(
            USAGE_JS.contains(
                "position:absolute;bottom:\" + QUOTA_BAR_BOTTOM_PX + \"px;left:0;right:0;"
            ),
            "V40 额度行应 absolute 满宽定位进 region 底部留白带（bottom 间隔）"
        );
        assert!(
            USAGE_JS.contains("ATTR_QUOTA_BAR_FIXED")
                && USAGE_JS.contains("QUOTA_BAR_FALLBACK_RIGHT_PX")
                && USAGE_JS.contains("QUOTA_BAR_FALLBACK_BOTTOM_PX"),
            "region 缺失兜底路径应有 fixed 切换标记与贴底居中常量（同会话条兜底模式）"
        );
        assert!(
            USAGE_JS.contains("quota5h: \"5h \""),
            "zh/en 词表应含额度行 5h 窗口前缀（V32 扁平化去「余」字；V46 去文字标签后保留——5h 是窗口标识而非列标签）"
        );
        // V46 防回退：失去消费方的额度/缓存标签词条删除（zh/en 两侧）
        assert!(
            !USAGE_JS.contains("quotaLead: \"")
                && !USAGE_JS.contains("quotaWk: \"")
                && !USAGE_JS.contains("cachePct: \""),
            "V46 去文字标签后 quotaLead/quotaWk/cachePct 词条应删除"
        );
        assert!(
            USAGE_JS.contains("var QUOTA_MID_REMAIN = 30")
                && USAGE_JS.contains("var QUOTA_LOW_REMAIN = 10"),
            "额度三档阈值应集中常量区"
        );
        assert!(
            USAGE_JS.contains("function quotaClassOf")
                && USAGE_JS.contains(".zbar-usage-quota-mid{color:#fbbf24;}")
                && USAGE_JS.contains(".zbar-usage-quota-low{color:#f87171;}"),
            "额度行应有三档配色（<30 黄 / <10 红 / 充足继承行色）"
        );
        // V29 时间戳尾段整体删除：fmtClock、词表 updated、专用样式全无
        // 残留（负向断言防回归）
        assert!(
            !USAGE_JS.contains("function fmtClock")
                && !USAGE_JS.contains("W.updated")
                && !USAGE_JS.contains("zbar-usage-qb-time"),
            "V29 应删除更新时间戳尾段（fmtClock/词表 updated/专用样式全删）"
        );
        // V29 额度行样式修正：字号升回基准（取消 ×0.9 缩小）、行高压缩
        // 1.4（V37 再压至 1.25）（限定额度行 CSS 块切片断言，避免与模
        // 型行 ×0.9 规则串味）
        let qcss_lo = USAGE_JS
            .find("\"[\" + ATTR_QUOTA_BAR + \"]{\"")
            .expect("额度行样式块应存在");
        // 切片右界回退到 UTF-8 字符边界（CSS 片段含中文注释，固定
        // +700 可能落在多字节字符中间导致 panic）
        let mut qcss_end = (qcss_lo + 700).min(USAGE_JS.len());
        while qcss_end > qcss_lo && !USAGE_JS.is_char_boundary(qcss_end) {
            qcss_end -= 1;
        }
        let qcss = &USAGE_JS[qcss_lo..qcss_end];
        assert!(
            qcss.contains("font-size:var(--zbar-usage-font-size,10px);")
                && !qcss.contains("* .9"),
            "额度行字号应与统计元素同级（取消 ×0.9 缩小）: {qcss}"
        );
        assert!(
            qcss.contains("line-height:1.2;")
                && qcss.contains("left:0;right:0;")
                && USAGE_JS.contains("width:100%;max-width:100%;")
                && USAGE_JS.contains("height:max(32px, calc(var(--zbar-usage-font-size,10px) * 2.3));")
                && !USAGE_JS.contains("width:176px"),
            "V43 额度行应满宽容器 + 32px 高常显胶囊（line-height 1.2）: {qcss}"
        );
        assert!(
            USAGE_JS.contains("var QUOTA_BAR_BOTTOM_PX = -13")
                && USAGE_JS.contains("var QUOTA_BAR_GAP_PX = 8")
                && !USAGE_JS.contains("var QUOTA_BAR_BOTTOM_PX = -24")
                && !USAGE_JS.contains("var QUOTA_BAR_BOTTOM_PX = -20")
                && !USAGE_JS.contains("var QUOTA_BAR_BOTTOM_PX = 4")
                && !USAGE_JS.contains("var QUOTA_BAR_BOTTOM_PX = 5"),
            "V40 额度行应废弃负偏移与静态预算，浮胶囊 bottom = QUOTA_BAR_GAP_PX 恒定间隔"
        );
        // V37 底部留白带：region 注入 padding-bottom（照会话条 padding-top
        // 预留空间同款模式），常量与 CSS 兜底两处同步；负向断言防负偏移
        // 方案回流（历史数值仅允许出现在沿革注释中，不允许出现在定义处）
        assert!(
            USAGE_JS.contains("var COMPOSER_PAD_BOTTOM_PX = 27")
                && USAGE_JS.contains("VAR_PAD_BOTTOM = \"--zbar-usage-pad-bottom\"")
                && USAGE_JS.contains("ATTR_QUOTA_BAND = \"data-zbar-usage-quota-band\"")
                && USAGE_JS.contains(
                    "\".chat-composer-region[\" + ATTR_QUOTA_BAND + \"]{\""
                )
                && USAGE_JS
                    .contains("padding-bottom:var(\" + VAR_PAD_BOTTOM + \",\" +")
                && USAGE_JS
                    .contains("COMPOSER_PAD_BOTTOM_PX + \"px) !important;}\""),
            "V43 底部留白带：只在 [data-zbar-usage-quota-band] 属性命中时注入 padding-bottom（27px 兜底）"
        );
        // V37R 留白带动态实测：静态预算实机失衡（行实际高约 26px 远超假设
        // 16px，行顶顶出带外与卡片贴死、带下空置 1:15）——行渲染后实测
        // bar.offsetHeight，带高 = 条高 + GAP + BOTTOM 经 setRegionPadBottom 写入
        // （照 setRegionPad 惯例：region 迁移还原、不高于基数回落 CSS 兜
        // 底；行隐藏/空会话保留最近一次写入值不新增移除路径；fixed 兜底
        // 路径不施加）
        assert!(
            USAGE_JS.contains("function syncQuotaBand(bar, region)")
                && USAGE_JS.contains("var h = bar.offsetHeight;")
                && USAGE_JS
                    .contains("setRegionPadBottom(region, h + QUOTA_BAR_GAP_PX + QUOTA_BAR_BOTTOM_PX);")
                && USAGE_JS.contains("function setRegionPadBottom(region, px)")
                && USAGE_JS
                    .contains("region.style.setProperty(VAR_PAD_BOTTOM, px + \"px\");")
                && USAGE_JS.contains("region.setAttribute(ATTR_QUOTA_BAND, \"\");")
                && USAGE_JS.contains("function clearQuotaBand()")
                && USAGE_JS
                    .contains("paddedBottomRegion.removeAttribute(ATTR_QUOTA_BAND);")
                && USAGE_JS.contains("syncQuotaBand(bar, quotaRegion);"),
            "V43 底部留白带应动态实测条高（offsetHeight + GAP + BOTTOM），属性随条可见性打上/清除"
        );
        // renderQuotaBar 体内：会话级无数据/开关关闭移除（V29 P1 修复：
        // sessHasData 入参闸取代初版全局判定 sessionDataEmpty——后者
        // 在多会话保活下任一旧会话有数据即放行，新建任务页仍出现孤立
        // 额度行）+ 额度数据分段（缺失不入段）+ 全缺失整行隐藏 + 复用
        // pickComposerRegion 挂载 + 无 HTML 注入面
        let qb_lo = USAGE_JS
            .find("function renderQuotaBar(sessHasData, anchor, activeMap)")
            .expect("renderQuotaBar 应存在");
        let qb_hi = USAGE_JS
            .find("function renderQuotaLine")
            .expect("renderQuotaLine 应存在");
        let quota_body = &USAGE_JS[qb_lo..qb_hi];
        assert!(
            quota_body.contains("if (!sessHasData || !quotaBarEnabled()) {")
                && quota_body.contains("removeQuotaBar();"),
            "当前会话无数据（会话级判定）或开关关闭都应移除额度行: {quota_body}"
        );
        assert!(
            !USAGE_JS.contains("function sessionDataEmpty")
                && !USAGE_JS.contains("lastTurns.length || lastRuns.length"),
            "全局级空判定 sessionDataEmpty 应由会话级判定取代并删除（负向断言防回归）"
        );
        // V29 P1 修复：会话级数据判定 currentSessionHasData——提炼
        // renderSessionBar「无数据即 removeBar」的同款条件：锚点/会话
        // id 缺失同判无数据；有数据 = sess 全量合计行命中（V21 新口
        // 径）或归属完成轮/进行中 run/活动轮任一存在。多会话保活下判
        // 定只看当前会话自身数据源，旧会话数据不再作为新建任务页的渲
        // 染依据
        assert!(
            USAGE_JS.contains("function currentSessionHasData(anchor, activeMap)")
                && USAGE_JS.contains("if (!anchor || !sessId) return false;")
                && USAGE_JS.contains("if (sv && sv.tt != null) return true;")
                && USAGE_JS.contains(
                    "sessionTotals(sessId) || sessionRunTotals(sessId) ||\n      activeMap.has(sessId)"
                ),
            "会话级判定应与 renderSessionBar removeBar 同口径（锚点/id 缺失 + 四路数据源任一）"
        );
        // renderAll 主路径求值一次并作为模型行/额度行的渲染闸（当前会
        // 话无数据时不渲染，与会话条 removeBar 同拍生效）
        assert!(
            USAGE_JS.contains("var sessHasData = currentSessionHasData(anchor, activeMap);"),
            "renderAll 主路径应求值会话级判定（sessHasData）"
        );
        assert!(
            quota_body.contains("W.quota5h + Math.round(qd.h5) + \"%\"")
                && quota_body.contains("Math.round(qd.wk) + \"%\"")
                && quota_body.contains("quotaClassOf(qd.h5)")
                && quota_body.contains("quotaClassOf(qd.wk)")
                && quota_body.contains("ico: \"battery\"")
                && quota_body.contains("ico: \"repeat\""),
            "额度行应按额度旁路数据渲染 5h/周剩余段（V46 去前导标签补 battery/repeat 图标，缺失不入段，无分隔符残留）: {quota_body}"
        );
        // V30 缓存命中率段：签名带 anchor（renderAll 主路径与 sessHasData
        // 同源传入），函数体内按 renderSessionBar 同款双口径算当前会话
        // tcr/tin（sess 全量合计命中即消费 sess 行，否则 totals +
        // runTotals 回退叠加），每周段之后追加「缓存 NN%」段（灰白不高
        // 亮）；分母为 0（cacheHitText 返回 "—"）不入段，不留残留。
        // V33 签名再增 activeMap（速度/本轮分段取活动轮 runs 行）
        assert!(
            USAGE_JS.contains("function renderQuotaBar(sessHasData, anchor, activeMap)")
                && quota_body.contains("var sessId = currentSessionId(anchor);")
                && quota_body.contains("var useSess = !!(sv && sv.tt != null);")
                && quota_body.contains("(totals ? totals.tcr : 0) + (runTotals ? runTotals.tcr : 0)")
                && quota_body.contains("(totals ? totals.tin : 0) + (runTotals ? runTotals.tin : 0)"),
            "renderQuotaBar 应接收 anchor/activeMap 并按 renderSessionBar 同款双口径算当前会话命中率: {quota_body}"
        );
        assert!(
            quota_body.contains("var hit = cacheHitText(tcr, tin);")
                && quota_body.contains("if (hit !== \"—\") {")
                && quota_body
                    .contains("segs.push({\n        ico: \"gauge\",\n        title: W.cacheTitle,\n        parts: [{ t: hit, c: \"\" }]\n      });"),
            "额度行应在每周段后追加缓存段（V46 去文字标签、仪表图标 + 口径 title，分母为 0 时不入段）: {quota_body}"
        );
        // V31/V32 窗口重置倒计时：a) 新增格式化与倒计时段函数（rs 缺失/
        // 非法/已过期 <= now 返回空串 → 该窗口不加段；时长阶梯 <1h "35m"
        // / <24h "2h35m" / ≥24h "5d2h"，英文单位不进词表；V32 去括号直
        // 接返回裸时长）；b) renderQuotaBar 版本闸放行 v:1/v:2（升级过渡
        // 期残留 v:1 无 h5rs/wkrs → 无倒计时段，行为与升级前一致）；
        // c) 5h/周段以 seg.sub 携带倒计时段、nowMs 每拍重算，全串比对纳
        // 入 sub（V32 起前置半角空格，零 DOM 机制保持）；d) renderQuotaLine
        // 以无 class 独立 span 渲染 sub（与百分比 span 平级不嵌套，恒继
        // 承灰白行色，阈值变色只属于剩余百分比）
        assert!(
            USAGE_JS.contains("function fmtResetCountdown")
                && USAGE_JS.contains("function quotaCountdownOf")
                && USAGE_JS.contains(
                    "if (typeof rs !== \"number\" || !isFinite(rs) || rs <= now) return \"\";"
                )
                && USAGE_JS.contains(
                    "if (typeof msLeft !== \"number\" || !isFinite(msLeft) || msLeft <= 0) return \"\";"
                )
                && USAGE_JS.contains("if (mins < 60) return mins + \"m\";")
                && USAGE_JS.contains("if (hours < 24) return hours + \"h\" + (mins % 60) + \"m\";")
                && USAGE_JS.contains("return Math.floor(hours / 24) + \"d\" + (hours % 24) + \"h\";")
                && USAGE_JS.contains("return body;")
                && !USAGE_JS.contains("（\" + body + \"）"),
            "V31/V32 应有倒计时格式化/裸时长函数（rs 缺失或 <= now 不加段，V32 去括号、时长阶梯保留）"
        );
        assert!(
            quota_body.contains("quotaData.v === 1 || quotaData.v === 2"),
            "renderQuotaBar 版本闸应放行 v:1/v:2（升级过渡期残留 v:1 文件无倒计时段照常显示）: {quota_body}"
        );
        assert!(
            quota_body.contains("var nowMs = Date.now();")
                && quota_body.contains("var h5cd = quotaCountdownOf(qd.h5rs, nowMs);")
                && quota_body.contains("var wkcd = quotaCountdownOf(qd.wkrs, nowMs);")
                && quota_body.contains("if (h5cd) h5Parts.push({ t: h5cd, c: \"zbar-usage-qfaint\" });")
                && quota_body.contains("if (wkcd) wkParts.push({ t: wkcd, c: \"zbar-usage-qfaint\" });"),
            "5h/周段应携带重置倒计时段（qfaint 弱色 span 显式回灰——阈变色仍只属于剩余百分比；Date.now() 每拍重算，签名比对纳入倒计时保持零 DOM 机制；V34 起段内间距由 flex gap 承担、文本不带前导空格）: {quota_body}"
        );
        let ql_lo = USAGE_JS
            .find("function renderQuotaLine")
            .expect("renderQuotaLine 应存在");
        let ql_hi = USAGE_JS
            .find("function renderAll")
            .expect("renderAll 应存在");
        let ql_body = &USAGE_JS[ql_lo..ql_hi];
        // V33 签名比对 + 分段渲染：qsig 属性无变化零 DOM；分段经
        // createElementNS 构建 SVG 图标 + parts 层级 span + 可选呼吸蓝
        // 点；无 innerHTML 注入面
        assert!(
            ql_body.contains("data-zbar-usage-qsig")
                && ql_body.contains("line.querySelector(\":scope > .zbar-usage-qfull\")")
                && ql_body.contains("bar.getAttribute(\"data-zbar-usage-qsig\") === sig")
                && ql_body.contains("bar.setAttribute(\"data-zbar-usage-qsig\", sig);"),
            "底部状态条应以签名比对渲染分段（V33：无变化零 DOM，行/容器重建时属性缺失自动重渲）: {ql_body}"
        );
        assert!(
            ql_body.contains("var ico = seg.ico ? makeIcon(seg.ico) : null;")
                && ql_body
                    .contains("sp.className = \"zbar-usage-qseg\" + (seg.c ? \" \" + seg.c : \"\");")
                && !ql_body.contains("data-prio")
                && !ql_body.contains("qseg-off")
                && ql_body.contains("if (seg.title) sp.title = seg.title;")
                && ql_body.contains("dot.className = \"zbar-usage-qdot\";")
                && !ql_body.contains("innerHTML"),
            "分段应由 qseg span（段级配色 + 原生 title + makeIcon SVG 图标 + parts 层级 span + 可选蓝点）构成，V44 起不带隐藏态 class/data-prio: {ql_body}"
        );
        // V44 滚动交互监听：随 qb-line 行元素创建挂载一次（wheel 非
        // passive 才能 preventDefault；scroll 同步渐隐 class）；行元素
        // 与条同生共死，断连重建走创建分支自然重挂
        assert!(
            ql_body.contains("line.addEventListener(\"wheel\", onQuotaWheel, { passive: false });")
                && ql_body.contains("line.addEventListener(\"scroll\", onQuotaFadeScroll, false);"),
            "滚轮/滚动监听应随 qb-line 创建挂载（元素即生命周期，不重复挂载）: {ql_body}"
        );
        assert!(
            quota_body.contains("if (!segs.length) {"),
            "查询未成功/h5 与 wk 全缺失时整行隐藏，不渲染空行"
        );
        assert!(
            quota_body.contains("pickComposerRegion(\"\")"),
            "额度行应复用 pickComposerRegion 挂进输入区容器（空 id = 第一个可见容器）"
        );
        assert!(
            !quota_body.contains("innerHTML"),
            "额度行同样只用 textContent 写入外部字符串（无注入面）"
        );
        // 额度行随 renderAll 主路径每拍渲染（数据 onload 与开关热重载直
        // 接生效，不依赖会话条渲染节奏）；V29 P1 修复起调用携带会话级
        // 判定闸，V30 起再传 anchor（当前会话缓存命中率口径），主路径
        // 仅剩一处调用
        let ra_lo = USAGE_JS
            .find("function renderAll")
            .expect("renderAll 应存在");
        let ra_all = &USAGE_JS[ra_lo..];
        assert_eq!(
            ra_all.matches("renderQuotaBar(sessHasData, anchor, activeMap);").count(),
            1,
            "renderAll 应只在主路径调用 renderQuotaBar（携带会话级判定闸/锚点/活动轮 Map，V33 起供速度与本轮分段取活动轮 runs 行；空轮分支改为 removeQuotaBar 隐藏）"
        );
        // 会话条不再承载额度段（V28 移出）：renderSessionBar 体内不得再
        // 出现额度数据消费与额度标签；CTX 负向断言保留。V29：模型行渲
        // 染同样从 renderSessionBar 内部移出（renderAll 独立调用）
        let sbq_lo = USAGE_JS
            .find("function renderSessionBar")
            .expect("renderSessionBar 应存在");
        let sbq_hi = USAGE_JS
            .find("function renderBarLine")
            .expect("renderBarLine 应存在");
        let sess_bar_body = &USAGE_JS[sbq_lo..sbq_hi];
        assert!(
            !sess_bar_body.contains("quotaData")
                && !sess_bar_body.contains("W.quota5h")
                && !sess_bar_body.contains("W.quotaWk"),
            "V28 起额度段应从会话条 segs 移出（由独立额度行承载）: {sess_bar_body}"
        );
        assert!(
            !sess_bar_body.contains("renderModelRows")
                && !sess_bar_body.contains("ensureModelsBox"),
            "V29 起模型行渲染与容器应从 renderSessionBar 内部移出（独立开关控制）: {sess_bar_body}"
        );
        assert!(
            !sess_bar_body.contains("\"CTX \""),
            "CTX 段不应因额度行改版而回归（V22 已删）"
        );
        // V29 模型行渲染解耦：renderAll 主路径独立调用 renderModelRows
        // （携带会话级判定闸，P1 修复），留白统一入口在主路径与空轮分
        // 支各同步一次
        assert_eq!(
            ra_all.matches("renderModelRows(sessHasData);").count(),
            1,
            "renderAll 应独立调用 renderModelRows（不随会话条开关联动；携带会话级判定闸）"
        );
        assert_eq!(
            ra_all.matches("syncComposerPad();").count(),
            2,
            "renderAll 应在主路径与空轮分支各调用一次 syncComposerPad（留白统一计算）"
        );
        // V27 缓存维度特征：每轮条 ⟲ 后补 ✚ 缓存写段（fmtTokens 等宽补
        // 位）；会话条 ⟲ 段 V27–V29 曾带 token + 命中率后缀（分母 0 显
        // 示 —），V30 起命中率挪至底部额度行「缓存」段、⟲ 段回归纯
        // token；Σ 口径不变（tsum 不含 tcw）；V21 双口径回退路径
        // sessionTotals/sessionRunTotals 同步累计 tcw（sess 命中分支读
        // sv.cw），防双计判定两分支都覆盖
        let cw_lo = USAGE_JS.find("function barLineOf").expect("barLineOf 应存在");
        let cw_hi = USAGE_JS.find("function lineOf").expect("lineOf 应存在");
        let bar_body = &USAGE_JS[cw_lo..cw_hi];
        assert!(
            bar_body.contains("{ t: \" ⟲ \", c: CLS_DIM },")
                && bar_body.contains("{ t: fmtTokens(v.cr) }\n    ];")
                && bar_body.contains("parts.push({ t: \" ✚ \", c: CLS_DIM, title: W.cwTitle });")
                && bar_body.contains("parts.push({ t: fmtTokens(v.cw), title: W.cwTitle });"),
            "每轮条 ⟲ 缓存读段后条件渲染 ✚ 缓存写段（V35 起 cw 0 整段隐藏、>0 带词表 title，标签弱色）: {bar_body}"
        );
        assert!(
            USAGE_JS.contains("function cacheHitText")
                && USAGE_JS.contains("if (denom <= 0) return \"—\";"),
            "缓存命中率应在分母为 0 时显示 —"
        );
        assert!(
            sess_bar_body.contains("{ t: \"⟲ \" + fmtTokens(tcr) }")
                && sess_bar_body.contains("var sessHit = cacheHitText(tcr, tin);")
                && sess_bar_body
                    .contains("crSeg.parts.push({ t: \" (\" + sessHit + \")\", c: CLS_DIM });")
                && sess_bar_body.contains("if (tcw > 0) {")
                && sess_bar_body
                    .contains("segs.push({ t: \"✚ \" + fmtTokens(tcw), title: W.cwTitle });"),
            "V35 起会话条 ⟲ 段补弱色命中率 (NN%)（sess 口径 cr/(cr+up)，与底部条「缓存」段同源；分母 0 不入段），✚ 段 cw 0 隐藏、>0 带 title: {sess_bar_body}"
        );
        assert!(
            sess_bar_body.contains("var tcw = useSess\n      ? sv.cw || 0"),
            "sess 命中分支应读 sv.cw（防双计判定不变）：{sess_bar_body}"
        );
        assert!(
            sess_bar_body.contains("(totals ? totals.tcw : 0) + (runTotals ? runTotals.tcw : 0)"),
            "回退分支应累计 totals.tcw + runTotals.tcw（V21 双口径都覆盖）：{sess_bar_body}"
        );
        assert!(
            sess_bar_body.contains("var tsum = tin + tout + tcr;")
                && !sess_bar_body.contains("tin + tout + tcr + tcw"),
            "Σ = ↑+↓+⟲ 口径不变（✚ 段与命中率不改变 Σ 定义）：{sess_bar_body}"
        );
        let st_lo = USAGE_JS.find("function sessionTotals").expect("sessionTotals 应存在");
        let st_hi = USAGE_JS.find("function sessionRunTotals").expect("sessionRunTotals 应存在");
        let totals_body = &USAGE_JS[st_lo..st_hi];
        assert!(
            totals_body.contains("tcw: sv.cw || 0") && totals_body.contains("tcw += t.cw || 0;"),
            "sessionTotals 两口径都应累计缓存写（sess 行 cw / turns 逐轮 cw）：{totals_body}"
        );
        let rt_lo = USAGE_JS.find("function sessionRunTotals").expect("sessionRunTotals 应存在");
        let rt_hi = USAGE_JS
            .find("/* 活动轮判定（V8 DOM 驱动，V9 多容器化）")
            .expect("活动轮判定注释应存在");
        let run_totals_body = &USAGE_JS[rt_lo..rt_hi];
        assert!(
            run_totals_body.contains("tcw += r.cw || 0;") && run_totals_body.contains("tcw += r.sub.cw || 0;"),
            "sessionRunTotals 应累计 runs 行与并入 sub 的缓存写：{run_totals_body}"
        );
        assert!(
            USAGE_JS.contains("cw: t.cw || 0,"),
            "完成态每轮条应透传 turns 行 cw 字段"
        );
        assert!(
            USAGE_JS.contains("var cw = (r.cw || 0) + (s ? s.cw || 0 : 0);"),
            "live 行缓存写应同口径并入子代理 sub.cw"
        );
        // V25 速度聚合段特征：语言词典（zh → 均/快/慢，其余 → avg/fast/min）
        // + 样本 < 2 隐藏 + 取整紧凑格式；聚合只在完成态入行（liveLineOf
        // 不传 agg），且位于速度位之后、TTFT 之前
        // V26 语言判定与词典特征：三信号优先级（DOM lang/CJK 文本 →
        // navigator.language → 默认 zh；navigator.language 在 ZCode 的
        // WebView 里返回英文，V25 据此判定导致中文界面显示英文标签）+
        // 词表集中 WORDS 常量对象（均/快/慢 ↔ avg/fast/slow，废弃
        // best/worst/min/recent 混用）
        assert!(
            USAGE_JS.contains("var LANG_ZH = (function ()"),
            "V26 语言判定应保留缓存式 IIFE（三信号优先级，启动判一次）"
        );
        assert!(
            USAGE_JS.contains("/[\\u4e00-\\u9fff]/.test"),
            "语言判定应含页面文本 CJK 探测（直接反映用户所见界面语言）"
        );
        assert!(
            USAGE_JS.contains("document.documentElement.lang")
                && USAGE_JS.contains("navigator.language"),
            "语言判定应先 DOM 探测再 navigator.language"
        );
        assert!(
            USAGE_JS.contains("var WORDS = {")
                && USAGE_JS.contains("avg: \"均\"")
                && USAGE_JS.contains("avg: \"avg\"")
                && USAGE_JS.contains("latest: \"最近\"")
                && USAGE_JS.contains("latest: \"latest\"")
                && USAGE_JS.contains("n: \"笔\"")
                && USAGE_JS.contains("n: \"requests\""),
            "词表应集中在 WORDS 常量对象（中英两套，tooltip 同表）"
        );
        assert!(
            !USAGE_JS.contains("\"best\"")
                && !USAGE_JS.contains("\"worst\"")
                && !USAGE_JS.contains("\"recent\"")
                && !USAGE_JS.contains("· worst"),
            "best/worst/recent 标签应彻底废弃（统一 均/快/慢 ↔ avg/fast/slow）"
        );
        let agg_lo = USAGE_JS
            .find("function aggTextOf")
            .expect("aggTextOf 应存在");
        let agg_hi = agg_lo + USAGE_JS[agg_lo..].find("function fmtTps").unwrap();
        let agg_body = &USAGE_JS[agg_lo..agg_hi];
        assert!(
            agg_body.contains("sa.n >= 2"),
            "聚合段应在样本 < 2 时隐藏（单笔样本 均=快=慢 无信息量）: {agg_body}"
        );
        assert!(
            agg_body.contains("Math.round("),
            "聚合段取整（整数展示）: {agg_body}"
        );
        assert!(
            USAGE_JS.contains("agg: t.sa || null"),
            "完成态每轮条应传入数据端 sa 聚合"
        );
        assert!(
            USAGE_JS.contains("if (agg) {")
                && USAGE_JS.contains("parts.push({ t: \" · \" + agg, c: CLS_DIM });"),
            "聚合段应追加在速度位之后、TTFT 之前（V33 弱色层级）"
        );
        let live_lo = USAGE_JS
            .find("function liveLineOf")
            .expect("liveLineOf 应存在");
        let live_body = &USAGE_JS[live_lo..];
        let live_body = &live_body[..live_body.find("\n  }").expect("liveLineOf 结尾")];
        assert!(
            !live_body.contains("agg"),
            "进行中轮（live 行）不应携带聚合段: {live_body}"
        );
        // V25 模型分组速度行特征：数据索引 + 渲染函数 + 三档变色 +
        // 动态留白变量与迁移还原（挂载自愈路径不变）
        assert!(
            USAGE_JS.contains("lastModels ="),
            "V25 应索引数据端顶层 models 数组"
        );
        assert!(
            USAGE_JS.contains("data.models.slice(0, MODEL_ROWS_MAX)"),
            "模型行渲染端应兜底截断到 3 行"
        );
        assert!(USAGE_JS.contains("function renderModelRows"));
        assert!(USAGE_JS.contains("\"zbar-usage-mrow\""));
        assert!(USAGE_JS.contains("zbar-usage-tps-fast"));
        assert!(USAGE_JS.contains("TPS_FAST_MIN = 70") && USAGE_JS.contains("TPS_MID_MIN = 40"));
        // V29 P1 修复：模型行渲染带会话级判定闸——当前会话无数据时随
        // 会话条一并隐藏（多会话保活下新建任务页不再出现孤立模型行），
        // 会话条开关关闭照常显示的解耦语义不变
        assert!(
            USAGE_JS.contains("function renderModelRows(sessHasData)")
                && USAGE_JS.contains(
                    "if (!sessHasData || !modelRowsEnabled() || !validModelCount()) {"
                ),
            "模型行应携带会话级判定闸（当前会话无数据时移除容器）"
        );
        // V29 模型速度行独立容器与开关（自会话条拆出，用户核心诉求是会
        // 话条关闭时模型行照常显示）：独立标记/开关变量/兜底定位常量 +
        // 独立开关读取与单例创建/移除函数（挂载/兜底照会话条/额度行既
        // 有模式）
        assert!(
            USAGE_JS.contains("ATTR_MODELS = \"data-zbar-usage-models\"")
                && USAGE_JS.contains("VAR_MODEL_ROWS = \"--zbar-usage-model-rows\"")
                && USAGE_JS.contains("ATTR_MODELS_FIXED")
                && USAGE_JS.contains("MODELS_FALLBACK_BOTTOM_PX"),
            "模型行容器应有独立标记、开关变量与兜底定位常量（同会话条/额度行模式）"
        );
        assert!(
            USAGE_JS.contains("function modelRowsEnabled")
                && USAGE_JS.contains("function ensureModelsBox()")
                && USAGE_JS.contains("function removeModelsBox"),
            "模型行容器应有独立开关读取与单例创建/移除函数"
        );
        assert!(
            USAGE_JS.contains("box.setAttribute(ATTR_MODELS, \"\")")
                && USAGE_JS.contains("box.setAttribute(ATTR_MODELS_FIXED, \"\")"),
            "模型行容器应挂进 region 并在缺失时退回 body + fixed（幂等迁移）"
        );
        assert!(
            USAGE_JS.contains("VAR_PAD_TOP = \"--zbar-usage-pad-top\""),
            "V25 模型行留白应走 --zbar-usage-pad-top 变量"
        );
        assert!(
            USAGE_JS.contains("padding-top:var(")
                && USAGE_JS.contains("COMPOSER_PAD_TOP_PX + \"px) !important;"),
            "输入区顶部留白应读动态变量并保留 26px 兜底（V37 起规则尾接 padding-bottom 声明，不再以规则块收尾符结束）"
        );
        assert!(
            USAGE_JS.contains("function setRegionPad"),
            "V25 留白应有迁移/隐藏还原路径（多会话保活切换不残留）"
        );
        // V26 模型区底板与弹性布局特征：深色半透明底板（亮暗两档）+
        // 圆角 + 毛玻璃；会话条容器 stretch 对齐（模型区与会话行同左缘/
        // 宽度锚定）；模型名唯一可截断（ellipsis），数值区 flex:none
        // 永不截断；旧独立宽度上限删除
        assert!(
            USAGE_JS.contains("backdrop-filter:blur(6px)")
                && USAGE_JS.contains("border-radius:8px")
                && USAGE_JS.contains("background:var(--color-card, rgba(2,6,23,.72))"),
            "模型区应有半透明底板（毛玻璃 + 圆角；V33 起背景消费客户端 --color-card 语义变量，回退原深底）"
        );
        assert!(
            !USAGE_JS.contains("prefers-color-scheme: light")
                && USAGE_JS
                    .contains("border:1px solid var(--color-border, rgba(148,163,184,.28))"),
            "V33 起底板背景/边框由客户端语义变量接管（随 .dark 翻转），亮色媒体查询应删除"
        );
        assert!(
            USAGE_JS.contains("ATTR_MODELS + \"]:empty{display:none;}\""),
            "无模型行时空底板应整体隐藏"
        );
        assert!(
            USAGE_JS.contains("flex-direction:column;align-items:stretch;"),
            "会话条容器应 stretch 对齐（V29 起容器仅承载逐段 span 行，模型区独立居中）"
        );
        assert!(
            USAGE_JS.contains(
                "flex:1 1 auto;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;}"
            ),
            "模型名应 flex:1 1 auto + ellipsis（截断只发生在模型名）"
        );
        assert!(
            USAGE_JS.contains(".zbar-usage-mval{\" +\n      \"flex:none;white-space:nowrap;")
                && USAGE_JS.contains(".zbar-usage-mextra{\" +\n      \"flex:none;white-space:nowrap;"),
            "数值区与跟随小字应 flex:none（行尾数值永不截断，修复 V25 worst 被裁）"
        );
        assert!(
            !USAGE_JS.contains("max-width:min(70vw,420px)"),
            "模型行旧独立宽度上限应删除（宽度随底板与会话条锚定）"
        );
        assert!(
            USAGE_JS.contains("color:#34d399;")
                && USAGE_JS.contains(
                    "\"html:not(.dark) [\" + ATTR_MODELS + \"] .zbar-usage-tps-fast{color:#059669;}\""
                ),
            "深色三档变色恒用亮色系；V36 起浅色经 html:not(.dark) 覆盖为加深版（底板消费 --color-card 随主题翻转，浅色下恰为浅底）"
        );
        // V26/V29 留白精确计算与统一施加特征：按实际字号与底板几何求和
        // （修复 V25 固定 14px/行在字号调大或底板增高后压到输入面板），
        // V29 起由 syncComposerPad 统一计算（会话条行/模型底板两个开关
        // 独立叠加，每拍渲染尾部求一次值）；tooltip 固定两行模板（显式
        // \n，无悬挂分隔符、无冗余分档说明）
        assert!(
            USAGE_JS.contains("function usageFontSize")
                && USAGE_JS.contains("function modelsPlateHeight")
                && USAGE_JS.contains("function syncComposerPad")
                && USAGE_JS.contains("modelsPlateHeight(validModelCount(), fs)"),
            "留白应按实际字号与有效行数精确计算并由统一入口施加（两个开关独立叠加）"
        );
        assert!(
            USAGE_JS.contains("SESSION_BAR_TOP_PX + fs * 1.5"),
            "会话条行留白应含条 top 与行高（会话条开关独立计入）"
        );
        assert!(
            USAGE_JS.contains("--zbar-usage-font-size")
                && USAGE_JS.contains("function usageFontSize"),
            "留白计算的字号应读 --zbar-usage-font-size 实际值"
        );
        assert!(
            USAGE_JS.contains("fmtTps(m.tps)"),
            "模型行 title 应为固定两行模板（显式换行防悬挂分隔符）"
        );
        assert!(
            !USAGE_JS.contains("color by latest value"),
            "tooltip 冗余分档说明应删除"
        );
        assert!(
            !USAGE_JS.contains("innerHTML"),
            "模型行同样只用 textContent 写入外部字符串（无注入面）"
        );
        // V22 CTX 删除特征：渲染段、变色样式、sv.cp 消费点全部零残留
        //（注释中的历史记载不算特征）
        assert!(
            !USAGE_JS.contains("\"CTX \" + Math.round(cp)"),
            "会话条不应再有 CTX 百分比段（V22 已删）"
        );
        assert!(
            !USAGE_JS.contains(".zbar-ctx-warn") && !USAGE_JS.contains(".zbar-ctx-high{"),
            "CTX 三档变色样式应已移除"
        );
        assert!(!USAGE_JS.contains("sv.cp"), "sv.cp 消费点应已删除");
        // V21 会话累计双口径特征：sessIndex 索引 + sessionTotals 优先读
        // sess 全量合计行（数据缺失回退旧 turns 遍历）+ 命中时跳过 runs
        // 叠加（model_usage 全量已含 runs 行聚合，叠加即双计）
        assert!(
            USAGE_JS.contains("var sessIndex = {};"),
            "V21 应新增 sess 会话级统计索引"
        );
        assert!(
            USAGE_JS.contains("sv && sv.tt != null"),
            "sessionTotals 应优先读 sess 行（tt != null 判定新口径）"
        );
        assert!(
            USAGE_JS.contains("var useSess = !!(sv && sv.tt != null);"),
            "renderSessionBar 应按 sess 命中切换口径"
        );
        assert!(
            USAGE_JS.contains("runTotals = useSess ? null : sessionRunTotals(sessId);"),
            "sess 命中时不得叠加 runs 合计（防双计）"
        );
        // V19 每轮统计条开关特征：开关变量 + 镜像读取函数 + renderAll
        // 第二遍渲染循环的关闭分支（对全部轮节点 removeRow 并跳过
        // renderOne）
        assert!(
            USAGE_JS.contains("VAR_TURN_BAR = \"--zbar-usage-turn-bar\""),
            "每轮统计条开关应读 --zbar-usage-turn-bar 变量（variables.css 渲染 1/0）"
        );
        assert!(
            USAGE_JS.contains("function turnBarEnabled"),
            "每轮统计条开关应有独立读取函数（镜像 sessionBarEnabled）"
        );
        assert!(
            USAGE_JS.contains("removeRow(nodes[i], id);"),
            "renderAll 关闭分支应对全部轮节点 removeRow 并跳过 renderOne"
        );
        // V18 空会话修复特征：renderAll 无轮节点早退分支必须调用
        // removeBar，否则新建任务/清空对话后会话条停留在上一个会话的
        // 累计值永不消失。切片范围：renderAll 函数体内空轮分支起点至
        // 该分支的 return 为止
        let ra_lo = USAGE_JS
            .find("function renderAll")
            .expect("renderAll 应存在");
        let ra_body = &USAGE_JS[ra_lo..];
        let br_off = ra_body
            .find("if (!nodes.length)")
            .expect("renderAll 空轮分支应存在");
        let br_len = ra_body[br_off..]
            .find("return;")
            .expect("空轮分支应包含 return");
        let empty_branch = &ra_body[br_off..br_off + br_len];
        assert!(
            empty_branch.contains("removeBar();"),
            "V18 空轮分支应调用 removeBar（空会话时移除会话条，防残留上一会话累计）: {empty_branch}"
        );
        // V29 空会话页隐藏额度行与模型行容器（新建任务页不再出现孤立悬
        // 浮的注入层），并把两个注入层清干净后的留白同步还原
        assert!(
            empty_branch.contains("removeQuotaBar();")
                && empty_branch.contains("removeModelsBox();")
                && empty_branch.contains("syncComposerPad();"),
            "V29 空轮分支应移除额度行与模型行容器并同步留白: {empty_branch}"
        );
        // V17 会话选择修复特征：可见性 helper（保活面板隐藏时跳过）、
        // composer 挂载点选择函数；旧 querySelector 首中写法应已删除
        assert!(
            USAGE_JS.contains("function visibleEl"),
            "V17 应新增可见性判断 helper（保活面板隐藏时 rect 归零或 display:none）"
        );
        assert!(
            USAGE_JS.contains("function pickComposerRegion"),
            "composer 挂载点应经 pickComposerRegion 可见优先并跟随当前会话容器"
        );
        assert!(
            !USAGE_JS
                .contains("(anchor || document).querySelector(SEL_SESSION_ID)"),
            "currentSessionId 旧 querySelector 首中写法应已删除（多会话保活时拿到旧会话）"
        );
        // V4 样式参数化特征（保留）：两个变量消费 + V3 写死值兜底 + 等待态乘系数
        assert!(
            USAGE_JS.contains("var(--zbar-usage-font-size,10px)"),
            "字号应消费 --zbar-usage-font-size 并以 V3 写死值兜底"
        );
        assert!(
            USAGE_JS.contains("opacity:var(--zbar-usage-opacity,.55)"),
            "正常态不透明度应消费 --zbar-usage-opacity 并以 V3 写死值兜底"
        );
        assert!(
            USAGE_JS.contains(
                "opacity:calc(var(--zbar-usage-opacity,.55)*.636)"
            ),
            "等待态低透明应由正常态乘 0.636 系数得出（默认 ≈0.35 同 V3）"
        );
        assert!(
            !USAGE_JS.contains("font:400 10px/1.5"),
            "font 简写应已拆分独立声明以参数化字号"
        );
        // V10 统一固定结构（沿袭 V5 图标：× 请求数 / t/s / TTFT）：
        // barLineOf 三态共用，全部字段位恒定渲染、等宽补位
        let lo = USAGE_JS.find("function barLineOf").expect("barLineOf 应存在");
        let hi = USAGE_JS.find("function lineOf").expect("lineOf 应存在");
        let bar_body = &USAGE_JS[lo..hi];
        assert!(
            bar_body.contains("String(v.req).padStart(3, \" \")"),
            "req 应 padStart(3) 等宽补位：{bar_body}"
        );
        assert!(
            bar_body.contains("{ t: padSpeed(v.speed), c: spdCls },")
                && bar_body.contains("{ t: \" t/s\", c: CLS_DIM }"),
            "输出速度应为 t/s 且经 padSpeed 补位（V33 数值三档色、单位弱色）: {bar_body}"
        );
        assert!(
            bar_body.contains("parts.push({ t: \" · TTFT \", c: CLS_DIM });")
                && bar_body
                    .contains("parts.push({ t: (v.ttft == null ? \"–\" : v.ttft).padStart(4, \" \") });"),
            "首字延迟应显示为 TTFT 且 padStart(4)，进行中/缺失显示 – 占位: {bar_body}"
        );
        assert!(
            !bar_body.contains("v.est") && !bar_body.contains("~"),
            "每轮条不得保留 DOM 估算前缀：{bar_body}"
        );
        assert!(
            !bar_body.contains("+ \" req\"")
                && !bar_body.contains("\" tok/s\"")
                && !bar_body.contains("\"首字 \""),
            "旧格式渲染片段（N req / tok/s / 首字）不应残留：{bar_body}"
        );
        // V10 新特征：token 字段等宽补位、速度/TTFT 占位、极简启动行与
        // 行尾 … 标记删除
        assert!(
            USAGE_JS.contains("padStart(5, \" \")"),
            "token 字段应经 fmtTokens padStart(5) 等宽补位"
        );
        assert!(
            USAGE_JS.contains("function padSpeed")
                && USAGE_JS.contains("return \"  – \";")
                && USAGE_JS.contains("s.quality === \"request_average\""),
            "速度应显示确认值、近似值前缀与等待占位"
        );
        assert!(
            USAGE_JS.contains("\"–\""),
            "TTFT 进行中/缺失应显示 – 占位"
        );
        assert!(
            !USAGE_JS.contains("function liveStartLineOf"),
            "启动窗口极简行函数应已删除（合并进统一格式函数）"
        );
        assert!(
            !USAGE_JS.contains("WAIT_TEXT"),
            "行尾 … 进行中标记应已删除（TTFT 位 – 已表达进行中）"
        );
        // 完成态：经统一格式函数渲染，不再逐段拼接。
        let lo = USAGE_JS.find("function lineOf").expect("lineOf 应存在");
        let hi = USAGE_JS
            .find("function liveLineOf")
            .expect("liveLineOf 应存在（lineOf 后下一个函数，V16 起 titleOf 已删）");
        let line_body = &USAGE_JS[lo..hi];
        assert!(
            line_body.contains("barLineOf({") && !line_body.contains("est:"),
            "完成态应经统一格式函数渲染且不得带估算字段：{line_body}"
        );
        assert!(
            !line_body.contains("parts.push")
                && !line_body.contains("+ \" req\"")
                && !line_body.contains("\" tok/s\"")
                && !line_body.contains("\"首字 \""),
            "完成态逐段拼接与旧格式渲染片段不应残留：{line_body}"
        );
        // V5 会话级实时统计条特征：独立渲染函数 + 实机调参常量 + 防重复
        // 标记 + 会话锚点 + tabular-nums + pointer-events
        assert!(USAGE_JS.contains("renderSessionBar"), "会话条渲染函数应存在");
        assert!(
            USAGE_JS.contains("SESSION_BAR_BOTTOM_PX"),
            "兜底 bottom 实机调参常量应存在"
        );
        // V13 会话条挂载进输入区容器 + CSS 留白定位特征：region relative
        // + padding-top 留白规则、条 absolute 居中、幂等迁移挂载、兜底
        // fixed 属性切换；V11 输入区上移规则应已删除（输入框还原原位）。
        // V25：留白改读 --zbar-usage-pad-top（兜底 26px），模型行显示时由
        // renderSessionBar 抬高，隐藏/迁移时还原
        assert!(
            USAGE_JS.contains(".chat-composer-region{position:relative !important;padding-top:var("),
            "输入区容器应有 relative + 顶部留白规则（V25 起读动态变量、兜底 26px）"
        );
        assert!(
            USAGE_JS.contains("COMPOSER_PAD_TOP_PX = 26"),
            "留白高度常量注释应存在（CSS 内写死 26px，两处同步）"
        );
        assert!(
            USAGE_JS.contains("position:absolute;top:4px;left:50%;transform:translateX(-50%);"),
            "会话条应 absolute 定位住进输入区容器顶部留白（不再 fixed/bottom）"
        );
        assert!(
            USAGE_JS.contains("region.appendChild(bar)")
                && USAGE_JS.contains("var SEL_COMPOSER = \".chat-composer-region\""),
            "会话条应幂等迁移挂载进输入区容器（SEL_COMPOSER 须有真实定义，注释提及不算）"
        );
        assert!(
            USAGE_JS.contains("ATTR_SESSION_BAR_FIXED"),
            "region 缺失兜底路径应有 fixed 切换标记属性"
        );
        assert!(
            USAGE_JS.contains("SESSION_BAR_BOTTOM_PX = 96"),
            "region 缺失退回 fixed 的兜底 bottom 应为 96"
        );
        // 会话条零测量定位：renderSessionBar 函数体内不得再有
        // getBoundingClientRect（V12 动态测量废弃；限定函数体范围断言
        // ——模板头 V12 历史注释合法提及该字样）
        let sb_lo = USAGE_JS
            .find("function renderSessionBar")
            .expect("renderSessionBar 应存在");
        let sb_hi = sb_lo
            + USAGE_JS[sb_lo..]
                .find("function renderAll")
                .expect("renderAll 应存在");
        let session_body = &USAGE_JS[sb_lo..sb_hi];
        assert!(
            !session_body.contains("getBoundingClientRect"),
            "会话条应零测量定位（V12 动态测量已废弃）：{session_body}"
        );
        assert!(
            !USAGE_JS.contains("addEventListener(\"resize\""),
            "absolute 随文档流自适应，V12 的 resize 监听应已删除"
        );
        assert!(
            !USAGE_JS.contains("var SESSION_BAR_ABOVE_PX"),
            "V12 测量间距常量定义应已删除"
        );
        assert!(
            !USAGE_JS.contains("var COMPOSER_GAP_PX")
                && !USAGE_JS.contains("chat-composer-region{padding-bottom"),
            "V11 输入区上移规则与 COMPOSER_GAP_PX 常量定义应已删除（输入框还原原位）"
        );
        assert!(!USAGE_JS.contains("TOKEN_CHARS"), "不得保留 DOM 文本 token 估算系数");
        assert!(!USAGE_JS.contains("DYN_TICK_MS"), "不得保留 DOM 文本采样定时器");
        assert!(!USAGE_JS.contains("SPEED_WINDOW_MS"), "不得保留速度滑动窗口估算");
        assert!(!USAGE_JS.contains("textContent.length"), "不得从 DOM 文本长度估算 token");
        assert!(USAGE_JS.contains("request_average"), "应保留请求平均速度质量标记");
        assert!(
            USAGE_JS.contains("data-zbar-usage-session"),
            "会话条防重复挂载标记应存在"
        );
        assert!(
            USAGE_JS.contains("[data-session-id]"),
            "会话 id 锚点选择器应存在"
        );
        assert!(
            USAGE_JS.contains("t.sess !== sessId"),
            "会话累计应按 sess 过滤 turns"
        );
        // V6 生成过程实时跳动特征：runIndex 数据索引、live 行渲染、
        // 数据驱动的估算目标检测、会话条 run 合计并入
        assert!(
            USAGE_JS.contains("runIndex[t.umid] = r") || USAGE_JS.contains("runIndex[r.umid] = r"),
            "应建立进行中轮 runIndex（umid → run 行）"
        );
        assert!(
            USAGE_JS.contains("function liveLineOf"),
            "进行中轮行渲染函数应存在"
        );
        assert!(
            !USAGE_JS.contains("LIVE_TITLE"),
            "V16 起进行中轮行不应再有悬浮 title 提示（已整体移除）"
        );
        assert!(
            USAGE_JS.contains("data-zbar-usage-row-state\", \"live\""),
            "进行中轮行应有独立 state 标记（与完成/等待态区分）"
        );
        assert!(
            USAGE_JS.contains("function findLiveNodes"),
            "活动轮判定/估算目标检测函数应存在（findLiveNodes，V9 多容器）"
        );
        assert!(
            USAGE_JS.contains("function sessionRunTotals"),
            "会话条应并入进行中 run 合计"
        );
        assert!(
            USAGE_JS.contains("r.psess !== sessId"),
            "子代理进行中轮应经 psess 并入父会话累计"
        );
        assert!(
            USAGE_JS.contains("(data && data.runs) || []"),
            "runs 缺失（旧导出文件）应兜底空数组"
        );
        assert!(
            USAGE_JS.contains("req: req,"),
            "进行中轮行应显示 run 真实请求数聚合（V9 起含并入的子代理 req，V10 经统一格式函数补位）"
        );
        // V8 启动窗口实时渲染特征（活动轮判定 DOM 驱动，修复发消息到
        // 首笔模型请求完成之间 live 条/估算目标/会话条全空白的启动窗口），
        // V9 多容器化：每容器取 DOM 顺序最后 umid 节点，不在 index 即该
        // 会话活动轮
        assert!(
            USAGE_JS.contains("if (!id || index[id]) continue"),
            "活动轮判定应为 DOM 驱动：会话 DOM 最后 umid 不在 index 即活动轮"
        );
        // 判定无 runs 依赖路径：findLiveNodes 函数体内不得引用 runIndex
        //（runIndex 是否命中只区分启动窗口/runs 阶段，不是判定前提），
        // 也不得依赖实机不可靠的 data-running
        let fl_lo = USAGE_JS
            .find("function findLiveNodes")
            .expect("findLiveNodes 应存在");
        let fl_hi = fl_lo
            + USAGE_JS[fl_lo..]
                .find("function ensureBar")
                .expect("ensureBar 应存在");
        let find_body = &USAGE_JS[fl_lo..fl_hi];
        assert!(
            !find_body.contains("runIndex["),
            "活动轮判定不得依赖 runs 数据（runIndex 命中不是前提）：{find_body}"
        );
        assert!(
            !find_body.contains("ATTR_RUNNING"),
            "活动轮判定不得依赖 data-running（实机取值存疑）：{find_body}"
        );
        // 启动窗口活动轮行：V10 起经统一格式函数渲染 0 / 估算值（极简
        // 行已删除）；renderOne 接收 renderAll 统一判定的活动轮 Map
        //（V9 多容器：会话 id → 活动轮节点）。V16 起无悬浮 title
        assert!(
            !USAGE_JS.contains("LIVE_START_TITLE"),
            "V16 起启动窗口活动轮行不应再有悬浮 title 提示（已整体移除）"
        );
        assert!(
            USAGE_JS.contains("function renderOne(node, turnId, isLast, activeMap)"),
            "renderOne 应接收统一判定的活动轮 Map（V9 多容器）"
        );
        // data-running 不再是任何渲染路径的必要条件：等待态判定与兜底
        // 选择器删除，仅保留 attributeFilter 刷新信号
        assert!(
            !USAGE_JS.contains("SEL_RUNNING_TURN"),
            "data-running 兜底选择器应已删除"
        );
        assert!(
            !USAGE_JS.contains("=== \"true\""),
            "不得再以 data-running 取值作为渲染必要条件"
        );
        assert!(
            USAGE_JS.contains("attributeFilter: [ATTR_TURN_ID, ATTR_RUNNING]"),
            "data-running 应仅保留为 MutationObserver 刷新信号"
        );
        // 会话条：无确认数据且无活动轮时不渲染；有活动轮时显示确认等待
        // 占位，不把 DOM 文本或未确认输出写入 Σ。
        assert!(
            USAGE_JS.contains("!totals && !runTotals && !active"),
            "会话条放弃渲染条件应含无活动轮判定（启动窗口即时显示）"
        );
        // 会话条速度段固定消费 sess 行的请求级快照；没有快照时显示 –。
        // V34：速度值 span 补三档色 class（无效快照/等待占位不着色），
        // 可见文本拼接串不变（全串比对零 DOM 机制保持）。
        assert!(
            USAGE_JS.contains("var tsum = tin + tout + tcr;")
                && USAGE_JS.contains("var sessionSpeed = sv && sv.speed ? sv.speed : null;")
                && USAGE_JS.contains("if (!activeId || !runIndex[activeId]")
                && USAGE_JS.contains("{ t: padSpeed(sessionSpeed), c: sessionSpdCls },")
                && USAGE_JS.contains("var sessionSpdCls =")
                && !USAGE_JS.contains("fmtTokens(est."),
            "会话条应只显示确认累计与请求级速度（V34 起速度值带三档色 class）"
        );
        assert!(
            USAGE_JS.contains("String(treq).padStart(3, \" \")"),
            "会话条 Σ req 应 padStart(3) 等宽补位"
        );
        assert!(
            !USAGE_JS.contains("(runTotals ? runTotals.tout : 0) + est.tok"),
            "会话条 Σ ↓ 真实数字不得再叠加估算（估算已移入动态段）"
        );
        assert!(
            USAGE_JS.contains("VAR_SESSION_BAR = \"--zbar-usage-session-bar\""),
            "会话条开关应读 --zbar-usage-session-bar 变量（variables.css 渲染 1/0）"
        );
        assert!(
            USAGE_JS.contains("position:fixed"),
            "会话条兜底路径应保留 fixed 定位（region 缺失时挂 body 退回旧悬浮方式，不占消息布局）"
        );
        assert!(
            USAGE_JS.contains("pointer-events:none"),
            "会话条应 pointer-events:none 防挡输入区交互"
        );
        assert!(
            USAGE_JS.contains("font-variant-numeric:tabular-nums"),
            "会话条应 tabular-nums 防数字跳动宽度抖动"
        );
        // DOM 文本估算已移除：无估算函数、无独立采样定时器、无估算消费
        // 段，只保留后端确认的请求聚合。
        assert!(
            !USAGE_JS.contains("function dynEstimate")
                && !USAGE_JS.contains("function dynSegmentText")
                && !USAGE_JS.contains("clearInterval"),
            "不得保留 DOM 文本估算函数或独立速度定时器"
        );
        // V9 子代理消耗实时化特征：document 级扫描（子代理详情面板与主
        // 对话同 document，面板在 workspace-main 锚点之外）、多容器活动
        // 轮判定、主轮 live 行 sub 合计、会话条 Σ 跳过 m:1 子代理行并
        // 计入主行 sub；速度仍由后端请求级快照提供。
        assert!(
            USAGE_JS.contains("var nodes = document.querySelectorAll(SEL_TURN)"),
            "renderAll 应 document 级扫描每轮节点（覆盖锚点外的子代理面板）"
        );
        assert!(
            USAGE_JS.contains("containers[i].querySelectorAll(SEL_TURN)")
                && USAGE_JS.contains("new Map()"),
            "活动轮判定应遍历所有会话容器并以 Map 返回（sess → 活动轮节点）"
        );
        assert!(
            !USAGE_JS.contains("dyn.targets") && !USAGE_JS.contains("function syncDyn"),
            "不得保留按 DOM 文本采样的多目标估算器"
        );
        assert!(
            !USAGE_JS.contains("function estFor") && USAGE_JS.contains("function sessOf"),
            "活动轮应按节点所属会话判定且不创建估算目标"
        );
        assert!(
            USAGE_JS.contains("var s = r.sub;")
                && USAGE_JS.contains("req: req,")
                && !USAGE_JS.contains("est: true"),
            "主轮 live 行数字段应合计并入的子代理聚合 sub"
        );
        assert!(
            !USAGE_JS.contains("liveTitleOf")
                && !USAGE_JS.contains("\"含子代理 \" + r.sub.n + \" 轮：\""),
            "V16 起每轮行不应再有悬浮 title（liveTitleOf 及子代理分解明细应已删除）"
        );
        assert!(
            USAGE_JS.contains("if (!r || r.m) continue;"),
            "会话条 runs 合计应跳过 m:1 子代理行（其值已并入主轮行 sub 防双计）"
        );
        assert!(
            USAGE_JS.contains("if (r.sub) {"),
            "会话条应把主轮行并入的子代理 sub 计入 Σ"
        );
        assert!(
            USAGE_JS.contains("var active = activeMap.has(sessId);"),
            "会话条活动轮判定应按本会话 id 从活动轮 Map 取（V9 多容器）"
        );
        assert!(
            !USAGE_JS.contains("dyn.node")
                && !USAGE_JS.contains("function startDyn(")
                && !USAGE_JS.contains("function findLiveNode("),
            "V8 单节点活动轮判定的旧接口应已移除"
        );
        // 不再维护“枯萎”目标：活动轮的生命周期完全由 DOM 节点与确认
        // 数据索引决定。
        assert!(
            !USAGE_JS.contains("STALE_MS") && !USAGE_JS.contains("function staleHolds"),
            "不得保留基于文本增长的枯萎判定"
        );
        // 用量条核心特征：同目录数据文件推导 + 统计口径、速度快照与等待态
        assert!(USAGE_JS.contains("usage-data.js"));
        assert!(USAGE_JS.contains("speed"));
        assert!(!USAGE_JS.contains("ttft * 10 >= d * 9"), "速度不得在前端重算");
        assert!(USAGE_JS.contains("data-zbar-usage-row"), "统计条防重复标记应存在");
        // V2 实机修复特征：数据版本校验、umid 匹配键、DOM 顺序最后节点
        // 渲染、ts 未变跳过重建与重渲染
        assert!(USAGE_JS.contains("data.v !== 2"), "数据版本校验应存在");
        assert!(USAGE_JS.contains("index[t.umid] = t"), "索引应以 umid 为匹配键");
        assert!(
            USAGE_JS.contains("lastOf[id] === nodes[i]"),
            "同轮多节点应只在 DOM 顺序最后一个节点渲染"
        );
        assert!(USAGE_JS.contains("data.ts === lastTs"), "ts 未变应跳过重渲染");
        // V3 实机修复特征：scheduled 复位移入 finally（renderAll 抛异常
        // 不再死锁渲染管线）+ 低频兜底渲染（不经过 scheduled 检查直接重渲）
        assert!(
            USAGE_JS.contains("} finally {"),
            "scheduleRender 复位应在 finally 中，保证异常不死锁"
        );
        assert!(
            USAGE_JS.contains("FALLBACK_RENDER_MS"),
            "低频兜底渲染定时器应存在"
        );

        // token 半透明化：浅色 :root 与深色 .dark 两套均定义
        // background / background-alt / panel 三个 token
        assert!(THEME_CSS.contains(".dark"));
        for token in [
            "--color-background:",
            "--color-background-alt:",
            "--color-panel:",
        ] {
            assert_eq!(
                THEME_CSS.matches(token).count(),
                2,
                "{token} 应在 :root 与 .dark 各定义一次"
            );
        }
        // 引用 neutral 原色不硬编码
        assert!(THEME_CSS.contains("var(--color-neutral-50)"));
        assert!(THEME_CSS.contains("var(--color-neutral-100)"));
        assert!(THEME_CSS.contains("var(--color-neutral-800)"));
        assert!(THEME_CSS.contains("var(--color-neutral-900)"));

        // V5 全局 token 分层：8 条全局底色 token（:root 与 .dark 各 4 条）
        // 全部改由固定氛围变量 --zbar-base-alpha（缺省兜底 0.25）驱动，
        // 与对话区/侧栏两个滑块彻底解绑，拖任一滑块不再牵连顶栏、
        // 右侧面板、卡片等全局底消费方
        assert_eq!(
            THEME_CSS.matches("var(--zbar-base-alpha, 0.25)").count(),
            8,
            "全局底色 token 应在 :root 与 .dark 各 4 条消费 --zbar-base-alpha"
        );
        assert!(
            !THEME_CSS.contains("var(--zbar-panel-opacity, 0.85)")
                && !THEME_CSS.contains("var(--zbar-sidebar-opacity, 0.85)"),
            "全局 token 不得再由滑块变量驱动"
        );
        // 每条全局 token 声明的取值只消费 --zbar-base-alpha，
        // 不得混入任何滑块变量（全局底与滑块彻底解绑）
        for token in [
            "--color-background:",
            "--color-background-alt:",
            "--color-panel:",
            "--color-sidebar:",
        ] {
            let mut rest = THEME_CSS;
            for _ in 0..2 {
                let start = rest.find(token).unwrap();
                let tail = &rest[start..];
                let value = &tail[..tail.find(';').unwrap()];
                assert!(
                    value.contains("var(--zbar-base-alpha, 0.25)"),
                    "{token} 应由固定氛围变量驱动：{value}"
                );
                assert!(
                    !value.contains("--zbar-panel-opacity")
                        && !value.contains("--zbar-sidebar-opacity"),
                    "{token} 不得消费滑块变量：{value}"
                );
                rest = &tail[value.len()..];
            }
        }

        // V6 三滑块唯一作用区：侧栏滑块变量只由侧栏容器元素规则在浅色、
        // 深色各消费一次；对话区滑块变量只由对话列元素规则各消费一次；
        // 右栏滑块变量只由右栏其余面板元素规则各消费一次
        // （--color-sidebar 已改由氛围变量驱动，不再消费滑块变量）
        assert_eq!(
            THEME_CSS.matches("--color-sidebar:").count(),
            2,
            "--color-sidebar 应在 :root 与 .dark 各定义一次"
        );
        assert_eq!(
            THEME_CSS.matches("var(--zbar-sidebar-opacity, 0)").count(),
            2,
            "侧栏滑块变量应只由侧栏容器元素规则在浅色、深色各消费一次"
        );
        assert_eq!(
            THEME_CSS.matches("var(--zbar-panel-opacity, 0)").count(),
            2,
            "对话区滑块变量应只由对话列元素规则在浅色、深色各消费一次"
        );
        assert_eq!(
            THEME_CSS.matches("var(--zbar-sidebar-right-opacity, 0)").count(),
            2,
            "右栏滑块变量应只由右栏其余面板元素规则在浅色、深色各消费一次"
        );
        assert!(THEME_CSS.contains("var(--color-neutral-950)"));

        // V9 主区域元素级规则 + V10 描边规则（选择器同批复用）：
        // 侧栏容器浅色、深色各 2 条（背景 1 + 描边 1）；对话区四面板
        // 组与右栏组同理
        assert_eq!(
            THEME_CSS.matches("#sidebar").count(),
            4,
            "侧栏容器应在浅深各 2 条规则（V9 背景 + V10 描边）"
        );
        // V6 修正：#content 实为"对话列 + 右侧面板"整体容器，其元素规则
        // 已删除，对话区滑块不再作用其上
        assert_eq!(
            THEME_CSS.matches("#content").count(),
            0,
            "#content 元素规则应已移除（否则会牵连右侧面板）"
        );
        // V9 对话区四面板组：四个面板值各出现 8 次——V9 对话区规则
        // 浅深各 1 次 + V9 右栏 :not 链浅深各 1 次 + V10 描边规则
        // （对话区段与 :not 链同批复用）浅深各 2 次（缺一即归属串味）
        for pane in [
            "workspace-main",
            "conversation-column",
            "conversation",
            "terminal",
        ] {
            let needle = format!("{pane}\"]");
            assert_eq!(
                THEME_CSS.matches(&needle).count(),
                8,
                "面板值 {pane} 应在 V9 背景规则、右栏 :not 链与 V10 描边规则中浅深共 8 次"
            );
        }
        // 对话区规则与右栏规则以各自选择器末段形态区分：后跟 " {" 的
        // terminal 选择器是对话区组末段（V10 描边组中 terminal 后随
        // 逗号、不新增该形态），右栏组末段是空态选择面板容器类
        assert_eq!(
            THEME_CSS.matches("terminal\"] {").count(),
            2,
            "对话区四面板组元素规则应在浅色与深色各一条"
        );
        assert_eq!(
            THEME_CSS.matches(".side-pane-open-tab-shell {").count(),
            4,
            "右栏元素规则应在浅深各 2 条（V9 背景 + V10 描边，:not 链 + 空态选择面板容器类）"
        );
        // V7 修正：选择器属性名为 data-pane-id（按带括号的选择器形态
        // 计数，注释中的裸属性名不影响结果），旧写法须在选择器中归零
        assert_eq!(
            THEME_CSS.matches("[data-panel-id").count(),
            0,
            "旧属性名 data-panel-id 不应再出现在任何选择器中"
        );
        assert_eq!(
            THEME_CSS.matches("[data-pane-id").count(),
            36,
            "V9 对话区 2 条规则各 4 次 + 右栏 2 条规则各 5 次 = 18；V10 描边浅深两组结构相同再 +18"
        );
        assert!(THEME_CSS.contains("html.dark #sidebar"));
        assert!(THEME_CSS.contains("html.dark [data-pane-id=\"workspace-main\"]"));
        assert!(THEME_CSS.contains("html.dark [data-pane-id=\"conversation-column\"]"));
        assert!(THEME_CSS.contains("html.dark .side-pane-open-tab-shell"));
        assert!(THEME_CSS.contains(
            "html.dark [data-pane-id]:not([data-pane-id=\"workspace-main\"]):not([data-pane-id=\"conversation-column\"]):not([data-pane-id=\"conversation\"]):not([data-pane-id=\"terminal\"])"
        ));
        // 每条元素规则只消费各自滑块变量并带 !important，防串味：
        // 侧栏容器规则只认侧栏滑块变量、对话区四面板组规则只认对话区
        // 滑块变量、右栏规则只认右栏滑块变量，互相不得混入其他滑块变量
        for (sel, own, others) in [
            (
                "#sidebar",
                "var(--zbar-sidebar-opacity, 0)",
                ["--zbar-panel-opacity", "--zbar-sidebar-right-opacity"],
            ),
            (
                "terminal\"] {",
                "var(--zbar-panel-opacity, 0)",
                ["--zbar-sidebar-opacity", "--zbar-sidebar-right-opacity"],
            ),
            (
                ".side-pane-open-tab-shell {",
                "var(--zbar-sidebar-right-opacity, 0)",
                ["--zbar-panel-opacity", "--zbar-sidebar-opacity"],
            ),
        ] {
            let mut rest = THEME_CSS;
            for _ in 0..2 {
                let start = rest.find(sel).unwrap();
                let tail = &rest[start..];
                let value = &tail[..tail.find(';').unwrap()];
                assert!(value.contains(own), "{sel} 规则应由自身滑块驱动：{value}");
                for other in others {
                    assert!(
                        !value.contains(other),
                        "{sel} 规则不得消费其他滑块变量：{value}"
                    );
                }
                assert!(
                    value.contains("!important"),
                    "{sel} 规则应带 !important 压过源样式：{value}"
                );
                rest = &tail[value.len()..];
            }
        }

        // V10 文字描边：浅深两套各一条容器级规则（描边由文字继承，
        // 选择器组与 V9 主区域背景规则同批），强度全部消费
        // --zbar-text-shadow（variables.css 渲染真值 + 热重载；兜底
        // 0 仅作旧 variables.css 容错，0=关闭时 alpha 为 0 不可见）
        assert_eq!(
            THEME_CSS.matches("text-shadow:").count(),
            2,
            "文字描边规则应在浅色与深色各一条"
        );
        assert_eq!(
            THEME_CSS.matches("var(--zbar-text-shadow").count(),
            4,
            "每条描边规则的模糊半径与 alpha 各消费一次强度变量"
        );
        // 深色黑描边 / 浅色白描边对称，模糊半径随强度温和缩放
        assert!(
            THEME_CSS.contains("rgba(0, 0, 0, var(--zbar-text-shadow, 0))"),
            "深色主题应为黑色描边"
        );
        assert!(
            THEME_CSS.contains("rgba(255, 255, 255, var(--zbar-text-shadow, 0))"),
            "浅色主题应为白色描边"
        );
        assert_eq!(
            THEME_CSS.matches("calc(3px + (var(--zbar-text-shadow, 0) * 5px))").count(),
            2,
            "模糊半径应随强度缩放且浅深各消费一次"
        );
        // 描边规则必须位于全部 V9 规则之后（文件末尾追加，V9 规则
        // 未被改写移动）：V9 最后一条规则（深色右栏背景）先于 V10 注释头
        let v9_last = THEME_CSS
            .find("html.dark .side-pane-open-tab-shell {\n  background-color:")
            .expect("V9 深色右栏背景规则应存在");
        let v10_block = THEME_CSS
            .find("文字可读性：壁纸过亮/过暗时给前景文字补描边")
            .expect("V10 描边注释头应存在");
        assert!(
            v9_last < v10_block,
            "V10 规则只能追加在 V9 规则之后，不得改写或移动既有规则"
        );

        // 热重载核心逻辑特征：data 标记定位 + href 兜底匹配 + 时间戳重读
        assert!(EFFECTS_JS.contains("link[data-zbar-variables]"));
        assert!(EFFECTS_JS.contains("link[href*=\"variables.css\"]"));
        assert!(EFFECTS_JS.contains("?t=\" + Date.now()"));
        // V5 撤销 theme.css 每秒 cache-bust：样式表 href 变更存在
        // "卸载失效 → 异步加载 → 恢复"窗口，失效窗口内三区域背景/文字
        // 描边规则整体失效，背景闪回原生底色形成周期闪烁；theme.css
        // 模板升级改由面板"重启 ZCode"冷启动完全重载
        assert!(
            !EFFECTS_JS.contains("reloadThemeLink") && !EFFECTS_JS.contains("findThemeLink"),
            "theme.css 热重载函数应已整体删除"
        );
        // poll 竞态防御：先快照后重读（避免快照撞上本轮重载失效窗口）+
        // 空值防御（任一变量读到空串视为失效窗口，本轮直接返回）
        assert!(
            EFFECTS_JS.contains("视为失效窗口"),
            "poll 空值防御注释应存在"
        );
        assert!(
            EFFECTS_JS.contains("now[VAR_NAMES[i]] === \"\""),
            "poll 变量空串检查应存在"
        );
        // 先快照后重读的顺序：reloadVarsLink() 调用（仅 poll 中一处）
        // 必须位于 snapshotOf() 快照语句之后
        assert!(
            EFFECTS_JS.find("var now = snapshotOf();")
                < EFFECTS_JS.find("reloadVarsLink();"),
            "poll 应先取快照再重读 variables.css"
        );
        // 换源淡入（可重复调用）与遮罩层
        assert!(EFFECTS_JS.contains("transition:opacity"));
        assert!(EFFECTS_JS.contains("rgba(0,0,0,"));

        // V3 图片支持特征：按扩展名分派 video/img 元素、img 同层样式、
        // 类型切换重建、播放速率仅视频
        assert!(EFFECTS_JS.contains("function kindOf(url)"));
        assert!(EFFECTS_JS.contains("createImage"));
        assert!(EFFECTS_JS.contains("createElement(\"img\")"));
        assert!(EFFECTS_JS.contains("mediaKind !== kind"));
        assert!(EFFECTS_JS.contains("mediaKind !== \"video\""));
        // img 的 onload 淡入语义
        assert!(EFFECTS_JS.contains("i.addEventListener(\"load\", onReady)"));

        // V6 壁纸层总开关：开关变量在 VAR_NAMES 快照内（开关变化才能被
        // sameSnapshot 感知）；空串/缺失按开启（仅显式 "0" 关闭）；关闭走
        // unmount + 记住 off 态；重开按已知指向恢复；off 分支必须在自愈
        // 分支之前提前返回（off 期间不得重新挂载）
        assert!(
            EFFECTS_JS.contains("VAR_WALLPAPER_ENABLED = \"--zbar-wallpaper-enabled\""),
            "壁纸层总开关应读 --zbar-wallpaper-enabled（variables.css 渲染 1/0）"
        );
        let names_block = &EFFECTS_JS[EFFECTS_JS
            .find("var VAR_NAMES = [")
            .expect("VAR_NAMES 数组应存在")
            ..EFFECTS_JS
                .find("var VAR_WALLPAPER_ENABLED")
                .expect("开关常量声明应存在")];
        assert!(
            names_block.contains("\"--zbar-wallpaper-enabled\""),
            "开关变量应进入 VAR_NAMES 快照，否则开关变化不会触发重新应用"
        );
        assert!(
            EFFECTS_JS.contains("if (VAR_NAMES[i] === VAR_WALLPAPER_ENABLED) continue;"),
            "开关变量应排除出空值防御（旧 variables.css 无此变量恒为空串）"
        );
        assert!(
            EFFECTS_JS.contains("var wallpaperOn = cssVar(VAR_WALLPAPER_ENABLED, \"1\") !== \"0\";"),
            "开关应按 usage_* 惯例读取：仅显式 0 视为关闭"
        );
        assert!(
            EFFECTS_JS.contains("          unmount();\n          wallpaperOff = true;"),
            "关闭态应移除注入三层并记住 off"
        );
        assert!(
            EFFECTS_JS.contains(
                "var resumeUrl = urlOf(now[\"--zbar-wallpaper-url\"]) || currentUrl;"
            ),
            "由关切开应按快照里的最新指向恢复（关闭期间换源不停留旧源）"
        );
        assert!(
            EFFECTS_JS.contains("if (wallpaperOn && !wallpaperOff && !dead && currentUrl && !mounted()) {"),
            "自愈分支应以 wallpaperOn 与 !wallpaperOff 双前提判定：off 期间不得重新挂载，\
             且不能只看 wallpaperOn（reloadVarsLink 失效窗口下 enabled 空串回退会误判为开启）"
        );
        // 防回归：自愈分支不得回退为只判 wallpaperOn（P1：off 稳态卡死 +
        // 重开双重挂载）
        assert!(
            !EFFECTS_JS.contains("if (wallpaperOn && !dead"),
            "自愈分支不得缺少 !wallpaperOff 前提"
        );
        // 关闭分支必须位于 reloadVarsLink 之后：off 只拦壁纸相关应用，
        // variables.css 的每秒 cache-bust 照常执行（usage.js 等热重载依赖）
        assert!(
            EFFECTS_JS.find("reloadVarsLink();") < EFFECTS_JS.find("if (!wallpaperOn) {"),
            "关闭分支应在 reloadVarsLink 之后，关闭壁纸不得停掉其余参数热重载"
        );
    }

    #[test]
    fn 注入_link带data标记() {
        let dir = test_dir("data-attrs");
        let index = dir.join("index.html");
        fs::write(&index, SAMPLE_HTML).unwrap();

        let html = apply_inject(&index, &dir).unwrap();
        // effects.js 热重载定位 variables.css 依赖 data-zbar-variables
        assert!(html.contains("data-zbar-variables"), "应注入 data-zbar-variables: {html}");
        assert!(html.contains("data-zbar-theme"));
        assert!(html.contains("data-zbar-effects"));
        // usage.js（用量统计条）注入行带 data-zbar-usage 标记
        assert!(html.contains("data-zbar-usage"), "应注入 data-zbar-usage: {html}");

        // 标记都在注入注释块内：剥离逻辑不受影响，还原后无残留
        assert_eq!(strip_inject_blocks(&html), SAMPLE_HTML);

        let _ = fs::remove_dir_all(&dir);
    }
}
