//! OpenCode Go 模块：双数据源（官方 API Key 优先场景 + 本地 SQLite 估算）。
//!
//! 两条路径，按「数据可得性」自动选择，互不依赖：
//!
//! **A. 官方 API（凭证型，agentId `opencodego`，kind=apiKey）**
//! `GET https://opencode.ai/zen/go/v1/usage`，`Authorization: Bearer <apiKey>`，
//! 直接返回官方计费口径的三窗口百分比：
//! `{"usage":{"rolling":{status,percent,resetsAt},"weekly":{...},"monthly":{...}}}`
//! 其中 `rolling` = 滚动 5 小时窗，`resetsAt` 为 RFC3339 UTC 时间戳。
//! 实测（2026-10 真实 key）：仅 `Authorization: Bearer` 可鉴权，`x-api-key`
//! 返回 401（与部分第三方实现「双头并发送」的说法不符，故只发 Bearer）；
//! 该接口为纯账户状态读取，不消耗 Go 套餐额度。无效 key → HTTP 401
//! `{"type":"error","error":{"type":"AuthError",...}}`。
//! 这是**官方权威口径**，因此只要用户配了 API Key 就优先展示（见
//! `fetch_local_entries` 的新鲜度门槛），不再叠加本地估算条目以免同源重复。
//!
//! **B. 本地 SQLite（零凭证回退）**
//! 直读 OpenCode 本地数据目录的 opencode.db（消息单表，不联合 part 表），
//! 按 CodexBar 同款常量估算滚动 5 小时窗 / 周 / 月三档用量百分比。表名随
//! OpenCode 版本自适应：v2 为 `session_message`（角色在列 `type`，provider 在
//! `$.model.providerID`），v1 为 `message`（角色在 `$.role`，provider 在
//! `$.providerID`）——详见 `query_records`。数据目录：
//! `~/.local/share/opencode/`（Windows 上 OpenCode 同样使用
//! `%USERPROFILE%\.local\share\opencode\`）；`ZBAR_OPENCODE_HOME` 可覆盖根
//! 目录（测试/便携场景用，对齐 codex.rs 的 ZBAR_CODEX_HOME 先例）。
//! 登录检测：auth.json 顶层键 `"opencode-go"` 内层 `"key"` 非空——key 本身
//! 不参与查询，只用于判断是否启用了 Go 计划。库存在但未登录 Go 计划 →
//! 返回一条 pending 条目提示。
//! 估算口径（CodexBar 常量）：滚动 5h 窗限额 $12、周（UTC 周一起）$30、
//! 月 $60；percent = used/limit*100（clamp 0-100）。5h 窗重置 = 窗口内最老
//! 一条 + 5h；周/月重置 = 下个周一 / 下月 1 日 00:00 UTC。这是本地用量
//! 推算，非官方额度数据（entry.message 注明）。
//!
//! 本地条目仅在「有 API Key」时受新鲜度门槛（LOCAL_FRESH_MS）约束：装了 CLI
//! 又长期不用的用户，本地库会停在最后一次使用时刻，若此时已配 Key 应以官方
//! 数据为准，避免陈旧估算覆盖权威值。无 Key 时始终展示本地估算（含陈旧值），
//! 因为那是唯一可得的数据。两者都不可得 → 空 Vec（前端 tab 不出现）。
//!
//! 工程纪律（对齐 kimi.rs / codex.rs 先例）：
//! - 只读打开：SQLITE_OPEN_READ_ONLY + busy_timeout 250ms，绝不写用户库；
//! - API 路径复用 quota_http_agent（15s 超时 + codex::resolve_proxy 代理）；
//! - 错误信息为中文且绝不包含 API Key 明文。

use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{now_ms, ProviderQuotaEntry, ProviderQuotaWindow};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// OpenCode Go 官方用量接口（三窗口百分比，非推理调用、不消耗套餐额度）。
const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
/// API Key 存在时本地估算条目的新鲜度门槛（毫秒）：本地最后用量早于
/// 「now - 本值」即视为陈旧，让位给官方 API（本地窗最长 5h，取 6h 留余量）。
const LOCAL_FRESH_MS: i64 = 6 * 3_600_000;

/// CodexBar 同款估算常量：5 小时滚动窗 / 周 / 月限额（美元）。
const LIMIT_5H_USD: f64 = 12.0;
const LIMIT_WEEK_USD: f64 = 30.0;
const LIMIT_MONTH_USD: f64 = 60.0;
/// 5 小时滚动窗长度（毫秒）。
const FIVE_HOURS_MS: i64 = 5 * 3_600_000;
/// 周窗长度（毫秒）。
const WEEK_MS: i64 = 7 * 86_400_000;

