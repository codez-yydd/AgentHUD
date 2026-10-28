//! Shared session-tree and request-speed primitives.
//!
//! The usage feed, the session HUD, and their two front ends must agree on
//! both the set of sessions being counted and the meaning of a speed value.
//! This module is deliberately independent of either renderer so that a
//! rollout hint can never turn into a confirmed token count and so that a
//! short request is not silently divided by an arbitrary one-second floor.
//! Sub-200ms windows are rejected outright (timing jitter, never inflated
//! to a fake rate) and windows beyond one hour are treated as hung or
//! leaking timers and dropped — see `MIN_GEN_MS` / `MAX_GEN_MS`.
//!
//! Speed measures model requests, never a whole agent turn. Tool execution
//! between requests is excluded. Tool-call arguments are model output and
//! remain in output_tokens; tool results belong to a later request's input.
//! Unknown reasoning timing uses a conservative request average, marked ≈.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpeedQuality {
    Generation,
    RequestAverage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpeedSnapshot {
    pub value: f64,
    pub quality: SpeedQuality,
    pub completed_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpeedState {
    Measuring,
    Recent,
    Unavailable,
}

/// 待处理用户消息（pu）的新鲜期：一条尚无完成轮的 user 消息只在这段
/// 时间内可以独立支撑"等待首请求"的状态；超期后除非该会话树内还有
/// 可复核的活跃证据（进行中轮等），否则按空闲/不可用处理，防止数小时
/// 前的孤儿消息永久冒充活跃轮。取 10 分钟与 usage_feed::RUN_WINDOW_MS /
/// TOOL_WINDOW_MS 的"无活动即不再认为运行"口径一致；这绝不是给所有轮
/// 加硬截止——真实耗时很长的首请求只要会话树内仍有进行中请求（runs）
/// 就继续保留等待。
pub(crate) const PENDING_FRESH_MS: i64 = 10 * 60 * 1000;

/// 样本生成窗口有效下限（参考项目 zcode-token-usage-statusbar 口径：
/// 200ms~1h 之间才视为可信生成窗口）。注意与模块头"不给短请求硬造一秒
/// 地板"立场的区别：1 秒地板是把过短窗口人为放大到 1000ms 伪造速率值
///（篡改样本分母）；本下限是把 <200ms 的计时抖动/缓存命中伪请求整体
/// 拒收——不产出速度、也不另造数值，是"拒绝坏样本"而非"改写分母"。
pub(crate) const MIN_GEN_MS: i64 = 200;

/// 样本生成窗口有效上限（见 MIN_GEN_MS）：超过 1 小时的"生成窗口"几乎
/// 必然是挂死请求或计时泄漏，保留会把均速分母拖成无意义的低速。应用于
/// generation 与 request_average 两档（request_average 的近似窗口同受
/// 边界约束）。
pub(crate) const MAX_GEN_MS: i64 = 3_600_000;

/// `model_usage` is written as a completed-request table in older schemas,
/// where `status` is absent. Empty/null status has the same compatibility
/// meaning; explicit non-terminal statuses must not produce a speed.
pub(crate) fn is_completed_status(status: Option<&str>) -> bool {
    status
        .map(|value| value.is_empty() || value == "completed" || value == "success")
        .unwrap_or(true)
}

/// Timing fields for one completed model request.
///
/// `completed_at` is the actual completion timestamp when the source has
/// one. `duration_ms` is only an explicitly approximate fallback; it is never
/// used to manufacture a generation speed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RequestTiming {
    pub output_tokens: i64,
    pub started_at: Option<i64>,
    pub first_token_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub request_id: Option<String>,
    /// Reasoning is already included in output; this is an uncertainty signal,
    /// never an extra term in the numerator.
    pub reasoning_tokens: Option<i64>,
    /// Claude may omit reasoning counts; keep its timing conservatively marked
    /// as a request average rather than assuming a provider's stream semantics.
    pub model_id: Option<String>,
}

