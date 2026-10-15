/**
 * 自定义时间范围控件（主窗口三页共用的页头时间范围触发按钮 + 弹出面板）。
 *
 * 形态参考 cc-switch 的 UsageDateRangePicker（自研实现，不搬代码），替掉原先
 * 「原生 date input + 分段控件末档自定义」的做法：
 *  - **触发按钮**：显示当前范围标签（生效的预设名，或自定义区间的
 *    「MM-DD ~ MM-DD」摘要）+ 下箭头 chevron；自定义生效时额外带日历图标并
 *    高亮（主色描边 + 浅底），与预设档一眼可分；
 *  - **弹出面板**（左右两列）：
 *      · 顶部一行预设快捷按钮（当前生效的预设高亮描边；点预设**立即生效并
 *        关闭面板**，由页面通过 presets.onClick 完成切换）；
 *      · 左列起止字段卡各一张（小标签 + 日期文本，点卡片激活该字段，激活态
 *        描边 + ring；日粒度，不含时间输入）、可选范围说明 / 错误文案、
 *        「取消 / 应用」按钮（起 > 止时应用禁用并显示错误）；
 *      · 右列自绘单月日历（见 DateRangeCalendar：周一开头 42 格、月份导航、
 *        今日描边环、范围内浅底、起止实心主色、minMs 之前禁用）。
 *
 * 区间语义（与总览 / 日志 / 对比三页的本地自然日口径一致）：
 *   [fromMs, toMs) 半开区间；fromMs = 起始日本地 0 点，
 *   toMs = 结束日**次日** 0 点（即结束日整天都含在区间内）。
 *
 * 日历点选状态机（cc-switch 同款交互）：
 *  - activeField = "from"：点某天 → 设为起始，自动推进到 "to"；
 *  - activeField = "to"：点某天 ≥ 起始 → 设为结束；点早于起始的天 → 视为
 *    **重新选起**（起点改为该天，仍停在 "to" 继续选止）。
 *  起 > 止（含改起后越过原止）时应用禁用并给出错误文案。
 *
 * 关闭路径：点击组件外、Esc、取消（Esc / 取消都把焦点交还触发器）；
 * 应用后同样关闭。面板贴在触发器下方，下方空间不足且上方更宽裕时翻到上方
 * （量的是缩放后视觉坐标，与 --ui-scale 兼容）；右缘对齐触发器（触发器在页头
 * 范围区，向左展开不探出主区），max-width calc(100vw - 24px) + 展开后水平 /
 * 垂直夹取兜底 900×640 最小窗口（向左仍溢出时整体右移，保证面板完整可见）。
 * 夹取边界是最近裁切祖先（主区滚动容器 .db-main、日志页 .db-logs-page 等
 * overflow 容器）的可见盒而非视口——页头 actions 因窗口过窄换行贴主区左缘
 * 时，按视口夹取会把面板左半裁进侧边栏外，必须以真正裁切者的可见区域为准。
 *
 * 配色全部走 --db-* 变量（面板底用实底 --db-pop-bg，不写 slate-900 类——
 * 本项目 .dark 会反转 slate 色板），亮暗两套见 dashboard.css。
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useI18n, type MessageKey } from "../i18n";
import type { Locale } from "../i18n/locale";
import {
  DateRangeCalendar,
  addDays,
  dayText,
  startOfDay,
  startOfMonth,
} from "./DateRangeCalendar";

/** 自定义时间范围（本地自然日半开区间 [fromMs, toMs)）：
 *  fromMs = 起始日 0 点，toMs = 结束日次日 0 点（含结束日整天）。 */
export interface CustomRangeValue {
  fromMs: number;
  toMs: number;
}

/** 面板顶部预设快捷项：label 为已 i18n 的显示名，active = 当前生效档（高亮描边）。
 *  点击由页面完成切换（组件只负责点击后收起面板），组件保持通用。 */
export interface RangePreset {
  label: string;
  active?: boolean;
  onClick: () => void;
}

const pad = (n: number) => String(n).padStart(2, "0");

/**
 * 区间摘要文案（页头触发按钮与卡片副标题共用）：单日 "MM-DD"、跨年时起始日带
 * 年份；两端用 " ~ " 连接。分隔符跟随语言（zh 用 "-"、en 用 "/"），与各页
 * 趋势轴标签的紧凑日期口径一致。toMs 为半开区间终点，摘要显示其前一日。
 */
