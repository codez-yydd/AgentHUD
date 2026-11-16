//! 灵动岛系统控制：音量 / 静音 / 亮度 / Wi-Fi / 蓝牙。
//!
//! - 音量：Windows 走 Core Audio（IAudioEndpointVolume），macOS 走 CoreAudio
//!   HAL（`kAudioDevicePropertyVolumeScalar` / `Mute`，和系统音量键同一套属性）；
//!   后台线程定期回读，用系统音量键改了也会同步到岛里；
//! - 亮度：macOS 用私有框架 DisplayServices（和系统亮度键同一套接口）；
//!   Windows WMI `WmiMonitorBrightness` + DDC/CI（笔记本/外接屏都能用）；
//! - Wi-Fi / 蓝牙：WinRT `Windows.Devices.Radios.Radio`（需要系统无线电访问许可，
//!   首次切换时系统可能弹权限框；被拒绝时前端会提示改走系统设置）。macOS 暂不支持。

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub const SYSTEM_EVENT: &str = "zbar://island-system";

static STOP: AtomicBool = AtomicBool::new(false);
static GEN: AtomicU64 = AtomicU64::new(0);
static APP: OnceLock<Mutex<Option<AppHandle>>> = OnceLock::new();
static STATE: OnceLock<Mutex<SystemState>> = OnceLock::new();

fn app_slot() -> &'static Mutex<Option<AppHandle>> {
    APP.get_or_init(|| Mutex::new(None))
}
fn state_slot() -> &'static Mutex<SystemState> {
    STATE.get_or_init(|| Mutex::new(SystemState::default()))
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemState {
    pub volume: f64,
    pub muted: bool,
    pub brightness: Option<f64>,
    pub brightness_supported: bool,
    pub wifi: Option<bool>,
    pub wifi_available: bool,
    pub bluetooth: Option<bool>,
    pub bluetooth_available: bool,
    pub error: String,
    pub updated_at: i64,
}

fn publish(app: &AppHandle, state: SystemState) {
    if let Ok(mut guard) = state_slot().lock() {
        *guard = state.clone();
    }
    let _ = app.emit(SYSTEM_EVENT, state);
}

#[cfg(target_os = "windows")]
fn endpoint_volume() -> Result<windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume, String> {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("音频设备枚举失败: {e}"))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .map_err(|e| format!("没有默认音频输出设备: {e}"))?;
        device
            .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            .map_err(|e| format!("读取音量控制失败: {e}"))
    }
}

#[cfg(target_os = "windows")]
fn read_volume() -> Result<(f64, bool), String> {
    unsafe {
        let ep = endpoint_volume()?;
        let volume = ep
            .GetMasterVolumeLevelScalar()
            .map_err(|e| format!("读取音量失败: {e}"))? as f64;
        let muted = ep
            .GetMute()
            .map_err(|e| format!("读取静音状态失败: {e}"))?
            .as_bool();
        Ok((volume.clamp(0.0, 1.0), muted))
    }
}

#[cfg(target_os = "macos")]
fn read_volume() -> Result<(f64, bool), String> {
    mac_audio::get().ok_or_else(|| "当前输出设备不提供系统音量".to_string())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn read_volume() -> Result<(f64, bool), String> {
    Err("当前平台不支持系统音量".into())
}

#[cfg(target_os = "windows")]
fn set_volume_inner(volume: f64) -> Result<(), String> {
    unsafe {
        let ep = endpoint_volume()?;
        ep.SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0) as f32, std::ptr::null())
            .map_err(|e| format!("设置音量失败: {e}"))
    }
}

