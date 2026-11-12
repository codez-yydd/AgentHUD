# 录屏音频 / Windows 裁剪 / 会议录音：实施交接

> 目标读者：接手的实施 agent。读完这份应该能直接动手，不需要再猜现状。
> 编写时间：2026-11-12；对应工作区状态在提交 `8941c62` 之后（灵动岛工具箱、片段库、启动器、剪贴板增强已合入，这三项**都没有做**）。
> 本文里的行号以当前工作区为准，改动后请重新定位符号，不要硬编码行号。

---

## 实施状态（批 1，2026-11-12；批 2 会议录音同日完成）

批 1 已完成 **任务 A（Windows MP4 + 麦克风）+ 任务 B（Windows MP4 裁剪）+ ffmpeg 基础设施**；**批 2 已完成任务 C（会议录音页）**，见下方「批 2 完成记录」。本文其余章节保留为背景与原始设计，行号对应批 1 之前的代码，**与现状已有出入，以代码为准**。

### 批 2 完成记录：任务 C 会议录音页（2026-11-12）

- **采集层**：`cpal 0.15`（默认特性，无 jack/asio），Windows WASAPI / macOS CoreAudio 同一套代码，**不依赖 ffmpeg**——会议页在没下载 ffmpeg 的机器上完整可用。
- **后端** `src-tauri/src/island/meeting.rs`：`meeting_devices / meeting_start / meeting_pause / meeting_resume / meeting_stop / meeting_add_mark / meeting_state / meeting_list / meeting_delete / meeting_rename / meeting_open_folder / meeting_privacy / meeting_confirm_privacy` 共 13 个命令（全部中文错误 `Result<_, String>`），注册在 lib.rs `generate_handler!` 末尾追加。
- **落盘**（C.1 口径）：`~/.agenthud/meetings/{YYYYMMDD-HHMM}.wav`（16kHz/16bit/单声道为目标；设备不支持 16k 时用设备默认采样率并记入侧车 `sample_rate`；声道一律降混单声道）+ 同名 `.json` 侧车 `{started_at, duration_ms, device, sample_rate, marks:[{ms,label}]}`（`#[serde(default)]` 兼容旧文件）。录制中写 `.wav.tmp`（44 字节占位头 + 追加数据），stop 回填真实长度再 rename；杀进程只留 `.tmp`，下次 start/list 自动清理，**不会产生 0 字节正式文件**。
- **暂停 / 有效时长**：暂停走 cpal 流的 pause（keeper 线程持有 `cpal::Stream`——它刻意 `!Send`，用「专职线程 + mpsc 控制通道」模式跨线程操控）；`duration_ms` 按实际写入采样数计算，天然扣除暂停段；marks 的 ms 用 `SegClock` 有效时间轴（纯函数可单测）。
- **隐私确认**：首次开始前端弹确认卡（展示保存路径），持久化在 island.db 新表 `meeting_settings`（store.rs 追加 `load/save_meeting_settings`，复用 ShotSettings 的单行 payload 模式，未动既有函数）。
- **前端**：工具箱第 5 子页「会议」（`src/island/MeetingPanel.tsx`，注册见 `ToolboxPage.tsx`）；状态机 空闲 → 录制中（计时 / 暂停继续 / 打标记 / 结束 / 「转写（即将支持）」置灰占位）→ 列表（打开文件夹 / 重命名 / 两段式删除）；子页切走再切回用 `meeting_state` 恢复录制视图。mock 走 `api.ts` 的假会话状态机，`.preview/harness/island-tool.html?mode=meeting&open=1` 可完整预览。
- **验证**：`cargo test --lib` 947 passed / 0 failed / 3 ignored（基线 938 + 新增 9 个纯函数单测：WAV 头布局与解析、降混、SegClock 暂停扣除、侧车兼容读、重名避让、文件名往返、名称消毒）；`cargo check --lib` 无新增错误与 warning；`npx tsc --noEmit` / `npx vite build` 通过；浏览器 mock 全链路已实测。
- **顺手修正**：`island/api.ts` 的 `stepIslandDrag` mock 之前恒回 `started: true`，浏览器预览里点任何按钮都可能被判成拖拽而收起面板；改为 `started: false`（与真实 Rust 阈值判定的「按住没拖」语义一致）。

### 批 3 追加记录：播放 / 自动音量 / 额度通知归属（2026-11-13）

真机试用后反馈的三点：录完不能就地播放、录出来的声音偏小、额度重置通知看不出是哪家订阅。补法：

