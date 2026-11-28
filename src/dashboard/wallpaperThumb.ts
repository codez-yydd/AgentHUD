/**
 * 壁纸库真实缩略图（主窗口皮肤中心「需注入 ZCode」子页专用）。
 *
 * 结构与流程：
 * - 上下文开关：WallpaperThumbModeContext（默认 false）。ThemePanel 是
 *   共用组件，目前只有主窗口皮肤页挂载（SkinPage 包 Provider 置 true）；
 *   未包 Provider 的场景拿默认值，预览保持原有 <img>/<video> 形态；
 * - 缓存两层：内存 Map（窗口会话内一直有效，切走再切回不重新生成；条目
 *   记住生成时的文件大小，卡片每次挂载 / 列表刷新时入队做一次 HEAD 校验，
 *   文件在同一路径被替换或损坏时跟着刷新）+ IndexedDB 持久层（库
 *   agenthud-wallpaper-thumbs，记录 {src,size,at}，以 HEAD 的 content-length
 *   校验文件未变才采用）；IndexedDB 出现任何异常（含超时）都静默关闭
 *   持久层，本会话只用内存层；
 * - 取帧：视频走离屏 <video preload="auto"> 等首帧回调（必须先于设 src
 *   注册）画 384 宽 canvas；图片走 new Image() 画布缩放。jpg/jpeg 编码
 *   JPEG，png/webp 编码 PNG（保透明；WKWebView 的 canvas 编码不出 WebP）。
 *   canvas 不可读像素时判 direct（环境取不了帧），本会话内后续条目直接
 *   退回现有预览写法，不再尝试取帧；
 * - 调度：并发 3 的先进先出队列；同一键在途请求合并；离开视口可取消
 *   排队（已在生成中的让它跑完并写缓存）。
 */
import { createContext, useContext } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { WallpaperEntry } from "../types";

/* ---------- 上下文开关 ---------- */

/** 缩略图模式开关：默认 false（未包 Provider 的场景走原预览；ThemePanel
 *  目前只有主窗口皮肤页挂载），仅主窗口皮肤页（SkinPage 的 injected 分支）
 *  置 true */
export const WallpaperThumbModeContext = createContext(false);

/** 读取缩略图模式开关 */
export function useWallpaperThumbMode(): boolean {
  return useContext(WallpaperThumbModeContext);
}

/* ---------- 结果类型与常量 ---------- */

/** 缩略图请求结果三态：
 *  - ready：已取得静态缩略图（data URL + 原始宽高；从持久层恢复时宽高
 *    未知记 0，预览侧 object-cover 铺满、不依赖宽高）；
 *  - failed：文件缺失 / 损坏 / 加载失败，走「预览不可用」占位；
 *  - direct：本环境读不了像素（canvas 被跨源污染等），退回现有写法 */
export type WallpaperThumbResult =
  | { state: "ready"; src: string; w: number; h: number }
  | { state: "failed" }
  | { state: "direct" };

/** 在途请求句柄：排队中被取消时 promise 以 null 结算（调用方忽略即可） */
export interface QueuedThumbRequest {
  promise: Promise<WallpaperThumbResult | null>;
  /** 取消。返回 true 表示任务尚在排队、已被移出并立即以 null 结算；
   *  返回 false 表示任务已在生成中或已结束（不受影响，会跑完写缓存并
   *  正常结算） */
  cancel: () => boolean;
}

/** 缩略图画布基准宽度 */
const THUMB_WIDTH = 384;
/** 缩略图长边上限：极端长宽比（如误放进壁纸目录的长截图）时按比例压到
 *  该值，避免画出超高画布、编码出巨大的 data URL；常规 16:9 / 9:16 壁纸
 *  的长边不超过它，不受影响 */
