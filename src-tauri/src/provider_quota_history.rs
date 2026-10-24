//! 全量 provider 额度快照历史（方案 B：手机端零配置看全部 agent 额度）。
//!
//! 与 agent_quota_history（CLI 登录态通道：codex / claude / cursor / kimi）
//! 分开存储：本文件覆盖其余 11 个凭证型 provider（deepseek / moonshot /
//! minimax / opencodego / alibaba / qoder / longcat / mimo / stepfun / grok /
//! alibabatoken），把 provider_quota.rs 每轮查询的归一结果
//! （ProviderQuotaEntry）采样为快照追加到 ~/.agenthud/provider_quota_history.jsonl，
//! 供 sync.rs 按 `last_uploaded_provider_quota_ts` 游标增量上传到
//! agent_quota_snapshots 通道（与 CLI 快照混在同一请求字段里，由 source 区分）。
//!
//! 工程纪律（沿用 agent_quota_history.rs 的全部惯例）：
//! - jsonl 追加 + append 锁 + 尾部换行修复 + 90 天滚动清理 + 24h 清理节流；
//! - 同 source 同 label 同秒去重（多凭证按 label 区分，互不挤占）；
//! - secret 永不落快照：label 是凭证备注名、message 是不含 secret 片段的
//!   中文错误文案，与 ProviderQuotaEntry 的既有契约一致。
//!
//! 刻意排除的来源（保持既有通道边界，避免数据混淆）：
//! - gemini：纯本地 CLI 型（无凭证体系），不采集；
//! - codex / claude / cursor / kimi：额度已有 agent_quota_history 通道，
//!   claude / cursor / kimi 在 provider_quota 分发里的手动凭证形态不重复采集，
//!   否则同一 source 两路快照在服务端 / 手机端会互相覆盖展示。

use chrono::Local;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::agent_quota_history::AgentQuotaSnapshot;
use crate::pricing::config_dir;
use crate::provider_quota::{ProviderQuotaBalance, ProviderQuotaEntry, ProviderQuotaWindow};

const RETENTION_MS: i64 = 90 * 86_400_000;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// label / plan_name 截断上限（与服务端列口径一致）。
const NAME_LIMIT: usize = 64;
/// 错误原因截断上限（与服务端列口径一致）。
const MESSAGE_LIMIT: usize = 128;

static LAST_CLEANUP: OnceLock<Mutex<Option<SystemTime>>> = OnceLock::new();
static APPEND_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
/// 同 (source, label) 的最近采样秒，用于同秒去重。
static LAST_TS_BY_KEY: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();

/// 纳入 provider 快照采集的来源（与 server/db.py 的 AGENT_QUOTA_WINDOWS
/// proto 7 扩展清单保持一致；kimi 走 CLI 通道不在此列）。
pub const PROVIDER_SNAPSHOT_SOURCES: [&str; 11] = [
    "deepseek",
    "moonshot",
    "minimax",
    "opencodego",
    "alibaba",
    "qoder",
    "longcat",
    "mimo",
    "stepfun",
    "grok",
    "alibabatoken",
];

/// provider 是否纳入快照采集。
pub fn is_provider_snapshot_source(provider: &str) -> bool {
    PROVIDER_SNAPSHOT_SOURCES.contains(&provider)
}

pub fn history_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("provider_quota_history.jsonl"))
}

/// 把一轮查询的归一 entry 采样为快照并追加（每条凭证一条快照）。
///
/// - 全部状态都记录：ok / expired / error / pending（错误态带 message，
///   手机端可显示查询失败原因）；
/// - 窗口映射：ProviderQuotaWindow 的 key 已是各模块归一后的稳定键
///   （alibaba/opencodego 的 per5Hour→hour5、perWeek→weekly、
///   perBillMonth→monthly 等映射在各 provider 模块内完成），此处直接沿用；
///   无有效百分比的窗口（协议形状只有 key/used_pct/reset_at 三字段）
///   不计入；
/// - 余额型（windows 为空但有 balance / plan_name）与错误态（只有 message）
///   快照照常落盘，由服务端哨兵行承载。
pub fn record_entries(provider: &str, entries: &[ProviderQuotaEntry]) {
    if !is_provider_snapshot_source(provider) {
        return;
    }
    for entry in entries {
        let snapshot = entry_to_snapshot(provider, entry);
        append_snapshot(&snapshot);
    }
}

