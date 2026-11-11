//! 会话维度取数（主窗口「会话」页）。
//!
//! 数据源与请求日志页完全一致（`request_logs` 的 7 个本地来源，只读打开、
//! 不建表不迁移），区别只是**把行按「来源 + 会话 id」重新分组**，回答两个
//! 问题：
//!
//! - 现在有哪些会话在跑、每个会话累计花了多少（实时会话账本）；
//! - 单个会话的上下文水位是怎么长起来的、被压缩过几次（上下文健康）。
//!
//! # 上下文口径
//!
//! 一次请求的 prompt 侧 token 就是"这一轮模型实际看到的上下文"。分源口径
//! 沿用 `request_logs`：`separate` 源（claude / kimi）的 `input_tokens`
//! **不含**缓存读写，prompt = input + cache_read + cache_write；其余
//! `included` 源 input 已含缓存读，prompt = input + cache_write。
//!
//! 压缩（compaction）没有跨来源统一的事件字段，这里用**水位骤降**做推断：
//! 按时间正序走一遍，若本轮 prompt 比上一轮低 45% 以上、且上一轮已经不小
//! （≥ 10k），记一次。这个口径对"自动压缩 / 清空会话 / 换分支重开"都成立，
//! 属于估算，界面按「压缩」标注而不宣称精确。
//!
//! # 为什么不给百分比水位
//!
//! 模型上下文窗口各家并不公开统一字段，这里**只收录明确已知的窗口**，
//! 未收录模型的 `limit` 一律 `None`——前端只画相对本会话峰值的绝对水位，
//! 不臆造"已用 73%"这种数字。

use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

use crate::pricing::ModelPrice;
use crate::request_logs::{query_source, RequestLogRow, SOURCES};

/// 单源一次最多取多少行（与请求日志深页保护同量级：实时页只关心最近窗口）
const MAX_ROWS: i64 = 5_000;
/// 判定「运行中」的活跃窗口：最近一次请求距现在不超过这个值
const ACTIVE_WINDOW_MS: i64 = 3 * 60 * 1000;
/// 会话查询窗口（小时）的夹取范围与缺省
const MIN_WINDOW_HOURS: i64 = 1;
const MAX_WINDOW_HOURS: i64 = 72;
const DEFAULT_WINDOW_HOURS: i64 = 6;
/// 会话账本最多返回多少条（按最近活动倒序截断，避免一次渲染上千行）
const MAX_SESSIONS: usize = 200;
/// 单会话上下文曲线的最大点数（超出按等距抽样，保首尾）
const MAX_CONTEXT_POINTS: usize = 600;
/// 压缩判定阈值：水位相对上一轮下降比例 + 上一轮的最小规模
const COMPACT_DROP_RATIO: f64 = 0.45;
const COMPACT_MIN_TOKENS: i64 = 10_000;

/// 一个会话的账本行（一次会话 = 同来源同 session_id 的全部请求）
#[derive(Debug, Clone, Serialize)]
pub struct LiveSession {
    /// 来源名（与请求日志的 RequestLogSource 同一套字面量）
    pub source: String,
    /// 会话 id；来源库没有该列（老库 / 部分 CLI）时为空串，前端归并成「未标注会话」
    pub session_id: String,
    /// 项目（能低成本拿到的来源才给）
    pub project: Option<String>,
    /// 最近一次请求的模型
    pub model_id: String,
    /// 会话内最早 / 最近一次请求时间（毫秒）
    pub started_at: i64,
    pub last_at: i64,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub cost_cny: f64,
    /// 最近一次请求的 prompt 侧 token（≈ 当前上下文占用）
    pub context_last: i64,
    /// 会话内 prompt 侧峰值
    pub context_peak: i64,
    /// prompt 侧骤降次数（≡ 上下文被压缩 / 清空，口径见模块注释）
    pub compactions: i64,
    /// 会话内缓存命中率（分子缓存读，分母按来源口径）
    pub cache_hit_rate: Option<f64>,
    /// 会话内按输出加权的平均输出速度（tok/s；无有效样本 → None）
    pub speed_tps: Option<f64>,
    /// 首字延迟均值（仅计 > 0 的样本）
    pub avg_ttft_ms: Option<f64>,
    /// "running"（最近 3 分钟内有请求）| "idle"
    pub state: String,
    /// 模型上下文窗口（仅内置表明确收录时给，否则 None）
    pub context_limit: Option<i64>,
}

