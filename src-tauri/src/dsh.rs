//! DeepSeek Harness (DSH) 会话用量导入模块。
//!
//! 数据来源：`~/.dsh/sessions/<projectKey>/<sessionId>/session.v4.jsonl.zstd`。
//! `ZBAR_DSH_HOME` 可覆盖 `.dsh` 根目录（测试/便携场景，对齐 claude.rs 的
//! ZBAR_CLAUDE_HOME 先例）。
//!
//! **为什么读事件日志而不是 session_projcache**：DSH 也把每个会话的投影快照
//! 落在 `~/.dsh/storages/session_projcache/sessions/<sid>.json`（明文 JSON），
//! 但它只是**节流（5s / 每 200 事件）的滞后缓存**——实测 8 个会话中 3 个落后于
//! 日志（活跃会话落后一个 turn）。且它只有「会话累计」一个数，没有逐 step 时间
//! 与逐模型明细，无法支撑「今日/7天/30天」范围筛选与模型分布。故本模块以**事件
//! 日志为权威源**，projcache 仅用于补 `cwd`（见 `project_cwd_fallback`）。
//!
//! **文件格式要点（均已实测确认）**：
//! - 多帧串联 zstd：整个文件由上百个独立 zstd 帧首尾相接（实测 151~156 帧），
//!   每帧解压后得到**一个完整 JSON 行**。`ruzstd` 的 `StreamingDecoder` 只解单帧，
//!   必须自行按 magic 切分（见 `split_zstd_frames`）。
//! - append-only：帧只追加，故可用 `file_progress` 记录字节偏移做增量续读。
//!
//! **计费口径（关键，踩过坑）**：`assistant/message.data.usage` 里
//! - `inputTokens` 是**非缓存输入**（实测 197/197 条满足
//!   `inputTokens + cacheReadTokens + outputTokens == totalTokens`），
//!   而项目 `cost_for`（lib.rs）假定 `input_tokens` **已包含** cache_read
//!   （`non_cache_input = input - cache_read`）。故落库时归一化为
//!   `input_tokens = inputTokens + cacheReadTokens`，否则缓存读会被双重扣减。
//! - `totalTokens` 是**单次请求的上下文总量**（逐步 9,093 → 101,326 → … 递增），
//!   **不是会话总量**，求和会严重重复计数。故落库的 `computed_total_tokens`
//!   按 `input+output` 自行求和，绝不累加 `totalTokens`。
//! - `cacheReadTokens` 占绝对多数（本机样本 4,630 万 / 4,830 万），计费必须走
//!   缓存读价，否则数字离谱。
//!
//! **速度口径**：`assistant/message.time` 实测与末个 stream chunk 的 `time`
//! 相差 0~1ms，即该 step 的生成结束时刻。生成窗口取「末 chunk − 首 chunk
//! （block-start）」，TTFT 取「首 chunk − 该 step 起始时刻」。因此本模块产出的是
//! **真生成速度**（`SpeedQuality::Generation`，前端不带 ≈ 前缀），且**有真实
//! TTFT**——这两点是 claude/kimi 导入库给不出的（它们的 jsonl 只有请求总耗时）。
//!
//! **无订阅额度（刻意不提供，别再来加）**：DSH 把 DeepSeek 平台账号登录态存在
//! `~/.dsh/.credentials.yaml`（记录名 `deepseek-account-platform/default`），
//! 但那条记录的 `kind` 是 `grant`，其 `token` **不是开放平台 API Key**——实测
//! 用它打 `GET https://api.deepseek.com/user/balance` 返回 401
//! （`Authentication Fails, Your api key: ****… is invalid`），而 platform 网端
//! 接口被 WAF 拦（HTTP 429 `Request Blocked`）。DeepSeek 本身按量计费、没有
//! Coding Plan 式的订阅窗口，故本来源**不产任何额度条目**：余额查询继续走既有
//! 凭证通道（`deepseek` provider 的 API Key），不在这里重复实现，也不臆造窗口。

use crate::db;
use crate::pricing::config_dir;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// zstd 帧 magic（小端 `0xFD2FB528`）。
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

// ============================================================
// 路径
// ============================================================

/// DSH 根目录（ZBAR_DSH_HOME 优先，其次 ~/.dsh）。不做存在性检查。
fn dsh_home_path() -> PathBuf {
    if let Ok(home) = std::env::var("ZBAR_DSH_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dsh")
}

/// 会话事件日志根目录：<home>/sessions。
fn sessions_root_path() -> PathBuf {
    dsh_home_path().join("sessions")
}

/// 投影快照目录（仅用于补 cwd；缺失不影响主流程）。
fn projcache_dir() -> PathBuf {
    dsh_home_path()
        .join("storages")
        .join("session_projcache")
        .join("sessions")
}

/// 自有导入库路径：~/.zbar/dsh.sqlite
fn dsh_db_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("dsh.sqlite"))
}

/// 本机是否装过 DSH 并产生过会话（presence 探测用：目录存在即可）。
pub fn has_local_data() -> bool {
    sessions_root_path().is_dir()
}

// ============================================================
// 建库
// ============================================================

/// 进程内串行化建表（claude.rs schema_migration_lock 同款）。
static SCHEMA_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn schema_lock() -> &'static Mutex<()> {
    SCHEMA_LOCK.get_or_init(|| Mutex::new(()))
}

/// 打开（必要时创建）导入库并确保表结构就绪。
///
/// 去重键 dedupe_key = `<sessionId>:<turn>:<step>`：DSH 每一步只有一条
/// `assistant/message`，重试（`llm/retry-started`）后同 (turn,step) 会再落一条，
/// 按唯一键覆盖取终值即可，无需 message.id。
///
/// `time_to_first_token_ms` 是本库特有列（claude 库刻意没有，因为其数据源给不出
/// TTFT）。`delegation_depth` 标记子 agent 会话（>0），用于排查与可选分列。
fn open_dsh_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let conn = Connection::open(dsh_db_path()?)
        .map_err(|e| format!("打开 DSH 导入库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    {
        let _guard = schema_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS model_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                dedupe_key TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                model_id TEXT NOT NULL DEFAULT '',
                provider_id TEXT NOT NULL DEFAULT 'deepseek-account',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                duration_ms REAL,
                time_to_first_token_ms INTEGER,
                updated_at INTEGER NOT NULL DEFAULT 0,
                cwd TEXT,
                project_key TEXT,
                delegation_depth INTEGER NOT NULL DEFAULT 0,
                UNIQUE(dedupe_key)
             );
             CREATE INDEX IF NOT EXISTS idx_dsh_mu_started ON model_usage(started_at);
             CREATE INDEX IF NOT EXISTS idx_dsh_mu_updated ON model_usage(updated_at);
             CREATE INDEX IF NOT EXISTS idx_dsh_mu_session ON model_usage(session_id);
             CREATE TABLE IF NOT EXISTS file_progress (
                path   TEXT    PRIMARY KEY,
                offset INTEGER NOT NULL,
                size   INTEGER NOT NULL
             );",
        )
        .map_err(|e| format!("初始化 DSH 导入库失败: {e}"))?;
    }
    Ok(conn)
}

