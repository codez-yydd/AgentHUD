//! 各 Agent 源本地凭证概览（主窗口「凭证与账号管理」页区块二的数据源）。
//!
//! 聚合各源 CLI 落在本机的登录/凭证文件的**只读静态信息**：源 id、账号标识
//! （email 或账号名）、凭证类型（OAuth / API Key / 未知）、凭证文件路径、
//! 最近修改时间、文件是否存在。供用户统一查看各处登录态，不做任何管理操作。
//!
//! 安全红线（与 provider_credentials 的 secret 纪律同源）：
//! - 凭证内容 / token / key 明文**绝不**返回前端，也**绝不**写日志——本模块
//!   只读文件提取元信息，错误串只含 IO/解析原因，不含文件内容片段；
//! - 一期纯静态：本模块**不发起任何 HTTP 请求**（不试连、不刷新、不校验），
//!   kimi 的续期熔断状态为进程内私有状态（无对外 getter），不在此暴露。
//!
//! 路径定位与各源模块的既有口径逐字对齐（环境变量覆盖 + 默认位置），但实现
//! 刻意独立：各模块的定位函数多为私有，本模块不改动任何既有模块，仅在注释
//! 里锚定对应出处，两侧路径规则需保持同步：
//! - codex：ZBAR_CODEX_HOME / CODEX_HOME → ~/.codex（auth.json，见 codex.rs
//!   的 load_codex_auth）
//! - claude：ZBAR_CLAUDE_HOME → ~/.claude（.credentials.json，见 claude.rs 的
//!   load_claude_auth；账号 email 取自 ~/.claude.json 的 oauthAccount，见下）
//! - gemini：ZBAR_GEMINI_HOME → ~/.gemini（oauth_creds.json，见 gemini.rs）
//! - grok：GROK_HOME → ~/.grok（auth.json，见 grok.rs）
//! - kimi：ZBAR_KIMI_HOME / KIMI_CODE_HOME → ~/.kimi-code/credentials/*.json
//!   （多文件并存取 mtime 最新，见 kimi.rs 的 scan_cli_credentials）
//! - opencode：ZBAR_OPENCODE_HOME → ~/.local/share/opencode（auth.json，见
//!   opencodego.rs 的 has_go_login）
//! - minimax：ZBAR_MINIMAX_HOME → ~/.minimax/auth/**（下探至多 3 层目录找
//!   auth.json，取 mtime 最新，见 minimax.rs 的 local_auth_files）
//! - dsh：ZBAR_DSH_HOME → ~/.dsh（.credentials.yaml，见 dsh.rs 模块头注释）
//!
//! 容错模型：单源提取失败（读失败 / 非法 JSON / 目录异常）只落该源条目的
//! error 字段（exists 仍如实标记），不阻塞其他源；整个命令只在任务本身
//! 失败时返回 Err。文件缺失是正常态（exists=false、error=None，前端显示
//! 「未检测到」），不算错误。

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 源条目里「多文件取最新」与「目录下探搜索」的 mtime 兜底值（读不到
/// metadata 的文件排在最后，与 kimi.rs scan_cli_credentials 同口径）。
const EPOCH: SystemTime = SystemTime::UNIX_EPOCH;

/// 单个源的凭证概览条目（fetch_credential_overview 返回元素）。
/// 字段命名沿 accounts.rs 的 AccountMeta 惯例（snake_case，前端同构）。
#[derive(Debug, Clone, Serialize)]
pub struct CredentialSourceOverview {
    /// 源 id（codex/claude/gemini/grok/kimi/opencode/minimax/dsh，
    /// 前端按此映射品牌名）
    pub source: String,
    /// 凭证类型："oauth" | "apiKey" | "unknown"
    pub kind: String,
    /// 账号标识（email 或账号名；提取不到为 None，绝不臆造）
    pub account: Option<String>,
    /// 凭证文件路径（缺失态显示该源的预期位置；kimi 缺失时为 credentials
    /// 目录、minimax 缺失时为 auth 目录）
    pub path: String,
    /// 文件最近修改时间（毫秒时间戳；读不到为 None）
    pub modified_at: Option<i64>,
    /// 凭证文件是否存在
    pub exists: bool,
    /// 单源提取失败原因（缺失为正常态不算错误；串不含文件内容）
    pub error: Option<String>,
}

// ============================================================
// 通用小工具（只读，无副作用）
// ============================================================

/// 读凭证文件文本的三态封装：Ok(None) = 文件不存在（正常缺失态）；
/// Ok(Some) = 内容；Err = 存在但读取失败（权限/IO 故障，视为该源错误）。
fn read_text(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取失败: {e}")),
    }
}