- **列表播放**：`MeetingPanel.tsx` 条目新增播放键，播放器展开在条目内（独占一行：播放/暂停、进度拖动、当前/总时长、停止）；后端 `meeting_audio(id)` 只做「校验 id → 确认 wav 存在 → `asset_protocol_scope().allow_file(wav)` → 返回路径」，前端 `convertFileSrc` 后交给 `<audio>`，**不复制、不转码**，asset 协议自带 Range 所以拖动进度不用整段重下。一次只播一条；开始录音前自动停（外放会串进麦克风）；删除正在播放的条目会收起播放器。浏览器预览用 `api.ts` 合成的 60 秒 WAV Blob 走同一套 UI。
- **自动音量**：`meeting.rs::normalize_wav_peak` 在 stop 的回填头之后、rename 之前扫一遍整段峰值，原地线性放大到 ≈ -2 dBFS；只放大不衰减、上限 +18 dB、峰值低于 ≈ -48 dBFS 视为静音不动，因此不会削波也不会把底噪抬成噪音。失败只打日志并保留原始录音（文件已完整，不能因为「想更响」让保存失败）。设置存 island.db `meeting_settings.autoGain`（默认开，会议页「音量」行可关），老 payload 缺字段自动补 true。
- **额度通知归属**：`quota_reminder::Entry` 新增 `source`，登记 ZCode 三个窗口时统一填 `ZCode · GLM Coding Plan {档位}`（档位由 `QuotaResult.level` 映射 lite/pro/max/ultra，未知档位首字母大写原样附上）；正文改为「ZCode · GLM Coding Plan Pro · 5 小时额度将在 今天 12:30 重置，之后可用新额度」——**先给归属、再给时间**，通知横幅第一行就能看出是哪家订阅；跨 provider 的 key 改成 `zcode:hour5` 前缀，Claude / Codex 将来接入不会撞键。
- **测试**：`cargo test --lib` 956 passed / 0 failed / 4 ignored；新增 `normalize_boosts_quiet_audio_and_forces_no_clip`（峰值 8000 → ≈26000、已足够响不动、静音不动）、`plan_label_maps_subscription_tiers`。

### 已完成项

- **ffmpeg 基础设施**（`src-tauri/src/island/screenshot/record/ffmpeg.rs`，新模块）：
  - 定位顺序 `AGENTHUD_FFMPEG`（支持指到 exe 或目录）→ exe 同目录 `bin\ffmpeg.exe` → `PATH`；
  - 按需下载：多源依次降级（BtbN GitHub → gyan.dev），zip 下载到程序目录 `bin\`（先落 `.part` 再 rename），只解压 `bin/ffmpeg.exe` 条目（路径穿越防护 + 体积上限），跑 `ffmpeg -version` 校验；进度经 `zbar://ffmpeg-download` 事件推送（download/extract/verify/done/error 阶段 + 字节数）；目录不可写时给中文指引（AGENTHUD_FFMPEG / 手动放置）；
  - dshow 音频设备枚举：按字节匹配 `"名字" (audio)` 引号模式 + `Alternative name "@device_cm_…"` 备用名；显示名按 UTF-8 → GBK → windows-1252 回退解码（复用既有 `encoding_rs`），**录制一律用 ASCII 备用名**，与本地代码页无关；
  - 系统声探测：关键词匹配「立体声混音 / 混音 / Stereo Mix / Wave Out / Loopback」，不硬编码设备名。
