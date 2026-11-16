//! Silero VAD 断句 + 原始音频切片。
//!
//! 状态机：
//!
//! 1. 输入按 512 样本切帧（Silero 固定窗口），不足一帧的尾巴留到下一次；
//! 2. 每帧先做轻度自动增益（[`normalize_gain`]）再喂 VAD，**只用于检测**；
//! 3. VAD 判定「开始说话」时，用最近 18 帧预卷（约 0.58s）+ 当前帧拼出
//!    utterance 开头，避免句首辅音被吞；
//! 4. VAD 吐出完整片段（`front()`）后，用 `vad_origin + segment.start()` 在
//!    原始音频环形缓冲里定位，前后各扩 400ms / 160ms，短于 250ms 的碎片
//!    丢弃；
//! 5. 识别必须用**原始**音频切片（不是增益后的检测音频），否则每 32ms 一次
//!    的独立放大会改变音色并放大停顿里的底噪。
//!
//! 纯函数（切帧 / 切片边界 / 预卷拼接）有单测；真实 VAD 模型路径不进单测。

use std::collections::VecDeque;
use std::path::Path;

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use super::audio::normalize_gain;
use super::{file_nonempty, TARGET_RATE};

/// VAD 固定窗口（Silero 要求 512 样本 @ 16k，约 32ms）。
const WINDOW: usize = 512;
/// 预卷帧数：18 帧 ≈ 0.58s（参考实现同为 18）。
const PRE_ROLL_FRAMES: usize = 18;
/// 原始音频环形缓冲上限：40s（参考实现 `deque(maxlen=1250)` 帧）。
const RAW_MAX_SAMPLES: usize = 40 * TARGET_RATE as usize;
/// 切片向前补 400ms：VAD 起点通常晚于真实起音。
const HEAD_PAD: u64 = TARGET_RATE as u64 * 400 / 1000;
/// 切片向后补 160ms：给收尾音留余量。
const TAIL_PAD: u64 = TARGET_RATE as u64 * 160 / 1000;
/// 短于 4000 样本（250ms）的片段丢弃（参考实现同一阈值）。
const MIN_SEGMENT_SAMPLES: usize = 4000;
/// 检测用自动增益参数：与参考实现一致。
const NORMALIZE_TARGET_PEAK: f32 = 0.85;
const NORMALIZE_MAX_GAIN: f32 = 6.0;

/// 一句可送去识别的音频切片（16k 单声道 f32）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SegmentAudio {
    pub samples: Vec<f32>,
    /// 相对本次会话开始（首帧时间轴）的起始秒数
    pub t0: f64,
    /// 结束秒数
    pub t1: f64,
}

/// VAD 断句器：喂 16k 单声道 f32 块，吐出完整句子切片。
pub(crate) struct Segmenter {
    vad: VoiceActivityDetector,
    /// 还没凑满一帧的尾巴
    vad_pending: Vec<f32>,
    /// 预卷：最近 18 帧（最旧在前）
    pre_roll: VecDeque<Vec<f32>>,
    /// 原始音频环形缓冲（检测用增益音频不入这里）
    raw: VecDeque<f32>,
    /// 已喂给 VAD 的样本总数
    fed: u64,
    /// VAD 内部样本计数器的原点（reset 后对齐 fed）
    vad_origin: u64,
    /// 当前是否在一句话中间（VAD 段未吐出）
    active: bool,
    /// 当前句已累计的原始音频（预卷 + 各帧），供叫停时兜底使用
    utterance: Vec<f32>,
}

