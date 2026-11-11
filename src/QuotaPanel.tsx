import { useEffect, useState, type CSSProperties } from "react";
import { useDataCache } from "./DataCache";
import {
  formatCountdownCore,
  formatResetStamp,
  formatTokens,
  levelLabel,
} from "./format";
import { remainingGradient, remainingTextColor } from "./widgets";
import { useI18n } from "./i18n";
import { useResetDisplay } from "./resetDisplay";
import {
  SwitchAccountButton,
  SwitchConfirmOverlay,
  useAccountSwitch,
} from "./accountSwitch";
import type { AccountQuotaEntry } from "./types";

export function QuotaPanel() {
  // 额度数据由全局 DataProvider 统一预加载 + 30s 定时刷新，
  // 此组件仅负责展示（纯展示层），不再自己请求。
  const { quota, quotaError, todayDelta, refreshQuota, accountQuotas } =
    useDataCache();
  // 卡片内嵌账号切换（全部账号区各非当前账号行）
  const sw = useAccountSwitch();
  const { t } = useI18n();
  const [now, setNow] = useState(Date.now());

  // 每秒更新倒计时显示
  useEffect(() => {
    const tickTimer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tickTimer);
  }, []);

  // 本地凭证缺失（未在 ZCode 客户端登录 Coding Plan）→ 显示登录引导
  // 注意：正则匹配 Rust 后端返回的中文错误串（quota.rs 的固定前缀文案），仅做布尔分支，不能翻译
  if (quotaError && quotaError.includes("未找到 ZCode Coding Plan 凭证")) {
    return (
      <div className="p-card--frame px-3 py-2.5">
        <div className="p-card__title">{t("quota.title")}</div>
        <p className="mt-1 text-[9.5px] leading-relaxed text-slate-600/80">
          {t("quota.configHint")}
        </p>
      </div>
    );
  }

  // 其他错误：仍有多账号数据时降级渲染整卡（当前账号行报错、其余账号照常展示
  // ——当前凭证失效时恰恰是用户最需要看其他账号、考虑切换的时刻）
  const quotaFailed = quotaError != null || !quota;
  if (quotaFailed && accountQuotas.length < 2) {
    return (
      <div className="p-card--frame px-3 py-2.5">
        <span className="text-[9.5px] leading-relaxed text-amber-700/90">
          {quotaError ? t("quota.failed", { msg: quotaError }) : t("common.loading")}
        </span>
      </div>
    );
  }

  const planBadge = quota?.level ? levelLabel(quota.level) : "—";

  return (
    <div className="relative p-card--frame px-3 py-2.5">
      {/* 标题行：与汇总页卡片同一套小标题 + 品红等级徽标 */}
      <div className="p-card__head">
        <div className="flex items-center gap-1.5 min-w-0">
          <span className="p-card__title">Coding Plan</span>
          <span
            className="p-tag"
            style={{ "--accent": "#8b5cf6" } as CSSProperties}
          >
            {planBadge}
          </span>
        </div>
        <button
          onClick={refreshQuota}
          className="card-frame__control"
          title={t("quota.refresh")}
          aria-label={t("quota.refresh")}
        >
          <svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.9"
            strokeLinecap="round"
            strokeLinejoin="round"
            className="h-3 w-3"
            aria-hidden
          >
            <path d="M20.5 11.5a8.5 8.5 0 1 0-2.6 6.1" />
            <path d="M20.5 4.5v6.2h-6.2" />
          </svg>
        </button>
      </div>

      {/* 当前账号额度（失败时此处留空，下方"全部账号"区当前账号行显示错误） */}
      {quotaFailed && (
        <p className="mb-1.5 text-[9.5px] leading-relaxed text-amber-700/90">
          {quotaError ? t("quota.failed", { msg: quotaError }) : t("common.loading")}
        </p>
      )}

      {/* 5小时窗口 */}
      {quota?.hour5 && (
        <QuotaBar
          label={t("common.hour5")}
          usedPct={quota.hour5.percentage}
          resetAt={quota.hour5.nextResetTime}
          now={now}
        />
      )}

      {/* 每周 */}
      {quota?.weekly && (
        <QuotaBar
          label={t("common.weekly")}
          usedPct={quota.weekly.percentage}
          resetAt={quota.weekly.nextResetTime}
          now={now}
          className="mt-1"
          delta={todayDelta}
        />
      )}

      {/* MCP 月度额度 */}
      {quota?.mcp && (
        <McpBar
          usedPct={quota.mcp.percentage}
          resetAt={quota.mcp.nextResetTime}
          now={now}
          currentValue={quota.mcp.currentValue}
          total={quota.mcp.usage}
          className="mt-1"
        />
      )}

      {/* 全部账号：≥2 个快照时逐账号展示（仅 1 个时与上方重复，不显示）。
          当前账号条目已由 DataCache 用 30s live quota 覆盖，其余为 5 分钟一轮。
          列表限高内滚：账号多时不能把 zai 标签的统计主内容挤出可视区。
          非当前账号行带「切换」按钮（确认后退出并重启 ZCode）。 */}
      {accountQuotas.length >= 2 && (
        <div className="mt-2 pt-2 border-t border-slate-900/6">
          <div className="p-label mb-1.5">{t("quota.allAccounts")}</div>
          <div className="max-h-36 overflow-y-auto overscroll-contain space-y-2">
            {accountQuotas.map((e) => (
              <AccountQuotaRow
                key={e.id}
                entry={e}
                onSwitch={e.is_current ? undefined : () => sw.request(e)}
                switchDisabled={sw.switching}
              />
            ))}
          </div>
        </div>
      )}

      {/* 切换结果反馈：成功绿色短暂展示，失败红字保留 */}
      {sw.notice && (
        <p
          className={`text-[9px] mt-1.5 leading-relaxed break-all ${
            sw.notice.kind === "ok" ? "text-emerald-600" : "text-rose-600"
          }`}
        >
          {sw.notice.text}
        </p>
      )}

      {/* 切换确认浮层（覆盖整卡） */}
      {sw.confirming && (
        <SwitchConfirmOverlay
          account={sw.confirming}
          switching={sw.switching}
          onConfirm={sw.confirm}
          onCancel={sw.cancel}
        />
      )}
    </div>
  );
}

