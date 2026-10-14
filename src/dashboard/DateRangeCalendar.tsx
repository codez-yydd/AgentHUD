/**
 * 自绘单月日历（自定义时间范围面板的右列）。
 *
 * 形态参考 cc-switch 的 UsageDateRangePicker（自研实现，不搬代码）：月份导航行
 * （‹ 月份名 ›，点月份名回本月）+ 周标题行（locale 窄 weekday，**周一开头**，
 * 与热力图 HeatmapCard 的轮转口径一致）+ 7 列日格。日格状态：
 *  - 非本月日期：淡化且不可点（只作为上一/下一月的对齐占位）；
 *  - 今日：描边环（--today，与热力图同款语义）；
 *  - 选中范围内（起止之间）：浅色底（--in-range）；
 *  - 起 / 止日：实心主色（sky 系）圆角块（--edge）；
 *  - 超出可选区间（minMs 之前 / maxMs 之后）：禁用淡化。
 *
 * 日期工具（本地自然日 0 点、月份加减、周一开头的 42 格月网格）都在本文件内，
 * 带注释；日期口径与总览 / 日志 / 对比三页一致（本地时区自然日，不使用
 * toISOString 之类 UTC 换算，避免错日）。
 */
import { useMemo } from "react";
import { useI18n } from "../i18n";
import { dateLocale } from "../i18n/locale";
import type { Locale } from "../i18n/locale";

/** 月网格行数 / 列数：6 行 × 7 列 = 42 格，恒覆盖任意月份（含 31 天 + 错位） */
export const CAL_ROWS = 6;
export const CAL_COLS = 7;

const pad2 = (n: number) => String(n).padStart(2, "0");

/** 本地自然日 0 点 */
export function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 加减自然日（跨月 / 跨年 / 夏令时交给 Date） */
export function addDays(ms: number, days: number): number {
  const d = new Date(ms);
  d.setDate(d.getDate() + days);
  return d.getTime();
}

/** 该日所在月的 1 号 0 点（月份网格的对齐基准） */
export function startOfMonth(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  d.setDate(1);
  return d.getTime();
}

/** 月份加减：先归一到 1 号再挪月，避免 31 号挪到 2 月被 Date 溢出到 3 月 */
export function addMonths(ms: number, delta: number): number {
  const d = new Date(startOfMonth(ms));
  d.setMonth(d.getMonth() + delta);
  return d.getTime();
}

/**
 * 周一开头的 42 格月网格：首格 = 该月 1 号所在周的周一，逐格 +1 天。
 * JS 的 getDay() 是周日 = 0，先 `(getDay()+6)%7` 轮转为 周一 = 0 再回退偏移。
 */
export function monthGrid(viewMonthMs: number): number[] {
  const first = startOfMonth(viewMonthMs);
  const offset = (new Date(first).getDay() + 6) % 7;
  const gridStart = addDays(first, -offset);
  return Array.from({ length: CAL_ROWS * CAL_COLS }, (_, i) =>
    addDays(gridStart, i)
  );
}