/// OpenCode 数据根目录（ZBAR_OPENCODE_HOME 优先，其次 ~/.local/share/opencode/）。
/// pub(crate)：用量导入模块（opencode.rs）复用同一口径，避免两处各写一份根目录
/// 解析导致 ZBAR_OPENCODE_HOME 覆盖只在一处生效。
pub(crate) fn opencode_root() -> PathBuf {
    if let Ok(home) = std::env::var("ZBAR_OPENCODE_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("opencode")
}

/// 本地库路径（presence 探测、额度估算与用量导入共用同一口径）。
pub(crate) fn db_path() -> PathBuf {
    opencode_root().join("opencode.db")
}

/// 本地数据是否可用（provider_credentials::has_credentials 对 opencodego 的
/// presence 特判用）：opencode.db 存在即视为可用，装了 OpenCode 的用户
/// tab 自动出现；凭证体系对其保持可选。
pub(crate) fn has_local_data() -> bool {
    db_path().exists()
}

/// 登录检测：auth.json 顶层 "opencode-go" 内层 "key" 非空（文件缺失/
/// 解析失败/键缺失一律视为未登录，不阻断主流程）。
fn has_go_login(root: &Path) -> bool {
    let path = root.join("auth.json");
    let Ok(data) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
        return false;
    };
    v.get("opencode-go")
        .and_then(|go| go.get("key"))
        .and_then(|k| k.as_str())
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
}

/// 以只读方式打开本地库：READONLY（绝不写用户数据）+ busy_timeout 250ms
/// （OpenCode 正在写入时短暂等待而非立即报错；估算用途失败可容忍）。
fn open_readonly(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("打开 OpenCode 本地库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    Ok(conn)
}

/// 表是否存在（v2/v1 形态探测与测试共用；table 名是代码内常量，无注入风险）。
fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// 读取 opencode-go 的 (created_ms, cost_usd) 序列。
///
/// **v2 / v1 自适应**：v2 把 v1 的 `message` 表改名成 `session_message`
/// （其迁移 SQL：`ALTER TABLE session RENAME TO session_v2` 同一批），且**角色从
/// data JSON 的 `$.role` 提升为列 `type`**（v2 的 `data` 里实测没有 role 字段），
/// 故先探测 `session_message` 用 `type = 'assistant'` 过滤，否则回退 `message`
/// 按 `$.role` 过滤。provider 两版都用 `COALESCE(NULLIF($.providerID,''),
/// $.model.providerID)`：v1 平铺、v2 嵌套在 `$.model` 里（实测）。
///
/// 只查单表（不联合 part/session），cost 必须是数值（`json_type` 过滤字符串
/// 脏值）；时间取 `COALESCE($.time.created, time_created)`（毫秒 epoch），
/// 任一时间都拿不到的行无法定位窗口，跳过。两版都认不出消息表时报错而非静默
/// 返回空（未知 schema 要暴露出来，不能假装「用量为 0」）。
fn query_records(conn: &Connection) -> Result<Vec<(i64, f64)>, String> {
    // 角色过滤片段：v2 用列、v1 用 data JSON
    let (table, role_filter) = if has_table(conn, "session_message") {
        ("session_message", "type = 'assistant'")
    } else if has_table(conn, "message") {
        ("message", "json_extract(data,'$.role') = 'assistant'")
    } else {
        return Err("OpenCode 本地库缺少消息表（既非 v2 的 session_message 也非 v1 的 message）".to_string());
    };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT COALESCE(json_extract(data,'$.time.created'), time_created), \
                    json_extract(data,'$.cost') \
             FROM {table} \
             WHERE COALESCE(NULLIF(json_extract(data,'$.providerID'), ''), \
                            json_extract(data,'$.model.providerID')) = 'opencode-go' \
               AND {role_filter} \
               AND json_type(data,'$.cost') IN ('integer','real')"
        ))
        .map_err(|e| format!("OpenCode 查询准备失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let ts: rusqlite::types::Value = row.get(0)?;
            let cost: f64 = row.get(1)?;
            Ok((ts, cost))
        })
        .map_err(|e| format!("OpenCode 用量查询失败: {e}"))?;
    let mut records = Vec::new();
    for row in rows {
        let (ts, cost) = row.map_err(|e| format!("OpenCode 用量读取失败: {e}"))?;
        let ts_ms = match ts {
            rusqlite::types::Value::Integer(ms) => ms,
            rusqlite::types::Value::Real(ms) => ms as i64,
            _ => continue, // COALESCE 后仍为 NULL：无法定位窗口，跳过
        };
        // 负值成本属脏数据，忽略
        if cost.is_finite() && cost >= 0.0 {
            records.push((ts_ms, cost));
        }
    }
    Ok(records)
}

/// 日历窗口边界（纯函数，now 注入便于单测；全部按 UTC 口径）：
/// 5h 滚动窗起点、本周（周一 00:00）起止、本月（1 日 00:00）起止。
struct WindowBounds {
    h5_start: i64,
    week_start: i64,
    week_reset: i64,
    month_start: i64,
    month_reset: i64,
}