/// 实时会话账本的一次取数结果
#[derive(Debug, Clone, Serialize)]
pub struct SessionFeed {
    pub sessions: Vec<LiveSession>,
    /// 查询失败的来源（库损坏 / 权限），前端弱提示「数据可能不完整」
    pub failed_sources: Vec<String>,
    /// 取数时刻（前端显示"刷新于 HH:mm:ss"）
    pub generated_at: i64,
    /// 本次生效的窗口小时数（夹取后的真实值）
    pub window_hours: i64,
}

/// 上下文曲线上的一个点（一次请求）
#[derive(Debug, Clone, Serialize)]
pub struct ContextPoint {
    pub ts: i64,
    /// 这一轮模型看到的上下文（prompt 侧 token）
    pub prompt_tokens: i64,
    pub output_tokens: i64,
    /// 这一轮的缓存读占比（0~1；分母同上，无有效分母 → None）
    pub cache_ratio: Option<f64>,
    pub model_id: String,
}

/// 单会话上下文健康视图
#[derive(Debug, Clone, Serialize)]
pub struct SessionContext {
    pub source: String,
    pub session_id: String,
    pub project: Option<String>,
    /// 最近一次请求的模型（limit 按它查）
    pub model_id: String,
    pub points: Vec<ContextPoint>,
    /// 水位峰值 / 当前值
    pub peak: i64,
    pub last: i64,
    /// 骤降（压缩）次数
    pub compactions: i64,
    /// 模型窗口上限（未知 → None，前端不画百分比）
    pub context_limit: Option<i64>,
    pub window_hours: i64,
}

/// 当前时刻（毫秒）
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 窗口小时数夹取（0 / 负数 / 缺省 → 6 小时；上下界夹到 1~72）
fn clamp_window(hours: Option<i64>) -> i64 {
    hours
        .filter(|h| *h > 0)
        .unwrap_or(DEFAULT_WINDOW_HOURS)
        .clamp(MIN_WINDOW_HOURS, MAX_WINDOW_HOURS)
}

/// 该来源的 input_tokens 是否不含缓存读写（separate 源）
fn is_separate(source: &str) -> bool {
    matches!(source, "claude" | "kimi")
}

/// prompt 侧 token 的口径本体（included / separate 两种来源分开处理）。
///
/// 抽成自由函数是为了让「水位公式」只有一处定义：`prompt_tokens` 用它、
/// 单测直接打它，避免以后改一处漏一处。
pub(crate) fn prompt_tokens_of(
    source: &str,
    input_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
) -> i64 {
    let base = input_tokens
        .max(0)
        .saturating_add(cache_write_tokens.max(0));
    if is_separate(source) {
        base.saturating_add(cache_read_tokens.max(0))
    } else {
        base
    }
}

/// 一次请求的 prompt 侧 token（≈ 本轮上下文占用）
pub(crate) fn prompt_tokens(row: &RequestLogRow) -> i64 {
    prompt_tokens_of(
        &row.source,
        row.input_tokens,
        row.cache_read_tokens,
        row.cache_write_tokens,
    )
}

/// 压缩判定：比上一轮低 45% 以上，且上一轮已经不小（≥ 10k）。
/// 会话聚合与单会话曲线共用这一条规则（前端画标记时会重算同样的阈值，
/// 只用于定位横坐标；口径改动需同步两处）。
fn is_compaction(previous: i64, current: i64) -> bool {
    previous >= COMPACT_MIN_TOKENS && (current as f64) < previous as f64 * (1.0 - COMPACT_DROP_RATIO)
}

/// 单行的缓存读占比（分母按来源口径；分母为 0 → None）
fn cache_ratio_of(row: &RequestLogRow) -> Option<f64> {
    let denominator = if is_separate(&row.source) {
        row.input_tokens
            .max(0)
            .saturating_add(row.cache_read_tokens.max(0))
            .saturating_add(row.cache_write_tokens.max(0))
    } else {
        row.input_tokens.max(0)
    };
    (denominator > 0).then(|| row.cache_read_tokens.max(0) as f64 / denominator as f64)
}