impl Segmenter {
    /// 建 VAD 断句器。
    ///
    /// `threshold` 0~1，越大越不敏感；`min_silence_ms` 判一句结束；
    /// `min_speech_ms` 判语音开始；`max_segment_s` 单句最长秒数（到点强制切）。
    pub fn new(
        vad_model: &Path,
        threshold: f32,
        min_silence_ms: u64,
        min_speech_ms: u64,
        max_segment_s: u64,
    ) -> Result<Self, String> {
        if !file_nonempty(vad_model) {
            return Err(format!(
                "静音检测模型缺失或不完整（{}），请在实时字幕设置里重新下载",
                vad_model.display()
            ));
        }
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(vad_model.to_string_lossy().into_owned()),
                threshold,
                min_silence_duration: min_silence_ms as f32 / 1000.0,
                min_speech_duration: min_speech_ms as f32 / 1000.0,
                window_size: WINDOW as i32,
                max_speech_duration: max_segment_s as f32,
            },
            sample_rate: TARGET_RATE as i32,
            num_threads: 1,
            ..Default::default()
        };
        // 缓冲 60s：远大于单句上限，避免极端长段把 VAD 内部缓冲撑爆。
        let vad = VoiceActivityDetector::create(&config, 60.0).ok_or_else(|| {
            "静音检测模型加载失败（请重新下载 silero_vad.onnx）".to_string()
        })?;
        Ok(Self {
            vad,
            vad_pending: Vec::new(),
            pre_roll: VecDeque::with_capacity(PRE_ROLL_FRAMES),
            raw: VecDeque::with_capacity(RAW_MAX_SAMPLES),
            fed: 0,
            vad_origin: 0,
            active: false,
            utterance: Vec::new(),
        })
    }

    /// 喂一块 16k 单声道 f32，返回本次凑齐的完整句子（可能 0 条或多条）。
    pub fn push(&mut self, samples: &[f32]) -> Vec<SegmentAudio> {
        self.vad_pending.extend_from_slice(samples);
        let frames = take_frames(&mut self.vad_pending, WINDOW);
        if frames.is_empty() {
            return Vec::new();
        }
        let mut segments = Vec::new();
        for frame in frames {
            self.process_frame(&frame, &mut segments);
        }
        segments
    }

    /// 清空全部状态（暂停 / 切设备 / 停止时调用），VAD 原点对齐已喂样本数。
    pub fn reset(&mut self) {
        self.vad_pending.clear();
        self.pre_roll.clear();
        self.raw.clear();
        self.utterance.clear();
        self.active = false;
        // 先清队列再重置模型状态，避免残留片段串到下一轮。
        self.vad.clear();
        self.vad.reset();
        self.vad_origin = self.fed;
    }

    /// 当前句已累计的原始音频（预卷 + 各帧）；用于叫停时的最后兜底识别。
    pub fn current_utterance(&self) -> &[f32] {
        &self.utterance
    }

    fn process_frame(&mut self, frame: &[f32], segments: &mut Vec<SegmentAudio>) {
        self.fed += frame.len() as u64;
        // 原始音频只进环形缓冲（识别用），VAD 吃的是增益后的副本。
        self.raw.extend(frame.iter().copied());
        let overflow = self.raw.len().saturating_sub(RAW_MAX_SAMPLES);
        if overflow > 0 {
            self.raw.drain(..overflow);
        }

        let detected_audio = normalize_gain(frame, NORMALIZE_TARGET_PEAK, NORMALIZE_MAX_GAIN);
        self.vad.accept_waveform(&detected_audio);

        if self.vad.detected() && !self.active {
            self.active = true;
            // 句首 = 预卷 + 当前帧（当前帧此时还没进预卷，正好不会重复）。
            self.utterance = begin_utterance(&self.pre_roll, frame);
        } else if self.active {
            self.utterance.extend_from_slice(frame);
        }

        if self.pre_roll.len() == PRE_ROLL_FRAMES {
            self.pre_roll.pop_front();
        }
        self.pre_roll.push_back(frame.to_vec());

        while !self.vad.is_empty() {
            if let Some(segment) = self.vad.front() {
                let history_start = self.fed - self.raw.len() as u64;
                if let Some((begin, end)) = segment_bounds(
                    self.fed,
                    self.raw.len(),
                    self.vad_origin,
                    segment.start(),
                    segment.n() as usize,
                ) {
                    if (end - begin) as usize >= MIN_SEGMENT_SAMPLES {
                        let begin_offset = (begin - history_start) as usize;
                        let end_offset = (end - history_start) as usize;
                        let samples: Vec<f32> = self
                            .raw
                            .iter()
                            .skip(begin_offset)
                            .take(end_offset - begin_offset)
                            .copied()
                            .collect();
                        segments.push(SegmentAudio {
                            samples,
                            t0: begin as f64 / TARGET_RATE as f64,
                            t1: end as f64 / TARGET_RATE as f64,
                        });
                    }
                }
            }
            self.vad.pop();
            // 一句结束：VAD 段吐出即视为该句收尾。
            self.active = false;
            self.utterance.clear();
        }
    }
}

