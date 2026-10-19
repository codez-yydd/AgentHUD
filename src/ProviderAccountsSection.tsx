import { useEffect, useState } from "react";
import type { MessageKey } from "./i18n";
import { useI18n } from "./i18n";
import type { CredentialKind } from "./types";
import { useDataCache, PROVIDER_QUOTA_STALE_MS } from "./DataCache";
import {
  CredentialsCard,
  type CredentialRegionOption,
} from "./CredentialsCard";
import type { BrandIconName } from "./BrandIcon";
import { QuotaEntryCard } from "./QuotaEntryCard";
import { useResetDisplay } from "./resetDisplay";

export interface ProviderAccountsSectionProps {
  /** 凭证 provider id（决定凭证文件 ~/.agenthud/credentials/<provider>.json 与查询分发）。
   *  可与面板 agent id 不同：DSH 面板承载 DeepSeek 平台余额，故传 "deepseek"。 */
  provider: string;
  /** 区块标题（词典键）。 */
  titleKey: MessageKey;
  /** 该 provider 的凭证录入形态（决定 secret 输入引导与占位符）。 */
  kind: CredentialKind;
  /** 完整接入步骤文案键（展开显示）。 */
  guideKey: MessageKey;
  /** 空态品牌图标。 */
  brand: BrandIconName;
  /** 区域下拉选项；不传则该 provider 无区域概念。 */
  regionOptions?: ReadonlyArray<CredentialRegionOption>;
  /** 本地型 provider：数据由本地库/本地登录态直读，凭证为「可选」补充来源，
   *  空态按双数据源引导（不传 = 常规凭证引导）。 */
  optional?: boolean;
  /** 覆盖双数据源空态提示（缺省 credentials.localEmptyHintDual，该文案是
   *  OpenCode 口径；MiniMax Code 传自己的键）。 */
  dualHintKey?: MessageKey;
  /** 额度卡品牌色（仅 renderQuotaCards 时生效；四个调用点均显式传入）。 */
  accent?: string;
  /** 是否在本区渲染该 provider 的手动凭证额度卡：
   *  - kimi / dsh 为 true——它们的面板里没有别处渲染 providerQuota 条目；
   *  - opencodego / minimax 为 false——其面板顶部额度块已渲染全部条目
   *    （本地口径 + 手动凭证混排），本区只承担凭证管理。 */
  renderQuotaCards?: boolean;
}

/**
 * 通用「其他账号」区：某 provider 的凭证管理卡 + （可选）每条凭证一张额度卡。
 *
 * 抽取自 KimiPanel 的 KimiOtherAccounts（结构与 ClaudePanel / CursorPanel 的
 * 「其他账号」区同构），现由四个本地采集型面板共用：Kimi Code（kimi）、
 * OpenCode（opencodego）、MiniMax Code（minimax）、DeepSeek Harness（deepseek
 * 平台余额）。用途是让**没装（或不想装）对应官方 CLI** 的用户也能手动录入
 * API Key，直接问官方接口拿订阅额度 / 账户余额。
 *
 * 口径（与抽取前逐条一致）：
 * - 本地 CLI 登录态不在此展示（各面板顶部已有本地/官方额度卡），避免双查询；
 * - 有凭证时默认展开（凭证卡 + 每账号一张配额卡）；无凭证时收起为单行入口
 *   （保持单账号用户界面基本不变，同时保留添加第一条凭证的入口）；
 * - 刷新时机：挂载补刷一轮 + 凭证增删改事件（DataCache 联动）+ 手动刷新；
 * - 凭证列表未加载完成不渲染（防闪：本地文件读，瞬时完成）。
 */
