/**
 * 设置窗（wset）控件库：一组可复用的表单控件。
 */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import {
  fontFamilyWithFallback,
  quoteFontFamily,
  SETTINGS_FONT_STACK,
} from "../../../island/fontFamily";
import { rowDomId } from "./schema";
import { pickFile, type SettingsFont } from "./settingsApi";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  CheckIcon,
  ChipCheckIcon,
  CloseIcon,
  DownArrowIcon,
  EyeIcon,
  EyeOffIcon,
  FolderIcon,
  PlusIcon,
  TrashIcon,
} from "./icons";

// ============================================================ 页面骨架

/** 页头：21/600 标题 + 12.5 说明 */
export function PageHeader({ title, desc }: { title: string; desc?: string }) {
  return (
    <header className="wset-page">
      <h1 className="wset-page__title">{title}</h1>
      {desc ? <p className="wset-page__desc">{desc}</p> : null}
    </header>
  );
}

/** 分组：11.5/600 提示色小标题 + 圆角 16 的卡片（无描边、无投影） */
export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="wset-sec">
      <div className="wset-sec__head">{title}</div>
      <Group>{children}</Group>
    </section>
  );
}

/** 分组卡片本体（需要自定义结构时单独用） */
export function Group({ children }: { children: ReactNode }) {
  return <div className="wset-group">{children}</div>;
}

export interface RowProps {
  title: string;
  desc?: string;
  /** 右侧控件（wide 时在文字下方整行） */
  children?: ReactNode;
  /** 宽行：文字在上、控件在下（列表 / 多选 / 长控件用） */
  wide?: boolean;
  /** 行内错误说明（红字 + 红底微光） */
  error?: string;
  /** 点路径：生成 DOM id，供「跳转到首个错误行」滚动定位 */
  rowKey?: string;
  /** 标题后的小标签（例如「未接入」） */
  tag?: ReactNode;
}

/** 一行设置：标题 13/600 + 说明 11.5/行高 1.45 + 控件右对齐 */
export function Row({ title, desc, children, wide, error, rowKey, tag }: RowProps) {
  return (
    <div
      className={`wset-row${wide ? " wset-row--wide" : ""}${error ? " wset-row--error" : ""}`}
      id={rowKey ? rowDomId(rowKey) : undefined}
    >
      <div className="wset-row__text">
        <div className="wset-row__title">
          <span>{title}</span>
          {tag}
        </div>
        {desc ? <div className="wset-row__desc">{desc}</div> : null}
        {error ? <div className="wset-row__err">{error}</div> : null}
      </div>
      {children ? <div className="wset-row__ctl">{children}</div> : null}
    </div>
  );
}

/** 分组内的一行纯说明（不参与保存） */
export function Note({ children }: { children: ReactNode }) {
  return <div className="wset-note">{children}</div>;
}

/** 「未接入」小标签（doc payload 的 unsupported 命中时显示在行标题后） */
export function Tag({
  children,
  tone = "accent",
}: {
  children: ReactNode;
  tone?: "accent" | "ok" | "danger" | "hint";
}) {
  return <span className={`wset-tag wset-tag--${tone}`}>{children}</span>;
}

/** 页尾「恢复本页默认值」+ 二次确认 */
export function PageResetBar({ onReset }: { onReset: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="wset-pagefoot">
      <Button size="sm" onClick={() => setOpen(true)}>
        恢复本页默认值
      </Button>
      <Confirm
        open={open}
        title="恢复本页默认值？"
        body="这一页的设置会全部恢复为出厂默认，并自动保存、立即生效。"
        okText="恢复本页"
        onOk={() => {
          setOpen(false);
          onReset();
        }}
        onCancel={() => setOpen(false)}
      />
    </div>
  );
}

// ============================================================ 开关 / 步进 / 下拉

/** 开关：46×26 胶囊，滑块 20，150ms 过渡 */
export function Switch({
  checked,
  onChange,
  ariaLabel,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  ariaLabel?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      className="wset-switch"
      onClick={() => onChange(!checked)}
    >
      <span className="wset-switch__knob" />
    </button>
  );
}

