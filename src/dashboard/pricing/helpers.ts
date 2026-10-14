/**
 * 价格设置页（主窗口大窗版）的纯工具与表格行模型：无 React。
 *
 * 金额格式化 / 汇率折算 / 草稿解析与窄窗 PricingPanel 同口径（逐字复制，
 * 窄窗原件不动），保证两版显示与落盘数值完全一致。
 */
import type { ModelInfo, ModelPrice, PricingConfig } from "../../types";
import { canonicalModelId, displayModelName, lookupPrice } from "../../modelName";
import type { MessageKey } from "../../i18n";

/** 可编辑的价格字段（顺序即表格列顺序；ModelPrice 实际只有这三项） */
export const PRICE_FIELDS: { key: keyof ModelPrice; labelKey: MessageKey }[] = [
  { key: "input", labelKey: "dashboard.pricing.colInput" },
  { key: "output", labelKey: "dashboard.pricing.colOutput" },
  { key: "cache_read", labelKey: "dashboard.pricing.colCacheRead" },
];

/** 价格数字显示格式化：6 位有效数字，去掉浮点乘法尾巴（0.39999999999999 → 0.4） */
export function fmtPrice(n: number): string {
  return String(Number(n.toPrecision(6)));
}

/** 价格三元组（输入/输出/缓存读）显示 */
export function fmtTriplet(p: ModelPrice): string {
  return `${fmtPrice(p.input)}/${fmtPrice(p.output)}/${fmtPrice(p.cache_read)}`;
}

/** 单个价格数字按汇率折算为人民币价（仅展示用，不落盘） */
export function scaleCnyValue(n: number, rate: number): number {
  return Number((n * rate).toPrecision(6));
}

/** 按汇率把美元价三元组折算为人民币价（仅展示用，不落盘） */
export function scaleCny(p: ModelPrice, rate: number): ModelPrice {
  return {
    input: scaleCnyValue(p.input, rate),
    output: scaleCnyValue(p.output, rate),
    cache_read: scaleCnyValue(p.cache_read, rate),
  };
}

/** 无价格模型的占位（复用同一引用，保证 memo 的行组件不会因 fallback 新对象而失效） */
export const ZERO_PRICE: ModelPrice = { input: 0, output: 0, cache_read: 0 };

/** models.dev 数据时间格式化（本地时区，YYYY-MM-DD HH:mm） */
export function fmtStamp(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}`;
}

/** 草稿字符串解析回数字。非法/空 → 0（窄窗同款）。 */
export function parseDraft(val: string): number {
  const trimmed = val.trim();
  if (trimmed === "" || trimmed === "." || trimmed === "-") return 0;
  const n = parseFloat(trimmed);
  return isNaN(n) ? 0 : n;
}

/** 行的价格来源标识：已配价（价格表命中）/ 未配价（只在模型清单里）/ 手动（仅价格表） */
export type PriceSource = "configured" | "unpriced" | "manual";

export interface PriceRow {
  /** 原始模型 id（唯一身份，落盘用） */
  id: string;
  /** 展示名（剥 provider 前缀，仅标签用） */
  displayName: string;
  /** 当前价格（无配置为 ZERO_PRICE 占位） */
  price: ModelPrice;
  /** 价格表中是否有该模型条目 */
  configured: boolean;
  source: PriceSource;
}

/**
 * 表格行：数据库里的模型 + 价格表里手动加的模型合并（窄窗 modelIds 同款口径）：
 *  - 隐藏名单（已删除模型）命中归一 key 的不展示；
 *  - 按归一名去重：仅大小写/隐藏字符不同的写法只保留一行，首个原始写法作展示 id；
 *  - 升序排序，行数较多时配合搜索过滤使用。
 */
export function buildPriceRows(
  pricing: PricingConfig,
  models: ModelInfo[]
): PriceRow[] {
  const hiddenSet = new Set(
    (pricing.hidden ?? []).map((h) => canonicalModelId(h))
  );
  const dbIds = new Set(models.map((model) => model.model_id));
  const seen = new Set<string>();
  const ids: string[] = [];
  for (const id of [
    ...models.map((model) => model.model_id),
    ...Object.keys(pricing.usd),
  ]) {
    const key = canonicalModelId(id);
    if (hiddenSet.has(key)) continue;
    if (seen.has(key)) continue;
    seen.add(key);
    ids.push(id);
  }
  ids.sort();
  return ids.map((id) => {
    const price = lookupPrice(pricing.usd, id);
    return {
      id,
      displayName: displayModelName(id),
      price: price ?? ZERO_PRICE,
      configured: price != null,
      source: !dbIds.has(id) ? "manual" : price != null ? "configured" : "unpriced",
    };
  });
}
