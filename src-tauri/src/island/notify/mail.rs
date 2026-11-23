//! Poll configured IMAP inboxes over implicit TLS without modifying them.
//!
//! 默认只读统计 `INBOX` 的新增未读邮件并投递一条「N 封新邮件」。打开
//! `mail.code_preview`（默认开）后，额外用 `BODY.PEEK[...]` 读取最新几封
//! 新邮件的**头部与 64KB 正文**，只在内存里抽取验证码（不落盘、不进日志、
//! 不回写已读状态：全程 `EXAMINE` + `BODY.PEEK`），抽到时每封投递一条带
//! `code` 的消息条目；一封都没抽到就仍投递原来的聚合数量提醒。

use super::{emit, push_item, running, NotifyItem, NotifyPrefs, SYNTH_ID};
use crate::island::settings::{self, ImapAccount};
use async_imap::error::Error as ImapError;
use async_imap::imap_proto::{
    AttributeValue, MailboxDatum, MessageSection, Response, SectionPath, Status,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{lookup_host, TcpStream};
use tokio::runtime::{Builder, Runtime};
use tokio::time::timeout;
use tokio_rustls::{client::TlsStream, TlsConnector};

const INBOX_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
/// 验证码正文读取的整轮预算。超时只是放弃内容读取（已抽到的保留），
/// 聚合数量提醒照旧；不会拖长既有的 `INBOX_TIMEOUT`。
const CONTENT_TIMEOUT: Duration = Duration::from_secs(6);
/// 每轮最多读取正文的邮件数（按 UID 从大到小取最新的几封）。
const CODE_FETCH_MAX: usize = 5;
/// 每轮最多投递的验证码条目数。
const CODE_PUSH_MAX: usize = 3;
/// 单封邮件正文的读取上限（与服务端 partial fetch 的 65536 对齐）。
const CODE_TEXT_LIMIT: usize = 64 * 1024;
/// 只取需要的头字段 + 正文前 64KB；两个 PEEK 保证不改 `\Seen`。
const CODE_FETCH_QUERY: &str =
    "(BODY.PEEK[HEADER.FIELDS (SUBJECT FROM DATE)] BODY.PEEK[TEXT]<0.65536>)";
const MAIL: &str = "mail";
static WORKER: Mutex<Option<(u64, Thread)>> = Mutex::new(None);
static SETTINGS_VERSION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, PartialEq, Eq, Hash)]
struct AccountKey {
    host: String,
    port: u16,
    user: String,
}

#[derive(PartialEq, Eq)]
struct Account {
    key: AccountKey,
    password: String,
}

fn accounts(imap: Vec<ImapAccount>) -> Vec<Account> {
    let mut seen = HashSet::new();
    imap.into_iter()
        .filter(|account| account.enabled && !account.password.is_empty())
        .filter_map(|account| {
            let key = AccountKey {
                host: account.host.to_ascii_lowercase(),
                port: u16::try_from(account.port).ok()?,
                user: account.user,
            };
            seen.insert(key.clone()).then_some(Account {
                key,
                password: account.password,
            })
        })
        .collect()
}

fn configuration() -> (Duration, Vec<Account>, bool) {
    if !NotifyPrefs::from_config().module_enabled(MAIL) {
        return (Duration::ZERO, Vec::new(), false);
    }
    let settings = settings::load().mail;
    (
        Duration::from_secs(settings.interval_s.clamp(30, 3600) as u64),
        accounts(settings.imap),
        settings.code_preview,
    )
}

pub(super) fn settings_changed() {
    SETTINGS_VERSION.fetch_add(1, Ordering::Release);
    if let Ok(worker) = WORKER.lock() {
        if let Some((_, thread)) = &*worker {
            thread.unpark();
        }
    }
}

pub(super) fn start(app: AppHandle, gen: u64) {
    let _ = thread::Builder::new()
        .name("zbar-island-imap".into())
        .spawn(move || {
            let Ok(runtime) = Builder::new_current_thread().enable_all().build() else {
                eprintln!("[zbar-island-imap] 无法启动 IMAP 检查服务");
                return;
            };
            {
                let Ok(mut worker) = WORKER.lock() else {
                    return;
                };
                if !running(gen) {
                    return;
                }
                if let Some((_, previous_thread)) = worker.replace((gen, thread::current())) {
                    previous_thread.unpark();
                }
            }
            poll(&app, gen, &runtime);
            if let Ok(mut worker) = WORKER.lock() {
                if worker.as_ref().is_some_and(|(current, _)| *current == gen) {
                    *worker = None;
                }
            }
        });
}

fn poll(app: &AppHandle, gen: u64, runtime: &Runtime) {
    let mut last_poll: Option<Instant> = None;
    let mut previous = Vec::new();
    let mut cursors = HashMap::new();
    while running(gen) {
        let version = SETTINGS_VERSION.load(Ordering::Acquire);
        let (interval, configured, code_preview) = configuration();
        if configured != previous {
            cursors.retain(|key, _| configured.iter().any(|account| account.key == *key));
            last_poll = None;
            previous = configured;
        }
        if !running(gen) {
            break;
        }
        if previous.is_empty() {
            thread::park();
            continue;
        }
        if let Some(remaining) = last_poll.and_then(|last| interval.checked_sub(last.elapsed())) {
            if !remaining.is_zero() {
                thread::park_timeout(remaining);
                continue;
            }
        }
        last_poll = Some(Instant::now());
        for account in &previous {
            if !account_is_current(version, gen, account) {
                continue;
            }
            // Thunderbird 托管时只要数量，不读正文：提前判定，避免头部与 64KB
            // 正文被拉下来又整条丢弃。
            let handled = thunderbird_handles(&account.key);
            let update = match runtime.block_on(check_inbox(
                account,
                cursors.get(&account.key).copied(),
                code_preview && !handled,
            )) {
                Ok(update) => update,
                Err(stage) => {
                    // Server text can echo credentials. Only log the local stage name.
                    eprintln!("[zbar-island-imap] {stage}");
                    continue;
                }
            };
            if !account_is_current(version, gen, account) {
                continue;
            }
            cursors.insert(account.key.clone(), update.cursor);
            if update.new_unread == 0 || handled {
                continue;
            }
            let prefs = NotifyPrefs::from_config();
            let now_ms = chrono::Utc::now().timestamp_millis();
            // 有验证码时每封一条独立提醒；一封都没抽到才退回「N 封新邮件」聚合。
            let mut changed = false;
            for mail in update.codes.into_iter().take(CODE_PUSH_MAX) {
                let item = NotifyItem {
                    id: SYNTH_ID.fetch_sub(1, Ordering::SeqCst),
                    kind: MAIL.into(),
                    name: "邮件".into(),
                    sender: mail.sender,
                    body: if mail.subject.is_empty() {
                        "新邮件".into()
                    } else {
                        mail.subject
                    },
                    code: mail.code,
                    time: mail.time.unwrap_or(now_ms),
                    read: false,
                    exe: String::new(),
                };
                changed |= push_item(item, &prefs);
            }
            if !changed {
                let item = NotifyItem {
                    id: SYNTH_ID.fetch_sub(1, Ordering::SeqCst),
                    kind: MAIL.into(),
                    name: "邮件".into(),
                    sender: account.key.user.chars().take(80).collect(),
                    body: format!("{} 封新邮件", update.new_unread),
                    code: String::new(),
                    time: now_ms,
                    read: false,
                    exe: String::new(),
                };
                changed = push_item(item, &prefs);
            }
            if changed && running(gen) {
                emit(app);
            }
        }
    }
}

fn account_is_current(version: u64, gen: u64, account: &Account) -> bool {
    running(gen)
        && (SETTINGS_VERSION.load(Ordering::Acquire) == version
            || configuration().1.contains(account))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    validity: u32,
    next_uid: u64,
}

