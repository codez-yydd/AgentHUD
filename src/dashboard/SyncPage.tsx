/**
 * 同步与设备页（主窗口第六页）：大屏版的同步与设备管理中心。
 *
 * 三个区块：
 * ① 本机同步状态卡——未注册给注册引导表单，已注册给状态网格（连通 /
 *    上次同步 / 待上传）+ 模式行（manual/auto 即改即存 + auto 间隔 blur
 *    提交）+ 操作行（立即同步 / 暂停开启 / 断开连接）；
 * ② 服务端设备表——master token 解锁管理操作（改名 / 合并 / 删数据），
 *    数据源是 listRemoteDevices 的原始结果（不过滤零记录设备，与筛选下拉的
 *    selectableRemoteDevices 口径不同）；
 * ③ 服务端数据管理——自动清理配置 + 三个危险清理（全部走确认弹层）。
 *
 * 数据获取：本页自己管理（不走 syncConfig.ts 的模块缓存，刷新即重读）；
 * 配置 / 设备 / 清理状态 / 连通探测各自独立容错，一个失败不拖垮整页；
 * refreshAll 带请求序号守卫（loadReqId，照 ReportPanel 先例），旧请求的
 * 响应不覆盖新请求。对齐总览页不做自动轮询，刷新按钮拉全量。
 * 配置读失败（无任何数据）走专用错误重试态，防止已注册用户被误导重注册。
 *
 * SyncConfig 写回铁律（照 SyncPanel.tsx 改名回写的事故注释先例）：改任何
 * 配置字段必须「先 getSyncConfig 取最新 → 只覆盖目标字段 → setSyncConfig
 * 写回」。绝不能用打开页面时的旧快照全量写回——后台同步线程会推进
 * last_sync_at 与各来源上传游标，全量写回会把游标回退造成一批记录重复上传
 * （Rust 侧 upload_incremental 结尾另有落盘竞态防护，两端双保险）。
 */
import { useEffect, useRef, useState } from "react";
import { useI18n, type TFn } from "../i18n";
import { AlertBanner } from "../layout";
import { DropdownMenu } from "../DropdownMenu";
import { Card, Skeleton } from "./primitives";
import { RefreshIcon } from "./icons";
import type {
  CleanupResult,
  CleanupStatus,
  DeviceInfo,
  RegisterRequest,
  SyncConfig,
  SyncMode,
  SyncPingResult,
} from "../types";
import {
  cleanupServer,
  disconnectDevice,
  getCleanupStatus,
  getSyncConfig,
  listRemoteDevices,
  mergeDevices,
  pendingUploadCount,
  pingSyncServer,
  registerDevice,
  renameDevice,
  setAutoCleanup,
  setSyncConfig,
  syncNow,
} from "../api";

/** 相对当前时间的友好描述（词条走本页 dashboard.sync.*） */
function timeAgo(t: TFn, ms: number | null | undefined): string {
  if (!ms) return t("dashboard.sync.never");
  const diff = Date.now() - ms;
  const min = Math.floor(diff / 60000);
  if (min < 1) return t("dashboard.sync.justNow");
  if (min < 60) return t("dashboard.sync.minAgo", { n: min });
  const hr = Math.floor(min / 60);
  if (hr < 24) return t("dashboard.sync.hourAgo", { n: hr });
  return t("dashboard.sync.dayAgo", { n: Math.floor(hr / 24) });
}

