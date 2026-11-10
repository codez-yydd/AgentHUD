//! ScreenCaptureKit 录屏：系统级流式抓屏（60fps）+ 系统声音，直接写成 MP4。
//!
//! 只在 macOS 15+ 走这条路径：`SCStream` 负责抓屏/拾音，`SCRecordingOutput`
//! 负责编码和写容器，我们只管开流、停流、等文件收尾。相比自己
//! `CGWindowListCreateImage` + VideoToolbox 逐帧编码，好处是：
//! - 帧率由系统给（30/60fps 都行），抓屏几乎不占 CPU；
//! - 能带系统声音（`capturesAudio`），自己抓屏做不到；
//! - 录出来的时间轴就是真实时间，不会因为抓帧慢而掉帧。
//!
//! 起流失败（没权限 / 老系统）时调用方会回退到逐帧抓屏的实现。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSString, NSURL};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCRecordingOutput, SCRecordingOutputConfiguration,
    SCRecordingOutputDelegate, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate,
};

struct DelegateState {
    /// 录制输出把文件彻底写完（moov 落盘）后置位
    finished: AtomicBool,
    error: Mutex<Option<String>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AgentHUDRecorderDelegate"]
    #[ivars = DelegateState]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl SCStreamDelegate for Delegate {}

    unsafe impl SCRecordingOutputDelegate for Delegate {
        #[unsafe(method(recordingOutputDidFinishRecording:))]
        fn did_finish(&self, _output: &SCRecordingOutput) {
            self.ivars().finished.store(true, Ordering::SeqCst);
        }

        #[unsafe(method(recordingOutput:didFailWithError:))]
        fn did_fail(&self, _output: &SCRecordingOutput, error: &NSError) {
            if let Ok(mut slot) = self.ivars().error.lock() {
                *slot = Some(error.localizedDescription().to_string());
            }
            self.ivars().finished.store(true, Ordering::SeqCst);
        }
    }
);

impl Delegate {
    fn create() -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateState {
            finished: AtomicBool::new(false),
            error: Mutex::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub struct Session {
    stream: Retained<SCStream>,
    delegate: Retained<Delegate>,
    #[allow(dead_code)]
    output: Retained<SCRecordingOutput>,
}

/// 异步接口（拿可录内容 / 起流 / 停流）都靠 block 回调，统一用 channel 等结果。
fn error_text(error: *mut NSError) -> Option<String> {
    if error.is_null() {
        return None;
    }
    Some(unsafe { (*error).localizedDescription().to_string() })
}

fn shareable_content(timeout: Duration) -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = mpsc::channel::<usize>();
    let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        if content.is_null() {
            let _ = tx.send(0);
            return;
        }
        // 回调线程和调用线程不同，Retained 不能直接跨线程，这里把 +1 的
        // 引用转成裸指针传过去，接收侧立刻 from_raw 接回所有权。
        let retained = unsafe { Retained::retain(content) };
        let raw = retained.map(Retained::into_raw).unwrap_or(std::ptr::null_mut());
        let _ = tx.send(raw as usize);
        let _ = error;
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    let raw = rx
        .recv_timeout(timeout)
        .map_err(|_| "获取可录内容超时".to_string())?;
    if raw == 0 {
        return Err("拿不到可录内容：请确认已授予「屏幕录制」权限".into());
    }
    unsafe { Retained::from_raw(raw as *mut SCShareableContent) }
        .ok_or_else(|| "可录内容已失效".to_string())
}

impl Session {
    /// `rect` 是全局逻辑点矩形（主屏左上为原点），`point_scale` 是显示器缩放。
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        rect: (f64, f64, f64, f64),
        out_w: u32,
        out_h: u32,
        fps: u32,
        cursor: bool,
        audio: bool,
        path: &Path,
    ) -> Result<Self, String> {
        let (x, y, w, h) = rect;
        if w < 1.0 || h < 1.0 {
            return Err("录制区域过小".into());
        }
        let content = shareable_content(Duration::from_secs(6))?;
        let displays = unsafe { content.displays() };
        let center = CGPoint::new(x + w / 2.0, y + h / 2.0);
        let mut chosen: Option<Retained<SCDisplay>> = None;
        let mut chosen_frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
        for display in displays.iter() {
            let frame = unsafe { display.frame() };
            let inside = center.x >= frame.origin.x
                && center.x < frame.origin.x + frame.size.width
                && center.y >= frame.origin.y
                && center.y < frame.origin.y + frame.size.height;
            if inside {
                chosen_frame = frame;
                chosen = Some(display);
                break;
            }
        }
        let display = match chosen {
            Some(display) => display,
            None => displays
                .iter()
                .next()
                .ok_or_else(|| "没有可录的显示器".to_string())?,
        };

        // sourceRect 用的是「相对该显示器」的点坐标
        let local = CGRect::new(
            CGPoint::new(x - chosen_frame.origin.x, y - chosen_frame.origin.y),
            CGSize::new(w, h),
        );

        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            )
        };

        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setWidth(out_w.max(2) as usize);
            config.setHeight(out_h.max(2) as usize);
            config.setShowsCursor(cursor);
            config.setCapturesAudio(audio);
            config.setExcludesCurrentProcessAudio(true);
            if audio {
                config.setSampleRate(48_000);
                config.setChannelCount(2);
            }
            config.setSourceRect(local);
            config.setDestinationRect(CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(out_w.max(2) as f64, out_h.max(2) as f64),
            ));
            config.setScalesToFit(false);
            config.setPreservesAspectRatio(true);
            config.setMinimumFrameInterval(CMTime {
                value: 1,
                timescale: fps.clamp(1, 120) as i32,
                flags: CMTimeFlags::Valid,
                epoch: 0,
            });
        }

        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let output_config = unsafe { SCRecordingOutputConfiguration::new() };
        unsafe { output_config.setOutputURL(&url) };

        let delegate = Delegate::create();
        let output = unsafe {
            SCRecordingOutput::initWithConfiguration_delegate(
                SCRecordingOutput::alloc(),
                &output_config,
                ProtocolObject::from_ref(&*delegate),
            )
        };
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*delegate)),
            )
        };
        unsafe { stream.addRecordingOutput_error(&output) }
            .map_err(|error| format!("添加录制输出失败: {}", error.localizedDescription()))?;

        let (tx, rx) = mpsc::channel::<Option<String>>();
        let block = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(error_text(error));
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
        match rx.recv_timeout(Duration::from_secs(6)) {
            Ok(None) => Ok(Self {
                stream,
                delegate,
                output,
            }),
            Ok(Some(message)) => Err(format!("开始录制失败: {message}")),
            Err(_) => Err("开始录制超时".into()),
        }
    }

    pub fn finished(&self) -> bool {
        self.delegate.ivars().finished.load(Ordering::SeqCst)
    }

    pub fn error(&self) -> Option<String> {
        self.delegate
            .ivars()
            .error
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// 停流并等录制输出把 MP4 收尾（moov 写盘）。
    pub fn stop(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel::<Option<String>>();
        let block = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(error_text(error));
        });
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&block)) };
        let stop_error = rx.recv_timeout(Duration::from_secs(10)).ok().flatten();

        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if let Some(message) = self.error() {
            return Err(format!("录制收尾失败: {message}"));
        }
        if let Some(message) = stop_error {
            return Err(format!("停止录制失败: {message}"));
        }
        if !self.finished() {
            return Err("录制收尾超时（文件可能不完整）".into());
        }
        Ok(())
    }
}
