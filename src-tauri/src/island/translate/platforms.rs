//! 翻译可复用的推理凭证。设置窗口只收平台条目标识、显示名称和模型 ID。
//!
//! 可用来源 = 凭证体系条目 ∪ 本机 CLI 登录态：
//! - 凭证：`apiKey` 通用；`token` 仅 kimi（refresh_token 换 access_token）；
//!   `cookie` 没有 OpenAI 兼容端点，一律不列。
//! - 本机：仅 kimi / minimax / opencodego。gemini 的 CLI OAuth access_token 是
//!   Code Assist 专用，打 generativelanguage.googleapis.com/v1beta 会 403
//!   ACCESS_TOKEN_SCOPE_INSUFFICIENT（正确通道是 cloudcode-pa 的 v1internal
//!   协议，并非 OpenAI 兼容端点），故不纳入；grok 的 OIDC/会话 token 也只在
//!   cli-chat-proxy 的额度端点验证过，同样不纳入。

use crate::provider_credentials::{load_query_snapshots, CredentialQuerySnapshot};
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashSet, io::Read, time::Duration};

enum ModelDirectory {
    Authenticated(&'static str),
    Public(&'static str),
    Curated,
}

struct Provider {
    id: &'static str,
    name: &'static str,
    china_base: &'static str,
    global_base: &'static str,
    models: ModelDirectory,
    fallback: &'static [&'static str],
}

const PROVIDERS: &[Provider] = &[
    Provider {
        id: "deepseek",
        name: "DeepSeek",
        china_base: "https://api.deepseek.com",
        global_base: "https://api.deepseek.com",
        models: ModelDirectory::Authenticated("/models"),
        fallback: &["deepseek-flash", "deepseek-v4-pro"],
    },
    Provider {
        id: "kimi",
        name: "Kimi Code",
        china_base: "https://api.kimi.com/coding/v1",
        global_base: "https://api.kimi.ai/coding/v1",
        models: ModelDirectory::Curated,
        fallback: &[
            "kimi-for-coding",
            "k3-256k",
            "k3",
            "k3.1-pro",
            "k3.1-pro-highspeed",
            "kimi-for-coding-highspeed",
        ],
    },
    Provider {
        id: "moonshot",
        name: "Moonshot",
        china_base: "https://api.moonshot.cn/v1",
        global_base: "https://api.moonshot.ai/v1",
        models: ModelDirectory::Authenticated("/models"),
        fallback: &["kimi-k3", "kimi-k2.5"],
    },
    Provider {
        id: "minimax",
        name: "MiniMax",
        china_base: "https://api.minimaxi.com/v1",
        global_base: "https://api.minimax.io/v1",
        models: ModelDirectory::Curated,
        fallback: &["MiniMax-M3.1-Flash", "MiniMax-M3"],
    },
    Provider {
        id: "alibaba",
        name: "阿里云百炼",
        china_base: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        global_base: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        models: ModelDirectory::Curated,
        fallback: &["qwen3.8-flash", "qwen3.8-max"],
    },
    Provider {
        id: "gemini",
        name: "Gemini",
        china_base: "https://generativelanguage.googleapis.com/v1beta/openai",
        global_base: "https://generativelanguage.googleapis.com/v1beta/openai",
        models: ModelDirectory::Authenticated("/models"),
        fallback: &["gemini-3.8-flash"],
    },
    Provider {
        id: "grok",
        name: "Grok",
        china_base: "https://api.x.ai/v1",
        global_base: "https://api.x.ai/v1",
        models: ModelDirectory::Authenticated("/language-models"),
        fallback: &["grok-4.7"],
    },
    Provider {
        id: "doubao",
        name: "火山方舟",
        china_base: "https://ark.cn-beijing.volces.com/api/v3",
        global_base: "https://ark.cn-beijing.volces.com/api/v3",
        models: ModelDirectory::Authenticated("/models"),
        fallback: &["doubao-seed-2-1-pro-260915", "doubao-seed-2-0-lite-260428"],
    },
    Provider {
        id: "opencodego",
        name: "OpenCode Go",
        china_base: "https://opencode.ai/inference/go/openai/v1",
        global_base: "https://opencode.ai/inference/go/openai/v1",
        models: ModelDirectory::Public("https://opencode.ai/inference/go/v1/models"),
        fallback: &["glm-5.3-flash", "glm-5.3", "kimi-k3"],
    },
];

#[derive(Serialize)]
pub struct TranslationPlatform {
    id: String,
    label: String,
}

pub(super) struct PlatformRequest {
    pub(super) api_key: String,
    pub(super) base_url: String,
    pub(super) headers: Vec<(&'static str, String)>,
}

/// 本机登录态选项的固定后缀（`<provider>:local`），与凭证条目的
/// `<provider>:<credentialId>` 区分，旧 id 不会被误判成本机来源。
const LOCAL_SELECTOR: &str = "local";
/// 本机登录态选项的标签后缀（如「Kimi · 本机登录」）。
const LOCAL_LABEL: &str = "本机登录";

/// 凭证类型能否用于翻译的 OpenAI 兼容端点：
/// - `apiKey`：通用（阿里云百炼的套餐 Key `sk-sp-` 与模型 API 不互通，另判）；
/// - `token`：仅 provider 明确声明可直接使用时放行。当前只有 kimi：其 secret
///   是 OAuth refresh_token，resolve 时复用既有换新链路换 access_token（见
///   kimi::translation_bearer_for_credential）；grok 的 kind=token 是本地
///   auth.json 的 OIDC/会话 token，现有代码只证明它可用于 cli-chat-proxy 的
///   额度端点，无法确认 api.x.ai 的 OpenAI 兼容端点接受它，宁缺毋滥不列入；
/// - `cookie`：cookie 型平台没有 OpenAI 兼容端点，一律不列出。
/// 本机登录态不经过本函数；gemini 的 Code Assist scope、grok 的 proxy token
/// 为何不纳入见模块头与 local_login_available。
fn credential_kind_usable(provider: &Provider, kind: &str) -> bool {
    match kind {
        "apiKey" => true,
        "token" => provider.id == "kimi",
        _ => false,
    }
}

fn usable_credential(provider: &Provider, credential: &CredentialQuerySnapshot) -> bool {
    let secret = credential.secret.trim();
    if secret.is_empty() || secret.chars().any(char::is_whitespace) {
        return false;
    }
    if !credential_kind_usable(provider, &credential.kind) {
        return false;
    }
    // 百炼的套餐 Key 与通用模型 API 不互通。
    credential.kind != "apiKey" || provider.id != "alibaba" || !secret.starts_with("sk-sp-")
}

fn to_option(provider: &Provider, credential: &CredentialQuerySnapshot) -> TranslationPlatform {
    let name = provider.name;
    let label = credential.label.trim();
    let label = if label.is_empty() {
        name.to_string()
    } else {
        format!("{name} · {label}")
    };
    TranslationPlatform {
        id: format!("{}:{}", provider.id, credential.id),
        label,
    }
}

fn local_option(provider: &Provider) -> TranslationPlatform {
    TranslationPlatform {
        id: format!("{}:{LOCAL_SELECTOR}", provider.id),
        label: format!("{} · {LOCAL_LABEL}", provider.name),
    }
}

/// 本机 CLI 登录态是否可用于翻译（只读文件探测，绝不发网络请求）。
/// 只纳入 kimi / minimax / opencodego；gemini 与 grok 的原因见模块头注释。
fn local_login_available(provider: &Provider) -> bool {
    match provider.id {
        "kimi" => crate::kimi::has_local_translation_login(),
        "minimax" => crate::minimax::has_local_translation_login(),
        "opencodego" => crate::opencodego::has_local_translation_login(),
        _ => false,
    }
}

/// 本机登录态 → Bearer 与区域提示（kimi/minimax 可能带区域）。
/// 仅在 resolve / island_translation_models 调用；kimi 的刷新复用其既有链路，
/// 失败返回中文原因（不含 secret）。
fn local_bearer(provider: &Provider) -> Result<(String, Option<String>), String> {
    match provider.id {
        "kimi" => crate::kimi::local_translation_bearer(),
        "minimax" => crate::minimax::local_translation_bearer(),
        "opencodego" => crate::opencodego::local_translation_bearer().map(|token| (token, None)),
        _ => Err("所选平台不支持本机登录态，请重新选择".to_string()),
    }
}

/// 本机登录态的区域值 → 现有 base_url 的区域口径：
/// minimax 的目录名可能是 en/io/global（见 minimax::region_hint），统一按
/// 国际站处理；kimi 的 region 文件为 global 时走 .ai 域名。
fn local_region(provider: &Provider, region: Option<&str>) -> Option<&'static str> {
    match provider.id {
        "minimax" => match region {
            Some("global") | Some("en") | Some("io") => Some("global"),
            _ => None,
        },
        "kimi" => (region == Some("global")).then_some("global"),
        _ => None,
    }
}

enum PlatformSelection<'a> {
    Credential(&'a str),
    Local,
}

/// 解析已保存的平台 id：兼容旧的 `<provider>:<credentialId>` 与新的
/// `<provider>:local`（本机登录态）。
fn parse_platform(platform: &str) -> Result<(&'static Provider, PlatformSelection<'_>), String> {
    let (provider_id, selector) = platform
        .split_once(':')
        .ok_or("请在翻译设置中选择已配置的平台")?;
    let provider = PROVIDERS
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or("所选平台不可用于翻译，请重新选择")?;
    if selector.is_empty() {
        return Err("请在翻译设置中选择已配置的平台".to_string());
    }
    if selector == LOCAL_SELECTOR {
        Ok((provider, PlatformSelection::Local))
    } else {
        Ok((provider, PlatformSelection::Credential(selector)))
    }
}

/// resolve / 模型列表共用的已解析鉴权信息。
struct PlatformAuth {
    provider: &'static Provider,
    bearer: String,
    base_url: String,
}

fn auth_from_local(provider: &'static Provider) -> Result<PlatformAuth, String> {
    let (bearer, region) = local_bearer(provider)?;
    let region = local_region(provider, region.as_deref());
    Ok(PlatformAuth {
        provider,
        bearer,
        base_url: base_url(provider, region).to_string(),
    })
}

fn auth_from_credential(
    provider: &'static Provider,
    credential: &CredentialQuerySnapshot,
) -> Result<PlatformAuth, String> {
    if !usable_credential(provider, credential) {
        return Err(format!(
            "{} 的所选平台凭证已不可用，请重新选择",
            provider.name
        ));
    }
    if credential.kind == "token" {
        // kimi：secret 是 OAuth refresh_token；复用凭证链路既有的换新/缓存/
        // 熔断，base_url 随凭证 region 走。失败原因是中文且不含 secret。
        let (bearer, base) = crate::kimi::translation_bearer_for_credential(
            &credential.id,
            &credential.kind,
            &credential.secret,
            credential.region.as_deref(),
        )?;
        return Ok(PlatformAuth {
            provider,
            bearer,
            base_url: base.to_string(),
        });
    }
    Ok(PlatformAuth {
        provider,
        bearer: credential.secret.trim().to_string(),
        base_url: base_url(provider, credential.region.as_deref()).to_string(),
    })
}

fn load_selected(platform: &str) -> Result<PlatformAuth, String> {
    let (provider, selection) = parse_platform(platform)?;
    match selection {
        PlatformSelection::Local => auth_from_local(provider),
        PlatformSelection::Credential(credential_id) => {
            let credential = load_query_snapshots(provider.id)
                .map_err(|_| format!("读取 {} 的平台凭证失败", provider.name))?
                .into_iter()
                .find(|credential| credential.id == credential_id)
                .ok_or_else(|| {
                    format!("{} 的所选平台凭证已不可用，请重新选择", provider.name)
                })?;
            auth_from_credential(provider, &credential)
        }
    }
}

fn base_url(provider: &Provider, region: Option<&str>) -> &'static str {
    if region == Some("global") {
        provider.global_base
    } else {
        provider.china_base
    }
}

