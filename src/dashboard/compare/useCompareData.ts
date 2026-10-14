/**
 * 周额度对比页取数 hook：渐进式拉取「本机 + 远端（可选设备）」的额度采样与
 * Token 用量，并交给 compare/model 按窄窗同一口径合并。
 *
 * 设计要点（对齐窄窗 ComparePanel 的数据流）：
 *  - 额度历史固定取近 90 天（COMPARE_HISTORY_DAYS）：矩阵的「当前剩余」
 *    不随页头范围缩水，范围切换只重新切片折线；
 *  - 分级加载（页面不再整页转圈）：
 *      ① 本地额度快照 + 本地 Token 就绪即发布——矩阵卡与趋势卡先出本机部分
 *         （本地 Token 仍一次取回全部范围预设）；
 *      ② 远端快照 / 远端 Agent 快照到达后补全矩阵与趋势；
 *      ③ Token 对比卡的远端部分按当前范围懒取——首载只取当前选中范围的
 *         远端 usage（hour 桶），切到未取过的范围才取该范围并缓存，
 *         已有范围不重取；
 *    各卡据此展示独立骨架，页面级 loading 只在首个结果到达前出现；
 *  - 同步配置与本地取数并行发起，只有远端部分等待配置（避免「先按未启用同步
 *    的本地口径取一遍、拿到设备配置后又全量重取」的双取）；getSyncConfig 失败
 *    或超时（SYNC_CONFIG_TIMEOUT_MS）按未启用同步处理，不卡首屏；
 *  - 设备筛选沿用窄窗语义：all = 本机 + 其他设备（排除本机自身）、
 *    local = 仅本机、具体 device_id = 仅该设备；同步未启用时强制回落本机；
 *  - 各来源独立容错：单个 Agent 的 Token 取数失败按 0 处理（矩阵与折线不受影响），
 *    远端历史在汇总模式下降级为「仅本机」并置 remoteDegraded（弱提示条），
 *    指定设备时明确报错；
 *  - 模块级缓存：最近一次完整结果 + 设备筛选 + 生成它的中间态一起存在模块
 *    顶层变量里，再次进入页面立即用缓存渲染（无加载态，切范围懒取照常可用）；
 *    缓存超过 60 秒才后台静默刷新，手动刷新永远强制重取；generation 序号守卫：
 *    筛选变化 / 刷新 / 卸载后的旧结果一律丢弃。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AccountMeta,
  AgentQuotaSnapshot,
  DeviceInfo,
  QuotaSnapshot,
  RemoteAgentQuotaSnapshot,
  RemoteSnapshot,
  RemoteUsage,
  SyncConfig,
} from "../../types";
import {
  getAgentQuotaHistory,
  getCompareTokensForAgent,
  getQuotaHistory,
  getSyncConfig,
  listAccounts,
  listRemoteDevices,
  remoteAgentQuotaSnapshots,
  remoteSnapshots,
  remoteUsage,
} from "../../api";
import { loadAgentVisibility } from "../../agentVisibility";
import { mergeAgentQuotaSnapshots } from "../../agentQuota";
import { useI18n } from "../../i18n";
import {
  COMPARE_AGENTS,
  COMPARE_RANGES,
  LOCAL_COMPARE_SOURCE,
  agentColorOf,
  agentLabelOf,
  bucketTotalsByStart,
  buildQuotaTimelines,
  historyStartMs,
  mergeQuotaSnapshots,
  rangeStartMs,
  remoteSourceOf,
  sumBuckets,
  sumRemoteUsageIn,
  toQuotaSnapshot,
  type CompareAgent,
  type CompareRange,
  type QuotaTimeline,
  type TokenComparison,
} from "./model";

/** 同步配置等待上限：超时按「未启用同步」回退本地口径单次加载（不卡首屏） */
const SYNC_CONFIG_TIMEOUT_MS = 5000;

/** 缓存新鲜期：超过则在后台静默刷新（不阻塞界面，完成后替换数据） */
const CACHE_FRESH_MS = 60_000;

