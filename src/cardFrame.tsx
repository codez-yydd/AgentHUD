/**
 * 数据卡片外框：统一承载「拖动底边改高度 / 恢复自适应 / 折叠为标题与摘要 /
 * 列表展开全部」四类交互，视觉完全交给调用方的 className，不改变既有卡片
 * 风格（面板 .card-base、主窗口 .db-card 各自沿用原样式）。
 *
 * 交互约定：
 *  - 普通状态：拖动手柄只在悬停 / 键盘聚焦 / 拖动中出现（轻量提示）；
 *    「调整显示」编辑态由 CardEditProvider 注入草稿，手柄常显（见 CSS 的
 *    .card-frame--editing）；
 *  - 高度按 CSS 逻辑 px 存储；HUD 面板与主窗口都可能整体缩放（#root 的
 *    transform: scale(--ui-scale)），拖动距离用 --ui-scale 换算回逻辑 px；
 *  - 手动高度下内容超出走卡片内部滚动；「展开全部」时取消固定高度，
 *    列表不再内部限高，改由页面整体滚动；
 *  - 折叠只隐藏正文，标题 / 摘要 / 操作按钮始终保留；
 *  - 最小高度 = max(注册表下限, 标题实测高度 + 内边距)，字号变大后自动抬升；
 *  - 不设置一屏高度上限（只防极端值），满足完整查看需求。
 */
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import type {
  CSSProperties,
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
  ReactNode,
} from "react";
import { useI18n } from "./i18n";
import {
  MAX_CARD_HEIGHT,
  MIN_CARD_HEIGHT,
  UI_LAYOUT_EVENT,
  UI_LAYOUT_LOCAL_EVENT,
  cardDefinition,
  readCardState,
  readPageLayout,
  writeCardState,
  type CardLayoutState,
  type LayoutScheme,
  type PageLayoutState,
  type UiWindow,
} from "./uiLayout";

/** 卡片引用：调用方给出稳定 ID 与所属窗口 / 页面 */
export interface CardFrameRef {
  window: UiWindow;
  page: string;
  /** 稳定卡片 ID（注册表键） */
  id: string;
}

/** 卡片正文渲染上下文 */
export interface CardBodyContext {
  /** 列表是否已「展开全部」 */
  listExpanded: boolean;
  /** 是否处于手动高度（正文超出时内部滚动） */
  fixed: boolean;
  /** 展示级别（主要 / 完整） */
  detail: "primary" | "full";
}

/** 「调整显示」编辑态控制器：由页面级 Provider 提供草稿，卡片改草稿而非落盘 */
export interface CardEditController {
  layout: PageLayoutState;
  scheme: LayoutScheme;
  patchCard: (cardId: string, patch: Partial<CardLayoutState>) => void;
}

const CardEditContext = createContext<CardEditController | null>(null);

/** 页面级编辑 Provider（阶段三「调整显示」使用；阶段一普通拖动走本地存储） */
export function CardEditProvider({
  value,
  children,
}: {
  value: CardEditController | null;
  children: ReactNode;
}) {
  return (
    <CardEditContext.Provider value={value}>
      {children}
    </CardEditContext.Provider>
  );
}

/** 当前宿主根节点的整体缩放（读 --ui-scale；缺失 / 非法按 1） */
export function currentUiScale(): number {
  try {
    const raw = getComputedStyle(document.documentElement)
      .getPropertyValue("--ui-scale")
      .trim();
    const value = parseFloat(raw);
    return Number.isFinite(value) && value > 0 ? value : 1;
  } catch {
    return 1;
  }
}

/** 折叠 / 展开箭头 */
function ChevronIcon({ collapsed }: { collapsed: boolean }) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="h-3 w-3"
      aria-hidden
    >
      <path d={collapsed ? "M4 6l4 4 4-4" : "M4 10l4-4 4 4"} />
    </svg>
  );
}