const THUMB_MAX_EDGE = 1024;
/** JPEG 编码质量 */
const JPEG_QUALITY = 0.85;
/** HEAD 请求超时 */
const HEAD_TIMEOUT_MS = 1500;
/** IndexedDB 操作超时 */
const IDB_TIMEOUT_MS = 1500;
/** 单条视频取帧总超时（从设 src 起算） */
const VIDEO_LOAD_TIMEOUT_MS = 10000;
/** 单张图片加载超时 */
const IMAGE_LOAD_TIMEOUT_MS = 10000;
/** loadeddata 后仍无首帧回调的顶帧等待 */
const FRAME_STALL_MS = 2000;
/** 黑场重取时 seeked 之后的兜底取帧延迟 */
const SEEK_FALLBACK_MS = 120;
/** 挂载静默期：1500ms 内没有新挂载就按当前挂载集合清理持久层 */
const PRUNE_DELAY_MS = 1500;
/** 取帧并发上限 */
const THUMB_CONCURRENCY = 3;

/* ---------- 内存层与 direct 会话标记 ---------- */

/** 内存层条目：结果 + 生成时的文件大小（HEAD content-length；未知记 -1），
 *  供卡片每次挂载 / 列表刷新时的会话内校验比对 */
interface MemThumbEntry {
  result: WallpaperThumbResult;
  size: number;
}

const memCache = new Map<string, MemThumbEntry>();

/** direct 全局标记：一旦确认本环境读不了像素，后续所有条目直接走 direct */
let directSession = false;

const DIRECT_RESULT: WallpaperThumbResult = { state: "direct" };

function markDirect(): void {
  directSession = true;
}

/** 同步读内存缓存（供预览组件首帧渲染直接出 <img>，不经过骨架） */
export function peekWallpaperThumb(key: string): WallpaperThumbResult | null {
  return memCache.get(key)?.result ?? null;
}

/* ---------- IndexedDB 持久层 ---------- */

interface ThumbRecord {
  src: string;
  size: number;
  at: number;
}

const IDB_NAME = "agenthud-wallpaper-thumbs";
const IDB_VERSION = 1;
const IDB_STORE = "thumbs";

/** 持久层故障标记：任何异常（打开 / 读写 / 超时）置位后本会话不再触碰 */
let idbBroken = false;

function openDb(): Promise<IDBDatabase | null> {
  return new Promise((resolve) => {
    let req: IDBOpenDBRequest;
    try {
      req = indexedDB.open(IDB_NAME, IDB_VERSION);
    } catch {
      resolve(null);
      return;
    }
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(IDB_STORE)) {
        db.createObjectStore(IDB_STORE);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => resolve(null);
    req.onblocked = () => resolve(null);
  });
}

/** 带超时的 Promise 竞速：超时 / 失败都以 null 结算 */
function withTimeoutP<T>(promise: Promise<T>, ms: number): Promise<T | null> {
  return new Promise((resolve) => {
    const timer = window.setTimeout(() => resolve(null), ms);
    promise.then(
      (value) => {
        window.clearTimeout(timer);
        resolve(value);
      },
      () => {
        window.clearTimeout(timer);
        resolve(null);
      },
    );
  });
}

/** 统一的事务执行：任何异常（请求错误 / 事务中止 / 超时）关闭持久层。
 *  超时包住整个事务——事务挂着不结算时不能让调用方无限等待（并发槽
 *  会被一直占用，攒满后整条队列停摆） */
