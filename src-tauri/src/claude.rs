//! Claude Code 用量统计模块。
//!
//! 数据来源：Claude Code 把每个会话记录在
//! ~/.claude/projects/<项目目录>/<会话uuid>.jsonl（append-only，每行一个 JSON
//! 事件；子代理会话在 <会话uuid>/subagents/ 下一层，递归遍历）。
//! token 用量在 type=assistant 行的 message.usage 里（input/output/
//! cache_read/cache_creation，无 total 字段，四者之和即总量）。
//!
//! 与 Codex 模块的关键差异：
//! - **去重键**：同一 message.id 会写多行（CLI 边流式边落盘：旧版中间块
//!   usage 全 0、末行为终值；新版每块都带最终用量）。按行直接累加会重复
//!   计数，故按 message.id 全局去重（resume/continue 是 fork 语义，新会话
//!   文件复制原历史且保留原 message.id，跨文件也必须合并为一条）；冲突时
//!   保留 computed_total 更大的一行，并打上 updated_at 修订标记——同步层
//!   据此把已上传的中间值补传修正为终值（Claude 独有：Codex 记录落盘后
//!   不可变）。
//! - **额度来源**：会话文件里没有 rate_limits 快照（Codex 有），订阅额度只能
//!   实时调 OAuth 端点 GET https://api.anthropic.com/api/oauth/usage
//!   （Claude Code CLI 内部同款，需 claude.ai 订阅登录；第三方中转/API Key
//!   模式无凭据，额度块整体不展示）。凭据只读，绝不刷新/写回——refresh_token
//!   是一次性轮换的，外部写回会搞坏 Claude Code 登录。
//!
//! 实现方式与 codex.rs 同构：原始 jsonl 只读 + 派生自有库 ~/.agenthud/claude.sqlite
//! （更名前 ~/.zbar，启动自动迁移），file_progress 偏移增量续读。

use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::db;
use crate::pricing::config_dir;

// ===== 路径定位 =====

/// 会话窗口查询（query_window_sessions）内层活跃扫查的行数上限（窗口内
/// 最近 N 笔请求，started_at 降序 LIMIT）：窗口内行数可能很大，以最近 N
/// 笔为界保证查询成本有硬上限；最新会话（排序前几名）必然在内层结果中
const WINDOW_SCAN_ROWS: i64 = 2000;

/// Claude projects 目录路径（不做存在性检查，供诊断展示）。
/// 环境变量 ZBAR_CLAUDE_HOME（指向 .claude 根目录）优先，否则 ~/.claude/projects。
fn projects_dir_path() -> PathBuf {
    if let Ok(home) = std::env::var("ZBAR_CLAUDE_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join("projects");
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".claude").join("projects")
}

/// 定位 Claude 会话目录。目录不存在返回友好中文错误（调用方按需容错降级）。
pub fn projects_dir() -> Result<PathBuf, String> {
    let p = projects_dir_path();
    if p.is_dir() {
        Ok(p)
    } else {
        Err(format!(
            "未找到 Claude 会话目录: {}。请确认 Claude Code 已安装并使用过，或设置 ZBAR_CLAUDE_HOME 环境变量指向 .claude 根目录。",
            p.display()
        ))
    }
}

/// 自有导入库路径：~/.agenthud/claude.sqlite（更名前 ~/.zbar，启动自动迁移）
pub(crate) fn claude_db_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("claude.sqlite"))
}

/// 打开（必要时创建）导入库并确保表结构就绪。这是自有库，读写均可用。
/// 与 codex.sqlite 同构，但无 rate_limits_state 表（会话文件里没有额度数据）。
///
/// 去重键 dedupe_key（全局唯一）：
/// - 有 message.id 的行 → message.id 本身（Claude Code 的 resume/continue 是
///   fork 语义：新会话文件会复制原历史且保留原 message.id，按会话内去重会把
///   同一次 API 调用算两遍，故 id 行必须跨文件全局去重）；
/// - 无 id 的旧行 → "<session_id>|<行序号>"（仅会话内去重）。
/// 冲突时保留 computed_total 更大的一行（同 id 多行 usage 为累计口径，末行
/// 即终值；取更大者防御末行意外回退的脏数据）。被覆盖的行 updated_at 记录
/// 修订时间，供同步补传（否则已上传的中间值永远不会被终值修正）。

/// 进程内串行化结构迁移（codex.rs SCHEMA_MIGRATION_LOCK 同款）。
static SCHEMA_MIGRATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn schema_migration_lock() -> &'static Mutex<()> {
    SCHEMA_MIGRATION_LOCK.get_or_init(|| Mutex::new(()))
}

fn open_claude_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = claude_db_path()?;
    let conn = Connection::open(&path).map_err(|e| format!("打开 Claude 导入库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;

    // 结构版本迁移：表存在但缺 dedupe_key / duration_ms 列（旧版结构）时整表重建。
    // 本库是从原始 jsonl 全量派生的缓存库，重建后下次导入自动补齐。重建后自增
    // id 从头重排，普通 id 游标选不出低 id 行，靠修订通道（query_revised_since，
    // 重建行 updated_at=导入时刻）全量补传到服务端收敛——勿删修订通道。
    // 进程内互斥：升级后首次启动多个命令并发 open，避免同时判旧+交错 DROP/CREATE
    //（跨进程竞态由 DROP IF EXISTS / CREATE IF NOT EXISTS 幂等兜底）。
    {
        let _guard = schema_migration_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let legacy = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'table' AND name = 'model_usage')
                      + (SELECT COUNT(*) FROM pragma_table_info('model_usage')
                         WHERE name = 'dedupe_key')
                      + (SELECT COUNT(*) FROM pragma_table_info('model_usage')
                         WHERE name = 'duration_ms')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c == 1 || c == 2) // 表存在(1)+两列全缺(0)→1；表存在+缺一列→2；表不存在→0 或新结构→3
            .unwrap_or(false);
        if legacy {
            eprintln!("[agenthud-claude] 检测到旧版导入库结构，重建（自动从原始会话重新导入）");
            conn.execute_batch("DROP TABLE IF EXISTS model_usage; DROP TABLE IF EXISTS file_progress;")
                .map_err(|e| format!("重建 Claude 导入库失败: {e}"))?;
        }
    }

    conn.execute_batch(SCHEMA_SQL)
        .map_err(|e| format!("初始化 Claude 导入库失败: {e}"))?;
    ensure_project_columns(&conn)?;
    ensure_timing_columns(&conn)?;
    Ok(conn)
}

/// 导入库建表 DDL（`open_claude_db` 与测试共用，避免测试手抄结构漂移）。
/// synchronous=NORMAL 取舍：WAL 模式下仅在 checkpoint 时 fsync，掉电至多
/// 丢最后一个提交的事务；file_progress 与数据行同事务、丢失后一起回退，
/// 幂等重导即可补齐——统计型数据可接受，换来批量导入的写放大大幅下降。
const SCHEMA_SQL: &str = "PRAGMA journal_mode=WAL;
     PRAGMA synchronous=NORMAL;
     CREATE TABLE IF NOT EXISTS model_usage (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL,
        dedupe_key TEXT NOT NULL,
        started_at INTEGER NOT NULL,
        model_id TEXT NOT NULL DEFAULT '',
        provider_id TEXT NOT NULL DEFAULT 'claude',
        input_tokens INTEGER NOT NULL DEFAULT 0,
        output_tokens INTEGER NOT NULL DEFAULT 0,
        cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
        cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
        reasoning_tokens INTEGER NOT NULL DEFAULT 0,
        computed_total_tokens INTEGER NOT NULL DEFAULT 0,
        duration_ms REAL,
        updated_at INTEGER NOT NULL DEFAULT 0,
        cwd TEXT,
        project_key TEXT,
        -- 计时 / 状态取证列（统一口径，ZCode 主库同名列语义一致）：
        -- request_started_at 请求起点（api-request 行时间戳或父链推断）；
        -- first_token_at 首个输出时刻（仅日志有明确记录才写）；
        -- completed_at 回复结束时刻（各块时间戳最大值）；
        -- timing_estimated 1 = 起点是父链推断的（界面带 ≈）；
        -- status 'completed' / 'error'（NULL = 未知或进行中）；
        -- error_code 最终失败码或最近一次重试码；retry_count 失败尝试次数。
        request_started_at INTEGER,
        first_token_at INTEGER,
        completed_at INTEGER,
        time_to_first_token_ms REAL,
        timing_estimated INTEGER,
        status TEXT,
        error_code TEXT,
        retry_count INTEGER NOT NULL DEFAULT 0,
        UNIQUE(dedupe_key)
     );
     CREATE INDEX IF NOT EXISTS idx_claude_model_usage_started ON model_usage(started_at);
     CREATE INDEX IF NOT EXISTS idx_claude_model_usage_updated ON model_usage(updated_at);
     CREATE INDEX IF NOT EXISTS idx_claude_mu_session ON model_usage(session_id);
     CREATE TABLE IF NOT EXISTS file_progress (
        path   TEXT    PRIMARY KEY,
        offset INTEGER NOT NULL,
        size   INTEGER NOT NULL
     );";

/// 进程内串行化补列迁移（kimi.rs ENSURE_DURATION_LOCK 同款）：
/// 升级后首次启动多个查询命令并发 open，避免同时判缺列 + 交错重复 ALTER。
static ENSURE_PROJECT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// 项目维度补列迁移：升级前创建的旧库 model_usage 无 cwd / project_key 列时
/// ALTER TABLE ADD COLUMN 补加（允许 NULL，旧行自然为 NULL，由存量回填补齐）。
/// 先 PRAGMA table_info 检查再 ALTER，幂等；新库已由 CREATE TABLE 直接带列。
fn ensure_project_columns(conn: &Connection) -> Result<(), String> {
    let lock = ENSURE_PROJECT_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for column in ["cwd", "project_key"] {
        if db::has_column(conn, "model_usage", column) {
            continue;
        }
        conn.execute_batch(&format!("ALTER TABLE model_usage ADD COLUMN {column} TEXT"))
            .map_err(|e| format!("迁移 Claude 导入库（补 {column} 列）失败: {e}"))?;
    }
    Ok(())
}

/// 进程内串行化计时列迁移（ensure_project_columns 同款锁风格）。
static ENSURE_TIMING_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// 计时 / 状态列迁移：升级前创建的旧库 model_usage 无计时状态 8 列时逐列
/// ALTER TABLE ADD COLUMN 补加（允许 NULL，旧行自然为 NULL，由 90 天回填
/// 重放补齐；retry_count 有 NOT NULL DEFAULT 0 兜底）。先 has_column 检查
/// 再 ALTER，幂等；新库已由 CREATE TABLE 直接带列。
fn ensure_timing_columns(conn: &Connection) -> Result<(), String> {
    let lock = ENSURE_TIMING_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    const TIMING_COLUMNS: [(&str, &str); 8] = [
        ("request_started_at", "INTEGER"),
        ("first_token_at", "INTEGER"),
        ("completed_at", "INTEGER"),
        ("time_to_first_token_ms", "REAL"),
        ("timing_estimated", "INTEGER"),
        ("status", "TEXT"),
        ("error_code", "TEXT"),
        ("retry_count", "INTEGER NOT NULL DEFAULT 0"),
    ];
    for (column, decl) in TIMING_COLUMNS {
        if db::has_column(conn, "model_usage", column) {
            continue;
        }
        // 另一个进程可能在本次检查与 ALTER 之间完成了迁移，此时重复列就是
        // 成功状态，后续查询可直接继续（codex.rs ensure_timing_columns 同款）
        if let Err(e) =
            conn.execute(&format!("ALTER TABLE model_usage ADD COLUMN {column} {decl}"), [])
        {
            let message = e.to_string().to_ascii_lowercase();
            if !message.contains("duplicate column name") {
                return Err(format!("迁移 Claude 导入库（补 {column} 列）失败: {e}"));
            }
        }
    }
    Ok(())
}

// ===== jsonl 行解析结构（未知字段自动忽略，巨大的 content 数组不会物化）=====
//
// 计时 / 状态取证用的新字段全部 `#[serde(default)]` + 容错反序列化：
// 这是 Claude Code 的内部格式、没有公开文档，缺字段 / 字段类型变化一律
// 降级为 None，绝不让整行解析失败（旧版行没有这些字段，user 行的
// message.content 是字符串而非块数组）。

/// 容错反序列化：任何 JSON 值都能进 Value，只有非 null 才返回 Some。
/// 数值字段的类型在版本间可能漂移（整数 / 浮点 / 字符串），逐字段按需提取。
fn lenient_value<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<serde_json::Value> = serde::Deserialize::deserialize(deserializer)?;
    Ok(value.filter(|v| !v.is_null()))
}

/// message.content 容错反序列化：assistant 行是块数组（取首块 type）、
/// user 行可能是纯字符串、缺失为 null——只有非空数组才保留，其余一律
/// None，保证整行解析永不因 content 形态失败。
fn lenient_content_first_block_type<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<serde_json::Value> = serde::Deserialize::deserialize(deserializer)?;
    let first = match value {
        Some(serde_json::Value::Array(items)) => items.into_iter().next(),
        _ => None,
    };
    Ok(first
        .and_then(|block| block.get("type").and_then(|t| t.as_str()).map(str::to_string)))
}

/// jsonl 单行事件。只取关心的时间戳/类型/message。
/// cwd 为行内顶层字段（会话的工作目录，user/assistant 行均携带），
/// 供项目维度聚合使用；旧行/无 cwd 的行为 None。
#[derive(Debug, Deserialize)]
struct TranscriptLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    line_type: Option<String>,
    #[serde(default)]
    message: Option<AssistantMessage>,
    #[serde(default)]
    cwd: Option<String>,
    // ---- 计时 / 状态取证字段（全部可选，缺字段降级）----
    #[serde(default)]
    uuid: Option<String>,
    #[serde(rename = "parentUuid", default)]
    parent_uuid: Option<String>,
    /// system 行的子类型（api_error 重试行）
    #[serde(default)]
    subtype: Option<String>,
    /// api-request 行的请求 id（与回复块的 requestRef 相等）
    #[serde(default)]
    id: Option<String>,
    /// CLI 版本（审计测试按它分桶统计覆盖率）
    #[serde(default)]
    version: Option<String>,
    /// assistant 块行指向本次请求的引用（= api-request 行的 id）
    #[serde(rename = "requestRef", default, deserialize_with = "lenient_value")]
    request_ref_json: Option<serde_json::Value>,
    #[serde(rename = "requestedModel", default, deserialize_with = "lenient_value")]
    requested_model_json: Option<serde_json::Value>,
    /// 思考块产出前的思考耗时（毫秒）
    #[serde(rename = "thinkingDurationMs", default, deserialize_with = "lenient_value")]
    thinking_duration_ms_json: Option<serde_json::Value>,
    /// 工具调用 / 文字块开始流出前的耗时（毫秒）
    #[serde(rename = "toolInputStreamMs", default, deserialize_with = "lenient_value")]
    tool_input_stream_ms_json: Option<serde_json::Value>,
    /// 块序号（首块为 0），旧版行没有 → 取最先出现的块当首块
    #[serde(rename = "apiBlockIndex", default, deserialize_with = "lenient_value")]
    api_block_index_json: Option<serde_json::Value>,
    /// 最终失败的回复标记（true = 这条回复是一次失败的 API 调用）
    #[serde(rename = "isApiErrorMessage", default, deserialize_with = "lenient_value")]
    is_api_error_message_json: Option<serde_json::Value>,
    /// 失败回复的 HTTP 状态码（如 429）
    #[serde(rename = "apiErrorStatus", default, deserialize_with = "lenient_value")]
    api_error_status_json: Option<serde_json::Value>,
    /// system/api_error 行里是对象（含 status），失败回复里是字符串
    #[serde(default, deserialize_with = "lenient_value")]
    error: Option<serde_json::Value>,
}

impl TranscriptLine {
    fn request_ref(&self) -> Option<String> {
        json_value_to_string(self.request_ref_json.as_ref())
    }
    fn requested_model(&self) -> Option<String> {
        json_value_to_string(self.requested_model_json.as_ref())
    }
    fn thinking_duration_ms(&self) -> Option<f64> {
        json_value_to_f64(self.thinking_duration_ms_json.as_ref())
    }
    fn tool_input_stream_ms(&self) -> Option<f64> {
        json_value_to_f64(self.tool_input_stream_ms_json.as_ref())
    }
    fn api_block_index(&self) -> Option<i64> {
        json_value_to_i64(self.api_block_index_json.as_ref())
    }
    fn is_api_error_message(&self) -> bool {
        self.is_api_error_message_json
            .as_ref()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
    fn api_error_status(&self) -> Option<i64> {
        json_value_to_i64(self.api_error_status_json.as_ref())
    }
}

/// Value → 字符串（数字 / 布尔等标量原样转文本，复合结构忽略）
fn json_value_to_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Value → f64（数值直取；数字字符串防御性解析）
fn json_value_to_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Value → i64（浮点防御性取整）
fn json_value_to_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)),
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// assistant 行的 message 对象（content 只取首块 type，serde 忽略其余字段）。
/// durationMs 为该次调用的总耗时（毫秒），旧版本 CLI 的行无此字段 → None。
#[derive(Debug, Deserialize)]
struct AssistantMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(rename = "durationMs", default)]
    duration_ms: Option<f64>,
    #[serde(default)]
    usage: Option<UsagePayload>,
    /// 末块的结束原因（end_turn / tool_use / stop_sequence…），中间块为 null
    #[serde(rename = "stop_reason", default, deserialize_with = "lenient_value")]
    stop_reason_json: Option<serde_json::Value>,
    /// 首个 content 块的类型（thinking / text / tool_use / *_tool_use），
    /// content 为字符串（user 行）或缺失时为 None
    #[serde(
        rename = "content",
        default,
        deserialize_with = "lenient_content_first_block_type"
    )]
    first_block_type: Option<String>,
}

impl AssistantMessage {
    fn stop_reason(&self) -> Option<String> {
        json_value_to_string(self.stop_reason_json.as_ref())
    }
}

/// 单次 API 调用的 token 用量。Claude 无 total 字段，四项之和即总量；
/// 也无独立 reasoning 字段（thinking 计入 output）。
#[derive(Debug, Default, Clone, Deserialize)]
struct UsagePayload {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
    #[serde(default)]
    cache_creation_input_tokens: i64,
}

impl UsagePayload {
    fn computed_total(&self) -> i64 {
        self.input_tokens + self.output_tokens + self.cache_read_input_tokens
            + self.cache_creation_input_tokens
    }
}

