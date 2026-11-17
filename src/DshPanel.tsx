import type { Currency, DshSnapshot, PricingConfig, TrendBucket } from "./types";
import { AgentUsagePanel, type AgentUsageSnapshot } from "./AgentUsagePanel";
import { BrandIcon } from "./BrandIcon";
import { ProviderQuotaErrorBanner, QuotaEntryCard } from "./QuotaEntryCard";
import { useDataCache } from "./DataCache";
import { AGENT_COLOR } from "./agentVisibility";
import { useNow } from "./nowClock";
import { ProviderAccountsSection } from "./ProviderAccountsSection";
import { useResetDisplay } from "./resetDisplay";
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
 *
 *  两条数据链路同页（与 MiniMax Code 合并口径同款思路）：
 *  - **本地用量**：解析 ~/.dsh 会话事件日志（纯本地采集，无凭证）；
 *  - **平台余额**：顶部「DeepSeek 平台余额」额度板块，展示 DeepSeek 开放平台
 *    API Key 余额或 DSH 本地登录态回退条目（凭证文件仍是
 *    ~/.agenthud/credentials/deepseek.json，provider id "deepseek"），调
 *    /user/balance 展示余额与赠送/充值拆分。
 *
 *  版式与 OpencodePanel / MinimaxPanel 同款：额度卡 shrink-0 常驻滚动区上方
 *  （不随用量滚走），面板只有一个滚动条由 AgentUsagePanel 提供。底部
 *  ProviderAccountsSection 只承担凭证管理（renderQuotaCards=false），不重复
 *  渲染额度卡。
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
  const resetDisplay = useResetDisplay();
  const { providerQuota, providerQuotaErrors, refreshProviderQuota } =
    useDataCache();
  const cacheEntry = providerQuota["deepseek"];
  const quotaEntries = cacheEntry?.entries ?? [];
  const refreshing = cacheEntry?.refreshing ?? false;
  // 整体查询失败（command 级 Err，如凭证文件损坏）：旧值照常展示，但必须
  // 叠加错误提示（见 ProviderQuotaErrorBanner）
  const quotaError = providerQuotaErrors["deepseek"];
  // 是否已完成首轮查询（ts>0）：区分「还没查过」与「查过确认无数据」
  //（照 GenericQuotaPanel；本地无数据库、也没配 API Key 时首轮返回空数组，
  // 不能一片空白——空态另有底部凭证区的引导）
  const firstRoundDone = (cacheEntry?.ts ?? 0) > 0;

  // 倒计时显示用时钟：收敛为共享 context（内存压降专项 6，见 nowClock.tsx）。
  // 原先「有重置数据才启定时器」的门槛随共享时钟取消——全局定时器只剩
  // 一个，无重置数据的组件仅多几次幂等重渲染。
  const now = useNow();

  // 查询中 / 首轮未回 / 已有条目 / 整体查询失败才渲染额度板块：首轮已回且
  // 确认无数据时整块不渲染（不留一块空标题），此时下方用量面板的空态已
  // 说明「没有本地数据」；失败态必须保留板块，否则错误对用户不可见。
  const showQuota =
    quotaEntries.length > 0 || refreshing || !firstRoundDone || !!quotaError;

  return (
    <>
      {showQuota && (
        <div className="shrink-0 px-3 pt-2.5">
          {/* 板块头：品牌 + 名称 + 额度手动刷新。全局 ↻ 走 DataCache.refresh()
              只刷范围用量、不刷 providerQuota，故保留额度刷新入口（否则余额
              只能等 120s 轮询） */}
          <div className="flex items-center justify-between gap-2 mb-1.5">
            <div className="flex items-center gap-1.5 min-w-0">
              <BrandIcon brand="deepseek" className="h-3.5 w-3.5 shrink-0" />
              <span className="section-title truncate">
                {t("stats.dshPlatformBalance")}
              </span>
            </div>
            <button
              onClick={() => void refreshProviderQuota("deepseek")}
              disabled={refreshing}
              className={`toolbar-btn shrink-0 ${refreshing ? "opacity-40" : ""}`}
              title={t("common.refresh")}
            >
              ↻
            </button>
          </div>

          {/* 整体查询失败提示（command 级 Err，如凭证文件损坏）：无错误时渲染
              null。无旧值时替代下方「查询中」占位，有旧值时叠加在配额卡上方
              （保旧不丢数据，但用户必须知道数据已不再新鲜）。凭证文件损坏时
              卡内嵌「重置凭证文件」自愈入口；mb-1.5 与下方卡片对齐间距 */}
          <ProviderQuotaErrorBanner provider="deepseek" className="mb-1.5" />

          {quotaEntries.length === 0 ? (
            // 空态一档：查询中或首轮未回（照 GenericQuotaPanel 的档位与文案）。
            // 整体查询失败且未在刷新（错误静置态）时改由上方错误卡表达，
            // 不能再显示「查询中」误导用户以为查询仍在进行。
            quotaError && !refreshing ? null : (
              <div className="card-base rounded-2xl px-3 py-3">
                <p className="text-[10px] text-slate-500 leading-relaxed">
                  {t("credentials.quotaRefreshing")}
                </p>
              </div>
            )
          ) : (
            // 每条凭证一张配额卡（与 GenericQuotaPanel / 顶部额度块共用渲染，
            // 品牌色取 AGENT_COLOR.deepseek）
            <div className="space-y-1.5">
              {quotaEntries.map((entry) => (
                <QuotaEntryCard
                  key={entry.credentialId}
                  entry={entry}
                  accent={AGENT_COLOR.deepseek}
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

      {/* 手动 API Key 录入区：DeepSeek 平台余额的额度卡由上方额度块统一渲染
          （含 DSH 本地登录回退条目与手动凭证），本区只承担凭证管理，故不重复
          渲染额度卡——与 opencodego / minimax 面板底部用法一致。 */}
      <ProviderAccountsSection
        provider="deepseek"
        titleKey="stats.dshPlatformBalance"
        kind="apiKey"
        guideKey="credentials.guide.deepseek"
        brand="deepseek"
        accent={AGENT_COLOR.deepseek}
        renderQuotaCards={false}
      />
    </>
  );
}
