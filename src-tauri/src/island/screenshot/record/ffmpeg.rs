//! ffmpeg 基础设施：定位 / 按需下载 / 音频设备枚举 / 录制与裁剪参数拼接。
//!
//! 设计要点：
//! - **不打包进安装包**：首次使用时从镜像下载，落盘位置固定为
//!   「应用 exe 同目录 bin\ffmpeg(.exe)」——装在哪就下在哪，不写 C 盘
//!   用户目录、不写系统临时目录；该目录不可写（如装进 Program Files）
//!   时给中文指引：设置环境变量 `AGENTHUD_FFMPEG` 或手动放置。
//! - **定位顺序**：`AGENTHUD_FFMPEG` → exe 同目录 `bin/ffmpeg(.exe)` →
//!   `PATH` 中的 ffmpeg → 中文错误。
//! - **多源下载 + 完整性校验**：只从 gyan 官方发布渠道下载（gyan.dev 与
//!   其 GitHub release 镜像 GyanD/codexffmpeg，同一固定版本同一文件），
//!   依次降级，全部失败才报错；下载/解压/校验进度通过
//!   `zbar://ffmpeg-download` 事件推给前端。gyan 构建不做 Authenticode
//!   代码签名（9.0.2 实测 NotSigned），完整性靠「锁定固定版本 URL +
//!   zip 整包 sha256 白名单」保证，哈希不匹配的包直接拒绝、不解压不执行。
//!   安装路径上 `-version` 校验失败的 ffmpeg.exe 会被删除（best-effort），
//!   不留下「定位得到但跑不起来」的残骸。
//! - **dshow 设备枚举**：ffmpeg 的 stderr 在中文系统上是本地代码页
//!   （GBK），解析按字节找 `"名字" (audio)` 引号模式，名字解码按
//!   UTF-8 → GBK → windows-1252 逐级回退；传给 ffmpeg 的一律用
//!   `@device_cm_…` 备用名（纯 ASCII，与代码页无关）。
//! - **停止策略（Windows 实测定案）**：ffmpeg 的键盘交互（`q` 优雅停止）
//!   走 console API，经管道写 stdin 无效（实测 `-stdin` 也不行）。因此：
//!   优先给独立进程组发 `CTRL_BREAK_EVENT`（本进程带 console 时生效，
//!   ffmpeg 会写完整 moov 后退出），2 秒未退或无 console 时 kill；
//!   录制输出用 fragmented MP4（`empty_moov + 0.2s 切片`），被 kill 的
//!   文件依然完整可播，最多丢最后一个切片。
//! - 单元测试只测纯函数（参数拼接、解析、定位顺序、URL 清单），
//!   不真下载、不真开设备。
//!
//! 生产代码里 dshow 枚举 / 录制参数 / 裁剪只在 Windows 路径被调用，但
//! 这些纯函数刻意跨平台编译（单测要在所有平台跑）；非 Windows 平台
//! 放开 dead_code 检查，Windows 上保持严格。
#![cfg_attr(not(windows), allow(dead_code))]

use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// 下载 zip 的体积上限（gyan essentials 9.0.2 实测 ~110MB，留足余量）
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;
/// 解压出的 ffmpeg.exe 单条目读取上限
const MAX_EXE_BYTES: u64 = 320 * 1024 * 1024;
/// 下载进度事件的节流间隔（字节）：每 ~2MB 推一次，避免高频 IPC
const PROGRESS_EMIT_STEP: u64 = 2 * 1024 * 1024;

/// Windows 上给子进程加 `CREATE_NO_WINDOW`：本程序是窗口子系统（没有自己
/// 的 console），直接 spawn 控制台程序（ffmpeg / powershell）会弹出一个黑框。
/// 加上之后子进程完全无窗口；代价是 `CTRL_BREAK` 停止路径不可用——本来在
/// 无 console 时也发不出去（见模块注释），停止走 kill + fMP4 兜底。
#[cfg(windows)]
pub(crate) fn hide_console(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(crate) fn hide_console(_command: &mut std::process::Command) {}

/// 锁定的下载包 sha256（zip 整包，小写 hex）。固定版本必须配固定 URL：
/// 滚动 latest 别名在 gyan 更新后会与这里的哈希失配，届时应随版本升级
/// 一起改这两个常量（升级 ffmpeg = 改 URL + 改哈希，两行）。
/// 实测依据（四路交叉一致）：ffmpeg-9.0.2-essentials_build.zip
/// （114,768,076 字节）分别从 gyan.dev packages 归档与 GitHub GyanD 镜像
/// 下载实测均为该值，且与官方公布的
/// `packages/ffmpeg-9.0.2-essentials_build.zip.sha256`、
/// `ffmpeg-release-essentials.zip.sha256` 内容一致。
const FFMPEG_ZIP_SHA256: &str =
    "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba";

/// 下载源清单：按序依次尝试，全部失败才返回错误。
/// 只保留 gyan 官方发布渠道（官网 packages 版本归档 + 其 GitHub release
/// 镜像），两个 URL 是同一构建产物的镜像，均受 `FFMPEG_ZIP_SHA256` 整包
/// 哈希把关；zip 内均含 `ffmpeg-…/bin/ffmpeg.exe`。原 BtbN 源已移除：无
/// 代码签名也进不了哈希白名单，继续保留等于保留执行未验证二进制的通道。
pub(crate) const FFMPEG_DOWNLOAD_URLS: &[&str] = &[
    // gyan.dev 官方 essentials 构建（packages 目录下的固定版本归档页，
    // 同目录提供官方 .sha256；直接放 builds/ 根下的固定版本名是 404）
    "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0.2-essentials_build.zip",
    // 同一构建的官方 GitHub release 镜像（gyan 本人仓库，winget 同源）
    "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip",
];

/// 麦克风 / 音频输入设备：`name` 是传给 ffmpeg 的设备标识（dshow 备用名），
/// `label` 是界面显示名。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AudioDevice {
    pub name: String,
    pub label: String,
}

/// ffmpeg 就位状态（前端据此决定显示下载引导还是麦克风下拉）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FfmpegStatus {
    pub ready: bool,
    /// 就位时给出实际使用的路径，便于排查
    pub path: Option<String>,
}

/// 下载进度事件负载（`zbar://ffmpeg-download`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FfmpegDownloadProgress {
    /// download | extract | verify | done
    pub stage: String,
    /// 当前是第几个下载源（1 起）
    pub source_index: u32,
    pub source_total: u32,
    /// 已下载字节数（download 阶段）
    pub downloaded: u64,
    /// 总字节数（服务器没给 Content-Length 时为 null）
    pub total: Option<u64>,
    pub message: String,
}

impl FfmpegDownloadProgress {
    fn stage(stage: &str, source_index: u32, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            source_index,
            source_total: FFMPEG_DOWNLOAD_URLS.len() as u32,
            downloaded: 0,
            total: None,
            message: message.into(),
        }
    }
}

