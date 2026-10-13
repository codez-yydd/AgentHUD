import { useEffect, useState } from "react";
import type {
  Currency,
  MinimaxcodeSnapshot,
  PricingConfig,
  TrendBucket,
} from "./types";
import { AgentUsagePanel, type AgentUsageSnapshot } from "./AgentUsagePanel";
import { BrandIcon } from "./BrandIcon";
import { QuotaEntryCard } from "./QuotaEntryCard";
import { useDataCache } from "./DataCache";
import { AGENT_COLOR } from "./agentVisibility";
import { ProviderAccountsSection } from "./ProviderAccountsSection";
import { useResetDisplay } from "./resetDisplay";
import { useI18n } from "./i18n";

interface Props {
  snapshot: MinimaxcodeSnapshot | null;
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
 * MiniMax Code 快照 → 通用面板快照：本地采集型**没有**订阅额度窗口，补一个
 * rate_limits: null（AgentUsagePanel 据此完全不渲染额度块；rate_limits_error
 * 为可选字段，不传即静默，不会出现授权失败提示）。
 * 套餐额度不会因此丢失：它是并列的另一条来源，由上方 providerQuota 的额度卡
 * 承担，与面板内的额度块语义不重叠。
 */
function toUsageSnapshot(
  snapshot: MinimaxcodeSnapshot | null
): AgentUsageSnapshot | null {
  return snapshot ? { ...snapshot, rate_limits: null } : null;
}

/**
 * MiniMax Code 用量面板：顶部套餐额度卡 + 下方通用 AgentUsagePanel 的
 * MiniMax 品牌皮肤（emerald）。
 *
 * 为什么两条数据链路合并在同一个 tab（用户已定）：本机 MiniMax 有两条来源
 * ——① 已存的 sk-cp- Key 查官方 Coding Plan 订阅额度（5h + 周窗口），
 * ② 本地 ~/.minimax 的 MiniMax Code 逐次调用用量；同一份订阅同时被两者
 * 计量，拆成两个 tab 会让用户看到两个「MiniMax」。故 agentId 仍是既有的
 * minimax（凭证、可见性、额度卡都不动），只是页面同时承载两条链路：
 * 用量走 get_minimaxcode_usage（本地库），额度走 get_provider_quota("minimax")
 * （注意 provider 是 **UI 侧 agent id**，minimaxcode 才是后端来源名），
 * 两条链路在 DataCache 里各自独立；面板底部另有手动 API Key 录入区
 * （ProviderAccountsSection，provider id 仍是 minimax），供没装 MiniMax Code、
 * 把订阅用在别的 agent 的用户直接凭 sk-cp- Key 读套餐额度。
 *
 * 版式与 OpencodePanel 同款：额度卡 shrink-0 常驻滚动区上方（不随用量滚走，
 * 面板只有一个滚动条由 AgentUsagePanel 提供——它自带滚动容器与内边距，
 * 嵌进外层滚动流会造成双重滚动 + 双重内边距错位）。
 *
 * 速度口径：后端置 speedQuality="request_average"（只有整次请求耗时、无
 * TTFT），面板会自动加 ≈ 前缀（本组件不介入）；缓存率沿用 included 口径
 * ——本库归一化时把 cache_read 并进 input_tokens，与 Codex 同。
 */
export function MinimaxPanel(props: Props) {
  const { t } = useI18n();
  const resetDisplay = useResetDisplay();
  const { providerQuota, refreshProviderQuota } = useDataCache();
  const cacheEntry = providerQuota["minimax"];
  const quotaEntries = cacheEntry?.entries ?? [];
  const refreshing = cacheEntry?.refreshing ?? false;
  // 是否已完成首轮查询（ts>0）：区分「还没查过」与「查过确认无数据」
  //（照 GenericQuotaPanel；本地无 MiniMax Code 数据、也没配 Coding Plan Key 时
  // 首轮返回空数组，不能一片空白——空态另有底部凭证区的引导）
  const firstRoundDone = (cacheEntry?.ts ?? 0) > 0;

  // 倒计时显示用时钟（有窗口带重置时间才有意义，空数据不启定时器）
  const hasResets = quotaEntries.some((e) =>
    e.windows.some((w) => w.resetsAt != null)
  );
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!hasResets) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [hasResets]);

  // 查询中 / 首轮未回 / 已有条目才渲染额度板块：首轮已回且确认无数据时整块
  // 不渲染（不留一块空标题）——没存过 Key 的机器本来就没有套餐额度可展示，
  // 此时下方用量面板已经说明了 MiniMax Code 的本地用量。
  const showQuota = quotaEntries.length > 0 || refreshing || !firstRoundDone;

  return (
    <>
      {showQuota && (
        <div className="shrink-0 px-3 pt-2.5">
          {/* 板块头：品牌 + 名称 + 额度手动刷新。全局 ↻ 走 DataCache.refresh()
              只刷范围用量、不刷 providerQuota，故保留 GenericQuotaPanel 原有的
              额度刷新入口（否则套餐额度只能等 120s 轮询） */}
          <div className="flex items-center justify-between gap-2 mb-1.5">
            <div className="flex items-center gap-1.5 min-w-0">
              <BrandIcon brand="minimax" className="h-3.5 w-3.5 shrink-0" />
              <span className="section-title truncate">
                {t("stats.minimaxQuota")}
              </span>
            </div>
            <button
              onClick={() => void refreshProviderQuota("minimax")}
              disabled={refreshing}
              className={`toolbar-btn shrink-0 ${refreshing ? "opacity-40" : ""}`}
              title={t("common.refresh")}
            >
              ↻
            </button>
          </div>

          {quotaEntries.length === 0 ? (
            // 空态一档：查询中或首轮未回（照 GenericQuotaPanel 的档位与文案；
            // 无本地数据也无 Key 时同样不能一片空白，底部凭证区给出录入引导）
            <div className="card-base rounded-2xl px-3 py-3">
              <p className="text-[10px] text-slate-500 leading-relaxed">
                {t("credentials.quotaRefreshing")}
              </p>
            </div>
          ) : (
            // 每条凭证一张配额卡（与 GenericQuotaPanel / KimiOtherAccounts
            // 共用渲染，品牌色取 AGENT_COLOR.minimax）
            <div className="space-y-1.5">
              {quotaEntries.map((entry) => (
                <QuotaEntryCard
                  key={entry.credentialId}
                  entry={entry}
                  accent={AGENT_COLOR.minimax}
                  resetDisplay={resetDisplay}
                  now={now}
                />
              ))}
            </div>
          )}
        </div>
      )}

      <AgentUsagePanel
        {...props}
        snapshot={toUsageSnapshot(props.snapshot)}
        theme={{
          rowBar: "bg-emerald-500/10",
          sortSelected: "bg-emerald-500/20 text-emerald-700",
          badge: "bg-emerald-500/15 text-emerald-700",
          accent: "emerald",
        }}
        empty={{
          name: "MiniMax Code",
          icon: "🟢",
          title: t("stats.minimaxcodeNotFound"),
          hint: t("stats.minimaxcodeNotFoundHint"),
        }}
        cacheRateMode="included"
      />

      {/* 手动 API Key 录入区：Coding Plan 的 sk-cp- Key（区域需与 Key 站点一致）。
       *  没装 MiniMax Code、或把订阅用在别的 agent 时，这里是唯一的额度来源；
       *  凭证条目产生的额度卡由上方额度块渲染（那里混排本地登录态与手动凭证），
       *  本区只承担凭证管理，故不重复渲染额度卡。空态提示用 MiniMax 自己的
       *  双源文案（默认那份是 OpenCode 口径，不能照搬）。 */}
      <ProviderAccountsSection
        provider="minimax"
        titleKey="stats.minimaxOtherAccounts"
        kind="apiKey"
        guideKey="credentials.guide.minimax"
        brand="minimax"
        regionOptions={[
          { value: "cn", label: t("credentials.regionCn") },
          { value: "global", label: t("credentials.regionGlobal") },
        ]}
        optional
        dualHintKey="credentials.localEmptyHintDualMinimax"
        accent={AGENT_COLOR.minimax}
      />
    </>
  );
}
