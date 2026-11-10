import ReactDOM from "react-dom/client";
import IslandApp from "./island/IslandApp";
import { IS_TAURI } from "./island/api";
import "./island/island.css";

// 浏览器预览：加一个渐变桌面，让 CSS 玻璃有可模糊的底；Tauri 窗口保持透明。
if (!IS_TAURI) {
  document.documentElement.classList.add("island-preview");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <IslandApp />
);