- **Windows MP4 录制**（`record.rs`）：`RecordOptions.mic`（camelCase wire）；gdigrab 30fps + dshow 音频 + libx264 `-preset veryfast -crf 23` + aac 160k；麦克风与系统声两路时 `amix` 混流；抓屏区与输出尺寸不一致时 `scale` 对齐（偶数化）；**启动前先跑一次设备枚举校验**，麦克风不在列表立即中文报错（不产静默无音轨文件）；系统声探测不到只降级并在 `RecordStarted.notice` 带一句提示；**停止：先向独立进程组发 `CTRL_BREAK`（2 秒内优雅收尾、moov 完整），未退或无 console（GUI 发布版）时 kill**——录制输出用 fragmented MP4（`empty_moov+default_base_moof` + 0.2s 切片 + `-flush_packets 1`），被 kill 的文件依然完整可播，最多丢最后 0.2s 切片（Windows 上管道 stdin 写 `q` 实测无效，ffmpeg 键盘交互走 console API，定案过程见 ffmpeg.rs 模块注释）；ffmpeg 子进程挂进 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object，应用任何路径退出（含崩溃）都不留孤儿进程继续录屏；ffmpeg 启动后 600ms 内退出（设备错/区域非法）会以 stderr 尾部报中文错误。
- **录制条入画**：录制条矩形与选区**相交时**才隐藏（拍板方案的隐藏），同时临时注册全局快捷键 `Ctrl+Shift+G`（与「开始录制」同键）结束录制 + 系统通知说明；录制结束注销快捷键，预览窗 `shot_record_preview` 原有的 `show()` 负责恢复窗口。选区不含录制条（多数场景）时录制条保持显示，Esc / 「结束」按钮照常可用——这是对拍板「录制期间隐藏」的工程化落地：无脑全隐藏会让用户失去唯一停止 UI。
- **Windows MP4 裁剪**（`trim_recording`）：默认重编码（`-ss` 输入侧快速 seek + 输出 `-t` 时长 + libx264/aac，精确到 0.001s），快速模式 `-c copy` 未做（按拍板）；ffmpeg 缺失给中文错误；GIF 分支零改动。
- **格式降级**：Windows 上 `mp4_supported()` 运行时判断（ffmpeg 就位与否）；缺失时请求 MP4 自动降级 GIF 并系统通知一次。
- **前端**：录屏条 MP4 格式下（仅 Windows）显示麦克风开关 + 设备下拉（`shot_record_audio_devices`）；ffmpeg 未就位显示「下载 ffmpeg」按钮，下载进度条就地展示；GIF 格式麦克风置灰 + 提示「GIF 无音轨」；麦克风偏好持久化到 `ShotSettings.micEnabled/micDevice`（`#[serde(default)]`，老配置兼容）；MP4 预览头部加「有声」徽标（`RecordResult.hasAudio`）。
- **新命令**：`shot_record_audio_devices` / `shot_record_ffmpeg_status` / `shot_record_ensure_ffmpeg`（自定义命令不走 ACL，capability 未改，`capabilities/rec.json` 的描述本来就写明录制工作全走自定义命令）。
- **测试**：`cargo test --lib` 938 passed / 0 failed / 3 ignored（基线 924 + 新增 14，全部为纯函数单测：定位顺序、URL 清单、dshow 解析、GBK 解码、loopback 匹配、录制/裁剪参数拼接、秒数格式化、zip 防伪、格式降级、录制条相交几何）；`npx tsc --noEmit` / `npx vite build` 通过。

### macOS 查证结论（拍板 3）

- **ScreenCaptureKit 原生支持麦克风**：`SCStreamConfiguration.captureMicrophone`（macOS 15.0+，WWDC24 session 10088 引入），且项目锁定的 `objc2-screen-capture-kit 0.3.2` 绑定**已生成** `setCaptureMicrophone(&self, bool)`（已从 docs.rs 0.3.2 页面核实）。
- **但本批留白，未接入**，理由：
  1. Apple 文档要求麦克风音频走 `SCStreamOutputTypeMicrophone` 输出类型的 sample handler，而 `sck.rs` 现有结构用的是 `SCRecordingOutput`（直写磁盘）——`captureMicrophone = true` 时 `SCRecordingOutput` 是否会把麦克风混进 MP4 无法在本机（Windows）验证，社区还有「开启后录制损坏」的报告（r/swift 2025）；
  2. 需要新增 `NSMicrophoneUsageDescription` 权限声明与运行时授权，属于 Tauri 配置 + Info.plist 改动，同样无法验证；
  3. 若 `SCRecordingOutput` 不写麦克风轨，就得改造成 sample handler 自行混流编码 = 大改，超出「最小侵入」。
- **后续在 mac 真机上验证的最小实验**（批 2 或专项）：在 `sck.rs` 的 `Session::start` 里给 `config` 加一行 `config.setCaptureMicrophone(mic_on)`，Info.plist 加 `NSMicrophoneUsageDescription`，录 10 秒说话检查产物音轨；能用就是三行改动接入 `RecordOptions.mic`，不能用再评估 sample handler 路线。**在验证之前绝不要动 sck.rs 已验证路径**。
- mac 的 MP4 裁剪（avconvert）与 `mod h264` / `mp4_track` 本批未动。

### 批 2 范围（未做）

