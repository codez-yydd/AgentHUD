//! OpenCode（开源 agent，v2 桌面版 / CLI）本地用量导入模块。
//!
//! 数据来源：`~/.local/share/opencode/opencode.db`（sqlite，WAL 模式）。根目录与
//! 库路径由 `opencodego::opencode_root()` / `opencodego::db_path()` 提供
//! （`ZBAR_OPENCODE_HOME` 可覆盖，对齐 codex/claude/dsh 的 ZBAR_*_HOME 先例）。
//!
//! **为什么读库而不是文件**：OpenCode 不落 jsonl 会话日志，用量只在 sqlite 里。
//! 源库由 OpenCode 自己写入，本模块**只读**打开（绝不写用户库）；派生库落在
//! `~/.agenthud/opencode.sqlite`（自有库，读写均可），与 dsh.sqlite / kimi.sqlite 同构。
//!
//! **v1 / v2 双形态自适应**：v2 是把 v1 改名迁移来的（其迁移 SQL：
//! `ALTER TABLE session RENAME TO session_v2`），用户可能装任一版本，故两套都认：
//! - 消息表：v2 `session_message`（角色在**列 `type`**，实测取值
//!   `user`/`assistant`/`system`/`idle`/`model-switched`；`data` JSON 里**没有**
//!   role 字段）/ v1 `message`（无 `type` 列，角色在 `data` 的 `$.role`）；
//! - 会话表：v2 `session_v2` + `project`（取目录与项目映射）/ v1 `session`
//!   （列名各版本略有差异，按列存在性探测，缺列只影响项目维度，不影响用量导入）；
//! - 模型/供应商：v2 嵌套 `$.model.id` + `$.model.providerID`（实测形态），
//!   v1 平铺 `$.modelID` + `$.providerID`。**平铺优先，取不到再取嵌套**。
//!
//! **计费口径（已用真实付费数据端到端验证）**：OpenCode 二进制内的计费公式为
//! `(tokens.input * price.input + (tokens.output + tokens.reasoning) * price.output
//!   + tokens.cache.read * price.cache.read + tokens.cache.write * price.cache.write) / 1e6`，
//! 用本机付费样本反算，与它自己写入 `$.cost` 的数值**精确命中**：
//! - 样本 A（opencode-go / deepseek-v4.1-flash，价 0.15 / 0.6 / 0.003）：
//!   `(3501*0.15 + 63*0.6 + 2816*0.003)/1e6 = 0.000571398` = 其 `$.cost`；
//!   若把 cache.read 并进 input 计价则得 0.0009938（✗），故 **`tokens.input` 不含
//!   cache read**；
//! - 样本 B：`(6179*0.15 + (91+12)*0.6 + 1536*0.003)/1e6 = 0.000993258` = 其 `$.cost`；
//!   不含 reasoning 则得 0.00098606（✗），故 **reasoning 按 output 价计费**。
//!
//! 而项目 `cost_for`（lib.rs）假定 `input_tokens` **已包含** cache_read
//! （`non_cache_input = input - cache_read`），只按 `output_tokens` 计价且不看
//! `reasoning_tokens`。故落库归一为：
//! - `input_tokens   = tokens.input + tokens.cache.read`（否则缓存读被双重扣减）
//! - `output_tokens  = tokens.output + tokens.reasoning`（否则 reasoning 漏计费）
//! - `cache_read_input_tokens = tokens.cache.read`、`cache_creation_input_tokens
//!   = tokens.cache.write`（原样，各按自己的价计费）
//! - `reasoning_tokens = tokens.reasoning`（**对本来源它是 output_tokens 的子集**，
//!   不是并列项；后来者勿把两者相加当总量）
//! - `computed_total_tokens = input_tokens + output_tokens`（reasoning 只计一次）
//!
//! 归一集中在一处：`normalize_tokens`（全模块唯一的计费口径校准点）。
//!
//! **价格分层提醒（不实现，仅记录）**：OpenCode 的模型价有 >200k 上下文分层
//! （`context_over_200k`），命中该档按另一套价计；本项目价格表没有分层，
//! 超大上下文请求的「按价重算」会与其自算成本有偏差。
//!
//! **`variant` 不落库**：实测取值 `default` / `high`，是思考档位而非模型身份
//! （同档价格相同），并进 `model_id` 会破坏按价查找，故只保留 `$.model.id`。
//!
//! **provider 原样存**：实测同一份数据里出现过 `opencode`（Zen，免费模型）与
//! `opencode-go`（订阅套餐）两个 providerID，按原样写入 `provider_id`，不归一。
//!
//! **OpenCode 自算成本另存一列**：`reported_cost_usd` 原样保存 `$.cost`
//! （USD 数值，OpenCode 自己按 models.dev 价格算好），供上层决定用官方口径还是
//! 按本项目价格表重算（本模块自身的 query_* 仍与 dsh 一致走 `cost_for`）。
//!
//! **行粒度：逐条 assistant 消息（不是会话聚合）**。实测 `session_v2` 的聚合值
//! **大于**其 assistant 行之和（差值如 in=+34/out=+4/cacheR=+512、in=+161/out=+3/
//! cacheR=+384），原因应是**会话标题生成等不属于对话消息的 LLM 调用**
//! （`session_message` 里没有对应行）。仍然选逐条口径，因为只有它能把用量正确
//! 归属到模型：实测同一会话里用户切换过模型（先 `opencode/step-5-preview-free`
//! 免费、后 `opencode-go/deepseek-v4.1-flash`），而 `session_v2.model` 只保留最后
//! 一个模型——用会话聚合会把免费模型的 input 记到 deepseek 头上。代价是**逐条
//! 口径会比 OpenCode 会话聚合少掉非对话调用（标题生成等）的那一小部分**，
//! 需要总量对齐时以上差异属已知偏差（详见 `reported_cost_usd` 列）。
//!
//! **增量口径（与 dsh 的字节偏移不同）**：数据源是数据库，没有「文件偏移」可言，
//! 故用**会话指纹**：`session_progress` 记录每个 session 的
//! `(MAX(time_updated), Σ time_updated, assistant 行数)`。每轮只处理「指纹变化 /
//! 未见过的」会话，处理时**按 dedupe_key 原地 upsert 该会话的行、并删掉源库里
//! 已不存在的键**——会话新增、追加消息、消息被编辑、甚至单条消息被删除（行数变化）
//! 都能被发现，且重复导入结果完全一致（幂等）。刻意不做「整会话删旧行重插」：
//! 那会让每行的自增 id 漂移，而同步上传的服务端按 `(device_id, source,
//! local_rowid)` 去重，重导一次就在服务端多出一份重复用量（详见 `write_session`）。
//! 三个分量缺一不可：只看 MAX 会漏掉「老消息被编辑」（MAX 可能不变）、
//! 也漏掉「删掉最后一条」（水位反而变小），故补 Σ 与行数。残留盲区：源库若把某行
//! 内容改写却不动任何时间戳（OpenCode 正常写入都会推进 `time_updated`），
//! 指纹不变、该次修订不会被发现。
//! 会话本身被删除时源库不再返回该 session，派生库保留其历史行（不主动清理，
//! 避免源库瞬时不可读时误删）。
//!
//! **耗时口径**：`duration_ms = $.time.completed - $.time.created`（请求总耗时，
//! 缺任一或倒挂则 NULL）。**TTFT 列存在但恒 NULL**：`$.time.streamed` 实测与
//! `completed` 仅差 2ms、与 `created` 差 3.35s，无法确认它是「首字到达」还是
//! 「流结束」，故不冒充 TTFT；因此速度质量标 `RequestAverage`（请求平均），
//! 不是 Generation。`opencodego::has_local_data` 的可见性未变（Go 套餐 presence
//! 仍只看库文件），本模块的 presence 另加「含 v1/v2 消息表」条件。

#![allow(dead_code)]
// 说明：本模块的查询函数刻意与 dsh.rs 同名同构（主线接线时逐点替换即可），
// 但当前只有 query_stats / query_trend / debug_info 被 lib.rs 的
// get_opencode_usage / get_opencode_debug 调用；sync 明细、projects 项目维度、
// session_hud 会话窗口、list_models 配价等入口待主线接线，故整模块允许
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

// ============================================================
// 路径与 presence
// ============================================================

/// 自有导入库路径：~/.agenthud/opencode.sqlite（与 dsh.sqlite / kimi.sqlite 同级）。
pub(crate) fn opencode_db_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("opencode.sqlite"))
}

/// 本机是否装过 OpenCode 并产生过用量：库存在**且**含 v2/v1 消息表。
///
/// 比 `opencodego::has_local_data`（库文件存在即可）更严：那个是 Go 套餐面板的
/// presence（只要装了 OpenCode 就该出现 tab），这里是「能不能导入用量」的判断
/// ——空库/未来新版改名后没有消息表时不该假装有数据。
pub fn has_local_data() -> bool {
    has_local_data_at(&crate::opencodego::db_path())
}

/// `has_local_data` 的路径注入版（单测用临时库，不碰真实用户目录）。
pub(crate) fn has_local_data_at(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    match open_source_readonly(path) {
        Ok(conn) => detect_shape(&conn).is_some(),
        Err(_) => false,
    }
}

// ============================================================
// 派生库（~/.agenthud/opencode.sqlite）
// ============================================================

/// 进程内串行化建表（claude/kimi/dsh 同款：多个查询命令并发 open 时不交错建表）。
static SCHEMA_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn schema_lock() -> &'static Mutex<()> {
    SCHEMA_LOCK.get_or_init(|| Mutex::new(()))
}

/// 派生库 schema。`model_usage` 逐列与 dsh.rs 一致（含 dedupe_key /
/// computed_total_tokens / updated_at / duration_ms 等），仅两处差异：
/// - 本库特有的 `reported_cost_usd`（OpenCode 自算成本，见模块头）；
/// - 没有 dsh 的 `file_progress`（数据源是数据库，改用 `session_progress` 水位）。
///
/// `session_meta` 是会话 → 项目/目录映射（项目维度用；dsh 把 cwd/project_key 直接
/// 冗余在 model_usage 行上，这里两者都做：行上冗余便于聚合，元表留全量便于排障）。
///
/// `time_to_first_token_ms` 与 dsh 同列但**恒 NULL**（见模块头 TTFT 说明）；
/// `delegation_depth` 同样恒 0（v2 有 `parent_id`，但「深度」语义未确认，
/// 不自造层级）。
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
     CREATE INDEX IF NOT EXISTS idx_opencode_mu_started ON model_usage(started_at);
     CREATE INDEX IF NOT EXISTS idx_opencode_mu_updated ON model_usage(updated_at);
     CREATE INDEX IF NOT EXISTS idx_opencode_mu_session ON model_usage(session_id);
     CREATE TABLE IF NOT EXISTS session_meta (
        session_id   TEXT PRIMARY KEY,
        project_id   TEXT NOT NULL DEFAULT '',
        directory    TEXT,
        worktree     TEXT,
        parent_id    TEXT NOT NULL DEFAULT '',
        time_created INTEGER NOT NULL DEFAULT 0,
        source_table TEXT NOT NULL DEFAULT '',
        captured_at  INTEGER NOT NULL DEFAULT 0
     );
     CREATE TABLE IF NOT EXISTS session_progress (
        session_id    TEXT PRIMARY KEY,
        watermark     INTEGER NOT NULL,
        revision_sum  INTEGER NOT NULL DEFAULT 0,
        message_count INTEGER NOT NULL,
        imported_at   INTEGER NOT NULL DEFAULT 0
     );";

/// 打开（必要时创建）派生库并确保表结构就绪。
fn open_opencode_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    open_opencode_db_at(&opencode_db_path()?)
}

/// 在指定路径打开派生库（表结构与生产库完全一致，供单测注入临时库）。
fn open_opencode_db_at(path: &Path) -> Result<Connection, String> {
    let conn =
        Connection::open(path).map_err(|e| format!("打开 OpenCode 导入库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    {
        // 建表串行化：并发 open 时第二个 CREATE 不会与第一个交错
        let _guard = schema_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute_batch(DERIVED_SCHEMA)
            .map_err(|e| format!("初始化 OpenCode 导入库失败: {e}"))?;
    }
    Ok(conn)
}

// ============================================================
// 源库形态探测（v1 / v2）
// ============================================================

/// 源库表形态：v2（`session_message` + `session_v2`）/ v1（`message` + `session`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceShape {
    V2,
    V1,
}