/** 数字输入：− / 数值 / 单位 / +（宽 158 有单位、138 无单位，高 34） */
export function Stepper({
  value,
  min,
  max,
  step = 1,
  decimals = 0,
  suffix,
  onChange,
  ariaLabel,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  decimals?: number;
  suffix?: string;
  onChange: (next: number) => void;
  ariaLabel?: string;
}) {
  const format = useCallback(
    (n: number) => {
      if (!Number.isFinite(n)) return "";
      return decimals > 0 ? n.toFixed(decimals) : String(Math.round(n));
    },
    [decimals]
  );
  const [text, setText] = useState(() => format(value));
  const editing = useRef(false);

  // 外部值（恢复默认 / 保存回填）变化时同步显示，正在输入时不打断
  useEffect(() => {
    if (!editing.current) setText(format(value));
  }, [value, format]);

  const nudge = (delta: number) => {
    const current = editing.current ? Number(text) : value;
    const base = Number.isFinite(current) ? current : min;
    const next = Math.min(max, Math.max(min, base + delta * step));
    const rounded = decimals > 0 ? Number(next.toFixed(decimals)) : Math.round(next);
    setText(format(rounded));
    onChange(rounded);
  };

  const commit = () => {
    editing.current = false;
    const raw = text.trim();
    const parsed = Number(raw);
    if (!raw || !Number.isFinite(parsed)) {
      setText(format(value));
      return;
    }
    // 允许暂时越界：保存时由校验给出「请输入 a ~ b 之间的数字」
    setText(format(parsed));
    if (parsed !== value) onChange(parsed);
  };

  return (
    <div className={`wset-step ${suffix ? "wset-step--158" : "wset-step--138"}`}>
      <button
        type="button"
        className="wset-step__btn"
        aria-label="减少"
        onClick={() => nudge(-1)}
      >
        −
      </button>
      <input
        className="wset-step__input"
        value={text}
        inputMode={decimals > 0 ? "decimal" : "numeric"}
        aria-label={ariaLabel}
        onFocus={() => {
          editing.current = true;
        }}
        onChange={(event) => {
          editing.current = true;
          setText(event.target.value);
        }}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
            event.currentTarget.blur();
          }
        }}
      />
      {suffix ? <span className="wset-step__suffix">{suffix}</span> : null}
      <button
        type="button"
        className="wset-step__btn"
        aria-label="增加"
        onClick={() => nudge(1)}
      >
        +
      </button>
    </div>
  );
}

/** 由 step 推断显示小数位（0.5 → 1 位，0.05 → 2 位，整数 → 0 位） */
function stepDecimals(step: number): number {
  if (!Number.isFinite(step) || step >= 1) return 0;
  const text = String(step);
  const dot = text.indexOf(".");
  return dot < 0 ? 0 : text.length - dot - 1;
}

/**
 * 滑动条：原生 input[type=range] + 同风格包装。
 * 滑块旁常驻显示「当前值 + 单位」，拖动时同步刷新；支持小数 step（字号 0.5）。
 */
export function Slider({
  value,
  min,
  max,
  step = 1,
  suffix,
  onChange,
  ariaLabel,
  disabled,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  suffix?: string;
  onChange: (next: number) => void;
  ariaLabel?: string;
  disabled?: boolean;
}) {
  const decimals = stepDecimals(step);
  const safe = Number.isFinite(value) ? value : min;
  // 已填充比例：用 CSS 变量画双色轨道（原生 range 不能用 ::before 改已填充色）
  const pct = max > min ? Math.min(100, Math.max(0, ((safe - min) / (max - min)) * 100)) : 0;
  const shown = decimals > 0 ? safe.toFixed(decimals) : String(Math.round(safe));
  return (
    <div className={`wset-slider${disabled ? " wset-slider--disabled" : ""}`}>
      <input
        className="wset-slider__input"
        type="range"
        min={min}
        max={max}
        step={step}
        value={safe}
        disabled={disabled}
        aria-label={ariaLabel}
        style={{ "--w-slider-pct": `${pct}%` } as CSSProperties}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <span className="wset-slider__value" aria-hidden="true">
        {shown}
        {suffix ? <span className="wset-slider__suffix">{suffix}</span> : null}
      </span>
    </div>
  );
}

export interface SelectOption<T extends string | number = string> {
  value: T;
  label: string;
}

