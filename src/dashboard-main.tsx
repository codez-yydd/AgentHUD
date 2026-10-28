import React from "react";
import ReactDOM from "react-dom/client";
import { DashboardApp } from "./dashboard/DashboardApp";
import { readSyncConfigCached } from "./dashboard/syncConfig";
import { CurrencyPrefProvider } from "./dashboard/settings/currencyPref";
import { bootstrapTheme, watchSystemTheme } from "./appearance";
import { applyFontSettings, watchFontChanges } from "./fontSettings";
import { applyLocale, detectLocale, loadLocale } from "./i18n/locale";
import { I18nProvider } from "./i18n";
import "./dashboard.css";

/**
 * 主窗口（Dashboard）入口：与面板 main.tsx 同款引导，去掉面板专属能力
 * （窗口尺寸恢复、面板透明度），但**接入同一套字号设置**——只是主窗口
 * 自己持有一份基准字号（默认 13px，可开启跟随面板），避免升级后主窗口
 * 被面板历史档位连带放大。--ui-scale 由 dashboard.css 的 #root 变换消费。
 *
 * 首帧前应用外观偏好：主题（bootstrapTheme：显式来源仅暗色时切 .dark，
 * 避免亮色用户看到闪烁；「跟随系统」来源重解析当前系统主题——应用未
 * 运行期间系统可能已切换）、字号与语言（持久化值优先，无值/损坏值回退
 * 系统语言检测）——都读面板写入的同一套 localStorage 键，保证打开主窗口
 * 时与面板一致，无"先亮后暗"跳变。
 *
 * 主窗口是「设置」页的属主窗口：系统主题变化监听（主题来源=跟随系统时
 * 立即切换并广播，其他窗口经 appearance-changed 跟随）与本窗口字号变更
 * 监听都挂在本入口；币种偏好上下文（CurrencyPrefProvider）包住全部页面。
 */
bootstrapTheme();
applyFontSettings();
watchFontChanges();
applyLocale(loadLocale() ?? detectLocale());
watchSystemTheme();

/** 同步配置预热上限：本地文件 IPC 通常 1–3ms，失败/超时才放行首帧 */
const SYNC_PRELOAD_TIMEOUT_MS = 150;

/** 首帧前读一次同步配置：总览页的持久化快照按「全部设备」口径渲染依赖它，
 *  否则首帧会先按本机口径画一帧、等 effect 读到配置后再切全部设备（数据
 *  从少到多闪一下）。超时/失败照常启动，页面内仍会重读并按未启用降级。 */
function preloadSyncConfig(): Promise<void> {
  return new Promise((resolve) => {
    const timer = window.setTimeout(resolve, SYNC_PRELOAD_TIMEOUT_MS);
    readSyncConfigCached({ refresh: true }).finally(() => {
      window.clearTimeout(timer);
      resolve();
    });
  });
}

void preloadSyncConfig().then(() => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <I18nProvider>
        <CurrencyPrefProvider>
          <DashboardApp />
        </CurrencyPrefProvider>
      </I18nProvider>
    </React.StrictMode>,
  );
});
