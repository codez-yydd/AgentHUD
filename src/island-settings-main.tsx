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

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsWindow />
  </React.StrictMode>
);