/** 下拉选择：34 高圆角 10，自定义弹层（圆角 14，item 32 高） */
export function Select<T extends string | number>({
  value,
  options,
  onChange,
  width = 180,
  ariaLabel,
}: {
  value: T;
  options: SelectOption<T>[];
  onChange: (next: T) => void;
  width?: number;
  ariaLabel?: string;
}) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<CSSProperties>({});
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const popRef = useRef<HTMLDivElement | null>(null);
  const current = options.find((item) => item.value === value) ?? options[0];

  const toggle = () => {
    if (open) {
      setOpen(false);
      return;
    }
    const rect = btnRef.current?.getBoundingClientRect();
    if (!rect) return;
    const itemH = 32;
    const menuH = options.length * itemH + 12;
    const below = rect.bottom + 4;
    const flip = below + menuH > window.innerHeight - 8 && rect.top - 4 - menuH > 8;
    const left = Math.min(rect.left, window.innerWidth - Math.max(rect.width, 140) - 8);
    setPos({
      left: Math.max(8, left),
      top: flip ? rect.top - 4 - menuH : below,
      minWidth: Math.max(rect.width, 120),
    });
    setOpen(true);
  };

  // 关闭：点外面 / Esc / 滚动。capture 阶段接管 Esc，避免同时触发窗口的关闭逻辑
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (popRef.current?.contains(target) || btnRef.current?.contains(target)) return;
      setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
    };
    const onScroll = (event: Event) => {
      if (popRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onScroll);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onScroll);
    };
  }, [open]);

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className="wset-select"
        style={{ width }}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onClick={toggle}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            toggle();
          }
        }}
      >
        <span className="wset-select__label">{current?.label ?? ""}</span>
        <span className="wset-select__chev">
          <DownArrowIcon size={16} />
        </span>
      </button>
      {open
        ? createPortal(
            <div className="wset-pop" style={pos} ref={popRef} role="listbox">
              {options.map((item) => (
                <button
                  key={String(item.value)}
                  type="button"
                  role="option"
                  aria-selected={item.value === value}
                  className={`wset-pop__item${item.value === value ? " wset-pop__item--on" : ""}`}
                  onClick={() => {
                    setOpen(false);
                    if (item.value !== value) onChange(item.value);
                  }}
                >
                  <span className="wset-pop__check">
                    {item.value === value ? <CheckIcon size={11} /> : null}
                  </span>
                  <span className="wset-pop__label">{item.label}</span>
                </button>
              ))}
            </div>,
            document.body
          )
        : null}
    </>
  );
}

// ============================================================ 输入

/** 单行输入：34 高圆角 10，支持密码眼睛 / 浏览按钮 / 错误描边 */
export function TextInput({
  value,
  onChange,
  placeholder,
  secret,
  invalid,
  wide,
  width,
  ariaLabel,
  onBrowse,
  browseTitle,
  inputMode,
  onFocus,
  onBlur,
}: {
  value: string;
  onChange: (next: string) => void;
  placeholder?: string;
  secret?: boolean;
  invalid?: boolean;
  wide?: boolean;
  width?: number;
  ariaLabel?: string;
  onBrowse?: () => void;
  browseTitle?: string;
  /** 移动端 / 数值单元格的软键盘类型（Records 的数字列用） */
  inputMode?: "text" | "numeric" | "decimal";
  /** 聚焦 / 失焦回调：Records 的数字列在失焦时才把文本解析成数字 */
  onFocus?: () => void;
  onBlur?: () => void;
}) {
  const [revealed, setRevealed] = useState(false);
  /**
   * 只带一个后缀图标（眼睛 / 浏览）时用「图标贴右内缘 + 文字右侧预留 34px」布局：
   * 长路径 / 长密钥不会被挤到图标下面。两个图标同时存在时退回并排，避免重叠。
   */
  const suffixCount = (secret ? 1 : 0) + (onBrowse ? 1 : 0);
  const suffix = suffixCount === 1;
  return (
    <div
      className={`wset-input${suffix ? " wset-input--suffix" : ""}${
        invalid ? " wset-input--error" : ""
      }${wide ? " wset-input--wide" : ""}`}
      style={width ? { width } : undefined}
    >
      <input
        type={secret && !revealed ? "password" : "text"}
        value={value}
        placeholder={placeholder}
        aria-label={ariaLabel}
        spellCheck={false}
        inputMode={inputMode}
        onFocus={onFocus}
        onBlur={onBlur}
        onChange={(event) => onChange(event.target.value)}
      />
      {secret ? (
        <button
          type="button"
          className="wset-input__btn"
          aria-label={revealed ? "隐藏" : "显示"}
          onClick={() => setRevealed((on) => !on)}
        >
          {revealed ? <EyeOffIcon size={13} /> : <EyeIcon size={13} />}
        </button>
      ) : null}
      {onBrowse ? (
        <button
          type="button"
          className="wset-input__btn"
          aria-label={browseTitle ?? "浏览"}
          title={browseTitle}
          onClick={onBrowse}
        >
          <FolderIcon size={13} />
        </button>
      ) : null}
    </div>
  );
}

