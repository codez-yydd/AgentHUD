/**
 * 页面级「调整显示」编辑状态：显示方案 / 显隐 / 顺序 / 展示级别 / 高度。
 *
 * 设计要点：
 *  - 编辑期间所有改动只写入 draft（内存），点「完成」才落盘；「取消」直接丢弃，
 *    视觉与交互都可回退；
 *  - 卡片高度拖动在编辑态由 CardFrame 的 CardEditProvider 写入同一份 draft，
 *    非编辑态仍走 phase 1 的即时落盘；
 *  - 用户改动预设后，完成时自动转存为自定义方案（custom 快照），切换方案
 *    不会销毁该快照；
 *  - 显隐/顺序只影响渲染，不停止采集、统计或计费；
 *  - 页面用 visibleIds 渲染卡片，保证顺序与隐藏即时生效。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { useI18n, type MessageKey } from "./i18n";
import { CardEditProvider, type CardEditController } from "./cardFrame";
import {
  UI_LAYOUT_EVENT,
  UI_LAYOUT_LOCAL_EVENT,
  cardDefinitions,
  defaultPageLayout,
  readCustomSnapshot,
  readPageLayout,
  resetPageLayout,
  saveCustomSnapshot,
  writePageLayout,
  type CardDetail,
  type CardLayoutState,
  type LayoutScheme,
  type PageLayoutState,
  type UiWindow,
} from "./uiLayout";

export interface PageLayoutEditor {
  windowKind: UiWindow;
  page: string;
  /** 非编辑态取已保存布局；编辑态取 draft（页面据此渲染） */
  layout: PageLayoutState;
  scheme: LayoutScheme;
  editing: boolean;
  /** 当前可见卡片 ID（已按顺序排列） */
  visibleIds: string[];
  /** 所有卡片都被隐藏（用于恢复入口） */
  allHidden: boolean;
  /** 编辑态卡片控件需要的 draft 控制器 */
  editProvider: CardEditController | null;
  openEditor: () => void;
  cancelEditor: () => void;
  finishEditor: () => void;
  resetAll: () => void;
  restoreAllNow: () => void;
  switchScheme: (scheme: LayoutScheme) => void;
  toggleHidden: (cardId: string) => void;
  setDetail: (cardId: string, detail: CardDetail) => void;
  toggleCollapsed: (cardId: string) => void;
  resetHeight: (cardId: string) => void;
  moveCard: (cardId: string, direction: -1 | 1) => void;
  reorderCard: (draggedId: string, beforeId: string) => void;
  patchDraftCard: (cardId: string, patch: Partial<CardLayoutState>) => void;
}

