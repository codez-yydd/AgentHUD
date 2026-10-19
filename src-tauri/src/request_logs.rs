//! 主窗口「请求日志」页的取数：把 7 个本地来源的请求级明细表（`model_usage`，
//! 每行 = 一次模型请求）按时间倒序合并成一份可分页的列表。
//!
//! ## 来源与打开方式
//!
//! - `zcode`：ZCode 主库（`db::db_path()`，**只读**打开，与 db.rs 同款 flags）；
//! - `codex` / `claude` / `kimi` / `dsh` / `opencode` / `minimaxcode`：各自的
//!   导入库 `~/.agenthud/<name>.sqlite`（与前端的「来源」概念对齐，见
//!   src/dashboard/compare/model.ts 的 remoteSource 映射）。
//!
//! 本模块对导入库只做 SELECT：直接 `Connection::open` 打开，**刻意不调用各模块的
//! `open_*_db()`**——那些函数会建表 / 补列 / 触发迁移，而这里只是读一眼；
//! 库文件不存在（未安装 / 未用过）按 0 条处理，ZCode 主库路径解析失败同样按
//! 0 条（未安装 ZCode 时页面必须照常显示空态）。
//!
//! ## 列差异与降级
//!
//! 各库 `model_usage` 列基本统一（started_at / model_id / provider_id /
//! input_tokens / output_tokens / cache_read_input_tokens /
//! cache_creation_input_tokens / reasoning_tokens / computed_total_tokens /
//! session_id），差异一律用 `db::has_column` 逐个降级、缺失列 SELECT NULL，
//! 绝不改库（老 zcode 主库无 session_id、老 claude 库无 cwd / project_key）：
//! - `duration_ms`：zcode / claude / kimi / dsh / opencode / minimaxcode 有
//!   （>0 才给，缺失与 0 都按 None）；
//! - `time_to_first_token_ms`：zcode 与 dsh 是真实数据（opencode / minimaxcode
//!   该列恒 NULL），其余来源无该列，统一按 None；
//! - `first_token_at` / `completed_at`（行级速度原料）：zcode 主库的事件时刻，
//!   导入库无该列按 NULL；
//! - `project`：codex / kimi 从各自 `session_meta` 表按会话取（无表 / 无该会话
//!   → None），claude / dsh / opencode / minimaxcode 取行内 cwd / project_key；
//!   zcode 主库没有项目列（归属在 zcode_sessions 派生库），本页不做跨库解析。
//!
//! ## 行级速度口径
//!
//! 单笔速度（`speed_tps` / `speed_quality`）的**唯一真源 = `token_speed`**：
//! `row_to_log` 逐行调 `token_speed::request_speed_at`，思考感知窗口（思考型
//! 请求 output 已含思考 token，生成窗口前移为 completed − started 全程，
//! 防止思考时长漏出分母导致速率虚高）、[200ms, 1h] 样本窗口边界与时序校验
//! 全在该行级判定内，前端 `speedOf` 不再自行判定生成窗口。`observed_at_ms`
//! 传 0：这些是已落库的历史行，不做未来时刻拒绝（与既有日志页语义一致）。
//! 判定失败（零输出 / 时序倒置 / 窗口越界 / 无时间戳）两字段为 None，前端回退。
//!
//! ## 分页与归并
//!
//! 每个来源按 `started_at DESC` 取「前 (page+1)×page_size 行」（**单源不带
//! OFFSET**：全局页码对应的 offset 落在归并之后，单源先 OFFSET 会把归并结果
//! 切歪），各源结果做 k 路归并（同毫秒按来源名稳定序）后切片出当前页；
//! total = 各源同条件 COUNT(*) 之和。单源失败（表不存在、库损坏等）只记
//! eprintln! 并跳过该源，把来源名放进 `failed_sources` 交前端弱提示，
//! 不让整页失败。
//!
//! ## 汇总指标
//!
//! `summary`：当前筛选条件（agent + model + 时间范围）下**全范围**聚合
//! （不是当前页），每源在分页查询外另发一条聚合 SQL（同 WHERE）：请求数 /
//! 各项 token 和 + 按 model_id 分组的成本折算输入（复用 Billable/cost_for，
//! 无价格模型贡献 0）。缓存命中率分子统一为 Σcache_read，分母分两套口径：
//! `included` 源（zcode / codex / dsh / opencode / minimaxcode，input 已含
//! 缓存读）= Σinput；`separate` 源（claude / kimi，缓存读写单独成列、不含在
//! input 内）= Σinput + Σcache_read + Σcache_write。分母为 0 → None。
//!
//! 耗时只认来源库里的**真实** `duration_ms`（> 0 才给）；没有该列或值为
//! NULL / 0 的来源（如 codex 会话文件本就不含耗时）按无耗时下发（None），
//! 前端显示灰化「—」，不再做任何估算回退。

use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// 全部来源（顺序即默认归并的稳定次序，同毫秒按此升序）
const SOURCES: [&str; 7] = [
    "zcode",
    "codex",
    "claude",
    "kimi",
    "dsh",
    "opencode",
    "minimaxcode",
];

/// 每页行数上下限与缺省（前端固定传 20；0 / 负数按缺省，超出按边界夹取）
const DEFAULT_PAGE_SIZE: i64 = 20;
const MIN_PAGE_SIZE: i64 = 10;
const MAX_PAGE_SIZE: i64 = 100;

/// 单源一次最多取多少行（深页保护：page=10000 时不该把单库整表读进内存）
const MAX_FETCH_ROWS: i64 = 5_000;

/// 模型筛选下拉的选项上限
const MAX_MODEL_OPTIONS: usize = 100;

