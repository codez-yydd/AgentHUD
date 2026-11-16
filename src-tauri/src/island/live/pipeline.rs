//! 实时字幕线程编排：采集主循环 + 定稿识别 + 草稿识别 + 翻译。
//!
//! ## 线程模型
//!
//! ```text
//! cpal 采集线程（audio::start 内部 keeper）
//!   └─ on_chunk → sync_channel(512) → VAD 主循环（本模块 run_pipeline）
//!        ├─ Segmenter::push → 完整句 → final_tx ──→ 定稿识别线程（1 个）
//!        │                                          └─ 定稿后并发翻译（最多 4 条）
//!        └─ current_utterance 快照 ──→ 草稿识别线程（1 个，可让路）
//!                                     └─ 草稿翻译线程（1 个，单飞 + 节流）
//! ```
//!
//! 定稿 / 草稿两个识别线程共用一份 `AsrEngine`（内部 `Mutex` 串行），与参考
//! 实现一致：识别的原生上下文只允许串行进入。
//!
//! ## 关键行为
//!
//! - **非阻塞启动**：`live_start` 只建会话并返回 `phase = starting`；625MB
//!   模型在后台线程加载，成功后 → `listening`，失败 → `phase = error` +
//!   中文原因并清掉会话。重复调用单飞。
//! - **草稿**：整段当前句重识别；音频 ≥0.7s 才出草稿；定稿忙时让路；两次
//!   草稿间隔 `max(draft_interval_ms, 单次耗时 / 0.5)`；句子定稿 / 重置后
//!   用「代号（generation）」丢弃迟到的旧草稿。
//! - **翻译**：定稿翻译失败重试 1 次（401/402/403 不重试）；草稿翻译单飞、
//!   节流（`len >= 6` 且新增字符 ≥ 设置值，间隔 `max(设置值, 耗时 * 1.2)`），
//!   失败静默；星号过多的草稿结果丢弃。
//! - **level**：RMS → `(20*log10(rms)+60)/60` clamp 0..1，≤250ms 推一次；
//!   WASAPI loopback 在系统没播放时完全不回调，静默 400ms 后电平自行落 0。
//! - **单份模型（P1-2）**：模型占位锁（`LOAD_LOCK`）在整个会话期间持有，收尾
//!   时先等定稿 / 草稿线程退出、模型引用计数归零才释放；快速 stop→start 时
//!   新会话的 pipeline 线程在后台等旧会话完全退出，任一时刻最多一份 625MB
//!   模型驻留（不会出现约 1.25GB 峰值）。
//! - **停止收尾（对参考实现的改进）**：停止时用 `Segmenter::current_utterance()`
//!   兜底补一次定稿（≥0.25s，最多等 3s，失败静默），避免 loopback 静音不回调
//!   导致最后一句丢失；`live_stop` 命令只置停止标志后立即返回，采集 join 与
//!   pipeline 收尾都放后台线程，不阻塞主线程（P2-6）。
//! - **非 Windows**：系统声音不可用，`live_start` 直接返回中文错误；
//!   麦克风照常。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tauri::AppHandle;

use super::asr::AsrEngine;
use super::audio;
use super::download;
use super::subtitles;
use super::translate as live_translate;
use super::vad::Segmenter;
use super::{LiveSettings, LiveStatus, TARGET_RATE};
use crate::island::translate::LiveTranslateTarget;

/// 采集块队列容量：约 512 × 10ms ≈ 5s 音频；满了丢新块，绝不阻塞回调。
const CHUNK_QUEUE: usize = 512;
/// 电平推送最小间隔（低配降频：128ms → 250ms，减少前端事件与重绘压力）。
const LEVEL_INTERVAL: Duration = Duration::from_millis(250);
/// 采集静默超过该时长后电平落 0（loopback 静音不回调）。
const LEVEL_IDLE: Duration = Duration::from_millis(400);
/// 草稿线程轮询间隔。
const DRAFT_POLL: Duration = Duration::from_millis(100);
/// 草稿识别最小音频长度。
const DRAFT_MIN_AUDIO_MS: u32 = 700;
/// 草稿重识别要求音频至少再增长的量（避免对同一段音频反复识别）。
const DRAFT_MIN_GROWTH_MS: u32 = 250;
/// 草稿翻译轮询间隔。
const DRAFT_TRANSLATE_POLL: Duration = Duration::from_millis(80);
/// 草稿翻译最短原文长度（字符数）。
const DRAFT_TRANSLATE_MIN_CHARS: usize = 6;
/// 停止兜底收尾：最短音频 250ms。
const TAIL_MIN_MS: u32 = 250;
/// 停止兜底收尾：最多等定稿 3s。
const TAIL_TIMEOUT: Duration = Duration::from_secs(3);
/// 定稿翻译并发上限（超出丢句并记日志，避免翻译慢时线程堆积）。
const FINAL_TRANSLATE_CONCURRENCY: usize = 4;
/// 草稿翻译失败退避基数：连续失败 2s → 4s → 8s（上限），避免 429 放大 /
/// 同一段日语按最小间隔反复重发导致计费翻倍（P2-3）。
const DRAFT_TRANSLATE_BACKOFF_BASE_MS: u64 = 2_000;
const DRAFT_TRANSLATE_BACKOFF_MAX_MS: u64 = 8_000;

