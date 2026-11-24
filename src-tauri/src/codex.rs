//! Codex CLI 用量统计模块。
//!
//! 数据来源：Codex CLI 把每个会话记录在 ~/.codex/sessions/年/月/日/rollout-*.jsonl
//! （append-only，每行一个 JSON 事件）。token 用量在 type=event_msg 且
//! payload.type=token_count 的事件里，取 last_token_usage（单次 API 调用值，
//! 逐条求和 == 会话最后一条的 total_token_usage）。
//!
//! 实现方式（原始文件只读 + 派生自有库）：
//! - 原始 jsonl 不做任何修改；把解析结果导入自有库 ~/.agenthud/codex.sqlite
//!   （更名前 ~/.zbar，启动自动迁移），file_progress 表记录每个文件"已处理到的
//!   字节偏移"，之后只增量续读。
//! - (session_id, event_seq) 唯一键 + 冲突 upsert：文件被重写或重复解析时幂等
//!   去重；历史空模型在重新归因后允许只更新模型名。
//! - token_count 事件本身不带模型名，用同文件内最近的 turn_context 事件
//!   payload.model 归因（会话开始前的事件模型留空 ""）。
//! - rate_limits 仅 ChatGPT 订阅登录模式非空（custom provider 全为 null），
//!   最新快照存 rate_limits_state 单行表，resets_at 由 Unix 秒转毫秒。

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{Datelike, Local, TimeZone};

use crate::agent_quota_history;
use crate::db;
use crate::pricing::config_dir;

// ===== 路径定位 =====

/// Codex sessions 目录路径（不做存在性检查，供诊断展示）。
/// 环境变量 ZBAR_CODEX_HOME（指向 .codex 根目录）优先，否则 ~/.codex/sessions。
fn sessions_dir_path() -> PathBuf {
    if let Ok(home) = std::env::var("ZBAR_CODEX_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join("sessions");
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".codex").join("sessions")
}

/// 定位 Codex 会话目录。目录不存在返回友好中文错误（调用方按需容错降级）。
pub fn sessions_dir() -> Result<PathBuf, String> {
    let p = sessions_dir_path();
    if p.is_dir() {
        Ok(p)
    } else {
        Err(format!(
            "未找到 Codex 会话目录: {}。请确认 Codex CLI 已安装并使用过，或设置 ZBAR_CODEX_HOME 环境变量指向 .codex 根目录。",
            p.display()
        ))
    }
}

/// 自有导入库路径：~/.agenthud/codex.sqlite（更名前 ~/.zbar，启动自动迁移）
pub(crate) fn codex_db_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("codex.sqlite"))
}

/// 会话窗口查询（query_window_sessions）内层活跃扫查的行数上限（窗口内
/// 最近 N 笔请求，started_at 降序 LIMIT）：窗口内行数可能很大，以最近 N
/// 笔为界保证查询成本有硬上限；最新会话（排序前几名）必然在内层结果中
const WINDOW_SCAN_ROWS: i64 = 2000;

/// 导入库建表 DDL（`open_codex_db` 与测试共用，避免测试手抄结构漂移）。
/// synchronous=NORMAL 取舍：WAL 模式下仅在 checkpoint 时 fsync，掉电至多
/// 丢最后一个提交的事务；file_progress 与数据行同事务、丢失后一起回退，
/// 幂等重导即可补齐——统计型数据可接受，换来批量导入的写放大大幅下降。
const SCHEMA_SQL: &str = "PRAGMA journal_mode=WAL;
     PRAGMA synchronous=NORMAL;
     CREATE TABLE IF NOT EXISTS model_usage (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL,
        event_seq INTEGER NOT NULL,
        started_at INTEGER NOT NULL,
        model_id TEXT NOT NULL DEFAULT '',
        provider_id TEXT NOT NULL DEFAULT 'codex',
        input_tokens INTEGER NOT NULL DEFAULT 0,
        output_tokens INTEGER NOT NULL DEFAULT 0,
        cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
        cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
        reasoning_tokens INTEGER NOT NULL DEFAULT 0,
        computed_total_tokens INTEGER NOT NULL DEFAULT 0,
        model_revision_at INTEGER NOT NULL DEFAULT 0,
        -- 计时 / 状态取证列（统一口径，与 claude.sqlite 同名同义）：
        -- request_started_at 请求起点（按事件顺序推断）；
        -- first_token_at 首个输出时刻（item_completed 的 started_at_ms）；
        -- completed_at 回复结束时刻（用量记录 / token_count 行时间戳）；
        -- timing_estimated 1 = 起点是推断的（界面带 ≈）；
        -- status 'completed' / 'cancelled' / 'error'（NULL = 未知或进行中）；
        -- error_code 最终失败码或最近一次重试码；retry_count 失败尝试次数。
        duration_ms REAL,
        request_started_at INTEGER,
        first_token_at INTEGER,
        completed_at INTEGER,
        time_to_first_token_ms REAL,
        timing_estimated INTEGER,
        status TEXT,
        error_code TEXT,
        retry_count INTEGER NOT NULL DEFAULT 0,
        UNIQUE(session_id, event_seq)
     );
     CREATE INDEX IF NOT EXISTS idx_codex_model_usage_started ON model_usage(started_at);
     CREATE INDEX IF NOT EXISTS idx_codex_mu_session ON model_usage(session_id);
     CREATE TABLE IF NOT EXISTS file_progress (
        path   TEXT    PRIMARY KEY,
        offset INTEGER NOT NULL,
        size   INTEGER NOT NULL
     );
     CREATE TABLE IF NOT EXISTS session_meta (
        session_id  TEXT PRIMARY KEY,
        cwd         TEXT,
        project_key TEXT,
        captured_at INTEGER
     );
     CREATE TABLE IF NOT EXISTS rate_limits_state (
        id                 INTEGER PRIMARY KEY CHECK (id = 1),
        observed_at        INTEGER NOT NULL,
        plan_type          TEXT,
        primary_pct        REAL,
        primary_reset_at   INTEGER,
        secondary_pct      REAL,
        secondary_reset_at INTEGER
     );";

/// 打开（必要时创建）导入库并确保表结构就绪。这是自有库，读写均可用。
fn open_codex_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = codex_db_path()?;
    let conn = Connection::open(&path).map_err(|e| format!("打开 Codex 导入库失败: {e}"))?;
    // 应用可能在启动时被多个实例同时唤起，结构迁移需要给另一实例提交事务留出时间。
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    conn.execute_batch(SCHEMA_SQL)
        .map_err(|e| format!("初始化 Codex 导入库失败: {e}"))?;
    ensure_model_revision_column(&conn)?;
    ensure_timing_columns(&conn)?;
    Ok(conn)
}

/// 进程内串行化导入库结构迁移；跨进程竞态由 ALTER TABLE 的重复列容错兜底。
static SCHEMA_MIGRATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn schema_migration_lock() -> &'static Mutex<()> {
    SCHEMA_MIGRATION_LOCK.get_or_init(|| Mutex::new(()))
}

/// 为已有导入库补充模型修订时间列。
///
/// 该列只在历史空模型被回填时写入，用于同步客户端把已经上传过的行再次补传。
/// 使用 PRAGMA + ALTER TABLE 而不是版本号，保证旧版数据库和重复启动都能幂等升级。
fn ensure_model_revision_column(conn: &Connection) -> Result<(), String> {
    let _guard = schema_migration_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut stmt = conn
        .prepare("PRAGMA table_info(model_usage)")
        .map_err(|e| format!("检查 Codex 导入库结构失败: {e}"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("检查 Codex 导入库结构失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("检查 Codex 导入库结构失败: {e}"))?;
    drop(stmt);
    if !columns.iter().any(|c| c == "model_revision_at") {
        let result = conn.execute(
            "ALTER TABLE model_usage
             ADD COLUMN model_revision_at INTEGER NOT NULL DEFAULT 0",
            [],
        );
        if let Err(e) = result {
            // 另一个进程可能在本次 PRAGMA 与 ALTER 之间完成了迁移，
            // 此时重复列就是成功状态，后续查询可直接继续。
            let message = e.to_string().to_ascii_lowercase();
            if !message.contains("duplicate column name") {
                return Err(format!("升级 Codex 导入库结构失败: {e}"));
            }
        }
    }
    Ok(())
}

/// 为已有导入库补充计时 / 状态 9 列（与 claude.sqlite 同名同义，见
/// SCHEMA_SQL 的列注释）。旧库升级后由 90 天回填重放补齐历史行；
/// retry_count 带 NOT NULL DEFAULT 0 兜底。PRAGMA table_info 检查后
/// ALTER TABLE，重复列名视为成功，幂等；新库已由建表语句直接带列。
fn ensure_timing_columns(conn: &Connection) -> Result<(), String> {
    let _guard = schema_migration_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut stmt = conn
        .prepare("PRAGMA table_info(model_usage)")
        .map_err(|e| format!("检查 Codex 导入库结构失败: {e}"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("检查 Codex 导入库结构失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("检查 Codex 导入库结构失败: {e}"))?;
    drop(stmt);
    const TIMING_COLUMNS: [(&str, &str); 9] = [
        ("duration_ms", "REAL"),
        ("request_started_at", "INTEGER"),
        ("first_token_at", "INTEGER"),
        ("completed_at", "INTEGER"),
        ("time_to_first_token_ms", "REAL"),
        ("timing_estimated", "INTEGER"),
        ("status", "TEXT"),
        ("error_code", "TEXT"),
        ("retry_count", "INTEGER NOT NULL DEFAULT 0"),
    ];
    for (name, decl) in TIMING_COLUMNS {
        if columns.iter().any(|c| c == name) {
            continue;
        }
        let result =
            conn.execute(&format!("ALTER TABLE model_usage ADD COLUMN {name} {decl}"), []);
        if let Err(e) = result {
            // 另一个进程可能在本次 PRAGMA 与 ALTER 之间完成了迁移，
            // 此时重复列就是成功状态，后续查询可直接继续。
            let message = e.to_string().to_ascii_lowercase();
            if !message.contains("duplicate column name") {
                return Err(format!("升级 Codex 导入库结构失败: {e}"));
            }
        }
    }
    Ok(())
}

// ===== jsonl 事件解析结构（全部 Option/默认值容错，坏行只跳过不中断）=====

/// jsonl 单行事件。payload 类型随 type 不同，先存原始 Value 再按需二次解析。
#[derive(Debug, Deserialize)]
struct RolloutLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    line_type: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

/// event_msg 事件 payload（只关心 token_count 一种）
#[derive(Debug, Deserialize)]
struct TokenCountPayload {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    #[serde(default)]
    info: Option<TokenCountInfo>,
    #[serde(default)]
    rate_limits: Option<RateLimitsPayload>,
}

#[derive(Debug, Deserialize)]
struct TokenCountInfo {
    #[serde(default)]
    last_token_usage: Option<TokenUsage>,
}

/// 单次 API 调用的 token 用量（last_token_usage）。
/// 字段映射到 zcode 口径：cached_input_tokens → cache_read_input_tokens、
/// reasoning_output_tokens → reasoning_tokens、total_tokens → computed_total_tokens。
/// PartialEq 供「用量记录与 token_count 结算逐项一致」的结束时刻判定。
#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
struct TokenUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cached_input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    reasoning_output_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
}

/// ChatGPT 订阅额度快照（custom provider 模式全为 null → Option 化）
#[derive(Debug, Deserialize)]
struct RateLimitsPayload {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    primary: Option<RateWindowPayload>,
    #[serde(default)]
    secondary: Option<RateWindowPayload>,
}

/// 单个额度窗口（primary=5小时，secondary=周）。resets_at 为 Unix 秒。
#[derive(Debug, Deserialize)]
struct RateWindowPayload {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<i64>,
    /// 窗口时长（分钟）。Plus 账号的 primary 也可能是周窗口，不能只按字段名归类。
    #[serde(default)]
    window_minutes: Option<i64>,
}

/// turn_context 事件 payload（token_count 靠它归因模型）
#[derive(Debug, Deserialize)]
struct TurnContextPayload {
    #[serde(default)]
    model: Option<String>,
}

/// session_meta 事件 payload（会话首行，含 cwd 项目目录）
#[derive(Debug, Deserialize)]
struct SessionMetaPayload {
    #[serde(default)]
    cwd: Option<String>,
}

/// 从一行 session_meta 事件中取出非空 cwd（项目维度数据源）。
fn cwd_from_session_meta(line: &RolloutLine) -> Option<String> {
    if line.line_type.as_deref() != Some("session_meta") {
        return None;
    }
    let value = line.payload.as_ref()?;
    serde_json::from_value::<SessionMetaPayload>(value.clone())
        .ok()?
        .cwd
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
}

/// 单独打开文件读首行，解析 session_meta 事件的 cwd。
/// 续读偏移已越过首行的老文件靠这条路径补齐项目维度（不做 fs::canonicalize，
/// 源目录可能已被删除）。
fn read_first_line_cwd(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut buf = Vec::with_capacity(4096);
    use std::io::BufRead;
    let n = reader.read_until(b'\n', &mut buf).ok()?;
    if n == 0 {
        return None;
    }
    let line = serde_json::from_slice::<RolloutLine>(&buf).ok()?;
    cwd_from_session_meta(&line)
}

/// 写入/补齐会话的项目维度记录（幂等）。
/// - 首次：直接插入（cwd 缺失时 project_key 存 NULL，作哨兵防止重复扫描）；
/// - 冲突：仅当库内 project_key 仍为 NULL 且新值非 NULL 时补上（自愈早期
///   无 cwd 的哨兵行），已有值绝不覆盖。
fn upsert_session_meta(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    cwd_raw: Option<&str>,
) -> Result<(), String> {
    let project_key = cwd_raw.and_then(crate::projects::normalize_cwd);
    tx.execute(
        "INSERT INTO session_meta (session_id, cwd, project_key, captured_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(session_id) DO UPDATE SET
            cwd = COALESCE(excluded.cwd, session_meta.cwd),
            project_key = COALESCE(excluded.project_key, session_meta.project_key),
            captured_at = excluded.captured_at
         WHERE session_meta.project_key IS NULL AND excluded.project_key IS NOT NULL",
        rusqlite::params![
            session_id,
            cwd_raw,
            project_key,
            chrono::Local::now().timestamp_millis()
        ],
    )
    .map_err(|e| format!("写入 Codex 会话项目维度失败: {e}"))?;
    Ok(())
}

/// 从一行 turn_context 中取出非空模型名。
fn model_from_turn_context(line: &RolloutLine) -> Option<String> {
    if line.line_type.as_deref() != Some("turn_context") {
        return None;
    }
    let value = line.payload.as_ref()?;
    let context = serde_json::from_value::<TurnContextPayload>(value.clone()).ok()?;
    context.model.filter(|model| !model.is_empty())
}

/// 扫描文件在指定字节偏移之前最近一次出现的模型名。
///
/// Codex 会话文件是追加写入的，但桌面端可能在 turn_context 写入前就触发一次
/// 导入。续读时如果只看导入库中最近一条非空记录，而该会话此前全是空模型，
/// 就会一直丢失上下文。因此这里从文件头恢复一次上下文，避免依赖脏数据自愈。
fn latest_model_before_offset(path: &Path, before_offset: u64) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开 Codex 会话文件失败: {e}"))?;
    let mut reader = std::io::BufReader::new(file);
    let mut pos = 0u64;
    let mut latest = String::new();
    let mut buf = Vec::with_capacity(4096);

    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| format!("读取 Codex 会话文件失败: {e}"))?;
        if n == 0 {
            break;
        }
        pos += n as u64;
        if pos > before_offset {
            break;
        }
        if let Ok(line) = serde_json::from_slice::<RolloutLine>(&buf) {
            if let Some(model) = model_from_turn_context(&line) {
                latest = model;
            }
        }
    }
    Ok(latest)
}

/// 查询用的最新额度快照（resets_at 已转为毫秒）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexRateLimits {
    pub plan_type: Option<String>,
    pub primary_pct: Option<f64>,
    /// 5 小时窗口重置时间（毫秒时间戳）
    pub primary_reset_at: Option<i64>,
    pub secondary_pct: Option<f64>,
    /// 周窗口重置时间（毫秒时间戳）
    pub secondary_reset_at: Option<i64>,
}

/// 事件内 rate_limits → 查询结构（resets_at 秒 → 毫秒）
fn to_rate_limits(r: &RateLimitsPayload) -> CodexRateLimits {
    let mut hour5 = (None, None);
    let mut weekly = (None, None);
    for (fallback_key, window) in [("hour5", &r.primary), ("weekly", &r.secondary)] {
        let Some(window) = window.as_ref() else {
            continue;
        };
        let key = classify_rate_window(window, fallback_key);
        let value = (window.used_percent, window.resets_at.map(|s| s * 1000));
        if key == "weekly" {
            weekly = value;
        } else {
            hour5 = value;
        }
    }
    CodexRateLimits {
        plan_type: r.plan_type.clone(),
        primary_pct: hour5.0,
        primary_reset_at: hour5.1,
        secondary_pct: weekly.0,
        secondary_reset_at: weekly.1,
    }
}

/// 把 Codex CLI 的额度窗口归一到前端的 5h / 周两个稳定窗口键。
/// `primary` 并不总是 5h，Plus 账号实测可能只有一个周窗口。
fn classify_rate_window(window: &RateWindowPayload, fallback_key: &str) -> &'static str {
    if window.window_minutes.unwrap_or(0) >= 2 * 24 * 60 {
        "weekly"
    } else if window.window_minutes.is_some() {
        "hour5"
    } else if fallback_key == "weekly" {
        "weekly"
    } else {
        "hour5"
    }
}

fn agent_windows_from_rate_limits(
    limits: &RateLimitsPayload,
) -> Vec<agent_quota_history::AgentQuotaWindow> {
    let mut windows = Vec::new();
    for (fallback_key, window) in [("hour5", &limits.primary), ("weekly", &limits.secondary)] {
        let Some(window) = window.as_ref() else {
            continue;
        };
        let Some(used_pct) = window.used_percent else {
            continue;
        };
        let key = classify_rate_window(window, fallback_key);
        windows.push(agent_quota_history::AgentQuotaWindow {
            key: key.to_string(),
            used_pct,
            reset_at: window.resets_at.map(|s| s * 1000),
        });
    }
    windows
}

