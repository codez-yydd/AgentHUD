/**
 * 设置页（主窗口第九页）：应用级设置的统一入口，五个设置项——
 * ① 语言（中文 / English）：复用面板同一套 i18n 存储（zbar-locale）与
 *    zbar://appearance-changed 广播，主窗口改了面板跟着变；
 * ② 外观主题（浅色 / 深色 / 跟随系统）：复用既有主题体系——来源偏好存
 *    zbar-theme-pref，生效值照常写 zbar-theme 并广播，跟随窗零改动；
 * ③ 币种（跟随语言 / ¥ 人民币 / $ 美元）：全局折算口径，读币种偏好上下文
 *    （真相在 Rust ~/.agenthud/currency.json，原子写），改后全部页面 +
 *    面板 + 菜单栏标题跟随（zbar://currency-changed）；
 * ④ 自动捕获新登录（默认开）：控制 accounts.rs 后台轮询线程
 *    （~/.agenthud/app-settings.json，原子写），线程每周期读开关，
 *    改完下一周期生效，无需重启；
 * ⑤ HUD 停靠条开关：与托盘右键菜单 / 面板顶部按钮同一份配置
 *    （hud-bar.json）与同一套命令 + 状态事件（zbar://hud-bar-state），
 *    三处入口状态互通。
 *
 * 刻意不做的：各页数据口径、时间范围等页面级偏好仍在各页自身；
 * 本页只收「应用级」开关，避免设置堆积。
 */
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useI18n, type MessageKey } from "../i18n";
import {
  APPEARANCE_CHANGED_EVENT,
  applyThemePref,
  loadThemePref,
  type ThemePref,
} from "../appearance";
import { getAppSettings, setAutoCapture as setAutoCaptureApi } from "../api";
import { getHudBarState, onHudBarState, setHudBarEnabled } from "../hudBar";
import { useCurrencyPref } from "./settings/currencyPref";
import type { CurrencyMode } from "../types";
import type { Locale } from "../i18n/locale";

/** 分段选项的通用描述（labelKey 全部是本页 dashboard.settings.* 词条） */
interface SegOption<T extends string> {
  value: T;
  labelKey: MessageKey;
}

/** 语言选项（zh / en） */
const LANGUAGES: SegOption<Locale>[] = [
  { value: "zh", labelKey: "dashboard.settings.languageZh" },
  { value: "en", labelKey: "dashboard.settings.languageEn" },
];

/** 主题来源选项 */
const THEMES: SegOption<ThemePref>[] = [
  { value: "light", labelKey: "dashboard.settings.themeLight" },
  { value: "dark", labelKey: "dashboard.settings.themeDark" },
  { value: "system", labelKey: "dashboard.settings.themeSystem" },
];

/** 币种选项（follow 默认 = 升级前的语言跟随口径） */
const CURRENCIES: SegOption<CurrencyMode>[] = [
  { value: "follow", labelKey: "dashboard.settings.currencyFollow" },
  { value: "cny", labelKey: "dashboard.settings.currencyCny" },
  { value: "usd", labelKey: "dashboard.settings.currencyUsd" },
];

/** 分段控件（复用页头 db-seg 形态）：通用三选/二选 */
function Segmented<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
}: {
  value: T;
  options: SegOption<T>[];
  onChange: (next: T) => void;
  ariaLabel: string;
}) {
  const { t } = useI18n();
  return (
    <div className="db-seg db-set-seg" role="radiogroup" aria-label={ariaLabel}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={value === option.value}
          className={`db-seg__btn ${value === option.value ? "db-seg__btn--active" : ""}`}
          onClick={() => onChange(option.value)}
        >
          {t(option.labelKey)}
        </button>
      ))}
    </div>
  );
}

