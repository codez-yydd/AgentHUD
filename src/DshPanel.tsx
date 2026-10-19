import type { Currency, DshSnapshot, PricingConfig, TrendBucket } from "./types";
import { AgentUsagePanel, type AgentUsageSnapshot } from "./AgentUsagePanel";
import { useI18n } from "./i18n";
import { AGENT_COLOR } from "./agentVisibility";
import { ProviderAccountsSection } from "./ProviderAccountsSection";

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
 *
 *  两条数据链路同页（与 MiniMax Code 合并口径同款思路）：
 *  - **本地用量**：解析 ~/.dsh 会话事件日志（纯本地采集，无凭证）；
 *  - **平台余额**：底部「DeepSeek 平台余额」区，手动录入 DeepSeek 开放平台
 *    API Key（凭证文件仍是 ~/.agenthud/credentials/deepseek.json，provider id
 *    "deepseek"），调 /user/balance 展示余额与赠送/充值拆分。
 *
 *  为什么 DSH 自己不产额度条目：DSH 的本地登录态（~/.dsh/.credentials.yaml 的
 *  grant token）不是开放平台 API Key，实测打开放平台接口返回 401（见 dsh.rs
 *  模块头），DeepSeek 本身也按量计费、没有 Coding Plan 式订阅窗口——余额查询
 *  走既有 deepseek 凭证通道，不在这里臆造窗口。用户把 DeepSeek 额度用在别的
 *  agent（如 Claude Code 接 DeepSeek 端点）时，这条通道就是唯一能看到余额的入口。
 *
 *  速度口径：后端置 speedQuality="generation"（真生成窗口）且 TTFT 有真值，
 *  面板不会出现 ≈ 前缀（是否加前缀完全由 AgentUsagePanel 依 speedQuality 决定，
 *  本组件不介入）；缓存率沿用 included 口径（input 含 cache_read，与 Codex 同）。 */
export function DshPanel(props: Props) {
  const { t } = useI18n();
  return (
    <>
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
      {/* DeepSeek 平台余额（手动 API Key）：DSH 面板内没有别处渲染 providerQuota
          条目，故本区自己渲染余额卡；有 Key 时默认展开 */}
      <ProviderAccountsSection
        provider="deepseek"
        titleKey="stats.dshPlatformBalance"
        kind="apiKey"
        guideKey="credentials.guide.deepseek"
        brand="deepseek"
        accent={AGENT_COLOR.deepseek}
        renderQuotaCards
      />
    </>
  );
}