/// ISO8601 时间戳（如 2026-06-22T12:41:14.214Z）→ 毫秒时间戳
fn parse_ts_ms(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// 从文件名提取会话 uuid（rollout-<时间>-<uuid>.jsonl → <uuid>）。
/// 固定取文件主干末 36 个字符（8-4-4-4-12 标准长度），兼容任何前缀；
/// 不依赖 session_meta 事件——续读偏移后首行可能已越过它，文件名始终稳定。
fn session_id_from_filename(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if stem.chars().count() >= 36 {
        let tail: String = stem.chars().rev().take(36).collect();
        let uid: String = tail.chars().rev().collect();
        // 轻量校验：标准 uuid 含 4 个连字符，不匹配则退回完整主干
        if uid.matches('-').count() == 4 {
            return uid;
        }
    }
    stem
}

// ===== 请求计时 / 状态状态机 =====
//
// 从同一文件的 jsonl 事件序列里为每次 API 请求取证耗时、首字、结束时刻、
// 状态与重试。取证思路参考 cc-switch（MIT 许可）的 RequestTimer，规则按
// 本机 Codex CLI 日志的实际事件顺序实现（内部格式未公开，一律缺字段就
// 降级）：
//   task_started → turn_context → response_item(user message) →
//   item_completed(AgentMessage，带 started_at_ms) → response_item(assistant)
//   → token_usage_record → token_count。
// 工具调用型的响应没有输出开始事件，而是 response_item *_call →
// token_usage_record → *_output → token_count；同一用户任务内可连续多次
// 这样的子请求（工具结果之后紧跟下一次模型输出，中间没有新的用户输入）。

/// 同批补写窗口（毫秒）：旧版本 CLI 会把上一请求的输出项成批补写在上一次
/// token_count 之后（不到 100 毫秒），这些项不算下一次请求的证据。
const BATCH_REWRITE_WINDOW_MS: i64 = 100;

/// 续读回看窗口（字节）：请求的边界行可能落在上次已读区域（用户输入之后
/// 等模型输出等了很久），回看重建状态机上下文（只读不写库）。与 claude.rs
/// 的 LOOKBACK_BYTES 同值同语义。
const LOOKBACK_BYTES: u64 = 1024 * 1024;

/// jsonl 行的事件类别（只看行头与 payload 关键字段，缺字段降级为忽略）。
#[derive(Debug, Clone)]
enum LineEvent {
    /// 边界（用户 / 轮次）：turn_context、task_started、非 assistant 的 message
    Boundary,
    /// 工具结果：response_item 且 payload.type 以 _output 结尾
    ToolResult,
    /// 模型输出：assistant 的 message、reasoning、以 _call 结尾的类型
    ModelOutput,
    /// 用量记录：顶层 token_usage_record（payload.usage 存在才有效）
    UsageRecord(TokenUsage),
    /// 输出开始：item_completed 的 item.type 是 Reasoning / AgentMessage，
    /// 携带 payload.started_at_ms
    OutputStart(i64),
    /// 结算：token_count 带 info.last_token_usage（现有写行位置）
    Settle(TokenUsage),
    /// 轮次被用户中断
    TurnAborted,
    /// 请求失败（错误码取 payload.code → payload.error.code → "error"）
    StreamError(String),
}

/// 从 error / stream_error 事件的 payload 里取错误码（缺失降级 "error"）。
fn error_code_from_payload(payload: &serde_json::Value) -> String {
    payload
        .get("code")
        .and_then(|v| v.as_str())
        .or_else(|| {
            payload
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("error")
        .to_string()
}

/// 把一行 rollout 事件分类为状态机输入；无法识别或缺关键字段时返回 None
/// （world_state / additional_tools / configuration_update / task_complete /
/// UserMessage 的 item_completed 等一律忽略）。
fn classify_line(line: &RolloutLine) -> Option<LineEvent> {
    let payload = line.payload.as_ref()?;
    match line.line_type.as_deref() {
        Some("turn_context") => Some(LineEvent::Boundary),
        Some("response_item") => {
            let item_type = payload.get("type")?.as_str()?;
            if item_type == "message" {
                let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                if role == "assistant" {
                    Some(LineEvent::ModelOutput)
                } else {
                    Some(LineEvent::Boundary)
                }
            } else if item_type == "reasoning" || item_type.ends_with("_call") {
                Some(LineEvent::ModelOutput)
            } else if item_type.ends_with("_output") {
                Some(LineEvent::ToolResult)
            } else {
                None
            }
        }
        Some("token_usage_record") => {
            let usage = serde_json::from_value::<TokenUsage>(payload.get("usage")?.clone()).ok()?;
            Some(LineEvent::UsageRecord(usage))
        }
        Some("event_msg") => {
            let msg_type = payload.get("type")?.as_str()?;
            match msg_type {
                "task_started" => Some(LineEvent::Boundary),
                "turn_aborted" => Some(LineEvent::TurnAborted),
                "error" | "stream_error" => {
                    Some(LineEvent::StreamError(error_code_from_payload(payload)))
                }
                "token_count" => {
                    let info = payload.get("info")?;
                    let last = info.get("last_token_usage")?;
                    let usage = serde_json::from_value::<TokenUsage>(last.clone()).ok()?;
                    Some(LineEvent::Settle(usage))
                }
                "item_completed" => {
                    let item_type = payload.get("item")?.get("type")?.as_str()?;
                    if item_type != "Reasoning" && item_type != "AgentMessage" {
                        return None;
                    }
                    Some(LineEvent::OutputStart(payload.get("started_at_ms")?.as_i64()?))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// 一次 token_count 结算的取证产物（状态机 → 写库层的交接物）。
#[derive(Debug, Clone, PartialEq)]
struct SettledUsage {
    /// token_count 行时间戳（model_usage.started_at，时间键含义不变）
    started_at: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    reasoning_tokens: i64,
    total_tokens: i64,
    request_started_at: Option<i64>,
    first_token_at: Option<i64>,
    completed_at: Option<i64>,
    duration_ms: Option<f64>,
    time_to_first_token_ms: Option<f64>,
    /// 起点是推断的 → 1；无起点 → NULL
    timing_estimated: Option<i64>,
    error_code: Option<String>,
    retry_count: i64,
}

/// 无用量结算的 0 token 行（被中断 / 最终失败的请求）。
#[derive(Debug, Clone, PartialEq)]
struct ZeroTokenRow {
    /// 事件时间戳（行的 started_at；event_seq 取它的负值，不占用正序号，
    /// 重放时键保持稳定）
    started_at: i64,
    /// "cancelled" / "error"
    status: &'static str,
    error_code: Option<String>,
    /// 记下待定事件时本请求内已有的失败条数
    retry_count: i64,
}

/// 状态机单次 feed 的产物。
#[derive(Debug, Clone, PartialEq)]
enum TimerOutcome {
    Usage(SettledUsage),
    Zero(ZeroTokenRow),
}

/// 请求内一次失败 / 中断事件（重试计数与最近错误码的原料）。
#[derive(Debug, Clone)]
struct FailureEvent {
    error_code: Option<String>,
}

/// 待定的中断 / 失败事件：单独出 0 token 行还是计入下一行的重试次数，
/// 取决于之后是否出现用量。
#[derive(Debug, Clone)]
struct PendingAbort {
    status: &'static str,
    error_code: Option<String>,
    /// 事件时间戳（0 token 行的时间键）
    at: i64,
    /// 记下时本请求内已有的失败条数（0 token 行的 retry_count 取它）
    prior_failures: i64,
}

/// 单文件的请求计时状态机（纯事件归纳，不触碰 IO 与库，便于单测）。
/// 各状态语义见 feed / settle 注释。
#[derive(Debug, Default)]
struct RequestTimer {
    /// 最近一条边界（用户 / 轮次）或工具结果的时间戳
    last_boundary_at: Option<i64>,
    /// 本请求内最后一个工具结果的时间戳（token_count 延后写出的判定）
    last_tool_result_at: Option<i64>,
    /// 本请求起点：第一条模型输出出现时定格的 last_boundary_at
    request_start: Option<i64>,
    /// 本请求内最后一次错误事件的时刻（error / stream_error）。非空时起点
    /// 改取它：耗时和首字描述的是产生这条回复的那一次请求，不含此前的
    /// 失败尝试与退避等待。用户中断不算错误，不写此字段
    last_failure_at: Option<i64>,
    /// 本请求内最早的输出开始时刻（item_completed 的 started_at_ms 最小值）
    first_output_at: Option<i64>,
    /// 本请求内最后一个模型输出行的时间戳
    last_output_at: Option<i64>,
    /// 本请求内最近一条用量记录（时间戳与用量；结束时刻的第一证据）
    usage_record: Option<(i64, TokenUsage)>,
    /// 上一次结算（token_count）的时间戳（同批补写判定基准）
    last_token_count_at: Option<i64>,
    /// 本请求内的失败 / 中断事件（折入结算行的重试计数）
    failures: Vec<FailureEvent>,
    /// 尚未归属的失败 / 中断事件（之后出现用量 → 计入该行；否则 0 token 行）
    pending: Option<PendingAbort>,
    /// 自上一次结算（含 0 token 行结算）之后是否出现过请求活动
    ///（边界 / 工具结果 / 模型输出任一）
    activity_since_settle: bool,
}

impl RequestTimer {
    /// 喂入一行事件；产生结算产物时返回（调用方负责写库与推进序号；
    /// 回看区调用时丢弃产物即可，状态机内部状态照常推进）。
    fn feed(&mut self, line: &RolloutLine) -> Option<TimerOutcome> {
        let ts = line.timestamp.as_deref().and_then(parse_ts_ms);
        match classify_line(line) {
            Some(LineEvent::Boundary) => {
                let ts = ts?;
                // 边界先结算挂起的失败 / 中断事件（出行判定用结算前的
                // 活动水位），再开启新请求现场
                let outcome = self.finalize_pending();
                self.reset_request();
                self.last_boundary_at = Some(ts);
                self.activity_since_settle = true;
                outcome
            }
            Some(LineEvent::ToolResult) => {
                let ts = ts?;
                self.last_boundary_at = Some(ts);
                self.last_tool_result_at = Some(ts);
                self.activity_since_settle = true;
                None
            }
            Some(LineEvent::ModelOutput) => {
                let ts = ts?;
                // 同批补写：距上一次结算不到 100 毫秒的输出属于上一请求，
                // 不作为本次请求的证据
                if self
                    .last_token_count_at
                    .is_some_and(|prev| ts.saturating_sub(prev) < BATCH_REWRITE_WINDOW_MS)
                {
                    return None;
                }
                // 起点：本次请求第一条模型输出出现时定格，之后不变
                if self.request_start.is_none() {
                    self.request_start = self.last_boundary_at;
                }
                self.last_output_at = Some(ts);
                self.activity_since_settle = true;
                None
            }
            Some(LineEvent::UsageRecord(usage)) => {
                let ts = ts?;
                self.usage_record = Some((ts, usage));
                // 失败 / 中断之后出现了用量 → 归入该行的重试计数，不单独出行
                self.dismiss_pending();
                None
            }
            Some(LineEvent::OutputStart(started_ms)) => {
                // 同批补写：与输出行同规则（基准是上一次结算的时刻）
                if self
                    .last_token_count_at
                    .is_some_and(|prev| started_ms.saturating_sub(prev) < BATCH_REWRITE_WINDOW_MS)
                {
                    return None;
                }
                self.first_output_at =
                    Some(self.first_output_at.map_or(started_ms, |prev| prev.min(started_ms)));
                None
            }
            Some(LineEvent::Settle(usage)) => {
                let ts = ts?;
                self.dismiss_pending();
                let outcome = TimerOutcome::Usage(self.settle(ts, usage));
                // 结算后清空本请求现场；边界与补写基准对齐到本次 token_count
                // 的时间戳（工具结果之后的下一次模型输出就是新的子请求）
                self.reset_request();
                self.last_boundary_at = Some(ts);
                self.last_token_count_at = Some(ts);
                self.activity_since_settle = false;
                Some(outcome)
            }
            Some(LineEvent::TurnAborted) => {
                let ts = ts?;
                self.stash_pending("cancelled", None, ts);
                None
            }
            Some(LineEvent::StreamError(code)) => {
                let ts = ts?;
                self.stash_pending("error", Some(code), ts);
                None
            }
            None => None,
        }
    }

    /// 按 token_count 结算一次请求（此刻各状态字段仍是本请求的现场）。
    /// 结束时刻的取证优先级：用量记录一致 > 工具结果紧跟（token_count 被
    /// 工具执行推后）> token_count 行时间戳。
    fn settle(&mut self, ts: i64, usage: TokenUsage) -> SettledUsage {
        let matched_at = self
            .usage_record
            .as_ref()
            // 1) 本请求内最近一条用量记录与本次结算用量逐项一致 → 用它的时间戳
            .filter(|(_, record)| *record == usage)
            .map(|(at, _)| *at)
            .or_else(|| {
                // 2) 最后一个模型输出之后出现过工具结果、token_count 在工具
                //    结果后 100 毫秒内写出 → token_count 的时间戳被工具执行
                //    推后，取最后一个模型输出的时间戳
                let output = self.last_output_at?;
                let tool = self.last_tool_result_at?;
                (tool > output && ts.saturating_sub(tool) < BATCH_REWRITE_WINDOW_MS)
                    .then_some(output)
            });
        // 3) 其余 → token_count 行时间戳
        let completed_at = matched_at.unwrap_or(ts);
        // 起点：正常请求取第一条模型输出出现前的边界；本请求内出现过错误
        // 事件（重试后成功）时改取最后一次错误事件的时刻——耗时和首字
        // 描述的是「产生这条回复的那一次请求」，不含此前的失败尝试与退避
        // 等待（错误事件出现时此前的输出证据已整体作废，见 stash_pending；
        // 用户中断不作废证据，不影响此分支）
        let request_started_at = self.last_failure_at.or(self.request_start);
        // duration = completed_at − request_started_at：两者都有且差值在合法
        // 生成窗口内才写（不满足置空，不做钳制）
        let duration_ms = request_started_at.and_then(|start| {
            let span = completed_at - start;
            (span >= crate::token_speed::MIN_GEN_MS && span <= crate::token_speed::MAX_GEN_MS)
                .then_some(span as f64)
        });
        // 首字：起点已知且时序不合法（first <= start 或 first > completed）
        // 时置空；起点未知时保留时刻本身（速度由读库方按两时刻判定）
        let mut first_token_at = self.first_output_at;
        if let Some(start) = request_started_at {
            if first_token_at.is_some_and(|first| first <= start) {
                first_token_at = None;
            }
            if first_token_at.is_some_and(|first| first > completed_at) {
                first_token_at = None;
            }
        }
        // ttft = first_token_at − request_started_at：首字必须落在起点与结束
        // 之间（起点未知时不给）
        let time_to_first_token_ms = match (request_started_at, first_token_at) {
            (Some(start), Some(first)) if start < first && first <= completed_at => {
                Some((first - start) as f64)
            }
            _ => None,
        };
        SettledUsage {
            started_at: ts,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cached_input_tokens,
            reasoning_tokens: usage.reasoning_output_tokens,
            total_tokens: usage.total_tokens,
            request_started_at,
            first_token_at,
            completed_at: Some(completed_at),
            duration_ms,
            time_to_first_token_ms,
            timing_estimated: request_started_at.map(|_| 1),
            error_code: self.failures.last().and_then(|f| f.error_code.clone()),
            retry_count: self.failures.len() as i64,
        }
    }

    /// 记一个待定的失败 / 中断事件：事件条目同时进本请求的失败列表
    ///（之后出现用量 / 结算时折入该行的 retry_count）；已有待定时先转正
    ///（它之后又出现了新失败，说明请求在重试，不再单独出行）。
    /// 错误类（error / stream_error）之后可能重试：作废失败前的输出证据，
    /// 起点改取错误时刻、首个输出只取失败之后的（耗时和首字不含失败尝试
    /// 与退避等待）。用户中断（turn_aborted）不会有重试，保留原有证据——
    /// 中断后不经边界直接出现的用量结算仍按正常规则取证。
    fn stash_pending(&mut self, status: &'static str, error_code: Option<String>, at: i64) {
        self.dismiss_pending();
        let prior_failures = self.failures.len() as i64;
        self.failures.push(FailureEvent {
            error_code: error_code.clone(),
        });
        self.pending = Some(PendingAbort {
            status,
            error_code,
            at,
            prior_failures,
        });
        if status == "error" {
            self.last_failure_at = Some(at);
            self.request_start = None;
            self.first_output_at = None;
            self.last_output_at = None;
            self.last_tool_result_at = None;
            self.usage_record = None;
        }
    }

    /// 挂起的失败 / 中断事件之后出现了用量 / 结算：计入该行的重试计数
    ///（事件条目在 stash 时已进 failures），不再单独出行。
    fn dismiss_pending(&mut self) {
        self.pending = None;
    }

    /// 结算挂起的失败 / 中断事件：自上一次结算之后出现过请求活动（边界 /
    /// 工具结果 / 模型输出任一）→ 写一条 0 token 行；空闲时按了中断 →
    /// 不出行。边界行与文件读尽（EOF）两处共用。
    fn finalize_pending(&mut self) -> Option<TimerOutcome> {
        let pending = self.pending.take()?;
        if !self.activity_since_settle {
            return None;
        }
        Some(TimerOutcome::Zero(ZeroTokenRow {
            started_at: pending.at,
            status: pending.status,
            error_code: pending.error_code,
            retry_count: pending.prior_failures,
        }))
    }

    /// 文件读尽时的结算：只有用户中断（turn_aborted）出行。错误 /
    /// 流错误之后可能跟着重试，立即出行会把「重试过但最终成功的请求」
    /// 拆成两行——留给下一条边界结算（续读时回看区会重建挂起状态）。
    /// 代价：错误之后会话再无任何新内容时这条失败不入库（本机日志尚
    /// 未出现过 error / stream_error 事件，取舍可接受）。
    fn finalize_pending_at_eof(&mut self) -> Option<TimerOutcome> {
        if self.pending.as_ref()?.status != "cancelled" {
            return None;
        }
        self.finalize_pending()
    }

    /// 重置本请求的现场状态（边界 / 结算时调用）。last_boundary_at、
    /// last_token_count_at 与活动水位由调用方按需推进；pending 不在此清
    ///（0 token 行的结算时机由 finalize_pending 负责）。
    fn reset_request(&mut self) {
        self.request_start = None;
        self.first_output_at = None;
        self.last_output_at = None;
        self.last_tool_result_at = None;
        self.usage_record = None;
        self.failures.clear();
        self.last_failure_at = None;
    }
}

// ===== 增量导入 =====

/// 导入互斥锁：面板查询 / 托盘标题刷新 / 同步上传可能并发触发导入，
/// 串行化避免同一文件被双份解析（冲突 upsert 可去重，但重复 IO 浪费）。
static IMPORT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn import_lock() -> &'static Mutex<()> {
    IMPORT_LOCK.get_or_init(|| Mutex::new(()))
}

/// 上次导入时间（节流用）。各查询入口（query_stats/query_trend/…）都会触发
/// 导入，前端 30s 一轮 × 4 个预设范围 × 多个命令会放大到十余次"锁 + 开库 +
/// 递归扫目录"；会话文件是分钟级追加，5 秒节流足够实时且省掉重复扫描。
static LAST_IMPORT_AT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
static LAST_RATE_LIMIT_BACKFILL_DAY: OnceLock<Mutex<Option<i32>>> = OnceLock::new();

fn last_import_at() -> &'static Mutex<Option<std::time::Instant>> {
    LAST_IMPORT_AT.get_or_init(|| Mutex::new(None))
}

/// 增量导入（查询入口用，5 秒节流；失败也计入节流窗口，避免故障时重试风暴）。
pub fn import_incremental() -> Result<(), String> {
    {
        let mut last = last_import_at()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.map(|t| t.elapsed() < std::time::Duration::from_secs(5)) == Some(true) {
            return Ok(());
        }
        *last = Some(std::time::Instant::now());
    }
    import_incremental_force()
}

/// 递归收集 sessions 目录下所有 rollout-*.jsonl。
/// 目录层级为 年/月/日 三层，逐层手动遍历（不引入额外依赖）；
/// 每层同时收集匹配文件，兼容布局微调。结果排序保证导入顺序稳定。
fn collect_session_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // 最多下钻 3 层（年/月/日），防御异常深层嵌套
            if depth > 0 {
                collect_session_files(&path, depth - 1, out);
            }
        } else {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                out.push(path);
            }
        }
    }
}

/// 导入驱动：每个文件一个事务（保持旧原子性边界——文件中途失败
/// （读取/写入/磁盘异常）时该文件已写行整体回滚，下次从旧偏移重导，
/// 续读的序号恢复始终基于干净数据，绝不产生序号漂移的孤儿行）。
/// - import 闭包返回 Err 时本函数记日志并丢弃事务（drop 即回滚），
/// 不阻断其他文件（对齐原有「单文件失败跳过」语义）；
/// - commit 失败同样记日志继续（该文件随事务回滚，下次重试）；
/// - prepared statement 复用不受影响：语句在 import_one_file 内按文件
/// prepare，提交由本函数在语句析构后进行。
/// 注：WAL + synchronous=NORMAL 下 commit 不再逐次 fsync，单文件事务的
/// 提交开销已足够低，无需为此上移事务边界引入孤儿行风险。
fn run_import_files<F>(
    conn: &mut Connection,
    files: &[PathBuf],
    fail_log_prefix: &str,
    import: F,
) -> Result<(), String>
where
    F: Fn(&mut Transaction<'_>, &Path) -> Result<(), String>,
{
    for path in files {
        let mut tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| format!("开启导入事务失败: {e}"))?;
        if let Err(e) = import(&mut tx, path) {
            // 丢弃事务（循环体末尾 drop）→ 本文件已写行整体回滚
            eprintln!(
                "[agenthud-codex] {fail_log_prefix} {} 失败（已回滚，下次重试）: {e}",
                path.display()
            );
            continue;
        }
        if let Err(e) = tx.commit() {
            eprintln!(
                "[agenthud-codex] 提交导入事务失败（{}，下次重试）: {e}",
                path.display()
            );
        }
    }
    Ok(())
}

/// 增量导入（不节流）：遍历 sessions 目录，把每个 rollout 文件新增部分解析入库。
/// 查询入口应调用节流版 import_incremental；诊断等需要"一定执行"的场景用本函数。
/// - file_progress 记录"已处理到的字节偏移"（对齐完整行末尾）；文件变短
///   （被重写）时从头重新解析，UNIQUE 键保证幂等。
/// - 续读时 event_seq 从该会话已入库的最大序号继续，避免序号回退导致
///   冲突 upsert 静默吞掉新事件。
/// - 每个文件一个事务：中途崩溃/失败整体回滚，下次从旧偏移重来
///   （run_import_files）。
/// - 单文件失败（被占用/磁盘异常）只记日志跳过，不阻断其他文件。
pub fn import_incremental_force() -> Result<(), String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = sessions_dir()?;
    let mut conn = open_codex_db()?;

    // 预载全部文件进度（单次查询；文件数通常几百，内存开销可忽略）
    let mut progress: HashMap<String, (u64, u64)> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT path, offset, size FROM file_progress")
            .map_err(|e| format!("读取导入进度失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| format!("读取导入进度失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取导入进度失败: {e}"))?;
        for (p, off, sz) in rows {
            progress.insert(p, (off.max(0) as u64, sz.max(0) as u64));
        }
    }

    let mut files = Vec::new();
    collect_session_files(&dir, 3, &mut files);
    files.sort();

    // 计时 / 状态一次性回填（PRAGMA user_version 标记）：标记 < 2 时，对
    // 最近 90 天修改过的文件无视已有偏移从头重放（已存在的行在 token 逐项
    // 一致时原地补计时，重放幂等）。跑完才写标记：个别文件失败照常记日志
    // 跳过、标记照写；中途退出则标记没写、下次重来。**不删 file_progress
    // 行**——正在运行的旧版实例会抢先重建进度，导致回填漏掉文件。（与
    // claude.rs 同款做法，两个库的 user_version 互不影响；.jsonl.zst 本来
    // 就不导入，不处理。）
    let backfill_needed = timing_backfill_pending(&conn);
    let backfill_cutoff = now_ms_epoch() - TIMING_BACKFILL_WINDOW_MS;

    run_import_files(&mut conn, &files, "导入", |tx, path| {
        let key = path.to_string_lossy().to_string();
        let known = if backfill_needed && file_modified_after(path, backfill_cutoff) {
            None
        } else {
            progress.get(&key).copied()
        };
        import_one_file(tx, path, known)
    })?;

    if backfill_needed {
        mark_timing_backfill_done(&mut conn);
    }

    // 历史版本可能在模型上下文尚未写入时就落了空 model_id。对这些会话重扫
    // 原始文件，借助同一套事件序号和 upsert 逻辑把模型名补回；没有模型上下文
    // 的文件跳过，避免每次查询都重复扫描无法修复的数据。
    let blank_sessions: HashSet<String> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT session_id FROM model_usage WHERE model_id = ''")
            .map_err(|e| format!("查询待修复 Codex 会话失败: {e}"))?;
        let sessions = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询待修复 Codex 会话失败: {e}"))?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|e| format!("查询待修复 Codex 会话失败: {e}"))?;
        sessions
    };
    run_import_files(&mut conn, &files, "回填模型名", |tx, path| {
        let session_id = session_id_from_filename(path);
        if !blank_sessions.contains(&session_id) {
            return Ok(());
        }
        let has_model = latest_model_before_offset(path, u64::MAX)
            .map(|model| !model.is_empty())
            .unwrap_or(false);
        if !has_model {
            return Ok(());
        }
        import_one_file(tx, path, None)
    })?;

    // 存量会话的项目维度回填（节流 + 每批限量，幂等说明见函数注释）
    if let Err(e) = backfill_session_meta_locked(&mut conn) {
        eprintln!("[agenthud-codex] 会话项目维度回填失败（下次重试）: {e}");
    }
    Ok(())
}

/// 计时回填的 PRAGMA user_version 标记值（1 留给历史迁移语义，本轮写 2；
/// claude.sqlite 用同名标记但库文件独立，互不影响）
const TIMING_BACKFILL_VERSION: i64 = 2;
/// 计时回填只重放最近修改过的文件（90 天；更早的保持现状）
const TIMING_BACKFILL_WINDOW_MS: i64 = 90 * 24 * 3_600_000;

/// 当前毫秒时间戳（回填窗口判定用；导入锁内调用，无并发问题）
fn now_ms_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 计时回填标记判定：PRAGMA user_version < 2 时需要重放（读取失败视为需要，
/// 失败仅影响本轮多做一次幂等重放，不丢数据）
fn timing_backfill_pending(conn: &Connection) -> bool {
    conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map(|version| version < TIMING_BACKFILL_VERSION)
        .unwrap_or(true)
}

/// 写入计时回填完成标记（失败只记日志：下轮导入多做一次幂等重放）
fn mark_timing_backfill_done(conn: &mut Connection) {
    let result = conn.execute_batch(&format!("PRAGMA user_version = {TIMING_BACKFILL_VERSION};"));
    if let Err(e) = result {
        eprintln!("[agenthud-codex] 写入回填标记失败（下次导入会重放一遍）: {e}");
    }
}

/// 文件修改时间是否在 cutoff 之后（读不到元信息按否处理，保持原偏移续读）
fn file_modified_after(path: &Path, cutoff_ms: i64) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_millis() as i64 >= cutoff_ms)
        .unwrap_or(false)
}

