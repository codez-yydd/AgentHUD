//! DeepSeek 开放平台余额查询模块。
//!
//! 数据来源：GET https://api.deepseek.com/user/balance（Bearer API Key 鉴权）。
//! DeepSeek 无国内/国际站之分，凭证 region 忽略。
//!
//! 响应样例（余额三项均为字符串数字）：
//! `{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":
//!   "110.00","granted_balance":"10.00","topped_up_balance":"100.00"}]}`
//!
//! 映射：total_balance → balance.amount；granted_balance → granted（赠送）；
//! topped_up_balance → topped_up（充值）；currency 直接取服务端值。多条
//! balance_infos 时优先取 currency=="USD" 的，否则取第一条。
//! 401/403 → status="expired"；is_available=false 且余额非 0 → 仍为 ok，
//! 但 message 提示「余额暂不可用于 API 调用」。

use crate::provider_credentials::CredentialQuerySnapshot;
use crate::provider_quota::{
    flatten_response, now_ms, parse_flexible_f64, quota_http_agent, ProviderQuotaBalance,
    ProviderQuotaEntry,
};
use std::sync::OnceLock;

/// 余额接口基址（无区域之分）。
const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";

/// 逐凭证查询 DeepSeek 余额（串行；单凭证失败产出 error/expired 条目，
/// 不阻塞其他凭证）。由 provider_quota 骨架分发调用。
pub(crate) fn fetch_quota_entries(
    snapshots: &[CredentialQuerySnapshot],
) -> Vec<ProviderQuotaEntry> {
    let agent = quota_http_agent();
    snapshots
        .iter()
        .map(|cred| {
            let raw = fetch_balance_raw(&agent, cred);
            entry_from_raw(&cred.id, &cred.label, &raw)
        })
        .collect()
}

/// 单凭证余额查询（网络层）：GET https://api.deepseek.com/user/balance。
/// 返回展平的 (HTTP 状态码, 响应体)；网络层彻底失败返回 Err（中文原因，
/// 不含 secret）。解析交给 entry_from_raw 纯函数（单测不联网）。
fn fetch_balance_raw(
    agent: &ureq::Agent,
    cred: &CredentialQuerySnapshot,
) -> Result<(u16, Option<String>), String> {
    let resp = agent
        .get(BALANCE_URL)
        .set("Authorization", &format!("Bearer {}", cred.secret))
        .set("Accept", "application/json")
        .call();
    flatten_response(resp).map_err(|e| format!("DeepSeek 余额{e}"))
}

/// 从余额/钱包数组里挑展示条目（纯函数）：优先 currency=="USD" 的，否则第一条。
/// 凭证路径（balance_infos）与 DSH 本地路径（normal_wallets / bonus_wallets）
/// 共用此口径，避免两处各写一份「挑 USD」逻辑。
fn pick_usd_or_first(infos: &[serde_json::Value]) -> Option<&serde_json::Value> {
    infos
        .iter()
        .find(|i| i.get("currency").and_then(|c| c.as_str()) == Some("USD"))
        .or_else(|| infos.first())
}

/// 从 balance_infos 中挑展示条目（纯函数，便于单测）：优先 currency=="USD"
/// 的，否则第一条；缺失/为空返回 None（由调用方报 error）。
fn pick_balance_info(v: &serde_json::Value) -> Option<&serde_json::Value> {
    pick_usd_or_first(v.get("balance_infos")?.as_array()?)
}

