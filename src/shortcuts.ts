/**
 * 快捷键文案的平台适配。
 *
 * 两套映射不能混用：
 * - `shortcutText`：应用内快捷键（复制/保存/撤销…）。macOS 上这些动作习惯用
 *   Command，代码里 `event.metaKey || event.ctrlKey` 也都认，所以显示成 ⌘C。
 * - `hotkeyText`：全局热键（形如 "Ctrl+Alt+A"，注册给 global-shortcut 插件的
 *   真实按键）。macOS 上 Ctrl 就是 Control，必须显示成 ⌃，不能换成 ⌘。
 */

export const IS_MAC =
  typeof navigator !== "undefined" && /Mac OS X|Macintosh/.test(navigator.userAgent);

/** 应用内：Ctrl → ⌘ */
const APP_MAC: Record<string, string> = {
  Ctrl: "⌘",
  Control: "⌘",
  Cmd: "⌘",
  Command: "⌘",
  Meta: "⌘",
  Alt: "⌥",
  Option: "⌥",
  Shift: "⇧",
};

/** 全局热键：Ctrl → ⌃（真实按下的就是 Control） */
const HOTKEY_MAC: Record<string, string> = {
  ...APP_MAC,
  Ctrl: "⌃",
  Control: "⌃",
};

function format(combo: string, mac: Record<string, string>): string {
  const trimmed = combo.trim();
  if (!IS_MAC) return trimmed;
  return trimmed
    .split("+")
    .map((part) => mac[part.trim()] ?? part.trim())
    .join("");
}

/** 应用内快捷键的展示文本：macOS `⌘C`，其它平台 `Ctrl+C`。 */
export function shortcutText(combo: string): string {
  return format(combo, APP_MAC);
}

/** 全局热键的展示文本：macOS `⌃⌥A`，其它平台 `Ctrl+Alt+A`。 */
export function hotkeyText(combo: string): string {
  return format(combo, HOTKEY_MAC);
}

/** 快捷键组合里的按键片段，Windows 上每个键单独一个 <kbd> 更好看。 */
export function shortcutParts(combo: string): string[] {
  if (IS_MAC) return [shortcutText(combo)];
  return combo.split("+").map((part) => part.trim());
}
