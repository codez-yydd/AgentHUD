//! 实时字幕音频采集：Windows WASAPI loopback（系统声音）+ 全平台麦克风。
//!
//! 统一输出 16kHz 单声道 f32（[`super::TARGET_RATE`]）：
//!
//! - **loopback 不走 `input_devices()`**：WASAPI 的实现是对「输出设备」用
//!   `default_output_config()` 拿格式，再调 `device.build_input_stream()`；
//!   cpal 发现设备是 render 端点时会自动加上
//!   `AUDCLNT_STREAMFLAGS_LOOPBACK`。所以 loopback 设备列表来自
//!   `output_devices()`，且只有 Windows 提供。
//! - cpal 的 `Stream` 是 `!Send`（`NotSendSyncAcrossAllPlatforms`），不能跨线程
//!   持有：本模块起一个 keeper 线程创建、持有并播放流，上层用
//!   [`AtomicBool`] 通知停止；线程退出时 stream 在**本线程** drop。
//! - 回调里「按采样格式转 f32 → 降混单声道 → 线性重采样到 16k」，再把块交给
//!   上层回调。重采样器在回调外创建（有内部相位状态，必须跨块复用）。
//! - 实测（本机 Windows 11 + 48kHz 扬声器）：系统完全没在播放时 WASAPI
//!   loopback **不会回调**（共享模式事件驱动，静音 = 没有数据）；pipeline 不能
//!   依赖周期性静音帧推进 VAD，停止/暂停采集时要用 [`super::vad::Segmenter`]
//!   的 `current_utterance()` 兜底收尾。
//!
//! 纯函数 [`downmix`] / [`plan_block`] / [`normalize_gain`] 不依赖真实设备，
//! 有单测覆盖；真实设备冒烟测试默认 `#[ignore]`。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::TARGET_RATE;

/// 采集可用设备名（cpal 没有跨平台稳定的设备 id，统一用设备名；空 = 默认设备）。
#[derive(Debug, Clone, Default)]
pub(crate) struct LiveDevices {
    /// 系统声音（loopback）设备：Windows 的输出设备；其它平台为空
    pub loopback: Vec<String>,
    /// 麦克风（输入）设备
    pub mic: Vec<String>,
}

/// 枚举当前可用设备（每次调用都是设备状态快照）。
pub(crate) fn list_devices() -> LiveDevices {
    let host = cpal::default_host();
    let mut devices = LiveDevices::default();
    #[cfg(windows)]
    {
        devices.loopback = match host.output_devices() {
            Ok(outputs) => collect_device_names(outputs),
            Err(_) => Vec::new(),
        };
    }
    devices.mic = match host.input_devices() {
        Ok(inputs) => collect_device_names(inputs),
        Err(_) => Vec::new(),
    };
    devices
}

/// 取设备名：跳过无名设备，按出现顺序去重（WASAPI 偶尔给同名端点）。
fn collect_device_names(devices: impl IntoIterator<Item = cpal::Device>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for device in devices {
        let Ok(name) = device.name() else { continue };
        let name = name.trim();
        if name.is_empty() || names.iter().any(|known| known == name) {
            continue;
        }
        names.push(name.to_string());
    }
    names
}

/// 启动采集：起 keeper 线程建流并播放，返回其句柄。
///
/// - `source`：`loopback`（系统声音，仅 Windows）或 `mic`（麦克风；非 Windows
///   平台无论传什么都会走麦克风）。
/// - `device`：设备名，空 = 默认设备。
/// - `stop`：置 true 后线程在 50ms 内停流并退出（stream 在线程内 drop）。
/// - `on_chunk`：16kHz 单声道 f32 块回调，在 cpal 采集线程里执行，**不要做
///   阻塞操作**；上层通常丢进队列。
///
/// 设备打不开 / 流启动失败时同步返回中文错误，调用方不用等线程。
pub(crate) fn start(
    source: &str,
    device: &str,
    stop: Arc<AtomicBool>,
    on_chunk: impl FnMut(&[f32]) + Send + 'static,
) -> Result<JoinHandle<()>, String> {
    let source = source.trim().to_ascii_lowercase();
    let device = device.trim().to_string();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let handle = thread::Builder::new()
        .name("island-live-audio".into())
        .spawn(move || keep_stream(source, device, stop, on_chunk, ready_tx))
        .map_err(|e| format!("启动音频采集线程失败: {e}"))?;
    // 等 keeper 线程把「设备 + 流 + play」走完再返回，失败能同步报给上层。
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(err)) => {
            let _ = handle.join();
            Err(err)
        }
        Err(_) => {
            let _ = handle.join();
            Err("启动音频采集失败（设备可能被占用）".to_string())
        }
    }
}

