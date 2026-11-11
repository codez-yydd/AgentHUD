/**
 * 「会话」页取数：实时会话账本 + 单会话上下文水位。
 *
 * 账本 15 秒自动刷新，但**只在窗口可见时**跑（后台挂着不重复扫 7 个来源库；
 * 与请求日志页同款取数成本）。上下文是离散的"点开才查"，切会话 / 换窗口
 * 才重新取，不参与轮询。
 *
 * 两个 hook 都用「请求序号」丢弃过期响应：连点刷新、快速切窗口时，先发后至
 * 的旧响应不会覆盖新数据（与 useLogsData 的做法一致）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { getLiveSessions, getSessionContext } from "../../api";
import type {
  RequestLogSource,
  SessionContext,
  SessionFeed,
} from "../../types";

/** 账本自动刷新间隔（毫秒） */
const REFRESH_MS = 15_000;

/** 会话选择器的最小标识（来源 + 会话 id 才能唯一定位一个会话） */
export interface SessionTarget {
  source: RequestLogSource;
  sessionId: string;
}

export interface SessionsFeedState {
  feed: SessionFeed | null;
  /** 首载中（还没有任何数据） */
  loading: boolean;
  /** 已有数据、正在后台刷新（表格不闪骨架） */
  refreshing: boolean;
  error: string | null;
  /** 数据自身的取数时刻（后端 generated_at，不是前端收到响应的时刻） */
  lastUpdatedAt: number | null;
  reload: () => void;
}

export function useSessionsFeed(windowHours: number): SessionsFeedState {
  const [feed, setFeed] = useState<SessionFeed | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(null);
  const requestId = useRef(0);
  /** 是否已有可用数据（决定首载骨架还是后台刷新态；避免把 feed 放进依赖） */
  const hasData = useRef(false);

  const reload = useCallback(() => {
    const id = ++requestId.current;
    if (hasData.current) setRefreshing(true);
    else setLoading(true);
    setError(null);
    getLiveSessions(windowHours)
      .then((next) => {
        if (id !== requestId.current) return;
        hasData.current = true;
        setFeed(next);
        setLastUpdatedAt(next.generated_at);
      })
      .catch((err: unknown) => {
        if (id !== requestId.current) return;
        // 后台刷新失败保留旧数据（页面降级为「上次的数据 + 错误条」）
        setError(String(err));
      })
      .finally(() => {
        if (id !== requestId.current) return;
        setLoading(false);
        setRefreshing(false);
      });
  }, [windowHours]);

  useEffect(() => {
    reload();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") reload();
    }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [reload]);

  return { feed, loading, refreshing, error, lastUpdatedAt, reload };
}

export interface SessionContextState {
  context: SessionContext | null;
  loading: boolean;
  error: string | null;
}

export function useSessionContext(
  target: SessionTarget | null,
  windowHours: number
): SessionContextState {
  const [context, setContext] = useState<SessionContext | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const source = target?.source;
  const sessionId = target?.sessionId;

  useEffect(() => {
    const id = ++requestId.current;
    if (!source || sessionId == null) {
      setContext(null);
      setLoading(false);
      setError(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    getSessionContext(source, sessionId, windowHours)
      .then((next) => {
        if (cancelled || id !== requestId.current) return;
        setContext(next);
      })
      .catch((err: unknown) => {
        if (cancelled || id !== requestId.current) return;
        setContext(null);
        setError(String(err));
      })
      .finally(() => {
        if (cancelled || id !== requestId.current) return;
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [source, sessionId, windowHours]);

  return { context, loading, error };
}