/// 防止两个入口同时触发下载（下载上百 MB，重入只会浪费带宽）。
/// 只在 Windows 的安装流程里加锁。
static DOWNLOAD_LOCK: Mutex<()> = Mutex::new(());

// ============================================================
// 定位
// ============================================================

pub(crate) struct Ffmpeg {
    pub(crate) path: PathBuf,
}

impl Ffmpeg {
    /// 定位 ffmpeg：环境变量 → exe 同目录 bin → PATH。找不到给中文错误。
    pub(crate) fn locate() -> Result<Self, String> {
        for candidate in locate_candidates() {
            if candidate.is_file() {
                return Ok(Self { path: candidate });
            }
        }
        Err(locate_error())
    }

    /// 快速状态查询（不报错）：前端决定是否显示下载引导。
    pub(crate) fn status() -> FfmpegStatus {
        match Self::locate() {
            Ok(ff) => FfmpegStatus {
                ready: true,
                path: Some(ff.path.to_string_lossy().to_string()),
            },
            Err(_) => FfmpegStatus {
                ready: false,
                path: None,
            },
        }
    }

    /// 运行一次性命令并等待退出（校验 / 枚举 / 裁剪用）。
    fn run_capture_output(&self, args: &[&str]) -> Result<std::process::Output, String> {
        let mut command = std::process::Command::new(&self.path);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        hide_console(&mut command);
        command
            .output()
            .map_err(|e| format!("启动 ffmpeg 失败（{}）: {e}", self.path.display()))
    }

    /// 校验 ffmpeg 可用：`ffmpeg -version` 退出码为 0。
    pub(crate) fn verify(&self) -> Result<(), String> {
        let output = self
            .run_capture_output(&["-version"])
            .map_err(|e| format!("ffmpeg 无法运行: {e}"))?;
        if !output.status.success() {
            return Err("ffmpeg 校验失败：-version 返回非零退出码".into());
        }
        Ok(())
    }

    /// 枚举音频输入设备。Windows 走 dshow；其它平台本项目暂不接入
    /// （mac 的 MP4 录制走 ScreenCaptureKit，不经过 ffmpeg）。
    pub(crate) fn audio_devices(&self) -> Result<Vec<AudioDevice>, String> {
        #[cfg(windows)]
        {
            // -list_devices 会以「没有实际输入」退出非 0，这是预期行为，
            // 设备清单都在 stderr 里
            let output = self.run_capture_output(&[
                "-hide_banner",
                "-f",
                "dshow",
                "-list_devices",
                "true",
                "-i",
                "dummy",
            ])?;
            Ok(parse_dshow_devices(&output.stderr))
        }
        #[cfg(not(windows))]
        {
            Err("当前系统暂不支持枚举麦克风设备".into())
        }
    }
}

/// exe 同目录下的 bin 目录（下载目标位置）。
pub(crate) fn bundled_bin_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("bin")))
}

fn platform_exe_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

/// 按定位顺序生成候选路径（不检查存在性，方便单测顺序逻辑）：
/// 1. `AGENTHUD_FFMPEG` 环境变量（可指到 exe 或其所在目录）；
/// 2. exe 同目录 `bin/ffmpeg(.exe)`；
/// 3. `PATH` 各目录中的 `ffmpeg(.exe)`。
fn locate_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(env_value) = std::env::var("AGENTHUD_FFMPEG") {
        let trimmed = env_value.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            // 两种形态都收：指向 exe 本体，或指向其所在目录（补文件名）。
            // 不用 is_dir 判定——值可能指向尚未存在的路径
            out.push(path.clone());
            out.push(path.join(platform_exe_name()));
        }
    }
    if let Some(dir) = bundled_bin_dir() {
        out.push(dir.join(platform_exe_name()));
    }
    let path_var = std::env::var("PATH").unwrap_or_default();
    out.extend(which_in_path(&path_var));
    out
}

/// 在 PATH 变量里逐目录找可执行文件（不检查存在性，由调用方判定）。
fn which_in_path(path_var: &str) -> Vec<PathBuf> {
    let name = platform_exe_name();
    std::env::split_paths(path_var)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .collect()
}

/// 找不到 ffmpeg 时的中文指引（下载目标不可写时的兜底也复用这段话术）。
fn locate_error() -> String {
    let target = bundled_bin_dir()
        .map(|dir| dir.join(platform_exe_name()).display().to_string())
        .unwrap_or_else(|| format!("<程序目录>\\bin\\{}", platform_exe_name()));
    format!(
        "未找到 ffmpeg：请在录屏设置里点击「下载 ffmpeg」，或手动把 ffmpeg.exe 放到 {target}，\
         也可设置环境变量 AGENTHUD_FFMPEG 指向已有的 ffmpeg.exe"
    )
}

// ============================================================
// 按需下载
// ============================================================

/// 确保 ffmpeg 就位：已能定位直接返回；否则走多源下载 → 解压 → 校验。
/// 进度通过 `zbar://ffmpeg-download` 事件推给前端。
/// 必须在阻塞线程调用（下载上百 MB）。
pub(crate) fn ensure_ffmpeg(app: &AppHandle) -> Result<FfmpegStatus, String> {
    if let Ok(ff) = Ffmpeg::locate() {
        return Ok(FfmpegStatus {
            ready: true,
            path: Some(ff.path.display().to_string()),
        });
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        return Err(
            "当前系统不支持自动下载 ffmpeg：请用 Homebrew 安装（brew install ffmpeg）\
             并确保其在 PATH 中，或设置环境变量 AGENTHUD_FFMPEG"
                .into(),
        );
    }
    #[cfg(windows)]
    {
        // 拿不到锁说明已有下载在进行中：等它结束再复查一次
        let _guard = DOWNLOAD_LOCK.lock().map_err(|_| "下载任务状态异常".to_string())?;
        if let Ok(ff) = Ffmpeg::locate() {
            return Ok(FfmpegStatus {
                ready: true,
                path: Some(ff.path.display().to_string()),
            });
        }
        let dir = bundled_bin_dir().ok_or_else(|| "无法确定程序所在目录".to_string())?;
        install_ffmpeg(app, &dir)?;
        let ff = Ffmpeg::locate().map_err(|_| "ffmpeg 安装后仍无法定位".to_string())?;
        Ok(FfmpegStatus {
            ready: true,
            path: Some(ff.path.display().to_string()),
        })
    }
}

