import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ModelSpeedStat } from "./types";
import { getModelSpeed } from "./api";
import { formatSeconds, formatTokens, formatTps } from "./format";
import { displayModelName } from "./modelName";
import { useI18n } from "./i18n";
import type { MessageKey } from "./i18n";
import { sourceMeta } from "./agentVisibility";
import { BrandIcon } from "./BrandIcon";
import type { BrandIconName } from "./BrandIcon";

/** 时间范围（今日 = 自然本地日；后端 days=0 使用同一口径） */
type SpeedRange = "today" | "7d";

/** 数据口径（与后端 scope 参数一致：local = 仅本机；all = 全部设备，并入
 *  同步服务端上其他设备的明细——远端行只进近似口径样本池，见后端模块头） */
type SpeedScope = "local" | "all";

const SPEED_RANGE_KEY = "zbar-speed-range";
/** 数据口径偏好 key（默认本机——全部设备依赖同步已启用） */
const SPEED_SCOPE_KEY = "zbar-speed-scope";

/** 后端来源名 → 品牌图标（速度页需要的正向映射；label 与品牌色统一走
 *  agentVisibility 的 sourceMeta——那里同时登记了 opencode / minimaxcode
 *  两个后端异名，与项目页会话来源、报告图例同一份口径。compare/model.ts
 *  的 AGENT_META 是反向映射且无 brand 字段，不改它的公共结构） */
const SOURCE_BRAND: Partial<Record<string, BrandIconName>> = {
  zcode: "zai",
  codex: "codex",
  claude: "claude",
  kimi: "kimi",
  dsh: "dsh",
  opencode: "opencodego",
  minimaxcode: "minimax",
};

function loadSpeedRange(): SpeedRange {
  try {
    const saved = localStorage.getItem(SPEED_RANGE_KEY);
    if (saved === "today" || saved === "7d") return saved;
  } catch {
    // 偏好只是锦上添花，存储不可用时使用默认范围。
  }
  return "7d";
}

function loadSpeedScope(): SpeedScope {
  try {
    const saved = localStorage.getItem(SPEED_SCOPE_KEY);
    if (saved === "local" || saved === "all") return saved;
  } catch {
    // 同 range 偏好：存储不可用时使用默认口径（本机）。
  }
  return "local";
}

function sortByP50(rows: ModelSpeedStat[]): ModelSpeedStat[] {
  return [...rows].sort((a, b) => {
    if (a.p50Tps == null && b.p50Tps == null) {
      return b.requests - a.requests || a.modelId.localeCompare(b.modelId);
    }
    if (a.p50Tps == null) return 1;
    if (b.p50Tps == null) return -1;
    return b.p50Tps - a.p50Tps || b.requests - a.requests;
  });
}

function tps(value: number | null): string {
  return value == null ? "—" : formatTps(value);
}

function seconds(value: number | null): string {
  return value == null ? "—" : formatSeconds(value);
}

function percent(value: number | null): string {
  return value == null ? "—" : `${Math.round(value * 100)}%`;
}

function speedBarColor(value: number | null): string {
  if (value == null) return "bg-slate-400/60";
  if (value >= 80) return "bg-emerald-500";
  if (value >= 40) return "bg-amber-500";
  return "bg-rose-500";
}

function SpeedGlyph() {
  return (
    <span className="speed-glyph" aria-hidden="true">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
        <path strokeLinecap="round" strokeLinejoin="round" d="M4 17.5 9.2 12l3.2 3.1L20 7.5" />
        <path strokeLinecap="round" d="M15.5 7.5H20v4.5" />
      </svg>
    </span>
  );
}

function RefreshGlyph() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
      <path strokeLinecap="round" strokeLinejoin="round" d="M20 11a8 8 0 0 0-14.7-3.9L4 9" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M4 5v4h4" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M4 13a8 8 0 0 0 14.7 3.9L20 15" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M20 19v-4h-4" />
    </svg>
  );
}

function OverviewMetric({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="speed-overview-metric" title={title}>
      <div className="speed-overview-label">{label}</div>
      <div className="speed-overview-value num">{value}</div>
    </div>
  );
}

function MiniMetric({
  label,
  value,
  primary = false,
  unit,
  title,
}: {
  label: string;
  value: string;
  primary?: boolean;
  unit?: string;
  /** 数值口径说明（近似速度的 ≈ 提示等），悬停可见 */
  title?: string;
}) {
  return (
    <div className={`speed-mini-metric ${primary ? "speed-mini-primary" : ""}`}>
      <div className="speed-mini-label">{label}</div>
      <div className="speed-mini-value-row">
        <div className="speed-mini-value num" title={title}>
          {value}
        </div>
        {unit && <span className="speed-mini-unit">{unit}</span>}
      </div>
    </div>
  );
}

