//! 会议录音（工具箱「会议」子页）：cpal 采集麦克风 → 本机 WAV + JSON 侧车。
//!
//! 采集层：`cpal`（Windows WASAPI / macOS CoreAudio 同一套代码），**不依赖
//! ffmpeg**——会议页在没下载 ffmpeg 的机器上完整可用。目标格式 16kHz /
//! 16bit / 单声道；设备不支持 16k 时用设备自己的采样率（实际值记入侧车
//! `sample_rate`），声道一律降混成单声道。
//!
//! 落盘（`~/.agenthud/meetings/`）：
//! - `{YYYYMMDD-HHMM}.wav`：PCM16 单声道。录制期间写的是 `.wav.tmp` 临时
//! - 录制期间写的是 `.wav.tmp` 临时文件（44 字节占位头 + 追加数据），stop 时
//!   回填真实长度、按设置做一次峰值归一化（自动音量），再 rename 成正
//!   式名——进程被杀只会留下 `.tmp` 残留（下次 start/list 顺带清理），
//!   **不会产生 0 字节正式文件**。
//! - `{YYYYMMDD-HHMM}.json` 侧车：`{ started_at, duration_ms, device,
//!   sample_rate, marks: [{ms, label}] }`。`duration_ms` 按实际写入的采样
//!   数计算（暂停期间 cpal 流不出数据，天然扣除暂停段）；`marks` 的 ms 基
//!   于有效时间轴（`SegClock`，暂停不计入）。侧车读取带 `#[serde(default)]`
//!   兼容旧文件缺字段。
//!
//! 交互约束：
//! - 暂停 / 继续走 cpal 流的 pause / play；
//! - 全局会话状态 `SESSION: Mutex<Option<LiveSession>>`（同截图录制条模式）；
//! - 设备被占用 / 拔出、磁盘写失败均给中文报错（带设备名 / 文件路径）；
//! - 转写明确不做（前端置灰占位），本模块不引入任何模型 / API 依赖；
//! - 播放走 Tauri asset 协议（`meeting_audio` 把文件动态放行，前端
//!   `convertFileSrc` 出 URL 交给 `<audio>`），不复制 / 不转码。
//!
//! 命令（中文错误 `Result<_, String>`）：`meeting_devices` / `meeting_start`
//! / `meeting_pause` / `meeting_resume` / `meeting_stop` / `meeting_add_mark`
//! / `meeting_state` / `meeting_list` / `meeting_delete` / `meeting_rename`
//! / `meeting_open_folder` / `meeting_privacy` / `meeting_confirm_privacy`
//! / `meeting_audio` / `meeting_settings` / `meeting_set_auto_gain`。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

/// 会议录音目录：`~/.agenthud/meetings/`（复用应用统一数据目录）
fn meetings_dir() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join("meetings"))
}

// ============================================================
// wire 类型（snake_case，与工具箱其余契约一致）
// ============================================================

/// 音频输入设备（下拉用）。cpal 没有跨平台稳定设备 id，id 取设备名。
#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

/// 时间标记：ms 为有效时间轴（扣除暂停段）上的位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingMark {
    pub ms: u64,
    pub label: String,
}

/// 开始录音后返回的会话信息
#[derive(Debug, Clone, Serialize)]
pub struct MeetingSessionInfo {
    /// 文件名主干（不含扩展名），同时是列表条目 id
    pub id: String,
    pub name: String,
    /// 正式 wav 路径（录制中实际写的是 `.wav.tmp`）
    pub path: String,
    pub dir: String,
    /// 开始时刻（UTC 毫秒时间戳）
    pub started_at: i64,
    pub device: String,
    /// 实际采样率（设备不支持 16k 时是设备默认值）
    pub sample_rate: u32,
}

/// 录制中的会话快照（页面重挂载后恢复录制视图用）
#[derive(Debug, Clone, Serialize)]
pub struct MeetingSessionBrief {
    pub id: String,
    pub name: String,
    pub device: String,
    pub sample_rate: u32,
    pub started_at: i64,
    pub paused: bool,
    /// 有效录音时长（毫秒，扣除暂停段）
    pub effective_ms: u64,
    pub marks: Vec<MeetingMark>,
    pub dir: String,
}

/// 结束录音的结果
#[derive(Debug, Clone, Serialize)]
pub struct MeetingStopResult {
    pub id: String,
    pub name: String,
    pub path: String,
    pub duration_ms: u64,
    /// wav 文件字节数（含 44 字节头）
    pub size: u64,
    pub marks: usize,
    pub sample_rate: u32,
    /// 非 None = 文件已保存，但录制期间发生过写入告警（写失败后回调持续
    /// 重试成功/或部分数据缺失）。前端据此 toast 告警并正常进列表；None =
    /// 全程无告警。真正保存失败走 Err，不走这个字段。
    pub warning: Option<String>,
}

/// 列表条目（最近录音）
#[derive(Debug, Clone, Serialize)]
pub struct MeetingItem {
    pub id: String,
    pub name: String,
    pub started_at: i64,
    pub duration_ms: u64,
    pub size: u64,
    pub sample_rate: u32,
    pub device: String,
    pub marks: Vec<MeetingMark>,
}

/// 首次使用隐私确认状态 + 保存目录（前端弹确认框展示路径用）
#[derive(Debug, Clone, Serialize)]
pub struct MeetingPrivacy {
    pub confirmed: bool,
    pub dir: String,
}

/// 会议录音设置（island.db 单行 payload，camelCase 与 ShotSettings 同模式）。
/// 字段一律带 default：老版本只存了 `privacyConfirmed`，新增字段按默认值补齐。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MeetingSettings {
    /// 用户已确认「录音会保存到本机磁盘」
    pub privacy_confirmed: bool,
    /// 保存时把音量归一化到目标峰值（默认开：多数麦克风默认电平偏低，
    /// 用户反馈「录出来声音小」）。手动关掉即保留原始电平。
    pub auto_gain: bool,
}

impl Default for MeetingSettings {
    fn default() -> Self {
        Self {
            privacy_confirmed: false,
            auto_gain: true,
        }
    }
}

/// JSON 侧车（与 wav 同名）。全部带 default：旧文件缺字段也能读。
/// `started_at` 同样不能少——手工损坏的侧车（缺该字段）若整体解析失败，
/// 时长会退化成 wav 头兜底、标记清零。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MeetingSidecar {
    #[serde(default)]
    started_at: i64,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    device: String,
    #[serde(default)]
    sample_rate: u32,
    #[serde(default)]
    marks: Vec<MeetingMark>,
}

// ============================================================
// 纯逻辑：WAV 头 / 降混 / 有效时间轴 / 文件名
// ============================================================