/// 上次会话项目维度回填时间（节流用）。
static LAST_SESSION_META_BACKFILL_AT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();

/// 存量会话项目维度回填（调用方必须已持有 import_lock）。
///
/// 幂等性保证（重复调用不重复读文件）：
/// - 只选 model_usage 中存在、session_meta 中完全缺失的会话（LEFT JOIN NULL
///   过滤），已回填过的会话天然不在候选集里；
/// - 每个候选会话无论是否找到 cwd 都写入 session_meta（找不到时 project_key
///   为 NULL 的哨兵行），下一轮候选查询即把它排除，同一会话只做一次首行 IO；
/// - upsert 的冲突分支仅在「库内 project_key 为 NULL 且新值非空」时补值，
///   重复执行不会覆盖已回填的值；
/// - 30 秒节流 + 每批最多 500 个会话：前端 30s 轮询不会放大成持续文件 IO，
///   量大时靠多轮调用分批消化，避免卡 UI。
fn backfill_session_meta_locked(conn: &mut Connection) -> Result<usize, String> {
    {
        let mut last = LAST_SESSION_META_BACKFILL_AT
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.map(|t| t.elapsed() < std::time::Duration::from_secs(30)) == Some(true) {
            return Ok(0);
        }
        *last = Some(std::time::Instant::now());
    }

    let missing: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT mu.session_id
                 FROM model_usage mu
                 LEFT JOIN session_meta sm ON sm.session_id = mu.session_id
                 WHERE sm.session_id IS NULL
                 LIMIT 500",
            )
            .map_err(|e| format!("查询待回填 Codex 会话失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询待回填 Codex 会话失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("查询待回填 Codex 会话失败: {e}"))?;
        rows
    };
    if missing.is_empty() {
        return Ok(0);
    }

    // session_id → 源文件映射（目录已消失/文件被删的会话落 NULL 哨兵）
    let dir = sessions_dir_path();
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_session_files(&dir, 3, &mut files);
    }
    let mut by_session: HashMap<String, &Path> = HashMap::new();
    for path in &files {
        by_session.entry(session_id_from_filename(path)).or_insert(path);
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启回填事务失败: {e}"))?;
    for session_id in &missing {
        let cwd = by_session
            .get(session_id.as_str())
            .and_then(|path| read_first_line_cwd(path));
        upsert_session_meta(&tx, session_id, cwd.as_deref())?;
    }
    tx.commit()
        .map_err(|e| format!("提交回填事务失败: {e}"))?;
    Ok(missing.len())
}

/// 存量会话项目维度回填（公开入口，项目浏览器查询前调用）。
/// 与增量导入共用 IMPORT_LOCK 串行；内部自带节流与分批限量。
pub fn backfill_session_meta() -> Result<usize, String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut conn = open_codex_db()?;
    backfill_session_meta_locked(&mut conn)
}

/// 把今天 Codex CLI 会话中已经记录的有效额度快照补入 Agent 历史。
///
/// 额度历史功能可能在用户当天已经使用了一段时间后才启动；如果只从
/// `wham/usage` 的首次实时采样开始，今日早先的真实使用会被误当成今日
/// 起点。Codex CLI 自己会在每次 token_count 事件中记录 rate_limits，
/// 这些是有效的提供方快照，不是网络失败时的旧回退值，因此可以安全补齐。
/// 每个本地日期最多扫描一次，重复启动通过历史中的“来源 + 同秒”集合去重。
pub fn backfill_today_rate_limit_history() -> Result<usize, String> {
    let now = Local::now();
    let day = now.date_naive();
    let day_key = day.num_days_from_ce();
    let backfill_day = LAST_RATE_LIMIT_BACKFILL_DAY.get_or_init(|| Mutex::new(None));
    {
        let guard = backfill_day
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *guard == Some(day_key) {
            return Ok(0);
        }
    }

    let Some(day_start) = day
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .map(|value| value.timestamp_millis())
    else {
        return Ok(0);
    };
    let day_dir = sessions_dir_path()
        .join(format!("{:04}", day.year()))
        .join(format!("{:02}", day.month()))
        .join(format!("{:02}", day.day()));
    if !day_dir.is_dir() {
        return Ok(0);
    }

    let mut files = Vec::new();
    collect_session_files(&day_dir, 0, &mut files);
    files.sort();
    if files.is_empty() {
        return Ok(0);
    }

    let mut snapshots = Vec::new();
    for path in files {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        for line in std::io::BufReader::new(file).lines().flatten() {
            let Ok(rollout) = serde_json::from_str::<RolloutLine>(&line) else {
                continue;
            };
            if rollout.line_type.as_deref() != Some("event_msg") {
                continue;
            }
            let Some(payload) = rollout.payload else {
                continue;
            };
            let Ok(token_count) = serde_json::from_value::<TokenCountPayload>(payload) else {
                continue;
            };
            if token_count.msg_type.as_deref() != Some("token_count") {
                continue;
            }
            let Some(limits) = token_count.rate_limits.as_ref() else {
                continue;
            };
            let Some(ts) = rollout.timestamp.as_deref().and_then(parse_ts_ms) else {
                continue;
            };
            if ts < day_start || ts > now.timestamp_millis() {
                continue;
            }
            let windows = agent_windows_from_rate_limits(limits);
            if windows.is_empty() {
                continue;
            }
            snapshots.push(agent_quota_history::AgentQuotaSnapshot {
                source: "codex".to_string(),
                ts,
                plan_type: limits.plan_type.clone(),
                windows,
                label: None,
                plan_name: None,
                balance: None,
                message: None,
            });
        }
    }
    snapshots.sort_by_key(|snapshot| snapshot.ts);

    let mut existing_seconds: HashSet<i64> = agent_quota_history::load_all()?
        .into_iter()
        .filter(|snapshot| snapshot.source == "codex")
        .map(|snapshot| snapshot.ts / 1000)
        .collect();
    let mut added = 0usize;
    for snapshot in snapshots {
        let second = snapshot.ts / 1000;
        if !existing_seconds.insert(second) {
            continue;
        }
        agent_quota_history::append_snapshot(&snapshot);
        added += 1;
    }

    let mut guard = backfill_day
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(day_key);
    Ok(added)
}

/// 解析单个 rollout 文件的增量部分。known = 上次记录的 (offset, size)。
/// 事务由调用方的 run_import_files 按文件提供（每文件一个，失败整体
/// 回滚，见 import_incremental_force）；行级 INSERT 在循环前 prepare 一次
/// 复用，避免每行重复解析 SQL。
/// 计时 / 状态取证：逐行喂 RequestTimer 状态机，token_count 结算一行一条
/// model_usage（契约不变），中断 / 失败在没有用量时补 0 token 行；续读时
/// 先回看游标前最多 1MB 重建上下文（feed_lookback，产物丢弃）。
fn import_one_file(
    tx: &Transaction<'_>,
    path: &Path,
    known: Option<(u64, u64)>,
) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("读取文件元信息失败: {e}"))?
        .len();

    // 无进度记录，或文件比记录时更短（被重写）→ 从头解析（序号从 1 重计，幂等）
    let (start_offset, reparse) = match known {
        Some((off, prev_size)) if size >= prev_size => (off, false),
        _ => (0, true),
    };
    if !reparse && start_offset >= size {
        return Ok(()); // 文件无新增内容
    }

    let session_id = session_id_from_filename(path);

    let mut file = std::fs::File::open(path).map_err(|e| format!("打开会话文件失败: {e}"))?;
    let mut reader = std::io::BufReader::new(&mut file);

    // 续读时 event_seq 接着该会话已入库的最大序号继续（唯一键稳定）；
    // 0 token 行的序号是负的事件时间戳毫秒，这里只统计正序号——否则首个
    // 请求被中断的会话续读后所有用量行都会落到负序号区间，之后从头重放
    // （文件被重写触发 reparse，从 seq=1 重排）会因不冲突而整段重复入库。
    // 同时恢复该会话最近一次归因的模型名。若历史记录全为空，则从原始文件
    // 的 start_offset 之前恢复最近上下文，避免继续把新增 token 归到空模型。
    let mut seq: i64 = 0;
    let mut current_model = String::new();
    if !reparse {
        let _ = tx.query_row(
            "SELECT COALESCE(MAX(CASE WHEN event_seq > 0 THEN event_seq END), 0),
                    COALESCE((SELECT model_id FROM model_usage
                              WHERE session_id = ?1 AND model_id != ''
                              ORDER BY event_seq DESC LIMIT 1), '')
             FROM model_usage WHERE session_id = ?1",
            rusqlite::params![session_id],
            |row| {
                seq = row.get(0)?;
                current_model = row.get(1)?;
                Ok(())
            },
        );
        if current_model.is_empty() {
            current_model = latest_model_before_offset(path, start_offset)?;
        }
    }

    // 项目维度：该会话尚无有效 project_key 时，单独读文件首行的 session_meta
    // 事件提取 cwd（每会话仅一次首行 IO；找到后 upsert 的冲突分支不再触发，
    // 找不到也落 NULL 哨兵行，重复导入不会重复读）。
    let has_project_key: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session_meta
                          WHERE session_id = ?1 AND project_key IS NOT NULL)",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !has_project_key {
        let cwd = read_first_line_cwd(path);
        upsert_session_meta(tx, &session_id, cwd.as_deref())?;
    }

    let mut pos = start_offset;
    let mut last_complete_end = start_offset;
    // 本文件内时间最新的非空额度快照（落库前与已存值比 observed_at 不回退）
    let mut latest_limits: Option<(CodexRateLimits, i64)> = None;
    // 仅在已有空模型行被补回时写入，供同步客户端补传修订行。取数据库中
    // 现有最大值 + 1，保证即使系统时钟回拨或同一毫秒发生多次修订，游标仍
    // 能按严格递增顺序工作。
    let max_model_revision: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(model_revision_at), 0) FROM model_usage",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("读取 Codex 模型修订游标失败: {e}"))?;
    let model_revision_at = chrono::Local::now()
        .timestamp_millis()
        .max(max_model_revision.saturating_add(1));
    let mut pending_model_seqs: Vec<i64> = Vec::new();

    // 行级 INSERT 预编译一次，循环内仅绑定执行（prepare 保留不可变借用，
    // 与循环内低频的 tx.execute 互不冲突，commit 由调用方在语句析构后进行）。
    // 冲突分支两种情况：
    // - 补空模型名（原行为不变）：历史空模型行被重新归因时更新模型名并打
    //   修订标记（?19），供同步客户端补传修订行；
    // - token 五项逐项相等（重放命中已有行）：给值为空的计时 / 状态列补上
    //   新值（COALESCE 已入库优先）、retry_count 取较大者。token 不相等的
    //   行绝不动——防止文件被改写后把计时贴到别的行上。
    let mut insert_usage = tx
        .prepare(
            "INSERT INTO model_usage
                (session_id, event_seq, started_at, model_id, provider_id,
                 input_tokens, output_tokens, cache_read_input_tokens,
                 cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                 duration_ms, request_started_at, first_token_at, completed_at,
                 time_to_first_token_ms, timing_estimated, status, error_code,
                 retry_count, model_revision_at)
             VALUES (?1, ?2, ?3, ?4, 'codex', ?5, ?6, ?7, 0, ?8, ?9,
                     ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, 0)
             ON CONFLICT(session_id, event_seq) DO UPDATE SET
                 model_id = CASE
                     WHEN model_usage.model_id = '' AND excluded.model_id != ''
                     THEN excluded.model_id ELSE model_usage.model_id END,
                 model_revision_at = CASE
                     WHEN model_usage.model_id = '' AND excluded.model_id != ''
                     THEN ?19 ELSE model_usage.model_revision_at END,
                 duration_ms = COALESCE(model_usage.duration_ms, excluded.duration_ms),
                 request_started_at = COALESCE(model_usage.request_started_at, excluded.request_started_at),
                 first_token_at = COALESCE(model_usage.first_token_at, excluded.first_token_at),
                 completed_at = COALESCE(model_usage.completed_at, excluded.completed_at),
                 time_to_first_token_ms = COALESCE(model_usage.time_to_first_token_ms, excluded.time_to_first_token_ms),
                 timing_estimated = COALESCE(model_usage.timing_estimated, excluded.timing_estimated),
                 status = COALESCE(model_usage.status, excluded.status),
                 error_code = COALESCE(model_usage.error_code, excluded.error_code),
                 retry_count = MAX(model_usage.retry_count, excluded.retry_count)
             WHERE (model_usage.model_id = '' AND excluded.model_id != '')
                OR (model_usage.input_tokens = excluded.input_tokens
                    AND model_usage.output_tokens = excluded.output_tokens
                    AND model_usage.cache_read_input_tokens = excluded.cache_read_input_tokens
                    AND model_usage.reasoning_tokens = excluded.reasoning_tokens
                    AND model_usage.computed_total_tokens = excluded.computed_total_tokens)",
        )
        .map_err(|e| format!("预编译用量写入语句失败: {e}"))?;

    // 请求计时状态机：从事件序列取证耗时 / 首字 / 状态 / 重试（结算产物
    // 由主循环写库；回看区调用只重建上下文，产物丢弃）
    let mut timer = RequestTimer::default();

    // 续读回看（start_offset > 0）：请求的边界行可能在上次已读部分（用户
    // 输入之后等模型输出等了很久），从游标前最多 1MB 读到游标重建上下文
    //（last_boundary_at / 待定失败事件等）。这段的结算与 0 token 行一律
    // 丢弃——不写库、不推进序号；进度偏移仍只按新增字节推进。
    if !reparse && start_offset > 0 {
        feed_lookback(&mut reader, start_offset, &mut timer)?;
    }
    reader
        .seek(SeekFrom::Start(start_offset))
        .map_err(|e| format!("定位读取偏移失败: {e}"))?;

    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| format!("读取会话文件失败: {e}"))?;
        if n == 0 {
            break; // EOF
        }
        pos += n as u64;
        // 只把"完整行"（带换行符）计入进度：末尾半行留待下次追加完整后重读
        let complete = buf.last() == Some(&b'\n');
        if complete {
            last_complete_end = pos;
        }

        // 逐行容错：JSON 解析失败/字段异常只跳过该行（偏移仍推进）
        let Ok(line) = serde_json::from_slice::<RolloutLine>(&buf) else {
            continue;
        };
        // 计时 / 状态状态机：所有类别都要喂（边界 / 工具结果 / 输出项都是
        // 耗时证据），产生结算产物时立即写库（一个 token_count 对应一行
        // model_usage 的契约不变）
        if let Some(outcome) = timer.feed(&line) {
            match outcome {
                TimerOutcome::Usage(settled) => {
                    seq += 1;
                    insert_usage
                        .execute(rusqlite::params![
                            session_id,
                            seq,
                            settled.started_at,
                            current_model,
                            settled.input_tokens,
                            settled.output_tokens,
                            settled.cache_read_tokens,
                            settled.reasoning_tokens,
                            settled.total_tokens,
                            settled.duration_ms,
                            settled.request_started_at,
                            settled.first_token_at,
                            settled.completed_at,
                            settled.time_to_first_token_ms,
                            settled.timing_estimated,
                            "completed",
                            settled.error_code,
                            settled.retry_count,
                            model_revision_at,
                        ])
                        .map_err(|e| format!("写入用量记录失败: {e}"))?;
                    if current_model.is_empty() {
                        pending_model_seqs.push(seq);
                    }
                }
                TimerOutcome::Zero(zero) => {
                    // 0 token 行：event_seq 取负的事件时间戳毫秒（不占用正
                    // 序号，续读不需要行号，重放时键不变）；冲突时 token 全
                    // 0 逐项相等走补空分支，值全同等于不改动
                    insert_usage
                        .execute(rusqlite::params![
                            session_id,
                            -zero.started_at,
                            zero.started_at,
                            current_model,
                            0i64,
                            0i64,
                            0i64,
                            0i64,
                            0i64,
                            None::<f64>,
                            None::<i64>,
                            None::<i64>,
                            None::<i64>,
                            None::<f64>,
                            None::<i64>,
                            zero.status,
                            zero.error_code,
                            zero.retry_count,
                            0i64,
                        ])
                        .map_err(|e| format!("写入中断用量记录失败: {e}"))?;
                    if current_model.is_empty() {
                        pending_model_seqs.push(-zero.started_at);
                    }
                }
            }
        }
        match line.line_type.as_deref() {
            // turn_context：更新当前模型（token_count 不带模型，靠它归因）
            Some("turn_context") => {
                if let Some(model) = model_from_turn_context(&line) {
                    current_model = model.clone();
                    // 若本次文件扫描中 token_count 先于 turn_context，先前插入的
                    // 空模型也属于这个会话；拿到首个模型上下文后立即补回，兼容
                    // Codex 文件出现事件顺序暂时不稳定的情况。
                    for pending_seq in pending_model_seqs.drain(..) {
                        tx.execute(
                            "UPDATE model_usage
                             SET model_id = ?1, model_revision_at = ?2
                             WHERE session_id = ?3 AND event_seq = ?4 AND model_id = ''",
                            rusqlite::params![
                                &model,
                                model_revision_at,
                                session_id,
                                pending_seq,
                            ],
                        )
                        .map_err(|e| format!("回填 Codex 模型名失败: {e}"))?;
                    }
                }
            }
            // event_msg + token_count：用量与计时结算已由状态机处理，这里
            // 只负责提取额度快照（条件与旧实现一致：缺 last_token_usage /
            // 缺时间戳的行不更新快照）
            Some("event_msg") => {
                let Some(v) = line.payload.as_ref() else { continue };
                let Ok(p) = serde_json::from_value::<TokenCountPayload>(v.clone()) else {
                    continue;
                };
                if p.msg_type.as_deref() != Some("token_count") {
                    continue;
                }
                let has_usage = p
                    .info
                    .as_ref()
                    .and_then(|i| i.last_token_usage.as_ref())
                    .is_some();
                if !has_usage {
                    continue;
                }
                // 没有时间戳无法归入统计区间，跳过（偏移仍推进）
                let Some(started_at) = line.timestamp.as_deref().and_then(parse_ts_ms) else {
                    continue;
                };

                // 仅 ChatGPT 订阅登录模式 primary 非空；custom provider 全为 null
                if let Some(limits) = p.rate_limits.as_ref() {
                    if limits.primary.is_some() {
                        latest_limits = Some((to_rate_limits(limits), started_at));
                    }
                }
            }
            _ => {}
        }
    }

    // 文件读尽：只结算仍挂起的用户中断（turn_aborted → cancelled 0 token
    // 行，自上一次结算之后出现过请求活动才出行）。错误 / 流错误不在此
    // 出行——之后可能跟着重试，立即结算会把重试成功的请求拆成两行，留给
    // 下一条边界结算（续读时回看区会重建挂起状态）
    if let Some(TimerOutcome::Zero(zero)) = timer.finalize_pending_at_eof() {
        insert_usage
            .execute(rusqlite::params![
                session_id,
                -zero.started_at,
                zero.started_at,
                current_model,
                0i64,
                0i64,
                0i64,
                0i64,
                0i64,
                None::<f64>,
                None::<i64>,
                None::<i64>,
                None::<i64>,
                None::<f64>,
                None::<i64>,
                zero.status,
                zero.error_code,
                zero.retry_count,
                0i64,
            ])
            .map_err(|e| format!("写入中断用量记录失败: {e}"))?;
        if current_model.is_empty() {
            pending_model_seqs.push(-zero.started_at);
        }
    }
    drop(insert_usage);

    // 进度：对齐到最后一条完整行末尾（末尾半行下次重读，靠唯一键幂等）。
    // 与本文件数据行同事务提交，中途失败一起回滚（断点语义不变）。
    let key = path.to_string_lossy().to_string();
    tx.execute(
        "INSERT INTO file_progress (path, offset, size) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET offset = ?2, size = ?3",
        rusqlite::params![key, last_complete_end as i64, size as i64],
    )
    .map_err(|e| format!("记录导入进度失败: {e}"))?;

    // 最新额度快照落库（observed_at 不回退，防止旧会话续写覆盖新快照）
    if let Some((snap, observed_at)) = latest_limits {
        let stored: Option<i64> = tx
            .query_row(
                "SELECT observed_at FROM rate_limits_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("读取额度快照失败: {e}"))?;
        if stored.map_or(true, |s| observed_at >= s) {
            tx.execute(
                "INSERT INTO rate_limits_state
                    (id, observed_at, plan_type, primary_pct, primary_reset_at,
                     secondary_pct, secondary_reset_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    observed_at = ?1, plan_type = ?2, primary_pct = ?3,
                    primary_reset_at = ?4, secondary_pct = ?5, secondary_reset_at = ?6",
                rusqlite::params![
                    observed_at,
                    snap.plan_type,
                    snap.primary_pct,
                    snap.primary_reset_at,
                    snap.secondary_pct,
                    snap.secondary_reset_at,
                ],
            )
            .map_err(|e| format!("写入额度快照失败: {e}"))?;
        }
    }

    Ok(())
}

/// 续读回看：从游标前最多 LOOKBACK_BYTES 字节读到游标，只喂事件重建
/// RequestTimer 上下文（last_boundary_at / 待定失败事件等；起点在行中间
/// 时丢掉开头那条不完整的行，跨越游标的行防御性整行丢弃）。回看区的
/// 结算与 0 token 行一律丢弃——不写库、不推进序号（仿 claude.rs::feed_lookback）。
fn feed_lookback(
    reader: &mut std::io::BufReader<&mut std::fs::File>,
    start_offset: u64,
    timer: &mut RequestTimer,
) -> Result<(), String> {
    let begin = start_offset.saturating_sub(LOOKBACK_BYTES);
    // 对齐判定：begin == 0 必在行边界；否则看前一字节是否换行（是 → 第一行
    // 完整可直接喂，否 → 第一行是被截断的半行，整行丢弃）
    let mut aligned = true;
    if begin > 0 {
        reader
            .seek(SeekFrom::Start(begin - 1))
            .map_err(|e| format!("定位回看偏移失败: {e}"))?;
        let mut probe = [0u8; 1];
        let n = std::io::Read::read(reader, &mut probe)
            .map_err(|e| format!("读取回看对齐字节失败: {e}"))?;
        aligned = n == 1 && probe[0] == b'\n';
    }
    reader
        .seek(SeekFrom::Start(begin))
        .map_err(|e| format!("定位回看偏移失败: {e}"))?;
    let limit = start_offset - begin;
    let mut consumed: u64 = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    loop {
        if consumed >= limit {
            break;
        }
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| format!("读取回看区失败: {e}"))?;
        if n == 0 {
            break; // EOF（游标之后没有更多内容，异常情况，防御）
        }
        consumed += n as u64;
        if !aligned {
            // 开头不完整的行：等下一个换行符之后才算对齐
            if buf.last() == Some(&b'\n') {
                aligned = true;
            }
            continue;
        }
        if consumed > limit {
            break; // 游标非行边界（防御）：跨过去的行不喂，避免与新增区重复
        }
        // 产物丢弃：回看区只重建上下文，写库只认新增字节里的结算
        if let Ok(line) = serde_json::from_slice::<RolloutLine>(&buf) {
            let _ = timer.feed(&line);
        }
    }
    Ok(())
}