/// 解析单凭证查询结果 → 展示条目（纯函数，网络无关，单测直接构造输入）。
/// 分支优先级：网络失败(error) > 401/403(expired) > 非 200(error) >
/// body 解析失败/缺 balance_infos(error) > 成功(ok + 余额，is_available=false
/// 且余额非 0 时附加提示消息)。
fn entry_from_raw(
    cred_id: &str,
    label: &str,
    raw: &Result<(u16, Option<String>), String>,
) -> ProviderQuotaEntry {
    // 失败条目构造（windows 恒空、无余额；message 承载原因）
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
        return fail("error", format!("DeepSeek 余额{}", raw.as_ref().unwrap_err()));
    };
    // Key 被服务端拒绝：视为凭证过期（凭证卡显示「已过期」徽章）
    if *http_status == 401 || *http_status == 403 {
        return fail("expired", "API Key 无效或已过期".to_string());
    }
    if *http_status != 200 {
        return fail("error", format!("DeepSeek 余额查询失败（HTTP {http_status}）"));
    }
    let Some(body) = body.as_deref() else {
        return fail("error", "DeepSeek 余额响应为空".to_string());
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return fail("error", "DeepSeek 余额响应解析失败".to_string());
    };
    let Some(info) = pick_balance_info(&v) else {
        return fail("error", "DeepSeek 余额响应缺少 balance_infos".to_string());
    };
    let Some(amount) = info.get("total_balance").and_then(parse_flexible_f64) else {
        return fail("error", "DeepSeek 余额响应缺少 total_balance".to_string());
    };
    // is_available=false 且余额非 0：账户异常冻结等场景，余额仍展示但提示
    // 暂不可用（status 保持 ok，不算凭证失效）；余额为 0 时不提示（新账号常态）
    let unavailable = v.get("is_available").and_then(|b| b.as_bool()) == Some(false);
    let message = if unavailable && amount != 0.0 {
        Some("余额暂不可用于 API 调用".to_string())
    } else {
        None
    };
    ProviderQuotaEntry {
        credential_id: cred_id.to_string(),
        label: label.to_string(),
        status: "ok".to_string(),
        windows: vec![],
        balance: Some(ProviderQuotaBalance {
            amount,
            // 币种直接取服务端值（"CNY" / "USD"）
            currency: info
                .get("currency")
                .and_then(|c| c.as_str())
                .unwrap_or("CNY")
                .to_string(),
            granted: info.get("granted_balance").and_then(parse_flexible_f64),
            topped_up: info.get("topped_up_balance").and_then(parse_flexible_f64),
            // 凭证通道余额接口无累计消费数据源，恒为 None（仅 DSH 本地登录通路有值）
            total_cost: None,
        }),
        plan_name: None,
        message,
        updated_at: now_ms(),
    }
}

// ============================================================
// 零凭证回退：DeepSeek Harness (DSH) 本地登录态（平台 grant token）
// ============================================================

/// 本地登录条目的 credential_id：不对应凭证体系任何条目，provider_quota 的
/// 校验回写按此跳过（与 gemini/minimax/grok 的本地条目同口径）。
const LOCAL_CREDENTIAL_ID: &str = "local";

/// 本地登录条目标签（凭证名位置展示；credential_id="local" 另有「本地」徽章）。
const LOCAL_LABEL: &str = "DSH 登录";

/// DSH 平台余额接口（已真机实测可用）：必须带完整浏览器特征头，否则被 WAF
/// 拦成 HTTP 429「Request Blocked」HTML。鉴权头是 x-dsh-auth-token，非 Bearer。
const LOCAL_SUMMARY_URL: &str = "https://platform.deepseek.com/api/v0/users/get_user_summary";

/// User-Agent 模板：{version} 由下方 DSH 版本探测填充（版本号不影响鉴权）。
const LOCAL_UA_TEMPLATE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) DeepSeek Harness/{version} Chrome/132.0.6834.83 Electron/34.0.0 Safari/537.36";

/// DSH 版本兜底常量：探测失败时用它（接口已实测不校验版本号，仅为仿真完整）。
const LOCAL_VERSION_FALLBACK: &str = "44.7.0";

/// 登录态失效消息（401 或业务 code==40003）。
const LOCAL_EXPIRED_MESSAGE: &str = "DSH 登录态已过期,请在 DeepSeek Harness 重新登录";