#[cfg(target_os = "macos")]
fn set_volume_inner(volume: f64) -> Result<(), String> {
    mac_audio::set_volume(volume)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn set_volume_inner(_volume: f64) -> Result<(), String> {
    Err("当前平台不支持系统音量".into())
}

#[cfg(target_os = "windows")]
fn set_mute_inner(muted: bool) -> Result<(), String> {
    unsafe {
        let ep = endpoint_volume()?;
        ep.SetMute(muted, std::ptr::null())
            .map_err(|e| format!("设置静音失败: {e}"))
    }
}

#[cfg(target_os = "macos")]
fn set_mute_inner(muted: bool) -> Result<(), String> {
    mac_audio::set_muted(muted)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn set_mute_inner(_muted: bool) -> Result<(), String> {
    Err("当前平台不支持系统静音".into())
}

/// macOS 音量 / 静音：CoreAudio HAL 的 `kAudioDevicePropertyVolumeScalar` /
/// `kAudioDevicePropertyMute`，和系统音量键、菜单栏滑块改的是同一个属性，
/// 所以改完立刻能回读，也不需要 AppleScript（不会弹「自动化」授权框，
/// 单次调用是微秒级，而 `osascript` 每次要起一个进程）。
///
/// 内置扬声器 / 耳机 / AirPods 走 element 0（main）；少数 USB DAC 与 HDMI
/// 只暴露左右声道（element 1、2），按同样的顺序回退。
#[cfg(target_os = "macos")]
mod mac_audio {
    use std::ffi::c_void;

    type OSStatus = i32;
    type AudioObjectID = u32;
    type AudioDeviceID = u32;

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    /// 四字符码（'vmvc' 之类），CoreAudio 的属性选择器/作用域都是这个东西。
    const fn fourcc(text: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*text)
    }

    const SYSTEM_OBJECT: AudioObjectID = 1;
    const SCOPE_GLOBAL: u32 = fourcc(b"glob");
    const SCOPE_OUTPUT: u32 = fourcc(b"outp");
    const ELEMENT_MAIN: u32 = 0;
    const PROP_DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
    const PROP_VOLUME: u32 = fourcc(b"vmvc");
    const PROP_MUTE: u32 = fourcc(b"mute");

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyData(
            object: AudioObjectID,
            address: *const AudioObjectPropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> OSStatus;
        fn AudioObjectSetPropertyData(
            object: AudioObjectID,
            address: *const AudioObjectPropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: u32,
            data: *const c_void,
        ) -> OSStatus;
        fn AudioObjectHasProperty(
            object: AudioObjectID,
            address: *const AudioObjectPropertyAddress,
        ) -> u8;
    }

    /// 默认输出设备；没有输出设备（纯外接显卡且没接音箱等）时为 None。
    fn default_output() -> Option<AudioDeviceID> {
        let address = AudioObjectPropertyAddress {
            selector: PROP_DEFAULT_OUTPUT,
            scope: SCOPE_GLOBAL,
            element: ELEMENT_MAIN,
        };
        let mut device: AudioDeviceID = 0;
        let mut size = std::mem::size_of::<AudioDeviceID>() as u32;
        let rc = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                &mut device as *mut AudioDeviceID as *mut c_void,
            )
        };
        (rc == 0 && device != 0).then_some(device)
    }

    fn volume_address(element: u32) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            selector: PROP_VOLUME,
            scope: SCOPE_OUTPUT,
            element,
        }
    }

    fn mute_address() -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress {
            selector: PROP_MUTE,
            scope: SCOPE_OUTPUT,
            element: ELEMENT_MAIN,
        }
    }

    fn has(device: AudioDeviceID, address: &AudioObjectPropertyAddress) -> bool {
        unsafe { AudioObjectHasProperty(device, address) != 0 }
    }

    fn read_scalar(device: AudioDeviceID, address: &AudioObjectPropertyAddress) -> Option<f32> {
        let mut value: f32 = 0.0;
        let mut size = std::mem::size_of::<f32>() as u32;
        let rc = unsafe {
            AudioObjectGetPropertyData(
                device,
                address,
                0,
                std::ptr::null(),
                &mut size,
                &mut value as *mut f32 as *mut c_void,
            )
        };
        (rc == 0).then_some(value)
    }

    fn write_scalar(device: AudioDeviceID, address: &AudioObjectPropertyAddress, value: f32) -> OSStatus {
        unsafe {
            AudioObjectSetPropertyData(
                device,
                address,
                0,
                std::ptr::null(),
                std::mem::size_of::<f32>() as u32,
                &value as *const f32 as *const c_void,
            )
        }
    }

    /// 依次尝试 main / 左 / 右声道，取第一个可读的元素。
    fn any_volume<F, T>(device: AudioDeviceID, mut f: F) -> Option<T>
    where
        F: FnMut(&AudioObjectPropertyAddress) -> Option<T>,
    {
        for element in [ELEMENT_MAIN, 1, 2] {
            let address = volume_address(element);
            if !has(device, &address) {
                continue;
            }
            if let Some(value) = f(&address) {
                return Some(value);
            }
        }
        None
    }

    pub fn get() -> Option<(f64, bool)> {
        let device = default_output()?;
        let volume = any_volume(device, |address| read_scalar(device, address))? as f64;
        let mut muted = false;
        let address = mute_address();
        if has(device, &address) {
            let mut value: u32 = 0;
            let mut size = std::mem::size_of::<u32>() as u32;
            let rc = unsafe {
                AudioObjectGetPropertyData(
                    device,
                    &address,
                    0,
                    std::ptr::null(),
                    &mut size,
                    &mut value as *mut u32 as *mut c_void,
                )
            };
            muted = rc == 0 && value != 0;
        }
        Some((volume.clamp(0.0, 1.0), muted))
    }

    pub fn set_volume(volume: f64) -> Result<(), String> {
        let device = default_output().ok_or_else(|| "没有默认音频输出设备".to_string())?;
        let target = volume.clamp(0.0, 1.0) as f32;
        let mut last = 0;
        for element in [ELEMENT_MAIN, 1, 2] {
            let address = volume_address(element);
            if !has(device, &address) {
                continue;
            }
            last = write_scalar(device, &address, target);
            if last == 0 {
                return Ok(());
            }
        }
        Err(format!(
            "设置音量失败（CoreAudio 返回 {last}）：当前输出设备不支持软件音量"
        ))
    }

    pub fn set_muted(muted: bool) -> Result<(), String> {
        let device = default_output().ok_or_else(|| "没有默认音频输出设备".to_string())?;
        let address = mute_address();
        if !has(device, &address) {
            return Err("当前输出设备不支持静音".into());
        }
        let value: u32 = u32::from(muted);
        let rc = unsafe {
            AudioObjectSetPropertyData(
                device,
                &address,
                0,
                std::ptr::null(),
                std::mem::size_of::<u32>() as u32,
                &value as *const u32 as *const c_void,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("设置静音失败（CoreAudio 返回 {rc}）"))
        }
    }
}