/// ISO8601 时间戳（如 2026-06-12T19:12:00.759Z）→ 毫秒时间戳
fn parse_ts_ms(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// 从文件名提取会话标识（<uuid>.jsonl → <uuid>）。
/// 会话文件与其子代理文件都以 uuid 命名，天然互不重名；主干即稳定标识。
fn session_id_from_filename(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

// ===== 按回复取证（解析核心：输入行 → 产出回复记录，无 IO，方便单测与审计复用）=====
//
// 一条回复（一次 API 响应）= 同一个 message.id 的全部 assistant 块行。新版
// 每个块行都带最终用量，块之间会夹着 user 工具结果行（客户端一边收流一边
// 执行工具，模型继续输出下一个工具调用块；本机最近 7 天 5366 条多块响应里
// 有 1316 个块前面夹着工具结果行，夹行之后的块计时仍可信）——所以工具结果
// 行、attachment、system 等行**不结束**当前回复，一条响应的所有块进同一条
// 记录。只有四种情况结束当前回复：message.id 不同（或没有 id）的 assistant
// 行、api-request 行（新请求开始）、真正的用户输入行（user 且 content 不是
// 工具结果）、文件读尽。

/// 续读回看窗口大小：重建父链 / api-request 索引 / 未归属 api_error 所需
/// 的游标前字节数（只读不写库）。取 1MB 的依据：本机 90 天内 5583 条回复
/// 的起点证据离首块全部在 1MB 以内，其中超过 256KB 的有 14 条（单条回复
/// 的流式块可能很长，256KB 会漏掉跨长块的起点证据）。
const LOOKBACK_BYTES: u64 = 1024 * 1024;
/// 父链索引保留的最近行数（超过淘汰最老，防止长会话内存无限增长）
const PARENT_CHAIN_KEEP: usize = 4096;
/// api-request 索引保留的最近条数
const API_REQUEST_KEEP: usize = 256;
/// 父链向上查找的最大跳数（防御数据里的 uuid 环）
const PARENT_CHAIN_MAX_HOPS: usize = 64;
/// 计时回填的 PRAGMA user_version 标记值（1 留给历史迁移语义，本轮写 2）
const TIMING_BACKFILL_VERSION: i64 = 2;
/// 计时回填只重放最近修改过的文件（90 天；更早的保持现状）
const TIMING_BACKFILL_WINDOW_MS: i64 = 90 * 24 * 3_600_000;

/// 窗口内一条回复的最终取证记录（写库层与解析层的唯一交接物）。
/// 各字段语义见 SCHEMA_SQL 的列注释；has_new_block 供增量续读判定
/// 「至少有一个块落在新增字节里」，false 时调用方不写库。
#[derive(Debug, Clone, PartialEq)]
struct ReplyRecord {
    message_id: String,
    started_at: i64,
    model_id: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    computed_total: i64,
    /// 行内真实 durationMs（有则原样落库，不做窗口校验——与历史口径一致）
    real_duration_ms: Option<f64>,
    /// completed_at − request_started_at（派生值，窗口校验通过才给）
    derived_duration_ms: Option<f64>,
    request_started_at: Option<i64>,
    first_token_at: Option<i64>,
    completed_at: Option<i64>,
    time_to_first_token_ms: Option<f64>,
    /// Some(0) = 起点来自 api-request 行；Some(1) = 父链推断；None = 无起点
    timing_estimated: Option<i64>,
    /// "completed" / "error"；None = 文件停在流式回复中间（未知 / 进行中）
    status: Option<&'static str>,
    error_code: Option<String>,
    retry_count: i64,
    has_new_block: bool,
    cwd: Option<String>,
    project_key: Option<String>,
    /// 回复首块的 CLI 版本（仅审计测试分桶用，不落库）
    version: Option<String>,
    /// 多块回复的块之间夹有 user 工具结果行（仅审计统计用，不落库）
    tool_result_between: bool,
}

impl ReplyRecord {
    /// 落库口径的耗时：真实值优先，否则派生值（各自已按规则校验 / 原样）
    fn duration_ms(&self) -> Option<f64> {
        self.real_duration_ms.or(self.derived_duration_ms)
    }
}

/// 写库 / 合并层的行快照（ReplyRecord 与库内已存行的统一形态，便于纯函数合并）。
/// 不含 session_id / cwd / project_key / updated_at（合并永不触碰它们）。
#[derive(Debug, Clone, PartialEq)]
struct StoredReply {
    started_at: i64,
    model_id: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    computed_total: i64,
    duration_ms: Option<f64>,
    request_started_at: Option<i64>,
    first_token_at: Option<i64>,
    completed_at: Option<i64>,
    time_to_first_token_ms: Option<f64>,
    timing_estimated: Option<i64>,
    status: Option<String>,
    error_code: Option<String>,
    retry_count: i64,
}

impl ReplyRecord {
    fn as_stored(&self) -> StoredReply {
        StoredReply {
            started_at: self.started_at,
            model_id: self.model_id.clone(),
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            computed_total: self.computed_total,
            duration_ms: self.duration_ms(),
            request_started_at: self.request_started_at,
            first_token_at: self.first_token_at,
            completed_at: self.completed_at,
            time_to_first_token_ms: self.time_to_first_token_ms,
            timing_estimated: self.timing_estimated,
            status: self.status.map(str::to_string),
            error_code: self.error_code.clone(),
            retry_count: self.retry_count,
        }
    }
}

/// 已入库行与新回复的合并（纯函数，可单测）。规则：
/// - token 各列 / started_at / model_id 仍然是「总量更大者覆盖」，不变；
/// - 新总量 **更大**：计时与状态 9 列（request_started_at / first_token_at /
///   completed_at / duration_ms / time_to_first_token_ms / timing_estimated /
///   status / error_code）新值非空就用新值；retry_count 权威重放取新值
///   （取证完整可纠正错归属），续读取较大者；
/// - 新总量 **相等**：同一条响应分趟落盘（新版每块都带最终用量），读到了
///   更多块。续读与权威重放同规则，仅起点四列不同：
///   1. completed_at：取两者最大值（两个都有取大的，只有一个用那个）；
///   2. duration_ms：completed_at 或 request_started_at 发生变化（合并后
///      的值与已入库的不同）时用新值（新值为空才保留旧值）——起点被纠正
///      成精确值后耗时必须跟着重算；两者都没变化时保留已入库值（旧值为
///      空才补新值）；
///   3. request_started_at / first_token_at / time_to_first_token_ms /
///      timing_estimated：续读旧值为空才补；权威重放时新记录**有请求起点**
///      则四列整体用新值（新值为空就写空——整文件重算判定证据无效后置空，
///      必须能清掉此前续读写入的错值），新记录没有起点才按「旧值为空才补」；
///   4. status / error_code 新值非空就用新值；retry_count 权威重放取新值，
///      续读取较大者；
/// - 新总量 **更小**（权威重放），或续读且新总量 < 旧总量（上面相等分支
///   之外）：计时与状态列只在旧值为空时补新值；completed_at 一律不改；
///   retry_count 取两者较大者；
/// - 调用方比较合并结果与旧值，完全相同则不写、不动 updated_at。
fn merge_reply(stored: &StoredReply, fresh: &StoredReply, authoritative: bool) -> StoredReply {
    let mut out = stored.clone();
    let tokens_win = fresh.computed_total > stored.computed_total;
    if tokens_win {
        out.started_at = fresh.started_at;
        // model_id 空串不覆盖非空旧值（失败回复的模型名回退全部失败时
        // fresh 仍为空，不能把已入库的真实模型名抹掉）
        if !fresh.model_id.is_empty() {
            out.model_id = fresh.model_id.clone();
        }
        out.input_tokens = fresh.input_tokens;
        out.output_tokens = fresh.output_tokens;
        out.cache_read_tokens = fresh.cache_read_tokens;
        out.cache_creation_tokens = fresh.cache_creation_tokens;
        out.computed_total = fresh.computed_total;
    }
    // 分支键：新总量更大（两种模式同规则）→ 权威重放且总量相等 → 其余
    //（权威重放且总量更小、续读且不大于旧总量，同一套「旧值非空不动」规则）
    let equal = fresh.computed_total == stored.computed_total;
    if tokens_win {
        // 新总量更大（两种模式同规则）
        out.duration_ms = fresh.duration_ms.or(stored.duration_ms);
        out.request_started_at = fresh.request_started_at.or(stored.request_started_at);
        out.first_token_at = fresh.first_token_at.or(stored.first_token_at);
        out.completed_at = fresh.completed_at.or(stored.completed_at);
        out.time_to_first_token_ms = fresh
            .time_to_first_token_ms
            .or(stored.time_to_first_token_ms);
        out.timing_estimated = fresh.timing_estimated.or(stored.timing_estimated);
        out.status = fresh.status.clone().or(stored.status.clone());
        out.error_code = fresh.error_code.clone().or(stored.error_code.clone());
        // 权威重放的取证是完整的（所有 api_error 行都读到过），retry_count
        // 以新值为准——纠正被旧版代码写错的归属；续读取较大者兜底
        out.retry_count = if authoritative {
            fresh.retry_count
        } else {
            stored.retry_count.max(fresh.retry_count)
        };
    } else if equal {
        // 总量相等：同一条响应分趟落盘（新版每块都带最终用量），读到了
        // 更多块 → completed_at 取两者的最大值（两个都有取大的，只有一个
        // 用那个）
        out.completed_at = match (stored.completed_at, fresh.completed_at) {
            (Some(old), Some(new)) => Some(old.max(new)),
            (old, None) => old,
            (None, new) => new,
        };
        // 起点 / 首字 / 估算标记（先合并，duration 的更新条件依赖合并后的起点）
        if authoritative && fresh.request_started_at.is_some() {
            // 整文件重算判定某项证据无效后置空，必须能清掉此前续读时写入
            // 的错值（例如「首字早于起点」）：四列整体跟随新记录
            out.request_started_at = fresh.request_started_at;
            out.first_token_at = fresh.first_token_at;
            out.time_to_first_token_ms = fresh.time_to_first_token_ms;
            out.timing_estimated = fresh.timing_estimated;
        } else {
            // 续读，或权威重放但新记录没有起点（旧版本文件）：旧值非空不动
            out.request_started_at = stored.request_started_at.or(fresh.request_started_at);
            out.first_token_at = stored.first_token_at.or(fresh.first_token_at);
            out.time_to_first_token_ms = stored
                .time_to_first_token_ms
                .or(fresh.time_to_first_token_ms);
            out.timing_estimated = stored.timing_estimated.or(fresh.timing_estimated);
        }
        out.status = fresh.status.clone().or(stored.status.clone());
        out.error_code = fresh.error_code.clone().or(stored.error_code.clone());
        // duration 随 completed_at 或起点的变化重算：任一变化用新值（新值
        // 为空才保留旧值）——否则起点被纠正成精确值后，耗时还是按旧起点算
        // 的，两者对不上；都没变化保留已入库值（旧值为空才补新值）
        let timing_changed = out.completed_at != stored.completed_at
            || out.request_started_at != stored.request_started_at;
        out.duration_ms = if timing_changed {
            fresh.duration_ms.or(stored.duration_ms)
        } else {
            stored.duration_ms.or(fresh.duration_ms)
        };
        out.retry_count = if authoritative {
            fresh.retry_count
        } else {
            stored.retry_count.max(fresh.retry_count)
        };
        // 已入库的 model_id 为空（模型名回退修正生效前的失败行）而新记录
        // 非空时补上；非空旧值不动
        if out.model_id.is_empty() && !fresh.model_id.is_empty() {
            out.model_id = fresh.model_id.clone();
        }
    } else {
        // 新总量更小（权威），或续读且不大于旧总量：旧值非空不动
        out.duration_ms = stored.duration_ms.or(fresh.duration_ms);
        out.request_started_at = stored.request_started_at.or(fresh.request_started_at);
        out.first_token_at = stored.first_token_at.or(fresh.first_token_at);
        out.time_to_first_token_ms = stored
            .time_to_first_token_ms
            .or(fresh.time_to_first_token_ms);
        out.timing_estimated = stored.timing_estimated.or(fresh.timing_estimated);
        out.status = stored.status.clone().or(fresh.status.clone());
        out.error_code = stored.error_code.clone().or(fresh.error_code.clone());
        // completed_at 不改（防止推后）
        out.retry_count = stored.retry_count.max(fresh.retry_count);
        // 已入库的 model_id 为空时用新值补上（同上，续读也补，无害）
        if out.model_id.is_empty() && !fresh.model_id.is_empty() {
            out.model_id = fresh.model_id.clone();
        }
    }
    out
}

/// 累积中的一条回复（同一 message.id 的全部块行，可跨工具结果行）
#[derive(Debug)]
struct ReplyAcc {
    message_id: String,
    blocks: Vec<ReplyBlock>,
    /// 是否已有块落在本次导入的新增字节里（增量续读的写库门槛）
    has_new_block: bool,
    version: Option<String>,
    /// 回复开始时挂接的未归属 api_error 行（上一条回复之后、本条之前的
    /// 重试，归给本条回复）
    pending_errors: Vec<ApiErrorHit>,
    /// 块之间夹有 user 工具结果行（审计统计用）
    tool_result_between: bool,
}

/// 回复内的一个块行（一行 = 一个 content 块）
#[derive(Debug, Clone)]
struct ReplyBlock {
    timestamp: Option<i64>,
    parent_uuid: Option<String>,
    model: Option<String>,
    requested_model: Option<String>,
    real_duration_ms: Option<f64>,
    stop_reason: Option<String>,
    usage: UsagePayload,
    api_block_index: Option<i64>,
    request_ref: Option<String>,
    thinking_duration_ms: Option<f64>,
    tool_input_stream_ms: Option<f64>,
    first_block_type: Option<String>,
    is_api_error: bool,
    api_error_status: Option<i64>,
}

impl ReplyBlock {
    fn computed_total(&self) -> i64 {
        self.usage.computed_total()
    }
    fn from_line(line: &TranscriptLine, msg: &AssistantMessage) -> ReplyBlock {
        ReplyBlock {
            timestamp: line.timestamp.as_deref().and_then(parse_ts_ms),
            parent_uuid: line.parent_uuid.clone(),
            model: msg.model.clone(),
            requested_model: line.requested_model(),
            real_duration_ms: msg.duration_ms.filter(|d| d.is_finite() && *d > 0.0),
            stop_reason: msg.stop_reason(),
            usage: msg.usage.clone().unwrap_or_default(),
            api_block_index: line.api_block_index(),
            request_ref: line.request_ref(),
            thinking_duration_ms: line.thinking_duration_ms(),
            tool_input_stream_ms: line.tool_input_stream_ms(),
            first_block_type: msg.first_block_type.clone(),
            is_api_error: line.is_api_error_message(),
            api_error_status: line.api_error_status(),
        }
    }
}

/// 父链节点：沿 uuid 向上找请求起点时的行证据
#[derive(Debug, Clone)]
struct ChainEntry {
    parent_uuid: Option<String>,
    timestamp: Option<i64>,
    line_type: Option<String>,
}

/// 未归属的 system/api_error 重试行（归给下一条回复）
#[derive(Debug, Clone)]
struct ApiErrorHit {
    status: Option<String>,
}

/// error.value → 错误码字符串（整数 429 → "429"；字符串原样；缺失 None）
fn api_error_status_string(error: Option<&serde_json::Value>) -> Option<String> {
    let status = error?.get("status")?;
    if let Some(code) = status.as_i64() {
        return Some(code.to_string());
    }
    status.as_str().map(str::to_string)
}

/// 首块的首个输出时刻证据：思考块用 thinkingDurationMs、工具调用或文字块
/// 用 toolInputStreamMs，均为「块落盘时刻 − 产出耗时」（毫秒），有则给出
/// 绝对时刻，否则 None（不做任何估算）。
fn first_output_evidence(block: &ReplyBlock) -> Option<i64> {
    let ts = block.timestamp?;
    let evidence_ms = match block.first_block_type.as_deref() {
        Some("thinking") => block.thinking_duration_ms,
        Some("text") => block.tool_input_stream_ms,
        Some(t) if t == "tool_use" || t.ends_with("_tool_use") => block.tool_input_stream_ms,
        _ => None,
    }?;
    if !evidence_ms.is_finite() || evidence_ms <= 0.0 {
        return None;
    }
    // 畸形超大值（毫秒数超出 i64 时间戳范围）不做回绕，按没有证据处理；
    // 首个输出时刻必须落在纪元之后且早于块落盘时刻
    ts.checked_sub(evidence_ms.round() as i64)
        .filter(|first| *first > 0 && *first < ts)
}

/// 请求起点解析（对整条回复）：
/// 1. 任一块的 requestRef 命中 api-request 索引 → 用那一行的时间戳，精确；
/// 2. 否则从首块 parentUuid 沿父链向上，跳过 attachment 行，取第一条其它行
///    的时间戳，标记为推断（timing_estimated = 1）；
/// 3. 都找不到 → 起点 / 估算标记均为空（派生计时一并置空，见 finalize）。
fn resolve_request_start(
    acc: &[ReplyBlock],
    first_block: &ReplyBlock,
    scanner: &ReplyScanner,
) -> (Option<i64>, Option<i64>) {
    for block in acc {
        if let Some(request_ref) = block.request_ref.as_deref().filter(|r| !r.is_empty()) {
            if let Some(&ts) = scanner.api_requests.get(request_ref) {
                return (Some(ts), Some(0));
            }
        }
    }
    let mut uuid = first_block.parent_uuid.clone();
    for _ in 0..PARENT_CHAIN_MAX_HOPS {
        let Some(current) = uuid.clone() else { break };
        let Some(entry) = scanner.parent_chain.get(&current) else { break };
        if entry.line_type.as_deref() == Some("attachment") {
            uuid = entry.parent_uuid.clone();
            continue;
        }
        // 第一条非 attachment 行的时间戳即请求起点（缺时间戳视为找不到）
        return (entry.timestamp, Some(1));
    }
    (None, None)
}

/// 行级扫描器：顺序喂入会话行，产出「回复级」取证记录。
/// 纯内存状态（父链 / api-request 索引 / 未归属 api_error / 最近真实模型名 /
/// cwd），无任何 IO；审计测试与本机回放共用同一套判定。
#[derive(Debug, Default)]
struct ReplyScanner {
    current: Option<ReplyAcc>,
    pending_api_errors: Vec<ApiErrorHit>,
    parent_chain: HashMap<String, ChainEntry>,
    parent_order: std::collections::VecDeque<String>,
    api_requests: HashMap<String, i64>,
    api_request_order: std::collections::VecDeque<String>,
    /// 最近一条 api-request 行的时间戳（重试时每次尝试写一行）。最终失败的
    /// 回复行上没有 requestRef（是另一格式的 requestId），起点取它——耗时
    /// 只描述产生这条回复的那一次请求，不含此前的重试等待
    last_api_request_at: Option<i64>,
    last_real_model: Option<String>,
    current_cwd: Option<String>,
    current_project: Option<String>,
}

impl ReplyScanner {
    fn new() -> ReplyScanner {
        ReplyScanner::default()
    }

    /// 从已入库行恢复 cwd / project（续读窗口内可能没有 cwd 行）
    fn restore_project(&mut self, cwd: Option<String>, project: Option<String>) {
        if self.current_cwd.is_none() {
            self.current_cwd = cwd;
        }
        if self.current_project.is_none() {
            self.current_project = project;
        }
    }

    /// 喂入一行。行导致当前回复结束时返回该回复的取证记录（fresh = 行来自
    /// 本次导入的新增字节；回看区的行传 false，产出的记录由调用方丢弃）。
    fn feed(&mut self, line: &TranscriptLine, fresh: bool) -> Option<ReplyRecord> {
        let line_type = line.line_type.as_deref();
        let is_assistant = line_type == Some("assistant");
        let message_id = if is_assistant {
            line.message
                .as_ref()
                .and_then(|m| m.id.as_deref())
                .filter(|id| !id.is_empty() && !id.contains('|'))
                .map(str::to_string)
        } else {
            None
        };
        // user 行的 content[0].type == tool_result = 工具结果行（客户端一边
        // 收流一边执行工具产生，不结束当前回复）；content 是字符串 / 缺失 /
        // 首块类型是别的 = 真正的用户输入，结束当前回复
        let is_user_tool_result = line_type == Some("user")
            && line.message.as_ref().and_then(|m| m.first_block_type.as_deref())
                == Some("tool_result");

        // 结束当前回复的四种情况（见模块级注释）：id 不同 / 无 id 的 assistant
        // 行、api-request 行（新请求开始）、真用户输入行、文件读尽（finish）。
        // 其它行（工具结果、attachment、system 等）不结束，上下文照常更新
        let mut finished = None;
        let ends_current = (is_assistant
            && self
                .current
                .as_ref()
                .is_some_and(|acc| message_id.as_deref() != Some(acc.message_id.as_str())))
            || line_type == Some("api-request")
            || (line_type == Some("user") && !is_user_tool_result);
        if ends_current {
            // 结束原因是「后面还有别的行」（feed 内恒 true；finish 内为 false）
            finished = self.finalize_current(true);
        }

        // 上下文刷新：cwd（任何行）、父链（有 uuid 的行）、api-request 索引、
        // 未归属 api_error、最近真实模型名
        if let Some(cwd) = line
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            let cwd = cwd.to_string();
            if let Some(project) = crate::projects::normalize_cwd(&cwd) {
                self.current_project = Some(project);
            }
            self.current_cwd = Some(cwd);
        }
        if let Some(uuid) = line.uuid.as_deref().filter(|u| !u.is_empty()) {
            if !self.parent_chain.contains_key(uuid) {
                self.parent_order.push_back(uuid.to_string());
                if self.parent_order.len() > PARENT_CHAIN_KEEP {
                    if let Some(oldest) = self.parent_order.pop_front() {
                        self.parent_chain.remove(&oldest);
                    }
                }
            }
            self.parent_chain.insert(
                uuid.to_string(),
                ChainEntry {
                    parent_uuid: line.parent_uuid.clone(),
                    timestamp: line.timestamp.as_deref().and_then(parse_ts_ms),
                    line_type: line.line_type.clone(),
                },
            );
        }
        match line_type {
            Some("api-request") => {
                if let (Some(id), Some(ts)) = (
                    line.id.as_deref().filter(|id| !id.is_empty()),
                    line.timestamp.as_deref().and_then(parse_ts_ms),
                ) {
                    // 重试时同一 id 可能写多行：覆盖为最近一条的时间戳
                    if !self.api_requests.contains_key(id) {
                        self.api_request_order.push_back(id.to_string());
                        if self.api_request_order.len() > API_REQUEST_KEEP {
                            if let Some(oldest) = self.api_request_order.pop_front() {
                                self.api_requests.remove(&oldest);
                            }
                        }
                    }
                    self.api_requests.insert(id.to_string(), ts);
                    // 错误回复的起点证据（最近一次尝试的发起时刻）
                    self.last_api_request_at = Some(ts);
                }
            }
            Some("system") if line.subtype.as_deref() == Some("api_error") => {
                self.pending_api_errors.push(ApiErrorHit {
                    status: api_error_status_string(line.error.as_ref()),
                });
            }
            _ => {}
        }
        // 工具结果行夹在当前回复的块之间：标记（审计统计用），不结束回复
        if is_user_tool_result {
            if let Some(acc) = self.current.as_mut() {
                acc.tool_result_between = true;
            }
        }
        // 真正的用户输入（非工具结果）：旧版本日志没有失败块，最终失败的
        // 请求只留下 api_error 行，用户重新输入后下一条回复与那次失败无关
        // ——未归属的重试行到此为止，不再归给后面的回复；带失败块的新版本
        // 日志不受影响（失败块在开始时照常消耗挂接）
        if line_type == Some("user") && !is_user_tool_result {
            self.pending_api_errors.clear();
        }

        if is_assistant {
            if let Some(msg) = line.message.as_ref() {
                if let Some(model) = msg.model.as_deref() {
                    if !model.is_empty() && model != "<synthetic>" {
                        self.last_real_model = Some(model.to_string());
                    }
                }
                // 无 message.id 的旧格式行不进回复分组（保持逐行写入路径）
                if let Some(message_id) = message_id {
                    let block = ReplyBlock::from_line(line, msg);
                    match self.current.as_mut() {
                        Some(acc) if acc.message_id == message_id => {
                            if fresh {
                                acc.has_new_block = true;
                            }
                            acc.blocks.push(block);
                        }
                        _ => {
                            // 新回复开始：把未归属的 api_error 挂接到它
                            //（上一条回复之后、本条之前的重试归给本条）
                            self.current = Some(ReplyAcc {
                                message_id,
                                blocks: vec![block],
                                has_new_block: fresh,
                                version: line.version.clone(),
                                pending_errors: std::mem::take(&mut self.pending_api_errors),
                                tool_result_between: false,
                            });
                        }
                    }
                }
            }
        }
        finished
    }

    /// 文件读尽：把仍在累积的回复落成记录（之后没有别的行 → 结束原因里
    /// 「本趟还读到了别的行」不成立，status 只看末块 stop_reason）。
    fn finish(&mut self) -> Option<ReplyRecord> {
        self.finalize_current(false)
    }

    /// 结束当前累积中的回复并产出取证记录（挂接在该回复上的 api_error 归入
    /// 重试）。没有任何块用量大于 0、也不是错误回复的回复（流式占位）整体
    /// 跳过，其挂接的 api_error 还回暂存，仍归给下一条真回复。
    fn finalize_current(&mut self, saw_other_line: bool) -> Option<ReplyRecord> {
        let mut acc = self.current.take()?;
        let cwd = self.current_cwd.clone();
        let project_key = self.current_project.clone();
        let record = self.finalize_acc(&mut acc, cwd, project_key, saw_other_line);
        if record.is_none() {
            // 占位回复被跳过：挂接的重试行还回暂存，归给下一条真回复
            self.pending_api_errors = std::mem::take(&mut acc.pending_errors);
        }
        record
    }

    /// 取证本体（纯计算）：用量块 / 首块 / 结束与首个输出时刻 / 请求起点 /
    /// 状态 / 重试 → 派生值与窗口校验。
    fn finalize_acc(
        &self,
        acc: &mut ReplyAcc,
        cwd: Option<String>,
        project_key: Option<String>,
        saw_other_line: bool,
    ) -> Option<ReplyRecord> {
        // 用量块：computed_total 最大的那个（并列取先出现——与旧逐行路径
        // 「总量更大者胜」的结果一致）
        let mut usage_index: Option<usize> = None;
        for (index, block) in acc.blocks.iter().enumerate() {
            if block.computed_total() <= 0 {
                continue;
            }
            let better = match usage_index {
                None => true,
                Some(best) => block.computed_total() > acc.blocks[best].computed_total(),
            };
            if better {
                usage_index = Some(index);
            }
        }
        let error_block = acc.blocks.iter().find(|b| b.is_api_error);
        let usage_block = usage_index.map(|index| &acc.blocks[index]);
        if usage_block.is_none() && error_block.is_none() {
            return None; // 流式占位
        }
        let is_error = error_block.is_some();
        let anchor = usage_block.or(error_block)?;
        // 没有时间戳无法归入统计区间，跳过（与旧逐行路径一致）
        let started_at = anchor.timestamp?;
        let completed_at = acc.blocks.iter().filter_map(|b| b.timestamp).max();

        // 重试行在本回复开始时挂接（上一条回复之后、本条之前出现的
        // api_error），条数进 retry_count、最后一条的状态码进 error_code
        let retry_count = acc.pending_errors.len() as i64;
        let retry_error_code = acc.pending_errors.last().and_then(|hit| hit.status.clone());

        // 分支取证：错误回复（token 全 0，错误码 / 模型名按失败块）与成功回复
        //（用量块取值；重试码取最近一次 api_error 的 status——自己的
        // apiErrorStatus 只属于失败回复）
        let (model_id, status, error_code, usage, real_duration_ms) = if is_error {
            let error_block = error_block?;
            let model = error_block
                .requested_model
                .clone()
                .filter(|m| !m.is_empty())
                .or_else(|| self.last_real_model.clone())
                .unwrap_or_default();
            let error_code = Some(
                error_block
                    .api_error_status
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "error".to_string()),
            );
            (model, Some("error"), error_code, UsagePayload::default(), error_block.real_duration_ms)
        } else {
            let usage_block = usage_block?;
            let model = usage_block
                .model
                .as_deref()
                .filter(|m| !m.is_empty() && *m != "<synthetic>")
                .map(str::to_string)
                .or_else(|| self.last_real_model.clone())
                .unwrap_or_default();
            // 末块有 stop_reason，或本趟在回复之后还读到了别的行 → completed；
            // 文件恰好停在流式回复中间 → NULL（下次续读补全）
            let last_block = acc.blocks.last()?;
            let status = if last_block.stop_reason.is_some() || saw_other_line {
                Some("completed")
            } else {
                None
            };
            (model, status, retry_error_code, usage_block.usage.clone(), usage_block.real_duration_ms)
        };

        // 首块：apiBlockIndex == 0 的块；没有该字段取最先出现的块
        let first_block = match acc
            .blocks
            .iter()
            .find(|b| b.api_block_index == Some(0))
            .or_else(|| acc.blocks.first())
        {
            Some(block) => block,
            None => return None,
        };

        // 请求起点：成功回复按 requestRef / 父链解析；最终失败的回复行上没有
        // requestRef（是另一格式的 requestId），起点取最近一条 api-request 行
        //（重试时是最后一次尝试的发起时刻，耗时不含此前的重试等待）；旧版本
        // 没有 api-request 行则起点为空，错误回复不走父链回退
        let (mut request_started_at, mut timing_estimated) =
            resolve_request_start(&acc.blocks, first_block, self);
        if is_error {
            (request_started_at, timing_estimated) = match self.last_api_request_at {
                Some(ts) => (Some(ts), Some(0)),
                None => (None, None),
            };
        }

        // 首个输出时刻照常取证；流式速度只需要它与 completed_at，不依赖请求
        // 起点，起点缺失不丢它。与起点的时序校验只在起点存在时做（不满足的
        // 那一项置空，不做钳制）
        let mut first_token_at = first_output_evidence(first_block);
        if let Some(start) = request_started_at {
            if first_token_at.is_some_and(|ft| ft <= start) {
                first_token_at = None;
            }
            if let Some(done) = completed_at {
                if first_token_at.is_some_and(|ft| ft > done) {
                    first_token_at = None;
                }
            }
        }
        // ttft = first_token_at − request_started_at（首字必须落在起点与结束之间；
        // completed_at 缺失时只要求大于起点；起点缺失时不给）
        let time_to_first_token_ms = match (request_started_at, first_token_at) {
            (Some(start), Some(first))
                if start < first && completed_at.map_or(true, |c| first <= c) =>
            {
                Some((first - start) as f64)
            }
            _ => None,
        };
        // 耗时派生：无真实 durationMs 时 = completed_at − request_started_at，
        // 两者都有且差值在合法窗口内才写
        let derived_duration_ms = match (request_started_at, completed_at) {
            (Some(start), Some(done)) => {
                let span = done - start;
                let valid = span >= crate::token_speed::MIN_GEN_MS
                    && span <= crate::token_speed::MAX_GEN_MS;
                valid.then_some(span as f64)
            }
            _ => None,
        };

        Some(ReplyRecord {
            message_id: acc.message_id.clone(),
            started_at,
            model_id,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            cache_creation_tokens: usage.cache_creation_input_tokens,
            computed_total: usage.computed_total(),
            real_duration_ms,
            derived_duration_ms,
            request_started_at,
            first_token_at,
            completed_at,
            time_to_first_token_ms,
            timing_estimated: if request_started_at.is_some() { timing_estimated } else { None },
            status,
            error_code,
            retry_count,
            has_new_block: acc.has_new_block,
            cwd,
            project_key,
            version: acc.version.clone(),
            tool_result_between: acc.tool_result_between,
        })
    }
}

// ===== 增量导入 =====

/// 导入互斥锁：面板查询 / 托盘标题刷新 / 同步上传可能并发触发导入，
/// 串行化避免同一文件被双份解析（唯一键可去重，但重复 IO 浪费）。
static IMPORT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn import_lock() -> &'static Mutex<()> {
    IMPORT_LOCK.get_or_init(|| Mutex::new(()))
}

/// 上次导入时间（节流用）。各查询入口都会触发导入，前端 30s 一轮 × 多个
/// 命令会放大扫描次数；会话文件分钟级追加，5 秒节流足够实时。
static LAST_IMPORT_AT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();

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

