/** 订阅重置时间的展示偏好：倒计时 / 具体时间点可同时开启，
 *  仅影响界面展示，不影响采集与同步。 */
import { useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";

export type ResetDisplay = { countdown: boolean; datetime: boolean };

const STORAGE_KEY = "zbar-reset-display";

/** 展示偏好变更跨窗广播（无 payload，接收方重读 localStorage）：设置入口
 *  迁到主窗口后，两窗的展示组件（面板各 Agent 面板 + 主窗额度驾驶舱）都
 *  经 useResetDisplay 监听本事件实时重读，无需重启或重挂载 */
export const RESET_DISPLAY_CHANGED_EVENT = "zbar://reset-display-changed";

/** 默认仅倒计时（= 历史行为），升级后渲染零变化。 */
const DEFAULT_DISPLAY: ResetDisplay = {
  countdown: true,
  datetime: false,
};

function sameDisplay(a: ResetDisplay, b: ResetDisplay): boolean {
  return a.countdown === b.countdown && a.datetime === b.datetime;
}

/** 读取展示偏好；缺失或损坏字段按字段回退默认值，保证升级后行为不变。 */
export function loadResetDisplay(): ResetDisplay {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_DISPLAY };
    const parsed = JSON.parse(raw) as Partial<ResetDisplay>;
    return {
      countdown: parsed.countdown !== false,
      datetime: parsed.datetime === true,
    };
  } catch {
    return { ...DEFAULT_DISPLAY };
  }
}

/** 设置页切换后立即保存并广播（另一窗实时跟随），异常时静默降级为当前
 *  会话内状态。 */
export function saveResetDisplay(display: ResetDisplay): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(display));
  } catch {
    // 隐私模式或存储配额不足不应阻断设置页操作。
  }
  emit(RESET_DISPLAY_CHANGED_EVENT).catch(() => {
    /* 静默：另一窗最多等下次挂载重读 */
  });
}

/** 各展示组件挂载时读取一次，并监听跨窗广播实时重读（主窗口设置页改后
 *  本窗不重挂载也能即时生效；同值守卫避免值未变时多余重渲染）。
 *  监听失败静默：退化为挂载时读取一次（= 升级前行为）。 */
export function useResetDisplay(): ResetDisplay {
  const [display, setDisplay] = useState<ResetDisplay>(loadResetDisplay);
  useEffect(() => {
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen(RESET_DISPLAY_CHANGED_EVENT, () => {
      setDisplay((prev) => {
        const next = loadResetDisplay();
        return sameDisplay(prev, next) ? prev : next;
      });
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：仅失去跨窗实时性 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, []);
  return display;
}
