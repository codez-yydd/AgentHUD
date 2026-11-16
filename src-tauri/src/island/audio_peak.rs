//! 真实音频峰值（收起态音乐视图的「律动条」用）。
//!
//! 前端当前用合成包络画律动条；本命令给它真实数据：Windows 上读默认渲染设备
//! 的 Core Audio `IAudioMeterInformation::GetPeakValue()`（0..1）。
//!
//! 性能设计（前端会以 30~60fps 轮询命令，必须 <2ms）：
//! - 独立的采样线程持有 COM 对象（`IMMDeviceEnumerator` + `IAudioMeterInformation`），
//!   在**自己线程里** `CoInitializeEx(MTA)`，只做「读峰值 → 写原子 u32（f32 bits）」；
//! - 命令本身不碰 COM，只读一个 `AtomicU32`，微秒级返回；
//! - 采样线程约 16ms 一拍（≈60Hz），前端 30fps 轮询读到的永远是最新值。
//!
//! 失败静默降级（绝不 panic、不卡主线程）：
//! - 没有默认输出设备 / COM 创建失败 / `GetPeakValue` 失败 → 峰值按 0 处理，
//!   下一拍或每 ~2s 重新枚举设备继续尝试；
//! - 非法值（NaN/∞/负数）夹成 0，>1 夹成 1；
//! - 非 Windows 平台不实现，命令固定返回 0.0。
//!
//! 只提供命令式契约（`island_audio_peak`），不额外推送事件：前端轮询即可，
//! 30~60fps 下命令开销可忽略，也避免事件通道被高频消息占满。

use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicBool, AtomicU64};

/// 采样线程的轮询间隔（毫秒）。16ms ≈ 60Hz，足够 30fps 前端轮询取到新值。
#[cfg(target_os = "windows")]
const PEAK_POLL_MS: u64 = 16;
/// 每多少拍重新枚举一次默认设备（约 2s）。用户切换耳机/显示器时能自动跟上。
#[cfg(target_os = "windows")]
const REENUM_EVERY_TICKS: u32 = 125;
/// 距上次 `island_audio_peak` 调用超过这么久就自停采样线程（音乐视图收起 /
/// 切到别的板块后没人再读峰值，没必要继续 16ms 一拍地占 CPU）。
#[cfg(target_os = "windows")]
const IDLE_STOP_MS: u64 = 30_000;

/// 采样线程只启动一次；命令只读下面这个原子值。
/// 线程因空闲超时或 [`suspend`] 退出时会复位，允许下次 invoke 重新拉起。
#[cfg(target_os = "windows")]
static STARTED: AtomicBool = AtomicBool::new(false);
/// 外部要求停采（岛总开关关闭等），线程看到后退出。
#[cfg(target_os = "windows")]
static STOP: AtomicBool = AtomicBool::new(false);
/// 采样线程代数：每次 start / suspend 推进一次，用来顶掉还在跑的老线程。
#[cfg(target_os = "windows")]
static GEN: AtomicU64 = AtomicU64::new(0);
/// 最后一次命令调用的单调时间（毫秒），用于空闲自停。
#[cfg(target_os = "windows")]
static LAST_USE: AtomicU64 = AtomicU64::new(0);
/// 单调时钟原点：`Instant` 不能塞进原子量，用启动时刻做基准换算毫秒。
#[cfg(target_os = "windows")]
static CLOCK: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
/// 最新峰值，f32 的 bit 表示（0.0 的 bit 就是 0）。
static PEAK_BITS: AtomicU32 = AtomicU32::new(0);

// ============================================================
// 纯函数（跨平台，单测覆盖）
// ============================================================

/// 把 Core Audio 的峰值夹进 0..=1：NaN/∞/负数 → 0，>1 → 1。
pub fn sanitize_peak(value: f32) -> f32 {
    if !value.is_finite() || value <= 0.0 {
        0.0
    } else if value > 1.0 {
        1.0
    } else {
        value
    }
}

/// 读取失败（None）静默按 0 处理；成功值再走 [`sanitize_peak`]。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn peak_or_zero(value: Option<f32>) -> f32 {
    value.map(sanitize_peak).unwrap_or(0.0)
}

/// 原子里存的 f32 bits → 可对外返回的安全峰值。
pub fn decode_peak(bits: u32) -> f32 {
    sanitize_peak(f32::from_bits(bits))
}

