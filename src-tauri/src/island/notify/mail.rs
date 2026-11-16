//! Poll configured IMAP inboxes over implicit TLS without fetching message content.

use super::{emit, push_item, running, NotifyItem, NotifyPrefs, SYNTH_ID};
use crate::island::settings::{self, ImapAccount};
use async_imap::error::Error as ImapError;
use async_imap::imap_proto::{MailboxDatum, Response, Status};
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

fn configuration() -> (Duration, Vec<Account>) {
    if !NotifyPrefs::from_config().module_enabled(MAIL) {
        return (Duration::ZERO, Vec::new());
    }
    let settings = settings::load().mail;
    (
        Duration::from_secs(settings.interval_s.clamp(30, 3600) as u64),
        accounts(settings.imap),
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
        let (interval, configured) = configuration();
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
            let update = match runtime.block_on(check_inbox(
                account,
                cursors.get(&account.key).copied(),
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
            if update.new_unread > 0 && !thunderbird_handles(&account.key) {
                let prefs = NotifyPrefs::from_config();
                let item = NotifyItem {
                    id: SYNTH_ID.fetch_sub(1, Ordering::SeqCst),
                    kind: MAIL.into(),
                    name: "邮件".into(),
                    sender: account.key.user.chars().take(80).collect(),
                    body: format!("{} 封新邮件", update.new_unread),
                    time: chrono::Utc::now().timestamp_millis(),
                    read: false,
                    exe: String::new(),
                };
                if running(gen) && push_item(item, &prefs) {
                    emit(app);
                }
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

#[derive(Debug, PartialEq, Eq)]
struct InboxUpdate {
    cursor: Cursor,
    new_unread: usize,
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
) -> Result<InboxUpdate, &'static str> {
    timeout(INBOX_TIMEOUT, async {
        let stream = connect_tls(&account.key).await?;
        unread_updates(stream, account, cursor).await
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
            }
        }
    }
    let _ = timeout(Duration::from_secs(2), session.logout()).await;
    Ok(InboxUpdate {
        cursor: Cursor { validity, next_uid },
        new_unread,
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
            ));
        (result, String::from_utf8(commands).unwrap())
    }

    #[test]
    fn establishes_read_only_baseline_without_fetching_messages() {
        let (update, commands) = transcript(
            "imap.example.invalid",
            None,
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
                new_unread: 0
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
                new_unread: 2
            })
        );
        assert!(commands.contains("A0003 UID SEARCH UID 57:4294967295 UNSEEN\r\n"));
    }

    #[test]
    fn handles_mailbox_uid_reset_and_netease_id() {
        let (update, commands) = transcript(
            "imap.163.com",
            Some(Cursor {
                validity: 41,
                next_uid: 57,
            }),
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
                new_unread: 0
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