/// keeper 线程主体：解析设备 → 建流 → play → 等 stop → 在本线程 drop stream。
fn keep_stream(
    source: String,
    device: String,
    stop: Arc<AtomicBool>,
    on_chunk: impl FnMut(&[f32]) + Send + 'static,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let plan = match plan_capture(&source, &device) {
        Ok(plan) => plan,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    let stream = match build_capture_stream(&plan.device, &plan.config, on_chunk) {
        Ok(stream) => stream,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    if let Err(err) = stream.play() {
        drop(stream);
        let _ = ready.send(Err(format!(
            "启动音频采集失败（设备可能被占用）: {err}"
        )));
        return;
    }
    let _ = ready.send(Ok(()));
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(50));
    }
    // 退出前在本线程 drop：cpal 的 Stream 不能跨线程丢弃。
    drop(stream);
}

/// 打开采集所需的设备与格式（此时还没建流）。
struct CapturePlan {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
}

fn plan_capture(source: &str, device: &str) -> Result<CapturePlan, String> {
    let host = cpal::default_host();
    #[cfg(windows)]
    {
        // 系统声音只有 Windows 能做；空 source 与 `loopback` 都按系统声音处理。
        if source != "mic" {
            return plan_loopback(&host, device);
        }
    }
    #[cfg(not(windows))]
    {
        // 非 Windows 平台没有 loopback 能力，忽略 source 一律走麦克风。
        let _ = source;
    }
    plan_mic(&host, device)
}

/// 系统声音：挑**输出设备**，格式取 `default_output_config()`（不是输入格式）。
#[cfg(windows)]
fn plan_loopback(host: &cpal::Host, device_name: &str) -> Result<CapturePlan, String> {
    let device = if device_name.is_empty() {
        host.default_output_device().ok_or_else(|| {
            "没有可用的默认扬声器，请先启用输出设备（耳机 / 音箱）".to_string()
        })?
    } else {
        let mut found = None;
        if let Ok(outputs) = host.output_devices() {
            for output in outputs {
                if output.name().ok().as_deref() == Some(device_name) {
                    found = Some(output);
                    break;
                }
            }
        }
        found.ok_or_else(|| format!("找不到输出设备「{device_name}」，可能已被拔出或禁用"))?
    };
    let config = device
        .default_output_config()
        .map_err(|e| format!("读取输出设备「{device_name}」参数失败: {e}"))?;
    Ok(CapturePlan { device, config })
}

/// 麦克风：挑**输入设备**，格式取 `default_input_config()`。
fn plan_mic(host: &cpal::Host, device_name: &str) -> Result<CapturePlan, String> {
    let device = if device_name.is_empty() {
        host.default_input_device()
            .ok_or_else(|| "没有可用的麦克风，请检查麦克风连接".to_string())?
    } else {
        let mut found = None;
        if let Ok(inputs) = host.input_devices() {
            for input in inputs {
                if input.name().ok().as_deref() == Some(device_name) {
                    found = Some(input);
                    break;
                }
            }
        }
        found.ok_or_else(|| format!("找不到输入设备「{device_name}」，可能已被拔出或禁用"))?
    };
    let config = device
        .default_input_config()
        .map_err(|e| format!("读取麦克风「{device_name}」参数失败: {e}"))?;
    Ok(CapturePlan { device, config })
}

/// 按采样格式挑回调分支（cpal 的 `build_input_stream` 按具体样本类型单态化）。
fn build_capture_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    on_chunk: impl FnMut(&[f32]) + Send + 'static,
) -> Result<cpal::Stream, String> {
    let channels = config.channels() as usize;
    let native_rate = config.sample_rate().0;
    let stream_config: cpal::StreamConfig = config.clone().into();
    // 重采样器有内部相位状态，必须跨块复用；16k 设备不重采样（避免线性重采样
    // 在同采样率下仍引入滤波/延迟）。
    let resampler = if native_rate == TARGET_RATE {
        None
    } else {
        Some(
            sherpa_onnx::LinearResampler::create(native_rate as i32, TARGET_RATE as i32)
                .ok_or_else(|| {
                    format!("初始化音频重采样器失败（{native_rate}Hz → {TARGET_RATE}Hz）")
                })?,
        )
    };
    match config.sample_format() {
        cpal::SampleFormat::F32 => {
            build_stream_typed::<f32>(device, &stream_config, channels, resampler, on_chunk, |s| s)
        }
        cpal::SampleFormat::I16 => build_stream_typed::<i16>(
            device,
            &stream_config,
            channels,
            resampler,
            on_chunk,
            |s: i16| s as f32 / 32_768.0,
        ),
        cpal::SampleFormat::U16 => build_stream_typed::<u16>(
            device,
            &stream_config,
            channels,
            resampler,
            on_chunk,
            |s: u16| (s as f32 - 32_768.0) / 32_768.0,
        ),
        other => Err(format!(
            "设备采样格式暂不支持（{other:?}），请换一个音频设备"
        )),
    }
}