/** 热键录制框：186×34，点击 / 空格 / 回车开始录制，Esc 取消，Backspace 清空 */
export function HotkeyField({
  value,
  onChange,
  onRecordingChange,
  ariaLabel,
}: {
  value: string;
  onChange: (next: string) => void;
  /** 录制状态变化：窗口据此让 Esc 只取消录制，不弹「放弃修改」 */
  onRecordingChange?: (recording: boolean) => void;
  ariaLabel?: string;
}) {
  const [recording, setRecording] = useState(false);
  const [flash, setFlash] = useState("");
  const flashTimer = useRef<number | null>(null);
  const ref = useRef<HTMLDivElement | null>(null);
  const normalized = value.trim();

  const setRecordingState = useCallback(
    (next: boolean) => {
      setRecording(next);
      onRecordingChange?.(next);
    },
    [onRecordingChange]
  );

  useEffect(
    () => () => {
      if (flashTimer.current != null) window.clearTimeout(flashTimer.current);
    },
    []
  );

  const fail = (message: string) => {
    setFlash(message);
    if (flashTimer.current != null) window.clearTimeout(flashTimer.current);
    flashTimer.current = window.setTimeout(() => setFlash(""), 1300);
  };

  const begin = () => {
    setFlash("");
    setRecordingState(true);
    ref.current?.focus();
  };

  const end = () => {
    setFlash("");
    setRecordingState(false);
  };

  const clear = () => {
    end();
    if (normalized) onChange("");
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (!recording) {
      if (event.key === " " || event.key === "Enter") {
        event.preventDefault();
        begin();
      }
      return;
    }
    // 录制中：所有按键都不再冒泡（避免触发窗口的 Ctrl+S / Esc 关闭）
    event.preventDefault();
    event.stopPropagation();

    const key = event.key;
    if (key === "Escape") {
      end();
      ref.current?.blur();
      return;
    }
    if (key === "Tab") {
      // 交还焦点继续 Tab 导航
      end();
      return;
    }
    if (key === "Backspace" || key === "Delete") {
      clear();
      return;
    }
    if (["Control", "Shift", "Alt", "Meta", "CapsLock", "NumLock", "ScrollLock"].includes(key)) {
      return;
    }

    let main = "";
    if (key.length === 1 && /[a-zA-Z0-9]/.test(key)) main = key.toUpperCase();
    else if (/^F([1-9]|1[0-9]|2[0-4])$/i.test(key)) main = key.toUpperCase();
    else if (key === " " || key === "Spacebar") main = "Space";
    if (!main) {
      fail("这个键不能用作热键");
      return;
    }

    const mods: string[] = [];
    if (event.ctrlKey) mods.push("Ctrl");
    if (event.altKey) mods.push("Alt");
    if (event.shiftKey) mods.push("Shift");
    if (event.metaKey) mods.push("Win");
    if (!mods.length && !main.startsWith("F")) {
      fail("需要 Ctrl / Alt / Shift / Win");
      return;
    }
    onChange([...mods, main].join("+"));
    setRecordingState(false);
    ref.current?.blur();
  };

  const labelClass = recording
    ? flash
      ? "wset-hotkey__label wset-hotkey__label--danger"
      : "wset-hotkey__label wset-hotkey__label--accent"
    : flash
      ? "wset-hotkey__label wset-hotkey__label--danger"
      : normalized
        ? "wset-hotkey__label"
        : "wset-hotkey__label wset-hotkey__label--hint";

  return (
    <div
      ref={ref}
      role="button"
      tabIndex={0}
      aria-label={ariaLabel}
      className={`wset-hotkey${recording ? " wset-hotkey--rec" : ""}`}
      onClick={begin}
      onBlur={() => {
        if (recording) end();
      }}
      onKeyDown={onKeyDown}
    >
      <span className={labelClass}>
        {recording ? flash || "按下快捷键…" : flash || normalized || "点击录制"}
      </span>
      {normalized && !recording ? (
        <button
          type="button"
          className="wset-hotkey__clear"
          aria-label="清除"
          onClick={(event) => {
            event.stopPropagation();
            clear();
          }}
        >
          <CloseIcon size={9} />
        </button>
      ) : null}
    </div>
  );
}

// ============================================================ 多选 / 字体

/** 多选胶囊：28 高，选中为强调色淡底 + 勾图标 */
export function Chips({
  options,
  value,
  onChange,
  ariaLabel,
}: {
  options: readonly { value: string; label: string }[];
  value: string[];
  onChange: (next: string[]) => void;
  ariaLabel?: string;
}) {
  const toggle = (key: string) => {
    if (value.includes(key)) onChange(value.filter((item) => item !== key));
    else onChange([...value, key]);
  };
  return (
    <div className="wset-chips" role="group" aria-label={ariaLabel}>
      {options.map((option) => {
        const on = value.includes(option.value);
        return (
          <button
            key={option.value}
            type="button"
            aria-pressed={on}
            className={`wset-chip${on ? " wset-chip--on" : ""}`}
            onClick={() => toggle(option.value)}
          >
            <span>{option.label}</span>
            {on ? <ChipCheckIcon size={12} /> : null}
          </button>
        );
      })}
    </div>
  );
}