/** 某来源在某范围下的 Token 合计（本机 / 远端各一份） */
interface TokenTotals {
  localTokens: number;
  localRequests: number;
  remoteTokens: number;
  remoteRequests: number;
}

/** 页面取数结果 */
export interface CompareRaw {
  /** 各额度系列的采样时间线（近 90 天，范围切片在视图层做；远端到达后补全） */
  timelines: QuotaTimeline[];
  /** 各范围下的 Agent Token 对比（本机部分一次取全；远端按当前范围懒取，
   *  未取过的范围可能缺键） */
  tokensByRange: Partial<Record<CompareRange, TokenComparison[]>>;
}

export interface CompareDataState {
  data: CompareRaw | null;
  /** 首个结果（本地额度历史）到达前的整页加载态 */
  loading: boolean;
  /** 额度矩阵 / 趋势的本机数据未就绪（卡片骨架用） */
  quotaPending: boolean;
  /** 当前范围的 Token 数据未就绪（卡片骨架用） */
  tokensPending: boolean;
  /** 远端快照仍在合并（弱标记用） */
  remotePending: boolean;
  /** 致命错误（全部数据都拿不到）：远端历史读取失败 / 本机取数抛错 */
  error: string | null;
  /** 汇总模式下远端历史失败：已降级为仅本机口径（弱提示条用） */
  remoteDegraded: boolean;
  lastUpdatedAt: number | null;
  /** 同步配置（null = 尚未读到；页面据此决定是否展示设备筛选） */
  syncConfig: SyncConfig | null;
  /** 可选的远端设备（排除本机自身） */
  devices: DeviceInfo[];
  /** 当前设备筛选：all / local / 具体 device_id */
  deviceFilter: string;
  setDeviceFilter: (value: string) => void;
  reload: () => void;
}

/**
 * 单 generation 的中间结果：本地与远端各阶段往里累加，发布时整体重建视图。
 * generation 切换（筛选变化 / 手动刷新 / 卸载）后旧对象的后续写入不再被读取。
 */
interface Generation {
  gen: number;
  nowMs: number;
  historyFromMs: number;
  enabledAgents: CompareAgent[];
  wantLocal: boolean;
  deviceFilter: string;
  zaiHistory: QuotaSnapshot[];
  remoteHistory: RemoteSnapshot[];
  agentHistory: AgentQuotaSnapshot[];
  remoteAgentHistory: RemoteAgentQuotaSnapshot[];
  accounts: AccountMeta[] | null;
  /** 本机各 Agent：范围起点 → 合计（一次请求带回全部范围预设） */
  localTotals: Map<CompareAgent, Map<number, { tokens: number; requests: number }>>;
  /** 远端 usage 缓存：`${agent}|${range}` → 桶数据（懒取，已有范围不重取） */
  remoteUsage: Map<string, RemoteUsage>;
  /** 在途的远端范围请求（去重，避免同一范围并发重复请求） */
  remoteInflight: Set<string>;
  syncConfig: SyncConfig | null;
  degraded: boolean;
  /** 该 generation 是否还需要远端数据（同步开启且筛选含远端） */
  remoteWanted: boolean;
  /** 清理定时器（配置读取超时兜底） */
  cleanup: (() => void) | null;
}

/** 模块级结果缓存（含设备筛选、设备列表与 generation 中间态）：切页（条件
 *  渲染 = 卸载）后重进即刻渲染，无加载态；中间态让命中缓存后的范围切换仍能
 *  懒取远端补全（否则重建视图时会丢掉本机部分）。只存在于本窗口的 JS 模块
 *  内存，不落盘。 */
let compareCache: {
  data: CompareRaw;
  at: number;
  deviceFilter: string;
  /** 设备筛选选项（同步开启时缓存，命中缓存后筛选下拉不闪空） */
  devices: DeviceInfo[];
  state: Generation;
} | null = null;

/** 错误信息截断（与总览页同款：长中文提示只展示摘要） */
function shortError(error: unknown): string {
  const text = String(error).replace(/^Error:\s*/, "");
  return text.length > 120 ? text.slice(0, 117) + "…" : text;
}

/** 远端 usage 缓存键 */
function remoteKey(agent: CompareAgent, range: CompareRange): string {
  return `${agent}|${range}`;
}