/// Without provider event guarantees, reasoning may precede the recorded
/// first token. Model names are only a conservative uncertainty signal.
fn has_uncertain_reasoning(request: &RequestTiming) -> bool {
    request.reasoning_tokens.is_some_and(|n| n > 0)
        || request.model_id.as_deref().is_some_and(|m| m.to_ascii_lowercase().contains("claude"))
}

/// Compute a speed without applying a minimum duration or a stale-value
/// fallback. This form is useful for already validated historical rows.
#[cfg(test)]
pub(crate) fn request_speed(request: &RequestTiming) -> Option<SpeedSnapshot> {
    request_speed_at(request, 0)
}

/// Compute a speed and reject timestamps that are in the future relative to
/// `observed_at_ms` (when non-zero).
pub(crate) fn request_speed_at(
    request: &RequestTiming,
    observed_at_ms: i64,
) -> Option<SpeedSnapshot> {
    if request.output_tokens <= 0 {
        return None;
    }

    let started = request.started_at;
    if started.is_some_and(|value| value <= 0) {
        return None;
    }
    let first = request.first_token_at;
    if first.is_some_and(|value| value <= 0) {
        return None;
    }
    let completed = request.completed_at;
    if completed.is_some_and(|value| value <= 0) {
        return None;
    }

    if let Some(observed) = (observed_at_ms > 0).then_some(observed_at_ms) {
        if first.is_some_and(|value| value > observed)
            || completed.is_some_and(|value| value > observed)
        {
            return None;
        }
    }

    if let Some(started) = started {
        if first.is_some_and(|value| value < started)
            || completed.is_some_and(|value| value <= started)
        {
            return None;
        }
    }

    // A real generation speed needs the first-token and completion times.
    // `started_at` is useful for validating the row when present, but is not
    // required because a few schema versions only recorded the two absolute
    // events.
    if let (Some(first), Some(completed)) = (first, completed) {
        if completed <= first {
            // Both event timestamps were supplied but are reversed: this row is
            // invalid even if a duration fallback happens to be present.
            return None;
        }
        // Reasoning placement is unknown: the full request window is a
        // conservative average, never an exact generation measurement.
        let (span, quality) = if has_uncertain_reasoning(request) {
            (completed.checked_sub(started?)?, SpeedQuality::RequestAverage)
        } else {
            (completed - first, SpeedQuality::Generation)
        };
        let value = finite_rate(request.output_tokens, span)?;
        return Some(SpeedSnapshot {
            value,
            quality,
            completed_at: completed,
            request_id: request.request_id.clone(),
        });
    }

    // A first-token timestamp without a completion timestamp is incomplete;
    // do not silently turn it into an average.
    if first.is_some() && completed.is_none() {
        return None;
    }

    // No first-token timestamp: a request-average speed is allowed. Prefer
    // the real absolute request span when both endpoints are present; use an
    // explicit duration only when one endpoint is unavailable.
    let duration = started
        .zip(completed)
        .and_then(|(started, completed)| completed.checked_sub(started))
        .filter(|value| *value > 0)
        .or_else(|| request.duration_ms.filter(|value| *value > 0))?;
    if let (Some(started), Some(completed)) = (started, completed) {
        if completed <= started {
            return None;
        }
    }

    let effective_completed = match (started, completed) {
        (_, Some(completed)) => completed,
        (Some(started), None) => started.checked_add(duration)?,
        // A polling timestamp is only an observation boundary. It is not a
        // completion event and must not manufacture a speed for a row that
        // has neither an absolute completion time nor a start time.
        (None, None) => return None,
    };
    if observed_at_ms > 0 && effective_completed > observed_at_ms {
        return None;
    }
    let value = finite_rate(request.output_tokens, duration)?;
    Some(SpeedSnapshot {
        value,
        quality: SpeedQuality::RequestAverage,
        completed_at: effective_completed,
        request_id: request.request_id.clone(),
    })
}