/// 文件 mtime → 毫秒时间戳（读不到为 None）。
fn mtime_ms(path: &Path) -> Option<i64> {
    let meta = fs::metadata(path).ok()?;
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

/// 从 JSON 值取非空字符串字段（trim 后为空视为缺失）。
fn non_empty_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|s| s.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 「单文件凭证」条目的公共骨架：缺失 → exists=false；读取失败 / 非法 JSON →
/// exists=true + error；解析成功 → exists=true，account/kind 由 extract 回填。
fn single_file_overview(
    source: &str,
    path: &Path,
    extract: impl Fn(&serde_json::Value) -> (String, Option<String>),
) -> CredentialSourceOverview {
    let mut item = CredentialSourceOverview {
        source: source.to_string(),
        kind: "unknown".to_string(),
        account: None,
        path: path.display().to_string(),
        modified_at: mtime_ms(path),
        exists: false,
        error: None,
    };
    let text = match read_text(path) {
        Ok(None) => return item, // 文件不存在：正常缺失态
        Ok(Some(t)) => t,
        Err(e) => {
            item.exists = true;
            item.error = Some(e);
            return item;
        }
    };
    item.exists = true;
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) => {
            let (kind, account) = extract(&v);
            item.kind = kind;
            item.account = account;
        }
        Err(e) => {
            item.error = Some(format!("文件不是合法 JSON: {e}"));
        }
    }
    item
}

/// base64url 解码（JWT payload 用；不足位补 padding；损坏输入返回 None）。
fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.trim())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(s.trim()))
        .ok()
}

/// 从 id_token（JWT）payload 解出 email（不验签，仅展示账号标识用；与
/// gemini.rs 的 decode_jwt_email 同口径，损坏/缺 email 静默 None）。
fn jwt_email(id_token: &str) -> Option<String> {
    let payload_b64 = id_token.trim().split('.').nth(1)?;
    let bytes = b64url_decode(payload_b64)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    non_empty_str(&v, "email")
}

// ============================================================
// 各源提取（纯函数，路径注入便于单测，不碰真实用户目录）
// ============================================================

/// Codex：~/.codex/auth.json（新版 tokens 嵌套 / 旧版扁平双兼容，与
/// codex.rs 的 load_codex_auth 同口径）。有 access_token 即视为 OAuth 登录，
/// 账号标识取 account_id。
fn codex_overview(root: &Path) -> CredentialSourceOverview {
    single_file_overview("codex", &root.join("auth.json"), |v| {
        let token = v
            .pointer("/tokens/access_token")
            .and_then(|t| t.as_str())
            .or_else(|| v.get("access_token").and_then(|t| t.as_str()))
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let account = v
            .pointer("/tokens/account_id")
            .and_then(|t| t.as_str())
            .or_else(|| v.get("account_id").and_then(|t| t.as_str()))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let kind = if token.is_some() { "oauth" } else { "unknown" };
        (kind.to_string(), account)
    })
}

/// Claude：~/.claude/.credentials.json（与 claude.rs 的 parse_credentials_json
/// 同口径：claudeAiOauth.accessToken）。另补 Claude Code 2.1.x 的落盘键形：
/// 账号 OAuth 未落盘时文件可能仅含 mcpOAuth（按 MCP 服务器标识索引的
/// OAuth 条目对象，每条含 accessToken/refreshToken/expiresAt 等，见 claude.rs
/// 的 MissingOAuth 注释），条目带非空 accessToken 亦是 OAuth 形凭证文件，
/// kind 同样归 oauth（quota 侧仍按无账号登录处理，两处口径各自独立）。
/// 账号标识尽力取家目录 ~/.claude.json 的 oauthAccount.emailAddress
/// （Claude Code 的账号信息落在该配置文件，凭证文件本身无 email）；
/// 读取/解析失败静默 None，不影响凭证文件本身的状态判定。
fn claude_overview(creds_path: &Path, account_json: &Path) -> CredentialSourceOverview {
    let mut item = single_file_overview("claude", creds_path, |v| {
        let token = v
            .pointer("/claudeAiOauth/accessToken")
            .and_then(|t| t.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty());
        // 仅含 mcpOAuth 的键形：任一条目带非空 accessToken 即 OAuth 凭证
        let mcp_oauth = v
            .get("mcpOAuth")
            .and_then(|m| m.as_object())
            .map(|entries| {
                entries
                    .values()
                    .any(|e| non_empty_str(e, "accessToken").is_some())
            })
            .unwrap_or(false);
        let kind = if token.is_some() || mcp_oauth {
            "oauth"
        } else {
            "unknown"
        };
        (kind.to_string(), None)
    });
    if item.exists && item.error.is_none() {
        item.account = read_text(account_json)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|v| {
                v.pointer("/oauthAccount/emailAddress")
                    .and_then(|e| e.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            });
    }
    item
}

