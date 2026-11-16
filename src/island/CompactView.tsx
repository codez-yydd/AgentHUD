/**
 * 收起态视图（P1）：rings 仍由 IslandApp 的
 * CollapsedPill 渲染，本文件负责 clock / pet / music / activity 四类内容，
 * 以及四视图共用的未读角标、尺寸公式。
 *
 * 尺寸公式（窗口 672 → 内容上限 640）：
 * - clock  = 150 + (天气 ? 64 : 0) + (播放中 ? 26 : 14) × 角标数
 * - pet    = 150 + 26 × 角标数
 * - music  = 最宽歌词或标题 + 封面、控制键、音频条、角标与留白
 * - activity = 音量/亮度 280；其它按标题、副标题与图标宽度计算
 * 横向高度：clock/pet 34、music 36、activity 40、rings 54（由 IslandApp 写进玻璃）。
 */

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type MutableRefObject,
  type ReactNode,
} from "react";
import { fmtClock, Icon, type IconName, useNow, WeatherGlyph } from "./ui";
import { islandAudioPeak } from "./api";
import { PetFace, type CompactPointerSample } from "./PetFace";
import { KIND_COLORS, NOTIFY_BRAND_ICONS, NOTIFY_BRAND_LABELS, NotifyBrandIcon } from "./notifyBrand";
import { audioMotionLevel } from "./liquid";
import type { LyricLine } from "./ui";
import type { MediaState, WeatherState } from "./types";
import { LIVE_CAPTION_HOLD_MS, tempUnitSuffix } from "./types";
import { fontFamilyWithFallback, ISLAND_FONT_STACK } from "./fontFamily";

export type CompactViewKey = "rings" | "clock" | "pet" | "music";
export type CompactActivityKind = "notify" | "volume" | "stock";
export type BadgeCounts = Record<string, number>;

/** 实时字幕层：收起态显示最新一句（上日文 / 下中文）。 */
export interface CompactLiveCaption {
  /** 是否正在聆听（false = 已停止，不再占用胶囊） */
  on: boolean;
  jp: string;
  zh: string;
  /** 最近一条字幕的到达时间（Date.now()，本地时钟；超过 7 秒回原视图） */
  at: number;
}

/** 收起态临时提醒（activity）：优先级高于普通视图，到 until 自动消失。 */
export interface CompactActivity {
  id: number;
  kind: CompactActivityKind;
  until: number;
  title?: string;
  sub?: string;
  icon?: IconName;
  /** 通知类：来源模块 kind（qq/wechat/mail/feishu/dingtalk），有品牌图时左侧渲染品牌图标 */
  sourceKind?: string;
  /** 通知类：左侧图标圆的 kind 颜色（KIND_COLORS） */
  color?: string;
  /** 股票异动：红涨绿跌，用主题 token，不写死颜色 */
  tone?: "up" | "down";
  /** 音量提醒：0..1 + 静音 */
  value?: number;
  muted?: boolean;
}

/** 横向收起内容高度。 */
export const COMPACT_HEIGHT: Record<CompactViewKey | "activity", number> = {
  rings: 54,
  clock: 34,
  pet: 34,
  music: 36,
  activity: 40,
};

/** 实时字幕层高度：上日文 12px / 下中文 15px 两行 + 上下留白（参考项目 H_SUB）。 */
export const LIVE_CAPTION_HEIGHT = 60;

/** 角标从右往左的绘制顺序（参考 compact.py BADGE_ORDER） */
export const BADGE_ORDER = ["mail", "wechat", "qq", "feishu", "dingtalk", "dsh"] as const;

const COMPACT_MAX_WIDTH = 640;
// 对应 .compact-music 的内边距、26px 封面、24.6px 律动条、28px 按钮和三段 8px 间隔。
const MUSIC_FIXED_WIDTH = 123;
// 竖排内边距 31px、26px 封面、最高 18px 律动条、28px 按钮和三段 10px 间隔。
const MUSIC_FIXED_HEIGHT = 133;
const BRAND_BADGE_SIZE = 20;

let measureCanvas: HTMLCanvasElement | null = null;
/** 用离屏 canvas 估文本宽度：尺寸是常量公式的一部分，必须在渲染前可算。 */
export function measureText(text: string, font: string): number {
  if (typeof document === "undefined") return text.length * 7;
  if (!measureCanvas) measureCanvas = document.createElement("canvas");
  const ctx = measureCanvas.getContext("2d");
  if (!ctx) return text.length * 7;
  ctx.font = font;
  return ctx.measureText(text).width;
}

