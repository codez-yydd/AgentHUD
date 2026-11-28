/**
 * 重置序列（驾驶舱右栏）：当前 resetAt 在未来的额度窗口，按重置时刻
 * 升序排列。每条含来源/账号、窗口、绝对重置时刻、倒计时与剩余条；
 * 最近的一条标「最近」。
 *
 * 版式与左栏「额度仪表」对齐：上方是同一套区块标题行（标题 + 副标题 +
 * 右侧数量元信息），下方是卡片本体（条目列表）。
 */
import type { CSSProperties } from "react";
import { formatResetStamp } from "../../format";
import type { TFn } from "../../i18n";
import {
  formatDateTime,
  resetCountdownText,
  windowLabel,
} from "./format";
import { sourceColorOf, type CockpitResetItem } from "./model";
import { quotaTone } from "./tone";

export function ResetRail({
  items,
  nowMs,
  t,
}: {
  items: CockpitResetItem[];
  nowMs: number;
  t: TFn;
}) {
  return (
    <>
      {/* 区块标题行：与「额度仪表」同款（标题 / 副标题 / 数量元信息） */}
      <header className="db-qc-section-head">
        <div>
          <h2 className="db-qc-section-title">
            {t("dashboard.quota.resetTitle")}
          </h2>
          <p className="db-qc-section-sub db-qc-section-sub--nowrap">
            {t("dashboard.quota.timelineOrder")}
          </p>
        </div>
        <span className="db-qc-section-meta num">{items.length}</span>
      </header>

      <div className="db-card db-qc-rail__card">
        {items.length === 0 ? (
          <div className="db-empty">{t("dashboard.quota.resetEmpty")}</div>
        ) : (
          <ol className="db-qc-timeline">
            {items.map((item, index) => {
              const tone = quotaTone(item.remainingPct);
              return (
                <li
                  key={`${item.series.id}:${item.windowKey}`}
                  className={`db-qc-reset ${
                    index === 0 ? "db-qc-reset--next" : ""
                  }`}
                  style={
                    { "--qc-source": sourceColorOf(item.series.source) } as CSSProperties
                  }
                >
                  <span className="db-qc-reset__dot" aria-hidden />
                  <div className="db-qc-reset__body">
                    <div className="db-qc-reset__top">
                      <span
                        className="db-qc-reset__label"
                        title={item.series.label}
                      >
                        {item.series.label}
                      </span>
                      {index === 0 && (
                        <span className="db-qc-reset__next">
                          {t("dashboard.quota.timelineNext")}
                        </span>
                      )}
                    </div>
                    <span
                      className="db-qc-reset__meta"
                      title={formatDateTime(item.resetAt)}
                    >
                      {windowLabel(t, item.windowKey)} ·{" "}
                      {formatResetStamp(item.resetAt)}
                    </span>
                    <div className="db-qc-reset__meter" aria-hidden>
                      <span
                        style={{
                          width: `${Math.min(Math.max(item.remainingPct, 0), 100)}%`,
                          background: tone,
                        }}
                      />
                    </div>
                  </div>
                  <div className="db-qc-reset__right">
                    <span className="db-qc-reset__in num">
                      {resetCountdownText(t, item.resetAt, nowMs)}
                    </span>
                    <span
                      className="db-qc-reset__pct num"
                      style={{ color: tone }}
                    >
                      {Math.round(item.remainingPct)}%
                    </span>
                  </div>
                </li>
              );
            })}
          </ol>
        )}
      </div>
    </>
  );
}