fn run_powershell(script: &str) -> Option<String> {
    use std::process::Command;
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
pub fn read_brightness() -> Option<f64> {
    mac_brightness::get()
}

#[cfg(not(target_os = "macos"))]
pub fn read_brightness() -> Option<f64> {
    #[cfg(target_os = "windows")]
    if let Some(value) = ddc::read() {
        return Some(value);
    }
    let text = run_powershell(
        "(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness -ErrorAction Stop | Select-Object -First 1).CurrentBrightness",
    )?;
    text.lines()
        .find_map(|line| line.trim().parse::<f64>().ok())
        .filter(|v| (0.0..=100.0).contains(v))
}

#[cfg(target_os = "macos")]
fn set_brightness_inner(value: f64) -> Result<f64, String> {
    mac_brightness::set(value.clamp(0.0, 100.0).round())
}

#[cfg(not(target_os = "macos"))]
fn set_brightness_inner(value: f64) -> Result<f64, String> {
    let value = value.clamp(0.0, 100.0).round();
    // Twinkle 路径有效时优先走它的命令行（`--All --Set`），成功即返回，
    // 不再叠加 DDC/WMI（双重设置会让滑块位置和实际亮度打架）；路径为空
    // 或调用失败时保持原来的 DDC/CI → WMI 兜底。
    #[cfg(target_os = "windows")]
    if let Some(exe) = twinkle::exe() {
        match twinkle::set(&exe, value) {
            Ok(()) => return Ok(value),
            Err(e) => eprintln!("[zbar-island-system] Twinkle 设置亮度失败，回退 DDC/CI: {e}"),
        }
    }
    #[cfg(target_os = "windows")]
    if ddc::set(value)? {
        return Ok(value);
    }
    let script = format!(
        "(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods -ErrorAction Stop | Select-Object -First 1).WmiSetBrightness(1, {value})"
    );
    run_powershell(&script)
        .ok_or_else(|| "设置亮度失败（显示器未启用 DDC/CI，且系统没有 WMI 亮度接口）".to_string())?;
    Ok(value)
}

/// Twinkle Tray 命令行亮度（Windows-only）：便携版 / 安装版设置页里填的
/// 可执行文件路径在这里被消费。Twinkle 自己最终也是走 DDC/CI 或 WMI，
/// 所以 `read_brightness` 不用改——回读到的仍是显示器真实亮度。
///
/// 三个约束都来自 Twinkle 便携版的行为（Electron Portable，每次启动会先
/// 解包并弹「安装中」进度）：
///
/// - **串行**：拖动滑块时前端 120ms 一拍，并发拉起多个 CLI 进程会互相
///   覆盖参数；已有调用在跑时本次直接跳过（下一拍 / 松手后还会再调）；
/// - **去重**：同一个整数亮度在 800ms 内只真正发一次；
/// - **不 kill**：首次启动可能在解包，等到上限还没退出就按「已受理」处理，
///   kill 掉会中断参数转发；
/// - **退出码**：exe 存在但执行失败（参数不兼容 / 无权限等）时进程会以非 0
///   退出，必须报给调用方才能回退 DDC/CI → WMI，不能假装设置成功。
#[cfg(target_os = "windows")]
mod twinkle {
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// 串行锁：同一时间只允许一个 Twinkle 调用在跑。
    static LOCK: Mutex<()> = Mutex::new(());
    /// 上次真正发出的整数亮度 + 时间（去重窗口内同值不再发）。
    static LAST: Mutex<Option<(i64, Instant)>> = Mutex::new(None);

    const DEDUP_WINDOW: Duration = Duration::from_millis(800);
    const WAIT_LIMIT: Duration = Duration::from_secs(5);
    const WAIT_POLL: Duration = Duration::from_millis(50);
    /// `CREATE_NO_WINDOW`：不闪控制台窗口（与 powershell 兜底同口径）。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// `--All --Set=<0-100>`：所有显示器一起调，与 DDC 分支「全部显示器都
    /// 写」保持同一语义。
    pub(super) fn args(value: i64) -> [String; 2] {
        let value = value.clamp(0, 100);
        ["--All".to_string(), format!("--Set={value}")]
    }

    /// 只接受「绝对路径 + `.exe` + 文件存在」：注册表残留、失效快捷方式、
    /// 手填的目录都在这里被挡掉。
    pub(super) fn usable(path: &Path) -> bool {
        path.is_absolute()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            && path.is_file()
    }

    /// `brightness.twinkle` → 可执行文件路径。
    ///
    /// trim + 去首尾引号（从资源管理器复制路径常见 `"C:\...\Twinkle.exe"`）
    /// + `%VAR%` 展开（与设置窗其它路径项同口径）；为空或无效返回 None，
    /// 调用方继续走 DDC/CI → WMI。`lookup` 只为单测注入环境变量。
    pub(super) fn resolve_with<F>(raw: &str, lookup: F) -> Option<PathBuf>
    where
        F: Fn(&str) -> Option<String>,
    {
        let raw = raw.trim().trim_matches('"').trim();
        if raw.is_empty() {
            return None;
        }
        let expanded = super::super::notify::expand_env_vars(raw, lookup);
        let path = PathBuf::from(expanded.trim());
        usable(&path).then_some(path)
    }

    fn resolve(raw: &str) -> Option<PathBuf> {
        resolve_with(raw, |name| std::env::var(name).ok())
    }

    /// 读设置文档里的 Twinkle 路径。每次调用都重新读盘：设置页保存后
    /// 下一拍亮度调用立刻生效，不用重启。
    pub(super) fn exe() -> Option<PathBuf> {
        resolve(&super::super::settings::load().brightness.twinkle)
    }

    /// 用 Twinkle 设置亮度；`exe` 由调用方先校验（`exe()`）。
    ///
    /// 串行锁被占 / 同值去重时返回 `Ok(())`——本次跳过不算失败，调用方
    /// 照常更新内存态即可。等到进程退出但退出码非 0 时返回 `Err`，调用方
    /// 据此回退 DDC/CI → WMI；5 秒内没退出（可能在解包 / 转发参数）仍按
    /// 「已受理」返回 `Ok(())`。
    pub(super) fn set(exe: &Path, value: f64) -> Result<(), String> {
        let value = value.clamp(0.0, 100.0).round() as i64;
        // 串行：已有调用在跑就直接跳过，绝不叠进程。
        let Ok(_running) = LOCK.try_lock() else {
            return Ok(());
        };
        // 去重：拖动时 120ms 一拍，同值 800ms 内只真正发一次。
        if let Ok(last) = LAST.lock() {
            if let Some((prev, at)) = *last {
                if prev == value && at.elapsed() < DEDUP_WINDOW {
                    return Ok(());
                }
            }
        }
        let mut command = Command::new(exe);
        command.args(args(value));
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .map_err(|e| format!("启动 Twinkle 失败（{}）: {e}", exe.display()))?;
        // 正常情况 Twinkle 把参数转给已运行实例后立刻退出；便携版首次
        // 启动在解包，最多等 5 秒，超时也不 kill（可能正在转发参数）。
        // 退出码必须检查：exe 存在但参数不兼容 / 无权限时进程会非 0 退出，
        // 此时 HUD 不能显示成功，也不写去重记录（下一拍要能重试 / 回退）。
        let started = Instant::now();
        let outcome: Option<Option<i32>> = loop {
            match child.try_wait() {
                // 已退出：交给下面统一判定退出码
                Ok(Some(status)) => break Some(status.code()),
                // 超时：按「已受理」处理，不 kill
                Ok(None) if started.elapsed() >= WAIT_LIMIT => break None,
                Ok(None) => std::thread::sleep(WAIT_POLL),
                // 等不到退出（极少见）：同样按「已受理」处理
                Err(_) => break None,
            }
        };
        if let Some(err) = outcome.flatten().and_then(exit_code_error) {
            return Err(err);
        }
        // 成功或超时（已受理）：记下去重窗口，避免拖动时反复拉起进程。
        if let Ok(mut last) = LAST.lock() {
            *last = Some((value, Instant::now()));
        }
        Ok(())
    }

    /// 退出码 → 错误：0 = 成功；非 0 = 执行失败（调用方回退 DDC/CI → WMI）。
    pub(super) fn exit_code_error(code: i32) -> Option<String> {
        (code != 0).then(|| format!("Twinkle 退出码 {code}"))
    }
}

/// macOS 亮度：走私有框架 DisplayServices（和系统亮度键同一套接口），
/// dlopen 在运行时加载，拿不到符号就退化为「不支持」。
/// 内置屏 / Apple 显示器可用；第三方外接屏若不支持会返回错误码。
#[cfg(target_os = "macos")]
mod mac_brightness {
    use std::ffi::{c_char, c_void, CString};
    use std::sync::OnceLock;

    type CGDirectDisplayID = u32;
    type GetFn = unsafe extern "C" fn(CGDirectDisplayID, *mut f32) -> i32;
    type SetFn = unsafe extern "C" fn(CGDirectDisplayID, f32) -> i32;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> CGDirectDisplayID;
        fn CGGetOnlineDisplayList(max: u32, displays: *mut CGDirectDisplayID, count: *mut u32)
            -> i32;
    }

    #[link(name = "System", kind = "dylib")]
    extern "C" {
        fn dlopen(filename: *const c_char, flag: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    const RTLD_LAZY: i32 = 1;
    const FRAMEWORK: &str = "/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices";

    struct Api {
        get: GetFn,
        set: SetFn,
    }

    // 只存函数指针，进程内常量，跨线程只读安全。
    unsafe impl Send for Api {}
    unsafe impl Sync for Api {}

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let path = CString::new(FRAMEWORK).ok()?;
            let handle = dlopen(path.as_ptr(), RTLD_LAZY);
            if handle.is_null() {
                return None;
            }
            let get_name = CString::new("DisplayServicesGetBrightness").ok()?;
            let set_name = CString::new("DisplayServicesSetBrightness").ok()?;
            let get_ptr = dlsym(handle, get_name.as_ptr());
            let set_ptr = dlsym(handle, set_name.as_ptr());
            if get_ptr.is_null() || set_ptr.is_null() {
                return None;
            }
            Some(Api {
                get: std::mem::transmute::<*mut c_void, GetFn>(get_ptr),
                set: std::mem::transmute::<*mut c_void, SetFn>(set_ptr),
            })
        })
        .as_ref()
    }

    /// 主屏优先；主屏不支持时依次试其它在线显示器（外接 Apple 屏）。
    fn displays() -> Vec<CGDirectDisplayID> {
        let mut list = [0u32; 8];
        let mut count: u32 = 0;
        let rc = unsafe { CGGetOnlineDisplayList(list.len() as u32, list.as_mut_ptr(), &mut count) };
        let mut out = vec![unsafe { CGMainDisplayID() }];
        if rc == 0 {
            for id in list.iter().take(count as usize) {
                if !out.contains(id) {
                    out.push(*id);
                }
            }
        }
        out
    }

    pub fn get() -> Option<f64> {
        let api = api()?;
        for id in displays() {
            let mut value: f32 = 0.0;
            if unsafe { (api.get)(id, &mut value) } == 0 {
                return Some((value as f64 * 100.0).clamp(0.0, 100.0));
            }
        }
        None
    }

    pub fn set(value: f64) -> Result<f64, String> {
        let api = api().ok_or_else(|| "系统亮度接口不可用".to_string())?;
        let target = (value / 100.0).clamp(0.0, 1.0) as f32;
        let mut last_code = 0;
        for id in displays() {
            let rc = unsafe { (api.set)(id, target) };
            if rc == 0 {
                return Ok((target as f64 * 100.0).round());
            }
            last_code = rc;
        }
        Err(format!(
            "设置亮度失败（返回码 {last_code}）：当前显示器可能不支持系统亮度控制"
        ))
    }
}

