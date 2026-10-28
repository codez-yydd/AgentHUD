/**
 * 活跃热力图卡片（ZCode 风格重做）：最近 52 个自然周（列 = 周、行 = 周一…周日），
 * 网格铺满容器宽度（repeat(52, minmax(0,1fr))，格子 aspect-square 自适应），
 * 有数据的格子带 1px 细边框、无数据格子仅中性底、悬停放大并提亮边框；
 * 底部月份标签行与网格同列对齐（跨度合并、最多 12 个标签）。
 *
 * 三种模式（纯客户端聚合，卡头分段切换，i18n）：
 *  - 每日：逐日格子，色阶按窗口内单日 token 峰值；
 *  - 每周：按周列聚合，渲染为「条形图」——值映射为该列从底部起的填充格数
 *    （filledRows = ceil(value/max×7)，恒以整列 7 格为基准），色阶按周峰值；
 *  - 累计：周值的累计和同法渲染，色阶按末周累计值。
 *    每周 / 累计模式下当前周（已过天数 < 7）也从列底向上填充，未来日作为
 *    普通条格参与填充、不留空，与左侧历史整周方向一致。
 * 列级 tooltip 显示该周 / 截至该周的合计；每日模式仍是逐格 tooltip。
 *
 * 每日模式的当前周：未来日渲染为虚线占位（--future，「这一格还没到」），
 * 今日格子加一圈亮蓝细环（--today）——与无数据灰格（l0）、有数据色块三者
 * 可区分，不再出现「列只有最上面几格、像从顶部断掉」的观感。模式选择写入
 * localStorage（键 zbar-heat-mode）持久化，重启后停留在用户上次选中的模式，
 * 不再回落到默认「每日」而让人误以为修复无效。
 *
 * 数据窗口：本地 SQLite 型来源取满 365 天（铺满 52 周）；Cursor 与其他设备
 * 仍维持 90 天窗口——90 天之前的格子是纯本机口径（脚注注明）。
 * 布局：网格占满整行宽度，三张紧凑统计卡落到下方；≤980px 窄窗下 1fr 自适应
 * 把格子压小但完整显示，不出横向滚动。
 */
import { useState } from "react";
import { formatTokensLocale } from "../../format";
import { useI18n, type MessageKey } from "../../i18n";
import { dateLocale } from "../../i18n/locale";
import { HEAT_LEGEND_LEVELS, dayKeyOf, heatLevel } from "./model";
import type { HeatMode, HeatSeries } from "./derive";
import { Card } from "./primitives";

/** 模式分段（顺序即展示顺序） */
const MODES: { mode: HeatMode; labelKey: MessageKey }[] = [
  { mode: "daily", labelKey: "dashboard.overview.heatMode.daily" },
  { mode: "weekly", labelKey: "dashboard.overview.heatMode.weekly" },
  { mode: "cumulative", labelKey: "dashboard.overview.heatMode.cumulative" },
];

/** 热力图列高（周一…周日固定 7 格）：周 / 累计模式的条形图恒按整列高度计算 */
const HEAT_ROWS = 7;

/** 模式偏好持久化键：不持久化时重启回到默认「每日」，用户切到「每周」看过
 *  效果后会误以为修复无效（与总览页范围偏好 zbar-dashboard-scope 同款做法） */
const HEAT_MODE_KEY = "zbar-heat-mode";

/** 读取模式偏好：无值 / 非法值一律回退 daily（默认逐日口径） */
function loadHeatMode(): HeatMode {
  try {
    const saved = localStorage.getItem(HEAT_MODE_KEY);
    if (saved === "daily" || saved === "weekly" || saved === "cumulative") {
      return saved;
    }
  } catch {
    /* 存储不可用：回退默认模式 */
  }
  return "daily";
}

/** 写入模式偏好（失败静默：记忆仅锦上添花，不影响渲染） */
function saveHeatMode(mode: HeatMode): void {
  try {
    localStorage.setItem(HEAT_MODE_KEY, mode);
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等 */
  }
}

/** 月份短标签（固定用 2000 年构造，只取月份文案，避开跨年边界） */
function monthLabel(month: number, locale: string): string {
  return new Intl.DateTimeFormat(locale === "en" ? "en-US" : "zh-CN", {
    month: "short",
  }).format(new Date(2000, month - 1, 1));
}

/**
 * 周 / 累计模式（条形图）的填充行数：ceil(value / max × 7)，恒以整列 7 格为
 * 基准——当前周已过天数不足 7 时也从列底起填（未来日作为条格参与），
 * 保证条形方向与历史整周一致。max ≤ 0 或 value ≤ 0 时为 0。
 */
function filledRows(value: number, max: number): number {
  if (!(value > 0) || !(max > 0)) return 0;
  return Math.min(HEAT_ROWS, Math.ceil((value / max) * HEAT_ROWS));
}

