/**
 * 跨窗口更新桥的常量与类型（纯声明模块，零副作用、零值导入）：
 * 主窗口「关于」页不直接执行更新——Update 资源存于面板 webview 的资源表，
 * 其 rid 在主窗口无效；且主窗口若值导入 updater.ts，模块加载时的顶层
 * removeItem 会清掉面板写入的 zbar.updateReady 红点标记（两窗 localStorage
 * 同源共享）。因此更新的执行始终留在面板，主窗口只经本文件的定向事件
 * 读写状态（面板侧桥接宿主见 updaterBridge.ts）。
 *
 * 事件名沿用项目 zbar:// 前缀约定；状态字段与 src/updater.ts 的 UpdateState
 * 手写对齐（刻意不做值/类型导入，理由见上），updater.ts 改字段时需同步。
 */

/** 面板 → 主窗口：更新状态定向广播（emitTo("main-window", ...)） */
export const REMOTE_UPDATE_STATE_EVENT = "zbar://remote-update-state";

/** 主窗口 → 面板：更新操作命令（emitTo("panel", ...)） */
export const REMOTE_UPDATE_COMMAND_EVENT = "zbar://remote-update-command";

/** 主窗口可请求面板执行的命令：检查更新 / 重启安装 / 重发当前状态 */
export type RemoteUpdateCommand = "check" | "install" | "sync";

/** 更新阶段（与 src/updater.ts 的 UpdatePhase 字面一致） */
export type RemoteUpdatePhase =
  | "idle"
  | "checking"
  | "downloading"
  | "ready"
  | "uptodate"
  | "installing"
  | "error";

/** 更新状态快照（字段与 src/updater.ts 的 UpdateState 完全一致） */
export interface RemoteUpdateState {
  phase: RemoteUpdatePhase;
  progress: number;
  readyVersion: string | null;
  pendingVersion: string | null;
  releaseNote: string | null;
  error: string | null;
}