- 任务 C：会议录音页（工具箱第 5 子页），采集层建议 cpal（避开 ffmpeg 常驻进程），落盘 `~/.agenthud/meetings/`；转写方案仍是产品决策，不要先写代码。
- macOS 麦克风：按上面的最小实验先行查证。
- `-c copy` 快速裁剪模式、鼠标点击高亮 / 聚光：未做。

### 与本文其余章节的差异

- 第 3 节 A.3① 的「下载到 `~/.agenthud/bin/`」已被用户拍板否决：实际下载到**应用 exe 同目录 `bin\`**；
- A.3⑤ 的参数模板已按双音频 amix / scale 对齐 / `-draw_mouse` 扩展，见 `ffmpeg.rs` 的 `build_record_args`（含单测快照）；
- 第 4 节的「先无损再重编码」双尝试被简化为**只做重编码**（用户拍板：避免 UI 复杂化）；
- `RecordResult` 新增 `hasAudio` 字段（仅 Serialize 方向，向后兼容）。

---


## 0. 结论先行（TL;DR）

1. **「Windows 端录屏裁剪」名不副实**：Windows 录出来的是 GIF，而 GIF 裁剪本来就是跨平台实现，已经能用。真正缺的是 Windows 的 **MP4 + 音轨** 整条链路。别在这个标题下写无用代码。
2. **麦克风 / 鼠标高亮 / 会议录音卡在同一个前提下**：Windows 端目前**没有任何音频采集与编码设施**，连 MP4 容器都没有（H.264 编码器是 macOS 的 VideoToolbox）。
3. **推荐一次性引入 ffmpeg sidecar**，把 MP4 录制、麦克风、系统声、精确裁剪、GIF 生成四件事一起解决；**不推荐**自己用 WASAPI + AAC 编码器拼一套（编码器没有干净可用的纯 Rust 实现，维护成本远高于打一个二进制）。
4. **会议记录的转写方案是产品决策**，本文只给三种路线的成本对比，**不要替用户选**（第 5 节）。

---

## 1. 代码地图（按阅读顺序）

| 文件 | 作用 | 关键符号 |
|---|---|---|
| `src-tauri/src/island/screenshot/record.rs`（约 2530 行） | 录制主逻辑，全部在这一个文件 | 见下表 |
| `src-tauri/src/island/screenshot.rs` | 截图/抓屏与公共设施；`capture_rect`、`ShotSettings`、`shot_start` 等 | `capture_rect`、`ShotSettings`(318) |
| `src-tauri/src/island/store.rs` | `ShotSettings` 的读写 | `load_shot_settings` / `save_shot_settings` |
| `src/rec/RecApp.tsx` | 录制条 + 预览 + 裁剪 UI | `trimmed`、`onGifFps`、`recordExport` |
| `src/shot/ShotApp.tsx` | 截图/录屏条（含录制格式选择） | `recFormat`、`startShot` |
| `src/shot/api.ts` | `shot_record_*` 的前端封装（带浏览器 mock） | `startRecord` / `stopRecord` / `recordPreview` / `recordExport` / `setGifFps` |
| `src-tauri/capabilities/rec.json` | 录制条窗口（label `shot-recorder`）的权限 | 目前只有 event + 拖拽 |
| `src-tauri/capabilities/island.json` | 灵动岛窗口权限 | 已含 dialog 读写 |

`record.rs` 里的关键位置：

| 行号 | 符号 | 说明 |
|---|---|---|
| 40 | `REC_WINDOW_TITLE` | 录制条窗口标题（macOS 用它把自己排除出画面） |
| 46 | `GIF_INTERVAL_MS` | GIF 帧间隔 33ms |
| 60-63 | `MP4_SUPPORTED` | `#[cfg(target_os = "macos")] true`，其它平台 false |
| 190 前后 | `struct RecordOptions` | `{ format, cursor, audio }`，wire 为 camelCase |
| 203 | `shot_record_start` | 命令入口，转 `start_recording` |
| 220 | `start_recording` | 校验、算输出尺寸、建目录、起线程 |
| 811 | `struct CaptureSpec` | 每帧抓取需要的几何参数 |
| 830 | `run_recording` | 录制主循环（macOS 优先走 `sck::Session`） |
| 1019 | `capture_frame` | 抓一帧：macOS 走 `mac_capture`，其它平台回落 `super::capture_rect` |
| 1051 | `enum Sink` | `Gif(GifSink)` 全平台；`Mp4(Mp4Sink)` **只在 macOS 编译** |
| 1085 | `struct GifSink` | GIF 帧缓冲与编码 |
| 1589 | `mod h264` | macOS VideoToolbox 编码器 |
| 2040 前后 | `mod mp4_track` | `#[cfg(target_os = "macos")]`，MP4 封装 |
| 2241 | `trim_recording` | GIF 分支跨平台；MP4 分支 macOS 走 `avconvert`，其它平台直接 `Err("当前系统暂不支持裁剪视频")` |
| 2324 | `clipboard_file` | 导出后复制文件（Windows 用 PowerShell `Set-Clipboard -Path`） |

