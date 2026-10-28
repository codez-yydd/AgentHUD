import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { UPDATE_READY_KEY, UPDATE_READY_EVENT } from "./updater";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  CredentialKind,
  Currency,
  PricingConfig,
  RangePreset,
  StatsTab,
} from "./types";
import { getHudBarState, onHudBarState, setHudBarEnabled } from "./hudBar";
import { fetchPin, openMainWindow, setPin, selectableRemoteDevices } from "./api";
import { useDataCache } from "./DataCache";
import { QuotaPanel } from "./QuotaPanel";
import { RangePicker } from "./RangePicker";
import { ZaiStatsContent } from "./ZaiStatsContent";
import { ModelSpeedPanel } from "./ModelSpeedPanel";
import { CodexPanel } from "./CodexPanel";
import { ClaudePanel } from "./ClaudePanel";
import { CursorPanel } from "./CursorPanel";
import { KimiPanel } from "./KimiPanel";
import { DshPanel } from "./DshPanel";
import { OpencodePanel } from "./OpencodePanel";
import { MinimaxPanel } from "./MinimaxPanel";
import { SummaryTab } from "./SummaryTab";
import { ProjectsPanel } from "./ProjectsPanel";
import { GenericQuotaPanel } from "./GenericQuotaPanel";
import { AddServiceMenu } from "./AddServiceMenu";
import { DropdownMenu } from "./DropdownMenu";
import {
  CredentialFormDialog,
  type CredentialOAuthFlow,
  type CredentialRegionOption,
} from "./CredentialsCard";
import {
  addProviderCredential,
  pollKimiDeviceAuth,
  startKimiDeviceAuth,
} from "./api";
import { BrandIcon, type BrandIconName } from "./BrandIcon";
import {
  AGENT_VISIBILITY_OPTIONS,
  CREDENTIAL_AGENT_KIND,
  enableAgentByCredential,
  isCredentialAgent,
  isPurePreferenceAgent,
  notifyCredentialsChanged,
  type AgentVisibility,
  type CredentialAgentId,
} from "./agentVisibility";
import { LanguageToggle, ThemeToggle } from "./layout";
import { useI18n, type TFn } from "./i18n";
import { dateLocale } from "./i18n/locale";

/** GitHub 仓库地址：托盘工具栏 GitHub 按钮用系统浏览器打开（openUrl
 *  失败时退化为 window.open，参照 UpdaterCard 的 DOWNLOAD_PAGE） */
const GITHUB_REPO = "https://github.com/codez-yydd/AgentHUD";

/** 区分国内/国际站的 provider：录入凭证时提供 region 下拉（与后端
 *  host_for_region 的站点路由一致，Cookie/Key 归属站点必须匹配）。
 *  kimi 同为双站（区域决定 usages 与 OAuth 域名组）。 */
const REGION_PROVIDERS: readonly string[] = [
  "kimi",
  "qoder",
  "alibaba",
  "alibabatoken",
  "minimax",
  "moonshot",
];

/** provider → region 下拉选项（无区域概念的服务返回 undefined，弹层不显示）。
 *  tab 面板与「＋添加服务」表单共用同一份配置。 */
function regionOptionsFor(
  provider: string,
  t: TFn
): ReadonlyArray<CredentialRegionOption> | undefined {
  if (!REGION_PROVIDERS.includes(provider)) return undefined;
  return [
    { value: "cn", label: t("credentials.regionCn") },
    { value: "global", label: t("credentials.regionGlobal") },
  ];
}

interface Props {
  currency: Currency;
  pricing: PricingConfig;
  agentVisibility: AgentVisibility;
  /** 凭证驱动 provider 的「已有凭证」状态（provider → bool；App 启动/事件时批量刷新） */
  credentialPresence: Record<string, boolean>;
  onGoSync: () => void;
  onGoReports: () => void;
  onGoTheme: () => void;
  onGoSettings: () => void;
  /** 设置页「添加凭证」快捷入口跳转：初始定位到该 provider 并直接打开
   *  添加表单（App 暂存 pending provider 传入，挂载即消费）。 */
  initialAdd?: string | null;
  /** initialAdd 已消费的回调（App 清空 pending，避免下次挂载重复打开） */
  onInitialAddConsumed?: () => void;
}

function loadStatsTab(agentVisibility: AgentVisibility): StatsTab {
  try {
    const saved = localStorage.getItem("zbar-tab");
    if (saved === "summary") return saved;
    if (saved === "projects") return saved;
    // 模型速度面板：本地 tab（无 Agent 偏好控制），直接恢复
    if (saved === "speed") return saved;
    // Agent tab：仅接受**确实存在 tab 按钮**的 id——AGENT_VISIBILITY_OPTIONS 是
    // tab 栏的唯一构造来源（见下方 statTabs）。已下架但为兼容历史数据仍留在
    // AgentId 联合类型 / localStorage 里的键（moonshot、deepseek）不再把面板
    // 恢复到「有 tab 值、无 tab 按钮」的悬挂态（那种情况下内容区只会回落汇总页，
    // 而 tab 栏没有任何高亮项）。按展示偏好恢复，未启用则回汇总。
    if (saved && AGENT_VISIBILITY_OPTIONS.some((o) => o.id === saved)) {
      return agentVisibility[saved as keyof AgentVisibility]
        ? (saved as StatsTab)
        : "summary";
    }
  } catch {
    // 存储不可用时使用默认标签，不阻断统计面板启动。
  }
  return "summary";
}

