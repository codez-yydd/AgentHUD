/**
 * 通用自定义下拉选择（触发器 + 绝对定位浮层列表）——悬浮面板与主窗口共用。
 *
 * 为什么不用浏览器原生 select：弹层由 WebView2 自己绘制、CSS 完全不可控——暗色
 * 主题下选中项是系统灰底、文字对比不足，没有勾选标记，首项还会被系统裁切。
 * 这里自建触发器与列表，样式全部走 Tailwind 工具类，亮暗自适应：
 *  - 两窗口的暗色都靠 .dark 翻转 slate 变量表（index.css / dashboard.css
 *    各有一份同源映射），故触发器/选项的边框、悬停底色用 border-slate-900/10、
 *    hover:bg-slate-900/6 这类「深档 + 低透明度」写法，暗色下自动翻成浅色
 *    微亮层，文字用 text-slate-900（暗色下翻为浅灰），一套类两边通用；
 *  - 弹层底必须用「实底」bg-elevated（亮=白 / 暗=slate-800），不能用
 *    bg-slate-800 之类的 slate 工具类——暗色下 slate 变量整体反转，
 *    bg-slate-800 会变成浅色（#cbd5e1），与浅色文字叠成一团看不见；
 *  - 选中项：强调色浅底 bg-sky-500/15 + 正常文字色（非灰字）+ 行尾强调色
 *    勾选（text-sky-600 暗色下翻为亮蓝），是整份列表里最清晰的一行。
 *
 * 触发器对齐面板输入控件惯例（32px 高、1px 边框、圆角 8px、低透明度底色），
 * 主窗口沿用同一默认外观，个别上下文（面板内紧凑行）由 className 只补
 * 布局类，不改观感。
 *
 * 组件只做「触发器 + 弹层」，保持通用：前置说明文字（如「设备」）由 label
 * 传入、直接渲染在触发器内；不传则只显示当前选中项。文案（含 i18n）一律由
 * 调用方组装成 options 的 label，组件不依赖 i18n。
 *
 * 交互：点击 / Enter / Space 展开与收起，上下键展开并落焦到当前选中项、在
 * 列表内循环移动，点击或 Enter 选中并收起，点击组件外、Esc、焦点移出组件
 * 均收起（Esc 把焦点交还触发器）。无障碍语义：触发器 aria-haspopup="listbox"
 * + aria-expanded，弹层 role="listbox"，选项 role="option" + aria-selected。
 *
 * 弹层方向：默认贴在触发器下方；悬浮面板只有 300×500，靠近底部的控件
 * （同步页合并对话框里的目标设备选择）下方空间不足会被面板的
 * overflow:hidden 裁掉，故展开后量一次可用空间，不够且上方更宽裕时自动
 * 翻到触发器上方（量的是缩放后的视觉坐标，与 --ui-scale 缩放兼容）。
 */
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type {
  FocusEvent as ReactFocusEvent,
  KeyboardEvent as ReactKeyboardEvent,
} from "react";

export interface DropdownOption {
  /** 选项值（受控：与 value 比较判定选中） */
  value: string;
  /** 选项显示文案（i18n 由调用方生成） */
  label: string;
}

interface DropdownMenuProps {
  options: DropdownOption[];
  /** 当前选中值（受控） */
  value: string;
  onChange: (value: string) => void;
  /** 无障碍名称：弹层用；触发器无可见 label 时也用它兜底 */
  ariaLabel: string;
  /** 触发器前置说明文字（如「设备」）；不传则只显示当前选中项 */
  label?: string;
  /** 禁用：降透明度且不可展开 */
  disabled?: boolean;
  /** 触发器原生 title（hover 提示），沿用原 select 的 title 场景 */
  title?: string;
  /** 只放布局类（宽度 / flex-1 / 外边距等）；观感类由组件内置，勿覆盖 */
  className?: string;
  /** 弹层水平贴边：start（默认）左缘对齐触发器；end 右缘对齐
   *  （触发器靠面板右端时用，避免弹层探出面板被裁） */
  align?: "start" | "end";
}

/** 下拉箭头（展开时旋转 180°，由 Tailwind 过渡类驱动） */
function CaretIcon({ open }: { open: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className={`h-3.5 w-3.5 shrink-0 text-slate-500 transition-transform ${
        open ? "rotate-180" : ""
      }`}
    >
      <path d="m6 9 6 6 6-6" />
    </svg>
  );
}

/** 选中项行尾勾选（仅选中行渲染） */
function CheckIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className="h-3.5 w-3.5 shrink-0 text-sky-600"
    >
      <path d="m5 12.5 4.5 4.5L19 7" />
    </svg>
  );
}