/// DDC/CI 亮度：外接显示器最通用的控制面。读取优先 `GetMonitorBrightness`
/// （直接返回 min/current/max），失败再走 VCP 0x10；写入同样两级回退。
/// 遍历所有物理显示器，读取取支持项的平均值，滑块写入时全部一起调。
#[cfg(target_os = "windows")]
mod ddc {
    use windows::Win32::Devices::Display::{
        DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
        GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply, SetMonitorBrightness,
        SetVCPFeature, PHYSICAL_MONITOR,
    };
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HANDLE, LPARAM, RECT, TRUE};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

    const VCP_BRIGHTNESS: u8 = 0x10;

    unsafe extern "system" fn collect_monitor(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<HMONITOR>);
        list.push(monitor);
        TRUE
    }

    fn monitors() -> Vec<HMONITOR> {
        let mut list: Vec<HMONITOR> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(collect_monitor),
                LPARAM((&mut list as *mut Vec<HMONITOR>) as isize),
            );
        }
        list
    }

    fn physical(monitor: HMONITOR) -> Vec<PHYSICAL_MONITOR> {
        let mut count = 0u32;
        if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count) }.is_err()
            || count == 0
        {
            return Vec::new();
        }
        let mut list = vec![PHYSICAL_MONITOR::default(); count as usize];
        if unsafe { GetPhysicalMonitorsFromHMONITOR(monitor, &mut list) }.is_err() {
            return Vec::new();
        }
        list
    }

    fn read_one(handle: HANDLE) -> Option<f64> {
        if handle.is_invalid() {
            return None;
        }
        let mut min = 0u32;
        let mut current = 0u32;
        let mut max = 0u32;
        if unsafe { GetMonitorBrightness(handle, &mut min, &mut current, &mut max) } != 0
            && max > min
        {
            return Some(((current.saturating_sub(min)) as f64 * 100.0 / (max - min) as f64)
                .clamp(0.0, 100.0));
        }
        let mut current = 0u32;
        let mut max = 0u32;
        if unsafe {
            GetVCPFeatureAndVCPFeatureReply(
                handle,
                VCP_BRIGHTNESS,
                None,
                &mut current,
                Some(&mut max),
            )
        } != 0
            && max > 0
        {
            return Some((current as f64 * 100.0 / max as f64).clamp(0.0, 100.0));
        }
        None
    }

    fn set_one(handle: HANDLE, value: f64) -> bool {
        if handle.is_invalid() {
            return false;
        }
        let mut min = 0u32;
        let mut current = 0u32;
        let mut max = 0u32;
        if unsafe { GetMonitorBrightness(handle, &mut min, &mut current, &mut max) } != 0
            && max > min
        {
            let raw = min + (value / 100.0 * (max - min) as f64).round() as u32;
            if unsafe { SetMonitorBrightness(handle, raw) } != 0 {
                return true;
            }
        }
        unsafe {
            SetVCPFeature(handle, VCP_BRIGHTNESS, value.round() as u32) != 0
        }
    }

    pub fn read() -> Option<f64> {
        let mut values = Vec::new();
        for monitor in monitors() {
            let mut list = physical(monitor);
            for item in &list {
                if let Some(value) = read_one(item.hPhysicalMonitor) {
                    values.push(value);
                }
            }
            if !list.is_empty() {
                unsafe {
                    let _ = DestroyPhysicalMonitors(&list);
                }
                list.clear();
            }
        }
        if values.is_empty() {
            None
        } else {
            Some(values.iter().sum::<f64>() / values.len() as f64)
        }
    }

    /// 返回是否至少有一台显示器写成功；全部失败时调用方回退 WMI。
    pub fn set(value: f64) -> Result<bool, String> {
        let mut applied = false;
        for monitor in monitors() {
            let list = physical(monitor);
            for item in &list {
                if set_one(item.hPhysicalMonitor, value) {
                    applied = true;
                }
            }
            if !list.is_empty() {
                unsafe {
                    let _ = DestroyPhysicalMonitors(&list);
                }
            }
        }
        Ok(applied)
    }
}