/** 本机某 Agent 在全部范围预设下的 Token 合计（一次请求带回全部区间）。 */
async function loadLocalTokenTotals(
  agent: CompareAgent,
  rangeFroms: number[],
  nowMs: number
): Promise<Map<number, { tokens: number; requests: number }>> {
  if (agent === "cursor") {
    // Cursor 后端把事件归属到「首个命中的区间」（cursor.rs 用 position
    // 匹配），嵌套区间会互相截断（7 天的用量不会再计入 30/90 天桶），
    // 因此把三个嵌套范围改写成互不重叠的相邻区间一次取回
    // （[90d,30d) + [30d,7d) + [7d,now)），再按「区间起点 ≥ 范围起点」
    // 累加还原各范围合计：90d = 三段和、30d = 后两段和、7d = 末段。
    // 其余来源都是逐区间独立聚合，可以直接传嵌套区间。
    const ascending = [...rangeFroms].sort((a, b) => a - b);
    const segments: [number, number][] = ascending
      .slice(1)
      .map((from, index) => [ascending[index], from]);
    segments.push([ascending[ascending.length - 1], nowMs]);
    const buckets = await getCompareTokensForAgent(
      LOCAL_COMPARE_SOURCE[agent],
      segments
    );
    const totals = new Map<number, { tokens: number; requests: number }>();
    for (const from of rangeFroms) {
      totals.set(
        from,
        sumBuckets(buckets.filter((bucket) => bucket.reset_at >= from))
      );
    }
    return totals;
  }
  // 传的是后端来源名（opencodego → "opencode"），不是 AgentId
  const periods: [number, number][] = rangeFroms.map((from) => [from, nowMs]);
  return bucketTotalsByStart(
    await getCompareTokensForAgent(LOCAL_COMPARE_SOURCE[agent], periods)
  );
}