/// 44 字节标准 PCM16 WAV 头（RIFF + fmt + data）。
fn wav_header(sample_rate: u32, channels: u16, data_len: u64) -> [u8; 44] {
    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels * 2;
    // 长度上限理论上达不到（u32 上限 ≈ 6 小时 16k 单声道）；超限截断保头合法
    let data_len32 = data_len.min(u32::MAX as u64) as u32;
    let riff_len = 36u32.saturating_add(data_len32);
    let mut b = [0u8; 44];
    b[0..4].copy_from_slice(b"RIFF");
    b[4..8].copy_from_slice(&riff_len.to_le_bytes());
    b[8..12].copy_from_slice(b"WAVE");
    b[12..16].copy_from_slice(b"fmt ");
    b[16..20].copy_from_slice(&16u32.to_le_bytes());
    b[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    b[22..24].copy_from_slice(&channels.to_le_bytes());
    b[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    b[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    b[32..34].copy_from_slice(&block_align.to_le_bytes());
    b[34..36].copy_from_slice(&16u16.to_le_bytes()); // bits per sample
    b[36..40].copy_from_slice(b"data");
    b[40..44].copy_from_slice(&data_len32.to_le_bytes());
    b
}

/// 解析本模块写出的 44 字节规范头 → (采样率, 声道数, data 长度)。
/// 只识别自己写的布局（侧车缺失时的兜底，外来 wav 直接返回 None）。
fn parse_wav_header(b: &[u8]) -> Option<(u32, u16, u64)> {
    if b.len() < 44 {
        return None;
    }
    if b[0..4] != *b"RIFF" || b[8..12] != *b"WAVE" || b[12..16] != *b"fmt " || b[36..40] != *b"data"
    {
        return None;
    }
    let fmt_code = u16::from_le_bytes([b[20], b[21]]);
    if fmt_code != 1 {
        return None;
    }
    let channels = u16::from_le_bytes([b[22], b[23]]);
    let rate = u32::from_le_bytes([b[24], b[25], b[26], b[27]]);
    let data_len = u32::from_le_bytes([b[40], b[41], b[42], b[43]]) as u64;
    if rate == 0 || channels == 0 {
        return None;
    }
    Some((rate, channels, data_len))
}

fn f32_to_pcm16(v: f32) -> i16 {
    (v * 32_767.0).clamp(-32_768.0, 32_767.0).round() as i16
}

/// 交错多声道采样 → 单声道 PCM16（多声道取平均）。channels < 1 按 1 处理。
fn downmix_to_mono_i16<T: Copy>(data: &[T], channels: usize, to_f32: fn(T) -> f32) -> Vec<i16> {
    let channels = channels.max(1);
    let mut out = Vec::with_capacity(data.len() / channels);
    if channels == 1 {
        for &s in data {
            out.push(f32_to_pcm16(to_f32(s)));
        }
    } else {
        for frame in data.chunks_exact(channels) {
            let sum: f32 = frame.iter().map(|&s| to_f32(s)).sum();
            out.push(f32_to_pcm16(sum / channels as f32));
        }
    }
    out
}

/// 分段时钟：暂停期间不计入有效时间轴。全部方法注入 `now_ms`，纯逻辑可单测。
#[derive(Debug, Default, Clone)]
struct SegClock {
    /// 当前未结算分段起点（None = 暂停中 / 未开始）
    seg_start_ms: Option<u64>,
    accumulated_ms: u64,
}

impl SegClock {
    fn pause(&mut self, now_ms: u64) {
        if let Some(start) = self.seg_start_ms.take() {
            self.accumulated_ms += now_ms.saturating_sub(start);
        }
    }

    fn resume(&mut self, now_ms: u64) {
        if self.seg_start_ms.is_none() {
            self.seg_start_ms = Some(now_ms);
        }
    }

    fn effective_ms(&self, now_ms: u64) -> u64 {
        self.accumulated_ms
            + self
                .seg_start_ms
                .map(|start| now_ms.saturating_sub(start))
                .unwrap_or(0)
    }
}

/// 录音文件名主干：`YYYYMMDD-HHMM`（本地时间）
fn meeting_file_stem(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("%Y%m%d-%H%M").to_string()
}

/// 从文件名主干反推开始时刻（兜底侧车缺失）。`-2` 等同分钟重名后缀忽略。
fn stem_to_started_at(stem: &str) -> Option<i64> {
    use chrono::TimeZone;
    let mut parts = stem.split('-');
    let date = parts.next()?;
    let time = parts.next()?;
    if date.len() != 8 || time.len() != 4 {
        return None;
    }
    let year = date[0..4].parse::<i32>().ok()?;
    let month = date[4..6].parse::<u32>().ok()?;
    let day = date[6..8].parse::<u32>().ok()?;
    let hour = time[0..2].parse::<u32>().ok()?;
    let minute = time[2..4].parse::<u32>().ok()?;
    chrono::Local
        .with_ymd_and_hms(year, month, day, hour, minute, 0)
        .single()
        .map(|t| t.timestamp_millis())
}

/// 列表 / 重命名可用的名称规则：非空、无路径分隔与 Windows 非法字符。
fn sanitize_meeting_name(raw: &str) -> Result<String, String> {
    const MAX_CHARS: usize = 100;
    let name = raw.trim();
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    if name == "." || name == ".." {
        return Err("名称不能是 . 或 ..".into());
    }
    if name.chars().count() > MAX_CHARS {
        return Err(format!("名称太长（上限 {MAX_CHARS} 字）"));
    }
    let forbidden = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    if let Some(bad) = name.chars().find(|c| c.is_control() || forbidden.contains(c)) {
        return Err(format!("名称不能包含字符 {bad:?}"));
    }
    Ok(name.to_string())
}

/// 同一分钟录第二条起加 `-2` / `-3` 后缀，绝不覆盖已有录音。`.wav.tmp` 同样
/// 要避让：上一条可能正处于 stop 收尾窗口（finalize→rename 期间会话槽已清），
/// 新会话若复用同一主干，File::create 会把正在保存的数据截断（stop→start
/// 快速连击竞态）。
fn unique_stem(dir: &Path, base: &str) -> String {
    let occupied = |stem: &str| {
        dir.join(format!("{stem}.wav")).exists() || dir.join(format!("{stem}.wav.tmp")).exists()
    };
    if !occupied(base) {
        return base.to_string();
    }
    for n in 2..100 {
        let candidate = format!("{base}-{n}");
        if !occupied(&candidate) {
            return candidate;
        }
    }
    format!("{base}-{}", chrono::Utc::now().timestamp_millis())
}

// ============================================================
// 录制会话（全局单例，照截图录制的全局状态模式）
// ============================================================

static SESSION: OnceLock<Mutex<Option<LiveSession>>> = OnceLock::new();

fn session_slot() -> &'static Mutex<Option<LiveSession>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

fn session_lock() -> MutexGuard<'static, Option<LiveSession>> {
    // 音频回调 / 命令路径都不 panic，锁中毒只可能来自外部 bug：恢复内部数据继续用
    session_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_or_recover<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// wav 写入器：录制期间 BufWriter 追加 PCM16 数据，stop 时 flush 后回填头。
struct Recorder {
    file: std::io::BufWriter<std::fs::File>,
    sample_rate: u32,
    /// 恒为 1：音频回调在写入前已把多声道降混成单声道，WAV 头的声道数必须
    /// 与实际数据一致。若误填设备原始声道数（如 2），播放器会按双声道解读
    /// 单声道数据 → 时长翻倍、音调低八度。
    channels: u16,
    bytes_written: u64,
}

impl Recorder {
    /// `device_channels` 仅作语义说明：无论设备几声道，落盘前已降混为单声道，
    /// 头里恒写 1。
    fn create(file: std::fs::File, sample_rate: u32, device_channels: u16) -> Self {
        let _ = device_channels;
        Self {
            file: std::io::BufWriter::new(file),
            sample_rate,
            channels: 1,
            bytes_written: 0,
        }
    }

    fn write_placeholder_header(&mut self) -> Result<(), String> {
        let header = wav_header(self.sample_rate, self.channels, 0);
        self.file
            .write_all(&header)
            .map_err(|e| format!("写入录音临时文件失败: {e}"))?;
        self.file
            .flush()
            .map_err(|e| format!("写入录音临时文件失败: {e}"))
    }

    /// 收尾：冲刷缓冲、回填真实 data 长度并强制落盘。之后才允许 rename。
    fn finalize(&mut self) -> Result<(), String> {
        self.file
            .flush()
            .map_err(|e| format!("写入录音文件失败: {e}"))?;
        let file = self.file.get_mut();
        file.seek(SeekFrom::Start(0))
            .map_err(|e| format!("回写录音文件头失败: {e}"))?;
        let header = wav_header(self.sample_rate, self.channels, self.bytes_written);
        file.write_all(&header)
            .map_err(|e| format!("回写录音文件头失败: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("保存录音文件失败: {e}"))
    }
}

/// 自动音量：整段峰值归一化到约 -2 dBFS，返回实际增益（1.0 = 没动）。
///
/// 为什么在保存时做而不是采集时乘固定增益：采集时不知道整段的峰值，固定
/// 增益要么不够响（安静麦克风），要么削波（大声 / 靠近麦克风）。保存前扫
/// 一遍真实峰值再乘，既明显变响又不会削波。
///
/// - 原地重写 data 段：PCM16 每个采样长度不变，WAV 头与时长都不用改；
/// - 只放大不衰减（已足够响的录音原样保留）；增益上限 +18 dB；
/// - 峰值低于约 -48 dBFS 视为静音（没说话 / 设备没采到），不去放大底噪；
/// - 失败向上返回错误，调用方保留原始录音，不让归一化影响「文件已保存」。
fn normalize_wav_peak(path: &Path) -> Result<f32, String> {
    /// 目标峰值 ≈ -2 dBFS（给播放器留一点余量，避免解码 / 音量联动时削波）
    const TARGET_PEAK: f32 = 32_767.0 * 0.794;
    /// 增益上限 +18 dB：再高就是把底噪抬成噪音了
    const MAX_GAIN: f32 = 8.0;
    /// 基本静音的阈值（≈ -48 dBFS）
    const SILENT_PEAK: i32 = 120;

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("打开录音文件失败: {e}"))?;
    let len = file
        .metadata()
        .map_err(|e| format!("读取录音文件大小失败: {e}"))?
        .len();
    // 跳过 44 字节头，只处理完整的 i16 采样（奇数字节不动）
    let data_len = len.saturating_sub(44) & !1;
    if data_len == 0 {
        return Ok(1.0);
    }

    let mut buffer = vec![0u8; 64 * 1024];
    let mut remaining = data_len;
    let mut peak = 0i32;
    file.seek(SeekFrom::Start(44))
        .map_err(|e| format!("读取录音文件失败: {e}"))?;
    while remaining > 0 {
        let want = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..want])
            .map_err(|e| format!("读取录音文件失败: {e}"))?;
        for sample in buffer[..want].chunks_exact(2) {
            let value = i16::from_le_bytes([sample[0], sample[1]]) as i32;
            peak = peak.max(value.abs());
        }
        remaining -= want as u64;
    }

    if peak < SILENT_PEAK {
        return Ok(1.0);
    }
    let gain = (TARGET_PEAK / peak as f32).clamp(1.0, MAX_GAIN);
    if gain < 1.02 {
        return Ok(1.0);
    }

    let mut offset = 44u64;
    remaining = data_len;
    while remaining > 0 {
        let want = remaining.min(buffer.len() as u64) as usize;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("读取录音文件失败: {e}"))?;
        file.read_exact(&mut buffer[..want])
            .map_err(|e| format!("读取录音文件失败: {e}"))?;
        for sample in buffer[..want].chunks_exact_mut(2) {
            let value = i16::from_le_bytes([sample[0], sample[1]]) as f32 * gain;
            let adjusted = value.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            sample.copy_from_slice(&adjusted.to_le_bytes());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("写入录音文件失败: {e}"))?;
        file.write_all(&buffer[..want])
            .map_err(|e| format!("写入录音文件失败: {e}"))?;
        offset += want as u64;
        remaining -= want as u64;
    }
    file.sync_all()
        .map_err(|e| format!("保存录音文件失败: {e}"))?;
    Ok(gain)
}

/// 音频回调与命令共享的录制上下文。字段各自加锁，避免音频线程与 UI 命令
/// 互相顶住（回调 10ms 一次，必须无阻塞）。
struct RecordCtx {
    recorder: Mutex<Recorder>,
    /// 实际写入的单声道采样数（暂停期间流不出数据 → 时长天然扣除暂停）
    samples: AtomicU64,
    /// 有效时间轴（marks 用，暂停不计入）
    clock: Mutex<SegClock>,
    marks: Mutex<Vec<MeetingMark>>,
    last_error: Mutex<Option<String>>,
    /// true = 已暂停（cpal 流 pause 之后）
    paused: AtomicBool,
    /// 墙钟锚点：SegClock 的 now_ms 都从这里换算
    anchor: Instant,
    sample_rate: u32,
    tmp_path: PathBuf,
    wav_path: PathBuf,
    json_path: PathBuf,
    dir: PathBuf,
}

/// cpal 的 `Stream` 刻意 `!Send`（`NotSendSyncAcrossAllPlatforms`），不能跨线程
/// 存进全局 Mutex。这里用「专职线程持有流」模式：流在 keeper 线程里创建、
/// 播放、暂停、继续、丢弃，命令侧通过控制通道与它交互，等待单次回执。
enum ControlMsg {
    Pause(mpsc::Sender<Result<(), String>>),
    Resume(mpsc::Sender<Result<(), String>>),
    /// 停流并结束 keeper 线程（文件收尾由命令侧在拿到回执后完成）
    Stop(mpsc::Sender<Result<(), String>>),
}

struct LiveSession {
    ctx: Arc<RecordCtx>,
    /// 与 keeper 线程的控制通道；全部 sender 被丢弃时线程自行停流退出
    control: mpsc::Sender<ControlMsg>,
    id: String,
    device: String,
    started_at: i64,
}

/// 发一条控制消息并等 keeper 线程回执（流操作在它自己的线程上执行）
fn send_control(
    control: &mpsc::Sender<ControlMsg>,
    make: impl FnOnce(mpsc::Sender<Result<(), String>>) -> ControlMsg,
) -> Result<(), String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    control
        .send(make(reply_tx))
        .map_err(|_| "录音设备线程已退出".to_string())?;
    reply_rx
        .recv()
        .map_err(|_| "录音设备线程已退出".to_string())?
}

