import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
} from "react";
import { IS_TAURI } from "../island/api";
import {
  closeShot,
  copyShot,
  copyText,
  getSessionImage,
  getShotSession,
  getShotSettings,
  ocrShot,
  openShot,
  openShotFolder,
  pinShot,
  qrShot,
  recordGif,
  saveShot,
  scrollShot,
  shotReady,
} from "./api";
import type {
  OcrResult,
  QrResult,
  RecordResult,
  ShotRect,
  ShotSession,
  ShotSettings,
} from "./types";

type Tool =
  | "cursor"
  | "rect"
  | "ellipse"
  | "arrow"
  | "pen"
  | "marker"
  | "text"
  | "number"
  | "mosaic"
  | "blur";

interface Point {
  x: number;
  y: number;
}

interface BaseOp {
  color: string;
  width: number;
}

type Op =
  | (BaseOp & { type: "rect"; x: number; y: number; w: number; h: number })
  | (BaseOp & { type: "ellipse"; x: number; y: number; w: number; h: number })
  | (BaseOp & { type: "arrow"; x1: number; y1: number; x2: number; y2: number })
  | (BaseOp & { type: "pen"; points: number[] })
  | (BaseOp & { type: "marker"; points: number[] })
  | (BaseOp & { type: "text"; x: number; y: number; text: string; size: number })
  | (BaseOp & { type: "number"; x: number; y: number; n: number; size: number })
  | (BaseOp & { type: "mosaic"; x: number; y: number; w: number; h: number })
  | (BaseOp & { type: "blur"; x: number; y: number; w: number; h: number });

interface Size {
  w: number;
  h: number;
}

interface Context {
  backdrop: string;
  width: number;
  height: number;
}

interface DisplayRect {
  left: number;
  top: number;
  width: number;
  height: number;
  scale: number;
}

interface SelectDrag {
  mode: "new" | "move" | "resize";
  handle: string;
  start: Point;
  origin: ShotRect;
  selection: ShotRect;
}

interface DrawDrag {
  start: Point;
  points: number[];
}

const COLORS = ["#ffffff", "#ff5d6c", "#ffb84d", "#56d98b", "#6ea8ff", "#c79bff", "#ff7ad9"];
const WIDTHS = [2, 4, 6, 9];
const TOOL_META: { key: Tool; label: string; hotkey: string }[] = [
  { key: "cursor", label: "选择", hotkey: "V" },
  { key: "rect", label: "矩形", hotkey: "R" },
  { key: "ellipse", label: "椭圆", hotkey: "O" },
  { key: "arrow", label: "箭头", hotkey: "A" },
  { key: "pen", label: "画笔", hotkey: "P" },
  { key: "marker", label: "高亮", hotkey: "H" },
  { key: "text", label: "文字", hotkey: "T" },
  { key: "number", label: "序号", hotkey: "N" },
  { key: "mosaic", label: "马赛克", hotkey: "M" },
  { key: "blur", label: "模糊", hotkey: "B" },
];

const HANDLES = [
  ["nw", 0, 0],
  ["n", 50, 0],
  ["ne", 100, 0],
  ["e", 100, 50],
  ["se", 100, 100],
  ["s", 50, 100],
  ["sw", 0, 100],
  ["w", 0, 50],
] as const;

function clamp(value: number, min: number, max: number) {
  return Math.max(min, Math.min(max, value));
}

function normRect(rect: ShotRect): ShotRect {
  const x = Math.min(rect.x, rect.x + rect.width);
  const y = Math.min(rect.y, rect.y + rect.height);
  return { x, y, width: Math.abs(rect.width), height: Math.abs(rect.height) };
}

function displaySize(rect: ShotRect): Size {
  return { w: Math.max(1, Math.round(rect.width)), h: Math.max(1, Math.round(rect.height)) };
}

function useViewport() {
  const [size, setSize] = useState({ width: window.innerWidth, height: window.innerHeight });
  useEffect(() => {
    const onResize = () => setSize({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return size;
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("截图图片加载失败"));
    image.src = src;
  });
}

function playTone() {
  try {
    const AudioCtor = window.AudioContext;
    if (!AudioCtor) return;
    const ctx = new AudioCtor();
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = "sine";
    osc.frequency.value = 880;
    gain.gain.setValueAtTime(0.0001, ctx.currentTime);
    gain.gain.exponentialRampToValueAtTime(0.06, ctx.currentTime + 0.012);
    gain.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + 0.11);
    osc.connect(gain).connect(ctx.destination);
    osc.start();
    osc.stop(ctx.currentTime + 0.12);
    window.setTimeout(() => void ctx.close(), 260);
  } catch {
    /* 音频不可用时静默 */
  }
}

