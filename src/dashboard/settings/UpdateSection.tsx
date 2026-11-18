/**
 * 主窗口「软件更新」共享区块：检查更新 / 后台下载进度 / 重启安装 / 失败兜底。
 * 「关于」页与「设置」页共用——AboutPage 原样渲染（showVersion 缺省 false，
 * 版本号由其品牌区胶囊展示）；SettingsPage 在「关于与更新」分区传
 * showVersion 额外显示一行当前版本（分区标题 db-set-group 由调用方渲染）。
 *
 * 更新的执行全部委托面板（面板是唯一更新执行者：Update 资源存于面板
 * webview 资源表，rid 在主窗口无效；主窗口值导入 updater.ts 还会清掉面板
 * 写入的红点标记）——本组件只经跨窗口事件桥（updaterEvents.ts，宿主见
 * updaterBridge.ts）订阅面板广播的状态、发送命令，语义与面板 UpdaterCard
 * 对齐但用 dashboard 常规字号；链接兜底经 opener 插件在本机浏览器打开
 * （capability 见 src-tauri/capabilities/dashboard.json 的 opener:default）。
 */
import { useEffect, useRef, useState } from "react";
import type { ReactElement } from "react";
import { emitTo, listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useI18n } from "../../i18n";
import {
  REMOTE_UPDATE_COMMAND_EVENT,
  REMOTE_UPDATE_STATE_EVENT,
  type RemoteUpdateCommand,
  type RemoteUpdatePhase,
  type RemoteUpdateState,
} from "../../updaterEvents";
import { RefreshIcon } from "../icons";

/** 面板窗口 label（与 capabilities/default.json 的 windows 白名单一致） */
const PANEL_TARGET = "panel";

/** 面板 UpdaterCard 的下载页（安装失败 / 未响应时的兜底去处） */
const DOWNLOAD_PAGE = "https://gitee.com/codezwx/AgentHUD/releases";

/** 乐观 checking 的兜底时限：面板未响应（桥未挂载等极端情况）时回落文案 */
const CHECK_FALLBACK_MS = 3000;

/**
 * 面板更新状态订阅：挂载时监听状态广播，并向面板发 sync 请求当前状态
 * （面板桥宿主收到 sync 后强制重发，覆盖「先开主窗口、面板早已启动」的
 * 时序）。返回 [state, send]：state 为 null 表示尚未收到任何广播；send
 * 把命令定向发给面板（emitTo 失败静默——更新区块退化为不可用，页面其余
 * 部分不受影响）。
 */
function useRemoteUpdateState(): [
  RemoteUpdateState | null,
  (command: RemoteUpdateCommand) => void,
] {
  const [state, setState] = useState<RemoteUpdateState | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlistenState: (() => void) | undefined;
    listen<RemoteUpdateState>(REMOTE_UPDATE_STATE_EVENT, (event) => {
      if (!disposed) setState(event.payload);
    })
      .then((unlisten) => {
        // effect 已清理（StrictMode 双挂载 / 快速卸载）时立即退订
        if (disposed) unlisten();
        else unlistenState = unlisten;
      })
      .catch(() => {
        /* 静默：收不到广播时更新区块停留在「未收到状态」形态 */
      });
    // 请求面板立即广播当前状态（幂等：面板桥按序列化去重也保证无副作用）。
    // 失败只 warn 不阻断：悬浮窗开关关闭（panel 被销毁）后这里是常态失败
    //（自动更新检查暂停，见设置页开关文案），留日志便于区分「面板没开」
    // 与「事件通道异常」
    emitTo(PANEL_TARGET, REMOTE_UPDATE_COMMAND_EVENT, "sync").catch((e) => {
      console.warn("[update] 面板不在线，更新状态同步请求未送达:", e);
    });
    return () => {
      disposed = true;
      unlistenState?.();
    };
  }, []);

  const send = (command: RemoteUpdateCommand) => {
    // 同上：panel 不在线（悬浮窗开关关闭）时命令不可达，warn 留痕
    emitTo(PANEL_TARGET, REMOTE_UPDATE_COMMAND_EVENT, command).catch((e) => {
      console.warn(`[update] 更新命令 ${command} 未送达（面板不在线）:`, e);
    });
  };

  return [state, send];
}

