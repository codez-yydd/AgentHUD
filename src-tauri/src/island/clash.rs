//! Clash Verge controls for the island, backed by the running Mihomo core on Windows.

use serde::{Deserialize, Serialize};

#[cfg(windows)]
#[path = "clash/controller.rs"]
mod controller;
#[cfg(windows)]
#[path = "clash/subscription.rs"]
mod subscription;
#[cfg(windows)]
#[path = "clash/telemetry.rs"]
mod telemetry;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Traffic {
    upload_speed: u64,
    download_speed: u64,
}

#[derive(Clone, Debug, Serialize)]
struct IpLocation {
    ip: String,
    region: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClashStatus {
    supported: bool,
    installed: bool,
    running: bool,
    mode: Option<&'static str>,
    mixed_port: Option<u16>,
    tun: Option<bool>,
    system_proxy: Option<bool>,
    can_set_system_proxy: bool,
    can_set_tun: bool,
    can_set_mode: bool,
    traffic: Option<Traffic>,
    #[cfg(windows)]
    subscription: Option<subscription::Subscription>,
    ip_location: Option<IpLocation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClashSwitch {
    SystemProxy,
    Tun,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClashMode {
    Rule,
    Global,
    Direct,
}

#[tauri::command]
pub async fn island_clash_status() -> Result<ClashStatus, String> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(windows_status::snapshot)
        .await
        .map_err(|e| format!("读取 Clash Verge 状态失败: {e}"));
    #[cfg(not(windows))]
    Ok(ClashStatus::default())
}

#[tauri::command]
pub async fn island_clash_open() -> Result<(), String> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(windows_status::open_app)
        .await
        .map_err(|e| format!("打开 Clash Verge 失败: {e}"))?;
    #[cfg(not(windows))]
    Err("Clash Verge 模块当前支持 Windows".into())
}

#[tauri::command]
pub async fn island_clash_set(kind: ClashSwitch, enabled: bool) -> Result<ClashStatus, String> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(move || {
        windows_status::set(windows_status::Change::Switch(kind, enabled))
    })
    .await
    .map_err(|e| format!("切换 Clash Verge 状态失败: {e}"))?;
    #[cfg(not(windows))]
    {
        let _ = (kind, enabled);
        Err("Clash Verge 模块当前支持 Windows".into())
    }
}

#[tauri::command]
pub async fn island_clash_set_mode(mode: ClashMode) -> Result<ClashStatus, String> {
    #[cfg(windows)]
    return tauri::async_runtime::spawn_blocking(move || {
        windows_status::set(windows_status::Change::Mode(mode))
    })
    .await
    .map_err(|e| format!("切换 Clash Verge 代理模式失败: {e}"))?;
    #[cfg(not(windows))]
    {
        let _ = mode;
        Err("Clash Verge 模块当前支持 Windows".into())
    }
}

