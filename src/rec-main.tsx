import ReactDOM from "react-dom/client";
import RecApp from "./rec/RecApp";
import "./rec/rec.css";

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

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<RecApp />);
