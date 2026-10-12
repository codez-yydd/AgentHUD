//! MiniMax Code（MiniMax 官方桌面版 / CLI 开源 agent）本地用量导入模块。
//!
//! 数据来源：`<数据根>/v2/sqlite/runtime-state.sqlite`（sqlite，WAL 模式）。
//! 数据根解析顺序（与 app 自身的解析顺序一致）：
//! 1. `ZBAR_MINIMAX_HOME`（本项目惯例的测试/便携覆盖，直接指向 `.minimax` 数据根）；
//! 2. `MINIMAX_DATA_DIR`，其次 `MAVIS_DATA_DIR`（app 自己的公开覆盖变量）；
//! 3. `~/.minimax`；
//! 4. `~/.mavis`（旧版基名，仅在 `~/.minimax` 不存在时）。
//! 源库路径先取 `<根>/v2/sqlite/runtime-state.sqlite`，缺失时回退 pre-v2 布局
//! `<根>/sqlite/runtime-state.sqlite`（同一份 runtime-state 库的两种安放位置）。
//!
//! **为什么读库而不是文件**：MiniMax Code 不落 jsonl 用量日志，用量只在 sqlite 里。
//! 源库由 MiniMax Code 自己写入且长期开着 WAL，本模块**只读**打开
//! （`SQLITE_OPEN_READ_ONLY` + 3s busy_timeout），绝不写用户库；派生库落在
//! `~/.zbar/minimaxcode.sqlite`（自有库，读写均可），与 opencode.sqlite / dsh.sqlite
//! 同构。
//!
//! ## 源表（schema 与真实库逐列核对过）
//!
//! - `local_runtime_token_usage`：app 自己的注释是「Stores per-turn token and cost
//!   usage for session/agent/global summaries」。**每条已提交的 assistant 消息
//!   （即每次 LLM 调用）恰好一行**，在历史提交后 best-effort 追加写入；表是
//!   只追加的，app 只在**删除会话**时删行。`ts` 与全部时长都是 Unix 毫秒。
//!   实测 `model` 列恒 NULL（列在但没写值），故逐次调用的模型名必须从别处取。
//! - `local_runtime_sessions`：会话维度（`title` / `workspace_dir` /
//!   `project_workspace_dir` / `is_default_workspace` / `project_id` /
//!   `session_kind` / `created_at_ms` / `updated_at_ms`），其中
//!   `extra_data_json` 形如
//!   `{"effectiveModel":"minimax/MiniMax-M3.1-Flash-Preview","appMode":"coding",...}`
//!   —— `effectiveModel` 是 `<providerId>/<modelId>`（按**第一个** `/` 拆）。
//! - `local_runtime_message_rows`：消息行，assistant 行的 `data_json` 带
//!   `$.context_usage_telemetry.model`（**裸模型 id，无 provider**）与
//!   `$.usage.request_duration_ms` / `$.usage.decode_duration_ms`。这是逐次调用
//!   模型名与请求耗时的唯一来源。
//! - `local_runtime_projects`：`project_kind`（`default` / `workspace`）与
//!   `workspace_dir`（CHECK 约束保证 `default` 行的 workspace_dir 恒为 NULL）。
//!
//! 导入查询与真实库上验证过的形态完全一致：用量行 LEFT JOIN 会话、再 LEFT JOIN
//! 项目（`ORDER BY ts, id`），外加一条 assistant 行扫描（`ORDER BY id`）。
//! 缺表按表存在性降级：会话表缺失时相关列退化为常量 NULL（只影响项目维度与
//! 模型兜底，不影响用量导入），assistant 行表缺失时模型只剩 `t.model` 与会话
//! `effectiveModel` 两级兜底。assistant 行另加 `json_valid(data_json)` 守卫：
//! SQLite 的 `json_extract` 遇到坏 JSON 会**直接报错**，一条脏行不该让整轮导入失败。
//!
//! ## 计费口径（三条独立证据，均已在本机真实数据上核对）
//!
//! 归一集中在一处：`normalize_tokens`（全模块唯一的计费口径校准点）。
//!
//! 1. **app 自己写的 `context_usage_telemetry.providerTokens` 证明 `input_tokens`
//!    不含 cache read**：真实第一次调用 `input 24775 + cacheRead 189 = 24964`
//!    **正好等于**遥测里的 `providerTokens: 24964`。app 侧源码注释也明说该 input
//!    是「已经与 cache_read_input_tokens / cache_creation_input_tokens 不相交的
//!    新鲜输入」（最后一次缓存断点之后的部分）。故本模块落库时把它折进
//!    `input_tokens`（见下一条为什么要折）。
//! 2. **app 自己的计费函数证明 reasoning 已含在 output 里**：app 的
//!    `calculateCost(model, usage)` = `cost.input = price.in * usage.input;
//!    cost.output = price.out * usage.output; cost.cacheRead = price.cacheRead *
//!    usage.cacheRead; cost.cacheWrite = price.cacheWrite * usage.cacheWrite`，
//!    **没有独立的 reasoning 项**；app 的 OpenAI provider 注释也写着
//!    「completion_tokens already includes reasoning_tokens」。故
//!    `output_tokens = raw.output_tokens`，**绝不能**再加 `reasoning_tokens`
//!    （加了就是把 reasoning 双重计费）。旁证：同一条遥测的
//!    `usage.total_tokens = 25043` 正好 = `input 24775 + cacheRead 189 +
//!    output 79`，即 cache read 只算一次、reasoning 已在 output 内。
//! 3. **本项目 `cost_for`（lib.rs）假定 `input_tokens` 已含 cache read**：
//!    它算的是 `(input_tokens − cache_read_tokens) * price.in +
//!    output_tokens * price.out + cache_read_tokens * price.cacheRead`。既然
//!    证据 1 说明源库 input 不含 cache read，落库就必须折进去，否则缓存读会被
//!    双重扣减（dsh / opencode 上踩过的同一个坑）。
//!
//! 落库映射：
//! - `input_tokens = raw.input_tokens + raw.cache_read_tokens`
//! - `output_tokens = raw.output_tokens`（**不加** reasoning，见证据 2）
//! - `cache_read_input_tokens = raw.cache_read_tokens`、
//!   `cache_creation_input_tokens = raw.cache_write_tokens`（原样，各按自己的价计费）
//! - `reasoning_tokens = raw.reasoning_tokens`（**对本来源它是 output_tokens 的
//!   子集**，不是并列项；后来者勿把两者相加当总量）
//! - `computed_total_tokens = input_tokens + output_tokens`（reasoning 只计一次）
//!
//! **已知缺口（不修，只记录）**：本项目 `cost_for` **从不给
//! `cache_creation_input_tokens` 计价**，故源库 `cache_write_tokens` 非零时
//! 「按本项目价格表重算」的花费会偏小。app 自己那笔成本不受影响——它按
//! `price.cacheWrite` 计过，原样存在 `reported_cost_usd` 列里（见下），
//! 需要官方口径时以上层切到该列。真实数据里 cache write 恒 0，缺口当前不体现。
//!
//! **app 自算成本另存一列**：`reported_cost_usd` 原样保存源库 `cost_usd`
//! （USD 数值，app 自己按 models.dev 价格算好），供上层决定用官方口径还是按本项目
//! 价格表重算（本模块自身的 `query_*` 仍与 dsh/opencode 一致走 `cost_for`）。
//!
//! ## 行粒度
//!
//! 导入单位 = **一条 `local_runtime_token_usage` 行**（每次已提交的 assistant
//! 消息 / LLM 调用），**不是**会话聚合——app 的会话/全局汇总都由这些逐行记录累加
//! 而来，只有逐行口径才能把用量正确归属到模型（一个会话中途可以换模型）。
//! `started_at = ts`、`session_id = session_id`。
//!
//! ## 模型 / provider 解析阶梯（逐行，取到即止）
//!
//! (a) `local_runtime_token_usage.model` 非 NULL 非空时用它（实测恒 NULL，但列在）；
//! (b) 该 `(session_id, turn_id)` 分组的 assistant 行 `$.context_usage_telemetry.model`
//!     ——**仅当该分组的用量行数与 assistant 行数完全相等**时按 `ts/id` 顺序一一
//!     配对使用（数量不等说明配对关系不可信，宁可退到下一级也不猜）；
//!     遥测模型是裸 id（无 provider），故再按下面的「等值补 provider」规则补；
//! (c) 会话 `extra_data_json.effectiveModel`（按第一个 `/` 拆成 provider / model）；
//! (d) 都取不到就留空 model_id + 空 provider_id（不臆造）。
//!
//! 阶梯 (b) 的 **provider 等值补齐**（不是猜）：遥测模型与会话 `effectiveModel`
//! 的模型段**逐字相同**时，取会话那一段的 provider；不同则 provider 留空。依据是
//! app 保证 `effectiveModel` 形如 `<providerId>/<modelId>`——模型名相同即这次调用
//! 走的就是该 provider 配置。不补的代价很实在：单轮会话必然命中 (b)，(c) 根本没
//! 机会跑，实测数据里几乎**每一行**的 provider_id 都会是空的；而会话中途换过模型
//! 时模型段本就不同，仍留空，不会张冠李戴。
//!
//! `/` 拆分有**两套**规则（`split_provider_model`）：
//! - 源库 `t.model` / 遥测 model 走「前缀像 provider 才拆」的守卫版，避免把
//!   `Qwen/Qwen3-32B` 这类自带命名空间的模型 id 误拆出假 provider；
//! - 会话 `effectiveModel` 走无条件首 `/` 拆分（app 自己保证该字段恒为
//!   `<providerId>/<modelId>`）。
//!
//! ## 耗时 / 速度口径
//!
//! `duration_ms` = 配对成功的 assistant 行 `$.usage.request_duration_ms`
//! （源库只有整请求耗时；**正数且 ≤ `MAX_DURATION_MS` 才采纳**，否则 NULL——
//! 绝不用 0 冒充「瞬时完成」）。本来源**没有 TTFT**（没有首字时刻，只有
//! `decode_duration_ms` 这种整段解码时长），故 `has_ttft = false`：
//! `time_to_first_token_ms` 列恒 NULL，速度质量是请求平均，由注册表默认的
//! `RequestAverage` 声明（本模块**不**覆写 `speed_quality`），界面上带 ≈ 前缀。
//!
//! ## 项目维度
//!
//! 满足任一条即视为**没有项目**：
//! - `is_default_workspace = 1`（默认工作区，不是用户目录）；
//! - 工作区是**合成的按会话工作区** `<数据根>/sessions/<session_id>/workspace`
//!   （app 给每个会话开的临时目录，也不是用户目录）。
//! 否则 `project_key` = 归一化后的工作区路径（`project_workspace_dir` 优先，
//! 回退 `workspace_dir`，再回退项目行的 `workspace_dir`），原始路径按原样存
//! `cwd`（`project_display`，保留大小写供展示）。归一化复用
//! `projects::normalize_cwd`（与本项目其他导入器同一口径）。
//!
//! ## 增量口径（源级指纹，而不是逐文件偏移）
//!
//! 数据源是一张小的只追加表，没有「文件偏移」可言，故用**源级指纹**：
//! `source_progress` 记录用量表的 `(COUNT(*), MAX(id), MAX(ts))` 与会话表的
//! `(COUNT(*), MAX(updated_at_ms))`。两者都未变 → 整轮跳过（连派生库都不开写）。
//! 任一变化 → **整表重扫 + 对账**：
//! 1. 按稳定 `dedupe_key`（`"<session_id>|<turn_id>|<源行 id>"`）**原地 upsert**
//!    每一行，存活行的自增 id 因此恒定——这是同步上传不重复计数的前提
//!    （服务端按 `(device_id, source, local_rowid)` 去重，重导换了 id 就会在
//!    服务端落成一批新行）；
//! 2. 用临时键表做差集，删掉派生库里「本次没扫到」的 `dedupe_key`
//!    （幽灵行清理）——这同时覆盖了 **app 删除会话**（源行消失）的情形。
//! 为什么会话表指纹也要参与判定：会话级 `effectiveModel` / `title` /
//! 工作区变化时会改写模型归属与项目维度，只用量表指纹会漏掉这类修订。
//! 残留盲区：源行内容被改写却不动 `ts`、也不新增 id（app 正常写入都会追加新行），
//! 指纹不变、该次修订不会被发现。
//! 会话被 app 删除时 `model_usage` 的幽灵行会被差集清掉；`session_meta` 保留
//! 历史行（不主动清理，避免源库瞬时不可读时误删）。
//!
//! 已知上限：源库删掉的行，服务端没有删除通道，远端会保留它曾经上传的用量
//! （客户端本地口径仍然准确），与 claude/kimi/dsh/opencode 各来源一致。

#![allow(dead_code)]
// 说明：本模块与 opencode.rs 同构——生产入口是 lib.rs 的
// get_minimaxcode_usage / get_minimaxcode_debug、local_agents 注册表
// （价格页 / 项目页 / 悬浮窗 / 托盘 / 对比页 / 待上传计数）与 sync.rs 的明细
// 上传通道；各 `*_at` 纯查询内核与 `has_local_data_at` / `detect_tables` /
// `ImportOutcome` 字段只供单测注入临时库（不碰真实用户目录），故整模块允许
// dead_code，避免编译期噪声警告掩盖真实问题。

use crate::db;
use crate::pricing::config_dir;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 趋势分桶上限（防 `to_ms = i64::MAX` 之类的无界调用把逐桶查询变成上亿次）。
const MAX_TREND_BUCKETS: usize = 5_000;

/// 请求耗时上限（毫秒）：源库 `request_duration_ms` 超出该值视为脏值 → NULL。
/// 10 分钟是单次 LLM 请求的物理上界，超过它的值只可能是计时异常。
const MAX_DURATION_MS: i64 = 600_000;

/// 源库用量表名（存在性是 presence 与形态判定的唯一硬条件）。
const USAGE_TABLE: &str = "local_runtime_token_usage";
/// 源库会话表名（可选表，缺失时只影响项目维度与模型兜底）。
const SESSIONS_TABLE: &str = "local_runtime_sessions";
/// 源库消息行表名（可选表，缺失时模型/耗时只剩源列与会话兜底）。
const MESSAGE_ROWS_TABLE: &str = "local_runtime_message_rows";
/// 源库项目表名（可选表，缺失时项目维度退化为会话工作区）。
const PROJECTS_TABLE: &str = "local_runtime_projects";

/// `source_progress` 的主键：一个源库一行（同库多形态不需要分行存储）。
const PROGRESS_KEY: &str = "runtime-state";

// ============================================================
// 路径与 presence
// ============================================================

/// 取非空环境变量路径（trim 后为空视为未设置，避免空串把根目录解析成当前目录）。
fn env_path(name: &str) -> Option<PathBuf> {
    let raw = std::env::var(name).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// MiniMax Code 数据根目录（即 `.minimax`），解析顺序见模块头。
/// 三级都不存在时返回默认 `~/.minimax`（不做存在性报错：presence 由
/// `has_local_data` 判定，诊断信息里要能看到「本该在哪」）。
pub(crate) fn data_root() -> PathBuf {
    if let Some(path) = env_path("ZBAR_MINIMAX_HOME") {
        return path;
    }
    if let Some(path) = env_path("MINIMAX_DATA_DIR") {
        return path;
    }
    if let Some(path) = env_path("MAVIS_DATA_DIR") {
        return path;
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let current = home.join(".minimax");
    if current.is_dir() {
        return current;
    }
    // 旧版基名只在当前基名缺失时才用（两个都在时以 ~/.minimax 为准）
    let legacy = home.join(".mavis");
    if legacy.is_dir() {
        return legacy;
    }
    current
}

/// 源库路径（v2 优先，缺失回退 pre-v2 布局）。
pub(crate) fn source_db_path() -> PathBuf {
    source_db_path_at(&data_root())
}

/// `source_db_path` 的根目录注入版（单测用临时根，不碰真实用户目录）。
pub(crate) fn source_db_path_at(root: &Path) -> PathBuf {
    let v2 = root.join("v2").join("sqlite").join("runtime-state.sqlite");
    if v2.exists() {
        return v2;
    }
    let legacy = root.join("sqlite").join("runtime-state.sqlite");
    if legacy.exists() {
        return legacy;
    }
    // 都不存在时给 v2 路径：诊断信息里展示的是「本该在哪」
    v2
}

/// 自有导入库路径：~/.zbar/minimaxcode.sqlite（与 opencode.sqlite 同级）。
pub(crate) fn minimaxcode_db_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("minimaxcode.sqlite"))
}

