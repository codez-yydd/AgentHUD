//! 灵动岛实时字幕（live）：采集 → VAD 断句 → 日语识别 → 中文翻译 → 字幕。
//!
//! 实时字幕链路（sherpa-onnx）：
//! 系统声音 / 麦克风 → Silero VAD 断句 → Parakeet 日语 0.6B INT8 离线识别
//! → OpenAI 兼容流式翻译 → 字幕事件。
//!
//! 子模块：
//!
//! - [`audio`]：cpal 采集（Windows WASAPI loopback 系统声音 / 全平台麦克风），
//!   回调里降混单声道并重采样到 [`TARGET_RATE`]；
//! - [`vad`]：sherpa-onnx Silero VAD 断句 + 原始音频切片；
//! - [`asr`]：sherpa-onnx NeMo CTC 离线识别（Parakeet 日语 0.6B INT8）；
//! - [`pipeline`]：线程编排（采集主循环 + 1 个定稿识别线程 + 1 个草稿识别
//!   线程 + 定稿翻译并发 + 草稿翻译单飞），见模块注释；
//! - [`download`]：模型下载（`.part` + Range 断点续传、HF / 镜像回退、进度事件）；
//! - [`translate`]：OpenAI 兼容 `/chat/completions` 流式 SSE 翻译；
//! - [`subtitles`]：字幕后端历史（≤80 条，草稿用「下一句 id」占位）。
//!
//! ## 模型目录布局（`models_dir` 下）
//!
//! ```text
//! <models_dir>/parakeet-ja-0.6b-int8/model.int8.onnx   （约 625MB）
//! <models_dir>/parakeet-ja-0.6b-int8/tokens.txt
//! <models_dir>/silero_vad.onnx
//! ```
//!
//! `models_dir` 支持绝对路径；相对路径以数据目录为根（`~/.agenthud/models`，
//! 更名前为 `~/.zbar`，见 [`crate::pricing::config_dir`]）。
//!
//! ## 前端契约（`src/island/types.ts` / `api.ts`）
//!
//! 命令：`live_status` / `live_start` / `live_stop` / `live_devices` /
//! `live_download_models` / `live_cancel_download` / `live_captions` /
//! `live_clear_captions`；事件 [`LIVE_EVENT`]（全量状态）与
//! [`LIVE_CAPTION_EVENT`]（单条字幕：草稿更新 / 定稿 / 译文增量）。
//!
//! 参数在 `live_start` 时快照（改设置需重新开始聆听生效）；岛总开关关闭时
//! 由 `island::stop_services` 调 [`stop`] 停掉采集并尽力补最后一句话。

pub mod asr;
pub mod audio;
pub mod download;
pub mod pipeline;
pub mod subtitles;
pub mod translate;
pub mod vad;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// 实时字幕状态事件（[`LiveStatus`] 全量快照；状态变化立即推，level 节流）。
pub const LIVE_EVENT: &str = "zbar://island-live";
/// 实时字幕单条字幕事件（[`LiveCaption`]：草稿更新 / 定稿 / 译文增量）。
pub const LIVE_CAPTION_EVENT: &str = "zbar://island-live-caption";
/// 识别与 VAD 的统一采样率（sherpa-onnx 只吃 16k 单声道 f32）。
pub(crate) const TARGET_RATE: u32 = 16000;

/// 日语识别模型目录名（参考项目下载的模型包名）。
pub(crate) const ASR_MODEL_DIR: &str = "parakeet-ja-0.6b-int8";
/// 日语识别模型文件名（INT8 量化，约 625MB）。
pub(crate) const ASR_MODEL_FILE: &str = "model.int8.onnx";
/// 日语识别词表文件名。
pub(crate) const ASR_TOKENS_FILE: &str = "tokens.txt";
/// Silero VAD 模型文件名。
pub(crate) const VAD_MODEL_FILE: &str = "silero_vad.onnx";

// ============================================================
// 前端 DTO（serde camelCase，字段与 src/island/types.ts 一一对应）
// ============================================================