/// keeper 线程主体：建流 → play → 循环处理控制消息 → Stop 时在**本线程**
/// drop Stream（满足 cpal 的线程约束）。
fn keep_stream_thread(
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    ctx: Arc<RecordCtx>,
    device_name: String,
    start_reply: mpsc::Sender<Result<(), String>>,
    control_rx: mpsc::Receiver<ControlMsg>,
) {
    let stream = match build_input_stream_for(&device, &config, Arc::clone(&ctx)) {
        Ok(stream) => stream,
        Err(e) => {
            let _ = start_reply.send(Err(format!("设备「{device_name}」无法开始录音: {e}")));
            return;
        }
    };
    if let Err(e) = stream.play() {
        drop(stream);
        let _ = start_reply.send(Err(format!("启动录音失败（设备可能被其他应用占用）: {e}")));
        return;
    }
    let _ = start_reply.send(Ok(()));
    for msg in control_rx {
        match msg {
            ControlMsg::Pause(reply) => {
                let _ = reply.send(stream.pause().map_err(|e| format!("暂停录音失败: {e}")));
            }
            ControlMsg::Resume(reply) => {
                let _ = reply.send(stream.play().map_err(|e| format!("继续录音失败: {e}")));
            }
            ControlMsg::Stop(reply) => {
                drop(stream);
                let _ = reply.send(Ok(()));
                return;
            }
        }
    }
    // 所有 sender 已被丢弃（会话被异常清理）：Stream 随作用域结束停流
}