/// 扫掉 bin 目录里历史下载中断残留的 `ffmpeg-download-*.zip.part` /
/// `ffmpeg.exe.part`。只按前缀匹配删文件、不递归，失败静默（best-effort）。
#[cfg(windows)]
fn cleanup_part_files(bin_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(bin_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("ffmpeg-download-") && name.ends_with(".zip.part")
            || name == "ffmpeg.exe.part"
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(windows)]
fn install_ffmpeg(app: &AppHandle, bin_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(bin_dir).map_err(|_| {
        format!(
            "无法在程序目录写入 ffmpeg（{}）：请设置环境变量 AGENTHUD_FFMPEG 指向已有的 \
             ffmpeg.exe，或手动把 ffmpeg.exe 放到该 bin 目录后重试",
            bin_dir.display()
        )
    })?;
    // 写权限探测：目标目录先落一个探测文件，避免下载上百 MB 后才发现写不进去
    let probe = bin_dir.join(".agenthud-write-probe");
    std::fs::write(&probe, b"probe").map_err(|_| {
        format!(
            "程序目录不可写（{}）：请设置环境变量 AGENTHUD_FFMPEG 指向已有的 ffmpeg.exe，\
             或手动把 ffmpeg.exe 放到该 bin 目录后重试",
            bin_dir.display()
        )
    })?;
    let _ = std::fs::remove_file(&probe);
    // 上次下载中断留下的 .part 一并扫掉（zip.part 上百 MB，残留最伤磁盘）
    cleanup_part_files(bin_dir);

    let mut last_error = String::new();
    for (index, url) in FFMPEG_DOWNLOAD_URLS.iter().enumerate() {
        let source_index = index as u32 + 1;
        match download_and_extract(app, url, bin_dir, source_index) {
            Ok(()) => return Ok(()),
            Err(error) => {
                eprintln!("[zbar-rec] ffmpeg 下载源 {source_index} 失败: {error}");
                last_error = format!("下载源 {source_index}（{url}）: {error}");
                let _ = app.emit(
                    "zbar://ffmpeg-download",
                    FfmpegDownloadProgress::stage("error", source_index, error.clone()),
                );
            }
        }
    }
    Err(format!(
        "ffmpeg 下载失败（已尝试 {} 个源），最后错误：{last_error}。\
         也可以手动把 ffmpeg.exe 放到 {} 后重试",
        FFMPEG_DOWNLOAD_URLS.len(),
        bin_dir.display()
    ))
}

#[cfg(windows)]
fn download_and_extract(
    app: &AppHandle,
    url: &str,
    bin_dir: &Path,
    source_index: u32,
) -> Result<(), String> {
    // 下载 zip 到临时文件（zip 包含上百 MB，不进内存）
    let zip_path = bin_dir.join(format!("ffmpeg-download-{}.zip.part", std::process::id()));
    let target = bin_dir.join("ffmpeg.exe");
    let result = (|| -> Result<(), String> {
        download_zip(app, url, &zip_path, source_index)?;
        // 完整性校验先于解压：哈希不符的包根本不该被解开、更不该被执行
        let _ = app.emit(
            "zbar://ffmpeg-download",
            FfmpegDownloadProgress::stage("verify", source_index, "正在校验下载包完整性…"),
        );
        verify_zip_sha256(&zip_path)?;
        let _ = app.emit(
            "zbar://ffmpeg-download",
            FfmpegDownloadProgress::stage("extract", source_index, "正在解压 ffmpeg.exe…"),
        );
        let bytes = extract_ffmpeg_exe(&zip_path)?;
        // 临时文件 + rename：中途断电/被杀不会留下半个 ffmpeg.exe
        let exe_part = bin_dir.join("ffmpeg.exe.part");
        std::fs::write(&exe_part, &bytes)
            .map_err(|e| format!("写入 ffmpeg.exe 失败（目录可能不可写）: {e}"))?;
        std::fs::rename(&exe_part, &target)
            .map_err(|e| format!("放置 ffmpeg.exe 失败: {e}"))?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&zip_path);
    result?;

    let _ = app.emit(
        "zbar://ffmpeg-download",
        FfmpegDownloadProgress::stage("verify", source_index, "正在校验 ffmpeg…"),
    );
    verify_installed_exe(&target)?;
    let _ = app.emit(
        "zbar://ffmpeg-download",
        FfmpegDownloadProgress::stage("done", source_index, "ffmpeg 已就绪"),
    );
    Ok(())
}

/// 校验刚放置的 ffmpeg.exe 可运行；`-version` 失败时删除该文件（best-effort）
/// 再报错——否则磁盘上会残留「locate 得到但跑不起来」的文件，UI 显示 MP4
/// 就绪、录制时才报错。删完之后 locate 失败，前端会重新出现下载引导。
#[cfg(windows)]
fn verify_installed_exe(target: &Path) -> Result<(), String> {
    let ff = Ffmpeg {
        path: target.to_path_buf(),
    };
    if let Err(error) = ff.verify() {
        let _ = std::fs::remove_file(target);
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn download_zip(
    app: &AppHandle,
    url: &str,
    dest: &Path,
    source_index: u32,
) -> Result<(), String> {
    // 只设连接超时不设总超时：上百 MB 的下载不能被整体超时掐断
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .user_agent(concat!("AgentHUD/", env!("CARGO_PKG_VERSION")))
        .build();
    let response = agent
        .get(url)
        .call()
        .map_err(|e| format!("请求失败: {e}"))?;
    let total: Option<u64> = response
        .header("Content-Length")
        .and_then(|value| value.parse().ok());
    let mut reader = response.into_reader();
    let mut file =
        std::fs::File::create(dest).map_err(|e| format!("创建临时下载文件失败: {e}"))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut next_emit: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| format!("下载中断: {e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|e| format!("写入下载文件失败: {e}"))?;
        downloaded += read as u64;
        if downloaded > MAX_DOWNLOAD_BYTES {
            return Err(format!(
                "下载内容超过 {}MB 上限，已中止（下载源可能异常）",
                MAX_DOWNLOAD_BYTES / 1024 / 1024
            ));
        }
        if downloaded >= next_emit {
            let _ = app.emit(
                "zbar://ffmpeg-download",
                FfmpegDownloadProgress {
                    stage: "download".into(),
                    source_index,
                    source_total: FFMPEG_DOWNLOAD_URLS.len() as u32,
                    downloaded,
                    total,
                    message: "正在下载 ffmpeg…".into(),
                },
            );
            next_emit = downloaded + PROGRESS_EMIT_STEP;
        }
    }
    file.flush().map_err(|e| format!("写入下载文件失败: {e}"))?;
    if downloaded == 0 {
        return Err("下载内容为空".into());
    }
    Ok(())
}

/// 计算文件 sha256 并与期望值比对（分块读取，zip 上百 MB 不进内存）。
/// 返回 `Ok(false)` 表示哈希不匹配。期望值是小写 hex。
#[cfg(windows)]
fn file_sha256_matches(path: &Path, expected_hex: &str) -> Result<bool, String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).map_err(|e| format!("读取下载文件失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("读取下载文件失败: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()) == expected_hex)
}

/// 下载包完整性校验（防镜像投毒 / 企业代理替换 / 传输损坏）：
/// sha256 与代码内白名单（`FFMPEG_ZIP_SHA256`）不符直接报错，zip 不落解压。
#[cfg(windows)]
fn verify_zip_sha256(zip_path: &Path) -> Result<(), String> {
    if !file_sha256_matches(zip_path, FFMPEG_ZIP_SHA256)? {
        return Err(
            "下载包完整性校验失败（sha256 不匹配）：内容可能被篡改或下载损坏，\
             已中止安装。可稍后重试；也可手动下载 \
             ffmpeg-9.0.2-essentials_build.zip，把其中的 ffmpeg.exe 放入\
             程序目录的 bin 文件夹，或设置环境变量 AGENTHUD_FFMPEG 指向已有的 \
             ffmpeg.exe"
                .into(),
        );
    }
    Ok(())
}

/// 从下载的 zip 里只取 ffmpeg.exe（两种包型都是 `xxx/bin/ffmpeg.exe` 包一层
/// 目录；条目路径走 enclosed_name 防穿越，见 pets.rs 的同类实现）。
fn extract_ffmpeg_exe(zip_path: &Path) -> Result<Vec<u8>, String> {
    let file =
        std::fs::File::open(zip_path).map_err(|e| format!("读取下载的 zip 失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("zip 包无法读取: {e}"))?;
    let mut best: Option<(u64, usize)> = None; // (条目大小, 条目下标)
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|e| format!("读取 zip 条目失败: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let name = enclosed.to_string_lossy().replace('\\', "/");
        if !name.ends_with("/ffmpeg.exe") && name != "ffmpeg.exe" {
            continue;
        }
        let size = entry.size();
        if best.map(|(best_size, _)| size > best_size).unwrap_or(true) {
            best = Some((size, index));
        }
    }
    let (_, index) =
        best.ok_or_else(|| "zip 包内未找到 ffmpeg.exe（下载源内容异常）".to_string())?;
    let entry = zip
        .by_index(index)
        .map_err(|e| format!("读取 zip 条目失败: {e}"))?;
    let mut bytes = Vec::new();
    entry
        .take(MAX_EXE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("解压 ffmpeg.exe 失败: {e}"))?;
    if bytes.len() as u64 > MAX_EXE_BYTES {
        return Err("ffmpeg.exe 超过体积上限，已中止".into());
    }
    if bytes.len() < 1024 * 1024 {
        return Err("解压出的 ffmpeg.exe 体积异常（小于 1MB），下载源可能已失效".into());
    }
    Ok(bytes)
}

// ============================================================
// dshow 设备枚举解析
// ============================================================

/// 解析 `ffmpeg -list_devices` 的 stderr。ffmpeg 输出形如：
///
/// ```text
/// [dshow @ …] DirectShow audio devices (some may be both video and audio devices)
/// [dshow @ …]  "麦克风阵列 (Realtek(R) Audio)"
/// [dshow @ …]   Alternative name "@device_cm_{…}\{…}"
/// ```
///
/// 规则（全部按字节匹配，不依赖编码）：
/// - 主名行：引号内的名字 + 行尾 `(audio)` 标记 → 音频设备；
/// - 紧随其后的 `Alternative name "@device_cm_…"` 行给出 ASCII 备用名，
///   录制时优先用备用名（与本地代码页无关，稳定可靠）。
pub(crate) fn parse_dshow_devices(stderr: &[u8]) -> Vec<AudioDevice> {
    let lines: Vec<&[u8]> = stderr.split(|b| *b == b'\n').collect();
    let mut devices = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some((name_bytes, is_audio)) = quoted_name_with_kind(line) else {
            continue;
        };
        if !is_audio {
            continue;
        }
        let label = decode_local_bytes(name_bytes);
        // 备用名在下一行；丢了也能用显示名兜底
        let name = lines
            .get(index + 1)
            .and_then(|next| alternative_name(next))
            .unwrap_or_else(|| label.clone());
        if name.trim().is_empty() {
            continue;
        }
        devices.push(AudioDevice { name, label });
    }
    devices
}

/// 行内找第一对引号之间的名字，并判断行尾标记是 (audio) 还是 (video)。
/// 返回 (名字字节, 是否音频设备)；没有引号或没有 a/v 标记返回 None。
fn quoted_name_with_kind(line: &[u8]) -> Option<(&[u8], bool)> {
    let first = line.iter().position(|b| *b == b'"')?;
    let rest = &line[first + 1..];
    let second = rest.iter().position(|b| *b == b'"')?;
    let name = &rest[..second];
    if name.is_empty() {
        return None;
    }
    let tail = &rest[second + 1..];
    // 标记可能紧跟名字引号，也可能是 "(audio, video)" 双标记
    let text = tail.to_ascii_lowercase();
    let has_audio = contains_subslice(&text, b"(audio");
    let has_video = contains_subslice(&text, b"(video");
    if !has_audio && !has_video {
        return None;
    }
    // Alternative name 行不带 (audio)/(video) 标记，这里天然不会误判
    Some((name, has_audio))
}

/// `Alternative name "@device_cm_…"` 行里提取备用名（只认 @ 开头的，
/// 避免把普通设备名行当备用名；该行没有 (audio)/(video) 标记，不会与
/// 主名行的解析互相干扰）。
fn alternative_name(line: &[u8]) -> Option<String> {
    let first = line.iter().position(|b| *b == b'"')?;
    let rest = &line[first + 1..];
    let second = rest.iter().position(|b| *b == b'"')?;
    let name = &rest[..second];
    if name.is_empty() || name[0] != b'@' {
        return None;
    }
    Some(String::from_utf8_lossy(name).to_string())
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// 按 UTF-8 → GBK → windows-1252 逐级回退解码本地代码页字节；
/// 全部失败时 lossy（中文 Windows 命中 GBK，英文系统设备名基本是 ASCII）。
pub(crate) fn decode_local_bytes(bytes: &[u8]) -> String {
    if std::str::from_utf8(bytes).is_ok() {
        return String::from_utf8_lossy(bytes).to_string();
    }
    let (gbk, _, had_errors) = encoding_rs::GBK.decode(bytes);
    if !had_errors {
        return gbk.to_string();
    }
    let (cp, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
    cp.to_string()
}

/// 在音频设备列表里找「立体声混音」类 loopback 设备（录系统声用）。
/// 不硬编码完整设备名：不同声卡的显示名不一样，按关键词模糊匹配
/// （中文「立体声混音」「混音」与英文 Stereo Mix / Wave Out Mix）。
pub(crate) fn find_loopback_device(devices: &[AudioDevice]) -> Option<AudioDevice> {
    let keywords = ["立体声混音", "混音", "stereo mix", "wave out", "loopback"];
    devices
        .iter()
        .find(|device| {
            let label = device.label.to_lowercase();
            keywords
                .iter()
                .any(|keyword| label.contains(&keyword.to_lowercase()))
        })
        .cloned()
}

// ============================================================
// 录制 / 裁剪参数拼接（纯函数，单测覆盖）
// ============================================================

/// 一条录制的音频输入安排：None = 不录。
#[derive(Debug, Clone, Default)]
pub(crate) struct RecordAudioSpec {
    /// 麦克风设备（dshow 备用名或显示名）
    pub mic: Option<String>,
    /// 系统声 loopback 设备（「立体声混音」类），探测不到就没有
    pub system: Option<String>,
}

impl RecordAudioSpec {
    pub(crate) fn has_audio(&self) -> bool {
        self.mic.is_some() || self.system.is_some()
    }
}

/// 拼接 Windows 录屏参数：gdigrab 抓屏 + dshow 音频 + libx264/aac。
/// 抓屏区与输出尺寸不同时经 scale 统一（偶数化也在这里完成）；
/// 麦克风与系统声两路都有时用 amix 混成一条音轨。
pub(crate) fn build_record_args(
    x: i32,
    y: i32,
    grab_w: u32,
    grab_h: u32,
    out_w: u32,
    out_h: u32,
    fps: u32,
    draw_mouse: bool,
    audio: &RecordAudioSpec,
    out_path: &Path,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        // 视频：gdigrab 桌面区域抓屏（物理像素坐标）
        "-f".into(),
        "gdigrab".into(),
        "-framerate".into(),
        fps.to_string(),
        "-draw_mouse".into(),
        if draw_mouse { "1".into() } else { "0".into() },
        "-offset_x".into(),
        x.to_string(),
        "-offset_y".into(),
        y.to_string(),
        "-video_size".into(),
        format!("{grab_w}x{grab_h}"),
        "-i".into(),
        "desktop".into(),
    ];
    let mic = audio.mic.as_deref();
    let system = audio.system.as_deref();
    if let Some(name) = mic {
        args.extend(["-f".into(), "dshow".into(), "-i".into(), format!("audio={name}")]);
    }
    if let Some(name) = system {
        args.extend(["-f".into(), "dshow".into(), "-i".into(), format!("audio={name}")]);
    }
    // 滤镜图：缩放（需要时）+ 双音频混流
    let need_scale = grab_w != out_w || grab_h != out_h;
    let mix_audio = mic.is_some() && system.is_some();
    let mut filters: Vec<String> = Vec::new();
    if need_scale {
        filters.push(format!("[0:v]scale={out_w}:{out_h}[v]"));
    }
    if mix_audio {
        // duration=first：跟随视频流时长收尾，避免混音把输出拉长
        filters.push("[1:a][2:a]amix=inputs=2:duration=first:dropout_transition=2[aout]".into());
    }
    if !filters.is_empty() {
        args.extend(["-filter_complex".into(), filters.join(";")]);
    }
    // 音频映射：双路混流用 [aout]；单路音频（麦克风或系统声）都是 1 号输入。
    // 注意 ffmpeg 只要出现一个显式 -map 就关闭默认流选择，所以**每个分支
    // 必须把自己需要的视频/音频映射全部补齐**，否则会丢流（只写 -map [aout]
    // 会输出纯音频，只写 -map [v] 会丢音轨）。
    let audio_map: Option<&str> = if mix_audio {
        Some("[aout]")
    } else if audio.has_audio() {
        Some("1:a")
    } else {
        None
    };
    if let Some(audio_map) = audio_map {
        if need_scale {
            args.extend(["-map".into(), "[v]".into()]);
        } else {
            args.extend(["-map".into(), "0:v".into()]);
        }
        args.extend(["-map".into(), audio_map.into()]);
    } else if need_scale {
        // 纯视频 + 缩放：filter_complex 已存在，视频必须显式映射
        args.extend(["-map".into(), "[v]".into()]);
    }
    // 纯视频 + 无缩放：不加任何 -map，走 ffmpeg 默认流选择（视频流）
    // 编码：libx264 + aac。输出用 fragmented MP4（empty_moov + 定长切片）：
    // Windows 上无法通过管道向 ffmpeg 发 `q`（键盘交互走 console API，管道
    // 无效，实测确认），停止只能靠 CTRL_BREAK（有 console 的环境）或 kill；
    // fMP4 让被强杀的文件依然完整可播，最多丢最后一个 0.2s 切片。
    args.extend([
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "23".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-frag_duration".into(),
        "200000".into(),
        "-flush_packets".into(),
        "1".into(),
    ]);
    if audio.has_audio() {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
    }
    args.extend([
        "-movflags".into(),
        "+empty_moov+default_base_moof".into(),
    ]);
    args.push(out_path.to_string_lossy().to_string());
    args
}

/// 拼接 MP4 裁剪参数：默认重编码（精确到 0.1s 级）。
/// `-ss` 放输入前快速 seek，配合重编码是精确裁剪；`-t` 是输出时长，
/// 避免 `-to` 在部分版本作为输入选项时的行为差异。
pub(crate) fn build_trim_args(
    source: &Path,
    out_path: &Path,
    start_ms: f64,
    end_ms: f64,
) -> Vec<String> {
    // 裁剪时长最少 0.1s：极端拖拽下区间可能只有几十毫秒
    let duration_ms = (end_ms - start_ms).max(100.0);
    vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-ss".into(),
        seconds_arg(start_ms),
        "-i".into(),
        source.to_string_lossy().to_string(),
        "-t".into(),
        seconds_arg(duration_ms),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "23".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "160k".into(),
        "-movflags".into(),
        "+faststart".into(),
        out_path.to_string_lossy().to_string(),
    ]
}

/// 毫秒 → 秒参数（保留 3 位小数，0.001s 精度）。
pub(crate) fn seconds_arg(ms: f64) -> String {
    format!("{:.3}", (ms / 1000.0).max(0.0))
}

// ============================================================
// 录制子进程（优雅停止）
// ============================================================

/// 运行中的 ffmpeg 录制进程。
/// 停止策略（Windows 实测定案，见模块注释）：ffmpeg 的键盘交互走 console
/// API，管道写 `q` 不生效——优先给进程组发 `CTRL_BREAK_EVENT`（本进程
/// 有 console 的环境能优雅收尾、moov 完整），2 秒没退就 kill。录制输出
/// 是 fragmented MP4，kill 落盘的文件依然可播，最多丢最后 0.2s 切片。
/// 只在 Windows 的录制线程使用（mac 的 MP4 走 ScreenCaptureKit）。
pub(crate) struct FfmpegRecording {
    child: std::process::Child,
    stderr_path: PathBuf,
    /// Job Object 句柄：应用任何路径退出（含崩溃 / 被任务管理器结束）时，
    /// 内核关闭 job 句柄并自动终结 job 内的 ffmpeg，杜绝孤儿进程继续
    /// 抓屏写盘。选它而不是 RunEvent 挂点：挂点只覆盖正常退出，崩溃路径
    /// 拦不住；KILL_ON_JOB_CLOSE 是内核级保证。
    #[cfg(windows)]
    _job: Option<JobGuard>,
}

/// 持有 job 句柄，drop 时显式关闭（正常路径不依赖进程退出时的隐式回收）。
#[cfg(windows)]
struct JobGuard(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for JobGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// 把子进程挂进 KILL_ON_JOB_CLOSE 的 Job Object。失败只记日志不阻塞
/// 录制（防孤儿是纵深防御，不能让它反过来让录屏不可用）。
#[cfg(windows)]
fn assign_child_to_job(child: &std::process::Child) -> Option<JobGuard> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    unsafe {
        let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .is_ok();
        if !configured {
            let _ = CloseHandle(job);
            return None;
        }
        let child_handle = windows::Win32::Foundation::HANDLE(child.as_raw_handle());
        if AssignProcessToJobObject(job, child_handle).is_err() {
            let _ = CloseHandle(job);
            return None;
        }
        Some(JobGuard(job))
    }
}

impl FfmpegRecording {
    /// 启动录制进程。stderr 落临时文件（结束后读尾部用于报错；用文件
    /// 而不是管道，避免 stderr 满缓冲阻塞 ffmpeg）。Windows 上以独立
    /// 进程组启动（停止时才能定向发 CTRL_BREAK 而不误伤自己），并挂进
    /// Job Object 防孤儿进程。
    pub(crate) fn spawn(ffmpeg: &Ffmpeg, args: &[String]) -> Result<Self, String> {
        let stderr_path = std::env::temp_dir().join(format!(
            "agenthud-ffmpeg-{}-{}.log",
            std::process::id(),
            chrono::Local::now().format("%Y%m%d%H%M%S%3f")
        ));
        let stderr_file =
            std::fs::File::create(&stderr_path).map_err(|e| format!("创建录制日志失败: {e}"))?;
        let mut command = std::process::Command::new(&ffmpeg.path);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(stderr_file));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            // 新进程组（停止时定向发 CTRL_BREAK 用）+ 不弹控制台黑框
            command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }
        let child = command
            .spawn()
            .map_err(|e| format!("启动 ffmpeg 录制失败: {e}"))?;
        #[cfg(windows)]
        let job = assign_child_to_job(&child);
        #[cfg(windows)]
        if job.is_none() {
            eprintln!("[zbar-rec] ffmpeg 未挂进 Job Object（防孤儿进程兜底失效，不影响本次录制）");
        }
        Ok(Self {
            child,
            stderr_path,
            #[cfg(windows)]
            _job: job,
        })
    }

    /// 非阻塞探测进程是否已退出（录制中轮询用）。
    pub(crate) fn try_finish(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// 停止录制：CTRL_BREAK 优先（有 console 的环境优雅收尾）→ 2 秒超时
    /// kill（fMP4 保证落盘文件可用）。返回最终退出状态。
    pub(crate) fn stop_graceful(&mut self) -> std::io::Result<std::process::ExitStatus> {
        #[cfg(windows)]
        {
            use windows::Win32::System::Console::{
                GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT,
            };
            // 本进程没有 console（GUI 发布版）时发送失败，直接走 kill 兜底
            let sent = unsafe {
                GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.child.id() as u32).is_ok()
            };
            if sent {
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                while std::time::Instant::now() < deadline {
                    if let Some(status) = self.child.try_wait()? {
                        return Ok(status);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                eprintln!("[zbar-rec] ffmpeg 收到 CTRL_BREAK 后 2 秒未退出，强制结束");
            }
        }
        let _ = self.child.kill();
        self.child.wait()
    }

    /// 读 stderr 日志尾部（报错诊断用），宽容解码本地代码页。
    pub(crate) fn stderr_tail(&self, max_chars: usize) -> String {
        let bytes = std::fs::read(&self.stderr_path).unwrap_or_default();
        let text = decode_local_bytes(&bytes);
        let chars: Vec<char> = text.chars().collect();
        if chars.len() <= max_chars {
            return text;
        }
        chars[chars.len() - max_chars..].iter().collect()
    }

    /// 收尾清理日志文件。
    pub(crate) fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.stderr_path);
    }
}

/// 运行一次性 ffmpeg 命令到完成（裁剪用），失败时带出 stderr 尾部。
pub(crate) fn run_to_done(ffmpeg: &Ffmpeg, args: &[String]) -> Result<(), String> {
    let stderr_path = std::env::temp_dir().join(format!(
        "agenthud-ffmpeg-{}-{}.log",
        std::process::id(),
        chrono::Local::now().format("%Y%m%d%H%M%S%3f")
    ));
    let result = (|| -> Result<(), String> {
        let stderr_file = std::fs::File::create(&stderr_path)
            .map_err(|e| format!("创建任务日志失败: {e}"))?;
        let mut command = std::process::Command::new(&ffmpeg.path);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(stderr_file));
        hide_console(&mut command);
        let output = command
            .output()
            .map_err(|e| format!("启动 ffmpeg 失败: {e}"))?;
        if !output.status.success() {
            let bytes = std::fs::read(&stderr_path).unwrap_or_default();
            let tail = decode_local_bytes(&bytes);
            let tail: String = tail.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
            return Err(format!("ffmpeg 执行失败: {tail}"));
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&stderr_path);
    result
}

// ============================================================
// 单元测试：只测纯函数，不真下载、不真开设备
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_candidates_order_env_first() {
        // 顺序：AGENTHUD_FFMPEG（本体 + 补名两种形态）→ exe/bin → PATH。
        // 集中在一个测试里改环境变量，避免 set_var 在并行测试间的竞态。
        std::env::set_var("AGENTHUD_FFMPEG", "D:/tools/ffmpeg.exe");
        let candidates = locate_candidates();
        assert!(!candidates.is_empty());
        assert_eq!(
            candidates[0],
            PathBuf::from("D:/tools/ffmpeg.exe"),
            "环境变量原值应最先命中"
        );
        assert_eq!(
            candidates[1],
            PathBuf::from("D:/tools/ffmpeg.exe").join(platform_exe_name()),
            "环境变量指到目录的形态紧随其后"
        );
        if let Some(bin) = bundled_bin_dir() {
            assert_eq!(candidates[2], bin.join(platform_exe_name()));
        }
        let path_candidates = which_in_path(&std::env::var("PATH").unwrap_or_default());
        assert_eq!(
            candidates.len(),
            3 + path_candidates.len(),
            "PATH 候选都应收在最后"
        );

        std::env::set_var("AGENTHUD_FFMPEG", "D:/tools/bin/");
        let candidates = locate_candidates();
        assert_eq!(candidates[1], PathBuf::from("D:/tools/bin").join(platform_exe_name()));
        std::env::remove_var("AGENTHUD_FFMPEG");

        let plain = locate_candidates();
        assert!(plain.iter().all(|p| !p.starts_with("D:/tools")), "{plain:?}");
    }

    #[test]
    fn which_in_path_collects_all_dirs() {
        let candidates = which_in_path("C:\\a;D:\\b;;");
        let name = platform_exe_name();
        assert_eq!(candidates.len(), 2, "空目录要跳过");
        assert_eq!(candidates[0], PathBuf::from("C:\\a").join(name));
        assert_eq!(candidates[1], PathBuf::from("D:\\b").join(name));
    }

    #[test]
    fn download_urls_are_pinned_gyan_official_versions() {
        // gyan 构建无 Authenticode 签名（9.0.2 实测 NotSigned），完整性只能靠
        // 固定版本 URL + zip 整包 sha256（FFMPEG_ZIP_SHA256）把关。固定哈希
        // 必须配固定版本 URL：滚动 latest 别名在 gyan 更新后会哈希失配。
        assert!(!FFMPEG_DOWNLOAD_URLS.is_empty(), "至少保留一个下载源");
        for url in FFMPEG_DOWNLOAD_URLS {
            assert!(url.starts_with("https://"), "下载源必须是 HTTPS: {url}");
            assert!(url.ends_with(".zip"), "下载源应是 zip 包: {url}");
            assert!(
                url.contains("packages/ffmpeg-9.0.2-essentials_build.zip")
                    || url.contains("/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip"),
                "必须锁定与 FFMPEG_ZIP_SHA256 匹配的固定版本: {url}"
            );
            let gyan_official = url.contains("www.gyan.dev/") || url.contains("github.com/GyanD/");
            assert!(
                gyan_official,
                "只允许 gyan 官方发布渠道（官网 / 其 GitHub release 镜像）: {url}"
            );
        }
        // BtbN 等进不了哈希白名单的第三方源不允许回流
        assert!(
            FFMPEG_DOWNLOAD_URLS.iter().all(|url| !url.contains("BtbN")),
            "BtbN 源已移除，不应出现在清单里"
        );
    }

    #[test]
    fn file_sha256_matches_accepts_and_rejects() {
        // 纯逻辑覆盖「匹配 / 不匹配」两条路径：用临时文件注入已知内容
        let dir = std::env::temp_dir().join(format!("agenthud-ff-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("sample.bin");
        std::fs::write(&path, b"agenthud").expect("写样例");
        // `sha256("agenthud")` 的期望值：与内容相符 → true；任意其他期望 → false
        let actual = {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(b"agenthud"))
        };
        assert!(file_sha256_matches(&path, &actual).expect("计算成功"));
        assert!(!file_sha256_matches(&path, "00".repeat(32).as_str()).expect("计算成功"));
        // 文件不存在 → Err（中文报错），而不是 panic / 误判
        assert!(file_sha256_matches(&dir.join("missing.bin"), &actual).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_zip_sha256_rejects_foreign_content() {
        // 内容与白名单不符 → 中文报错并指引手动放置，绝不能放行解压
        let dir = std::env::temp_dir().join(format!("agenthud-ff-sha2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("poisoned.zip");
        std::fs::write(&path, b"not the pinned zip").expect("写假包");
        let error = verify_zip_sha256(&path).expect_err("哈希不符应报错");
        assert!(error.contains("完整性校验失败"), "错误信息: {error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_installed_exe_deletes_broken_file() {
        // 校验失败 → 删除 + 报错路径真跑通：篡改的「ffmpeg.exe」（非 PE 内容）
        // 启动必然失败，必须删掉而不是留在磁盘上让 UI 显示「MP4 就绪」
        let dir =
            std::env::temp_dir().join(format!("agenthud-ff-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let target = dir.join(platform_exe_name());
        std::fs::write(&target, b"this is definitely not a PE executable").expect("写假 exe");
        let error = verify_installed_exe(&target).expect_err("假 exe 应校验失败");
        assert!(
            error.contains("ffmpeg"),
            "错误信息应可读: {error}"
        );
        assert!(!target.exists(), "校验失败的 exe 必须被删除");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "手动实测：真实访问网络下载 gyan 固定版本 zip（约 110MB）并验证白名单哈希"]
    fn manual_downloaded_zip_passes_pinned_hash() {
        // 「校验通过」路径的端到端实测：从 FFMPEG_DOWNLOAD_URLS[0] 真下载，
        // verify_zip_sha256 必须放行（HTTPS + sha256 白名单双保险均成立）
        let dir = std::env::temp_dir().join(format!("agenthud-ff-manual-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let zip_path = dir.join("pinned.zip");
        let mut reader = ureq::get(FFMPEG_DOWNLOAD_URLS[0])
            .call()
            .expect("下载应成功")
            .into_reader();
        let mut file = std::fs::File::create(&zip_path).expect("建临时文件");
        std::io::copy(&mut reader, &mut file).expect("落盘");
        drop(file);
        verify_zip_sha256(&zip_path).expect("固定版本 zip 必须通过白名单哈希");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_dshow_devices_picks_audio_with_alternative_names() {
        // 中文 Windows 上旧版 ffmpeg 的 stderr 是 GBK；“麦克风阵列”的
        // GBK 编码是 C2F3 BFCB B7E7 D5F3 C1D0；标记行始终是 ASCII
        let gbk_name: &[u8] = b"\xc2\xf3\xbf\xcb\xb7\xe7\xd5\xf3\xc1\xd0";
        let mut stderr: Vec<u8> = Vec::new();
        stderr.extend_from_slice(b"[in#0 @ 0x1] DirectShow video devices\r\n");
        stderr.extend_from_slice(b"[in#0 @ 0x1]  \"USB Camera\" (video)\r\n");
        stderr.extend_from_slice(
            b"[in#0 @ 0x1]   Alternative name \"@device_cm_{v-guid}\"\r\n",
        );
        stderr.extend_from_slice(b"[in#0 @ 0x1] DirectShow audio devices\r\n");
        stderr.push(b'"');
        stderr.extend_from_slice(gbk_name);
        stderr.extend_from_slice(b" (Realtek Audio)\" (audio)\r\n");
        stderr.extend_from_slice(
            b"[in#0 @ 0x1]   Alternative name \"@device_cm_{a-guid}\"\r\n",
        );
        let devices = parse_dshow_devices(&stderr);
        assert_eq!(devices.len(), 1, "视频设备不该进来: {devices:?}");
        assert_eq!(devices[0].name, "@device_cm_{a-guid}");
        assert_eq!(devices[0].label, "麦克风阵列 (Realtek Audio)");
    }

    #[test]
    fn parse_dshow_devices_skips_alternative_lines() {
        let stderr = b"[dshow @ 0x1]  \"Mic\" (audio)\n[dshow @ 0x1]  \"Camera\" (video)\n";
        let devices = parse_dshow_devices(stderr);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].label, "Mic");
        assert_eq!(devices[0].name, "Mic", "没有备用名时用显示名兜底");
    }

    #[test]
    fn decode_local_bytes_prefers_utf8() {
        assert_eq!(decode_local_bytes(b"plain ascii"), "plain ascii");
        assert_eq!(decode_local_bytes("中文".as_bytes()), "中文");
        // GBK 编码的“麦克风”应能解码回中文
        assert_eq!(decode_local_bytes(b"\xc2\xf3\xbf\xcb\xb7\xe7"), "麦克风");
    }

    #[test]
    fn find_loopback_device_matches_stereo_mix_keywords() {
        let devices = vec![
            AudioDevice {
                name: "@device_cm_a1".into(),
                label: "麦克风阵列 (Realtek Audio)".into(),
            },
            AudioDevice {
                name: "@device_cm_a2".into(),
                label: "立体声混音 (Realtek Audio)".into(),
            },
        ];
        assert_eq!(
            find_loopback_device(&devices).map(|d| d.name),
            Some("@device_cm_a2".into())
        );
        // 没有混音设备时返回 None
        assert!(find_loopback_device(&devices[..1]).is_none());
    }

    #[test]
    fn build_record_args_video_only() {
        let audio = RecordAudioSpec::default();
        let args = build_record_args(
            100,
            200,
            800,
            600,
            800,
            600,
            30,
            true,
            &audio,
            Path::new("D:/out/clip.mp4"),
        );
        assert_eq!(args[0], "-y");
        // 无音频时不该有 dshow / aac / amix
        assert!(!args.iter().any(|a| a == "dshow"));
        assert!(!args.iter().any(|a| a == "aac"));
        assert!(!args.iter().any(|a| a.contains("amix")));
        // 尺寸相同不需要 filter_complex，也不需要任何显式 -map
        assert!(!args.iter().any(|a| a == "-filter_complex"));
        assert!(!args.iter().any(|a| a == "-map"), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["-draw_mouse", "1"]));
        assert!(args.windows(2).any(|w| w == ["-offset_x", "100"]));
        assert!(args.windows(2).any(|w| w == ["-video_size", "800x600"]));
        // 录制输出必须是 fragmented MP4（强杀后文件仍可播）
        assert!(
            args.windows(2).any(|w| w == ["-movflags", "+empty_moov+default_base_moof"]),
            "{args:?}"
        );
        assert!(args.windows(2).any(|w| w == ["-flush_packets", "1"]));
        assert_eq!(args.last().unwrap(), "D:/out/clip.mp4");
    }

    #[test]
    fn build_record_args_mic_only_maps_first_audio() {
        let audio = RecordAudioSpec {
            mic: Some("@device_cm_mic".into()),
            system: None,
        };
        let args = build_record_args(
            0,
            0,
            1000,
            700,
            500,
            350,
            30,
            false,
            &audio,
            Path::new("out.mp4"),
        );
        // 尺寸不同要 scale，并映射 [v]
        assert!(args.iter().any(|a| a == "-filter_complex"));
        let filter = args
            .windows(2)
            .find(|w| w[0] == "-filter_complex")
            .map(|w| w[1].clone())
            .unwrap();
        assert!(filter.contains("[0:v]scale=500:350[v]"), "滤镜: {filter}");
        assert!(args.windows(2).any(|w| w == ["-map", "[v]"]));
        assert!(args.windows(2).any(|w| w == ["-map", "1:a"]));
        assert!(args.windows(2).any(|w| w == ["-draw_mouse", "0"]));
        // dshow 输入用设备名
        assert!(args.iter().any(|a| a == "audio=@device_cm_mic"));
        assert!(args.iter().any(|a| a == "aac"));
    }

    #[test]
    fn build_record_args_mic_and_system_amix() {
        let audio = RecordAudioSpec {
            mic: Some("@mic".into()),
            system: Some("@mix".into()),
        };
        // 尺寸一致（无缩放）+ 双音频：filter_complex 只含 amix，此时视频
        // 必须显式 -map 0:v，否则唯一映射 [aout] 会把视频流整个丢掉
        let args = build_record_args(
            0,
            0,
            800,
            600,
            800,
            600,
            30,
            true,
            &audio,
            Path::new("out.mp4"),
        );
        let filter = args
            .windows(2)
            .find(|w| w[0] == "-filter_complex")
            .map(|w| w[1].clone())
            .unwrap();
        assert!(
            filter.contains("[1:a][2:a]amix=inputs=2:duration=first"),
            "双音频要混流: {filter}"
        );
        assert!(args.windows(2).any(|w| w == ["-map", "0:v"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["-map", "[aout]"]), "{args:?}");
        assert_eq!(
            args.iter().filter(|a| *a == "audio=@mic").count(),
            1,
            "只该有一路麦克风输入"
        );
    }

    #[test]
    fn build_record_args_system_only_with_scale_keeps_audio_map() {
        // 麦克风关 + 有立体声混音 + 需要缩放：视频走 [v]，系统声是 1 号输入
        let audio = RecordAudioSpec {
            mic: None,
            system: Some("@mix".into()),
        };
        let args = build_record_args(
            0,
            0,
            1001,
            701,
            1000,
            700,
            30,
            true,
            &audio,
            Path::new("out.mp4"),
        );
        assert!(args.windows(2).any(|w| w == ["-map", "[v]"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["-map", "1:a"]), "{args:?}");
        assert!(args.iter().any(|a| a == "audio=@mix"));
        assert!(args.iter().any(|a| a == "aac"), "有音频时必须编码 aac");
    }

    #[test]
    fn build_record_args_mic_only_without_scale_maps_video_explicitly() {
        // 麦克风单路 + 尺寸一致：没有 filter_complex，但 -map 1:a 一旦出现，
        // 视频也必须显式 -map 0:v，不能指望默认流选择
        let audio = RecordAudioSpec {
            mic: Some("@mic".into()),
            system: None,
        };
        let args = build_record_args(
            0,
            0,
            800,
            600,
            800,
            600,
            30,
            true,
            &audio,
            Path::new("out.mp4"),
        );
        assert!(!args.iter().any(|a| a == "-filter_complex"), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["-map", "0:v"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["-map", "1:a"]), "{args:?}");
    }

    #[test]
    fn build_trim_args_uses_input_seek_and_output_duration() {
        let args = build_trim_args(
            Path::new("src.mp4"),
            Path::new("out.mp4"),
            500.0,
            3_200.0,
        );
        // -ss 在 -i 之前（快速 seek），-t 是输出时长（精确裁剪）
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert!(ss < input);
        assert_eq!(args[ss + 1], "0.500");
        let t = args.iter().position(|a| a == "-t").unwrap();
        assert!(t > input);
        assert_eq!(args[t + 1], "2.700");
        // 重编码而不是 -c copy
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(!args.iter().any(|a| a == "copy"));
        assert_eq!(args[args.len() - 2], "+faststart");
    }

    #[test]
    fn seconds_arg_formats_milliseconds() {
        assert_eq!(seconds_arg(500.0), "0.500");
        assert_eq!(seconds_arg(0.0), "0.000");
        assert_eq!(seconds_arg(61_234.0), "61.234");
        assert_eq!(seconds_arg(-5.0), "0.000", "负数要钳到 0");
    }

    #[test]
    fn extract_ffmpeg_exe_rejects_fake_zip() {
        // 造一个没有 ffmpeg.exe 的 zip：应当报中文错误而不是 panic
        let dir = std::env::temp_dir().join(format!("agenthud-ff-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let zip_path = dir.join("fake.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        zip.start_file(
            "bin/readme.txt",
            zip::write::FileOptions::<()>::default(),
        )
        .unwrap();
        zip.write_all(b"no exe here").unwrap();
        zip.finish().unwrap();
        let error = extract_ffmpeg_exe(&zip_path).expect_err("应报错");
        assert!(error.contains("未找到 ffmpeg.exe"), "错误信息: {error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