/** 角标：未读 > 0 的 kind，按 BADGE_ORDER 排，额外 kind 排在后面。 */
export function badgeList(counts: BadgeCounts): { kind: string; color: string }[] {
  const seen = new Set<string>();
  const out: { kind: string; color: string }[] = [];
  for (const kind of BADGE_ORDER) {
    seen.add(kind);
    if ((counts[kind] ?? 0) > 0) out.push({ kind, color: KIND_COLORS[kind] ?? KIND_COLORS.other });
  }
  for (const [kind, value] of Object.entries(counts)) {
    if (value > 0 && !seen.has(kind)) out.push({ kind, color: KIND_COLORS[kind] ?? KIND_COLORS.other });
  }
  return out;
}

function compactFont(weight: number, size: number, family?: string): string {
  return `${weight} ${size}px ${fontFamilyWithFallback(family, ISLAND_FONT_STACK)}`;
}

/**
 * 超宽时保留句尾（最新说出的话），左侧补省略号（参考项目 Qt 的 ElideLeft）。
 * 按字符后缀二分测宽，中日文混排都能整字截断，不会切半个假名。
 */
export function tailText(text: string, maxWidth: number, font: string): string {
  const clean = (text ?? "").replace(/\s+/g, " ").trim();
  if (!clean || maxWidth <= 0) return clean;
  if (measureText(clean, font) <= maxWidth) return clean;
  const ellipsis = "…";
  let low = 0;
  let high = clean.length;
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    const suffix = clean.slice(clean.length - mid);
    if (measureText(`${ellipsis}${suffix}`, font) <= maxWidth) low = mid;
    else high = mid - 1;
  }
  return low > 0 ? `${ellipsis}${clean.slice(clean.length - low)}` : ellipsis;
}

/** 整首歌词里按当前字体测得最宽的一行。 */
export function longestLyricText(lines: LyricLine[], family?: string): string {
  const font = compactFont(500, 13, family);
  let best = "";
  let bestWidth = -1;
  for (const line of lines) {
    const text = line.text?.trim();
    if (!text) continue;
    const width = measureText(text, font);
    if (width > bestWidth) {
      bestWidth = width;
      best = text;
    }
  }
  return best;
}

/** 收起态当前要显示的那一行文本（music 宽度公式与画布一致）。 */
export function compactText(media: MediaState, lyricLine: LyricLine | null): string {
  if (lyricLine?.text) return lyricLine.text;
  if (media.title) return media.artist ? `${media.title} · ${media.artist}` : media.title;
  return "未在播放";
}

interface CompactTypography {
  fontScale?: number;
  fontFamily?: string;
}

interface CompactMusicSizeOptions extends CompactTypography {
  badges: BadgeCounts;
  media: MediaState;
  lyricLine: LyricLine | null;
  /** 整首最宽歌词，避免切换歌词引起尺寸变化。 */
  musicText?: string;
}

function musicCopySize(opts: CompactMusicSizeOptions, fontSize: number): number {
  const font = compactFont(500, fontSize * (opts.fontScale ?? 1), opts.fontFamily);
  return Math.max(
    measureText(opts.musicText || compactText(opts.media, opts.lyricLine), font),
    measureText(compactText(opts.media, null), font)
  );
}

// 品牌图标之间隔 5px，离播放按钮 10px（横排 8px 间距 + 2px 外边距）。
function musicBadgeSize(count: number): number {
  return count ? BRAND_BADGE_SIZE * count + 5 * (count - 1) + 10 : 0;
}

function mediaPlaying(media: MediaState): boolean {
  return media.hasSession && media.playing && Boolean(media.title);
}

export function compactMusicHeightFor(opts: CompactMusicSizeOptions, maxHeight = 492): number {
  const badgesHeight = musicBadgeSize(badgeList(opts.badges).length);
  const contentHeight = MUSIC_FIXED_HEIGHT + badgesHeight + musicCopySize(opts, 12);
  return Math.min(maxHeight, Math.ceil(contentHeight + 2));
}