/** 开关行控件：胶囊滑动开关（状态即语义，title 给可访问名） */
function ToggleSwitch({
  checked,
  disabled,
  title,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  title: string;
  onChange: (next: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={title}
      title={title}
      disabled={disabled}
      className={`db-set-switch ${checked ? "db-set-switch--on" : ""}`}
      onClick={() => onChange(!checked)}
    >
      <span className="db-set-switch__knob" aria-hidden />
    </button>
  );
}

export function SettingsPage() {
  const { locale, setLocale, t } = useI18n();
  const { mode: currencyMode, setMode: setCurrencyMode } = useCurrencyPref();

  // ---- 主题来源：挂载读一次；监听外观广播重读（侧栏快捷切换 / 其他窗口
  //      改主题后，本页分段选项跟着当前来源走，不停留在旧值） ----
  const [themePref, setThemePref] = useState<ThemePref>(() => loadThemePref());
  useEffect(() => {
    let dispose: (() => void) | undefined;
    let cancelled = false;
    const sync = () => setThemePref(loadThemePref());
    sync();
    listen(APPEARANCE_CHANGED_EVENT, sync)
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：保持挂载时读取的来源 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, []);

  // ---- 自动捕获开关：挂载读一次（后端文件缺失/损坏回退默认开）。
  //      写入即时生效：Rust 后台线程每周期重读开关，改完下个周期（≤60s）
  //      起效，无需重启。读取失败按后端默认口径显示「开」 ----
  const [autoCapture, setAutoCaptureValue] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then((settings) => {
        if (!cancelled) setAutoCaptureValue(settings.autoCapture);
      })
      .catch(() => {
        if (!cancelled) setAutoCaptureValue(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);
  function changeAutoCapture(next: boolean) {
    // 乐观更新 + 失败回滚（照 HUD 开关先例）
    const prev = autoCapture;
    setAutoCaptureValue(next);
    setAutoCaptureApi(next).catch(() => {
      setAutoCaptureValue(prev);
    });
  }

  // ---- HUD 停靠条：与托盘菜单 / 面板按钮共用同一状态事件流
  //      （zbar://hud-bar-state），任何一处切换本页开关同步 ----
  const [hudBar, setHudBar] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    let dispose: (() => void) | undefined;
    getHudBarState()
      .then((state) => {
        if (!cancelled) setHudBar(state.enabled);
      })
      .catch(() => {
        /* 静默：保持 null（开关禁用） */
      });
    onHudBarState((enabled) => setHudBar(enabled))
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：只失去跨入口实时性 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, []);
  function changeHudBar(next: boolean) {
    // 乐观更新 + 失败回滚（hudBar.ts 约定：失败时由调用方回滚）
    const prev = hudBar;
    setHudBar(next);
    setHudBarEnabled(next).catch(() => {
      setHudBar(prev);
    });
  }

  return (
    <div className="db-page db-set-page">
      <header className="db-page__header">
        <div>
          <h1 className="db-page__title">{t("dashboard.settings")}</h1>
          <p className="db-page__desc">{t("dashboard.settings.desc")}</p>
        </div>
      </header>

      <div className="db-set-list">
        {/* ① 语言 */}
        <section className="db-set-row">
          <div className="db-set-row__text">
            <h2 className="db-set-row__title">{t("dashboard.settings.language")}</h2>
            <p className="db-set-row__desc">{t("dashboard.settings.languageHint")}</p>
          </div>
          <Segmented
            value={locale}
            options={LANGUAGES}
            onChange={setLocale}
            ariaLabel={t("dashboard.settings.language")}
          />
        </section>

        {/* ② 外观主题 */}
        <section className="db-set-row">
          <div className="db-set-row__text">
            <h2 className="db-set-row__title">{t("dashboard.settings.theme")}</h2>
            <p className="db-set-row__desc">{t("dashboard.settings.themeHint")}</p>
          </div>
          <Segmented
            value={themePref}
            options={THEMES}
            onChange={applyThemePref}
            ariaLabel={t("dashboard.settings.theme")}
          />
        </section>

        {/* ③ 币种 */}
        <section className="db-set-row">
          <div className="db-set-row__text">
            <h2 className="db-set-row__title">{t("dashboard.settings.currency")}</h2>
            <p className="db-set-row__desc">{t("dashboard.settings.currencyHint")}</p>
          </div>
          <Segmented
            value={currencyMode}
            options={CURRENCIES}
            onChange={setCurrencyMode}
            ariaLabel={t("dashboard.settings.currency")}
          />
        </section>

        {/* ④ 自动捕获新登录 */}
        <section className="db-set-row">
          <div className="db-set-row__text">
            <h2 className="db-set-row__title">{t("dashboard.settings.autoCapture")}</h2>
            <p className="db-set-row__desc">{t("dashboard.settings.autoCaptureHint")}</p>
          </div>
          <ToggleSwitch
            checked={autoCapture === true}
            disabled={autoCapture === null}
            title={t("dashboard.settings.autoCapture")}
            onChange={changeAutoCapture}
          />
        </section>

        {/* ⑤ HUD 停靠条 */}
        <section className="db-set-row">
          <div className="db-set-row__text">
            <h2 className="db-set-row__title">{t("dashboard.settings.hudBar")}</h2>
            <p className="db-set-row__desc">{t("dashboard.settings.hudBarHint")}</p>
          </div>
          <ToggleSwitch
            checked={hudBar === true}
            disabled={hudBar === null}
            title={t("dashboard.settings.hudBar")}
            onChange={changeHudBar}
          />
        </section>
      </div>
    </div>
  );
}