前端录制格式的默认值在 `src/shot/ShotApp.tsx`：`IS_MAC ? "mp4" : "gif"`；后端 `normalize_format` 也会在 `MP4_SUPPORTED == false` 时把请求降级成 GIF。

---

## 2. 现状能力矩阵

| 能力 | macOS | Windows | 依据 |
|---|---|---|---|
| 区域抓帧 | `CGWindowListCreateImage` | `capture_rect`（与截图同一套） | `capture_frame`(1019) |
| 输出 GIF | ✅（帧序列 → gif crate） | ✅ | `Sink::Gif` |
| 输出 MP4 | ✅（ScreenCaptureKit 或 VideoToolbox） | ❌ 编译期就没有 | `MP4_SUPPORTED`(60-63)、`Sink::Mp4` |
| 系统声音 | ✅（SCK `setCapturesAudio`） | ❌ | `record/sck.rs` 只在 macOS 编译 |
| 麦克风 | ❌ | ❌ | 全局没有任何音频输入采集代码 |
| 裁剪 GIF | ✅ | ✅ | `trim_recording`(2241) |
| 裁剪 MP4 | ✅（avconvert） | ❌ 返回「当前系统暂不支持裁剪视频」 | 同左 |
| 鼠标指针入画 | ✅（`cursor` 选项） | 部分（随 `capture_rect` 是否带指针而定，未单独验证） | `CaptureSpec.cursor` |
| 鼠标点击高亮 / 聚光 | ❌ | ❌ | 无任何实现 |

**一句话**：Windows 用户今天只能录 GIF，视频没有声音；「Windows 裁剪」对它录出来的 GIF 已经成立，所以任务 B 只有在做了 MP4 之后才有意义。

---

## 3. 任务 A：录屏带麦克风（Windows 优先）

### A.1 需求边界（先和用户确认再动手）

- 是「录屏时把人声混进去」还是「单独录会议音频」？本文 A 做前者，C 做后者，两者共享采集层。
- 只做 Windows，还是 macOS 一起？**建议先只做 Windows**：本机是 Windows，macOS 路径（SCK / avconvert）无法在这里验证，改坏了没人发现。
- 系统声音要不要？Windows 上 ffmpeg 的 dshow 没有原生 loopback 设备，需要用户机器上有「立体声混音」或虚拟声卡；建议：**麦克风必做，系统声作为可选能力**，检测不到就不显示开关。

### A.2 三种方案对比

| 方案 | 视频/音频封装 | 依赖体积 | 风险 |
|---|---|---|---|
| **① ffmpeg sidecar（推荐）** | 全交给 ffmpeg：`gdigrab` + `dshow` + libx264 + aac | +60～100MB（Windows 单文件 ~80MB） | 分发/首次解压/杀软误报；但功能一次到位 |
| ② 纯 Rust（cpal + 编码器） | cpal 采麦克风；系统声要 WASAPI loopback（`windows` crate）；AAC 编码没有干净实现 | 小 | 编码器是硬伤：`fdk-aac` 要编译 C；Opus 进 MP4 兼容性差 |
| ③ 只做 macOS | SCK 已有系统声，加 audio input 混流 | 0 | 本机无法验证；Windows 用户完全无收益 |

**选方案 ①。** 理由：MP4 录制、音频混流、精确裁剪、GIF 生成可以共用同一个二进制；后续「会议录音」也能直接复用。

### A.3 落地步骤

**① 依赖与分发**
- 把 `ffmpeg.exe`（Windows）与 `ffmpeg`（macOS，若后续做）放进 Tauri `bundle.resources`，或首次使用时下载到 `~/.agenthud/bin/`。
- 定位顺序建议：环境变量 `AGENTHUD_FFMPEG` → `resource_dir/ffmpeg(.exe)` → `PATH` → 报错（中文，提示用户装或用设置里的路径）。
- macOS 打包注意签名/公证；Windows 注意 SmartScreen（可在 README 说明）。

