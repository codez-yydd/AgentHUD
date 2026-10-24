//! 多设备同步：配置读写 + 增量上传 + HTTP 调用 + 后台同步线程。
//!
//! 设计要点（见 server/README.md）：
//! - model_usage 的普通明细用 (device_id, source, local_rowid) 去重；模型名回填通过
//!   独立修订游标补传，不影响普通 rowid 游标。
//! - 客户端维护游标 last_uploaded_rowid，只上传 rowid > 游标 的新记录。
//! - 复用项目现有 ureq HTTP 客户端 + pricing::config_dir() 的 ~/.agenthud/ 目录（更名前 ~/.zbar，启动自动迁移）。
//! - 本地采集型来源（codex / claude / dsh / opencode / kimi / minimaxcode）各自一套
//!   「rowid 游标 + 修订游标」，见 upload_derived_source_incremental。
//! - HTTP 超时分层（agent_with_timeouts）：连接期统一 timeout_connect 快速失败，
//!   总超时按调用场景分三档——后台静默查询 connect 2s/总 5s、后台上传与
//!   协议探测 connect 3s/总 15s、用户主动操作 connect 3s/总超时保持原值。
//!   各路径调用方均有静默降级或下轮重试兜底，快速失败不损失正确性。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use crate::agent_quota_history::{self, AgentQuotaSnapshot};
use crate::db::{self, default_source, UsageRow};
use crate::pricing::config_dir;

/// 桌面端价格表同步完成后通知前端重读本地 pricing.json。
/// 未初始化（单测/无窗口）时 emit 为 no-op。
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

pub fn set_app_handle(handle: AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

pub(crate) fn emit_pricing_updated(concurrent: bool, local: bool) {
    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit(
            "zbar://pricing-updated",
            serde_json::json!({ "concurrent": concurrent, "local": local }),
        );
    }
}

// ===== 配置（~/.agenthud/sync.json）=====

/// 同步模式。
/// - manual：仅手动点「立即同步」时上传。
/// - auto：后台线程按 interval_seconds 自动上传。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    Manual,
    Auto,
}

impl Default for SyncMode {
    fn default() -> Self {
        SyncMode::Manual
    }
}

/// 同步配置。master_token 不持久化（注册完即丢）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: SyncMode,
    /// auto 模式间隔（秒），默认 60。
    #[serde(default = "default_interval")]
    pub interval_seconds: u64,
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub device_token: String,
    /// 已上传到的本机 rowid 游标。
    #[serde(default)]
    pub last_uploaded_rowid: i64,
    /// 已上传到的 Codex 导入库 rowid 游标（与 zcode 游标相互独立）。
    #[serde(default)]
    pub last_uploaded_codex_rowid: i64,
    /// 已上传到的 Claude 导入库 rowid 游标（与上面两个游标相互独立）。
    #[serde(default)]
    pub last_uploaded_claude_rowid: i64,
    /// Claude 修订行补传游标（updated_at 毫秒时间戳，见 claude 模块修订机制）。
    #[serde(default)]
    pub last_uploaded_claude_rev_ts: i64,
    /// Codex 模型名修订行补传游标（模型上下文回填时间戳）。
    #[serde(default)]
    pub last_uploaded_codex_model_rev_ts: i64,
    /// 同一修订时间戳下的 Codex 模型修订 rowid（处理毫秒级并列）。
    #[serde(default)]
    pub last_uploaded_codex_model_rev_id: i64,
    /// 已上传到的 DSH 导入库 rowid 游标（与上面各来源相互独立）。
    #[serde(default)]
    pub last_uploaded_dsh_rowid: i64,
    /// DSH 修订行补传游标（updated_at 毫秒时间戳，见 dsh 模块修订机制）。
    #[serde(default)]
    pub last_uploaded_dsh_rev_ts: i64,
    /// 已上传到的 OpenCode 导入库 rowid 游标（与上面各来源相互独立）。
    #[serde(default)]
    pub last_uploaded_opencode_rowid: i64,
    /// OpenCode 修订行补传游标（updated_at 毫秒时间戳，见 opencode 模块修订机制）。
    #[serde(default)]
    pub last_uploaded_opencode_rev_ts: i64,
    /// 已上传到的 Kimi 导入库 rowid 游标（与 Kimi 额度快照是两条独立通道）。
    #[serde(default)]
    pub last_uploaded_kimi_rowid: i64,
    /// Kimi 修订行补传游标（updated_at 毫秒时间戳，见 kimi 模块修订机制）。
    #[serde(default)]
    pub last_uploaded_kimi_rev_ts: i64,
    /// 已上传到的 MiniMax Code 导入库 rowid 游标（与上面各来源相互独立）。
    #[serde(default)]
    pub last_uploaded_minimaxcode_rowid: i64,
    /// MiniMax Code 修订行补传游标（updated_at 毫秒时间戳，见 minimaxcode
    /// 模块修订机制）。
    #[serde(default)]
    pub last_uploaded_minimaxcode_rev_ts: i64,
    /// 已上传到的快照 ts 游标（额度快照）。
    #[serde(default)]
    pub last_uploaded_snapshot_ts: i64,
    /// 已上传到的 Agent 额度快照 ts 游标。
    #[serde(default)]
    pub last_uploaded_agent_quota_snapshot_ts: i64,
    /// 已上传到的 provider 额度快照 ts 游标（方案 B：deepseek 等 11 个
    /// 凭证型 provider 的快照与 Agent 快照同通道混传，但游标独立——
    /// 仅在服务端 proto >= 7 时推进，旧服务端丢弃新 source 后不会丢数据）。
    #[serde(default)]
    pub last_uploaded_provider_quota_ts: i64,
    /// 上次成功同步的毫秒时间戳。
    #[serde(default)]
    pub last_sync_at: i64,
}

fn default_interval() -> u64 {
    60
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: SyncMode::Manual,
            interval_seconds: default_interval(),
            server_url: String::new(),
            device_id: String::new(),
            device_name: String::new(),
            device_token: String::new(),
            last_uploaded_rowid: 0,
            last_uploaded_codex_rowid: 0,
            last_uploaded_claude_rowid: 0,
            last_uploaded_claude_rev_ts: 0,
            last_uploaded_codex_model_rev_ts: 0,
            last_uploaded_codex_model_rev_id: 0,
            last_uploaded_dsh_rowid: 0,
            last_uploaded_dsh_rev_ts: 0,
            last_uploaded_opencode_rowid: 0,
            last_uploaded_opencode_rev_ts: 0,
            last_uploaded_kimi_rowid: 0,
            last_uploaded_kimi_rev_ts: 0,
            last_uploaded_minimaxcode_rowid: 0,
            last_uploaded_minimaxcode_rev_ts: 0,
            last_uploaded_snapshot_ts: 0,
            last_uploaded_agent_quota_snapshot_ts: 0,
            last_uploaded_provider_quota_ts: 0,
            last_sync_at: 0,
        }
    }
}

/// 配置文件路径：~/.agenthud/sync.json
pub fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("sync.json"))
}

/// 读取同步配置；文件不存在返回默认空配置（不报错）。
pub fn load_sync_config() -> Result<SyncConfig, String> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(SyncConfig::default());
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取同步配置失败: {e}"))?;
    serde_json::from_str::<SyncConfig>(&data)
        .map_err(|e| format!("解析同步配置失败: {e}"))
}

/// 写入同步配置。
pub fn save_sync_config(cfg: &SyncConfig) -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = config_path()?;
    let data = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化同步配置失败: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("写入同步配置失败: {e}"))
}

// ===== HTTP 请求/响应结构（与服务端对齐）=====

#[derive(Debug, Deserialize)]
pub struct RegisterResponse {
    pub device_id: String,
    pub device_token: String,
    pub device_name: String,
}

#[derive(Debug, Deserialize)]
struct SyncResponse {
    accepted: usize,
    max_rowid: i64,
    /// 新版服务端返回；旧服务端不返回 → 用默认 0。保留字段以正确反序列化协议。
    #[serde(default)]
    #[allow(dead_code)]
    accepted_snapshots: usize,
    #[serde(default)]
    max_snapshot_ts: Option<i64>,
    #[serde(default)]
    accepted_agent_quota_snapshots: usize,
    #[serde(default)]
    max_agent_quota_snapshot_ts: Option<i64>,
    /// 服务端协议版本：2 = 支持多来源，3 = 支持 Agent 额度快照，
    /// 7 = 支持全量 provider 额度快照（label/plan_name/balance/message）。
    /// 旧服务端不返回 → 0。codex 上传前据此探测，防止旧服务端按
    /// (device_id, local_rowid) 撞键静默丢弃记录后游标仍推进。
    #[serde(default)]
    proto: u32,
}

#[derive(Debug, Clone, Serialize)]
struct SyncPayload {
    records: Vec<UsageRow>,
    last_rowid: Option<i64>,
    /// 额度快照（可选；旧服务端会忽略，向后兼容）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    snapshots: Vec<crate::quota_history::QuotaSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_snapshot_ts: Option<i64>,
    /// Codex / Claude / Cursor 额度快照（可选；旧服务端会忽略）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    agent_quota_snapshots: Vec<AgentQuotaSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_agent_quota_snapshot_ts: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_count: Option<i64>,
    /// 最后活跃时刻（最后一条明细的 started_at 毫秒；新版服务端返回，
    /// 旧服务端 / 只传过快照的设备为 null，前端显示 —）
    pub last_seen_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfoRaw {
    device_id: String,
    device_name: String,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    record_count: Option<i64>,
    /// 旧服务端不返回该字段：serde default 保证反序列化不失败（可缺省铁律）
    #[serde(default)]
    last_seen_at: Option<i64>,
}

/// /usage 返回的远端聚合（与服务端 UsageResult 对齐，字段名一致）。
/// 前端合并本地 + 远端时用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteUsage {
    pub from_ms: i64,
    pub to_ms: i64,
    pub overall: RemoteOverall,
    pub by_model: Vec<RemoteModelStat>,
    pub trend: Vec<RemoteTrendBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteOverall {
    #[serde(default)]
    pub requests: i64,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteModelStat {
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub requests: i64,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    /// 数据来源（新版服务端返回；旧服务端不返回时默认 zcode）
    #[serde(default = "default_source")]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTrendBucketModel {
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub requests: i64,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTrendBucket {
    pub label: String,
    #[serde(default)]
    pub by_model: Vec<RemoteTrendBucketModel>,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default)]
    pub requests: i64,
}

// 清理相关
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupStatus {
    pub total_records: i64,
    pub devices: Vec<DeviceInfoRaw>,
    pub auto_config: AutoCleanupConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AutoCleanupConfig {
    #[serde(default)]
    pub auto_enabled: bool,
    #[serde(default)]
    pub auto_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupResult {
    pub action: String,
    pub records_deleted: i64,
    #[serde(default)]
    pub devices_deleted: Option<i64>,
}

// ===== 注册 =====

/// 向服务器注册设备。验证 master_token 成功后返回完整配置（含 device_token）。
/// master_token 不写入配置文件。
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub server_url: String,
    pub master_token: String,
    pub device_name: String,
}

pub fn register_device(req: RegisterRequest) -> Result<SyncConfig, String> {
    let base = normalize_url(&req.server_url)?;
    #[derive(Serialize)]
    struct Body<'a> {
        master_token: &'a str,
        device_name: &'a str,
    }
    let resp: RegisterResponse = agent_with_timeouts(3, 10)
        .post(&format!("{base}/register"))
        .send_json(Body {
            master_token: req.master_token.trim(),
            device_name: req.device_name.trim(),
        })
        .map_err(map_http_err("注册设备"))?
        .into_json()
        .map_err(|e| format!("解析注册响应失败: {e}"))?;

    // 读取现有配置，保留游标等字段
    let mut cfg = load_sync_config()?;
    let server_changed = cfg.server_url != base;
    cfg.enabled = true;
    cfg.server_url = base;
    cfg.device_id = resp.device_id;
    cfg.device_name = resp.device_name;
    cfg.device_token = resp.device_token;
    // 注册不重置游标，沿用已有值（首次为 0，会全量上传）
    save_sync_config(&cfg)?;
    if server_changed {
        // 换服务端后 revision/pending 不能复用，重新从新服务端对齐
        crate::pricing_sync::reset_state();
    }
    Ok(cfg)
}

// ===== 增量上传 =====

/// 一次同步的结果（供 UI 显示）。
#[derive(Debug, Clone, Serialize)]
pub struct SyncOutcome {
    pub uploaded: usize,
    pub new_max_rowid: i64,
    pub last_sync_at: i64,
}

/// 服务端对 provider 额度快照（proto 7）支持状态的进程内缓存：
/// (最近一次探测时刻, 是否支持)。未知 / 距上次探测超过 30 分钟时返回 true
/// （本轮附加发送一次，按响应更新缓存）；确认不支持的窗口期内返回 false
/// （跳过附加，避免每轮把未确认的历史快照重发一遍的冗余流量；服务端升级
/// 后最多 30 分钟自动恢复）。仅进程内有效，重启后重探一次，代价可控。
static PROVIDER_SYNC_STATE: OnceLock<Mutex<Option<(Instant, bool)>>> = OnceLock::new();
const PROVIDER_PROTO_RECHECK: Duration = Duration::from_secs(30 * 60);

fn should_attach_provider_snapshots() -> bool {
    let cell = PROVIDER_SYNC_STATE.get_or_init(|| Mutex::new(None));
    let Ok(state) = cell.lock() else {
        return true; // 锁中毒时按未知处理：附加发送并由响应兜底
    };
    should_attach_with(*state, Instant::now())
}

/// 「本轮是否附加发送 provider 快照」的判定（纯函数，单测注入假时刻）：
/// - 从未探测（None）→ true（首轮探测）；
/// - 上次探测支持 → true（正常增量，每轮都带）；
/// - 上次探测不支持且未到重探窗口 → false（跳过附加，零冗余流量）；
/// - 上次探测不支持但已超过 30 分钟 → true（服务端可能已升级，重探）。
fn should_attach_with(probe: Option<(Instant, bool)>, now: Instant) -> bool {
    match probe {
        None => true,
        Some((at, supported)) => supported || now.duration_since(at) >= PROVIDER_PROTO_RECHECK,
    }
}

fn remember_provider_proto_supported(supported: bool) {
    let cell = PROVIDER_SYNC_STATE.get_or_init(|| Mutex::new(None));
    if let Ok(mut state) = cell.lock() {
        *state = Some((Instant::now(), supported));
    }
}

/// provider 快照游标的推进决策（纯函数，单测注入 mock proto 响应）：
/// 仅当「本轮确实附加发送了 provider 快照」且「服务端自报 proto >= 7」时
/// 推进到本批最大 ts；否则保留本地游标——旧服务端按 AGENT_QUOTA_WINDOWS
/// 白名单静默丢弃新 source，此时推进游标会让未确认的 provider 快照永久
/// 丢失（参考 codex 游标在 proto < 2 不推进的先例）。
fn provider_quota_cursor_after(
    attached: bool,
    proto: u32,
    provider_max: Option<i64>,
    current: i64,
) -> i64 {
    if attached && proto >= 7 {
        provider_max.unwrap_or(current)
    } else {
        current
    }
}

/// 执行一次增量上传（分阶段）：先 zcode 明细（含额度快照），再 codex / claude /
/// dsh / opencode / kimi 各派生库的明细。各来源游标相互独立：
/// - zcode 失败：整体返回 Err，游标不前进（下次重试），与原有行为一致；
/// - 其余来源失败：不阻断已成功来源的结果，错误仅记日志，游标停在最近
///   成功批次并随配置落盘（下次从断点续传）。
pub fn upload_incremental() -> Result<SyncOutcome, String> {
    let mut cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let base = cfg.server_url.clone();
    let token = cfg.device_token.clone();

    // 上传前先触发一次派生库增量导入（import_incremental 自带 30 秒节流，
    // 后台同步每轮至多真正导入一次，不会高频阻塞），让新会话的
    // session_id → project_key 映射尽早可用，缩小「无映射 → 明细连
    // session_id 都不带」的上传窗口。失败静默：未安装 ZCode / 派生库
    // 不可用时不影响同步本身。
    let _ = crate::zcode_sessions::import_incremental();

    // 读取待上传的快照（ts > 游标）。读失败不阻断明细同步。
    let mut pending_snaps = crate::quota_history::load_all()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.ts > cfg.last_uploaded_snapshot_ts)
        .collect::<Vec<_>>();
    let snap_max_ts = pending_snaps.iter().map(|s| s.ts).max();
    let mut snapshot_cursor_advanced = false;
    let mut pending_agent_quota_snapshots = agent_quota_history::load_all()
        .unwrap_or_default()
        .into_iter()
        .filter(|snapshot| snapshot.ts > cfg.last_uploaded_agent_quota_snapshot_ts)
        .collect::<Vec<_>>();
    let agent_quota_snapshot_max_ts = pending_agent_quota_snapshots
        .iter()
        .map(|snapshot| snapshot.ts)
        .max();
    let mut agent_quota_snapshot_cursor_advanced = false;

