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
import { onEvent } from "../../island/api";
import { Button, Confirm, IconButton, StatusDot } from "./wset/controls";
import {
  ActionsIcon,
  CheckIcon,
  CloseIcon,
  GearIcon,
  HotkeyIcon,
  InfoIcon,
  LauncherIcon,
  LiveIcon,
  LookIcon,
  MediaIcon,
  MessageIcon,
  OpenIcon,
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
import { hideSettingsWindow, openSettingsFile, startWindowDrag } from "./wset/settingsApi";
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
  const [confirmClose, setConfirmClose] = useState(false);
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

  const hide = useCallback(() => {
    void hideSettingsWindow();
  }, []);

  /** 关闭 / 取消 / Esc：有未保存改动先确认 */
  const requestClose = useCallback(() => {
    if (draft.dirtyCount > 0) {
      setConfirmClose(true);
      return;
    }
    hide();
  }, [draft.dirtyCount, hide]);

  const handleSave = useCallback(() => {
    // 输入框里的半截编辑（例如步进框里刚敲的数字）先提交再保存：
    // blur 会同步触发 onBlur → setValue，draftRef 随即就是最新值
    const active = document.activeElement;
    if (active instanceof HTMLElement) active.blur();
    void draft.save();
  }, [draft]);

  // 键盘：Ctrl/Cmd+S 保存；Esc 关闭（录制中 / 弹层打开时由对应控件接管）
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (recording) return;
      const mod = event.ctrlKey || event.metaKey;
      if (mod && (event.key === "s" || event.key === "S")) {
        event.preventDefault();
        handleSave();
        return;
      }
      if (event.key === "Escape") {
        if (confirmClose) return;
        event.preventDefault();
        requestClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [confirmClose, handleSave, recording, requestClose]);

  /** 标题栏整条可拖动；按钮区排除（否则点按钮会先触发拖动） */
  const onHeaderMouseDown = (event: ReactMouseEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (target?.closest("button, a, input, [data-no-drag]")) return;
    void startWindowDrag();
  };

  const openConfig = () => {
    void openSettingsFile().catch((error) => {
      const reason =
        typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
      draft.setStatus(`打开配置文件失败：${reason}`, "danger", true);
    });
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
          <p className="wset-panel__desc">正在读取 settings.json…</p>
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
          <div className="wset__sub">settings.json · 修改后点「保存并应用」立即生效</div>
        </div>
        <div className="wset__spacer" />
        <IconButton ariaLabel="打开配置文件" title="打开 settings.json" onClick={openConfig}>
          <OpenIcon size={14} />
        </IconButton>
        <IconButton ariaLabel="关闭" title="关闭" onClick={requestClose}>
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
        <div className="wset__footbtns">
          <Button onClick={requestClose}>取消</Button>
          <Button
            variant="primary"
            icon={<CheckIcon size={12} />}
            onClick={handleSave}
            disabled={draft.loading && !draft.doc}
          >
            保存并应用
          </Button>
        </div>
      </footer>

      <Confirm
        open={confirmClose}
        title="放弃未保存的修改？"
        body={`还有 ${draft.dirtyCount} 项修改没有保存，关闭后这些修改会丢失。`}
        okText="放弃修改"
        danger
        onOk={() => {
          setConfirmClose(false);
          draft.discard();
          hide();
        }}
        onCancel={() => setConfirmClose(false)}
      />
    </div>
  );
}