/** 多账号列表单行：名称 + 当前 pill + 等级徽标（sky，与汇总页 ZCode 徽标同色；
 *  violet 只留给"当前"一个语义）+ 每周剩余（条）+ 5小时剩余小字 + 切换按钮 */
function AccountQuotaRow({
  entry,
  onSwitch,
  switchDisabled,
}: {
  entry: AccountQuotaEntry;
  /** 非当前账号时传入（点击弹切换确认）；当前账号为 undefined */
  onSwitch?: () => void;
  switchDisabled?: boolean;
}) {
  const { t } = useI18n();
  const { display_name, is_current, quota, error } = entry;

  if (!quota) {
    return (
      <div className="p-quota-account" title={error ?? undefined}>
        <span className="p-quota-account__name">{display_name}</span>
        {is_current && (
          <span className="p-tag" style={{ "--accent": "#8b5cf6" } as CSSProperties}>
            {t("settings.accountsCurrent")}
          </span>
        )}
        <span className="ml-auto shrink-0 text-[8.5px] text-rose-500/90">
          {t("quota.quotaFail")}
        </span>
      </div>
    );
  }

  // 每周数据缺失时不得当作"剩余 100%"渲染（接口异常/未开通），整行数值与条都不出
  const weeklyRemain = quota.weekly
    ? Math.max(0, 100 - quota.weekly.percentage)
    : null;
  const hour5RemainPct = quota.hour5
    ? Math.max(0, 100 - quota.hour5.percentage)
    : null;
  // 今日增量：各账号自己的带指纹采样算出（当前账号由 DataCache 用 30s live 值
  // 覆盖）；采样不足(<2)不显示，与 QuotaBar 的 delta 徽标同口径
  const delta = entry.today_delta;
  const hasDelta = delta != null && delta[1] >= 2 && delta[0] > 0;

  return (
    <div>
      <div className="p-quota-account">
        <span className="p-quota-account__name">{display_name}</span>
        {is_current && (
          <span className="p-tag" style={{ "--accent": "#8b5cf6" } as CSSProperties}>
            {t("settings.accountsCurrent")}
          </span>
        )}
        {quota.level && (
          <span className="p-tag" style={{ "--accent": "#0ea5e9" } as CSSProperties}>
            {levelLabel(quota.level)}
          </span>
        )}
        {onSwitch && (
          <span className="ml-auto shrink-0">
            <SwitchAccountButton onClick={onSwitch} disabled={switchDisabled} />
          </span>
        )}
        {weeklyRemain != null && (
          <span
            className={`num text-[9.5px] font-semibold shrink-0 whitespace-nowrap ${
              onSwitch ? "ml-1.5" : "ml-auto"
            }`}
            style={{ color: remainingTextColor(weeklyRemain) }}
            title={t("common.remaining", { pct: Math.round(weeklyRemain) })}
          >
            {t("quota.weekShort")} {Math.round(weeklyRemain)}%
          </span>
        )}
      </div>
      {weeklyRemain != null && (
        <div className="p-quota__track mt-1">
          <div
            className="p-quota__fill"
            style={{
              width: `${weeklyRemain}%`,
              background: remainingGradient(weeklyRemain),
            }}
          />
        </div>
      )}
      <div className="p-quota-meta">
        {hour5RemainPct != null && (
          <span>
            {t("quota.hour5Short")} {Math.round(hour5RemainPct)}%
          </span>
        )}
        {quota.mcp && (
          <span>
            MCP{" "}
            {typeof quota.mcp.currentValue === "number" &&
            typeof quota.mcp.usage === "number"
              ? `${quota.mcp.currentValue}/${quota.mcp.usage}`
              : `${Math.round(quota.mcp.percentage)}%`}
          </span>
        )}
        {hasDelta && (
          <span>{t("quota.todayDelta", { pct: delta[0] })}</span>
        )}
      </div>
    </div>
  );
}