    // provider 额度快照（方案 B）：与 Agent 快照同请求字段混传（source 区分），
    // 但游标独立。仅在「服务端确认支持 proto 7」期间附加发送——旧服务端会
    // 按 AGENT_QUOTA_WINDOWS 静默丢弃新 source，若照发照推进游标，provider
    // 快照会永久丢失（参考 codex 在 proto < 2 不推进游标的先例）。支持状态
    // 来自上一次 /sync 响应的 proto 字段，未知或过期（30 分钟）时重探一轮：
    // 附带发送一次并按响应更新缓存；确认不支持期间跳过附加（零冗余流量），
    // 服务端升级后最长 30 分钟自动恢复上传。
    let mut pending_provider_quota_snapshots = Vec::new();
    let provider_quota_max_ts;
    if should_attach_provider_snapshots() {
        pending_provider_quota_snapshots = crate::provider_quota_history::load_all()
            .unwrap_or_default()
            .into_iter()
            .filter(|snapshot| snapshot.ts > cfg.last_uploaded_provider_quota_ts)
            .collect::<Vec<_>>();
    }
    provider_quota_max_ts = pending_provider_quota_snapshots
        .iter()
        .map(|snapshot| snapshot.ts)
        .max();

    // ===== 阶段一：zcode 明细（records 固定 source="zcode"）=====
    let mut since = cfg.last_uploaded_rowid;
    let mut total_uploaded = 0usize;
    const BATCH: usize = 500;
    let mut first_batch = true;
    // M5：派生库有映射且服务端 proto >= 5 时，zcode 明细附带会话/项目维度
    // （session_id / project_key / project_display）；否则按旧契约上传。
    let zcode_with_projects = zcode_projects_supported(&base, &token);

    loop {
        let records = query_zcode_rows_with_sessions(since, BATCH, zcode_with_projects)?;
        // 明细耗尽，且还有未发的快照 → 发一个空 records 的批次把快照送出
        let records_empty = records.is_empty();
        if records_empty
            && pending_snaps.is_empty()
            && pending_agent_quota_snapshots.is_empty()
            && pending_provider_quota_snapshots.is_empty()
        {
            break;
        }
        if records_empty && !first_batch {
            // 明细已发完，快照在首批已随带；无更多数据
            break;
        }
        // 本批最大 rowid（游标必须至少推进到这里，否则死循环）
        let batch_max = records.last().map(|r| r.local_rowid).unwrap_or(since);

        // 快照只在首批携带（一次性发完）；之后清空避免重复发。
        // provider 快照与 Agent 快照混在同一数组（source 区分），请求的
        // last_agent_quota_snapshot_ts 仍只取 Agent 侧最大值——服务端按请求
        // 值回传，Agent 游标语义保持不变；provider 游标由下方按 proto 独立推进。
        let snaps_to_send = if first_batch {
            std::mem::take(&mut pending_snaps)
        } else {
            Vec::new()
        };
        let last_snapshot_ts = if first_batch { snap_max_ts } else { None };
        let agent_quota_snapshots_to_send = if first_batch {
            let mut snapshots = std::mem::take(&mut pending_agent_quota_snapshots);
            snapshots.append(&mut pending_provider_quota_snapshots);
            snapshots
        } else {
            Vec::new()
        };
        let last_agent_quota_snapshot_ts = if first_batch {
            agent_quota_snapshot_max_ts
        } else {
            None
        };
        let provider_snapshots_attached = first_batch && provider_quota_max_ts.is_some();

        let payload = SyncPayload {
            records,
            last_rowid: Some(batch_max),
            snapshots: snaps_to_send,
            last_snapshot_ts,
            agent_quota_snapshots: agent_quota_snapshots_to_send,
            last_agent_quota_snapshot_ts,
        };
        // 防御对称：zcode 阶段一批次与派生来源一样走降级重试发送
        // （服务端自报 proto>=5 却 4xx 拒收新字段时剥字段重试，见函数注释）
        let resp: SyncResponse =
            post_zcode_batch_with_fallback(&base, &token, &payload, zcode_with_projects)?;

        total_uploaded += resp.accepted;
        total_uploaded += resp.accepted_agent_quota_snapshots;
        // 快照游标：取服务端返回值（新服务端）或本批最大 ts（旧服务端不回填）。
        if first_batch {
            if let Some(ts) = resp.max_snapshot_ts {
                cfg.last_uploaded_snapshot_ts = ts;
            } else if let Some(ts) = snap_max_ts {
                cfg.last_uploaded_snapshot_ts = ts;
            }
            snapshot_cursor_advanced = true;
        }
        // Agent 额度快照只有 proto >= 3 的服务端真正接收；旧服务端会忽略
        // 新字段，此时保留本地游标，升级服务端后自动补传。
        if first_batch && agent_quota_snapshot_max_ts.is_some() {
            if resp.proto >= 3 {
                if let Some(ts) = resp.max_agent_quota_snapshot_ts {
                    cfg.last_uploaded_agent_quota_snapshot_ts = ts;
                } else if let Some(ts) = agent_quota_snapshot_max_ts {
                    cfg.last_uploaded_agent_quota_snapshot_ts = ts;
                }
                agent_quota_snapshot_cursor_advanced = true;
            } else {
                eprintln!(
                    "[zbar-sync] 服务端版本过旧（不支持 Agent 额度快照），本地快照暂不推进上传游标"
                );
            }
        }
        // provider 额度快照游标：仅 proto >= 7 的服务端真正接收（旧服务端按
        // 白名单静默丢弃新 source）。附加发送后按响应 proto 决定推进与否
        // （决策逻辑抽在 provider_quota_cursor_after，单测覆盖），并更新
        // 进程内支持缓存（不支持的 30 分钟内不再附加以免重传风暴，服务端
        // 升级后自动重探恢复）。
        if first_batch && provider_snapshots_attached {
            let advanced = provider_quota_cursor_after(
                provider_snapshots_attached,
                resp.proto,
                provider_quota_max_ts,
                cfg.last_uploaded_provider_quota_ts,
            );
            if advanced != cfg.last_uploaded_provider_quota_ts {
                cfg.last_uploaded_provider_quota_ts = advanced;
                remember_provider_proto_supported(true);
            } else if resp.proto < 7 {
                remember_provider_proto_supported(false);
                eprintln!(
                    "[zbar-sync] 服务端未升级到 proto 7（当前 proto={}），provider 额度快照暂不推进上传游标，升级服务端后自动补传",
                    resp.proto
                );
            }
        }
        // 游标必须推进到本批最大 rowid（无论服务端是否接受，本地都已处理过这些记录）。
        // 取 max 防止服务端返回的旧游标回退。
        since = resp.max_rowid.max(batch_max);
        first_batch = false;
        // 明细空 + 快照发完 → 结束
        if records_empty {
            break;
        }
    }

    // ===== 阶段二：Codex 明细（records 固定 source="codex"，失败不阻断）=====
    let mut codex_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "codex") {
        Ok(n) => codex_uploaded = n,
        Err(e) => {
            // 游标已随每批成功推进（见函数内），稍后随配置落盘，下次从断点续传
            eprintln!("[zbar-sync] Codex 增量上传失败（下次重试）: {e}");
        }
    }

    // ===== 阶段三：Claude 明细（records 固定 source="claude"，失败不阻断）=====
    let mut claude_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "claude") {
        Ok(n) => claude_uploaded = n,
        Err(e) => {
            eprintln!("[zbar-sync] Claude 增量上传失败（下次重试）: {e}");
        }
    }

    // ===== 阶段四：DSH 明细（本地采集型，与 zcode/kimicode 同属可上传来源）=====
    let mut dsh_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "dsh") {
        Ok(n) => dsh_uploaded = n,
        Err(e) => {
            eprintln!("[zbar-sync] DSH 增量上传失败（下次重试）: {e}");
        }
    }

    // ===== 阶段五：OpenCode 明细（本地采集型，派生库 ~/.agenthud/opencode.sqlite）=====
    let mut opencode_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "opencode") {
        Ok(n) => opencode_uploaded = n,
        Err(e) => {
            eprintln!("[zbar-sync] OpenCode 增量上传失败（下次重试）: {e}");
        }
    }

    // ===== 阶段六：Kimi 明细（本地采集型，派生库 ~/.agenthud/kimi.sqlite）=====
    // 注意：这是**用量明细**通道；Kimi 的额度快照走 agent_quota_snapshots，
    // 两者游标与 payload 字段都不同，互不影响。
    let mut kimi_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "kimi") {
        Ok(n) => kimi_uploaded = n,
        Err(e) => {
            eprintln!("[zbar-sync] Kimi 增量上传失败（下次重试）: {e}");
        }
    }

    // ===== 阶段七：MiniMax Code 明细（本地采集型，派生库 ~/.agenthud/minimaxcode.sqlite）=====
    let mut minimaxcode_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "minimaxcode") {
        Ok(n) => minimaxcode_uploaded = n,
        Err(e) => {
            eprintln!("[zbar-sync] MiniMax Code 增量上传失败（下次重试）: {e}");
        }
    }

    let _ = snapshot_cursor_advanced; // 标记已用，避免未读警告
    let _ = agent_quota_snapshot_cursor_advanced;
    let now = chrono::Local::now().timestamp_millis();
    cfg.last_uploaded_rowid = since;
    cfg.last_sync_at = now;
    // 落盘竞态防护（与本轮开头的 load 之间隔着多批网络上传，可达 15s+）：
    // 期间用户可能在 UI 上「暂停同步」或「断开连接」——这些操作会立即写盘
    // （enabled=false / 清空凭证）。若把开头读到的旧快照原样全量写回，会把
    // 用户的暂停/断开静默复活（断开完全失效）。故落盘前重读配置并按
    // merge_config_for_save 的决策合并：
    // - device_token 与本轮开头不同（断开/重注册会换凭证）→ 放弃本轮落盘；
    //   本轮推进的游标丢弃，下轮按盘上旧游标重传，服务端按
    //   (device_id, source, local_rowid) 主键 ON CONFLICT 幂等去重，无副作用。
    // - token 相同（含「仅暂停」：token 不变）→ 以最新配置为底，只覆盖本轮
    //   推进的游标与 last_sync_at；enabled / mode / interval / server_url /
    //   device_id / device_name / device_token 一律取最新值，不回滚用户操作。
    // 重读失败同样放弃落盘：宁可下轮重传，不可回滚用户的暂停/断开。
    match load_sync_config() {
        Ok(latest) => match merge_config_for_save(&token, &cfg, &latest) {
            Some(merged) => save_sync_config(&merged)?,
            None => eprintln!(
                "[zbar-sync] 同步期间凭证已变更（断开/重注册），放弃本轮游标落盘；服务端按主键幂等去重，下轮重传无副作用"
            ),
        },
        Err(e) => eprintln!(
            "[zbar-sync] 落盘前重读配置失败，放弃本轮落盘（下轮重传，服务端幂等兜底）: {e}"
        ),
    }
    // 价格表同步失败不影响用量上传；pending 保留，下一轮重试
    if let Err(e) = crate::pricing_sync::sync_with_server(&base, &token) {
        eprintln!("[zbar-sync] 价格表同步失败（下一轮重试）: {e}");
    }

    Ok(SyncOutcome {
        uploaded: total_uploaded
            + codex_uploaded
            + claude_uploaded
            + dsh_uploaded
            + opencode_uploaded
            + kimi_uploaded
            + minimaxcode_uploaded,
        new_max_rowid: since,
        last_sync_at: now,
    })
}