/// 本机是否装过 MiniMax Code 并产生过用量：源库存在**且**含用量表。
///
/// 比「库文件存在」更严：空库或未来新版改名后没有用量表时不该假装有数据
/// （本判定同时是前端 `minimax` 页签的 presence 依据，见 provider_credentials）。
pub fn has_local_data() -> bool {
    has_local_data_at(&source_db_path())
}

/// `has_local_data` 的路径注入版（单测用临时库，不碰真实用户目录）。
pub(crate) fn has_local_data_at(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    match open_source_readonly(path) {
        Ok(conn) => table_exists(&conn, USAGE_TABLE),
        Err(_) => false,
    }
}

/// 以只读方式打开**用户库**（绝不写 MiniMax Code 的数据）。
///
/// busy_timeout 取 3s：app 常驻且 WAL 模式下随时可能写入，宁可等它写完
/// （与 claude/kimi/dsh/opencode 导入库同量级），也不能写用户库。
fn open_source_readonly(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("打开 MiniMax Code 本地库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    Ok(conn)
}

/// 表是否存在（表名都是代码内常量，无注入风险）。
fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

// ============================================================
// 派生库（~/.zbar/minimaxcode.sqlite）
// ============================================================

/// 进程内串行化建表（claude/kimi/dsh/opencode 同款：多个查询命令并发 open 时
/// 不交错建表）。
static SCHEMA_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn schema_lock() -> &'static Mutex<()> {
    SCHEMA_LOCK.get_or_init(|| Mutex::new(()))
}

/// 派生库 schema。`model_usage` 逐列与 opencode.rs 一致（含 dedupe_key /
/// computed_total_tokens / updated_at / duration_ms / reported_cost_usd 等），
/// 只有数据源特有的增量表不同（opencode 是逐会话 `session_progress`，
/// 本模块是源级 `source_progress`，见模块头）。
///
/// `session_meta` 是会话维度的全量快照（标题 / 工作区 / 项目键 / 时间戳），
/// 行上已把 cwd / project_key 冗余进 `model_usage` 供聚合，本表留全量便于排障
/// 与后续展示标题（当前查询不消费，项目维度全部走 `model_usage`）。
///
/// `time_to_first_token_ms` 与 opencode 同列但**恒 NULL**（本来源没有首字时刻，
/// 见模块头）；`delegation_depth` 同样恒 0（源库有 parent_session_id，但「深度」
/// 语义未确认，不自造层级）。
const DERIVED_SCHEMA: &str = "PRAGMA journal_mode=WAL;
     CREATE TABLE IF NOT EXISTS model_usage (
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
        duration_ms REAL,
        time_to_first_token_ms INTEGER,
        updated_at INTEGER NOT NULL DEFAULT 0,
        cwd TEXT,
        project_key TEXT,
        delegation_depth INTEGER NOT NULL DEFAULT 0,
        reported_cost_usd REAL,
        UNIQUE(dedupe_key)
     );
     CREATE INDEX IF NOT EXISTS idx_minimaxcode_mu_started ON model_usage(started_at);
     CREATE INDEX IF NOT EXISTS idx_minimaxcode_mu_updated ON model_usage(updated_at);
     CREATE INDEX IF NOT EXISTS idx_minimaxcode_mu_session ON model_usage(session_id);
     CREATE TABLE IF NOT EXISTS session_meta (
        session_id           TEXT PRIMARY KEY,
        title                TEXT,
        session_kind         TEXT NOT NULL DEFAULT '',
        directory            TEXT,
        project_key          TEXT,
        is_default_workspace INTEGER NOT NULL DEFAULT 0,
        project_kind         TEXT NOT NULL DEFAULT '',
        time_created         INTEGER NOT NULL DEFAULT 0,
        time_updated         INTEGER NOT NULL DEFAULT 0,
        captured_at          INTEGER NOT NULL DEFAULT 0
     );
     CREATE TABLE IF NOT EXISTS source_progress (
        source              TEXT PRIMARY KEY,
        usage_count         INTEGER NOT NULL DEFAULT 0,
        usage_max_id        INTEGER NOT NULL DEFAULT 0,
        usage_max_ts        INTEGER NOT NULL DEFAULT 0,
        session_count       INTEGER NOT NULL DEFAULT 0,
        session_max_updated INTEGER NOT NULL DEFAULT 0,
        imported_at         INTEGER NOT NULL DEFAULT 0
     );";

/// 打开（必要时创建）派生库并确保表结构就绪。
fn open_minimaxcode_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    open_minimaxcode_db_at(&minimaxcode_db_path()?)
}

/// 在指定路径打开派生库（表结构与生产库完全一致，供单测注入临时库）。
fn open_minimaxcode_db_at(path: &Path) -> Result<Connection, String> {
    let conn =
        Connection::open(path).map_err(|e| format!("打开 MiniMax Code 导入库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    {
        // 建表串行化：并发 open 时第二个 CREATE 不会与第一个交错
        let _guard = schema_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute_batch(DERIVED_SCHEMA)
            .map_err(|e| format!("初始化 MiniMax Code 导入库失败: {e}"))?;
    }
    Ok(conn)
}

// ============================================================
// 源库读取（指纹 / 用量行 / assistant 行）
// ============================================================

/// 源库可选表的存在性（缺表退化为常量 NULL，绝不因缺表让整轮导入失败）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceTables {
    pub sessions: bool,
    pub message_rows: bool,
    pub projects: bool,
}

pub(crate) fn detect_tables(src: &Connection) -> SourceTables {
    SourceTables {
        sessions: table_exists(src, SESSIONS_TABLE),
        message_rows: table_exists(src, MESSAGE_ROWS_TABLE),
        projects: table_exists(src, PROJECTS_TABLE),
    }
}

/// 源级指纹：用量表 `(行数, MAX(id), MAX(ts))` + 会话表 `(行数, MAX(updated_at_ms))`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SourceFingerprint {
    pub usage_count: i64,
    pub usage_max_id: i64,
    pub usage_max_ts: i64,
    pub session_count: i64,
    pub session_max_updated: i64,
}

/// 采集源级指纹。三个分量各自的职责：行数管「app 删了行」（会话被删时行数变小，
/// MAX(id) 不会变大）、MAX(id) 管「追加了新行」、MAX(ts) 管「新行的时刻更晚」；
/// 会话表分量管「标题 / effectiveModel / 工作区被改写」（这类变化会改模型归属与
/// 项目维度）。缺表的分量按 0 计（该表不存在即无可比较内容）。
pub(crate) fn collect_fingerprint(
    src: &Connection,
    tables: &SourceTables,
) -> Result<SourceFingerprint, String> {
    let (usage_count, usage_max_id, usage_max_ts): (i64, i64, i64) = src
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(id), 0), COALESCE(MAX(ts), 0)
             FROM local_runtime_token_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("采集 MiniMax Code 源库指纹失败: {e}"))?;
    let (session_count, session_max_updated) = if tables.sessions {
        src.query_row(
            "SELECT COUNT(*), COALESCE(MAX(updated_at_ms), 0) FROM local_runtime_sessions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("采集 MiniMax Code 会话指纹失败: {e}"))?
    } else {
        (0, 0)
    };
    Ok(SourceFingerprint {
        usage_count,
        usage_max_id,
        usage_max_ts,
        session_count,
        session_max_updated,
    })
}

/// 源库一条用量行（源列原值 + 会话/项目 join 出来的维度，未做任何归一）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SourceUsageRow {
    pub id: i64,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub ts: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
    /// 源表 `model` 列（实测恒 NULL，但形态上可能是 `<provider>/<model>` 或裸 id）
    pub model: Option<String>,
    pub title: Option<String>,
    pub is_default_workspace: Option<i64>,
    pub project_workspace_dir: Option<String>,
    pub workspace_dir: Option<String>,
    pub session_kind: Option<String>,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
    pub extra_data_json: Option<String>,
    pub project_kind: Option<String>,
    pub project_dir: Option<String>,
}

/// 用量行扫描 SQL（会话表/项目表缺失时对应列退化为常量 NULL）。
fn usage_query_sql(tables: &SourceTables) -> String {
    if tables.sessions {
        let session_join = "LEFT JOIN local_runtime_sessions s ON s.session_id = t.session_id";
        let project_join = if tables.projects {
            "LEFT JOIN local_runtime_projects p ON p.project_id = s.project_id"
        } else {
            ""
        };
        let project_cols = if tables.projects {
            "p.project_kind, p.workspace_dir"
        } else {
            "NULL, NULL"
        };
        format!(
            "SELECT t.id, t.session_id, t.turn_id, t.ts,
                    t.input_tokens, t.output_tokens, t.reasoning_tokens,
                    t.cache_read_tokens, t.cache_write_tokens, t.cost_usd, t.model,
                    s.title, s.is_default_workspace, s.project_workspace_dir, s.workspace_dir,
                    s.session_kind, s.created_at_ms, s.updated_at_ms, s.extra_data_json,
                    {project_cols}
             FROM local_runtime_token_usage t
             {session_join} {project_join}
             ORDER BY t.ts, t.id"
        )
    } else {
        "SELECT t.id, t.session_id, t.turn_id, t.ts,
                t.input_tokens, t.output_tokens, t.reasoning_tokens,
                t.cache_read_tokens, t.cache_write_tokens, t.cost_usd, t.model,
                NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL
         FROM local_runtime_token_usage t
         ORDER BY t.ts, t.id"
            .to_string()
    }
}

/// 全表扫描用量行（`ORDER BY ts, id`；配对与差集清理都依赖这个稳定顺序）。
fn fetch_usage_rows(
    src: &Connection,
    tables: &SourceTables,
) -> Result<Vec<SourceUsageRow>, String> {
    let sql = usage_query_sql(tables);
    let mut stmt = src
        .prepare(&sql)
        .map_err(|e| format!("准备 MiniMax Code 用量查询失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SourceUsageRow {
                id: row.get(0)?,
                session_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                turn_id: row.get(2)?,
                ts: row.get(3)?,
                input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                reasoning_tokens: row.get(6)?,
                cache_read_tokens: row.get(7)?,
                cache_write_tokens: row.get(8)?,
                cost_usd: row.get(9)?,
                model: row.get(10)?,
                title: row.get(11)?,
                is_default_workspace: row.get(12)?,
                project_workspace_dir: row.get(13)?,
                workspace_dir: row.get(14)?,
                session_kind: row.get(15)?,
                created_at_ms: row.get(16)?,
                updated_at_ms: row.get(17)?,
                extra_data_json: row.get(18)?,
                project_kind: row.get(19)?,
                project_dir: row.get(20)?,
            })
        })
        .map_err(|e| format!("读取 MiniMax Code 用量失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code 用量失败: {e}"))?;
    Ok(rows)
}

/// 源库一条 assistant 消息行（只取配对需要的列）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AssistantRow {
    pub id: i64,
    pub session_id: String,
    pub turn_id: Option<String>,
    /// `$.context_usage_telemetry.model`（裸模型 id；缺失为 NULL）
    pub telemetry_model: Option<String>,
    /// `$.usage.request_duration_ms`（JSON 非数值时 SQL 侧 CAST 成 0/NULL）
    pub request_duration_ms: Option<f64>,
}

/// 扫描全部 assistant 行（`ORDER BY id`；配对依赖这个稳定顺序）。
///
/// 两处防御：`role='assistant'` 过滤（user/system 行没有用量语义）；
/// `json_valid(data_json)` 过滤——SQLite 的 `json_extract` 遇坏 JSON 会直接报错，
/// 一条脏行不该让整轮导入失败（坏行对应用量行自然退到下一级模型兜底）。
/// 两个 JSON 字段都 CAST 成目标类型：JSON 里存成字符串/数字的脏值不会让
/// rusqlite 读列失败（非数值 CAST 成 0/NULL，而不是抛 InvalidColumnType；
/// 数值字符串会被 CAST 成数，属容错而非脏值）。
fn fetch_assistant_rows(src: &Connection) -> Result<Vec<AssistantRow>, String> {
    let mut stmt = src
        .prepare(
            "SELECT id, session_id, turn_id,
                    CAST(json_extract(data_json,'$.context_usage_telemetry.model') AS TEXT),
                    CAST(json_extract(data_json,'$.usage.request_duration_ms') AS REAL)
             FROM local_runtime_message_rows
             WHERE role = 'assistant' AND json_valid(data_json)
             ORDER BY id",
        )
        .map_err(|e| format!("准备 MiniMax Code assistant 行查询失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(AssistantRow {
                id: row.get(0)?,
                session_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                turn_id: row.get(2)?,
                telemetry_model: row
                    .get::<_, Option<String>>(3)?
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                request_duration_ms: row.get(4)?,
            })
        })
        .map_err(|e| format!("读取 MiniMax Code assistant 行失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code assistant 行失败: {e}"))?;
    Ok(rows)
}

// ============================================================
// 纯函数：归一 / 拆分 / 维度解析
// ============================================================

/// token 归一（**全模块唯一的计费口径校准点**）。
///
/// 返回值顺序：`(input_tokens, output_tokens, cache_read, cache_write, reasoning)`
/// （前两项是落库口径，后三项原样透传）。映射依据见模块头三条证据：
/// - `input  = raw_input + raw_cache_read` ← 源库 input **不含** cache read，
///   而项目 `cost_for` 假定 `input_tokens` 已含 cache_read；
/// - `output = raw_output` ← 源库 output **已含** reasoning（app 自己的计费函数
///   里没有独立 reasoning 项），再加一次就是双重计费；
/// - `reasoning` 原样返回，但**它是 output 的子集**，勿再与 output 相加当总量。
///
/// 若某天确认某版本的 input 已含 cache read，只需改下面第一个式子的 `+`，
/// 其余代码与测试语义不变。
pub(crate) fn normalize_tokens(
    raw_input: i64,
    raw_output: i64,
    raw_cache_read: i64,
    raw_cache_write: i64,
    raw_reasoning: i64,
) -> (i64, i64, i64, i64, i64) {
    // ↓↓↓ 唯一校准点：源库 input 是否已含 cache read ↓↓↓
    let input_tokens = raw_input + raw_cache_read;
    // ↓↓↓ 第二个校准点：源库 output 是否已含 reasoning（本来源：已含，不加）↓↓↓
    let output_tokens = raw_output;
    (
        input_tokens,
        output_tokens,
        raw_cache_read,
        raw_cache_write,
        raw_reasoning,
    )
}

/// 前缀是否「像 provider id」：非空、≤32 字符、首字符是 ASCII 小写字母或数字、
/// 其余只含 `[a-z0-9._-]`、且至少有一个 ASCII 字母。
///
/// 为什么需要这个守卫：模型 id 本身可以自带命名空间（`Qwen/Qwen3-32B`、
/// `meta-llama/Llama-3-70B`），把它们的首段当 provider 会造出假 provider。
/// 本项目里 provider id 一律是小写 slug（minimax / opencode-go /
/// deepseek-account / zhipu ...），大写或含空格的段只可能是模型名的一部分。
fn provider_like(prefix: &str) -> bool {
    !prefix.is_empty()
        && prefix.len() <= 32
        && prefix
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            .unwrap_or(false)
        && prefix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && prefix.chars().any(|c| c.is_ascii_alphabetic())
}

