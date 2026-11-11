/**
 * 「会话」页（主窗口）：实时会话账本 + 上下文健康。
 *
 * 两个 tab 共用一次账本取数（15 秒轮询，窗口不可见时暂停）：
 *  - 实时会话：按「来源 + 会话」一行，回答"现在有哪些会话在跑、各花了多少"；
 *  - 上下文健康：左侧会话列表 + 右侧水位曲线，回答"这个会话的上下文涨到哪了、
 *    被压缩过几次"。上下文是点开才查（`get_session_context`），不参与轮询。
 *
 * 口径与请求日志页同源同算法（Rust `session_inspect` 直接复用 `request_logs`
 * 的来源查询），所以同一时刻两页的 token / 花费数字对得上。上下文水位是
 * prompt 侧 token 的估算，压缩次数由水位骤降推断——界面按估算措辞呈现，
 * 不宣称精确（详见 Rust 模块注释）。
 */
import { useMemo, useState } from "react";
import type { TFn } from "../../i18n";
import { useI18n } from "../../i18n";
import { AlertBanner, EmptyState } from "../../layout";
import { DropdownMenu } from "../../DropdownMenu";
import { formatCost, formatTokensLocale } from "../../format";
import { useCurrencyPref } from "../settings/currencyPref";
import { costOf } from "../overview/derive";
import { sourceLabel, sourceShortLabel } from "../logs/model";
import { RefreshIcon } from "../icons";
import { Card, Skeleton } from "../primitives";
import type {
  Currency,
  LiveSession,
  RequestLogSource,
  SessionContext,
} from "../../types";
import {
  useSessionContext,
  useSessionsFeed,
  type SessionTarget,
} from "./useSessionsData";

/** 时间窗口档位（小时）：1h 看正在跑、72h 看长会话的回溯范围 */
const WINDOWS = [1, 6, 24, 72];

/** 状态筛选 */
type StateFilter = "all" | "running" | "idle";

/** 数字本地化用的语言（formatTokensLocale 的入参口径） */
type NumLocale = "zh" | "en";