/// 模型上下文窗口：只收录明确已知的一档，未收录返回 None。
///
/// 刻意做得很保守——宁可不显示百分比，也不要给出错误的水位。新增模型时
/// 在这里补一行即可（按小写子串匹配，先命中先用）。
fn context_limit_of(model: &str) -> Option<i64> {
    let model = model.to_lowercase();
    const TABLE: [(&str, i64); 9] = [
        ("claude", 200_000),
        ("gpt-5", 272_000),
        ("codex", 272_000),
        ("gemini", 1_000_000),
        ("glm-4.6", 200_000),
        ("glm-5", 200_000),
        ("kimi", 256_000),
        ("deepseek", 128_000),
        ("minimax", 200_000),
    ];
    TABLE
        .iter()
        .find(|(needle, _)| model.contains(needle))
        .map(|(_, limit)| *limit)
}

/// 会话聚合的中间态
#[derive(Default)]
struct Agg {
    project: Option<String>,
    model_id: String,
    started_at: i64,
    last_at: i64,
    requests: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    total_tokens: i64,
    cost_usd: f64,
    context_last: i64,
    context_peak: i64,
    compactions: i64,
    speed_weighted: f64,
    speed_output: i64,
    ttft_sum: f64,
    ttft_count: i64,
}

/// 把一个会话的行（已按时间正序）压成一行账本
fn aggregate(
    source: &str,
    session_id: &str,
    rows: &[RequestLogRow],
    pricing: &BTreeMap<String, ModelPrice>,
    fx: f64,
    now: i64,
) -> LiveSession {
    let mut agg = Agg {
        started_at: rows.first().map(|row| row.started_at).unwrap_or(now),
        ..Agg::default()
    };
    let mut previous_prompt: Option<i64> = None;
    for row in rows {
        let prompt = prompt_tokens(row);
        agg.requests += 1;
        agg.input_tokens += row.input_tokens.max(0);
        agg.output_tokens += row.output_tokens.max(0);
        agg.cache_read_tokens += row.cache_read_tokens.max(0);
        agg.cache_write_tokens += row.cache_write_tokens.max(0);
        agg.total_tokens += row.total_tokens.max(0);
        agg.cost_usd += crate::cost_for_source(row, pricing, &row.source);
        agg.last_at = agg.last_at.max(row.started_at);
        if !row.model_id.is_empty() {
            agg.model_id.clone_from(&row.model_id);
        }
        if let Some(project) = row.project.as_deref().filter(|p| !p.is_empty()) {
            agg.project = Some(project.to_string());
        }
        // 上下文水位：正序遍历，最后一行即"当前"
        agg.context_last = prompt;
        agg.context_peak = agg.context_peak.max(prompt);
        if let Some(previous) = previous_prompt {
            if previous >= COMPACT_MIN_TOKENS
                && (prompt as f64) < previous as f64 * (1.0 - COMPACT_DROP_RATIO)
            {
                agg.compactions += 1;
            }
        }
        previous_prompt = Some(prompt);
        // 速度只统计有输出且有速度样本的行，按输出量加权（长回答权重更高）
        if let Some(speed) = row.speed_tps.filter(|s| s.is_finite() && *s > 0.0) {
            if row.output_tokens > 0 {
                let weight = row.output_tokens as f64;
                agg.speed_weighted += speed * weight;
                agg.speed_output += row.output_tokens;
            }
        }
        if let Some(ttft) = row.ttft_ms.filter(|t| t.is_finite() && *t > 0.0) {
            agg.ttft_sum += ttft;
            agg.ttft_count += 1;
        }
    }
    let hit_denominator = if is_separate(source) {
        agg.input_tokens
            .saturating_add(agg.cache_read_tokens)
            .saturating_add(agg.cache_write_tokens)
    } else {
        agg.input_tokens
    };
    LiveSession {
        source: source.to_string(),
        session_id: session_id.to_string(),
        project: agg.project,
        context_limit: context_limit_of(&agg.model_id),
        model_id: agg.model_id,
        started_at: agg.started_at,
        last_at: agg.last_at,
        requests: agg.requests,
        input_tokens: agg.input_tokens,
        output_tokens: agg.output_tokens,
        cache_read_tokens: agg.cache_read_tokens,
        cache_write_tokens: agg.cache_write_tokens,
        total_tokens: agg.total_tokens,
        cost_usd: agg.cost_usd,
        cost_cny: agg.cost_usd * fx,
        context_last: agg.context_last,
        context_peak: agg.context_peak,
        compactions: agg.compactions,
        cache_hit_rate: (hit_denominator > 0)
            .then(|| agg.cache_read_tokens as f64 / hit_denominator as f64),
        speed_tps: (agg.speed_output > 0).then(|| agg.speed_weighted / agg.speed_output as f64),
        avg_ttft_ms: (agg.ttft_count > 0).then(|| agg.ttft_sum / agg.ttft_count as f64),
        state: if now.saturating_sub(agg.last_at) <= ACTIVE_WINDOW_MS {
            "running".to_string()
        } else {
            "idle".to_string()
        },
    }
}