/** 本地时区绝对时间（YYYY-MM-DD HH:mm），用于 title 悬停与注册时间列 */
function formatDateTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}`;
}

/** 确认弹层状态（五种危险操作共用一个弹层形态） */
type ConfirmState =
  | { kind: "disconnect" }
  | { kind: "device"; device: DeviceInfo }
  | { kind: "before" }
  | { kind: "all" }
  | { kind: "reset" };

export function SyncPage() {
  const { t } = useI18n();

  // ---- 数据（各块独立容错）----
  const [config, setConfig] = useState<SyncConfig | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [pending, setPending] = useState(0);
  const [ping, setPing] = useState<SyncPingResult | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[] | null>(null);
  const [devicesError, setDevicesError] = useState<string | null>(null);
  const [cleanupStatus, setCleanupStatus] = useState<CleanupStatus | null>(
    null
  );
  const [cleanupError, setCleanupError] = useState<string | null>(null);

  // ---- master token（解锁设备管理 / 数据清理；只在内存，不落盘）----
  const [masterInput, setMasterInput] = useState("");
  const [showMaster, setShowMaster] = useState(false);
  const masterReady = masterInput.trim().length > 0;

  // ---- 操作状态 ----
  const [busy, setBusy] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [registering, setRegistering] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const flashTimer = useRef<number | undefined>(undefined);

  /** 请求序号守卫（照 ReportPanel 的 loadReqId 先例）：快速连点刷新时，
   *  旧请求的响应不再写 state，防止旧数据覆盖新一轮结果 */
  const loadReqId = useRef(0);

  // ---- 草稿 ----
  const [modeDraft, setModeDraft] = useState<SyncMode>("manual");
  const [intervalDraft, setIntervalDraft] = useState(60);
  const [daysDraft, setDaysDraft] = useState(30);
  const [regForm, setRegForm] = useState<RegisterRequest>({
    server_url: "",
    master_token: "",
    device_name: "",
  });

  // ---- 弹层 ----
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);
  const [renameTarget, setRenameTarget] = useState<DeviceInfo | null>(null);
  const [mergeSource, setMergeSource] = useState<DeviceInfo | null>(null);

  /** 页头轻提示（短暂展示后自动消失，照 SyncPanel 的 syncFlash 模式） */
  function showFlash(msg: string, ms = 2500) {
    setFlash(msg);
    window.clearTimeout(flashTimer.current);
    flashTimer.current = window.setTimeout(() => setFlash(null), ms);
  }

  /** 注册后的附属数据并行拉取（pending / 连通 / 设备 / 清理状态），
   *  各自独立容错：单块失败只落对应 error，不拖垮整页；每个写入点都带
   *  请求序号检查（调用前提：已有 device_token） */
  async function fetchAttached(reqId: number) {
    pendingUploadCount()
      .then((n) => {
        if (reqId === loadReqId.current) setPending(n);
      })
      .catch(() => {
        if (reqId === loadReqId.current) setPending(0);
      });
    pingSyncServer()
      .then((r) => {
        if (reqId === loadReqId.current) setPing(r);
      })
      .catch((e) => {
        if (reqId === loadReqId.current)
          setPing({ status: "error", detail: String(e) });
      });
    try {
      const list = await listRemoteDevices();
      if (reqId !== loadReqId.current) return;
      setDevices(list);
      setDevicesError(null);
    } catch (e) {
      if (reqId !== loadReqId.current) return;
      setDevices(null);
      setDevicesError(String(e));
    }
    try {
      const st = await getCleanupStatus();
      if (reqId !== loadReqId.current) return;
      setCleanupStatus(st);
      setDaysDraft(st.auto_config.auto_days || 30);
      setCleanupError(null);
    } catch (e) {
      if (reqId !== loadReqId.current) return;
      setCleanupStatus(null);
      setCleanupError(String(e));
    }
  }

  /** 全量刷新：配置重读（不走 syncConfig.ts 缓存）→ 注册则并行拉附属数据。
   *  开头清一次操作错误（刷新即重试入口）；仅最新请求收尾 loading/refreshing */
  async function refreshAll() {
    const reqId = ++loadReqId.current;
    setRefreshing(true);
    setActionError(null);
    try {
      const cfg = await getSyncConfig();
      if (reqId !== loadReqId.current) return;
      setConfig(cfg);
      setConfigError(null);
      setModeDraft(cfg.mode);
      setIntervalDraft(cfg.interval_seconds);
      if (cfg.device_token) {
        await fetchAttached(reqId);
      } else {
        // 未注册 / 已断开：远端数据全部清空，避免残留上一连接的设备表
        setPending(0);
        setPing(null);
        setDevices(null);
        setDevicesError(null);
        setCleanupStatus(null);
        setCleanupError(null);
      }
    } catch (e) {
      if (reqId !== loadReqId.current) return;
      setConfigError(String(e));
    } finally {
      if (reqId === loadReqId.current) {
        setLoading(false);
        setRefreshing(false);
      }
    }
  }

  useEffect(() => {
    void refreshAll();
    return () => window.clearTimeout(flashTimer.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /**
   * SyncConfig 的读-改-写：先拉最新配置，只覆盖目标字段后写回。patch 支持函数
   * 形式（基于 latest 计算，翻转开关类操作不依赖页面快照）。页面内的 config
   * 是打开页面 / 上次刷新时的快照，直接用它全量写回会把后台同步线程刚推进的
   * last_sync_at 与各来源上传游标回退，造成一批记录重复上传
   * （SyncPanel.tsx 设备改名回写有同款事故注释）。
   */
  async function updateConfigFields(
    patch:
      | Partial<SyncConfig>
      | ((latest: SyncConfig) => Partial<SyncConfig>)
  ) {
    const latest = await getSyncConfig();
    const changes = typeof patch === "function" ? patch(latest) : patch;
    const next = { ...latest, ...changes };
    await setSyncConfig(next);
    setConfig(next);
    setModeDraft(next.mode);
    setIntervalDraft(next.interval_seconds);
  }

  // ---- 注册 ----
  async function handleRegister() {
    setRegistering(true);
    setActionError(null);
    try {
      await registerDevice(regForm);
      setRegForm({ server_url: "", master_token: "", device_name: "" });
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setRegistering(false);
    }
  }

  // ---- 状态卡操作 ----
  /** 模式 / 间隔即改即存（与卡③自动清理的提交模型一致）：
   *  分段点击立即提交、间隔输入 blur 提交；与当前生效值相同则不提交 */
  async function commitMode(mode: SyncMode, interval: number) {
    if (busy || syncing) return; // 防并发提交（读写-改-写串行化）
    if (
      config &&
      config.mode === mode &&
      config.interval_seconds === interval
    ) {
      return;
    }
    setBusy(true);
    setActionError(null);
    try {
      await updateConfigFields({ mode, interval_seconds: interval });
      showFlash(t("dashboard.sync.modeSaved"), 1500);
    } catch (e) {
      setActionError(String(e));
    } finally {
      setBusy(false);
    }
  }

  /** 翻转同步开关：基于落盘前的最新配置取反（不依赖页面快照——页面快照
   *  可能落后于悬浮窗面板里刚做过的开关操作） */
  async function handleToggleEnabled() {
    setBusy(true);
    setActionError(null);
    try {
      await updateConfigFields((latest) => ({ enabled: !latest.enabled }));
    } catch (e) {
      setActionError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleSyncNow() {
    setSyncing(true);
    setActionError(null);
    try {
      const outcome = await syncNow();
      showFlash(t("dashboard.sync.uploaded", { count: outcome.uploaded }));
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setSyncing(false);
    }
  }

  async function handleDisconnect() {
    setBusy(true);
    setActionError(null);
    try {
      await disconnectDevice();
      setConfirm(null);
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setBusy(false);
    }
  }

  // ---- 设备管理（master token）----
  async function handleRename(deviceId: string, newName: string) {
    if (!masterReady) {
      setActionError(t("dashboard.sync.masterNeeded"));
      return;
    }
    setBusy(true);
    setActionError(null);
    try {
      const res = await renameDevice(masterInput.trim(), deviceId, newName);
      if (res.updated === 0) {
        // 设备已被他人删除：提示并刷新，不留悬空弹层
        setActionError(t("sync.deviceMissing"));
        setRenameTarget(null);
        await refreshAll();
        return;
      }
      // 改的是本机设备名时回写 sync.json 保持一致（避免下次同步注册成新
      // 名字）；必须走读-改-写，见 updateConfigFields 的竞态说明
      if (config && config.device_id === deviceId) {
        await updateConfigFields({ device_name: newName });
      }
      showFlash(t("sync.renamed", { name: newName }));
      setRenameTarget(null);
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleMerge(sourceId: string, targetId: string) {
    if (!masterReady) {
      setActionError(t("dashboard.sync.masterNeeded"));
      return;
    }
    setBusy(true);
    setActionError(null);
    try {
      const res = await mergeDevices(masterInput.trim(), sourceId, targetId);
      const targetName =
        devices?.find((d) => d.device_id === targetId)?.device_name ??
        t("dashboard.sync.mergeTarget");
      showFlash(
        t("dashboard.sync.merged", {
          count: res.records_moved,
          name: targetName,
        })
      );
      setMergeSource(null);
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setBusy(false);
    }
  }

  /** 服务端清理统一入口（device / before / all / reset），结果文案区分
   *  是否连设备一起删（reset 返回 devices_deleted） */
  async function handleCleanup(
    action: "device" | "before" | "all" | "reset",
    deviceId?: string,
    days?: number
  ) {
    if (!masterReady) {
      setActionError(t("dashboard.sync.masterNeeded"));
      return;
    }
    setBusy(true);
    setActionError(null);
    try {
      const res: CleanupResult = await cleanupServer(
        masterInput.trim(),
        action,
        deviceId,
        days
      );
      const msg =
        res.devices_deleted != null
          ? t("dashboard.sync.deletedBoth", {
              records: res.records_deleted,
              devices: res.devices_deleted,
            })
          : t("dashboard.sync.deleted", { count: res.records_deleted });
      showFlash(msg);
      setConfirm(null);
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleAutoCleanup(enabled: boolean, days: number) {
    if (!masterReady) {
      setActionError(t("dashboard.sync.masterNeeded"));
      return;
    }
    setBusy(true);
    setActionError(null);
    try {
      await setAutoCleanup(masterInput.trim(), enabled, days);
      setCleanupStatus(await getCleanupStatus());
      showFlash(t("dashboard.sync.autoCleanupSaved"), 1500);
    } catch (e) {
      setActionError(String(e));
    } finally {
      setBusy(false);
    }
  }

  // ---- 派生状态 ----
  const registered = config != null && config.device_token.length > 0;
  const paused = config != null && !config.enabled;
  /** 本机置顶，其余按注册时间升序（与服务端返回顺序一致） */
  const sortedDevices =
    devices == null
      ? null
      : [
          ...devices.filter((d) => d.device_id === config?.device_id),
          ...devices.filter((d) => d.device_id !== config?.device_id),
        ];
  const totalRecords = cleanupStatus?.total_records ?? 0;
  const devicesSub =
    sortedDevices != null
      ? t("dashboard.sync.devicesSub", {
          count: sortedDevices.length,
          records: totalRecords,
        })
      : undefined;

  // ---- 连通状态格的展示值 ----
  const pingView = (() => {
    if (ping == null) return { text: "…", cls: "is-unknown", title: "" };
    switch (ping.status) {
      case "ok":
        return { text: t("dashboard.sync.pingOk"), cls: "is-ok", title: "" };
      case "disabled":
        return {
          text: t("dashboard.sync.pingDisabled"),
          cls: "is-unknown",
          title: t("dashboard.sync.pingDisabledHint"),
        };
      default:
        // unreachable（网络失败，detail 带原因）/ error（前端 catch 合成）
        return {
          text: t("dashboard.sync.pingUnreachable"),
          cls: "is-bad",
          title: ping.detail ?? "",
        };
    }
  })();

  return (
    <div className="db-page db-sync-page">
      <header className="db-page__header db-page__header--sticky">
        <div>
          <h1 className="db-page__title">{t("dashboard.sync")}</h1>
          <p className="db-page__desc">{t("dashboard.sync.desc")}</p>
        </div>
        <div className="db-page__actions">
          {/* 操作反馈锚点（吸顶，弹层关闭后仍可见）：错误优先于成功提示；
              长错误文案截断、title 悬停看全文 */}
          {actionError ? (
            <span className="db-sync__error" title={actionError}>
              {actionError}
            </span>
          ) : flash ? (
            <span className="db-sync__flash">{flash}</span>
          ) : null}
          <button
            type="button"
            className="db-btn"
            onClick={() => void refreshAll()}
            disabled={refreshing}
          >
            <RefreshIcon
              className={`h-3.5 w-3.5 ${refreshing ? "db-spin" : ""}`}
            />
            {t("common.refresh")}
          </button>
        </div>
      </header>

      {/* 配置刷新失败但已有旧数据：弱提示条保留（不清空已渲染内容） */}
      {configError && config != null && (
        <AlertBanner type="error">{configError}</AlertBanner>
      )}

      {config == null && configError != null ? (
        /* 配置读失败且无任何数据：专用错误重试态（照总览页 hardError 模式），
           绝不落注册表单——已注册用户误重注册会在服务端产生重复设备 */
        <div className="db-errorstate">
          <div className="db-errorstate__title">
            {t("dashboard.sync.errorTitle")}
          </div>
          <div className="db-errorstate__reason">
            {t("dashboard.sync.errorReason", { error: configError })}
          </div>
          <button
            type="button"
            className="db-btn"
            onClick={() => void refreshAll()}
          >
            <RefreshIcon className="h-3.5 w-3.5" />
            {t("dashboard.sync.retry")}
          </button>
        </div>
      ) : loading && config == null ? (
        /* 首屏骨架：状态卡 + 设备卡的形状占位 */
        <>
          <div className="db-card">
            <div className="db-card__body">
              <Skeleton className="h-24 w-full" />
            </div>
          </div>
          <div className="db-card">
            <div className="db-card__body">
              <Skeleton className="h-40 w-full" />
            </div>
          </div>
        </>
      ) : !registered ? (
        /* ---- 区块①（未注册）：注册引导表单 ---- */
        <Card
          title={t("dashboard.sync.registerTitle")}
          sub={t("dashboard.sync.registerDesc")}
        >
          <div className="db-sync-form">
            <label className="db-sync-field">
              <span>{t("dashboard.sync.serverUrl")}</span>
              <input
                type="text"
                value={regForm.server_url}
                placeholder="http://192.168.1.100:3838"
                onChange={(e) =>
                  setRegForm((f) => ({ ...f, server_url: e.target.value }))
                }
              />
            </label>
            <label className="db-sync-field">
              <span>{t("dashboard.sync.masterToken")}</span>
              <span className="db-sync-field__box">
                <input
                  type={showMaster ? "text" : "password"}
                  value={regForm.master_token}
                  onChange={(e) =>
                    setRegForm((f) => ({ ...f, master_token: e.target.value }))
                  }
                />
                <button
                  type="button"
                  onClick={() => setShowMaster((v) => !v)}
                >
                  {showMaster ? t("common.hide") : t("common.show")}
                </button>
              </span>
            </label>
            <label className="db-sync-field">
              <span>{t("dashboard.sync.deviceName")}</span>
              <input
                type="text"
                value={regForm.device_name}
                placeholder={t("sync.namePh")}
                onChange={(e) =>
                  setRegForm((f) => ({ ...f, device_name: e.target.value }))
                }
              />
            </label>
            {/* 非 https:// 一律警示（裸地址会被后端默认补 http://，
                同样是明文） */}
            {regForm.server_url.trim() !== "" &&
              !/^https:\/\//i.test(regForm.server_url.trim()) && (
                <p className="db-sync-warn">
                  {t("dashboard.sync.httpWarn")}
                </p>
              )}
            <button
              type="button"
              className="db-btn db-btn--primary db-sync-form__submit"
              onClick={() => void handleRegister()}
              disabled={
                registering ||
                !regForm.server_url.trim() ||
                !regForm.master_token.trim() ||
                !regForm.device_name.trim()
              }
            >
              {registering
                ? t("dashboard.sync.registering")
                : t("dashboard.sync.register")}
            </button>
          </div>
        </Card>
      ) : (
        <>
          {/* ---- 区块①（已注册）：本机同步状态卡 ---- */}
          <Card
            title={t("dashboard.sync.statusTitle")}
            sub={`${config!.device_name} · ${config!.device_id.slice(0, 6)}`}
            action={
              <span
                className={`db-sync-state ${
                  paused ? "db-sync-state--paused" : "db-sync-state--on"
                }`}
              >
                {paused
                  ? t("dashboard.sync.statePaused")
                  : t("dashboard.sync.stateEnabled")}
              </span>
            }
          >
            <div className="db-sync-grid">
              <div className="db-sync-grid__item">
                <span className="db-sync-grid__label">
                  {t("dashboard.sync.fieldServer")}
                </span>
                <span
                  className="db-sync-grid__value num"
                  title={config!.server_url}
                >
                  {config!.server_url}
                </span>
              </div>
              <div className="db-sync-grid__item">
                <span className="db-sync-grid__label">
                  {t("dashboard.sync.fieldPing")}
                </span>
                <span
                  className={`db-sync-grid__value db-sync-ping ${pingView.cls}`}
                  title={pingView.title}
                >
                  {pingView.text}
                </span>
              </div>
              <div className="db-sync-grid__item">
                <span className="db-sync-grid__label">
                  {t("dashboard.sync.fieldLastSync")}
                </span>
                <span
                  className="db-sync-grid__value"
                  title={
                    config!.last_sync_at
                      ? formatDateTime(config!.last_sync_at)
                      : t("dashboard.sync.never")
                  }
                >
                  {timeAgo(t, config!.last_sync_at)}
                </span>
              </div>
              <div className="db-sync-grid__item">
                <span className="db-sync-grid__label">
                  {t("dashboard.sync.fieldPending")}
                </span>
                <span className="db-sync-grid__value num">
                  {t("dashboard.sync.pendingCount", { count: pending })}
                </span>
              </div>
            </div>

            {/* 同步模式：即改即存（分段点击提交 / auto 间隔 blur 提交），
                无独立保存按钮（与卡③自动清理的提交模型一致） */}
            <div className="db-sync-mode">
              <span className="db-sync-mode__label">
                {t("dashboard.sync.mode")}
              </span>
              <div className="db-seg db-seg--sm" role="group">
                {(["manual", "auto"] as SyncMode[]).map((m) => (
                  <button
                    key={m}
                    type="button"
                    className={`db-seg__btn ${
                      modeDraft === m ? "db-seg__btn--active" : ""
                    }`}
                    aria-pressed={modeDraft === m}
                    onClick={() => {
                      setModeDraft(m);
                      void commitMode(m, intervalDraft);
                    }}
                  >
                    {m === "manual"
                      ? t("dashboard.sync.manual")
                      : t("dashboard.sync.auto")}
                  </button>
                ))}
              </div>
              {modeDraft === "auto" && (
                <label className="db-sync-mode__interval">
                  <span>{t("dashboard.sync.interval")}</span>
                  <input
                    type="number"
                    min={10}
                    value={intervalDraft}
                    onChange={(e) =>
                      setIntervalDraft(
                        Math.max(10, parseInt(e.target.value) || 60)
                      )
                    }
                    onBlur={() => {
                      const v = Math.max(10, intervalDraft);
                      setIntervalDraft(v);
                      if (v !== config?.interval_seconds) {
                        void commitMode("auto", v);
                      }
                    }}
                    className="num"
                  />
                </label>
              )}
            </div>

            {/* 操作行 */}
            <div className="db-sync-ops">
              <button
                type="button"
                className="db-btn db-btn--primary"
                onClick={() => void handleSyncNow()}
                disabled={syncing || busy || paused}
                title={paused ? t("dashboard.sync.syncPausedHint") : undefined}
              >
                {syncing
                  ? t("dashboard.sync.syncing")
                  : t("dashboard.sync.syncNow")}
              </button>
              <button
                type="button"
                className="db-btn"
                onClick={() => void handleToggleEnabled()}
                disabled={busy || syncing}
              >
                {paused
                  ? t("dashboard.sync.resume")
                  : t("dashboard.sync.pause")}
              </button>
              <button
                type="button"
                className="db-btn db-sync-ops__danger"
                onClick={() => setConfirm({ kind: "disconnect" })}
                disabled={busy || syncing}
              >
                {t("dashboard.sync.disconnect")}
              </button>
            </div>
          </Card>

          {/* ---- 区块②：服务端设备表 ---- */}
          <Card title={t("dashboard.sync.devicesTitle")} sub={devicesSub}>
            {/* master token 解锁管理操作：仅本页内存，离开本页即失效 */}
            <div className="db-sync-master">
              <label className="db-sync-field">
                <span>{t("dashboard.sync.masterLabel")}</span>
                <span className="db-sync-field__box">
                  <input
                    type={showMaster ? "text" : "password"}
                    value={masterInput}
                    placeholder={t("dashboard.sync.masterPlaceholder")}
                    onChange={(e) => setMasterInput(e.target.value)}
                  />
                  <button
                    type="button"
                    onClick={() => setShowMaster((v) => !v)}
                  >
                    {showMaster ? t("common.hide") : t("common.show")}
                  </button>
                </span>
              </label>
              <p className="db-sync-master__hint">
                {t("dashboard.sync.masterHint")}
              </p>
            </div>

            {devicesError ? (
              <AlertBanner type="error">
                {t("dashboard.sync.devicesError", { error: devicesError })}
              </AlertBanner>
            ) : sortedDevices == null ? (
              <Skeleton className="h-24 w-full" />
            ) : sortedDevices.length === 0 ? (
              <p className="db-sync-empty">
                {t("dashboard.sync.devicesEmpty")}
              </p>
            ) : (
              <>
                {/* 操作说明（信息性弱提示，照卡③ dataHint 先例）：
                    操作列三个按钮裸文案语义不明，合并尤其易误解为「合并视图」 */}
                <p className="db-sync-scopehint">
                  {t("dashboard.sync.devicesOpsHint")}
                </p>
                <div className="db-table-wrap">
                <table className="db-table db-sync-devices">
                  <thead>
                    <tr>
                      <th>{t("dashboard.sync.colDevice")}</th>
                      <th>{t("dashboard.sync.colDeviceId")}</th>
                      <th>{t("dashboard.sync.colRegistered")}</th>
                      <th>
                        <span title={t("dashboard.sync.lastSeenHint")}>
                          {t("dashboard.sync.colLastSeen")}
                        </span>
                      </th>
                      <th>
                        <span title={t("dashboard.sync.recordsHint")}>
                          {t("dashboard.sync.colRecords")}
                        </span>
                      </th>
                      <th className="db-sync-devices__ops">
                        {t("dashboard.sync.colOps")}
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {sortedDevices.map((d) => {
                      const isLocal = d.device_id === config?.device_id;
                      const opsTitle = masterReady
                        ? undefined
                        : t("dashboard.sync.masterNeeded");
                      return (
                        <tr key={d.device_id}>
                          <td>
                            <span className="db-sync-devname">
                              {d.device_name}
                              {isLocal && (
                                <span className="db-sync-localbadge">
                                  {t("dashboard.sync.localBadge")}
                                </span>
                              )}
                              <button
                                type="button"
                                className="db-linkbtn"
                                onClick={() => setRenameTarget(d)}
                                disabled={!masterReady || busy}
                                title={opsTitle ?? t("dashboard.sync.rename")}
                              >
                                {t("dashboard.sync.rename")}
                              </button>
                            </span>
                          </td>
                          <td className="num" title={d.device_id}>
                            {d.device_id.slice(0, 6)}
                          </td>
                          <td className="num" title={formatDateTime(d.created_at)}>
                            {formatDateTime(d.created_at)}
                          </td>
                          <td>
                            {d.last_seen_at ? (
                              <span
                                title={`${formatDateTime(
                                  d.last_seen_at
                                )} · ${t("dashboard.sync.lastSeenHint")}`}
                              >
                                {timeAgo(t, d.last_seen_at)}
                              </span>
                            ) : (
                              <span
                                className="db-sync-nodata"
                                title={t("dashboard.sync.lastSeenHint")}
                              >
                                {t("dashboard.sync.noData")}
                              </span>
                            )}
                          </td>
                          <td
                            className="num"
                            title={t("dashboard.sync.recordsHint")}
                          >
                            {(d.record_count ?? 0).toLocaleString()}
                          </td>
                          <td className="db-sync-devices__ops">
                            {/* 本机行只留改名（照 SyncPanel 先例：本机数据的
                                删除 / 合并走断开重连或在其他设备上操作，
                                防误操作当前正在同步的注册） */}
                            {!isLocal && (
                              <>
                                <button
                                  type="button"
                                  className="db-linkbtn"
                                  onClick={() => setMergeSource(d)}
                                  disabled={!masterReady || busy}
                                  title={
                                    opsTitle ?? t("dashboard.sync.mergeHint")
                                  }
                                >
                                  {t("dashboard.sync.merge")}
                                </button>
                                <button
                                  type="button"
                                  className="db-linkbtn db-sync-del"
                                  onClick={() =>
                                    setConfirm({ kind: "device", device: d })
                                  }
                                  disabled={!masterReady || busy}
                                  title={
                                    opsTitle ??
                                    t("dashboard.sync.deleteDataHint")
                                  }
                                >
                                  {t("dashboard.sync.deleteData")}
                                </button>
                              </>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
                </div>
              </>
            )}
          </Card>

          {/* ---- 区块③：服务端数据管理 ---- */}
          <Card
            title={t("dashboard.sync.dataTitle")}
            action={
              cleanupStatus ? (
                <span className="num db-sync-total">
                  {t("dashboard.sync.totalRecords", {
                    count: cleanupStatus.total_records,
                  })}
                </span>
              ) : undefined
            }
          >
            {cleanupError ? (
              <AlertBanner type="error">
                {t("dashboard.sync.cleanupError", { error: cleanupError })}
              </AlertBanner>
            ) : cleanupStatus == null ? (
              <Skeleton className="h-16 w-full" />
            ) : (
              <>
                {/* 范围说明（信息性弱提示，区别于未解锁的 amber 提示）：
                    这些操作清理的是服务端汇总数据，本机数据不受影响 */}
                <p className="db-sync-scopehint">
                  {t("dashboard.sync.dataHint")}
                </p>
                {/* 未解锁就地弱提示（不只靠禁用按钮的 title） */}
                {!masterReady && (
                  <p className="db-sync-lockedhint">
                    {t("dashboard.sync.masterLockedHint")}
                  </p>
                )}
                {/* 自动清理：开关 + 保留天数（onBlur 提交，避免每键发请求）。
                    天数框常显（不随开关隐藏）：关闭态也可见可改，开关翻转与
                    天数提交均用当前草稿值 daysDraft，先改天数再开开关一次生效；
                    天数 onBlur 只更新天数，传当前实际开关状态（关闭态改天数
                    不会意外把开关打开） */}
                <div className="db-sync-autoclean">
                  <span>{t("dashboard.sync.autoCleanup")}</span>
                  <button
                    type="button"
                    className={`db-btn db-btn--sm ${
                      cleanupStatus.auto_config.auto_enabled
                        ? "db-sync-autoclean__on"
                        : ""
                    }`}
                    onClick={() =>
                      void handleAutoCleanup(
                        !cleanupStatus.auto_config.auto_enabled,
                        Math.max(1, daysDraft)
                      )
                    }
                    disabled={!masterReady || busy}
                    title={
                      masterReady
                        ? undefined
                        : t("dashboard.sync.masterNeeded")
                    }
                  >
                    {cleanupStatus.auto_config.auto_enabled
                      ? t("dashboard.sync.on")
                      : t("dashboard.sync.off")}
                  </button>
                  <label className="db-sync-mode__interval">
                    <span>{t("dashboard.sync.keepDays")}</span>
                    <input
                      type="number"
                      min={1}
                      value={daysDraft}
                      onChange={(e) =>
                        setDaysDraft(parseInt(e.target.value) || 30)
                      }
                      onBlur={() => {
                        const days = Math.max(1, daysDraft);
                        if (days !== cleanupStatus.auto_config.auto_days) {
                          void handleAutoCleanup(
                            cleanupStatus.auto_config.auto_enabled,
                            days
                          );
                        }
                      }}
                      disabled={!masterReady || busy}
                      className="num"
                    />
                  </label>
                  {/* 副说明独占一行（flex-basis:100%，见 .db-sync-opshint） */}
                  <p className="db-sync-opshint">
                    {t("dashboard.sync.autoCleanupHint")}
                  </p>
                </div>

                {/* 危险操作：三者全部走确认弹层。每操作一行（按钮 + 副说明），
                    按钮的回调 / 禁用条件与之前完全一致 */}
                <div className="db-sync-danger">
                  <div className="db-sync-danger__row">
                    <button
                      type="button"
                      className="db-btn"
                      onClick={() => setConfirm({ kind: "before" })}
                      disabled={!masterReady || busy}
                      title={
                        masterReady
                          ? undefined
                          : t("dashboard.sync.masterNeeded")
                      }
                    >
                      {t("dashboard.sync.cleanByTime")}
                    </button>
                    <p className="db-sync-opshint">
                      {t("dashboard.sync.cleanByTimeHint")}
                    </p>
                  </div>
                  <div className="db-sync-danger__row">
                    <button
                      type="button"
                      className="db-btn"
                      onClick={() => setConfirm({ kind: "all" })}
                      disabled={!masterReady || busy}
                      title={
                        masterReady
                          ? undefined
                          : t("dashboard.sync.masterNeeded")
                      }
                    >
                      {t("dashboard.sync.clearAll")}
                    </button>
                    <p className="db-sync-opshint">
                      {t("dashboard.sync.clearAllHint")}
                    </p>
                  </div>
                  <div className="db-sync-danger__row">
                    <button
                      type="button"
                      className="db-btn"
                      onClick={() => setConfirm({ kind: "reset" })}
                      disabled={!masterReady || busy}
                      title={
                        masterReady
                          ? undefined
                          : t("dashboard.sync.masterNeeded")
                      }
                    >
                      {t("dashboard.sync.reset")}
                    </button>
                    <p className="db-sync-opshint">
                      {t("dashboard.sync.resetHint")}
                    </p>
                  </div>
                </div>
              </>
            )}
          </Card>
        </>
      )}

      {/* ---- 确认弹层（断开 / 删设备数据 / 按时间清理 / 全部清空 / 重置）：
          大窗同构形态（照 PricingPage 删除确认先例）。断开不删数据用普通
          主色键，其余破坏性操作用红键 ---- */}
      {confirm && (
        <ConfirmDialog
          state={confirm}
          busy={busy}
          defaultDays={cleanupStatus?.auto_config.auto_days || 30}
          onCancel={() => setConfirm(null)}
          onConfirm={(days) => {
            switch (confirm.kind) {
              case "disconnect":
                void handleDisconnect();
                break;
              case "device":
                void handleCleanup("device", confirm.device.device_id);
                break;
              case "before":
                void handleCleanup("before", undefined, days);
                break;
              case "all":
                void handleCleanup("all");
                break;
              case "reset":
                void handleCleanup("reset");
                break;
            }
          }}
        />
      )}

      {/* ---- 设备改名弹层 ---- */}
      {renameTarget && (
        <RenameDialog
          device={renameTarget}
          busy={busy}
          onCancel={() => setRenameTarget(null)}
          onConfirm={(name) => void handleRename(renameTarget.device_id, name)}
        />
      )}

      {/* ---- 设备合并弹层 ---- */}
      {mergeSource && sortedDevices != null && sortedDevices.length > 1 && (
        <MergeDialog
          source={mergeSource}
          devices={sortedDevices}
          localDeviceId={config?.device_id ?? ""}
          busy={busy}
          onCancel={() => setMergeSource(null)}
          onConfirm={(targetId) =>
            void handleMerge(mergeSource.device_id, targetId)
          }
        />
      )}
    </div>
  );
}

/** 确认弹层：按 kind 切换标题 / 描述 / 确认键文案；before 带天数输入（默认
 *  自动清理的保留天数）。断开用主色键（不删数据），其余红键。文件级组件：
 *  弹层打开期间父级重渲染不重挂载，天数输入不丢。Escape 关闭、确认键在
 *  busy 期间禁用（防双击重复请求）。 */
function ConfirmDialog({
  state,
  busy,
  defaultDays,
  onCancel,
  onConfirm,
}: {
  state: ConfirmState;
  busy: boolean;
  defaultDays: number;
  onCancel: () => void;
  onConfirm: (days: number) => void;
}) {
  const { t } = useI18n();
  const [days, setDays] = useState(defaultDays);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const title =
    state.kind === "disconnect"
      ? t("dashboard.sync.confirmDisconnectTitle")
      : state.kind === "device"
        ? t("dashboard.sync.confirmDeviceTitle")
        : state.kind === "before"
          ? t("dashboard.sync.confirmBeforeTitle")
          : state.kind === "all"
            ? t("dashboard.sync.confirmAllTitle")
            : t("dashboard.sync.confirmResetTitle");
  const desc =
    state.kind === "disconnect"
      ? t("dashboard.sync.confirmDisconnectDesc")
      : state.kind === "device"
        ? t("dashboard.sync.confirmDeviceDesc")
        : state.kind === "before"
          ? t("dashboard.sync.confirmBeforeDesc", { days })
          : state.kind === "all"
            ? t("dashboard.sync.confirmAllDesc")
            : t("dashboard.sync.confirmResetDesc");
  const confirmLabel =
    state.kind === "disconnect"
      ? t("dashboard.sync.confirmDisconnect")
      : state.kind === "device"
        ? t("dashboard.sync.confirmDelete")
        : state.kind === "before"
          ? t("dashboard.sync.confirmClean")
          : state.kind === "all"
            ? t("dashboard.sync.confirmClear")
            : t("dashboard.sync.confirmReset");

  return (
    <div className="db-confirm-mask">
      <div className="db-confirm" role="alertdialog" aria-modal="true">
        <div className="db-confirm__title">{title}</div>
        <p className="db-confirm__desc">{desc}</p>
        {state.kind === "before" && (
          <label className="db-sync-confirm-days">
            <span>{t("dashboard.sync.keepDays")}</span>
            <input
              type="number"
              min={1}
              value={days}
              onChange={(e) =>
                setDays(Math.max(1, parseInt(e.target.value) || 30))
              }
              className="num"
            />
          </label>
        )}
        <div className="db-confirm__actions">
          <button
            type="button"
            className="db-btn"
            onClick={onCancel}
            disabled={busy}
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className={`db-btn ${
              state.kind === "disconnect"
                ? "db-btn--primary"
                : "db-btn--danger"
            }`}
            onClick={() => onConfirm(days)}
            disabled={busy}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 设备改名弹层（1-32 字，Enter 提交，照 SyncPanel 的 RenameDialog 交互）。
 *  Escape 关闭、确认键 busy 禁用。 */
function RenameDialog({
  device,
  busy,
  onCancel,
  onConfirm,
}: {
  device: DeviceInfo;
  busy: boolean;
  onCancel: () => void;
  onConfirm: (newName: string) => void;
}) {
  const { t } = useI18n();
  const [name, setName] = useState(device.device_name);
  const trimmed = name.trim();
  const valid = trimmed.length > 0 && trimmed.length <= 32;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className="db-confirm-mask">
      <div className="db-confirm" role="alertdialog" aria-modal="true">
        <div className="db-confirm__title">
          {t("dashboard.sync.renameTitle")}
        </div>
        <p className="db-confirm__desc">
          {t("dashboard.sync.renameDesc", {
            name: device.device_name,
            id: device.device_id.slice(0, 6),
          })}
        </p>
        <input
          type="text"
          className="db-sync-rename-input"
          value={name}
          maxLength={32}
          autoFocus
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && valid && !busy) onConfirm(trimmed);
            if (e.key === "Escape") onCancel();
          }}
        />
        <div className="db-sync-rename-count num">{trimmed.length}/32</div>
        <div className="db-confirm__actions">
          <button
            type="button"
            className="db-btn"
            onClick={onCancel}
            disabled={busy}
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="db-btn db-btn--primary"
            disabled={!valid || busy}
            onClick={() => onConfirm(trimmed)}
          >
            {t("common.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 设备合并弹层：来源信息 + 目标设备下拉 + 破坏性警示（红键确认）。
 *  默认目标 = 除来源外最新注册的设备（照 SyncPanel 的 MergeDialog 先例：
 *  不默认本机，避免历史数据在本机"全部汇总"视图中不可见）。Escape 关闭、
 *  确认键 busy 禁用（合并重入会报「设备不存在」，必须防双击）。 */
function MergeDialog({
  source,
  devices,
  localDeviceId,
  busy,
  onCancel,
  onConfirm,
}: {
  source: DeviceInfo;
  devices: DeviceInfo[];
  localDeviceId: string;
  busy: boolean;
  onCancel: () => void;
  onConfirm: (targetId: string) => void;
}) {
  const { t } = useI18n();
  const candidates = devices.filter((d) => d.device_id !== source.device_id);
  const defaultTarget =
    [...candidates].sort((a, b) => b.created_at - a.created_at)[0]
      ?.device_id ??
    candidates[0]?.device_id ??
    "";
  const [target, setTarget] = useState(defaultTarget);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className="db-confirm-mask">
      <div className="db-confirm" role="alertdialog" aria-modal="true">
        <div className="db-confirm__title">
          {t("dashboard.sync.mergeTitle")}
        </div>
        <p className="db-confirm__desc">
          {t("dashboard.sync.mergeDesc", {
            name: source.device_name,
            id: source.device_id.slice(0, 6),
            count: source.record_count ?? 0,
          })}
        </p>
        <DropdownMenu
          className="db-sync-merge-select"
          ariaLabel={t("dashboard.sync.mergeTarget")}
          options={candidates.map((d) => ({
            value: d.device_id,
            label:
              t("common.deviceOption", {
                name: d.device_name,
                id: d.device_id.slice(0, 6),
              }) +
              (d.device_id === localDeviceId
                ? ` · ${t("dashboard.sync.localBadge")}`
                : ""),
          }))}
          value={target}
          onChange={setTarget}
        />
        <p className="db-sync-warn">{t("dashboard.sync.mergeWarn")}</p>
        <div className="db-confirm__actions">
          <button
            type="button"
            className="db-btn"
            onClick={onCancel}
            disabled={busy}
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="db-btn db-btn--danger"
            disabled={!target || busy}
            onClick={() => onConfirm(target)}
          >
            {t("dashboard.sync.mergeConfirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