/** 恢复自适应图标 */
function ResetIcon() {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="h-3 w-3"
      aria-hidden
    >
      <path d="M3.2 6.2a5 5 0 1 1-.2 3.4" />
      <path d="M3 2.8v3.4h3.4" />
    </svg>
  );
}

export interface CardFrameProps {
  frame: CardFrameRef;
  /** 卡片视觉类（面板 card-base / 主窗口 db-card） */
  className?: string;
  /** 正文容器附加类（图表卡传 flex 布局） */
  bodyClassName?: string;
  /** 注册表外的兜底下限 */
  minHeight?: number;
  /** 标题行渲染函数；controls 由 CardFrame 生成，调用方放进标题右侧 */
  head: (controls: ReactNode) => ReactNode;
  children: ReactNode | ((ctx: CardBodyContext) => ReactNode);
  /** 折叠态摘要；不传时用通用提示 */
  summary?: ReactNode;
  /** 列表卡：总数 > 收起阈值时渲染「展开全部 / 收起」 */
  list?: { total: number; collapsedCount: number };
}

/**
 * 卡片外框。所有布局状态都通过 localStorage（阶段一）/ 编辑草稿（阶段三）
 * 读写，组件本身不拥有业务状态，数据刷新不会重置布局。
 */
export function CardFrame({
  frame,
  className = "",
  bodyClassName = "",
  minHeight,
  head,
  children,
  summary,
  list,
}: CardFrameProps) {
  const { t } = useI18n();
  const def = cardDefinition(frame.id);
  const caps = def?.caps ?? {
    resize: true,
    collapse: true,
    expandList: false,
    detail: false,
    hide: true,
  };
  const edit = useContext(CardEditContext);
  const [stored, setStored] = useState<CardLayoutState>(() =>
    readCardState(frame.window, frame.page, frame.id)
  );
  // 页面顺序（CSS flex order）：布局配置改顺序后无需页面重排 JSX
  const [pageOrder, setPageOrder] = useState<number>(() =>
    readPageLayout(frame.window, frame.page).order.indexOf(frame.id)
  );
  const state = edit?.layout.cards[frame.id] ?? stored;

  const cardRef = useRef<HTMLDivElement | null>(null);
  const headRef = useRef<HTMLDivElement | null>(null);
  const [baseMin, setBaseMin] = useState(
    () => minHeight ?? def?.minHeight ?? 80
  );
  const [liveHeight, setLiveHeight] = useState<number | null>(null);
  const [dragging, setDragging] = useState(false);
  const [measuredMin, setMeasuredMin] = useState(baseMin);
  const cleanupRef = useRef<(() => void) | null>(null);

  // 注册表 / 调用方下限变化
  useEffect(() => {
    setBaseMin(minHeight ?? def?.minHeight ?? 80);
  }, [minHeight, def?.minHeight]);

  // 同窗口布局事件（其他卡片切换方案 / 其他窗口广播）→ 重读本卡状态
  useEffect(() => {
    const reload = () => {
      setStored(readCardState(frame.window, frame.page, frame.id));
      setPageOrder(
        readPageLayout(frame.window, frame.page).order.indexOf(frame.id)
      );
    };
    window.addEventListener(UI_LAYOUT_LOCAL_EVENT, reload);
    let dispose: (() => void) | undefined;
    let cancelled = false;
    import("@tauri-apps/api/event")
      .then(({ listen }) => listen(UI_LAYOUT_EVENT, reload))
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 非 Tauri 宿主：只用同窗口事件 */
      });
    return () => {
      cancelled = true;
      window.removeEventListener(UI_LAYOUT_LOCAL_EVENT, reload);
      dispose?.();
    };
  }, [frame.window, frame.page, frame.id]);

  // 拖动 / 卸载时清理全局监听与光标
  useEffect(() => {
    return () => {
      cleanupRef.current?.();
      document.body.classList.remove("card-resizing");
    };
  }, []);

  // 最小高度随标题实测高度与字号变化（标题、操作按钮不被裁掉）
  useEffect(() => {
    const card = cardRef.current;
    const headEl = headRef.current;
    if (!card || !headEl) return;
    const measure = () => {
      const style = getComputedStyle(card);
      const padTop = parseFloat(style.paddingTop) || 0;
      const padBottom = parseFloat(style.paddingBottom) || 0;
      const borderTop = parseFloat(style.borderTopWidth) || 0;
      const borderBottom = parseFloat(style.borderBottomWidth) || 0;
      const next = Math.max(
        baseMin,
        Math.ceil(headEl.offsetHeight + padTop + padBottom + borderTop + borderBottom + 12)
      );
      setMeasuredMin((prev) => (Math.abs(prev - next) > 0.5 ? next : prev));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(headEl);
    ro.observe(card);
    return () => ro.disconnect();
  }, [baseMin, state.collapsed, state.listExpanded, state.hidden]);

  const fixed =
    state.height != null && !state.listExpanded && !state.collapsed;
  const effectiveHeight = fixed
    ? Math.max(liveHeight ?? state.height!, measuredMin)
    : undefined;

  /** 写入状态：编辑态写草稿，普通态立即落盘 */
  const applyPatch = (patch: Partial<CardLayoutState>) => {
    if (edit) {
      edit.patchCard(frame.id, patch);
    } else {
      setStored(writeCardState(frame.window, frame.page, frame.id, patch));
    }
  };

  const startDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || state.listExpanded || state.collapsed) return;
    const card = cardRef.current;
    if (!card) return;
    event.preventDefault();
    event.stopPropagation();
    const scale = currentUiScale();
    const startY = event.clientY;
    const startH = card.offsetHeight;
    const minH = Math.max(MIN_CARD_HEIGHT, measuredMin);
    const handle = event.currentTarget;
    const pointerId = event.pointerId;
    let latest = Math.max(minH, Math.min(startH, MAX_CARD_HEIGHT));
    let moved = false;
    try {
      handle.setPointerCapture(pointerId);
    } catch {
      /* 不支持指针捕获：仍可用 window 监听拖动 */
    }
    setDragging(true);
    setLiveHeight(latest);
    document.body.classList.add("card-resizing");

    const onMove = (ev: PointerEvent) => {
      const delta = (ev.clientY - startY) / scale;
      if (Math.abs(delta) > 2) moved = true;
      latest = Math.max(minH, Math.min(startH + delta, MAX_CARD_HEIGHT));
      setLiveHeight(latest);
      ev.preventDefault();
    };
    const finish = (commit: boolean) => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      cleanupRef.current = null;
      try {
        handle.releasePointerCapture(pointerId);
      } catch {
        /* 指针已释放 / 不支持捕获 */
      }
      document.body.classList.remove("card-resizing");
      setDragging(false);
      setLiveHeight(null);
      if (commit && moved) {
        applyPatch({ height: Math.round(latest) });
      }
    };
    const onUp = () => finish(true);
    const onCancel = () => finish(false);
    const cleanup = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      document.body.classList.remove("card-resizing");
    };
    cleanupRef.current = cleanup;
    window.addEventListener("pointermove", onMove, { passive: false });
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
  };

  /** 键盘调高 / 调低（无鼠标也能用）+ Home 恢复自适应 */
  const onHandleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Home") {
      event.preventDefault();
      applyPatch({ height: null });
      return;
    }
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    const card = cardRef.current;
    if (!card) return;
    const step = event.shiftKey ? 32 : 8;
    const current = state.height ?? card.offsetHeight;
    const next =
      event.key === "ArrowDown"
        ? Math.min(Math.max(current, measuredMin) + step, MAX_CARD_HEIGHT)
        : Math.max(
            Math.min(current, MAX_CARD_HEIGHT) - step,
            Math.max(MIN_CARD_HEIGHT, measuredMin)
          );
    applyPatch({ height: Math.round(next) });
  };

  const toggleCollapsed = () => applyPatch({ collapsed: !state.collapsed });
  const resetHeight = () => applyPatch({ height: null });
  const toggleList = () => applyPatch({ listExpanded: !state.listExpanded });

  const hasControls = caps.collapse || (caps.resize && state.height != null);
  const controls = hasControls ? (
    <div className="card-frame__controls" onPointerDown={(e) => e.stopPropagation()}>
      {caps.collapse && (
        <button
          type="button"
          className="card-frame__control"
          title={state.collapsed ? t("card.expand") : t("card.collapse")}
          aria-label={state.collapsed ? t("card.expand") : t("card.collapse")}
          aria-expanded={!state.collapsed}
          onClick={toggleCollapsed}
        >
          <ChevronIcon collapsed={state.collapsed} />
        </button>
      )}
      {caps.resize && state.height != null && (
        <button
          type="button"
          className="card-frame__control"
          title={t("card.resetHeight")}
          aria-label={t("card.resetHeight")}
          onClick={resetHeight}
        >
          <ResetIcon />
        </button>
      )}
    </div>
  ) : null;

  const bodyCtx: CardBodyContext = {
    listExpanded: state.listExpanded,
    fixed,
    detail: state.detail,
  };

  const showListToggle =
    !!list && list.total > list.collapsedCount && !state.collapsed;
  const showHandle =
    caps.resize && !state.listExpanded && !state.collapsed;
  const effectiveOrder = edit
    ? edit.layout.order.indexOf(frame.id)
    : pageOrder;

  const style: CSSProperties | undefined = {
    ...(effectiveHeight ? { height: `${effectiveHeight}px` } : null),
    // flex 容器内按注册表 / 用户顺序显示；+10 让页头 / 编辑栏（order 0/1）始终在前，
    // 同时不改变卡片之间的相对顺序。hidden 只隐藏显示内容
    order: effectiveOrder >= 0 ? effectiveOrder + 10 : undefined,
    display: state.hidden ? "none" : undefined,
  };

  return (
    <div
      ref={cardRef}
      className={`card-frame ${fixed ? "card-frame--fixed" : ""} ${
        state.collapsed ? "card-frame--collapsed" : ""
      } ${dragging ? "card-frame--dragging" : ""} ${
        edit ? "card-frame--editing" : ""
      } ${className}`}
      style={style}
      data-card-id={frame.id}
      data-card-height={fixed ? Math.round(effectiveHeight ?? 0) : "auto"}
      data-card-collapsed={state.collapsed ? "1" : "0"}
      data-list-expanded={state.listExpanded ? "1" : "0"}
      data-card-hidden={state.hidden ? "1" : "0"}
    >
      <div ref={headRef} className="card-frame__head">
        {head(controls)}
      </div>

      {state.collapsed ? (
        <div className="card-frame__summary">
          {summary ?? (
            <span className="card-frame__summary-muted">
              {t("card.summaryFallback")}
            </span>
          )}
        </div>
      ) : (
        <div
          className={`card-frame__body ${bodyClassName}`}
          style={
            fixed
              ? { flex: "1 1 0%", minHeight: 0, overflowY: "auto", overscrollBehavior: "contain" }
              : undefined
          }
        >
          {typeof children === "function" ? children(bodyCtx) : children}
        </div>
      )}

      {showListToggle && (
        <div className="card-frame__listfoot">
          {state.listExpanded && (
            <span className="card-frame__listhint">
              {t("card.listExpandedHint")}
            </span>
          )}
          <button
            type="button"
            className="card-frame__linkbtn"
            aria-expanded={state.listExpanded}
            onClick={toggleList}
          >
            {state.listExpanded
              ? t("card.collapseList")
              : t("card.expandAll", { count: list.total })}
          </button>
        </div>
      )}

      {showHandle && (
        <div
          className="card-frame__grip"
          role="separator"
          aria-orientation="horizontal"
          aria-label={t("card.dragHint")}
          tabIndex={0}
          title={t("card.dragHint")}
          onPointerDown={startDrag}
          onKeyDown={onHandleKeyDown}
          onDoubleClick={resetHeight}
        >
          <span aria-hidden />
        </div>
      )}
    </div>
  );
}