export function UpdateSection({ showVersion = false }: { showVersion?: boolean }) {
  const { t } = useI18n();
  const [remoteState, send] = useRemoteUpdateState();
  // 当前版本（getVersion 失败显示 "—"，照面板 UpdaterCard 同口径）；仅
  // showVersion（设置页分区）时展示，AboutPage 的版本号常驻其品牌区胶囊
  const [version, setVersion] = useState("");
  // 乐观 checking：尚未收到过面板广播时点「检查更新」的本地占位——3 秒内
  // 没有收到任何广播则回落「面板未响应」兜底；一旦收到过广播即永久解除
  // （remoteState 非空后 handleCheck 不再启动兜底计时）
  const [optimisticChecking, setOptimisticChecking] = useState(false);
  const [fallbackDue, setFallbackDue] = useState(false);
  const fallbackTimerRef = useRef<number | undefined>(undefined);

  useEffect(() => {
    if (!showVersion) return;
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
  }, [showVersion]);

  // 面板广播到达：乐观占位与兜底全部让位给面板真实状态
  useEffect(() => {
    if (remoteState) {
      setOptimisticChecking(false);
      setFallbackDue(false);
    }
  }, [remoteState]);

  // 卸载时清兜底计时（StrictMode 双挂载下旧计时器不残留）
  useEffect(() => {
    return () => window.clearTimeout(fallbackTimerRef.current);
  }, []);

  const handleCheck = () => {
    send("check");
    if (remoteState) return;
    setOptimisticChecking(true);
    setFallbackDue(false);
    window.clearTimeout(fallbackTimerRef.current);
    fallbackTimerRef.current = window.setTimeout(() => {
      setOptimisticChecking(false);
      setFallbackDue(true);
    }, CHECK_FALLBACK_MS);
  };

  // 打开外链：opener 插件失败时兜底 window.open（照面板 UpdaterCard 先例）
  const openLink = (url: string) => {
    openUrl(url).catch(() => window.open(url, "_blank"));
  };

  // 面板真实状态（null = 尚未收到广播）；乐观 checking 只在 null 期间接管
  const phase: RemoteUpdatePhase | null = remoteState
    ? remoteState.phase
    : optimisticChecking
      ? "checking"
      : null;
  const progress = remoteState?.progress ?? 0;
  const readyVersion = remoteState?.readyVersion ?? null;
  const releaseNoteText = remoteState?.releaseNote?.trim() || null;
  const error = remoteState?.error ?? null;

  // 卡片头 action 槽按阶段分派：busy / ready 显示不可点状态文字（槽位与
  // 按钮同高防跳动），其余阶段显示可点「检查更新」——updater.ts 在 ready
  // 态对 check 命令直接短路返回且无状态广播，可点按钮只会是死交互；ready
  // 态全页唯一主按钮是「重启安装」。checking 照总览页加载态先例：按钮不
  // 卸载，RefreshIcon 旋转 + 置灰（避免行高跳动与键盘焦点丢失）
  let headAction: ReactElement;
  switch (phase) {
    case "checking":
      headAction = (
        <button type="button" className="db-btn db-btn--primary" disabled>
          <RefreshIcon className="db-spin h-3.5 w-3.5" />
          {t("dashboard.about.checking")}
        </button>
      );
      break;
    case "downloading":
      // 下载中文字带实时百分比（复用进度条旁的同一词条）
      headAction = (
        <span className="db-about-phase num">
          {t("dashboard.about.downloading", { pct: progress })}
        </span>
      );
      break;
    case "installing":
      headAction = (
        <span className="db-about-phase">{t("dashboard.about.installing")}</span>
      );
      break;
    case "ready":
      headAction = (
        <span className="db-about-phase">{t("dashboard.about.readyShort")}</span>
      );
      break;
    default:
      headAction = (
        <button
          type="button"
          className="db-btn db-btn--primary"
          onClick={handleCheck}
        >
          {t("dashboard.about.checkUpdate")}
        </button>
      );
  }

  return (
    <section className="db-card db-about-card">
      <div className="db-about-card__head">
        <h2 className="db-about-card__title">
          {t("dashboard.about.updateTitle")}
        </h2>
        {headAction}
      </div>

      {/* 当前版本行：仅设置页分区显示（showVersion）；AboutPage 的版本号
          常驻品牌区胶囊，不在此重复 */}
      {showVersion && (
        <div className="db-about-statusrow">
          <span>{t("settings.currentVersion")}</span>
          <span className="num">{version ? `v${version}` : "—"}</span>
        </div>
      )}

      {/* 检查结论行：只在「已是最新」时渲染——发现新版本的版本信息已并入
          downloading / ready 的状态句，版本号常驻品牌区胶囊，结论行不再
          重复（error 态也不渲染，避免与红色失败信息冲突） */}
      {phase === "uptodate" && (
        <div className="db-about-statusrow">
          <span className="db-about-ok">
            <span aria-hidden>✓</span>{" "}
            {t("dashboard.about.upToDate")}
          </span>
        </div>
      )}

      {/* 面板未响应兜底：点了检查但 3 秒内没有任何广播（桥不可用的极端
          情况），提示可稍后重试或直接打开下载页 */}
      {remoteState === null && fallbackDue && (
        <p className="db-about-fallback">
          {t("dashboard.about.panelUnresponsive")}
          <button
            type="button"
            className="db-about-fallback__link"
            onClick={() => openLink(DOWNLOAD_PAGE)}
          >
            {t("dashboard.about.openDownloadPage")}
          </button>
        </p>
      )}

      {phase === "downloading" && (
        <div className="db-about-progresswrap">
          <p className="db-about-status">
            {t("dashboard.about.backgroundDownloading", {
              v: remoteState?.pendingVersion ?? "",
            })}
          </p>
          <div className="db-about-progressline">
            <div
              className="db-about-progress"
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={progress}
            >
              <div
                className="db-about-progress__bar"
                style={{ width: `${progress}%` }}
              />
            </div>
            <span className="num">
              {t("dashboard.about.downloading", { pct: progress })}
            </span>
          </div>
        </div>
      )}

      {phase === "ready" && readyVersion && (
        <div className="db-about-ready">
          <p className="db-about-status db-about-status--accent">
            {t("dashboard.about.updateReady", { v: readyVersion })}
          </p>
          <div className="db-about-btnrow">
            <button
              type="button"
              className="db-btn db-btn--primary"
              onClick={() => send("install")}
            >
              {t("dashboard.about.restartUpdate")}
            </button>
            <button
              type="button"
              className="db-btn"
              onClick={() => openLink(DOWNLOAD_PAGE)}
            >
              {t("dashboard.about.openDownloadPage")}
            </button>
          </div>
          {releaseNoteText && (
            <pre className="db-about-note">{releaseNoteText}</pre>
          )}
        </div>
      )}

      {phase === "installing" && (
        <p className="db-about-status">{t("dashboard.about.installing")}</p>
      )}

      {phase === "error" && error && (
        <div className="db-about-error">
          <p className="db-about-error__text">
            {t("dashboard.about.updateFailed", { msg: error })}
          </p>
          <button
            type="button"
            className="db-btn"
            onClick={() => openLink(DOWNLOAD_PAGE)}
          >
            {t("dashboard.about.openDownloadPage")}
          </button>
        </div>
      )}

      {/* 更新源说明小字：双 endpoint（GitHub / Gitee 自动降级），用主窗口
          专用词条（不再提面板设置入口红点——红点只在面板存在） */}
      <p className="db-about-hint">{t("dashboard.about.updateHint")}</p>
    </section>
  );
}