#[derive(Default)]
struct Radios {
    wifi: Option<bool>,
    bluetooth: Option<bool>,
}

#[cfg(target_os = "windows")]
fn read_radios() -> Radios {
    use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
    let mut out = Radios::default();
    let access = match Radio::RequestAccessAsync().and_then(|op| op.get()) {
        Ok(a) => a,
        Err(_) => return out,
    };
    if access != RadioAccessStatus::Allowed {
        return out;
    }
    let list = match Radio::GetRadiosAsync().and_then(|op| op.get()) {
        Ok(l) => l,
        Err(_) => return out,
    };
    let size = list.Size().unwrap_or(0);
    for i in 0..size {
        let Ok(radio) = list.GetAt(i) else { continue };
        let Ok(kind) = radio.Kind() else { continue };
        let on = radio
            .State()
            .map(|s| s == RadioState::On)
            .unwrap_or(false);
        if kind == RadioKind::WiFi {
            out.wifi = Some(on);
        } else if kind == RadioKind::Bluetooth {
            out.bluetooth = Some(on);
        }
    }
    out
}

#[cfg(not(target_os = "windows"))]
fn read_radios() -> Radios {
    Radios::default()
}

#[cfg(target_os = "windows")]
fn set_radio_inner(kind_name: &str, on: bool) -> Result<(), String> {
    use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
    let kind = match kind_name {
        "wifi" => RadioKind::WiFi,
        "bluetooth" => RadioKind::Bluetooth,
        _ => return Err("未知无线电类型".into()),
    };
    let access = Radio::RequestAccessAsync()
        .and_then(|op| op.get())
        .map_err(|e| format!("无线电权限请求失败: {e}"))?;
    if access != RadioAccessStatus::Allowed {
        return Err("系统未授予无线电访问权限，请到 Windows 设置里手动切换".into());
    }
    let list = Radio::GetRadiosAsync()
        .and_then(|op| op.get())
        .map_err(|e| format!("枚举无线电失败: {e}"))?;
    for i in 0..list.Size().unwrap_or(0) {
        let Ok(radio) = list.GetAt(i) else { continue };
        if radio.Kind().ok() == Some(kind) {
            let target = if on { RadioState::On } else { RadioState::Off };
            radio
                .SetStateAsync(target)
                .and_then(|op| op.get())
                .map_err(|e| format!("切换失败: {e}"))?;
            return Ok(());
        }
    }
    Err("没有找到对应无线电适配器".into())
}