/// 从待处理缓冲里切出完整窗口；不足一帧的尾巴留在 `pending` 里。
fn take_frames(pending: &mut Vec<f32>, window: usize) -> Vec<Vec<f32>> {
    if window == 0 {
        return Vec::new();
    }
    let count = pending.len() / window;
    if count == 0 {
        return Vec::new();
    }
    let complete = count * window;
    let frames = pending[..complete]
        .chunks_exact(window)
        .map(|chunk| chunk.to_vec())
        .collect();
    pending.drain(..complete);
    frames
}

/// 在原始音频里定位 VAD 片段的识别切片边界：
/// `[段起点 - 400ms, 段终点 + 160ms]`，并夹到当前环形缓冲范围内。
///
/// 返回全局样本下标 `(begin, end)`（相对会话开始），无有效音频返回 `None`。
fn segment_bounds(
    fed: u64,
    raw_len: usize,
    vad_origin: u64,
    start: i64,
    n: usize,
) -> Option<(u64, u64)> {
    // 环形缓冲里最旧样本的全局下标。
    let history_start = fed.checked_sub(raw_len as u64)?;
    let segment_start = vad_origin.saturating_add(u64::try_from(start).ok()?);
    let begin = segment_start.saturating_sub(HEAD_PAD).max(history_start);
    let end = segment_start
        .saturating_add(n as u64)
        .saturating_add(TAIL_PAD)
        .min(fed);
    if end <= begin {
        return None;
    }
    Some((begin, end))
}

