//! MiniMax（Coding Plan 订阅）额度查询模块。
//!
//! 数据来源：GET https://api.{host}/v1/token_plan/remains（Bearer Coding Plan
//! Token 鉴权，`sk-cp-` 前缀）。host 按凭证 region 分流：
//! - `region == Some("global")` → api.minimax.io（国际站）
//! - 其余（None 或 "cn"）→ api.minimaxi.com（国内站，默认）
//!
//! 新版端点 404/501（部分站点未部署）时回退旧版开放平台路径
//! `/v1/api/openplatform/coding_plan/remains`；401/403 属凭证错误，
//! 直接判定 expired 不做回退。
//!
//! 响应结构（信封）：
//! `{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[
//!   {"model_name":"MiniMax-M2","current_interval_total_count":1000,
//!    "current_interval_usage_count":300,"current_interval_remaining_percent":70,
//!    "start_time":1730000000,"end_time":1730018000,
//!    "current_weekly_total_count":5000,"current_weekly_usage_count":1200,
//!    "current_weekly_remaining_percent":76,"weekly_start_time":1729958400,
//!    "remains_time":7200}]}}`
//!
//! 语义陷阱（CodexBar 源码注释明确，务必保持）：
//! - `current_interval_usage_count` 字段名带 usage 但实际语义是【剩余量】，
//!   不得当作已用量；已用量 = total - usage_count；
//! - 百分比一律以 `*_remaining_percent` 为准：usedPercent = 100 - 剩余百分比；
//!   缺失时用 (total - usage_count)/total 计数回退；
//! - `start_time/end_time/remains_time` 是 epoch 秒或毫秒（>10^12 判毫秒）
//!   自适应；窗口重置时间 = end_time（或 now + remains_time 兜底）。
//!
//! 窗口映射：model_remains 多条时取剩余百分比最低（最接近耗尽）的一条做
//! 主窗口，其余忽略；primary key="interval"（title「当前窗口」，5h 由
//! end-start 可推断但 title 固定），secondary key="weekly"（title「本周」；
//! title 为硬编码中文，前端 QuotaEntryCard 按 window.key 做 i18n 映射）。
//! status_code==1004 或 status_msg 含 "cookie"/"login" 或 HTTP 401/403 →
//! expired（默认区域文案附加国际站切换提示，见 expired_message）；
//! 其他非 0 → error（带 status_msg）。
//!
//! ## 零凭证回退：MiniMax Code 本地 OAuth 登录态
//!
//! MiniMax Code 把自家 OAuth 登录态落在
//! `<数据根>/auth/<env>/<region>/<clientId>/auth.json`（本机真实布局
//! `~/.minimax/auth/prod/cn/mcode-public/auth.json`；records map 的 value 带
//! accessToken / refreshToken / clientId / expiresAtMs）。用户**没配 Coding Plan
//! 凭证**时这条本地登录态是唯一问得动的来源，故作为零凭证回退：凭证条目优先，
//! 只有一条凭证条目都没有时才追加本地登录条目（见 provider_quota 的 minimax 分支）。
//!
//! **端点只能走开放平台路径**（本机真实登录态实测）：
//! - 同一 accessToken 打 `/v1/api/openplatform/coding_plan/remains` → HTTP 200
//!   `{"model_remains":null,"base_resp":{"status_code":2062,"status_msg":"no active
//!   token plan subscription"}}`——token **被接受**，只是该账号没有生效订阅；
//! - 同一 token 打主路径 `/v1/token_plan/remains` → HTTP 200
//!   `{"base_resp":{"status_code":1004,"status_msg":"login fail: Please carry the
//!   API secret key in the 'Authorization' field of the request header"}}`
//!   ——主路径只认 `sk-cp-` Key，本地 OAuth token 必然失败。
//! 故本地路径固定用 FALLBACK_PATH，且不做 should_fallback 判定（那会先打一次
//! 注定失败的主路径）。
//!
//! **静默纪律**：无 auth.json / 记录不可解析 / token 已过期 / 网络失败 / 非 200 /
//! body 不可解析 / 其他业务码 → 一律不产条目（本地登录是补充展示，不该在面板上
//! 刷错误）；token 只在内存里流转，不写日志、不进任何错误消息。过期也**不刷新**
//! ——MiniMax Code 的刷新端点与 client 行为未知，不猜。唯一例外是业务码 2062
//! （已登录但无生效订阅）：产出一条 pending 中性条目说明原因，让面板解释空白
//! 而不是留空（前端 QuotaEntryCard 对 ok/pending 的 message 走中性说明行）。

use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, now_ms, parse_flexible_f64, quota_http_agent, ProviderQuotaEntry,
    ProviderQuotaWindow,
};
use std::path::{Path, PathBuf};

/// 新版端点路径（优先）。
const PRIMARY_PATH: &str = "/v1/token_plan/remains";
/// 旧版开放平台端点路径（新版 404/501 时回退）。
const FALLBACK_PATH: &str = "/v1/api/openplatform/coding_plan/remains";

/// region → API 主机域名（纯函数，便于单测）：国际站（凭证的 "global"，以及
/// MiniMax Code 本地登录目录用的 "en"/"io"，见 region_hint）走 api.minimax.io；
/// 其余（None/"cn"/未知值）默认国内站 api.minimaxi.com。凭证下拉只提供
/// cn/global（provider_credentials 的 REGIONS），en/io 仅来自本地登录目录名，
/// 两条路径共用同一映射。
fn host_for_region(region: Option<&str>) -> &'static str {
    match region {
        Some("global") | Some("en") | Some("io") => "api.minimax.io",
        _ => "api.minimaxi.com",
    }
}

/// 401/403 与登录态失效共用的过期文案（纯函数，便于单测）：region 为默认
///（None/"cn"，走国内站端点）时附加区域切换提示——国际站 Key 误填默认区域
/// 是最常见的「假过期」场景，引导用户在编辑弹层把区域切换为国际站；
/// global 不附加（区域无错配可能）。
fn expired_message(region: Option<&str>) -> String {
    if region == Some("global") {
        "Coding Plan Token 无效或已过期".to_string()
    } else {
        "Coding Plan Token 无效或已过期（如为国际站 Key 请在编辑中将区域切换为国际站）".to_string()
    }
}

