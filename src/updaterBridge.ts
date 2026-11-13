/**
 * 面板侧更新桥宿主：把面板 updater.ts 的状态变化定向广播给主窗口，并代
 * 主窗口执行更新命令。面板是唯一更新执行者——Update 资源存于面板 webview
 * 的资源表，其 rid 在主窗口无效，且主窗口值导入 updater.ts 会在模块加载
 * 时清掉面板写入的 zbar.updateReady 红点标记（两窗 localStorage 同源共享）；
 * 主窗口「关于」页只经跨窗口事件桥（updaterEvents.ts）读写状态，updater.ts
 * 本体零改动，桥接是纯增强，所有失败静默、不阻断面板自身的更新链路。
 *
 * 仅面板加载（App.tsx 挂载 startUpdaterBridgeHost）；主窗口任何文件不得
 * 导入本模块（它值依赖 updater.ts，见上）。
 */
import { emitTo, listen } from "@tauri-apps/api/event";
import {
  checkAndDownloadInBackground,
  getUpdateState,
  installPendingUpdate,
  UPDATE_STATE_EVENT,
} from "./updater";
import {
  REMOTE_UPDATE_COMMAND_EVENT,
  REMOTE_UPDATE_STATE_EVENT,
  type RemoteUpdateCommand,
} from "./updaterEvents";

/** 主窗口 label（与 capabilities/dashboard.json 的 windows 白名单一致） */
const MAIN_WINDOW_TARGET = "main-window";

/**
 * 启动更新桥：监听本窗 UPDATE_STATE_EVENT（updater.ts 每次 setState 都会
 * dispatch）→ 序列化去重后 emitTo 定向主窗口；同时受理主窗口的更新命令。
 * 返回清理函数（unlisten + removeEventListener）。
 */
export function startUpdaterBridgeHost(): () => void {
  // 上次广播的序列化快照：下载进度每个 chunk 都会触发 UPDATE_STATE_EVENT，
  // 不去重会对主窗口高频 emitTo；状态实际变化才发
  let lastSerialized = "";

  const broadcast = () => {
    const state = getUpdateState();
    const serialized = JSON.stringify(state);
    if (serialized === lastSerialized) return;
    lastSerialized = serialized;
    // 定向 main-window，不全局 emit（避免打扰宠物 / Session HUD 等窗口）；
    // 失败静默：桥接是增强，不影响面板本窗的更新流程
    emitTo(MAIN_WINDOW_TARGET, REMOTE_UPDATE_STATE_EVENT, state).catch(
      () => {}
    );
  };

  // 挂载即广播一次当前状态（主窗口可能晚于面板启动才进入「关于」页，
  // 那条路径由 sync 命令兜底；这里覆盖主窗口先开面板后装桥的时序）
  broadcast();

  window.addEventListener(UPDATE_STATE_EVENT, broadcast);

  let disposed = false;
  let unlistenCommand: (() => void) | undefined;
  // payload 泛型标注为命令联合类型：switch 分支即编译期校验的合法命令值
  listen<RemoteUpdateCommand>(REMOTE_UPDATE_COMMAND_EVENT, (event) => {
    switch (event.payload) {
      case "check":
        // 与面板「检查更新」按钮同语义（非静默：发现异常会置 error 态，
        // 经状态广播回流给主窗口展示）
        void checkAndDownloadInBackground({ silent: false });
        break;
      case "install":
        // installPendingUpdate 失败时已置 error 态并广播，这里只吞 rejection
        installPendingUpdate().catch(() => {});
        break;
      case "sync":
        // 主窗口关于页挂载时请求初始状态：清掉去重指纹强制重发当前状态
        lastSerialized = "";
        broadcast();
        break;
    }
  })
    .then((unlisten) => {
      // 清理已发生（StrictMode 双挂载 / 快速卸载）时立即退订
      if (disposed) unlisten();
      else unlistenCommand = unlisten;
    })
    .catch(() => {
      /* 静默：桥接收令不可用时主窗口按钮退化为无响应，面板链路不受影响 */
    });

  return () => {
    disposed = true;
    unlistenCommand?.();
    window.removeEventListener(UPDATE_STATE_EVENT, broadcast);
  };
}