/** 本地时间 HH:mm:ss */
function timeText(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** 相对时间（以本次取数时刻为基准，避免每秒重渲染） */
function relativeText(t: TFn, ms: number, now: number): string {
  const diff = Math.max(0, now - ms);
  if (diff < 60_000) return t("dashboard.sessions.relJustNow");
  if (diff < 3_600_000) {
    return t("dashboard.sessions.relMinutes", { n: Math.floor(diff / 60_000) });
  }
  if (diff < 86_400_000) {
    return t("dashboard.sessions.relHours", {
      n: Math.floor(diff / 3_600_000),
    });
  }
  return t("dashboard.sessions.relDays", { n: Math.floor(diff / 86_400_000) });
}

/** 会话的展示名：项目优先，缺项目时用会话 id 前缀 */
function targetName(session: LiveSession): string {
  const project = session.project?.trim();
  if (project) return project;
  return session.session_id ? session.session_id.slice(0, 12) : "";
}

/** 指标卡（复用总览页 db-metric 形态，本页只用最简版本） */
function Metric({
  label,
  value,
  sub,
  title,
}: {
  label: string;
  value: string;
  sub?: string;
  title?: string;
}) {
  return (
    <div className="db-card db-metric" title={title}>
      <div className="db-metric__label">{label}</div>
      <div className="db-metric__value num">
        <span>{value}</span>
      </div>
      {sub && <div className="db-metric__sub">{sub}</div>}
    </div>
  );
}

/** 状态点：运行中带呼吸圈，空闲为灰点 */
function StateDot({ state }: { state: LiveSession["state"] }) {
  return (
    <span
      className={`db-sessions__dot db-sessions__dot--${state}`}
      aria-hidden
    />
  );
}

/** 上下文水位的小型进度条（只有模型窗口已知时才给出比例） */
function ContextMeter({
  session,
  locale,
}: {
  session: LiveSession;
  locale: NumLocale;
}) {
  const limit = session.context_limit;
  const ratio =
    limit != null && limit > 0
      ? Math.min(1, session.context_last / limit)
      : null;
  return (
    <div className="db-sessions__context">
      <span className="num">{formatTokensLocale(session.context_last, locale)}</span>
      {ratio != null && (
        <span
          className="db-sessions__meter"
          title={`${formatTokensLocale(session.context_last, locale)} / ${formatTokensLocale(limit!, locale)}`}
        >
          <span
            className={`db-sessions__meter-fill${
              ratio >= 0.9
                ? " db-sessions__meter-fill--hot"
                : ratio >= 0.7
                  ? " db-sessions__meter-fill--warn"
                  : ""
            }`}
            style={{ width: `${Math.round(ratio * 100)}%` }}
          />
        </span>
      )}
      {session.compactions > 0 && (
        <span
          className="db-sessions__compact-chip"
          title={`${session.compactions}`}
        >
          ↺{session.compactions}
        </span>
      )}
    </div>
  );
}

/** 实时会话表 */
function LiveTable({
  sessions,
  now,
  currency,
  onOpen,
}: {
  sessions: LiveSession[];
  now: number;
  currency: Currency;
  onOpen: (session: LiveSession) => void;
}) {
  const { locale, t } = useI18n();
  return (
    <div className="db-table-wrap">
      <table className="db-table db-sessions-table">
        <thead>
          <tr>
            <th className="db-sessions__col-state">
              {t("dashboard.sessions.colState")}
            </th>
            <th className="db-sessions__col-source">
              {t("dashboard.sessions.colSource")}
            </th>
            <th className="db-sessions__col-target">
              {t("dashboard.sessions.colTarget")}
            </th>
            <th className="db-sessions__col-model">
              {t("dashboard.sessions.colModel")}
            </th>
            <th className="db-table__num db-sessions__col-requests">
              {t("dashboard.sessions.colRequests")}
            </th>
            <th className="db-table__num db-sessions__col-tokens">
              {t("dashboard.sessions.colTokens")}
            </th>
            <th className="db-table__num db-sessions__col-context">
              {t("dashboard.sessions.colContext")}
            </th>
            <th className="db-table__num db-sessions__col-cost">
              {t("dashboard.sessions.colCost")}
            </th>
            <th className="db-table__num db-sessions__col-activity">
              {t("dashboard.sessions.colActivity")}
            </th>
          </tr>
        </thead>
        <tbody>
          {sessions.map((session) => {
            const name = targetName(session);
            const cost = costOf(session.cost_usd, session.cost_cny, currency);
            return (
              <tr
                key={`${session.source}-${session.session_id}-${session.started_at}`}
                className="db-sessions__row"
                tabIndex={0}
                title={t("dashboard.sessions.openContext")}
                onClick={() => onOpen(session)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    onOpen(session);
                  }
                }}
              >
                <td className="db-sessions__col-state">
                  <span className="db-sessions__state">
                    <StateDot state={session.state} />
                    {session.state === "running"
                      ? t("dashboard.sessions.stateRunning")
                      : t("dashboard.sessions.stateIdle")}
                  </span>
                </td>
                <td
                  className="db-sessions__col-source"
                  title={sourceLabel(session.source)}
                >
                  {sourceShortLabel(session.source)}
                </td>
                <td className="db-sessions__col-target">
                  {name ? (
                    <span className="db-sessions__name" title={session.session_id}>
                      {name}
                    </span>
                  ) : (
                    <span className="db-table__dash">
                      {t("dashboard.sessions.unlabeled")}
                    </span>
                  )}
                </td>
                <td className="db-sessions__col-model">
                  <span className="db-sessions__model" title={session.model_id}>
                    {session.model_id || "—"}
                  </span>
                </td>
                <td className="db-table__num db-sessions__col-requests num">
                  {session.requests}
                </td>
                <td className="db-table__num db-sessions__col-tokens num">
                  <span className="db-sessions__tokens">
                    {formatTokensLocale(session.input_tokens, locale)}
                    <span className="db-sessions__slash"> / </span>
                    {formatTokensLocale(session.output_tokens, locale)}
                  </span>
                </td>
                <td className="db-table__num db-sessions__col-context">
                  <ContextMeter session={session} locale={locale} />
                </td>
                <td className="db-table__num db-sessions__col-cost num">
                  {cost > 0 ? (
                    formatCost(cost, currency)
                  ) : (
                    <span className="db-table__dash">—</span>
                  )}
                </td>
                <td className="db-table__num db-sessions__col-activity num">
                  {relativeText(t, session.last_at, now)}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** 上下文水位曲线（纯 SVG 手绘，与项目其它图表同一做法，零图表依赖） */
function ContextChart({
  context,
  locale,
}: {
  context: SessionContext;
  locale: NumLocale;
}) {
  const { t } = useI18n();
  const points = context.points;
  const width = 720;
  const height = 210;
  const stripHeight = 54;
  const padX = 46;
  const padY = 16;

  const geometry = useMemo(() => {
    if (points.length === 0) return null;
    const t0 = points[0].ts;
    const t1 = points[points.length - 1].ts;
    const span = Math.max(1, t1 - t0);
    const ceiling = Math.max(context.peak, context.context_limit ?? 0) * 1.08;
    const ymax = ceiling > 0 ? ceiling : 1;
    const xOf = (ts: number) =>
      padX + ((ts - t0) / span) * (width - padX * 2);
    const yOf = (value: number) =>
      padY + (1 - Math.min(1, value / ymax)) * (height - padY * 2);

    const line = points
      .map(
        (point, index) =>
          `${index === 0 ? "M" : "L"}${xOf(point.ts).toFixed(1)},${yOf(
            point.prompt_tokens
          ).toFixed(1)}`
      )
      .join(" ");
    const baseline = height - padY;
    const area = `${line} L${xOf(t1).toFixed(1)},${baseline} L${xOf(
      t0
    ).toFixed(1)},${baseline} Z`;

    // 压缩：水位较上一轮骤降（与后端同一口径，这里重算只为拿到横坐标）
    const compactions: { ts: number; prompt: number }[] = [];
    for (let index = 1; index < points.length; index += 1) {
      const previous = points[index - 1].prompt_tokens;
      const current = points[index].prompt_tokens;
      if (previous >= 10_000 && current < previous * 0.55) {
        compactions.push({ ts: points[index].ts, prompt: current });
      }
    }

    // 缓存占比条：只画有分母的点，缺失处断开；纵向铺满独立的小条带
    const cacheY = (ratio: number) =>
      4 + (1 - Math.min(1, Math.max(0, ratio))) * (stripHeight - 10);
    const cacheSegments: string[] = [];
    let segment = "";
    for (const point of points) {
      if (point.cache_ratio == null) {
        if (segment) cacheSegments.push(segment);
        segment = "";
        continue;
      }
      const x = xOf(point.ts).toFixed(1);
      const y = cacheY(point.cache_ratio).toFixed(1);
      segment += `${segment ? "L" : "M"}${x},${y}`;
    }
    if (segment) cacheSegments.push(segment);
    // 面积路径：每条线段闭合到条带基线，视觉上才像"占比"而不是一根线
    const cacheAreas = cacheSegments.map((segmentPath) => {
      const xs = segmentPath.match(/[ML]([\d.]+),/g) ?? [];
      const firstX = xs[0]?.slice(1, -1) ?? "";
      const lastX = xs[xs.length - 1]?.slice(1, -1) ?? firstX;
      return `${segmentPath} L${lastX},${stripHeight} L${firstX},${stripHeight} Z`;
    });

    return {
      t0,
      t1,
      ymax,
      line,
      area,
      compactions,
      cacheSegments,
      cacheAreas,
      xOf,
      yOf,
    };
  }, [points, context.peak, context.context_limit]);

  if (!geometry) {
    return <p className="db-sessions__chart-empty">{t("dashboard.sessions.context.empty")}</p>;
  }

  const limitY =
    context.context_limit != null
      ? geometry.yOf(context.context_limit)
      : null;

  return (
    <div className="db-sessions__charts">
      <svg
        className="db-sessions__svg"
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={t("dashboard.sessions.context.chartTitle")}
      >
        {/* 横向网格：0 / 50% / 100%（相对峰值或模型窗口的较高者） */}
        {[0, 0.5, 1].map((ratio) => {
          const y = padY + ratio * (height - padY * 2);
          return (
            <line
              key={ratio}
              x1={padX}
              y1={y}
              x2={width - padX}
              y2={y}
              className="db-sessions__grid"
            />
          );
        })}
        {/* 模型窗口上限（已知时）：虚线 + 右侧标签 */}
        {limitY != null && (
          <>
            <line
              x1={padX}
              y1={limitY}
              x2={width - padX}
              y2={limitY}
              className="db-sessions__limit"
            />
            <text
              x={width - padX}
              y={limitY - 5}
              textAnchor="end"
              className="db-sessions__limit-label"
            >
              {formatTokensLocale(context.context_limit!, locale)}
            </text>
          </>
        )}
        <path d={geometry.area} className="db-sessions__area" />
        <path d={geometry.line} className="db-sessions__line" />
        {/* 压缩标记：竖虚线 + 落点圆点 */}
        {geometry.compactions.map((mark) => {
          const x = geometry.xOf(mark.ts);
          return (
            <g key={`c-${mark.ts}`}>
              <line
                x1={x}
                y1={padY}
                x2={x}
                y2={height - padY}
                className="db-sessions__compact-line"
              />
              <circle
                cx={x}
                cy={geometry.yOf(mark.prompt)}
                r={3.4}
                className="db-sessions__compact-dot"
              >
                <title>{t("dashboard.sessions.context.compactionMark")}</title>
              </circle>
            </g>
          );
        })}
        {/* 横轴：首尾时刻 */}
        <text x={padX} y={height - 2} className="db-sessions__axis">
          {timeText(geometry.t0)}
        </text>
        <text
          x={width - padX}
          y={height - 2}
          textAnchor="end"
          className="db-sessions__axis"
        >
          {timeText(geometry.t1)}
        </text>
      </svg>

      <svg
        className="db-sessions__svg db-sessions__svg--strip"
        viewBox={`0 0 ${width} ${stripHeight}`}
        role="img"
        aria-label={t("dashboard.sessions.context.cacheTitle")}
      >
        <line
          x1={padX}
          y1={stripHeight - 1}
          x2={width - padX}
          y2={stripHeight - 1}
          className="db-sessions__grid"
        />
        {geometry.cacheAreas.map((areaPath, index) => (
          <path key={`a-${index}`} d={areaPath} className="db-sessions__cache-area" />
        ))}
        {geometry.cacheSegments.map((segment, index) => (
          <path key={index} d={segment} className="db-sessions__cache-line" />
        ))}
      </svg>
      <div className="db-sessions__chart-legend">
        <span className="db-sessions__legend">
          <i className="db-sessions__swatch db-sessions__swatch--context" />
          {t("dashboard.sessions.context.chartTitle")}
        </span>
        <span className="db-sessions__legend">
          <i className="db-sessions__swatch db-sessions__swatch--cache" />
          {t("dashboard.sessions.context.cacheTitle")}
        </span>
      </div>
    </div>
  );
}

/** 上下文健康面板：左会话列表 + 右水位视图 */
function ContextPanel({
  sessions,
  target,
  onPick,
  now,
  windowHours,
}: {
  sessions: LiveSession[];
  target: SessionTarget | null;
  onPick: (session: LiveSession) => void;
  now: number;
  windowHours: number;
}) {
  const { locale, t } = useI18n();
  const { currency } = useCurrencyPref();
  const { context, loading, error } = useSessionContext(target, windowHours);

  const selected = useMemo(
    () =>
      sessions.find(
        (session) =>
          session.source === target?.source &&
          session.session_id === target?.sessionId
      ) ?? null,
    [sessions, target]
  );

  return (
    <div className="db-sessions__split">
      <aside className="db-sessions__picks" aria-label={t("dashboard.sessions.context.sessions")}>
        {sessions.map((session) => {
          const active =
            session.source === target?.source &&
            session.session_id === target?.sessionId;
          const name = targetName(session);
          return (
            <button
              key={`${session.source}-${session.session_id}-${session.started_at}`}
              type="button"
              className={`db-sessions__pick${active ? " db-sessions__pick--active" : ""}`}
              onClick={() => onPick(session)}
            >
              <span className="db-sessions__pick-top">
                <StateDot state={session.state} />
                <span className="db-sessions__pick-source">
                  {sourceShortLabel(session.source)}
                </span>
                <span className="db-sessions__pick-time num">
                  {relativeText(t, session.last_at, now)}
                </span>
              </span>
              <span className="db-sessions__pick-name" title={session.session_id}>
                {name || t("dashboard.sessions.unlabeled")}
              </span>
              <span className="db-sessions__pick-meta num">
                {formatTokensLocale(session.context_last, locale)} ctx ·{" "}
                {session.requests} req
                {session.compactions > 0 ? ` · ↺${session.compactions}` : ""}
              </span>
            </button>
          );
        })}
      </aside>

      <div className="db-sessions__detail">
        {target == null ? (
          <EmptyState
            title={t("dashboard.sessions.context.pick")}
            hint={t("dashboard.sessions.context.pickHint")}
          />
        ) : error ? (
          <AlertBanner type="error">
            {t("dashboard.sessions.context.error", { error })}
          </AlertBanner>
        ) : loading ? (
          <div className="db-sessions__chart-skeleton">
            <Skeleton className="h-[210px] w-full" />
          </div>
        ) : context == null ? (
          <EmptyState title={t("dashboard.sessions.context.pick")} hint={t("dashboard.sessions.context.pickHint")} />
        ) : (
          <>
            <header className="db-sessions__detail-head">
              <div>
                <h3 className="db-sessions__detail-title">
                  {context.project?.trim() || selected?.project?.trim() || target.sessionId.slice(0, 12) || t("dashboard.sessions.unlabeled")}
                </h3>
                <p className="db-sessions__detail-sub">
                  {sourceLabel(context.source)} · {context.model_id || "—"} ·{" "}
                  {context.points.length} {t("dashboard.sessions.context.requests")}
                </p>
              </div>
              {selected && (
                <span className="db-sessions__detail-cost num">
                  {formatCost(
                    costOf(selected.cost_usd, selected.cost_cny, currency),
                    currency
                  )}
                </span>
              )}
            </header>

            <div className="db-sessions__kv">
              <div>
                <span className="db-sessions__kv-label">
                  {t("dashboard.sessions.context.current")}
                </span>
                <strong className="num">
                  {formatTokensLocale(context.last, locale)}
                </strong>
              </div>
              <div>
                <span className="db-sessions__kv-label">
                  {t("dashboard.sessions.context.peak")}
                </span>
                <strong className="num">
                  {formatTokensLocale(context.peak, locale)}
                </strong>
              </div>
              <div>
                <span className="db-sessions__kv-label">
                  {t("dashboard.sessions.context.limit")}
                </span>
                <strong className="num">
                  {context.context_limit != null
                    ? formatTokensLocale(context.context_limit, locale)
                    : t("dashboard.sessions.context.limitUnknown")}
                </strong>
              </div>
              <div>
                <span className="db-sessions__kv-label">
                  {t("dashboard.sessions.context.compactions")}
                </span>
                <strong className="num">{context.compactions}</strong>
              </div>
            </div>

            <ContextChart context={context} locale={locale} />
            <p className="db-sessions__note">
              {t("dashboard.sessions.context.note")}
            </p>
          </>
        )}
      </div>
    </div>
  );
}

export function SessionsPage() {
  const { t } = useI18n();
  const [tab, setTab] = useState<"live" | "context">("live");
  const [windowHours, setWindowHours] = useState(6);
  const [stateFilter, setStateFilter] = useState<StateFilter>("all");
  const [sourceFilter, setSourceFilter] = useState<string>("all");
  const [target, setTarget] = useState<SessionTarget | null>(null);
  const { feed, loading, refreshing, error, lastUpdatedAt, reload } =
    useSessionsFeed(windowHours);
  const { currency } = useCurrencyPref();

  const sessions = useMemo(() => feed?.sessions ?? [], [feed]);
  const now = feed?.generated_at ?? Date.now();

  const sourceOptions = useMemo(() => {
    const seen: string[] = [];
    for (const session of sessions) {
      if (!seen.includes(session.source)) seen.push(session.source);
    }
    return [
      { value: "all", label: t("dashboard.sessions.sourceAll") },
      ...seen.map((source) => ({ value: source, label: sourceLabel(source) })),
    ];
  }, [sessions, t]);

  const filtered = useMemo(
    () =>
      sessions.filter((session) => {
        if (stateFilter !== "all" && session.state !== stateFilter) return false;
        if (sourceFilter !== "all" && session.source !== sourceFilter)
          return false;
        return true;
      }),
    [sessions, stateFilter, sourceFilter]
  );

  const running = sessions.filter((session) => session.state === "running");
  const activeSources = useMemo(
    () => new Set(sessions.map((session) => session.source)).size,
    [sessions]
  );
  const totalRequests = sessions.reduce(
    (sum, session) => sum + session.requests,
    0
  );
  const totalCost = sessions.reduce(
    (sum, session) => sum + costOf(session.cost_usd, session.cost_cny, currency),
    0
  );

  // 上下文 tab 的有效目标：选中的会话不在当前筛选结果里时回退到第一条
  const effectiveTarget = useMemo<SessionTarget | null>(() => {
    if (filtered.length === 0) return null;
    if (target) {
      const hit = filtered.find(
        (session) =>
          session.source === target.source &&
          session.session_id === target.sessionId
      );
      if (hit) return target;
    }
    const first = filtered[0];
    return { source: first.source as RequestLogSource, sessionId: first.session_id };
  }, [filtered, target]);

  const openContext = (session: LiveSession) => {
    setTarget({
      source: session.source,
      sessionId: session.session_id,
    });
    setTab("context");
  };

  const isEmpty = feed != null && sessions.length === 0;
  const isFilteredEmpty = !isEmpty && filtered.length === 0;

  return (
    <div className="db-page">
      <header className="db-page__header">
        <div>
          <h1 className="db-page__title">{t("dashboard.sessions")}</h1>
          <p className="db-page__desc">{t("dashboard.sessions.desc")}</p>
        </div>
        <div className="db-page__actions">
          <DropdownMenu
            label={t("dashboard.sessions.window")}
            ariaLabel={t("dashboard.sessions.window")}
            options={WINDOWS.map((hours) => ({
              value: String(hours),
              label: t("dashboard.sessions.windowOption", { hours }),
            }))}
            value={String(windowHours)}
            onChange={(value) => setWindowHours(Number(value))}
          />
          <button
            type="button"
            className="db-btn"
            onClick={reload}
            disabled={loading}
          >
            <RefreshIcon
              className={`h-3.5 w-3.5 ${loading || refreshing ? "db-spin" : ""}`}
            />
            {t("dashboard.sessions.refresh")}
          </button>
          <span className="db-page__updated num">
            {lastUpdatedAt != null
              ? t("dashboard.sessions.lastUpdated", {
                  time: timeText(lastUpdatedAt),
                })
              : ""}
          </span>
        </div>
      </header>

      <div className="db-seg db-sessions__tabs" role="tablist">
        {(
          [
            ["live", "dashboard.sessions.tabLive"],
            ["context", "dashboard.sessions.tabContext"],
          ] as const
        ).map(([value, key]) => (
          <button
            key={value}
            type="button"
            role="tab"
            aria-selected={tab === value}
            className={`db-seg__btn ${tab === value ? "db-seg__btn--active" : ""}`}
            onClick={() => setTab(value)}
          >
            {t(key)}
          </button>
        ))}
      </div>

      {error && (
        <AlertBanner type="error">
          {t("dashboard.sessions.error", { error })}
        </AlertBanner>
      )}
      {feed != null && feed.failed_sources.length > 0 && (
        <div className="db-hintbar">
          {t("dashboard.sessions.partial", {
            sources: feed.failed_sources
              .map((source) => sourceLabel(source))
              .join(" / "),
          })}
        </div>
      )}

      <div className="db-metrics db-sessions__metrics">
        <Metric
          label={t("dashboard.sessions.metricRunning")}
          value={String(running.length)}
          sub={t("dashboard.sessions.metricRunningHint")}
        />
        <Metric
          label={t("dashboard.sessions.metricSources")}
          value={String(activeSources)}
          sub={t("dashboard.sessions.windowOption", { hours: windowHours })}
        />
        <Metric
          label={t("dashboard.sessions.metricRequests")}
          value={String(totalRequests)}
          sub={t("dashboard.sessions.metricSessions") + ` ${sessions.length}`}
        />
        <Metric
          label={t("dashboard.sessions.metricCost")}
          value={formatCost(totalCost, currency)}
          sub={currency.toUpperCase()}
        />
      </div>

      {loading && feed == null ? (
        <Card>
          <div className="db-sessions__skeleton">
            {Array.from({ length: 6 }).map((_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        </Card>
      ) : isEmpty ? (
        <EmptyState
          title={t("dashboard.sessions.emptyTitle")}
          hint={t("dashboard.sessions.emptyHint")}
          action={
            <button type="button" className="db-btn mt-1" onClick={reload}>
              {t("dashboard.sessions.refresh")}
            </button>
          }
        />
      ) : tab === "live" ? (
        <Card
          title={t("dashboard.sessions.tabLive")}
          sub={t("dashboard.sessions.windowOption", { hours: windowHours })}
          action={
            <div className="db-sessions__filters">
              <DropdownMenu
                label={t("dashboard.sessions.filterState")}
                ariaLabel={t("dashboard.sessions.filterState")}
                options={[
                  { value: "all", label: t("dashboard.sessions.stateAll") },
                  { value: "running", label: t("dashboard.sessions.stateRunning") },
                  { value: "idle", label: t("dashboard.sessions.stateIdle") },
                ]}
                value={stateFilter}
                onChange={(value) => setStateFilter(value as StateFilter)}
              />
              <DropdownMenu
                label={t("dashboard.sessions.filterSource")}
                ariaLabel={t("dashboard.sessions.filterSource")}
                options={sourceOptions}
                value={sourceFilter}
                onChange={setSourceFilter}
              />
            </div>
          }
        >
          {isFilteredEmpty ? (
            <EmptyState
              title={t("dashboard.sessions.emptyFilteredTitle")}
              hint={t("dashboard.sessions.emptyFilteredHint")}
            />
          ) : (
            <LiveTable
              sessions={filtered}
              now={now}
              currency={currency}
              onOpen={openContext}
            />
          )}
        </Card>
      ) : (
        <Card
          title={t("dashboard.sessions.tabContext")}
          sub={t("dashboard.sessions.context.desc")}
        >
          <ContextPanel
            sessions={filtered}
            target={effectiveTarget}
            onPick={openContext}
            now={now}
            windowHours={windowHours}
          />
        </Card>
      )}
    </div>
  );
}