/// 一封新邮件里抽出的验证码：只保留投递要用的字段，正文原样丢弃、不落盘。
#[derive(Debug, Clone, PartialEq, Eq)]
struct MailCode {
    sender: String,
    subject: String,
    code: String,
    /// `Date` 头可解析时的毫秒时间戳；解析失败为 None（投递时用当前时间）。
    time: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
struct InboxUpdate {
    cursor: Cursor,
    new_unread: usize,
    /// 本轮抽到验证码的邮件（开关关闭或没抽到时为空）。
    codes: Vec<MailCode>,
}

fn tls_config() -> Option<Arc<ClientConfig>> {
    static CONFIG: OnceLock<Option<Arc<ClientConfig>>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .ok()?
            .with_root_certificates(roots)
            .with_no_client_auth();
            Some(Arc::new(config))
        })
        .clone()
}

async fn check_inbox(
    account: &Account,
    cursor: Option<Cursor>,
    code_preview: bool,
) -> Result<InboxUpdate, &'static str> {
    timeout(INBOX_TIMEOUT, async {
        let stream = connect_tls(&account.key).await?;
        unread_updates(stream, account, cursor, code_preview).await
    })
    .await
    .map_err(|_| "IMAP 检查超时")?
}

async fn connect_tls(key: &AccountKey) -> Result<TlsStream<TcpStream>, &'static str> {
    let host = key.host.as_str();
    let name = ServerName::try_from(host.to_owned()).map_err(|_| "IMAP 服务器地址无效")?;
    let connector = TlsConnector::from(tls_config().ok_or("无法初始化 IMAP TLS")?);
    let addresses = lookup_host((host, key.port))
        .await
        .map_err(|_| "无法解析 IMAP 服务器地址")?;
    for address in addresses {
        if let Ok(Ok(socket)) = timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
            return connector
                .connect(name, socket)
                .await
                .map_err(|_| "无法建立受信任的 IMAP TLS 连接");
        }
    }
    Err("无法连接 IMAP 服务器")
}

async fn unread_updates<S: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send>(
    stream: S,
    account: &Account,
    cursor: Option<Cursor>,
    code_preview: bool,
) -> Result<InboxUpdate, &'static str> {
    let mut client = async_imap::Client::new(stream);
    client
        .read_response()
        .await
        .map_err(|_| "无法读取 IMAP 服务器问候")?
        .ok_or("IMAP 服务器关闭了连接")?;
    let mut session = client
        .login(&account.key.user, &account.password)
        .await
        .map_err(|_| "IMAP 登录失败，请检查账户和授权码")?;
    // NetEase expects an IMAP ID before opening the mailbox.
    let host = account.key.host.as_str();
    if [".163.com", ".126.com", ".yeah.net"]
        .iter()
        .any(|suffix| host.ends_with(suffix))
    {
        match session
            .id([
                ("name", Some("AgentHUD")),
                ("version", Some(env!("CARGO_PKG_VERSION"))),
            ])
            .await
        {
            Ok(_) | Err(ImapError::Bad(_) | ImapError::No(_)) => {}
            Err(_) => return Err("无法完成 IMAP 客户端标识交换"),
        }
    }
    let mailbox = session
        .examine("INBOX")
        .await
        .map_err(|_| "无法只读打开 IMAP 收件箱")?;
    let validity = mailbox
        .uid_validity
        .filter(|value| *value > 0)
        .ok_or("IMAP 服务器未提供有效收件箱 UIDVALIDITY")?;
    let mut next_uid = if let Some(uid) = mailbox.uid_next.filter(|value| *value > 0) {
        u64::from(uid)
    } else {
        let all = checked_uid_search(&mut session, "ALL")
            .await
            .ok_or("无法检查 IMAP 收件箱游标")?;
        u64::from(all.into_iter().max().unwrap_or(0)) + 1
    };
    let mut new_unread = 0;
    let mut unread_uids: Vec<u32> = Vec::new();
    // A missing baseline or mailbox UID reset starts a new baseline. UIDNEXT
    // advances independently of read flags, so marking an old message unread
    // cannot result in a fresh notification.
    if let Some(cursor) = cursor.filter(|cursor| cursor.validity == validity) {
        next_uid = next_uid.max(cursor.next_uid);
        if cursor.next_uid <= u64::from(u32::MAX) {
            let query = format!("UID {}:{} UNSEEN", cursor.next_uid, u32::MAX);
            let uids = checked_uid_search(&mut session, &query)
                .await
                .ok_or("无法查询 IMAP 收件箱未读邮件")?;
            for uid in uids
                .into_iter()
                .map(u64::from)
                .filter(|uid| *uid >= cursor.next_uid)
            {
                // Messages may arrive after EXAMINE returns its UIDNEXT.
                next_uid = next_uid.max(uid + 1);
                new_unread += 1;
                if let Ok(uid) = u32::try_from(uid) {
                    unread_uids.push(uid);
                }
            }
        }
    }
    // 开关关闭时这里一次都不发：命令序列与旧版完全一致（LOGIN / EXAMINE /
    // SEARCH / LOGOUT），不会读取任何正文。
    let mut codes = Vec::new();
    if code_preview && !unread_uids.is_empty() {
        unread_uids.sort_unstable_by(|a, b| b.cmp(a));
        unread_uids.truncate(CODE_FETCH_MAX);
        let deadline = Instant::now() + CONTENT_TIMEOUT;
        for uid in unread_uids {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match timeout(remaining, fetch_mail_code(&mut session, uid)).await {
                Ok(Some(mail)) => {
                    codes.push(mail);
                    if codes.len() >= CODE_PUSH_MAX {
                        break;
                    }
                }
                Ok(None) => {}
                // 内容读取超时：放弃剩余正文，已抽到的照常投递，没有就退回聚合。
                Err(_) => break,
            }
        }
    }
    let _ = timeout(Duration::from_secs(2), session.logout()).await;
    Ok(InboxUpdate {
        cursor: Cursor { validity, next_uid },
        new_unread,
        codes,
    })
}

async fn checked_uid_search<S: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send>(
    session: &mut async_imap::Session<S>,
    query: &str,
) -> Option<HashSet<u32>> {
    // A tagged NO/BAD must leave the last successful UID cursor in place.
    let request = session.run_command(format!("UID SEARCH {query}")).await.ok()?;
    let mut ids = HashSet::new();
    while let Some(response) = session.read_response().await.ok()? {
        match response.parsed() {
            Response::MailboxData(MailboxDatum::Search(values)) => ids.extend(values),
            Response::Done { tag, status, .. } if *tag == request => {
                return matches!(status, Status::Ok).then_some(ids);
            }
            _ => {}
        }
    }
    None
}

/// 读取一封新邮件（只读）：头部三个字段 + 正文前 64KB。
///
/// 用 `run_command` + `read_response` 而不是 `uid_fetch` 返回的 Stream，是为了
/// 复用本文件既有「自己配对 tagged Done」的写法：Stream 必须完整消费到 Done，
/// 中途丢弃会让会话滞留未响应的字面量；这里读到 Done 就返回，语义更直白。
async fn fetch_mail_code<S: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send>(
    session: &mut async_imap::Session<S>,
    uid: u32,
) -> Option<MailCode> {
    let request = session
        .run_command(format!("UID FETCH {uid} {CODE_FETCH_QUERY}"))
        .await
        .ok()?;
    let mut header: Option<Vec<u8>> = None;
    let mut text: Option<Vec<u8>> = None;
    while let Some(response) = session.read_response().await.ok()? {
        match response.parsed() {
            Response::Fetch(_, attributes) => {
                for attribute in attributes {
                    match attribute {
                        AttributeValue::BodySection {
                            section: Some(SectionPath::Full(MessageSection::Header)),
                            data: Some(data),
                            ..
                        } => header = Some(data.to_vec()),
                        AttributeValue::BodySection {
                            section: Some(SectionPath::Full(MessageSection::Text)),
                            data: Some(data),
                            ..
                        } => text = Some(data.to_vec()),
                        _ => {}
                    }
                }
            }
            Response::Done { tag, status, .. } if *tag == request => {
                // 服务端不支持 partial fetch / 拒绝读取：按这封失败处理。
                // 游标照旧推进，其它邮件与「N 封新邮件」聚合提醒都不受影响。
                if !matches!(status, Status::Ok) {
                    return None;
                }
                // 正文为空也要继续：有的通知邮件把验证码只写在主题里。
                let mut text = text.unwrap_or_default();
                text.truncate(CODE_TEXT_LIMIT);
                return extract_code_from_mail(header.as_deref().unwrap_or_default(), &text);
            }
            _ => {}
        }
    }
    None
}