impl SourceShape {
    /// 消息表名。
    fn message_table(self) -> &'static str {
        match self {
            SourceShape::V2 => "session_message",
            SourceShape::V1 => "message",
        }
    }

    /// 会话表名（仅用于取目录/项目映射，缺表不影响用量导入）。
    fn session_table(self) -> &'static str {
        match self {
            SourceShape::V2 => "session_v2",
            SourceShape::V1 => "session",
        }
    }

    /// 只认 assistant 行的 WHERE 片段：v2 用 `type` 列（实测 data 里没有 role
    /// 字段），v1 用 data JSON 的 `$.role`。user 行只有「选了哪个模型」没有
    /// tokens/cost，system/idle/model-switched 同理，都不参与导入与水位统计。
    fn assistant_filter(self) -> &'static str {
        match self {
            SourceShape::V2 => "type = 'assistant'",
            SourceShape::V1 => "json_extract(data,'$.role') = 'assistant'",
        }
    }

    /// 诊断展示用的版本标记。
    fn label(self) -> &'static str {
        match self {
            SourceShape::V2 => "v2",
            SourceShape::V1 => "v1",
        }
    }
}

/// 表是否存在（table 名是代码内常量，无注入风险）。
fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// 探测源库形态：v2 优先（`session_message`），否则回退 v1（`message`）；
/// 两者都没有 → None（未装 OpenCode、空库或未来新版改名）。
pub(crate) fn detect_shape(conn: &Connection) -> Option<SourceShape> {
    if table_exists(conn, "session_message") {
        Some(SourceShape::V2)
    } else if table_exists(conn, "message") {
        Some(SourceShape::V1)
    } else {
        None
    }
}

/// 以只读方式打开**用户库**（绝不写 OpenCode 的数据）。
///
/// 与 `opencodego::open_readonly` 的差异：那里是额度估算（失败可容忍，250ms
/// 就放弃），这里是导入（宁可等 OpenCode 写完），故 busy_timeout 取 3s，
/// 与 claude/kimi/dsh 导入库同量级。
fn open_source_readonly(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("打开 OpenCode 本地库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    Ok(conn)
}

/// v1 的会话 id 表达式：优先列 `sessionID`（早期 v1 列名），缺列时取 data JSON
/// 的 `$.sessionID`（v1 数据里恒有；v2 已把它提升为 `session_id` 列）。
fn v1_session_id_expr(conn: &Connection) -> String {
    if db::has_column(conn, "message", "sessionID") {
        "COALESCE(NULLIF(sessionID, ''), json_extract(data,'$.sessionID'), '')".to_string()
    } else {
        "COALESCE(json_extract(data,'$.sessionID'), '')".to_string()
    }
}

// ============================================================
// 字段提取与归一（纯函数，单测直接喂真实 JSON）
// ============================================================

/// 宽松取整数：兼容 JSON 整数与浮点（`as_i64` 对 `7476.0` 返回 None），
/// 其余类型（字符串/布尔/缺失）一律按 0——token 是脏值时宁可当 0，也不报错。
fn as_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        _ => 0,
    }
}

/// 取非空字符串（trim 后为空视为缺失）。
fn as_non_empty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 从 assistant 行的 `data` JSON 提取原始 token 五元组：
/// `(input, output, cache_read, cache_write, reasoning)`，全部为 OpenCode 原值
/// （未归一，路径见模块头；缺字段按 0）。
pub(crate) fn extract_raw_tokens(data: &Value) -> (i64, i64, i64, i64, i64) {
    let tokens = data.get("tokens");
    let cache = tokens.and_then(|t| t.get("cache"));
    (
        as_int(tokens.and_then(|t| t.get("input"))),
        as_int(tokens.and_then(|t| t.get("output"))),
        as_int(cache.and_then(|c| c.get("read"))),
        as_int(cache.and_then(|c| c.get("write"))),
        as_int(tokens.and_then(|t| t.get("reasoning"))),
    )
}

/// token 归一（**全模块唯一的计费口径校准点**）。
///
/// 返回值顺序：`(input_tokens, output_tokens, cache_read, cache_write, reasoning)`
/// （前两项是落库口径，后三项原样透传）。映射依据见模块头（已用真实付费样本
/// 端到端验证）：
/// - `input  = raw_input + raw_cache_read` ← 本来源 input **不含** cache read，
///   而项目 `cost_for` 假定 `input_tokens` 已含 cache_read；
/// - `output = raw_output + raw_reasoning` ← reasoning 按 output 价计费；
/// - `reasoning` 原样返回，但**它是 output 的子集**，勿再与 output 相加当总量。
///
/// 若某天确认本来源（或某版本）的 input 已含 cache read，只需改下面第一个式子的
/// `+`，其余代码与测试语义不变。
pub(crate) fn normalize_tokens(
    raw_input: i64,
    raw_output: i64,
    raw_cache_read: i64,
    raw_cache_write: i64,
    raw_reasoning: i64,
) -> (i64, i64, i64, i64, i64) {
    // ↓↓↓ 唯一校准点：input 是否已含 cache read ↓↓↓
    let input_tokens = raw_input + raw_cache_read;
    let output_tokens = raw_output + raw_reasoning;
    (
        input_tokens,
        output_tokens,
        raw_cache_read,
        raw_cache_write,
        raw_reasoning,
    )
}

/// 提取 `(model_id, provider_id)`：**平铺优先**（v1 `$.modelID` / `$.providerID`），
/// 取不到再取嵌套（v2 实测 `$.model.id` / `$.model.providerID`，另兼容
/// `$.model.modelID` 这一别名写法）。两者都取不到时为空串（不臆造来源）；
/// `variant` 刻意不进 model_id（见模块头）。
pub(crate) fn extract_model_provider(data: &Value) -> (String, String) {
    let nested = data.get("model");
    let model_id = as_non_empty_str(data.get("modelID"))
        .or_else(|| as_non_empty_str(nested.and_then(|m| m.get("id"))))
        .or_else(|| as_non_empty_str(nested.and_then(|m| m.get("modelID"))))
        .unwrap_or_default();
    let provider_id = as_non_empty_str(data.get("providerID"))
        .or_else(|| as_non_empty_str(nested.and_then(|m| m.get("providerID"))))
        .unwrap_or_default();
    (model_id, provider_id)
}

/// 提取 OpenCode 自算成本（USD）：只认数值（字符串脏值忽略，口径同
/// `opencodego::query_records` 的 json_type 过滤），负值/非有限值忽略——
/// 该列只作「官方口径」参考，脏值不该污染。缺字段 → None（区分「免费模型
/// cost=0」与「没有 cost 数据」两种情况）。
pub(crate) fn extract_reported_cost(data: &Value) -> Option<f64> {
    data.get("cost")
        .and_then(|c| c.as_f64())
        .filter(|c| c.is_finite() && *c >= 0.0)
}

/// 提取 `(起始毫秒, 耗时毫秒)`：起始取 `$.time.created`，缺失回退源表
/// `time_created` 列（v1 早期行可能只在列上有时间）；耗时 =
/// `$.time.completed - $.time.created`（缺任一、倒挂或非正 → None，
/// 绝不用 0 冒充「瞬时完成」）。两个时间都是**毫秒 epoch**。
pub(crate) fn extract_timing(
    data: &Value,
    column_time_created: Option<i64>,
) -> (Option<i64>, Option<f64>) {
    let time = data.get("time");
    let created = time
        .and_then(|t| t.get("created"))
        .and_then(|c| c.as_i64())
        .or(column_time_created);
    let completed = time.and_then(|t| t.get("completed")).and_then(|c| c.as_i64());
    let duration_ms = match (created, completed) {
        (Some(created), Some(completed)) if completed > created => {
            Some((completed - created) as f64)
        }
        _ => None,
    };
    (created, duration_ms)
}

/// 行级 cwd 兜底：会话表缺 `directory` 时用消息自带的 `$.path.cwd`
/// （v1 的 assistant 行实测带 `path: {cwd, root}`；v2 未见该字段）。
fn extract_row_cwd(data: &Value) -> Option<String> {
    as_non_empty_str(data.get("path").and_then(|p| p.get("cwd")))
}

// ============================================================
// 源库读取（会话指纹 / 元信息 / 消息行）
// ============================================================

/// 单个会话在源库里的指纹（三元组，任一变化都要重导）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionFingerprint {
    pub session_id: String,
    /// 源库 assistant 行数（用于发现「消息被删除」这类修订）
    pub rows: i64,
    /// 源库最大 `time_updated`（毫秒；缺列时回退 `time_created`，再缺为 0）
    pub watermark: i64,
    /// 源库 `time_updated` 之和（同上的回退口径）。
    /// 为什么不能只看 MAX：老消息被编辑时 MAX 可能不变，Σ 会变——这是
    /// 「会话被编辑」三种情形里最容易漏的一种。
    pub revision_sum: i64,
}

/// 源库每个会话的指纹。
///
/// 三个分量各自的职责：MAX 管「追加了新消息」（新行时刻最大）、Σ 管「任意一行被
/// 改写」（时刻被推进）、行数管「消息被删除」（删掉最后一条时 MAX 反而变小）。
/// 只留其中之一都会有漏检的修订路径。
fn collect_session_fingerprints(
    conn: &Connection,
    shape: SourceShape,
) -> Result<Vec<SessionFingerprint>, String> {
    let sql = match shape {
        SourceShape::V2 => format!(
            "SELECT session_id, COUNT(*),
                    COALESCE(MAX(time_updated), MAX(time_created), 0),
                    COALESCE(SUM(COALESCE(time_updated, time_created, 0)), 0)
             FROM session_message WHERE {} GROUP BY session_id",
            shape.assistant_filter()
        ),
        SourceShape::V1 => format!(
            "SELECT {sid}, COUNT(*),
                    COALESCE(MAX(time_updated), MAX(time_created), 0),
                    COALESCE(SUM(COALESCE(time_updated, time_created, 0)), 0)
             FROM message WHERE {filter} GROUP BY 1",
            sid = v1_session_id_expr(conn),
            filter = shape.assistant_filter()
        ),
    };
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 OpenCode 会话指纹查询失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SessionFingerprint {
                session_id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                rows: row.get(1)?,
                watermark: row.get(2)?,
                revision_sum: row.get(3)?,
            })
        })
        .map_err(|e| format!("读取 OpenCode 会话指纹失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 会话指纹失败: {e}"))?;
    // 会话 id 缺失（v1 脏数据）无法记录水位，跳过而不是归到一个空 id 上
    Ok(rows
        .into_iter()
        .filter(|f| !f.session_id.is_empty())
        .collect())
}

/// 源库的会话元信息（会话 → 项目/目录映射）。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SessionMeta {
    pub project_id: String,
    /// 会话工作目录（优先；实测 v2 形如 `C:/Users/.../项目`）
    pub directory: Option<String>,
    /// 项目 worktree（`directory` 缺失时的兜底）
    pub worktree: Option<String>,
    /// 父会话 id（子 agent 会话非空；本模块只落库备查，不推断层级）
    pub parent_id: String,
    pub time_created: i64,
}

impl SessionMeta {
    /// 会话的原始目录：`directory` 优先，`worktree` 兜底，空白归一为 None。
    fn cwd(&self) -> Option<String> {
        self.directory.clone().or_else(|| self.worktree.clone())
    }
}

/// 读取会话元信息；v1 列名不确定或表缺失时返回空映射（只影响项目维度，
/// 不影响用量导入——行级 `$.path.cwd` 兜底仍然生效）。
fn load_session_meta(conn: &Connection, shape: SourceShape) -> HashMap<String, SessionMeta> {
    match shape {
        SourceShape::V2 => load_session_meta_v2(conn),
        SourceShape::V1 => load_session_meta_v1(conn),
    }
    .unwrap_or_default()
}

/// v2 元信息：`session_v2` LEFT JOIN `project` 取 worktree。
fn load_session_meta_v2(conn: &Connection) -> Result<HashMap<String, SessionMeta>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.id,
                    COALESCE(s.project_id, ''),
                    NULLIF(TRIM(COALESCE(s.directory, '')), ''),
                    NULLIF(TRIM(COALESCE(p.worktree, '')), ''),
                    COALESCE(s.parent_id, ''),
                    COALESCE(s.time_created, 0)
             FROM session_v2 s LEFT JOIN project p ON p.id = s.project_id",
        )
        .map_err(|e| format!("准备 OpenCode 会话元信息查询失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                SessionMeta {
                    project_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    directory: row.get(2)?,
                    worktree: row.get(3)?,
                    parent_id: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    time_created: row.get(5)?,
                },
            ))
        })
        .map_err(|e| format!("读取 OpenCode 会话元信息失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 会话元信息失败: {e}"))?;
    Ok(rows.into_iter().collect())
}

