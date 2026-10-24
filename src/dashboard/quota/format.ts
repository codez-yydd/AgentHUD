/**
 * 额度驾驶舱页的展示格式化小工具：窗口标签 / 相对时间 / 倒计时。
 * 与页面取数、聚合分离，供控制台、仪表卡、重置序列等子组件共用。
 */
import { formatCountdownCore } from "../../format";
import type { TFn } from "../../i18n";
import type { QuotaRisk } from "./model";

/** 窗口档位标签：已知键（含常见的 provider 特有键 credits）走词条，
 *  其余未知键原样展示 */
export function windowLabel(t: TFn, key: string): string {
  switch (key) {
    case "hour5":
      return t("dashboard.quota.win.hour5");
    case "weekly":
      return t("dashboard.quota.win.weekly");
    case "monthly":
      return t("dashboard.quota.win.monthly");
    case "cursor_auto":
      return t("dashboard.quota.win.cursor_auto");
    case "cursor_api":
      return t("dashboard.quota.win.cursor_api");
    case "mcp":
      return t("dashboard.quota.win.mcp");
    case "credits":
      return t("dashboard.quota.win.credits");
    default:
      return key;
  }
}

/** 风险标签（控制台图例 / 仪表卡风险胶囊共用文字口径） */
export function riskLabel(t: TFn, risk: QuotaRisk): string {
  switch (risk) {
    case "critical":
      return t("dashboard.quota.riskCritical");
    case "warning":
      return t("dashboard.quota.riskWarning");
    default:
      return t("dashboard.quota.riskHealthy");
  }
}

/** 相对当前时间的友好描述（词条复用 dashboard.sync.*，AccountsPage 同先例） */
export function timeAgo(
  t: TFn,
  ms: number | null | undefined,
  nowMs = Date.now()
): string {
  if (!ms) return t("dashboard.sync.never");
  const diff = Math.max(0, nowMs - ms);
  const min = Math.floor(diff / 60000);
  if (min < 1) return t("dashboard.sync.justNow");
  if (min < 60) return t("dashboard.sync.minAgo", { n: min });
  const hr = Math.floor(min / 60);
  if (hr < 24) return t("dashboard.sync.hourAgo", { n: hr });
  return t("dashboard.sync.dayAgo", { n: Math.floor(hr / 24) });
}

/** 本地时区绝对时间（YYYY-MM-DD HH:mm），用于 title 悬停 */
export function formatDateTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}`;
}

/** 距重置还有多久（"3h 05m 后"；复用 formatCountdownCore 的核心 + 词条后缀） */
export function resetCountdownText(
  t: TFn,
  resetAt: number,
  nowMs = Date.now()
): string {
  const remain = resetAt - nowMs;
  if (remain <= 0) return t("dashboard.sync.noData");
  return t("dashboard.quota.resetIn", { time: formatCountdownCore(remain) });
}

/** 重置倒计时的纯时长部分（"3h 05m"），仪表卡的小字用 */
export function countdownCore(
  resetAt: number,
  nowMs = Date.now()
): string | null {
  const remain = resetAt - nowMs;
  if (remain <= 0) return null;
  return formatCountdownCore(remain);
}
