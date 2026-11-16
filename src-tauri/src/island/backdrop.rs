//! 桌面底板采样。开启「不出现在截图和录屏里」时，DXGI 提供与面板位置对齐的
//! 桌面纹理。允许录屏显示灵动岛时，使用同一显示器的相邻区域，避开窗口回采。
//! 采集过程不修改窗口采集标志，避免录制画面闪烁。

#[cfg(windows)]
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::sync::Arc;
#[cfg(windows)]
use std::sync::Mutex;

#[cfg(windows)]
#[path = "backdrop/desktop.rs"]
mod desktop;

/// 相邻区域按比例下采样，保留边缘折射所需的细节。
#[cfg(windows)]
const DOWNSCALE: u32 = 3;

#[derive(Clone, serde::Serialize)]
pub struct BackdropFrame {
    pub revision: u32,
    pub url: Arc<str>,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameSource {
    Aligned,
    Neighbor { width: u32, height: u32, hash: u64 },
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

    fn update(&mut self, source: FrameSource, url: String) {
        let revision = self
            .latest
            .as_ref()
            .map_or(1, |(_, frame)| frame.revision.wrapping_add(1));
        self.latest = Some((
            source,
            BackdropFrame {
                revision,
                url: Arc::from(url),
            },
        ));
    }

    fn aligned(&mut self, url: String, known_revision: Option<u32>) -> Option<BackdropFrame> {
        let unchanged = self.latest.as_ref().is_some_and(|(source, frame)| {
            *source == FrameSource::Aligned && frame.url.as_ref() == url
        });
        if !unchanged {
            self.update(FrameSource::Aligned, url);
        }
        self.reply(known_revision)
    }

    fn neighbor(
        &mut self,
        source: FrameSource,
        known_revision: Option<u32>,
        encode: impl FnOnce() -> Result<String, String>,
    ) -> Result<Option<BackdropFrame>, String> {
        if !self
            .latest
            .as_ref()
            .is_some_and(|(latest, _)| *latest == source)
        {
            self.update(source, encode()?);
        }
        Ok(self.reply(known_revision))
    }
}

#[cfg(windows)]
fn cached_backdrop(
    known_revision: Option<u32>,
    unavailable: &str,
) -> Result<Option<BackdropFrame>, String> {
    let cache = BACKDROP
        .lock()
        .map_err(|_| "桌面底板缓存不可用".to_string())?;
    if cache.latest.is_none() {
        return Err(unavailable.to_string());
    }
    Ok(cache.reply(known_revision))
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

    #[test]
    fn resends_unacknowledged_neighbor_without_encoding_again() {
        let mut cache = FrameCache::default();
        let source = neighbor(10, 16);
        let first = cache
            .neighbor(source, None, || Ok("neighbor".into()))
            .unwrap()
            .unwrap();
        let no_reencode = || panic!("cached pixels must not be re-encoded");
        let missed = cache
            .neighbor(source, None, no_reencode)
            .unwrap()
            .unwrap();
        assert_eq!(first.revision, missed.revision);
        assert!(Arc::ptr_eq(&first.url, &missed.url));
        assert!(cache
            .neighbor(source, Some(first.revision), no_reencode)
            .unwrap()
            .is_none());
        let resized = cache
            .neighbor(neighbor(16, 10), Some(first.revision), || {
                Ok("resized".into())
            })
            .unwrap()
            .unwrap();
        assert_ne!(resized.revision, first.revision);
    }

    #[test]
    fn returning_to_same_neighbor_pixels_after_aligned_frame_publishes_them() {
        let mut cache = FrameCache::default();
        let source = neighbor(10, 16);
        let initial = cache
            .neighbor(source, None, || Ok("neighbor".into()))
            .unwrap()
            .unwrap();
        let aligned = cache
            .aligned("aligned".into(), Some(initial.revision))
            .unwrap();
        assert!(cache
            .aligned("aligned".into(), Some(aligned.revision))
            .is_none());
        let neighbor_again = cache
            .neighbor(source, Some(aligned.revision), || Ok("neighbor".into()))
            .unwrap()
            .unwrap();
        assert_ne!(aligned.revision, neighbor_again.revision);
        assert_eq!(neighbor_again.url.as_ref(), "neighbor");
    }
}

/// 窗口所在显示器的物理边界和句柄。
#[cfg(target_os = "windows")]
fn monitor_for(hwnd: windows::Win32::Foundation::HWND) -> Option<desktop::Monitor> {
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
    use super::sample_rect;

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
) -> Result<Option<BackdropFrame>, String> {
    use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder, RgbaImage};
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER,
        DIB_RGB_COLORS, HALFTONE, HGDIOBJ, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };

    if width < 4 || height < 4 {
        return Err("窗口尺寸过小".into());
    }

    // 本应用自己的截图/录屏正在抓屏时复用上一张。
    if crate::island::capture_guard::active() {
        return cached_backdrop(known_revision, "正在录屏或截图，跳过桌面底板采样");
    }

    let hwnd = HWND(hwnd as *mut c_void);
    let Some(monitor) = monitor_for(hwnd) else {
        return cached_backdrop(known_revision, "没有可用的桌面底板采样区域");
    };
    let excluded = || {
        let mut affinity = 0;
        unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) }.is_ok()
            && affinity == WDA_EXCLUDEFROMCAPTURE.0
    };
    if excluded() {
        let w = i32::try_from(width).map_err(|_| "窗口宽度过大")?;
        let h = i32::try_from(height).map_err(|_| "窗口高度过大")?;
        let canvas = RECT {
            left: x,
            top: y,
            right: x.checked_add(w).ok_or("窗口横坐标无效")?,
            bottom: y.checked_add(h).ok_or("窗口纵坐标无效")?,
        };
        if let Some(url) = desktop::capture(monitor, canvas).filter(|_| excluded()) {
            return BACKDROP
                .lock()
                .map(|mut cache| cache.aligned(url, known_revision))
                .map_err(|_| "桌面底板缓存不可用".to_string());
        }
    }
    if !excluded() {
        desktop::reset();
    }
    let r = monitor.bounds;
    let area = (r.left, r.top, r.right - r.left, r.bottom - r.top);
    let Some((sx, sy, sw, sh)) = sample_rect((x, y, width, height), area) else {
        return cached_backdrop(known_revision, "没有可用的桌面底板采样区域");
    };
    let tw = (sw / DOWNSCALE).clamp(2, 320) as i32;
    let th = (sh / DOWNSCALE).clamp(2, 320) as i32;

    let result = (|| -> Result<RgbaImage, String> {
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
            let ok = StretchBlt(
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

            let image = if ok.as_bool() && !bits.is_null() {
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

            image.ok_or_else(|| "StretchBlt 抓取桌面失败".to_string())
        }
    })();

    let image = result?;
    // 比较相邻取样本身，未变化时复用已编码的图；客户端尚未收到就重发。
    let source = FrameSource::Neighbor {
        width: image.width(),
        height: image.height(),
        hash: thumbnail_hash(&image),
    };
    let mut cache = BACKDROP
        .lock()
        .map_err(|_| "桌面底板缓存不可用".to_string())?;
    cache.neighbor(source, known_revision, || {
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
    })
}

#[cfg(not(target_os = "windows"))]
pub fn capture(
    _hwnd: isize,
    _x: i32,
    _y: i32,
    _width: u32,
    _height: u32,
    _known_revision: Option<u32>,
) -> Result<Option<BackdropFrame>, String> {
    Err("桌面毛玻璃采样仅支持 Windows".into())
}