/// A source-recorded request or stream duration; never subtract TTFT here.
/// Some importers timestamp the usage event at completion, so this path does
/// not synthesize completion by adding duration to that event timestamp.
pub(crate) fn duration_speed(
    output_tokens: i64, duration_ms: f64, quality: SpeedQuality, completed_at: i64,
) -> Option<SpeedSnapshot> {
    if output_tokens <= 0 || !duration_ms.is_finite()
        || duration_ms < MIN_GEN_MS as f64 || duration_ms > MAX_GEN_MS as f64 {
        return None;
    }
    let value = output_tokens as f64 * 1000.0 / duration_ms;
    (value.is_finite() && value > 0.0).then_some(SpeedSnapshot {
        value, quality, completed_at, request_id: None,
    })
}

/// DSH records a stream span; other importers record whole-request durations.
pub(crate) fn duration_quality(source: &str) -> SpeedQuality {
    if source == "dsh" { SpeedQuality::Generation } else { SpeedQuality::RequestAverage }
}

fn finite_rate(output_tokens: i64, span_ms: i64) -> Option<f64> {
    // 样本时长边界（generation 与 request_average 两档共用）：极短窗口
    //（<MIN_GEN_MS）多为计时抖动/缓存命中伪请求，极长窗口（>MAX_GEN_MS）
    // 多为挂死/计时泄漏——整体拒收，不放大也不截断分母（与"不给短请求
    // 硬造一秒地板"的区别见 MIN_GEN_MS 注释）。
    if !(MIN_GEN_MS..=MAX_GEN_MS).contains(&span_ms) {
        return None;
    }
    let value = output_tokens as f64 * 1000.0 / span_ms as f64;
    value.is_finite().then_some(value).filter(|value| *value > 0.0)
}

/// Validated per-request sample. Both numerator and denominator must be
/// accumulated together; a turn's wall-clock duration is never a substitute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GenerationSample {
    pub output_tokens: i64,
    pub generation_ms: i64,
    pub tps: f64,
    pub quality: SpeedQuality,
}

pub(crate) fn speed_sample(request: &RequestTiming, observed_at_ms: i64) -> Option<GenerationSample> {
    let snapshot = request_speed_at(request, observed_at_ms)?;
    let span = if snapshot.quality == SpeedQuality::Generation {
        request.completed_at?.checked_sub(request.first_token_at?)?
    } else {
        request.started_at.zip(request.completed_at)
            .and_then(|(s,c)| c.checked_sub(s)).filter(|ms| *ms > 0)
            .or(request.duration_ms)?
    };
    Some(GenerationSample { output_tokens: request.output_tokens,
        generation_ms: span, tps: snapshot.value, quality: snapshot.quality })
}

#[cfg(test)]
pub(crate) fn generation_sample(request: &RequestTiming, observed_at_ms: i64) -> Option<GenerationSample> {
    speed_sample(request, observed_at_ms).filter(|s| s.quality == SpeedQuality::Generation)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionTree {
    pub root: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SessionTreeIndex {
    has_parent: bool,
    parent_of: BTreeMap<String, String>,
    children: BTreeMap<String, Vec<String>>,
}

impl SessionTreeIndex {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn has_parent(&self) -> bool {
        self.has_parent
    }

    pub(crate) fn root_for(&self, session_id: &str) -> String {
        if !self.has_parent {
            return session_id.to_string();
        }

        let mut path = Vec::new();
        let mut position = BTreeMap::<String, usize>::new();
        let mut current = session_id.to_string();
        loop {
            if let Some(&start) = position.get(&current) {
                // A malformed cycle has no natural root. Choose a stable
                // canonical member so every entry point reports the same
                // tree instead of looping or double-counting unpredictably.
                return path[start..]
                    .iter()
                    .min()
                    .cloned()
                    .unwrap_or(current);
            }
            position.insert(current.clone(), path.len());
            path.push(current.clone());
            let Some(parent) = self.parent_of.get(&current) else {
                return current;
            };
            current = parent.clone();
        }
    }

    pub(crate) fn members(&self, root: &str) -> Vec<String> {
        let mut members = Vec::new();
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([root.to_string()]);
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current.clone()) {
                continue;
            }
            members.push(current.clone());
            if let Some(children) = self.children.get(&current) {
                queue.extend(children.iter().cloned());
            }
        }
        members
    }

    pub(crate) fn tree(&self, root: &str) -> SessionTree {
        SessionTree {
            root: root.to_string(),
            members: self.members(root),
        }
    }

    #[cfg(test)]
    fn from_relations(relations: &[(&str, Option<&str>)]) -> Self {
        let mut index = Self {
            has_parent: true,
            ..Self::default()
        };
        for (id, parent) in relations {
            if let Some(parent) = parent.filter(|value| !value.is_empty()) {
                index
                    .parent_of
                    .insert((*id).to_string(), (*parent).to_string());
                index
                    .children
                    .entry((*parent).to_string())
                    .or_default()
                    .push((*id).to_string());
            }
        }
        index
    }
}