/// 采样线程写入最新读数（NaN/∞/负数已在入口夹掉）。
#[cfg(target_os = "windows")]
fn store_peak(value: f32) {
    PEAK_BITS.store(sanitize_peak(value).to_bits(), Ordering::Relaxed);
}

fn current_peak() -> f32 {
    decode_peak(PEAK_BITS.load(Ordering::Relaxed))
}

// ============================================================
// 命令
// ============================================================

/// 当前系统音频峰值（0..1，失败/无设备 = 0.0）。
///
/// 前端音乐视图以 30~60fps 轮询即可；命令只读原子值，不阻塞、不分配。
/// 首次调用只会懒启动采样线程并立即返回，不等待任何 COM 操作。
#[tauri::command]
pub fn island_audio_peak() -> f32 {
    ensure_started();
    current_peak()
}

/// 停止采样线程（岛总开关关闭时由 `stop_services` 调用）：置 STOP 并复位
/// STARTED，下次 `island_audio_peak` 会重新拉起线程。非 Windows 没有采样
/// 线程，空实现。
pub fn suspend() {
    #[cfg(target_os = "windows")]
    {
        STOP.store(true, Ordering::SeqCst);
        // 推进代数：可能还在 sleep 的老线程醒来后会看到代数不一致直接退出，
        // 且不会把新一代线程的 STARTED 复位掉。
        GEN.fetch_add(1, Ordering::SeqCst);
        STARTED.store(false, Ordering::SeqCst);
    }
}

#[cfg(target_os = "windows")]
fn ensure_started() {
    // 每次命令都刷新使用时间：线程据此判断「还有人在读峰值」。
    LAST_USE.store(now_ms(), Ordering::Relaxed);
    imp::start();
}

#[cfg(not(target_os = "windows"))]
fn ensure_started() {}