// ============================================================
// 事件日志解析（纯函数，便于单测）
// ============================================================

/// 单步用量记录（从一条 assistant/message 提取）。
///
/// 生成窗口与 TTFT **不在此计算**：TTFT 需要 `step/start` 的起始时刻，而它出现
/// 在同一条 assistant/message **之前**的另一行里，解析单行时拿不到。故这里只
/// 提取 stream 的首末 chunk 时刻，落库前由 `derive_timing` 统一算。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StepRecord {
    pub turn: i64,
    pub step: i64,
    /// step 结束时刻（ms epoch，实测与末个 stream chunk 相差 0~1ms）。
    pub ended_at: i64,
    pub provider: String,
    pub model: String,
    /// 已归一化：uncachedInput + cacheRead（对齐 cost_for 的 input 口径）。
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    /// stream 首个带 time 的元素时刻（block-start）。
    pub first_chunk_at: Option<i64>,
    /// stream 末个带 time 的元素时刻。
    pub last_chunk_at: Option<i64>,
}

/// 从 stream 数组提取 (首个带 time 的元素时刻, 末个带 time 的元素时刻)：纯函数。
pub(crate) fn stream_bounds(stream: Option<&Value>) -> (Option<i64>, Option<i64>) {
    let Some(arr) = stream.and_then(|s| s.as_array()) else {
        return (None, None);
    };
    let mut first: Option<i64> = None;
    let mut last: Option<i64> = None;
    for item in arr {
        let Some(t) = item.get("time").and_then(|t| t.as_i64()) else {
            continue;
        };
        if first.is_none() {
            first = Some(t);
        }
        last = Some(t);
    }
    (first, last)
}

/// 由 stream 首末时刻与 step 起始时刻推导 (生成窗口ms, TTFTms)：纯函数。
///
/// - 生成窗口 = 末 chunk − 首 chunk（首 chunk 即 `block-start`，开始吐字时刻）；
///   窗口 ≤ 0 或时刻缺失 → None（绝不用 0 冒充）；
/// - TTFT = step 起始 → 首 chunk。缺 `step/start`、或首 chunk 落在
///   [step 起始, step 结束] 之外（计时异常）→ None。宁可不出数，也不把
///   生成耗时当 TTFT 展示。
pub(crate) fn derive_timing(
    first_chunk_at: Option<i64>,
    last_chunk_at: Option<i64>,
    step_started_at: Option<i64>,
    ended_at: i64,
) -> (Option<f64>, Option<i64>) {
    let (Some(start_chunk), Some(end_chunk)) = (first_chunk_at, last_chunk_at) else {
        return (None, None);
    };
    let duration = end_chunk - start_chunk;
    let duration_ms = if duration > 0 {
        Some(duration as f64)
    } else {
        None
    };
    let ttft_ms = match step_started_at {
        Some(st) if start_chunk >= st && start_chunk <= ended_at => Some(start_chunk - st),
        _ => None,
    };
    (duration_ms, ttft_ms)
}

/// 单条事件日志行的解析结果：要么是元信息（首行 header），要么是若干步用量。
#[derive(Debug, Default)]
pub(crate) struct ParsedLine {
    /// header 行携带的会话元信息（其余行为 None）。
    pub session: Option<SessionMeta>,
    pub steps: Vec<StepRecord>,
    /// `step/start` 携带的 (turn, step, 起始时刻)，供后续 assistant/message 算 TTFT。
    pub step_start: Option<(i64, i64, i64)>,
}

/// 会话元信息（事件日志首行 + projcache 兜底）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionMeta {
    pub id: String,
    pub created_at: i64,
    pub cwd: Option<String>,
    pub delegation_depth: i64,
}