/// 句首音频 = 预卷（最旧在前）+ 当前帧。
fn begin_utterance(pre_roll: &VecDeque<Vec<f32>>, frame: &[f32]) -> Vec<f32> {
    let capacity = pre_roll.iter().map(Vec::len).sum::<usize>() + frame.len();
    let mut utterance = Vec::with_capacity(capacity);
    for preroll in pre_roll {
        utterance.extend_from_slice(preroll);
    }
    utterance.extend_from_slice(frame);
    utterance
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_frames_slices_complete_windows_only() {
        let mut pending: Vec<f32> = (0..1_000).map(|value| value as f32).collect();
        let frames = take_frames(&mut pending, WINDOW);
        assert_eq!(frames.len(), 1, "1000 样本只够 1 个 512 帧");
        assert_eq!(frames[0].len(), WINDOW);
        assert_eq!(frames[0][0], 0.0);
        assert_eq!(frames[0][WINDOW - 1], 511.0);
        assert_eq!(pending.len(), 488, "不足一帧的尾巴留下轮");

        // 再补到 512 的整数倍：一次切出 2 帧，pending 清空。
        pending.resize(2 * WINDOW, 1.0);
        let frames = take_frames(&mut pending, WINDOW);
        assert_eq!(frames.len(), 2);
        assert!(pending.is_empty());
    }

    #[test]
    fn take_frames_keeps_short_input_untouched() {
        let mut pending = vec![0.5_f32; 100];
        assert!(take_frames(&mut pending, WINDOW).is_empty());
        assert_eq!(pending.len(), 100);

        // window 为 0 时防御性返回，不除零。
        assert!(take_frames(&mut pending, 0).is_empty());
    }

    #[test]
    fn begin_utterance_prepends_pre_roll_in_order() {
        let mut pre_roll = VecDeque::new();
        pre_roll.push_back(vec![1.0, 2.0]);
        pre_roll.push_back(vec![3.0, 4.0]);
        let frame = [5.0, 6.0];
        let utterance = begin_utterance(&pre_roll, &frame);
        assert_eq!(utterance, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

        // 空预卷 = 只有当前帧
        assert_eq!(begin_utterance(&VecDeque::new(), &frame), frame.to_vec());
    }

    #[test]
    fn segment_bounds_pads_head_and_tail() {
        // 段 [16000, 32000)：前补 6400（400ms）、后补 2560（160ms）。
        let (begin, end) = segment_bounds(48_000, 48_000, 0, 16_000, 16_000).expect("应能定位");
        assert_eq!(begin, 16_000 - 6_400);
        assert_eq!(end, 32_000 + 2_560);
    }

    #[test]
    fn segment_bounds_clamps_to_ring_buffer() {
        // 环形缓冲只有最近 2000 样本（全局 18000..20000），段起点更早：
        // begin 夹到 history_start，end 夹到 fed。
        let (begin, end) =
            segment_bounds(20_000, 2_000, 0, 17_000, 1_000).expect("仍应定位到可用部分");
        assert_eq!(begin, 18_000);
        assert_eq!(end, 20_000);
    }

    #[test]
    fn segment_bounds_rejects_audio_already_dropped() {
        // 段整体早于环形缓冲：没有可用音频。
        assert_eq!(segment_bounds(20_000, 2_000, 0, 0, 1_000), None);
        // 环形缓冲比已喂样本还长是异常状态：不 panic，返回 None。
        assert_eq!(segment_bounds(100, 500, 0, 0, 100), None);
    }

    #[test]
    fn segment_bounds_applies_vad_origin_after_reset() {
        // reset 后 VAD 内部计数从 0 重新开始，真实位置 = origin + start。
        let (begin, end) = segment_bounds(48_000, 48_000, 10_000, 0, 8_000).expect("应能定位");
        assert_eq!(begin, 10_000 - 6_400);
        assert_eq!(end, 18_000 + 2_560);
    }

    #[test]
    fn segment_bounds_rejects_negative_start() {
        // VAD 理论上不会给负起点；万一给了当作无效，不 panic。
        assert_eq!(segment_bounds(48_000, 48_000, 0, -1, 8_000), None);
    }

    /// 真实 Silero VAD 冒烟（默认 ignore）：模型不在时自动跳过。
    ///
    /// 模型来源：<https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx>，
    /// 参考项目放在 `<models_dir>/silero_vad.onnx`；也可用环境变量
    /// `AGENTHUD_LIVE_VAD_MODEL` 直接指定文件。
    ///
    /// 运行：`cargo test --lib island::live::vad -- --ignored --nocapture`
    #[test]
    #[ignore = "需要 silero_vad.onnx"]
    fn segmenter_real_model_smoke() {
        use std::path::PathBuf;

        let path: PathBuf = match std::env::var("AGENTHUD_LIVE_VAD_MODEL") {
            Ok(value) => PathBuf::from(value),
            Err(_) => match super::super::models_root() {
                Ok(root) => super::super::vad_model_path(&root),
                Err(err) => {
                    eprintln!("跳过：{err}");
                    return;
                }
            },
        };
        if !super::super::file_nonempty(&path) {
            eprintln!("跳过：{} 不是有效的 VAD 模型", path.display());
            return;
        }
        let mut segmenter =
            Segmenter::new(&path, 0.4, 350, 200, 10).expect("加载真实 Silero VAD");
        // 纯静音不应断出句子；再来一段 220Hz 测试音确认不 panic。
        let silence = vec![0.0_f32; TARGET_RATE as usize];
        assert!(segmenter.push(&silence).is_empty(), "静音不应断句");
        segmenter.reset();
        let tone: Vec<f32> = (0..TARGET_RATE as usize * 2)
            .map(|index| {
                (index as f32 * 2.0 * std::f32::consts::PI * 220.0 / TARGET_RATE as f32).sin() * 0.3
            })
            .collect();
        let _ = segmenter.push(&tone);
        segmenter.reset();
        assert!(segmenter.current_utterance().is_empty());
    }
}