/** 一天 → "YYYY-MM-DD"（本地时区；面板字段卡与可选下限说明共用） */
export function dayText(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

/** 两个自然日是否同一天（null 之间不算相同） */
export function sameDay(a: number | null, b: number | null): boolean {
  return a != null && b != null && a === b;
}

/** 月份标题：按语言给 "2026年10月" / "October 2026"（Intl 按月名本地化） */
export function monthTitle(ms: number, locale: Locale): string {
  return new Intl.DateTimeFormat(dateLocale(locale), {
    year: "numeric",
    month: "long",
  }).format(new Date(ms));
}

export function DateRangeCalendar({
  viewMonth,
  onViewMonth,
  fromDay,
  toDay,
  todayDay,
  minDay,
  maxDay,
  onPickDay,
}: {
  /** 当前展示月（该月 1 号 0 点） */
  viewMonth: number;
  /** 月份切换（‹ / › 与「回本月」按钮） */
  onViewMonth: (next: number) => void;
  /** 草稿起始日（本地 0 点；null = 尚未选起） */
  fromDay: number | null;
  /** 草稿结束日（本地 0 点；null = 尚未选止） */
  toDay: number | null;
  /** 今日（本地 0 点；描边环标记） */
  todayDay: number;
  /** 可选最早 / 最晚日期（本地 0 点）；minDay 为 null = 不限最早 */
  minDay: number | null;
  maxDay: number;
  /** 点选某天（状态机在面板组件里：先起后止，止 < 起视为重新选起） */
  onPickDay: (day: number) => void;
}) {
  const { locale, t } = useI18n();
  const days = useMemo(() => monthGrid(viewMonth), [viewMonth]);
  const title = useMemo(() => monthTitle(viewMonth, locale), [viewMonth, locale]);
  // 日格 title / aria-label 的完整日期（按月名本地化，一次构造复用 42 格）
  const dayFmt = useMemo(
    () =>
      new Intl.DateTimeFormat(dateLocale(locale), {
        year: "numeric",
        month: "long",
        day: "numeric",
      }),
    [locale]
  );
  // 周标题：复用词典 date.weekdays（周日开头），轮转为周一首列——与热力图口径一致
  const weekdayCells = t("date.weekdays").split(" ");
  const weekdays = [...weekdayCells.slice(1), weekdayCells[0]];

  const monthStart = startOfMonth(viewMonth);
  // 月份导航上下界：不越过可选区间所在的月份（更早 / 更晚的月份整月不可点）
  const canPrev = minDay == null || monthStart > startOfMonth(minDay);
  const canNext = monthStart < startOfMonth(maxDay);

  return (
    <div className="db-cal">
      <div className="db-cal__nav">
        <button
          type="button"
          className="db-cal__navbtn"
          disabled={!canPrev}
          aria-label={t("dashboard.customRange.prevMonth")}
          title={t("dashboard.customRange.prevMonth")}
          onClick={() => onViewMonth(addMonths(viewMonth, -1))}
        >
          ‹
        </button>
        <button
          type="button"
          className="db-cal__title"
          title={t("dashboard.customRange.thisMonth")}
          onClick={() => onViewMonth(startOfMonth(todayDay))}
        >
          {title}
        </button>
        <button
          type="button"
          className="db-cal__navbtn"
          disabled={!canNext}
          aria-label={t("dashboard.customRange.nextMonth")}
          title={t("dashboard.customRange.nextMonth")}
          onClick={() => onViewMonth(addMonths(viewMonth, 1))}
        >
          ›
        </button>
      </div>

      <div className="db-cal__weekdays" aria-hidden>
        {weekdays.map((name, i) => (
          <span key={`${name}-${i}`} className="db-cal__weekday">
            {name}
          </span>
        ))}
      </div>

      <div className="db-cal__grid">
        {days.map((day) => {
          const outside = day < monthStart || day >= addMonths(monthStart, 1);
          const disabled =
            outside || (minDay != null && day < minDay) || day > maxDay;
          const isEdge = sameDay(day, fromDay) || sameDay(day, toDay);
          const inRange =
            fromDay != null &&
            toDay != null &&
            day > fromDay &&
            day < toDay;
          const isToday = sameDay(day, todayDay);
          const label = dayFmt.format(new Date(day));
          return (
            <button
              key={day}
              type="button"
              className={`db-cal__day${outside ? " db-cal__day--outside" : ""}${
                isEdge ? " db-cal__day--edge" : ""
              }${inRange ? " db-cal__day--in-range" : ""}${
                isToday ? " db-cal__day--today" : ""
              }`}
              disabled={disabled}
              aria-pressed={isEdge}
              aria-current={isToday ? "date" : undefined}
              title={label}
              aria-label={label}
              onClick={() => onPickDay(day)}
            >
              {new Date(day).getDate()}
            </button>
          );
        })}
      </div>
    </div>
  );
}
