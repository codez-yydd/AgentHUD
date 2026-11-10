//! 灵动岛身后的桌面毛玻璃采样。
//!
//! WebView2 的 `backdrop-filter` 只能模糊同一页面里的内容，看不到窗口后面的
//! 桌面；CSS 半透明玻璃于是只是"透"，不是"磨砂"。这里照着 wisland 的做法，
//! 用 GDI 把窗口所在屏幕区域缩成一张很小的图（缩略图本身就是面积平均），
//! 前端再把它铺在玻璃底下并做一次高斯模糊。
//!
//! 抓屏前临时给灵动岛窗口设置 `WDA_EXCLUDEFROMCAPTURE`，抓完立刻恢复，
//! 这样既不会把岛自己拍进去形成递归，也不会影响用户平时截图/录屏时看到岛。

use base64::{engine::general_purpose::STANDARD, Engine as _};

/// 物理像素 → 缩略图。6 倍下采样后再由 CSS 模糊，足够柔且 IPC 很小。
const DOWNSCALE: u32 = 6;

/// 抓取窗口矩形下的桌面，返回可直接塞进 CSS `url(...)` 的 data URL。
///
/// `hwnd` 用 `isize` 传递，避免原始 HWND 指针跨线程的 `Send` 限制。
#[cfg(target_os = "windows")]
pub fn capture(hwnd: isize, x: i32, y: i32, width: u32, height: u32) -> Result<String, String> {
    use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder, RgbaImage};
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::DwmFlush;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt, BITMAPINFO,
        BITMAPINFOHEADER, DIB_RGB_COLORS, HALFTONE, HGDIOBJ, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };

    if width < 4 || height < 4 {
        return Err("窗口尺寸过小".into());
    }

    let tw = (width / DOWNSCALE).clamp(2, 240) as i32;
    let th = (height / DOWNSCALE).clamp(2, 240) as i32;
    let hwnd = HWND(hwnd as *mut c_void);

    // 抓屏期间把岛排除掉；失败就放弃这一帧，避免把上一帧岛的画面叠进去。
    unsafe {
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)
            .map_err(|e| format!("无法排除灵动岛窗口: {e}"))?;
        // 等 DWM 真正按“排除”重组一帧再抓，否则刚开始的截图仍可能带着岛。
        let _ = DwmFlush();
    }

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
            let bitmap = match CreateDIBSection(
                Some(mem),
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            ) {
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
                x,
                y,
                width as i32,
                height as i32,
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

    unsafe {
        let _ = SetWindowDisplayAffinity(hwnd, WDA_NONE);
        // 让 DWM 立刻按“恢复可捕获”重组一帧。只改 affinity 而不 flush 时，
        // 截图/录屏会继续把岛当成排除状态几十到上百毫秒（实机逐帧验证）。
        let _ = DwmFlush();
    }

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
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

#[cfg(not(target_os = "windows"))]
pub fn capture(_hwnd: isize, _x: i32, _y: i32, _width: u32, _height: u32) -> Result<String, String> {
    Err("桌面毛玻璃采样仅支持 Windows".into())
}
