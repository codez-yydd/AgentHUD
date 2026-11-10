import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { IS_TAURI } from "../island/api";
import {
  cancelRecord,
  closeRecord,
  exportRecord,
  getRecordState,
  previewRecord,
  recordMedia,
  recordSrc,
  setGifFps,
  stopRecord,
} from "../shot/api";
import type { RecordResult } from "../shot/types";

/** 与后端 MAX_RECORD_MS 对齐：最长 5 分钟 */
const MAX_RECORD_MS = 5 * 60 * 1000;

type Phase = "recording" | "finalizing" | "preview" | "error";

function pad(value: number): string {
  return String(Math.max(0, Math.floor(value))).padStart(2, "0");
}

function clock(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  return `${pad(Math.floor(total / 60))}:${pad(total % 60)}`;
}

function clockPrecise(ms: number): string {
  const total = Math.max(0, ms / 1000);
  return `${pad(Math.floor(total / 60))}:${pad(Math.floor(total % 60))}.${Math.floor(
    (total % 1) * 10
  )}`;
}

function fileSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

export default function RecApp() {
  const recordId = useMemo(() => {
    const id = Number(new URLSearchParams(window.location.search).get("id"));
    return Number.isFinite(id) && id > 0 ? id : 0;
  }, []);
  const [phase, setPhase] = useState<Phase>("recording");
  const [elapsed, setElapsed] = useState(0);
  // 点「结束」之后后端还要收尾（GIF 编码可能几秒），计时停在点击那一刻
  const [frozenAt, setFrozenAt] = useState<number | null>(null);
  const [result, setResult] = useState<RecordResult | null>(null);
  const [error, setError] = useState("");
  const [src, setSrc] = useState("");
  const [toast, setToast] = useState("");
  const [busy, setBusy] = useState("");
  const [progress, setProgress] = useState(0);
  const [gifFps, setGifFpsState] = useState(10);
  const [rendering, setRendering] = useState(false);
  const [mediaError, setMediaError] = useState(false);

  const phaseRef = useRef<Phase>("recording");
  const elapsedRef = useRef(0);
  elapsedRef.current = elapsed;
  useEffect(() => {
    phaseRef.current = phase;
  }, [phase]);

  // 录制中轮询后端：时长从后端来（前端窗口可能被节流），结束后顺手把窗口放大成预览。
  useEffect(() => {
    if (!recordId) {
      setError("录屏参数缺失");
      setPhase("error");
      return;
    }
    let dead = false;
    let timer = 0;
    const tick = async () => {
      if (dead) return;
      try {
        const next = await getRecordState(recordId);
        if (dead) return;
        setElapsed(next.elapsedMs);
        setProgress(next.progress);
        if (next.gifFps) setGifFpsState(next.gifFps);
        if (!next.running) {
          if (next.error) {
            setError(next.error);
            setPhase("error");
            return;
          }
          if (next.result) {
            setResult(next.result);
            setElapsed(next.result.durationMs);
            setPhase("preview");
            void previewRecord(recordId).catch(() => {});
            return;
          }
          setError("录制没有产出文件");
          setPhase("error");
          return;
        }
      } catch (problem) {
        if (!dead) {
          setError(String(problem));
          setPhase("error");
        }
        return;
      }
      timer = window.setTimeout(tick, 260);
    };
    timer = window.setTimeout(tick, 160);
    return () => {
      dead = true;
      window.clearTimeout(timer);
    };
  }, [recordId]);

  // 录制失败：录制条先提示一下，然后把覆盖层放回来并带出原因。
  useEffect(() => {
    if (phase !== "error") return;
    const timer = window.setTimeout(() => {
      void cancelRecord(recordId, error).catch(() => {});
    }, 1800);
    return () => window.clearTimeout(timer);
  }, [error, phase, recordId]);

  // 预览源：优先 asset 协议（可拖动播放），不行再退回 blob。
  const loadBlob = useCallback(async () => {
    if (!result) return;
    try {
      const bytes = await recordMedia(recordId);
      const mime = result.format === "mp4" ? "video/mp4" : "image/gif";
      setSrc(URL.createObjectURL(new Blob([bytes], { type: mime })));
    } catch (problem) {
      setMediaError(true);
      setError(String(problem));
    }
  }, [recordId, result]);

  useEffect(() => {
    if (!result) return;
    let dead = false;
    // 直接走 blob：asset 协议（自定义 scheme）喂 <video> 时不支持 Range，
    // 表现是能出首帧、能播一小段就卡住（用户反馈的“只能看 3 秒”）。
    if (!IS_TAURI) {
      void loadBlob();
      return () => {
        dead = true;
      };
    }
    void (async () => {
      // 记录文件路径放进 asset 白名单（万一以后要直接开/另存），
      // 但播放用 blob，保证能完整拖动、完整播放。
      try {
        await recordSrc(recordId);
      } catch {
        /* 路径拿不到不影响 blob 播放 */
      }
      if (!dead) await loadBlob();
    })();
    return () => {
      dead = true;
      setSrc("");
    };
  }, [loadBlob, recordId, result]);

  const stop = useCallback(() => {
    setFrozenAt(elapsedRef.current);
    setPhase("finalizing");
    void stopRecord(recordId).catch((problem) => {
      setError(String(problem));
      setPhase("error");
    });
  }, [recordId]);

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(""), 2200);
    return () => window.clearTimeout(timer);
  }, [toast]);

  // Esc：录制中=结束录制，预览中=关闭预览
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (phaseRef.current === "recording") stop();
      else if (phaseRef.current === "preview") void closeRecord(recordId).catch(() => {});
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [recordId, stop]);

  if (phase === "recording" || phase === "finalizing") {
    return (
      <div className="rec-shell rec-shell-bar">
        <div className="rec-bar">
          <span className={phase === "finalizing" ? "rec-dot busy" : "rec-dot"} />
          <span className="rec-clock">{clock(frozenAt ?? elapsed)}</span>
          <span className="rec-limit">/ {clock(MAX_RECORD_MS)}</span>
          <button
            type="button"
            className="rec-stop"
            disabled={phase === "finalizing"}
            title={phase === "finalizing" ? "正在生成文件…" : "结束录制"}
            onClick={stop}
          >
            {phase === "finalizing" ? (
              <span className="rec-spinner" />
            ) : (
              <span className="rec-stop-icon" />
            )}
            {phase === "finalizing"
              ? progress > 0
                ? `生成中 ${progress}%`
                : "生成中"
              : "结束"}
          </button>
        </div>
      </div>
    );
  }

  if (phase === "error") {
    return (
      <div className="rec-shell rec-shell-bar">
        <div className="rec-bar error">
          <span className="rec-dot error" />
          <span className="rec-error-text" title={error}>
            录制失败
          </span>
          <button
            type="button"
            className="rec-stop"
            onClick={() => void cancelRecord(recordId, error).catch(() => {})}
          >
            返回
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="rec-shell rec-shell-preview">
      <Preview
        recordId={recordId}
        result={result}
        src={src}
        error={mediaError ? error : ""}
        busy={busy}
        toast={toast}
        gifFps={gifFps}
        rendering={rendering}
        onToast={setToast}
        onBusy={setBusy}
        onGifFps={async (fps) => {
          setRendering(true);
          setProgress(0);
          try {
            const next = await setGifFps(recordId, fps);
            setGifFpsState(next.gifFps);
            if (next.result) setResult(next.result);
            await loadBlob();
          } catch (problem) {
            setToast(String(problem));
          } finally {
            setRendering(false);
            setProgress(0);
          }
        }}
        onMediaError={() => {
          setMediaError(true);
          void loadBlob();
        }}
      />
    </div>
  );
}