/// 单条 entry → 快照（纯函数，供单测覆盖映射规则）。
fn entry_to_snapshot(provider: &str, entry: &ProviderQuotaEntry) -> AgentQuotaSnapshot {
    let windows = entry
        .windows
        .iter()
        .filter_map(|window| window_to_agent_window(window))
        .collect::<Vec<_>>();
    let label = truncate_non_empty(&entry.label, NAME_LIMIT);
    let plan_name = entry
        .plan_name
        .as_deref()
        .and_then(|name| truncate_non_empty(name, NAME_LIMIT));
    // 错误态保留原因（status != ok）；ok / pending 的 message 是平台提示，
    // 不属于错误态语义，不进快照（手机端按 message 有无判定错误展示）
    let message = if entry.status != "ok" {
        entry
            .message
            .as_deref()
            .and_then(|message| truncate_non_empty(message, MESSAGE_LIMIT))
    } else {
        None
    };
    AgentQuotaSnapshot {
        source: provider.to_string(),
        ts: entry.updated_at,
        plan_type: None,
        windows,
        label,
        plan_name,
        balance: sanitize_balance(entry.balance.as_ref()),
        message,
    }
}

/// ProviderQuotaWindow → AgentQuotaWindow：协议形状只有 key / used_pct /
/// reset_at 三字段，无有效百分比的窗口（如只有 used/total 的加油包剩余量）
/// 无法承载，直接丢弃。
fn window_to_agent_window(window: &ProviderQuotaWindow) -> Option<crate::agent_quota_history::AgentQuotaWindow> {
    let used_pct = window.used_percent?;
    if !used_pct.is_finite() || !(0.0..=100.0).contains(&used_pct) {
        return None;
    }
    Some(crate::agent_quota_history::AgentQuotaWindow {
        key: window.key.clone(),
        used_pct,
        reset_at: window.resets_at,
    })
}

/// 余额字段的有限性清洗（NaN / 无穷不落盘；amount 无效则整体丢弃）。
fn sanitize_balance(balance: Option<&ProviderQuotaBalance>) -> Option<ProviderQuotaBalance> {
    let balance = balance?;
    if !balance.amount.is_finite() {
        return None;
    }
    Some(ProviderQuotaBalance {
        amount: balance.amount,
        currency: balance.currency.chars().take(16).collect(),
        granted: balance.granted.filter(|value| value.is_finite()),
        topped_up: balance.topped_up.filter(|value| value.is_finite()),
    })
}

/// 截断到上限；空白串归一为 None。
fn truncate_non_empty(value: &str, limit: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(limit).collect())
}

/// 追加一条快照。同一 (source, label) 同一秒内的重复调用只保留第一条，
/// 避免同一轮多入口查询把同一份额度重复计入采样数。
pub fn append_snapshot(snapshot: &AgentQuotaSnapshot) {
    if snapshot.source.trim().is_empty() {
        return;
    }
    // 与 CLI 通道共用同一份「无窗口快照必须有扩展载荷」的语义；本通道的
    // 调用方（entry_to_snapshot）保证了这一点，这里再防御一次。
    if snapshot.windows.is_empty()
        && snapshot.balance.is_none()
        && snapshot.plan_name.is_none()
        && snapshot.message.is_none()
    {
        return;
    }
    if let Err(e) = try_append(snapshot) {
        eprintln!("[zbar-provider-history] 写 provider 额度快照失败: {e}");
    }
}

fn try_append(snapshot: &AgentQuotaSnapshot) -> Result<(), String> {
    let append_lock = APPEND_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = append_lock
        .lock()
        .map_err(|_| "provider 额度快照写入锁已中毒".to_string())?;
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = history_path()?;

    maybe_cleanup(&path);
    if path.exists() {
        ensure_trailing_newline(&path)?;
    }

    let dedup_key = format!(
        "{}|{}",
        snapshot.source,
        snapshot.label.as_deref().unwrap_or("")
    );
    let seen = LAST_TS_BY_KEY.get_or_init(|| {
        let mut values: HashMap<String, i64> = HashMap::new();
        if let Ok(snapshots) = load_all() {
            for old in snapshots {
                let key = format!("{}|{}", old.source, old.label.as_deref().unwrap_or(""));
                values
                    .entry(key)
                    .and_modify(|ts| *ts = (*ts).max(old.ts))
                    .or_insert(old.ts);
            }
        }
        Mutex::new(values)
    });
    let mut seen = seen
        .lock()
        .map_err(|_| "provider 额度快照去重锁已中毒".to_string())?;
    if let Some(last_ts) = seen.get(&dedup_key) {
        if same_second(*last_ts, snapshot.ts) {
            return Ok(());
        }
    }

    let line = serde_json::to_string(snapshot)
        .map_err(|e| format!("序列化 provider 额度快照失败: {e}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开 provider 额度快照文件失败: {e}"))?;
    writeln!(file, "{line}").map_err(|e| format!("写入 provider 额度快照失败: {e}"))?;
    seen.entry(dedup_key)
        .and_modify(|last_ts| *last_ts = (*last_ts).max(snapshot.ts))
        .or_insert(snapshot.ts);
    Ok(())
}