fn window_bounds(now: i64) -> WindowBounds {
    use chrono::{Datelike, TimeZone};
    let dt = chrono::Utc
        .timestamp_millis_opt(now)
        .single()
        .unwrap_or_else(chrono::Utc::now);
    // 当日 00:00 UTC（毫秒）
    let midnight = dt
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|n| chrono::Utc.from_utc_datetime(&n).timestamp_millis())
        .unwrap_or(now);
    // 周一 = 00:00 - 距周一的天数
    let week_start = midnight - (dt.weekday().num_days_from_monday() as i64) * 86_400_000;
    // 月窗简化为自然月（UTC）：本月 1 日 00:00 起，下月 1 日 00:00 重置
    let (year, month) = (dt.year(), dt.month());
    let month_start = chrono::NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|n| chrono::Utc.from_utc_datetime(&n).timestamp_millis())
        .unwrap_or(0);
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let month_reset = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|n| chrono::Utc.from_utc_datetime(&n).timestamp_millis())
        .unwrap_or(month_start + 30 * 86_400_000);
    WindowBounds {
        h5_start: now - FIVE_HOURS_MS,
        week_start,
        week_reset: week_start + WEEK_MS,
        month_start,
        month_reset,
    }
}

/// 单窗百分比：used/limit*100，clamp 0-100（超限按 100 展示，不炸进度条）。
fn percent_of(used: f64, limit: f64) -> f64 {
    (used / limit * 100.0).clamp(0.0, 100.0)
}

/// 窗口聚合（纯函数，now 注入便于单测）：滚动 5h 主窗 + 周副窗 + 月第三窗。
/// 5h 窗重置 = 窗口内最老一条 + 5h（窗内无记录则不展示重置时间）；
/// 周/月窗按日历边界累计。
fn build_entry(now: i64, records: &[(i64, f64)]) -> ProviderQuotaEntry {
    let bounds = window_bounds(now);
    let mut used5h = 0.0;
    let mut used_week = 0.0;
    let mut used_month = 0.0;
    // 5h 窗内最老一条（重置时间锚点）
    let mut oldest_in_5h: Option<i64> = None;
    for (ts, cost) in records {
        if *ts >= bounds.h5_start {
            used5h += cost;
            oldest_in_5h = Some(oldest_in_5h.map_or(*ts, |old| old.min(*ts)));
        }
        if *ts >= bounds.week_start {
            used_week += cost;
        }
        if *ts >= bounds.month_start {
            used_month += cost;
        }
    }
    let reset5h = oldest_in_5h.map(|old| old + FIVE_HOURS_MS);

    let mk_window = |key: &str, title: &str, used: f64, limit: f64, resets_at: Option<i64>| {
        ProviderQuotaWindow {
            key: key.to_string(),
            title: title.to_string(),
            used_percent: Some(percent_of(used, limit)),
            used: Some(used),
            total: Some(limit),
            unit: Some("$".to_string()),
            resets_at,
        }
    };

    ProviderQuotaEntry {
        credential_id: "local".to_string(),
        label: "本地估算".to_string(),
        status: "ok".to_string(),
        windows: vec![
            mk_window("hour5", "5 小时窗口", used5h, LIMIT_5H_USD, reset5h),
            mk_window("weekly", "本周", used_week, LIMIT_WEEK_USD, Some(bounds.week_reset)),
            mk_window("monthly", "本月", used_month, LIMIT_MONTH_USD, Some(bounds.month_reset)),
        ],
        balance: None,
        plan_name: None,
        // 本地推算非官方口径，message 注明避免误导
        message: Some("本地用量估算，非官方额度数据".to_string()),
        updated_at: now_ms(),
    }
}

/// 构造本地估算的骨架条目（credential_id 固定 "local"，写回时被跳过）。
fn local_entry(status: &str, message: Option<String>) -> ProviderQuotaEntry {
    ProviderQuotaEntry {
        credential_id: "local".to_string(),
        label: "本地估算".to_string(),
        status: status.to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message,
        updated_at: now_ms(),
    }
}

/// 本地 SQLite 路径（无网络，纯文件 IO）。返回 None 表示该来源完全给不出
/// 数据（库不存在 **或** 未登录 Go 计划）——两者都由调用方按「当前可用来源」
/// 决定提示还是走 API：
/// - 打开/查询失败 → 单条 error（中文原因）；
/// - 成功 → ok 条目（5h/周/月三窗估算）。
fn fetch_local_entries() -> Option<Vec<ProviderQuotaEntry>> {
    let root = opencode_root();
    let db = db_path();
    if !db.exists() {
        return None;
    }
    // 未登录 Go 计划：本地无法判断套餐用量。这里不产生条目——有 API Key 时
    // 应由官方接口出数据（「装了 CLI 但没登录、只用 Key」是真实场景），
    // 无 Key 时由 fetch_quota_entries 补一条 pending 提示。
    if !has_go_login(&root) {
        return None;
    }
    let records = match open_readonly(&db).and_then(|conn| query_records(&conn)) {
        Ok(records) => records,
        Err(e) => return Some(vec![local_entry("error", Some(e))]),
    };
    let now = now_ms();
    Some(vec![build_entry(now, &records)])
}

/// 本地估算是否足够新鲜（有 API Key 时的让位判据）：取库内最新一条用量时间，
/// 早于 now - LOCAL_FRESH_MS 即视为陈旧。库空/无时间戳 → 不新鲜（让位 API）。
pub(crate) fn local_is_fresh(now: i64, records: &[(i64, f64)]) -> bool {
    records
        .iter()
        .map(|(ts, _)| *ts)
        .max()
        .map(|latest| now.saturating_sub(latest) < LOCAL_FRESH_MS)
        .unwrap_or(false)
}

