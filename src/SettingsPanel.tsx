import { useCallback, useEffect, useRef, useState } from "react";
import { openIslandSettings, openMainWindow } from "./api";
import {
  commitPanelFontPx,
  loadPanelFontPx,
  previewFontPx,
  resetFontPx,
} from "./fontSettings";
import { FontSizeControl } from "./FontSizeControl";
import {
  WIN_PRESETS,
  BASE_PCT,
  loadWinSizePct,
  persistWinSizePct,
  applyWindowPct,
  type WinSizePct,
} from "./windowSize";
import { WIN_SIZE_CHANGED_EVENT } from "./ResizeHandles";
import { UPDATE_READY_KEY, UPDATE_READY_EVENT } from "./updater";
import {
  PageShell,
  PageHeader,
  PageBody,
  SettingsCard,
  PillGroup,
  PillButton,
  BtnPrimary,
  AlertBanner,
} from "./layout";
import { useI18n } from "./i18n";

interface Props {
  onBack: () => void;
}

/**
 * 设置页（悬浮窗）：只保留悬浮窗自身的窗口级设置——基准字号 / 窗口大小 /
 * 语言 / 灵动岛设置入口。
 * ZCode 账号切换与关于与更新两卡已迁至主窗口（凭证与账号页 / 设置页），
 * 本页仅在更新就绪时底部给一条去主窗口安装的引导（红点仍在顶栏菜单入口）。
 * 应用级配置（面板透明度、悬浮窗开关、开机自启、数据来源、重置时间展示、
 * 汇率、网络代理、全局快捷键）已全部迁至主窗口设置页，本页不再是其入口。
 * 单列滚动，改完即存（无整页保存按钮）。
 */