#[cfg(not(target_os = "windows"))]
fn set_radio_inner(_kind_name: &str, _on: bool) -> Result<(), String> {
    Err("当前平台不支持无线电控制".into())
}

fn refresh_state() {
    let mut state = state_slot()
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    state.updated_at = chrono::Utc::now().timestamp_millis();
    match read_volume() {
        Ok((v, m)) => {
            state.volume = v;
            state.muted = m;
        }
        Err(e) => state.error = e,
    }
    let radios = read_radios();
    state.wifi_available = radios.wifi.is_some();
    state.wifi = radios.wifi;
    state.bluetooth_available = radios.bluetooth.is_some();
    state.bluetooth = radios.bluetooth;
    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
        publish(&app, state);
    }
}

pub fn start(app: AppHandle) {
    if let Ok(mut slot) = app_slot().lock() {
        *slot = Some(app.clone());
    }
    STOP.store(false, Ordering::SeqCst);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    thread::Builder::new()
        .name("zbar-island-system".into())
        .spawn(move || {
            // WMI 亮度查询较慢（起 PowerShell），只在后台线程首拍读一次，
            // 不能阻塞 setup / 主线程。
            let brightness = read_brightness();
            if let Ok(mut guard) = state_slot().lock() {
                guard.brightness = brightness;
                guard.brightness_supported = brightness.is_some();
            }
            let mut last_radio = std::time::Instant::now() - Duration::from_secs(10);
            let mut last_brightness = std::time::Instant::now();
            let mut first_tick = true;
            loop {
                if STOP.load(Ordering::Relaxed) || GEN.load(Ordering::SeqCst) != gen {
                    return;
                }
                let mut state = state_slot()
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_default();
                let before = state.clone();
                state.updated_at = chrono::Utc::now().timestamp_millis();
                match read_volume() {
                    Ok((v, m)) => {
                        if (state.volume - v).abs() > 0.001 || state.muted != m {
                            state.volume = v;
                            state.muted = m;
                            state.error.clear();
                        }
                    }
                    Err(e) => state.error = e,
                }
                if last_radio.elapsed() >= Duration::from_secs(5) {
                    last_radio = std::time::Instant::now();
                    let radios = read_radios();
                    state.wifi_available = radios.wifi.is_some();
                    state.wifi = radios.wifi;
                    state.bluetooth_available = radios.bluetooth.is_some();
                    state.bluetooth = radios.bluetooth;
                }
                // macOS 的 DisplayServices 读取是内存级调用，每拍回读一次，
                // 用亮度键改完立刻同步；Windows 的 DDC/CI / WMI 回读要几百
                // 毫秒，只对已确认支持的屏幕每 10s 读一次，不支持就不读。
                if last_brightness.elapsed() >= brightness_interval(state.brightness_supported) {
                    last_brightness = std::time::Instant::now();
                    if let Some(value) = read_brightness() {
                        state.brightness = Some(value);
                        state.brightness_supported = true;
                    }
                }
                let changed = first_tick
                    || (state.volume - before.volume).abs() > 0.0001
                    || state.muted != before.muted
                    || state.wifi != before.wifi
                    || state.bluetooth != before.bluetooth
                    || state.brightness != before.brightness
                    || state.brightness_supported != before.brightness_supported
                    || state.wifi_available != before.wifi_available
                    || state.bluetooth_available != before.bluetooth_available;
                first_tick = false;
                if changed {
                    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
                        publish(&app, state.clone());
                    }
                } else if let Ok(mut guard) = state_slot().lock() {
                    // 内容没变：只刷新 updatedAt，不广播，避免反复重渲染整岛
                    *guard = state;
                }
                thread::sleep(tick_interval());
            }
        })
        .ok();
}