function DetailChip({ label, value, warn = false }: { label: string; value: string; warn?: boolean }) {
  return (
    <div className={`speed-detail-chip ${warn ? "speed-detail-warn" : ""}`}>
      <span className="speed-detail-label">{label}</span>
      <span className="speed-detail-value num">{value}</span>
    </div>
  );
}

function ModelSpeedRow({
  model,
  rank,
  maxP50,
}: {
  model: ModelSpeedStat;
  rank: number;
  maxP50: number;
}) {
  const { t } = useI18n();
  // 近似口径（导入库 request_average：按请求总耗时折算、含首字延迟）：
  // ≈ 标记放在 label 侧（"≈P50"/"≈慢" 等，label 文本小放得下）而不是数值
  // 本体——260px 窄窗的 MiniMetric 每格 value 仅约 20px，"≈53.4" 会被截成
  // "≈5…"（对齐日志页 LogTable 的 ≈ 约定，但位置随窄窗形态挪到 label）；
  // title 挂口径说明；generation（zcode 精确）与 null（无样本 → —）不加
  const approximate = model.speedQuality === "request_average";
  const approxTitle = approximate ? t("speed.approxHint") : undefined;
  const speedLabel = (key: MessageKey) => (approximate ? `≈${t(key)}` : t(key));
  const primarySpeed = model.p50Tps ?? model.avgTps;
  const ratio = model.p50Tps != null && maxP50 > 0 ? model.p50Tps / maxP50 : 0;
  const lowSuccess = model.successRate != null && model.successRate < 0.95;
  // 模型名旁标注速度合格样本数（≠总请求数：无可信生成区间的请求不产生
  // 速度样本）；总请求数在明细区「请求」格展示
  const sampleText =
    model.speedSampleCount == null
      ? t("speed.samplesNone")
      : t("speed.speedSamples", { n: formatTokens(model.speedSampleCount) });
  // 来源徽标 + 渠道名：300px 窄窗两行头部密度受限——徽标 10px 小图标（品牌
  // 色取 agentVisibility 的 AGENT_COLOR，与项目页/报告图例一致）；收缩优先
  // 级：来源名近完整 > 渠道 chip 保底 3~4 字符（min-width 兜底）> 样本数可
  // 截断（明细区「请求」格有兜底）；渠道为空时只显示来源名
  const brand = SOURCE_BRAND[model.source];
  const sourceLabel = brand ? sourceMeta(model.source).label : t("speed.sourceUnknown");
  const sourceTitle = model.provider ? `${sourceLabel} · ${model.provider}` : sourceLabel;

  return (
    <article className={`speed-model-card ${rank === 0 ? "speed-model-featured" : ""}`}>
      <div className="flex items-start gap-2.5 min-w-0">
        <span className={`speed-rank ${rank === 0 ? "speed-rank-first" : ""}`}>{rank + 1}</span>
        <div className="min-w-0 flex-1 pt-px">
          <div className="flex items-center gap-1.5 min-w-0">
            <span className="speed-model-name truncate" title={model.modelId}>
              {displayModelName(model.modelId)}
            </span>
            {lowSuccess && <span className="speed-warning-dot" />}
          </div>
          <div className="flex items-center gap-1.5 mt-1 min-w-0">
            <span className="speed-source" title={sourceTitle}>
              {brand && (
                <BrandIcon
                  brand={brand}
                  className="speed-source-icon"
                  style={{ color: sourceMeta(model.source).color }}
                />
              )}
              <span className="speed-source-name">{sourceLabel}</span>
            </span>
            {model.provider && (
              <span className="speed-provider" title={model.provider}>
                {model.provider}
              </span>
            )}
            {/* 样本数不带 shrink-0：260-300px 最坏情况下它是第二行首个可
                牺牲项（明细区「请求」格有兜底），收缩让位给来源名与渠道
                chip 的渠道区分信息；title 给全量 */}
            <span className="speed-sample-count" title={sampleText}>
              {sampleText}
            </span>
          </div>
        </div>
        <div className="shrink-0 text-right">
          <div className="flex items-baseline justify-end gap-0.5 whitespace-nowrap">
            <span className="speed-primary-value num" title={approxTitle}>
              {tps(primarySpeed)}
            </span>
            <span className="speed-primary-unit">t/s</span>
          </div>
          <div className="speed-primary-label">{speedLabel("speed.p50")}</div>
        </div>
      </div>

      <div className="speed-bar-wrap" aria-label={t("speed.relativeSpeed", { pct: Math.round(ratio * 100) })}>
        <div className="speed-bar-meta">
          <span>{t("speed.relativeLabel")}</span>
          <span className="speed-bar-label num">{Math.round(ratio * 100)}%</span>
        </div>
        <div className="speed-bar-track">
          <div className={`speed-bar-fill ${speedBarColor(model.p50Tps)}`} style={{ width: `${Math.min(Math.max(ratio * 100, 0), 100)}%` }} />
        </div>
      </div>

      <div className="grid grid-cols-4 gap-1.5 mt-2.5">
        <MiniMetric label={speedLabel("speed.p10")} value={tps(model.p10Tps)} unit="t/s" title={approxTitle} />
        <MiniMetric label={speedLabel("speed.p50")} value={tps(model.p50Tps)} primary unit="t/s" title={approxTitle} />
        <MiniMetric label={speedLabel("speed.p90")} value={tps(model.p90Tps)} unit="t/s" title={approxTitle} />
        <MiniMetric label={t("speed.ttft")} value={seconds(model.ttftAvgMs)} />
      </div>

      <div className="grid grid-cols-2 gap-1.5 mt-1.5">
        <DetailChip
          label={t("speed.inputShort")}
          value={t("speed.averagePeak", {
            avg: formatTokens(model.inAvg),
            max: formatTokens(model.inMax),
          })}
        />
        <DetailChip
          label={t("speed.outputShort")}
          value={t("speed.averagePeak", {
            avg: formatTokens(model.outAvg),
            max: formatTokens(model.outMax),
          })}
        />
        <DetailChip
          label={t("speed.latencyShort")}
          value={t("speed.averageSlower", {
            avg: seconds(model.durAvgMs),
            slow: seconds(model.durP90Ms),
          })}
        />
        <DetailChip
          label={t("speed.samplesShort")}
          value={t("speed.requestSummary", {
            n: String(model.requests),
            pct: percent(model.successRate),
          })}
          warn={lowSuccess}
        />
      </div>
    </article>
  );
}

