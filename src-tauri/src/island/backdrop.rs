//! 桌面底板采样。开启「不出现在截图和录屏里」时，DXGI 提供与面板位置对齐的
//! 桌面纹理。相邻区域只用于自动主题的亮度判断，采样避开窗口回采。
//! 采集过程不修改窗口采集标志，避免录制画面闪烁。

#[cfg(windows)]
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::sync::Arc;
#[cfg(windows)]
use std::sync::Mutex;
#[cfg(windows)]
use windows::Win32::Foundation::{HWND, RECT};

#[cfg(windows)]
#[path = "backdrop/desktop.rs"]
mod desktop;

#[cfg(windows)]
pub(super) fn reserve_native_capture() -> bool {
    desktop::reserve_for_native()
}

#[cfg(windows)]
pub(super) fn release_native_capture() {
    desktop::release_from_native();
}

/// 相邻区域按比例下采样，保留自动主题亮度判断所需的代表性；缩略图短边
/// 上限 256（曾为 320：每帧解码位图越大，前端光栅内存压力越大，256 在
/// 亮度统计上已远超充分采样密度）。
#[cfg(windows)]
const DOWNSCALE: u32 = 3;

#[derive(Clone, serde::Serialize)]
pub struct BackdropFrame {
    pub revision: u32,
    pub url: Option<Arc<str>>,
    pub aligned: bool,
    /// 相邻区域帧的平均亮度（0..=1，Rust 侧随帧算好；Aligned / Unavailable
    /// 路径为 `None`——全窗折射帧不参与自动主题，前端不需要亮度）。
    pub luminance: Option<f32>,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameSource {
    Aligned,
    Neighbor { width: u32, height: u32, hash: u64 },
    Unavailable,
}

#[cfg(any(windows, test))]
#[derive(Default)]
struct FrameCache {
    latest: Option<(FrameSource, BackdropFrame)>,
}

#[cfg(windows)]
static BACKDROP: Mutex<FrameCache> = Mutex::new(FrameCache { latest: None });

#[cfg(any(windows, test))]
impl FrameCache {
    fn reply(&self, known_revision: Option<u32>) -> Option<BackdropFrame> {
        self.latest
            .as_ref()
            .map(|(_, frame)| frame)
            .filter(|frame| Some(frame.revision) != known_revision)
            .cloned()
    }

    fn update(&mut self, source: FrameSource, url: Option<String>, luminance: Option<f32>) {
        let revision = self
            .latest
            .as_ref()
            .map_or(1, |(_, frame)| frame.revision.wrapping_add(1));
        self.latest = Some((
            source,
            BackdropFrame {
                revision,
                url: url.map(Arc::from),
                aligned: source == FrameSource::Aligned,
                luminance,
            },
        ));
    }

    fn aligned(&mut self, url: String, known_revision: Option<u32>) -> Option<BackdropFrame> {
        let unchanged = self.latest.as_ref().is_some_and(|(source, frame)| {
            *source == FrameSource::Aligned && frame.url.as_deref() == Some(url.as_str())
        });
        if !unchanged {
            self.update(FrameSource::Aligned, Some(url), None);
        }
        self.reply(known_revision)
    }

    fn neighbor(
        &mut self,
        source: FrameSource,
        luminance: Option<f32>,
        known_revision: Option<u32>,
        encode: impl FnOnce() -> Result<String, String>,
    ) -> Result<Option<BackdropFrame>, String> {
        if !self
            .latest
            .as_ref()
            .is_some_and(|(latest, _)| *latest == source)
        {
            self.update(source, Some(encode()?), luminance);
        }
        Ok(self.reply(known_revision))
    }

    fn unavailable(&mut self, known_revision: Option<u32>) -> Option<BackdropFrame> {
        if !matches!(self.latest.as_ref(), Some((FrameSource::Unavailable, _))) {
            self.update(FrameSource::Unavailable, None, None);
        }
        self.reply(known_revision)
    }