/// 模型下载进度（`done` / `total` 为字节数；拿不到总大小时 `total = 0`）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDownloadState {
    /// idle / downloading / verifying / done / error / cancelled
    pub state: String,
    pub done: u64,
    pub total: u64,
    /// 当前正在下载的文件名 / 阶段名
    pub label: String,
    /// 失败原因（`state = error` 时非空）
    pub message: String,
}

impl Default for LiveDownloadState {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            done: 0,
            total: 0,
            label: String::new(),
            message: String::new(),
        }
    }
}

/// 实时字幕运行快照（命令返回 + [`LIVE_EVENT`] 事件载荷）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStatus {
    pub running: bool,
    /// idle / starting / listening / stopping / error
    pub phase: String,
    /// 失败原因（`phase = error` 时非空）
    pub error: String,
    /// 三个模型文件是否齐全且非空（每次取快照都按磁盘实时判定）
    pub model_ready: bool,
    /// 采集来源：loopback（系统声音）/ mic（麦克风）
    pub source: String,
    pub device: String,
    /// 最近一次音频电平 0..1（RMS → (20log10+60)/60）
    pub level: f32,
    pub download: LiveDownloadState,
}

impl Default for LiveStatus {
    fn default() -> Self {
        Self {
            running: false,
            phase: "idle".into(),
            error: String::new(),
            model_ready: false,
            source: "loopback".into(),
            device: String::new(),
            level: 0.0,
            download: LiveDownloadState::default(),
        }
    }
}

/// 一条字幕：草稿（`draft`）会滚动刷新，定稿（`final`）不再变化。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveCaption {
    pub id: u64,
    /// 日语原文
    pub jp: String,
    /// 中文译文（流式增量更新；翻译关闭时为空）
    pub zh: String,
    pub draft: bool,
    #[serde(rename = "final")]
    pub final_: bool,
    /// 毫秒时间戳（本机时钟）
    pub ts: i64,
}

/// 可选音频设备列表（按采集来源分组；id 即设备名，空 = 默认设备）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDevices {
    /// 系统声音（loopback）设备：Windows 的输出设备；其它平台为空
    pub loopback: Vec<String>,
    /// 麦克风（输入）设备
    pub mic: Vec<String>,
}

impl From<audio::LiveDevices> for LiveDevices {
    fn from(devices: audio::LiveDevices) -> Self {
        Self {
            loopback: devices.loopback,
            mic: devices.mic,
        }
    }
}

// ============================================================
// 运行时配置快照（settings.json 的 live.*）
// ============================================================

/// 实时字幕运行时配置快照（`settings.json` 里 `live.*` 的扁平化视图）。
///
/// 与 `settings::LiveDoc` 的区别：类型换成下游好用的 `u64` / `usize` / `f32`，
/// 并只在 pipeline 启动时读一次（改设置需重启字幕会话才生效）。
#[derive(Debug, Clone)]
pub(crate) struct LiveSettings {
    /// 模型根目录：绝对路径原样，相对路径以数据目录为根，空则 `models`
    pub models_dir: String,
    /// `loopback`（系统声音，仅 Windows）或 `mic`（麦克风）
    pub audio_source: String,
    /// 设备名；空 = 默认设备
    pub audio_device: String,
    /// VAD 语音阈值（0.1~0.9）
    pub vad_threshold: f32,
    /// 判定一句结束所需的静音时长（ms）
    pub vad_min_silence_ms: u64,
    /// 判定语音开始所需的最短语音时长（ms）
    pub vad_min_speech_ms: u64,
    /// 是否实时出草稿字（向导流重识别）
    pub asr_partial: bool,
    /// 草稿重识别的最小间隔（ms）
    pub asr_draft_interval_ms: u64,
    /// 整句识别线程数
    pub asr_final_threads: usize,
    /// VAD 单句最长时长（秒），超时强制切句
    pub asr_max_segment_s: u64,
    /// 翻译上文字幕开关（语义见 [`translate`] 模块注释：开启时把最近
    /// `translate_context_lines` 句定稿日文作为翻译上文，上限 3 句）
    pub asr_context_prompt: bool,
    /// 是否启用翻译
    pub translate_enabled: bool,
    /// 是否实时翻译草稿
    pub translate_live: bool,
    /// 草稿翻译最小间隔（ms）
    pub translate_live_min_interval_ms: u64,
    /// 草稿翻译最少新增字符数
    pub translate_live_min_new_chars: usize,
    /// 翻译上下文保留的句数
    pub translate_context_lines: usize,
}