/** 普通视图宽度（rings 走原自适应测量，不在这里）。 */
export function compactWidthFor(
  view: CompactViewKey,
  opts: CompactMusicSizeOptions & {
    hasWeather: boolean;
    maxWidth?: number;
  }
): number {
  const badgeCount = badgeList(opts.badges).length;
  const badgeWidth = badgeCount * (view === "pet" || mediaPlaying(opts.media) ? BRAND_BADGE_SIZE + 6 : 14);
  const limit = opts.maxWidth ?? COMPACT_MAX_WIDTH;
  if (view === "clock") {
    return Math.min(limit, 150 + (opts.hasWeather ? 64 : 0) + badgeWidth);
  }
  if (view === "pet") return Math.min(limit, 150 + badgeWidth);
  if (view === "music") {
    const contentWidth = MUSIC_FIXED_WIDTH + musicBadgeSize(badgeCount) + musicCopySize(opts, 13);
    return Math.min(limit, Math.ceil(contentWidth + 2));
  }
  return Math.min(limit, 500);
}

/**
 * 实时字幕层宽度：按参考项目 W_TEXT，最宽 640，窗口/内容更窄时取可用宽度。
 * 字幕层始终居中铺满这个宽度（两行文字内部再按实测宽度截尾）。
 */
export function liveCaptionWidthFor(maxWidth = COMPACT_MAX_WIDTH): number {
  return Math.max(240, Math.min(COMPACT_MAX_WIDTH, maxWidth));
}

/** 音量保留 280px；提醒按文字实测宽度扩展到可用空间。 */
export function activityWidthFor(
  activity: CompactActivity | null,
  {
    fontScale = 1,
    fontFamily,
    maxWidth = COMPACT_MAX_WIDTH,
  }: CompactTypography & { maxWidth?: number } = {}
): number {
  if (!activity || activity.kind === "volume") return Math.min(maxWidth, 280);
  const titleWidth = measureText(activity.title ?? "", compactFont(650, 13 * fontScale, fontFamily));
  const subWidth = measureText(activity.sub ?? "", compactFont(400, 12 * fontScale, fontFamily));
  // 内边距 23px + 图标 26px + 图文间隔 10px；副标题另占 10px 间隔。
  const fixed = activity.sub ? 69 : 59;
  return Math.min(maxWidth, Math.max(220, Math.ceil(fixed + titleWidth + subWidth + 2)));
}

/** 未读角标按消息来源去重。 */
function CompactBadges({ counts, branded = false }: { counts: BadgeCounts; branded?: boolean }) {
  const entries = badgeList(counts);
  if (entries.length === 0) return null;
  return (
    <span className="compact-badges" role="group" aria-label={`未读 ${entries.length} 类`}>
      {entries.map(({ kind, color }) => {
        if (!branded) return <i key={kind} style={{ background: color }} />;
        const label = `${NOTIFY_BRAND_LABELS[kind] ?? "消息"} · ${counts[kind]} 条未读`;
        return (
          <span key={kind} className="compact-message-badge" role="img" aria-label={label} title={label}>
            {NOTIFY_BRAND_ICONS[kind] ? (
              <NotifyBrandIcon kind={kind} size={BRAND_BADGE_SIZE} />
            ) : (
              <Icon name="message" size={BRAND_BADGE_SIZE} />
            )}
          </span>
        );
      })}
    </span>
  );
}

function ClockView({ weather, badges, brandedBadges }: {
  weather: WeatherState | null;
  badges: BadgeCounts;
  brandedBadges: boolean;
}) {
  const now = useNow();
  // 温度单位跟随数据源（imperial → °F），其余按摄氏度 °。
  const unit = tempUnitSuffix(weather);
  return (
    <div className="compact-view compact-clock">
      <span className="compact-clock-time">{fmtClock(now)}</span>
      <span className="compact-clock-tail">
        {weather ? (
          <span className="compact-weather" title={`${weather.city} ${weather.desc}`}>
            <WeatherGlyph icon={weather.icon || "cloudy"} size={17} />
            <b>{Math.round(weather.temp)}{unit}</b>
          </span>
        ) : null}
        <CompactBadges counts={badges} branded={brandedBadges} />
      </span>
    </div>
  );
}

function PetView({
  badges,
  pointerRef,
  surpriseToken,
  pressed,
  active,
}: {
  badges: BadgeCounts;
  pointerRef: MutableRefObject<CompactPointerSample>;
  surpriseToken: number;
  pressed: boolean;
  active: boolean;
}) {
  return (
    <div className="compact-view compact-pet">
      <span className="compact-pet-body">
        <PetFace
          active={active}
          pointerRef={pointerRef}
          surpriseToken={surpriseToken}
          pressed={pressed}
        />
      </span>
      <CompactBadges counts={badges} branded />
    </div>
  );
}

