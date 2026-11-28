/**
 * 自绘标题栏（Windows / Linux 通栏）：窗口 decorations: false（见
 * src-tauri/src/main_window.rs）之后替代系统标题栏——左侧品牌区（logo +
 * AgentHUD + 副标题），右侧窗口控制按钮（最小化 / 最大化↔还原 / 关闭）。
 *
 * macOS 下本组件整体隐藏（CSS：.db-shell--mac .db-titlebar），窗口控制由
 * 侧栏顶部的红绿灯（DashboardApp 的 SidebarShellTop）承担；组件保持挂载，
 * 两平台的壳结构都渲染在 DOM 里、由 CSS 决定显示哪一套。
 *
 * 拖拽 / 双击最大化 / 最大化状态同步逻辑抽在 useWindowChrome.ts（与
 * SidebarShellTop 共用）；窗口方法语义见该文件头注释。关闭按钮走 close()：
 * 省内存改造后 Rust 侧不再拦截 main-window 的 CloseRequested，默认关闭 =
 * 销毁窗口（WebView 渲染进程随之释放，下次打开走现场建窗，见 lib.rs /
 * main_window.rs），前端无需感知差异。
 *
 * 边缘缩放不需要本组件参与：Windows 下 tao 对不透明无边框窗提供
 * WM_NCHITTEST 系统热区（见 src/ResizeHandles.tsx 头注释），故本窗不挂
 * ResizeHandles（它绑定的是面板专用的窗口尺寸落盘逻辑）。
 */
import type { MouseEvent as ReactMouseEvent } from "react";
import { useI18n } from "../i18n";
// 软件 logo：与 dashboard.html 的窗口图标同源（Vite 打包为带哈希的资源 URL）
import logo from "../assets/logo.png";
import { CloseIcon, MaximizeIcon, MinimizeIcon, RestoreIcon } from "./icons";
import {
  useMaximized,
  useTauriWindow,
  useWindowDrag,
} from "./useWindowChrome";

/** 按钮上吞掉 mousedown：防拖拽与双击最大化误触（见 useWindowChrome 文件头） */
function swallowMouseDown(e: ReactMouseEvent<HTMLElement>) {
  e.stopPropagation();
}

export function TitleBar({ subtitle }: { subtitle?: string }) {
  const { t } = useI18n();
  const win = useTauriWindow();
  const maximized = useMaximized(win);
  const { onMouseDown, onMouseUp } = useWindowDrag(win);

  return (
    <header className="db-titlebar" onMouseDown={onMouseDown} onMouseUp={onMouseUp}>
      {/* 品牌区：原侧栏 db-brand 的品牌信息（logo 缩到 18px 适配 40px 栏高）；
          logo alt 留空——品牌名已在旁边可见文本里，避免读屏重复播报 */}
      <div className="db-titlebar__brand">
        <img
          src={logo}
          alt=""
          className="db-titlebar__logo"
          draggable={false}
        />
        <span className="db-titlebar__name">AgentHUD</span>
        {/* 副标题：默认主窗口的「用量与额度管理」，灵动岛设置窗口传入自己的
            标题（见文件头）；品牌名 AgentHUD 本身不随窗口变化 */}
        <span className="db-titlebar__tagline">
          {subtitle ?? t("dashboard.subtitle")}
        </span>
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