fn now_ms(ctx: &RecordCtx) -> u64 {
    ctx.anchor.elapsed().as_millis() as u64
}

fn set_stream_error(ctx: &RecordCtx, message: String) {
    let mut slot = lock_or_recover(&ctx.last_error);
    // 只保留第一条（后续回调大概率是同一根因刷屏）
    if slot.is_none() {
        *slot = Some(message);
    }
}

fn check_stream_error(ctx: &RecordCtx) -> Result<(), String> {
    match lock_or_recover(&ctx.last_error).clone() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// 音频回调入口：降混成单声道 PCM16 后追加写入临时文件。
fn ingest_mono(ctx: &RecordCtx, mono: &[i16]) {
    let mut bytes = Vec::with_capacity(mono.len() * 2);
    for s in mono {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let mut rec = lock_or_recover(&ctx.recorder);
    match rec.file.write_all(&bytes) {
        Ok(()) => {
            rec.bytes_written += bytes.len() as u64;
            ctx.samples.fetch_add(mono.len() as u64, Ordering::Relaxed);
        }
        Err(e) => {
            set_stream_error(
                ctx,
                format!("写入录音文件失败（{}）: {e}", ctx.tmp_path.display()),
            );
        }
    }
}

/// 按采样格式挑回调分支（cpal 0.15 的 build_input_stream 按具体 T 单态化；
/// 覆盖常见输入格式，其余给中文报错而不是静默失败）。
fn build_input_stream_for(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    ctx: Arc<RecordCtx>,
) -> Result<cpal::Stream, String> {
    let channels = config.channels() as usize;
    match config.sample_format() {
        cpal::SampleFormat::F32 => {
            build_stream_typed::<f32>(device, config, ctx, channels, |s: f32| s)
        }
        cpal::SampleFormat::I16 => {
            build_stream_typed::<i16>(device, config, ctx, channels, |s: i16| s as f32 / 32_768.0)
        }
        cpal::SampleFormat::U8 => build_stream_typed::<u8>(
            device,
            config,
            ctx,
            channels,
            |s: u8| (s as f32 - 128.0) / 128.0,
        ),
        other => Err(format!(
            "设备采样格式暂不支持（{other:?}），请换一个输入设备"
        )),
    }
}

fn build_stream_typed<T: cpal::SizedSample + 'static>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    ctx: Arc<RecordCtx>,
    channels: usize,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream, String> {
    // cpal 0.15 的 build_input_stream 收裸 StreamConfig（SupportedStreamConfig 可 into）
    let stream_config: cpal::StreamConfig = config.clone().into();
    let data_ctx = Arc::clone(&ctx);
    let err_ctx = Arc::clone(&ctx);
    let stream = device
        .build_input_stream(
            &stream_config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mono = downmix_to_mono_i16(data, channels, to_f32);
                ingest_mono(&data_ctx, &mono);
            },
            move |err: cpal::StreamError| {
                set_stream_error(&err_ctx, format!("录音设备错误: {err}"));
            },
            None,
        )
        .map_err(|e| format!("打开输入设备失败（可能被其他应用占用）: {e}"))?;
    Ok(stream)
}

/// 选输入配置：目标 16kHz；在设备支持的区间里优先「单声道 + 默认采样格式」，
/// 其次单声道 / 默认格式；都不满足 16k 时用设备默认配置（实际值记入侧车）。
fn pick_input_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
    let dev_name = device.name().unwrap_or_else(|_| "未知设备".into());
    let default = device
        .default_input_config()
        .map_err(|e| format!("读取设备「{dev_name}」参数失败: {e}"))?;
    let target = cpal::SampleRate(16_000);
    let mut best: Option<cpal::SupportedStreamConfig> = None;
    let mut best_score = (-1i32, -1i32);
    if let Ok(ranges) = device.supported_input_configs() {
        for range in ranges {
            if !(range.min_sample_rate()..=range.max_sample_rate()).contains(&target) {
                continue;
            }
            let score = ((range.channels() == 1) as i32,
                (range.sample_format() == default.sample_format()) as i32);
            if best.is_none() || score > best_score {
                best = Some(range.with_sample_rate(target));
                best_score = score;
            }
        }
    }
    Ok(best.unwrap_or(default))
}

fn resolve_input_device(name: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    let want = name.map(str::trim).filter(|s| !s.is_empty());
    match want {
        None => host
            .default_input_device()
            .ok_or_else(|| "未检测到默认音频输入设备，请检查麦克风连接".to_string()),
        Some(name) => {
            let devices = host
                .input_devices()
                .map_err(|e| format!("枚举音频输入设备失败: {e}"))?;
            for device in devices {
                if device.name().ok().as_deref() == Some(name) {
                    return Ok(device);
                }
            }
            Err(format!(
                "找不到输入设备「{name}」，可能已被拔出、禁用或被其他应用占用"
            ))
        }
    }
}

/// tmp 残留的年龄阈值：stop 收尾（finalize→rename）期间新产生的 tmp 是
/// 正在保存的录音，绝不能当残留删（stop→start 快速连击竞态）；只清理修改
/// 时间早于该阈值的文件。
const STALE_TMP_MAX_AGE_MS: i64 = 60_000;

/// 纯函数：tmp 修改时间早于阈值才视为上次进程被杀的残留（注入时钟可单测）。
fn stale_tmp_deletable(modified_ms: i64, now_ms: i64) -> bool {
    modified_ms >= 0 && now_ms - modified_ms >= STALE_TMP_MAX_AGE_MS
}

/// 清理上次进程被杀留下的临时文件（正式文件只在 rename 后出现，不会被误删）。
/// 双重保护：① 会话槽被占用（录制中）时不动目录；② 只删年龄超过阈值的
/// tmp——stop 收尾期间新产生的 tmp 靠年龄躲过清理。
fn cleanup_stale_tmp() {
    if session_lock().is_some() {
        return;
    }
    let Ok(dir) = meetings_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !(name.ends_with(".wav.tmp") || name.ends_with(".json.tmp")) {
            continue;
        }
        let deletable = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| stale_tmp_deletable(systemtime_ms(t), now_ms))
            .unwrap_or(false);
        if deletable {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn start_session(device: Option<&str>) -> Result<MeetingSessionInfo, String> {
    cleanup_stale_tmp();
    let mut guard = session_lock();
    if guard.is_some() {
        return Err("已有会议录音正在进行，请先结束当前录音".into());
    }

    let device = resolve_input_device(device)?;
    let device_name = device.name().unwrap_or_else(|_| "未知设备".into());
    let config = pick_input_config(&device)?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();

    let dir = meetings_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("创建会议录音目录失败（{}）: {e}", dir.display()))?;
    let stem = unique_stem(&dir, &meeting_file_stem(chrono::Local::now()));
    let wav_path = dir.join(format!("{stem}.wav"));
    let tmp_path = dir.join(format!("{stem}.wav.tmp"));
    let json_path = dir.join(format!("{stem}.json"));

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|e| format!("创建录音临时文件失败（{}）: {e}", tmp_path.display()))?;
    // 无论设备几声道，回调已降混单声道：Recorder 头里恒写 1 声道
    let mut recorder = Recorder::create(file, sample_rate, channels);
    if let Err(e) = recorder.write_placeholder_header() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("{e}（{}）", tmp_path.display()));
    }

    let ctx = Arc::new(RecordCtx {
        recorder: Mutex::new(recorder),
        samples: AtomicU64::new(0),
        clock: Mutex::new(SegClock {
            seg_start_ms: Some(0),
            accumulated_ms: 0,
        }),
        marks: Mutex::new(Vec::new()),
        last_error: Mutex::new(None),
        paused: AtomicBool::new(false),
        anchor: Instant::now(),
        sample_rate,
        tmp_path: tmp_path.clone(),
        wav_path: wav_path.clone(),
        json_path: json_path.clone(),
        dir: dir.clone(),
    });

    // Stream 由 keeper 线程创建并持有（cpal Stream 刻意 !Send，不能跨线程）
    let (control_tx, control_rx) = mpsc::channel();
    let (start_tx, start_rx) = mpsc::channel();
    let ctx_for_thread = Arc::clone(&ctx);
    let device_for_thread = device;
    let config_for_thread = config;
    let name_for_thread = device_name.clone();
    std::thread::Builder::new()
        .name("agenthud-meeting-rec".into())
        .spawn(move || {
            keep_stream_thread(
                device_for_thread,
                config_for_thread,
                ctx_for_thread,
                name_for_thread,
                start_tx,
                control_rx,
            )
        })
        .map_err(|e| format!("启动录音线程失败: {e}"))?;

    // 等流真正建起来（失败时清理临时文件，不留下半个会话）
    if let Err(e) = start_rx
        .recv()
        .map_err(|_| "录音线程意外退出".to_string())
        .flatten()
    {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    let started_at = chrono::Utc::now().timestamp_millis();
    let info = MeetingSessionInfo {
        id: stem.clone(),
        name: stem.clone(),
        path: wav_path.display().to_string(),
        dir: dir.display().to_string(),
        started_at,
        device: device_name.clone(),
        sample_rate,
    };
    *guard = Some(LiveSession {
        ctx,
        control: control_tx,
        id: stem,
        device: device_name,
        started_at,
    });
    Ok(info)
}