/// 启动自动探测「Clash Verge 程序」路径（设置里从未填过时用）：默认配置
/// （写死的卸载表 subkey / 常见目录）→ 通用探测（运行进程 / 按 DisplayName
/// 枚举卸载表 / 常见目录）→ 运行进程反查兜底。非 Windows 返回 None。
pub(super) fn detect_app_path() -> Option<String> {
    #[cfg(windows)]
    return windows_status::detect_app_path();
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
mod windows_status {
    use super::{controller, subscription, telemetry, ClashMode, ClashStatus, ClashSwitch};
    use crate::island::{notify, settings};
    use serde::Deserialize;
    use std::collections::hash_map::DefaultHasher;
    use std::fs::File;
    use std::hash::{Hash, Hasher};
    use std::io::Read;
    use std::net::{SocketAddr, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Networking::WinInet::{
        InternetQueryOptionW, InternetSetOptionW, INTERNET_OPTION_PER_CONNECTION_OPTION,
        INTERNET_OPTION_PROXY_SETTINGS_CHANGED, INTERNET_OPTION_REFRESH, INTERNET_PER_CONN_FLAGS,
        INTERNET_PER_CONN_OPTIONW, INTERNET_PER_CONN_OPTIONW_0, INTERNET_PER_CONN_OPTION_LISTW,
        INTERNET_PER_CONN_PROXY_SERVER, PROXY_TYPE_DIRECT, PROXY_TYPE_PROXY,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };
    use winreg::RegKey;

    const APP_EXE: &str = "clash-verge.exe";
    const APP_DIR: &str = "io.github.clash-verge-rev.clash-verge-rev";
    const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Clash Verge";
    const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    static SWITCH: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct Processes {
        app_pid: Option<u32>,
        core: bool,
    }

    fn processes() -> Processes {
        let mut found = Processes::default();
        let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
            return found;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        unsafe {
            let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
            while more {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                if name.eq_ignore_ascii_case(APP_EXE) {
                    found.app_pid = Some(entry.th32ProcessID);
                } else if name.eq_ignore_ascii_case("verge-mihomo.exe")
                    || name.eq_ignore_ascii_case("verge-mihomo-alpha.exe")
                {
                    found.core = true;
                }
                if found.app_pid.is_some() && found.core {
                    break;
                }
                more = Process32NextW(snapshot, &mut entry).is_ok();
            }
            let _ = CloseHandle(snapshot);
        }
        found
    }

    fn process_path(pid: u32) -> Option<PathBuf> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut name = vec![0u16; 32_768];
        let mut len = name.len() as u32;
        let result = unsafe {
            let result = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(name.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(process);
            result
        };
        result.ok()?;
        existing_exe(PathBuf::from(String::from_utf16_lossy(
            &name[..len as usize],
        )))
    }

    fn expanded_path(value: &str) -> PathBuf {
        PathBuf::from(notify::expand_env_vars(
            value.trim().trim_matches('"'),
            |name| std::env::var(name).ok(),
        ))
    }

    fn existing_exe(path: PathBuf) -> Option<PathBuf> {
        (path.is_absolute()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            && path.is_file())
        .then_some(path)
    }

    fn installed_path(config: &settings::ClashDoc) -> Option<PathBuf> {
        existing_exe(expanded_path(&config.app))
            .or_else(|| {
                [
                    (HKEY_CURRENT_USER, KEY_READ),
                    (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY),
                    (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY),
                ]
                .into_iter()
                .find_map(|(hive, access)| {
                    let key = RegKey::predef(hive)
                        .open_subkey_with_flags(UNINSTALL_KEY, access)
                        .ok()?;
                    let dir = key.get_value::<String, _>("InstallLocation").ok()?;
                    existing_exe(expanded_path(&dir).join(APP_EXE))
                })
            })
            .or_else(|| {
                ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
                    .into_iter()
                    .filter_map(std::env::var_os)
                    .map(PathBuf::from)
                    .flat_map(|dir| [dir.join("Clash Verge"), dir.join("Programs/Clash Verge")])
                    .find_map(|dir| existing_exe(dir.join(APP_EXE)))
            })
    }

    /// 启动自动探测「Clash Verge 程序」路径：默认配置（写死的卸载表 subkey /
    /// 常见目录）→ 通用探测（运行进程、按 DisplayName 枚举卸载表、常见目录）
    /// → 运行进程反查兜底。Clash Verge Rev 的卸载项 subkey 是 GUID，只能按
    /// DisplayName 枚举（见 `app_paths`）。
    pub(super) fn detect_app_path() -> Option<String> {
        installed_path(&settings::ClashDoc::default())
            .or_else(|| {
                super::super::app_paths::detect_exe(
                    &[APP_EXE],
                    &["Clash Verge"],
                    &[
                        ("ProgramFiles", &["Clash Verge", APP_EXE][..]),
                        ("ProgramFiles(x86)", &["Clash Verge", APP_EXE][..]),
                        ("LOCALAPPDATA", &["Programs", "Clash Verge", APP_EXE][..]),
                        ("LOCALAPPDATA", &["Clash Verge", APP_EXE][..]),
                    ],
                )
            })
            .or_else(|| processes().app_pid.and_then(process_path))
            .and_then(|path| path.to_str().map(str::to_string))
    }

    fn config_dir(config: &settings::ClashDoc) -> Option<PathBuf> {
        let configured = expanded_path(&config.config_dir);
        if configured.is_absolute() && configured.is_dir() {
            return Some(configured);
        }
        [dirs::config_dir(), dirs::data_local_dir()]
            .into_iter()
            .flatten()
            .map(|dir| dir.join(APP_DIR))
            .find(|dir| dir.is_dir())
    }

    #[derive(Default, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct RuntimeConfig {
        mode: Option<String>,
        mixed_port: Option<u16>,
        tun: Option<TunConfig>,
        external_controller: Option<String>,
        external_controller_pipe: Option<String>,
        secret: Option<String>,
    }

    #[derive(Deserialize)]
    struct TunConfig {
        enable: Option<bool>,
    }

    fn runtime_config(dir: &Path) -> RuntimeConfig {
        ["clash-verge.yaml", "config.yaml"]
            .iter()
            .find_map(|name| {
                let mut text = String::new();
                File::open(dir.join(name))
                    .ok()?
                    .take(4 * 1024 * 1024)
                    .read_to_string(&mut text)
                    .ok()?;
                yaml_serde::from_str(text.trim_start_matches('\u{feff}')).ok()
            })
            .unwrap_or_default()
    }

    fn mode_label(mode: &str) -> Option<&'static str> {
        match mode.to_ascii_lowercase().as_str() {
            "rule" => Some("rule"),
            "global" => Some("global"),
            "direct" => Some("direct"),
            "script" => Some("script"),
            _ => None,
        }
    }

    impl RuntimeConfig {
        fn controller(&self) -> Option<controller::Controller> {
            controller::Controller::new(
                self.external_controller.as_deref(),
                self.external_controller_pipe.as_deref(),
                self.secret.as_deref(),
            )
        }

        fn tun_enabled(&self) -> Option<bool> {
            self.tun.as_ref()?.enable
        }

        fn mode(&self) -> Option<&'static str> {
            self.mode.as_deref().and_then(mode_label)
        }
    }

    fn live_config(endpoint: &controller::Controller) -> Result<RuntimeConfig, String> {
        let response = controller::request(endpoint, "GET", "/configs", None)?;
        serde_json::from_value(response).map_err(|_| "无法读取 Mihomo 的运行配置".to_string())
    }

    fn proxy_points_to_clash(value: &str, port: u16) -> bool {
        value.split(';').any(|part| {
            let address = part
                .split_once('=')
                .map_or(part, |(_, address)| address)
                .trim();
            let address = address
                .split_once("://")
                .map_or(address, |(_, address)| address);
            let Some((host, proxy_port)) = address.rsplit_once(':') else {
                return false;
            };
            (host == "127.0.0.1" || host == "[::1]" || host.eq_ignore_ascii_case("localhost"))
                && proxy_port.parse::<u16>().ok() == Some(port)
        })
    }

    fn system_proxy(port: u16) -> Option<bool> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(INTERNET_SETTINGS)
            .ok()?;
        let enabled = key.get_value::<u32, _>("ProxyEnable").unwrap_or(0) != 0;
        let address = key
            .get_value::<String, _>("ProxyServer")
            .unwrap_or_default();
        Some(enabled && proxy_points_to_clash(&address, port))
    }

    fn proxy_flag(flags: u32, enabled: bool) -> u32 {
        if enabled {
            flags | PROXY_TYPE_PROXY | PROXY_TYPE_DIRECT
        } else {
            (flags & !PROXY_TYPE_PROXY) | PROXY_TYPE_DIRECT
        }
    }

    fn set_system_proxy(port: u16, enabled: bool) -> Result<(), String> {
        let current = system_proxy(port).ok_or("无法读取 Windows 系统代理状态")?;
        if current == enabled {
            return Ok(());
        }
        if enabled {
            let address = SocketAddr::from(([127, 0, 0, 1], port));
            TcpStream::connect_timeout(&address, Duration::from_millis(250))
                .map_err(|_| "Clash Verge 的混合端口未在本机监听")?;
        }

        let mut server: Vec<u16> = format!("127.0.0.1:{port}\0").encode_utf16().collect();
        let mut options = [
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_FLAGS,
                ..Default::default()
            },
            INTERNET_PER_CONN_OPTIONW {
                dwOption: INTERNET_PER_CONN_PROXY_SERVER,
                Value: INTERNET_PER_CONN_OPTIONW_0 {
                    pszValue: PWSTR(server.as_mut_ptr()),
                },
            },
        ];
        let mut list = INTERNET_PER_CONN_OPTION_LISTW {
            dwSize: std::mem::size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32,
            dwOptionCount: 1,
            pOptions: options.as_mut_ptr(),
            ..Default::default()
        };
        let mut size = list.dwSize;
        unsafe {
            InternetQueryOptionW(
                None,
                INTERNET_OPTION_PER_CONNECTION_OPTION,
                Some((&mut list as *mut INTERNET_PER_CONN_OPTION_LISTW).cast()),
                &mut size,
            )
            .map_err(|e| format!("无法查询 Windows 代理选项: {e}"))?;
            let flags = options[0].Value.dwValue;
            options[0].Value = INTERNET_PER_CONN_OPTIONW_0 {
                dwValue: proxy_flag(flags, enabled),
            };
            list.pOptions = options.as_mut_ptr();
            list.dwOptionCount = if enabled { 2 } else { 1 };
            InternetSetOptionW(
                None,
                INTERNET_OPTION_PER_CONNECTION_OPTION,
                Some((&list as *const INTERNET_PER_CONN_OPTION_LISTW).cast()),
                list.dwSize,
            )
            .map_err(|e| format!("切换 Windows 系统代理失败: {e}"))?;
            let _ = InternetSetOptionW(None, INTERNET_OPTION_PROXY_SETTINGS_CHANGED, None, 0);
            let _ = InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0);
        }
        if system_proxy(port) != Some(enabled) {
            return Err("Windows 系统代理未切换到请求的状态".into());
        }
        Ok(())
    }

    pub(super) fn snapshot() -> ClashStatus {
        let config = settings::load().clash;
        let found = processes();
        let dir = config_dir(&config);
        let saved = dir.as_deref().map(runtime_config).unwrap_or_default();
        let endpoint = found.core.then(|| saved.controller()).flatten();
        let live = endpoint
            .as_ref()
            .and_then(|endpoint| live_config(endpoint).ok());
        status(
            &config,
            &found,
            dir.as_deref(),
            &saved,
            endpoint.as_ref(),
            live.as_ref(),
        )
    }

    fn mixed_port(
        config: &settings::ClashDoc,
        saved: &RuntimeConfig,
        live: Option<&RuntimeConfig>,
    ) -> Option<u16> {
        live.and_then(|live| live.mixed_port)
            .or(saved.mixed_port)
            .or_else(|| u16::try_from(config.proxy_port).ok())
            .filter(|port| *port > 0)
    }

    fn status(
        config: &settings::ClashDoc,
        processes: &Processes,
        dir: Option<&Path>,
        saved: &RuntimeConfig,
        endpoint: Option<&controller::Controller>,
        live: Option<&RuntimeConfig>,
    ) -> ClashStatus {
        let installed =
            processes.app_pid.is_some() || processes.core || installed_path(config).is_some();
        let subscription = dir.and_then(subscription::load);
        if !processes.core {
            telemetry::clear();
            return ClashStatus {
                supported: true,
                installed,
                subscription,
                ..Default::default()
            };
        }

        let mixed_port = mixed_port(config, saved, live);
        let confirmed_mode = live.and_then(RuntimeConfig::mode);
        let mode = confirmed_mode.or_else(|| saved.mode());
        let confirmed_tun = live.and_then(RuntimeConfig::tun_enabled);
        let tun = confirmed_tun.or_else(|| saved.tun_enabled());
        let system_proxy = mixed_port.and_then(system_proxy);
        let endpoint_key = endpoint.map(controller::Controller::identity);
        let mut hasher = DefaultHasher::new();
        (endpoint_key, mixed_port, mode, tun, system_proxy).hash(&mut hasher);

        ClashStatus {
            supported: true,
            installed,
            running: true,
            mode,
            mixed_port,
            tun,
            system_proxy,
            can_set_system_proxy: system_proxy.is_some(),
            can_set_tun: confirmed_tun.is_some(),
            can_set_mode: confirmed_mode.is_some(),
            traffic: endpoint.and_then(telemetry::traffic),
            ip_location: mixed_port.and_then(|port| telemetry::egress(port, hasher.finish())),
            subscription,
        }
    }

    pub(super) enum Change {
        Switch(ClashSwitch, bool),
        Mode(ClashMode),
    }

    pub(super) fn set(change: Change) -> Result<ClashStatus, String> {
        let _switch = SWITCH.lock().map_err(|_| "Clash Verge 控制暂不可用")?;
        let found = processes();
        if !found.core {
            return Err("Clash Verge 内核未运行".into());
        }
        let config = settings::load().clash;
        let dir = config_dir(&config);
        let saved = dir.as_deref().map(runtime_config).unwrap_or_default();
        let endpoint = saved.controller();
        let mut live = endpoint
            .as_ref()
            .and_then(|endpoint| live_config(endpoint).ok());

        match change {
            Change::Switch(ClashSwitch::SystemProxy, enabled) => {
                let port = mixed_port(&config, &saved, live.as_ref())
                    .ok_or("未发现 Clash Verge 的混合端口")?;
                set_system_proxy(port, enabled)?;
            }
            Change::Switch(ClashSwitch::Tun, enabled) => {
                let endpoint = endpoint.as_ref().ok_or("未发现 Mihomo 本地控制器")?;
                let confirmed = live
                    .as_ref()
                    .and_then(RuntimeConfig::tun_enabled)
                    .ok_or("无法读取 TUN 的实际状态，请检查 Mihomo 本地控制器")?;
                if confirmed != enabled {
                    let update = serde_json::json!({"tun": {"enable": enabled}});
                    controller::request(endpoint, "PATCH", "/configs", Some(&update))?;
                    live = Some(live_config(endpoint)?);
                    if live.as_ref().and_then(RuntimeConfig::tun_enabled) != Some(enabled) {
                        return Err(
                            "TUN 未切换到请求的状态，请检查 Clash Verge 服务模式或管理员权限"
                                .into(),
                        );
                    }
                }
            }
            Change::Mode(mode) => {
                let endpoint = endpoint.as_ref().ok_or("未发现 Mihomo 本地控制器")?;
                let confirmed = live
                    .as_ref()
                    .and_then(RuntimeConfig::mode)
                    .ok_or("无法读取当前代理模式，请检查 Mihomo 本地控制器")?;
                let requested = match mode {
                    ClashMode::Rule => "rule",
                    ClashMode::Global => "global",
                    ClashMode::Direct => "direct",
                };
                if confirmed != requested {
                    let update = serde_json::json!({"mode": requested});
                    controller::request(endpoint, "PATCH", "/configs", Some(&update))?;
                    live = Some(live_config(endpoint)?);
                    if live.as_ref().and_then(RuntimeConfig::mode) != Some(requested) {
                        return Err("Mihomo 未切换到请求的代理模式".into());
                    }
                }
            }
        }
        telemetry::clear();
        Ok(status(
            &config,
            &found,
            dir.as_deref(),
            &saved,
            endpoint.as_ref(),
            live.as_ref(),
        ))
    }

    pub(super) fn open_app() -> Result<(), String> {
        if notify::focus_running_app(APP_EXE) {
            return Ok(());
        }
        let config = settings::load().clash;
        let path = installed_path(&config)
            .or_else(|| processes().app_pid.and_then(process_path))
            .ok_or_else(|| {
                "找不到 Clash Verge，请在灵动岛设置的「系统与控制」中填写程序路径".to_string()
            })?;
        open::that(path).map_err(|e| format!("打开 Clash Verge 失败: {e}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_generated_runtime_fields_and_ignores_other_sections() {
            let config: RuntimeConfig = yaml_serde::from_str("mode: 'global' # profile\nmixed-port: 7897\ndns:\n  enable: false\ntun: {enable: true}\nproxies:\n- mode: rule\n  mixed-port: 10\n").unwrap();
            assert_eq!(config.mode.as_deref().and_then(mode_label), Some("global"));
            assert_eq!(config.mixed_port, Some(7897));
            assert_eq!(config.tun_enabled(), Some(true));
        }

        #[test]
        fn toggles_fixed_proxy_without_resetting_pac_flags() {
            let automatic = 4 | 8;
            let enabled = proxy_flag(automatic, true);
            assert_eq!(enabled, automatic | PROXY_TYPE_DIRECT | PROXY_TYPE_PROXY);
            assert_eq!(proxy_flag(enabled, false), automatic | PROXY_TYPE_DIRECT);
        }

        #[test]
        fn recognizes_only_the_expected_local_system_proxy() {
            assert!(proxy_points_to_clash(
                "http=127.0.0.1:7897;https=localhost:7897",
                7897
            ));
            assert!(proxy_points_to_clash("http://[::1]:7897", 7897));
            assert!(!proxy_points_to_clash("127.0.0.10:7897", 7897));
            assert!(!proxy_points_to_clash("127.0.0.1:7890", 7897));
        }
    }
}