/** 字体选择：226×34，点开 620×560 的本机字体库弹窗 */
export function FontPicker({
  value,
  fonts,
  loading,
  onChange,
}: {
  value: string;
  fonts: SettingsFont[];
  loading?: boolean;
  onChange: (family: string) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        className="wset-font"
        aria-haspopup="dialog"
        onClick={() => setOpen(true)}
        style={value ? { fontFamily: fontFamilyWithFallback(value, "var(--w-font)") } : undefined}
      >
        <span className={`wset-font__label${value ? "" : " wset-font__label--hint"}`}>
          {loading ? "读取字体…" : value || "默认字体"}
        </span>
        <span className="wset-font__chev">
          <DownArrowIcon size={16} />
        </span>
      </button>
      {open ? (
        <FontDialog
          current={value}
          fonts={fonts}
          onCancel={() => setOpen(false)}
          onPick={(family) => {
            setOpen(false);
            if (family !== value) onChange(family);
          }}
        />
      ) : null}
    </>
  );
}

/** 本机字体库弹窗：搜索 / 只看中文 / 38 高列表 / 预览 / 取消·使用这个字体 */
function FontDialog({
  current,
  fonts,
  onPick,
  onCancel,
}: {
  current: string;
  fonts: SettingsFont[];
  onPick: (family: string) => void;
  onCancel: () => void;
}) {
  const [query, setQuery] = useState("");
  const [zhOnly, setZhOnly] = useState(false);
  const [selected, setSelected] = useState(current);
  const listRef = useRef<HTMLDivElement | null>(null);

  const rows = fonts.filter((font) => {
    if (zhOnly && !font.has_chinese) return false;
    const text = font.family.toLowerCase();
    return !query.trim() || text.includes(query.trim().toLowerCase());
  });
  const shownCount = rows.length;
  const currentFont = fonts.find((font) => font.family === selected);

  // Esc 取消 / Enter 确认：capture 阶段接管，避免同时触发窗口的关闭逻辑
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCancel();
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        event.stopPropagation();
        if (selected === "" || shownCount > 0) onPick(selected);
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [onCancel, onPick, selected, shownCount]);

  // 默认把当前字体滚到可见位置
  useLayoutEffect(() => {
    const row = listRef.current?.querySelector<HTMLElement>("[data-selected='1']");
    row?.scrollIntoView({ block: "center" });
  }, []);

  const previewNote = (() => {
    if (!selected) {
      return {
        text: "默认字体：内置字体栈（HarmonyOS Sans → MiSans → 微软雅黑 UI）",
        tone: "hint" as const,
      };
    }
    if (!shownCount) return { text: "没有匹配的字体，换个关键词试试。", tone: "danger" as const };
    if (currentFont?.has_chinese) {
      return { text: `${selected} · 含中文字形，灵动岛和设置窗口都会换用它。`, tone: "ok" as const };
    }
    return {
      text: `${selected} · 不含中文字形，中文会自动回退到系统字体。`,
      tone: "accent" as const,
    };
  })();

  return createPortal(
    <div className="wset-modal" onMouseDown={onCancel}>
      <div
        className="wset-modal__card wset-fontmodal"
        role="dialog"
        aria-modal="true"
        aria-label="选择字体"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="wset-fontmodal__head">
          <span className="wset-fontmodal__title">选择字体</span>
          <button type="button" className="wset-iconbtn" aria-label="关闭" onClick={onCancel}>
            <CloseIcon size={12} />
          </button>
        </div>
        <div className="wset-fontmodal__count">
          读取到本机 {fonts.length} 个字体
          {query.trim() || zhOnly ? ` · 匹配 ${shownCount} 个` : ""}。
          选中的字体会应用到灵动岛、设置窗口和右键菜单。
        </div>
        <div className="wset-fontmodal__search">
          <TextInput
            wide
            value={query}
            onChange={setQuery}
            placeholder="搜索字体名字…"
            ariaLabel="搜索字体"
          />
          <button
            type="button"
            aria-pressed={zhOnly}
            className={`wset-chip${zhOnly ? " wset-chip--on" : ""}`}
            onClick={() => setZhOnly((on) => !on)}
          >
            只看中文字体
          </button>
        </div>
        <div className="wset-fontlist" ref={listRef}>
          <button
            type="button"
            className={`wset-fontrow${selected === "" ? " wset-fontrow--on" : ""}`}
            data-selected={selected === "" ? "1" : undefined}
            onClick={() => setSelected("")}
          >
            <span>默认字体</span>
            <span className="wset-fontrow__note">内置字体栈</span>
          </button>
          {rows.map((font) => (
            <button
              key={font.family}
              type="button"
              className={`wset-fontrow${selected === font.family ? " wset-fontrow--on" : ""}`}
              data-selected={selected === font.family ? "1" : undefined}
              style={{ fontFamily: quoteFontFamily(font.family) }}
              onClick={() => setSelected(font.family)}
            >
              <span className="wset-fontrow__name">{font.family}</span>
              {font.has_chinese ? <span className="wset-fontrow__badge">中文</span> : null}
            </button>
          ))}
          {!rows.length ? <div className="wset-fontlist__empty">没有匹配的字体</div> : null}
        </div>
        <div
          className="wset-fontpreview"
          style={{ fontFamily: fontFamilyWithFallback(selected, SETTINGS_FONT_STACK) }}
        >
          中文字体预览&nbsp;&nbsp;AaBbCc&nbsp;&nbsp;0123456789
        </div>
        <div className={`wset-fontnote wset-fontnote--${previewNote.tone}`}>
          {previewNote.text}
        </div>
        <div className="wset-fontmodal__foot">
          <Button onClick={onCancel}>取消</Button>
          <Button
            variant="primary"
            disabled={selected !== "" && !shownCount}
            onClick={() => onPick(selected)}
          >
            使用这个字体
          </Button>
        </div>
      </div>
    </div>,
    document.body
  );
}