**② 后端新模块**：建议 `src-tauri/src/island/screenshot/record/ffmpeg.rs`

```rust
pub struct Ffmpeg { path: PathBuf }

impl Ffmpeg {
    /// 定位二进制；找不到给中文错误
    pub fn locate() -> Result<Self, String>;
    /// 列出音频输入设备（Windows: dshow；macOS: avfoundation）
    pub fn audio_devices(&self) -> Result<Vec<AudioDevice>, String>;
    /// 启动录制；返回句柄（持有 child，stop 时发 "q" 收尾）
    pub fn start(&self, spec: &CaptureSpec, audio: &AudioTask) -> Result<FfmpegRecording, String>;
}

pub struct AudioDevice { pub name: String, pub label: String }
pub struct AudioTask { pub mic: Option<String>, pub system: bool }
```

Windows 设备枚举：`ffmpeg -hide_banner -f dshow -list_devices true -i dummy`，stderr 里解析 `"xxx" (audio)`。

**③ 命令与结构改动**
- `RecordOptions` 增加字段（保持 camelCase wire）：
  ```rust
  pub struct RecordOptions {
      pub format: Option<String>,     // 不动
      pub cursor: Option<bool>,       // 不动
      pub audio: Option<bool>,        // 语义改为「录系统声」，仅 mp4
      pub mic: Option<String>,        // 新增：麦克风设备名；None = 不录
  }
  ```
- 新增命令：`shot_record_audio_devices() -> Vec<AudioDevice>`（录制条下拉用）。
- `ShotSettings`（`screenshot.rs:318`）增加 `mic_device: String`、`mic_enabled: bool`，走 `store.rs` 的现有读写；`#[serde(default)]` 保证老配置文件兼容。
- `start_recording`(220)：当 `mic` 非空时走 ffmpeg 分支；否则保持现有路径（GIF 或 macOS SCK）**完全不变**，降低回归面。

**④ 前端改动**
- `src/shot/ShotApp.tsx`：录屏分类下增加「麦克风」开关 + 设备下拉（调 `shot_record_audio_devices`），选择持久化到 `ShotSettings`。
- `src/rec/RecApp.tsx`：MP4 预览里加音量图标（可选）；GIF 时麦克风开关置灰并提示「GIF 无音轨」。
- capability 不用加：`shot_record_*` 是自定义命令，不走 ACL。

**⑤ ffmpeg 参数模板（Windows）**

```
ffmpeg -y -hide_banner -loglevel error ^
  -f gdigrab -framerate 30 -offset_x {x} -offset_y {y} -video_size {w}x{h} -i desktop ^
  -f dshow -i audio="{mic}" ^
  -c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p ^
  -c:a aac -b:a 160k -movflags +faststart "{out}.mp4"
```
- 无麦克风时：去掉 `-f dshow -i ...` 与 `-c:a aac`（不能留空音频输入）。
- 系统声：dshow 设备名形如「立体声混音 (Realtek(R) Audio)」；检测不到就不传，别硬编码。
- 停止：stdin 写 `q` + `wait()`；超时 3s 后再 `kill()`。**不要**只 kill，否则 mp4 收不到 moov。
- 录制条入画：Windows 没有 macOS 的 `below` 排除机制（`recorder_window_number()` 返回 0）。二选一：录制期间隐藏录制条窗口，或把录制条移出选区。

**⑥ 错误与降级**
- 找不到 ffmpeg → 中文错误 + 设置页提示，**回落到 GIF**（不要让整个录屏不可用）。
- 麦克风设备被占用/拔掉 → 录制直接失败并提示设备名，不做静默无音轨。
- 磁盘写入失败 → 停止并给出路径。

### A.4 验收标准

- Windows 录 10 秒区域视频：输出 MP4，播放器能听到麦克风声音；
- 关掉麦克风开关：MP4 无音轨但能正常播放；
- 没有 ffmpeg 的机器：自动回落 GIF，提示一次，不崩；
- GIF 路径完全不变（帧率 10 可调、裁剪、复制文件都正常）；
- macOS 编译通过（不做行为承诺，若没做就不要动 mac 分支）。

### A.5 测试方法

1. `cargo test --lib record`（现有单测在 `record.rs` 末尾，`encodes_frames_into_playable_mp4` 只在 macOS 跑）。
2. 真机：录 10 秒说话 → 用系统播放器确认音轨存在（Windows：文件属性→详细信息→「音频」）。
3. 拔麦克风再录 → 预期是明确报错，不是无声文件。
4. 录 5 分钟 → 检查时长与文件大小（约 30fps 1080p ≈ 2～4MB/s，需提示用户 `-crf` 可调）。