fn same_second(left_ms: i64, right_ms: i64) -> bool {
    left_ms / 1000 == right_ms / 1000
}

/// 进程异常退出后，若文件尾没有换行，先补齐边界再追加新快照。
fn ensure_trailing_newline(path: &std::path::Path) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("打开 provider 额度快照文件失败: {e}"))?;
    let len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| format!("定位 provider 额度快照文件失败: {e}"))?;
    if len == 0 {
        return Ok(());
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::Start(len - 1))
        .map_err(|e| format!("定位 provider 额度快照文件失败: {e}"))?;
    file.read_exact(&mut last)
        .map_err(|e| format!("读取 provider 额度快照末字节失败: {e}"))?;
    if last[0] != b'\n' {
        file.seek(SeekFrom::End(0))
            .map_err(|e| format!("定位 provider 额度快照文件失败: {e}"))?;
        file.write_all(b"\n")
            .map_err(|e| format!("补写 provider 额度快照换行失败: {e}"))?;
    }
    Ok(())
}

fn maybe_cleanup(path: &PathBuf) {
    let cell = LAST_CLEANUP.get_or_init(|| Mutex::new(None));
    let Ok(mut last) = cell.lock() else {
        return;
    };
    if let Some(t) = *last {
        if t.elapsed().unwrap_or(Duration::ZERO) < CLEANUP_INTERVAL {
            return;
        }
    }
    *last = Some(SystemTime::now());
    drop(last);

    if let Err(e) = try_cleanup(path) {
        eprintln!("[zbar-provider-history] 滚动清理失败（24h 后重试）: {e}");
    }
}

fn try_cleanup(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let cutoff = Local::now().timestamp_millis() - RETENTION_MS;
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("打开 provider 额度快照文件失败: {e}"))?;
    let reader = BufReader::new(file);
    let mut kept = Vec::new();
    let mut removed = 0usize;
    for line in reader.lines() {
        let line = match line {
            Ok(line) if !line.trim().is_empty() => line,
            _ => continue,
        };
        match serde_json::from_str::<AgentQuotaSnapshot>(&line) {
            Ok(snapshot) if snapshot.ts < cutoff => removed += 1,
            _ => kept.push(line),
        }
    }
    if removed == 0 {
        return Ok(());
    }

    let tmp = path.with_extension("jsonl.tmp");
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .map_err(|e| format!("打开 provider 额度临时文件失败: {e}"))?;
    for line in kept {
        writeln!(out, "{line}").map_err(|e| format!("写入 provider 额度临时文件失败: {e}"))?;
    }
    out.flush()
        .map_err(|e| format!("刷新 provider 额度临时文件失败: {e}"))?;
    drop(out);
    fs::rename(&tmp, path).map_err(|e| format!("替换 provider 额度快照文件失败: {e}"))
}

/// 读取全部快照，损坏行跳过并按时间升序返回（CLI 通道文件不混入）。
pub fn load_all() -> Result<Vec<AgentQuotaSnapshot>, String> {
    let path = history_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = OpenOptions::new()
        .read(true)
        .open(&path)
        .map_err(|e| format!("打开 provider 额度快照文件失败: {e}"))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(line) if !line.trim().is_empty() => line,
            _ => continue,
        };
        if let Ok(snapshot) = serde_json::from_str::<AgentQuotaSnapshot>(&line) {
            if is_provider_snapshot_source(&snapshot.source) {
                out.push(snapshot);
            }
        }
    }
    out.sort_by_key(|snapshot| snapshot.ts);
    Ok(out)
}