/// Gemini：~/.gemini/oauth_creds.json（与 gemini.rs 的 parse_creds 同口径）。
/// access/refresh/id_token 任一存在即 OAuth；账号标识从 id_token（JWT）解 email。
fn gemini_overview(root: &Path) -> CredentialSourceOverview {
    single_file_overview("gemini", &root.join("oauth_creds.json"), |v| {
        let has_oauth = ["access_token", "refresh_token", "id_token"]
            .iter()
            .any(|k| non_empty_str(v, k).is_some());
        let kind = if has_oauth { "oauth" } else { "unknown" };
        let account = non_empty_str(v, "id_token").as_deref().and_then(jwt_email);
        (kind.to_string(), account)
    })
}

/// Grok：~/.grok/auth.json（顶层键为 scope URL，OIDC 前缀优先、旧会话键
/// 回退，与 grok.rs 的 select_local_auth 同口径）。账号标识取条目 email。
fn grok_overview(root: &Path) -> CredentialSourceOverview {
    const OIDC_SCOPE_PREFIX: &str = "https://auth.x.ai::";
    const LEGACY_SESSION_SCOPE: &str = "https://accounts.x.ai/sign-in";
    single_file_overview("grok", &root.join("auth.json"), |v| {
        let mut oidc: Option<&serde_json::Value> = None;
        let mut legacy: Option<&serde_json::Value> = None;
        let Some(obj) = v.as_object() else {
            return ("unknown".to_string(), None);
        };
        for (scope, entry) in obj {
            if non_empty_str(entry, "key").is_none() {
                continue; // 残缺条目（无 key）不能挤掉健康条目
            }
            if scope.starts_with(OIDC_SCOPE_PREFIX) {
                oidc = oidc.or(Some(entry));
            } else if scope == LEGACY_SESSION_SCOPE || scope.contains("/sign-in") {
                legacy = legacy.or(Some(entry));
            }
        }
        let entry = oidc.or(legacy);
        match entry {
            Some(e) => ("oauth".to_string(), non_empty_str(e, "email")),
            // 文件在且是合法 JSON 但没有可用登录条目（残缺记录）：类型未知
            None => ("unknown".to_string(), None),
        }
    })
}