function MusicView({
  media,
  lyricLine,
  lyricKey,
  badges,
  vertical,
  active,
  audioLevelRef,
  onTogglePlay,
}: {
  media: MediaState;
  lyricLine: LyricLine | null;
  lyricKey: string;
  badges: BadgeCounts;
  vertical: boolean;
  active: boolean;
  audioLevelRef: MutableRefObject<number>;
  onTogglePlay: () => void;
}) {
  const playing = mediaPlaying(media);
  const text = compactText(media, lyricLine);
  const isLyric = Boolean(lyricLine?.text);
  const wrapRef = useRef<HTMLSpanElement | null>(null);
  const lineRef = useRef<HTMLSpanElement | null>(null);
  const [marquee, setMarquee] = useState(false);
  const marqueeAnim = useRef<Animation | null>(null);

  // 换行：新行以 0.32s 上移淡入（key 变化触发 CSS 动画）
  const [lineState, setLineState] = useState({ key: lyricKey, text });
  useEffect(() => {
    if (lineState.key !== lyricKey) setLineState({ key: lyricKey, text });
    else if (lineState.text !== text) setLineState({ key: lyricKey, text });
  }, [lyricKey, text, lineState.key, lineState.text]);

  // 超宽跑马灯：34px/s，行间留 40px，先停 2s 再滚（参考 compact.py 的
  // period = (full + gap) / 34 + 2 秒停顿）。用 Web Animations 才能把
  // 「停 2s + 匀速 34px/s」写准；CSS 关键帧的百分比无法按文本宽度参数化。
  // 视口宽度在切视图时是 440ms 弹簧过渡，首帧量到的宽度可能还没到位，
  // 所以用 ResizeObserver 在宽度稳定后重算，避免正常长度被误判成超宽。
  useLayoutEffect(() => {
    const box = wrapRef.current;
    const line = lineRef.current;
    if (!box || !line) return;
    const measure = () => {
      const target = lineRef.current;
      const container = wrapRef.current;
      if (!target || !container) return;
      const overflow = vertical
        ? target.scrollHeight - container.clientHeight
        : target.scrollWidth - container.clientWidth;
      marqueeAnim.current?.cancel();
      marqueeAnim.current = null;
      if (overflow <= 2) {
        setMarquee(false);
        return;
      }
      setMarquee(true);
      if (typeof target.animate !== "function") return;
      const distance = overflow + 40;
      const holdMs = 2000;
      const scrollMs = (distance / 34) * 1000;
      const duration = holdMs + scrollMs;
      const translate = vertical ? "translateY" : "translateX";
      marqueeAnim.current = target.animate(
        [
          { transform: `${translate}(0px)`, offset: 0 },
          { transform: `${translate}(0px)`, offset: holdMs / duration },
          { transform: `${translate}(${-distance}px)`, offset: 1 },
        ],
        { duration, iterations: Infinity, easing: "linear" }
      );
    };
    measure();
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(box);
    observer?.observe(line);
    return () => {
      observer?.disconnect();
      marqueeAnim.current?.cancel();
      marqueeAnim.current = null;
    };
  }, [lineState.key, lineState.text, vertical]);

  // 律动条和液态轮廓共用约 30Hz 的音频采样；直接更新 DOM 和 ref。
  const barsRef = useRef([0.18, 0.18, 0.18, 0.18]);
  const barEls = useRef<Array<HTMLElement | null>>([]);
  useEffect(() => {
    const paintBars = () => {
      barEls.current.forEach((element, index) => {
        if (element) element.style.height = `${4 + 14 * (barsRef.current[index] ?? 0.18)}px`;
      });
    };
    if (!active || !playing) {
      // 无播放：停止轮询，条回到静止高度。
      barsRef.current = [0.18, 0.18, 0.18, 0.18];
      paintBars();
      return;
    }
    let alive = true;
    let timer = 0;
    let level = 0;
    let lastSoundAt = -Infinity;
    const tick = async () => {
      const raw = await islandAudioPeak().catch(() => 0);
      if (!alive) return;
      const now = performance.now();
      const sample = audioMotionLevel(raw);
      if (sample > 0) {
        lastSoundAt = now;
        level = level === 0 ? sample : level * 0.5 + sample * 0.5;
      } else if (now - lastSoundAt >= 100) {
        // 短暂的零值不打断波动，持续静音时逐步回到静止。
        level = level > 0.015 ? level * 0.55 : 0;
      }
      audioLevelRef.current = level;
      const t = now / 1000;
      barsRef.current = barsRef.current.map((value, index) => {
        const target =
          0.18 + 0.82 * level * (0.55 + 0.45 * Math.sin(t * (7.3 + index * 2.1) + index * 1.7) ** 2);
        return value + (target - value) * 0.35;
      });
      paintBars();
      timer = window.setTimeout(tick, 33);
    };
    void tick();
    return () => {
      alive = false;
      audioLevelRef.current = 0;
      window.clearTimeout(timer);
    };
  }, [active, playing, audioLevelRef]);

  return (
    <div className="compact-view compact-music">
      <span className="compact-cover">
        {media.cover ? (
          <img src={media.cover} alt="" draggable={false} />
        ) : (
          <Icon name="music" size={13} />
        )}
      </span>
      <span className="compact-music-text" ref={wrapRef}>
        <span
          key={lineState.key}
          className={`compact-music-line ${isLyric ? "is-lyric" : "is-title"}`}
          title={lineState.text}
        >
          <span
            ref={lineRef}
            className={`compact-music-line-inner ${marquee ? "is-marquee" : ""}`}
          >
            {lineState.text}
          </span>
        </span>
      </span>
      <span className={`compact-bars ${playing ? "is-playing" : ""}`} aria-hidden="true">
        {[0, 1, 2, 3].map((index) => (
          <i
            key={index}
            ref={(element) => {
              barEls.current[index] = element;
            }}
            style={{ height: "7px" }}
          />
        ))}
      </span>
      <button
        type="button"
        className="compact-play"
        disabled={!media.canToggle}
        onClick={(event) => {
          event.stopPropagation();
          onTogglePlay();
        }}
        title={playing ? "暂停" : "播放"}
        aria-label={playing ? "暂停" : "播放"}
      >
        <Icon name={playing ? "pause" : "play"} size={14} />
      </button>
      <CompactBadges counts={badges} branded />
    </div>
  );
}