// ============================================================ 列表编辑器

/** 字符串列表：逐项输入、可选上下移动与删除 + 底部添加行 */
export function StringList({
  values,
  onChange,
  placeholder = "添加一项…",
  addLabel = "添加",
  reorderable = false,
}: {
  values: string[];
  onChange: (next: string[]) => void;
  placeholder?: string;
  addLabel?: string;
  reorderable?: boolean;
}) {
  const [draft, setDraft] = useState("");
  const update = (index: number, text: string) => {
    const next = [...values];
    next[index] = text;
    onChange(next);
  };
  const move = (index: number, delta: number) => {
    const to = index + delta;
    if (to < 0 || to >= values.length) return;
    const next = [...values];
    const [item] = next.splice(index, 1);
    next.splice(to, 0, item);
    onChange(next);
  };
  const add = () => {
    const text = draft.trim();
    if (!text) return;
    onChange([...values, text]);
    setDraft("");
  };
  return (
    <div className="wset-list">
      {values.map((value, index) => (
        <div className="wset-list__row" key={`${index}-${value}`}>
          <TextInput
            wide
            value={value}
            placeholder={placeholder}
            onChange={(text) => update(index, text)}
          />
          {reorderable ? (
            <>
              <button
                type="button"
                className="wset-iconbtn"
                aria-label="上移"
                disabled={index === 0}
                onClick={() => move(index, -1)}
              >
                <ArrowUpIcon size={12} />
              </button>
              <button
                type="button"
                className="wset-iconbtn"
                aria-label="下移"
                disabled={index === values.length - 1}
                onClick={() => move(index, 1)}
              >
                <ArrowDownIcon size={12} />
              </button>
            </>
          ) : null}
          <button
            type="button"
            className="wset-iconbtn"
            aria-label="删除"
            onClick={() => onChange(values.filter((_, i) => i !== index))}
          >
            <TrashIcon size={12} />
          </button>
        </div>
      ))}
      <div className="wset-list__add">
        <TextInput
          wide
          value={draft}
          placeholder={placeholder}
          onChange={setDraft}
        />
        <Button onClick={add}>
          <PlusIcon size={11} />
          {addLabel}
        </Button>
      </div>
    </div>
  );
}

export interface RecordFieldDef {
  key: string;
  /** 列上方的小标题；留空只显示控件（IMAP / 城市 / 启动器的纯 placeholder 列） */
  label?: string;
  placeholder?: string;
  secret?: boolean;
  width?: number;
  /** 横向伸缩系数（CSS flex-grow；默认 1，明文列可以给 2） */
  stretch?: number;
  /** 单元格类型：文本 / 整数 / 小数 / 路径 / 开关 */
  kind?: "text" | "int" | "number" | "path" | "switch";
  /** 路径列的浏览方式 */
  browse?: "file" | "dir" | "any";
  /** 新增一行时该列的默认值 */
  default?: unknown;
}

