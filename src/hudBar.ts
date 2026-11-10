/**
 * 灵动岛 HUD 与主面板之间的桥：顶部工具栏开关按钮 + 面板/键盘状态同步。
 *
 * 状态真相在 Rust（内部 SQLite 配置库的 enabled），本模块只做两件事：
 * - 启动时调用 `island_state` 拉一次，避免按钮先按 false 渲染；
 * - 点击时调用 `island_set_enabled`（Rust 在主线程建窗/隐藏，随后广播
 *   `zbar://island-state`）。托盘/其它入口也会广播同一事件，面板因此永远
 *   不需要自己维护第二份开关状态。
 *
 * 旧版贴边停靠条（hud-bar）的配置文件与模块仍保留，但灵动岛默认接管
 * HUD（replaceLegacyBar），面板按钮已改为控制灵动岛。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** 灵动岛开关状态事件（Rust island::set_enabled_on_main 广播） */
export const HUD_BAR_STATE_EVENT = "zbar://island-state";

/** 面板按钮需要的开关快照（scale/stretch 仅为兼容旧调用点保留） */
export interface HudBarState {
  enabled: boolean;
  scale: number;
  stretch: number;
}

/** 读取当前灵动岛开关（启动时对齐按钮高亮） */
export async function getHudBarState(): Promise<HudBarState> {
  const state = await invoke<{ enabled: boolean }>("island_state");
  return { enabled: Boolean(state?.enabled), scale: 1, stretch: 1 };
}

/** 开关灵动岛；返回落定后的开启态（失败时抛错，由调用方回滚乐观状态） */
export function setHudBarEnabled(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("island_set_enabled", { enabled });
}

/** 订阅灵动岛开关状态事件；返回取消订阅函数 */
export function onHudBarState(
  handler: (enabled: boolean) => void
): Promise<UnlistenFn> {
  return listen<{ enabled?: boolean }>(HUD_BAR_STATE_EVENT, (e) => {
    if (typeof e.payload?.enabled === "boolean") handler(e.payload.enabled);
  });
}