export function SettingsPanel({ onBack }: Props) {
  const { locale, t, setLocale } = useI18n();
  const [error, setError] = useState<string | null>(null);

  // ===== 更新就绪引导：设置页不再挂更新卡（检查/下载/安装全部迁至主窗口
  // 设置页），但顶栏菜单设置入口的红点（StatsPanel）仍按同一份 localStorage
  // 标记点亮；这里同步读标记，就绪时给用户明确去处，无更新时不渲染 =====
  const [updateReady, setUpdateReady] = useState(() => {
    try {
      return Boolean(localStorage.getItem(UPDATE_READY_KEY));
    } catch {
      return false;
    }
  });
  useEffect(() => {
    const sync = () => {
      try {
        setUpdateReady(Boolean(localStorage.getItem(UPDATE_READY_KEY)));
      } catch {
        /* 忽略存储异常 */
      }
    };
    window.addEventListener(UPDATE_READY_EVENT, sync);
    return () => window.removeEventListener(UPDATE_READY_EVENT, sync);
  }, []);

  // 打开主窗口（照 StatsPanel 的 openMainWindowWithToast 模式）：失败在提示
  // 行内轻提示 2.4s 自清（建窗失败不打断本页交互），成功清掉失败提示
  const [mainOpenFailed, setMainOpenFailed] = useState(false);
  const mainOpenTimerRef = useRef<number | undefined>(undefined);
  useEffect(
    () => () => {
      window.clearTimeout(mainOpenTimerRef.current);
    },
    []
  );
  const openMainWindowWithToast = useCallback((page?: string) => {
    openMainWindow(page)
      .then(() => {
        window.clearTimeout(mainOpenTimerRef.current);
        mainOpenTimerRef.current = undefined;
        setMainOpenFailed(false);
      })
      .catch(() => {
        setMainOpenFailed(true);
        window.clearTimeout(mainOpenTimerRef.current);
        mainOpenTimerRef.current = window.setTimeout(() => {
          mainOpenTimerRef.current = undefined;
          setMainOpenFailed(false);
        }, 2400);
      });
  }, []);

  // ===== 外观：基准字号（逻辑 px；旧 zbar-ui-scale 档位自动迁移）=====
  const [fontPx, setFontPx] = useState<number>(loadPanelFontPx);
  // ===== 外观：窗口大小（工作区百分比，档位见 WIN_PRESETS；null = 从未调整过，视同标准档）=====
  const [winPct, setWinPct] = useState<WinSizePct | null>(loadWinSizePct);
  // 边缘拖拽在设置页可见时落盘后同步档位胶囊：ResizeHandles 每次成功
  // 落盘会广播该事件，这里重读 localStorage 刷新高亮 / 自定义态
  useEffect(() => {
    const onWinSizeChanged = () => setWinPct(loadWinSizePct());
    window.addEventListener(WIN_SIZE_CHANGED_EVENT, onWinSizeChanged);
    return () =>
      window.removeEventListener(WIN_SIZE_CHANGED_EVENT, onWinSizeChanged);
  }, []);

  // 窗口大小档位匹配：winPct 与「BASE_PCT × scale」的 w/h 差值均在容差内即视为该档位
  // （拖拽落盘与理论档位存在像素圆整误差，0.005 容差覆盖 4 位小数存储精度）
  const matchesPreset = (pct: WinSizePct | null, scale: number): boolean =>
    pct !== null &&
    Math.abs(pct.w - BASE_PCT.w * scale) <= 0.005 &&
    Math.abs(pct.h - BASE_PCT.h * scale) <= 0.005;

  return (
    <PageShell>
      <PageHeader title={t("settings.title")} onBack={onBack} />

      <PageBody className="page-stack">
        {error && <AlertBanner>{error}</AlertBanner>}

        {/* 基准字号：小/标准/大/特大为旧档位等比迁移值，自定义支持 10~24px
            （0.5px 步进）；输入过程实时预览，回车/失焦确认并落盘 */}
        <SettingsCard
          title={t("font.panelTitle")}
          hint={t("font.panelHint")}
        >
          <FontSizeControl
            target="panel"
            value={fontPx}
            onChange={(px) => setFontPx(commitPanelFontPx(px))}
            onPreview={(px) => previewFontPx("panel", px)}
            onReset={() => setFontPx(resetFontPx("panel"))}
          />
        </SettingsCard>

        {/* 窗口大小：工作区百分比档位（见 windowSize.ts 的 WIN_PRESETS）；
            点击先落盘理论值（档位判定以其为准），再异步应用 setSize + 右边界夹取 */}
        <SettingsCard
          title={t("settings.winSize")}
          hint={t("settings.winSizeHint")}
        >
          <PillGroup>
            {WIN_PRESETS.map((p) => (
              <PillButton
                key={p.scale}
                active={
                  winPct === null
                    ? p.scale === 1
                    : matchesPreset(winPct, p.scale)
                }
                onClick={() => {
                  const pct = { w: BASE_PCT.w * p.scale, h: BASE_PCT.h * p.scale };
                  persistWinSizePct(pct);
                  void applyWindowPct(pct);
                  setWinPct(pct);
                }}
              >
                {t(p.labelKey)}
              </PillButton>
            ))}
            {/* 自定义态：已调整过但不匹配任何档位（边缘拖拽的结果），四个档位均不 active */}
            {winPct !== null && !WIN_PRESETS.some((p) => matchesPreset(winPct, p.scale)) && (
              <PillButton active={false} disabled>
                {t("settings.winSizeCustom")}
              </PillButton>
            )}
          </PillGroup>
        </SettingsCard>

        {/* 语言：与顶栏 LanguageToggle 读写同一 Context，切换后全站即时同步。
            位置紧随字号 / 窗口大小，构成外观三连（字号 / 大小 / 语言） */}
        <SettingsCard title={t("settings.language")}>
          <PillGroup>
            <PillButton
              active={locale === "zh"}
              onClick={() => setLocale("zh")}
            >
              {t("settings.langZh")}
            </PillButton>
            <PillButton
              active={locale === "en"}
              onClick={() => setLocale("en")}
            >
              {t("settings.langEn")}
            </PillButton>
          </PillGroup>
        </SettingsCard>

        {/* 灵动岛设置入口：灵动岛开启鼠标穿透后右键菜单不可达，设置窗关闭后
            只能从这里重新打开（否则透明度/穿透等设置被锁死无法调整） */}
        <SettingsCard
          title={t("settings.islandSettings")}
          action={
            <BtnPrimary
              onClick={() =>
                openIslandSettings().catch((e) => setError(String(e)))
              }
            >
              {t("settings.islandSettingsOpen")}
            </BtnPrimary>
          }
        >
          <p className="text-[9px] text-slate-500 leading-relaxed">
            {t("settings.islandSettingsHint")}
          </p>
        </SettingsCard>

        {/* 桌面宠物设置已收敛到皮肤页（ThemePanel）的宠物卡：总开关 +
            注入版/悬浮窗形态二选一（pet.json 唯一真相源），本页不再重复 */}

        {/* ZCode 账号切换与关于与更新两卡已迁至主窗口（凭证与账号页 /
            关于页），本页不再挂载；更新就绪时在底部给一条去主窗口关于页
            安装的引导（顶栏菜单设置入口的红点仍照常点亮），无更新时不渲染 */}
        {updateReady && (
          <div className="flex items-center justify-between gap-2 rounded-md bg-sky-500/10 px-2 py-1.5">
            <span className="text-[9px] leading-relaxed text-sky-700/90">
              {t("settings.updateReadyMainHint")}
            </span>
            <span className="flex shrink-0 items-center gap-1.5">
              {mainOpenFailed && (
                <span className="text-[9px] text-rose-600">
                  {t("settings.openMainWindowFail")}
                </span>
              )}
              <button
                onClick={() => openMainWindowWithToast("about")}
                className="rounded bg-sky-500 px-1.5 py-0.5 text-[9px] text-white transition-colors hover:bg-sky-600"
              >
                {t("settings.openMainWindow")}
              </button>
            </span>
          </div>
        )}
      </PageBody>
    </PageShell>
  );
}
