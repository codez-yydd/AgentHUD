/**
 * 主窗口设置页「悬浮窗」分区：悬浮窗开关 + 面板透明度（自悬浮窗设置面板
 * 迁移；透明度作用对象是悬浮窗自身，主窗口只是配置宿主）。
 *
 * - 悬浮窗开关：关闭即销毁 panel 窗口（省内存），开启或托盘呼出时由 Rust
 *   侧现场建窗。与悬浮窗内旧版不同，本窗口不在 panel 窗口里运行，关闭
 *   分支的响应不会因窗口自销毁而丢失，失败回滚可靠成立；
 * - 面板透明度：滑杆写 localStorage（zbar-panel-alpha）并广播
 *   zbar://panel-alpha-changed，悬浮窗监听后重读并应用 --panel-alpha
 *   实时跟随（主窗口自身不消费该变量）。
 */
import { useEffect, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import { useI18n } from "../../i18n";
import {
  getPanelEnabled,
  // 别名避免与本组件 panelEnabled 的 useState setter setPanelEnabled 撞名
  setPanelEnabled as savePanelSwitch,
} from "../../api";
import {
  PANEL_ALPHA_CHANGED_EVENT,
  loadPanelAlpha,
  persistPanelAlpha,
} from "../../appearance";
import { ToggleSwitch } from "./controls";

export function PanelSettings() {
  const { t } = useI18n();

  // ===== 悬浮窗启用开关（关闭即销毁 panel 窗口，释放 WebView 渲染进程）=====
  const [panelEnabled, setPanelEnabled] = useState(true);
  const [panelLoaded, setPanelLoaded] = useState(false);
  const [savingPanel, setSavingPanel] = useState(false);
  const [panelError, setPanelError] = useState<string | null>(null);

  // ===== 面板透明度（写盘 + 跨窗广播，悬浮窗实时应用）=====
  const [alpha, setAlpha] = useState<number>(() => loadPanelAlpha());

  useEffect(() => {
    getPanelEnabled()
      .then(setPanelEnabled)
      .catch((e) =>
        setPanelError(t("settings.floatWindowReadFail", { msg: String(e) }))
      )
      .finally(() => setPanelLoaded(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 应用悬浮窗开关：开启 → Rust 侧现场建窗并显示聚焦；关闭 → 销毁 panel
  // 窗口。本窗口（主窗口）不随 panel 销毁，Promise 正常返回；乐观更新 +
  // 失败回滚（仅建窗/销窗被拒时窗口仍在）
  const handlePanelEnabledChange = async (enabled: boolean) => {
    setSavingPanel(true);
    setPanelError(null);
    setPanelEnabled(enabled);
    try {
      await savePanelSwitch(enabled);
    } catch (e) {
      setPanelEnabled(!enabled);
      setPanelError(
        enabled
          ? t("settings.floatWindowFailOn", { msg: String(e) })
          : t("settings.floatWindowFailOff", { msg: String(e) })
      );
    } finally {
      setSavingPanel(false);
    }
  };

  // 透明度提交：写盘 + 跨窗广播（悬浮窗重读后应用）。拖动步进 0.05、
  // 全程最多 16 档，逐档落盘与广播的开销可忽略（localStorage 写入 + 一次
  // 事件 emit），悬浮窗跟随无感延迟；离开本页无需冲刷（每档已落盘）
  const commitAlpha = (v: number) => {
    setAlpha(v);
    persistPanelAlpha(v);
    emit(PANEL_ALPHA_CHANGED_EVENT).catch(() => {
      /* 静默：悬浮窗下次启动仍会读取持久化值 */
    });
  };

  return (
    <>
      {/* 悬浮窗开关 */}
      <section className="db-set-row">
        <div className="db-set-row__text">
          <h2 className="db-set-row__title">{t("settings.floatWindow")}</h2>
          <p className="db-set-row__desc">{t("settings.floatWindowHint")}</p>
          {!panelLoaded && (
            <p className="db-set-note">{t("settings.readingState")}</p>
          )}
          {panelError && (
            <>
              <p className="db-set-note db-set-note--error">{panelError}</p>
              {/* 开关操作失败可能是超时（配置已落盘、窗口动作未确认），
                  UI 回滚态与磁盘状态可能短暂不一致，提示以重启为准 */}
              <p className="db-set-note">{t("settings.floatWindowFailNote")}</p>
            </>
          )}
        </div>
        <ToggleSwitch
          checked={panelEnabled}
          disabled={!panelLoaded || savingPanel}
          title={t("settings.floatWindow")}
          onChange={handlePanelEnabledChange}
        />
      </section>

      {/* 面板透明度：滑杆实时广播，悬浮窗即时跟随 */}
      <section className="db-set-row db-set-row--block">
        <div className="db-set-row__text">
          <h2 className="db-set-row__title">{t("settings.panelOpacity")}</h2>
          <p className="db-set-row__desc">{t("settings.opacityHint")}</p>
        </div>
        <div className="db-iset-sliderbar">
          <span className="db-set-inline__label">{t("settings.opacity")}</span>
          <input
            type="range"
            className="db-iset-range"
            min="0.2"
            max="1"
            step="0.05"
            value={alpha}
            onChange={(e) => commitAlpha(parseFloat(e.target.value))}
            style={
              {
                "--range-pct": `${((alpha - 0.2) / 0.8) * 100}%`,
              } as React.CSSProperties
            }
            aria-label={t("settings.panelOpacity")}
          />
          <span className="db-iset-value">{Math.round(alpha * 100)}%</span>
        </div>
      </section>
    </>
  );
}
