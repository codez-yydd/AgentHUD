import ReactDOM from "react-dom/client";
import IslandApp from "./island/IslandApp";
import { IS_TAURI } from "./island/api";
import "./island/island.css";

// 禁用 WebView2 默认右键菜单（返回/刷新/打印等原生菜单）；
// 输入框类元素放行，保留复制/粘贴的原生菜单，与其他窗口入口对齐。
// capture 阶段 preventDefault 不影响 React 合成事件，IslandApp 岛体上的
// 自定义右键菜单照常工作，这里只补拦岛体外区域的原生菜单。
document.addEventListener(
  "contextmenu",
  (event) => {
    const target = event.target as HTMLElement | null;
    if (target?.closest("input, textarea, [contenteditable='true']")) return;
    event.preventDefault();
  },
  true
);

// 浏览器预览：加一个渐变桌面，让 CSS 玻璃有可模糊的底；Tauri 窗口保持透明。
if (!IS_TAURI) {
  document.documentElement.classList.add("island-preview");
}

// 渲染期异常兜底：HUD 是透明窗口，一旦整棵树崩了用户只会看到「岛消失」，
// 这里把错误直接画到窗口里，至少能看见发生了什么。
window.addEventListener("error", (event) => {
  const root = document.getElementById("root");
  if (!root || root.dataset.crashed === "1") return;
  root.dataset.crashed = "1";
  root.textContent = `灵动岛渲染出错: ${event.message}`;
  root.style.cssText =
    "position:fixed;inset:0;padding:8px 12px;color:#ff9b9b;font:12px/1.4 -apple-system,sans-serif;background:#1a1013;white-space:pre-wrap;overflow:auto";
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <IslandApp />
);
