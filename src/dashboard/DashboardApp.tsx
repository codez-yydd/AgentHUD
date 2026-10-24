/**
 * 主窗口（Dashboard）应用壳：顶部自绘标题栏（品牌标识 + 窗口控制，替代
 * decorations: false 后的系统标题栏）+ 左侧固定侧栏（五项导航 + 底部
 * 语言/主题快捷切换）+ 右侧主区。
 *
 * 视图切换用组件内 state（对齐面板 App.tsx 的 view 模式，不引路由库）：
 * 用量总览 / 请求日志 / 模型速度 / 周额度对比 / 同步与设备 / 价格设置 六个
 * 页面各自独立取数渲染。
 */
import { useEffect, useRef, useState } from "react";
import type { ComponentType } from "react";
import { listen } from "@tauri-apps/api/event";
import { LanguageToggle, ThemeToggle } from "../layout";
import { APPEARANCE_CHANGED_EVENT, loadTheme, type Theme } from "../appearance";
import { loadLocale } from "../i18n/locale";
import { useI18n, type MessageKey } from "../i18n";
import type { IconProps } from "./icons";
import {
  CompareIcon,
  LogsIcon,
  OverviewIcon,
  PricingIcon,
  SpeedIcon,
  SyncIcon,
} from "./icons";
import { TitleBar } from "./TitleBar";
import { OverviewPage } from "./OverviewPage";
import { LogsPage } from "./logs/LogsPage";
import { SpeedPage } from "./SpeedPage";
import { ComparePage } from "./ComparePage";
import { PricingPage } from "./PricingPage";
import { SyncPage } from "./SyncPage";

/** 主区视图（新增页面时在此扩展） */
type View = "overview" | "logs" | "speed" | "compare" | "sync" | "pricing";

/** 面板 → 主窗口跳页事件（Rust 侧 open_main_window(page) 在开窗/聚焦后
 *  emit_to 本窗，见 main_window.rs；事件名风格沿 zbar://appearance-changed，
 *  两侧字面量需保持一致）。payload 为下方合法页 id */
const DASHBOARD_NAVIGATE_EVENT = "zbar://dashboard-navigate";

/** 跳页 payload 的合法值（与 View 枚举同源；与 Rust 侧 DASHBOARD_PAGES
 *  白名单字面一致，改动任一侧需同步）。非法值忽略，保持当前页 */
const NAVIGATABLE_VIEWS: readonly string[] = [
  "overview",
  "logs",
  "speed",
  "compare",
  "sync",
  "pricing",
];

/** 侧栏导航项：顺序即展示顺序；labelKey 同时用作页面标题词典键 */
const NAV_ITEMS: {
  view: View;
  labelKey: MessageKey;
  Icon: ComponentType<IconProps>;
}[] = [
  { view: "overview", labelKey: "dashboard.overview", Icon: OverviewIcon },
  { view: "logs", labelKey: "dashboard.logs", Icon: LogsIcon },
  { view: "speed", labelKey: "dashboard.speed", Icon: SpeedIcon },
  { view: "compare", labelKey: "dashboard.compare", Icon: CompareIcon },
  { view: "sync", labelKey: "dashboard.sync", Icon: SyncIcon },
  { view: "pricing", labelKey: "dashboard.pricing", Icon: PricingIcon },
];

/**
 * 把主题应用到本窗 <html>（appearance.ts loadTheme 同语义：仅 "dark" 视为
 * 暗色，其余一律亮色）。刻意不调 applyTheme：那是"写入方"语义（会再次
 * 广播外观事件）——本窗在同步流里只是跟随者，直接切类避免"收到广播 →
 * 再广播"的回声（对齐 session-hud-main.ts 的 applyHudTheme 做法）。
 */
function applyDomTheme(theme: Theme): void {
  document.documentElement.classList.toggle("dark", theme === "dark");
}