/// v1 元信息：`session` 表的列名各版本略有差异（`projectID` / `directory` /
/// `parentID` 未必都在），故按列存在性拼 SQL——缺列退化成常量，最多丢掉项目
/// 维度，绝不因缺列让整轮导入失败。
fn load_session_meta_v1(conn: &Connection) -> Result<HashMap<String, SessionMeta>, String> {
    if !table_exists(conn, "session") {
        return Ok(HashMap::new());
    }
    let project_expr = if db::has_column(conn, "session", "projectID") {
        "COALESCE(projectID, '')"
    } else {
        "''"
    };
    let directory_expr = if db::has_column(conn, "session", "directory") {
        "NULLIF(TRIM(COALESCE(directory, '')), '')"
    } else {
        "NULL"
    };
    let parent_expr = if db::has_column(conn, "session", "parentID") {
        "COALESCE(parentID, '')"
    } else {
        "''"
    };
    let created_expr = if db::has_column(conn, "session", "time_created") {
        "COALESCE(time_created, 0)"
    } else {
        "0"
    };
    let sql = format!(
        "SELECT id, {project_expr}, {directory_expr}, NULL, {parent_expr}, {created_expr} FROM session"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 OpenCode v1 会话元信息查询失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                SessionMeta {
                    project_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    directory: row.get(2)?,
                    worktree: None,
                    parent_id: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    time_created: row.get(5)?,
                },
            ))
        })
        .map_err(|e| format!("读取 OpenCode v1 会话元信息失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode v1 会话元信息失败: {e}"))?;
    Ok(rows.into_iter().collect())
}

/// 源库一行消息（只取导入需要的三列）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SourceMessage {
    pub id: String,
    pub time_created: Option<i64>,
    pub data: String,
}

/// 取某会话的全部 assistant 行（按时间 + id 排序，保证重导时行号稳定）。
fn fetch_session_rows(
    conn: &Connection,
    shape: SourceShape,
    session_id: &str,
) -> Result<Vec<SourceMessage>, String> {
    let sql = match shape {
        SourceShape::V2 => format!(
            "SELECT id, time_created, data FROM {} WHERE {} AND session_id = ?1
             ORDER BY time_created, id",
            shape.message_table(),
            shape.assistant_filter()
        ),
        SourceShape::V1 => format!(
            "SELECT id, time_created, data FROM message
             WHERE {filter} AND {sid} = ?1
             ORDER BY time_created, id",
            filter = shape.assistant_filter(),
            sid = v1_session_id_expr(conn)
        ),
    };
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 OpenCode 会话消息查询失败: {e}"))?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok(SourceMessage {
                id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                time_created: row.get(1)?,
                data: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })
        .map_err(|e| format!("读取 OpenCode 会话消息失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 会话消息失败: {e}"))?;
    Ok(rows)
}

// ============================================================
// 行构造与落库
// ============================================================

/// 落库前的单行用量（口径已归一，见 `normalize_tokens`）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UsageRecord {
    pub dedupe_key: String,
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
}

/// 一个会话的导入结果：用量行 + 行级 cwd 兜底。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SessionImport {
    pub records: Vec<UsageRecord>,
    /// 会话表给不出目录时的行级兜底（首个非空 `$.path.cwd`）
    pub cwd_fallback: Option<String>,
}

/// 把源库消息行解析成用量行（纯函数，坏行跳过而不阻断整个会话）。
///
/// 去重键 `dedupe_key = "<session_id>|<message.id>"`：`session_message.id` 是源库
/// 主键，天然唯一；id 缺失（理论不可达）时退化为「会话 + 起始时刻 + 序号」，
/// 序号来自本函数内确定性排序后的行号，重导时稳定，仍然幂等。
pub(crate) fn build_session_import(
    session_id: &str,
    rows: &[SourceMessage],
) -> SessionImport {
    let mut import = SessionImport::default();
    for (index, row) in rows.iter().enumerate() {
        // 坏 JSON 行跳过（源库被外部工具写过也不该让整轮导入失败）
        let Ok(data) = serde_json::from_str::<Value>(&row.data) else {
            continue;
        };
        let (created, duration_ms) = extract_timing(&data, row.time_created);
        // 拿不到起始时刻的行无法落时间轴（口径同 opencodego：跳过而非塞 0）
        let Some(started_at) = created else {
            continue;
        };
        let raw = extract_raw_tokens(&data);
        let (input_tokens, output_tokens, cache_read, cache_write, reasoning) =
            normalize_tokens(raw.0, raw.1, raw.2, raw.3, raw.4);
        // 全零行是占位（如请求失败后补写的空 assistant 消息），不落库
        if input_tokens == 0 && output_tokens == 0 && cache_write == 0 {
            continue;
        }
        let (model_id, provider_id) = extract_model_provider(&data);
        let dedupe_key = if row.id.is_empty() {
            format!("{session_id}|@{started_at}|{index}")
        } else {
            format!("{session_id}|{}", row.id)
        };
        if import.cwd_fallback.is_none() {
            import.cwd_fallback = extract_row_cwd(&data);
        }
        import.records.push(UsageRecord {
            dedupe_key,
            started_at,
            model_id,
            provider_id,
            input_tokens,
            output_tokens,
            cache_read,
            cache_write,
            reasoning,
            // 只算一次：reasoning 是 output 的子集（见模块头）
            computed_total: input_tokens + output_tokens,
            duration_ms,
            reported_cost_usd: extract_reported_cost(&data),
        });
    }
    import
}

/// 把会话指纹映射成 `(watermark, revision_sum, message_count)`，供指纹比对。
fn load_progress(conn: &Connection) -> Result<HashMap<String, (i64, i64, i64)>, String> {
    let mut stmt = conn
        .prepare("SELECT session_id, watermark, revision_sum, message_count FROM session_progress")
        .map_err(|e| format!("读取 OpenCode 导入水位失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?),
            ))
        })
        .map_err(|e| format!("读取 OpenCode 导入水位失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 导入水位失败: {e}"))?;
    Ok(rows.into_iter().collect())
}

/// 指纹比对的元组形态（顺序与 `load_progress` 一致）。
fn fingerprint_key(fingerprint: &SessionFingerprint) -> (i64, i64, i64) {
    (
        fingerprint.watermark,
        fingerprint.revision_sum,
        fingerprint.rows,
    )
}

/// 写入单个会话：**按 dedupe_key 原地 upsert**，再清掉源库里已不存在的行
/// （差集清理），最后写入会话指纹（同一事务）。
///
/// 为什么不再「先删该会话旧行、再整会话重插」：删重插会给同一会话的**每一行**
/// 换一个新的自增 id，而同步上传的服务端按 `(device_id, source, local_rowid)`
/// 去重（见 server/db.py）⇒ 重导出来的行在服务端落成一批**新行**，旧行仍在，
/// 同一笔用量被算两次。OpenCode 的活跃会话每追加一条消息就整会话重导一次
/// （指纹变了就重导），重导 n 次就是服务端 n 份，放大极快。原地 upsert 让
/// dedupe_key 不变的行**保持 id**（服务端按同一主键覆盖，配合修订补传通道
/// 修正内容），只有源库真删掉的消息才需要清掉——用临时键表做差集，避免
/// `NOT IN (...)` 的参数个数上限（会话行数可能上千）。
///
/// 已知上限：源库删掉的消息，服务端没有删除通道，远端会保留它曾经上传的用量
/// （客户端本地口径仍然准确）；这与 claude/kimi/dsh 各来源一致。
fn write_session(
    conn: &mut Connection,
    fingerprint: &SessionFingerprint,
    import: &SessionImport,
    meta: Option<&SessionMeta>,
    shape: SourceShape,
    now: i64,
) -> Result<(), String> {
    let session_id = fingerprint.session_id.as_str();
    // 目录：会话表 directory → project worktree → 行级 $.path.cwd
    let cwd = meta.and_then(|m| m.cwd()).or_else(|| import.cwd_fallback.clone());
    let project_key = cwd.as_deref().and_then(crate::projects::normalize_cwd);

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启 OpenCode 导入事务失败: {e}"))?;
    // 本次导入的键集合（临时表，仅本连接可见；事务外无残留）。末尾用它做差集，
    // 找出「上一次导入过、这次源库里已经没有了」的消息。
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS opencode_import_keys (dedupe_key TEXT PRIMARY KEY);
         DELETE FROM opencode_import_keys;",
    )
    .map_err(|e| format!("准备 OpenCode 导入键集合失败: {e}"))?;

    for record in &import.records {
        tx.execute(
            "INSERT OR IGNORE INTO opencode_import_keys (dedupe_key) VALUES (?1)",
            [&record.dedupe_key],
        )
        .map_err(|e| format!("记录 OpenCode 导入键失败: {e}"))?;
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
                -- 采纳的修订（总量变大，或空模型名被回填）才推进它。否则活跃会话
                -- 每追加一条消息就会把整会话的行标记成「已修订」，同步侧每轮都要
                -- 全量重传（服务端对未变行是无操作，纯浪费带宽：万行会话就是
                -- 万行/轮）。判定条件与服务端 server/db.py 的覆盖守卫逐条对齐。
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
                session_id,
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
                cwd,
                project_key,
                record.reported_cost_usd,
            ],
        )
        .map_err(|e| format!("写入 OpenCode 用量记录失败: {e}"))?;
    }

    // 幽灵行清理：源库里已经删掉的消息（消息改写/删除/回退），派生库里不该继续
    // 留着——否则本地统计会比源库多算。只删本会话、且不在本次导入键集合里的行；
    // 键仍存在的行保持原 id（这正是消灭服务端重复计数的前提，见函数注释）。
    tx.execute(
        "DELETE FROM model_usage
          WHERE session_id = ?1
            AND dedupe_key NOT IN (SELECT dedupe_key FROM opencode_import_keys)",
        [session_id],
    )
    .map_err(|e| format!("清理 OpenCode 幽灵行失败: {e}"))?;

    // 会话元信息（项目维度与排障用；行上已冗余 cwd/project_key，这里留全量）
    let meta = meta.cloned().unwrap_or_default();
    tx.execute(
        "INSERT INTO session_meta
            (session_id, project_id, directory, worktree, parent_id, time_created,
             source_table, captured_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(session_id) DO UPDATE SET
            project_id = excluded.project_id,
            directory = excluded.directory,
            worktree = excluded.worktree,
            parent_id = excluded.parent_id,
            time_created = excluded.time_created,
            source_table = excluded.source_table,
            captured_at = excluded.captured_at",
        rusqlite::params![
            session_id,
            meta.project_id,
            meta.directory,
            meta.worktree,
            meta.parent_id,
            meta.time_created,
            shape.message_table(),
            now,
        ],
    )
    .map_err(|e| format!("写入 OpenCode 会话元信息失败: {e}"))?;

    tx.execute(
        "INSERT INTO session_progress
            (session_id, watermark, revision_sum, message_count, imported_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(session_id) DO UPDATE SET
            watermark = excluded.watermark,
            revision_sum = excluded.revision_sum,
            message_count = excluded.message_count,
            imported_at = excluded.imported_at",
        rusqlite::params![
            session_id,
            fingerprint.watermark,
            fingerprint.revision_sum,
            fingerprint.rows,
            now
        ],
    )
    .map_err(|e| format!("记录 OpenCode 导入水位失败: {e}"))?;

    tx.commit()
        .map_err(|e| format!("提交 OpenCode 导入事务失败: {e}"))
}

/// 单轮导入的结果（诊断与单测断言用）。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ImportOutcome {
    /// 源库形态标记："v2" / "v1" / "none"
    pub shape: String,
    /// 源库出现的会话数（有 assistant 行的）
    pub source_sessions: i64,
    /// 本轮因指纹变化而重导的会话数
    pub reimported_sessions: i64,
    /// 本轮写入的用量行数
    pub records_written: i64,
}