/// 邮件头里投递需要的字段，其余一律不保留（正文也不落盘）。
#[derive(Default)]
struct MailHead {
    subject: String,
    sender: String,
    time: Option<i64>,
    transfer_encoding: String,
    charset: String,
}

/// 头部只取 Subject / From / Date / CTE / Content-Type，续行先折叠成一行。
fn parse_head(header: &[u8]) -> MailHead {
    let raw = String::from_utf8_lossy(header);
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in raw.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = fields.last_mut() {
                // RFC 2047 编码词跨行拆开时不能补空格，否则解出「验 证码」这类
                // 断词，后面的关键词匹配会失配。
                let continues_word = last.1.ends_with("?=") && line.trim_start().starts_with("=?");
                if !continues_word {
                    last.1.push(' ');
                }
                last.1.push_str(line.trim());
            }
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        fields.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let mut head = MailHead::default();
    for (name, value) in fields {
        match name.as_str() {
            "subject" => head.subject = decode_rfc2047(&value),
            "from" => head.sender = decode_rfc2047(&value),
            "date" => head.time = parse_mail_date(&value),
            "content-transfer-encoding" => head.transfer_encoding = value.to_ascii_lowercase(),
            "content-type" => head.charset = content_type_charset(&value),
            _ => {}
        }
    }
    head
}

fn parse_mail_date(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|date| date.timestamp_millis())
}

/// `Content-Type` 里的 `charset=`（去掉引号；大小写不敏感）。
fn content_type_charset(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let Some(index) = lower.find("charset=") else {
        return String::new();
    };
    let rest = value[index + "charset=".len()..].trim_start();
    let end = rest
        .find(|c: char| c == ';' || c.is_whitespace())
        .unwrap_or(rest.len());
    rest[..end].trim_matches(|c| c == '"' || c == '\'').to_string()
}

/// RFC 2047 编码词（`=?charset?B|Q?...?=`）尽力解码：base64 / Q 都试，
/// 解不出来就原样保留（宁可变乱码，不要吞字）。
fn decode_rfc2047(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(start) = rest.find("=?") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let mut fields = after.splitn(3, '?');
        let (Some(charset), Some(encoding), Some(tail)) =
            (fields.next(), fields.next(), fields.next())
        else {
            out.push_str("=?");
            rest = after;
            continue;
        };
        let Some((payload, remaining)) = tail.split_once("?=") else {
            out.push_str("=?");
            rest = after;
            continue;
        };
        let decoded = match encoding.to_ascii_lowercase().as_str() {
            "b" => BASE64
                .decode(payload)
                .ok()
                .map(|bytes| decode_charset(&bytes, charset)),
            "q" => Some(decode_charset(&decode_q_encoding(payload), charset)),
            _ => None,
        };
        match decoded {
            Some(value) => {
                out.push_str(&value);
                rest = remaining;
            }
            None => {
                out.push_str("=?");
                rest = after;
            }
        }
    }
    out
}