/// 回读周期。macOS 的亮度和音量都是本地属性读取（微秒级），可以每拍都读，
/// 用系统快捷键改完 HUD 立刻跟上；Windows 的亮度要走 DDC/CI 或 PowerShell，
/// 只在已确认支持的屏幕上低频回读。
fn brightness_interval(supported: bool) -> Duration {
    #[cfg(target_os = "macos")]
    {
        if supported {
            Duration::ZERO
        } else {
            // 首次可能因为屏幕刚唤醒读到失败，低频重试
            Duration::from_secs(5)
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if supported {
            Duration::from_secs(10)
        } else {
            Duration::from_secs(3600)
        }
    }
}

fn tick_interval() -> Duration {
    #[cfg(target_os = "macos")]
    {
        Duration::from_millis(800)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Duration::from_millis(1800)
    }
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    GEN.fetch_add(1, Ordering::SeqCst);
}

#[tauri::command]
pub fn island_system() -> SystemState {
    state_slot().lock().map(|s| s.clone()).unwrap_or_default()
}

#[tauri::command]
pub async fn island_set_volume(volume: f64) -> Result<SystemState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        set_volume_inner(volume)?;
        refresh_state();
        Ok(island_system())
    })
    .await
    .map_err(|e| format!("音量任务失败: {e}"))?
}

#[tauri::command]
pub async fn island_toggle_mute() -> Result<SystemState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let muted = state_slot().lock().map(|s| s.muted).unwrap_or(false);
        set_mute_inner(!muted)?;
        refresh_state();
        Ok(island_system())
    })
    .await
    .map_err(|e| format!("静音任务失败: {e}"))?
}

#[tauri::command]
pub async fn island_set_brightness(value: f64) -> Result<SystemState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let v = set_brightness_inner(value)?;
        if let Ok(mut guard) = state_slot().lock() {
            guard.brightness = Some(v);
            guard.brightness_supported = true;
            guard.updated_at = chrono::Utc::now().timestamp_millis();
        }
        refresh_state();
        Ok(island_system())
    })
    .await
    .map_err(|e| format!("亮度任务失败: {e}"))?
}

#[tauri::command]
pub async fn island_toggle_radio(kind: String, on: bool) -> Result<SystemState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        set_radio_inner(&kind, on)?;
        thread::sleep(Duration::from_millis(350));
        refresh_state();
        Ok(island_system())
    })
    .await
    .map_err(|e| format!("无线电任务失败: {e}"))?
}

#[tauri::command]
pub fn island_open_system_settings(kind: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let target = match kind.as_str() {
        "wifi" => "x-apple.systempreferences:com.apple.wifi-settings-extension",
        "bluetooth" => "x-apple.systempreferences:com.apple.Bluetooth-Settings.extension",
        "display" => "x-apple.systempreferences:com.apple.Displays-Settings.extension",
        "sound" => "x-apple.systempreferences:com.apple.Sound-Settings.extension",
        _ => "x-apple.systempreferences:",
    };
    #[cfg(not(target_os = "macos"))]
    let target = match kind.as_str() {
        "wifi" => "ms-settings:network-wifi",
        "bluetooth" => "ms-settings:bluetooth",
        "display" => "ms-settings:display",
        "sound" => "ms-settings:sound",
        _ => "ms-settings:",
    };
    open::that(target).map_err(|e| format!("打开系统设置失败: {e}"))
}

