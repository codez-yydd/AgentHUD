//! 实时字幕翻译：OpenAI 兼容 `/chat/completions` 流式 SSE。
//!
//! 与设置窗「翻译」页共用同一份数据源（`island::translate` 的
//! `TranslateConfig` + `platforms::resolve`，外加一个 `pub(crate)` 辅助函数
//! [`crate::island::translate::live_translate_target`]）；密钥只在本机内存里
//! 使用，不写日志。
//!
//! ## 请求口径
//!
//! - system 提示词逐字使用 [`SYSTEM_PROMPT`]（日译中字幕风格 + 口癖 / 名台词
//!   / 网络用语约定）；
//! - user 内容：`【上文】\n<最后 ≤3 句日文>\n\n【台词】\n<当前句>`；没有上文时
//!   只有 `【台词】` 段；
//! - `temperature = 1.0`、`max_tokens = 200`、`stream = true`、
//!   `stream_options = {"include_usage": true}`；
//! - DeepSeek 与自定义接口额外带 `thinking = {"type": "disabled"}`（其它平台
//!   未必认这个字段，带了可能 422）；
//! - 响应解析 `data:` 行的 `choices[0].delta.content`，累计后交给回调；
//!   `[DONE]` 结束；HTTP 错误码映射成中文原因，401/402/403 不重试（由调用方
//!   决定，见 `pipeline`）。服务端不支持 / 忽略 `stream: true`（SSE 累计为空、
//!   回整段 JSON）时回退解析 `choices[0].message.content`（P2-5）。
//!
//! ## `live.asr.context_prompt` 的语义（本次适配决定）
//!
//! 参考实现里 `asr.context_prompt` 是死字段（Parakeet CTC 不吃 prompt）。
//! 本次把它定义为**翻译上文开关**：开启时把最近 `live.translate.context_lines`
//! 句定稿日文（上限 3 句）作为 `【上文】`；关闭时不带上文。这样该设置项
//! 才有真实行为，且与「上文仅供理解指代和语气、不翻译」的提示词约定一致。

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{json, Value};

use crate::island::translate::{live_translate_target, LiveTranslateTarget};

/// 翻译系统提示词（参考实现逐字使用，不要改标点 / 编号）。
pub(crate) const SYSTEM_PROMPT: &str = concat!(
    "你是日本动画的日译中字幕翻译。把用户给出的一句日语台词译成简体中文字幕。\n",
    "要求：\n",
    "1. 只输出译文，不加解释、注音、原文、引号或说话人。\n",
    "2. 口语化、简洁，符合中文字幕习惯；保留语气、情绪、时态、否定和疑问。\n",
    "3. 保留角色口癖与人设语气（如「〜にゃ」→喵、「〜ですわ」大小姐腔、「〜のじゃ」老派口吻、「〜っス」后辈腔），用自然的中文表达，不要抹平。\n",
    "4. 名台词、作品专有名词、招式名使用中文圈通行译法（如「全集中・水の呼吸」→全集中·水之呼吸，「知らない天井だ」→陌生的天花板）。\n",
    "5. 网络用语与俚语按语境意译：草→笑死，エモい→戳心/好有感觉，推し→本命/推，尊い→太神了，ワンチャン→有戏，陰キャ→阴角，やばい按褒贬选「绝了/不妙」。\n",
    "6. 谐音梗、双关尽量用中文谐音或保留笑点，实在不行就意译。\n",
    "7. 原文省略主语时不擅自补出，句子没说完时保留未完成的语气。\n",
    "8. 上文仅用于理解指代和语气，不要翻译上文。",
);

/// 流式翻译的读取超时（秒）：逐 data 行读，允许首 token 稍慢。
const TRANSLATE_TIMEOUT_SECS: u64 = 30;
/// 单句 `max_tokens`：字幕一句话足够，压住模型自由发挥。
const MAX_TOKENS: u32 = 200;

/// 翻译请求错误：`status` 是 HTTP 状态码（网络 / 解析错误为 `None`），
/// 调用方据此决定是否重试（401/402/403 不重试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveTranslateError {
    pub status: Option<u16>,
    pub message: String,
}