/// 递归收集 projects 目录下所有会话 jsonl。
/// 层级为 <项目目录>/<uuid>.jsonl 与 <项目目录>/<uuid>/subagents/<uuid>.jsonl，
/// 防御性下钻 5 层。结果排序保证导入顺序稳定。
fn collect_session_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_session_files(&path, depth - 1, out);
            }
        } else {
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(".jsonl") {
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
            .transaction()
            .map_err(|e| format!("开启导入事务失败: {e}"))?;
        if let Err(e) = import(&mut tx, path) {
            // 丢弃事务（循环体末尾 drop）→ 本文件已写行整体回滚
            eprintln!(
                "[agenthud-claude] {fail_log_prefix} {} 失败（已回滚，下次重试）: {e}",
                path.display()
            );
            continue;
        }
        if let Err(e) = tx.commit() {
            eprintln!(
                "[agenthud-claude] 提交导入事务失败（{}，下次重试）: {e}",
                path.display()
            );
        }
    }
    Ok(())
}

/// 增量导入（不节流）：遍历 projects 目录，把每个会话文件新增部分解析入库。
/// - file_progress 记录"已处理到的字节偏移"（对齐完整行末尾）；文件变短
///   （被重写）时从头重新解析，UNIQUE 键保证幂等。
/// - 同一 message.id 的后续行（续传/重写场景）靠 ON CONFLICT 覆盖旧值。
/// - 每个文件一个事务：中途崩溃/失败整体回滚，下次从旧偏移重来
///   （run_import_files）。
/// - 单文件失败只记日志跳过，不阻断其他文件。
pub fn import_incremental_force() -> Result<(), String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = projects_dir()?;
    let mut conn = open_claude_db()?;

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
    collect_session_files(&dir, 5, &mut files);
    files.sort();

    // 计时 / 状态一次性回填（PRAGMA user_version 标记）：标记 < 2 时，对
    // 最近 90 天修改过的文件无视已有偏移从头重放（已存在的行原地补计时，
    // 重放幂等）。跑完才写标记：个别文件失败照常记日志跳过、标记照写；
    // 中途退出则标记没写、下次重来。**不删 file_progress 行**——正在运行的
    // 旧版实例会抢先重建进度，导致回填漏文件。
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

    // 存量行项目维度回填（节流 + 每批限量，幂等说明见函数注释）
    if let Err(e) = backfill_project_keys_locked(&mut conn) {
        eprintln!("[agenthud-claude] 项目维度回填失败（下次重试）: {e}");
    }
    Ok(())
}

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
        eprintln!("[agenthud-claude] 写入回填标记失败（下次导入会重放一遍）: {e}");
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

/// 上次项目维度回填时间（节流用）。
static LAST_PROJECT_BACKFILL_AT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();

/// 读取会话文件前 N 行，找首个非空 cwd（旧行无行内 cwd 时回填用）。
fn read_first_cwd(path: &Path, max_lines: usize) -> Option<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    for line in std::io::BufReader::new(file).lines().take(max_lines).flatten() {
        if let Ok(line) = serde_json::from_str::<TranscriptLine>(&line) {
            if let Some(cwd) = line
                .cwd
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// 存量行项目维度回填（调用方必须已持有 import_lock）。
///
/// 幂等性保证（重复调用不重复读文件）：
/// - 只选 project_key 仍为 NULL 的会话（升级前的旧行 / 行内无 cwd 的新行），
///   回填后（含找不到 cwd 而标成 UNKNOWN 哨兵的）不再进入候选集；
/// - 每个候选会话只读一次源文件的前 20 行；找不到 cwd 时把该会话全部
///   NULL 行直接置为 UNKNOWN 哨兵，下一轮候选查询即排除，不会重复 IO；
/// - UPDATE 带 `AND project_key IS NULL` 条件，与增量导入写入的值不冲突；
/// - 30 秒节流 + 每批最多 500 个会话，量大时靠多轮调用分批消化。
fn backfill_project_keys_locked(conn: &mut Connection) -> Result<usize, String> {
    {
        let mut last = LAST_PROJECT_BACKFILL_AT
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
            .prepare("SELECT DISTINCT session_id FROM model_usage WHERE project_key IS NULL LIMIT 500")
            .map_err(|e| format!("查询待回填 Claude 会话失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询待回填 Claude 会话失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("查询待回填 Claude 会话失败: {e}"))?;
        rows
    };
    if missing.is_empty() {
        return Ok(0);
    }

    // session_id → 源文件映射（文件名主干即会话 uuid；目录已消失/文件被删
    // 的会话直接落 UNKNOWN 哨兵）
    let dir = projects_dir_path();
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_session_files(&dir, 5, &mut files);
    }
    let mut by_session: HashMap<String, &Path> = HashMap::new();
    for path in &files {
        by_session
            .entry(session_id_from_filename(path))
            .or_insert(path);
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启 Claude 回填事务失败: {e}"))?;
    for session_id in &missing {
        let cwd = by_session
            .get(session_id.as_str())
            .and_then(|path| read_first_cwd(path, 20));
        match cwd.and_then(|c| {
            crate::projects::normalize_cwd(&c)
                .map(|key| (c, key))
        }) {
            Some((raw, key)) => {
                tx.execute(
                    "UPDATE model_usage SET cwd = ?1, project_key = ?2
                     WHERE session_id = ?3 AND project_key IS NULL",
                    rusqlite::params![raw, key, session_id],
                )
                .map_err(|e| format!("回填 Claude 项目维度失败: {e}"))?;
            }
            None => {
                // 找不到 cwd（文件被删/前 20 行无 cwd）：置哨兵防止重复扫描
                tx.execute(
                    "UPDATE model_usage SET project_key = ?1
                     WHERE session_id = ?2 AND project_key IS NULL",
                    rusqlite::params![crate::projects::UNKNOWN_PROJECT, session_id],
                )
                .map_err(|e| format!("回填 Claude 项目维度哨兵失败: {e}"))?;
            }
        }
    }
    tx.commit()
        .map_err(|e| format!("提交 Claude 回填事务失败: {e}"))?;
    Ok(missing.len())
}

/// 存量行项目维度回填（公开入口，项目浏览器查询前调用）。
/// 与增量导入共用 IMPORT_LOCK 串行；内部自带节流与分批限量。
pub fn backfill_project_keys() -> Result<usize, String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut conn = open_claude_db()?;
    backfill_project_keys_locked(&mut conn)
}

/// 解析单个会话文件的增量部分。known = 上次记录的 (offset, size)。
/// 事务由调用方的 run_import_files 按文件提供（每文件一个，失败整体
/// 回滚，见 import_incremental_force）；行级 INSERT 在循环前 prepare 一次
/// 复用，避免每行重复解析 SQL。
///
/// 有 message.id 的行按「回复」取证写入（ReplyScanner：计时 / 状态 / 失败行 /
/// 重试）；无 message.id 的旧格式行保持原逐行写入路径（序号键、无计时）。
/// 续读（start_offset > 0）时先回看游标前最多 LOOKBACK_BYTES 字节重建上下文
///（父链 / api-request 索引 / 未归属 api_error / 跨游标的未结束回复），
/// 这段不写库；只有至少有一个块落在新增字节里的回复才落库。
fn import_one_file(
    tx: &Transaction<'_>,
    path: &Path,
    known: Option<(u64, u64)>,
) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("读取文件元信息失败: {e}"))?
        .len();

    // 无进度记录，或文件比记录时更短（被重写）→ 从头解析（唯一键幂等）
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

    // 无 message.id 的行（旧版本 CLI）退回"会话内行序号"做去重键：
    // - 续读（文件增长）：从该会话已用的最大序号继续，避免 seq:N 与历史行
    //   撞键错乱合并（撞键后"总量更大者胜"会把两条不同请求折叠成一条）；
    // - 重解析（文件变短被重写）：从 0 重计，重放行与旧行撞相同键，
    //   靠"总量更大者胜"幂等去重（与 codex.rs 的 seq 语义一致）。
    let mut line_seq: i64 = 0;
    // 回复级取证的行级扫描器（cwd / 父链 / api-request 索引都在其内部）
    let mut scanner = ReplyScanner::new();
    if !reparse {
        let prefix = format!("{session_id}|");
        let keys: Vec<String> = {
            let mut stmt = match tx.prepare(
                "SELECT dedupe_key FROM model_usage WHERE session_id = ?1 AND dedupe_key LIKE ?2",
            ) {
                Ok(s) => s,
                Err(_) => return Err("准备序号恢复查询失败".into()),
            };
            let like = format!("{prefix}%");
            // 先绑定再 match：让 MappedRows 临时值在 stmt 之前析构
            let rows = stmt.query_map(rusqlite::params![session_id, like], |row| {
                row.get::<_, String>(0)
            });
            match rows {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            }
        };
        line_seq = keys
            .iter()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter_map(|s| s.parse::<i64>().ok())
            .max()
            .unwrap_or(0);
        // 项目上下文恢复：续读窗口内可能没有 cwd 行（在上次已读部分），从该会话
        // 已入库行恢复，避免新增行 project_key 退化为 NULL；重解析路径从文件头
        // 重读，cwd 行自然重建上下文。
        let _ = tx.query_row(
            "SELECT cwd, project_key FROM model_usage
             WHERE session_id = ?1 AND project_key IS NOT NULL
             LIMIT 1",
            rusqlite::params![session_id],
            |row| {
                scanner.restore_project(row.get(0)?, row.get(1)?);
                Ok(())
            },
        );
    }
    // 修订时间：同一 message.id 的终值晚于中间值落盘，覆盖已入库行时打上
    // 修订标记（同步层据此把修正后的值补传到服务端）
    let now_ms = chrono::Local::now().timestamp_millis();

    // 行级 upsert 预编译一次，循环内仅绑定执行（prepare 保留不可变借用，
    // commit 由调用方在语句析构后进行）。仅服务无 message.id 的旧格式行，
    // 新格式回复由 write_reply_record 按回复合并写入。
    let mut upsert_usage = tx
        .prepare(
            "INSERT INTO model_usage
                (session_id, dedupe_key, started_at, model_id, provider_id,
                 input_tokens, output_tokens, cache_read_input_tokens,
                 cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                 duration_ms, updated_at, cwd, project_key)
             VALUES (?1, ?2, ?3, ?4, 'claude', ?5, ?6, ?7, ?8, 0, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(dedupe_key) DO UPDATE SET
                started_at = excluded.started_at,
                model_id = excluded.model_id,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_read_input_tokens = excluded.cache_read_input_tokens,
                cache_creation_input_tokens = excluded.cache_creation_input_tokens,
                computed_total_tokens = excluded.computed_total_tokens,
                duration_ms = excluded.duration_ms,
                updated_at = ?11
             WHERE excluded.computed_total_tokens > model_usage.computed_total_tokens",
        )
        .map_err(|e| format!("预编译用量写入语句失败: {e}"))?;

    // 回看区（续读才有）：只重建上下文（父链 / api-request 索引 / 未归属
    // api_error / 跨游标的未结束回复 / 最近真实模型名），不写库
    if !reparse && start_offset > 0 {
        feed_lookback(&mut reader, start_offset, &mut scanner)?;
    }
    reader
        .seek(SeekFrom::Start(start_offset))
        .map_err(|e| format!("定位读取偏移失败: {e}"))?;

    let mut pos = start_offset;
    let mut last_complete_end = start_offset;
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
        let Ok(line) = serde_json::from_slice::<TranscriptLine>(&buf) else {
            continue;
        };
        // 回复级处理：本行结束当前回复时立即落库（记录里的 cwd / 重试等
        // 快照都取自本行之前的状态）
        if let Some(record) = scanner.feed(&line, true) {
            if record.has_new_block {
                write_reply_record(tx, &session_id, &record, now_ms, reparse)?;
            }
        }
        // 无 message.id 的 assistant 行保持原逐行写入路径（序号键、无计时）
        if line.line_type.as_deref() == Some("assistant") {
            let Some(msg) = line.message.as_ref() else {
                continue;
            };
            let has_dedupe_id = msg
                .id
                .as_deref()
                .is_some_and(|id| !id.is_empty() && !id.contains('|'));
            if has_dedupe_id {
                continue; // 新格式行已进回复分组
            }
            // CLI 内部占位行（如中断提示）模型名为 <synthetic>，跳过
            let model = match msg.model.as_deref() {
                Some(m) if !m.is_empty() && m != "<synthetic>" => m.to_string(),
                _ => continue,
            };
            let Some(usage) = msg.usage.as_ref() else {
                continue;
            };
            let total = usage.computed_total();
            if total <= 0 {
                // 流式过程中的 0 值占位行 / 空调用，跳过（真正终值由后续行携带）
                continue;
            }
            // 没有时间戳无法归入统计区间，跳过（偏移仍推进）
            let Some(started_at) = line.timestamp.as_deref().and_then(parse_ts_ms) else {
                continue;
            };

            line_seq += 1;
            // 无 id 行仅会话内去重（序号键带 session 前缀，message.id 形如
            // msg_<hex>，不含 "|"，两类键天然不冲突）
            let dedupe_key = format!("{session_id}|{line_seq}");

            // 同一序号键的多行 usage 为累计口径（末行即终值）：后写覆盖先写；
            // 仅当新行总量更大时覆盖，防御末行意外回退为小值的脏数据。
            // 覆盖不改变自增 id（上传 rowid 稳定），但打上 updated_at 修订标记。
            // 项目维度列不参与覆盖更新（fork 复制历史时行归属首个项目，保持稳定）。
            upsert_usage
                .execute(rusqlite::params![
                    session_id,
                    dedupe_key,
                    started_at,
                    model,
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_read_input_tokens,
                    usage.cache_creation_input_tokens,
                    total,
                    msg.duration_ms,
                    now_ms,
                    &scanner.current_cwd,
                    &scanner.current_project,
                ])
                .map_err(|e| format!("写入用量记录失败: {e}"))?;
        }
    }
    // EOF：仍在累积的回复（文件停在流式回复中间）也落库（status 按 NULL）
    if let Some(record) = scanner.finish() {
        if record.has_new_block {
            write_reply_record(tx, &session_id, &record, now_ms, reparse)?;
        }
    }
    drop(upsert_usage);

    // 进度：对齐到最后一条完整行末尾（末尾半行下次重读，靠唯一键幂等）。
    // 与本文件数据行同事务提交，中途失败一起回滚（断点语义不变）。
    let key = path.to_string_lossy().to_string();
    tx.execute(
        "INSERT INTO file_progress (path, offset, size) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET offset = ?2, size = ?3",
        rusqlite::params![key, last_complete_end as i64, size as i64],
    )
    .map_err(|e| format!("记录导入进度失败: {e}"))?;

    Ok(())
}

/// 续读回看：从游标前最多 LOOKBACK_BYTES 字节读到游标，只喂行重建 scanner
/// 上下文（起点在行中间时丢掉开头那条不完整的行；跨越游标的行防御性整行
/// 丢弃）。产出的回复记录一律丢弃——回看区不写库，写库只认新增字节里的块。
fn feed_lookback(
    reader: &mut std::io::BufReader<&mut std::fs::File>,
    start_offset: u64,
    scanner: &mut ReplyScanner,
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
        if let Ok(line) = serde_json::from_slice::<TranscriptLine>(&buf) {
            scanner.feed(&line, false);
        }
    }
    Ok(())
}

/// 按回复落库：新回复直接 INSERT；已存在（dedupe_key 相同）的回复由纯函数
/// merge_reply 合并，内容确实变了才 UPDATE（updated_at 置为本趟时间），
/// 重放无变化时不写——否则同步的修订通道会反复补传。id / cwd / project_key
/// 永不因合并而改变（fork 复制历史时行归属首个项目，保持稳定）。
/// `authoritative` = 从头解析（新文件 / 文件被重写 / 90 天回填重放），整文件
/// 证据完整，计时状态列新值非空即覆盖（可纠正被更早版本代码写错的值）。
fn write_reply_record(
    tx: &Transaction<'_>,
    session_id: &str,
    record: &ReplyRecord,
    now_ms: i64,
    authoritative: bool,
) -> Result<(), String> {
    // 失败回复取不到模型名（行上没有 requestedModel、本文件也没有真实模型
    // 名可回退）时，按项目 → 全库的顺序取最近一条真实模型名，避免空名条目
    // 进清单 / 用量分组 / 同步
    let mut record = record.clone();
    if record.model_id.is_empty() {
        if let Some(model) = resolve_fallback_model(tx, record.project_key.as_deref()) {
            record.model_id = model;
        }
    }
    let fresh = record.as_stored();
    match load_stored_reply(tx, &record.message_id)? {
        None => insert_reply(tx, session_id, &record, &fresh, now_ms),
        Some(stored) => {
            let merged = merge_reply(&stored, &fresh, authoritative);
            if merged == stored {
                return Ok(());
            }
            update_reply(tx, &record.message_id, &merged, now_ms)
        }
    }
}

/// 失败回复的模型名库内回退：依次取本项目（project_key 相同）、全库的最近
/// 一条真实模型名（非空且非 `<synthetic>`；入库的 model_id 已在解析层过滤
/// `<synthetic>`，这里防御性再过滤一次）。都取不到返回 None，保留空串。
fn resolve_fallback_model(
    tx: &Transaction<'_>,
    project_key: Option<&str>,
) -> Option<String> {
    if let Some(project) = project_key {
        let project_model: Option<String> = tx
            .query_row(
                "SELECT model_id FROM model_usage
                 WHERE project_key = ?1 AND model_id != '' AND model_id != '<synthetic>'
                 ORDER BY started_at DESC, id DESC LIMIT 1",
                [project],
                |row| row.get(0),
            )
            .ok();
        if project_model.is_some() {
            return project_model;
        }
    }
    tx.query_row(
        "SELECT model_id FROM model_usage
         WHERE model_id != '' AND model_id != '<synthetic>'
         ORDER BY started_at DESC, id DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .ok()
}

/// 已入库回复的行快照（SELECT 列序与读取下标严格一致）
fn load_stored_reply(
    tx: &Transaction<'_>,
    dedupe_key: &str,
) -> Result<Option<StoredReply>, String> {
    let mut stmt = tx
        .prepare(
            "SELECT started_at, model_id, input_tokens, output_tokens,
                    cache_read_input_tokens, cache_creation_input_tokens,
                    computed_total_tokens, duration_ms,
                    request_started_at, first_token_at, completed_at,
                    time_to_first_token_ms, timing_estimated, status, error_code, retry_count
             FROM model_usage WHERE dedupe_key = ?1",
        )
        .map_err(|e| format!("准备回复查询语句失败: {e}"))?;
    let mut rows = stmt
        .query_map([dedupe_key], |row| {
            Ok(StoredReply {
                started_at: row.get(0)?,
                model_id: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cache_read_tokens: row.get(4)?,
                cache_creation_tokens: row.get(5)?,
                computed_total: row.get(6)?,
                duration_ms: row.get(7)?,
                request_started_at: row.get(8)?,
                first_token_at: row.get(9)?,
                completed_at: row.get(10)?,
                time_to_first_token_ms: row.get(11)?,
                timing_estimated: row.get(12)?,
                status: row.get(13)?,
                error_code: row.get(14)?,
                retry_count: row.get(15)?,
            })
        })
        .map_err(|e| format!("查询已入库回复失败: {e}"))?;
    rows.next()
        .transpose()
        .map_err(|e| format!("读取已入库回复失败: {e}"))
}

/// 新回复 INSERT（自增 id 分配一次后永不改变——上传 rowid 稳定的前提）
fn insert_reply(
    tx: &Transaction<'_>,
    session_id: &str,
    record: &ReplyRecord,
    fresh: &StoredReply,
    now_ms: i64,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO model_usage
            (session_id, dedupe_key, started_at, model_id, provider_id,
             input_tokens, output_tokens, cache_read_input_tokens,
             cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
             duration_ms, updated_at, cwd, project_key,
             request_started_at, first_token_at, completed_at,
             time_to_first_token_ms, timing_estimated, status, error_code, retry_count)
         VALUES (?1, ?2, ?3, ?4, 'claude', ?5, ?6, ?7, ?8, 0, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
        rusqlite::params![
            session_id,
            record.message_id,
            fresh.started_at,
            fresh.model_id,
            fresh.input_tokens,
            fresh.output_tokens,
            fresh.cache_read_tokens,
            fresh.cache_creation_tokens,
            fresh.computed_total,
            fresh.duration_ms,
            now_ms,
            record.cwd,
            record.project_key,
            fresh.request_started_at,
            fresh.first_token_at,
            fresh.completed_at,
            fresh.time_to_first_token_ms,
            fresh.timing_estimated,
            fresh.status,
            fresh.error_code,
            fresh.retry_count,
        ],
    )
    .map_err(|e| format!("写入回复用量记录失败: {e}"))?;
    Ok(())
}

/// 已存在回复的合并 UPDATE（dedupe_key 定位，id / session / cwd / project_key 不动）
fn update_reply(
    tx: &Transaction<'_>,
    dedupe_key: &str,
    merged: &StoredReply,
    now_ms: i64,
) -> Result<(), String> {
    tx.execute(
        "UPDATE model_usage SET
            started_at = ?1, model_id = ?2,
            input_tokens = ?3, output_tokens = ?4,
            cache_read_input_tokens = ?5, cache_creation_input_tokens = ?6,
            computed_total_tokens = ?7, duration_ms = ?8,
            request_started_at = ?9, first_token_at = ?10, completed_at = ?11,
            time_to_first_token_ms = ?12, timing_estimated = ?13,
            status = ?14, error_code = ?15, retry_count = ?16,
            updated_at = ?17
         WHERE dedupe_key = ?18",
        rusqlite::params![
            merged.started_at,
            merged.model_id,
            merged.input_tokens,
            merged.output_tokens,
            merged.cache_read_tokens,
            merged.cache_creation_tokens,
            merged.computed_total,
            merged.duration_ms,
            merged.request_started_at,
            merged.first_token_at,
            merged.completed_at,
            merged.time_to_first_token_ms,
            merged.timing_estimated,
            merged.status,
            merged.error_code,
            merged.retry_count,
            now_ms,
            dedupe_key,
        ],
    )
    .map_err(|e| format!("合并更新回复用量记录失败: {e}"))?;
    Ok(())
}

// ===== 查询函数（与 codex.rs 同名同构，查 claude.sqlite；查询前先增量导入）=====

/// 查询 [from_ms, to_ms) 内的统计（口径与 db::query_stats 完全一致）。
/// 新版导入已按回复取证 time_to_first_token_ms（老库无该列时按 NULL 聚合）。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
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
        .map_err(|e| format!("查询 Claude 整体统计失败: {e}"))?;

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
        .map_err(|e| format!("准备 Claude 模型分组查询失败: {e}"))?;

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
        .map_err(|e| format!("读取 Claude 模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 模型分组失败: {e}"))?;

    // B3 速度质量标记：Claude 行的速度按行内真实 durationMs（新版 jsonl）
    // 或派生耗时（结束 − 起点）计算，包含首字等待，口径仍是**请求平均**
    // 近似——显式标 request_average（TS 侧以 ≈ 前缀区分），不与 ZCode 的
    // 可信生成速度混为同一种 t/s；无速度时不标（None）。
    if overall.speed.avg_tps.is_some() {
        overall.speed.speed_quality = Some(crate::token_speed::SpeedQuality::RequestAverage);
    }
    for model in &mut by_model {
        if model.speed.avg_tps.is_some() {
            model.speed.speed_quality = Some(crate::token_speed::SpeedQuality::RequestAverage);
        }
    }

    db::fill_duration_metrics(&conn, "claude", from_ms, to_ms, &mut overall, &mut by_model)?;

    let (earliest_ms, latest_ms): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(started_at), MAX(started_at) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("查询 Claude 时间范围失败: {e}"))?;

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
///   clamp）、↓ = Σ output、⟲ = Σ cache_read、× = 行数；另产会话级
///   请求平均速度原料（仅统计 duration_ms > 0 行的 Σoutput 与
///   Σduration，NULL/0/负耗时行分子分母一并排除）；
/// - 排序按最近活动倒序（MAX(started_at) DESC），只取前 `limit` 条；
/// - `total_active` 为窗口内 distinct 会话总数（含未展示的，供折叠
///   提示"还有 N 个"）。
///
/// 项目目录取该会话最近一行非空 cwd（model_usage 自带 cwd/project_key
/// 列，目录末段由 session_hud 侧解析）。查询前先增量导入（5 秒节流，
/// 未安装/失败静默返回 Err，由调用方跳过该组）。
pub fn query_window_sessions(
    window_start_ms: i64,
    limit: usize,
) -> Result<crate::session_hud::AgentWindowSessions, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
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
    let sql = "SELECT session_id,
            MAX(started_at),
            COALESCE(SUM(input_tokens + cache_creation_input_tokens), 0),
            COALESCE(SUM(output_tokens), 0),
            COALESCE(SUM(cache_read_input_tokens), 0),
            COUNT(*),
            GROUP_CONCAT(DISTINCT model_id),
            (SELECT m2.cwd FROM model_usage m2
              WHERE m2.session_id = model_usage.session_id
                AND m2.cwd IS NOT NULL AND m2.cwd != ''
              ORDER BY m2.started_at DESC LIMIT 1),
            -- 请求平均速度原料：仅统计 duration_ms > 0 的行，分子分母
            -- 同范围（NULL/0/负耗时行一并排除，防短耗时行放大均值）
            COALESCE(SUM(CASE WHEN duration_ms BETWEEN 200 AND 3600000 AND output_tokens > 0 THEN output_tokens ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN duration_ms BETWEEN 200 AND 3600000 AND output_tokens > 0 THEN duration_ms ELSE 0.0 END), 0.0)
         FROM model_usage
         WHERE session_id IN (
            SELECT session_id FROM model_usage
             WHERE started_at >= ?1 ORDER BY started_at DESC LIMIT ?2
         )
         GROUP BY session_id
         ORDER BY 2 DESC
         LIMIT ?3";
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("准备 Claude 会话窗口查询失败: {e}"))?;
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
                    // duration_ms 为 REAL，SUM 结果读 f64
                    speed_out_tokens: row.get(8)?,
                    speed_ms: row.get(9)?,
                    // claude 无生成中/TTFT/生成口径数据源（transcript 信号
                    // 仅 kimi 有），增强字段恒默认值
                    generating: false,
                    ttft_ms: None,
                    gen_out_tokens: 0,
                    gen_ms: 0.0,
                })
            },
        )
        .map_err(|e| format!("读取 Claude 会话窗口失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 会话窗口失败: {e}"))?;
    let total_active: usize = conn
        .query_row(
            "SELECT COUNT(DISTINCT session_id) FROM model_usage WHERE started_at >= ?1",
            [window_start_ms],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as usize)
        .map_err(|e| format!("统计 Claude 活跃会话数失败: {e}"))?;
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
    let conn = open_claude_db()?;
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
        .map_err(|e| format!("准备 Claude 趋势查询失败: {e}"))?;

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
            .map_err(|e| format!("读取 Claude 趋势统计失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 Claude 趋势统计失败: {e}"))?;

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