/** 页面布局编辑状态 hook（一个页面调用一次） */
export function usePageLayoutEditor(
  windowKind: UiWindow,
  page: string
): PageLayoutEditor {
  const [layout, setLayout] = useState<PageLayoutState>(() =>
    readPageLayout(windowKind, page)
  );
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<PageLayoutState | null>(null);
  const editingRef = useRef(false);
  editingRef.current = editing;

  // 其他窗口 / 其他页面写入后重读；编辑中不打断 draft
  useEffect(() => {
    const reload = () => {
      if (!editingRef.current) setLayout(readPageLayout(windowKind, page));
    };
    const onLocal = (event: Event) => {
      const detail = (event as CustomEvent<{ windowKind?: string; page?: string }>)
        .detail;
      if (detail?.windowKind === windowKind && detail?.page === page) reload();
    };
    window.addEventListener(UI_LAYOUT_LOCAL_EVENT, onLocal);
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen(UI_LAYOUT_EVENT, reload)
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 非 Tauri 宿主：只用同窗口事件 */
      });
    return () => {
      cancelled = true;
      window.removeEventListener(UI_LAYOUT_LOCAL_EVENT, onLocal);
      dispose?.();
    };
  }, [windowKind, page]);

  const effective = editing && draft ? draft : layout;

  const patchDraft = useCallback(
    (fn: (current: PageLayoutState) => PageLayoutState) => {
      setDraft((current) => {
        if (!current) return current;
        const next = fn(current);
        // 任何手动改动都视为自定义布局：方案胶囊即时切到「自定义」，
        // 完成时转存 custom 快照（切换预设不会销毁它）
        return { ...next, customized: true, scheme: "custom" };
      });
    },
    []
  );

  const patchDraftCard = useCallback(
    (cardId: string, patch: Partial<CardLayoutState>) => {
      patchDraft((current) => ({
        ...current,
        cards: {
          ...current.cards,
          [cardId]: { ...current.cards[cardId], ...patch },
        },
      }));
    },
    [patchDraft]
  );

  const openEditor = useCallback(() => {
    const fresh = readPageLayout(windowKind, page);
    setLayout(fresh);
    setDraft(fresh);
    setEditing(true);
  }, [windowKind, page]);

  const cancelEditor = useCallback(() => {
    setDraft(null);
    setEditing(false);
    setLayout(readPageLayout(windowKind, page));
  }, [windowKind, page]);

  const finishEditor = useCallback(() => {
    if (!draft) {
      setEditing(false);
      return;
    }
    let target = draft;
    // 在预设上做了手动改动 → 存为自定义方案（保留当前视觉，不丢改动）
    if (target.customized) {
      target = { ...target, scheme: "custom" };
    }
    if (target.scheme === "custom") {
      saveCustomSnapshot(windowKind, page, target);
    }
    const clean = writePageLayout(windowKind, page, target);
    setLayout(clean);
    setDraft(null);
    setEditing(false);
  }, [draft, windowKind, page]);

  const switchScheme = useCallback(
    (scheme: LayoutScheme) => {
      setDraft((current) => {
        if (!current) return current;
        if (scheme === "custom") {
          const saved = readCustomSnapshot(windowKind, page);
          if (saved) return { ...saved, scheme: "custom" };
          return { ...defaultPageLayout(windowKind, page, "custom"), scheme: "custom" };
        }
        return { ...defaultPageLayout(windowKind, page, scheme), scheme };
      });
    },
    [windowKind, page]
  );

  const toggleHidden = useCallback(
    (cardId: string) => {
      const state = effective.cards[cardId];
      if (!state) return;
      patchDraftCard(cardId, { hidden: !state.hidden });
    },
    [effective.cards, patchDraftCard]
  );

  const setDetail = useCallback(
    (cardId: string, detail: CardDetail) => {
      patchDraftCard(cardId, { detail });
    },
    [patchDraftCard]
  );

  const toggleCollapsed = useCallback(
    (cardId: string) => {
      const state = effective.cards[cardId];
      if (!state) return;
      patchDraftCard(cardId, { collapsed: !state.collapsed });
    },
    [effective.cards, patchDraftCard]
  );

  const resetHeight = useCallback(
    (cardId: string) => {
      patchDraftCard(cardId, { height: null, listExpanded: false });
    },
    [patchDraftCard]
  );

  const moveCard = useCallback(
    (cardId: string, direction: -1 | 1) => {
      patchDraft((current) => {
        const order = [...current.order];
        const index = order.indexOf(cardId);
        const target = index + direction;
        if (index < 0 || target < 0 || target >= order.length) return current;
        [order[index], order[target]] = [order[target], order[index]];
        return { ...current, order };
      });
    },
    [patchDraft]
  );

  const reorderCard = useCallback(
    (draggedId: string, beforeId: string) => {
      if (draggedId === beforeId) return;
      patchDraft((current) => {
        const order = current.order.filter((id) => id !== draggedId);
        const index = order.indexOf(beforeId);
        if (index < 0) return current;
        order.splice(index, 0, draggedId);
        return { ...current, order };
      });
    },
    [patchDraft]
  );

  const resetAll = useCallback(() => {
    const clean = resetPageLayout(windowKind, page);
    setLayout(clean);
    setDraft(null);
    setEditing(false);
  }, [windowKind, page]);

  /** 未进入编辑态时，全隐藏页面的直接恢复入口 */
  const restoreAllNow = useCallback(() => {
    if (editingRef.current) {
      patchDraft((current) => {
        const cards: Record<string, CardLayoutState> = {};
        for (const [id, state] of Object.entries(current.cards)) {
          cards[id] = { ...state, hidden: false };
        }
        return { ...current, cards };
      });
      return;
    }
    const current = readPageLayout(windowKind, page);
    const cards: Record<string, CardLayoutState> = {};
    for (const [id, state] of Object.entries(current.cards)) {
      cards[id] = { ...state, hidden: false };
    }
    const clean = writePageLayout(windowKind, page, {
      ...current,
      cards,
      customized: true,
    });
    setLayout(clean);
  }, [windowKind, page, patchDraft]);

  const visibleIds = useMemo(
    () => effective.order.filter((id) => effective.cards[id] && !effective.cards[id].hidden),
    [effective]
  );

  return {
    windowKind,
    page,
    layout: effective,
    scheme: effective.scheme,
    editing,
    visibleIds,
    allHidden: visibleIds.length === 0,
    editProvider:
      editing && draft
        ? { layout: draft, scheme: draft.scheme, patchCard: patchDraftCard }
        : null,
    openEditor,
    cancelEditor,
    finishEditor,
    resetAll,
    restoreAllNow,
    switchScheme,
    toggleHidden,
    setDetail,
    toggleCollapsed,
    resetHeight,
    moveCard,
    reorderCard,
    patchDraftCard,
  };
}

