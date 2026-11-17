import React from "react";
import ReactDOM from "react-dom/client";
import { bootstrapTheme } from "./appearance";
import { applyFontSettings } from "./fontSettings";
import { SettingsWindow } from "./dashboard/islandSettings/SettingsWindow";
import "./island-settings.css";

/**
 * 灵动岛设置窗口（island-settings.html）入口。
 */
bootstrapTheme();
applyFontSettings();

// 禁用 WebView2 默认右键菜单（返回/刷新/打印等原生菜单）；
// 输入框类元素放行，保留复制/粘贴的原生菜单，与 IslandApp 的右键逻辑对齐。
document.addEventListener(
  "contextmenu",
  (event) => {
    const target = event.target as HTMLElement | null;
    if (target?.closest("input, textarea, [contenteditable='true']")) return;
    event.preventDefault();
  },
  true
);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsWindow />
  </React.StrictMode>
);