fn build_stream_typed<T: cpal::SizedSample + 'static>(
    device: &cpal::Device,
    stream_config: &cpal::StreamConfig,
    channels: usize,
    resampler: Option<sherpa_onnx::LinearResampler>,
    mut sink: impl FnMut(&[f32]) + Send + 'static,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream, String> {
    device
        .build_input_stream(
            stream_config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mut converted = Vec::with_capacity(data.len());
                for &sample in data {
                    converted.push(to_f32(sample));
                }
                let mono = downmix(&converted, channels);
                let chunk = match &resampler {
                    Some(resampler) => resampler.resample(&mono, false),
                    None => mono,
                };
                if !chunk.is_empty() {
                    sink(&chunk);
                }
            },
            |err: cpal::StreamError| {
                // 设备热插拔等运行期错误无法回传上层；只记一行，避免回调里 panic。
                eprintln!("[zbar-island-live] 音频采集流错误: {err}");
            },
            None,
        )
        .map_err(|e| format!("启动音频采集失败（设备可能被占用）: {e}"))
}

/// 交织多声道 → 单声道：按帧求平均；NaN / 无穷按 0 处理；不足一帧的尾巴丢弃。
pub(crate) fn downmix(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples
            .iter()
            .map(|&sample| if sample.is_finite() { sample } else { 0.0 })
            .collect();
    }
    let mut mono = Vec::with_capacity(samples.len() / channels);
    for frame in samples.chunks_exact(channels) {
        let sum: f32 = frame
            .iter()
            .map(|&sample| if sample.is_finite() { sample } else { 0.0 })
            .sum();
        let value = sum / channels as f32;
        mono.push(if value.is_finite() { value } else { 0.0 });
    }
    mono
}

/// 采集块大小（样本数）：参考实现按「采样率 × 块时长」算，最少 160 样本。
///
/// 当前 cpal 方案是回调驱动、不主动切块；保留该纯函数给后续 pipeline 估算
/// 队列水位 / 单帧时长用。
pub(crate) fn plan_block(sample_rate: u32, block_ms: u32) -> usize {
    let frames = (sample_rate as u64 * block_ms as u64) / 1000;
    frames.max(160) as usize
}

