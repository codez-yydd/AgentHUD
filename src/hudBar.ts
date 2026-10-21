/**
 * HUD 停靠条与主面板之间的桥：顶部工具栏开关按钮 + 托盘/面板状态同步。
 *
 * 状态真相在 Rust（~/.agenthud/hud-bar.json 的 enabled），本模块只做两件事：
 * - 启动时主动拉一次 `hud_bar_state`，避免按钮先按 false 渲染；
 * - 点击时调用 `hud_bar_set_enabled`（Rust 在主线程建窗/隐藏，随后广播
 *   `zbar://hud-bar-state`）。托盘右键菜单、异常销毁等其它入口也会广播
 *   同一事件，面板因此永远不需要自己维护第二份开关状态。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** HUD 开关状态事件（Rust hud_bar::HUD_BAR_STATE_EVENT 同步） */
export const HUD_BAR_STATE_EVENT = "zbar://hud-bar-state";

/** hud_bar_state 命令回执（scale/stretch 面板暂未使用，留作诊断/后续设置项） */
export interface HudBarState {
  enabled: boolean;
  scale: number;
  stretch: number;
}

/** 读取当前 HUD 开关（启动时对齐按钮高亮） */
export function getHudBarState(): Promise<HudBarState> {
  return invoke<HudBarState>("hud_bar_state");
}

/** 开关 HUD；返回落定后的开启态（失败时抛错，由调用方回滚乐观状态） */
export function setHudBarEnabled(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("hud_bar_set_enabled", { enabled });
}

/** 订阅 HUD 开关状态事件；返回取消订阅函数 */
export function onHudBarState(
  handler: (enabled: boolean) => void
): Promise<UnlistenFn> {
  return listen<{ enabled?: boolean }>(HUD_BAR_STATE_EVENT, (e) => {
    if (typeof e.payload?.enabled === "boolean") handler(e.payload.enabled);
  });
}
