//! 灵动岛身后的桌面毛玻璃采样。
//!
//! WebView2 的 `backdrop-filter` 只能模糊同一页面里的内容，看不到窗口后面的
//! 桌面；CSS 半透明玻璃于是只是"透"，不是"磨砂"。这里照着 wisland 的做法，
//! 用 GDI 把窗口旁边一块屏幕区域缩成一张很小的图（缩略图本身就是面积平均），
//! 前端再把它铺在玻璃底下并做一次高斯模糊。
//!
//! 以前抓屏前会临时给岛窗口设置 `WDA_EXCLUDEFROMCAPTURE` 把岛从画面里剔掉。
//! 这个标志只对截屏 API 生效，却会让**所有**正在录屏/截图的人（OBS、飞书/
//! Lark 录屏、系统截图……）的画面里岛周期性整块消失 1~2 帧：每次 650ms 的
//! 底板采样都会闪一下。现在不再碰窗口的采集标志，改为采样窗口**旁边**一块
//! 同尺寸的桌面区域——玻璃本来就是 22px 高斯模糊 + 深色染色，邻域颜色和正
//! 后方在观感上分不出来，而任何录屏里岛都连续可见。

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::sync::{Mutex, OnceLock};

/// 物理像素 → 缩略图。6 倍下采样后再由 CSS 模糊，足够柔且 IPC 很小。
const DOWNSCALE: u32 = 6;

/// 最近一次成功采到的底板（data URL）。本应用自己的截图/录屏进行中时直接
/// 复用它，避免两边同时抢 GDI 桌面；毛玻璃底下本来就是模糊壁纸，冻结几秒
/// 没有观感差别。
static LAST_BACKDROP: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn last_slot() -> &'static Mutex<Option<String>> {
    LAST_BACKDROP.get_or_init(|| Mutex::new(None))
}

fn cached_backdrop() -> Option<String> {
    last_slot().lock().ok().and_then(|slot| slot.clone())
}

fn store_backdrop(url: &str) {
    if let Ok(mut slot) = last_slot().lock() {
        *slot = Some(url.to_string());
    }
}

/// 窗口所在显示器的矩形 `(x, y, w, h)`（物理 px）。
#[cfg(target_os = "windows")]
fn monitor_rect(hwnd: windows::Win32::Foundation::HWND) -> Option<(i32, i32, i32, i32)> {
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
    let rect = info.rcMonitor;
    Some((
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    ))
}

/// 选一块与窗口同尺寸、但不与窗口重叠的桌面区域来采样。
///
/// 优先取同一水平带的右侧 / 左侧（颜色和岛正后方最接近），放不下再取
/// 下方 / 上方，最后才退回窗口矩形。纯计算，方便单测。
#[cfg(target_os = "windows")]
fn sample_rect(
    (wx, wy, ww, wh): (i32, i32, u32, u32),
    (ax, ay, aw, ah): (i32, i32, i32, i32),
) -> (i32, i32, u32, u32) {
    let w = ww.max(1) as i32;
    let h = wh.max(1) as i32;
    // 至少留 64px 才值得拉伸；再窄就会把一列像素拉成竖条纹。
    const MIN_SIDE: i32 = 64;
    let right = ax + aw - (wx + w);
    let left = wx - ax;
    let below = ay + ah - (wy + h);
    let above = wy - ay;
    if right >= MIN_SIDE.min(w) {
        return (wx + w, wy, right.min(w) as u32, wh);
    }
    if left >= MIN_SIDE.min(w) {
        return (wx - left.min(w), wy, left.min(w) as u32, wh);
    }
    if below >= MIN_SIDE.min(h) {
        return (wx, wy + h, ww, below.min(h) as u32);
    }
    if above >= MIN_SIDE.min(h) {
        return (wx, wy - above.min(h), ww, above.min(h) as u32);
    }
    // 窗口几乎占满显示器：没有可用的邻域，退回窗口区域。
    (wx, wy, ww.min(aw.max(1) as u32), wh.min(ah.max(1) as u32))
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::sample_rect;

    #[test]
    fn centered_island_samples_the_right_band() {
        assert_eq!(
            sample_rect((684, 0, 552, 520), (0, 0, 1920, 1080)),
            (1236, 0, 552, 520)
        );
    }

    #[test]
    fn right_docked_island_samples_the_left_band() {
        assert_eq!(
            sample_rect((1368, 0, 552, 520), (0, 0, 1920, 1080)),
            (816, 0, 552, 520)
        );
    }

    #[test]
    fn narrow_screen_shrinks_the_sample_instead_of_overlapping() {
        let (sx, sy, sw, sh) = sample_rect((407, 0, 552, 520), (0, 0, 1366, 768));
        assert_eq!((sx, sy, sw, sh), (959, 0, 407, 520));
        assert!(sx >= 407 + 552, "采样不能压在窗口上");
    }

    #[test]
    fn small_screen_still_gets_a_side_band() {
        let (sx, sy, sw, sh) = sample_rect((0, 0, 552, 520), (0, 0, 800, 600));
        assert_eq!((sx, sy, sw, sh), (552, 0, 248, 520));
        assert!(sx + sw as i32 <= 800 && sy + sh as i32 <= 600);
    }

    #[test]
    fn full_monitor_falls_back_to_the_window_rect() {
        assert_eq!(
            sample_rect((0, 0, 552, 520), (0, 0, 552, 520)),
            (0, 0, 552, 520)
        );
    }
}

/// 抓取窗口矩形下的桌面，返回可直接塞进 CSS `url(...)` 的 data URL。
///
/// `hwnd` 用 `isize` 传递，避免原始 HWND 指针跨线程的 `Send` 限制。
#[cfg(target_os = "windows")]
pub fn capture(hwnd: isize, x: i32, y: i32, width: u32, height: u32) -> Result<String, String> {
    use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder, RgbaImage};
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER,
        DIB_RGB_COLORS, HALFTONE, HGDIOBJ, SRCCOPY,
    };

    if width < 4 || height < 4 {
        return Err("窗口尺寸过小".into());
    }

    // 本应用自己的截图/录屏正在抓屏时复用上一张，避免两边同时抢 GDI 桌面。
    if crate::island::capture_guard::active() {
        return cached_backdrop().ok_or_else(|| "正在录屏或截图，跳过桌面底板采样".to_string());
    }

    let hwnd = HWND(hwnd as *mut c_void);
    let (sx, sy, sw, sh) = sample_rect(
        (x, y, width, height),
        monitor_rect(hwnd).unwrap_or((x, y, width as i32, height as i32)),
    );
    let tw = (sw / DOWNSCALE).clamp(2, 240) as i32;
    let th = (sh / DOWNSCALE).clamp(2, 240) as i32;

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
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("编码桌面背景失败: {e}"))?;
    let url = format!("data:image/png;base64,{}", STANDARD.encode(png));
    store_backdrop(&url);
    Ok(url)
}

#[cfg(not(target_os = "windows"))]
pub fn capture(
    _hwnd: isize,
    _x: i32,
    _y: i32,
    _width: u32,
    _height: u32,
) -> Result<String, String> {
    cached_backdrop().ok_or_else(|| "桌面毛玻璃采样仅支持 Windows".to_string())
}