impl LiveTranslateError {
    fn network(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
        }
    }

    /// HTTP 错误码 → 中文原因（401/402/403/404/422/429/5xx 单独给可操作提示）。
    fn http(code: u16, detail: Option<&String>) -> Self {
        let reason = match code {
            401 => "API Key 无效",
            402 => "余额不足",
            403 => "无权限",
            404 => "模型名不对",
            422 => "参数错误（服务端不接受当前请求格式）",
            429 => "请求过于频繁",
            500..=599 => "翻译服务端错误",
            _ => "翻译请求失败",
        };
        let detail = detail
            .map(|raw| raw.trim())
            .filter(|raw| !raw.is_empty())
            .map(|raw| format!("：{}", truncate(raw, 160)));
        Self {
            status: Some(code),
            message: format!("{reason}（HTTP {code}）{}", detail.unwrap_or_default()),
        }
    }
}

impl std::fmt::Display for LiveTranslateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
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

/// user 消息：`【上文】` 段可选，`【台词】` 段必带。
fn user_content(jp: &str, context: &[String]) -> String {
    let mut content = String::new();
    if !context.is_empty() {
        content.push_str("【上文】\n");
        content.push_str(&context.join("\n"));
        content.push_str("\n\n");
    }
    content.push_str("【台词】\n");
    content.push_str(jp.trim());
    content
}

/// 组装 `/chat/completions` 请求体（纯函数，便于单测形状）。
pub(crate) fn build_payload(
    target: &LiveTranslateTarget,
    jp: &str,
    context: &[String],
) -> Value {
    let mut payload = json!({
        "model": target.model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_content(jp, context)},
        ],
        "temperature": 1.0,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if target.thinking_disabled {
        payload["thinking"] = json!({"type": "disabled"});
    }
    payload
}

/// 直连 Agent（本地 Ollama / LM Studio 等不该被系统代理绕出去）。
fn direct_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(TRANSLATE_TIMEOUT_SECS))
        .build()
}

/// 接口地址是否指向本机（loopback 一律直连，不走用户配置的代理）。
fn is_loopback(url: &str) -> bool {
    let Ok(url) = tauri::Url::parse(url) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default().trim_start_matches('[');
    host.eq_ignore_ascii_case("localhost")
        || host.starts_with("127.")
        || host.trim_end_matches(']') == "::1"
}

/// 构造流式请求 Agent：本机接口直连；其它优先复用应用代理配置
/// （`net_config::http_agent`），代理串非法时退回直连，避免一句字幕都翻不出来。
fn build_agent(url: &str) -> ureq::Agent {
    if is_loopback(url) {
        return direct_agent();
    }
    match crate::net_config::http_agent(TRANSLATE_TIMEOUT_SECS) {
        Ok(agent) => agent,
        Err(error) => {
            eprintln!("[zbar-island-live] 翻译代理配置不可用，改走直连: {error}");
            direct_agent()
        }
    }
}