// ===== 查询函数（与 db.rs 同名同构，查 codex.sqlite；查询前先增量导入）=====

/// 查询 [from_ms, to_ms) 内的统计（口径与 db::query_stats 完全一致）。
/// 新版导入已按事件序列取证耗时 / 首字（老库无列时按 NULL 聚合），
/// 速度原料由 fill_duration_metrics 统一补齐。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let speed = db::speed_agg_columns(
        db::has_column(&conn, "model_usage", "duration_ms"),
        db::has_column(&conn, "model_usage", "time_to_first_token_ms"),
    );

    let mut overall: db::OverallStat = conn
        .query_row(
            &format!(
                "SELECT
                    COUNT(*),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_input_tokens),0),
                    COALESCE(SUM(cache_creation_input_tokens),0),
                    COALESCE(SUM(reasoning_tokens),0),
                    COALESCE(SUM(computed_total_tokens),0)
                    {speed}
                 FROM model_usage
                 WHERE started_at >= ?1 AND started_at < ?2"
            ),
            rusqlite::params![from_ms, to_ms],
            |row| {
                Ok(db::OverallStat {
                    requests: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_read_tokens: row.get(3)?,
                    cache_write_tokens: row.get(4)?,
                    reasoning_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                    speed: db::SpeedMetrics {
                        avg_tps: row.get(7)?,
                        max_tps: row.get(8)?,
                        avg_ttft_ms: row.get(9)?,
                        ..Default::default()
                    },
                })
            },
        )
        .map_err(|e| format!("查询 Codex 整体统计失败: {e}"))?;

    let mut stmt = conn
        .prepare(&format!(
            "SELECT
                model_id,
                provider_id,
                COUNT(*),
                COALESCE(SUM(input_tokens),0),
                COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_input_tokens),0),
                COALESCE(SUM(cache_creation_input_tokens),0),
                COALESCE(SUM(reasoning_tokens),0),
                COALESCE(SUM(computed_total_tokens),0) AS total_tokens
                {speed}
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2
             GROUP BY provider_id, model_id
             ORDER BY total_tokens DESC",
        ))
        .map_err(|e| format!("准备 Codex 模型分组查询失败: {e}"))?;

    let mut by_model = stmt
        .query_map(rusqlite::params![from_ms, to_ms], |row| {
            Ok(db::ModelStat {
                model_id: row.get(0)?,
                provider_id: row.get(1)?,
                requests: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                cache_write_tokens: row.get(6)?,
                reasoning_tokens: row.get(7)?,
                total_tokens: row.get(8)?,
                speed: db::SpeedMetrics {
                    avg_tps: row.get(9)?,
                    max_tps: row.get(10)?,
                    avg_ttft_ms: row.get(11)?,
                    ..Default::default()
                },
            })
        })
        .map_err(|e| format!("读取 Codex 模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 模型分组失败: {e}"))?;

    // 事件时刻齐全的行改按流式窗口口径（写法与 claude::query_stats 一致）；
    // 无计时列的老库内部直接按行数守恒返回，等价于旧占位结果
    db::fill_duration_metrics(&conn, "codex", from_ms, to_ms, &mut overall, &mut by_model)?;

    let (earliest_ms, latest_ms): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(started_at), MAX(started_at) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("查询 Codex 时间范围失败: {e}"))?;

    Ok(db::Stats {
        from_ms,
        to_ms,
        overall,
        by_model,
        earliest_ms,
        latest_ms,
        current_model: db::query_current_model(&conn),
    })
}

/// 会话悬浮窗（session_hud）用：查询活跃窗口内的会话行。
///
/// 口径与 ZCode 组一致：
/// - 活跃判定 = `started_at >= window_start`（窗口内有 model_usage 落库）；
/// - 行累计为会话**全生命周期**合计（外层对窗口内会话集合不再加时间
///   过滤）：↑ = Σ max(0, input − cache_read)（SQLite 双参 MAX 逐行
///   clamp）、↓ = Σ output、⟲ = Σ cache_read、× = 行数；
/// - 排序按最近活动倒序（MAX(started_at) DESC），只取前 `limit` 条；
/// - `total_active` 为窗口内 distinct 会话总数（含未展示的，供折叠
///   提示"还有 N 个"）。
///
/// 项目目录取 session_meta.cwd（导入器按会话首行 session_meta 事件
/// upsert，session_id 主键等值查；无记录为 None，目录末段由
/// session_hud 侧解析）。查询前先增量导入（5 秒节流，未安装/失败
/// 静默返回 Err，由调用方跳过该组）。
pub fn query_window_sessions(
    window_start_ms: i64,
    limit: usize,
) -> Result<crate::session_hud::AgentWindowSessions, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    query_window_sessions_at(&conn, window_start_ms, limit)
}

/// query_window_sessions 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_window_sessions_at(
    conn: &Connection,
    window_start_ms: i64,
    limit: usize,
) -> Result<crate::session_hud::AgentWindowSessions, String> {
    // 内层子查询按 started_at 索引倒序取窗口内最近 N 笔的会话（成本
    // 硬上限，绝不全表扫），外层对命中的会话全生命周期聚合后取最新
    // `limit` 条
    let sql = "SELECT model_usage.session_id,
            MAX(model_usage.started_at),
            COALESCE(SUM(MAX(0, model_usage.input_tokens - model_usage.cache_read_input_tokens)), 0),
            COALESCE(SUM(model_usage.output_tokens), 0),
            COALESCE(SUM(model_usage.cache_read_input_tokens), 0),
            COUNT(*),
            GROUP_CONCAT(DISTINCT model_usage.model_id),
            (SELECT sm.cwd FROM session_meta sm
              WHERE sm.session_id = model_usage.session_id)
         FROM model_usage
         WHERE model_usage.session_id IN (
            SELECT session_id FROM model_usage
             WHERE started_at >= ?1 ORDER BY started_at DESC LIMIT ?2
         )
         GROUP BY model_usage.session_id
         ORDER BY 2 DESC
         LIMIT ?3";
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("准备 Codex 会话窗口查询失败: {e}"))?;
    let sessions = stmt
        .query_map(
            rusqlite::params![window_start_ms, WINDOW_SCAN_ROWS, limit as i64],
            |row| {
                Ok(crate::session_hud::AgentSessionBrief {
                    session_id: row.get(0)?,
                    last_at: row.get(1)?,
                    in_tokens: row.get(2)?,
                    out_tokens: row.get(3)?,
                    cache_read: row.get(4)?,
                    req_count: row.get(5)?,
                    models: row.get(6)?,
                    project: row.get(7)?,
                    // 源数据无请求耗时（无 duration_ms 列），无速度原料
                    speed_out_tokens: 0,
                    speed_ms: 0.0,
                    // codex 无生成中/TTFT/生成口径数据源（transcript 信号
                    // 仅 kimi 有），增强字段恒默认值
                    generating: false,
                    ttft_ms: None,
                    gen_out_tokens: 0,
                    gen_ms: 0.0,
                })
            },
        )
        .map_err(|e| format!("读取 Codex 会话窗口失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 会话窗口失败: {e}"))?;
    let total_active: usize = conn
        .query_row(
            "SELECT COUNT(DISTINCT session_id) FROM model_usage WHERE started_at >= ?1",
            [window_start_ms],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as usize)
        .map_err(|e| format!("统计 Codex 活跃会话数失败: {e}"))?;
    Ok(crate::session_hud::AgentWindowSessions {
        total_active,
        sessions,
    })
}

/// 查询 [from_ms, to_ms) 内的分桶统计（与 db::query_trend 同思路，复用其对齐/标签函数）。
/// bucket 为 "hour" 或 "day"。
pub fn query_trend(
    from_ms: i64,
    to_ms: i64,
    bucket: &str,
) -> Result<Vec<db::TrendBucketRaw>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let width = if bucket == "hour" { 3_600_000 } else { 86_400_000 };

    let mut start = db::align_bucket_start(from_ms, bucket);
    let sql = "SELECT
                model_id,
                provider_id,
                COUNT(*),
                COALESCE(SUM(input_tokens),0),
                COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_input_tokens),0),
                COALESCE(SUM(computed_total_tokens),0)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2
             GROUP BY provider_id, model_id";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("准备 Codex 趋势查询失败: {e}"))?;

    let mut out: Vec<db::TrendBucketRaw> = Vec::new();
    while start < to_ms {
        let end = start + width;
        let by_model: Vec<db::BucketModelStat> = stmt
            .query_map(rusqlite::params![start, end], |row| {
                Ok(db::BucketModelStat {
                    model_id: row.get(0)?,
                    provider_id: row.get(1)?,
                    requests: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    cache_read_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                })
            })
            .map_err(|e| format!("读取 Codex 趋势统计失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 Codex 趋势统计失败: {e}"))?;

        let total_tokens = by_model.iter().map(|m| m.total_tokens).sum();
        let requests = by_model.iter().map(|m| m.requests).sum();

        out.push(db::TrendBucketRaw {
            label: db::bucket_label(start, bucket),
            by_model,
            total_tokens,
            requests,
        });

        start = end;
    }

    Ok(out)
}

/// 查询 id > since 的明细记录（同步上传用）。source 固定 "codex"，local_rowid = id。
/// proto 5：附带会话 id 与项目维度（project_key / 原始 cwd 来自 session_meta，
/// 未回填的会话两字段为 None，服务端落 NULL）。
/// proto 8：耗时/首字按列存在性带出（与 request_logs 的行级查询同款动态降级口径）。
/// proto 9：附带流式速度证据 speed_evidence（事件时刻齐全时）。
/// proto 10：附带状态扩展列（status / error_code / retry_count /
/// timing_estimated，按列存在性带出）。实现见 query_rows_since_id。
pub fn query_since(since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    // 增量上传按 id 游标分页：WHERE 与 ORDER BY 都必须锚定 mu.id
    let rows = query_rows_since_id(
        &conn,
        "mu.id > ?1",
        "mu.id ASC",
        &[since],
        2,
        limit,
        "增量",
    )?;
    Ok(rows.into_iter().map(|row| row.usage).collect())
}

/// query_since / query_model_revised_since_at 共用的行查询内核（WHERE /
/// ORDER BY / 占位符由调用方给：limit_idx = WHERE 参数个数 + 1）。proto 10
/// 状态扩展列追加在 SELECT 末尾（见 row 内注释的下标表），事件时刻两列
/// （first_token_at / completed_at）仅供流式速度证据判定、不进 UsageRow——
/// 有事件证据的行同时携带 speed_evidence（proto 9 通道，见
/// token_speed::import_evidence），禁止用这两列反推相对量。
fn query_rows_since_id(
    conn: &Connection,
    where_clause: &str,
    order_by: &str,
    params: &[i64],
    limit_idx: usize,
    limit: usize,
    kind: &str,
) -> Result<Vec<ModelRevisionRow>, String> {
    // proto 8：与 query_since 同款耗时/首字列（无列 → 恒 None；查询/同步层
    // 不做时间戳差相减，只转发导入时按事件时刻算好并带 timing_estimated
    // 标记落库的列）
    let (duration, ttft) = db::speed_columns(conn);
    // proto 10：状态扩展列同款按列存在性带出（session_meta 无同名列，
    // JOIN 下裸列名解析无歧义）
    let (status, error_code, retry_count, timing_estimated) = db::timing_columns(conn);
    // 事件时刻（speed_evidence 判定用；无列恒 NULL → evidence 走 None 回退）
    let (first_col, completed_col) = event_columns(conn);
    let sql = format!(
        "SELECT mu.id, mu.started_at, mu.model_id, mu.provider_id,
                mu.input_tokens, mu.output_tokens, mu.cache_read_input_tokens,
                mu.cache_creation_input_tokens, mu.reasoning_tokens, mu.computed_total_tokens,
                mu.model_revision_at, mu.session_id, sm.project_key, sm.cwd,
                {duration}, {ttft}, {status}, {error_code}, {retry_count},
                {timing_estimated}, {first_col}, {completed_col}
         FROM model_usage mu
         LEFT JOIN session_meta sm ON sm.session_id = mu.session_id
         WHERE {where_clause}
         ORDER BY {order_by}
         LIMIT ?{limit_idx}"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 Codex {kind}查询失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(
                params.iter().copied().chain(std::iter::once(limit as i64)),
            ),
            |row| {
                let output_tokens: i64 = row.get(5)?;
                let duration_ms: Option<f64> = row.get(14)?;
                let status_raw: Option<String> = row.get(16)?;
                let timing = crate::token_speed::ImportTiming {
                    output_tokens,
                    row_at: row.get(1)?,
                    first_token_at: row.get(20)?,
                    completed_at: row.get(21)?,
                    duration_ms,
                    fallback_quality: crate::token_speed::duration_quality("codex"),
                    observed_at_ms: 0,
                    completed: crate::token_speed::is_completed_status(status_raw.as_deref()),
                };
                Ok(ModelRevisionRow {
                    usage: db::UsageRow {
                    speed_evidence: crate::token_speed::import_evidence(&timing),
                    local_rowid: row.get(0)?,
                    started_at: row.get(1)?,
                    model_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    provider_id: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    input_tokens: row.get(4)?,
                    output_tokens,
                    cache_read_input_tokens: row.get(6)?,
                    cache_creation_input_tokens: row.get(7)?,
                    reasoning_tokens: row.get(8)?,
                    computed_total_tokens: row.get(9)?,
                    source: "codex".into(),
                    // 列序以 SELECT 为准：model_revision_at=10 挤占了 query_since
                    // 里的 session_id 位，此后逐列 +1（session_id=11、project_key=12、
                    // cwd=13、duration=14、ttft=15；speed_columns / timing_columns
                    // / event_columns 无列时以 NULL / 0 字面量占位，列数不变）。
                    // proto 10 状态扩展列：status=16、error_code=17、
                    // retry_count=18、timing_estimated=19；事件时刻仅判定用：
                    // first=20、completed=21（revision_at 读下标 10，见下方）
                    session_id: row.get(11)?,
                    project_key: row.get(12)?,
                    project_display: row.get(13)?,
                    duration_ms,
                    time_to_first_token_ms: row.get(15)?,
                    status: row.get(16)?,
                    error_code: row.get::<_, Option<String>>(17)?.filter(|c| !c.is_empty()),
                    retry_count: row.get(18)?,
                    timing_estimated: row.get::<_, Option<i64>>(19)?.map(|v| v != 0),
                },
                revision_at: row.get(10)?,
                })
            },
        )
        .map_err(|e| format!("读取 Codex {kind}记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex {kind}记录失败: {e}"))?;
    Ok(rows)
}

/// 导入库的事件时刻列（first_token_at / completed_at）：无列时给 NULL 占位
/// （列数不变）。这两列仅供 speed_evidence 判定，禁止用它们反推相对量。
fn event_columns(conn: &Connection) -> (&'static str, &'static str) {
    (
        if db::has_column(conn, "model_usage", "first_token_at") { "first_token_at" } else { "NULL" },
        if db::has_column(conn, "model_usage", "completed_at") { "completed_at" } else { "NULL" },
    )
}

/// proto 10 定向重传的行筛选条件（见 query_timing_backlog_at）：带任一计时 /
/// 状态证据即入选。列存在性动态降级——缺列的维度以字面量 0 占位（恒假，
/// 该维度不参与判定，查询不报错）。status / error_code 空串按无值处理；
/// duration_ms 要求非空且大于 0（SQL 的 NULL > 0 为假，直接比较即可）。
/// 当前导入器写耗时时必写 completed_at，duration_ms 条件是为了不依赖该
/// 不变量的兜底。
fn timing_backlog_condition(conn: &Connection) -> String {
    let present = |name: &str| -> String {
        if db::has_column(conn, "model_usage", name) {
            format!("{name} IS NOT NULL")
        } else {
            "0".to_string()
        }
    };
    let nonempty = |name: &str| -> String {
        if db::has_column(conn, "model_usage", name) {
            format!("NULLIF({name},'') IS NOT NULL")
        } else {
            "0".to_string()
        }
    };
    let retry = if db::has_column(conn, "model_usage", "retry_count") {
        "COALESCE(retry_count,0) > 0".to_string()
    } else {
        "0".to_string()
    };
    let duration = if db::has_column(conn, "model_usage", "duration_ms") {
        "duration_ms > 0".to_string()
    } else {
        "0".to_string()
    };
    let joined = [
        present("request_started_at"),
        present("first_token_at"),
        present("completed_at"),
        present("time_to_first_token_ms"),
        present("timing_estimated"),
        duration,
        retry,
        nonempty("status"),
        nonempty("error_code"),
    ]
    .join(" OR ");
    format!("({joined})")
}

/// proto 10 一次性定向重传查询（同步上传用，入口见 query_since 的同款约定）。
/// 只选出**带计时 / 状态证据**的行（request_started_at / first_token_at /
/// completed_at / time_to_first_token_ms / timing_estimated / status /
/// error_code 任一非空，或 retry_count > 0），按 id 升序分页。普通增量通道
/// （query_since，按 id 游标）不受本查询影响——一次性通道不携带
/// last_rowid、不推进任何游标，服务端按主键幂等去重，失败重跑无副作用。
pub fn query_timing_backlog(
    after_id: i64,
    limit: usize,
) -> Result<Vec<ModelRevisionRow>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    query_timing_backlog_at(&conn, after_id, limit)
}

/// `query_timing_backlog` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_timing_backlog_at(
    conn: &Connection,
    after_id: i64,
    limit: usize,
) -> Result<Vec<ModelRevisionRow>, String> {
    let condition = timing_backlog_condition(conn);
    query_rows_since_id(
        conn,
        &format!("mu.id > ?1 AND {condition}"),
        "mu.id ASC",
        &[after_id],
        2,
        limit,
        "计时定向重传",
    )
}

/// 查询模型名被历史回填过的记录（同步补传用）。
///
/// 这些记录的 local_rowid 通常已经小于普通 Codex 上传游标，因此不能复用
/// query_since；按修订时间 + id 双游标分页，上传时不携带 last_rowid，避免
/// 把普通增量游标回退或推进到错误位置。
#[derive(Debug, Clone)]
pub struct ModelRevisionRow {
    pub usage: db::UsageRow,
    pub revision_at: i64,
}

pub fn query_model_revised_since(
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<ModelRevisionRow>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    query_model_revised_since_at(&conn, since_ts, after_id, limit)
}

/// `query_model_revised_since` 的纯查询内核（连接注入版，供单元测试）。
/// 下标对应关系由 revised_query_maps_revision_and_speed_columns 测试锁定。
pub(crate) fn query_model_revised_since_at(
    conn: &Connection,
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<ModelRevisionRow>, String> {
    // 修订序分页：WHERE 锚定 (model_revision_at, id) 双游标，只补模型名
    // 已知的行；排序按修订时间优先（与增量查询的 id 游标序不同）
    query_rows_since_id(
        conn,
        "(mu.model_revision_at > ?1 OR (mu.model_revision_at = ?1 AND mu.id > ?2))
            AND mu.model_id != ''",
        "mu.model_revision_at ASC, mu.id ASC",
        &[since_ts, after_id],
        3,
        limit,
        "模型修订",
    )
}

/// 导入库当前最大 rowid（供「待上传条数」显示用）。
pub fn max_rowid() -> Result<i64, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let max: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM model_usage", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("查询 Codex 最大 rowid 失败: {e}"))?;
    Ok(max)
}