function ActivityView({ activity }: { activity: CompactActivity }) {
  if (activity.kind === "volume") {
    const value = Math.max(0, Math.min(1, activity.value ?? 0));
    return (
      <div className="compact-view compact-activity is-volume">
        <span className="compact-activity-icon is-plain">
          <Icon name={activity.muted ? "volumeOff" : "volume"} size={15} />
        </span>
        <span className="compact-volume-track">
          <i style={{ width: `${Math.round(value * 100)}%` }} />
        </span>
        <b className="compact-volume-value">
          {activity.muted ? "静音" : `${Math.round(value * 100)}`}
        </b>
      </div>
    );
  }
  const icon: IconName =
    activity.icon ?? (activity.kind === "stock" ? "chart" : "message");
  // 通知类命中品牌图时用消息页同款 PNG chip；不再叠加通用 icon 与内联色底。
  const brandKind =
    activity.sourceKind && NOTIFY_BRAND_ICONS[activity.sourceKind]
      ? activity.sourceKind
      : null;
  return (
    <div className={`compact-view compact-activity act-${activity.kind}`}>
      <span
        className={`compact-activity-icon ${brandKind ? "is-brand" : ""} ${
          activity.tone ? `is-${activity.tone}` : ""
        } ${activity.color && !brandKind ? "is-tinted" : ""}`}
        style={activity.color && !brandKind ? { background: activity.color } : undefined}
      >
        {brandKind ? (
          <NotifyBrandIcon kind={brandKind} size={26} />
        ) : (
          <Icon name={icon} size={14} />
        )}
      </span>
      <span className="compact-activity-copy">
        <b className={activity.tone ? `tone-${activity.tone}` : ""}>{activity.title}</b>
        {activity.sub ? (
          <em className={activity.tone ? `tone-${activity.tone}` : ""}>{activity.sub}</em>
        ) : null}
      </span>
    </div>
  );
}

/**
 * 实时字幕层（收起态）：上日文 12px / 透明度 0.62，下中文 15px / 加粗。
 * 中文还没出来时显示「…」；超宽保留句尾（最新说出的话），用CSS动画 280ms 淡入。
 */
