/**
 * 灵动岛设置窗外壳（island-settings.html 的根组件）
 */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ComponentType,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import logoMark from "../../assets/logo-mark.png";
import { APPEARANCE_CHANGED_EVENT } from "../../appearance";
import { fontFamilyWithFallback, SETTINGS_FONT_STACK, SETTINGS_NUM_FONT_STACK } from "../../island/fontFamily";
import { onEvent } from "../../island/api";
import { Button, IconButton, StatusDot } from "./wset/controls";
import {
  ActionsIcon,
  CloseIcon,
  GearIcon,
  HotkeyIcon,
  InfoIcon,
  LauncherIcon,
  LiveIcon,
  LookIcon,
  MediaIcon,
  MessageIcon,
  OrderIcon,
  StockIcon,
  SystemIcon,
  TranslateIcon,
  WeatherIcon,
  type WsetIconProps,
} from "./wset/icons";
import {
  PAGES,
  pageOfKey,
  rowDomId,
  type PageKey,
  type PageMeta,
} from "./wset/schema";
import { hideSettingsWindow, startWindowDrag } from "./wset/settingsApi";
import { useSettingsDraft } from "./wset/useSettingsDraft";
import { AppearancePage } from "./wset/pages/AppearancePage";
import { OrderPage } from "./wset/pages/OrderPage";
import { HotkeysPage } from "./wset/pages/HotkeysPage";
import { ActionsPage } from "./wset/pages/ActionsPage";
import { MediaPage } from "./wset/pages/MediaPage";
import { NotifyPage } from "./wset/pages/NotifyPage";
import { StocksPage } from "./wset/pages/StocksPage";
import { WeatherPage } from "./wset/pages/WeatherPage";
import { TranslatePage } from "./wset/pages/TranslatePage";
import { LivePage } from "./wset/pages/LivePage";
import { LauncherPage } from "./wset/pages/LauncherPage";
import { SystemPage } from "./wset/pages/SystemPage";
import { AdvancedPage } from "./wset/pages/AdvancedPage";
import { AboutPage } from "./wset/pages/AboutPage";
import { PlaceholderPage } from "./wset/pages/PlaceholderPage";

/** 侧栏 14 个分类的图标（顺序 / 文案在 schema 的 PAGES 里，改这里只换图标） */
const NAV_ICONS: Record<PageKey, ComponentType<WsetIconProps>> = {
  look: LookIcon,
  order: OrderIcon,
  hotkeys: HotkeyIcon,
  actions: ActionsIcon,
  media: MediaIcon,
  notify: MessageIcon,
  stocks: StockIcon,
  weather: WeatherIcon,
  translate: TranslateIcon,
  live: LiveIcon,
  launcher: LauncherIcon,
  system: SystemIcon,
  advanced: GearIcon,
  about: InfoIcon,
};

/** 可以写进 data-wtheme 的显式主题（auto 不在其中，需要先解析） */
type ResolvedTheme = "light" | "dark" | "rouge" | "sky";

const EXPLICIT_THEMES = new Set<string>(["light", "dark", "rouge", "sky"]);

/** 系统深浅色（auto 主题解析依据，与 appearance.ts 同一个 .dark 类） */
function systemTheme(): ResolvedTheme {
  return document.documentElement.classList.contains("dark") ? "dark" : "light";
}