function SpeedRangeToggle({ range, onChange }: { range: SpeedRange; onChange: (range: SpeedRange) => void }) {
  const { t } = useI18n();
  return (
    <div className="speed-range" role="tablist" aria-label={t("speed.rangeLabel")}>
      {(["today", "7d"] as const).map((item) => (
        <button
          key={item}
          type="button"
          role="tab"
          aria-selected={range === item}
          className={`speed-range-button ${range === item ? "speed-range-active" : ""}`}
          onClick={() => onChange(item)}
        >
          {item === "today" ? t("speed.rangeToday") : t("speed.range7d")}
        </button>
      ))}
    </div>
  );
}

/** 数据口径切换（本机 / 全部）：复用 range toggle 的样式族（speed-range /
 *  speed-range-button / speed-range-active），窄窗零新增 CSS */
function SpeedScopeToggle({ scope, onChange }: { scope: SpeedScope; onChange: (scope: SpeedScope) => void }) {
  const { t } = useI18n();
  return (
    <div className="speed-range" role="tablist" aria-label={t("speed.scopeLabel")}>
      {(["local", "all"] as const).map((item) => (
        <button
          key={item}
          type="button"
          role="tab"
          aria-selected={scope === item}
          title={item === "all" ? t("speed.scopeAllTitle") : undefined}
          className={`speed-range-button ${scope === item ? "speed-range-active" : ""}`}
          onClick={() => onChange(item)}
        >
          {item === "local" ? t("speed.scopeLocal") : t("speed.scopeAll")}
        </button>
      ))}
    </div>
  );
}

