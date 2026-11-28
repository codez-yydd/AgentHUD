/**
 * 主窗口页头「⋯」更多操作菜单：28×28 无边框图标按钮 + 右对齐弹层。
 *
 * 为什么不复用 src/DropdownMenu.tsx：那是「选择」语义（aria-haspopup=
 * "listbox" / role="listbox"，选项带勾选标记），放「执行操作」入口语义不符，
 * 且触发器观感是描边按钮——页头低频操作要的是第三级视觉权重（无边框图标
 * 按钮，见「页头精简」已认可原型）。本组件只服务主窗口（悬浮面板不加载），
 * 样式落在 dashboard.css 的 .hdr-more-wrap / .hdr-more / .hdr-menu 族（沿用
 * 原型类名），颜色全部走 --db-* 变量，亮暗自动适配。
 *
 * 交互（对齐 DropdownMenu 的成熟模式）：
 *  - 点击 / Enter / Space 展开收起（按钮原生激活统一走 onClick）；
 *  - 触发器上 ↓ / ↑ 展开并落焦首 / 末项，菜单项间 ↓ / ↑ 循环移动焦点；
 *  - 菜单项点击 / Enter / Space 执行并收起（焦点回触发器，连续操作不断链）；
 *  - 点击外部、Esc、焦点移出组件时收起；Esc 额外把焦点交还触发器。
 * 无障碍：触发器 aria-haspopup="menu" + aria-expanded，弹层 role="menu"，
 * 菜单项 role="menuitem"。处理函数由调用方原样传入，本组件只管展开收起，
 * 不改任何业务语义。
 */
import { useEffect, useId, useRef, useState } from "react";
import type {
  FocusEvent as ReactFocusEvent,
  KeyboardEvent as ReactKeyboardEvent,
  ReactNode,
} from "react";

/** 菜单项：执行型入口（处理函数原样调用调用方既有逻辑） */
export interface MoreActionsItem {
  /** 稳定 key（React 列表） */
  key: string;
  /** 菜单文案（i18n 由调用方生成） */
  label: string;
  /** 行首图标（约 13px 线性 svg，颜色随 .hdr-menu-item 走第三档文字色） */
  icon: ReactNode;
  /** 执行回调 */
  onSelect: () => void;
  /** 禁用（如价格页同步进行中）：置灰且不可执行 */
  disabled?: boolean;
}

interface MoreActionsMenuProps {
  /** 触发器的 title 与 aria-label（词典 dashboard.moreActions） */
  label: string;
  items: MoreActionsItem[];
  /** 触发器右上角角标（如价格页差异红点 .db-dot，含数字）；可选 */
  badge?: ReactNode;
}

export function MoreActionsMenu({ label, items, badge }: MoreActionsMenuProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
  // 键盘展开时要落焦的菜单项下标（null = 仅展开、焦点留在触发器）。
  // 不能在 onTriggerKeyDown 里同步 focus：setOpen(true) 后菜单 DOM 还没
  // 渲染，itemRefs 还是旧值（DropdownMenu 的同款时序处理）
  const pendingFocusRef = useRef<number | null>(null);
  const menuId = useId();

  // 展开完成、菜单项 ref 就绪后再落焦（↓ / ↑ 展开时焦点直接进入菜单）
  useEffect(() => {
    if (!open) return;
    const index = pendingFocusRef.current;
    pendingFocusRef.current = null;
    if (index != null) itemRefs.current[index]?.focus();
  }, [open]);

  // 展开时：点击组件外 / Esc 关闭；Esc 同时把焦点交还触发器，键盘操作不断链
  //（与 DropdownMenu 同款监听与清理）。窗口失焦（切到别的应用，如 Cmd+Tab）
  // 也收起——对齐 macOS 原生菜单行为，避免回来后弹层还挂着
  useEffect(() => {
    if (!open) return;
    function onDown(e: MouseEvent) {
      if (
        rootRef.current &&
        e.target instanceof Node &&
        !rootRef.current.contains(e.target)
      ) {
        setOpen(false);
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      setOpen(false);
      triggerRef.current?.focus();
    }
    function onWinBlur() {
      setOpen(false);
    }
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    window.addEventListener("blur", onWinBlur);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", onWinBlur);
    };
  }, [open]);

  /** 执行并收起；焦点回触发器 */
  function runItem(index: number) {
    const item = items[index];
    if (!item || item.disabled) return;
    setOpen(false);
    item.onSelect();
    triggerRef.current?.focus();
  }

  /** 触发器 ↓ / ↑：展开并落焦首 / 末项（已展开时只移动焦点） */
  function onTriggerKeyDown(e: ReactKeyboardEvent<HTMLButtonElement>) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    if (items.length === 0) return;
    e.preventDefault();
    const index = e.key === "ArrowDown" ? 0 : items.length - 1;
    if (open) {
      // 菜单已在 DOM 里，直接移动焦点
      itemRefs.current[index]?.focus();
      return;
    }
    pendingFocusRef.current = index;
    setOpen(true);
  }

  /** 菜单项间 ↓ / ↑ 循环移动（Enter / Space 走按钮原生激活 → runItem） */
  function onItemKeyDown(
    e: ReactKeyboardEvent<HTMLButtonElement>,
    index: number
  ) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const delta = e.key === "ArrowDown" ? 1 : -1;
    itemRefs.current[(index + delta + items.length) % items.length]?.focus();
  }

  /** 焦点整体移出组件（Tab 走开）时收起，避免留下无归属的浮层 */
  function onBlur(e: ReactFocusEvent<HTMLDivElement>) {
    if (
      e.relatedTarget instanceof Node &&
      rootRef.current?.contains(e.relatedTarget)
    ) {
      return;
    }
    setOpen(false);
  }

  return (
    <div ref={rootRef} className="hdr-more-wrap" onBlur={onBlur}>
      <button
        ref={triggerRef}
        type="button"
        className="hdr-more"
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => setOpen((prev) => !prev)}
        onKeyDown={onTriggerKeyDown}
      >
        {/* ⋯ 横向三点（fill，颜色随按钮文字色） */}
        <svg
          viewBox="0 0 16 16"
          width="16"
          height="16"
          fill="currentColor"
          aria-hidden="true"
        >
          <circle cx="2.8" cy="8" r="1.5" />
          <circle cx="8" cy="8" r="1.5" />
          <circle cx="13.2" cy="8" r="1.5" />
        </svg>
        {badge}
      </button>
      {open && (
        <div id={menuId} className="hdr-menu" role="menu" aria-label={label}>
          {items.map((item, index) => (
            <button
              key={item.key}
              ref={(el) => {
                itemRefs.current[index] = el;
              }}
              type="button"
              role="menuitem"
              className="hdr-menu-item"
              disabled={item.disabled}
              onClick={() => runItem(index)}
              onKeyDown={(e) => onItemKeyDown(e, index)}
            >
              {item.icon}
              <span>{item.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
