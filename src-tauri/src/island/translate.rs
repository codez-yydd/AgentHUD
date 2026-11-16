//! 翻译：已配置平台或自定义接口、剪贴板翻译热键。
//!
//! 两件事都直接读设置文档 `settings.json` 的 `translate.*`，
//! 不经过 `IslandConfig`：
//! - 翻译按 OpenAI 协议发送 Chat Completions 请求，
//!   只认 `choices[0].message.content`；API Key 只在本机内存里使用，不写日志。
//! - 剪贴板翻译热键的编排在 `island::mod`（读 → 翻 → 写回 + 通知），本模块
//!   只提供配置与纯翻译函数，方便单测。
//!
//! 网络失败 / 非 2xx / 空译文都返回中文错误；错误文案里可能出现服务端
//! 返回的原因，但会先抹掉 API Key 再拼进去。

use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

use super::settings;

pub mod platforms;

/// 翻译系统提示：只输出简体中文译文
const TRANSLATE_SYSTEM_PROMPT: &str =
    "你是翻译助手。把用户给出的文本翻译成简体中文，只输出译文，不要解释。";
/// 单次翻译请求超时（大模型首 token 可能较慢）
const TRANSLATE_TIMEOUT: Duration = Duration::from_secs(20);

/// 翻译配置（`translate.*` 的运行时快照）。
struct TranslateConfig {
    api_key: String,
    url: String,
    model: String,
    headers: Vec<(&'static str, String)>,
}

impl TranslateConfig {
    fn from_settings(
        translate: &settings::TranslateDoc,
        resolve_platform: impl FnOnce(&str) -> Result<platforms::PlatformRequest, String>,
    ) -> Result<Self, String> {
        if translate.custom_enabled {
            let url = chat_url(&translate.base_url)?;
            let model = translate.model.trim();
            if model.is_empty() {
                return Err("请填写自定义翻译模型".into());
            }
            let api_key = effective_api_key(&translate.api_key, &translate.import_from);
            if api_key.is_empty() {
                return Err("请填写自定义翻译 API Key".into());
            }
            return Ok(Self {
                api_key,
                url,
                model: model.into(),
                headers: Vec::new(),
            });
        }

        let platform = translate.platform.trim();
        if platform.is_empty() {
            return Err("请在「翻译」设置页中选择并保存翻译平台".into());
        }
        let model = translate.platform_model.trim();
        if model.is_empty() {
            return Err("请在「翻译」设置页中选择并保存平台模型".into());
        }
        let request = resolve_platform(platform)?;
        Ok(Self {
            api_key: request.api_key,
            url: chat_url(&request.base_url)?,
            model: model.into(),
            headers: request.headers,
        })
    }
}

/// `island_translate` 的返回体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TranslateResult {
    pub text: String,
    pub translation: String,
    pub model: String,
}

// ============================================================
// 翻译：URL / payload / 响应 / Key
// ============================================================

/// 接口地址可以是 API 根地址或完整的 Chat Completions 端点。
fn chat_url(base: &str) -> Result<String, String> {
    let base = base.trim();
    if base.is_empty() {
        return Err("请填写翻译接口地址".into());
    }
    let invalid = "翻译接口地址必须是有效的 HTTP 或 HTTPS 地址";
    let mut url = tauri::Url::parse(base).map_err(|_| invalid)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid.into());
    }
    let path = url.path().trim_end_matches('/');
    let path = if path.to_ascii_lowercase().ends_with("/chat/completions") {
        path.to_string()
    } else {
        format!("{path}/chat/completions")
    };
    url.set_path(&path);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn build_payload(model: &str, text: &str) -> Value {
    json!({
        "model": model,
        "messages": [
            {"role": "system", "content": TRANSLATE_SYSTEM_PROMPT},
            {"role": "user", "content": text},
        ],
        "stream": false,
    })
}

