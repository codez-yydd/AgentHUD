/**
 * 请求日志明细表：10 列（时间 / 来源 / 模型 / 输入 / 输出 / 缓存 / 花费 / 耗时 /
 * 首字 / 速度）。缓存读写合并为一列（"12.3万 / 4.5万"，精确值走 title），列宽按
 * 1100 默认窗口的内容区（约 810px）排布——逐列固定宽合计 ≈ 781px（172 + 72 +
 * 75 + 60 + 60 + 92 + 66 + 60 + 58 + 66，逐列见 dashboard.css 的
 * .db-logs__col-*；单元格水平内边距 8px，时间列覆盖「跨月时间 + 最宽有界
 * 徽标」，模型列取 min，富余归它，上限 320）。更窄的窗口或超长
 * 数值（跨年时间、十亿级 token）由 .db-table-wrap 横向滚动兜底，不会把数值
 * 挤成两行。
 *
 * 单元格口径：
 *  - 时间：今天 "HH:mm:ss"，其余 "MM-DD HH:mm"（跨年补年份），完整时间在 title 里
 *    （详情抽屉也有）；时间后按行状态挂小徽标（error 红色显错误码 / 无码「失败」、
 *    cancelled 中性「中断」、成功但重试过的行琥珀色「重试 N」，其余不显示，悬停
 *    给完整口径，各来源读同名字段、规则相同）；来源列显示短名，title 给与下拉
 *    一致的全名；远端行（带 device 字段）在来源下加设备名小徽标（title 给设备名
 *    + 完整 id，本机行不标——默认口径下本机是主体，逐行全标只是噪音）；
 *  - token 用项目现成的 locale 感知紧凑格式化（中文 万/亿、英文 K/M/B），
 *    缓存列两项并列；
 *  - 花费：按展示币种取一列；有 token 但折算为 0 → 「未定价」（与总览页 Agent 表
 *    同一套口径，避免把"没配价"伪装成"不花钱"）；无 token → 「—」；
 *  - 耗时 / 首字：秒一位 / 两位小数；取来源库的真实值（导入器从会话日志取证
 *    回填），请求起点是推断的行（timing_estimated）带前缀 ≈ 并在 title 里说明
 *    近似口径；没有可用计时证据的行灰化「—」，title 按行归属与来源区分原因
 *    （远端行同步明细不含这些列、Claude / Codex 缺的是哪个计时事件）；速度
 *    展示后端校验过的数值及其计时口径，首字估算与请求均速带 ≈；
 *  - 缺失值一律灰化「—」并带 title 说明原因（无数据 / 未定价 / 来源没有可用的
 *    计时证据——属数据源限制，不是加载失败）。
 * 行可点击打开详情抽屉（键盘 Enter / Space 同效）；`busy` 期间旧行整体压暗
 * 且不可点（翻页 / 切筛选的等待期，避免点开已被替换的数据）。
 *
 * 行身份与刷新动画：行的 React key 用后端下发的稳定行键 `row_key`（本机行
 * 「来源|行 id」、远端行「来源|设备|行 id」，同一行刷新前后不变），并在
 * `<tr>` 上输出 `data-row-key` 供动画读取。每次提交后记录各行位置，同一
 * 查询（viewKey = 表内这批行生效时的筛选 + 页码，未变）的第 1 页刷新时：
 * 新出现的行淡入并带单元格底色高亮渐隐，原有行从原位置平移到新位置（像素
 * 差，远端行带设备徽标、行高不等）；首次加载、切筛选、翻页（viewKey 随新
 * 数据一起变化）与取数等待期（busy）不播；reduced-motion 用户只保留底色
 * 高亮；不支持 Web Animations API 的环境整体跳过。
 */
