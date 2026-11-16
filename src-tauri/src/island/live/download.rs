//! 实时字幕模型下载：parakeet 日语识别模型 + Silero VAD。
//!
//! 目标文件（`<models_root>` 下，见 [`super::models_root`]）：
//!
//! ```text
//! parakeet-ja-0.6b-int8/model.int8.onnx   655542604 B
//! parakeet-ja-0.6b-int8/tokens.txt            28557 B
//! silero_vad.onnx                            643854 B
//! ```
//!
//! ## 下载源
//!
//! - 识别模型 / 词表：HuggingFace 仓库
//!   `csukuangfj/sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8`。
//!   主机顺序：环境变量 `HF_ENDPOINT` → `https://huggingface.co` →
//!   `https://hf-mirror.com`（逐台重试，失败自动换下一台）。
//! - VAD：**优先 GitHub Release**
//!   `https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx`
//!   （实测可用）；HF 的 `csukuangfj/sherpa-onnx-vad-models` 路径已失效
//!   （返回 29 字节错误页），只作为最后的兜底。
//!
//! ## 实现要点
//!
//! - 先写 `<目标>.part`，校验大小通过后再 rename 成正式文件；**取消时保留
//!   `.part`**，下次下载用 HTTP `Range` 断点续传（服务器忽略 Range 返回 200
//!   时自动从头重写）。
//! - 进度（`done` / `total` / `label` / `message`）经 [`super::LIVE_EVENT`]
//!   推送，节流 250ms 或 4MB（状态切换立即推）。
//! - 大小校验：词表 / VAD 必须与预期字节数完全一致（小文件，不一致基本是
//!   错误页）；识别模型若上游仓库更新导致大小变化，`> 600MB` 兜底接受并
//!   在日志里如实说明。
//! - HTTP 复用应用代理配置（`net_config::http_agent`），代理串非法时退回直连。
//! - 单飞：下载线程运行期间重复 `live_download_models` 只返回当前状态；
//!   取消后立刻再点会等旧线程退出后自动重启（`download_restart`）。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::AppHandle;

use super::{LiveDownloadState, LiveStatus};

/// 识别模型（INT8，约 625MB）。
const ASR_MODEL_BYTES: u64 = 655_542_604;
/// 识别词表。
const ASR_TOKENS_BYTES: u64 = 28_557;
/// Silero VAD。
const VAD_MODEL_BYTES: u64 = 643_854;
/// 识别模型「上游更新」兜底阈值：超过 600MB 就当量化模型本体接受。
const MODEL_MIN_ACCEPT_BYTES: u64 = 600 * 1024 * 1024;
/// 进度节流：间隔 250ms 或新增 4MB 推送一次。
const PROGRESS_INTERVAL_MS: u64 = 250;
const PROGRESS_STEP_BYTES: u64 = 4 * 1024 * 1024;
/// 下载分块。
const DOWNLOAD_CHUNK: usize = 64 * 1024;
/// 下载读取超时（秒）：单块读不到数据才失败，整文件不设总超时。
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;

const HF_ENDPOINT_ENV: &str = "HF_ENDPOINT";
const HF_DEFAULT: &str = "https://huggingface.co";
const HF_MIRROR: &str = "https://hf-mirror.com";
const ASR_MODEL_REPO: &str = "csukuangfj/sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8";
const VAD_HF_REPO: &str = "csukuangfj/sherpa-onnx-vad-models";
const VAD_GITHUB_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx";

/// 一个待下载文件。
struct ModelFile {
    /// 展示用文件名（进度 `label`）
    label: &'static str,
    target: PathBuf,
    expected: u64,
    /// 识别模型允许「> 600MB 兜底」；小文件不允许
    allow_large: bool,
    /// 按优先级排序的下载地址（主机回退）
    urls: Vec<String>,
}

#[derive(Debug)]
enum FileError {
    Cancelled,
    Failed(String),
}

enum Outcome {
    Done,
    Cancelled,
    Failed(String),
}

// ============================================================
// 公共入口（Tauri 命令 / pipeline 自动开始）
// ============================================================

