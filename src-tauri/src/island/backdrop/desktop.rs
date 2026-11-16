//! Capture a canvas-aligned desktop texture when the island is already excluded.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType, ImageEncoder, RgbImage};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION, DXGI_MODE_ROTATION_ROTATE180,
    DXGI_MODE_ROTATION_ROTATE270, DXGI_MODE_ROTATION_ROTATE90, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutput5,
    IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

const RETRY_DELAY: Duration = Duration::from_secs(2);
const IDLE_RELEASE: Duration = Duration::from_secs(3);
const MAX_IMAGE_SIDE: u32 = 640;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Monitor {
    pub id: usize,
    pub bounds: RECT,
}

struct Texture {
    handle: ID3D11Texture2D,
    size: (u32, u32),
}

struct Desktop {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    output: IDXGIOutputDuplication,
    rotation: DXGI_MODE_ROTATION,
    mirror: Option<Texture>,
    staging: Option<Texture>,
    encoded: Option<(RECT, String)>,
}

#[derive(Default)]
struct State {
    monitor: Option<Monitor>,
    desktop: Option<Desktop>,
    retry_after: Option<Instant>,
    last_request: Option<Instant>,
    reaper_running: bool,
}

static CAPTURE: Mutex<State> = Mutex::new(State {
    monitor: None,
    desktop: None,
    retry_after: None,
    last_request: None,
    reaper_running: false,
});
static CAPTURE_IDLE: Condvar = Condvar::new();

pub(super) fn reset() {
    if let Ok(mut state) = CAPTURE.lock() {
        if state.monitor.is_some() {
            state.monitor = None;
            state.desktop = None;
            state.retry_after = None;
            state.last_request = None;
            CAPTURE_IDLE.notify_one();
        }
    }
}

pub(super) fn capture(monitor: Monitor, canvas: RECT) -> Option<String> {
    let mut state = CAPTURE.lock().ok()?;
    state.last_request = Some(Instant::now());
    if !state.reaper_running {
        state.reaper_running = std::thread::Builder::new()
            .name("zbar-island-glass-idle".into())
            .spawn(release_when_idle)
            .is_ok();
    }
    if state.monitor != Some(monitor) {
        state.monitor = Some(monitor);
        state.desktop = None;
        state.retry_after = None;
    }
    if state
        .retry_after
        .is_some_and(|until| Instant::now() < until)
    {
        return None;
    }
    if state.desktop.is_none() {
        state.desktop = Desktop::new(monitor.id);
    }
    if let Some(desktop) = &mut state.desktop {
        if let Ok(frame) = desktop.capture(canvas, monitor.bounds) {
            return frame;
        }
    }
    state.desktop = None;
    state.retry_after = Some(Instant::now() + RETRY_DELAY);
    None
}

fn release_when_idle() {
    let Ok(mut state) = CAPTURE.lock() else {
        return;
    };
    while let Some(remaining) = state
        .last_request
        .and_then(|last| IDLE_RELEASE.checked_sub(last.elapsed()))
        .filter(|remaining| !remaining.is_zero())
    {
        let Ok((updated, _)) = CAPTURE_IDLE.wait_timeout(state, remaining) else {
            return;
        };
        state = updated;
    }
    *state = State::default();
}