/// 单调时钟毫秒（原点为首次调用时刻）。
#[cfg(target_os = "windows")]
fn now_ms() -> u64 {
    CLOCK
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

// ============================================================
// Windows 实现：独立 COM 线程采样
// ============================================================

#[cfg(target_os = "windows")]
mod imp {
    use std::thread;
    use std::time::Duration;

    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    use super::*;

    pub(super) fn start() {
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        // 允许上次因空闲超时 / suspend 退出的线程被重新拉起。
        STOP.store(false, Ordering::SeqCst);
        let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
        // 起线程失败就保持 0.0（STARTED 已置位，不再反复尝试；静默降级）。
        let _ = thread::Builder::new()
            .name("zbar-island-audio-peak".into())
            .spawn(move || run(gen));
    }

    /// 采样线程：COM 只在本线程初始化并持有；命令线程永远不碰 COM。
    /// `gen` 用来识别「自己是否还是当前那一代线程」：suspend 之后紧接着又被
    /// 命令拉起新线程时，老线程不能去复位 STARTED（否则会反复起出多套线程）。
    fn run(gen: u64) {
        unsafe {
            // 独立线程首次初始化 MTA；S_FALSE（已初始化过）也算成功。
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                // 无法用 COM：峰值保持 0，静默降级；复位 STARTED 让下次
                // invoke 还能重试（例如 COM 被别的线程初始化失败的偶发情况）。
                if GEN.load(Ordering::SeqCst) == gen {
                    STARTED.store(false, Ordering::SeqCst);
                }
                return;
            }
        }
        let mut enumerator: Option<IMMDeviceEnumerator> = None;
        let mut meter: Option<IAudioMeterInformation> = None;
        let mut ticks_since_enum = REENUM_EVERY_TICKS;

        loop {
            // 空闲自停：音乐视图收起 / 别的板块隐藏后，前端不再轮询本命令，
            // 线程没必要继续 16ms 一拍。外部 stop_services 也会置 STOP，
            // 新的 start 会推进 GEN 顶掉旧线程。
            if STOP.load(Ordering::Relaxed)
                || GEN.load(Ordering::SeqCst) != gen
                || now_ms().saturating_sub(LAST_USE.load(Ordering::Relaxed)) > IDLE_STOP_MS
            {
                break;
            }
            if meter.is_none() || ticks_since_enum >= REENUM_EVERY_TICKS {
                ticks_since_enum = 0;
                if let Some(fresh) = activate_meter(&mut enumerator) {
                    meter = Some(fresh);
                }
                // 失败时保留旧的 meter（设备短暂占用/切屏时不至于立刻归零）
            }
            let mut reading: Option<f32> = None;
            let mut invalid = false;
            match meter.as_ref().map(|m| unsafe { m.GetPeakValue() }) {
                Some(Ok(peak)) => reading = Some(peak),
                Some(Err(_)) => invalid = true,
                None => {}
            }
            store_peak(peak_or_zero(reading));
            if invalid {
                // 设备失效/被移除：下一拍强制重新枚举（本轮已按 0 上报）。
                meter = None;
                ticks_since_enum = REENUM_EVERY_TICKS;
            }
            ticks_since_enum = ticks_since_enum.saturating_add(1);
            thread::sleep(Duration::from_millis(PEAK_POLL_MS));
        }
        // 只有「自己还是当前线程」时才复位状态；已被新一代顶掉的老线程直接退出。
        if GEN.load(Ordering::SeqCst) == gen {
            // 退出后峰值归零，前端下次 invoke 重新拉起时会立刻拿到新读数。
            store_peak(0.0);
            STOP.store(false, Ordering::SeqCst);
            STARTED.store(false, Ordering::SeqCst);
        }
    }

    /// 打开（或复用缓存的）默认渲染设备的峰值表。
    fn activate_meter(
        enumerator: &mut Option<IMMDeviceEnumerator>,
    ) -> Option<IAudioMeterInformation> {
        unsafe {
            if enumerator.is_none() {
                *enumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok();
            }
            let enumerator = enumerator.as_ref()?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .ok()?;
            device
                .Activate::<IAudioMeterInformation>(CLSCTX_ALL, None)
                .ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_is_clamped_to_unit_range() {
        assert_eq!(sanitize_peak(0.0), 0.0);
        assert_eq!(sanitize_peak(0.42), 0.42);
        assert_eq!(sanitize_peak(1.0), 1.0);
        assert_eq!(sanitize_peak(1.5), 1.0);
        assert_eq!(sanitize_peak(f32::MAX), 1.0);
        // 负值/NaN/±∞ 都是「没有有效读数」→ 0
        assert_eq!(sanitize_peak(-0.3), 0.0);
        assert_eq!(sanitize_peak(f32::NAN), 0.0);
        assert_eq!(sanitize_peak(f32::INFINITY), 0.0);
        assert_eq!(sanitize_peak(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn failure_path_returns_zero() {
        // 无设备 / GetPeakValue 失败（None）→ 0，不 panic
        assert_eq!(peak_or_zero(None), 0.0);
        // 成功路径按夹取规则
        assert_eq!(peak_or_zero(Some(0.75)), 0.75);
        assert_eq!(peak_or_zero(Some(4.0)), 1.0);
        assert_eq!(peak_or_zero(Some(-1.0)), 0.0);
        assert_eq!(peak_or_zero(Some(f32::NAN)), 0.0);
    }

    #[test]
    fn command_is_safe_and_idempotent() {
        // 首次调用只会懒启动采样线程并立刻返回（不阻塞），值必在 0..=1；
        // 重复调用不会重复起线程，也不 panic（无设备时就是 0.0）。
        let first = island_audio_peak();
        assert!((0.0..=1.0).contains(&first), "peak={first}");
        let second = island_audio_peak();
        assert!((0.0..=1.0).contains(&second), "peak={second}");
    }

    #[test]
    fn atomic_roundtrip_decodes_safely() {
        // 0.0 的 bit 表示就是 0（初始值），解码仍是 0
        assert_eq!(decode_peak(0), 0.0);
        // 正常值往返
        assert_eq!(decode_peak(0.5f32.to_bits()), 0.5);
        assert_eq!(decode_peak(1.0f32.to_bits()), 1.0);
        // 被写坏的原子值（NaN / 越界 / 负数）也必须安全返回 0..1
        assert_eq!(decode_peak(f32::NAN.to_bits()), 0.0);
        assert_eq!(decode_peak(f32::INFINITY.to_bits()), 0.0);
        assert_eq!(decode_peak((-2.0f32).to_bits()), 0.0);
        assert_eq!(decode_peak(9.0f32.to_bits()), 1.0);
    }
}
