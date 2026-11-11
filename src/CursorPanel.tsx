import { useEffect, useState, type CSSProperties } from "react";
import type {
  AgentQuotaDelta,
  CursorSnapshot,
  Currency,
  TrendPoint,
} from "./types";
import { formatCost, formatTokens, formatPct } from "./format";
import {
  CurrentModelBar,
  Metric,
  ProgressBar,
  TrendChart,
  remainingGradient,
  remainingTextColor,
} from "./widgets";
import { SortToggle } from "./layout";
import { useI18n } from "./i18n";
import { useDataCache, PROVIDER_QUOTA_STALE_MS } from "./DataCache";
import { CredentialsCard } from "./CredentialsCard";
import { ProviderQuotaErrorBanner, QuotaEntryCard } from "./QuotaEntryCard";
import { useResetDisplay } from "./resetDisplay";

interface Props {
  snapshot: CursorSnapshot | null;
  loading: boolean;
  error: string | null;
  currency: Currency;
  /** USD→CNY 汇率（events 花费转 CNY 用） */
  fxRate: number;
  autoQuotaDelta?: AgentQuotaDelta;
  apiQuotaDelta?: AgentQuotaDelta;
}

/** 美分 → 美元 */
function centsToUsd(cents: number | null): number | null {
  return cents == null ? null : cents / 100;
}

/** 套餐额度行：标签 + 剩余% 在上，通栏进度条在下（Auto / API 同一套） */
function PlanQuotaRow({
  label,
  hint,
  usedPct,
  delta,
}: {
  label: string;
  hint?: string;
  usedPct: number;
  delta?: AgentQuotaDelta;
}) {
  const { t } = useI18n();
  const remain = Math.max(0, 100 - usedPct);
  const deltaText =
    delta && delta.samples >= 2 && delta.pct > 0
      ? t("quota.todayDelta", { pct: Math.round(delta.pct) })
      : null;
  return (
    <div className="p-quota">
      <span className="p-quota__label" title={hint ? `${label} · ${hint}` : label}>
        {label}
      </span>
      <div className="p-quota__track">
        <div
          className="p-quota__fill"
          style={{ width: `${remain}%`, background: remainingGradient(remain) }}
        />
      </div>
      {deltaText && (
        <span className="p-quota__delta" title={deltaText}>
          {deltaText}
        </span>
      )}
      <span
        className="p-quota__pct num"
        style={{ color: remainingTextColor(remain) }}
        title={t("common.remaining", { pct: Math.round(remain) })}
      >
        {Math.round(remain)}%
      </span>
    </div>
  );
}

/**
 * Cursor 用量面板：主链路（本地 Cursor 登录态 / 凭证体系单条生效）+ 底部
 * 「其他账号」可折叠区（凭证体系 kind=cookie 多账号堆叠，照 ClaudePanel 模式）。
 * 无凭证时折叠区收起为单行入口，主链路展示与原先完全一致。
 */
export function CursorPanel(props: Props) {
  return (
    <div className="flex-1 min-h-0 flex flex-col">
      <CursorPanelContent {...props} />
      <CursorOtherAccounts />
    </div>
  );
}

