import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { StatsPanel } from "./StatsPanel";
import { SettingsPanel } from "./SettingsPanel";
import { SyncPanel } from "./SyncPanel";
import { DataProvider } from "./DataCache";
import { ResizeHandles } from "./ResizeHandles";
import { ReportsPanel } from "./ReportsPanel";
import { ThemePanel } from "./ThemePanel";
import { SkinHubPanel } from "./SkinHubPanel";
import { SkinStandalonePanel } from "./SkinStandalonePanel";
import { ADD_SERVICE_EVENT } from "./AddServiceMenu";
import {
  fetchPricing,
  hasProviderCredentials,
  remoteSourcePresence,
  saveCurrency,
  syncAgentVisibility,
} from "./api";
import { startUpdateScheduler } from "./updater";
import { useI18n } from "./i18n";
import { loadLocale } from "./i18n/locale";
import {
  APPEARANCE_CHANGED_EVENT,
  applyTheme,
  loadTheme,
  type Theme,
} from "./appearance";
import type { Currency, PricingConfig } from "./types";
import {
  AGENT_VISIBILITY_EVENT,
  CREDENTIALS_CHANGED_EVENT,
  CREDENTIAL_AGENTS,
  CREDENTIAL_AGENT_ALIAS,
  PRESENCE_PROVIDERS,
  REMOTE_SOURCE_AGENTS,
  clearAgentManuallyDisabled,
  enableAgentByCredential,
  loadAgentVisibility,
  markAgentManuallyDisabled,
  saveAgentVisibility,
  type AgentId,
  type AgentVisibility,
} from "./agentVisibility";

// 皮肤域三层视图：skinHub（分类页）→ skinStandalone（免注入子页）/
// theme（需注入子页）；分类页本身不发起任何注入状态检测。
// 窄窗二期减负：价格设置页已收编到主窗口（Dashboard 定价页），面板不再有
// pricing 视图——统计页「价格设置」按钮改为 openMainWindow("pricing") 直达
type View =
  | "stats"
  | "sync"
  | "reports"
  | "skinHub"
  | "skinStandalone"
  | "theme"
  | "settings";