/** 数字单元格：输入过程中保留原文，失焦时才解析写回（避免「3.」丢小数点） */
function RecordNumberField({
  value,
  integer,
  placeholder,
  width,
  ariaLabel,
  onChange,
}: {
  value: unknown;
  integer?: boolean;
  placeholder?: string;
  width?: number;
  ariaLabel?: string;
  onChange: (next: number | null) => void;
}) {
  const format = (raw: unknown) => {
    if (typeof raw === "number" && Number.isFinite(raw)) {
      return integer ? String(Math.round(raw)) : String(raw);
    }
    if (typeof raw === "string" && raw.trim()) return raw;
    return "";
  };
  const [text, setText] = useState(() => format(value));
  const editing = useRef(false);

  useEffect(() => {
    if (!editing.current) setText(format(value));
    // format 只依赖 integer（其余是纯函数），value 变化时同步外部回填
  }, [value, integer]);

  const commit = () => {
    editing.current = false;
    const raw = text.trim();
    if (!raw) {
      if (text !== "") setText("");
      if (value !== null) onChange(null);
      return;
    }
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) {
      setText(format(value));
      return;
    }
    const next = integer ? Math.round(parsed) : parsed;
    setText(format(next));
    if (next !== value) onChange(next);
  };

  return (
    <TextInput
      wide={!width}
      value={text}
      width={width}
      placeholder={placeholder}
      ariaLabel={ariaLabel}
      inputMode={integer ? "numeric" : "decimal"}
      onFocus={() => {
        editing.current = true;
      }}
      onBlur={commit}
      onChange={(next) => {
        editing.current = true;
        setText(next);
      }}
    />
  );
}

/** 路径对话框（记录编辑器的路径列）：失败保持原值，不打断其它编辑 */
async function browseRecordPath(
  field: RecordFieldDef,
  apply: (path: string) => void
): Promise<void> {
  try {
    const path = await pickFile({ mode: "open", directory: field.browse === "dir" });
    if (path) apply(path);
  } catch {
    /* 浏览器预览 / ACL 拒绝时静默保留原值 */
  }
}

/** 结构化列表编辑器（天气城市 / IMAP 账户 / 启动器） */
export function RecordEditor({
  items,
  fields,
  onChange,
  addLabel = "添加一项",
  emptyLabel = "还没有内容",
  newItem,
  onAdd,
}: {
  items: Record<string, unknown>[];
  fields: RecordFieldDef[];
  onChange: (next: Record<string, unknown>[]) => void;
  addLabel?: string;
  emptyLabel?: string;
  /** 新增一行的初始值（覆盖 fields[].default） */
  newItem?: Record<string, unknown>;
  /** 提供后「添加」按钮改为触发它，不再插入空行（天气城市走选择器直加时用） */
  onAdd?: () => void;
}) {
  const update = (index: number, key: string, value: unknown) => {
    const next = items.map((item, i) => (i === index ? { ...item, [key]: value } : item));
    onChange(next);
  };
  const move = (index: number, delta: number) => {
    const to = index + delta;
    if (to < 0 || to >= items.length) return;
    const next = [...items];
    const [item] = next.splice(index, 1);
    next.splice(to, 0, item);
    onChange(next);
  };

  return (
    <div className="wset-list">
      {!items.length ? <div className="wset-list__empty">{emptyLabel}</div> : null}
      {items.map((item, index) => (
        <div className="wset-record" key={index}>
          <div className="wset-record__fields">
            {fields.map((field) => {
              const label = field.label ? <span>{field.label}</span> : null;
              const common = {
                placeholder: field.placeholder,
                width: field.width,
                ariaLabel: field.label || field.key,
              };
              if (field.kind === "switch") {
                return (
                  <div
                    className="wset-record__field wset-record__field--switch"
                    key={field.key}
                  >
                    {label}
                    <Switch
                      checked={Boolean(item[field.key])}
                      ariaLabel={field.label || field.key}
                      onChange={(next) => update(index, field.key, next)}
                    />
                  </div>
                );
              }
              return (
                <label
                  className="wset-record__field"
                  key={field.key}
                  style={
                    field.width
                      ? { flex: "0 0 auto", minWidth: field.width }
                      : { flex: field.stretch ?? 1 }
                  }
                >
                  {label}
                  {field.kind === "int" || field.kind === "number" ? (
                    <RecordNumberField
                      value={item[field.key]}
                      integer={field.kind === "int"}
                      placeholder={field.placeholder}
                      width={field.width}
                      ariaLabel={field.label || field.key}
                      onChange={(next) => update(index, field.key, next)}
                    />
                  ) : (
                    <TextInput
                      {...common}
                      wide={!field.width}
                      value={String(item[field.key] ?? "")}
                      secret={field.secret}
                      onBrowse={
                        field.browse
                          ? () => void browseRecordPath(field, (path) => update(index, field.key, path))
                          : undefined
                      }
                      browseTitle={field.browse === "dir" ? "选择文件夹" : "选择文件"}
                      onChange={(text) => update(index, field.key, text)}
                    />
                  )}
                </label>
              );
            })}
          </div>
          <div className="wset-record__ops">
            <button
              type="button"
              className="wset-iconbtn"
              aria-label="上移"
              disabled={index === 0}
              onClick={() => move(index, -1)}
            >
              <ArrowUpIcon size={12} />
            </button>
            <button
              type="button"
              className="wset-iconbtn"
              aria-label="下移"
              disabled={index === items.length - 1}
              onClick={() => move(index, 1)}
            >
              <ArrowDownIcon size={12} />
            </button>
            <button
              type="button"
              className="wset-iconbtn"
              aria-label="删除"
              onClick={() => onChange(items.filter((_, i) => i !== index))}
            >
              <TrashIcon size={12} />
            </button>
          </div>
        </div>
      ))}
      <div className="wset-list__add">
        <Button
          onClick={() => {
            if (onAdd) {
              onAdd();
              return;
            }
            const row: Record<string, unknown> = {};
            for (const field of fields) {
              if (field.kind === "switch") row[field.key] = field.default ?? true;
              else if (field.kind === "int" || field.kind === "number") {
                row[field.key] = field.default ?? null;
              } else row[field.key] = field.default ?? "";
            }
            onChange([...items, { ...row, ...newItem }]);
          }}
        >
          <PlusIcon size={11} />
          {addLabel}
        </Button>
      </div>
    </div>
  );
}