import type { ReactNode } from "react";
import { useLayoutEffect, useRef } from "react";
import { Skeleton } from "../primitives";
import { useI18n } from "../../i18n";
import { formatCost, formatTokensLocale } from "../../format";
import { costOf } from "../overview/derive";
import type { Currency, RequestLogRow } from "../../types";
import {
  LOG_PAGE_SIZE,
  durationText,
  estimatedText,
  exactCount,
  formatLogSpeed,
  logTimeFull,
  logTimeShort,
  noDurationHintKey,
  noTtftHintKey,
  sourceLabel,
  sourceShortLabel,
  speedHintKey,
  speedLabelKey,
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

/**
 * 时间列旁的请求状态徽标：按行状态返回小标签或 null（未知 / 进行中不显示）。
 *  - error：红色，有错误码显码（如 429）、无码显「失败」，悬停说明完整口径
 *    （失败前有过重试的行再带上重试次数）；
 *  - cancelled：中性灰「中断」；
 *  - completed 且 retry_count > 0：琥珀色「重试 N」，悬停给次数与最近错误码。
 * ZCode 等来源的行读同样的字段（后端归一后各来源同名列），规则不区分来源。
 */
function StatusBadge({ row }: { row: RequestLogRow }) {
  const { t } = useI18n();
  if (row.status === "error") {
    const code = row.error_code;
    const n = row.retry_count ?? 0;
    return (
      <span
        className="db-tag db-logs__status db-tag--error"
        title={
          code
            ? n > 0
              ? n === 1
                ? t("dashboard.logs.statusErrorRetryTitleOne", { code, n })
                : t("dashboard.logs.statusErrorRetryTitle", { code, n })
              : t("dashboard.logs.statusErrorTitle", { code })
            : n > 0
              ? n === 1
                ? t("dashboard.logs.statusErrorRetryNoCodeTitleOne", { n })
                : t("dashboard.logs.statusErrorRetryNoCodeTitle", { n })
              : t("dashboard.logs.statusErrorNoCodeTitle")
        }
      >
        {code || t("dashboard.logs.statusFailed")}
      </span>
    );
  }
  if (row.status === "cancelled") {
    return (
      <span
        className="db-tag db-logs__status db-logs__status--fit db-tag--cancelled"
        title={t("dashboard.logs.statusCancelledTitle")}
      >
        {t("dashboard.logs.statusCancelled")}
      </span>
    );
  }
  if (row.status === "completed" && (row.retry_count ?? 0) > 0) {
    const n = row.retry_count ?? 0;
    const code = row.error_code;
    return (
      <span
        className="db-tag db-logs__status db-logs__status--fit db-tag--retry"
        title={
          code
            ? n === 1
              ? t("dashboard.logs.statusRetryTitleOne", { n, code })
              : t("dashboard.logs.statusRetryTitle", { n, code })
            : n === 1
              ? t("dashboard.logs.statusRetryNoCodeTitleOne", { n })
              : t("dashboard.logs.statusRetryNoCodeTitle", { n })
        }
      >
        {t("dashboard.logs.statusRetry", { n })}
      </span>
    );
  }
  return null;
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
  page,
  viewKey,
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
  /** 表内这批行所属的页码（页面传 rowsPage）：刷新行动画只在第 1 页播放
   *  （翻页到达的新页不做进场动画） */
  page: number;
  /** 视图键 = 表内这批行生效时的筛选条件 + 页码（页面传 useLogsData 的
   *  rowsViewKey，它只在取数成功时随 rows 更新）：相邻两次提交键相同 =
   *  同一查询的刷新才播行动画；切筛选 / 翻页的键随新数据一起变化、首载
   *  无上一次状态，一律跳过 */
  viewKey: string;
  /** 设备 id → 展示名（页面统一维护：本机带「（本机）」后缀、远端取清单
   *  设备名、缺名回退 id 前 8 位）；远端行徽标用 */
  deviceLabel: (deviceId: string) => string;
  /** 当前选中的具体设备 id（可选）：整表已筛到该设备时逐行徽标无信息量，
   *  与之相同的行不再渲染设备徽标；undefined = 未筛设备，逐行照常标记 */
  selectedDevice?: string;
  onOpen: (row: RequestLogRow) => void;
}) {
  const { locale, t } = useI18n();

  // ===== 刷新行动画 =====
  // 上一次提交的视图状态：视图键 + 行键集合 + 各行在表体内容里的位置。数据
  // 刷新提交时对比：「本次有、上次没有」= 新行（淡入 + 单元格底色高亮渐隐）；
  // 「两次都在但 top 变了」= 位移行（从原位置平移到新位置）。无论是否播动画
  // 都写回本次状态，作为下一帧的对比基线——同一批行因 busy 等原因重复触发
  // effect 时 keys 已收录全部行，自然不会重播。
  const prevStateRef = useRef<{
    viewKey: string;
    keys: Set<string>;
    tops: Map<string, number>;
  } | null>(null);
  const tableRef = useRef<HTMLTableElement | null>(null);
  const tbodyRef = useRef<HTMLTableSectionElement | null>(null);
  /** 表体滚动容器：行位置改记表体内容坐标时读它的 scrollTop */
  const wrapRef = useRef<HTMLDivElement | null>(null);

  useLayoutEffect(() => {
    const table = tableRef.current;
    const tbody = tbodyRef.current;
    const wrap = wrapRef.current;
    if (!table || !tbody || !wrap) return;
    // 提交后先记录本次的行身份与位置（DOM 已更新、尚未绘制，位置即最终值）
    const trs = Array.from(
      tbody.querySelectorAll<HTMLTableRowElement>("tr[data-row-key]")
    );
    const keys = new Set<string>();
    const tops = new Map<string, number>();
    for (const tr of trs) {
      const key = tr.dataset.rowKey;
      if (!key) continue;
      keys.add(key);
      // 表体内容坐标（视口坐标 + 当前滚动量）：用户在两次刷新之间滚过表体
      // 时视口坐标整体漂移，直接相减会把滚动距离错算成行位移
      tops.set(key, tr.getBoundingClientRect().top + wrap.scrollTop);
    }
    const prev = prevStateRef.current;
    // 播放条件：浏览器支持 Web Animations API、第 1 页、同一视图（viewKey
    // 未变）、不是取数等待期（busy 时表内是旧行，动画等数据到达那次提交再播）
    const canAnimate =
      typeof Element.prototype.animate === "function" &&
      page === 1 &&
      !busy &&
      prev != null &&
      prev.viewKey === viewKey;
    if (canAnimate && prev) {
      // reduced-motion 用户只保留单元格底色高亮（无淡入、无位移）
      const reduceMotion = window.matchMedia(
        "(prefers-reduced-motion: reduce)"
      ).matches;
      // 高亮色从表格元素上读 CSS 变量：暗色值定义在 .dark 上，:root 读不到
      const highlight = getComputedStyle(table)
        .getPropertyValue("--db-row-highlight")
        .trim();
      for (const tr of trs) {
        const key = tr.dataset.rowKey;
        if (!key) continue;
        if (!prev.keys.has(key)) {
          // 新行：整行淡入（延迟 160ms，让位移动画先走）+ 每个单元格底色
          // 高亮后 1.2s 渐隐（inset boxShadow 铺满单元格，fill backwards
          // 让延迟期就保持满格高亮）
          if (!reduceMotion) {
            tr.animate([{ opacity: 0 }, { opacity: 1 }], {
              duration: 240,
              delay: 160,
              easing: "ease-out",
              fill: "backwards",
            });
          }
          if (highlight) {
            for (const td of tr.querySelectorAll<HTMLTableCellElement>("td")) {
              td.animate(
                [
                  { boxShadow: `inset 0 0 0 100vmax ${highlight}` },
                  { boxShadow: "inset 0 0 0 100vmax transparent" },
                ],
                { duration: 1200, delay: 300, easing: "ease-in", fill: "backwards" }
              );
            }
          }
        } else if (!reduceMotion) {
          // 老行：两次都在但位置变了 → 从原位置平移到新位置。按记录的像素
          // 差算（远端行带设备徽标、行高不等，不能按行高倍数位移）；记录的
          // 是表体内容坐标，位移不受两次刷新之间的滚动影响
          const prevTop = prev.tops.get(key);
          const top = tops.get(key);
          if (prevTop != null && top != null && prevTop !== top) {
            const delta = prevTop - top;
            tr.animate(
              [{ transform: `translateY(${delta}px)` }, { transform: "none" }],
              { duration: 320, easing: "cubic-bezier(0.2, 0, 0, 1)" }
            );
          }
        }
      }
    }
    prevStateRef.current = { viewKey, keys, tops };
    // rows 引用变化（新一批数据）、busy / viewKey / page 变化都可能触发本
    // effect；同一 rows 引用重复触发时 keys 已收录全部行，不会重播动画
  }, [rows, viewKey, page, busy]);

  return (
    <div
      ref={wrapRef}
      className={`db-table-wrap${busy ? " db-table-wrap--busy" : ""}`}
      aria-busy={busy || undefined}
    >
      <table ref={tableRef} className="db-table db-logs-table">
        <LogTableHead />
        <tbody ref={tbodyRef}>
          {rows.map((row) => {
            const cost = costOf(row.cost_usd, row.cost_cny, currency);
            const duration = estimatedText(
              durationText(row.duration_ms),
              row.timing_estimated
            );
            const ttft = estimatedText(ttftText(row.ttft_ms), row.timing_estimated);
            const speed = speedOf(row);
            const fullTime = logTimeFull(row.started_at);
            return (
              <tr
                // 后端下发的稳定行键（同一行刷新前后不变）：新行插到顶部时
                // 旧行 key 不变，React 只增删变化的行，刷新动画才有可能
                key={row.row_key}
                data-row-key={row.row_key}
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
                {/* 时间 + 状态徽标：单元格 title 保留完整时间，徽标自带更具体的
                    title（HTML 原生提示取最内层带 title 的元素，悬停徽标时生效）。
                    内容包进 .db-logs__time 弹性容器：时间文字不收缩，错误码徽标
                    在容器内收缩截断，有界徽标（中断 / 重试）完整显示，列宽由
                    .db-logs__col-time 的固定宽度约束 */}
                <td className="db-logs__col-time num" title={fullTime}>
                  <span className="db-logs__time">
                    <span className="db-logs__time-text">
                      {logTimeShort(row.started_at, nowMs)}
                    </span>
                    <StatusBadge row={row} />
                  </span>
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
                {/* 耗时 / 首字：起点是推断的行（timing_estimated）带 ≈ 前缀 +
                    估算口径 title；缺失走灰化「—」，title 按行归属 / 来源区分
                    措辞（远端行 / Claude / Codex / 其它来源各自说明原因） */}
                <td className="db-table__num db-logs__col-duration num">
                  {duration != null ? (
                    <span
                      title={
                        row.timing_estimated
                          ? t("dashboard.logs.timingEstimatedHint")
                          : undefined
                      }
                    >
                      {duration}
                    </span>
                  ) : (
                    <Dash title={t(noDurationHintKey(row))} />
                  )}
                </td>
                <td className="db-table__num db-logs__col-ttft num">
                  {ttft != null ? (
                    <span
                      title={
                        row.timing_estimated
                          ? t("dashboard.logs.timingEstimatedHint")
                          : undefined
                      }
                    >
                      {ttft}
                    </span>
                  ) : (
                    <Dash title={t(noTtftHintKey(row))} />
                  )}
                </td>
                <td className="db-table__num db-logs__col-speed num">
                  {speed ? (
                    <span title={t(speedHintKey(speed.basis, row.source))}>
                      {formatLogSpeed(speed.tps, speed.basis)}
                      <span className="db-logs__speed-unit">t/s</span>
                      <span className="db-logs__speed-basis">
                        {t(speedLabelKey(speed.basis))}
                      </span>
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