---

## 4. 任务 B：Windows 端录屏裁剪

**现状**：Windows 只产出 GIF，`trim_recording` 的 GIF 分支已经跨平台，**今天就能裁**。所以本任务 = 「做了任务 A 之后，让 MP4 也能在 Windows 上裁」。

### 实现

`trim_recording`(2241) 增加：

```rust
#[cfg(not(target_os = "macos"))]
{
    // 先尝试无损（关键帧对齐），失败再重编码
    let ff = Ffmpeg::locate()?;
    let ok = ff.run(&[
        "-y", "-ss", &format!("{:.3}", start_ms / 1000.0),
        "-to", &format!("{:.3}", end_ms / 1000.0),
        "-i", source, "-c", "copy", "-avoid_negative_ts", "make_zero", out,
    ]).is_ok();
    if !ok {
        ff.run(&["-y", "-ss", … , "-i", source, "-c:v", "libx264", "-c:a", "copy", out])?;
    }
    return Ok(out);
}
```

注意：
- `-c copy` 只能按关键帧切，误差可达 1～2 秒；预览里的「裁剪」期望是精确到 0.1s 的，**默认走重编码**，把 `-c copy` 作为「快速模式」选项（可选）。
- 现有 `shot_record_export` 的入参是 `start_ms/end_ms: f64`，别改成整数。
- GIF 分支不要动。

### 验收

- 裁 0.5s～3.2s → 输出时长 2.7s ±100ms；
- 裁剪后仍能复制到剪贴板 / 保存；
- 不裁剪（区间 == 全片）→ 直接复用源文件，不做二次编码；
- 没有 ffmpeg 时给出中文错误而不是 panic。

---

## 5. 任务 C：会议 / 语音记录

### C.1 最小可用（不依赖转写）

**新页面**：建议放灵动岛「工具箱」的第 5 个子页（现有 4 个：二维码 / 图片 / 片段 / 启动；注册方式见第 7 节），标题「会议」。

- 开始 / 暂停 / 继续 / 结束；计时、输入设备选择、电平条（可选）。
- 落盘：`~/.agenthud/meetings/{YYYYMMDD-HHMM}.wav`（16kHz / 16bit / 单声道 ≈ 1.9MB/分钟）+ 同名 `.json` 侧车：`{ started_at, duration_ms, device, marks: [{ms, label}] }`。
- 写入用「临时文件 + rename」，进程被杀不产生 0 字节文件。
- 列表：最近若干条，可打开所在文件夹、删除、重命名。
- 首次使用弹一次隐私提示：**录音会写到本机磁盘**，需用户确认。

### C.2 转写（需要用户决策，先别实现）

| 路线 | 成本 | 体验 | 备注 |
|---|---|---|---|
| 本地 whisper（whisper-rs / sherpa-onnx） | 模型 75MB（tiny）～1.5GB（large）；whisper-rs 需要 cmake/LLVM | 离线、隐私好、中文小模型准确率一般 | 首选 sherpa-onnx（有预编译产物，省编译工具链） |
| 不转写 | 0 | 只有录音 + 标记 | 最快落地；文案要写清楚 |
| 外部 API | 需要密钥、联网 | 准确率高 | 与产品「本机处理」的定位冲突 |

**不要**在这个决策落定前写转写代码。可以在 UI 上先放「转写（即将支持）」的占位。

### C.3 验收

- 录 30 秒中文 → 回放清晰、时长正确；
- 点暂停再继续 → 音频不丢段、侧车 marks 正确；
- 录制中杀进程 → 目录里没有 0 字节 wav；
- 磁盘满 / 设备被占用 → 明确中文错误。

---

## 6. 不要做 / 容易踩的坑