/// 上传轮结束时「安全落盘」的合并决策（纯函数，单测锁定）：
/// - `started_token` = 本轮开头读到的 device_token；`ran` = 本轮跑完的内存
///   配置（含推进后的游标与 last_sync_at）；`latest` = 落盘前重读的最新配置。
/// - token 不同（用户断开/重注册换凭证）→ None：放弃落盘。本轮游标推进
///   丢弃，下轮按盘上旧游标重传，服务端按主键 ON CONFLICT 幂等去重。
/// - token 相同 → Some：以 latest 为底（enabled / mode / interval_seconds /
///   server_url / device_id / device_name / device_token 全取最新，不回滚
///   用户在 UI 上的暂停等操作），仅覆盖本轮推进的字段：各来源明细游标
///   （last_uploaded_*_rowid / *_rev_ts / codex 模型回填双游标）、三类快照
///   游标与 last_sync_at。新增来源的游标若忘在这里覆盖，会被
///   `落盘合并取最新身份与本轮游标` 单测的字段清单提醒补齐。
fn merge_config_for_save(
    started_token: &str,
    ran: &SyncConfig,
    latest: &SyncConfig,
) -> Option<SyncConfig> {
    if latest.device_token != started_token {
        return None;
    }
    let mut merged = latest.clone();
    merged.last_uploaded_rowid = ran.last_uploaded_rowid;
    merged.last_uploaded_codex_rowid = ran.last_uploaded_codex_rowid;
    merged.last_uploaded_claude_rowid = ran.last_uploaded_claude_rowid;
    merged.last_uploaded_claude_rev_ts = ran.last_uploaded_claude_rev_ts;
    merged.last_uploaded_codex_model_rev_ts = ran.last_uploaded_codex_model_rev_ts;
    merged.last_uploaded_codex_model_rev_id = ran.last_uploaded_codex_model_rev_id;
    merged.last_uploaded_dsh_rowid = ran.last_uploaded_dsh_rowid;
    merged.last_uploaded_dsh_rev_ts = ran.last_uploaded_dsh_rev_ts;
    merged.last_uploaded_opencode_rowid = ran.last_uploaded_opencode_rowid;
    merged.last_uploaded_opencode_rev_ts = ran.last_uploaded_opencode_rev_ts;
    merged.last_uploaded_kimi_rowid = ran.last_uploaded_kimi_rowid;
    merged.last_uploaded_kimi_rev_ts = ran.last_uploaded_kimi_rev_ts;
    merged.last_uploaded_minimaxcode_rowid = ran.last_uploaded_minimaxcode_rowid;
    merged.last_uploaded_minimaxcode_rev_ts = ran.last_uploaded_minimaxcode_rev_ts;
    merged.last_uploaded_snapshot_ts = ran.last_uploaded_snapshot_ts;
    merged.last_uploaded_agent_quota_snapshot_ts = ran.last_uploaded_agent_quota_snapshot_ts;
    merged.last_uploaded_provider_quota_ts = ran.last_uploaded_provider_quota_ts;
    merged.last_sync_at = ran.last_sync_at;
    Some(merged)
}

/// 构造派生来源的上传批次 payload。with_projects=false 时剥掉每条记录的
/// 会话/项目维度字段（proto 4 降级：服务端未升级时按旧契约上传，
/// 字段缺失序列化端即不发，服务端落 NULL，与旧行为完全一致）。
fn build_derived_payload(
    records: &[UsageRow],
    last_rowid: Option<i64>,
    with_projects: bool,
) -> SyncPayload {
    let records = if with_projects {
        records.to_vec()
    } else {
        records
            .iter()
            .map(|row| {
                let mut row = row.clone();
                row.session_id = None;
                row.project_key = None;
                row.project_display = None;
                row
            })
            .collect()
    };
    SyncPayload {
        records,
        last_rowid,
        snapshots: Vec::new(),
        last_snapshot_ts: None,
        agent_quota_snapshots: Vec::new(),
        last_agent_quota_snapshot_ts: None,
    }
}

// ===== zcode 明细的会话/项目维度增强（M5）=====

