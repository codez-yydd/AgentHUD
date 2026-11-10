import { useEffect, useMemo, useRef, useState } from "react";
import {
  copyShotFile,
  deleteShot,
  editShotFile,
  getShotSettings,
  listShotHistory,
  listShotMonitors,
  openShot,
  openShotFolder,
  pinShotFile,
  prewarmShot,
  onShotSaved,
  setShotSettings,
  startShot,
} from "../shot/api";
import type { ShotHistoryItem, ShotMonitor, ShotSettings } from "../shot/types";
import { hotkeyText } from "../shortcuts";
import { Icon, SectionHead } from "./ui";

type StartMode = "shot" | "scroll" | "record";

const DEFAULT_SETTINGS: ShotSettings = {
  autoCopy: true,
  autoPin: false,
  playSound: false,
  format: "png",
  delaySec: 0,
  hotkey: "Ctrl+Alt+A",
};

function fmtAgo(ts: number) {
  const diff = Math.max(0, Date.now() - ts);
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const date = new Date(ts);
  return `${date.getMonth() + 1}/${date.getDate()} ${String(date.getHours()).padStart(2, "0")}:${String(
    date.getMinutes()
  ).padStart(2, "0")}`;
}

function fmtBytes(bytes: number) {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function fmtDim(item: ShotHistoryItem) {
  return `${item.width} × ${item.height}`;
}

export function ScreenshotPage({ onToast }: { onToast: (message: string) => void }) {
  const [settings, setSettings] = useState<ShotSettings>(DEFAULT_SETTINGS);
  const [monitors, setMonitors] = useState<ShotMonitor[]>([]);
  const [monitorIndex, setMonitorIndex] = useState<number | "current">("current");
  const [history, setHistory] = useState<ShotHistoryItem[]>([]);
  const [loadingHistory, setLoadingHistory] = useState(true);
  const [starting, setStarting] = useState<StartMode | "">("");
  const [pendingDelete, setPendingDelete] = useState("");
  const deleteTimer = useRef<number | null>(null);

  useEffect(() => {
    // 预热覆盖窗口：用户从这一页点“区域截图”时不会再等 WebView 冷启动
    void prewarmShot();
    void getShotSettings()
      .then(setSettings)
      .catch(() => {});
    void listShotMonitors()
      .then(setMonitors)
      .catch(() => {});
    void listShotHistory(6)
      .then(setHistory)
      .catch(() => {})
      .finally(() => setLoadingHistory(false));
    const off = onShotSaved((item) => {
      setHistory((current) => [item, ...current.filter((i) => i.path !== item.path)].slice(0, 6));
    });
    return () => {
      off();
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
    };
  }, []);

  const currentMonitor = useMemo(() => {
    if (monitorIndex === "current") return null;
    return monitors.find((m) => m.index === monitorIndex) ?? null;
  }, [monitorIndex, monitors]);

  const persist = (patch: Partial<ShotSettings>, message?: string) => {
    const next = { ...settings, ...patch };
    setSettings(next);
    void setShotSettings(next)
      .then((saved) => setSettings(saved))
      .catch((error) => onToast(String(error)));
    if (message) onToast(message);
  };

  const start = async (mode: StartMode) => {
    if (starting) return;
    setStarting(mode);
    try {
      // 三个分类都走同一个覆盖层，进去后还能再切。
      await startShot(
        mode === "shot" ? "region" : mode,
        settings.delaySec,
        currentMonitor ? currentMonitor.index : undefined
      );
    } catch (error) {
      onToast(String(error));
    } finally {
      setStarting("");
    }
  };

  const remove = (item: ShotHistoryItem) => {
    if (pendingDelete !== item.path) {
      setPendingDelete(item.path);
      onToast("再点一次删除这张截图");
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
      deleteTimer.current = window.setTimeout(() => setPendingDelete(""), 2400);
      return;
    }
    setPendingDelete("");
    void deleteShot(item.path)
      .then(() => {
        setHistory((current) => current.filter((i) => i.path !== item.path));
        onToast("截图已删除");
      })
      .catch((error) => onToast(String(error)));
  };

  const copyHistory = (item: ShotHistoryItem) => {
    void copyShotFile(item.path)
      .then(() => onToast("已复制到剪贴板"))
      .catch((error) => onToast(String(error)));
  };

  const pinHistory = (item: ShotHistoryItem) => {
    void pinShotFile(item.path)
      .then(() => onToast("已贴到桌面"))
      .catch((error) => onToast(String(error)));
  };

  const editHistory = (item: ShotHistoryItem) => {
    void editShotFile(item.path).catch((error) => onToast(String(error)));
  };

  return (
    <div className="page shot-page">
      <SectionHead
        title="截图"
        hint="一个快捷键呼出，之后在截图条里切换 截图 / 滚动截图 / 录屏"
        action={
          <button
            type="button"
            className="mini-icon-btn"
            title="打开截图文件夹"
            aria-label="打开截图文件夹"
            onClick={() => void openShotFolder().catch((e) => onToast(String(e)))}
          >
            <Icon name="folder" size={13} />
          </button>
        }
      />

      <div className="shot-launch-grid">
        <button
          type="button"
          className={`shot-launch primary wide ${starting === "shot" ? "busy" : ""}`}
          disabled={Boolean(starting)}
          onClick={() => void start("shot")}
        >
          <span className="shot-launch-icon">
            <Icon name="camera" size={20} />
          </span>
          <span className="shot-launch-text">
            <b>截图</b>
            <em>拖动框选 · 单击选窗口 · 桌面单击整屏</em>
          </span>
          <kbd>{hotkeyText(settings.hotkey)}</kbd>
        </button>
        <button
          type="button"
          className={`shot-launch ${starting === "scroll" ? "busy" : ""}`}
          disabled={Boolean(starting)}
          onClick={() => void start("scroll")}
        >
          <span className="shot-launch-icon">
            <Icon name="scroll" size={19} />
          </span>
          <span className="shot-launch-text">
            <b>滚动截图</b>
            <em>框选后自动滚动拼接</em>
          </span>
        </button>
        <button
          type="button"
          className={`shot-launch record ${starting === "record" ? "busy" : ""}`}
          disabled={Boolean(starting)}
          onClick={() => void start("record")}
        >
          <span className="shot-launch-icon">
            <span className="shot-launch-rec" />
          </span>
          <span className="shot-launch-text">
            <b>录屏</b>
            <em>MP4 / GIF，最长 5 分钟</em>
          </span>
        </button>
      </div>

      <div className="shot-options">
        <label className="shot-option" title="截图所在显示器">
          <Icon name="monitor" size={12} />
          <select
            value={monitorIndex}
            onChange={(event) => {
              const value = event.target.value;
              setMonitorIndex(value === "current" ? "current" : Number(value));
            }}
          >
            <option value="current">当前屏幕</option>
            {monitors.map((monitor) => (
              <option key={monitor.index} value={monitor.index}>
                {monitor.primary ? "主屏" : `屏 ${monitor.index + 1}`} · {monitor.width}×{monitor.height}
              </option>
            ))}
          </select>
        </label>
        <label className="shot-option" title="延时后自动抓屏">
          <Icon name="timer" size={12} />
          <select
            value={settings.delaySec}
            onChange={(event) => persist({ delaySec: Number(event.target.value) })}
          >
            <option value={0}>无延迟</option>
            <option value={3}>延时 3 秒</option>
            <option value={5}>延时 5 秒</option>
            <option value={10}>延时 10 秒</option>
          </select>
        </label>
        <div className="shot-format" role="group" aria-label="保存格式">
          {(["png", "jpg"] as const).map((format) => (
            <button
              key={format}
              type="button"
              className={settings.format === format ? "active" : ""}
              onClick={() => persist({ format }, `保存格式：${format.toUpperCase()}`)}
            >
              {format.toUpperCase()}
            </button>
          ))}
        </div>
      </div>

      <div className="shot-toggles">
        <button
          type="button"
          className={settings.autoCopy ? "active" : ""}
          onClick={() => persist({ autoCopy: !settings.autoCopy })}
        >
          <Icon name="copy" size={11} />
          保存后复制
        </button>
        <button
          type="button"
          className={settings.autoPin ? "active" : ""}
          onClick={() => persist({ autoPin: !settings.autoPin })}
        >
          <Icon name="pin" size={11} />
          保存后贴图
        </button>
        <button
          type="button"
          className={settings.playSound ? "active" : ""}
          onClick={() => persist({ playSound: !settings.playSound })}
        >
          <Icon name="volume" size={11} />
          提示音
        </button>
      </div>

      <div className="shot-history-head">
        <div className="shot-history-title">
          <Icon name="image" size={13} />
          <span>最近截图</span>
          <em>{history.length}</em>
        </div>
        <button type="button" onClick={() => void openShotFolder().catch((e) => onToast(String(e)))}>
          打开文件夹
        </button>
      </div>

      {loadingHistory ? (
        <div className="shot-history-loading">
          <span className="shot-skeleton" />
          <span className="shot-skeleton" />
        </div>
      ) : history.length === 0 ? (
        <div className="shot-history-empty">
          <span>
            <Icon name="camera" size={18} />
          </span>
          <div>
            <b>还没有截图</b>
            <em>从上面的区域截图开始，标注后会出现在这里</em>
          </div>
        </div>
      ) : (
        <div className="shot-history-list">
          {history.map((item) => (
            <div
              className={`shot-history-item ${pendingDelete === item.path ? "pending-delete" : ""}`}
              key={item.path}
            >
              <button
                type="button"
                className="shot-history-thumb"
                title="打开截图"
                onClick={() => void openShot(item.path).catch((e) => onToast(String(e)))}
              >
                <img src={item.thumb} alt="" />
              </button>
              <button
                type="button"
                className="shot-history-main"
                title={item.path}
                onClick={() => void openShot(item.path).catch((e) => onToast(String(e)))}
              >
                <span className="shot-history-name">{item.name.replace(/^Shot_/, "")}</span>
                <span className="shot-history-meta">
                  {fmtDim(item)} · {fmtAgo(item.createdAt)} · {fmtBytes(item.size)}
                </span>
              </button>
              <div className="shot-history-actions">
                <button type="button" title="编辑标注" onClick={() => editHistory(item)}>
                  <Icon name="edit" size={12} />
                </button>
                <button type="button" title="复制" onClick={() => copyHistory(item)}>
                  <Icon name="copy" size={12} />
                </button>
                <button type="button" title="贴图" onClick={() => pinHistory(item)}>
                  <Icon name="pin" size={12} />
                </button>
                <button
                  type="button"
                  className={pendingDelete === item.path ? "danger" : ""}
                  title={pendingDelete === item.path ? "再次点击删除" : "删除"}
                  onClick={() => remove(item)}
                >
                  <Icon name="trash" size={12} />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="shot-page-foot">
        <Icon name="camera" size={10} />
        <span>{monitors.length || 1} 个显示器 · 本地 OCR / 二维码 / 录屏</span>
        <span>·</span>
        <span>保存到 图片/AgentHUD Screenshots</span>
      </div>
    </div>
  );
}
