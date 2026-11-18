import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { NowProvider } from "./nowClock";
import { StatsPanel } from "./StatsPanel";
import { SettingsPanel } from "./SettingsPanel";
import { DataProvider } from "./DataCache";
import { ResizeHandles } from "./ResizeHandles";
import {
  fetchPricing,
  getCurrencySetting,
  hasDshPlatformGrant,
  hasProviderCredentials,
  saveCurrency,
  syncAgentVisibility,
} from "./api";
import { startUpdateScheduler } from "./updater";
import { startUpdaterBridgeHost } from "./updaterBridge";
import { useI18n } from "./i18n";
import { loadLocale } from "./i18n/locale";
import {
  APPEARANCE_CHANGED_EVENT,
  PANEL_ALPHA_CHANGED_EVENT,
  applyPanelAlpha,
  applyTheme,
  loadPanelAlpha,
  loadTheme,
  type Theme,
} from "./appearance";
import type { Currency, CurrencyMode, PricingConfig } from "./types";
import {
  AGENT_VISIBILITY_BROADCAST_EVENT,
  AGENT_VISIBILITY_EVENT,
  CREDENTIALS_CHANGED_EVENT,
  CREDENTIAL_AGENTS,
  CREDENTIAL_AGENT_ALIAS,
  PRESENCE_PROVIDERS,
  enableAgentByCredential,
  loadAgentVisibility,
  type AgentId,
  type AgentVisibility,
} from "./agentVisibility";

// 窄窗三期减负：报表 / 设备同步 / 皮肤（skinHub → skinStandalone / theme
// 三层）视图已完全迁往主窗口（用量报告 / 同步与设备 / 皮肤中心页），
// SyncPanel / ReportsPanel / 皮肤域组件的挂载点随之移除；失去挂载方的壳
//（SyncPanel / SkinHubPanel）已删除，仍被主窗口页面复用的组件保留
// （ReportContent / SkinStandalonePanel / ThemePanel 等）。
// 价格设置页此前已收编（统计页「价格设置」按钮 openMainWindow("pricing") 直达）。
// 面板只剩：stats（统计）与 settings（面板自身设置，入口在「…」菜单）
type View = "stats" | "settings";

