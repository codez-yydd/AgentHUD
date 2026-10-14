/**
 * 额度矩阵卡（大窗版独有）：行 = Agent（BrandIcon + AGENT_COLOR 惯例），
 * 行内每个「额度系列」一块——Z.ai 每账号一块、其余 Agent 各一块。
 * 每块展示：周额度剩余 %（进度条 + 剩余色阶，与总览页额度列同色板）、
 * 重置时刻（↻，带「下次重置」标注）、5 小时窗口（快照有该窗口才显示）。
 * 最近一次采样时刻写在卡片副标题里（原先是额度块上的原生 title 悬停，
 * 悬停前看不到）；各块自身取的都是该时刻之前各自最新的采样。
 *
 * 数值来自 buildQuotaMatrix（模型层），口径与窄窗 ComparePanel 的系列
 * 合并规则完全一致：本机 + 远端快照按「时间 + 账号指纹」去重取较高值。
 */
import { BrandIcon } from "../../BrandIcon";
import { formatResetStamp } from "../../format";
import { useI18n } from "../../i18n";
import { remainingGradient, remainingTextColor } from "../../widgets";
import { Card, Skeleton } from "../primitives";
import type { MatrixCell, MatrixRow } from "./model";

export function QuotaMatrixCard({
  rows,
  /** 本机额度快照尚未就绪（首屏卡片骨架，不阻塞页面其余卡片） */
  pending = false,
}: {
  rows: MatrixRow[];
  pending?: boolean;
}) {
  const { t } = useI18n();

  // 最近一次采样时刻：所有额度块里最晚的一个（无采样时不出这段副标题）
  let latestSampledAt = 0;
  for (const row of rows) {
    for (const cell of row.cells) {
      if (cell.sampledAt > latestSampledAt) latestSampledAt = cell.sampledAt;
    }
  }
  const sub = latestSampledAt
    ? `${t("dashboard.compare.matrixSub")} · ${t(
        "dashboard.compare.matrixSampledLatest",
        { time: formatResetStamp(latestSampledAt) }
      )}`
    : t("dashboard.compare.matrixSub");

  return (
    <Card title={t("dashboard.compare.matrixTitle")} sub={sub}>
      {pending && rows.length === 0 ? (
        // 本机额度快照未就绪：卡片内骨架（页面其余卡片照常渲染）
        <div className="db-skeleton-stack">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-16 w-full" />
          ))}
        </div>
      ) : rows.length === 0 ? (
        <div className="db-empty">{t("dashboard.compare.matrixEmpty")}</div>
      ) : (
        <div className="db-matrix">
          {rows.map((row) => (
            <div key={row.agent} className="db-matrix__row">
              <div className="db-matrix__agent">
                <span className="db-agent__icon" style={{ color: row.color }}>
                  <BrandIcon brand={row.brand} className="h-4 w-4" />
                </span>
                <span className="db-agent__name">{row.label}</span>
              </div>
              <div className="db-matrix__cells">
                {row.cells.map((cell) => (
                  <QuotaCellView key={cell.id} cell={cell} />
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}

function QuotaCellView({ cell }: { cell: MatrixCell }) {
  const { t } = useI18n();
  // 重置时刻小于当前时刻说明已过期（快照滞后），不展示倒计时语义的时间点
  const showReset = cell.resetAt != null && cell.resetAt > Date.now();
  const remaining = Math.min(Math.max(cell.remainingPct, 0), 100);

  return (
    <div className="db-qcell">
      <div className="db-qcell__head">
        <span className="db-qcell__dot" style={{ background: cell.color }} />
        <span
          className="db-qcell__label"
          title={cell.legacy ? t("compare.legacyAccountHint") : undefined}
        >
          {cell.label}
        </span>
      </div>

      {/* 数值措辞与面板既有词条对齐：周配额读作「周剩 84%」（quota.weekShort）、
          Cursor 账单周期读作「账单周期 84%」，不再用大窗自造的「剩余」后缀 */}
      <div className="db-qcell__value">
        <span className="db-qcell__kind">
          {cell.windowKind === "billing"
            ? t("dashboard.overview.billingCycle")
            : t("quota.weekShort")}
        </span>
        <span
          className="db-qcell__pct num"
          style={{ color: remainingTextColor(remaining) }}
        >
          {Math.round(remaining)}%
        </span>
      </div>

      <div className="db-qcell__track">
        <div
          className="db-qcell__fill"
          style={{ width: `${remaining}%`, background: remainingGradient(remaining) }}
        />
      </div>

      <div className="db-qcell__foot num">
        {showReset && cell.resetAt != null ? (
          /* ↻ 自身不表意：补「下次重置」的悬停说明与可访问名 */
          <span
            className="db-qcell__reset"
            title={t("dashboard.compare.matrixNextReset")}
            aria-label={t("dashboard.compare.matrixNextReset")}
          >
            ↻ {formatResetStamp(cell.resetAt)}
          </span>
        ) : (
          <span className="db-qcell__reset db-qcell__reset--none">--</span>
        )}
        {cell.hour5RemainingPct != null && (
          <span className="db-qcell__hour5">
            {t("common.hour5")}{" "}
            <span
              style={{ color: remainingTextColor(cell.hour5RemainingPct) }}
            >
              {t("common.remaining", {
                pct: Math.round(cell.hour5RemainingPct),
              })}
            </span>
          </span>
        )}
      </div>
    </div>
  );
}