/// 增量导入内核（源库/派生库连接注入，不碰任何全局路径；生产与单测共用）。
///
/// 幂等保证：同一份源数据重复导入时，第二次的会话指纹与水位完全一致 → 整会话
/// 跳过 → 派生库内容与自增 id 全都不变（重导同一份数据也只会原地 upsert，
/// 存活行的 id 不漂移，见 `write_session`）。
pub(crate) fn import_from_source(
    src: &Connection,
    conn: &mut Connection,
    now: i64,
) -> Result<ImportOutcome, String> {
    let Some(shape) = detect_shape(src) else {
        // 库在但没有 v1/v2 消息表：不是错误，静默（tab 由 presence 决定不出现）
        return Ok(ImportOutcome {
            shape: "none".to_string(),
            ..Default::default()
        });
    };
    let fingerprints = collect_session_fingerprints(src, shape)?;
    let meta = load_session_meta(src, shape);
    let progress = load_progress(conn)?;
    let mut outcome = ImportOutcome {
        shape: shape.label().to_string(),
        source_sessions: fingerprints.len() as i64,
        ..Default::default()
    };
    for fingerprint in &fingerprints {
        let known = progress.get(&fingerprint.session_id);
        if known == Some(&fingerprint_key(fingerprint)) {
            continue; // 三元指纹完全一致 → 该会话无新增/无修订
        }
        let rows = fetch_session_rows(src, shape, &fingerprint.session_id)?;
        let import = build_session_import(&fingerprint.session_id, &rows);
        write_session(
            conn,
            fingerprint,
            &import,
            meta.get(&fingerprint.session_id),
            shape,
            now,
        )?;
        outcome.reimported_sessions += 1;
        outcome.records_written += import.records.len() as i64;
    }
    Ok(outcome)
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
/// 未装 OpenCode（库不存在）或源库无可识别表时静默返回（不是错误）。
pub fn import_incremental_force() -> Result<(), String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = crate::opencodego::db_path();
    if !path.exists() {
        return Ok(());
    }
    let src = open_source_readonly(&path)?;
    if detect_shape(&src).is_none() {
        return Ok(()); // 不在有数据时白建一个空派生库
    }
    let mut conn = open_opencode_db()?;
    import_from_source(&src, &mut conn, crate::provider_quota::now_ms())?;
    Ok(())
}

// ============================================================
// 查询（与 dsh.rs 同名同构；查询前先增量导入）
// ============================================================

/// 查询 [from_ms, to_ms) 内的统计。
///
/// 与 dsh 的差异：本库 `time_to_first_token_ms` 恒 NULL（见模块头），故按
/// **无 TTFT** 聚合——生成窗口退化为请求总耗时，速度质量标 `RequestAverage`
/// （与 claude/kimi 同款，前端带 ≈ 前缀），不冒充 Generation。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
    query_stats_at(&conn, from_ms, to_ms)
}

/// `query_stats` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_stats_at(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
) -> Result<db::Stats, String> {
    // 校准点：本库 ttft 列恒 NULL。若将来确认 `$.time.streamed` = 首字时刻并回填
    // 该列，把下面的 false 换成 db::has_column(conn, "model_usage",
    // "time_to_first_token_ms")，速度即切到「总耗时 − TTFT」的真生成窗口。
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
        .map_err(|e| format!("查询 OpenCode 整体统计失败: {e}"))?;

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
        .map_err(|e| format!("准备 OpenCode 模型分组查询失败: {e}"))?;

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
        .map_err(|e| format!("读取 OpenCode 模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 模型分组失败: {e}"))?;

    // 速度质量：本库只有请求总耗时，没有真生成窗口 → 请求平均（前端带 ≈）
    if overall.speed.avg_tps.is_some() {
        overall.speed.speed_quality = Some(crate::token_speed::SpeedQuality::RequestAverage);
    }
    for model in &mut by_model {
        if model.speed.avg_tps.is_some() {
            model.speed.speed_quality = Some(crate::token_speed::SpeedQuality::RequestAverage);
        }
    }

    db::fill_duration_metrics(&conn, "opencode", from_ms, to_ms, &mut overall, &mut by_model)?;

    let (earliest_ms, latest_ms): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(started_at), MAX(started_at) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("查询 OpenCode 时间范围失败: {e}"))?;

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
    let conn = open_opencode_db()?;
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
        .map_err(|e| format!("准备 OpenCode 趋势查询失败: {e}"))?;

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
            .map_err(|e| format!("读取 OpenCode 趋势统计失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 OpenCode 趋势统计失败: {e}"))?;

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
    let conn = open_opencode_db()?;
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
        .map_err(|e| format!("准备 OpenCode 模型列表查询失败: {e}"))?;
    let models = stmt
        .query_map([], |row| {
            Ok(db::ModelInfo {
                provider_id: row.get(0)?,
                model_id: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取 OpenCode 模型列表失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 模型列表失败: {e}"))?;
    Ok(models)
}

/// 按指定周期聚合 Token（对比页需要真实 [reset_at, end_at) 边界）。
pub fn query_period_buckets(periods: &[(i64, i64)]) -> Result<Vec<db::PeriodBucket>, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
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
        .map_err(|e| format!("准备 OpenCode 周期聚合查询失败: {e}"))?;
    let mut out = Vec::with_capacity(periods.len());
    for &(reset_at, end_at) in periods {
        let (total_tokens, requests): (i64, i64) = stmt
            .query_row(rusqlite::params![reset_at, end_at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("查询 OpenCode 周期聚合失败: {e}"))?;
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
/// source 固定 "opencode"，local_rowid = 自增 id；带 proto 5 的会话/项目维度。
pub fn query_since(since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
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
/// 为什么本来源也需要修订通道：会话被重导（追加消息 / 编辑消息）时，**未变的行
/// 内容若被改写，自增 id 不变**（原地 upsert，见 `write_session`）——`id > 游标`
/// 查询永远选不出它，服务端会一直保留旧小值。本查询按 `updated_at`（每次重导都会
/// 推进）选出这些行重传，由服务端「总量更大者胜」的 upsert 覆盖修正。
/// after_id 分页：按 id 升序、只取 id > after_id 的行，调用方逐批推进。
pub fn query_revised_since(
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
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
/// 两者列口径完全一致，只有过滤条件不同）。proto 8：附带耗时/首字
/// （本库 duration_ms 为真实请求总耗时；time_to_first_token_ms 列存在但
/// 恒 NULL，见模块头 TTFT 说明 → 首字实际恒 None）。
fn query_rows_at(
    conn: &Connection,
    tail: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<db::UsageRow>, String> {
    // proto 8：耗时/首字按列存在性带出（真实值 > 0 才给，无列恒 None，
    // 不做时间戳差估算；口径与 db::query_since 一致）
    let (duration, ttft) = db::speed_columns(conn);
    let sql = format!(
        "SELECT id, started_at, model_id, provider_id,
                input_tokens, output_tokens, cache_read_input_tokens,
                cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                session_id, project_key, cwd, {duration}, {ttft}
         FROM model_usage {tail}"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 OpenCode 明细查询失败: {e}"))?;
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
                source: "opencode".to_string(),
                session_id: row.get(10)?,
                project_key: row.get(11)?,
                project_display: row.get(12)?,
                duration_ms: row.get(13)?,
                time_to_first_token_ms: row.get(14)?,
            })
        })
        .map_err(|e| format!("读取 OpenCode 明细失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 明细失败: {e}"))?;
    Ok(rows)
}

/// 会话悬浮窗（session_hud）用：查询活跃窗口内的会话行。
///
/// 与 dsh 的差异：本来源没有 TTFT、也没有「生成窗口」（`duration_ms` 是请求总
/// 耗时），故 `ttft_ms` / `gen_out_tokens` / `gen_ms` 恒空，速度只走请求平均
/// 通道（session_hud 侧同理按 RequestAverage 展示）；`generating` 恒 false
/// （OpenCode 的运行中状态在 `session_v2.time_idle` 等字段上，语义未确认，
/// 不臆造生成中状态）。`input_tokens` 在本库**已含** cache_read，故非缓存输入 =
/// `input_tokens − cache_read_input_tokens`。
pub fn query_window_sessions(
    window_start_ms: i64,
    limit: usize,
) -> Result<crate::session_hud::AgentWindowSessions, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
    query_window_sessions_at(&conn, window_start_ms, limit)
}

/// 会话悬浮窗模型速度区：窗口内按模型的速度摘要。
///
/// 口径 = **请求平均**（本库 `duration_ms` 是整请求耗时，没有生成窗口原料，
/// 见模块头；前端带 ≈），聚合复用各来源派生库通用的
/// `session_hud::collect_derived_model_speeds`。
pub fn query_window_model_speeds(
    window_start_ms: i64,
    limit: usize,
) -> Result<Vec<crate::session_hud::HudModelSpeed>, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
    crate::session_hud::collect_derived_model_speeds(
        &conn,
        window_start_ms,
        limit,
        crate::local_agents::OPENCODE,
        crate::token_speed::SpeedQuality::RequestAverage,
    )
}

/// `query_window_sessions` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn query_window_sessions_at(
    conn: &Connection,
    window_start_ms: i64,
    limit: usize,
) -> Result<crate::session_hud::AgentWindowSessions, String> {
    /// 内层候选行上限（与 claude/dsh 同量级：限制成本，不全表扫）。
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
        .map_err(|e| format!("准备 OpenCode 会话窗口查询失败: {e}"))?;
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
        .map_err(|e| format!("读取 OpenCode 会话窗口失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 会话窗口失败: {e}"))?;
    let total_active: usize = conn
        .query_row(
            "SELECT COUNT(DISTINCT session_id) FROM model_usage WHERE started_at >= ?1",
            [window_start_ms],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n.max(0) as usize)
        .map_err(|e| format!("统计 OpenCode 活跃会话数失败: {e}"))?;
    Ok(crate::session_hud::AgentWindowSessions {
        total_active,
        sessions,
    })
}

/// 按项目 × 模型聚合 [from_ms, to_ms) 的用量（SQL 侧 GROUP BY，不逐行加载）。
/// project_key 为 NULL/空串（未回填或无目录）归入 unknown 哨兵键。
pub fn query_project_model_rows(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<crate::projects::ProjectModelRow>, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
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
        .map_err(|e| format!("准备 OpenCode 项目聚合查询失败: {e}"))?;
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
        .map_err(|e| format!("读取 OpenCode 项目聚合失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 项目聚合失败: {e}"))?;
    Ok(rows)
}