/// 开始下载（幂等）。`auto_start = true` 表示「开始聆听」等模型：下载完成后
/// 自动拉起 pipeline；下载失败 / 取消则把 pending 会话标 error / 回 idle。
///
/// P2-2：模型其实已经就绪（下载线程刚写完 / 边下边点「开始聆听」）时，这个
/// 分支也必须消费 `auto_start` 并补起 loader，否则等待中的会话会永远停在
/// `starting`（TOCTOU：`live_start` 判模型缺失 → 这里判已就绪 → 两边都不拉线程）。
pub(crate) fn start(app: &AppHandle, auto_start: bool) -> LiveStatus {
    super::bind_app(app);
    let root = match super::models_root() {
        Ok(root) => root,
        Err(error) => {
            super::publish_status(|status| {
                status.download = failed_state(&error);
            });
            return super::current_status();
        }
    };
    let total = expected_total();
    let mut spawn: Option<Arc<AtomicBool>> = None;
    // 模型已就绪且有人在等自动开始：锁外补起 pipeline 线程（不持锁 spawn）。
    let mut pending_loader = false;
    let status = {
        let mut guard = super::lock_state();
        guard.auto_start |= auto_start;
        if guard.download_active {
            // 旧线程还在收尾（取消 / 出错后立刻再点）：等它退出后自动重启。
            if !matches!(
                guard.status.download.state.as_str(),
                "downloading" | "verifying"
            ) {
                guard.download_restart = true;
            }
            guard.status.clone()
        } else if super::models_ready_at(&root) {
            // TOCTOU 修复：模型已就绪时把 pending 标记收走并补一次 loader。
            pending_loader = mark_ready_locked(&mut guard, total);
            guard.status.clone()
        } else {
            let cancel = Arc::new(AtomicBool::new(false));
            guard.download_active = true;
            guard.download_cancel = Some(Arc::clone(&cancel));
            guard.status.download = LiveDownloadState {
                state: "downloading".into(),
                done: 0,
                total,
                label: "准备下载".into(),
                message: String::new(),
            };
            spawn = Some(cancel);
            guard.status.clone()
        }
    };
    // 先推状态快照，再起线程：否则线程内的进度 / listening 事件可能先于这份
    // 快照到达前端，事件乱序会让进度或阶段短暂回退。
    super::emit_status(&status);
    if let Some(cancel) = spawn {
        spawn_worker(app.clone(), cancel);
    }
    if pending_loader {
        // 会话存在但 pipeline 线程还没起来时补起；已在跑 / 没有会话则无操作。
        super::spawn_loader_if_pending(app);
    }
    super::current_status()
}

/// 模型已就绪分支（锁内）：写 `done` 状态并**取走** `auto_start`。
///
/// 返回 `true` 表示有人在等「下载完成后自动开始」，调用方要在锁外补一次
/// [`super::spawn_loader_if_pending`]（P2-2：不消费标记会让 `live_start`
/// 永远停在 `starting`）。
fn mark_ready_locked(guard: &mut super::LiveState, total: u64) -> bool {
    guard.status.download = done_state(total);
    guard.status.model_ready = true;
    std::mem::take(&mut guard.auto_start)
}

/// 取消下载（`.part` 保留，可断点续传）。正在等待模型自动开始的话同时放弃启动。
pub(crate) fn cancel(app: Option<&AppHandle>) -> LiveStatus {
    if let Some(app) = app {
        super::bind_app(app);
    }
    let status = {
        let mut guard = super::lock_state();
        stop_locked(&mut guard);
        guard.status.clone()
    };
    super::emit_status(&status);
    super::current_status()
}

/// 停掉下载但不推送事件（`pipeline::stop` 内部用，外部另有一次统一推送）。
pub(crate) fn cancel_pending() {
    let mut guard = super::lock_state();
    stop_locked(&mut guard);
}