export function formatRangeSummary(
  fromMs: number,
  toMs: number,
  locale: Locale
): string {
  const lastMs = toMs > fromMs ? toMs - 1 : fromMs;
  const from = new Date(fromMs);
  const to = new Date(lastMs);
  const sep = locale === "en" ? "/" : "-";
  const day = (d: Date) => `${pad(d.getMonth() + 1)}${sep}${pad(d.getDate())}`;
  const fromText =
    from.getFullYear() === to.getFullYear()
      ? day(from)
      : `${from.getFullYear()}${sep}${day(from)}`;
  return `${fromText} ~ ${day(to)}`;
}

/** 面板默认跨度（天）：未选过自定义时预填最近 7 天，保证面板一打开就有可应用的值 */
const DEFAULT_SPAN_DAYS = 7;

/** 下箭头（展开时旋转 180°；与 DropdownMenu 的 CaretIcon 同款形态） */
function ChevronIcon({ open }: { open: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className={`db-range-trigger__chevron${open ? " is-open" : ""}`}
    >
      <path d="m6 9 6 6 6-6" />
    </svg>
  );
}

/** 日历图标（自定义生效时出现在触发按钮标签前） */
function CalendarIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className="db-range-trigger__icon"
    >
      <rect x="3" y="5" width="18" height="16" rx="2.5" />
      <path d="M3 10h18" />
      <path d="M8 3v4" />
      <path d="M16 3v4" />
    </svg>
  );
}

/** 从 start 的父级向上找最近的裁切祖先：overflowX / overflowY 任一命中
 *  auto / scroll / hidden / clip / overlay 即返回该元素——超出其 padding box
 *  的内容会被直接裁掉（且大多滚不到），面板的夹取兜底要以它的可见盒为边界
 *  （如主区滚动容器 .db-main、日志页 .db-logs-page）。到顶没有则返回 null。 */
function nearestClipAncestor(start: HTMLElement | null): HTMLElement | null {
  const clipOverflows = ["auto", "scroll", "hidden", "clip", "overlay"];
  let node = start?.parentElement ?? null;
  while (node) {
    const style = getComputedStyle(node);
    if (
      clipOverflows.includes(style.overflowX) ||
      clipOverflows.includes(style.overflowY)
    ) {
      return node;
    }
    node = node.parentElement;
  }
  return null;
}

