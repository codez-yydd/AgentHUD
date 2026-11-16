import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { translateText } from "../dashboard/islandSettings/wset/settingsApi";
import { copyText } from "./api";
import { Icon } from "./ui";
import type { LiveCaption, LiveStatus } from "./types";

/** 字节 → MB（整数；模型约 630MB，进度只需要到 1MB） */
function fmtMb(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0";
  return String(Math.round(bytes / (1024 * 1024)));
}

/** 运行阶段 → 中文（idle / starting / listening / stopping / error） */
function phaseText(status: LiveStatus | null): string {
  if (!status) return "未开始";
  switch (status.phase) {
    case "starting":
      return "正在启动…";
    case "listening":
      return "聆听中";
    case "stopping":
      return "正在停止…";
    case "error":
      return status.error || "启动失败";
    default:
      return status.running ? "聆听中" : "未开始";
  }
}

export function TranslatePanel({
  onOpenSettings,
  liveStatus,
  liveCaptions,
  liveBusy = false,
  onToggleLive,
  onClearLive,
}: {
  onOpenSettings: () => void;
  /** 实时字幕运行快照（null = 后端还没返回 / 不可用） */
  liveStatus: LiveStatus | null;
  /** 字幕历史（旧 → 新，最新在末尾；最多 80 条） */
  liveCaptions: LiveCaption[];
  /** 开始 / 停止请求进行中 */
  liveBusy?: boolean;
  onToggleLive: () => void;
  /** 清空字幕历史 */
  onClearLive: () => void;
}) {
  const sourceId = useId();
  const resultId = useId();
  const statusId = useId();
  const [source, setSource] = useState("");
  const [result, setResult] = useState("");
  const [pending, setPending] = useState(false);
  const [status, setStatus] = useState<{ text: string; error?: boolean } | null>(null);
  const inFlight = useRef(false);

  // ===== 实时字幕：跟随最新（滚到底）/ 上翻查看历史时浮出「回到最新」 =====
  const listRef = useRef<HTMLDivElement | null>(null);
  const pinnedRef = useRef(true);
  const [pinned, setPinned] = useState(true);

  const scrollToLatest = () => {
    pinnedRef.current = true;
    setPinned(true);
    const list = listRef.current;
    if (list) list.scrollTop = list.scrollHeight;
  };

  // 新字幕到达：正在跟随时自动滚到底；正在看历史则停在原处（新增行在下方，不挪视口）
  useEffect(() => {
    const list = listRef.current;
    if (!list || !pinnedRef.current) return;
    list.scrollTop = list.scrollHeight;
  }, [liveCaptions]);

  // 首次挂载直接看最新一句
  useEffect(() => {
    const list = listRef.current;
    if (list) list.scrollTop = list.scrollHeight;
  }, []);

  const handleScroll = () => {
    const list = listRef.current;
    if (!list) return;
    const atBottom = list.scrollHeight - list.scrollTop - list.clientHeight <= 8;
    if (atBottom !== pinnedRef.current) {
      pinnedRef.current = atBottom;
      setPinned(atBottom);
    }
  };

  const running = Boolean(liveStatus?.running);
  const level = Math.max(0, Math.min(1, liveStatus?.level ?? 0));
  const download = liveStatus?.download;
  const downloading = download?.state === "downloading" || download?.state === "verifying";
  const percent =
    download && download.total > 0
      ? Math.max(0, Math.min(100, Math.round((download.done / download.total) * 100)))
      : 0;
  // 状态文本：错误红字 > 下载进度 > 运行阶段
  const liveState = !liveStatus
    ? { text: "实时字幕未就绪", error: false }
    : download?.state === "error" && download.message
      ? { text: `模型下载失败：${download.message}`, error: true }
      : liveStatus.phase === "error"
        ? { text: liveStatus.error || "实时字幕启动失败", error: true }
        : downloading
          ? {
              text:
                download.state === "verifying"
                  ? `正在校验模型…${percent}%`
                  : `正在下载模型 ${percent}%（${fmtMb(download.done)}/${fmtMb(download.total)} MB）`,
              error: false,
            }
          : { text: phaseText(liveStatus), error: false };

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const text = source.trim();
    if (!text || inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    setResult("");
    setStatus(null);
    try {
      setResult(await translateText(text));
      setStatus({ text: "翻译完成" });
    } catch (error) {
      setStatus({ text: error instanceof Error ? error.message : String(error), error: true });
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  }

  async function copyResult() {
    try {
      await copyText(result);
      setStatus({ text: "已复制译文" });
    } catch (error) {
      setStatus({ text: error instanceof Error ? error.message : String(error), error: true });
    }
  }

  return (
    <form className="page translate-page" onSubmit={submit}>
      <div className="translate-fields">
        <div className="translate-field">
          <label className="translate-field-label" htmlFor={sourceId}>原文</label>
          <textarea
            id={sourceId}
            value={source}
            placeholder="输入或粘贴要翻译的文本"
            readOnly={pending}
            onChange={(event) => {
              setSource(event.currentTarget.value);
              setResult("");
              setStatus(null);
            }}
          />
        </div>
        <div className="translate-field">
          <div className="translate-field-label">
            <label htmlFor={resultId}>简体中文译文</label>
            <button
              type="button"
              className="mini-text-btn translate-copy"
              disabled={!result}
              onClick={() => void copyResult()}
            >
              <Icon name="copy" size={13} /> 复制译文
            </button>
          </div>
          <textarea
            id={resultId}
            value={result}
            readOnly
            aria-describedby={statusId}
          />
        </div>
      </div>
      <div className="translate-actions">
        <button type="button" className="mini-text-btn" onClick={onOpenSettings}>打开设置</button>
        <span id={statusId} className={`translate-status${status?.error ? " is-error" : ""}`} role="status">
          {status?.text}
        </span>
        <button type="submit" className="mini-text-btn translate-submit" disabled={!source.trim() || pending}>
          <Icon name="translate" size={15} /> {pending ? "翻译中…" : "翻译"}
        </button>
      </div>

      {/* 实时字幕：开始/停止 + 状态 + 电平条 + 字幕历史（日语/中文成对，草稿行淡化） */}
      <section className="live-panel" aria-label="实时字幕">
        <div className="live-panel-head">
          <div className="live-panel-title">
            <Icon name="mic" size={14} />
            <span>实时字幕</span>
          </div>
          <span className={`live-panel-state${liveState.error ? " is-error" : ""}`} role="status">
            {liveState.text}
          </span>
          <button
            type="button"
            className={`mini-text-btn live-toggle${running ? " is-on" : ""}`}
            onClick={onToggleLive}
            disabled={liveBusy}
            aria-pressed={running}
          >
            <Icon name="mic" size={14} /> {running ? "停止" : "开始聆听"}
          </button>
        </div>
        <div className={`live-level${running ? " is-active" : ""}`} aria-hidden="true">
          <i style={{ width: `${Math.round(level * 100)}%` }} />
        </div>
        <div className="live-history-wrap">
          <div
            className="live-history"
            ref={listRef}
            onScroll={handleScroll}
            role="log"
            aria-label="实时字幕历史"
          >
            {liveCaptions.length === 0 ? (
              <p className="live-empty">播放日语视频后点「开始聆听」</p>
            ) : (
              liveCaptions.map((line) => (
                <div
                  key={line.id}
                  className={`live-line ${line.final ? "is-final" : "is-draft"}`}
                  title={line.final ? undefined : "正在识别 / 翻译，可能还会变化"}
                >
                  <span className="live-line-jp">{line.jp}</span>
                  <span className="live-line-zh">{line.zh || "…"}</span>
                </div>
              ))
            )}
          </div>
          {!pinned ? (
            <button type="button" className="live-jump" onClick={scrollToLatest}>
              ↓ 回到最新
            </button>
          ) : null}
        </div>
        <div className="live-panel-foot">
          <span className="live-panel-count">
            {liveCaptions.length ? `共 ${liveCaptions.length} 句` : ""}
          </span>
          <button
            type="button"
            className="mini-text-btn"
            onClick={onClearLive}
            disabled={!liveCaptions.length}
          >
            <Icon name="trash" size={13} /> 清空
          </button>
        </div>
      </section>
    </form>
  );
}
