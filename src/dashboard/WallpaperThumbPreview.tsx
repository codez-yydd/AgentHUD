/**
 * 壁纸缩略图预览（主窗口皮肤中心「需注入 ZCode」子页专用）：
 * 在 WallpaperThumbModeContext 为 true 时由 ThemePanel 的 WallpaperCard
 * 渲染在 16:9 预览区内容器内（本组件不含外层 16:9 容器）。
 *
 * - 进入视口附近（IntersectionObserver，rootMargin 480px）才请求生成；
 *   离开视口且尚未开始生成的取消排队，已在生成中的让它跑完写缓存；
 * - 内存缓存命中时首次渲染就直接是 <img>，不经过骨架；
 * - ready：静态缩略图 160ms 淡入；视频卡 hover 时在其上叠一个自动播放的
 *   <video> 实时预览，hovered 变假立即卸载；
 * - failed：沿用 ThemePanel 的占位标记（emoji + 「预览不可用」，class 名
 *   一致，dashboard.css 隐藏 emoji 的既有规则继续生效）；
 * - direct：本环境取不了帧，退回现有 <img>/<video> 写法，视频 hover 播放 /
 *   离开暂停并回 0 的行为同现状。
 */
import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useI18n } from "../i18n";
import type { WallpaperEntry } from "../types";
import {
  peekWallpaperThumb,
  registerThumbMount,
  requestWallpaperThumb,
  type QueuedThumbRequest,
  type WallpaperThumbResult,
} from "./wallpaperThumb";