/// 读设置文档里的 `live.*`（每次调用都重新读，不缓存）。
pub(crate) fn settings() -> LiveSettings {
    let live = super::settings::load().live;
    LiveSettings {
        models_dir: live.models_dir.clone(),
        audio_source: live.audio.source.clone(),
        audio_device: live.audio.device.clone(),
        vad_threshold: live.vad.threshold as f32,
        vad_min_silence_ms: live.vad.min_silence_ms.max(0) as u64,
        vad_min_speech_ms: live.vad.min_speech_ms.max(0) as u64,
        asr_partial: live.asr.partial,
        asr_draft_interval_ms: live.asr.draft_interval_ms.max(0) as u64,
        asr_final_threads: live.asr.final_threads.max(1) as usize,
        asr_max_segment_s: live.asr.max_segment_s.max(1) as u64,
        asr_context_prompt: live.asr.context_prompt,
        translate_enabled: live.translate.enabled,
        translate_live: live.translate.live,
        translate_live_min_interval_ms: live.translate.live_min_interval_ms.max(0) as u64,
        translate_live_min_new_chars: live.translate.live_min_new_chars.max(0) as usize,
        translate_context_lines: live.translate.context_lines.max(0) as usize,
    }
}

/// 解析模型根目录：绝对路径原样返回，相对路径拼到数据目录下，空回退 `models`。
pub(crate) fn models_root() -> Result<PathBuf, String> {
    resolve_models_root(&settings().models_dir)
}

fn resolve_models_root(models_dir: &str) -> Result<PathBuf, String> {
    let raw = models_dir.trim();
    // 空值回退："models" 是参考项目的默认目录名，按相对路径落到数据目录下。
    let dir = if raw.is_empty() { "models" } else { raw };
    let path = Path::new(dir);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(crate::pricing::config_dir()?.join(dir))
}

/// 日语识别模型目录：`<root>/parakeet-ja-0.6b-int8`
pub(crate) fn asr_model_dir(root: &Path) -> PathBuf {
    root.join(ASR_MODEL_DIR)
}

/// Silero VAD 模型路径：`<root>/silero_vad.onnx`
pub(crate) fn vad_model_path(root: &Path) -> PathBuf {
    root.join(VAD_MODEL_FILE)
}

/// 三个模型文件是否齐全且非空（缺失或 0 字节都算没下载完）。
pub(crate) fn models_ready() -> bool {
    models_root()
        .map(|root| models_ready_at(&root))
        .unwrap_or(false)
}

pub(crate) fn models_ready_at(root: &Path) -> bool {
    let asr = asr_model_dir(root);
    file_nonempty(&asr.join(ASR_MODEL_FILE))
        && file_nonempty(&asr.join(ASR_TOKENS_FILE))
        && file_nonempty(&vad_model_path(root))
}

/// 文件存在且非空（模型完整性检查的公共口径）。
pub(crate) fn file_nonempty(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() > 0)
        .unwrap_or(false)
}

/// 单测用临时目录（进程内唯一；调用方负责清理）。
#[cfg(test)]
pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agenthud-live-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建测试临时目录");
    dir
}

// ============================================================
// 进程内共享运行时（状态 + 会话句柄 + 应用句柄）
// ============================================================

/// 一次聆听会话的线程句柄（`epoch` 用来丢弃上一个会话迟到的收尾）。
pub(crate) struct Session {
    pub(crate) epoch: u64,
    /// 置 true 后：采集 keeper 线程 50ms 内停流，VAD 主循环 / 草稿线程退出。
    pub(crate) stop: Arc<AtomicBool>,
    /// 采集 keeper 线程（由 pipeline 线程建流成功后登记，便于 stop 时 join）。
    pub(crate) audio: Option<JoinHandle<()>>,
    /// pipeline 线程（模型加载 + VAD 主循环）。
    pub(crate) pipeline: Option<JoinHandle<()>>,
}

