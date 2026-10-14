/**
 * Token 对比卡（大窗版独有）：所选范围内各 Agent 的实际用量对比，
 * 本机与「其他设备」分段展示（合并统计沿用窄窗口径：同一 Agent 的
 * 本机聚合 + 远端同步来源求和；Cursor 明细不上传，只有本机段）。
 *
 * 与额度百分比不是同一指标：额度看的是订阅窗口已用比例，这里看 token 实量。
 */
import { BrandIcon } from "../../BrandIcon";
import { formatTokensLocale } from "../../format";
import { useI18n } from "../../i18n";
import { agentBrandOf, type TokenComparison } from "./model";
import { Card, Skeleton } from "../primitives";

export function TokensCompareCard({
  rows,
  rangeLabel,
  hasRemote,
  pending = false,
}: {
  rows: TokenComparison[];
  /** 页头当前范围文案（卡片副标题） */
  rangeLabel: string;
  /** 当前筛选是否含其他设备（决定图例与分段是否展示） */
  hasRemote: boolean;
  /** 当前范围数据（含远端懒取）未就绪（卡片骨架） */
  pending?: boolean;
}) {
  const { locale, t } = useI18n();
  const max = Math.max(...rows.map((row) => row.localTokens + row.remoteTokens), 1);

  return (
    <Card
      title={t("dashboard.compare.tokensTitle")}
      sub={t("dashboard.compare.tokensSub", { range: rangeLabel })}
      action={
        hasRemote ? (
          <div className="db-legend">
            <span className="db-legend__item">
              <i className="db-swatch db-swatch--input" />
              {t("dashboard.compare.tokensLocal")}
            </span>
            <span className="db-legend__item">
              <i className="db-swatch db-swatch--output" />
              {t("dashboard.compare.tokensRemote")}
            </span>
          </div>
        ) : undefined
      }
    >
      {pending && rows.length === 0 ? (
        // 当前范围数据（本机或远端懒取）未就绪：卡片内骨架
        <div className="db-skeleton-stack db-skeleton-stack--rows">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-[20px] w-full" />
          ))}
        </div>
      ) : rows.length === 0 ? (
        <div className="db-empty">{t("dashboard.compare.tokensEmpty")}</div>
      ) : (
        <div className="db-tokencmp">
          {rows.map((row) => {
            const total = row.localTokens + row.remoteTokens;
            const requests = row.localRequests + row.remoteRequests;
            const localWidth = (row.localTokens / max) * 100;
            const remoteWidth = (row.remoteTokens / max) * 100;
            return (
              <div key={row.agent} className="db-tokencmp__row">
                <div className="db-tokencmp__agent">
                  <span className="db-agent__icon" style={{ color: row.color }}>
                    <BrandIcon brand={agentBrandOf(row.agent)} className="h-4 w-4" />
                  </span>
                  <span className="db-agent__name">{row.label}</span>
                </div>

                <div
                  className="db-tokencmp__bar"
                  title={`${row.label} ${formatTokensLocale(total, locale)}`}
                >
                  <div
                    className="db-tokencmp__seg db-tokencmp__seg--local"
                    style={{ width: `${localWidth}%` }}
                  />
                  <div
                    className="db-tokencmp__seg db-tokencmp__seg--remote"
                    style={{ width: `${remoteWidth}%` }}
                  />
                </div>

                <div className="db-tokencmp__numbers">
                  {/* 总量 + 请求数一行；本机/其他设备明细独立第二行
                      （不再与总量挤同一行）。各项单行截断，title 兜底 */}
                  <div className="db-tokencmp__numbers-main">
                    <span
                      className="num db-tokencmp__total"
                      title={formatTokensLocale(total, locale)}
                    >
                      {formatTokensLocale(total, locale)}
                    </span>
                    <span
                      className="db-tokencmp__requests"
                      title={t("dashboard.compare.tokensRequests", {
                        count: requests,
                      })}
                    >
                      {t("dashboard.compare.tokensRequests", { count: requests })}
                    </span>
                  </div>
                  {hasRemote && (
                    <span className="db-tokencmp__split num">
                      {t("dashboard.compare.tokensLocal")}{" "}
                      {formatTokensLocale(row.localTokens, locale)}
                      {" · "}
                      {t("dashboard.compare.tokensRemote")}{" "}
                      {formatTokensLocale(row.remoteTokens, locale)}
                    </span>
                  )}
                </div>
              </div>
            );
          })}
          <p className="db-tokencmp__foot">
            {t("dashboard.compare.tokensFootnote")}
          </p>
        </div>
      )}
    </Card>
  );
}
