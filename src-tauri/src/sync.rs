//! 多设备同步：配置读写 + 增量上传 + HTTP 调用 + 后台同步线程。
//!
//! 设计要点（见 server/README.md）：
//! - model_usage 的普通明细用 (device_id, source, local_rowid) 去重；模型名回填通过
//!   独立修订游标补传，不影响普通 rowid 游标。
//! - 客户端维护游标 last_uploaded_rowid，只上传 rowid > 游标 的新记录。
//! - 复用项目现有 ureq HTTP 客户端 + pricing::config_dir() 的 ~/.zbar/ 目录。
//! - 本地采集型来源（codex / claude / dsh / opencode / kimi）各自一套
//!   「rowid 游标 + 修订游标」，见 upload_derived_source_incremental。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::agent_quota_history::{self, AgentQuotaSnapshot};
use crate::db::{self, default_source, UsageRow};
use crate::pricing::config_dir;

// ===== 配置（~/.zbar/sync.json）=====

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
    /// 已上传到的快照 ts 游标（额度快照）。
    #[serde(default)]
    pub last_uploaded_snapshot_ts: i64,
    /// 已上传到的 Agent 额度快照 ts 游标。
    #[serde(default)]
    pub last_uploaded_agent_quota_snapshot_ts: i64,
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
            last_uploaded_snapshot_ts: 0,
            last_uploaded_agent_quota_snapshot_ts: 0,
            last_sync_at: 0,
        }
    }
}