pub(crate) struct LiveState {
    pub(crate) status: LiveStatus,
    pub(crate) app: Option<AppHandle>,
    pub(crate) session: Option<Session>,
    /// 会话代号：每次 live_start 自增；旧线程收尾时据此判断是否已过期。
    pub(crate) epoch: u64,
    /// 「下载完成后自动开始聆听」的 pending 标记（live_start 遇模型缺失时置位）。
    pub(crate) auto_start: bool,
    /// 下载线程是否还在跑（含取消后正在收尾的短暂窗口）。
    pub(crate) download_active: bool,
    /// 下载线程已取消 / 出错、但用户又点了下载：等旧线程退出后自动重启。
    pub(crate) download_restart: bool,
    pub(crate) download_cancel: Option<Arc<AtomicBool>>,
}

impl Default for LiveState {
    fn default() -> Self {
        Self {
            status: LiveStatus::default(),
            app: None,
            session: None,
            epoch: 0,
            auto_start: false,
            download_active: false,
            download_restart: false,
            download_cancel: None,
        }
    }
}

fn state_slot() -> &'static Mutex<LiveState> {
    static STATE: OnceLock<Mutex<LiveState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(LiveState::default()))
}

pub(crate) fn lock_state() -> std::sync::MutexGuard<'static, LiveState> {
    state_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 本机毫秒时间戳（字幕 `ts` / 事件时间）。
pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// 记录应用句柄（事件广播用；任何命令首次调用时绑定一次）。
pub(crate) fn bind_app(app: &AppHandle) {
    let mut guard = lock_state();
    if guard.app.is_none() {
        guard.app = Some(app.clone());
    }
}

fn app_handle() -> Option<AppHandle> {
    lock_state().app.clone()
}

/// 在锁内取状态快照（`modelReady` 按磁盘实时刷新）。
pub(crate) fn status_snapshot_locked(guard: &mut LiveState) -> LiveStatus {
    guard.status.model_ready = models_ready();
    guard.status.clone()
}

/// 当前状态快照（不推送事件）。
pub(crate) fn current_status() -> LiveStatus {
    let mut guard = lock_state();
    status_snapshot_locked(&mut guard)
}

/// 推送一份状态快照（payload 为 [`LiveStatus`] 全量）。
pub(crate) fn emit_status(status: &LiveStatus) {
    if let Some(app) = app_handle() {
        let _ = app.emit(LIVE_EVENT, status);
    }
}

/// 推送当前状态。
pub(crate) fn emit_current_status() {
    let (status, app) = {
        let guard = lock_state();
        (guard.status.clone(), guard.app.clone())
    };
    if let Some(app) = app {
        let _ = app.emit(LIVE_EVENT, &status);
    }
}

/// 修改状态 → 立即推送 → 返回新快照（状态变化必须走这里）。
pub(crate) fn publish_status(update: impl FnOnce(&mut LiveStatus)) -> LiveStatus {
    let (status, app) = {
        let mut guard = lock_state();
        update(&mut guard.status);
        (guard.status.clone(), guard.app.clone())
    };
    if let Some(app) = app {
        let _ = app.emit(LIVE_EVENT, &status);
    }
    status
}

/// 仅当 `epoch` 仍是当前**活动会话**时才改状态并推送（旧会话迟到的状态直接丢弃）。
///
/// 与 [`publish_status`] 的区别是锁内多一道会话校验：`stop()` 取走会话后旧
/// 会话的 epoch 立即过期，它迟到的「listening / level」推送不会再翻回状态
/// （P2-1）。返回是否真的写入并推送。
pub(crate) fn publish_status_for_epoch(
    epoch: u64,
    update: impl FnOnce(&mut LiveStatus),
) -> bool {
    let (status, app) = {
        let mut guard = lock_state();
        if !session_is_current(&guard, epoch) {
            return false;
        }
        update(&mut guard.status);
        (guard.status.clone(), guard.app.clone())
    };
    if let Some(app) = app {
        let _ = app.emit(LIVE_EVENT, &status);
    }
    true
}