/** 页面级 Provider：把编辑态 draft 注入卡片外框（常显手柄 / 草稿高度） */
export function PageLayoutEditProvider({
  editor,
  children,
}: {
  editor: PageLayoutEditor;
  children: ReactNode;
}) {
  return (
    <CardEditProvider value={editor.editProvider}>{children}</CardEditProvider>
  );
}

/** 「调整显示」入口按钮（两窗通用样式类，由各自 CSS 变量着色） */
export function LayoutEntryButton({
  onClick,
  className = "",
}: {
  onClick: () => void;
  className?: string;
}) {
  const { t } = useI18n();
  return (
    <button
      type="button"
      className={`layout-entry ${className}`}
      title={t("customize.entryHint")}
      onClick={onClick}
    >
      <svg
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        className="h-3 w-3"
        aria-hidden
      >
        <path d="M2 4h7M12 4h2M2 8h2M7 8h7M2 12h9M14 12h0" />
        <circle cx="10.5" cy="4" r="1.3" />
        <circle cx="5.5" cy="8" r="1.3" />
        <circle cx="12.2" cy="12" r="1.3" />
      </svg>
      {t("customize.entry")}
    </button>
  );
}

/** 编辑工具栏：方案选择 + 卡片显隐/排序/展示级别 + 完成/取消/恢复默认 */
export function LayoutEditorBar({ editor }: { editor: PageLayoutEditor }) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(true);
  const [dragId, setDragId] = useState<string | null>(null);
  const layout = editor.layout;
  const pageDefs = cardDefinitions(editor.windowKind, editor.page);
  const hiddenCount = pageDefs.filter((d) => layout.cards[d.id]?.hidden).length;

  return (
    <section className="layout-editor" aria-label={t("customize.title")}>
      <div className="layout-editor__top">
        <div className="layout-editor__heading">
          <span className="layout-editor__title">{t("customize.title")}</span>
          <span className="layout-editor__badge">{t("customize.editing")}</span>
          <span className="layout-editor__meta">
            {t("customize.cardCount", { count: pageDefs.length })}
            {hiddenCount > 0 ? ` · ${t("customize.hiddenCards", { count: hiddenCount })}` : ""}
          </span>
        </div>
        <div className="layout-editor__actions">
          <button
            type="button"
            className="layout-editor__btn"
            onClick={() => setExpanded((v) => !v)}
            aria-expanded={expanded}
          >
            {expanded ? "▲" : "▼"} {t("customize.selectCards")}
          </button>
          <button
            type="button"
            className="layout-editor__btn"
            onClick={editor.cancelEditor}
          >
            {t("customize.cancel")}
          </button>
          <button
            type="button"
            className="layout-editor__btn"
            onClick={editor.resetAll}
          >
            {t("customize.resetDefault")}
          </button>
          <button
            type="button"
            className="layout-editor__btn layout-editor__btn--primary"
            onClick={editor.finishEditor}
          >
            {t("customize.done")}
          </button>
        </div>
      </div>

      <p className="layout-editor__hint">{t("customize.editBarHint")}</p>

      <div className="layout-editor__schemes">
        <span className="layout-editor__label">{t("customize.scheme")}</span>
        <div className="layout-editor__scheme-pills">
          {(
            [
              ["classic", "customize.schemeClassic", "customize.schemeClassicHint"],
              ["simple", "customize.schemeSimple", "customize.schemeSimpleHint"],
              ["custom", "customize.schemeCustom", "customize.schemeCustomHint"],
            ] as const
          ).map(([value, labelKey, hintKey]) => (
            <button
              key={value}
              type="button"
              title={t(hintKey)}
              className={`layout-editor__pill ${
                layout.scheme === value ? "layout-editor__pill--active" : ""
              }`}
              aria-pressed={layout.scheme === value}
              onClick={() => editor.switchScheme(value)}
            >
              {t(labelKey)}
            </button>
          ))}
        </div>
      </div>
      <p className="layout-editor__hint">{t("customize.schemeSwitchHint")}</p>

      {expanded && (
        <div className="layout-editor__list">
          {layout.order.map((id, index) => {
            const def = pageDefs.find((d) => d.id === id);
            const state = layout.cards[id];
            if (!def || !state) return null;
            const hidden = state.hidden;
            return (
              <div
                key={id}
                className={`layout-editor__row ${dragId === id ? "is-dragging" : ""} ${
                  hidden ? "is-hidden" : ""
                }`}
                draggable
                onDragStart={() => setDragId(id)}
                onDragEnd={() => setDragId(null)}
                onDragOver={(e) => e.preventDefault()}
                onDrop={(e) => {
                  e.preventDefault();
                  if (dragId) editor.reorderCard(dragId, id);
                  setDragId(null);
                }}
              >
                <span className="layout-editor__grip" title={t("customize.dragHint")}>
                  ⠿
                </span>
                <label className="layout-editor__name">
                  <input
                    type="checkbox"
                    checked={!hidden}
                    aria-label={t("customize.visibleToggle")}
                    onChange={() => editor.toggleHidden(id)}
                  />
                  <span>{t(def.labelKey as MessageKey)}</span>
                </label>
                <div className="layout-editor__row-actions">
                  {def.caps.detail && (
                    <select
                      className="layout-editor__select"
                      value={state.detail}
                      title={t("customize.detail")}
                      aria-label={`${t("customize.detail")} · ${t(
                        def.labelKey as MessageKey
                      )}`}
                      onChange={(e) =>
                        editor.setDetail(id, e.target.value as CardDetail)
                      }
                    >
                      <option value="primary">{t("customize.detailPrimary")}</option>
                      <option value="full">{t("customize.detailFull")}</option>
                    </select>
                  )}
                  {def.caps.resize && (
                    <button
                      type="button"
                      className="layout-editor__mini"
                      disabled={state.height == null && !state.listExpanded}
                      onClick={() => editor.resetHeight(id)}
                    >
                      {state.height != null
                        ? `${Math.round(state.height)}px`
                        : t("customize.heightAuto")}
                    </button>
                  )}
                  {def.caps.collapse && (
                    <button
                      type="button"
                      className={`layout-editor__mini ${
                        state.collapsed ? "is-on" : ""
                      }`}
                      aria-pressed={state.collapsed}
                      onClick={() => editor.toggleCollapsed(id)}
                    >
                      {t("customize.collapsed")}
                    </button>
                  )}
                  <button
                    type="button"
                    className="layout-editor__mini layout-editor__mini--icon"
                    title={t("customize.moveUp")}
                    aria-label={t("customize.moveUp")}
                    disabled={index === 0}
                    onClick={() => editor.moveCard(id, -1)}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className="layout-editor__mini layout-editor__mini--icon"
                    title={t("customize.moveDown")}
                    aria-label={t("customize.moveDown")}
                    disabled={index === layout.order.length - 1}
                    onClick={() => editor.moveCard(id, 1)}
                  >
                    ↓
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {editor.allHidden && (
        <div className="layout-editor__empty">
          <span>{t("customize.noCardsHint")}</span>
          <button
            type="button"
            className="layout-editor__btn"
            onClick={editor.restoreAllNow}
          >
            {t("customize.restoreAll")}
          </button>
        </div>
      )}
    </section>
  );
}
