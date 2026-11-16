//! Local Mihomo controller transport. Both TCP and Windows pipes carry HTTP/1.1.

use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::os::windows::io::AsRawHandle;
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_NOT_CONNECTED, HANDLE,
};
use windows::Win32::System::Pipes::{PeekNamedPipe, SetNamedPipeHandleState, PIPE_NOWAIT};

const PIPE_PREFIX: &str = r"\\.\pipe\";
const DEFAULT_PIPE: &str = r"\\.\pipe\verge-mihomo";
const IO_TIMEOUT: Duration = Duration::from_millis(2200);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(350);
const PIPE_RETRY: Duration = Duration::from_millis(8);
const MAX_HEADERS: usize = 16 * 1024;
const MAX_JSON: usize = 8 * 1024 * 1024;
const MAX_TRAFFIC: usize = 4096;

#[derive(Clone, Hash)]
pub(super) struct Controller {
    pipe: Option<String>,
    http: Option<SocketAddr>,
    secret: Option<String>,
}

impl Controller {
    pub(super) fn new(
        http: Option<&str>,
        pipe: Option<&str>,
        secret: Option<&str>,
    ) -> Option<Self> {
        let http = http.map(str::trim).filter(|value| !value.is_empty());
        let pipe = pipe.map(str::trim).filter(|value| !value.is_empty());
        let candidate_pipe = pipe.or_else(|| http.is_none().then_some(DEFAULT_PIPE));
        let pipe = candidate_pipe
            .filter(|name| is_local_pipe(name))
            .map(str::to_owned);
        let http = http.and_then(loopback_controller);
        let secret = secret
            .filter(|key| !key.is_empty() && key.bytes().all(|byte| (b' '..=b'~').contains(&byte)))
            .map(str::to_owned);
        if pipe.is_none() && http.is_none() {
            return None;
        }
        Some(Self { pipe, http, secret })
    }

    pub(super) fn identity(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

fn is_local_pipe(path: &str) -> bool {
    path.get(..PIPE_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PIPE_PREFIX))
        && path[PIPE_PREFIX.len()..].split('\\').all(|component| {
            !matches!(component, "" | "." | "..")
                && !component.contains('/')
                && component.chars().all(|ch| !ch.is_control())
        })
}

fn discover_verge_pipe() -> Option<String> {
    let mut pipes = fs::read_dir(PIPE_PREFIX).ok()?.filter_map(|entry| {
        let name = entry.ok()?.file_name().into_string().ok()?;
        let lower = name.to_ascii_lowercase();
        let path = format!("{PIPE_PREFIX}{name}");
        ((lower.starts_with("verge-mihomo-") || lower.starts_with("verge-mihomo_"))
            && is_local_pipe(&path))
        .then_some(path)
    });
    let pipe = pipes.next()?;
    // With multiple running cores, only use an explicit name or the unsuffixed default.
    pipes.next().is_none().then_some(pipe)
}

fn loopback_controller(address: &str) -> Option<SocketAddr> {
    let address = address.strip_prefix("http://").unwrap_or(address);
    let (host, port) = address.rsplit_once(':')?;
    let port = port.parse::<u16>().ok().filter(|port| *port > 0)?;
    let host = if host.eq_ignore_ascii_case("localhost") || host.is_empty() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .ok()?
    };
    let host = if host.is_unspecified() {
        match host {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        }
    } else if host.is_loopback() {
        host
    } else {
        return None;
    };
    Some(SocketAddr::new(host, port))
}

enum Connection {
    Pipe(File),
    Tcp(TcpStream),
}

impl Connection {
    fn open_pipe(name: &str) -> io::Result<Self> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let file = loop {
            match OpenOptions::new().read(true).write(true).open(name) {
                Ok(file) => break file,
                Err(error) if win_error(&error, ERROR_PIPE_BUSY.0) && Instant::now() < deadline => {
                    thread::sleep(PIPE_RETRY);
                }
                Err(error) => return Err(error),
            }
        };
        // This handle belongs to this one request; no other pipe consumer is changed.
        unsafe {
            SetNamedPipeHandleState(HANDLE(file.as_raw_handle()), Some(&PIPE_NOWAIT), None, None)
        }
        .map_err(io::Error::other)?;
        Ok(Self::Pipe(file))
    }
}