/// 取消下载 + 放弃「下载完成后自动开始」。
fn stop_locked(guard: &mut super::LiveState) {
    guard.auto_start = false;
    guard.download_restart = false;
    if let Some(cancel) = guard.download_cancel.take() {
        cancel.store(true, Ordering::SeqCst);
    }
    if matches!(
        guard.status.download.state.as_str(),
        "downloading" | "verifying"
    ) {
        guard.status.download.state = "cancelled".into();
        guard.status.download.message =
            "已取消下载（已下载的部分保留，再次下载会断点续传）".into();
    }
    // 「开始聆听」正在等模型下载：取消下载 = 放弃这次启动，回到 idle。
    if guard
        .session
        .as_ref()
        .map(|session| session.pipeline.is_none())
        .unwrap_or(false)
    {
        guard.session = None;
        guard.status.running = false;
        guard.status.phase = "idle".into();
        guard.status.error.clear();
    }
}

// ============================================================
// 下载线程
// ============================================================

fn spawn_worker(app: AppHandle, cancel: Arc<AtomicBool>) {
    let spawned = std::thread::Builder::new()
        .name("island-live-download".into())
        .spawn(move || {
            let outcome = run(&app, &cancel);
            match outcome {
                Outcome::Done | Outcome::Cancelled => {}
                Outcome::Failed(message) => {
                    if !cancel.load(Ordering::SeqCst) {
                        let message_for_status = message.clone();
                        super::publish_status(|status| {
                            status.download = failed_state(&message_for_status);
                        });
                    }
                    // 「开始聆听」等着的模型下载失败：会话标 error，别卡在 starting。
                    if super::take_auto_start() {
                        super::fail_pending_session(message);
                    }
                }
            }
            let restart = {
                let mut guard = super::lock_state();
                guard.download_active = false;
                guard.download_cancel = None;
                let restart = guard.download_restart;
                guard.download_restart = false;
                restart
            };
            if restart {
                start(&app, false);
            }
        });
    if let Err(error) = spawned {
        eprintln!("[zbar-island-live] 启动模型下载线程失败: {error}");
        let message = format!("启动模型下载线程失败：{error}");
        let message_for_status = message.clone();
        let mut guard = super::lock_state();
        guard.download_active = false;
        guard.download_cancel = None;
        guard.status.download = failed_state(&message_for_status);
        drop(guard);
        super::emit_current_status();
        if super::take_auto_start() {
            super::fail_pending_session(message);
        }
    }
}

fn run(app: &AppHandle, cancel: &Arc<AtomicBool>) -> Outcome {
    let root = match super::models_root() {
        Ok(root) => root,
        Err(error) => return Outcome::Failed(error),
    };
    let files = model_files(&root);
    let total: u64 = files.iter().map(|file| file.expected).sum();
    let mut progress = Progress::new(total);
    let mut base_done = 0u64;
    for file in &files {
        if cancel.load(Ordering::SeqCst) {
            return Outcome::Cancelled;
        }
        match download_file(cancel, file, base_done, &mut progress) {
            Ok(()) => base_done += file.expected,
            Err(FileError::Cancelled) => return Outcome::Cancelled,
            Err(FileError::Failed(message)) => return Outcome::Failed(message),
        }
    }
    if cancel.load(Ordering::SeqCst) {
        return Outcome::Cancelled;
    }
    progress.verifying(cancel, base_done);
    if !super::models_ready_at(&root) {
        return Outcome::Failed(
            "模型文件校验失败：下载完成后仍缺文件，请检查磁盘空间或重新下载".into(),
        );
    }
    progress.done(cancel, total);
    // 模型就绪：如果这次下载是「开始聆听」触发的，自动拉起 pipeline。
    if super::take_auto_start() {
        super::spawn_loader_if_pending(app);
    }
    Outcome::Done
}