/// `/` 拆分（返回 `(model_id, provider_id)`）。
///
/// - `guard = true`（源表 `t.model` / 遥测 model 用）：仅当前缀「像 provider」
///   时才拆，否则整体当 model_id（不臆造 provider）；
/// - `guard = false`（会话 `effectiveModel` 用）：无条件按**第一个** `/` 拆
///   ——app 自己保证该字段恒为 `<providerId>/<modelId>`，模型段里再出现 `/`
///   也仍归模型（`split_once` 只切第一刀）。
///
/// 无 `/`、前缀为空（`"/x"`）或模型段为空（`"x/"`）时整体当 model_id、
/// provider 留空——**宁可留空也不猜**。
pub(crate) fn split_provider_model(raw: &str, guard: bool) -> (String, String) {
    let raw = raw.trim();
    if let Some((prefix, rest)) = raw.split_once('/') {
        let rest = rest.trim();
        if !rest.is_empty() && (!guard || provider_like(prefix)) {
            return (rest.to_string(), prefix.to_string());
        }
    }
    (raw.to_string(), String::new())
}

/// 从会话 `extra_data_json` 取 `effectiveModel`（`<providerId>/<modelId>`）。
/// 空串 / 非法 JSON / 字段缺失一律 None（不报错：脏会话元信息不该阻断导入）。
pub(crate) fn effective_model(extra_data_json: Option<&str>) -> Option<String> {
    let raw = extra_data_json?;
    let value: Value = serde_json::from_str(raw).ok()?;
    value
        .get("effectiveModel")
        .and_then(|m| m.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 逐行模型解析阶梯（见模块头）。返回 `(model_id, provider_id)`。
pub(crate) fn resolve_model(
    row_model: Option<&str>,
    telemetry_model: Option<&str>,
    session_effective_model: Option<&str>,
) -> (String, String) {
    // (a) 源表 model 列（实测恒 NULL，但形态上最权威：它就是这一次调用记的模型）
    if let Some(raw) = row_model.map(str::trim).filter(|s| !s.is_empty()) {
        return split_provider_model(raw, true);
    }
    // (b) 配对成功的 assistant 行遥测模型（裸 id，通常无 provider）。若它与会话
    //     `effectiveModel` 的**模型段逐字相同**，就把会话那段的 provider 补上——
    //     这是「同一个模型名」的等值判定，不是猜：app 保证 effectiveModel 是
    //     `<providerId>/<modelId>`，模型名相同即这次调用走的就是该 provider 配置。
    //     （不补的话，实测数据里几乎**每一行**的 provider_id 都是空的：阶梯 (b)
    //     在单轮会话里必然命中，阶梯 (c) 根本没机会跑。）
    //     会话中途换过模型时遥测模型 ≠ 会话模型段 → provider 仍留空，
    //     绝不张冠李戴。
    if let Some(raw) = telemetry_model.map(str::trim).filter(|s| !s.is_empty()) {
        let (model_id, provider_id) = split_provider_model(raw, true);
        if provider_id.is_empty() {
            if let Some(effective) = session_effective_model
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let (session_model, session_provider) = split_provider_model(effective, false);
                if !session_provider.is_empty() && session_model == model_id {
                    return (model_id, session_provider);
                }
            }
        }
        return (model_id, provider_id);
    }
    // (c) 会话 effectiveModel（app 保证是 "<providerId>/<modelId>"）
    if let Some(raw) = session_effective_model
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return split_provider_model(raw, false);
    }
    // (d) 都取不到：空 model + 空 provider（不臆造 provider，也不编造模型名）
    (String::new(), String::new())
}

/// 采纳配对得到的请求耗时：有限、正数、且不超过 `MAX_DURATION_MS` 才要，
/// 否则 NULL（绝不用 0 冒充「瞬时完成」）。
fn accept_duration(raw: Option<f64>) -> Option<f64> {
    raw.filter(|v| v.is_finite() && *v > 0.0 && *v <= MAX_DURATION_MS as f64)
}

/// 源库自算成本：只认有限非负数值（字符串/负数脏值忽略，口径同 opencode）。
/// 缺字段 → None，区分「免费模型 cost=0」与「源库没给 cost」两种情况。
fn accept_reported_cost(raw: Option<f64>) -> Option<f64> {
    raw.filter(|c| c.is_finite() && *c >= 0.0)
}

/// 会话的原始工作区路径：`project_workspace_dir` 优先，回退 `workspace_dir`，
/// 再回退项目行的 `workspace_dir`（仅 `project_kind='workspace'` 时；`default`
/// 行的该列被 CHECK 约束保证为 NULL）。空白串一律按缺失处理。
fn session_workspace(row: &SourceUsageRow) -> Option<String> {
    let clean = |v: &Option<String>| -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };
    clean(&row.project_workspace_dir)
        .or_else(|| clean(&row.workspace_dir))
        .or_else(|| {
            let is_project_workspace = row
                .project_kind
                .as_deref()
                .map(|k| k == "workspace")
                .unwrap_or(false);
            if is_project_workspace {
                clean(&row.project_dir)
            } else {
                None
            }
        })
}

/// 会话的「真实项目目录」：默认工作区与合成的按会话工作区都不算项目（见模块头）。
/// 返回原始路径（保留大小写，供展示）；判定用归一化后的路径比较。
fn project_workspace(row: &SourceUsageRow, root: &Path) -> Option<String> {
    if row.is_default_workspace.unwrap_or(0) != 0 {
        return None;
    }
    let candidate = session_workspace(row)?;
    let synthetic = root
        .join("sessions")
        .join(&row.session_id)
        .join("workspace");
    let same = match (
        crate::projects::normalize_cwd(&candidate),
        crate::projects::normalize_cwd(&synthetic.to_string_lossy()),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    if same {
        return None;
    }
    Some(candidate)
}

// ============================================================
// 行构造与对账落库
// ============================================================

/// 落库前的单行用量（口径已归一，见 `normalize_tokens`）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UsageRecord {
    pub dedupe_key: String,
    pub session_id: String,
    pub started_at: i64,
    pub model_id: String,
    pub provider_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
    pub computed_total: i64,
    pub duration_ms: Option<f64>,
    pub reported_cost_usd: Option<f64>,
    pub cwd: Option<String>,
    pub project_key: Option<String>,
}

/// 会话维度快照（`session_meta` 表的一行）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionMetaRow {
    pub session_id: String,
    pub title: Option<String>,
    pub session_kind: String,
    pub directory: Option<String>,
    pub project_key: Option<String>,
    pub is_default_workspace: bool,
    pub project_kind: String,
    pub time_created: i64,
    pub time_updated: i64,
}

/// 一轮扫描的产物。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ScanResult {
    pub records: Vec<UsageRecord>,
    pub sessions: Vec<SessionMetaRow>,
}

/// `("session", "turn")` 配对键：`turn_id` 为 NULL/空统一成空串（仍可成组，
/// 顺序配对的正确性由「数量相等」守卫保证）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TurnKey {
    session_id: String,
    turn_id: String,
}

fn turn_key(session_id: &str, turn_id: Option<&str>) -> TurnKey {
    TurnKey {
        session_id: session_id.to_string(),
        turn_id: turn_id.unwrap_or("").trim().to_string(),
    }
}

/// 把源库用量行 + assistant 行解析成落库行（纯函数，便于单测直接喂真实数据）。
///
/// 配对规则（模块头阶梯 (b)）：按 `(session_id, turn_id)` 分组，**仅当该分组的
/// 用量行数与 assistant 行数完全相等**时，把两组各按稳定顺序（用量 `ts,id`、
/// assistant `id`）一一配对，取遥测模型与请求耗时；数量不等时整组不配对
/// （宁可退到会话级兜底，也不猜错配对关系）。
pub(crate) fn build_scan(
    rows: &[SourceUsageRow],
    assistants: &[AssistantRow],
    root: &Path,
) -> ScanResult {
    let mut usage_groups: HashMap<TurnKey, Vec<usize>> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        usage_groups
            .entry(turn_key(&row.session_id, row.turn_id.as_deref()))
            .or_default()
            .push(index);
    }
    let mut assistant_groups: HashMap<TurnKey, Vec<&AssistantRow>> = HashMap::new();
    for assistant in assistants {
        assistant_groups
            .entry(turn_key(&assistant.session_id, assistant.turn_id.as_deref()))
            .or_default()
            .push(assistant);
    }

    // 配对结果：用量行下标 → (遥测模型, 请求耗时)
    let mut paired: HashMap<usize, (Option<String>, Option<f64>)> = HashMap::new();
    for (key, indices) in &usage_groups {
        let Some(group) = assistant_groups.get(key) else {
            continue;
        };
        if group.len() != indices.len() {
            continue; // 数量不等 → 整组不配对
        }
        // 用量行按 (ts, id) 升序（源查询已是该顺序，这里显式排序防调用方乱序）
        let mut indices = indices.clone();
        indices.sort_by_key(|i| (rows[*i].ts, rows[*i].id));
        for (slot, index) in indices.iter().enumerate() {
            let assistant = group[slot];
            paired.insert(
                *index,
                (
                    assistant.telemetry_model.clone(),
                    accept_duration(assistant.request_duration_ms),
                ),
            );
        }
    }

    let mut scan = ScanResult::default();
    // 会话维度：每会话一条（同一会话的会话级字段在所有行上必然相同，取首次出现）
    let mut session_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // 会话 effectiveModel 解析缓存（每会话只解析一次 JSON）
    let mut effective_cache: HashMap<String, Option<String>> = HashMap::new();

    for (index, row) in rows.iter().enumerate() {
        if row.session_id.is_empty() {
            continue; // 无会话 id 的行无法落会话维度，跳过而不是归到空 id 上
        }
        let effective = effective_cache
            .entry(row.session_id.clone())
            .or_insert_with(|| effective_model(row.extra_data_json.as_deref()))
            .clone();
        // 项目维度：原始路径原样存（展示），归一化后的键供聚合（每行只算一次）
        let project = project_workspace(row, root);
        let project_key = project.as_deref().and_then(crate::projects::normalize_cwd);
        if session_seen.insert(row.session_id.clone()) {
            let workspace = session_workspace(row);
            scan.sessions.push(SessionMetaRow {
                session_id: row.session_id.clone(),
                title: row
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                session_kind: row.session_kind.clone().unwrap_or_default(),
                directory: workspace,
                project_key: project_key.clone(),
                is_default_workspace: row.is_default_workspace.unwrap_or(0) != 0,
                project_kind: row.project_kind.clone().unwrap_or_default(),
                time_created: row.created_at_ms.unwrap_or(0),
                time_updated: row.updated_at_ms.unwrap_or(0),
            });
        }

        let (input_tokens, output_tokens, cache_read, cache_write, reasoning) = normalize_tokens(
            row.input_tokens,
            row.output_tokens,
            row.cache_read_tokens,
            row.cache_write_tokens,
            row.reasoning_tokens,
        );
        let (telemetry_model, duration_ms) = paired.get(&index).cloned().unwrap_or((None, None));
        let (model_id, provider_id) = resolve_model(
            row.model.as_deref(),
            telemetry_model.as_deref(),
            effective.as_deref(),
        );
        scan.records.push(UsageRecord {
            // 稳定去重键：三段都来自源库且不会被 app 改写（turn_id 缺失时用空串段）
            dedupe_key: format!(
                "{}|{}|{}",
                row.session_id,
                row.turn_id.as_deref().unwrap_or("").trim(),
                row.id
            ),
            session_id: row.session_id.clone(),
            started_at: row.ts,
            model_id,
            provider_id,
            input_tokens,
            output_tokens,
            cache_read,
            cache_write,
            reasoning,
            // 只算一次：reasoning 是 output 的子集（见模块头证据 2）
            computed_total: input_tokens + output_tokens,
            duration_ms,
            reported_cost_usd: accept_reported_cost(row.cost_usd),
            cwd: project,
            project_key,
        });
    }
    scan
}

/// 读取派生库里的源级水位（首轮返回 None）。
fn load_progress(conn: &Connection) -> Result<Option<SourceFingerprint>, String> {
    conn.query_row(
        "SELECT usage_count, usage_max_id, usage_max_ts, session_count, session_max_updated
         FROM source_progress WHERE source = ?1",
        [PROGRESS_KEY],
        |row| {
            Ok(SourceFingerprint {
                usage_count: row.get(0)?,
                usage_max_id: row.get(1)?,
                usage_max_ts: row.get(2)?,
                session_count: row.get(3)?,
                session_max_updated: row.get(4)?,
            })
        },
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(format!("读取 MiniMax Code 导入水位失败: {other}")),
    })
}

/// 单轮导入的结果（诊断与单测断言用）。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ImportOutcome {
    /// 源库形态标记："runtime-state"（有用量表）/ "none"（没有）
    pub shape: String,
    /// 源库用量行数（本轮扫描到的）
    pub source_rows: i64,
    /// 源库有用量行的会话数
    pub source_sessions: i64,
    /// 指纹未变 → 整轮跳过（派生库内容与自增 id 都不动）
    pub skipped: bool,
    /// 本轮写入（原地 upsert）的用量行数
    pub records_written: i64,
    /// 幽灵行清理删掉的行数
    pub ghosts_removed: i64,
}