struct TimedConnection {
    transport: Connection,
    deadline: Instant,
}

fn win_error(error: &io::Error, code: u32) -> bool {
    error.raw_os_error() == Some(code as i32)
}

impl TimedConnection {
    fn remaining(&self) -> io::Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(remaining)
    }
}

impl Read for TimedConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let remaining = self.remaining()?;
            match &mut self.transport {
                Connection::Tcp(stream) => {
                    stream.set_read_timeout(Some(remaining))?;
                    return stream.read(buf);
                }
                Connection::Pipe(file) => {
                    // A nonblocking File read can report an empty pipe as EOF. Peek first
                    // to distinguish a response still being produced from a closed pipe.
                    let mut available = 0;
                    let peek = unsafe {
                        PeekNamedPipe(
                            HANDLE(file.as_raw_handle()),
                            None,
                            0,
                            None,
                            Some(&mut available),
                            None,
                        )
                    };
                    if let Err(error) = peek {
                        let code = error.code();
                        if code == windows::core::HRESULT::from_win32(ERROR_BROKEN_PIPE.0)
                            || code
                                == windows::core::HRESULT::from_win32(ERROR_PIPE_NOT_CONNECTED.0)
                        {
                            return Ok(0);
                        }
                        if code != windows::core::HRESULT::from_win32(ERROR_NO_DATA.0) {
                            return Err(io::Error::other(error));
                        }
                    }
                    if available > 0 {
                        let bytes = buf.len().min(available as usize);
                        return file.read(&mut buf[..bytes]);
                    }
                    thread::sleep(PIPE_RETRY.min(remaining));
                }
            }
        }
    }
}

impl Write for TimedConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let remaining = self.remaining()?;
            match &mut self.transport {
                Connection::Tcp(stream) => {
                    stream.set_write_timeout(Some(remaining))?;
                    return stream.write(buf);
                }
                Connection::Pipe(file) => match file.write(buf) {
                    Ok(0) if !buf.is_empty() => thread::sleep(PIPE_RETRY.min(remaining)),
                    result => return result,
                },
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        // There is no user-space write buffer, and FlushFileBuffers on a pipe may block.
        Ok(())
    }
}

pub(super) fn request(
    endpoint: &Controller,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    match (method, path, body) {
        ("GET", "/configs" | "/traffic" | "/connections", None) => {}
        ("PATCH", "/configs", Some(config)) if is_supported_patch(config) => {}
        _ => return Err("无效的 Mihomo 控制请求".into()),
    }
    // A fallback is safe only before a request has been sent, especially for PATCH.
    let pipe = endpoint.pipe.as_deref().and_then(|name| {
        Connection::open_pipe(name).ok().or_else(|| {
            if !name.eq_ignore_ascii_case(DEFAULT_PIPE) {
                if let Ok(pipe) = Connection::open_pipe(DEFAULT_PIPE) {
                    return Some(pipe);
                }
            }
            let discovered = discover_verge_pipe()?;
            (!discovered.eq_ignore_ascii_case(name))
                .then(|| Connection::open_pipe(&discovered).ok())
                .flatten()
        })
    });
    let (transport, authority) = if let Some(pipe) = pipe {
        (pipe, None)
    } else if let Some(address) = endpoint.http {
        let tcp = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
            .map_err(|_| "无法连接本机 Mihomo 控制接口".to_string())?;
        (Connection::Tcp(tcp), Some(address))
    } else {
        return Err("无法连接本机 Mihomo 命名管道".into());
    };

    let host = authority.map_or_else(|| "localhost".to_owned(), |host| host.to_string());
    let mut data = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept: application/json\r\n");
    // Mihomo's local named pipe does not require a secret. Avoid sending it there.
    if let Some(key) = endpoint.secret.as_deref().filter(|_| authority.is_some()) {
        data.push_str("Authorization: Bearer ");
        data.push_str(key);
        data.push_str("\r\n");
    }
    let payload = body.map(Value::to_string).unwrap_or_default();
    if body.is_some() {
        data.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            payload.len()
        ));
    }
    data.push_str("\r\n");
    data.push_str(&payload);
    let mut transport = TimedConnection {
        transport,
        deadline: Instant::now() + IO_TIMEOUT,
    };
    transport
        .write_all(data.as_bytes())
        .map_err(|_| "无法发送 Mihomo 控制请求".to_string())?;
    response(BufReader::new(transport), path == "/traffic")
}

