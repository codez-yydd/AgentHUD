/**
 * 主窗口设置页「通用」分区：自悬浮窗设置面板迁移的应用级配置——
 * 开机自启 / 网络代理 / 全局快捷键 / 重置时间展示 / 汇率五项。
 * 逻辑与悬浮窗版逐项对齐（保存时机、失败回滚、防呆口径不变），版式换成
 * 主窗口设置行（db-set-row）。
 *
 * 跨窗实时性：重置时间展示写入后经 zbar://reset-display-changed 广播，
 * 悬浮窗各 Agent 面板与本窗额度驾驶舱的 useResetDisplay 实时重读；
 * 其余三项落 Rust 配置文件（~/.agenthud），由各消费方按既有节奏读取。
 */
import { useEffect, useState } from "react";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import { useI18n } from "../../i18n";
import {
  fetchFxRate,
  getCursorConfig,
  getProxyConfig,
  getShortcutConfig,
  setCursorConfig,
  setProxyConfig,
  setShortcutConfig,
} from "../../api";
import type {
  CursorConfig,
  ProxyConfig,
  ShortcutConfig,
} from "../../types";
import { formatResetStamp } from "../../format";
import {
  loadResetDisplay,
  saveResetDisplay,
  type ResetDisplay,
} from "../../resetDisplay";
import { ToggleSwitch } from "./controls";

