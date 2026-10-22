/**
 * 额度矩阵卡（大窗版独有）：全部「额度系列」平铺进一个网格——
 * Z.ai 每账号一块、其余 Agent 各一块，不再按 Agent 分行（行头只会吃掉
 * 纵向空间，固定 2 列还会让奇数订阅空一格）。块头左侧是所属 Agent 的
 * BrandIcon（染系列色，与趋势卡图例同色板，行信息在组件层并入块参数）+
 * 系列名，右侧是剩余大数（仅账单周期窗口额外带「账单周期」小字——卡片
 * 标题已是「周额度剩余」，周窗口不再重复措辞）。每块展示：周额度剩余 %（进度条 +
 * 剩余色阶，与总览页额度列同色板）、重置时刻（↻，带「下次重置」标注）、
 * 5 小时窗口（快照有该窗口才显示）。
 * 最近一次采样时刻写在卡片副标题里（原先是额度块上的原生 title 悬停，
 * 悬停前看不到）；各块自身取的都是该时刻之前各自最新的采样。
 *
 * 列数不走 CSS auto-fill：列数与订阅数不整除时末行会留出大片空位
 * （如 6 块落进 5 列 = 5+1），改由 ResizeObserver 测容器宽后按
 * 「可容纳范围内最大的整除列数」以内联模板接管（见 pickColumnCount）。
 *
 * 数值来自 buildQuotaMatrix（模型层），口径与窄窗 ComparePanel 的系列
 * 合并规则完全一致：本机 + 远端快照按「时间 + 账号指纹」去重取较高值。
 */
import { useEffect, useRef, useState } from "react";
import { BrandIcon } from "../../BrandIcon";
import type { BrandIconName } from "../../BrandIcon";
import { formatResetStamp } from "../../format";
import { useI18n } from "../../i18n";
import { remainingGradient, remainingTextColor } from "../../widgets";
import { Card, Skeleton } from "../primitives";
import type { MatrixCell, MatrixRow } from "./model";

/** 网格列选择参数：块间距（与 .db-matrix 的 gap 一致）、单块最小宽
 *  （再窄块头挤不下）、单块最大宽（再宽视觉松散、进度条失去刻度感） */
const GRID_GAP = 12;
const MIN_BLOCK = 190;
const HARD_MAX = 520;

/**
 * 按「订阅数整除」优选列数：在宽度可容纳的列数里，从最多列往下找第一个
 * 能整除订阅数、且单块不超 HARD_MAX 的列数（6 块取 3 列而非 5+1）；
 * 搜不到整除（质数块数等）回落 min(可容纳列数, 订阅数)——末行少量空位
 * 可接受，块保持紧凑。
 */
function pickColumnCount(count: number, width: number): number {
  const maxFit = Math.max(
    1,
    Math.floor((width + GRID_GAP) / (MIN_BLOCK + GRID_GAP))
  );
  const cap = Math.min(maxFit, count);
  for (let cols = cap; cols >= 1; cols--) {
    if (
      count % cols === 0 &&
      (width - (cols - 1) * GRID_GAP) / cols <= HARD_MAX
    ) {
      return cols;
    }
  }
  return cap;
}

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

  // 订阅块总数与容器内容宽 → 整除优选列数。未测得宽（首帧）时为 0，
  // 网格沿用 CSS 兜底模板，测得后切换内联模板；容器是块级拉伸，改列数
  // 不会反过来改变容器宽，无测量回路
  const cellCount = rows.reduce((sum, row) => sum + row.cells.length, 0);
  const gridRef = useRef<HTMLDivElement | null>(null);
  const [gridWidth, setGridWidth] = useState(0);

  // 挂载 / 骨架退场（pending→数据）/ 订阅增删都会重建 .db-matrix，重挂
  // 观察器；窗口宽度变化由 ResizeObserver 持续跟进，卸载时断开
  useEffect(() => {
    const el = gridRef.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width ?? 0;
      if (width > 0) setGridWidth(width);
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [pending, rows.length]);

  const columnCount = gridWidth > 0 ? pickColumnCount(cellCount, gridWidth) : 0;

  return (
    <Card title={t("dashboard.compare.matrixTitle")} sub={sub}>
      {pending && rows.length === 0 ? (
        // 本机额度快照未就绪：骨架块复用矩阵网格模板（同列宽同块高），
        // 加载完成后布局不跳变（页面其余卡片照常渲染）
        <div className="db-matrix">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="db-qcell--skeleton" />
          ))}
        </div>
      ) : rows.length === 0 ? (
        <div className="db-empty">{t("dashboard.compare.matrixEmpty")}</div>
      ) : (
        // 行信息（brand）只在这里展开；系列色由模型层的 cell.color 给出。
        // 测得宽后以内联「整除优选」模板接管（未测量时走 CSS 兜底模板）；
        // 仅一个订阅时加 --solo 封顶块宽，进度条不拉满整行
        <div
          className={
            cellCount === 1 ? "db-matrix db-matrix--solo" : "db-matrix"
          }
          ref={gridRef}
          style={
            columnCount > 0
              ? {
                  gridTemplateColumns: `repeat(${columnCount}, minmax(0, 1fr))`,
                }
              : undefined
          }
        >
          {rows.map((row) =>
            row.cells.map((cell) => (
              <QuotaCellView key={cell.id} cell={cell} brand={row.brand} />
            ))
          )}
        </div>
      )}
    </Card>
  );
}

function QuotaCellView({
  cell,
  brand,
}: {
  cell: MatrixCell;
  /** 所属 Agent 的品牌图标（染 cell.color 系列色，与趋势图例逐块对应） */
  brand: BrandIconName;
}) {
  const { t } = useI18n();
  // 重置时刻小于当前时刻说明已过期（快照滞后），不展示倒计时语义的时间点
  const showReset = cell.resetAt != null && cell.resetAt > Date.now();
  const remaining = Math.min(Math.max(cell.remainingPct, 0), 100);

  return (
    <div className="db-qcell">
      <div className="db-qcell__head">
        {/* 图标染系列色而非 Agent 行色：暗色下取提亮色阶，与下方趋势卡
            图例同色，zai 多账号块还能与折线逐块对应 */}
        <span className="db-agent__icon" style={{ color: cell.color }}>
          <BrandIcon brand={brand} className="h-4 w-4" />
        </span>
        {/* 恒设全名悬停：长名截断后仍有查看途径 */}
        <span className="db-qcell__label" title={cell.label}>
          {cell.label}
        </span>
        {/* 周窗口不再渲染「周剩」小字（卡片标题已表意）；出现小字即非周
            窗口（Cursor 账单周期读作「账单周期」，与总览页词条对齐） */}
        <span className="db-qcell__value">
          {cell.windowKind === "billing" && (
            <span className="db-qcell__kind">
              {t("dashboard.overview.billingCycle")}
            </span>
          )}
          <span
            className="db-qcell__pct num"
            style={{ color: remainingTextColor(remaining) }}
          >
            {Math.round(remaining)}%
          </span>
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