impl Desktop {
    fn new(id: usize) -> Option<Self> {
        unsafe {
            let (adapter, output) = find_output(id)?;
            let rotation = output.GetDesc().ok()?.Rotation;
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .ok()?;
            let device = device?;
            let ctx = ctx?;
            let output = output
                .cast::<IDXGIOutput5>()
                .and_then(|out| out.DuplicateOutput1(&device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]))
                .or_else(|_| output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device))
                .ok()?;
            Some(Self {
                device,
                ctx,
                output,
                rotation,
                mirror: None,
                staging: None,
                encoded: None,
            })
        }
    }

    fn capture(&mut self, canvas: RECT, bounds: RECT) -> Result<Option<String>, ()> {
        let Some(crop) = Crop::new(canvas, bounds, self.rotation) else {
            return Ok(None);
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        unsafe {
            match self.output.AcquireNextFrame(8, &mut info, &mut resource) {
                Ok(()) => {
                    let updated = self.mirror.is_none() || info.LastPresentTime != 0;
                    let copied = if updated {
                        self.snapshot(resource)
                    } else {
                        Ok(())
                    };
                    self.output.ReleaseFrame().map_err(|_| ())?;
                    copied?;
                }
                Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
                Err(_) => return Err(()),
            }
        }
        if let Some((_, url)) = self.encoded.as_ref().filter(|(rect, _)| *rect == canvas) {
            return Ok(Some(url.clone()));
        }
        let Some(mirror) = &self.mirror else {
            return Ok(None);
        };
        let (raw_w, raw_h) = mirror.size;
        let sub = crop.source;
        // Display mode changes invalidate geometry before some drivers report ACCESS_LOST.
        if sub.right > raw_w || sub.bottom > raw_h {
            return Err(());
        }
        let size = (sub.right - sub.left, sub.bottom - sub.top);
        let staging = ensure_texture(&self.device, &mut self.staging, size, true)?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let image = unsafe {
            self.ctx
                .CopySubresourceRegion(staging, 0, 0, 0, 0, &mirror.handle, 0, Some(&sub));
            self.ctx
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|_| ())?;
            let pitch = mapped.RowPitch as usize;
            let img = if mapped.pData.is_null() || pitch < size.0 as usize * 4 {
                None
            } else {
                let bytes = std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>(),
                    pitch * (size.1 - 1) as usize + size.0 as usize * 4,
                );
                Some(crop.downsample(bytes, pitch))
            };
            self.ctx.Unmap(staging, 0);
            img.ok_or(())?
        };
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 82)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgb8,
            )
            .map_err(|_| ())?;
        let url = format!("data:image/jpeg;base64,{}", STANDARD.encode(&bytes));
        self.encoded = Some((canvas, url.clone()));
        Ok(Some(url))
    }

    fn snapshot(&mut self, resource: Option<IDXGIResource>) -> Result<(), ()> {
        let frame = resource
            .ok_or(())?
            .cast::<ID3D11Texture2D>()
            .map_err(|_| ())?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { frame.GetDesc(&mut desc) };
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(());
        }
        let mirror = ensure_texture(
            &self.device,
            &mut self.mirror,
            (desc.Width, desc.Height),
            false,
        )?;
        unsafe { self.ctx.CopyResource(mirror, &frame) };
        self.encoded = None;
        Ok(())
    }
}

unsafe fn find_output(id: usize) -> Option<(IDXGIAdapter, IDXGIOutput)> {
    let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
    for adapter_index in 0.. {
        let adapter = factory.EnumAdapters1(adapter_index).ok()?;
        for output_index in 0.. {
            let Ok(output) = adapter.EnumOutputs(output_index) else {
                break;
            };
            if output
                .GetDesc()
                .is_ok_and(|desc| desc.AttachedToDesktop.as_bool() && desc.Monitor.0 as usize == id)
            {
                return Some((adapter.cast().ok()?, output));
            }
        }
    }
    None
}

fn ensure_texture<'a>(
    device: &ID3D11Device,
    slot: &'a mut Option<Texture>,
    size: (u32, u32),
    staging: bool,
) -> Result<&'a ID3D11Texture2D, ()> {
    if slot.as_ref().is_none_or(|texture| texture.size != size) {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.0,
            Height: size.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: if staging {
                D3D11_USAGE_STAGING
            } else {
                D3D11_USAGE_DEFAULT
            },
            CPUAccessFlags: if staging {
                D3D11_CPU_ACCESS_READ.0 as u32
            } else {
                0
            },
            ..Default::default()
        };
        let mut handle = None;
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut handle))
                .map_err(|_| ())?
        };
        *slot = Some(Texture {
            handle: handle.ok_or(())?,
            size,
        });
    }
    Ok(&slot.as_ref().ok_or(())?.handle)
}

struct Crop {
    canvas: RECT,
    desktop: RECT,
    source: D3D11_BOX,
    rotation: DXGI_MODE_ROTATION,
}

impl Crop {
    fn new(canvas: RECT, desktop: RECT, rotation: DXGI_MODE_ROTATION) -> Option<Self> {
        let l = canvas.left.max(desktop.left);
        let r = canvas.right.min(desktop.right);
        let t = canvas.top.max(desktop.top);
        let b = canvas.bottom.min(desktop.bottom);
        if r <= l || b <= t {
            return None;
        }
        let (x0, y0) = source_point((l, t), desktop, rotation);
        let (x1, y1) = source_point((r - 1, b - 1), desktop, rotation);
        Some(Self {
            canvas,
            desktop,
            source: D3D11_BOX {
                left: x0.min(x1),
                top: y0.min(y1),
                right: x0.max(x1) + 1,
                bottom: y0.max(y1) + 1,
                front: 0,
                back: 1,
            },
            rotation,
        })
    }

