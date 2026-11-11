/** 灵动岛各页面：主页 / 消息 / 股票 / 天气 / 控制 / 剪贴板 / 设置 */

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { getStockMinute, locateCurrentPlace, mediaAction as runMediaAction, searchStock } from "./api";
import type {
  ClipItem,
  HudBarAgent,
  HudBarUsage,
  IslandConfig,
  LyricsState,
  MediaState,
  NotifyItem,
  NotifyState,
  StockMinute,
  StocksState,
  SystemState,
  WeatherState,
} from "./types";
import { hotkeyText } from "../shortcuts";
import type { TabKey } from "./types";
import { citiesOf, districtsOf, findDivisionPath, matchPlace, provinces } from "./china";
import {
  agentColor,
  agentLabel,
  BrandMark,
  currentLyricLine,
  EmptyState,
  fmtClock,
  fmtCountdown,
  fmtDate,
  fmtDuration,
  fmtMoney,
  Icon,
  type IconName,
  parseLyrics,
  QuotaBar,
  quotaLabel,
  quotaTone,
  remainingPct,
  SectionHead,
  Slider,
  timeAgo,
  Toggle,
  useNow,
  WeatherGlyph,
} from "./ui";

// ============================================================
// 主页
// ============================================================

function AgentCard({ agent, compact = false }: { agent: HudBarAgent; compact?: boolean }) {
  const now = useNow();
  const placeholder = agent.status === "unavailable" || agent.status === "usage";
  return (
    <div
      className={`agent-card ${compact ? "compact" : ""} ${placeholder ? "placeholder" : ""} ${
        agent.balance ? "balance-card" : ""
      }`}
    >
      <div className="agent-head">
        <span className="agent-icon" style={{ background: `${agentColor(agent.source)}22` }}>
          <BrandMark source={agent.source} size={compact ? 16 : 18} />
        </span>
        <span className="agent-name" title={agent.account ? `${agentLabel(agent.source)} · ${agent.account}` : agentLabel(agent.source)}>
          {agentLabel(agent.source)}
          {agent.account ? <em className="agent-account"> · {agent.account}</em> : null}
        </span>
        {agent.plan ? <span className="agent-plan">{agent.plan}</span> : null}
        {placeholder ? (
          <span className="agent-badge warn">{agent.status === "unavailable" ? "暂无数据" : "仅用量"}</span>
        ) : agent.balance ? (
          <span className="agent-badge ok">按量</span>
        ) : (
          <span className="agent-status-dot" />
        )}
      </div>
      {agent.balance ? (
        <div className="balance-row">
          <span className="balance-amount">{fmtMoney(agent.balance.amount, agent.balance.currency)}</span>
          <span className="balance-label">余额</span>
          {agent.balance.granted != null || agent.balance.toppedUp != null ? (
            <span className="balance-split">
              {agent.balance.toppedUp != null ? `充值 ${fmtMoney(agent.balance.toppedUp, agent.balance.currency)}` : ""}
              {agent.balance.toppedUp != null && agent.balance.granted != null ? " · " : ""}
              {agent.balance.granted != null ? `赠送 ${fmtMoney(agent.balance.granted, agent.balance.currency)}` : ""}
            </span>
          ) : null}
        </div>
      ) : agent.quotas.length === 0 ? (
        <div className="agent-empty">{placeholder ? "登录后显示额度" : "暂无额度窗口"}</div>
      ) : (
        <div className="quota-list">
          {agent.quotas.map((q) => {
            const pct = remainingPct(q);
            return (
              <div className="quota-row" key={q.key} data-tone={quotaTone(q)}>
                <span className="quota-label">{quotaLabel(q, true)}</span>
                <QuotaBar quota={q} compact />
                <span className="quota-pct">{Math.round(pct)}%</span>
                <span className="quota-reset">{fmtCountdown(q.resetAt, now)}</span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function HomePage({
  usage,
  weather,
  media,
  lyrics,
  muted,
  config,
  refreshing,
  onRefresh,
  onNavigate,
  onToggleMute,
  onOpenMain,
  onLaunch,
}: {
  usage: HudBarUsage | null;
  weather: WeatherState | null;
  media: MediaState;
  lyrics: LyricsState | null;
  muted: boolean;
  config: IslandConfig;
  refreshing: boolean;
  onRefresh: () => void;
  onNavigate: (tab: TabKey) => void;
  onToggleMute: () => void;
  onOpenMain: () => void;
  onLaunch: (path: string) => void;
}) {
  const now = useNow();
  const agents = usage?.agents ?? [];
  const pageRef = useRef<HTMLDivElement>(null);
  const [page, setPage] = useState(0);
  const wheelAt = useRef(0);
  const pageCount = Math.max(1, Math.ceil(agents.length / 4));
  const safePage = Math.min(page, pageCount - 1);
  const pageAgents = useMemo(
    () => Array.from({ length: pageCount }, (_, index) => agents.slice(index * 4, index * 4 + 4)),
    [agents, pageCount]
  );

  useEffect(() => {
    if (page > pageCount - 1) setPage(pageCount - 1);
  }, [page, pageCount]);

  const turnPage = (direction: number) => {
    const nowMs = Date.now();
    if (nowMs - wheelAt.current < 240) return;
    wheelAt.current = nowMs;
    setPage((current) => Math.max(0, Math.min(pageCount - 1, current + direction)));
  };

  const lyricLine = useMemo(() => {
    if (!lyrics?.ok || !media.hasSession) return null;
    return currentLyricLine(parseLyrics(lyrics.lrc, lyrics.translation), media.position);
  }, [lyrics, media.hasSession, media.position]);

  // 面板高度固定（492），媒体条 / 3 行额度卡 / 大字号都可能把主页顶出滚动条。
  // 这里按实际可用高度把整页等比缩到刚好放下：不出现滚动条，也不裁内容；
  // 宽度按 100/scale 补偿，缩完仍铺满面板内容区。
  // 不写依赖数组：账号数量 / 媒体条 / 字号 / 可用高度任一变化都要重新量。
  useLayoutEffect(() => {
    const pageEl = pageRef.current;
    const contentEl = pageEl?.parentElement;
    if (!pageEl || !contentEl) return;
    const cs = getComputedStyle(contentEl);
    const available =
      contentEl.clientHeight -
      (parseFloat(cs.paddingTop) || 0) -
      (parseFloat(cs.paddingBottom) || 0);
    // 量自然高度前先还原成 1:1，否则会把上一轮的补偿宽度量进去
    pageEl.style.width = "";
    const natural = pageEl.offsetHeight;
    // 只差 1px 以内不缩，避免亚像素来回抖
    if (available <= 0 || natural <= available + 1) {
      pageEl.style.width = "";
      pageEl.style.scale = "";
      pageEl.style.transform = "";
      return;
    }
    const scale = Math.max(0.65, available / natural);
    pageEl.style.transformOrigin = "top left";
    pageEl.style.width = `${(100 / scale).toFixed(3)}%`;
    // 优先用独立的 scale 属性：它和页面入场动画的 transform 互不覆盖，
    // 老 WebView 不支持时再退回 transform。
    if (typeof CSS === "undefined" || CSS.supports("scale", "0.9")) {
      pageEl.style.scale = scale.toFixed(4);
      pageEl.style.transform = "";
    } else {
      pageEl.style.scale = "";
      pageEl.style.transform = `scale(${scale.toFixed(4)})`;
    }
  });

  return (
    <div className="page home-page" ref={pageRef}>
      <div className="home-hero">
        <div className="home-weather">
          <div className="home-weather-glyph">
            <WeatherGlyph icon={weather?.icon ?? "cloudy"} size={44} />
          </div>
          <div>
            <div className="home-weather-temp">
              {weather ? `${Math.round(weather.temp)}°` : "--°"}
              <span className="home-weather-desc">{weather?.desc ?? "天气加载中"}</span>
            </div>
            <div className="home-weather-meta">
              <span>
                <Icon name="mapPin" size={11} /> {weather?.city ?? config.weatherCity}
              </span>
              <span>体感 {weather ? Math.round(weather.feelsLike) : "--"}°</span>
              <span>
                高 {weather?.daily[0] ? Math.round(weather.daily[0].max) : "--"}° / 低{" "}
                {weather?.daily[0] ? Math.round(weather.daily[0].min) : "--"}°
              </span>
            </div>
          </div>
        </div>
        <div className="home-clock">
          <div className="home-clock-time">{fmtClock(now)}</div>
          <div className="home-clock-date">{fmtDate(now)}</div>
        </div>
      </div>

      <div className="home-section-head">
        <div className="home-section-title">
          <Icon name="quota" size={14} />
          <span>Agent 额度</span>
          <span className="home-section-count">{agents.length}</span>
        </div>
        <div className="home-section-actions">
          <button
            type="button"
            className={`mini-icon-btn ${refreshing ? "spinning" : ""}`}
            onClick={onRefresh}
            title="刷新额度"
            aria-label="刷新额度"
          >
            <Icon name="refresh" size={13} />
          </button>
          <button type="button" className="mini-text-btn" onClick={() => onNavigate("settings")}>
            设置
          </button>
        </div>
      </div>

      {agents.length === 0 ? (
        <div className="agent-grid">
          <div className="agent-card skeleton-card">
            <div className="skeleton-line w60" />
            <div className="skeleton-line w90" />
            <div className="skeleton-line w45" />
          </div>
          <div className="agent-card skeleton-card">
            <div className="skeleton-line w50" />
            <div className="skeleton-line w80" />
            <div className="skeleton-line w35" />
          </div>
        </div>
      ) : (
        <div
          className="agent-pager"
          onWheel={(e) => {
            if (pageCount <= 1) return;
            const delta = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
            if (Math.abs(delta) < 4) return;
            e.preventDefault();
            e.stopPropagation();
            turnPage(delta > 0 ? 1 : -1);
          }}
        >
          <div className="agent-pager-viewport">
            <div className="agent-pager-track" style={{ transform: `translateX(-${safePage * 100}%)` }}>
              {pageAgents.map((items, index) => (
                <div className="agent-grid agent-page" key={index} aria-hidden={index !== safePage}>
                  {items.map((agent) => (
                    <AgentCard key={agent.id} agent={agent} compact />
                  ))}
                </div>
              ))}
            </div>
          </div>
          {pageCount > 1 ? (
            <div className="agent-page-dots" role="tablist" aria-label="额度分页">
              {Array.from({ length: pageCount }, (_, index) => (
                <button
                  key={index}
                  type="button"
                  className={`agent-page-bar ${index === safePage ? "active" : ""}`}
                  onClick={() => setPage(index)}
                  aria-label={`第 ${index + 1} 页`}
                  aria-selected={index === safePage}
                  role="tab"
                />
              ))}
            </div>
          ) : null}
        </div>
      )}

      {media.hasSession ? (
        <div className="media-strip">
          <div className="media-cover">
            {media.cover ? (
              <img src={media.cover} alt="" />
            ) : (
              <span className="media-cover-fallback">
                <Icon name="music" size={20} />
              </span>
            )}
          </div>
          <div className="media-main">
            <div className="media-title">{media.title || "未命名曲目"}</div>
            <div className="media-sub">
              {media.artist || media.appName}
              {media.album ? ` · ${media.album}` : ""}
            </div>
            {lyricLine ? (
              <div className="media-lyric">
                {lyricLine.trans || lyricLine.text}
              </div>
            ) : null}
            <div className="media-progress">
              <span style={{ width: `${media.duration > 0 ? (media.position / media.duration) * 100 : 0}%` }} />
            </div>
          </div>
          <div className="media-time">
            {fmtDuration(media.position)} / {fmtDuration(media.duration)}
          </div>
          <div className="media-controls">
            <button type="button" onClick={() => void runMediaAction("prev")} aria-label="上一首" disabled={!media.canPrev}>
              <Icon name="prev" size={15} />
            </button>
            <button type="button" className="primary" onClick={() => void runMediaAction("toggle")} aria-label="播放/暂停">
              <Icon name={media.playing ? "pause" : "play"} size={16} />
            </button>
            <button type="button" onClick={() => void runMediaAction("next")} aria-label="下一首" disabled={!media.canNext}>
              <Icon name="next" size={15} />
            </button>
          </div>
        </div>
      ) : null}

      <div className="quick-actions">
        <button type="button" className="quick-action" onClick={onToggleMute} title="静音">
          <Icon name={muted ? "volumeOff" : "volume"} size={15} />
          <span>{muted ? "取消静音" : "静音"}</span>
        </button>
        <button type="button" className="quick-action" onClick={onOpenMain} title="打开主窗口">
          <Icon name="external" size={15} />
          <span>主窗口</span>
        </button>
      </div>

      <div className="launcher-row">
        {config.launcher.slice(0, 6).map((item) => (
          <button
            key={`${item.name}-${item.path}`}
            type="button"
            className="launcher-item"
            title={item.path}
            onClick={() => onLaunch(item.path)}
          >
            <span className="launcher-icon">
              <Icon name={(item.icon as IconName) || "folder"} size={17} />
            </span>
            <span>{item.name}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

// ============================================================
// 消息
// ============================================================

const MODULE_META: Record<string, { label: string; icon: IconName; tone: string }> = {
  qq: { label: "QQ", icon: "message", tone: "#5eb7ff" },
  wechat: { label: "微信", icon: "message", tone: "#3ddc84" },
  mail: { label: "邮件", icon: "note", tone: "#ffb65c" },
  feishu: { label: "飞书", icon: "message", tone: "#7aa2ff" },
  dingtalk: { label: "钉钉", icon: "message", tone: "#5ea9ff" },
};

export function MessagesPage({
  notify,
  onMarkRead,
  onClear,
  onOpenItem,
  onOpenSettings,
}: {
  notify: NotifyState;
  onMarkRead: () => void;
  onClear: (kind?: string) => void;
  onOpenItem: (item: NotifyItem) => void;
  onOpenSettings: () => void;
}) {
  const now = useNow();
  const [filter, setFilter] = useState<string>("all");
  const counts = useMemo(() => {
    const map: Record<string, number> = {};
    for (const item of notify.items) {
      map[item.kind] = (map[item.kind] ?? 0) + (item.read ? 0 : 1);
    }
    return map;
  }, [notify.items]);
  const visible = useMemo(
    () => (filter === "all" ? notify.items : notify.items.filter((i) => i.kind === filter)),
    [filter, notify.items]
  );
  const kinds = useMemo(() => {
    const set = new Set<string>();
    for (const item of notify.items) set.add(item.kind);
    return Array.from(set);
  }, [notify.items]);

  return (
    <div className="page messages-page">
      <SectionHead
        title="消息"
        hint={`${notify.unread} 条未读 · 系统通知 + 飞书 / 钉钉浮窗`}
        action={
          <div className="head-actions">
            <button type="button" className="mini-text-btn" onClick={onMarkRead}>
              全部已读
            </button>
            <button type="button" className="mini-icon-btn" onClick={onOpenSettings} title="通知设置">
              <Icon name="settings" size={13} />
            </button>
          </div>
        }
      />
      <div className="module-tabs">
        <button type="button" className={filter === "all" ? "active" : ""} onClick={() => setFilter("all")}>
          全部
          {notify.unread > 0 ? <span className="module-count">{notify.unread}</span> : null}
        </button>
        {kinds.map((kind) => {
          const meta = MODULE_META[kind] ?? { label: kind, icon: "message" as IconName, tone: "#8fa3c8" };
          return (
            <button
              key={kind}
              type="button"
              className={filter === kind ? "active" : ""}
              onClick={() => setFilter(kind)}
            >
              <span className="module-dot" style={{ background: meta.tone }} />
              {meta.label}
              {counts[kind] ? <span className="module-count">{counts[kind]}</span> : null}
            </button>
          );
        })}
        {notify.items.length > 0 ? (
          <button type="button" className="module-clear" title="清空当前筛选" onClick={() => onClear(filter === "all" ? undefined : filter)}>
            <Icon name="trash" size={13} />
          </button>
        ) : null}
      </div>
      <div className="message-list">
        {visible.length === 0 ? (
          <EmptyState icon="message" title="没有新消息" hint="QQ / 微信 / 邮件读系统通知，飞书 / 钉钉读右下角通知浮窗" />
        ) : (
          visible.map((item) => {
            const meta = MODULE_META[item.kind] ?? { label: item.name, icon: "message" as IconName, tone: "#8fa3c8" };
            return (
              <button
                key={item.id}
                type="button"
                className={`message-item ${item.read ? "read" : "unread"}`}
                onClick={() => onOpenItem(item)}
                title={`打开${item.name}`}
              >
                <span className="message-icon" style={{ background: `${meta.tone}20`, color: meta.tone }}>
                  <Icon name={meta.icon} size={16} />
                </span>
                <div className="message-main">
                  <div className="message-top">
                    <span className="message-sender">{item.sender || item.name}</span>
                    <span className="message-app">{meta.label}</span>
                    <span className="message-time">{timeAgo(item.time, now)}</span>
                  </div>
                  <div className="message-body">{item.body || "新消息"}</div>
                </div>
                {!item.read ? <span className="unread-dot" /> : null}
              </button>
            );
          })
        )}
      </div>
    </div>
  );
}

// ============================================================
// 股票
// ============================================================

function Sparkline({ points, prevClose, height = 42 }: { points: number[]; prevClose: number; height?: number }) {
  if (points.length < 2) {
    return <div className="sparkline-empty" style={{ height }} />;
  }
  const min = Math.min(...points, prevClose || Infinity);
  const max = Math.max(...points, prevClose || -Infinity);
  const span = Math.max(0.0001, max - min);
  const w = 220;
  const step = w / (points.length - 1);
  const path = points
    .map((p, i) => `${i === 0 ? "M" : "L"}${(i * step).toFixed(1)},${(height - ((p - min) / span) * (height - 4) - 2).toFixed(1)}`)
    .join(" ");
  const up = points[points.length - 1] >= prevClose;
  return (
    <svg className="sparkline" viewBox={`0 0 ${w} ${height}`} preserveAspectRatio="none" aria-hidden="true">
      <defs>
        <linearGradient id={`spark-${up ? "up" : "down"}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={up ? "var(--stock-up)" : "var(--stock-down)"} stopOpacity="0.28" />
          <stop offset="100%" stopColor={up ? "var(--stock-up)" : "var(--stock-down)"} stopOpacity="0" />
        </linearGradient>
      </defs>
      <path
        d={`${path} L${w},${height} L0,${height} Z`}
        fill={`url(#spark-${up ? "up" : "down"})`}
        stroke="none"
      />
      <path d={path} fill="none" stroke={up ? "var(--stock-up)" : "var(--stock-down)"} strokeWidth="1.6" />
      {prevClose > 0 ? (
        <line
          x1="0"
          x2={w}
          y1={height - ((prevClose - min) / span) * (height - 4) - 2}
          y2={height - ((prevClose - min) / span) * (height - 4) - 2}
          stroke="var(--text-faint)"
          strokeWidth="1"
          strokeDasharray="3 4"
        />
      ) : null}
    </svg>
  );
}

/** 行情接口的美股键大小写不固定，比较时统一忽略大小写。 */
function sameStock(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

export function StocksPage({
  stocks,
  config,
  onAdd,
  onRemove,
  onRefresh,
}: {
  stocks: StocksState | null;
  config: IslandConfig;
  onAdd: (code: string) => void;
  onRemove: (code: string) => void;
  onRefresh: () => void;
}) {
  const now = useNow();
  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<{ code: string; name: string }[]>([]);
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<string | null>(stocks?.quotes[0]?.code ?? null);
  const [minute, setMinute] = useState<StockMinute | null>(null);
  const requestId = useRef(0);

  // 只显示仍在自选里的行情：后端刷新有一拍延迟，删除后不能靠等下一次
  // 轮询才消失；同时兼容缓存里 usaapl / 配置里 usAAPL 的大小写差异。
  const quotes = (stocks?.quotes ?? []).filter((q) =>
    config.stocks.some((code) => sameStock(code, q.code))
  );

  useEffect(() => {
    if (!quotes.length) {
      if (selected !== null) setSelected(null);
      return;
    }
    if (!selected || !quotes.some((q) => sameStock(q.code, selected))) {
      setSelected(quotes[0].code);
    }
    // quotes 是派生数组，依赖原始 stocks / config.stocks 即可。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, stocks, config.stocks]);

  useEffect(() => {
    if (!selected) return;
    const id = ++requestId.current;
    getStockMinute(selected)
      .then((data) => {
        if (id === requestId.current) setMinute(data);
      })
      .catch(() => {
        if (id === requestId.current) setMinute(null);
      });
  }, [selected]);

  const search = async () => {
    if (!query.trim()) return;
    setBusy(true);
    try {
      setResults(await searchStock(query));
    } catch {
      setResults([]);
    } finally {
      setBusy(false);
    }
  };

  const quote = quotes.find((q) => sameStock(q.code, selected ?? "")) ?? null;
  const up = (quote?.pct ?? 0) >= 0;
  const quoteTime = quote
    ? / /.test(quote.time)
      ? quote.time.split(" ").pop()?.slice(0, 5) ?? ""
      : quote.time.length >= 12
        ? `${quote.time.slice(8, 10)}:${quote.time.slice(10, 12)}`
        : quote.time
    : "";

  return (
    <div className="page stocks-page">
      <SectionHead
        title="股票"
        hint={
          quotes.length === 0
            ? "还没有自选，点右上角 + 添加"
            : stocks?.ok
              ? `${quotes.length} 只自选 · ${quotes[0]?.status ?? ""} · ${fmtClock(stocks.updatedAt)} 更新`
              : stocks?.error || "行情加载中"
        }
        action={
          <div className="head-actions">
            <button type="button" className="mini-icon-btn" onClick={() => setSearching((v) => !v)} title="添加自选">
              <Icon name={searching ? "close" : "plus"} size={13} />
            </button>
            <button type="button" className="mini-icon-btn" onClick={onRefresh} title="刷新行情">
              <Icon name="refresh" size={13} />
            </button>
          </div>
        }
      />

      {searching ? (
        <div className="stock-search">
          <div className="stock-search-row">
            <Icon name="search" size={14} />
            <input
              autoFocus
              value={query}
              placeholder="股票代码 / 名称，如 600519、腾讯"
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void search();
              }}
            />
            <button type="button" className="mini-text-btn" disabled={busy} onClick={() => void search()}>
              {busy ? "搜索中" : "搜索"}
            </button>
          </div>
          {results.length > 0 ? (
            <div className="search-results">
              {results.map((r) => (
                <button
                  key={r.code}
                  type="button"
                  className="search-result"
                  onClick={() => {
                    onAdd(r.code);
                    setSearching(false);
                    setQuery("");
                    setResults([]);
                  }}
                >
                  <span>{r.name}</span>
                  <span className="search-code">{r.code}</span>
                  <Icon name="plus" size={12} />
                </button>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}

      {quote ? (
        <div className={`stock-detail ${up ? "up" : "down"}`}>
          <div className="stock-detail-head">
            <div>
              <div className="stock-detail-name">
                {quote.name}
                <span className="stock-code">{quote.code.toUpperCase()}</span>
              </div>
              <div className="stock-detail-status">
                {quote.status} · {quoteTime}
              </div>
            </div>
            <div className="stock-detail-price">
              <div className="price">{quote.price.toFixed(2)}</div>
              <div className="change">
                {quote.chg >= 0 ? "+" : ""}
                {quote.chg.toFixed(2)} ({quote.pct >= 0 ? "+" : ""}
                {quote.pct.toFixed(2)}%)
              </div>
            </div>
          </div>
          <Sparkline points={minute?.points ?? []} prevClose={minute?.prevClose || quote.prevClose} height={64} />
          <div className="stock-stats">
            <span>今开 {quote.open.toFixed(2)}</span>
            <span>最高 {quote.high.toFixed(2)}</span>
            <span>最低 {quote.low.toFixed(2)}</span>
            <span>昨收 {quote.prevClose.toFixed(2)}</span>
          </div>
        </div>
      ) : null}

      {quotes.length === 0 ? (
        <div className="stock-empty">
          <Icon name="chart" size={18} />
          <span>还没有自选，点右上角 + 搜索添加</span>
        </div>
      ) : (
        <div className="stock-list">
          {quotes.map((q) => {
            const isUp = q.pct >= 0;
            return (
              <div
                key={q.code}
                className={`stock-row ${sameStock(selected ?? "", q.code) ? "active" : ""}`}
                onClick={() => setSelected(q.code)}
                role="button"
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === "Enter") setSelected(q.code);
                }}
              >
                <span className="stock-row-name">
                  <span className="stock-market">{q.market.toUpperCase()}</span>
                  {q.name}
                </span>
                <span className={`stock-row-pct ${isUp ? "up" : "down"}`}>
                  {isUp ? "+" : ""}
                  {q.pct.toFixed(2)}%
                </span>
                <span className="stock-row-price">{q.price.toFixed(2)}</span>
                <button
                  type="button"
                  className="stock-remove"
                  title={`移除 ${q.name}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    onRemove(q.code);
                  }}
                >
                  <Icon name="close" size={11} />
                </button>
              </div>
            );
          })}
        </div>
      )}
      <div className="page-footnote">
        自选 {config.stocks.length} 只 · 每 {config.stockIntervalSec}s 刷新 · 涨跌幅≥{config.stockAlertPct}% 在岛外提醒
        <span className="footnote-time">{fmtClock(now)}</span>
      </div>
    </div>
  );
}

// ============================================================
// 天气
// ============================================================

export function WeatherPage({
  weather,
  onRefresh,
  onOpenSettings,
}: {
  weather: WeatherState | null;
  onRefresh: () => void;
  onOpenSettings: () => void;
}) {
  if (!weather) {
    return (
      <div className="page">
        <EmptyState icon="weather" title="天气加载中" hint="正在向 Open-Meteo 请求当前天气与预报" />
      </div>
    );
  }

  const daily = weather.daily.slice(0, 5);
  const today = daily[0];
  const lo = daily.length ? Math.min(...daily.map((d) => d.min)) : 0;
  const hi = daily.length ? Math.max(...daily.map((d) => d.max)) : 1;
  const span = Math.max(1, hi - lo);
  const rain = weather.hourly.slice(0, 6).reduce((max, h) => Math.max(max, h.precip), 0);
  const facts: [string, string][] = [
    ["体感", `${Math.round(weather.feelsLike)}°`],
    ["湿度", `${Math.round(weather.humidity)}%`],
    ["风速", `${Math.round(weather.wind)} km/h`],
    ["降水", rain > 0 ? `${Math.round(rain)}%` : "—"],
    ["日出", weather.sunrise || "—"],
    ["日落", weather.sunset || "—"],
    ["最高", today ? `${Math.round(today.max)}°` : "—"],
    ["最低", today ? `${Math.round(today.min)}°` : "—"],
  ];
  const updated = weather.ok ? `更新于 ${fmtClock(weather.updatedAt)}` : weather.error;

  return (
    <div className="page weather-page">
      <div className="weather-top">
        <button type="button" className="weather-city" onClick={onOpenSettings} title="切换天气城市">
          <WeatherGlyph icon={weather.icon} size={17} />
          <b>{weather.city}</b>
          <span>{Math.round(weather.temp)}°</span>
        </button>
        <span className="weather-updated" title={updated}>
          {updated}
        </span>
        <button type="button" className="weather-refresh" onClick={onRefresh} title="刷新天气" aria-label="刷新天气">
          <Icon name="refresh" size={14} />
        </button>
      </div>

      <div className="weather-body">
        <div className="weather-now">
          <div className="weather-now-main">
            <WeatherGlyph icon={weather.icon} size={70} className="weather-now-glyph" />
            <span className="weather-temp">{Math.round(weather.temp)}°</span>
          </div>
          <div className="weather-desc">{weather.desc}</div>
          <div className="weather-cityline">
            {weather.city}
            {today ? ` · 最低 ${Math.round(today.min)}°  最高 ${Math.round(today.max)}°` : ""}
          </div>
        </div>

        <div className="weather-forecast">
          {daily.map((d, i) => {
            const start = Math.max(0, Math.min(100, ((d.min - lo) / span) * 100));
            const end = Math.max(start, Math.min(100, ((d.max - lo) / span) * 100));
            return (
              <div className={`forecast-row ${i === 0 ? "today" : ""}`} key={d.date}>
                <span className="forecast-day">{i === 0 ? "今天" : d.weekday}</span>
                <WeatherGlyph icon={d.icon} size={18} />
                <span className="forecast-min">{Math.round(d.min)}°</span>
                <span className="forecast-bar">
                  <i
                    style={{
                      clipPath: `inset(0 ${100 - end}% 0 ${start}% round 999px)`,
                    }}
                  />
                </span>
                <span className="forecast-max">{Math.round(d.max)}°</span>
              </div>
            );
          })}
        </div>
      </div>

      <div className="weather-facts">
        {facts.map(([label, value]) => (
          <div className="weather-fact" key={label}>
            <span>{label}</span>
            <b>{value}</b>
          </div>
        ))}
      </div>

      <div className="weather-hours">
        {weather.hourly.slice(0, 8).map((h) => (
          <div className="weather-hour" key={h.time}>
            <span className="weather-hour-label">{h.label.replace(":00", "时")}</span>
            <WeatherGlyph icon={h.icon} size={19} />
            <span className="weather-hour-temp">{Math.round(h.temp)}°</span>
            <span className={`weather-hour-rain ${h.precip >= 20 ? "show" : ""}`}>
              {h.precip >= 20 ? `${Math.round(h.precip)}%` : ""}
            </span>
          </div>
        ))}
      </div>

    </div>
  );
}

// ============================================================
// 控制
// ============================================================

export function ControlsPage({
  system,
  onVolume,
  onMute,
  onBrightness,
  onRadio,
  onOpenSettings,
  onOpenMain,
}: {
  system: SystemState;
  onVolume: (v: number) => void;
  onMute: () => void;
  onBrightness: (v: number) => void;
  onRadio: (kind: "wifi" | "bluetooth", on: boolean) => void;
  onOpenSettings: (kind: string) => void;
  onOpenMain: () => void;
}) {
  const [volume, setVolumeDraft] = useState(Math.round(system.volume * 100));
  const [brightness, setBrightnessDraft] = useState(Math.round(system.brightness ?? 0));
  const volumeTimer = useRef<number | null>(null);
  const brightnessTimer = useRef<number | null>(null);
  // 用户刚拖过滑块时压住外部回读：后端每拍回读音量/亮度，回读值可能比
  // 刚拖到的位置晚一拍（或是按下前的老值），直接灌进 draft 会让滑块来回跳。
  const localHold = useRef(0);

  useEffect(() => {
    if (performance.now() < localHold.current) return;
    setVolumeDraft(Math.round(system.volume * 100));
  }, [system.volume]);
  useEffect(() => {
    if (performance.now() < localHold.current) return;
    setBrightnessDraft(Math.round(system.brightness ?? 0));
  }, [system.brightness]);

  const changeVolume = (v: number) => {
    localHold.current = performance.now() + 1200;
    setVolumeDraft(v);
    if (volumeTimer.current != null) window.clearTimeout(volumeTimer.current);
    volumeTimer.current = window.setTimeout(() => onVolume(v / 100), 120);
  };
  const changeBrightness = (v: number) => {
    localHold.current = performance.now() + 1200;
    setBrightnessDraft(v);
    if (brightnessTimer.current != null) window.clearTimeout(brightnessTimer.current);
    brightnessTimer.current = window.setTimeout(() => onBrightness(v), 120);
  };

  const wifiOn = Boolean(system.wifi);
  const btOn = Boolean(system.bluetooth);
  const wifiSub = system.wifiAvailable ? (wifiOn ? "已连接" : "已关闭") : "未检测到 · 打开设置";
  const btSub = system.bluetoothAvailable ? (btOn ? "已开启" : "已关闭") : "未检测到 · 打开设置";
  const brightnessKnown = system.brightnessSupported || system.brightness != null;
  const brightnessPending = !brightnessKnown && system.updatedAt === 0;
  const brightnessText = brightnessKnown
    ? `${brightness}%`
    : brightnessPending
      ? "检测中"
      : "不可用";

  return (
    <div className="page controls-page">
      <div className="ctl-sliders">
        <div className="ctl-slider">
          <span className="ctl-slider-fill" style={{ width: `${volume}%` }} />
          <button
            type="button"
            className="ctl-slider-icon"
            onClick={onMute}
            title={system.muted ? "取消静音" : "静音"}
            aria-label={system.muted ? "取消静音" : "静音"}
          >
            <Icon name={system.muted ? "volumeOff" : "volume"} size={17} />
          </button>
          <input
            className="ctl-range"
            type="range"
            min={0}
            max={100}
            step={1}
            value={volume}
            aria-label="音量"
            onChange={(e) => changeVolume(Number(e.target.value))}
          />
          <span className={`ctl-slider-value ${volume > 74 ? "on-fill" : ""}`}>
            {system.muted ? "静音" : `${volume}%`}
          </span>
        </div>

        <div className={`ctl-slider ${brightnessKnown ? "" : "disabled"}`}>
          <span className="ctl-slider-fill" style={{ width: `${brightness}%` }} />
          <span className="ctl-slider-icon">
            <Icon name="brightness" size={17} />
          </span>
          <input
            className="ctl-range"
            type="range"
            min={0}
            max={100}
            step={1}
            value={brightness}
            disabled={!brightnessKnown}
            aria-label="亮度"
            onChange={(e) => changeBrightness(Number(e.target.value))}
          />
          <span
            className={`ctl-slider-value ${
              brightnessKnown && brightness > 74 ? "on-fill" : ""
            }`}
          >
            {brightnessText}
          </span>
        </div>
      </div>

      <div className="ctl-grid">
        <button
          type="button"
          className={`ctl-tile ${wifiOn && system.wifiAvailable ? "on" : ""}`}
          onClick={() => (system.wifiAvailable ? onRadio("wifi", !wifiOn) : onOpenSettings("wifi"))}
        >
          <span className="ctl-tile-icon">
            <Icon name="wifi" size={19} />
          </span>
          <span className="ctl-tile-copy">
            <b>Wi-Fi</b>
            <span>{wifiSub}</span>
          </span>
          {!system.wifiAvailable ? (
            <span className="ctl-tile-tail">
              <Icon name="external" size={12} />
            </span>
          ) : null}
        </button>

        <button
          type="button"
          className={`ctl-tile ${btOn && system.bluetoothAvailable ? "on" : ""}`}
          onClick={() =>
            system.bluetoothAvailable ? onRadio("bluetooth", !btOn) : onOpenSettings("bluetooth")
          }
        >
          <span className="ctl-tile-icon">
            <Icon name="bluetooth" size={19} />
          </span>
          <span className="ctl-tile-copy">
            <b>蓝牙</b>
            <span>{btSub}</span>
          </span>
          {!system.bluetoothAvailable ? (
            <span className="ctl-tile-tail">
              <Icon name="external" size={12} />
            </span>
          ) : null}
        </button>

        <button type="button" className="ctl-tile" onClick={onOpenMain}>
          <span className="ctl-tile-icon">
            <Icon name="quota" size={19} />
          </span>
          <span className="ctl-tile-copy">
            <b>主窗口</b>
            <span>额度与趋势</span>
          </span>
          <span className="ctl-tile-tail">
            <Icon name="external" size={12} />
          </span>
        </button>

        <button type="button" className="ctl-tile" onClick={() => onOpenSettings("home")}>
          <span className="ctl-tile-icon">
            <Icon name="settings" size={19} />
          </span>
          <span className="ctl-tile-copy">
            <b>系统设置</b>
            <span>声音 / 显示 / 网络</span>
          </span>
          <span className="ctl-tile-tail">
            <Icon name="external" size={12} />
          </span>
        </button>
      </div>

      {system.error ? (
        <div className="ctl-error">
          <Icon name="globe" size={13} />
          <span>{system.error}</span>
        </div>
      ) : null}
    </div>
  );
}

// ============================================================
// 剪贴板
// ============================================================

export function ClipboardPage({
  items,
  onCopy,
  onClear,
  onToast,
}: {
  items: ClipItem[];
  onCopy: (id: number) => void;
  onClear: () => void;
  onToast: (msg: string) => void;
}) {
  const now = useNow();
  return (
    <div className="page clipboard-page">
      <SectionHead
        title="剪贴板"
        hint={`最近 ${items.length} 条 · 只保存在本机内存`}
        action={
          items.length > 0 ? (
            <button type="button" className="mini-text-btn" onClick={onClear}>
              清空
            </button>
          ) : null
        }
      />
      {items.length === 0 ? (
        <EmptyState icon="clipboard" title="剪贴板还是空的" hint="复制文字或图片后会自动出现在这里" />
      ) : (
        <div className="clip-list">
          {items.map((item) => (
            <button
              type="button"
              className={`clip-item ${item.kind}`}
              key={item.id}
              onClick={() => {
                onCopy(item.id);
                onToast("已复制到剪贴板");
              }}
            >
              {item.kind === "image" ? (
                <span className="clip-thumb">
                  <img src={item.image} alt={item.preview} />
                </span>
              ) : (
                <span className="clip-icon">
                  <Icon name="note" size={15} />
                </span>
              )}
              <span className="clip-main">
                <span className="clip-preview">{item.preview || item.text}</span>
                <span className="clip-meta">
                  {item.kind === "image" ? `${item.width}×${item.height}` : "文本"} · {timeAgo(item.time, now)}
                </span>
              </span>
              <span className="clip-copy">
                <Icon name="copy" size={13} />
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// ============================================================
// 设置
// ============================================================

const HUD_FONT_MIN = 10;
const HUD_FONT_MAX = 18;
const HUD_FONT_DEFAULT = 13;
const HUD_FONT_PRESETS = [
  { value: 11.7, label: "小" },
  { value: 13, label: "标准" },
  { value: 14.3, label: "大" },
  { value: 16.25, label: "特大" },
];

function formatFontPx(px: number): string {
  if (Number.isInteger(px)) return String(px);
  return px.toFixed(2).replace(/0+$/u, "").replace(/\.$/u, "");
}

function FontSizeField({
  value,
  onCommit,
}: {
  value: number;
  onCommit: (px: number) => void;
}) {
  const [draft, setDraft] = useState(() => formatFontPx(value));
  const [error, setError] = useState("");
  const focusedRef = useRef(false);

  useEffect(() => {
    if (!focusedRef.current) {
      setDraft(formatFontPx(value));
      setError("");
    }
  }, [value]);

  const commit = () => {
    const next = parseFloat(draft);
    if (!Number.isFinite(next) || next < HUD_FONT_MIN || next > HUD_FONT_MAX) {
      setError(`请输入 ${HUD_FONT_MIN}–${HUD_FONT_MAX} 之间的字号`);
      setDraft(formatFontPx(value));
      return;
    }
    setError("");
    const clean = Math.round(next * 100) / 100;
    setDraft(formatFontPx(clean));
    onCommit(clean);
  };

  const step = (direction: 1 | -1) => {
    const snapped = Math.round(value * 2) / 2;
    const next = Math.max(HUD_FONT_MIN, Math.min(HUD_FONT_MAX, snapped + direction * 0.5));
    setDraft(formatFontPx(next));
    setError("");
    onCommit(next);
  };

  return (
    <>
      <div className="segmented">
        {HUD_FONT_PRESETS.map((preset) => {
          const active = Math.abs(value - preset.value) < 0.001;
          return (
            <button
              type="button"
              key={preset.value}
              className={active ? "active" : ""}
              onClick={() => {
                setDraft(formatFontPx(preset.value));
                setError("");
                onCommit(preset.value);
              }}
            >
              {preset.label}
              {active ? <Icon name="check" size={10} /> : null}
            </button>
          );
        })}
      </div>
      <div className="font-size-row">
        <button
          type="button"
          className="font-size-step"
          aria-label="减小字号"
          title="减小 0.5px"
          onClick={() => step(-1)}
        >
          −
        </button>
        <label className="font-size-field">
          <input
            type="number"
            inputMode="decimal"
            min={HUD_FONT_MIN}
            max={HUD_FONT_MAX}
            step={0.5}
            value={draft}
            aria-label="自定义基准字号"
            onFocus={() => {
              focusedRef.current = true;
            }}
            onChange={(event) => {
              setDraft(event.target.value);
              setError("");
            }}
            onBlur={() => {
              focusedRef.current = false;
              commit();
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                (event.currentTarget as HTMLInputElement).blur();
              }
            }}
          />
          <span>px</span>
        </label>
        <button
          type="button"
          className="font-size-step"
          aria-label="增大字号"
          title="增大 0.5px"
          onClick={() => step(1)}
        >
          +
        </button>
        <button
          type="button"
          className="font-size-reset"
          onClick={() => {
            setDraft(String(HUD_FONT_DEFAULT));
            setError("");
            onCommit(HUD_FONT_DEFAULT);
          }}
        >
          恢复默认
        </button>
      </div>
      {error ? <div className="font-size-error">{error}</div> : null}
      <div className="settings-row-hint">
        基准 {HUD_FONT_DEFAULT}px，可输入 {HUD_FONT_MIN}–{HUD_FONT_MAX}px；只缩放文字，图标和窗口尺寸不变。
      </div>
    </>
  );
}

export function SettingsPage({
  config,
  agents,
  onChange,
  onOpenMain,
  onQuit,
  onClose,
  onToast,
}: {
  config: IslandConfig;
  /** 当前读到的账号（已按用户的顺序排好），用于「排序」一栏 */
  agents: HudBarAgent[];
  onChange: (patch: Partial<IslandConfig>) => void;
  onOpenMain: () => void;
  onQuit: () => void;
  onClose: () => void;
  onToast: (msg: string) => void;
}) {
  const [provinceCode, setProvinceCode] = useState("");
  const [cityCode, setCityCode] = useState("");
  const [districtCode, setDistrictCode] = useState("");
  const [hotkey, setHotkey] = useState(config.hotkeyToggle);
  const [locating, setLocating] = useState(false);

  useEffect(() => setHotkey(config.hotkeyToggle), [config.hotkeyToggle]);

  useEffect(() => {
    const path = findDivisionPath("", config.weatherCity, config.weatherLabel || config.weatherCity);
    if (path.province) setProvinceCode(String(path.province.c));
    if (path.city) setCityCode(String(path.city.c));
    if (path.district) setDistrictCode(String(path.district.c));
  }, [config.weatherCity, config.weatherLabel]);

  const province = provinces().find((item) => String(item.c) === provinceCode);
  const cityList = citiesOf(provinceCode);
  const city = cityList.find((item) => String(item.c) === cityCode);
  const districtList = districtsOf(provinceCode, cityCode);

  const save = (patch: Partial<IslandConfig>, message = "已保存") => {
    onChange(patch);
    onToast(message);
  };

  const commitWeather = (
    nextProvince: string,
    nextCity: string,
    nextDistrict: string,
    override?: { lat: number; lon: number; label?: string }
  ) => {
    const cityNode = citiesOf(nextProvince).find((item) => String(item.c) === nextCity);
    if (!cityNode) return;
    const districtNode = districtsOf(nextProvince, nextCity).find(
      (item) => String(item.c) === nextDistrict
    );
    const shortCity = cityNode.n.replace(/(特别行政区|自治州|地区|市|盟)$/u, "");
    const label =
      override?.label ??
      (districtNode ? `${shortCity} · ${districtNode.n}` : shortCity);
    save(
      {
        weatherCity: cityNode.n,
        weatherLabel: label,
        weatherLat: override?.lat ?? null,
        weatherLon: override?.lon ?? null,
      },
      `天气位置已切换到 ${label}`
    );
  };

  const locate = async () => {
    if (locating) return;
    setLocating(true);
    try {
      const place = await locateCurrentPlace();
      if (!place.ok) throw new Error(place.error || "定位失败");
      const path = matchPlace(place.province, place.city);
      if (path.province && path.city) {
        const nextProvince = String(path.province.c);
        const nextCity = String(path.city.c);
        setProvinceCode(nextProvince);
        setCityCode(nextCity);
        setDistrictCode("");
        commitWeather(nextProvince, nextCity, "", {
          lat: place.lat,
          lon: place.lon,
          label: path.city.n.replace(/(特别行政区|自治州|地区|市|盟)$/u, ""),
        });
      } else {
        save(
          {
            weatherCity: place.city,
            weatherLabel: place.label || place.city,
            weatherLat: place.lat,
            weatherLon: place.lon,
          },
          `已定位到 ${place.label || place.city}`
        );
      }
    } catch (e) {
      onToast(`定位失败: ${String(e)}`);
    } finally {
      setLocating(false);
    }
  };

  // 上移 / 下移一个账号：把当前顺序整份落盘（不在表里的新账号自然排在后面）
  const moveAgent = (index: number, delta: number) => {
    const to = index + delta;
    if (to < 0 || to >= agents.length) return;
    const ids = agents.map((agent) => agent.id);
    const [moving] = ids.splice(index, 1);
    ids.splice(to, 0, moving);
    save({ agentOrder: ids }, "顺序已保存");
  };
  const agentSub = (agent: HudBarAgent) =>
    [agent.account, agent.plan].filter((part) => Boolean(part && part.trim())).join(" · ");

  return (
    <div className="page settings-page">
      <SectionHead title="设置" hint="所有改动即时生效，只保存在应用内部状态中" />

      <div className="settings-block">
        <div className="settings-label">排序</div>
        <div className="settings-row-hint">
          收起胶囊里的环和主页额度卡按这里的顺序排列；新出现的账号排到最后
        </div>
        {agents.length === 0 ? (
          <div className="settings-row-hint">还没读到账号数据</div>
        ) : (
          <div className="agent-order">
            {agents.map((agent, index) => (
              <div className="agent-order-row" key={agent.id}>
                <BrandMark source={agent.source} size={18} />
                <span className="agent-order-copy">
                  <b>{agentLabel(agent.source)}</b>
                  {agentSub(agent) ? <span>{agentSub(agent)}</span> : null}
                </span>
                <button
                  type="button"
                  className="agent-order-btn"
                  disabled={index === 0}
                  onClick={() => moveAgent(index, -1)}
                  title="上移"
                  aria-label={`${agentLabel(agent.source)} 上移`}
                >
                  <Icon name="arrowUp" size={12} />
                </button>
                <button
                  type="button"
                  className="agent-order-btn"
                  disabled={index === agents.length - 1}
                  onClick={() => moveAgent(index, 1)}
                  title="下移"
                  aria-label={`${agentLabel(agent.source)} 下移`}
                >
                  <Icon name="arrowDown" size={12} />
                </button>
              </div>
            ))}
          </div>
        )}
        {agents.length > 0 && (config.agentOrder?.length ?? 0) > 0 ? (
          <button
            type="button"
            className="agent-order-reset"
            onClick={() => save({ agentOrder: [] }, "已恢复默认顺序")}
          >
            恢复默认顺序
          </button>
        ) : null}
      </div>

      <div className="settings-block">
        <div className="settings-label">深色毛玻璃</div>
        <div className="settings-row">
          <div>
            <div className="settings-row-title">透明度</div>
            <div className="settings-row-hint">
              越高越能透出桌面壁纸；只调玻璃底色，不影响文字和图标
            </div>
          </div>
          <div className="settings-inline-value">{config.transparency}%</div>
        </div>
        <Slider
          value={config.transparency}
          min={0}
          max={90}
          step={1}
          onChange={(v) => onChange({ transparency: v })}
          ariaLabel="毛玻璃透明度"
        />
      </div>

      <div className="settings-block">
        <div className="settings-row">
          <div>
            <div className="settings-row-title">字体大小</div>
            <div className="settings-row-hint">
              收起胶囊和展开面板一起缩放；只改文字，不改图标和窗口尺寸
            </div>
          </div>
          <div className="settings-inline-value">{formatFontPx(config.fontSize ?? 13)} px</div>
        </div>
        <FontSizeField
          value={config.fontSize ?? 13}
          onCommit={(px) =>
            save({ fontSize: px }, `字体大小已设为 ${formatFontPx(px)}px`)
          }
        />
      </div>

      <div className="settings-block">
        <div className="settings-label">展开方式</div>
        <div className="segmented">
          {[
            { key: "hover", label: "悬停展开" },
            { key: "never", label: "长关" },
          ].map((mode) => (
            <button
              type="button"
              key={mode.key}
              className={config.expandMode === mode.key ? "active" : ""}
              onClick={() => save({ expandMode: mode.key as IslandConfig["expandMode"] })}
            >
              {mode.label}
              {config.expandMode === mode.key ? <Icon name="check" size={10} /> : null}
            </button>
          ))}
        </div>
        <div className="settings-row-hint">长关：悬停不自动展开，点击胶囊展开；移出后仍会收起。</div>
      </div>

      <div className="settings-block">
        <div className="settings-label">窗口</div>
        <div className="settings-row">
          <div>
            <div className="settings-row-title">全屏时自动隐藏</div>
            <div className="settings-row-hint">游戏 / 全屏视频时让出屏幕，退出后自动回来</div>
          </div>
          <Toggle
            checked={config.hideFullscreen}
            onChange={(next) => save({ hideFullscreen: next })}
            label="全屏时自动隐藏"
          />
        </div>
      </div>

      <div className="settings-block">
        <div className="settings-label-row">
          <span className="settings-label">天气位置</span>
          <button
            type="button"
            className={`settings-locate ${locating ? "busy" : ""}`}
            onClick={() => void locate()}
            disabled={locating}
          >
            <Icon name={locating ? "refresh" : "mapPin"} size={12} />
            {locating ? "定位中" : "获取当前位置"}
          </button>
        </div>
        <div className="weather-cascade">
          <label>
            <span>省份</span>
            <select
              value={provinceCode}
              onChange={(e) => {
                const next = e.target.value;
                setProvinceCode(next);
                const first = citiesOf(next)[0];
                setCityCode(first ? String(first.c) : "");
                setDistrictCode("");
              }}
            >
              <option value="">选择省份</option>
              {provinces().map((item) => (
                <option value={String(item.c)} key={item.c}>
                  {item.n}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>城市</span>
            <select
              value={cityCode}
              disabled={!provinceCode}
              onChange={(e) => {
                const next = e.target.value;
                setCityCode(next);
                setDistrictCode("");
                commitWeather(provinceCode, next, "");
              }}
            >
              <option value="">选择城市</option>
              {cityList.map((item) => (
                <option value={String(item.c)} key={item.c}>
                  {item.n}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>区县</span>
            <select
              value={districtCode}
              disabled={!cityCode}
              onChange={(e) => {
                const next = e.target.value;
                setDistrictCode(next);
                commitWeather(provinceCode, cityCode, next);
              }}
            >
              <option value="">全市</option>
              {districtList.map((item) => (
                <option value={String(item.c)} key={item.c}>
                  {item.n}
                </option>
              ))}
            </select>
          </label>
        </div>
        <div className="settings-row-hint">
          三级联动选择，无需输入。当前：{config.weatherLabel || city?.n || province?.n || config.weatherCity}
        </div>
      </div>

      <div className="settings-block">
        <div className="settings-label">股票</div>
        <div className="settings-row">
          <div>
            <div className="settings-row-title">涨跌提醒阈值</div>
            <div className="settings-row-hint">达到阈值时在收起态弹出一次提醒</div>
          </div>
          <div className="settings-inline-value">{config.stockAlertPct.toFixed(1)}%</div>
        </div>
        <Slider
          value={config.stockAlertPct}
          min={0.5}
          max={10}
          step={0.5}
          onChange={(v) => onChange({ stockAlertPct: v })}
          ariaLabel="涨跌提醒阈值"
        />
        <div className="settings-row">
          <div>
            <div className="settings-row-title">刷新间隔</div>
            <div className="settings-row-hint">行情轮询频率</div>
          </div>
          <div className="segmented small">
            {[3, 5, 10, 30].map((sec) => (
              <button
                type="button"
                key={sec}
                className={config.stockIntervalSec === sec ? "active" : ""}
                onClick={() => save({ stockIntervalSec: sec })}
              >
                {sec}s
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="settings-block">
        <div className="settings-label">快捷操作</div>
        <div className="settings-input-row">
          <input
            value={hotkey}
            placeholder="Ctrl+Alt+Shift+I"
            onChange={(e) => setHotkey(e.target.value)}
          />
          <button
            type="button"
            className="settings-save"
            onClick={() => save({ hotkeyToggle: hotkey.trim() || "Ctrl+Alt+Shift+I" }, "热键已保存")}
          >
            保存热键
          </button>
        </div>
        <div className="settings-row-hint">
          开关灵动岛的全局热键（Tauri accelerator 格式，当前显示为{" "}
          <b>{hotkeyText(hotkey.trim() || "Ctrl+Alt+Shift+I")}</b>）；被其它软件占用时不会生效。
        </div>
        <div className="settings-action-row">
          <button type="button" onClick={onOpenMain}>
            <Icon name="external" size={14} /> 打开主窗口
          </button>
          <button type="button" onClick={onClose}>
            <Icon name="eyeOff" size={14} /> 关闭灵动岛
          </button>
          <button type="button" className="danger" onClick={onQuit}>
            <Icon name="power" size={14} /> 退出 AgentHUD
          </button>
        </div>
        <div className="settings-footnote">
          全局热键 {config.hotkeyToggle} · 版本 1.8.1 · 灵感来自 Misaka273/wisland（MIT）
        </div>
      </div>
    </div>
  );
}
