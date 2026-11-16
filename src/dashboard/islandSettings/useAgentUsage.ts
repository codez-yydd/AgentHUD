/**
 * 「排序」分区用的账号数据：首帧 island_quota_snapshot，之后跟
 * zbar://hud-bar-usage 推送（与灵动岛主页额度卡同一数据流，口径完全一致）。
 *
 * 为什么不在设置窗口里另起一个轮询命令：账号列表的定义（哪些 provider 展开
 * 成账号、展示名与套餐怎么来）全在 Rust 的 hud_bar 线程里，设置窗口只是
 * 换个姿势展示同一份快照；再发明一份取数路径只会让两处口径漂移。
 *
 * 排序规则照搬 IslandApp.orderedUsage：按 config.agentOrder 排名，表里没有
 * 的新账号保持数据源原顺序排在后面（排序稳定性由 Array.prototype.sort 的
 * 稳定实现保证，前端运行时都满足）。
 */
import { useEffect, useMemo, useState } from "react";
import { getQuotaSnapshot, onEvent } from "../../island/api";
import type { HudBarAgent, HudBarUsage } from "../../island/types";

/** 额度推送事件名（与 Rust 侧 hud_bar 的广播事件同一字面量） */
const HUD_BAR_USAGE_EVENT = "zbar://hud-bar-usage";

/**
 * 读取并排序账号列表。
 * @param order 用户自定义顺序（config.agentOrder）；空数组/未设置 = 数据源顺序
 * @returns 已排序的账号数组；尚未拿到快照时为空数组（分区显示空态提示）
 */
export function useOrderedAgents(order: string[] | undefined): HudBarAgent[] {
  const [usage, setUsage] = useState<HudBarUsage | null>(null);

  // 首帧补拉 + 订阅推送：onEvent 在窗口卸载 / ACL 拒绝时已静默，返回退订函数
  useEffect(() => {
    let alive = true;
    getQuotaSnapshot()
      .then((snapshot) => {
        if (alive && snapshot) setUsage(snapshot);
      })
      .catch(() => {
        /* 静默：拿不到快照时排序分区显示空态，其它分区不受影响 */
      });
    const off = onEvent<HudBarUsage>(HUD_BAR_USAGE_EVENT, (next) => {
      if (next?.agents) setUsage(next);
    });
    return () => {
      alive = false;
      off();
    };
  }, []);

  return useMemo(() => {
    const agents = usage?.agents ?? [];
    if (!order?.length) return agents;
    const rank = new Map(order.map((id, index) => [id, index]));
    // 复制后再排：不能原地修改 state 里的数组（React 之外还被事件回调持有）
    return [...agents].sort((a, b) => {
      const ra = rank.get(a.id);
      const rb = rank.get(b.id);
      if (ra == null && rb == null) return 0;
      if (ra == null) return 1;
      if (rb == null) return -1;
      return ra - rb;
    });
  }, [usage, order]);
}