/// 解析单条事件日志行（纯函数，单测直接喂真实行文本）。
/// 只关心 3 类事件（DSH 有 60 余种，其余一律忽略且不报错）：
/// `session`（首行元信息）、`step/start`（记起始时刻）、`assistant/message`（用量）。
pub(crate) fn parse_line(line: &str) -> Option<ParsedLine> {
    let v: Value = serde_json::from_str(line).ok()?;
    let kind = v.get("type").and_then(|t| t.as_str())?;
    match kind {
        "session" => {
            let id = v.get("id").and_then(|t| t.as_str())?.to_string();
            Some(ParsedLine {
                session: Some(SessionMeta {
                    id,
                    created_at: v.get("createdAt").and_then(|t| t.as_i64()).unwrap_or(0),
                    cwd: v
                        .get("cwd")
                        .and_then(|t| t.as_str())
                        .map(str::trim)
                        .filter(|c| !c.is_empty())
                        .map(|c| c.to_string()),
                    delegation_depth: v
                        .get("delegationDepth")
                        .and_then(|t| t.as_i64())
                        .unwrap_or(0),
                }),
                ..Default::default()
            })
        }
        "step/start" => {
            let data = v.get("data")?;
            let time = v.get("time").and_then(|t| t.as_i64())?;
            Some(ParsedLine {
                step_start: Some((
                    data.get("turn").and_then(|t| t.as_i64())?,
                    data.get("step").and_then(|t| t.as_i64())?,
                    time,
                )),
                ..Default::default()
            })
        }
        "assistant/message" => {
            let data = v.get("data")?;
            let ended_at = v.get("time").and_then(|t| t.as_i64())?;
            let turn = data.get("turn").and_then(|t| t.as_i64())?;
            let step = data.get("step").and_then(|t| t.as_i64())?;
            let usage = data.get("usage")?;
            let uncached = usage.get("inputTokens").and_then(|t| t.as_i64()).unwrap_or(0);
            let output = usage
                .get("outputTokens")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            let cache_read = usage
                .get("cacheReadTokens")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            let cache_write = usage
                .get("cacheWriteTokens")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            // 全零的占位行跳过（与 claude 导入同款防御）
            if uncached + output + cache_read + cache_write <= 0 {
                return Some(ParsedLine::default());
            }
            // provider/model 在 message.source；缺失时留默认值，不阻断导入
            let source = data.get("message").and_then(|m| m.get("source"));
            let provider = source
                .and_then(|s| s.get("provider"))
                .and_then(|t| t.as_str())
                .unwrap_or("deepseek-account")
                .to_string();
            let model = source
                .and_then(|s| s.get("model"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            let (first_chunk_at, last_chunk_at) = stream_bounds(data.get("stream"));
            Some(ParsedLine {
                steps: vec![StepRecord {
                    turn,
                    step,
                    ended_at,
                    provider,
                    model,
                    // 归一化：cost_for 口径要求 input 已含 cache_read
                    input_tokens: uncached + cache_read,
                    output_tokens: output,
                    cache_read_tokens: cache_read,
                    first_chunk_at,
                    last_chunk_at,
                }],
                ..Default::default()
            })
        }
        _ => Some(ParsedLine::default()),
    }
}

// ============================================================
// 多帧 zstd
// ============================================================

/// 按 magic 把多帧串联的 zstd 字节流切分为各帧的区间 [start, end)。
///
/// zstd 帧以 `28 B5 2F FD` 开头（skippable 帧另有一套 magic，DSH 未使用，故
/// 这里只认标准 magic）。刻意不解析帧头长度：按 magic 切分已在真实文件上验证过
/// （本机样本 825/825 帧切分正确）。
pub(crate) fn split_zstd_frames(buf: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    if buf.len() < ZSTD_MAGIC.len() {
        return Vec::new();
    }
    for i in 0..=(buf.len() - ZSTD_MAGIC.len()) {
        if buf[i..i + ZSTD_MAGIC.len()] == ZSTD_MAGIC {
            starts.push(i);
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(k, &s)| (s, starts.get(k + 1).copied().unwrap_or(buf.len())))
        .collect()
}

/// 解压「以完整帧结尾」的字节前缀，返回 (解出的所有 JSON 行, 已确定完整的字节数)。
///
/// **一帧不一定只有一行**：实测本机会话日志 1342 帧中有 697 帧含多行（合计
/// 2114 行），早期只抽样看前几帧得出的「一帧一行」结论是错的 —— 若按帧当行，
/// 多行帧里的多个 JSON 会被粘成一段而整体解析失败（表现为 assistant/message
/// 一条都取不到）。故这里统一按换行切分，行内空白行丢弃。
///
/// 末尾帧若解压失败（DSH 正在写入 → 帧不完整），该帧被丢弃且**不计入已消费
/// 字节**（由调用方据返回的 `consumed` 决定进度），下次追加后重读，
/// 避免永久丢数据。
fn decode_frames(buf: &[u8]) -> (Vec<String>, usize) {
    let frames = split_zstd_frames(buf);
    let mut out = Vec::new();
    let mut consumed = 0usize;
    for &(start, end) in frames.iter() {
        let Ok(mut dec) = ruzstd::decoding::StreamingDecoder::new(&buf[start..end]) else {
            consumed = start;
            return (out, consumed);
        };
        let mut text = String::new();
        use std::io::Read;
        match dec.read_to_string(&mut text) {
            Ok(_) => {
                for line in text.split('\n') {
                    let line = line.trim_end_matches('\r').trim();
                    if !line.is_empty() {
                        out.push(line.to_string());
                    }
                }
                consumed = end;
            }
            Err(_) => {
                // 该帧不完整：不推进到它的结尾，停在这里
                consumed = start;
                return (out, consumed);
            }
        }
    }
    (out, consumed)
}

// ============================================================
// cwd 兜底（读 projcache）
// ============================================================

/// 从投影快照取 cwd（兜底用）：事件日志 header 缺失 cwd 时才读。
/// 只认 unit version 7 且 `identity.formatVersion == 4`，其余一律放弃
/// （DSH 升级换格式时宁可少一个 cwd，也不误解析）。同名 `.bak.<stamp>` 文件
/// 不参与（文件名不匹配本函数构造的路径）。
fn project_cwd_fallback(session_id: &str) -> Option<(String, i64)> {
    let path = projcache_dir().join(format!("{session_id}.json"));
    let data = std::fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&data).ok()?;
    if v.get("version").and_then(|t| t.as_i64()) != Some(7) {
        return None;
    }
    let record = v.get("record")?;
    let identity = record.get("identity")?;
    if identity.get("formatVersion").and_then(|t| t.as_i64()) != Some(4) {
        return None;
    }
    let cwd = identity
        .get("cwd")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|c| !c.is_empty())?
        .to_string();
    let created_at = identity
        .get("createdAt")
        .and_then(|t| t.as_i64())
        .unwrap_or(0);
    Some((cwd, created_at))
}

// ============================================================
// 增量导入
// ============================================================

static IMPORT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static LAST_IMPORT_AT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();

fn import_lock() -> &'static Mutex<()> {
    IMPORT_LOCK.get_or_init(|| Mutex::new(()))
}

fn last_import_at() -> &'static Mutex<Option<std::time::Instant>> {
    LAST_IMPORT_AT.get_or_init(|| Mutex::new(None))
}

/// 增量导入（查询入口用，5 秒节流；失败也计入窗口避免重试风暴）。
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