/// 判断 zcode 明细上传是否附带会话/项目维度（proto 5）：
/// 派生库有映射 且 服务端自报 proto >= 5。
/// 探测用空批次请求（无写入副作用，与派生来源的协议探测同款）；网络
/// 失败/派生库不可用时返回 false，上传退回原契约（三字段不发），不影响
/// 原有错误路径。
fn zcode_projects_supported(base: &str, token: &str) -> bool {
    if !crate::zcode_sessions::has_project_mapping() {
        return false; // 派生库无映射：无可附带维度，跳过协议探测
    }
    let probe: SyncResponse = match agent_with_timeouts(3, 15)
        .post(&format!("{base}/sync"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(&SyncPayload {
            records: Vec::new(),
            last_rowid: None,
            snapshots: Vec::new(),
            last_snapshot_ts: None,
            agent_quota_snapshots: Vec::new(),
            last_agent_quota_snapshot_ts: None,
        })
        .and_then(|resp| resp.into_json::<SyncResponse>().map_err(|e| e.into()))
    {
        Ok(resp) => resp,
        Err(e) => {
            // 探测失败不阻断同步：本轮不带新字段（原行为），下轮重探
            eprintln!("[zbar-sync] zcode 项目维度协议探测失败（本轮按旧契约上传）: {e}");
            return false;
        }
    };
    if probe.proto >= 5 {
        true
    } else {
        eprintln!(
            "[zbar-sync] 服务端未升级到 proto 5（当前 proto={}），zcode 明细暂不携带项目维度字段",
            probe.proto
        );
        false
    }
}

/// 查询主库 rowid > since 的 zcode 明细（与 db::query_since 同 SQL、同
/// rowid 游标口径，多带 session_id 列与 proto 8 的耗时/首字列），并按
/// zcode_sessions 派生库的 session_id → (project_key, cwd) 映射填充三字段。
/// with_projects=false、主库无 session_id 列或映射查不到该会话时，三字段
/// 保持 None；耗时/首字无列恒 None（禁止时间戳差估算）。
fn query_zcode_rows_with_sessions(
    since: i64,
    limit: usize,
    with_projects: bool,
) -> Result<Vec<UsageRow>, String> {
    let conn = db::open_db()?;
    // 老版本主库无 session_id 列：直接走原查询（行为逐字节一致）
    if !db::has_column(&conn, "model_usage", "session_id") {
        return db::query_since(since, limit);
    }
    let map = if with_projects {
        crate::zcode_sessions::session_project_map()
    } else {
        HashMap::new()
    };
    query_zcode_rows_with_sessions_at(&conn, since, limit, &map)
}

/// `query_zcode_rows_with_sessions` 的纯查询内核（连接 + 项目映射注入版，
/// 供单元测试锁定 SELECT 动态拼列 ↔ 数字下标映射）。
fn query_zcode_rows_with_sessions_at(
    conn: &rusqlite::Connection,
    since: i64,
    limit: usize,
    project_map: &HashMap<String, (String, String)>,
) -> Result<Vec<UsageRow>, String> {
    // proto 8：耗时/首字按列存在性带出（真实值 > 0 才给，无列恒 None，
    // 不做时间戳差估算；口径与 db::query_since 一致）
    let (duration, ttft) = db::speed_columns(conn);
    let sql = format!(
        "SELECT rowid, started_at, model_id, provider_id,
            COALESCE(input_tokens,0), COALESCE(output_tokens,0),
            COALESCE(cache_read_input_tokens,0), COALESCE(cache_creation_input_tokens,0),
            COALESCE(reasoning_tokens,0), COALESCE(computed_total_tokens,0),
            session_id, {duration}, {ttft}
     FROM model_usage
     WHERE rowid > ?1
     ORDER BY rowid ASC
     LIMIT ?2"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 zcode 增量查询失败: {e}"))?;
    // 中间结构：(UsageRow, 该行的 session_id)；session_id 不进 UsageRow
    // （db.rs 的同步契约结构，禁止改动），回填完成后丢弃
    let rows = stmt
        .query_map(rusqlite::params![since, limit as i64], |row| {
            Ok((
                UsageRow {
                    local_rowid: row.get(0)?,
                    started_at: row.get(1)?,
                    model_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    provider_id: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    input_tokens: row.get(4)?,
                    output_tokens: row.get(5)?,
                    cache_read_input_tokens: row.get(6)?,
                    cache_creation_input_tokens: row.get(7)?,
                    reasoning_tokens: row.get(8)?,
                    computed_total_tokens: row.get(9)?,
                    // 本库（zcode）的行固定标记为 zcode 来源
                    source: "zcode".into(),
                    session_id: None,
                    project_key: None,
                    project_display: None,
                    duration_ms: row.get(11)?,
                    time_to_first_token_ms: row.get(12)?,
                },
                row.get::<_, Option<String>>(10)?,
            ))
        })
        .map_err(|e| format!("读取 zcode 增量记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 zcode 增量记录失败: {e}"))?;

    // 逐行回填维度：查到映射才附带三字段（绑定出现，查不到保持 None）
    Ok(rows
        .into_iter()
        .map(|(mut row, session_id)| {
            if let Some((key, display)) = session_id.as_ref().and_then(|id| project_map.get(id)) {
                row.session_id = session_id.clone();
                row.project_key = Some(key.clone());
                row.project_display = Some(display.clone());
            }
            row
        })
        .collect())
}

/// 发送一批明细到 /sync（抽出公共发送逻辑，供 proto 5 降级重试复用）。
fn post_sync_batch(
    base: &str,
    token: &str,
    payload: &SyncPayload,
) -> Result<SyncResponse, String> {
    agent_with_timeouts(3, 15)
        .post(&format!("{base}/sync"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(payload)
        .map_err(map_http_err("上传数据"))?
        .into_json()
        .map_err(|e| format!("解析上传响应失败: {e}"))
}

/// 错误串是否为 HTTP 4xx。map_http_err 生成的格式固定为「xx失败: HTTP {code}: body」，
/// 字符串匹配足以覆盖降级重试场景的判定（不区分 4xx 具体码）。
fn is_http_4xx(err: &str) -> bool {
    err.contains("HTTP 4")
}

/// 带降级重试的批次上传：正常路径按探测结果决定是否携带项目维度字段
/// （proto 5）；若服务端自报 proto>=5 但处理新字段时返回 4xx（实现不一致
/// 的中间版本），剥掉新字段按 proto 4 契约重试一次。降级事件记日志不弹窗，
/// 重试仍失败才向调用方报错（游标不推进，下次重试）。
fn post_batch_with_fallback(
    base: &str,
    token: &str,
    records: &[UsageRow],
    last_rowid: Option<i64>,
    with_projects: bool,
) -> Result<SyncResponse, String> {
    let payload = build_derived_payload(records, last_rowid, with_projects);
    match post_sync_batch(base, token, &payload) {
        Ok(resp) => Ok(resp),
        Err(e) if with_projects && is_http_4xx(&e) => {
            eprintln!(
                "[zbar-sync] proto 5 上传被服务端拒绝（{e}），本批降级为 proto 4 重试（不含项目维度字段）"
            );
            let fallback = build_derived_payload(records, last_rowid, false);
            post_sync_batch(base, token, &fallback)
        }
        Err(e) => Err(e),
    }
}

/// zcode 阶段一批次的降级重试发送，与派生来源的 post_batch_with_fallback
/// 防御对称：服务端自报 proto>=5 却在处理新字段时返回 4xx（实现不一致的
/// 中间版本）时，剥掉明细的三字段按 proto 4 契约重试一次，重试仍失败才
/// 返回 Err。若不降级，单批 4xx 会让 upload_incremental 提前返回——阶段
/// 二/三不再执行、游标不推进，而协议探测每轮仍成功，同步将永久卡死。
/// zcode 批次额外携带额度快照字段，与派生来源的 payload 不同构，无法
/// 直接复用 build_derived_payload，故单独实现（重试时仅剥明细三字段，
/// 快照原样保留）。with_projects=false 时批次本就不带三字段，4xx 不重试。
fn post_zcode_batch_with_fallback(
    base: &str,
    token: &str,
    payload: &SyncPayload,
    with_projects: bool,
) -> Result<SyncResponse, String> {
    match post_sync_batch(base, token, payload) {
        Ok(resp) => Ok(resp),
        Err(e) if with_projects && is_http_4xx(&e) => {
            eprintln!(
                "[zbar-sync] proto 5 上传被服务端拒绝（{e}），本批降级为 proto 4 重试（不含项目维度字段）"
            );
            let mut fallback = payload.clone();
            fallback.records = fallback
                .records
                .drain(..)
                .map(|mut row| {
                    row.session_id = None;
                    row.project_key = None;
                    row.project_display = None;
                    row
                })
                .collect();
            post_sync_batch(base, token, &fallback)
        }
        Err(e) => Err(e),
    }
}

/// 一个批次的发送回执：只留批循环需要的两个数（服务端接受的条数 + 服务端回传的
/// 游标）。抽成独立结构是为了让批循环变成可注入的纯逻辑——单测用假查询/假发送
/// 覆盖「游标推进」的行为，不碰网络与真实服务端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BatchAck {
    accepted: usize,
    max_rowid: i64,
}

/// 明细增量批循环（**游标推进逻辑的唯一定义处**，各来源共用）：
/// 反复取 `rowid > since` 的下一批直到取空；每批成功发送后把游标推进到
/// `max(服务端回传游标, 本批最大 rowid)`——取 max 既保证不死循环（本批最大
/// rowid 是下界），也不会被服务端回传的旧游标拖回退。每批推进都立刻通过
/// `on_advance` 回写配置游标，故中途失败时已成功的批次不丢断点（调用方随
/// 配置落盘，下次从断点续传）；失败时直接返回 Err，剩余批次留给下一轮。
///
/// query(since, limit) / send(records, batch_max) 由调用方按来源注入。
fn pump_source_batches<Q, S, A>(
    cursor: i64,
    batch: usize,
    mut query: Q,
    mut send: S,
    mut on_advance: A,
) -> Result<(usize, i64), String>
where
    Q: FnMut(i64, usize) -> Result<Vec<UsageRow>, String>,
    S: FnMut(&[UsageRow], i64) -> Result<BatchAck, String>,
    A: FnMut(i64),
{
    let mut since = cursor;
    let mut total = 0usize;
    loop {
        let records = query(since, batch)?;
        if records.is_empty() {
            break;
        }
        // 本批最大 rowid（游标必须至少推进到这里，否则死循环）
        let batch_max = records.last().map(|r| r.local_rowid).unwrap_or(since);
        let ack = send(&records, batch_max)?;
        total += ack.accepted;
        since = ack.max_rowid.max(batch_max);
        on_advance(since);
    }
    Ok((total, since))
}

/// 修订补传扫描（claude / dsh / opencode / kimi / minimaxcode 同构，
/// codex 的模型名回填另有一套）：
/// 本来源存在「行 id 不变、内容被覆盖」的修订——普通 `id > 游标` 查询永远选不出它。
/// 这里按修订时间游标 `updated_at > since_ts` 分批选出这些行重传，服务端以
/// 「总量更大者胜」的 upsert 覆盖修正（见 server/db.py）。`after_id` 逐批推进，
/// 防止同一批记录被反复选出。返回 (成功上传条数, 本次扫描水位)。
///
/// 水位取**扫描开始时刻**（而不是扫描中读到的最大 updated_at）：扫描期间新落盘
/// 的修订时间更大，留给下一轮自然选出，避免一边扫一边追平永不停机。
fn sweep_source_revisions<Q, S>(
    since_ts: i64,
    batch: usize,
    mut query: Q,
    mut send: S,
) -> Result<(usize, i64), String>
where
    Q: FnMut(i64, i64, usize) -> Result<Vec<UsageRow>, String>,
    S: FnMut(&[UsageRow]) -> Result<usize, String>,
{
    let sweep_watermark = chrono::Local::now().timestamp_millis();
    let mut after_id = 0i64;
    let mut total = 0usize;
    loop {
        let records = query(since_ts, after_id, batch)?;
        if records.is_empty() {
            break;
        }
        after_id = records.last().map(|r| r.local_rowid).unwrap_or(after_id);
        total += send(&records)?;
    }
    Ok((total, sweep_watermark))
}

/// 上传派生来源（本地采集型导入库）的增量（id > 对应游标，循环分批）。
/// 查询前由各模块自行增量导入（原始 jsonl / 本地库 → 派生 sqlite）。各条路径
/// 完全同构，仅数据源与游标字段不同，故按 source 参数泛化：
/// - "codex" → crate::codex + last_uploaded_codex_rowid（另有模型名回填通道）
/// - "claude" → crate::claude + last_uploaded_claude_rowid
/// - "dsh" → crate::dsh + last_uploaded_dsh_rowid
/// - "opencode" → crate::opencode + last_uploaded_opencode_rowid
/// - "kimi" → crate::kimi + last_uploaded_kimi_rowid（与额度快照通道无关）
/// - "minimaxcode" → crate::minimaxcode + last_uploaded_minimaxcode_rowid
/// 未安装对应 CLI 时静默返回 0（不算错误，避免后台同步每轮刷错误日志）。
/// 首批上传前先探测服务端协议版本（proto ≥ 2 才支持多来源）：
/// 旧服务端无 source 列，记录会按 (device_id, local_rowid) 撞键静默丢弃
/// 且游标推进 → 数据永久丢失，故版本不足时不推进游标直接报错，
/// 升级服务端后自动恢复上传。成功的每批都推进游标（部分失败也保留断点，
/// 由调用方负责落盘）。返回本次成功上传的条数。
fn upload_derived_source_incremental(
    cfg: &mut SyncConfig,
    base: &str,
    token: &str,
    source: &str,
) -> Result<usize, String> {
    let (dir_exists, has_pending, cursor): (bool, bool, i64) = match source {
        "codex" => {
            let dir = crate::codex::sessions_dir().is_ok();
            let pending = dir
                && (!crate::codex::query_since(cfg.last_uploaded_codex_rowid, 1)?.is_empty()
                    || !crate::codex::query_model_revised_since(
                        cfg.last_uploaded_codex_model_rev_ts,
                        cfg.last_uploaded_codex_model_rev_id,
                        1,
                    )?
                    .is_empty());
            (dir, pending, cfg.last_uploaded_codex_rowid)
        }
        "claude" => {
            let dir = crate::claude::projects_dir().is_ok();
            let pending = dir
                && !crate::claude::query_since(cfg.last_uploaded_claude_rowid, 1)?.is_empty();
            (dir, pending, cfg.last_uploaded_claude_rowid)
        }
        "dsh" => {
            // 会话日志目录存在即视为已安装（dsh 模块无「必须报错」的目录入口，
            // 与 claude::projects_dir() 的 Result 语义对齐：目录不存在就不上传）
            let dir = crate::dsh::has_local_data();
            let pending = dir
                && (!crate::dsh::query_since(cfg.last_uploaded_dsh_rowid, 1)?.is_empty()
                    || !crate::dsh::query_revised_since(cfg.last_uploaded_dsh_rev_ts, 0, 1)?
                        .is_empty());
            (dir, pending, cfg.last_uploaded_dsh_rowid)
        }
        "opencode" => {
            // OpenCode 的「已安装」判定 = 本地库里存在 v1/v2 消息表
            // （opencode::has_local_data，比只看库文件是否存在更严：空库/改名后
            // 的新版库没有消息表，不该假装有数据）
            let dir = crate::opencode::has_local_data();
            let pending = dir
                && (!crate::opencode::query_since(cfg.last_uploaded_opencode_rowid, 1)?
                    .is_empty()
                    || !crate::opencode::query_revised_since(
                        cfg.last_uploaded_opencode_rev_ts,
                        0,
                        1,
                    )?
                    .is_empty());
            (dir, pending, cfg.last_uploaded_opencode_rowid)
        }
        "kimi" => {
            let dir = crate::kimi::sessions_dir().is_ok();
            let pending = dir
                && (!crate::kimi::query_since(cfg.last_uploaded_kimi_rowid, 1)?.is_empty()
                    || !crate::kimi::query_revised_since(cfg.last_uploaded_kimi_rev_ts, 0, 1)?
                        .is_empty());
            (dir, pending, cfg.last_uploaded_kimi_rowid)
        }
        "minimaxcode" => {
            // MiniMax Code 的「已安装」判定 = 本地 runtime-state 库存在且有用量表
            //（minimaxcode::has_local_data，比只看库文件是否存在更严：空库/改名后
            // 的新版库没有用量表，不该假装有数据）
            let dir = crate::minimaxcode::has_local_data();
            let pending = dir
                && (!crate::minimaxcode::query_since(cfg.last_uploaded_minimaxcode_rowid, 1)?
                    .is_empty()
                    || !crate::minimaxcode::query_revised_since(
                        cfg.last_uploaded_minimaxcode_rev_ts,
                        0,
                        1,
                    )?
                    .is_empty());
            (dir, pending, cfg.last_uploaded_minimaxcode_rowid)
        }
        _ => return Err(format!("未知数据来源: {source}")),
    };
    // 未安装对应 CLI：无会话目录，静默跳过（不算错误，避免后台同步每轮刷日志）
    if !dir_exists {
        return Ok(0);
    }
    if !has_pending {
        return Ok(0); // 无待上传数据，跳过（也免去协议探测请求）
    }

    // 协议探测：空批次请求无任何写入副作用，仅读回服务端能力标记
    let probe: SyncResponse = agent_with_timeouts(3, 15)
        .post(&format!("{base}/sync"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(&SyncPayload {
            records: Vec::new(),
            last_rowid: None,
            snapshots: Vec::new(),
            last_snapshot_ts: None,
            agent_quota_snapshots: Vec::new(),
            last_agent_quota_snapshot_ts: None,
        })
        .map_err(map_http_err("探测服务端协议"))?
        .into_json()
        .map_err(|e| format!("解析协议探测响应失败: {e}"))?;
    if probe.proto < 2 {
        return Err(format!(
            "服务端版本过旧（不支持多来源同步），{source} 数据暂不上传，请升级服务端 zbar-sync 后重试"
        ));
    }
    // proto 5 = 服务端支持明细的会话/项目维度（session_id / project_key /
    // project_display）。未升级的服务端会忽略新字段（落 NULL），不报错，
    // 但为避免字段被静默丢弃，这里主动按旧契约剥掉字段上传并记日志；
    // 服务端升级后下一轮同步自动携带完整维度（游标已推进的旧行不补传，
    // 与 proto 2/3/4 的能力协商惯例一致）。
    let with_projects = probe.proto >= 5;
    if !with_projects {
        eprintln!(
            "[zbar-sync] 服务端未升级到 proto 5（当前 proto={}），{source} 明细暂不携带项目维度字段",
            probe.proto
        );
    }

    let mut total = 0usize;
    const BATCH: usize = 500;
    // 明细增量：批循环与游标推进抽在 pump_source_batches（可单测），这里只按
    // 来源注入查询与游标字段。
    let (batch_uploaded, _since) = pump_source_batches(
        cursor,
        BATCH,
        |since, limit| match source {
            "codex" => crate::codex::query_since(since, limit),
            "claude" => crate::claude::query_since(since, limit),
            "dsh" => crate::dsh::query_since(since, limit),
            "opencode" => crate::opencode::query_since(since, limit),
            "kimi" => crate::kimi::query_since(since, limit),
            "minimaxcode" => crate::minimaxcode::query_since(since, limit),
            _ => unreachable!("来源已在上文校验"),
        },
        |records, batch_max| {
            let resp =
                post_batch_with_fallback(&base, &token, records, Some(batch_max), with_projects)?;
            Ok(BatchAck {
                accepted: resp.accepted,
                max_rowid: resp.max_rowid,
            })
        },
        // 每批成功就回写游标：中途失败时已成功的批次不丢断点（调用方落盘）
        |advanced| match source {
            "codex" => cfg.last_uploaded_codex_rowid = advanced,
            "claude" => cfg.last_uploaded_claude_rowid = advanced,
            "dsh" => cfg.last_uploaded_dsh_rowid = advanced,
            "opencode" => cfg.last_uploaded_opencode_rowid = advanced,
            "kimi" => cfg.last_uploaded_kimi_rowid = advanced,
            "minimaxcode" => cfg.last_uploaded_minimaxcode_rowid = advanced,
            _ => unreachable!("来源已在上文校验"),
        },
    )?;
    total += batch_uploaded;

    // Codex 模型名回填不会产生新的 id，普通 rowid 游标选不出这些旧记录。
    // 参照 Claude 修订补传：不携带 last_rowid，只让服务端按同一主键做修正，
    // 同时独立维护修订时间游标，避免影响普通增量上传。
    if source == "codex" {
        let mut revision_ts = cfg.last_uploaded_codex_model_rev_ts;
        let mut revision_id = cfg.last_uploaded_codex_model_rev_id;
        loop {
            let revised =
                crate::codex::query_model_revised_since(revision_ts, revision_id, BATCH)?;
            if revised.is_empty() {
                break;
            }
            let last = revised.last().expect("修订记录非空");
            revision_ts = last.revision_at;
            revision_id = last.usage.local_rowid;
            let records = revised.into_iter().map(|row| row.usage).collect::<Vec<_>>();
            let resp = post_batch_with_fallback(&base, &token, &records, None, with_projects)?;
            total += resp.accepted;
        }
        // 补传期间新产生且修订序号更大的记录留给下一轮，避免漏掉并发写入。
        cfg.last_uploaded_codex_model_rev_ts = revision_ts;
        cfg.last_uploaded_codex_model_rev_id = revision_id;
    }

    // Claude / DSH / OpenCode / Kimi / MiniMax Code 修订补传（扫描逻辑与水位口径见
    // sweep_source_revisions）：这些来源都会「行 id 不变、内容被覆盖」——
    // Claude 会话流式落盘、DSH 同一步先小后大、OpenCode 指纹变化整会话重导
    // 时存活行原地 upsert、Kimi wire.jsonl 被重写重放、MiniMax Code 整表重扫
    // 时存活行原地 upsert——上面的 id 游标永远选不出
    // 它们，必须按 updated_at 重传，由服务端「总量更大者胜」upsert 覆盖修正。
    // 旧服务端（无覆盖守卫）会忽略重传——退化为旧行为（远端保留中间值），
    // 无数据损坏，故无需协议协商。
    if matches!(
        source,
        "claude" | "dsh" | "opencode" | "kimi" | "minimaxcode"
    ) {
        let rev_cursor = match source {
            "claude" => cfg.last_uploaded_claude_rev_ts,
            "dsh" => cfg.last_uploaded_dsh_rev_ts,
            "opencode" => cfg.last_uploaded_opencode_rev_ts,
            "kimi" => cfg.last_uploaded_kimi_rev_ts,
            "minimaxcode" => cfg.last_uploaded_minimaxcode_rev_ts,
            _ => unreachable!("来源已在上文校验"),
        };
        let (rev_uploaded, sweep_watermark) = sweep_source_revisions(
            rev_cursor,
            BATCH,
            |since_ts, after_id, limit| match source {
                "claude" => crate::claude::query_revised_since(since_ts, after_id, limit),
                "dsh" => crate::dsh::query_revised_since(since_ts, after_id, limit),
                "opencode" => crate::opencode::query_revised_since(since_ts, after_id, limit),
                "kimi" => crate::kimi::query_revised_since(since_ts, after_id, limit),
                "minimaxcode" => {
                    crate::minimaxcode::query_revised_since(since_ts, after_id, limit)
                }
                _ => unreachable!("来源已在上文校验"),
            },
            |records| {
                let resp = post_batch_with_fallback(&base, &token, records, None, with_projects)?;
                Ok(resp.accepted)
            },
        )?;
        total += rev_uploaded;
        // 推进到补传开始前的水位：补传期间新发生的修订（updated_at 更大）
        // 下一轮自然选出
        match source {
            "claude" => cfg.last_uploaded_claude_rev_ts = sweep_watermark,
            "dsh" => cfg.last_uploaded_dsh_rev_ts = sweep_watermark,
            "opencode" => cfg.last_uploaded_opencode_rev_ts = sweep_watermark,
            "kimi" => cfg.last_uploaded_kimi_rev_ts = sweep_watermark,
            "minimaxcode" => cfg.last_uploaded_minimaxcode_rev_ts = sweep_watermark,
            _ => unreachable!("来源已在上文校验"),
        }
    }
    Ok(total)
}

// ===== 远端查询 =====

#[derive(Debug, Deserialize)]
pub struct RemoteUsageRequest {
    pub from_ms: i64,
    pub to_ms: i64,
    /// "hour" 或 "day"
    pub bucket: String,
    /// 排除本机设备（避免重复计算本机数据），逗号分隔多个 device_id。
    /// 为空表示不排除（查全部）。
    #[serde(default)]
    pub exclude_device: String,
    /// 仅查询这些设备（逗号分隔），优先级高于 exclude_device。
    /// 用于设备筛选器选具体设备时只查它。
    #[serde(default)]
    pub devices: String,
    /// 数据来源过滤："zcode" | "codex" | "claude"，空 = 全部来源。
    #[serde(default)]
    pub source: String,
}

/// 拉取远端聚合数据。
/// - devices 非空：只查指定设备
/// - 否则 exclude_device 非空：排除本机，拿其他设备（与本机合并）
/// - 都空：查全部
pub fn fetch_remote_usage(req: RemoteUsageRequest) -> Result<RemoteUsage, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;

    let mut url = format!(
        "{base}/usage?from_ms={}&to_ms={}&bucket={}",
        req.from_ms, req.to_ms, req.bucket
    );
    if !req.source.is_empty() {
        url.push_str(&format!("&source={}", req.source));
    }
    if !req.devices.is_empty() {
        url.push_str(&format!("&devices={}", req.devices));
    } else if !req.exclude_device.is_empty() {
        url.push_str(&format!("&exclude_device={}", req.exclude_device));
    }

    let resp: RemoteUsage = agent_with_timeouts(2, 5)
        .get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(map_http_err("查询远端数据"))?
        .into_json()
        .map_err(|e| format!("解析远端数据失败: {e}"))?;
    Ok(resp)
}

// ===== 远端逐条请求明细（请求日志页「全部设备」口径）=====

/// GET /usage_records 返回的单条明细（字段与服务端 db.query_usage_records
/// 的行一致）。proto 8 起服务端回传耗时/首字（duration_ms /
/// time_to_first_token_ms，真实值才非 None；旧服务端不返回该键 → None，
/// serde default 兜底）。速度列仍不回传——行级速度原料（first_token_at /
/// completed_at）远端不上传，由前端 speedOf 用耗时+首字+output 推导。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteUsageRecord {
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub local_rowid: i64,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_input_tokens: i64,
    #[serde(default)]
    pub cache_creation_input_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub computed_total_tokens: i64,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub project_key: Option<String>,
    #[serde(default)]
    pub project_display: Option<String>,
    /// 请求耗时毫秒（proto 8 起服务端回传；旧服务端不返回该键 → None）
    #[serde(default)]
    pub duration_ms: Option<f64>,
    /// 首字毫秒（proto 8 起服务端回传；旧服务端不返回该键 → None）
    #[serde(default)]
    pub time_to_first_token_ms: Option<f64>,
}

/// GET /usage_records 的 by_model 分组：按 (source, provider_id, model_id)
/// 聚合，字段命名与 /usage 的 by_model 一致。带 source 维度是必须的——客户端
/// 要把远端汇总按真实 source 归并进对应来源路（缓存命中率分源口径依赖它）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteUsageModelGroup {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub requests: i64,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

/// GET /usage_records 响应：同 WHERE 下的总条数 + 逐条明细 + 模型分组。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteUsageRecords {
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub rows: Vec<RemoteUsageRecord>,
    #[serde(default)]
    pub by_model: Vec<RemoteUsageModelGroup>,
}

/// GET /usage_records 查询参数。
#[derive(Debug, Deserialize, Clone)]
pub struct RemoteUsageRecordsRequest {
    pub from_ms: i64,
    pub to_ms: i64,
    /// 来源过滤（客户端单来源筛选时传该来源名），空 = 全部来源。
    #[serde(default)]
    pub source: String,
    /// 模型 id 精确过滤，空 = 不筛。
    #[serde(default)]
    pub model_id: String,
    /// 排除本机设备（本机记录本地直读，不重复返回），逗号分隔。
    #[serde(default)]
    pub exclude_device: String,
    /// 仅查询这些设备（逗号分隔），优先级高于 exclude_device。
    #[serde(default)]
    pub devices: String,
    /// 行数上限（服务端夹紧 ≤5000；只需要 by_model 时传 1）。
    #[serde(default)]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

/// 拉取远端逐条请求明细（GET /usage_records）。
/// - devices 非空：只查指定设备；
/// - 否则 exclude_device 非空：排除本机（请求日志页「全部设备」口径恒走此参，
///   与本机直读数据不相交，拼接即免去重）；
/// - 都空：查全部。
pub fn fetch_remote_usage_records(
    req: RemoteUsageRecordsRequest,
) -> Result<RemoteUsageRecords, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;

    // 数字参数拼进基础 URL；任意字符串参数（source / model_id / 设备 id 列表）
    // 一律走 query 对的百分号编码（ureq 内部 form_urlencoded，已有查询串时
    // 自动以 & 追加）：手工拼接时 Werkzeug 会把字面 `+` 解成空格、`&` / `#`
    // 直接截断参数，远端按错误条件过滤返回空、静默失真。
    let url = format!(
        "{base}/usage_records?from_ms={}&to_ms={}&limit={}&offset={}",
        req.from_ms, req.to_ms, req.limit, req.offset
    );
    let mut request = agent_with_timeouts(2, 5).get(&url);
    if !req.source.is_empty() {
        request = request.query("source", &req.source);
    }
    if !req.model_id.is_empty() {
        request = request.query("model_id", &req.model_id);
    }
    if !req.devices.is_empty() {
        request = request.query("devices", &req.devices);
    } else if !req.exclude_device.is_empty() {
        request = request.query("exclude_device", &req.exclude_device);
    }
    let resp: RemoteUsageRecords = request
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(map_http_err("查询远端请求明细"))?
        .into_json()
        .map_err(|e| format!("解析远端请求明细失败: {e}"))?;
    Ok(resp)
}

// ===== 远端模型清单（价格设置页 / 价格检查用）=====

/// 远端一条模型记录（全部设备 × 全部来源的去重并集）
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteModelInfo {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model_id: String,
}

/// 远端模型清单缓存，条目 = (最近一次尝试时间, 清单)。
/// - 成功后 TTL_REMOTE_MODELS（5 分钟）内直接复用；
/// - 拉取失败写入短负缓存 TTL_REMOTE_MODELS_RETRY（30 秒），期间用旧值/空值顶住，
///   避免服务器不可达时设置页每次打开都等一轮 3s 超时（list_models 与
///   check_pricing_updates 首开即并发调用，无负缓存最坏叠加约 6s）。
static REMOTE_MODELS_CACHE: OnceLock<Mutex<Option<(Instant, Vec<RemoteModelInfo>)>>> =
    OnceLock::new();
const TTL_REMOTE_MODELS: Duration = Duration::from_secs(300);
const TTL_REMOTE_MODELS_RETRY: Duration = Duration::from_secs(30);

/// 实际请求服务端 GET /models（轻量 distinct 清单）。
fn fetch_remote_models_once() -> Result<Vec<RemoteModelInfo>, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Ok(Vec::new());
    }
    #[derive(Deserialize)]
    struct ModelsResponse {
        #[serde(default)]
        models: Vec<RemoteModelInfo>,
    }
    let resp: ModelsResponse = ureq::get(&format!("{}/models", cfg.server_url))
        .set("Authorization", &format!("Bearer {}", cfg.device_token))
        // 清单接口很轻，超时收紧到 3s：避免服务器不可达时拖慢设置页首次加载
        .timeout(Duration::from_secs(3))
        .call()
        .map_err(map_http_err("查询远端模型清单"))?
        .into_json()
        .map_err(|e| format!("解析远端模型清单失败: {e}"))?;
    Ok(resp
        .models
        .into_iter()
        .filter(|m| !m.model_id.is_empty())
        .collect())
}