fn pause_session() -> Result<(), String> {
    // 第一段：取控制通道并等 keeper 回执（不持全局锁，避免顶住 mark/state）
    let control = {
        let guard = session_lock();
        let sess = guard.as_ref().ok_or("当前没有正在进行的会议录音")?;
        check_stream_error(&sess.ctx)?;
        sess.control.clone()
    };
    send_control(&control, ControlMsg::Pause)?;
    // 第二段：流暂停成功后结算时间轴、更新标志
    let guard = session_lock();
    if let Some(sess) = guard.as_ref() {
        lock_or_recover(&sess.ctx.clock).pause(now_ms(&sess.ctx));
        sess.ctx.paused.store(true, Ordering::Relaxed);
    }
    Ok(())
}

fn resume_session() -> Result<(), String> {
    let control = {
        let guard = session_lock();
        let sess = guard.as_ref().ok_or("当前没有正在进行的会议录音")?;
        check_stream_error(&sess.ctx)?;
        sess.control.clone()
    };
    send_control(&control, ControlMsg::Resume)?;
    let guard = session_lock();
    if let Some(sess) = guard.as_ref() {
        lock_or_recover(&sess.ctx.clock).resume(now_ms(&sess.ctx));
        sess.ctx.paused.store(false, Ordering::Relaxed);
    }
    Ok(())
}

fn add_mark_to_session(label: Option<String>) -> Result<MeetingMark, String> {
    let guard = session_lock();
    let sess = guard.as_ref().ok_or("当前没有正在进行的会议录音")?;
    check_stream_error(&sess.ctx)?;
    let label = match label {
        Some(raw) => raw.trim().chars().take(60).collect::<String>(),
        None => String::new(),
    };
    let ms = {
        let clock = lock_or_recover(&sess.ctx.clock);
        clock.effective_ms(now_ms(&sess.ctx))
    };
    let mark = MeetingMark { ms, label };
    lock_or_recover(&sess.ctx.marks).push(mark.clone());
    Ok(mark)
}

fn session_state() -> Option<MeetingSessionBrief> {
    let guard = session_lock();
    let sess = guard.as_ref()?;
    let ctx = &sess.ctx;
    // 尾表达式里的临时 MutexGuard 会延长守卫借用（E0597），先落成局部值
    let effective_ms = lock_or_recover(&ctx.clock).effective_ms(now_ms(ctx));
    let marks = lock_or_recover(&ctx.marks).clone();
    let dir = ctx.dir.display().to_string();
    Some(MeetingSessionBrief {
        id: sess.id.clone(),
        name: sess.id.clone(),
        device: sess.device.clone(),
        sample_rate: ctx.sample_rate,
        started_at: sess.started_at,
        paused: ctx.paused.load(Ordering::Relaxed),
        effective_ms,
        marks,
        dir,
    })
}

fn stop_session() -> Result<MeetingStopResult, String> {
    let sess = session_lock().take().ok_or("当前没有正在进行的会议录音")?;
    // 让 keeper 线程在本线程停流（cpal Stream 不能跨线程 drop）；
    // 通道失败（线程已退）也继续收尾文件，尽力保留已写数据
    let _ = send_control(&sess.control, ControlMsg::Stop);
    let ctx = &sess.ctx;
    let write_error = check_stream_error(ctx).err();
    let samples = ctx.samples.load(Ordering::Relaxed);
    let sample_rate = lock_or_recover(&ctx.recorder).sample_rate;

    // 回填真实长度；写失败（磁盘满等）保留 tmp 并报路径，不产出残缺正式文件
    if let Err(e) = lock_or_recover(&ctx.recorder).finalize() {
        return Err(format!("{e}（{}）", ctx.tmp_path.display()));
    }
    // 自动音量：按整段峰值归一化（设置里默认开）。失败只记日志——文件本身
    // 已经完整，不能因为「想让它更响」而让保存失败。
    if crate::island::store::load_meeting_settings().auto_gain {
        match normalize_wav_peak(&ctx.tmp_path) {
            Ok(gain) if gain > 1.02 => {
                eprintln!("[meeting] 自动音量 +{:.1} dB", 20.0 * gain.log10());
            }
            Ok(_) => {}
            Err(e) => eprintln!("[meeting] 自动音量失败，保留原始录音: {e}"),
        }
    }
    let duration_ms = if sample_rate > 0 {
        samples * 1000 / sample_rate as u64
    } else {
        0
    };
    std::fs::rename(&ctx.tmp_path, &ctx.wav_path).map_err(|e| {
        format!(
            "保存录音文件失败（{} → {}）: {e}",
            ctx.tmp_path.display(),
            ctx.wav_path.display()
        )
    })?;
    let size = std::fs::metadata(&ctx.wav_path)
        .map(|m| m.len())
        .unwrap_or(0);

    let marks = lock_or_recover(&ctx.marks).clone();
    let sidecar = MeetingSidecar {
        started_at: sess.started_at,
        duration_ms,
        device: sess.device.clone(),
        sample_rate,
        marks: marks.clone(),
    };
    let text = serde_json::to_string_pretty(&sidecar)
        .map_err(|e| format!("序列化录音信息失败: {e}"))?;
    crate::provider_credentials::atomic_write(&ctx.json_path, &text)
        .map_err(|e| format!("保存录音信息失败（{}）: {e}", ctx.json_path.display()))?;

    // 录制期间的写入告警放在 warning 里随 Ok 返回（文件已收尾可播）：
    // 前端 toast 告警并正常进列表；只有上面 finalize/rename/侧车真正失败
    // 才走 Err。前端对 Err 也会退出录制视图（会话槽已清，流已停）。
    Ok(MeetingStopResult {
        id: sess.id.clone(),
        name: sess.id.clone(),
        path: ctx.wav_path.display().to_string(),
        duration_ms,
        size,
        marks: marks.len(),
        sample_rate,
        warning: write_error.map(|err| {
            format!("录音期间写入文件失败，已保留当前文件（{}）：{err}", ctx.wav_path.display())
        }),
    })
}

