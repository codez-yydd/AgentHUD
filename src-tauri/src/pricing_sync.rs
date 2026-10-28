//! 模型价格与同步服务的一致性（桌面端 <-> 服务端，iOS 只读）。
//!
//! 设计约束：
//! - **没有配置同步服务时**：只读写本机 ~/.agenthud/pricing.json，不产生
//!   任何网络请求，也不写 pending 状态；
//! - **配置了服务端时**：服务端价格表是全局唯一权威源。本地编辑先落
//!   pending，下一次同步按模型增量推送；推送成功后用服务端返回的整表
//!   覆盖本地，从而让所有桌面端/手机端最终收敛到同一份价格；
//! - 不同设备改不同模型互不覆盖；同一模型以服务端最后接收的写入为准，
//!   响应里的 concurrent 标记用于桌面端提示“已与其他设备合并”。
//! - 服务端不可达/未升级时 pending 保留，本地照常计费，下一轮重试。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::pricing::{self, ModelPrice, PricingConfig};
use crate::sync::{agent_with_timeouts, map_http_err};

/// 本地价格同步状态（~/.agenthud/pricing_sync.json）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PricingSyncState {
    /// 最后一次成功对齐的服务端 revision
    #[serde(default)]
    pub revision: i64,
    /// 是否已完成首次对齐（首次需要判断服务端是否为空）
    #[serde(default)]
    pub initialized: bool,
    /// 待推送的模型级变更：Some=新增/修改，None=删除
    #[serde(default)]
    pub pending: BTreeMap<String, Option<ModelPrice>>,
    /// 待推送的隐藏名单（整表）
    #[serde(default)]
    pub pending_hidden: Option<Vec<String>>,
    /// 最后一次对齐时使用的汇率；用于发现本机汇率变化
    #[serde(default)]
    pub last_fx: Option<f64>,
}

/// 服务端 /pricing 的返回（GET / POST 同构）。
#[derive(Debug, Clone, Deserialize)]
struct PricingTableResponse {
    #[serde(default)]
    revision: i64,
    #[serde(default)]
    updated_at: i64,
    #[serde(default)]
    updated_by: String,
    #[serde(default)]
    usd: BTreeMap<String, ModelPrice>,
    #[serde(default)]
    hidden: Vec<String>,
    #[serde(default)]
    fx_rate: f64,
    #[serde(default)]
    synced: bool,
    #[serde(default)]
    applied: usize,
    #[serde(default)]
    concurrent: bool,
}

#[derive(Debug, Serialize)]
struct PricingPushPayload<'a> {
    base_revision: i64,
    changes: &'a BTreeMap<String, Option<ModelPrice>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hidden: Option<&'a Vec<String>>,
    fx_rate: f64,
}

/// 状态文件读写锁（只保护毫秒级文件操作，不覆盖网络等待）。
fn state_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 同进程内串行化整轮价格同步（后台同步 + 手动同步可能同时触发）。
fn sync_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn state_path() -> Result<PathBuf, String> {
    Ok(pricing::config_dir()?.join("pricing_sync.json"))
}

fn read_state_locked() -> PricingSyncState {
    let path = match state_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[zbar-pricing] 无法定位价格同步状态文件: {e}");
            return PricingSyncState::default();
        }
    };
    if !path.exists() {
        return PricingSyncState::default();
    }
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(state) => state,
        Err(e) => {
            eprintln!("[zbar-pricing] 价格同步状态损坏，按未初始化处理: {e}");
            PricingSyncState::default()
        }
    }
}

fn write_state_locked(state: &PricingSyncState) -> Result<(), String> {
    let path = state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(state)
        .map_err(|e| format!("序列化价格同步状态失败: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写入价格同步状态失败: {e}"))
}

fn valid_fx(rate: f64) -> Option<f64> {
    if rate.is_finite() && rate > 0.0 && rate <= 1000.0 {
        Some(rate)
    } else {
        None
    }
}