export function DropdownMenu({
  options,
  value,
  onChange,
  ariaLabel,
  label,
  disabled = false,
  title,
  className = "",
  align = "start",
}: DropdownMenuProps) {
  const [open, setOpen] = useState(false);
  // 弹层展开方向：down 正常向下；up 下方空间不足时翻到触发器上方
  const [placement, setPlacement] = useState<"down" | "up">("down");
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const optionRefs = useRef<(HTMLButtonElement | null)[]>([]);
  // 键盘展开时要落焦的选项下标（null = 仅展开、焦点留在触发器）
  const pendingFocusRef = useRef<number | null>(null);
  const menuId = useId();

  // 展开时：点击组件外 / Esc 关闭。Esc 同时把焦点交还触发器，键盘操作不断链
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
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  // 展开完成、选项 ref 就绪后再落焦（上下键展开时焦点直接进入列表）
  useEffect(() => {
    if (!open) return;
    const index = pendingFocusRef.current;
    pendingFocusRef.current = null;
    if (index != null) optionRefs.current[index]?.focus();
  }, [open]);

  // 展开后（绘制前）量一次空间：下方装不下弹层而上方的空间更大时翻到上方。
  // 用 getBoundingClientRect 的视觉坐标，与 window.innerHeight 同一坐标系，
  // #root 的 --ui-scale 缩放不会让判断失真。
  useLayoutEffect(() => {
    if (!open) {
      setPlacement("down");
      return;
    }
    const menu = menuRef.current;
    const trigger = triggerRef.current;
    if (!menu || !trigger) return;
    const triggerRect = trigger.getBoundingClientRect();
    const menuHeight = menu.getBoundingClientRect().height;
    const spaceBelow = window.innerHeight - triggerRect.bottom;
    const spaceAbove = triggerRect.top;
    // 4px 间隙 + 弹层高度超过下方空间，且上方更宽裕 → 向上展开
    setPlacement(menuHeight + 8 > spaceBelow && spaceAbove > spaceBelow ? "up" : "down");
  }, [open, options.length]);

  // 展开状态下被禁用（如取消同步）：立即收起，不留不可操作的浮层
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  /** 选中并收起；焦点回到触发器，连续键盘切换不丢焦点 */
  function pick(next: string) {
    if (next !== value) onChange(next);
    setOpen(false);
    triggerRef.current?.focus();
  }

  /** 触发器键盘：上下键展开（落焦当前选中项），已展开则由当前选中项接续移动 */
  function onTriggerKeyDown(e: ReactKeyboardEvent<HTMLButtonElement>) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    if (options.length === 0) return;
    e.preventDefault();
    const selectedIndex = options.findIndex((item) => item.value === value);
    const fallback = e.key === "ArrowDown" ? 0 : options.length - 1;
    const index = selectedIndex >= 0 ? selectedIndex : fallback;
    if (open) {
      // 焦点已在选项上时走 onOptionKeyDown，这里只处理焦点仍在触发器的情况
      optionRefs.current[index]?.focus();
      return;
    }
    pendingFocusRef.current = index;
    setOpen(true);
  }

  /** 选项内上下键循环移动焦点（Enter / Space 走按钮原生激活） */
  function onOptionKeyDown(
    e: ReactKeyboardEvent<HTMLButtonElement>,
    index: number
  ) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const delta = e.key === "ArrowDown" ? 1 : -1;
    const next = (index + delta + options.length) % options.length;
    optionRefs.current[next]?.focus();
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

  const selectedLabel = options.find((item) => item.value === value)?.label ?? "";

  return (
    <div ref={rootRef} className={`relative ${className}`} onBlur={onBlur}>
      <button
        ref={triggerRef}
        type="button"
        title={title}
        className="inline-flex w-full items-center gap-1.5 h-8 px-2.5 rounded-lg border border-slate-900/10 bg-slate-900/5 text-[11px] text-slate-900 text-left cursor-pointer select-none transition-colors hover:bg-slate-900/10 disabled:opacity-40 disabled:cursor-not-allowed focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-sky-600"
        aria-haspopup="listbox"
        aria-expanded={open}
        // 有可见 label 时由「label + 当前值」拼出可访问名（把选中项也读出来）；
        // 无可见 label 时用 ariaLabel 兜底，避免出现无名按钮
        aria-label={label == null ? ariaLabel : undefined}
        aria-controls={open ? menuId : undefined}
        disabled={disabled}
        onClick={() => setOpen((prev) => !prev)}
        onKeyDown={onTriggerKeyDown}
      >
        {label != null && (
          <span className="shrink-0 text-[10px] text-slate-500">{label}</span>
        )}
        {/* 当前选中项文案：设备名可能较长，单行省略（宽上限与原有下拉一致） */}
        <span className="min-w-0 flex-1 max-w-[220px] truncate text-left">
          {selectedLabel}
        </span>
        <CaretIcon open={open} />
      </button>
      {open && (
        <div
          ref={menuRef}
          id={menuId}
          // z-30 与 .db-tip 同级，高于吸顶头部与卡片内容；实底 bg-elevated
          // 阻断下层内容透出（面板玻璃底与主窗口卡片都是半透明的）
          className={`absolute z-30 box-border min-w-full max-w-[300px] max-h-[280px] overflow-y-auto overscroll-contain rounded-lg border border-slate-900/10 bg-elevated p-1 shadow-xl ${
            align === "end" ? "right-0" : "left-0"
          } ${placement === "up" ? "bottom-full mb-1" : "top-full mt-1"}`}
          role="listbox"
          aria-label={ariaLabel}
        >
          {options.map((option, index) => {
            const active = option.value === value;
            return (
              <button
                key={option.value}
                ref={(el) => {
                  optionRefs.current[index] = el;
                }}
                type="button"
                role="option"
                aria-selected={active}
                // 选中行的强调底色写在非悬停分支上：不给选中行加悬停灰，
                // 避免悬停时把选中底色盖掉（原有 CSS 靠源顺序解决的同一问题）
                className={`flex w-full h-8 items-center gap-2 rounded-md px-2 text-left text-[11px] text-slate-900 cursor-pointer transition-colors focus-visible:outline-2 focus-visible:outline-offset-[-1px] focus-visible:outline-sky-600 ${
                  active ? "bg-sky-500/15 font-semibold" : "hover:bg-slate-900/6"
                }`}
                onClick={() => pick(option.value)}
                onKeyDown={(e) => onOptionKeyDown(e, index)}
              >
                <span className="min-w-0 flex-1 truncate">{option.label}</span>
                {active && <CheckIcon />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