/// 对账落库：**同一事务**内完成「原地 upsert 全部扫到的行 → 差集清幽灵行 →
/// 会话维度 upsert → 水位写入」。
///
/// 幂等保证：存活行的 `dedupe_key` 不变 ⇒ 自增 id 不变（同步上传的服务端按
/// `(device_id, source, local_rowid)` 去重，换 id 会让同一笔用量在服务端多出一份）；
/// 重复导入同一份源数据时行内容、行数、id 全都一致。见模块头「增量口径」。
fn write_scan(
    conn: &mut Connection,
    scan: &ScanResult,
    fingerprint: &SourceFingerprint,
    now: i64,
) -> Result<(i64, i64), String> {
    let tx = conn
        .transaction()
        .map_err(|e| format!("开启 MiniMax Code 导入事务失败: {e}"))?;
    // 本次导入的键集合（临时表，仅本连接可见；事务外无残留）。末尾用它做差集，
    // 找出「上一次导入过、这次源库里已经没有了」的行（含 app 删除会话）。
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS minimaxcode_import_keys (dedupe_key TEXT PRIMARY KEY);
         DELETE FROM minimaxcode_import_keys;",
    )
    .map_err(|e| format!("准备 MiniMax Code 导入键集合失败: {e}"))?;

    for record in &scan.records {
        tx.execute(
            "INSERT OR IGNORE INTO minimaxcode_import_keys (dedupe_key) VALUES (?1)",
            [&record.dedupe_key],
        )
        .map_err(|e| format!("记录 MiniMax Code 导入键失败: {e}"))?;
        tx.execute(
            "INSERT INTO model_usage
                (session_id, dedupe_key, started_at, model_id, provider_id,
                 input_tokens, output_tokens, cache_read_input_tokens,
                 cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                 duration_ms, time_to_first_token_ms, updated_at, cwd, project_key,
                 delegation_depth, reported_cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, ?13, ?14, ?15,
                     0, ?16)
             ON CONFLICT(dedupe_key) DO UPDATE SET
                started_at = excluded.started_at,
                model_id = excluded.model_id,
                provider_id = excluded.provider_id,
                input_tokens = excluded.input_tokens,
                output_tokens = excluded.output_tokens,
                cache_read_input_tokens = excluded.cache_read_input_tokens,
                cache_creation_input_tokens = excluded.cache_creation_input_tokens,
                reasoning_tokens = excluded.reasoning_tokens,
                computed_total_tokens = excluded.computed_total_tokens,
                duration_ms = excluded.duration_ms,
                -- updated_at 是**同步修订标记**，不是内容版本：只有服务端会真正
                -- 采纳的修订（总量变大，或空模型名被回填）才推进它。否则每次整表
                -- 重扫都会把全部行标记成「已修订」，同步侧每轮都要全量重传
                -- （服务端对未变行是无操作，纯浪费带宽）。判定条件与服务端
                -- server/db.py 的覆盖守卫逐条对齐。
                updated_at = CASE
                    WHEN excluded.computed_total_tokens > model_usage.computed_total_tokens
                      OR (COALESCE(model_usage.model_id, '') = ''
                          AND COALESCE(excluded.model_id, '') != '')
                    THEN ?13
                    ELSE model_usage.updated_at
                END,
                cwd = excluded.cwd,
                project_key = excluded.project_key,
                reported_cost_usd = excluded.reported_cost_usd",
            rusqlite::params![
                record.session_id,
                record.dedupe_key,
                record.started_at,
                record.model_id,
                record.provider_id,
                record.input_tokens,
                record.output_tokens,
                record.cache_read,
                record.cache_write,
                record.reasoning,
                record.computed_total,
                record.duration_ms,
                now,
                record.cwd,
                record.project_key,
                record.reported_cost_usd,
            ],
        )
        .map_err(|e| format!("写入 MiniMax Code 用量记录失败: {e}"))?;
    }

    // 幽灵行清理：源库里已经不存在的行（app 删会话 / 删行）派生库里不该继续留着，
    // 否则本地统计会比源库多算。**全表差集**（本来源是整表重扫，不是逐会话重导）：
    // 键仍存在的行保持原 id（这正是消灭服务端重复计数的前提，见函数注释）。
    let ghosts = tx
        .execute(
            "DELETE FROM model_usage
              WHERE dedupe_key NOT IN (SELECT dedupe_key FROM minimaxcode_import_keys)",
            [],
        )
        .map_err(|e| format!("清理 MiniMax Code 幽灵行失败: {e}"))? as i64;

    for meta in &scan.sessions {
        tx.execute(
            "INSERT INTO session_meta
                (session_id, title, session_kind, directory, project_key,
                 is_default_workspace, project_kind, time_created, time_updated, captured_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(session_id) DO UPDATE SET
                title = excluded.title,
                session_kind = excluded.session_kind,
                directory = excluded.directory,
                project_key = excluded.project_key,
                is_default_workspace = excluded.is_default_workspace,
                project_kind = excluded.project_kind,
                time_created = excluded.time_created,
                time_updated = excluded.time_updated,
                captured_at = excluded.captured_at",
            rusqlite::params![
                meta.session_id,
                meta.title,
                meta.session_kind,
                meta.directory,
                meta.project_key,
                meta.is_default_workspace as i64,
                meta.project_kind,
                meta.time_created,
                meta.time_updated,
                now,
            ],
        )
        .map_err(|e| format!("写入 MiniMax Code 会话元信息失败: {e}"))?;
    }

    tx.execute(
        "INSERT INTO source_progress
            (source, usage_count, usage_max_id, usage_max_ts, session_count,
             session_max_updated, imported_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(source) DO UPDATE SET
            usage_count = excluded.usage_count,
            usage_max_id = excluded.usage_max_id,
            usage_max_ts = excluded.usage_max_ts,
            session_count = excluded.session_count,
            session_max_updated = excluded.session_max_updated,
            imported_at = excluded.imported_at",
        rusqlite::params![
            PROGRESS_KEY,
            fingerprint.usage_count,
            fingerprint.usage_max_id,
            fingerprint.usage_max_ts,
            fingerprint.session_count,
            fingerprint.session_max_updated,
            now,
        ],
    )
    .map_err(|e| format!("记录 MiniMax Code 导入水位失败: {e}"))?;

    tx.commit()
        .map_err(|e| format!("提交 MiniMax Code 导入事务失败: {e}"))?;
    Ok((ghosts, scan.records.len() as i64))
}

/// 增量导入内核（源库/派生库连接与数据根注入，不碰任何全局路径；生产与单测共用）。
///
/// 幂等保证：同一份源数据重复导入时第二次指纹与水位完全一致 → 整轮跳过 →
/// 派生库内容与自增 id 全都不变；即便指纹变了（例如只剩会话维度变化），
/// 原地 upsert 也只会覆盖内容，存活行 id 不漂移（见 `write_scan`）。
pub(crate) fn import_from_source(
    src: &Connection,
    conn: &mut Connection,
    root: &Path,
    now: i64,
) -> Result<ImportOutcome, String> {
    if !table_exists(src, USAGE_TABLE) {
        // 库在但没有用量表：不是错误，静默（页签由 presence 决定不出现）
        return Ok(ImportOutcome {
            shape: "none".to_string(),
            ..Default::default()
        });
    }
    let tables = detect_tables(src);
    let fingerprint = collect_fingerprint(src, &tables)?;
    if load_progress(conn)? == Some(fingerprint) {
        // 两个指纹（用量表 + 会话表）都未变 → 连扫描都省掉
        return Ok(ImportOutcome {
            shape: "runtime-state".to_string(),
            source_rows: fingerprint.usage_count,
            skipped: true,
            ..Default::default()
        });
    }
    let rows = fetch_usage_rows(src, &tables)?;
    let assistants = if tables.message_rows {
        fetch_assistant_rows(src)?
    } else {
        Vec::new()
    };
    let scan = build_scan(&rows, &assistants, root);
    let (ghosts_removed, records_written) = write_scan(conn, &scan, &fingerprint, now)?;
    Ok(ImportOutcome {
        shape: "runtime-state".to_string(),
        source_rows: rows.len() as i64,
        source_sessions: scan.sessions.len() as i64,
        skipped: false,
        records_written,
        ghosts_removed,
    })
}

// ============================================================
// 增量导入入口（5 秒节流）
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

/// 增量导入（不节流）：打开只读源库 + 派生库，交给 `import_from_source`。
/// 未装 MiniMax Code（源库不存在）或源库没有用量表时静默返回（不是错误）。
pub fn import_incremental_force() -> Result<(), String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = source_db_path();
    if !path.exists() {
        return Ok(());
    }
    let src = open_source_readonly(&path)?;
    if !table_exists(&src, USAGE_TABLE) {
        return Ok(()); // 不在无数据时白建一个空派生库
    }
    let mut conn = open_minimaxcode_db()?;
    import_from_source(&src, &mut conn, &data_root(), crate::provider_quota::now_ms())?;
    Ok(())
}

// ============================================================
// 查询（与 opencode.rs / dsh.rs 同名同构；查询前先增量导入）
// ============================================================

/// 查询 [from_ms, to_ms) 内的统计。
///
/// **无 TTFT**：本来源只有整请求耗时（`duration_ms`），`time_to_first_token_ms`
/// 列恒 NULL，故生成窗口退化为请求总耗时，速度质量标 `RequestAverage`
/// （前端带 ≈ 前缀），不冒充 Generation。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_stats_at(&conn, from_ms, to_ms)
}

/// `query_stats` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_stats_at(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
) -> Result<db::Stats, String> {
    // 校准点：本库 ttft 列恒 NULL（源库没有首字时刻）。若将来从别的字段确认了
    // 首字时刻并回填该列，把下面的 false 换成 db::has_column(...) 即可切换口径。
    let has_ttft = false;
    let speed = db::speed_agg_columns(
        db::has_column(conn, "model_usage", "duration_ms"),
        has_ttft,
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
        .map_err(|e| format!("查询 MiniMax Code 整体统计失败: {e}"))?;

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
        .map_err(|e| format!("准备 MiniMax Code 模型分组查询失败: {e}"))?;

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
        .map_err(|e| format!("读取 MiniMax Code 模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code 模型分组失败: {e}"))?;

    // 速度质量：本库只有请求总耗时，没有真生成窗口 → 请求平均（前端带 ≈）
    if overall.speed.avg_tps.is_some() {
        overall.speed.speed_quality = Some(crate::token_speed::SpeedQuality::RequestAverage);
    }
    for model in &mut by_model {
        if model.speed.avg_tps.is_some() {
            model.speed.speed_quality = Some(crate::token_speed::SpeedQuality::RequestAverage);
        }
    }

    let (earliest_ms, latest_ms): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(started_at), MAX(started_at) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("查询 MiniMax Code 时间范围失败: {e}"))?;

    Ok(db::Stats {
        from_ms,
        to_ms,
        overall,
        by_model,
        earliest_ms,
        latest_ms,
        current_model: db::query_current_model(conn),
    })
}

/// 查询 [from_ms, to_ms) 内的分桶统计；bucket 为 "hour" 或 "day"。
/// 桶数上限见 `MAX_TREND_BUCKETS`（防 `to_ms = i64::MAX` 之类的无界调用卡死）。
pub fn query_trend(
    from_ms: i64,
    to_ms: i64,
    bucket: &str,
) -> Result<Vec<db::TrendBucketRaw>, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_trend_at(&conn, from_ms, to_ms, bucket)
}

/// `query_trend` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_trend_at(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
    bucket: &str,
) -> Result<Vec<db::TrendBucketRaw>, String> {
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
        .map_err(|e| format!("准备 MiniMax Code 趋势查询失败: {e}"))?;

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
            .map_err(|e| format!("读取 MiniMax Code 趋势统计失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 MiniMax Code 趋势统计失败: {e}"))?;

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
    let conn = open_minimaxcode_db()?;
    list_models_at(&conn)
}

/// `list_models` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn list_models_at(conn: &Connection) -> Result<Vec<db::ModelInfo>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT provider_id, model_id
             FROM model_usage
             ORDER BY model_id",
        )
        .map_err(|e| format!("准备 MiniMax Code 模型列表查询失败: {e}"))?;
    let models = stmt
        .query_map([], |row| {
            Ok(db::ModelInfo {
                provider_id: row.get(0)?,
                model_id: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取 MiniMax Code 模型列表失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code 模型列表失败: {e}"))?;
    Ok(models)
}

/// 按指定周期聚合 Token（对比页需要真实 [reset_at, end_at) 边界）。
pub fn query_period_buckets(periods: &[(i64, i64)]) -> Result<Vec<db::PeriodBucket>, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_period_buckets_at(&conn, periods)
}

/// `query_period_buckets` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_period_buckets_at(
    conn: &Connection,
    periods: &[(i64, i64)],
) -> Result<Vec<db::PeriodBucket>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(SUM(computed_total_tokens),0), COUNT(*)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2",
        )
        .map_err(|e| format!("准备 MiniMax Code 周期聚合查询失败: {e}"))?;
    let mut out = Vec::with_capacity(periods.len());
    for &(reset_at, end_at) in periods {
        let (total_tokens, requests): (i64, i64) = stmt
            .query_row(rusqlite::params![reset_at, end_at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("查询 MiniMax Code 周期聚合失败: {e}"))?;
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
/// source 固定 "minimaxcode"，local_rowid = 自增 id；带 proto 5 的会话/项目维度。
pub fn query_since(since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_since_at(&conn, since, limit)
}

/// `query_since` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_since_at(
    conn: &Connection,
    since: i64,
    limit: usize,
) -> Result<Vec<db::UsageRow>, String> {
    query_rows_at(
        conn,
        "WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
        rusqlite::params![since, limit as i64],
    )
}

/// 查询修订行（updated_at > since_ts，同步补传用）。
///
/// 为什么本来源也需要修订通道：整表重扫时**未变行的自增 id 不变**（原地 upsert，
/// 见 `write_scan`），若它的内容被改写（例如模型名从空被回填、总量变大），
/// `id > 游标` 查询永远选不出它，服务端会一直保留旧值。本查询按 `updated_at`
/// （只在上面的 upsert 守卫判定服务端会采纳修订时才推进）选出这些行重传。
/// after_id 分页：按 id 升序、只取 id > after_id 的行，调用方逐批推进。
pub fn query_revised_since(
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_revised_since_at(&conn, since_ts, after_id, limit)
}

/// `query_revised_since` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_revised_since_at(
    conn: &Connection,
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<db::UsageRow>, String> {
    query_rows_at(
        conn,
        "WHERE updated_at > ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
        rusqlite::params![since_ts, after_id, limit as i64],
    )
}

/// `query_since` / `query_revised_since` 共用的行查询（WHERE/分页由调用方给，
/// 两者列口径完全一致，只有过滤条件不同）。
fn query_rows_at(
    conn: &Connection,
    tail: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<db::UsageRow>, String> {
    let sql = format!(
        "SELECT id, started_at, model_id, provider_id,
                input_tokens, output_tokens, cache_read_input_tokens,
                cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                session_id, project_key, cwd
         FROM model_usage {tail}"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 MiniMax Code 明细查询失败: {e}"))?;
    let rows = stmt
        .query_map(params, |row| {
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
                source: "minimaxcode".to_string(),
                session_id: row.get(10)?,
                project_key: row.get(11)?,
                project_display: row.get(12)?,
            })
        })
        .map_err(|e| format!("读取 MiniMax Code 明细失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code 明细失败: {e}"))?;
    Ok(rows)
}

/// 会话悬浮窗（session_hud）用：查询活跃窗口内的会话行。
///
/// 与 opencode 一致：本来源没有 TTFT、也没有「生成窗口」（`duration_ms` 是请求
/// 总耗时），故 `ttft_ms` / `gen_out_tokens` / `gen_ms` 恒空，速度只走请求平均
/// 通道；`generating` 恒 false（源库的会话 `status` 语义未确认，不臆造生成中）。
/// `input_tokens` 在本库**已含** cache_read，故非缓存输入 =
/// `input_tokens − cache_read_input_tokens`。
pub fn query_window_sessions(
    window_start_ms: i64,
    limit: usize,
) -> Result<crate::session_hud::AgentWindowSessions, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_window_sessions_at(&conn, window_start_ms, limit)
}

/// `query_window_sessions` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_window_sessions_at(
    conn: &Connection,
    window_start_ms: i64,
    limit: usize,
) -> Result<crate::session_hud::AgentWindowSessions, String> {
    /// 内层候选行上限（与 claude/dsh/opencode 同量级：限制成本，不全表扫）。
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
            COALESCE(SUM(CASE WHEN duration_ms > 0 THEN duration_ms ELSE 0.0 END), 0.0)
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
        .map_err(|e| format!("准备 MiniMax Code 会话窗口查询失败: {e}"))?;
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
                    // 本来源无运行中标记可读，不臆造
                    generating: false,
                    // 无 TTFT 来源（见模块头），恒空
                    ttft_ms: None,
                    // 无真生成窗口：留空让 session_hud 走请求平均通道
                    gen_out_tokens: 0,
                    gen_ms: 0.0,
                })
            },
        )
        .map_err(|e| format!("读取 MiniMax Code 会话窗口失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code 会话窗口失败: {e}"))?;
    let total_active: usize = conn
        .query_row(
            "SELECT COUNT(DISTINCT session_id) FROM model_usage WHERE started_at >= ?1",
            [window_start_ms],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as usize)
        .map_err(|e| format!("统计 MiniMax Code 活跃会话数失败: {e}"))?;
    Ok(crate::session_hud::AgentWindowSessions {
        total_active,
        sessions,
    })
}

/// 按项目 × 模型聚合 [from_ms, to_ms) 的用量（SQL 侧 GROUP BY，不逐行加载）。
/// project_key 为 NULL/空串（默认工作区、合成工作区或无目录）归入 unknown 哨兵键。
pub fn query_project_model_rows(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<crate::projects::ProjectModelRow>, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_project_model_rows_at(&conn, from_ms, to_ms)
}