/// 逐文件下载：已有正确大小的正式文件直接跳过；否则从 `.part` 续传。
fn download_file(
    cancel: &Arc<AtomicBool>,
    file: &ModelFile,
    base_done: u64,
    progress: &mut Progress,
) -> Result<(), FileError> {
    if let Ok(metadata) = std::fs::metadata(&file.target) {
        if metadata.is_file() && size_ok(file.label, metadata.len(), file.expected, file.allow_large)
        {
            progress.downloading(cancel, base_done + metadata.len(), file.label);
            return Ok(());
        }
        // 大小不对的残file（旧版本 / 中断的 rename 前残留）：删掉重下。
        let _ = std::fs::remove_file(&file.target);
    }
    if let Some(parent) = file.target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FileError::Failed(format!("创建模型目录失败（{}）：{error}", parent.display()))
        })?;
    }
    let part = part_path(&file.target);
    let mut offset = std::fs::metadata(&part).map(|meta| meta.len()).unwrap_or(0);
    if offset > file.expected {
        // .part 比预期还大（换过模型 / 文件损坏）：清掉从头来。
        let _ = std::fs::remove_file(&part);
        offset = 0;
    }
    let mut last_error = String::new();
    for url in &file.urls {
        if cancel.load(Ordering::SeqCst) {
            return Err(FileError::Cancelled);
        }
        let agent = download_agent(url);
        match fetch_into_part(&agent, url, &part, offset, cancel, file, base_done, progress) {
            Ok(()) => {
                let actual = std::fs::metadata(&part).map(|meta| meta.len()).unwrap_or(0);
                if !size_ok(file.label, actual, file.expected, file.allow_large) {
                    // 命中错误页 / 镜像不完整：删掉 .part 换下一个下载源重来，
                    // 不能因为第一台主机返回了 200 就放弃后面可用的镜像。
                    let _ = std::fs::remove_file(&part);
                    offset = 0;
                    last_error = format!(
                        "{} 返回的内容大小异常（实际 {actual} 字节，预期 {} 字节），可能命中错误页",
                        file.label, file.expected
                    );
                    continue;
                }
                std::fs::rename(&part, &file.target).map_err(|error| {
                    FileError::Failed(format!(
                        "保存模型文件失败（{}）：{error}",
                        file.target.display()
                    ))
                })?;
                progress.downloading(cancel, base_done + actual, file.label);
                return Ok(());
            }
            Err(FileError::Cancelled) => return Err(FileError::Cancelled),
            Err(FileError::Failed(message)) => {
                last_error = message;
                offset = std::fs::metadata(&part).map(|meta| meta.len()).unwrap_or(0);
            }
        }
    }
    Err(FileError::Failed(if last_error.is_empty() {
        format!("{} 下载失败：所有下载源都不可用", file.label)
    } else {
        format!("{} 下载失败：{last_error}", file.label)
    }))
}

/// 从单个 URL 拉取到 `.part`（支持 Range 续传；取消时保留已下载部分）。
fn fetch_into_part(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    offset: u64,
    cancel: &AtomicBool,
    file: &ModelFile,
    base_done: u64,
    progress: &mut Progress,
) -> Result<(), FileError> {
    let mut request = agent.get(url);
    if offset > 0 {
        request = request.set("Range", &format!("bytes={offset}-"));
    }
    let response = match request.call() {
        Ok(response) => response,
        Err(ureq::Error::Status(code, _)) => {
            // 416：.part 已经不小于远端文件，交给大小校验判断是否已完成。
            if code == 416 && offset > 0 {
                return Ok(());
            }
            return Err(FileError::Failed(format!("HTTP {code}")));
        }
        Err(ureq::Error::Transport(error)) => {
            return Err(FileError::Failed(format!("网络连接失败：{error}")))
        }
    };
    let status = response.status();
    // 206 = 服务器接受了 Range；200 = 忽略 Range，只能从头写。
    let resume = status == 206 && offset > 0;
    let (mut handle, mut written) = if resume {
        match std::fs::OpenOptions::new().append(true).open(part) {
            Ok(handle) => (handle, offset),
            Err(error) => {
                return Err(FileError::Failed(format!(
                    "打开断点文件失败（{}）：{error}",
                    part.display()
                )))
            }
        }
    } else {
        (
            std::fs::File::create(part).map_err(|error| {
                FileError::Failed(format!("创建下载临时文件失败（{}）：{error}", part.display()))
            })?,
            0,
        )
    };
    let mut reader = response.into_reader();
    let mut buffer = vec![0u8; DOWNLOAD_CHUNK];
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = handle.flush();
            return Err(FileError::Cancelled);
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|error| FileError::Failed(format!("下载中断：{error}")))?;
        if read == 0 {
            break;
        }
        handle
            .write_all(&buffer[..read])
            .map_err(|error| FileError::Failed(format!("写入模型文件失败：{error}")))?;
        written += read as u64;
        progress.downloading(cancel, base_done + written, file.label);
    }
    handle
        .flush()
        .map_err(|error| FileError::Failed(format!("写入模型文件失败：{error}")))?;
    Ok(())
}