/// 轻度自动增益：峰值拉到 `target_peak`，增益上限 `max_gain`，结果夹到 ±1。
///
/// 与参考实现同口径：
/// - 空输入 / 峰值 < 1e-4 原样返回（不放大底噪）；
/// - 需要的增益 ≤ 1.02 时原样返回（避免来回抖动、反复改音色）。
pub(crate) fn normalize_gain(samples: &[f32], target_peak: f32, max_gain: f32) -> Vec<f32> {
    if samples.is_empty() {
        return samples.to_vec();
    }
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    if !(peak >= 1e-4) || !peak.is_finite() {
        return samples.to_vec();
    }
    let gain = (target_peak / peak).min(max_gain);
    if gain <= 1.02 {
        return samples.to_vec();
    }
    samples
        .iter()
        .map(|&sample| (sample * gain).clamp(-1.0, 1.0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_samples(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "实际 {actual:?} 期望 {expected:?}");
        for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (a - e).abs() < 1e-6,
                "第 {index} 个样本：实际 {a} 期望 {e}"
            );
        }
    }

    #[test]
    fn downmix_averages_channels() {
        let stereo = [0.5, -0.5, 0.25, 0.75, 1.0, 0.0];
        assert_samples(&downmix(&stereo, 2), &[0.0, 0.5, 0.5]);
        // 5.1：6 声道按帧平均
        let surround = [0.0, 0.0, 0.0, 0.0, 0.0, 0.6];
        assert_samples(&downmix(&surround, 6), &[0.1]);
    }

    #[test]
    fn downmix_sanitizes_nan_and_inf() {
        let bad = [f32::NAN, 1.0, f32::INFINITY, 1.0];
        assert_samples(&downmix(&bad, 2), &[0.5, 0.5]);
        // 单声道也不能把 NaN 透传下去
        assert_samples(&downmix(&[f32::NAN, 0.25], 1), &[0.0, 0.25]);
    }

    #[test]
    fn downmix_handles_degenerate_input() {
        assert!(downmix(&[], 2).is_empty());
        // 不足一帧的尾巴丢弃
        assert_samples(&downmix(&[0.5, 0.5, 1.0], 2), &[0.5]);
        // channels 为 0 时按单声道处理，不 panic
        assert_samples(&downmix(&[0.2, -0.2], 0), &[0.2, -0.2]);
    }

    #[test]
    fn plan_block_matches_sample_math() {
        assert_eq!(plan_block(48_000, 80), 3_840);
        assert_eq!(plan_block(44_100, 100), 4_410);
        assert_eq!(plan_block(16_000, 80), 1_280);
        // 太小的块夹到 160 样本
        assert_eq!(plan_block(16_000, 5), 160);
        assert_eq!(plan_block(8_000, 0), 160);
    }

    #[test]
    fn normalize_gain_boosts_quiet_audio_to_target_peak() {
        let quiet = [0.05, -0.05, 0.025];
        let boosted = normalize_gain(&quiet, 0.85, 6.0);
        let peak = boosted.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!((peak - 0.05 * 6.0).abs() < 1e-6, "增益应封顶 6 倍，实际峰值 {peak}");
    }

    #[test]
    fn normalize_gain_leaves_loud_and_silent_audio_unchanged() {
        let loud = [0.9, -0.8, 0.3];
        assert_eq!(normalize_gain(&loud, 0.85, 6.0), loud.to_vec());

        let silent = [0.0, 0.0, 1e-5, -1e-5];
        assert_eq!(normalize_gain(&silent, 0.85, 6.0), silent.to_vec());

        let empty: [f32; 0] = [];
        assert!(normalize_gain(&empty, 0.85, 6.0).is_empty());
    }

    #[test]
    fn normalize_gain_clips_output() {
        // 目标峰值 > 1 且增益很大时，结果必须夹到 ±1（正常设置下目标峰值
        // 0.85 不会触发，这里是防御性覆盖）。
        let quiet = [0.01, -0.01];
        let boosted = normalize_gain(&quiet, 1.5, 100.0);
        assert_samples(&boosted, &[1.0, -1.0]);

        // 直接给一个非有限值也不 panic（走「原样返回」分支）。
        let nan = [f32::NAN];
        let out = normalize_gain(&nan, 0.85, 6.0);
        assert!(out[0].is_nan());
    }

    /// 真实设备冒烟（默认 ignore）：枚举设备，并尝试开始采集 700ms 后停止。
    ///
    /// 只验证「设备能打开、流能播放、stop 后 keeper 线程能正常退出」，
    /// 不断言一定收到样本（系统声音没在播放时 WASAPI loopback 可能只回空缓冲）。
    /// 运行：`cargo test --lib island::live::audio -- --ignored --nocapture`
    #[test]
    #[ignore = "需要真实音频设备"]
    fn device_smoke_capture() {
        use std::sync::atomic::AtomicU64;

        let devices = list_devices();
        println!("loopback 设备: {:?}", devices.loopback);
        println!("麦克风设备: {:?}", devices.mic);

        // loopback 只在 Windows 有效；mic 用第一台设备（没有就试默认设备）。
        let mut attempts: Vec<(&str, String)> = Vec::new();
        #[cfg(windows)]
        attempts.push(("loopback", String::new()));
        attempts.push(("mic", devices.mic.first().cloned().unwrap_or_default()));

        for (source, device) in attempts {
            let stop = Arc::new(AtomicBool::new(false));
            let stop_thread = Arc::clone(&stop);
            let samples = Arc::new(AtomicU64::new(0));
            let samples_in_callback = Arc::clone(&samples);
            let handle = match start(source, &device, stop_thread, move |chunk: &[f32]| {
                samples_in_callback.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }) {
                Ok(handle) => handle,
                Err(err) => {
                    println!("{source} 启动失败（跳过）: {err}");
                    continue;
                }
            };
            thread::sleep(Duration::from_millis(700));
            stop.store(true, Ordering::Relaxed);
            assert!(handle.join().is_ok(), "{source} 的 keeper 线程不应 panic");
            println!(
                "{source}（{}）700ms 收到 {} 个 16k 样本",
                if device.is_empty() {
                    "默认设备"
                } else {
                    device.as_str()
                },
                samples.load(Ordering::Relaxed)
            );
        }
    }
}
