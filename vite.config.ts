import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  // 多入口：主面板 index.html + 独立桌面宠物窗口 pet.html（宠物功能
  // 第二阶段，Rust 侧以 WebviewUrl::App("pet.html") 加载；dev 模式下
  // dev server 直接服务根目录的 pet.html，无需额外配置）+ 会话悬浮窗
  // session-hud.html（同款范式，Rust 侧以 WebviewUrl::App 加载）+ 主窗口
  // dashboard.html（管理/分析大窗，Rust 侧 main_window.rs 以
  // WebviewUrl::App("dashboard.html") 加载，同款范式）+ 灵动岛设置窗口
  // island-settings.html（920×660 透明无边框圆角小窗，14 页设置表单，
  // 自绘标题栏 / 侧栏 / 页脚，样式在 src/island-settings.css，不复用主窗口
  // db-* 外壳；Rust 侧 island_settings_window.rs 以
  // WebviewUrl::App("island-settings.html") 加载，同款范式；注意它与
  // island.html 是两个窗口：岛是透明置顶小窗，设置窗是 920×660 的独立设置窗）+
  // HUD 停靠条 hud.html（贴边常驻额度条，Rust 侧 hud_bar.rs 以
  // WebviewUrl::App 加载，同款范式）+ 托盘菜单窗 tray-menu.html（仅 macOS
  // 托盘右键弹出的自定义菜单，Rust 侧 tray_menu.rs 以 WebviewUrl::App
  // 加载，零 React 轻壳，同款范式）+ 新消息桌面玻璃提示框 notify-popup.html
  // （透明置顶小窗，尺寸紧贴卡片堆叠、空闲即隐藏；Rust 侧 island/popup.rs
  // 懒创建并以 WebviewUrl::App("notify-popup.html") 加载，同款范式）
  build: {
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        pet: fileURLToPath(new URL("./pet.html", import.meta.url)),
        "session-hud": fileURLToPath(
          new URL("./session-hud.html", import.meta.url)
        ),
        dashboard: fileURLToPath(new URL("./dashboard.html", import.meta.url)),
        hud: fileURLToPath(new URL("./hud.html", import.meta.url)),
        island: fileURLToPath(new URL("./island.html", import.meta.url)),
        // 新消息桌面玻璃提示框（label notify-popup）：Rust 侧
        // island/popup.rs 以 WebviewUrl::App("notify-popup.html") 懒加载，
        // capabilities/notify-popup.json 的 windows 白名单同源。
        "notify-popup": fileURLToPath(
          new URL("./notify-popup.html", import.meta.url)
        ),
        // 灵动岛设置独立窗口（label island-settings）。入口名与 Rust 侧
        // WebviewUrl::App("island-settings.html") 及 capabilities/
        // island-settings.json 的 windows 白名单同源，三处改名需同步。
        "island-settings": fileURLToPath(
          new URL("./island-settings.html", import.meta.url)
        ),
        shot: fileURLToPath(new URL("./shot.html", import.meta.url)),
        pin: fileURLToPath(new URL("./pin.html", import.meta.url)),
        rec: fileURLToPath(new URL("./rec.html", import.meta.url)),
        "tray-menu": fileURLToPath(
          new URL("./tray-menu.html", import.meta.url)
        ),
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      //
      // 除了 Rust 侧（Tauri 自己的 watcher 负责重编译），还要把**不参与
      // 前端构建的目录**整体排除。否则任何写文件都会被 Vite 当成"页面
      // 变更"给所有窗口发 full-reload：
      // - ios/：Xcode 构建会往 ios/agenthud/build 写上千个产物文件，
      //   逐个触发 reload，HUD 停靠条/详情卡会一直重载闪烁、dev 日志刷屏；
      // - server/：本地同步服务的 SQLite 写入同样会触发 reload；
      // - dist/、doc/：构建产物与文档截图，改了也不需要热更新。
      ignored: [
        "**/src-tauri/**",
        "**/ios/**",
        "**/server/**",
        "**/dist/**",
        "**/doc/**",
      ],
    },
  },
}));