/// 常见 DSH 安装目录（用于读 version 文件；不存在即跳过，探测失败不 panic）。
const LOCAL_VERSION_SEARCH_DIRS: &[&str] = &[
    "Programs/deepseek-harness",
    "Programs/DeepSeek Harness",
];

/// 读单个 version 文件的候选内容（取首个非空行，仅接受数字/点组成的版本号）。
fn version_from_file(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().find_map(|l| {
        let l = l.trim();
        (!l.is_empty()).then_some(l)
    })?;
    let valid = !line.is_empty()
        && line.len() <= 32
        && line.chars().all(|c| c.is_ascii_digit() || c == '.');
    valid.then(|| line.to_string())
}

/// 探测 DSH 版本（可选）：从常见安装目录的 version 文件读取，读不到用兜底常量。
/// 探测过程只读文件、失败静默，绝不 panic。
fn detect_local_version() -> String {
    // D:\app\DeepSeek Harness 这类盘符安装路径也纳入探测（Windows 常见部署形态）。
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        if !local.trim().is_empty() {
            for rel in LOCAL_VERSION_SEARCH_DIRS {
                dirs.push(std::path::PathBuf::from(&local).join(rel));
            }
        }
    }
    dirs.push(std::path::PathBuf::from(r"D:\app\DeepSeek Harness"));
    for name in ["version", "version.txt", "VERSION"] {
        for dir in &dirs {
            if let Some(v) = version_from_file(&dir.join(name)) {
                return v;
            }
        }
    }
    LOCAL_VERSION_FALLBACK.to_string()
}

/// 进程内缓存的 DSH 版本（OnceLock：首次探测后不再读盘）。
fn local_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(detect_local_version)
}

/// 本地登录条目的骨架（credential_id 固定 "local"，跳过校验回写）。
fn local_entry(status: &str, message: Option<String>, balance: Option<ProviderQuotaBalance>) -> ProviderQuotaEntry {
    ProviderQuotaEntry {
        credential_id: LOCAL_CREDENTIAL_ID.to_string(),
        label: LOCAL_LABEL.to_string(),
        status: status.to_string(),
        windows: vec![],
        balance,
        plan_name: None,
        message,
        updated_at: now_ms(),
    }
}