/// 收集 `sessions/<projectKey>/<sessionId>/session.v4.jsonl.zstd`。
/// 层级固定两层，防御性下钻 3 层；只认 `.zstd` 后缀。
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
        } else if path
            .file_name()
            .map(|n| n.to_string_lossy().ends_with(".zstd"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

/// 增量导入（不节流）：逐会话日志按字节偏移续读新增帧并入库。
/// 单个文件失败只记日志跳过，不阻断其他文件。
pub fn import_incremental_force() -> Result<(), String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = sessions_root_path();
    if !root.is_dir() {
        // 未安装 DSH：不是错误，静默返回（tab 由 presence 决定不出现）
        return Ok(());
    }
    let mut conn = open_dsh_db()?;

    let mut progress: HashMap<String, u64> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT path, offset FROM file_progress")
            .map_err(|e| format!("读取 DSH 导入进度失败: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("读取 DSH 导入进度失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 DSH 导入进度失败: {e}"))?;
        for (p, off) in rows {
            progress.insert(p, off.max(0) as u64);
        }
    }

    let mut files = Vec::new();
    collect_session_files(&root, 3, &mut files);
    files.sort();

    for path in &files {
        let key = path.to_string_lossy().to_string();
        let known = progress.get(&key).copied();
        if let Err(e) = import_one_file(&mut conn, path, known) {
            eprintln!("[zbar-dsh] 导入 {} 失败（下次重试）: {e}", path.display());
        }
    }
    Ok(())
}

/// 导入单个会话日志：从 `known_offset` 起读取新增字节，逐帧解压逐行解析入库。
///
/// 与 claude.rs 的差异：claude 是逐行 jsonl（末尾半行留待下次），DSH 是
/// **多帧 zstd**——一个 frame 就是一条完整 JSON 行，故进度以「完整解压成功的
/// 帧边界」为准，不完整的末帧不推进进度。
fn import_one_file(
    conn: &mut Connection,
    path: &Path,
    known: Option<u64>,
) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(|e| format!("打开会话日志失败: {e}"))?;
    let size = file
        .metadata()
        .map_err(|e| format!("读取会话日志元信息失败: {e}"))?
        .len();
    // 文件变短（被重写/轮转）→ 从头重解析，UNIQUE 键保证幂等
    let mut offset = match known {
        Some(off) if off <= size => off,
        _ => 0,
    };
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("定位会话日志失败: {e}"))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("读取会话日志失败: {e}"))?;

    let session_dir_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let (lines, consumed) = decode_frames(&buf);
    if lines.is_empty() && consumed == 0 {
        return Ok(()); // 无可解析内容（可能只追加了不完整帧）
    }

    let now = crate::provider_quota::now_ms();
    let tx = conn
        .transaction()
        .map_err(|e| format!("开启 DSH 导入事务失败: {e}"))?;

    // 会话元信息：优先事件日志 header；缺失时用 projcache 兜底
    let mut meta: Option<SessionMeta> = None;
    // step/start 的起始时刻，供同 (turn,step) 的 assistant/message 算 TTFT。
    // DSH 保证 step/start 先于该 step 的 assistant/message 落盘，故单遍扫描即可。
    let mut step_starts: HashMap<(i64, i64), i64> = HashMap::new();
    let mut raw_steps: Vec<StepRecord> = Vec::new();

    for line in &lines {
        let Some(parsed) = parse_line(line) else {
            continue;
        };
        if let Some(s) = parsed.session {
            if meta.is_none() {
                meta = Some(s);
            }
        }
        if let Some((turn, step, at)) = parsed.step_start {
            step_starts.insert((turn, step), at);
        }
        raw_steps.extend(parsed.steps);
    }

    let session_id = meta
        .as_ref()
        .map(|m| m.id.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| session_dir_name.clone());

    let mut cwd = meta.as_ref().and_then(|m| m.cwd.clone());
    let delegation_depth = meta.as_ref().map(|m| m.delegation_depth).unwrap_or(0);
    // 兜底：事件日志 header 未给 cwd 时读投影快照（只取 cwd，不取别的字段——
    // 快照是滞后缓存，用量绝不从这里取）
    if cwd.is_none() {
        if let Some((fb_cwd, _)) = project_cwd_fallback(&session_id) {
            cwd = Some(fb_cwd);
        }
    }

    let project_key = cwd.as_deref().and_then(crate::projects::normalize_cwd);

    // 逐 step 归一化后写库
    for r in &raw_steps {
        let step_started = step_starts.get(&(r.turn, r.step)).copied();
        let (duration_ms, ttft_ms) =
            derive_timing(r.first_chunk_at, r.last_chunk_at, step_started, r.ended_at);
        let computed_total = r.input_tokens + r.output_tokens;
        let dedupe_key = format!("{session_id}:{}:{}", r.turn, r.step);
        tx.execute(
            "INSERT INTO model_usage
                (session_id, dedupe_key, started_at, model_id, provider_id,
                 input_tokens, output_tokens, cache_read_input_tokens,
                 cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                 duration_ms, time_to_first_token_ms, updated_at, cwd, project_key,
                 delegation_depth)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(dedupe_key) DO UPDATE SET
                started_at = excluded.started_at,
                model_id = excluded.model_id,
                provider_id = excluded.provider_id,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_read_input_tokens = excluded.cache_read_input_tokens,
                computed_total_tokens = excluded.computed_total_tokens,
                duration_ms = excluded.duration_ms,
                time_to_first_token_ms = excluded.time_to_first_token_ms,
                updated_at = ?12",
            rusqlite::params![
                session_id,
                dedupe_key,
                r.ended_at,
                r.model,
                r.provider,
                r.input_tokens,
                r.output_tokens,
                r.cache_read_tokens,
                computed_total,
                duration_ms,
                ttft_ms,
                now,
                cwd,
                project_key,
                delegation_depth,
            ],
        )
        .map_err(|e| format!("写入 DSH 用量记录失败: {e}"))?;
    }

    // 进度：只记录「所有解压成功的帧」已消费的字节数
    let key = path.to_string_lossy().to_string();
    offset += consumed as u64;
    tx.execute(
        "INSERT INTO file_progress (path, offset, size) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET offset = ?2, size = ?3",
        rusqlite::params![key, offset as i64, size as i64],
    )
    .map_err(|e| format!("记录 DSH 导入进度失败: {e}"))?;
    tx.commit()
        .map_err(|e| format!("提交 DSH 导入事务失败: {e}"))?;
    Ok(())
}

// ============================================================
// 查询（与 claude.rs 同名同构，查 dsh.sqlite；查询前先增量导入）
// ============================================================

/// 查询 [from_ms, to_ms) 内的统计。
///
/// 与 claude 的关键差异：**传 `has_ttft = true`** —— 本库有
/// `time_to_first_token_ms` 列，`db::speed_agg_columns` 会用
/// 「总耗时 − TTFT」当真生成窗口算 t/s，而不是把首字等待也算进速度。
/// 因此速度质量标 `Generation`（前端不带 ≈ 前缀），与 claude 的
/// `RequestAverage` 区分开。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
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
        .map_err(|e| format!("查询 DSH 整体统计失败: {e}"))?;

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
        .map_err(|e| format!("准备 DSH 模型分组查询失败: {e}"))?;

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
        .map_err(|e| format!("读取 DSH 模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 DSH 模型分组失败: {e}"))?;

    // 速度质量：生成窗口取自 stream 首末 chunk（真生成区间），非请求总耗时
    if overall.speed.avg_tps.is_some() {
        overall.speed.speed_quality = Some(crate::token_speed::SpeedQuality::Generation);
    }
    for model in &mut by_model {
        if model.speed.avg_tps.is_some() {
            model.speed.speed_quality = Some(crate::token_speed::SpeedQuality::Generation);
        }
    }

    let (earliest_ms, latest_ms): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(started_at), MAX(started_at) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("查询 DSH 时间范围失败: {e}"))?;

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