export function SettingsWindow() {
  const draft = useSettingsDraft();
  const [page, setPage] = useState<PageKey>("look");
  const [recording, setRecording] = useState(false);
  /** 外观事件计数：只用来让 auto 主题重新解析（.dark 由别处切换） */
  const [appearanceTick, setAppearanceTick] = useState(0);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const meta = useMemo<PageMeta>(
    () => PAGES.find((item) => item.key === page) ?? PAGES[0],
    [page]
  );

  // 主题：显式主题直接用；auto 时按当前系统深浅色解析（草稿改动立即预览）
  const themeValue = String(draft.value("island.theme") ?? "auto");
  const resolvedTheme = useMemo<ResolvedTheme>(() => {
    // appearanceTick 只参与「重新解析」的触发，不参与计算逻辑
    void appearanceTick;
    return EXPLICIT_THEMES.has(themeValue) ? (themeValue as ResolvedTheme) : systemTheme();
  }, [themeValue, appearanceTick]);

  useEffect(() => {
    document.documentElement.dataset.wtheme = resolvedTheme;
  }, [resolvedTheme]);

  // 字体预览：草稿值实时覆盖整个设置窗口（含 portal 到 body 的字体弹窗）。
  // 数字控件也以自定义字体为首选，但保留独立的数字回退栈。
  const fontFamily = String(draft.value("island.font_family") ?? "").trim();
  useEffect(() => {
    const root = document.documentElement;
    if (fontFamily) {
      root.style.setProperty("--w-font", fontFamilyWithFallback(fontFamily, SETTINGS_FONT_STACK));
      root.style.setProperty(
        "--w-num-font",
        fontFamilyWithFallback(fontFamily, SETTINGS_NUM_FONT_STACK)
      );
    } else {
      root.style.removeProperty("--w-font");
      root.style.removeProperty("--w-num-font");
    }
    return () => {
      root.style.removeProperty("--w-font");
      root.style.removeProperty("--w-num-font");
    };
  }, [fontFamily]);

  // 外观广播：auto 主题需要跟着系统深浅色重新解析（配置同步在 useSettingsDraft）
  useEffect(
    () =>
      onEvent(APPEARANCE_CHANGED_EVENT, () => {
        setAppearanceTick((tick) => tick + 1);
      }),
    []
  );

  // 切页时内容区回到顶部
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [page]);

  // 首个校验错误：跳到错误所在页并滚动到该行
  const firstErrorKey = Object.keys(draft.errors)[0] ?? null;
  useEffect(() => {
    if (!firstErrorKey) return;
    const target = pageOfKey(firstErrorKey);
    if (target && target !== page) setPage(target);
    const raf = requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        document
          .getElementById(rowDomId(firstErrorKey))
          ?.scrollIntoView({ block: "center", behavior: "smooth" });
      });
    });
    return () => cancelAnimationFrame(raf);
  }, [firstErrorKey, page]);

  const flushPendingSave = draft.flushPendingSave;
  const hide = useCallback(() => {
    // 关窗前尽力冲刷一次尚未提交的防抖保存（来不及也无妨，
    // 设计上以「最后一次已提交保存」为界，不追求绝对不丢）
    flushPendingSave();
    void hideSettingsWindow();
  }, [flushPendingSave]);

  // 键盘：Esc 直接关窗（录制中 / 弹层打开时由对应控件接管）
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (recording) return;
      if (event.key === "Escape") {
        event.preventDefault();
        hide();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [hide, recording]);

  /** 标题栏整条可拖动；按钮区排除（否则点按钮会先触发拖动） */
  const onHeaderMouseDown = (event: ReactMouseEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (target?.closest("button, a, input, [data-no-drag]")) return;
    void startWindowDrag();
  };

  const renderPage = (): ReactNode => {
    if (draft.loadError) {
      return (
        <div className="wset-panel">
          <h1 className="wset-panel__title">无法读取灵动岛设置</h1>
          <p className="wset-panel__desc">
            {draft.loadError}
            。设置数据由灵动岛后端提供（island_settings_doc），请确认后端已接线后重试。
          </p>
          <div className="wset-panel__actions">
            <Button variant="primary" onClick={() => void draft.retry()}>
              重试
            </Button>
          </div>
        </div>
      );
    }
    if (!draft.doc) {
      return (
        <div className="wset-panel">
          <p className="wset-panel__desc">正在读取设置…</p>
        </div>
      );
    }
    switch (page) {
      case "look":
        return <AppearancePage draft={draft} saveTick={draft.saveTick} />;
      case "order":
        return <OrderPage draft={draft} saveTick={draft.saveTick} />;
      case "hotkeys":
        return (
          <HotkeysPage
            draft={draft}
            saveTick={draft.saveTick}
            onRecordingChange={setRecording}
          />
        );
      case "actions":
        return <ActionsPage draft={draft} saveTick={draft.saveTick} />;
      case "media":
        return <MediaPage draft={draft} saveTick={draft.saveTick} />;
      case "notify":
        return <NotifyPage draft={draft} saveTick={draft.saveTick} />;
      case "stocks":
        return <StocksPage draft={draft} saveTick={draft.saveTick} />;
      case "weather":
        return <WeatherPage draft={draft} saveTick={draft.saveTick} />;
      case "translate":
        return <TranslatePage draft={draft} saveTick={draft.saveTick} />;
      case "live":
        return <LivePage draft={draft} saveTick={draft.saveTick} />;
      case "launcher":
        return <LauncherPage draft={draft} saveTick={draft.saveTick} />;
      case "system":
        return <SystemPage draft={draft} saveTick={draft.saveTick} />;
      case "advanced":
        return <AdvancedPage draft={draft} saveTick={draft.saveTick} />;
      case "about":
        return <AboutPage draft={draft} saveTick={draft.saveTick} />;
      default:
        return <PlaceholderPage meta={meta} />;
    }
  };

  return (
    <div className="wset">
      <header className="wset__header" onMouseDown={onHeaderMouseDown}>
        <img className="wset__logo" src={logoMark} alt="" draggable={false} />
        <div className="wset__titles">
          <div className="wset__title">AgentHUD 设置</div>
        </div>
        <div className="wset__spacer" />
        <IconButton ariaLabel="关闭" title="关闭" onClick={hide}>
          <CloseIcon size={14} />
        </IconButton>
      </header>

      <div className="wset__body">
        <nav className="wset__side" aria-label="设置分类">
          {PAGES.map((item) => {
            const Icon = NAV_ICONS[item.key];
            const active = item.key === page;
            return (
              <button
                key={item.key}
                type="button"
                className={`wset-nav__item${active ? " wset-nav__item--active" : ""}`}
                aria-current={active ? "page" : undefined}
                onClick={() => setPage(item.key)}
              >
                <span className="wset-nav__icon">
                  <Icon size={18} />
                </span>
                <span className="wset-nav__label">{item.title}</span>
              </button>
            );
          })}
        </nav>

        <div className="wset__content" ref={scrollRef}>
          {renderPage()}
        </div>
      </div>

      <footer className="wset__foot">
        <div className={`wset-status wset-status--${draft.status.role}`}>
          <StatusDot role={draft.status.role} />
          <span className="wset-status__text" title={draft.status.text}>
            {draft.status.text}
          </span>
        </div>
      </footer>
    </div>
  );
}
