/**
 * 模型价格宽表（大窗版）：全宽表格 + 吸顶表头 + 行内编辑。
 *
 * 编辑沿用窄窗 PricingPanel 的「草稿 + 失焦提交」模式（draft 由页面持有，
 * 键入只重渲染对应的一行——行组件用 memo 固定）：输入框恒定编辑美元价，
 * 人民币只在币种为 ¥ 时作为折算提示展示（与窄窗 diff 面板的「≈ ¥」同口径）。
 */
import { memo } from "react";
import type { Currency, ModelPrice } from "../../types";
import { useI18n, type MessageKey } from "../../i18n";
import {
  PRICE_FIELDS,
  fmtPrice,
  parseDraft,
  scaleCnyValue,
  type PriceRow,
  type PriceSource,
} from "./helpers";

/** 来源标识 → 词典键 */
function sourceLabelKey(source: PriceSource): MessageKey {
  return source === "manual"
    ? "dashboard.pricing.sourceManual"
    : source === "configured"
      ? "dashboard.pricing.sourceConfigured"
      : "dashboard.pricing.sourceUnpriced";
}

export function PriceTable({
  rows,
  total,
  currency,
  fxRate,
  draft,
  onDraftChange,
  onCommit,
  onRemove,
}: {
  /** 过滤后要展示的行 */
  rows: PriceRow[];
  /** 过滤前的总行数（区分「一个模型都没有」与「搜索无结果」两种空态） */
  total: number;
  currency: Currency;
  fxRate: number;
  draft: Record<string, string>;
  onDraftChange: (id: string, key: keyof ModelPrice, value: string) => void;
  onCommit: (id: string, key: keyof ModelPrice) => void;
  onRemove: (id: string) => void;
}) {
  const { t } = useI18n();

  if (rows.length === 0) {
    return (
      <div className="db-empty">
        {total === 0
          ? t("dashboard.pricing.noModels")
          : t("dashboard.pricing.filterEmpty")}
      </div>
    );
  }

  return (
    <div className="db-price-wrap">
      <table className="db-table db-price-table">
        <thead>
          <tr>
            <th>{t("dashboard.pricing.colModel")}</th>
            {PRICE_FIELDS.map((field) => (
              <th key={field.key} className="db-table__num">
                {t(field.labelKey)}
              </th>
            ))}
            <th>{t("dashboard.pricing.colSource")}</th>
            <th className="db-price-table__ops">
              {t("dashboard.pricing.colActions")}
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <PriceRowView
              key={row.id}
              row={row}
              currency={currency}
              fxRate={fxRate}
              dIn={draft[`${row.id}|input`]}
              dOut={draft[`${row.id}|output`]}
              dCache={draft[`${row.id}|cache_read`]}
              onDraftChange={onDraftChange}
              onCommit={onCommit}
              onRemove={onRemove}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** 单行（memo：草稿按三个原始类型 props 传递，键入时只重渲染这一行） */
const PriceRowView = memo(function PriceRowView({
  row,
  currency,
  fxRate,
  dIn,
  dOut,
  dCache,
  onDraftChange,
  onCommit,
  onRemove,
}: {
  row: PriceRow;
  currency: Currency;
  fxRate: number;
  dIn: string | undefined;
  dOut: string | undefined;
  dCache: string | undefined;
  onDraftChange: (id: string, key: keyof ModelPrice, value: string) => void;
  onCommit: (id: string, key: keyof ModelPrice) => void;
  onRemove: (id: string) => void;
}) {
  const { t } = useI18n();
  const drafts: Record<keyof ModelPrice, string | undefined> = {
    input: dIn,
    output: dOut,
    cache_read: dCache,
  };

  return (
    <tr>
      <td>
        <div className="db-price-name">
          <span className="db-price-name__display">{row.displayName}</span>
          {row.displayName !== row.id && (
            <span className="db-price-name__id num">{row.id}</span>
          )}
        </div>
      </td>

      {PRICE_FIELDS.map((field) => {
        const draftVal = drafts[field.key];
        const stored = row.price[field.key];
        const shown =
          draftVal !== undefined ? draftVal : stored ? String(stored) : "";
        const value = draftVal !== undefined ? parseDraft(draftVal) : stored;
        const fieldLabel = t(field.labelKey);
        return (
          <td key={field.key} className="db-price-cell">
            <div className="db-price-input">
              <span className="db-price-input__symbol">$</span>
              <input
                type="text"
                inputMode="decimal"
                value={shown}
                placeholder="0"
                aria-label={`${row.displayName} ${fieldLabel}`}
                onChange={(e) => {
                  // 只允许数字和小数点（与窄窗一致）
                  const next = e.target.value.replace(/[^\d.]/g, "");
                  onDraftChange(row.id, field.key, next);
                }}
                onBlur={() => onCommit(row.id, field.key)}
                className="num db-price-input__field"
              />
            </div>
            {currency === "cny" && value > 0 && (
              <span className="db-price-cell__cny num">
                ≈ ¥ {fmtPrice(scaleCnyValue(value, fxRate))}
              </span>
            )}
          </td>
        );
      })}

      <td>
        <span className={`db-tag db-tag--${row.source}`}>
          {t(sourceLabelKey(row.source))}
        </span>
      </td>

      <td className="db-price-table__ops">
        <button
          type="button"
          title={t("dashboard.pricing.removeModel")}
          aria-label={t("dashboard.pricing.removeModel")}
          onClick={() => onRemove(row.id)}
          className="db-price-remove"
        >
          ×
        </button>
      </td>
    </tr>
  );
});