/// Q 编码：`_` 是空格，`=XX` 是字节；按字节处理，非法序列原样保留。
fn decode_q_encoding(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'_' => {
                out.push(b' ');
                index += 1;
            }
            b'=' if index + 2 < bytes.len() => {
                match (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        out.push(high << 4 | low);
                        index += 3;
                    }
                    _ => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    out
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 字节 → 文本：有 charset 声明时按声明解；声明缺失或解出较多 U+FFFD 时再用
/// GB18030 解一遍，取替换字符更少的那份（银行 / 电商 OTP 邮件常把 GBK 写成
/// utf-8 或干脆不写 Content-Type）；两份都有替换字符时才退回 UTF-8 lossy，
/// 保证 ASCII 数字不会丢。
fn decode_charset(bytes: &[u8], charset: &str) -> String {
    let label = charset.trim().trim_matches(|c| c == '"' || c == '\'');
    let primary = if label.is_empty() {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        match encoding_rs::Encoding::for_label(label.as_bytes()) {
            Some(encoding) => {
                let (text, _, _) = encoding.decode(bytes);
                text.into_owned()
            }
            None => String::from_utf8_lossy(bytes).into_owned(),
        }
    };
    let replaced = primary.matches('\u{FFFD}').count();
    if replaced == 0 {
        return primary;
    }
    // 声明的字符集基本能解开（替换字符 ≤1%）就尊重声明，不猜。
    let total = primary.chars().count().max(1);
    if !label.is_empty() && replaced * 100 <= total {
        return primary;
    }
    let (fallback, _, _) = encoding_rs::GB18030.decode(bytes);
    let fallback = fallback.into_owned();
    if fallback.matches('\u{FFFD}').count() < replaced {
        fallback
    } else {
        primary
    }
}

/// 正文 → 纯文本：先按传输编码/字符集解码，再去掉标签、实体，压缩空白。
fn body_text(body: &[u8], head: &MailHead) -> String {
    let mut bytes = decode_transfer(body, &head.transfer_encoding);
    // 顶层 CTE 没写 quoted-printable 时（多段邮件很常见：每段自己声明 CTE），
    // 用 `=XX` 特征兜底识别；base64 的 `=` 补位不会跟着十六进制字符。
    if !head.transfer_encoding.contains("base64") && looks_quoted_printable(&bytes) {
        // 逐行解：混排邮件里的 base64 段（行尾 `=` 补位）不能被 QP 软换行吃掉。
        bytes = decode_quoted_printable_lines(&bytes);
    }
    let text = decode_charset(&bytes, &head.charset);
    let text = decode_base64_lines(&text);
    compress_whitespace(&unescape_entities(&strip_html(&text)))
}

fn decode_transfer(body: &[u8], encoding: &str) -> Vec<u8> {
    if encoding.contains("base64") {
        if let Ok(bytes) = BASE64.decode(
            body.iter()
                .copied()
                .filter(|byte| !byte.is_ascii_whitespace())
                .collect::<Vec<u8>>(),
        ) {
            return bytes;
        }
    }
    if encoding.contains("quoted-printable") {
        return decode_quoted_printable(body);
    }
    body.to_vec()
}

fn looks_quoted_printable(bytes: &[u8]) -> bool {
    let mut hits = 0;
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'=' {
            if bytes[index + 1] == b'\n'
                || (bytes[index + 1] == b'\r' && bytes.get(index + 2) == Some(&b'\n'))
            {
                hits += 1;
            } else if index + 2 < bytes.len()
                && bytes[index + 1].is_ascii_hexdigit()
                && bytes[index + 2].is_ascii_hexdigit()
                && (bytes[index + 1].is_ascii_alphabetic()
                    || bytes[index + 2].is_ascii_alphabetic())
            {
                hits += 1;
            }
        }
        if hits >= 2 {
            return true;
        }
        index += 1;
    }
    false
}

fn decode_quoted_printable(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'=' {
            if bytes.get(index + 1) == Some(&b'\n') {
                index += 2;
                continue;
            }
            if bytes.get(index + 1) == Some(&b'\r') && bytes.get(index + 2) == Some(&b'\n') {
                index += 3;
                continue;
            }
            if index + 2 < bytes.len() {
                if let (Some(high), Some(low)) = (
                    hex_value(bytes[index + 1]),
                    hex_value(bytes[index + 2]),
                ) {
                    out.push(high << 4 | low);
                    index += 3;
                    continue;
                }
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

/// QP 兜底解码（多段邮件里每段自己声明 CTE 时用）：逐行处理，base64 行原样
/// 保留 —— 整段按 QP 解会把 base64 行尾的 `=` 补位当成软换行吃掉，混排邮件
/// （QP 文本段 + base64 文本段）里整块就解不出来了。非 base64 行的行尾 `=`
/// 仍是软换行：去掉 `=` 并吞掉换行。
fn decode_quoted_printable_lines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        while index < bytes.len() && bytes[index] != b'\n' {
            index += 1;
        }
        let newline = index < bytes.len();
        let line_end = index;
        if newline {
            index += 1;
        }
        let mut content_end = line_end;
        if content_end > start && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let line = &bytes[start..content_end];
        if is_base64_bytes(line) {
            out.extend_from_slice(line);
            if newline {
                out.push(b'\n');
            }
            continue;
        }
        let mut soft_break = false;
        out.extend_from_slice(&decode_qp_line(line, &mut soft_break));
        if !soft_break && newline {
            out.push(b'\n');
        }
    }
    out
}

/// QP 单行解码：`=XX` 转字节；行尾孤立的 `=` 记作软换行（由调用方决定是否
/// 吞掉换行），其余原样保留。
fn decode_qp_line(line: &[u8], soft_break: &mut bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    let mut index = 0;
    while index < line.len() {
        if line[index] == b'=' {
            if index + 2 < line.len() {
                if let (Some(high), Some(low)) =
                    (hex_value(line[index + 1]), hex_value(line[index + 2]))
                {
                    out.push(high << 4 | low);
                    index += 3;
                    continue;
                }
            }
            if index + 1 == line.len() {
                *soft_break = true;
                index += 1;
                continue;
            }
        }
        out.push(line[index]);
        index += 1;
    }
    out
}

/// 多段邮件里 base64 段通常按 76/64 字符折行：把连续满足单行条件的行攒成
/// 一块再解码 —— 逐行解码经常切断 UTF-8 多字节字符，整行被丢等于没解析。
/// 解出的块只在「像文本」时才替换原行，二进制附件原样保留。
fn decode_base64_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut block = String::new();
    for line in text.lines() {
        if is_base64_line(line.trim()) {
            block.push_str(line.trim());
            continue;
        }
        flush_base64_block(&mut block, &mut out);
        out.push_str(line);
        out.push('\n');
    }
    flush_base64_block(&mut block, &mut out);
    out
}

fn flush_base64_block(block: &mut String, out: &mut String) {
    if block.is_empty() {
        return;
    }
    match decode_base64_block(block) {
        Some(decoded) => {
            out.push_str(&decoded);
            out.push('\n');
        }
        None => {
            out.push_str(block);
            out.push('\n');
        }
    }
    block.clear();
}

fn is_base64_line(line: &str) -> bool {
    is_base64_bytes(line.as_bytes())
}

fn is_base64_bytes(line: &[u8]) -> bool {
    let mut start = 0;
    let mut end = line.len();
    while start < end && line[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && line[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let len = end - start;
    len >= 32
        && len % 4 == 0
        && line[start..end]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

fn decode_base64_block(block: &str) -> Option<String> {
    let bytes = BASE64.decode(block).ok()?;
    let text = decode_charset(&bytes, "");
    let total = text.chars().count();
    if total == 0 {
        return None;
    }
    // 可打印占比挡住二进制附件；U+FFFD 也算不可打印（GB18030 兜底后仍解不出的字节）。
    let printable = text
        .chars()
        .filter(|c| (!c.is_control() && *c != '\u{FFFD}') || matches!(c, '\n' | '\r' | '\t'))
        .count();
    (printable * 10 >= total * 9).then(|| text)
}

/// 去掉 HTML 标签（用一个空格占位，避免把「验证码」和数字粘成一个词）。
/// 只有 `<` 后紧跟 `/`、`!`、`?` 或 ASCII 字母时才当标签，否则按字面量输出
/// （正文里的 `a < b`、被截断的尖括号不能吞掉后面整段文字）。
fn strip_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('<') {
        out.push_str(&rest[..index]);
        let after = &rest[index..];
        let mut chars = after.chars();
        chars.next();
        let tag_like = chars
            .next()
            .is_some_and(|c| matches!(c, '/' | '!' | '?') || c.is_ascii_alphabetic());
        match after.find('>') {
            Some(end) if tag_like && end <= 512 => {
                out.push(' ');
                rest = &after[end + 1..];
            }
            _ => {
                out.push('<');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn unescape_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let after = &rest[index..];
        let Some(end) = after.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let entity = &after[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => u32::from_str_radix(
                &entity[2..],
                16,
            )
            .ok()
            .and_then(char::from_u32),
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        match decoded {
            Some(value) => {
                out.push(value);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn compress_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
    }
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 关键词锚定抽验证码：没有关键词一律 None（宁缺勿滥）。
///
/// 规则：
/// - 中文「验证码 / 校验码 / 动态码 / 确认码 / 登录码」；
/// - 英文 `verification code` / `verify code` / `security code` / `passcode` /
///   `one-time code` / `code` / `OTP`（大小写不敏感，整词边界）；
/// - 关键词后 0–24 字符内取候选：4–8 位连续数字，或字母数字混合（必须紧邻
///   关键词 ≤4 字符）；数字中间允许一个 `-` 或空格，输出时去掉；
/// - 排除年份 19xx/20xx、超过 8 位的长串（手机号 / 金额 / 订单号）以及紧邻
///   「金额 / 元 / ¥ / 尾号 / 订单」的数字；
/// - 多个关键词命中时按出现顺序取第一个能抽到候选的（即最近的那个）。
///
/// `pub(super)`：系统通知来源的邮件（没有 IMAP）在 `notify.rs` 的 toast
/// 链路里复用同一套规则就近抽码。
pub(super) fn extract_code(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    for (_, end) in keyword_spans(&lower) {
        let limit = (end + CODE_WINDOW).min(chars.len());
        if let Some(code) = candidate_after(&chars, end, limit) {
            return Some(code);
        }
    }
    None
}

const CN_CODE_KEYWORDS: [&str; 5] = ["验证码", "校验码", "动态码", "确认码", "登录码"];
const EN_CODE_KEYWORDS: [&str; 7] = [
    "verification code",
    "verify code",
    "security code",
    "passcode",
    "one-time code",
    "code",
    "otp",
];
/// 关键词之后搜索候选的最大字符数。
const CODE_WINDOW: usize = 24;
/// 字母数字混合候选必须紧邻关键词的最大字符间隔。
const CODE_KEYWORD_GAP: usize = 4;

fn keyword_spans(lower: &[char]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for keyword in CN_CODE_KEYWORDS {
        collect_spans(lower, &keyword.chars().collect::<Vec<_>>(), false, &mut spans);
    }
    let mut english = Vec::new();
    for keyword in EN_CODE_KEYWORDS {
        let mut found = Vec::new();
        collect_spans(lower, &keyword.chars().collect::<Vec<_>>(), true, &mut found);
        // 只有裸 `code` 有歧义（promo / zip / order code 等），前面 12 个字符
        // 出现修饰词就跳过；`verification code` 这类本身明确的短语不参与过滤
        // （"order verification code" 是真实写法）。中文关键词同理：
        // 「订单号 1234，验证码 5678」不能被前文的「订单」误杀。
        if keyword == "code" {
            found.retain(|(start, _)| !ambiguous_before(lower, *start));
        }
        english.extend(found);
    }
    spans.extend(english);
    spans.sort_unstable();
    spans.dedup();
    spans
}

fn ambiguous_before(text: &[char], start: usize) -> bool {
    let before: String = text[start.saturating_sub(12)..start].iter().collect();
    const WORDS: [&str; 6] = ["promo", "coupon", "discount", "postal", "zip", "order"];
    const CN: [&str; 4] = ["优惠码", "折扣码", "邮编", "订单"];
    WORDS.iter().any(|word| before.contains(word)) || CN.iter().any(|word| before.contains(word))
}

fn collect_spans(
    text: &[char],
    needle: &[char],
    word_boundary: bool,
    out: &mut Vec<(usize, usize)>,
) {
    if needle.is_empty() || needle.len() > text.len() {
        return;
    }
    for start in 0..=text.len() - needle.len() {
        if &text[start..start + needle.len()] != needle {
            continue;
        }
        let end = start + needle.len();
        if word_boundary
            && ((start > 0 && text[start - 1].is_ascii_alphanumeric())
                || (end < text.len() && text[end].is_ascii_alphanumeric()))
        {
            continue;
        }
        out.push((start, end));
    }
}

/// 在 `[from, to)` 里找第一个可接受的候选（`from` = 关键词结束位置）。
fn candidate_after(chars: &[char], from: usize, to: usize) -> Option<String> {
    let mut index = from;
    while index < to {
        if !chars[index].is_ascii_alphanumeric() {
            index += 1;
            continue;
        }
        let gap = index - from;
        let (token, start, end, separator) = scan_token(chars, index);
        index = end;
        if token.len() < 4 || token.len() > 8 {
            continue;
        }
        // 后面还接着字母数字 / 分隔符+字母数字：说明这是更长串的一段，整段丢弃。
        if let Some(&next) = chars.get(end) {
            if next.is_ascii_alphanumeric()
                || (matches!(next, '-' | ' ')
                    && chars
                        .get(end + 1)
                        .is_some_and(|value| value.is_ascii_alphanumeric()))
            {
                continue;
            }
        }
        let letters = token.chars().filter(|c| c.is_ascii_alphabetic()).count();
        let digits = token.chars().filter(|c| c.is_ascii_digit()).count();
        if letters > 0 {
            // 字母数字混合只接受紧贴关键词的（`AB12CD`、`AB-12CD`）；带空格的
            // 一律不拼，否则 "code is 442233" 会被拼成 "IS442233"。
            if separator != Some(' ') && gap <= CODE_KEYWORD_GAP && digits > 0 {
                return Some(token.to_ascii_uppercase());
            }
            // 混合串里可能藏着真正的数字验证码（"code is 442233" 的整段
            // "is442233" 被拒后，再在段内找 4–8 位连续数字）。
            if let Some(code) = digit_runs(chars, start, end) {
                return Some(code);
            }
            continue;
        }
        if excluded_number(&token, chars, start, end) {
            continue;
        }
        return Some(token);
    }
    None
}

/// 从 `start` 起吃掉一段字母数字；中间允许一个 `-` 或空格（输出时不算进 token）。
fn scan_token(chars: &[char], start: usize) -> (String, usize, usize, Option<char>) {
    let mut token = String::new();
    let mut index = start;
    let mut separator = None;
    while index < chars.len() {
        let c = chars[index];
        if c.is_ascii_alphanumeric() {
            token.push(c);
            index += 1;
        } else if separator.is_none()
            && matches!(c, '-' | ' ')
            && index > start
            && chars
                .get(index + 1)
                .is_some_and(|value| value.is_ascii_alphanumeric())
        {
            separator = Some(c);
            index += 1;
        } else {
            break;
        }
    }
    (token, start, index, separator)
}

/// 在一段「字母数字混合」的候选里找 4–8 位连续数字（不跨字母 / 分隔符）。
fn digit_runs(chars: &[char], start: usize, end: usize) -> Option<String> {
    let mut index = start;
    while index < end {
        if !chars[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < end && chars[index].is_ascii_digit() {
            index += 1;
        }
        let run: String = chars[run_start..index].iter().collect();
        if (4..=8).contains(&run.len()) && !excluded_number(&run, chars, run_start, index) {
            return Some(run);
        }
    }
    None
}

/// 年份、金额 / 订单号附近、以及被截断的长数字都不算验证码。
fn excluded_number(token: &str, chars: &[char], start: usize, end: usize) -> bool {
    if token.len() == 4 {
        if let Ok(year) = token.parse::<u32>() {
            if (1900..=2099).contains(&year) {
                return true;
            }
        }
    }
    let before: String = chars[start.saturating_sub(12)..start].iter().collect();
    let after: String = chars[end..(end + 4).min(chars.len())].iter().collect();
    const BEFORE: [&str; 4] = ["金额", "尾号", "订单", "¥"];
    const AFTER: [&str; 2] = ["元", "¥"];
    BEFORE.iter().any(|word| before.contains(word))
        || AFTER.iter().any(|word| after.contains(word))
}

/// 邮件头 + 正文 → 验证码条目（主题、发件人都截断；正文只在内存里用一次）。
fn extract_code_from_mail(header: &[u8], body: &[u8]) -> Option<MailCode> {
    let head = parse_head(header);
    let text = body_text(body, &head);
    let code = extract_code(&head.subject).or_else(|| extract_code(&text))?;
    Some(MailCode {
        sender: truncate_chars(&head.sender, 40),
        subject: truncate_chars(&head.subject, 60),
        code,
        time: head.time,
    })
}

fn thunderbird_handles(key: &AccountKey) -> bool {
    thunderbird_running()
        && discover_thunderbird_accounts()
            .iter()
            .any(|account| matches_thunderbird_account(key, account))
}

fn matches_thunderbird_account(key: &AccountKey, account: &ImapAccount) -> bool {
    account.host.eq_ignore_ascii_case(&key.host)
        && account.port == i64::from(key.port)
        && account.user == key.user
}

#[cfg(windows)]
fn thunderbird_running() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = false;
    unsafe {
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let end = entry
                .szExeFile
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end])
                .eq_ignore_ascii_case("thunderbird.exe")
            {
                found = true;
                break;
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

#[cfg(not(windows))]
fn thunderbird_running() -> bool {
    false
}

#[cfg(windows)]
pub(in crate::island) fn discover_thunderbird_accounts() -> Vec<ImapAccount> {
    use std::fs::File;
    use std::io::Read;
    use std::path::Path;

    fn read_local(path: &Path, limit: u64) -> Option<String> {
        let mut text = String::new();
        File::open(path)
            .ok()?
            .take(limit + 1)
            .read_to_string(&mut text)
            .ok()?;
        (text.len() as u64 <= limit).then_some(text)
    }

    let Some(root) = dirs::config_dir().map(|dir| dir.join("Thunderbird")) else {
        return Vec::new();
    };
    let Some(profiles) = read_local(&root.join("profiles.ini"), 1024 * 1024) else {
        return Vec::new();
    };
    for path in thunderbird_profiles(&profiles) {
        if let Some(prefs) = read_local(&root.join(path).join("prefs.js"), 4 * 1024 * 1024) {
            let accounts = parse_thunderbird_accounts(&prefs);
            if !accounts.is_empty() {
                return accounts;
            }
        }
    }
    Vec::new()
}

#[cfg(not(windows))]
pub(in crate::island) fn discover_thunderbird_accounts() -> Vec<ImapAccount> {
    Vec::new()
}

#[cfg(any(windows, test))]
fn thunderbird_profiles(ini: &str) -> Vec<&str> {
    let mut installs = Vec::new();
    let mut defaults = Vec::new();
    let mut others = Vec::new();
    for section in ini.trim_start_matches('\u{feff}').split('[').skip(1) {
        let Some((name, values)) = section.split_once(']') else {
            continue;
        };
        let name = name.to_ascii_lowercase();
        let values: HashMap<_, _> = values
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim()))
            .collect();
        if name.starts_with("install") {
            installs.extend(
                values
                    .get("default")
                    .copied()
                    .filter(|path| !path.is_empty()),
            );
        } else if name.starts_with("profile") {
            let path = values.get("path").copied().filter(|path| !path.is_empty());
            if values.get("default").copied() == Some("1") {
                defaults.extend(path);
            } else {
                others.extend(path);
            }
        }
    }
    let mut seen = HashSet::new();
    installs
        .into_iter()
        .chain(defaults)
        .chain(others)
        .filter(|path| seen.insert(*path))
        .collect()
}

#[cfg(any(windows, test))]
fn parse_thunderbird_accounts(prefs: &str) -> Vec<ImapAccount> {
    use std::collections::BTreeMap;
    let mut servers: BTreeMap<&str, HashMap<&str, &str>> = BTreeMap::new();
    for line in prefs.lines() {
        let Some((key, value)) = line
            .trim()
            .strip_prefix(r#"user_pref("mail.server."#)
            .and_then(|pref| pref.strip_suffix(");"))
            .and_then(|pref| pref.split_once(r#"","#))
        else {
            continue;
        };
        let Some((server, field)) = key.split_once('.') else {
            continue;
        };
        if matches!(field, "type" | "hostname" | "userName" | "port") {
            servers
                .entry(server)
                .or_default()
                .insert(field, value.trim());
        }
    }
    let mut seen = HashSet::new();
    servers
        .values()
        .filter_map(|fields| {
            let kind: String = serde_json::from_str(fields.get("type")?).ok()?;
            if kind != "imap" {
                return None;
            }
            let host: String = serde_json::from_str(fields.get("hostname")?).ok()?;
            let user: String = serde_json::from_str(fields.get("userName")?).ok()?;
            let host = host.trim().to_ascii_lowercase();
            let user = user.trim().to_owned();
            let port = fields
                .get("port")
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(993);
            if host.is_empty()
                || user.is_empty()
                || !seen.insert((host.clone(), port, user.clone()))
            {
                return None;
            }
            Some(ImapAccount {
                host,
                port: i64::from(port),
                user,
                ..ImapAccount::default()
            })
        })
        .take(16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor as Input, Read};
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    #[derive(Debug)]
    struct ImapTranscript<'a> {
        replies: Input<&'a [u8]>,
        commands: &'a mut Vec<u8>,
    }

    impl AsyncRead for ImapTranscript<'_> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let read = self.replies.read(buf.initialize_unfilled())?;
            buf.advance(read);
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for ImapTranscript<'_> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.commands.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn transcript(
        host: &str,
        cursor: Option<Cursor>,
        code_preview: bool,
        actions: &str,
    ) -> (Result<InboxUpdate, &'static str>, String) {
        let replies = format!("* OK IMAP ready\r\nA0001 OK Logged in\r\n{actions}");
        let mut commands = Vec::new();
        let account = Account {
            key: AccountKey {
                host: host.into(),
                port: 993,
                user: "sample@example.invalid".into(),
            },
            password: "app-token".into(),
        };
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(unread_updates(
                ImapTranscript {
                    replies: Input::new(replies.as_bytes()),
                    commands: &mut commands,
                },
                &account,
                cursor,
                code_preview,
            ));
        (result, String::from_utf8(commands).unwrap())
    }

    /// 构造一段带字面量的 `UID FETCH` 回复（格式与真实服务器一致）。
    fn fetch_reply(tag: u32, uid: u32, header: &str, body: &str) -> String {
        format!(
            "* 1 FETCH (UID {uid} BODY[HEADER.FIELDS (SUBJECT FROM DATE)] {{{}}}\r\n{} BODY[TEXT]<0> {{{}}}\r\n{})\r\nA{tag:04} OK Fetch completed\r\n",
            header.len(),
            header,
            body.len(),
            body
        )
    }

    #[test]
    fn code_preview_fetch_failure_keeps_cursor_and_returns_no_codes() {
        let (update, commands) = transcript(
            "imap.example.invalid",
            Some(Cursor {
                validity: 42,
                next_uid: 57,
            }),
            true,
            "* OK [UIDVALIDITY 42] Valid\r\n* OK [UIDNEXT 60] Next\r\nA0002 OK Examined\r\n\
             * SEARCH 57\r\nA0003 OK Searched\r\nA0004 NO partial fetch unsupported\r\n\
             * BYE Logging out\r\nA0005 OK Goodbye\r\n",
        );
        let update = update.expect("搜索成功就要返回更新");
        assert_eq!(update.new_unread, 1);
        assert!(update.codes.is_empty(), "FETCH 失败只影响内容，不影响数量");
        assert!(commands.contains(
            "UID FETCH 57 (BODY.PEEK[HEADER.FIELDS (SUBJECT FROM DATE)] BODY.PEEK[TEXT]<0.65536>)"
        ));
        assert!(!commands.contains("STORE"));
    }

    /// 开关打开：只读（PEEK）取最新 UID 的正文，抽到验证码就带上发件人 / 主题 / 时间。
    #[test]
    fn code_preview_reads_newest_mail_with_peek_only() {
        let newest_header = "Subject: 登录验证码\r\nFrom: AgentHUD <no-reply@example.invalid>\r\nDate: Tue, 12 Nov 2024 10:00:00 +0800\r\n\r\n";
        let newest_body = "您的验证码是 123456，5 分钟内有效。";
        let older_header = "Subject: 发货通知\r\nFrom: Shop <shop@example.invalid>\r\n\r\n";
        let older_body = "您的订单 99887766 已发货，请留意查收。";
        let actions = format!(
            "* OK [UIDVALIDITY 42] Valid\r\n* OK [UIDNEXT 60] Next\r\nA0002 OK Examined\r\n\
             * SEARCH 57 58\r\nA0003 OK Searched\r\n{}{}* BYE Logging out\r\nA0006 OK Goodbye\r\n",
            fetch_reply(4, 58, newest_header, newest_body),
            fetch_reply(5, 57, older_header, older_body)
        );
        let (update, commands) = transcript(
            "imap.example.invalid",
            Some(Cursor {
                validity: 42,
                next_uid: 57,
            }),
            true,
            &actions,
        );
        let update = update.expect("读取成功");
        assert_eq!(update.new_unread, 2);
        assert_eq!(update.codes.len(), 1, "只有一封带验证码");
        let mail = &update.codes[0];
        assert_eq!(mail.code, "123456");
        assert_eq!(mail.subject, "登录验证码");
        assert_eq!(mail.sender, "AgentHUD <no-reply@example.invalid>");
        assert!(mail.time.is_some(), "Date 头可解析时用邮件时间");
        // 大 UID（最新）先读；两封都用 PEEK，且没有任何改状态的命令。
        let fetch58 = commands.find("UID FETCH 58 ").expect("先读最新的 58");
        let fetch57 = commands.find("UID FETCH 57 ").expect("再读 57");
        assert!(fetch58 < fetch57);
        assert!(commands
            .contains("BODY.PEEK[HEADER.FIELDS (SUBJECT FROM DATE)] BODY.PEEK[TEXT]<0.65536>"));
        assert!(!commands.contains("STORE"));
        assert!(!commands.contains(" FLAGS"));
    }

    #[test]
    fn extracts_chinese_six_digit_code() {
        assert_eq!(
            extract_code("您的验证码为 123456，5 分钟内有效").as_deref(),
            Some("123456")
        );
        assert_eq!(extract_code("校验码：654321").as_deref(), Some("654321"));
    }

    #[test]
    fn extracts_english_verification_code() {
        assert_eq!(
            extract_code("Your verification code: 987654").as_deref(),
            Some("987654")
        );
        assert_eq!(
            extract_code("One-time code is 442233").as_deref(),
            Some("442233")
        );
        assert_eq!(extract_code("OTP 778899").as_deref(), Some("778899"));
    }

    #[test]
    fn joins_digits_split_by_space_or_hyphen() {
        assert_eq!(extract_code("验证码 123 456").as_deref(), Some("123456"));
        assert_eq!(extract_code("验证码：123-456").as_deref(), Some("123456"));
        // 两段以上不能拼，避免把长串截成验证码
        assert_eq!(extract_code("验证码 123 456 789"), None);
    }

    #[test]
    fn extracts_code_behind_html_tag() {
        assert_eq!(
            extract_code("验证码<b style=\"color:red\">246810</b>").as_deref(),
            Some("246810")
        );
    }

    #[test]
    fn requires_keyword_anchor() {
        assert_eq!(extract_code("订单号 123456 已发货"), None);
        assert_eq!(extract_code("您的 6 位数字是 123456"), None);
        assert_eq!(extract_code(""), None);
        assert_eq!(extract_code("   \n  "), None);
    }

    #[test]
    fn rejects_years_and_long_numbers() {
        assert_eq!(extract_code("验证码有效期至 2024 年"), None);
        assert_eq!(extract_code("验证码 13812345678"), None);
        assert_eq!(extract_code("验证码 1234567890"), None);
    }

    #[test]
    fn rejects_numbers_next_to_amount_words() {
        assert_eq!(extract_code("验证码 金额 1234 元"), None);
        assert_eq!(extract_code("验证码 尾号 1234"), None);
        assert_eq!(extract_code("验证码 1234 元"), None);
        assert_eq!(extract_code("验证码 订单 1234"), None);
    }

    #[test]
    fn extracts_alphanumeric_code() {
        assert_eq!(extract_code("验证码 AB12CD").as_deref(), Some("AB12CD"));
        assert_eq!(extract_code("验证码：ab-12cd").as_deref(), Some("AB12CD"));
    }

    #[test]
    fn takes_nearest_candidate_after_keyword() {
        assert_eq!(
            extract_code("验证码 111111 验证码 222222").as_deref(),
            Some("111111")
        );
    }

    #[test]
    fn decodes_quoted_printable_keyword() {
        // =E9=AA=8C=E8=AF=81=E7=A0=81 即「验证码」
        let decoded = String::from_utf8(decode_quoted_printable(
            b"=E9=AA=8C=E8=AF=81=E7=A0=81=EF=BC=9A123456",
        ))
        .expect("QP 解出 UTF-8");
        assert_eq!(decoded, "验证码：123456");
        assert_eq!(extract_code(&decoded).as_deref(), Some("123456"));
    }

    #[test]
    fn decodes_rfc2047_base64_subject() {
        assert_eq!(decode_rfc2047("=?UTF-8?B?6aqM6K+B56CB?="), "验证码");
        assert_eq!(decode_rfc2047("Re: =?utf-8?b?6aqM6K+B56CB?="), "Re: 验证码");
        // 解不出来就原样保留，不能吞字
        assert_eq!(decode_rfc2047("=?bogus?B?%%%?="), "=?bogus?B?%%%?=");
    }

    #[test]
    fn extracts_code_from_full_mail_with_quoted_printable_body() {
        let header = b"Subject: =?UTF-8?B?6aqM6K+B56CB?=\r\nFrom: =?UTF-8?B?5L2g5aW9?= <no-reply@example.invalid>\r\n\r\n";
        let body = b"Content-Transfer-Encoding: quoted-printable\r\n\r\n\
=E6=82=A8=E7=9A=84=E9=AA=8C=E8=AF=81=E7=A0=81=E6=98=AF=EF=BC=9A123456";
        let mail = extract_code_from_mail(header, body).expect("抽到验证码");
        assert_eq!(mail.code, "123456");
        assert_eq!(mail.subject, "验证码");
        assert_eq!(mail.sender, "你好 <no-reply@example.invalid>");
        assert!(mail.time.is_none());
    }

    #[test]
    fn ignores_body_without_keyword() {
        let header = b"Subject: \r\nFrom: a@example.invalid\r\n\r\n";
        assert_eq!(extract_code_from_mail(header, b"123456"), None);
    }

    /// 顶层声明 gb2312 的 QP 正文（银行 / 电商常见形态）：按声明解码后能抽到码。
    #[test]
    fn extracts_gbk_quoted_printable_body_with_declared_charset() {
        let header = b"Subject: =?UTF-8?B?6aqM6K+B56CB?=\r\nContent-Type: text/plain; charset=gb2312\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n";
        let body = b"=C4=FA=B5=C4=D1=E9=D6=A4=C2=EB=CA=C7=A3=BA123456";
        assert_eq!(
            extract_code_from_mail(header, body).map(|mail| mail.code).as_deref(),
            Some("123456")
        );
    }

    /// 没写 charset、正文是 GBK：GB18030 兜底解码后仍能抽到码。
    #[test]
    fn extracts_gbk_body_without_charset_declaration() {
        let header =
            b"Subject: x\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n";
        let body = b"=C4=FA=B5=C4=D1=E9=D6=A4=C2=EB=CA=C7=A3=BA654321";
        assert_eq!(
            extract_code_from_mail(header, body).map(|mail| mail.code).as_deref(),
            Some("654321")
        );
    }

    /// 多段邮件里按 76/64 字符折行的 base64 段：整块解码（逐行解码会切断 UTF-8）。
    fn folded_base64_mail(width: usize) -> Vec<u8> {
        let plain = "尊敬的用户，您的登录验证码是 846215，请在 10 分钟内使用。退订回 T";
        let encoded = BASE64.encode(plain.as_bytes());
        let mut body = String::from(
            "--BOUNDARY\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n",
        );
        for chunk in encoded.as_bytes().chunks(width) {
            body.push_str(std::str::from_utf8(chunk).expect("base64 是 ASCII"));
            body.push_str("\r\n");
        }
        body.push_str("--BOUNDARY--\r\n");
        body.into_bytes()
    }

    #[test]
    fn extracts_code_from_76_char_folded_base64_block() {
        let header =
            b"Subject: \xE7\x99\xBB\xE5\xBD\x95\xE9\x80\x9A\xE7\x9F\xA5\r\nContent-Type: multipart/mixed; boundary=\"BOUNDARY\"\r\n\r\n";
        let body = folded_base64_mail(76);
        assert_eq!(
            extract_code_from_mail(header, &body).map(|mail| mail.code).as_deref(),
            Some("846215")
        );
    }

    #[test]
    fn extracts_code_from_64_char_folded_base64_block() {
        let header =
            b"Subject: \xE7\x99\xBB\xE5\xBD\x95\xE9\x80\x9A\xE7\x9F\xA5\r\nContent-Type: multipart/mixed; boundary=\"BOUNDARY\"\r\n\r\n";
        let body = folded_base64_mail(64);
        assert_eq!(
            extract_code_from_mail(header, &body).map(|mail| mail.code).as_deref(),
            Some("846215")
        );
    }

    /// 同一封邮件里 QP 文本段 + base64 文本段：QP 兜底解码不能吃掉 base64 补位。
    #[test]
    fn decodes_base64_part_next_to_quoted_printable_part() {
        let filler = "尊敬的用户，这是一段较长的通知正文，用于检验分段编码。";
        let plain = format!("{filler}{filler}本次校验码是 731942，请勿泄露。");
        let encoded = BASE64.encode(plain.as_bytes());
        let mut folded = String::new();
        for chunk in encoded.as_bytes().chunks(76) {
            folded.push_str(std::str::from_utf8(chunk).expect("base64 是 ASCII"));
            folded.push_str("\r\n");
        }
        let body = format!(
            "--B\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nHello=20World=21=20plain=20text\r\n--B\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{folded}--B--\r\n"
        );
        let header = b"Subject: notice\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\n";
        assert_eq!(
            extract_code_from_mail(header, body.as_bytes())
                .map(|mail| mail.code)
                .as_deref(),
            Some("731942")
        );
    }

    /// 二进制附件（图片）的 base64 块不能被当成正文抽码，也不能 panic。
    #[test]
    fn ignores_binary_base64_attachment() {
        let mut binary = vec![0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        binary.extend(0u8..=255);
        let encoded = BASE64.encode(&binary);
        let body = format!(
            "--BOUNDARY\r\nContent-Type: image/png\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--BOUNDARY--\r\n",
            encoded
        );
        let header = b"Subject: photo\r\nContent-Type: multipart/mixed; boundary=\"BOUNDARY\"\r\n\r\n";
        assert_eq!(extract_code_from_mail(header, body.as_bytes()), None);
    }

    /// 正文为空时仍要看主题：有的通知邮件把验证码只写在主题里。
    #[test]
    fn extracts_code_from_subject_when_body_is_empty() {
        let header = b"Subject: \xE9\xAA\x8C\xE8\xAF\x81\xE7\xA0\x81 135790\r\nFrom: bank@example.invalid\r\n\r\n";
        let mail = extract_code_from_mail(header, b"").expect("主题里的验证码");
        assert_eq!(mail.code, "135790");
        assert_eq!(mail.subject, "验证码 135790");
    }

    /// RFC 2047 编码词跨行折开：拼接时不能插空格，否则关键词被拆成「验 证码」。
    #[test]
    fn joins_folded_rfc2047_words_without_space() {
        let header = b"Subject: =?UTF-8?B?5oKo55qE6aqM6K+B56CBIA==?=\r\n =?UTF-8?B?MTM1Nzkw?=\r\n\r\n";
        let head = parse_head(header);
        assert_eq!(head.subject, "您的验证码 135790");
        assert_eq!(extract_code(&head.subject).as_deref(), Some("135790"));
    }

    /// promo code / zip code 不是验证码；中文关键词与明确短语不参与这条过滤。
    #[test]
    fn ignores_ambiguous_code_words() {
        assert_eq!(extract_code("Your promo code SAVE20 is ready"), None);
        assert_eq!(extract_code("postal code 123456"), None);
        assert_eq!(extract_code("让用户填写 zip code 123456"), None);
        // 「订单号 1234，验证码 5678」是常见写法，前缀里的「订单」不能把真验证码挤掉。
        assert_eq!(
            extract_code("订单号 998877，验证码 654321").as_deref(),
            Some("654321")
        );
        // "order verification code" 是真实写法，不能被 order 误杀。
        assert_eq!(
            extract_code("Your order verification code is 246810").as_deref(),
            Some("246810")
        );
    }

    /// `a < b` 这类非标签文本不能被 strip_html 吞掉；真标签仍要折叠成空格。
    #[test]
    fn keeps_angle_brackets_that_are_not_tags() {
        // 后面出现过 `>`，旧实现会把 `<` 到 `>` 之间整段删掉（连带验证码）。
        let text = body_text("price a < b, 验证码 778899 > 1".as_bytes(), &MailHead::default());
        assert!(text.contains("a < b"), "非标签尖括号要保留：{text}");
        assert_eq!(extract_code(&text).as_deref(), Some("778899"));
        let html = body_text("<p>验证码</p><b>246810</b>".as_bytes(), &MailHead::default());
        assert_eq!(extract_code(&html).as_deref(), Some("246810"));
    }

    #[test]
    fn parses_date_header() {
        let head = parse_head(b"Subject: x\r\nDate: Tue, 12 Nov 2024 10:00:00 +0800\r\n\r\n");
        assert!(
            head.time.is_some_and(|time| time > 1_700_000_000_000),
            "RFC 2822 日期要能解析成毫秒时间戳"
        );
    }

    #[test]
    fn establishes_read_only_baseline_without_fetching_messages() {
        let (update, commands) = transcript(
            "imap.example.invalid",
            None,
            false,
            "* 41 EXISTS\r\n* OK [UIDVALIDITY 42] Valid\r\n* OK [UIDNEXT 57] Next\r\n\
             A0002 OK [READ-ONLY] Examined\r\n* BYE Logging out\r\nA0003 OK Goodbye\r\n",
        );
        assert_eq!(
            update,
            Ok(InboxUpdate {
                cursor: Cursor {
                    validity: 42,
                    next_uid: 57
                },
                new_unread: 0,
                codes: Vec::new()
            })
        );
        assert_eq!(
            commands,
            "A0001 LOGIN \"sample@example.invalid\" \"app-token\"\r\n\
             A0002 EXAMINE \"INBOX\"\r\nA0003 LOGOUT\r\n"
        );
    }

    #[test]
    fn counts_only_new_unread_uids_and_advances_cursor_past_late_arrival() {
        let (update, commands) = transcript(
            "imap.example.invalid",
            Some(Cursor {
                validity: 42,
                next_uid: 57,
            }),
            false,
            "* OK [UIDVALIDITY 42] Valid\r\n* OK [UIDNEXT 60] Next\r\nA0002 OK Examined\r\n\
             * SEARCH 40 57 61 61\r\nA0003 OK Searched\r\n* BYE Logging out\r\nA0004 OK Goodbye\r\n",
        );
        assert_eq!(
            update,
            Ok(InboxUpdate {
                cursor: Cursor {
                    validity: 42,
                    next_uid: 62
                },
                new_unread: 2,
                codes: Vec::new()
            })
        );
        assert!(commands.contains("A0003 UID SEARCH UID 57:4294967295 UNSEEN\r\n"));
        // 开关关闭：就算有新未读也不发 FETCH，命令序列与旧版一致。
        assert!(!commands.contains("FETCH"), "关闭提取验证码时不得发 FETCH");
        assert!(!commands.contains("STORE"));
    }

    #[test]
    fn handles_mailbox_uid_reset_and_netease_id() {
        let (update, commands) = transcript(
            "imap.163.com",
            Some(Cursor {
                validity: 41,
                next_uid: 57,
            }),
            false,
            "* ID (\"name\" \"Mail Server\")\r\nA0002 OK ID\r\n\
             * OK [UIDVALIDITY 42] Valid\r\nA0003 OK Examined\r\n\
             * SEARCH 51 55\r\nA0004 OK Searched\r\n* BYE Logging out\r\nA0005 OK Goodbye\r\n",
        );
        assert_eq!(
            update,
            Ok(InboxUpdate {
                cursor: Cursor {
                    validity: 42,
                    next_uid: 56
                },
                new_unread: 0,
                codes: Vec::new()
            })
        );
        assert!(commands
            .lines()
            .any(|line| line.starts_with("A0002 ID ") && line.contains("\"AgentHUD\"")));
        assert!(commands.contains("A0004 UID SEARCH ALL\r\n"));
        assert!(!commands.contains("UNSEEN"));
    }

    #[test]
    fn continues_when_netease_server_rejects_optional_id_command() {
        let (update, commands) = transcript(
            "imap.163.com",
            None,
            false,
            "A0002 BAD Unsupported command\r\n\
             * OK [UIDVALIDITY 42] Valid\r\n* OK [UIDNEXT 57] Next\r\n\
             A0003 OK Examined\r\n* BYE Logging out\r\nA0004 OK Goodbye\r\n",
        );
        assert_eq!(update.unwrap().cursor.next_uid, 57);
        assert!(commands.contains("A0003 EXAMINE \"INBOX\"\r\n"));
    }

    #[test]
    fn failed_search_does_not_return_a_new_cursor() {
        let (result, _) = transcript(
            "imap.example.invalid",
            Some(Cursor {
                validity: 42,
                next_uid: 57,
            }),
            false,
            "* OK [UIDVALIDITY 42] Valid\r\n* OK [UIDNEXT 70] Next\r\nA0002 OK Examined\r\n\
             A0003 NO temporarily unavailable\r\n",
        );
        assert_eq!(result, Err("无法查询 IMAP 收件箱未读邮件"));
    }

    #[test]
    fn uses_only_enabled_unique_accounts_with_a_password() {
        let account = ImapAccount {
            host: "IMAP.Example.Invalid".into(),
            user: "me@example.invalid".into(),
            password: "token".into(),
            ..ImapAccount::default()
        };
        let active = accounts(vec![
            ImapAccount {
                enabled: false,
                ..account.clone()
            },
            ImapAccount {
                password: String::new(),
                ..account.clone()
            },
            account.clone(),
            ImapAccount {
                host: "imap.example.invalid".into(),
                ..account.clone()
            },
            ImapAccount {
                host: "imap.other.invalid".into(),
                ..account
            },
        ]);
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].key.host, "imap.example.invalid");
        assert_eq!(active[1].key.host, "imap.other.invalid");
    }

    #[test]
    fn imports_profile_metadata_without_credentials() {
        assert_eq!(
            thunderbird_profiles(
                "[Profile0]\nPath=Profiles/other\n\
                 [Profile1]\nPath=Profiles/default\nDefault=1\n\
                 [InstallAA]\nDefault=Profiles/installed\n"
            ),
            vec!["Profiles/installed", "Profiles/default", "Profiles/other"]
        );
        let accounts = parse_thunderbird_accounts(
            r#"
            user_pref("mail.server.server1.type", "imap");
            user_pref("mail.server.server1.hostname", "IMAP.Example.Invalid");
            user_pref("mail.server.server1.userName", "name\u0040example.invalid");
            user_pref("mail.server.server1.port", 1993);
            user_pref("mail.server.server1.password", "never nhập");
            user_pref("mail.server.server2.type", "pop3");
            user_pref("mail.server.server2.hostname", "pop.example.invalid");
            user_pref("mail.server.server2.userName", "name");
            "#,
        );
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].host, "imap.example.invalid");
        assert_eq!(accounts[0].user, "name@example.invalid");
        assert_eq!(accounts[0].port, 1993);
        assert!(accounts[0].password.is_empty());

        let mut key = AccountKey {
            host: "IMAP.EXAMPLE.INVALID".into(),
            port: 1993,
            user: accounts[0].user.clone(),
        };
        assert!(matches_thunderbird_account(&key, &accounts[0]));
        key.port = 993;
        assert!(!matches_thunderbird_account(&key, &accounts[0]));
        key.port = 1993;
        key.user = "another@example.invalid".into();
        assert!(!matches_thunderbird_account(&key, &accounts[0]));
    }
}