export default function App() {
  const { locale, setLocale } = useI18n();
  const [view, setView] = useState<View>("stats");
  // 币种从属语言：中文=人民币、英文=美元。语言是显式偏好，切换语言时币种跟随
  // （含启动对齐，菜单栏标题同步刷新）。窄窗价格页移除后本窗不再有手动切换
  // 币种的入口，币种只随语言（临时切换口径的需求由主窗口定价页承接）
  const langCurrency: Currency = locale === "en" ? "usd" : "cny";
  const [currency, setCurrency] = useState<Currency>(langCurrency);
  const [pricing, setPricing] = useState<PricingConfig>({
    usd: {},
  });
  const [agentVisibility, setAgentVisibility] = useState<AgentVisibility>(() =>
    loadAgentVisibility()
  );
  // 凭证驱动 provider 的「已有凭证」状态（provider → bool）：
  // 启动批量探测 + 凭证增删改事件后单点重查（本地文件级检查，开销可忽略）
  const [credentialPresence, setCredentialPresence] = useState<
    Record<string, boolean>
  >({});
  // 设置页「添加凭证」快捷入口的跳转目标：暂存 provider，切回统计页时
  // 交给 StatsPanel 打开该服务的添加表单（挂载即消费，消费后清空）
  const [pendingAddProvider, setPendingAddProvider] = useState<string | null>(
    null
  );

  // 快捷入口事件（SettingsPanel 派发）：切回统计页并携带目标 provider
  useEffect(() => {
    const open = (e: Event) => {
      const provider = (e as CustomEvent<unknown>).detail;
      if (typeof provider === "string" && provider) {
        setPendingAddProvider(provider);
        setView("stats");
      }
    };
    window.addEventListener(ADD_SERVICE_EVENT, open);
    return () => window.removeEventListener(ADD_SERVICE_EVENT, open);
  }, []);

  // 外观双向同步（补面板侧跟随）：主题/语言切换后 applyTheme / setLocale 都会
  // 广播 zbar://appearance-changed——面板此前只发不收，在主窗口里切主题/语言后
  // 本窗要重启才生效。这里补上「跟随者」分支：收到广播重读 localStorage，
  // 与当前 DOM / i18n 状态不同才应用（相同即跳过，避免再 emit 造成回声循环：
  // applyTheme 与 setLocale 都会再次广播，主窗口侧同样有同值守卫）。
  // 监听失败静默：保持启动时读取的偏好。
  const localeRef = useRef(locale);
  localeRef.current = locale;

  useEffect(() => {
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen(APPEARANCE_CHANGED_EVENT, () => {
      // 主题：以 <html> 现有类为本窗当前值（ThemeToggle 切换后即时同步）
      const nextTheme: Theme = loadTheme();
      const currentTheme: Theme = document.documentElement.classList.contains(
        "dark"
      )
        ? "dark"
        : "light";
      if (nextTheme !== currentTheme) applyTheme(nextTheme);
      // 语言：与当前不同才 setLocale（同值调用虽幂等但会再广播一次）
      const nextLocale = loadLocale();
      if (nextLocale && nextLocale !== localeRef.current) {
        setLocale(nextLocale);
      }
    })
      .then((unlisten) => {
        // effect 已清理（StrictMode 双挂载 / 快速卸载）时立即退订
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：保持启动时读取的主题与语言 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [setLocale]);

  const handleAgentVisibilityChange = (id: AgentId, visible: boolean) => {
    // 显式关闭标记：设置页手动关闭的凭证型 agent，此后 presence 无→有
    // 不再被「有凭证自动显示」打开；重新手动开启即清除标记恢复联动
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
      return next;
    });
  };

  // 启动时把 localStorage 里的展示开关镜像到 Rust（注入额度线程/后续冷启
  // 动读取），避免首次运行还按默认名单显示
  useEffect(() => {
    void syncAgentVisibility(loadAgentVisibility()).catch(() => {});
  }, []);

  // ===== 凭证驱动的 provider 联动 =====
  // 1) presence 探测：启动时批量查全部探测名单（凭证驱动的新 provider +
  //    claude/cursor「其他账号」区）；凭证增删改事件后重查该 provider
  //    （事件源：CredentialsCard 的添加/编辑/删除操作）。
  useEffect(() => {
    const probe = (providers: readonly string[]) => {
      for (const p of providers) {
        hasProviderCredentials(p)
          .then((has) =>
            setCredentialPresence((prev) =>
              prev[p] === has ? prev : { ...prev, [p]: has }
            )
          )
          .catch(() => {
            // 探测失败视为无凭证（tab 不显示），不阻断启动
          });
      }
    };
    probe(PRESENCE_PROVIDERS);
    const onCredentialsChanged = (e: Event) => {
      const provider = (e as CustomEvent<{ provider: string }>).detail
        ?.provider;
      if (
        typeof provider === "string" &&
        provider &&
        PRESENCE_PROVIDERS.includes(provider)
      ) {
        probe([provider]);
      }
    };
    window.addEventListener(CREDENTIALS_CHANGED_EVENT, onCredentialsChanged);
    return () =>
      window.removeEventListener(
        CREDENTIALS_CHANGED_EVENT,
        onCredentialsChanged
      );
  }, []);

  // 2) 「有凭证自动显示」：presence 由无到有的瞬间自动开启对应 agent
  //    （写 localStorage + 广播）。只在状态迁移时触发；用户在设置页显式
  //    关闭过的 agent 不再自动开启（见 enableAgentByCredential 的标记检查），
  //    删除全部凭证时由 disableAgentByCredential 回退偏好。
  //    探测键 = 凭证 provider id，目标 = 要显示的 agent：两者多数同字面
  //    （CREDENTIAL_AGENTS），另有少数异名关联（CREDENTIAL_AGENT_ALIAS，
  //    如 deepseek 凭证 → dsh 面板里的 DeepSeek 平台余额区）。
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

  // 3) visibility 层广播：设置页之外的写入（enableAgentByCredential）
  //    经事件通知本组件重读 localStorage，保持单一真相源。
  useEffect(() => {
    const sync = () => {
      const next = loadAgentVisibility();
      setAgentVisibility(next);
      void syncAgentVisibility(next).catch(() => {});
    };
    window.addEventListener(AGENT_VISIBILITY_EVENT, sync);
    return () => window.removeEventListener(AGENT_VISIBILITY_EVENT, sync);
  }, []);

  // 4) 「远端有数据也显示」：Kimi / DSH / OpenCode / MiniMax Code 四个本地
  //    采集型来源的可见性默认关闭，presence 又只看本机——于是「别的设备在用、
  //    这台没装这个 CLI」时，本机连 tab 都没有，远端数据完全看不到。这里把
  //    远端来源存在性并入联动（后端读同步服务的模型清单并去重出 source，
  //    未启用同步 / 服务器不可达时返回空列表，行为与升级前一致），有数据即
  //    自动开启对应 tab。用户在设置页显式关闭过的 agent 不会被重新打开
  //    （enableAgentByCredential 内部检查手动关闭标记）。
  //    每 5 分钟复查一次：其他设备后来才开始用某个 agent 时本机 tab 也会出现。
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

  // 语言对应币种生效：启动时把后端偏好对齐到语言（覆盖历史遗留的手动值，
  // 保证菜单栏标题与界面一致），语言切换时立即跟随并持久化。本地
  // zbar-currency 键已无任何读取方（窄窗价格页移除后废弃），不再写入
  useEffect(() => {
    setCurrency(langCurrency);
    saveCurrency(langCurrency).catch(() => {});
  }, [langCurrency]);

  useEffect(() => {
    fetchPricing()
      .then(setPricing)
      .catch(() => {});
  }, []);

  // 启动更新调度：延迟 10s 首次检查，之后每 1h 后台检查并下载；
  // 下载完成后写 localStorage 供设置入口红点展示，失败静默不打扰
  useEffect(() => startUpdateScheduler(), []);

  const backToStats = () => {
    fetchPricing()
      .then((p) => {
        // 内容没变则复用旧引用：fetchPricing 每次都返回新对象，若直接 setPricing，
        // pricing 引用变化会让 DataProvider 的刷新函数重建并立即对 4 个预设范围
        // 发起 12 个并发命令（含 7d/30d 全量扫描）+ 多次全量缓存写盘，造成返回卡顿
        setPricing((prev) =>
          JSON.stringify(prev) === JSON.stringify(p) ? prev : p
        );
      })
      .catch(() => {});
    setView("stats");
  };

  return (
    <DataProvider pricing={pricing} credentialPresence={credentialPresence}>
      <div className="panel-shell">
        {view === "stats" ? (
          <StatsPanel
            currency={currency}
            pricing={pricing}
            agentVisibility={agentVisibility}
            credentialPresence={credentialPresence}
            onGoSync={() => setView("sync")}
            onGoReports={() => setView("reports")}
            onGoTheme={() => setView("skinHub")}
            onGoSettings={() => setView("settings")}
            initialAdd={pendingAddProvider}
            onInitialAddConsumed={() => setPendingAddProvider(null)}
          />
        ) : view === "reports" ? (
          <ReportsPanel
            onBack={() => setView("stats")}
            pricing={pricing}
            currency={currency}
            agentVisibility={agentVisibility}
          />
        ) : view === "skinHub" ? (
          <SkinHubPanel
            onBack={backToStats}
            onOpenStandalone={() => setView("skinStandalone")}
            onOpenInjected={() => setView("theme")}
          />
        ) : view === "skinStandalone" ? (
          <SkinStandalonePanel onBack={() => setView("skinHub")} />
        ) : view === "theme" ? (
          <ThemePanel onBack={() => setView("skinHub")} />
        ) : view === "settings" ? (
          <SettingsPanel
            onBack={backToStats}
            agentVisibility={agentVisibility}
            onAgentVisibilityChange={handleAgentVisibilityChange}
          />
        ) : (
          <SyncPanel onBack={backToStats} />
        )}
        {/* 边缘拖拽热区：所有 view 共用，放在 panel-shell 最后一个子节点 */}
        <ResizeHandles />
      </div>
    </DataProvider>
  );
}
