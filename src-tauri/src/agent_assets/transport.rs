//! §8 服务端 HTTP 客户端（契约 `doc/agent-assets-contract.md`）。
//!
//! 复用 `sync.rs` 的 `load_sync_config` 地址与 `device_token` 凭证、
//! `agent_with_timeouts` 超时分层与错误口径；独立之处：
//!
//! - 错误是结构化 [`AssetError`]：优先解析服务端 §4.2 错误体原样透传 code，
//!   解析不出再按 HTTP 状态归类；
//! - 旧服务端（无 `GET /agent-assets/capabilities` 路由）返回
//!   `OLD_SERVER_UNSUPPORTED` 的明确状态，前端提示升级后本地能力不受影响；
//! - 打包信封直接 JSON POST/PUT（内容 base64，契约 §3.1），无压缩层。
//!
//! B 工作包（服务端）未联调前调用会得到网络/路由错误，属预期；
//! 命令签名与返回结构即本模块定稿。

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::agent_assets::bundle::AssetBundle;
use crate::agent_assets::AssetError;
use crate::sync::{agent_with_timeouts, load_sync_config};

// ===== 响应结构（契约 §7.1 / §8）=====

/// 服务端容量限制（契约 §8.1 limits；字段与建议默认值见 §8.7）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerLimits {
    pub max_request_bytes: u64,
    pub max_file_bytes: u64,
    pub max_bundle_bytes: u64,
    pub max_entries_per_bundle: usize,
    pub max_revisions_per_asset: usize,
    pub library_quota_bytes: u64,
}

/// `GET /agent-assets/capabilities` 的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub api_version: u32,
    pub limits: ServerLimits,
}

/// 服务端支持状态（命令 `agent_assets_server_status` 的返回；契约 §7）。
/// `supported=false` 时 `reason` 为 AssetError code：CONFIG_MISSING /
/// OLD_SERVER_UNSUPPORTED / NETWORK_ERROR / UNAUTHORIZED。
#[derive(Debug, Clone, Serialize)]
pub struct ServerAssetStatus {
    pub supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<ServerLimits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 人读细节（网络错误原因等），与 reason 配套。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 同步地址为非回环的裸 `http://`（建议改用 HTTPS；仅回环地址为 false）。
    pub insecure_transport: bool,
}

/// 资源元数据（列表/上传/更新共用；契约 §7.1 RemoteAsset）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAsset {
    pub asset_id: String,
    pub tool: String,
    pub kind: String,
    pub native_id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub origin_scope: String,
    pub native_format: String,
    pub latest_version: u64,
    pub latest_bundle_sha256: String,
    pub latest_manifest_sha256: String,
    pub total_size: u64,
    pub entry_count: usize,
    /// epoch 毫秒。
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

/// 单个版本元数据（契约 §7.1 RemoteRevision）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteRevision {
    pub asset_id: String,
    pub version: u64,
    pub bundle_sha256: String,
    pub manifest_sha256: String,
    pub total_size: u64,
    pub entry_count: usize,
    /// epoch 毫秒。
    pub created_at: i64,
    pub created_by_device: String,
}

/// POST/PUT 的响应（契约 §7.1 RemoteAssetResult）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAssetResult {
    pub asset: RemoteAsset,
    pub revision: RemoteRevision,
    /// 幂等重放（同 client_request_id 重试命中，契约 §8.6）。
    #[serde(default)]
    pub replayed: bool,
    /// 相同内容去重（未写新版本，返回已有资源，契约 §8.6）。
    #[serde(default)]
    pub deduplicated: bool,
}

/// `GET /agent-assets` 分页结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAssetPage {
    pub items: Vec<RemoteAsset>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
}

/// 列表查询参数（命令 `agent_assets_list_remote` 的入参）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// native_id / display_name 子串，大小写不敏感。
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_size: Option<u32>,
}

// ===== 基础设施 =====

/// 服务端错误体（契约 §4.2）。
#[derive(Debug, Deserialize)]
struct WireError {
    error: WireErrorBody,
}

#[derive(Debug, Deserialize)]
struct WireErrorBody {
    code: String,
    message: String,
    #[serde(default)]
    details: Option<serde_json::Value>,
}

/// 读配置并要求已注册设备（device_token 非空），否则 CONFIG_MISSING。
fn require_config() -> Result<(String, String), AssetError> {
    let cfg = load_sync_config().map_err(|e| AssetError::io(format!("读取同步配置失败: {e}")))?;
    if cfg.server_url.is_empty() || cfg.device_token.is_empty() {
        return Err(AssetError::config_missing(
            "尚未注册同步服务端，请先在同步页完成设备注册",
        ));
    }
    Ok((cfg.server_url, cfg.device_token))
}