/// `epoch` 是否是当前活动会话：`stop()` 把会话从槽位取走后即视为过期
/// （旧会话的收尾线程只能走到 [`finish_epoch`]，不能回写运行态）。
fn session_is_current(state: &LiveState, epoch: u64) -> bool {
    state.session.as_ref().map(|session| session.epoch) == Some(epoch)
}

/// 直接返回 `phase = error` 的状态（启动参数非法等同步失败路径）。
pub(crate) fn error_status(message: &str) -> LiveStatus {
    publish_status(|status| {
        status.running = false;
        status.phase = "error".into();
        status.error = message.to_string();
        status.level = 0.0;
    })
}

/// 会话收尾：仅当 `epoch` 仍是当前会话时改状态，旧会话迟到的收尾直接丢弃。
///
/// - 清掉当前会话句柄；
/// - `running = false`、`level = 0`；
/// - `phase = phase`（成功收尾 `idle`，失败 `error`），失败时写中文 `error`。
pub(crate) fn finish_epoch(epoch: u64, phase: &str, error: Option<String>) {
    let app = {
        let mut guard = lock_state();
        if guard.epoch != epoch {
            return;
        }
        if guard.session.as_ref().map(|session| session.epoch) == Some(epoch) {
            guard.session = None;
        }
        guard.status.running = false;
        guard.status.phase = phase.to_string();
        guard.status.level = 0.0;
        match error {
            Some(message) => guard.status.error = message,
            None => guard.status.error.clear(),
        }
        guard.app.clone()
    };
    if let Some(app) = app {
        let status = { lock_state().status.clone() };
        let _ = app.emit(LIVE_EVENT, &status);
    }
}

/// 把当前会话标成 error（下载失败等异步失败路径）。
pub(crate) fn fail_pending_session(message: String) {
    let epoch = lock_state().session.as_ref().map(|session| session.epoch);
    if let Some(epoch) = epoch {
        finish_epoch(epoch, "error", Some(message));
    }
}

/// 取走「下载完成后自动开始」标记。
pub(crate) fn take_auto_start() -> bool {
    let mut guard = lock_state();
    let value = guard.auto_start;
    guard.auto_start = false;
    value
}

/// 会话存在但 pipeline 线程还没起来时，补起 pipeline 线程。
///
/// 两条路径会调用：`live_start`（模型已就绪）与模型下载完成（自动开始）。
pub(crate) fn spawn_loader_if_pending(app: &AppHandle) -> bool {
    let (epoch, stop) = {
        let guard = lock_state();
        let Some(session) = guard.session.as_ref() else {
            return false;
        };
        if session.pipeline.is_some() {
            return false;
        }
        (session.epoch, Arc::clone(&session.stop))
    };
    let app_clone = app.clone();
    let spawned = std::thread::Builder::new()
        .name("island-live-pipeline".into())
        .spawn(move || pipeline::run(app_clone, epoch, stop));
    match spawned {
        Ok(handle) => {
            let mut guard = lock_state();
            if let Some(session) = guard.session.as_mut() {
                if session.epoch == epoch {
                    session.pipeline = Some(handle);
                    return true;
                }
            }
            false
        }
        Err(error) => {
            finish_epoch(epoch, "error", Some(format!("启动实时字幕线程失败：{error}")));
            false
        }
    }
}

/// pipeline 线程建流成功后登记采集句柄（stop 时要 join，确保设备尽快释放）。
pub(crate) fn register_audio_handle(epoch: u64, handle: JoinHandle<()>) {
    let mut guard = lock_state();
    if let Some(session) = guard.session.as_mut() {
        if session.epoch == epoch {
            session.audio = Some(handle);
        }
    }
}