export function useCompareData(range: CompareRange): CompareDataState {
  const { t } = useI18n();
  const [data, setData] = useState<CompareRaw | null>(null);
  const [loading, setLoading] = useState(true);
  const [quotaPending, setQuotaPending] = useState(true);
  const [tokensPending, setTokensPending] = useState(true);
  const [remotePending, setRemotePending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [remoteDegraded, setRemoteDegraded] = useState(false);
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(null);
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [deviceFilter, setDeviceFilter] = useState<string>("all");
  const [nonce, setNonce] = useState(0);

  // generation 守卫：筛选 / 刷新 / 卸载后旧结果一律丢弃
  const genRef = useRef(0);
  const genStateRef = useRef<Generation | null>(null);
  // 手动刷新标记：强制重取（跳过缓存）
  const manualRef = useRef(false);
  // 当前页头范围（懒取远端 usage 用；避免把 range 塞进 generation 依赖）
  const rangeRef = useRef(range);
  // 最近一次发布的结果（缓存写入用）
  const publishedRef = useRef<CompareRaw | null>(null);
  // 设备列表（写进缓存，命中缓存后筛选下拉不闪空；不参与 generation 依赖）
  const devicesRef = useRef<DeviceInfo[]>([]);
  const applyDevices = useCallback((list: DeviceInfo[]) => {
    devicesRef.current = list;
    setDevices(list);
  }, []);

  /** 用当前 generation 的中间结果重建视图并发布（各阶段就绪即调用） */
  const publish = useCallback(
    (g: Generation) => {
      const mergedZai = mergeQuotaSnapshots(
        g.zaiHistory,
        g.remoteHistory.map(toQuotaSnapshot)
      );
      const mergedAgent = mergeAgentQuotaSnapshots(
        g.agentHistory,
        g.remoteAgentHistory
      );
      const timelines = buildQuotaTimelines(
        mergedZai,
        mergedAgent,
        g.enabledAgents,
        g.accounts,
        t
      );

      // Token 口径沿用「按 Agent 实际用量」：本机按来源名聚合区间，
      // 远端按桶与范围的区间重叠求和；两者相加即合并结果。
      const tokensByRange: Partial<Record<CompareRange, TokenComparison[]>> = {};
      for (const rangeKey of COMPARE_RANGES) {
        const fromMs = rangeStartMs(rangeKey, g.nowMs);
        const rows: TokenComparison[] = [];
        for (const agent of g.enabledAgents) {
          const local = g.localTotals.get(agent)?.get(fromMs) ?? {
            tokens: 0,
            requests: 0,
          };
          const remote = g.remoteUsage.get(remoteKey(agent, rangeKey));
          const remoteSum = remote
            ? sumRemoteUsageIn(remote, fromMs, g.nowMs)
            : { tokens: 0, requests: 0 };
          const totals: TokenTotals = {
            localTokens: local.tokens,
            localRequests: local.requests,
            remoteTokens: remoteSum.tokens,
            remoteRequests: remoteSum.requests,
          };
          if (
            totals.localTokens + totals.remoteTokens > 0 ||
            totals.localRequests + totals.remoteRequests > 0
          ) {
            rows.push({
              agent,
              label: agentLabelOf(agent),
              color: agentColorOf(agent),
              ...totals,
            });
          }
        }
        rows.sort(
          (a, b) =>
            b.localTokens +
            b.remoteTokens -
            (a.localTokens + a.remoteTokens)
        );
        tokensByRange[rangeKey] = rows;
      }

      const next: CompareRaw = { timelines, tokensByRange };
      publishedRef.current = next;
      setData(next);
    },
    [t]
  );

  /**
   * 懒取某范围的远端 usage（hour 桶）：已取过的范围直接命中缓存不重取；
   * 未启用同步 / 该 Agent 未登记远端来源时无事发生。就绪后重建视图（补上该
   * 范围的远端贡献）。仅本机口径的筛选下，该范围的 Token 数据在远端到达前
   * 保持骨架态。
   */
  const ensureRemoteRange = useCallback(
    async (rangeKey: CompareRange): Promise<void> => {
      const g = genStateRef.current;
      if (!g || !g.remoteWanted || !g.syncConfig) return;
      const config = g.syncConfig;
      const targets = g.enabledAgents.filter((agent) => remoteSourceOf(agent));
      const pending = targets.filter(
        (agent) => !g.remoteUsage.has(remoteKey(agent, rangeKey))
      );
      if (pending.length === 0) return;
      // 具体设备筛选下本机 Token 不取数：该范围只能等远端到达，期间给骨架
      if (!g.wantLocal) setTokensPending(true);
      await Promise.all(
        pending.map(async (agent) => {
          const key = remoteKey(agent, rangeKey);
          if (g.remoteInflight.has(key)) return;
          g.remoteInflight.add(key);
          try {
            const remote = await remoteUsage(
              rangeStartMs(rangeKey, g.nowMs),
              g.nowMs,
              "hour",
              {
                excludeDevice:
                  g.deviceFilter === "all" ? config.device_id : undefined,
                devices: g.deviceFilter === "all" ? undefined : g.deviceFilter,
                source: remoteSourceOf(agent),
              }
            );
            if (genRef.current !== g.gen) return;
            g.remoteUsage.set(key, remote);
          } catch {
            // 远端没有该来源或服务暂不可用时，不影响其他 Agent 展示。
          } finally {
            g.remoteInflight.delete(key);
          }
        })
      );
      if (genRef.current !== g.gen) return;
      publish(g);
      setTokensPending(false);
    },
    [publish]
  );

  // 主取数：本地阶段 + 远端阶段（配置就绪后），各自就绪即发布
  const run = useCallback(
    async (): Promise<void> => {
      const gen = ++genRef.current;
      genStateRef.current?.cleanup?.();
      const isCurrent = () => gen === genRef.current;

      const nowMs = Date.now();
      const historyFromMs = historyStartMs(nowMs);
      const rangeFroms = COMPARE_RANGES.map((item) => rangeStartMs(item, nowMs));
      const visibility = loadAgentVisibility();
      const enabledAgents = COMPARE_AGENTS.filter((agent) => visibility[agent]);
      const wantLocal = deviceFilter === "all" || deviceFilter === "local";
      const hasZai = enabledAgents.includes("zai");

      const g: Generation = {
        gen,
        nowMs,
        historyFromMs,
        enabledAgents,
        wantLocal,
        deviceFilter,
        zaiHistory: [],
        remoteHistory: [],
        agentHistory: [],
        remoteAgentHistory: [],
        accounts: null,
        localTotals: new Map(),
        remoteUsage: new Map(),
        remoteInflight: new Set(),
        syncConfig: null,
        degraded: false,
        remoteWanted: false,
        cleanup: null,
      };
      genStateRef.current = g;

      setError(null);
      setRemoteDegraded(false);
      setQuotaPending(true);
      setTokensPending(true);

      // 同步配置与本地取数并行发起：仅远端阶段等待它（读取失败 / 超时按
      // 「未启用同步」处理，本地口径照常出数，不卡首屏）
      let syncTimer: number | null = null;
      const syncPromise = Promise.race([
        getSyncConfig().catch(() => null),
        new Promise<null>((resolve) => {
          syncTimer = window.setTimeout(() => resolve(null), SYNC_CONFIG_TIMEOUT_MS);
        }),
      ]);
      g.cleanup = () => {
        if (syncTimer != null) window.clearTimeout(syncTimer);
      };
      syncPromise.then((config) => {
        if (syncTimer != null) window.clearTimeout(syncTimer);
        if (!isCurrent()) return;
        setSyncConfig(config);
        if (config?.enabled && config.device_token) {
          listRemoteDevices()
            .then((list) => {
              if (isCurrent()) applyDevices(list);
            })
            .catch(() => {
              /* 静默：设备列表读不到时筛选只留本机口径 */
            });
        }
      });

      // ===== ① 本地：额度历史（本机）+ 本地 Token（全部范围一次取回）=====
      const localTasks: Promise<unknown>[] = [];
      if (wantLocal) {
        // all=true：本机历史含全部账号的快照，按指纹分组正是要利用它
        localTasks.push(
          getQuotaHistory(true, historyFromMs).then((history) => {
            g.zaiHistory = history;
          })
        );
        localTasks.push(
          getAgentQuotaHistory(historyFromMs, nowMs)
            .then((history) => {
              g.agentHistory = history;
            })
            .catch(() => {
              // Agent 历史读取失败不应阻断其余订阅的对比。
            })
        );
        for (const agent of enabledAgents) {
          localTasks.push(
            loadLocalTokenTotals(agent, rangeFroms, nowMs)
              .then((totals) => {
                g.localTotals.set(agent, totals);
              })
              .catch(() => {
                // 单个来源失败按 0 处理，不影响其余 Agent。
              })
          );
        }
      }
      if (hasZai) {
        localTasks.push(
          listAccounts()
            .then((state) => {
              g.accounts = state.accounts;
            })
            .catch(() => {
              // 账号列表读取失败时系列标签退化为指纹前 6 位。
            })
        );
      }
      try {
        await Promise.all(localTasks);
      } catch (e) {
        // 本机历史读取是整页数据的根基：失败即整页错误态（远端不必再取）
        if (isCurrent()) {
          setError(shortError(e));
          setLoading(false);
          setQuotaPending(false);
          setTokensPending(false);
          setRemotePending(false);
        }
        return;
      }
      if (!isCurrent()) return;
      publish(g);
      setLastUpdatedAt(Date.now());
      setQuotaPending(!wantLocal);
      setTokensPending(!wantLocal);
      setLoading(false);

      // ===== ②③ 远端：快照补全矩阵与趋势；当前范围 remote usage 并行懒取 =====
      const config = await syncPromise;
      g.cleanup = null;
      if (!isCurrent()) return;
      const syncEnabled = !!config?.enabled && !!config.device_token;
      if (!syncEnabled || deviceFilter === "local") {
        setQuotaPending(false);
        setTokensPending(false);
        setRemotePending(false);
        compareCache = {
          data: publishedRef.current ?? { timelines: [], tokensByRange: {} },
          at: Date.now(),
          deviceFilter,
          devices: devicesRef.current,
          state: g,
        };
        return;
      }
      g.syncConfig = config;
      g.remoteWanted = true;
      const snapshotOpts =
        deviceFilter === "all"
          ? { excludeDevice: config?.device_id }
          : { devices: deviceFilter };

      setRemotePending(true);
      const remoteTasks: Promise<unknown>[] = [];
      remoteTasks.push(
        remoteSnapshots(historyFromMs, nowMs, snapshotOpts)
          .then((history) => {
            g.remoteHistory = history;
          })
          .catch((e) => {
            // 汇总模式保留本机数据降级展示；具体设备筛选则明确提示失败。
            if (deviceFilter !== "all") {
              throw new Error(
                t("dashboard.compare.remoteHistoryFailed", {
                  msg: shortError(e),
                })
              );
            }
            g.degraded = true;
          })
      );
      remoteTasks.push(
        remoteAgentQuotaSnapshots(historyFromMs, nowMs, snapshotOpts)
          .then((history) => {
            g.remoteAgentHistory = history;
          })
          .catch(() => {
            // 汇总保留本机快照；指定设备没有远端快照时仍显示其他可用数据。
            g.degraded = true;
          })
      );
      // 当前选中范围的远端 usage 懒取（与快照并行，不额外串行）
      remoteTasks.push(ensureRemoteRange(rangeRef.current));

      try {
        await Promise.all(remoteTasks);
      } catch (e) {
        if (isCurrent()) setError(shortError(e));
      }
      if (!isCurrent()) return;
      publish(g);
      setRemoteDegraded(g.degraded);
      setQuotaPending(false);
      setTokensPending(false);
      setRemotePending(false);
      setLastUpdatedAt(Date.now());
      compareCache = {
        data: publishedRef.current ?? { timelines: [], tokensByRange: {} },
        at: Date.now(),
        deviceFilter,
        devices: devicesRef.current,
        state: g,
      };
    },
    [applyDevices, deviceFilter, ensureRemoteRange, publish, t]
  );

  // 设备筛选 / 手动刷新触发新 generation；命中未过期缓存时完全不取数
  useEffect(() => {
    const manual = manualRef.current;
    manualRef.current = false;
    const cached = manual ? null : compareCache;
    const reusable = cached != null && cached.deviceFilter === deviceFilter;
    if (reusable && cached) {
      // 缓存命中：立即渲染（无加载态）；同时恢复中间态与设备列表，范围切换的
      // 远端懒取仍能补全（否则重建视图会丢掉本机部分）
      const gen = ++genRef.current;
      genStateRef.current = {
        ...cached.state,
        gen,
        cleanup: null,
        remoteInflight: new Set(),
      };
      publishedRef.current = cached.data;
      setData(cached.data);
      setLastUpdatedAt(cached.at);
      setError(null);
      setSyncConfig(cached.state.syncConfig);
      devicesRef.current = cached.devices;
      setDevices(cached.devices);
      setLoading(false);
      setQuotaPending(false);
      setTokensPending(false);
      setRemotePending(false);
      if (Date.now() - cached.at < CACHE_FRESH_MS) {
        // 新鲜期内不取数；返回清理函数让卸载/切换时作废恢复出来的 generation
        // （在途的远端懒取结果随之丢弃）
        return () => {
          genRef.current += 1;
        };
      }
    } else if (!manual) {
      // 首载：等首个结果到达前显示整页骨架
      setLoading(true);
    }
    void run();
    return () => {
      // 卸载 / 重新触发：作废当前 generation
      genRef.current += 1;
      genStateRef.current?.cleanup?.();
    };
  }, [deviceFilter, nonce, run]);

  // 范围切换：未取过的范围懒取远端 usage（已有范围命中缓存不重取）
  useEffect(() => {
    rangeRef.current = range;
    void ensureRemoteRange(range);
  }, [range, ensureRemoteRange]);

  const reload = useCallback(() => {
    manualRef.current = true;
    setNonce((n) => n + 1);
  }, []);

  return {
    data,
    loading,
    quotaPending,
    tokensPending,
    remotePending,
    error,
    remoteDegraded,
    lastUpdatedAt,
    syncConfig,
    devices,
    deviceFilter,
    setDeviceFilter,
    reload,
  };
}
