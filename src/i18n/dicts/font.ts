/**
 * 界面字号词典（基准字号 / 自定义输入 / 双窗联动 / ZCode 注入条提示）。
 * 独立成文件，避免与 settings 词典互相覆盖。
 */

export const font = {
  "font.custom": "自定义",
  "font.basePx": "基准字号",
  "font.pxUnit": "px",
  "font.decrease": "减小 0.5px",
  "font.increase": "增大 0.5px",
  "font.resetDefault": "恢复默认",
  "font.preview": "实时预览",
  "font.logicalPxHint":
    "这里的 px 是界面逻辑像素：标题和核心数字更大、辅助文字稍小，整层层级同比缩放。",
  "font.invalid": "请输入 {min}～{max} 之间的数字（可精确到 0.5px）。",
  "font.range": "可调范围 {min}–{max}px，步进 0.5px",
  "font.panelTitle": "悬浮面板字号",
  "font.dashboardTitle": "主窗口字号",
  "font.panelHint":
    "调整悬浮面板的文字与控件尺寸；旧版小/标准/大/特大档位会按原比例迁移。",
  "font.dashboardHint":
    "调整主窗口的文字与控件尺寸；不改动采集、Token 计算、速度与计费口径。",
  "font.link": "与悬浮面板使用相同字号",
  "font.linkHint":
    "开启后主窗口跟随悬浮面板的基准字号；两边仍分别保存，关闭后恢复主窗口自己的设置。",
  "font.current": "当前 {px}px",
  "font.defaultValue": "默认 {px}px",
  "font.applied": "字号已应用",
};

/** 英文对照（键集必须与中文一致） */
export const fontEn: { [K in keyof typeof font]: string } = {
  "font.custom": "Custom",
  "font.basePx": "Base size",
  "font.pxUnit": "px",
  "font.decrease": "Decrease by 0.5px",
  "font.increase": "Increase by 0.5px",
  "font.resetDefault": "Reset to default",
  "font.preview": "Live preview",
  "font.logicalPxHint":
    "px here means logical UI pixels: titles and key numbers are larger, secondary text smaller, and the whole hierarchy scales together.",
  "font.invalid": "Enter a number between {min} and {max} (0.5px steps allowed).",
  "font.range": "Range {min}–{max}px, step 0.5px",
  "font.panelTitle": "Panel font size",
  "font.dashboardTitle": "Main window font size",
  "font.panelHint":
    "Adjust text and control sizes in the floating panel; legacy preset levels migrate at the same visual scale.",
  "font.dashboardHint":
    "Adjust text and control sizes in the main window; data collection, token, speed and billing calculations are unchanged.",
  "font.link": "Use the same size as the floating panel",
  "font.linkHint":
    "The main window follows the panel's base size. Both still store their own value, so turning this off restores the main window setting.",
  "font.current": "Current {px}px",
  "font.defaultValue": "Default {px}px",
  "font.applied": "Font size applied",
};