/// 读取指定时间范围内的本地快照（[from_ms, to_ms) 半开区间，升序）。
/// 主窗口「额度驾驶舱」页（QuotaPage）的 90 天窗口取数用，与 CLI 通道的
/// agent_quota_history::load_range 同构。
pub fn load_range(from_ms: i64, to_ms: i64) -> Result<Vec<AgentQuotaSnapshot>, String> {
    Ok(filter_range(load_all()?, from_ms, to_ms))
}

/// 区间过滤的纯函数部分（便于单测注入：快照数组由测试直接构造，
/// 不触碰真实的 ~/.agenthud/provider_quota_history.jsonl）。
fn filter_range(
    snapshots: Vec<AgentQuotaSnapshot>,
    from_ms: i64,
    to_ms: i64,
) -> Vec<AgentQuotaSnapshot> {
    snapshots
        .into_iter()
        .filter(|snapshot| snapshot.ts >= from_ms && snapshot.ts < to_ms)
        .collect()
}

/// 删除 provider 额度快照历史（与 CLI 通道的清理入口对称）。
pub fn clear_history() -> Result<(), String> {
    let append_lock = APPEND_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = append_lock
        .lock()
        .map_err(|_| "provider 额度快照写入锁已中毒".to_string())?;
    let path = history_path()?;
    if path.exists() {
        fs::remove_file(path).map_err(|e| format!("删除 provider 额度快照文件失败: {e}"))?;
    }
    if let Some(seen) = LAST_TS_BY_KEY.get() {
        seen.lock()
            .map_err(|_| "provider 额度快照去重锁已中毒".to_string())?
            .clear();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(key: &str, used_percent: Option<f64>) -> ProviderQuotaWindow {
        ProviderQuotaWindow {
            key: key.to_string(),
            title: String::new(),
            used_percent,
            used: None,
            total: None,
            unit: None,
            resets_at: Some(1_730_000_000_000),
        }
    }

    fn entry(status: &str) -> ProviderQuotaEntry {
        ProviderQuotaEntry {
            credential_id: "cred-1".into(),
            label: "  主账号  ".into(),
            status: status.into(),
            windows: vec![
                window("hour5", Some(30.0)),
                // 无百分比的窗口（如加油包剩余量）被丢弃
                window("fuel", None),
                // 越界百分比被丢弃
                window("weekly", Some(120.0)),
            ],
            balance: Some(ProviderQuotaBalance {
                amount: 110.0,
                currency: "CNY".into(),
                granted: Some(10.0),
                topped_up: Some(100.0),
            }),
            plan_name: Some("Pay-as-you-go".into()),
            message: Some("API Key 无效或已过期".into()),
            updated_at: 1_730_000_000_000,
        }
    }

    #[test]
    fn entry_maps_to_snapshot_with_windows_label_balance() {
        let snapshot = entry_to_snapshot("deepseek", &entry("ok"));
        assert_eq!(snapshot.source, "deepseek");
        assert_eq!(snapshot.label.as_deref(), Some("主账号")); // 去空白
        assert_eq!(snapshot.plan_name.as_deref(), Some("Pay-as-you-go"));
        // 仅保留有有效百分比的窗口
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].key, "hour5");
        assert_eq!(snapshot.windows[0].used_pct, 30.0);
        assert_eq!(snapshot.windows[0].reset_at, Some(1_730_000_000_000));
        assert!(snapshot.balance.is_some());
        // ok 状态不携带 message（手机端按 message 判定错误展示）
        assert!(snapshot.message.is_none());
        assert!(snapshot.plan_type.is_none());
    }

    #[test]
    fn error_entry_keeps_message() {
        let snapshot = entry_to_snapshot("qoder", &entry("error"));
        assert_eq!(snapshot.message.as_deref(), Some("API Key 无效或已过期"));
    }

    #[test]
    fn balance_only_entry_survives_append_gate() {
        // 余额型：windows 全被过滤后为空，但有 balance —— 允许落盘
        let snapshot = entry_to_snapshot("moonshot", &entry("ok"));
        let balance_only = AgentQuotaSnapshot {
            windows: Vec::new(),
            ..snapshot.clone()
        };
        assert!(balance_only.windows.is_empty());
        assert!(balance_only.balance.is_some());
        // append_snapshot 的防御门不会拒绝它（这里只验证判定逻辑本身）
        let no_payload = AgentQuotaSnapshot {
            source: "moonshot".into(),
            ts: 1,
            plan_type: None,
            windows: Vec::new(),
            label: None,
            plan_name: None,
            balance: None,
            message: None,
        };
        assert!(no_payload.balance.is_none()
            && no_payload.plan_name.is_none()
            && no_payload.message.is_none());
    }

    #[test]
    fn non_provider_sources_are_rejected() {
        // CLI 通道来源与未接入来源不采集
        assert!(!is_provider_snapshot_source("codex"));
        assert!(!is_provider_snapshot_source("kimi"));
        assert!(!is_provider_snapshot_source("gemini"));
        assert!(!is_provider_snapshot_source("claude"));
        assert!(!is_provider_snapshot_source("cursor"));
        assert!(is_provider_snapshot_source("deepseek"));
        assert!(is_provider_snapshot_source("alibabatoken"));
    }

    /// 区间过滤（load_range 的纯函数部分）：[from, to) 半开区间——
    /// 恰好等于 from 的采样保留、恰好等于 to 的采样排除，与 CLI 通道
    /// agent_quota_history::load_range 的口径一致。
    #[test]
    fn filter_range_is_half_open() {
        let snap = |ts: i64| AgentQuotaSnapshot {
            source: "deepseek".into(),
            ts,
            plan_type: None,
            windows: Vec::new(),
            label: None,
            plan_name: None,
            balance: None,
            message: None,
        };
        let snaps = vec![snap(1_000), snap(2_000), snap(3_000), snap(4_000)];
        let out = filter_range(snaps, 2_000, 4_000);
        assert_eq!(
            out.iter().map(|s| s.ts).collect::<Vec<_>>(),
            vec![2_000, 3_000],
            "from 侧闭、to 侧开；界外采样剔除且保持升序"
        );
        // 空区间 / 全界外为空
        assert!(filter_range(vec![snap(1_000)], 2_000, 3_000).is_empty());
        assert!(filter_range(Vec::new(), 0, i64::MAX).is_empty());
    }

    #[test]
    fn invalid_balance_is_dropped_and_fields_truncated() {
        let mut value = entry("ok");
        value.balance = Some(ProviderQuotaBalance {
            amount: f64::NAN,
            currency: "CNY".into(),
            granted: None,
            topped_up: None,
        });
        let snapshot = entry_to_snapshot("deepseek", &value);
        assert!(snapshot.balance.is_none());

        // label 截断到 64 字（以字符计，中文一字一字符）
        let mut long = entry("ok");
        long.label = "长".repeat(80);
        let snapshot = entry_to_snapshot("deepseek", &long);
        assert_eq!(snapshot.label.as_deref().map(|s| s.chars().count()), Some(64));

        // 空白 label 归一为 None
        let mut blank = entry("ok");
        blank.label = "   ".into();
        let snapshot = entry_to_snapshot("deepseek", &blank);
        assert!(snapshot.label.is_none());
    }

    #[test]
    fn cleanup_removes_only_expired_snapshots() {
        let path = std::env::temp_dir().join(format!(
            "zbar-provider-history-test-{}-{}.jsonl",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("cleanup")
                .replace(':', "_")
        ));
        let expired = AgentQuotaSnapshot {
            source: "deepseek".into(),
            ts: Local::now().timestamp_millis() - RETENTION_MS - 1,
            plan_type: None,
            windows: Vec::new(),
            label: Some("旧账号".into()),
            plan_name: None,
            balance: Some(ProviderQuotaBalance {
                amount: 1.0,
                currency: "CNY".into(),
                granted: None,
                topped_up: None,
            }),
            message: None,
        };
        let current = AgentQuotaSnapshot {
            ts: Local::now().timestamp_millis(),
            ..expired.clone()
        };
        let contents = format!(
            "{}\n{}\n",
            serde_json::to_string(&expired).unwrap(),
            serde_json::to_string(&current).unwrap()
        );
        std::fs::write(&path, contents).unwrap();

        try_cleanup(&path).unwrap();
        let lines = std::fs::read_to_string(&path).unwrap();
        let snapshots: Vec<AgentQuotaSnapshot> = lines
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(snapshots, vec![current]);
        let _ = std::fs::remove_file(path);
    }
}