// ============================================================ 按钮 / 确认框 / 状态

/** 胶囊按钮：primary 强调色、ghost 浅底、danger 红字（32 高，radius 999） */
export function Button({
  children,
  variant = "ghost",
  size = "md",
  icon,
  onClick,
  disabled,
  title,
  ariaLabel,
}: {
  children: ReactNode;
  variant?: "primary" | "ghost" | "danger";
  size?: "md" | "sm";
  icon?: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  title?: string;
  ariaLabel?: string;
}) {
  return (
    <button
      type="button"
      className={`wset-btn wset-btn--${variant}${size === "sm" ? " wset-btn--sm" : ""}`}
      onClick={onClick}
      disabled={disabled}
      title={title}
      aria-label={ariaLabel}
    >
      {icon ? <span className="wset-btn__icon">{icon}</span> : null}
      <span>{children}</span>
    </button>
  );
}

/** 圆形图标按钮（标题栏 / 列表操作） */
export function IconButton({
  children,
  onClick,
  size = 30,
  ariaLabel,
  title,
  disabled,
}: {
  children: ReactNode;
  onClick?: () => void;
  size?: number;
  ariaLabel: string;
  title?: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className="wset-iconbtn wset-iconbtn--round"
      style={{ width: size, height: size }}
      onClick={onClick}
      aria-label={ariaLabel}
      title={title ?? ariaLabel}
      disabled={disabled}
    >
      {children}
    </button>
  );
}

/** 确认框：420 宽、圆角 18，Esc 取消 / Enter 确认 */
export function Confirm({
  open,
  title,
  body,
  okText = "确定",
  cancelText = "取消",
  danger,
  onOk,
  onCancel,
}: {
  open: boolean;
  title: string;
  body: string;
  okText?: string;
  cancelText?: string;
  danger?: boolean;
  onOk: () => void;
  onCancel: () => void;
}) {
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCancel();
      } else if (event.key === "Enter") {
        event.preventDefault();
        event.stopPropagation();
        onOk();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [open, onCancel, onOk]);

  if (!open) return null;
  return createPortal(
    <div className="wset-modal" onMouseDown={onCancel}>
      <div
        className="wset-modal__card wset-confirm"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="wset-confirm__title">{title}</div>
        <div className="wset-confirm__body">{body}</div>
        <div className="wset-confirm__foot">
          <Button onClick={onCancel}>{cancelText}</Button>
          <Button variant={danger ? "danger" : "primary"} onClick={onOk}>
            {okText}
          </Button>
        </div>
      </div>
    </div>,
    document.body
  );
}

/** 页脚状态圆点：颜色随状态语义（hint / accent / ok / danger） */
export function StatusDot({ role }: { role: "hint" | "accent" | "ok" | "danger" }) {
  return <span className={`wset-dot wset-dot--${role}`} aria-hidden="true" />;
}
