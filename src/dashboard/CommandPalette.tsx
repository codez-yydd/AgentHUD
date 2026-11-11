/**
 * 命令面板（Ctrl/⌘ + K）：主窗口 11 个页面之间的快速跳转。
 *
 * 只做「导航」这一件事——不做模型 / 请求全文搜索（那需要跨页取数，
 * 与各页自己的筛选语义冲突）。键盘：↑/↓ 移动、Enter 打开、Esc 关闭；
 * 打开时自动聚焦输入框，查询在打开瞬间清空。
 *
 * 样式全部走 dashboard.css 的 .db-cmdk*（--db-* 变量，亮暗自动生效）。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { useI18n } from "../i18n";

export interface CommandPaletteItem {
  /** 稳定 id（通常是页面 view id） */
  id: string;
  /** 展示名（已是当前语言文案） */
  label: string;
  /** 右侧灰色说明（如分组名） */
  hint?: string;
  /** 前导图标节点（可省略） */
  icon?: React.ReactNode;
}

export function CommandPalette({
  open,
  onClose,
  items,
  onRun,
}: {
  open: boolean;
  onClose: () => void;
  items: CommandPaletteItem[];
  onRun: (id: string) => void;
}) {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  // 每次打开：清空查询、选中第一项、聚焦输入框
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActive(0);
    // 下一帧聚焦：面板刚挂载时 input 还未稳定
    const timer = window.setTimeout(() => inputRef.current?.focus(), 0);
    return () => window.clearTimeout(timer);
  }, [open]);

  const filtered = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return items;
    return items.filter((item) => item.label.toLowerCase().includes(keyword));
  }, [items, query]);

  // 结果变化后把选中项收进范围（避免"高亮了一个不存在的下标"）
  useEffect(() => {
    setActive((current) => Math.min(current, Math.max(0, filtered.length - 1)));
  }, [filtered.length]);

  // 选中项滚动可见（键盘连续移动时不至于跑到列表外）
  useEffect(() => {
    if (!open) return;
    const list = listRef.current;
    const node = list?.querySelector<HTMLElement>(
      `[data-cmdk-index="${active}"]`
    );
    node?.scrollIntoView({ block: "nearest" });
  }, [active, open]);

  if (!open) return null;

  const run = (item: CommandPaletteItem) => {
    onRun(item.id);
  };

  return (
    <div
      className="db-cmdk"
      role="presentation"
      onMouseDown={(event) => {
        // 点遮罩关闭：只在遮罩自身按下时生效（面板内部点击不冒泡到关闭）
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className="db-cmdk__panel"
        role="dialog"
        aria-modal="true"
        aria-label={t("dashboard.cmdk.title")}
      >
        <input
          ref={inputRef}
          className="db-cmdk__input"
          value={query}
          placeholder={t("dashboard.cmdk.placeholder")}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              onClose();
            } else if (event.key === "ArrowDown") {
              event.preventDefault();
              setActive((current) =>
                Math.min(current + 1, Math.max(0, filtered.length - 1))
              );
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              setActive((current) => Math.max(0, current - 1));
            } else if (event.key === "Enter") {
              event.preventDefault();
              const target = filtered[active];
              if (target) run(target);
            }
          }}
        />
        <div className="db-cmdk__list" ref={listRef}>
          {filtered.length === 0 ? (
            <p className="db-cmdk__empty">{t("dashboard.cmdk.empty")}</p>
          ) : (
            filtered.map((item, index) => (
              <button
                key={item.id}
                type="button"
                data-cmdk-index={index}
                className={`db-cmdk__item${
                  index === active ? " db-cmdk__item--active" : ""
                }`}
                onMouseEnter={() => setActive(index)}
                onClick={() => run(item)}
              >
                {item.icon}
                <span className="db-cmdk__label">{item.label}</span>
                {item.hint && <span className="db-cmdk__hint">{item.hint}</span>}
              </button>
            ))
          )}
        </div>
        <div className="db-cmdk__footer">
          <span>{t("dashboard.cmdk.hintMove")}</span>
          <span>{t("dashboard.cmdk.hintRun")}</span>
          <span>{t("dashboard.cmdk.hintClose")}</span>
        </div>
      </div>
    </div>
  );
}
