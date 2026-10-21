import React from "react";
import ReactDOM from "react-dom/client";
import { DashboardApp } from "./dashboard/DashboardApp";
import { readSyncConfigCached } from "./dashboard/syncConfig";
import { applyTheme, loadTheme } from "./appearance";
import { applyLocale, detectLocale, loadLocale } from "./i18n/locale";
import { I18nProvider } from "./i18n";
import "./dashboard.css";

/**
 * 主窗口（Dashboard）入口：与面板 main.tsx 同款引导，但去掉面板专属能力
 * （窗口尺寸恢复、面板透明度/整体缩放——那是 300×500 悬浮窗的适配层，
 * 大窗不需要，也避免把 --panel-alpha / --ui-scale 带进普通窗）。
 *
 * 首帧前应用外观偏好：主题（仅暗色时切 .dark，避免亮色用户看到闪烁；
 * 与 dashboard.html 的内联兜底脚本同语义，幂等）与语言（持久化值优先，
 * 无值/损坏值回退系统语言检测）——两者都读面板写入的同一套 localStorage
 * 键，保证打开主窗口时与面板一致，无"先亮后暗"跳变。
 */
if (loadTheme() === "dark") applyTheme("dark");
applyLocale(loadLocale() ?? detectLocale());

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
        <DashboardApp />
      </I18nProvider>
    </React.StrictMode>,
  );
});