/// 远端模型清单（带缓存、负缓存与静默降级），绝不向调用方报错：
/// - 未启用同步 → 恒为空且不写缓存（刚注册设备后立即可见远端模型）；
/// - 拉取失败 → 30s 内不重试，期间沿用上次清单（含过期值）；
/// - 完全无缓存且失败 → 空列表。
pub fn remote_models_cached() -> Vec<RemoteModelInfo> {
    // 未启用同步直接短路，不触碰缓存
    let enabled = load_sync_config()
        .map(|c| c.enabled && !c.device_token.is_empty())
        .unwrap_or(false);
    if !enabled {
        return Vec::new();
    }

    let cache = REMOTE_MODELS_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((last_attempt, models)) = guard.as_ref() {
        // 有数据：5 分钟内复用；空数据（上次失败）：30s 负缓存内不重试
        let ttl = if models.is_empty() {
            TTL_REMOTE_MODELS_RETRY
        } else {
            TTL_REMOTE_MODELS
        };
        if last_attempt.elapsed() < ttl {
            return models.clone();
        }
    }
    match fetch_remote_models_once() {
        Ok(models) => {
            *guard = Some((Instant::now(), models.clone()));
            models
        }
        Err(_) => {
            // 失败：只推进尝试时间戳，保留旧清单继续顶住（无旧值则为空）
            if let Some((last_attempt, models)) = guard.as_mut() {
                *last_attempt = Instant::now();
                models.clone()
            } else {
                *guard = Some((Instant::now(), Vec::new()));
                Vec::new()
            }
        }
    }
}

// ===== 远端额度快照查询（对比页/报告页用）=====

/// 远端单条快照（带 device_id，供前端按设备筛选）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSnapshot {
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub ts: i64,
    /// 归属账号指纹（新版上传的快照携带；旧服务端不回传则 None，
    /// 该快照不参与账号敏感计算，仅趋势图等宽松口径使用）
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub weekly_pct: u32,
    #[serde(default)]
    pub weekly_reset: Option<i64>,
    #[serde(default)]
    pub hour5_pct: u32,
    #[serde(default)]
    pub mcp_pct: u32,
    #[serde(default)]
    pub mcp_used: Option<i64>,
    #[serde(default)]
    pub mcp_total: Option<i64>,
}