export default function App() {
  const { locale, setLocale } = useI18n();
  const [view, setView] = useState<View>("stats");
  // 币种：默认从属语言（中文=人民币、英文=美元）。语言是显式偏好，切换
  // 语言时币种跟随（含启动对齐，菜单栏标题同步刷新）。窄窗价格页移除后
  // 本窗不再有手动切换币种的入口（临时切换口径的需求由主窗口定价页承接）。
  // 主窗口「设置」页可手动指定全局币种（currency.json 的 mode）：
  // 显式指定时本窗花费口径跟随手动值，语言切换不再覆盖（见下方两个 effect）。
  const langCurrency: Currency = locale === "en" ? "usd" : "cny";
  const [currencyMode, setCurrencyMode] = useState<CurrencyMode>("follow");
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

  // 注：数据来源开关的设置入口已迁到主窗口设置页（写入方为主窗口的
  // useAgentVisibilitySync，经 zbar://agent-visibility-changed 广播），本窗
  // 只保留读取 + 监听刷新 tab 显隐；凭证驱动的自动显示联动因凭证添加 UI
  // 在本窗而保留（见下方 effect），与主窗口侧联动幂等共存。

  // 启动时把 localStorage 里的展示开关镜像到 Rust（注入额度线程/后续冷启
  // 动读取），避免首次运行还按默认名单显示。主窗口（联动宿主）也会在挂载
  // 时做同样镜像——幂等写，双窗共存无竞态；本窗默认常驻，主窗口关闭即
  // 销毁的场景下由这里兜底保证镜像新鲜
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
        // deepseek 是混合型 provider：有手动 API Key 走开放平台余额接口，
        // 没有 Key 时后端回退到 DSH 本地登录条目（grant）。两条路任一存在
        // 都应置 presence=true，才能触发 120s 额度轮询把本地余额拉出来。
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
  //    （写 localStorage + 同窗/跨窗广播）。只在状态迁移时触发；用户在主窗口
  //    设置页显式关闭过的 agent 不再自动开启（见 enableAgentByCredential
  //    的标记检查），删除全部凭证时由 disableAgentByCredential 回退偏好。
  //    探测键 = 凭证 provider id，目标 = 要显示的 agent：两者多数同字面
  //    （CREDENTIAL_AGENTS），另有少数异名关联（CREDENTIAL_AGENT_ALIAS，
  //    如 deepseek 凭证 → dsh 面板里的 DeepSeek 平台余额区）。
  //    联动宿主已迁主窗口（useAgentVisibilitySync 同款逻辑），本窗保留一份
  //    的原因：凭证添加 UI（CredentialsCard / 添加服务表单）在本窗，主窗口
  //    关闭即销毁是常态——没有这份联动时，本窗添加凭证要等主窗口打开才出
  //    tab。enableAgentByCredential 幂等，双窗并存无重复写入伤害。
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

  // 3) visibility 层监听：本组件之外的写入（两窗的 enableAgentByCredential
  //    与主窗口设置页开关）经同窗 window 事件 + 跨窗 zbar://agent-visibility-
  //    changed 广播通知本窗重读 localStorage，保持单一真相源，并幂等刷新
  //    Rust 镜像（主窗口关闭即销毁时本窗是镜像维护兜底方）。
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
        /* 静默：主窗口改动后延迟到下次挂载重读 */
      });
    return () => {
      cancelled = true;
      dispose?.();
      window.removeEventListener(AGENT_VISIBILITY_EVENT, sync);
    };
  }, []);

  // 4) 「远端有数据也显示」的 5 分钟轮询已随设置迁移由主窗口持有
  //    （useAgentVisibilitySync），本窗不再轮询；主窗口写入同样经上面的
  //    跨窗广播回流本窗刷新 tab。

  // 面板透明度跨窗跟随：设置入口在主窗口设置页，滑杆落盘后广播
  // zbar://panel-alpha-changed，本窗重读并应用 --panel-alpha（启动时
  // main.tsx 已应用一次持久化值）。监听失败静默：保持当前透明度
  useEffect(() => {
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen(PANEL_ALPHA_CHANGED_EVENT, () => {
      applyPanelAlpha(loadPanelAlpha());
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：保持启动时读取的透明度 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, []);

  // 语言对应币种生效（follow 模式）：启动时把后端偏好对齐到语言（覆盖历史
  // 遗留的手动值，保证菜单栏标题与界面一致），语言切换时立即跟随并持久化。
  // 手动指定模式（mode ≠ follow）下**不覆盖**——面板语言链只更新
  // currency.json 的快照字段（save_currency 保留 mode），本窗展示跟随手动值。
  // 本地 zbar-currency 键已无任何读取方（窄窗价格页移除后废弃），不再写入
  const currencyModeRef = useRef(currencyMode);
  currencyModeRef.current = currencyMode;
  const langCurrencyRef = useRef(langCurrency);
  langCurrencyRef.current = langCurrency;
  useEffect(() => {
    if (currencyModeRef.current !== "follow") return;
    setCurrency(langCurrency);
    saveCurrency(langCurrency).catch(() => {});
  }, [langCurrency]);

  // 币种来源初始化与跨窗跟随：启动读一次设置（手动指定则本窗口径跟随），
  // 并监听 zbar://currency-changed（主窗口设置页改币种后 Rust 广播）——
  // 主窗口改了，面板跟着变，无需重启。读取/监听失败静默：保持 follow
  // （= 升级前行为）
  useEffect(() => {
    let cancelled = false;
    // 时序守卫：启动读取是异步的，其响应可能在 currency-changed 事件之后
    // 才返回（读到的是事件前的旧值）；事件已到达后晚到的旧响应直接丢弃，
    // 避免旧值覆盖事件带来的新值（显示短暂回退）
    let eventSeen = false;
    getCurrencySetting()
      .then((setting) => {
        if (cancelled || eventSeen) return;
        setCurrencyMode(setting.mode);
        if (setting.mode !== "follow") setCurrency(setting.currency);
      })
      .catch(() => {});
    let dispose: (() => void) | undefined;
    listen<{ mode?: string; currency?: string }>(
      "zbar://currency-changed",
      (event) => {
        eventSeen = true;
        const mode: CurrencyMode =
          event.payload?.mode === "cny" || event.payload?.mode === "usd"
            ? event.payload.mode
            : "follow";
        setCurrencyMode(mode);
        if (mode === "follow") {
          // 回到跟随语言：快照字段立即对齐语言推导值（Rust 侧切回 follow
          // 保留旧快照，若停在手动值，菜单栏标题币种会与语言不一致，
          // 直到下次语言切换才纠正；这里主动回写，set_currency 会顺带
          // 刷新菜单栏标题）
          setCurrency(langCurrencyRef.current);
          saveCurrency(langCurrencyRef.current).catch(() => {});
        } else if (
          event.payload?.currency === "cny" ||
          event.payload?.currency === "usd"
        ) {
          setCurrency(event.payload.currency);
        }
      }
    )
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：只失去跨窗实时性 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
    // langCurrency 不进依赖：事件回包缺币种值时的回退取「当下语言」推导值，
    // 语言切换由上方 follow effect 覆盖，这里用 ref 防监听反复重挂
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    fetchPricing()
      .then(setPricing)
      .catch(() => {});
  }, []);

  // 服务端价格表被其他桌面端更新后，后端会写回本地 pricing.json 并广播；
  // 这里重读以触发 DataProvider/成本重算（无服务端时事件永不触发）。
  useEffect(() => {
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen("zbar://pricing-updated", () => {
      fetchPricing()
        .then((p) =>
          setPricing((prev) =>
            JSON.stringify(prev) === JSON.stringify(p) ? prev : p
          )
        )
        .catch(() => {});
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, []);

  // 启动更新调度：延迟 10s 首次检查，之后每 1h 后台检查并下载；
  // 下载完成后写 localStorage 供设置入口红点展示，失败静默不打扰。
  // 同处挂载更新桥宿主：面板是唯一更新执行者（Update 资源 rid 只在本
  // webview 有效），主窗口「关于」页的状态读写都经跨窗口事件桥（见
  // updaterBridge.ts），updater.ts 本体零改动
  useEffect(() => startUpdateScheduler(), []);
  useEffect(() => startUpdaterBridgeHost(), []);

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
    /* 共享 1s 时钟（内存压降专项 6）：面板各处倒计时不再各自挂定时器，
       全树每秒一次重渲染；窗口 hidden 时暂停（见 nowClock.tsx） */
    <NowProvider>
    <DataProvider pricing={pricing} credentialPresence={credentialPresence}>
      <div className="panel-shell">
        {view === "stats" ? (
          <StatsPanel
            currency={currency}
            pricing={pricing}
            agentVisibility={agentVisibility}
            credentialPresence={credentialPresence}
            onGoSettings={() => setView("settings")}
          />
        ) : (
          <SettingsPanel onBack={backToStats} />
        )}
        {/* 边缘拖拽热区：所有 view 共用，放在 panel-shell 最后一个子节点 */}
        <ResizeHandles />
      </div>
    </DataProvider>
    </NowProvider>
  );
}