pub(super) fn resolve(platform: &str) -> Result<PlatformRequest, String> {
    let auth = load_selected(platform)?;
    let headers = if auth.provider.id == "opencodego" {
        // 每次翻译是独立的单轮请求；Go 要求调用携带会话标识。
        vec![(
            "x-opencode-session",
            format!("ses_{}", uuid::Uuid::new_v4()),
        )]
    } else {
        Vec::new()
    };
    Ok(PlatformRequest {
        api_key: auth.bearer,
        base_url: auth.base_url,
        headers,
    })
}

/// 平台下拉 = 凭证条目 ∪ 本机登录态（每个 provider 先列凭证、再列 local）。
/// 只做文件读取，不发网络请求；凭证文件读失败只在「一条可用来源都没有」时
/// 上抛，避免单个损坏文件把其他可用平台一起吞掉。
fn list_options(
    load_credentials: &dyn Fn(&str) -> Result<Vec<CredentialQuerySnapshot>, String>,
    local_available: &dyn Fn(&Provider) -> bool,
) -> Result<Vec<TranslationPlatform>, String> {
    let mut options = Vec::new();
    let mut load_error = None;
    for provider in PROVIDERS {
        match load_credentials(provider.id) {
            Ok(credentials) => options.extend(
                credentials
                    .iter()
                    .filter(|credential| usable_credential(provider, credential))
                    .map(|credential| to_option(provider, credential)),
            ),
            Err(_) => load_error = Some(format!("读取 {} 的平台凭证失败", provider.name)),
        }
        if local_available(provider) {
            options.push(local_option(provider));
        }
    }
    if options.is_empty() {
        if let Some(error) = load_error {
            return Err(error);
        }
    }
    Ok(options)
}