fn is_supported_patch(config: &Value) -> bool {
    if !config.as_object().is_some_and(|map| map.len() == 1) {
        return false;
    }
    if let Some(mode) = config["mode"].as_str() {
        return matches!(mode, "rule" | "global" | "direct");
    }
    config["tun"].as_object().is_some_and(|map| map.len() == 1)
        && config["tun"]["enable"].is_boolean()
}

fn read_line(reader: &mut impl BufRead, limit: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    reader.take(limit as u64 + 1).read_until(b'\n', &mut line)?;
    if line.len() > limit {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(line)
}

fn read_error(error: io::Error) -> String {
    if matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        "读取 Mihomo 控制接口超时".into()
    } else {
        "Mihomo 控制接口返回了不完整或无效的数据".into()
    }
}

fn response(mut reader: impl BufRead, first_frame: bool) -> Result<Value, String> {
    let status_line = read_line(&mut reader, MAX_HEADERS).map_err(read_error)?;
    let status = std::str::from_utf8(&status_line)
        .ok()
        .and_then(|text| {
            let mut fields = text.split_ascii_whitespace();
            fields.next()?.starts_with("HTTP/").then_some(())?;
            fields.next()?.parse::<u16>().ok()
        })
        .ok_or_else(|| "Mihomo 控制接口响应格式无效".to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!("Mihomo 控制接口返回 HTTP {status}"));
    }
    if status == 204 {
        return Ok(Value::Null);
    }

    let mut header_budget = MAX_HEADERS - status_line.len();
    let mut chunked = false;
    let mut content_length = None;
    loop {
        let line = read_line(&mut reader, header_budget).map_err(read_error)?;
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        if line.is_empty() {
            return Err("Mihomo 控制接口响应头不完整".into());
        }
        header_budget -= line.len();
        if let Some((name, value)) = std::str::from_utf8(&line)
            .ok()
            .and_then(|line| line.split_once(':'))
        {
            if name.eq_ignore_ascii_case("transfer-encoding") {
                chunked = value
                    .split(',')
                    .any(|value| value.trim().eq_ignore_ascii_case("chunked"));
            } else if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<u64>().ok();
            }
        }
    }

    let limit = if first_frame { MAX_TRAFFIC } else { MAX_JSON };
    let bytes = if chunked {
        read_chunked(&mut reader, first_frame, limit).map_err(read_error)?
    } else if first_frame {
        let mut bounded = reader.take(content_length.unwrap_or(limit as u64 + 1));
        read_line(&mut bounded, limit).map_err(read_error)?
    } else if let Some(length) = content_length {
        if length > limit as u64 {
            return Err("Mihomo 控制接口响应过大".into());
        }
        let mut bytes = vec![0; length as usize];
        reader.read_exact(&mut bytes).map_err(read_error)?;
        bytes
    } else {
        let mut bytes = Vec::new();
        reader
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(read_error)?;
        bytes
    };
    if bytes.len() > limit {
        return Err("Mihomo 控制接口响应过大".into());
    }
    let bytes = if first_frame {
        bytes
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default()
    } else {
        &bytes
    };
    serde_json::from_slice(bytes).map_err(|_| "Mihomo 控制接口没有返回有效 JSON".into())
}