    fn downsample(&self, pixels: &[u8], pitch: usize) -> RgbImage {
        let width = (self.canvas.right - self.canvas.left) as u32;
        let height = (self.canvas.bottom - self.canvas.top) as u32;
        let out_w = width.div_ceil(2).min(MAX_IMAGE_SIDE);
        let out_h = height.div_ceil(2).min(MAX_IMAGE_SIDE);
        // A pair of samples per axis covers the original pixels at half scale.
        let columns: Vec<_> = (0..out_w)
            .map(|x| sample_axis(self.canvas.left, width, out_w, x))
            .collect();
        RgbImage::from_fn(out_w, out_h, |x, y| {
            let ys = sample_axis(self.canvas.top, height, out_h, y);
            let mut sum = [0u16; 3];
            for sy in ys {
                for sx in columns[x as usize] {
                    // Extend last valid pixels into the off-screen part of the canvas
                    // so its original screen alignment survives edge docking.
                    let sx = sx.clamp(self.desktop.left, self.desktop.right - 1);
                    let sy = sy.clamp(self.desktop.top, self.desktop.bottom - 1);
                    let (raw_x, raw_y) = source_point((sx, sy), self.desktop, self.rotation);
                    let offset = (raw_y - self.source.top) as usize * pitch
                        + (raw_x - self.source.left) as usize * 4;
                    sum[0] += u16::from(pixels[offset + 2]);
                    sum[1] += u16::from(pixels[offset + 1]);
                    sum[2] += u16::from(pixels[offset]);
                }
            }
            image::Rgb(sum.map(|channel| (channel / 4) as u8))
        })
    }
}

fn sample_axis(start: i32, source: u32, output: u32, index: u32) -> [i32; 2] {
    [1, 3].map(|quarter| {
        let offset = (u64::from(index) * 4 + quarter) * u64::from(source) / (u64::from(output) * 4);
        start + offset as i32
    })
}

fn source_point((x, y): (i32, i32), bounds: RECT, rotation: DXGI_MODE_ROTATION) -> (u32, u32) {
    let local_x = (x - bounds.left) as u32;
    let local_y = (y - bounds.top) as u32;
    let width = (bounds.right - bounds.left) as u32;
    let height = (bounds.bottom - bounds.top) as u32;
    match rotation {
        DXGI_MODE_ROTATION_ROTATE90 => (local_y, width - 1 - local_x),
        DXGI_MODE_ROTATION_ROTATE180 => (width - 1 - local_x, height - 1 - local_y),
        DXGI_MODE_ROTATION_ROTATE270 => (height - 1 - local_y, local_x),
        _ => (local_x, local_y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_MODE_ROTATION_IDENTITY;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn offscreen_canvas_extends_the_monitor_edge_without_shifting_pixels() {
        let crop = Crop::new(
            rect(-8, 0, 0, 2),
            rect(-6, 0, 0, 2),
            DXGI_MODE_ROTATION_IDENTITY,
        )
        .unwrap();
        let pixels: Vec<_> = [5, 7, 20, 40, 90, 110]
            .into_iter()
            .cycle()
            .take(12)
            .flat_map(|red| [0, 0, red, 0])
            .collect();
        let result = crop.downsample(&pixels, 24);
        assert_eq!(result.dimensions(), (4, 1));
        assert_eq!(
            result.pixels().map(|p| p[0]).collect::<Vec<_>>(),
            [5, 6, 30, 100]
        );
    }

    #[test]
    fn portrait_output_rotates_capture_to_desktop_coordinates() {
        let bounds = rect(0, 0, 2, 4);
        let crop = Crop::new(bounds, bounds, DXGI_MODE_ROTATION_ROTATE90).unwrap();
        let pixels: Vec<_> = [0, 0, 100, 100, 20, 20, 120, 120]
            .into_iter()
            .flat_map(|red| [0, 0, red, 0])
            .collect();
        let result = crop.downsample(&pixels, 16);
        assert_eq!(result.dimensions(), (1, 2));
        assert_eq!(result.pixels().map(|p| p[0]).collect::<Vec<_>>(), [10, 110]);
    }
}