/// 进度推送器：节流 250ms / 4MB，阶段切换（label 变化）立即推。
struct Progress {
    total: u64,
    last_at: Instant,
    last_done: u64,
    label: &'static str,
}

impl Progress {
    fn new(total: u64) -> Self {
        Self {
            total,
            // 让第一次进度更新立即推送
            last_at: Instant::now() - Duration::from_secs(1),
            last_done: 0,
            label: "",
        }
    }

    fn downloading(&mut self, cancel: &AtomicBool, done: u64, label: &'static str) {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let stage_changed = label != self.label;
        let due = self.last_at.elapsed() >= Duration::from_millis(PROGRESS_INTERVAL_MS);
        let stepped = done.saturating_sub(self.last_done) >= PROGRESS_STEP_BYTES;
        if !stage_changed && !due && !stepped {
            return;
        }
        self.last_at = Instant::now();
        self.last_done = done;
        self.label = label;
        let total = self.total;
        super::publish_status(|status| {
            status.download = LiveDownloadState {
                state: "downloading".into(),
                done,
                total,
                label: label.to_string(),
                message: String::new(),
            };
        });
    }

    fn verifying(&mut self, cancel: &AtomicBool, done: u64) {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        self.last_at = Instant::now();
        self.last_done = done;
        self.label = "校验模型文件";
        let total = self.total;
        super::publish_status(|status| {
            status.download = LiveDownloadState {
                state: "verifying".into(),
                done,
                total,
                label: "校验模型文件".into(),
                message: String::new(),
            };
        });
    }

    fn done(&mut self, cancel: &AtomicBool, total: u64) {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        super::publish_status(|status| {
            status.download = done_state(total);
            // 模型已校验就绪：状态事件也要带上，否则手动下载完成后前端
            // 仍显示「未下载」，直到下一次 live_status 快照。
            status.model_ready = true;
        });
    }
}

fn done_state(total: u64) -> LiveDownloadState {
    LiveDownloadState {
        state: "done".into(),
        done: total,
        total,
        label: "模型已就绪".into(),
        message: String::new(),
    }
}

fn failed_state(message: &str) -> LiveDownloadState {
    LiveDownloadState {
        state: "error".into(),
        done: 0,
        total: 0,
        label: "下载失败".into(),
        message: message.to_string(),
    }
}

// ============================================================
// 下载源 / 文件清单 / 大小校验（纯函数，单测覆盖）
// ============================================================

/// HF 主机顺序：`HF_ENDPOINT`（用户显式指定，镜像站常用）→ 官方 → 镜像。
fn hf_hosts_from(endpoint: Option<&str>) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for host in [endpoint.unwrap_or_default(), HF_DEFAULT, HF_MIRROR] {
        let host = host.trim().trim_end_matches('/');
        if host.is_empty() || hosts.iter().any(|known| known == host) {
            continue;
        }
        hosts.push(host.to_string());
    }
    hosts
}

fn hf_hosts() -> Vec<String> {
    let endpoint = std::env::var(HF_ENDPOINT_ENV).ok();
    hf_hosts_from(endpoint.as_deref())
}

/// `<host>/<repo>/resolve/main/<file>`（HF 的直链解析地址）。
fn hf_resolve_urls(hosts: &[String], repo: &str, file: &str) -> Vec<String> {
    hosts
        .iter()
        .map(|host| format!("{host}/{repo}/resolve/main/{file}"))
        .collect()
}

