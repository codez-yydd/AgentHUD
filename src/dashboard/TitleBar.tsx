/**
 * 自绘标题栏：主窗口 decorations: false（见 src-tauri/src/main_window.rs）
 * 之后替代系统标题栏——左侧品牌区（logo + ZBar + 副标题，原侧栏 db-brand
 * 上移），右侧窗口控制按钮（最小化 / 最大化↔还原 / 关闭）。
 *
 * 交互语义对齐 Windows 标题栏：
 * - 拖拽：全栏按下左键 → getCurrentWindow().startDragging()（手动挂法
 *   对齐 StatsPanel.tsx 的窗口拖头：不用 data-tauri-drag-region——子元素会
 *   吞事件；按钮上 stopPropagation 防拖拽误触，其余子元素（logo/文字）
 *   照常可拖）；
 * - 双击空白处 = 最大化 / 还原切换。刻意不挂 React onDoubleClick：Windows /
 *   Linux 的 startDragging 是原生模态拖拽循环，WebView 收不到当次 mouseup
 *   （mouseup 被循环吃掉，tauri#11945 有记载），浏览器 click → dblclick
 *   序列不可靠；因此双击分支改在 mousedown 上按 e.detail 分派（双击不走
 *   startDragging），macOS 则放在 mouseup（detail===2 且指针未移动）——
 *   与 Tauri 官方 data-tauri-drag-region 脚本（tauri/src/window/scripts/
 *   drag.js）同款做法；
 * - 关闭按钮走 close()：Rust 侧 WindowEvent::CloseRequested 挂点对
 *   main-window 做 prevent_close + hide（隐藏不销毁，见 lib.rs），不能直接
 *   调 hide 绕开该挂点；
 * - 最大化状态：挂载时查一次 isMaximized()，onResized 监听刷新（覆盖
 *   按钮点击、双击、系统热区缩放三条路径），卸载时退订。
 *
 * 边缘缩放不需要本组件参与：Windows 下 tao 对不透明无边框窗提供
 * WM_NCHITTEST 系统热区（见 src/ResizeHandles.tsx 头注释），故本窗不挂
 * ResizeHandles（它绑定的是面板专用的窗口尺寸落盘逻辑）。
 */
import { useEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { getCurrentWindow, type Window as TauriWindow } from "@tauri-apps/api/window";
// 软件 logo：与 dashboard.html 的窗口图标同源（Vite 打包为带哈希的资源 URL）
import logo from "../assets/logo.png";
import { useI18n } from "../i18n";
import { CloseIcon, MaximizeIcon, MinimizeIcon, RestoreIcon } from "./icons";

/** 是否 macOS（沿用 ResizeHandles / hud-resize 的判断风格：platform 优先，
 *  缺失时退回 UA）：决定双击最大化落在 mousedown 还是 mouseup 上 */
const isMac =
  typeof navigator !== "undefined" &&
  /mac/i.test(navigator.platform || navigator.userAgent);

/** 按钮上吞掉 mousedown：防拖拽与双击最大化误触（见文件头） */
function swallowMouseDown(e: ReactMouseEvent<HTMLElement>) {
  e.stopPropagation();
}

export function TitleBar() {
  const { t } = useI18n();
  // 窗口句柄：非 Tauri 宿主（浏览器直接开 dev server）下 getCurrentWindow()
  // 会抛错，兜底为 null（按钮禁用 + 拖拽无效），不影响页面其余部分渲染
  const [win] = useState<TauriWindow | null>(() => {
    try {
      return getCurrentWindow();
    } catch {
      return null;
    }
  });
  const [maximized, setMaximized] = useState(false);
  // macOS 分支：记录按下点，mouseup 时指针未移动才认作双击最大化
  const pressPointRef = useRef({ x: 0, y: 0 });

  // 最大化状态同步：初始查一次 + onResized 刷新。查询/监听失败一律静默
  // （按钮退回"最大化"图标，不影响其它交互）
  useEffect(() => {
    if (!win) return;
    let cancelled = false;
    let dispose: (() => void) | undefined;
    const sync = () => {
      win
        .isMaximized()
        .then((v) => {
          if (!cancelled) setMaximized(v);
        })
        .catch(() => {
          /* 静默：保持当前图标 */
        });
    };
    sync();
    win
      .onResized(sync)
      .then((unlisten) => {
        // effect 已清理（StrictMode 双挂载 / 快速卸载）时立即退订
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：图标不随系统热区缩放路径刷新（按钮路径仍会刷新） */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [win]);

  const handleBarMouseDown = (e: ReactMouseEvent<HTMLHeadElement>) => {
    // 仅左键（Windows 标题栏语义；按钮上的 mousedown 已 stopPropagation）
    if (e.button !== 0) return;
    if (isMac) {
      // macOS：拖拽会话的 mouseup 会回到 WebView，双击在 mouseup 判定
      // （见 handleBarMouseUp），此处只记录按下点
      pressPointRef.current = { x: e.clientX, y: e.clientY };
    } else {
      // Windows / Linux：startDragging 之后收不到 mouseup，双击改在此按
      // detail 分派（见文件头）；detail > 2（三连击等）不拖拽也不切换，
      // 与 Tauri 官方 drag.js 一致
      if (e.detail === 2) {
        void win?.toggleMaximize();
        return;
      }
      if (e.detail !== 1) return;
    }
    void win?.startDragging();
  };

  const handleBarMouseUp = (e: ReactMouseEvent<HTMLHeadElement>) => {
    // macOS：双击第二下的 mouseup，且指针未移动 → 最大化 / 还原切换
    if (!isMac || e.button !== 0 || e.detail !== 2) return;
    const { x, y } = pressPointRef.current;
    if (e.clientX !== x || e.clientY !== y) return;
    void win?.toggleMaximize();
  };

  return (
    <header
      className="db-titlebar"
      onMouseDown={handleBarMouseDown}
      onMouseUp={handleBarMouseUp}
    >
      {/* 品牌区：原侧栏 db-brand 的品牌信息（logo 缩到 18px 适配 40px 栏高）；
          logo alt 留空——品牌名已在旁边可见文本里，避免读屏重复播报 */}
      <div className="db-titlebar__brand">
        <img
          src={logo}
          alt=""
          className="db-titlebar__logo"
          draggable={false}
        />
        <span className="db-titlebar__name">ZBar</span>
        <span className="db-titlebar__tagline">{t("dashboard.subtitle")}</span>
      </div>

      {/* 窗口控制：与 Windows 标题栏同序（最小化 / 最大化↔还原 / 关闭），
          图标随 isMaximized 状态切换，提示与 aria-label 走 i18n */}
      <div className="db-titlebar__controls">
        <button
          type="button"
          className="db-titlebar__btn"
          onMouseDown={swallowMouseDown}
          onClick={() => void win?.minimize()}
          disabled={!win}
          title={t("dashboard.titlebar.minimize")}
          aria-label={t("dashboard.titlebar.minimize")}
        >
          <MinimizeIcon className="h-3 w-3" />
        </button>
        <button
          type="button"
          className="db-titlebar__btn"
          onMouseDown={swallowMouseDown}
          onClick={() => void win?.toggleMaximize()}
          disabled={!win}
          title={
            maximized
              ? t("dashboard.titlebar.restore")
              : t("dashboard.titlebar.maximize")
          }
          aria-label={
            maximized
              ? t("dashboard.titlebar.restore")
              : t("dashboard.titlebar.maximize")
          }
        >
          {maximized ? (
            <RestoreIcon className="h-3 w-3" />
          ) : (
            <MaximizeIcon className="h-3 w-3" />
          )}
        </button>
        <button
          type="button"
          className="db-titlebar__btn db-titlebar__btn--close"
          onMouseDown={swallowMouseDown}
          onClick={() => void win?.close()}
          disabled={!win}
          title={t("dashboard.titlebar.close")}
          aria-label={t("dashboard.titlebar.close")}
        >
          <CloseIcon className="h-3 w-3" />
        </button>
      </div>
    </header>
  );
}
