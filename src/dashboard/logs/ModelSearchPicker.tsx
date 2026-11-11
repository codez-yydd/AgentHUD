/**
 * 模型搜索选择器（「等价花费」的添加对比模型用）。
 *
 * 为什么不用共用的 DropdownMenu：参考价表是运行时合并表，几百个模型一条条翻
 * 根本找不到；这里给一个带输入框的弹层，**子串优先、子序列兜底**的模糊匹配
 * （"glm46" 能中 "glm-4.6"，"sonnet45" 能中 "claude-sonnet-4-5-20250929"）。
 *
 * 交互：点触发器展开 → 自动聚焦输入框 → ↑↓ 移动、Enter 添加、Esc 或点外部关闭；
 * 选中后立即回调并收起（行会出现在抽屉里，要再加就再点一次）。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { useI18n } from "../../i18n";

export interface ModelSearchOption {
  value: string;
  label: string;
  /** 右侧灰色附加信息（如输入价），可选 */
  hint?: string;
}

/** 结果条数上限：模糊搜索不该把几百条一次画出来 */
const MAX_RESULTS = 60;

/**
 * 模糊匹配打分：连续子串优先（越靠前分越高），命中不到时退回按序子序列；
 * 完全不匹配返回 null。空查询返回 0（= 不过滤）。
 */
function fuzzyScore(needle: string, haystack: string): number | null {
  if (!needle) return 0;
  const hay = haystack.toLowerCase();
  const direct = hay.indexOf(needle);
  if (direct >= 0) return 1000 - direct;
  let cursor = -1;
  let score = 0;
  let streak = 0;
  for (const char of needle) {
    const found = hay.indexOf(char, cursor + 1);
    if (found < 0) return null;
    if (found === cursor + 1) {
      streak += 1;
      score += 3 + streak;
    } else {
      streak = 0;
      score += 1;
    }
    cursor = found;
  }
  return score;
}

export function ModelSearchPicker({
  options,
  onPick,
  label,
}: {
  options: ModelSearchOption[];
  onPick: (value: string) => void;
  label: string;
}) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  // 展开时清空查询、高亮第一项并聚焦输入框
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActive(0);
    const timer = window.setTimeout(() => inputRef.current?.focus(), 0);
    return () => window.clearTimeout(timer);
  }, [open]);

  // 点外部收起（面板本身就是弹层，不能只靠 blur）
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    return () => document.removeEventListener("mousedown", onPointerDown);
  }, [open]);

  const results = useMemo(() => {
    // 空白去掉：允许「glm 4.6」这类带空格输入
    const needle = query.trim().toLowerCase().replace(/\s+/g, "");
    if (!needle) return options.slice(0, MAX_RESULTS);
    return options
      .map((option) => ({
        option,
        score: fuzzyScore(needle, `${option.label} ${option.value}`),
      }))
      .filter(
        (item): item is { option: ModelSearchOption; score: number } =>
          item.score != null
      )
      .sort((a, b) => b.score - a.score)
      .slice(0, MAX_RESULTS)
      .map((item) => item.option);
  }, [options, query]);

  const pick = (value: string) => {
    onPick(value);
    setOpen(false);
  };

  return (
    <div className="db-model-picker" ref={rootRef}>
      <button
        type="button"
        className="db-btn db-model-picker__trigger"
        aria-expanded={open}
        aria-haspopup="dialog"
        onClick={() => setOpen((value) => !value)}
      >
        {label}
      </button>
      {open && (
        <div className="db-model-picker__pop" role="dialog" aria-label={label}>
          <input
            ref={inputRef}
            className="db-model-picker__input"
            value={query}
            placeholder={t("dashboard.logs.detailEquivalentSearch")}
            onChange={(event) => {
              setQuery(event.target.value);
              setActive(0);
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                setOpen(false);
              } else if (event.key === "ArrowDown") {
                event.preventDefault();
                setActive((current) =>
                  Math.min(current + 1, Math.max(0, results.length - 1))
                );
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setActive((current) => Math.max(0, current - 1));
              } else if (event.key === "Enter") {
                event.preventDefault();
                const target = results[active];
                if (target) pick(target.value);
              }
            }}
          />
          <div className="db-model-picker__list">
            {results.length === 0 ? (
              <p className="db-model-picker__empty">
                {t("dashboard.logs.detailEquivalentNoMatch")}
              </p>
            ) : (
              results.map((option, index) => (
                <button
                  key={option.value}
                  type="button"
                  className={`db-model-picker__item${
                    index === active ? " db-model-picker__item--active" : ""
                  }`}
                  onMouseEnter={() => setActive(index)}
                  onClick={() => pick(option.value)}
                  title={option.value}
                >
                  <span className="db-model-picker__name">{option.label}</span>
                  {option.hint && (
                    <span className="db-model-picker__hint num">
                      {option.hint}
                    </span>
                  )}
                </button>
              ))
            )}
          </div>
        </div>
      )}
    </div>
  );
}