/// 取走采集句柄（pipeline 退出时 join）。
pub(crate) fn take_audio_handle(epoch: u64) -> Option<JoinHandle<()>> {
    let mut guard = lock_state();
    let session = guard.session.as_mut()?;
    if session.epoch == epoch {
        session.audio.take()
    } else {
        None
    }
}

// ============================================================
// Tauri 命令（命令名 / 返回体与前端 api.ts 约定一致）
// ============================================================

/// 当前实时字幕状态快照（不触发任何启动 / 下载动作）。
#[tauri::command]
pub fn live_status(app: AppHandle) -> LiveStatus {
    bind_app(&app);
    let mut status = current_status();
    if !status.running {
        // 空闲态回显当前设置，设置页切换采集来源后无需先开始聆听。
        let settings = settings();
        status.source = settings.audio_source;
        status.device = settings.audio_device;
    }
    status
}

/// 开始聆听（非阻塞）：立即返回 `phase = starting`，模型在后台线程加载。
#[tauri::command]
pub fn live_start(app: AppHandle) -> LiveStatus {
    pipeline::start(&app)
}

/// 停止聆听：立即返回（phase 先 `stopping`，后台收尾完变 `idle`）。
#[tauri::command]
pub fn live_stop(app: AppHandle) -> LiveStatus {
    pipeline::stop(Some(&app))
}

/// 枚举可选音频设备（loopback 仅 Windows；其它平台为空数组）。
#[tauri::command]
pub fn live_devices() -> LiveDevices {
    audio::list_devices().into()
}

/// 下载识别 / VAD 模型（幂等；进度经 [`LIVE_EVENT`] 推送）。
#[tauri::command]
pub fn live_download_models(app: AppHandle) -> LiveStatus {
    download::start(&app, false)
}

/// 取消模型下载（`.part` 保留，可断点续传）。
#[tauri::command]
pub fn live_cancel_download(app: AppHandle) -> LiveStatus {
    download::cancel(Some(&app))
}

/// 字幕历史（旧 → 新，最多 80 条）。
#[tauri::command]
pub fn live_captions() -> Vec<LiveCaption> {
    subtitles::snapshot()
}

/// 清空字幕历史（前端本地状态自行清理，这里只清后端）。
#[tauri::command]
pub fn live_clear_captions() {
    subtitles::clear();
}

