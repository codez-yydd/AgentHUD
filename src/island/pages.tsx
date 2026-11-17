/** 灵动岛各页面：主页 / 消息 / 股票 / 天气 / 控制 / 剪贴板（设置已迁到独立窗口） */

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type WheelEvent as ReactWheelEvent,
} from "react";
import { getStockMinute, searchStock } from "./api";
import { CityPickerModal, type CityPick } from "./cityPickerModal";
import { ClashPanel } from "./ClashPanel";
import { PetFace, type CompactPointerSample } from "./PetFace";
import {
  KIND_COLORS,
  NOTIFY_BRAND_ICONS,
  NOTIFY_BRAND_LABELS,
  NOTIFY_MODULE_KEYS,
  NotifyBrandIcon,
} from "./notifyBrand";
import type {
  ClipItem,
  HudBarAgent,
  HudBarUsage,
  ExpandMode,
  IslandConfig,
  LauncherItem,
  MediaState,
  NotifyItem,
  NotifyState,
  StockMinute,
  StocksState,
  SystemState,
  WeatherCityState,
  WeatherState,
} from "./types";
import { tempUnitSuffix, windUnitSuffix } from "./types";
import {
  agentColor,
  agentLabel,
  BrandMark,
  EmptyState,
  fmtClock,
  fmtCountdown,
  fmtDate,
  fmtDuration,
  fmtMoney,
  Icon,
  type IconName,
  QuotaBar,
  quotaLabel,
  quotaTone,
  remainingPct,
  SectionHead,
  timeAgo,
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
          {(() => {
            // 拆分小字：充值 / 赠送 / 累计消费（DSH 本地登录条目有累计消费），
            // 用 " · " 连接，与充值/赠送保持同一小字风格。文案硬编码中文，
            // 与本文件其余余额文案一致。
            const b = agent.balance;
            const parts: string[] = [];
            if (b.toppedUp != null) parts.push(`充值 ${fmtMoney(b.toppedUp, b.currency)}`);
            if (b.granted != null) parts.push(`赠送 ${fmtMoney(b.granted, b.currency)}`);
            if (b.totalCost != null) parts.push(`累计消费 ${fmtMoney(b.totalCost, b.currency)}`);
            return parts.length > 0 ? <span className="balance-split">{parts.join(" · ")}</span> : null;
          })()}
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

/** 主页小形象相对收起态的放大倍率（CSS 缩放；指针坐标同比归一） */
const HOME_PET_SCALE = 2.3;

export function HomePage({
  usage,
  weather,
  media,
  muted,
  config,
  expandMode,
  passthrough,
  active,
  refreshing,
  onRefresh,
  onToggleMute,
  onToggleAlways,
  onTogglePassthrough,
  onMediaAction,
  onOpenMain,
  onLaunch,
  onAddLauncher,
  onLauncherContextMenu,
  liveOn,
  liveBusy = false,
  onToggleLive,
}: {
  usage: HudBarUsage | null;
  weather: WeatherState | null;
  media: MediaState;
  muted: boolean;
  config: IslandConfig;
  expandMode: ExpandMode;
  passthrough: boolean;
  /** 面板展开且页面可见：控制小形象 rAF 是否运行 */
  active: boolean;
  refreshing: boolean;
  onRefresh: () => void;
  onToggleMute: () => void;
  /** 「保持展开」：expandMode always ↔ hover */
  onToggleAlways: () => void;
  /** 「可穿透」：islandSetPassthrough，打开后面板会收起 */
  onTogglePassthrough: () => void;
  onMediaAction: (action: "toggle" | "next" | "prev") => void;
  onOpenMain: () => void;
  onLaunch: (path: string) => void;
  onAddLauncher: () => void;
  onLauncherContextMenu: (item: LauncherItem, x: number, y: number) => void;
  /** 实时字幕是否在聆听 */
  liveOn: boolean;
  /** 开始 / 停止请求进行中（防连点） */
  liveBusy?: boolean;
  /** 开始 / 停止实时字幕 */
  onToggleLive: () => void;
}) {
  const now = useNow();
  const agents = usage?.agents ?? [];
  const pageRef = useRef<HTMLDivElement>(null);
  const stripRef = useRef<HTMLDivElement | null>(null);
  const petRef = useRef<HTMLDivElement | null>(null);
  const petPointerRef = useRef<CompactPointerSample>({ x: 9999, y: 9999, inside: false });
  const [page, setPage] = useState(0);
  const wheelAt = useRef(0);
  // 所有额度条目（余额型 / 订阅窗口型）统一按 4 张/页（2×2）分页展示。
  const pageCount = Math.max(1, Math.ceil(agents.length / 4));
  const safePage = Math.min(page, pageCount - 1);
  const pageAgents = useMemo(
    () =>
      Array.from({ length: pageCount }, (_, index) =>
        agents.slice(index * 4, index * 4 + 4)
      ),
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

  // 主页小形象：面板展开时监听窗口指针，换算成「相对小形象中心」的坐标。
  // PetFace 内部按 scale 归一化视线/靠近阈值；页面隐藏或收起时父层传
  // active=false，rAF 会暂停。
  useEffect(() => {
    if (!active) return;
    const onMove = (event: PointerEvent) => {
      const rect = petRef.current?.getBoundingClientRect();
      if (!rect) return;
      // 主页小形象用 CSS 放大到 2.3 倍（PetFace 内部仍是 1x 逻辑坐标）：
      // 指针位移也按倍率归一，视线/靠近阈值才与收起态同一套手感。
      petPointerRef.current = {
        x: (event.clientX - (rect.left + rect.width / 2)) / HOME_PET_SCALE,
        y: (event.clientY - (rect.top + rect.height / 2)) / HOME_PET_SCALE,
        inside: true,
      };
    };
    window.addEventListener("pointermove", onMove);
    return () => window.removeEventListener("pointermove", onMove);
  }, [active]);

  // 面板高度按标签常量表变化（home 492 / 其它按内容），媒体卡 + 小形象 +
  // 四动作 + 额度卡 + 启动器可能把主页顶出滚动条。这里仍按**展开态**的内容盒
  // 把整页等比缩到刚好放下：不裁切、不出现滚动条（P0 逻辑，布局改版后继续复用）。
  //
  // 三条约束（改之前先读一遍，都是实测踩过的坑）：
  // 1. **在展开态宽度下量**：收起态侧边胶囊只有 70px 宽，按当前宽度量会让整页
  //    自然高度因超窄宽度暴涨，把主页缩到 0.65；展开后又要跳回正常比例——
  //    就是「打开 1~2 秒后字体突然变大」。这里始终把页面临时撑到目标宽度
  //    （638 = 面板 640 - 1px 边框 ×2）再量，收起态也能算出正确比例。
  // 2. **展开后冻结小波动**：数据刷新（媒体卡 / 账号数变化）会让自然高度小幅
  //    波动；已缩放显示时只要还装得下（留 24px 余量）就不动比例。
  // 3. 真正需要改比例时（首次量、内容明显变多）由 CSS transition 平滑过去，
  //    见 island.css 的 .home-page。
  const fitScaleRef = useRef(0);
  const fitPage = useCallback(() => {
    const pageEl = pageRef.current;
    const panelEl = pageEl?.closest<HTMLElement>(".island-panel");
    const islandEl = panelEl?.closest<HTMLElement>(".island");
    const contentEl = pageEl?.parentElement;
    if (!pageEl || !panelEl || !islandEl || !contentEl) return;
    const maxWidth = parseFloat(getComputedStyle(islandEl).maxWidth);
    const width = Number.isFinite(maxWidth) && maxWidth > 0 ? maxWidth - 2 : 638;
    const cs = getComputedStyle(contentEl);
    const available =
      panelEl.clientHeight -
      (panelEl.querySelector<HTMLElement>(".island-nav")?.clientHeight ?? 42) -
      (parseFloat(cs.paddingTop) || 0) -
      (parseFloat(cs.paddingBottom) || 0);
    if (width < 360 || available < 200) return;
    // 量自然高度前先把页面撑到展开态宽度，且还原成 1:1（不带上一轮的补偿）。
    // 关键机制：读 offsetHeight 会强制一次同步样式落账，把测量用的中间宽度
    // （638px）提交成补间基准；如果之后恢复 transition 和写最终宽度落在
    // 同一次样式变更里，浏览器就会从 638px 补间到目标宽度，横向挤压一次——
    // useLayoutEffect 每次渲染都跑（时钟每秒一跳），展开后便成了“挤一下又
    // 恢复”的无限循环。所以测量后必须先把宽度写回原值，并趁 transition 仍
    // 为 none 再次强制落账，让中间值永远当不成补间起点；决策无变化时整趟
    // 循环净样式为零，每秒的时钟渲染不再产生任何补间。
    const previousTransition = pageEl.style.transition;
    const previousWidth = pageEl.style.width;
    pageEl.style.transition = "none";
    pageEl.style.width = `${width}px`;
    const natural = pageEl.offsetHeight;
    const applied = fitScaleRef.current;
    let scale = applied || 1;
    let finalWidth: string;
    let finalScale: number | null = null; // null 表示本轮不改比例
    if (
      islandEl.classList.contains("open") &&
      applied > 0 &&
      applied < 0.999 &&
      natural * applied <= available + 24
    ) {
      finalWidth = `${(100 / applied).toFixed(3)}%`;
    } else if (natural * scale > available + 4) {
      // P0：主页自然高度已按 3 列/单行动作压实；默认字号 fit 下限 0.92，
      // 避免整页被缩到 0.86 后小字只剩 6.9–8.6px。大字号（>1.2 倍）仍允许
      // 继续缩，保留 fit 兜底、不裁切内容。
      const fsScale =
        parseFloat(getComputedStyle(pageEl).getPropertyValue("--fs-scale")) || 1;
      const floor = fsScale > 1.2 ? 0.78 : 0.92;
      scale = Math.min(scale, Math.max(floor, available / natural));
      fitScaleRef.current = scale;
      pageEl.style.transformOrigin = "top left";
      finalScale = scale;
      finalWidth = scale >= 0.999 ? "" : `${(100 / scale).toFixed(3)}%`;
    } else if (natural * scale < available * 0.9) {
      finalWidth =
        applied > 0 && applied < 0.999 ? `${(100 / applied).toFixed(3)}%` : "";
    } else {
      fitScaleRef.current = scale;
      pageEl.style.transformOrigin = "top left";
      finalScale = scale;
      finalWidth = scale >= 0.999 ? "" : `${(100 / scale).toFixed(3)}%`;
    }
    pageEl.style.width = previousWidth;
    void pageEl.offsetHeight;
    pageEl.style.transition = previousTransition;
    if (finalWidth !== previousWidth) {
      pageEl.style.width = finalWidth;
    }
    if (finalScale !== null) {
      if (finalScale >= 0.999) {
        pageEl.style.scale = "";
        pageEl.style.transform = "";
      } else if (typeof CSS === "undefined" || CSS.supports("scale", "0.9")) {
        pageEl.style.scale = finalScale.toFixed(4);
        pageEl.style.transform = "";
      } else {
        pageEl.style.scale = "";
        pageEl.style.transform = `scale(${finalScale.toFixed(4)})`;
      }
    }
  }, []);

  // 不写依赖数组：账号数量 / 媒体 / 字号任一变化都要重新量
  useLayoutEffect(() => {
    fitPage();
  });

  // 面板尺寸变化（展开/收起、拖拽调高）后补量一次：80ms 防抖吸收动画帧
  useEffect(() => {
    const contentEl = pageRef.current?.parentElement;
    if (!contentEl) return;
    let timer = 0;
    const schedule = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(fitPage, 80);
    };
    const observer = new ResizeObserver(schedule);
    observer.observe(contentEl);
    return () => {
      window.clearTimeout(timer);
      observer.disconnect();
    };
  }, [fitPage]);

  // 启动器横向条滚轮：把纵向滚轮折算成横向滚动（参考 LaunchStrip）
  const scrollLauncher = (event: ReactWheelEvent<HTMLDivElement>) => {
    const element = stripRef.current;
    if (!element || element.scrollWidth <= element.clientWidth) return;
    const delta = Math.abs(event.deltaY) > Math.abs(event.deltaX) ? event.deltaY : event.deltaX;
    if (Math.abs(delta) < 2) return;
    event.preventDefault();
    event.stopPropagation();
    element.scrollLeft += delta;
  };

  const progress =
    media.duration > 0
      ? Math.max(0, Math.min(100, (media.position / media.duration) * 100))
      : 0;
  const holdOpen = expandMode === "always";
  // 温度单位跟随数据源（imperial → °F），其余按摄氏度 °。
  const unit = tempUnitSuffix(weather);

  return (
    <div className="page home-page" ref={pageRef}>
      <div className="home-hero">
        <div className="home-weather">
          <div className="home-weather-glyph">
            <WeatherGlyph icon={weather?.icon ?? "cloudy"} size={40} />
          </div>
          <div>
            <div className="home-weather-temp">
              {weather ? `${Math.round(weather.temp)}${unit}` : "--°"}
              <span className="home-weather-desc">{weather?.desc ?? "天气加载中"}</span>
            </div>
            <div className="home-weather-meta">
              <span>
                <Icon name="mapPin" size={11} /> {weather?.city ?? config.weatherCity}
              </span>
              <span>体感 {weather ? Math.round(weather.feelsLike) : "--"}{unit}</span>
              <span>
                高 {weather?.daily[0] ? Math.round(weather.daily[0].max) : "--"}{unit} / 低{" "}
                {weather?.daily[0] ? Math.round(weather.daily[0].min) : "--"}{unit}
              </span>
            </div>
          </div>
        </div>
        <div className="home-clock">
          <div className="home-clock-time">{fmtClock(now)}</div>
          <div className="home-clock-date">{fmtDate(now)}</div>
        </div>
      </div>

      <div className="home-main">
        {/* 媒体卡：封面 58 + 标题 Elide + 只读进度条 + 上一首/播放暂停/下一首 */}
        <div className="home-media-card">
          <span className="home-media-cover">
            {media.cover ? (
              <img src={media.cover} alt="" draggable={false} />
            ) : (
              <Icon name="music" size={22} />
            )}
          </span>
          <span className="home-media-body">
            <span className="home-media-title" title={media.title || "未在播放"}>
              {media.hasSession ? media.title || "未命名曲目" : "未在播放"}
            </span>
            <span className="home-media-sub">
              {media.hasSession
                ? media.artist || media.appName || "系统媒体"
                : "打开播放器后自动接入系统媒体"}
            </span>
            <span className="home-media-progress" aria-hidden="true">
              <i style={{ width: `${progress}%` }} />
            </span>
            <span className="home-media-times">
              <span>{fmtDuration(media.position)}</span>
              <span>{fmtDuration(media.duration)}</span>
            </span>
          </span>
          <span className="home-media-controls">
            <button
              type="button"
              className="home-media-btn"
              onClick={() => onMediaAction("prev")}
              aria-label="上一首"
              disabled={!media.canPrev}
            >
              <Icon name="prev" size={15} />
            </button>
            <button
              type="button"
              className="home-media-btn is-primary"
              disabled={!media.canToggle}
              onClick={() => onMediaAction("toggle")}
              aria-label={media.playing ? "暂停" : "播放"}
            >
              <Icon name={media.playing ? "pause" : "play"} size={16} />
            </button>
            <button
              type="button"
              className="home-media-btn"
              onClick={() => onMediaAction("next")}
              aria-label="下一首"
              disabled={!media.canNext}
            >
              <Icon name="next" size={15} />
            </button>
          </span>
        </div>

        {/* 小形象：复用收起态的 PetFace，用 CSS 放大到 2.3 倍，眼睛跟随鼠标 */}
        <div className="home-pet" ref={petRef} aria-hidden="true">
          <span className="home-pet-canvas">
            <PetFace active={active} pointerRef={petPointerRef} />
          </span>
        </div>

        {/* 四动作：保持展开 / 可穿透 / 静音 / 实时字幕（开关，聆听中高亮） */}
        <div className="home-actions">
          <button
            type="button"
            className={`home-action ${holdOpen ? "is-on" : ""}`}
            onClick={onToggleAlways}
            title={holdOpen ? "恢复悬停展开" : "保持展开（常驻展开）"}
          >
            <Icon name={holdOpen ? "pin" : "eye"} size={17} />
            <span>保持展开</span>
          </button>
          <button
            type="button"
            className={`home-action ${passthrough ? "is-on" : ""}`}
            onClick={onTogglePassthrough}
            title="可穿透（右键菜单可关闭）"
          >
            <Icon name="monitor" size={17} />
            <span>可穿透</span>
          </button>
          <button
            type="button"
            className={`home-action ${muted ? "is-on" : ""}`}
            onClick={onToggleMute}
            title={muted ? "取消静音" : "静音"}
          >
            <Icon name={muted ? "volumeOff" : "volume"} size={17} />
            <span>{muted ? "已静音" : "静音"}</span>
          </button>
          <button
            type="button"
            className={`home-action ${liveOn ? "is-on" : ""}`}
            onClick={onToggleLive}
            disabled={liveBusy}
            title={liveOn ? "停止实时字幕（收起后显示最新一句）" : "实时字幕（开始聆听）"}
            aria-pressed={liveOn}
          >
            <Icon name="mic" size={17} />
            <span>实时字幕</span>
            {liveOn ? <em className="home-action-badge is-live">聆听中</em> : null}
          </button>
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
          <button
            type="button"
            className="mini-icon-btn"
            onClick={onOpenMain}
            title="打开主窗口"
            aria-label="打开主窗口"
          >
            <Icon name="external" size={13} />
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
                <div
                  className={`agent-grid agent-page ${
                    items.length === 2 ? "is-two" : items.length === 1 ? "is-one" : ""
                  }`}
                  key={index}
                  aria-hidden={index !== safePage}
                >
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

      {/* 启动器：横向条（高 46、图标 44），滚轮横向滚动，点击打开 */}
      <div
        className="launcher-strip"
        ref={stripRef}
        onWheel={scrollLauncher}
        title="启动器（滚轮横向滚动）"
      >
        <button
          type="button"
          className="launcher-strip-item"
          title="添加应用或文件"
          aria-label="添加应用或文件"
          onClick={onAddLauncher}
        >
          <Icon name="plus" size={20} />
        </button>
        {config.launcher.map((item, index) => (
          <button
            key={`${item.name}-${item.path}-${index}`}
            type="button"
            className="launcher-strip-item"
            title={`${item.name} · ${item.path}`}
            aria-label={item.name}
            onClick={() => onLaunch(item.path)}
            onContextMenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
              onLauncherContextMenu(item, event.clientX, event.clientY);
            }}
          >
            <Icon name={(item.icon as IconName) || "folder"} size={20} />
          </button>
        ))}
      </div>
    </div>
  );
}