/// 端点缺失形态（404 Not Found / 501 Not Implemented）→ 尝试回退旧端点；
/// 401/403 属凭证错误（Key 无效），网络层彻底失败也不回退（避免双倍超时）。
fn should_fallback(raw: &Result<(u16, Option<String>), String>) -> bool {
    matches!(raw, Ok((404, _)) | Ok((501, _)))
}

/// 逐凭证查询 MiniMax Coding Plan 额度（串行；单凭证失败产出 error/expired
/// 条目，不阻塞其他凭证）。由 provider_quota 骨架分发调用。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    snapshots
        .iter()
        .map(|cred| {
            let base = host_for_region(cred.region.as_deref());
            let raw = fetch_remains_with_fallback(&agent, base, cred);
            entry_from_raw(&cred.id, &cred.label, cred.region.as_deref(), &raw)
        })
        .collect()
}

/// 单次请求（网络层）：GET https://{host}{path}，头含 Bearer Token、
/// accept: application/json 与 MM-API-Source: ZBar。返回展平的
/// (HTTP 状态码, 响应体)；网络层彻底失败返回 Err（中文原因，不含 secret）。
fn fetch_remains_raw(
    agent: &ureq::Agent,
    host: &str,
    path: &str,
    cred: &CredentialQuerySnapshot,
) -> Result<(u16, Option<String>), String> {
    let resp = agent
        .get(&format!("https://{host}{path}"))
        .set("Authorization", &format!("Bearer {}", cred.secret))
        .set("accept", "application/json")
        .set("MM-API-Source", "ZBar")
        .call();
    flatten_response(resp).map_err(|e| format!("MiniMax 额度{e}"))
}

/// 带回退的查询：主端点返回 404/501（站点未部署新版端点）时改用旧版
/// 开放平台路径重试一次；其余结果原样返回。
fn fetch_remains_with_fallback(
    agent: &ureq::Agent,
    host: &str,
    cred: &CredentialQuerySnapshot,
) -> Result<(u16, Option<String>), String> {
    let primary = fetch_remains_raw(agent, host, PRIMARY_PATH, cred);
    if should_fallback(&primary) {
        return fetch_remains_raw(agent, host, FALLBACK_PATH, cred);
    }
    primary
}

/// epoch 秒/毫秒自适应（纯函数）：>10^12 视为毫秒，否则按秒 ×1000，
/// 统一归一为毫秒时间戳（前端 resetsAt/展示均为 ms 口径）。
fn epoch_to_ms(raw: f64) -> i64 {
    if raw > 1_000_000_000_000.0 {
        raw as i64
    } else {
        (raw * 1000.0) as i64
    }
}

/// 从单条 model_remains 中取「剩余百分比」：优先 remaining_percent 字段，
/// 缺失时按计数回退 (usage_count)/total（usage_count 语义是剩余量）；
/// 两者皆缺 → None（选主时视为最不紧急）。
fn remaining_pct_of(model: &serde_json::Value) -> Option<f64> {
    if let Some(pct) = model
        .get("current_interval_remaining_percent")
        .and_then(parse_flexible_f64)
    {
        return Some(pct);
    }
    let total = model
        .get("current_interval_total_count")
        .and_then(parse_flexible_f64)?;
    let remaining = model
        .get("current_interval_usage_count")
        .and_then(parse_flexible_f64)?;
    if total <= 0.0 {
        return None;
    }
    Some(remaining / total * 100.0)
}