/// 请求日志的一行（一次模型请求）。字段为 snake_case，与既有 usage 系契约一致。
#[derive(Debug, Clone, Serialize)]
pub struct RequestLogRow {
    /// 来源名："zcode" | "codex" | "claude" | "kimi" | "dsh" | "opencode" | "minimaxcode"
    pub source: String,
    /// 请求开始时间（毫秒时间戳）
    pub started_at: i64,
    pub model_id: String,
    pub provider_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// 缓存读 token（行输入侧口径，input_tokens 已含本项）
    pub cache_read_tokens: i64,
    /// 缓存写 token
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    /// 后端 computed_total_tokens 口径的总量
    pub total_tokens: i64,
    /// 按本机价格表逐行折算的花费（无价格模型为 0）
    pub cost_usd: f64,
    /// 同上 × 当前汇率
    pub cost_cny: f64,
    /// 会话 id（老库无该列 → None）
    pub session_id: Option<String>,
    /// 项目（能低成本拿到的来源才给，拿不到 None）
    pub project: Option<String>,
    /// 请求耗时（毫秒；来源库真实 duration_ms > 0 才给，无该列 / NULL / 0
    /// 一律 None——不做任何估算回退）
    pub duration_ms: Option<f64>,
    /// 首字延迟（毫秒；无该列或 <= 0 → None）
    pub ttft_ms: Option<f64>,
    /// 单笔输出速度（tok/s；唯一真源 = token_speed 行级判定，缺列 / 时序
    /// 无效 / 样本窗口越界 → None，前端自行回退）
    pub speed_tps: Option<f64>,
    /// 速度口径质量标记（serde 序列化为 "generation" | "request_average"，
    /// 与前端 SpeedQuality 契约一致）：request_average = 请求平均近似，
    /// 界面带 ≈ 前缀；无速度为 None
    pub speed_quality: Option<crate::token_speed::SpeedQuality>,
}

impl crate::Billable for RequestLogRow {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn input_tokens(&self) -> i64 {
        self.input_tokens
    }
    fn output_tokens(&self) -> i64 {
        self.output_tokens
    }
    fn cache_read_tokens(&self) -> i64 {
        self.cache_read_tokens
    }
}

/// get_request_logs 的返回：范围内总条数 + 当前页明细 + 全范围汇总 + 失败来源
#[derive(Debug, Serialize)]
pub struct RequestLogsPage {
    pub total: i64,
    pub rows: Vec<RequestLogRow>,
    /// 当前筛选条件（来源 + 模型 + 时间范围）下的**全范围**汇总（非当前页）
    pub summary: RequestLogSummary,
    /// 本次查询**打开或查询失败**被跳过的来源名（库文件不存在不算——那是
    /// 未安装 / 未用过，属于正常的 0 条）。前端在 total > 0 时据此弱提示
    /// 「数据可能不完整」（对齐总览页 partialFailures 惯例）。
    pub failed_sources: Vec<String>,
}

/// 当前筛选条件下的全范围汇总（指标卡用）。
///
/// 缓存命中率口径分源：`included` 源（zcode / codex / dsh / opencode /
/// minimaxcode）input_tokens 已含缓存读，分母 = Σinput；`separate` 源
/// （claude / kimi）缓存读写单独成列、不含在 input 内，分母 = Σinput +
/// Σcache_read + Σcache_write。分子统一为 Σcache_read。分母为 0 → None。
#[derive(Debug, Clone, Default, Serialize)]
pub struct RequestLogSummary {
    pub requests: i64,
    /// 全范围按模型分组折算后求和（无价格模型贡献 0）
    pub cost_usd: f64,
    /// 同上 × 当前汇率
    pub cost_cny: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    /// 缓存命中率（0-1）；分母为 0 → None（前端显示「—」）
    pub cache_hit_rate: Option<f64>,
}

/// 单来源聚合的原始形态（区间内全部行；成本另按 by_model 分组折算）
#[derive(Debug, Clone, Default, PartialEq)]
struct SourceTotals {
    requests: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    total_tokens: i64,
    /// 按 model_id 分组的三项 token（成本折算的最小输入；不同模型单价不同，
    /// 必须逐组乘价再求和）
    by_model: Vec<ModelTokens>,
}

/// 按模型分组的 token 三项（成本折算用；Billable 载体）
#[derive(Debug, Clone, PartialEq)]
struct ModelTokens {
    model_id: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
}

impl crate::Billable for ModelTokens {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn input_tokens(&self) -> i64 {
        self.input_tokens
    }
    fn output_tokens(&self) -> i64 {
        self.output_tokens
    }
    fn cache_read_tokens(&self) -> i64 {
        self.cache_read_tokens
    }
}

/// 模型筛选项（跨源合并同名模型后的计数）
#[derive(Debug, Serialize)]
pub struct RequestLogModelCount {
    pub model_id: String,
    pub count: i64,
}

/// 解析来源筛选：None / 空串 = 全部；未知来源报错（来源名是代码内常量，
/// 前端传错时应当立刻暴露而不是静默空列表）。
fn select_sources(agent: Option<&str>) -> Result<Vec<&'static str>, String> {
    match agent {
        None => Ok(SOURCES.to_vec()),
        Some(name) if name.is_empty() => Ok(SOURCES.to_vec()),
        Some(name) => SOURCES
            .iter()
            .copied()
            .find(|s| *s == name)
            .map(|s| vec![s])
            .ok_or_else(|| format!("未知的请求日志来源: {name}")),
    }
}