/// /snapshots 返回包装
#[derive(Debug, Deserialize)]
struct SnapshotsResponse {
    #[serde(default)]
    snapshots: Vec<RemoteSnapshot>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RemoteSnapshotRequest {
    pub from_ms: i64,
    pub to_ms: i64,
    #[serde(default)]
    pub exclude_device: String,
    #[serde(default)]
    pub devices: String,
}

/// 拉取远端额度快照（带 device_id）。
pub fn fetch_remote_snapshots(req: RemoteSnapshotRequest) -> Result<Vec<RemoteSnapshot>, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Ok(Vec::new());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;

    let mut url = format!(
        "{base}/snapshots?from_ms={}&to_ms={}",
        req.from_ms, req.to_ms
    );
    if !req.devices.is_empty() {
        url.push_str(&format!("&devices={}", req.devices));
    } else if !req.exclude_device.is_empty() {
        url.push_str(&format!("&exclude_device={}", req.exclude_device));
    }

    let resp: SnapshotsResponse = agent_with_timeouts(2, 5)
        .get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(map_http_err("查询远端快照"))?
        .into_json()
        .map_err(|e| format!("解析远端快照失败: {e}"))?;
    Ok(resp.snapshots)
}

/// 远端单条 Agent 额度快照（带 device_id，供今日增量计算按设备筛选）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAgentQuotaSnapshot {
    #[serde(flatten)]
    pub snapshot: AgentQuotaSnapshot,
    #[serde(default)]
    pub device_id: String,
}

#[derive(Debug, Deserialize)]
struct AgentQuotaSnapshotsResponse {
    #[serde(default)]
    snapshots: Vec<RemoteAgentQuotaSnapshot>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RemoteAgentQuotaSnapshotRequest {
    pub from_ms: i64,
    pub to_ms: i64,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub exclude_device: String,
    #[serde(default)]
    pub devices: String,
}

/// 拉取远端 Codex / Claude / Cursor 额度快照。
pub fn fetch_remote_agent_quota_snapshots(
    req: RemoteAgentQuotaSnapshotRequest,
) -> Result<Vec<RemoteAgentQuotaSnapshot>, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Ok(Vec::new());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;
    let mut url = format!(
        "{base}/agent-quota-snapshots?from_ms={}&to_ms={}",
        req.from_ms, req.to_ms
    );
    if !req.source.is_empty() {
        url.push_str(&format!("&source={}", req.source));
    }
    if !req.devices.is_empty() {
        url.push_str(&format!("&devices={}", req.devices));
    } else if !req.exclude_device.is_empty() {
        url.push_str(&format!("&exclude_device={}", req.exclude_device));
    }

    let resp: AgentQuotaSnapshotsResponse = agent_with_timeouts(2, 5)
        .get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(map_http_err("查询远端 Agent 额度快照"))?
        .into_json()
        .map_err(|e| format!("解析远端 Agent 额度快照失败: {e}"))?;
    Ok(resp.snapshots)
}

/// 拉取设备列表（供前端设备筛选器与主窗口同步页）。
/// 仅检查 device_token（未注册/已断开即空列表）；暂停态（enabled=false 但
/// 凭证仍在）依旧可查——主窗口同步页在暂停时也要展示设备表。
pub fn fetch_devices() -> Result<Vec<DeviceInfo>, String> {
    let cfg = load_sync_config()?;
    if cfg.device_token.is_empty() {
        return Ok(Vec::new());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;
    let raw: Vec<DeviceInfoRaw> = agent_with_timeouts(2, 5)
        .get(&format!("{base}/devices"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(map_http_err("查询设备列表"))?
        .into_json()
        .map_err(|e| format!("解析设备列表失败: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|r| DeviceInfo {
            device_id: r.device_id,
            device_name: r.device_name,
            created_at: r.created_at,
            record_count: r.record_count,
            last_seen_at: r.last_seen_at,
        })
        .collect())
}

/// 同步服务端连通性探测结果（ping_sync_server 命令）。
/// - disabled：未注册（无 device_token），无服务端可探；
/// - ok：GET {server}/health 返回 2xx（/health 无鉴权）；
/// - unreachable：网络层失败（连不上 / 超时），detail 带可读原因。
/// 本函数不产生 "error" 状态：读取本地配置失败直接走 Err（invoke reject），
/// 前端的 catch 分支自行把异常折算成 status="error" 展示。
#[derive(Debug, Clone, Serialize)]
pub struct SyncPingResult {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 探测同步服务端连通性（主窗口同步页「连通状态」格）。
/// 无鉴权 GET /health，超时档取后台静默查询档（connect 2s / 总 5s，与
/// fetch_devices 同款）：探测失败快速返回，不拖慢刷新链路。
pub fn ping_sync_server() -> Result<SyncPingResult, String> {
    let cfg = load_sync_config()?;
    if cfg.device_token.is_empty() {
        return Ok(SyncPingResult {
            status: "disabled".into(),
            detail: None,
        });
    }
    match agent_with_timeouts(2, 5)
        .get(&format!("{}/health", cfg.server_url))
        .call()
    {
        Ok(_) => Ok(SyncPingResult {
            status: "ok".into(),
            detail: None,
        }),
        Err(e) => Ok(SyncPingResult {
            status: "unreachable".into(),
            detail: Some(map_http_err("探测服务端")(e)),
        }),
    }
}

// ===== 清理 =====

/// 转发清理请求到服务器。master_token 由客户端临时持有（从 UI 输入）。
#[derive(Debug, Deserialize)]
pub struct CleanupServerRequest {
    pub master_token: String,
    pub action: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub days: u32,
}

pub fn cleanup_server(req: CleanupServerRequest) -> Result<CleanupResult, String> {
    let cfg = load_sync_config()?;
    let base = &cfg.server_url;
    #[derive(Serialize)]
    struct Body<'a> {
        master_token: &'a str,
        action: &'a str,
        device_id: &'a str,
        days: u32,
    }
    let resp: CleanupResult = agent_with_timeouts(3, 10)
        .post(&format!("{base}/cleanup"))
        .send_json(Body {
            master_token: req.master_token.trim(),
            action: &req.action,
            device_id: &req.device_id,
            days: req.days,
        })
        .map_err(map_http_err("执行清理"))?
        .into_json()
        .map_err(|e| format!("解析清理响应失败: {e}"))?;
    Ok(resp)
}

// ===== 设备合并 / 改名 =====

#[derive(Debug, Deserialize)]
pub struct MergeDevicesRequest {
    pub master_token: String,
    pub source_device_id: String,
    pub target_device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    pub records_moved: i64,
    pub snapshots_moved: i64,
}

/// 合并设备：把来源设备数据并入目标设备后删除来源。master token 鉴权。
pub fn merge_devices(req: MergeDevicesRequest) -> Result<MergeResult, String> {
    let cfg = load_sync_config()?;
    let base = &cfg.server_url;
    #[derive(Serialize)]
    struct Body<'a> {
        master_token: &'a str,
        source_device_id: &'a str,
        target_device_id: &'a str,
    }
    let resp: MergeResult = agent_with_timeouts(3, 15)
        .post(&format!("{base}/device/merge"))
        .send_json(Body {
            master_token: req.master_token.trim(),
            source_device_id: req.source_device_id.trim(),
            target_device_id: req.target_device_id.trim(),
        })
        .map_err(map_http_err("合并设备"))?
        .into_json()
        .map_err(|e| format!("解析合并响应失败: {e}"))?;
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct RenameDeviceRequest {
    pub master_token: String,
    pub device_id: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameResult {
    pub updated: i64,
}

/// 修改设备显示名。master token 鉴权。
pub fn rename_device(req: RenameDeviceRequest) -> Result<RenameResult, String> {
    let cfg = load_sync_config()?;
    let base = &cfg.server_url;
    #[derive(Serialize)]
    struct Body<'a> {
        master_token: &'a str,
        device_id: &'a str,
        device_name: &'a str,
    }
    let resp: RenameResult = agent_with_timeouts(3, 10)
        .post(&format!("{base}/device/rename"))
        .send_json(Body {
            master_token: req.master_token.trim(),
            device_id: req.device_id.trim(),
            device_name: req.device_name.trim(),
        })
        .map_err(map_http_err("改名设备"))?
        .into_json()
        .map_err(|e| format!("解析改名响应失败: {e}"))?;
    Ok(resp)
}

/// 查询清理状态（数据量 + 自动清理配置）。device_token 鉴权。
/// 与 fetch_devices 同款：暂停态（enabled=false 但凭证仍在）依旧可查，
/// 主窗口同步页在暂停时也要展示数据管理区。
pub fn fetch_cleanup_status() -> Result<CleanupStatus, String> {
    let cfg = load_sync_config()?;
    if cfg.device_token.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;
    let resp: CleanupStatus = agent_with_timeouts(3, 10)
        .get(&format!("{base}/cleanup/status"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(map_http_err("查询清理状态"))?
        .into_json()
        .map_err(|e| format!("解析清理状态失败: {e}"))?;
    Ok(resp)
}

/// 配置服务端自动清理。
#[derive(Debug, Deserialize)]
pub struct AutoCleanupServerRequest {
    pub master_token: String,
    pub auto_enabled: bool,
    pub auto_days: u32,
}

pub fn set_auto_cleanup(req: AutoCleanupServerRequest) -> Result<AutoCleanupConfig, String> {
    let cfg = load_sync_config()?;
    let base = &cfg.server_url;
    #[derive(Serialize)]
    struct Body {
        master_token: String,
        auto_enabled: bool,
        auto_days: u32,
    }
    let resp: AutoCleanupConfig = agent_with_timeouts(3, 10)
        .post(&format!("{base}/cleanup/config"))
        .send_json(Body {
            master_token: req.master_token,
            auto_enabled: req.auto_enabled,
            auto_days: req.auto_days,
        })
        .map_err(map_http_err("配置自动清理"))?
        .into_json()
        .map_err(|e| format!("解析自动清理配置失败: {e}"))?;
    Ok(resp)
}

// ===== 断开连接 =====

/// 清空**各来源明细**的上传游标（断开连接时用：下次注册必须重新全量上传明细）。
/// 集中成一处，新增来源若忘了在这里加游标字段，会被单测
/// `disconnect_resets_every_source_cursor` 抓住——它按 serde 字段名扫描所有
/// `last_uploaded_*`，不依赖人工枚举。
///
/// 刻意不动 `last_uploaded_snapshot_ts` / `last_uploaded_agent_quota_snapshot_ts` /
/// `last_uploaded_provider_quota_ts`：那是额度快照通道的游标，重置它会让下次
/// 注册后重传全部历史快照（行为变更），与本次任务无关，保持既有语义
/// （只补传本次连接后新增的快照）。
fn reset_source_cursors(cfg: &mut SyncConfig) {
    cfg.last_uploaded_rowid = 0;
    cfg.last_uploaded_codex_rowid = 0;
    cfg.last_uploaded_claude_rowid = 0;
    cfg.last_uploaded_claude_rev_ts = 0;
    cfg.last_uploaded_codex_model_rev_ts = 0;
    cfg.last_uploaded_codex_model_rev_id = 0;
    cfg.last_uploaded_dsh_rowid = 0;
    cfg.last_uploaded_dsh_rev_ts = 0;
    cfg.last_uploaded_opencode_rowid = 0;
    cfg.last_uploaded_opencode_rev_ts = 0;
    cfg.last_uploaded_kimi_rowid = 0;
    cfg.last_uploaded_kimi_rev_ts = 0;
    cfg.last_uploaded_minimaxcode_rowid = 0;
    cfg.last_uploaded_minimaxcode_rev_ts = 0;
    cfg.last_sync_at = 0;
}

/// 清空凭证（保留 enabled=false），不删服务器数据。
pub fn disconnect() -> Result<(), String> {
    let mut cfg = load_sync_config()?;
    cfg.enabled = false;
    cfg.device_id.clear();
    cfg.device_name.clear();
    cfg.device_token.clear();
    reset_source_cursors(&mut cfg);
    // 重连可能换服务端，价格同步状态（revision/pending）一并复位
    crate::pricing_sync::reset_state();
    // 保留 server_url + mode + interval，方便下次重连
    save_sync_config(&cfg)
}

// ===== 后台同步线程 =====

/// 启动后台同步线程：若 mode=auto 且 enabled，按 interval_seconds 循环上传。
/// 模仿 lib.rs 的 spawn_title_updater 模式。静默失败不影响本地展示。
pub fn spawn_sync_worker() {
    std::thread::spawn(|| loop {
        let cfg = load_sync_config().unwrap_or_default();
        if cfg.enabled && cfg.mode == SyncMode::Auto {
            // 静默失败：出错只记不抛，下次重试
            if let Err(e) = upload_incremental() {
                eprintln!("[zbar-sync] 后台同步失败: {e}");
            }
        }
        let sleep_secs = if cfg.enabled && cfg.mode == SyncMode::Auto {
            cfg.interval_seconds.max(10)
        } else {
            // 非自动模式：每 60s 检查一次配置变化（用户可能切到 auto）
            60
        };
        std::thread::sleep(Duration::from_secs(sleep_secs));
    });
}

// ===== 辅助 =====

/// 规范化服务器 URL：去尾斜杠，补 scheme。
fn normalize_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("服务器地址不能为空".into());
    }
    // 允许 http:// https:// 或裸 IP/域名（无 scheme 时默认 http://）
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("http://{trimmed}"))
    }
}

