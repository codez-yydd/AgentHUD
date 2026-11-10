//! 灵动岛系统控制：音量 / 静音 / 亮度 / Wi-Fi / 蓝牙。
//!
//! - 音量：Windows Core Audio（IAudioEndpointVolume），后台线程每 2 秒读一次，
//!   外部改音量也会同步到岛里；
//! - 亮度：WMI `WmiMonitorBrightness`（笔记本可用；台式机通常不支持，返回空）；
//! - Wi-Fi / 蓝牙：WinRT `Windows.Devices.Radios.Radio`（需要系统无线电访问许可，
//!   首次切换时系统可能弹权限框；被拒绝时前端会提示改走系统设置）。

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

#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
fn set_mute_inner(_muted: bool) -> Result<(), String> {
    Err("当前平台不支持系统静音".into())
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
                // DDC/CI 读取很快；只对已经确认支持的屏幕每 10s 回读一次，
                // 让用户用显示器实体按键改亮度后 HUD 也能跟上。不支持时
                // 不轮询，避免每次走 WMI/PowerShell 拖慢后台线程。
                if state.brightness_supported && last_brightness.elapsed() >= Duration::from_secs(10)
                {
                    last_brightness = std::time::Instant::now();
                    if let Some(value) = read_brightness() {
                        state.brightness = Some(value);
                    }
                }
                let changed = first_tick
                    || (state.volume - before.volume).abs() > 0.0001
                    || state.muted != before.muted
                    || state.wifi != before.wifi
                    || state.bluetooth != before.bluetooth
                    || state.brightness != before.brightness
                    || state.wifi_available != before.wifi_available
                    || state.bluetooth_available != before.bluetooth_available;
                first_tick = false;
                if changed {
                    if let Some(app) = app_slot().lock().ok().and_then(|g| g.clone()) {
                        publish(&app, state.clone());
                    }
                } else if let Ok(mut guard) = state_slot().lock() {
                    // 内容没变：只刷新 updatedAt，不广播，避免每 1.8s 重渲染整岛
                    *guard = state;
                }
                thread::sleep(Duration::from_millis(1800));
            }
        })
        .ok();
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
