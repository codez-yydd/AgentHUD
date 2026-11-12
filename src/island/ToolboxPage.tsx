/**
 * 灵动岛「工具」页：二维码生成 + 图片压缩 / 格式转换。
 *
 * 两块能力都在 Rust 侧（src-tauri/src/toolbox.rs），页面只负责取参与展示：
 * - 二维码：输入内容后 260ms 防抖请后端出 PNG（data URL 直接 <img>），
 *   容错 / 每模块像素 / 静区任一改动都会重算；空内容不发请求。
 * - 图片：选文件 → 后端读尺寸 / 源体积 / 720px 预览 → 选目标格式、JPEG
 *   质量、最长边 → 转换并保存，内联给出「旧体积 → 新体积（涨跌%）」。
 *
 * 本页不联网、不读用量库；错误一律内联显示（后端给的是中文串），只有
 * 「已保存」这类成功反馈走岛上的 toast。浏览器预览（!IS_TAURI）下文件对话框
 * 换成固定假路径，其余走 api.ts 的 mock 分支。
 */
import { useCallback, useEffect, useState } from "react";
import {
  open as openNativeDialog,
  save as saveNativeDialog,
} from "@tauri-apps/plugin-dialog";
import {
  IS_TAURI,
  meetingState,
  toolboxImageInspect,
  toolboxImageSave,
  toolboxQrPng,
  toolboxQrSave,
} from "./api";
import type { ToolboxImageInfo, ToolboxImageOut, ToolboxQrPng, TabKey } from "./types";
import { Icon, SectionHead } from "./ui";
import { SnippetsPanel } from "./SnippetsPanel";
import { LauncherPanel } from "./LauncherPanel";
import { MeetingPanel } from "./MeetingPanel";

