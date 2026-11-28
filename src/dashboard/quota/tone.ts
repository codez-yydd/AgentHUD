import { riskOf } from "./model";

/**
 * 剩余 % → 三档语义色（额度驾驶舱 / 重置序列共用）。
 *
 * 颜色不再走 src/widgets 的连续色阶（亮色卡片上是土黄 / 暗红 / 橄榄绿，
 * 与整页中性语言不符），改按 model.riskOf 的风险档位返回 dashboard.css
 * 的语义令牌（CSS 变量引用，亮暗主题由变量各自切换）：
 * - 正常（>50%）= --db-quota-ok（品牌色：条与弧的中性档）；
 * - 偏低（20–50%）= --db-quota-warn（琥珀）；
 * - 告急（≤20%）= --db-quota-danger（错误红）。
 * 风险分档阈值不在此处（riskOf 的 20% / 50% 不动）。
 */
export function quotaTone(remainingPct: number): string {
  const risk = riskOf(remainingPct);
  return risk === "critical"
    ? "var(--db-quota-danger)"
    : risk === "warning"
      ? "var(--db-quota-warn)"
      : "var(--db-quota-ok)";
}