// ============================================================
// 列表 / 管理
// ============================================================

fn systemtime_ms(t: std::time::SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 侧车缺失时从 wav 头兜底推时长（只读前 44 字节，不整读文件）
fn wav_duration_from_header(path: &Path) -> Option<(u32, u64)> {
    let mut head = [0u8; 44];
    let mut file = std::fs::File::open(path).ok()?;
    std::io::Read::read_exact(&mut file, &mut head).ok()?;
    let (rate, channels, data_len) = parse_wav_header(&head)?;
    let byte_rate = rate as u64 * channels as u64 * 2;
    if byte_rate == 0 {
        return None;
    }
    Some((rate, data_len * 1000 / byte_rate))
}

fn build_meeting_item(dir: &Path, stem: &str, wav_path: &Path) -> MeetingItem {
    let mut item = MeetingItem {
        id: stem.to_string(),
        name: stem.to_string(),
        started_at: 0,
        duration_ms: 0,
        size: std::fs::metadata(wav_path).map(|m| m.len()).unwrap_or(0),
        sample_rate: 0,
        device: String::new(),
        marks: Vec::new(),
    };
    let json_path = dir.join(format!("{stem}.json"));
    if let Ok(raw) = std::fs::read_to_string(&json_path) {
        if let Ok(sidecar) = serde_json::from_str::<MeetingSidecar>(&raw) {
            item.started_at = sidecar.started_at;
            item.duration_ms = sidecar.duration_ms;
            item.sample_rate = sidecar.sample_rate;
            item.device = sidecar.device;
            item.marks = sidecar.marks;
        }
    }
    if item.duration_ms == 0 {
        if let Some((rate, ms)) = wav_duration_from_header(wav_path) {
            item.duration_ms = ms;
            if item.sample_rate == 0 {
                item.sample_rate = rate;
            }
        }
    }
    if item.started_at == 0 {
        item.started_at = stem_to_started_at(stem)
            .or_else(|| {
                std::fs::metadata(wav_path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(systemtime_ms)
            })
            .unwrap_or(0);
    }
    item
}

const MAX_MEETING_ITEMS: usize = 200;

fn list_meetings() -> Result<Vec<MeetingItem>, String> {
    cleanup_stale_tmp();
    let dir = meetings_dir()?;
    let mut items = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(items), // 目录还没建过 = 没有录音
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".wav") else {
            continue;
        };
        items.push(build_meeting_item(&dir, stem, &path));
    }
    items.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    items.truncate(MAX_MEETING_ITEMS);
    Ok(items)
}

/// id / 路径安全校验：列表 id 就是文件主干，防路径穿越复用同一套名称规则
fn clean_meeting_id(raw: &str) -> Result<String, String> {
    sanitize_meeting_name(raw)
}

fn delete_meeting(id: &str) -> Result<(), String> {
    let id = clean_meeting_id(id)?;
    let dir = meetings_dir()?;
    let wav = dir.join(format!("{id}.wav"));
    if !wav.exists() {
        return Err("录音不存在或已被删除".into());
    }
    std::fs::remove_file(&wav).map_err(|e| format!("删除录音文件失败: {e}"))?;
    let json = dir.join(format!("{id}.json"));
    if json.exists() {
        let _ = std::fs::remove_file(&json);
    }
    Ok(())
}

fn rename_meeting(id: &str, new_name: &str) -> Result<MeetingItem, String> {
    let id = clean_meeting_id(id)?;
    let name = sanitize_meeting_name(new_name)?;
    let dir = meetings_dir()?;
    let src_wav = dir.join(format!("{id}.wav"));
    if !src_wav.exists() {
        return Err("录音不存在或已被删除".into());
    }
    let dst_wav = dir.join(format!("{name}.wav"));
    if dst_wav.exists() {
        return Err(format!("已存在同名录音「{name}」"));
    }
    std::fs::rename(&src_wav, &dst_wav).map_err(|e| format!("重命名失败: {e}"))?;
    let src_json = dir.join(format!("{id}.json"));
    let dst_json = dir.join(format!("{name}.json"));
    if src_json.exists() && !dst_json.exists() {
        let _ = std::fs::rename(&src_json, &dst_json);
    }
    Ok(build_meeting_item(&dir, &name, &dst_wav))
}

fn open_meeting_folder(id: &str) -> Result<(), String> {
    let id = clean_meeting_id(id)?;
    let dir = meetings_dir()?;
    let wav = dir.join(format!("{id}.wav"));
    if !wav.exists() {
        return Err("录音不存在或已被删除".into());
    }
    // 复用截图目录「打开所在文件夹」的既有实现（open crate）
    open::that(&dir).map_err(|e| format!("打开会议录音目录失败: {e}"))
}

// ============================================================
// Tauri 命令
// ============================================================

/// 枚举音频输入设备（设备下拉用）。
///
/// 每次调用都是**当前设备状态的快照**：cpal 的 WASAPI 后端把 `IMMDeviceEnumerator`
/// 缓存在进程级 `OnceLock` 里，但 `input_devices()` 每次都会重新
/// `EnumAudioEndpoints(DEVICE_STATE_ACTIVE)`，所以插拔设备后再调一次就能看到
/// 变化——前端靠「4s 轮询 + 展开下拉即重扫 + 窗口聚焦重扫」实现热插拔，
/// 不需要设备变更通知。
///
/// 一台设备都没有时返回**空列表而不是错误**：前端有空态提示（「未检测到麦克风」），
/// 而 `Err` 会被当成枚举失败（驱动异常 / 权限不足）单独提示，两者语义要分开。
#[tauri::command]
pub async fn meeting_devices() -> Result<Vec<AudioDevice>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let host = cpal::default_host();
        let mut out: Vec<AudioDevice> = Vec::new();
        let devices = host
            .input_devices()
            .map_err(|e| format!("枚举音频输入设备失败: {e}"))?;
        for device in devices {
            if let Ok(name) = device.name() {
                // cpal 的 WASAPI 端点名偶尔会重复（同型号两台设备同名）；id 也用
                // 名字，重复项在下拉里只留一条，避免 React key 重复与「选哪个都一样」
                if !out.iter().any(|item| item.id == name) {
                    out.push(AudioDevice { id: name.clone(), name });
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("枚举音频输入设备任务失败: {e}"))?
}

/// 开始录音；`device` 传 None / 空串用默认输入设备
#[tauri::command]
pub async fn meeting_start(device: Option<String>) -> Result<MeetingSessionInfo, String> {
    tauri::async_runtime::spawn_blocking(move || start_session(device.as_deref()))
        .await
        .map_err(|e| format!("启动会议录音任务失败: {e}"))?
}

#[tauri::command]
pub fn meeting_pause() -> Result<(), String> {
    pause_session()
}

#[tauri::command]
pub fn meeting_resume() -> Result<(), String> {
    resume_session()
}

/// 录制中打时间标记；`label` 可选，返回打在有效时间轴上的标记
#[tauri::command]
pub fn meeting_add_mark(label: Option<String>) -> Result<MeetingMark, String> {
    add_mark_to_session(label)
}

/// 录制中的会话快照（None = 空闲）。子页重挂载后恢复录制视图用。
#[tauri::command]
pub fn meeting_state() -> Option<MeetingSessionBrief> {
    session_state()
}

#[tauri::command]
pub async fn meeting_stop() -> Result<MeetingStopResult, String> {
    tauri::async_runtime::spawn_blocking(stop_session)
        .await
        .map_err(|e| format!("结束会议录音任务失败: {e}"))?
}

/// 最近录音列表（started_at 倒序，最多 200 条；顺带清理上次残留的 .tmp）
#[tauri::command]
pub async fn meeting_list() -> Result<Vec<MeetingItem>, String> {
    tauri::async_runtime::spawn_blocking(list_meetings)
        .await
        .map_err(|e| format!("读取会议录音列表任务失败: {e}"))?
}

#[tauri::command]
pub async fn meeting_delete(id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || delete_meeting(&id))
        .await
        .map_err(|e| format!("删除会议录音任务失败: {e}"))?
}