export function ProviderAccountsSection({
  provider,
  titleKey,
  kind,
  guideKey,
  brand,
  regionOptions,
  optional = false,
  dualHintKey,
  accent = "#4338ca",
  renderQuotaCards = false,
}: ProviderAccountsSectionProps) {
  const { t } = useI18n();
  const resetDisplay = useResetDisplay();
  const { credentials, refreshCredentials, providerQuota, refreshProviderQuota } =
    useDataCache();
  const entries = credentials[provider];
  const hasCreds = (entries?.length ?? 0) > 0;
  // 展开态：用户显式操作优先，否则有凭证自动展开、无凭证收起
  const [expandedOverride, setExpandedOverride] = useState<boolean | null>(null);
  const open = expandedOverride ?? hasCreds;
  const cacheEntry = providerQuota[provider];
  const quotaEntries = cacheEntry?.entries ?? [];
  const refreshing = cacheEntry?.refreshing ?? false;

  // 凭证列表为按需缓存（无轮询）：首挂载加载一次
  const entriesLoaded = entries !== undefined;
  useEffect(() => {
    if (!entriesLoaded) refreshCredentials(provider).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entriesLoaded, provider]);

  // 有凭证但额度缓存缺失或已老化（>120s）→ 立即补刷一轮。只判「无缓存」
  // 不够：冷启动会从 localStorage 恢复上次的 providerQuota 缓存，无缓存
  // 条件恒 false，额度直到下个 120s 轮询都不刷新；老化阈值与轮询同频
  useEffect(() => {
    if (!hasCreds) return;
    const cached = providerQuota[provider];
    if (!cached || Date.now() - cached.ts > PROVIDER_QUOTA_STALE_MS) {
      void refreshProviderQuota(provider);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hasCreds, provider]);

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

  // 凭证列表未加载完成不渲染（防闪：加载为本地文件读，瞬时完成）
  if (!entriesLoaded) return null;

  return (
    <div className="shrink-0 max-h-[45%] overflow-y-auto overscroll-contain border-t border-slate-900/8 mx-3 pb-2.5">
      {/* 区块头：标题 + 计数 + 手动刷新 + 展开收起 */}
      <div className="flex items-center justify-between gap-2 pt-1.5 pb-1">
        <button
          onClick={() => setExpandedOverride(!open)}
          className="flex items-center gap-1 min-w-0 text-[10px] text-slate-500 hover:text-slate-700 transition-colors"
        >
          <span
            className={`inline-block transition-transform ${open ? "rotate-90" : ""}`}
          >
            ›
          </span>
          <span className="truncate">{t(titleKey)}</span>
          {hasCreds && (
            <span className="num text-[9px] text-slate-500/70 shrink-0">
              {t("credentials.countBadge", { n: entries!.length })}
            </span>
          )}
        </button>
        <div className="flex items-center gap-1 shrink-0">
          {hasCreds && (
            <button
              onClick={() => void refreshProviderQuota(provider)}
              disabled={refreshing}
              className={`toolbar-btn shrink-0 ${refreshing ? "opacity-40" : ""}`}
              title={t("common.refresh")}
            >
              ↻
            </button>
          )}
        </div>
      </div>

      {open && (
        <>
          <CredentialsCard
            provider={provider}
            kind={kind}
            guideKey={guideKey}
            brand={brand}
            regionOptions={regionOptions}
            optional={optional}
            dualHintKey={dualHintKey}
          />

          {/* 额度查询中提示（有凭证但条目未返回；空 entries 时收敛展示） */}
          {renderQuotaCards && quotaEntries.length === 0 && refreshing && (
            <div className="card-base rounded-2xl px-3 py-2 mt-1.5">
              <p className="text-[10px] text-slate-500 leading-relaxed">
                {t("credentials.quotaRefreshing")}
              </p>
            </div>
          )}

          {/* 每条手动凭证一张配额卡（与 GenericQuotaPanel / 顶部额度块共用渲染） */}
          {renderQuotaCards &&
            quotaEntries.map((entry) => (
              <div key={entry.credentialId} className="mt-1.5 first:mt-0">
                <QuotaEntryCard
                  entry={entry}
                  accent={accent}
                  resetDisplay={resetDisplay}
                  now={now}
                />
              </div>
            ))}
        </>
      )}
    </div>
  );
}
