/**
 * 请求日志明细表：10 列（时间 / 来源 / 模型 / 输入 / 输出 / 缓存 / 花费 / 耗时 /
 * 首字 / 速度）。缓存读写合并为一列（"12.3万 / 4.5万"，精确值走 title），列宽按
 * 1100 默认窗口的内容区（约 840px）排布——固定列宽合计 = 表 min-width 800px
 * （96 + 72 + 150 + 64 + 64 + 100 + 68 + 58 + 56 + 72，逐列见 dashboard.css
 * 的 .db-logs__col-*；模型列取 min，富余归它，上限 320）。更窄的窗口或超长
 * 数值（跨年时间、十亿级 token）由 .db-table-wrap 横向滚动兜底，不会把数值
 * 挤成两行。
 *
 * 单元格口径：
 *  - 时间：今天 "HH:mm:ss"，其余 "MM-DD HH:mm"（跨年补年份），完整时间在 title 里
 *    （详情抽屉也有）；来源列显示短名，title 给与下拉一致的全名；
 *  - token 用项目现成的 locale 感知紧凑格式化（中文 万/亿、英文 K/M/B），
 *    缓存列两项并列；
 *  - 花费：按展示币种取一列；有 token 但折算为 0 → 「未定价」（与总览页 Agent 表
 *    同一套口径，避免把"没配价"伪装成"不花钱"）；无 token → 「—」；
 *  - 耗时 / 首字：秒一位 / 两位小数；耗时只认来源库的真实值（Codex 会话文件无
 *    耗时、旧版 Claude Code 不落盘耗时 → 灰化「—」）；速度口径与详情抽屉完全
 *    一致（model.speedOf + formatTps：生成窗口 = 耗时 − 首字，无首字时退回总
 *    耗时并标 ≈，title 说明原因）；
 *  - 缺失值一律灰化「—」并带 title 说明原因（无数据 / 未定价 / 来源不落盘耗时或
 *    首字——属数据源限制，不是加载失败；无耗时时速度列与耗时列同为「—」）。
 * 行可点击打开详情抽屉（键盘 Enter / Space 同效）；`busy` 期间旧行整体压暗
 * 且不可点（翻页 / 切筛选的等待期，避免点开已被替换的数据）。
 */
import type { ReactNode } from "react";
import { Skeleton } from "../primitives";
import { useI18n } from "../../i18n";
import { formatCost, formatTokensLocale, formatTps } from "../../format";
import { costOf } from "../overview/derive";
import type { Currency, RequestLogRow } from "../../types";
import {
  LOG_PAGE_SIZE,
  durationText,
  exactCount,
  logTimeFull,
  logTimeShort,
  sourceLabel,
  sourceShortLabel,
  speedApproxHintKey,
  speedOf,
  ttftText,
} from "./model";

/** 灰化占位：无值默认「—」，也可放文字（未定价）；title 说明为什么没有值 */
function Dash({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <span className="db-table__dash" title={title}>
      {children ?? "—"}
    </span>
  );
}

/** 表头（真实表格与骨架共用同一份，保证形状一致） */
function LogTableHead() {
  const { t } = useI18n();
  return (
    <thead>
      <tr>
        <th className="db-logs__col-time">{t("dashboard.logs.colTime")}</th>
        <th className="db-logs__col-source">{t("dashboard.logs.colSource")}</th>
        <th className="db-logs__col-model">{t("dashboard.logs.colModel")}</th>
        <th className="db-table__num db-logs__col-input">
          {t("dashboard.logs.colInput")}
        </th>
        <th className="db-table__num db-logs__col-output">
          {t("dashboard.logs.colOutput")}
        </th>
        <th className="db-table__num db-logs__col-cache">
          {t("dashboard.logs.colCache")}
        </th>
        <th className="db-table__num db-logs__col-cost">
          {t("dashboard.logs.colCost")}
        </th>
        <th className="db-table__num db-logs__col-duration">
          {t("dashboard.logs.colDuration")}
        </th>
        <th className="db-table__num db-logs__col-ttft">
          {t("dashboard.logs.colTtft")}
        </th>
        <th className="db-table__num db-logs__col-speed">
          {t("dashboard.logs.colSpeed")}
        </th>
      </tr>
    </thead>
  );
}