/// Kimi：~/.kimi-code/credentials/*.json（多文件取 mtime 最新，与 kimi.rs
/// 的 scan_cli_credentials 同口径）。OAuth 三件套（access_token +
/// refresh_token + expires_at）齐全 → oauth；否则带 apiKey/api_key/
/// accessToken/access_token/token 任一非空键 → apiKey（键集合与 kimi.rs
/// 的 extract_token_from_value 对齐，不同版本/登录方式的字段名不统一，
/// 仅带 accessToken 的文件也是可用凭据，不得误判 unknown）。文件缺失/
/// 目录缺失为正常缺失态（path 显示 credentials 目录）。
/// 目录下有 json 文件但全部无法解析 → 错误态（不静默吞掉坏文件）。
fn kimi_overview(root: &Path) -> CredentialSourceOverview {
    let dir = root.join("credentials");
    let mut item = CredentialSourceOverview {
        source: "kimi".to_string(),
        kind: "unknown".to_string(),
        account: None,
        path: dir.display().to_string(),
        modified_at: None,
        exists: false,
        error: None,
    };
    // 收集目录下的 json 文件并按 mtime 降序（读不到 mtime 的垫底）
    let mut files: Vec<(SystemTime, PathBuf)> = fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let path = e.path();
                    if path.extension().map(|x| x == "json").unwrap_or(false) {
                        Some((
                            e.metadata()
                                .and_then(|m| m.modified())
                                .unwrap_or(EPOCH),
                            path,
                        ))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    if files.is_empty() {
        return item; // 目录缺失 / 无 json 文件：正常缺失态
    }
    for (_, path) in &files {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => continue, // 单文件读失败：尝试下一个（更旧的）文件
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        item.exists = true;
        item.path = path.display().to_string();
        item.modified_at = mtime_ms(path);
        let is_oauth = [
            non_empty_str(&v, "access_token").is_some(),
            non_empty_str(&v, "refresh_token").is_some(),
            v.get("expires_at").and_then(|t| t.as_i64()).is_some(),
        ]
        .iter()
        .all(|ok| *ok);
        let kind = if is_oauth {
            "oauth"
        } else if ["apiKey", "api_key", "accessToken", "access_token", "token"]
            .iter()
            .any(|k| non_empty_str(&v, k).is_some())
        {
            // 键集合与 kimi.rs extract_token_from_value 逐键对齐：否则仅带
            // accessToken（camelCase，OAuth 三件套不齐）的凭据文件会被
            // 误判为 unknown，而额度查询侧其实认它
            "apiKey"
        } else {
            "unknown"
        };
        item.kind = kind.to_string();
        item.account = None; // 凭据文件无账号字段，绝不臆造
        return item;
    }
    // 有 json 文件但全部读不出/解析不了：错误态（文件在，exists 如实为 true）
    let newest = &files[0].1;
    item.exists = true;
    item.path = newest.display().to_string();
    item.modified_at = mtime_ms(newest);
    item.error = Some("credentials 目录下的凭据文件均无法解析".to_string());
    item
}

/// OpenCode：~/.local/share/opencode/auth.json。顶层 "opencode-go" 内层
/// "key" 非空为订阅登录（oauth，与 opencodego.rs 的 has_go_login 同口径）；
/// 其余条目带 type=oauth → oauth，带非空 key/api 字符串 → apiKey，都无 →
/// unknown。账号标识：auth.json 无账号字段，None。
fn opencode_overview(root: &Path) -> CredentialSourceOverview {
    single_file_overview("opencode", &root.join("auth.json"), |v| {
        let go_login = v
            .pointer("/opencode-go/key")
            .and_then(|k| k.as_str())
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .is_some();
        if go_login {
            return ("oauth".to_string(), None);
        }
        let mut saw_oauth = false;
        let mut saw_key = false;
        if let Some(obj) = v.as_object() {
            for entry in obj.values() {
                if non_empty_str(entry, "type").as_deref() == Some("oauth") {
                    saw_oauth = true;
                }
                if ["key", "api"]
                    .iter()
                    .any(|k| non_empty_str(entry, k).is_some())
                {
                    saw_key = true;
                }
            }
        }
        let kind = if saw_oauth {
            "oauth"
        } else if saw_key {
            "apiKey"
        } else {
            "unknown"
        };
        (kind.to_string(), None)
    })
}

/// Minimax：~/.minimax/auth 下至多 3 层目录搜索 auth.json（真实布局
/// auth/prod/cn/<clientId>/auth.json，与 minimax.rs 的 local_auth_files 同
/// 口径），取 mtime 最新。records 里任一 accessToken 非空 → oauth，账号
/// 标识取该条 clientId。找不到任何 auth.json → 正常缺失态（path 显示
/// auth 目录）。
fn minimax_overview(root: &Path) -> CredentialSourceOverview {
    const MAX_DEPTH: usize = 3;
    let auth_root = root.join("auth");
    let mut found: Vec<PathBuf> = vec![];
    collect_named_files(&auth_root, "auth.json", MAX_DEPTH, &mut found);
    let mut item = CredentialSourceOverview {
        source: "minimax".to_string(),
        kind: "unknown".to_string(),
        account: None,
        path: auth_root.display().to_string(),
        modified_at: None,
        exists: false,
        error: None,
    };
    // mtime 最新优先
    found.sort_by_key(|p| std::cmp::Reverse(mtime_ms(p).unwrap_or(0)));
    for path in &found {
        let text = match read_text(path) {
            Ok(Some(t)) => t,
            _ => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue, // 残缺文件跳过，尝试下一个
        };
        item.exists = true;
        item.path = path.display().to_string();
        item.modified_at = mtime_ms(path);
        let mut account = None;
        let mut saw_token = false;
        if let Some(records) = v.get("records").and_then(|r| r.as_object()) {
            for record in records.values() {
                if non_empty_str(record, "accessToken").is_some() {
                    saw_token = true;
                    if account.is_none() {
                        account = non_empty_str(record, "clientId");
                    }
                }
            }
        }
        item.kind = if saw_token {
            "oauth".to_string()
        } else {
            "unknown".to_string()
        };
        item.account = account;
        return item;
    }
    if !found.is_empty() {
        // 有 auth.json 但全部读不出/解析不了：错误态
        let newest = &found[0];
        item.exists = true;
        item.path = newest.display().to_string();
        item.modified_at = mtime_ms(newest);
        item.error = Some("auth.json 文件无法解析".to_string());
    }
    item
}

/// 递归收集指定文件名的直接命中（文件名匹配即收，目录继续下探；深度用尽
/// 停止）。供 minimax 的 auth 目录搜索（也可复用给后续同形来源）。
fn collect_named_files(dir: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // 目录不存在/不可读：无命中
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // depth 语义：还可下探几层目录（3 = env/region/clientId 三层目录
            // 内的文件都能命中；当前目录内的文件收集不受 depth 限制）
            if depth >= 1 {
                collect_named_files(&path, name, depth - 1, out);
            }
        } else if path.file_name().map(|n| n == name).unwrap_or(false) {
            out.push(path);
        }
    }
}

