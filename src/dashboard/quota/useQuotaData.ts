/**
 * 额度驾驶舱页取数 hook：纯本地历史读取 + 手动 live 刷新。
 *
 * 零自动 HTTP 铁律：页面加载只读三张本地历史表（zai / CLI 通道 / provider
 * 通道，最新快照即「当前值」，快照时间随行标注让用户知道新鲜度），
 * 不发任何网络请求；只有页头「刷新」按钮才触发各源 live 查询
 * （fetchQuota / fetch*Usage / fetchProviderQuota，Rust 侧均有 TTL 缓存，
 * 直接调安全），live 成功自然写入历史采样，完成后重读本地历史刷新整页。
 *
 * 容错与守卫（照 SyncPage 先例）：
 * - loadReqId 请求序号守卫：快速连点刷新 / 卸载后，旧请求的响应不写 state；
 * - 三路历史各自独立容错：单路失败只落 partialFailures 弱提示（该通道展示
 *   「暂无历史数据」），不拖垮其他通道；三路全部失败且无任何数据才是整页
 *   硬错误（db-errorstate 重试态）；
 * - live 刷新逐源容错：失败源名收进 refreshFailures 弱提示，其余源照常；
 * - 账号列表（zai 系列命名用）读取失败静默：系列标签退化为指纹前 6 位。
 *
 * live 目标集合：
 * - zai + codex/claude/cursor/kimi：按 agent 展示开关过滤（用户隐藏的源
 *   不主动查询，尊重设置语义）；
 * - 11 个凭证型 provider：历史里出现过的源 ∪ 本机有凭证的源
 *   （hasProviderCredentials 逐源探测，均为本地文件检查）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { AccountMeta } from "../../types";
import {
  fetchClaudeUsage,
  fetchCodexUsage,
  fetchCursorUsage,
  fetchKimiUsage,
  fetchProviderQuota,
  fetchQuota,
  getAgentQuotaHistory,
  getProviderQuotaHistory,
  getQuotaHistory,
  hasProviderCredentials,
  listAccounts,
} from "../../api";
import { loadAgentVisibility, type AgentId } from "../../agentVisibility";
import { useI18n } from "../../i18n";
import {
  QUOTA_COCKPIT_SOURCES,
  buildCockpitSeries,
  sourceLabelOf,
  type CockpitSeries,
  type QuotaCockpitSource,
} from "./model";

/** 历史取数窗口：与对比页 COMPARE_HISTORY_DAYS 一致（快照保留期即 90 天） */
export const QUOTA_COCKPIT_DAYS = 90;

/** provider 通道的 live 查询目标全集（与 Rust PROVIDER_SNAPSHOT_SOURCES 同集） */
const PROVIDER_LIVE_SOURCES: readonly QuotaCockpitSource[] =
  QUOTA_COCKPIT_SOURCES.filter(
    (s) => s !== "zai" && !["codex", "claude", "cursor", "kimi"].includes(s)
  );