/// model_remains 多条时选「剩余百分比最低」的一条做主窗口（最接近耗尽的
/// 模型最值得关注）；百分比拿不到的条目视为 +∞（最不紧急）；全拿不到回退
/// 第一条。空数组返回 None（由调用方报 error）。
fn pick_primary_model(models: &[serde_json::Value]) -> Option<&serde_json::Value> {
    let first = models.first()?;
    Some(
        models
            .iter()
            .min_by(|a, b| {
                let pa = remaining_pct_of(a).unwrap_or(f64::INFINITY);
                let pb = remaining_pct_of(b).unwrap_or(f64::INFINITY);
                pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(first),
    )
}

/// 已用百分比（纯函数）：优先 100 - remaining_percent；百分比缺失时按
/// (total - usage_count)/total 计数回退（usage_count 语义是剩余量）；
/// 两者都拿不到 → None（前端只展示 used/total）。结果 clamp 到 0-100。
fn used_percent(
    remaining_percent: Option<f64>,
    total: Option<f64>,
    remaining_count: Option<f64>,
) -> Option<f64> {
    if let Some(pct) = remaining_percent {
        return Some((100.0 - pct).clamp(0.0, 100.0));
    }
    let total = total?;
    let remaining = remaining_count?;
    if total <= 0.0 {
        return None;
    }
    Some(((total - remaining) / total * 100.0).clamp(0.0, 100.0))
}

/// 构造单个用量窗口：任一核心字段缺失超过半数时由调用方决定是否跳过；
/// 此处只做「已用量 = total - 剩余量」的换算与 clamp，unit 统一「次」。
fn build_window(
    key: &str,
    title: &str,
    total: Option<f64>,
    remaining_count: Option<f64>,
    remaining_percent: Option<f64>,
    resets_at: Option<i64>,
) -> ProviderQuotaWindow {
    let used = match (total, remaining_count) {
        (Some(total), Some(remaining)) => Some((total - remaining).max(0.0)),
        _ => None,
    };
    ProviderQuotaWindow {
        key: key.to_string(),
        title: title.to_string(),
        used_percent: used_percent(remaining_percent, total, remaining_count),
        used,
        total,
        unit: Some("次".to_string()),
        resets_at,
    }
}

/// 字段是否有值（窗口裁剪用）：JSON null / 缺失都算无值。
fn has_field(model: &serde_json::Value, key: &str) -> bool {
    model.get(key).map(|v| !v.is_null()).unwrap_or(false)
}

/// 解析单凭证查询结果 → 展示条目（纯函数，网络无关，单测直接构造输入）。
/// 分支优先级：网络失败(error) > 401/403(expired) > 非 200(error) >
/// body 解析失败(error) > base_resp.status_code != 0（1004/含 cookie|login
/// → expired，其余 → error）> 缺 model_remains(error) > 成功(ok + 双窗口)。
fn entry_from_raw(
    cred_id: &str,
    label: &str,
    region: Option<&str>,
    raw: &Result<(u16, Option<String>), String>,
) -> ProviderQuotaEntry {
    // 失败条目构造（windows 恒空；message 承载原因）
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

    let Ok((http_status, body)) = raw else {
        return fail("error", format!("MiniMax 额度{}", raw.as_ref().unwrap_err()));
    };
    // Token 被服务端拒绝：视为凭证过期（凭证卡显示「已过期」徽章）；
    // 默认区域附加国际站切换提示（见 expired_message）
    if *http_status == 401 || *http_status == 403 {
        return fail("expired", expired_message(region));
    }
    if *http_status != 200 {
        return fail("error", format!("MiniMax 额度查询失败（HTTP {http_status}）"));
    }
    let Some(body) = body.as_deref() else {
        return fail("error", "MiniMax 额度响应为空".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return fail("error", "MiniMax 额度响应解析失败".to_string());
    };
    // 业务信封：base_resp.status_code != 0 时按语义分流
    let status_code = v
        .get("base_resp")
        .and_then(|b| b.get("status_code"))
        .and_then(parse_flexible_f64)
        .unwrap_or(0.0);
    if status_code != 0.0 {
        let status_msg = v
            .get("base_resp")
            .and_then(|b| b.get("status_msg"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        // 1004 为平台约定的登录态失效码；部分站点把原因写在 status_msg
        // （含 cookie/login 字样），两者都归为 Token 过期（默认区域附加
        // 国际站切换提示，见 expired_message）
        if status_code == 1004.0
            || status_msg.to_lowercase().contains("cookie")
            || status_msg.to_lowercase().contains("login")
        {
            return fail("expired", expired_message(region));
        }
        let reason = if status_msg.trim().is_empty() {
            format!("status_code {status_code}")
        } else {
            status_msg.to_string()
        };
        return fail("error", format!("MiniMax 平台返回错误: {reason}"));
    }
    let Some(models) = v
        .get("data")
        .and_then(|d| d.get("model_remains"))
        .and_then(|m| m.as_array())
        .filter(|m| !m.is_empty())
    else {
        return fail("error", "MiniMax 额度响应缺少 model_remains".to_string());
    };
    let Some(model) = pick_primary_model(models) else {
        return fail("error", "MiniMax 额度响应缺少 model_remains".to_string());
    };

    // 窗口重置时间：end_time 优先；缺失时 now + remains_time 兜底
    // （remains_time 同样秒/毫秒自适应）。周窗按 weekly_start_time + 7 天估算。
    let interval_reset = model
        .get("end_time")
        .and_then(parse_flexible_f64)
        .map(epoch_to_ms)
        .or_else(|| {
            model
                .get("remains_time")
                .and_then(parse_flexible_f64)
                .map(|rt| now_ms() + epoch_to_ms(rt))
        });
    let weekly_reset = model
        .get("weekly_start_time")
        .and_then(parse_flexible_f64)
        .map(|ws| epoch_to_ms(ws) + 7 * 86_400_000);

    let mut windows = Vec::new();
    // 会话窗口（主窗）：interval 字段任一有值才产出
    if has_field(model, "current_interval_total_count")
        || has_field(model, "current_interval_usage_count")
        || has_field(model, "current_interval_remaining_percent")
    {
        windows.push(build_window(
            "interval",
            "当前窗口",
            model
                .get("current_interval_total_count")
                .and_then(parse_flexible_f64),
            model
                .get("current_interval_usage_count")
                .and_then(parse_flexible_f64),
            model
                .get("current_interval_remaining_percent")
                .and_then(parse_flexible_f64),
            interval_reset,
        ));
    }
    // 周窗（副窗）：weekly 字段任一有值才产出
    if has_field(model, "current_weekly_total_count")
        || has_field(model, "current_weekly_usage_count")
        || has_field(model, "current_weekly_remaining_percent")
    {
        windows.push(build_window(
            "weekly",
            "本周",
            model
                .get("current_weekly_total_count")
                .and_then(parse_flexible_f64),
            model
                .get("current_weekly_usage_count")
                .and_then(parse_flexible_f64),
            model
                .get("current_weekly_remaining_percent")
                .and_then(parse_flexible_f64),
            weekly_reset,
        ));
    }

    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows,
        balance: None,
        plan_name: None,
        message: None,
        updated_at: now_ms(),
    }
}

// ============================================================
// 零凭证回退：MiniMax Code 本地 OAuth 登录态（见模块头）
// ============================================================

/// 本地登录条目的 credential_id：不对应凭证体系任何条目，provider_quota 的
/// 校验回写按此值跳过（与 gemini/grok/opencodego 的本地条目同口径）。
const LOCAL_CREDENTIAL_ID: &str = "local";

/// 本地登录条目标签（凭证名位置展示；credential_id="local" 另有「本地」徽章）。
const LOCAL_LABEL: &str = "MiniMax Code 登录";

/// `<root>/auth` 下搜索 auth.json 的最大下探层数（真实布局是 3 层目录：
/// env/region/clientId）。目录命名随版本可能变化，故按「够深就找、找到就用」
/// 实现，不写死层级。
const LOCAL_AUTH_MAX_DEPTH: usize = 3;

/// token 过期提前量（毫秒）：距过期不足 60 秒视为不可用（与 kimi 同口径，
/// 防止请求发出途中刚好过期）。过期不刷新（见模块头），直接静默无条目。
const LOCAL_TOKEN_SKEW_MS: i64 = 60_000;

/// `base_resp.status_code`：已登录但账号没有生效的 Token Plan 订阅
/// （本机真实登录态实测返回 2062 / "no active token plan subscription"）。
const LOCAL_NO_SUBSCRIPTION_CODE: f64 = 2062.0;

/// 2062 的中性说明文案（账号状态，不是查询故障，故不按 error 展示）。
const LOCAL_NO_SUBSCRIPTION_MESSAGE: &str =
    "MiniMax Code 已登录的账号当前没有生效的 Token Plan 订阅（未开通或已到期）";

/// 一条本地登录记录（auth.json 的 records map 一条）。token 只在内存里流转：
/// 不写任何文件、不落日志、不进错误消息。
#[derive(Debug, Clone, PartialEq)]
struct LocalLogin {
    access_token: String,
    expires_at_ms: i64,
    client_id: String,
    /// 区域提示（由 auth.json 所在目录名推出，见 region_hint）；None 走默认国内站
    region: Option<String>,
}

/// token 是否可用（纯函数）：距过期还有 LOCAL_TOKEN_SKEW_MS 以上才算可用。
fn token_usable(expires_at_ms: i64, now: i64) -> bool {
    now + LOCAL_TOKEN_SKEW_MS < expires_at_ms
}

/// 解析 auth.json 的 records（纯函数，便于单测）：形态
/// `{"records":{"com.minimax.mcode.oauth.prod.cn\u0000<sub>":{...}}}`（key 里的
/// 记录名不影响解析），每个 value 取 accessToken / expiresAtMs / clientId。
/// accessToken 非空且 expiresAtMs 为数值（容忍字符串数字）才算一条；region
/// 不在文件里，留 None 由调用方按目录名补齐。
fn parse_login_records(auth: &serde_json::Value) -> Vec<LocalLogin> {
    let Some(records) = auth.get("records").and_then(|r| r.as_object()) else {
        return vec![];
    };
    records
        .values()
        .filter_map(|record| {
            let access_token = record
                .get("accessToken")
                .and_then(|t| t.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())?;
            let expires_at_ms = record.get("expiresAtMs").and_then(parse_flexible_f64)?;
            let client_id = record
                .get("clientId")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            Some(LocalLogin {
                access_token: access_token.to_string(),
                expires_at_ms: expires_at_ms as i64,
                client_id,
                region: None,
            })
        })
        .collect()
}

/// 选取可用登录记录（纯函数）：先按 token_usable 过滤（过期/即将过期直接丢，
/// 不刷新），再取 expiresAtMs 最大的一条——同一目录树里可能有多条记录（多次
/// 登录 / 多客户端），最近一次登录最可信。无可用记录 → None（静默）。
fn pick_login_record(records: &[LocalLogin], now: i64) -> Option<LocalLogin> {
    records
        .iter()
        .filter(|record| token_usable(record.expires_at_ms, now))
        .max_by_key(|record| record.expires_at_ms)
        .cloned()
}

/// 从 auth.json 的目录段推断区域提示（纯函数，便于单测）。真实布局是
/// `<env>/<region>/<clientId>/auth.json`（本机实测 prod/cn/mcode-public），
/// 目录命名随版本可能变化，故按三步容错：
/// 1. 目录段里出现已知区域名（cn/en/global/io，忽略大小写）→ 直接用它（最稳）；
/// 2. 末段与文件里的 clientId 相同 → 取它前一段（`<env>/<region>/<clientId>` 形态）；
/// 3. 否则取最后一段（`<region>/auth.json` 形态）。
/// 都取不到 → None（由 host_for_region 按默认国内站处理）。
fn region_hint(components: &[String], client_id: &str) -> Option<String> {
    let known = |c: &str| matches!(c.to_ascii_lowercase().as_str(), "cn" | "en" | "global" | "io");
    if let Some(hit) = components.iter().find(|c| known(c)) {
        return Some(hit.to_ascii_lowercase());
    }
    let client_id = client_id.trim();
    if components.len() >= 2 && !client_id.is_empty() && components[components.len() - 1] == client_id {
        return Some(components[components.len() - 2].clone());
    }
    components.last().cloned()
}

/// auth.json 相对 `<root>/auth` 的目录段（不含文件名；`auth/auth.json` → 空）。
fn auth_path_components(auth_root: &Path, auth_file: &Path) -> Vec<String> {
    auth_file
        .strip_prefix(auth_root)
        .ok()
        .and_then(|rel| rel.parent())
        .map(|parent| {
            parent
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 收集 `<root>/auth` 下最多 LOCAL_AUTH_MAX_DEPTH 层目录里的 auth.json。
/// 目录不存在 / 读不动 → 空 Vec（静默：没装过或没登录过 MiniMax Code）。
fn local_auth_files(auth_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_auth_files(auth_root, LOCAL_AUTH_MAX_DEPTH, &mut files);
    files
}

/// `local_auth_files` 的递归体：先收本层的 auth.json，再按剩余深度下探子目录
/// （深度上限保证符号链接环/超深目录不会无限下探）。
fn collect_auth_files(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_auth_files(&path, depth - 1, files);
            }
        } else if path.file_name().is_some_and(|name| name == "auth.json") {
            files.push(path);
        }
    }
}

/// 读取本地登录态并选出可用记录（无网络：纯文件 IO + 上面的纯函数）。
/// 目录/文件缺失、读失败、坏 JSON、记录全过期 → None（静默，绝不刷新）。
fn load_local_login(auth_root: &Path, now: i64) -> Option<LocalLogin> {
    let mut records = Vec::new();
    for path in local_auth_files(auth_root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(auth) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let components = auth_path_components(auth_root, &path);
        for mut login in parse_login_records(&auth) {
            login.region = region_hint(&components, &login.client_id);
            records.push(login);
        }
    }
    pick_login_record(&records, now)
}

/// 补齐开放平台端点的信封：该端点把 `model_remains` 放在**顶层**（实测 2062
/// 响应体就是 `{"model_remains":null,"base_resp":{...}}`，没有 token_plan 端点
/// 的 data 信封），而 `entry_from_raw` 按 `data.model_remains` 取。这里在解析前
/// 把顶层的 `model_remains` 搬进 `data`（已有 data 信封时原样返回），让两条路径
/// 共用同一个解析器，而不是写第二个解析器。
fn hoist_model_remains(mut v: serde_json::Value) -> serde_json::Value {
    if v.get("data").is_some_and(|d| !d.is_null()) {
        return v;
    }
    let Some(models) = v.get("model_remains").cloned() else {
        return v;
    };
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "data".to_string(),
            serde_json::json!({ "model_remains": models }),
        );
    }
    v
}

/// 本地登录条目的骨架（credential_id 固定 "local"：provider_quota 的校验回写
/// 按此跳过，前端按此渲染「本地」徽章）。
fn local_entry(status: &str, message: Option<String>) -> ProviderQuotaEntry {
    ProviderQuotaEntry {
        credential_id: LOCAL_CREDENTIAL_ID.to_string(),
        label: LOCAL_LABEL.to_string(),
        status: status.to_string(),
        windows: vec![],
        balance: None,
        plan_name: None,
        message,
        updated_at: now_ms(),
    }
}

/// 本地登录响应 → 展示条目（纯函数，网络无关，单测直接构造输入）。只认三种
/// 可解释形态，其余一律 None（静默，见模块头纪律）：
/// - 业务码 2062（no active token plan subscription）→ 一条 pending 中性条目
///   说明已登录但无生效订阅（与 gemini/opencodego 的 pending 同口径）；
/// - 业务码 0 → 交给凭证路径的 `entry_from_raw` 解析（先按 hoist_model_remains
///   补齐开放平台端点的顶层 model_remains），解析结果非 ok（缺 model_remains
///   等）也不产条目；
/// - 其余业务码（含 1004 登录失败）、非 200、坏 JSON → None。
fn entry_from_local_raw(
    raw: &Result<(u16, Option<String>), String>,
    region: Option<&str>,
) -> Option<ProviderQuotaEntry> {
    let Ok((200, Some(body))) = raw else {
        return None;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return None;
    };
    let status_code = v
        .get("base_resp")
        .and_then(|b| b.get("status_code"))
        .and_then(parse_flexible_f64)
        .unwrap_or(0.0);
    if status_code == LOCAL_NO_SUBSCRIPTION_CODE {
        return Some(local_entry(
            "pending",
            Some(LOCAL_NO_SUBSCRIPTION_MESSAGE.to_string()),
        ));
    }
    if status_code != 0.0 {
        return None;
    }
    let normalized = Ok((200u16, Some(hoist_model_remains(v).to_string())));
    let entry = entry_from_raw(LOCAL_CREDENTIAL_ID, LOCAL_LABEL, region, &normalized);
    (entry.status == "ok").then_some(entry)
}

/// 本地登录态的单次请求（网络层）：只用 FALLBACK_PATH（理由见模块头）。
/// 头与凭证路径完全一致（Bearer + accept + MM-API-Source: ZBar）；
/// 网络层彻底失败 → Err（中文原因，不含 token）。
fn fetch_local_remains_raw(
    agent: &ureq::Agent,
    host: &str,
    access_token: &str,
) -> Result<(u16, Option<String>), String> {
    let resp = agent
        .get(&format!("https://{host}{FALLBACK_PATH}"))
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("accept", "application/json")
        .set("MM-API-Source", "ZBar")
        .call();
    flatten_response(resp).map_err(|e| format!("MiniMax Code 登录态额度{e}"))
}

/// 零凭证回退入口：MiniMax Code 本地登录态 → 订阅额度条目（不可用则 None）。
/// 任何一步不可用（无 auth.json / 记录不可解析 / token 已过期 / 网络失败 /
/// 非 200 / body 不可解析 / 其他业务码）都静默返回 None——本地登录是补充来源，
/// 不该在面板上刷错误条目。唯一有产出的是业务码 0（ok 双窗口）与 2062（pending
/// 说明）。由 provider_quota 在 minimax 一条凭证条目都没有时调用。
pub(crate) fn fetch_local_login_entry() -> Option<ProviderQuotaEntry> {
    let auth_root = crate::minimaxcode::data_root().join("auth");
    let login = load_local_login(&auth_root, now_ms())?;
    let host = host_for_region(login.region.as_deref());
    let raw = fetch_local_remains_raw(&quota_http_agent(), host, &login.access_token);
    entry_from_local_raw(&raw, login.region.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "mm-1";
    const LABEL: &str = "Coding Plan";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    /// 任务给定的样例响应（成功路径：双窗口 + 百分比反推 + 秒级时间戳）。
    const SAMPLE_BODY: &str = r#"{
        "base_resp": {"status_code": 0, "status_msg": ""},
        "data": {
            "model_remains": [
                {
                    "model_name": "MiniMax-M2",
                    "current_interval_total_count": 1000,
                    "current_interval_usage_count": 300,
                    "current_interval_remaining_percent": 70,
                    "start_time": 1730000000,
                    "end_time": 1730018000,
                    "current_weekly_total_count": 5000,
                    "current_weekly_usage_count": 1200,
                    "current_weekly_remaining_percent": 76,
                    "weekly_start_time": 1729958400,
                    "remains_time": 7200
                }
            ]
        }
    }"#;

    /// 成功路径：ok 状态、双窗口、百分比以 remaining_percent 反推、
    /// usage_count（剩余语义）换算已用量、重置时间秒 → 毫秒。
    #[test]
    fn parses_sample_dual_windows() {
        let entry = entry_from_raw(CRED_ID, LABEL, None, &ok_raw(SAMPLE_BODY));
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);
        assert_eq!(entry.windows.len(), 2);

        // 主窗：interval（当前窗口）——usage_count=300 是剩余量，已用 = 700
        let interval = &entry.windows[0];
        assert_eq!(interval.key, "interval");
        assert_eq!(interval.title, "当前窗口");
        assert_eq!(interval.used_percent, Some(30.0)); // 100 - 70
        assert_eq!(interval.used, Some(700.0)); // 1000 - 300（剩余）
        assert_eq!(interval.total, Some(1000.0));
        assert_eq!(interval.unit.as_deref(), Some("次"));
        assert_eq!(interval.resets_at, Some(1_730_018_000_000)); // 秒 → 毫秒

        // 副窗：weekly（本周）
        let weekly = &entry.windows[1];
        assert_eq!(weekly.key, "weekly");
        assert_eq!(weekly.title, "本周");
        assert_eq!(weekly.used_percent, Some(24.0)); // 100 - 76
        assert_eq!(weekly.used, Some(3800.0)); // 5000 - 1200（剩余）
        // weekly_start_time + 7 天（估算）
        assert_eq!(weekly.resets_at, Some(1_729_958_400_000 + 7 * 86_400_000));
    }

    /// status_code == 1004 → expired（默认区域文案带国际站切换提示）。
    #[test]
    fn status_code_1004_maps_to_expired() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":1004,"status_msg":"invalid token"},"data":null}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "expired");
        assert_eq!(entry.windows.len(), 0);
        assert_eq!(entry.message.as_deref(), Some(expired_message(None).as_str()));
    }

    /// status_msg 含 cookie / login（不区分大小写）→ expired。
    #[test]
    fn login_keyword_in_status_msg_maps_to_expired() {
        for msg in ["Please login first", "COOKIE 失效", "login required"] {
            let body = serde_json::json!({
                "base_resp": {"status_code": 1001, "status_msg": msg},
                "data": null
            })
            .to_string();
            let entry = entry_from_raw(CRED_ID, LABEL, None, &ok_raw(&body));
            assert_eq!(entry.status, "expired", "status_msg={msg} 应判定 expired");
            assert_eq!(
                entry.message.as_deref(),
                Some(expired_message(None).as_str())
            );
        }
    }

    /// 其他非 0 status_code → error（带 status_msg）。
    #[test]
    fn other_status_code_maps_to_error() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":1008,"status_msg":"quota exceeded"},"data":null}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        let message = entry.message.expect("error 条目必须有原因");
        assert!(message.contains("quota exceeded"), "消息应带 status_msg: {message}");
        assert!(!message.contains("sk-"), "错误消息不得含 secret 片段");
    }

    /// remaining_percent 缺失 → 按 (total - usage_count)/total 计数回退
    /// （usage_count 语义是剩余量：300 剩余 / 1000 总量 → 已用 70%）。
    #[test]
    fn missing_remaining_percent_falls_back_to_counts() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[
               {"model_name":"MiniMax-M2",
                "current_interval_total_count":1000,
                "current_interval_usage_count":300}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.windows.len(), 1); // weekly 字段全缺 → 周窗不产出
        let interval = &entry.windows[0];
        assert_eq!(interval.used_percent, Some(70.0));
        assert_eq!(interval.used, Some(700.0));
        // end_time / remains_time 均缺 → 无重置时间
        assert_eq!(interval.resets_at, None);
    }

    /// 时间戳秒/毫秒自适应（>10^12 判毫秒）。
    #[test]
    fn epoch_seconds_and_ms_adaptive() {
        assert_eq!(epoch_to_ms(1_730_018_000.0), 1_730_018_000_000);
        assert_eq!(epoch_to_ms(1_730_018_000_000.0), 1_730_018_000_000);

        // end_time 直接给毫秒 → 原样保留
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[
               {"model_name":"M","current_interval_total_count":100,
                "current_interval_usage_count":10,
                "current_interval_remaining_percent":90,
                "end_time":1730018000000}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.windows[0].resets_at, Some(1_730_018_000_000));
    }

    /// HTTP 401/403 → expired（假 Key 手测链路的预期分支，不做端点回退）；
    /// 默认区域文案带国际站切换提示（精确断言见 expired_message_region_hint）。
    #[test]
    fn unauthorized_maps_to_expired() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some(r#"{"error":"invalid"}"#.to_string())));
            let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert_eq!(
                entry.message.as_deref(),
                Some(expired_message(None).as_str())
            );
            assert!(entry.windows.is_empty());
        }
    }

    /// 过期文案按 region 区分（P2 区域一致性提示）：默认区域（None/"cn"，
    /// 走国内站端点）附加「切换国际站」提示；global 不附加（无区域错配）。
    /// 消息不含 secret 片段。
    #[test]
    fn expired_message_region_hint() {
        assert_eq!(
            expired_message(Some("global")),
            "Coding Plan Token 无效或已过期"
        );
        assert_eq!(
            expired_message(None),
            "Coding Plan Token 无效或已过期（如为国际站 Key 请在编辑中将区域切换为国际站）"
        );
        assert_eq!(expired_message(Some("cn")), expired_message(None));

        // 端到端：401 + 默认区域 → expired 条目消息带提示；global → 不带
        let raw = Ok((401u16, Some(r#"{"error":"invalid"}"#.to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "expired");
        let message = entry.message.expect("expired 条目必须有原因");
        assert!(message.contains("国际站"), "默认区域应带区域提示: {message}");
        assert!(!message.contains("sk-"), "错误消息不得含 secret 片段");
        let entry = entry_from_raw(CRED_ID, LABEL, Some("global"), &raw);
        assert_eq!(
            entry.message.as_deref(),
            Some("Coding Plan Token 无效或已过期")
        );
    }

    /// 网络层失败 → error，原因透传；非 200 非 401/403 → error 带状态码。
    #[test]
    fn network_and_server_failures_map_to_error() {
        let raw: Result<(u16, Option<String>), String> =
            Err("网络错误或服务不可用: connection timed out".to_string());
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误或服务不可用"));

        let raw = Ok((500, Some("internal error".to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("500"));
    }

    /// 回退判定：仅端点缺失形态（404/501）回退；401/403/200/网络错误不回退。
    #[test]
    fn fallback_only_on_missing_endpoint() {
        assert!(should_fallback(&Ok((404, None))));
        assert!(should_fallback(&Ok((501, None))));
        assert!(!should_fallback(&Ok((401, None)))); // 凭证错误不回退
        assert!(!should_fallback(&Ok((200, None))));
        assert!(!should_fallback(&Ok((500, None))));
        assert!(!should_fallback(&Err("网络错误".to_string()))); // 避免双倍超时
    }

    /// region → 主机分流：global 国际站，其余默认国内站。
    #[test]
    fn region_maps_to_host() {
        assert_eq!(host_for_region(Some("global")), "api.minimax.io");
        assert_eq!(host_for_region(Some("cn")), "api.minimaxi.com");
        assert_eq!(host_for_region(None), "api.minimaxi.com");
        assert_eq!(host_for_region(Some("weird")), "api.minimaxi.com");
    }

    /// 多条 model_remains：取剩余百分比最低（最接近耗尽）的一条做主窗口。
    #[test]
    fn multiple_models_picks_lowest_remaining() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[
               {"model_name":"MiniMax-M2","current_interval_total_count":1000,
                "current_interval_usage_count":700,"current_interval_remaining_percent":70},
               {"model_name":"MiniMax-Text01","current_interval_total_count":100,
                "current_interval_usage_count":10,"current_interval_remaining_percent":10}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "ok");
        // 两条都只有 interval 字段 → 只产出会话主窗
        assert_eq!(entry.windows.len(), 1);
        // 选中剩余 10% 的那条：已用 90%
        assert_eq!(entry.windows[0].used_percent, Some(90.0));
        assert_eq!(entry.windows[0].used, Some(90.0));
        assert_eq!(entry.windows[0].total, Some(100.0));
    }

    /// 全部字段缺失（model_remains 为空对象）→ 无窗口产出，状态仍 ok；
    /// model_remains 缺失 / 空 → error。
    #[test]
    fn empty_or_missing_model_remains() {
        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("model_remains"));

        let raw = ok_raw(
            r#"{"base_resp":{"status_code":0,"status_msg":""},"data":{"model_remains":[{}]}}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, None, &raw);
        assert_eq!(entry.status, "ok");
        assert!(entry.windows.is_empty());
    }

    // ===== 零凭证回退：MiniMax Code 本地登录态 =====

    /// 本机真实 auth.json 形态（记录名带 \u0000 分隔，key 不参与解析）。
    const LOCAL_AUTH_BODY: &str = r#"{
        "schemaVersion": 1,
        "records": {
            "com.minimax.mcode.oauth.prod.cn\u0000sub-1": {
                "schemaVersion": 1,
                "accessToken": "mmoat_local_token",
                "refreshToken": "mmort_local_token",
                "tokenType": "Bearer",
                "clientId": "mcode-public",
                "scopes": ["agent.default"],
                "audience": "agent-backend",
                "expiresAtMs": 1791839661268,
                "generation": 3
            }
        }
    }"#;

    fn auth_value(body: &str) -> serde_json::Value {
        serde_json::from_str(body).expect("测试 JSON 必须可解析")
    }

    /// 过期提前量：距过期不足 60 秒即不可用（防请求途中过期）。
    #[test]
    fn local_token_usable_respects_skew() {
        assert!(token_usable(1_000_000, 900_000)); // 剩 100s
        assert!(!token_usable(1_000_000, 940_001)); // 剩 59.999s
        assert!(!token_usable(1_000_000, 1_000_000)); // 已到期
        assert!(!token_usable(0, 0));
    }

    /// 解析 records：取 accessToken / expiresAtMs / clientId；空 token 跳过；
    /// 字符串数字容忍；缺 records 返回空表。
    #[test]
    fn local_parse_login_records() {
        let records = parse_login_records(&auth_value(LOCAL_AUTH_BODY));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].access_token, "mmoat_local_token");
        assert_eq!(records[0].expires_at_ms, 1_791_839_661_268);
        assert_eq!(records[0].client_id, "mcode-public");
        assert_eq!(records[0].region, None, "region 不在文件里，由目录名补");

        let mixed = r#"{"records":{
            "a":{"accessToken":"t1","expiresAtMs":"1700000000000","clientId":"c1"},
            "b":{"accessToken":"   ","expiresAtMs":1700000000000},
            "c":{"accessToken":"t3"},
            "d":"not-an-object"
        }}"#;
        let records = parse_login_records(&auth_value(mixed));
        assert_eq!(records.len(), 1, "空 token / 缺 expiresAtMs / 非对象都要跳过");
        assert_eq!(records[0].access_token, "t1");
        assert_eq!(records[0].expires_at_ms, 1_700_000_000_000);

        assert!(parse_login_records(&auth_value(r#"{"schemaVersion":1}"#)).is_empty());
    }

    /// 多条登录记录：只挑未过期的，且取最晚过期的一条（同一账号多代登录态并存）。
    #[test]
    fn local_pick_login_record_prefers_latest_usable() {
        let records = vec![
            LocalLogin {
                access_token: "old".into(),
                expires_at_ms: 1_000_000,
                client_id: "c".into(),
                region: None,
            },
            LocalLogin {
                access_token: "new".into(),
                expires_at_ms: 2_000_000,
                client_id: "c".into(),
                region: None,
            },
        ];
        let picked = pick_login_record(&records, 900_000).expect("应挑到未过期的一条");
        assert_eq!(picked.access_token, "new");
        // 全部过期 → None（静默无条目，不刷新）
        assert!(pick_login_record(&records, 3_000_000).is_none());
        assert!(pick_login_record(&[], 0).is_none());
    }

    /// 目录名 → 区域提示（三步容错）：已知区域名（cn/en/global/io）直接命中；
    /// 末段是 clientId 时取它前一段；层级不足时按最后一段兜底；空 → None。
    #[test]
    fn local_region_hint_from_path() {
        let comps = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        // 第 1 步：目录段里出现已知区域名（本机真实布局 prod/cn/mcode-public）
        assert_eq!(
            region_hint(&comps(&["prod", "cn", "mcode-public"]), "mcode-public"),
            Some("cn".to_string())
        );
        assert_eq!(
            region_hint(&comps(&["prod", "EN", "mcode-public"]), "mcode-public"),
            Some("en".to_string()),
            "大小写不敏感，且归一为小写"
        );
        // 第 2 步：没有已知区域名，但末段 == clientId → 取前一段
        assert_eq!(
            region_hint(&comps(&["prod", "ap-sg", "mcode-public"]), "mcode-public"),
            Some("ap-sg".to_string())
        );
        // 第 3 步：都不满足 → 最后一段兜底（由 host_for_region 决定默认国内站）
        assert_eq!(
            region_hint(&comps(&["mcode-public"]), "mcode-public"),
            Some("mcode-public".to_string())
        );
        // 空段 → None
        assert_eq!(region_hint(&comps(&[]), "mcode-public"), None);
    }

    /// 本地登录条目：只认 2062（pending 说明）与业务码 0 + 可解析双窗口（ok）；
    /// 其余一律 None（静默，不在面板刷错误）。
    #[test]
    fn local_entry_from_raw_handles_all_outcomes() {
        // 2062：已登录但无生效订阅 → pending + 中文说明（本机真实响应）
        let no_sub = ok_raw(
            r#"{"model_remains":null,"base_resp":{"status_code":2062,"status_msg":"no active token plan subscription"}}"#,
        );
        let entry = entry_from_local_raw(&no_sub, Some("cn")).expect("2062 应产出一条说明条目");
        assert_eq!(entry.credential_id, "local");
        assert_eq!(entry.status, "pending");
        assert!(entry.windows.is_empty());
        assert!(entry.message.unwrap().contains("没有生效的 Token Plan 订阅"));

        // 业务码 0 + 顶层 model_remains（开放平台端点没有 data 信封）→ ok 双窗口
        let ok_body = r#"{"model_remains":[{
            "model_name":"MiniMax-M2",
            "current_interval_total_count":1000,
            "current_interval_usage_count":300,
            "current_interval_remaining_percent":70,
            "current_weekly_total_count":5000,
            "current_weekly_usage_count":1200,
            "current_weekly_remaining_percent":76
        }],"base_resp":{"status_code":0,"status_msg":""}}"#;
        let entry = entry_from_local_raw(&ok_raw(ok_body), Some("cn")).expect("业务码 0 应产出条目");
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, "local");
        assert_eq!(entry.windows.len(), 2, "顶层 model_remains 要能补齐信封后被解析");
        assert_eq!(entry.windows[0].key, "interval");

        // 1004（主路径才认 sk-cp Key）/ 其它业务码 / 非 200 / 坏 JSON / 网络失败 → None
        let login_fail = ok_raw(
            r#"{"base_resp":{"status_code":1004,"status_msg":"login fail: Please carry the API secret key"}}"#,
        );
        assert!(entry_from_local_raw(&login_fail, None).is_none());
        assert!(entry_from_local_raw(&ok_raw(r#"{"base_resp":{"status_code":7}}"#), None).is_none());
        assert!(entry_from_local_raw(&Ok((500, Some("oops".into()))), None).is_none());
        assert!(entry_from_local_raw(&ok_raw("not json"), None).is_none());
        assert!(entry_from_local_raw(&Err("网络错误".into()), None).is_none());
        // 业务码 0 但 model_remains 缺失 → 凭证路径解析为 error，本地路径不产条目
        assert!(entry_from_local_raw(
            &ok_raw(r#"{"base_resp":{"status_code":0,"status_msg":""}}"#),
            None
        )
        .is_none());
    }

    /// 只有 2062 与业务码 0 会产出条目：确认 `fetch_local_login_entry` 的静默
    /// 纪律在「没有 auth.json」时不 panic、不产条目（临时空目录，不碰真实 home）。
    #[test]
    fn local_login_without_auth_file_is_silent() {
        let dir = std::env::temp_dir().join(format!("zbar-minimax-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        assert!(load_local_login(&dir, 1_000_000).is_none());
        assert!(load_local_login(&dir.join("missing"), 1_000_000).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真实目录布局能被发现：`<root>/auth/prod/cn/mcode-public/auth.json`（三层，
    /// 本机实测形态）。断言的读取结果与 region 提示都来自目录名，不需要联网，
    /// 也不碰真实 home。
    #[test]
    fn local_login_discovers_real_auth_layout() {
        let root = std::env::temp_dir().join(format!("zbar-minimax-auth-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("auth").join("prod").join("cn").join("mcode-public");
        std::fs::create_dir_all(&dir).expect("建 auth 目录失败");
        std::fs::write(dir.join("auth.json"), LOCAL_AUTH_BODY).expect("写 auth.json 失败");

        let login = load_local_login(&root.join("auth"), 1_700_000_000_000)
            .expect("应发现三层目录里的登录态");
        assert_eq!(login.access_token, "mmoat_local_token");
        assert_eq!(login.client_id, "mcode-public");
        assert_eq!(login.region.as_deref(), Some("cn"), "区域来自目录名");
        assert_eq!(
            host_for_region(login.region.as_deref()),
            "api.minimaxi.com",
            "cn 走国内站端点"
        );

        // 同一目录树里多份登录态（不同 clientId）→ 取最晚过期的一条
        let other = root.join("auth").join("prod").join("en").join("mcode-public");
        std::fs::create_dir_all(&other).expect("建第二份 auth 目录失败");
        let newer = LOCAL_AUTH_BODY.replace("1791839661268", "1900000000000");
        std::fs::write(other.join("auth.json"), newer).expect("写第二份 auth.json 失败");
        let login = load_local_login(&root.join("auth"), 1_700_000_000_000).expect("应有可用登录态");
        assert_eq!(login.region.as_deref(), Some("en"));
        assert_eq!(host_for_region(login.region.as_deref()), "api.minimax.io");

        let _ = std::fs::remove_dir_all(&root);
    }
}