/** 二维码容错级别 */
type QrEcc = "L" | "M" | "Q" | "H";
const QR_ECC: readonly QrEcc[] = ["L", "M", "Q", "H"];
/** 每模块像素 / 静区（模块数）的常用档位 */
const QR_SCALES = [4, 6, 8, 12] as const;
const QR_QUIETS = [0, 2, 4, 8] as const;
/** 输出格式与最长边档位（0 = 不缩放） */
type OutFormat = "png" | "jpeg" | "webp";
const OUT_FORMATS: readonly { value: OutFormat; label: string }[] = [
  { value: "png", label: "PNG" },
  { value: "jpeg", label: "JPEG" },
  { value: "webp", label: "WebP" },
];
const MAX_EDGES = [0, 800, 1280, 1920] as const;
const IMAGE_FILTERS = [
  { name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp"] },
];

/** 字节数 → 人读（B / KB / MB） */
function fmtBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10_240 ? 1 : 0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

/** 拆分路径为「目录 + 去扩展名文件名」，保留原分隔符 */
function splitPath(path: string): { dir: string; stem: string } {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const dir = cut >= 0 ? path.slice(0, cut + 1) : "";
  const file = cut >= 0 ? path.slice(cut + 1) : path;
  const dot = file.lastIndexOf(".");
  return { dir, stem: dot > 0 ? file.slice(0, dot) : file };
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 文件对话框：浏览器预览没有系统对话框，给一个固定假路径（只影响截图） */
async function pickImagePath(): Promise<string | null> {
  if (!IS_TAURI) return "C:\\Users\\demo\\Pictures\\dashboard-shot.png";
  const picked = await openNativeDialog({
    multiple: false,
    directory: false,
    filters: IMAGE_FILTERS,
  });
  return typeof picked === "string" ? picked : null;
}

async function pickSavePath(
  defaultPath: string,
  extensions: string[]
): Promise<string | null> {
  if (!IS_TAURI) return defaultPath;
  const path = await saveNativeDialog({
    defaultPath,
    filters: [{ name: extensions[0].toUpperCase(), extensions }],
  });
  return typeof path === "string" ? path : null;
}

/** 小号分段选择（岛内统一形态：胶囊槽 + 浮起选中块） */
function MiniSeg<T extends string | number>({
  value,
  options,
  onChange,
  format,
  ariaLabel,
}: {
  value: T;
  options: readonly T[];
  onChange: (next: T) => void;
  format?: (value: T) => string;
  ariaLabel: string;
}) {
  return (
    <div className="tool-seg" role="radiogroup" aria-label={ariaLabel}>
      {options.map((option) => (
        <button
          key={String(option)}
          type="button"
          role="radio"
          aria-checked={option === value}
          className={option === value ? "active" : ""}
          onClick={() => onChange(option)}
        >
          {format ? format(option) : String(option)}
        </button>
      ))}
    </div>
  );
}

export function ToolboxPage({
  onToast,
  onOpenTab,
  onOpenMainWindow,
  onOpenPath,
}: {
  onToast: (message: string) => void;
  /** 启动器切到岛内别的页面 */
  onOpenTab: (tab: TabKey) => void;
  /** 启动器打开主窗口页面 */
  onOpenMainWindow: (page?: string) => void;
  /** 启动器打开项目目录 */
  onOpenPath: (path: string) => void;
}) {
  const [mode, setMode] = useState<"qr" | "img" | "snip" | "launch" | "meeting">("qr");
  // ---- 会议录音指示（UI-5）----
  // 轮询后端会话状态（轻量：读一次进程内会话槽），录音进行时在「会议」标签
  // 上显示红点、提示行常驻提醒；挂载后的第一次查询若发现活跃会话，直接切
  // 回「会议」子页（一级标签切走再回来时，工具页重置到二维码，录音会变得
  // 不可感知）。之后的轮询不再自动切换：录音开始后用户去别的子页浏览，
  // 不该被强行拽回。
  const [meetingActive, setMeetingActive] = useState(false);

  useEffect(() => {
    let cancelled = false;
    // first=true 仅挂载后的第一次查询：命中活跃会话才自动切到「会议」子页。
    // 之后的轮询只更新红点/提示，不再拉拽用户（录音开始后用户去别的子页
    // 浏览，不该被强行拽回）。
    const check = (first: boolean) => {
      meetingState()
        .then((brief) => {
          if (cancelled) return;
          const active = Boolean(brief);
          setMeetingActive(active);
          if (first && active) setMode("meeting");
        })
        .catch(() => {
          /* 状态查询失败只影响指示，不打扰页面 */
        });
    };
    check(true);
    const timer = window.setInterval(() => check(false), 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  // ---- 二维码 ----
  const [qrText, setQrText] = useState("");
  const [qrEcc, setQrEcc] = useState<QrEcc>("M");
  const [qrScale, setQrScale] = useState<number>(8);
  const [qrQuiet, setQrQuiet] = useState<number>(4);
  const [qr, setQr] = useState<ToolboxQrPng | null>(null);
  const [qrBusy, setQrBusy] = useState(false);
  const [qrError, setQrError] = useState("");

  // 输入防抖 + 参数联动：被新输入取代的请求直接丢弃
  useEffect(() => {
    if (!qrText.trim()) {
      setQr(null);
      setQrError("");
      setQrBusy(false);
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      setQrBusy(true);
      toolboxQrPng(qrText, qrEcc, qrScale, qrQuiet)
        .then((next) => {
          if (cancelled) return;
          setQr(next);
          setQrError("");
        })
        .catch((error: unknown) => {
          if (cancelled) return;
          setQr(null);
          setQrError(errorText(error));
        })
        .finally(() => {
          if (!cancelled) setQrBusy(false);
        });
    }, 260);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [qrText, qrEcc, qrScale, qrQuiet]);

  const saveQr = useCallback(async () => {
    if (!qr) return;
    const path = await pickSavePath("qrcode.png", ["png"]);
    if (!path) return;
    try {
      const out = await toolboxQrSave(qrText, qrEcc, qrScale, qrQuiet, path);
      onToast(`已保存 ${fmtBytes(out.bytes)} 的二维码`);
    } catch (error) {
      setQrError(errorText(error));
    }
  }, [qr, qrText, qrEcc, qrScale, qrQuiet, onToast]);

  // ---- 图片 ----
  const [img, setImg] = useState<ToolboxImageInfo | null>(null);
  const [format, setFormat] = useState<OutFormat>("jpeg");
  const [quality, setQuality] = useState(82);
  const [maxEdge, setMaxEdge] = useState<number>(0);
  const [imgBusy, setImgBusy] = useState(false);
  const [imgError, setImgError] = useState("");
  const [imgOut, setImgOut] = useState<ToolboxImageOut | null>(null);

  const pickImage = useCallback(async () => {
    const picked = await pickImagePath();
    if (!picked) return;
    setImgBusy(true);
    setImgError("");
    setImgOut(null);
    try {
      const info = await toolboxImageInspect(picked);
      setImg(info);
      // 目标格式默认跟随源图：PNG 截图默认转 JPEG 会糊字
      setFormat(
        info.format === "jpeg" ? "jpeg" : info.format === "webp" ? "webp" : "png"
      );
    } catch (error) {
      setImg(null);
      setImgError(errorText(error));
    } finally {
      setImgBusy(false);
    }
  }, []);

  const convert = useCallback(async () => {
    if (!img) return;
    const ext = format === "jpeg" ? "jpg" : format;
    const { dir, stem } = splitPath(img.path);
    const path = await pickSavePath(`${dir}${stem}-compressed.${ext}`, [ext]);
    if (!path) return;
    setImgBusy(true);
    setImgError("");
    setImgOut(null);
    try {
      const out = await toolboxImageSave(
        img.path,
        format,
        format === "jpeg" ? quality : null,
        maxEdge,
        path
      );
      setImgOut(out);
      onToast(`已保存 ${fmtBytes(out.bytes)}`);
    } catch (error) {
      setImgError(errorText(error));
    } finally {
      setImgBusy(false);
    }
  }, [img, format, quality, maxEdge, onToast]);

  const deltaPct =
    img && imgOut && img.bytes > 0
      ? Math.round(((imgOut.bytes - img.bytes) / img.bytes) * 100)
      : null;

  return (
    <div className="page tool-page">
      <SectionHead
        title="工具箱"
        hint={
          meetingActive && mode !== "meeting"
            ? "● 会议录音进行中 · 「会议」子页可查看"
            : mode === "qr"
              ? "二维码生成 · 全部本机处理"
              : mode === "img"
                ? "图片压缩与格式转换 · 全部本机处理"
                : mode === "snip"
                  ? "常用 prompt / 模板 · 点一条即复制"
                  : mode === "meeting"
                    ? "会议录音 · 保存到本机 ~/.agenthud/meetings"
                    : "搜页面、动作或项目 · ↑↓ 选择、回车执行"
        }
      />

      <div className="tool-switch" role="tablist" aria-label="工具分类">
        <button
          type="button"
          role="tab"
          aria-selected={mode === "qr"}
          className={mode === "qr" ? "active" : ""}
          onClick={() => setMode("qr")}
        >
          <Icon name="spark" size={11} />
          二维码
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={mode === "img"}
          className={mode === "img" ? "active" : ""}
          onClick={() => setMode("img")}
        >
          <Icon name="image" size={11} />
          图片
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={mode === "snip"}
          className={mode === "snip" ? "active" : ""}
          onClick={() => setMode("snip")}
        >
          <Icon name="note" size={11} />
          片段
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={mode === "launch"}
          className={mode === "launch" ? "active" : ""}
          onClick={() => setMode("launch")}
        >
          <Icon name="search" size={11} />
          启动
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={mode === "meeting"}
          className={mode === "meeting" ? "active" : ""}
          onClick={() => setMode("meeting")}
        >
          <Icon name="mic" size={11} />
          会议
          {meetingActive ? <span className="meet-tab-live" title="录音进行中" /> : null}
        </button>
      </div>

      {mode === "meeting" ? (
        <MeetingPanel onToast={onToast} />
      ) : mode === "snip" ? (
        <SnippetsPanel onToast={onToast} />
      ) : mode === "launch" ? (
        <LauncherPanel
          onOpenTab={onOpenTab}
          onOpenMainWindow={onOpenMainWindow}
          onOpenPath={onOpenPath}
          onToast={onToast}
        />
      ) : mode === "qr" ? (
        <div className="tool-qr">
          <div className="tool-col">
            <textarea
              className="tool-textarea"
              rows={3}
              value={qrText}
              placeholder="粘贴网址或任意文本，换行会保留"
              spellCheck={false}
              onChange={(event) => setQrText(event.target.value)}
            />
            <div className="tool-row">
              <span className="tool-label">容错</span>
              <MiniSeg
                value={qrEcc}
                options={QR_ECC}
                onChange={setQrEcc}
                ariaLabel="容错级别"
              />
            </div>
            <div className="tool-row">
              <span className="tool-label">像素</span>
              <MiniSeg
                value={qrScale}
                options={QR_SCALES}
                onChange={setQrScale}
                ariaLabel="每模块像素"
                format={(value) => `${value}`}
              />
              <span className="tool-label">静区</span>
              <MiniSeg
                value={qrQuiet}
                options={QR_QUIETS}
                onChange={setQrQuiet}
                ariaLabel="静区模块数"
              />
            </div>
            <p className="tool-hint">
              级别越高越抗污损，能装的内容越少；短链建议 M。
            </p>
          </div>

          <div className="tool-preview">
            {qr ? (
              <img className="tool-qr-img" src={qr.data_url} alt="二维码预览" />
            ) : (
              <div className="tool-empty">
                {qrBusy ? "生成中…" : "输入内容后自动生成"}
              </div>
            )}
            {qr ? (
              <div className="tool-meta">
                {qr.modules}×{qr.modules} · {qr.size}px · {fmtBytes(qr.bytes)}
              </div>
            ) : null}
            {qrError ? <div className="tool-error">{qrError}</div> : null}
            <button
              type="button"
              className="tool-btn primary"
              disabled={!qr}
              onClick={() => void saveQr()}
            >
              保存 PNG
            </button>
          </div>
        </div>
      ) : (
        <div className="tool-img">
          <div className="tool-preview">
            {img ? (
              <img
                className="tool-img-preview"
                src={img.preview}
                alt="图片预览"
              />
            ) : (
              <div className="tool-empty">
                {imgBusy ? "读取中…" : "还没有选择图片"}
              </div>
            )}
            <button
              type="button"
              className="tool-btn"
              disabled={imgBusy}
              onClick={() => void pickImage()}
            >
              {img ? "换一张" : "选择图片"}
            </button>
            <div className="tool-meta">
              {img
                ? `${img.width}×${img.height} · ${fmtBytes(img.bytes)} · ${img.format.toUpperCase()}`
                : "PNG / JPEG / WebP / GIF / BMP"}
            </div>
          </div>

          <div className="tool-col">
            <div className="tool-row">
              <span className="tool-label">格式</span>
              <MiniSeg
                value={format}
                options={OUT_FORMATS.map((item) => item.value)}
                onChange={setFormat}
                ariaLabel="输出格式"
                format={(value) =>
                  OUT_FORMATS.find((item) => item.value === value)?.label ?? value
                }
              />
            </div>
            <div className="tool-row">
              <span className="tool-label">质量</span>
              <input
                type="range"
                className="tool-range"
                min={40}
                max={100}
                step={1}
                value={quality}
                disabled={format !== "jpeg"}
                aria-label="JPEG 质量"
                onChange={(event) => setQuality(Number(event.target.value))}
              />
              <span className="tool-val">
                {format === "jpeg" ? quality : "—"}
              </span>
            </div>
            <div className="tool-row">
              <span className="tool-label">最长边</span>
              <MiniSeg
                value={maxEdge}
                options={MAX_EDGES}
                onChange={setMaxEdge}
                ariaLabel="最长边"
                format={(value) => (value === 0 ? "原图" : `${value}`)}
              />
            </div>
            <button
              type="button"
              className="tool-btn primary"
              disabled={!img || imgBusy}
              onClick={() => void convert()}
            >
              {imgBusy ? "处理中…" : "转换并保存"}
            </button>
            <p className="tool-hint">
              PNG / WebP 为无损编码；要显著变小请选 JPEG 或限制最长边。
            </p>
            {imgOut && img ? (
              <div className="tool-meta">
                {fmtBytes(img.bytes)} → {fmtBytes(imgOut.bytes)}
                {deltaPct == null ? "" : `（${deltaPct > 0 ? "+" : ""}${deltaPct}%）`}
                {" · "}
                {imgOut.width}×{imgOut.height}
              </div>
            ) : null}
            {imgError ? <div className="tool-error">{imgError}</div> : null}
          </div>
        </div>
      )}
    </div>
  );
}