fn list_configured() -> Result<Vec<TranslationPlatform>, String> {
    list_options(&load_query_snapshots, &local_login_available)
}

/// 已保存且可用于翻译的平台条目：凭证体系条目 ∪ 本机 CLI 登录态。
/// 本命令只做文件探测，不发任何网络请求。
#[tauri::command]
pub async fn island_translation_platforms() -> Result<Vec<TranslationPlatform>, String> {
    tauri::async_runtime::spawn_blocking(list_configured)
        .await
        .map_err(|_| "读取已配置的平台失败".to_string())?
}

fn model_ids(value: &Value, secret: &str) -> Vec<String> {
    let Some(models) = value
        .get("data")
        .or_else(|| value.get("models"))
        .unwrap_or(value)
        .as_array()
    else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    models
        .iter()
        .filter(|model| {
            if let Some(outputs) = model
                .pointer("/modalities/output_modalities")
                .or_else(|| model.pointer("/architecture/output_modalities"))
                .or_else(|| model.get("output_modalities"))
                .and_then(Value::as_array)
            {
                return outputs.iter().any(|item| item == "text");
            }
            true
        })
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| {
            let name = id.to_ascii_lowercase();
            !id.is_empty()
                && id.len() <= 160
                && !id.contains(secret)
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-._:/@+".contains(&c))
                && ![
                    "embedding",
                    "rerank",
                    "tts",
                    "imagen",
                    "veo",
                    "grok-imagine",
                ]
                .iter()
                .any(|word| name.contains(word))
        })
        .filter(|id| seen.insert(*id))
        .map(str::to_owned)
        .collect()
}

