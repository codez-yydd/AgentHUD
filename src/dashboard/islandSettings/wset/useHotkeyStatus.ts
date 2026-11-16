/**
 * 全局热键注册状态：首帧查一次，保存成功后延迟 400ms 复查。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { getHotkeyStatus, type SettingsHotkeyStatus } from "./settingsApi";

/** 保存后复查注册状态的延迟（毫秒） */
export const HOTKEY_RECHECK_MS = 400;

export interface HotkeyStatusState {
  status: SettingsHotkeyStatus | null;
  loading: boolean;
  refresh: () => void;
}

export function useHotkeyStatus(saveTick: number): HotkeyStatusState {
  const [status, setStatus] = useState<SettingsHotkeyStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const refresh = useCallback(() => {
    setLoading(true);
    getHotkeyStatus()
      .then((value) => {
        if (alive.current) setStatus(value);
      })
      .catch(() => {
        /* 查询失败保持中性（null → 显示「状态未知」），不打断页面 */
        if (alive.current) setStatus(null);
      })
      .finally(() => {
        if (alive.current) setLoading(false);
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    if (!saveTick) return;
    const timer = window.setTimeout(refresh, HOTKEY_RECHECK_MS);
    return () => window.clearTimeout(timer);
  }, [saveTick, refresh]);

  return { status, loading, refresh };
}