/// `query_project_model_rows` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_project_model_rows_at(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<crate::projects::ProjectModelRow>, String> {
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
        .map_err(|e| format!("准备 MiniMax Code 项目聚合查询失败: {e}"))?;
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
        .map_err(|e| format!("读取 MiniMax Code 项目聚合失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code 项目聚合失败: {e}"))?;
    Ok(rows)
}

/// 按项目统计 [from_ms, to_ms) 内的会话数（COUNT(DISTINCT session_id)）。
pub fn query_project_session_counts(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<(String, i64)>, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_project_session_counts_at(&conn, from_ms, to_ms)
}

/// `query_project_session_counts` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_project_session_counts_at(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<(String, i64)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(NULLIF(project_key, ''), ?1), COUNT(DISTINCT session_id)
             FROM model_usage
             WHERE started_at >= ?2 AND started_at < ?3
             GROUP BY 1",
        )
        .map_err(|e| format!("准备 MiniMax Code 项目会话统计失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![crate::projects::UNKNOWN_PROJECT, from_ms, to_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("读取 MiniMax Code 项目会话统计失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code 项目会话统计失败: {e}"))?;
    Ok(rows)
}

/// 项目键 → 原始形态 cwd（保留大小写，取字典序最小者做代表）。
pub fn query_project_display_paths() -> Result<Vec<(String, String)>, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_project_display_paths_at(&conn)
}

/// `query_project_display_paths` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_project_display_paths_at(
    conn: &Connection,
) -> Result<Vec<(String, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT project_key, MIN(cwd)
             FROM model_usage
             WHERE project_key IS NOT NULL AND project_key != ?1
               AND cwd IS NOT NULL AND cwd != ''
             GROUP BY project_key",
        )
        .map_err(|e| format!("准备 MiniMax Code 项目路径查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![crate::projects::UNKNOWN_PROJECT], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("读取 MiniMax Code 项目路径失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code 项目路径失败: {e}"))?;
    Ok(rows)
}

/// 分页查询指定项目的会话明细（按会话最后活跃时间降序）。
/// 返回 (匹配会话总数, 按会话 × 模型聚合的行)；offset/limit 作用在会话粒度。
/// project_key 传 UNKNOWN_PROJECT 时匹配 NULL/空串/哨兵的会话。
/// 与 opencode 一致：无 TTFT（`has_ttft = false`），会话级速度是请求平均口径。
pub fn query_project_sessions(
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Result<(u32, Vec<crate::projects::ProjectSessionModelRow>), String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_project_sessions_at(&conn, project_key, from_ms, to_ms, offset, limit)
}

/// `query_project_sessions` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_project_sessions_at(
    conn: &Connection,
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Result<(u32, Vec<crate::projects::ProjectSessionModelRow>), String> {
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
        .map_err(|e| format!("查询 MiniMax Code 项目会话总数失败: {e}"))?;

    let speed = db::session_speed_agg_columns(
        db::has_column(conn, "model_usage", "duration_ms"),
        false, // ttft 列恒 NULL，见模块头
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
        .map_err(|e| format!("准备 MiniMax Code 项目会话查询失败: {e}"))?;
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
                    // 生成口径原料本来源给不出（无真生成窗口），恒空
                    gen_out: None,
                    gen_ms: None,
                    gen_count: 0,
                })
            },
        )
        .map_err(|e| format!("读取 MiniMax Code 项目会话失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MiniMax Code 项目会话失败: {e}"))?;
    Ok((total.max(0) as u32, rows))
}

/// 库内最大自增 id（「待上传条数」展示用，与 claude/codex/dsh/opencode 同款）。
/// 不触发增量导入（只读游标口径，避免为一次计数做全量扫描）。
pub fn max_rowid() -> Result<i64, String> {
    let conn = open_minimaxcode_db()?;
    max_rowid_at(&conn)
}

/// `max_rowid` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn max_rowid_at(conn: &Connection) -> Result<i64, String> {
    let max: Option<i64> = conn
        .query_row("SELECT MAX(id) FROM model_usage", [], |row| row.get(0))
        .map_err(|e| format!("查询 MiniMax Code 最大 rowid 失败: {e}"))?;
    Ok(max.unwrap_or(0))
}

/// 累计的 MiniMax Code 自算成本（USD）汇总，供上层决定「官方口径 vs 价格表重算」。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ReportedCostSummary {
    /// 有 cost 数据的行合计（USD）
    pub total_usd: f64,
    /// 有 cost 数据的行数
    pub priced_rows: i64,
    /// 总行数（差值即「源库没给 cost」的行）
    pub total_rows: i64,
}

/// 查询 [from_ms, to_ms) 内 MiniMax Code 自算成本合计。
pub fn query_reported_cost(from_ms: i64, to_ms: i64) -> Result<ReportedCostSummary, String> {
    import_incremental()?;
    let conn = open_minimaxcode_db()?;
    query_reported_cost_at(&conn, from_ms, to_ms)
}

/// `query_reported_cost` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_reported_cost_at(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
) -> Result<ReportedCostSummary, String> {
    conn.query_row(
        "SELECT COALESCE(SUM(reported_cost_usd), 0.0),
                COUNT(reported_cost_usd),
                COUNT(*)
         FROM model_usage
         WHERE started_at >= ?1 AND started_at < ?2",
        rusqlite::params![from_ms, to_ms],
        |row| {
            Ok(ReportedCostSummary {
                total_usd: row.get(0)?,
                priced_rows: row.get(1)?,
                total_rows: row.get(2)?,
            })
        },
    )
    .map_err(|e| format!("查询 MiniMax Code 自算成本失败: {e}"))
}

// ============================================================
// 诊断
// ============================================================

/// 诊断信息（设置页/排障用）。字段与 `opencode::OpencodeDebugInfo` 逐字段一致
/// （前端复用同款诊断卡片，形状不同会渲染不出来）。
#[derive(Debug, serde::Serialize)]
pub struct MinimaxcodeDebugInfo {
    /// 源库路径（runtime-state.sqlite）
    pub db_path: String,
    pub db_exists: bool,
    /// 源库形态："runtime-state" / "none"
    pub source_shape: String,
    /// 源库有用量行的会话数
    pub source_sessions: i64,
    /// 源库用量行数
    pub source_messages: i64,
    /// 派生库记录数
    pub imported_records: i64,
    /// 派生库最新一条记录时间
    pub latest_record_ms: Option<i64>,
    /// MiniMax Code 自算成本合计（USD）
    pub reported_cost_usd: f64,
    /// 带自算成本的行数（与 imported_records 的差值 = 源库没给 cost 的行）
    pub reported_cost_rows: i64,
}

pub fn debug_info() -> Result<MinimaxcodeDebugInfo, String> {
    let path = source_db_path();
    let exists = path.exists();
    let (source_shape, source_sessions, source_messages) = match open_source_readonly(&path) {
        Ok(conn) => source_debug(&conn),
        Err(_) => ("none".to_string(), 0, 0),
    };
    let (imported_records, latest_record_ms, reported_cost_usd, reported_cost_rows) =
        match open_minimaxcode_db() {
            Ok(conn) => derived_debug(&conn),
            Err(_) => (0, None, 0.0, 0),
        };
    Ok(MinimaxcodeDebugInfo {
        db_path: path.display().to_string(),
        db_exists: exists,
        source_shape,
        source_sessions,
        source_messages,
        imported_records,
        latest_record_ms,
        reported_cost_usd,
        reported_cost_rows,
    })
}

/// 源库侧诊断字段：`(形态, 有用量行的会话数, 用量行数)`。
/// 会话数按**用量表**里出现的会话去重统计（与「有没有用量数据」同口径，
/// 不把只有会话元信息、没有任何 LLM 调用的空会话算进来）。
pub(crate) fn source_debug(conn: &Connection) -> (String, i64, i64) {
    if !table_exists(conn, USAGE_TABLE) {
        return ("none".to_string(), 0, 0);
    }
    let tables = detect_tables(conn);
    match collect_fingerprint(conn, &tables) {
        Ok(fingerprint) => {
            let sessions: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT session_id) FROM local_runtime_token_usage",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            (
                "runtime-state".to_string(),
                sessions,
                fingerprint.usage_count,
            )
        }
        Err(_) => ("runtime-state".to_string(), 0, 0),
    }
}