/// 查询 id > since 的明细记录（同步上传用）。source 固定 "claude"，local_rowid = id。
/// proto 5：附带会话 id 与项目维度（project_key / 原始 cwd 为行内列，
/// 未回填的旧行两字段为 None，服务端落 NULL）。
/// proto 8：附带耗时（本库 duration_ms 为 jsonl 的 durationMs 真实总耗时，
/// 无 TTFT 列 → 首字恒 None）。
/// proto 9：附带流式速度证据 speed_evidence（事件时刻齐全时）。
/// proto 10：附带状态扩展列（status / error_code / retry_count /
/// timing_estimated，按列存在性带出）。实现见 query_rows_since。
pub fn query_since(since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    query_rows_since(&conn, "id > ?1", &[since], 2, limit, "增量")
}

/// query_since / query_revised_since 共用的行查询内核（WHERE/占位符由调用方
/// 给：limit_idx = WHERE 参数个数 + 1）。proto 10 起状态扩展列追加在 SELECT
/// 末尾（下标 15~18），事件时刻两列（下标 19~20）仅供流式速度证据判定、不进
/// UsageRow——有事件证据的行同时携带 speed_evidence（proto 9 通道，见
/// token_speed::import_evidence），禁止用这两列反推相对量。
fn query_rows_since(
    conn: &Connection,
    where_clause: &str,
    params: &[i64],
    limit_idx: usize,
    limit: usize,
    kind: &str,
) -> Result<Vec<db::UsageRow>, String> {
    // proto 8：耗时/首字按列存在性带出（真实值 > 0 才给，无列恒 None；
    // 查询/同步层不做时间戳差相减，只转发导入时按事件时刻算好并带
    // timing_estimated 标记落库的列；口径与 db::query_since 一致）
    let (duration, ttft) = db::speed_columns(conn);
    // proto 10：状态扩展列同款按列存在性带出
    let (status, error_code, retry_count, timing_estimated) = db::timing_columns(conn);
    // 事件时刻（speed_evidence 判定用；无列恒 NULL → evidence 走 None 回退）
    let (first_col, completed_col) = event_columns(conn);
    let sql = format!(
        "SELECT id, started_at, model_id, provider_id,
                input_tokens, output_tokens, cache_read_input_tokens,
                cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                session_id, project_key, cwd, {duration}, {ttft},
                {status}, {error_code}, {retry_count}, {timing_estimated},
                {first_col}, {completed_col}
         FROM model_usage
         WHERE {where_clause}
         ORDER BY id ASC
         LIMIT ?{limit_idx}"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 Claude {kind}查询失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(
                params.iter().copied().chain(std::iter::once(limit as i64)),
            ),
            |row| {
                let output_tokens: i64 = row.get(5)?;
                let duration_ms: Option<f64> = row.get(13)?;
                let status_raw: Option<String> = row.get(15)?;
                let timing = crate::token_speed::ImportTiming {
                    output_tokens,
                    row_at: row.get(1)?,
                    first_token_at: row.get(19)?,
                    completed_at: row.get(20)?,
                    duration_ms,
                    fallback_quality: crate::token_speed::duration_quality("claude"),
                    observed_at_ms: 0,
                    completed: crate::token_speed::is_completed_status(status_raw.as_deref()),
                };
                Ok(db::UsageRow {
                    // proto 9：流式速度证据（无事件时刻 → None，远端按耗时回退）
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
                    source: "claude".into(),
                    session_id: row.get(10)?,
                    project_key: row.get(11)?,
                    project_display: row.get(12)?,
                    duration_ms,
                    time_to_first_token_ms: row.get(14)?,
                    // proto 10 状态扩展列（下标 15~18，与 SELECT 末尾严格对应）
                    status: row.get(15)?,
                    error_code: row.get::<_, Option<String>>(16)?.filter(|c| !c.is_empty()),
                    retry_count: row.get(17)?,
                    timing_estimated: row.get::<_, Option<i64>>(18)?.map(|v| v != 0),
                })
            },
        )
        .map_err(|e| format!("读取 Claude {kind}记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude {kind}记录失败: {e}"))?;
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

/// 导入库当前最大 rowid（供「待上传条数」显示用）。
pub fn max_rowid() -> Result<i64, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let max: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM model_usage", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("查询 Claude 最大 rowid 失败: {e}"))?;
    Ok(max)
}

/// 查询修订行（updated_at > since_ts 的记录，同步补传用）。
/// Claude 会话流式落盘：同一 message.id 的中间值可能先入库并被上传，
/// 终值稍后覆盖本地行（id 不变）——常规的 id > 游标查询永远选不出它，
/// 服务端会一直保留旧小值。本查询按修订时间选出这些行，上传后服务端
/// 以"总量更大者胜"的 upsert 覆盖修正（见 server/db.py）。
/// after_id 分页：按 id 升序、只取 id > after_id 的行，调用方逐批推进。
pub fn query_revised_since(
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    query_revised_since_at(&conn, since_ts, after_id, limit)
}

/// `query_revised_since` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_revised_since_at(
    conn: &Connection,
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<db::UsageRow>, String> {
    query_rows_since(
        conn,
        "updated_at > ?1 AND id > ?2",
        &[since_ts, after_id],
        3,
        limit,
        "修订",
    )
}

/// 列出导入库中出现过的所有 (provider_id, model_id) 组合，供价格配置用。
/// provider_id 恒为 "claude"。
pub fn list_models() -> Result<Vec<db::ModelInfo>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    list_models_at(&conn)
}

/// `list_models` 的纯查询内核（连接注入版，供单元测试）。
/// 忽略空模型名（历史失败行在模型名三级回退全部失败时会留空串，不进清单）。
pub(crate) fn list_models_at(conn: &Connection) -> Result<Vec<db::ModelInfo>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT provider_id, model_id
             FROM model_usage
             WHERE model_id != ''
             ORDER BY model_id",
        )
        .map_err(|e| format!("准备 Claude 模型列表查询失败: {e}"))?;

    let models = stmt
        .query_map([], |row| {
            Ok(db::ModelInfo {
                provider_id: row.get(0)?,
                model_id: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取 Claude 模型列表失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 模型列表失败: {e}"))?;
    Ok(models)
}

/// 按指定周期聚合 Claude Token。
/// 对比页需要真实的 [reset_at, end_at) 边界，不能用只带 HH:00 的趋势 label 反推跨日周期。
pub fn query_period_buckets(
    periods: &[(i64, i64)],
) -> Result<Vec<db::PeriodBucket>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT
                COALESCE(SUM(computed_total_tokens),0),
                COUNT(*)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2",
        )
        .map_err(|e| format!("准备 Claude 周期聚合查询失败: {e}"))?;

    let mut out = Vec::with_capacity(periods.len());
    for &(reset_at, end_at) in periods {
        let (total_tokens, requests): (i64, i64) = stmt
            .query_row(rusqlite::params![reset_at, end_at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("查询 Claude 周期聚合失败: {e}"))?;
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
/// project_key 为 NULL/空串（未回填或无 cwd）归入 unknown 哨兵键。
pub fn query_project_model_rows(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<crate::projects::ProjectModelRow>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(NULLIF(project_key, ''), ?1),
                    model_id,
                    COUNT(*),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_input_tokens),0),
                    COALESCE(SUM(computed_total_tokens),0)
             FROM model_usage
             WHERE started_at >= ?2 AND started_at < ?3
             GROUP BY 1, 2",
        )
        .map_err(|e| format!("准备 Claude 项目聚合查询失败: {e}"))?;
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
        .map_err(|e| format!("读取 Claude 项目聚合失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 项目聚合失败: {e}"))?;
    Ok(rows)
}

/// 按项目统计 [from_ms, to_ms) 内的会话数（COUNT(DISTINCT session_id)）。
pub fn query_project_session_counts(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<(String, i64)>, String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(NULLIF(project_key, ''), ?1), COUNT(DISTINCT session_id)
             FROM model_usage
             WHERE started_at >= ?2 AND started_at < ?3
             GROUP BY 1",
        )
        .map_err(|e| format!("准备 Claude 项目会话统计失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![crate::projects::UNKNOWN_PROJECT, from_ms, to_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("读取 Claude 项目会话统计失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 项目会话统计失败: {e}"))?;
    Ok(rows)
}

/// 项目键 → 原始形态 cwd（保留大小写，取字典序最小者做代表）。
pub fn query_project_display_paths() -> Result<Vec<(String, String)>, String> {
    let conn = open_claude_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT project_key, MIN(cwd)
             FROM model_usage
             WHERE project_key IS NOT NULL AND project_key != ?1
               AND cwd IS NOT NULL AND cwd != ''
             GROUP BY project_key",
        )
        .map_err(|e| format!("准备 Claude 项目路径查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![crate::projects::UNKNOWN_PROJECT], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("读取 Claude 项目路径失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 项目路径失败: {e}"))?;
    Ok(rows)
}

/// 分页查询指定项目的会话明细（按会话最后活跃时间降序）。
/// 返回 (匹配会话总数, 按会话 × 模型聚合的行)；offset/limit 作用在会话粒度。
/// project_key 传 UNKNOWN_PROJECT 时匹配 NULL/空串/哨兵的会话。
pub fn query_project_sessions(
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Result<(u32, Vec<crate::projects::ProjectSessionModelRow>), String> {
    import_incremental()?;
    let conn = open_claude_db()?;
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
                SELECT DISTINCT session_id
                FROM model_usage
                WHERE started_at >= ?1 AND started_at < ?2
                  AND COALESCE(NULLIF(project_key, ''), ?3) = ?4
            )",
            rusqlite::params![from_ms, to_ms, crate::projects::UNKNOWN_PROJECT, project_key],
            |row| row.get(0),
        )
        .map_err(|e| format!("查询 Claude 项目会话总数失败: {e}"))?;

    // 会话级速度聚合（口径与 query_stats 的面板统计完全一致，无 TTFT 列 →
    // ttft 恒 None；传 SUM/COUNT 供会话级跨模型合并，避免二次平均偏差）
    let speed = db::session_speed_agg_columns(
        db::has_column(&conn, "model_usage", "duration_ms"),
        false,
    );
    let mut stmt = conn
        .prepare(&format!(
            "SELECT session_id, model_id,
                    MIN(started_at), MAX(started_at), COUNT(*),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_input_tokens),0),
                    COALESCE(SUM(cache_creation_input_tokens),0)
                    {speed}
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2
               AND COALESCE(NULLIF(project_key, ''), ?3) = ?4
               AND session_id IN (
                   SELECT session_id FROM (
                       SELECT session_id
                       FROM model_usage
                       WHERE started_at >= ?1 AND started_at < ?2
                         AND COALESCE(NULLIF(project_key, ''), ?3) = ?4
                       GROUP BY session_id
                       ORDER BY MAX(started_at) DESC
                       LIMIT ?5 OFFSET ?6
                   )
               )
             GROUP BY session_id, model_id"
        ))
        .map_err(|e| format!("准备 Claude 项目会话查询失败: {e}"))?;
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
        .map_err(|e| format!("读取 Claude 项目会话失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Claude 项目会话失败: {e}"))?;
    Ok((total.max(0) as u32, rows))
}

// ===== 实时额度（Anthropic OAuth 端点，参照 CodexBar 的实现）=====

/// Claude 订阅额度（字段口径与 CodexRateLimits 一致，前端同款渲染）。
/// plan_type 来自凭据的订阅类型（pro/max 等；中转模式无凭据 → None）。
/// 模型专属周窗口 / 超额消费为增量字段：API 响应存在才有值（serde 序列化
/// 时 None 直接省略），旧响应与现有渲染完全不受影响。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeRateLimits {
    pub plan_type: Option<String>,
    pub primary_pct: Option<f64>,
    /// 5 小时会话窗口重置时间（毫秒时间戳）
    pub primary_reset_at: Option<i64>,
    pub secondary_pct: Option<f64>,
    /// 周窗口重置时间（毫秒时间戳）
    pub secondary_reset_at: Option<i64>,
    /// Sonnet 模型专属周窗口已用百分比（seven_day_sonnet / limits[].weekly_scoped）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sonnet_weekly_pct: Option<f64>,
    /// Sonnet 模型专属周窗口重置时间（毫秒时间戳）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sonnet_weekly_reset_at: Option<i64>,
    /// Opus 模型专属周窗口已用百分比（seven_day_opus / limits[].weekly_scoped）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opus_weekly_pct: Option<f64>,
    /// Opus 模型专属周窗口重置时间（毫秒时间戳）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opus_weekly_reset_at: Option<i64>,
    /// 超额消费已用金额（美元；月度 extra_usage，字段双兼容解析）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_used: Option<f64>,
    /// 超额消费上限金额（美元；缺省时前端只展示已用金额）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_limit: Option<f64>,
}

/// 实时额度失败分类（本地链路缓存与手动凭证链路共用）。授权类失败
/// （OAuth 条目缺失 / token 被拒）是用户可自助修复的问题，透出明确中文
/// 提示；网络类失败保持既有静默降级行为（额度块不展示即可）。
#[derive(Debug, Clone)]
pub enum ClaudeUsageFailure {
    /// 凭据中找不到 Claude OAuth 条目（如 Claude Code 2.1.x 钥匙串条目
    /// 仅含 mcpOAuth / 缺 accessToken）
    MissingOAuth(String),
    /// OAuth token 被服务端拒绝（HTTP 401/403）
    TokenRejected,
    /// 网络/服务/解析等其他失败（消息仅用于日志与诊断，不透出前端）
    Other(String),
}

/// OAuth 条目缺失（含 Claude Code 2.1.x 凭据仅含 mcpOAuth 的场景）的统一
/// 明确提示：区别于模糊网络错误，用户运行一次 claude 即可自助修复。
const MISSING_OAUTH_MESSAGE: &str =
    "未找到 Claude OAuth 凭证，请在终端运行一次 claude 完成登录授权";

/// token 被服务端拒绝（401/403）时的统一提示。
const TOKEN_REJECTED_MESSAGE: &str = "OAuth Token 已失效，请在终端重新运行 claude 授权";

impl ClaudeUsageFailure {
    /// 完整中文消息（日志 / 诊断 / 测试断言用）。
    pub fn message(&self) -> String {
        match self {
            ClaudeUsageFailure::MissingOAuth(m) => m.clone(),
            ClaudeUsageFailure::TokenRejected => TOKEN_REJECTED_MESSAGE.to_string(),
            ClaudeUsageFailure::Other(e) => e.clone(),
        }
    }

    /// 透出给前端的错误提示：仅授权类失败有明确提示（用户可自助修复），
    /// 网络类失败返回 None（保持静默降级，与既有行为一致）。
    pub fn user_message(&self) -> Option<String> {
        match self {
            ClaudeUsageFailure::Other(_) => None,
            _ => Some(self.message()),
        }
    }
}

/// Claude Code 登录凭证（只读，绝不修改/刷新——refresh_token 一次性轮换，
/// 外部写回极易搞坏 Claude Code 登录；token 过期由 Claude Code 自行刷新）。
#[derive(Debug)]
struct ClaudeAuth {
    access_token: String,
    /// 订阅类型徽标（subscriptionType 优先，rateLimitTier 兜底）
    plan_label: Option<String>,
}

/// 订阅套餐展示名归一（纯函数）：rateLimitTier 含乘数档位标识
/// （default_claude_max_5x / default_claude_max_20x 等）时归一为
/// "Max 5x"/"Max 20x"——乘数是用户最关心的信息，原始 tier 串过长不适合
/// 徽标展示；其余原样返回。
fn plan_display_name(label: &str) -> String {
    let lower = label.to_ascii_lowercase();
    if lower.contains("20x") {
        "Max 20x".to_string()
    } else if lower.contains("5x") {
        "Max 5x".to_string()
    } else {
        label.to_string()
    }
}

/// 解析 .credentials.json / Keychain 里的凭据 JSON。
/// 结构：{ "claudeAiOauth": { "accessToken", "refreshToken", "expiresAt"(毫秒),
/// "scopes", "rateLimitTier", "subscriptionType" } }（字段名为 camelCase）。
/// OAuth 条目缺失（含仅 mcpOAuth）/ 缺 accessToken 归 MissingOAuth 明确
/// 错误态；JSON 本身解析失败归 Other。
fn parse_credentials_json(data: &str) -> Result<ClaudeAuth, ClaudeUsageFailure> {
    let v: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| ClaudeUsageFailure::Other(format!("解析 Claude 凭据失败: {e}")))?;
    let oauth = v
        .get("claudeAiOauth")
        .ok_or_else(|| ClaudeUsageFailure::MissingOAuth(MISSING_OAUTH_MESSAGE.into()))?;
    let token = oauth
        .get("accessToken")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| ClaudeUsageFailure::MissingOAuth(MISSING_OAUTH_MESSAGE.into()))?;
    // 套餐名：rateLimitTier 含乘数档位（5x/20x）时优先归一为 "Max 5x"/"Max 20x"
    //（subscriptionType 通常是 pro/max，不携带乘数信息）；否则沿用既有
    // subscriptionType → rateLimitTier 优先级原样展示。
    let tier = oauth
        .get("rateLimitTier")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let sub_type = oauth
        .get("subscriptionType")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let plan_label = tier
        .filter(|t| {
            let lower = t.to_ascii_lowercase();
            lower.contains("5x") || lower.contains("20x")
        })
        .map(plan_display_name)
        .or_else(|| sub_type.map(|s| s.to_string()))
        .or_else(|| tier.map(|t| t.to_string()));
    Ok(ClaudeAuth {
        access_token: token.to_string(),
        plan_label,
    })
}

/// 读取 Claude Code 登录凭证：
/// 1. ZBAR_CLAUDE_HOME 环境变量或 ~/.claude 下的 .credentials.json
///    （Windows/Linux 的标准位置）；
/// 2. macOS 上凭据在登录钥匙串（服务名 "Claude Code-credentials"），经
///    `security find-generic-password -w` 读取（只读；若系统弹窗询问则取消，
///    读取失败按"无凭据"降级，不影响 token 统计）。
fn load_claude_auth() -> Result<ClaudeAuth, ClaudeUsageFailure> {
    let root = std::env::var("ZBAR_CLAUDE_HOME")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".claude")
        });
    let path = root.join(".credentials.json");
    if path.exists() {
        let data = std::fs::read_to_string(&path).map_err(|e| {
            ClaudeUsageFailure::Other(format!("读取 .credentials.json 失败: {e}"))
        })?;
        return parse_credentials_json(&data);
    }

    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                let data = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !data.is_empty() {
                    return parse_credentials_json(&data);
                }
            }
        }
    }

    Err(ClaudeUsageFailure::Other(
        "未找到 Claude 登录凭证（.credentials.json / 钥匙串），订阅额度不可用（token 统计不受影响）"
            .into(),
    ))
}

/// /api/oauth/usage 响应结构（窗口字段名与 Codex 的 wham/usage 不同：
/// 这里是 five_hour/seven_day + utilization 百分比(0-100) + resets_at ISO8601）。
/// 增量字段：seven_day_sonnet / seven_day_opus 模型专属周窗口、extra_usage
/// 月度超额消费、limits[] 补充窗口数组——均为 Option/默认值，旧响应不受影响。
#[derive(Debug, Deserialize)]
struct OAuthUsageResponse {
    #[serde(default)]
    five_hour: Option<OAuthWindow>,
    #[serde(default)]
    seven_day: Option<OAuthWindow>,
    /// Sonnet 模型专属周窗口（部分套餐返回；结构与 seven_day 相同）
    #[serde(default)]
    seven_day_sonnet: Option<OAuthWindow>,
    /// Opus 模型专属周窗口（结构与 seven_day 相同）
    #[serde(default)]
    seven_day_opus: Option<OAuthWindow>,
    /// 月度超额消费（金额结构；字段名做 used/limit 双兼容尝试，见
    /// parse_extra_usage——保留原始 Value 以兼容字段名差异）
    #[serde(default)]
    extra_usage: Option<serde_json::Value>,
    /// limits[] 补充窗口（带 weekly_scoped 标识的模型专属行，见
    /// apply_scoped_weekly_limit）
    #[serde(default)]
    limits: Vec<OAuthLimit>,
}

#[derive(Debug, Deserialize)]
struct OAuthWindow {
    /// 已用百分比（0-100，社区实测与 CodexBar 均按百分数直读）
    #[serde(default)]
    utilization: Option<f64>,
    /// 重置时间（ISO8601，如 2026-08-14T12:34:56Z）
    #[serde(default)]
    resets_at: Option<String>,
}

/// limits[] 条目（CodexBar 口径）：weekly_scoped=true 表示模型专属周窗口，
/// 进模型专属行；scope 含 "All models" 的通用行留在主周行，不进专属槽位。
#[derive(Debug, Deserialize)]
struct OAuthLimit {
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
    /// 窗口类型（"five_hour" / "seven_day"；仅周窗口类型进模型专属周行）
    #[serde(default)]
    limit_type: Option<String>,
    /// 归属范围（"All models" / "Opus" / "Sonnet"，大小写不敏感识别）
    #[serde(default)]
    scope: Option<String>,
    /// 模型专属周窗口标识
    #[serde(default)]
    weekly_scoped: Option<bool>,
}

/// extra_usage（月度超额消费，美元）→ (已用, 上限)。字段名按 utilization
/// 响应惯例做双兼容尝试：已用取 spend|used，上限取 limit|monthly_limit；
/// 两者全缺（或结构不是对象）→ None（不渲染附加信息行）。
fn parse_extra_usage(v: &serde_json::Value) -> Option<(Option<f64>, Option<f64>)> {
    let used = crate::provider_quota::num_any(v, &["spend", "used"]);
    let limit = crate::provider_quota::num_any(v, &["limit", "monthly_limit"]);
    (used.is_some() || limit.is_some()).then_some((used, limit))
}

/// limits[] 中带 weekly_scoped 标识的单条模型专属周窗口补位（纯函数）。
/// 只在对应槽位为空时写入（顶层 seven_day_opus/sonnet 已验证字段优先）；
/// "All models" 通用 scope 与非周窗口类型（five_hour）跳过，无法识别模型名
/// 的 scoped 条目保守忽略（不猜测归属）。
fn apply_scoped_weekly_limit(
    sonnet: &mut Option<OAuthWindow>,
    opus: &mut Option<OAuthWindow>,
    limit: &OAuthLimit,
) {
    if limit.weekly_scoped != Some(true) || limit.utilization.is_none() {
        return;
    }
    // 仅周窗口类型进模型专属周行（five_hour 的 scoped 条目不展示）
    if matches!(limit.limit_type.as_deref(), Some(t)
        if !t.contains("seven_day") && !t.contains("weekly"))
    {
        return;
    }
    let Some(scope) = limit.scope.as_deref().map(str::to_ascii_lowercase) else {
        return;
    };
    if scope.contains("all model") || scope.contains("all_model") {
        return; // 通用 scope 留主周行（seven_day），不进模型专属槽位
    }
    let slot = if scope.contains("opus") {
        opus
    } else if scope.contains("sonnet") {
        sonnet
    } else {
        return;
    };
    if slot.is_none() {
        *slot = Some(OAuthWindow {
            utilization: limit.utilization,
            resets_at: limit.resets_at.clone(),
        });
    }
}