/// DSH 平台余额响应 → 展示条目（纯函数，网络无关，单测直接构造输入）。
///
/// 只认三种可解释形态，其余一律 None（静默，对齐 minimax 先例：本地登录是补充
/// 来源，不该在面板上刷错误条目）：
/// - HTTP 401 或业务 code/biz_code==40003 → 一条 expired 条目（提示重新登录）；
/// - HTTP 200 且 code==0、biz_code==0、normal_wallets 非空 → ok 条目（余额 +
///   赠送余额 + 累计消费）；normal 按「优先 USD 否则第一条」取 amount/currency，
///   bonus 里找同 currency 条目的 balance 作 granted（没有则省略），total_costs
///   里找同 currency 条目的 amount 作 total_cost（没有则省略）；
/// - 429（WAF 拦截，响应是 HTML）/ 其他非 200 / body 非 JSON / code!=0 /
///   biz_data 缺失 / normal_wallets 为空 / 网络失败 → None。
fn entry_from_local_raw(raw: &Result<(u16, Option<String>), String>) -> Option<ProviderQuotaEntry> {
    let Ok((http_status, body)) = raw else {
        return None; // 网络失败：静默
    };
    if *http_status == 401 {
        return Some(local_entry("expired", Some(LOCAL_EXPIRED_MESSAGE.to_string()), None));
    }
    if *http_status != 200 {
        return None; // 429（WAF HTML）及其他非 200：静默
    }
    let body = body.as_deref()?;
    let v = serde_json::from_str::<serde_json::Value>(body).ok()?; // 非 JSON：静默

    let code = v.get("code").and_then(parse_flexible_f64)?;
    if code == 40003.0 {
        return Some(local_entry("expired", Some(LOCAL_EXPIRED_MESSAGE.to_string()), None));
    }
    if code != 0.0 {
        return None;
    }
    let data = v.get("data")?;
    let biz_code = data.get("biz_code").and_then(parse_flexible_f64)?;
    if biz_code == 40003.0 {
        return Some(local_entry("expired", Some(LOCAL_EXPIRED_MESSAGE.to_string()), None));
    }
    if biz_code != 0.0 {
        return None;
    }
    let biz_data = data.get("biz_data")?;
    let normals = biz_data.get("normal_wallets")?.as_array()?;
    let normal = pick_usd_or_first(normals)?;
    let amount = normal.get("balance").and_then(parse_flexible_f64)?;
    let currency = normal
        .get("currency")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
        .unwrap_or("CNY")
        .to_string();
    // 赠送余额：只在 bonus_wallets 里有同币种条目时取（无则省略 granted）
    let granted = biz_data
        .get("bonus_wallets")
        .and_then(|b| b.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|w| w.get("currency").and_then(|c| c.as_str()) == Some(currency.as_str()))
        })
        .and_then(|w| w.get("balance").and_then(parse_flexible_f64));
    // 累计消费：与 granted 同口径，只在 total_costs 里有同币种条目时取（无则省略）
    // 选「第一条同币种」——与 granted/normal 的 find 口径一致，避免跨币种误取
    let total_cost = biz_data
        .get("total_costs")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|w| w.get("currency").and_then(|c| c.as_str()) == Some(currency.as_str()))
        })
        .and_then(|w| w.get("amount").and_then(parse_flexible_f64));

    Some(local_entry(
        "ok",
        None,
        Some(ProviderQuotaBalance {
            amount,
            currency,
            granted,
            topped_up: None,
            total_cost,
        }),
    ))
}

/// DSH 平台余额查询（网络层）：照抄实测成功的完整请求头集合。
/// 时区偏移用 chrono::Local 的本地偏移秒（东八区为正，即 28800）；
/// UA 按模板拼 DSH 版本。网络层失败返回 Err（中文原因，不含 token）。
fn fetch_local_grant_raw(
    agent: &ureq::Agent,
    token: &str,
    version: &str,
) -> Result<(u16, Option<String>), String> {
    let offset_secs = chrono::Local::now().offset().local_minus_utc().to_string();
    let ua = LOCAL_UA_TEMPLATE.replace("{version}", version);
    let resp = agent
        .get(LOCAL_SUMMARY_URL)
        .set("x-dsh-auth-token", token)
        .set("x-client-bundle-id", "")
        .set("x-client-platform", "desktop-win")
        .set("x-client-version", version)
        .set("x-client-locale", "zh_CN")
        .set("x-client-timezone-offset", &offset_secs)
        .set("User-Agent", &ua)
        .set("Accept", "application/json, text/plain, */*")
        .set("Accept-Language", "zh-CN,zh;q=0.9")
        .set("Referer", "https://platform.deepseek.com/")
        .set("Origin", "https://platform.deepseek.com")
        .call();
    flatten_response(resp).map_err(|e| format!("DSH 登录态余额查询{e}"))
}

