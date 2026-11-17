import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import {
  applyPanelAlpha,
  bootstrapTheme,
  loadPanelAlpha,
  watchSystemTheme,
} from "./appearance";
import { applyFontSettings, watchFontChanges } from "./fontSettings";
import { applyLocale, detectLocale, loadLocale } from "./i18n/locale";
import { I18nProvider } from "./i18n";
import { restoreWindowSize } from "./windowSize";
import "./index.css";

// 禁用 WebView2 默认右键菜单（返回/刷新/打印等原生菜单）；
// 输入框类元素放行，保留复制/粘贴的原生菜单，与其他窗口入口对齐。
document.addEventListener(
  "contextmenu",
  (event) => {
    const target = event.target as HTMLElement | null;
    if (target?.closest("input, textarea, [contenteditable='true']")) return;
    event.preventDefault();
  },
  true
);

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

// 系统主题跟随（「跟随系统」来源时系统切深浅色立即广播）：原先唯一宿主
// 是主窗口（dashboard-main 挂载），省内存改造后主窗口关闭即销毁——主窗
// 不在场时系统切换不再广播，面板/托盘菜单/灵动岛停在旧主题。panel 常驻
// （默认开启），在此补挂为宿主：watchSystemTheme 自带幂等标记与广播回声
// 守卫，与 dashboard-main 双挂载安全。残余限制：悬浮窗开关关闭（本窗销毁）
// 且主窗口也不在场时仍无宿主，彻底解法是把监听下沉 Rust（更新机制之外
// 的遗留项，暂不做）
watchSystemTheme();

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
