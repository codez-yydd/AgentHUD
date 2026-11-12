//! 额度重置提醒：额度窗口（5 小时 / 每周 / MCP 月度）临近重置时弹一次系统通知。
//!
//! # 为什么不需要后台轮询
//!
//! 重置时刻是**绝对时间**：`quota::fetch_quota` 每次成功查询都会把三个窗口的
//! `next_reset_time` 登记进来（面板打开时 30 秒一轮），登记一次就能一直用到那个点。
//! 所以后台只需要一个 30 秒线程读内存里的登记表——**不额外发任何网络请求**，
//! 配额查询的频率与行为完全不变。
//!
//! # 去重与过期
//!
//! - 同一 `(窗口, 重置时刻)` 只通知一次；窗口刷新出新的 reset 时刻后自然重新武装；
//! - 用量为 0 的窗口不登记：「一次都没用过」的窗口重置提醒只是噪音；
//! - 重置时刻过去超过 6 小时的登记项与已通知键一起回收，表不会无限增长；
//! - 进程重启后登记表清空，等下一次额度查询重新登记（最坏错过一次提醒，
//!   不会重复轰炸）。
//!
//! 目前只登记 ZCode Coding Plan 的三个窗口；Claude / Codex 的额度窗口在各自
//! 模块里，将来要一并提醒时只需在它们的取数成功后调一次 [`record_entries`]。
//!
//! 通知正文带**归属**（「ZCode · GLM Coding Plan Pro · 5 小时额度」）与**相对
//! 日期**（今天 / 明天 HH:MM）：只写「5 小时额度将在 12:30 重置」时，同时开着
//! 多家订阅的用户无法判断是哪个 Agent 的哪个窗口。

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use chrono::TimeZone;

/// 提前多久提醒（毫秒）
const LEAD_MS: i64 = 5 * 60 * 1000;
/// 后台检查间隔（毫秒）：30 秒一轮，提醒时刻误差不超过半个 tick
const TICK_MS: u64 = 30 * 1000;
/// 登记项保留宽限：重置时刻过去这么久后就不再保留（防表无限增长）
const STALE_MS: i64 = 6 * 60 * 60 * 1000;

/// 一个待提醒的额度窗口。
///
/// pub(crate)：`record_entries` 是对其他模块开放的统一登记入口（Claude / Codex
/// 将来接入时直接构造条目），所以类型本身也要同可见性。
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// 窗口标识（同一窗口的多次登记互相覆盖；跨 provider 用前缀区分）
    pub(crate) key: String,
    /// 归属说人话：哪个 Agent 的哪份订阅（如「ZCode · GLM Coding Plan Pro」）
    pub(crate) source: String,
    /// 通知文案里的窗口名（如「5 小时额度」）
    pub(crate) label: String,
    /// 下次重置时刻（毫秒）
    pub(crate) reset_ms: i64,
}

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    /// 已通知过的 `(窗口, 重置时刻)`：`key@reset_ms`
    notified: HashSet<String>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn key_of(key: &str, reset_ms: i64) -> String {
    format!("{key}@{reset_ms}")
}

/// 重置时刻的本地时钟文案（今天 / 明天 + HH:MM）：通知正文里给人看
fn clock_text(ms: i64) -> String {
    use chrono::Datelike;
    let Some(local) = chrono::Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let time = local.format("%H:%M").to_string();
    let days = (local.date_naive() - chrono::Local::now().date_naive()).num_days();
    match days {
        0 => format!("今天 {time}"),
        1 => format!("明天 {time}"),
        _ => format!("{}月{}日 {time}", local.month(), local.day()),
    }
}

/// 判断一个窗口是否值得登记：有重置时刻、用量非 0、且还没重置
fn push(
    entries: &mut Vec<Entry>,
    source: &str,
    key: &str,
    label: &str,
    reset: Option<i64>,
    percentage: u32,
) {
    let Some(reset_ms) = reset else {
        return;
    };
    if percentage == 0 || reset_ms <= now_ms() {
        return;
    }
    entries.retain(|entry| entry.key != key);
    entries.push(Entry {
        key: key.to_string(),
        source: source.to_string(),
        label: label.to_string(),
        reset_ms,
    });
}