/// 模型占位锁：从加载模型一直持有到会话完全收尾（模型 Arc 归零）。
///
/// 注意它**不是**只包住「加载」动作：快速 stop→start 时，旧会话的收尾线程
/// （补最后一句话最长 3s，且定稿 / 草稿线程还各持一份识别引擎）会继续持有
/// 模型，新会话的 pipeline 线程必须先阻塞在这把锁上，等旧模型彻底释放后才
/// 加载，保证任一时刻最多一份 625MB 模型驻留（P1-2）。
static LOAD_LOCK: Mutex<()> = Mutex::new(());

/// 一句送定稿识别的音频。
struct FinalJob {
    samples: Vec<f32>,
}

// ============================================================
// 启停
// ============================================================

/// 开始聆听（非阻塞）。返回最新状态快照，不抛异常。
pub(crate) fn start(app: &AppHandle) -> LiveStatus {
    super::bind_app(app);
    // 单飞前置检查：已在启动 / 聆听 / 等模型下载时，重复调用直接回当前状态
    // （下面的参数校验不能把运行中的会话误标成 error）。
    if super::lock_state().session.is_some() {
        return super::current_status();
    }
    let cfg = super::settings();
    // 系统声音采集只有 Windows 有（WASAPI loopback）；其它平台明确报错，
    // 而不是悄悄退化成麦克风（参考契约：loopback 返回中文错误，mic 可用）。
    #[cfg(not(target_os = "windows"))]
    if cfg.audio_source != "mic" {
        return super::error_status("系统声音采集仅支持 Windows，请在实时字幕设置里把采集来源改为麦克风");
    }
    let root = match super::models_root() {
        Ok(root) => root,
        Err(error) => return super::error_status(&error),
    };
    let ready = super::models_ready_at(&root);
    let began = {
        let mut guard = super::lock_state();
        if guard.session.is_some() {
            // 单飞：已在启动 / 聆听 / 等模型下载，重复调用只回当前状态。
            None
        } else {
            guard.epoch += 1;
            let epoch = guard.epoch;
            let download = guard.status.download.clone();
            guard.status = LiveStatus {
                running: true,
                phase: "starting".into(),
                error: String::new(),
                model_ready: ready,
                source: cfg.audio_source.clone(),
                device: cfg.audio_device.clone(),
                level: 0.0,
                download,
            };
            guard.session = Some(super::Session {
                epoch,
                stop: Arc::new(AtomicBool::new(false)),
                audio: None,
                pipeline: None,
            });
            Some(epoch)
        }
    };
    if began.is_none() {
        return super::current_status();
    }
    super::emit_current_status();
    if ready {
        super::spawn_loader_if_pending(app);
    } else {
        // 模型缺失：自动下载并在完成后自动开始（参考实现同口径）。
        download::start(app, true);
    }
    super::current_status()
}

/// 停止聆听：置停止标志 → 立即返回；后台线程收尾（补最后一句话）后变 idle。
///
/// P2-6：采集 keeper 的 join（设备释放）和 pipeline 线程的 reaper join 都放到
/// 后台线程，命令本身不阻塞主线程（`island::stop_services` 关岛时同步调用）。
pub(crate) fn stop(app: Option<&AppHandle>) -> LiveStatus {
    if let Some(app) = app {
        super::bind_app(app);
    }
    let session = {
        let mut guard = super::lock_state();
        let session = guard.session.take();
        if let Some(session) = &session {
            session.stop.store(true, Ordering::SeqCst);
        }
        guard.status.running = false;
        guard.status.level = 0.0;
        let has_pipeline = session
            .as_ref()
            .map(|session| session.pipeline.is_some())
            .unwrap_or(false);
        if has_pipeline {
            // 等 pipeline 线程收尾（最后一句定稿最多 3s），完成后再回 idle。
            guard.status.phase = "stopping".into();
        } else {
            // 从没真正开始（还在等模型下载 / 加载线程已结束）：直接回 idle。
            guard.status.phase = "idle".into();
            guard.status.error.clear();
        }
        session
    };
    // 正在为「自动开始」下载模型：一并取消（.part 保留）。
    download::cancel_pending();
    if let Some(mut session) = session {
        // 采集 keeper 线程看到 stop 后 ≤50ms 停流并在线程内 drop stream；这里
        // 只把句柄交给后台线程 join，主线程立即返回（P2-6）。pipeline 线程的
        // 收尾（最后一句定稿 ≤3s）同理，后台 reaper join 完即结束。
        let audio = session.audio.take();
        let pipeline = session.pipeline.take();
        let spawned = thread::Builder::new()
            .name("island-live-reaper".into())
            .spawn(move || {
                if let Some(audio) = audio {
                    let _ = audio.join();
                }
                if let Some(pipeline) = pipeline {
                    let _ = pipeline.join();
                }
            });
        if let Err(error) = spawned {
            eprintln!("[zbar-island-live] 启动停止收尾线程失败: {error}");
        }
    }
    let status = super::current_status();
    super::emit_status(&status);
    status
}

// ============================================================
// pipeline 线程主体
// ============================================================