/// 按项目统计 [from_ms, to_ms) 内的会话数（COUNT(DISTINCT session_id)）。
pub fn query_project_session_counts(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<(String, i64)>, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
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
        .map_err(|e| format!("准备 OpenCode 项目会话统计失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![crate::projects::UNKNOWN_PROJECT, from_ms, to_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("读取 OpenCode 项目会话统计失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 项目会话统计失败: {e}"))?;
    Ok(rows)
}

/// 项目键 → 原始形态 cwd（保留大小写，取字典序最小者做代表）。
pub fn query_project_display_paths() -> Result<Vec<(String, String)>, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
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
        .map_err(|e| format!("准备 OpenCode 项目路径查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![crate::projects::UNKNOWN_PROJECT], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("读取 OpenCode 项目路径失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 项目路径失败: {e}"))?;
    Ok(rows)
}

/// 分页查询指定项目的会话明细（按会话最后活跃时间降序）。
/// 返回 (匹配会话总数, 按会话 × 模型聚合的行)；offset/limit 作用在会话粒度。
/// project_key 传 UNKNOWN_PROJECT 时匹配 NULL/空串/哨兵的会话。
/// 与 dsh 的差异：无 TTFT（`has_ttft = false`），会话级速度是请求平均口径。
pub fn query_project_sessions(
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Result<(u32, Vec<crate::projects::ProjectSessionModelRow>), String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
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
        .map_err(|e| format!("查询 OpenCode 项目会话总数失败: {e}"))?;

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
        .map_err(|e| format!("准备 OpenCode 项目会话查询失败: {e}"))?;
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
        .map_err(|e| format!("读取 OpenCode 项目会话失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 OpenCode 项目会话失败: {e}"))?;
    Ok((total.max(0) as u32, rows))
}

/// 库内最大自增 id（「待上传条数」展示用，与 claude/codex/dsh 同款）。
/// 不触发增量导入（只读游标口径，避免为一次计数做全量扫描）。
pub fn max_rowid() -> Result<i64, String> {
    let conn = open_opencode_db()?;
    max_rowid_at(&conn)
}

/// `max_rowid` 的纯查询内核（连接注入版，供单元测试）。
pub(crate) fn max_rowid_at(conn: &Connection) -> Result<i64, String> {
    let max: Option<i64> = conn
        .query_row("SELECT MAX(id) FROM model_usage", [], |row| row.get(0))
        .map_err(|e| format!("查询 OpenCode 最大 rowid 失败: {e}"))?;
    Ok(max.unwrap_or(0))
}

/// 累计的 OpenCode 自算成本（USD）汇总，供上层决定「官方口径 vs 价格表重算」。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ReportedCostSummary {
    /// 有 cost 数据的行合计（USD）
    pub total_usd: f64,
    /// 有 cost 数据的行数
    pub priced_rows: i64,
    /// 总行数（差值即「源库没给 cost」的行，如旧版本写入的记录）
    pub total_rows: i64,
}

/// 查询 [from_ms, to_ms) 内 OpenCode 自算成本合计（不触发增量导入之外的动作）。
pub fn query_reported_cost(from_ms: i64, to_ms: i64) -> Result<ReportedCostSummary, String> {
    import_incremental()?;
    let conn = open_opencode_db()?;
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
    .map_err(|e| format!("查询 OpenCode 自算成本失败: {e}"))
}

// ============================================================
// 诊断
// ============================================================

/// 诊断信息（设置页/排障用）。
#[derive(Debug, serde::Serialize)]
pub struct OpencodeDebugInfo {
    /// 源库路径（opencode.db）
    pub db_path: String,
    pub db_exists: bool,
    /// 源库形态："v2" / "v1" / "none"
    pub source_shape: String,
    /// 源库有 assistant 行的会话数
    pub source_sessions: i64,
    /// 源库 assistant 行数
    pub source_messages: i64,
    /// 派生库记录数
    pub imported_records: i64,
    /// 派生库最新一条记录时间
    pub latest_record_ms: Option<i64>,
    /// OpenCode 自算成本合计（USD）
    pub reported_cost_usd: f64,
    /// 带自算成本的行数（与 imported_records 的差值 = 源库没给 cost 的行）
    pub reported_cost_rows: i64,
}

pub fn debug_info() -> Result<OpencodeDebugInfo, String> {
    let path = crate::opencodego::db_path();
    let exists = path.exists();
    let (source_shape, source_sessions, source_messages) = match open_source_readonly(&path) {
        Ok(conn) => source_debug(&conn),
        Err(_) => ("none".to_string(), 0, 0),
    };
    let (imported_records, latest_record_ms, reported_cost_usd, reported_cost_rows) =
        match open_opencode_db() {
            Ok(conn) => derived_debug(&conn),
            Err(_) => (0, None, 0.0, 0),
        };
    Ok(OpencodeDebugInfo {
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

/// 源库侧诊断字段：`(形态, 有 assistant 行的会话数, assistant 行数)`。
pub(crate) fn source_debug(conn: &Connection) -> (String, i64, i64) {
    let Some(shape) = detect_shape(conn) else {
        return ("none".to_string(), 0, 0);
    };
    match collect_session_fingerprints(conn, shape) {
        Ok(fingerprints) => (
            shape.label().to_string(),
            fingerprints.len() as i64,
            fingerprints.iter().map(|f| f.rows).sum(),
        ),
        Err(_) => (shape.label().to_string(), 0, 0),
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

    /// 真实样本 A（免费模型，用户在本机 OpenCode v2 实跑一轮后原样摘录）：
    /// 嵌套 model/provider、`cost` 为 0、tokens 四类齐全。
    /// 时刻为实测值；`streamed` 语义未确认故不参与断言（见模块头 TTFT 说明）。
    const REAL_FREE_ASSISTANT: &str = r#"{"time":{"created":1791831838192,"streamed":1791831841544,"completed":1791831841546},"agent":"build","model":{"id":"step-5-preview-free","providerID":"opencode","variant":"high"},"finish":"stop","cost":0,"tokens":{"input":7476,"output":100,"reasoning":42,"cache":{"read":192,"write":0}}}"#;

    /// 真实样本 A'（付费订阅，实测原值）：opencode-go / deepseek-v4.1-flash。
    /// `$.cost` 由 OpenCode 按 models.dev 价（0.15 / 0.6 / 0.003）算得
    /// `(3501*0.15 + 63*0.6 + 2816*0.003)/1e6 = 0.000571398`（精确命中）。
    /// 起始时刻未从源库摘录，用合成值占位（不影响 token/cost 断言）。
    const REAL_PAID_ASSISTANT_A: &str = r#"{"time":{"created":1791832000000,"completed":1791832002091},"model":{"id":"deepseek-v4.1-flash","providerID":"opencode-go","variant":"default"},"finish":"stop","cost":0.000571398,"tokens":{"input":3501,"output":63,"reasoning":0,"cache":{"read":2816,"write":0}}}"#;

    /// 真实样本 B（付费，reasoning 非零）：
    /// `(6179*0.15 + (91+12)*0.6 + 1536*0.003)/1e6 = 0.000993258`（精确命中；
    /// 不含 reasoning 则得 0.00098606，证明 reasoning 按 output 价计费）。
    const REAL_PAID_ASSISTANT_B: &str = r#"{"time":{"created":1791832100000,"completed":1791832102091},"model":{"id":"deepseek-v4.1-flash","providerID":"opencode-go","variant":"default"},"finish":"stop","cost":0.000993258,"tokens":{"input":6179,"output":91,"reasoning":12,"cache":{"read":1536,"write":0}}}"#;

    /// v1 平铺形态的 assistant 行（`$.modelID` / `$.providerID` / `$.role`，
    /// 附带 `$.path.cwd` 行级目录兜底）。
    const V1_ASSISTANT: &str = r#"{"role":"assistant","sessionID":"ses_v1","providerID":"anthropic","modelID":"claude-sonnet-4","cost":0.0123,"path":{"cwd":"C:/work/legacy"},"time":{"created":1700000000000,"completed":1700000005000},"tokens":{"input":1000,"output":200,"reasoning":50,"cache":{"read":300,"write":100}}}"#;

    /// 每个测试独立的临时目录（互不干扰；结束前删除）。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zbar-opencode-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir
    }

    /// 建**真实 v2 表结构**的源库（列名与实测库一致）：session_message /
    /// session_v2 / project，并写入一个项目 + 一个会话。
    fn create_v2_source(path: &Path) {
        let conn = Connection::open(path).expect("打开源库失败");
        conn.execute_batch(
            "CREATE TABLE session_message (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 type TEXT NOT NULL,
                 seq INTEGER,
                 time_created INTEGER,
                 time_updated INTEGER,
                 data TEXT NOT NULL
             );
             CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 directory TEXT,
                 title TEXT,
                 model TEXT,
                 cost REAL,
                 tokens_input INTEGER,
                 tokens_output INTEGER,
                 tokens_reasoning INTEGER,
                 tokens_cache_read INTEGER,
                 tokens_cache_write INTEGER,
                 time_created INTEGER,
                 time_updated INTEGER,
                 parent_id TEXT
             );
             CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT, name TEXT);
             INSERT INTO project (id, worktree, name) VALUES ('p1', 'C:/work/demo', 'demo');
             INSERT INTO session_v2
                 (id, project_id, directory, title, cost, tokens_input, tokens_output,
                  tokens_reasoning, tokens_cache_read, tokens_cache_write,
                  time_created, time_updated, parent_id)
             VALUES ('ses_a', 'p1', 'C:/work/demo', '演示', 0.0, 7476, 100, 42, 192, 0,
                     100, 200, '');",
        )
        .expect("建 v2 源库失败");
    }

    /// 建 v1 表结构的源库（`message` 无 type 列、角色在 `$.role`；`session`
    /// 只有 directory 列，缺 projectID/parentID —— 正是列存在性探测要覆盖的场景）。
    fn create_v1_source(path: &Path) {
        let conn = Connection::open(path).expect("打开源库失败");
        conn.execute_batch(
            "CREATE TABLE message (
                 id TEXT PRIMARY KEY,
                 sessionID TEXT,
                 role TEXT,
                 time_created INTEGER,
                 time_updated INTEGER,
                 data TEXT NOT NULL
             );
             CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, title TEXT);
             INSERT INTO session (id, directory, title) VALUES ('ses_v1', NULL, '旧会话');",
        )
        .expect("建 v1 源库失败");
    }

    fn insert_v2_message(
        conn: &Connection,
        id: &str,
        session_id: &str,
        kind: &str,
        seq: i64,
        time_created: i64,
        time_updated: i64,
        data: &str,
    ) {
        conn.execute(
            "INSERT INTO session_message
                (id, session_id, type, seq, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![id, session_id, kind, seq, time_created, time_updated, data],
        )
        .expect("插入源消息失败");
    }

    /// 派生库（临时文件，不碰真实 ~/.agenthud）。
    fn open_derived(dir: &Path) -> Connection {
        open_opencode_db_at(&dir.join("derived.sqlite")).expect("打开派生库失败")
    }

    /// model_usage 的一行快照（不含自增 id，用于幂等比对）。
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
    }

    fn dump_rows(conn: &Connection) -> Vec<RowDump> {
        let mut stmt = conn
            .prepare(
                "SELECT session_id, dedupe_key, started_at, model_id, provider_id,
                        input_tokens, output_tokens, cache_read_input_tokens,
                        cache_creation_input_tokens, reasoning_tokens,
                        computed_total_tokens, duration_ms, reported_cost_usd
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
            })
        })
        .expect("查询快照失败")
        .collect::<Result<Vec<_>, _>>()
        .expect("读取快照失败")
    }

    /// 派生库自增 id 列表（幂等断言用：水位未变时连 id 都不该动）。
    fn dump_ids(conn: &Connection) -> Vec<i64> {
        let mut stmt = conn
            .prepare("SELECT id FROM model_usage ORDER BY id")
            .expect("准备 id 查询失败");
        stmt.query_map([], |row| row.get(0))
            .expect("查询 id 失败")
            .collect::<Result<Vec<_>, _>>()
            .expect("读取 id 失败")
    }

    /// **两个真实付费样本 + 一个真实免费样本**：归一后字段值、自算成本存留，
    /// 并用 OpenCode 自己的价反算成本，钉住「input 不含 cache read、
    /// reasoning 按 output 价」两条口径。
    #[test]
    fn real_samples_pin_normalization_and_reported_cost() {
        // 用 OpenCode 二进制里的价（input/output/cache_read，USD/百万 token）
        // 反算它自己写的 cost：必须与 `$.cost` 精确一致。
        let recompute = |raw: (i64, i64, i64, i64, i64)| -> f64 {
            (raw.0 as f64 * 0.15
                + (raw.1 + raw.4) as f64 * 0.6
                + raw.2 as f64 * 0.003)
                / 1e6
        };

        // 样本 A（免费模型）：input 7476 / output 100 / reasoning 42 / cacheR 192
        let data: Value = serde_json::from_str(REAL_FREE_ASSISTANT).unwrap();
        let raw = extract_raw_tokens(&data);
        assert_eq!(raw, (7476, 100, 192, 0, 42));
        assert_eq!(
            normalize_tokens(raw.0, raw.1, raw.2, raw.3, raw.4),
            (7668, 142, 192, 0, 42),
            "免费样本归一：input 含 cache read、output 含 reasoning"
        );
        assert_eq!(7668 + 142, 7810, "computed_total = input + output");
        assert_eq!(extract_reported_cost(&data), Some(0.0));
        assert_eq!(
            extract_model_provider(&data),
            ("step-5-preview-free".to_string(), "opencode".to_string())
        );
        // 请求总耗时 3354ms（created → completed），不是 TTFT
        assert_eq!(
            extract_timing(&data, None),
            (Some(1791831838192), Some(3354.0))
        );

        // 样本 A'（付费，reasoning=0）：cost 精确命中
        let data: Value = serde_json::from_str(REAL_PAID_ASSISTANT_A).unwrap();
        let raw = extract_raw_tokens(&data);
        assert_eq!(raw, (3501, 63, 2816, 0, 0));
        assert_eq!(
            normalize_tokens(raw.0, raw.1, raw.2, raw.3, raw.4),
            (6317, 63, 2816, 0, 0)
        );
        assert_eq!(6317 + 63, 6380);
        let cost = extract_reported_cost(&data).expect("付费样本应带 cost");
        assert!(
            (recompute(raw) - cost).abs() < 1e-12,
            "反算 {} 应等于源库 cost {}",
            recompute(raw),
            cost
        );
        assert_eq!(
            extract_model_provider(&data),
            ("deepseek-v4.1-flash".to_string(), "opencode-go".to_string())
        );

        // 样本 B（付费，reasoning=12 → 按 output 价计费）
        let data: Value = serde_json::from_str(REAL_PAID_ASSISTANT_B).unwrap();
        let raw = extract_raw_tokens(&data);
        assert_eq!(raw, (6179, 91, 1536, 0, 12));
        assert_eq!(
            normalize_tokens(raw.0, raw.1, raw.2, raw.3, raw.4),
            (7715, 103, 1536, 0, 12)
        );
        assert_eq!(7715 + 103, 7818);
        let cost = extract_reported_cost(&data).expect("付费样本应带 cost");
        assert!(
            (recompute(raw) - cost).abs() < 1e-12,
            "reasoning 必须按 output 价计费：反算 {} vs 源库 {}",
            recompute(raw),
            cost
        );
        assert_eq!((recompute(raw) * 1e6).round(), 993.0);
    }

    /// 归一映射本身（顺序/子集关系）钉在单测里：input += cache_read、
    /// output += reasoning，reasoning 原样输出但不再叠加。
    #[test]
    fn normalize_tokens_pins_the_billing_mapping() {
        assert_eq!(normalize_tokens(10, 5, 3, 2, 4), (13, 9, 3, 2, 4));
        // 全零不 panic、不产生负值（脏负数源数据同理原样透传，不做猜测）
        assert_eq!(normalize_tokens(0, 0, 0, 0, 0), (0, 0, 0, 0, 0));
        // reasoning 是 output 的子集：总量里只出现一次
        let (input, output, _, _, reasoning) = normalize_tokens(100, 50, 20, 0, 7);
        assert_eq!((input, output), (120, 57));
        assert_eq!(input + output, 177);
        assert!(reasoning <= output);
    }

    /// 平铺（v1）优先、嵌套（v2）回退、空白串视为缺失、provider 原样不归一。
    #[test]
    fn model_provider_supports_both_shapes() {
        let flat: Value = serde_json::from_str(
            r#"{"providerID":"openai","modelID":"gpt-5","model":{"id":"ignored","providerID":"ignored"}}"#,
        )
        .unwrap();
        assert_eq!(
            extract_model_provider(&flat),
            ("gpt-5".to_string(), "openai".to_string())
        );
        let nested: Value =
            serde_json::from_str(r#"{"model":{"id":"glm-5","providerID":"zhipu"}}"#).unwrap();
        assert_eq!(
            extract_model_provider(&nested),
            ("glm-5".to_string(), "zhipu".to_string())
        );
        // 空白串按缺失处理 → 回退嵌套
        let blank: Value = serde_json::from_str(
            r#"{"modelID":"  ","providerID":"","model":{"id":"m","providerID":"p"}}"#,
        )
        .unwrap();
        assert_eq!(
            extract_model_provider(&blank),
            ("m".to_string(), "p".to_string())
        );
        // 两个形态都缺 → 空串（provider 不归一、不臆造）
        let empty: Value = serde_json::json!({});
        assert_eq!(extract_model_provider(&empty), (String::new(), String::new()));
    }

    /// 缺字段/脏字段的容错：不 panic、不用 0 冒充耗时、字符串 cost 忽略、
    /// 浮点 token 也能读。
    #[test]
    fn missing_and_dirty_fields_degrade_safely() {
        let empty = serde_json::json!({});
        assert_eq!(extract_raw_tokens(&empty), (0, 0, 0, 0, 0));
        assert_eq!(extract_reported_cost(&empty), None);
        assert_eq!(extract_timing(&empty, None), (None, None));
        // 源表时间列兜底（data 里没有 time.created 时）
        assert_eq!(extract_timing(&empty, Some(1700)), (Some(1700), None));
        // 脏 cost：字符串 / 负数 / 布尔 一律忽略
        for raw in [r#"{"cost":"1.25"}"#, r#"{"cost":-3}"#, r#"{"cost":true}"#] {
            let v: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(extract_reported_cost(&v), None, "脏 cost 必须忽略: {raw}");
        }
        // 时刻倒挂 / 相等 → 无耗时（绝不用 0 冒充「瞬时完成」）
        let inverted: Value =
            serde_json::from_str(r#"{"time":{"created":2000,"completed":1000}}"#).unwrap();
        assert_eq!(extract_timing(&inverted, None), (Some(2000), None));
        let same: Value =
            serde_json::from_str(r#"{"time":{"created":2000,"completed":2000}}"#).unwrap();
        assert_eq!(extract_timing(&same, None), (Some(2000), None));
        // 浮点 token（12.0）可读；缺 cache 对象时按 0
        let floats: Value =
            serde_json::from_str(r#"{"tokens":{"input":12.0,"output":0}}"#).unwrap();
        assert_eq!(extract_raw_tokens(&floats), (12, 0, 0, 0, 0));
    }

    /// v2 增量导入：首次入库 → 水位未变时整会话跳过（幂等，连自增 id 都不动）
    /// → 追加消息后只重导该会话。
    #[test]
    fn v2_import_is_incremental_and_idempotent() {
        let dir = temp_dir("v2-incr");
        let src_path = dir.join("opencode.db");
        create_v2_source(&src_path);
        let src = Connection::open(&src_path).expect("打开源库失败");
        insert_v2_message(
            &src,
            "msg_1",
            "ses_a",
            "assistant",
            6,
            1791831838192,
            1791831841547,
            REAL_FREE_ASSISTANT,
        );
        // 干扰行：user 行（只有模型选择，无 tokens/cost）不该产生用量行
        insert_v2_message(
            &src,
            "msg_0",
            "ses_a",
            "user",
            4,
            1791831827832,
            1791831827833,
            r#"{"time":{"created":1791831827832},"text":"你好"}"#,
        );
        let mut derived = open_derived(&dir);

        let first = import_from_source(&src, &mut derived, 1_000).expect("首次导入失败");
        assert_eq!(first.shape, "v2");
        assert_eq!(first.source_sessions, 1);
        assert_eq!(first.reimported_sessions, 1);
        assert_eq!(first.records_written, 1);
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 1, "user 行不产生用量行");
        let row = &rows[0];
        assert_eq!(row.session_id, "ses_a");
        assert_eq!(row.dedupe_key, "ses_a|msg_1");
        assert_eq!(row.started_at, 1791831838192);
        assert_eq!(row.model_id, "step-5-preview-free");
        assert_eq!(row.provider_id, "opencode");
        assert_eq!(
            (row.input, row.output, row.cache_read, row.cache_write, row.reasoning, row.total),
            (7668, 142, 192, 0, 42, 7810)
        );
        assert_eq!(row.duration_ms, Some(3354.0));
        assert_eq!(row.reported_cost, Some(0.0));
        let ids_before = dump_ids(&derived);

        // 幂等：指纹（水位 + 行数）未变 → 不重导，内容与 id 完全不动
        let second = import_from_source(&src, &mut derived, 2_000).expect("二次导入失败");
        assert_eq!(second.reimported_sessions, 0, "指纹未变不应重导");
        assert_eq!(second.records_written, 0);
        assert_eq!(dump_rows(&derived), rows);
        assert_eq!(dump_ids(&derived), ids_before);

        // 会话追加消息（time_updated 更大）→ 只重导该会话，行数 +1
        insert_v2_message(
            &src,
            "msg_2",
            "ses_a",
            "assistant",
            8,
            1791832000000,
            1791832002091,
            REAL_PAID_ASSISTANT_A,
        );
        let third = import_from_source(&src, &mut derived, 3_000).expect("追加导入失败");
        assert_eq!(third.reimported_sessions, 1);
        assert_eq!(third.records_written, 2);
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 2);
        // 同一会话里两个模型的用量各自归属（这就是不用会话聚合的原因）
        assert_eq!(rows[0].model_id, "step-5-preview-free");
        assert_eq!(rows[0].provider_id, "opencode");
        assert_eq!(rows[1].model_id, "deepseek-v4.1-flash");
        assert_eq!(rows[1].provider_id, "opencode-go");
        assert_eq!(rows[1].reported_cost, Some(0.000571398));
        // 目录来自 session_v2.directory（→ project_key 由 normalize_cwd 归一）
        let (cwd, project_key): (Option<String>, Option<String>) = derived
            .query_row(
                "SELECT cwd, project_key FROM model_usage WHERE dedupe_key = 'ses_a|msg_1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读取 cwd 失败");
        assert_eq!(cwd.as_deref(), Some("C:/work/demo"));
        assert_eq!(project_key, crate::projects::normalize_cwd("C:/work/demo"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// v2 修订：消息被编辑 → 同会话重导且值被覆盖（不新增重复行）；
    /// 消息被删除 → 行数指纹变化 → 派生库不留幽灵行。
    #[test]
    fn v2_reimport_handles_edits_and_removals() {
        let dir = temp_dir("v2-revise");
        let src_path = dir.join("opencode.db");
        create_v2_source(&src_path);
        let src = Connection::open(&src_path).expect("打开源库失败");
        insert_v2_message(
            &src,
            "msg_1",
            "ses_a",
            "assistant",
            6,
            1791831838192,
            1791831841547,
            REAL_FREE_ASSISTANT,
        );
        insert_v2_message(
            &src,
            "msg_2",
            "ses_a",
            "assistant",
            8,
            1791832000000,
            1791832002091,
            REAL_PAID_ASSISTANT_A,
        );
        let mut derived = open_derived(&dir);
        import_from_source(&src, &mut derived, 1_000).expect("首次导入失败");
        assert_eq!(dump_rows(&derived).len(), 2);

        // 编辑 msg_1 的用量（并推进 time_updated）→ 整会话重导，行数不变、值更新
        src.execute(
            "UPDATE session_message SET data = ?1, time_updated = ?2 WHERE id = 'msg_1'",
            rusqlite::params![REAL_PAID_ASSISTANT_B, 1791832200000i64],
        )
        .expect("更新源消息失败");
        let outcome = import_from_source(&src, &mut derived, 2_000).expect("修订导入失败");
        assert_eq!(outcome.reimported_sessions, 1, "水位变新应触发重导");
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 2, "重导不得产生重复行");
        let edited = rows
            .iter()
            .find(|r| r.dedupe_key == "ses_a|msg_1")
            .expect("编辑后的行应仍在");
        assert_eq!(
            (
                edited.input,
                edited.output,
                edited.cache_read,
                edited.cache_write,
                edited.reasoning,
                edited.total
            ),
            (7715, 103, 1536, 0, 12, 7818)
        );

        // 删除 msg_2：源库 assistant 行数 2 → 1，指纹变化必须被发现
        src.execute("DELETE FROM session_message WHERE id = 'msg_2'", [])
            .expect("删除源消息失败");
        let outcome = import_from_source(&src, &mut derived, 3_000).expect("删除后导入失败");
        assert_eq!(outcome.reimported_sessions, 1);
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 1, "被删消息在派生库不得留下幽灵行");
        assert_eq!(rows[0].dedupe_key, "ses_a|msg_1");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 重导必须**保住存活行的自增 id**——这是服务端不重复计数的前提：
    /// 服务端按 `(device_id, source, local_rowid)` 去重（server/db.py），若整会话
    /// 重导换了 id，同一笔用量会在服务端落成一批新行、旧行仍在 → 重复计数。
    /// 同时断言：新消息拿到更大的新 id（`id > 游标` 能选出它）、按内容改写过的行
    /// 自增 id 不变但 `updated_at` 推进（修订补传通道能选出它）、幽灵行差集清理
    /// 只作用于本会话（不得误删别的会话）。
    #[test]
    fn v2_reimport_keeps_rowids_stable() {
        let dir = temp_dir("v2-stable-id");
        let src_path = dir.join("opencode.db");
        create_v2_source(&src_path);
        let src = Connection::open(&src_path).expect("打开源库失败");
        insert_v2_message(
            &src,
            "msg_1",
            "ses_a",
            "assistant",
            6,
            1791831838192,
            1791831841547,
            REAL_FREE_ASSISTANT,
        );
        // 第二个会话：验证「重导 ses_a」时差集清理不会误删其他会话的行
        insert_v2_message(
            &src,
            "msg_b1",
            "ses_b",
            "assistant",
            2,
            1791831000000,
            1791831001000,
            REAL_PAID_ASSISTANT_A,
        );
        let mut derived = open_derived(&dir);
        import_from_source(&src, &mut derived, 1_000).expect("首次导入失败");

        let id_of = |conn: &Connection, key: &str| -> i64 {
            conn.query_row(
                "SELECT id FROM model_usage WHERE dedupe_key = ?1",
                [key],
                |row| row.get(0),
            )
            .expect("读取自增 id 失败")
        };
        let updated_of = |conn: &Connection, key: &str| -> i64 {
            conn.query_row(
                "SELECT updated_at FROM model_usage WHERE dedupe_key = ?1",
                [key],
                |row| row.get(0),
            )
            .expect("读取 updated_at 失败")
        };
        let id_1 = id_of(&derived, "ses_a|msg_1");
        let id_b1 = id_of(&derived, "ses_b|msg_b1");

        // 追加消息 → 整会话重导：老行的 id 必须原地不动，新行才是新 id
        insert_v2_message(
            &src,
            "msg_2",
            "ses_a",
            "assistant",
            8,
            1791832000000,
            1791832002091,
            REAL_PAID_ASSISTANT_A,
        );
        import_from_source(&src, &mut derived, 2_000).expect("追加导入失败");
        assert_eq!(dump_rows(&derived).len(), 3);
        assert_eq!(
            id_of(&derived, "ses_a|msg_1"),
            id_1,
            "追加消息不得让已有行换 id（否则服务端会多出一份重复用量）"
        );
        assert_eq!(
            id_of(&derived, "ses_b|msg_b1"),
            id_b1,
            "重导 ses_a 时差集清理必须限定在本会话，不得动 ses_b 的行"
        );
        let id_2 = id_of(&derived, "ses_a|msg_2");
        assert!(id_2 > id_1, "新消息应拿到新的、更大的 id（id 游标才能选出它）");

        // 编辑老消息（总量变大）→ 仍是原地覆盖：id 不变、updated_at 推进
        src.execute(
            "UPDATE session_message SET data = ?1, time_updated = ?2 WHERE id = 'msg_1'",
            rusqlite::params![REAL_PAID_ASSISTANT_B, 1791832200000i64],
        )
        .expect("更新源消息失败");
        import_from_source(&src, &mut derived, 3_000).expect("修订导入失败");
        assert_eq!(id_of(&derived, "ses_a|msg_1"), id_1, "修订覆盖不得换 id");
        assert_eq!(id_of(&derived, "ses_a|msg_2"), id_2);
        assert_eq!(updated_of(&derived, "ses_a|msg_1"), 3_000, "被覆盖行应打上修订时间");
        assert_eq!(
            updated_of(&derived, "ses_a|msg_2"),
            2_000,
            "未变行不得被打上修订标记（否则每轮同步都要重传整会话）"
        );

        // 修订通道：按 updated_at 选出的只有被改写的 msg_1（id 游标选不出它）
        let revised = query_revised_since_at(&derived, 2_500, 0, 10).expect("修订查询失败");
        assert_eq!(revised.len(), 1, "只应选出本次重导改写过的行");
        assert_eq!(revised[0].local_rowid, id_1);
        assert_eq!(revised[0].computed_total_tokens, 7818);
        // 明细通道（id 游标）：ses_a 里 id > id_1 的只有新消息 msg_2，
        // 老行不会被重复上传（ses_b 的行是别的会话，与 id 游标无关）
        let since = query_since_at(&derived, id_1, 10).expect("明细查询失败");
        let a_ids: Vec<i64> = since
            .iter()
            .filter(|r| r.session_id.as_deref() == Some("ses_a"))
            .map(|r| r.local_rowid)
            .collect();
        assert_eq!(a_ids, vec![id_2], "ses_a 的老行不该被 id 游标再选一次");
        assert!(
            since.iter().any(|r| r.local_rowid == id_b1),
            "ses_b 的行不受 ses_a 重导影响"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 「老消息被编辑」也必须触发重导：编辑第一条消息只推进它自己的
    /// `time_updated`（MAX 与行数都不变），只有 Σ 会变——这正是三元指纹里
    /// Σ 分量存在的理由（旧版只看 MAX + 行数会漏掉这类修订）。
    #[test]
    fn v2_older_message_edit_is_detected_by_revision_sum() {
        let dir = temp_dir("v2-oldedit");
        let src_path = dir.join("opencode.db");
        create_v2_source(&src_path);
        let src = Connection::open(&src_path).expect("打开源库失败");
        let old_data = r#"{"time":{"created":2000,"completed":2500},"cost":0.1,"model":{"id":"m-old","providerID":"opencode"},"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}}}"#;
        insert_v2_message(&src, "msg_old", "ses_a", "assistant", 2, 2000, 2000, old_data);
        insert_v2_message(
            &src,
            "msg_new",
            "ses_a",
            "assistant",
            6,
            1791832000000,
            1791832002091,
            REAL_PAID_ASSISTANT_A,
        );
        let mut derived = open_derived(&dir);
        import_from_source(&src, &mut derived, 1_000).expect("首次导入失败");
        assert_eq!(dump_rows(&derived).len(), 2);

        // 只编辑老消息并推进它自己的 time_updated（MAX 仍是 msg_new 的时刻）
        src.execute(
            "UPDATE session_message SET data = ?1, time_updated = 2500 WHERE id = 'msg_old'",
            [REAL_PAID_ASSISTANT_B],
        )
        .expect("编辑老消息失败");
        let outcome = import_from_source(&src, &mut derived, 2_000).expect("修订导入失败");
        assert_eq!(
            outcome.reimported_sessions, 1,
            "MAX 未变但 Σ 变了，必须重导（否则老消息的修订永远进不来）"
        );
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 2, "重导不得产生重复行");
        let edited = rows
            .iter()
            .find(|r| r.dedupe_key == "ses_a|msg_old")
            .expect("老消息行应仍在");
        assert_eq!(edited.model_id, "deepseek-v4.1-flash");
        assert_eq!(edited.input, 7715);
        assert_eq!(edited.total, 7818);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// v1 形态：`message` 表（无 type 列、角色在 `$.role`）+ 平铺 model/provider
    /// + 行级 `$.path.cwd` 兜底 + `session` 表缺 projectID 列也不炸。
    #[test]
    fn v1_schema_is_detected_and_imported() {
        let dir = temp_dir("v1");
        let src_path = dir.join("opencode.db");
        create_v1_source(&src_path);
        let src = Connection::open(&src_path).expect("打开源库失败");
        src.execute(
            "INSERT INTO message (id, sessionID, role, time_created, time_updated, data)
             VALUES ('msg_v1', 'ses_v1', 'assistant', 1700000000000, 1700000005000, ?1)",
            [V1_ASSISTANT],
        )
        .expect("插入 v1 消息失败");
        // 干扰行：$$.role = 'user' 不产生用量行
        src.execute(
            "INSERT INTO message (id, sessionID, role, time_created, time_updated, data)
             VALUES ('msg_v1_user', 'ses_v1', 'user', 1700000000000, 1700000000001,
                     '{\"role\":\"user\",\"sessionID\":\"ses_v1\"}')",
            [],
        )
        .expect("插入 v1 user 行失败");

        assert_eq!(detect_shape(&src), Some(SourceShape::V1));
        assert!(has_local_data_at(&src_path));

        let mut derived = open_derived(&dir);
        let outcome = import_from_source(&src, &mut derived, 1_000).expect("v1 导入失败");
        assert_eq!(outcome.shape, "v1");
        assert_eq!(outcome.source_sessions, 1);
        assert_eq!(outcome.records_written, 1);
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.session_id, "ses_v1");
        assert_eq!(row.model_id, "claude-sonnet-4");
        assert_eq!(row.provider_id, "anthropic");
        // input=1000+300、output=200+50、cache_read=300、cache_write=100、reasoning=50
        assert_eq!(
            (row.input, row.output, row.cache_read, row.cache_write, row.reasoning, row.total),
            (1300, 250, 300, 100, 50, 1550)
        );
        assert_eq!(row.duration_ms, Some(5000.0));
        assert_eq!(row.reported_cost, Some(0.0123));
        // session 表 directory 为 NULL → 回退消息自带的 $.path.cwd
        let (cwd, project_key): (Option<String>, Option<String>) = derived
            .query_row(
                "SELECT cwd, project_key FROM model_usage WHERE dedupe_key = 'ses_v1|msg_v1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("读取 cwd 失败");
        assert_eq!(cwd.as_deref(), Some("C:/work/legacy"));
        assert_eq!(project_key, crate::projects::normalize_cwd("C:/work/legacy"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// presence 判定：库存在**且**含 v1/v2 消息表才为真；空库/无关表为假。
    #[test]
    fn has_local_data_requires_a_message_table() {
        let dir = temp_dir("presence");
        // 缺失的库
        assert!(!has_local_data_at(&dir.join("missing.db")));
        // 空库（无表）
        let empty_path = dir.join("empty.db");
        Connection::open(&empty_path).expect("建空库失败");
        assert!(!has_local_data_at(&empty_path));
        // 只有无关表的库
        let other_path = dir.join("other.db");
        let other = Connection::open(&other_path).expect("建库失败");
        other
            .execute_batch("CREATE TABLE account (id TEXT PRIMARY KEY);")
            .expect("建无关表失败");
        assert!(!has_local_data_at(&other_path));
        // v2 / v1 都算有数据
        let v2_path = dir.join("v2.db");
        create_v2_source(&v2_path);
        assert!(has_local_data_at(&v2_path));
        let v1_path = dir.join("v1.db");
        create_v1_source(&v1_path);
        assert!(has_local_data_at(&v1_path));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 空数据不 panic：无表源库、有表但零消息、空派生库上的全套查询都给零值。
    #[test]
    fn empty_source_and_unknown_schema_do_not_panic() {
        let dir = temp_dir("empty");
        // (1) 既非 v1 也非 v2 的空库
        let src_path = dir.join("opencode.db");
        Connection::open(&src_path).expect("建空库失败");
        let src = Connection::open(&src_path).expect("打开源库失败");
        assert_eq!(detect_shape(&src), None);
        assert_eq!(source_debug(&src), ("none".to_string(), 0, 0));
        let mut derived = open_derived(&dir);
        let outcome = import_from_source(&src, &mut derived, 1_000).expect("空库导入不该报错");
        assert_eq!(outcome, ImportOutcome { shape: "none".to_string(), ..Default::default() });

        // (2) 有 v2 表但零消息
        let empty_v2_path = dir.join("empty_v2.db");
        create_v2_source(&empty_v2_path);
        let empty_v2 = Connection::open(&empty_v2_path).expect("打开源库失败");
        // 造一个只有表没有消息的源库：删掉建库时写入的会话与项目
        empty_v2
            .execute_batch("DELETE FROM session_v2; DELETE FROM project;")
            .expect("清空 v2 源库失败");
        let outcome = import_from_source(&empty_v2, &mut derived, 2_000).expect("零消息导入失败");
        assert_eq!(outcome.shape, "v2");
        assert_eq!(outcome.source_sessions, 0);

        // (3) 空派生库上全套查询：零值、不 panic
        let stats = query_stats_at(&derived, 0, i64::MAX).expect("空库统计失败");
        assert_eq!(stats.overall.requests, 0);
        assert_eq!(stats.overall.total_tokens, 0);
        assert!(stats.earliest_ms.is_none());
        assert!(stats.latest_ms.is_none());
        assert!(stats.by_model.is_empty());
        assert!(stats.current_model.is_none());
        assert!(list_models_at(&derived).expect("空库模型列表失败").is_empty());
        assert_eq!(max_rowid_at(&derived).expect("空库 max_rowid 失败"), 0);
        assert!(query_since_at(&derived, 0, 10).expect("空库明细失败").is_empty());
        assert!(query_revised_since_at(&derived, 0, 0, 10)
            .expect("空库修订失败")
            .is_empty());
        let trend = query_trend_at(&derived, 0, 7_200_000, "hour").expect("空库趋势失败");
        assert_eq!(trend.len(), 2);
        assert!(trend.iter().all(|b| b.total_tokens == 0 && b.requests == 0));
        let periods =
            query_period_buckets_at(&derived, &[(0, i64::MAX)]).expect("空库周期聚合失败");
        assert_eq!(periods[0].total_tokens, 0);
        assert_eq!(periods[0].requests, 0);
        let window =
            query_window_sessions_at(&derived, 0, 10).expect("空库会话窗口失败");
        assert_eq!(window.total_active, 0);
        assert!(window.sessions.is_empty());
        assert!(query_project_model_rows_at(&derived, 0, i64::MAX)
            .expect("空库项目聚合失败")
            .is_empty());
        assert!(query_project_session_counts_at(&derived, 0, i64::MAX)
            .expect("空库项目会话统计失败")
            .is_empty());
        assert!(query_project_display_paths_at(&derived)
            .expect("空库项目路径失败")
            .is_empty());
        let (total, sessions) = query_project_sessions_at(&derived, crate::projects::UNKNOWN_PROJECT, 0, i64::MAX, 0, 10)
            .expect("空库项目会话失败");
        assert_eq!((total, sessions.len()), (0, 0));
        let cost = query_reported_cost_at(&derived, 0, i64::MAX).expect("空库成本失败");
        assert_eq!(cost, ReportedCostSummary { total_usd: 0.0, priced_rows: 0, total_rows: 0 });
        assert_eq!(derived_debug(&derived), (0, None, 0.0, 0));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 查询聚合：overall 与 by_model 自洽、趋势求和等于总量、明细/修订/
    /// 会话窗口/项目维度四通道都能用；自算成本按范围累计。
    #[test]
    fn queries_aggregate_imported_rows() {
        let dir = temp_dir("queries");
        let src_path = dir.join("opencode.db");
        create_v2_source(&src_path);
        let src = Connection::open(&src_path).expect("打开源库失败");
        // ses_a 两条（免费 + 付费，同会话不同模型）
        insert_v2_message(
            &src,
            "msg_1",
            "ses_a",
            "assistant",
            6,
            1791831838192,
            1791831841547,
            REAL_FREE_ASSISTANT,
        );
        insert_v2_message(
            &src,
            "msg_2",
            "ses_a",
            "assistant",
            8,
            1791832000000,
            1791832002091,
            REAL_PAID_ASSISTANT_A,
        );
        // ses_b：另一个目录（项目维度要能分出两个项目键）
        src.execute(
            "INSERT INTO session_v2 (id, project_id, directory, title, time_created, time_updated)
             VALUES ('ses_b', 'p2', 'C:/work/other', 'b', 300, 400)",
            [],
        )
        .expect("插入第二个会话失败");
        insert_v2_message(
            &src,
            "msg_3",
            "ses_b",
            "assistant",
            2,
            1791832100000,
            1791832102091,
            REAL_PAID_ASSISTANT_B,
        );
        let mut derived = open_derived(&dir);
        import_from_source(&src, &mut derived, 1_000).expect("导入失败");

        // 统计：overall 与 by_model 自洽、input 已含 cache_read
        let stats = query_stats_at(&derived, 0, i64::MAX).expect("统计失败");
        assert_eq!(stats.overall.requests, 3);
        let model_total: i64 = stats.by_model.iter().map(|m| m.total_tokens).sum();
        assert_eq!(stats.overall.total_tokens, model_total);
        assert_eq!(stats.overall.total_tokens, 7810 + 6380 + 7818);
        assert!(stats.overall.input_tokens >= stats.overall.cache_read_tokens);
        assert_eq!(
            stats.overall.reasoning_tokens,
            42 + 0 + 12,
            "reasoning 原样落库（output 的子集，不重复计入总量）"
        );
        assert_eq!(stats.by_model.len(), 2, "两个 provider 各一个模型分组");
        assert_eq!(stats.current_model.as_ref().map(|m| m.model_id.as_str()), Some("deepseek-v4.1-flash"));
        // 速度：本来源只有请求总耗时 → 请求平均口径
        if let Some(quality) = stats.overall.speed.speed_quality {
            assert_eq!(quality, crate::token_speed::SpeedQuality::RequestAverage);
        }

        // 趋势：真实范围求和 == 总量（起点 0 的大范围只验证桶数上限）
        assert!(query_trend_at(&derived, 0, i64::MAX, "day")
            .expect("趋势失败")
            .len()
            <= MAX_TREND_BUCKETS);
        let from = stats.earliest_ms.expect("应有最早时刻");
        let to = stats.latest_ms.expect("应有最晚时刻") + 1;
        let trend = query_trend_at(&derived, from, to, "day").expect("趋势失败");
        let trend_total: i64 = trend.iter().map(|b| b.total_tokens).sum();
        assert_eq!(trend_total, stats.overall.total_tokens);

        // 模型清单（配价页数据源）
        let models = list_models_at(&derived).expect("模型列表失败");
        assert!(models.iter().any(|m| m.model_id == "step-5-preview-free"
            && m.provider_id == "opencode"));
        assert!(models
            .iter()
            .any(|m| m.model_id == "deepseek-v4.1-flash" && m.provider_id == "opencode-go"));

        // 周期聚合 / 明细 / 修订通道
        let periods = query_period_buckets_at(&derived, &[(0, i64::MAX), (0, 1791831900000)])
            .expect("周期聚合失败");
        assert_eq!(periods[0].total_tokens, stats.overall.total_tokens);
        assert_eq!(periods[0].requests, 3);
        assert!(periods[1].total_tokens < periods[0].total_tokens);
        let rows = query_since_at(&derived, 0, 100).expect("明细失败");
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.source == "opencode"));
        assert_eq!(rows[0].session_id.as_deref(), Some("ses_a"));
        assert!(rows.iter().all(|r| r.project_key.is_some()));
        let revised = query_revised_since_at(&derived, 0, 0, 100).expect("修订失败");
        assert_eq!(revised.len(), 3);
        // 分页游标语义：after_id 之后不再返回
        let last_id = revised.last().map(|r| r.local_rowid).unwrap_or(0);
        assert!(query_revised_since_at(&derived, 0, last_id, 100)
            .expect("修订分页失败")
            .is_empty());

        // 会话窗口（HUD）：两个会话、按最后活动倒序、无生成中/无 TTFT
        let window = query_window_sessions_at(&derived, 0, 10).expect("会话窗口失败");
        assert_eq!(window.total_active, 2);
        assert_eq!(window.sessions.len(), 2);
        assert!(window.sessions.iter().all(|s| !s.generating
            && s.ttft_ms.is_none()
            && s.gen_out_tokens == 0
            && s.gen_ms == 0.0));
        assert!(window.sessions.iter().all(|s| s.in_tokens > 0 && s.out_tokens > 0));

        // 模型速度区（悬浮窗底部）：按「来源 × 模型」聚合，请求平均口径
        //（本库有 TTFT 列但恒 NULL、duration_ms 是请求总耗时）
        let speeds = crate::session_hud::collect_derived_model_speeds(
            &derived,
            0,
            3,
            crate::local_agents::OPENCODE,
            crate::token_speed::SpeedQuality::RequestAverage,
        )
        .expect("模型速度聚合失败");
        assert_eq!(speeds.len(), 2, "两个模型各一行：{speeds:?}");
        assert_eq!(speeds[0].model, "deepseek-v4.1-flash", "按最近使用降序");
        assert_eq!(speeds[0].samples, 2, "msg_2 + msg_3 同模型");
        assert_eq!(speeds[1].model, "step-5-preview-free");
        assert!(speeds.iter().all(|r| r.agent == "opencode"
            && r.quality == crate::token_speed::SpeedQuality::RequestAverage
            && r.samples > 0
            && r.tps > 0.0
            && r.avg_tps > 0.0));

        // 项目维度：两个项目键 + 会话数
        let projects = query_project_model_rows_at(&derived, 0, i64::MAX).expect("项目聚合失败");
        let keys: Vec<&str> = projects.iter().map(|p| p.project_key.as_str()).collect();
        assert_eq!(keys.len(), 3, "两个项目 × 模型组合（一个项目两个模型）");
        assert!(keys.contains(&crate::projects::normalize_cwd("C:/work/demo").unwrap().as_str()));
        assert!(keys.contains(&crate::projects::normalize_cwd("C:/work/other").unwrap().as_str()));
        let counts = query_project_session_counts_at(&derived, 0, i64::MAX).expect("项目会话数失败");
        assert_eq!(counts.iter().map(|(_, n)| *n).sum::<i64>(), 2);
        let display = query_project_display_paths_at(&derived).expect("项目路径失败");
        assert_eq!(display.len(), 2);
        let demo_key = crate::projects::normalize_cwd("C:/work/demo").unwrap();
        let (total, session_rows) =
            query_project_sessions_at(&derived, &demo_key, 0, i64::MAX, 0, 10)
                .expect("项目会话明细失败");
        assert_eq!(total, 1, "demo 项目只有 ses_a 一个会话");
        assert_eq!(session_rows.len(), 2, "该会话里有两条模型行");

        // 自算成本：只统计有 cost 的行（本样本三行都有）
        let cost = query_reported_cost_at(&derived, 0, i64::MAX).expect("成本查询失败");
        assert_eq!(cost.total_rows, 3);
        assert_eq!(cost.priced_rows, 3);
        assert!((cost.total_usd - (0.0 + 0.000571398 + 0.000993258)).abs() < 1e-12);
        let (records, latest, total_cost, cost_rows) = derived_debug(&derived);
        assert_eq!((records, cost_rows), (3, 3));
        assert_eq!(latest, Some(1791832100000));
        assert!((total_cost - cost.total_usd).abs() < 1e-12);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 冒烟：对本机**真实** `~/.local/share/opencode/opencode.db` 做一次增量导入与
    /// 全范围查询。源库只读打开、派生库落在临时目录（**不碰真实 ~/.agenthud**），
    /// 故可安全地在本机反复跑。本机未装 OpenCode 时早退（他人机器/CI 上不算失败）。
    /// 断言重点：真实数据上的幂等性、overall 与 by_model 自洽、归一化落地
    /// （input ≥ cache_read）、自算成本覆盖全部行。
    #[test]
    fn real_home_import_and_query_smoke() {
        let path = crate::opencodego::db_path();
        if !path.exists() {
            eprintln!("[test] 本机无 OpenCode 库，跳过");
            return;
        }
        let Ok(src) = open_source_readonly(&path) else {
            eprintln!("[test] 打开真实源库失败，跳过");
            return;
        };
        let Some(shape) = detect_shape(&src) else {
            eprintln!("[test] 真实源库无 v1/v2 消息表，跳过");
            return;
        };
        let dir = temp_dir("real-smoke");
        let mut derived = open_derived(&dir);
        let outcome = import_from_source(&src, &mut derived, crate::provider_quota::now_ms())
            .expect("真实库首次导入失败");
        assert_eq!(outcome.shape, shape.label());
        let first = dump_rows(&derived);
        let ids = dump_ids(&derived);
        // 幂等：同一份源数据再导一次，内容与自增 id 都不动
        import_from_source(&src, &mut derived, crate::provider_quota::now_ms())
            .expect("真实库二次导入失败");
        assert_eq!(dump_rows(&derived), first, "真实库重复导入必须幂等");
        assert_eq!(dump_ids(&derived), ids);
        if first.is_empty() {
            eprintln!("[test] 真实库无 assistant 用量行，跳过后续断言");
            std::fs::remove_dir_all(&dir).ok();
            return;
        }

        let stats = query_stats_at(&derived, 0, i64::MAX).expect("真实库统计失败");
        assert_eq!(stats.overall.requests, first.len() as i64);
        let by_model_total: i64 = stats.by_model.iter().map(|m| m.total_tokens).sum();
        assert_eq!(stats.overall.total_tokens, by_model_total, "overall 与 by_model 不一致");
        assert!(
            stats.overall.input_tokens >= stats.overall.cache_read_tokens,
            "input_tokens({}) 应 >= cache_read_tokens({})（归一化失效？）",
            stats.overall.input_tokens,
            stats.overall.cache_read_tokens
        );
        // 时间范围与趋势求和自洽
        let from = stats.earliest_ms.unwrap_or(0);
        let to = stats.latest_ms.unwrap_or(0) + 1;
        let trend = query_trend_at(&derived, from, to, "day").expect("真实库趋势失败");
        assert_eq!(
            trend.iter().map(|b| b.total_tokens).sum::<i64>(),
            stats.overall.total_tokens
        );
        // 自算成本列覆盖全部行（真实库每行 assistant 都带 cost）
        let cost = query_reported_cost_at(&derived, 0, i64::MAX).expect("真实库成本失败");
        assert_eq!(cost.total_rows, first.len() as i64);
        // 项目维度：目录非空的会话应能给出项目键
        if let Some(first_project) = query_project_model_rows_at(&derived, 0, i64::MAX)
            .expect("真实库项目聚合失败")
            .first()
        {
            assert!(!first_project.project_key.is_empty());
        }
        eprintln!(
            "[test] 真实库 {}：{} 个会话指纹、{} 行用量、{} token、自算成本 ${:.6}",
            shape.label(),
            outcome.source_sessions,
            first.len(),
            stats.overall.total_tokens,
            cost.total_usd
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 坏行与无时间戳行只跳过自己：源库里有坏 JSON / 全零 token / 无时间的
    /// assistant 行时，同会话其他行照常入库。
    #[test]
    fn bad_rows_are_skipped_without_blocking_the_session() {
        let dir = temp_dir("badrows");
        let src_path = dir.join("opencode.db");
        create_v2_source(&src_path);
        let src = Connection::open(&src_path).expect("打开源库失败");
        insert_v2_message(&src, "msg_ok", "ses_a", "assistant", 6, 1000, 2000, REAL_PAID_ASSISTANT_A);
        // 坏 JSON
        insert_v2_message(&src, "msg_bad", "ses_a", "assistant", 7, 1100, 2100, "not json");
        // 全零 token 的占位行
        insert_v2_message(
            &src,
            "msg_zero",
            "ses_a",
            "assistant",
            8,
            1200,
            2200,
            r#"{"time":{"created":1200,"completed":1300},"cost":0,"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}}}"#,
        );
        // 无任何时间（data 与列都空）
        insert_v2_message(
            &src,
            "msg_notime",
            "ses_a",
            "assistant",
            9,
            // time_created 列留 NULL：用 rusqlite 直接写 NULL
            0,
            0,
            r#"{"cost":0,"tokens":{"input":10,"output":10}}"#,
        );
        src.execute("UPDATE session_message SET time_created = NULL, time_updated = NULL WHERE id = 'msg_notime'", [])
            .expect("置空时间失败");
        let mut derived = open_derived(&dir);
        let outcome = import_from_source(&src, &mut derived, 1_000).expect("导入失败");
        assert_eq!(outcome.records_written, 1, "只应写入那条完好的行");
        let rows = dump_rows(&derived);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].dedupe_key, "ses_a|msg_ok");
        // 水位仍以源库 assistant 行数为准（4 行）→ 后续无变化时不重导
        let again = import_from_source(&src, &mut derived, 2_000).expect("二次导入失败");
        assert_eq!(again.reimported_sessions, 0);

        std::fs::remove_dir_all(&dir).ok();
    }
}