function LiveCaptionView({
  live,
  fontScale = 1,
}: {
  live: CompactLiveCaption;
  fontScale?: number;
}) {
  const boxRef = useRef<HTMLDivElement | null>(null);
  const [boxWidth, setBoxWidth] = useState(0);
  // 宽度要实测：胶囊宽度由 IslandApp 按 min(640, 可用宽度) 算，首帧也拿得到。
  useLayoutEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const measure = () => setBoxWidth(box.clientWidth);
    measure();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(box);
    return () => observer?.disconnect();
  }, []);
  const jpRaw = live.jp.trim();
  const zhRaw = live.zh.trim();
  // 左右各 18px 内边距；测宽按当前字号倍率（与 CSS 的 calc() 同口径）。
  const maxWidth = Math.max(60, boxWidth - 36);
  const jp =
    boxWidth > 0 ? tailText(jpRaw, maxWidth, compactFont(400, 12 * fontScale)) : jpRaw;
  const zh =
    boxWidth > 0 && zhRaw
      ? tailText(zhRaw, maxWidth, compactFont(650, 15 * fontScale))
      : zhRaw;
  return (
    <div className="compact-live" ref={boxRef} aria-live="polite">
      <span className="compact-live-jp" title={jpRaw}>
        {jp}
      </span>
      <span className="compact-live-zh" title={zhRaw || "正在翻译…"}>
        {zh || "…"}
      </span>
    </div>
  );
}

export interface CompactViewProps {
  /** 目标视图；实时字幕出现时会被字幕层覆盖（rings 也算） */
  view: CompactViewKey | "rings";
  activity: CompactActivity | null;
  media: MediaState;
  lyricLine: LyricLine | null;
  lyricKey: string;
  weather: WeatherState | null;
  badges: BadgeCounts;
  quota?: ReactNode;
  vertical: boolean;
  /** false = 展开 / 页面隐藏：暂停 pet rAF 与律动条 */
  active: boolean;
  pressed: boolean;
  surpriseToken: number;
  pointerRef: MutableRefObject<CompactPointerSample>;
  audioLevelRef: MutableRefObject<number>;
  onTogglePlay: () => void;
  /** 点击收起内容（除播放键外）→ 走和胶囊单击相同的延迟展开逻辑 */
  onActivate: (event: ReactMouseEvent<HTMLDivElement>) => void;
  /** 实时字幕层：on 且最后一句未超过 7 秒时覆盖原视图 */
  live?: CompactLiveCaption | null;
  /** 字号倍率（字幕截尾按实际字号测宽） */
  fontScale?: number;
}

export function CompactView({
  view,
  activity,
  media,
  lyricLine,
  lyricKey,
  weather,
  badges,
  quota,
  vertical,
  active,
  pressed,
  surpriseToken,
  pointerRef,
  audioLevelRef,
  onTogglePlay,
  onActivate,
  live = null,
  fontScale = 1,
}: CompactViewProps) {
  // 临时提醒（activity：音量 / 通知 / 股票异动）优先于字幕；字幕优先于
  // rings/clock/pet/music，最后一句后 7 秒回原视图。「at」是本地到达时间
  //（不是后端 ts），与本机时钟同源，避免时钟漂移。
  const showLive =
    activity === null &&
    Boolean(
      live &&
        live.on &&
        (live.jp.trim() || live.zh.trim()) &&
        Date.now() - live.at < LIVE_CAPTION_HOLD_MS
    );
  const content = activity ? (
    <ActivityView activity={activity} />
  ) : showLive && live ? (
    <LiveCaptionView live={live} fontScale={fontScale} />
  ) : view === "clock" ? (
    <ClockView weather={weather} badges={badges} brandedBadges={mediaPlaying(media)} />
  ) : view === "pet" ? (
    <PetView
      badges={badges}
      pointerRef={pointerRef}
      surpriseToken={surpriseToken}
      pressed={pressed}
      active={active}
    />
  ) : view === "music" ? (
    <MusicView
      media={media}
      lyricLine={lyricLine}
      lyricKey={lyricKey}
      badges={badges}
      vertical={vertical}
      active={active}
      audioLevelRef={audioLevelRef}
      onTogglePlay={onTogglePlay}
    />
  ) : null;

  if (!content) return null;
  const surfaceView = showLive ? "live" : activity ? "activity" : view;
  return (
    <div
      className={`compact-surface ${vertical ? "is-vertical" : ""} ${
        activity && !showLive ? "is-activity" : `view-${surfaceView}`
      }${quota && !showLive ? " has-quota" : ""}${showLive ? " is-live" : ""}`}
      data-view={surfaceView}
      onClick={onActivate}
    >
      {content}
      {showLive ? null : quota}
    </div>
  );
}