/// 解析一段 SSE 文本流：把 `choices[0].delta.content` 累计后回调。
///
/// 回调参数是**累计译文**（不是增量）：前端 / 字幕历史都按累计文本覆盖同一
/// 条字幕。回调返回 `false` 表示调用方主动放弃（定稿到达 / 历史被清空），
/// 立即停止读取。
///
/// P2-5：部分 OpenAI 兼容服务端不支持 / 忽略 `stream: true`，直接回整段
/// JSON。SSE 累计为空时回退解析普通响应的 `choices[0].message.content`
/// （兼容 `delta.content` / 旧版 `text`），避免定稿翻译整句失败。流里实在
/// 没有任何内容时给带「当前服务端似乎不支持流式」提示的中文错误。
pub(crate) fn consume_sse(
    mut reader: impl BufRead,
    on_delta: &mut dyn FnMut(&str) -> bool,
) -> Result<(), LiveTranslateError> {
    let mut accumulated = String::new();
    // 原样保留响应文本：非流式回退时整段按 JSON 解析。
    let mut raw = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| LiveTranslateError::network(format!("读取翻译流失败：{error}")))?;
        if read == 0 {
            break;
        }
        raw.push_str(&line);
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with(':') {
            continue;
        }
        let Some(data) = trimmed.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(error) = value.get("error") {
            return Err(LiveTranslateError::network(format!(
                "翻译服务返回错误：{}",
                truncate(&error.to_string(), 200)
            )));
        }
        let Some(delta) = value
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        else {
            continue;
        };
        if delta.is_empty() {
            continue;
        }
        accumulated.push_str(delta);
        if !on_delta(&accumulated) {
            return Ok(());
        }
    }
    if accumulated.trim().is_empty() {
        // 非流式回退：服务端忽略 stream，回的是普通 `/chat/completions` JSON。
        match plain_response_content(&raw) {
            Ok(Some(text)) if !text.trim().is_empty() => {
                let _ = on_delta(text.trim());
                return Ok(());
            }
            Err(error) => return Err(error),
            _ => {}
        }
        return Err(LiveTranslateError::network(
            "翻译服务没有返回内容（当前服务端似乎不支持流式，请检查接口地址或模型是否支持 stream）",
        ));
    }
    Ok(())
}

/// 非流式响应兜底：解析整段 JSON 的 `choices[0].message.content`
/// （兼容 `delta.content` / 旧版 `text`），服务端错误体原样转中文错误。
///
/// `Ok(None)` = 不是可识别的 JSON 响应（保持原有「没有返回内容」错误）。
fn plain_response_content(raw: &str) -> Result<Option<String>, LiveTranslateError> {
    let Ok(value) = serde_json::from_str::<Value>(raw.trim()) else {
        return Ok(None);
    };
    if let Some(error) = value.get("error") {
        return Err(LiveTranslateError::network(format!(
            "翻译服务返回错误：{}",
            truncate(&error.to_string(), 200)
        )));
    }
    let text = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
        })
        .or_else(|| value.pointer("/choices/0/text").and_then(Value::as_str));
    Ok(text.map(str::to_string))
}

/// 发起一次流式翻译。`on_delta` 收到的是累计译文。
pub(crate) fn stream(
    target: &LiveTranslateTarget,
    jp: &str,
    context: &[String],
    on_delta: &mut dyn FnMut(&str) -> bool,
) -> Result<(), LiveTranslateError> {
    if jp.trim().is_empty() {
        return Ok(());
    }
    let payload = build_payload(target, jp, context);
    let agent = build_agent(&target.url);
    let mut request = agent
        .post(&target.url)
        .set("Content-Type", "application/json")
        .set("Accept", "text/event-stream");
    let api_key = target.api_key.trim();
    if !api_key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {api_key}"));
    }
    for (name, value) in &target.headers {
        request = request.set(name, value);
    }
    let response = match request.send_json(&payload) {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            // 服务端在 4xx/5xx 时通常会回 JSON 错误详情，先抹掉 Key 再拼进中文原因。
            let body = response.into_string().ok();
            let detail = body
                .as_deref()
                .and_then(|body| serde_json::from_str::<Value>(body).ok())
                .and_then(|value| server_error_message(&value))
                .map(|detail| crate::island::translate::redact_secret(&detail, api_key));
            return Err(LiveTranslateError::http(code, detail.as_ref()));
        }
        Err(ureq::Error::Transport(error)) => {
            let detail = crate::island::translate::redact_secret(&error.to_string(), api_key);
            return Err(LiveTranslateError::network(format!("网络连接失败：{detail}")));
        }
    };
    let reader = BufReader::new(response.into_reader());
    // 流内错误事件也可能回显请求内容：统一再抹一次 Key。
    consume_sse(reader, on_delta).map_err(|mut error| {
        error.message = crate::island::translate::redact_secret(&error.message, api_key);
        error
    })
}

/// 兼容 `{"error":"..."}` 与 `{"error":{"message":"..."}}` 两种错误体。
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

