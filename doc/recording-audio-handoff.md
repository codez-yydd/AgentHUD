# 录屏音频 / Windows 裁剪 / 会议录音：实施交接

> 目标读者：接手的实施 agent。读完这份应该能直接动手，不需要再猜现状。
> 编写时间：2026-11-12；对应工作区状态在提交 `8941c62` 之后（灵动岛工具箱、片段库、启动器、剪贴板增强已合入，这三项**都没有做**）。
> 本文里的行号以当前工作区为准，改动后请重新定位符号，不要硬编码行号。

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