/// 打开来源库（只做 SELECT，绝不建表 / 迁移）：
/// - 库文件不存在（未安装 / 未用过）→ `Ok(None)`，按 0 条处理而不是错误；
/// - 其余打开失败按错误上报（调用方记日志并跳过该源）。
fn open_source(source: &str) -> Result<Option<Connection>, String> {
    if source == "zcode" {
        // 主库路径解析失败（未找到 ZCode 数据库）按 0 条：未安装场景
        let Ok(path) = crate::db::db_path() else {
            return Ok(None);
        };
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("打开 ZCode 主库失败: {e}"))?;
        conn.busy_timeout(std::time::Duration::from_secs(3))
            .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
        return Ok(Some(conn));
    }
    let path = source_db_path(source)?;
    if !path.exists() {
        return Ok(None);
    }
    let conn =
        Connection::open(&path).map_err(|e| format!("打开 {source} 导入库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    Ok(Some(conn))
}

/// 来源名 → 导入库路径（各模块的路径函数已 pub(crate) 化，不复制路径逻辑）。
fn source_db_path(source: &str) -> Result<PathBuf, String> {
    match source {
        "codex" => crate::codex::codex_db_path(),
        "claude" => crate::claude::claude_db_path(),
        "kimi" => crate::kimi::kimi_db_path(),
        "dsh" => crate::dsh::dsh_db_path(),
        "opencode" => crate::opencode::opencode_db_path(),
        "minimaxcode" => crate::minimaxcode::minimaxcode_db_path(),
        other => Err(format!("未知的请求日志来源: {other}")),
    }
}

/// 表是否存在（表名都是代码内常量，无注入风险）
fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// 项目取值表达式：project_key 优先、cwd 兜底（两列都缺 → None）。
/// prefix 用于 session_meta 相关子查询里的列限定（"sm."）。
fn combine_project(has_key: bool, has_cwd: bool, prefix: &str) -> Option<String> {
    match (has_key, has_cwd) {
        (true, true) => Some(format!(
            "COALESCE(NULLIF({prefix}project_key,''), NULLIF({prefix}cwd,''))"
        )),
        (true, false) => Some(format!("NULLIF({prefix}project_key,'')")),
        (false, true) => Some(format!("NULLIF({prefix}cwd,'')")),
        (false, false) => None,
    }
}

/// 行内项目列（claude / dsh / opencode / minimaxcode）
fn row_project_expr(conn: &Connection) -> Option<String> {
    combine_project(
        crate::db::has_column(conn, "model_usage", "project_key"),
        crate::db::has_column(conn, "model_usage", "cwd"),
        "",
    )
}

/// 会话元表项目列（codex / kimi：session_id → project_key / cwd）。
/// 用相关子查询而不是 JOIN：不产生行膨胀，且无该会话时自然为 NULL。
fn session_meta_project_expr(conn: &Connection) -> Option<String> {
    if !table_exists(conn, "session_meta") {
        return None;
    }
    let inner = combine_project(
        crate::db::has_column(conn, "session_meta", "project_key"),
        crate::db::has_column(conn, "session_meta", "cwd"),
        "sm.",
    )?;
    Some(format!(
        "(SELECT {inner} FROM session_meta sm WHERE sm.session_id = model_usage.session_id)"
    ))
}

/// 项目维度取值表达式（拿不到选 NULL，见模块头说明）
fn project_expr(conn: &Connection, source: &str) -> Option<String> {
    match source {
        "codex" | "kimi" => session_meta_project_expr(conn),
        "claude" | "dsh" | "opencode" | "minimaxcode" => row_project_expr(conn),
        // zcode：主库无项目列，归属在 zcode_sessions 派生库（不做跨库解析）
        _ => None,
    }
}

/// 耗时取值表达式：只听来源库的**真实** `duration_ms`（列存在且 > 0 才给，
/// 否则 NULL）。没有该列（如 codex）或值为 NULL / 0 的来源按无耗时下发，
/// 不做任何估算回退。
fn duration_expr(conn: &Connection) -> String {
    if crate::db::has_column(conn, "model_usage", "duration_ms") {
        "CASE WHEN COALESCE(duration_ms,0) > 0 THEN duration_ms END".to_string()
    } else {
        "NULL".to_string()
    }
}

/// 按库实际结构拼 SELECT 列表（列序与 `row_to_log` 的读取下标严格一致）。
/// 动态性只来自 `has_column` 探测，列名全部是代码内常量，无注入风险。
fn select_list(conn: &Connection, source: &str) -> String {
    let session = if crate::db::has_column(conn, "model_usage", "session_id") {
        "session_id"
    } else {
        "NULL"
    };
    let duration = duration_expr(conn);
    let ttft = if crate::db::has_column(conn, "model_usage", "time_to_first_token_ms") {
        "CASE WHEN time_to_first_token_ms > 0 THEN time_to_first_token_ms END"
    } else {
        "NULL"
    };
    // 行级速度原料（绝对事件时刻；缺失列按 NULL，正负 / 时序校验交给
    // token_speed 行级判定，SQL 侧不做过滤）
    let first_token = if crate::db::has_column(conn, "model_usage", "first_token_at") {
        "first_token_at"
    } else {
        "NULL"
    };
    let completed = if crate::db::has_column(conn, "model_usage", "completed_at") {
        "completed_at"
    } else {
        "NULL"
    };
    let project = project_expr(conn, source).unwrap_or_else(|| "NULL".into());
    format!(
        "started_at, COALESCE(model_id,''), COALESCE(provider_id,''), \
         COALESCE(input_tokens,0), COALESCE(output_tokens,0), \
         COALESCE(cache_read_input_tokens,0), COALESCE(cache_creation_input_tokens,0), \
         COALESCE(reasoning_tokens,0), COALESCE(computed_total_tokens,0), \
         {session}, {project}, {duration}, {ttft}, {first_token}, {completed}"
    )
}

/// 行读取（列序与 `select_list` 一致；花费由调用方按本机价格表统一折算）。
/// 行级速度的**唯一真源 = `token_speed`**：思考感知窗口（思考型请求用
/// completed − started 全程窗口）、[200ms, 1h] 样本窗口边界与时序校验都在
/// `request_speed_at` 的行级判定里，前端 speedOf 不再自行判定生成窗口。
/// `observed_at_ms` 传 0：已落库的历史行不做未来时刻拒绝（与既有日志页
/// 语义一致）；判定失败两字段为 None，前端按 null 回退。
fn row_to_log(row: &rusqlite::Row<'_>, source: &str) -> rusqlite::Result<RequestLogRow> {
    let started_at: i64 = row.get(0)?;
    let model_id: String = row.get(1)?;
    let output_tokens: i64 = row.get(4)?;
    let reasoning_tokens: i64 = row.get(7)?;
    let duration_ms: Option<f64> = row.get(11)?;
    let speed = crate::token_speed::request_speed_at(
        &crate::token_speed::RequestTiming {
            output_tokens,
            started_at: Some(started_at),
            first_token_at: row.get(13)?,
            completed_at: row.get(14)?,
            // 真实耗时只作 request_average 近似窗口的兜底（无事件时刻的
            // 来源，如 claude / kimi 导入库），绝不参与生成口径分母
            duration_ms: duration_ms
                .filter(|value| *value > 0.0)
                .map(|value| value as i64),
            request_id: None,
            // 思考感知窗口信号：零值按 None（缺列已 COALESCE 0，同款降级）；
            // 模型名原样透传（claude 系按名识别，大小写不敏感）
            reasoning_tokens: (reasoning_tokens > 0).then_some(reasoning_tokens),
            model_id: (!model_id.is_empty()).then(|| model_id.clone()),
        },
        0,
    );
    Ok(RequestLogRow {
        source: source.to_string(),
        started_at,
        model_id,
        provider_id: row.get(2)?,
        input_tokens: row.get(3)?,
        output_tokens,
        cache_read_tokens: row.get(5)?,
        cache_write_tokens: row.get(6)?,
        reasoning_tokens,
        total_tokens: row.get(8)?,
        cost_usd: 0.0,
        cost_cny: 0.0,
        session_id: row.get(9)?,
        project: row.get(10)?,
        duration_ms,
        ttft_ms: row.get(12)?,
        speed_tps: speed.as_ref().map(|snapshot| snapshot.value),
        speed_quality: speed.map(|snapshot| snapshot.quality),
    })
}

/// 时间区间 + 可选模型筛选的 WHERE（`?3` 传 NULL 表示不筛模型）
const WHERE_LOG: &str =
    "WHERE started_at >= ?1 AND started_at < ?2 AND (?3 IS NULL OR model_id = ?3)";

/// 缓存命中率的分母口径：`separate` 源（claude / kimi）的 input_tokens **不含**
/// 缓存读（缓存读写单独成列），分母 = 输入 + 缓存读 + 缓存写；其余 `included`
/// 源（zcode / codex / dsh / opencode / minimaxcode）输入已含缓存读，分母 =
/// 输入。分子统一为缓存读（见模块头）。
fn cache_denominator(source: &str, totals: &SourceTotals) -> i64 {
    if matches!(source, "claude" | "kimi") {
        totals
            .input_tokens
            .saturating_add(totals.cache_read_tokens)
            .saturating_add(totals.cache_write_tokens)
    } else {
        totals.input_tokens
    }
}

/// 逐源聚合结果合并成汇总指标：请求数 / 各项 token 和直接相加；成本按
/// `by_model` 分组逐模型乘价再求和（复用 cost_for，无价格模型贡献 0）；
/// 命中率按来源口径逐个累计（分子分母分源累计，避免 included / separate
/// 混算），分母为 0 → None。
fn merge_summaries(
    totals: &[(&str, SourceTotals)],
    pricing: &BTreeMap<String, crate::pricing::ModelPrice>,
    fx: f64,
) -> RequestLogSummary {
    let mut summary = RequestLogSummary::default();
    let mut hit_numerator = 0i64;
    let mut hit_denominator = 0i64;
    for (source, item) in totals {
        summary.requests += item.requests;
        summary.input_tokens += item.input_tokens;
        summary.output_tokens += item.output_tokens;
        summary.cache_read_tokens += item.cache_read_tokens;
        summary.cache_write_tokens += item.cache_write_tokens;
        summary.total_tokens += item.total_tokens;
        for group in &item.by_model {
            summary.cost_usd += crate::cost_for(group, pricing);
        }
        hit_numerator += item.cache_read_tokens;
        hit_denominator += cache_denominator(source, item);
    }
    summary.cost_cny = summary.cost_usd * fx;
    summary.cache_hit_rate = (hit_denominator > 0)
        .then(|| hit_numerator as f64 / hit_denominator as f64);
    summary
}

/// 单来源全范围聚合（与分页查询同一 WHERE）：一条聚合 SQL 出请求数与各项
/// token 和，另一条按 model_id 分组出成本折算输入（两条 SQL 结果合并为
/// SourceTotals；请求数为 0 时不必再发分组查询）。
fn query_source_totals(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
    model: Option<&str>,
) -> rusqlite::Result<SourceTotals> {
    let mut totals = conn.query_row(
        &format!(
            "SELECT COUNT(*), \
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), \
                    COALESCE(SUM(cache_read_input_tokens),0), \
                    COALESCE(SUM(cache_creation_input_tokens),0), \
                    COALESCE(SUM(reasoning_tokens),0), \
                    COALESCE(SUM(computed_total_tokens),0) \
             FROM model_usage {WHERE_LOG}"
        ),
        params![from_ms, to_ms, model],
        |row| {
            Ok(SourceTotals {
                requests: row.get(0)?,
                input_tokens: row.get(1)?,
                output_tokens: row.get(2)?,
                cache_read_tokens: row.get(3)?,
                cache_write_tokens: row.get(4)?,
                // 5 = reasoning_tokens（汇总契约不展示，跳过读取）
                total_tokens: row.get(6)?,
                by_model: Vec::new(),
            })
        },
    )?;
    if totals.requests == 0 {
        return Ok(totals);
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT COALESCE(model_id,''), COALESCE(SUM(input_tokens),0), \
                COALESCE(SUM(output_tokens),0), COALESCE(SUM(cache_read_input_tokens),0) \
         FROM model_usage {WHERE_LOG} GROUP BY model_id"
    ))?;
    let groups = stmt
        .query_map(params![from_ms, to_ms, model], |row| {
            Ok(ModelTokens {
                model_id: row.get(0)?,
                input_tokens: row.get(1)?,
                output_tokens: row.get(2)?,
                cache_read_tokens: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    totals.by_model = groups;
    Ok(totals)
}

/// 单来源查询：(该来源区间内的全范围聚合, 按 started_at DESC 的前 limit 行)。
/// 库不存在返回 (空聚合, 空)；其余错误上报给调用方记录并跳过。
fn query_source(
    source: &str,
    from_ms: i64,
    to_ms: i64,
    model: Option<&str>,
    limit: i64,
) -> Result<(SourceTotals, Vec<RequestLogRow>), String> {
    let Some(conn) = open_source(source)? else {
        return Ok((SourceTotals::default(), Vec::new()));
    };
    let totals = query_source_totals(&conn, from_ms, to_ms, model)
        .map_err(|e| format!("统计 {source} 请求条数失败: {e}"))?;
    if totals.requests == 0 {
        return Ok((totals, Vec::new()));
    }
    let sql = format!(
        "SELECT {} FROM model_usage {WHERE_LOG} ORDER BY started_at DESC LIMIT ?4",
        select_list(&conn, source)
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 {source} 请求日志查询失败: {e}"))?;
    let rows = stmt
        .query_map(params![from_ms, to_ms, model, limit], |row| {
            row_to_log(row, source)
        })
        .map_err(|e| format!("读取 {source} 请求日志失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 {source} 请求日志失败: {e}"))?;
    Ok((totals, rows))
}

/// 单来源的模型计数（get_request_log_models 用；库不存在返回空）
fn query_source_models(
    source: &str,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<RequestLogModelCount>, String> {
    let Some(conn) = open_source(source)? else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(model_id,''), COUNT(*) FROM model_usage \
             WHERE started_at >= ?1 AND started_at < ?2 \
             GROUP BY model_id ORDER BY 2 DESC",
        )
        .map_err(|e| format!("准备 {source} 模型清单查询失败: {e}"))?;
    let rows = stmt
        .query_map(params![from_ms, to_ms], |row| {
            Ok(RequestLogModelCount {
                model_id: row.get(0)?,
                count: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取 {source} 模型清单失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 {source} 模型清单失败: {e}"))?;
    Ok(rows)
}

/// 取数窗口与分页入参的归一：page >= 0；page_size <= 0 按缺省 20，
/// 否则夹在 [10, 100]。
fn normalize_paging(page: i64, page_size: i64) -> (i64, i64) {
    let page = page.max(0);
    let page_size = if page_size <= 0 {
        DEFAULT_PAGE_SIZE
    } else {
        page_size.clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE)
    };
    (page, page_size)
}

/// 逐源取数并合并：返回（各来源全范围聚合、归并前的全部明细、失败来源名）。
/// 单源失败只记 eprintln! 并计入 failed_sources，其余来源照常合并
///（可测接缝：fetch 由调用方注入，测试用假 fetch 构造失败源）。
fn collect_sources<F>(
    sources: &[&'static str],
    fetch: F,
) -> (Vec<(&'static str, SourceTotals)>, Vec<RequestLogRow>, Vec<String>)
where
    F: Fn(&str) -> Result<(SourceTotals, Vec<RequestLogRow>), String>,
{
    let mut totals: Vec<(&'static str, SourceTotals)> = Vec::new();
    let mut rows: Vec<RequestLogRow> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for source in sources {
        match fetch(source) {
            Ok((source_totals, list)) => {
                totals.push((source, source_totals));
                rows.extend(list);
            }
            Err(e) => {
                eprintln!("[zbar-request-logs] 来源 {source} 查询失败，已跳过: {e}");
                failed.push((*source).to_string());
            }
        }
    }
    (totals, rows, failed)
}

/// get_request_logs：请求日志分页查询（7 个本地来源合并后按时间倒序）。
///
/// - `agent`：来源筛选（None / 空串 = 全部 7 源；只认 SOURCES 里的来源名）；
/// - `model`：模型 id 精确筛选（None = 不筛）；
/// - 同步命令：全部是本地 SQLite 只读查询，单源最多 (page+1)×page_size 行
///   （上限 MAX_FETCH_ROWS），毫秒级，与 get_codex_usage 同款形态但不做并发。
#[tauri::command]
pub fn get_request_logs(
    agent: Option<String>,
    model: Option<String>,
    from_ms: i64,
    to_ms: i64,
    page: i64,
    page_size: i64,
) -> Result<RequestLogsPage, String> {
    let sources = select_sources(agent.as_deref())?;
    let (page, page_size) = normalize_paging(page, page_size);
    let model = match model.as_deref() {
        Some(m) if !m.is_empty() => Some(m),
        _ => None,
    };
    // 单源取足「到当前页为止」的行数，归并后再全局切片（见模块头）
    let limit = page
        .saturating_add(1)
        .saturating_mul(page_size)
        .min(MAX_FETCH_ROWS);

    let (totals, mut merged, failed_sources) = collect_sources(&sources, |source| {
        query_source(source, from_ms, to_ms, model, limit)
    });
    let total: i64 = totals.iter().map(|(_, item)| item.requests).sum();

    // 时间倒序；同毫秒按来源名稳定序（sort_by 稳定，源内保持库内顺序）
    merged.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| a.source.cmp(&b.source))
    });

    let skip = usize::try_from(page.saturating_mul(page_size)).unwrap_or(usize::MAX);
    let mut rows: Vec<RequestLogRow> = merged
        .into_iter()
        .skip(skip)
        .take(page_size as usize)
        .collect();

    // 花费：当前页逐行、汇总按模型分组，都走 cost_for 口径（无价格模型按 0）；
    // 人民币 = 美元 × 当前汇率
    let pricing = crate::load_pricing().unwrap_or_default();
    let fx = crate::load_fx_rate();
    for row in &mut rows {
        row.cost_usd = crate::cost_for(row, &pricing.usd);
        row.cost_cny = row.cost_usd * fx;
    }
    let summary = merge_summaries(&totals, &pricing.usd, fx);

    Ok(RequestLogsPage {
        total,
        rows,
        summary,
        failed_sources,
    })
}

/// get_request_log_models：区间内出现过的模型 + 请求条数（模型筛选下拉用）。
/// 逐源 GROUP BY 后跨源合并同名模型，按条数降序（同数按模型名升序）、上限 100。
#[tauri::command]
pub fn get_request_log_models(
    agent: Option<String>,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<RequestLogModelCount>, String> {
    let sources = select_sources(agent.as_deref())?;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for source in &sources {
        match query_source_models(source, from_ms, to_ms) {
            Ok(list) => {
                for item in list {
                    // 空模型名（历史回填未完成的行）不进筛选下拉
                    if item.model_id.is_empty() {
                        continue;
                    }
                    *counts.entry(item.model_id).or_insert(0) += item.count;
                }
            }
            Err(e) => eprintln!("[zbar-request-logs] 来源 {source} 模型清单查询失败，已跳过: {e}"),
        }
    }
    let mut merged: Vec<RequestLogModelCount> = counts
        .into_iter()
        .map(|(model_id, count)| RequestLogModelCount { model_id, count })
        .collect();
    merged.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.model_id.cmp(&b.model_id)));
    merged.truncate(MAX_MODEL_OPTIONS);
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 分页入参归一：缺省 / 越界 / 负数
    #[test]
    fn normalize_paging_clamps() {
        assert_eq!(normalize_paging(0, 20), (0, 20));
        assert_eq!(normalize_paging(-3, 20), (0, 20));
        assert_eq!(normalize_paging(2, 0), (2, 20));
        assert_eq!(normalize_paging(2, 3), (2, 10));
        assert_eq!(normalize_paging(2, 999), (2, 100));
    }

    /// 来源筛选：空 / None = 全部；未知来源报错
    #[test]
    fn select_sources_filters() {
        assert_eq!(select_sources(None).unwrap(), SOURCES.to_vec());
        assert_eq!(select_sources(Some("")).unwrap(), SOURCES.to_vec());
        assert_eq!(select_sources(Some("zcode")).unwrap(), vec!["zcode"]);
        assert!(select_sources(Some("nope")).is_err());
    }

    /// 项目表达式：列存在性组合与 session_meta 前缀
    #[test]
    fn combine_project_variants() {
        assert_eq!(
            combine_project(true, true, "").unwrap(),
            "COALESCE(NULLIF(project_key,''), NULLIF(cwd,''))"
        );
        assert_eq!(combine_project(true, false, "sm.").unwrap(), "NULLIF(sm.project_key,'')");
        assert_eq!(combine_project(false, true, "").unwrap(), "NULLIF(cwd,'')");
        assert!(combine_project(false, false, "").is_none());
    }

    /// 未知来源的库路径直接报错（不会静默取到别的库）
    #[test]
    fn unknown_source_path_errors() {
        assert!(source_db_path("nope").is_err());
    }

    /// SELECT 列表随库结构降级，且与 row_to_log 的下标一一对应：
    /// 建一张「齐全列」的内存表 + 一张「缺列」的内存表各读一行。
    #[test]
    fn select_list_degrades_and_reads() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER NOT NULL,
                model_id TEXT, provider_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER,
                reasoning_tokens INTEGER, computed_total_tokens INTEGER,
                session_id TEXT, duration_ms REAL, time_to_first_token_ms INTEGER,
                cwd TEXT, project_key TEXT);
             INSERT INTO model_usage VALUES
                (1000, 'glm-5', 'zhipu', 10, 20, 5, 1, 2, 30, 's1', 1200.0, 300, '/tmp/a', 'a');",
        )
        .unwrap();
        let sql = format!("SELECT {} FROM model_usage", select_list(&conn, "dsh"));
        let row = conn.query_row(&sql, [], |row| row_to_log(row, "dsh")).unwrap();
        assert_eq!(row.started_at, 1000);
        assert_eq!(row.model_id, "glm-5");
        assert_eq!(row.total_tokens, 30);
        assert_eq!(row.session_id.as_deref(), Some("s1"));
        assert_eq!(row.project.as_deref(), Some("a"));
        assert_eq!(row.duration_ms, Some(1200.0));
        assert_eq!(row.ttft_ms, Some(300.0));
        // 速度列缺 first_token_at / completed_at 原料：退化为 request_average
        // 近似（duration_ms 兜底窗口），不是 None
        assert_eq!(
            row.speed_quality,
            Some(crate::token_speed::SpeedQuality::RequestAverage)
        );
        assert!(row.speed_tps.is_some());

        // 旧库：无 session_id / duration_ms / ttft / 项目列 → 全 None、不报错
        let legacy = Connection::open_in_memory().unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE model_usage (
                    started_at INTEGER NOT NULL, model_id TEXT, provider_id TEXT,
                    input_tokens INTEGER, output_tokens INTEGER,
                    cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER,
                    reasoning_tokens INTEGER, computed_total_tokens INTEGER);
                 INSERT INTO model_usage VALUES (2000, 'm', 'p', 1, 2, 0, 0, 0, 3);",
            )
            .unwrap();
        let sql = format!("SELECT {} FROM model_usage", select_list(&legacy, "claude"));
        let row = legacy
            .query_row(&sql, [], |row| row_to_log(row, "claude"))
            .unwrap();
        assert_eq!(row.started_at, 2000);
        assert!(row.session_id.is_none());
        assert!(row.project.is_none());
        assert!(row.duration_ms.is_none());
        assert!(row.ttft_ms.is_none());
        // 无任何时间原料（无事件时刻也无耗时）：速度两字段回退 None
        assert!(row.speed_tps.is_none());
        assert!(row.speed_quality.is_none());
    }

    /// 行级速度走 token_speed 唯一真源：非思考行用首字窗口（completed −
    /// first），思考型行（reasoning > 0）用全程窗口（completed − started，
    /// 思考 token 在首字之前流出、按首字窗口会把思考时长漏出分母导致虚高），
    /// claude 系按模型名识别（reasoning 恒 0 也前移窗口）；仅有 duration_ms
    /// 的导入库形态退化为 request_average 近似（序列化值 "request_average"，
    /// 前端 ≈ 前缀依赖该契约）。
    #[test]
    fn row_speed_follows_token_speed_truth() {
        // zcode 形态（齐全列：含 first_token_at / completed_at）
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER NOT NULL,
                model_id TEXT, provider_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER,
                reasoning_tokens INTEGER, computed_total_tokens INTEGER,
                session_id TEXT, duration_ms REAL, time_to_first_token_ms INTEGER,
                cwd TEXT, project_key TEXT,
                first_token_at INTEGER, completed_at INTEGER);
             INSERT INTO model_usage VALUES
                -- 非思考行：首字窗口 8s → 1000 tok = 125 t/s（真实耗时 101s
                -- 只进 duration 列，不参与生成分母）
                (1000, 'glm-5', 'p', 10, 1000, 0, 0, 0, 1010,
                 's', 101000.0, 92000, NULL, NULL, 93000, 101000),
                -- 思考行：同量级时间戳 + reasoning > 0 → 全程窗口 100s
                -- = 10 t/s（首字窗口会把 92s 思考时长漏出分母、虚高 12.5 倍）
                (2000, 'glm-5', 'p', 10, 1000, 0, 0, 80000, 81010,
                 's', 101000.0, 92000, NULL, NULL, 94000, 102000),
                -- claude 系：reasoning 恒 0，按模型名识别 → 全程窗口 10 t/s
                (3000, 'Claude-Opus-5', 'p', 10, 1000, 0, 0, 0, 1010,
                 's', 101000.0, 92000, NULL, NULL, 95000, 103000);",
        )
        .unwrap();
        let sql = format!(
            "SELECT {} FROM model_usage ORDER BY started_at",
            select_list(&conn, "zcode")
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let rows = stmt
            .query_map([], |row| row_to_log(row, "zcode"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 3);
        // 非思考行：首字窗口（generation，无 ≈）
        assert_eq!(
            rows[0].speed_quality,
            Some(crate::token_speed::SpeedQuality::Generation)
        );
        let tps = rows[0].speed_tps.expect("非思考行应有生成速度");
        assert!((tps - 125.0).abs() < 1e-9, "{tps}");
        // 思考行：全程窗口（思考感知，速度不被虚高）
        let tps = rows[1].speed_tps.expect("思考行应有生成速度");
        assert!((tps - 10.0).abs() < 1e-9, "{tps}");
        assert_eq!(
            rows[1].speed_quality,
            Some(crate::token_speed::SpeedQuality::Generation)
        );
        // claude 系按名识别：同思考行口径
        let tps = rows[2].speed_tps.expect("claude 行应有生成速度");
        assert!((tps - 10.0).abs() < 1e-9, "{tps}");

        // claude / kimi 导入库形态：只有 duration_ms（无事件时刻列）→
        // request_average 近似，serde 序列化为 "request_average"（snake_case
        // 值 + snake_case 字段名，前端 SpeedQuality 契约）
        let imported = Connection::open_in_memory().unwrap();
        imported
            .execute_batch(
                "CREATE TABLE model_usage (
                    started_at INTEGER NOT NULL, model_id TEXT, provider_id TEXT,
                    input_tokens INTEGER, output_tokens INTEGER,
                    cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER,
                    reasoning_tokens INTEGER, computed_total_tokens INTEGER,
                    duration_ms REAL);
                 INSERT INTO model_usage VALUES
                    (1000, 'kimi-k2', 'p', 10, 1200, 0, 0, 0, 1210, 1000.0);",
            )
            .unwrap();
        let sql = format!(
            "SELECT {} FROM model_usage",
            select_list(&imported, "kimi")
        );
        let row = imported
            .query_row(&sql, [], |row| row_to_log(row, "kimi"))
            .unwrap();
        assert_eq!(
            row.speed_quality,
            Some(crate::token_speed::SpeedQuality::RequestAverage)
        );
        let tps = row.speed_tps.expect("有耗时的行应有近似速度");
        assert!((tps - 1200.0).abs() < 1e-9, "{tps}");
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["speed_quality"], "request_average");
        assert!(json["speed_tps"].is_number());

        // 无 first_token_at / completed_at 也无 duration_ms（codex 形态）：
        // 无速度原料 → 两字段 None，前端回退（旧公式同样算不出）
        let codex_like = Connection::open_in_memory().unwrap();
        codex_like
            .execute_batch(
                "CREATE TABLE model_usage (
                    started_at INTEGER NOT NULL, model_id TEXT, provider_id TEXT,
                    input_tokens INTEGER, output_tokens INTEGER,
                    cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER,
                    reasoning_tokens INTEGER, computed_total_tokens INTEGER);
                 INSERT INTO model_usage VALUES
                    (1000, 'gpt-5', 'p', 10, 1200, 0, 0, 0, 1210);",
            )
            .unwrap();
        let sql = format!(
            "SELECT {} FROM model_usage",
            select_list(&codex_like, "codex")
        );
        let row = codex_like
            .query_row(&sql, [], |row| row_to_log(row, "codex"))
            .unwrap();
        assert!(row.speed_tps.is_none());
        assert!(row.speed_quality.is_none());
    }

    /// 耗时只认真实 duration_ms：列存在时 > 0 才给；NULL / 0 → None；
    /// 无该列（如 codex 导入库）→ NULL 表达式，不报错也不估算。
    #[test]
    fn duration_reads_real_only() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER NOT NULL,
                model_id TEXT, provider_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER,
                reasoning_tokens INTEGER, computed_total_tokens INTEGER,
                duration_ms REAL);
             INSERT INTO model_usage VALUES
                (1, 'm', 'p', 1, 1, 0, 0, 0, 2, 5000.0),
                (2, 'm', 'p', 1, 1, 0, 0, 0, 2, NULL),
                (3, 'm', 'p', 1, 1, 0, 0, 0, 2, 0.0);
             ",
        )
        .unwrap();
        let sql = format!(
            "SELECT {} FROM model_usage ORDER BY started_at",
            select_list(&conn, "claude")
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let rows = stmt
            .query_map([], |row| row_to_log(row, "claude"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows[0].duration_ms, Some(5000.0));
        assert!(rows[1].duration_ms.is_none());
        assert!(rows[2].duration_ms.is_none());

        // 无 duration_ms 列：SELECT NULL 且下标不错位（ttft 紧跟其后）
        let no_duration = Connection::open_in_memory().unwrap();
        no_duration
            .execute_batch(
                "CREATE TABLE model_usage (
                    started_at INTEGER NOT NULL,
                    model_id TEXT, provider_id TEXT,
                    input_tokens INTEGER, output_tokens INTEGER,
                    cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER,
                    reasoning_tokens INTEGER, computed_total_tokens INTEGER,
                    time_to_first_token_ms INTEGER);
                 INSERT INTO model_usage VALUES (4000, 'm', 'p', 1, 1, 0, 0, 0, 2, 700);",
            )
            .unwrap();
        let sql = format!("SELECT {} FROM model_usage", select_list(&no_duration, "codex"));
        let row = no_duration
            .query_row(&sql, [], |row| row_to_log(row, "codex"))
            .unwrap();
        assert_eq!(row.started_at, 4000);
        assert!(row.duration_ms.is_none());
        assert_eq!(row.ttft_ms, Some(700.0));
    }

    /// 单来源聚合：一条聚合 SQL（计数 + token 和）+ 一条按模型分组 SQL 合并；
    /// 模型筛选条件同样生效；无行时请求数为 0 且不报错。
    #[test]
    fn query_source_totals_merges_two_sqls() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER NOT NULL,
                model_id TEXT, provider_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER,
                reasoning_tokens INTEGER, computed_total_tokens INTEGER);
             INSERT INTO model_usage VALUES
                (100, 'a', 'p', 10, 5, 4, 1, 2, 20),
                (200, 'a', 'p', 20, 6, 0, 0, 0, 26),
                (300, 'b', 'p', 30, 7, 3, 2, 1, 42),   -- 超出区间，不计
                (150, 'b', 'p', 40, 8, 9, 0, 0, 57);",
        )
        .unwrap();
        let totals = query_source_totals(&conn, 0, 300, None).unwrap();
        assert_eq!(totals.requests, 3);
        assert_eq!(totals.input_tokens, 70);
        assert_eq!(totals.output_tokens, 19);
        assert_eq!(totals.cache_read_tokens, 13);
        assert_eq!(totals.cache_write_tokens, 1);
        assert_eq!(totals.total_tokens, 103);
        assert_eq!(totals.by_model.len(), 2);
        assert_eq!(
            totals.by_model[0],
            ModelTokens {
                model_id: "a".into(),
                input_tokens: 30,
                output_tokens: 11,
                cache_read_tokens: 4,
            }
        );
        assert_eq!(
            totals.by_model[1],
            ModelTokens {
                model_id: "b".into(),
                input_tokens: 40,
                output_tokens: 8,
                cache_read_tokens: 9,
            }
        );

        // 模型筛选：只算 b
        let only_b = query_source_totals(&conn, 0, 300, Some("b")).unwrap();
        assert_eq!(only_b.requests, 1);
        assert_eq!(only_b.input_tokens, 40);
        assert_eq!(only_b.by_model.len(), 1);

        // 区间内无行：0 且 by_model 空
        let empty = query_source_totals(&conn, 10_000, 20_000, None).unwrap();
        assert_eq!(empty, SourceTotals::default());
    }

    /// 汇总合并：请求数与各项 token 直接相加；成本按模型分组逐组乘价求和
    ///（无价格模型贡献 0）；命中率按来源口径（included / separate）累计。
    #[test]
    fn merge_summaries_mixes_cache_rates() {
        // included 源：分母 = input（input 已含缓存读）
        let included = SourceTotals {
            requests: 2,
            input_tokens: 100,
            output_tokens: 10,
            cache_read_tokens: 40,
            cache_write_tokens: 0,
            total_tokens: 110,
            by_model: vec![ModelTokens {
                model_id: "priced".into(),
                input_tokens: 100,
                output_tokens: 10,
                cache_read_tokens: 40,
            }],
        };
        // separate 源：分母 = input + read + write
        let separate = SourceTotals {
            requests: 1,
            input_tokens: 50,
            output_tokens: 5,
            cache_read_tokens: 30,
            cache_write_tokens: 20,
            total_tokens: 105,
            by_model: vec![ModelTokens {
                model_id: "unpriced".into(),
                input_tokens: 50,
                output_tokens: 5,
                cache_read_tokens: 30,
            }],
        };
        let mut pricing = BTreeMap::new();
        pricing.insert(
            "priced".to_string(),
            crate::pricing::ModelPrice {
                input: 1.0,
                output: 2.0,
                cache_read: 0.5,
            },
        );
        let summary = merge_summaries(
            &[("codex", included.clone()), ("claude", separate)],
            &pricing,
            7.0,
        );
        assert_eq!(summary.requests, 3);
        assert_eq!(summary.input_tokens, 150);
        assert_eq!(summary.output_tokens, 15);
        assert_eq!(summary.cache_read_tokens, 70);
        assert_eq!(summary.cache_write_tokens, 20);
        assert_eq!(summary.total_tokens, 215);
        // 只折算到 priced 一组：非缓存输入 60*1.0 + 输出 10*2.0 + 缓存读 40*0.5 = 100（每百万 token）
        let expected_usd = (60.0 * 1.0 + 10.0 * 2.0 + 40.0 * 0.5) / 1_000_000.0;
        assert!((summary.cost_usd - expected_usd).abs() < 1e-12);
        assert!((summary.cost_cny - expected_usd * 7.0).abs() < 1e-12);
        // 分子 = 40 + 30 = 70；分母 = 100（included input）+ 50 + 30 + 20（separate）= 200
        let rate = summary.cache_hit_rate.expect("应有命中率");
        assert!((rate - 70.0 / 200.0).abs() < 1e-12);

        // 分母为 0（无输入、无缓存）→ None
        let empty = merge_summaries(&[], &pricing, 7.0);
        assert!(empty.cache_hit_rate.is_none());
        assert_eq!(empty.requests, 0);
        assert_eq!(empty.cost_usd, 0.0);

        // 只有 included 源时命中率 = read / input
        let only_included = merge_summaries(&[("zcode", included)], &pricing, 7.0);
        assert!((only_included.cache_hit_rate.unwrap() - 0.4).abs() < 1e-12);
        // 只有 separate 源、且输入与缓存全为 0 → None（分母 0）
        let zero_separate = SourceTotals::default();
        assert!(merge_summaries(&[("kimi", zero_separate)], &pricing, 7.0)
            .cache_hit_rate
            .is_none());
    }

    /// 失败来源进 failed_sources、其余来源照常合并（单源失败不拖垮整页）；
    /// 「库不存在」的 0 条不算失败
    #[test]
    fn collect_sources_tracks_failures() {
        let make_row = |ts: i64, source: &str| RequestLogRow {
            source: source.to_string(),
            started_at: ts,
            model_id: "m".into(),
            provider_id: "p".into(),
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: 2,
            cost_usd: 0.0,
            cost_cny: 0.0,
            session_id: None,
            project: None,
            duration_ms: None,
            ttft_ms: None,
            speed_tps: None,
            speed_quality: None,
        };
        let (totals, rows, failed) =
            collect_sources(&["codex", "kimi", "dsh"], |source| match source {
                "kimi" => Err("no such table: model_usage".to_string()),
                // dsh 库不存在（未安装）：正常 0 条，不算失败
                "dsh" => Ok((SourceTotals::default(), Vec::new())),
                other => Ok((
                    SourceTotals {
                        requests: 1,
                        ..Default::default()
                    },
                    vec![make_row(1000, other)],
                )),
            });
        let total: i64 = totals.iter().map(|(_, item)| item.requests).sum();
        assert_eq!(total, 1);
        assert_eq!(totals.len(), 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "codex");
        assert_eq!(failed, vec!["kimi".to_string()]);
    }

    /// 会话元表相关子查询：有表才拼、无表降级为 NULL
    #[test]
    fn session_meta_project_expr_guards_missing_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE model_usage (session_id TEXT);")
            .unwrap();
        assert!(session_meta_project_expr(&conn).is_none());
        conn.execute_batch(
            "CREATE TABLE session_meta (session_id TEXT PRIMARY KEY, cwd TEXT, project_key TEXT);",
        )
        .unwrap();
        let expr = session_meta_project_expr(&conn).unwrap();
        assert!(expr.contains("sm.session_id = model_usage.session_id"));
        assert!(expr.contains("sm.project_key"));
    }
}