/// 模型列表是否需要先解析凭证/Bearer：只有平台自带的模型目录
/// （Authenticated / Public）才需要；Curated 平台直接返回已验证的 fallback，
/// 避免为了拿模型列表去换 token / 写盘（kimi/minimax/alibaba）。
fn models_need_credentials(provider: &Provider) -> bool {
    !matches!(&provider.models, ModelDirectory::Curated)
}

fn fallback_models(provider: &Provider) -> Vec<String> {
    provider
        .fallback
        .iter()
        .map(|model| model.to_string())
        .collect()
}

fn fetch_models(auth: &PlatformAuth) -> Option<Vec<String>> {
    let (url, authenticate) = match auth.provider.models {
        ModelDirectory::Authenticated(path) => (format!("{}{path}", auth.base_url), true),
        ModelDirectory::Public(url) => (url.to_string(), false),
        ModelDirectory::Curated => return None,
    };
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout(Duration::from_secs(7))
        .redirects(0);
    if let Some(proxy) = crate::codex::resolve_proxy().and_then(|url| ureq::Proxy::new(&url).ok()) {
        builder = builder.proxy(proxy);
    }
    let mut request = builder.build().get(&url).set("Accept", "application/json");
    if authenticate {
        request = request.set("Authorization", &format!("Bearer {}", auth.bearer));
    }
    let response = request.call().ok()?;
    if !(200..300).contains(&response.status()) {
        return None;
    }
    let value = serde_json::from_reader(response.into_reader().take(1024 * 1024)).ok()?;
    let ids = model_ids(&value, &auth.bearer);
    (!ids.is_empty()).then_some(ids)
}