/// 重命名（wav 与侧车 json 一起改名）；返回改名后的条目
#[tauri::command]
pub async fn meeting_rename(id: String, new_name: String) -> Result<MeetingItem, String> {
    tauri::async_runtime::spawn_blocking(move || rename_meeting(&id, &new_name))
        .await
        .map_err(|e| format!("重命名会议录音任务失败: {e}"))?
}

/// 打开该录音所在文件夹（复用 open crate，与截图目录行为一致）
#[tauri::command]
pub fn meeting_open_folder(id: String) -> Result<(), String> {
    open_meeting_folder(&id)
}

/// 首次使用隐私确认状态 + 保存目录（前端弹确认框展示路径）
#[tauri::command]
pub fn meeting_privacy() -> Result<MeetingPrivacy, String> {
    let confirmed = crate::island::store::load_meeting_settings().privacy_confirmed;
    Ok(MeetingPrivacy {
        confirmed,
        dir: meetings_dir()?.display().to_string(),
    })
}

/// 用户确认「录音保存到本机磁盘」后持久化（island.db 单行 payload 模式）
#[tauri::command]
pub fn meeting_confirm_privacy() -> Result<bool, String> {
    let mut settings = crate::island::store::load_meeting_settings();
    settings.privacy_confirmed = true;
    crate::island::store::save_meeting_settings(&settings)?;
    Ok(true)
}

/// 播放录音：把该 wav 动态放行进 asset 协议，返回文件路径（前端用
/// `convertFileSrc` 转成 `<audio>` 能用的 URL）。不复制、不转码——asset
/// 协议支持 Range 请求，进度拖动能直接用。
#[tauri::command]
pub fn meeting_audio(app: tauri::AppHandle, id: String) -> Result<String, String> {
    use tauri::Manager;
    let id = clean_meeting_id(&id)?;
    let wav = meetings_dir()?.join(format!("{id}.wav"));
    if !wav.exists() {
        return Err("录音不存在或已被删除".into());
    }
    app.asset_protocol_scope()
        .allow_file(&wav)
        .map_err(|e| format!("放行录音文件失败: {e}"))?;
    Ok(wav.display().to_string())
}

/// 会议录音设置（目前只有「自动音量」；隐私确认仍走 meeting_privacy）
#[tauri::command]
pub fn meeting_settings() -> MeetingSettings {
    crate::island::store::load_meeting_settings()
}

/// 开关「保存时自动提升音量」并返回最新设置
#[tauri::command]
pub fn meeting_set_auto_gain(enabled: bool) -> Result<MeetingSettings, String> {
    let mut settings = crate::island::store::load_meeting_settings();
    settings.auto_gain = enabled;
    crate::island::store::save_meeting_settings(&settings)?;
    Ok(settings)
}

