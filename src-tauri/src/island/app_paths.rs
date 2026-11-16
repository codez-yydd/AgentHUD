//! 本机已安装程序的自动探测（Windows-only）。
//!
//! 启动时给设置文档里「从没设置过」的空路径项填空（见
//! `island::settings::seed_detected_paths`）。三层探测，按命中概率从高到低：
//!
//! 1. **运行中进程反查**：进程快照里按 exe 名匹配，再用
//!    `QueryFullProcessImageNameW` 取真实镜像路径——用户自己挑的安装位置
//!    通常只会在进程里体现；
//! 2. **卸载表枚举**：HKCU + HKLM×{64,32} 的 `Uninstall`，DisplayName /
//!    InstallLocation 命中关键字后，依次从 `InstallLocation`、
//!    `DisplayIcon`、`UninstallString` 解析安装目录；目录解析复用
//!    `crate::accounts::install_dir_from_uninstall_string`；
//! 3. **调用方给的常见目录**（环境变量根 + 相对段），最后一段是 exe 文件名。
//!
//! 最终只接受「绝对路径 + `.exe` + 文件存在」；探测失败静默返回 `None`
//! （启动期不写日志、不重试）。

#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

/// 自动探测可执行文件。
///
/// - `exe_names`：大小写不敏感的 exe 文件名；
/// - `keywords`：卸载表 `DisplayName` / `InstallLocation` 的包含关键字；
/// - `common`：`(环境变量名, 相对路径段)`，最后一段应是 exe 文件名。
///
/// 非 Windows 平台永远返回 `None`，调用方保持原值不变。
#[cfg(windows)]
pub(crate) fn detect_exe(
    exe_names: &[&str],
    keywords: &[&str],
    common: &[(&str, &[&str])],
) -> Option<PathBuf> {
    running_exe(exe_names)
        .or_else(|| uninstall_exe(exe_names, keywords))
        .or_else(|| common_exe(exe_names, common))
}

#[cfg(not(windows))]
pub(crate) fn detect_exe(
    _exe_names: &[&str],
    _keywords: &[&str],
    _common: &[(&str, &[&str])],
) -> Option<PathBuf> {
    None
}

/// 只接受「绝对路径 + .exe + 文件存在」：注册表里的残留项、失效快捷方式
/// 都在这里被挡掉。
#[cfg(windows)]
fn acceptable(path: &Path) -> bool {
    path.is_absolute()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        && path.is_file()
}

#[cfg(windows)]
fn name_matches(path: &Path, exe_names: &[&str]) -> bool {
    path.file_name()
        .map(|name| name.to_string_lossy())
        .is_some_and(|name| exe_names.iter().any(|want| name.eq_ignore_ascii_case(want)))
}

/// 展开 `%VAR%` 并去掉首尾引号；未定义的变量原样保留（与设置窗同口径）。
#[cfg(windows)]
fn expanded(raw: &str) -> String {
    let raw = raw.trim().trim_matches('"').trim();
    super::notify::expand_env_vars(raw, |name| std::env::var(name).ok())
}

// ============================================================
// ① 运行中进程反查
// ============================================================

#[cfg(windows)]
fn running_exe(exe_names: &[&str]) -> Option<PathBuf> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = None;
    unsafe {
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if exe_names.iter().any(|want| name.eq_ignore_ascii_case(want)) {
                if let Some(path) = process_image(entry.th32ProcessID) {
                    found = Some(path);
                    break;
                }
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

/// 进程 PID → 镜像绝对路径（读不到 / 没有权限都返回 None，继续看下一个）。
#[cfg(windows)]
fn process_image(pid: u32) -> Option<PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = vec![0u16; 32_768];
    let mut len = buffer.len() as u32;
    let result = unsafe {
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result
    };
    result.ok()?;
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize]));
    acceptable(&path).then_some(path)
}

// ============================================================
// ② 卸载注册表
// ============================================================