/// pipeline 线程入口（`island-live-pipeline`）。
pub(crate) fn run(app: AppHandle, epoch: u64, stop: Arc<AtomicBool>) {
    // 线程内 panic 也必须落状态 + 置 stop：否则定稿 / 草稿线程收不到停止信号、
    // 一直持有模型 Arc，`LOAD_LOCK` 会被永久占用（下一会话再也加载不了模型）。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_pipeline(&app, epoch, &stop)
    }));
    stop.store(true, Ordering::SeqCst);
    match result {
        Ok(Ok(())) => super::finish_epoch(epoch, "idle", None),
        Ok(Err(message)) => {
            eprintln!("[zbar-island-live] 实时字幕启动失败: {message}");
            super::finish_epoch(epoch, "error", Some(message));
        }
        Err(_) => {
            eprintln!("[zbar-island-live] 实时字幕线程异常退出，会话已重置");
            super::finish_epoch(epoch, "error", Some("实时字幕线程异常退出，请重试".into()));
        }
    }
}

fn run_pipeline(_app: &AppHandle, epoch: u64, stop: &Arc<AtomicBool>) -> Result<(), String> {
    // 模型占位锁：**整个会话期间持有**（不是只包加载动作）。快速 stop→start 时
    // 旧会话的收尾线程（补最后一句 ≤3s，定稿 / 草稿线程还各持一份识别引擎）会
    // 阻塞在这里的新会话 pipeline 线程之前，保证任一时刻最多一份模型驻留（P1-2）。
    // 本函数返回时 `engine` / `segmenter` 先于这把锁释放（声明顺序保证），
    // 且两个识别线程已在收尾处 join，模型引用计数必然归零。
    let _model_guard = LOAD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cfg = super::settings();
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }
    // 1. 加载识别 / VAD 模型（占位锁保证同一时刻只有一份 625MB 模型）。
    let root = super::models_root()?;
    if !super::models_ready_at(&root) {
        return Err("识别模型未下载完成，请先下载模型（约 626MB）".into());
    }
    let engine = Arc::new(AsrEngine::create(
        &super::asr_model_dir(&root),
        cfg.asr_final_threads,
    )?);
    let segmenter = Arc::new(Mutex::new(Segmenter::new(
        &super::vad_model_path(&root),
        cfg.vad_threshold,
        cfg.vad_min_silence_ms,
        cfg.vad_min_speech_ms,
        cfg.asr_max_segment_s,
    )?));
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }

    // 2. 翻译目标只解析一次（未配置 / 未启用 → 只出日文）。
    let translate_target: Option<LiveTranslateTarget> = if cfg.translate_enabled {
        live_translate::target_or_warn()
    } else {
        None
    };

    // 3. 定稿 / 草稿 / 草稿翻译线程。
    let final_busy = Arc::new(AtomicBool::new(false));
    let draft_gen = Arc::new(AtomicU64::new(0));
    let (final_tx, final_rx) = mpsc::channel::<FinalJob>();
    let (final_done_tx, final_done_rx) = mpsc::channel::<()>();
    let final_worker = spawn_final_worker(
        Arc::clone(&engine),
        Arc::clone(&final_busy),
        final_rx,
        final_done_tx,
        cfg.clone(),
        translate_target.clone(),
        Arc::clone(stop),
    )?;
    let draft_worker = match spawn_draft_worker(
        Arc::clone(&engine),
        Arc::clone(&segmenter),
        Arc::clone(stop),
        Arc::clone(&final_busy),
        Arc::clone(&draft_gen),
        cfg.clone(),
    ) {
        Ok(worker) => worker,
        Err(error) => {
            // 定稿线程已经起来且持一份模型 Arc：等它退出再返回，否则下一会话
            // 会在旧模型还没释放时再加载一份（P1-2）。
            drop(final_tx);
            join_thread(final_worker, "定稿识别");
            return Err(error);
        }
    };
    let draft_translator = match (cfg.translate_live, translate_target.as_ref()) {
        (true, Some(target)) => match spawn_draft_translator(
            Arc::clone(stop),
            Arc::clone(&final_busy),
            cfg.clone(),
            target.clone(),
        ) {
            Ok(worker) => Some(worker),
            Err(error) => {
                // 两个识别线程都已起来并各持一份模型 Arc：置 stop 等它们退出
                // （模型引用归零）再返回，保证锁释放即模型已释放（P1-2）。
                stop.store(true, Ordering::SeqCst);
                drop(final_tx);
                join_model_workers(final_worker, draft_worker, None);
                return Err(error);
            }
        },
        _ => None,
    };

    // 4. 起采集（阻塞到 stream play 成功；失败同步返回中文错误）。
    let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<f32>>(CHUNK_QUEUE);
    let audio_handle = match audio::start(
        &cfg.audio_source,
        &cfg.audio_device,
        Arc::clone(stop),
        move |chunk: &[f32]| {
            // 采集回调里绝不阻塞：队列满就丢块（丢的是静音 / 极端过载，不拖住音频线程）。
            let _ = chunk_tx.try_send(chunk.to_vec());
        },
    ) {
        Ok(handle) => handle,
        Err(error) => {
            // 让定稿线程退出：丢发送端 + 已置 stop（`run` 收尾会置位；这里提前置）。
            stop.store(true, Ordering::SeqCst);
            drop(final_tx);
            // 音频没起来也要等两个识别线程退出：它们各持一份模型 Arc，提前返回
            // 会让下一会话在旧模型还没释放时再加载一份（P1-2）。
            join_model_workers(final_worker, draft_worker, draft_translator);
            return Err(error);
        }
    };
    super::register_audio_handle(epoch, audio_handle);
    // P2-1：建流成功后的 listening 必须带 epoch 校验——stop() 取走会话后旧会话
    // 不得把状态从 stopping / idle 翻回 listening。
    super::publish_status_for_epoch(epoch, |status| {
        status.running = true;
        status.phase = "listening".into();
        status.error.clear();
        status.level = 0.0;
        status.model_ready = true;
        status.source = cfg.audio_source.clone();
        status.device = cfg.audio_device.clone();
    });

    // 5. VAD 主循环。
    let mut last_level_at = Instant::now() - LEVEL_INTERVAL;
    let mut last_audio_at = Instant::now();
    let mut level = 0.0f32;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match chunk_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                last_audio_at = Instant::now();
                level = level_from_rms(&chunk);
                if last_level_at.elapsed() >= LEVEL_INTERVAL {
                    last_level_at = Instant::now();
                    // P2-1：level 同样带 epoch，stop 后旧会话不得继续写状态。
                    super::publish_status_for_epoch(epoch, |status| status.level = level);
                }
                feed_chunk(&segmenter, &chunk, &final_tx, &draft_gen);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // loopback 在系统没播放时完全不回调：电平要自己落 0，否则
                // 胶囊上的电平条会停在最后一帧。
                if level > 0.0 && last_audio_at.elapsed() >= LEVEL_IDLE {
                    level = 0.0;
                    if last_level_at.elapsed() >= LEVEL_INTERVAL {
                        last_level_at = Instant::now();
                        super::publish_status_for_epoch(epoch, |status| status.level = 0.0);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // 6. 收尾：先把采集队列里还没消费的块倒进 VAD，再补一次「当前句」定稿。
    while let Ok(chunk) = chunk_rx.try_recv() {
        feed_chunk(&segmenter, &chunk, &final_tx, &draft_gen);
    }
    let tail_min = audio::plan_block(TARGET_RATE, TAIL_MIN_MS);
    let tail = {
        let mut guard = segmenter.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let utterance = guard.current_utterance();
        let tail = if utterance.len() >= tail_min {
            utterance.to_vec()
        } else {
            Vec::new()
        };
        // 会话结束：清空 VAD / 当前句状态，避免残留到下一次 start。
        guard.reset();
        tail
    };
    if !tail.is_empty() {
        let _ = final_tx.send(FinalJob { samples: tail });
    }
    drop(final_tx);
    // 等定稿线程把队列（含收尾句）处理完；超时也放行（后台线程会自己写完）。
    let _ = final_done_rx.recv_timeout(TAIL_TIMEOUT);
    // 采集设备释放：keeper 线程看到 stop 后 ≤50ms 退出。
    if let Some(handle) = super::take_audio_handle(epoch) {
        let _ = handle.join();
    }
    // P1-2 收尾关键：定稿 / 草稿线程各持一份 `AsrEngine` Arc，必须等它们退出
    // （引用计数归零、625MB 模型真正释放）才让本函数返回并放掉 `LOAD_LOCK`；
    // 否则下一会话会并行加载第二份模型。这两个 join 都在后台 pipeline 线程里，
    // 不阻塞命令响应（极端情况下最多再等最后一两次识别跑完）。
    join_model_workers(final_worker, draft_worker, draft_translator);
    Ok(())
}

/// 等两个识别线程退出并释放它们持有的模型引用（`draft_translator` 只持翻译
/// 目标、不持模型，直接 drop 即可）。
///
/// 这是 P1-2 的收尾动作：`run_pipeline` 的 `engine`（本地 Arc）在这些线程退出
/// 后才可能是最后一个引用，随后与 `LOAD_LOCK` 一起释放，保证任一时刻最多一份
/// 模型驻留。线程 panic 只记日志，不把错误往上抛（收尾路径已在停止流程里）。
fn join_model_workers(
    final_worker: JoinHandle<()>,
    draft_worker: JoinHandle<()>,
    draft_translator: Option<JoinHandle<()>>,
) {
    join_thread(final_worker, "定稿识别");
    join_thread(draft_worker, "草稿识别");
    drop(draft_translator);
}

/// join 一个收尾线程；panic 只记日志（模型引用随线程结束一起释放）。
fn join_thread(handle: JoinHandle<()>, label: &str) {
    if handle.join().is_err() {
        eprintln!("[zbar-island-live] {label}线程异常退出，模型引用已随线程释放");
    }
}

/// 喂一块 16k 音频进 VAD；完整句进定稿队列，并让旧草稿代号失效。
fn feed_chunk(
    segmenter: &Mutex<Segmenter>,
    chunk: &[f32],
    final_tx: &mpsc::Sender<FinalJob>,
    draft_gen: &AtomicU64,
) {
    let segments = {
        let mut guard = segmenter.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.push(chunk)
    };
    for segment in segments {
        // 时间轴异常（t1 <= t0）的碎片直接丢，不浪费一次整句识别。
        if segment.t1 <= segment.t0 {
            continue;
        }
        // 句子结束：此前基于这段音频的草稿结果一律作废（draft_gen 变化）。
        draft_gen.fetch_add(1, Ordering::SeqCst);
        if final_tx
            .send(FinalJob {
                samples: segment.samples,
            })
            .is_err()
        {
            break;
        }
    }
}

// ============================================================
// 定稿识别线程
// ============================================================

fn spawn_final_worker(
    engine: Arc<AsrEngine>,
    final_busy: Arc<AtomicBool>,
    final_rx: mpsc::Receiver<FinalJob>,
    final_done_tx: mpsc::Sender<()>,
    cfg: LiveSettings,
    translate_target: Option<LiveTranslateTarget>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, String> {
    thread::Builder::new()
        .name("island-live-final".into())
        .spawn(move || {
            let active_translations = Arc::new(AtomicUsize::new(0));
            while let Ok(job) = final_rx.recv() {
                final_busy.store(true, Ordering::SeqCst);
                let result = engine.transcribe(&job.samples);
                final_busy.store(false, Ordering::SeqCst);
                match result {
                    Ok(text) => {
                        let text = text.trim();
                        if text.is_empty() {
                            // 没听清：把草稿冻结为定稿，避免历史里留下永远
                            // `draft = true` 的幽灵条目。
                            subtitles::freeze_draft_as_final();
                        } else {
                            let caption = subtitles::finalize(text);
                            // P2-8：已停止就不再为收尾句发起翻译（同步在飞翻译
                            // 由 translate_final 的回调 stop 检查中止）。
                            if cfg.translate_enabled && !stop.load(Ordering::SeqCst) {
                                if let Some(target) = translate_target.as_ref() {
                                    spawn_final_translation(
                                        &cfg,
                                        target,
                                        caption.id,
                                        text.to_string(),
                                        Arc::clone(&active_translations),
                                        Arc::clone(&stop),
                                    );
                                }
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("[zbar-island-live] 定稿识别失败: {error}");
                    }
                }
            }
            let _ = final_done_tx.send(());
        })
        .map_err(|error| format!("启动定稿识别线程失败：{error}"))
}

/// 定稿翻译并发（每条一个短线程；超过上限丢句，翻译慢时不堆积）。
fn spawn_final_translation(
    cfg: &LiveSettings,
    target: &LiveTranslateTarget,
    id: u64,
    jp: String,
    active: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
) {
    if stop.load(Ordering::SeqCst) {
        return;
    }
    if active.fetch_add(1, Ordering::SeqCst) >= FINAL_TRANSLATE_CONCURRENCY {
        active.fetch_sub(1, Ordering::SeqCst);
        eprintln!("[zbar-island-live] 定稿翻译并发已满，跳过一句（id={id}）");
        return;
    }
    let context = translation_context(cfg, id);
    let target = target.clone();
    let counter = Arc::clone(&active);
    let spawned = thread::Builder::new()
        .name("island-live-translate".into())
        .spawn(move || {
            translate_final(id, &jp, &context, &target, &stop);
            counter.fetch_sub(1, Ordering::SeqCst);
        });
    if spawned.is_err() {
        active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 定稿翻译：失败重试 1 次；401/402/403（Key / 余额 / 权限）不重试。
///
/// P2-8：停止后不再往字幕历史写译文——回调里先查 stop，已停止立即中止流。
fn translate_final(
    id: u64,
    jp: &str,
    context: &[String],
    target: &LiveTranslateTarget,
    stop: &AtomicBool,
) {
    for attempt in 0..2 {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let result = live_translate::stream(target, jp, context, &mut |text| {
            if stop.load(Ordering::SeqCst) {
                return false;
            }
            subtitles::set_final_zh(id, text)
        });
        match result {
            Ok(()) => return,
            Err(error) => {
                let retryable = !matches!(error.status, Some(401 | 402 | 403));
                if attempt == 1 || !retryable {
                    eprintln!("[zbar-island-live] 定稿翻译失败（id={id}）: {error}");
                    return;
                }
                thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

// ============================================================
// 草稿识别线程
// ============================================================

fn spawn_draft_worker(
    engine: Arc<AsrEngine>,
    segmenter: Arc<Mutex<Segmenter>>,
    stop: Arc<AtomicBool>,
    final_busy: Arc<AtomicBool>,
    draft_gen: Arc<AtomicU64>,
    cfg: LiveSettings,
) -> Result<JoinHandle<()>, String> {
    thread::Builder::new()
        .name("island-live-draft".into())
        .spawn(move || {
            let mut last_gen = u64::MAX;
            let mut last_len = 0usize;
            let mut next_at = Instant::now();
            let min_samples = audio::plan_block(TARGET_RATE, DRAFT_MIN_AUDIO_MS);
            let growth_samples = audio::plan_block(TARGET_RATE, DRAFT_MIN_GROWTH_MS);
            while !stop.load(Ordering::SeqCst) {
                thread::sleep(DRAFT_POLL);
                if !cfg.asr_partial {
                    continue;
                }
                if final_busy.load(Ordering::SeqCst) {
                    // 定稿忙时让路：草稿识别会抢同一把识别器锁。
                    continue;
                }
                if Instant::now() < next_at {
                    continue;
                }
                let (samples, generation) = {
                    let guard = segmenter.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    (
                        guard.current_utterance().to_vec(),
                        draft_gen.load(Ordering::SeqCst),
                    )
                };
                if generation != last_gen {
                    // 上一句已定稿 / 会话重置：草稿重新计时。
                    last_gen = generation;
                    last_len = 0;
                }
                if samples.len() < min_samples {
                    continue;
                }
                if last_len > 0 && samples.len() <= last_len + growth_samples {
                    continue;
                }
                let started = Instant::now();
                let result = engine.transcribe(&samples);
                let cost_ms = started.elapsed().as_millis() as u64;
                // 间隔 = max(设置值, 单次耗时 / 0.5)：识别越慢，草稿越稀。
                let wait_ms = cfg.asr_draft_interval_ms.max(cost_ms.saturating_mul(2));
                next_at = started + Duration::from_millis(wait_ms);
                if draft_gen.load(Ordering::SeqCst) != generation {
                    // 识别期间这句话已定稿（或会话重置）：迟到的草稿丢弃。
                    continue;
                }
                match result {
                    Ok(text) => {
                        let text = text.trim();
                        last_len = samples.len();
                        if text.is_empty() || live_translate::star_heavy(text) {
                            continue;
                        }
                        // P2-8：停止后不再往历史写草稿（避免定稿收尾后又冒出
                        // 一条永远 draft = true 的幽灵条目）。
                        if stop.load(Ordering::SeqCst) {
                            continue;
                        }
                        subtitles::update_draft(text);
                    }
                    Err(error) => {
                        last_len = samples.len();
                        eprintln!("[zbar-island-live] 草稿识别失败: {error}");
                    }
                }
            }
        })
        .map_err(|error| format!("启动草稿识别线程失败：{error}"))
}

// ============================================================
// 草稿翻译线程（单飞 + 节流）
// ============================================================

fn spawn_draft_translator(
    stop: Arc<AtomicBool>,
    final_busy: Arc<AtomicBool>,
    cfg: LiveSettings,
    target: LiveTranslateTarget,
) -> Result<JoinHandle<()>, String> {
    thread::Builder::new()
        .name("island-live-zh-draft".into())
        .spawn(move || {
            let mut last_id = u64::MAX;
            let mut last_text = String::new();
            let mut next_at = Instant::now();
            // 连续失败次数：2s → 4s → 8s（上限）指数退避（P2-3）。
            let mut fail_streak = 0u32;
            while !stop.load(Ordering::SeqCst) {
                thread::sleep(DRAFT_TRANSLATE_POLL);
                // 定稿忙时让路：定稿翻译的优先级更高。
                if final_busy.load(Ordering::SeqCst) {
                    continue;
                }
                let Some((id, jp)) = subtitles::active_draft() else {
                    continue;
                };
                if id != last_id {
                    last_id = id;
                    last_text.clear();
                    next_at = Instant::now();
                    fail_streak = 0;
                }
                let chars = jp.chars().count();
                if chars < DRAFT_TRANSLATE_MIN_CHARS {
                    continue;
                }
                let new_chars = if jp.starts_with(&last_text) {
                    chars.saturating_sub(last_text.chars().count())
                } else {
                    chars
                };
                if new_chars < cfg.translate_live_min_new_chars {
                    continue;
                }
                if Instant::now() < next_at {
                    continue;
                }
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let context = translation_context(&cfg, id);
                let started = Instant::now();
                let mut aborted = false;
                let mut noisy = false;
                let result = live_translate::stream(&target, &jp, &context, &mut |text| {
                    // P2-8：已停止就不再往字幕历史写译文。
                    if stop.load(Ordering::SeqCst) {
                        return false;
                    }
                    if live_translate::star_heavy(text) {
                        // 模型把整段输出成星号 / Markdown：中止并清掉半截译文。
                        noisy = true;
                        return false;
                    }
                    if !subtitles::set_draft_zh(id, text) {
                        // 这句话已经定稿 / 被清空：让路给定稿翻译。
                        aborted = true;
                        return false;
                    }
                    true
                });
                let cost = started.elapsed();
                let base_wait_ms = cfg
                    .translate_live_min_interval_ms
                    .max((cost.as_millis() as u64).saturating_mul(12) / 10);
                next_at = started + Duration::from_millis(base_wait_ms);
                if aborted {
                    // 句子已定稿 / 被清空：记下原文，避免下一拍按「新增字符」把
                    // 同一段日语再发一遍（P2-3）。
                    last_text = jp;
                    eprintln!("[zbar-island-live] 草稿翻译中止（id={id}，句子已定稿 / 清空）");
                    continue;
                }
                if noisy {
                    // 星号噪声：清掉半截译文，并记下原文别原样重发（P2-3）。
                    let _ = subtitles::set_draft_zh(id, "");
                    last_text = jp;
                    fail_streak = 0;
                    eprintln!("[zbar-island-live] 草稿翻译输出星号噪声，已丢弃（id={id}）");
                    continue;
                }
                match result {
                    Ok(()) => {
                        // 翻译成功（含流被提前中止）：记下已翻过的原文，算新增字符。
                        last_text = jp;
                        fail_streak = 0;
                    }
                    Err(error) => {
                        // P2-3：失败也记下原文，并按 2s/4s/8s 退避。否则每一拍
                        // 都按最小间隔把同一段日语重发，429 放大且翻译计费翻倍。
                        last_text = jp;
                        fail_streak = fail_streak.saturating_add(1);
                        let backoff_ms = draft_translate_backoff_ms(fail_streak);
                        next_at = started + Duration::from_millis(base_wait_ms.max(backoff_ms));
                        eprintln!(
                            "[zbar-island-live] 草稿翻译失败（id={id}，连续 {fail_streak} 次，{backoff_ms}ms 后重试）: {error}"
                        );
                    }
                }
            }
        })
        .map_err(|error| format!("启动草稿翻译线程失败：{error}"))
}

// ============================================================
// 小工具
// ============================================================

/// 翻译上文：`live.asr.context_prompt` 开启时取最近 ≤3 句定稿日文。
fn translation_context(cfg: &LiveSettings, id: u64) -> Vec<String> {
    if !cfg.asr_context_prompt {
        return Vec::new();
    }
    let limit = cfg.translate_context_lines.min(3);
    if limit == 0 {
        return Vec::new();
    }
    subtitles::context_before(id, limit)
}

/// 草稿翻译连续失败的退避：第 1 / 2 / ≥3 次分别 2s / 4s / 8s（上限 8s）。
fn draft_translate_backoff_ms(fail_streak: u32) -> u64 {
    let shift = fail_streak.saturating_sub(1).min(2);
    (DRAFT_TRANSLATE_BACKOFF_BASE_MS << shift).min(DRAFT_TRANSLATE_BACKOFF_MAX_MS)
}

/// 音频块 RMS → 0..1 电平：`(20*log10(rms) + 60) / 60`，clamp 到 0..1。
fn level_from_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples
        .iter()
        .map(|sample| {
            let value = if sample.is_finite() { *sample } else { 0.0 };
            value * value
        })
        .sum();
    let rms = (sum / samples.len() as f32).sqrt();
    if !rms.is_finite() || rms <= 1e-7 {
        return 0.0;
    }
    (((20.0 * rms.log10()) + 60.0) / 60.0).clamp(0.0, 1.0)
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn level_formula_matches_contract() {
        assert_eq!(level_from_rms(&[]), 0.0);
        assert_eq!(level_from_rms(&[0.0, 0.0, 0.0]), 0.0);
        // 满幅方波 RMS = 1 → 20log10(1) = 0dB → (0+60)/60 = 1.0
        assert!((level_from_rms(&[1.0, -1.0, 1.0, -1.0]) - 1.0).abs() < 1e-6);
        // 幅度 0.5 的方波：20log10(0.5) ≈ -6.02dB → (53.98)/60 ≈ 0.8997
        let level = level_from_rms(&[0.5, -0.5, 0.5, -0.5]);
        assert!((level - 0.8997).abs() < 1e-3, "实际电平 {level}");
        // 很小但不为零：落在 0..1 内
        let quiet = level_from_rms(&[1e-4, -1e-4]);
        assert!((0.0..=1.0).contains(&quiet));
        // NaN / inf 不传染
        assert_eq!(level_from_rms(&[f32::NAN, f32::INFINITY]), 0.0);
    }

    /// P2-3：草稿翻译失败退避必须按 2s → 4s → 8s 增长并在 8s 封顶。
    #[test]
    fn draft_translate_backoff_grows_to_cap() {
        assert_eq!(draft_translate_backoff_ms(1), 2_000);
        assert_eq!(draft_translate_backoff_ms(2), 4_000);
        assert_eq!(draft_translate_backoff_ms(3), 8_000);
        assert_eq!(draft_translate_backoff_ms(9), 8_000, "退避封顶 8s");
    }

    #[test]
    fn translation_context_respects_switch_and_cap() {
        let mut cfg = super::super::LiveSettings {
            models_dir: String::new(),
            audio_source: "mic".into(),
            audio_device: String::new(),
            vad_threshold: 0.4,
            vad_min_silence_ms: 350,
            vad_min_speech_ms: 200,
            asr_partial: true,
            asr_draft_interval_ms: 500,
            asr_final_threads: 6,
            asr_max_segment_s: 10,
            asr_context_prompt: false,
            translate_enabled: true,
            translate_live: true,
            translate_live_min_interval_ms: 900,
            translate_live_min_new_chars: 4,
            translate_context_lines: 10,
        };
        // 关闭：不带上文（不读字幕历史，避免并行测试互相影响）
        assert!(translation_context(&cfg, 1).is_empty());
        cfg.asr_context_prompt = true;
        // 开启但上下文句数被钳到 0：仍为空
        cfg.translate_context_lines = 0;
        assert!(translation_context(&cfg, 1).is_empty());
    }

    /// 真实 VAD + ASR 冒烟（默认 ignore）：按 80ms 块把真实日语音频喂进
    /// `Segmenter`，把断出的段送 `AsrEngine`，断言识别文本含日文字符。
    ///
    /// 三个环境变量缺省时自动跳过（不碰仓库外资源）：
    /// - `AGENTHUD_LIVE_MODEL_DIR`：`parakeet-ja-0.6b-int8/` 目录（含
    ///   `model.int8.onnx` + `tokens.txt`）；
    /// - `AGENTHUD_LIVE_VAD_MODEL`：`silero_vad.onnx` 路径；
    /// - `AGENTHUD_LIVE_TEST_WAV`：16k/48k 单声道 16bit 日语测试音频。
    ///
    /// 运行：`cargo test --lib island::live::pipeline -- --ignored --nocapture`
    #[test]
    #[ignore = "需要本地 625MB 日语模型 + silero_vad.onnx + 日语测试 wav"]
    fn real_vad_asr_smoke() {
        let model_dir = match std::env::var("AGENTHUD_LIVE_MODEL_DIR") {
            Ok(value) => PathBuf::from(value),
            Err(_) => {
                eprintln!("跳过：未设置 AGENTHUD_LIVE_MODEL_DIR");
                return;
            }
        };
        let vad_model = match std::env::var("AGENTHUD_LIVE_VAD_MODEL") {
            Ok(value) => PathBuf::from(value),
            Err(_) => {
                eprintln!("跳过：未设置 AGENTHUD_LIVE_VAD_MODEL");
                return;
            }
        };
        let wav = match std::env::var("AGENTHUD_LIVE_TEST_WAV") {
            Ok(value) => PathBuf::from(value),
            Err(_) => {
                eprintln!("跳过：未设置 AGENTHUD_LIVE_TEST_WAV");
                return;
            }
        };
        if !super::super::file_nonempty(&vad_model) {
            eprintln!("跳过：VAD 模型不存在 {}", vad_model.display());
            return;
        }
        if !model_dir.join(super::super::ASR_MODEL_FILE).is_file()
            || !model_dir.join(super::super::ASR_TOKENS_FILE).is_file()
        {
            eprintln!("跳过：{} 下没有完整识别模型", model_dir.display());
            return;
        }
        let samples = match read_test_wav(&wav) {
            Some(samples) => samples,
            None => {
                eprintln!("跳过：无法解析测试音频 {}", wav.display());
                return;
            }
        };
        assert!(
            samples.len() > TARGET_RATE as usize * 3,
            "测试音频应至少 3 秒，实际 {} 样本",
            samples.len()
        );

        let engine = AsrEngine::create(&model_dir, 2).expect("加载真实日语模型");
        let mut segmenter =
            Segmenter::new(&vad_model, 0.4, 350, 200, 10).expect("加载真实 Silero VAD");

        let chunk = audio::plan_block(TARGET_RATE, 80);
        let mut segments = Vec::new();
        for piece in samples.chunks(chunk) {
            segments.extend(segmenter.push(piece));
        }
        // 尾部补 1s 静音，让 VAD 把最后一句吐出来（真实管线同样是静音断句）。
        let silence = vec![0.0f32; TARGET_RATE as usize];
        for piece in silence.chunks(chunk) {
            segments.extend(segmenter.push(piece));
        }
        if segments.is_empty() {
            // 停得早 / VAD 没断句：按 pipeline 停止收尾同口径用当前句兜底。
            let tail = segmenter.current_utterance().to_vec();
            assert!(
                tail.len() >= audio::plan_block(TARGET_RATE, TAIL_MIN_MS),
                "13 秒日语音频不应一段都切不出来"
            );
            segments.push(super::super::vad::SegmentAudio {
                samples: tail,
                t0: 0.0,
                t1: 0.0,
            });
        }

        let mut text = String::new();
        for segment in &segments {
            let piece = engine
                .transcribe(&segment.samples)
                .expect("识别真实日语音频");
            eprintln!(
                "片段 {:.2}s..{:.2}s（{} 样本）→ {piece}",
                segment.t0,
                segment.t1,
                segment.samples.len()
            );
            text.push_str(&piece);
        }
        let text = text.trim().to_string();
        eprintln!("真实冒烟识别文本：{text}");
        assert!(!text.is_empty(), "真实日语音频应识别出文本");
        assert!(
            text.chars().any(is_japanese_char),
            "识别文本应含日文字符，实际：{text}"
        );
    }

    /// 日文字符判定：平假名 / 片假名 / 汉字 / 々。
    fn is_japanese_char(ch: char) -> bool {
        matches!(ch,
            '\u{3040}'..='\u{309F}'   // 平假名
            | '\u{30A0}'..='\u{30FF}' // 片假名
            | '\u{4E00}'..='\u{9FFF}' // 汉字
            | '\u{3005}'              // 々
        )
    }

    /// 最小 RIFF/WAVE 解析：只支持单声道 16bit（测试音频来源固定）。
    /// 采样率不是 16k 时用 sherpa 的线性重采样器转成 16k（与采集链路同口径）。
    fn read_test_wav(path: &Path) -> Option<Vec<f32>> {
        let bytes = std::fs::read(path).ok()?;
        if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return None;
        }
        let mut channels = 1u16;
        let mut rate = 0u32;
        let mut bits = 0u16;
        let mut data: &[u8] = &[];
        let mut pos = 12usize;
        while pos + 8 <= bytes.len() {
            let id = [bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]];
            let size = u32::from_le_bytes([
                bytes[pos + 4],
                bytes[pos + 5],
                bytes[pos + 6],
                bytes[pos + 7],
            ]) as usize;
            pos += 8;
            let body = bytes.get(pos..pos + size)?;
            if id == *b"fmt " && size >= 16 {
                channels = u16::from_le_bytes([body[2], body[3]]);
                rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                bits = u16::from_le_bytes([body[14], body[15]]);
            } else if id == *b"data" {
                data = body;
                break;
            }
            pos += size + (size % 2);
        }
        if channels != 1 || bits != 16 || rate == 0 || data.is_empty() {
            return None;
        }
        let mono: Vec<f32> = data
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0)
            .collect();
        if rate == TARGET_RATE {
            return Some(mono);
        }
        let resampler = sherpa_onnx::LinearResampler::create(rate as i32, TARGET_RATE as i32)?;
        Some(resampler.resample(&mono, true))
    }
}