1. **不要给 GIF 加音轨**：GIF 规范没有音轨，前端开关要能识别并置灰。
2. **不要动 macOS 已验证路径**（SCK、`avconvert`、`mod h264`）：没有 mac 机器就不要改，`#[cfg]` 保持原样。
3. **`RecordOptions.audio` 的语义**现在是「系统声」且仅 MP4 生效；前端默认传 true，别让 GIF 路径因此报错。
4. **录制条入画**：macOS 靠 `below` 参数排除录制条，Windows 没有等价机制——引入 ffmpeg 后必须处理（隐藏/移出选区），否则每条视频左上角都有个计时器。
5. **停止录制必须优雅收尾**（stdin `q` 或发送事件），直接 kill 会让 MP4 没有 moov atom，文件打不开。
6. **前端参数名是 camelCase**（`recordId`、`startMs`、`endMs`），Rust 侧 snake_case，Tauri 自动映射，别手写转换。
7. **单位**：时长、裁剪区间都是毫秒 `f64`；GIF 帧间隔固定 33ms（`GIF_INTERVAL_MS`），别按 30fps 算 33.33。
8. **测试不要真开麦克风依赖**：单元测试只测纯函数（参数拼接、设备名解析、路径生成），录音本身留给手测。

---

## 7. 可复用设施（照着用，别重造）

| 需求 | 现成方案 |
|---|---|
| 系统通知 | `crate::notify(title, body)`（`src-tauri/src/lib.rs`，内部按平台分流，失败静默） |
| 设置持久化 | `ShotSettings`（`island/screenshot.rs:318`）+ `island/store.rs` 的 load/save，`#[serde(rename_all="camelCase", default)]` |
| 岛内新页面 | `src/island/types.ts` 的 `TabKey` → `src/island/IslandApp.tsx` 的 `TABS` + `renderPage()` 的 `case`；工具页子页在 `src/island/ToolboxPage.tsx` |
| 新命令注册 | `src-tauri/src/lib.rs` 的 `tauri::generate_handler![...]`；自定义命令不受 ACL 限制 |
| 窗口权限 | `src-tauri/capabilities/*.json`；录制条是 `rec.json`、岛是 `island.json` |
| 前端浏览器预览 | `src/island/mock.ts` + `.preview/harness/`（gitignore；截图与版式验证都走它） |
| 后端单测风格 | 模块内 `#[cfg(test)] mod tests`；文件 IO 用 `std::env::temp_dir()` + uuid 子目录，结束删除 |
| 中文错误文案 | 直接返回 `Result<_, String>`，前端 toast/内联展示，不做错误码 |

---

## 8. 回归清单（改完必须过）

- [ ] `cargo test --lib`（当前基线 924 passed / 0 failed / 3 ignored）
- [ ] `npx tsc --noEmit`、`npx vite build`
- [ ] macOS 能编译（`cargo check --lib` 不能只在 Windows 过）——如无 mac 机器，至少确认没有把 `#[cfg(target_os = "macos")]` 分支改坏
- [ ] 截图 / 滚动截图 / OCR / 贴图 不受影响
- [ ] GIF 录制 → 停止 → 裁剪 → 导出 全链路
- [ ] 没有 ffmpeg 的机器启动不报错、录屏回落 GIF
- [ ] 岛内 8 个标签切换正常，工具页 4 个子页正常
- [ ] `cargo test --lib record` 里的 MP4 单测在 macOS 上仍通过（如果有条件）

---

## 9. 需要先问用户的三个问题

1. **ffmpeg sidecar 能不能接受体积增加**（Windows 安装包 +60～100MB）？不能的话只能做「只录麦克风到 WAV」的降级版。
2. **会议记录要不要转写**？本地模型 / 不转写 / 外部 API——这决定是否引入 ONNX/whisper 依赖与首次模型下载体验。
3. **macOS 要不要一起做**？如果需要，请在有 mac 的机器上验证 SCK 音频输入与 `avconvert` 裁剪，否则建议先只交付 Windows。

---

## 附：本文参考的关键代码片段（便于快速定位）

```rust
// record.rs:60-63 —— Windows 没有 MP4
#[cfg(target_os = "macos")]
const MP4_SUPPORTED: bool = true;
#[cfg(not(target_os = "macos"))]
const MP4_SUPPORTED: bool = false;

// record.rs:1051 —— 只有 macOS 编译 Mp4 分支
enum Sink {
    Gif(GifSink),
    #[cfg(target_os = "macos")]
    Mp4(Mp4Sink),
}

// record.rs:2241 —— 非 macOS 的 MP4 裁剪直接失败
#[cfg(not(target_os = "macos"))]
{
    let _ = (source, start_ms, end_ms);
    Err("当前系统暂不支持裁剪视频".into())
}
```

```rust
// record.rs:190 前后 —— 现有选项（要在这里加 mic）
pub struct RecordOptions {
    pub format: Option<String>,   // "mp4" | "gif"
    pub cursor: Option<bool>,     // 是否画鼠标
    pub audio: Option<bool>,      // MP4 是否录系统声
}
```
