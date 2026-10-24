import { remainingAccentColor, remainingTextColor } from "../../widgets";

/** 剩余 % → 仪表/强调色：亮色卡片压暗，暗色卡片提亮（暗底可读） */
export function quotaTone(remainingPct: number, dark: boolean): string {
  return dark ? remainingAccentColor(remainingPct) : remainingTextColor(remainingPct);
}
