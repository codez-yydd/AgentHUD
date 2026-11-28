/**
 * 窗口壳共用交互 hook：自绘标题栏（TitleBar，Windows / Linux 通栏）与
 * macOS 侧栏顶部（红绿灯行 + 品牌行，DashboardApp 的 SidebarShellTop）
 * 共用同一套窗口句柄获取、拖拽 / 双击最大化、最大化状态与焦点状态逻辑。
 *
 * 交互语义对齐原生标题栏（原 TitleBar.tsx 内联实现上移，行为不变）：
 * - 拖拽：拖拽区按下左键 → getCurrentWindow().startDragging()（手动挂法
 *   对齐 StatsPanel.tsx 的窗口拖头：不用 data-tauri-drag-region——子元素会
 *   吞事件；按钮上 stopPropagation 防拖拽误触，其余子元素照常可拖）；
 * - 双击空白处 = 最大化 / 还原切换。刻意不挂 React onDoubleClick：Windows /
 *   Linux 的 startDragging 是原生模态拖拽循环，WebView 收不到当次 mouseup
 *   （mouseup 被循环吃掉，tauri#11945 有记载），浏览器 click → dblclick
 *   序列不可靠；因此双击分支改在 mousedown 上按 e.detail 分派（双击不走
 *   startDragging），macOS 则放在 mouseup（detail===2 且指针未移动）——
 *   与 Tauri 官方 data-tauri-drag-region 脚本（tauri/src/window/scripts/
 *   drag.js）同款做法；
 * - 焦点：macOS 红绿灯在窗口失焦时变系统灰（监听 onFocusChanged；监听
 *   失败 / 非宿主时保持聚焦态，圆点保持彩色）。
 */
import { useEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import {
  getCurrentWindow,
  type Window as TauriWindow,
} from "@tauri-apps/api/window";

/** 是否 macOS（沿 ResizeHandles / hud-resize 的判断风格：platform 优先，
 *  缺失时退回 UA）：决定双击最大化落在 mousedown 还是 mouseup 上，也决定
 *  壳平台修饰类（db-shell--mac / db-shell--win） */
export const IS_MAC =
  typeof navigator !== "undefined" &&
  /mac/i.test(navigator.platform || navigator.userAgent);

/** 窗口句柄：非 Tauri 宿主（浏览器直接开 dev server）下 getCurrentWindow()
 *  会抛错，兜底为 null（按钮禁用 + 拖拽无效），不影响页面其余部分渲染 */
export function useTauriWindow(): TauriWindow | null {
  const [win] = useState<TauriWindow | null>(() => {
    try {
      return getCurrentWindow();
    } catch {
      return null;
    }
  });
  return win;
}

/** 最大化状态同步：初始查一次 + onResized 刷新（覆盖按钮点击、双击、系统
 *  热区缩放三条路径）。查询 / 监听失败一律静默（退回"最大化"图标） */
export function useMaximized(win: TauriWindow | null): boolean {
  const [maximized, setMaximized] = useState(false);
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
          /* 静默：保持当前状态 */
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
  return maximized;
}

/** 窗口焦点：初始聚焦，onFocusChanged 跟随；监听失败保持聚焦态（红绿灯
 *  保持彩色）。卸载 / StrictMode 双挂载时及时退订 */
export function useWindowFocused(win: TauriWindow | null): boolean {
  const [focused, setFocused] = useState(true);
  useEffect(() => {
    if (!win) return;
    let cancelled = false;
    let dispose: (() => void) | undefined;
    win
      .onFocusChanged((event) => {
        if (!cancelled) setFocused(event.payload);
      })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：拿不到焦点状态时保持彩色（组件按 focused=true 渲染） */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [win]);
  return focused;
}

/**
 * 拖拽区行为（返回给拖拽头元素挂 onMouseDown / onMouseUp）：
 * - Windows / Linux：mousedown 上按 detail 分派（detail===2 双击切最大化，
 *   detail===1 启动拖拽，其余不响应）；
 * - macOS：mousedown 记录按下点并启动拖拽，mouseup 判定双击（detail===2
 *   且指针未移动才切换，避免拖拽途中误触）。
 * 调用方须在拖拽区内的按钮上 stopPropagation mousedown（见各组件）。
 */
export function useWindowDrag(win: TauriWindow | null): {
  onMouseDown: (e: ReactMouseEvent<HTMLElement>) => void;
  onMouseUp: (e: ReactMouseEvent<HTMLElement>) => void;
} {
  // macOS 分支：记录按下点，mouseup 时指针未移动才认作双击最大化
  const pressPointRef = useRef({ x: 0, y: 0 });

  const onMouseDown = (e: ReactMouseEvent<HTMLElement>) => {
    // 仅左键（Windows 标题栏语义；按钮上的 mousedown 已 stopPropagation）
    if (e.button !== 0) return;
    if (IS_MAC) {
      // macOS：拖拽会话的 mouseup 会回到 WebView，双击在 mouseup 判定，
      // 此处只记录按下点
      pressPointRef.current = { x: e.clientX, y: e.clientY };
    } else {
      // Windows / Linux：startDragging 之后收不到 mouseup，双击改在此按
      // detail 分派；detail > 2（三连击等）不拖拽也不切换，
      // 与 Tauri 官方 drag.js 一致
      if (e.detail === 2) {
        void win?.toggleMaximize();
        return;
      }
      if (e.detail !== 1) return;
    }
    void win?.startDragging();
  };

  const onMouseUp = (e: ReactMouseEvent<HTMLElement>) => {
    // macOS：双击第二下的 mouseup，且指针未移动 → 最大化 / 还原切换
    if (!IS_MAC || e.button !== 0 || e.detail !== 2) return;
    const { x, y } = pressPointRef.current;
    if (e.clientX !== x || e.clientY !== y) return;
    void win?.toggleMaximize();
  };

  return { onMouseDown, onMouseUp };
}