/** 额度条行内的重置时间（QuotaBar / McpBar 共用；原独立列 ResetSpan 已并入行内）：
 *  三态按 useResetDisplay——倒计时开启（双开或仅倒计时）显示精简倒计时「↻ 3h48m」，
 *  双开时时间点移入 title；仅时间点显示「↻ 10-15 05:17」；两者都关返回 null
 *  （不渲染空节点，避免 flex gap 白占宽）。
 *  完整文案（含「后刷新 / 重置于」本地化后缀）统一由 title 悬浮兜底；
 *  260px 窄窗下行内空间不足时按 min-w-0 + truncate 省略。 */
function ResetInline({ resetAt, now }: { resetAt: number; now: number }) {
  const { t } = useI18n();
  const display = useResetDisplay();
  if (!display.countdown && !display.datetime) return null;
  const countdown = formatCountdownCore(resetAt - now);
  const stamp = formatResetStamp(resetAt);
  const title = `↻ ${t("common.refreshIn", { time: countdown })} · ${t(
    "common.resetAt",
    { time: stamp }
  )}`;
  return (
    <span className="p-quota__reset" title={title}>
      {display.countdown ? `↻ ${countdown}` : `↻ ${stamp}`}
    </span>
  );
}

/** 单条额度进度条（剩余版：填充剩余量，颜色随剩余渐变）。
 *  单行内联布局：标签 / 进度条(flex-1) / 剩余% / 重置倒计时 / 今日增量 同行，
 *  三组额度纵向占用由原三行结构（约 34px/组）压到约 16px/组。 */