/// 构建分层超时的 HTTP Agent：连接期 connect_secs 快速失败（防火墙静默
/// 丢包 / DNS 挂起时不再吃满总超时），总超时 total_secs 兜底整个请求生命
/// 周期（ureq 2 的 timeout_connect 只在 AgentBuilder 上提供，故按 Agent
/// 维度配置）。分档取值：
/// - 后台静默查询（拉取远端聚合/明细/快照/设备列表）：connect 2s + 总 5s，
///   调用方均有静默降级兜底（返回空/下轮重试），快速失败无损正确性；
/// - 后台上传与协议探测（post_sync_batch / 两处 proto 探测）：connect 3s
///   + 总 15s（上传体大、批量大，保留原总超时余量）；
/// - 用户主动操作（注册/清理/合并/改名等）：connect 3s + 总超时保持原值
///   （用户在等待结果，只收紧连接期，不缩短等待上限）。
pub(crate) fn agent_with_timeouts(connect_secs: u64, total_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(connect_secs))
        .timeout(Duration::from_secs(total_secs))
        .build()
}

/// 把 ureq::Error 转为可读中文错误。
pub(crate) fn map_http_err(op: &str) -> impl Fn(ureq::Error) -> String + '_ {
    let op = op.to_string();
    move |e: ureq::Error| {
        let detail = match e {
            ureq::Error::Status(code, resp) => {
                let body = resp.into_string().unwrap_or_default();
                format!("HTTP {code}: {body}")
            }
            ureq::Error::Transport(t) => {
                let kind = t.kind();
                let msg = t.message().unwrap_or("").to_string();
                format!("{kind:?}: {msg}")
            }
        };
        format!("{op}失败: {detail}")
    }
}

#[cfg(test)]
mod tests {
    //! 同步上传通道的单测：**一律不碰网络、不碰真实服务端、不碰真实 ~/.agenthud**
    //! （配置读写用 serde 在内存里来回，派生库相关行为由各模块自己的测试覆盖）。
    //! 覆盖四件事：
    //! 1. 游标推进（pump_source_batches，各来源共用同一实现）；
    //! 2. 修订补传扫描（sweep_source_revisions，claude/dsh/opencode/kimi/minimaxcode 共用）；
    //! 3. 配置文件向后兼容（旧 sync.json 缺新字段仍可反序列化）；
    //! 4. disconnect 的游标重置不漏来源。

    use super::*;

    /// 造一条明细（字段值不参与断言，只要 local_rowid 可控）。
    fn row(local_rowid: i64) -> UsageRow {
        UsageRow {
            local_rowid,
            started_at: local_rowid * 1_000,
            model_id: "m".into(),
            provider_id: "opencode".into(),
            input_tokens: 1,
            output_tokens: 2,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_tokens: 0,
            computed_total_tokens: 3,
            source: "opencode".into(),
            session_id: None,
            project_key: None,
            project_display: None,
            duration_ms: None,
            time_to_first_token_ms: None,
        }
    }

    /// 批循环：游标逐批推进到本批最大 rowid、循环到取空为止；每批都通过
    /// on_advance 回写游标（这是「中途失败也不丢断点」的前提）。
    #[test]
    fn pump_advances_cursor_per_batch_until_exhausted() {
        let mut batches = vec![vec![row(3), row(7)], vec![row(9)], Vec::new()].into_iter();
        let mut queries: Vec<(i64, usize)> = Vec::new();
        let mut sent: Vec<Vec<i64>> = Vec::new();
        let mut advances: Vec<i64> = Vec::new();

        let (total, since) = pump_source_batches(
            0,
            500,
            |since, limit| {
                queries.push((since, limit));
                Ok(batches.next().unwrap_or_default())
            },
            |records, batch_max| {
                sent.push(records.iter().map(|r| r.local_rowid).collect());
                Ok(BatchAck {
                    accepted: records.len(),
                    max_rowid: batch_max,
                })
            },
            |advanced| advances.push(advanced),
        )
        .expect("批循环不该失败");

        assert_eq!(queries, vec![(0, 500), (7, 500), (9, 500)], "下一批必须从新游标取");
        assert_eq!(sent, vec![vec![3, 7], vec![9]]);
        assert_eq!(advances, vec![7, 9], "每批成功都应立刻回写游标");
        assert_eq!(total, 3);
        assert_eq!(since, 9);
    }

    /// 服务端回传的游标比本批最大 rowid 小（旧服务端回 0、或落库失败回旧值）时，
    /// 本地游标必须用本批最大 rowid 兜底——否则下一轮取到同一批，死循环。
    #[test]
    fn pump_never_rewinds_cursor_on_stale_server_ack() {
        let mut first = true;
        let mut advances: Vec<i64> = Vec::new();
        let (total, since) = pump_source_batches(
            0,
            500,
            |_since, _limit| {
                if first {
                    first = false;
                    Ok(vec![row(4)])
                } else {
                    Ok(Vec::new())
                }
            },
            |_records, _batch_max| {
                Ok(BatchAck {
                    accepted: 1,
                    max_rowid: 0, // 服务端回传旧游标
                })
            },
            |advanced| advances.push(advanced),
        )
        .expect("批循环不该失败");

        assert_eq!(since, 4, "服务端回传 0 时本地游标必须停在本批最大 rowid");
        assert_eq!(advances, vec![4]);
        assert_eq!(total, 1);
    }

    /// 中途失败：已成功批次推进的游标要保留（调用方随配置落盘 → 下次从断点续传），
    /// 错误照常向上抛。
    #[test]
    fn pump_keeps_checkpoint_when_a_later_batch_fails() {
        let mut calls = 0;
        let mut advances: Vec<i64> = Vec::new();
        let err = pump_source_batches(
            0,
            500,
            |_since, _limit| {
                calls += 1;
                if calls == 1 {
                    Ok(vec![row(2)])
                } else {
                    Err("模拟查询失败".to_string())
                }
            },
            |_records, batch_max| {
                Ok(BatchAck {
                    accepted: 1,
                    max_rowid: batch_max,
                })
            },
            |advanced| advances.push(advanced),
        )
        .expect_err("查询失败必须向上报错");

        assert!(err.contains("模拟查询失败"), "错误应原样上抛: {err}");
        assert_eq!(advances, vec![2], "首批已成功的断点必须保留");
    }

    /// 修订扫描：按 after_id 分批取到空为止，累计服务端接受的条数，水位取
    /// **扫描开始前**的时刻（扫描期间的修订留给下一轮，避免越扫越多）。
    #[test]
    fn sweep_pages_by_id_and_returns_pre_sweep_watermark() {
        let before = chrono::Local::now().timestamp_millis();
        let mut batches = vec![vec![row(1), row(2)], vec![row(5)], Vec::new()].into_iter();
        let mut queries: Vec<(i64, i64, usize)> = Vec::new();
        let mut sent: Vec<Vec<i64>> = Vec::new();

        let (total, watermark) = sweep_source_revisions(
            1_000,
            2,
            |since_ts, after_id, limit| {
                queries.push((since_ts, after_id, limit));
                Ok(batches.next().unwrap_or_default())
            },
            |records| {
                sent.push(records.iter().map(|r| r.local_rowid).collect());
                Ok(records.len())
            },
        )
        .expect("修订扫描不该失败");
        let after = chrono::Local::now().timestamp_millis();

        assert_eq!(
            queries,
            vec![(1_000, 0, 2), (1_000, 2, 2), (1_000, 5, 2)],
            "修订游标固定，after_id 逐批推进（防止同批被反复选出）"
        );
        assert_eq!(sent, vec![vec![1, 2], vec![5]]);
        assert_eq!(total, 3);
        assert!(
            watermark >= before && watermark <= after,
            "水位应取扫描开始时刻：{before} <= {watermark} <= {after}"
        );
    }

    /// 修订扫描上传失败：错误上抛，调用方不会推进修订游标（水位是返回值，
    /// 出错就没有返回值）——下一轮重新扫，服务端 upsert 幂等。
    #[test]
    fn sweep_propagates_send_error_without_watermark() {
        let err = sweep_source_revisions(
            0,
            500,
            |_since_ts, _after_id, _limit| Ok(vec![row(1)]),
            |_records| Err("模拟上传失败".to_string()),
        )
        .expect_err("上传失败必须向上报错");
        assert!(err.contains("模拟上传失败"), "错误应原样上抛: {err}");
    }

    /// 未知来源必须在任何查询/网络请求前就报错（防止拼错来源名时静默当成功）。
    #[test]
    fn unknown_source_is_rejected_before_any_query() {
        let mut cfg = SyncConfig::default();
        // 地址指向不可路由端口：真发请求会超时失败，而本用例必须立刻返回
        let err = upload_derived_source_incremental(
            &mut cfg,
            "http://127.0.0.1:1",
            "token",
            "opencodego",
        )
        .expect_err("未知来源必须报错");
        assert!(err.contains("未知数据来源"), "错误信息应指明来源非法: {err}");
        assert_eq!(cfg.last_uploaded_opencode_rowid, 0, "报错不得改动任何游标");
    }

    /// 旧配置文件（升级前写入的 sync.json，没有 opencode / kimi 的四个游标字段）
    /// 必须仍能反序列化，且既有字段值原样保留、新字段取 0。
    #[test]
    fn legacy_config_without_new_cursors_still_deserializes() {
        let legacy = r#"{
            "enabled": true,
            "mode": "auto",
            "interval_seconds": 300,
            "server_url": "http://192.168.1.9:8787",
            "device_id": "dev-1",
            "device_name": "本机",
            "device_token": "tok",
            "last_uploaded_rowid": 11,
            "last_uploaded_codex_rowid": 22,
            "last_uploaded_claude_rowid": 33,
            "last_uploaded_claude_rev_ts": 44,
            "last_uploaded_codex_model_rev_ts": 55,
            "last_uploaded_codex_model_rev_id": 66,
            "last_uploaded_dsh_rowid": 77,
            "last_uploaded_dsh_rev_ts": 88,
            "last_uploaded_snapshot_ts": 99,
            "last_uploaded_agent_quota_snapshot_ts": 111,
            "last_sync_at": 222
        }"#;
        let cfg: SyncConfig =
            serde_json::from_str(legacy).expect("旧配置必须仍可反序列化（新字段用 serde 默认）");

        // 既有字段一个都不能丢
        assert!(cfg.enabled);
        assert_eq!(cfg.mode, SyncMode::Auto);
        assert_eq!(cfg.interval_seconds, 300);
        assert_eq!(cfg.device_id, "dev-1");
        assert_eq!(cfg.last_uploaded_rowid, 11);
        assert_eq!(cfg.last_uploaded_dsh_rowid, 77);
        assert_eq!(cfg.last_uploaded_dsh_rev_ts, 88);
        assert_eq!(cfg.last_uploaded_agent_quota_snapshot_ts, 111);
        assert_eq!(cfg.last_sync_at, 222);
        // 新增的四个游标字段缺省为 0（= 下次同步从头全量上传）
        assert_eq!(cfg.last_uploaded_opencode_rowid, 0);
        assert_eq!(cfg.last_uploaded_opencode_rev_ts, 0);
        assert_eq!(cfg.last_uploaded_kimi_rowid, 0);
        assert_eq!(cfg.last_uploaded_kimi_rev_ts, 0);

