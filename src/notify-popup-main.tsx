/**
 * 消息玻璃提示框入口（notify-popup.html）。
 *
 * 独立 Tauri 窗口（label `notify-popup`）的页面入口，由
 * src-tauri/src/island/popup.rs 以 `WebviewUrl::App("notify-popup.html")`
 * 懒加载（首条新消息到达时才建窗，隐藏态预热与 1.2s 出卡延迟重叠）。
 *
 * 窗口形态：透明、无边框、置顶、不进任务栏、**不抢焦点**、尺寸紧贴卡片堆叠
 * （空闲即 hide）。样式只在本入口引入，不牵连岛内的 island.css。
 */

import ReactDOM from "react-dom/client";
import NotifyPopup from "./island/NotifyPopup";
import "./island/popup.css";

// 渲染期异常：浮窗是透明小窗，不能往里画错误面板（会变成一块吃点击的色块），
// 只把错误留在控制台，窗口保持透明。
window.addEventListener("error", (event) => {
  console.error("[zbar-island-popup] 渲染出错:", event.message);
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<NotifyPopup />);
