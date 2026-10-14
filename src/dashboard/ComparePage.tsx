/**
 * 周额度对比页（主窗口大窗版，300px 窄窗 ComparePanel 的大窗形态）。
 *
 * 结构：页头（设备筛选 + 时间范围 7/30/90 天 + 刷新）→ 额度矩阵（行 =
 * Agent、行内每系列一块：周额度剩余 / 重置时刻 / 5 小时窗口）→ 额度趋势
 * （多序列 SVG 折线，图例可开关）→ Token 对比（本机 + 其他设备分段）。
 *
 * 数据口径与窄窗完全一致（compare/model 是窄窗逻辑的复制改造）：额度系列
 * 按 Z.ai 账号指纹拆分、其余 Agent 各一条；本机与远端快照按「时间 + 账号
 * 指纹」去重取较高值；Token 按各 Agent 实际用量归集。窄窗原件不动。
 *
 * 加载形态：分级渐进——本机额度快照就绪即渲染矩阵（本机部分），远端快照 /
 * Token 到达后补全；Token 对比卡的远端部分按当前范围懒取。三张卡各自带
 * 骨架，页面不再整页转圈（见 useCompareData 头部注释）。
 */
import { useMemo, useState } from "react";
import { useI18n, type MessageKey } from "../i18n";
import { AlertBanner, EmptyState } from "../layout";
// 下拉组件已提升为两窗口共用（src/DropdownMenu.tsx，Tailwind 工具类自适应亮暗）
import { DropdownMenu } from "../DropdownMenu";
import { RefreshIcon } from "./icons";
import { useDarkTheme } from "./primitives";
import { QuotaMatrixCard } from "./compare/QuotaMatrixCard";
import { QuotaTrendCard, type TrendSeriesView } from "./compare/QuotaTrendCard";
import { TokensCompareCard } from "./compare/TokensCompareCard";
import {
  buildQuotaMatrix,
  rangeStartMs,
  seriesColorOf,
  type CompareRange,
} from "./compare/model";
import { useCompareData } from "./compare/useCompareData";

/** 页头范围分段（顺序即展示顺序；默认近 30 天，兼顾趋势可读性与跨度） */
const RANGES: { range: CompareRange; labelKey: MessageKey }[] = [
  { range: "7d", labelKey: "dashboard.overview.range.7d" },
  { range: "30d", labelKey: "dashboard.overview.range.30d" },
  { range: "90d", labelKey: "dashboard.overview.range.90d" },
];

