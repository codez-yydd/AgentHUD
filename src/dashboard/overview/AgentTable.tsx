/**
 * Agent 统计表：每个 Agent 一行——图标/名称、今日 Token、范围内 Token、
 * 范围内成本、周额度剩余%（取不到显示 "--"）、5 小时窗口剩余%（无该窗口
 * 显示 "--"）。
 *
 * 品牌图标与配色对齐 agentVisibility 的 AGENT_COLOR / BrandIcon 惯例
 * （agent id 同时就是 BrandIcon 品牌名，无需再做域映射）。
 * 「全部设备」范围内某 Agent 含其他设备贡献时，范围内 Token 旁加「含远端」
 * 小标记（token 值本身已是本机 + 远端的合并结果）。
 * 5 小时列优先取快照 hour5 窗口（Z.ai 的 hour5_pct、Codex/Claude/Kimi 的
 * windows key="hour5"），额度走 provider quota 通道的 Agent（OpenCode Go /
 * MiniMax Code）取同一份条目里的 5 小时窗口；其余显示 "--"。
 * 周额度列在 provider 条目带月窗口（key="monthly"，如 OpenCode Go）时，
 * 主体（周/账单周期）与进度条下方再附加一组紧凑的「xx% 月 ↻ 时间 + 细进度条」。
 */
import { BrandIcon } from "../../BrandIcon";
import { formatCost, formatResetStamp, formatTokensLocale } from "../../format";
import { useI18n } from "../../i18n";
import { remainingGradient, remainingTextColor } from "../../widgets";
import type { Currency } from "../../types";
import { costOf, type AgentRow } from "./derive";
import { Card } from "./primitives";
import type { Hour5Info } from "./useOverviewData";

