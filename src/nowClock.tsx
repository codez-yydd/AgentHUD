/**
 * panel 共享 1s 时钟 context（内存压降专项 6，照搬 src/island/ui.tsx 的
 * NowProvider 范式）。
 *
 * 原先 10 处面板组件（QuotaPanel / SummaryTab / ModelSpeedPanel /
 * AgentUsagePanel / ClaudePanel / CursorPanel / GenericQuotaPanel /
 * MinimaxPanel / OpencodePanel / ProviderAccountsSection）各自挂一个
 * 1s setInterval 驱动倒计时显示，同一窗口每秒触发 10 次独立的 React
 * 重渲染。收敛为单一 Provider 后每秒只有一次 context 广播、一次树级
 * 重渲染。
 *
 * 窗口 hidden 时暂停 tick：面板窗隐藏（visible:false 出生、被用户挂起）
 * 时倒计时不可见，tick 只是空转重渲染；恢复可见立即同步一次再继续。
 * 原先个别组件用「有重置数据才启定时器」的门槛（hasResets），共享时钟
 * 下定时器常驻但全局只剩这一个，门槛不再必要——无重置数据的组件拿到的
 * now 照常更新，仅多几次幂等重渲染，换来整体 tick 数量 10 → 1。
 */

import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

const NowContext = createContext(Date.now());

export function NowProvider({ children }: { children: ReactNode }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    let timer: number | null = null;
    const start = () => {
      if (timer != null) return;
      timer = window.setInterval(() => setNow(Date.now()), 1000);
    };
    const stop = () => {
      if (timer != null) {
        window.clearInterval(timer);
        timer = null;
      }
    };
    const onVisible = () => {
      if (document.visibilityState === "hidden") {
        stop();
        return;
      }
      // 恢复可见：先同步一次，倒计时无陈旧残留，再继续 tick
      setNow(Date.now());
      start();
    };
    if (document.visibilityState === "hidden") stop();
    else start();
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      stop();
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);
  return <NowContext.Provider value={now}>{children}</NowContext.Provider>;
}

export function useNow(): number {
  return useContext(NowContext);
}
