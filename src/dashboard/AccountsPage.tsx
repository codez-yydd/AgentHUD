/**
 * 凭证与账号管理页（主窗口第七页）：各 Agent 源账号凭证的统一查看与管理。
 *
 * 两个区块：
 * ① ZCode 账号——复用 accounts.rs 的快照体系（与设置页 AccountsCard 同一批
 *    invoke 命令：list/capture/switch/rename/remove，不重复造后端）。展示
 *    快照表（账号名 / 登录 Provider / 捕获更新时间）+ 当前登录标记
 *    （is_current 由后端按实时指纹回填）；操作含手动捕获、行内重命名、
 *    切换登录与删除快照——后两者是危险操作，全部走确认弹层（db-confirm，
 *    切换后果文案与设置页 AccountsCard 逐字一致：退出并重启 ZCode / 自动
 *    备份 / 失败回滚；删除只清本应用快照）。切换 / 删除期间按钮 busy 防重。
 * ② 其他源凭证一览——新命令 fetch_credential_overview 聚合各源 CLI 落在
 *    本机的登录/凭证文件静态信息（源 / 账号标识 / 凭证类型 / 文件路径 /
 *    修改时间 / 是否存在），纯只读：后端保证不返回任何凭证明文，本页也不
 *    做任何主动联网校验；单源失败只标红该源，不阻塞其他源。
 *
 * 数据获取与 SyncPage 同款纪律：本页自管数据 + refreshAll 带请求序号守卫
 * （loadReqId，旧响应不覆盖新请求），不自动轮询（后端 60s 自动捕获有事件
 * 推送，见下方监听）。两个数据源各自独立容错：账号列表 / 凭证概览一个失败
 * 不拖垮另一个，失败且无数据的那块在卡内走 AlertBanner + 重试；两者同时
 * 失败且无任何数据才落整页 db-errorstate 错误重试态。
 */
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useI18n, type TFn } from "../i18n";
import { AlertBanner } from "../layout";
import { Card, Skeleton } from "./primitives";
import { RefreshIcon } from "./icons";
import type {
  AccountMeta,
  AccountsState,
  CredentialSourceOverview,
} from "../types";
import {
  captureAccount,
  fetchCredentialOverview,
  listAccounts,
  removeAccount,
  renameAccount,
  switchAccount,
} from "../api";

/** 后端「ZCode 换账号自动捕获」事件（accounts.rs 的 spawn_auto_capture 每
 * 60s 轮询，捕获到新登录时广播）：打开着的本页立即刷新账号列表，与设置页
 * AccountsCard 同款监听（payload 不用，只当刷新信号）。 */
const ACCOUNTS_AUTO_CAPTURED_EVENT = "zbar://accounts-auto-captured";

/** 源 id → 品牌名（品牌名不翻译，沿 en.ts 头注释的口径；顺序即展示顺序，
 *  与后端 credential_overview 的返回顺序一致；kimi = Kimi Code CLI，与
 *  agentVisibility.ts SOURCE_META 等全站品牌元数据同字面） */
const SOURCE_NAMES: Record<string, string> = {
  codex: "Codex",
  claude: "Claude",
  gemini: "Gemini",
  grok: "Grok",
  kimi: "Kimi Code",
  opencode: "OpenCode",
  minimax: "MiniMax",
  dsh: "DSH",
};

/** 相对当前时间的友好描述（词条复用 dashboard.sync.* 的时间词组，两侧
 * 语义通用，不为本页重复造词） */
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