/// 本次要下载的三个文件（顺序固定：模型 → 词表 → VAD，进度累加稳定）。
fn model_files(root: &Path) -> Vec<ModelFile> {
    let hosts = hf_hosts();
    let asr_dir = super::asr_model_dir(root);
    let mut vad_urls = vec![VAD_GITHUB_URL.to_string()];
    vad_urls.extend(hf_resolve_urls(
        &hosts,
        VAD_HF_REPO,
        super::VAD_MODEL_FILE,
    ));
    vec![
        ModelFile {
            label: super::ASR_MODEL_FILE,
            target: asr_dir.join(super::ASR_MODEL_FILE),
            expected: ASR_MODEL_BYTES,
            allow_large: true,
            urls: hf_resolve_urls(&hosts, ASR_MODEL_REPO, super::ASR_MODEL_FILE),
        },
        ModelFile {
            label: super::ASR_TOKENS_FILE,
            target: asr_dir.join(super::ASR_TOKENS_FILE),
            expected: ASR_TOKENS_BYTES,
            allow_large: false,
            urls: hf_resolve_urls(&hosts, ASR_MODEL_REPO, super::ASR_TOKENS_FILE),
        },
        ModelFile {
            label: super::VAD_MODEL_FILE,
            target: super::vad_model_path(root),
            expected: VAD_MODEL_BYTES,
            allow_large: false,
            urls: vad_urls,
        },
    ]
}

fn expected_total() -> u64 {
    ASR_MODEL_BYTES + ASR_TOKENS_BYTES + VAD_MODEL_BYTES
}

/// 下载完成后的 `.part` 路径（与原文件名同目录，便于一眼看出是临时文件）。
fn part_path(target: &Path) -> PathBuf {
    PathBuf::from(format!("{}.part", target.to_string_lossy()))
}

/// 大小校验：完全一致通过；识别模型允许「> 600MB 兜底」（上游仓库更新）。
fn size_ok(label: &str, actual: u64, expected: u64, allow_large: bool) -> bool {
    if actual == expected {
        return true;
    }
    if allow_large && actual > MODEL_MIN_ACCEPT_BYTES {
        eprintln!(
            "[zbar-island-live] {label} 大小 {actual} 字节与预期 {expected} 字节不一致\
（上游仓库可能更新），按 >600MB 兜底接受"
        );
        return true;
    }
    false
}

/// 接口地址是否指向本机（如自建镜像 / 测试服务：不走用户代理）。
fn is_loopback_url(url: &str) -> bool {
    let Ok(url) = tauri::Url::parse(url) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default().trim_start_matches('[');
    host.eq_ignore_ascii_case("localhost")
        || host.starts_with("127.")
        || host.trim_end_matches(']') == "::1"
}