function CursorPanelContent({
  snapshot,
  loading,
  error,
  currency,
  fxRate,
  autoQuotaDelta,
  apiQuotaDelta,
}: Props) {
  const { t } = useI18n();
  const [trendMetric, setTrendMetric] = useState<"cost" | "token">("cost");
  const [sortBy, setSortBy] = useState<"cost" | "token" | "requests">("cost");

  // 加载中 & 无数据
  if (loading && !snapshot) {
    return (
      <div className="flex-1 flex items-center justify-center text-xs text-slate-700/40">
        {t("common.loadingUsage", { name: "Cursor" })}
      </div>
    );
  }

  // 未登录 / 错误
  if (!snapshot || !snapshot.logged_in) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center px-6 text-center gap-2">
        <div className="text-2xl opacity-40">🖱️</div>
        <div className="text-xs text-slate-700/60 font-medium">
          {t("cursor.notLoggedIn")}
        </div>
        <div className="text-[10px] text-slate-700/40 leading-relaxed">
          {t("cursor.loginHint")}
        </div>
        {error && (
          <div className="text-[10px] text-red-600/70 mt-1 leading-relaxed">
            {error}
          </div>
        )}
      </div>
    );
  }

  // 错误但已有数据（events 拉取失败等情况）
  const partialError =
    snapshot.events_error ?? (error && snapshot.logged_in ? error : null);

  const plan = snapshot.plan;
  const onDemand = snapshot.on_demand;
  const events = snapshot.events;

  // 按需用量
  const odUsedUsd = centsToUsd(onDemand?.used_cents ?? null);
  const odLimitUsd = centsToUsd(onDemand?.limit_cents ?? null);

  // events 花费（转当前货币）
  const eventsCost =
    events?.total_cost_usd != null
      ? currency === "cny"
        ? events.total_cost_usd * fxRate
        : events.total_cost_usd
      : 0;
  // 26px 金额在 260px 窄窗 + 1.25 字号档下会超宽：值单行截断不折行，
  // 完整金额由 title 悬浮兜底
  const eventsCostText = formatCost(eventsCost, currency);

  // Cursor daily → TrendPoint（复用 TrendChart）
  const trendPoints: TrendPoint[] = (snapshot.daily ?? []).map((d) => ({
    label: d.date,
    total_tokens: d.total_tokens,
    requests: d.requests,
    cost_cny: d.cost_usd * fxRate,
    cost_usd: d.cost_usd,
  }));

  return (
    <div className="panel-body">
      {/* 账户信息 */}
      <div className="p-inline">
        <span className="p-inline__label">{t("cursor.account")}</span>
        <span className="p-inline__value">
          {snapshot.account_email || snapshot.account_name || t("cursor.unknown")}
        </span>
        {snapshot.membership_type && (
          <span
            className="p-tag capitalize"
            style={{ "--accent": "#8b5cf6" } as CSSProperties}
          >
            {snapshot.membership_type}
          </span>
        )}
      </div>

      {partialError && (
        <div className="px-2.5 py-1.5 rounded-lg bg-amber-500/15 text-amber-700 text-[10px]">
          {partialError}
        </div>
      )}

      <CurrentModelBar model={snapshot.current_model} />

      {/* 套餐额度：与 Cursor 客户端一致，只展示 Auto / API（本计费周期，不随时间范围变化） */}
      {plan && (
        <div className="p-card--frame px-2.5 py-2">
          <div className="p-card__head">
            <span className="p-card__title">{t("cursor.planQuota")}</span>
          </div>
          {plan.auto_pct != null || plan.api_pct != null ? (
            <div className="flex flex-col gap-0.5">
              {plan.auto_pct != null && (
                <PlanQuotaRow label="Auto" usedPct={plan.auto_pct} delta={autoQuotaDelta} />
              )}
              {plan.api_pct != null && (
                <PlanQuotaRow label="API" usedPct={plan.api_pct} delta={apiQuotaDelta} />
              )}
            </div>
          ) : (
            <div className="p-quota-agent__empty">{t("cursor.noQuotaData")}</div>
          )}
        </div>
      )}

      {/* 按需用量 */}
      {onDemand && onDemand.enabled !== false && (
        <div className="p-card--frame px-2.5 py-2">
          <div className="p-card__head">
            <span className="p-card__title">{t("cursor.onDemand")}</span>
            <span className="p-chart-label">
              {odUsedUsd != null
                ? formatCost(
                    currency === "cny" ? odUsedUsd * fxRate : odUsedUsd,
                    currency
                  )
                : "—"}
              {odLimitUsd != null
                ? " / " +
                  formatCost(
                    currency === "cny" ? odLimitUsd * fxRate : odLimitUsd,
                    currency
                  )
                : ""}
            </span>
          </div>
          {odLimitUsd != null && odLimitUsd > 0 && odUsedUsd != null && (
            <ProgressBar
              pct={(odLimitUsd - odUsedUsd) / odLimitUsd}
              height="h-1"
              gradient={remainingGradient(
                ((odLimitUsd - odUsedUsd) / odLimitUsd) * 100
              )}
            />
          )}
        </div>
      )}

      {/* 计费周期 */}
      {(snapshot.billing_cycle_start || snapshot.billing_cycle_end) && (
        <div className="flex items-center justify-between gap-2 p-faint-text">
          <span>
            {snapshot.billing_cycle_start
              ? t("cursor.cycle", { date: snapshot.billing_cycle_start.slice(0, 10) })
              : ""}
          </span>
          {snapshot.billing_cycle_end && (
            <span>
              {t("cursor.resetDate", { date: snapshot.billing_cycle_end.slice(0, 10) })}
            </span>
          )}
        </div>
      )}

      {/* events 概览（所选时间范围内的 Token 使用明细） */}
      {events && (
        <>
          <div className="p-hero p-hero--brand p-hero--violet">
            <div className="p-hero__head">
              <span className="p-eyebrow">
                {t("cursor.tokenSpend")} · {t("cursor.selectedRange")}
              </span>
              <span className="p-badge">
                <i />
                {t("common.totalTokens")} {formatTokens(events.total_tokens)}
              </span>
            </div>
            <div className="p-hero__value num" title={eventsCostText}>
              {eventsCostText}
            </div>
          </div>

          {/* 趋势图（Cursor events 按日聚合，bucket 始终为 day） */}
          {trendPoints.length > 0 && (
            <TrendChart
              points={trendPoints}
              bucket="day"
              currency={currency}
              metric={trendMetric}
              onMetricChange={setTrendMetric}
            />
          )}

          {/* 三个指标 */}
          <div className="grid grid-cols-3 gap-1.5">
            <Metric label={t("common.requests")} value={String(events.requests)} />
            <Metric
              label={t("common.cacheRate")}
              value={
                events.input_tokens + events.cache_read_tokens > 0
                  ? formatPct(
                      events.cache_read_tokens /
                        (events.input_tokens + events.cache_read_tokens),
                    )
                  : "0%"
              }
              accent="text-emerald-600"
            />
            <Metric label={t("common.output")} value={formatTokens(events.output_tokens)} />
          </div>

          {/* 按模型排行 */}
          {snapshot.by_model.length > 0 && (
            <div className="p-card--frame px-2.5 py-2">
              <div className="p-card__head">
                <span className="p-card__title">{t("cursor.byModel")}</span>
                <SortToggle
                  options={[
                    { key: "cost", label: t("common.cost") },
                    { key: "token", label: "Token" },
                    { key: "requests", label: t("common.requests") },
                  ]}
                  value={sortBy}
                  onChange={setSortBy}
                />
              </div>
              <div className="flex flex-col gap-0.5">
                {(() => {
                  const rows = snapshot.by_model.map((m) => ({
                    m,
                    sortVal:
                      sortBy === "cost"
                        ? m.cost_usd
                        : sortBy === "token"
                          ? m.total_tokens
                          : m.requests,
                  }));
                  rows.sort((a, b) => b.sortVal - a.sortVal);
                  const maxVal = rows.length ? rows[0].sortVal : 0;
                  return rows.map(({ m, sortVal }) => {
                    const pct =
                      maxVal > 0 ? Math.max(sortVal / maxVal, 0.02) : 0;
                    return (
                      <div key={m.model} className="p-model-row">
                        <div
                          className="p-model-row__bar bg-violet-500/10"
                          style={{ width: `${pct * 100}%` }}
                        />
                        <span className="p-model-row__name" title={m.model}>
                          {m.model}
                        </span>
                        <div className="p-model-row__nums">
                          {/* 请求/Token/花费三列深浅递进（浅→中→深） */}
                          <span className="p-model-row__req" title={t("common.requestCount")}>
                            {m.requests}
                          </span>
                          <span className="p-model-row__tok" title={t("common.totalTokens")}>
                            {formatTokens(m.total_tokens)}
                          </span>
                          <span className="p-model-row__cost">
                            {formatCost(
                              currency === "cny"
                                ? m.cost_usd * fxRate
                                : m.cost_usd,
                              currency
                            )}
                          </span>
                        </div>
                      </div>
                    );
                  });
                })()}
              </div>
            </div>
          )}
        </>
      )}

      {/* 无 events 数据但已登录 */}
      {!events && (
        <div className="p-card--frame px-3 py-6 text-center">
          <p className="text-[9.5px] leading-relaxed text-slate-600/80">
            {snapshot.events_error
              ? t("cursor.eventsFailed", { msg: snapshot.events_error })
              : t("cursor.noEvents")}
          </p>
        </div>
      )}
    </div>
  );
}