export function WallpaperThumbPreview({
  entry,
  label,
  hovered,
}: {
  entry: WallpaperEntry;
  label: string;
  /** 整张卡片是否处于 hover（视频卡 hover 实时预览的开关） */
  hovered: boolean;
}) {
  const { t } = useI18n();
  const url = convertFileSrc(entry.previewPath);
  const isVideo = entry.kind === "video";
  const [result, setResult] = useState<WallpaperThumbResult | null>(() =>
    peekWallpaperThumb(entry.previewPath),
  );
  const [imgIn, setImgIn] = useState(false);
  const [livePlaying, setLivePlaying] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const reqRef = useRef<QueuedThumbRequest | null>(null);
  const directVideoRef = useRef<HTMLVideoElement | null>(null);
  // 当前 entry：promise 结算是异步的，用它判断结果是否还属于这张卡
  // （列表刷新会换 entry 对象，旧请求的结果不再回给组件，只进缓存）
  const entryRef = useRef(entry);
  // 已经拿到过非 null 结果（含 failed）的 entry 对象：同一次挂载、列表
  // 未刷新时不再重复请求（损坏文件不反复建离屏 video 重试）
  const settledEntryRef = useRef<WallpaperEntry | null>(null);

  // 每次渲染后同步最新 entry，保证结算回调比对的是已提交的值
  useEffect(() => {
    entryRef.current = entry;
  });

  const startRequest = () => {
    if (reqRef.current) return;
    if (settledEntryRef.current === entry) return;
    const req = requestWallpaperThumb(entry);
    reqRef.current = req;
    void req.promise.then((r) => {
      // entry 已被列表刷新换掉：结果只进缓存，不再回给这张卡
      if (entryRef.current !== entry) return;
      if (reqRef.current === req) reqRef.current = null;
      if (r) {
        settledEntryRef.current = entry;
        setResult(r);
      }
    });
  };

  // 接近视口才生成；离开视口取消排队（已在生成中的由模块侧跑完写缓存）。
  // 预取距离取 480px：正好覆盖「往下滚一两下」的距离，慢速下扫量测中
  // 露出时还没生成完的张数归零，多生成的只有排在可见卡之后的几张
  // （240px 档位会闪骨架，960px 档位无额外收益还多花生成）。
  // root 必须取卡片所在的滚动容器：root 为 null 时 IntersectionObserver
  // 先按各级滚动祖先裁剪目标矩形，卡片没滚进 PageBody（overflow-y-auto）
  // 就永远不相交，rootMargin 的预取距离不生效；以滚动容器为 root 后
  // rootMargin 才相对它计算（容器挂载后不变，不必监听）
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const scroller = el.closest(".overflow-y-auto");
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) startRequest();
        else reqRef.current?.cancel();
      },
      { root: scroller ?? null, rootMargin: "480px" },
    );
    io.observe(el);
    return () => {
      io.disconnect();
      reqRef.current?.cancel();
      // 生成中的任务不受取消影响（跑完写缓存）；清掉句柄，让列表刷新后
      // 新 entry 的请求不被旧请求挡住
      reqRef.current = null;
    };
    // entry 引用来自父组件的壁纸列表 state，稳定；变更时重建观察即可
  }, [entry]);

  // 挂载登记：参与持久层清理的「当前挂载集合」
  useEffect(() => registerThumbMount(entry.previewPath), [entry.previewPath]);

  // 键变化（防御性）：重置为当前缓存值、清「已结算 entry」标记，
  // 让新键照常请求校验
  useEffect(() => {
    settledEntryRef.current = null;
    setResult(peekWallpaperThumb(entry.previewPath));
    setImgIn(false);
    setLivePlaying(false);
  }, [entry.previewPath]);

  // ready 首次出现 160ms 淡入：先绘制一帧再加 is-in，保证过渡生效
  useEffect(() => {
    if (result?.state !== "ready") return;
    const timer = window.setTimeout(() => setImgIn(true), 30);
    return () => window.clearTimeout(timer);
  }, [result]);

  // hover 结束立即复位实时预览的淡入标记（下次 hover 挂新 video 重新等 playing）
  useEffect(() => {
    if (!hovered) setLivePlaying(false);
  }, [hovered]);

  // direct 退回 <video> 现状写法时，hover 播放 / 离开暂停并回 0 同现状
  useEffect(() => {
    const v = directVideoRef.current;
    if (!v) return;
    if (hovered) {
      v.currentTime = 0;
      // 静默失败：自动播放被拒时保留首帧静态预览即可
      v.play().catch(() => {});
    } else {
      v.pause();
      v.currentTime = 0;
    }
  }, [hovered, result]);

  // React 的 muted 属性在部分渲染路径只设 attribute 不设 property，
  // 而自动播放要求 muted property 为 true，ref 回调里显式兜底（ThemePanel 同款）
  const bindLive = (el: HTMLVideoElement | null) => {
    if (el) el.muted = true;
  };
  const bindDirectVideo = (el: HTMLVideoElement | null) => {
    directVideoRef.current = el;
    if (el) el.muted = true;
  };

  return (
    <div className="db-wpthumb" ref={rootRef}>
      {result?.state === "ready" ? (
        <>
          <img
            src={result.src}
            alt={label}
            decoding="async"
            className={`db-wpthumb__img${imgIn ? " is-in" : ""}`}
          />
          {isVideo && hovered && (
            <video
              ref={bindLive}
              className={`db-wpthumb__live${livePlaying ? " is-playing" : ""}`}
              src={url}
              muted
              loop
              playsInline
              autoPlay
              preload="auto"
              onPlaying={() => setLivePlaying(true)}
            />
          )}
        </>
      ) : result?.state === "failed" ? (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-0.5 text-slate-400">
          <span
            className="text-[14px] leading-none"
            title={t(isVideo ? "theme.libraryKindVideo" : "theme.libraryKindImage")}
          >
            {isVideo ? "🎬" : "🖼"}
          </span>
          <span className="text-[8px]">{t("theme.previewUnavailable")}</span>
        </div>
      ) : result?.state === "direct" ? (
        isVideo ? (
          <video
            ref={bindDirectVideo}
            src={`${url}#t=0.1`}
            muted
            loop
            playsInline
            preload="metadata"
            onError={() => setResult({ state: "failed" })}
            className="absolute inset-0 h-full w-full object-cover"
          />
        ) : (
          <img
            src={url}
            alt={label}
            loading="lazy"
            onError={() => setResult({ state: "failed" })}
            className="absolute inset-0 h-full w-full object-cover"
          />
        )
      ) : (
        <span className="db-wpthumb__skeleton">
          {isVideo ? (
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden
            >
              {/* 视频线条图标：胶片框 + 播放三角 */}
              <rect x="3" y="5" width="18" height="14" rx="2" />
              <path d="m10 9 5 3-5 3z" />
            </svg>
          ) : (
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden
            >
              {/* 图片线条图标：相框 + 太阳 + 山形 */}
              <rect x="3" y="5" width="18" height="14" rx="2" />
              <circle cx="9" cy="10" r="1.5" />
              <path d="m21 15-4.5-4.5L9 18" />
            </svg>
          )}
        </span>
      )}
    </div>
  );
}
