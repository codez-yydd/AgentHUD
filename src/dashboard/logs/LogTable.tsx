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
 *    （详情抽屉也有）；来源列显示短名，title 给与下拉一致的全名；远端行
 *    （带 device 字段）在来源下加设备名小徽标（title 给设备名 + 完整 id，
 *    本机行不标——默认口径下本机是主体，逐行全标只是噪音）；
 *  - token 用项目现成的 locale 感知紧凑格式化（中文 万/亿、英文 K/M/B），
 *    缓存列两项并列；
 *  - 花费：按展示币种取一列；有 token 但折算为 0 → 「未定价」（与总览页 Agent 表
 *    同一套口径，避免把"没配价"伪装成"不花钱"）；无 token → 「—」；
 *  - 耗时 / 首字：秒一位 / 两位小数；耗时只认来源库的真实值（Codex 会话文件无
 *    耗时、旧版 Claude Code 不落盘耗时 → 灰化「—」；远端行（带 device 字段）
 *    的同步明细不含耗时 / 首字，同样灰化并给专属 title）；速度口径与详情抽屉
 *    完全一致（model.speedOf + formatTps 两层结构：后端 token_speed 真源优先——
 *    含思考感知窗口与样本窗口边界；后端缺速度时回退本地「耗时 − 首字」公式，
 *    近似值标 ≈，title 说明原因）；
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
  deviceLabel,
  selectedDevice,
  onOpen,
}: {
  rows: RequestLogRow[];
  /** 当前时刻（时间列「今天」判定的基准；页面每次取数后更新，保证跨日渲染正确） */
  nowMs: number;
  currency: Currency;
  /** 正在取数且表内仍是旧行：压暗 + aria-busy + 不可点开详情 */
  busy: boolean;
  /** 设备 id → 展示名（页面统一维护：本机带「（本机）」后缀、远端取清单
   *  设备名、缺名回退 id 前 8 位）；远端行徽标用 */
  deviceLabel: (deviceId: string) => string;
  /** 当前选中的具体设备 id（可选）：整表已筛到该设备时逐行徽标无信息量，
   *  与之相同的行不再渲染设备徽标；undefined = 未筛设备，逐行照常标记 */
  selectedDevice?: string;
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
                    ZCode / DSH，hover 可对上；远端行（带 device）在来源下加
                    设备名小徽标，title 给「设备名 + 完整 id」；本机行不标，
                    已筛到该设备（selectedDevice）的行同样不标——整表同一
                    设备时逐行徽标只是噪音 */}
                <td className="db-logs__col-source" title={sourceLabel(row.source)}>
                  {sourceShortLabel(row.source)}
                  {row.device != null &&
                    row.device !== "" &&
                    row.device !== selectedDevice && (
                    <span
                      className="db-logs__device"
                      title={t("common.deviceOption", {
                        name: deviceLabel(row.device),
                        id: row.device,
                      })}
                    >
                      {deviceLabel(row.device)}
                    </span>
                  )}
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
                {/* 耗时：只显示来源库的真实耗时；缺失（来源不落盘 / 远端行
                    同步明细不含耗时）走灰化「—」，title 按行归属区分措辞 */}
                <td className="db-table__num db-logs__col-duration num">
                  {duration ?? (
                    <Dash
                      title={t(
                        row.device != null
                          ? "dashboard.logs.noDurationRemoteTitle"
                          : "dashboard.logs.noDurationTitle"
                      )}
                    />
                  )}
                </td>
                <td className="db-table__num db-logs__col-ttft num">
                  {ttft ?? (
                    <Dash
                      title={t(
                        row.device != null
                          ? "dashboard.logs.noTtftRemoteTitle"
                          : "dashboard.logs.noTtftTitle"
                      )}
                    />
                  )}
                </td>
                {/* 速度列：口径与详情抽屉一致（speedOf 两层结构：后端
                    token_speed 真源优先、本地公式回退）；无耗时算不出速度，
                    与耗时列同为灰化「—」；≈ 在后端 request_average（缺事件
                    时刻按总耗时近似）或本地回退无首字时出现，title 说明该口径 */}
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