export function LogTable({
  rows,
  nowMs,
  currency,
  busy,
  onOpen,
}: {
  rows: RequestLogRow[];
  /** 当前时刻（时间列「今天」判定的基准；页面每次取数后更新，保证跨日渲染正确） */
  nowMs: number;
  currency: Currency;
  /** 正在取数且表内仍是旧行：压暗 + aria-busy + 不可点开详情 */
  busy: boolean;
  onOpen: (row: RequestLogRow) => void;
}) {
  const { locale, t } = useI18n();

  return (
    <div
      className={`db-table-wrap${busy ? " db-table-wrap--busy" : ""}`}
      aria-busy={busy || undefined}
    >
      <table className="db-table db-logs-table">
        <LogTableHead />
        <tbody>
          {rows.map((row, index) => {
            const cost = costOf(row.cost_usd, row.cost_cny, currency);
            const duration = durationText(row.duration_ms);
            const ttft = ttftText(row.ttft_ms);
            const speed = speedOf(row);
            const speedHintKey = speedApproxHintKey(speed?.approximate === true);
            const fullTime = logTimeFull(row.started_at);
            return (
              <tr
                // 行内无稳定唯一键（同毫秒可并行多条），用「来源 + 时间 + 序号」
                // 组合键：同页内唯一且顺序稳定
                key={`${row.source}-${row.started_at}-${index}`}
                className="db-logs__row"
                tabIndex={0}
                onClick={() => {
                  if (!busy) onOpen(row);
                }}
                onKeyDown={(e) => {
                  if (busy) return;
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onOpen(row);
                  }
                }}
              >
                <td className="db-logs__col-time num" title={fullTime}>
                  {logTimeShort(row.started_at, nowMs)}
                </td>
                {/* 短名 + 全名 title：下拉叫 Z.ai / DeepSeek Harness，表里叫
                    ZCode / DSH，hover 可对上 */}
                <td className="db-logs__col-source" title={sourceLabel(row.source)}>
                  {sourceShortLabel(row.source)}
                </td>
                <td className="db-logs__col-model">
                  <span className="db-logs__model" title={row.model_id}>
                    {row.model_id || "—"}
                  </span>
                </td>
                <td className="db-table__num db-logs__col-input num">
                  {formatTokensLocale(row.input_tokens, locale)}
                </td>
                <td className="db-table__num db-logs__col-output num">
                  {formatTokensLocale(row.output_tokens, locale)}
                </td>
                {/* 缓存读 / 缓存写合并列：精确值在 title */}
                <td
                  className="db-table__num db-logs__col-cache num"
                  title={t("dashboard.logs.cacheCellTitle", {
                    read: exactCount(row.cache_read_tokens, locale),
                    write: exactCount(row.cache_write_tokens, locale),
                  })}
                >
                  {formatTokensLocale(row.cache_read_tokens, locale)} /{" "}
                  {formatTokensLocale(row.cache_write_tokens, locale)}
                </td>
                <td className="db-table__num db-logs__col-cost num">
                  {cost > 0 ? (
                    formatCost(cost, currency)
                  ) : row.total_tokens > 0 ? (
                    <Dash title={t("dashboard.logs.unpricedHint")}>
                      {t("dashboard.logs.unpriced")}
                    </Dash>
                  ) : (
                    <Dash title={t("dashboard.logs.cellNoData")} />
                  )}
                </td>
                {/* 耗时：只显示来源库的真实耗时；缺失（来源不落盘）走灰化「—」 */}
                <td className="db-table__num db-logs__col-duration num">
                  {duration ?? <Dash title={t("dashboard.logs.noDurationTitle")} />}
                </td>
                <td className="db-table__num db-logs__col-ttft num">
                  {ttft ?? <Dash title={t("dashboard.logs.noTtftTitle")} />}
                </td>
                {/* 速度列：口径与详情抽屉一致；无耗时算不出速度，与耗时列
                    同为灰化「—」；≈ 只在「无首字退回总耗时」时出现，
                    title 说明该口径 */}
                <td className="db-table__num db-logs__col-speed num">
                  {speed ? (
                    <span title={speedHintKey ? t(speedHintKey) : undefined}>
                      {speed.approximate ? "≈" : ""}
                      {formatTps(speed.tps)}
                      <span className="db-logs__speed-unit">t/s</span>
                    </span>
                  ) : (
                    <Dash title={t("dashboard.logs.noSpeedTitle")} />
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** 骨架：行数与每页条数一致（数据到达时不出现高度跳变），形状与真实表格同构 */
export function LogTableSkeleton() {
  return (
    <div className="db-table-wrap">
      <table className="db-table db-logs-table">
        <LogTableHead />
        <tbody>
          {Array.from({ length: LOG_PAGE_SIZE }, (_, i) => (
            <tr key={i}>
              {Array.from({ length: 10 }, (_, col) => (
                <td key={col}>
                  <Skeleton
                    className={`h-3 ${
                      col === 2 ? "w-32" : col === 5 ? "w-12" : col >= 3 ? "w-10 ml-auto" : "w-16"
                    }`}
                  />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