/// 套餐档位的显示名：接口 `level`（lite/pro/max/ultra）→「GLM Coding Plan Xxx」。
/// 缺档位（旧接口 / 空串）时只说产品名；未知档位首字母大写后原样附上。
fn plan_label(level: &str) -> String {
    let level = level.trim();
    if level.is_empty() {
        return "GLM Coding Plan".to_string();
    }
    let tier = match level.to_ascii_lowercase().as_str() {
        "lite" => "Lite".to_string(),
        "pro" => "Pro".to_string(),
        "max" => "Max".to_string(),
        "ultra" => "Ultra".to_string(),
        _ => {
            let mut chars = level.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    };
    format!("GLM Coding Plan {tier}")
}

/// 已通知键是否还需要保留：解析出重置时刻，落在宽限期内就留着
fn notified_still_useful(key: &str, now: i64) -> bool {
    key.rsplit_once('@')
        .and_then(|(_, ms)| ms.parse::<i64>().ok())
        .map(|ms| ms > now - STALE_MS)
        .unwrap_or(false)
}

/// 登记一批窗口的下次重置时刻（同 key 覆盖为最新值，顺手回收过期项）
pub(crate) fn record_entries(entries: Vec<Entry>) {
    let Ok(mut guard) = state().lock() else {
        return;
    };
    let now = now_ms();
    guard.entries.retain(|entry| entry.reset_ms > now - STALE_MS);
    for entry in entries {
        guard.entries.retain(|existing| existing.key != entry.key);
        guard.entries.push(entry);
    }
    guard
        .notified
        .retain(|key| notified_still_useful(key, now));
}

/// 登记 ZCode 额度三个窗口的重置时刻（quota::fetch_quota 成功查询后调用）。
///
/// 通知里要能看出「哪个 Agent 的哪份订阅」，所以每条都带
/// `source = "ZCode · GLM Coding Plan {档位}"`——只写「5 小时额度将在 12:30
/// 重置」用户没法判断是哪个产品的窗口。
pub(crate) fn record(result: &crate::quota::QuotaResult) {
    let source = format!("ZCode · {}", plan_label(&result.level));
    let mut entries: Vec<Entry> = Vec::new();
    if let Some(hour5) = &result.hour5 {
        push(
            &mut entries,
            &source,
            "zcode:hour5",
            "5 小时额度",
            hour5.next_reset_time,
            hour5.percentage,
        );
    }
    if let Some(weekly) = &result.weekly {
        push(
            &mut entries,
            &source,
            "zcode:weekly",
            "每周额度",
            weekly.next_reset_time,
            weekly.percentage,
        );
    }
    if let Some(mcp) = &result.mcp {
        push(
            &mut entries,
            &source,
            "zcode:mcp",
            "MCP 月度额度",
            mcp.next_reset_time,
            mcp.percentage,
        );
    }
    record_entries(entries);
}

/// 启动后台检查线程（setup 里调用一次，进程生命周期常驻）
pub(crate) fn spawn() {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(TICK_MS));
        check();
    });
}

/// 进入提前量窗口就通知（同一重置时刻只一次）
fn check() {
    let now = now_ms();
    let mut due: Vec<(String, String, i64)> = Vec::new();
    if let Ok(mut guard) = state().lock() {
        guard.entries.retain(|entry| entry.reset_ms > now - STALE_MS);
        // 先收集候选（不可变借用），再写 notified（可变借用），避免借用冲突
        let candidates: Vec<(String, String, String, i64)> = guard
            .entries
            .iter()
            .filter_map(|entry| {
                let remaining = entry.reset_ms - now;
                let key = key_of(&entry.key, entry.reset_ms);
                if remaining > 0 && remaining <= LEAD_MS && !guard.notified.contains(&key) {
                    Some((
                        key,
                        entry.source.clone(),
                        entry.label.clone(),
                        entry.reset_ms,
                    ))
                } else {
                    None
                }
            })
            .collect();
        for (key, source, label, reset_ms) in candidates {
            guard.notified.insert(key);
            due.push((source, label, reset_ms));
        }
    }
    for (source, label, reset_ms) in due {
        crate::notify(
            "AgentHUD · 额度即将重置".to_string(),
            format!(
                "{} · {}将在 {} 重置，之后可用新额度",
                source,
                label,
                clock_text(reset_ms)
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 登记规则：0% 不登记、已过时刻不登记、缺时刻不登记、同 key 覆盖
    #[test]
    fn push_skips_unused_and_past_windows() {
        let mut entries = Vec::new();
        let future = now_ms() + 3_600_000;
        let source = "ZCode · GLM Coding Plan Pro";
        push(&mut entries, source, "zcode:hour5", "5 小时额度", Some(future), 0);
        assert!(entries.is_empty(), "0% 的窗口不该登记");
        push(
            &mut entries,
            source,
            "zcode:hour5",
            "5 小时额度",
            Some(now_ms() - 1),
            50,
        );
        assert!(entries.is_empty(), "已经过去的重置时刻不该登记");
        push(&mut entries, source, "zcode:hour5", "5 小时额度", None, 50);
        assert!(entries.is_empty(), "没有重置时刻不该登记");
        push(
            &mut entries,
            source,
            "zcode:hour5",
            "5 小时额度",
            Some(future),
            50,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, source, "通知要能说出是哪个 Agent 的订阅");
        push(
            &mut entries,
            source,
            "zcode:hour5",
            "5 小时额度",
            Some(future + 1_000),
            50,
        );
        assert_eq!(entries.len(), 1, "同 key 覆盖而不是追加");
        assert_eq!(entries[0].reset_ms, future + 1_000);
    }

    /// 订阅名：档位映射 + 缺省兜底
    #[test]
    fn plan_label_maps_subscription_tiers() {
        assert_eq!(plan_label(""), "GLM Coding Plan");
        assert_eq!(plan_label("  "), "GLM Coding Plan");
        assert_eq!(plan_label("pro"), "GLM Coding Plan Pro");
        assert_eq!(plan_label(" MAX "), "GLM Coding Plan Max");
        assert_eq!(plan_label("ultra"), "GLM Coding Plan Ultra");
        // 未知档位首字母大写后原样附上（接口将来加档位不至于空白）
        assert_eq!(plan_label("team"), "GLM Coding Plan Team");
    }

    /// 已通知键按重置时刻过期（宽限期外回收）
    #[test]
    fn notified_keys_expire_after_stale_window() {
        let now = now_ms();
        assert!(notified_still_useful(&key_of("zcode:hour5", now + 1_000), now));
        assert!(!notified_still_useful(
            &key_of("zcode:hour5", now - STALE_MS - 1),
            now
        ));
        assert!(!notified_still_useful("garbage", now));
    }
}