/// RFC3339 时间戳 → 毫秒 epoch（纯函数）。官方 `resetsAt` 为
/// `2026-10-12T19:21:25.000Z` 形态；解析失败返回 None（该窗不显示倒计时，
/// 不影响百分比展示）。
pub(crate) fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// 官方 usage 响应 → 三窗口条目（纯函数，便于单测）。
/// 缺 `usage` 对象 → None（调用方按响应解析失败处理）；单窗缺 `percent`
/// 则跳过该窗（周/月窗允许缺失，与 CodexBar 口径一致）；`percent` 兼容
/// 数值与字符串数字两种形态并 clamp 0-100。
pub(crate) fn entry_from_api_usage(
    cred_id: &str,
    cred_label: &str,
    raw: &str,
) -> Option<ProviderQuotaEntry> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let usage = v.get("usage")?;
    // (api 字段名, 窗口 key, 展示名)——key 复用 kimi 既有词表，前端渲染一致
    let specs = [
        ("rolling", "hour5", "5 小时窗口"),
        ("weekly", "weekly", "本周"),
        ("monthly", "monthly", "本月"),
    ];
    let mut windows = Vec::new();
    for (field, key, title) in specs {
        let Some(win) = usage.get(field).filter(|w| !w.is_null()) else {
            continue;
        };
        // percent 是唯一必填项；缺则视为该窗未接入（跳过而非报错）
        let Some(percent) = crate::provider_quota::parse_flexible_f64(
            win.get("percent").unwrap_or(&serde_json::Value::Null),
        ) else {
            continue;
        };
        windows.push(ProviderQuotaWindow {
            key: key.to_string(),
            title: title.to_string(),
            used_percent: Some(percent.clamp(0.0, 100.0)),
            // 官方只给百分比，不透出金额：不虚构 used/total/unit
            used: None,
            total: None,
            unit: None,
            resets_at: crate::provider_quota::get_any(win, &["resetsAt", "resets_at"])
                .and_then(|t| t.as_str())
                .and_then(parse_rfc3339_ms),
        });
    }
    if windows.is_empty() {
        return None;
    }
    Some(ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: cred_label.to_string(),
        status: "ok".to_string(),
        windows,
        balance: None,
        plan_name: None,
        // 官方口径，无需「非官方」免责声明
        message: None,
        updated_at: now_ms(),
    })
}

/// 单凭证请求官方用量接口，返回条目（网络/状态码/解析错误都收敛为条目，
/// 由 provider_quota 的回写循环登记到凭证 last_check）。
fn fetch_api_entry(agent: &ureq::Agent, cred: &CredentialQuerySnapshot) -> ProviderQuotaEntry {
    // 错误条目构造：status "expired" 仅用于鉴权失败（凭证卡按「校验失败」展示）
    let fail = |status: &str, msg: String| ProviderQuotaEntry {
        credential_id: cred.id.clone(),
        label: cred.label.clone(),
        status: status.to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message: Some(msg),
        updated_at: now_ms(),
    };
    // 只发 Bearer：实测 x-api-key 不被接受（401），双头并发反而易踩鉴权歧义
    let result = agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {}", cred.secret))
        .set("Accept", "application/json")
        .call();
    let (status, body) = match crate::provider_quota::flatten_response(result) {
        Ok(v) => v,
        Err(e) => return fail("error", e),
    };
    // 401/403 = Key 无效或已过期；其余非 200 带状态码（响应体可能含原因）
    match status {
        200 => {}
        401 | 403 => return fail("expired", "API Key 无效或已过期".to_string()),
        other => {
            return fail(
                "error",
                format!("OpenCode 用量查询失败（HTTP {other}）"),
            )
        }
    }
    let raw = body.unwrap_or_default();
    entry_from_api_usage(&cred.id, &cred.label, &raw)
        .unwrap_or_else(|| fail("error", "OpenCode 用量响应解析失败".to_string()))
}