/// 从 `choices[0].message.content` 取译文（trim）；缺字段 / 空内容给中文错误。
pub fn parse_completion(value: &Value) -> Result<String, String> {
    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .ok_or("翻译服务返回格式不正确（缺少 choices 数组）")?;
    let first = choices.first().ok_or("翻译服务没有返回任何译文")?;
    let content = first
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .ok_or("翻译服务返回格式不正确（缺少 choices[0].message.content）")?;
    let content = content.trim();
    // 部分兼容接口把推理放在消息正文开头；只返回后面的译文。
    let content = match content.strip_prefix("<think>") {
        Some(reasoning) => reasoning
            .split_once("</think>")
            .map(|(_, translation)| translation.trim())
            .unwrap_or_default(),
        None => content,
    };
    if content.is_empty() {
        return Err("翻译服务返回了空内容".into());
    }
    Ok(content.to_string())
}

/// 实际使用的 Key：优先 `translate.api_key`；为空时尝试 `translate.import_from`
/// 指向的 JSON（兼容 `{"translate":{"api_key":"..."}}` 与顶层
/// `{"api_key":"..."}`，读不到 / 解析失败一律静默回退为空）。
fn effective_api_key(direct: &str, import_from: &str) -> String {
    let direct = direct.trim();
    if !direct.is_empty() {
        return direct.to_string();
    }
    let path = import_from.trim();
    if path.is_empty() {
        return String::new();
    }
    read_imported_api_key(path).unwrap_or_default()
}

fn read_imported_api_key(path: &str) -> Option<String> {
    // 从资源管理器复制路径时常见首尾引号，一并去掉。
    let path = path.trim().trim_matches('"').trim();
    if path.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let nested = value
        .get("translate")
        .and_then(|translate| translate.get("api_key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty());
    let top_level = value
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty());
    nested.or(top_level).map(str::to_string)
}

/// 设置页测试和剪贴板热键均使用当前已保存的配置。
pub(super) fn translate_saved(text: &str) -> Result<TranslateResult, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("没有需要翻译的文本".into());
    }
    let cfg = TranslateConfig::from_settings(&settings::load().translate, platforms::resolve)?;
    let payload = build_payload(&cfg.model, text);
    let value = post_json(
        &cfg.url,
        &payload,
        TRANSLATE_TIMEOUT,
        Some(&cfg.api_key),
        &cfg.headers,
    )
    .map_err(|error| format!("翻译请求失败：{error}"))?;
    Ok(TranslateResult {
        text: text.into(),
        translation: parse_completion(&value)?,
        model: cfg.model,
    })
}

// ============================================================
// 实时字幕翻译的配置出口（供 island::live::translate 复用）
// ============================================================