// ============================================================
// 消息
// ============================================================

const DEFAULT_NOTIFY_MODULES = ["qq", "wechat", "mail"];

function normalizeModules(value: string[] | undefined): string[] {
  const wanted = new Set((value ?? DEFAULT_NOTIFY_MODULES).map((kind) => kind.trim().toLowerCase()));
  return NOTIFY_MODULE_KEYS.filter((kind) => wanted.has(kind));
}

function moduleMeta(kind: string): { label: string; icon: IconName; tone: string } {
  return {
    label: NOTIFY_BRAND_LABELS[kind] ?? kind,
    icon: kind === "mail" ? "note" : "message",
    tone: KIND_COLORS[kind] ?? KIND_COLORS.other,
  };
}

/** 通知时间：Rust 是 UNIX 秒；mock / 个别事件可能给毫秒，统一成 HH:MM */
function moduleClock(ts: number): string {
  if (!Number.isFinite(ts) || ts <= 0) return "";
  const date = new Date(ts > 1e12 ? ts : ts * 1000);
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/** 历史行描述规则（参考 compact.py / app.py 的通知口径） */
function messageDescription(item: NotifyItem): string {
  const body = (item.body ?? "").trim();
  const sender = (item.sender ?? "").trim();
  if (body) return sender ? `${sender}：${body}` : body;
  if (item.kind === "mail") return "新邮件";
  if (sender) return `${sender} 发来消息`;
  return "新消息";
}

function messageSortTime(item: NotifyItem): number {
  return item.time > 1e12 ? item.time : item.time * 1000;
}

type MsgMenu =
  | { mode: "plus"; x: number; y: number; kind?: undefined }
  | { mode: "tile"; x: number; y: number; kind: string }
  | { mode: "clear"; x: number; y: number; kind?: undefined };

export function MessagesPage({
  notify,
  config,
  onMarkReadAll,
  onMarkReadKind,
  onClear,
  onModulesChange,
  onOpenItem,
  onOpenModule,
  onOpenSettings,
}: {
  notify: NotifyState;
  config: IslandConfig;
  /** 每个显示模块调 island_notify_mark_read_kind（只清未读，不删历史） */
  onMarkReadAll: (kinds: string[]) => void;
  onMarkReadKind: (kind: string) => void;
  onClear: (kind?: string) => void;
  /** 写 config.notifyModules（"+" 添加 / 右键移除） */
  onModulesChange: (modules: string[]) => void;
  onOpenItem: (item: NotifyItem) => void;
  onOpenModule: (kind: string, exe?: string) => void;
  onOpenSettings: () => void;
}) {
  const pageRef = useRef<HTMLDivElement | null>(null);
  const addBtnRef = useRef<HTMLButtonElement | null>(null);
  const [menu, setMenu] = useState<MsgMenu | null>(null);
  const enabled = useMemo(() => normalizeModules(config.notifyModules), [config.notifyModules]);
  const enabledSet = useMemo(() => new Set(enabled), [enabled]);
  const visibleItems = useMemo(
    () =>
      notify.items
        .filter((item) => enabledSet.has(item.kind) && (item.kind !== "wechat" || !item.read))
        .sort((a, b) => messageSortTime(b) - messageSortTime(a)),
    [notify.items, enabledSet]
  );
  const latestWechat = visibleItems.find((item) => item.kind === "wechat");
  const wechatUnread = latestWechat
    ? notify.wechatUnread ?? visibleItems.filter((item) => item.kind === "wechat").length
    : 0;
  const history = useMemo(() => {
    const others = visibleItems.filter((item) => item.kind !== "wechat");
    if (!latestWechat || !wechatUnread) return others.slice(0, 30);
    return [...others.slice(0, 29), latestWechat]
      .sort((a, b) => messageSortTime(b) - messageSortTime(a));
  }, [visibleItems, latestWechat, wechatUnread]);
  const stats = useMemo(() => {
    const map: Record<string, { unread: number; last: number }> = {};
    for (const item of visibleItems) {
      const entry = map[item.kind] ?? { unread: 0, last: 0 };
      if (!item.read) entry.unread += 1;
      entry.last = Math.max(entry.last, messageSortTime(item));
      map[item.kind] = entry;
    }
    if (map.wechat) map.wechat.unread = wechatUnread;
    return map;
  }, [visibleItems, wechatUnread]);
  const missingModules = NOTIFY_MODULE_KEYS.filter((kind) => !enabledSet.has(kind));
  const denseTiles = enabled.length === NOTIFY_MODULE_KEYS.length;

  const closeMenu = () => setMenu(null);

  // Esc 关闭页面菜单（点击外部由 backdrop 负责）
  useEffect(() => {
    if (!menu) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenu(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu]);

  /** 以 client 坐标打开页面内菜单（坐标按页面左上角换算，避免面板 transform 偏移） */
  const openMenu = (clientX: number, clientY: number, next: Omit<MsgMenu, "x" | "y">) => {
    const rect = pageRef.current?.getBoundingClientRect();
    if (!rect) return;
    const x = Math.max(6, Math.min(clientX - rect.left, Math.max(6, rect.width - 188)));
    const y = Math.max(6, Math.min(clientY - rect.top, Math.max(6, rect.height - 140)));
    setMenu({ ...next, x, y } as MsgMenu);
  };

  const handlePageContextMenu = (event: ReactMouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    openMenu(event.clientX, event.clientY, { mode: "clear" });
  };

  const handleTileContextMenu = (event: ReactMouseEvent, kind: string) => {
    event.preventDefault();
    event.stopPropagation();
    openMenu(event.clientX, event.clientY, { mode: "tile", kind });
  };

  const handlePlusMenu = () => {
    const rect = addBtnRef.current?.getBoundingClientRect();
    if (!rect) return;
    openMenu(rect.right - 178, rect.bottom + 6, { mode: "plus" });
  };

  const addModule = (kind: string) => {
    const next = normalizeModules([...enabled, kind]);
    onModulesChange(next);
    closeMenu();
  };

  const removeModule = (kind: string) => {
    onModulesChange(normalizeModules(enabled.filter((item) => item !== kind)));
    closeMenu();
  };

  const openModule = (kind: string) => {
    const latest = visibleItems.find((item) => item.kind === kind);
    onOpenModule(kind, latest?.exe);
    onMarkReadKind(kind);
  };

  return (
    <div
      className="page messages-page"
      ref={pageRef}
      onContextMenu={handlePageContextMenu}
    >
      <SectionHead
        title="消息"
        hint={`${notify.unread} 条未读 · ${enabled.length} 个模块`}
        action={
          <div className="head-actions msg-head-actions">
            <button
              type="button"
              ref={addBtnRef}
              className="mini-icon-btn msg-add"
              onClick={handlePlusMenu}
              title="添加消息模块"
              aria-label="添加消息模块"
            >
              <Icon name="plus" size={13} />
            </button>
            <button
              type="button"
              className="mini-text-btn"
              onClick={() => onMarkReadAll(enabled)}
            >
              全部已读
            </button>
            <button
              type="button"
              className="mini-icon-btn"
              onClick={onOpenSettings}
              title="打开系统通知设置"
              aria-label="打开系统通知设置"
            >
              <Icon name="settings" size={13} />
            </button>
          </div>
        }
      />

      {/* 模块磁贴：84px，色圆图标 + 名称 + 未读数 + 最近一条 HH:MM */}
      <div className={`msg-tiles${denseTiles ? " is-dense" : ""}`}>
        {enabled.map((kind) => {
          const meta = moduleMeta(kind);
          const stat = stats[kind];
          return (
            <button
              key={kind}
              type="button"
              className="msg-tile"
              onClick={() => openModule(kind)}
              onContextMenu={(event) => handleTileContextMenu(event, kind)}
              title={`${meta.label} · 右键管理模块`}
            >
              <span className="msg-tile-head">
                {NOTIFY_BRAND_ICONS[kind] ? (
                  <NotifyBrandIcon kind={kind} size={denseTiles ? 21 : 30} />
                ) : (
                  <span className="msg-tile-icon" style={{ background: meta.tone }}>
                    <Icon name={meta.icon} size={15} />
                  </span>
                )}
                <span className="msg-tile-name">{meta.label}</span>
              </span>
              <span className="msg-tile-foot">
                <b className={stat?.unread ? "has-unread" : ""} style={stat?.unread ? { color: meta.tone } : undefined}>
                  {stat?.unread ?? 0}
                </b>
                <em>{stat ? moduleClock(stat.last) : ""}</em>
              </span>
            </button>
          );
        })}
        {enabled.length === 0 ? (
          <div className="msg-tiles-empty">点右上角 + 添加消息模块</div>
        ) : null}
      </div>

      {/* 历史：最多 30 行 × 30px；点击仍打开来源应用（AgentHUD 既有能力） */}
      <div className="msg-history" onContextMenu={handlePageContextMenu}>
        {history.length === 0 ? (
          <div className="msg-history-empty">
            <Icon name="message" size={18} />
            <span>没有新消息</span>
            <em>可在「灵动岛设置 → 消息与邮件」配置通知来源</em>
          </div>
        ) : (
          history.map((item) => {
            const meta = moduleMeta(item.kind);
            const isWechat = item.kind === "wechat";
            const description = isWechat ? `${wechatUnread} 条未读消息` : messageDescription(item);
            return (
              <button
                key={isWechat ? "wechat-summary" : item.id}
                type="button"
                className={`msg-history-row ${item.read ? "is-read" : ""}`}
                onClick={() => onOpenItem(item)}
                title={`${meta.label} · ${description}`}
              >
                {NOTIFY_BRAND_ICONS[item.kind] ? (
                  <NotifyBrandIcon kind={item.kind} size={18} />
                ) : (
                  <i style={{ background: meta.tone }} />
                )}
                <span className="msg-history-name">{meta.label}</span>
                <span className="msg-history-desc">{description}</span>
                <span className="msg-history-time">{moduleClock(item.time)}</span>
              </button>
            );
          })
        )}
      </div>

      {menu ? (
        <>
          <div
            className="msg-menu-backdrop"
            onPointerDown={closeMenu}
            onContextMenu={(event) => {
              event.preventDefault();
              closeMenu();
            }}
          />
          <div
            className="msg-menu"
            style={{ left: menu.x, top: menu.y }}
            onPointerDown={(event) => event.stopPropagation()}
          >
            {menu.mode === "plus" ? (
              missingModules.length === 0 ? (
                <button type="button" disabled>
                  所有模块都已添加
                </button>
              ) : (
                missingModules.map((kind) => {
                  const meta = moduleMeta(kind);
                  return (
                    <button key={kind} type="button" onClick={() => addModule(kind)}>
                      <i style={{ background: meta.tone }} />
                      添加 {meta.label}
                    </button>
                  );
                })
              )
            ) : menu.mode === "tile" ? (
              <>
                <button
                  type="button"
                  onClick={() => {
                    onClear(menu.kind);
                    closeMenu();
                  }}
                >
                  清空「{moduleMeta(menu.kind).label}」消息
                </button>
                <div className="msg-menu-divider" />
                <button type="button" onClick={() => removeModule(menu.kind)}>
                  移除「{moduleMeta(menu.kind).label}」模块
                </button>
              </>
            ) : (
              <>
                <button
                  type="button"
                  disabled={visibleItems.length === 0}
                  onClick={() => {
                    onClear(undefined);
                    closeMenu();
                  }}
                >
                  清空全部消息
                </button>
              </>
            )}
          </div>
        </>
      ) : null}
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
  onBind,
  onUnbind,
  onRemoveCity,
  onAddCity,
  onAddCityExact,
}: {
  weather: WeatherState | null;
  onRefresh: () => void;
  onBind: (city: WeatherCityState) => void;
  onUnbind: () => void;
  onRemoveCity: (city: WeatherCityState) => void;
  onAddCity: (query: string) => Promise<string>;
  onAddCityExact: (city: {
    name: string;
    province: string;
    lat: number;
    lon: number;
  }) => Promise<string>;
}) {
  const pageRef = useRef<HTMLDivElement | null>(null);
  const noteTimer = useRef<number | null>(null);
  const [selected, setSelected] = useState(0);
  // 城市选择弹层开关（省市联动 / 搜索 / 当前位置 / 手动输入统一由共享组件承载）。
  const [pickerOpen, setPickerOpen] = useState(false);
  const [note, setNote] = useState<{ text: string; tone: "ok" | "error" } | null>(null);
  const [menu, setMenu] = useState<{ index: number; x: number; y: number } | null>(null);
  const cities = weather?.cities ?? [];
  const cityCount = cities.length;

  // 城市列表变化（绑定 / 取消绑定 / 增删）后夹取选中下标，避免指向已消失的城市。
  useEffect(() => {
    setSelected((current) => (cityCount === 0 ? 0 : Math.min(current, cityCount - 1)));
  }, [cityCount]);

  // Esc 关闭右键菜单（城市选择弹层自己的 Esc 由共享组件处理）。
  useEffect(() => {
    if (!menu) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenu(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu]);

  useEffect(
    () => () => {
      if (noteTimer.current != null) window.clearTimeout(noteTimer.current);
    },
    []
  );

  const flashNote = (text: string, tone: "ok" | "error") => {
    setNote({ text, tone });
    if (noteTimer.current != null) window.clearTimeout(noteTimer.current);
    noteTimer.current = window.setTimeout(() => setNote(null), 3600);
  };

  /** 以 client 坐标打开页面内菜单（坐标按页面左上角换算，避免面板 transform 偏移） */
  const openMenu = (event: ReactMouseEvent, index: number) => {
    event.preventDefault();
    event.stopPropagation();
    const rect = pageRef.current?.getBoundingClientRect();
    if (!rect) return;
    const x = Math.max(6, Math.min(event.clientX - rect.left, Math.max(6, rect.width - 210)));
    const y = Math.max(6, Math.min(event.clientY - rect.top, Math.max(6, rect.height - 156)));
    setMenu({ index, x, y });
  };

  const closePicker = () => setPickerOpen(false);
  const openPicker = () => setPickerOpen(true);

  /** 省市联动 / 当前位置选中城市：带坐标直加，成功提示并关闭弹层，失败行内提示。 */
  const pickExact = async (city: CityPick) => {
    try {
      const message = await onAddCityExact(city);
      flashNote(message, "ok");
      closePicker();
    } catch (error) {
      flashNote(String(error), "error");
    }
  };

  /** 手动输入（海外城市 / 兜底）：走原有在线 geocode 流程；失败抛出由弹层行内提示。 */
  const submitManual = async (value: string) => {
    try {
      const message = await onAddCity(value);
      flashNote(message, "ok");
      closePicker();
    } catch (error) {
      flashNote(String(error), "error");
      throw error;
    }
  };

  if (!weather) {
    return (
      <div className="page">
        <EmptyState icon="weather" title="天气加载中" hint="正在请求当前天气与预报" />
      </div>
    );
  }

  const selectedCity = cityCount === 0 ? null : cities[Math.min(selected, cityCount - 1)];
  const menuCity = menu && menu.index < cityCount ? cities[menu.index] : null;
  const daily = selectedCity?.daily.slice(0, 5) ?? [];
  const today = daily[0];
  // 温度单位跟随数据源（imperial → °F），其余按摄氏度 °；风速同理（mph / km/h）。
  const unit = tempUnitSuffix(weather);
  const windUnit = windUnitSuffix(weather);
  const lo = daily.length ? Math.min(...daily.map((d) => d.min)) : 0;
  const hi = daily.length ? Math.max(...daily.map((d) => d.max)) : 1;
  const span = Math.max(1, hi - lo);
  const rain = selectedCity
    ? selectedCity.hourly.slice(0, 6).reduce((max, h) => Math.max(max, h.precip), 0)
    : 0;
  const facts: [string, string][] = selectedCity
    ? [
        ["体感", `${Math.round(selectedCity.feelsLike)}${unit}`],
        ["湿度", `${Math.round(selectedCity.humidity)}%`],
        ["风速", `${Math.round(selectedCity.wind)} ${windUnit}`],
        ["降水", rain > 0 ? `${Math.round(rain)}%` : "—"],
        ["日出", selectedCity.sunrise || "—"],
        ["日落", selectedCity.sunset || "—"],
        ["最高", today ? `${Math.round(today.max)}${unit}` : "—"],
        ["最低", today ? `${Math.round(today.min)}${unit}` : "—"],
      ]
    : [];
  // stale：取数失败但已回填最近成功数据，按有数据处理并给出顶部小提示。
  const hasData = Boolean(selectedCity && (selectedCity.daily.length > 0 || selectedCity.hourly.length > 0));
  const staleHint = selectedCity?.stale
    ? `更新失败，显示 ${fmtClock(selectedCity.updatedAt)} 数据${selectedCity.errorBrief ? `（${selectedCity.errorBrief}）` : ""}`
    : "";
  const updated = selectedCity
    ? selectedCity.ok
      ? `更新于 ${fmtClock(selectedCity.updatedAt)}`
      : staleHint || selectedCity.error || "更新失败"
    : "";

  const menuStatus = (city: WeatherCityState): string => {
    if (city.isMain) {
      if (!weather.followsIp) return "主城市：已绑定";
      return city.isIp ? "主城市：跟随公网 IP" : "主城市：IP 不可用，暂用自定义城市";
    }
    return city.isIp ? "当前网络城市（跟随公网 IP）" : "";
  };

  const bindCity = (city: WeatherCityState) => {
    setMenu(null);
    setSelected(0);
    onBind(city);
  };

  const unbindCity = () => {
    setMenu(null);
    setSelected(0);
    onUnbind();
  };

  const removeCity = (city: WeatherCityState) => {
    setMenu(null);
    setSelected(0);
    onRemoveCity(city);
  };

  return (
    <div className="page weather-page" ref={pageRef}>
      <div className="weather-top">
        <div className="weather-cities">
          {cities.map((city, index) => (
            <button
              key={city.name}
              type="button"
              className={`weather-chip ${index === selected ? "is-active" : ""}`}
              onClick={() => setSelected(index)}
              onContextMenu={(event) => openMenu(event, index)}
              title={`${city.label} · 右键管理城市`}
            >
              {city.isMain ? <span className="weather-chip-dot" /> : null}
              <b>{city.label}</b>
              <span className="weather-chip-temp">{city.ok || city.stale ? `${Math.round(city.temp)}${unit}` : "--°"}</span>
              {city.isIp ? <span className="weather-chip-ip">IP</span> : null}
            </button>
          ))}
        </div>
        <button
          type="button"
          className="weather-add"
          onClick={openPicker}
          title="添加城市"
          aria-label="添加城市"
        >
          <Icon name="plus" size={13} />
        </button>
        {note ? (
          <span className={`weather-note ${note.tone === "error" ? "error" : ""}`} title={note.text}>
            {note.text}
          </span>
        ) : updated ? (
          <span className="weather-updated" title={updated}>
            {updated}
          </span>
        ) : null}
        <button type="button" className="weather-refresh" onClick={onRefresh} title="刷新天气" aria-label="刷新天气">
          <Icon name="refresh" size={14} />
        </button>
      </div>

      {cityCount === 0 ? (
        <div className="weather-empty">
          <Icon name="weather" size={26} />
          <b>还没有城市</b>
          <span>点右上角 + 选省市或定位添加，或在设置 → 天气 → 城市列表里添加</span>
        </div>
      ) : !selectedCity || !hasData ? (
        <div className={`weather-empty ${selectedCity && !selectedCity.ok ? "is-error" : ""}`}>
          <Icon name="weather" size={26} />
          <b>{selectedCity && !selectedCity.ok ? `${selectedCity.label} 天气获取失败` : "暂无天气数据"}</b>
          <span>{selectedCity?.error || "点右上角刷新重试"}</span>
          {selectedCity?.error ? (
            <details className="weather-error-detail">
              <summary>错误详情</summary>
              <code>{selectedCity.error}</code>
            </details>
          ) : null}
        </div>
      ) : (
        <>
          {selectedCity.stale ? (
            <div className="weather-stale-banner" title={selectedCity.error}>
              <Icon name="refresh" size={11} />
              <span>{staleHint}</span>
            </div>
          ) : null}
          <div className="weather-body">
            <div className="weather-now">
              <div className="weather-now-main">
                <WeatherGlyph icon={selectedCity.icon} size={70} className="weather-now-glyph" />
                <span className="weather-temp">{Math.round(selectedCity.temp)}{unit}</span>
              </div>
              <div className="weather-desc">{selectedCity.desc}</div>
              <div className="weather-cityline">
                {selectedCity.label}
                {today ? ` · 最低 ${Math.round(today.min)}${unit}  最高 ${Math.round(today.max)}${unit}` : ""}
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
                    <span className="forecast-min">{Math.round(d.min)}{unit}</span>
                    <span className="forecast-bar">
                      <i
                        style={{
                          clipPath: `inset(0 ${100 - end}% 0 ${start}% round 999px)`,
                        }}
                      />
                    </span>
                    <span className="forecast-max">{Math.round(d.max)}{unit}</span>
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
            {selectedCity.hourly.slice(0, 8).map((h) => (
              <div className="weather-hour" key={h.time}>
                <span className="weather-hour-label">{h.label.replace(":00", "时")}</span>
                <WeatherGlyph icon={h.icon} size={19} />
                <span className="weather-hour-temp">{Math.round(h.temp)}{unit}</span>
                <span className={`weather-hour-rain ${h.precip >= 20 ? "show" : ""}`}>
                  {h.precip >= 20 ? `${Math.round(h.precip)}%` : ""}
                </span>
              </div>
            ))}
          </div>
        </>
      )}

      {menu && menuCity ? (
        <>
          <div
            className="msg-menu-backdrop"
            onPointerDown={() => setMenu(null)}
            onContextMenu={(event) => {
              event.preventDefault();
              setMenu(null);
            }}
          />
          <div
            className="msg-menu weather-menu"
            style={{ left: menu.x, top: menu.y }}
            onPointerDown={(event) => event.stopPropagation()}
          >
            {menuCity.isMain || menuCity.isIp ? (
              <button type="button" disabled className="weather-menu-status">
                {menuStatus(menuCity)}
              </button>
            ) : null}
            {!menuCity.isMain || weather.followsIp ? (
              <button type="button" onClick={() => bindCity(menuCity)}>
                绑定主城市
              </button>
            ) : null}
            {!weather.followsIp ? (
              <button type="button" onClick={unbindCity}>
                {menuCity.isMain || menuCity.isIp ? "取消绑定（恢复跟随公网 IP）" : "取消绑定"}
              </button>
            ) : null}
            {(menuCity.isMain ? weather.followsIp && menuCity.isUser : menuCity.isUser && !menuCity.isIp) ? (
              <>
                <div className="msg-menu-divider" />
                <button type="button" className="danger" onClick={() => removeCity(menuCity)}>
                  删除该城市
                </button>
              </>
            ) : null}
          </div>
        </>
      ) : null}

      {pickerOpen ? (
        <CityPickerModal
          onPick={(city) => void pickExact(city)}
          onManual={(value) => submitManual(value)}
          onClose={closePicker}
        />
      ) : null}
    </div>
  );
}

// ============================================================
// 控制
// ============================================================

export function ControlsPage({
  system,
  active,
  onVolume,
  onMute,
  onBrightness,
  onRadio,
  onOpenSettings,
  onOpenMain,
  onConfigureClash,
  onClashError,
}: {
  system: SystemState;
  active: boolean;
  onVolume: (v: number) => void;
  onMute: () => void;
  onBrightness: (v: number) => void;
  onRadio: (kind: "wifi" | "bluetooth", on: boolean) => void;
  onOpenSettings: (kind: string) => void;
  onOpenMain: () => void;
  onConfigureClash: () => void;
  onClashError: (error: string) => void;
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
    ? `${brightness}%${system.brightnessSoft ? "·软" : ""}`
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

      </div>

      <ClashPanel active={active} onConfigure={onConfigureClash} onError={onClashError} />

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
  limit,
  onCopy,
  onPin,
  onDelete,
  onClear,
  onLimit,
  onOcr,
  onToast,
}: {
  items: ClipItem[];
  limit: number;
  onCopy: (id: number) => void;
  onPin: (id: number, pinned: boolean) => void;
  onDelete: (id: number) => void;
  onClear: () => void;
  onLimit: (limit: number) => void;
  onOcr: (item: ClipItem) => void;
  onToast: (msg: string) => void;
}) {
  const now = useNow();
  const [query, setQuery] = useState("");
  const keyword = query.trim().toLowerCase();
  const visible = keyword
    ? items.filter(
        (item) =>
          item.text.toLowerCase().includes(keyword) ||
          item.preview.toLowerCase().includes(keyword)
      )
    : items;
  const pinnedCount = items.filter((item) => item.pinned).length;
  const searchable = items.some((item) => item.kind === "text");

  return (
    <div className="page clipboard-page">
      <SectionHead
        title="剪贴板"
        hint={`最近 ${items.length} 条 · 仅本机内存${pinnedCount ? ` · 已钉 ${pinnedCount}` : ""}`}
        action={
          items.some((item) => !item.pinned) ? (
            <button type="button" className="mini-text-btn" onClick={onClear}>
              清空
            </button>
          ) : null
        }
      />

      {items.length > 0 ? (
        <div className="clip-toolbar">
          {searchable ? (
            <label className="clip-search">
              <Icon name="search" size={11} />
              <input
                value={query}
                placeholder="搜索文本"
                onChange={(event) => setQuery(event.target.value)}
              />
              {query ? (
                <button
                  type="button"
                  className="clip-search-clear"
                  title="清空搜索"
                  aria-label="清空搜索"
                  onClick={() => setQuery("")}
                >
                  ×
                </button>
              ) : null}
            </label>
          ) : (
            <span className="clip-toolbar-hint">只保留图片</span>
          )}
          <label className="clip-limit" title="保留条数上限">
            <span>上限</span>
            <select
              value={limit}
              onChange={(event) => onLimit(Number(event.target.value))}
            >
              {[5, 10, 20, 50].map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </label>
        </div>
      ) : null}

      {items.length === 0 ? (
        <EmptyState icon="clipboard" title="剪贴板还是空的" hint="复制文字或图片后会自动出现在这里" />
      ) : visible.length === 0 ? (
        <EmptyState icon="search" title="没有匹配的条目" hint="换个关键词试试" />
      ) : (
        <div className="clip-list">
          {visible.map((item) => (
            <div
              className={`clip-item ${item.kind} ${item.pinned ? "pinned" : ""}`}
              key={item.id}
              role="button"
              tabIndex={0}
              onClick={() => {
                onCopy(item.id);
                onToast("已复制到剪贴板");
              }}
              onKeyDown={(event) => {
                if (event.key !== "Enter" && event.key !== " ") return;
                event.preventDefault();
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
                  {item.kind === "image" ? `${item.width}×${item.height}` : "文本"} ·{" "}
                  {timeAgo(item.time, now)}
                  {item.pinned ? " · 已钉" : ""}
                </span>
              </span>
              <span className="clip-actions">
                {item.kind === "image" ? (
                  <button
                    type="button"
                    className="clip-action"
                    title="识别文字并复制"
                    aria-label="识别文字并复制"
                    onClick={(event) => {
                      event.stopPropagation();
                      onOcr(item);
                    }}
                  >
                    <Icon name="note" size={12} />
                  </button>
                ) : null}
                <button
                  type="button"
                  className={`clip-action ${item.pinned ? "on" : ""}`}
                  title={item.pinned ? "取消钉住" : "钉住（不被挤出）"}
                  aria-label={item.pinned ? "取消钉住" : "钉住"}
                  onClick={(event) => {
                    event.stopPropagation();
                    onPin(item.id, !item.pinned);
                  }}
                >
                  <Icon name="pin" size={12} />
                </button>
                <button
                  type="button"
                  className="clip-action danger"
                  title="删除这条"
                  aria-label="删除这条"
                  onClick={(event) => {
                    event.stopPropagation();
                    onDelete(item.id);
                  }}
                >
                  <Icon name="trash" size={12} />
                </button>
                <span className="clip-copy" aria-hidden>
                  <Icon name="copy" size={13} />
                </span>
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