interface PreviewProps {
  recordId: number;
  result: RecordResult | null;
  src: string;
  error: string;
  busy: string;
  toast: string;
  gifFps: number;
  rendering: boolean;
  onToast: (text: string) => void;
  onBusy: (text: string) => void;
  onGifFps: (fps: number) => void;
  onMediaError: () => void;
}

function Preview({
  recordId,
  result,
  src,
  error,
  busy,
  toast,
  gifFps,
  rendering,
  onToast,
  onBusy,
  onGifFps,
  onMediaError,
}: PreviewProps) {
  // 以视频实际时长为准（ScreenCaptureKit 录出来的精确时长只有容器知道）
  const [mediaDuration, setMediaDuration] = useState<number | null>(null);
  const duration = Math.max(1, mediaDuration ?? result?.durationMs ?? 1);
  const isVideo = result?.format === "mp4";
  useEffect(() => {
    setMediaDuration(null);
  }, [result?.path]);
  const [inMs, setInMs] = useState(0);
  const [outMs, setOutMs] = useState(duration);
  const [currentMs, setCurrentMs] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [muted, setMuted] = useState(false);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const inRef = useRef(inMs);
  const outRef = useRef(outMs);
  inRef.current = inMs;
  outRef.current = outMs;

  useEffect(() => {
    setInMs(0);
    setOutMs(duration);
  }, [duration]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    const onTime = () => {
      const at = video.currentTime * 1000;
      setCurrentMs(at);
      if (at >= outRef.current - 20) {
        video.pause();
        setPlaying(false);
        video.currentTime = inRef.current / 1000;
        setCurrentMs(inRef.current);
      }
    };
    const onEnd = () => setPlaying(false);
    video.addEventListener("timeupdate", onTime);
    video.addEventListener("pause", onEnd);
    return () => {
      video.removeEventListener("timeupdate", onTime);
      video.removeEventListener("pause", onEnd);
    };
  }, [isVideo, src]);

  const togglePlay = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (video.paused) {
      const from = Math.max(inRef.current, Math.min(currentMs, outRef.current - 100));
      video.currentTime = from / 1000;
      void video.play().catch(() => {});
      setPlaying(true);
    } else {
      video.pause();
      setPlaying(false);
    }
  }, [currentMs]);

  const seek = useCallback((ms: number) => {
    const video = videoRef.current;
    const clamped = Math.max(0, Math.min(ms, duration));
    setCurrentMs(clamped);
    if (video) video.currentTime = clamped / 1000;
  }, [duration]);

  const changeRange = useCallback(
    (nextIn: number, nextOut: number) => {
      const start = Math.max(0, Math.min(nextIn, nextOut - 150));
      const end = Math.min(duration, Math.max(nextOut, start + 150));
      setInMs(start);
      setOutMs(end);
      if (videoRef.current) videoRef.current.currentTime = start / 1000;
      setCurrentMs(start);
    },
    [duration]
  );

  const trimmed = inMs > 60 || outMs < duration - 60;
  const rangeMs = Math.max(0, outMs - inMs);

  const run = async (action: "copy" | "save") => {
    if (busy || !result) return;
    onBusy(action === "copy" ? "正在复制…" : "正在准备下载…");
    try {
      const path = await exportRecord(recordId, inMs, outMs, action);
      if (action === "save") {
        if (path) onToast("已保存");
      } else {
        onToast("已复制到剪贴板");
      }
    } catch (problem) {
      onToast(String(problem));
    } finally {
      onBusy("");
    }
  };

  return (
    <>
      <header className="rec-head">
        <span className={`rec-format ${result?.format ?? "gif"}`}>
          {(result?.format ?? "gif").toUpperCase()}
        </span>
        <span className="rec-meta">
          {result ? `${result.width} × ${result.height}` : "—"}
          <i />
          {result ? fileSize(result.size) : "—"}
          <i />
          {clock(duration)}
          {trimmed ? ` · 已裁剪 ${clockPrecise(rangeMs)}` : ""}
        </span>
        <button
          type="button"
          className="rec-close"
          title="关闭"
          onClick={() => void closeRecord(recordId).catch(() => {})}
        >
          <svg width="13" height="13" viewBox="0 0 13 13" fill="none" aria-hidden>
            <path d="M2.5 2.5l8 8m0-8l-8 8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          </svg>
        </button>
      </header>

      <div className="rec-stage">
        {error ? (
          <div className="rec-stage-error">
            <b>无法预览</b>
            <span>{error}</span>
          </div>
        ) : !src ? (
          <span className="rec-spinner big" />
        ) : isVideo ? (
          <video
            ref={videoRef}
            src={src}
            playsInline
            preload="auto"
            onClick={togglePlay}
            onError={onMediaError}
            onLoadedMetadata={(event) => {
              const seconds = event.currentTarget.duration;
              if (Number.isFinite(seconds) && seconds > 0) {
                setMediaDuration(Math.round(seconds * 1000));
              }
            }}
          />
        ) : (
          <img src={src} alt="录屏预览" onError={onMediaError} />
        )}
      </div>

      <div className="rec-controls">
        <div className="rec-control-head">
          {isVideo ? (
            <>
              <button type="button" className="rec-play" onClick={togglePlay} title="播放 / 暂停">
                {playing ? (
                  <svg width="15" height="15" viewBox="0 0 14 14" aria-hidden>
                    <rect x="2.5" y="2" width="3.4" height="10" rx="1.1" fill="currentColor" />
                    <rect x="8.1" y="2" width="3.4" height="10" rx="1.1" fill="currentColor" />
                  </svg>
                ) : (
                  <svg width="15" height="15" viewBox="0 0 14 14" aria-hidden>
                    <path d="M3.4 2.2l8 4.8-8 4.8V2.2z" fill="currentColor" />
                  </svg>
                )}
              </button>
              <button
                type="button"
                className="rec-mute"
                title={muted ? "取消静音" : "静音"}
                onClick={() => {
                  const video = videoRef.current;
                  if (!video) return;
                  video.muted = !video.muted;
                  setMuted(video.muted);
                }}
              >
                {muted ? (
                  <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden>
                    <path d="M2 5.2h2.3L7.4 2.8v8.4L4.3 8.8H2z" fill="currentColor" />
                    <path d="M9.4 5l3 4m0-4l-3 4" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
                  </svg>
                ) : (
                  <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden>
                    <path d="M2 5.2h2.3L7.4 2.8v8.4L4.3 8.8H2z" fill="currentColor" />
                    <path d="M9.3 4.6a2.8 2.8 0 010 4.8M11 2.9a5 5 0 010 8.2" stroke="currentColor" strokeWidth="1.2" fill="none" strokeLinecap="round" />
                  </svg>
                )}
              </button>
              <span className="rec-time">
                {clockPrecise(trimmed ? inMs : currentMs)}
                <i>/</i>
                {clockPrecise(trimmed ? outMs : duration)}
              </span>
            </>
          ) : (
            <>
              <span className="rec-gif-tag">GIF</span>
              <div className="rec-fps" role="group" aria-label="GIF 帧率">
                <span className="rec-fps-label">帧率</span>
                {[5, 10, 15, 20, 30].map((value) => (
                  <button
                    key={value}
                    type="button"
                    className={gifFps === value ? "active" : ""}
                    disabled={rendering}
                    title={`${value}fps：帧率越高越流畅，文件也越大`}
                    onClick={() => onGifFps(value)}
                  >
                    {value}
                  </button>
                ))}
              </div>
              {rendering ? <span className="rec-rendering">重新生成中…</span> : null}
            </>
          )}
          {trimmed ? (
            <button
              type="button"
              className="rec-reset"
              onClick={() => changeRange(0, duration)}
              title="恢复完整时长"
            >
              重置裁剪
            </button>
          ) : (
            <span className="rec-trim-hint">拖动时间轴裁剪</span>
          )}
        </div>
        <Timeline
          duration={duration}
          inMs={inMs}
          outMs={outMs}
          currentMs={currentMs}
          onSeek={seek}
          onChange={changeRange}
          showPlayhead={isVideo}
        />
      </div>

      <footer className="rec-foot">
        <span className="rec-foot-note">
          {trimmed ? `导出 ${clockPrecise(inMs)} - ${clockPrecise(outMs)}` : "整段导出"}
        </span>
        {toast ? <span className="rec-toast">{toast}</span> : null}
        <button
          type="button"
          className="rec-btn"
          disabled={!result || busy !== ""}
          onClick={() => void run("copy")}
        >
          <svg width="13" height="13" viewBox="0 0 14 14" aria-hidden>
            <rect x="4.6" y="4.4" width="7.2" height="7.4" rx="1.6" stroke="currentColor" strokeWidth="1.3" fill="none" />
            <path d="M9.2 4.2V3.3c0-.9-.7-1.6-1.6-1.6H3.2c-.9 0-1.6.7-1.6 1.6v4.4c0 .9.7 1.6 1.6 1.6h1" stroke="currentColor" strokeWidth="1.3" fill="none" strokeLinecap="round" />
          </svg>
          复制
        </button>
        <button
          type="button"
          className="rec-btn primary"
          disabled={!result || busy !== ""}
          onClick={() => void run("save")}
        >
          <svg width="13" height="13" viewBox="0 0 14 14" aria-hidden>
            <path d="M7 1.8v7.1m0 0L4.4 6.3M7 8.9l2.6-2.6" stroke="currentColor" strokeWidth="1.4" fill="none" strokeLinecap="round" strokeLinejoin="round" />
            <path d="M2.2 10.3v1.1c0 .9.7 1.6 1.6 1.6h6.4c.9 0 1.6-.7 1.6-1.6v-1.1" stroke="currentColor" strokeWidth="1.4" fill="none" strokeLinecap="round" />
          </svg>
          下载
        </button>
      </footer>
    </>
  );
}