/// 优先读平台模型目录，目录未提供或无法读取时使用已验证的模型 ID。
#[tauri::command]
pub async fn island_translation_models(platform: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 先按 provider 的模型目录形态分流：Curated（kimi/minimax/alibaba）
        // 不依赖凭证，直接返回 fallback，避免为了模型列表换 token / 写盘。
        let (provider, _) = parse_platform(&platform)?;
        if !models_need_credentials(provider) {
            return Ok(fallback_models(provider));
        }
        let auth = load_selected(&platform)?;
        Ok(fetch_models(&auth).unwrap_or_else(|| fallback_models(auth.provider)))
    })
    .await
    .map_err(|_| "读取平台模型列表失败".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider(id: &str) -> &'static Provider {
        PROVIDERS.iter().find(|provider| provider.id == id).unwrap()
    }

    fn credential(kind: &str, secret: &str) -> CredentialQuerySnapshot {
        CredentialQuerySnapshot {
            id: "account-1".into(),
            label: "工作".into(),
            kind: kind.into(),
            secret: secret.into(),
            region: None,
        }
    }

    #[test]
    fn translation_platforms_expose_metadata_only_and_filter_quota_secrets() {
        let entry = credential("apiKey", "sk-example");
        assert!(usable_credential(provider("deepseek"), &entry));
        assert!(!usable_credential(
            provider("gemini"),
            &credential("apiKey", " a b ")
        ));
        assert!(!usable_credential(
            provider("deepseek"),
            &credential("apiKey", "")
        ));
        assert_eq!(
            serde_json::to_value(to_option(provider("deepseek"), &entry)).unwrap(),
            json!({"id":"deepseek:account-1","label":"DeepSeek · 工作"})
        );
    }

    #[test]
    fn translation_platform_kinds_follow_provider_policy() {
        // apiKey 放行；百炼套餐 Key 与通用模型 API 不互通。
        assert!(usable_credential(
            provider("kimi"),
            &credential("apiKey", "sk-key")
        ));
        assert!(!usable_credential(
            provider("alibaba"),
            &credential("apiKey", "sk-sp-plan")
        ));
        // token 只按 provider 声明放行：kimi 的 token 是 refresh_token；
        // grok 的 OIDC/会话 token 未经 api.x.ai 兼容性验证，不纳入。
        assert!(usable_credential(
            provider("kimi"),
            &credential("token", "refresh")
        ));
        assert!(!usable_credential(
            provider("grok"),
            &credential("token", "oidc-session")
        ));
        // cookie 型平台没有 OpenAI 兼容端点，一律拒绝。
        assert!(!usable_credential(
            provider("deepseek"),
            &credential("cookie", "session-cookie")
        ));
    }

    #[test]
    fn translation_platform_ids_accept_credentials_and_local() {
        let (deepseek, selection) = parse_platform("deepseek:account-1").unwrap();
        assert_eq!(deepseek.id, "deepseek");
        assert!(matches!(
            selection,
            PlatformSelection::Credential("account-1")
        ));

        let (kimi, selection) = parse_platform("kimi:local").unwrap();
        assert_eq!(kimi.id, "kimi");
        assert!(matches!(selection, PlatformSelection::Local));

        // 旧 id / 新 id 之外的形式仍按原口径报错。
        assert!(parse_platform("kimi").is_err());
        assert!(parse_platform("unknown:local").is_err());
        assert!(parse_platform("kimi:").is_err());
    }

    #[test]
    fn translation_local_options_use_distinct_ids_and_labels() {
        assert_eq!(
            serde_json::to_value(local_option(provider("minimax"))).unwrap(),
            json!({"id":"minimax:local","label":"MiniMax · 本机登录"})
        );
        assert_eq!(local_option(provider("kimi")).id, "kimi:local");
        assert_eq!(local_option(provider("kimi")).label, "Kimi Code · 本机登录");
        assert_eq!(local_option(provider("opencodego")).id, "opencodego:local");
    }

    /// gemini 的 CLI OAuth 是 Code Assist 专用，打 OpenAI 兼容端点会 403
    /// ACCESS_TOKEN_SCOPE_INSUFFICIENT；grok 的本地 token 同样无 api.x.ai 证据。
    #[test]
    fn translation_local_sources_exclude_gemini_and_grok() {
        assert!(!local_login_available(provider("gemini")));
        assert!(!local_login_available(provider("grok")));
    }

    #[test]
    fn translation_provider_hosts_follow_the_credentials_region() {
        assert_eq!(
            base_url(provider("kimi"), None),
            "https://api.kimi.com/coding/v1"
        );
        assert_eq!(
            base_url(provider("kimi"), Some("global")),
            "https://api.kimi.ai/coding/v1"
        );
        assert_eq!(
            base_url(provider("minimax"), Some("global")),
            "https://api.minimax.io/v1"
        );
    }

    #[test]
    fn translation_local_sources_keep_existing_region_mapping() {
        let minimax = provider("minimax");
        assert_eq!(local_region(minimax, Some("cn")), None);
        assert_eq!(local_region(minimax, None), None);
        assert_eq!(local_region(minimax, Some("global")), Some("global"));
        assert_eq!(local_region(minimax, Some("en")), Some("global"));
        assert_eq!(local_region(minimax, Some("io")), Some("global"));
        assert_eq!(
            base_url(minimax, local_region(minimax, Some("io"))),
            "https://api.minimax.io/v1"
        );

        let kimi = provider("kimi");
        assert_eq!(local_region(kimi, Some("global")), Some("global"));
        assert_eq!(local_region(kimi, None), None);
        assert_eq!(
            base_url(kimi, local_region(kimi, Some("global"))),
            "https://api.kimi.ai/coding/v1"
        );

        // opencodego 的本地来源无区域概念，始终走默认域名。
        assert_eq!(local_region(provider("opencodego"), Some("global")), None);
    }

    /// Curated 平台（kimi/minimax/alibaba）的模型列表不依赖凭证解析：
    /// 直接返回 fallback，避免为了模型列表去换 token / 写盘。
    #[test]
    fn translation_curated_model_lists_need_no_credentials() {
        for id in ["kimi", "minimax", "alibaba"] {
            assert!(!models_need_credentials(provider(id)), "{id}");
            assert_eq!(
                fallback_models(provider(id)),
                provider(id)
                    .fallback
                    .iter()
                    .map(|model| model.to_string())
                    .collect::<Vec<_>>()
            );
        }
        for id in ["deepseek", "moonshot", "gemini", "grok", "doubao", "opencodego"] {
            assert!(models_need_credentials(provider(id)), "{id}");
        }
    }

    #[test]
    fn translation_credential_auth_keeps_old_ids_and_provider_base() {
        let auth = auth_from_credential(
            provider("opencodego"),
            &credential("apiKey", " sk-example "),
        )
        .unwrap();
        assert_eq!(auth.bearer, "sk-example");
        assert_eq!(auth.base_url, "https://opencode.ai/inference/go/openai/v1");

        let auth = auth_from_credential(
            provider("deepseek"),
            &credential("apiKey", "sk-old"),
        )
        .unwrap();
        assert_eq!(auth.bearer, "sk-old");
        assert_eq!(auth.base_url, "https://api.deepseek.com");
    }

    #[test]
    fn translation_platform_list_merges_credentials_and_local_sources() {
        let entry = credential("apiKey", "sk-example");
        let list = list_options(
            &|id| {
                if id == "opencodego" {
                    Ok(vec![entry.clone()])
                } else {
                    Ok(Vec::new())
                }
            },
            &|_| false,
        )
        .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "opencodego:account-1");

        // 没有任何来源（凭证文件缺失视为空）→ 空数组而不是报错。
        assert!(list_options(&|_| Ok(Vec::new()), &|_| false)
            .unwrap()
            .is_empty());

        // 凭证条目 + 本机登录态同时存在 → 两条都出现（凭证在前）。
        let both = list_options(
            &|id| {
                if id == "kimi" {
                    Ok(vec![credential("apiKey", "sk-kimi")])
                } else {
                    Ok(Vec::new())
                }
            },
            &|provider| provider.id == "kimi",
        )
        .unwrap();
        assert_eq!(
            both.iter().map(|option| option.id.as_str()).collect::<Vec<_>>(),
            ["kimi:account-1", "kimi:local"]
        );

        // 只有本机登录态也能列出来。
        let local_only =
            list_options(&|_| Ok(Vec::new()), &|provider| provider.id == "kimi").unwrap();
        assert_eq!(local_only.len(), 1);
        assert_eq!(local_only[0].id, "kimi:local");

        // 全部读取失败且无本机来源 → 保留原有错误口径。
        assert!(list_options(&|_| Err("模拟读取失败".to_string()), &|_| false).is_err());
    }

    #[test]
    fn translation_model_directory_omits_duplicates_and_non_text_outputs() {
        let directory = json!({"data": [
            {"id": "small"},
            {"id": "small"},
            {"id": "larger:preview"},
            {"id": "image-model", "modalities": {"output_modalities": ["image"]}},
            {"id": "model-embedding"},
            {"id": "sk-reflected"},
            {"id": "invalid model"}
        ]});
        assert_eq!(
            model_ids(&directory, "sk-reflected"),
            ["small", "larger:preview"]
        );
    }
}
