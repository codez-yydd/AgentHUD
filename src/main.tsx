import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import {
  applyPanelAlpha,
  bootstrapTheme,
  loadPanelAlpha,
} from "./appearance";
import { applyFontSettings, watchFontChanges } from "./fontSettings";
import { applyLocale, detectLocale, loadLocale } from "./i18n/locale";
import { I18nProvider } from "./i18n";
import { restoreWindowSize } from "./windowSize";
import "./index.css";

// 首帧前应用外观偏好（主题/透明度/字号）与语言偏好，避免暗色/英文用户看到闪烁
// 主题走 bootstrapTheme：显式来源仅暗色时切 .dark（与升级前同语义）；
// 「跟随系统」来源重解析当前系统主题（应用未运行期间系统可能已切换）
bootstrapTheme();
applyPanelAlpha(loadPanelAlpha());
// 字号：读新键，旧 zbar-ui-scale 自动按 13px 基准迁移，升级后视觉不变
applyFontSettings();
// 主窗口「设置」页改字号后跨窗同步（本窗常驻，监听随窗口销毁自动清理）
watchFontChanges();
applyLocale(loadLocale() ?? detectLocale());

// 恢复持久化的窗口尺寸（百分比 → 当前显示器像素）：未存储时内部早退保持默认，
// 窗口此时仍隐藏无闪烁；fire-and-forget 不阻塞首帧
void restoreWindowSize();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      <App />
    </I18nProvider>
  </React.StrictMode>,
);
