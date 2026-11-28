/**
 * 主窗口页头「多久没更新」灰字：数据最后成功刷新的相对时间——自动刷新关着
 * 的页面（请求日志选「关闭」、用量总览与周额度对比只能手动刷）在页头直接
 * 看出数据多久没更新。
 *
 * 背景：「页头精简」把常驻的「上次刷新 hh:mm:ss」收进了刷新按钮的原生提示
 * （title / aria-label，词条 dashboard.refreshLastAt）；本组件恢复一个弱化的
 * 相对时间版本（「刚刚更新」「5 分钟前更新」…），绝对时间仍在原生提示里。
 * at 为 null（还没成功取过数）时不渲染、不占位。
 *
 * 使用方（都放在 .db-page__actions 最前面：文字变长只往左长，右侧控件——
 * 包括最右的「⋯」——位置不动）：
 *  - 请求日志：自动刷新「关闭」时传 lastUpdatedAt，选了档位传 null；
 *  - 用量总览 / 周额度对比：只能手动刷新，一直传 lastUpdatedAt。
 * 只服务主窗口（悬浮面板 / 灵动岛不加载）；样式是 dashboard.css 现成的
 * .db-page__updated（12px / --db-text-dim / 不换行），相对时间文案复用
 * quota/format 的 timeAgo（词条走 dashboard.sync.*）。
 */
import { useEffect, useState } from "react";
import { useI18n } from "../i18n";
import { timeAgo } from "./quota/format";

/** 时钟步进：每 30 秒对齐一次当前时间，分钟级文字最迟 30 秒翻新 */
const CLOCK_STEP_MS = 30_000;
/** 1 分钟内算「刚刚更新」（与 timeAgo 的 min < 1 口径一致） */
const JUST_NOW_MS = 60_000;

/** 刷新时刻（本地时区 HH:mm:ss；与各页 timeText 同款），给原生提示用 */
function timeText(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export function RefreshedAgo({ at }: { at: number | null }) {
  const { t } = useI18n();
  // 分钟级时钟：只驱动本组件重渲染，整页不跟着动。窗口隐藏时灰字不可见，
  // 定时器直接停掉（主窗口关着就是隐藏态，别空转）；恢复可见立即同步一次
  // 再继续对齐 tick（src/nowClock.tsx 的 NowProvider 同款生命周期）。
  // 首帧也按当前 visibilityState 决定起不起
  const [, setTick] = useState(0);
  useEffect(() => {
    let timer: number | null = null;
    // 每 30 秒对齐到整点触发一次，触发后自重排到下一个整点（hidden 页面的
    // 定时器有节流误差，每次重排都按当前时间重新对齐，误差不累积）
    const start = () => {
      if (timer != null) return;
      timer = window.setTimeout(() => {
        timer = null;
        setTick((n) => n + 1);
        start();
      }, CLOCK_STEP_MS - (Date.now() % CLOCK_STEP_MS));
    };
    const stop = () => {
      if (timer != null) {
        window.clearTimeout(timer);
        timer = null;
      }
    };
    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        stop();
        return;
      }
      // 恢复可见：先同步一次，相对时间无陈旧残留，再继续 tick
      setTick((n) => n + 1);
      start();
    };
    if (document.visibilityState === "hidden") stop();
    else start();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      stop();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  // 钩子已全部在顶部调用完，这里才退出：没有可显示的时刻就不渲染（不占位）
  if (at == null) return null;

  const nowMs = Date.now();
  const text =
    nowMs - at < JUST_NOW_MS
      ? t("dashboard.refreshedJustNow")
      : t("dashboard.refreshedAgo", { time: timeAgo(t, at, nowMs) });
  const title = t("dashboard.refreshLastAt", { time: timeText(at) });
  return (
    <span className="db-page__updated" title={title} aria-label={title}>
      {text}
    </span>
  );
}