export function AgentTable({
  rows,
  currency,
}: {
  rows: AgentRow[];
  currency: Currency;
}) {
  const { locale, t } = useI18n();

  return (
    <Card
      title={t("dashboard.overview.agentTitle")}
      sub={t("dashboard.overview.agentSub")}
    >
      {rows.length === 0 ? (
        <div className="db-empty">{t("dashboard.overview.agentEmpty")}</div>
      ) : (
        <div className="db-table-wrap">
          <table className="db-table">
            <thead>
              <tr>
                <th>{t("dashboard.overview.colAgent")}</th>
                <th className="db-table__num">{t("dashboard.overview.colToday")}</th>
                <th className="db-table__num">{t("dashboard.overview.colRange")}</th>
                <th className="db-table__num">{t("dashboard.overview.colCost")}</th>
                <th className="db-table__quota">{t("dashboard.overview.colQuota")}</th>
                <th
                  className="db-table__quota db-table__quota--compact"
                  title={t("dashboard.overview.colHour5Hint")}
                >
                  {t("dashboard.overview.colHour5")}
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.id}>
                  <td>
                    <div className="db-agent">
                      <span className="db-agent__icon" style={{ color: row.color }}>
                        <BrandIcon brand={row.id} className="h-4 w-4" />
                      </span>
                      <span className="db-agent__name">{row.label}</span>
                    </div>
                  </td>
                  <td className="db-table__num num">
                    {row.todayTokens > 0
                      ? formatTokensLocale(row.todayTokens, locale)
                      : "—"}
                  </td>
                  <td className="db-table__num num">
                    {row.rangeTokens > 0
                      ? formatTokensLocale(row.rangeTokens, locale)
                      : "—"}
                    {/* 范围内数值含其他设备贡献时的行内小标记（「全部设备」才会出现） */}
                    {row.remoteRangeTokens > 0 && (
                      <span
                        className="db-tag db-tag--remote"
                        title={t("dashboard.overview.remoteIncludedHint")}
                      >
                        {t("dashboard.overview.remoteIncluded")}
                      </span>
                    )}
                  </td>
                  <td className="db-table__num num">{costCellText(row, currency, t)}</td>
                  <td>
                    <QuotaCell row={row} />
                  </td>
                  <td className="db-table__quota--compact">
                    <Hour5Cell hour5={row.quota?.hour5 ?? null} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}

/** 成本单元格：有成本显示金额；用量的 token 无定价（后端按 0 折算）显示「未定价」；
 *  无用量显示占位符——避免把"没配价"伪装成"不花钱"。 */
function costCellText(
  row: AgentRow,
  currency: Currency,
  t: (key: "dashboard.overview.unpriced") => string
): string {
  const cost = costOf(row.rangeCostUsd, row.rangeCostCny, currency);
  if (cost > 0) return formatCost(cost, currency);
  if (row.rangeTokens > 0) return t("dashboard.overview.unpriced");
  return "—";
}

function QuotaCell({ row }: { row: AgentRow }) {
  const { t } = useI18n();
  if (!row.quota) {
    return <span className="db-table__dash num">--</span>;
  }
  const { remainingPct, resetAt, window, monthly } = row.quota;
  const showReset = resetAt != null && resetAt > Date.now();
  const monthlyResetAt = monthly?.resetAt ?? null;
  const showMonthlyReset = monthlyResetAt != null && monthlyResetAt > Date.now();
  return (
    <div className="db-quota">
      <div className="db-quota__row">
        <span
          className="db-quota__pct num"
          style={{ color: remainingTextColor(remainingPct) }}
        >
          {Math.round(remainingPct)}%
        </span>
        <span className="db-quota__label">
          {window === "billing"
            ? t("dashboard.overview.billingCycle")
            : t("dashboard.overview.weekShort")}
        </span>
        {showReset && resetAt != null && (
          <span className="db-quota__reset num">↻ {formatResetStamp(resetAt)}</span>
        )}
      </div>
      <div className="db-quota__track">
        <div
          className="db-quota__fill"
          style={{
            width: `${Math.min(Math.max(remainingPct, 0), 100)}%`,
            background: remainingGradient(remainingPct),
          }}
        />
      </div>
      {/* 月额度第二组：仅 provider 通道带 key="monthly" 窗口的 Agent（如
          OpenCode Go）渲染；快照体系 Agent 的 quota.monthly 恒为 null。
          与主行同构的紧凑版——「百分数 + 月 + ↻ 时间」小字一行 + 更细的
          进度条，置于主条下方，不干扰「百分数 + 色条」的主体结构 */}
      {monthly && (
        <div className="db-quota__monthly">
          <div className="db-quota__row db-quota__row--monthly">
            <span
              className="db-quota__pct num"
              style={{ color: remainingTextColor(monthly.remainingPct) }}
            >
              {Math.round(monthly.remainingPct)}%
            </span>
            <span className="db-quota__label">
              {t("dashboard.overview.monthlyShort")}
            </span>
            {showMonthlyReset && monthlyResetAt != null && (
              <span className="db-quota__reset num">
                ↻ {formatResetStamp(monthlyResetAt)}
              </span>
            )}
          </div>
          <div className="db-quota__track db-quota__track--mini">
            <div
              className="db-quota__fill"
              style={{
                width: `${Math.min(Math.max(monthly.remainingPct, 0), 100)}%`,
                background: remainingGradient(monthly.remainingPct),
              }}
            />
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * 5 小时窗口剩余：百分数 + ↻ 重置时刻 + 细进度条（列宽紧凑，不加窗口文字
 * 标签）。重置时刻与周额度列 QuotaCell 同款小字样式、行内直接可见；容器
 * title 悬停提示保留作为补充（无重置时刻或已过期时不显示提示）。
 * 行内不换行（CSS white-space: nowrap），保证 140px 紧凑列内不错位
 *（列宽对齐 .db-quota__track 主条，重置时刻整行完整可见）。
 * 无该窗口（或额度取数失败）时显示 "--"。
 */
function Hour5Cell({ hour5 }: { hour5: Hour5Info | null }) {
  const { t } = useI18n();
  if (!hour5) {
    return <span className="db-table__dash num">--</span>;
  }
  const remaining = Math.min(Math.max(hour5.remainingPct, 0), 100);
  const resetAt = hour5.resetAt;
  const showReset = resetAt != null && resetAt > Date.now();
  const resetHint =
    showReset && resetAt != null
      ? t("dashboard.overview.hour5ResetHint", {
          time: formatResetStamp(resetAt),
        })
      : undefined;
  return (
    <div className="db-quota db-quota--compact" title={resetHint}>
      <div className="db-quota__row">
        <span
          className="db-quota__pct num"
          style={{ color: remainingTextColor(remaining) }}
        >
          {Math.round(remaining)}%
        </span>
        {showReset && resetAt != null && (
          <span className="db-quota__reset num">
            ↻ {formatResetStamp(resetAt)}
          </span>
        )}
      </div>
      <div className="db-quota__track">
        <div
          className="db-quota__fill"
          style={{
            width: `${remaining}%`,
            background: remainingGradient(remaining),
          }}
        />
      </div>
    </div>
  );
}