#[cfg(windows)]
fn uninstall_exe(exe_names: &[&str], keywords: &[&str]) -> Option<PathBuf> {
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };
    use winreg::RegKey;

    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    // HKCU 覆盖 per-user 安装；HKLM 两个视图覆盖 64/32 位安装器。
    for (hive, flags) in [
        (HKEY_CURRENT_USER, KEY_READ),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY),
    ] {
        let Ok(uninstall) = RegKey::predef(hive).open_subkey_with_flags(UNINSTALL, flags) else {
            continue;
        };
        for key in uninstall.enum_keys().flatten() {
            let Ok(entry) = uninstall.open_subkey(key) else {
                continue;
            };
            let display_name = entry.get_value::<String, _>("DisplayName").unwrap_or_default();
            let install_location = entry
                .get_value::<String, _>("InstallLocation")
                .unwrap_or_default();
            if !matches_keyword(&display_name, keywords)
                && !matches_keyword(&install_location, keywords)
            {
                continue;
            }
            if let Some(path) = entry_exe(&entry, exe_names) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(windows)]
fn matches_keyword(value: &str, keywords: &[&str]) -> bool {
    let value = value.to_ascii_lowercase();
    keywords
        .iter()
        .any(|keyword| value.contains(&keyword.to_ascii_lowercase()))
}

/// 从一个卸载表项里解析 exe：InstallLocation → DisplayIcon → UninstallString。
#[cfg(windows)]
fn entry_exe(entry: &winreg::RegKey, exe_names: &[&str]) -> Option<PathBuf> {
    // InstallLocation 最可靠：就是安装目录。
    if let Some(path) = entry
        .get_value::<String, _>("InstallLocation")
        .ok()
        .and_then(|raw| directory(&raw))
        .and_then(|dir| first_exe(&dir, exe_names))
    {
        return Some(path);
    }
    // DisplayIcon 常见形如 `C:\App\App.exe,0` 或 `"C:\App\App.exe",-1`。
    if let Some(path) = entry
        .get_value::<String, _>("DisplayIcon")
        .ok()
        .and_then(|raw| icon_exe(&raw, exe_names))
    {
        return Some(path);
    }
    // UninstallString：NSIS 的卸载器与被安装程序同目录。
    entry
        .get_value::<String, _>("UninstallString")
        .ok()
        .and_then(|command| crate::accounts::install_dir_from_uninstall_string(&command))
        .and_then(|dir| first_exe(&dir, exe_names))
}

/// 注册表里的目录值：展开环境变量、去引号，必须是已存在的绝对目录。
#[cfg(windows)]
fn directory(raw: &str) -> Option<PathBuf> {
    let path = PathBuf::from(expanded(raw));
    (path.is_absolute() && path.is_dir()).then_some(path)
}

#[cfg(windows)]
fn first_exe(dir: &Path, exe_names: &[&str]) -> Option<PathBuf> {
    exe_names
        .iter()
        .map(|name| dir.join(name))
        .find(|path| acceptable(path))
}

#[cfg(windows)]
fn icon_exe(raw: &str, exe_names: &[&str]) -> Option<PathBuf> {
    // 去掉 `,0` / `,-1` 这类图标索引尾巴；路径本身极少含逗号，只用数字尾巴消歧。
    let raw = match raw.trim().rsplit_once(',') {
        Some((head, tail)) if tail.trim().parse::<i32>().is_ok() => head.trim(),
        _ => raw.trim(),
    };
    let path = PathBuf::from(expanded(raw));
    (acceptable(&path) && name_matches(&path, exe_names)).then_some(path)
}

// ============================================================
// ③ 调用方给的常见目录
// ============================================================

#[cfg(windows)]
fn common_exe(exe_names: &[&str], common: &[(&str, &[&str])]) -> Option<PathBuf> {
    common.iter().find_map(|(root, parts)| {
        let mut path = PathBuf::from(std::env::var_os(root)?);
        for part in parts.iter() {
            path.push(part);
        }
        (name_matches(&path, exe_names) && acceptable(&path)).then_some(path)
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 不存在的程序名：三层（进程快照 / 卸载表 / 常见目录）都要真实跑一遍，
    /// 不能 panic，最终返回 None。本机卸载表有内容，能顺带覆盖枚举分支。
    #[test]
    fn unknown_program_probes_every_layer_without_panic() {
        assert_eq!(
            detect_exe(
                &["agenthud-no-such-app.exe"],
                &["AgentHUD No Such App"],
                &[(
                    "ProgramFiles",
                    &["AgentHUD No Such App", "agenthud-no-such-app.exe"][..],
                )],
            ),
            None
        );
    }

    /// 只接受「绝对路径 + .exe + 文件存在」：相对路径、非 exe、目录都要拒绝。
    #[test]
    fn only_absolute_existing_exe_is_accepted() {
        assert!(!acceptable(Path::new("calc.exe")));
        assert!(!acceptable(Path::new(r"C:\Windows")));
        assert!(!acceptable(Path::new(r"C:\Windows\notepad.txt")));
        let exe = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join("System32")
            .join("notepad.exe");
        assert!(acceptable(&exe), "系统自带 exe 必须被接受: {}", exe.display());
        assert!(name_matches(&exe, &["NOTEPAD.EXE"]), "exe 名比较要不区分大小写");
        assert!(!name_matches(&exe, &["calc.exe"]));
    }
}