/// 列出导入库中出现过的所有 (provider_id, model_id) 组合，供价格配置用。
/// provider_id 恒为 "codex"。
pub fn list_models() -> Result<Vec<db::ModelInfo>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT provider_id, model_id
             FROM model_usage
             ORDER BY model_id",
        )
        .map_err(|e| format!("准备 Codex 模型列表查询失败: {e}"))?;

    let models = stmt
        .query_map([], |row| {
            Ok(db::ModelInfo {
                provider_id: row.get(0)?,
                model_id: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取 Codex 模型列表失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 模型列表失败: {e}"))?;

    Ok(models)
}

/// 最新一次非空额度快照（导入时存 rate_limits_state 单行表；custom provider 模式为 None）。
/// 快照有天然时效：5 小时/周窗口的百分比只在对应 resets_at 之前有意义，
/// 过期窗口（重置时间已过）直接剔除——否则两个月前切到 API 中转模式的
/// 用户会看到早已失效的订阅额度，误以为数据错误。两窗口都过期返回 None。
pub fn latest_rate_limits() -> Result<Option<CodexRateLimits>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let row = conn
        .query_row(
            "SELECT plan_type, primary_pct, primary_reset_at,
                    secondary_pct, secondary_reset_at
             FROM rate_limits_state WHERE id = 1",
            [],
            |row| {
                Ok(CodexRateLimits {
                    plan_type: row.get(0)?,
                    primary_pct: row.get(1)?,
                    primary_reset_at: row.get(2)?,
                    secondary_pct: row.get(3)?,
                    secondary_reset_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("查询 Codex 额度快照失败: {e}"))?;

    let Some(mut snap) = row else {
        return Ok(None);
    };
    let now_ms = chrono::Local::now().timestamp_millis();
    // 重置时间已过的窗口不再展示（快照观察值早已失效）
    let primary_valid =
        snap.primary_reset_at.map(|t| t > now_ms).unwrap_or(false);
    let secondary_valid =
        snap.secondary_reset_at.map(|t| t > now_ms).unwrap_or(false);
    if !primary_valid {
        snap.primary_pct = None;
        snap.primary_reset_at = None;
    }
    if !secondary_valid {
        snap.secondary_pct = None;
        snap.secondary_reset_at = None;
    }
    if !primary_valid && !secondary_valid {
        return Ok(None);
    }
    Ok(Some(snap))
}

/// 按指定周期聚合 Codex Token。
/// 对比页需要真实的 [reset_at, end_at) 边界，不能用只带 HH:00 的趋势 label 反推跨日周期。
pub fn query_period_buckets(
    periods: &[(i64, i64)],
) -> Result<Vec<db::PeriodBucket>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT
                COALESCE(SUM(computed_total_tokens),0),
                COUNT(*)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2",
        )
        .map_err(|e| format!("准备 Codex 周期聚合查询失败: {e}"))?;

    let mut out = Vec::with_capacity(periods.len());
    for &(reset_at, end_at) in periods {
        let (total_tokens, requests): (i64, i64) = stmt
            .query_row(rusqlite::params![reset_at, end_at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("查询 Codex 周期聚合失败: {e}"))?;
        out.push(db::PeriodBucket {
            reset_at,
            end_at,
            total_tokens,
            requests,
        });
    }
    Ok(out)
}

// ===== 项目/会话维度查询（项目浏览器用，cwd 归一化见 projects.rs）=====

/// 按项目 × 模型聚合 [from_ms, to_ms) 的用量（SQL 侧 GROUP BY，不逐行加载）。
/// project_key 缺失（无 cwd 的会话）归入调用方传入的 unknown 哨兵键。
pub fn query_project_model_rows(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<crate::projects::ProjectModelRow>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(sm.project_key, ?1),
                    mu.model_id,
                    COUNT(*),
                    COALESCE(SUM(mu.input_tokens),0),
                    COALESCE(SUM(mu.output_tokens),0),
                    COALESCE(SUM(mu.cache_read_input_tokens),0),
                    COALESCE(SUM(mu.computed_total_tokens),0)
             FROM model_usage mu
             LEFT JOIN session_meta sm ON sm.session_id = mu.session_id
             WHERE mu.started_at >= ?2 AND mu.started_at < ?3
             GROUP BY 1, 2",
        )
        .map_err(|e| format!("准备 Codex 项目聚合查询失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![crate::projects::UNKNOWN_PROJECT, from_ms, to_ms],
            |row| {
                Ok(crate::projects::ProjectModelRow {
                    project_key: row.get(0)?,
                    model_id: row.get(1)?,
                    requests: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    cache_read_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                })
            },
        )
        .map_err(|e| format!("读取 Codex 项目聚合失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 项目聚合失败: {e}"))?;
    Ok(rows)
}

/// 按项目统计 [from_ms, to_ms) 内的会话数（COUNT(DISTINCT session_id)）。
pub fn query_project_session_counts(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<(String, i64)>, String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(sm.project_key, ?1), COUNT(DISTINCT mu.session_id)
             FROM model_usage mu
             LEFT JOIN session_meta sm ON sm.session_id = mu.session_id
             WHERE mu.started_at >= ?2 AND mu.started_at < ?3
             GROUP BY 1",
        )
        .map_err(|e| format!("准备 Codex 项目会话统计失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![crate::projects::UNKNOWN_PROJECT, from_ms, to_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("读取 Codex 项目会话统计失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 项目会话统计失败: {e}"))?;
    Ok(rows)
}

/// 项目键 → 原始形态 cwd（保留大小写，取字典序最小者做代表）。
pub fn query_project_display_paths() -> Result<Vec<(String, String)>, String> {
    let conn = open_codex_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT project_key, MIN(cwd)
             FROM session_meta
             WHERE project_key IS NOT NULL AND cwd IS NOT NULL AND cwd != ''
             GROUP BY project_key",
        )
        .map_err(|e| format!("准备 Codex 项目路径查询失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| format!("读取 Codex 项目路径失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 项目路径失败: {e}"))?;
    Ok(rows)
}

/// 分页查询指定项目的会话明细（按会话最后活跃时间降序）。
/// 返回 (匹配会话总数, 按会话 × 模型聚合的行)；offset/limit 作用在会话粒度。
/// project_key 传 UNKNOWN_PROJECT 时匹配 project_key 为 NULL 的会话。
pub fn query_project_sessions(
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Result<(u32, Vec<crate::projects::ProjectSessionModelRow>), String> {
    import_incremental()?;
    let conn = open_codex_db()?;
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
                SELECT DISTINCT mu.session_id
                FROM model_usage mu
                LEFT JOIN session_meta sm ON sm.session_id = mu.session_id
                WHERE mu.started_at >= ?1 AND mu.started_at < ?2
                  AND COALESCE(sm.project_key, ?3) = ?4
            )",
            rusqlite::params![from_ms, to_ms, crate::projects::UNKNOWN_PROJECT, project_key],
            |row| row.get(0),
        )
        .map_err(|e| format!("查询 Codex 项目会话总数失败: {e}"))?;

    // 速度 / TTFT 聚合片段按列探测（新库有计时列，老库降级占位 NULL/0 →
    // 会话级两指标为 None，前端隐藏）；仍走统一片段保持四源同构
    let speed = db::session_speed_agg_columns(
        db::has_column(&conn, "model_usage", "duration_ms"),
        db::has_column(&conn, "model_usage", "time_to_first_token_ms"),
    );
    let mut stmt = conn
        .prepare(&format!(
            "SELECT mu.session_id, mu.model_id,
                    MIN(mu.started_at), MAX(mu.started_at), COUNT(*),
                    COALESCE(SUM(mu.input_tokens),0),
                    COALESCE(SUM(mu.output_tokens),0),
                    COALESCE(SUM(mu.cache_read_input_tokens),0),
                    COALESCE(SUM(mu.cache_creation_input_tokens),0)
                    {speed}
             FROM model_usage mu
             LEFT JOIN session_meta sm ON sm.session_id = mu.session_id
             WHERE mu.started_at >= ?1 AND mu.started_at < ?2
               AND COALESCE(sm.project_key, ?3) = ?4
               AND mu.session_id IN (
                   SELECT session_id FROM (
                       SELECT mu2.session_id AS session_id
                       FROM model_usage mu2
                       LEFT JOIN session_meta sm2 ON sm2.session_id = mu2.session_id
                       WHERE mu2.started_at >= ?1 AND mu2.started_at < ?2
                         AND COALESCE(sm2.project_key, ?3) = ?4
                       GROUP BY mu2.session_id
                       ORDER BY MAX(mu2.started_at) DESC
                       LIMIT ?5 OFFSET ?6
                   )
               )
             GROUP BY mu.session_id, mu.model_id"
        ))
        .map_err(|e| format!("准备 Codex 项目会话查询失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                from_ms,
                to_ms,
                crate::projects::UNKNOWN_PROJECT,
                project_key,
                limit as i64,
                offset as i64
            ],
            |row| {
                Ok(crate::projects::ProjectSessionModelRow {
                    session_id: row.get(0)?,
                    model_id: row.get(1)?,
                    first_at: row.get(2)?,
                    last_at: row.get(3)?,
                    requests: row.get(4)?,
                    input_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    cache_read_tokens: row.get(7)?,
                    cache_write_tokens: row.get(8)?,
                    tps_sum: row.get(9)?,
                    tps_count: row.get(10)?,
                    ttft_sum: row.get(11)?,
                    ttft_count: row.get(12)?,
                    gen_out: None,
                    gen_ms: None,
                    gen_count: 0,
                })
            },
        )
        .map_err(|e| format!("读取 Codex 项目会话失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Codex 项目会话失败: {e}"))?;
    Ok((total.max(0) as u32, rows))
}

// ===== 实时额度（ChatGPT backend-api，参照 CodexBar 的实现）=====

/// Codex 登录凭证（只读 ~/.codex/auth.json，绝不修改/刷新——
/// refresh_token 是一次性轮换的，外部写回极易把 Codex CLI 的登录搞坏，
/// 刷新由 Codex CLI 自己按 8 天周期维护）。
struct CodexAuth {
    access_token: String,
    account_id: Option<String>,
}

/// 读取 auth.json 的 access_token / account_id。
/// 新版结构在 tokens 嵌套对象里，旧版扁平在顶层，两种都兼容。
fn load_codex_auth() -> Result<CodexAuth, String> {
    let root = std::env::var("ZBAR_CODEX_HOME")
        .or_else(|_| std::env::var("CODEX_HOME"))
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        });
    let path = root.join("auth.json");
    if !path.exists() {
        return Err("未找到 Codex 登录凭证（auth.json），请先 codex login".into());
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 auth.json 失败: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| format!("解析 auth.json 失败: {e}"))?;
    let token = v
        .pointer("/tokens/access_token")
        .and_then(|t| t.as_str())
        .or_else(|| v.get("access_token").and_then(|t| t.as_str()))
        .filter(|t| !t.is_empty())
        .ok_or("auth.json 中无 access_token")?;
    let account_id = v
        .pointer("/tokens/account_id")
        .and_then(|t| t.as_str())
        .or_else(|| v.get("account_id").and_then(|t| t.as_str()))
        .filter(|t| !t.is_empty())
        .map(|s| s.to_string());
    Ok(CodexAuth {
        access_token: token.to_string(),
        account_id,
    })
}

/// wham/usage 响应结构（字段名与 jsonl 里的 rate_limits 不同：
/// 这里是 primary_window/secondary_window + reset_at 秒）。
/// 注意窗口组合不固定（Plus 账号实测 primary 就是周窗口、secondary 为 null），
/// 展示前需按 limit_window_seconds 归类，不能假定 primary=5h。
#[derive(Debug, Deserialize)]
struct WhamUsageResponse {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<WhamRateLimit>,
}

#[derive(Debug, Deserialize)]
struct WhamRateLimit {
    #[serde(default)]
    primary_window: Option<WhamWindow>,
    #[serde(default)]
    secondary_window: Option<WhamWindow>,
}

#[derive(Debug, Deserialize)]
struct WhamWindow {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    reset_at: Option<i64>,
    /// 窗口时长（秒）：18000=5小时，604800=周
    #[serde(default)]
    limit_window_seconds: Option<i64>,
}

/// 解析代理地址，优先级：HTTPS_PROXY 环境变量 > 系统代理（Windows 注册表 /
/// macOS scutil）> 直连。chatgpt.com 在部分网络需代理才可达，而桌面应用
/// 常见情况是用户开了系统代理但没有设置环境变量。
/// claude 模块的实时额度请求（api.anthropic.com）复用同一探测。
pub(crate) fn resolve_proxy() -> Option<String> {
    for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    system_proxy()
}

/// 读取系统代理设置，返回代理 URL（如 "http://127.0.0.1:7890"）。
/// - Windows：注册表 HKCU\...\Internet Settings 的 ProxyEnable/ProxyServer
/// - macOS：scutil --proxy 输出（HTTPSEnable/HTTPSProxy 等，Clash/Surge 等
///   设置系统代理后即可被读到）
/// - 其他平台：无（仅环境变量）
///
/// `pub(crate)`：灵动岛天气模块的出站请求也复用它，让 Clash 等只设系统代理、
/// 不设环境变量的场景下 open-meteo / geocoding 同样能走代理。
pub(crate) fn system_proxy() -> Option<String> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let settings = hkcu
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
            .ok()?;
        let enabled: u32 = settings.get_value("ProxyEnable").ok()?;
        if enabled != 1 {
            return None;
        }
        let server: String = settings.get_value("ProxyServer").ok()?;
        let server = server.trim().to_string();
        if server.is_empty() {
            return None;
        }
        // ProxyServer 两种形态：统一代理 "host:port"；分协议 "http=a:b;https=c:d"
        let picked = server
            .split(';')
            .find(|p| p.trim_start().starts_with("https="))
            .map(|p| p.split('=').nth(1).unwrap_or("").trim().to_string())
            .or_else(|| {
                if server.contains('=') {
                    // 分协议但无 https 项：退回 http 项
                    server
                        .split(';')
                        .find(|p| p.trim_start().starts_with("http="))
                        .and_then(|p| p.split('=').nth(1))
                        .map(|s| s.trim().to_string())
                } else {
                    Some(server.clone())
                }
            })?;
        if picked.is_empty() {
            return None;
        }
        Some(normalize_proxy_url(&picked))
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("scutil")
            .arg("--proxy")
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        parse_scutil_proxy(&text)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// 裸 "host:port" 补全为 "http://host:port"（已带 scheme 的原样返回）。
/// ureq 的 Proxy 需要完整 URL；socks5:// 由调用方（scutil 解析）自行拼好。
fn normalize_proxy_url(addr: &str) -> String {
    if addr.contains("://") {
        addr.to_string()
    } else {
        format!("http://{addr}")
    }
}

/// 解析 `scutil --proxy` 输出，取第一个启用的代理（HTTPS > HTTP > SOCKS5）。
/// 输出形如：
/// ```text
/// <dictionary> {
///   HTTPEnable : 1
///   HTTPProxy : 127.0.0.1
///   HTTPPort : 7890
///   HTTPSEnable : 1
///   HTTPSProxy : 127.0.0.1
///   HTTPSPort : 7890
///   SOCKSEnable : 0
/// }
/// ```
/// 仅 macOS 构建与测试编译纳入（Windows/Linux 普通构建下无人调用，
/// 不加条件会触发 dead_code 警告）
#[cfg(any(target_os = "macos", test))]
fn parse_scutil_proxy(text: &str) -> Option<String> {
    // 逐行查找 "key : value"
    let find = |key: &str| -> Option<String> {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim() == key {
                Some(v.trim().to_string())
            } else {
                None
            }
        })
    };
    if find("HTTPSEnable")?.as_str() == "1" {
        let host = find("HTTPSProxy")?;
        let port = find("HTTPSPort")?;
        if !host.is_empty() && !port.is_empty() {
            return Some(format!("http://{host}:{port}"));
        }
    }
    if find("HTTPEnable")?.as_str() == "1" {
        let host = find("HTTPProxy")?;
        let port = find("HTTPPort")?;
        if !host.is_empty() && !port.is_empty() {
            return Some(format!("http://{host}:{port}"));
        }
    }
    if find("SOCKSEnable")?.as_str() == "1" {
        let host = find("SOCKSProxy")?;
        let port = find("SOCKSPort")?;
        if !host.is_empty() && !port.is_empty() {
            return Some(format!("socks5://{host}:{port}"));
        }
    }
    None
}

#[cfg(test)]
mod proxy_tests {
    use super::*;

    #[test]
    fn scutil_https_preferred() {
        let out = "<dictionary> {\n  HTTPEnable : 1\n  HTTPProxy : 127.0.0.1\n  HTTPPort : 7890\n  HTTPSEnable : 1\n  HTTPSProxy : 127.0.0.1\n  HTTPSPort : 7891\n  SOCKSEnable : 0\n}\n";
        assert_eq!(
            parse_scutil_proxy(out).as_deref(),
            Some("http://127.0.0.1:7891")
        );
    }

    #[test]
    fn scutil_http_fallback() {
        let out = "<dictionary> {\n  HTTPEnable : 1\n  HTTPProxy : 127.0.0.1\n  HTTPPort : 1087\n  HTTPSEnable : 0\n}\n";
        assert_eq!(
            parse_scutil_proxy(out).as_deref(),
            Some("http://127.0.0.1:1087")
        );
    }

    #[test]
    fn scutil_socks_fallback() {
        let out = "<dictionary> {\n  HTTPEnable : 0\n  HTTPSEnable : 0\n  SOCKSEnable : 1\n  SOCKSProxy : 127.0.0.1\n  SOCKSPort : 1080\n}\n";
        assert_eq!(
            parse_scutil_proxy(out).as_deref(),
            Some("socks5://127.0.0.1:1080")
        );
    }

    #[test]
    fn scutil_all_disabled() {
        let out = "<dictionary> {\n  HTTPEnable : 0\n  HTTPSEnable : 0\n  SOCKSEnable : 0\n}\n";
        assert_eq!(parse_scutil_proxy(out), None);
    }

    #[test]
    fn normalize_proxy_url_adds_scheme() {
        assert_eq!(normalize_proxy_url("127.0.0.1:33210"), "http://127.0.0.1:33210");
        assert_eq!(
            normalize_proxy_url("socks5://127.0.0.1:1080"),
            "socks5://127.0.0.1:1080"
        );
    }

    #[test]
    fn primary_week_window_is_classified_as_weekly() {
        let payload: RateLimitsPayload = serde_json::from_str(
            r#"{"plan_type":"plus","primary":{"used_percent":26.0,"window_minutes":10080,"resets_at":1787364605}}"#,
        )
        .expect("额度窗口解析失败");
        let limits = to_rate_limits(&payload);
        assert_eq!(limits.primary_pct, None);
        assert_eq!(limits.secondary_pct, Some(26.0));
        assert_eq!(limits.secondary_reset_at, Some(1_787_364_605_000));
    }
}

/// 实时额度结果缓存（成功 60s / 失败 15s 双 TTL，照 claude.rs / kimi.rs 模式）。
/// 成功缓存：前端多命令高频触发（30s 一轮 × 4 个预设范围都进
/// get_codex_usage），防止打爆 wham/usage 端点。
/// 失败负缓存：网络不通/未登录时同样会被高频触发，若不缓存，一轮内每个
/// 范围都要各付满 10s 超时；短暂缓存失败结果可避免重试风暴拖慢查询。
static LIVE_LIMITS_CACHE: OnceLock<
    Mutex<Option<(std::time::Instant, Result<Option<CodexRateLimits>, String>)>>,
> = OnceLock::new();

/// single-flight 在途标记：True = 有线程正在发真实 HTTP。
/// 同一轮 4 个范围并发同时 cache miss 时，只有首个线程发请求，其余在
/// Condvar 上等待共享同一份结果（否则 4 发重复 HTTP 各 10s 超时）。
/// 等待有上限（HTTP 总超时 10s + 余量），leader 意外挂死时等待者超时后
/// 退回各自请求，不会永久阻塞。
static LIVE_LIMITS_INFLIGHT: OnceLock<(Mutex<bool>, std::sync::Condvar)> = OnceLock::new();

/// single-flight 等待上限：略大于 HTTP 总超时 10s，leader 正常必然先完成。
const LIVE_LIMITS_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// 拉取实时订阅额度：GET https://chatgpt.com/backend-api/wham/usage
/// （Codex CLI 内部使用的同一端点，需 ChatGPT 订阅登录；API 中转模式
/// 的用量不走该接口，但只要本机登录过订阅账号即可查询账号额度）。
/// 失败返回 Err，调用方降级到本地快照。60 秒内复用上次结果。
#[allow(dead_code)]
pub fn fetch_live_rate_limits() -> Result<Option<CodexRateLimits>, String> {
    fetch_live_rate_limits_with_freshness().map(|(limits, _)| limits)
}

/// 拉取实时额度并标记本次结果是否来自新的 HTTP 请求。
/// 缓存命中仍可用于当前进度展示，但不应作为新的历史采样。
/// single-flight 的等待者拿到的是 leader 新请求的结果，但标记为 false：
/// 同一轮的采样由 leader 完成一次即可，避免并发范围各自重复写入历史。
pub fn fetch_live_rate_limits_with_freshness(
) -> Result<(Option<CodexRateLimits>, bool), String> {
    loop {
        // 1) 缓存命中（双 TTL）：Ok 60s / Err 15s（负缓存防重试风暴）
        let cache = LIVE_LIMITS_CACHE.get_or_init(|| Mutex::new(None));
        {
            let guard = cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((at, val)) = guard.as_ref() {
                let ttl = if val.is_ok() { 60 } else { 15 };
                if at.elapsed() < std::time::Duration::from_secs(ttl) {
                    return val.clone().map(|limits| (limits, false));
                }
            }
        }

        // 2) single-flight：有在途请求则等待（leader 完成后先写缓存再清标记，
        //    醒来回到循环头必然命中缓存）；等待超时视为 leader 挂死，退回直接请求
        let (lock, cond) = LIVE_LIMITS_INFLIGHT
            .get_or_init(|| (Mutex::new(false), std::sync::Condvar::new()));
        {
            let mut inflight = lock.lock().unwrap_or_else(|p| p.into_inner());
            if *inflight {
                let deadline = std::time::Instant::now() + LIVE_LIMITS_MAX_WAIT;
                while *inflight {
                    let remain = deadline.saturating_duration_since(std::time::Instant::now());
                    if remain.is_zero() {
                        break;
                    }
                    let (guard, _) = cond
                        .wait_timeout(inflight, remain)
                        .unwrap_or_else(|p| p.into_inner());
                    inflight = guard;
                }
                if !*inflight {
                    continue; // leader 已完成并写入缓存，回循环头取结果
                }
                // 超时（leader 意外挂死）：放弃等待，本线程直接请求
                //（退化为无 single-flight 的旧行为，多发一次无害）
                eprintln!("[agenthud-codex] 实时额度在途请求等待超时，本线程改为直接请求");
            }
            *inflight = true;
        }

        // 3) leader：执行真实 HTTP，写缓存后广播唤醒所有等待者
        let result = fetch_live_rate_limits_uncached();
        *cache.lock().unwrap_or_else(|p| p.into_inner()) =
            Some((std::time::Instant::now(), result.clone()));
        {
            let mut inflight = lock.lock().unwrap_or_else(|p| p.into_inner());
            *inflight = false;
            cond.notify_all();
        }
        return result.map(|limits| (limits, true));
    }
}

/// 真实发起 wham/usage HTTP 请求（无缓存、无并发去重，由
/// fetch_live_rate_limits_with_freshness 统一调度）。
fn fetch_live_rate_limits_uncached() -> Result<Option<CodexRateLimits>, String> {
    let auth = load_codex_auth()?;
    let mut builder = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10));
    if let Some(url) = resolve_proxy() {
        match ureq::Proxy::new(&url) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => eprintln!("[agenthud-codex] 代理地址无效（改为直连）: {url} ({e})"),
        }
    }
    let agent = builder.build();

    let mut req = agent
        .get("https://chatgpt.com/backend-api/wham/usage")
        .set("Authorization", &format!("Bearer {}", auth.access_token))
        .set("User-Agent", "codex-cli");
    if let Some(acc) = &auth.account_id {
        req = req.set("ChatGPT-Account-Id", acc);
    }
    let resp: WhamUsageResponse = req
        .call()
        .map_err(|e| format!("实时额度请求失败: {e}"))?
        .into_json()
        .map_err(|e| format!("解析实时额度失败: {e}"))?;

    // 窗口按时长归类到 5h/周两个展示槽（≤1 天视为 5h 窗口，≥2 天视为周窗口），
    // 同槽后出现的覆盖先出现的（secondary 通常更长期）
    let mut hour5: Option<(Option<f64>, Option<i64>)> = None;
    let mut weekly: Option<(Option<f64>, Option<i64>)> = None;
    for win in [resp.rate_limit.as_ref().and_then(|r| r.primary_window.as_ref()),
                resp.rate_limit.as_ref().and_then(|r| r.secondary_window.as_ref())]
        .into_iter()
        .flatten()
    {
        let slot = (win.used_percent, win.reset_at.map(|s| s * 1000));
        let secs = win.limit_window_seconds.unwrap_or(0);
        if secs >= 2 * 86_400 {
            weekly = Some(slot);
        } else {
            hour5 = Some(slot);
        }
    }
    Ok(if hour5.is_some() || weekly.is_some() {
        Some(CodexRateLimits {
            plan_type: resp.plan_type,
            primary_pct: hour5.as_ref().and_then(|s| s.0),
            primary_reset_at: hour5.as_ref().and_then(|s| s.1),
            secondary_pct: weekly.as_ref().and_then(|s| s.0),
            secondary_reset_at: weekly.as_ref().and_then(|s| s.1),
        })
    } else {
        None
    })
}