fn local_fx_rate() -> f64 {
    crate::cursor::load_cursor_config()
        .ok()
        .and_then(|c| valid_fx(c.usd_cny_rate))
        .unwrap_or(7.2)
}

/// 记录一次本地价格编辑（set_pricing 保存后调用）。
///
/// 仅在已配置同步服务时记录：未使用服务端的用户不会产生额外文件；配置
/// 后第一轮同步会把本地价格作为初始表发布（服务端为空时）或对齐服务端。
pub fn record_local_change(old: &PricingConfig, new: &PricingConfig) {
    let cfg = crate::sync::load_sync_config().unwrap_or_default();
    if !sync_configured(&cfg) {
        return;
    }
    let _guard = lock(state_lock());
    let mut state = read_state_locked();
    let changed = diff_into(&mut state, old, new);
    if !changed {
        return;
    }
    if let Err(e) = write_state_locked(&state) {
        eprintln!("[zbar-pricing] 记录待同步价格失败: {e}");
    }
}

/// 是否配置了同步服务。未配置（只有本机）时价格编辑完全本地，不应
/// 创建 pending/触发任何网络请求。
fn sync_configured(cfg: &crate::sync::SyncConfig) -> bool {
    !cfg.server_url.is_empty() && !cfg.device_token.is_empty()
}

/// 纯函数：把 old→new 的差异合并进 state，返回是否有变化。
fn diff_into(state: &mut PricingSyncState, old: &PricingConfig, new: &PricingConfig) -> bool {
    let mut changed = false;
    let keys: BTreeSet<&String> = old.usd.keys().chain(new.usd.keys()).collect();
    for key in keys {
        if old.usd.get(key) != new.usd.get(key) {
            state.pending.insert(key.to_string(), new.usd.get(key).cloned());
            changed = true;
        }
    }
    if old.hidden != new.hidden {
        state.pending_hidden = Some(new.hidden.clone());
        changed = true;
    }
    changed
}

/// 把模型级变更应用到一张价格表（Some 写入、None 删除）。
fn apply_changes(
    usd: &mut BTreeMap<String, ModelPrice>,
    changes: &BTreeMap<String, Option<ModelPrice>>,
) {
    for (model_id, price) in changes {
        match price {
            Some(price) => {
                usd.insert(model_id.clone(), price.clone());
            }
            None => {
                usd.remove(model_id);
            }
        }
    }
}

/// 重置与服务端的对齐状态（断开连接/换服务端时调用）。
///
/// 只清 revision/initialized/汇率游标，**保留本地尚未推送的 pending**：
/// 用户改完价格后如果先断开再重连，本地修改仍会在下一轮同步推送，不会
/// 因为 reset 丢掉。
pub fn reset_state() {
    let _guard = lock(state_lock());
    let path = match state_path() {
        Ok(p) => p,
        Err(_) => return,
    };
    if !path.exists() {
        return;
    }
    let mut state = read_state_locked();
    state.revision = 0;
    state.initialized = false;
    state.last_fx = None;
    if let Err(e) = write_state_locked(&state) {
        eprintln!("[zbar-pricing] 重置价格同步状态失败: {e}");
    }
}

fn changes_from_local(local: &PricingConfig) -> BTreeMap<String, Option<ModelPrice>> {
    local
        .usd
        .iter()
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect()
}

fn clear_pending_and_init() {
    let _guard = lock(state_lock());
    let mut state = read_state_locked();
    state.pending.clear();
    state.pending_hidden = None;
    state.initialized = true;
    let _ = write_state_locked(&state);
}