    fn during_capture(
        &mut self,
        excluded: bool,
        sample_neighbor: bool,
        known_revision: Option<u32>,
    ) -> Option<BackdropFrame> {
        match self.latest.as_ref().map(|(source, _)| *source) {
            Some(FrameSource::Aligned) if excluded => self.reply(known_revision),
            Some(FrameSource::Neighbor { .. }) if sample_neighbor => self.reply(known_revision),
            _ => self.unavailable(known_revision),
        }
    }
}

#[cfg(windows)]
fn backdrop_cache() -> Result<std::sync::MutexGuard<'static, FrameCache>, String> {
    BACKDROP
        .lock()
        .map_err(|_| "桌面底板缓存不可用".to_string())
}

/// 缩略图像素的 FNV-1a 64 位内容哈希（无依赖；只为相邻两拍的等值判断，
/// 不承担抗碰撞安全职责——壁纸类画面哈希碰撞时最坏表现为少更新一拍底板，
/// 下一拍内容变化自愈）。
#[cfg(windows)]
fn thumbnail_hash(image: &image::RgbaImage) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in image.as_raw() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 相邻区域缩略图的平均亮度（0..=1）。公式与被替换掉的前端
/// `sampleBackdropLuminance` 完全一致：Rec.709 加权
/// (0.2126R + 0.7152G + 0.0722B) / 255，alpha < 8 的像素不计入，
/// 没有任何不透明像素时返回 `None`。
///
/// 在 Rust 侧随帧算好（capture 已持有 `RgbaImage`），前端邻域采样就不再
/// 需要 `new Image()` + `decode()` 解码链——那条链的解码位图释放全靠 GC，
/// 页面空闲时 GC 极少触发，正是渲染进程原生内存无上限堆积的来源。
#[cfg(any(windows, test))]
fn average_luminance(image: &image::RgbaImage) -> Option<f32> {
    let mut total = 0f32;
    let mut count = 0u32;
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0;
        if a < 8 {
            continue;
        }
        total += (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0;
        count += 1;
    }
    (count > 0).then(|| total / count as f32)
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn neighbor(width: u32, height: u32) -> FrameSource {
        FrameSource::Neighbor {
            width,
            height,
            hash: 7,
        }
    }

    /// 亮度纯函数（与被替换的前端 sampleBackdropLuminance 公式对齐）
    #[test]
    fn luminance_of_flat_colors_matches_reference_weights() {
        use image::RgbaImage;
        let white = RgbaImage::from_pixel(4, 4, image::Rgba([255, 255, 255, 255]));
        assert!((average_luminance(&white).unwrap() - 1.0).abs() < 1e-5);
        let black = RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]));
        assert!(average_luminance(&black).unwrap().abs() < 1e-5);
        // 纯红按 Rec.709 权重：0.2126
        let red = RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]));
        assert!((average_luminance(&red).unwrap() - 0.2126).abs() < 1e-4);
    }

    /// alpha < 8 的像素不计入；全透明图没有亮度（前端保持上一拍平滑值）
    #[test]
    fn luminance_skips_transparent_pixels_and_none_when_fully_clear() {
        use image::RgbaImage;
        let mut mixed = RgbaImage::from_pixel(2, 1, image::Rgba([255, 255, 255, 255]));
        mixed.put_pixel(0, 0, image::Rgba([255, 255, 255, 0]));
        assert!((average_luminance(&mixed).unwrap() - 1.0).abs() < 1e-5);
        let clear = RgbaImage::from_pixel(2, 2, image::Rgba([120, 120, 120, 0]));
        assert!(average_luminance(&clear).is_none());
    }

    #[test]
    fn resends_unacknowledged_neighbor_without_encoding_again() {
        let mut cache = FrameCache::default();
        let source = neighbor(10, 16);
        let first = cache
            .neighbor(source, Some(0.5), None, || Ok("neighbor".into()))
            .unwrap()
            .unwrap();
        assert_eq!(first.luminance, Some(0.5));
        let no_reencode = || panic!("cached pixels must not be re-encoded");
        let missed = cache
            .neighbor(source, Some(0.5), None, no_reencode)
            .unwrap()
            .unwrap();
        assert_eq!(first.revision, missed.revision);
        assert!(!missed.aligned);
        assert_eq!(missed.luminance, Some(0.5));
        assert!(Arc::ptr_eq(&first.url.unwrap(), &missed.url.unwrap()));
        assert!(cache
            .neighbor(source, None, Some(first.revision), no_reencode)
            .unwrap()
            .is_none());
        let resized = cache
            .neighbor(neighbor(16, 10), None, Some(first.revision), || {
                Ok("resized".into())
            })
            .unwrap()
            .unwrap();
        assert_ne!(resized.revision, first.revision);
        // 缩放后是另一帧：未提供亮度时不再沿用旧帧的值
        assert_eq!(resized.luminance, None);
    }

    #[test]
    fn returning_to_same_neighbor_pixels_after_aligned_frame_publishes_them() {
        let mut cache = FrameCache::default();
        let source = neighbor(10, 16);
        let initial = cache
            .neighbor(source, None, None, || Ok("neighbor".into()))
            .unwrap()
            .unwrap();
        let aligned = cache
            .aligned("aligned".into(), Some(initial.revision))
            .unwrap();
        assert!(aligned.aligned);
        assert!(cache
            .aligned("aligned".into(), Some(aligned.revision))
            .is_none());
        let neighbor_again = cache
            .neighbor(source, None, Some(aligned.revision), || Ok("neighbor".into()))
            .unwrap()
            .unwrap();
        assert_ne!(aligned.revision, neighbor_again.revision);
        assert!(!neighbor_again.aligned);
        assert_eq!(neighbor_again.url.as_deref(), Some("neighbor"));
    }

    #[test]
    fn unavailable_clears_aligned_frame_and_is_resendable_until_acknowledged() {
        let mut cache = FrameCache::default();
        let aligned = cache.aligned("aligned".into(), None).unwrap();
        let missing = cache.unavailable(Some(aligned.revision)).unwrap();
        assert_ne!(missing.revision, aligned.revision);
        assert!(!missing.aligned);
        assert_eq!(
            serde_json::to_value(&missing).unwrap().get("url"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(cache.unavailable(None).unwrap().revision, missing.revision);
        assert!(cache.unavailable(Some(missing.revision)).is_none());

        let recovered = cache
            .aligned("aligned".into(), Some(missing.revision))
            .unwrap();
        assert_ne!(recovered.revision, missing.revision);
        assert!(recovered.aligned);
    }

    #[test]
    fn paused_capture_only_reuses_frames_allowed_in_the_current_mode() {
        let mut cache = FrameCache::default();
        let aligned = cache.aligned("aligned".into(), None).unwrap();
        assert_eq!(
            cache.during_capture(true, false, None).unwrap().revision,
            aligned.revision
        );
        let missing = cache
            .during_capture(false, true, Some(aligned.revision))
            .unwrap();
        assert!(missing.url.is_none());
        let neighbor = cache
            .neighbor(neighbor(10, 16), None, Some(missing.revision), || {
                Ok("neighbor".into())
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            cache.during_capture(false, true, None).unwrap().revision,
            neighbor.revision
        );
        assert!(cache
            .during_capture(false, false, Some(neighbor.revision))
            .unwrap()
            .url
            .is_none());
    }
}

/// 窗口所在显示器的物理边界和句柄。
#[cfg(target_os = "windows")]
fn monitor_for(hwnd: HWND) -> Option<desktop::Monitor> {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    Some(desktop::Monitor { id: monitor.0 as usize, bounds: info.rcMonitor })
}

#[cfg(windows)]
fn window_rect(hwnd: HWND) -> Option<RECT> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

#[cfg(windows)]
fn bounding_rect(a: RECT, b: RECT) -> RECT {
    RECT {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    }
}

#[cfg(windows)]
fn sample_outside_motion(sample: RECT, before: RECT, after: RECT) -> bool {
    let window = bounding_rect(before, after);
    sample.right <= window.left
        || window.right <= sample.left
        || sample.bottom <= window.top
        || window.bottom <= sample.top
}

/// 背景合成也可能晚于窗口移动，邻域取样避开请求时和当前的窗口位置。
#[cfg(windows)]
fn neighbor_outside_window(
    (x, y, w, h): (i32, i32, u32, u32),
    current: RECT,
    monitor: RECT,
) -> Option<(i32, i32, u32, u32)> {
    let requested = RECT {
        left: x,
        top: y,
        right: x.checked_add(i32::try_from(w).ok()?)?,
        bottom: y.checked_add(i32::try_from(h).ok()?)?,
    };
    let window = bounding_rect(requested, current);
    sample_rect(
        (
            window.left,
            window.top,
            u32::try_from(window.right.checked_sub(window.left)?).ok()?,
            u32::try_from(window.bottom.checked_sub(window.top)?).ok()?,
        ),
        (
            monitor.left,
            monitor.top,
            monitor.right - monitor.left,
            monitor.bottom - monitor.top,
        ),
    )
}

/// 在显示器内取灵动岛附近的桌面区域，避免截入岛窗口和屏幕外的黑色像素。
/// 优先取同一水平带的右侧 / 左侧，再试下方 / 上方。
#[cfg(target_os = "windows")]
fn sample_rect(
    (wx, wy, ww, wh): (i32, i32, u32, u32),
    (ax, ay, aw, ah): (i32, i32, i32, i32),
) -> Option<(i32, i32, u32, u32)> {
    if aw <= 0 || ah <= 0 || ww == 0 || wh == 0 {
        return None;
    }
    let w = i32::try_from(ww).ok()?;
    let h = i32::try_from(wh).ok()?;
    let window_right = wx.saturating_add(w);
    let window_bottom = wy.saturating_add(h);
    let area_right = ax.saturating_add(aw);
    let area_bottom = ay.saturating_add(ah);

    // 画布比可见本体大，停靠时它自然伸出屏幕。侧带使用可见区间。
    let visible_x = wx.max(ax);
    let visible_y = wy.max(ay);
    let visible_w = window_right.min(area_right).saturating_sub(visible_x);
    let visible_h = window_bottom.min(area_bottom).saturating_sub(visible_y);

    // 至少留 64px 才值得拉伸；再窄就会把一列像素拉成竖条纹。
    const MIN_SIDE: i32 = 64;
    if visible_h > 0 {
        let right_x = window_right.max(ax);
        let left_x = wx.min(area_right);
        let right_w = area_right.saturating_sub(right_x).min(w);
        let left_w = left_x.saturating_sub(ax).min(w);
        if right_w >= MIN_SIDE.min(w) {
            return Some((right_x, visible_y, right_w as u32, visible_h as u32));
        }
        if left_w >= MIN_SIDE.min(w) {
            return Some((left_x - left_w, visible_y, left_w as u32, visible_h as u32));
        }
    }
    if visible_w > 0 {
        let below_y = window_bottom.max(ay);
        let above_y = wy.min(area_bottom);
        let below_h = area_bottom.saturating_sub(below_y).min(h);
        let above_h = above_y.saturating_sub(ay).min(h);
        if below_h >= MIN_SIDE.min(h) {
            return Some((visible_x, below_y, visible_w as u32, below_h as u32));
        }
        if above_h >= MIN_SIDE.min(h) {
            return Some((visible_x, above_y - above_h, visible_w as u32, above_h as u32));
        }
    }
    None
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{neighbor_outside_window, sample_outside_motion, sample_rect, RECT};

    #[test]
    fn centered_island_samples_the_right_band() {
        // 液态画布窗口 780×632：右侧只剩 456px，取右侧带（同尺寸拉伸）
        assert_eq!(
            sample_rect((684, 0, 780, 632), (0, 0, 1920, 1080)),
            Some((1464, 0, 456, 632))
        );
    }

    #[test]
    fn right_docked_island_samples_the_left_band() {
        assert_eq!(
            sample_rect((1368, 0, 780, 632), (0, 0, 1920, 1080)),
            Some((588, 0, 780, 632))
        );
    }

    #[test]
    fn narrow_screen_shrinks_the_sample_instead_of_overlapping() {
        let (sx, sy, sw, sh) = sample_rect((407, 0, 780, 632), (0, 0, 1366, 768)).unwrap();
        assert_eq!((sx, sy, sw, sh), (1187, 0, 179, 632));
        assert!(sx >= 407 + 780, "采样不能压在窗口上");
    }

    #[test]
    fn top_and_bottom_docked_bands_exclude_offscreen_rows() {
        assert_eq!(
            sample_rect((570, -70, 780, 632), (0, 0, 1920, 1080)),
            Some((1350, 0, 570, 562))
        );
        assert_eq!(
            sample_rect((570, 518, 780, 632), (0, 0, 1920, 1080)),
            Some((1350, 518, 570, 562))
        );
    }

    #[test]
    fn vertical_fallback_excludes_columns_in_other_monitors() {
        assert_eq!(
            sample_rect((-970, -100, 930, 600), (-900, -200, 900, 1000)),
            Some((-900, 500, 860, 300))
        );
    }

    #[test]
    fn no_usable_neighbor_skips_sampling_instead_of_capturing_the_island() {
        assert_eq!(sample_rect((0, 0, 780, 632), (0, 0, 800, 600)), None);
        assert_eq!(sample_rect((0, 0, 780, 632), (0, 0, 780, 632)), None);
    }

    #[test]
    fn moving_window_samples_outside_its_old_and_current_bounds() {
        let monitor = rect(0, 0, 1920, 1080);
        let current = rect(630, 20, 1410, 652);
        assert_eq!(
            neighbor_outside_window((570, 20, 780, 632), current, monitor),
            Some((1410, 20, 510, 632)),
        );
        assert_eq!(
            neighbor_outside_window((690, 20, 780, 632), current, monitor),
            Some((1470, 20, 450, 632)),
        );
    }

    #[test]
    fn moving_window_rejects_overlap_without_rejecting_parallel_motion() {
        let sample = rect(900, 50, 1200, 600);
        let before = rect(100, 50, 880, 682);
        assert!(sample_outside_motion(sample, before, rect(100, 95, 880, 727)));
        assert!(sample_outside_motion(sample, before, rect(80, 95, 860, 727)));
        assert!(!sample_outside_motion(sample, before, rect(130, 50, 910, 682)));
        assert!(!sample_outside_motion(sample, before, rect(1210, 50, 1990, 682)));
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }
}

/// 抓取液态玻璃底板。只有来方已经确认显示最新版本时才省略图片。
///
/// `hwnd` 用 `isize` 传递，避免原始 HWND 指针跨线程的 `Send` 限制。
#[cfg(target_os = "windows")]
pub fn capture(
    hwnd: isize,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    known_revision: Option<u32>,
    sample_neighbor: bool,
) -> Result<Option<BackdropFrame>, String> {
    use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder, RgbaImage};
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC,
        SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER,
        DIB_RGB_COLORS, HALFTONE, HGDIOBJ, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };

    let hwnd = HWND(hwnd as *mut c_void);
    let excluded = || {
        let mut affinity = 0;
        unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }.is_ok()
            && affinity == WDA_EXCLUDEFROMCAPTURE.0
    };
    let paused = || {
        let excluded = excluded();
        backdrop_cache()
            .map(|mut cache| cache.during_capture(excluded, sample_neighbor, known_revision))
    };
    let unavailable = || {
        if crate::island::capture_guard::active() {
            return paused();
        }
        backdrop_cache().map(|mut cache| cache.unavailable(known_revision))
    };

    // 本应用截图或录屏期间只复用与当前采样模式匹配的帧。
    if crate::island::capture_guard::active() {
        return paused();
    }
    if width < 4 || height < 4 {
        return unavailable();
    }

    if excluded() {
        let aligned = (|| {
            let monitor = monitor_for(hwnd)?;
            let canvas = RECT {
                left: x,
                top: y,
                right: x.checked_add(i32::try_from(width).ok()?)?,
                bottom: y.checked_add(i32::try_from(height).ok()?)?,
            };
            desktop::capture(monitor, canvas)
        })();
        if crate::island::capture_guard::active() {
            return paused();
        }
        if let Some(url) = aligned.filter(|_| excluded()) {
            return backdrop_cache().map(|mut cache| cache.aligned(url, known_revision));
        }
    }
    if !excluded() {
        desktop::reset();
    }
    if !sample_neighbor {
        return unavailable();
    }
    let Some(window) = window_rect(hwnd) else {
        return unavailable();
    };
    let Some((sx, sy, sw, sh)) = monitor_for(hwnd)
        .and_then(|monitor| neighbor_outside_window((x, y, width, height), window, monitor.bounds))
    else {
        return unavailable();
    };
    let tw = (sw / DOWNSCALE).clamp(2, 256) as i32;
    let th = (sh / DOWNSCALE).clamp(2, 256) as i32;
    let sample = RECT {
        left: sx,
        top: sy,
        right: sx.saturating_add(sw as i32),
        bottom: sy.saturating_add(sh as i32),
    };
    let capture_bounds = || {
        if crate::island::capture_guard::active() {
            return None;
        }
        window_rect(hwnd)
    };
    let result = (|| -> Result<Option<RgbaImage>, String> {
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err("取屏幕 DC 失败".into());
            }
            let mem = CreateCompatibleDC(Some(screen));
            if mem.is_invalid() {
                ReleaseDC(None, screen);
                return Err("创建内存 DC 失败".into());
            }

            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: tw,
                // 负高度 = 自上而下；屏幕抓取出来的行序和 CSS 背景一致。
                biHeight: -th,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            };

            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(mem);
                    ReleaseDC(None, screen);
                    return Err(format!("创建 DIB 失败: {e}"));
                }
            };

            let old = SelectObject(mem, HGDIOBJ(bitmap.0));
            let _ = SetStretchBltMode(mem, HALFTONE);
            let _ = SetBrushOrgEx(mem, 0, 0, None);
            let verified = capture_bounds()
                .filter(|before| sample_outside_motion(sample, window, *before))
                .and_then(|before| {
                    let copied = StretchBlt(
                        mem,
                        0,
                        0,
                        tw,
                        th,
                        Some(screen),
                        sx,
                        sy,
                        sw as i32,
                        sh as i32,
                        SRCCOPY,
                    );
                    // 等待本轮 GDI 写入，再检查实际窗口并读取位图内存。
                    let flushed = GdiFlush();
                    let after = capture_bounds()?;
                    sample_outside_motion(sample, before, after)
                        .then_some(copied.as_bool() && flushed.as_bool())
                });

            let image = if verified == Some(true) && !bits.is_null() {
                let len = (tw * th * 4) as usize;
                let raw = std::slice::from_raw_parts(bits as *const u8, len);
                let mut rgba = Vec::with_capacity(len);
                for px in raw.chunks_exact(4) {
                    // DIB 是 BGRA，alpha 通道在屏幕抓取里恒为 0，手动补满。
                    rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
                Some(RgbaImage::from_raw(tw as u32, th as u32, rgba).unwrap())
            } else {
                None
            };

            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);

            if verified.is_none() {
                return Ok(None);
            }
            image.map(Some).ok_or_else(|| "StretchBlt 抓取桌面失败".to_string())
        }
    })();

    if crate::island::capture_guard::active() {
        return paused();
    }
    let Ok(Some(image)) = result else {
        return unavailable();
    };
    // 比较相邻取样本身，未变化时复用已编码的图；客户端尚未收到就重发。
    // 亮度随帧在 Rust 侧算好（见 average_luminance），前端邻域采样只读值。
    let source = FrameSource::Neighbor {
        width: image.width(),
        height: image.height(),
        hash: thumbnail_hash(&image),
    };
    let luminance = average_luminance(&image);
    let mut cache = backdrop_cache()?;
    let result = cache.neighbor(source, luminance, known_revision, || {
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(|error| format!("编码桌面背景失败: {error}"))?;
        Ok(format!("data:image/png;base64,{}", STANDARD.encode(png)))
    });
    Ok(result.unwrap_or_else(|_| cache.unavailable(known_revision)))
}

#[cfg(not(target_os = "windows"))]
pub fn capture(
    _hwnd: isize,
    _x: i32,
    _y: i32,
    _width: u32,
    _height: u32,
    _known_revision: Option<u32>,
    _sample_neighbor: bool,
) -> Result<Option<BackdropFrame>, String> {
    Err("桌面毛玻璃采样仅支持 Windows".into())
}