/**
 * Cursor「其他账号」区：凭证体系 ~/.agenthud/credentials/cursor.json 的
 * kind=cookie 条目（浏览器复制的 WorkosCursorSessionToken Cookie 头，含
 * 旧手动 cookie 的一次性迁移条目），每条凭证调同一 usage-summary 端点
 * 展示套餐额度（后端 get_provider_quota("cursor")）。
 * - 本地登录态不在此展示（上方主面板），避免双查询；
 * - 有凭证时默认展开（凭证卡 + 每账号一张配额卡）；无凭证时收起为单行
 *   入口（保持单账号用户界面基本不变，同时保留添加第一条凭证的入口）；
 * - 凭证为可选（optional）：主链路可自动读取本地 Cursor 登录态，手动
 *   Cookie 适合无本机登录态或多账号堆叠场景；
 * - 刷新时机：挂载补刷一轮（缓存缺失或老化 >120s）+ 凭证增删改事件
 *   （DataCache 联动）+ 手动刷新 + 120s 通用轮询（App 的 presence 探测
 *   名单含 cursor，有凭证才轮询）。
 */
function CursorOtherAccounts() {
  const { t } = useI18n();
  const resetDisplay = useResetDisplay();
  const {
    credentials,
    refreshCredentials,
    providerQuota,
    providerQuotaErrors,
    refreshProviderQuota,
  } = useDataCache();
  const entries = credentials["cursor"];
  const hasCreds = (entries?.length ?? 0) > 0;
  // 整体查询失败（command 级 Err，典型：凭证文件损坏）：
  // 是坏文件场景下本区唯一可达的自愈入口（见下方门槛注释）
  const quotaError = providerQuotaErrors["cursor"];
  // 展开态：用户显式操作优先，否则有凭证（或整体查询失败需展示自愈入口）
  // 自动展开、无凭证收起
  const [expandedOverride, setExpandedOverride] = useState<boolean | null>(null);
  const open = expandedOverride ?? (hasCreds || !!quotaError);
  const cacheEntry = providerQuota["cursor"];
  const quotaEntries = cacheEntry?.entries ?? [];
  const refreshing = cacheEntry?.refreshing ?? false;

  // 凭证列表为按需缓存（无轮询）：首挂载加载一次
  const entriesLoaded = entries !== undefined;
  useEffect(() => {
    if (!entriesLoaded) refreshCredentials("cursor").catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entriesLoaded]);

  // 有凭证但额度缓存缺失或已老化（>120s）→ 立即补刷一轮。只判「无缓存」
  // 不够：冷启动会从 localStorage 恢复上次的 providerQuota 缓存，无缓存
  // 条件恒 false，额度直到下个 120s 轮询都不刷新；老化阈值与轮询同频
  useEffect(() => {
    if (!hasCreds) return;
    const cached = providerQuota["cursor"];
    if (!cached || Date.now() - cached.ts > PROVIDER_QUOTA_STALE_MS) {
      void refreshProviderQuota("cursor");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hasCreds]);

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

  // 凭证列表未加载完成不渲染（防闪：加载为本地文件读，瞬时完成）。
  // 整体查询失败时仍渲染：坏文件会让凭证列表永远读取失败（entriesLoaded
  // 恒 false），若按原门槛整区隐藏，带「重置凭证文件」自愈入口的错误
  // banner 不可达，用户没有任何修复路径（2026-11-03 凭证损坏事故形态）
  if (!entriesLoaded && !quotaError) return null;

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
          <span className="truncate">{t("stats.cursorOtherAccounts")}</span>
          {hasCreds && (
            <span className="num text-[9px] text-slate-500/70 shrink-0">
              {t("credentials.countBadge", { n: entries!.length })}
            </span>
          )}
        </button>
        <div className="flex items-center gap-1 shrink-0">
          {hasCreds && (
            <button
              onClick={() => void refreshProviderQuota("cursor")}
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
            provider="cursor"
            kind="cookie"
            guideKey="credentials.guide.cursor"
            brand="cursor"
            optional
          />

          {/* 整体查询失败提示（command 级 Err，如凭证文件损坏）：无旧值时它
              就是本区的主要状态（不再永远「查询中」），有旧值时叠加在配额卡
              上方——旧数据照常展示，但用户必须知道它已不再新鲜。
              凭证文件损坏时本卡内嵌「重置凭证文件」自愈入口（见组件注释） */}
          <ProviderQuotaErrorBanner provider="cursor" className="mt-1.5" />

          {/* 额度查询中提示（有凭证但条目未返回；空 entries 时收敛展示） */}
          {quotaEntries.length === 0 && refreshing && (
            <div className="card-base rounded-2xl px-3 py-2 mt-1.5">
              <p className="text-[10px] text-slate-500 leading-relaxed">
                {t("credentials.quotaRefreshing")}
              </p>
            </div>
          )}

          {/* 每条手动凭证一张配额卡（与 Claude 其他账号区共用渲染） */}
          {quotaEntries.map((entry) => (
            <div key={entry.credentialId} className="mt-1.5 first:mt-0">
              <QuotaEntryCard
                entry={entry}
                accent="#8b5cf6"
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
