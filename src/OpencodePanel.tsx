import { useEffect, useState } from "react";
import type {
  Currency,
  OpencodeSnapshot,
  PricingConfig,
  TrendBucket,
} from "./types";
import { AgentUsagePanel, type AgentUsageSnapshot } from "./AgentUsagePanel";
import { BrandIcon } from "./BrandIcon";
import { QuotaEntryCard } from "./QuotaEntryCard";
import { useDataCache } from "./DataCache";
import { AGENT_COLOR } from "./agentVisibility";
import { useResetDisplay } from "./resetDisplay";
import { useI18n } from "./i18n";

interface Props {
  snapshot: OpencodeSnapshot | null;
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
 * OpenCode 快照 → 通用面板快照：本地采集型**没有**订阅额度窗口，补一个
 * rate_limits: null（AgentUsagePanel 据此完全不渲染额度块；rate_limits_error
 * 为可选字段，不传即静默，不会出现授权失败提示）。
 * Go 套餐额度不会因此丢失：它是并列的另一条来源，由下方 providerQuota 的
 * 额度卡承担，与面板内的额度块语义不重叠。
 */
function toUsageSnapshot(
  snapshot: OpencodeSnapshot | null
): AgentUsageSnapshot | null {
  return snapshot ? { ...snapshot, rate_limits: null } : null;
}

/**
 * OpenCode 用量面板：顶部 Go 套餐额度卡 + 下方通用 AgentUsagePanel 的
 * OpenCode 品牌皮肤（orange）。
 *
 * 为什么额度卡不进 GenericQuotaPanel：OpenCode 与 Kimi Code / ZCode 同类
 * （自带 CLI 与订阅的 agent），标签页应以「本地用量统计」为主、套餐额度为
 * 附属板块；agentId 仍是既有的 opencodego，后端命令/模块名里的 "opencode"
 * 只是命名。用量数据走 get_opencode_usage（本地 opencode.db 导入库），
 * 额度走 get_provider_quota("opencodego")（本地库估算 / 官方 API 两路），
 * 两条链路在 DataCache 里各自独立。
 *
 * 版式与 KimiPanel 同款：额度卡 shrink-0 常驻滚动区上方（不随用量滚走，
 * 面板只有一个滚动条由 AgentUsagePanel 提供——它自带滚动容器与内边距，
 * 嵌进外层滚动流会造成双重滚动 + 双重内边距错位）。
 *
 * 速度口径：后端置 speedQuality="request_average"（逐条 assistant 消息的
 * 平均，非真生成窗口），面板会自动加 ≈ 前缀（本组件不介入）；缓存率沿用
 * included 口径——本库归一化时把 cache_read 并进 input_tokens，与 Codex 同。
 */
export function OpencodePanel(props: Props) {
  const { t } = useI18n();
  const resetDisplay = useResetDisplay();
  const { providerQuota, refreshProviderQuota } = useDataCache();
  const cacheEntry = providerQuota["opencodego"];
  const quotaEntries = cacheEntry?.entries ?? [];
  const refreshing = cacheEntry?.refreshing ?? false;
  // 是否已完成首轮查询（ts>0）：区分「还没查过」与「查过确认无数据」
  //（照 GenericQuotaPanel；opencodego 无凭证录入入口，用户没有凭证卡可看，
  // 首轮未回时不能一片空白）
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
  // 不渲染（不留一块空标题），此时下方用量面板的空态已说明「没有本地数据」。
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
              <BrandIcon brand="opencodego" className="h-3.5 w-3.5 shrink-0" />
              <span className="section-title truncate">
                {t("stats.opencodeQuota")}
              </span>
            </div>
            <button
              onClick={() => void refreshProviderQuota("opencodego")}
              disabled={refreshing}
              className={`toolbar-btn shrink-0 ${refreshing ? "opacity-40" : ""}`}
              title={t("common.refresh")}
            >
              ↻
            </button>
          </div>

          {quotaEntries.length === 0 ? (
            // 空态一档：查询中或首轮未回（照 GenericQuotaPanel 的档位与文案；
            // opencodego 是本地型，没有凭证也会真的发起查询，故占位同样适用）
            <div className="card-base rounded-2xl px-3 py-3">
              <p className="text-[10px] text-slate-500 leading-relaxed">
                {t("credentials.quotaRefreshing")}
              </p>
            </div>
          ) : (
            // 每条凭证一张配额卡（与 GenericQuotaPanel / KimiOtherAccounts
            // 共用渲染，品牌色取 AGENT_COLOR.opencodego）
            <div className="space-y-1.5">
              {quotaEntries.map((entry) => (
                <QuotaEntryCard
                  key={entry.credentialId}
                  entry={entry}
                  accent={AGENT_COLOR.opencodego}
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
          rowBar: "bg-orange-500/10",
          sortSelected: "bg-orange-500/20 text-orange-700",
          badge: "bg-orange-500/15 text-orange-700",
          accent: "orange",
        }}
        empty={{
          name: "OpenCode",
          icon: "🧩",
          title: t("stats.opencodeNotFound"),
          hint: t("stats.opencodeNotFoundHint"),
        }}
        cacheRateMode="included"
      />
    </>
  );
}
