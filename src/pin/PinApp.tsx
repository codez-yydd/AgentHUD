import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { closePin, copyShot, getPinData, saveShot } from "../shot/api";
import type { PinData } from "../shot/types";
import "./pin.css";

function clamp(value: number, min: number, max: number) {
  return Math.max(min, Math.min(max, value));
}

export default function PinApp() {
  const [data, setData] = useState<PinData | null>(null);
  const [toast, setToast] = useState("");
  const [busy, setBusy] = useState(false);
  const toastTimer = useRef<number | null>(null);
  const resizeRef = useRef<{
    startX: number;
    startY: number;
    width: number;
    height: number;
    ratio: number;
  } | null>(null);

  const id = Number(new URLSearchParams(window.location.search).get("id"));

  useEffect(() => {
    if (!id) return;
    void getPinData(id)
      .then(setData)
      .catch((error) => setToast(String(error)));
  }, [id]);

  useEffect(() => {
    return () => {
      if (toastTimer.current) window.clearTimeout(toastTimer.current);
    };
  }, []);

  const notify = (message: string) => {
    setToast(message);
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(""), 1600);
  };

  const copy = async () => {
    if (!data || busy) return;
    setBusy(true);
    try {
      await copyShot(data.image);
      notify("已复制");
    } catch (error) {
      notify(String(error));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!data || busy) return;
    setBusy(true);
    try {
      const item = await saveShot(data.image, "png");
      notify(`已保存 ${item.name}`);
    } catch (error) {
      notify(String(error));
    } finally {
      setBusy(false);
    }
  };

  const close = () => {
    if (!id) return;
    void closePin(id).catch(() => {
      void getCurrentWindow().close();
    });
  };

  const beginResize = async (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!data) return;
    event.preventDefault();
    event.stopPropagation();
    const win = getCurrentWindow();
    const scale = await win.scaleFactor();
    const size = await win.innerSize();
    resizeRef.current = {
      startX: event.screenX,
      startY: event.screenY,
      width: size.width / scale,
      height: size.height / scale,
      ratio: data.width / Math.max(1, data.height),
    };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  };

  const moveResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    const state = resizeRef.current;
    if (!state) return;
    const desiredW = Math.max(
      state.width + (event.screenX - state.startX),
      (state.height + (event.screenY - state.startY)) * state.ratio
    );
    const width = clamp(desiredW, 140, 1200);
    const height = width / state.ratio;
    void getCurrentWindow().setSize(new LogicalSize(Math.round(width), Math.round(height)));
  };

  const endResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    resizeRef.current = null;
    if ((event.currentTarget as HTMLElement).hasPointerCapture(event.pointerId)) {
      (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
    }
  };

  return (
    <div className="pin-root">
      <div className="pin-shell" data-tauri-drag-region>
        {data ? (
          <>
            <img
              className="pin-image"
              src={data.image}
              alt=""
              draggable={false}
              data-tauri-drag-region
              onDoubleClick={() => void copy()}
            />
            <div className="pin-toolbar">
              <button type="button" title="复制" onClick={() => void copy()}>
                <PinIcon name="copy" />
              </button>
              <button type="button" title="保存" onClick={() => void save()}>
                <PinIcon name="save" />
              </button>
              <button type="button" className="pin-close" title="关闭" onClick={close}>
                <PinIcon name="close" />
              </button>
            </div>
            <div className="pin-hint">
              <span>{data.width} × {data.height}</span>
              <span>双击复制 · 拖动移动</span>
            </div>
            <div
              className="pin-resize"
              title="拖动缩放"
              onPointerDown={(event) => void beginResize(event)}
              onPointerMove={moveResize}
              onPointerUp={endResize}
              onPointerCancel={endResize}
            />
          </>
        ) : (
          <div className="pin-loading">
            <span className="pin-spinner" />
          </div>
        )}
      </div>
      {toast ? <div className="pin-toast">{toast}</div> : null}
    </div>
  );
}

function PinIcon({ name }: { name: "copy" | "save" | "close" }) {
  const paths = {
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
  };
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {paths[name]}
    </svg>
  );
}