interface TimelineProps {
  duration: number;
  inMs: number;
  outMs: number;
  currentMs: number;
  showPlayhead: boolean;
  onSeek: (ms: number) => void;
  onChange: (inMs: number, outMs: number) => void;
}

function Timeline({
  duration,
  inMs,
  outMs,
  currentMs,
  showPlayhead,
  onSeek,
  onChange,
}: TimelineProps) {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const dragRef = useRef<"in" | "out" | "seek" | null>(null);
  // window 级监听里读最新值，避免闭包过期 + 摆脱 setPointerCapture 的坑
  const live = useRef({ duration, inMs, outMs, onSeek, onChange });
  live.current = { duration, inMs, outMs, onSeek, onChange };

  useEffect(() => {
    const at = (clientX: number) => {
      const rect = trackRef.current?.getBoundingClientRect();
      if (!rect || rect.width <= 0) return 0;
      const ratio = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
      return ratio * live.current.duration;
    };
    const onMove = (event: PointerEvent) => {
      const which = dragRef.current;
      if (!which) return;
      const { inMs: from, outMs: to, onSeek: seek, onChange: change } = live.current;
      const value = at(event.clientX);
      if (which === "in") change(Math.min(value, to - 150), to);
      else if (which === "out") change(from, Math.max(value, from + 150));
      else seek(value);
    };
    const stop = () => {
      dragRef.current = null;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
    window.addEventListener("blur", stop);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
      window.removeEventListener("blur", stop);
    };
  }, []);

  const grab = (which: "in" | "out") => (event: React.PointerEvent<HTMLElement>) => {
    event.preventDefault();
    event.stopPropagation();
    dragRef.current = which;
  };

  const percent = (value: number) =>
    `${(Math.min(1, Math.max(0, value / duration)) * 100).toFixed(3)}%`;
  const trimmed = inMs > 60 || outMs < duration - 60;

  return (
    <div
      className="rec-timeline"
      ref={trackRef}
      onPointerDown={(event) => {
        dragRef.current = "seek";
        const rect = trackRef.current?.getBoundingClientRect();
        if (rect && rect.width > 0) {
          const ratio = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
          onSeek(ratio * duration);
        }
      }}
    >
      <div className="rec-track">
        <div
          className="rec-range"
          style={{ left: percent(inMs), width: `${((outMs - inMs) / duration) * 100}%` }}
        />
        {showPlayhead ? (
          <div className="rec-playhead" style={{ left: percent(currentMs) }} />
        ) : null}
      </div>
      <button
        type="button"
        className="rec-handle"
        style={{ left: percent(inMs) }}
        title={`开始 ${clockPrecise(inMs)}`}
        onPointerDown={grab("in")}
      />
      <button
        type="button"
        className="rec-handle"
        style={{ left: percent(outMs) }}
        title={`结束 ${clockPrecise(outMs)}`}
        onPointerDown={grab("out")}
      />
      <div className="rec-ticks">
        <span>{clockPrecise(trimmed ? inMs : 0)}</span>
        <span className="rec-ticks-mid">{clockPrecise((inMs + outMs) / 2)}</span>
        <span>{clockPrecise(trimmed ? outMs : duration)}</span>
      </div>
    </div>
  );
}
