/**
 * 「Agent 与 Skill」管理页（主窗口独立入口）。
 *
 * 覆盖方案首期交互闭环（doc/agent-skill-management-plan.md §7 / §42）：
 * ① 顶部服务端状态（能力接口探测；CONFIG_MISSING / OLD_SERVER_UNSUPPORTED /
 *    NETWORK_ERROR / UNAUTHORIZED 都是一等状态，不白屏）；
 * ② 工具 × 类型 × 作用域选择（能力矩阵四档，pending 条目不可操作），项目级
 *    需项目根路径（可从本机已知项目补全或手填）；
 * ③ 本机资源发现 → 展开 inspect（元数据 / 文件清单 / 敏感字段 / 包外引用 /
 *    frontmatter 原文，二进制条目不预览内容）→ 打包预览 / 上传（先预览确认，
 *    断网重试复用同一 client_request_id）/ 导出到本机目录；
 * ④ 远端资源库（仅服务端 supported 时启用）：筛选分页 / 版本历史 / 下载
 *    （fetch_bundle → 指纹展示 → export_to_disk）/ 安装（fetch_bundle →
 *    install，目标作用域与项目根可选，installable=false 置灰）；
 * ⑤ 上传已有同名资源时走 update_remote：409 VERSION_CONFLICT 展示服务端
 *    current_version，并给「重新预览后再试」（重新 plan/pack）与「另存为新
 *    资源」（改走 POST + 新请求 ID）两条路。
 *
 * 数据契约见 doc/agent-assets-contract.md；命令错误统一是 `{code,message,
 * details?}` 对象，catch 后按 code 分支（UNAUTHORIZED / OLD_SERVER_UNSUPPORTED /
 * ADAPTER_NOT_FOUND / VERSION_CONFLICT / NETWORK_ERROR 等）。
 *
 * 样式复用 dashboard.css 的既有类（db-page / db-card / db-btn / db-seg /
 * db-search / db-confirm / db-table 等），自定义部分只用 Tailwind 工具类与
 * --db-* 变量，保证亮暗主题与窄窗口自动适配；不新增 CSS 文件。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import { open as openNativeDialog } from "@tauri-apps/plugin-dialog";
import { useI18n, type TFn } from "../i18n";
import { AlertBanner } from "../layout";
import { Card, Skeleton } from "./primitives";
import { RefreshIcon } from "./icons";
import {
  discoverAgentAssets,
  exportAgentAssetBundleToDisk,
  fetchAgentAssetBundle,
  getAgentAssetCapabilities,
  getAgentAssetServerStatus,
  getProjects,
  importAgentAssetFiles,
  inspectAgentAsset,
  installAgentAssetBundle,
  listAgentAssetRevisions,
  listRemoteAgentAssets,
  packAgentAsset,
  planAgentAssetExport,
  updateRemoteAgentAsset,
  uploadAgentAsset,
} from "../api";
import type {
  AssetBundle,
  AssetErrorInfo,
  AssetKind,
  AssetScope,
  CapabilityEntry,
  DiscoveredAsset,
  DiskExportResult,
  ExportPlan,
  ImportCandidate,
  InspectionReport,
  InstallResult,
  RemoteAsset,
  RemoteAssetPage,
  RemoteAssetResult,
  RemoteRevision,
  ServerAssetStatus,
} from "../api";

// ===== 常量 =====

/** 工具展示顺序与品牌名（契约 §2.1；品牌名不翻译，与全站口径一致）。 */
const TOOL_ORDER: { id: string; name: string }[] = [
  { id: "zcode", name: "ZCode" },
  { id: "codex", name: "Codex" },
  { id: "claude", name: "Claude Code" },
  { id: "kimi", name: "Kimi Code" },
  { id: "opencode", name: "OpenCode" },
  { id: "dsh", name: "DeepSeek Harness" },
  { id: "minimax", name: "MiniMax Code" },
];

const KIND_ORDER: AssetKind[] = ["agent", "skill"];
const SCOPE_ORDER: AssetScope[] = ["user", "project"];
/** 远端列表每页条数（契约 §8.2：page_size ≤ 100） */
const REMOTE_PAGE_SIZE = 20;
/** 项目根补全候选的取数窗口：近 90 天用过的项目 */
const PROJECT_LOOKBACK_MS = 90 * 24 * 60 * 60 * 1000;
/** 项目根候选下拉的 id（<datalist>） */
const PROJECT_DATALIST_ID = "agenthud-agent-assets-projects";

// ===== 通用小工具 =====

/** 字节数展示（契约容量一律按二进制单位 KiB/MiB/GiB）。 */
function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${Math.round(n)} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = n;
  let idx = -1;
  do {
    value /= 1024;
    idx += 1;
  } while (value >= 1024 && idx < units.length - 1);
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[idx]}`;
}

/** 本地时区绝对时间（YYYY-MM-DD HH:mm:ss），资源版本/更新时间展示用。 */
function formatDateTime(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "—";
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(
    d.getHours()
  )}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** 哈希缩写（完整值放 title）。 */
function shortHash(hash: string): string {
  if (!hash) return "—";
  return hash.length > 16 ? `${hash.slice(0, 16)}…` : hash;
}

/**
 * Tauri 错误归一：契约命令的 Err 是 `{code,message,details?}` 对象；
 * 命令未注册 / ACL 拒绝等宿主错误是字符串。两类都折成统一形状，
 * 前端只按 code 分支（字符串错误归 UNKNOWN，保留原文供展示）。
 */
function toAssetError(e: unknown): AssetErrorInfo {
  if (e && typeof e === "object" && "code" in e) {
    const obj = e as {
      code?: unknown;
      message?: unknown;
      details?: unknown;
    };
    return {
      code: typeof obj.code === "string" ? obj.code : "INTERNAL",
      message: typeof obj.message === "string" ? obj.message : "",
      details:
        obj.details && typeof obj.details === "object"
          ? (obj.details as Record<string, unknown>)
          : undefined,
    };
  }
  return {
    code: "UNKNOWN",
    message: typeof e === "string" ? e : String(e),
  };
}

/** 错误提示正文：在通用错误码上追加本地化引导，后端 message 原样保留。 */
function errorText(t: TFn, err: AssetErrorInfo): string {
  const base = err.message;
  switch (err.code) {
    case "UNAUTHORIZED":
      return `${t("dashboard.agentAssets.error.unauthorized")} ${base}`;
    case "OLD_SERVER_UNSUPPORTED":
      return `${t("dashboard.agentAssets.error.oldServer")} ${base}`;
    case "NETWORK_ERROR":
      return `${t("dashboard.agentAssets.error.network")} ${base}`;
    case "ADAPTER_NOT_FOUND":
      return `${t("dashboard.agentAssets.error.adapter")} ${base}`;
    default:
      return base || t("dashboard.agentAssets.server.unknown");
  }
}

/** 命令未提供（C/D 未合并 / ACL 拒绝）时给出人读引导，其余错误保持原样。 */
function actionErrorText(t: TFn, err: AssetErrorInfo): string {
  const lower = err.message.toLowerCase();
  if (
    err.code === "NOT_IMPLEMENTED" ||
    (err.code === "UNKNOWN" &&
      (lower.includes("not found") ||
        lower.includes("not allowed") ||
        lower.includes("not registered")))
  ) {
    return t("dashboard.agentAssets.transfer.commandUnavailable", {
      error: err.message,
    });
  }
  return errorText(t, err);
}

/** 从 details 读数值（409 的 current_version 等）。 */
function numberDetail(err: AssetErrorInfo, key: string): number | null {
  const raw = err.details?.[key];
  if (typeof raw === "number" && Number.isFinite(raw)) return raw;
  if (typeof raw === "string" && raw.trim() !== "") {
    const n = Number(raw);
    if (Number.isFinite(n)) return n;
  }
  return null;
}

/** UUIDv4：创建资源的幂等键（断网重试必须复用；仅在重新预览/另存为新资源时换新）。 */
function newRequestId(): string {
  try {
    if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
      return crypto.randomUUID();
    }
  } catch {
    /* 极老 WebView 无 randomUUID：走下方手工生成 */
  }
  const bytes = new Uint8Array(16);
  try {
    if (typeof crypto !== "undefined" && crypto.getRandomValues) {
      crypto.getRandomValues(bytes);
    } else {
      throw new Error("no crypto");
    }
  } catch {
    for (let i = 0; i < bytes.length; i += 1) {
      bytes[i] = Math.floor(Math.random() * 256);
    }
  }
  // RFC 4122 version 4 variant
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(
    16,
    20
  )}-${hex.slice(20)}`;
}

/** 资源唯一键（发现列表 / inspect 展开态共用）。 */
function assetKey(asset: DiscoveredAsset): string {
  return `${asset.tool}\u0000${asset.kind}\u0000${asset.scope}\u0000${asset.native_id}`;
}

/** 导出预览时的文件清单基线（path+size 集合，用于上传前发现内容变化）。 */
interface PreviewFileEntry {
  path: string;
  size: number;
}

/** 重新打包后的文件清单是否与预览时完全一致（path+size 集合）。 */
function sameFileList(
  preview: PreviewFileEntry[],
  packed: { path: string; size: number }[]
): boolean {
  if (preview.length !== packed.length) return false;
  const expected = new Set(preview.map((f) => `${f.path}\u0000${f.size}`));
  return packed.every((f) => expected.has(`${f.path}\u0000${f.size}`));
}

/** Escape 关闭弹层（与 AccountsPage 的 ConfirmDialog 同款）。 */
function useEscape(active: boolean, onClose: () => void) {
  useEffect(() => {
    if (!active) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active, onClose]);
}

// ===== 页面 =====