export function CustomRangeMenu({
  value,
  onChange,
  presets,
  minMs,
  maxMs,
  triggerLabel,
  ariaLabel,
}: {
  /** 当前自定义区间（null = 未启用自定义，当前是预设档） */
  value: CustomRangeValue | null;
  /** 应用回调：仅在起止有效时触发（预设档切换由页面负责清空自定义） */
  onChange: (next: CustomRangeValue) => void;
  /** 面板顶部预设快捷行（各页现有 RANGES 列表；点击即生效并关闭面板） */
  presets: RangePreset[];
  /** 可选最早日期（本地自然日；不传 = 不限制，面板内说明下限） */
  minMs?: number;
  /** 可选最晚日期（本地自然日；默认今天） */
  maxMs?: number;
  /** 触发按钮文案（各页现有 rangeLabel：预设名或区间摘要）；缺省时组件自行
   *  推导（自定义摘要 → 生效预设名 → 「自定义」） */
  triggerLabel?: string;
  /** 触发按钮 aria-label（缺省用「自定义时间范围」） */
  ariaLabel?: string;
}) {
  const { locale, t } = useI18n();
  const [open, setOpen] = useState(false);
  // 面板展开方向：down 正常向下；up 下方空间不足时翻到触发器上方
  const [placement, setPlacement] = useState<"down" | "up">("down");
  // 水平兜底位移：面板默认右缘对齐触发器，触发器左侧空间不足（页头 actions
  // 因窗口过窄换行贴主区左缘时最典型）时整体右移；夹取边界取最近裁切祖先
  // （.db-main / .db-logs-page 等滚动或 overflow 容器）的可见盒而非视口——
  // 换行贴左时按视口夹取会把面板左半裁进侧边栏外，必须以真正裁切者为准
  const [shiftX, setShiftX] = useState(0);
  // 草稿状态：面板打开时按当前值（或默认近 7 天）重置，应用前不影响页面
  const [fromDay, setFromDay] = useState<number | null>(null);
  const [toDay, setToDay] = useState<number | null>(null);
  // 当前正在编辑的字段（日历点选的目标；见文件头状态机）
  const [activeField, setActiveField] = useState<"from" | "to">("from");
  // 日历展示月（该月 1 号 0 点）
  const [viewMonth, setViewMonth] = useState(() => startOfMonth(Date.now()));
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  const maxDay = startOfDay(maxMs ?? Date.now());
  const minDay = minMs != null ? startOfDay(minMs) : null;
  const todayDay = startOfDay(Date.now());

  /** 展开面板：用当前选中区间（或默认最近 7 天，受下限约束）初始化草稿 */
  function openPanel() {
    const end = value ? startOfDay(value.toMs - 1) : maxDay;
    let start = value ? startOfDay(value.fromMs) : addDays(end, -(DEFAULT_SPAN_DAYS - 1));
    if (minDay != null && start < minDay) start = minDay;
    if (start > end) start = end;
    setFromDay(start);
    setToDay(end);
    setActiveField("from");
    setViewMonth(startOfMonth(start));
    setOpen(true);
  }

  /** 收起面板（点击外部 / Esc / 取消 / 应用后调用） */
  function closePanel() {
    setOpen(false);
  }

  /** 关闭并把焦点交还触发器（键盘操作不断链） */
  function closeAndFocus() {
    closePanel();
    triggerRef.current?.focus();
  }

  // 展开时：点击组件外 / Esc 关闭；Esc 同时把焦点交还触发器
  useEffect(() => {
    if (!open) return;
    function onDown(e: MouseEvent) {
      if (
        rootRef.current &&
        e.target instanceof Node &&
        !rootRef.current.contains(e.target)
      ) {
        closePanel();
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      // 收起并把焦点交还触发器（setOpen / ref 都是稳定引用，无闭包陈旧问题）
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

  // 展开后（绘制前）量一次空间：真正裁切面板的是最近的 overflow 祖先（主区
  // 滚动容器 .db-main、日志页 .db-logs-page 等），上下可用空间与水平夹取都以
  // 该祖先的可见盒为边界（组件外没有 overflow 容器时退回视口）——页头
  // actions 因窗口过窄换行贴主区左缘时，按视口量会把面板左半裁进侧边栏外。
  // 下方装不下面板而上方更宽裕时翻到上方；水平夹进可见盒（12px 安全边距），
  // 向左溢出的部分无法滚到，必须主动右移（与 DropdownMenu 同款测量做法，
  // 量的是缩放后的视觉坐标，与 --ui-scale 兼容）
  useLayoutEffect(() => {
    if (!open) {
      setPlacement("down");
      setShiftX(0);
      return;
    }
    const panel = panelRef.current;
    const trigger = triggerRef.current;
    if (!panel || !trigger) return;
    const triggerRect = trigger.getBoundingClientRect();
    const panelRect = panel.getBoundingClientRect();
    const clipRect = nearestClipAncestor(rootRef.current)?.getBoundingClientRect();
    const viewLeft = clipRect?.left ?? 0;
    const viewRight = clipRect?.right ?? window.innerWidth;
    const viewTop = clipRect?.top ?? 0;
    const viewBottom = clipRect?.bottom ?? window.innerHeight;
    const spaceBelow = viewBottom - triggerRect.bottom;
    const spaceAbove = triggerRect.top - viewTop;
    setPlacement(
      panelRect.height + 8 > spaceBelow && spaceAbove > spaceBelow ? "up" : "down"
    );
    const margin = 12;
    const overflowLeft = viewLeft + margin - panelRect.left;
    const overflowRight = panelRect.right - (viewRight - margin);
    setShiftX(
      overflowLeft > 0 ? overflowLeft : overflowRight > 0 ? -overflowRight : 0
    );
  }, [open]);

  /** 日历点选：先选起（自动推进到止）；选止时早于起始 → 视为重新选起 */
  function pickDay(day: number) {
    if (activeField === "from") {
      setFromDay(day);
      setActiveField("to");
      return;
    }
    if (fromDay != null && day < fromDay) {
      setFromDay(day);
      setActiveField("to");
      return;
    }
    setToDay(day);
    setActiveField("from");
  }

  /** 字段卡点击：激活该字段并把日历翻到该日期所在月（无值时留在当前月） */
  function activateField(field: "from" | "to") {
    setActiveField(field);
    const day = field === "from" ? fromDay : toDay;
    if (day != null) setViewMonth(startOfMonth(day));
  }

  // 校验：任一为空 / 起晚于止 / 越出可选范围 → 给出说明并禁用「应用」
  let errorKey: MessageKey | null = null;
  if (fromDay == null || toDay == null) {
    errorKey = "dashboard.customRange.errorIncomplete";
  } else if (fromDay > toDay) {
    errorKey = "dashboard.customRange.errorOrder";
  } else if (
    (minDay != null && fromDay < minDay) ||
    fromDay > maxDay ||
    toDay > maxDay
  ) {
    errorKey = "dashboard.customRange.errorOutOfRange";
  }

  function apply() {
    if (fromDay == null || toDay == null || errorKey != null) return;
    // 半开区间：结束日次日 0 点（结束日整天都计入）
    onChange({ fromMs: fromDay, toMs: addDays(toDay, 1) });
    closeAndFocus();
  }

  // 触发按钮文案：页面传入的 rangeLabel 优先，缺省按「自定义摘要 → 生效预设名
  // → 自定义」推导（预设名由调用方的 presets 提供，组件不依赖各页的档位定义）
  const summary =
    value != null ? formatRangeSummary(value.fromMs, value.toMs, locale) : "";
  const presetLabel = presets.find((item) => item.active)?.label ?? "";
  const label =
    triggerLabel ?? (value != null ? summary : presetLabel || t("dashboard.customRange"));
  const custom = value != null;

  return (
    <div ref={rootRef} className="db-range-menu">
      <button
        ref={triggerRef}
        type="button"
        className={`db-range-trigger${custom ? " db-range-trigger--active" : ""}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={ariaLabel ?? t("dashboard.customRange.title")}
        title={
          custom
            ? t("dashboard.customRange.selectedHint", { range: summary })
            : t("dashboard.customRange.title")
        }
        onClick={() => (open ? closeAndFocus() : openPanel())}
      >
        {custom && <CalendarIcon />}
        <span className="db-range-trigger__label">{label}</span>
        <ChevronIcon open={open} />
      </button>
      {open && (
        <div
          ref={panelRef}
          className={`db-range-panel${
            placement === "up" ? " db-range-panel--up" : ""
          }`}
          style={shiftX !== 0 ? { transform: `translateX(${shiftX}px)` } : undefined}
          role="dialog"
          aria-label={t("dashboard.customRange.title")}
        >
          {/* 预设快捷行：当前生效档高亮描边；点击即生效并收起（切换由页面完成） */}
          <div
            className="db-range-presets"
            role="group"
            aria-label={t("dashboard.customRange.presets")}
          >
            {presets.map((preset) => (
              <button
                key={preset.label}
                type="button"
                className={`db-range-preset${
                  preset.active ? " db-range-preset--active" : ""
                }`}
                aria-pressed={preset.active === true}
                onClick={() => {
                  preset.onClick();
                  closeAndFocus();
                }}
              >
                {preset.label}
              </button>
            ))}
          </div>

          <div className="db-range-cols">
            {/* 左列：起止字段卡 + 范围说明 / 错误 + 取消 / 应用 */}
            <div className="db-range-fields">
              <button
                type="button"
                className={`db-range-field${
                  activeField === "from" ? " db-range-field--active" : ""
                }`}
                aria-pressed={activeField === "from"}
                onClick={() => activateField("from")}
              >
                <span className="db-range-field__label">
                  {t("dashboard.customRange.start")}
                </span>
                <span
                  className={`db-range-field__value num${
                    fromDay == null ? " db-range-field__value--empty" : ""
                  }`}
                >
                  {fromDay != null ? dayText(fromDay) : t("dashboard.customRange.empty")}
                </span>
              </button>
              <button
                type="button"
                className={`db-range-field${
                  activeField === "to" ? " db-range-field--active" : ""
                }`}
                aria-pressed={activeField === "to"}
                onClick={() => activateField("to")}
              >
                <span className="db-range-field__label">
                  {t("dashboard.customRange.end")}
                </span>
                <span
                  className={`db-range-field__value num${
                    toDay == null ? " db-range-field__value--empty" : ""
                  }`}
                >
                  {toDay != null ? dayText(toDay) : t("dashboard.customRange.empty")}
                </span>
              </button>

              {/* 可选范围下限说明（各页限制不同：总览 = 取数窗口 365 天，
                  对比页 = 额度快照保留期；日志页不传 minMs 即不显示） */}
              {minDay != null && (
                <p className="db-range-panel__hint">
                  {t("dashboard.customRange.minHint", { date: dayText(minDay) })}
                </p>
              )}
              {errorKey != null && (
                <p className="db-range-panel__error" role="alert">
                  {t(errorKey)}
                </p>
              )}
              <div className="db-range-panel__actions">
                <button
                  type="button"
                  className="db-btn db-btn--sm"
                  onClick={closeAndFocus}
                >
                  {t("dashboard.customRange.cancel")}
                </button>
                <button
                  type="button"
                  className="db-btn db-btn--sm db-range-panel__apply"
                  disabled={errorKey != null}
                  onClick={apply}
                >
                  {t("dashboard.customRange.apply")}
                </button>
              </div>
            </div>

            {/* 右列：自绘单月日历（周一开头，minMs / maxMs 之外禁用） */}
            <DateRangeCalendar
              viewMonth={viewMonth}
              onViewMonth={setViewMonth}
              fromDay={fromDay}
              toDay={toDay}
              todayDay={todayDay}
              minDay={minDay}
              maxDay={maxDay}
              onPickDay={pickDay}
            />
          </div>
        </div>
      )}
    </div>
  );
}