/** 刷新时刻（本地时区 HH:mm:ss） */
function timeText(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export function ComparePage() {
  const { t } = useI18n();
  const [range, setRange] = useState<CompareRange>("30d");
  const {
    data,
    loading,
    quotaPending,
    tokensPending,
    remotePending,
    error,
    remoteDegraded,
    lastUpdatedAt,
    syncConfig,
    devices,
    deviceFilter,
    setDeviceFilter,
    reload,
  } = useCompareData(range);

  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;
  // 设备筛选项：全部 + 本机（带设备名）+ 其他设备（名称 + 指纹前 6 位）。
  // 文案沿用原原生 select 的 i18n 生成逻辑，仅换承载组件（同步未启用时
  // 整个筛选不渲染，与改动前一致）
  const deviceOptions = useMemo(
    () => [
      { value: "all", label: t("dashboard.compare.deviceAll") },
      {
        value: "local",
        label: syncConfig?.device_name
          ? t("stats.deviceLocalName", { name: syncConfig.device_name })
          : t("stats.deviceLocal"),
      },
      ...devices
        .filter((device) => device.device_id !== syncConfig?.device_id)
        .map((device) => ({
          value: device.device_id,
          label: t("common.deviceOption", {
            name: device.device_name,
            id: device.device_id.slice(0, 6),
          }),
        })),
    ],
    [devices, syncConfig, t]
  );
  // 页头范围在额度时间线上的切片起点（本地今日 0 点往前推 N-1 天）
  const fromMs = useMemo(() => rangeStartMs(range), [range]);
  // 暗色主题下系列色走提亮色阶（深档色在 slate-900 底上几乎不可见，见 model.ts）
  const dark = useDarkTheme();

  // 额度矩阵：口径与窄窗一致的系列合并结果（每系列取最近一次采样）。
  // 本机快照就绪即有值，远端快照到达后由 hook 二次发布补全。
  const matrix = useMemo(
    () => (data ? buildQuotaMatrix(data.timelines, dark) : []),
    [data, dark]
  );

  // Token 对比：本机部分取数一次拿全，远端按当前范围懒取（未取过的范围
  // 首次切换时才请求，切回已有范围不重取）
  const tokens = data?.tokensByRange[range] ?? [];

  // 趋势序列：按页头范围切片采样点后交给折线图
  const trend: TrendSeriesView[] = useMemo(() => {
    if (!data) return [];
    const seriesList = data.timelines.map((item) => item.series);
    const nowMs = Date.now();
    return data.timelines.map((timeline) => ({
      id: timeline.series.id,
      label: timeline.series.label,
      color: seriesColorOf(seriesList, timeline.series.id, dark),
      points: timeline.samples.filter(
        (sample) => sample.ts >= fromMs && sample.ts <= nowMs
      ),
    }));
  }, [data, fromMs, dark]);

  const rangeLabel = t(RANGES.find((item) => item.range === range)!.labelKey);
  const hasRemote = syncEnabled && deviceFilter !== "local";
  // 空态：额度与 Token 都没有可展示的数据（未启用任何 Agent / 从未采样）——
  // 必须在三张卡都就绪（非骨架态）之后才判定，否则会把「还在加载」说成空
  const isEmpty =
    data != null &&
    !quotaPending &&
    !tokensPending &&
    matrix.length === 0 &&
    tokens.length === 0;

  return (
    <div className="db-page">
      <header className="db-page__header">
        <div>
          <h1 className="db-page__title">{t("dashboard.compare")}</h1>
          <p className="db-page__desc">{t("dashboard.compare.desc")}</p>
        </div>
        <div className="db-page__actions">
          {syncEnabled && (
            <DropdownMenu
              label={t("dashboard.compare.device")}
              ariaLabel={t("dashboard.compare.device")}
              options={deviceOptions}
              value={deviceFilter}
              onChange={setDeviceFilter}
            />
          )}
          <div
            className="db-seg"
            role="group"
            aria-label={t("dashboard.compare.rangeLabel")}
          >
            {RANGES.map(({ range: value, labelKey }) => (
              <button
                key={value}
                type="button"
                onClick={() => setRange(value)}
                className={`db-seg__btn ${
                  range === value ? "db-seg__btn--active" : ""
                }`}
                aria-pressed={range === value}
              >
                {t(labelKey)}
              </button>
            ))}
          </div>
          <button
            type="button"
            className="db-btn"
            onClick={reload}
            disabled={loading}
          >
            <RefreshIcon className={`h-3.5 w-3.5 ${loading ? "db-spin" : ""}`} />
            {t("common.refresh")}
          </button>
          {/* 远端合并中：本机额度已可见，其他设备数据仍在后台合并（弱标记） */}
          {hasRemote && remotePending && (
            <span className="db-page__merging">
              {t("dashboard.overview.remoteMerging")}
            </span>
          )}
          <span className="db-page__updated num">
            {lastUpdatedAt != null
              ? t("dashboard.overview.lastUpdated", {
                  time: timeText(lastUpdatedAt),
                })
              : ""}
          </span>
        </div>
      </header>

      {error && (
        <AlertBanner type="error">
          {t("dashboard.compare.errorBanner", { error })}
        </AlertBanner>
      )}
      {/* 汇总模式下远端历史失败：未清数据、只是降级为仅本机口径，
          用弱提示条说明（比 AlertBanner 轻，不抢主数据） */}
      {remoteDegraded && !error && (
        <div className="db-hintbar">{t("dashboard.compare.remoteDegraded")}</div>
      )}

      {data == null && !loading ? (
        /* 首载即失败（本机额度历史读不到）：给出重试入口 */
        <EmptyState
          title={t("dashboard.compare.emptyTitle")}
          hint={t("dashboard.compare.emptyHint")}
          action={
            <button type="button" className="db-btn mt-1" onClick={reload}>
              {t("dashboard.compare.retry")}
            </button>
          }
        />
      ) : isEmpty ? (
        <EmptyState
          title={t("dashboard.compare.emptyTitle")}
          hint={t("dashboard.compare.emptyHint")}
          action={
            <button type="button" className="db-btn mt-1" onClick={reload}>
              {t("dashboard.compare.retry")}
            </button>
          }
        />
      ) : (
        <>
          {/* 三张卡各自带骨架：本地快照 / 远端补全 / 当前范围 Token 分级就绪 */}
          <QuotaMatrixCard rows={matrix} pending={quotaPending} />
          <QuotaTrendCard
            series={trend}
            fromMs={fromMs}
            toMs={Date.now()}
            rangeLabel={rangeLabel}
            pending={quotaPending}
          />
          <TokensCompareCard
            rows={tokens}
            rangeLabel={rangeLabel}
            hasRemote={hasRemote}
            pending={tokensPending}
          />
          {/* 口径脚注：额度百分比与 Token 实量是两个指标，避免误读为同一数据 */}
          <p className="db-page__footnote">
            {t("dashboard.compare.footnote")}
          </p>
        </>
      )}
    </div>
  );
}