/// DSH：~/.dsh/.credentials.yaml（DeepSeek 平台账号登录态；YAML 非本模块
/// 解析范围，仅按文本特征分类：出现 `kind:` 记录字段即视为 OAuth 授权
/// （grant）登录态，否则 unknown。账号标识 None（yaml 里的记录名是固定
/// 的服务标识而非账号，不冒充账号名）。
fn dsh_overview(root: &Path) -> CredentialSourceOverview {
    let path = root.join(".credentials.yaml");
    let mut item = CredentialSourceOverview {
        source: "dsh".to_string(),
        kind: "unknown".to_string(),
        account: None,
        path: path.display().to_string(),
        modified_at: mtime_ms(&path),
        exists: false,
        error: None,
    };
    match read_text(&path) {
        Ok(None) => {} // 未登录 / 未装 DSH：正常缺失态（item 维持初值）
        Ok(Some(text)) => {
            item.exists = true;
            if text.contains("kind:") {
                item.kind = "oauth".to_string();
            }
        }
        Err(e) => {
            item.exists = true;
            item.error = Some(e);
        }
    }
    item
}

// ============================================================
// 路径定位（环境变量口径与各源模块对齐，见模块头注释）
// ============================================================

/// 覆盖型环境变量解析：取第一个非空值；都未设置返回 None。
fn env_dir(keys: &[&str]) -> Option<PathBuf> {
    keys.iter().find_map(|k| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
}

/// 默认家目录（与各源模块相同的兜底写法）。
fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// 聚合全部源的凭证概览（生产入口：真实路径解析 + 逐源提取）。
/// 任何单源异常都只体现在该源条目的 error 字段，本函数不返回 Err。
fn credential_overview() -> Vec<CredentialSourceOverview> {
    let home = home();
    vec![
        codex_overview(
            &env_dir(&["ZBAR_CODEX_HOME", "CODEX_HOME"]).unwrap_or_else(|| home.join(".codex")),
        ),
        claude_overview(
            &env_dir(&["ZBAR_CLAUDE_HOME"])
                .unwrap_or_else(|| home.join(".claude"))
                .join(".credentials.json"),
            &home.join(".claude.json"),
        ),
        gemini_overview(&env_dir(&["ZBAR_GEMINI_HOME"]).unwrap_or_else(|| home.join(".gemini"))),
        grok_overview(&env_dir(&["GROK_HOME"]).unwrap_or_else(|| home.join(".grok"))),
        kimi_overview(
            &env_dir(&["ZBAR_KIMI_HOME", "KIMI_CODE_HOME"])
                .unwrap_or_else(|| home.join(".kimi-code")),
        ),
        opencode_overview(
            &env_dir(&["ZBAR_OPENCODE_HOME"])
                .unwrap_or_else(|| home.join(".local").join("share").join("opencode")),
        ),
        minimax_overview(&env_dir(&["ZBAR_MINIMAX_HOME"]).unwrap_or_else(|| home.join(".minimax"))),
        dsh_overview(&env_dir(&["ZBAR_DSH_HOME"]).unwrap_or_else(|| home.join(".dsh"))),
    ]
}

// ============================================================
// Tauri command（纯文件 IO，统一 async + spawn_blocking 惯例）
// ============================================================

/// 各 Agent 源本地凭证概览（主窗口「凭证与账号管理」页区块二）。
/// 只读静态信息，不发 HTTP、不返回任何凭证明文；单源失败不阻塞其他源。
#[tauri::command]
pub async fn fetch_credential_overview() -> Result<Vec<CredentialSourceOverview>, String> {
    tauri::async_runtime::spawn_blocking(credential_overview)
        .await
        .map_err(|e| format!("凭证概览聚合任务失败: {e}"))
}

// ============================================================
// 单元测试（temp 目录注入路径，不触碰真实用户目录；也不读环境变量，
// 测试只调纯函数入口，避免并行测试互改 env 的竞态）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试独立的临时目录，结束时清理（accounts.rs 的 TempBase 同款）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "zbar-credoverview-test-{tag}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_millis()
            ));
            fs::create_dir_all(&dir).expect("创建临时目录失败");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Codex 三态：存在（新版 tokens 嵌套 → oauth + account_id）/ 缺失 /
    /// 非法 JSON（exists=true + error）。
    #[test]
    fn codex_overview_three_states() {
        let tmp = TempDir::new("codex");

        // ① 存在：新版 tokens 嵌套结构
        fs::write(
            tmp.0.join("auth.json"),
            r#"{"tokens":{"access_token":"at","account_id":"acc-123","refresh_token":"rt"}}"#,
        )
        .unwrap();
        let item = codex_overview(&tmp.0);
        assert!(item.exists);
        assert_eq!(item.kind, "oauth");
        assert_eq!(item.account.as_deref(), Some("acc-123"));
        assert!(item.error.is_none());
        assert!(item.modified_at.is_some());

        // 旧版扁平结构同样兼容（account_id 顶层）
        fs::write(
            tmp.0.join("auth.json"),
            r#"{"access_token":"at","account_id":"old-id"}"#,
        )
        .unwrap();
        assert_eq!(
            codex_overview(&tmp.0).account.as_deref(),
            Some("old-id"),
            "旧版扁平 auth.json 也应取到 account_id"
        );

        // ② 缺失：正常缺失态，不算错误
        let missing = codex_overview(&tmp.0.join("nope"));
        assert!(!missing.exists);
        assert!(missing.error.is_none());
        assert_eq!(missing.kind, "unknown");

        // ③ 非法 JSON：文件在 → exists=true + error，不 panic
        fs::write(tmp.0.join("auth.json"), "not a json").unwrap();
        let broken = codex_overview(&tmp.0);
        assert!(broken.exists);
        assert!(broken.error.is_some());
        assert_eq!(broken.kind, "unknown");
        assert!(broken.account.is_none());
        // 错误串不含文件内容（安全红线：明文不进错误消息）
        assert!(!broken.error.unwrap().contains("not a json"));
    }

    /// Claude：凭证文件 OAuth + 账号 email 尽力取自 ~/.claude.json；
    /// 凭证文件缺失时即使账号配置存在也算缺失态。
    #[test]
    fn claude_overview_extracts_email_from_claude_json() {
        let tmp = TempDir::new("claude");
        let creds = tmp.0.join(".credentials.json");
        let account_json = tmp.0.join(".claude.json");

        fs::write(
            &creds,
            r#"{"claudeAiOauth":{"accessToken":"at","refreshToken":"rt","expiresAt":1}}"#,
        )
        .unwrap();
        fs::write(
            &account_json,
            r#"{"oauthAccount":{"emailAddress":"me@anthropic.test"},"projects":{}}"#,
        )
        .unwrap();
        let item = claude_overview(&creds, &account_json);
        assert!(item.exists);
        assert_eq!(item.kind, "oauth");
        assert_eq!(item.account.as_deref(), Some("me@anthropic.test"));

        // 账号配置文件缺失/损坏：不阻塞凭证态，账号标识落 None
        let item = claude_overview(&creds, &tmp.0.join("absent.json"));
        assert_eq!(item.kind, "oauth");
        assert!(item.account.is_none());

        // 凭证文件缺失：整体缺失态
        let missing = claude_overview(&tmp.0.join("none.json"), &account_json);
        assert!(!missing.exists);
        assert!(missing.error.is_none());
    }

    /// Claude Code 2.1.x 键形：凭证文件仅含 mcpOAuth（按 MCP 服务器标识
    /// 索引的 OAuth 条目）而无 claudeAiOauth——同样是 OAuth 形凭证文件，
    /// kind=oauth。键名/条目名均为测试伪造，不含真实凭证值。
    #[test]
    fn claude_overview_mcp_only_credentials_is_oauth() {
        let tmp = TempDir::new("claude-mcp");
        let creds = tmp.0.join(".credentials.json");

        // 仅 mcpOAuth、条目带 accessToken → oauth（实机复现「未知」的键形）
        fs::write(
            &creds,
            r#"{"mcpOAuth":{"plugin:demo:demo|0f0f0f0f":{"serverName":"demo","serverUrl":"https://mcp.demo.test","accessToken":"at","refreshToken":"rt","expiresAt":1,"scope":"mcp"}}}"#,
        )
        .unwrap();
        let item = claude_overview(&creds, &tmp.0.join("absent.json"));
        assert!(item.exists);
        assert_eq!(item.kind, "oauth");
        assert!(item.account.is_none());
        assert!(item.error.is_none());

        // mcpOAuth 在但条目无 accessToken（残缺）：不冒充 OAuth，仍 unknown
        fs::write(
            &creds,
            r#"{"mcpOAuth":{"plugin:demo:demo|0f0f":{"serverName":"demo","serverUrl":"https://mcp.demo.test"}}}"#,
        )
        .unwrap();
        let item = claude_overview(&creds, &tmp.0.join("absent.json"));
        assert!(item.exists);
        assert_eq!(item.kind, "unknown");
        assert!(item.error.is_none());

        // 空的 mcpOAuth 对象：无任何条目 → unknown
        fs::write(&creds, r#"{"mcpOAuth":{}}"#).unwrap();
        assert_eq!(
            claude_overview(&creds, &tmp.0.join("absent.json")).kind,
            "unknown"
        );
    }

    /// Gemini：oauth_creds.json → oauth，账号 email 从 id_token（JWT）解出；
    /// 非法 JSON → 错误态。
    #[test]
    fn gemini_overview_decodes_jwt_email() {
        let tmp = TempDir::new("gemini");
        // 手造 JWT：header.payload.signature，payload 带 email
        let payload = b64url_encode(r#"{"email":"dev@google.test"}"#);
        let jwt = format!("h.{payload}.sig");
        fs::write(
            tmp.0.join("oauth_creds.json"),
            format!(r#"{{"access_token":"at","refresh_token":"rt","id_token":"{jwt}"}}"#),
        )
        .unwrap();
        let item = gemini_overview(&tmp.0);
        assert!(item.exists);
        assert_eq!(item.kind, "oauth");
        assert_eq!(item.account.as_deref(), Some("dev@google.test"));

        // 非法 JSON：错误态
        fs::write(tmp.0.join("oauth_creds.json"), "{oops").unwrap();
        let broken = gemini_overview(&tmp.0);
        assert!(broken.exists);
        assert!(broken.error.is_some());
        assert!(broken.account.is_none());
    }

    /// Grok：OIDC scope 前缀键优先，账号标识取条目 email；无可用条目 →
    /// 合法 JSON 但 kind=unknown。
    #[test]
    fn grok_overview_prefers_oidc_entry() {
        let tmp = TempDir::new("grok");
        fs::write(
            tmp.0.join("auth.json"),
            r#"{
              "https://accounts.x.ai/sign-in": {"key": "legacy", "email": "legacy@x.test"},
              "https://auth.x.ai::123": {"key": "oidc", "email": "oidc@x.test"}
            }"#,
        )
        .unwrap();
        let item = grok_overview(&tmp.0);
        assert_eq!(item.kind, "oauth");
        assert_eq!(item.account.as_deref(), Some("oidc@x.test"));

        // 只有残缺条目（无 key）：不算登录，kind=unknown 且不报错
        fs::write(
            tmp.0.join("auth.json"),
            r#"{"https://auth.x.ai::123": {"email": "broken@x.test"}}"#,
        )
        .unwrap();
        let item = grok_overview(&tmp.0);
        assert!(item.exists);
        assert_eq!(item.kind, "unknown");
        assert!(item.account.is_none());
        assert!(item.error.is_none());
    }

    /// Kimi：多文件取 mtime 最新；OAuth 三件套齐全 → oauth，纯 apiKey →
    /// apiKey；目录缺失 → 正常缺失态（path 指向 credentials 目录）。
    #[test]
    fn kimi_overview_picks_latest_and_classifies() {
        let tmp = TempDir::new("kimi");
        let dir = tmp.0.join("credentials");
        fs::create_dir_all(&dir).unwrap();

        // 目录缺失：正常缺失态
        let missing = kimi_overview(&tmp.0.join("nope"));
        assert!(!missing.exists);
        assert!(missing.error.is_none());
        assert!(missing.path.ends_with("credentials"));

        // 旧文件（OAuth）与新文件（apiKey）：取 mtime 最新 → apiKey
        fs::write(
            dir.join("old.json"),
            r#"{"access_token":"a","refresh_token":"r","expires_at":123}"#,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.join("new.json"), r#"{"apiKey":"sk-xxx"}"#).unwrap();
        let item = kimi_overview(&tmp.0);
        assert!(item.exists);
        assert_eq!(item.kind, "apiKey");
        assert!(item.path.ends_with("new.json"));

        // 只有 OAuth 三件套时 → oauth
        fs::remove_file(dir.join("new.json")).unwrap();
        let item = kimi_overview(&tmp.0);
        assert_eq!(item.kind, "oauth");

        // 全部非法 JSON：错误态（不静默）
        fs::write(dir.join("old.json"), "broken").unwrap();
        let broken = kimi_overview(&tmp.0);
        assert!(broken.exists);
        assert!(broken.error.is_some());
    }

    /// kimi 的 apiKey 分类键集合与 kimi.rs extract_token_from_value 逐键
    /// 对齐：仅带 accessToken（camelCase）或三件套不齐的 access_token
    /// （snake）的凭据文件都是额度查询侧认的可用凭据，概览必须归为
    /// apiKey，不得误判 unknown。各场景用独立子目录，避开多文件 mtime
    /// 排序的不确定性。
    #[test]
    fn kimi_overview_token_keys_align_with_kimi_rs() {
        let tmp = TempDir::new("kimi-keys");
        for (case, content) in [
            ("camel", r#"{"accessToken":"at"}"#),
            ("snake_partial_oauth", r#"{"access_token":"at","expires_at":123}"#),
        ] {
            let root = tmp.0.join(case);
            let dir = root.join("credentials");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("cred.json"), content).unwrap();
            let item = kimi_overview(&root);
            assert!(item.exists, "{case}：文件在即视为已检测到");
            assert_eq!(
                item.kind, "apiKey",
                "{case}：OAuth 三件套不齐时按 token 类键归 apiKey"
            );
            assert!(item.error.is_none());
        }

        // 无任何 token 类键（也不构成 OAuth）：照旧 unknown
        let root = tmp.0.join("none");
        let dir = root.join("credentials");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cred.json"), r#"{"foo":"bar"}"#).unwrap();
        assert_eq!(kimi_overview(&root).kind, "unknown");
    }

    /// OpenCode：opencode-go 订阅登录 → oauth；仅普通 provider 条目 →
    /// apiKey / unknown。
    #[test]
    fn opencode_overview_classifies_entries() {
        let tmp = TempDir::new("opencode");

        fs::write(tmp.0.join("auth.json"), r#"{"opencode-go":{"key":"go-key"}}"#).unwrap();
        let item = opencode_overview(&tmp.0);
        assert_eq!(item.kind, "oauth");
        assert!(item.account.is_none());

        fs::write(
            tmp.0.join("auth.json"),
            r#"{"anthropic":{"type":"oauth","access":"a"}}"#,
        )
        .unwrap();
        assert_eq!(opencode_overview(&tmp.0).kind, "oauth");

        fs::write(
            tmp.0.join("auth.json"),
            r#"{"openrouter":{"api":"sk-or"}}"#,
        )
        .unwrap();
        assert_eq!(opencode_overview(&tmp.0).kind, "apiKey");

        // 空对象：合法 JSON 但无凭据形态 → unknown（缺失态另测）
        fs::write(tmp.0.join("auth.json"), "{}").unwrap();
        assert_eq!(opencode_overview(&tmp.0).kind, "unknown");

        let missing = opencode_overview(&tmp.0.join("nope"));
        assert!(!missing.exists);
        assert!(missing.error.is_none());
    }

    /// Minimax：auth/<env>/<region>/<clientId>/auth.json 深层布局可搜到，
    /// 账号标识取 clientId；auth 目录缺失 → 正常缺失态。
    #[test]
    fn minimax_overview_finds_nested_auth() {
        let tmp = TempDir::new("minimax");
        let nested = tmp.0.join("auth").join("prod").join("cn").join("mcode-public");
        fs::create_dir_all(&nested).unwrap();
        // records 的 key 不参与解析逻辑（本模块只遍历 values），用空格代替
        // 真实键名中的 NUL 分隔符即可（见 minimax.rs 的 parse_login_records）
        fs::write(
            nested.join("auth.json"),
            r#"{"records":{"com.minimax.mcode.oauth.prod.cn sub":{"accessToken":"at","expiresAtMs":9e15,"clientId":"mcode-public"}}}"#,
        )
        .unwrap();
        let item = minimax_overview(&tmp.0);
        assert!(item.exists);
        assert_eq!(item.kind, "oauth");
        assert_eq!(item.account.as_deref(), Some("mcode-public"));
        assert!(item.path.ends_with("auth.json"));

        // auth 目录缺失：正常缺失态，path 指向 auth 目录
        let missing = minimax_overview(&tmp.0.join("nope"));
        assert!(!missing.exists);
        assert!(missing.error.is_none());
        assert!(missing.path.ends_with("auth"));
    }

    /// DSH：.credentials.yaml 出现 kind: 记录 → oauth（grant 登录态）；
    /// 缺失 → 正常缺失态。
    #[test]
    fn dsh_overview_detects_grant_record() {
        let tmp = TempDir::new("dsh");
        fs::write(
            tmp.0.join(".credentials.yaml"),
            "deepseek-account-platform/default:\n  kind: grant\n  token: secret-value\n",
        )
        .unwrap();
        let item = dsh_overview(&tmp.0);
        assert!(item.exists);
        assert_eq!(item.kind, "oauth");
        assert!(item.account.is_none());

        let missing = dsh_overview(&tmp.0.join("nope"));
        assert!(!missing.exists);
        assert!(missing.error.is_none());
    }

    /// JWT payload 的 base64url 编码辅助（测试造 id_token 用）。
    fn b64url_encode(s: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
    }
}