/** N 天前（含今天共 N 天）的本地自然日 0 点（与对比页 startOfDaysAgo 同口径） */
function startOfDaysAgo(days: number, nowMs: number): number {
  const d = new Date(nowMs);
  d.setDate(d.getDate() - (days - 1));
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

export interface QuotaCockpitRaw {
  series: CockpitSeries[];
  /** 全部采样的最近时刻（页头新鲜度标注用；无任何采样为 null） */
  latestSampleAt: number | null;
  /** 有历史数据的来源集合（「暂无历史数据」名单 = 全集 − 这份） */
  sourcesWithData: Set<QuotaCockpitSource>;
}

export interface QuotaCockpitState {
  data: QuotaCockpitRaw | null;
  /** 首载进行中（无任何数据可渲染时） */
  loading: boolean;
  /** 整页硬错误：三路历史全部失败且无任何数据（重试态用） */
  error: string | null;
  /** 弱提示：读取失败的本地历史通道名（单通道降级，不阻塞其他通道） */
  partialFailures: string[] | null;
  /** live 刷新进行中（刷新按钮禁用 + 旋转） */
  refreshing: boolean;
  /** live 刷新失败源名（弱提示；全部成功为 null） */
  refreshFailures: string[] | null;
  /** 纯本地重读（live 完成后 / 手动重试入口共用） */
  reload: () => void;
  /** live 查询全部目标源，完成后重读本地历史 */
  refreshLive: () => void;
}

/** 错误信息截断（与总览/对比页同款：长中文提示只展示摘要） */
function shortError(error: unknown): string {
  const text = String(error).replace(/^Error:\s*/, "");
  return text.length > 120 ? text.slice(0, 117) + "…" : text;
}

export function useQuotaData(): QuotaCockpitState {
  const { t } = useI18n();
  const [data, setData] = useState<QuotaCockpitRaw | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [partialFailures, setPartialFailures] = useState<string[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshFailures, setRefreshFailures] = useState<string[] | null>(null);

  /** 请求序号守卫（照 SyncPage 的 loadReqId 先例）：旧请求的响应不写 state */
  const loadReqId = useRef(0);
  /** 上一轮 live 刷新时 provider 历史里出现过的来源（live 目标并集的一半） */
  const historySourcesRef = useRef<Set<QuotaCockpitSource>>(new Set());
  /** 是否已成功发布过数据（三路全失败时区分「无任何数据」与「有旧数据」，
   *  用 ref 而非 state 依赖，保持 loadLocal 引用稳定） */
  const hasDataRef = useRef(false);

  /** 纯本地读取：三路历史 + 账号列表并行，各路独立容错 */
  const loadLocal = useCallback(async () => {
    const reqId = ++loadReqId.current;
    const isCurrent = () => reqId === loadReqId.current;
    const nowMs = Date.now();
    const fromMs = startOfDaysAgo(QUOTA_COCKPIT_DAYS, nowMs);

    const [zaiRes, agentRes, providerRes, accountsRes] = await Promise.allSettled([
      getQuotaHistory(true, fromMs),
      getAgentQuotaHistory(fromMs, nowMs),
      getProviderQuotaHistory(fromMs, nowMs),
      listAccounts(),
    ]);
    if (!isCurrent()) return;

    const zai = zaiRes.status === "fulfilled" ? zaiRes.value : [];
    const agent = agentRes.status === "fulfilled" ? agentRes.value : [];
    const provider = providerRes.status === "fulfilled" ? providerRes.value : [];
    const accounts: AccountMeta[] | null =
      accountsRes.status === "fulfilled" ? accountsRes.value.accounts : null;

    // 单通道失败：弱提示（该通道按「暂无历史数据」降级），不阻塞其他通道
    const failures: string[] = [];
    if (zaiRes.status === "rejected") failures.push(t("dashboard.quota.channelZai"));
    if (agentRes.status === "rejected") failures.push(t("dashboard.quota.channelAgent"));
    if (providerRes.status === "rejected") {
      failures.push(t("dashboard.quota.channelProvider"));
    }
    setPartialFailures(failures.length > 0 ? failures : null);

    // 三路历史全部失败的两种处置：
    // - 尚无任何已发布数据：整页硬错误（db-errorstate 重试态）；
    // - 已有旧数据：保留旧数据不清空（弱提示已由 partialFailures 表达）。
    const allFailed =
      zaiRes.status === "rejected" &&
      agentRes.status === "rejected" &&
      providerRes.status === "rejected";
    if (allFailed) {
      if (!hasDataRef.current) {
        const first = [zaiRes, agentRes, providerRes].find(
          (r) => r.status === "rejected"
        );
        setError(
          first != null && first.status === "rejected"
            ? shortError(first.reason)
            : String(t("dashboard.quota.errorTitle"))
        );
      }
      setLoading(false);
      return;
    }
    setError(null);

    const series = buildCockpitSeries({ zai, agent, provider, accounts });
    const sourcesWithData = new Set<QuotaCockpitSource>(series.map((s) => s.source));
    historySourcesRef.current = sourcesWithData;
    let latestSampleAt: number | null = null;
    for (const item of series) {
      for (const win of item.windows) {
        const ts = win.samples[win.samples.length - 1]?.ts;
        if (ts != null && (latestSampleAt == null || ts > latestSampleAt)) {
          latestSampleAt = ts;
        }
      }
    }
    hasDataRef.current = true;
    setData({ series, latestSampleAt, sourcesWithData });
    setLoading(false);
  }, [t]);

  /** live 刷新：按可见性与凭证探测确定目标源，逐源容错并行查询，
   *  全部结束后重读本地历史（live 成功的源已把新采样写进历史表） */
  const refreshLive = useCallback(async () => {
    const reqId = ++loadReqId.current;
    setRefreshing(true);
    setRefreshFailures(null);
    const failures: string[] = [];
    const now = Date.now();
    const from = now - 3_600_000; // live 命令的统计窗口给最近 1 小时（轻量）
    const visibility = loadAgentVisibility();

    const tasks: { name: string; run: () => Promise<unknown> }[] = [];
    const enabled = (id: AgentId) => visibility[id] !== false;
    if (enabled("zai")) {
      tasks.push({ name: sourceLabelOf("zai"), run: () => fetchQuota() });
    }
    if (enabled("codex")) {
      tasks.push({
        name: sourceLabelOf("codex"),
        run: () => fetchCodexUsage(from, now, "day"),
      });
    }
    if (enabled("claude")) {
      tasks.push({
        name: sourceLabelOf("claude"),
        run: () => fetchClaudeUsage(from, now, "day"),
      });
    }
    if (enabled("cursor")) {
      tasks.push({
        name: sourceLabelOf("cursor"),
        run: () => fetchCursorUsage(from, now),
      });
    }
    if (enabled("kimi")) {
      tasks.push({
        name: sourceLabelOf("kimi"),
        run: () => fetchKimiUsage(from, now, "day"),
      });
    }
    // provider 目标 = 历史出现过的源 ∪ 本机有凭证的源（本地文件探测）
    const withCredentials = await Promise.all(
      PROVIDER_LIVE_SOURCES.map(async (source) => ({
        source,
        has: await hasProviderCredentials(source).catch(() => false),
      }))
    );
    if (reqId !== loadReqId.current) {
      // 本轮已被作废（并发触发了更新一轮 refreshLive / reload 前进了序号）：
      // refreshing 由本轮置起，退出前必须复位，否则在「页头刷新 × 错误卡
      // 重新加载」并发触发时它会永久卡在 true（按钮禁用、图标常转直到
      // 重挂载）。若前进源于新的一轮 refreshLive，其自身收尾会再管理该状态。
      setRefreshing(false);
      return;
    }
    for (const { source, has } of withCredentials) {
      if (has || historySourcesRef.current.has(source)) {
        tasks.push({
          name: sourceLabelOf(source),
          run: () => fetchProviderQuota(source),
        });
      }
    }

    await Promise.all(
      tasks.map(async ({ name, run }) => {
        try {
          await run();
        } catch {
          failures.push(name); // 单源失败不阻塞其他源
        }
      })
    );
    if (reqId !== loadReqId.current) {
      // 同上：作废退出也必须复位 refreshing，保证任何路径退出都收尾
      setRefreshing(false);
      return;
    }
    setRefreshFailures(failures.length > 0 ? failures : null);
    setRefreshing(false);
    // 历史表已被 live 查询自然更新：重读本地（loadReqId 再 +1，本轮作废）
    await loadLocal();
  }, [loadLocal]);

  // 首载：纯本地读取（零 HTTP）
  useEffect(() => {
    const subscription = listen("zbar://account-changed", () => {
      void fetchQuota().catch(() => {}).then(() => loadLocal());
    });
    return () => { void subscription.then((unlisten) => unlisten()); };
  }, [loadLocal]);

  useEffect(() => {
    void loadLocal();
    return () => {
      loadReqId.current += 1; // 卸载后作废在途请求
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return {
    data,
    loading,
    error,
    partialFailures,
    refreshing,
    refreshFailures,
    reload: () => void loadLocal(),
    refreshLive: () => void refreshLive(),
  };
}