export function HeatmapCard({ series }: { series: HeatSeries }) {
  const { locale, t } = useI18n();
  // 模式偏好懒初始化（只在挂载时读一次；非法值由 loadHeatMode 兜底为 daily）
  const [mode, setMode] = useState<HeatMode>(loadHeatMode);
  const [hoverKey, setHoverKey] = useState<string | null>(null);
  const [hoverWeek, setHoverWeek] = useState<number | null>(null);
  const intlLocale = dateLocale(locale);

  /** 今日日键（与 model.ts 同源的本地自然日 "MM-DD"）：每日模式据此加高亮环 */
  const todayKey = dayKeyOf(Date.now());

  // 星期行标签：复用 date.weekdays（周日开头），轮转为周一首行
  const weekdayCells = t("date.weekdays").split(" ");
  const weekdays = [...weekdayCells.slice(1), weekdayCells[0]];

  // 当前模式的色阶分母（日 / 周 / 累计三种口径各自独立）
  const max =
    mode === "daily"
      ? series.dayMax
      : mode === "weekly"
        ? series.weekMax
        : series.cumMax;

  /** 悬停浮层横向避让：首两列贴左、末两列贴右，其余居中 */
  const tipEdge = (colIndex: number): string =>
    colIndex < 2
      ? "db-tip--left"
      : colIndex >= series.columns.length - 2
        ? "db-tip--right"
        : "";

  return (
    <Card
      className="db-card--heat"
      title={t("dashboard.overview.heatTitle")}
      sub={t("dashboard.overview.heatSub")}
      frame={{ window: "dashboard", page: "overview", id: "overview.heatmap" }}
      summary={t("customize.summaryHeatmap")}
      bodyClassName="db-card__body--heat"
      action={
        <div className="db-heat__actions">
          {/* 每日 / 每周 / 累计：纯客户端聚合切换，不重新取数 */}
          <div
            className="db-seg"
            role="group"
            aria-label={t("dashboard.overview.heatModeLabel")}
          >
            {MODES.map(({ mode: value, labelKey }) => (
              <button
                key={value}
                type="button"
                onClick={() => {
                  setMode(value);
                  saveHeatMode(value);
                }}
                className={`db-seg__btn ${
                  mode === value ? "db-seg__btn--active" : ""
                }`}
                aria-pressed={mode === value}
              >
                {t(labelKey)}
              </button>
            ))}
          </div>
          <div className="db-heat__legend">
            {/* 「少→多」只放 1..4 四档色阶：0 档（灰格）表示无数据，不是"更少"，
                单独用文字标注在分隔线右侧 */}
            <span>{t("dashboard.overview.heatLess")}</span>
            {HEAT_LEGEND_LEVELS.map((level) => (
              <i key={level} className={`db-heat__cell db-heat__cell--l${level}`} />
            ))}
            <span>{t("dashboard.overview.heatMore")}</span>
            <i className="db-heat__legend-sep" />
            <i className="db-heat__cell db-heat__cell--l0" />
            <span>{t("dashboard.overview.heatNoData")}</span>
          </div>
        </div>
      }
    >
      <div className="db-heat">
        <div className="db-heat__chart">
          <div className="db-heat__body">
            <div className="db-heat__weekdays">
              {weekdays.map((name, i) => (
                <span key={`${name}-${i}`} className="db-heat__weekday">
                  {name}
                </span>
              ))}
            </div>

            <div className="db-heat__grid">
              {series.columns.map((column, colIndex) => {
                const week = series.weeks[colIndex];
                const isHoverColumn = hoverWeek === colIndex;
                return (
                  <div
                    key={colIndex}
                    className="db-heat__col"
                    onMouseEnter={
                      mode === "daily" ? undefined : () => setHoverWeek(colIndex)
                    }
                    onMouseLeave={
                      mode === "daily" ? undefined : () => setHoverWeek(null)
                    }
                  >
                        {mode === "daily"
                          ? column.map((cell, rowIndex) => {
                              if (!cell.key || !cell.day) {
                                // 未来日 / 取数窗口外的空格：虚线占位表示「还没
                                // 到」，与无数据灰格（l0）区分开；不再是全透明
                                // 空白，当前周不会看起来"只有上面几格"
                                return (
                                  <i
                                    key={rowIndex}
                                    className="db-heat__cell db-heat__cell--future"
                                  />
                                );
                              }
                              const level = heatLevel(cell.day.total, series.dayMax);
                              const isHover = hoverKey === cell.key;
                              return (
                                <span
                                  key={rowIndex}
                                  className="db-heat__cell-wrap"
                                  onMouseEnter={() => setHoverKey(cell.key)}
                                  onMouseLeave={() => setHoverKey(null)}
                                >
                              {isHover && (
                                // 上半行向下弹、下半行向上弹：两个方向都不会压住
                                // 卡片头（下方是月份标签行，浮层落在卡片体内）
                                <span
                                  className={`db-tip ${
                                    rowIndex < 4 ? "db-tip--down" : "db-tip--up"
                                  } ${tipEdge(colIndex)}`}
                                >
                                  <span className="db-tip__title num">
                                    {cell.key}
                                  </span>
                                  <span className="db-tip__row">
                                    <span className="db-tip__label">
                                      {t("dashboard.overview.ttTotal")}
                                    </span>
                                    <span className="num">
                                      {formatTokensLocale(cell.day.total, locale)}
                                    </span>
                                  </span>
                                  <span className="db-tip__row">
                                    <span className="db-tip__label">
                                      {t("dashboard.overview.ttRequests")}
                                    </span>
                                    <span className="num">
                                      {cell.day.requests.toLocaleString(intlLocale)}
                                    </span>
                                  </span>
                                </span>
                              )}
                              <i
                                className={`db-heat__cell db-heat__cell--l${level}${
                                  cell.key === todayKey
                                    ? " db-heat__cell--today"
                                    : ""
                                }`}
                              />
                            </span>
                          );
                        })
                      : column.map((_cell, rowIndex) => {
                          // 周 / 累计模式按「条形图」语义渲染：锚定整列 7 格
                          // 底部向上填充 filled 格（ceil(value/max×7)，恒按
                          // 整列 7 格计算）。当前周只有已过天数时同样从列底
                          // 长起，未来日不再单独留空（--future 只用于每日模式），
                          // 而是作为普通条格参与填充——方向与左侧历史整周一致；未填充格为
                          // 中性底（l0），色阶按整周 / 累计值定档。
                          const value =
                            mode === "weekly" ? week.total : week.cumTotal;
                          const filled = filledRows(value, max);
                          const level = heatLevel(value, max);
                          return (
                            <i
                              key={rowIndex}
                              className={`db-heat__cell ${
                                rowIndex >= HEAT_ROWS - filled
                                  ? `db-heat__cell--l${level}`
                                  : "db-heat__cell--l0"
                              }`}
                            />
                          );
                        })}

                    {mode !== "daily" && isHoverColumn && (
                      // 列级 tooltip：日期标题 + 主体量 + 请求数。
                      // 累计模式把「截至本周累计」提到第一行（保留 --strong 强调样式）：
                      // 列高按累计口径渲染，第一眼读数应与之一致，避免把周量误当累计值；
                      // 周量「本周合计」降为第二行普通样式。每周模式维持「本周合计」
                      // 第一行的原顺序。请求数恒为最后一行。
                      <span
                        className={`db-tip db-tip--down ${tipEdge(colIndex)}`}
                      >
                        <span className="db-tip__title num">
                          {week.startKey ?? "--"} ~ {week.endKey ?? "--"}
                        </span>
                        {mode === "cumulative" && (
                          <span className="db-tip__row db-tip__row--strong">
                            <span className="db-tip__label">
                              {t("dashboard.overview.heatCumTotal")}
                            </span>
                            <span className="num">
                              {formatTokensLocale(week.cumTotal, locale)}
                            </span>
                          </span>
                        )}
                        <span className="db-tip__row">
                          <span className="db-tip__label">
                            {t("dashboard.overview.heatWeekTotal")}
                          </span>
                          <span className="num">
                            {formatTokensLocale(week.total, locale)}
                          </span>
                        </span>
                        <span className="db-tip__row">
                          <span className="db-tip__label">
                            {t("dashboard.overview.ttRequests")}
                          </span>
                          <span className="num">
                            {(mode === "cumulative"
                              ? week.cumRequests
                              : week.requests
                            ).toLocaleString(intlLocale)}
                          </span>
                        </span>
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          </div>

          {/* 底部月份标签行：与 52 列同网格，按跨度合并（最多 12 个标签） */}
          <div className="db-heat__monthrow">
            <div className="db-heat__monthrow-spacer" />
            <div className="db-heat__months">
              {series.monthSpans.map((span) => (
                <span
                  key={`${span.month}-${span.start}`}
                  className="db-heat__month"
                  style={{ gridColumn: `${span.start + 1} / span ${span.span}` }}
                >
                  {monthLabel(span.month, locale)}
                </span>
              ))}
            </div>
          </div>
        </div>

        <div className="db-heat__aside">
          <HeatStat
            label={t("dashboard.overview.heatTotal")}
            value={formatTokensLocale(series.totalTokens, locale)}
          />
          <HeatStat
            label={t("dashboard.overview.heatActiveDays")}
            value={String(series.activeDays)}
          />
          <HeatStat
            label={t("dashboard.overview.heatActiveAvg")}
            value={formatTokensLocale(Math.round(series.activeDailyAvg), locale)}
          />
        </div>

        {/* 口径脚注：90 天之后只有本地 SQLite 型来源，Cursor 与其他设备不覆盖 */}
        <p className="db-heat__foot">{t("dashboard.overview.heatFootnote")}</p>
      </div>
    </Card>
  );
}

function HeatStat({ label, value }: { label: string; value: string }) {
  return (
    <div className="db-heat__stat">
      <div className="db-heat__stat-label">{label}</div>
      <div className="db-heat__stat-value num">{value}</div>
    </div>
  );
}
