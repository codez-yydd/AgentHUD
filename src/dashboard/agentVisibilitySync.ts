/**
 * 主窗口的 Agent 展示可见性联动（自悬浮窗 App.tsx 迁移，主窗口成为配置
 * 宿主后的唯一「设置入口写入方」）：
 *
 *  1) presence 探测：挂载批量查探测名单（凭证型 provider + claude/cursor
 *     「其他账号」+ dsh/deepseek 特判）；凭证增删改后单点重查——事件源为
 *     同窗 window 事件与跨窗 Tauri 事件（zbar://credentials-changed）双路：
 *     凭证操作 UI 全在悬浮窗（CredentialsCard / 添加服务表单），跨窗事件
 *     让本 hook 在主窗口在场时接住联动（= 升级前悬浮窗内联动的等价行为）；
 *  2) 「有凭证自动显示」：presence 由无到有的瞬间 enableAgentByCredential
 *     （幂等；显式关闭过的 agent 不再自动开启）；
 *  3) visibility 变化监听：同窗 window 事件 + 跨窗 zbar://agent-visibility-
 *     changed（含本 hook 的设置写入与其他窗口的联动写入）→ 重读 localStorage
 *     刷新 state，并镜像到 Rust（ZCode 注入额度线程读取的名单）；
 *  4) 「远端有数据也显示」：每 5 分钟查同步服务的 source 清单，有数据即
 *     自动开启对应 agent（Kimi / DSH / OpenCode / MiniMax Code 四个本地
 *     采集型来源，解决「别的设备在用、本机没装 CLI 时远端数据无入口」）；
 *  5) 设置页写路径 setAgentVisible：显式关闭标记（manually-disabled）+
 *     localStorage 保存 + Rust 镜像 + 跨窗广播（悬浮窗收事件重读 tab 显隐）。
 *
 * 悬浮窗 App.tsx 侧保留：presence 探测（喂 DataProvider 轮询门槛与 tab
 * 门槛）、「有凭证自动显示」（凭证添加宿主在本窗，保证主窗口不在场时
 * CredentialsCard 路径的闭环）与启动/变更镜像（主窗口关闭即销毁，本窗
 * 常驻时兜底刷新 Rust 名单）。两侧写入均经 broadcastAgentVisibilityChanged
 * 互相同步，enable/disable 与镜像幂等，双窗并发无竞态伤害。
 */
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AGENT_VISIBILITY_BROADCAST_EVENT,
  AGENT_VISIBILITY_EVENT,
  CREDENTIALS_BROADCAST_EVENT,
  CREDENTIALS_CHANGED_EVENT,
  CREDENTIAL_AGENT_ALIAS,
  CREDENTIAL_AGENTS,
  PRESENCE_PROVIDERS,
  REMOTE_SOURCE_AGENTS,
  broadcastAgentVisibilityChanged,
  clearAgentManuallyDisabled,
  enableAgentByCredential,
  loadAgentVisibility,
  markAgentManuallyDisabled,
  saveAgentVisibility,
  type AgentId,
  type AgentVisibility,
} from "../agentVisibility";
import {
  hasDshPlatformGrant,
  hasProviderCredentials,
  remoteSourcePresence,
  syncAgentVisibility,
} from "../api";

