import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import type { IslandBodyRect } from "./types";

export interface NativeGlassGeometry extends IslandBodyRect {
  radii: [number, number, number, number];
}

export function useNativeGlass(available: boolean, enabled: boolean) {
  const [ready, setReady] = useState(false);
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;
  const geometryRef = useRef<NativeGlassGeometry | null>(null);
  const luminance = useRef<number | null>(null);
  const syncRef = useRef(() => {});
  const updateGeometry = useCallback((geometry: NativeGlassGeometry) => {
    geometryRef.current = geometry;
    syncRef.current();
  }, []);

  useEffect(() => {
    if (!available) return;
    let disposed = false;
    let failed = false;
    let listening = false;
    let pending = false;
    let lastGeometry = "";
    let eventVersion = 0;
    let unlistenReady: UnlistenFn | undefined;
    let unlistenLuma: UnlistenFn | undefined;

    const setActive = (active: boolean) => {
      if (!active) luminance.current = null;
      setReady(active);
    };
    const shouldShow = () => !disposed && enabledRef.current && !document.hidden;

    // 同一时刻只提交一帧。拖拽或变形在等待 IPC 时产生的新帧只保留最新一份。
    const sync = () => {
      if (!listening || pending || failed) return;
      const geometry = shouldShow() ? geometryRef.current : null;
      const key = geometry ? JSON.stringify(geometry) : "";
      if (key === lastGeometry) return;
      lastGeometry = key;
      if (!geometry && !disposed) setActive(false);
      pending = true;
      const versionAtRequest = eventVersion;
      void invoke<boolean>("island_sync_native_glass", { geometry }).then(
        (active) => {
          if (shouldShow() && geometry && versionAtRequest === eventVersion) setActive(active);
        },
        () => {
          failed = true;
          if (!disposed) setActive(false);
        },
      ).finally(() => {
        pending = false;
        sync();
      });
    };
    const onVisibility = () => {
      if (document.hidden) setActive(false);
      sync();
    };
    syncRef.current = sync;
    document.addEventListener("visibilitychange", onVisibility);

    void listen<number>("zbar://island-native-glass-luma", ({ payload }) => {
      if (shouldShow() && Number.isFinite(payload)) luminance.current = payload;
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenLuma = unlisten;
    }).catch(() => {});

    // 先订阅再发首帧，避免漏掉原生交换链首次上屏的就绪事件。
    void listen<boolean>("zbar://island-native-glass", ({ payload }) => {
      eventVersion += 1;
      if (!disposed) setActive(payload && shouldShow() && lastGeometry !== "");
    }).then((unlisten) => {
      if (disposed) {
        unlisten();
        return;
      }
      unlistenReady = unlisten;
      listening = true;
      sync();
    }).catch(() => {});

    return () => {
      disposed = true;
      syncRef.current = () => {};
      document.removeEventListener("visibilitychange", onVisibility);
      unlistenReady?.();
      unlistenLuma?.();
      sync();
    };
  }, [available]);

  useLayoutEffect(() => {
    if (!enabled) setReady(false);
    syncRef.current();
  }, [enabled]);

  return { active: enabled && ready, updateGeometry, luminance };
}
