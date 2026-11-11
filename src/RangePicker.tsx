import { useState, type ReactNode } from "react";
import type { RangePreset } from "./types";
import { dateStr, rangeToMs } from "./format";
import { DatePicker } from "./DatePicker";
import { useI18n, type MessageKey } from "./i18n";

interface Props {
  preset: RangePreset;
  custom: { from: string; to: string };
  /** 可选：from 下限（如数据保留期），不传则不限 */
  min?: string;
  onChange: (preset: RangePreset, custom: { from: string; to: string }) => void;
  /** 同一行右侧的附加控件（悬浮面板放设备筛选小胶囊），不传则只渲染分段控件 */
  trailing?: ReactNode;
}

// 模式 A：常量表存词典键，渲染时查（label 跟随 UI 语言）
const PRESETS: { value: RangePreset; labelKey: MessageKey }[] = [
  { value: "today", labelKey: "range.today" },
  { value: "1d", labelKey: "range.24h" },
  { value: "7d", labelKey: "range.7d" },
  { value: "30d", labelKey: "range.30d" },
  { value: "custom", labelKey: "range.custom" },
];

/**
 * 时间范围分段控件（悬浮面板 / 主窗口共用观感语言）。
 *
 * 选中态用一枚滑动的「胶囊滑块」（p-seg__thumb）而不是给按钮本身刷底色：
 * 滑块是绝对定位的单个元素，宽度 = 1/N、transform 按索引步进，切换时有一段
 * 位移过渡，视觉上比逐个按钮换色更连贯；按钮只负责文字颜色与命中区。
 * `trailing` 让设备筛选胶囊与分段控件同行（面板只有 300px 宽，单独一行会
 * 白吃 30px 高度）。
 */
export function RangePicker({ preset, custom, min, onChange, trailing }: Props) {
  const [showCustom, setShowCustom] = useState(preset === "custom");
  const { t } = useI18n();
  const index = Math.max(
    0,
    PRESETS.findIndex((p) => p.value === preset)
  );

  return (
    <div className="panel-range flex flex-col gap-2">
      <div className="p-row">
        <div
          className="p-seg"
          role="group"
          aria-label={t("dashboard.overview.rangeLabel")}
        >
          <span
            className="p-seg__thumb"
            aria-hidden
            style={{
              width: `calc((100% - 4px) / ${PRESETS.length})`,
              transform: `translateX(${index * 100}%)`,
            }}
          />
          {PRESETS.map((p) => (
            <button
              key={p.value}
              type="button"
              aria-pressed={preset === p.value}
              onClick={() => {
                onChange(p.value, custom);
                setShowCustom(p.value === "custom");
              }}
              className="p-seg__item"
            >
              {t(p.labelKey)}
            </button>
          ))}
        </div>
        {trailing}
      </div>
      {showCustom && (
        <div className="flex items-center gap-1.5 text-[11px]">
          <DatePicker
            value={custom.from}
            min={min}
            max={custom.to}
            onChange={(v) => onChange("custom", { ...custom, from: v })}
          />
          <span className="text-slate-700/40">→</span>
          <DatePicker
            value={custom.to}
            min={custom.from}
            max={dateStr(Date.now())}
            onChange={(v) => onChange("custom", { ...custom, to: v })}
          />
        </div>
      )}
    </div>
  );
}

export function resolveRange(
  preset: RangePreset,
  custom: { from: string; to: string }
): [number, number] {
  return rangeToMs(preset, custom);
}