/**
 * 统计面板 —— 纯展示层。
 *
 * 所有数据（z.ai / Cursor / quota）由全局 DataProvider 统一预加载 + 定时刷新，
 * 此组件仅负责从缓存读取并渲染，不再自己发请求或维护定时器。
 * 数据常驻 Provider，切到其他页面再切回来瞬时恢复，无需重新加载。
 */
export function StatsPanel({
  currency,
  pricing,
  agentVisibility,
  credentialPresence,
  onGoSync,
  onGoReports,
  onGoTheme,
  onGoSettings,
  initialAdd,
  onInitialAddConsumed,
}: Props) {
  const cache = useDataCache();
  const { locale, t } = useI18n();
  const {
    preset,
    custom,
    trendBucket,
    deviceFilter,
    setPreset,
    setCustom,
    setDeviceFilter,
    stats,
    cost,
    trend,
    error,
    codex,
    codexError,
    claude,
    claudeError,
    cursor,
    cursorError,
    kimi,
    kimiError,
    dsh,
    dshError,
    opencode,
    opencodeError,
    minimaxcode,
    minimaxcodeError,
    fxRate,
    syncConfig,
    remoteDevices,
    syncEnabled,
    agentQuotaDeltas,
    lastUpdate,
    refreshing,
    refresh,
  } = cache;

  // 标签：汇总 | Z.ai | Codex | Claude | Cursor（关闭的 Agent 不显示）
  // 设置页快捷入口跳转时初始 tab 直接定位到目标 provider（避免先落汇总再跳）
  const [tab, setTab] = useState<StatsTab>(() =>
    initialAdd && isCredentialAgent(initialAdd)
      ? initialAdd
      : loadStatsTab(agentVisibility)
  );

  // ===== 「＋添加服务」入口 =====
  // addMenuOpen：服务选择浮层；addProvider：添加目标（非空时弹出该服务的
  // 添加表单）。设置页快捷入口跳转时直接进入添加态（同一套提交链路）。
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const [addProvider, setAddProvider] = useState<CredentialAgentId | null>(() =>
    initialAdd && isCredentialAgent(initialAdd) ? initialAdd : null
  );
  const [addBusy, setAddBusy] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);
  // 「＋」按钮 + 浮层的锚容器（点击外部关闭时把按钮一并豁免，
  // 保证再次点击「＋」可正常切换浮层开关）
  const addAnchorRef = useRef<HTMLDivElement | null>(null);
  // 设置页快捷入口跳转的锚定目标（仅挂载时取值）：与 addProvider 的临时
  // 保留分开——用户主动进入该服务面板，取消自动弹出的表单后仍应停留在
  // 面板内浏览凭证卡，而不是被门槛 effect 弹回汇总页
  const pinnedProviderRef = useRef<CredentialAgentId | null>(
    initialAdd && isCredentialAgent(initialAdd) ? initialAdd : null
  );

  // initialAdd 只在挂载时消费一次：App 收到回调即清空 pending
  useEffect(() => {
    if (initialAdd && isCredentialAgent(initialAdd)) {
      onInitialAddConsumed?.();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 浮层交互：点击外部 / Esc 关闭（浮层挂载时才注册）
  useEffect(() => {
    if (!addMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (
        addAnchorRef.current &&
        e.target instanceof Node &&
        !addAnchorRef.current.contains(e.target)
      ) {
        setAddMenuOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setAddMenuOpen(false);
    };
    document.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey);
    };
  }, [addMenuOpen]);

  // 添加表单提交（复用 CredentialsCard 添加路径的联动语义）：保存成功 →
  // 开启 tab 偏好（有凭证自动显示，含 manually-disabled 标记检查）→
  // 广播 presence 重探 → 切到该服务 tab → 关闭表单
  const handleAddSubmit = async (form: {
    label: string;
    secret: string;
    region: string | null;
    kind?: CredentialKind;
  }) => {
    if (!addProvider) return;
    setAddBusy(true);
    setAddError(null);
    const provider = addProvider;
    try {
      // 无区域下拉的服务 region 初始为空串，原样提交会被后端判为非法
      // 区域；空串/纯空白统一提交 null（未选择），对齐 CredentialsCard。
      // kind：kimi 表单内可选（OAuth 令牌/API Key），其余服务用默认映射
      const trimmedRegion = form.region?.trim() ?? "";
      await addProviderCredential(
        provider,
        form.label,
        form.kind ?? CREDENTIAL_AGENT_KIND[provider],
        form.secret,
        trimmedRegion.length > 0 ? trimmedRegion : null
      );
      enableAgentByCredential(provider);
      notifyCredentialsChanged(provider);
      setAddProvider(null);
      setAddError(null);
      setTab(provider);
    } catch (e) {
      setAddError(t("credentials.saveFail", { msg: String(e) }));
    } finally {
      setAddBusy(false);
    }
  };

  // OAuth 网页登录流程（目前仅 kimi 提供）：与手动添加成功后的联动语义
  // 一致——开启 tab 偏好 + 广播 presence/额度刷新 + 切到该服务 tab。
  // useMemo 稳定引用：api 函数为模块级导入（引用恒定），仅 addProvider
  // 变化时重建——弹层每秒重渲染（时钟等）不得让轮询定时器被反复清除重建
  const oauthFlow: CredentialOAuthFlow | undefined = useMemo(
    () =>
      addProvider === "kimi"
        ? {
            onStart: (region) => startKimiDeviceAuth(region),
            onPoll: (sessionId) => pollKimiDeviceAuth(sessionId),
          }
        : undefined,
    [addProvider]
  );
  const handleOAuthSuccess = (provider: CredentialAgentId) => {
    enableAgentByCredential(provider);
    notifyCredentialsChanged(provider);
    setAddProvider(null);
    setAddError(null);
    setTab(provider);
  };

  useEffect(() => {
    if (
      tab !== "summary" &&
      tab !== "projects" &&
      tab !== "speed" &&
      !agentVisibility[tab] &&
      // 凭证驱动的新 provider：有凭证时即使偏好未开也保留（「有凭证自动显示」）；
      // kimi 属首批 5 个（纯偏好控制，见 PURE_PREFERENCE_AGENTS），不参与该保留
      !(isCredentialAgent(tab) && !isPurePreferenceAgent(tab) && credentialPresence[tab]) &&
      // 「＋添加服务」流程中临时保留目标 tab（添加成功前 visibility/presence
      // 均未就绪；取消添加后无此保留，tab 回落汇总——本来就没有该 tab）；
      // 设置页跳转的锚定目标同样保留（用户主动进入该面板）
      tab !== addProvider && tab !== pinnedProviderRef.current
    ) {
      setTab("summary");
    }
  }, [agentVisibility, credentialPresence, tab, addProvider]);

  // 模式 C：「汇总」/「项目」标签走词典（其余是品牌名），语言切换时随 t 重建；
  // 「速度」同为词典标签（模型速度统计面板，本地库数据与 Agent 无关）
  const statTabs = useMemo<
    ReadonlyArray<{ id: StatsTab; label: string; brand?: BrandIconName }>
  >(
    () => [
      { id: "summary", label: t("stats.tab.summary") },
      { id: "projects", label: t("projects.tab") },
      { id: "speed", label: t("stats.tab.speed") },
      ...AGENT_VISIBILITY_OPTIONS.map((agent) => ({
        id: agent.id,
        label: agent.label,
        brand: agent.id as BrandIconName,
      })),
    ],
    [t]
  );

  const visibleTabs = statTabs.filter((item) =>
    item.id === "summary" || item.id === "projects" || item.id === "speed"
      ? true
      : // 凭证驱动的新 provider：「已启用或有凭证」才显示 tab（默认隐藏，
        // 添加凭证 / 手动开启后出现）；kimi 属首批 5 个，tab 仍纯偏好控制
        //（presence 只驱动「其他账号」区的补刷/轮询，见 PURE_PREFERENCE_AGENTS）
        isCredentialAgent(item.id) && !isPurePreferenceAgent(item.id)
        ? agentVisibility[item.id] || credentialPresence[item.id]
        : agentVisibility[item.id]
  );

  // tab 栏溢出检测：横向可滚动时右缘渐隐（暗示还有更多 tab，可拖动查看）
  const tabScrollRef = useRef<HTMLDivElement | null>(null);
  const [tabOverflow, setTabOverflow] = useState(false);
  useEffect(() => {
    const el = tabScrollRef.current;
    if (!el) return;
    const check = () => setTabOverflow(el.scrollWidth > el.clientWidth + 1);
    check();
    // 容器尺寸变化（窗口缩放 / 字号档位）与 tab 增减都会改变溢出状态
    const observer = new ResizeObserver(check);
    observer.observe(el);
    return () => observer.disconnect();
  }, [visibleTabs.length]);

  // 鼠标滚轮左右滑动 tab 栏（Agent 变多后 tab 经常溢出，滚轮是最顺手的通道）：
  // - 只有确实溢出时才接管，否则完全不动默认行为（不溢出时滚轮该滚页面就滚页面）；
  // - 触控板横向手势（deltaX 为主）直接用；普通鼠标纵向滚轮（deltaY）换算成横向；
  // - 已经滚到两端时把事件还给页面，避免「划到尽头就卡住、页面也滚不动」；
  // - 必须用原生监听：React 的事件委托给 root 挂的是 passive wheel，拦不住
  //   纵向滚动，会出现「tab 在滚、页面同时也在滚」的双滚。
  useEffect(() => {
    const el = tabScrollRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      const max = el.scrollWidth - el.clientWidth;
      if (max <= 1) return;
      // deltaMode：0=像素（Chromium/WebView2 默认）、1=行、2=页
      const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? el.clientWidth : 1;
      const raw = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
      const delta = raw * unit;
      if (!delta) return;
      const next = Math.min(max, Math.max(0, el.scrollLeft + delta));
      if (next === el.scrollLeft) return;
      el.scrollLeft = next;
      e.preventDefault();
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // ===== 窗口置顶常驻（仅 Windows）=====
  const isWindows =
    typeof navigator !== "undefined" &&
    /windows/i.test(navigator.userAgent);
  const [pinned, setPinned] = useState(false);

  // ===== HUD 停靠条开关（顶部工具栏图标按钮）=====
  // 状态真相在 Rust（hud-bar.json）：启动拉一次，之后托盘菜单/本按钮/异常
  // 销毁都通过 zbar://hud-bar-state 广播同步，面板不维护第二份开关。
  const [hudEnabled, setHudEnabled] = useState(false);
  const [hudBusy, setHudBusy] = useState(false);
  useEffect(() => {
    let alive = true;
    getHudBarState()
      .then((state) => {
        if (alive) setHudEnabled(state.enabled);
      })
      .catch(() => {
        /* 读取失败按关闭态显示；点击时以命令回执为准 */
      });
    const unlisten = onHudBarState((enabled) => {
      if (alive) setHudEnabled(enabled);
    });
    return () => {
      alive = false;
      void unlisten.then((fn) => fn()).catch(() => {
        /* 窗口卸载/ACL 拒绝：无副作用 */
      });
    };
  }, []);

  const toggleHudBar = useCallback(() => {
    if (hudBusy) return;
    const next = !hudEnabled;
    // 乐观更新：建窗/隐藏需要主线程往返，先让按钮立刻响应；失败回滚
    setHudEnabled(next);
    setHudBusy(true);
    setHudBarEnabled(next)
      .then((actual) => setHudEnabled(actual))
      .catch(() => setHudEnabled(!next))
      .finally(() => setHudBusy(false));
  }, [hudBusy, hudEnabled]);

  // 标题栏「主窗口」图标按钮打开失败的轻提示（建窗失败此前是纯静默：点了
  // 没反应，用户只会以为按钮坏了）。提示胶囊展示在底部信息行（工具条内没有
  // 放文字的位置），2.4s 后自动消失，不新增常驻 UI。
  const [mainOpenFailed, setMainOpenFailed] = useState(false);
  const mainOpenTimerRef = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (mainOpenTimerRef.current != null) {
        window.clearTimeout(mainOpenTimerRef.current);
      }
    },
    []
  );

  // 打开主窗口的统一入口（标题栏图标按钮 + 底部「价格设置」按钮共用）：
  // 可选 page 为 Dashboard 侧栏页 id——合法时 Rust 侧开窗/聚焦后向主窗口
  // 发 zbar://dashboard-navigate 跳页（价格入口传 "pricing" 直达定价页），
  // 缺省只开窗。失败提示复用底部信息行的轻胶囊（2.4s 自动消失）
  const openMainWindowWithToast = useCallback((page?: string) => {
    openMainWindow(page)
      .then(() => {
        // 成功：清掉上一次的失败提示
        if (mainOpenTimerRef.current != null) {
          window.clearTimeout(mainOpenTimerRef.current);
          mainOpenTimerRef.current = null;
        }
        setMainOpenFailed(false);
      })
      .catch(() => {
        setMainOpenFailed(true);
        if (mainOpenTimerRef.current != null) {
          window.clearTimeout(mainOpenTimerRef.current);
        }
        mainOpenTimerRef.current = window.setTimeout(() => {
          mainOpenTimerRef.current = null;
          setMainOpenFailed(false);
        }, 2400);
      });
  }, []);

  useEffect(() => {
    if (!isWindows) return;
    fetchPin()
      .then(setPinned)
      .catch(() => {});
  }, [isWindows]);

  // ===== 更新红点：后台下载完成后，设置入口按钮提示 =====
  const [updateReady, setUpdateReady] = useState(() => {
    try {
      return Boolean(localStorage.getItem(UPDATE_READY_KEY));
    } catch {
      return false;
    }
  });
  useEffect(() => {
    const sync = () => {
      try {
        setUpdateReady(Boolean(localStorage.getItem(UPDATE_READY_KEY)));
      } catch {
        /* 忽略存储异常 */
      }
    };
    window.addEventListener(UPDATE_READY_EVENT, sync);
    return () => window.removeEventListener(UPDATE_READY_EVENT, sync);
  }, []);

  // 记忆当前标签（localStorage 异常静默，对齐 cache.ts：记忆仅锦上添花，不影响主流程）
  useEffect(() => {
    try {
      localStorage.setItem("zbar-tab", tab);
    } catch {
      /* 忽略：QuotaExceededError、隐私模式等 */
    }
  }, [tab]);

  // 速度面板重查令牌：全局 ↻ 刷新时递增，驱动 ModelSpeedPanel 重新查询。
  // 速度面板是本地库即席查询，不在 DataCache 缓存体系内，cache.refresh()
  // 触及不到，需独立令牌联动（面板 useEffect 消费该 prop）
  const [speedRefresh, setSpeedRefresh] = useState(0);

  // 简洁用量卡的时间范围文案：与顶部 RangePicker 同一 preset / custom 状态
  const summaryRangeLabel =
    preset === "custom"
      ? `${custom.from} → ${custom.to}`
      : t(
          preset === "today"
            ? "range.today"
            : preset === "1d"
              ? "range.24h"
              : preset === "7d"
                ? "range.7d"
                : "range.30d"
        );

  // 完整导出图片头部的设备筛选文案（与顶部设备下拉同一状态）
  const summaryDeviceLabel =
    deviceFilter === "all"
      ? t("stats.deviceAll")
      : deviceFilter === "local"
        ? syncConfig?.device_name
          ? t("stats.deviceLocalName", { name: syncConfig.device_name })
          : t("stats.deviceLocal")
        : (() => {
            const device = selectableRemoteDevices(
              remoteDevices,
              syncConfig?.device_id
            ).find((d) => d.device_id === deviceFilter);
            return device
              ? t("common.deviceOption", {
                  name: device.device_name,
                  id: device.device_id.slice(0, 6),
                })
              : t("stats.deviceAll");
          })();

  return (
    // relative：承载「＋添加服务」的添加凭证全卡弹层（CredentialFormDialog
    // 的 absolute inset-0 以最近定位祖先为界）
    <div className="relative flex flex-col h-full">
      {/* 顶部 */}
      <div className="px-3 pt-2.5 pb-2 border-b border-slate-900/8">
        {/* Windows 无边框窗口拖动 */}
        <div
          className={`panel-topbar flex items-center justify-between mb-2 ${
            isWindows ? "cursor-default" : ""
          }`}
          onMouseDown={
            isWindows
              ? (e) => {
                  if (!(e.target as HTMLElement).closest("button")) {
                    getCurrentWindow().startDragging();
                  }
                }
              : undefined
          }
        >
          <div className="flex items-center gap-1.5 select-none min-w-0">
            <h1 className="text-[13px] font-bold text-slate-900/90 tracking-tight truncate">
              AgentHUD
            </h1>
          </div>
          {/* shrink-0：窗口压到 300px 时标题先让位（truncate），按钮组不压缩。
              含主窗口入口/HUD 开关共 11 个 button（Windows 下；非 Windows 无
              置顶按钮为 10 个）：11×22px + 10×2px 间距 = 262px ≤ 300 −
              24(px-3) = 276px 可用宽度，按钮不换行也不裁切；标题压到约
              14px 由 truncate 收尾 */}
          <div className="panel-toolbar flex items-center gap-0.5 shrink-0">
            <button
              onClick={onGoReports}
              className="toolbar-btn"
              title={t("stats.reports")}
            >
              📊
            </button>
            <button
              onClick={onGoSync}
              className={`toolbar-btn ${
                syncEnabled ? "text-emerald-600!" : ""
              }`}
              title={syncEnabled ? t("stats.syncOn") : t("stats.syncOff")}
            >
              ⇅
            </button>
            {/* 主窗口（Dashboard 大窗）入口：显示器图标，与相邻工具按钮同款
                尺寸/悬停样式。放大窗分析（与托盘标题同口径）是「去别的窗口」
                的导航，与 ⇅（同步）这类入口同组；不带 page（停留在上次页面） */}
            <button
              onClick={() => openMainWindowWithToast()}
              className="toolbar-btn"
              title={t("dashboard.openMainWindow")}
              aria-label={t("dashboard.openMainWindow")}
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                className="h-3.5 w-3.5"
                aria-hidden
              >
                <rect x="2" y="3" width="20" height="14" rx="2" />
                <path d="M8 21h8" />
                <path d="M12 17v4" />
              </svg>
            </button>
            {/* HUD 停靠条开关：与主窗口入口同组（都是"另一个常驻视图"）。
                开启时高亮为翠绿，悬停 title 说明当前状态与点击动作；托盘
                右键菜单仍保留同一开关，两处状态经 zbar://hud-bar-state 同步 */}
            <button
              onClick={toggleHudBar}
              disabled={hudBusy}
              className={`toolbar-btn ${
                hudEnabled ? "text-emerald-600!" : ""
              } ${hudBusy ? "opacity-40" : ""}`}
              title={
                hudEnabled ? t("stats.hudBarOn") : t("stats.hudBarOff")
              }
              aria-label={t("stats.hudBar")}
              aria-pressed={hudEnabled}
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                className="h-3.5 w-3.5"
                aria-hidden
              >
                <rect x="5" y="3" width="14" height="18" rx="3.5" />
                <path d="M9 8h.01M9 12h.01M9 16h.01" />
              </svg>
            </button>
            <ThemeToggle />
            <LanguageToggle />
            <button
              onClick={onGoTheme}
              className="toolbar-btn"
              title={t("theme.toolbarEntry")}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="h-3.5 w-3.5" aria-hidden>
                <circle cx="13.5" cy="6.5" r=".5" fill="currentColor" />
                <circle cx="17.5" cy="10.5" r=".5" fill="currentColor" />
                <circle cx="8.5" cy="7.5" r=".5" fill="currentColor" />
                <circle cx="6.5" cy="12.5" r=".5" fill="currentColor" />
                <path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2z" />
              </svg>
            </button>
            <button
              onClick={onGoSettings}
              className="toolbar-btn relative"
              title={t("stats.settings")}
            >
              ⚙
              {updateReady && (
                <span className="absolute top-0 right-0 w-1.5 h-1.5 rounded-full bg-rose-500" />
              )}
            </button>
            {isWindows && (
              <button
                onClick={() => {
                  const next = !pinned;
                  setPinned(next);
                  setPin(next).catch(() => setPinned(!next));
                }}
                className={`toolbar-btn ${pinned ? "text-sky-600!" : ""}`}
                title={pinned ? t("stats.unpin") : t("stats.pin")}
              >
                📌
              </button>
            )}
            <button
              onClick={() => {
                refresh();
                setSpeedRefresh((v) => v + 1);
              }}
              disabled={refreshing}
              className={`toolbar-btn ${refreshing ? "opacity-40" : ""}`}
              title={t("common.refresh")}
            >
              ↻
            </button>
            {/* GitHub 仓库入口：系统浏览器打开项目主页，与相邻工具按钮同款
                尺寸/悬停样式（openUrl 失败退化为 window.open，参照 UpdaterCard） */}
            <button
              onClick={() =>
                openUrl(GITHUB_REPO).catch(() =>
                  window.open(GITHUB_REPO, "_blank")
                )
              }
              className="toolbar-btn"
              title={t("stats.github")}
              aria-label={t("stats.github")}
            >
              <svg
                viewBox="0 0 24 24"
                fill="currentColor"
                className="h-3.5 w-3.5"
                aria-hidden
              >
                <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
              </svg>
            </button>
          </div>
        </div>
        {/* 速度 tab 自管时间范围（面板标题栏「今日 / 7天」小切换），全局
            RangePicker 对它无输入（面板不消费 preset/custom，点了没反应）；
            设备筛选同理（面板查询不走 DataCache 的设备过滤）。两者在 speed
            tab 激活时一并隐藏；此时 tab 栏回到 mt-2 顶距，布局不塌陷 */}
        {tab !== "speed" && (
          <RangePicker
            preset={preset}
            custom={custom}
            onChange={(p, c) => {
              setPreset(p);
              setCustom(c);
            }}
          />        )}
        {/* 设备筛选单独占一行，标签保持完整名称；未来新增 Agent 时横向滚动。 */}
        {tab !== "speed" && syncEnabled && (
          <div className="mt-2 flex items-center">
            {/* 设备筛选：改用共用 DropdownMenu（原生弹层在 WebView2 暗色下
                选中项灰底难读）；title/文案与原 select 一致 */}
            <DropdownMenu
              className="w-[4.75rem] shrink-0"
              title={t("stats.deviceFilter")}
              ariaLabel={t("stats.deviceFilter")}
              options={[
                { value: "all", label: t("stats.deviceAll") },
                {
                  value: "local",
                  label: syncConfig?.device_name
                    ? t("stats.deviceLocalName", { name: syncConfig.device_name })
                    : t("stats.deviceLocal"),
                },
                // 过滤口径见 selectableRemoteDevices：排除本机 + 零记录设备
                ...selectableRemoteDevices(remoteDevices, syncConfig?.device_id).map(
                  (d) => ({
                    value: d.device_id,
                    label: t("common.deviceOption", {
                      name: d.device_name,
                      id: d.device_id.slice(0, 6),
                    }),
                  }),
                ),
              ]}
              value={deviceFilter}
              onChange={setDeviceFilter}
            />
          </div>
        )}
        {/* tab 栏：横向滚动区 + 右侧固定「＋添加服务」入口。渐隐遮罩只盖
            滚动区；「＋」在遮罩之外始终可见——新 provider 的 tab 默认隐藏，
            这是新用户添加第一个凭证服务的唯一常驻入口 */}
        <div className={`${tab !== "speed" && syncEnabled ? "mt-1.5" : "mt-2"} flex items-stretch gap-1 min-w-0`}>
          <div
            ref={tabScrollRef}
            className={`min-w-0 overflow-x-auto ${
              tabOverflow ? "tab-fade-right" : ""
            }`}
            aria-label={t("stats.sourcesAria")}
          >
            <div className="flex w-max min-w-full gap-1 p-0.5 rounded-xl bg-slate-900/4">
              {visibleTabs.map((item) => {
              const tabId = item.id;
              const activeColors: Record<string, string> = {
                cursor: "bg-violet-500/12 text-violet-700 shadow-sm",
                claude: "bg-orange-500/12 text-orange-700 shadow-sm",
                codex: "bg-emerald-500/12 text-emerald-700 shadow-sm",
                kimi: "bg-indigo-500/12 text-indigo-700 shadow-sm",
                zai: "bg-sky-500/12 text-sky-700 shadow-sm",
                summary: "bg-sky-500/15 text-sky-700 shadow-sm",
                projects: "bg-amber-500/12 text-amber-700 shadow-sm",
                speed: "bg-teal-500/12 text-teal-700 shadow-sm",
                // 凭证驱动的新 provider：激活色贴近各自品牌色
                gemini: "bg-blue-500/12 text-blue-700 shadow-sm",
                grok: "bg-sky-500/12 text-sky-700 shadow-sm",
                qoder: "bg-violet-500/12 text-violet-700 shadow-sm",
                opencodego: "bg-orange-500/12 text-orange-700 shadow-sm",
                minimax: "bg-emerald-500/12 text-emerald-700 shadow-sm",
                moonshot: "bg-indigo-500/12 text-indigo-700 shadow-sm",
                deepseek: "bg-blue-600/12 text-blue-700 shadow-sm",
                longcat: "bg-yellow-400/20 text-yellow-700 shadow-sm",
                mimo: "bg-orange-500/12 text-orange-700 shadow-sm",
                alibaba: "bg-orange-600/12 text-orange-700 shadow-sm",
                alibabatoken: "bg-orange-400/15 text-orange-700 shadow-sm",
                stepfun: "bg-blue-500/12 text-blue-700 shadow-sm",
                doubao: "bg-teal-500/12 text-teal-700 shadow-sm",
                // DeepSeek Harness：与 deepseek 同品牌蓝（本地采集型）
                dsh: "bg-blue-600/12 text-blue-700 shadow-sm",
              };
              return (
                <button
                  key={tabId}
                  onClick={() => setTab(tabId)}
                  type="button"
                  aria-pressed={tab === tabId}
                  className={`flex shrink-0 items-center justify-center gap-1 whitespace-nowrap rounded-lg px-2.5 py-1 text-[10px] font-medium transition-all duration-150 ${
                    tab === tabId
                      ? activeColors[tabId] ?? "bg-surface text-slate-900 shadow-sm"
                      : "text-slate-600/60 hover:text-slate-800 hover:bg-slate-900/4"
                  }`}
                >
                  {item.brand && (
                    <BrandIcon brand={item.brand} className="h-3 w-3 shrink-0" />
                  )}
                  <span>{item.label}</span>
                </button>
              );
            })}
            </div>
          </div>
          {/* 「＋添加服务」：锚容器含按钮本体（点击外部关闭时豁免按钮，
              再次点击「＋」可切换浮层）；浮层 absolute 挂在按钮下方 */}
          <div ref={addAnchorRef} className="relative shrink-0">
            <button
              type="button"
              onClick={() => setAddMenuOpen((v) => !v)}
              title={t("credentials.addService")}
              aria-label={t("credentials.addService")}
              aria-expanded={addMenuOpen}
              className="flex h-full w-7 items-center justify-center rounded-xl bg-slate-900/4 text-slate-600/60 hover:text-slate-800 hover:bg-slate-900/8 transition-all duration-150"
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.5"
                strokeLinecap="round"
                className="h-3 w-3"
                aria-hidden
              >
                <path d="M12 5v14M5 12h14" />
              </svg>
            </button>
            {addMenuOpen && (
              <AddServiceMenu
                onPick={(p) => {
                  setAddMenuOpen(false);
                  setAddError(null);
                  setAddProvider(p);
                }}
              />
            )}
          </div>
        </div>
      </div>

      {/* 标签内容（zai：额度卡与统计内容同处一个滚动流，额度卡不再固定置顶
          挤压统计区高度，向下滚动时随内容滚出视野。
          额度采样由 DataProvider 全局定时器负责，与组件挂载无关。 */}
      {tab === "zai" ? (
        <div className="flex-1 min-h-0 overflow-y-auto px-3 py-2.5 page-stack">
          <QuotaPanel />
          {error && (
            <div className="mb-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
              {error}
            </div>
          )}
          <ZaiStatsContent
            stats={stats}
            cost={cost}
            trend={trend}
            pricing={pricing}
            currency={currency}
            trendBucket={trendBucket}
          />
        </div>
      ) : tab === "codex" ? (
        <CodexPanel
          snapshot={codex}
          loading={!codex && !codexError}
          error={codexError}
          currency={currency}
          fxRate={fxRate}
          trendBucket={trendBucket}
          pricing={pricing}
          agentQuotaDelta={agentQuotaDeltas.codex?.weekly}
        />
      ) : tab === "claude" ? (
        <ClaudePanel
          snapshot={claude}
          loading={!claude && !claudeError}
          error={claudeError}
          currency={currency}
          fxRate={fxRate}
          trendBucket={trendBucket}
          pricing={pricing}
          agentQuotaDelta={agentQuotaDeltas.claude?.weekly}
        />
      ) : tab === "cursor" ? (
        <CursorPanel
          snapshot={cursor}
          loading={!cursor && !cursorError}
          error={cursorError}
          currency={currency}
          fxRate={fxRate}
          autoQuotaDelta={agentQuotaDeltas.cursor?.cursor_auto}
          apiQuotaDelta={agentQuotaDeltas.cursor?.cursor_api}
        />
      ) : tab === "kimi" ? (
        <KimiPanel
          snapshot={kimi}
          loading={!kimi && !kimiError}
          error={kimiError}
          currency={currency}
          fxRate={fxRate}
          trendBucket={trendBucket}
          pricing={pricing}
          agentQuotaDelta={agentQuotaDeltas.kimi?.weekly}
        />
      ) : tab === "dsh" ? (
        // DSH（DeepSeek Harness）：用量为纯本地采集（快照无 rate_limits，额度块
        // 自动不渲染）；面板底部另有「DeepSeek 平台余额」区（ProviderAccountsSection，
        // provider id 为 deepseek，手动 API Key → /user/balance）
        <DshPanel
          snapshot={dsh}
          loading={!dsh && !dshError}
          error={dshError}
          currency={currency}
          fxRate={fxRate}
          trendBucket={trendBucket}
          pricing={pricing}
        />
      ) : tab === "opencodego" ? (
        // OpenCode：本地采集型用量面板（本地 opencode.db 导入库）+ 顶部 Go
        // 套餐额度卡（providerQuota["opencodego"]）。agentId 仍是 opencodego，
        // 故必须在 isCredentialAgent 分支之前摘出来——否则会落到
        // GenericQuotaPanel 只剩额度、看不到本地用量统计
        <OpencodePanel
          snapshot={opencode}
          loading={!opencode && !opencodeError}
          error={opencodeError}
          currency={currency}
          fxRate={fxRate}
          trendBucket={trendBucket}
          pricing={pricing}
        />
      ) : tab === "minimax" ? (
        // MiniMax Code：本地采集型用量面板（本地 ~/.minimax 库）+ 顶部套餐
        // 额度卡（providerQuota["minimax"]，仍由已存的 sk-cp- Key 查询）。
        // agentId 仍是 minimax，故必须在 isCredentialAgent 分支之前摘出来
        // ——否则会落到 GenericQuotaPanel 只剩额度、看不到本地用量统计
        <MinimaxPanel
          snapshot={minimaxcode}
          loading={!minimaxcode && !minimaxcodeError}
          error={minimaxcodeError}
          currency={currency}
          fxRate={fxRate}
          trendBucket={trendBucket}
          pricing={pricing}
        />
      ) : isCredentialAgent(tab) ? (
        // 凭证驱动的新 provider：通用额度面板（凭证卡 + 配额卡；配额数据
        // 由面板经 DataCache 的 providerQuota 缓存自取，未接入的 provider
        // entries 为空，仅显示凭证管理与接入提示）
        <GenericQuotaPanel
          provider={tab}
          title={
            AGENT_VISIBILITY_OPTIONS.find((o) => o.id === tab)?.label ?? tab
          }
          // Qoder 区分国际站（qoder.com）与中国站（qoder.com.cn），
          // Cookie 归属站点必须与 region 一致，录入时显式可选；
          // 阿里 Coding Plan / Token Plan、MiniMax / Moonshot 同为
          // 国内/国际双站（选项配置见 regionOptionsFor，与「＋添加服务」
          // 表单共用同一份；后端 host_for_region 按 region 选查询主机）
          regionOptions={regionOptionsFor(tab, t)}
        />
      ) : tab === "projects" ? (
        <ProjectsPanel
          preset={preset}
          custom={custom}
          currency={currency}
          fxRate={fxRate}
        />
      ) : tab === "speed" ? (
        // 模型速度统计：本地库即席查询（与 Agent 面板的 DataProvider 流
        // 独立），自取数据自管时间范围；refreshToken 驱动全局 ↻ 联动重查
        <ModelSpeedPanel refreshToken={speedRefresh} />
      ) : (
        <SummaryTab
          stats={stats}
          cost={cost}
          trend={trend}
          codex={codex}
          claude={claude}
          cursor={cursor}
          kimi={kimi}
          dsh={dsh}
          opencode={opencode}
          minimaxcode={minimaxcode}
          currency={currency}
          bucket={trendBucket}
          fxRate={fxRate}
          pricing={pricing}
          agentVisibility={agentVisibility}
          rangeLabel={summaryRangeLabel}
          rangePreset={preset}
          onRangeChange={(p: RangePreset) => setPreset(p)}
          deviceLabel={summaryDeviceLabel}
        />
      )}

      {/* 底部：刷新时刻 + 价格设置入口（窄窗二期减负：面板不再有价格页，
          入口改为打开主窗口并直达定价页，跳页走 Rust 侧
          zbar://dashboard-navigate）。主窗口入口已回到标题栏图标按钮
          （见上方工具条），打开失败的轻提示仍留在这一行——工具条内没有放
          文字胶囊的位置，底部行有富余空间 */}
      <div className="px-3 py-1.5 border-t border-slate-900/8 flex items-center justify-between gap-2 text-[10px] text-slate-600/50">
        <span className="num">
          {lastUpdate
            ? new Date(lastUpdate).toLocaleTimeString(dateLocale(locale), {
                hour: "2-digit",
                minute: "2-digit",
              })
            : ""}
        </span>
        <div className="flex items-center gap-2 shrink-0">
          {/* 打开失败的行内轻提示：与面板既有错误提示同色（bg-red-500/15 +
              text-red-700），只是压成底部行里的一个小胶囊，2.4s 后自动消失 */}
          {mainOpenFailed && (
            <span className="px-1.5 py-0.5 rounded bg-red-500/15 text-red-700 whitespace-nowrap">
              {t("dashboard.openMainWindowFailed")}
            </span>
          )}
          <button
            onClick={() => openMainWindowWithToast("pricing")}
            className="hover:text-sky-600 transition-colors font-medium"
            title={t("stats.priceSettingsHint")}
            aria-label={t("stats.priceSettingsHint")}
          >
            {t("stats.priceSettings")}
          </button>
        </div>
      </div>

      {/* 「＋添加服务」→ 该服务的添加凭证弹层（复用 CredentialsCard 表单，
          覆盖整个面板；添加成功后 tab 自动出现并切换，见 handleAddSubmit） */}
      {addProvider && (
        <CredentialFormDialog
          kind={CREDENTIAL_AGENT_KIND[addProvider]}
          editing={null}
          regionOptions={regionOptionsFor(addProvider, t)}
          // kimi 表单内可选凭证类型（OAuth 令牌 / API Key）
          kindSelectable={addProvider === "kimi"}
          oauth={oauthFlow}
          onOAuthSuccess={() => handleOAuthSuccess(addProvider)}
          busy={addBusy}
          error={addError}
          titleText={`${
            AGENT_VISIBILITY_OPTIONS.find((o) => o.id === addProvider)?.label ??
            addProvider
          } · ${t("credentials.add")}`}
          onCancel={() => {
            setAddProvider(null);
            setAddError(null);
          }}
          onSubmit={handleAddSubmit}
        />
      )}
    </div>
  );
}