/// 实时字幕翻译的请求目标：与设置窗「翻译」页共用同一份数据源。
///
/// 字段是 `pub(crate)`，只给 `island::live::translate` 用；这里不落任何缓存，
/// 每次字幕会话开始时解析一次（`live_start` 快照语义）。
#[derive(Clone)]
pub(crate) struct LiveTranslateTarget {
    /// 完整 `/chat/completions` 端点
    pub(crate) url: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
    /// 平台要求的额外请求头（如 OpenCode Go 的会话标识）
    pub(crate) headers: Vec<(&'static str, String)>,
    /// DeepSeek 与自定义接口支持 `thinking = {"type": "disabled"}`；
    /// 其它平台未必认这个字段（可能 422），不带。
    pub(crate) thinking_disabled: bool,
}

/// 组装实时字幕翻译目标：直接复用「翻译」页已保存的平台 / 自定义接口。
///
/// 未配置时返回中文错误，调用方（字幕管线）降级为「只出日文」，不阻断识别。
pub(crate) fn live_translate_target() -> Result<LiveTranslateTarget, String> {
    let doc = settings::load().translate;
    let custom = doc.custom_enabled;
    // 平台 id 形如 `deepseek:<credentialId>`：取 provider 段判断是否 DeepSeek。
    let provider = doc
        .platform
        .split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let config = TranslateConfig::from_settings(&doc, platforms::resolve)?;
    Ok(LiveTranslateTarget {
        url: config.url,
        api_key: config.api_key,
        model: config.model,
        headers: config.headers,
        thinking_disabled: custom || provider == "deepseek",
    })
}

/// 把错误详情里的 API Key 抹成 `***`（实时字幕翻译复用本模块口径）。
pub(crate) fn redact_secret(text: &str, secret: &str) -> String {
    redact(text, Some(secret))
}

/// POST JSON（可选 Bearer）。ureq 对非 2xx 返回 `Error::Status`，这里统一转成
/// 带状态码的错误；服务端返回的详情会先抹掉 API Key、再截断。
fn post_json(
    url: &str,
    payload: &Value,
    timeout: Duration,
    bearer: Option<&str>,
    headers: &[(&str, String)],
) -> Result<Value, String> {
    let mut builder = ureq::AgentBuilder::new().timeout(timeout);
    if bearer.is_some() {
        builder = builder.redirects(0);
    }
    let agent = builder.build();
    let mut request = agent.post(url).set("Content-Type", "application/json");
    if let Some(key) = bearer {
        request = request.set("Authorization", &format!("Bearer {key}"));
    }
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = match request.send_json(payload) {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let detail = response
                .into_string()
                .ok()
                .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                .and_then(|value| server_error_message(&value));
            return Err(match detail {
                Some(detail) => {
                    format!("HTTP {code}（{}）", truncate(&redact(&detail, bearer), 160))
                }
                None => format!("HTTP {code}"),
            });
        }
        Err(ureq::Error::Transport(error)) => {
            return Err(format!(
                "网络连接失败：{}",
                redact(&error.to_string(), bearer)
            ));
        }
    };
    let status = response.status();
    let body = response
        .into_string()
        .map_err(|error| format!("读取响应失败：{error}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    serde_json::from_str(&body).map_err(|_| "返回内容不是合法 JSON".to_string())
}

/// 服务端错误详情：兼容 `{"error":"..."}` 与 OpenAI 风格的
/// `{"error":{"message":"..."}}`。
fn server_error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    match error {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        Value::Object(_) => error
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// 错误详情里不允许出现 API Key。
fn redact(text: &str, secret: Option<&str>) -> String {
    match secret.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => text.replace(key, "***"),
        None => text.to_string(),
    }
}

/// 翻译一段文本（设置窗「测试翻译」与剪贴板热键共用）。
/// `target` 预留给多语言，目前只支持简体中文。
#[tauri::command]
pub async fn island_translate(
    text: String,
    target: Option<String>,
) -> Result<TranslateResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _ = target;
        translate_saved(&text)
    })
    .await
    .map_err(|error| format!("翻译任务失败：{error}"))?
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    /// 每个测试一个临时 JSON 文件，Drop 时清理（不碰真实用户配置）。
    struct TempJsonFile {
        path: PathBuf,
    }

    impl TempJsonFile {
        fn new(tag: &str, body: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "agenthud-translate-test-{}-{tag}.json",
                std::process::id()
            ));
            std::fs::write(&path, body).expect("写测试 JSON");
            Self { path }
        }

        fn path(&self) -> String {
            self.path.to_string_lossy().into_owned()
        }
    }

    impl Drop for TempJsonFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn chat_url_requires_http_and_handles_trailing_slashes() {
        assert!(chat_url("").is_err());
        assert!(chat_url("ftp://example.com").is_err());
        assert!(chat_url("https://user:password@example.com").is_err());
        assert_eq!(
            chat_url("https://example.com///").unwrap(),
            "https://example.com/chat/completions"
        );
        assert_eq!(
            chat_url("http://127.0.0.1:11434/v1/").unwrap(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert_eq!(
            chat_url("https://example.com/chat/completions/?api-version=1").unwrap(),
            "https://example.com/chat/completions?api-version=1"
        );
    }

    #[test]
    fn build_payload_matches_openai_shape() {
        let value = build_payload("selected-model", "Hello, world!");
        assert_eq!(value["model"], json!("selected-model"));
        assert_eq!(value["stream"], json!(false));
        let messages = value["messages"].as_array().expect("messages 数组");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], json!("system"));
        assert!(messages[0]["content"]
            .as_str()
            .expect("system content")
            .contains("简体中文"));
        assert_eq!(messages[1]["role"], json!("user"));
        assert_eq!(messages[1]["content"], json!("Hello, world!"));
    }

    #[test]
    fn parse_completion_trims_content() {
        let value = json!({
            "choices": [{"message": {"role": "assistant", "content": "  你好，世界 \n"}}]
        });
        assert_eq!(parse_completion(&value).unwrap(), "你好，世界");
    }

    #[test]
    fn parse_completion_uses_translation_after_inline_reasoning() {
        let value = json!({
            "choices": [{"message": {"content": " <think>reasoning</think> 你好 "}}]
        });
        assert_eq!(parse_completion(&value).unwrap(), "你好");
        let reasoning_only = json!({
            "choices": [{"message": {"content": "<think>unfinished reasoning"}}]
        });
        assert!(parse_completion(&reasoning_only).is_err());
    }

    #[test]
    fn parse_completion_rejects_missing_or_empty_content() {
        assert!(parse_completion(&json!({})).is_err());
        assert!(parse_completion(&json!({"choices": []})).is_err());
        assert!(parse_completion(&json!({"choices": [{"message": {}}]})).is_err());
        assert!(parse_completion(&json!({"choices": [{"message": {"content": "   "}}]})).is_err());
        assert!(parse_completion(&json!({"choices": [{"message": {"content": null}}]})).is_err());
        assert!(parse_completion(&json!("not-object")).is_err());
    }

    #[test]
    fn effective_api_key_prefers_direct_value() {
        let file = TempJsonFile::new("direct", r#"{"translate":{"api_key":"file-key"}}"#);
        assert_eq!(
            effective_api_key("  direct-key  ", &file.path()),
            "direct-key"
        );
    }

    #[test]
    fn effective_api_key_reads_nested_and_top_level_import() {
        let nested = TempJsonFile::new("nested", r#"{"translate":{"api_key":" nested-key "}}"#);
        assert_eq!(effective_api_key("", &nested.path()), "nested-key");

        let top = TempJsonFile::new("top", r#"{"api_key":"top-key"}"#);
        assert_eq!(effective_api_key("", &top.path()), "top-key");

        // 嵌套键存在但为空：回退顶层字符串
        let fallback = TempJsonFile::new(
            "fallback",
            r#"{"translate":{"api_key":""},"api_key":"outer-key"}"#,
        );
        assert_eq!(effective_api_key("", &fallback.path()), "outer-key");
    }

    #[test]
    fn effective_api_key_is_silent_on_broken_import() {
        let broken = TempJsonFile::new("broken", "{ not json");
        assert!(effective_api_key("", &broken.path()).is_empty());
        assert!(effective_api_key("", "Z:\\definitely-missing\\config.json").is_empty());
        assert!(effective_api_key("", "").is_empty());
    }

    #[test]
    fn translation_sources_are_mutually_exclusive() {
        let mut doc = settings::TranslateDoc {
            platform: " provider/account ".into(),
            platform_model: " platform-model ".into(),
            base_url: " https://custom.example/v1/ ".into(),
            model: " custom-model ".into(),
            api_key: " custom-key ".into(),
            ..settings::TranslateDoc::default()
        };
        assert!(!doc.custom_enabled);
        let platform = TranslateConfig::from_settings(&doc, |id| {
            assert_eq!(id, "provider/account");
            Ok(platforms::PlatformRequest {
                base_url: "https://platform.example/v1".into(),
                api_key: "platform-key".into(),
                headers: vec![("x-session", "translation".into())],
            })
        })
        .unwrap();
        assert_eq!(platform.url, "https://platform.example/v1/chat/completions");
        assert_eq!(platform.model, "platform-model");
        assert_eq!(platform.api_key, "platform-key");
        assert_eq!(platform.headers, vec![("x-session", "translation".into())]);

        doc.custom_enabled = true;
        let custom =
            TranslateConfig::from_settings(&doc, |_| panic!("自定义模式不能读取平台凭证")).unwrap();
        assert_eq!(custom.url, "https://custom.example/v1/chat/completions");
        assert_eq!(custom.model, "custom-model");
        assert_eq!(custom.api_key, "custom-key");
        assert!(custom.headers.is_empty());

        doc.custom_enabled = false;
        doc.platform.clear();
        let err = TranslateConfig::from_settings(&doc, |_| panic!("缺少平台时不能查找凭证"))
            .err()
            .unwrap();
        assert!(err.contains("选择并保存翻译平台"));
    }

    #[test]
    fn error_detail_hides_api_key() {
        assert_eq!(
            redact("bad key sk-secret rejected", Some("sk-secret")),
            "bad key *** rejected"
        );
        assert_eq!(redact("no secret", None), "no secret");
    }
}