export function DashboardApp() {
  const { locale, setLocale, t } = useI18n();
  const [view, setView] = useState<View>("overview");
  // 主题只用于给 ThemeToggle 换 key（见下方）：组件自身状态由 layout.tsx
  // 管理，这里记录"当前应显示的主题"以便外部切换后强制重读
  const [theme, setTheme] = useState<Theme>(() => loadTheme());

  // 外观同步流：面板切换主题（applyTheme）/ 语言（setLocale）后广播
  // zbar://appearance-changed（无 payload，接收方重读 localStorage 自行
  // 对比；常量与面板共用 appearance.ts 单一来源）。
  //  - 主题：切 .dark（纯 CSS 生效）+ setTheme 更新 key，让 ThemeToggle
  //    重新挂载并重读偏好（它只在挂载时读一次 localStorage，外窗切换后
  //    图标不换 key 会停在上一次的状态）；
  //  - 语言：与当前不同才 setLocale（同值调用也会广播 + 写盘，虽幂等但
  //    无必要；setLocale 内部会同步 <html lang> 并持久化）。
  // 监听失败静默：保持启动时读取的偏好（本窗为长生命周期，监听随窗口
  // 销毁自动清理，卸载时兜底 unlisten）。
  const localeRef = useRef(locale);
  localeRef.current = locale;

  useEffect(() => {
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen(APPEARANCE_CHANGED_EVENT, () => {
      const nextTheme = loadTheme();
      applyDomTheme(nextTheme);
      setTheme(nextTheme);
      const nextLocale = loadLocale();
      if (nextLocale && nextLocale !== localeRef.current) {
        setLocale(nextLocale);
      }
    })
      .then((unlisten) => {
        // effect 已清理（StrictMode 双挂载 / 快速卸载）时立即退订
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：保持启动时读取的主题与语言 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [setLocale]);

  // 面板入口跳页（如窄窗「价格设置」按钮 → 本窗定价页）：监听 Rust 侧
  // zbar://dashboard-navigate，payload 合法才 setView（Rust 侧已按白名单
  // 过滤，这里兜底校验，非法/缺省忽略）。窗口预建隐藏时 WebView 已存活、
  // 本监听已注册，事件可达（appearance-changed 同款前提）；导航是离散
  // 意图，无需外观同步那套同值守卫。监听失败静默：入口退化为只开窗。
  useEffect(() => {
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen<string>(DASHBOARD_NAVIGATE_EVENT, (event) => {
      if (NAVIGATABLE_VIEWS.includes(event.payload)) {
        setView(event.payload as View);
      }
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：保持当前页 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, []);

  return (
    <div className="db-shell">
      <TitleBar />

      {/* 标题栏之下的主体行：左侧固定侧栏 + 右侧主区 */}
      <div className="db-body">
        <aside className="db-sidebar">
          <nav className="db-nav">
            {NAV_ITEMS.map(({ view: itemView, labelKey, Icon }) => (
              <button
                key={itemView}
                type="button"
                onClick={() => setView(itemView)}
                className={`db-nav__item ${
                  view === itemView ? "db-nav__item--active" : ""
                }`}
                aria-current={view === itemView ? "page" : undefined}
              >
                <span className="db-nav__icon">
                  <Icon className="h-4 w-4" />
                </span>
                {t(labelKey)}
              </button>
            ))}
          </nav>

          {/* 底部：复用面板的语言/主题快捷切换（只换按钮样式类，读写同一套
              localStorage 偏好与同一广播，行为与面板一致）；ThemeToggle 换
              key 以跟随外窗切换（见上方同步流注释） */}
          <div className="db-sidebar__footer">
            <ThemeToggle key={theme} className="db-icon-btn" />
            <LanguageToggle className="db-icon-btn" />
          </div>
        </aside>

        <main className="db-main">
          {view === "overview" ? (
            <OverviewPage />
          ) : view === "logs" ? (
            <LogsPage />
          ) : view === "speed" ? (
            <SpeedPage />
          ) : view === "compare" ? (
            <ComparePage />
          ) : view === "sync" ? (
            <SyncPage />
          ) : (
            <PricingPage />
          )}
        </main>
      </div>
    </div>
  );
}
