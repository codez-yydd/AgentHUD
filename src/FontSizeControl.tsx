/**
 * 基准字号控件：小 / 标准 / 大 / 特大预设 + 自定义 px 输入（增减、实时预览、
 * 恢复默认）。面板设置页与主窗口设置页共用同一交互，样式只用两窗都存在的
 * Tailwind 工具类（颜色走 slate / surface 语义变量，深浅主题自动翻转）。
 *
 * 输入纪律：输入过程允许暂时清空；实时预览只在解析为合法数字时生效；
 * 回车 / 失焦才确认。非数字或越界给出行内提示、不保存，并回退已保存值。
 */
import { useEffect, useRef, useState } from "react";
import { useI18n } from "./i18n";
import { PillButton, PillGroup } from "./layout";
import {
  FONT_MAX_PX,
  FONT_MIN_PX,
  FONT_PRESETS,
  formatFontPx,
  isValidFontPx,
  stepFontPx,
  type FontTarget,
} from "./fontSettings";

export function FontSizeControl({
  target,
  value,
  onChange,
  onPreview,
  onReset,
  disabled = false,
}: {
  target: FontTarget;
  /** 当前已保存的基准字号 */
  value: number;
  /** 确认（回车 / 失焦 / 预设 / 增减）后保存并应用 */
  onChange: (px: number) => void;
  /** 输入过程中的实时预览（不落盘） */
  onPreview: (px: number) => void;
  /** 恢复默认（13px） */
  onReset: () => void;
  /** 跟随其他窗口时禁用本窗输入 */
  disabled?: boolean;
}) {
  const { t } = useI18n();
  const [draft, setDraft] = useState(() => formatFontPx(value));
  const [error, setError] = useState<string | null>(null);
  const focusedRef = useRef(false);

  // 外部值变化（切换跟随 / 其他窗口广播）时同步草稿；正在输入时不打断
  useEffect(() => {
    if (!focusedRef.current) {
      setDraft(formatFontPx(value));
      setError(null);
    }
  }, [value]);

  const parseDraft = (): number => parseFloat(draft);
  const commitDraft = () => {
    const n = parseDraft();
    if (!isValidFontPx(n)) {
      setError(t("font.invalid", { min: FONT_MIN_PX, max: FONT_MAX_PX }));
      setDraft(formatFontPx(value));
      onPreview(value);
      return;
    }
    setError(null);
    onChange(n);
  };

  const activePreset = FONT_PRESETS.find(
    (preset) => Math.abs(preset.px - value) < 0.05
  );

  return (
    <div className="font-size-control flex flex-col gap-2">
      <PillGroup>
        {FONT_PRESETS.map((preset) => (
          <PillButton
            key={preset.id}
            active={activePreset?.id === preset.id}
            disabled={disabled}
            onClick={() => {
              setDraft(formatFontPx(preset.px));
              setError(null);
              onChange(preset.px);
            }}
          >
            {t(preset.labelKey)}
          </PillButton>
        ))}
        <PillButton
          active={!activePreset}
          disabled={disabled}
          onClick={() => {
            const input = document.getElementById(
              `font-size-input-${target}`
            ) as HTMLInputElement | null;
            input?.focus();
            input?.select();
          }}
        >
          {t("font.custom")}
        </PillButton>
      </PillGroup>

      <div className="flex items-center gap-1.5">
        <button
          type="button"
          className="font-size-control__step"
          title={t("font.decrease")}
          aria-label={t("font.decrease")}
          disabled={disabled}
          onClick={() => {
            const next = stepFontPx(value, -1);
            setDraft(formatFontPx(next));
            setError(null);
            onChange(next);
          }}
        >
          −
        </button>
        <label className="font-size-control__field flex min-w-0 flex-1 items-center gap-1 rounded-lg border border-slate-900/10 bg-surface/40 px-2 py-1">
          <span className="sr-only">{t("font.basePx")}</span>
          <input
            id={`font-size-input-${target}`}
            type="number"
            inputMode="decimal"
            className="num min-w-0 flex-1 border-0 bg-transparent p-0 text-[11px] text-slate-900/90 outline-none"
            value={draft}
            min={FONT_MIN_PX}
            max={FONT_MAX_PX}
            step={0.5}
            disabled={disabled}
            aria-invalid={error != null}
            aria-describedby={error ? `font-size-error-${target}` : undefined}
            onFocus={() => {
              focusedRef.current = true;
            }}
            onChange={(e) => {
              const text = e.target.value;
              setDraft(text);
              const n = parseFloat(text);
              if (isValidFontPx(n)) {
                setError(null);
                onPreview(n);
              } else {
                setError(null);
              }
            }}
            onBlur={() => {
              focusedRef.current = false;
              commitDraft();
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                commitDraft();
                (e.currentTarget as HTMLInputElement).blur();
              } else if (e.key === "Escape") {
                setDraft(formatFontPx(value));
                setError(null);
                onPreview(value);
                (e.currentTarget as HTMLInputElement).blur();
              }
            }}
          />
          <span className="shrink-0 text-[9px] text-slate-500">
            {t("font.pxUnit")}
          </span>
        </label>
        <button
          type="button"
          className="font-size-control__step"
          title={t("font.increase")}
          aria-label={t("font.increase")}
          disabled={disabled}
          onClick={() => {
            const next = stepFontPx(value, 1);
            setDraft(formatFontPx(next));
            setError(null);
            onChange(next);
          }}
        >
          +
        </button>
        <button
          type="button"
          className="shrink-0 px-1 text-[10px] font-medium text-sky-600 hover:underline disabled:opacity-40"
          title={t("font.resetDefault")}
          disabled={disabled}
          onClick={() => {
            setDraft("13");
            setError(null);
            onReset();
          }}
        >
          {t("font.resetDefault")}
        </button>
      </div>

      {error && (
        <p
          id={`font-size-error-${target}`}
          className="text-[9px] leading-relaxed text-rose-600"
          role="alert"
        >
          {error}
        </p>
      )}
      <p className="text-[9px] leading-relaxed text-slate-500">
        {t("font.range", { min: FONT_MIN_PX, max: FONT_MAX_PX })} ·{" "}
        {t("font.logicalPxHint")}
      </p>
    </div>
  );
}