/// 与同步服务对齐一次价格表。
///
/// 调用方（sync.rs）只在已启用同步且带 device_token 时调用；失败只记日志，
/// 由下一轮重试。返回 Ok(()) 表示“本轮已完成”（可能无变化）。
pub fn sync_with_server(base: &str, token: &str) -> Result<(), String> {
    let _serial = lock(sync_lock());
    let local = pricing::load_pricing().unwrap_or_default();
    let local_fx = local_fx_rate();

    let (state, has_pending) = {
        let _guard = lock(state_lock());
        let state = read_state_locked();
        let has_pending = !state.pending.is_empty() || state.pending_hidden.is_some();
        (state, has_pending)
    };

    if has_pending {
        // 推送前先确认服务端表还在。服务端被清空/换库时若只推 pending，
        // 会把“少数改动模型”当成整表写回并覆盖本地其余模型；此时必须
        // 用本机完整表重建。
        let probe = fetch(base, token)?;
        if !probe.synced {
            if local.usd.is_empty() {
                clear_pending_and_init();
                return Ok(());
            }
            let changes = changes_from_local(&local);
            let hidden = Some(local.hidden.clone());
            let response = push(base, token, 0, &changes, hidden.as_ref(), local_fx)?;
            commit(response, &changes, &hidden, local_fx)?;
            return Ok(());
        }
        let response = push(
            base,
            token,
            state.revision,
            &state.pending,
            state.pending_hidden.as_ref(),
            local_fx,
        )?;
        commit(response, &state.pending, &state.pending_hidden, local_fx)?;
        return Ok(());
    }

    if !state.initialized {
        let response = fetch(base, token)?;
        if response.synced {
            commit(response, &BTreeMap::new(), &None, local_fx)?;
            return Ok(());
        }
        // 服务端还没有价格表：把本机价格作为初始版本发布
        if local.usd.is_empty() {
            let _guard = lock(state_lock());
            let mut state = read_state_locked();
            if !state.initialized {
                state.initialized = true;
                let _ = write_state_locked(&state);
            }
            return Ok(());
        }
        let changes = changes_from_local(&local);
        let hidden = Some(local.hidden.clone());
        let response = push(base, token, 0, &changes, hidden.as_ref(), local_fx)?;
        commit(response, &changes, &hidden, local_fx)?;
        return Ok(());
    }

    // 先确认服务端表还在：若被清空/换库，必须用本机整表重建，绝不能
    // 只推一个 fx 变更把空表升级成“已同步”后覆盖本机价格。
    let response = fetch(base, token)?;
    if !response.synced {
        if local.usd.is_empty() {
            return Ok(());
        }
        let changes = changes_from_local(&local);
        let hidden = Some(local.hidden.clone());
        let pushed = push(base, token, 0, &changes, hidden.as_ref(), local_fx)?;
        commit(pushed, &changes, &hidden, local_fx)?;
        return Ok(());
    }

    // 已对齐：本机汇率变化时单独推送一次（价格未变也要让手机端汇率跟上）
    if state.last_fx.map(|fx| (fx - local_fx).abs() > 1e-9).unwrap_or(false) {
        let empty = BTreeMap::new();
        let pushed = push(base, token, state.revision, &empty, None, local_fx)?;
        commit(pushed, &empty, &None, local_fx)?;
        return Ok(());
    }

    let remote_fx = valid_fx(response.fx_rate);
    let drifted = response.revision > state.revision
        || response.usd != local.usd
        || response.hidden != local.hidden
        || remote_fx.map(|fx| (fx - local_fx).abs() > 1e-9).unwrap_or(false);
    if drifted {
        commit(response, &BTreeMap::new(), &None, local_fx)?;
    }
    Ok(())
}