export function useAgentVisibilitySync(): {
  agentVisibility: AgentVisibility;
  setAgentVisible: (id: AgentId, visible: boolean) => void;
} {
  // 凭证驱动 provider 的「已有凭证」状态（provider → bool）：启动批量探测 +
  // 凭证增删改事件后单点重查（本地文件级检查，开销可忽略）
  const [credentialPresence, setCredentialPresence] = useState<
    Record<string, boolean>
  >({});
  const [agentVisibility, setAgentVisibility] = useState<AgentVisibility>(() =>
    loadAgentVisibility()
  );

  // 设置页开关写路径（主窗口是唯一设置入口；写 localStorage + Rust 镜像 +
  // 跨窗广播，悬浮窗监听后重读刷新 tab 显隐）。显式关闭标记：手动关闭的
  // 凭证型 agent 此后 presence 无→有不再被「有凭证自动显示」打开，重新
  // 手动开启即清除标记恢复联动。
  const setAgentVisible = (id: AgentId, visible: boolean) => {
    if (visible) {
      clearAgentManuallyDisabled(id);
    } else {
      markAgentManuallyDisabled(id);
    }
    setAgentVisibility((current) => {
      const next = { ...current, [id]: visible };
      saveAgentVisibility(next);
      // 同步 Rust 侧镜像：ZCode 注入额度线程据此只查/只显示开启的 agent
      void syncAgentVisibility(next).catch(() => {});
      broadcastAgentVisibilityChanged();
      return next;
    });
  };

  // 挂载时把 localStorage 里的展示开关镜像到 Rust（注入额度线程/后续冷启
  // 动读取），避免首次运行还按默认名单显示
  useEffect(() => {
    void syncAgentVisibility(loadAgentVisibility()).catch(() => {});
  }, []);

  // 1) presence 探测：启动批量查全部探测名单；凭证增删改事件后重查该
  //    provider（同窗事件源：未来主窗口内的凭证操作；跨窗事件源：悬浮窗
  //    的 CredentialsCard / 添加服务表单，见 notifyCredentialsChanged）
  useEffect(() => {
    const probe = (providers: readonly string[]) => {
      for (const p of providers) {
        // deepseek 是混合型 provider：有手动 API Key 走开放平台余额接口，
        // 没有 Key 时后端回退到 DSH 本地登录条目（grant）。两条路任一存在
        // 都应置 presence=true，才能触发联动开启 dsh tab（与悬浮窗同款口径）。
        // 两路 Promise 并行，任一为真即视为有；全部失败才按「无」处理。
        const presence$: Promise<boolean> =
          p === "deepseek"
            ? Promise.all([
                hasProviderCredentials(p).catch(() => false),
                hasDshPlatformGrant().catch(() => false),
              ]).then(([hasKey, hasGrant]) => hasKey || hasGrant)
            : hasProviderCredentials(p);
        presence$
          .then((has) =>
            setCredentialPresence((prev) =>
              prev[p] === has ? prev : { ...prev, [p]: has }
            )
          )
          .catch(() => {
            // 探测失败视为无凭证（不触发自动开启），不阻断启动
          });
      }
    };
    probe(PRESENCE_PROVIDERS);
    const onCredentialsChanged = (provider: unknown) => {
      if (
        typeof provider === "string" &&
        provider &&
        PRESENCE_PROVIDERS.includes(provider)
      ) {
        probe([provider]);
      }
    };
    const onWindowEvent = (e: Event) => {
      onCredentialsChanged((e as CustomEvent<{ provider: string }>).detail?.provider);
    };
    window.addEventListener(CREDENTIALS_CHANGED_EVENT, onWindowEvent);
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen<string>(CREDENTIALS_BROADCAST_EVENT, (event) => {
      onCredentialsChanged(event.payload);
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：悬浮窗凭证操作后延迟到下次挂载探测 */
      });
    return () => {
      cancelled = true;
      dispose?.();
      window.removeEventListener(CREDENTIALS_CHANGED_EVENT, onWindowEvent);
    };
  }, []);

  // 2) 「有凭证自动显示」：presence 由无到有的瞬间自动开启对应 agent
  //    （写 localStorage + 同窗/跨窗广播）。只在状态迁移时触发；用户在
  //    设置页显式关闭过的 agent 不再自动开启（见 enableAgentByCredential
  //    的标记检查）。探测键 = 凭证 provider id，目标 = 要显示的 agent：
  //    两者多数同字面（CREDENTIAL_AGENTS），另有少数异名关联
  //    （CREDENTIAL_AGENT_ALIAS，如 deepseek 凭证 → dsh 面板里的平台余额区）。
  const prevPresenceRef = useRef<Record<string, boolean>>({});
  useEffect(() => {
    const prev = prevPresenceRef.current;
    const next: Record<string, boolean> = { ...prev };
    const pairs: ReadonlyArray<{ probe: string; agent: AgentId }> = [
      ...CREDENTIAL_AGENTS.map((id) => ({
        probe: id as string,
        agent: id as AgentId,
      })),
      ...CREDENTIAL_AGENT_ALIAS.map(({ provider, agent }) => ({
        probe: provider,
        agent,
      })),
    ];
    for (const { probe, agent } of pairs) {
      const has = !!credentialPresence[probe];
      if (has && !prev[probe]) {
        enableAgentByCredential(agent);
      }
      next[probe] = has;
    }
    prevPresenceRef.current = next;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [credentialPresence]);

  // 3) visibility 层监听：本 hook 之外的写入（两窗联动 enableAgentByCredential
  //    都带同窗 + 跨窗双广播）经事件通知重读 localStorage，保持单一真相源，
  //    并幂等刷新 Rust 镜像。
  useEffect(() => {
    const sync = () => {
      const next = loadAgentVisibility();
      setAgentVisibility(next);
      void syncAgentVisibility(next).catch(() => {});
    };
    window.addEventListener(AGENT_VISIBILITY_EVENT, sync);
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen(AGENT_VISIBILITY_BROADCAST_EVENT, sync)
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：另一窗写入后延迟到下次挂载重读 */
      });
    return () => {
      cancelled = true;
      dispose?.();
      window.removeEventListener(AGENT_VISIBILITY_EVENT, sync);
    };
  }, []);

  // 4) 「远端有数据也显示」：后端读同步服务的模型清单并去重出 source，
  //    有数据即自动开启对应 tab（未启用同步 / 服务器不可达时返回空列表，
  //    行为与升级前一致）。用户显式关闭过的 agent 不会被重新打开
  //    （enableAgentByCredential 内部检查手动关闭标记）。每 5 分钟复查一次。
  useEffect(() => {
    let alive = true;
    const probe = () => {
      remoteSourcePresence()
        .then((sources) => {
          if (!alive) return;
          const remote = new Set(sources);
          for (const { source, agent } of REMOTE_SOURCE_AGENTS) {
            if (remote.has(source)) enableAgentByCredential(agent);
          }
        })
        .catch(() => {
          // 探测失败视为无远端数据（不显示新 tab），不阻断启动
        });
    };
    probe();
    const timer = window.setInterval(probe, 5 * 60_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  return { agentVisibility, setAgentVisible };
}
