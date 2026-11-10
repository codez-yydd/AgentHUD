/** 灵动岛外壳：收起胶囊 / 展开面板 / 标签导航 / 右键菜单 / 深色玻璃与事件桥 */

import { useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import {
  clearClipboard,
  clearNotify,
  copyClipboard,
  beginIslandDrag,
  endIslandDrag,
  getClipboard,
  getIslandBackdrop,
  getIslandState,
  getLyrics,
  getMedia,
  getNotify,
  getQuotaSnapshot,
  getStocks,
  getSystem,
  getTodayUsage,
  getWeather,
  IS_TAURI,
  markNotifyRead,
  onEvent,
  openMainWindow,
  openNotifySettings,
  openPath,
  openSystemSettings,
  quitApp,
  refreshQuota,
  refreshStocks,
  refreshWeather,
  setBrightness,
  setIslandConfig,
  islandSetHoverRect,
  setIslandEnabled,
  setIslandExpanded,
  setKeyboard,
  setVolume,
  stepIslandDrag,
  toggleMute,
  toggleRadio,
} from "./api";
import type {
  ClipItem,
  CompactView,
  HudBarAgent,
  HudBarQuota,
  HudBarUsage,
  IslandConfig,
  IslandEdge,
  IslandState,
  LyricsState,
  MediaState,
  NotifyState,
  StockAlert,
  StocksState,
  SystemState,
  TabKey,
  TodayUsageTotals,
  WeatherState,
} from "./types";
import {
  agentLabel,
  BrandMark,
  currentLyricLine,
  fmtClock,
  fmtDuration,
  fmtMoney,
  Icon,
  type IconName,
  NowProvider,
  parseLyrics,
  remainingPct,
  Slider,
  useNow,
  WeatherGlyph,
} from "./ui";
import {
  ClipboardPage,
  ControlsPage,
  HomePage,
  MessagesPage,
  SettingsPage,
  StocksPage,
  WeatherPage,
} from "./pages";
import { ScreenshotPage } from "./ShotPage";

/** 桌面抓屏目前只有 Windows 实现；macOS 上不轮询，避免每 150ms 一次空报错。 */
const CAN_CAPTURE_BACKDROP =
  IS_TAURI && typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);

const TABS: { key: TabKey; label: string; icon: IconName }[] = [
  { key: "home", label: "主页", icon: "home" },
  { key: "msg", label: "消息", icon: "message" },
  { key: "stock", label: "股票", icon: "chart" },
  { key: "weather", label: "天气", icon: "weather" },
  { key: "ctl", label: "控制", icon: "sliders" },
  { key: "clip", label: "剪贴板", icon: "clipboard" },
  { key: "shot", label: "截图", icon: "camera" },
];

const EMPTY_MEDIA: MediaState = {
  hasSession: false,
  appId: "",
  appName: "",
  title: "",
  artist: "",
  album: "",
  playing: false,
  position: 0,
  duration: 0,
  canPrev: false,
  canNext: false,
  cover: "",
  source: "",
};