/// 岛总开关关闭 / 停服务时停掉实时字幕（尽力补最后一句话，失败静默）。
///
/// 由 `island::stop_services` 调用；没有活动会话时是幂等的空操作。
pub fn stop() {
    let _ = pipeline::stop(None);
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolve_models_root_keeps_absolute_path() {
        let dir = temp_dir("models-root-abs");
        let root = resolve_models_root(dir.to_str().expect("临时目录路径")).expect("解析绝对路径");
        assert_eq!(root, dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_models_root_joins_config_dir_for_relative_path() {
        let config = crate::pricing::config_dir().expect("定位数据目录");
        let root = resolve_models_root("models").expect("解析相对路径");
        assert_eq!(root, config.join("models"));
    }

    #[test]
    fn resolve_models_root_falls_back_to_models_when_blank() {
        let config = crate::pricing::config_dir().expect("定位数据目录");
        let root = resolve_models_root("   ").expect("解析空路径");
        assert_eq!(root, config.join("models"), "空 models_dir 回退 models");
    }

    #[test]
    fn model_paths_live_under_models_root() {
        let root = PathBuf::from("root");
        assert_eq!(asr_model_dir(&root), root.join(ASR_MODEL_DIR));
        assert_eq!(
            asr_model_dir(&root).join(ASR_MODEL_FILE),
            root.join(ASR_MODEL_DIR).join("model.int8.onnx")
        );
        assert_eq!(vad_model_path(&root), root.join("silero_vad.onnx"));
    }

    #[test]
    fn models_ready_requires_three_non_empty_files() {
        let root = temp_dir("models-ready");
        let asr_dir = asr_model_dir(&root);
        fs::create_dir_all(&asr_dir).expect("创建识别模型目录");
        let model = asr_dir.join(ASR_MODEL_FILE);
        let tokens = asr_dir.join(ASR_TOKENS_FILE);
        let vad = vad_model_path(&root);
        for path in [&model, &tokens, &vad] {
            fs::write(path, b"x").expect("写测试模型文件");
        }
        assert!(models_ready_at(&root), "三个文件齐且非空时应就绪");

        fs::write(&vad, b"").expect("清空 VAD 模型");
        assert!(!models_ready_at(&root), "0 字节文件不算就绪");

        fs::write(&vad, b"x").expect("恢复 VAD 模型");
        fs::remove_file(&tokens).expect("删除词表");
        assert!(!models_ready_at(&root), "缺词表不算就绪");

        let _ = fs::remove_dir_all(&root);
    }

    /// 前端契约：DTO 必须序列化成 camelCase，`final` 字段不能带下划线。
    #[test]
    fn dto_serializes_to_frontend_contract() {
        let status = LiveStatus {
            running: true,
            phase: "listening".into(),
            error: String::new(),
            model_ready: true,
            source: "mic".into(),
            device: "麦克风".into(),
            level: 0.5,
            download: LiveDownloadState {
                state: "downloading".into(),
                done: 1,
                total: 2,
                label: "model.int8.onnx".into(),
                message: String::new(),
            },
        };
        let value = serde_json::to_value(&status).expect("状态可序列化");
        assert_eq!(value["modelReady"], serde_json::json!(true));
        assert!(value.get("model_ready").is_none(), "不能残留 snake_case");
        assert_eq!(value["download"]["state"], serde_json::json!("downloading"));
        assert_eq!(value["download"]["label"], serde_json::json!("model.int8.onnx"));

        let caption = LiveCaption {
            id: 3,
            jp: "テスト".into(),
            zh: "测试".into(),
            draft: false,
            final_: true,
            ts: 42,
        };
        let value = serde_json::to_value(&caption).expect("字幕可序列化");
        assert_eq!(value["final"], serde_json::json!(true));
        assert!(value.get("final_").is_none(), "final 字段必须就是 final");
        assert_eq!(value["draft"], serde_json::json!(false));
        assert_eq!(value["ts"], serde_json::json!(42));

        let devices = LiveDevices {
            loopback: vec!["扬声器".into()],
            mic: vec!["麦克风".into()],
        };
        let value = serde_json::to_value(&devices).expect("设备可序列化");
        assert_eq!(value["loopback"][0], serde_json::json!("扬声器"));
        assert_eq!(value["mic"][0], serde_json::json!("麦克风"));
    }

    #[test]
    fn default_status_is_idle_and_not_running() {
        let status = LiveStatus::default();
        assert!(!status.running);
        assert_eq!(status.phase, "idle");
        assert_eq!(status.download.state, "idle");
    }

    /// `now_millis` 是毫秒（字幕 ts 前端直接做日期运算，不能是秒）。
    #[test]
    fn now_millis_is_millisecond_scale() {
        let now = now_millis();
        assert!(now > 1_700_000_000_000, "毫秒时间戳应大于 2023 年");
    }

    /// P2-1：状态推送必须校验会话 epoch——stop 取走会话后旧会话立即过期，
    /// 迟到的 listening / level 推送不得回写。
    #[test]
    fn epoch_guard_discards_stale_session() {
        let mut state = LiveState::default();
        assert!(!session_is_current(&state, 0), "没有活动会话时任何 epoch 都过期");

        state.epoch = 2;
        state.session = Some(Session {
            epoch: 2,
            stop: Arc::new(AtomicBool::new(false)),
            audio: None,
            pipeline: None,
        });
        assert!(session_is_current(&state, 2), "当前会话的 epoch 应通过校验");
        assert!(!session_is_current(&state, 1), "上一个会话的 epoch 必须被丢弃");

        // stop() 语义：会话被取走后旧 epoch 立即过期（即使 LiveState.epoch 没变）。
        state.session = None;
        assert!(!session_is_current(&state, 2), "会话被取走后不得再改运行态");
    }
}
