//! 桌面底板采样。开启「不出现在截图和录屏里」时，DXGI 提供与面板位置对齐的
//! 桌面纹理。允许录屏显示灵动岛时，使用同一显示器的相邻区域，避开窗口回采。
//! 采集过程不修改窗口采集标志，避免录制画面闪烁。

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
#[path = "backdrop/desktop.rs"]
mod desktop;

/// 相邻区域按比例下采样，保留边缘折射所需的细节。
const DOWNSCALE: u32 = 3;

/// 最近一次成功采到的底板（data URL）。本应用自己的截图/录屏进行中时直接
/// 复用它，避免与录屏和截图同时读取桌面。
static LAST_BACKDROP: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// 最近一次采样的缩略图内容哈希（内存压降专项：桌面画面没变时跳过 PNG
/// 编码与 base64，返回空串哨兵，前端据此跳过 CSS 变量更新与解码）。
static LAST_HASH: OnceLock<Mutex<Option<u64>>> = OnceLock::new();

fn last_slot() -> &'static Mutex<Option<String>> {
    LAST_BACKDROP.get_or_init(|| Mutex::new(None))
}

fn last_hash_slot() -> &'static Mutex<Option<u64>> {
    LAST_HASH.get_or_init(|| Mutex::new(None))
}

/// 清空上一拍内容哈希，强制下一拍 capture 必定重新编码并回传完整 data URL。
///
/// 用途：前端页面重载（renderer 崩溃自动恢复等）后 CSS `--backdrop` 从零
/// 开始，若桌面与采样区域都没变，哈希恒命中会让重载后的前端持续拿到空串
/// 哨兵、底图缺失。前端在每轮轮询的首拍带 force=true 调用 island_backdrop，
/// 命令层据此调用本函数；后续拍恢复哨兵去重，不损失省益。
pub fn invalidate_last_hash() {
    if let Ok(mut slot) = last_hash_slot().lock() {
        *slot = None;
    }
}

fn cached_backdrop() -> Option<String> {
    last_slot().lock().ok().and_then(|slot| slot.clone())
}

fn store_backdrop(url: &str) {
    if let Ok(mut slot) = last_slot().lock() {
        *slot = Some(url.to_string());
    }
}

/// 缩略图像素的 FNV-1a 64 位内容哈希（无依赖；只为相邻两拍的等值判断，
/// 不承担抗碰撞安全职责——壁纸类画面哈希碰撞时最坏表现为少更新一拍底板，
/// 下一拍内容变化自愈）。
fn thumbnail_hash(image: &image::RgbaImage) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in image.as_raw() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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

/// 抓取用于液态玻璃纹理的桌面底板，以图片 data URL 返回。
///
/// `hwnd` 用 `isize` 传递，避免原始 HWND 指针跨线程的 `Send` 限制。
#[cfg(target_os = "windows")]
pub fn capture(hwnd: isize, x: i32, y: i32, width: u32, height: u32) -> Result<String, String> {
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
        return cached_backdrop().ok_or_else(|| "正在录屏或截图，跳过桌面底板采样".to_string());
    }

    let hwnd = HWND(hwnd as *mut c_void);
    let Some(monitor) = monitor_for(hwnd) else {
        return cached_backdrop().ok_or_else(|| "没有可用的桌面底板采样区域".to_string());
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
            store_backdrop(&url);
            return Ok(url);
        }
    }
    if !excluded() {
        desktop::reset();
    }
    let r = monitor.bounds;
    let area = (r.left, r.top, r.right - r.left, r.bottom - r.top);
    let Some((sx, sy, sw, sh)) = sample_rect((x, y, width, height), area) else {
        return cached_backdrop().ok_or_else(|| "没有可用的桌面底板采样区域".to_string());
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
    // 内容去重（内存压降专项）：PNG 编码 + base64 + IPC 传输是每拍最大的
    // 开销，桌面静止时（长时间挂机的大多数时刻）缩略图逐字节相同，哈希
    // 相同即返回空串哨兵，前端跳过 CSS 更新。哈希在编码前算，省掉的就是
    // 整段编码。
    let hash = thumbnail_hash(&image);
    if last_hash_slot()
        .lock()
        .ok()
        .map(|slot| slot.as_ref() == Some(&hash))
        .unwrap_or(false)
    {
        return Ok(String::new());
    }
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
    if let Ok(mut slot) = last_hash_slot().lock() {
        *slot = Some(hash);
    }
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
