/**
 * 主窗口（Dashboard）应用壳：左侧固定侧栏（产品标识 + 四项导航 + 底部
 * 语言/主题快捷切换）+ 右侧主区。
 *
 * 视图切换用组件内 state（对齐面板 App.tsx 的 view 模式，不引路由库）：
 * 用量总览 / 请求日志 / 周额度对比 / 价格设置 四个页面各自独立取数渲染。
 */
import { useEffect, useRef, useState } from "react";
import type { ComponentType } from "react";
import { listen } from "@tauri-apps/api/event";
// 软件 logo：源图取自 src-tauri/icons/128x128.png（128px 透明 PNG），
// 由 Vite 打包为带哈希的资源 URL（dev 走 dev server，构建走 dist/assets）
import logo from "../assets/logo.png";
import { LanguageToggle, ThemeToggle } from "../layout";
import { APPEARANCE_CHANGED_EVENT, loadTheme, type Theme } from "../appearance";
import { loadLocale } from "../i18n/locale";
import { useI18n, type MessageKey } from "../i18n";
import type { IconProps } from "./icons";
import { CompareIcon, LogsIcon, OverviewIcon, PricingIcon } from "./icons";
import { OverviewPage } from "./OverviewPage";
import { LogsPage } from "./logs/LogsPage";
import { ComparePage } from "./ComparePage";
import { PricingPage } from "./PricingPage";

/** 主区视图（新增页面时在此扩展） */
type View = "overview" | "logs" | "compare" | "pricing";

/** 侧栏导航项：顺序即展示顺序；labelKey 同时用作页面标题词典键 */
const NAV_ITEMS: {
  view: View;
  labelKey: MessageKey;
  Icon: ComponentType<IconProps>;
}[] = [
  { view: "overview", labelKey: "dashboard.overview", Icon: OverviewIcon },
  { view: "logs", labelKey: "dashboard.logs", Icon: LogsIcon },
  { view: "compare", labelKey: "dashboard.compare", Icon: CompareIcon },
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

  return (
    <div className="db-shell">
      <aside className="db-sidebar">
        {/* 产品标识：软件 logo 图标 + 名称（图标为透明底 PNG，样式见
            dashboard.css .db-brand__logo-img） */}
        <div className="db-brand">
          <img
            src={logo}
            alt="ZBar"
            className="db-brand__logo-img"
            draggable={false}
          />
          <div>
            <div className="db-brand__name">ZBar</div>
            <div className="db-brand__tagline">{t("dashboard.subtitle")}</div>
          </div>
        </div>

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
            localStorage 偏好与同一广播，行为与面板一致）；ThemeToggle 换 key
            以跟随外窗切换（见上方同步流注释） */}
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
        ) : view === "compare" ? (
          <ComparePage />
        ) : (
          <PricingPage />
        )}
      </main>
    </div>
  );
}