/// 读取 OpenCode Go 用量并产出展示条目，双数据源按可得性合并：
/// 1. 无 API Key → 仅本地估算；本地给不出数据时补一条 pending 提示（装了 CLI
///    但未登录 Go 计划的常见态），库不存在则空 Vec（前端 tab 不出现）；
/// 2. 有 API Key → 官方三窗口为权威值。另在本地估算「新鲜」时附带保留一条
///    （credential_id="local"，含 5h 窗重置倒计时与金额估算，官方接口只给
///    百分比）；陈旧则丢弃，避免旧值覆盖官方口径；
/// 3. 有 Key 但本地库不存在 → 只出官方条目（正是「不装 CLI 只用 Key」的主场景）。
pub(crate) fn fetch_quota_entries(snapshots: &[CredentialQuerySnapshot]) -> Vec<ProviderQuotaEntry> {
    // 无 API Key：本地是唯一来源。库存在但未登录 Go 计划 → pending 提示，
    // 否则保持历史行为（库不存在 = 空 Vec，前端 tab 不出现）
    if snapshots.is_empty() {
        return match fetch_local_entries() {
            Some(entries) => entries,
            None if db_path().exists() => {
                vec![local_entry(
                    "pending",
                    Some("未检测到 OpenCode Go 订阅登录".to_string()),
                )]
            }
            None => vec![],
        };
    }
    let mut entries = Vec::new();
    // 本地估算仅在新鲜时保留：陈旧本地库（CLI 已弃用）不得覆盖官方数据
    if let Some(local) = fetch_local_entries() {
        let fresh = open_readonly(&db_path())
            .and_then(|conn| query_records(&conn))
            .map(|records| local_is_fresh(now_ms(), &records))
            .unwrap_or(false);
        if fresh {
            entries.extend(local);
        }
    }
    let agent = crate::provider_quota::quota_http_agent();
    entries.extend(snapshots.iter().map(|cred| fetch_api_entry(&agent, cred)));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 已知锚点：2024-01-01（周一）00:00:00 UTC = 1704067200000 ms。
    const MON_0: i64 = 1_704_067_200_000;
    /// 2024-01-03（周三）12:00:00 UTC = 1704273600000 ms。
    const WED_NOON: i64 = MON_0 + 2 * 86_400_000 + 12 * 3_600_000;
    /// 2024-02-01 00:00:00 UTC = 1706745600000 ms。
    const FEB_1: i64 = 1_706_745_600_000;

    /// 建内存库并插入 message 样例行（data JSON + time_created）。
    /// v1 形态：表名 `message`，角色与 provider 都在 data JSON 里。
    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        conn.execute_batch(
            "CREATE TABLE message (
                id INTEGER PRIMARY KEY,
                data TEXT NOT NULL,
                time_created INTEGER
             );",
        )
        .expect("建表失败");
        conn
    }

    /// 建内存库并插入 session_message 样例行（v2 形态：角色在列 `type`，
    /// provider/model 嵌套在 `$.model` 里 —— 均为实测形状）。
    fn memory_db_v2() -> Connection {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        conn.execute_batch(
            "CREATE TABLE session_message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                type TEXT NOT NULL,
                seq INTEGER,
                time_created INTEGER,
                time_updated INTEGER,
                data TEXT NOT NULL
             );",
        )
        .expect("建表失败");
        conn
    }

    fn insert_v2_row(conn: &Connection, type_: &str, data: &str, time_created: Option<i64>) {
        conn.execute(
            "INSERT INTO session_message (session_id, type, data, time_created)
             VALUES ('ses_test', ?1, ?2, ?3)",
            rusqlite::params![type_, data, time_created],
        )
        .expect("插入失败");
    }

    fn insert_row(conn: &Connection, data: &str, time_created: Option<i64>) {
        conn.execute(
            "INSERT INTO message (data, time_created) VALUES (?1, ?2)",
            rusqlite::params![data, time_created],
        )
        .expect("插入失败");
    }

    /// 窗口聚合：5h 滚动窗 / 周 / 月各自累计 + 重置时间锚定。
    #[test]
    fn aggregates_windows_with_reset_anchors() {
        let in_5h_a = WED_NOON - 3_600_000; // 窗内（1h 前）
        let in_5h_b = WED_NOON - 3 * 3_600_000; // 窗内（3h 前，最老）
        let out_5h = WED_NOON - 10 * 3_600_000; // 出 5h 窗，在本周本月
        let week_only = MON_0 + 3_600_000; // 周一凌晨：本周本月
        let records = vec![
            (in_5h_a, 2.0),
            (in_5h_b, 1.5),
            (out_5h, 4.0),
            (week_only, 0.5),
        ];
        let entry = build_entry(WED_NOON, &records);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, "local");
        assert_eq!(entry.label, "本地估算");
        assert_eq!(entry.windows.len(), 3);

        // 5h 主窗：2.0 + 1.5 = 3.5 → 3.5/12 = 29.17%；重置 = 最老一条 + 5h
        let h5 = &entry.windows[0];
        assert_eq!(h5.key, "hour5");
        assert_eq!(h5.title, "5 小时窗口");
        assert!((h5.used_percent.unwrap() - 3.5 / 12.0 * 100.0).abs() < 1e-9);
        assert_eq!(h5.used, Some(3.5));
        assert_eq!(h5.total, Some(12.0));
        assert_eq!(h5.resets_at, Some(in_5h_b + FIVE_HOURS_MS));

        // 周窗：全部 4 条都在本周 → 8.0/30；重置 = 下周一 00:00 UTC
        let weekly = &entry.windows[1];
        assert_eq!(weekly.key, "weekly");
        assert_eq!(weekly.used, Some(8.0));
        assert!((weekly.used_percent.unwrap() - 8.0 / 30.0 * 100.0).abs() < 1e-9);
        assert_eq!(weekly.resets_at, Some(MON_0 + WEEK_MS));

        // 月窗：同样全部在本月 → 8.0/60；重置 = 下月 1 日 00:00 UTC
        let monthly = &entry.windows[2];
        assert_eq!(monthly.key, "monthly");
        assert_eq!(monthly.used, Some(8.0));
        assert!((monthly.used_percent.unwrap() - 8.0 / 60.0 * 100.0).abs() < 1e-9);
        assert_eq!(monthly.resets_at, Some(FEB_1));

        // 估算口径声明
        assert_eq!(
            entry.message.as_deref(),
            Some("本地用量估算，非官方额度数据")
        );
    }

    /// 百分比 clamp：单条超大花费把 5h 窗打满也只展示 100%，不为负/超百。
    #[test]
    fn percent_clamps_to_0_100() {
        let entry = build_entry(WED_NOON, &[(WED_NOON - 1000, 999.0)]);
        let h5 = &entry.windows[0];
        assert_eq!(h5.used_percent, Some(100.0));
        // 空 records：三窗 used=0 → percent=0，5h 无重置时间
        let entry = build_entry(WED_NOON, &[]);
        for w in &entry.windows {
            assert_eq!(w.used_percent, Some(0.0));
            assert_eq!(w.used, Some(0.0));
        }
        assert_eq!(entry.windows[0].resets_at, None);
        // 周/月窗仍有日历重置时间
        assert_eq!(entry.windows[1].resets_at, Some(MON_0 + WEEK_MS));
    }

    /// **v1 路径**的查询过滤：providerID/role 不符排除、cost 字符串脏值排除、
    /// 时间 COALESCE（data 缺 time.created 时回退 time_created 列）、
    /// 两处时间全缺的行跳过。
    #[test]
    fn query_filters_and_coalesce() {
        let conn = memory_db();
        // 合法行 A：data 带 time.created（毫秒），cost 数值
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"assistant",
                "time":{"created":1704270000000},"cost":1.25}"#,
            Some(111111),
        );
        // 合法行 B：data 缺 time.created → COALESCE 回退 time_created 列
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"assistant","cost":2}"#,
            Some(WED_NOON - 3_600_000),
        );
        // 排除：其他 provider
        insert_row(
            &conn,
            r#"{"providerID":"anthropic","role":"assistant","cost":99.0,
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 排除：非 assistant
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"user","cost":99.0,
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 排除：cost 为字符串脏值（json_type 不在 integer/real）
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"assistant","cost":"3.0",
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 跳过：cost 合法但 data 与列的时间全缺
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"assistant","cost":5.0}"#,
            None,
        );

        let records = query_records(&conn).expect("查询应成功");
        // 行 A 取 data 时间；行 B 回退 time_created；其余被过滤
        assert_eq!(
            records,
            vec![(1_704_270_000_000, 1.25), (WED_NOON - 3_600_000, 2.0)]
        );

        // 聚合口径：两条都在 5h 窗内（WED_NOON 视角）
        let entry = build_entry(WED_NOON, &records);
        assert_eq!(entry.windows[0].used, Some(3.25));
        // 行 B（更晚）不是最老一条 → 重置锚定在行 A + 5h
        assert_eq!(entry.windows[0].resets_at, Some(1_704_270_000_000 + FIVE_HOURS_MS));
    }

    /// **v2 路径**的查询过滤（表 `session_message`）：角色认列 `type`（v2 的
    /// data 里没有 role 字段），provider 认嵌套 `$.model.providerID`；平铺
    /// `$.providerID` 仍兼容（两形态取先非空者）；时间 COALESCE 与容错口径同 v1。
    #[test]
    fn query_v2_uses_type_column_and_nested_provider() {
        let conn = memory_db_v2();
        // 合法行 A：嵌套 provider/model（实测 v2 形态），data 带 time.created
        insert_v2_row(
            &conn,
            "assistant",
            r#"{"cost":1.25,"model":{"id":"deepseek-v4.1-flash","providerID":"opencode-go"},
                "time":{"created":1704270000000},"tokens":{"input":10,"output":5}}"#,
            Some(111111),
        );
        // 合法行 B：data 缺 time.created → COALESCE 回退 time_created 列
        insert_v2_row(
            &conn,
            "assistant",
            r#"{"cost":2,"model":{"providerID":"opencode-go","id":"m"}}"#,
            Some(WED_NOON - 3_600_000),
        );
        // 合法行 C：平铺 providerID 仍认（两版本共存期的容错）
        insert_v2_row(
            &conn,
            "assistant",
            r#"{"providerID":"opencode-go","modelID":"m","cost":0.5,
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 排除：其他 provider（嵌套形态）
        insert_v2_row(
            &conn,
            "assistant",
            r#"{"cost":99.0,"model":{"providerID":"opencode","id":"step-5-preview-free"},
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 排除：v2 的 type 非 assistant（data 里即使写了 role=assistant 也不算，
        // 因为 v2 的角色在列上）
        insert_v2_row(
            &conn,
            "user",
            r#"{"role":"assistant","providerID":"opencode-go","cost":99.0,
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 排除：cost 字符串脏值
        insert_v2_row(
            &conn,
            "assistant",
            r#"{"cost":"3.0","model":{"providerID":"opencode-go"},
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 跳过：时间全缺
        insert_v2_row(
            &conn,
            "assistant",
            r#"{"cost":5.0,"model":{"providerID":"opencode-go"}}"#,
            None,
        );

        let records = query_records(&conn).expect("v2 查询应成功");
        assert_eq!(
            records,
            vec![
                (1_704_270_000_000, 1.25),
                (WED_NOON - 3_600_000, 2.0),
                (1_704_270_000_000, 0.5)
            ]
        );
    }

    /// 两版表都不存在（未知 schema）→ 报错而非静默返回空：未知形态必须暴露，
    /// 不能让额度面板把「读不到」显示成「用量为 0」。
    #[test]
    fn query_without_known_message_table_errors() {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        conn.execute_batch("CREATE TABLE account (id TEXT PRIMARY KEY);")
            .expect("建表失败");
        let err = query_records(&conn).expect_err("未知 schema 应报错");
        assert!(err.contains("消息表"), "错误信息应说明缺表原因: {err}");
        // v2 表存在时优先用它（即使同时残留 v1 的 message 表）
        conn.execute_batch(
            "CREATE TABLE message (id INTEGER PRIMARY KEY, data TEXT, time_created INTEGER);
             CREATE TABLE session_message (
                id TEXT PRIMARY KEY, session_id TEXT, type TEXT, data TEXT NOT NULL,
                time_created INTEGER);
             INSERT INTO message (data, time_created) VALUES
                ('{\"providerID\":\"opencode-go\",\"role\":\"assistant\",\"cost\":9.0}', 1);
             INSERT INTO session_message (session_id, type, data, time_created) VALUES
                ('s', 'assistant', '{\"cost\":1.0,\"model\":{\"providerID\":\"opencode-go\"}}', 7);",
        )
        .expect("建双表失败");
        let records = query_records(&conn).expect("应成功");
        assert_eq!(records, vec![(7, 1.0)], "共存时应只认 v2 表，不重复累加");
    }

    /// 登录检测：auth.json 顶层 opencode-go.key 非空才有效；
    /// 文件缺失 / 解析失败 / 键缺失 / 空 key 一律视为未登录。
    #[test]
    fn go_login_detection() {
        let tmp = std::env::temp_dir().join(format!("zbar-oc-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let auth = tmp.join("auth.json");

        // 文件缺失 → false
        assert!(!has_go_login(&tmp));

        // 非空 key → true
        std::fs::write(&auth, r#"{"opencode-go":{"key":"sk-abc"}}"#).unwrap();
        assert!(has_go_login(&tmp));

        // 空 key / 键缺失 / 顶层无 opencode-go / 坏 JSON → false
        std::fs::write(&auth, r#"{"opencode-go":{"key":"  "}}"#).unwrap();
        assert!(!has_go_login(&tmp));
        std::fs::write(&auth, r#"{"opencode-go":{}}"#).unwrap();
        assert!(!has_go_login(&tmp));
        std::fs::write(&auth, r#"{"other":1}"#).unwrap();
        assert!(!has_go_login(&tmp));
        std::fs::write(&auth, "not json").unwrap();
        assert!(!has_go_login(&tmp));

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 日历窗口边界：周一 00:00 起点、周日起点回退到上周一、跨月重置。
    #[test]
    fn calendar_bounds() {
        // 周一 00:00 本身：周窗起点 = 自身，重置 = +7 天
        let b = window_bounds(MON_0);
        assert_eq!(b.week_start, MON_0);
        assert_eq!(b.week_reset, MON_0 + WEEK_MS);
        assert_eq!(b.month_start, MON_0);
        assert_eq!(b.month_reset, FEB_1);

        // 周三正午：起点回退到本周一
        let b = window_bounds(WED_NOON);
        assert_eq!(b.week_start, MON_0);
        assert_eq!(b.h5_start, WED_NOON - FIVE_HOURS_MS);

        // 周日（2024-01-07 00:00 = MON_0 + 6 天）：仍归本周
        let sunday = MON_0 + 6 * 86_400_000;
        let b = window_bounds(sunday);
        assert_eq!(b.week_start, MON_0);
        assert_eq!(b.week_reset, MON_0 + WEEK_MS);
    }

    /// 官方 API 响应解析：用真实抓取到的响应体做基准（2026-10 实测，
    /// `GET /zen/go/v1/usage`）。三窗口顺序 rolling→hour5 / weekly / monthly，
    /// 百分比取官方值，resetsAt 的 RFC3339 正确转毫秒。
    #[test]
    fn api_usage_parses_real_response() {
        // 真实响应（原样粘贴，勿手改形状）
        let raw = r#"{"usage":{
            "rolling":{"status":"ok","percent":0,"resetsAt":"2026-10-12T19:21:25.000Z"},
            "weekly":{"status":"ok","percent":0,"resetsAt":"2026-10-19T00:00:00.000Z"},
            "monthly":{"status":"ok","percent":0,"resetsAt":"2026-11-12T14:02:26.000Z"}}}"#;
        let e = entry_from_api_usage("cred-1", "我的 Go", raw).expect("应解析成功");
        assert_eq!(e.credential_id, "cred-1");
        assert_eq!(e.label, "我的 Go");
        assert_eq!(e.status, "ok");
        // 官方口径无金额、无免责声明
        assert!(e.balance.is_none());
        assert!(e.plan_name.is_none());
        assert!(e.message.is_none());
        assert_eq!(e.windows.len(), 3);
        // 窗口 key 复用 kimi 词表（前端渲染分支一致）
        let keys: Vec<&str> = e.windows.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, vec!["hour5", "weekly", "monthly"]);
        assert_eq!(e.windows[0].title, "5 小时窗口");
        // rolling = 滚动 5h 窗
        assert_eq!(e.windows[0].used_percent, Some(0.0));
        assert_eq!(e.windows[1].used_percent, Some(0.0));
        assert_eq!(e.windows[2].used_percent, Some(0.0));
        // 官方只给百分比：不虚构金额字段
        assert!(e.windows[0].used.is_none());
        assert!(e.windows[0].total.is_none());
        assert!(e.windows[0].unit.is_none());
        // resetsAt → 毫秒 epoch（2026-10-19T00:00:00Z = 1792368000000）
        assert_eq!(e.windows[1].resets_at, parse_rfc3339_ms("2026-10-19T00:00:00.000Z"));
        assert_eq!(e.windows[1].resets_at, Some(1_792_368_000_000));
    }

    /// 官方百分比：数值/字符串双形态、越界 clamp、缺窗跳过、缺 usage 判失败。
    #[test]
    fn api_usage_edge_cases() {
        // percent 字符串数字 + usedPercent 变体不支持（只认 percent）+ 越界 clamp
        let raw = r#"{"usage":{
            "rolling":{"status":"ok","percent":"37.5","resetsAt":"2026-10-12T19:21:25.000Z"},
            "weekly":{"status":"ok","percent":140},
            "monthly":{"status":"ok"}}}"#;
        let e = entry_from_api_usage("c", "l", raw).expect("应解析成功");
        // monthly 无 percent → 跳过（不报错、不产生第 3 窗）
        assert_eq!(e.windows.len(), 2);
        assert_eq!(e.windows[0].used_percent, Some(37.5));
        // 越界按 100 展示（不炸进度条）
        assert_eq!(e.windows[1].used_percent, Some(100.0));

        // weekly/monthly 缺失是允许的（CodexBar 口径）；rolling 仍在即可
        let only_rolling = r#"{"usage":{"rolling":{"percent":5}}}"#;
        let e = entry_from_api_usage("c", "l", only_rolling).expect("应解析成功");
        assert_eq!(e.windows.len(), 1);
        // 缺 resetsAt → 不显示倒计时但不影响百分比
        assert_eq!(e.windows[0].resets_at, None);

        // 缺 usage 对象 / 坏 JSON / 全窗无 percent → None（调用方转解析失败）
        assert!(entry_from_api_usage("c", "l", r#"{"error":"x"}"#).is_none());
        assert!(entry_from_api_usage("c", "l", "not json").is_none());
        assert!(entry_from_api_usage("c", "l", r#"{"usage":{}}"#).is_none());
        // 401 错误体（AuthError 形状）同样判失败
        assert!(entry_from_api_usage(
            "c",
            "l",
            r#"{"type":"error","error":{"type":"AuthError","message":"Unauthorized"}}"#
        )
        .is_none());
    }

    /// RFC3339 解析：官方带毫秒 Z 形态、无毫秒、带偏移、脏值。
    #[test]
    fn rfc3339_parsing() {
        // 2026-10-12T19:21:25Z（实测 rolling.resetsAt）
        assert_eq!(
            parse_rfc3339_ms("2026-10-12T19:21:25.000Z"),
            Some(1_791_832_885_000)
        );
        // 无毫秒等价
        assert_eq!(
            parse_rfc3339_ms("2026-10-12T19:21:25Z"),
            parse_rfc3339_ms("2026-10-12T19:21:25.000Z")
        );
        // 带时区偏移等价于同一瞬时（+08:00 = UTC 03:21:25）
        assert_eq!(
            parse_rfc3339_ms("2026-10-12T11:21:25+08:00"),
            parse_rfc3339_ms("2026-10-12T03:21:25Z")
        );
        // 脏值/空 → None
        assert_eq!(parse_rfc3339_ms(""), None);
        assert_eq!(parse_rfc3339_ms("  "), None);
        assert_eq!(parse_rfc3339_ms("2026-10-12"), None);
        assert_eq!(parse_rfc3339_ms("garbage"), None);
    }

    /// 新鲜度门槛：有 API Key 时陈旧本地估算让位官方数据。
    /// 门槛 6h，用 5h/7h 两侧取样。
    #[test]
    fn local_freshness_gate() {
        let now = WED_NOON;
        // 5h 前（门槛内）→ 新鲜
        assert!(local_is_fresh(now, &[(now - 5 * 3_600_000, 1.0)]));
        // 恰好 6h（门槛边界，< 判定）→ 陈旧
        assert!(!local_is_fresh(now, &[(now - LOCAL_FRESH_MS, 1.0)]));
        // 7h 前 → 陈旧
        assert!(!local_is_fresh(now, &[(now - 7 * 3_600_000, 1.0)]));
        // 取最新一条判定：老记录 + 新记录 → 新鲜
        assert!(local_is_fresh(
            now,
            &[(now - 7 * 3_600_000, 1.0), (now - 3_600_000, 1.0)]
        ));
        // 库空 / 无记录 → 不新鲜（让位 API）
        assert!(!local_is_fresh(now, &[]));
    }
}