/// ureq 错误 → AssetError：优先透传服务端 §4.2 错误体；解析不出按状态码归类；
/// 传输层失败 → NETWORK_ERROR（口径沿用 sync.rs 的 map_http_err，仅换载体）。
fn map_http_error(op: &str, e: ureq::Error) -> AssetError {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            if let Ok(wire) = serde_json::from_str::<WireError>(&body) {
                let mut err = AssetError::new(&wire.error.code, wire.error.message);
                err.details = wire.error.details;
                return err;
            }
            let code_str = match code {
                401 => "UNAUTHORIZED",
                403 => "FORBIDDEN",
                404 => "NOT_FOUND",
                409 => "VERSION_CONFLICT",
                413 => "PAYLOAD_TOO_LARGE",
                _ => "INTERNAL",
            };
            AssetError::new(code_str, format!("{op}失败: HTTP {code}: {body}"))
        }
        ureq::Error::Transport(t) => AssetError::network(format!(
            "{op}失败: {:?}: {}",
            t.kind(),
            t.message().unwrap_or("")
        )),
    }
}

/// 构造查询档 agent（connect_secs / total_secs 秒；鉴权头在调用处 set）。
fn authed_agent(connect_secs: u64, total_secs: u64) -> ureq::Agent {
    agent_with_timeouts(connect_secs, total_secs)
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// 主机名是否为回环地址（`localhost`、`127.0.0.0/8`、`::1`）。
fn is_loopback_host(host: &str) -> bool {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    h == "localhost" || h == "::1" || h.starts_with("127.")
}

/// 同步地址是否为非回环的裸 `http://`（用于前端展示安全提示）。
///
/// 只做展示判断，不阻断请求：自托管用户可能暂时没有 HTTPS 证书。
fn is_insecure_transport(url: &str) -> bool {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("http://") {
        return false; // https 视为安全；无 scheme 由 normalize_url 处理，不在此判断
    }
    let rest = &trimmed["http://".len()..];
    let authority_end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // 去掉 userinfo，再取 host（IPv6 形如 [::1]:8080）
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if host_port.starts_with('[') {
        host_port
            .find(']')
            .map(|end| &host_port[..=end])
            .unwrap_or(host_port)
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    !is_loopback_host(host)
}

// ===== 各端点（契约 §8.1–§8.5）=====

/// `GET /agent-assets/capabilities`：能力探测 + 容量限制。
/// 网络失败/旧服务端不 Err，而是折成 `supported=false` 的状态对象
/// （前端要展示「待升级」而非报错弹窗；契约 §7 ServerAssetStatus）。
pub fn server_status() -> Result<ServerAssetStatus, AssetError> {
    let cfg = match load_sync_config() {
        Ok(c) => c,
        Err(e) => {
            return Ok(ServerAssetStatus {
                supported: false,
                api_version: None,
                limits: None,
                reason: Some("CONFIG_MISSING".into()),
                detail: Some(e),
                insecure_transport: false,
            })
        }
    };
    let insecure_transport = is_insecure_transport(&cfg.server_url);
    if cfg.server_url.is_empty() || cfg.device_token.is_empty() {
        return Ok(ServerAssetStatus {
            supported: false,
            api_version: None,
            limits: None,
            reason: Some("CONFIG_MISSING".into()),
            detail: Some("尚未注册同步服务端".into()),
            insecure_transport,
        });
    }
    match authed_agent(2, 5)
        .get(&format!("{}/agent-assets/capabilities", cfg.server_url))
        .set("Authorization", &bearer(&cfg.device_token))
        .call()
    {
        Ok(resp) => match resp.into_json::<ServerCapabilities>() {
            Ok(caps) => Ok(ServerAssetStatus {
                supported: true,
                api_version: Some(caps.api_version),
                limits: Some(caps.limits),
                reason: None,
                detail: None,
                insecure_transport,
            }),
            Err(e) => Ok(ServerAssetStatus {
                supported: false,
                api_version: None,
                limits: None,
                reason: Some("OLD_SERVER_UNSUPPORTED".into()),
                detail: Some(format!("能力接口响应无法解析（可能为旧服务端或代理）: {e}")),
                insecure_transport,
            }),
        },
        Err(ureq::Error::Status(404, _)) | Err(ureq::Error::Status(405, _)) => {
            // 旧服务端没有该路由：明确提示升级；本地发现/打包/原生导出不受影响
            Ok(ServerAssetStatus {
                supported: false,
                api_version: None,
                limits: None,
                reason: Some("OLD_SERVER_UNSUPPORTED".into()),
                detail: Some("服务端版本过旧，未提供 agent-assets 能力接口，请升级服务端".into()),
                insecure_transport,
            })
        }
        Err(ureq::Error::Status(401, _)) => Ok(ServerAssetStatus {
            supported: false,
            api_version: None,
            limits: None,
            reason: Some("UNAUTHORIZED".into()),
            detail: Some("设备凭证无效或已被撤销".into()),
            insecure_transport,
        }),
        Err(ureq::Error::Status(403, _)) => Ok(ServerAssetStatus {
            // 403 通常来自反向代理/WAF（服务端自身错误用 401/404 等）；
            // reason 仍归类 NETWORK_ERROR，但 detail 明确写出 403。
            supported: false,
            api_version: None,
            limits: None,
            reason: Some("NETWORK_ERROR".into()),
            detail: Some(
                "请求被反向代理或 WAF 拦截（HTTP 403）：请检查服务端地址、网关与访问规则".into(),
            ),
            insecure_transport,
        }),
        Err(e) => Ok(ServerAssetStatus {
            supported: false,
            api_version: None,
            limits: None,
            reason: Some("NETWORK_ERROR".into()),
            detail: Some(map_http_error("探测 agent-assets 能力", e).message),
            insecure_transport,
        }),
    }
}

/// `GET /agent-assets`：列表（仅元数据 + 分页筛选，契约 §8.2）。
pub fn list_remote_assets(query: ListQuery) -> Result<RemoteAssetPage, AssetError> {
    let (base, token) = require_config()?;
    let mut url = format!("{base}/agent-assets?");
    let mut pairs: Vec<String> = Vec::new();
    if let Some(v) = query.tool.as_deref().filter(|s| !s.is_empty()) {
        pairs.push(format!("tool={}", urlencode(v)));
    }
    if let Some(v) = query.kind.as_deref().filter(|s| !s.is_empty()) {
        pairs.push(format!("kind={}", urlencode(v)));
    }
    if let Some(v) = query.scope.as_deref().filter(|s| !s.is_empty()) {
        pairs.push(format!("scope={}", urlencode(v)));
    }
    if let Some(v) = query.q.as_deref().filter(|s| !s.is_empty()) {
        pairs.push(format!("q={}", urlencode(v)));
    }
    if let Some(v) = query.page {
        pairs.push(format!("page={v}"));
    }
    if let Some(v) = query.page_size {
        pairs.push(format!("page_size={v}"));
    }
    url.push_str(&pairs.join("&"));
    let resp = authed_agent(2, 10)
        .get(&url)
        .set("Authorization", &bearer(&token))
        .call()
        .map_err(|e| map_http_error("查询资源列表", e))?;
    resp.into_json::<RemoteAssetPage>()
        .map_err(|e| AssetError::io(format!("解析资源列表失败: {e}")))
}

/// POST / PUT 公共请求体。
#[derive(Serialize)]
struct BundleRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    client_request_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<u64>,
    bundle: &'a AssetBundle,
}

/// `POST /agent-assets`：上传（client_request_id 幂等，契约 §8.3/§8.6）。
/// 超时档放宽（内容可达 25 MiB 请求体）：connect 5s / 总 60s。
pub fn upload_asset(
    bundle: &AssetBundle,
    client_request_id: &str,
) -> Result<RemoteAssetResult, AssetError> {
    let (base, token) = require_config()?;
    let body = BundleRequest {
        client_request_id: Some(client_request_id),
        expected_version: None,
        bundle,
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .build();
    let resp = agent
        .post(&format!("{base}/agent-assets"))
        .set("Authorization", &bearer(&token))
        .send_json(&body)
        .map_err(|e| map_http_error("上传资源", e))?;
    resp.into_json::<RemoteAssetResult>()
        .map_err(|e| AssetError::io(format!("解析上传响应失败: {e}")))
}

/// `PUT /agent-assets/<id>`：更新（expected_version 乐观并发，冲突 409，契约 §8.4）。
pub fn update_asset(
    asset_id: &str,
    expected_version: u64,
    bundle: &AssetBundle,
) -> Result<RemoteAssetResult, AssetError> {
    let (base, token) = require_config()?;
    let body = BundleRequest {
        client_request_id: None,
        expected_version: Some(expected_version),
        bundle,
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .build();
    let resp = agent
        .put(&format!("{base}/agent-assets/{}", urlencode(asset_id)))
        .set("Authorization", &bearer(&token))
        .send_json(&body)
        .map_err(|e| map_http_error("更新资源", e))?;
    resp.into_json::<RemoteAssetResult>()
        .map_err(|e| AssetError::io(format!("解析更新响应失败: {e}")))
}

/// `GET /agent-assets/<id>/revisions`：版本列表（降序，契约 §8.5）。
#[derive(Debug, Deserialize)]
struct RevisionsPage {
    items: Vec<RemoteRevision>,
}

pub fn list_revisions(asset_id: &str) -> Result<Vec<RemoteRevision>, AssetError> {
    let (base, token) = require_config()?;
    let resp = authed_agent(2, 10)
        .get(&format!(
            "{base}/agent-assets/{}/revisions",
            urlencode(asset_id)
        ))
        .set("Authorization", &bearer(&token))
        .call()
        .map_err(|e| map_http_error("查询版本列表", e))?;
    let page: RevisionsPage = resp
        .into_json()
        .map_err(|e| AssetError::io(format!("解析版本列表失败: {e}")))?;
    Ok(page.items)
}

/// `GET /agent-assets/<id>/revisions/<rev>/bundle`：下载某版本信封（契约 §8.5）。
/// 内容较大，超时档 connect 5s / 总 60s。返回信封未复验——安装/预览前
/// 调用方必须走 `bundle::parse_bundle` 复验指纹（契约 §3.3）。
pub fn fetch_bundle(asset_id: &str, version: u64) -> Result<AssetBundle, AssetError> {
    let (base, token) = require_config()?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .build();
    let resp = agent
        .get(&format!(
            "{base}/agent-assets/{}/revisions/{version}/bundle",
            urlencode(asset_id)
        ))
        .set("Authorization", &bearer(&token))
        .call()
        .map_err(|e| map_http_error("下载资源包", e))?;
    resp.into_json::<AssetBundle>()
        .map_err(|e| AssetError::bundle_invalid(format!("解析资源包信封失败: {e}")))
}

/// 极简百分号编码（仅查询串用；UTF-8 字节逐个转义）。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ============================================================
// 单测：URL 编码与错误归类（不联网；HTTP 面由 B 联调覆盖）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encoding_keeps_safe_chars() {
        assert_eq!(urlencode("zcode"), "zcode");
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("中"), "%E4%B8%AD");
        assert_eq!(urlencode("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn wire_error_body_passes_code_through() {
        // 直接构造 ureq::Error 比较繁琐，这里验证 WireError 的解析形状
        let body = r#"{"error":{"code":"VERSION_CONFLICT","message":"预期版本 3，服务端当前为 5","details":{"current_version":5}}}"#;
        let wire: WireError = serde_json::from_str(body).unwrap();
        assert_eq!(wire.error.code, "VERSION_CONFLICT");
        assert_eq!(wire.error.details.unwrap()["current_version"], 5);
    }

    #[test]
    fn status_shapes_skip_none_fields() {
        let s = ServerAssetStatus {
            supported: true,
            api_version: Some(1),
            limits: None,
            reason: None,
            detail: None,
            insecure_transport: false,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("limits").is_none());
        assert!(v.get("reason").is_none());
        assert_eq!(v["api_version"], 1);
        // 前端据此展示 HTTPS 建议，字段必须始终存在
        assert_eq!(v["insecure_transport"], false);
    }

    #[test]
    fn insecure_transport_only_flags_non_loopback_http() {
        assert!(!is_insecure_transport("https://sync.example.com"));
        assert!(!is_insecure_transport("http://127.0.0.1:8787"));
        assert!(!is_insecure_transport("http://127.9.8.7:8787/base"));
        assert!(!is_insecure_transport("http://localhost:8787"));
        assert!(!is_insecure_transport("http://LOCALHOST:8787"));
        assert!(!is_insecure_transport("http://[::1]:8787"));
        assert!(is_insecure_transport("http://sync.example.com"));
        assert!(is_insecure_transport("HTTP://192.168.1.20:8787"));
        assert!(is_insecure_transport("http://user:pass@example.com:8787"));
    }

    #[test]
    fn list_query_deserializes_from_frontend_shape() {
        // 前端 invoke 传 { tool: "zcode", kind: "skill", page: 2 }（缺省字段可省）
        let q: ListQuery =
            serde_json::from_str(r#"{"tool":"zcode","kind":"skill","page":2}"#).unwrap();
        assert_eq!(q.tool.as_deref(), Some("zcode"));
        assert_eq!(q.kind.as_deref(), Some("skill"));
        assert_eq!(q.page, Some(2));
        assert!(q.q.is_none() && q.scope.is_none());
    }
}