/// 查询 [from_ms, to_ms) 内的分桶统计；bucket 为 "hour" 或 "day"。
///
/// **桶数上限**：本函数按桶逐个查询，桶数由时间跨度决定。若调用方传入
/// `to_ms = i64::MAX`（冒烟测试/异常调用），按天迭代会是 1 亿+ 次而实际卡死
/// —— 故设上限 `MAX_TREND_BUCKETS`：超出即截断到该数量的桶（正常 UI 范围
/// 最多 30 天/24 小时，远达不到上限，行为不变）。
pub fn query_trend(
    from_ms: i64,
    to_ms: i64,
    bucket: &str,
) -> Result<Vec<db::TrendBucketRaw>, String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
    let width = if bucket == "hour" { 3_600_000 } else { 86_400_000 };

    let mut start = db::align_bucket_start(from_ms, bucket);
    let mut stmt = conn
        .prepare(
            "SELECT
                model_id,
                provider_id,
                COUNT(*),
                COALESCE(SUM(input_tokens),0),
                COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_input_tokens),0),
                COALESCE(SUM(computed_total_tokens),0)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2
             GROUP BY provider_id, model_id",
        )
        .map_err(|e| format!("准备 DSH 趋势查询失败: {e}"))?;

    /// 分桶数量上限（防 to_ms = i64::MAX 之类的无界调用卡死）。
    const MAX_TREND_BUCKETS: usize = 5_000;

    let mut out: Vec<db::TrendBucketRaw> = Vec::new();
    while start < to_ms && out.len() < MAX_TREND_BUCKETS {
        let end = start.saturating_add(width);
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
            .map_err(|e| format!("读取 DSH 趋势统计失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 DSH 趋势统计失败: {e}"))?;

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

/// 列出出现过的所有 (provider_id, model_id)，供价格配置用。
pub fn list_models() -> Result<Vec<db::ModelInfo>, String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT provider_id, model_id
             FROM model_usage
             ORDER BY model_id",
        )
        .map_err(|e| format!("准备 DSH 模型列表查询失败: {e}"))?;
    let models = stmt
        .query_map([], |row| {
            Ok(db::ModelInfo {
                provider_id: row.get(0)?,
                model_id: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取 DSH 模型列表失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 DSH 模型列表失败: {e}"))?;
    Ok(models)
}

/// 按指定周期聚合 Token（对比页需要真实 [reset_at, end_at) 边界）。
pub fn query_period_buckets(periods: &[(i64, i64)]) -> Result<Vec<db::PeriodBucket>, String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(SUM(computed_total_tokens),0), COUNT(*)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2",
        )
        .map_err(|e| format!("准备 DSH 周期聚合查询失败: {e}"))?;
    let mut out = Vec::with_capacity(periods.len());
    for &(reset_at, end_at) in periods {
        let (total_tokens, requests): (i64, i64) = stmt
            .query_row(rusqlite::params![reset_at, end_at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("查询 DSH 周期聚合失败: {e}"))?;
        out.push(db::PeriodBucket {
            reset_at,
            end_at,
            total_tokens,
            requests,
        });
    }
    Ok(out)
}

/// 查询 id > since 的明细记录（同步增量上传用）。
/// source 固定 "dsh"，local_rowid = 自增 id；带上 proto 5 的会话/项目维度。
pub fn query_since(since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, started_at, model_id, provider_id,
                    input_tokens, output_tokens, cache_read_input_tokens,
                    cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                    session_id, project_key, cwd
             FROM model_usage
             WHERE id > ?1
             ORDER BY id ASC
             LIMIT ?2",
        )
        .map_err(|e| format!("准备 DSH 明细查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![since, limit as i64], |row| {
            Ok(db::UsageRow {
                local_rowid: row.get(0)?,
                started_at: row.get(1)?,
                model_id: row.get(2)?,
                provider_id: row.get(3)?,
                input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                cache_read_input_tokens: row.get(6)?,
                cache_creation_input_tokens: row.get(7)?,
                reasoning_tokens: row.get(8)?,
                computed_total_tokens: row.get(9)?,
                source: "dsh".to_string(),
                session_id: row.get(10)?,
                project_key: row.get(11)?,
                project_display: row.get(12)?,
            })
        })
        .map_err(|e| format!("读取 DSH 明细失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 DSH 明细失败: {e}"))?;
    Ok(rows)
}

/// 查询修订行（updated_at > since_ts，同步补传用）。
///
/// 为什么 DSH 也需要修订通道：我们以**事件日志**为准，但一个 step 的
/// `assistant/message` 可能先以较小值落盘（DSH 侧尚未写全）之后再被同键
/// `ON CONFLICT` 覆盖为终值 —— 行 id 不变，普通 `id > 游标` 查询永远选不出它，
/// 服务端会一直保留旧小值。本查询按修订时间选出这些行，上传后由服务端
/// 以「总量更大者胜」的 upsert 覆盖修正（见 server/db.py）。
/// after_id 分页：按 id 升序、只取 id > after_id 的行，调用方逐批推进。
pub fn query_revised_since(
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, started_at, model_id, provider_id,
                    input_tokens, output_tokens, cache_read_input_tokens,
                    cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                    session_id, project_key, cwd
             FROM model_usage
             WHERE updated_at > ?1 AND id > ?2
             ORDER BY id ASC
             LIMIT ?3",
        )
        .map_err(|e| format!("准备 DSH 修订查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![since_ts, after_id, limit as i64], |row| {
            Ok(db::UsageRow {
                local_rowid: row.get(0)?,
                started_at: row.get(1)?,
                model_id: row.get(2)?,
                provider_id: row.get(3)?,
                input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                cache_read_input_tokens: row.get(6)?,
                cache_creation_input_tokens: row.get(7)?,
                reasoning_tokens: row.get(8)?,
                computed_total_tokens: row.get(9)?,
                source: "dsh".to_string(),
                session_id: row.get(10)?,
                project_key: row.get(11)?,
                project_display: row.get(12)?,
            })
        })
        .map_err(|e| format!("读取 DSH 修订行失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 DSH 修订行失败: {e}"))?;
    Ok(rows)
}

/// 会话悬浮窗（session_hud）用：查询活跃窗口内的会话行。
///
/// 口径与 claude::query_window_sessions 一致（内层按 started_at 索引取窗口内
/// 最近 N 笔的会话，成本硬上限；外层对命中会话做全生命周期聚合），差异在于
/// **本模块多出真生成速度原料与 TTFT**：
/// - `gen_out_tokens` / `gen_ms`：`duration_ms > 0` 且 `output_tokens > 0` 行的
///   Σoutput 与 Σduration（分子分母同范围）→ session_hud 侧标 Generation；
/// - `ttft_ms`：最新一条有 TTFT 的 step 值；
/// - `generating`：DSH 无「运行中」标记可读（其会话投影才有，且是滞后缓存），
///   故恒 false，不臆造生成中状态。
///
/// 注意 `input_tokens` 在本库**已含** cache_read（见模块头计费口径），
/// 故非缓存输入 = input_tokens − cache_read_input_tokens。
pub fn query_window_sessions(
    window_start_ms: i64,
    limit: usize,
) -> Result<crate::session_hud::AgentWindowSessions, String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
    query_window_sessions_at(&conn, window_start_ms, limit)
}

/// query_window_sessions 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_window_sessions_at(
    conn: &Connection,
    window_start_ms: i64,
    limit: usize,
) -> Result<crate::session_hud::AgentWindowSessions, String> {
    /// 内层候选行上限（与 claude 同量级：限制成本，不全表扫）。
    const WINDOW_SCAN_ROWS: i64 = 2000;

    let sql = "SELECT session_id,
            MAX(started_at),
            COALESCE(SUM(MAX(0, input_tokens - cache_read_input_tokens)), 0),
            COALESCE(SUM(output_tokens), 0),
            COALESCE(SUM(cache_read_input_tokens), 0),
            COUNT(*),
            GROUP_CONCAT(DISTINCT model_id),
            (SELECT m2.cwd FROM model_usage m2
              WHERE m2.session_id = model_usage.session_id
                AND m2.cwd IS NOT NULL AND m2.cwd != ''
              ORDER BY m2.started_at DESC LIMIT 1),
            COALESCE(SUM(CASE WHEN duration_ms > 0 THEN output_tokens ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN duration_ms > 0 THEN duration_ms ELSE 0.0 END), 0.0),
            (SELECT CAST(ROUND(m3.time_to_first_token_ms) AS INTEGER)
               FROM model_usage m3
              WHERE m3.session_id = model_usage.session_id
                AND m3.time_to_first_token_ms IS NOT NULL
              ORDER BY m3.started_at DESC LIMIT 1)
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
        .map_err(|e| format!("准备 DSH 会话窗口查询失败: {e}"))?;
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
                    speed_out_tokens: row.get(8)?,
                    speed_ms: row.get(9)?,
                    // DSH 无运行中标记可读，不臆造
                    generating: false,
                    ttft_ms: row.get(10)?,
                    gen_out_tokens: row.get(8)?,
                    gen_ms: row.get(9)?,
                })
            },
        )
        .map_err(|e| format!("读取 DSH 会话窗口失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 DSH 会话窗口失败: {e}"))?;
    let total_active: usize = conn
        .query_row(
            "SELECT COUNT(DISTINCT session_id) FROM model_usage WHERE started_at >= ?1",
            [window_start_ms],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as usize)
        .map_err(|e| format!("统计 DSH 活跃会话数失败: {e}"))?;
    Ok(crate::session_hud::AgentWindowSessions {
        total_active,
        sessions,
    })
}