fn read_chunked(reader: &mut impl BufRead, first_frame: bool, limit: usize) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let length = read_line(reader, 128)?;
        let length = std::str::from_utf8(&length)
            .ok()
            .and_then(|line| usize::from_str_radix(line.split(';').next()?.trim(), 16).ok())
            .ok_or(io::ErrorKind::InvalidData)?;
        if length == 0 {
            return Ok(body);
        }
        let start = body.len();
        if length > limit - start {
            return Err(io::ErrorKind::InvalidData.into());
        }
        body.resize(start + length, 0);
        reader.read_exact(&mut body[start..])?;
        if first_frame && body[start..].contains(&b'\n') {
            return Ok(body);
        }
        let mut ending = [0; 2];
        reader.read_exact(&mut ending)?;
        if ending != *b"\r\n" {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_connections_stay_local() {
        assert!(is_local_pipe(r"\\.\pipe\verge-mihomo"));
        assert!(!is_local_pipe(r"\\remote\pipe\verge-mihomo"));
        assert!(!is_local_pipe(r"\\.\pipe\..\remote"));
        assert_eq!(
            loopback_controller("0.0.0.0:9090"),
            "127.0.0.1:9090".parse().ok()
        );
        assert_eq!(loopback_controller("[::]:9090"), "[::1]:9090".parse().ok());
        assert_eq!(
            loopback_controller("localhost:9090"),
            "127.0.0.1:9090".parse().ok()
        );
        assert!(loopback_controller("localhost:9090@100.0.0.1").is_none());
        assert!(loopback_controller("192.168.0.1:9090").is_none());
    }

    #[test]
    fn traffic_reads_the_first_sample_without_waiting_for_end_of_stream() {
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let left = r#"{"up":12,"#;
        let right = "\"down\":34}\n";
        let stream = format!(
            "{head}{:x}\r\n{left}\r\n{:x}\r\n{right}\r\n",
            left.len(),
            right.len()
        );
        assert_eq!(
            response(stream.as_bytes(), true).unwrap(),
            serde_json::json!({"up": 12, "down": 34})
        );
    }

    #[test]
    fn parses_fixed_length_and_rejects_incomplete_or_unsuccessful_responses() {
        let valid = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(response(&valid[..], false).unwrap(), serde_json::json!({}));
        assert!(response(&valid[..valid.len() - 1], false).is_err());
        assert_eq!(
            response(&b"HTTP/1.1 204 No Content\r\n\r\n"[..], false).unwrap(),
            Value::Null
        );
        assert!(response(&b"HTTP/1.1 401 Unauthorized\r\n\r\n"[..], false).is_err());
    }

    fn assert_local_patch(patch: Value) {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = Controller::new(
            Some(&listener.local_addr().unwrap().to_string()),
            None,
            None,
        )
        .unwrap();
        let expected = patch.clone();
        let mock = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut input = BufReader::new(&mut socket);
            assert_eq!(
                read_line(&mut input, 128).unwrap(),
                b"PATCH /configs HTTP/1.1\r\n"
            );
            let mut length = 0;
            loop {
                let line = read_line(&mut input, 256).unwrap();
                assert!(!line.is_empty());
                if line == b"\r\n" {
                    break;
                }
                if let Some(value) = line.strip_prefix(b"Content-Length: ") {
                    length = std::str::from_utf8(value).unwrap().trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            input.read_exact(&mut body).unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), expected);
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
                .unwrap();
        });
        assert_eq!(
            request(&endpoint, "PATCH", "/configs", Some(&patch)).unwrap(),
            Value::Null
        );
        mock.join().unwrap();
    }

    #[test]
    fn sends_supported_config_patches_to_a_local_controller() {
        assert_local_patch(serde_json::json!({"tun": {"enable": true}}));
        for mode in ["rule", "global", "direct"] {
            assert_local_patch(serde_json::json!({"mode": mode}));
        }
        assert!(!is_supported_patch(&serde_json::json!({"mode": "script"})));
        assert!(!is_supported_patch(
            &serde_json::json!({"mode": "rule", "mixed-port": 7897})
        ));
    }
}