#[cfg(all(test, target_os = "macos"))]
mod mac_audio_tests {
    /// 真机冒烟：读一次当前音量再原样写回，确认 CoreAudio HAL 通路可用。
    #[test]
    fn read_then_write_same_volume() {
        let (volume, _) = super::mac_audio::get().expect("读取音量失败");
        assert!((0.0..=1.0).contains(&volume), "音量越界: {volume}");
        super::mac_audio::set_volume(volume).expect("写回音量失败");
        let (again, _) = super::mac_audio::get().expect("回读音量失败");
        assert!(
            (again - volume).abs() <= 0.02,
            "写回后不一致: {volume} -> {again}"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod mac_brightness_tests {
    /// 真机冒烟：读一次当前亮度，再把它原样写回去，确认私有框架调用可用。
    #[test]
    fn read_then_write_same_value() {
        let current = super::mac_brightness::get().expect("读取亮度失败");
        assert!((0.0..=100.0).contains(&current), "亮度越界: {current}");
        let written = super::mac_brightness::set(current).expect("写回亮度失败");
        assert!(
            (written - current).abs() <= 2.0,
            "写回后不一致: {current} -> {written}"
        );
    }
}

#[cfg(all(test, target_os = "windows"))]
mod twinkle_tests {
    use super::twinkle::{args, exit_code_error, resolve_with, usable};
    use std::path::{Path, PathBuf};

    /// 用测试进程自身的 exe 当「真实存在的 .exe」样本，不依赖本机装没装 Twinkle。
    fn real_exe() -> PathBuf {
        std::env::current_exe().expect("测试进程自身路径")
    }

    fn no_env(_name: &str) -> Option<String> {
        None
    }

    #[test]
    fn usable_requires_absolute_existing_exe() {
        let exe = real_exe();
        assert!(usable(&exe), "测试进程自身应是合法 exe: {}", exe.display());
        assert!(!usable(Path::new("Twinkle.exe")), "相对路径要拒绝");
        assert!(!usable(Path::new(r"C:\Windows")), "目录要拒绝");
        assert!(
            !usable(Path::new(r"C:\Windows\notepad.txt")),
            "非 exe 扩展名要拒绝"
        );
        assert!(
            !usable(Path::new(r"C:\__agent_hud_no_such__\Twinkle.exe")),
            "不存在的文件要拒绝"
        );
    }

    #[test]
    fn resolve_trim_quotes_and_expand_env_vars() {
        let exe = real_exe();
        let name = exe
            .file_name()
            .expect("exe 文件名")
            .to_string_lossy()
            .to_string();
        let dir = exe.parent().expect("exe 父目录").display().to_string();

        // 空串 / 空白 / 未配置 → None（调用方回退 DDC/WMI）
        assert_eq!(resolve_with("", no_env), None);
        assert_eq!(resolve_with("   ", no_env), None);
        // 相对路径、不存在路径 → None
        assert_eq!(resolve_with("Twinkle.exe", no_env), None);
        assert_eq!(
            resolve_with(r"C:\__agent_hud_no_such__\Twinkle.exe", no_env),
            None
        );

        // 从资源管理器复制的带引号路径照样能解析
        let quoted = format!("  \"{}\"  ", exe.display());
        assert_eq!(resolve_with(&quoted, no_env), Some(exe.clone()));

        // `%VAR%` 展开后再做「存在性」校验
        let raw = format!("%AGENT_HUD_TEST_DIR%\\{name}");
        let lookup = |var: &str| (var == "AGENT_HUD_TEST_DIR").then(|| dir.clone());
        assert_eq!(resolve_with(&raw, lookup), Some(exe));
    }

    #[test]
    fn set_args_are_all_monitors_and_clamped() {
        assert_eq!(args(0), ["--All", "--Set=0"]);
        assert_eq!(args(37), ["--All", "--Set=37"]);
        assert_eq!(args(100), ["--All", "--Set=100"]);
        // 传入越界值也只会拼出合法整数（调用方已 clamp，这里再兜一层）
        assert_eq!(args(-1), ["--All", "--Set=0"]);
        assert_eq!(args(101), ["--All", "--Set=100"]);
    }

    /// 退出码判定：0 = 成功；非 0 必须报错（调用方据此回退 DDC/CI → WMI），
    /// 不能像旧实现那样把失败当成已受理。
    #[test]
    fn exit_code_decides_success_or_fallback() {
        assert_eq!(exit_code_error(0), None);
        assert_eq!(exit_code_error(1).as_deref(), Some("Twinkle 退出码 1"));
        // 崩溃 / 权限错误的退出码可能是负数（如 STATUS_ACCESS_DENIED 展开值）
        assert_eq!(
            exit_code_error(-1073741790).as_deref(),
            Some("Twinkle 退出码 -1073741790")
        );
    }
}