/** 股票代码比较：腾讯行情的键大小写不固定（usAAPL / usaapl），统一忽略大小写。 */
function sameStock(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

const EMPTY_SYSTEM: SystemState = {
  volume: 0,
  muted: false,
  brightness: null,
  brightnessSupported: false,
  wifi: null,
  wifiAvailable: false,
  bluetooth: null,
  bluetoothAvailable: false,
  error: "",
  updatedAt: 0,
};

function timeGreeting(now: number): string {
  const h = new Date(now).getHours();
  if (h < 6) return "夜深了";
  if (h < 11) return "早上好";
  if (h < 14) return "中午好";
  if (h < 18) return "下午好";
  return "晚上好";
}

export default function IslandApp() {
  const [config, setConfig] = useState<IslandConfig | null>(null);
  const [enabled, setEnabled] = useState(true);
  const [open, setOpen] = useState(
    () => !IS_TAURI && new URLSearchParams(window.location.search).get("open") === "1"
  );
  const [tab, setTab] = useState<TabKey>("home");
  const [usage, setUsage] = useState<HudBarUsage | null>(null);
  const [todayUsage, setTodayUsage] = useState<TodayUsageTotals | null>(null);
  const [weather, setWeather] = useState<WeatherState | null>(null);
  const [stocks, setStocks] = useState<StocksState | null>(null);
  const [media, setMedia] = useState<MediaState>(EMPTY_MEDIA);
  const [lyrics, setLyrics] = useState<LyricsState | null>(null);
  const [system, setSystem] = useState<SystemState>(EMPTY_SYSTEM);
  const [clipboard, setClipboard] = useState<ClipItem[]>([]);
  const [notify, setNotify] = useState<NotifyState>({ items: [], unread: 0 });
  const [refreshing, setRefreshing] = useState(false);
  const [toast, setToast] = useState("");
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null);
  const [edge, setEdge] = useState<IslandEdge>("top");
  const [dragging, setDragging] = useState(false);
  const [snapHiding, setSnapHiding] = useState(false);

  const rootRef = useRef<HTMLDivElement | null>(null);
  const islandRef = useRef<HTMLDivElement | null>(null);
  const collapseTimer = useRef<number | null>(null);
  const hoverTimer = useRef<number | null>(null);
  const shrinkTimer = useRef<number | null>(null);
  const toastTimer = useRef<number | null>(null);
  const configTimer = useRef<number | null>(null);
  const configRef = useRef<IslandConfig | null>(null);
  // 连续快速改配置（例如连点删除多只股票）时，串行落盘并忽略旧响应，
  // 否则后返回的旧快照会把刚删掉的股票又写回去。
  const configRevision = useRef(0);
  const configSaveQueue = useRef<Promise<void>>(Promise.resolve());
  const openRef = useRef(open);
  const bootRef = useRef(false);
  const backdropSeq = useRef(0);
  const edgeRef = useRef<IslandEdge>("top");
  const pointerRef = useRef<{
    id: number;
    x: number;
    y: number;
    started: boolean;
  } | null>(null);
  const dragActiveRef = useRef(false);
  const dragStartingRef = useRef(false);
  const dragEpochRef = useRef(0);
  const dragOriginEdgeRef = useRef<IslandEdge | null>(null);
  const suppressClickRef = useRef(false);
  const pressingRef = useRef(false);

  configRef.current = config;
  edgeRef.current = edge;

  const previewParams = useMemo(() => new URLSearchParams(window.location.search), []);
  const previewOpen = !IS_TAURI && previewParams.get("open") === "1";
  const previewTab = previewParams.get("tab") as TabKey | null;

  useEffect(() => {
    if (IS_TAURI) return;
    const wanted = previewParams.get("edge");
    if (wanted === "left" || wanted === "right" || wanted === "top") {
      setEdge(wanted);
    }
  }, [previewParams]);

  const showToast = (message: string) => {
    setToast(message);
    if (toastTimer.current != null) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(""), 2000);
  };

  // ===== 启动加载 =====
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const state: IslandState = await getIslandState();
        if (!alive) return;
        setConfig(state.config);
        configRef.current = state.config;
        const wantedEdge = previewParams.get("edge");
        setEdge(
          wantedEdge === "left" || wantedEdge === "right" || wantedEdge === "top"
            ? wantedEdge
            : state.config.edge ?? "top"
        );
        setEnabled(state.enabled);
        void getQuotaSnapshot().then((data) => alive && data && setUsage(data));
        void getTodayUsage().then((data) => alive && data && setTodayUsage(data));
        void getWeather().then((data) => alive && data && setWeather(data));
        void getStocks().then((data) => alive && data && setStocks(data));
        void getMedia().then((data) => alive && setMedia(data));
        void getSystem().then((data) => alive && setSystem(data));
        void getClipboard().then((data) => alive && setClipboard(data));
        void getNotify().then((data) => alive && setNotify(data));
      } catch (e) {
        if (alive) showToast(`灵动岛加载失败: ${String(e)}`);
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // ===== 桌面毛玻璃底板 =====
  // 展开约 650ms 一拍、收起约 1.8s 一拍；抓屏结果通过 base64 PNG 回到 CSS。
  // 串行轮询，绝不让两次抓屏叠在一起；切页/窗口显隐时自动重开。
  useEffect(() => {
    if (!CAN_CAPTURE_BACKDROP || !enabled) return;
    let alive = true;
    let timer: number | null = null;
    const tick = () => {
      if (!alive) return;
      if (document.visibilityState === "hidden") {
        timer = window.setTimeout(tick, 900);
        return;
      }
      const seq = ++backdropSeq.current;
      getIslandBackdrop()
        .then((url) => {
          if (!alive || seq !== backdropSeq.current || !url) return;
          // 直接写 CSS 变量，不为背景图重渲染整个面板（页面里有图表/
          // 输入框，每次抓屏都做 React 全树渲染会让展开动画掉帧）。
          islandRef.current?.style.setProperty("--backdrop", `url("${url}")`);
        })
        .catch(() => {})
        .finally(() => {
          if (!alive) return;
          // 深模糊下背景变化本来就慢；低频采样能显著降低抓屏排除窗口对
          // 用户截图/录屏的干扰，也省掉不必要的 PNG 编码。
          timer = window.setTimeout(tick, open ? 650 : 1800);
        });
    };
    // 展开/收起会改变窗口尺寸，等 WebView 视口稳定后再抓第一帧。
    timer = window.setTimeout(tick, open ? 120 : 0);
    return () => {
      alive = false;
      backdropSeq.current++;
      if (timer != null) window.clearTimeout(timer);
    };
  }, [enabled, open]);

  // ===== 外部事件 =====
  useEffect(() => {
    const unsubs = [
      onEvent<HudBarUsage>("zbar://hud-bar-usage", setUsage),
      onEvent<TodayUsageTotals>("zbar://island-today-usage", setTodayUsage),
      onEvent<WeatherState>("zbar://island-weather", setWeather),
      onEvent<StocksState>("zbar://island-stocks", setStocks),
      onEvent<MediaState>("zbar://island-media", setMedia),
      onEvent<SystemState>("zbar://island-system", setSystem),
      onEvent<ClipItem[]>("zbar://island-clipboard", setClipboard),
      onEvent<NotifyState>("zbar://island-notify", setNotify),
      onEvent<IslandConfig>("zbar://island-config", (next) => {
        if (next && next.enabled !== undefined) {
          setConfig(next);
          configRef.current = next;
          if (next.edge) setEdge(next.edge);
        }
      }),
      onEvent<{ enabled?: boolean }>("zbar://island-state", (payload) => {
        if (typeof payload?.enabled === "boolean") setEnabled(payload.enabled);
      }),
      onEvent<StockAlert>("zbar://island-stock-alert", (alert) => {
        if (alert?.name) showToast(`${alert.name} ${alert.text}`);
      }),
      onEvent("zbar://island-expand", () => {
        // macOS：WebView 收不到 mouseenter，由 Rust 轮询光标后在悬停时广播
        expand();
      }),
      onEvent("zbar://island-collapse", () => {
        setOpen(false);
        openRef.current = false;
        setContextMenu(null);
        void setKeyboard(false);
        // 窗口也要真的缩回悬浮条：截图恢复、全屏隐藏等路径只会发事件，
        // 不缩窗口的话会停在展开尺寸，且透明区域仍然算 :hover。
        if (shrinkTimer.current != null) window.clearTimeout(shrinkTimer.current);
        shrinkTimer.current = window.setTimeout(() => {
          void setIslandExpanded(false).catch(() => {});
        }, 30);
      }),
    ];
    return () => unsubs.forEach((fn) => fn());
  }, []);

  // 收起态把胶囊的实际矩形报给 Rust：macOS 上由 Rust 轮询光标判断悬停展开，
  // 用整扇窗（含透明边距）当热区会太灵敏，所以以真实胶囊为准。
  //
  // 关键：`width/height` 有 500ms 过渡，收起刚开始时量到的是**展开面板的尺寸**
  // （约 500×450），拿去当热区会让鼠标还没靠近胶囊就把面板重新撑开。所以这里
  // 用 rAF 连续取帧，等矩形连续几帧不再变化（动画真正结束）才上报。
  useEffect(() => {
    if (!IS_TAURI || open) return;
    let alive = true;
    let last = "";
    let stable = 0;
    const deadline = performance.now() + 1500;
    const push = (rect: DOMRect) => {
      void islandSetHoverRect(rect.left, rect.top, rect.width, rect.height);
    };
    const tick = () => {
      if (!alive) return;
      const el = islandRef.current;
      const rect = el?.getBoundingClientRect();
      if (rect && rect.width >= 1 && rect.height >= 1) {
        const key = `${rect.left.toFixed(1)},${rect.top.toFixed(1)},${rect.width.toFixed(1)},${rect.height.toFixed(1)}`;
        if (key === last) {
          stable += 1;
        } else {
          last = key;
          stable = 0;
        }
        // 连续 3 帧不变 ≈ 过渡已经落定
        if (stable >= 2) {
          push(rect);
          return;
        }
      }
      if (performance.now() > deadline) {
        if (rect && rect.width >= 1 && rect.height >= 1) push(rect);
        return;
      }
      window.requestAnimationFrame(tick);
    };
    const raf = window.requestAnimationFrame(tick);
    return () => {
      alive = false;
      window.cancelAnimationFrame(raf);
    };
  }, [open, edge]);

  // 失去系统焦点（点其它应用）后恢复「不可聚焦」，下次启动/点击不再抢焦点
  useEffect(() => {
    const onBlur = () => {
      if (!IS_TAURI) return;
      window.setTimeout(() => {
        void setKeyboard(false);
        if (
          configRef.current?.expandMode === "hover" &&
          !rootRef.current?.matches(":hover")
        ) {
          collapse();
        }
      }, 80);
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // ===== 歌词：换曲后按「曲名 + 歌手」取一次，失败静默 =====
  useEffect(() => {
    if (!media.hasSession || !media.title.trim()) {
      setLyrics(null);
      return;
    }
    setLyrics(null);
    let alive = true;
    const timer = window.setTimeout(() => {
      getLyrics(media.title, media.artist)
        .then((value) => {
          if (alive) setLyrics(value);
        })
        .catch(() => {
          if (alive) setLyrics(null);
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [media.hasSession, media.title, media.artist]);

  // ===== 首次展开策略（预览参数）=====
  useEffect(() => {
    if (!config || bootRef.current) return;
    bootRef.current = true;
    const initialTab =
      previewTab && (previewTab === "settings" || TABS.some((t) => t.key === previewTab))
        ? previewTab
        : "home";
    setTab(initialTab);
    if (!enabled) return;
    if (previewOpen) {
      setOpen(true);
      openRef.current = true;
      void setIslandExpanded(true);
    }
  }, [config, enabled, previewOpen, previewTab]);

  // ===== 展开 / 收起 =====
  // 参考 wisland：悬停先停留 220ms 再展开，离开 450ms 后收起；宽度和
  // 高度由 CSS 过渡完成，内容等玻璃长到接近满高才淡入，收起时先淡出内容。
  const cancelHover = () => {
    if (hoverTimer.current != null) {
      window.clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
  };

  const cancelCollapse = () => {
    if (collapseTimer.current != null) {
      window.clearTimeout(collapseTimer.current);
      collapseTimer.current = null;
    }
    if (shrinkTimer.current != null) {
      window.clearTimeout(shrinkTimer.current);
      shrinkTimer.current = null;
    }
  };

  const showExpanded = () => {
    setOpen(true);
  };

  const expand = (immediate = false) => {
    cancelHover();
    cancelCollapse();
    if (shrinkTimer.current != null) {
      window.clearTimeout(shrinkTimer.current);
      shrinkTimer.current = null;
    }
    if (openRef.current) return;
    openRef.current = true;
    setContextMenu(null);
    // 先让窗口长到 520 高，再等视口真的变成 520 后开始玻璃尺寸动画。
    // 只等 invoke resolve 不够：WebView2 的实际 resize 会晚几帧，若此时
    // CSS 已经在长高，OS 改窗口时会闪一帧空白（实机录屏能抓到）。
    if (IS_TAURI && !immediate) {
      void setIslandExpanded(true)
        .catch(() => {})
        .then(() => {
          const started = performance.now();
          const waitViewport = () => {
            if (!openRef.current) return;
            const settled =
              (window.innerHeight >= 518 && window.innerWidth >= 550) ||
              performance.now() - started > 360;
            if (!settled) {
              window.requestAnimationFrame(waitViewport);
              return;
            }
            window.requestAnimationFrame(() => {
              window.requestAnimationFrame(() => {
                if (openRef.current) showExpanded();
              });
            });
          };
          window.requestAnimationFrame(waitViewport);
        });
    } else {
      void setIslandExpanded(true).catch(() => {});
      showExpanded();
    }
  };

  const closeNow = () => {
    cancelHover();
    cancelCollapse();
    if (!openRef.current) return;
    openRef.current = false;
    setContextMenu(null);
    setOpen(false);
    void setKeyboard(false);
    shrinkTimer.current = window.setTimeout(() => {
      void setIslandExpanded(false).catch(() => {});
    }, 560);
  };

  const collapse = (delay = 450) => {
    cancelHover();
    if (collapseTimer.current != null) {
      window.clearTimeout(collapseTimer.current);
      collapseTimer.current = null;
    }
    collapseTimer.current = window.setTimeout(() => {
      collapseTimer.current = null;
      // 指针仍停在岛上（含右键菜单）或输入框有焦点时不收。
      if (rootRef.current?.matches(":hover")) return;
      const active = document.activeElement as HTMLElement | null;
      if (
        active &&
        rootRef.current?.contains(active) &&
        (active.tagName === "INPUT" ||
          active.tagName === "TEXTAREA" ||
          active.tagName === "SELECT")
      ) {
        return;
      }
      closeNow();
    }, delay);
  };

  const scheduleExpand = () => {
    if (!enabled || configRef.current?.expandMode !== "hover" || openRef.current) return;
    if (pressingRef.current) return;
    if (hoverTimer.current != null) return;
    hoverTimer.current = window.setTimeout(() => {
      hoverTimer.current = null;
      // 按住鼠标时绝不自动展开：用户可能正准备拖动。
      if (!pressingRef.current && rootRef.current?.matches(":hover")) expand();
    }, 220);
  };

  const toggleOpen = () => {
    if (openRef.current) closeNow();
    else expand();
  };

  // ===== 配置保存（稍作防抖，滑杆拖动不会打爆 IPC）=====
  const updateConfig = (patch: Partial<IslandConfig>, immediate = false) => {
    const current = configRef.current;
    if (!current) return;
    const next = { ...current, ...patch };
    setConfig(next);
    configRef.current = next;
    const revision = ++configRevision.current;
    if (configTimer.current != null) window.clearTimeout(configTimer.current);
    const commit = () => {
      const snapshot = configRef.current;
      if (!snapshot) return;
      configSaveQueue.current = configSaveQueue.current
        .catch(() => {
          /* 上一次失败不能卡住后续保存 */
        })
        .then(() => setIslandConfig(snapshot))
        .then((saved) => {
          // 期间又改过配置：这是旧快照的响应，不能回写覆盖新状态。
          if (revision !== configRevision.current) return;
          if (saved) {
            setConfig(saved);
            configRef.current = saved;
          }
        })
        .catch((e) => showToast(`保存配置失败: ${String(e)}`));
    };
    if (immediate) commit();
    else configTimer.current = window.setTimeout(commit, 220);
  };

  const cycleCompact = (direction: number) => {
    const order: CompactView[] = ["quota", "clock", "music"];
    const current = configRef.current?.compactView ?? "quota";
    const index = Math.max(0, order.indexOf(current));
    const next = order[(index + (direction > 0 ? 1 : order.length - 1)) % order.length];
    updateConfig({ compactView: next }, true);
  };

  const refreshAll = () => {
    setRefreshing(true);
    void refreshQuota();
    void refreshWeather();
    void refreshStocks();
    void getMedia().then(setMedia).catch(() => {});
    void getSystem().then(setSystem).catch(() => {});
    void getClipboard().then(setClipboard).catch(() => {});
    void getNotify().then(setNotify).catch(() => {});
    void getTodayUsage().then((data) => data && setTodayUsage(data)).catch(() => {});
    window.setTimeout(() => setRefreshing(false), 900);
  };

  // ===== 拖拽 / 吸附 =====
  // 位置由 Rust 读系统光标结算（pointer 离开窗口也不会断），前端只负责
  // 60fps 步进、速度形变，以及松手后的淡出/换向。这样不会出现：鼠标拖到
  // 屏幕边、WebView 收不到 pointermove、胶囊卡住不动。
  const stopDragVisual = () => {
    islandRef.current?.style.setProperty("--drag-vx", "0px");
    islandRef.current?.style.setProperty("--drag-vy", "0px");
    islandRef.current?.style.setProperty("--drag-stretch", "0");
    islandRef.current?.style.setProperty("--drag-sx", "1");
    islandRef.current?.style.setProperty("--drag-sy", "1");
  };

  const finishDrag = () => {
    if (!dragActiveRef.current && !dragStartingRef.current) return;
    pressingRef.current = false;
    dragEpochRef.current += 1;
    dragStartingRef.current = false;
    if (!dragActiveRef.current) {
      setDragging(false);
      stopDragVisual();
      suppressClickRef.current = false;
      return;
    }
    dragActiveRef.current = false;
    setSnapHiding(true);
    window.setTimeout(() => {
      void endIslandDrag()
        .then((result) => {
          dragOriginEdgeRef.current = null;
          setEdge(result.edge);
          const current = configRef.current;
          if (current) {
            const next = { ...current, edge: result.edge };
            configRef.current = next;
            setConfig(next);
          }
        })
        .catch((e) => showToast(`吸附失败: ${String(e)}`))
        .finally(() => {
          setDragging(false);
          setSnapHiding(false);
          stopDragVisual();
          suppressClickRef.current = true;
          window.setTimeout(() => {
            suppressClickRef.current = false;
          }, 240);
        });
    }, 130);
  };

  const runDragLoop = () => {
    const tick = () => {
      if (!dragActiveRef.current && !dragStartingRef.current) return;
      void stepIslandDrag()
        .then((step) => {
          if (!step.active) {
            if (dragActiveRef.current) {
              finishDrag();
            } else {
              // 只是按住没拖：让 click 正常发生，不做吸附
              dragStartingRef.current = false;
              pressingRef.current = false;
              pointerRef.current = null;
              setDragging(false);
              stopDragVisual();
            }
            return;
          }
          if (step.started && !dragActiveRef.current) {
            // Rust 判定越过阈值，此刻才进入拖动视觉
            dragActiveRef.current = true;
            dragStartingRef.current = false;
            // 拖拽期间窗口会被收成横条：面板必须同时切回收起态，
            // 否则 React 还在渲染 552 宽的面板，窗口却只有 70 宽（内容被裁花）。
            if (openRef.current) {
              openRef.current = false;
              setOpen(false);
              setContextMenu(null);
              void setKeyboard(false);
              cancelCollapse();
            }
            if (edgeRef.current !== "top") {
              dragOriginEdgeRef.current = edgeRef.current;
              setEdge("top");
            }
            setDragging(true);
            suppressClickRef.current = true;
          }
          if (!dragActiveRef.current) {
            window.requestAnimationFrame(tick);
            return;
          }
          const speed = Math.min(18, Math.hypot(step.dx, step.dy));
          const stretch = Math.min(1, speed / 11);
          const element = islandRef.current;
          if (element) {
            // 沿运动方向拉长、垂直方向收窄，少量“液体”形变；斜向拖动时两者接近 1。
            const ax = Math.abs(step.dx);
            const ay = Math.abs(step.dy);
            const total = ax + ay;
            const hx = total > 0.001 ? ax / total : 0.5;
            const hy = total > 0.001 ? ay / total : 0.5;
            const sx = 1 + stretch * (0.042 * hx - 0.021 * hy);
            const sy = 1 + stretch * (0.042 * hy - 0.021 * hx);
            element.style.setProperty("--drag-vx", `${Math.max(-18, Math.min(18, step.dx))}px`);
            element.style.setProperty("--drag-vy", `${Math.max(-18, Math.min(18, step.dy))}px`);
            element.style.setProperty("--drag-stretch", stretch.toFixed(3));
            element.style.setProperty("--drag-sx", sx.toFixed(4));
            element.style.setProperty("--drag-sy", sy.toFixed(4));
          }
          window.requestAnimationFrame(tick);
        })
        .catch(() => {
          finishDrag();
        });
    };
    window.requestAnimationFrame(tick);
  };

  const beginPointerDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    const target = event.target instanceof Element ? event.target : null;
    if (!target) return;
    // 表单控件和滑杆要保留原生拖拽/选择语义；其余区域（包括按钮、卡片、
    // 导航栏和页面空白）都允许按住拖动。没有超过阈值的按压仍会正常触发
    // 点击，超过阈值才收成横条并开始拖窗。
    if (
      target.closest(
        "input, textarea, select, [contenteditable='true'], .ctl-range"
      )
    ) {
      return;
    }
    // 收起态整颗胶囊可拖；展开态整个面板（导航、内容、空白）都可拖。
    const onPill = Boolean(target.closest(".pill"));
    const onPanel = Boolean(target.closest(".island-panel"));
    if (openRef.current ? !onPanel : !onPill) return;
    // 悬停展开的 220ms 计时器必须取消，否则按住拖动时面板会突然弹开。
    cancelHover();
    pressingRef.current = true;
    pointerRef.current = {
      id: event.pointerId,
      x: event.screenX,
      y: event.screenY,
      started: false,
    };
    suppressClickRef.current = false;
    // 按下就 arm：拖动是否真正开始由 Rust 依据「光标位移 ≥5px」判定。
    // 这样即使指针在很薄的胶囊上很快移出窗口、收不到 pointermove，
    // 拖拽依然能启动（macOS 上窗口不跟随鼠标捕获）。
    const epoch = ++dragEpochRef.current;
    dragStartingRef.current = true;
    void beginIslandDrag()
      .then((result) => {
        if (epoch !== dragEpochRef.current) return;
        if (!result.active) {
          dragStartingRef.current = false;
          pressingRef.current = false;
          return;
        }
        runDragLoop();
      })
      .catch((e) => {
        dragStartingRef.current = false;
        pressingRef.current = false;
        showToast(`拖拽失败: ${String(e)}`);
      });
  };

  const endPointerDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    const pointer = pointerRef.current;
    pointerRef.current = null;
    pressingRef.current = false;
    if (pointer?.started) {
      event.preventDefault();
      finishDrag();
    }
  };

  const cancelPointerDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    // 窗口被拖动时 WebView 可能发出 pointercancel；此时左键仍按着，
    // 交给 island_drag_step 的 GetAsyncKeyState 判定真正松手。
    if (dragActiveRef.current || dragStartingRef.current) return;
    endPointerDrag(event);
  };

  const handlePointerDownCapture = (event: ReactPointerEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement | null;
    const needsKeyboard = Boolean(
      target?.closest(
        "textarea, select, [contenteditable='true'], input:not([type='range']):not([type='checkbox']):not([type='radio']):not([type='button'])"
      )
    );
    void setKeyboard(needsKeyboard).then(() => {
      if (needsKeyboard) {
        window.setTimeout(() => target?.focus?.(), 30);
      }
    });
  };

  if (!config) {
    return <div className="island-boot" />;
  }

  // 深色玻璃只有一套配色；透明度控制的是玻璃色层，不是整块 UI 的 opacity。
  const transparency = Number.isFinite(config.transparency)
    ? Math.max(0, Math.min(90, config.transparency))
    : 40;
  const glassTint = Math.max(0.1, 0.9 - transparency * 0.0085);
  // 透明度和底图亮度一起走：越透越暗，避免高透明度时白底上白字发飘。
  const backdropBrightness = Math.max(0.32, 0.78 - transparency * 0.0045);
  // 与面板 / 主窗口同口径：13px 是原始设计基准，自定义值按比例缩放全部文字。
  const fontSize = Number.isFinite(config.fontSize)
    ? Math.max(10, Math.min(18, config.fontSize))
    : 13;
  const fontScale = fontSize / 13;
  const islandStyle = {
    "--glass-tint": glassTint.toFixed(3),
    "--backdrop-brightness": backdropBrightness.toFixed(3),
  } as CSSProperties;
  // 字号倍率挂在最外层，右键菜单 / toast 也一起缩放；面板和胶囊都继承。
  const rootFontStyle = { "--fs-scale": fontScale.toFixed(3) } as CSSProperties;
  const hidden = new Set(config.hiddenTabs);
  const tabs = TABS.filter((t) => t.key === "home" || !hidden.has(t.key));
  const activeTab =
    tab === "settings" || tabs.some((t) => t.key === tab) ? tab : "home";

  const renderPage = () => {
    switch (activeTab) {
      case "home":
        return (
          <HomePage
            usage={usage}
            weather={weather}
            media={media}
            lyrics={lyrics}
            muted={system.muted}
            config={config}
            refreshing={refreshing}
            onRefresh={refreshAll}
            onNavigate={setTab}
            onToggleMute={() => {
              void toggleMute().then(setSystem).catch((e) => showToast(String(e)));
            }}
            onOpenMain={() => void openMainWindow().catch((e) => showToast(String(e)))}
            onLaunch={(path) => void openPath(path).catch((e) => showToast(String(e)))}
          />
        );
      case "msg":
        return (
          <MessagesPage
            notify={notify}
            onMarkRead={() => {
              void markNotifyRead().then(setNotify).catch(() => {});
            }}
            onClear={(kind) => {
              void clearNotify(kind).then(setNotify).catch(() => {});
            }}
            onOpenSettings={() => {
              void openNotifySettings().catch(() => showToast("无法打开通知设置"));
            }}
          />
        );
      case "stock":
        return (
          <StocksPage
            stocks={stocks}
            config={config}
            onAdd={(code) => {
              // 用 ref 而不是本次渲染的 config：连续快速增删时不会拿旧列表覆盖。
              const current = configRef.current;
              if (!current) return;
              if (current.stocks.some((c) => sameStock(c, code))) {
                showToast("已在自选里");
                return;
              }
              updateConfig(
                { stocks: [...current.stocks.filter((c) => !sameStock(c, code)), code] },
                true
              );
              showToast("已添加自选");
              window.setTimeout(() => void refreshStocks(), 600);
            }}
            onRemove={(code) => {
              const current = configRef.current;
              if (!current) return;
              const next = current.stocks.filter((c) => !sameStock(c, code));
              if (next.length === current.stocks.length) {
                showToast("自选里没有这只");
                return;
              }
              updateConfig({ stocks: next }, true);
              showToast("已移除自选");
              window.setTimeout(() => void refreshStocks(), 350);
            }}
            onRefresh={() => void refreshStocks()}
          />
        );
      case "weather":
        return (
          <WeatherPage
            weather={weather}
            onRefresh={() => void refreshWeather()}
            onOpenSettings={() => setTab("settings")}
          />
        );
      case "ctl":
        return (
          <ControlsPage
            system={system}
            onVolume={(v) => void setVolume(v).then(setSystem).catch(() => {})}
            onMute={() => void toggleMute().then(setSystem).catch((e) => showToast(String(e)))}
            onBrightness={(v) => void setBrightness(v).then(setSystem).catch((e) => showToast(String(e)))}
            onRadio={(kind, on) =>
              void toggleRadio(kind, on)
                .then(setSystem)
                .catch((e) => showToast(String(e)))
            }
            onOpenSettings={(kind) => void openSystemSettings(kind).catch(() => {})}
            onOpenMain={() => void openMainWindow().catch((e) => showToast(String(e)))}
          />
        );
      case "clip":
        return (
          <ClipboardPage
            items={clipboard}
            onCopy={(id) => {
              void copyClipboard(id).catch((e) => showToast(String(e)));
            }}
            onClear={() => {
              void clearClipboard().then(() => setClipboard([])).catch(() => {});
            }}
            onToast={showToast}
          />
        );
      case "shot":
        return <ScreenshotPage onToast={showToast} />;
      case "settings":
        return (
          <SettingsPage
            config={config}
            onChange={(patch) => updateConfig(patch)}
            onOpenMain={() => void openMainWindow().catch((e) => showToast(String(e)))}
            onQuit={() => void quitApp()}
            onClose={() => {
              setOpen(false);
              openRef.current = false;
              void setIslandEnabled(false).catch((e) => showToast(String(e)));
            }}
            onToast={showToast}
          />
        );
      default:
        return null;
    }
  };

  const contextItems = (
    <div
      className="island-context-menu"
      style={{
        left: Math.min(contextMenu?.x ?? 120, 318),
        top: Math.min(contextMenu?.y ?? 60, 150),
      }}
      onPointerDown={(e) => e.stopPropagation()}
      onPointerEnter={() => cancelCollapse()}
      onClick={(e) => e.stopPropagation()}
    >
      <button type="button" onClick={() => { toggleOpen(); setContextMenu(null); }}>
        <Icon name={open ? "chevron" : "external"} size={13} />
        {open ? "收起灵动岛" : "展开灵动岛"}
      </button>
      <div className="context-divider" />
      <div className="context-glass">
        <div className="context-glass-head">
          <span>玻璃透明度</span>
          <span>{transparency}%</span>
        </div>
        <Slider
          value={transparency}
          min={0}
          max={90}
          step={1}
          onChange={(value) => updateConfig({ transparency: value })}
          ariaLabel="玻璃透明度"
        />
      </div>
      <div className="context-divider" />
      <button type="button" onClick={() => { refreshAll(); setContextMenu(null); }}>
        <Icon name="refresh" size={13} /> 刷新数据
      </button>
      <button type="button" onClick={() => { void openMainWindow(); setContextMenu(null); }}>
        <Icon name="external" size={13} /> 打开主窗口
      </button>
      <div className="context-divider" />
      <button
        type="button"
        onClick={() => {
          setContextMenu(null);
          void setIslandEnabled(false);
        }}
      >
        <Icon name="eyeOff" size={13} /> 关闭灵动岛
      </button>
      <button type="button" className="danger" onClick={() => void quitApp()}>
        <Icon name="power" size={13} /> 退出 AgentHUD
      </button>
    </div>
  );

  return (
    <NowProvider>
      <div
        ref={rootRef}
        className={`island-root edge-${edge} ${open ? "is-open" : "is-collapsed"} ${
          dragging ? "is-dragging" : ""
        } ${snapHiding ? "is-snap-hiding" : ""}`}
        style={rootFontStyle}
        onPointerDownCapture={handlePointerDownCapture}
        onPointerEnter={() => {
          if (dragActiveRef.current || dragStartingRef.current) return;
          if (openRef.current) cancelCollapse();
        }}
        onPointerLeave={() => {
          if (dragActiveRef.current || dragStartingRef.current) return;
          collapse();
        }}
        onContextMenu={(e) => {
          const target = e.target as HTMLElement | null;
          if (target?.closest("input, textarea, [contenteditable='true']")) {
            return;
          }
          if (dragActiveRef.current || dragStartingRef.current) return;
          e.preventDefault();
          if (!openRef.current) expand();
          setContextMenu({ x: e.clientX, y: e.clientY });
        }}
        onWheel={(e) => {
          if (openRef.current || dragActiveRef.current) return;
          e.preventDefault();
          cycleCompact(e.deltaY);
        }}
      >
        <div
          ref={islandRef}
          className={`island ${open ? "open" : "collapsed"}`}
          style={islandStyle}
          onPointerDown={beginPointerDrag}
          onPointerUp={endPointerDrag}
          onPointerCancel={cancelPointerDrag}
          onClickCapture={(e) => {
            if (!suppressClickRef.current) return;
            e.preventDefault();
            e.stopPropagation();
          }}
          onPointerEnter={() => {
            if (dragActiveRef.current || dragStartingRef.current) return;
            if (openRef.current) cancelCollapse();
            else scheduleExpand();
          }}
          onPointerLeave={() => {
            if (dragActiveRef.current || dragStartingRef.current) return;
            // 没进入拖动就离开窗口（例如按下后从边缘滑出），清掉按压态，
            // 否则后续悬停展开会被 pressingRef 永久挡住。
            pressingRef.current = false;
            pointerRef.current = null;
            cancelHover();
            collapse();
          }}
        >
          <div className="pill-wrap" aria-hidden={open}>
            <CollapsedPill
              config={config}
              usage={usage}
              todayUsage={todayUsage}
              weather={weather}
              media={media}
              lyrics={lyrics}
              system={system}
              edge={edge}
              onOpen={() => {
                if (suppressClickRef.current) return;
                toggleOpen();
              }}
            />
          </div>
          <div className="island-panel" aria-hidden={!open}>
            <nav className="island-nav" title="按住顶部空白处拖动 HUD">
              {tabs.map((item) => (
                <button
                  key={item.key}
                  type="button"
                  className={`nav-tab ${activeTab === item.key ? "active" : ""}`}
                  onClick={() => setTab(item.key)}
                  title={item.label}
                >
                  <Icon name={item.icon} size={15} />
                  <span>{item.label}</span>
                  {item.key === "msg" && notify.unread > 0 ? (
                    <span className="nav-badge">{notify.unread > 9 ? "9+" : notify.unread}</span>
                  ) : null}
                </button>
              ))}
              <span className="nav-spacer" />
              <button
                type="button"
                className={`nav-tab gear ${activeTab === "settings" ? "active" : ""}`}
                onClick={() => setTab("settings")}
                title="设置"
              >
                <Icon name="settings" size={15} />
              </button>
            </nav>
            <div className="island-content">{renderPage()}</div>
          </div>
        </div>
        {contextMenu ? (
          <>
            <div className="context-backdrop" onClick={() => setContextMenu(null)} />
            {contextItems}
          </>
        ) : null}
        {toast ? <div className="island-toast">{toast}</div> : null}
      </div>
    </NowProvider>
  );
}

function fmtTokens(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0";
  // 顶部横排同样要自动升级单位，否则 1.3B 会一直显示成 1341.4M
  if (value >= 1_000_000_000) {
    const billions = value / 1_000_000_000;
    // 和竖排 fmtTokensSide 保持同样的精度（1.33B，而不是 1.3B）
    return `${billions >= 100 ? Math.round(billions) : Number(billions.toFixed(2))}B`;
  }
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(value >= 10_000 ? 0 : 1)}K`;
  return String(Math.round(value));
}

/** 竖排花费：减少小数位，把字号让出来；百元/千元级直接取整。 */
function fmtCostSide(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0.00";
  if (value >= 100) return String(Math.round(value));
  if (value >= 10) return value.toFixed(1);
  return value.toFixed(2);
}
/** 竖排 48px 胶囊专用的更短 token：1091.1M → 1091M、1.234B → 1.23B。
 *  保留 M/B/K 单位语义，靠减少小数位给字号让空间。 */
function fmtTokensSide(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0";
  const trim = (n: number, digits: number) =>
    n.toFixed(digits).replace(/\.?0+$/, "");  if (value >= 1_000_000_000) return `${trim(value / 1_000_000_000, 2)}B`;
  if (value >= 1_000_000) {
    const millions = value / 1_000_000;
    return `${millions >= 100 ? Math.round(millions) : trim(millions, 1)}M`;
  }
  if (value >= 1_000) {
    const thousands = value / 1_000;
    return `${thousands >= 100 ? Math.round(thousands) : trim(thousands, 1)}K`;
  }
  return String(Math.round(value));
}

function CollapsedPill({
  config,
  usage,
  todayUsage,
  weather,
  media,
  lyrics,
  system,
  edge,
  onOpen,
}: {
  config: IslandConfig;
  usage: HudBarUsage | null;
  todayUsage: TodayUsageTotals | null;
  weather: WeatherState | null;
  media: MediaState;
  lyrics: LyricsState | null;
  system: SystemState;
  edge: IslandEdge;
  onOpen: () => void;
}) {
  const clockNow = useNow();
  const view = config.compactView;
  const lyricLine = useMemo(() => {
    if (!lyrics?.ok) return null;
    return currentLyricLine(parseLyrics(lyrics.lrc, lyrics.translation), media.position);
  }, [lyrics, media.position]);
  const agents = usage?.agents ?? [];
  const limit = edge === "top" ? (agents.length > 6 ? 5 : 6) : 7;
  const shown = agents.slice(0, limit);
  const extra = agents.length - shown.length;
  const todayLabel = todayUsage
    ? `${todayUsage.currency === "USD" ? "$" : "¥"}${
        edge === "top" ? todayUsage.cost.toFixed(2) : fmtCostSide(todayUsage.cost)
      }`
    : "--";
  const tokensLabel = todayUsage
    ? edge === "top"
      ? fmtTokens(todayUsage.tokens)
      : fmtTokensSide(todayUsage.tokens)
    : "--";

  if (view === "clock") {
    return (
      <button type="button" className={`pill pill-clock edge-${edge}`} onClick={onOpen}>
        <span className="pill-clock-time">{fmtClock(clockNow)}</span>
        <span className="pill-clock-date">{timeGreeting(clockNow)}</span>
        <span className="pill-usage-inline">
          <b>{todayUsage ? fmtTokens(todayUsage.tokens) : "--"}</b>
          <span>≈{todayLabel}</span>
        </span>
        <span className="pill-weather-mini">
          <WeatherGlyph icon={weather?.icon ?? "cloudy"} size={20} />
          {weather ? `${Math.round(weather.temp)}°` : "--°"}
        </span>
        <span className="pill-chevron">
          <Icon name="chevron" size={12} />
        </span>
      </button>
    );
  }

  if (view === "music" && media.hasSession) {
    return (
      <button type="button" className={`pill pill-music edge-${edge}`} onClick={onOpen}>
        <span className="pill-cover">
          {media.cover ? <img src={media.cover} alt="" /> : <Icon name="music" size={16} />}
        </span>
        <span className="pill-music-main">
          <span className="pill-music-title">{media.title || "未命名曲目"}</span>
          <span className="pill-music-sub">
            {lyricLine?.trans || lyricLine?.text || media.artist || media.appName}
          </span>
        </span>
        <span className={`pill-wave ${media.playing ? "playing" : ""}`} aria-hidden="true">
          <i />
          <i />
          <i />
          <i />
        </span>
        <span className="pill-music-time">
          {fmtDuration(media.position)} / {fmtDuration(media.duration)}
        </span>
        <span className="pill-chevron">
          <Icon name="chevron" size={12} />
        </span>
      </button>
    );
  }

  const showPlans = shown.some((agent) => Boolean(planTag(agent)));

  return (
    <button type="button" className={`pill pill-quota edge-${edge}`} onClick={onOpen}>
      <span className="pill-weather">
        <WeatherGlyph icon={weather?.icon ?? "cloudy"} size={edge === "top" ? 24 : 22} />
        <span className="pill-weather-temp">{weather ? `${Math.round(weather.temp)}°` : "--°"}</span>
        <span className="pill-weather-city">{weather?.city ?? config.weatherCity}</span>
      </span>
      <span className="pill-divider" />
      <span className={`pill-agents ${showPlans ? "has-plans" : ""}`}>
        {shown.length === 0 ? (
          <span className="pill-agent-empty">
            <span className="pill-pulse" /> 额度查询中
          </span>
        ) : (
          shown.map((agent) => <PillAgent key={agent.id} agent={agent} />)
        )}
        {extra > 0 ? <span className="pill-more">+{extra}</span> : null}
      </span>
      <span className="pill-usage">
        <span className="pill-usage-tokens">{tokensLabel}</span>
        <span className="pill-usage-cost">≈{todayLabel}</span>
      </span>
      <span className="pill-tail">
        {system.muted ? <Icon name="volumeOff" size={12} /> : null}
        <span className="pill-chevron">
          <Icon name="chevron" size={12} />
        </span>
      </span>
    </button>
  );
}

/** 收起态环：沿用旧版 HUD 的同心圆几何（viewBox 42）。 */
const RING_LAYOUTS: { radii: number[]; stroke: number; icon: number }[] = [
  { radii: [17.2], stroke: 3.7, icon: 15 },
  { radii: [17.2, 12.9], stroke: 2.7, icon: 11.5 },
  { radii: [17.2, 13.2, 9.3], stroke: 2.2, icon: 9.8 },
];
const RING_SIZE = 32;

/** 订阅型条目只把真实存在的 5h / 周 / 月窗口画成环；缺失窗口不补假弧。 */
function ringQuotas(agent: HudBarAgent): HudBarQuota[] {
  const quotas = agent.quotas;
  const canonical = quotas.some((q) =>
    ["hour5", "interval", "weekly", "monthly"].includes(q.key)
  );
  if (!canonical) return quotas.slice(0, RING_LAYOUTS.length);
  const out: HudBarQuota[] = [];
  const short = quotas.find((q) => q.key === "hour5") ?? quotas.find((q) => q.key === "interval");
  const weekly = quotas.find((q) => q.key === "weekly");
  const monthly = quotas.find((q) => q.key === "monthly");
  if (short) out.push(short);
  if (weekly) out.push(weekly);
  if (monthly) out.push(monthly);
  return out;
}

function ringTone(used: number): "ok" | "warn" | "low" {
  if (used >= 90) return "low";
  if (used >= 70) return "warn";
  return "ok";
}

/** 环下标：订阅档位。OpenCode Go 的档位就是产品名；DSH 没有订阅，
 *  只有开放平台余额；其它无 plan 的余额型条目标「余额」。 */
function planTag(agent: HudBarAgent): string {
  const plan = agent.plan?.trim();
  if (plan) return plan;
  if (agent.source === "opencodego") return "Go";
  if (agent.source === "dsh" || agent.balance) return "余额";
  return "";
}

/**
 * 旧版 HUD 的「球形」额度：每个 Agent 一枚环，环心是品牌图标；每个真实
 * 额度窗口一圈，从外到内依次 5h / 周 / 月。弧长按已用百分比，颜色按
 * 已用阈值分档（<70 绿 / 70–89 黄 / ≥90 红），条上不写数字，明细放
 * aria-label（读屏）与展开主页的额度卡里，不做鼠标悬浮文字。 */
function PillAgent({ agent }: { agent: HudBarAgent }) {
  const wins = ringQuotas(agent).filter((q) => Number.isFinite(q.usedPct));
  const layout = RING_LAYOUTS[Math.max(0, wins.length - 1)];
  const balance = agent.balance;
  const plan = planTag(agent);
  const offline = !wins.length && !balance && agent.status === "unavailable";
  const ariaBits: string[] = [agentLabel(agent.source)];
  if (agent.account) ariaBits.push(`· ${agent.account}`);
  if (agent.plan) ariaBits.push(`(${agent.plan})`);
  if (wins.length) {
    ariaBits.push(
      wins
        .map(
          (q) =>
            `${q.labelZh} 已用 ${Math.round(q.usedPct)}%（剩余 ${Math.round(
              remainingPct(q)
            )}%）`
        )
        .join(" · ")
    );
  } else if (balance) {
    ariaBits.push(`余额 ${fmtMoney(balance.amount, balance.currency)}`);
  } else {
    ariaBits.push(offline ? "额度暂不可用" : "暂无额度");
  }
  const ariaLabel = ariaBits.join(" ");

  return (
    <span
      className={`pill-ring ${offline ? "is-offline" : ""} ${
        balance && !wins.length ? "is-balance" : ""
      }`}
      aria-label={ariaLabel}
      style={{ width: RING_SIZE, height: RING_SIZE }}
    >
      <svg viewBox="0 0 42 42" aria-hidden="true">
        {layout.radii.map((radius, index) => {
          const quota = wins[index];
          if (!quota) {
            return (
              <circle
                key={`track-${index}`}
                cx="21"
                cy="21"
                r={radius}
                fill="none"
                stroke="var(--ring-track)"
                strokeWidth={layout.stroke}
              />
            );
          }
          const used = Math.max(0, Math.min(100, quota.usedPct));
          return (
            <g key={quota.key}>
              <circle
                cx="21"
                cy="21"
                r={radius}
                fill="none"
                stroke="var(--ring-track)"
                strokeWidth={layout.stroke}
              />
              {used > 0 ? (
                <circle
                  className="pill-ring-arc"
                  cx="21"
                  cy="21"
                  r={radius}
                  fill="none"
                  stroke={`var(--${ringTone(used)})`}
                  strokeWidth={layout.stroke}
                  strokeLinecap="round"
                  pathLength={100}
                  strokeDasharray={`${used} 100`}
                  transform="rotate(-90 21 21)"
                />
              ) : null}
            </g>
          );
        })}
        {!wins.length ? (
          <circle
            cx="21"
            cy="21"
            r={layout.radii[0]}
            fill="none"
            stroke="var(--accent)"
            strokeOpacity={balance ? 0.62 : 0.26}
            strokeWidth={balance ? 2.4 : 2}
            strokeLinecap="round"
            strokeDasharray={balance ? "1.5 5.2" : undefined}
            transform="rotate(-90 21 21)"
          />
        ) : null}
      </svg>
      <span className="pill-ring-icon">
        <BrandMark source={agent.source} size={layout.icon} />
      </span>
      {plan ? (
        <span className="pill-ring-plan" aria-hidden="true">
          {plan}
        </span>
      ) : null}
    </span>
  );
}

export { IslandApp };