/// 零凭证回退入口：DSH 本地登录态（平台 grant token）→ 平台余额条目。
/// 无 grant token / 网络失败 / 非成功非过期响应一律 None（静默）——本地登录是
/// 补充来源，不该在面板上刷错误条目。由 provider_quota 在 deepseek 一条凭证
/// 条目都没有时调用。
pub(crate) fn fetch_local_login_entry() -> Option<ProviderQuotaEntry> {
    let token = crate::dsh::read_platform_grant()?;
    let raw = fetch_local_grant_raw(&quota_http_agent(), &token, local_version());
    entry_from_local_raw(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRED_ID: &str = "ds-1";
    const LABEL: &str = "充值账号";

    fn ok_raw(body: &str) -> Result<(u16, Option<String>), String> {
        Ok((200, Some(body.to_string())))
    }

    /// 成功路径（样例响应）：金额/赠送/充值解析 + 币种取服务端值。
    #[test]
    fn parses_sample_balance() {
        let raw = ok_raw(
            r#"{"is_available":true,"balance_infos":[{"currency":"CNY",
               "total_balance":"110.00","granted_balance":"10.00",
               "topped_up_balance":"100.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, CRED_ID);
        assert_eq!(entry.label, LABEL);
        assert!(entry.windows.is_empty());
        assert!(entry.message.is_none());
        let balance = entry.balance.expect("成功条目必须有余额");
        assert_eq!(balance.amount, 110.0);
        assert_eq!(balance.currency, "CNY");
        assert_eq!(balance.granted, Some(10.0));
        assert_eq!(balance.topped_up, Some(100.0));
    }

    /// 多条 balance_infos：优先 USD，否则第一条（保持缺 USD 时的现状）。
    #[test]
    fn prefers_usd_balance_info() {
        // 有 USD 条目：取 USD
        let raw = ok_raw(
            r#"{"is_available":true,"balance_infos":[
               {"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"},
               {"currency":"USD","total_balance":"15.00","granted_balance":"1.00","topped_up_balance":"14.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        let balance = entry.balance.expect("余额存在");
        assert_eq!(balance.currency, "USD");
        assert_eq!(balance.amount, 15.0);
        assert_eq!(balance.topped_up, Some(14.0));

        // 无 USD 条目：回退第一条
        let raw = ok_raw(
            r#"{"is_available":true,"balance_infos":[
               {"currency":"CNY","total_balance":"88.00","granted_balance":"8.00","topped_up_balance":"80.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.balance.expect("余额存在").amount, 88.0);
    }

    /// is_available=false 且余额非 0 → status=ok + 提示消息；余额为 0 不提示。
    #[test]
    fn unavailable_balance_keeps_ok_with_message() {
        let raw = ok_raw(
            r#"{"is_available":false,"balance_infos":[{"currency":"CNY",
               "total_balance":"50.00","granted_balance":"0.00",
               "topped_up_balance":"50.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        assert_eq!(
            entry.message.as_deref(),
            Some("余额暂不可用于 API 调用")
        );
        assert_eq!(entry.balance.expect("余额仍应展示").amount, 50.0);

        // 余额为 0 + is_available=false（新账号常态）：不提示
        let raw = ok_raw(
            r#"{"is_available":false,"balance_infos":[{"currency":"CNY",
               "total_balance":"0.00","granted_balance":"0.00","topped_up_balance":"0.00"}]}"#,
        );
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "ok");
        assert!(entry.message.is_none());
    }

    /// 401/403 → expired「API Key 无效或已过期」（假 Key 手测链路的预期分支）。
    #[test]
    fn unauthorized_maps_to_expired() {
        for status in [401u16, 403] {
            let raw = Ok((status, Some("invalid key".to_string())));
            let entry = entry_from_raw(CRED_ID, LABEL, &raw);
            assert_eq!(entry.status, "expired", "HTTP {status} 应判定为 expired");
            assert_eq!(entry.message.as_deref(), Some("API Key 无效或已过期"));
            assert!(entry.balance.is_none());
        }
    }

    /// 缺 balance_infos / 非 JSON body → error；网络失败 → error。
    #[test]
    fn malformed_responses_map_to_error() {
        let raw = ok_raw(r#"{"is_available":true}"#);
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("balance_infos"));

        let raw = Ok((200, Some("not json".to_string())));
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("解析失败"));

        let raw: Result<(u16, Option<String>), String> =
            Err("网络错误或服务不可用: connection timed out".to_string());
        let entry = entry_from_raw(CRED_ID, LABEL, &raw);
        assert_eq!(entry.status, "error");
        assert!(entry.message.unwrap().contains("网络错误或服务不可用"));
    }

    // ============================================================
    // DSH 本地登录态（平台 grant token）解析测试
    // ============================================================

    /// 实测成功样例：normal（充值钱包）+ bonus（赠送钱包）+ total_costs 同 CNY 币种。
    /// 金额取 normal_wallets 的 balance；granted 取 bonus 同币种 balance；
    /// total_cost 取 total_costs 同币种 amount；topped_up 省略。
    #[test]
    fn local_parses_sample_with_bonus() {
        let raw = ok_raw(
            r#"{"code":0,"msg":"","data":{"biz_code":0,"biz_msg":"","biz_data":{
               "normal_wallets":[{"currency":"CNY","balance":"1.3654366000000000","token_estimation":"0"}],
               "bonus_wallets":[{"currency":"CNY","balance":"0.5","token_estimation":"0"}],
               "total_costs":[{"currency":"CNY","amount":"33.6368975200000000"}]}}}"#,
        );
        let entry = entry_from_local_raw(&raw).expect("成功样例应产出条目");
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, LOCAL_CREDENTIAL_ID);
        assert_eq!(entry.label, LOCAL_LABEL);
        assert!(entry.windows.is_empty());
        assert!(entry.message.is_none());
        let balance = entry.balance.expect("成功条目必须有余额");
        assert!((balance.amount - 1.3654366).abs() < 1e-9);
        assert_eq!(balance.currency, "CNY");
        assert_eq!(balance.granted, Some(0.5));
        assert_eq!(balance.topped_up, None);
        assert!(balance.total_cost.is_some());
        assert!((balance.total_cost.unwrap() - 33.63689752).abs() < 1e-9);
    }

    /// bonus 与 normal 币种不同 → granted 省略（不跨币种取赠送）。
    #[test]
    fn local_omits_granted_when_bonus_currency_differs() {
        let raw = ok_raw(
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{
               "normal_wallets":[{"currency":"CNY","balance":"10"}],
               "bonus_wallets":[{"currency":"USD","balance":"2"}]}}}"#,
        );
        let entry = entry_from_local_raw(&raw).expect("应产出条目");
        let balance = entry.balance.expect("余额存在");
        assert_eq!(balance.currency, "CNY");
        assert_eq!(balance.amount, 10.0);
        assert_eq!(balance.granted, None, "币种不同不应取 bonus");
    }

    /// bonus_wallets 缺失 → granted 省略，其余正常产出。
    #[test]
    fn local_omits_granted_when_no_bonus_wallets() {
        let raw = ok_raw(
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{
               "normal_wallets":[{"currency":"CNY","balance":"7.5"}]}}}"#,
        );
        let entry = entry_from_local_raw(&raw).expect("应产出条目");
        let balance = entry.balance.expect("余额存在");
        assert_eq!(balance.amount, 7.5);
        assert_eq!(balance.granted, None);
    }

    /// total_costs 缺失 / 无同币种条目 → total_cost 省略（不跨币种误取）。
    #[test]
    fn local_omits_total_cost_when_missing_or_currency_differs() {
        // total_costs 字段完全缺失 → None
        let raw = ok_raw(
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{
               "normal_wallets":[{"currency":"CNY","balance":"7.5"}]}}}"#,
        );
        let entry = entry_from_local_raw(&raw).expect("应产出条目");
        assert_eq!(entry.balance.expect("余额存在").total_cost, None);

        // total_costs 只有 USD 条目，normal 选中的是 CNY → None（不跨币种）
        let raw = ok_raw(
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{
               "normal_wallets":[{"currency":"CNY","balance":"7.5"}],
               "total_costs":[{"currency":"USD","amount":"9.9"}]}}}"#,
        );
        let entry = entry_from_local_raw(&raw).expect("应产出条目");
        assert_eq!(entry.balance.expect("余额存在").total_cost, None);

        // total_costs 有同币种条目但 amount 缺失 → None
        let raw = ok_raw(
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{
               "normal_wallets":[{"currency":"CNY","balance":"7.5"}],
               "total_costs":[{"currency":"CNY"}]}}}"#,
        );
        let entry = entry_from_local_raw(&raw).expect("应产出条目");
        assert_eq!(entry.balance.expect("余额存在").total_cost, None);
    }

    /// 多条钱包：优先 USD（与凭证口径一致，复用 pick_usd_or_first）。
    #[test]
    fn local_prefers_usd_wallet() {
        let raw = ok_raw(
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{
               "normal_wallets":[
                 {"currency":"CNY","balance":"100"},
                 {"currency":"USD","balance":"15"}]}}}"#,
        );
        let entry = entry_from_local_raw(&raw).expect("应产出条目");
        let balance = entry.balance.expect("余额存在");
        assert_eq!(balance.currency, "USD");
        assert_eq!(balance.amount, 15.0);
    }

    /// HTTP 401 → expired（无余额）。
    #[test]
    fn local_401_maps_to_expired() {
        let raw = Ok((401, Some("Unauthorized".to_string())));
        let entry = entry_from_local_raw(&raw).expect("401 应产出 expired 条目");
        assert_eq!(entry.status, "expired");
        assert_eq!(entry.message.as_deref(), Some(LOCAL_EXPIRED_MESSAGE));
        assert!(entry.balance.is_none());
    }

    /// 业务 code==40003 或 biz_code==40003 → expired（无余额）。
    #[test]
    fn local_code_40003_maps_to_expired() {
        let raw = ok_raw(r#"{"code":40003,"msg":"login expired"}"#);
        let entry = entry_from_local_raw(&raw).expect("code 40003 应产出 expired 条目");
        assert_eq!(entry.status, "expired");
        assert_eq!(entry.message.as_deref(), Some(LOCAL_EXPIRED_MESSAGE));
        assert!(entry.balance.is_none());

        let raw = ok_raw(
            r#"{"code":0,"data":{"biz_code":40003,"biz_data":{}}}"#,
        );
        let entry = entry_from_local_raw(&raw).expect("biz_code 40003 应产出 expired 条目");
        assert_eq!(entry.status, "expired");
        assert!(entry.balance.is_none());
    }

    /// 429（WAF HTML）/ 其他非 200 / 非 JSON / 网络失败 → None（静默）。
    #[test]
    fn local_waf_and_network_failures_are_silent() {
        let waf = Ok((429, Some("<html>Request Blocked</html>".to_string())));
        assert!(entry_from_local_raw(&waf).is_none());
        let other = Ok((500, Some("server error".to_string())));
        assert!(entry_from_local_raw(&other).is_none());
        let non_json = Ok((200, Some("not json".to_string())));
        assert!(entry_from_local_raw(&non_json).is_none());
        let net_err: Result<(u16, Option<String>), String> = Err("网络错误或服务不可用".to_string());
        assert!(entry_from_local_raw(&net_err).is_none());
    }

    /// code!=0 / biz_data 缺失 / normal_wallets 为空 → None（静默）。
    #[test]
    fn local_unknown_or_empty_responses_are_silent() {
        let bad_code = ok_raw(r#"{"code":1,"msg":"error"}"#);
        assert!(entry_from_local_raw(&bad_code).is_none());
        let no_biz = ok_raw(r#"{"code":0,"data":{"biz_code":0}}"#);
        assert!(entry_from_local_raw(&no_biz).is_none());
        let empty_normals = ok_raw(
            r#"{"code":0,"data":{"biz_code":0,"biz_data":{"normal_wallets":[]}}}"#,
        );
        assert!(entry_from_local_raw(&empty_normals).is_none());
        let no_normals = ok_raw(r#"{"code":0,"data":{"biz_code":0,"biz_data":{}}}"#);
        assert!(entry_from_local_raw(&no_normals).is_none());
    }
}