/// Load the complete session relation once. Missing tables/columns deliberately
/// return an empty legacy index, which means every requested session is its own
/// tree and preserves old-database behavior.
pub(crate) fn load_session_tree_index(conn: &Connection) -> Result<SessionTreeIndex, String> {
    let has_session = table_exists(conn, "session")
        && crate::db::has_column(conn, "session", "id")
        && crate::db::has_column(conn, "session", "parent_id");
    if !has_session {
        return Ok(SessionTreeIndex::empty());
    }

    let mut index = SessionTreeIndex {
        has_parent: true,
        ..SessionTreeIndex::default()
    };
    let mut stmt = conn
        .prepare("SELECT id, parent_id FROM session")
        .map_err(|error| format!("准备会话关系查询失败: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .map_err(|error| format!("读取会话关系失败: {error}"))?;
    for row in rows {
        let (Some(id), parent) = row.map_err(|error| format!("读取会话关系失败: {error}"))? else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        if let Some(parent) = parent.filter(|value| !value.is_empty()) {
            index.parent_of.insert(id.clone(), parent.clone());
            index.children.entry(parent).or_default().push(id);
        }
    }
    for children in index.children.values_mut() {
        children.sort();
        children.dedup();
    }
    Ok(index)
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [table],
        |_| Ok(()),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_execution_wait_is_excluded_from_request_speed() {
        let requests = [RequestTiming {
            output_tokens: 100, started_at: Some(1_000), first_token_at: Some(2_000),
            completed_at: Some(3_000), ..Default::default()
        }, RequestTiming {
            // A tool runs for 60 seconds between these two model requests.
            output_tokens: 100, started_at: Some(63_000), first_token_at: Some(64_000),
            completed_at: Some(65_000), ..Default::default()
        }];
        let samples: Vec<_> = requests.iter().filter_map(|r| speed_sample(r, 100_000)).collect();
        let output: i64 = samples.iter().map(|s| s.output_tokens).sum();
        let span: i64 = samples.iter().map(|s| s.generation_ms).sum();
        assert_eq!((output, span), (200, 2_000));
        assert_eq!(output as f64 * 1000.0 / span as f64, 100.0);
        assert!(duration_speed(90, 1.0, SpeedQuality::Generation, 0).is_none());
        assert!(duration_speed(0, 1000.0, SpeedQuality::Generation, 0).is_none());
        assert!(duration_speed(90, f64::INFINITY, SpeedQuality::Generation, 0).is_none());
    }

    #[test]
    fn generation_speed_uses_exact_short_span_without_floor() {
        // 250ms 短窗口按真实时长计速：50 tok → 200 t/s。1 秒地板仍不存在
        //（窗口不被放大）；<200ms 的抖动样本由 MIN_GEN_MS 整体拒收，见
        // 思考与边界专项测试。
        let request = RequestTiming {
            output_tokens: 50,
            started_at: Some(1_000),
            first_token_at: Some(1_010),
            completed_at: Some(1_260),
            ..RequestTiming::default()
        };
        let speed = request_speed(&request).expect("valid generation speed");
        assert_eq!(speed.quality, SpeedQuality::Generation);
        assert_eq!(speed.completed_at, 1_260);
        assert!((speed.value - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn request_average_is_explicitly_approximate() {
        let request = RequestTiming {
            output_tokens: 100,
            started_at: Some(2_000),
            duration_ms: Some(200),
            ..RequestTiming::default()
        };
        let speed = request_speed_at(&request, 2_500).expect("valid average speed");
        assert_eq!(speed.quality, SpeedQuality::RequestAverage);
        assert_eq!(speed.completed_at, 2_200);
        assert!((speed.value - 500.0).abs() < f64::EPSILON);

        let from_absolute_request_times = RequestTiming {
            output_tokens: 100,
            started_at: Some(2_000),
            completed_at: Some(2_200),
            ..RequestTiming::default()
        };
        let speed = request_speed(&from_absolute_request_times).expect("derived request duration");
        assert_eq!(speed.quality, SpeedQuality::RequestAverage);
        assert_eq!(speed.completed_at, 2_200);
        assert!((speed.value - 500.0).abs() < f64::EPSILON);

        let absolute_times_override_conflicting_duration = RequestTiming {
            output_tokens: 100,
            started_at: Some(2_000),
            completed_at: Some(2_200),
            duration_ms: Some(50),
            ..RequestTiming::default()
        };
        let speed = request_speed(&absolute_times_override_conflicting_duration)
            .expect("absolute request span remains authoritative");
        assert!((speed.value - 500.0).abs() < f64::EPSILON);
    }

    #[test]
    fn invalid_timing_never_becomes_zero_or_stale_speed() {
        let reversed = RequestTiming {
            output_tokens: 100,
            first_token_at: Some(3_000),
            completed_at: Some(2_999),
            duration_ms: Some(100),
            ..RequestTiming::default()
        };
        assert!(request_speed(&reversed).is_none());

        let future = RequestTiming {
            output_tokens: 100,
            started_at: Some(4_000),
            duration_ms: Some(100),
            ..RequestTiming::default()
        };
        assert!(request_speed_at(&future, 4_050).is_none());

        let zero = RequestTiming {
            output_tokens: 0,
            ..RequestTiming::default()
        };
        assert!(request_speed(&zero).is_none());

        let first_before_start = RequestTiming {
            output_tokens: 100,
            started_at: Some(5_000),
            first_token_at: Some(4_999),
            completed_at: Some(6_000),
            ..RequestTiming::default()
        };
        assert!(request_speed(&first_before_start).is_none());

        let first_without_completion = RequestTiming {
            output_tokens: 100,
            started_at: Some(5_000),
            first_token_at: Some(5_100),
            duration_ms: Some(900),
            ..RequestTiming::default()
        };
        assert!(request_speed(&first_without_completion).is_none());

        let duration_without_timestamps = RequestTiming {
            output_tokens: 100,
            duration_ms: Some(100),
            ..RequestTiming::default()
        };
        assert!(request_speed_at(&duration_without_timestamps, 9_999).is_none());
    }

    #[test]
    fn generation_sample_accepts_short_and_fast_but_rejects_invalid_rows() {
        // 真实输出 1000 Token、首 Token 到完成 8 秒：可信 125 t/s——TTFT 再长
        // （92 秒）也不改用旧"总耗时−TTFT/兜底"公式。
        let long_ttft = RequestTiming {
            output_tokens: 1000,
            started_at: Some(1_000),
            first_token_at: Some(93_000),
            completed_at: Some(101_000),
            ..RequestTiming::default()
        };
        let sample = generation_sample(&long_ttft, 200_000).expect("长 TTFT 样本仍可信");
        assert_eq!(sample.generation_ms, 8_000);
        assert!((sample.tps - 125.0).abs() < f64::EPSILON);

        // 合法 250ms 短样本与 >500 t/s 高速样本不因"速度阈值"被丢弃
        //（500 tok / 250ms = 2000 t/s）；<200ms 的窗口由样本时长边界整体
        // 拒收（见思考与边界专项测试）。
        let short = RequestTiming {
            output_tokens: 500,
            started_at: Some(1_000),
            first_token_at: Some(1_010),
            completed_at: Some(1_260),
            ..RequestTiming::default()
        };
        let sample = generation_sample(&short, 2_000).expect("250ms 合法短样本");
        assert_eq!(sample.generation_ms, 250);
        assert!((sample.tps - 2_000.0).abs() < f64::EPSILON);

        // 逆序（completed <= first）：无样本。
        let reversed = RequestTiming {
            output_tokens: 100,
            first_token_at: Some(3_000),
            completed_at: Some(2_999),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&reversed, 9_999).is_none());

        // 未来时刻（晚于观测时刻）：无样本。
        let future = RequestTiming {
            output_tokens: 100,
            started_at: Some(4_000),
            first_token_at: Some(4_100),
            completed_at: Some(4_200),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&future, 4_100).is_none());

        // first 早于 started：无样本。
        let before_start = RequestTiming {
            output_tokens: 100,
            started_at: Some(5_000),
            first_token_at: Some(4_999),
            completed_at: Some(6_000),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&before_start, 9_999).is_none());

        // 零输出：无样本。
        let zero_output = RequestTiming {
            output_tokens: 0,
            started_at: Some(5_000),
            first_token_at: Some(5_100),
            completed_at: Some(6_000),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&zero_output, 9_999).is_none());

        // 首 Token 缺失：只能算 request_average，绝不算生成样本。
        let no_first = RequestTiming {
            output_tokens: 100,
            started_at: Some(5_000),
            completed_at: Some(6_000),
            duration_ms: Some(1_000),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&no_first, 9_999).is_none());
        assert_eq!(
            request_speed_at(&no_first, 9_999).map(|speed| speed.quality),
            Some(SpeedQuality::RequestAverage)
        );
    }

    #[test]
    fn 思考段生成窗口前移_理由token与claude系按名识别() {
        // reasoning_tokens > 0（任意模型）：思考段在首字之前流出且已计入
        // output_tokens，生成窗口前移为 completed − started（100s），绝不
        // 按首字窗口（8s）虚高成 125 t/s。1000 tok / 100s = 10 t/s。
        let reasoning = RequestTiming {
            output_tokens: 1000,
            started_at: Some(1_000),
            first_token_at: Some(93_000),
            completed_at: Some(101_000),
            reasoning_tokens: Some(80_000),
            model_id: Some("glm-5.3".to_string()),
            ..RequestTiming::default()
        };
        let speed = request_speed(&reasoning).expect("思考样本有效");
        assert_eq!(speed.quality, SpeedQuality::RequestAverage);
        assert!((speed.value - 10.0).abs() < f64::EPSILON, "{speed:?}");
        let sample = speed_sample(&reasoning, 200_000).expect("思考样本进聚合池");
        assert_eq!(sample.generation_ms, 100_000, "聚合分母与速度分母同源");
        assert!((sample.tps - 10.0).abs() < f64::EPSILON);

        // Claude 系列：reasoning 恒 0（甚至缺列）但 output 已含思考，按
        // 模型名（含 "claude"，大小写不敏感）识别，同样前移窗口。
        let claude = RequestTiming {
            output_tokens: 1000,
            started_at: Some(1_000),
            first_token_at: Some(93_000),
            completed_at: Some(101_000),
            reasoning_tokens: Some(0),
            model_id: Some("Claude-Opus-5.5".to_string()),
            ..RequestTiming::default()
        };
        let speed = request_speed(&claude).expect("claude 思考样本有效");
        assert!((speed.value - 10.0).abs() < f64::EPSILON, "{speed:?}");

        // 非思考模型（无 reasoning、模型名不含 claude）：不受影响，维持
        // completed − first 首字窗口（8s → 125 t/s）。
        let plain = RequestTiming {
            output_tokens: 1000,
            started_at: Some(1_000),
            first_token_at: Some(93_000),
            completed_at: Some(101_000),
            reasoning_tokens: None,
            model_id: Some("GLM-5.3".to_string()),
            ..RequestTiming::default()
        };
        let speed = request_speed(&plain).expect("非思考样本有效");
        assert!((speed.value - 125.0).abs() < f64::EPSILON, "{speed:?}");
        let sample = generation_sample(&plain, 200_000).expect("非思考样本进聚合池");
        assert_eq!(sample.generation_ms, 8_000);
    }

    #[test]
    fn 思考段缺started退回首字窗口_时长越界样本整体拒绝() {
        // started 缺失：含思考段也退回 completed − first 首字窗口，照常
        // 产出，不因此丢样本。1000 tok / 100s = 10 t/s。
        let no_started = RequestTiming {
            output_tokens: 1000,
            first_token_at: Some(1_000),
            completed_at: Some(101_000),
            reasoning_tokens: Some(50_000),
            ..RequestTiming::default()
        };
        assert!(speed_sample(&no_started, 200_000).is_none(), "unknown reasoning start cannot manufacture a precise rate");

        // <200ms（MIN_GEN_MS 之下）：计时抖动样本整体拒绝——不产出速度，
        // 也不降级为 request_average 把同一段时长重新放行。
        let jitter = RequestTiming {
            output_tokens: 50,
            started_at: Some(1_000),
            first_token_at: Some(1_010),
            completed_at: Some(1_020),
            ..RequestTiming::default()
        };
        assert!(request_speed(&jitter).is_none(), "<200ms 不得产出速度");
        assert!(generation_sample(&jitter, 9_999).is_none());

        // >1h（MAX_GEN_MS 之上）：挂死/计时泄漏样本整体拒绝。生成窗口按
        // 首 Token 窗口（completed − first）越界判定：非思考请求窗口为
        // completed − first = 3_600_001ms。
        let hung = RequestTiming {
            output_tokens: 5_000_000,
            started_at: Some(1_000),
            first_token_at: Some(2_000),
            completed_at: Some(2_000 + 3_600_001),
            ..RequestTiming::default()
        };
        assert!(request_speed(&hung).is_none(), ">1h 不得产出速度");
        assert!(generation_sample(&hung, 9_999_999).is_none());

        // request_average 同受边界约束：<200ms 与 >1h 的近似窗口均拒绝。
        let approx_jitter = RequestTiming {
            output_tokens: 100,
            started_at: Some(2_000),
            completed_at: Some(2_100),
            ..RequestTiming::default()
        };
        assert!(request_speed(&approx_jitter).is_none());
        let approx_hung = RequestTiming {
            output_tokens: 100,
            started_at: Some(1),
            completed_at: Some(1 + 3_600_001),
            ..RequestTiming::default()
        };
        assert!(request_speed(&approx_hung).is_none());

        // 边界含端点：恰 200ms 与恰 1h 仍为有效样本。
        let min_edge = RequestTiming {
            output_tokens: 100,
            started_at: Some(1_000),
            first_token_at: Some(1_000),
            completed_at: Some(1_200),
            ..RequestTiming::default()
        };
        let speed = request_speed(&min_edge).expect("200ms 边界样本有效");
        assert!((speed.value - 500.0).abs() < f64::EPSILON);
        let max_edge = RequestTiming {
            output_tokens: 3_600_000,
            started_at: Some(1_000),
            first_token_at: Some(1_000),
            completed_at: Some(1_000 + 3_600_000),
            ..RequestTiming::default()
        };
        let speed = request_speed(&max_edge).expect("1h 边界样本有效");
        assert!((speed.value - 1_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn deep_tree_and_cycle_are_finite_and_stable() {
        let index = SessionTreeIndex::from_relations(&[
            ("root", None),
            ("a", Some("root")),
            ("b", Some("a")),
            ("c", Some("b")),
        ]);
        assert_eq!(index.root_for("c"), "root");
        assert_eq!(index.members("root"), ["root", "a", "b", "c"]);

        let cycle = SessionTreeIndex::from_relations(&[
            ("z", Some("y")),
            ("y", Some("x")),
            ("x", Some("y")),
        ]);
        assert_eq!(cycle.root_for("x"), "x");
        assert_eq!(cycle.root_for("y"), "x");
        assert_eq!(cycle.members("x"), ["x", "y", "z"]);
    }

    #[test]
    fn legacy_index_keeps_each_session_separate() {
        let index = SessionTreeIndex::empty();
        assert!(!index.has_parent());
        assert_eq!(index.root_for("child"), "child");
        assert_eq!(index.members("child"), ["child"]);
    }
}