/// /api/oauth/usage 响应体 → 结构化额度（纯函数，本地登录态链路与手动凭证
/// 链路共用；plan_type 由调用方按凭证信息填充，此处恒为 None）。
pub(crate) fn parse_usage_response(
    body: &str,
) -> Result<ClaudeRateLimits, ClaudeUsageFailure> {
    let mut resp: OAuthUsageResponse = serde_json::from_str(body).map_err(|e| {
        ClaudeUsageFailure::Other(format!("解析实时额度失败: {e}"))
    })?;
    // limits[] 中带 weekly_scoped 标识的模型专属窗口补位（顶层字段优先）
    let limits = std::mem::take(&mut resp.limits);
    for limit in &limits {
        apply_scoped_weekly_limit(&mut resp.seven_day_sonnet, &mut resp.seven_day_opus, limit);
    }

    let conv =
        |w: &OAuthWindow| -> (Option<f64>, Option<i64>) { (w.utilization, w.resets_at.as_deref().and_then(parse_ts_ms)) };
    let (primary_pct, primary_reset_at) = resp
        .five_hour
        .as_ref()
        .map(conv)
        .unwrap_or((None, None));
    let (secondary_pct, secondary_reset_at) = resp
        .seven_day
        .as_ref()
        .map(conv)
        .unwrap_or((None, None));
    let (sonnet_weekly_pct, sonnet_weekly_reset_at) = resp
        .seven_day_sonnet
        .as_ref()
        .map(conv)
        .unwrap_or((None, None));
    let (opus_weekly_pct, opus_weekly_reset_at) = resp
        .seven_day_opus
        .as_ref()
        .map(conv)
        .unwrap_or((None, None));
    let (extra_used, extra_limit) = resp
        .extra_usage
        .as_ref()
        .and_then(parse_extra_usage)
        .unwrap_or((None, None));

    Ok(ClaudeRateLimits {
        plan_type: None,
        primary_pct,
        primary_reset_at,
        secondary_pct,
        secondary_reset_at,
        sonnet_weekly_pct,
        sonnet_weekly_reset_at,
        opus_weekly_pct,
        opus_weekly_reset_at,
        extra_used,
        extra_limit,
    })
}

/// 实时额度结果缓存（成功 60s / 失败 15s 双 TTL）。
/// 成功缓存：前端多命令高频触发，防止打爆端点（该端点对高频请求返回 429）。
/// 失败负缓存：无凭据/网络不通时同样会被高频触发，一轮 30s tick 内并发
/// 4~5 个真实 HTTP 请求（各 10s 超时）既浪费也可能触发 Anthropic 限流，
/// 故失败结果也短暂缓存。
static LIVE_LIMITS_CACHE: OnceLock<
    Mutex<Option<(std::time::Instant, Result<Option<ClaudeRateLimits>, ClaudeUsageFailure>)>>,
> = OnceLock::new();

/// single-flight 在途标记：True = 有线程正在发真实 HTTP（照 codex.rs 模式）。
/// 同一轮多个命令并发同时 cache miss 时，只有首个线程发请求，其余在
/// Condvar 上等待共享同一份结果（否则一轮 30s tick 内并发 4~5 个重复
/// HTTP 各 10s 超时，还可能触发 Anthropic 429 限流）。等待有上限
/// （大于 HTTP 总超时），leader 意外挂死时等待者超时后退回各自请求，
/// 不会永久阻塞。
static LIVE_LIMITS_INFLIGHT: OnceLock<(Mutex<bool>, std::sync::Condvar)> = OnceLock::new();

/// single-flight 等待上限：略大于 HTTP 总超时 10s，leader 正常必然先完成。
const LIVE_LIMITS_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// OAuth 额度端点（Claude Code CLI 内部同款；需 claude.ai 订阅 OAuth 登录，
/// 需带 anthropic-beta: oauth-2025-04-20 头）。
const OAUTH_USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";

/// 拉取 Claude 订阅额度：GET https://api.anthropic.com/api/oauth/usage
/// （第三方中转/API Key 模式无凭据，返回 Err 由调用方降级为不展示额度块）。
#[allow(dead_code)]
pub fn fetch_live_rate_limits() -> Result<Option<ClaudeRateLimits>, String> {
    fetch_live_rate_limits_with_freshness()
        .map(|(limits, _)| limits)
        .map_err(|f| f.message())
}

/// 拉取实时额度并标记本次结果是否来自新的 HTTP 请求。
/// 缓存命中仍可用于当前进度展示，但不应作为新的历史采样。
/// 失败按 ClaudeUsageFailure 分类（授权类可透出明确提示，网络类静默降级）。
/// single-flight 的等待者拿到的是 leader 新请求的结果，但标记为 false：
/// 同一轮的采样由 leader 完成一次即可，避免并发命令各自重复写入历史。
pub fn fetch_live_rate_limits_with_freshness(
) -> Result<(Option<ClaudeRateLimits>, bool), ClaudeUsageFailure> {
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
                eprintln!("[agenthud-claude] 实时额度在途请求等待超时，本线程改为直接请求");
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

/// 用指定 OAuth access token 请求 usage 端点（本地登录态链路与手动凭证
/// 链路共用；agent 由调用方构造，各自决定超时）。返回展平后的
/// (HTTP 状态码, 响应体)；401/403 归类为 TokenRejected，网络层失败归 Other。
fn fetch_usage_raw(
    agent: &ureq::Agent,
    token: &str,
) -> Result<(u16, Option<String>), ClaudeUsageFailure> {
    let resp = agent
        .get(OAUTH_USAGE_ENDPOINT)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("User-Agent", "claude-code/2.1.0")
        .call();
    let (status, body) =
        crate::provider_quota::flatten_response(resp).map_err(ClaudeUsageFailure::Other)?;
    if status == 401 || status == 403 {
        return Err(ClaudeUsageFailure::TokenRejected);
    }
    Ok((status, body))
}

fn fetch_live_rate_limits_uncached() -> Result<Option<ClaudeRateLimits>, ClaudeUsageFailure> {
    let auth = load_claude_auth()?;
    let mut builder = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10));
    // 复用 Codex 模块的代理探测（环境变量 > 系统代理 > 直连）：
    // api.anthropic.com 与 chatgpt.com 一样在部分网络需代理才可达
    if let Some(url) = crate::codex::resolve_proxy() {
        match ureq::Proxy::new(&url) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => eprintln!("[agenthud-claude] 代理地址无效（改为直连）: {url} ({e})"),
        }
    }
    let agent = builder.build();

    let (status, body) = fetch_usage_raw(&agent, &auth.access_token)?;
    if status != 200 {
        return Err(ClaudeUsageFailure::Other(format!(
            "实时额度查询失败（HTTP {status}）"
        )));
    }
    let mut limits = parse_usage_response(body.as_deref().unwrap_or_default())?;
    // 套餐名：parse_credentials_json 已做乘数档位归一（Max 5x / Max 20x）
    limits.plan_type = auth.plan_label;

    let has_window = limits.primary_pct.is_some()
        || limits.secondary_pct.is_some()
        || limits.opus_weekly_pct.is_some()
        || limits.sonnet_weekly_pct.is_some();
    Ok(has_window.then_some(limits))
}

// ===== 手动凭证多账号查询（provider_quota 的 "claude" 分支调用）=====
// 用户在凭证体系（~/.agenthud/credentials/claude.json）手动添加 kind=token 条目
// （sk-ant-oat 开头的 OAuth access token，可从本机/另一台机器的
// .credentials.json 的 claudeAiOauth.accessToken 取得）。本地登录态不并入
// 本链路——本地路径继续走 fetch_live_rate_limits（带 60s 缓存与历史采样），
// 避免同一 token 双查询。

/// ClaudeRateLimits → 通用额度面板窗口列表（手动凭证条目卡渲染用）。
/// 标题与 grok.rs 同为硬编码中文短语（后端产出、前端直展示）；
/// 百分比窗口 clamp 0-100，超额消费为金额行（limit 缺失时前端只展示金额）。
fn quota_windows_from_limits(
    limits: &ClaudeRateLimits,
) -> Vec<crate::provider_quota::ProviderQuotaWindow> {
    use crate::provider_quota::ProviderQuotaWindow;
    let pct_window = |key: &str, title: &str, pct: Option<f64>, reset: Option<i64>| {
        pct.map(|p| ProviderQuotaWindow {
            key: key.to_string(),
            title: title.to_string(),
            used_percent: Some(p.clamp(0.0, 100.0)),
            used: None,
            total: None,
            unit: None,
            resets_at: reset,
        })
    };
    let mut out = Vec::new();
    for w in [
        pct_window("hour5", "5 小时", limits.primary_pct, limits.primary_reset_at),
        pct_window("weekly", "本周", limits.secondary_pct, limits.secondary_reset_at),
        pct_window("opus_weekly", "Opus 周额度", limits.opus_weekly_pct, limits.opus_weekly_reset_at),
        pct_window("sonnet_weekly", "Sonnet 周额度", limits.sonnet_weekly_pct, limits.sonnet_weekly_reset_at),
    ]
    .into_iter()
    .flatten()
    {
        out.push(w);
    }
    if limits.extra_used.is_some() || limits.extra_limit.is_some() {
        out.push(ProviderQuotaWindow {
            key: "extra_usage".to_string(),
            title: "超额消费".to_string(),
            used_percent: None,
            used: limits.extra_used,
            total: limits.extra_limit,
            unit: Some("$".to_string()),
            resets_at: None,
        });
    }
    out
}

/// 手动凭证的单条查询结果 → 展示条目（纯函数，单测直接构造输入）。
/// 分支：401/403(expired「Token 已失效」；fetch_usage_raw 归 TokenRejected，
/// 此处对原始状态码做同款兜底) > 网络失败(error) > 非 200(error)
/// > 解析失败/缺用量(error) > 成功(ok + 各窗口 + 超额消费行)。
/// 手动 token 无凭据 JSON，套餐名无来源（plan_name=None）。
fn entry_from_usage_raw(
    cred_id: &str,
    label: &str,
    raw: &Result<(u16, Option<String>), ClaudeUsageFailure>,
) -> crate::provider_quota::ProviderQuotaEntry {
    use crate::provider_quota::{now_ms, ProviderQuotaEntry};
    let fail = |status: &str, message: String| ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: status.to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some(message),
        updated_at: now_ms(),
    };
    let token_rejected = || {
        fail(
            "expired",
            "Token 已失效，请重新获取并更新 sk-ant-oat 凭证".to_string(),
        )
    };
    let (status, body) = match raw {
        Ok(pair) => pair.clone(),
        Err(f) => {
            let (status, message) = match f {
                ClaudeUsageFailure::TokenRejected => (
                    "expired",
                    "Token 已失效，请重新获取并更新 sk-ant-oat 凭证".to_string(),
                ),
                ClaudeUsageFailure::MissingOAuth(m) => ("error", m.clone()),
                ClaudeUsageFailure::Other(e) => ("error", format!("Claude 额度{e}")),
            };
            return fail(status, message);
        }
    };
    if status == 401 || status == 403 {
        return token_rejected();
    }
    if status != 200 {
        return fail("error", format!("Claude 额度查询失败（HTTP {status}）"));
    }
    let limits = match parse_usage_response(body.as_deref().unwrap_or_default()) {
        Ok(l) => l,
        Err(ClaudeUsageFailure::Other(e)) => return fail("error", e),
        Err(_) => return fail("error", "Claude 额度响应解析失败".to_string()),
    };
    let windows = quota_windows_from_limits(&limits);
    if windows.is_empty() {
        return fail("error", "Claude 额度响应缺少用量数据".to_string());
    }
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows,
        balance: None,
        plan_name: limits.plan_type.clone(),
        message: None,
        updated_at: now_ms(),
    }
}

/// 查询手动凭证（kind=token）的订阅额度：逐条调同一 OAuth usage 端点
/// （Bearer <手动token> + 同 beta 头），解析复用 parse_usage_response。
/// 单条失败产出 error/expired 条目，不阻塞其他条目；secret 为空的凭证跳过。
pub(crate) fn fetch_manual_quota_entries(
    snapshots: &[crate::provider_credentials::CredentialQuerySnapshot],
) -> Vec<crate::provider_quota::ProviderQuotaEntry> {
    let agent = crate::provider_quota::quota_http_agent();
    let mut entries = Vec::new();
    for cred in snapshots {
        let token = cred.secret.trim();
        if token.is_empty() {
            continue;
        }
        entries.push(entry_from_usage_raw(
            &cred.id,
            &cred.label,
            &fetch_usage_raw(&agent, token),
        ));
    }
    entries
}

// ===== 诊断 =====

/// Claude 诊断信息（排查"无数据"问题）
#[derive(Debug, Clone, Serialize)]
pub struct ClaudeDebugInfo {
    /// projects 目录路径
    pub projects_dir: String,
    /// 目录是否存在
    pub projects_dir_exists: bool,
    /// 目录下会话 jsonl 文件数
    pub session_files: usize,
    /// 导入库累计记录数
    pub imported_records: i64,
    /// 最新一条用量的时间（毫秒）
    pub latest_session_ms: Option<i64>,
}