function QuotaBar({
  label,
  usedPct,
  resetAt,
  now,
  className = "",
  delta,
}: {
  label: string;
  usedPct: number;
  resetAt: number | null;
  now: number;
  className?: string;
  /** 今日增量：[增量百分比, 采样数]。仅 weekly 传入 */
  delta?: [number, number] | null;
}) {
  const { t } = useI18n();
  const used = Math.min(Math.max(usedPct, 0), 100);
  const remaining = 100 - used;

  // 今日增量徽标：采样不足(<2)不显示数字
  const deltaPct = delta ? delta[0] : 0;
  const hasDelta = delta != null && delta[1] >= 2 && deltaPct > 0;

  return (
    <div className={`p-quota ${className}`}>
      <span className="p-quota__label" title={label}>
        {label}
      </span>
      <div className="p-quota__track">
        <div
          className="p-quota__fill"
          style={{
            width: `${remaining}%`,
            background: remainingGradient(remaining),
          }}
        />
      </div>
      {/* 今日增量：紧凑「↑N%」，完整本地化文案 title 兜底 */}
      {hasDelta && (
        <span
          className="p-quota__delta"
          title={t("quota.todayDelta", { pct: deltaPct })}
        >
          ↑{deltaPct}%
        </span>
      )}
      {resetAt != null && resetAt > now && (
        <ResetInline resetAt={resetAt} now={now} />
      )}
      <span
        className="p-quota__pct num"
        style={{ color: remainingTextColor(remaining) }}
        title={t("common.remaining", { pct: Math.round(remaining) })}
      >
        {Math.round(remaining)}%
      </span>
    </div>
  );
}

/** MCP 绝对值行内紧凑格式：千级以内原样（12/1000），上万后单个数值超 5 位，
 *  连同进度条一起挤压行内空间，改用 formatTokens 缩写（12345 → 12.3K） */
function compactAbs(n: number): string {
  return n >= 10_000 ? formatTokens(n) : String(n);
}

/** MCP 月度额度条：与 QuotaBar 同款单行内联布局（剩余版）。
 *  - 标签固定文本 "MCP"；中间倒计时接口通常不返回（无 nextResetTime 则不渲染）
 *  - 绝对值「已用 / 总量」由原独立副行并入本行（8px 紧凑串，原始值 title 兜底） */
function McpBar({
  usedPct,
  resetAt,
  now,
  currentValue,
  total,
  className = "",
}: {
  usedPct: number;
  resetAt?: number | null;
  now: number;
  currentValue?: number;
  total?: number;
  className?: string;
}) {
  const { t } = useI18n();
  const used = Math.min(Math.max(usedPct, 0), 100);
  const remaining = 100 - used;

  // 行内绝对值：两端都有值时给出「已用/总量」紧凑串 + 原始值（title 悬浮可查）
  const abs =
    typeof currentValue === "number" && typeof total === "number"
      ? {
          text: `${compactAbs(currentValue)}/${compactAbs(total)}`,
          title: `${currentValue} / ${total}`,
        }
      : null;

  return (
    <div className={`p-quota ${className}`}>
      <span className="p-quota__label">MCP</span>
      <div className="p-quota__track">
        <div
          className="p-quota__fill"
          style={{
            width: `${remaining}%`,
            background: remainingGradient(remaining),
          }}
        />
      </div>
      {resetAt != null && resetAt > now && (
        <ResetInline resetAt={resetAt} now={now} />
      )}
      {abs && (
        <span className="p-quota__reset" title={abs.title}>
          {abs.text}
        </span>
      )}
      <span
        className="p-quota__pct num"
        style={{ color: remainingTextColor(remaining) }}
        title={t("common.remaining", { pct: Math.round(remaining) })}
      >
        {Math.round(remaining)}%
      </span>
    </div>
  );
}