/// 草稿译文过滤：模型偶尔把整段输出成 `***` / Markdown 星号，直接丢弃。
pub(crate) fn star_heavy(text: &str) -> bool {
    let total = text.chars().count();
    let stars = text
        .chars()
        .filter(|ch| matches!(ch, '*' | '＊'))
        .count();
    if stars == 0 {
        return false;
    }
    stars >= 3 || stars * 5 >= total
}

/// 解析失败时给调用方一个可读的中文错误（单测 / 日志用）。
pub(crate) fn target_or_warn() -> Option<LiveTranslateTarget> {
    match live_translate_target() {
        Ok(target) => Some(target),
        Err(error) => {
            eprintln!("[zbar-island-live] 翻译未配置，已降级为只出日文: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn target(model: &str, thinking_disabled: bool) -> LiveTranslateTarget {
        LiveTranslateTarget {
            url: "https://api.example.com/chat/completions".into(),
            api_key: "sk-test".into(),
            model: model.into(),
            headers: vec![("x-session", "abc".into())],
            thinking_disabled,
        }
    }

    #[test]
    fn payload_matches_reference_shape() {
        let value = build_payload(
            &target("deepseek-chat", true),
            "おはよう",
            &["一句目".into(), "二句目".into()],
        );
        assert_eq!(value["model"], json!("deepseek-chat"));
        assert_eq!(value["temperature"], json!(1.0));
        assert_eq!(value["max_tokens"], json!(200));
        assert_eq!(value["stream"], json!(true));
        assert_eq!(value["stream_options"]["include_usage"], json!(true));
        assert_eq!(value["thinking"]["type"], json!("disabled"));
        let messages = value["messages"].as_array().expect("messages 数组");
        assert_eq!(messages.len(), 2);
        let system = messages[0]["content"].as_str().expect("system 文本");
        assert!(system.starts_with("你是日本动画的日译中字幕翻译。"));
        assert!(system.contains("8. 上文仅用于理解指代和语气，不要翻译上文。"));
        let user = messages[1]["content"].as_str().expect("user 文本");
        assert_eq!(user, "【上文】\n一句目\n二句目\n\n【台词】\nおはよう");
    }

    #[test]
    fn payload_without_context_has_no_context_section() {
        let value = build_payload(&target("m", false), "  ただいま  ", &[]);
        assert!(value.get("thinking").is_none(), "非 DeepSeek 平台不带 thinking");
        let user = value["messages"][1]["content"].as_str().expect("user 文本");
        assert_eq!(user, "【台词】\nただいま");
        assert!(!user.contains("【上文】"));
    }

    #[test]
    fn consume_sse_accumulates_deltas_and_stops_on_done() {
        let body = concat!(
            ": keep-alive\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"\u{597d}\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let mut pushes: Vec<String> = Vec::new();
        consume_sse(Cursor::new(body), &mut |text| {
            pushes.push(text.to_string());
            true
        })
        .expect("SSE 应解析成功");
        assert_eq!(pushes, vec!["你", "你好"]);
    }

    #[test]
    fn consume_sse_aborts_when_callback_rejects() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"A\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"B\"}}]}\n\n",
        );
        let mut count = 0;
        consume_sse(Cursor::new(body), &mut |_| {
            count += 1;
            false
        })
        .expect("回调拒绝不算错误");
        assert_eq!(count, 1, "回调返回 false 后必须停止读取");
    }

    #[test]
    fn consume_sse_reports_empty_and_server_error_and_ignores_garbage() {
        assert!(consume_sse(Cursor::new("data: [DONE]\n"), &mut |_| true).is_err());
        let error = consume_sse(
            Cursor::new("data: {\"error\":{\"message\":\"bad key\"}}\n"),
            &mut |_| true,
        )
        .expect_err("错误事件必须上抛");
        assert!(error.message.contains("bad key"), "实际：{}", error.message);
        // 非法 JSON 行忽略；没有任何内容 => 报「没有返回内容」
        assert!(consume_sse(Cursor::new("data: {oops\n"), &mut |_| true).is_err());
    }

    /// P2-5：服务端不支持流式、直接回整段 JSON 时必须回退解析
    /// `choices[0].message.content`，而不是整句翻译失败。
    #[test]
    fn consume_sse_falls_back_to_plain_json_response() {
        let body = "{\"id\":\"x\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"你好，世界\"}}]}";
        let mut pushes: Vec<String> = Vec::new();
        consume_sse(Cursor::new(body), &mut |text| {
            pushes.push(text.to_string());
            true
        })
        .expect("非流式 JSON 应回退解析成功");
        assert_eq!(pushes, vec!["你好，世界"]);

        // 服务端 pretty print 成多行也要能解析（按行读不丢内容）。
        let pretty = "{\n  \"choices\": [\n    {\"message\": {\"content\": \"ただいま\"}}\n  ]\n}\n";
        let mut pushes: Vec<String> = Vec::new();
        consume_sse(Cursor::new(pretty), &mut |text| {
            pushes.push(text.to_string());
            true
        })
        .expect("多行普通 JSON 应回退解析成功");
        assert_eq!(pushes, vec!["ただいま"]);

        // 200 + 普通 JSON 错误体：仍要上抛可读错误（并抹掉 Key 由 stream 负责）。
        let error = consume_sse(
            Cursor::new("{\"error\":{\"message\":\"bad key\"}}"),
            &mut |_| true,
        )
        .expect_err("普通 JSON 错误体必须上抛");
        assert!(error.message.contains("bad key"), "实际：{}", error.message);

        // 完全不是 JSON：保留原「没有返回内容」语义，但要提示可能不支持流式。
        let error = consume_sse(Cursor::new("data: [DONE]\n"), &mut |_| true)
            .expect_err("空流必须报错");
        assert!(
            error.message.contains("不支持流式"),
            "错误文案应提示服务端不支持流式，实际：{}",
            error.message
        );
    }

    #[test]
    fn http_error_maps_codes_to_chinese_reasons() {
        for (code, reason) in [
            (401u16, "API Key 无效"),
            (402, "余额不足"),
            (403, "无权限"),
            (404, "模型名不对"),
            (422, "参数错误"),
            (429, "请求过于频繁"),
            (503, "翻译服务端错误"),
        ] {
            let error = LiveTranslateError::http(code, Some(&"detail".to_string()));
            assert!(
                error.message.contains(reason),
                "HTTP {code} 应含「{reason}」，实际：{}",
                error.message
            );
            assert_eq!(error.status, Some(code));
        }
        assert!(LiveTranslateError::http(418, None).message.contains("HTTP 418"));
    }

    #[test]
    fn star_heavy_filters_markdown_noise() {
        assert!(!star_heavy("你好"));
        assert!(!star_heavy(""));
        // Markdown 强调 / 星号刷屏都不该出现在字幕里
        assert!(star_heavy("**强调**"));
        assert!(star_heavy("***"));
        assert!(star_heavy("＊ ＊ ＊ ＊"));
        assert!(star_heavy("******"));
    }

    #[test]
    fn loopback_detection_skips_proxy() {
        assert!(is_loopback("http://127.0.0.1:11434/v1/chat/completions"));
        assert!(is_loopback("http://localhost:1234/chat/completions"));
        assert!(is_loopback("http://[::1]:8080/chat/completions"));
        assert!(!is_loopback("https://api.deepseek.com/chat/completions"));
        assert!(!is_loopback("not a url"));
    }

    /// 起一个只服务一次请求的本地 HTTP 服务（测试流式解析 / 请求体形状）。
    fn serve_once(
        response: String,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地测试端口");
        let addr = listener.local_addr().expect("读取本地端口");
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("接受测试连接");
            let request = read_http_request(&mut socket);
            use std::io::Write as _;
            socket
                .write_all(response.as_bytes())
                .expect("写本地测试响应");
            let _ = socket.flush();
            request
        });
        (format!("http://{addr}/chat/completions"), handle)
    }

    /// 读一次完整 HTTP 请求（头 + Content-Length 体），用于断言请求形状。
    fn read_http_request(socket: &mut std::net::TcpStream) -> String {
        use std::io::Read as _;
        let mut data = Vec::new();
        let mut buffer = [0u8; 4096];
        let header_end = loop {
            if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            let read = socket.read(&mut buffer).expect("读取请求头");
            if read == 0 {
                break data.len();
            }
            data.extend_from_slice(&buffer[..read]);
        };
        let headers = String::from_utf8_lossy(&data[..header_end]).to_ascii_lowercase();
        let content_length = headers
            .split("content-length:")
            .nth(1)
            .and_then(|rest| rest.split("\r\n").next())
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while data.len() < header_end + content_length {
            let read = socket.read(&mut buffer).expect("读取请求体");
            if read == 0 {
                break;
            }
            data.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8_lossy(&data).to_string()
    }

    /// 端到端流式翻译（本地 SSE 服务）：请求体形状 + 累计译文回调。
    #[test]
    fn stream_parses_local_sse_end_to_end() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"，世界\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
            sse.len()
        );
        let (url, server) = serve_once(response);
        let mut target = target("test-model", true);
        target.url = url;
        target.headers = vec![("x-session", "case-1".into())];
        let mut pushes: Vec<String> = Vec::new();
        stream(&target, "おはよう", &["一句目".into()], &mut |text| {
            pushes.push(text.to_string());
            true
        })
        .expect("本地 SSE 流式翻译应成功");
        assert_eq!(pushes.first().map(String::as_str), Some("你好"));
        assert_eq!(pushes.last().map(String::as_str), Some("你好，世界"));

        let request = server.join().expect("本地服务线程");
        assert!(
            request.starts_with("POST /chat/completions "),
            "实际请求行：{request}"
        );
        assert!(request.contains("\"model\":\"test-model\""));
        assert!(request.contains("\"stream\":true"));
        assert!(request.contains("\"thinking\":{\"type\":\"disabled\"}"));
        assert!(request.contains("\"content\":\"【上文】\\n一句目\\n\\n【台词】\\nおはよう\""));
        assert!(
            request.to_ascii_lowercase().contains("authorization: bearer sk-test"),
            "实际请求头：{request}"
        );
        assert!(request.to_ascii_lowercase().contains("x-session: case-1"));
    }

    /// P2-5 端到端：服务端忽略 `stream: true`、返回普通 JSON 时仍能翻译。
    #[test]
    fn stream_falls_back_when_server_ignores_stream() {
        let body = "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"你好\"}}]}";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (url, server) = serve_once(response);
        let mut target = target("test-model", false);
        target.url = url;
        let mut pushes: Vec<String> = Vec::new();
        stream(&target, "おはよう", &[], &mut |text| {
            pushes.push(text.to_string());
            true
        })
        .expect("非流式服务端也应能翻译");
        assert_eq!(pushes, vec!["你好"]);

        let request = server.join().expect("本地服务线程");
        assert!(
            request.contains("\"stream\":true"),
            "请求仍按流式发起（回退发生在解析侧）：{request}"
        );
    }

    /// HTTP 错误映射：401 要变中文原因、带状态码，且错误详情里不能出现 Key。
    #[test]
    fn stream_maps_http_401_to_chinese_reason() {
        let body = "{\"error\":{\"message\":\"bad key sk-live-secret\"}}";
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (url, server) = serve_once(response);
        let mut target = target("test-model", false);
        target.url = url;
        target.api_key = "sk-live-secret".into();
        let error = stream(&target, "おはよう", &[], &mut |_| true).expect_err("401 必须报错");
        assert_eq!(error.status, Some(401), "实际：{:?}", error);
        assert!(error.message.contains("API Key 无效"), "实际：{}", error.message);
        assert!(
            !error.message.contains("sk-live-secret"),
            "错误详情必须抹掉 Key：{}",
            error.message
        );
        let _ = server.join();
    }
}