/// 按项目 × 模型聚合 [from_ms, to_ms) 的用量（SQL 侧 GROUP BY，不逐行加载）。
/// project_key 为 NULL/空串（未回填或无 cwd）归入 unknown 哨兵键。
pub fn query_project_model_rows(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<crate::projects::ProjectModelRow>, String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
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
        .map_err(|e| format!("准备 DSH 项目聚合查询失败: {e}"))?;
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
        .map_err(|e| format!("读取 DSH 项目聚合失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 DSH 项目聚合失败: {e}"))?;
    Ok(rows)
}

/// 按项目统计 [from_ms, to_ms) 内的会话数（COUNT(DISTINCT session_id)）。
pub fn query_project_session_counts(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<(String, i64)>, String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(NULLIF(project_key, ''), ?1), COUNT(DISTINCT session_id)
             FROM model_usage
             WHERE started_at >= ?2 AND started_at < ?3
             GROUP BY 1",
        )
        .map_err(|e| format!("准备 DSH 项目会话统计失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![crate::projects::UNKNOWN_PROJECT, from_ms, to_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("读取 DSH 项目会话统计失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 DSH 项目会话统计失败: {e}"))?;
    Ok(rows)
}

/// 项目键 → 原始形态 cwd（保留大小写，取字典序最小者做代表）。
pub fn query_project_display_paths() -> Result<Vec<(String, String)>, String> {
    let conn = open_dsh_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT project_key, MIN(cwd)
             FROM model_usage
             WHERE project_key IS NOT NULL AND project_key != ?1
               AND cwd IS NOT NULL AND cwd != ''
             GROUP BY project_key",
        )
        .map_err(|e| format!("准备 DSH 项目路径查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![crate::projects::UNKNOWN_PROJECT], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("读取 DSH 项目路径失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 DSH 项目路径失败: {e}"))?;
    Ok(rows)
}

/// 分页查询指定项目的会话明细（按会话最后活跃时间降序）。
/// 返回 (匹配会话总数, 按会话 × 模型聚合的行)；offset/limit 作用在会话粒度。
/// project_key 传 UNKNOWN_PROJECT 时匹配 NULL/空串/哨兵的会话。
/// 与 claude 的差异：本库有 `time_to_first_token_ms`，故传 `has_ttft = true`，
/// 会话级速度与 TTFT 均为真生成口径。
pub fn query_project_sessions(
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Result<(u32, Vec<crate::projects::ProjectSessionModelRow>), String> {
    import_incremental()?;
    let conn = open_dsh_db()?;
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
        .map_err(|e| format!("查询 DSH 项目会话总数失败: {e}"))?;

    let speed = db::session_speed_agg_columns(
        db::has_column(&conn, "model_usage", "duration_ms"),
        db::has_column(&conn, "model_usage", "time_to_first_token_ms"),
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
        .map_err(|e| format!("准备 DSH 项目会话查询失败: {e}"))?;
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
                    // session_speed_agg_columns 的 4 列：Σtps、COUNT(tps)、Σttft、COUNT(ttft)
                    tps_sum: row.get(9)?,
                    tps_count: row.get(10)?,
                    ttft_sum: row.get(11)?,
                    ttft_count: row.get(12)?,
                    // 生成口径原料由 zcode 的行级采样提供，DSH 走 tps_sum 通道
                    gen_out: None,
                    gen_ms: None,
                    gen_count: 0,
                })
            },
        )
        .map_err(|e| format!("读取 DSH 项目会话失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 DSH 项目会话失败: {e}"))?;
    Ok((total.max(0) as u32, rows))
}

/// 库内最大自增 id（「待上传条数」展示用，与 claude/codex 同款）。
/// 不触发增量导入（只读游标口径，避免为一次计数做全量扫描）。
pub fn max_rowid() -> Result<i64, String> {
    let conn = open_dsh_db()?;
    let max: Option<i64> = conn
        .query_row("SELECT MAX(id) FROM model_usage", [], |row| row.get(0))
        .map_err(|e| format!("查询 DSH 最大 rowid 失败: {e}"))?;
    Ok(max.unwrap_or(0))
}

/// 诊断信息（设置页/排障用）。
#[derive(Debug, serde::Serialize)]
pub struct DshDebugInfo {
    pub sessions_dir: String,
    pub sessions_dir_exists: bool,
    /// 会话日志文件数
    pub session_files: u64,
    /// 导入库记录数
    pub imported_records: i64,
    /// 最新一条记录时间
    pub latest_record_ms: Option<i64>,
}