export default function ShotApp() {
  const viewport = useViewport();
  const [session, setSession] = useState<ShotSession | null>(null);
  const [phase, setPhase] = useState<"loading" | "select" | "edit">("loading");
  const [selection, setSelection] = useState<ShotRect | null>(null);
  const [context, setContext] = useState<Context | null>(null);
  const [origin, setOrigin] = useState<ShotRect | null>(null);
  const [baseImage, setBaseImage] = useState("");
  const [baseSize, setBaseSize] = useState<Size>({ w: 0, h: 0 });
  const [tool, setTool] = useState<Tool>("rect");
  const [color, setColor] = useState(COLORS[1]);
  const [stroke, setStroke] = useState(WIDTHS[1]);
  const [ops, setOps] = useState<Op[]>([]);
  const [redo, setRedo] = useState<Op[]>([]);
  const [draft, setDraft] = useState<Op | null>(null);
  const [textInput, setTextInput] = useState<{ x: number; y: number; value: string } | null>(null);
  const [toast, setToast] = useState("");
  const [busy, setBusy] = useState("");
  const [modal, setModal] = useState<
    | { kind: "ocr"; result: OcrResult }
    | { kind: "qr"; result: QrResult }
    | { kind: "record"; result: RecordResult }
    | null
  >(null);
  const [settings, setSettings] = useState<ShotSettings | null>(null);
  const [historyNote, setHistoryNote] = useState("");
  const [scrollAvailable, setScrollAvailable] = useState(true);

  const stageRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const baseImgRef = useRef<HTMLImageElement | null>(null);
  const selectDragRef = useRef<SelectDrag | null>(null);
  const drawDragRef = useRef<DrawDrag | null>(null);
  const draftRef = useRef<Op | null>(null);
  const toastTimerRef = useRef<number | null>(null);
  const cancelTextRef = useRef(false);
  const blobUrlsRef = useRef<string[]>([]);

  const makeImageUrl = useCallback((bytes: ArrayBuffer) => {
    const url = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
    blobUrlsRef.current.push(url);
    return url;
  }, []);

  useEffect(() => {
    return () => {
      blobUrlsRef.current.forEach((url) => URL.revokeObjectURL(url));
      blobUrlsRef.current = [];
    };
  }, []);

  const showToast = useCallback((message: string, ms = 1800) => {
    setToast(message);
    if (toastTimerRef.current) window.clearTimeout(toastTimerRef.current);
    toastTimerRef.current = window.setTimeout(() => setToast(""), ms);
  }, []);

  useEffect(() => {
    return () => {
      if (toastTimerRef.current) window.clearTimeout(toastTimerRef.current);
    };
  }, []);

  useEffect(() => {
    void getShotSettings().then(setSettings).catch(() => {});
  }, []);

  const idFromUrl = useMemo(() => {
    const id = Number(new URLSearchParams(window.location.search).get("id"));
    return Number.isFinite(id) && id > 0 ? id : IS_TAURI ? 0 : 1;
  }, []);

  useEffect(() => {
    let dead = false;
    void (async () => {
      try {
        if (!idFromUrl) throw new Error("截图会话参数缺失");
        const next = await getShotSession(idFromUrl);
        const bytes = await getSessionImage(idFromUrl);
        if (dead) return;
        const imageUrl = makeImageUrl(bytes);
        const hydrated = { ...next, image: imageUrl, backdrop: imageUrl };
        setSession(hydrated);
        setScrollAvailable(next.mode === "region");
        setContext({
          backdrop: imageUrl,
          width: next.width,
          height: next.height,
        });
        if (next.mode === "edit") {
          await beginEdit(imageUrl, next.origin, imageUrl);
          if (!IS_TAURI && new URLSearchParams(window.location.search).get("annotate") === "1") {
            setOps([
              { type: "rect", x: 438, y: 270, w: 672, h: 150, color: "#6ea8ff", width: 4 },
              { type: "number", x: 1182, y: 504, n: 1, size: 28, color: "#ff5d6c", width: 4 },
              { type: "text", x: 1216, y: 504, text: "重点看这里", size: 22, color: "#ffffff", width: 4 },
              { type: "arrow", x1: 1170, y1: 490, x2: 1120, y2: 428, color: "#c79bff", width: 4 },
              { type: "marker", points: [466, 391, 520, 390, 575, 392], color: "#ffb84d", width: 5 },
              { type: "text", x: 597, y: 391, text: "一键截图", size: 21, color: "#a06a00", width: 4 },
            ]);
          }
        } else {
          setPhase("select");
          if (!IS_TAURI && new URLSearchParams(window.location.search).get("select") === "1") {
            setSelection({ x: 420, y: 250, width: 1000, height: 560 });
          }
        }
        await new Promise((resolve) => window.setTimeout(resolve, 40));
        await shotReady(idFromUrl);
        if (settings?.playSound) playTone();
      } catch (error) {
        if (!dead) {
          showToast(String(error), 4200);
          // 会话/图片读写失败时不要留下一个隐藏窗口和隐藏的 HUD。
          window.setTimeout(() => void closeShot(), 2600);
        }
      }
    })();
    return () => {
      dead = true;
    };
    // BEGIN 只在窗口创建时执行一次；beginEdit 是稳定纯函数式的状态写入。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idFromUrl]);

  async function beginEdit(image: string, nextOrigin: ShotRect | null, backdrop?: string) {
    const img = await loadImage(image);
    baseImgRef.current = img;
    setBaseImage(image);
    setBaseSize({ w: img.naturalWidth, h: img.naturalHeight });
    setOrigin(nextOrigin);
    setOps([]);
    setRedo([]);
    setDraft(null);
    setTextInput(null);
    if (backdrop) {
      setContext((current) =>
        current ? { ...current, backdrop } : { backdrop, width: img.naturalWidth, height: img.naturalHeight }
      );
    }
    setPhase("edit");
  }

  const display = useMemo<DisplayRect | null>(() => {
    if (!context || baseSize.w <= 0 || baseSize.h <= 0) return null;
    if (origin) {
      const scale = viewport.width / Math.max(1, context.width);
      return {
        left: origin.x * scale,
        top: origin.y * scale,
        width: baseSize.w * scale,
        height: baseSize.h * scale,
        scale,
      };
    }
    const maxW = Math.max(320, viewport.width - 150);
    const maxH = Math.max(220, viewport.height - 170);
    const scale = Math.min(maxW / baseSize.w, maxH / baseSize.h, 1);
    return {
      left: (viewport.width - baseSize.w * scale) / 2,
      top: (viewport.height - baseSize.h * scale) / 2,
      width: baseSize.w * scale,
      height: baseSize.h * scale,
      scale,
    };
  }, [baseSize, context, origin, viewport.height, viewport.width]);

  const selectScale = session ? viewport.width / Math.max(1, session.width) : 1;

  const selectionCss = useMemo(() => {
    if (!selection) return null;
    const rect = normRect(selection);
    return {
      left: rect.x * selectScale,
      top: rect.y * selectScale,
      width: rect.width * selectScale,
      height: rect.height * selectScale,
      rect,
    };
  }, [selection, selectScale]);

  const redraw = useCallback(() => {
    const canvas = canvasRef.current;
    const image = baseImgRef.current;
    if (!canvas || !image || !display) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
    const unit = 1 / Math.max(0.05, display.scale);
    for (const op of ops) drawOp(ctx, op, unit);
    if (draft) drawOp(ctx, draft, unit);
  }, [display, draft, ops]);

  useEffect(() => {
    redraw();
  }, [redraw, baseImage, baseSize.w, baseSize.h]);

  useEffect(() => {
    if (phase !== "edit") return;
    const canvas = canvasRef.current;
    if (canvas) {
      canvas.width = baseSize.w;
      canvas.height = baseSize.h;
    }
    redraw();
  }, [baseSize.h, baseSize.w, phase, redraw]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const focused = document.activeElement;
      const typing =
        focused instanceof HTMLInputElement ||
        focused instanceof HTMLTextAreaElement ||
        (focused instanceof HTMLElement && focused.isContentEditable);
      if (event.key === "Escape") {
        event.preventDefault();
        if (modal) {
          setModal(null);
          return;
        }
        if (typing) {
          setTextInput(null);
          (focused as HTMLElement).blur();
          return;
        }
        void closeShot();
        return;
      }
      if (typing) return;
      if (phase === "select" && event.key === "Enter" && selection) {
        event.preventDefault();
        void cropSelection();
        return;
      }
      if (
        phase === "select" &&
        (event.ctrlKey || event.metaKey) &&
        event.shiftKey &&
        event.key.toLowerCase() === "r" &&
        selection &&
        scrollAvailable
      ) {
        event.preventDefault();
        void finishScroll();
        return;
      }
      if (
        phase === "select" &&
        (event.ctrlKey || event.metaKey) &&
        event.shiftKey &&
        event.key.toLowerCase() === "g" &&
        selection
      ) {
        event.preventDefault();
        void finishRecord();
        return;
      }
      if (phase !== "edit") return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
        event.preventDefault();
        if (event.shiftKey) redoStep();
        else undoStep();
        return;
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "y") {
        event.preventDefault();
        redoStep();
        return;
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "c") {
        event.preventDefault();
        void finishCopy();
        return;
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void finishSave();
        return;
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "p") {
        event.preventDefault();
        void finishPin();
        return;
      }
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "o") {
        event.preventDefault();
        void runOcr();
        return;
      }
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "q") {
        event.preventDefault();
        void runQr();
        return;
      }
      const meta = TOOL_META.find((t) => t.hotkey.toLowerCase() === event.key.toLowerCase());
      if (meta) setTool(meta.key);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modal, phase, selection, settings, ops]);

  const stagePoint = (event: ReactPointerEvent): Point => {
    const stage = stageRef.current;
    if (!stage) return { x: 0, y: 0 };
    const rect = stage.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  };

  const selectPoint = (event: ReactPointerEvent): Point => {
    const point = stagePoint(event);
    return {
      x: clamp(point.x / Math.max(0.01, selectScale), 0, session?.width ?? 0),
      y: clamp(point.y / Math.max(0.01, selectScale), 0, session?.height ?? 0),
    };
  };

  const startSelection = (event: ReactPointerEvent) => {
    if (phase !== "select" || !session) return;
    const target = event.target as HTMLElement;
    if (target.closest(".shot-floating")) return;
    const point = selectPoint(event);
    const stage = stageRef.current;
    stage?.setPointerCapture(event.pointerId);
    if (selection) {
      const rect = normRect(selection);
      const inside =
        point.x >= rect.x &&
        point.x <= rect.x + rect.width &&
        point.y >= rect.y &&
        point.y <= rect.y + rect.height;
      if (inside) {
        selectDragRef.current = {
          mode: "move",
          handle: "",
          start: point,
          origin: rect,
          selection: rect,
        };
        return;
      }
    }
    const next = { x: point.x, y: point.y, width: 0, height: 0 };
    setSelection(next);
    selectDragRef.current = {
      mode: "new",
      handle: "",
      start: point,
      origin: next,
      selection: next,
    };
  };

  const startResize = (event: ReactPointerEvent, handle: string) => {
    if (!selection) return;
    event.stopPropagation();
    const point = selectPoint(event);
    const rect = normRect(selection);
    stageRef.current?.setPointerCapture(event.pointerId);
    selectDragRef.current = {
      mode: "resize",
      handle,
      start: point,
      origin: rect,
      selection: rect,
    };
  };

  const moveSelection = (event: ReactPointerEvent) => {
    if (phase !== "select" || !selectDragRef.current || !session) return;
    const point = selectPoint(event);
    const drag = selectDragRef.current;
    if (drag.mode === "new") {
      const rect = normRect({
        x: drag.start.x,
        y: drag.start.y,
        width: point.x - drag.start.x,
        height: point.y - drag.start.y,
      });
      setSelection(rect);
      return;
    }
    if (drag.mode === "move") {
      const dx = point.x - drag.start.x;
      const dy = point.y - drag.start.y;
      const width = drag.origin.width;
      const height = drag.origin.height;
      setSelection({
        x: clamp(drag.origin.x + dx, 0, session.width - width),
        y: clamp(drag.origin.y + dy, 0, session.height - height),
        width,
        height,
      });
      return;
    }
    const rect = { ...drag.origin };
    const minSize = 8;
    if (drag.handle.includes("w")) {
      const nx = clamp(point.x, 0, rect.x + rect.width - minSize);
      rect.width += rect.x - nx;
      rect.x = nx;
    }
    if (drag.handle.includes("e")) {
      rect.width = clamp(point.x - rect.x, minSize, session.width - rect.x);
    }
    if (drag.handle.includes("n")) {
      const ny = clamp(point.y, 0, rect.y + rect.height - minSize);
      rect.height += rect.y - ny;
      rect.y = ny;
    }
    if (drag.handle.includes("s")) {
      rect.height = clamp(point.y - rect.y, minSize, session.height - rect.y);
    }
    setSelection(rect);
  };

  const endSelection = (event: ReactPointerEvent) => {
    if (selectDragRef.current) {
      selectDragRef.current = null;
      if (stageRef.current?.hasPointerCapture(event.pointerId)) {
        stageRef.current.releasePointerCapture(event.pointerId);
      }
    }
    setSelection((current) => {
      if (!current) return null;
      const rect = normRect(current);
      if (rect.width < 6 || rect.height < 6) return null;
      return rect;
    });
  };

  async function cropSelection() {
    if (!session?.image || !selection) return;
    const rect = normRect(selection);
    if (rect.width < 2 || rect.height < 2) return;
    try {
      const img = await loadImage(session.image);
      const canvas = document.createElement("canvas");
      const size = displaySize(rect);
      canvas.width = size.w;
      canvas.height = size.h;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("创建截图画布失败");
      ctx.drawImage(
        img,
        Math.round(rect.x),
        Math.round(rect.y),
        size.w,
        size.h,
        0,
        0,
        size.w,
        size.h
      );
      await beginEdit(canvas.toDataURL("image/png"), rect);
    } catch (error) {
      showToast(String(error));
    }
  }

  function selectFull() {
    if (!session?.image) return;
    setScrollAvailable(false);
    void beginEdit(session.image, {
      x: 0,
      y: 0,
      width: session.width,
      height: session.height,
    });
  }

  async function finishScroll() {
    if (!session || !selection || busy) return;
    const rect = normRect(selection);
    setBusy("正在滚动拼接长截图…");
    try {
      const result = await scrollShot(session.id, {
        x: Math.round(rect.x),
        y: Math.round(rect.y),
        width: Math.round(rect.width),
        height: Math.round(rect.height),
      });
      const bytes = await getSessionImage(session.id);
      const imageUrl = makeImageUrl(bytes);
      setSession((current) =>
        current
          ? {
              ...current,
              mode: "edit",
              image: imageUrl,
              backdrop: imageUrl,
              width: result.width,
              height: result.height,
              origin: null,
              title: "滚动长截图",
            }
          : current
      );
      setScrollAvailable(false);
      setContext({ backdrop: imageUrl, width: result.width, height: result.height });
      await beginEdit(imageUrl, null, imageUrl);
      setSelection(null);
      showToast(`已拼接 ${result.frames} 帧 · ${result.width}×${result.height}`, 2600);
    } catch (error) {
      showToast(String(error), 3600);
    } finally {
      setBusy("");
    }
  }

  async function finishRecord() {
    if (!session || !selection || busy) return;
    const rect = normRect(selection);
    setBusy("正在录制 8 秒 GIF…");
    try {
      const result = await recordGif(
        session.id,
        {
          x: Math.round(rect.x),
          y: Math.round(rect.y),
          width: Math.round(rect.width),
          height: Math.round(rect.height),
        },
        8
      );
      setModal({ kind: "record", result });
    } catch (error) {
      showToast(String(error), 4200);
    } finally {
      setBusy("");
    }
  }

  const imagePoint = (event: ReactPointerEvent): Point => {
    const canvas = canvasRef.current;
    if (!canvas || !display) return { x: 0, y: 0 };
    const rect = canvas.getBoundingClientRect();
    return {
      x: clamp(((event.clientX - rect.left) / rect.width) * canvas.width, 0, canvas.width),
      y: clamp(((event.clientY - rect.top) / rect.height) * canvas.height, 0, canvas.height),
    };
  };

  const drawPointerDown = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    if (phase !== "edit" || busy) return;
    const point = imagePoint(event);
    if (tool === "cursor") return;
    if (tool === "text") {
      setTextInput({ x: point.x, y: point.y, value: "" });
      return;
    }
    if (tool === "number") {
      const n = ops.reduce((max, op) => (op.type === "number" ? Math.max(max, op.n) : max), 0) + 1;
      const unit = 1 / Math.max(0.05, display?.scale ?? 1);
      setOps((current) => [
        ...current,
        {
          type: "number",
          x: point.x,
          y: point.y,
          n,
          size: 28 * unit,
          color,
          width: stroke * unit,
        },
      ]);
      setRedo([]);
      return;
    }
    event.currentTarget.setPointerCapture(event.pointerId);
    drawDragRef.current = { start: point, points: [point.x, point.y] };
    const unit = 1 / Math.max(0.05, display?.scale ?? 1);
    let next: Op | null = null;
    if (tool === "rect" || tool === "ellipse" || tool === "mosaic" || tool === "blur") {
      next = {
        type: tool,
        x: point.x,
        y: point.y,
        w: 0,
        h: 0,
        color,
        width: stroke * unit,
      };
    } else if (tool === "arrow") {
      next = {
        type: "arrow",
        x1: point.x,
        y1: point.y,
        x2: point.x,
        y2: point.y,
        color,
        width: stroke * unit,
      };
    } else if (tool === "pen" || tool === "marker") {
      next = {
        type: tool,
        points: [point.x, point.y],
        color,
        width: (tool === "marker" ? stroke * 3.2 : stroke) * unit,
      };
    }
    draftRef.current = next;
    setDraft(next);
  };

  const drawPointerMove = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const drag = drawDragRef.current;
    if (!drag || !draftRef.current) return;
    const point = imagePoint(event);
    const start = drag.start;
    const current = draftRef.current;
    let next: Op;
    if (current.type === "rect" || current.type === "ellipse" || current.type === "mosaic" || current.type === "blur") {
      next = { ...current, x: Math.min(start.x, point.x), y: Math.min(start.y, point.y), w: Math.abs(point.x - start.x), h: Math.abs(point.y - start.y) };
    } else if (current.type === "arrow") {
      next = { ...current, x2: point.x, y2: point.y };
    } else if (current.type === "pen" || current.type === "marker") {
      drag.points = [...drag.points, point.x, point.y];
      next = { ...current, points: drag.points };
    } else {
      return;
    }
    draftRef.current = next;
    setDraft(next);
  };

  const drawPointerUp = (event: ReactPointerEvent<HTMLCanvasElement>) => {
    const drag = drawDragRef.current;
    const current = draftRef.current;
    if (!drag) return;
    drawDragRef.current = null;
    draftRef.current = null;
    setDraft(null);
    if (!current) return;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    let commit: Op | null = current;
    if (current.type === "rect" || current.type === "ellipse" || current.type === "mosaic" || current.type === "blur") {
      if (current.w < 3 || current.h < 3) commit = null;
    } else if (current.type === "arrow") {
      if (Math.hypot(current.x2 - current.x1, current.y2 - current.y1) < 5) commit = null;
    } else if (current.type === "pen" || current.type === "marker") {
      if (current.points.length < 6) commit = null;
    }
    if (commit) {
      setOps((list) => [...list, commit as Op]);
      setRedo([]);
    }
  };

  function commitText() {
    if (cancelTextRef.current) {
      cancelTextRef.current = false;
      setTextInput(null);
      return;
    }
    if (!textInput || !textInput.value.trim()) {
      setTextInput(null);
      return;
    }
    const unit = 1 / Math.max(0.05, display?.scale ?? 1);
    const op: Op = {
      type: "text",
      x: textInput.x,
      y: textInput.y,
      text: textInput.value,
      size: 24 * unit,
      color,
      width: stroke * unit,
    };
    setOps((list) => [...list, op]);
    setRedo([]);
    setTextInput(null);
  }

  function undoStep() {
    if (!ops.length) return;
    const last = ops[ops.length - 1];
    setOps(ops.slice(0, -1));
    setRedo([...redo, last]);
  }

  function redoStep() {
    if (!redo.length) return;
    const last = redo[redo.length - 1];
    setOps([...ops, last]);
    setRedo(redo.slice(0, -1));
  }

  function reselect() {
    if (!session) return;
    setPhase("select");
    setSelection(null);
    setOps([]);
    setRedo([]);
    setDraft(null);
    setBaseImage("");
    setOrigin(null);
    setTextInput(null);
    setContext({
      backdrop: session.image ?? session.backdrop ?? "",
      width: session.width,
      height: session.height,
    });
  }

  function exportImage(): string {
    redraw();
    const canvas = canvasRef.current;
    if (!canvas) throw new Error("画布尚未准备好");
    return canvas.toDataURL("image/png");
  }

  async function finishCopy() {
    if (busy) return;
    setBusy("正在复制到剪贴板…");
    try {
      const data = exportImage();
      await copyShot(data);
      showToast("已复制到剪贴板", 900);
      window.setTimeout(() => void closeShot(), 320);
    } catch (error) {
      showToast(String(error), 3600);
      setBusy("");
    }
  }

  async function finishSave() {
    if (busy) return;
    setBusy("正在保存…");
    try {
      const data = exportImage();
      const item = await saveShot(data, settings?.format ?? "png");
      setHistoryNote(`已保存 ${item.name}`);
      let extra = "";
      if (settings?.autoCopy) {
        try {
          await copyShot(data);
          extra = "，已复制";
        } catch {
          extra = "";
        }
      }
      if (settings?.autoPin) {
        try {
          await pinShot(data);
          extra += "，已贴图";
        } catch {
          extra = "";
        }
      }
      showToast(`已保存${extra}`, 1100);
      window.setTimeout(() => void closeShot(), 480);
    } catch (error) {
      showToast(String(error), 3600);
      setBusy("");
    }
  }

  async function finishPin() {
    if (busy) return;
    setBusy("正在创建贴图…");
    try {
      const data = exportImage();
      await pinShot(data);
      showToast("已贴到桌面", 900);
      window.setTimeout(() => void closeShot(), 300);
    } catch (error) {
      showToast(String(error), 3600);
      setBusy("");
    }
  }

  async function runOcr() {
    if (busy) return;
    setBusy("正在识别文字…");
    try {
      const result = await ocrShot(exportImage());
      setModal({ kind: "ocr", result });
    } catch (error) {
      showToast(String(error), 4200);
    } finally {
      setBusy("");
    }
  }

  async function runQr() {
    if (busy) return;
    setBusy("正在识别二维码…");
    try {
      const result = await qrShot(exportImage());
      setModal({ kind: "qr", result });
    } catch (error) {
      showToast(String(error), 4200);
    } finally {
      setBusy("");
    }
  }

  async function copyModalText(text: string) {
    try {
      await copyText(text);
      showToast("文字已复制", 1100);
    } catch {
      showToast("复制失败，请手动选择", 1600);
    }
  }

  const canvasStyle = display
    ? {
        left: display.left,
        top: display.top,
        width: display.width,
        height: display.height,
      }
    : undefined;

  return (
    <div
      ref={stageRef}
      className={`shot-root phase-${phase} ${phase === "edit" ? "tool-" + tool : ""}`}
      onPointerDown={phase === "select" ? startSelection : undefined}
      onPointerMove={phase === "select" ? moveSelection : undefined}
      onPointerUp={phase === "select" ? endSelection : undefined}
      onPointerCancel={phase === "select" ? endSelection : undefined}
      onDoubleClick={() => {
        if (phase === "select" && selection) void cropSelection();
      }}
    >
      {phase === "loading" ? (
        <div className="shot-loading">
          <span className="shot-spinner" />
          <div>{busy || "正在获取屏幕画面…"}</div>
        </div>
      ) : null}

      {phase === "select" && session?.image ? (
        <>
          <img className="shot-freeze" src={session.image} alt="" draggable={false} />
          {!selectionCss ? <div className="shot-dim shot-dim-full" /> : null}
          {selectionCss ? (
            <>
              <div
                className="shot-dim"
                style={{ left: 0, top: 0, width: "100%", height: selectionCss.top }}
              />
              <div
                className="shot-dim"
                style={{
                  left: 0,
                  top: selectionCss.top + selectionCss.height,
                  width: "100%",
                  bottom: 0,
                }}
              />
              <div
                className="shot-dim"
                style={{
                  left: 0,
                  top: selectionCss.top,
                  width: selectionCss.left,
                  height: selectionCss.height,
                }}
              />
              <div
                className="shot-dim"
                style={{
                  left: selectionCss.left + selectionCss.width,
                  right: 0,
                  top: selectionCss.top,
                  height: selectionCss.height,
                }}
              />
              <div
                className="shot-selection"
                style={{
                  left: selectionCss.left,
                  top: selectionCss.top,
                  width: selectionCss.width,
                  height: selectionCss.height,
                }}
              >
                {HANDLES.map(([handle, x, y]) => (
                  <span
                    key={handle}
                    className={`shot-handle handle-${handle}`}
                    style={{ left: `${x}%`, top: `${y}%` }}
                    onPointerDown={(event) => startResize(event, handle)}
                  />
                ))}
                <span className="shot-size-pill">
                  {Math.round(selectionCss.rect.width)} × {Math.round(selectionCss.rect.height)}
                </span>
              </div>
            </>
          ) : null}
          <div className="shot-top-pill">
            <span className="shot-live-dot" />
            <span>{session.title}</span>
            <span className="shot-top-sep" />
            <span>{session.monitor.width} × {session.monitor.height}</span>
            <span className="shot-top-sep" />
            <span>拖动框选</span>
          </div>
          <div className="shot-floating shot-select-actions" onPointerDown={(e) => e.stopPropagation()}>
            <span className="shot-selection-label">
              {selection
                ? `${Math.round(normRect(selection).width)} × ${Math.round(normRect(selection).height)}`
                : "拖动选择区域"}
            </span>
            <button
              type="button"
              className="shot-btn primary"
              disabled={!selection}
              onClick={() => void cropSelection()}
            >
              <ShotIcon name="pen" size={14} /> 标注
            </button>
            {scrollAvailable ? (
              <button
                type="button"
                className="shot-btn"
                disabled={!selection}
                onClick={() => void finishScroll()}
              >
                <ShotIcon name="scroll" size={14} /> 滚动
              </button>
            ) : null}
            <button
              type="button"
              className="shot-btn record"
              disabled={!selection}
              title="录制 8 秒 GIF（Ctrl+Shift+G）"
              onClick={() => void finishRecord()}
            >
              <span className="shot-rec-dot" /> 录制
            </button>
            <button type="button" className="shot-btn" onClick={selectFull}>
              <ShotIcon name="full" size={14} /> 全屏
            </button>
            <button type="button" className="shot-btn ghost" onClick={() => void closeShot()}>
              取消
            </button>
          </div>
          <div className="shot-hint">
            <kbd>拖动</kbd> 选择区域
            <span>·</span>
            <kbd>双击</kbd> 进入标注
            <span>·</span>
            <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd> 滚动
            <span>·</span>
            <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>G</kbd> 录制
            <span>·</span>
            <kbd>Esc</kbd> 取消
          </div>
        </>
      ) : null}

      {phase === "edit" && context ? (
        <>
          <img
            className={`shot-backdrop ${origin ? "is-exact" : ""}`}
            src={context.backdrop}
            alt=""
            draggable={false}
          />
          <div className="shot-editor-vignette" />
          <div className="shot-canvas-wrap" style={canvasStyle}>
            <canvas
              ref={canvasRef}
              className="shot-canvas"
              width={baseSize.w || 1}
              height={baseSize.h || 1}
              onPointerDown={drawPointerDown}
              onPointerMove={drawPointerMove}
              onPointerUp={drawPointerUp}
              onPointerCancel={drawPointerUp}
            />
            {textInput ? (
              <textarea
                className="shot-text-input"
                autoFocus
                style={{
                  left: `${textInput.x * (display?.scale ?? 1)}px`,
                  top: `${textInput.y * (display?.scale ?? 1)}px`,
                  fontSize: `${Math.max(13, 24 * (display?.scale ?? 1))}px`,
                  color,
                  borderColor: color,
                  width: `${Math.max(130, 240 * (display?.scale ?? 1))}px`,
                }}
                value={textInput.value}
                onChange={(event) => setTextInput({ ...textInput, value: event.target.value })}
                onBlur={commitText}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && !event.shiftKey) {
                    event.preventDefault();
                    event.currentTarget.blur();
                  }
                  if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    cancelTextRef.current = true;
                    event.currentTarget.blur();
                  }
                }}
              />
            ) : null}
            <span className="shot-canvas-badge">
              {baseSize.w} × {baseSize.h}
            </span>
          </div>

          <div className="shot-edit-top">
            <span className="shot-live-dot" />
            <span>{session?.title ?? "截图"}</span>
            <span className="shot-top-sep" />
            <span>{baseSize.w} × {baseSize.h}</span>
            {historyNote ? (
              <>
                <span className="shot-top-sep" />
                <span className="shot-saved-note">{historyNote}</span>
              </>
            ) : null}
          </div>

          <div className="shot-toolbar-wrap">
            <div
              className="shot-toolbar"
              onPointerDown={(event) => event.stopPropagation()}
            >
              <div className="shot-tool-group">
                {TOOL_META.map((item) => (
                  <button
                    key={item.key}
                    type="button"
                    className={`shot-tool ${tool === item.key ? "active" : ""}`}
                    title={`${item.label} (${item.hotkey})`}
                    onClick={() => {
                      setTool(item.key);
                      if (item.key !== "text") setTextInput(null);
                    }}
                  >
                    <ShotIcon name={item.key} size={16} />
                  </button>
                ))}
              </div>
              <span className="shot-divider" />
              <div className="shot-color-group">
                {COLORS.map((value) => (
                  <button
                    key={value}
                    type="button"
                    className={`shot-color ${color === value ? "active" : ""}`}
                    style={{ background: value }}
                    title="画笔颜色"
                    onClick={() => setColor(value)}
                  />
                ))}
              </div>
              <span className="shot-divider" />
              <div className="shot-width-group">
                {WIDTHS.map((value) => (
                  <button
                    key={value}
                    type="button"
                    className={`shot-width ${stroke === value ? "active" : ""}`}
                    title={`线条粗细 ${value}`}
                    onClick={() => setStroke(value)}
                  >
                    <span style={{ width: value + 2, height: value + 2 }} />
                  </button>
                ))}
              </div>
              <span className="shot-divider" />
              <div className="shot-action-group">
                <button type="button" className="shot-action" title="撤销 (Ctrl+Z)" onClick={undoStep} disabled={!ops.length}>
                  <ShotIcon name="undo" size={15} />
                </button>
                <button type="button" className="shot-action" title="重做 (Ctrl+Y)" onClick={redoStep} disabled={!redo.length}>
                  <ShotIcon name="redo" size={15} />
                </button>
                <button type="button" className="shot-action" title="重新框选" onClick={reselect} disabled={!session}>
                  <ShotIcon name="crop" size={15} />
                </button>
              </div>
              <span className="shot-divider" />
              <div className="shot-action-group">
                <button type="button" className="shot-action" title="提取文字（Ctrl+Shift+O）" onClick={() => void runOcr()}>
                  <ShotIcon name="ocr" size={15} />
                </button>
                <button type="button" className="shot-action" title="识别二维码（Ctrl+Shift+Q）" onClick={() => void runQr()}>
                  <ShotIcon name="qr" size={15} />
                </button>
                <button type="button" className="shot-action" title="贴到桌面（Ctrl+P）" onClick={() => void finishPin()}>
                  <ShotIcon name="pin" size={15} />
                </button>
                <button type="button" className="shot-action" title="复制 (Ctrl+C)" onClick={() => void finishCopy()}>
                  <ShotIcon name="copy" size={15} />
                </button>
              </div>
              <span className="shot-divider" />
              <button type="button" className="shot-btn save" onClick={() => void finishSave()}>
                <ShotIcon name="save" size={15} /> 保存
              </button>
              <button type="button" className="shot-close" title="关闭 (Esc)" onClick={() => void closeShot()}>
                <ShotIcon name="close" size={15} />
              </button>
            </div>
            <div className="shot-toolbar-hint">
              <kbd>Ctrl</kbd>+<kbd>C</kbd> 复制
              <span>·</span>
              <kbd>Ctrl</kbd>+<kbd>S</kbd> 保存
              <span>·</span>
              <kbd>Ctrl</kbd>+<kbd>Z</kbd> 撤销
              <span>·</span>
              <kbd>⌫</kbd> 重选
            </div>
          </div>
        </>
      ) : null}

      {busy ? (
        <div className="shot-busy">
          <span className="shot-spinner" />
          <span>{busy}</span>
        </div>
      ) : null}

      {toast ? <div className="shot-toast">{toast}</div> : null}

      {modal ? (
        <div className="shot-modal-backdrop" onPointerDown={() => setModal(null)}>
          <div className="shot-modal" onPointerDown={(event) => event.stopPropagation()}>
            <div className="shot-modal-head">
              <div>
                <b>
                  {modal.kind === "ocr"
                    ? "提取文字"
                    : modal.kind === "qr"
                      ? "二维码识别"
                      : "GIF 已保存"}
                </b>
                <span>
                  {modal.kind === "ocr"
                    ? modal.result.ok
                      ? `${modal.result.lines.length} 行 · 本地识别`
                      : "未识别到文字"
                    : modal.kind === "qr"
                      ? modal.result.ok
                        ? `${modal.result.texts.length} 个结果`
                        : "未识别到二维码"
                      : `${modal.result.width} × ${modal.result.height} · ${modal.result.frames} 帧 · ${(modal.result.size / 1024 / 1024).toFixed(1)} MB`}
                </span>
              </div>
              <button type="button" onClick={() => setModal(null)}>
                <ShotIcon name="close" size={15} />
              </button>
            </div>
            {modal.kind === "record" ? (
              <>
                <div className="shot-record-summary">
                  <span className="shot-rec-dot" />
                  <div>
                    <b>录制完成 · {Math.round(modal.result.durationMs / 1000)} 秒</b>
                    <em>{modal.result.path}</em>
                  </div>
                </div>
                <div className="shot-modal-actions">
                  <button
                    type="button"
                    className="shot-btn"
                    onClick={() => void openShotFolder().catch((e) => showToast(String(e)))}
                  >
                    打开文件夹
                  </button>
                  <button
                    type="button"
                    className="shot-btn"
                    onClick={() => void openShot(modal.result.path).catch((e) => showToast(String(e)))}
                  >
                    打开 GIF
                  </button>
                  <button type="button" className="shot-btn primary" onClick={() => setModal(null)}>
                    完成
                  </button>
                </div>
              </>
            ) : modal.kind === "ocr" ? (
              modal.result.ok ? (
                <>
                  <textarea readOnly value={modal.result.text} />
                  <div className="shot-modal-actions">
                    <button type="button" className="shot-btn" onClick={() => void copyModalText(modal.result.text)}>
                      复制全部
                    </button>
                    <button type="button" className="shot-btn primary" onClick={() => setModal(null)}>
                      完成
                    </button>
                  </div>
                </>
              ) : (
                <div className="shot-modal-empty">{modal.result.error || "未识别到文字"}</div>
              )
            ) : modal.result.ok ? (
              <div className="shot-qr-list">
                {modal.result.texts.map((text) => (
                  <button
                    key={text}
                    type="button"
                    className="shot-qr-item"
                    onClick={() => void copyModalText(text)}
                    title="复制"
                  >
                    <span>{text}</span>
                    <ShotIcon name="copy" size={13} />
                  </button>
                ))}
              </div>
            ) : (
              <div className="shot-modal-empty">{modal.result.error || "未识别到二维码"}</div>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}

function drawArrow(
  ctx: CanvasRenderingContext2D,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  width: number
) {
  const angle = Math.atan2(y2 - y1, x2 - x1);
  const head = Math.max(width * 4.2, 14);
  const hx = x2 - Math.cos(angle) * head;
  const hy = y2 - Math.sin(angle) * head;
  ctx.beginPath();
  ctx.moveTo(x1, y1);
  ctx.lineTo(hx, hy);
  ctx.stroke();
  ctx.beginPath();
  ctx.moveTo(x2, y2);
  ctx.lineTo(x2 - Math.cos(angle - 0.46) * head, y2 - Math.sin(angle - 0.46) * head);
  ctx.lineTo(x2 - Math.cos(angle + 0.46) * head, y2 - Math.sin(angle + 0.46) * head);
  ctx.closePath();
  ctx.fill();
}

function drawOp(ctx: CanvasRenderingContext2D, op: Op, unit: number) {
  ctx.save();
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.strokeStyle = op.color;
  ctx.fillStyle = op.color;
  ctx.lineWidth = Math.max(1, op.width);
  if (op.type === "rect") {
    ctx.strokeRect(op.x, op.y, op.w, op.h);
  } else if (op.type === "ellipse") {
    ctx.beginPath();
    ctx.ellipse(op.x + op.w / 2, op.y + op.h / 2, op.w / 2, op.h / 2, 0, 0, Math.PI * 2);
    ctx.stroke();
  } else if (op.type === "arrow") {
    drawArrow(ctx, op.x1, op.y1, op.x2, op.y2, op.width);
  } else if (op.type === "pen" || op.type === "marker") {
    if (op.points.length < 2) {
      ctx.restore();
      return;
    }
    ctx.globalAlpha = op.type === "marker" ? 0.36 : 1;
    ctx.lineWidth = op.type === "marker" ? op.width * 4.2 : op.width;
    ctx.beginPath();
    ctx.moveTo(op.points[0], op.points[1]);
    for (let i = 2; i < op.points.length; i += 2) {
      ctx.lineTo(op.points[i], op.points[i + 1]);
    }
    ctx.stroke();
  } else if (op.type === "text") {
    ctx.font = `600 ${Math.max(10, op.size)}px "Segoe UI", "Microsoft YaHei", sans-serif`;
    ctx.textBaseline = "middle";
    const lines = op.text.split("\n");
    lines.forEach((line, index) => {
      ctx.fillText(line, op.x, op.y + index * op.size * 1.3);
    });
  } else if (op.type === "number") {
    const r = Math.max(10, op.size * 0.62);
    ctx.beginPath();
    ctx.arc(op.x, op.y, r, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = "#fff";
    ctx.font = `700 ${Math.max(9, r * 1.05)}px "Segoe UI", sans-serif`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(String(op.n), op.x, op.y + 1);
  } else if (op.type === "mosaic" || op.type === "blur") {
    const x = Math.max(0, Math.floor(op.x));
    const y = Math.max(0, Math.floor(op.y));
    const w = Math.max(1, Math.floor(op.w));
    const h = Math.max(1, Math.floor(op.h));
    if (w < 2 || h < 2) {
      ctx.restore();
      return;
    }
    const data = ctx.getImageData(x, y, w, h);
    const temp = document.createElement("canvas");
    temp.width = w;
    temp.height = h;
    const tctx = temp.getContext("2d");
    if (!tctx) {
      ctx.restore();
      return;
    }
    tctx.putImageData(data, 0, 0);
    if (op.type === "mosaic") {
      const block = Math.max(4, Math.round(14 * unit));
      const sw = Math.max(1, Math.ceil(w / block));
      const sh = Math.max(1, Math.ceil(h / block));
      const small = document.createElement("canvas");
      small.width = sw;
      small.height = sh;
      const sctx = small.getContext("2d");
      if (!sctx) {
        ctx.restore();
        return;
      }
      sctx.imageSmoothingEnabled = false;
      sctx.drawImage(temp, 0, 0, sw, sh);
      ctx.imageSmoothingEnabled = false;
      ctx.drawImage(small, 0, 0, sw, sh, x, y, w, h);
    } else {
      ctx.save();
      ctx.filter = `blur(${Math.max(3, 9 * unit)}px)`;
      ctx.drawImage(temp, x, y);
      ctx.restore();
    }
  }
  ctx.restore();
}

type IconName =
  | Tool
  | "undo"
  | "redo"
  | "crop"
  | "ocr"
  | "qr"
  | "pin"
  | "copy"
  | "save"
  | "close"
  | "scroll"
  | "full";

function ShotIcon({ name, size = 16 }: { name: IconName; size?: number }) {
  const paths: Record<IconName, ReactElement> = {
    cursor: <path d="m5 3 13 8-6 1.4L9.5 19 5 3Z" />,
    rect: <rect x="3.5" y="5" width="17" height="14" rx="2" />,
    ellipse: <ellipse cx="12" cy="12" rx="8.5" ry="7" />,
    arrow: <path d="m4 19 15-14M12 4h7v7" />,
    pen: <path d="m4 20 4.2-1 10.3-10.3a1.8 1.8 0 0 0 0-2.6l-.6-.6a1.8 1.8 0 0 0-2.6 0L5 15.8 4 20Zm10-13 3 3" />,
    marker: <path d="m5 17 7.7-12.3a1.7 1.7 0 0 1 2.4-.5l1.2.8a1.7 1.7 0 0 1 .5 2.4L9 19H5v-2Zm2.5 2H20" />,
    text: <path d="M5 5h14M12 5v14M8 19h8" />,
    number: (
      <>
        <circle cx="12" cy="12" r="8.5" />
        <path d="M10 9.5 12 8v8M10.5 16h3" />
      </>
    ),
    mosaic: (
      <>
        <rect x="4" y="4" width="6" height="6" rx="1" />
        <rect x="14" y="4" width="6" height="6" rx="1" />
        <rect x="4" y="14" width="6" height="6" rx="1" />
        <rect x="14" y="14" width="6" height="6" rx="1" />
      </>
    ),
    blur: (
      <>
        <circle cx="12" cy="12" r="8" />
        <path d="M8 12a4 4 0 0 1 8 0M10 15h4" />
      </>
    ),
    undo: <path d="M9 7 4 12l5 5M5 12h8a6 6 0 0 1 6 6" />,
    redo: <path d="m15 7 5 5-5 5M19 12h-8a6 6 0 0 0-6 6" />,
    crop: <path d="M7 3v14h14M3 7h14v14" />,
    ocr: (
      <>
        <path d="M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5a1.5 1.5 0 0 1-1.5 1.5H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16" />
        <path d="M8 9h8M8 12h5M8 15h7" />
      </>
    ),
    qr: (
      <>
        <rect x="4" y="4" width="6" height="6" rx="1" />
        <rect x="14" y="4" width="6" height="6" rx="1" />
        <rect x="4" y="14" width="6" height="6" rx="1" />
        <path d="M14 14h2v2h-2zM18 14h2v2M14 18h2v2h2v-2h2v2" />
      </>
    ),
    pin: <path d="M9 4h6l-1 5 4 4-1.5 1.5L12 11l-4.5 3.5L6 13l4-4 8 18" />,
    copy: (
      <>
        <rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2" />
        <path d="M15.5 5.5A2 2 0 0 0 13.5 3.5H5.8a2 2 0 0 0-2 2v8.1a2 2 0 0 0 2 2" />
      </>
    ),
    save: (
      <>
        <path d="M5 4h11l3 3v13H5z" />
        <path d="M8 4v6h8V4M8 20v-6h8v6" />
      </>
    ),
    close: <path d="m6 6 12 12M18 6 6 18" />,
    scroll: (
      <>
        <rect x="5" y="3.5" width="14" height="17" rx="2" />
        <path d="M12 7v8M9 12l3 3 3-3" />
      </>
    ),
    full: (
      <>
        <path d="M4 9V5a1 1 0 0 1 1-1h4M15 4h4a1 1 0 0 1 1 1v4M20 15v4a1 1 0 0 1-1 1h-4M9 20H5a1 1 0 0 1-1-1v-4" />
      </>
    ),
  };
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {paths[name]}
    </svg>
  );
}
