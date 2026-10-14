import React from "react";
import ReactDOM from "react-dom/client";
import { DashboardApp } from "./dashboard/DashboardApp";
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

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      <DashboardApp />
    </I18nProvider>
  </React.StrictMode>,
);