// ===== 诊断 =====

/// Codex 诊断信息（排查"无数据"问题）
#[derive(Debug, Clone, Serialize)]
pub struct CodexDebugInfo {
    /// sessions 目录路径
    pub sessions_dir: String,
    /// 目录是否存在
    pub sessions_dir_exists: bool,
    /// 目录下 rollout 文件数
    pub session_files: usize,
    /// 导入库累计记录数
    pub imported_records: i64,
    /// 最新一条用量的时间（毫秒）
    pub latest_session_ms: Option<i64>,
}

/// 诊断信息（排查数据导入问题用）。导入失败不阻断——目录不存在时
/// 恰恰要靠这些字段定位问题。诊断必须真实执行导入（绕过节流）。
pub fn debug_info() -> Result<CodexDebugInfo, String> {
    if let Err(e) = import_incremental_force() {
        eprintln!("[agenthud-codex] 诊断时增量导入失败: {e}");
    }

    let dir = sessions_dir_path();
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_session_files(&dir, 3, &mut files);
    }

    let (imported_records, latest_session_ms) = open_codex_db()
        .map(|conn| {
            conn.query_row(
                "SELECT COUNT(*), MAX(started_at) FROM model_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((0, None))
        })
        .unwrap_or((0, None));

    Ok(CodexDebugInfo {
        sessions_dir: dir.display().to_string(),
        sessions_dir_exists: dir.is_dir(),
        session_files: files.len(),
        imported_records,
        latest_session_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冒烟测试：对本机真实 sessions 目录执行增量导入并做全范围查询，
    /// 打印结果供人工核对（无 Codex 环境时跳过，不视为失败）。
    /// 重复运行第二次还会验证幂等性：记录数不因重复导入而变化
    /// （用不节流的 force 版本，绕开 5 秒节流对二次导入的跳过）。
    #[test]
    fn import_and_query_smoke() {
        if sessions_dir().is_err() {
            eprintln!("[test] 本机无 Codex sessions 目录，跳过");
            return;
        }
        import_incremental_force().expect("增量导入失败");
        let first = query_stats(0, i64::MAX).expect("全范围查询失败");
        // 幂等：再导一次，记录数不变
        import_incremental_force().expect("二次导入失败");
        let second = query_stats(0, i64::MAX).expect("二次查询失败");
        assert_eq!(
            first.overall.requests, second.overall.requests,
            "重复导入导致记录数变化"
        );
        assert!(first.overall.requests > 0, "导入了 0 条记录");
        assert_eq!(
            first.overall.total_tokens,
            first.by_model.iter().map(|m| m.total_tokens).sum::<i64>(),
            "overall 与 by_model 汇总不一致"
        );
        assert!(
            first.by_model.iter().all(|m| !m.model_id.is_empty()),
            "Codex 统计中仍存在空模型分组: {:?}",
            first
                .by_model
                .iter()
                .filter(|m| m.model_id.is_empty())
                .collect::<Vec<_>>()
        );
        // 趋势管道：以 [earliest, latest+1h) 小时桶查询，至少有一个非空桶
        if let (Some(e), Some(l)) = (first.earliest_ms, first.latest_ms) {
            let trend = query_trend(e, l + 3_600_000, "hour").expect("趋势查询失败");
            let non_empty = trend.iter().filter(|b| b.total_tokens > 0).count();
            assert!(non_empty > 0, "趋势桶全部为空");
            eprintln!("[test] trend 桶数={} 非空桶={}", trend.len(), non_empty);
        }
        eprintln!(
            "[test] requests={} overall={:?} by_model={:?} rate_limits={:?}",
            first.overall.requests,
            first.overall,
            first
                .by_model
                .iter()
                .map(|m| (m.model_id.clone(), m.requests, m.total_tokens))
                .collect::<Vec<_>>(),
            latest_rate_limits().unwrap_or(None),
        );
        // 实时额度（wham/usage）：网络环境不同结果不同，只打印不断言。
        // chatgpt.com 不可达时应返回 Err（调用方降级到本地快照）。
        match fetch_live_rate_limits() {
            Ok(v) => eprintln!("[test] 实时额度: {v:?}"),
            Err(e) => eprintln!("[test] 实时额度不可用（降级本地快照）: {e}"),
        }
    }
    /// 会话悬浮窗窗口查询（query_window_sessions_at）的 SQL 聚合口径：
    /// 活跃判定看窗口、累计看全生命周期、项目目录来自 session_meta、
    /// total_active 含未展示会话、limit 截断。
    #[test]
    fn hud_window_sessions_aggregation() {
        let tmp = std::env::temp_dir().join(format!("zbar-codex-hudwin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let db_path = tmp.join("codex-hudwin.sqlite");
        let conn = Connection::open(&db_path).expect("打开临时库失败");
        // 复用生产建表（open_codex_db 写死 ~/.agenthud 路径，测试直接手建
        // 同构最小表；列集与 query_window_sessions_at 用到的列一致）
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                event_seq INTEGER NOT NULL,
                started_at INTEGER NOT NULL,
                model_id TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                UNIQUE(session_id, event_seq));
             CREATE INDEX idx_mu_started ON model_usage(started_at);
             CREATE INDEX idx_mu_session ON model_usage(session_id);
             CREATE TABLE session_meta (
                session_id  TEXT PRIMARY KEY,
                cwd         TEXT,
                project_key TEXT,
                captured_at INTEGER);",
        )
        .expect("建测试表失败");
        let now = 2_000_000_000_000_i64;
        let win = now - 10 * 60_000;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage
                (session_id, event_seq, started_at, model_id,
                 input_tokens, output_tokens, cache_read_input_tokens)
             VALUES
                ('s-x', 1, {a}, 'gpt-5', 100, 3, 40),
                ('s-x', 2, {b}, 'gpt-5', 30, 7, 50),
                ('s-y', 1, {early}, 'gpt-5', 9000, 8000, 100),
                ('s-y', 2, {c}, 'gpt-6', 10, 1, 0),
                ('s-ghost', 1, {early2}, 'gpt-5', 5, 5, 0);
             INSERT INTO session_meta (session_id, cwd, project_key, captured_at)
             VALUES ('s-x', 'C:/work/demo', 'k', 1);",
            a = now - 120_000,
            b = now - 30_000,
            c = now - 60_000,
            early = now - 3 * 3600_000,
            early2 = now - 5 * 3600_000,
        ))
        .expect("写入样例行失败");

        let found = query_window_sessions_at(&conn, win, 3).expect("窗口查询失败");
        assert_eq!(found.total_active, 2, "s-ghost 窗口外不计活跃");
        assert_eq!(found.sessions.len(), 2);
        assert_eq!(found.sessions[0].session_id, "s-x");
        assert_eq!(found.sessions[1].session_id, "s-y");
        // s-x：↑ = 60 + max(0, 30−50)=0 → 60、↓ = 10、⟲ = 90、× = 2；
        // 项目目录来自 session_meta.cwd 原值；源无耗时列 → 速度原料恒 0
        let s_x = &found.sessions[0];
        assert_eq!(s_x.in_tokens, 60);
        assert_eq!(s_x.out_tokens, 10);
        assert_eq!(s_x.cache_read, 90);
        assert_eq!(s_x.req_count, 2);
        assert_eq!(s_x.models.as_deref(), Some("gpt-5"));
        assert_eq!(s_x.project.as_deref(), Some("C:/work/demo"));
        assert_eq!(s_x.speed_out_tokens, 0, "Codex 无 duration_ms 列");
        assert!((s_x.speed_ms - 0.0).abs() < f64::EPSILON);
        // s-y：全生命周期累计 + 多模型 distinct
        let s_y = &found.sessions[1];
        assert_eq!(s_y.in_tokens, 8910);
        assert_eq!(s_y.models.as_deref(), Some("gpt-5,gpt-6"));
        assert_eq!(s_y.project, None);

        // limit 截断：只取最新 1 条，total_active 仍为窗口内全部
        let found = query_window_sessions_at(&conn, win, 1).expect("limit 查询失败");
        assert_eq!(found.total_active, 2);
        assert_eq!(found.sessions.len(), 1);
        assert_eq!(found.sessions[0].session_id, "s-x");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 导入把 token_count 行落库并按 turn_context 归因模型；
    /// 重复导入（重解析）不改变行数与模型归属（幂等）。
    #[test]
    fn import_one_file_attributes_model() {
        let tmp = std::env::temp_dir().join(format!("zbar-codex-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let db_path = tmp.join("codex-import.sqlite");
        let mut conn = Connection::open(&db_path).expect("打开临时库失败");
        conn.execute_batch(SCHEMA_SQL).expect("初始化临时库失败");

        let file = tmp.join("rollout-2026-08-16T13-55-42-01a00923-e47a-7463-9fb8-a5c4bad67de2.jsonl");
        let lines = [
            r#"{"timestamp":"2026-08-16T05:55:58.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[]}}"#,
            // token_count 靠 turn_context 归因模型
            r#"{"timestamp":"2026-08-16T05:55:58.500Z","type":"turn_context","payload":{"model":"gpt-5.1-codex"}}"#,
            r#"{"timestamp":"2026-08-16T05:56:05.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}"#,
            r#"{"timestamp":"2026-08-16T06:36:05.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}"#,
            r#"{"timestamp":"2026-08-16T06:36:07.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}}}}"#,
        ];
        std::fs::write(&file, lines.join("\n") + "\n").expect("写入测试会话文件失败");

        // 事务由调用方提供（与生产 run_import_files 的每文件事务同构）
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("开启导入事务失败");
        import_one_file(&tx, &file, None).expect("导入失败");
        tx.commit().expect("提交导入事务失败");
        let rows = |conn: &Connection| -> Vec<(i64, String, i64)> {
            let mut stmt = conn
                .prepare("SELECT event_seq, model_id, computed_total_tokens FROM model_usage ORDER BY event_seq")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            rows(&conn),
            vec![
                (1, "gpt-5.1-codex".to_string(), 15),
                (2, "gpt-5.1-codex".to_string(), 15),
                (3, "gpt-5.1-codex".to_string(), 4),
            ]
        );

        // 重解析（known=None）幂等：行数与归属不变
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("开启重解析事务失败");
        import_one_file(&tx, &file, None).expect("重解析失败");
        tx.commit().expect("提交重解析事务失败");
        assert_eq!(rows(&conn).len(), 3, "重解析不应改变行数");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// query_model_revised_since 的 SELECT 列序 ↔ row.get 数字下标映射回归测试。
    /// 本查询比 query_since 多一列 model_revision_at（下标 10），其后所有列
    /// 顺移一位——历史上正是在这里沿用了 query_since 的下标导致错位且 get(16)
    /// 越界，codex 修订上传通道整体不可用。含耗时/首字两列的库（模拟）验证
    /// 全字段映射，缺两列的库（生产 schema 同构）验证 NULL 占位降级分支。
    #[test]
    fn revised_query_maps_revision_and_speed_columns() {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        // 与 open_codex_db 的 SCHEMA_SQL 同构，另补耗时/首字两列
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                event_seq INTEGER NOT NULL,
                started_at INTEGER NOT NULL,
                model_id TEXT NOT NULL DEFAULT '',
                provider_id TEXT NOT NULL DEFAULT 'codex',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                model_revision_at INTEGER NOT NULL DEFAULT 0,
                duration_ms REAL,
                time_to_first_token_ms REAL
             );
             CREATE TABLE session_meta (
                session_id  TEXT PRIMARY KEY,
                cwd         TEXT,
                project_key TEXT,
                captured_at INTEGER
             );
             INSERT INTO model_usage (session_id, event_seq, started_at, model_id,
                input_tokens, output_tokens, computed_total_tokens,
                model_revision_at, duration_ms, time_to_first_token_ms)
             VALUES
                ('s-1', 1, 1000, 'gpt-5.1-codex', 200, 100, 300, 900, 3200.5, 480.0),
                -- 空模型名的行不进修订通道
                ('s-2', 1, 2000, '', 1, 1, 2, 950, 10.0, 1.0);
             INSERT INTO session_meta (session_id, cwd, project_key, captured_at)
             VALUES ('s-1', '/w/demo', 'demo', 1);",
        )
        .expect("建表并写入样例行失败");

        let rows = query_model_revised_since_at(&conn, 0, 0, 10).expect("修订查询失败");
        assert_eq!(rows.len(), 1, "空模型名行不进修订通道");
        let first = &rows[0];
        assert_eq!(first.revision_at, 900, "model_revision_at 取下标 10");
        let u = &first.usage;
        // 逐字段对齐 SELECT 列序：model_revision_at(10) 之后顺移一位——
        // session_id=11、project_key=12、cwd=13、duration=14、ttft=15
        assert_eq!(u.local_rowid, 1);
        assert_eq!(u.started_at, 1000);
        assert_eq!(u.model_id, "gpt-5.1-codex");
        assert_eq!(u.input_tokens, 200);
        assert_eq!(u.output_tokens, 100);
        assert_eq!(u.computed_total_tokens, 300);
        assert_eq!(u.session_id.as_deref(), Some("s-1"), "session_id 应取下标 11");
        assert_eq!(u.project_key.as_deref(), Some("demo"), "project_key 应取下标 12");
        assert_eq!(u.project_display.as_deref(), Some("/w/demo"), "cwd 应取下标 13");
        assert!((u.duration_ms.unwrap() - 3200.5).abs() < 1e-6);
        assert!((u.time_to_first_token_ms.unwrap() - 480.0).abs() < 1e-6);

        // 缺耗时/首字两列的库（生产 schema 同构）：两字段降级 None，列数仍
        // 以 NULL 字面量占位，查询不报错、其余列不错位
        conn.execute_batch(
            "DROP TABLE model_usage;
             DROP TABLE session_meta;
             CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                event_seq INTEGER NOT NULL,
                started_at INTEGER NOT NULL,
                model_id TEXT NOT NULL DEFAULT '',
                provider_id TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                model_revision_at INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE session_meta (
                session_id  TEXT PRIMARY KEY,
                cwd         TEXT,
                project_key TEXT,
                captured_at INTEGER
             );
             INSERT INTO model_usage (session_id, event_seq, started_at, model_id,
                computed_total_tokens, model_revision_at)
             VALUES ('s-9', 1, 1000, 'gpt-5.1-codex', 30, 990);
             INSERT INTO session_meta (session_id, cwd, project_key, captured_at)
             VALUES ('s-9', '/w/x', 'x', 1);",
        )
        .expect("重建缺列表失败");
        let rows = query_model_revised_since_at(&conn, 0, 0, 10).expect("缺列库修订查询失败");
        assert_eq!(rows.len(), 1);
        let u = &rows[0].usage;
        assert_eq!(rows[0].revision_at, 990);
        assert_eq!(u.duration_ms, None, "无 duration_ms 列应恒 None");
        assert_eq!(u.time_to_first_token_ms, None, "无 ttft 列应恒 None");
        assert_eq!(u.session_id.as_deref(), Some("s-9"), "NULL 占位分支下标不应漂移");
        assert_eq!(u.project_key.as_deref(), Some("x"));
        assert_eq!(u.project_display.as_deref(), Some("/w/x"));
        assert_eq!(u.computed_total_tokens, 30);
    }

    /// proto 10 定向重传查询（query_timing_backlog_at）：只选带计时 / 状态
    /// 证据的行（任一字段非空或 retry_count > 0；空串 status 不算），按 id
    /// 升序分页；缺列库各维度以 0 占位，查询不报错、返回空。
    #[test]
    fn timing_backlog_query_selects_rows_with_evidence_only() {
        // 分支一：完整列库（新版导入库 schema 同构）
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                event_seq INTEGER NOT NULL,
                started_at INTEGER NOT NULL,
                model_id TEXT NOT NULL DEFAULT '',
                provider_id TEXT NOT NULL DEFAULT 'codex',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                model_revision_at INTEGER NOT NULL DEFAULT 0,
                duration_ms REAL,
                time_to_first_token_ms REAL,
                request_started_at INTEGER,
                first_token_at INTEGER,
                completed_at INTEGER,
                timing_estimated INTEGER,
                status TEXT,
                error_code TEXT,
                retry_count INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE session_meta (
                session_id  TEXT PRIMARY KEY,
                cwd         TEXT,
                project_key TEXT,
                captured_at INTEGER
             );
             INSERT INTO model_usage (id, session_id, event_seq, started_at, model_id,
                computed_total_tokens, status, error_code, duration_ms) VALUES
                (1, 's', 1, 1000, 'm', 10, NULL, NULL, NULL),      -- 纯用量无证据：排除
                (2, 's', 1, 2000, 'm', 10, 'error', '429', NULL),  -- 有状态：入选
                (3, 's', 1, 3000, 'm', 10, '', NULL, NULL),        -- 空串 status：排除
                (4, 's', 1, 4000, 'm', 10, NULL, NULL, NULL),      -- 见下方 UPDATE
                (5, 's', 1, 5000, 'm', 10, NULL, NULL, 1500.0);    -- 只有耗时：入选
             UPDATE model_usage SET first_token_at = 4001 WHERE id = 4;
             UPDATE model_usage SET retry_count = 2, timing_estimated = 1
                WHERE id IN (2, 4);",
        )
        .expect("建表并写入样例行失败");
        let ids = |after_id: i64| -> Vec<i64> {
            query_timing_backlog_at(&conn, after_id, 10)
                .expect("定向查询失败")
                .into_iter()
                .map(|row| row.usage.local_rowid)
                .collect()
        };
        assert_eq!(ids(0), vec![2, 4, 5], "带证据的行入选（空串 status 不算、只有耗时也算）");
        assert_eq!(ids(2), vec![4, 5], "按 id 分页：after_id 之后的行");
        assert_eq!(ids(4), vec![5]);
        assert!(ids(5).is_empty(), "取空即结束");

        // 分支二：缺全部计时/状态列的库（旧导入库 schema 同构）：条件全 0
        // 占位，查询不报错、恒返回空
        let legacy = Connection::open_in_memory().unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE model_usage (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL,
                    event_seq INTEGER NOT NULL,
                    started_at INTEGER NOT NULL,
                    model_id TEXT NOT NULL DEFAULT '',
                    provider_id TEXT NOT NULL DEFAULT 'codex',
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                    model_revision_at INTEGER NOT NULL DEFAULT 0
                 );
                 INSERT INTO model_usage (session_id, event_seq, started_at, model_id,
                    computed_total_tokens) VALUES ('s', 1, 1000, 'm', 10);
                 CREATE TABLE session_meta (
                    session_id  TEXT PRIMARY KEY,
                    cwd         TEXT,
                    project_key TEXT,
                    captured_at INTEGER
                 );",
            )
            .expect("建缺列表失败");
        let rows = query_timing_backlog_at(&legacy, 0, 10).expect("缺列库定向查询失败");
        assert!(rows.is_empty(), "无计时列的库没有定向重传行");
    }

    // ===== 计时 / 状态取证测试（合成数据，字段布局按本机 Codex CLI 实测
    // 构造，不含真实对话内容）=====

    /// ISO8601 时间 → 毫秒时间戳（测试断言用）
    fn ms(s: &str) -> i64 {
        parse_ts_ms(s).unwrap_or_else(|| panic!("测试时间戳格式错误: {s}"))
    }

    fn ev(ts: &str, payload: &str) -> String {
        format!(r#"{{"timestamp":"{ts}","type":"event_msg","payload":{payload}}}"#)
    }

    fn ri(ts: &str, payload: &str) -> String {
        format!(r#"{{"timestamp":"{ts}","type":"response_item","payload":{payload}}}"#)
    }

    fn usage_json(input: i64, cached: i64, output: i64, reasoning: i64, total: i64) -> String {
        format!(
            r#"{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":{reasoning},"total_tokens":{total}}}"#
        )
    }

    fn token_count(ts: &str, usage: &str) -> String {
        ev(
            ts,
            &format!(r#"{{"type":"token_count","info":{{"last_token_usage":{usage}}}}}"#),
        )
    }

    fn usage_record(ts: &str, usage: &str) -> String {
        format!(r#"{{"timestamp":"{ts}","type":"token_usage_record","payload":{{"usage":{usage}}}}}"#)
    }

    fn task_started(ts: &str) -> String {
        ev(ts, r#"{"type":"task_started","turn_id":"t1"}"#)
    }

    fn turn_context(ts: &str) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"turn_context","payload":{{"model":"gpt-5.1-codex"}}}}"#
        )
    }

    fn user_message(ts: &str) -> String {
        ri(ts, r#"{"type":"message","role":"user","content":[]}"#)
    }

    fn assistant_message(ts: &str) -> String {
        ri(ts, r#"{"type":"message","role":"assistant","content":[]}"#)
    }

    fn tool_call(ts: &str) -> String {
        ri(ts, r#"{"type":"custom_tool_call","name":"shell","input":"ls"}"#)
    }

    fn tool_output(ts: &str) -> String {
        ri(ts, r#"{"type":"custom_tool_call_output","output":"ok"}"#)
    }

    /// AgentMessage 的输出开始事件（started_at_ms 是取证的首个输出时刻）
    fn agent_message_done(ts: &str, started_at_ms: i64) -> String {
        ev(
            ts,
            &format!(
                r#"{{"type":"item_completed","item":{{"type":"AgentMessage","id":"it-1"}},"started_at_ms":{started_at_ms},"completed_at_ms":{}}}"#,
                started_at_ms + 1
            ),
        )
    }

    fn turn_aborted(ts: &str) -> String {
        ev(ts, r#"{"type":"turn_aborted","reason":"interrupted"}"#)
    }

    fn error_event(ts: &str, code: &str) -> String {
        ev(ts, &format!(r#"{{"type":"error","code":"{code}"}}"#))
    }

    /// model_usage 行的计时 / 状态快照（按 started_at 时间序读取）。
    #[derive(Debug, PartialEq)]
    struct RowSnapshot {
        event_seq: i64,
        started_at: i64,
        model_id: String,
        input_tokens: i64,
        output_tokens: i64,
        reasoning_tokens: i64,
        computed_total_tokens: i64,
        request_started_at: Option<i64>,
        first_token_at: Option<i64>,
        completed_at: Option<i64>,
        duration_ms: Option<f64>,
        time_to_first_token_ms: Option<f64>,
        timing_estimated: Option<i64>,
        status: Option<String>,
        error_code: Option<String>,
        retry_count: i64,
    }

    fn read_rows(conn: &Connection) -> Vec<RowSnapshot> {
        let mut stmt = conn
            .prepare(
                "SELECT event_seq, started_at, model_id, input_tokens, output_tokens,
                        reasoning_tokens, computed_total_tokens,
                        request_started_at, first_token_at, completed_at,
                        duration_ms, time_to_first_token_ms, timing_estimated,
                        status, error_code, retry_count
                 FROM model_usage ORDER BY started_at, event_seq",
            )
            .expect("准备快照查询失败");
        stmt.query_map([], |row| {
            Ok(RowSnapshot {
                event_seq: row.get(0)?,
                started_at: row.get(1)?,
                model_id: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                reasoning_tokens: row.get(5)?,
                computed_total_tokens: row.get(6)?,
                request_started_at: row.get(7)?,
                first_token_at: row.get(8)?,
                completed_at: row.get(9)?,
                duration_ms: row.get(10)?,
                time_to_first_token_ms: row.get(11)?,
                timing_estimated: row.get(12)?,
                status: row.get(13)?,
                error_code: row.get(14)?,
                retry_count: row.get(15)?,
            })
        })
        .expect("读取快照失败")
        .collect::<Result<Vec<_>, _>>()
        .expect("快照行解析失败")
    }

    /// 临时库 + 会话文件（文件名含标准 uuid，session_id 从主干末 36 字符提取）
    fn setup_codex_case(tag: &str) -> (PathBuf, Connection, PathBuf) {
        let tmp = std::env::temp_dir().join(format!(
            "zbar-codex-timing-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let conn = Connection::open(tmp.join("codex.sqlite")).expect("打开临时库失败");
        conn.execute_batch(SCHEMA_SQL).expect("初始化临时库失败");
        let file = tmp.join("rollout-2026-08-16T10-00-00-01a209f0-a6bb-7c61-a631-2a69c9e0ea48.jsonl");
        (tmp, conn, file)
    }

    fn write_session(path: &Path, lines: &[String]) {
        std::fs::write(path, lines.join("\n") + "\n").expect("写入测试会话文件失败");
    }

    fn run_import(conn: &mut Connection, file: &Path, known: Option<(u64, u64)>) {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("开启导入事务失败");
        import_one_file(&tx, file, known).expect("导入失败");
        tx.commit().expect("提交导入事务失败");
    }

    /// 今日（0.171 布局）的完整顺序：文字回复请求 + 纯工具调用子请求 +
    /// 工具结果后紧跟 token_count + 后续 AgentMessage 子请求。逐项断言
    /// 起点 / 首个输出 / 结束时刻 / 耗时 / 首字。
    #[test]
    fn import_codex_timing_full_sequence() {
        let (tmp, mut conn, file) = setup_codex_case("full");
        let u1 = usage_json(100, 10, 5, 3, 118);
        let u2 = usage_json(50, 20, 7, 1, 78);
        let u3 = usage_json(60, 30, 20, 5, 115);
        let lines = [
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.010Z"),
            user_message("2026-08-16T10:00:00.020Z"),
            ev(
                "2026-08-16T10:00:00.030Z",
                r#"{"type":"item_completed","item":{"type":"UserMessage","id":"um"}}"#,
            ),
            agent_message_done("2026-08-16T10:00:08.000Z", ms("2026-08-16T10:00:05.000Z")),
            assistant_message("2026-08-16T10:00:08.010Z"),
            usage_record("2026-08-16T10:00:08.020Z", &u1),
            token_count("2026-08-16T10:00:08.030Z", &u1),
            // 第二轮：纯工具调用子请求（无输出开始事件）→ 工具结果 → 紧跟
            // token_count → 同轮的后续 AgentMessage 子请求
            task_started("2026-08-16T10:01:00.000Z"),
            turn_context("2026-08-16T10:01:00.010Z"),
            user_message("2026-08-16T10:01:00.020Z"),
            tool_call("2026-08-16T10:01:03.000Z"),
            usage_record("2026-08-16T10:01:03.100Z", &u2),
            tool_output("2026-08-16T10:01:03.500Z"),
            token_count("2026-08-16T10:01:03.520Z", &u2),
            agent_message_done("2026-08-16T10:01:08.000Z", ms("2026-08-16T10:01:06.000Z")),
            assistant_message("2026-08-16T10:01:08.010Z"),
            usage_record("2026-08-16T10:01:08.020Z", &u3),
            token_count("2026-08-16T10:01:08.030Z", &u3),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 3, "三次 token_count 结算对应三行");
        let base = "2026-08-16T10:00:00.020Z";
        // 行 1：文字回复（起点 = 用户输入、首字 = started_at_ms、结束 = 用量记录）
        let r1 = &rows[0];
        assert_eq!(r1.event_seq, 1);
        assert_eq!(r1.started_at, ms("2026-08-16T10:00:08.030Z"));
        assert_eq!(r1.model_id, "gpt-5.1-codex");
        assert_eq!(r1.request_started_at, Some(ms(base)));
        assert_eq!(r1.first_token_at, Some(ms("2026-08-16T10:00:05.000Z")));
        assert_eq!(r1.completed_at, Some(ms("2026-08-16T10:00:08.020Z")));
        assert_eq!(r1.duration_ms, Some(8000.0));
        assert_eq!(r1.time_to_first_token_ms, Some(4980.0));
        assert_eq!(r1.timing_estimated, Some(1));
        assert_eq!(r1.status.as_deref(), Some("completed"));
        assert_eq!(r1.retry_count, 0);
        // 行 2：纯工具调用子请求（无输出开始 → 无首字；用量记录一致 →
        // 结束取用量记录时刻；起点 = 本轮用户输入）
        let r2 = &rows[1];
        assert_eq!(r2.event_seq, 2);
        assert_eq!(r2.request_started_at, Some(ms("2026-08-16T10:01:00.020Z")));
        assert_eq!(r2.first_token_at, None, "纯工具调用没有输出开始事件");
        assert_eq!(r2.completed_at, Some(ms("2026-08-16T10:01:03.100Z")));
        assert_eq!(r2.duration_ms, Some(3080.0));
        assert_eq!(r2.time_to_first_token_ms, None);
        assert_eq!(r2.timing_estimated, Some(1));
        assert_eq!(r2.input_tokens, 50);
        assert_eq!(r2.output_tokens, 7);
        assert_eq!(r2.reasoning_tokens, 1);
        assert_eq!(r2.computed_total_tokens, 78);
        // 行 3：子请求 2（起点 = 上一次 token_count 的时刻，即工具结果之后
        // 的下一个子请求开始）
        let r3 = &rows[2];
        assert_eq!(r3.event_seq, 3);
        assert_eq!(r3.request_started_at, Some(ms("2026-08-16T10:01:03.520Z")));
        assert_eq!(r3.first_token_at, Some(ms("2026-08-16T10:01:06.000Z")));
        assert_eq!(r3.completed_at, Some(ms("2026-08-16T10:01:08.020Z")));
        assert_eq!(r3.duration_ms, Some(4500.0));
        assert_eq!(r3.time_to_first_token_ms, Some(2480.0));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 生成窗口不足 200 毫秒的行：起点、首字、结束三个时刻照常落库
    ///（流式速度由读库方判定，导入层不做速度取舍），耗时按派生窗口校验。
    #[test]
    fn import_codex_timing_short_generation_window() {
        let (tmp, mut conn, file) = setup_codex_case("short");
        let u = usage_json(10, 0, 4, 0, 14);
        let lines = [
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.005Z"),
            user_message("2026-08-16T10:00:00.010Z"),
            agent_message_done("2026-08-16T10:00:03.050Z", ms("2026-08-16T10:00:03.000Z")),
            assistant_message("2026-08-16T10:00:03.060Z"),
            usage_record("2026-08-16T10:00:03.100Z", &u),
            token_count("2026-08-16T10:00:03.110Z", &u),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        // 生成窗口 = completed − first = 100ms（< 200ms）；耗时窗口
        // = completed − start = 3090ms（合法）。两个时刻必须都落库，
        // 速度由读库方按窗口合法性判定
        assert_eq!(r.first_token_at, Some(ms("2026-08-16T10:00:03.000Z")));
        assert_eq!(r.completed_at, Some(ms("2026-08-16T10:00:03.100Z")));
        assert_eq!(r.request_started_at, Some(ms("2026-08-16T10:00:00.010Z")));
        assert_eq!(r.duration_ms, Some(3090.0));
        assert_eq!(r.time_to_first_token_ms, Some(2990.0));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// token_count 延后写出（工具结果之后不到 100 毫秒、没有用量记录）→
    /// 结束时刻取最后一个模型输出的时间戳（token_count 被 tool 执行推后）。
    #[test]
    fn import_codex_timing_delayed_token_count() {
        let (tmp, mut conn, file) = setup_codex_case("delayed");
        let u = usage_json(30, 5, 9, 2, 46);
        let lines = [
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.005Z"),
            user_message("2026-08-16T10:00:00.010Z"),
            agent_message_done("2026-08-16T10:00:02.000Z", ms("2026-08-16T10:00:01.000Z")),
            assistant_message("2026-08-16T10:00:02.010Z"),
            tool_call("2026-08-16T10:00:03.000Z"),
            tool_output("2026-08-16T10:00:03.300Z"),
            token_count("2026-08-16T10:00:03.350Z", &u),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(
            r.completed_at,
            Some(ms("2026-08-16T10:00:03.000Z")),
            "工具结果后 50ms 的 token_count → 取最后一个模型输出时刻"
        );
        assert_eq!(r.request_started_at, Some(ms("2026-08-16T10:00:00.010Z")));
        assert_eq!(r.first_token_at, Some(ms("2026-08-16T10:00:01.000Z")));
        assert_eq!(r.duration_ms, Some(2990.0));
        assert_eq!(r.time_to_first_token_ms, Some(990.0));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 用量记录与 token_count 的用量对不上 → 结束时刻退回 token_count 行
    /// 时间戳（不再贴给用量记录）。
    #[test]
    fn import_codex_timing_usage_mismatch_falls_back_to_row_ts() {
        let (tmp, mut conn, file) = setup_codex_case("mismatch");
        let lines = [
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.005Z"),
            user_message("2026-08-16T10:00:00.010Z"),
            agent_message_done("2026-08-16T10:00:02.000Z", ms("2026-08-16T10:00:01.000Z")),
            assistant_message("2026-08-16T10:00:02.010Z"),
            usage_record(
                "2026-08-16T10:00:02.500Z",
                &usage_json(1, 0, 1, 0, 2),
            ),
            token_count(
                "2026-08-16T10:00:02.520Z",
                &usage_json(10, 2, 5, 1, 18),
            ),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(
            r.completed_at,
            Some(ms("2026-08-16T10:00:02.520Z")),
            "用量不一致 → 结束时刻取 token_count 行时间戳"
        );
        assert_eq!(r.duration_ms, Some(2510.0));
        assert_eq!(r.time_to_first_token_ms, Some(990.0));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 旧版布局：没有 item_completed 的 started_at_ms → 只有耗时没有首字；
    /// 上一次 token_count 之后不到 100 毫秒的成批补写输出不算新请求
    ///（补写后的第二次结算没有起点，不产生派生耗时）。
    #[test]
    fn import_codex_timing_legacy_layout_and_batch_rewrite() {
        let (tmp, mut conn, file) = setup_codex_case("legacy");
        let lines = [
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.005Z"),
            user_message("2026-08-16T10:00:00.010Z"),
            assistant_message("2026-08-16T10:00:03.000Z"),
            token_count("2026-08-16T10:00:03.100Z", &usage_json(20, 0, 6, 0, 26)),
            // 旧版本成批补写：距上次结算 50ms / 40ms 的输出项
            assistant_message("2026-08-16T10:00:03.150Z"),
            agent_message_done("2026-08-16T10:00:03.160Z", ms("2026-08-16T10:00:03.140Z")),
            token_count("2026-08-16T10:00:03.200Z", &usage_json(20, 0, 6, 0, 26)),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 2);
        // 行 1：无输出开始事件 → 只有耗时
        let r1 = &rows[0];
        assert_eq!(r1.request_started_at, Some(ms("2026-08-16T10:00:00.010Z")));
        assert_eq!(r1.first_token_at, None, "旧版无 started_at_ms");
        assert_eq!(r1.completed_at, Some(ms("2026-08-16T10:00:03.100Z")));
        assert_eq!(r1.duration_ms, Some(3090.0));
        assert_eq!(r1.time_to_first_token_ms, None);
        // 行 2：补写输出不算新请求的证据 → 无起点、无派生耗时、无首字
        let r2 = &rows[1];
        assert_eq!(
            r2.request_started_at, None,
            "补写输出被忽略 → 无模型输出 → 无起点"
        );
        assert_eq!(r2.duration_ms, None);
        assert_eq!(r2.first_token_at, None);
        assert_eq!(r2.time_to_first_token_ms, None);
        assert_eq!(r2.timing_estimated, None);
        assert_eq!(r2.completed_at, Some(ms("2026-08-16T10:00:03.200Z")));
        assert_eq!(r2.status.as_deref(), Some("completed"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 带 turn_aborted 的轮次（其后没有用量）→ 一条 status = cancelled 的
    /// 0 token 行：event_seq 取负的事件时间戳毫秒，token 全 0，计时列全空。
    #[test]
    fn import_codex_timing_turn_aborted_writes_zero_token_row() {
        let (tmp, mut conn, file) = setup_codex_case("abort");
        let lines = [
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.005Z"),
            user_message("2026-08-16T10:00:00.010Z"),
            agent_message_done("2026-08-16T10:00:02.000Z", ms("2026-08-16T10:00:01.000Z")),
            assistant_message("2026-08-16T10:00:02.010Z"),
            turn_aborted("2026-08-16T10:00:03.500Z"),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(
            r.event_seq,
            -ms("2026-08-16T10:00:03.500Z"),
            "0 token 行的序号是负的事件时间戳毫秒"
        );
        assert_eq!(r.started_at, ms("2026-08-16T10:00:03.500Z"));
        assert_eq!(r.model_id, "gpt-5.1-codex");
        assert_eq!(r.input_tokens, 0);
        assert_eq!(r.output_tokens, 0);
        assert_eq!(r.computed_total_tokens, 0);
        assert_eq!(r.status.as_deref(), Some("cancelled"));
        assert_eq!(r.error_code, None);
        assert_eq!(r.retry_count, 0);
        assert_eq!(r.request_started_at, None, "0 token 行无计时");
        assert_eq!(r.first_token_at, None);
        assert_eq!(r.completed_at, None);
        assert_eq!(r.duration_ms, None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// error 之后跟着用量 → 不单独出 0 token 行，计入正常行的重试次数并
    /// 携带错误码（重试后成功的行 status 仍是 completed）。
    #[test]
    fn import_codex_timing_error_then_usage_counts_as_retry() {
        let (tmp, mut conn, file) = setup_codex_case("retry");
        let u = usage_json(40, 8, 12, 2, 62);
        let lines = [
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.005Z"),
            user_message("2026-08-16T10:00:00.010Z"),
            error_event("2026-08-16T10:00:01.000Z", "rate_limit_exhausted"),
            agent_message_done("2026-08-16T10:00:05.000Z", ms("2026-08-16T10:00:04.000Z")),
            assistant_message("2026-08-16T10:00:05.010Z"),
            usage_record("2026-08-16T10:00:05.020Z", &u),
            token_count("2026-08-16T10:00:05.030Z", &u),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1, "错误后有用量 → 不单独出 0 token 行");
        let r = &rows[0];
        assert_eq!(r.status.as_deref(), Some("completed"));
        assert_eq!(r.retry_count, 1, "一次失败尝试计入重试次数");
        assert_eq!(r.error_code.as_deref(), Some("rate_limit_exhausted"));
        // 耗时和首字描述「产生这条回复的那一次请求」：起点取最后一次失败
        // 事件的时刻（不含失败尝试与退避等待），首字只取失败之后的输出
        assert_eq!(
            r.request_started_at,
            Some(ms("2026-08-16T10:00:01.000Z")),
            "有失败事件 → 起点改取最后一次失败时刻"
        );
        assert_eq!(
            r.completed_at,
            Some(ms("2026-08-16T10:00:05.020Z")),
            "用量一致 → 结束取用量记录时刻"
        );
        assert_eq!(
            r.duration_ms,
            Some(4020.0),
            "耗时不含失败前的尝试段（5010ms − 失败前的 990ms）"
        );
        assert_eq!(r.first_token_at, Some(ms("2026-08-16T10:00:04.000Z")));
        assert_eq!(r.time_to_first_token_ms, Some(3000.0));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 空闲时（自上次结算后没有任何请求活动）的 turn_aborted 不出行：
    /// 文件头直接中断、完整结算后中断，两种都不产生 0 token 行。
    #[test]
    fn import_codex_timing_idle_abort_writes_nothing() {
        let (tmp, mut conn, file) = setup_codex_case("idle");
        let u = usage_json(10, 0, 4, 0, 14);
        let lines = [
            // 文件头直接中断（无任何边界 / 输出）
            turn_aborted("2026-08-16T09:00:00.000Z"),
            // 完整结算的请求
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.005Z"),
            user_message("2026-08-16T10:00:00.010Z"),
            agent_message_done("2026-08-16T10:00:03.000Z", ms("2026-08-16T10:00:02.000Z")),
            assistant_message("2026-08-16T10:00:03.010Z"),
            token_count("2026-08-16T10:00:03.020Z", &u),
            // 结算后空闲中断
            turn_aborted("2026-08-16T10:05:00.000Z"),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1, "空闲中断不产生 0 token 行");
        assert_eq!(rows[0].status.as_deref(), Some("completed"));
        assert_eq!(rows[0].event_seq, 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 0 token 行的负序号不得污染续读的正序号：会话首个请求被中断（EOF
    /// 时结算出负 event_seq 行）后追加一轮用量，续读的用量行必须接着正
    /// 序号（seq = 1）落库；此后从头重放（文件被重写触发 reparse）因
    /// 键相同而幂等，不重复入库、token 总量不变。
    #[test]
    fn import_codex_zero_token_row_does_not_break_seq() {
        let (tmp, mut conn, file) = setup_codex_case("negseq");
        let u = usage_json(70, 20, 30, 6, 126);
        // 第一段：首个请求被用户中断（有边界与输出 → 出 0 token 行）
        write_session(
            &file,
            &[
                task_started("2026-08-16T10:00:00.000Z"),
                turn_context("2026-08-16T10:00:00.005Z"),
                user_message("2026-08-16T10:00:00.010Z"),
                assistant_message("2026-08-16T10:00:00.050Z"),
                turn_aborted("2026-08-16T10:00:01.000Z"),
            ],
        );
        run_import(&mut conn, &file, None);
        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_seq, -ms("2026-08-16T10:00:01.000Z"));
        assert_eq!(rows[0].status.as_deref(), Some("cancelled"));
        let (offset, size): (i64, i64) = conn
            .query_row(
                "SELECT offset, size FROM file_progress WHERE path = ?1",
                rusqlite::params![file.to_string_lossy().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读取导入进度失败");

        // 第二段：中断之后新的一轮请求正常完成
        let mut grown = std::fs::read_to_string(&file).expect("读取会话文件失败");
        grown.pop();
        grown.push('\n');
        grown.push_str(&task_started("2026-08-16T10:00:09.000Z"));
        grown.push('\n');
        grown.push_str(&turn_context("2026-08-16T10:00:09.005Z"));
        grown.push('\n');
        grown.push_str(&user_message("2026-08-16T10:00:09.010Z"));
        grown.push('\n');
        grown.push_str(&agent_message_done(
            "2026-08-16T10:00:11.000Z",
            ms("2026-08-16T10:00:10.000Z"),
        ));
        grown.push('\n');
        grown.push_str(&assistant_message("2026-08-16T10:00:11.010Z"));
        grown.push('\n');
        grown.push_str(&usage_record("2026-08-16T10:00:11.020Z", &u));
        grown.push('\n');
        grown.push_str(&token_count("2026-08-16T10:00:11.030Z", &u));
        grown.push('\n');
        std::fs::write(&file, grown).expect("追加会话内容失败");

        run_import(&mut conn, &file, Some((offset as u64, size as u64)));
        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 2, "0 token 行 + 一次正常结算");
        let usage = rows
            .iter()
            .find(|r| r.status.as_deref() == Some("completed"))
            .expect("缺少 completed 行");
        assert_eq!(
            usage.event_seq, 1,
            "续读的用量行必须接正序号（0 token 行的负序号不参与 MAX）"
        );
        assert_eq!(usage.computed_total_tokens, 126);

        // 从头重放（reparse 路径，seq 从 1 重排）：键相同 → 不增行、token
        // 总量不变（修复前负 seq 会让这里的重放整段重复入库）
        run_import(&mut conn, &file, None);
        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 2, "重放不新增行");
        let total: i64 = rows.iter().map(|r| r.computed_total_tokens).sum();
        assert_eq!(total, 126, "重放不重复计数（0 token 行贡献 0）");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 错误事件在文件读尽时不出行（之后可能跟着重试，立即结算会把重试
    /// 成功的请求拆成两行），留给下一条边界结算；turn_aborted 是用户中断、
    /// 不会有重试，照旧在文件读尽时结算为 cancelled 行。
    #[test]
    fn import_codex_error_pending_not_settled_at_eof() {
        let (tmp, mut conn, file) = setup_codex_case("erroreof");
        let u = usage_json(50, 10, 15, 3, 78);
        // 场景一：error 后文件读尽 → 不出行；追加用量后续读 → 只有一条
        // completed 行（retry_count = 1），重试不拆成两行
        write_session(
            &file,
            &[
                task_started("2026-08-16T10:00:00.000Z"),
                turn_context("2026-08-16T10:00:00.005Z"),
                user_message("2026-08-16T10:00:00.010Z"),
                error_event("2026-08-16T10:00:01.000Z", "rate_limit_exhausted"),
            ],
        );
        run_import(&mut conn, &file, None);
        assert!(
            read_rows(&conn).is_empty(),
            "error 在文件读尽时不出 0 token 行"
        );
        let (offset, size): (i64, i64) = conn
            .query_row(
                "SELECT offset, size FROM file_progress WHERE path = ?1",
                rusqlite::params![file.to_string_lossy().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读取导入进度失败");

        let mut grown = std::fs::read_to_string(&file).expect("读取会话文件失败");
        grown.pop();
        grown.push('\n');
        grown.push_str(&agent_message_done(
            "2026-08-16T10:00:05.000Z",
            ms("2026-08-16T10:00:04.000Z"),
        ));
        grown.push('\n');
        grown.push_str(&assistant_message("2026-08-16T10:00:05.010Z"));
        grown.push('\n');
        grown.push_str(&usage_record("2026-08-16T10:00:05.020Z", &u));
        grown.push('\n');
        grown.push_str(&token_count("2026-08-16T10:00:05.030Z", &u));
        grown.push('\n');
        std::fs::write(&file, grown).expect("追加会话内容失败");
        run_import(&mut conn, &file, Some((offset as u64, size as u64)));

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1, "重试成功只有一条 completed 行");
        assert_eq!(rows[0].status.as_deref(), Some("completed"));
        assert_eq!(rows[0].retry_count, 1);
        assert_eq!(rows[0].error_code.as_deref(), Some("rate_limit_exhausted"));

        // 场景二：error 后文件读尽 → 不出行；追加新边界后续读 → 一条
        // error 的 0 token 行（挂起状态经回看区重建后由边界结算）
        let (tmp2, mut conn2, file2) = setup_codex_case("erroreof2");
        write_session(
            &file2,
            &[
                task_started("2026-08-16T10:00:00.000Z"),
                turn_context("2026-08-16T10:00:00.005Z"),
                user_message("2026-08-16T10:00:00.010Z"),
                error_event("2026-08-16T10:00:01.000Z", "server_error"),
            ],
        );
        run_import(&mut conn2, &file2, None);
        assert!(read_rows(&conn2).is_empty());
        let (offset2, size2): (i64, i64) = conn2
            .query_row(
                "SELECT offset, size FROM file_progress WHERE path = ?1",
                rusqlite::params![file2.to_string_lossy().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读取导入进度失败");

        let mut grown2 = std::fs::read_to_string(&file2).expect("读取会话文件失败");
        grown2.pop();
        grown2.push('\n');
        grown2.push_str(&task_started("2026-08-16T10:00:30.000Z"));
        grown2.push('\n');
        std::fs::write(&file2, grown2).expect("追加会话内容失败");
        run_import(&mut conn2, &file2, Some((offset2 as u64, size2 as u64)));

        let rows = read_rows(&conn2);
        assert_eq!(rows.len(), 1, "新边界结算挂起的 error");
        assert_eq!(rows[0].status.as_deref(), Some("error"));
        assert_eq!(rows[0].error_code.as_deref(), Some("server_error"));
        assert_eq!(rows[0].event_seq, -ms("2026-08-16T10:00:01.000Z"));
        assert_eq!(rows[0].computed_total_tokens, 0);
        let _ = std::fs::remove_dir_all(&tmp2);

        // 场景三（对照组）：turn_aborted 照旧在文件读尽时出行
        let (tmp3, mut conn3, file3) = setup_codex_case("abort3");
        write_session(
            &file3,
            &[
                task_started("2026-08-16T10:00:00.000Z"),
                turn_context("2026-08-16T10:00:00.005Z"),
                user_message("2026-08-16T10:00:00.010Z"),
                turn_aborted("2026-08-16T10:00:01.000Z"),
            ],
        );
        run_import(&mut conn3, &file3, None);
        let rows = read_rows(&conn3);
        assert_eq!(rows.len(), 1, "turn_aborted 在文件读尽时仍出行");
        assert_eq!(rows[0].status.as_deref(), Some("cancelled"));
        let _ = std::fs::remove_dir_all(&tmp3);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 用户中断后不经边界直接出现带用量的 token_count（工具执行中被中断、
    /// 用量事后补写）：中断不作废输出证据，起点仍是输出之前的边界、结束
    /// 取用量记录时刻，耗时正常。对照 error 用例（起点取错误时刻）见
    /// import_codex_timing_error_then_usage_counts_as_retry。
    #[test]
    fn import_codex_aborted_then_usage_keeps_original_evidence() {
        let (tmp, mut conn, file) = setup_codex_case("abortusage");
        let u = usage_json(60, 15, 18, 4, 97);
        let lines = [
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.005Z"),
            user_message("2026-08-16T10:00:00.010Z"),
            agent_message_done("2026-08-16T10:00:03.050Z", ms("2026-08-16T10:00:03.000Z")),
            assistant_message("2026-08-16T10:00:03.060Z"),
            turn_aborted("2026-08-16T10:00:03.500Z"),
            // 中断之后没有新的边界，用量事后补写并结算
            usage_record("2026-08-16T10:00:04.000Z", &u),
            token_count("2026-08-16T10:00:04.010Z", &u),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);

        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1, "有用量 → 不出 0 token 行");
        let r = &rows[0];
        assert_eq!(r.status.as_deref(), Some("completed"));
        // retry_count 的现有行为不变：中断事件折入重试计数
        assert_eq!(r.retry_count, 1);
        assert_eq!(r.error_code, None);
        // 起点与首字取中断前证据（用户中断不重试，证据不作废）
        assert_eq!(
            r.request_started_at,
            Some(ms("2026-08-16T10:00:00.010Z")),
            "cancelled 不改写起点 → 仍是输出之前的边界"
        );
        assert_eq!(r.first_token_at, Some(ms("2026-08-16T10:00:03.000Z")));
        assert_eq!(
            r.completed_at,
            Some(ms("2026-08-16T10:00:04.000Z")),
            "用量一致 → 结束取用量记录时刻"
        );
        assert_eq!(r.duration_ms, Some(3990.0), "耗时是完整的正常请求窗口");
        assert_eq!(r.time_to_first_token_ms, Some(2990.0));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 重放幂等：同一文件从头再导一遍 → 行数与各列不变；token 与已入库行
    /// 不一致的行不被补计时（防止文件被改写后把计时贴到别的行上）。
    #[test]
    fn import_codex_replay_preserves_rows_and_skips_mismatched_tokens() {
        let (tmp, mut conn, file) = setup_codex_case("replay");
        let u = usage_json(100, 10, 30, 5, 145);
        let lines = [
            task_started("2026-08-16T10:00:00.000Z"),
            turn_context("2026-08-16T10:00:00.005Z"),
            user_message("2026-08-16T10:00:00.010Z"),
            agent_message_done("2026-08-16T10:00:05.000Z", ms("2026-08-16T10:00:03.000Z")),
            assistant_message("2026-08-16T10:00:05.010Z"),
            usage_record("2026-08-16T10:00:05.020Z", &u),
            token_count("2026-08-16T10:00:05.030Z", &u),
        ];
        write_session(&file, &lines);
        run_import(&mut conn, &file, None);
        let first = read_rows(&conn);
        assert_eq!(first.len(), 1);

        // 重放（known = None，与回填路径同构）：不新增行、各列不变
        run_import(&mut conn, &file, None);
        assert_eq!(read_rows(&conn), first, "重放不改变任何取证值");

        // 模拟文件被改写：把库中行的 token 改成与文件不一致的值、计时清空，
        // 再重放——token 不相等的行绝不补计时
        conn.execute_batch(
            "UPDATE model_usage SET input_tokens = 999, computed_total_tokens = 1109,
                duration_ms = NULL, request_started_at = NULL, first_token_at = NULL,
                completed_at = NULL, timing_estimated = NULL",
        )
        .expect("改写已入库行失败");
        run_import(&mut conn, &file, None);
        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1, "token 不一致的重放不新增行");
        let r = &rows[0];
        assert_eq!(r.input_tokens, 999, "token 不一致时已入库行不被覆盖");
        assert_eq!(r.duration_ms, None, "token 不一致时不补计时");
        assert_eq!(r.first_token_at, None);
        assert_eq!(r.completed_at, None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 计时回填标记只触发一次：新库 / 旧库 user_version = 0 时判定需要，
    /// 写入标记后不再触发（重放幂等的前提下避免每轮全量扫描）。
    #[test]
    fn timing_backfill_runs_once() {
        let (tmp, mut conn, _file) = setup_codex_case("backfill");
        // SCHEMA_SQL 建的新库 user_version 仍为 0（标记语义与表结构无关）
        assert!(
            timing_backfill_pending(&conn),
            "升级前 user_version < 2 应判定需要回填"
        );
        mark_timing_backfill_done(&mut conn);
        assert!(!timing_backfill_pending(&conn), "写入标记后不再触发");
        // 幂等：重复写标记无害
        mark_timing_backfill_done(&mut conn);
        assert!(!timing_backfill_pending(&conn));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 旧结构库经 ensure_timing_columns 补列后，与新库（SCHEMA_SQL）的列
    /// 集合完全一致（顺序无关），retry_count 带 NOT NULL DEFAULT 0 兜底。
    #[test]
    fn ensure_timing_columns_matches_new_schema() {
        let legacy = Connection::open_in_memory().expect("打开内存库失败");
        // 升级前的旧结构：无 9 个计时状态列
        legacy
            .execute_batch(
                "CREATE TABLE model_usage (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL,
                    event_seq INTEGER NOT NULL,
                    started_at INTEGER NOT NULL,
                    model_id TEXT NOT NULL DEFAULT '',
                    provider_id TEXT NOT NULL DEFAULT 'codex',
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                    model_revision_at INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(session_id, event_seq)
                 );
                 INSERT INTO model_usage (session_id, event_seq, started_at, model_id)
                 VALUES ('s-old', 1, 1000, 'gpt-5.1-codex');",
            )
            .expect("建旧结构表失败");

        ensure_timing_columns(&legacy).expect("补列迁移失败");
        // 幂等：重复执行不报错
        ensure_timing_columns(&legacy).expect("重复补列应幂等");

        let columns = |conn: &Connection| -> Vec<String> {
            let mut stmt = conn
                .prepare("SELECT name FROM pragma_table_info('model_usage')")
                .unwrap();
            let mut names: Vec<String> = stmt
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            names.sort();
            names
        };
        let fresh = Connection::open_in_memory().expect("打开新库失败");
        fresh.execute_batch(SCHEMA_SQL).expect("建新库失败");
        assert_eq!(
            columns(&legacy),
            columns(&fresh),
            "补列后的旧库与新库列集合应一致"
        );
        // retry_count 的 NOT NULL DEFAULT 0 兜底：旧行迁移后可读且为 0
        let retry: (i64, Option<String>) = legacy
            .query_row(
                "SELECT retry_count, status FROM model_usage WHERE session_id = 's-old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读取迁移后的旧行失败");
        assert_eq!(retry, (0, None));
    }

    /// 续读：边界行（用户输入）在上次已读区域、输出与结算在新增区域 →
    /// 回看重建上下文后计时完整（起点对准旧区域的用户输入时刻）。
    #[test]
    fn import_codex_lookback_rebuilds_boundary_context() {
        let (tmp, mut conn, file) = setup_codex_case("lookback");
        let u = usage_json(80, 20, 25, 4, 129);
        // 第一段：只有边界行（用户已输入、模型还没输出）
        write_session(
            &file,
            &[
                task_started("2026-08-16T10:00:00.000Z"),
                turn_context("2026-08-16T10:00:00.005Z"),
                user_message("2026-08-16T10:00:00.010Z"),
            ],
        );
        run_import(&mut conn, &file, None);
        assert!(
            read_rows(&conn).is_empty(),
            "没有结算不应产生数据行"
        );
        let (offset, size): (i64, i64) = conn
            .query_row(
                "SELECT offset, size FROM file_progress WHERE path = ?1",
                rusqlite::params![file.to_string_lossy().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读取导入进度失败");

        // 第二段：等了很久之后输出与结算到达
        let mut grown = std::fs::read_to_string(&file).expect("读取会话文件失败");
        grown.pop(); // 去掉此前的结尾换行，追加新行
        grown.push('\n');
        grown.push_str(&agent_message_done(
            "2026-08-16T10:00:12.000Z",
            ms("2026-08-16T10:00:10.000Z"),
        ));
        grown.push('\n');
        grown.push_str(&assistant_message("2026-08-16T10:00:12.010Z"));
        grown.push('\n');
        grown.push_str(&usage_record("2026-08-16T10:00:12.020Z", &u));
        grown.push('\n');
        grown.push_str(&token_count("2026-08-16T10:00:12.030Z", &u));
        grown.push('\n');
        std::fs::write(&file, grown).expect("追加会话内容失败");

        run_import(&mut conn, &file, Some((offset as u64, size as u64)));
        let rows = read_rows(&conn);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(
            r.request_started_at,
            Some(ms("2026-08-16T10:00:00.010Z")),
            "回看重建的边界（旧区域用户输入）应作为起点"
        );
        assert_eq!(r.first_token_at, Some(ms("2026-08-16T10:00:10.000Z")));
        assert_eq!(r.completed_at, Some(ms("2026-08-16T10:00:12.020Z")));
        assert_eq!(r.duration_ms, Some(12010.0));
        assert_eq!(r.time_to_first_token_ms, Some(9990.0));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 本机只读审计：对真实 ~/.codex/sessions 下所有 .jsonl 跑同一套状态机
    ///（不写库），按 session_meta 的 cli_version 分桶只打印聚合数字，对照
    /// 计划「本机数据依据」。仅人工核对，不计入常规测试（#[ignore]）。
    #[test]
    #[ignore]
    fn local_codex_timing_audit() {
        let Ok(dir) = sessions_dir() else {
            eprintln!("[audit] 本机无 Codex sessions 目录，跳过");
            return;
        };
        let mut files = Vec::new();
        collect_session_files(&dir, 3, &mut files);
        files.sort();

        /// 读首行 session_meta 的 payload.cli_version（读不到按 unknown 分桶）
        fn cli_version(path: &Path) -> String {
            let Ok(file) = std::fs::File::open(path) else {
                return "unknown".into();
            };
            let mut reader = std::io::BufReader::new(file);
            let mut buf = Vec::with_capacity(4096);
            use std::io::BufRead;
            if reader.read_until(b'\n', &mut buf).unwrap_or(0) == 0 {
                return "unknown".into();
            }
            serde_json::from_slice::<RolloutLine>(&buf)
                .ok()
                .and_then(|line| line.payload)
                .and_then(|payload| payload.get("cli_version").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_else(|| "unknown".into())
        }

        #[derive(Default)]
        struct Bucket {
            requests: u64,
            with_request_start: u64,
            with_first_output: u64,
            with_stream_window: u64,
            zero_token_rows: u64,
        }

        let mut buckets: std::collections::BTreeMap<String, Bucket> = std::collections::BTreeMap::new();
        for path in &files {
            let Ok(file) = std::fs::File::open(path) else {
                continue;
            };
            let version = cli_version(path);
            let mut timer = RequestTimer::default();
            let collect = |outcome: Option<TimerOutcome>, bucket: &mut Bucket| {
                match outcome {
                    Some(TimerOutcome::Usage(s)) => {
                        bucket.requests += 1;
                        if s.request_started_at.is_some() {
                            bucket.with_request_start += 1;
                        }
                        if s.first_token_at.is_some() {
                            bucket.with_first_output += 1;
                        }
                        // 流式速度原料：首字与结束齐全且生成窗口合法
                        if let (Some(first), Some(done)) = (s.first_token_at, s.completed_at) {
                            let span = done - first;
                            if s.output_tokens > 0
                                && span >= crate::token_speed::MIN_GEN_MS
                                && span <= crate::token_speed::MAX_GEN_MS
                            {
                                bucket.with_stream_window += 1;
                            }
                        }
                    }
                    Some(TimerOutcome::Zero(_)) => bucket.zero_token_rows += 1,
                    None => {}
                }
            };
            let bucket = buckets.entry(version).or_default();
            let mut bucket = &mut *bucket;
            use std::io::BufRead;
            for line in std::io::BufReader::new(file).lines().flatten() {
                if let Ok(line) = serde_json::from_str::<RolloutLine>(&line) {
                    collect(timer.feed(&line), &mut bucket);
                }
            }
            // 文件读尽：只结算仍挂起的用户中断（与生产 EOF 结算同语义）
            collect(timer.finalize_pending_at_eof(), &mut bucket);
        }
        eprintln!(
            "[audit] 文件数={}（目录 {}）",
            files.len(),
            dir.display()
        );
        eprintln!(
            "[audit] {:<10} {:>8} {:>10} {:>12} {:>14} {:>10}",
            "version", "请求数", "有起点", "有首个输出", "两者+窗口≥200ms", "0token行"
        );
        for (version, bucket) in &buckets {
            let pct = |count: u64| -> String {
                if bucket.requests == 0 {
                    "—".to_string()
                } else {
                    format!("{:.1}%", count as f64 * 100.0 / bucket.requests as f64)
                }
            };
            eprintln!(
                "[audit] {:<10} {:>8} {:>10} {:>12} {:>14} {:>10}",
                version,
                bucket.requests,
                pct(bucket.with_request_start),
                pct(bucket.with_first_output),
                pct(bucket.with_stream_window),
                bucket.zero_token_rows,
            );
        }
    }
}