fn push(
    base: &str,
    token: &str,
    base_revision: i64,
    changes: &BTreeMap<String, Option<ModelPrice>>,
    hidden: Option<&Vec<String>>,
    fx_rate: f64,
) -> Result<PricingTableResponse, String> {
    let payload = PricingPushPayload {
        base_revision,
        changes,
        hidden,
        fx_rate,
    };
    agent_with_timeouts(3, 10)
        .post(&format!("{base}/pricing"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(&payload)
        .map_err(map_http_err("同步价格表"))?
        .into_json::<PricingTableResponse>()
        .map_err(|e| format!("解析价格表响应失败: {e}"))
}

fn fetch(base: &str, token: &str) -> Result<PricingTableResponse, String> {
    agent_with_timeouts(3, 10)
        .get(&format!("{base}/pricing"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(map_http_err("读取价格表"))?
        .into_json::<PricingTableResponse>()
        .map_err(|e| format!("解析价格表响应失败: {e}"))
}

/// 应用服务端返回的整表；若同步期间本地又产生了新编辑，则把新编辑叠加
/// 回本地并保留 pending，下一轮继续推送（避免覆盖用户刚改的内容）。
fn commit(
    response: PricingTableResponse,
    sent_changes: &BTreeMap<String, Option<ModelPrice>>,
    sent_hidden: &Option<Vec<String>>,
    local_fx: f64,
) -> Result<(), String> {
    let _guard = lock(state_lock());
    let mut state = read_state_locked();
    let newer = &state.pending != sent_changes || &state.pending_hidden != sent_hidden;

    let current = pricing::load_pricing().unwrap_or_default();
    let mut usd = response.usd;
    let mut hidden = response.hidden;
    if newer {
        apply_changes(&mut usd, &state.pending);
        if let Some(h) = &state.pending_hidden {
            hidden = h.clone();
        }
    } else {
        state.pending.clear();
        state.pending_hidden = None;
    }

    state.revision = response.revision;
    state.initialized = true;
    let remote_fx = valid_fx(response.fx_rate);

    let target = PricingConfig { usd, hidden };
    let pricing_changed = target.usd != current.usd || target.hidden != current.hidden;
    if pricing_changed {
        pricing::save_pricing(&target)?;
    }

    let mut fx_changed = false;
    let mut fx_ok = true;
    if let Some(fx) = remote_fx {
        if (local_fx - fx).abs() > 1e-9 {
            match crate::cursor::set_usd_cny_rate(fx) {
                Ok(changed) => fx_changed = changed,
                Err(e) => {
                    fx_ok = false;
                    eprintln!("[zbar-pricing] 应用服务端汇率失败: {e}");
                }
            }
        }
    }
    if fx_ok {
        if let Some(fx) = remote_fx {
            state.last_fx = Some(fx);
        }
    }

    write_state_locked(&state)?;
    if pricing_changed || fx_changed || response.concurrent {
        crate::sync::emit_pricing_updated(response.concurrent, false);
    }
    let _ = (
        response.updated_at,
        response.updated_by,
        response.applied,
        response.synced,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    const TABLE_RESPONSE: &str = r#"{"revision":2,"updated_at":1,"updated_by":"d","usd":{"glm-4.6":{"input":1.0,"output":2.0,"cache_read":0.1}},"hidden":[],"fx_rate":7.2,"synced":true,"applied":1,"concurrent":false}"#;

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).expect("读取 mock 请求失败");
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&data[..pos]).to_lowercase();
                let len = header
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if data.len() >= pos + 4 + len {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&data).into_owned()
    }

    fn mock_server(body: &'static str, expects: &'static [&'static str]) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("mock addr");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let request = read_http_request(&mut stream);
            let haystack = request.to_lowercase();
            for expect in expects {
                assert!(
                    haystack.contains(&expect.to_lowercase()),
                    "请求缺少 {expect:?}: {request}"
                );
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("写 mock 响应失败");
        });
        (format!("http://{addr}"), handle)
    }

    #[test]
    fn push_sends_incremental_changes_and_parses_table() {
        let mut changes = BTreeMap::new();
        changes.insert("glm-4.6".to_string(), Some(price(1.0)));
        let (base, server) = mock_server(
            TABLE_RESPONSE,
            &[
                "POST /pricing",
                "authorization: bearer t",
                "\"base_revision\":1",
                "\"glm-4.6\"",
                "\"fx_rate\":7.2",
            ],
        );
        let response = push(&base, "t", 1, &changes, None, 7.2).expect("push");
        assert_eq!(response.revision, 2);
        assert!(response.synced);
        assert_eq!(response.usd["glm-4.6"].input, 1.0);
        assert_eq!(response.usd["glm-4.6"].cache_read, 0.1);
        server.join().expect("mock server");
    }

    #[test]
    fn fetch_reads_table_and_sends_token() {
        let (base, server) =
            mock_server(TABLE_RESPONSE, &["GET /pricing", "authorization: bearer t"]);
        let response = fetch(&base, "t").expect("fetch");
        assert_eq!(response.revision, 2);
        assert_eq!(response.usd.len(), 1);
        server.join().expect("mock server");
    }

    fn price(input: f64) -> ModelPrice {
        ModelPrice {
            input,
            output: input * 2.0,
            cache_read: 0.0,
            cache_write: None,
        }
    }

    fn config(usd: &[(&str, f64)]) -> PricingConfig {
        PricingConfig {
            usd: usd
                .iter()
                .map(|(id, value)| (id.to_string(), price(*value)))
                .collect(),
            hidden: Vec::new(),
        }
    }

    #[test]
    fn diff_marks_added_changed_and_deleted_models() {
        let mut state = PricingSyncState::default();
        let old = config(&[("a", 1.0), ("b", 2.0)]);
        let mut new = config(&[("a", 1.5), ("c", 3.0)]);
        new.hidden.push("b".into());

        assert!(diff_into(&mut state, &old, &new));
        assert_eq!(state.pending.len(), 3);
        assert_eq!(state.pending["a"], Some(price(1.5)));
        assert_eq!(state.pending["b"], None);
        assert_eq!(state.pending["c"], Some(price(3.0)));
        assert_eq!(state.pending_hidden.as_deref(), Some(&["b".to_string()][..]));

        // 再改一次同一个模型，以最新值为准
        let newer = config(&[("a", 9.0)]);
        assert!(diff_into(&mut state, &new, &newer));
        assert_eq!(state.pending["a"], Some(price(9.0)));
    }

    #[test]
    fn sync_configured_requires_url_and_token() {
        let mut cfg = crate::sync::SyncConfig::default();
        assert!(!sync_configured(&cfg));
        cfg.server_url = "http://127.0.0.1:3838".into();
        assert!(!sync_configured(&cfg));
        cfg.device_token = "t".into();
        assert!(sync_configured(&cfg));
    }

    #[test]
    fn diff_is_noop_for_identical_configs() {
        let mut state = PricingSyncState::default();
        let cfg = config(&[("a", 1.0)]);
        assert!(!diff_into(&mut state, &cfg, &cfg));
        assert!(state.pending.is_empty());
    }

    #[test]
    fn apply_changes_writes_and_deletes() {
        let mut usd: BTreeMap<String, ModelPrice> =
            [("a".to_string(), price(1.0)), ("b".to_string(), price(2.0))]
                .into_iter()
                .collect();
        let changes: BTreeMap<String, Option<ModelPrice>> = [
            ("a".to_string(), Some(price(9.0))),
            ("b".to_string(), None),
            ("c".to_string(), Some(price(3.0))),
        ]
        .into_iter()
        .collect();
        apply_changes(&mut usd, &changes);
        assert_eq!(usd.len(), 2);
        assert_eq!(usd["a"], price(9.0));
        assert!(!usd.contains_key("b"));
        assert_eq!(usd["c"], price(3.0));
    }

    #[test]
    fn invalid_fx_is_rejected() {
        assert!(valid_fx(7.2).is_some());
        assert!(valid_fx(0.0).is_none());
        assert!(valid_fx(-1.0).is_none());
        assert!(valid_fx(f64::NAN).is_none());
    }
}