/** 本地时区绝对时间（YYYY-MM-DD HH:mm），用于时间列 title 悬停 */
function formatDateTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}`;
}

/** 确认弹层状态（切换登录 / 删除快照两种危险操作共用一个弹层形态） */
type ConfirmState =
  | { kind: "switch"; account: AccountMeta }
  | { kind: "delete"; account: AccountMeta };

/** 凭证类型 kind → 词条键（oauth / apiKey / unknown 之外的值按 unknown 兜底） */
function kindLabelKey(kind: string) {
  if (kind === "oauth") return "dashboard.accounts.kindOauth" as const;
  if (kind === "apiKey") return "dashboard.accounts.kindApiKey" as const;
  return "dashboard.accounts.kindUnknown" as const;
}

export function AccountsPage() {
  const { t } = useI18n();

  // ---- 数据（两块各自独立容错）----
  const [accounts, setAccounts] = useState<AccountsState | null>(null);
  const [accountsError, setAccountsError] = useState<string | null>(null);
  const [overview, setOverview] = useState<CredentialSourceOverview[] | null>(
    null
  );
  const [overviewError, setOverviewError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);

  // ---- 操作状态 ----
  const [capturing, setCapturing] = useState(false);
  const [switchingId, setSwitchingId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [renamingBusy, setRenamingBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const flashTimer = useRef<number | undefined>(undefined);

  /** 请求序号守卫（照 SyncPage / ReportPanel 的 loadReqId 先例）：快速连点
   *  刷新时旧请求的响应不再写 state，防止旧数据覆盖新一轮结果 */
  const loadReqId = useRef(0);

  // ---- 行内重命名（编辑中的快照 id + 草稿；Enter 提交 / Esc 取消）----
  const [renameId, setRenameId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");

  // ---- 弹层 ----
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);

  /** 任一写操作进行中：禁用全部行内操作与捕获键（防并发写快照目录） */
  const busy =
    capturing || switchingId !== null || deletingId !== null || renamingBusy;

  /** 页头轻提示（短暂展示后自动消失，照 SyncPage 的 flash 模式） */
  function showFlash(msg: string, ms = 2500) {
    setFlash(msg);
    window.clearTimeout(flashTimer.current);
    flashTimer.current = window.setTimeout(() => setFlash(null), ms);
  }

  /** 全量刷新：账号列表与凭证概览并行拉取、各自独立容错；仅最新请求收尾
   *  loading / refreshing（开头清一次操作错误，刷新即重试入口） */
  async function refreshAll() {
    const reqId = ++loadReqId.current;
    setRefreshing(true);
    setActionError(null);
    const acc = listAccounts()
      .then((s) => {
        if (reqId !== loadReqId.current) return;
        setAccounts(s);
        setAccountsError(null);
      })
      .catch((e) => {
        if (reqId !== loadReqId.current) return;
        setAccounts(null);
        setAccountsError(String(e));
      });
    const ov = fetchCredentialOverview()
      .then((o) => {
        if (reqId !== loadReqId.current) return;
        setOverview(o);
        setOverviewError(null);
      })
      .catch((e) => {
        if (reqId !== loadReqId.current) return;
        setOverview(null);
        setOverviewError(String(e));
      });
    await Promise.all([acc, ov]);
    if (reqId === loadReqId.current) {
      setLoading(false);
      setRefreshing(false);
    }
  }

  useEffect(() => {
    void refreshAll();
    return () => window.clearTimeout(flashTimer.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 后台自动捕获到新登录时刷新（与 AccountsCard 同款监听；失败静默）
  useEffect(() => {
    let dispose: (() => void) | undefined;
    let cancelled = false;
    listen(ACCOUNTS_AUTO_CAPTURED_EVENT, () => {
      void refreshAll();
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => {
        /* 静默：保持手动捕获路径 */
      });
    return () => {
      cancelled = true;
      dispose?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ---- 区块①操作（复用 AccountsCard 的命令链路）----
  useEffect(() => {
    const subscription = listen("zbar://account-changed", () => void refreshAll());
    return () => { void subscription.then((unlisten) => unlisten()); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 手动捕获当前登录（同账号重复捕获为更新；成功后 flash 区分新建/更新） */
  async function handleCapture() {
    setCapturing(true);
    setActionError(null);
    try {
      const out = await captureAccount();
      showFlash(
        out.updated_existing
          ? t("dashboard.accounts.capturedUpdate")
          : t("dashboard.accounts.capturedNew")
      );
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setCapturing(false);
    }
  }

  /** 切换登录（确认弹层后执行；成功 flash 在重启失败时追加手动打开提示，
   *  文案与 AccountsCard 一致） */
  async function handleSwitch(account: AccountMeta) {
    setSwitchingId(account.id);
    setActionError(null);
    try {
      const out = await switchAccount(account.id);
      showFlash(
        out.zcode_relaunched
          ? t("dashboard.accounts.switched", { name: out.switched_to })
          : t("dashboard.accounts.switched", { name: out.switched_to }) +
              t("dashboard.accounts.relaunchFail")
      );
      setConfirm(null);
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setSwitchingId(null);
    }
  }

  /** 删除快照（确认弹层后执行；只删本应用快照文件，不影响 ZCode 登录） */
  async function handleDelete(account: AccountMeta) {
    setDeletingId(account.id);
    setActionError(null);
    try {
      await removeAccount(account.id);
      showFlash(t("dashboard.accounts.deleted", { name: account.display_name }));
      setConfirm(null);
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setDeletingId(null);
    }
  }

  /** 行内重命名提交（32 字上限由后端再截断兜底；name_locked 置位后重捕获
   *  不再覆盖手动命名——语义见 accounts.rs 的 rename_account） */
  async function handleRenameCommit(id: string, name: string) {
    setRenamingBusy(true);
    setActionError(null);
    try {
      await renameAccount(id, name);
      setRenameId(null);
      showFlash(t("dashboard.accounts.renamed", { name }));
      await refreshAll();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setRenamingBusy(false);
    }
  }

  // ---- 派生状态 ----
  const current = accounts?.current ?? null;
  /** 当前登录匹配到的快照名（未捕获过该账号时 null → 显示未识别） */
  const currentName =
    accounts?.accounts.find((a) => a.id === current?.matched_snapshot_id)
      ?.display_name ?? null;
  /** 区块②副标题：检测到凭证的源数 / 总源数 */
  const foundCount =
    overview?.filter((o) => o.exists && o.error == null).length ?? 0;
  const overviewSub =
    overview != null
      ? t("dashboard.accounts.overviewSub", {
          count: foundCount,
          total: overview.length,
        })
      : undefined;
  /** 两块数据都取失败且无任何数据 → 整页错误重试态（照 SyncPage 的硬错误） */
  const hardError =
    !loading &&
    accounts == null &&
    overview == null &&
    accountsError != null &&
    overviewError != null;

  return (
    <div className="db-page db-acc-page">
      <header className="db-page__header db-page__header--sticky">
        <div>
          <h1 className="db-page__title">{t("dashboard.accounts")}</h1>
          <p className="db-page__desc">{t("dashboard.accounts.desc")}</p>
        </div>
        <div className="db-page__actions">
          {/* 操作反馈锚点（吸顶）：错误优先于成功提示；长错误截断、title 看全文 */}
          {actionError ? (
            <span className="db-acc__error" title={actionError}>
              {actionError}
            </span>
          ) : flash ? (
            <span className="db-acc__flash">{flash}</span>
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

      {hardError ? (
        /* 两块都失败且无数据：整页错误重试态（db-errorstate，照 SyncPage） */
        <div className="db-errorstate">
          <div className="db-errorstate__title">
            {t("dashboard.accounts.errorTitle")}
          </div>
          <div className="db-errorstate__reason">
            {t("dashboard.accounts.errorReason", {
              error: accountsError ?? overviewError ?? "",
            })}
          </div>
          <button
            type="button"
            className="db-btn"
            onClick={() => void refreshAll()}
          >
            <RefreshIcon className="h-3.5 w-3.5" />
            {t("dashboard.accounts.retry")}
          </button>
        </div>
      ) : loading && accounts == null && overview == null ? (
        /* 首屏骨架：账号卡 + 概览卡的形状占位 */
        <>
          <div className="db-card">
            <div className="db-card__body">
              <Skeleton className="h-32 w-full" />
            </div>
          </div>
          <div className="db-card">
            <div className="db-card__body">
              <Skeleton className="h-48 w-full" />
            </div>
          </div>
        </>
      ) : (
        <>
          {/* ---- 区块①：ZCode 账号（快照表）---- */}
          <Card
            title={t("dashboard.accounts.zcodeTitle")}
            sub={
              accounts
                ? t("dashboard.accounts.zcodeSub", {
                    count: accounts.accounts.length,
                  })
                : undefined
            }
            action={
              <button
                type="button"
                className="db-btn db-btn--primary"
                onClick={() => void handleCapture()}
                disabled={busy}
              >
                {capturing
                  ? t("dashboard.accounts.capturing")
                  : t("dashboard.accounts.capture")}
              </button>
            }
          >
            {/* 顶部说明：后台每分钟自动捕获 + 手动捕获入口（快照仅存本机） */}
            <p className="db-acc-hint">{t("dashboard.accounts.autoCaptureHint")}</p>

            {accountsError && accounts == null ? (
              <AlertBanner type="error">
                {t("dashboard.accounts.accountsError", { error: accountsError })}
              </AlertBanner>
            ) : accounts == null ? (
              <Skeleton className="h-24 w-full" />
            ) : (
              <>
                {/* 当前登录行：匹配快照名 / 未识别（ZCode 源提取不到 email，
                    副文本的邮箱/指纹前 8 位回落值恒为误导信息，不再展示） */}
                <div className="db-acc-current">
                  <span className="db-acc-current__label">
                    {t("dashboard.accounts.currentLogin")}
                  </span>
                  <span className="db-acc-current__name">
                    {current
                      ? (currentName ?? t("dashboard.accounts.unknown"))
                      : t("dashboard.accounts.unknown")}
                  </span>
                </div>

                {accounts.accounts.length === 0 ? (
                  <p className="db-acc-empty">{t("dashboard.accounts.empty")}</p>
                ) : (
                  <div className="db-table-wrap">
                    <table className="db-table db-acc-accounts">
                      <thead>
                        <tr>
                          <th>{t("dashboard.accounts.colName")}</th>
                          <th>{t("dashboard.accounts.colProvider")}</th>
                          <th>{t("dashboard.accounts.colUpdated")}</th>
                          <th className="db-acc-accounts__ops">
                            {t("dashboard.accounts.colOps")}
                          </th>
                        </tr>
                      </thead>
                      <tbody>
                        {accounts.accounts.map((a) =>
                          renameId === a.id ? (
                            /* 行内重命名：账号名格变输入框（Enter 提交 /
                               Esc 取消 / blur 取消——误触提交比误取消更难
                               挽救，宁可不提交）；32 字上限与弹层改名一致 */
                            <RenameRow
                              key={a.id}
                              draft={renameDraft}
                              busy={renamingBusy}
                              onChange={setRenameDraft}
                              onCommit={(name) =>
                                void handleRenameCommit(a.id, name)
                              }
                              onCancel={() => setRenameId(null)}
                            />
                          ) : (
                            <tr key={a.id}>
                              <td>
                                <span className="db-acc-name">
                                  {a.display_name}
                                  {a.is_current && (
                                    <span className="db-acc-badge">
                                      {t("dashboard.accounts.currentBadge")}
                                    </span>
                                  )}
                                </span>
                              </td>
                              <td title={a.login_provider ?? undefined}>
                                {a.login_provider ?? (
                                  <span className="db-acc-nodata">
                                    {t("dashboard.sync.noData")}
                                  </span>
                                )}
                              </td>
                              <td
                                className="db-acc-time"
                                title={
                                  a.updated_at
                                    ? formatDateTime(a.updated_at)
                                    : t("dashboard.sync.never")
                                }
                              >
                                {timeAgo(t, a.updated_at)}
                              </td>
                              <td className="db-acc-accounts__ops">
                                {/* 切换到自己是无意义操作：当前登录行不渲染
                                    「切换」键，只留重命名/删除；非当前行三键齐全 */}
                                {!a.is_current && (
                                  <button
                                    type="button"
                                    className="db-linkbtn"
                                    onClick={() => setConfirm({ kind: "switch", account: a })}
                                    disabled={busy}
                                    title={t("dashboard.accounts.switch")}
                                  >
                                    {switchingId === a.id
                                      ? t("dashboard.accounts.switching")
                                      : t("dashboard.accounts.switch")}
                                  </button>
                                )}
                                <button
                                  type="button"
                                  className="db-linkbtn"
                                  onClick={() => {
                                    setRenameId(a.id);
                                    setRenameDraft(a.display_name);
                                  }}
                                  disabled={busy}
                                >
                                  {t("dashboard.accounts.rename")}
                                </button>
                                <button
                                  type="button"
                                  className="db-linkbtn db-acc-del"
                                  onClick={() => setConfirm({ kind: "delete", account: a })}
                                  disabled={busy}
                                >
                                  {deletingId === a.id
                                    ? t("dashboard.accounts.deleting")
                                    : t("dashboard.accounts.delete")}
                                </button>
                              </td>
                            </tr>
                          )
                        )}
                      </tbody>
                    </table>
                  </div>
                )}
              </>
            )}
          </Card>

          {/* ---- 区块②：其他源凭证一览（只读静态信息）---- */}
          <Card
            title={t("dashboard.accounts.overviewTitle")}
            sub={overviewSub}
          >
            {/* 安全说明：只读元信息，不展示凭证内容、不做主动联网校验 */}
            <p className="db-acc-hint">{t("dashboard.accounts.overviewHint")}</p>

            {overviewError && overview == null ? (
              <AlertBanner type="error">
                {t("dashboard.accounts.overviewError", { error: overviewError })}
              </AlertBanner>
            ) : overview == null ? (
              <Skeleton className="h-32 w-full" />
            ) : (
              <div className="db-table-wrap">
                <table className="db-table db-acc-sources">
                  <thead>
                    <tr>
                      <th>{t("dashboard.accounts.colSource")}</th>
                      <th>{t("dashboard.accounts.colAccount")}</th>
                      <th>{t("dashboard.accounts.colKind")}</th>
                      <th>{t("dashboard.accounts.colPath")}</th>
                      <th>{t("dashboard.accounts.colModified")}</th>
                      <th>{t("dashboard.accounts.colStatus")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {overview.map((o) => {
                      /** 后端新增源时前端品牌名兜底为源 id 本身 */
                      const name = SOURCE_NAMES[o.source] ?? o.source;
                      const missing = !o.exists && o.error == null;
                      return (
                        <tr key={o.source} className={missing ? "db-acc-miss" : ""}>
                          <td>{name}</td>
                          <td title={o.account ?? undefined}>
                            {o.account ?? (
                              <span className="db-acc-nodata">
                                {t("dashboard.sync.noData")}
                              </span>
                            )}
                          </td>
                          <td>
                            {o.exists && o.error == null ? (
                              <span
                                className={`db-acc-kind db-acc-kind--${o.kind === "oauth" || o.kind === "apiKey" ? o.kind : "unknown"}`}
                              >
                                {t(kindLabelKey(o.kind))}
                              </span>
                            ) : (
                              <span className="db-acc-nodata">
                                {t("dashboard.sync.noData")}
                              </span>
                            )}
                          </td>
                          <td className="num db-acc-path" title={o.path}>
                            {o.path}
                          </td>
                          <td
                            className="db-acc-time"
                            title={
                              o.modified_at
                                ? formatDateTime(o.modified_at)
                                : t("dashboard.sync.noData")
                            }
                          >
                            {missing ? (
                              <span className="db-acc-nodata">
                                {t("dashboard.sync.noData")}
                              </span>
                            ) : (
                              timeAgo(t, o.modified_at)
                            )}
                          </td>
                          <td>
                            {o.error ? (
                              <span
                                className="db-acc-status db-acc-status--bad"
                                title={o.error}
                              >
                                {t("dashboard.accounts.sourceError")}
                              </span>
                            ) : missing ? (
                              <span className="db-acc-status db-acc-status--dim">
                                {t("dashboard.accounts.statusMissing")}
                              </span>
                            ) : (
                              <span className="db-acc-status db-acc-status--ok">
                                {t("dashboard.accounts.statusFound")}
                              </span>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </Card>
        </>
      )}

      {/* ---- 确认弹层（切换登录 / 删除快照）：大窗同构形态（照 SyncPage 的
          db-confirm）。切换不删数据用普通主色键（与 AccountsCard 的天蓝确认
          键一致），删除为破坏性红键。busy 期间两键禁用（防双击重复请求）---- */}
      {confirm && (
        <ConfirmDialog
          state={confirm}
          busy={busy}
          onCancel={() => setConfirm(null)}
          onConfirm={() => {
            if (confirm.kind === "switch") void handleSwitch(confirm.account);
            else void handleDelete(confirm.account);
          }}
        />
      )}
    </div>
  );
}

/** 确认弹层：按 kind 切换标题 / 描述 / 确认键文案。切换后果文案与设置页
 *  AccountsCard 逐字一致（退出并重启 ZCode / 自动备份 / 失败回滚）；删除
 *  确认基础后果与 settings.accountsRemoveConfirm 一致（只清本应用快照），
 *  主窗口版额外说明当前登录账号快照会被自动捕获重建。Escape 关闭、确认
 *  键 busy 禁用。文件级组件：弹层打开期间父级重渲染不重挂载。 */
function ConfirmDialog({
  state,
  busy,
  onCancel,
  onConfirm,
}: {
  state: ConfirmState;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useI18n();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const isSwitch = state.kind === "switch";
  const title = isSwitch
    ? t("dashboard.accounts.confirmSwitchTitle")
    : t("dashboard.accounts.confirmDeleteTitle");
  const desc = isSwitch
    ? t("dashboard.accounts.confirmSwitchDesc", {
        name: state.account.display_name,
      })
    : t("dashboard.accounts.confirmDeleteDesc");
  const confirmLabel = isSwitch
    ? t("dashboard.accounts.confirmSwitch")
    : t("dashboard.accounts.confirmDelete");

  return (
    <div className="db-confirm-mask">
      <div className="db-confirm" role="alertdialog" aria-modal="true">
        <div className="db-confirm__title">{title}</div>
        <p className="db-confirm__desc">{desc}</p>
        <div className="db-confirm__actions">
          <button type="button" className="db-btn" onClick={onCancel} disabled={busy}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className={`db-btn ${isSwitch ? "db-btn--primary" : "db-btn--danger"}`}
            onClick={onConfirm}
            disabled={busy}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 行内重命名行（账号名列展开为整行编辑态）：输入框 + 字数 + 保存/取消。
 *  Enter 提交、Escape 取消、blur 取消（误触提交比误取消更难挽救）；32 字
 *  上限与后端 rename_account 的截断兜底一致。 */
function RenameRow({
  draft,
  busy,
  onChange,
  onCommit,
  onCancel,
}: {
  draft: string;
  busy: boolean;
  onChange: (v: string) => void;
  onCommit: (name: string) => void;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const trimmed = draft.trim();
  const valid = trimmed.length > 0 && trimmed.length <= 32;

  return (
    <tr>
      <td colSpan={4}>
        <span className="db-acc-rename">
          <input
            type="text"
            value={draft}
            maxLength={32}
            autoFocus
            disabled={busy}
            aria-label={t("dashboard.accounts.rename")}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && valid && !busy) onCommit(trimmed);
              if (e.key === "Escape") onCancel();
            }}
            onBlur={() => {
              if (!busy) onCancel();
            }}
          />
          <span className="db-acc-rename__count num">
            {t("dashboard.accounts.renameCount", { n: trimmed.length })}
          </span>
          <button
            type="button"
            className="db-btn db-btn--sm db-btn--primary"
            disabled={!valid || busy}
            onMouseDown={(e) => e.preventDefault() /* 保住输入框焦点，走 blur 前 onClick */}
            onClick={() => onCommit(trimmed)}
          >
            {t("dashboard.accounts.renameSave")}
          </button>
          <button
            type="button"
            className="db-btn db-btn--sm"
            disabled={busy}
            onClick={onCancel}
          >
            {t("common.cancel")}
          </button>
        </span>
      </td>
    </tr>
  );
}