/// 配置文件路径：~/.zbar/sync.json
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
    /// 服务端协议版本：2 = 支持多来源，3 = 支持 Agent 额度快照。
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfoRaw {
    device_id: String,
    device_name: String,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    record_count: Option<i64>,
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
    let resp: RegisterResponse = ureq::post(&format!("{base}/register"))
        .timeout(Duration::from_secs(10))
        .send_json(Body {
            master_token: req.master_token.trim(),
            device_name: req.device_name.trim(),
        })
        .map_err(map_http_err("注册设备"))?
        .into_json()
        .map_err(|e| format!("解析注册响应失败: {e}"))?;

    // 读取现有配置，保留游标等字段
    let mut cfg = load_sync_config()?;
    cfg.enabled = true;
    cfg.server_url = base;
    cfg.device_id = resp.device_id;
    cfg.device_name = resp.device_name;
    cfg.device_token = resp.device_token;
    // 注册不重置游标，沿用已有值（首次为 0，会全量上传）
    save_sync_config(&cfg)?;
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
        if records_empty && pending_snaps.is_empty() && pending_agent_quota_snapshots.is_empty() {
            break;
        }
        if records_empty && !first_batch {
            // 明细已发完，快照在首批已随带；无更多数据
            break;
        }
        // 本批最大 rowid（游标必须至少推进到这里，否则死循环）
        let batch_max = records.last().map(|r| r.local_rowid).unwrap_or(since);

        // 快照只在首批携带（一次性发完）；之后清空避免重复发
        let snaps_to_send = if first_batch {
            std::mem::take(&mut pending_snaps)
        } else {
            Vec::new()
        };
        let last_snapshot_ts = if first_batch { snap_max_ts } else { None };
        let agent_quota_snapshots_to_send = if first_batch {
            std::mem::take(&mut pending_agent_quota_snapshots)
        } else {
            Vec::new()
        };
        let last_agent_quota_snapshot_ts = if first_batch {
            agent_quota_snapshot_max_ts
        } else {
            None
        };

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

    // ===== 阶段五：OpenCode 明细（本地采集型，派生库 ~/.zbar/opencode.sqlite）=====
    let mut opencode_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "opencode") {
        Ok(n) => opencode_uploaded = n,
        Err(e) => {
            eprintln!("[zbar-sync] OpenCode 增量上传失败（下次重试）: {e}");
        }
    }

    // ===== 阶段六：Kimi 明细（本地采集型，派生库 ~/.zbar/kimi.sqlite）=====
    // 注意：这是**用量明细**通道；Kimi 的额度快照走 agent_quota_snapshots，
    // 两者游标与 payload 字段都不同，互不影响。
    let mut kimi_uploaded = 0usize;
    match upload_derived_source_incremental(&mut cfg, &base, &token, "kimi") {
        Ok(n) => kimi_uploaded = n,
        Err(e) => {
            eprintln!("[zbar-sync] Kimi 增量上传失败（下次重试）: {e}");
        }
    }

    let _ = snapshot_cursor_advanced; // 标记已用，避免未读警告
    let _ = agent_quota_snapshot_cursor_advanced;
    let now = chrono::Local::now().timestamp_millis();
    cfg.last_uploaded_rowid = since;
    cfg.last_sync_at = now;
    save_sync_config(&cfg)?;

    Ok(SyncOutcome {
        uploaded: total_uploaded
            + codex_uploaded
            + claude_uploaded
            + dsh_uploaded
            + opencode_uploaded
            + kimi_uploaded,
        new_max_rowid: since,
        last_sync_at: now,
    })
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
    let probe: SyncResponse = match ureq::post(&format!("{base}/sync"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(15))
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
/// rowid 游标口径，仅多带 session_id 列），并按 zcode_sessions 派生库的
/// session_id → (project_key, cwd) 映射填充三字段。
/// with_projects=false、主库无 session_id 列或映射查不到该会话时，三字段
/// 保持 None（与 db::query_since 原行为逐字节一致）。
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
    let sql = "SELECT rowid, started_at, model_id, provider_id,
            COALESCE(input_tokens,0), COALESCE(output_tokens,0),
            COALESCE(cache_read_input_tokens,0), COALESCE(cache_creation_input_tokens,0),
            COALESCE(reasoning_tokens,0), COALESCE(computed_total_tokens,0),
            session_id
     FROM model_usage
     WHERE rowid > ?1
     ORDER BY rowid ASC
     LIMIT ?2";
    let mut stmt = conn
        .prepare(sql)
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
                },
                row.get::<_, Option<String>>(10)?,
            ))
        })
        .map_err(|e| format!("读取 zcode 增量记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 zcode 增量记录失败: {e}"))?;

    // 逐行回填维度：查到映射才附带三字段（绑定出现，查不到保持 None）
    let map = if with_projects {
        crate::zcode_sessions::session_project_map()
    } else {
        HashMap::new()
    };
    Ok(rows
        .into_iter()
        .map(|(mut row, session_id)| {
            if let Some((key, display)) = session_id.as_ref().and_then(|id| map.get(id)) {
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
    ureq::post(&format!("{base}/sync"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(15))
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

/// 修订补传扫描（claude / dsh / opencode / kimi 同构，codex 的模型名回填另有一套）：
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
    let probe: SyncResponse = ureq::post(&format!("{base}/sync"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(15))
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

    // Claude / DSH / OpenCode / Kimi 修订补传（扫描逻辑与水位口径见
    // sweep_source_revisions）：这些来源都会「行 id 不变、内容被覆盖」——
    // Claude 会话流式落盘、DSH 同一步先小后大、OpenCode 指纹变化整会话重导
    // 时存活行原地 upsert、Kimi wire.jsonl 被重写重放——上面的 id 游标永远选不出
    // 它们，必须按 updated_at 重传，由服务端「总量更大者胜」upsert 覆盖修正。
    // 旧服务端（无覆盖守卫）会忽略重传——退化为旧行为（远端保留中间值），
    // 无数据损坏，故无需协议协商。
    if matches!(source, "claude" | "dsh" | "opencode" | "kimi") {
        let rev_cursor = match source {
            "claude" => cfg.last_uploaded_claude_rev_ts,
            "dsh" => cfg.last_uploaded_dsh_rev_ts,
            "opencode" => cfg.last_uploaded_opencode_rev_ts,
            "kimi" => cfg.last_uploaded_kimi_rev_ts,
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

    let resp: RemoteUsage = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(map_http_err("查询远端数据"))?
        .into_json()
        .map_err(|e| format!("解析远端数据失败: {e}"))?;
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

    let resp: SnapshotsResponse = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
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

    let resp: AgentQuotaSnapshotsResponse = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(map_http_err("查询远端 Agent 额度快照"))?
        .into_json()
        .map_err(|e| format!("解析远端 Agent 额度快照失败: {e}"))?;
    Ok(resp.snapshots)
}

/// 拉取设备列表（供前端设备筛选器）。
pub fn fetch_devices() -> Result<Vec<DeviceInfo>, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Ok(Vec::new());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;
    let raw: Vec<DeviceInfoRaw> = ureq::get(&format!("{base}/devices"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
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
        })
        .collect())
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
    let resp: CleanupResult = ureq::post(&format!("{base}/cleanup"))
        .timeout(Duration::from_secs(10))
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
    let resp: MergeResult = ureq::post(&format!("{base}/device/merge"))
        .timeout(Duration::from_secs(15))
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
    let resp: RenameResult = ureq::post(&format!("{base}/device/rename"))
        .timeout(Duration::from_secs(10))
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
pub fn fetch_cleanup_status() -> Result<CleanupStatus, String> {
    let cfg = load_sync_config()?;
    if !cfg.enabled || cfg.device_token.is_empty() {
        return Err("同步未启用或未注册设备".into());
    }
    let base = &cfg.server_url;
    let token = &cfg.device_token;
    let resp: CleanupStatus = ureq::get(&format!("{base}/cleanup/status"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
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
    let resp: AutoCleanupConfig = ureq::post(&format!("{base}/cleanup/config"))
        .timeout(Duration::from_secs(10))
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
/// 刻意不动 `last_uploaded_snapshot_ts` / `last_uploaded_agent_quota_snapshot_ts`：
/// 那是额度快照通道的游标，重置它会让下次注册后重传全部历史快照（行为变更），
/// 与本次任务无关，保持既有语义（只补传本次连接后新增的快照）。
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

/// 把 ureq::Error 转为可读中文错误。
fn map_http_err(op: &str) -> impl Fn(ureq::Error) -> String + '_ {
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
    //! 同步上传通道的单测：**一律不碰网络、不碰真实服务端、不碰真实 ~/.zbar**
    //! （配置读写用 serde 在内存里来回，派生库相关行为由各模块自己的测试覆盖）。
    //! 覆盖四件事：
    //! 1. 游标推进（pump_source_batches，各来源共用同一实现）；
    //! 2. 修订补传扫描（sweep_source_revisions，claude/dsh/opencode/kimi 共用）；
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
            ..Default::default()
        };
        let text = serde_json::to_string(&cfg).expect("序列化失败");
        for key in [
            "last_uploaded_opencode_rowid",
            "last_uploaded_opencode_rev_ts",
            "last_uploaded_kimi_rowid",
            "last_uploaded_kimi_rev_ts",
        ] {
            assert!(text.contains(key), "配置应包含字段 {key}: {text}");
        }
        let back: SyncConfig = serde_json::from_str(&text).expect("反序列化失败");
        assert_eq!(back.last_uploaded_opencode_rowid, 7);
        assert_eq!(back.last_uploaded_opencode_rev_ts, 8);
        assert_eq!(back.last_uploaded_kimi_rowid, 9);
        assert_eq!(back.last_uploaded_kimi_rev_ts, 10);
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
        const SNAPSHOT_CURSORS: [&str; 2] = [
            "last_uploaded_snapshot_ts",
            "last_uploaded_agent_quota_snapshot_ts",
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
}
