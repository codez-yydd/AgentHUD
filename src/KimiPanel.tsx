import type {
  AgentQuotaDelta,
  Currency,
  KimiSnapshot,
  PricingConfig,
  TrendBucket,
} from "./types";
import { AgentUsagePanel } from "./AgentUsagePanel";
import { SectionCard } from "./layout";
import { useI18n } from "./i18n";
import { AGENT_COLOR } from "./agentVisibility";
import { ProviderAccountsSection } from "./ProviderAccountsSection";

interface Props {
  snapshot: KimiSnapshot | null;
  loading: boolean;
  error: string | null;
  currency: Currency;
  /** USD→CNY 汇率：人民币花费 = 美元 × 汇率（价格只存美元） */
  fxRate: number;
  trendBucket: TrendBucket;
  /** 按模型折算花费用（与 Z.ai 页同款前端自算） */
  pricing: PricingConfig;
  agentQuotaDelta?: AgentQuotaDelta;
}

/**
 * Kimi 用量面板：通用 AgentUsagePanel 的 indigo 品牌皮肤，
 * 外层附加加油包余额小节（接口带 boosterWallet 时才有值）+ 底部
 * 「其他账号」区（凭证体系 ~/.agenthud/credentials/kimi.json：OAuth 网页登录
 * 保存的 refresh_token / 手动粘贴的 API Key，每条按其 region 调对应域名组的
 * usages 端点展示订阅额度）。该区由通用的 ProviderAccountsSection 渲染，
 * 与 OpenCode / MiniMax Code / DeepSeek Harness 三个面板共用同一形态——
 * 没装对应 CLI、把订阅用在别的 agent 里的用户也能手动录入 API Key 看额度。
 */
export function KimiPanel(props: Props) {
  const { t } = useI18n();
  const { snapshot } = props;
  const booster = snapshot?.rate_limits;
  return (
    <>
      {/* 加油包余额：与下方"额度"卡语义相邻，固定在滚动区上方常显 */}
      {booster?.booster_balance != null && (
        <div className="px-3 pt-2.5 shrink-0">
          <SectionCard title={t("stats.boosterBalance")}>
            <div className="flex items-baseline gap-3">
              <span className="num text-[15px] font-bold text-indigo-700">
                ¥{booster.booster_balance.toFixed(2)}
              </span>
              {booster.booster_monthly_used != null && (
                <span className="num text-[10px] text-slate-500">
                  {t("stats.boosterMonthlyUsed", {
                    amount: booster.booster_monthly_used.toFixed(2),
                  })}
                </span>
              )}
            </div>
          </SectionCard>
        </div>
      )}
      {/* 额度获取失败：统计仍正常展示，仅透出失败原因（与 booster 块同位互斥） */}
      {snapshot && !snapshot.rate_limits && snapshot.rate_limits_error && (
        <div className="px-3 pt-2.5 shrink-0">
          <p
            className="text-[9px] text-amber-700/90 leading-relaxed break-all"
            title={snapshot.rate_limits_error}
          >
            ⚠ {t("quota.quotaFail")}：{snapshot.rate_limits_error}
          </p>
        </div>
      )}
      <AgentUsagePanel
        {...props}
        theme={{
          rowBar: "bg-indigo-500/10",
          sortSelected: "bg-indigo-500/20 text-indigo-700",
          badge: "bg-indigo-500/15 text-indigo-700",
          accent: "indigo",
        }}
        empty={{
          name: "Kimi Code",
          icon: "🖥️",
          title: t("stats.kimiNotFound"),
          hint: t("stats.kimiNotFoundHint"),
        }}
        cacheRateMode="separate"
      />
      {/* 其他账号：本地 CLI 登录态不在此展示（上方 AgentUsagePanel 额度卡），
          避免双查询；有凭证时默认展开（凭证卡 + 每账号一张配额卡） */}
      <ProviderAccountsSection
        provider="kimi"
        titleKey="stats.kimiOtherAccounts"
        kind="token"
        guideKey="credentials.guide.kimi"
        brand="kimi"
        regionOptions={[
          { value: "cn", label: t("credentials.regionCn") },
          { value: "global", label: t("credentials.regionGlobal") },
        ]}
        accent={AGENT_COLOR.kimi}
        renderQuotaCards
      />
    </>
  );
}