/// 下载用 Agent：本机地址直连；其它复用应用代理配置（`~/.agenthud/proxy.json`），
/// 代理非法退回直连。
fn download_agent(url: &str) -> ureq::Agent {
    if is_loopback_url(url) {
        return ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
            .build();
    }
    match crate::net_config::http_agent(DOWNLOAD_TIMEOUT_SECS) {
        Ok(agent) => agent,
        Err(error) => {
            eprintln!("[zbar-island-live] 下载代理配置不可用，改走直连: {error}");
            ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
                .timeout_write(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
                .build()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P2-2：`models_ready_at` 分支必须消费 `auto_start` 并告诉调用方补 loader；
    /// 否则「边下边点开始聆听」的会话会永远停在 `starting`。
    #[test]
    fn models_ready_branch_consumes_auto_start() {
        let mut state = super::super::LiveState::default();
        state.auto_start = true;
        let pending = mark_ready_locked(&mut state, 123);
        assert!(pending, "有等待者时应返回 true 让调用方补 loader");
        assert!(!state.auto_start, "auto_start 标记必须被取走（只拉起一次）");
        assert_eq!(state.status.download.state, "done");
        assert!(state.status.model_ready, "就绪分支应置 model_ready");

        // 没有等待者：不补 loader，但仍要把下载状态写成 done。
        let mut state = super::super::LiveState::default();
        assert!(!mark_ready_locked(&mut state, 0));
        assert_eq!(state.status.download.state, "done");
    }

    #[test]
    fn hf_hosts_prefer_env_then_official_then_mirror() {
        let expected = |hosts: [&str; 2]| hosts.map(str::to_string);
        // env 指到镜像站时，与内置镜像去重，只留两台。
        assert_eq!(
            hf_hosts_from(Some("https://hf-mirror.com/")),
            expected(["https://hf-mirror.com", HF_DEFAULT])
        );
        // 与官方重复时不重复列出
        assert_eq!(
            hf_hosts_from(Some(HF_DEFAULT)),
            expected([HF_DEFAULT, HF_MIRROR])
        );
        // 没有 env 时官方 → 镜像
        assert_eq!(hf_hosts_from(None), expected([HF_DEFAULT, HF_MIRROR]));
        assert_eq!(hf_hosts_from(Some("   ")), expected([HF_DEFAULT, HF_MIRROR]));
    }

    #[test]
    fn resolve_urls_use_hf_repo_layout() {
        let hosts = vec!["https://huggingface.co".to_string()];
        assert_eq!(
            hf_resolve_urls(&hosts, ASR_MODEL_REPO, "model.int8.onnx"),
            vec![
                "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt_ctc-0.6b-ja-35000-int8/resolve/main/model.int8.onnx"
                    .to_string()
            ]
        );
    }

    #[test]
    fn model_files_layout_and_source_priority() {
        let root = PathBuf::from("models-root");
        let files = model_files(&root);
        assert_eq!(files.len(), 3);

        assert_eq!(files[0].label, "model.int8.onnx");
        assert_eq!(
            files[0].target,
            root.join("parakeet-ja-0.6b-int8").join("model.int8.onnx")
        );
        assert_eq!(files[0].expected, 655_542_604);
        assert!(files[0].allow_large);

        assert_eq!(files[1].label, "tokens.txt");
        assert_eq!(files[1].expected, 28_557);
        assert!(!files[1].allow_large);

        assert_eq!(files[2].label, "silero_vad.onnx");
        assert_eq!(files[2].target, root.join("silero_vad.onnx"));
        assert_eq!(files[2].expected, 643_854);
        assert_eq!(
            files[2].urls.first().map(String::as_str),
            Some(VAD_GITHUB_URL),
            "VAD 必须先试 GitHub Release"
        );
        assert!(
            files[2].urls.iter().any(|url| url.contains("sherpa-onnx-vad-models")),
            "HF 作为兜底源保留"
        );

        assert_eq!(
            expected_total(),
            655_542_604 + 28_557 + 643_854,
            "进度条总大小与三个文件之和一致"
        );
    }

    #[test]
    fn part_path_appends_suffix() {
        let target = PathBuf::from("models").join("silero_vad.onnx");
        assert_eq!(
            part_path(&target),
            PathBuf::from("models").join("silero_vad.onnx.part")
        );
    }

    #[test]
    fn size_check_is_exact_for_small_files_and_tolerant_for_model() {
        // 小文件必须完全一致（不一致基本是 29 字节错误页之类）
        assert!(size_ok("tokens.txt", 28_557, 28_557, false));
        assert!(!size_ok("tokens.txt", 29, 28_557, false));
        assert!(!size_ok("silero_vad.onnx", 29, 643_854, false));

        // 识别模型：精确命中；上游更新后 > 600MB 兜底接受
        assert!(size_ok("model.int8.onnx", 655_542_604, 655_542_604, true));
        assert!(size_ok("model.int8.onnx", 620 * 1024 * 1024, 655_542_604, true));
        assert!(!size_ok("model.int8.onnx", 100 * 1024 * 1024, 655_542_604, true));
        assert!(!size_ok("model.int8.onnx", 29, 655_542_604, true));

        // 不开兜底时模型也必须精确
        assert!(!size_ok("model.int8.onnx", 620 * 1024 * 1024, 655_542_604, false));
    }

    /// 起一个只服务一次 GET 的本地 HTTP 服务（验证续传 / 大小兜底 / 主机回退）。
    fn serve_once(
        body: &'static [u8],
        status_line: &'static str,
        extra_headers: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地下载测试端口");
        let addr = listener.local_addr().expect("读取本地测试端口");
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("接受下载测试连接");
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            loop {
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
                let read = socket.read(&mut buffer).expect("读取下载请求头");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let head = format!(
                "HTTP/1.1 {status_line}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).expect("写下载响应头");
            socket.write_all(body).expect("写下载响应体");
            let _ = socket.flush();
            String::from_utf8_lossy(&request).to_string()
        });
        (format!("http://{addr}/model.bin"), handle)
    }

    fn test_file(target: PathBuf, expected: u64, urls: Vec<String>) -> ModelFile {
        ModelFile {
            label: "model.bin",
            target,
            expected,
            allow_large: false,
            urls,
        }
    }

    #[test]
    fn download_file_resumes_part_then_renames() {
        let root = super::super::temp_dir("dl-resume");
        let target = root.join("model.bin");
        let part = part_path(&target);
        std::fs::write(&part, b"abcde").expect("写断点文件");
        let (url, server) = serve_once(b"fghij", "206 Partial Content", "Content-Range: bytes 5-9/10\r\n");
        let file = test_file(target.clone(), 10, vec![url]);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress = Progress::new(10);

        download_file(&cancel, &file, 0, &mut progress).expect("断点续传应成功");
        assert_eq!(std::fs::read(&target).expect("读下载结果"), b"abcdefghij");
        assert!(!part.exists(), "完成后 .part 必须被 rename 掉");
        let request = server.join().expect("本地下载服务线程").to_ascii_lowercase();
        assert!(
            request.contains("range: bytes=5-"),
            "续传请求必须带 Range：{request}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn download_file_restarts_when_server_ignores_range() {
        let root = super::super::temp_dir("dl-ignore-range");
        let target = root.join("model.bin");
        let part = part_path(&target);
        std::fs::write(&part, b"old!!").expect("写断点文件");
        // 服务器忽略 Range 返回 200 全量：必须从头重写而不是追加。
        let (url, server) = serve_once(b"0123456789", "200 OK", "");
        let file = test_file(target.clone(), 10, vec![url]);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress = Progress::new(10);

        download_file(&cancel, &file, 0, &mut progress).expect("忽略 Range 时应从头重写");
        assert_eq!(std::fs::read(&target).expect("读下载结果"), b"0123456789");
        let _ = server.join();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn wrong_size_response_falls_back_to_next_host() {
        let root = super::super::temp_dir("dl-fallback");
        let target = root.join("model.bin");
        // 第一台主机返回 3 字节错误页（预期 10），第二台才是正确内容。
        let (bad, bad_server) = serve_once(b"err", "200 OK", "");
        let (good, good_server) = serve_once(b"0123456789", "200 OK", "");
        let file = test_file(target.clone(), 10, vec![bad, good]);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress = Progress::new(10);

        download_file(&cancel, &file, 0, &mut progress).expect("应自动回退到第二台主机");
        assert_eq!(std::fs::read(&target).expect("读下载结果"), b"0123456789");
        let _ = bad_server.join();
        let _ = good_server.join();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cancel_keeps_part_file_and_reports_cancelled() {
        let root = super::super::temp_dir("dl-cancel");
        let target = root.join("model.bin");
        let part = part_path(&target);
        std::fs::write(&part, b"partial").expect("写断点文件");
        let file = test_file(target.clone(), 10, vec!["http://127.0.0.1:1/never".into()]);
        let cancel = Arc::new(AtomicBool::new(true));
        let mut progress = Progress::new(10);

        let result = download_file(&cancel, &file, 0, &mut progress);
        assert!(
            matches!(result, Err(FileError::Cancelled)),
            "已取消时必须返回 Cancelled"
        );
        assert_eq!(
            std::fs::read(&part).expect("读断点文件"),
            b"partial",
            "取消必须保留 .part"
        );
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