        // 极端情况：空对象也是合法配置（每个字段都有 serde(default)）
        let empty: SyncConfig = serde_json::from_str("{}").expect("空配置应可反序列化");
        assert!(!empty.enabled);
        assert_eq!(empty.last_uploaded_opencode_rowid, 0);
        assert_eq!(empty.last_uploaded_kimi_rev_ts, 0);
    }

    /// 新字段会写进配置文件且往返一致（旧客户端读到未知字段会忽略，
    /// 但新客户端自己必须能读回自己的游标）。
    #[test]
    fn new_cursors_round_trip_through_config_json() {
        let cfg = SyncConfig {
            last_uploaded_opencode_rowid: 7,
            last_uploaded_opencode_rev_ts: 8,
            last_uploaded_kimi_rowid: 9,
            last_uploaded_kimi_rev_ts: 10,
            last_uploaded_minimaxcode_rowid: 11,
            last_uploaded_minimaxcode_rev_ts: 12,
            ..Default::default()
        };
        let text = serde_json::to_string(&cfg).expect("序列化失败");
        for key in [
            "last_uploaded_opencode_rowid",
            "last_uploaded_opencode_rev_ts",
            "last_uploaded_kimi_rowid",
            "last_uploaded_kimi_rev_ts",
            "last_uploaded_minimaxcode_rowid",
            "last_uploaded_minimaxcode_rev_ts",
        ] {
            assert!(text.contains(key), "配置应包含字段 {key}: {text}");
        }
        let back: SyncConfig = serde_json::from_str(&text).expect("反序列化失败");
        assert_eq!(back.last_uploaded_opencode_rowid, 7);
        assert_eq!(back.last_uploaded_opencode_rev_ts, 8);
        assert_eq!(back.last_uploaded_kimi_rowid, 9);
        assert_eq!(back.last_uploaded_kimi_rev_ts, 10);
        assert_eq!(back.last_uploaded_minimaxcode_rowid, 11);
        assert_eq!(back.last_uploaded_minimaxcode_rev_ts, 12);
    }

    /// R1 落盘竞态：上传期间凭证变了（断开清空 / 重注册换号）→ 放弃落盘，
    /// 不把旧快照的 enabled / device_token 写回复活用户的断开操作。
    #[test]
    fn 落盘前凭证变更则放弃落盘() {
        let mut ran = SyncConfig::default();
        ran.device_token = "tok-a".into();
        ran.enabled = true;
        ran.last_uploaded_rowid = 500;

        // 断开：token 清空
        let disconnected = SyncConfig::default();
        assert!(merge_config_for_save("tok-a", &ran, &disconnected).is_none());

        // 重注册：token 换成别的值
        let mut reregistered = SyncConfig::default();
        reregistered.device_token = "tok-b".into();
        assert!(merge_config_for_save("tok-a", &ran, &reregistered).is_none());
    }

    /// R1 落盘竞态：token 未变（仅暂停等 UI 操作）→ 以最新配置为底合并，
    /// 身份/开关/模式取最新值（不回滚暂停与改名），游标与 last_sync_at 取
    /// 本轮推进值。字段清单逐项断言：新增来源游标忘覆盖时在此补断言。
    #[test]
    fn 落盘合并取最新身份与本轮游标() {
        let mut ran = SyncConfig::default();
        ran.device_token = "tok-a".into();
        ran.enabled = true;
        ran.mode = SyncMode::Auto;
        ran.interval_seconds = 60;
        ran.device_name = "old-name".into();
        ran.last_uploaded_rowid = 500;
        ran.last_uploaded_codex_rowid = 1;
        ran.last_uploaded_claude_rowid = 2;
        ran.last_uploaded_claude_rev_ts = 3;
        ran.last_uploaded_codex_model_rev_ts = 4;
        ran.last_uploaded_codex_model_rev_id = 5;
        ran.last_uploaded_dsh_rowid = 6;
        ran.last_uploaded_dsh_rev_ts = 7;
        ran.last_uploaded_opencode_rowid = 8;
        ran.last_uploaded_opencode_rev_ts = 9;
        ran.last_uploaded_kimi_rowid = 10;
        ran.last_uploaded_kimi_rev_ts = 11;
        ran.last_uploaded_minimaxcode_rowid = 12;
        ran.last_uploaded_minimaxcode_rev_ts = 13;
        ran.last_uploaded_snapshot_ts = 14;
        ran.last_uploaded_agent_quota_snapshot_ts = 15;
        ran.last_uploaded_provider_quota_ts = 16;
        ran.last_sync_at = 123_456;

        let mut latest = SyncConfig::default();
        latest.device_token = "tok-a".into();
        // 本轮上传期间用户在 UI 上的操作：暂停 + 改模式/间隔 + 改设备名
        latest.enabled = false;
        latest.mode = SyncMode::Manual;
        latest.interval_seconds = 300;
        latest.device_name = "renamed".into();
        latest.last_uploaded_rowid = 100;

        let merged = merge_config_for_save("tok-a", &ran, &latest).unwrap();
        // 身份与开关取最新：不回滚用户的暂停 / 模式 / 改名
        assert!(!merged.enabled);
        assert_eq!(merged.mode, SyncMode::Manual);
        assert_eq!(merged.interval_seconds, 300);
        assert_eq!(merged.device_name, "renamed");
        assert_eq!(merged.device_token, "tok-a");
        // 游标与 last_sync_at 取本轮推进值（不被盘上旧值拖回）
        assert_eq!(merged.last_uploaded_rowid, 500);
        assert_eq!(merged.last_uploaded_codex_rowid, 1);
        assert_eq!(merged.last_uploaded_claude_rowid, 2);
        assert_eq!(merged.last_uploaded_claude_rev_ts, 3);
        assert_eq!(merged.last_uploaded_codex_model_rev_ts, 4);
        assert_eq!(merged.last_uploaded_codex_model_rev_id, 5);
        assert_eq!(merged.last_uploaded_dsh_rowid, 6);
        assert_eq!(merged.last_uploaded_dsh_rev_ts, 7);
        assert_eq!(merged.last_uploaded_opencode_rowid, 8);
        assert_eq!(merged.last_uploaded_opencode_rev_ts, 9);
        assert_eq!(merged.last_uploaded_kimi_rowid, 10);
        assert_eq!(merged.last_uploaded_kimi_rev_ts, 11);
        assert_eq!(merged.last_uploaded_minimaxcode_rowid, 12);
        assert_eq!(merged.last_uploaded_minimaxcode_rev_ts, 13);
        assert_eq!(merged.last_uploaded_snapshot_ts, 14);
        assert_eq!(merged.last_uploaded_agent_quota_snapshot_ts, 15);
        assert_eq!(merged.last_uploaded_provider_quota_ts, 16);
        assert_eq!(merged.last_sync_at, 123_456);
    }

    /// disconnect 必须清空**每个来源**的明细游标。这里按 serde 字段名扫描
    /// （而不是人工枚举字段），所以将来新增来源却忘了在 reset_source_cursors
    /// 里加一行时，本用例会失败。
    #[test]
    fn disconnect_resets_every_source_cursor() {
        // 把所有 last_uploaded_* 字段灌成非零值，再走一遍重置
        let mut obj = serde_json::to_value(SyncConfig::default())
            .expect("序列化默认配置失败")
            .as_object()
            .cloned()
            .expect("配置应是 JSON 对象");
        for (key, value) in obj.iter_mut() {
            if key.starts_with("last_uploaded") {
                *value = serde_json::json!(1_234);
            }
        }
        let mut cfg: SyncConfig =
            serde_json::from_value(serde_json::Value::Object(obj)).expect("反序列化失败");
        cfg.last_sync_at = 1_234;
        reset_source_cursors(&mut cfg);

        let after = serde_json::to_value(&cfg).expect("序列化失败");
        let after = after.as_object().expect("配置应是 JSON 对象");
        // 额度快照通道的游标不属于「来源明细」，reset_source_cursors 刻意不动
        const SNAPSHOT_CURSORS: [&str; 3] = [
            "last_uploaded_snapshot_ts",
            "last_uploaded_agent_quota_snapshot_ts",
            "last_uploaded_provider_quota_ts",
        ];
        let mut checked = 0usize;
        for (key, value) in after.iter() {
            if !key.starts_with("last_uploaded") || SNAPSHOT_CURSORS.contains(&key.as_str()) {
                continue;
            }
            assert_eq!(
                value,
                &serde_json::json!(0),
                "{key} 未被 reset_source_cursors 清空"
            );
            checked += 1;
        }
        assert!(
            checked >= 10,
            "扫到的明细游标字段只有 {checked} 个，断言可能已失效"
        );
        assert_eq!(
            after.get("last_sync_at"),
            Some(&serde_json::json!(0)),
            "last_sync_at 也应清空"
        );
        assert_eq!(
            after.get("last_uploaded_snapshot_ts"),
            Some(&serde_json::json!(1_234)),
            "快照游标保持既有语义（不重置）"
        );
    }

    // ===== provider 快照游标门控（proto 7）=====

    /// proto < 7 的服务端会按白名单静默丢弃新 source：此时游标必须原地不动，
    /// 否则未确认的 provider 快照永久丢失（codex 在 proto < 2 不推进的同款语义）。
    #[test]
    fn provider_cursor_holds_when_proto_below_7() {
        assert_eq!(
            provider_quota_cursor_after(true, 6, Some(9_000), 5_000),
            5_000,
            "proto 6：即使已附加发送，游标也不得推进"
        );
        assert_eq!(
            provider_quota_cursor_after(true, 3, Some(9_000), 0),
            0,
            "proto 3（旧服务端）：游标不推进"
        );
        assert_eq!(
            provider_quota_cursor_after(false, 7, Some(9_000), 5_000),
            5_000,
            "本轮未附加发送（支持缓存确认不支持）：游标不推进"
        );
    }

    /// proto >= 7 且本轮确实附加发送：游标推进到本批最大 ts。
    #[test]
    fn provider_cursor_advances_on_proto7_ack() {
        assert_eq!(
            provider_quota_cursor_after(true, 7, Some(9_000), 5_000),
            9_000
        );
        // 未来版本（8+）同样视为支持
        assert_eq!(
            provider_quota_cursor_after(true, 8, Some(9_000), 5_000),
            9_000
        );
        // 防御分支：附加发送但无待发数据（max=None）→ 保留原值
        assert_eq!(provider_quota_cursor_after(true, 7, None, 5_000), 5_000);
    }

    /// 附加发送的判定（30 分钟重探状态机，注入假时刻）：
    /// 未探测 → 附加；确认支持 → 每轮附加；确认不支持且未到窗口 → 跳过；
    /// 超过 30 分钟 → 重探。
    #[test]
    fn provider_attach_gate_rechecks_after_30_minutes() {
        let now = Instant::now();
        // 从未探测：首轮附加（探测即真实上传，无副作用）
        assert!(should_attach_with(None, now));
        // 上次探测支持：正常增量，每轮都附加
        assert!(should_attach_with(Some((now, true)), now));
        // 上次探测不支持、29 分钟后：窗口期内跳过（零冗余流量）
        assert!(!should_attach_with(
            Some((now - PROVIDER_PROTO_RECHECK + Duration::from_secs(60), false)),
            now
        ));
        // 上次探测不支持、31 分钟后：服务端可能已升级，重探
        assert!(should_attach_with(
            Some((now - PROVIDER_PROTO_RECHECK - Duration::from_secs(60), false)),
            now
        ));
    }

    /// query_zcode_rows_with_sessions 的 SELECT 动态拼列 ↔ 数字下标映射回归
    /// 测试：SELECT 尾部由 speed_columns 动态拼入耗时/首字两列（本查询还多带
    /// 一列 session_id），行读取用数字下标——列序与下标错位会让相邻字段串值。
    /// 含两列的库验证全字段映射与项目维度回填，缺两列的库验证降级 None。
    #[test]
    fn zcode_rows_query_maps_speed_and_session_columns() {
        let conn = rusqlite::Connection::open_in_memory().expect("打开内存库失败");
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER, model_id TEXT, provider_id TEXT,
                input_tokens INTEGER DEFAULT 0, output_tokens INTEGER DEFAULT 0,
                cache_read_input_tokens INTEGER DEFAULT 0,
                cache_creation_input_tokens INTEGER DEFAULT 0,
                reasoning_tokens INTEGER DEFAULT 0,
                computed_total_tokens INTEGER DEFAULT 0,
                session_id TEXT,
                duration_ms INTEGER,
                time_to_first_token_ms INTEGER
             );
             INSERT INTO model_usage (started_at, model_id, provider_id, output_tokens,
                computed_total_tokens, session_id, duration_ms, time_to_first_token_ms)
             VALUES
                (1000, 'glm-5.3', 'zai', 30, 30, 's-1', 1500, 220),
                -- 0 值耗时首字不给值（CASE WHEN > 0 口径）
                (2000, 'glm-5.3', 'zai', 8, 8, 's-2', 0, 0),
                -- 无会话的行：三字段保持 None
                (3000, 'glm-5.3', 'zai', 5, 5, NULL, NULL, NULL);",
        )
        .expect("建表并写入样例行失败");
        let mut map = HashMap::new();
        map.insert(
            "s-1".to_string(),
            ("proj-1".to_string(), "/d/proj-1".to_string()),
        );

        let rows =
            query_zcode_rows_with_sessions_at(&conn, 0, 10, &map).expect("zcode 增量查询失败");
        assert_eq!(rows.len(), 3);
        let first = &rows[0];
        assert_eq!(first.started_at, 1000);
        assert_eq!(first.output_tokens, 30);
        assert_eq!(first.source, "zcode");
        // session_id 紧邻耗时列，一并断言防错位串值
        assert_eq!(first.session_id.as_deref(), Some("s-1"));
        assert_eq!(first.project_key.as_deref(), Some("proj-1"));
        assert_eq!(first.project_display.as_deref(), Some("/d/proj-1"));
        assert!((first.duration_ms.unwrap() - 1500.0).abs() < 1e-6);
        assert!((first.time_to_first_token_ms.unwrap() - 220.0).abs() < 1e-6);
        let second = &rows[1];
        assert_eq!(second.session_id, None, "map 查不到的会话三字段保持 None");
        assert_eq!(second.project_key, None);
        assert_eq!(second.duration_ms, None, "duration_ms=0 不给值");
        assert_eq!(second.time_to_first_token_ms, None, "ttft=0 不给值");
        let third = &rows[2];
        assert_eq!(third.session_id, None, "无会话行三字段恒 None");
        assert_eq!(third.duration_ms, None, "NULL 耗时不给值");

        // 缺耗时/首字两列的库（保留 session_id——内核 SQL 恒含该列，无该列的
        // 老主库由薄壳 has_column 分支整体转投 db::query_since，不进内核）：
        // 两字段走 NULL 占位降级恒 None，查询不报错
        let conn = rusqlite::Connection::open_in_memory().expect("打开内存库失败");
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER, model_id TEXT, provider_id TEXT,
                input_tokens INTEGER DEFAULT 0, output_tokens INTEGER DEFAULT 0,
                cache_read_input_tokens INTEGER DEFAULT 0,
                cache_creation_input_tokens INTEGER DEFAULT 0,
                reasoning_tokens INTEGER DEFAULT 0,
                computed_total_tokens INTEGER DEFAULT 0,
                session_id TEXT
             );
             INSERT INTO model_usage (started_at, model_id, provider_id, output_tokens, session_id)
             VALUES (5000, 'glm-5.3', 'zai', 50, 's-8');",
        )
        .expect("重建缺列表失败");
        let rows = query_zcode_rows_with_sessions_at(&conn, 0, 10, &HashMap::new())
            .expect("缺列库 zcode 增量查询失败");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].output_tokens, 50);
        assert_eq!(rows[0].duration_ms, None, "无 duration_ms 列应恒 None");
        assert_eq!(rows[0].time_to_first_token_ms, None, "无 ttft 列应恒 None");
    }
}