/// 诊断信息（get_codex_debug 同款用途）。导入失败不阻断——目录不存在时
/// 恰恰要靠这些字段定位问题。诊断必须真实执行导入（绕过节流）。
pub fn debug_info() -> Result<ClaudeDebugInfo, String> {
    if let Err(e) = import_incremental_force() {
        eprintln!("[agenthud-claude] 诊断时增量导入失败: {e}");
    }

    let dir = projects_dir_path();
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_session_files(&dir, 5, &mut files);
    }

    let (imported_records, latest_session_ms) = open_claude_db()
        .map(|conn| {
            conn.query_row(
                "SELECT COUNT(*), MAX(started_at) FROM model_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((0, None))
        })
        .unwrap_or((0, None));

    Ok(ClaudeDebugInfo {
        projects_dir: dir.display().to_string(),
        projects_dir_exists: dir.is_dir(),
        session_files: files.len(),
        imported_records,
        latest_session_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// 凭据 JSON 解析：标准结构 / 缺 accessToken 报错 / 无 claudeAiOauth 报错。
    #[test]
    fn parse_credentials_variants() {
        let auth = parse_credentials_json(
            r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"r",
                "expiresAt":1755000000000,"scopes":["user:profile"],
                "rateLimitTier":"pro","subscriptionType":"pro"}}"#,
        )
        .expect("标准凭据解析失败");
        assert_eq!(auth.access_token, "tok");
        assert_eq!(auth.plan_label.as_deref(), Some("pro"));

        let tier_only = parse_credentials_json(
            r#"{"claudeAiOauth":{"accessToken":"tok","rateLimitTier":"max5"}}"#,
        )
        .expect("仅 rateLimitTier 的凭据解析失败");
        assert_eq!(tier_only.plan_label.as_deref(), Some("max5"));

        // 无 claudeAiOauth（如 Claude Code 2.1.x 钥匙串条目仅含 mcpOAuth）/
        // 有条目但缺 accessToken：均为明确错误态（用户可运行 claude 自助修复），
        // 而非模糊网络错误
        for broken in [r#"{"mcpOAuth":{}}"#, r#"{"claudeAiOauth":{}}"#] {
            let err = parse_credentials_json(broken).expect_err("应报 MissingOAuth");
            match err {
                ClaudeUsageFailure::MissingOAuth(m) => {
                    assert_eq!(m, MISSING_OAUTH_MESSAGE);
                }
                other => panic!("应为 MissingOAuth，实际 {other:?}"),
            }
        }
        // JSON 本身损坏 → Other（静默降级，不误导为授权问题）
        assert!(matches!(
            parse_credentials_json("not json"),
            Err(ClaudeUsageFailure::Other(_))
        ));
    }

    /// 套餐推断增强：rateLimitTier 含乘数标识（default_claude_max_5x /
    /// default_claude_max_20x）→ 套餐名归一为 "Max 5x"/"Max 20x"；
    /// 无乘数的 tier/subscriptionType 沿用既有优先级原样展示。
    #[test]
    fn plan_tier_multiplier_label() {
        let parse_label = |json: &str| {
            parse_credentials_json(json)
                .expect("凭据解析失败")
                .plan_label
        };
        assert_eq!(
            parse_label(r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"default_claude_max_5x"}}"#).as_deref(),
            Some("Max 5x")
        );
        assert_eq!(
            parse_label(r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"default_claude_max_20x"}}"#).as_deref(),
            Some("Max 20x")
        );
        // subscriptionType 通常不携带乘数；tier 含乘数时乘数优先
        assert_eq!(
            parse_label(
                r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"default_claude_max_20x","subscriptionType":"max"}}"#
            )
            .as_deref(),
            Some("Max 20x")
        );
        // 无乘数：沿用 subscriptionType → rateLimitTier 优先级
        assert_eq!(
            parse_label(r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"max","subscriptionType":"pro"}}"#).as_deref(),
            Some("pro")
        );
        // "max5"（无 x）不是乘数标识，原样保留
        assert_eq!(
            parse_label(r#"{"claudeAiOauth":{"accessToken":"t","rateLimitTier":"max5"}}"#).as_deref(),
            Some("max5")
        );
    }

    /// OAuth 额度响应解析口径：utilization 百分比直读、resets_at ISO 转毫秒。
    #[test]
    fn oauth_usage_response_parsing() {
        let v: OAuthUsageResponse = serde_json::from_str(
            r#"{"five_hour":{"utilization":42.5,"resets_at":"2026-08-14T12:00:00Z"},
                "seven_day":{"utilization":13.0,"resets_at":"2026-08-17T03:30:00Z"}}"#,
        )
        .expect("响应解析失败");
        assert_eq!(v.five_hour.as_ref().unwrap().utilization, Some(42.5));
        assert_eq!(v.seven_day.as_ref().unwrap().utilization, Some(13.0));
        // 毫秒换算用纪元原点验证（数值直观不易错），另验证带毫秒小数的时间可解析
        assert_eq!(parse_ts_ms("1970-01-01T00:00:00Z"), Some(0));
        assert!(parse_ts_ms("2026-06-12T19:12:00.759Z").is_some());
    }

    /// 模型专属周窗口：顶层 seven_day_sonnet / seven_day_opus 字段映射到
    /// ClaudeRateLimits 增量字段（含 resets_at 毫秒转换）。
    #[test]
    fn model_scoped_weekly_windows() {
        let limits = parse_usage_response(
            r#"{"five_hour":{"utilization":10.0,"resets_at":"2026-08-14T12:00:00Z"},
                "seven_day":{"utilization":20.0,"resets_at":"2026-08-17T03:30:00Z"},
                "seven_day_sonnet":{"utilization":33.0,"resets_at":"2026-08-17T04:00:00Z"},
                "seven_day_opus":{"utilization":55.5,"resets_at":"2026-08-18T05:00:00Z"}}"#,
        )
        .expect("解析失败");
        assert_eq!(limits.primary_pct, Some(10.0));
        assert_eq!(limits.secondary_pct, Some(20.0));
        assert_eq!(limits.sonnet_weekly_pct, Some(33.0));
        assert_eq!(
            limits.sonnet_weekly_reset_at,
            parse_ts_ms("2026-08-17T04:00:00Z")
        );
        assert_eq!(limits.opus_weekly_pct, Some(55.5));
        assert_eq!(
            limits.opus_weekly_reset_at,
            parse_ts_ms("2026-08-18T05:00:00Z")
        );
        assert_eq!(limits.extra_used, None);
        assert_eq!(limits.extra_limit, None);
    }

    /// limits[] 补充窗口（CodexBar 口径）：weekly_scoped=true 且 scope 可识别
    /// 模型（Opus/Sonnet）→ 补进对应模型专属槽位；"All models" 通用 scope 留
    /// 主周行不覆盖；five_hour 类型的 scoped 条目忽略；顶层字段优先；
    /// 无法识别模型名的 scoped 条目保守忽略。
    #[test]
    fn weekly_scoped_limits_fill() {
        let limits = parse_usage_response(
            r#"{"five_hour":{"utilization":10.0},
                "seven_day":{"utilization":20.0},
                "limits":[
                    {"limit_type":"five_hour","scope":"All models","utilization":11.0},
                    {"limit_type":"seven_day","scope":"All models","utilization":21.0},
                    {"limit_type":"seven_day","scope":"Opus","weekly_scoped":true,"utilization":66.0,"resets_at":"2026-08-18T05:00:00Z"},
                    {"limit_type":"seven_day","scope":"Sonnet","weekly_scoped":true,"utilization":44.0}
                ]}"#,
        )
        .expect("解析失败");
        // 通用 "All models" scope 不覆盖主周行（仍是 seven_day 的 20.0）
        assert_eq!(limits.secondary_pct, Some(20.0));
        assert_eq!(limits.primary_pct, Some(10.0));
        assert_eq!(limits.opus_weekly_pct, Some(66.0));
        assert_eq!(limits.sonnet_weekly_pct, Some(44.0));

        // 顶层 seven_day_opus 优先，limits 里的 scoped 条目不覆盖已有槽位
        let limits = parse_usage_response(
            r#"{"seven_day_opus":{"utilization":5.0},
                "limits":[{"limit_type":"seven_day","scope":"Opus","weekly_scoped":true,"utilization":99.0},
                          {"limit_type":"seven_day","scope":"Haiku","weekly_scoped":true,"utilization":7.0}]}"#,
        )
        .expect("解析失败");
        assert_eq!(limits.opus_weekly_pct, Some(5.0));
        assert_eq!(limits.sonnet_weekly_pct, None); // 未知模型名保守忽略
    }

    /// extra_usage（月度超额消费）：spend/limit 与 used/limit 双兼容取值，
    /// 两者全缺 / 无该字段 → None（不渲染附加信息行）。
    #[test]
    fn extra_usage_parsing() {
        // spend/limit 结构
        let limits =
            parse_usage_response(r#"{"extra_usage":{"spend":3.2,"limit":35}}"#).expect("解析失败");
        assert_eq!(limits.extra_used, Some(3.2));
        assert_eq!(limits.extra_limit, Some(35.0));
        // used/limit 兼容形态 + 仅一侧有值
        let limits =
            parse_usage_response(r#"{"extra_usage":{"used":1.5}}"#).expect("解析失败");
        assert_eq!(limits.extra_used, Some(1.5));
        assert_eq!(limits.extra_limit, None);
        // 字符串数字（数值解析弹性，与 provider_quota::parse_flexible_f64 同口径）
        let limits =
            parse_usage_response(r#"{"extra_usage":{"spend":"12.5","monthly_limit":"100"}}"#)
                .expect("解析失败");
        assert_eq!(limits.extra_used, Some(12.5));
        assert_eq!(limits.extra_limit, Some(100.0));
        // 全缺 / 空对象 / 无字段 → None
        let limits =
            parse_usage_response(r#"{"extra_usage":{"other":1}}"#).expect("解析失败");
        assert_eq!(limits.extra_used, None);
        assert_eq!(limits.extra_limit, None);
        let limits = parse_usage_response(r#"{"five_hour":{"utilization":1.0}}"#).expect("解析失败");
        assert_eq!(limits.extra_used, None);
    }

    /// 旧响应零回归：只有 five_hour/seven_day 的历史响应，解析后增量字段
    /// 全部为 None，主字段口径与改造前一致。
    #[test]
    fn legacy_response_backward_compat() {
        let limits = parse_usage_response(
            r#"{"five_hour":{"utilization":42.5,"resets_at":"2026-08-14T12:00:00Z"},
                "seven_day":{"utilization":13.0,"resets_at":"2026-08-17T03:30:00Z"}}"#,
        )
        .expect("解析失败");
        assert_eq!(limits.plan_type, None);
        assert_eq!(limits.primary_pct, Some(42.5));
        assert_eq!(limits.secondary_pct, Some(13.0));
        assert_eq!(limits.sonnet_weekly_pct, None);
        assert_eq!(limits.sonnet_weekly_reset_at, None);
        assert_eq!(limits.opus_weekly_pct, None);
        assert_eq!(limits.opus_weekly_reset_at, None);
        assert_eq!(limits.extra_used, None);
        assert_eq!(limits.extra_limit, None);
        // 坏 JSON → Other（本地链路静默降级）
        assert!(matches!(
            parse_usage_response("not json"),
            Err(ClaudeUsageFailure::Other(_))
        ));
    }

    /// 手动凭证条目状态映射：401/403 → expired「Token 已失效」；网络失败 /
    /// 非 200 / 坏 JSON → error；完整成功响应 → ok + 各窗口 + 超额消费金额行。
    #[test]
    fn manual_entry_status_mapping() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some("denied".to_string())));
            let entry = entry_from_usage_raw("abc-1", "Max 订阅", &raw);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert!(
                entry.message.as_deref().unwrap().contains("Token 已失效"),
                "expired 消息应包含 Token 已失效"
            );
            assert!(entry.windows.is_empty());
        }
        // 网络层失败 → error
        let raw: Result<(u16, Option<String>), ClaudeUsageFailure> =
            Err(ClaudeUsageFailure::Other("网络错误或服务不可用: timeout".into()));
        let entry = entry_from_usage_raw("abc-1", "手动", &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误"));
        // 500 → error 带状态码
        let entry = entry_from_usage_raw("abc-1", "手动", &Ok((500, Some("oops".into()))));
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));
        // 200 坏 JSON → error
        let entry = entry_from_usage_raw("abc-1", "手动", &Ok((200, Some("not json".into()))));
        assert_eq!(entry.status, "error");
    }

    /// 手动凭证成功路径：ok 条目携带 hour5/weekly/opus/sonnet 四个百分比窗口
    /// （标题为后端硬编码中文短语）与超额消费金额行。
    #[test]
    fn manual_entry_ok_windows() {
        let body = r#"{"five_hour":{"utilization":10.0,"resets_at":"2026-08-14T12:00:00Z"},
            "seven_day":{"utilization":20.0,"resets_at":"2026-08-17T03:30:00Z"},
            "seven_day_opus":{"utilization":66.0},
            "seven_day_sonnet":{"utilization":44.0},
            "extra_usage":{"spend":3.2,"limit":35}}"#;
        let entry = entry_from_usage_raw("cred-9", "另一台机器", &Ok((200, Some(body.into()))));
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.message, None);
        let keys: Vec<&str> = entry.windows.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, vec!["hour5", "weekly", "opus_weekly", "sonnet_weekly", "extra_usage"]);
        assert_eq!(entry.windows[0].title, "5 小时");
        assert_eq!(entry.windows[1].title, "本周");
        assert_eq!(entry.windows[2].title, "Opus 周额度");
        assert_eq!(entry.windows[3].title, "Sonnet 周额度");
        // 超额消费金额行：used/total 为美元金额，unit 为 $
        let extra = &entry.windows[4];
        assert_eq!(extra.title, "超额消费");
        assert_eq!(extra.used, Some(3.2));
        assert_eq!(extra.total, Some(35.0));
        assert_eq!(extra.unit.as_deref(), Some("$"));
        assert_eq!(extra.used_percent, None);
        // 响应无任何窗口 → error（缺用量数据）
        let entry = entry_from_usage_raw("cred-9", "手动", &Ok((200, Some("{}".into()))));
        assert_eq!(entry.status, "error");
    }

    /// 冒烟测试：对本机真实 projects 目录执行增量导入并做全范围查询，
    /// 打印结果供人工核对（无 Claude 环境时跳过，不视为失败）。
    /// 重复运行第二次验证幂等：记录数不因重复导入而变化。
    #[test]
    fn import_and_query_smoke() {
        if projects_dir().is_err() {
            eprintln!("[test] 本机无 Claude projects 目录，跳过");
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
        // 趋势管道：至少有一个非空桶
        if let (Some(e), Some(l)) = (first.earliest_ms, first.latest_ms) {
            let trend = query_trend(e, l + 3_600_000, "hour").expect("趋势查询失败");
            let non_empty = trend.iter().filter(|b| b.total_tokens > 0).count();
            assert!(non_empty > 0, "趋势桶全部为空");
            eprintln!("[test] trend 桶数={} 非空桶={}", trend.len(), non_empty);
        }
        eprintln!(
            "[test] requests={} overall={:?} by_model={:?}",
            first.overall.requests,
            first.overall,
            first
                .by_model
                .iter()
                .map(|m| (m.model_id.clone(), m.requests, m.total_tokens))
                .collect::<Vec<_>>(),
        );
        // 修订查询：单次导入内"部分值行先入库、终值行覆盖"会打上 updated_at，
        // 本机存在多行 message 的会话时应有修订行；无则 0 条（查询本身必须成功）
        let revised = query_revised_since(0, 0, 500).expect("修订查询失败");
        eprintln!(
            "[test] 修订行数量={}（示例 {:?}）",
            revised.len(),
            revised
                .iter()
                .take(3)
                .map(|r| (r.local_rowid, r.computed_total_tokens))
                .collect::<Vec<_>>()
        );
        // 实时额度（OAuth）：本机未登录订阅/网络不通时应返回 Err（额度块不展示）
        match fetch_live_rate_limits() {
            Ok(v) => eprintln!("[test] 实时额度: {v:?}"),
            Err(e) => eprintln!("[test] 实时额度不可用（不展示额度块）: {e}"),
        }
    }
    /// 会话悬浮窗窗口查询（query_window_sessions_at）的 SQL 聚合口径：
    /// 活跃判定看窗口、累计看全生命周期、cwd 取会话最近一行非空值、
    /// 请求平均速度仅统计 duration_ms > 0 行（NULL/0 排除）、
    /// total_active 含未展示会话、limit 截断。
    #[test]
    fn hud_window_sessions_aggregation() {
        let tmp = std::env::temp_dir().join(format!("zbar-claude-hudwin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let db_path = tmp.join("claude-hudwin.sqlite");
        let conn = Connection::open(&db_path).expect("打开临时库失败");
        // 复用生产建表（open_claude_db 写死 ~/.agenthud 路径，测试直接手建
        // 同构最小表；列集与 query_window_sessions_at 用到的列一致）
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                dedupe_key TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                model_id TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                cwd TEXT,
                project_key TEXT,
                duration_ms REAL,
                UNIQUE(dedupe_key));
             CREATE INDEX idx_mu_started ON model_usage(started_at);
             CREATE INDEX idx_mu_session ON model_usage(session_id);",
        )
        .expect("建测试表失败");
        let now = 1_000_000_000_000_i64;
        let win = now - 10 * 60_000;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage
                (session_id, dedupe_key, started_at, model_id,
                 input_tokens, output_tokens, cache_read_input_tokens, cwd, duration_ms)
             VALUES
                -- s-a：窗口内两行；cwd 取最近一行（第二行 /b 覆盖第一行 /a）；
                -- 两行均有有效耗时 → 速度原料 Σout = 4 / Σms = 2500
                ('s-a', 'd1', {a}, 'claude-x', 100, 3, 40, '/Users/a/proj-a', 2000.0),
                ('s-a', 'd2', {b}, 'claude-y', 10, 1, 0, '/Users/a/proj-b', 500.0),
                -- s-b：窗口内仅一小行，早期大额行计入全生命周期累计；
                -- 耗时 NULL / 0 的行不参与速度聚合
                ('s-b', 'd3', {early}, 'claude-x', 9000, 8000, 100, NULL, NULL),
                ('s-b', 'd4', {c}, 'claude-x', 10, 1, 0, NULL, 0.0),
                -- s-ghost：仅窗口外活跃 → 不算活跃
                ('s-ghost', 'd5', {early2}, 'claude-x', 5, 5, 0, NULL, NULL);",
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
        // 排序：最近活动倒序（s-a 的最新行比 s-b 新）
        assert_eq!(found.sessions[0].session_id, "s-a");
        assert_eq!(found.sessions[1].session_id, "s-b");
        // s-a：↑ = 60 + 10 = 70、↓ = 4、⟲ = 40、× = 2；cwd 取最近一行 /b
        let s_a = &found.sessions[0];
        assert_eq!(s_a.in_tokens, 110);
        assert_eq!(s_a.out_tokens, 4);
        assert_eq!(s_a.cache_read, 40);
        assert_eq!(s_a.req_count, 2);
        assert_eq!(s_a.models.as_deref(), Some("claude-x,claude-y"));
        assert_eq!(s_a.project.as_deref(), Some("/Users/a/proj-b"));
        // s-a 速度原料：两行耗时均有效 → Σout = 4、Σms = 2500
        // （换算为 1.6 t/s 由 brief_from_agent_row 完成）
        assert_eq!(s_a.speed_out_tokens, 4);
        assert!((s_a.speed_ms - 2500.0).abs() < f64::EPSILON);
        // s-b：全生命周期 ↑ = 8900 + 10、cwd 全空 → None；
        // NULL/0 耗时行全部排除 → 无速度原料
        let s_b = &found.sessions[1];
        assert_eq!(s_b.in_tokens, 9010);
        assert_eq!(s_b.out_tokens, 8001);
        assert_eq!(s_b.project, None);
        assert_eq!(s_b.speed_out_tokens, 0);
        assert!((s_b.speed_ms - 0.0).abs() < f64::EPSILON);

        // limit 截断：只取最新 1 条，total_active 仍为窗口内全部
        let found = query_window_sessions_at(&conn, win, 1).expect("limit 查询失败");
        assert_eq!(found.total_active, 2);
        assert_eq!(found.sessions.len(), 1);
        assert_eq!(found.sessions[0].session_id, "s-a");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 导入落库只写真实耗时（行内 durationMs，旧版 CLI 无该字段即 NULL）：
    /// 同一 message.id 的流式累计行以「总量更大者胜」取末行，重解析幂等。
    #[test]
    fn import_one_file_persists_real_duration() {
        let tmp = std::env::temp_dir().join(format!("zbar-claude-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let db_path = tmp.join("claude-import.sqlite");
        let mut conn = Connection::open(&db_path).expect("打开临时库失败");
        conn.execute_batch(SCHEMA_SQL).expect("初始化临时库失败");

        let file = tmp.join("077cdc16-d987-44ac-b445-3f6d0abb4861.jsonl");
        let lines = [
            r#"{"timestamp":"2026-06-12T19:12:00.000Z","type":"user","message":{"role":"user"}}"#,
            // 旧版 CLI 无 durationMs：耗时保持 NULL（不写估算）
            r#"{"timestamp":"2026-06-12T19:12:05.000Z","type":"assistant","message":{"id":"msg_a","model":"glm-5.3","usage":{"input_tokens":10,"output_tokens":5}}}"#,
            // 同 message.id 的流式累计行（末行即终值）
            r#"{"timestamp":"2026-06-12T19:12:09.000Z","type":"assistant","message":{"id":"msg_a","model":"glm-5.3","usage":{"input_tokens":10,"output_tokens":20}}}"#,
            // 新版 CLI 行内有真实耗时
            r#"{"timestamp":"2026-06-12T19:13:07.500Z","type":"assistant","message":{"id":"msg_b","model":"glm-5.3","durationMs":7500,"usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ];
        std::fs::write(&file, lines.join("\n") + "\n").expect("写入测试会话文件失败");

        // 事务由调用方提供（与生产 run_import_files 的每文件事务同构）
        let tx = conn.transaction().expect("开启导入事务失败");
        import_one_file(&tx, &file, None).expect("导入失败");
        tx.commit().expect("提交导入事务失败");
        let rows = |conn: &Connection| -> Vec<(String, Option<f64>, i64)> {
            let mut stmt = conn
                .prepare(
                    "SELECT dedupe_key, duration_ms, computed_total_tokens
                     FROM model_usage ORDER BY id",
                )
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            rows(&conn),
            vec![
                ("msg_a".to_string(), None, 30), // 旧版行无真实耗时 → NULL
                ("msg_b".to_string(), Some(7500.0), 2), // 行内 durationMs 原样落库
            ]
        );

        // 重解析（文件被重写 / 续读路径）幂等：值不变
        let tx = conn.transaction().expect("开启重解析事务失败");
        import_one_file(&tx, &file, None).expect("重解析失败");
        tx.commit().expect("提交重解析事务失败");
        assert_eq!(rows(&conn).len(), 2, "重解析不应新增行");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// query_revised_since 的 SELECT 动态拼列 ↔ 数字下标映射回归测试
    /// （import_and_query_smoke 里的修订调用只验证「查询不报错」，本测试补
    /// 字段值断言）：SELECT 尾部由 speed_columns 动态拼入耗时/首字两列
    /// （生产 schema 只有 duration_ms、无 ttft 列，这里建含两列的库验证完整
    /// 映射），行读取用数字下标——列序与下标错位会让相邻字段串值。
    /// 缺两列的库再验证降级 None（NULL 占位分支）。
    #[test]
    fn revised_query_maps_speed_and_session_columns() {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        // 与 open_claude_db 的 SCHEMA_SQL 同构，另补 time_to_first_token_ms 列
        conn.execute_batch(
            "CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                dedupe_key TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                model_id TEXT NOT NULL DEFAULT '',
                provider_id TEXT NOT NULL DEFAULT 'claude',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                duration_ms REAL,
                time_to_first_token_ms REAL,
                updated_at INTEGER NOT NULL DEFAULT 0,
                cwd TEXT,
                project_key TEXT,
                UNIQUE(dedupe_key)
             );
             INSERT INTO model_usage (session_id, dedupe_key, started_at, model_id,
                input_tokens, output_tokens, computed_total_tokens, duration_ms,
                time_to_first_token_ms, updated_at, cwd, project_key)
             VALUES
                ('s-1', 'd-1', 1000, 'claude-x', 50, 25, 75, 4200.5, 660.0, 800, '/u/p1', 'p1'),
                -- 0/NULL 的耗时首字不给值（CASE WHEN > 0 口径）
                ('s-1', 'd-2', 2000, 'claude-x', 10, 5, 15, 0, NULL, 850, '/u/p1', 'p1'),
                -- updated_at 不达标的行不被选出
                ('s-2', 'd-3', 3000, 'claude-x', 1, 1, 2, 300.0, 50.0, 700, '/u/p2', 'p2');",
        )
        .expect("建表并写入样例行失败");

        let rows = query_revised_since_at(&conn, 750, 0, 10).expect("修订查询失败");
        assert_eq!(rows.len(), 2, "updated_at > 750 的行只有两行");
        let first = &rows[0];
        // 逐字段对齐 SELECT 列序（session/project 紧邻耗时列，一并断言防错位串值）
        assert_eq!(first.local_rowid, 1);
        assert_eq!(first.session_id.as_deref(), Some("s-1"));
        assert_eq!(first.project_key.as_deref(), Some("p1"));
        assert_eq!(first.project_display.as_deref(), Some("/u/p1"));
        assert_eq!(first.output_tokens, 25);
        assert_eq!(first.computed_total_tokens, 75);
        assert!((first.duration_ms.unwrap() - 4200.5).abs() < 1e-6);
        assert!((first.time_to_first_token_ms.unwrap() - 660.0).abs() < 1e-6);
        assert_eq!(rows[1].duration_ms, None, "duration_ms=0 不给值");
        assert_eq!(rows[1].time_to_first_token_ms, None, "ttft NULL 不给值");

        // 缺耗时/首字两列的库：两字段降级 None，查询不报错（NULL 占位分支）
        conn.execute_batch(
            "DROP TABLE model_usage;
             CREATE TABLE model_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                dedupe_key TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                model_id TEXT NOT NULL DEFAULT '',
                provider_id TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                cwd TEXT,
                project_key TEXT
             );
             INSERT INTO model_usage (session_id, dedupe_key, started_at, updated_at)
             VALUES ('s-9', 'd-9', 1000, 900);",
        )
        .expect("重建缺列表失败");
        let rows = query_revised_since_at(&conn, 880, 0, 10).expect("缺列库修订查询失败");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].duration_ms, None, "无 duration_ms 列应恒 None");
        assert_eq!(rows[0].time_to_first_token_ms, None, "无 ttft 列应恒 None");
        assert_eq!(rows[0].session_id.as_deref(), Some("s-9"));
    }

    // ===== 按回复取证（计时 / 状态 / 失败行 / 回填）的合成数据测试 =====
    // 按本机日志字段布局构造，不放真实对话内容。

    /// RFC3339 → 毫秒（断言里与行内时间戳同源换算，避免手抄数字漂移）
    fn ms(ts: &str) -> i64 {
        parse_ts_ms(ts).expect("测试时间戳解析失败")
    }

    /// 临时目录 + SCHEMA_SQL 库（与 open_claude_db 同构，但不碰 ~/.agenthud）
    fn setup_temp_db(tag: &str) -> (PathBuf, Connection) {
        let tmp =
            std::env::temp_dir().join(format!("zbar-claude-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let db_path = tmp.join("claude.sqlite");
        let conn = Connection::open(&db_path).expect("打开临时库失败");
        conn.execute_batch(SCHEMA_SQL).expect("初始化临时库失败");
        (tmp, conn)
    }

    fn write_session_lines(tmp: &Path, name: &str, content: &str) -> PathBuf {
        let file = tmp.join(name);
        std::fs::write(&file, content).expect("写入测试会话文件失败");
        file
    }

    /// 全量导入一次（known=None，与重解析路径一致）
    fn run_import(conn: &mut Connection, file: &Path) {
        let tx = conn.transaction().expect("开启导入事务失败");
        import_one_file(&tx, file, None).expect("导入失败");
        tx.commit().expect("提交导入事务失败");
    }

    /// 查一条回复的取证列（列序对齐 StoredReply）
    fn read_reply(conn: &Connection, key: &str) -> StoredReply {
        let mut stmt = conn
            .prepare(
                "SELECT started_at, model_id, input_tokens, output_tokens,
                        cache_read_input_tokens, cache_creation_input_tokens,
                        computed_total_tokens, duration_ms,
                        request_started_at, first_token_at, completed_at,
                        time_to_first_token_ms, timing_estimated, status, error_code, retry_count
                 FROM model_usage WHERE dedupe_key = ?1",
            )
            .expect("准备取证列查询失败");
        stmt.query_row([key], |row| {
            Ok(StoredReply {
                started_at: row.get(0)?,
                model_id: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cache_read_tokens: row.get(4)?,
                cache_creation_tokens: row.get(5)?,
                computed_total: row.get(6)?,
                duration_ms: row.get(7)?,
                request_started_at: row.get(8)?,
                first_token_at: row.get(9)?,
                completed_at: row.get(10)?,
                time_to_first_token_ms: row.get(11)?,
                timing_estimated: row.get(12)?,
                status: row.get(13)?,
                error_code: row.get(14)?,
                retry_count: row.get(15)?,
            })
        })
        .expect("读取回复行失败")
    }

    fn read_updated_at(conn: &Connection, key: &str) -> i64 {
        conn.query_row(
            "SELECT updated_at FROM model_usage WHERE dedupe_key = ?1",
            [key],
            |row| row.get(0),
        )
        .expect("读取 updated_at 失败")
    }

    fn count_rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM model_usage", [], |row| row.get(0))
            .expect("计数失败")
    }

    /// 思考开头的回复：首字来自 thinkingDurationMs、起点来自 api-request 行
    ///（timing_estimated = 0，耗时 / 首字不带 ≈），耗时 = 结束 − 起点。
    #[test]
    fn thinking_first_reply_uses_thinking_duration_and_api_request_start() {
        let (tmp, mut conn) = setup_temp_db("thinking");
        let file = write_session_lines(
            &tmp,
            "sess-thinking.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-12T10:00:00.000Z","type":"user","uuid":"u0","cwd":"/tmp/proj-a","message":{"role":"user"}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T10:00:00.500Z","type":"api-request","id":"req-1"}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T10:00:03.000Z","type":"assistant","uuid":"a1","parentUuid":"u0","requestRef":"req-1","apiBlockIndex":0,"thinkingDurationMs":2000,"message":{"id":"msg_t","model":"glm-5.3","content":[{"type":"thinking","thinking":"……"}],"usage":{"input_tokens":100,"output_tokens":2}}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T10:00:05.000Z","type":"assistant","uuid":"a2","parentUuid":"a1","requestRef":"req-1","apiBlockIndex":1,"message":{"id":"msg_t","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text","text":"回答"}],"usage":{"input_tokens":100,"output_tokens":20}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);

        let row = read_reply(&conn, "msg_t");
        // 用量取 computed_total 最大的末块；起点 = api-request 行时间戳（精确）
        assert_eq!(row.started_at, ms("2026-06-12T10:00:05.000Z"));
        assert_eq!(row.computed_total, 120);
        assert_eq!(row.output_tokens, 20);
        assert_eq!(row.request_started_at, Some(ms("2026-06-12T10:00:00.500Z")));
        assert_eq!(row.timing_estimated, Some(0), "api-request 关联的起点是精确值");
        // 首字 = 首块时间戳 − thinkingDurationMs
        assert_eq!(
            row.first_token_at,
            Some(ms("2026-06-12T10:00:03.000Z") - 2_000),
            "思考块的首字来自 thinkingDurationMs"
        );
        assert_eq!(row.completed_at, Some(ms("2026-06-12T10:00:05.000Z")));
        assert_eq!(row.duration_ms, Some(4_500.0), "耗时 = 结束 − 起点");
        assert_eq!(row.time_to_first_token_ms, Some(500.0));
        assert_eq!(row.status.as_deref(), Some("completed"));
        assert_eq!(row.error_code, None);
        assert_eq!(row.retry_count, 0);
        // 流式速度口径的生成窗口 = 结束 − 首字 = 4 秒，可换算出可信速度
        let window = row.completed_at.unwrap() - row.first_token_at.unwrap();
        assert_eq!(window, 4_000);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 工具调用开头的回复：首字来自首块 tool_use 的 toolInputStreamMs。
    #[test]
    fn tool_first_reply_uses_tool_input_stream_duration() {
        let (tmp, mut conn) = setup_temp_db("tool");
        let file = write_session_lines(
            &tmp,
            "sess-tool.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-12T11:00:00.000Z","type":"api-request","id":"req-2"}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T11:00:02.000Z","type":"assistant","requestRef":"req-2","apiBlockIndex":0,"toolInputStreamMs":800,"message":{"id":"msg_tool","model":"glm-5.3","content":[{"type":"tool_use","name":"Bash"}],"usage":{"input_tokens":10,"output_tokens":3}}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T11:00:04.000Z","type":"assistant","requestRef":"req-2","apiBlockIndex":1,"message":{"id":"msg_tool","model":"glm-5.3","stop_reason":"tool_use","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":8}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        let row = read_reply(&conn, "msg_tool");
        assert_eq!(row.request_started_at, Some(ms("2026-06-12T11:00:00.000Z")));
        assert_eq!(row.timing_estimated, Some(0));
        assert_eq!(
            row.first_token_at,
            Some(ms("2026-06-12T11:00:02.000Z") - 800),
            "工具调用首字来自 toolInputStreamMs"
        );
        assert_eq!(row.time_to_first_token_ms, Some(1_200.0));
        assert_eq!(row.status.as_deref(), Some("completed"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 文字开头的回复：首字来自首块 text 的 toolInputStreamMs。
    #[test]
    fn text_first_reply_uses_tool_input_stream_duration() {
        let (tmp, mut conn) = setup_temp_db("text");
        let file = write_session_lines(
            &tmp,
            "sess-text.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-12T12:00:01.000Z","type":"api-request","id":"req-3"}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T12:00:03.000Z","type":"assistant","requestRef":"req-3","apiBlockIndex":0,"toolInputStreamMs":500,"message":{"id":"msg_x","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text","text":"你好"}],"usage":{"input_tokens":10,"output_tokens":30}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        let row = read_reply(&conn, "msg_x");
        assert_eq!(
            row.first_token_at,
            Some(ms("2026-06-12T12:00:03.000Z") - 500),
            "文字首字来自 toolInputStreamMs"
        );
        assert_eq!(row.time_to_first_token_ms, Some(1_500.0));
        assert_eq!(row.completed_at, Some(ms("2026-06-12T12:00:03.000Z")));
        assert_eq!(row.status.as_deref(), Some("completed"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 父链回退（无 api-request 行 / 无新版计时字段的旧版本）：起点沿
    /// parentUuid 向上、跳过 attachment 行取第一条其它行的时间戳，
    /// timing_estimated = 1（界面带 ≈）；无首个输出证据 → first_token_at 为空。
    #[test]
    fn parent_chain_fallback_skips_attachment_and_marks_estimated() {
        let (tmp, mut conn) = setup_temp_db("parent");
        let file = write_session_lines(
            &tmp,
            "sess-parent.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-12T13:00:00.000Z","type":"user","uuid":"u1","cwd":"/tmp/proj-b","message":{"role":"user"}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T13:00:00.300Z","type":"attachment","uuid":"att1","parentUuid":"u1"}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T13:00:02.000Z","type":"assistant","uuid":"a1","parentUuid":"att1","message":{"id":"msg_p","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":50,"output_tokens":10}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        let row = read_reply(&conn, "msg_p");
        assert_eq!(
            row.request_started_at,
            Some(ms("2026-06-12T13:00:00.000Z")),
            "起点跳过 attachment 取 user 行时间戳"
        );
        assert_eq!(row.timing_estimated, Some(1), "父链起点是推断值");
        assert_eq!(row.first_token_at, None, "无首个输出证据不落首字");
        assert_eq!(row.time_to_first_token_ms, None);
        assert_eq!(row.duration_ms, Some(2_000.0), "耗时 = 结束 − 推断起点");
        assert_eq!(row.status.as_deref(), Some("completed"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 流式回复跨两次导入：第一次文件停在流中（status NULL）；第二次追加
    /// 末块（总量更大、带 stop_reason）后重放，状态由空补成 completed，
    /// token / completed_at 更新，updated_at 打修订标记。
    #[test]
    fn streaming_reply_completed_across_two_imports() {
        let (tmp, mut conn) = setup_temp_db("stream");
        let head = concat!(
            r#"{"timestamp":"2026-06-12T14:00:02.000Z","type":"assistant","apiBlockIndex":0,"message":{"id":"msg_s","model":"glm-5.3","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":1}}}"#,
            "\n",
        );
        let tail = concat!(
            r#"{"timestamp":"2026-06-12T14:00:05.000Z","type":"assistant","apiBlockIndex":1,"message":{"id":"msg_s","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":40}}}"#,
            "\n",
        );
        let file = write_session_lines(&tmp, "sess-stream.jsonl", head);
        run_import(&mut conn, &file);
        let first = read_reply(&conn, "msg_s");
        assert_eq!(first.status, None, "文件停在流式中间 → 状态未知");
        assert_eq!(first.computed_total, 11);
        assert_eq!(first.completed_at, Some(ms("2026-06-12T14:00:02.000Z")));
        let updated_first = read_updated_at(&conn, "msg_s");

        // 追加末块后重放（updated_at 毫秒粒度，稍等保证可观测变化）
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file, format!("{head}{tail}")).expect("追加末块失败");
        run_import(&mut conn, &file);

        let second = read_reply(&conn, "msg_s");
        assert_eq!(second.computed_total, 50, "总量更大者覆盖");
        assert_eq!(second.status.as_deref(), Some("completed"), "状态由空补全");
        assert_eq!(second.completed_at, Some(ms("2026-06-12T14:00:05.000Z")));
        assert_ne!(
            read_updated_at(&conn, "msg_s"),
            updated_first,
            "内容变化必须打修订标记（同步补传）"
        );
        assert_eq!(count_rows(&conn), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 最终失败的回复入库为 0 token 行：status=error、error_code 取
    /// apiErrorStatus（缺失降级 "error"）、模型名取 requestedModel（缺失回退
    /// 本文件最近一次真实模型名）、<synthetic> 不再被跳过。
    #[test]
    fn error_reply_persists_as_zero_token_row() {
        let (tmp, mut conn) = setup_temp_db("error");
        let file = write_session_lines(
            &tmp,
            "sess-error.jsonl",
            concat!(
                // 前一条真实回复：让本文件有「最近真实模型名」可用
                r#"{"timestamp":"2026-06-12T15:00:00.000Z","type":"assistant","message":{"id":"msg_ok","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":5,"output_tokens":5}}}"#,
                "\n",
                // 一次失败尝试（重试行）
                r#"{"timestamp":"2026-06-12T15:01:00.000Z","type":"system","subtype":"api_error","error":{"status":429,"retryAttempt":1,"maxRetries":3,"source":"api"}}"#,
                "\n",
                // 最终失败的回复：<synthetic> 模型、用量全 0
                r#"{"timestamp":"2026-06-12T15:01:01.000Z","type":"assistant","uuid":"e1","isApiErrorMessage":true,"apiErrorStatus":429,"requestedModel":"glm-9","error":"API Error: 429","message":{"id":"msg_err","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#,
                "\n",
                // 带错误码的失败回复之后再来一条无 apiErrorStatus 的（降级 "error"）
                r#"{"timestamp":"2026-06-12T15:02:01.000Z","type":"assistant","uuid":"e2","isApiErrorMessage":true,"error":"API Error: overloaded","message":{"id":"msg_err2","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);

        let row = read_reply(&conn, "msg_err");
        assert_eq!(row.computed_total, 0, "失败回复入库为 0 token 行");
        assert_eq!(row.input_tokens, 0);
        assert_eq!(row.output_tokens, 0);
        assert_eq!(row.status.as_deref(), Some("error"));
        assert_eq!(row.error_code.as_deref(), Some("429"));
        assert_eq!(row.model_id, "glm-9", "模型名取 requestedModel");
        assert_eq!(row.started_at, ms("2026-06-12T15:01:01.000Z"));
        assert_eq!(row.retry_count, 1, "错误回复之前的 api_error 记入重试");
        // 成功回复的重试码优先级不受影响（自己的 apiErrorStatus 只属于失败回复）

        let row2 = read_reply(&conn, "msg_err2");
        assert_eq!(row2.error_code.as_deref(), Some("error"), "无状态码降级为 error");
        assert_eq!(row2.model_id, "glm-5.3", "模型名回退最近一次真实模型名");
        assert_eq!(row2.retry_count, 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 带重试的成功回复：两条回复之间的 api_error 归给后一条回复
    ///（retry_count 计数、error_code 取最后一条的状态码，status 仍 completed）。
    #[test]
    fn retried_reply_counts_attempts_and_keeps_last_error_code() {
        let (tmp, mut conn) = setup_temp_db("retry");
        let file = write_session_lines(
            &tmp,
            "sess-retry.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-12T16:00:00.000Z","type":"assistant","message":{"id":"msg_prev","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":5,"output_tokens":5}}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T16:01:00.000Z","type":"system","subtype":"api_error","error":{"status":429,"retryAttempt":1,"maxRetries":3}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T16:01:02.000Z","type":"system","subtype":"api_error","error":{"status":500,"retryAttempt":2,"maxRetries":3}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T16:01:04.000Z","type":"assistant","message":{"id":"msg_r","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":20,"output_tokens":30}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        let row = read_reply(&conn, "msg_r");
        assert_eq!(row.retry_count, 2, "两条 api_error 归给本条回复");
        assert_eq!(row.error_code.as_deref(), Some("500"), "取最后一条的状态码");
        assert_eq!(row.status.as_deref(), Some("completed"), "重试后成功仍是 completed");
        // 前一条回复不受影响
        let prev = read_reply(&conn, "msg_prev");
        assert_eq!(prev.retry_count, 0);
        assert_eq!(prev.error_code, None);
        let _ = std::fs::remove_dir_all(&tmp);
    }


    /// 全量重放完全相同的文件：不产生任何写入（幂等，行数与 updated_at 不变）。
    #[test]
    fn replay_without_change_skips_write() {
        let (tmp, mut conn) = setup_temp_db("replay");
        let file = write_session_lines(
            &tmp,
            "sess-replay.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-12T18:00:00.000Z","type":"api-request","id":"req-9"}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T18:00:03.000Z","type":"assistant","requestRef":"req-9","apiBlockIndex":0,"toolInputStreamMs":700,"message":{"id":"msg_re","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":30}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        let first = read_reply(&conn, "msg_re");
        let updated_first = read_updated_at(&conn, "msg_re");
        std::thread::sleep(std::time::Duration::from_millis(5));
        run_import(&mut conn, &file);
        assert_eq!(read_reply(&conn, "msg_re"), first, "重放不改变任何取证值");
        assert_eq!(read_updated_at(&conn, "msg_re"), updated_first);
        assert_eq!(count_rows(&conn), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 计时回填标记只触发一次：新库 / 旧库 user_version=0 时判定需要，
    /// 写入标记后不再触发（重放幂等的前提下避免每轮全量扫描）。
    #[test]
    fn timing_backfill_runs_once() {
        let (_tmp, mut conn) = setup_temp_db("backfill");
        // SCHEMA_SQL 建的新库 user_version 仍为 0（标记语义与表结构无关）
        assert!(
            timing_backfill_pending(&conn),
            "升级前 user_version < 2 应判定需要回填"
        );
        mark_timing_backfill_done(&mut conn);
        assert!(
            !timing_backfill_pending(&conn),
            "写入标记后不再触发"
        );
        // 幂等：重复写标记无害
        mark_timing_backfill_done(&mut conn);
        assert!(!timing_backfill_pending(&conn));
    }

    /// 旧结构库经 ensure_timing_columns 补列后，与新库（SCHEMA_SQL）的列集合
    /// 完全一致（顺序无关），retry_count 带 NOT NULL DEFAULT 0 兜底。
    #[test]
    fn ensure_timing_columns_matches_new_schema() {
        let legacy = Connection::open_in_memory().expect("打开内存库失败");
        // 升级前的旧结构：无 8 个计时状态列
        legacy
            .execute_batch(
                "CREATE TABLE model_usage (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL,
                    dedupe_key TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    model_id TEXT NOT NULL DEFAULT '',
                    provider_id TEXT NOT NULL DEFAULT 'claude',
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                    duration_ms REAL,
                    updated_at INTEGER NOT NULL DEFAULT 0,
                    cwd TEXT,
                    project_key TEXT,
                    UNIQUE(dedupe_key)
                 );
                 INSERT INTO model_usage (session_id, dedupe_key, started_at, model_id)
                 VALUES ('s-old', 'd-old', 1000, 'glm-5.3');",
            )
            .expect("建旧结构表失败");

        ensure_timing_columns(&legacy).expect("补列迁移失败");
        // 幂等：重复执行不报错
        ensure_timing_columns(&legacy).expect("重复补列应幂等");

        let columns = |conn: &Connection| -> Vec<String> {
            let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('model_usage')").unwrap();
            let mut names: Vec<String> = stmt
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            names.sort();
            names
        };
        let (_tmp, fresh_conn) = setup_temp_db("schema");
        assert_eq!(columns(&legacy), columns(&fresh_conn), "补列后列集合应与新库一致");
        // 旧行的 retry_count 有默认值兜底、其余列为 NULL
        let row = read_reply(&legacy, "d-old");
        assert_eq!(row.retry_count, 0);
        assert_eq!(row.request_started_at, None);
        assert_eq!(row.status, None);
    }

    /// 续读（start_offset > 0）：跨游标的回复由回看区重建父链后正确合并，
    /// 只有落在新增字节里的块才触发写库；回看区不产生额外行。
    #[test]
    fn resume_merges_reply_across_cursor() {
        let (tmp, mut conn) = setup_temp_db("resume");
        let head = concat!(
            r#"{"timestamp":"2026-06-12T19:00:00.000Z","type":"user","uuid":"u1","cwd":"/tmp/proj-c","message":{"role":"user"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T19:00:02.000Z","type":"assistant","uuid":"a1","parentUuid":"u1","apiBlockIndex":0,"message":{"id":"msg_c","model":"glm-5.3","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":2}}}"#,
            "\n",
        );
        let tail = concat!(
            r#"{"timestamp":"2026-06-12T19:00:06.000Z","type":"assistant","uuid":"a2","message":{"id":"msg_c","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":40}}}"#,
            "\n",
        );
        let file = write_session_lines(&tmp, "sess-resume.jsonl", head);
        let offset = head.len() as u64; // 游标 = 前半部分末尾（行边界）
        run_import(&mut conn, &file);
        let first = read_reply(&conn, "msg_c");
        assert_eq!(first.status, None);
        assert_eq!(
            first.request_started_at,
            Some(ms("2026-06-12T19:00:00.000Z")),
            "首帧父链起点已可推导"
        );

        // 追加末块后按游标续读：块 1 在回看区、块 2 在新增区
        std::fs::write(&file, format!("{head}{tail}")).expect("追加末块失败");
        let tx = conn.transaction().expect("开启续读事务失败");
        import_one_file(&tx, &file, Some((offset, offset))).expect("续读失败");
        tx.commit().expect("提交续读事务失败");

        let second = read_reply(&conn, "msg_c");
        assert_eq!(second.computed_total, 50, "跨游标回复正确合并");
        assert_eq!(second.status.as_deref(), Some("completed"));
        assert_eq!(second.completed_at, Some(ms("2026-06-12T19:00:06.000Z")));
        assert_eq!(
            second.request_started_at,
            Some(ms("2026-06-12T19:00:00.000Z")),
            "回看区重建的父链仍能推导起点"
        );
        assert_eq!(second.timing_estimated, Some(1));
        assert_eq!(count_rows(&conn), 1, "回看区不产生额外行");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// merge_reply 的合并契约（纯函数）。总量相等时（同一条响应分趟落盘，
    /// 读到了更多块）completed_at 一律取两者最大值；duration 随 completed_at
    /// 或起点的变化用新值（都没变才保留已入库值）。权威重放且新记录有起点：
    /// 起点四列整体用新值（可清掉旧值）；续读、或新记录无起点：旧值为空才
    /// 补。总量更大 / 更小的分支不变。
    #[test]
    fn merge_reply_authoritative_and_incremental_contract() {
        // 库里旧值：估算起点、错误状态、错误归属的 retry_count
        let stored = StoredReply {
            started_at: 1000,
            model_id: "glm-5.3".into(),
            input_tokens: 30,
            output_tokens: 30,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            computed_total: 30,
            duration_ms: Some(5_000.0),
            request_started_at: Some(900),
            first_token_at: None,
            completed_at: Some(2000),
            time_to_first_token_ms: None,
            timing_estimated: Some(1),
            status: Some("error".into()),
            error_code: Some("429".into()),
            retry_count: 5,
        };
        // 整文件重放的新证据：精确起点、正确状态与 retry、更晚的结束时刻
        //（总量相同 = 同一条响应读到了更多块）
        let fresh = StoredReply {
            started_at: stored.started_at,
            model_id: stored.model_id.clone(),
            input_tokens: stored.input_tokens,
            output_tokens: stored.output_tokens,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            computed_total: 30,
            duration_ms: Some(1_000.0),
            request_started_at: Some(800),
            first_token_at: None,
            completed_at: Some(3000),
            time_to_first_token_ms: None,
            timing_estimated: Some(0),
            status: Some("completed".into()),
            error_code: None,
            retry_count: 1,
        };

        // 权威重放 + 总量相等（更多块）：completed_at 取最大值，duration 随
        // 变化用新值；新记录有起点 → 起点四列整体用新值（可清旧值，这里旧
        // 值本就为空）
        let merged = merge_reply(&stored, &fresh, true);
        assert_eq!(merged.request_started_at, Some(800), "精确起点覆盖估算起点");
        assert_eq!(merged.timing_estimated, Some(0));
        assert_eq!(merged.retry_count, 1, "错误的 retry_count 被纠正");
        assert_eq!(merged.status.as_deref(), Some("completed"));
        assert_eq!(
            merged.duration_ms,
            Some(1_000.0),
            "completed_at 与起点都变化时 duration 用新值"
        );
        assert_eq!(
            merged.completed_at,
            Some(3000),
            "总量相等时 completed_at 取两者最大值"
        );
        // token 各列仍按「总量更大者覆盖」：总量相同 → 不动
        assert_eq!(merged.input_tokens, stored.input_tokens);
        assert_eq!(merged.started_at, stored.started_at);
        // 新值为空的列保留旧值
        assert_eq!(merged.error_code.as_deref(), Some("429"), "新值为空保留旧值");

        // 总量相等 + 只有起点被纠正（completed 没变）→ duration 仍用新值
        //（与精确起点对上）
        let mut corrected_start_only = fresh.clone();
        corrected_start_only.completed_at = stored.completed_at;
        let merged = merge_reply(&stored, &corrected_start_only, true);
        assert_eq!(merged.request_started_at, Some(800), "起点被纠正");
        assert_eq!(
            merged.duration_ms,
            Some(1_000.0),
            "只有起点变化时 duration 也用新值"
        );
        // completed 与起点都没变 → duration 保留已入库值
        let mut identical = fresh.clone();
        identical.completed_at = stored.completed_at;
        identical.request_started_at = stored.request_started_at;
        let merged = merge_reply(&stored, &identical, true);
        assert_eq!(
            merged.duration_ms,
            Some(5_000.0),
            "completed 与起点都没变时 duration 保留已入库值"
        );
        assert_eq!(merged.completed_at, Some(2000));
        assert_eq!(merged.request_started_at, Some(900));

        // 权威重放 + 总量相等 + 新记录无起点（旧版本文件）：起点四列只在
        // 旧值为空时补（不因无起点清掉旧值）
        let mut no_start = fresh.clone();
        no_start.request_started_at = None;
        no_start.timing_estimated = None;
        let merged = merge_reply(&stored, &no_start, true);
        assert_eq!(merged.request_started_at, Some(900), "新记录无起点 → 旧值非空不动");
        assert_eq!(merged.timing_estimated, Some(1));
        // 总量相等 → completed 仍取最大
        assert_eq!(merged.completed_at, Some(3000));

        // 权威重放 + 总量相等 + 新记录有起点但首字证据被判定无效（置空）：
        // 能清掉此前续读写入的非空 first_token_at
        let mut stored_with_bad_ttft = stored.clone();
        stored_with_bad_ttft.first_token_at = Some(500);
        stored_with_bad_ttft.time_to_first_token_ms = Some(400.0);
        let merged = merge_reply(&stored_with_bad_ttft, &fresh, true);
        assert_eq!(
            merged.first_token_at, None,
            "新证据判定首字无效时清掉旧值"
        );
        assert_eq!(merged.time_to_first_token_ms, None);

        // 权威重放 + 总量更小：9 列只在旧值为空时补、completed_at 不改
        let smaller = StoredReply {
            computed_total: 10,
            status: Some("completed".into()),
            ..fresh.clone()
        };
        let merged = merge_reply(&stored, &smaller, true);
        assert_eq!(merged.request_started_at, Some(900), "旧值非空不动");
        assert_eq!(merged.status.as_deref(), Some("error"), "旧值非空不动");
        assert_eq!(merged.completed_at, Some(2000), "completed_at 不改");
        assert_eq!(merged.retry_count, 5, "retry 取较大者");
        // token 各列不动（新总量更小）
        assert_eq!(merged.computed_total, 30);

        // 已入库的 model_id 为空（模型名回退修正生效前的失败行）而新记录
        // 非空：等总量与总量更小两种情况下都补上
        let mut no_model = stored.clone();
        no_model.model_id = String::new();
        let merged = merge_reply(&no_model, &fresh, true);
        assert_eq!(
            merged.model_id, "glm-5.3",
            "等总量时空模型名用新值补上"
        );
        let merged = merge_reply(&no_model, &smaller, true);
        assert_eq!(
            merged.model_id, "glm-5.3",
            "总量更小时空模型名用新值补上"
        );
        // 非空旧值保持不变（前面各段已断言 merged.model_id == "glm-5.3"）

        // 权威重放 + 总量更大：9 列新值非空即覆盖（含 duration）
        let bigger = StoredReply {
            computed_total: 60,
            duration_ms: Some(1_000.0),
            completed_at: Some(3000),
            ..fresh.clone()
        };
        let merged = merge_reply(&stored, &bigger, true);
        assert_eq!(merged.duration_ms, Some(1_000.0), "总量更大时新证据覆盖");
        assert_eq!(merged.completed_at, Some(3000));
        assert_eq!(merged.retry_count, 1);

        // 非权威续读 + 总量相等（更多块）：completed_at 取最大值、duration
        // 用新值；起点四列旧值非空不动、retry 取较大者
        let merged = merge_reply(&stored, &fresh, false);
        assert_eq!(
            merged.completed_at,
            Some(3000),
            "续读同样把 completed_at 推进到最大值"
        );
        assert_eq!(merged.duration_ms, Some(1_000.0));
        assert_eq!(merged.request_started_at, Some(900), "续读不动旧起点");
        assert_eq!(merged.timing_estimated, Some(1));
        assert_eq!(merged.retry_count, 5, "续读取较大者");
        assert_eq!(merged.status.as_deref(), Some("completed"));

        // 非权威续读 + 总量相等 + completed 与起点都没变：duration 保留
        let merged = merge_reply(&stored, &identical, false);
        assert_eq!(merged.completed_at, Some(2000));
        assert_eq!(merged.duration_ms, Some(5_000.0));

        // model_id 空串不覆盖非空旧值（失败回复三级回退全部失败时 fresh
        // 仍为空串，不能把已入库的真实模型名抹掉）
        let mut empty_model = fresh.clone();
        empty_model.model_id = String::new();
        let merged = merge_reply(&stored, &empty_model, true);
        assert_eq!(merged.model_id, "glm-5.3", "空模型名不覆盖非空旧值");
        let merged = merge_reply(&stored, &empty_model, false);
        assert_eq!(merged.model_id, "glm-5.3");
    }

    /// 权威重放（回填 / 文件重写）纠正早先入库的错误证据：第一次导入没有
    /// api-request 行（父链估算起点 + 误归属的 retry_count），文件被改写后
    /// 从头重放，精确起点与正确 retry 覆盖旧值。
    #[test]
    fn authoritative_replay_overrides_stale_evidence() {
        let (tmp, mut conn) = setup_temp_db("authoritative");
        let stale = concat!(
            r#"{"timestamp":"2026-06-12T10:00:00.000Z","type":"user","uuid":"u1","cwd":"/tmp/proj-x","message":{"role":"user"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T10:00:01.000Z","type":"system","subtype":"api_error","error":{"status":429,"retryAttempt":1,"maxRetries":2}}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T10:00:02.000Z","type":"system","subtype":"api_error","error":{"status":429,"retryAttempt":2,"maxRetries":2}}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T10:00:05.000Z","type":"assistant","uuid":"a1","parentUuid":"u1","message":{"id":"msg_a","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":20,"output_tokens":10}}}"#,
            "\n",
        );
        let file = write_session_lines(&tmp, "sess-auth.jsonl", stale);
        run_import(&mut conn, &file);
        let stale_row = read_reply(&conn, "msg_a");
        assert_eq!(stale_row.timing_estimated, Some(1), "旧证据只能父链估算");
        assert_eq!(stale_row.retry_count, 2);
        assert_eq!(stale_row.duration_ms, Some(5_000.0));

        // 文件被改写（更早版本代码写不了 api-request 行的取证）：插入精确
        // 起点行、去掉一条误归属的 api_error，从头重放（known=None → 权威）
        let corrected = concat!(
            r#"{"timestamp":"2026-06-12T10:00:00.000Z","type":"user","uuid":"u1","cwd":"/tmp/proj-x","message":{"role":"user"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T10:00:04.000Z","type":"api-request","id":"req-x"}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T10:00:01.000Z","type":"system","subtype":"api_error","error":{"status":429,"retryAttempt":1,"maxRetries":2}}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T10:00:05.000Z","type":"assistant","uuid":"a1","parentUuid":"u1","requestRef":"req-x","message":{"id":"msg_a","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":20,"output_tokens":10}}}"#,
            "\n",
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file, corrected).expect("改写文件失败");
        run_import(&mut conn, &file);

        let row = read_reply(&conn, "msg_a");
        assert_eq!(row.request_started_at, Some(ms("2026-06-12T10:00:04.000Z")), "精确起点覆盖估算起点");
        assert_eq!(row.timing_estimated, Some(0));
        assert_eq!(row.retry_count, 1, "误归属的 retry_count 被纠正");
        // 总量相等但起点被纠正（估算 → 精确）：duration 的更新条件包含起点
        // 变化 → 改用按精确起点算出的新值（不再包含重试等待）
        assert_eq!(row.duration_ms, Some(1_000.0));
        assert_eq!(count_rows(&conn), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 请求起点缺失时首个输出时刻不丢（流式速度只依赖首字与结束两个证据）：
    /// 无 requestRef、父链也查不到 → 起点 / ttft / 估算标记为空，
    /// first_token_at 照常落库。
    #[test]
    fn first_output_evidence_kept_without_request_start() {
        let (tmp, mut conn) = setup_temp_db("orphan-ft");
        let file = write_session_lines(
            &tmp,
            "sess-orphan.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-12T20:00:03.000Z","type":"assistant","apiBlockIndex":0,"toolInputStreamMs":800,"message":{"id":"msg_o","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":30}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        let row = read_reply(&conn, "msg_o");
        assert_eq!(row.request_started_at, None, "无 requestRef、无父链 → 起点为空");
        assert_eq!(row.timing_estimated, None);
        assert_eq!(
            row.first_token_at,
            Some(ms("2026-06-12T20:00:03.000Z") - 800),
            "起点缺失不丢首个输出时刻"
        );
        assert_eq!(row.time_to_first_token_ms, None, "无起点不给 ttft");
        assert_eq!(row.duration_ms, None, "无起点不给派生耗时");
        assert_eq!(row.completed_at, Some(ms("2026-06-12T20:00:03.000Z")));
        assert_eq!(row.status.as_deref(), Some("completed"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 失败回复的请求起点取最近一条 api-request 行（重试时是最后一次尝试
    /// 的发起时刻，耗时不含重试等待）；即使父链可达也不走父链回退。
    #[test]
    fn error_reply_uses_latest_api_request_as_start() {
        let (tmp, mut conn) = setup_temp_db("error-start");
        let file = write_session_lines(
            &tmp,
            "sess-error-start.jsonl",
            concat!(
                // 父链可达的 user 行（更早的时间戳）：若误走父链起点会是它
                r#"{"timestamp":"2026-06-12T15:00:00.000Z","type":"user","uuid":"u1","cwd":"/tmp/proj-e","message":{"role":"user"}}"#,
                "\n",
                // 第一次尝试（失败）
                r#"{"timestamp":"2026-06-12T16:00:00.000Z","type":"api-request","id":"req-e1"}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T16:00:01.000Z","type":"system","subtype":"api_error","error":{"status":429,"retryAttempt":1,"maxRetries":2}}"#,
                "\n",
                // 第二次尝试（也失败，最终写出错误回复）
                r#"{"timestamp":"2026-06-12T16:00:10.000Z","type":"api-request","id":"req-e2"}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T16:00:12.000Z","type":"assistant","uuid":"e1","parentUuid":"u1","isApiErrorMessage":true,"apiErrorStatus":429,"requestedModel":"glm-5.3","error":"API Error: 429","message":{"id":"msg_e","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        let row = read_reply(&conn, "msg_e");
        assert_eq!(
            row.request_started_at,
            Some(ms("2026-06-12T16:00:10.000Z")),
            "起点取最近一条 api-request 行（不走父链）"
        );
        assert_eq!(row.timing_estimated, Some(0));
        assert_eq!(row.duration_ms, Some(2_000.0), "耗时不含第一次尝试的重试等待");
        assert_eq!(row.completed_at, Some(ms("2026-06-12T16:00:12.000Z")));
        assert_eq!(row.status.as_deref(), Some("error"));
        assert_eq!(row.error_code.as_deref(), Some("429"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 旧版本没有 api-request 行：错误回复起点为空（不走父链回退，避免把
    /// 重试等待算进耗时），token 与状态照常落库。
    #[test]
    fn error_reply_without_api_request_has_no_start() {
        let (tmp, mut conn) = setup_temp_db("error-nostart");
        let file = write_session_lines(
            &tmp,
            "sess-error-nostart.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-12T15:00:00.000Z","type":"user","uuid":"u1","message":{"role":"user"}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T16:00:02.000Z","type":"assistant","uuid":"e1","parentUuid":"u1","isApiErrorMessage":true,"apiErrorStatus":500,"requestedModel":"glm-5.3","error":"API Error: 500","message":{"id":"msg_e2","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        let row = read_reply(&conn, "msg_e2");
        assert_eq!(row.request_started_at, None, "无 api-request 行起点为空");
        assert_eq!(row.timing_estimated, None);
        assert_eq!(row.duration_ms, None);
        assert_eq!(row.completed_at, Some(ms("2026-06-12T16:00:02.000Z")));
        assert_eq!(row.status.as_deref(), Some("error"));
        assert_eq!(row.error_code.as_deref(), Some("500"));
        let _ = std::fs::remove_dir_all(&tmp);
    }


    /// 新版每块行都带最终用量：首块先落盘入库（completed_at = 首块时间戳），
    /// 后续块追加后续读（总量相同、首块时间戳不变 = 同一条回复读到了更多
    /// 块）→ completed_at 推进到末块、duration 随之更新、updated_at 变化；
    /// 第三次全量重放无变化不写。
    #[test]
    fn more_blocks_resume_advances_completed_at() {
        let (tmp, mut conn) = setup_temp_db("more-blocks");
        let head = concat!(
            r#"{"timestamp":"2026-06-12T21:00:00.000Z","type":"api-request","id":"req-m"}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T21:00:03.000Z","type":"assistant","requestRef":"req-m","apiBlockIndex":0,"message":{"id":"msg_m","model":"glm-5.3","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#,
            "\n",
        );
        let tail = r#"{"timestamp":"2026-06-12T21:00:07.000Z","type":"assistant","requestRef":"req-m","apiBlockIndex":1,"message":{"id":"msg_m","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#;
        let file = write_session_lines(&tmp, "sess-more.jsonl", head);
        let offset = head.len() as u64;
        run_import(&mut conn, &file);
        let first = read_reply(&conn, "msg_m");
        assert_eq!(first.completed_at, Some(ms("2026-06-12T21:00:03.000Z")));
        assert_eq!(first.duration_ms, Some(3_000.0));
        assert_eq!(first.status, None);
        let updated_first = read_updated_at(&conn, "msg_m");

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file, format!("{head}{tail}\n")).expect("追加末块失败");
        let tx = conn.transaction().expect("开启续读事务失败");
        import_one_file(&tx, &file, Some((offset, offset))).expect("续读失败");
        tx.commit().expect("提交续读事务失败");

        let second = read_reply(&conn, "msg_m");
        assert_eq!(
            second.completed_at,
            Some(ms("2026-06-12T21:00:07.000Z")),
            "总量相同、首块时间戳不变 → completed_at 推进到末块"
        );
        assert_eq!(
            second.duration_ms,
            Some(7_000.0),
            "completed_at 变化 → duration 用新值"
        );
        assert_eq!(second.status.as_deref(), Some("completed"), "末块带 stop_reason");
        assert_ne!(
            read_updated_at(&conn, "msg_m"),
            updated_first,
            "内容变化打修订标记"
        );
        assert_eq!(count_rows(&conn), 1);

        // 第三次全量重放：证据完全一致 → 不写、updated_at 不变
        std::thread::sleep(std::time::Duration::from_millis(5));
        let updated_second = read_updated_at(&conn, "msg_m");
        run_import(&mut conn, &file);
        assert_eq!(
            read_reply(&conn, "msg_m").completed_at,
            Some(ms("2026-06-12T21:00:07.000Z"))
        );
        assert_eq!(
            read_updated_at(&conn, "msg_m"),
            updated_second,
            "重放无变化不动 updated_at"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 权威重放时新记录有起点、首字被判定无效（不晚于精确起点 → 置空）：
    /// 能清掉此前续读 / 旧代码写入的非空 first_token_at。
    #[test]
    fn authoritative_replay_clears_invalid_first_token() {
        let (tmp, mut conn) = setup_temp_db("clear-ft");
        // 第一次导入：无 api-request 行（父链起点更早），首字证据有效入库
        let stale = concat!(
            r#"{"timestamp":"2026-06-12T22:00:00.000Z","type":"user","uuid":"u1","cwd":"/tmp/proj-z","message":{"role":"user"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T22:00:03.000Z","type":"assistant","uuid":"a1","parentUuid":"u1","toolInputStreamMs":2000,"message":{"id":"msg_z","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#,
            "\n",
        );
        let file = write_session_lines(&tmp, "sess-clear.jsonl", stale);
        run_import(&mut conn, &file);
        let first = read_reply(&conn, "msg_z");
        assert_eq!(
            first.first_token_at,
            Some(ms("2026-06-12T22:00:03.000Z") - 2_000),
            "父链起点下首字证据有效"
        );
        assert_eq!(first.time_to_first_token_ms, Some(1_000.0));

        // 重放：出现 api-request 行，精确起点（22:00:02.500）晚于原首字
        //（22:00:01）→ finalize 判定首字无效置空；权威重放 + 新记录有起点
        // → 起点四列整体用新值，清掉已入库的错值
        let corrected = concat!(
            r#"{"timestamp":"2026-06-12T22:00:00.000Z","type":"user","uuid":"u1","cwd":"/tmp/proj-z","message":{"role":"user"}}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T22:00:02.500Z","type":"api-request","id":"req-z"}"#,
            "\n",
            r#"{"timestamp":"2026-06-12T22:00:03.000Z","type":"assistant","uuid":"a1","parentUuid":"u1","requestRef":"req-z","toolInputStreamMs":2000,"message":{"id":"msg_z","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#,
            "\n",
        );
        std::fs::write(&file, corrected).expect("改写文件失败");
        run_import(&mut conn, &file);

        let row = read_reply(&conn, "msg_z");
        assert_eq!(
            row.request_started_at,
            Some(ms("2026-06-12T22:00:02.500Z")),
            "精确起点覆盖父链起点"
        );
        assert_eq!(
            row.first_token_at, None,
            "首字不晚于起点被判定无效，清掉已入库值"
        );
        assert_eq!(row.time_to_first_token_ms, None);
        assert_eq!(count_rows(&conn), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 一次响应的块之间夹着 user 工具结果行（客户端边收流边执行工具）：
    /// 工具结果行不结束回复，idx0 思考块 / idx1、idx2 工具块进同一条记录；
    /// 首字来自 idx0 的 thinkingDurationMs，completed_at 是全部块时间戳的
    /// 最大值（idx2），耗时 = 结束 − 起点（api-request 行）。
    #[test]
    fn tool_result_between_blocks_keeps_one_reply() {
        let (tmp, mut conn) = setup_temp_db("tool-result");
        let file = write_session_lines(
            &tmp,
            "sess-tool-result.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-12T23:00:00.000Z","type":"api-request","id":"req-t"}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T23:00:02.000Z","type":"assistant","requestRef":"req-t","apiBlockIndex":0,"thinkingDurationMs":1500,"message":{"id":"msg_w","model":"glm-5.3","content":[{"type":"thinking","thinking":"……"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T23:00:04.000Z","type":"assistant","requestRef":"req-t","apiBlockIndex":1,"message":{"id":"msg_w","model":"glm-5.3","content":[{"type":"tool_use","name":"Bash"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T23:00:05.000Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#,
                "\n",
                r#"{"timestamp":"2026-06-12T23:00:08.000Z","type":"assistant","requestRef":"req-t","apiBlockIndex":2,"message":{"id":"msg_w","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"tool_use","name":"Edit"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        assert_eq!(count_rows(&conn), 1, "夹工具结果的全部块进同一条记录");
        let row = read_reply(&conn, "msg_w");
        assert_eq!(
            row.first_token_at,
            Some(ms("2026-06-12T23:00:02.000Z") - 1_500),
            "首字来自 idx0 思考块"
        );
        assert_eq!(
            row.completed_at,
            Some(ms("2026-06-12T23:00:08.000Z")),
            "结束时刻取 idx2 块的时间戳"
        );
        assert_eq!(row.request_started_at, Some(ms("2026-06-12T23:00:00.000Z")));
        assert_eq!(row.timing_estimated, Some(0));
        assert_eq!(row.duration_ms, Some(8_000.0), "耗时 = 结束 − 起点");
        assert_eq!(row.time_to_first_token_ms, Some(500.0));
        assert_eq!(row.computed_total, 30);
        assert_eq!(row.status.as_deref(), Some("completed"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 分两趟导入夹工具结果的响应：第一趟只到工具结果行（completed 停在
    /// 已落盘的末块）；第二趟追加 idx2 块后续读 → completed_at 推进、首字
    /// 与起点不变、duration 更新、updated_at 变化；第三趟无新增不写。
    #[test]
    fn tool_result_reply_advances_across_two_imports() {
        let (tmp, mut conn) = setup_temp_db("tool-result-resume");
        let head = concat!(
            r#"{"timestamp":"2026-06-13T01:00:00.000Z","type":"api-request","id":"req-u"}"#,
            "\n",
            r#"{"timestamp":"2026-06-13T01:00:02.000Z","type":"assistant","requestRef":"req-u","apiBlockIndex":0,"thinkingDurationMs":1500,"message":{"id":"msg_u","model":"glm-5.3","content":[{"type":"thinking"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#,
            "\n",
            r#"{"timestamp":"2026-06-13T01:00:04.000Z","type":"user","message":{"role":"user","content":[{"type":"tool_result"}]}}"#,
            "\n",
        );
        let tail = r#"{"timestamp":"2026-06-13T01:00:08.000Z","type":"assistant","requestRef":"req-u","apiBlockIndex":1,"message":{"id":"msg_u","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"tool_use"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#;
        let file = write_session_lines(&tmp, "sess-tool-resume.jsonl", head);
        let offset = head.len() as u64;
        run_import(&mut conn, &file);
        let first = read_reply(&conn, "msg_u");
        assert_eq!(first.completed_at, Some(ms("2026-06-13T01:00:02.000Z")));
        assert_eq!(
            first.first_token_at,
            Some(ms("2026-06-13T01:00:02.000Z") - 1_500)
        );
        assert_eq!(first.request_started_at, Some(ms("2026-06-13T01:00:00.000Z")));
        let updated_first = read_updated_at(&conn, "msg_u");

        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(&file, format!("{head}{tail}\n")).expect("追加 idx2 块失败");
        let tx = conn.transaction().expect("开启续读事务失败");
        import_one_file(&tx, &file, Some((offset, offset))).expect("续读失败");
        tx.commit().expect("提交续读事务失败");

        let second = read_reply(&conn, "msg_u");
        assert_eq!(
            second.completed_at,
            Some(ms("2026-06-13T01:00:08.000Z")),
            "completed_at 推进到 idx2 块的时间戳"
        );
        assert_eq!(
            second.first_token_at,
            Some(ms("2026-06-13T01:00:02.000Z") - 1_500),
            "首字不变"
        );
        assert_eq!(
            second.request_started_at,
            Some(ms("2026-06-13T01:00:00.000Z")),
            "起点不变"
        );
        assert_eq!(second.duration_ms, Some(8_000.0), "duration 随结束时刻更新");
        assert_ne!(
            read_updated_at(&conn, "msg_u"),
            updated_first,
            "内容变化打修订标记"
        );
        assert_eq!(count_rows(&conn), 1);

        // 第三趟全量重放：证据一致 → 不写、updated_at 不变
        std::thread::sleep(std::time::Duration::from_millis(5));
        let updated_second = read_updated_at(&conn, "msg_u");
        run_import(&mut conn, &file);
        assert_eq!(read_updated_at(&conn, "msg_u"), updated_second, "重放无变化不写");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 回复划分的结束条件回归：user 工具结果行不结束当前回复；真正的用户
    /// 输入行（content 首块是 text）结束；api-request 行（新请求开始）结束
    /// ——结束后的回复 status 由「读到了后续行」判定为 completed（末块无
    /// stop_reason 也成立）。
    #[test]
    fn reply_split_boundaries_user_and_api_request() {
        let (tmp, mut conn) = setup_temp_db("split");
        let file = write_session_lines(
            &tmp,
            "sess-split.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-13T02:00:01.000Z","type":"api-request","id":"req-s1"}"#,
                "\n",
                // msg_x：无 stop_reason 的块
                r#"{"timestamp":"2026-06-13T02:00:02.000Z","type":"assistant","requestRef":"req-s1","apiBlockIndex":0,"message":{"id":"msg_x","model":"glm-5.3","content":[{"type":"tool_use"}],"usage":{"input_tokens":10,"output_tokens":10}}}"#,
                "\n",
                // 工具结果：不结束 msg_x（completed 仍是它自己的时间戳）
                r#"{"timestamp":"2026-06-13T02:00:03.000Z","type":"user","message":{"role":"user","content":[{"type":"tool_result"}]}}"#,
                "\n",
                // 真正的用户输入：结束 msg_x
                r#"{"timestamp":"2026-06-13T02:00:04.000Z","type":"user","message":{"role":"user","content":[{"type":"text","text":"继续"}]}}"#,
                "\n",
                // msg_y：新回复，末块无 stop_reason
                r#"{"timestamp":"2026-06-13T02:00:05.000Z","type":"assistant","apiBlockIndex":0,"message":{"id":"msg_y","model":"glm-5.3","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":20}}}"#,
                "\n",
                // 新请求开始：结束 msg_y（status=completed：读到了后续行）
                r#"{"timestamp":"2026-06-13T02:00:06.000Z","type":"api-request","id":"req-s2"}"#,
                "\n",
                // msg_z：第三条回复
                r#"{"timestamp":"2026-06-13T02:00:08.000Z","type":"assistant","requestRef":"req-s2","apiBlockIndex":0,"message":{"id":"msg_z","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":30}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        assert_eq!(count_rows(&conn), 3, "三条回复各自独立成行");
        let x = read_reply(&conn, "msg_x");
        assert_eq!(
            x.completed_at,
            Some(ms("2026-06-13T02:00:02.000Z")),
            "工具结果行不推进 completed_at"
        );
        assert_eq!(
            x.status.as_deref(),
            Some("completed"),
            "真用户输入结束回复 → 读到了后续行"
        );
        let y = read_reply(&conn, "msg_y");
        assert_eq!(y.completed_at, Some(ms("2026-06-13T02:00:05.000Z")));
        assert_eq!(
            y.status.as_deref(),
            Some("completed"),
            "api-request 行结束回复 → 读到了后续行"
        );
        let z = read_reply(&conn, "msg_z");
        assert_eq!(z.completed_at, Some(ms("2026-06-13T02:00:08.000Z")));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 失败回复的模型名三级回退：本文件没有真实模型名（第一条 assistant 行
    /// 就是失败回复、行上也没有 requestedModel）时，依次取同项目、全库的
    /// 最近一条真实模型名；都取不到留空串，且空名不进清单（list_models_at）。
    #[test]
    fn error_reply_model_falls_back_through_library() {
        let (tmp, mut conn) = setup_temp_db("model-fallback");
        // 场景 C（对照）：空库上第一条回复就失败 → 无任何回退 → 空串，
        // 且空名不出现在清单里
        let orphan = write_session_lines(
            &tmp,
            "sess-orphan-fail.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-14T01:00:01.000Z","type":"assistant","uuid":"e0","cwd":"/tmp/proj-orphan","isApiErrorMessage":true,"error":"API Error: 429","message":{"id":"msg_orphan","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &orphan);
        let orphan_row = read_reply(&conn, "msg_orphan");
        assert_eq!(orphan_row.model_id, "", "无任何回退来源时保留空串");
        let models = list_models_at(&conn).expect("清单查询失败");
        assert!(
            models.iter().all(|m| !m.model_id.is_empty()),
            "空模型名不进清单"
        );

        // 场景 A（项目回退）：另一会话（同 cwd → 同 project_key）先有真实
        // 模型行，其后的失败回复回退到该项目最近的真实模型名
        let seeded = write_session_lines(
            &tmp,
            "sess-seed.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-14T02:00:00.000Z","type":"assistant","cwd":"/tmp/proj-a","message":{"id":"msg_seed","model":"glm-base","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":10,"output_tokens":10}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &seeded);
        let failing = write_session_lines(
            &tmp,
            "sess-fail-a.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-14T02:00:05.000Z","type":"assistant","uuid":"e1","cwd":"/tmp/proj-a","isApiErrorMessage":true,"apiErrorStatus":429,"error":"API Error: 429","message":{"id":"msg_fail_a","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &failing);
        let row = read_reply(&conn, "msg_fail_a");
        assert_eq!(
            row.model_id, "glm-base",
            "失败回复回退到同项目的最近真实模型名"
        );

        // 场景 B（全库回退）：项目里没有任何真实模型名的另一个会话失败回复
        // → 回退到全库最近的真实模型名
        let failing_b = write_session_lines(
            &tmp,
            "sess-fail-b.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-14T03:00:05.000Z","type":"assistant","uuid":"e2","cwd":"/tmp/proj-b","isApiErrorMessage":true,"apiErrorStatus":500,"error":"API Error: 500","message":{"id":"msg_fail_b","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &failing_b);
        let row = read_reply(&conn, "msg_fail_b");
        assert_eq!(
            row.model_id, "glm-base",
            "项目内无真实模型名时回退到全库最近一条"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 首字证据的畸形超大耗时（毫秒数超出 i64 时间戳范围）：不回绕、不
    /// panic（debug 构建下普通减法会溢出崩溃、整个文件导入失败），按没有
    /// 证据处理。
    #[test]
    fn first_output_evidence_rejects_absurd_durations() {
        let base = ReplyBlock {
            timestamp: Some(1_000_000_000_000),
            parent_uuid: None,
            model: Some("glm-5.3".into()),
            requested_model: None,
            real_duration_ms: None,
            stop_reason: None,
            usage: UsagePayload::default(),
            api_block_index: Some(0),
            request_ref: None,
            thinking_duration_ms: None,
            tool_input_stream_ms: None,
            first_block_type: Some("thinking".into()),
            is_api_error: false,
            api_error_status: None,
        };
        let mut huge = base.clone();
        huge.thinking_duration_ms = Some(9.0e18);
        assert_eq!(
            first_output_evidence(&huge),
            None,
            "超大思考耗时按没有证据处理"
        );
        let mut absurd = base.clone();
        absurd.thinking_duration_ms = Some(f64::MAX);
        assert_eq!(first_output_evidence(&absurd), None, "f64::MAX 不 panic");
        // 正常值不受影响
        let mut normal = base;
        normal.thinking_duration_ms = Some(1_500.0);
        assert_eq!(
            first_output_evidence(&normal),
            Some(1_000_000_000_000 - 1_500)
        );
    }

    /// 旧版本日志没有失败块：最终失败只留下 api_error 行，用户重新输入后
    /// 下一条回复与那次失败无关——重试行被真正的用户输入清空，不再归给
    /// 后面的回复（带失败块的新版本日志照常消耗，见 error_reply 测试）。
    #[test]
    fn retry_errors_cleared_by_real_user_input() {
        let (tmp, mut conn) = setup_temp_db("retry-clear");
        let file = write_session_lines(
            &tmp,
            "sess-retry-clear.jsonl",
            concat!(
                r#"{"timestamp":"2026-06-14T04:00:00.000Z","type":"assistant","message":{"id":"msg_ok","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":5,"output_tokens":5}}}"#,
                "\n",
                r#"{"timestamp":"2026-06-14T04:01:00.000Z","type":"system","subtype":"api_error","error":{"status":429,"retryAttempt":1,"maxRetries":2}}"#,
                "\n",
                // 用户重新输入（真正的用户输入行）：丢弃未归属的重试行
                r#"{"timestamp":"2026-06-14T04:02:00.000Z","type":"user","message":{"role":"user","content":[{"type":"text","text":"再试一次"}]}}"#,
                "\n",
                r#"{"timestamp":"2026-06-14T04:02:05.000Z","type":"assistant","message":{"id":"msg_after","model":"glm-5.3","stop_reason":"end_turn","content":[{"type":"text"}],"usage":{"input_tokens":20,"output_tokens":30}}}"#,
                "\n",
            ),
        );
        run_import(&mut conn, &file);
        let after = read_reply(&conn, "msg_after");
        assert_eq!(
            after.retry_count, 0,
            "重试行被真正的用户输入清空，不归给后面的回复"
        );
        assert_eq!(after.error_code, None);
        assert_eq!(after.status.as_deref(), Some("completed"));
        let ok = read_reply(&conn, "msg_ok");
        assert_eq!(ok.retry_count, 0, "更早的回复也不受影响");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 并发首启的重复列迁移容忍：两列场景中后到的 ALTER 会得到
    /// "duplicate column name" 错误，ensure_timing_columns 把它当成功处理
    ///（对已补列的库重复执行也必须幂等不报错）。
    #[test]
    fn ensure_timing_columns_tolerates_duplicate_column() {
        let (_tmp, conn) = setup_temp_db("dup-column");
        // 模拟并发进程已完成迁移：直接 ALTER 已存在的列 → 产生 duplicate
        // column 错误，且错误消息与实现里的匹配串一致
        let err = conn
            .execute("ALTER TABLE model_usage ADD COLUMN status TEXT", [])
            .expect_err("重复加列应报错");
        assert!(
            err.to_string()
                .to_ascii_lowercase()
                .contains("duplicate column name"),
            "错误消息应含 duplicate column name，实际: {err}"
        );
        // 容忍路径：has_column 认为存在时跳过；重复调用幂等不报错
        ensure_timing_columns(&conn).expect("重复迁移应幂等");
        ensure_timing_columns(&conn).expect("重复迁移应幂等");
    }

    /// 本机只读审计：对真实 ~/.claude/projects 跑同一套解析（不写库），
    /// 按 CLI version 分桶打印覆盖率与中位数，对照计划「本机数据依据」。
    /// 仅人工核对，不计入常规测试（#[ignore]）。
    #[test]
    #[ignore]
    fn local_claude_timing_audit() {
        let Ok(dir) = projects_dir() else {
            eprintln!("[audit] 本机无 Claude projects 目录，跳过");
            return;
        };
        let mut files = Vec::new();
        collect_session_files(&dir, 5, &mut files);
        files.sort();

        #[derive(Default)]
        struct Bucket {
            replies: u64,
            with_first_output: u64,
            with_api_request: u64,
            with_parent_fallback: u64,
            durations_ms: Vec<f64>,
            ttft_ms: Vec<f64>,
            stream_speeds: Vec<f64>,
            /// 带重试的回复数与各自的重试次数（看最大值 / 中位数分布）
            with_retry: u64,
            retry_counts: Vec<i64>,
            /// 最终失败的回复数（status = error 的 0 token 行）
            errors: u64,
            /// 多块且块间夹有 user 工具结果行的回复数（同一响应边收流边执行
            /// 工具，块被工具结果行隔开）
            with_tool_result: u64,
        }
        fn median(values: &mut Vec<f64>) -> Option<f64> {
            if values.is_empty() {
                return None;
            }
            values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            Some(values[values.len() / 2])
        }
        fn median_i64(values: &mut Vec<i64>) -> Option<i64> {
            if values.is_empty() {
                return None;
            }
            values.sort_unstable();
            Some(values[values.len() / 2])
        }

        let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
        for path in &files {
            let Ok(file) = std::fs::File::open(path) else {
                continue;
            };
            let mut scanner = ReplyScanner::new();
            let mut collect = |record: Option<ReplyRecord>,
                               buckets: &mut BTreeMap<String, Bucket>| {
                let Some(record) = record else { return };
                let version = record.version.clone().unwrap_or_else(|| "unknown".into());
                let bucket = buckets.entry(version).or_default();
                bucket.replies += 1;
                if record.first_token_at.is_some() {
                    bucket.with_first_output += 1;
                }
                match record.timing_estimated {
                    Some(0) => bucket.with_api_request += 1,
                    Some(1) => bucket.with_parent_fallback += 1,
                    _ => {}
                }
                if let Some(duration) = record.duration_ms() {
                    bucket.durations_ms.push(duration);
                }
                if let Some(ttft) = record.time_to_first_token_ms {
                    bucket.ttft_ms.push(ttft);
                }
                // 流式速度口径：首字与结束齐全且生成窗口合法
                if let (Some(first), Some(done)) = (record.first_token_at, record.completed_at) {
                    let span = done - first;
                    if record.output_tokens > 0
                        && span >= crate::token_speed::MIN_GEN_MS
                        && span <= crate::token_speed::MAX_GEN_MS
                    {
                        bucket
                            .stream_speeds
                            .push(record.output_tokens as f64 * 1000.0 / span as f64);
                    }
                }
                if record.retry_count > 0 {
                    bucket.with_retry += 1;
                    bucket.retry_counts.push(record.retry_count);
                }
                if record.status == Some("error") {
                    bucket.errors += 1;
                }
                if record.tool_result_between {
                    bucket.with_tool_result += 1;
                }
            };
            use std::io::BufRead;
            for line in std::io::BufReader::new(file).lines().flatten() {
                if let Ok(line) = serde_json::from_str::<TranscriptLine>(&line) {
                    let finished = scanner.feed(&line, true);
                    collect(finished, &mut buckets);
                }
            }
            collect(scanner.finish(), &mut buckets);
        }

        eprintln!(
            "[audit] 文件数={}（目录 {}）",
            files.len(),
            dir.display()
        );
        eprintln!(
            "[audit] {:<10} {:>8} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>18} {:>8} {:>12}",
            "version", "回复数", "首字覆盖", "req关联", "父链回退", "耗时中位", "首字中位", "流速中位", "重试回复(最大/中位)", "失败回复", "夹工具结果"
        );
        for (version, bucket) in &buckets {
            let pct = |count: u64| -> String {
                if bucket.replies == 0 {
                    "—".to_string()
                } else {
                    format!("{:.1}%", count as f64 * 100.0 / bucket.replies as f64)
                }
            };
            let fmt_ms = |value: Option<f64>| -> String {
                match value {
                    Some(v) => format!("{:.0}ms", v),
                    None => "—".to_string(),
                }
            };
            let retry_summary = if bucket.with_retry == 0 {
                "0".to_string()
            } else {
                format!(
                    "{}(最大{}/中位{})",
                    bucket.with_retry,
                    bucket.retry_counts.iter().max().unwrap_or(&0),
                    median_i64(&mut bucket.retry_counts.clone()).unwrap_or(0)
                )
            };
            eprintln!(
                "[audit] {:<10} {:>8} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>18} {:>8} {:>12}",
                version,
                bucket.replies,
                pct(bucket.with_first_output),
                pct(bucket.with_api_request),
                pct(bucket.with_parent_fallback),
                fmt_ms(median(&mut bucket.durations_ms.clone())),
                fmt_ms(median(&mut bucket.ttft_ms.clone())),
                bucket
                    .stream_speeds
                    .is_empty()
                    .then(|| "—".to_string())
                    .unwrap_or_else(|| format!(
                        "{:.1}t/s",
                        median(&mut bucket.stream_speeds.clone()).unwrap()
                    )),
                retry_summary,
                bucket.errors,
                bucket.with_tool_result,
            );
        }
    }
}