// ============================================================
// 单测：只测纯函数，不真开音频设备（录音本身留给手测）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_has_canonical_layout() {
        let b = wav_header(16_000, 1, 0);
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[8..12], b"WAVE");
        assert_eq!(&b[12..16], b"fmt ");
        // fmt 块长 16 + PCM 格式码
        assert_eq!(u32::from_le_bytes([b[16], b[17], b[18], b[19]]), 16);
        assert_eq!(u16::from_le_bytes([b[20], b[21]]), 1);
        assert_eq!(u16::from_le_bytes([b[22], b[23]]), 1);
        // 采样率 / 字节率 / 块对齐 / 位深
        assert_eq!(u32::from_le_bytes([b[24], b[25], b[26], b[27]]), 16_000);
        assert_eq!(u32::from_le_bytes([b[28], b[29], b[30], b[31]]), 32_000);
        assert_eq!(u16::from_le_bytes([b[32], b[33]]), 2);
        assert_eq!(u16::from_le_bytes([b[34], b[35]]), 16);
        assert_eq!(&b[36..40], b"data");
        assert_eq!(u32::from_le_bytes([b[40], b[41], b[42], b[43]]), 0);
    }

    #[test]
    fn wav_header_sizes_track_data_len() {
        let b = wav_header(48_000, 2, 960_000);
        assert_eq!(u32::from_le_bytes([b[4], b[5], b[6], b[7]]), 960_036);
        assert_eq!(u32::from_le_bytes([b[40], b[41], b[42], b[43]]), 960_000);
        assert_eq!(u32::from_le_bytes([b[28], b[29], b[30], b[31]]), 192_000);
    }

    #[test]
    fn parse_wav_header_roundtrips_and_rejects_foreign() {
        let b = wav_header(16_000, 1, 64_000);
        assert_eq!(parse_wav_header(&b), Some((16_000, 1, 64_000)));
        // 非法 / 外来头：长度不足、缺 RIFF、非 PCM
        assert!(parse_wav_header(&b[..40]).is_none());
        let mut bad = b;
        bad[0] = b'X';
        assert!(parse_wav_header(&bad).is_none());
        let mut not_pcm = b;
        not_pcm[20] = 3;
        assert!(parse_wav_header(&not_pcm).is_none());
    }

    #[test]
    fn downmix_mono_passthrough_and_stereo_average() {
        let mono_f32 = [0.0f32, 0.5, -1.0, 1.0];
        assert_eq!(
            downmix_to_mono_i16(&mono_f32, 1, |s: f32| s),
            vec![0, 16_384, -32_767, 32_767]
        );
        // 立体声取平均：左右各 0.5 → 0.5
        let stereo = [0.5f32, 0.5, -1.0, 1.0];
        assert_eq!(
            downmix_to_mono_i16(&stereo, 2, |s: f32| s),
            vec![16_384, 0]
        );
        // i16 输入直通换算：满幅正弦端点
        let mono_i16 = [0i16, 16_384, -32_768];
        let out = downmix_to_mono_i16(&mono_i16, 1, |s: i16| s as f32 / 32_768.0);
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 16_384);
        assert_eq!(out[2], -32_767);
        // channels=0 按 1 处理，不 panic
        assert_eq!(downmix_to_mono_i16(&[0.25f32], 0, |s: f32| s), vec![8_192]);
    }

    #[test]
    fn seg_clock_deducts_paused_segments() {
        let mut clock = SegClock::default();
        clock.resume(1_000);
        // 录了 5 秒后暂停 10 秒，有效时间轴不动
        assert_eq!(clock.effective_ms(6_000), 5_000);
        clock.pause(6_000);
        assert_eq!(clock.effective_ms(16_000), 5_000);
        // 继续后再记 3 秒
        clock.resume(16_000);
        assert_eq!(clock.effective_ms(19_000), 8_000);
        clock.pause(19_000);
        // 暂停中重复 pause 是幂等 no-op，时间轴不再增长
        clock.pause(20_000);
        assert_eq!(clock.effective_ms(21_000), 8_000);
        // 再继续并重复 resume（幂等）：从 21_000 起重新计时
        clock.resume(21_000);
        clock.resume(22_000);
        assert_eq!(clock.effective_ms(23_000), 10_000);
    }

    #[test]
    fn sidecar_roundtrips_and_reads_old_files() {
        let sidecar = MeetingSidecar {
            started_at: 1_700_000_000_000,
            duration_ms: 65_432,
            device: "麦克风 (Realtek Audio)".into(),
            sample_rate: 48_000,
            marks: vec![MeetingMark { ms: 1_200, label: "结论".into() }],
        };
        let text = serde_json::to_string(&sidecar).unwrap();
        let parsed: MeetingSidecar = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.duration_ms, 65_432);
        assert_eq!(parsed.marks.len(), 1);
        // 旧版侧车没有 sample_rate / marks / device 字段也能读
        let old: MeetingSidecar = serde_json::from_str(r#"{"started_at": 42}"#).unwrap();
        assert_eq!(old.started_at, 42);
        assert_eq!(old.duration_ms, 0);
        assert!(old.marks.is_empty());
        assert!(old.device.is_empty());
        assert_eq!(old.sample_rate, 0);
        // 手工损坏的侧车缺 started_at 也能读：其余字段照常生效，
        // started_at 落 default 0 后由 build_meeting_item 从文件名/修改时间兜底
        let damaged: MeetingSidecar =
            serde_json::from_str(r#"{"duration_ms": 12345, "marks": [{"ms": 7, "label": "a"}]}"#)
                .unwrap();
        assert_eq!(damaged.started_at, 0);
        assert_eq!(damaged.duration_ms, 12_345);
        assert_eq!(damaged.marks.len(), 1);
    }

    #[test]
    fn unique_stem_avoids_collision() {
        let dir = std::env::temp_dir().join(format!("agenthud-meeting-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        assert_eq!(unique_stem(&dir, "20261112-1015"), "20261112-1015");
        std::fs::write(dir.join("20261112-1015.wav"), b"x").expect("占位");
        assert_eq!(unique_stem(&dir, "20261112-1015"), "20261112-1015-2");
        std::fs::write(dir.join("20261112-1015-2.wav"), b"x").expect("占位");
        assert_eq!(unique_stem(&dir, "20261112-1015"), "20261112-1015-3");
        // stop 收尾中的 tmp 同样避让：新会话绝不能复用正在保存的主干
        // （stop→start 快速连击时 File::create 同名 tmp 会截断正在保存的数据）
        std::fs::write(dir.join("20261112-1015-3.wav.tmp"), b"x").expect("占位 tmp");
        assert_eq!(unique_stem(&dir, "20261112-1015"), "20261112-1015-4");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stereo_device_writes_mono_wav_header() {
        let dir = std::env::temp_dir().join(format!("agenthud-meeting-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("stereo.wav");
        let file = std::fs::File::create(&path).expect("建临时文件");
        // 设备报 2 声道：回调已降混单声道，WAV 头必须写 1（否则播放器按双声道
        // 解读单声道数据 → 时长翻倍、音调低八度）
        let mut rec = Recorder::create(file, 16_000, 2);
        rec.write_placeholder_header().expect("写占位头");
        rec.bytes_written = 3_200;
        rec.finalize().expect("回填头");
        let mut head = [0u8; 44];
        std::io::Read::read_exact(&mut std::fs::File::open(&path).expect("打开产物"), &mut head)
            .expect("读头");
        assert_eq!(parse_wav_header(&head), Some((16_000, 1, 3_200)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 测试用：写一个 16kHz 单声道 PCM16 wav（峰值可控）
    fn write_test_wav(path: &Path, samples: &[i16]) {
        let mut file = std::fs::File::create(path).expect("建测试 wav");
        let header = wav_header(16_000, 1, (samples.len() * 2) as u64);
        file.write_all(&header).expect("写头");
        for sample in samples {
            file.write_all(&sample.to_le_bytes()).expect("写数据");
        }
        file.sync_all().expect("落盘");
    }

    /// 测试用：读回 wav data 段的最大振幅
    fn wav_peak(path: &Path) -> i32 {
        let bytes = std::fs::read(path).expect("读回 wav");
        bytes[44..]
            .chunks_exact(2)
            .map(|c| (i16::from_le_bytes([c[0], c[1]]) as i32).abs())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn normalize_boosts_quiet_audio_and_forces_no_clip() {
        let dir = std::env::temp_dir().join(format!("agenthud-meeting-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join("quiet.wav");
        // 峰值 8000（≈ -12 dBFS）的方波：增益 ≈ 3.25 → 峰值 ≈ 26000（-2 dBFS）
        let samples: Vec<i16> = (0..4_000).map(|i| if i % 2 == 0 { 8_000 } else { -8_000 }).collect();
        write_test_wav(&path, &samples);
        let gain = normalize_wav_peak(&path).expect("归一化");
        assert!((gain - 3.25).abs() < 0.05, "增益应≈3.25，实际 {gain}");
        let peak = wav_peak(&path);
        assert!((25_000..=26_500).contains(&peak), "峰值应≈26000，实际 {peak}");

        // 已经很响（32000）：只放大不衰减，原样返回
        let loud: Vec<i16> = (0..2_000).map(|i| if i % 2 == 0 { 32_000 } else { -32_000 }).collect();
        write_test_wav(&path, &loud);
        assert_eq!(normalize_wav_peak(&path).expect("归一化"), 1.0);
        assert_eq!(wav_peak(&path), 32_000);

        // 基本静音（全是 0）：不放大底噪
        write_test_wav(&path, &[0i16; 1_000]);
        assert_eq!(normalize_wav_peak(&path).expect("归一化"), 1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_tmp_deletable_only_after_threshold() {
        // 刚产生的 tmp（stop 收尾中）绝不能删
        assert!(!stale_tmp_deletable(1_000, 1_000));
        assert!(!stale_tmp_deletable(1_000, 60_999));
        // 到达 / 超过阈值（1 分钟）才可删
        assert!(stale_tmp_deletable(1_000, 61_000));
        assert!(stale_tmp_deletable(1_000, 200_000));
        // 边界异常输入一律不删：宁可多留一个残留，也不能误删正在保存的录音
        assert!(!stale_tmp_deletable(-1, 100_000));
        assert!(!stale_tmp_deletable(1_000, 500));
    }

    #[test]
    fn file_stem_and_started_at_roundtrip() {
        use chrono::TimeZone;
        let ts = chrono::Local
            .with_ymd_and_hms(2026, 11, 12, 10, 15, 0)
            .single()
            .expect("合法本地时间");
        let stem = meeting_file_stem(ts);
        assert_eq!(stem, "20261112-1015");
        assert_eq!(stem_to_started_at(&stem), Some(ts.timestamp_millis()));
        // 重名后缀不影响解析
        assert_eq!(
            stem_to_started_at("20261112-1015-2"),
            Some(ts.timestamp_millis())
        );
        assert_eq!(stem_to_started_at("周会"), None);
        assert_eq!(stem_to_started_at("2026-1015"), None);
    }

    #[test]
    fn sanitize_meeting_name_rejects_bad_input() {
        assert!(sanitize_meeting_name("  周会记录  ").unwrap() == "周会记录");
        assert!(sanitize_meeting_name("").is_err());
        assert!(sanitize_meeting_name("   ").is_err());
        assert!(sanitize_meeting_name(".").is_err());
        assert!(sanitize_meeting_name("..").is_err());
        assert!(sanitize_meeting_name("a/b").is_err());
        assert!(sanitize_meeting_name("a\\b").is_err());
        assert!(sanitize_meeting_name("a:b").is_err());
        assert!(sanitize_meeting_name("a*b").is_err());
        assert!(sanitize_meeting_name(&"名".repeat(101)).is_err());
        assert_eq!(sanitize_meeting_name(&"名".repeat(100)).unwrap().chars().count(), 100);
    }
}