export function AgentAssetsPage() {
  const { t } = useI18n();

  // ---- 能力矩阵与服务端状态 ----
  const [capabilities, setCapabilities] = useState<CapabilityEntry[] | null>(
    null
  );
  const [capabilitiesError, setCapabilitiesError] = useState<string | null>(
    null
  );
  const [server, setServer] = useState<ServerAssetStatus | null>(null);
  const [serverBusy, setServerBusy] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  // ---- 选择器（工具 / 类型 / 作用域 / 项目根）----
  const [tool, setTool] = useState("zcode");
  const [kind, setKind] = useState<AssetKind>("agent");
  const [scope, setScope] = useState<AssetScope>("user");
  const [projectRoot, setProjectRoot] = useState("");
  // 项目根候选（近 90 天用量里出现过的项目路径；页面打开时懒加载一次）
  const [knownProjects, setKnownProjects] = useState<string[] | null>(null);
  // 最新项目根的 ref 镜像：扫描/检查回调读它，避免项目根逐字击键触发重扫码
  const projectRootRef = useRef(projectRoot);
  projectRootRef.current = projectRoot;
  const serverRef = useRef(server);
  serverRef.current = server;
  const lastDestDirRef = useRef("");
  // confirmUpload 回调内需要刷新远端列表；loadRemote 在其后定义，用 ref 接线
  const loadRemoteRef = useRef<(() => void) | null>(null);

  // ---- 本机资源 ----
  const [assets, setAssets] = useState<DiscoveredAsset[] | null>(null);
  const [localLoading, setLocalLoading] = useState(false);
  const [localError, setLocalError] = useState<AssetErrorInfo | null>(null);
  const [localActionError, setLocalActionError] = useState<AssetErrorInfo | null>(
    null
  );
  const [actionBusy, setActionBusy] = useState<{
    key: string;
    action: "plan" | "upload" | "export";
  } | null>(null);
  const localReq = useRef(0);

  // ---- 手选原生文件/目录导入（契约 §7.3）----
  // 手选候选与 discover 项分开存放：候选自带完整 inspect 预览，且后续
  // plan/pack 必须携带 sourceOverride（显式资源根），不能走 discover 根定位。
  const [importOpen, setImportOpen] = useState(false);
  const [importBusy, setImportBusy] = useState(false);
  const [importCandidates, setImportCandidates] = useState<
    ImportCandidate[] | null
  >(null);
  const [importError, setImportError] = useState<AssetErrorInfo | null>(null);
  const importReq = useRef(0);

  // ---- 展开的 inspect ----
  const [expandedKey, setExpandedKey] = useState<string | null>(null);
  const [inspect, setInspect] = useState<{
    key: string;
    report: InspectionReport;
  } | null>(null);
  const [inspectLoading, setInspectLoading] = useState(false);
  const [inspectError, setInspectError] = useState<AssetErrorInfo | null>(
    null
  );
  const inspectReq = useRef(0);

  // ---- 本地操作弹层 ----
  const [planDialog, setPlanDialog] = useState<{
    plan: ExportPlan;
    asset: DiscoveredAsset;
  } | null>(null);
  const [uploadDialog, setUploadDialog] = useState<UploadDialogState | null>(
    null
  );
  const [exportDialog, setExportDialog] = useState<ExportDialogState | null>(
    null
  );
  const [dirPickerHint, setDirPickerHint] = useState<string | null>(null);

  // ---- 远端资源库 ----
  const [remoteFilterTool, setRemoteFilterTool] = useState("");
  const [remoteFilterKind, setRemoteFilterKind] = useState<"" | AssetKind>("");
  const [remoteFilterScope, setRemoteFilterScope] = useState<"" | AssetScope>(
    ""
  );
  const [remoteQueryInput, setRemoteQueryInput] = useState("");
  const [remoteQuery, setRemoteQuery] = useState("");
  const [remotePage, setRemotePage] = useState(1);
  const [remotePageData, setRemotePageData] = useState<RemoteAssetPage | null>(
    null
  );
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [remoteError, setRemoteError] = useState<AssetErrorInfo | null>(null);
  const remoteReq = useRef(0);

  // 版本历史（同一时间只展开一个资源的版本列表）
  const [historyAssetId, setHistoryAssetId] = useState<string | null>(null);
  const [revisions, setRevisions] = useState<RemoteRevision[] | null>(null);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<AssetErrorInfo | null>(null);
  const historyReq = useRef(0);

  // 远端下载 / 安装弹层
  const [remoteDialog, setRemoteDialog] = useState<RemoteDialogState | null>(
    null
  );
  const remoteDialogSeq = useRef(0);

  // ===== 派生的能力条目 =====
  const entry = useMemo(
    () =>
      capabilities?.find(
        (c) => c.tool === tool && c.kind === kind && c.scope === scope
      ) ?? null,
    [capabilities, tool, kind, scope]
  );
  const effectiveProjectRoot =
    scope === "project" ? projectRoot.trim() : null;
  const scanBlockedReason = useMemo(() => {
    if (!capabilities) return null;
    if (!entry) return t("dashboard.agentAssets.select.unsupported");
    if (entry.status !== "verified") {
      return t("dashboard.agentAssets.select.pendingHint");
    }
    if (!entry.scannable) return t("dashboard.agentAssets.select.notScannable");
    if (scope === "project" && !effectiveProjectRoot) {
      return t("dashboard.agentAssets.select.projectRootRequired");
    }
    return null;
  }, [capabilities, entry, scope, effectiveProjectRoot, t]);
  const canScan = capabilities != null && scanBlockedReason == null;
  const canArchive =
    entry != null && entry.status === "verified" && entry.archivable;
  // 手选导入固定按用户级记录（契约 §7.3），入口只看 tool × kind × user 的能力；
  // 与当前页面 scope 无关（项目级视图下也可导入用户级资源）。
  const importEntry = useMemo(
    () =>
      capabilities?.find(
        (c) => c.tool === tool && c.kind === kind && c.scope === "user"
      ) ?? null,
    [capabilities, tool, kind]
  );
  const importCanArchive =
    importEntry != null &&
    importEntry.status === "verified" &&
    importEntry.archivable;
  const importBlockedReason = useMemo(() => {
    if (!capabilities) return t("common.loading");
    if (!importEntry) {
      return t("dashboard.agentAssets.import.blockedUnsupported");
    }
    if (importEntry.status !== "verified") {
      return t("dashboard.agentAssets.import.blockedPending");
    }
    if (!importEntry.archivable) {
      return t("dashboard.agentAssets.import.blockedUnsupported");
    }
    return null;
  }, [capabilities, importEntry, t]);
  const serverSupported = server?.supported === true;
  const uploadBlockedReason = useMemo(() => {
    if (!serverSupported) {
      if (!server) return t("dashboard.agentAssets.server.checking");
      if (server.reason === "OLD_SERVER_UNSUPPORTED") {
        return t("dashboard.agentAssets.error.oldServer");
      }
      if (server.reason === "CONFIG_MISSING") {
        return t("dashboard.agentAssets.server.configMissing");
      }
      if (server.reason === "UNAUTHORIZED") {
        return t("dashboard.agentAssets.error.unauthorized");
      }
      if (server.reason === "NETWORK_ERROR") {
        return t("dashboard.agentAssets.server.network");
      }
      return t("dashboard.agentAssets.remote.disabled");
    }
    return null;
  }, [server, serverSupported, t]);

  /** 全量能力矩阵加载（失败可重试；失败时仍可用本地页面但选择器无能力信息）。 */
  const loadCapabilities = useCallback(async () => {
    setCapabilitiesError(null);
    try {
      const list = await getAgentAssetCapabilities();
      setCapabilities(list);
    } catch (e) {
      setCapabilities([]);
      setCapabilitiesError(String(e));
    }
  }, []);

  /** 服务端能力探测（契约 §7：命令本身不抛错，异常也折成状态对象）。 */
  const loadServerStatus = useCallback(async () => {
    setServerBusy(true);
    setServerError(null);
    try {
      const status = await getAgentAssetServerStatus();
      setServer(status);
    } catch (e) {
      setServer(null);
      setServerError(String(e));
    } finally {
      setServerBusy(false);
    }
  }, []);

  /**
   * 扫描本机资源。项目根从 ref 读最新值（输入框逐字击键不触发本函数），
   * 工具/类型/作用域变化由下方 effect 自动触发；请求序号守卫旧响应。
   */
  const loadLocal = useCallback(async () => {
    const reqId = ++localReq.current;
    setLocalActionError(null);
    setLocalError(null);
    // 列表即将重扫：收起旧的 inspect 展开态，避免展示已过期内容
    setExpandedKey(null);
    setInspect(null);
    setInspectError(null);
    if (!entry || entry.status !== "verified" || !entry.scannable) {
      setAssets(null);
      setLocalLoading(false);
      return;
    }
    const root =
      scope === "project" ? projectRootRef.current.trim() : null;
    if (scope === "project" && !root) {
      setAssets(null);
      setLocalLoading(false);
      return;
    }
    setLocalLoading(true);
    try {
      const list = await discoverAgentAssets(tool, kind, scope, root);
      if (reqId !== localReq.current) return;
      setAssets(list);
    } catch (e) {
      if (reqId !== localReq.current) return;
      setAssets(null);
      setLocalError(toAssetError(e));
    } finally {
      if (reqId === localReq.current) setLocalLoading(false);
    }
  }, [entry, tool, kind, scope]);

  // 首屏：能力矩阵 + 服务端状态（并行）
  useEffect(() => {
    void loadCapabilities();
    void loadServerStatus();
  }, [loadCapabilities, loadServerStatus]);

  // 工具/类型/作用域（或能力矩阵就绪）变化后自动扫描；项目根只在点「扫描」时读最新值
  useEffect(() => {
    void loadLocal();
  }, [loadLocal]);

  // 已知项目路径（项目根输入补全；页面打开时懒加载一次，失败静默）
  useEffect(() => {
    if (knownProjects != null) return;
    const to = Date.now();
    const from = to - PROJECT_LOOKBACK_MS;
    getProjects(from, to)
      .then((list) =>
        setKnownProjects(
          list
            .filter((p) => !p.is_unknown && p.display_path)
            .map((p) => p.display_path as string)
        )
      )
      .catch(() => setKnownProjects([]));
  }, [knownProjects]);

  // 工具/类型变化即切换上下文：手选导入结果属于旧上下文，清空并在途结果作废
  useEffect(() => {
    importReq.current += 1;
    setImportOpen(false);
    setImportBusy(false);
    setImportCandidates(null);
    setImportError(null);
  }, [tool, kind]);

  // ===== 本机资源操作 =====

  const handleScan = useCallback(() => {
    setExpandedKey(null);
    setInspect(null);
    setInspectError(null);
    void loadLocal();
  }, [loadLocal]);

  /**
   * 手选原生文件/目录导入（契约 §7.3）：
   * agent 走向导文件多选（按工具给扩展名过滤），skill 走目录选择；
   * 选完调 agent_assets_import_files 逐项展示结果，不识别项不阻断整批。
   */
  const handleImportPick = useCallback(async () => {
    if (!importCanArchive || importBusy) return;
    let picked: string | string[] | null = null;
    try {
      if (kind === "agent") {
        picked = await openNativeDialog({
          multiple: true,
          directory: false,
          title: t("dashboard.agentAssets.import.pickAgentTitle"),
          filters: [
            {
              name: t("dashboard.agentAssets.import.agentFilter"),
              extensions: agentImportExtensions(tool),
            },
          ],
        });
      } else {
        try {
          picked = await openNativeDialog({
            multiple: true,
            directory: true,
            title: t("dashboard.agentAssets.import.pickSkillTitle"),
          });
        } catch {
          // 部分平台的目录多选不可用：退回单选目录
          picked = await openNativeDialog({
            multiple: false,
            directory: true,
            title: t("dashboard.agentAssets.import.pickSkillTitle"),
          });
        }
      }
    } catch (e) {
      setImportOpen(true);
      setImportBusy(false);
      setImportCandidates(null);
      setImportError({
        code: "IO_ERROR",
        message: t("dashboard.agentAssets.import.pickFailed", {
          error: String(e),
        }),
      });
      return;
    }

    const paths = Array.from(
      new Set(
        Array.isArray(picked)
          ? picked
          : typeof picked === "string" && picked
            ? [picked]
            : []
      )
    );
    if (paths.length === 0) return;

    const reqId = ++importReq.current;
    setImportOpen(true);
    setImportBusy(true);
    setImportCandidates(null);
    setImportError(null);
    try {
      const items = await importAgentAssetFiles(tool, kind, paths);
      if (reqId !== importReq.current) return;
      setImportCandidates(items);
    } catch (e) {
      if (reqId !== importReq.current) return;
      setImportError(toAssetError(e));
    } finally {
      if (reqId === importReq.current) setImportBusy(false);
    }
  }, [importBusy, importCanArchive, kind, t, tool]);

  /** 清空手选导入结果（同时作废在途请求）。 */
  const clearImport = useCallback(() => {
    importReq.current += 1;
    setImportOpen(false);
    setImportBusy(false);
    setImportCandidates(null);
    setImportError(null);
  }, []);

  const toggleInspect = useCallback(async (asset: DiscoveredAsset) => {
    const key = assetKey(asset);
    if (expandedKey === key) {
      setExpandedKey(null);
      setInspect(null);
      setInspectError(null);
      return;
    }
    setExpandedKey(key);
    setInspect(null);
    setInspectError(null);
    const reqId = ++inspectReq.current;
    setInspectLoading(true);
    try {
      const root =
        asset.scope === "project" ? projectRootRef.current.trim() : null;
      const report = await inspectAgentAsset(
        asset.tool,
        asset.kind,
        asset.scope,
        asset.native_id,
        root
      );
      if (reqId !== inspectReq.current) return;
      setInspect({ key, report });
    } catch (e) {
      if (reqId !== inspectReq.current) return;
      setInspectError(toAssetError(e));
    } finally {
      if (reqId === inspectReq.current) setInspectLoading(false);
    }
  }, [expandedKey]);

  /** 查同名远端资源（上传时判断走 POST 新建还是 PUT 更新）。 */
  const findRemoteMatches = useCallback(
    async (asset: DiscoveredAsset): Promise<RemoteAsset[]> => {
      const page = await listRemoteAgentAssets({
        tool: asset.tool,
        kind: asset.kind,
        scope: asset.scope,
        q: asset.native_id,
        pageSize: 100,
      });
      return page.items.filter(
        (item) =>
          item.tool === asset.tool &&
          item.kind === asset.kind &&
          item.native_id === asset.native_id &&
          item.origin_scope === asset.scope
      );
    },
    []
  );

  /** 生成导出计划并打开对应弹层（预览 / 上传确认 / 导出）。 */
  const openPlanFlow = useCallback(
    async (
      ref: LocalAssetRef,
      action: "plan" | "upload" | "export"
    ) => {
      const { asset, sourceOverride } = ref;
      setActionBusy({ key: ref.key, action });
      setLocalActionError(null);
      try {
        const root =
          asset.scope === "project" ? projectRootRef.current.trim() : null;
        const plan = await planAgentAssetExport(
          asset.tool,
          asset.kind,
          asset.scope,
          asset.native_id,
          root,
          sourceOverride
        );
        // 上传预览额外记录实际打包出的 path+size 基线：确认时重新 pack，
        // 内容或文件集合变化则中止（避免上传与预览不一致的包）。
        let previewFiles: PreviewFileEntry[] = [];
        if (action === "upload") {
          const previewBundle = await packAgentAsset(
            asset.tool,
            asset.kind,
            asset.scope,
            asset.native_id,
            root,
            sourceOverride
          );
          previewFiles = previewBundle.manifest.files.map((file) => ({
            path: file.path,
            size: file.size,
          }));
        }
        if (action === "plan") {
          setPlanDialog({ plan, asset });
          return;
        }
        if (action === "export") {
          setDirPickerHint(null);
          setExportDialog({
            plan,
            asset,
            sourceOverride,
            destDir: lastDestDirRef.current,
            overwrite: false,
            busy: false,
            error: null,
            result: null,
          });
          return;
        }
        // 上传：服务端 supported 时顺带查同名远端资源，供用户选择更新目标
        let matches: RemoteAsset[] = [];
        let matchError: string | null = null;
        if (serverRef.current?.supported) {
          try {
            matches = await findRemoteMatches(asset);
          } catch (e) {
            matchError = errorText(t, toAssetError(e));
          }
        }
        const preferred = matches.length === 1 ? matches[0] : null;
        setDirPickerHint(null);
        setUploadDialog({
          plan,
          asset,
          sourceOverride,
          matches,
          matchError,
          targetAssetId: preferred?.asset_id ?? null,
          expectedVersion: preferred?.latest_version ?? null,
          requestId: newRequestId(),
          previewFiles,
          bundle: null,
          conflict: null,
          busy: false,
          error: null,
          result: null,
        });
      } catch (e) {
        setLocalActionError(toAssetError(e));
      } finally {
        setActionBusy(null);
      }
    },
    [findRemoteMatches, t]
  );

  /** 上传确认：pack（最新内容）→ POST 新建 / PUT 更新，409 单独处理。 */
  const confirmUpload = useCallback(
    async (state: UploadDialogState) => {
      if (state.busy) return;
      setUploadDialog({ ...state, busy: true, error: null, conflict: null });
      let bundle = state.bundle;
      try {
        if (!bundle) {
          const root =
            state.asset.scope === "project"
              ? projectRootRef.current.trim()
              : null;
          bundle = await packAgentAsset(
            state.asset.tool,
            state.asset.kind,
            state.asset.scope,
            state.asset.native_id,
            root,
            state.sourceOverride
          );
          if (!sameFileList(state.previewFiles, bundle.manifest.files)) {
            setUploadDialog({
              ...state,
              busy: false,
              bundle: null,
              error: {
                code: "VALIDATION_FAILED",
                message: t("dashboard.agentAssets.plan.contentChanged"),
              },
              conflict: null,
            });
            return;
          }
        }
        let result: RemoteAssetResult;
        if (
          state.targetAssetId != null &&
          state.expectedVersion != null
        ) {
          result = await updateRemoteAgentAsset(
            state.targetAssetId,
            state.expectedVersion,
            bundle
          );
        } else {
          result = await uploadAgentAsset(bundle, state.requestId);
        }
        setUploadDialog({
          ...state,
          busy: false,
          bundle,
          error: null,
          conflict: null,
          result,
        });
        // 上传成功刷新远端列表（若当前页有对应数据）
        void loadRemoteRef.current?.();
      } catch (e) {
        const err = toAssetError(e);
        if (err.code === "VERSION_CONFLICT") {
          const current = numberDetail(err, "current_version");
          setUploadDialog({
            ...state,
            busy: false,
            bundle,
            error: null,
            conflict: {
              expected: state.expectedVersion ?? 0,
              current: current ?? 0,
            },
          });
        } else {
          setUploadDialog({
            ...state,
            busy: false,
            bundle,
            error: err,
            conflict: null,
          });
        }
      }
    },
    [t]
  );

  /** 409 后重新预览：重新 plan_export（必要时重新查同名资源），清掉旧 bundle。 */
  const rePreviewUpload = useCallback(
    async (state: UploadDialogState) => {
      const conflict = state.conflict;
      setUploadDialog({
        ...state,
        busy: true,
        conflict: null,
        error: null,
        result: null,
        bundle: null,
      });
      try {
        const root =
          state.asset.scope === "project"
            ? projectRootRef.current.trim()
            : null;
        const plan = await planAgentAssetExport(
          state.asset.tool,
          state.asset.kind,
          state.asset.scope,
          state.asset.native_id,
          root,
          state.sourceOverride
        );
        const previewBundle = await packAgentAsset(
          state.asset.tool,
          state.asset.kind,
          state.asset.scope,
          state.asset.native_id,
          root,
          state.sourceOverride
        );
        let matches = state.matches;
        let matchError: string | null = null;
        try {
          matches = await findRemoteMatches(state.asset);
        } catch (e) {
          matchError = errorText(t, toAssetError(e));
        }
        const target = state.targetAssetId
          ? matches.find((m) => m.asset_id === state.targetAssetId) ?? null
          : null;
        // 目标资源仍在：用服务端最新版本（与冲突时观察到的 current 取较大值，
        // 列表可能因刚失败的查询而偏旧）；已被删除：退回新建资源
        const nextTargetId = target?.asset_id ?? null;
        const nextExpected = nextTargetId
          ? Math.max(target?.latest_version ?? 0, conflict?.current ?? 0) || null
          : null;
        setUploadDialog({
          ...state,
          plan,
          matches,
          matchError,
          targetAssetId: nextTargetId,
          expectedVersion: nextExpected,
          requestId: nextTargetId == null ? newRequestId() : state.requestId,
          previewFiles: previewBundle.manifest.files.map((file) => ({
            path: file.path,
            size: file.size,
          })),
          bundle: null,
          conflict: null,
          busy: false,
          error: null,
          result: null,
        });
      } catch (e) {
        setUploadDialog({
          ...state,
          busy: false,
          conflict: null,
          error: toAssetError(e),
          result: null,
          bundle: null,
        });
      }
    },
    [findRemoteMatches, t]
  );

  /** 409 后另存为新资源：改走 POST + 新请求 ID，并重新预览最新文件。 */
  const saveUploadAsNew = useCallback(async (state: UploadDialogState) => {
    setUploadDialog({
      ...state,
      busy: true,
      conflict: null,
      error: null,
      result: null,
      bundle: null,
    });
    try {
      const root =
        state.asset.scope === "project"
          ? projectRootRef.current.trim()
          : null;
      const plan = await planAgentAssetExport(
        state.asset.tool,
        state.asset.kind,
        state.asset.scope,
        state.asset.native_id,
        root,
        state.sourceOverride
      );
      const previewBundle = await packAgentAsset(
        state.asset.tool,
        state.asset.kind,
        state.asset.scope,
        state.asset.native_id,
        root,
        state.sourceOverride
      );
      setUploadDialog({
        ...state,
        plan,
        targetAssetId: null,
        expectedVersion: null,
        requestId: newRequestId(),
        previewFiles: previewBundle.manifest.files.map((file) => ({
          path: file.path,
          size: file.size,
        })),
        bundle: null,
        conflict: null,
        error: null,
        result: null,
        busy: false,
      });
    } catch (e) {
      setUploadDialog({
        ...state,
        busy: false,
        conflict: null,
        error: toAssetError(e),
        result: null,
        bundle: null,
      });
    }
  }, []);

  /** 切换上传目标（新建 ↔ 更新某远端资源）。 */
  const chooseUploadTarget = useCallback(
    (state: UploadDialogState, target: RemoteAsset | null) => {
      setUploadDialog({
        ...state,
        targetAssetId: target?.asset_id ?? null,
        expectedVersion: target?.latest_version ?? null,
        conflict: null,
        error: null,
        result: null,
      });
    },
    []
  );

  /** 导出到本机目录：pack（最新内容）→ export_to_disk。 */
  const confirmExport = useCallback(
    async (state: ExportDialogState) => {
      const destDir = state.destDir.trim();
      if (!destDir) {
        setExportDialog({
          ...state,
          error: {
            code: "VALIDATION_FAILED",
            message: t("dashboard.agentAssets.plan.destDirRequired"),
          },
        });
        return;
      }
      setExportDialog({ ...state, busy: true, error: null });
      try {
        const root =
          state.asset.scope === "project"
            ? projectRootRef.current.trim()
            : null;
        const bundle = await packAgentAsset(
          state.asset.tool,
          state.asset.kind,
          state.asset.scope,
          state.asset.native_id,
          root,
          state.sourceOverride
        );
        const result = await exportAgentAssetBundleToDisk(
          bundle,
          destDir,
          state.overwrite
        );
        lastDestDirRef.current = destDir;
        setExportDialog({
          ...state,
          destDir,
          busy: false,
          error: null,
          result,
        });
      } catch (e) {
        setExportDialog({
          ...state,
          busy: false,
          error: toAssetError(e),
        });
      }
    },
    [t]
  );

  // ===== 远端资源库 =====

  const loadRemote = useCallback(async () => {
    if (!serverRef.current?.supported) {
      // 作废在途请求：服务端刚变为不可用时，旧响应不能再回填列表
      remoteReq.current += 1;
      setRemotePageData(null);
      setRemoteError(null);
      setRemoteLoading(false);
      return;
    }
    const reqId = ++remoteReq.current;
    setRemoteLoading(true);
    setRemoteError(null);
    try {
      const data = await listRemoteAgentAssets({
        tool: remoteFilterTool || undefined,
        kind: remoteFilterKind || undefined,
        scope: remoteFilterScope || undefined,
        q: remoteQuery.trim() || undefined,
        page: remotePage,
        pageSize: REMOTE_PAGE_SIZE,
      });
      if (reqId !== remoteReq.current) return;
      // 远端资源被删/筛选变化导致当前页越界：回到第一页重新取数
      if (data.items.length === 0 && data.total > 0 && remotePage > 1) {
        setRemotePage(1);
        return;
      }
      setRemotePageData(data);
      // 列表刷新后收起版本历史，避免展示已不在当前页的资源
      setHistoryAssetId(null);
      setRevisions(null);
      setHistoryError(null);
    } catch (e) {
      if (reqId !== remoteReq.current) return;
      setRemotePageData(null);
      setRemoteError(toAssetError(e));
    } finally {
      if (reqId === remoteReq.current) setRemoteLoading(false);
    }
  }, [
    remoteFilterTool,
    remoteFilterKind,
    remoteFilterScope,
    remoteQuery,
    remotePage,
  ]);
  // confirmUpload 在 useCallback 内需要最新 loadRemote，用 ref 避免依赖成环
  loadRemoteRef.current = () => {
    void loadRemote();
  };

  // 服务端就绪 / 筛选 / 翻页变化时拉列表
  useEffect(() => {
    void loadRemote();
  }, [loadRemote, server?.supported]);

  const toggleHistory = useCallback(
    async (asset: RemoteAsset) => {
      if (historyAssetId === asset.asset_id) {
        setHistoryAssetId(null);
        setRevisions(null);
        setHistoryError(null);
        return;
      }
      setHistoryAssetId(asset.asset_id);
      setRevisions(null);
      setHistoryError(null);
      const reqId = ++historyReq.current;
      setHistoryLoading(true);
      try {
        const list = await listAgentAssetRevisions(asset.asset_id);
        if (reqId !== historyReq.current) return;
        setRevisions(list);
      } catch (e) {
        if (reqId !== historyReq.current) return;
        setHistoryError(toAssetError(e));
      } finally {
        if (reqId === historyReq.current) setHistoryLoading(false);
      }
    },
    [historyAssetId]
  );

  /** 远端行可用性：下载看 native_exportable；安装看目标作用域 installable。
   *
   * 能力条目同时按 `native_format` 过滤：同一 工具×类型 下可能有
   * `markdown-frontmatter` 与 `yaml-legacy` / `jsonc-inline` 多条，后者
   * installable=false，不能因为同工具同 kind 的 Markdown 条目可安装就误放行。
   */
  const capsForRemote = useCallback(
    (asset: RemoteAsset): CapabilityEntry[] =>
      capabilities?.filter(
        (c) =>
          c.tool === asset.tool &&
          c.kind === asset.kind &&
          c.native_format === asset.native_format
      ) ?? [],
    [capabilities]
  );
  const isRemoteExportable = useCallback(
    (asset: RemoteAsset): boolean => {
      if (!capabilities) return true; // 能力未加载：先允许，命令失败会报错
      const caps = capsForRemote(asset);
      if (caps.length === 0) return true; // 未知 tool/kind/format 容忍（契约 §2.3）
      return caps.some((c) => c.status === "verified" && c.native_exportable);
    },
    [capabilities, capsForRemote]
  );
  /** 内联/旧 YAML 只能下载后手动合并/加载，前端直接置灰（不等后端拒绝）。 */
  const installBlockedByFormat = useCallback(
    (asset: RemoteAsset): boolean =>
      asset.native_format === "jsonc-inline" ||
      asset.native_format === "yaml-legacy",
    []
  );
  const installableScopes = useCallback(
    (asset: RemoteAsset): AssetScope[] => {
      if (!capabilities || installBlockedByFormat(asset)) return [];
      return capsForRemote(asset)
        .filter((c) => c.status === "verified" && c.installable)
        .map((c) => c.scope);
    },
    [capabilities, capsForRemote, installBlockedByFormat]
  );
  /** 该工具未在本机实测（verified_versions 全空）：安装弹层展示知情提示。 */
  const toolUnverified = useCallback(
    (asset: RemoteAsset): boolean => {
      const caps = capsForRemote(asset);
      return caps.length > 0 && caps.every((c) => c.verified_versions.length === 0);
    },
    [capsForRemote]
  );

  /** 打开远端下载/安装弹层并取回资源包（指纹与文件清单展示的数据源）。 */
  const openRemoteDialog = useCallback(
    async (
      asset: RemoteAsset,
      version: number,
      mode: "download" | "install"
    ) => {
      const seq = ++remoteDialogSeq.current;
      const scopes = installableScopes(asset);
      const originScope: AssetScope =
        asset.origin_scope === "project" ? "project" : "user";
      const defaultScope = scopes.includes(originScope)
        ? originScope
        : scopes[0] ?? "user";
      setDirPickerHint(null);
      setRemoteDialog({
        asset,
        version,
        mode,
        bundle: null,
        fetching: true,
        fetchError: null,
        destDir: lastDestDirRef.current,
        targetScope: defaultScope,
        projectRoot: scope === "project" ? projectRoot : "",
        overwrite: false,
        busy: false,
        error: null,
        result: null,
      });
      try {
        const bundle = await fetchAgentAssetBundle(asset.asset_id, version);
        if (seq !== remoteDialogSeq.current) return;
        setRemoteDialog((current) =>
          current &&
          current.asset.asset_id === asset.asset_id &&
          current.version === version
            ? { ...current, bundle, fetching: false }
            : current
        );
      } catch (e) {
        if (seq !== remoteDialogSeq.current) return;
        const err = toAssetError(e);
        setRemoteDialog((current) =>
          current &&
          current.asset.asset_id === asset.asset_id &&
          current.version === version
            ? { ...current, fetching: false, fetchError: err }
            : current
        );
      }
    },
    [installableScopes, projectRoot, scope]
  );

  /** 重取资源包（下载/安装弹层内的失败重试）。 */
  const retryRemoteFetch = useCallback(async (state: RemoteDialogState) => {
    const seq = ++remoteDialogSeq.current;
    setRemoteDialog({ ...state, fetching: true, fetchError: null });
    try {
      const bundle = await fetchAgentAssetBundle(
        state.asset.asset_id,
        state.version
      );
      if (seq !== remoteDialogSeq.current) return;
      setRemoteDialog((current) =>
        current ? { ...current, bundle, fetching: false } : current
      );
    } catch (e) {
      if (seq !== remoteDialogSeq.current) return;
      const err = toAssetError(e);
      setRemoteDialog((current) =>
        current ? { ...current, fetching: false, fetchError: err } : current
      );
    }
  }, []);

  /** 下载 = 取回的信封原样导出到目标目录（不执行内容）。 */
  const confirmRemoteDownload = useCallback(
    async (state: RemoteDialogState) => {
      if (!state.bundle) return;
      const destDir = state.destDir.trim();
      if (!destDir) {
        setRemoteDialog({
          ...state,
          error: {
            code: "VALIDATION_FAILED",
            message: t("dashboard.agentAssets.plan.destDirRequired"),
          },
        });
        return;
      }
      setRemoteDialog({ ...state, busy: true, error: null });
      try {
        const result = await exportAgentAssetBundleToDisk(
          state.bundle,
          destDir,
          state.overwrite
        );
        lastDestDirRef.current = destDir;
        setRemoteDialog({
          ...state,
          destDir,
          busy: false,
          error: null,
          result,
        });
      } catch (e) {
        setRemoteDialog({ ...state, busy: false, error: toAssetError(e) });
      }
    },
    [t]
  );

  /** 安装 = 目标作用域校验 + install（默认跳过冲突，覆盖前备份）。 */
  const confirmRemoteInstall = useCallback(
    async (state: RemoteDialogState) => {
      if (!state.bundle) return;
      const root =
        state.targetScope === "project" ? state.projectRoot.trim() : null;
      if (state.targetScope === "project" && !root) {
        setRemoteDialog({
          ...state,
          error: {
            code: "VALIDATION_FAILED",
            message: t("dashboard.agentAssets.transfer.projectRootRequired"),
          },
        });
        return;
      }
      setRemoteDialog({ ...state, busy: true, error: null });
      try {
        const result = await installAgentAssetBundle(
          state.bundle,
          state.targetScope,
          root,
          state.overwrite
        );
        setRemoteDialog({ ...state, busy: false, error: null, result });
      } catch (e) {
        setRemoteDialog({ ...state, busy: false, error: toAssetError(e) });
      }
    },
    [t]
  );

  /** 目录选择（plugin-dialog；dashboard 窗口未授权时提示手填绝对路径）。 */
  const pickDirectory = useCallback(
    async (onPicked: (dir: string) => void) => {
      setDirPickerHint(null);
      try {
        const picked = await openNativeDialog({
          directory: true,
          multiple: false,
        });
        if (typeof picked === "string" && picked) onPicked(picked);
      } catch {
        setDirPickerHint(
          t("dashboard.agentAssets.select.chooseDirFailed")
        );
      }
    },
    [t]
  );
  /** 页头刷新：服务端状态 + 能力矩阵 + 本机资源 + 远端列表。 */
  const refreshAll = useCallback(async () => {
    setRefreshing(true);
    try {
      await Promise.all([loadServerStatus(), loadCapabilities(), loadLocal()]);
      void loadRemote();
    } finally {
      setRefreshing(false);
    }
  }, [loadServerStatus, loadCapabilities, loadLocal, loadRemote]);

  // ===== 渲染 =====

  const remoteTotalPages =
    remotePageData && remotePageData.page_size > 0
      ? Math.max(1, Math.ceil(remotePageData.total / remotePageData.page_size))
      : 1;

  return (
    <div className="db-page">
      <header className="db-page__header">
        <div>
          <h1 className="db-page__title">{t("dashboard.agentAssets")}</h1>
          <p className="db-page__desc">{t("dashboard.agentAssets.desc")}</p>
        </div>
        <div className="db-page__actions">
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

      {/* ---------- ① 服务端状态 ---------- */}
      <ServerStatusCard
        server={server}
        busy={serverBusy}
        error={serverError}
        onRetry={() => void loadServerStatus()}
      />

      {capabilitiesError && (
        <AlertBanner type="error">
          {capabilitiesError}
          <button
            type="button"
            className="db-linkbtn ml-2"
            onClick={() => void loadCapabilities()}
          >
            {t("dashboard.agentAssets.error.retry")}
          </button>
        </AlertBanner>
      )}

      {/* ---------- ② 工具与作用域 ---------- */}
      <Card
        title={t("dashboard.agentAssets.select.title")}
        sub={
          entry
            ? `${entry.display_name} · ${kindLabel(t, entry.kind)} · ${scopeLabel(
                t,
                entry.scope
              )}`
            : undefined
        }
        action={
          <button
            type="button"
            className="db-btn db-btn--sm db-btn--primary"
            onClick={handleScan}
            disabled={!canScan || localLoading}
          >
            {localLoading
              ? t("dashboard.agentAssets.select.scanning")
              : t("dashboard.agentAssets.select.scan")}
          </button>
        }
      >
        <div className="flex flex-wrap items-end gap-3">
          <label className="flex flex-col gap-1">
            <span style={dimText}>{t("dashboard.agentAssets.select.tool")}</span>
            <select
              className={selectClass}
              value={tool}
              onChange={(event) => setTool(event.target.value)}
            >
              {TOOL_ORDER.map((item) => {
                const pending = isToolPending(capabilities, item.id);
                return (
                  <option key={item.id} value={item.id} disabled={pending}>
                    {item.name}
                    {pending
                      ? ` (${t("dashboard.agentAssets.select.pending")})`
                      : ""}
                  </option>
                );
              })}
            </select>
          </label>

          <div className="flex flex-col gap-1">
            <span style={dimText}>
              {t("dashboard.agentAssets.select.kind")}
            </span>
            <div
              className="db-seg db-seg--sm"
              role="group"
              aria-label={t("dashboard.agentAssets.select.kind")}
            >
              {KIND_ORDER.map((value) => (
                <button
                  key={value}
                  type="button"
                  className={`db-seg__btn ${
                    kind === value ? "db-seg__btn--active" : ""
                  }`}
                  onClick={() => setKind(value)}
                  aria-pressed={kind === value}
                >
                  {kindLabel(t, value)}
                </button>
              ))}
            </div>
          </div>

          <div className="flex flex-col gap-1">
            <span style={dimText}>
              {t("dashboard.agentAssets.select.scope")}
            </span>
            <div
              className="db-seg db-seg--sm"
              role="group"
              aria-label={t("dashboard.agentAssets.select.scope")}
            >
              {SCOPE_ORDER.map((value) => (
                <button
                  key={value}
                  type="button"
                  className={`db-seg__btn ${
                    scope === value ? "db-seg__btn--active" : ""
                  }`}
                  onClick={() => setScope(value)}
                  aria-pressed={scope === value}
                >
                  {scopeLabel(t, value)}
                </button>
              ))}
            </div>
          </div>

          {scope === "project" && (
            <label className="flex min-w-[240px] flex-1 flex-col gap-1">
              <span style={dimText}>
                {t("dashboard.agentAssets.select.projectRoot")}
              </span>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  className={`${inputClass} flex-1`}
                  value={projectRoot}
                  list={PROJECT_DATALIST_ID}
                  placeholder={t(
                    "dashboard.agentAssets.select.projectRootPlaceholder"
                  )}
                  onChange={(event) => setProjectRoot(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") handleScan();
                  }}
                />
                <button
                  type="button"
                  className="db-btn db-btn--sm"
                  onClick={() =>
                    void pickDirectory((dir) => setProjectRoot(dir))
                  }
                >
                  {t("dashboard.agentAssets.select.chooseDir")}
                </button>
              </div>
            </label>
          )}
        </div>

        {/* 项目根候选（datalist 常驻：安装弹层选项目级时同样可用） */}
        <datalist id={PROJECT_DATALIST_ID}>
          {(knownProjects ?? []).map((path) => (
            <option key={path} value={path} />
          ))}
        </datalist>

        {/* 能力四档（按当前 工具 × 类型 × 作用域） */}
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <span style={dimText}>
            {t("dashboard.agentAssets.select.capabilities")}:
          </span>
          {entry ? (
            <>
              <CapChip
                on={entry.scannable}
                label={t("dashboard.agentAssets.select.capScannable")}
              />
              <CapChip
                on={entry.archivable}
                label={t("dashboard.agentAssets.select.capArchivable")}
              />
              <CapChip
                on={entry.native_exportable}
                label={t("dashboard.agentAssets.select.capNativeExportable")}
              />
              <CapChip
                on={entry.installable}
                label={t("dashboard.agentAssets.select.capInstallable")}
              />
              {entry.status !== "verified" && (
                <span className="rounded-full bg-amber-500/15 px-2 py-0.5 text-[10px] font-medium text-amber-700">
                  {t("dashboard.agentAssets.select.pending")}
                </span>
              )}
            </>
          ) : (
            <span className="text-[11px]" style={dimText}>
              {capabilities
                ? t("dashboard.agentAssets.select.unsupported")
                : t("common.loading")}
            </span>
          )}
        </div>
        {entry?.notes && (
          <p className="mt-2 text-[11px] leading-relaxed" style={dimText}>
            {t("dashboard.agentAssets.select.notes")}: {entry.notes}
          </p>
        )}
        {scanBlockedReason && capabilities && (
          <p className="mt-1 text-[11px]" style={dimText}>
            {scanBlockedReason}
          </p>
        )}
        {dirPickerHint && (
          <p className="mt-1 text-[11px]" style={dimText}>
            {dirPickerHint}
          </p>
        )}
      </Card>

      {/* ---------- ③ 本机资源 ---------- */}
      <Card
        title={t("dashboard.agentAssets.local.title")}
        sub={
          assets != null
            ? t("dashboard.agentAssets.local.sub", { n: assets.length })
            : undefined
        }
        action={
          <button
            type="button"
            className="db-btn db-btn--sm"
            onClick={() => void handleImportPick()}
            disabled={!importCanArchive || importBusy || actionBusy != null}
            title={importBlockedReason ?? undefined}
          >
            {importBusy
              ? t("dashboard.agentAssets.import.busy")
              : t("dashboard.agentAssets.import.action")}
          </button>
        }
      >
        {importOpen && (
          <ImportResultsPanel
            candidates={importCandidates}
            busy={importBusy}
            error={importError}
            canArchive={importCanArchive}
            serverSupported={serverSupported}
            uploadBlockedReason={uploadBlockedReason}
            actionBusy={actionBusy}
            onRepick={() => void handleImportPick()}
            onClear={clearImport}
            onAction={(ref, action) => void openPlanFlow(ref, action)}
          />
        )}
        {localActionError && (
          <AlertBanner type="error">
            {errorText(t, localActionError)}
          </AlertBanner>
        )}
        {localError && (
          <AlertBanner
            type={localError.code === "ADAPTER_NOT_FOUND" ? "warning" : "error"}
          >
            {localError.code === "ADAPTER_NOT_FOUND"
              ? t("dashboard.agentAssets.local.adapterMissing")
              : t("dashboard.agentAssets.local.error", {
                  error: errorText(t, localError),
                })}
            <button
              type="button"
              className="db-linkbtn ml-2"
              onClick={handleScan}
            >
              {t("dashboard.agentAssets.error.retry")}
            </button>
          </AlertBanner>
        )}

        {localLoading || capabilities == null ? (
          <Skeleton className="h-24 w-full" />
        ) : assets == null ? (
          <p className="db-empty">
            {scanBlockedReason ?? t("dashboard.agentAssets.local.empty")}
          </p>
        ) : assets.length === 0 ? (
          <div className="db-empty">
            <div>{t("dashboard.agentAssets.local.empty")}</div>
            <div className="mt-1 text-[11px]">
              {t("dashboard.agentAssets.local.emptyHint")}
            </div>
          </div>
        ) : (
          <div>
            {assets.map((asset) => {
              const key = assetKey(asset);
              const busy = actionBusy?.key === key;
              const anyBusy = actionBusy != null;
              const open = expandedKey === key;
              return (
                <div
                  key={key}
                  className="border-b py-3 last:border-b-0"
                  style={{ borderColor: "var(--db-row-border)" }}
                >
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span
                          className="text-[13px] font-semibold"
                          style={{ color: "var(--db-text)" }}
                        >
                          {asset.display_name}
                        </span>
                        <span className="num text-[11px]" style={dimText}>
                          {asset.native_id}
                        </span>
                        <span className="db-tag db-tag--configured">
                          {asset.native_format}
                        </span>
                        {asset.warnings.length > 0 && (
                          <span
                            className="rounded-full bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium text-amber-700"
                            title={asset.warnings.join("\n")}
                          >
                            ⚠ {t("dashboard.agentAssets.local.warnings", {
                              n: asset.warnings.length,
                            })}
                          </span>
                        )}
                      </div>
                      {asset.description && (
                        <p
                          className="mt-1 break-words text-[11px] leading-relaxed"
                          style={dimText}
                        >
                          {asset.description}
                        </p>
                      )}
                      <p
                        className="num mt-1 truncate text-[10px]"
                        style={dimText}
                        title={asset.source_root}
                      >
                        {t("dashboard.agentAssets.local.sourceRoot")}:{" "}
                        {asset.source_root}
                      </p>
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                      <button
                        type="button"
                        className="db-linkbtn"
                        onClick={() => void toggleInspect(asset)}
                      >
                        {open
                          ? t("dashboard.agentAssets.local.collapse")
                          : t("dashboard.agentAssets.local.expand")}
                      </button>
                      <button
                        type="button"
                        className="db-btn db-btn--sm"
                        disabled={!canArchive || anyBusy}
                        onClick={() =>
                          void openPlanFlow(
                            { asset, sourceOverride: null, key },
                            "plan"
                          )
                        }
                      >
                        {busy && actionBusy?.action === "plan"
                          ? t("dashboard.agentAssets.local.actionPlanBusy")
                          : t("dashboard.agentAssets.local.actionPlan")}
                      </button>
                      <button
                        type="button"
                        className="db-btn db-btn--sm db-btn--primary"
                        disabled={!canArchive || anyBusy || !serverSupported}
                        title={uploadBlockedReason ?? undefined}
                        onClick={() =>
                          void openPlanFlow(
                            { asset, sourceOverride: null, key },
                            "upload"
                          )
                        }
                      >
                        {busy && actionBusy?.action === "upload"
                          ? t("dashboard.agentAssets.local.actionUploadBusy")
                          : t("dashboard.agentAssets.local.actionUpload")}
                      </button>
                      <button
                        type="button"
                        className="db-btn db-btn--sm"
                        disabled={!canArchive || anyBusy}
                        onClick={() =>
                          void openPlanFlow(
                            { asset, sourceOverride: null, key },
                            "export"
                          )
                        }
                      >
                        {busy && actionBusy?.action === "export"
                          ? t("dashboard.agentAssets.local.actionExportBusy")
                          : t("dashboard.agentAssets.local.actionExport")}
                      </button>
                    </div>
                  </div>

                  {open && (
                    <div
                      className="mt-3 rounded-xl border p-3"
                      style={{
                        borderColor: "var(--db-border)",
                        background: "var(--db-hover)",
                      }}
                    >
                      <InspectorContent
                        report={inspect?.key === key ? inspect.report : null}
                        loading={inspectLoading}
                        error={inspectError}
                      />
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </Card>

      {/* ---------- ④ 远端资源库 ---------- */}
      <Card
        title={t("dashboard.agentAssets.remote.title")}
        sub={
          remotePageData
            ? t("dashboard.agentAssets.remote.sub", {
                n: remotePageData.total,
              })
            : undefined
        }
        action={
          <button
            type="button"
            className="db-btn db-btn--sm"
            onClick={() => void loadRemote()}
            disabled={!serverSupported || remoteLoading}
          >
            <RefreshIcon
              className={`h-3 w-3 ${remoteLoading ? "db-spin" : ""}`}
            />
            {t("dashboard.agentAssets.remote.refresh")}
          </button>
        }
      >
        {!serverSupported ? (
          <p className="db-empty">
            {server
              ? server.reason === "OLD_SERVER_UNSUPPORTED"
                ? t("dashboard.agentAssets.error.oldServer")
                : t("dashboard.agentAssets.remote.disabled")
              : t("dashboard.agentAssets.server.checking")}
          </p>
        ) : (
          <>
            <p className="mb-2 text-[11px] leading-relaxed" style={dimText}>
              {t("dashboard.agentAssets.remote.updateHint")}
            </p>
            {/* 筛选行：tool / kind / scope / q */}
            <div className="mb-3 flex flex-wrap items-end gap-2">
              <select
                className={selectClass}
                value={remoteFilterTool}
                aria-label={t("dashboard.agentAssets.select.tool")}
                onChange={(event) => {
                  setRemoteFilterTool(event.target.value);
                  setRemotePage(1);
                }}
              >
                <option value="">
                  {t("dashboard.agentAssets.remote.allTools")}
                </option>
                {TOOL_ORDER.map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.name}
                  </option>
                ))}
              </select>
              <select
                className={selectClass}
                value={remoteFilterKind}
                aria-label={t("dashboard.agentAssets.select.kind")}
                onChange={(event) => {
                  setRemoteFilterKind(event.target.value as "" | AssetKind);
                  setRemotePage(1);
                }}
              >
                <option value="">
                  {t("dashboard.agentAssets.remote.allKinds")}
                </option>
                {KIND_ORDER.map((value) => (
                  <option key={value} value={value}>
                    {kindLabel(t, value)}
                  </option>
                ))}
              </select>
              <select
                className={selectClass}
                value={remoteFilterScope}
                aria-label={t("dashboard.agentAssets.select.scope")}
                onChange={(event) => {
                  setRemoteFilterScope(event.target.value as "" | AssetScope);
                  setRemotePage(1);
                }}
              >
                <option value="">
                  {t("dashboard.agentAssets.remote.allScopes")}
                </option>
                {SCOPE_ORDER.map((value) => (
                  <option key={value} value={value}>
                    {scopeLabel(t, value)}
                  </option>
                ))}
              </select>
              <input
                type="search"
                className={`${inputClass} min-w-[180px] flex-1`}
                value={remoteQueryInput}
                placeholder={t(
                  "dashboard.agentAssets.remote.searchPlaceholder"
                )}
                onChange={(event) => setRemoteQueryInput(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    setRemoteQuery(remoteQueryInput);
                    setRemotePage(1);
                  }
                }}
              />
              <button
                type="button"
                className="db-btn db-btn--sm"
                onClick={() => {
                  setRemoteQuery(remoteQueryInput);
                  setRemotePage(1);
                }}
              >
                {t("dashboard.agentAssets.remote.refresh")}
              </button>
            </div>

            {remoteError && (
              <AlertBanner type="error">
                {t("dashboard.agentAssets.remote.error", {
                  error: errorText(t, remoteError),
                })}
                <button
                  type="button"
                  className="db-linkbtn ml-2"
                  onClick={() => void loadRemote()}
                >
                  {t("dashboard.agentAssets.error.retry")}
                </button>
              </AlertBanner>
            )}

            {remoteLoading && remotePageData == null ? (
              <Skeleton className="h-32 w-full" />
            ) : remotePageData == null ? (
              <p className="db-empty">
                {remoteError
                  ? t("dashboard.agentAssets.remote.empty")
                  : t("dashboard.agentAssets.remote.loading")}
              </p>
            ) : remotePageData.items.length === 0 ? (
              <p className="db-empty">
                {t("dashboard.agentAssets.remote.empty")}
              </p>
            ) : (
              <div>
                {remotePageData.items.map((asset) => {
                  const caps = capsForRemote(asset);
                  const exportable = isRemoteExportable(asset);
                  const formatBlocked = installBlockedByFormat(asset);
                  const installScopes = installableScopes(asset);
                  const installDisabled = formatBlocked || installScopes.length === 0;
                  const historyOpen = historyAssetId === asset.asset_id;
                  return (
                    <div
                      key={asset.asset_id}
                      className="border-b py-3 last:border-b-0"
                      style={{ borderColor: "var(--db-row-border)" }}
                    >
                      <div className="flex flex-wrap items-start justify-between gap-3">
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <span
                              className="text-[13px] font-semibold"
                              style={{ color: "var(--db-text)" }}
                            >
                              {asset.display_name}
                            </span>
                            <span className="num text-[11px]" style={dimText}>
                              {asset.native_id}
                            </span>
                            <span className="db-tag db-tag--manual">
                              {toolDisplayName(asset.tool)}
                            </span>
                            <span className="db-tag db-tag--configured">
                              {remoteKindLabel(t, asset.kind)}
                            </span>
                            <span className="db-tag db-tag--configured">
                              {remoteScopeLabel(t, asset.origin_scope)}
                            </span>
                            <span className="db-tag db-tag--new num">
                              {t("dashboard.agentAssets.remote.version", {
                                version: asset.latest_version,
                              })}
                            </span>
                            {caps.length === 0 && capabilities && (
                              <span className="db-tag db-tag--unpriced">
                                {t("dashboard.agentAssets.select.pending")}
                              </span>
                            )}
                          </div>
                          {asset.description && (
                            <p
                              className="mt-1 break-words text-[11px] leading-relaxed"
                              style={dimText}
                            >
                              {asset.description}
                            </p>
                          )}
                          <p
                            className="num mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px]"
                            style={dimText}
                          >
                            <span>
                              {t("dashboard.agentAssets.remote.entries", {
                                n: asset.entry_count,
                              })}
                            </span>
                            <span>{formatBytes(asset.total_size)}</span>
                            <span>
                              {t("dashboard.agentAssets.remote.updated", {
                                time: formatDateTime(asset.updated_at),
                              })}
                            </span>
                            <span className="truncate" title={asset.asset_id}>
                              {asset.asset_id}
                            </span>
                          </p>
                        </div>
                        <div className="flex flex-wrap items-center gap-2">
                          <button
                            type="button"
                            className="db-linkbtn"
                            onClick={() => void toggleHistory(asset)}
                          >
                            {historyOpen ? "▾" : "▸"}{" "}
                            {t("dashboard.agentAssets.remote.history")}
                          </button>
                          <button
                            type="button"
                            className="db-btn db-btn--sm"
                            disabled={!exportable}
                            title={
                              exportable
                                ? undefined
                                : t(
                                    "dashboard.agentAssets.remote.exportDisabled"
                                  )
                            }
                            onClick={() =>
                              void openRemoteDialog(
                                asset,
                                asset.latest_version,
                                "download"
                              )
                            }
                          >
                            {t("dashboard.agentAssets.remote.download")}
                          </button>
                          <button
                            type="button"
                            className="db-btn db-btn--sm"
                            disabled={installDisabled}
                            title={
                              !installDisabled
                                ? undefined
                                : formatBlocked
                                  ? t(
                                      "dashboard.agentAssets.remote.installFormatDisabled"
                                    )
                                  : t(
                                      "dashboard.agentAssets.remote.installDisabled"
                                    )
                            }
                            onClick={() =>
                              void openRemoteDialog(
                                asset,
                                asset.latest_version,
                                "install"
                              )
                            }
                          >
                            {t("dashboard.agentAssets.remote.install")}
                          </button>
                          {capabilities != null && installDisabled && (
                            <span
                              className="rounded-full bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium text-amber-700"
                              title={
                                formatBlocked
                                  ? t(
                                      "dashboard.agentAssets.remote.installFormatDisabled"
                                    )
                                  : t(
                                      "dashboard.agentAssets.remote.installDisabled"
                                    )
                              }
                            >
                              {t("dashboard.agentAssets.remote.installBadge")}
                            </span>
                          )}
                        </div>
                      </div>

                      {historyOpen && (
                        <div
                          className="mt-3 rounded-xl border p-2"
                          style={{
                            borderColor: "var(--db-border)",
                            background: "var(--db-hover)",
                          }}
                        >
                          {historyLoading ? (
                            <p className="db-empty">
                              {t(
                                "dashboard.agentAssets.remote.historyLoading"
                              )}
                            </p>
                          ) : historyError ? (
                            <AlertBanner type="error">
                              {t(
                                "dashboard.agentAssets.remote.historyError",
                                { error: errorText(t, historyError) }
                              )}
                            </AlertBanner>
                          ) : revisions && revisions.length > 0 ? (
                            <div className="db-table-wrap">
                              <table className="db-table">
                                <thead>
                                  <tr>
                                    <th>
                                      {t(
                                        "dashboard.agentAssets.remote.colVersion"
                                      )}
                                    </th>
                                    <th>
                                      {t(
                                        "dashboard.agentAssets.remote.colSha"
                                      )}
                                    </th>
                                    <th>
                                      {t(
                                        "dashboard.agentAssets.remote.colFiles"
                                      )}
                                    </th>
                                    <th>
                                      {t(
                                        "dashboard.agentAssets.remote.colCreated"
                                      )}
                                    </th>
                                    <th>
                                      {t(
                                        "dashboard.agentAssets.remote.colDevice"
                                      )}
                                    </th>
                                    <th />
                                  </tr>
                                </thead>
                                <tbody>
                                  {revisions.map((revision) => (
                                    <tr key={revision.version}>
                                      <td className="num">
                                        {t(
                                          "dashboard.agentAssets.remote.version",
                                          { version: revision.version }
                                        )}
                                      </td>
                                      <td
                                        className="num"
                                        title={revision.bundle_sha256}
                                      >
                                        {shortHash(revision.bundle_sha256)}
                                      </td>
                                      <td className="num">
                                        {revision.entry_count} ·{" "}
                                        {formatBytes(revision.total_size)}
                                      </td>
                                      <td className="num">
                                        {formatDateTime(revision.created_at)}
                                      </td>
                                      <td
                                        className="num truncate"
                                        title={revision.created_by_device}
                                      >
                                        {revision.created_by_device}
                                      </td>
                                      <td>
                                        <button
                                          type="button"
                                          className="db-linkbtn"
                                          onClick={() =>
                                            void openRemoteDialog(
                                              asset,
                                              revision.version,
                                              "download"
                                            )
                                          }
                                        >
                                          {t(
                                            "dashboard.agentAssets.remote.downloadThis"
                                          )}
                                        </button>
                                      </td>
                                    </tr>
                                  ))}
                                </tbody>
                              </table>
                            </div>
                          ) : (
                            <p className="db-empty">
                              {t("dashboard.agentAssets.remote.historyEmpty")}
                            </p>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })}

                <div className="db-pager">
                  <button
                    type="button"
                    className="db-btn db-btn--sm"
                    disabled={remotePage <= 1 || remoteLoading}
                    onClick={() =>
                      setRemotePage((page) => Math.max(1, page - 1))
                    }
                  >
                    ‹ {t("dashboard.agentAssets.remote.pagePrev")}
                  </button>
                  <span className="db-pager__info num">
                    {t("dashboard.agentAssets.remote.pageInfo", {
                      page: remotePageData.page,
                      total: remoteTotalPages,
                    })}
                  </span>
                  <button
                    type="button"
                    className="db-btn db-btn--sm"
                    disabled={
                      remotePage >= remoteTotalPages || remoteLoading
                    }
                    onClick={() =>
                      setRemotePage((page) =>
                        Math.min(remoteTotalPages, page + 1)
                      )
                    }
                  >
                    {t("dashboard.agentAssets.remote.pageNext")} ›
                  </button>
                  <span className="db-pager__total num">
                    {t("dashboard.agentAssets.remote.pageTotal", {
                      n: remotePageData.total,
                    })}
                  </span>
                </div>
              </div>
            )}
          </>
        )}
      </Card>

      <p className="db-page__footnote">
        {t("dashboard.agentAssets.footnote")}
      </p>

      {/* ---------- 弹层 ---------- */}
      {planDialog && (
        <Modal
          title={t("dashboard.agentAssets.plan.title")}
          onClose={() => setPlanDialog(null)}
          footer={
            <button
              type="button"
              className="db-btn"
              onClick={() => setPlanDialog(null)}
            >
              {t("dashboard.agentAssets.plan.close")}
            </button>
          }
        >
          <PlanDetails
            plan={planDialog.plan}
            subtitle={t("dashboard.agentAssets.plan.subtitle", {
              tool: toolDisplayName(planDialog.plan.tool),
              kind: kindLabel(t, planDialog.plan.kind),
              name: planDialog.plan.display_name,
            })}
          />
        </Modal>
      )}

      {uploadDialog && (
        <UploadDialog
          state={uploadDialog}
          onTarget={(target) => chooseUploadTarget(uploadDialog, target)}
          onConfirm={() => void confirmUpload(uploadDialog)}
          onRePreview={() => void rePreviewUpload(uploadDialog)}
          onSaveAsNew={() => void saveUploadAsNew(uploadDialog)}
          onClose={() => {
            if (!uploadDialog.busy) setUploadDialog(null);
          }}
        />
      )}

      {exportDialog && (
        <ExportDialog
          state={exportDialog}
          dirHint={dirPickerHint}
          onPickDir={() =>
            void pickDirectory((dir) =>
              setExportDialog((current) =>
                current ? { ...current, destDir: dir } : current
              )
            )
          }
          onDestDir={(destDir) =>
            setExportDialog((current) =>
              current ? { ...current, destDir } : current
            )
          }
          onOverwrite={(overwrite) =>
            setExportDialog((current) =>
              current ? { ...current, overwrite } : current
            )
          }
          onConfirm={() => void confirmExport(exportDialog)}
          onClose={() => {
            if (!exportDialog.busy) setExportDialog(null);
          }}
        />
      )}

      {remoteDialog && (
        <RemoteTransferDialog
          state={remoteDialog}
          installScopes={installableScopes(remoteDialog.asset)}
          unverifiedTool={toolUnverified(remoteDialog.asset)}
          dirHint={dirPickerHint}
          onPickDir={() =>
            void pickDirectory((dir) =>
              setRemoteDialog((current) =>
                current ? { ...current, destDir: dir } : current
              )
            )
          }
          onPickProjectRoot={() =>
            void pickDirectory((dir) =>
              setRemoteDialog((current) =>
                current ? { ...current, projectRoot: dir } : current
              )
            )
          }
          onDestDir={(destDir) =>
            setRemoteDialog((current) =>
              current ? { ...current, destDir } : current
            )
          }
          onProjectRoot={(root) =>
            setRemoteDialog((current) =>
              current ? { ...current, projectRoot: root } : current
            )
          }
          onScope={(targetScope) =>
            setRemoteDialog((current) =>
              current ? { ...current, targetScope } : current
            )
          }
          onOverwrite={(overwrite) =>
            setRemoteDialog((current) =>
              current ? { ...current, overwrite } : current
            )
          }
          onRetryFetch={() => void retryRemoteFetch(remoteDialog)}
          onConfirm={() =>
            remoteDialog.mode === "download"
              ? void confirmRemoteDownload(remoteDialog)
              : void confirmRemoteInstall(remoteDialog)
          }
          onClose={() => {
            if (!remoteDialog.busy) {
              remoteDialogSeq.current += 1;
              setRemoteDialog(null);
            }
          }}
        />
      )}
    </div>
  );
}

// ===== 弹层状态类型 =====

/** 上传确认弹层状态（含 409 冲突分支与幂等请求 ID）。 */
/**
 * 页面内本地资源引用：discover 项 sourceOverride=null；手选导入项必须携带
 * 显式资源根（candidate.source_root），plan/pack 都按它定位。
 */
interface LocalAssetRef {
  asset: DiscoveredAsset;
  sourceOverride: string | null;
  /** 行级 busy/展开的唯一键（手选导入项以 import:<path> 区分） */
  key: string;
}

interface UploadDialogState {
  plan: ExportPlan;
  asset: DiscoveredAsset;
  /** 手选导入的显式资源根；discover 项为 null */
  sourceOverride: string | null;
  /** 同名远端资源（service 端 supported 时查询） */
  matches: RemoteAsset[];
  matchError: string | null;
  /** null = POST 新建；非 null = PUT 更新该资源 */
  targetAssetId: string | null;
  expectedVersion: number | null;
  /** POST 幂等键：断网重试复用；另存为新资源时换新 */
  requestId: string;
  /** 预览时的文件清单基线（path+size）；确认上传前重新打包后必须一致 */
  previewFiles: PreviewFileEntry[];
  /** 已打包的信封：首次确认后缓存，重试复用同一内容 */
  bundle: AssetBundle | null;
  conflict: { expected: number; current: number } | null;
  busy: boolean;
  error: AssetErrorInfo | null;
  result: RemoteAssetResult | null;
}

/** 导出到本机目录弹层状态。 */
interface ExportDialogState {
  plan: ExportPlan;
  asset: DiscoveredAsset;
  /** 手选导入的显式资源根；discover 项为 null */
  sourceOverride: string | null;
  destDir: string;
  overwrite: boolean;
  busy: boolean;
  error: AssetErrorInfo | null;
  result: DiskExportResult | null;
}

/** 远端下载 / 安装弹层状态。 */
interface RemoteDialogState {
  asset: RemoteAsset;
  version: number;
  mode: "download" | "install";
  bundle: AssetBundle | null;
  fetching: boolean;
  fetchError: AssetErrorInfo | null;
  destDir: string;
  targetScope: AssetScope;
  projectRoot: string;
  overwrite: boolean;
  busy: boolean;
  error: AssetErrorInfo | null;
  result: DiskExportResult | InstallResult | null;
}

// ===== 通用展示组件 =====

const dimText: CSSProperties = { color: "var(--db-text-dim)" };

/** 表单控件统一外观（复用 --db-* 变量，亮暗自动翻转）。 */
const selectClass =
  "h-[30px] rounded-[9px] border border-[var(--db-border)] bg-[var(--db-card-bg)] px-2 text-[12px] text-[var(--db-text)] outline-none focus:border-[var(--db-active-text)]";
const inputClass =
  "h-[30px] rounded-[9px] border border-[var(--db-border)] bg-[var(--db-card-bg)] px-2 text-[12px] text-[var(--db-text)] outline-none focus:border-[var(--db-active-text)]";

/** 能力四档小徽标：开 = 绿色实义；关 = 灰底删除线（title 说明不可用）。 */
function CapChip({ on, label }: { on: boolean; label: string }) {
  const { t } = useI18n();
  return (
    <span
      className={`rounded-full px-2 py-0.5 text-[10px] font-medium ${
        on
          ? "bg-emerald-500/12 text-emerald-700"
          : "bg-[var(--db-track)] text-[var(--db-text-dim)] line-through"
      }`}
      title={on ? undefined : t("dashboard.agentAssets.select.capOffHint")}
    >
      {on ? "✓" : "✕"} {label}
    </span>
  );
}

/** 是否整款工具的占位条目（DSH / MiniMax pending）。 */
function isToolPending(
  capabilities: CapabilityEntry[] | null,
  toolId: string
): boolean {
  if (!capabilities) return false;
  const caps = capabilities.filter((c) => c.tool === toolId);
  return caps.length > 0 && caps.every((c) => c.status !== "verified");
}

/** 手选 agent 文件的扩展名（与各适配器 ensure_agent_file 的允许列表一致）。 */
function agentImportExtensions(toolId: string): string[] {
  if (toolId === "codex") return ["toml"];
  if (toolId === "kimi") return ["md", "yaml", "yml"];
  return ["md"];
}

function kindLabel(t: TFn, kind: AssetKind): string {
  return kind === "agent"
    ? t("dashboard.agentAssets.select.kindAgent")
    : t("dashboard.agentAssets.select.kindSkill");
}

function scopeLabel(t: TFn, scope: AssetScope): string {
  return scope === "user"
    ? t("dashboard.agentAssets.select.scopeUser")
    : t("dashboard.agentAssets.select.scopeProject");
}

/** 远端返回的 kind/scope 是字符串透传（未知值原样显示，不报错）。 */
function remoteKindLabel(t: TFn, kind: string): string {
  if (kind === "agent") return t("dashboard.agentAssets.select.kindAgent");
  if (kind === "skill") return t("dashboard.agentAssets.select.kindSkill");
  return kind;
}

function remoteScopeLabel(t: TFn, scope: string): string {
  if (scope === "user") return t("dashboard.agentAssets.select.scopeUser");
  if (scope === "project") {
    return t("dashboard.agentAssets.select.scopeProject");
  }
  return scope;
}

function toolDisplayName(toolId: string): string {
  return TOOL_ORDER.find((item) => item.id === toolId)?.name ?? toolId;
}

/** 文件标记徽标（可执行 / 二进制）。 */
function FileFlags({
  executable,
  binary,
}: {
  executable: boolean;
  binary: boolean;
}) {
  const { t } = useI18n();
  return (
    <span className="flex flex-wrap gap-1">
      {executable && (
        <span className="db-tag db-tag--manual">
          {t("dashboard.agentAssets.local.executable")}
        </span>
      )}
      {binary && (
        <span className="db-tag db-tag--unpriced">
          {t("dashboard.agentAssets.local.binary")}
        </span>
      )}
    </span>
  );
}

// ===== 服务端状态卡 =====

function ServerStatusCard({
  server,
  busy,
  error,
  onRetry,
}: {
  server: ServerAssetStatus | null;
  busy: boolean;
  error: string | null;
  onRetry: () => void;
}) {
  const { t } = useI18n();
  const limits = server?.limits;

  return (
    <Card title={t("dashboard.agentAssets.server.title")}>
      {server?.insecure_transport && (
        <div className="mb-2">
          <AlertBanner type="warning">
            {t("dashboard.agentAssets.server.insecureTransport")}
          </AlertBanner>
        </div>
      )}
      {error ? (
        <AlertBanner type="error">
          {error}
          <button type="button" className="db-linkbtn ml-2" onClick={onRetry}>
            {t("dashboard.agentAssets.server.retry")}
          </button>
        </AlertBanner>
      ) : !server ? (
        <p className="db-empty">
          {busy
            ? t("dashboard.agentAssets.server.checking")
            : t("dashboard.agentAssets.server.unknown")}
        </p>
      ) : server.supported ? (
        <>
          <div className="flex flex-wrap items-center gap-2 text-[12px]">
            <span className="inline-block h-2 w-2 rounded-full bg-emerald-500" />
            <span style={{ color: "var(--db-text)" }}>
              {t("dashboard.agentAssets.server.supported", {
                version: server.api_version ?? "—",
              })}
            </span>
          </div>
          {limits && (
            <p className="mt-1 text-[11px] leading-relaxed" style={dimText}>
              {t("dashboard.agentAssets.server.limits", {
                file: formatBytes(limits.max_file_bytes),
                bundle: formatBytes(limits.max_bundle_bytes),
                request: formatBytes(limits.max_request_bytes),
                entries: limits.max_entries_per_bundle,
                revisions: limits.max_revisions_per_asset,
                quota: formatBytes(limits.library_quota_bytes),
              })}
            </p>
          )}
        </>
      ) : (
        <>
          <AlertBanner
            type={
              server.reason === "OLD_SERVER_UNSUPPORTED" ||
              server.reason === "CONFIG_MISSING"
                ? "warning"
                : "error"
            }
          >
            {server.reason === "CONFIG_MISSING"
              ? t("dashboard.agentAssets.server.configMissing")
              : server.reason === "OLD_SERVER_UNSUPPORTED"
                ? t("dashboard.agentAssets.server.oldServer")
                : server.reason === "UNAUTHORIZED"
                  ? t("dashboard.agentAssets.server.unauthorized")
                  : server.reason === "NETWORK_ERROR"
                    ? t("dashboard.agentAssets.server.network")
                    : t("dashboard.agentAssets.server.unknown")}
          </AlertBanner>
          <div className="flex flex-wrap items-center gap-3">
            {server.detail && (
              <span className="text-[11px]" style={dimText}>
                {t("dashboard.agentAssets.server.detail", {
                  detail: server.detail,
                })}
              </span>
            )}
            <span className="text-[11px]" style={dimText}>
              {t("dashboard.agentAssets.server.localOnly")}
            </span>
            <button
              type="button"
              className="db-btn db-btn--sm"
              onClick={onRetry}
              disabled={busy}
            >
              {t("dashboard.agentAssets.server.retry")}
            </button>
          </div>
        </>
      )}
    </Card>
  );
}

// ===== 本机 inspect 预览 =====

function InspectorContent({
  report,
  loading,
  error,
}: {
  report: InspectionReport | null;
  loading: boolean;
  error: AssetErrorInfo | null;
}) {
  const { t } = useI18n();
  if (loading) {
    return <p className="db-empty">{t("dashboard.agentAssets.local.inspectLoading")}</p>;
  }
  if (error) {
    return (
      <AlertBanner type="error">
        {t("dashboard.agentAssets.local.inspectError", {
          error: errorText(t, error),
        })}
      </AlertBanner>
    );
  }
  if (!report) {
    return <p className="db-empty">{t("dashboard.agentAssets.local.inspectLoading")}</p>;
  }

  const meta = report.metadata;
  const entryFile = report.files.find(
    (file) => file.path === report.asset.entry_path
  );
  const entryIsBinary = entryFile?.binary === true;
  return (
    <div className="flex flex-col gap-3">
      {/* 元数据 */}
      <div>
        <div className="mb-1 text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
          {t("dashboard.agentAssets.local.metadata")}
        </div>
        <div className="grid gap-1 text-[11px]" style={dimText}>
          <div>
            <span className="font-medium">
              {t("dashboard.agentAssets.local.metaName")}:{" "}
            </span>
            <span className="num">{meta.name ?? "—"}</span>
          </div>
          <div>
            <span className="font-medium">
              {t("dashboard.agentAssets.local.metaDescription")}:{" "}
            </span>
            {meta.description ?? "—"}
          </div>
          <div>
            <span className="font-medium">
              {t("dashboard.agentAssets.local.metaModel")}:{" "}
            </span>
            <span className="num">{meta.model ?? "—"}</span>
          </div>
          <div>
            <span className="font-medium">
              {t("dashboard.agentAssets.local.metaTools")}:{" "}
            </span>
            <span className="num">
              {meta.tools.length > 0 ? meta.tools.join(", ") : "—"}
            </span>
          </div>
          {!meta.parsed_ok && (
            <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-2 py-1 text-amber-700">
              {t("dashboard.agentAssets.local.parseFailed", {
                error: meta.parse_error ?? "—",
              })}
            </div>
          )}
        </div>
      </div>

      {/* 敏感字段（显眼样式） */}
      {report.sensitive_fields.length > 0 && (
        <div className="rounded-lg border border-rose-500/30 bg-rose-500/10 px-3 py-2 text-[11px] text-rose-600">
          <div className="font-semibold">
            {t("dashboard.agentAssets.local.sensitive", {
              n: report.sensitive_fields.length,
            })}
          </div>
          <p className="mt-0.5">{t("dashboard.agentAssets.local.sensitiveHint")}</p>
          <ul className="mt-1 list-disc break-all pl-4">
            {report.sensitive_fields.map((item, index) => (
              <li key={`${item.file}-${item.field}-${index}`}>
                <span className="num">{item.file}</span> · {item.field}: {item.reason}
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* 包外引用 */}
      {report.external_references.length > 0 && (
        <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-700">
          <div className="font-semibold">
            {t("dashboard.agentAssets.local.externalRefs", {
              n: report.external_references.length,
            })}
          </div>
          <ul className="mt-1 list-disc break-all pl-4">
            {report.external_references.map((ref, index) => (
              <li key={`${ref}-${index}`} className="num">
                {ref}
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* 警告 */}
      {report.warnings.length > 0 && (
        <div className="text-[11px]" style={dimText}>
          <div className="font-semibold" style={{ color: "var(--db-text)" }}>
            {t("dashboard.agentAssets.local.warningsTitle")}
          </div>
          <ul className="mt-1 list-disc break-words pl-4">
            {report.warnings.map((warning, index) => (
              <li key={`${warning}-${index}`}>{warning}</li>
            ))}
          </ul>
        </div>
      )}

      {/* 文件清单 */}
      <div>
        <div className="mb-1 text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
          {t("dashboard.agentAssets.local.files", { n: report.files.length })}
        </div>
        <div className="db-table-wrap">
          <table className="db-table">
            <thead>
              <tr>
                <th>{t("dashboard.agentAssets.plan.colPath")}</th>
                <th className="db-table__num">
                  {t("dashboard.agentAssets.plan.fileSize")}
                </th>
                <th>{t("dashboard.agentAssets.plan.colFlags")}</th>
              </tr>
            </thead>
            <tbody>
              {report.files.map((file) => (
                <tr key={file.path}>
                  <td className="num break-all">{file.path}</td>
                  <td className="db-table__num num">
                    {file.binary ? "—" : formatBytes(file.size)}
                  </td>
                  <td>
                    <FileFlags
                      executable={file.executable}
                      binary={file.binary}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      {/* frontmatter 原文（只读；二进制入口不展示内容） */}
      <div>
        <div className="mb-1 text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
          {t("dashboard.agentAssets.local.frontmatter")}
        </div>
        {entryIsBinary ? (
          <p className="db-empty">{t("dashboard.agentAssets.local.binary")}</p>
        ) : meta.frontmatter_raw ? (
          <pre
            className="num max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-lg border p-2 text-[11px]"
            style={{
              borderColor: "var(--db-border)",
              background: "var(--db-card-bg)",
              color: "var(--db-text)",
            }}
          >
            {meta.frontmatter_raw}
          </pre>
        ) : (
          <p className="db-empty">
            {t("dashboard.agentAssets.local.noFrontmatter")}
          </p>
        )}
      </div>
    </div>
  );
}

// ===== 手选导入结果（契约 §7.3） =====

function ImportResultsPanel({
  candidates,
  busy,
  error,
  canArchive,
  serverSupported,
  uploadBlockedReason,
  actionBusy,
  onRepick,
  onClear,
  onAction,
}: {
  candidates: ImportCandidate[] | null;
  busy: boolean;
  error: AssetErrorInfo | null;
  /** 当前工具 × 类型 × 用户级是否可归档（手选导入固定用户级） */
  canArchive: boolean;
  serverSupported: boolean;
  /** 上传禁用原因（服务端未就绪时的说明，挂在按钮 title 上） */
  uploadBlockedReason: string | null;
  actionBusy: { key: string; action: "plan" | "upload" | "export" } | null;
  onRepick: () => void;
  onClear: () => void;
  onAction: (ref: LocalAssetRef, action: "plan" | "upload" | "export") => void;
}) {
  const { t } = useI18n();
  const okCount = candidates?.filter((item) => item.importable).length ?? 0;
  const anyBusy = actionBusy != null;

  return (
    <div
      className="mb-3 rounded-xl border p-3"
      style={{
        borderColor: "var(--db-border)",
        background: "var(--db-hover)",
      }}
    >
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-baseline gap-2">
          <span
            className="text-[12px] font-semibold"
            style={{ color: "var(--db-text)" }}
          >
            {t("dashboard.agentAssets.import.title")}
          </span>
          {candidates && (
            <span className="text-[11px]" style={dimText}>
              {t("dashboard.agentAssets.import.sub", {
                ok: okCount,
                total: candidates.length,
              })}
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="db-btn db-btn--sm"
            onClick={onRepick}
            disabled={busy || !canArchive}
          >
            {t("dashboard.agentAssets.import.repick")}
          </button>
          <button
            type="button"
            className="db-linkbtn"
            onClick={onClear}
            disabled={busy}
          >
            {t("dashboard.agentAssets.import.clear")}
          </button>
        </div>
      </div>
      <p className="mb-2 text-[11px]" style={dimText}>
        {t("dashboard.agentAssets.import.scopeHint")}
      </p>

      {busy ? (
        <p className="db-empty">
          {t("dashboard.agentAssets.import.loading")}
        </p>
      ) : error ? (
        <AlertBanner type="error">{errorText(t, error)}</AlertBanner>
      ) : !candidates || candidates.length === 0 ? (
        <p className="db-empty">
          {t("dashboard.agentAssets.import.empty")}
        </p>
      ) : (
        <div>
          {candidates.map((candidate) => {
            const candidateAsset = candidate.asset;
            const candidateMeta = candidate.metadata;
            const importable =
              candidate.importable &&
              candidateAsset != null &&
              candidateMeta != null;
            if (!importable || !candidateAsset || !candidateMeta) {
              const reason =
                candidate.error ??
                candidateMeta?.parse_error ??
                candidate.warnings.join("; ");
              return (
                <div
                  key={`bad:${candidate.path}`}
                  className="border-b py-3 last:border-b-0"
                  style={{ borderColor: "var(--db-row-border)" }}
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="db-tag db-tag--unpriced">
                      {t("dashboard.agentAssets.import.notImportable")}
                    </span>
                    <span
                      className="num break-all text-[11px]"
                      style={dimText}
                    >
                      {candidate.path}
                    </span>
                  </div>
                  <div className="mt-1 break-words rounded-lg border border-rose-500/30 bg-rose-500/10 px-2.5 py-1.5 text-[11px] text-rose-600">
                    {reason || "—"}
                  </div>
                  {candidate.files.length > 0 && (
                    <p className="mt-1 text-[10px]" style={dimText}>
                      {t("dashboard.agentAssets.local.files", {
                        n: candidate.files.length,
                      })}
                    </p>
                  )}
                </div>
              );
            }

            const key = `import:${candidate.path}`;
            const rowBusy = actionBusy?.key === key;
            const override =
              candidate.source_root ?? candidateAsset.source_root;
            const report: InspectionReport = {
              asset: candidateAsset,
              metadata: candidateMeta,
              files: candidate.files,
              sensitive_fields: candidate.sensitive_fields,
              warnings: candidate.warnings,
              external_references: candidate.external_references,
            };
            return (
              <div
                key={`ok:${candidate.path}`}
                className="border-b py-3 last:border-b-0"
                style={{ borderColor: "var(--db-row-border)" }}
              >
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span
                        className="text-[13px] font-semibold"
                        style={{ color: "var(--db-text)" }}
                      >
                        {candidateAsset.display_name}
                      </span>
                      <span className="num text-[11px]" style={dimText}>
                        {candidateAsset.native_id}
                      </span>
                      <span className="db-tag db-tag--new">
                        {candidateAsset.native_format}
                      </span>
                      {candidate.warnings.length > 0 && (
                        <span
                          className="rounded-full bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium text-amber-700"
                          title={candidate.warnings.join("\n")}
                        >
                          ⚠{" "}
                          {t("dashboard.agentAssets.local.warnings", {
                            n: candidate.warnings.length,
                          })}
                        </span>
                      )}
                    </div>
                    {candidateAsset.description && (
                      <p
                        className="mt-1 break-words text-[11px] leading-relaxed"
                        style={dimText}
                      >
                        {candidateAsset.description}
                      </p>
                    )}
                    <p
                      className="num mt-1 break-all text-[10px]"
                      style={dimText}
                      title={candidate.path}
                    >
                      {t("dashboard.agentAssets.import.path")}: {candidate.path}
                    </p>
                    <p
                      className="num mt-0.5 break-all text-[10px]"
                      style={dimText}
                      title={override}
                    >
                      {t("dashboard.agentAssets.local.sourceRoot")}: {override}
                    </p>
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      type="button"
                      className="db-btn db-btn--sm"
                      disabled={!canArchive || anyBusy}
                      onClick={() =>
                        onAction(
                          { asset: candidateAsset, sourceOverride: override, key },
                          "plan"
                        )
                      }
                    >
                      {rowBusy && actionBusy?.action === "plan"
                        ? t("dashboard.agentAssets.local.actionPlanBusy")
                        : t("dashboard.agentAssets.local.actionPlan")}
                    </button>
                    <button
                      type="button"
                      className="db-btn db-btn--sm db-btn--primary"
                      disabled={!canArchive || anyBusy || !serverSupported}
                      title={uploadBlockedReason ?? undefined}
                      onClick={() =>
                        onAction(
                          { asset: candidateAsset, sourceOverride: override, key },
                          "upload"
                        )
                      }
                    >
                      {rowBusy && actionBusy?.action === "upload"
                        ? t("dashboard.agentAssets.local.actionUploadBusy")
                        : t("dashboard.agentAssets.local.actionUpload")}
                    </button>
                    <button
                      type="button"
                      className="db-btn db-btn--sm"
                      disabled={!canArchive || anyBusy}
                      onClick={() =>
                        onAction(
                          { asset: candidateAsset, sourceOverride: override, key },
                          "export"
                        )
                      }
                    >
                      {rowBusy && actionBusy?.action === "export"
                        ? t("dashboard.agentAssets.local.actionExportBusy")
                        : t("dashboard.agentAssets.local.actionExport")}
                    </button>
                  </div>
                </div>
                <div className="mt-2">
                  <InspectorContent report={report} loading={false} error={null} />
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

// ===== 导出计划详情（预览 / 上传确认 / 导出共用） =====

function PlanDetails({
  plan,
  subtitle,
}: {
  plan: ExportPlan;
  subtitle: string;
}) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-3">
      <div className="text-[11px]" style={dimText}>
        {subtitle}
      </div>
      <div className="grid gap-1 text-[11px]" style={dimText}>
        <div className="break-all">
          <span className="font-medium">
            {t("dashboard.agentAssets.plan.entry")}:{" "}
          </span>
          <span className="num">{plan.entry_path}</span>
        </div>
        <div className="break-all">
          <span className="font-medium">
            {t("dashboard.agentAssets.plan.sourceRoot")}:{" "}
          </span>
          <span className="num">{plan.source_root}</span>
        </div>
        {plan.target_versions.length > 0 && (
          <div>
            <span className="font-medium">
              {t("dashboard.agentAssets.plan.targetVersions")}:{" "}
            </span>
            <span className="num">{plan.target_versions.join(", ")}</span>
          </div>
        )}
      </div>

      {plan.sensitive_fields.length > 0 && (
        <div className="rounded-lg border border-rose-500/30 bg-rose-500/10 px-3 py-2 text-[11px] text-rose-600">
          <div className="font-semibold">
            {t("dashboard.agentAssets.plan.sensitive", {
              n: plan.sensitive_fields.length,
            })}
          </div>
          <ul className="mt-1 list-disc break-all pl-4">
            {plan.sensitive_fields.map((item, index) => (
              <li key={`${item.file}-${item.field}-${index}`}>
                <span className="num">{item.file}</span> · {item.field}: {item.reason}
              </li>
            ))}
          </ul>
        </div>
      )}

      {plan.external_references.length > 0 && (
        <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-700">
          <div className="font-semibold">
            {t("dashboard.agentAssets.plan.external", {
              n: plan.external_references.length,
            })}
          </div>
          <ul className="mt-1 list-disc break-all pl-4">
            {plan.external_references.map((ref, index) => (
              <li key={`${ref}-${index}`} className="num">
                {ref}
              </li>
            ))}
          </ul>
        </div>
      )}

      {plan.warnings.length > 0 && (
        <div className="text-[11px]" style={dimText}>
          <div className="font-semibold" style={{ color: "var(--db-text)" }}>
            {t("dashboard.agentAssets.plan.warnings")}
          </div>
          <ul className="mt-1 list-disc break-words pl-4">
            {plan.warnings.map((warning, index) => (
              <li key={`${warning}-${index}`}>{warning}</li>
            ))}
          </ul>
        </div>
      )}

      <div>
        <div className="mb-1 text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
          {t("dashboard.agentAssets.plan.files", { n: plan.files.length })}
        </div>
        <div className="db-table-wrap max-h-60 overflow-y-auto">
          <table className="db-table">
            <thead>
              <tr>
                <th>{t("dashboard.agentAssets.plan.colPath")}</th>
                <th>{t("dashboard.agentAssets.plan.colFlags")}</th>
              </tr>
            </thead>
            <tbody>
              {plan.files.map((file) => (
                <tr key={file.path}>
                  <td className="num break-all">{file.path}</td>
                  <td>
                    <FileFlags
                      executable={file.executable}
                      binary={file.binary}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}

// ===== 通用弹层 =====

function Modal({
  title,
  onClose,
  children,
  footer,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}) {
  const { t } = useI18n();
  useEscape(true, onClose);
  return (
    <div
      className="db-confirm-mask"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className="db-confirm"
        role="dialog"
        aria-modal="true"
        style={{
          width: 760,
          maxWidth: "calc(100vw - 32px)",
          maxHeight: "calc(100vh - 96px)",
          padding: 0,
          display: "flex",
          flexDirection: "column",
        }}
      >
        <header
          className="flex items-center justify-between gap-3 border-b px-4 py-3"
          style={{ borderColor: "var(--db-border)" }}
        >
          <h2 className="text-[13px] font-semibold" style={{ color: "var(--db-text)" }}>
            {title}
          </h2>
          <button
            type="button"
            className="db-linkbtn"
            onClick={onClose}
            aria-label={t("dashboard.agentAssets.plan.close")}
            title={t("dashboard.agentAssets.plan.close")}
          >
            ✕
          </button>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">{children}</div>
        {footer && (
          <footer
            className="flex flex-wrap items-center justify-end gap-2 border-t px-4 py-3"
            style={{ borderColor: "var(--db-border)" }}
          >
            {footer}
          </footer>
        )}
      </div>
    </div>
  );
}

// ===== 上传确认弹层 =====

function UploadDialog({
  state,
  onTarget,
  onConfirm,
  onRePreview,
  onSaveAsNew,
  onClose,
}: {
  state: UploadDialogState;
  onTarget: (target: RemoteAsset | null) => void;
  onConfirm: () => void;
  onRePreview: () => void;
  onSaveAsNew: () => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const result = state.result;
  const conflict = state.conflict;

  return (
    <Modal
      title={t("dashboard.agentAssets.plan.uploadTitle")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="db-btn" onClick={onClose} disabled={state.busy}>
            {t("dashboard.agentAssets.plan.close")}
          </button>
          {!result && (
            <button
              type="button"
              className="db-btn db-btn--primary"
              onClick={onConfirm}
              disabled={state.busy || conflict != null}
            >
              {state.busy
                ? t("dashboard.agentAssets.plan.uploading")
                : state.error?.code === "NETWORK_ERROR"
                  ? t("dashboard.agentAssets.plan.retryUpload")
                  : t("dashboard.agentAssets.plan.confirmUpload")}
            </button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <PlanDetails
          plan={state.plan}
          subtitle={t("dashboard.agentAssets.plan.subtitle", {
            tool: toolDisplayName(state.plan.tool),
            kind: kindLabel(t, state.plan.kind),
            name: state.plan.display_name,
          })}
        />

        {result ? (
          <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-[11px] text-emerald-700">
            <div className="font-semibold">
              {t("dashboard.agentAssets.plan.uploadSuccess", {
                name: result.asset.display_name,
                version: result.revision.version,
              })}
            </div>
            <div className="mt-1 flex flex-wrap gap-1">
              {result.replayed && (
                <span className="db-tag db-tag--new">
                  {t("dashboard.agentAssets.plan.replayed")}
                </span>
              )}
              {result.deduplicated && (
                <span className="db-tag db-tag--new">
                  {t("dashboard.agentAssets.plan.deduplicated")}
                </span>
              )}
            </div>
            <div className="num mt-1 break-all">
              {t("dashboard.agentAssets.plan.assetId", {
                id: result.asset.asset_id,
              })}
            </div>
          </div>
        ) : (
          <>
            {conflict && (
              <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[11px] text-amber-700">
                <div className="font-semibold">
                  {t("dashboard.agentAssets.plan.conflictTitle")}
                </div>
                <p className="mt-1">
                  {t("dashboard.agentAssets.plan.conflictDesc", {
                    current: conflict.current,
                    expected: conflict.expected,
                  })}
                </p>
                <div className="mt-2 flex flex-wrap gap-2">
                  <button
                    type="button"
                    className="db-btn db-btn--sm"
                    onClick={onRePreview}
                    disabled={state.busy}
                  >
                    {t("dashboard.agentAssets.plan.rePreview")}
                  </button>
                  <button
                    type="button"
                    className="db-btn db-btn--sm"
                    onClick={onSaveAsNew}
                    disabled={state.busy}
                  >
                    {t("dashboard.agentAssets.plan.saveAsNew")}
                  </button>
                </div>
              </div>
            )}

            {state.error && (
              <AlertBanner type="error">
                {errorText(t, state.error)}
              </AlertBanner>
            )}

            <div>
              <div className="mb-1 text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
                {t("dashboard.agentAssets.plan.target")}
              </div>
              <div className="flex flex-col gap-1">
                <label className="flex items-center gap-2 text-[11px]" style={dimText}>
                  <input
                    type="radio"
                    name="agent-assets-upload-target"
                    checked={state.targetAssetId == null}
                    disabled={state.busy}
                    onChange={() => onTarget(null)}
                  />
                  {t("dashboard.agentAssets.plan.createNew")}
                </label>
                {state.matches.map((match) => (
                  <label
                    key={match.asset_id}
                    className="flex items-center gap-2 text-[11px]"
                    style={dimText}
                  >
                    <input
                      type="radio"
                      name="agent-assets-upload-target"
                      checked={state.targetAssetId === match.asset_id}
                      disabled={state.busy}
                      onChange={() => onTarget(match)}
                    />
                    {t("dashboard.agentAssets.plan.updateTo", {
                      name: match.display_name,
                      next: match.latest_version + 1,
                      current: match.latest_version,
                    })}
                  </label>
                ))}
              </div>
              {state.matchError && (
                <p className="mt-1 text-[11px] text-amber-700">
                  {t("dashboard.agentAssets.plan.matchError", {
                    error: state.matchError,
                  })}
                </p>
              )}
            </div>

            <div className="text-[11px]" style={dimText}>
              {t("dashboard.agentAssets.plan.uploadHint")}
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}

// ===== 导出到本机目录弹层 =====

function ExportDialog({
  state,
  dirHint,
  onPickDir,
  onDestDir,
  onOverwrite,
  onConfirm,
  onClose,
}: {
  state: ExportDialogState;
  dirHint: string | null;
  onPickDir: () => void;
  onDestDir: (destDir: string) => void;
  onOverwrite: (overwrite: boolean) => void;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const result = state.result;

  return (
    <Modal
      title={t("dashboard.agentAssets.plan.exportTitle")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="db-btn" onClick={onClose} disabled={state.busy}>
            {t("dashboard.agentAssets.plan.close")}
          </button>
          {!result && (
            <button
              type="button"
              className="db-btn db-btn--primary"
              onClick={onConfirm}
              disabled={state.busy}
            >
              {state.busy
                ? t("dashboard.agentAssets.plan.exporting")
                : t("dashboard.agentAssets.plan.confirmExport")}
            </button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <PlanDetails
          plan={state.plan}
          subtitle={t("dashboard.agentAssets.plan.subtitle", {
            tool: toolDisplayName(state.plan.tool),
            kind: kindLabel(t, state.plan.kind),
            name: state.plan.display_name,
          })}
        />

        {result ? (
          <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-[11px] text-emerald-700">
            <div className="font-semibold">
              {t("dashboard.agentAssets.plan.exportSuccess", {
                n: result.written.length,
              })}
            </div>
            {result.skipped.length > 0 && (
              <div className="mt-1">
                {t("dashboard.agentAssets.plan.skipped", {
                  n: result.skipped.length,
                })}
              </div>
            )}
            {result.backup_dir && (
              <div className="num mt-1 break-all">
                {t("dashboard.agentAssets.plan.backupDir", {
                  dir: result.backup_dir,
                })}
              </div>
            )}
            {result.warnings.length > 0 && (
              <ul className="mt-1 list-disc break-words pl-4">
                {result.warnings.map((warning, index) => (
                  <li key={`${warning}-${index}`}>{warning}</li>
                ))}
              </ul>
            )}
          </div>
        ) : (
          <>
            {state.error && (
              <AlertBanner type="error">
                {actionErrorText(t, state.error)}
              </AlertBanner>
            )}
            <label className="flex flex-col gap-1">
              <span className="text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
                {t("dashboard.agentAssets.plan.destDir")}
              </span>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  className={`${inputClass} flex-1`}
                  value={state.destDir}
                  placeholder={t(
                    "dashboard.agentAssets.plan.destDirPlaceholder"
                  )}
                  onChange={(event) => onDestDir(event.target.value)}
                />
                <button
                  type="button"
                  className="db-btn db-btn--sm"
                  onClick={onPickDir}
                >
                  {t("dashboard.agentAssets.select.chooseDir")}
                </button>
              </div>
              {dirHint && (
                <span className="text-[11px]" style={dimText}>
                  {dirHint}
                </span>
              )}
            </label>
            <label className="flex items-center gap-2 text-[11px]" style={dimText}>
              <input
                type="checkbox"
                checked={state.overwrite}
                onChange={(event) => onOverwrite(event.target.checked)}
              />
              {t("dashboard.agentAssets.plan.overwrite")}
            </label>
            <div className="text-[11px]" style={dimText}>
              {t("dashboard.agentAssets.plan.exportHint")}
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}

// ===== 远端下载 / 安装弹层 =====

function BundleDetails({ bundle }: { bundle: AssetBundle }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-3">
      <div>
        <div className="mb-1 text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
          {t("dashboard.agentAssets.transfer.fingerprint")}
        </div>
        <div className="grid gap-1 text-[11px]" style={dimText}>
          <div className="break-all">
            <span className="font-medium">
              {t("dashboard.agentAssets.transfer.manifestSha")}:{" "}
            </span>
            <span className="num" title={bundle.manifest_sha256}>
              {shortHash(bundle.manifest_sha256)}
            </span>
          </div>
          <div className="break-all">
            <span className="font-medium">
              {t("dashboard.agentAssets.transfer.bundleSha")}:{" "}
            </span>
            <span className="num" title={bundle.bundle_sha256}>
              {shortHash(bundle.bundle_sha256)}
            </span>
          </div>
        </div>
      </div>

      <div>
        <div className="mb-1 text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
          {t("dashboard.agentAssets.transfer.files", {
            n: bundle.manifest.files.length,
          })}
        </div>
        <div className="db-table-wrap max-h-60 overflow-y-auto">
          <table className="db-table">
            <thead>
              <tr>
                <th>{t("dashboard.agentAssets.plan.colPath")}</th>
                <th className="db-table__num">
                  {t("dashboard.agentAssets.plan.fileSize")}
                </th>
                <th>{t("dashboard.agentAssets.plan.colFlags")}</th>
              </tr>
            </thead>
            <tbody>
              {bundle.manifest.files.map((file) => (
                <tr key={file.path}>
                  <td className="num break-all">{file.path}</td>
                  <td className="db-table__num num">
                    {file.binary ? "—" : formatBytes(file.size)}
                  </td>
                  <td>
                    <FileFlags
                      executable={file.executable}
                      binary={file.binary}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}

function RemoteTransferDialog({
  state,
  installScopes,
  unverifiedTool,
  dirHint,
  onPickDir,
  onPickProjectRoot,
  onDestDir,
  onProjectRoot,
  onScope,
  onOverwrite,
  onRetryFetch,
  onConfirm,
  onClose,
}: {
  state: RemoteDialogState;
  /** 该资源在当前能力矩阵下可安装的目标作用域（空 = 不支持安装） */
  installScopes: AssetScope[];
  /** 目标工具的 verified_versions 为空（未在本机实测） */
  unverifiedTool: boolean;
  dirHint: string | null;
  onPickDir: () => void;
  onPickProjectRoot: () => void;
  onDestDir: (destDir: string) => void;
  onProjectRoot: (root: string) => void;
  onScope: (scope: AssetScope) => void;
  onOverwrite: (overwrite: boolean) => void;
  onRetryFetch: () => void;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const installing = state.mode === "install";
  const result = state.result;

  return (
    <Modal
      title={
        installing
          ? t("dashboard.agentAssets.transfer.installTitle")
          : t("dashboard.agentAssets.transfer.downloadTitle")
      }
      onClose={onClose}
      footer={
        <>
          <button type="button" className="db-btn" onClick={onClose} disabled={state.busy}>
            {t("dashboard.agentAssets.transfer.close")}
          </button>
          {!result && !state.fetching && !state.fetchError && state.bundle && (
            <button
              type="button"
              className="db-btn db-btn--primary"
              onClick={onConfirm}
              disabled={
                state.busy || (installing && installScopes.length === 0)
              }
            >
              {state.busy
                ? installing
                  ? t("dashboard.agentAssets.transfer.installing")
                  : t("dashboard.agentAssets.transfer.downloading")
                : installing
                  ? t("dashboard.agentAssets.transfer.confirmInstall")
                  : t("dashboard.agentAssets.transfer.confirmDownload")}
            </button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <div className="text-[11px]" style={dimText}>
          {t("dashboard.agentAssets.transfer.subtitle", {
            name: state.asset.display_name,
            version: state.version,
          })}{" "}
          · {toolDisplayName(state.asset.tool)} ·{" "}
          {remoteKindLabel(t, state.asset.kind)}
        </div>

        {state.fetching ? (
          <p className="db-empty">
            {t("dashboard.agentAssets.transfer.fetching")}
          </p>
        ) : state.fetchError ? (
          <>
            <AlertBanner type="error">
              {t("dashboard.agentAssets.transfer.fetchError", {
                error: errorText(t, state.fetchError),
              })}
            </AlertBanner>
            <button type="button" className="db-btn db-btn--sm" onClick={onRetryFetch}>
              {t("dashboard.agentAssets.error.retry")}
            </button>
          </>
        ) : state.bundle ? (
          <>
            <BundleDetails bundle={state.bundle} />
            {result ? (
              <ResultBlock state={state} />
            ) : (
              <>
                {state.error && (
                  <AlertBanner type="error">
                    {actionErrorText(t, state.error)}
                  </AlertBanner>
                )}

                {installing ? (
                  <>
                    {installScopes.length === 0 && (
                      <AlertBanner type="warning">
                        {t("dashboard.agentAssets.remote.installDisabled")}
                      </AlertBanner>
                    )}
                    {unverifiedTool && installScopes.length > 0 && (
                      <AlertBanner type="warning">
                        {t("dashboard.agentAssets.transfer.unverifiedHint")}
                      </AlertBanner>
                    )}
                    <div>
                      <div className="mb-1 text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
                        {t("dashboard.agentAssets.transfer.installScope")}
                      </div>
                      <div className="db-seg db-seg--sm" role="group">
                        {installScopes.map((scope) => (
                          <button
                            key={scope}
                            type="button"
                            className={`db-seg__btn ${
                              state.targetScope === scope
                                ? "db-seg__btn--active"
                                : ""
                            }`}
                            onClick={() => onScope(scope)}
                            aria-pressed={state.targetScope === scope}
                          >
                            {scopeLabel(t, scope)}
                          </button>
                        ))}
                      </div>
                    </div>
                    {state.targetScope === "project" && (
                      <label className="flex flex-col gap-1">
                        <span className="text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
                          {t("dashboard.agentAssets.transfer.projectRoot")}
                        </span>
                        <div className="flex items-center gap-2">
                          <input
                            type="text"
                            className={`${inputClass} flex-1`}
                            value={state.projectRoot}
                            list={PROJECT_DATALIST_ID}
                            placeholder={t(
                              "dashboard.agentAssets.select.projectRootPlaceholder"
                            )}
                            onChange={(event) =>
                              onProjectRoot(event.target.value)
                            }
                          />
                          <button
                            type="button"
                            className="db-btn db-btn--sm"
                            onClick={onPickProjectRoot}
                          >
                            {t("dashboard.agentAssets.select.chooseDir")}
                          </button>
                        </div>
                      </label>
                    )}
                    <div className="text-[11px]" style={dimText}>
                      {t("dashboard.agentAssets.transfer.installHint")}
                    </div>
                  </>
                ) : (
                  <>
                    <label className="flex flex-col gap-1">
                      <span className="text-[11px] font-semibold" style={{ color: "var(--db-text)" }}>
                        {t("dashboard.agentAssets.plan.destDir")}
                      </span>
                      <div className="flex items-center gap-2">
                        <input
                          type="text"
                          className={`${inputClass} flex-1`}
                          value={state.destDir}
                          placeholder={t(
                            "dashboard.agentAssets.plan.destDirPlaceholder"
                          )}
                          onChange={(event) => onDestDir(event.target.value)}
                        />
                        <button
                          type="button"
                          className="db-btn db-btn--sm"
                          onClick={onPickDir}
                        >
                          {t("dashboard.agentAssets.select.chooseDir")}
                        </button>
                      </div>
                    </label>
                    <div className="text-[11px]" style={dimText}>
                      {t("dashboard.agentAssets.transfer.downloadHint")}
                    </div>
                  </>
                )}

                <label className="flex items-center gap-2 text-[11px]" style={dimText}>
                  <input
                    type="checkbox"
                    checked={state.overwrite}
                    onChange={(event) => onOverwrite(event.target.checked)}
                  />
                  {t("dashboard.agentAssets.plan.overwrite")}
                </label>
                {dirHint && (
                  <p className="text-[11px]" style={dimText}>
                    {dirHint}
                  </p>
                )}
              </>
            )}
          </>
        ) : null}
      </div>
    </Modal>
  );
}

/** 下载/安装结果块（按 mode 解释 returned 结构）。 */
function ResultBlock({ state }: { state: RemoteDialogState }) {
  const { t } = useI18n();
  if (state.mode === "download") {
    const result = state.result as DiskExportResult;
    return (
      <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-[11px] text-emerald-700">
        <div className="font-semibold">
          {t("dashboard.agentAssets.transfer.exportSuccess", {
            n: result.written.length,
          })}
        </div>
        {result.skipped.length > 0 && (
          <div className="mt-1">
            {t("dashboard.agentAssets.transfer.skipped", {
              n: result.skipped.length,
            })}
          </div>
        )}
        {result.backup_dir && (
          <div className="num mt-1 break-all">
            {t("dashboard.agentAssets.transfer.backupDir", {
              dir: result.backup_dir,
            })}
          </div>
        )}
        {result.warnings.length > 0 && (
          <ul className="mt-1 list-disc break-words pl-4">
            {result.warnings.map((warning, index) => (
              <li key={`${warning}-${index}`}>{warning}</li>
            ))}
          </ul>
        )}
      </div>
    );
  }
  const result = state.result as InstallResult;
  return (
    <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-3 py-2 text-[11px] text-emerald-700">
      <div className="font-semibold">
        {t("dashboard.agentAssets.transfer.installSuccess", {
          n: result.installed_paths.length,
        })}
      </div>
      {result.skipped.length > 0 && (
        <div className="mt-1">
          {t("dashboard.agentAssets.transfer.skipped", {
            n: result.skipped.length,
          })}
        </div>
      )}
      {result.backup_dir && (
        <div className="num mt-1 break-all">
          {t("dashboard.agentAssets.transfer.backupDir", {
            dir: result.backup_dir,
          })}
        </div>
      )}
      {result.warnings.length > 0 && (
        <ul className="mt-1 list-disc break-words pl-4">
          {result.warnings.map((warning, index) => (
            <li key={`${warning}-${index}`}>{warning}</li>
          ))}
        </ul>
      )}
      {result.tool_instructions && (
        <div className="mt-2">
          <div className="font-semibold">
            {t("dashboard.agentAssets.transfer.toolInstructions")}
          </div>
          <pre className="num mt-1 whitespace-pre-wrap break-words">
            {result.tool_instructions}
          </pre>
        </div>
      )}
    </div>
  );
}