pub fn debug_info() -> Result<DshDebugInfo, String> {
    let dir = sessions_root_path();
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_session_files(&dir, 3, &mut files);
    }
    let (imported_records, latest_record_ms) = match open_dsh_db() {
        Ok(conn) => {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM model_usage", [], |row| row.get(0))
                .unwrap_or(0);
            let latest: Option<i64> = conn
                .query_row("SELECT MAX(started_at) FROM model_usage", [], |row| {
                    row.get(0)
                })
                .unwrap_or(None);
            (count, latest)
        }
        Err(_) => (0, None),
    };
    Ok(DshDebugInfo {
        sessions_dir: dir.display().to_string(),
        sessions_dir_exists: dir.is_dir(),
        session_files: files.len() as u64,
        imported_records,
        latest_record_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实事件的 JSON 行（从本机 DSH 日志采样，形状与字段名均为实测）。
    const ASSISTANT_LINE: &str = r#"{"type":"assistant/message","seq":26,"time":1791820208130,"data":{"turn":1,"step":1,"message":{"role":"assistant","source":{"kind":"model","provider":"deepseek-account","model":"deepseek-flash"}},"usage":{"inputTokens":7578,"outputTokens":139,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":7717},"stream":[{"type":"chunk","time":1791820206402,"chunk":{"type":"block-start"}},{"type":"reasoning-chunks","time0":1791820207629,"dt":[54,13]},{"type":"chunk","time":1791820208129,"chunk":{"type":"block-stop"}}]}}"#;

    /// 取本机任意一个真实会话日志的原始字节（无则 None → 相关测试跳过）。
    fn real_session_bytes() -> Option<Vec<u8>> {
        let root = sessions_root_path();
        if !root.is_dir() {
            return None;
        }
        let mut files = Vec::new();
        collect_session_files(&root, 3, &mut files);
        files.sort();
        files
            .into_iter()
            .find_map(|p| std::fs::read(&p).ok().filter(|b| b.len() > 64))
    }

    /// 会话 header 行解析：id / createdAt / cwd / delegationDepth。
    #[test]
    fn parses_session_header() {
        let line = r#"{"type":"session","version":4,"id":"session-abc","createdAt":1791820037486,"cwd":"D:\\code\\demo","isSeeded":false,"delegationDepth":2,"agentPreset":"standard"}"#;
        let p = parse_line(line).expect("应解析成功");
        let s = p.session.expect("应带会话元信息");
        assert_eq!(s.id, "session-abc");
        assert_eq!(s.created_at, 1791820037486);
        assert_eq!(s.cwd.as_deref(), Some(r"D:\code\demo"));
        assert_eq!(s.delegation_depth, 2);
        assert!(p.steps.is_empty());
    }

    /// cwd 缺失/空白 → None（不产生空字符串脏值）。
    #[test]
    fn session_cwd_blank_becomes_none() {
        let line =
            r#"{"type":"session","version":4,"id":"s1","createdAt":1,"cwd":"   ","delegationDepth":0}"#;
        let p = parse_line(line).expect("应解析成功");
        assert_eq!(p.session.unwrap().cwd, None);
    }

    /// step/start 解析出 (turn, step, 起始时刻)，供 TTFT 计算。
    #[test]
    fn parses_step_start() {
        let line =
            r#"{"type":"step/start","seq":2,"time":1791820205000,"data":{"turn":1,"step":1}}"#;
        let p = parse_line(line).expect("应解析成功");
        assert_eq!(p.step_start, Some((1, 1, 1791820205000)));
        assert!(p.steps.is_empty());
    }

    /// **计费口径核心断言**：inputTokens 是「非缓存输入」，落库必须归一化为
    /// uncached + cacheRead（cost_for 假定 input 已含 cache_read）。
    /// 真实行 A：uncached=7578、cacheRead=0 → input=7578。
    #[test]
    fn assistant_usage_is_normalized_to_cache_inclusive_input() {
        let p = parse_line(ASSISTANT_LINE).expect("应解析成功");
        assert_eq!(p.steps.len(), 1);
        let s = &p.steps[0];
        assert_eq!(s.turn, 1);
        assert_eq!(s.step, 1);
        assert_eq!(s.ended_at, 1791820208130);
        assert_eq!(s.provider, "deepseek-account");
        assert_eq!(s.model, "deepseek-flash");
        assert_eq!(s.input_tokens, 7578);
        assert_eq!(s.output_tokens, 139);
        assert_eq!(s.first_chunk_at, Some(1791820206402));
        assert_eq!(s.last_chunk_at, Some(1791820208129));
    }

    /// 归一化的关键场景：cacheRead 非零时必须并入 input（否则缓存读被双重扣减）。
    /// 真实行 B（第二次调用，命中缓存）：uncached=190、cacheRead=7680 → input=7870。
    #[test]
    fn cache_read_tokens_are_folded_into_input() {
        let line = r#"{"type":"assistant/message","seq":30,"time":100,"data":{"turn":1,"step":2,"message":{"source":{"provider":"deepseek-account","model":"deepseek-flash"}},"usage":{"inputTokens":190,"outputTokens":137,"cacheReadTokens":7680,"cacheWriteTokens":0,"totalTokens":8007}}}"#;
        let s = &parse_line(line).expect("应解析成功").steps[0];
        assert_eq!(s.cache_read_tokens, 7680);
        assert_eq!(s.input_tokens, 7870, "input 必须是 uncached + cacheRead");
        // 归一化后 total 由 input+output 自算；绝不等于原始 totalTokens 的累加口径
        assert_eq!(s.input_tokens + s.output_tokens, 8007);
    }

    /// stream 首末时刻提取：跳过无 time 的元素（如 reasoning-chunks 只有 time0）。
    #[test]
    fn stream_bounds_skips_untimed_entries() {
        let (f, l) = stream_bounds(Some(&serde_json::json!([
            {"type": "reasoning-chunks", "time0": 111},
            {"type": "chunk", "time": 500},
            {"type": "chunk", "time": 900}
        ])));
        assert_eq!((f, l), (Some(500), Some(900)));
        assert_eq!(stream_bounds(None), (None, None));
        assert_eq!(stream_bounds(Some(&serde_json::json!({}))), (None, None));
        assert_eq!(
            stream_bounds(Some(&serde_json::json!([{"x": 1}]))),
            (None, None)
        );
    }

    /// 生成窗口与 TTFT：用真实样本时刻。
    /// 首 chunk 1791820206402、末 1791820208129 → 窗口 1727ms；
    /// step 起始 1791820205000 → TTFT 1402ms。
    #[test]
    fn derives_generation_window_and_ttft() {
        let (dur, ttft) = derive_timing(
            Some(1791820206402),
            Some(1791820208129),
            Some(1791820205000),
            1791820208130,
        );
        assert_eq!(dur, Some(1727.0));
        assert_eq!(ttft, Some(1402));
    }

    /// 缺 step/start → 不算 TTFT（宁可不给，也不把生成耗时当首字延迟）；
    /// 时刻倒挂同样判异常；窗口 ≤0 不给窗口。
    #[test]
    fn timing_rejects_missing_or_inverted_bounds() {
        let (dur, ttft) = derive_timing(Some(1000), Some(2000), None, 2000);
        assert_eq!(dur, Some(1000.0));
        assert_eq!(ttft, None);
        assert_eq!(derive_timing(Some(900), Some(2000), Some(1000), 2000).1, None);
        assert_eq!(
            derive_timing(Some(2500), Some(2600), Some(1000), 2000).1,
            None
        );
        assert_eq!(derive_timing(Some(2000), Some(2000), None, 2000).0, None);
        assert_eq!(derive_timing(Some(3000), Some(2000), None, 3000).0, None);
        assert_eq!(
            derive_timing(None, Some(2000), Some(1000), 2000),
            (None, None)
        );
        assert_eq!(
            derive_timing(Some(1000), None, Some(900), 2000),
            (None, None)
        );
    }

    /// 全零 usage 的占位行跳过（与 claude 导入同款防御），不产生 0 token 记录。
    #[test]
    fn zero_usage_placeholder_is_skipped() {
        let line = r#"{"type":"assistant/message","seq":9,"time":100,"data":{"turn":1,"step":1,"message":{"source":{"provider":"x","model":"m"}},"usage":{"inputTokens":0,"outputTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":0}}}"#;
        assert!(parse_line(line).expect("应解析成功").steps.is_empty());
    }

    /// 缺 model 时留空串而非报错；provider 缺失回退默认值。
    #[test]
    fn missing_model_and_provider_fall_back() {
        let line = r#"{"type":"assistant/message","seq":1,"time":100,"data":{"turn":2,"step":3,"message":{},"usage":{"inputTokens":10,"outputTokens":5}}}"#;
        let s = &parse_line(line).expect("应解析成功").steps[0];
        assert_eq!(s.model, "");
        assert_eq!(s.provider, "deepseek-account");
        assert_eq!(s.input_tokens, 10);
        assert_eq!((s.first_chunk_at, s.last_chunk_at), (None, None));
    }

    /// 未知事件类型（DSH 有 60 余种）与坏 JSON 一律静默忽略，不报错。
    #[test]
    fn unknown_events_and_bad_json_are_ignored() {
        assert!(parse_line(r#"{"type":"tool/call","seq":5,"time":1,"data":{}}"#)
            .expect("应解析成功")
            .steps
            .is_empty());
        assert!(parse_line("not json").is_none());
        assert!(parse_line(r#"{"no_type":1}"#).is_none());
    }

    /// 空/过短字节流不 panic，且切不出帧。
    #[test]
    fn splits_empty_and_short_input() {
        assert!(split_zstd_frames(&[]).is_empty());
        assert!(split_zstd_frames(&[0x28, 0xb5, 0x2f]).is_empty());
        // 只有 magic、没有帧体：切出 1 段（解压时自然失败，按不完整帧处理）
        assert_eq!(split_zstd_frames(&ZSTD_MAGIC).len(), 1);
    }

    /// **多帧切分与解压（真实文件）**：本机会话日志应切出多帧、每段以 magic
    /// 开头且首尾相接；解压后**行数 ≥ 帧数**（一帧可含多行，实测多数帧含多行），
    /// 全部解压成功且 consumed 覆盖整个输入。
    #[test]
    fn real_file_splits_and_decodes_all_frames() {
        let Some(bytes) = real_session_bytes() else {
            eprintln!("[test] 本机无 DSH 会话日志，跳过");
            return;
        };
        let frames = split_zstd_frames(&bytes);
        assert!(frames.len() > 1, "真实会话日志应为多帧串联");
        assert_eq!(frames[0].0, 0, "首帧从 0 开始");
        for w in frames.windows(2) {
            assert_eq!(w[0].1, w[1].0, "帧区间应首尾相接");
        }
        for &(s, _) in frames.iter().take(5) {
            assert_eq!(&bytes[s..s + 4], &ZSTD_MAGIC);
        }
        let (lines, consumed) = decode_frames(&bytes);
        assert!(lines.len() >= frames.len(), "行数不应少于帧数");
        assert_eq!(consumed, bytes.len(), "完整流应全部消费");
        assert!(
            lines.iter().any(|l| l.contains(r#""type":"session""#)),
            "应含会话 header 行"
        );
        // 每行都应是可独立解析的 JSON —— 这是「一帧多行」被打散的直接证据
        let parsed = lines
            .iter()
            .filter(|l| serde_json::from_str::<Value>(l).is_ok())
            .count();
        assert_eq!(parsed, lines.len(), "所有行都应是独立可解析的 JSON");
    }

    /// **截断帧不得推进进度**（DSH 正在写入时的关键防护）：
    /// 拼接前 N 帧 + 下一帧的前 10 字节，则前 N 帧应解出，而 consumed
    /// 必须停在第 N+1 帧的起点，绝不越过——否则该帧数据会被永久跳过。
    #[test]
    fn truncated_frame_does_not_advance_progress() {
        let Some(bytes) = real_session_bytes() else {
            eprintln!("[test] 本机无 DSH 会话日志，跳过");
            return;
        };
        let frames = split_zstd_frames(&bytes);
        if frames.len() < 3 {
            eprintln!("[test] 会话日志帧数不足，跳过");
            return;
        }
        let keep = 3; // 保留前 3 整帧
        let next_start = frames[keep].0;
        let truncated = &bytes[..next_start + 10];
        let (lines, consumed) = decode_frames(truncated);
        // 只保留前 keep 帧（每帧可能多行）：把前 keep 帧单独解压做基准对比
        let baseline = decode_frames(&bytes[..next_start]).0;
        assert_eq!(lines.len(), baseline.len(), "仅前 {keep} 帧的行应可解");
        assert_eq!(consumed, next_start, "进度必须停在第 {} 帧起点", keep + 1);
        assert!(consumed < truncated.len(), "不得把截断帧算作已消费");
    }

    /// 冒烟：对本机真实 `~/.dsh` 执行增量导入并做全范围查询。
    /// 本机无 DSH 数据时早退（他人机器/CI 上不算失败）。断言重点：
    /// - 幂等：连跑两次导入，记录数不变（帧偏移续读 + UNIQUE 键）；
    /// - 口径自洽：overall.total_tokens == by_model 求和、趋势求和 == 总量；
    /// - 归一化落地：input_tokens 必须 ≥ cache_read（因 input 已含缓存读）；
    /// - 真速度：本库有 TTFT 列，应能算出 avg_tps 或 avg_ttft_ms。
    #[test]
    fn real_home_import_and_query_smoke() {
        if !sessions_root_path().is_dir() {
            eprintln!("[test] 本机无 DSH 会话目录，跳过");
            return;
        }
        import_incremental_force().expect("首次增量导入失败");
        let first = query_stats(0, i64::MAX).expect("全范围查询失败");
        import_incremental_force().expect("二次增量导入失败");
        let second = query_stats(0, i64::MAX).expect("二次查询失败");
        assert_eq!(
            first.overall.requests, second.overall.requests,
            "重复导入导致记录数变化（幂等被破坏）"
        );
        if first.overall.requests == 0 {
            eprintln!("[test] DSH 目录存在但无用量记录，跳过后续断言");
            return;
        }
        let sum: i64 = first.by_model.iter().map(|m| m.total_tokens).sum();
        assert_eq!(
            first.overall.total_tokens, sum,
            "overall 与 by_model 汇总不一致"
        );
        assert!(
            first.overall.input_tokens >= first.overall.cache_read_tokens,
            "input_tokens({}) 应 >= cache_read_tokens({})（归一化失效？）",
            first.overall.input_tokens,
            first.overall.cache_read_tokens
        );
        assert!(
            first.overall.speed.avg_tps.is_some() || first.overall.speed.avg_ttft_ms.is_some(),
            "真实 DSH 数据应能算出速度或 TTFT"
        );
        if first.overall.speed.avg_tps.is_some() {
            assert_eq!(
                first.overall.speed.speed_quality,
                Some(crate::token_speed::SpeedQuality::Generation),
                "DSH 速度应为真生成口径（Generation）"
            );
        }
        let trend =
            query_trend(first.earliest_ms.unwrap_or(0), first.latest_ms.unwrap_or(0) + 1, "day")
                .expect("趋势查询失败");
        let trend_total: i64 = trend.iter().map(|b| b.total_tokens).sum();
        assert_eq!(
            trend_total, first.overall.total_tokens,
            "趋势总量与 overall 不一致"
        );
    }

    /// query_trend 的桶数上限：`to_ms = i64::MAX` 这类无界调用必须被截断而不是
    /// 迭代上亿次（此前会真实卡死）。用真实库 + 超大范围验证能立即返回。
    #[test]
    fn trend_bucket_count_is_bounded() {
        if !sessions_root_path().is_dir() {
            eprintln!("[test] 本机无 DSH 会话目录，跳过");
            return;
        }
        let started = std::time::Instant::now();
        let buckets = query_trend(0, i64::MAX, "day").expect("趋势查询应返回而不卡死");
        assert!(
            buckets.len() <= 5_000,
            "桶数应被上限截断，实际 {}",
            buckets.len()
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "应快速返回，实际耗时 {:?}",
            started.elapsed()
        );
    }
}