/// 派生库侧诊断字段：
/// `(记录数, 最新记录时刻, 自算成本合计, 带成本的行数)`。
pub(crate) fn derived_debug(conn: &Connection) -> (i64, Option<i64>, f64, i64) {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM model_usage", [], |row| row.get(0))
        .unwrap_or(0);
    let latest: Option<i64> = conn
        .query_row("SELECT MAX(started_at) FROM model_usage", [], |row| {
            row.get(0)
        })
        .unwrap_or(None);
    let (cost, cost_rows): (f64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(reported_cost_usd), 0.0), COUNT(reported_cost_usd)
             FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0.0, 0));
    (count, latest, cost, cost_rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实样本（用户在本机 MiniMax Code 实跑一轮后从
    /// `~/.minimax/v2/sqlite/runtime-state.sqlite` 原样摘录）：
    /// 第一次调用的用量行 + 对应 assistant 行的遥测块。
    /// 三条计费证据都钉在这组数字上：
    /// - `providerTokens 24964 = input 24775 + cacheRead 189`（input 不含 cache read）；
    /// - 遥测 `usage.total_tokens 25043 = 24775 + 189 + 79`（cache read 只算一次，
    ///   reasoning 已在 output 内）；
    /// - 源行 `cost_usd = 0.0`（免费模型）。
    /// 顺序：`(input, output, reasoning, cache_read, cache_write, cost_usd)`
    /// ——注意与 `normalize_tokens` 的形参顺序不同（那边是
    /// `input, output, cache_read, cache_write, reasoning`）。
    const REAL_USAGE: (i64, i64, i64, i64, i64, f64) = (
        24775, // input_tokens（不含 cache read）
        79,    // output_tokens（已含 reasoning；本样本 reasoning = 0）
        0,     // reasoning_tokens
        189,   // cache_read_tokens
        0,     // cache_write_tokens
        0.0,   // cost_usd
    );
    const REAL_REQUEST_DURATION_MS: f64 = 1952.0;
    const REAL_TELEMETRY_MODEL: &str = "MiniMax-M3.1-Flash-Preview";
    const REAL_SESSION_ID: &str = "mvs_a2a535fab0a54f799391d8c9a986bb04";
    const REAL_TURN_ID: &str = "cc37904c-4537-4efa-86b4-0249d07a96b4";
    const REAL_TS: i64 = 1_791_832_993_857;

    /// assistant 行 `data_json`（只保留断言用到的字段，其余原样略去）。
    fn real_assistant_json() -> String {
        format!(
            r#"{{"usage":{{"total_tokens":25043,"context_window":512000,"input_tokens":24775,
                "output_tokens":79,"cache_read":189,"decode_duration_ms":475,
                "request_duration_ms":{REAL_REQUEST_DURATION_MS}}},
                "context_usage_telemetry":{{"trigger":"terminal","turnId":"{REAL_TURN_ID}",
                "model":"{REAL_TELEMETRY_MODEL}","localTokens":24962,"providerTokens":24964}}}}"#
        )
    }

    /// 每个测试独立的临时目录（互不干扰；结束前删除）。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zbar-minimaxcode-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir
    }

    /// 建**真实表结构**的源库（列名/约束与实测库一致）。
    fn create_source(path: &Path) -> Connection {
        let conn = Connection::open(path).expect("打开源库失败");
        conn.execute_batch(
            "CREATE TABLE local_runtime_token_usage (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL,
                 agent_name TEXT NOT NULL,
                 framework_type TEXT NOT NULL,
                 turn_id TEXT,
                 model TEXT,
                 ts INTEGER NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 reasoning_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 cache_write_tokens INTEGER NOT NULL,
                 cost_usd REAL,
                 raw TEXT
             );
             CREATE TABLE local_runtime_sessions (
                 session_id TEXT PRIMARY KEY,
                 record_json TEXT NOT NULL,
                 updated_at_ms INTEGER NOT NULL,
                 agent_name TEXT,
                 session_kind TEXT NOT NULL DEFAULT 'unknown',
                 workspace_dir TEXT,
                 project_workspace_dir TEXT,
                 is_default_workspace INTEGER NOT NULL DEFAULT 0,
                 title TEXT,
                 created_at_ms INTEGER,
                 extra_data_json TEXT NOT NULL DEFAULT '{}',
                 project_id INTEGER
             );
             CREATE TABLE local_runtime_message_rows (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL,
                 msg_id TEXT NOT NULL,
                 role TEXT,
                 turn_id TEXT,
                 created_at_ms INTEGER NOT NULL,
                 data_json TEXT NOT NULL,
                 source TEXT,
                 UNIQUE(session_id, msg_id)
             );
             CREATE TABLE local_runtime_projects (
                 project_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 project_kind TEXT NOT NULL,
                 workspace_dir TEXT
             );",
        )
        .expect("建源库失败");
        conn
    }

    /// 插入一条用量行（返回自增 id）。`tokens` 顺序 =
    /// `(input, output, reasoning, cache_read, cache_write)`（源表列顺序）。
    fn insert_usage(
        conn: &Connection,
        session_id: &str,
        turn_id: Option<&str>,
        model: Option<&str>,
        ts: i64,
        tokens: (i64, i64, i64, i64, i64),
        cost: Option<f64>,
    ) -> i64 {
        conn.execute(
            "INSERT INTO local_runtime_token_usage
                (session_id, agent_name, framework_type, turn_id, model, ts,
                 input_tokens, output_tokens, reasoning_tokens, cache_read_tokens,
                 cache_write_tokens, cost_usd)
             VALUES (?1, 'minimax-code', 'local', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                session_id,
                turn_id,
                model,
                ts,
                tokens.0,
                tokens.1,
                tokens.2,
                tokens.3,
                tokens.4,
                cost
            ],
        )
        .expect("插入源用量行失败");
        conn.last_insert_rowid()
    }

    /// 插入一条会话行。
    fn insert_session(
        conn: &Connection,
        session_id: &str,
        title: &str,
        is_default_workspace: i64,
        workspace: &str,
        project_id: Option<i64>,
        extra_data_json: &str,
        updated_at_ms: i64,
    ) {
        conn.execute(
            "INSERT INTO local_runtime_sessions
                (session_id, record_json, updated_at_ms, session_kind, workspace_dir,
                 project_workspace_dir, is_default_workspace, title, created_at_ms,
                 extra_data_json, project_id)
             VALUES (?1, '{}', ?2, 'conversation', ?3, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                session_id,
                updated_at_ms,
                workspace,
                is_default_workspace,
                title,
                updated_at_ms - 1000,
                extra_data_json,
                project_id
            ],
        )
        .expect("插入源会话行失败");
    }

    /// 插入一条 assistant 消息行。
    fn insert_assistant(
        conn: &Connection,
        session_id: &str,
        msg_id: &str,
        turn_id: Option<&str>,
        created_at_ms: i64,
        data_json: &str,
    ) {
        conn.execute(
            "INSERT INTO local_runtime_message_rows
                (session_id, msg_id, role, turn_id, created_at_ms, data_json)
             VALUES (?1, ?2, 'assistant', ?3, ?4, ?5)",
            rusqlite::params![session_id, msg_id, turn_id, created_at_ms, data_json],
        )
        .expect("插入源 assistant 行失败");
    }

    /// 派生库（临时文件，不碰真实 ~/.zbar）。
    fn open_derived(dir: &Path) -> Connection {
        open_minimaxcode_db_at(&dir.join("derived.sqlite")).expect("打开派生库失败")
    }

    /// `model_usage` 的一行快照（不含自增 id，用于幂等比对）。
    #[derive(Debug, Clone, PartialEq)]
    struct RowDump {
        session_id: String,
        dedupe_key: String,
        started_at: i64,
        model_id: String,
        provider_id: String,
        input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        reasoning: i64,
        total: i64,
        duration_ms: Option<f64>,
        reported_cost: Option<f64>,
        cwd: Option<String>,
        project_key: Option<String>,
    }

    fn dump_rows(conn: &Connection) -> Vec<RowDump> {
        let mut stmt = conn
            .prepare(
                "SELECT session_id, dedupe_key, started_at, model_id, provider_id,
                        input_tokens, output_tokens, cache_read_input_tokens,
                        cache_creation_input_tokens, reasoning_tokens,
                        computed_total_tokens, duration_ms, reported_cost_usd,
                        cwd, project_key
                 FROM model_usage ORDER BY session_id, dedupe_key",
            )
            .expect("准备快照查询失败");
        stmt.query_map([], |row| {
            Ok(RowDump {
                session_id: row.get(0)?,
                dedupe_key: row.get(1)?,
                started_at: row.get(2)?,
                model_id: row.get(3)?,
                provider_id: row.get(4)?,
                input: row.get(5)?,
                output: row.get(6)?,
                cache_read: row.get(7)?,
                cache_write: row.get(8)?,
                reasoning: row.get(9)?,
                total: row.get(10)?,
                duration_ms: row.get(11)?,
                reported_cost: row.get(12)?,
                cwd: row.get(13)?,
                project_key: row.get(14)?,
            })
        })
        .expect("查询快照失败")
        .collect::<Result<Vec<_>, _>>()
        .expect("读取快照失败")
    }

    /// 派生库自增 id 列表（幂等断言用：内容未变时连 id 都不该动）。
    fn dump_ids(conn: &Connection) -> Vec<i64> {
        let mut stmt = conn
            .prepare("SELECT id FROM model_usage ORDER BY id")
            .expect("准备 id 查询失败");
        stmt.query_map([], |row| row.get(0))
            .expect("查询 id 失败")
            .collect::<Result<Vec<_>, _>>()
            .expect("读取 id 失败")
    }

    /// 归一映射本身钉在单测里：**input += cache_read，output 不加 reasoning**，
    /// reasoning 原样输出（它是 output 的子集，总量里只出现一次）。
    #[test]
    fn normalize_tokens_pins_the_billing_mapping() {
        assert_eq!(normalize_tokens(10, 5, 3, 2, 4), (13, 5, 3, 2, 4));
        // 全零不 panic、不产生负值（脏负数源数据同理原样透传，不做猜测）
        assert_eq!(normalize_tokens(0, 0, 0, 0, 0), (0, 0, 0, 0, 0));
        // reasoning 是 output 的子集：output 一滴不加，总量只出现一次
        let (input, output, _, _, reasoning) = normalize_tokens(100, 50, 20, 0, 7);
        assert_eq!((input, output), (120, 50));
        assert_eq!(input + output, 170);
        assert!(reasoning <= output);
        // 与 opencode 的差异就在第二项：那边 output += reasoning（其 output 不含
        // reasoning），本来源**不能加**（app 的计费函数没有独立 reasoning 项）。
        assert_ne!(normalize_tokens(100, 50, 20, 0, 7).1, 57);
    }

    /// 真实样本端到端：token 归一后 `input_tokens` 正好等于 app 遥测里的
    /// `providerTokens`（24964），`computed_total` 正好等于遥测的
    /// `usage.total_tokens`（25043）——两条独立数字同时命中，钉死「input 不含
    /// cache read、reasoning 不额外加」；请求耗时取配对 assistant 行的
    /// `request_duration_ms`；自算成本原样落库。
    #[test]
    fn real_sample_pins_normalization_duration_and_reported_cost() {
        // REAL_USAGE 的顺序是 (input, output, reasoning, cacheRead, cacheWrite, cost)，
        // 而 normalize_tokens 收的是 (input, output, cacheRead, cacheWrite, reasoning)
        let raw = REAL_USAGE;
        let (input, output, cache_read, cache_write, reasoning) =
            normalize_tokens(raw.0, raw.1, raw.3, raw.4, raw.2);
        assert_eq!(
            (input, output, cache_read, cache_write, reasoning),
            (24964, 79, 189, 0, 0)
        );
        assert_eq!(input, 24964, "应等于 app 遥测的 providerTokens");
        assert_eq!(input + output, 25043, "应等于 app 遥测的 usage.total_tokens");
        assert_eq!(accept_reported_cost(Some(raw.5)), Some(0.0), "免费模型 cost=0");

        // 耗时只认配对 assistant 行的 request_duration_ms（decode_duration_ms 是
        // 整段解码时长，不是首字延迟，本模块不用它）
        assert_eq!(accept_duration(Some(REAL_REQUEST_DURATION_MS)), Some(1952.0));
        assert_eq!(accept_duration(Some(475.0)), Some(475.0));
        // 脏值：0 / 负数 / 非有限 / 超上界 → NULL（绝不用 0 冒充瞬时完成）
        assert_eq!(accept_duration(Some(0.0)), None);
        assert_eq!(accept_duration(Some(-1.0)), None);
        assert_eq!(accept_duration(Some(f64::NAN)), None);
        assert_eq!(accept_duration(Some(MAX_DURATION_MS as f64 + 1.0)), None);
        assert_eq!(accept_duration(None), None);
    }

    /// `/` 拆分两套规则 + 会话 effectiveModel 解析。
    #[test]
    fn provider_split_and_effective_model_rules() {
        // 守卫版：前缀像 provider 才拆（app 的 providerId 都是小写 slug）
        assert_eq!(
            split_provider_model("minimax/MiniMax-M3.1-Flash-Preview", true),
            (
                "MiniMax-M3.1-Flash-Preview".to_string(),
                "minimax".to_string()
            )
        );
        // 取第一个 '/'：模型段里的 '/' 归模型
        assert_eq!(
            split_provider_model("openrouter/anthropic/claude-3.5", true),
            ("anthropic/claude-3.5".to_string(), "openrouter".to_string())
        );
        // 裸 id：整体当 model_id，provider 留空
        assert_eq!(
            split_provider_model("MiniMax-M3.1-Flash-Preview", true),
            ("MiniMax-M3.1-Flash-Preview".to_string(), String::new())
        );
        // 前缀不像 provider（大写 / 含空格 / 纯数字）→ 不拆，不臆造 provider
        for raw in [
            "Qwen/Qwen3-32B",
            "Meta Llama/Llama-3-70B",
            "123/456",
            "  spaced /model",
        ] {
            let (model, provider) = split_provider_model(raw, true);
            assert_eq!(provider, "", "不应从 {raw} 拆出 provider");
            assert_eq!(model, raw.trim());
        }
        // 空段："/model" 与 "x/" 都整体当 model_id
        assert_eq!(
            split_provider_model("/model", true),
            ("/model".to_string(), String::new())
        );
        assert_eq!(
            split_provider_model("x/", true),
            ("x/".to_string(), String::new())
        );
        // 非守卫版（effectiveModel 专用）：无条件按第一个 '/' 拆
        assert_eq!(
            split_provider_model("MiniMax/M3", false),
            ("M3".to_string(), "MiniMax".to_string())
        );

        // effectiveModel 解析：正常 / 缺失 / 空串 / 坏 JSON 都不 panic
        assert_eq!(
            effective_model(Some(
                r#"{"appMode":"coding","effectiveModel":"minimax/MiniMax-M3.1-Flash-Preview"}"#
            )),
            Some("minimax/MiniMax-M3.1-Flash-Preview".to_string())
        );
        assert_eq!(effective_model(Some("{}")), None);
        assert_eq!(effective_model(Some(r#"{"effectiveModel":"  "}"#)), None);
        assert_eq!(effective_model(Some("not json")), None);
        assert_eq!(effective_model(None), None);
    }

    /// 模型解析阶梯：(a) 源列 > (b) 配对遥测 > (c) 会话 effectiveModel > (d) 空。
    /// provider 宁空不猜（(b) 的裸 id 不借用会话 provider）。
    #[test]
    fn model_resolution_ladder_prefers_source_then_telemetry_then_session() {
        assert_eq!(
            resolve_model(
                Some("minimax/abab6.5s-chat"),
                Some("ignored"),
                Some("minimax/ignored")
            ),
            ("abab6.5s-chat".to_string(), "minimax".to_string())
        );
        assert_eq!(
            resolve_model(
                Some("   "),
                Some(REAL_TELEMETRY_MODEL),
                Some("minimax/MiniMax-M3.1-Flash-Preview")
            ),
            (REAL_TELEMETRY_MODEL.to_string(), "minimax".to_string()),
            "遥测模型与会话模型段逐字相同 → 补上会话级 provider（等值判定，非猜测）"
        );
        assert_eq!(
            resolve_model(
                Some("   "),
                Some(REAL_TELEMETRY_MODEL),
                Some("minimax-cn/Other-Model")
            ),
            (REAL_TELEMETRY_MODEL.to_string(), String::new()),
            "会话中途换过模型（模型段不同）→ provider 留空，绝不张冠李戴"
        );
        assert_eq!(
            resolve_model(Some("   "), Some("Qwen/Qwen3-32B"), Some("minimax/x")),
            ("Qwen/Qwen3-32B".to_string(), String::new()),
            "模型名自带命名空间时不当成 provider 前缀（守卫版拆分）"
        );
        assert_eq!(
            resolve_model(None, None, Some("minimax/MiniMax-M3.1-Flash-Preview")),
            (
                "MiniMax-M3.1-Flash-Preview".to_string(),
                "minimax".to_string()
            )
        );
        assert_eq!(
            resolve_model(None, None, None),
            (String::new(), String::new())
        );
    }

    /// 主链路：真实形态的三个源表 → 归一化 token / 模型归属 / 耗时 / 幂等。
    ///
    /// 覆盖三条模型解析路径与两种耗时来源：
    /// - ses_a 第一轮：源列 NULL + 遥测配对成功 → 裸模型 id、耗时 1952ms；
    /// - ses_a 第二轮：用量行 2 条 vs assistant 行 1 条（数量不等）→ 不配对，
    ///   退到会话 effectiveModel（provider=minimax）、耗时 NULL；
    /// - ses_b：源列直接给了 `provider/model`（最高优先级）。
    #[test]
    fn fixture_import_maps_tokens_models_and_durations() {
        let dir = temp_dir("import");
        let root = dir.join("minimax-root");
        std::fs::create_dir_all(root.join("sessions")).expect("建临时数据根失败");
        let src_path = root.join("runtime-state.sqlite");
        let src = create_source(&src_path);

        let synthetic = root
            .join("sessions")
            .join(REAL_SESSION_ID)
            .join("workspace");
        let synthetic = synthetic.to_string_lossy().to_string();
        insert_session(
            &src,
            REAL_SESSION_ID,
            "并行会话",
            1,
            &synthetic,
            Some(1),
            r#"{"appMode":"coding","effectiveModel":"minimax/MiniMax-M3.1-Flash-Preview"}"#,
            1_791_832_996_056,
        );
        insert_session(
            &src,
            "mvs_real_project",
            "真实项目会话",
            0,
            "C:/work/demo",
            Some(2),
            r#"{"effectiveModel":"minimax/MiniMax-M3.1-Flash-Preview"}"#,
            1_791_833_000_000,
        );
        src.execute_batch(
            "INSERT INTO local_runtime_projects (project_id, project_kind, workspace_dir)
                 VALUES (1, 'default', NULL), (2, 'workspace', 'C:/work/demo');",
        )
        .expect("插入源项目行失败");

        // ses_a 第一轮：与 assistant 行 1:1 配对
        insert_usage(
            &src,
            REAL_SESSION_ID,
            Some(REAL_TURN_ID),
            None,
            REAL_TS,
            (24775, 79, 0, 189, 0),
            Some(0.0),
        );
        insert_assistant(
            &src,
            REAL_SESSION_ID,
            "msg_a1",
            Some(REAL_TURN_ID),
            1_791_832_995_792,
            &real_assistant_json(),
        );
        // ses_a 第二轮：用量 2 行、assistant 1 行 → 整组不配对
        insert_usage(
            &src,
            REAL_SESSION_ID,
            Some("turn_2"),
            None,
            REAL_TS + 60_000,
            (1000, 20, 10, 50, 5),
            Some(0.5),
        );
        insert_usage(
            &src,
            REAL_SESSION_ID,
            Some("turn_2"),
            None,
            REAL_TS + 61_000,
            (2000, 30, 0, 60, 0),
            None,
        );
        insert_assistant(
            &src,
            REAL_SESSION_ID,
            "msg_a2",
            Some("turn_2"),
            1_791_833_055_000,
            r#"{"usage":{"request_duration_ms":300},"context_usage_telemetry":{"model":"should-not-be-used"}}"#,
        );
        // ses_b：源列自带 provider/model
        insert_usage(
            &src,
            "mvs_real_project",
            Some("turn_b1"),
            Some("minimax/abab6.5s-chat"),
            1_791_833_100_000,
            (100, 10, 0, 0, 0),
            Some(0.25),
        );

        let mut derived = open_derived(&dir);
        let first = import_from_source(&src, &mut derived, &root, 1_000).expect("首次导入失败");
        assert_eq!(first.shape, "runtime-state");
        assert_eq!(first.source_rows, 4);
        assert_eq!(first.source_sessions, 2);
        assert!(!first.skipped);
        assert_eq!(first.records_written, 4);
        assert_eq!(first.ghosts_removed, 0);

        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 4);
        let row_a1 = rows
            .iter()
            .find(|r| r.dedupe_key == format!("{REAL_SESSION_ID}|{REAL_TURN_ID}|1"))
            .expect("第一轮用量行应存在");
        assert_eq!(row_a1.started_at, REAL_TS, "started_at = 源行 ts（毫秒）");
        // 归一：input 含 cache read、output 不加 reasoning
        assert_eq!(
            (
                row_a1.input,
                row_a1.output,
                row_a1.cache_read,
                row_a1.cache_write,
                row_a1.reasoning,
                row_a1.total
            ),
            (24964, 79, 189, 0, 0, 25043)
        );
        assert_eq!(row_a1.model_id, REAL_TELEMETRY_MODEL, "取配对的遥测模型");
        assert_eq!(
            row_a1.provider_id, "minimax",
            "遥测模型与会话模型段逐字相同 → 用会话 effectiveModel 补 provider"
        );
        assert_eq!(row_a1.duration_ms, Some(1952.0));
        assert_eq!(row_a1.reported_cost, Some(0.0));
        // 默认工作区 + 合成工作区 → 不算项目
        assert!(row_a1.cwd.is_none(), "合成工作区不该当项目目录");
        assert!(row_a1.project_key.is_none());

        let row_a2 = rows
            .iter()
            .find(|r| r.dedupe_key == format!("{REAL_SESSION_ID}|turn_2|2"))
            .expect("第二轮第一行应存在");
        assert_eq!(
            row_a2.model_id, "MiniMax-M3.1-Flash-Preview",
            "配对数量不等 → 退到会话 effectiveModel"
        );
        assert_eq!(row_a2.provider_id, "minimax");
        assert_eq!(row_a2.duration_ms, None, "不配对就没有耗时");
        assert_eq!(
            (row_a2.input, row_a2.output, row_a2.reasoning, row_a2.total),
            (1050, 20, 10, 1070)
        );
        let row_a3 = rows
            .iter()
            .find(|r| r.dedupe_key == format!("{REAL_SESSION_ID}|turn_2|3"))
            .expect("第二轮第二行应存在");
        assert_eq!(row_a3.duration_ms, None);
        assert_eq!(row_a3.reported_cost, None, "源库没给 cost 时留 NULL 而不是 0");

        let row_b1 = rows
            .iter()
            .find(|r| r.dedupe_key == "mvs_real_project|turn_b1|4")
            .expect("ses_b 用量行应存在");
        assert_eq!(row_b1.model_id, "abab6.5s-chat", "源列 model 优先级最高");
        assert_eq!(row_b1.provider_id, "minimax");
        assert_eq!(row_b1.cwd.as_deref(), Some("C:/work/demo"));
        assert_eq!(
            row_b1.project_key,
            crate::projects::normalize_cwd("C:/work/demo")
        );

        // 幂等：指纹未变 → 整轮跳过，内容与自增 id 全都不动
        let ids_before = dump_ids(&derived);
        let second = import_from_source(&src, &mut derived, &root, 2_000).expect("二次导入失败");
        assert!(second.skipped, "源指纹未变应整轮跳过");
        assert_eq!(second.records_written, 0);
        assert_eq!(dump_rows(&derived), rows);
        assert_eq!(dump_ids(&derived), ids_before);

        // 会话元信息：标题 / 工作区 / 项目键都落库（标题供展示与排障）
        let (title, directory, project_key, is_default): (
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
        ) = derived
            .query_row(
                "SELECT title, directory, project_key, is_default_workspace
                 FROM session_meta WHERE session_id = ?1",
                [REAL_SESSION_ID],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("读取会话元信息失败");
        assert_eq!(title.as_deref(), Some("并行会话"));
        assert_eq!(directory.as_deref(), Some(synthetic.as_str()));
        assert!(project_key.is_none(), "合成工作区不算项目");
        assert_eq!(is_default, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 幽灵行清理：源行被删（app 删会话/删行）→ 重扫时差集清掉派生行；
    /// 存活行保持原自增 id（同步上传不重复计数的前提）。
    #[test]
    fn reimport_removes_ghost_rows_and_keeps_rowids_stable() {
        let dir = temp_dir("ghost");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).expect("建临时根失败");
        let src = create_source(&root.join("runtime-state.sqlite"));
        insert_session(
            &src,
            "ses_a",
            "a",
            0,
            "C:/work/keep",
            None,
            r#"{"effectiveModel":"minimax/M3"}"#,
            100,
        );
        insert_session(
            &src,
            "ses_b",
            "b",
            0,
            "C:/work/drop",
            None,
            r#"{"effectiveModel":"minimax/M3"}"#,
            100,
        );
        insert_usage(&src, "ses_a", Some("t1"), None, 1_000, (10, 1, 0, 0, 0), Some(0.1));
        insert_usage(&src, "ses_b", Some("t1"), None, 2_000, (20, 2, 0, 0, 0), Some(0.2));
        let mut derived = open_derived(&dir);
        import_from_source(&src, &mut derived, &root, 100).expect("首次导入失败");
        let keep_key = "ses_a|t1|1";
        let keep_id: i64 = derived
            .query_row(
                "SELECT id FROM model_usage WHERE dedupe_key = ?1",
                [keep_key],
                |row| row.get(0),
            )
            .expect("读取自增 id 失败");
        assert_eq!(dump_rows(&derived).len(), 2);

        // app 删除会话 ses_b（连带删掉它的用量行）
        src.execute("DELETE FROM local_runtime_token_usage WHERE session_id = 'ses_b'", [])
            .expect("删除源用量行失败");
        src.execute("DELETE FROM local_runtime_sessions WHERE session_id = 'ses_b'", [])
            .expect("删除源会话行失败");
        let outcome = import_from_source(&src, &mut derived, &root, 200).expect("重扫失败");
        assert!(!outcome.skipped, "源指纹变化必须触发重扫");
        assert_eq!(outcome.ghosts_removed, 1, "被删会话的派生行应清掉");
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].dedupe_key, keep_key);
        let keep_id_after: i64 = derived
            .query_row(
                "SELECT id FROM model_usage WHERE dedupe_key = ?1",
                [keep_key],
                |row| row.get(0),
            )
            .expect("读取自增 id 失败");
        assert_eq!(keep_id_after, keep_id, "存活行的自增 id 不得漂移");

        // 会话元信息不主动清理（历史行保留，避免源库瞬时不可读时误删）
        let sessions: i64 = derived
            .query_row("SELECT COUNT(*) FROM session_meta", [], |row| row.get(0))
            .expect("统计会话元信息失败");
        assert_eq!(sessions, 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 只有会话维度变化（用量表指纹不变）也必须重扫：`effectiveModel` /
    /// 工作区改写会改模型归属与项目键，只用量表指纹会漏掉这类修订。
    #[test]
    fn session_only_change_triggers_rescan() {
        let dir = temp_dir("session-only");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).expect("建临时根失败");
        let src = create_source(&root.join("runtime-state.sqlite"));
        insert_session(
            &src,
            "ses_a",
            "旧标题",
            0,
            "C:/work/old",
            None,
            r#"{"effectiveModel":"minimax/old-model"}"#,
            100,
        );
        insert_usage(&src, "ses_a", Some("t1"), None, 1_000, (10, 1, 0, 0, 0), None);
        let mut derived = open_derived(&dir);
        import_from_source(&src, &mut derived, &root, 100).expect("首次导入失败");
        let row = &dump_rows(&derived)[0];
        assert_eq!(row.model_id, "old-model");
        assert_eq!(row.cwd.as_deref(), Some("C:/work/old"));

        // 只改会话（用量表一行未动、ts/id 都不变）
        src.execute(
            "UPDATE local_runtime_sessions
                SET extra_data_json = ?1, workspace_dir = ?2, project_workspace_dir = ?2,
                    title = '新标题', updated_at_ms = 200
              WHERE session_id = 'ses_a'",
            rusqlite::params![
                r#"{"effectiveModel":"minimax/new-model"}"#,
                "C:/work/new"
            ],
        )
        .expect("更新源会话失败");
        let outcome = import_from_source(&src, &mut derived, &root, 200).expect("重扫失败");
        assert!(!outcome.skipped, "会话指纹变化必须触发重扫");
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 1, "重扫不得产生重复行");
        assert_eq!(rows[0].model_id, "new-model", "模型归属应跟着会话更新");
        assert_eq!(rows[0].cwd.as_deref(), Some("C:/work/new"));
        let title: Option<String> = derived
            .query_row(
                "SELECT title FROM session_meta WHERE session_id = 'ses_a'",
                [],
                |row| row.get(0),
            )
            .expect("读取标题失败");
        assert_eq!(title.as_deref(), Some("新标题"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 修订通道：模型名从空被回填（总量同步变大）→ 行 id 不变、`updated_at`
    /// 推进，`id > 游标` 选不出它而 `updated_at > 游标` 能选出；未变行不得被打上
    /// 修订标记（否则每轮同步都要全量重传）。
    #[test]
    fn revision_marker_only_moves_for_server_accepted_changes() {
        let dir = temp_dir("revision");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).expect("建临时根失败");
        let src = create_source(&root.join("runtime-state.sqlite"));
        insert_session(&src, "ses_a", "a", 0, "C:/work/a", None, "{}", 100);
        insert_usage(&src, "ses_a", Some("t1"), None, 1_000, (10, 1, 0, 0, 0), None);
        insert_usage(&src, "ses_a", Some("t2"), None, 2_000, (20, 2, 0, 0, 0), None);
        let mut derived = open_derived(&dir);
        import_from_source(&src, &mut derived, &root, 100).expect("首次导入失败");
        let id_of = |conn: &Connection, key: &str| -> i64 {
            conn.query_row(
                "SELECT id FROM model_usage WHERE dedupe_key = ?1",
                [key],
                |row| row.get(0),
            )
            .expect("读取 id 失败")
        };
        let updated_of = |conn: &Connection, key: &str| -> i64 {
            conn.query_row(
                "SELECT updated_at FROM model_usage WHERE dedupe_key = ?1",
                [key],
                |row| row.get(0),
            )
            .expect("读取 updated_at 失败")
        };
        let id_t1 = id_of(&derived, "ses_a|t1|1");

        // 会话补上 effectiveModel（模型名从空被回填）+ 用量不变
        src.execute(
            "UPDATE local_runtime_sessions SET extra_data_json = ?1, updated_at_ms = 200
              WHERE session_id = 'ses_a'",
            [r#"{"effectiveModel":"minimax/M3"}"#],
        )
        .expect("更新源会话失败");
        import_from_source(&src, &mut derived, &root, 200).expect("重扫失败");
        assert_eq!(id_of(&derived, "ses_a|t1|1"), id_t1, "回填不得换 id");
        assert_eq!(updated_of(&derived, "ses_a|t1|1"), 200, "空模型名回填应打修订标记");
        assert_eq!(
            updated_of(&derived, "ses_a|t2|2"),
            200,
            "同一轮重扫里，另一个空模型名行同样被回填（服务端会采纳）"
        );

        // 改一条源行的 token（总量变大）并把它的 ts 推到新的最大值（指纹可见）
        // → 整表重扫，但只有总量真的变大的那一行推进修订标记。
        // 注意：源行内容被改写却既不动 ts 也不新增 id 时指纹不变、该次修订发现不了
        //（模块头「残留盲区」，app 正常写入都是追加新行，不会走这条路径）。
        src.execute(
            "UPDATE local_runtime_token_usage SET input_tokens = 99, ts = 3000 WHERE id = 1",
            [],
        )
        .expect("更新源用量行失败");
        import_from_source(&src, &mut derived, &root, 300).expect("重扫失败");
        assert_eq!(updated_of(&derived, "ses_a|t1|1"), 300, "总量变大应推进修订标记");
        assert_eq!(
            updated_of(&derived, "ses_a|t2|2"),
            200,
            "未变行不得被打上修订标记（否则每轮同步都要重传全表）"
        );

        // 修订通道按 updated_at 选出被改写的行；未被改写的行选不出
        let revised = query_revised_since_at(&derived, 250, 0, 10).expect("修订查询失败");
        assert_eq!(revised.len(), 1);
        assert_eq!(revised[0].computed_total_tokens, 100);
        // 明细通道：id 游标只选新行（这里没有新行）
        assert!(query_since_at(&derived, id_t1, 10)
            .expect("明细查询失败")
            .iter()
            .all(|r| r.local_rowid != id_t1));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 查询聚合：overall 与 by_model 自洽、趋势求和等于总量、明细/修订/会话窗口/
    /// 项目维度各通道都能用、自算成本按范围累计。
    #[test]
    fn queries_aggregate_imported_rows() {
        let dir = temp_dir("queries");
        let root = dir.join("root");
        std::fs::create_dir_all(root.join("sessions")).expect("建临时根失败");
        let src = create_source(&root.join("runtime-state.sqlite"));
        let synthetic = root.join("sessions").join("ses_a").join("workspace");
        insert_session(
            &src,
            "ses_a",
            "会话 A",
            1,
            &synthetic.to_string_lossy(),
            None,
            r#"{"effectiveModel":"minimax/MiniMax-M3.1-Flash-Preview"}"#,
            100,
        );
        insert_session(&src, "ses_b", "会话 B", 0, "C:/work/other", None, "{}", 100);
        insert_usage(
            &src,
            "ses_a",
            Some("t1"),
            None,
            1_000,
            (24775, 79, 0, 189, 0),
            Some(0.0),
        );
        insert_assistant(
            &src,
            "ses_a",
            "m1",
            Some("t1"),
            1_100,
            &real_assistant_json(),
        );
        insert_usage(&src, "ses_b", Some("t1"), None, 2_000, (1_000, 100, 20, 50, 10), Some(0.5));
        let mut derived = open_derived(&dir);
        import_from_source(&src, &mut derived, &root, 100).expect("导入失败");

        let stats = query_stats_at(&derived, 0, i64::MAX).expect("统计失败");
        assert_eq!(stats.overall.requests, 2);
        let model_total: i64 = stats.by_model.iter().map(|m| m.total_tokens).sum();
        assert_eq!(stats.overall.total_tokens, model_total);
        assert_eq!(
            stats.overall.total_tokens,
            25043 + 1_150,
            "ses_b 行：input 1000 + 缓存读 50、output 100"
        );
        assert!(stats.overall.input_tokens >= stats.overall.cache_read_tokens);
        assert_eq!(stats.overall.reasoning_tokens, 20, "reasoning 原样落库");
        // 速度：只有请求总耗时 → 请求平均口径（前端带 ≈）
        assert_eq!(
            stats.overall.speed.speed_quality,
            Some(crate::token_speed::SpeedQuality::RequestAverage)
        );
        assert_eq!(stats.overall.speed.avg_ttft_ms, None, "无 TTFT 来源不给首字延迟");
        assert_eq!(
            stats.current_model.as_ref().map(|m| m.model_id.as_str()),
            Some("MiniMax-M3.1-Flash-Preview")
        );

        // 趋势：真实范围求和 == 总量
        let from = stats.earliest_ms.expect("应有最早时刻");
        let to = stats.latest_ms.expect("应有最晚时刻") + 1;
        let trend = query_trend_at(&derived, from, to, "day").expect("趋势失败");
        assert_eq!(
            trend.iter().map(|b| b.total_tokens).sum::<i64>(),
            stats.overall.total_tokens
        );
        assert!(query_trend_at(&derived, 0, i64::MAX, "day")
            .expect("趋势失败")
            .len()
            <= MAX_TREND_BUCKETS);

        // 模型清单（配价页数据源）
        let models = list_models_at(&derived).expect("模型列表失败");
        assert!(models
            .iter()
            .any(|m| m.model_id == "MiniMax-M3.1-Flash-Preview"));

        // 周期聚合 / 明细 / 修订通道
        let periods = query_period_buckets_at(&derived, &[(0, i64::MAX), (0, 1_500)])
            .expect("周期聚合失败");
        assert_eq!(periods[0].total_tokens, stats.overall.total_tokens);
        assert_eq!(periods[0].requests, 2);
        assert!(periods[1].total_tokens < periods[0].total_tokens);
        let rows = query_since_at(&derived, 0, 100).expect("明细失败");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.source == "minimaxcode"));
        assert!(rows.iter().all(|r| r.session_id.is_some()));
        assert_eq!(
            query_revised_since_at(&derived, 0, 0, 100)
                .expect("修订失败")
                .len(),
            2
        );
        assert_eq!(max_rowid_at(&derived).expect("max_rowid 失败"), 2);

        // 会话窗口（HUD）：两个会话、按最后活动倒序、无生成中/无 TTFT
        let window = query_window_sessions_at(&derived, 0, 10).expect("会话窗口失败");
        assert_eq!(window.total_active, 2);
        assert_eq!(window.sessions.len(), 2);
        assert!(window
            .sessions
            .iter()
            .all(|s| !s.generating
                && s.ttft_ms.is_none()
                && s.gen_out_tokens == 0
                && s.gen_ms == 0.0));
        assert!(window
            .sessions
            .iter()
            .all(|s| s.in_tokens > 0 && s.out_tokens > 0));

        // 项目维度：ses_a 是合成工作区（无项目），ses_b 是真实目录 → unknown + 1 个键
        let projects = query_project_model_rows_at(&derived, 0, i64::MAX).expect("项目聚合失败");
        let keys: Vec<&str> = projects.iter().map(|p| p.project_key.as_str()).collect();
        assert!(keys.contains(&crate::projects::UNKNOWN_PROJECT));
        let other = crate::projects::normalize_cwd("C:/work/other").unwrap();
        assert!(keys.contains(&other.as_str()));
        let counts =
            query_project_session_counts_at(&derived, 0, i64::MAX).expect("项目会话数失败");
        assert_eq!(counts.iter().map(|(_, n)| *n).sum::<i64>(), 2);
        let display = query_project_display_paths_at(&derived).expect("项目路径失败");
        assert_eq!(display.len(), 1);
        let (total, session_rows) =
            query_project_sessions_at(&derived, &other, 0, i64::MAX, 0, 10)
                .expect("项目会话明细失败");
        assert_eq!((total, session_rows.len()), (1, 1));

        // 自算成本：只统计有 cost 的行（两行都有）
        let cost = query_reported_cost_at(&derived, 0, i64::MAX).expect("成本查询失败");
        assert_eq!((cost.total_rows, cost.priced_rows), (2, 2));
        assert!((cost.total_usd - 0.5).abs() < 1e-12);
        let (records, latest, total_cost, cost_rows) = derived_debug(&derived);
        assert_eq!((records, cost_rows), (2, 2));
        assert_eq!(latest, Some(2_000));
        assert!((total_cost - 0.5).abs() < 1e-12);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 空数据/缺表都不 panic：无用量的空库、缺会话表/消息表/项目表的库、
    /// 空派生库上的全套查询都给零值。
    #[test]
    fn empty_and_degraded_sources_do_not_panic() {
        let dir = temp_dir("empty");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).expect("建临时根失败");
        // (1) 只有用量表（会话/消息/项目表都缺）的空库
        let src_path = root.join("runtime-state.sqlite");
        let src = Connection::open(&src_path).expect("建源库失败");
        src.execute_batch(
            "CREATE TABLE local_runtime_token_usage (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL,
                 agent_name TEXT NOT NULL,
                 framework_type TEXT NOT NULL,
                 turn_id TEXT,
                 model TEXT,
                 ts INTEGER NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 reasoning_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 cache_write_tokens INTEGER NOT NULL,
                 cost_usd REAL,
                 raw TEXT
             );",
        )
        .expect("建用量表失败");
        assert!(has_local_data_at(&src_path));
        let tables = detect_tables(&src);
        assert_eq!(
            tables,
            SourceTables {
                sessions: false,
                message_rows: false,
                projects: false
            }
        );
        assert_eq!(source_debug(&src), ("runtime-state".to_string(), 0, 0));
        let mut derived = open_derived(&dir);
        let outcome = import_from_source(&src, &mut derived, &root, 100).expect("空库导入失败");
        assert_eq!(outcome.shape, "runtime-state");
        assert_eq!(outcome.source_rows, 0);
        assert_eq!(outcome.records_written, 0);

        // 缺会话表时仍能导入（只丢项目维度）
        src.execute(
            "INSERT INTO local_runtime_token_usage
                (session_id, agent_name, framework_type, turn_id, model, ts,
                 input_tokens, output_tokens, reasoning_tokens, cache_read_tokens,
                 cache_write_tokens, cost_usd)
             VALUES ('ses_x', 'a', 'local', 't1', 'minimax/M3', 1000, 5, 1, 0, 0, 0, NULL)",
            [],
        )
        .expect("插入用量行失败");
        let outcome = import_from_source(&src, &mut derived, &root, 200).expect("缺表导入失败");
        assert_eq!(outcome.records_written, 1);
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model_id, "M3");
        assert!(rows[0].cwd.is_none());

        // (2) 没有用量表的库（不是错误，也不是 presence）
        let other_path = root.join("other.sqlite");
        let other = Connection::open(&other_path).expect("建库失败");
        other
            .execute_batch("CREATE TABLE account (id TEXT PRIMARY KEY);")
            .expect("建无关表失败");
        assert!(!has_local_data_at(&other_path));
        assert!(!has_local_data_at(&root.join("missing.sqlite")));
        let outcome =
            import_from_source(&other, &mut derived, &root, 300).expect("无关库导入不该报错");
        assert_eq!(
            outcome,
            ImportOutcome {
                shape: "none".to_string(),
                ..Default::default()
            }
        );
        assert_eq!(source_debug(&other), ("none".to_string(), 0, 0));

        // (3) 空派生库上全套查询：零值、不 panic
        std::fs::create_dir_all(dir.join("sub")).expect("建空派生库目录失败");
        let empty = open_derived(&dir.join("sub"));
        let stats = query_stats_at(&empty, 0, i64::MAX).expect("空库统计失败");
        assert_eq!(stats.overall.requests, 0);
        assert_eq!(stats.overall.total_tokens, 0);
        assert!(stats.earliest_ms.is_none());
        assert!(stats.by_model.is_empty());
        assert!(stats.current_model.is_none());
        assert!(list_models_at(&empty).expect("空库模型列表失败").is_empty());
        assert_eq!(max_rowid_at(&empty).expect("空库 max_rowid 失败"), 0);
        assert!(query_since_at(&empty, 0, 10).expect("空库明细失败").is_empty());
        assert!(query_revised_since_at(&empty, 0, 0, 10)
            .expect("空库修订失败")
            .is_empty());
        let trend = query_trend_at(&empty, 0, 7_200_000, "hour").expect("空库趋势失败");
        assert_eq!(trend.len(), 2);
        assert!(trend.iter().all(|b| b.total_tokens == 0 && b.requests == 0));
        let periods =
            query_period_buckets_at(&empty, &[(0, i64::MAX)]).expect("空库周期聚合失败");
        assert_eq!((periods[0].total_tokens, periods[0].requests), (0, 0));
        let window = query_window_sessions_at(&empty, 0, 10).expect("空库会话窗口失败");
        assert_eq!(window.total_active, 0);
        assert!(window.sessions.is_empty());
        assert!(query_project_model_rows_at(&empty, 0, i64::MAX)
            .expect("空库项目聚合失败")
            .is_empty());
        assert!(query_project_session_counts_at(&empty, 0, i64::MAX)
            .expect("空库项目会话统计失败")
            .is_empty());
        assert!(query_project_display_paths_at(&empty)
            .expect("空库项目路径失败")
            .is_empty());
        let (total, sessions) = query_project_sessions_at(
            &empty,
            crate::projects::UNKNOWN_PROJECT,
            0,
            i64::MAX,
            0,
            10,
        )
        .expect("空库项目会话失败");
        assert_eq!((total, sessions.len()), (0, 0));
        assert_eq!(
            query_reported_cost_at(&empty, 0, i64::MAX).expect("空库成本失败"),
            ReportedCostSummary {
                total_usd: 0.0,
                priced_rows: 0,
                total_rows: 0
            }
        );
        assert_eq!(derived_debug(&empty), (0, None, 0.0, 0));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 坏行只跳过自己：坏 JSON 的 assistant 行（`json_valid` 守卫）与脏耗时
    /// 不影响同会话其他行；坏 JSON 的会话 `extra_data_json` 也不影响导入。
    #[test]
    fn bad_rows_are_skipped_without_blocking_the_import() {
        let dir = temp_dir("badrows");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).expect("建临时根失败");
        let src = create_source(&root.join("runtime-state.sqlite"));
        insert_session(
            &src,
            "ses_a",
            "a",
            0,
            "C:/work/a",
            None,
            "not json",
            100,
        );
        insert_usage(&src, "ses_a", Some("t1"), None, 1_000, (10, 1, 0, 0, 0), None);
        // 坏 JSON 的 assistant 行：json_extract 会报错，必须被 json_valid 挡掉
        insert_assistant(&src, "ses_a", "bad", Some("t1"), 1_000, "not json");
        // 脏耗时（非数值字符串）：CAST 后为 0 → 不采纳（数值字符串会被 CAST 成数，
        // 属于容错而非脏值，见 fetch_assistant_rows 注释）
        insert_assistant(
            &src,
            "ses_a",
            "dirty",
            Some("t2"),
            2_000,
            r#"{"usage":{"request_duration_ms":"oops"},"context_usage_telemetry":{"model":"m2"}}"#,
        );
        insert_usage(&src, "ses_a", Some("t2"), None, 2_000, (20, 2, 0, 0, 0), None);

        let mut derived = open_derived(&dir);
        let outcome = import_from_source(&src, &mut derived, &root, 100).expect("导入失败");
        assert_eq!(outcome.records_written, 2, "坏 assistant 行不该让整轮导入失败");
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 2);
        let t1 = rows.iter().find(|r| r.dedupe_key == "ses_a|t1|1").unwrap();
        // t1 分组：用量 1 行 vs 有效 assistant 0 行（坏行被挡）→ 不配对
        assert_eq!(t1.duration_ms, None);
        assert_eq!(t1.model_id, "", "会话 effectiveModel 是坏 JSON → 整体退到空");
        let t2 = rows.iter().find(|r| r.dedupe_key == "ses_a|t2|2").unwrap();
        assert_eq!(t2.model_id, "m2", "脏耗时不影响模型配对");
        assert_eq!(t2.duration_ms, None, "非数值耗时 CAST 成 0 → 不采纳");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 源库路径解析：v2 布局优先，缺失时回退 pre-v2 布局。
    #[test]
    fn source_db_path_prefers_v2_then_falls_back() {
        let dir = temp_dir("paths");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).expect("建临时根失败");
        // 两个都没有 → 给 v2 路径（诊断里展示「本该在哪」）
        assert_eq!(
            source_db_path_at(&root),
            root.join("v2").join("sqlite").join("runtime-state.sqlite")
        );
        // 只有旧布局 → 回退
        std::fs::create_dir_all(root.join("sqlite")).expect("建旧布局目录失败");
        let legacy = root.join("sqlite").join("runtime-state.sqlite");
        std::fs::write(&legacy, b"").expect("建旧布局库失败");
        assert_eq!(source_db_path_at(&root), legacy);
        // 两个都在 → v2 优先
        std::fs::create_dir_all(root.join("v2").join("sqlite")).expect("建 v2 目录失败");
        let v2 = root.join("v2").join("sqlite").join("runtime-state.sqlite");
        std::fs::write(&v2, b"").expect("建 v2 布局库失败");
        assert_eq!(source_db_path_at(&root), v2);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 真机冒烟（对齐 opencode.rs / dsh.rs 的同名用例）：本机装了 MiniMax Code
    /// 且跑过对话时，对**真实源库**整链路跑一遍——只读源库 + 临时派生库，
    /// **不碰** `~/.zbar/minimaxcode.sqlite`；没装或没数据的机器直接跳过，
    /// 保证 CI 与他人机器必过。
    ///
    /// 断言的都不是「本机恰好有多少行」，而是口径不变量：指纹未变要跳过整轮、
    /// 清指纹强制重扫后行数与自增 id 必须逐行不变（同步游标不许回退）、
    /// 汇总自洽（requests = 行数、总量 = 各模型之和、input 含 cache read、
    /// 计算总量 = input + output）、以及真实样本行（若仍在库中）逐字段钉住
    /// 归一化结果与请求耗时。
    #[test]
    fn real_home_import_and_query_smoke() {
        let root = data_root();
        let src_path = source_db_path_at(&root);
        if !src_path.exists() {
            eprintln!("跳过：本机没有 MiniMax Code 源库 {}", src_path.display());
            return;
        }
        let src = open_source_readonly(&src_path).expect("只读打开真实源库失败");
        if !table_exists(&src, USAGE_TABLE) {
            eprintln!("跳过：真实源库里没有 {USAGE_TABLE} 表");
            return;
        }

        let dir = temp_dir("real-home");
        let mut conn = open_minimaxcode_db_at(&dir.join("derived.sqlite"))
            .expect("打开临时派生库失败");
        let now = 1_791_840_000_000i64;

        let first = import_from_source(&src, &mut conn, &root, now).expect("首次导入失败");
        assert_eq!(first.shape, "runtime-state");
        assert!(!first.skipped, "首次导入不该被判为跳过（派生库是空的）");
        if first.source_rows == 0 {
            eprintln!("跳过：真实源库用量表还是空的（尚未产生用量）");
            return;
        }
        assert!(
            first.records_written > 0,
            "源库有用量行却一行都没落库（{} 行 / 跳过={}）",
            first.source_rows,
            first.skipped
        );

        // 1) 指纹未变 → 整轮跳过（不重扫、不重写）
        let second = import_from_source(&src, &mut conn, &root, now).expect("二次导入失败");
        assert!(second.skipped, "指纹未变时应当跳过整轮导入");

        // 2) 清掉指纹强制重扫 → 行数与最大自增 id 必须不变（重导不许换 id）
        let before = query_rows_at(&conn, "ORDER BY id", []).expect("明细查询失败");
        let max_before = max_rowid_at(&conn).expect("max_rowid 失败");
        conn.execute("DELETE FROM source_progress", [])
            .expect("清指纹失败");
        let third = import_from_source(&src, &mut conn, &root, now).expect("重扫导入失败");
        assert!(!third.skipped, "清掉指纹后应当真的重扫");
        let after = query_rows_at(&conn, "ORDER BY id", []).expect("明细查询失败");
        assert_eq!(before.len(), after.len(), "重扫后行数发生变化");
        assert_eq!(
            max_rowid_at(&conn).expect("max_rowid 失败"),
            max_before,
            "重扫后最大自增 id 变化（会让同步游标重复上传）"
        );
        assert_eq!(
            after.len() as i64,
            max_before,
            "max_rowid 应当等于行数（id 从 1 连续自增）"
        );
        assert_eq!(
            before.iter().map(|r| r.local_rowid).collect::<Vec<_>>(),
            after.iter().map(|r| r.local_rowid).collect::<Vec<_>>(),
            "重扫后逐行 id 序列变化"
        );

        // 3) 汇总自洽 + 逐行口径
        let stats = query_stats_at(&conn, 0, i64::MAX).expect("stats 查询失败");
        assert_eq!(stats.overall.requests, after.len() as i64, "requests ≠ 行数");
        assert_eq!(
            stats.overall.total_tokens,
            stats.by_model.iter().map(|m| m.total_tokens).sum::<i64>(),
            "总量与按模型之和不一致"
        );
        assert!(
            stats.overall.input_tokens >= stats.overall.cache_read_tokens,
            "input 应当已含 cache read"
        );
        for row in &after {
            assert_eq!(
                row.computed_total_tokens,
                row.input_tokens + row.output_tokens,
                "计算总量口径被破坏（reasoning 不该再相加）: {:?}",
                row.session_id
            );
            assert!(
                row.input_tokens >= row.cache_read_input_tokens,
                "行内 input 未含 cache read: {:?}",
                row.session_id
            );
            assert_eq!(row.source, "minimaxcode");
        }

        // 4) 真实样本行（本机第一次调用那条）若仍在库中，逐字段钉死归一化结果
        if let Some(sample) = after
            .iter()
            .find(|r| r.started_at == REAL_TS && r.session_id.as_deref() == Some(REAL_SESSION_ID))
        {
            assert_eq!(sample.input_tokens, 24_964, "input 应含 cache read（24775+189）");
            assert_eq!(sample.output_tokens, REAL_USAGE.1);
            assert_eq!(sample.reasoning_tokens, REAL_USAGE.2);
            assert_eq!(sample.cache_read_input_tokens, REAL_USAGE.3);
            assert_eq!(sample.cache_creation_input_tokens, REAL_USAGE.4);
            assert_eq!(sample.computed_total_tokens, 25_043);
            assert_eq!(sample.model_id, REAL_TELEMETRY_MODEL);
            assert_eq!(sample.provider_id, "minimax");
            let duration: Option<f64> = conn
                .query_row(
                    "SELECT duration_ms FROM model_usage WHERE id = ?1",
                    [sample.local_rowid],
                    |r| r.get(0),
                )
                .expect("读耗时失败");
            assert_eq!(
                duration,
                Some(REAL_REQUEST_DURATION_MS),
                "耗时应取同轮 assistant 行的 request_duration_ms"
            );
        }

        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }
}