/// 实时会话账本的同步实现：近 window 小时内出现过的会话，按最近活动倒序。
///
/// 单个会话的累计值覆盖**整个查询窗口**（不是只算活跃那几分钟），否则一个
/// 跑了 3 小时、1 分钟前还在请求的会话会被算成"只有 1 分钟的量"。
pub fn collect_live_sessions(window_hours: Option<i64>) -> Result<SessionFeed, String> {
    let now = now_ms();
    let hours = clamp_window(window_hours);
    let from = now.saturating_sub(hours * 3_600_000);
    let pricing = crate::load_pricing().unwrap_or_default();
    let fx = crate::load_fx_rate();

    let mut groups: HashMap<(String, String), Vec<RequestLogRow>> = HashMap::new();
    let mut failed_sources: Vec<String> = Vec::new();
    for source in SOURCES {
        match query_source(source, from, now, None, MAX_ROWS) {
            Ok((_totals, rows)) => {
                for row in rows {
                    let session_id = row.session_id.clone().unwrap_or_default();
                    groups
                        .entry((row.source.clone(), session_id))
                        .or_default()
                        .push(row);
                }
            }
            Err(error) => {
                eprintln!("[zbar-session-inspect] 来源 {source} 会话查询失败，已跳过: {error}");
                failed_sources.push(source.to_string());
            }
        }
    }

    let mut sessions: Vec<LiveSession> = groups
        .into_iter()
        .map(|((source, session_id), mut rows)| {
            rows.sort_by_key(|row| row.started_at);
            aggregate(&source, &session_id, &rows, &pricing.usd, fx, now)
        })
        .collect();
    // 最近活动在前；同毫秒按来源 / 会话 id 稳定排序，避免每拍顺序抖动
    sessions.sort_by(|a, b| {
        b.last_at
            .cmp(&a.last_at)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    sessions.truncate(MAX_SESSIONS);

    Ok(SessionFeed {
        sessions,
        failed_sources,
        generated_at: now,
        window_hours: hours,
    })
}

/// 单会话上下文健康：近 hours 小时内该会话每一次请求的水位序列。
pub fn collect_session_context(
    source: &str,
    session_id: &str,
    hours: Option<i64>,
) -> Result<SessionContext, String> {
    if !SOURCES.contains(&source) {
        return Err(format!("未知的会话来源: {source}"));
    }
    let now = now_ms();
    let window = clamp_window(hours);
    let from = now.saturating_sub(window * 3_600_000);
    let (_totals, rows) = query_source(source, from, now, None, MAX_ROWS)?;

    let mut matched: Vec<RequestLogRow> = rows
        .into_iter()
        .filter(|row| row.session_id.as_deref().unwrap_or("") == session_id)
        .collect();
    matched.sort_by_key(|row| row.started_at);

    let project = matched
        .iter()
        .rev()
        .find_map(|row| row.project.as_deref().filter(|p| !p.is_empty()))
        .map(str::to_string);
    let model_id = matched
        .iter()
        .rev()
        .map(|row| row.model_id.clone())
        .find(|model| !model.is_empty())
        .unwrap_or_default();

    let mut points: Vec<ContextPoint> = matched
        .into_iter()
        .map(|row| ContextPoint {
            ts: row.started_at,
            prompt_tokens: prompt_tokens(&row),
            output_tokens: row.output_tokens.max(0),
            cache_ratio: cache_ratio_of(&row),
            model_id: row.model_id,
        })
        .collect();
    // 超长会话等距抽样（保首尾），避免一次把上万点丢给前端
    if points.len() > MAX_CONTEXT_POINTS {
        points = downsample(points, MAX_CONTEXT_POINTS);
    }

    let mut peak = 0i64;
    let mut compactions = 0i64;
    let mut previous: Option<i64> = None;
    for point in &points {
        peak = peak.max(point.prompt_tokens);
        if let Some(previous_prompt) = previous {
            if previous_prompt >= COMPACT_MIN_TOKENS
                && (point.prompt_tokens as f64) < previous_prompt as f64 * (1.0 - COMPACT_DROP_RATIO)
            {
                compactions += 1;
            }
        }
        previous = Some(point.prompt_tokens);
    }

    Ok(SessionContext {
        source: source.to_string(),
        session_id: session_id.to_string(),
        project,
        context_limit: context_limit_of(&model_id),
        model_id,
        peak,
        last: points.last().map(|point| point.prompt_tokens).unwrap_or(0),
        compactions,
        points,
        window_hours: window,
    })
}

/// 等距抽样（保首尾），用于超长会话的水位曲线
fn downsample(points: Vec<ContextPoint>, target: usize) -> Vec<ContextPoint> {
    if target < 2 || points.len() <= target {
        return points;
    }
    let step = points.len() as f64 / (target as f64 - 1.0);
    let last_index = points.len() - 1;
    let mut out = Vec::with_capacity(target);
    for index in 0..target - 1 {
        let source_index = ((index as f64 * step) as usize).min(last_index);
        out.push(points[source_index].clone());
    }
    out.push(points[last_index].clone());
    out
}

// ===== Tauri 命令（async 包装）=====
// 取数 = 多库 SQLite 只读查询（与请求日志页同量级），同步命令会跑在主线程
// 冻结托盘事件，统一 async + spawn_blocking 卸载到阻塞线程池。

/// 实时会话账本（近 `window_hours` 小时，缺省 6 小时）
#[tauri::command]
pub async fn get_live_sessions(window_hours: Option<i64>) -> Result<SessionFeed, String> {
    tauri::async_runtime::spawn_blocking(move || collect_live_sessions(window_hours))
        .await
        .map_err(|e| format!("实时会话任务失败: {e}"))?
}

/// 单会话上下文水位序列
#[tauri::command]
pub async fn get_session_context(
    source: String,
    session_id: String,
    hours: Option<i64>,
) -> Result<SessionContext, String> {
    tauri::async_runtime::spawn_blocking(move || {
        collect_session_context(&source, &session_id, hours)
    })
    .await
    .map_err(|e| format!("会话上下文任务失败: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// included 源（input 已含缓存读）：prompt = input + cache_write
    #[test]
    fn prompt_tokens_included_source() {
        assert_eq!(prompt_tokens_of("zcode", 1_000, 600, 100), 1_100);
        assert_eq!(prompt_tokens_of("codex", 1_000, 0, 0), 1_000);
    }

    /// separate 源（claude / kimi）：input 不含缓存读写，三项相加
    #[test]
    fn prompt_tokens_separate_source() {
        assert_eq!(prompt_tokens_of("claude", 400, 600, 100), 1_100);
        assert_eq!(prompt_tokens_of("kimi", 0, 900, 0), 900);
    }

    /// 脏数据（负值）按 0 计，不把水位算成负数
    #[test]
    fn prompt_tokens_clamps_negative() {
        assert_eq!(prompt_tokens_of("zcode", -5, -1, -2), 0);
        assert_eq!(prompt_tokens_of("claude", -5, 30, -2), 30);
    }

    /// 压缩判定：较上一轮降 45% 以上且上一轮 ≥ 10k 才算
    #[test]
    fn compaction_threshold() {
        assert!(is_compaction(100_000, 40_000));
        assert!(is_compaction(10_000, 1_000));
        // 只掉 40%：正常波动，不算压缩
        assert!(!is_compaction(100_000, 60_000));
        // 上一轮太小：换会话 / 首轮都会掉，不当作压缩
        assert!(!is_compaction(9_999, 100));
        assert!(!is_compaction(0, 0));
    }

    /// 模型窗口只认内置表里的已知档位，未知模型不臆造上限
    #[test]
    fn context_limit_table() {
        assert_eq!(context_limit_of("claude-sonnet-4-5-20250929"), Some(200_000));
        assert_eq!(context_limit_of("gpt-5.1-codex"), Some(272_000));
        assert_eq!(context_limit_of("glm-4.6"), Some(200_000));
        assert_eq!(context_limit_of("some-new-model"), None);
    }

    /// 超长会话等距抽样：保首尾、按目标条数收敛
    #[test]
    fn downsample_keeps_ends() {
        let points: Vec<ContextPoint> = (0..1_000)
            .map(|index| ContextPoint {
                ts: index,
                prompt_tokens: index,
                output_tokens: 1,
                cache_ratio: None,
                model_id: "m".to_string(),
            })
            .collect();
        let sampled = downsample(points, 100);
        assert_eq!(sampled.len(), 100);
        assert_eq!(sampled.first().unwrap().ts, 0);
        assert_eq!(sampled.last().unwrap().ts, 999);
    }
}