async function idbRun<T>(
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T | null> {
  if (idbBroken) return null;
  try {
    const db = await withTimeoutP(openDb(), IDB_TIMEOUT_MS);
    if (!db) {
      idbBroken = true;
      return null;
    }
    const result = await new Promise<T | null>((resolve) => {
      let settled = false;
      const finish = (value: T | null, broken: boolean) => {
        if (settled) return;
        settled = true;
        window.clearTimeout(timer);
        if (broken) idbBroken = true;
        resolve(value);
      };
      const timer = window.setTimeout(() => finish(null, true), IDB_TIMEOUT_MS);
      const tx = db.transaction(IDB_STORE, mode);
      const req = run(tx.objectStore(IDB_STORE));
      req.onsuccess = () => finish(req.result ?? null, false);
      req.onerror = () => finish(null, true);
      tx.onabort = () => finish(null, true);
      tx.oncomplete = () => db.close();
    });
    return result;
  } catch {
    idbBroken = true;
    return null;
  }
}

function idbGet(key: string): Promise<ThumbRecord | null> {
  return idbRun("readonly", (store) => store.get(key));
}

/** 写入失败静默忽略（缩略图丢了下次重新生成） */
function idbPut(key: string, record: ThumbRecord): void {
  void idbRun("readwrite", (store) => store.put(record, key));
}

function idbDelete(key: string): void {
  void idbRun("readwrite", (store) => store.delete(key));
}

/** 以 keep 集合为准清理持久层（每次进入皮肤页最多清一次，见 registerThumbMount） */
async function idbPrune(keep: Set<string>): Promise<void> {
  if (idbBroken) return;
  try {
    const db = await withTimeoutP(openDb(), IDB_TIMEOUT_MS);
    if (!db) {
      idbBroken = true;
      return;
    }
    await new Promise<void>((resolve) => {
      // 超时包住整个事务（同 idbRun）：事务挂着不结算时不能无限等待
      let settled = false;
      const finish = (broken: boolean) => {
        if (settled) return;
        settled = true;
        window.clearTimeout(timer);
        if (broken) idbBroken = true;
        resolve();
      };
      const timer = window.setTimeout(() => finish(true), IDB_TIMEOUT_MS);
      const tx = db.transaction(IDB_STORE, "readwrite");
      const store = tx.objectStore(IDB_STORE);
      const keysReq = store.getAllKeys();
      keysReq.onsuccess = () => {
        for (const k of keysReq.result) {
          if (!keep.has(String(k))) store.delete(k);
        }
        finish(false);
      };
      keysReq.onerror = () => finish(true);
      tx.onabort = () => finish(true);
      tx.oncomplete = () => db.close();
    });
  } catch {
    idbBroken = true;
  }
}

/* ---------- HEAD 校验 ---------- */

/** HEAD 结果：number = 2xx 且拿到 content-length；"missing" = 非 2xx
 *  （文件不在 / 未放行）；"unknown" = 2xx 但没有或非法的 content-length
 *  （文件存在、大小未知）；null = 超时或 fetch 抛异常（存在与否未知） */
type HeadSizeResult = number | "missing" | "unknown" | null;

/** HEAD 取 content-length：1500ms 超时 */
async function headSize(url: string): Promise<HeadSizeResult> {
  try {
    const ctrl = new AbortController();
    const timer = window.setTimeout(() => ctrl.abort(), HEAD_TIMEOUT_MS);
    try {
      const res = await fetch(url, {
        method: "HEAD",
        cache: "no-store",
        signal: ctrl.signal,
      });
      if (!res.ok) return "missing";
      const raw = res.headers.get("content-length");
      if (raw === null) return "unknown";
      const size = Number(raw);
      return Number.isFinite(size) && size >= 0 ? size : "unknown";
    } finally {
      window.clearTimeout(timer);
    }
  } catch {
    return null;
  }
}

/* ---------- 画布编码 ---------- */

/** 编码结果：字符串 = data URL；"bad-source" = 原始宽高非法（这一张取
 *  不到尺寸，判 failed，不连累别的卡）；null = 画布不可读像素 / 编码
 *  异常（环境问题，判 direct） */
type EncodeResult = string | "bad-source" | null;

/** 画 384 宽缩略图并编码（长边超过 THUMB_MAX_EDGE 时等比压到上限） */
function encodeThumbnail(
  source: CanvasImageSource,
  srcW: number,
  srcH: number,
  mime: "image/jpeg" | "image/png",
  quality?: number,
): EncodeResult {
  if (srcW <= 0 || srcH <= 0) return "bad-source";
  try {
    let w = THUMB_WIDTH;
    let h = Math.max(1, Math.round((THUMB_WIDTH * srcH) / srcW));
    const longEdge = Math.max(w, h);
    if (longEdge > THUMB_MAX_EDGE) {
      const scale = THUMB_MAX_EDGE / longEdge;
      w = Math.max(1, Math.round(w * scale));
      h = Math.max(1, Math.round(h * scale));
    }
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.drawImage(source, 0, 0, canvas.width, canvas.height);
    return canvas.toDataURL(mime, quality);
  } catch {
    return null;
  }
}

/** 32×18 采样亮度（黑场淡入检测）；读像素失败按非黑场处理，不阻塞主流程 */
function sampleLuma(v: HTMLVideoElement): { mean: number; max: number } | null {
  try {
    if (!v.videoWidth || !v.videoHeight) return null;
    const canvas = document.createElement("canvas");
    canvas.width = 32;
    canvas.height = 18;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return null;
    ctx.drawImage(v, 0, 0, 32, 18);
    const data = ctx.getImageData(0, 0, 32, 18).data;
    let sum = 0;
    let max = 0;
    for (let i = 0; i < data.length; i += 4) {
      const y = 0.299 * data[i] + 0.587 * data[i + 1] + 0.114 * data[i + 2];
      sum += y;
      if (y > max) max = y;
    }
    return { mean: sum / (data.length / 4), max };
  } catch {
    return null;
  }
}

/* ---------- 取帧（视频） ---------- */

interface LoadedVideoFrame {
  el: HTMLVideoElement;
  width: number;
  height: number;
}

/** 卸掉离屏视频：暂停、清源、触发 load 释放解码器、移出 DOM */
function destroyVideoEl(v: HTMLVideoElement): void {
  try {
    v.pause();
  } catch {
    // 已暂停等场景的异常忽略
  }
  v.removeAttribute("src");
  try {
    v.load();
  } catch {
    // 空源 load 的异常忽略
  }
  v.remove();
}

/**
 * 加载一段视频并等到可取的首帧。
 * - 首帧回调（requestVideoFrameCallback，不支持时退 loadeddata + 一帧 rAF）
 *   必须在设 src 之前注册，否则会错过那一帧；
 * - 页面不可见时少数视频的首帧回调迟迟不来：loadeddata 后 2000ms 仍无帧
 *   就静音 play()、playing 后立即 pause()「顶一下」；
 * - 首帧近黑（黑场淡入）：先注册重取回调（rVFC + seeked 后 120ms 兜底），
 *   再把 currentTime 定位到 min(1, duration/2) 重取一次（引擎不支持 rVFC
 *   时直接用当前首帧）；
 * - 从设 src 起算 10s 仍未拿到帧判失败。
 * 成功返回未卸载的视频元素（调用方编码后负责 destroyVideoEl）；
 * 失败返回 null（内部已卸载）。
 */
function loadVideoFrame(
  url: string,
  anonymous: boolean,
): Promise<LoadedVideoFrame | null> {
  return new Promise((resolve) => {
    const v = document.createElement("video");
    v.muted = true;
    v.playsInline = true;
    v.preload = "auto";
    if (anonymous) v.crossOrigin = "anonymous";
    // 离屏挂载（不显示、不拦截交互），保证帧回调与 play() 顶帧可靠触发
    v.setAttribute("aria-hidden", "true");
    v.style.cssText =
      "position:fixed;left:0;top:0;width:1px;height:1px;opacity:0;pointer-events:none;";
    document.body.appendChild(v);

    let settled = false;
    let gotFirst = false;
    let deadline = 0;
    let stall = 0;
    let fallback = 0;
    const listeners: Array<() => void> = [];
    const on = (target: EventTarget, type: string, fn: () => void) => {
      target.addEventListener(type, fn);
      listeners.push(() => target.removeEventListener(type, fn));
    };
    const teardown = () => {
      if (deadline) window.clearTimeout(deadline);
      if (stall) window.clearTimeout(stall);
      if (fallback) window.clearTimeout(fallback);
      for (const off of listeners) off();
      listeners.length = 0;
    };
    const fail = () => {
      if (settled) return;
      settled = true;
      teardown();
      destroyVideoEl(v);
      resolve(null);
    };
    const succeed = () => {
      if (settled) return;
      settled = true;
      teardown();
      resolve({ el: v, width: v.videoWidth, height: v.videoHeight });
    };

    // 首帧处理：近黑（黑场淡入）则定位中段重取一次，否则直接成功
    const onFirstFrame = () => {
      if (settled) return;
      gotFirst = true;
      const duration = Number.isFinite(v.duration) ? v.duration : 0;
      const luma = sampleLuma(v);
      if (!luma || luma.mean >= 12 || luma.max >= 40 || duration <= 0.25) {
        succeed();
        return;
      }
      let redone = false;
      const redo = () => {
        if (redone || settled) return;
        redone = true;
        succeed();
      };
      // 先注册重取回调（rVFC；seeked 后 120ms 兜底），再定位，避免错过帧；
      // 引擎不支持 rVFC 时直接成功（当前帧就是首帧，回调会在 rAF 里抛
      // TypeError，干等 10 秒超时）
      if (typeof v.requestVideoFrameCallback === "function") {
        v.requestVideoFrameCallback(() => redo());
        on(v, "seeked", () => {
          fallback = window.setTimeout(redo, SEEK_FALLBACK_MS);
        });
        v.currentTime = Math.min(1, duration / 2);
      } else {
        succeed();
      }
    };

    // 首帧回调先注册（必须在设 src 之前）；不支持 rVFC 时退 loadeddata + rAF
    if (typeof v.requestVideoFrameCallback === "function") {
      v.requestVideoFrameCallback(() => onFirstFrame());
    }
    on(v, "loadeddata", () => {
      if (typeof v.requestVideoFrameCallback !== "function") {
        requestAnimationFrame(() => onFirstFrame());
      }
      stall = window.setTimeout(() => {
        if (settled || gotFirst) return;
        on(v, "playing", () => v.pause());
        // 静默失败：顶帧被拒就交给总超时兜底
        v.play().catch(() => {});
      }, FRAME_STALL_MS);
    });
    on(v, "error", () => fail());
    deadline = window.setTimeout(() => fail(), VIDEO_LOAD_TIMEOUT_MS);

    v.src = url; // 不带 #t=：直接取原生首帧
  });
}

async function captureVideo(
  url: string,
  headLen: HeadSizeResult,
): Promise<WallpaperThumbResult> {
  let frame = await loadVideoFrame(url, true);
  // 仅在 HEAD 表明文件存在（2xx，大小未知也算）时才去 crossOrigin 重试
  if (!frame && headLen !== null && headLen !== "missing") {
    // anonymous 加载失败但文件存在：去掉 crossOrigin 再试一次；
    // 这一路能加载但 canvas 不可读像素时判 direct
    frame = await loadVideoFrame(url, false);
  }
  if (!frame) return { state: "failed" };
  const src = encodeThumbnail(
    frame.el,
    frame.width,
    frame.height,
    "image/jpeg",
    JPEG_QUALITY,
  );
  destroyVideoEl(frame.el);
  // 原始宽高非法：只是这一张取不到尺寸，判 failed 占位，不连累别的卡
  if (src === "bad-source") return { state: "failed" };
  if (src === null) {
    markDirect();
    return { state: "direct" };
  }
  return { state: "ready", src, w: frame.width, h: frame.height };
}

/* ---------- 取帧（图片） ---------- */

function loadImage(
  url: string,
  anonymous: boolean,
): Promise<HTMLImageElement | null> {
  return new Promise((resolve) => {
    const img = new Image();
    if (anonymous) img.crossOrigin = "anonymous";
    img.decoding = "async";
    const timer = window.setTimeout(() => {
      img.onload = null;
      img.onerror = null;
      // 去掉 src 中止还在进行的下载（空字符串会被解析成当前文档地址，
      // 要用 removeAttribute）
      img.removeAttribute("src");
      resolve(null);
    }, IMAGE_LOAD_TIMEOUT_MS);
    img.onload = () => {
      window.clearTimeout(timer);
      resolve(img);
    };
    img.onerror = () => {
      window.clearTimeout(timer);
      resolve(null);
    };
    img.src = url;
  });
}

async function captureImage(
  url: string,
  fileName: string,
  headLen: HeadSizeResult,
): Promise<WallpaperThumbResult> {
  // png / webp 走 PNG 保住透明通道（WKWebView 的 canvas 编码不出 WebP），
  // jpg / jpeg 等走 JPEG
  const png = /\.(?:png|webp)$/i.test(fileName);
  let img = await loadImage(url, true);
  // 仅在 HEAD 表明文件存在（2xx，大小未知也算）时才去 crossOrigin 重试
  if (!img && headLen !== null && headLen !== "missing") {
    // anonymous 加载失败但文件存在：去掉 crossOrigin 再试一次；
    // 能加载但 canvas 不可读像素时判 direct
    img = await loadImage(url, false);
  }
  if (!img) return { state: "failed" };
  const src = encodeThumbnail(
    img,
    img.naturalWidth,
    img.naturalHeight,
    png ? "image/png" : "image/jpeg",
    JPEG_QUALITY,
  );
  // 原始宽高非法：只是这一张取不到尺寸，判 failed 占位，不连累别的卡
  if (src === "bad-source") return { state: "failed" };
  if (src === null) {
    markDirect();
    return { state: "direct" };
  }
  return { state: "ready", src, w: img.naturalWidth, h: img.naturalHeight };
}

/* ---------- 单键完整流程 ---------- */

/** 单键取缩略图：内存未命中时并行 HEAD + 读持久层；持久记录的 size 与
 *  HEAD 的 content-length 一致才采用；否则生成并回写两层缓存。取帧地址
 *  按文件大小追加查询串（mediaUrl），绕开 WebKit 对 asset URL 的内存缓存 */
async function loadThumb(entry: WallpaperEntry): Promise<WallpaperThumbResult> {
  const key = entry.previewPath;
  const url = convertFileSrc(key);
  const [headLen, record] = await Promise.all([headSize(url), idbGet(key)]);
  // 持久记录有效性防御（IndexedDB 里可能存了损坏值）：src 必须是图片
  // data URL、size 必须是数字，不满足当没有记录
  const validRecord =
    record &&
    typeof record.src === "string" &&
    record.src.startsWith("data:image/") &&
    typeof record.size === "number"
      ? record
      : null;
  if (
    validRecord &&
    typeof headLen === "number" &&
    validRecord.size === headLen
  ) {
    // 持久命中：文件大小未变，直接采用（宽高只在生成时可知，记 0，
    // 预览侧 object-cover 铺满、不依赖宽高）。HEAD 是 "unknown" 时不采用，
    // 照常重新生成——宁可多生成一次，不冒显示旧图的险
    const hit: WallpaperThumbResult = {
      state: "ready",
      src: validRecord.src,
      w: 0,
      h: 0,
    };
    memCache.set(key, { result: hit, size: headLen });
    return hit;
  }
  // 取帧地址追加按文件大小区分的查询串：asset 协议的响应没有
  // Cache-Control，同一 URL 会被 WebKit 的资源内存缓存复用，文件原地
  // 替换后取到的仍是旧内容；不同版本（大小变化）即不同 URL 绕开缓存，
  // 同一版本仍可复用。HEAD 校验继续用不带查询串的地址（asset 协议只按
  // path 定位文件，查询串不参与）。HEAD 未拿到大小时用时间戳，每次取帧
  // 都视为新版本
  const mediaUrl =
    url + "?v=" + (typeof headLen === "number" ? headLen : Date.now());
  const result =
    entry.kind === "video"
      ? await captureVideo(mediaUrl, headLen)
      : await captureImage(mediaUrl, entry.fileName, headLen);
  // 只把 ready 写进内存层（条目记住生成时的文件大小，供会话内校验比对）；
  // 失败不缓存（拷贝中的文件、瞬时繁忙导致超时，之后刷新列表生成新 entry
  // 对象、组件重新请求即自动重试；真正损坏的文件每次挂载多花一次生成开销，
  // 可接受）；direct 由会话级标记兜底，无需逐键缓存
  if (result.state === "ready") {
    memCache.set(key, {
      result,
      size: typeof headLen === "number" ? headLen : -1,
    });
    idbPut(key, {
      src: result.src,
      // HEAD 没拿到大小时记 -1（不用 0，避免与 0 字节文件撞车；-1 不匹配
      // 任何真实 HEAD 大小，之后照常重新生成）
      size: typeof headLen === "number" ? headLen : -1,
      at: Date.now(),
    });
  } else if (result.state === "failed") {
    // 生成失败：清内存条目（重新生成路径可能压着旧条目）+ 删持久记录，
    // 避免坏图残留
    memCache.delete(key);
    idbDelete(key);
  }
  return result;
}

/**
 * 内存命中后的会话内校验（卡片每次挂载 / 每次列表刷新各一次，入队懒执行）：
 * - HEAD 大小与生成时一致 → 沿用内存缩略图（返回同一对象，不触发重渲染）；
 * - 大小变了 → 走 loadThumb 重新生成；
 * - HEAD 明确非 2xx（文件被删 / 未放行）→ failed 占位并清内存条目；
 * - HEAD 超时 / 异常（null）或 2xx 但大小未知（"unknown"）→ 沿用内存
 *   缩略图（不因一次没问到就抹图）。
 */
async function revalidateThumb(
  entry: WallpaperEntry,
): Promise<WallpaperThumbResult> {
  const key = entry.previewPath;
  const cached = memCache.get(key);
  if (!cached) return loadThumb(entry);
  const head = await headSize(convertFileSrc(key));
  if (head === null || head === "unknown") return cached.result;
  if (head === "missing") {
    memCache.delete(key);
    return { state: "failed" };
  }
  if (head === cached.size) return cached.result;
  // 环境已确认取不了帧时不反复尝试媒体加载，直接 direct
  if (directSession) return { state: "direct" };
  return loadThumb(entry);
}

/* ---------- 并发队列（并发 3，先进先出，支持取消排队） ---------- */

interface QueueJob {
  start: () => void;
}

const queue: QueueJob[] = [];
let running = 0;

function pump(): void {
  if (running >= THUMB_CONCURRENCY) return;
  const next = queue.shift();
  if (next) next.start();
}

/**
 * 入队一个生成 / 校验任务。取消只对尚未开始生成的排队任务生效（移出队列
 * 并以 null 结算，调用方滚回视口会重新请求）；已在生成中的任务不受影响，
 * 跑完写缓存并正常结算——保证挂着的卡片只要没卸载，请求过的缩略图生成完
 * 就一定能拿到，不会停在骨架。
 */
function enqueue(
  task: () => Promise<WallpaperThumbResult>,
): QueuedThumbRequest {
  let done = false;
  let settle: (result: WallpaperThumbResult | null) => void = () => {};
  const promise = new Promise<WallpaperThumbResult | null>((resolve) => {
    settle = resolve;
  });
  const job: QueueJob = {
    start: () => {
      running += 1;
      task()
        .catch((): WallpaperThumbResult => ({ state: "failed" }))
        .then((result) => {
          running -= 1;
          done = true;
          settle(result);
          pump();
        });
    },
  };
  if (running < THUMB_CONCURRENCY) job.start();
  else queue.push(job);
  return {
    promise,
    cancel: (): boolean => {
      if (done) return false;
      const i = queue.indexOf(job);
      if (i >= 0) {
        // 仍在队列（尚未开始生成）：移除并立即以 null 结算
        queue.splice(i, 1);
        done = true;
        settle(null);
        return true;
      }
      // 已在生成中：不中断，让它跑完写缓存并正常结算
      return false;
    },
  };
}

/* ---------- 在途合并 ---------- */

interface InflightSlot {
  req: QueuedThumbRequest;
  refs: number;
}

const inflight = new Map<string, InflightSlot>();

/** 最后一个引用放弃时对在途任务调 cancel，透传其结果（true = 任务尚在
 *  排队、已被取消） */
function releaseInflight(key: string, slot: InflightSlot): boolean {
  slot.refs -= 1;
  if (slot.refs > 0) return false;
  // 只在「还在排队、已被取消」时立刻删槽；任务已经在生成中的槽留给
  // 结算回调删，列表刷新后的新请求自然并入在途任务，避免同一个文件
  // 被解码、编码两遍
  const cancelledNow = slot.req.cancel();
  if (cancelledNow && inflight.get(key) === slot) inflight.delete(key);
  return cancelledNow;
}

function immediateRequest(
  result: WallpaperThumbResult,
): QueuedThumbRequest {
  return { promise: Promise.resolve(result), cancel: () => false };
}

/**
 * 请求一个壁纸条目的缩略图（并发 3 排队；同键在途请求合并）。
 * direct 会话且无内存条目时同步结算；其余入队异步生成 / 校验（内存命中
 * 走 revalidateThumb 校验文件，其余走 loadThumb 生成）。
 * 组件卸载或滚离视口时对返回句柄调 cancel（只取消尚未开始生成的排队
 * 任务；已在生成中的会跑完写缓存并正常结算）。
 */
export function requestWallpaperThumb(
  entry: WallpaperEntry,
): QueuedThumbRequest {
  const key = entry.previewPath;
  const cached = memCache.get(key);
  // 已有 ready 条目优先于 direct 会话标记（已拿到的是真实成果，只需校验）
  if (!cached && directSession) return immediateRequest(DIRECT_RESULT);
  const slot = inflight.get(key);
  if (slot) {
    slot.refs += 1;
    return {
      promise: slot.req.promise,
      cancel: () => releaseInflight(key, slot),
    };
  }
  // 内存命中也要再校验一次（每挂载 / 每列表刷新一次）：文件被替换或损坏
  // 时会话内跟着刷新；校验在并发队列里执行，保持懒加载与并发上限
  const req = enqueue(
    cached ? () => revalidateThumb(entry) : () => loadThumb(entry),
  );
  const fresh: InflightSlot = { req, refs: 1 };
  inflight.set(key, fresh);
  void req.promise.then(() => {
    // 结算（完成或取消）后清理合并槽，避免拦住后续新请求
    if (inflight.get(key) === fresh) inflight.delete(key);
  });
  return {
    promise: req.promise,
    cancel: () => releaseInflight(key, fresh),
  };
}

/* ---------- 挂载登记与持久层清理 ---------- */

const mountedKeys = new Set<string>();
let pruneTimer = 0;
let prunedRound = false;

/**
 * 登记一个已挂载的缩略图组件（键 = previewPath），返回注销函数（卸载时调）。
 * 挂载后 1500ms 内没有新挂载，就以当前挂载集合为准清理持久层里不再使用的
 * 键；挂载集合从空变非空视为一次新的进入皮肤页，允许再清一次。
 */
export function registerThumbMount(key: string): () => void {
  const firstOfRound = mountedKeys.size === 0;
  mountedKeys.add(key);
  if (firstOfRound) prunedRound = false;
  if (pruneTimer) window.clearTimeout(pruneTimer);
  pruneTimer = window.setTimeout(() => {
    pruneTimer = 0;
    // 集合为空说明这次进入没有实际渲染出任何壁纸卡，不据此清空
    if (prunedRound || mountedKeys.size === 0) return;
    prunedRound = true;
    // 传拷贝：事务回调异步执行时集合可能已变（组件卸载），避免误删
    void idbPrune(new Set(mountedKeys));
  }, PRUNE_DELAY_MS);
  return () => {
    mountedKeys.delete(key);
  };
}