export function GeneralSettings() {
  const { locale, t } = useI18n();
  // 配置加载成功后才允许保存/更新/应用：加载失败时组件停在默认值，
  // 若仍可保存会把默认汇率等值写回后端覆盖真实配置（沿悬浮窗版口径）
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // ===== 开机自启：插件写当前用户启动项（Windows 启动项 / macOS
  //      LaunchAgent），不需要管理员权限 =====
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [autostartLoaded, setAutostartLoaded] = useState(false);
  const [savingAutostart, setSavingAutostart] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);

  // ===== 网络代理（models.dev 价格同步与汇率更新共用的出站代理）=====
  // proxySaved：已持久化的地址（空串 = 直连）；proxyDraft：输入草稿（null = 未编辑）
  const [proxySaved, setProxySaved] = useState("");
  const [proxyDraft, setProxyDraft] = useState<string | null>(null);
  const [savingProxy, setSavingProxy] = useState(false);
  const [proxySavedFlash, setProxySavedFlash] = useState(false);
  const [proxyError, setProxyError] = useState<string | null>(null);

  // ===== 全局快捷键（唤起/隐藏面板）=====
  const [shortcutCfg, setShortcutCfg] = useState<ShortcutConfig | null>(null);
  const [shortcutDraft, setShortcutDraft] = useState("");
  const [savingShortcut, setSavingShortcut] = useState(false);
  const [shortcutSavedFlash, setShortcutSavedFlash] = useState(false);
  const [shortcutError, setShortcutError] = useState<string | null>(null);

  // ===== 重置时间展示（localStorage 持久化，改完即存并跨窗广播）=====
  const [resetDisplay, setResetDisplay] = useState<ResetDisplay>(loadResetDisplay);

  // ===== 汇率：Cursor 配置（本页仅消费其中的汇率字段）=====
  const [cursorCfg, setCursorCfg] = useState<CursorConfig>({
    cookie_source: "auto",
    cookie_header: "",
    usd_cny_rate: 7.2,
    fx_rate_auto: true,
    fx_rate_fetched_at: null,
    fx_rate_source: null,
  });
  const [savingCursor, setSavingCursor] = useState(false);
  const [cursorSavedFlash, setCursorSavedFlash] = useState(false);
  // 汇率「立即更新」进行中（防重复点击）与结果反馈（✓ 成功 / ✗ 失败）
  const [fxUpdating, setFxUpdating] = useState(false);
  const [fxUpdateResult, setFxUpdateResult] = useState<string | null>(null);
  // 汇率手动输入草稿：失焦再解析，避免清空输入时被 parseFloat(NaN) 立即跳回默认值
  const [fxDraft, setFxDraft] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([getShortcutConfig(), getCursorConfig(), getProxyConfig()])
      .then(([s, cc, pc]: [ShortcutConfig, CursorConfig, ProxyConfig]) => {
        setShortcutCfg(s);
        setShortcutDraft(s.accelerator);
        setCursorCfg(cc);
        setProxySaved(pc.proxy);
        setLoaded(true);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    isAutostartEnabled()
      .then(setAutostartEnabled)
      .catch((e) =>
        setAutostartError(t("settings.autostartReadFail", { msg: String(e) }))
      )
      .finally(() => setAutostartLoaded(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 应用开机自启设置：成功后回写 UI 态，失败提示（不动已写入的系统状态，
  // 插件内部保证失败时不残留半套启动项）
  const handleAutostartChange = async (enabled: boolean) => {
    setSavingAutostart(true);
    setAutostartError(null);
    try {
      if (enabled) {
        await enableAutostart();
      } else {
        await disableAutostart();
      }
      setAutostartEnabled(enabled);
    } catch (e) {
      setAutostartError(
        enabled
          ? t("settings.autostartFailOn", { msg: String(e) })
          : t("settings.autostartFailOff", { msg: String(e) })
      );
    } finally {
      setSavingAutostart(false);
    }
  };

  // 保存代理地址：草稿失焦才提交（与手动汇率输入同款），空串 = 直连。
  // 仅持久化不探测——代理是否可用由实际同步/汇率更新时的报错暴露
  const commitProxy = async () => {
    if (proxyDraft === null) return;
    const v = proxyDraft.trim();
    setProxyDraft(null);
    if (v === proxySaved) return;
    setSavingProxy(true);
    setProxyError(null);
    try {
      await setProxyConfig({ proxy: v });
      setProxySaved(v);
      setProxySavedFlash(true);
      setTimeout(() => setProxySavedFlash(false), 1500);
    } catch (e) {
      // 保存失败：输入框回到已保存值，错误单独展示（与快捷键卡片同款）
      setProxyError(String(e));
    } finally {
      setSavingProxy(false);
    }
  };

  // 保存并应用快捷键（注册失败时提示用户改键）；启用开关与键位一并落盘
  const handleSaveShortcut = async () => {
    if (!shortcutCfg) return;
    setSavingShortcut(true);
    setShortcutError(null);
    try {
      const next = { ...shortcutCfg, accelerator: shortcutDraft.trim() };
      await setShortcutConfig(next);
      setShortcutCfg(next);
      setShortcutSavedFlash(true);
      setTimeout(() => setShortcutSavedFlash(false), 1500);
    } catch (e) {
      setShortcutError(String(e));
    } finally {
      setSavingShortcut(false);
    }
  };

  // 保存 Cursor 配置（本页改动落盘：手动汇率输入、自动更新开关）
  const handleSaveCursor = async () => {
    setSavingCursor(true);
    setError(null);
    try {
      await setCursorConfig(cursorCfg);
      setCursorSavedFlash(true);
      setTimeout(() => setCursorSavedFlash(false), 1500);
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingCursor(false);
    }
  };

  // 立即联网更新汇率。nextCfg 用于勾选自动时（state 尚未生效）传入最新配置，
  // 保证后端 load→改→save 基于最新值，不覆盖未落盘的改动。
  const handleFetchFxRate = async (nextCfg?: CursorConfig) => {
    setFxUpdating(true);
    setFxUpdateResult(null);
    try {
      // 先保存当前配置再联网（后端在磁盘配置上合并汇率，与测试连接同款顺序）
      await setCursorConfig(nextCfg ?? cursorCfg);
      const [rate, source] = await fetchFxRate();
      setCursorCfg((c) => ({
        ...c,
        usd_cny_rate: rate,
        fx_rate_fetched_at: Date.now(),
        fx_rate_source: source,
      }));
      setFxUpdateResult(
        `✓ ${rate.toFixed(2)}${locale === "zh" ? "（" : " ("}${source}${locale === "zh" ? "）" : ")"}`
      );
    } catch (e) {
      // 失败保留旧汇率值，只提示错误
      setFxUpdateResult(`✗ ${String(e)}`);
    } finally {
      setFxUpdating(false);
    }
  };

  return (
    <>
      {error && (
        <p className="db-set-note db-set-note--error" role="alert">
          {error}
        </p>
      )}

      {/* 开机自启 */}
      <section className="db-set-row">
        <div className="db-set-row__text">
          <h2 className="db-set-row__title">{t("settings.autostart")}</h2>
          <p className="db-set-row__desc">{t("settings.autostartHint")}</p>
          {!autostartLoaded && (
            <p className="db-set-note">{t("settings.readingState")}</p>
          )}
          {autostartError && (
            <p className="db-set-note db-set-note--error">{autostartError}</p>
          )}
        </div>
        <ToggleSwitch
          checked={autostartEnabled}
          disabled={!autostartLoaded || savingAutostart}
          title={t("settings.autostart")}
          onChange={handleAutostartChange}
        />
      </section>

      {/* 网络代理：models.dev 价格在线同步与汇率更新共用的出站代理（空 = 直连） */}
      <section className="db-set-row db-set-row--block">
        <div className="db-set-row__text">
          <h2 className="db-set-row__title">{t("settings.proxyCard")}</h2>
          <p className="db-set-row__desc">{t("settings.proxyHint")}</p>
        </div>
        <div className="db-set-blockbody">
          <input
            type="text"
            className="db-set-input"
            value={proxyDraft ?? proxySaved}
            placeholder="http://127.0.0.1:7890"
            disabled={savingProxy}
            onChange={(e) => setProxyDraft(e.target.value)}
            onBlur={commitProxy}
            spellCheck={false}
          />
          <p className="db-set-note">
            {savingProxy
              ? t("common.saving")
              : proxySavedFlash
                ? t("common.saved")
                : proxySaved === ""
                  ? t("settings.proxyDirect")
                  : proxySaved}
          </p>
          {proxyError && (
            <p className="db-set-note db-set-note--error">{proxyError}</p>
          )}
        </div>
      </section>

      {/* 全局快捷键：唤起/隐藏面板；启用开关与键位点「应用」一并生效 */}
      {shortcutCfg && (
        <section className="db-set-row db-set-row--block">
          <div className="db-set-row__text">
            <h2 className="db-set-row__title">{t("settings.shortcut")}</h2>
            <p className="db-set-row__desc">{t("settings.shortcutHint")}</p>
          </div>
          <div className="db-set-blockbody">
            <div className="db-set-inline">
              <input
                type="text"
                className="db-set-input db-set-input--mono"
                value={shortcutDraft}
                placeholder="alt+shift+z"
                onChange={(e) => setShortcutDraft(e.target.value)}
                spellCheck={false}
              />
              <label className="db-set-check" title={t("settings.shortcutEnable")}>
                <span>{t("settings.shortcutEnable")}</span>
                <ToggleSwitch
                  checked={shortcutCfg.enabled}
                  title={t("settings.shortcutEnable")}
                  onChange={(next) =>
                    setShortcutCfg({ ...shortcutCfg, enabled: next })
                  }
                />
              </label>
              <button
                type="button"
                className="db-btn db-btn--primary"
                onClick={handleSaveShortcut}
                disabled={savingShortcut || !loaded}
              >
                {savingShortcut
                  ? t("settings.applying")
                  : shortcutSavedFlash
                    ? t("settings.applied")
                    : t("settings.apply")}
              </button>
            </div>
            {shortcutError && (
              <p className="db-set-note db-set-note--error">{shortcutError}</p>
            )}
          </div>
        </section>
      )}

      {/* 重置时间展示：倒计时 / 具体时间点可同时开启，写入即跨窗广播 */}
      <section className="db-set-row">
        <div className="db-set-row__text">
          <h2 className="db-set-row__title">{t("settings.resetDisplay")}</h2>
          <p className="db-set-row__desc">{t("settings.resetDisplayHint")}</p>
        </div>
        <div className="db-set-checks">
          {(
            [
              ["countdown", t("settings.resetCountdown")],
              ["datetime", t("settings.resetDatetime")],
            ] as [keyof ResetDisplay, string][]
          ).map(([key, label]) => (
            <label key={key} className="db-set-check">
              <span>{label}</span>
              <ToggleSwitch
                checked={resetDisplay[key]}
                title={label}
                onChange={(next) => {
                  const nextDisplay = { ...resetDisplay, [key]: next };
                  // 落盘 + 广播（zbar://reset-display-changed），两窗实时生效
                  saveResetDisplay(nextDisplay);
                  setResetDisplay(nextDisplay);
                }}
              />
            </label>
          ))}
        </div>
      </section>

      {/* 汇率：USD→CNY 折算（价格只存美元，人民币花费实时折算） */}
      <section className="db-set-row db-set-row--block">
        <div className="db-set-row__text">
          <h2 className="db-set-row__title">{t("settings.fxCard")}</h2>
          <p className="db-set-row__desc">{t("settings.fxNote")}</p>
        </div>
        <div className="db-set-blockbody">
          <div className="db-set-inline">
            <span className="db-set-inline__label">USD→CNY</span>
            <input
              type="number"
              step="0.1"
              min="0.1"
              className="db-set-input db-set-input--num"
              value={fxDraft !== null ? fxDraft : String(cursorCfg.usd_cny_rate)}
              readOnly={cursorCfg.fx_rate_auto}
              title={
                cursorCfg.fx_rate_auto
                  ? t("settings.fxAutoNote")
                  : undefined
              }
              onChange={(e) => setFxDraft(e.target.value)}
              onBlur={() => {
                // 失焦才解析：清空/非法输入保持原值，不被立即跳回默认汇率
                if (fxDraft !== null) {
                  const v = parseFloat(fxDraft);
                  if (v > 0 && v !== cursorCfg.usd_cny_rate) {
                    setCursorCfg({ ...cursorCfg, usd_cny_rate: v });
                  }
                  setFxDraft(null);
                }
              }}
            />
            <button
              type="button"
              className="db-btn"
              onClick={() => handleSaveCursor()}
              // 自动更新开启时输入框 readOnly，手动保存无意义，一并禁用
              //（下方「立即更新」不受影响）
              disabled={savingCursor || !loaded || cursorCfg.fx_rate_auto}
            >
              {savingCursor
                ? t("common.saving")
                : cursorSavedFlash
                  ? t("common.saved")
                  : t("common.save")}
            </button>
          </div>
          <div className="db-set-inline">
            <label className="db-set-check" title={t("settings.fxDailyTitle")}>
              <span>{t("settings.fxDaily")}</span>
              <ToggleSwitch
                checked={cursorCfg.fx_rate_auto}
                title={t("settings.fxDaily")}
                onChange={(next) => {
                  const nextCfg = { ...cursorCfg, fx_rate_auto: next };
                  setCursorCfg(nextCfg);
                  // 勾选自动且从未获取过：顺带立即拉一次，避免长期显示"尚未联网获取"
                  if (next && !cursorCfg.fx_rate_fetched_at) {
                    handleFetchFxRate(nextCfg);
                  }
                }}
              />
            </label>
            <button
              type="button"
              className="db-btn"
              onClick={() => handleFetchFxRate()}
              disabled={fxUpdating || !loaded}
              title={t("settings.updateNowTitle")}
            >
              {fxUpdating ? t("settings.updating") : t("settings.updateNow")}
            </button>
            {fxUpdateResult && (
              <span
                className={`db-set-note ${
                  fxUpdateResult.startsWith("✓")
                    ? "db-set-note--ok"
                    : "db-set-note--error"
                }`}
              >
                {fxUpdateResult}
              </span>
            )}
            <span className="db-set-note">
              {cursorCfg.fx_rate_fetched_at
                ? `${cursorCfg.fx_rate_source ?? t("settings.fxUnknownSource")} · ${formatResetStamp(cursorCfg.fx_rate_fetched_at)}`
                : t("settings.fxNever")}
            </span>
          </div>
          {cursorCfg.fx_rate_auto && (
            <p className="db-set-note">{t("settings.fxAutoNote")}</p>
          )}
        </div>
      </section>
    </>
  );
}
