import type { Currency, DshSnapshot, PricingConfig, TrendBucket } from "./types";
import { AgentUsagePanel, type AgentUsageSnapshot } from "./AgentUsagePanel";
import { useI18n } from "./i18n";

interface Props {
  snapshot: DshSnapshot | null;
  loading: boolean;
  error: string | null;
  currency: Currency;
  /** USD→CNY 汇率：人民币花费 = 美元 × 汇率（价格只存美元） */
  fxRate: number;
  trendBucket: TrendBucket;
  /** 按模型折算花费用（与 Z.ai 页同款前端自算） */
  pricing: PricingConfig;
}

/**
 * DSH 快照 → 通用面板快照：本地采集型**没有**订阅额度窗口，补一个
 * rate_limits: null（AgentUsagePanel 据此完全不渲染额度块；rate_limits_error
 * 为可选字段，不传即静默，不会出现授权失败提示）。stats/trend 与 z.ai 同构，
 * 其余字段结构化兼容，故只需补这一个字段。
 */
function toUsageSnapshot(
  snapshot: DshSnapshot | null
): AgentUsageSnapshot | null {
  return snapshot ? { ...snapshot, rate_limits: null } : null;
}

/** DeepSeek Harness 用量面板：通用 AgentUsagePanel 的 DeepSeek 品牌皮肤（sky）。
 *  纯本地采集（~/.dsh 会话数据），无凭证卡、无「其他账号」区。
 *  速度口径：后端置 speedQuality="generation"（真生成窗口）且 TTFT 有真值，
 *  面板不会出现 ≈ 前缀（是否加前缀完全由 AgentUsagePanel 依 speedQuality 决定，
 *  本组件不介入）；缓存率沿用 included 口径（input 含 cache_read，与 Codex 同）。 */
export function DshPanel(props: Props) {
  const { t } = useI18n();
  return (
    <AgentUsagePanel
      {...props}
      snapshot={toUsageSnapshot(props.snapshot)}
      theme={{
        rowBar: "bg-sky-500/10",
        sortSelected: "bg-sky-500/20 text-sky-700",
        badge: "bg-sky-500/15 text-sky-700",
        accent: "sky",
      }}
      empty={{
        name: "DeepSeek Harness",
        icon: "🐋",
        title: t("stats.dshNotFound"),
        hint: t("stats.dshNotFoundHint"),
      }}
      cacheRateMode="included"
    />
  );
}