export function ModelSpeedPanel({ refreshToken = 0 }: { refreshToken?: number }) {
  const { t } = useI18n();
  const [range, setRange] = useState<SpeedRange>(loadSpeedRange);
  const [scope, setScope] = useState<SpeedScope>(loadSpeedScope);
  const [stats, setStats] = useState<ModelSpeedStat[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 远端拉取是否失败（仅"全部设备"口径下可能为 true；失败时本机数据照常，
  // 页面弱提示而非报错——对齐日志页 failed_sources 的容错精神）
  const [remoteFailed, setRemoteFailed] = useState(false);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    try {
      localStorage.setItem(SPEED_RANGE_KEY, range);
    } catch {
      // ignore storage errors
    }
  }, [range]);

  useEffect(() => {
    try {
      localStorage.setItem(SPEED_SCOPE_KEY, scope);
    } catch {
      // ignore storage errors
    }
  }, [scope]);

  // 请求序号（竞态守卫）：scope=all 含远端 HTTP（连接 2s / 总 5s 超时），
  // 本机 SQLite 亚秒返回——切口径 / 时间范围 / 手动刷新可与在途请求并发，
  // 慢的旧响应后到会覆盖新数据（数据与切换钮相反）、污染 error、提前熄掉
  // loading。序号递增后，旧请求的 then / catch / finally 三条路径全部静默
  // 丢弃（setStats / setRemoteFailed / setUpdatedAt / setError / setLoading
  // 五个 setter 全被守卫覆盖）
  const requestSeq = useRef(0);

  const load = useCallback(
    (resetError: boolean) => {
      const seq = ++requestSeq.current;
      if (resetError) setError(null);
      setLoading(true);
      getModelSpeed(range === "today" ? 0 : 7, scope)
        .then((resp) => {
          if (seq !== requestSeq.current) return;
          setStats(resp.stats);
          setRemoteFailed(resp.remoteFailed);
          setUpdatedAt(Date.now());
          setError(null);
        })
        .catch((e) => {
          if (seq !== requestSeq.current) return;
          setError(String(e));
        })
        .finally(() => {
          if (seq !== requestSeq.current) return;
          setLoading(false);
        });
    },
    [range, scope]
  );

  useEffect(() => {
    let cancelled = false;
    const refresh = (resetError: boolean) => {
      if (!cancelled) load(resetError);
    };
    refresh(true);
    const timer = window.setInterval(() => refresh(false), 30_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [load, refreshToken]);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const rows = useMemo(() => (stats ? sortByP50(stats) : []), [stats]);
  const maxP50 = rows.reduce((max, row) => Math.max(max, row.p50Tps ?? 0), 0);
  const overview = useMemo(() => {
    const requests = rows.reduce((sum, row) => sum + row.requests, 0);
    const speedRows = rows.filter((row) => row.p50Tps != null);
    const meanP50 = speedRows.length > 0
      ? speedRows.reduce((sum, row) => sum + (row.p50Tps ?? 0), 0) / speedRows.length
      : null;
    const successRows = rows.filter((row) => row.successRate != null);
    // 成功率权重用本机行数：后端 successRate 的分母只算本机行（远端行无
    // status 列），权重用 requests 会在全部设备口径下被远端行稀释失真；
    // 本机模式 localRequests == requests，行为不变
    const successRequests = successRows.reduce((sum, row) => sum + row.localRequests, 0);
    const success = successRequests > 0
      ? successRows.reduce((sum, row) => sum + row.localRequests * (row.successRate ?? 0), 0) / successRequests
      : null;
    return { requests, meanP50, success };
  }, [rows]);

  // 最快值取排序后首个有 p50 的行：该行可能来自 request_average 来源，
  // hero 的 ≈ 标记与该卡片保持一致（挪到 kicker/label 侧的同一形态），
  // 避免同一数字两处口径标记不同
  const fastestRow = rows.find((row) => row.p50Tps != null);
  const fastest = fastestRow?.p50Tps ?? null;
  const fastestApprox = fastestRow?.speedQuality === "request_average";
  const stamp = updatedAt == null ? "" : new Date(updatedAt).toTimeString().slice(0, 8);
  const ago = updatedAt == null ? 0 : Math.max(0, Math.round((now - updatedAt) / 1000));

  const content = stats === null ? (
    error ? (
      <div className="speed-state speed-state-error">
        <div className="speed-state-icon">!</div>
        <div className="text-xs text-slate-700/80 font-medium">{t("speed.loadFailed")}</div>
        <div className="text-[10px] text-red-700/80 break-all max-w-[230px] text-center">{error}</div>
        <button type="button" className="speed-retry-button" onClick={() => load(true)}>{t("speed.retry")}</button>
      </div>
    ) : (
      <div className="speed-state">
        <div className="speed-loading-orb" />
        <div className="text-xs text-slate-600/80">{t("common.loading")}</div>
      </div>
    )
  ) : rows.length === 0 ? (
    <div className="speed-state speed-state-empty">
      <div className="speed-empty-chart" aria-hidden="true"><span /><span /><span /><span /></div>
      <div className="text-xs text-slate-700/80 font-medium">{t("speed.empty")}</div>
      <div className="text-[10px] text-slate-500 leading-relaxed max-w-[220px] text-center">{t("speed.emptyHint")}</div>
      <button type="button" className="speed-retry-button" onClick={() => load(true)}>{t("speed.refresh")}</button>
    </div>
  ) : (
    <>
      <div className="speed-list-heading">
        <div className="flex items-center gap-1.5">
          <span className="speed-heading-dot" />
          <span>{t("speed.ranking")}</span>
        </div>
        <span className="num">{t("speed.modelCount", { n: rows.length })}</span>
      </div>
      <div className="flex flex-col gap-2">
        {rows.map((row, index) => (
          // key 用三元组：同名模型跨来源 / 跨渠道是独立分组，modelId 会重复
          <ModelSpeedRow
            key={`${row.source}:${row.provider}:${row.modelId}`}
            model={row}
            rank={index}
            maxP50={maxP50}
          />
        ))}
      </div>
    </>
  );

  return (
    <div className="speed-page flex-1 min-h-0 overflow-y-auto px-3 py-3">
      <section className="speed-hero">
        <div className="flex items-start justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <SpeedGlyph />
            <div className="min-w-0">
              <h2 className="speed-hero-title truncate">{t("speed.title")}</h2>
              <p className="speed-hero-subtitle truncate">{t("speed.subtitle")}</p>
            </div>
          </div>
          {/* 260-300px 窄窗放不下并排两组 toggle：右侧改为两行栈——上行
              时间范围 + 刷新（主控），下行数据口径（本机 / 全部，次级），
              不换行溢出；左列标题块两行高与右列两行控件高接近，hero 增高
              控制在一档控件以内。行间距 8px（gap-2）：与组内元素间距 6px
              拉开一档，首眼不误读成一摞四态切换 */}
          <div className="flex flex-col items-end gap-2 shrink-0">
            <div className="flex items-center gap-1.5">
              <SpeedRangeToggle range={range} onChange={setRange} />
              <button type="button" className={`speed-refresh ${loading ? "speed-refresh-loading" : ""}`} onClick={() => load(true)} aria-label={t("speed.refresh")}>
                <RefreshGlyph />
              </button>
            </div>
            <SpeedScopeToggle scope={scope} onChange={setScope} />
          </div>
        </div>

        <div className="grid grid-cols-[1.08fr_1fr] gap-2 mt-3">
          <div className="speed-fastest-block">
            <div className="speed-kicker" title={fastestApprox ? t("speed.approxHint") : undefined}>
              {fastestApprox ? `≈${t("speed.fastest")}` : t("speed.fastest")}
            </div>
            {/* 最快值 2rem：260px 窄窗 + 1.25 字号档下会顶破左列——
                单行截断（min-w-0 + truncate），单位 t/s 保持可见 */}
            <div className="flex items-baseline gap-1 mt-1 min-w-0">
              <span className="speed-fastest-value num min-w-0 truncate">{tps(fastest)}</span>
              <span className="speed-fastest-unit">t/s</span>
            </div>
            <div className="speed-fastest-caption">{t("speed.fastestHint")}</div>
          </div>
          <div className="grid grid-cols-2 gap-1.5">
            <OverviewMetric label={t("speed.overviewModels")} value={String(rows.length)} />
            <OverviewMetric label={t("speed.overviewRequests")} value={formatTokens(overview.requests)} />
            <OverviewMetric label={t("speed.overviewMean")} value={overview.meanP50 == null ? "—" : `${formatTps(overview.meanP50)}`} title={t("speed.overviewMeanHint")} />
            <OverviewMetric label={t("speed.overviewSuccess")} value={percent(overview.success)} />
          </div>
        </div>

        <div className="speed-hero-footer">
          <span>{updatedAt == null ? t("speed.waitingData") : t("speed.updatedAt", { time: stamp, ago: String(ago) })}</span>
          <span className="speed-live-mark"><i />{t("speed.autoRefresh")}</span>
        </div>
      </section>

      {/* 全部设备口径下远端拉取失败：本机数据照常，灰色弱提示（非报错色）。
          title 兜底英文长文案在窄窗的截断；整体刷新失败（error 非空）时
          让位给红错误条，不双条叠加 */}
      {scope === "all" && remoteFailed && stats !== null && !error && (
        <div className="speed-remote-note" title={t("speed.remoteUnavailable")}>
          {t("speed.remoteUnavailable")}
        </div>
      )}
      {error && stats !== null && <div className="speed-inline-error">{t("speed.fail", { msg: error })}</div>}
      <section className="speed-results">{content}</section>
    </div>
  );
}
