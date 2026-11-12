//! 工具箱：二维码生成与图片压缩 / 格式转换。
//!
//! 纯本机处理，不联网、不读价格表与用户配置：给什么就处理什么。
//!
//! - 二维码：`qrcode` crate 只负责算模块矩阵，本模块自绘 RGBA 再用 `image`
//!   编 PNG。刻意不开该 crate 的 `image` feature——那会再引一次 image 0.25
//!   （本仓已有同版本），自绘二十行反而更可控，也顺手支持自定义静区与
//!   每模块像素。
//! - 图片：解码 → 可选等比缩小 → 按目标格式重编码后写盘。
//!   JPEG 没有透明通道，RGBA 一律先合成到白底再编码，避免透明区存成黑色；
//!   PNG 保留透明。本仓 image 0.25 的 WebP 编码器只有**无损**实现，故 WebP
//!   选项在界面上明确标注为无损（不是有损压缩）。
//! - 预览：后端缩到 720px 再编 JPEG data URL 返回，前端不依赖 asset 协议
//!   （也就不必给主窗口额外开文件读取权限）。

use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, RgbImage, Rgba, RgbaImage};
use qrcode::types::QrError;
use qrcode::{Color as QrColor, EcLevel, QrCode};
use serde::Serialize;
use std::path::Path;

/// 每模块像素（默认 8）：太小打印糊，太大大幅增加体积。
const QR_SCALE_DEFAULT: u32 = 8;
const QR_SCALE_MIN: u32 = 2;
const QR_SCALE_MAX: u32 = 32;

/// 静区（quiet zone）模块数：规范建议 4；0 = 不留白（贴进白底卡片时用）。
const QR_QUIET_DEFAULT: u32 = 4;
const QR_QUIET_MAX: u32 = 8;

/// 生成图最长边硬上限：超过就自动下调每模块像素，避免一张二维码算出
/// 几十 MB 的 PNG（版本 40 + 32px/模块 ≈ 5900px 边长）。
const QR_SIDE_LIMIT: u32 = 2600;

/// 预览图最长边与 JPEG 质量（只用于界面回显，不落盘）。
const PREVIEW_MAX_EDGE: u32 = 720;
const PREVIEW_JPEG_QUALITY: u8 = 82;

/// JPEG 质量默认值与允许范围。
const JPEG_QUALITY_DEFAULT: u32 = 82;
const JPEG_QUALITY_MIN: u32 = 1;
const JPEG_QUALITY_MAX: u32 = 100;

/// 「最长边」允许上限；0 = 不缩放。上限只为挡住误传的超大值。
const MAX_EDGE_LIMIT: u32 = 8192;

/// 二维码生成结果（前端直接 <img src=data_url> 显示）。
#[derive(Debug, Clone, Serialize)]
pub struct QrPng {
    /// `data:image/png;base64,...`
    pub data_url: String,
    /// 逻辑模块数（不含静区），前端用来提示「版本 / 容量」
    pub modules: u32,
    /// 实际生成边长（像素，含静区）
    pub size: u32,
    /// PNG 字节数
    pub bytes: u64,
}

/// 二维码落盘结果。
#[derive(Debug, Clone, Serialize)]
pub struct QrSaved {
    pub path: String,
    pub bytes: u64,
    pub size: u32,
}

/// 选中图片的元信息 + 预览。
#[derive(Debug, Clone, Serialize)]
pub struct ImageInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// 源文件字节数
    pub bytes: u64,
    /// 按扩展名粗分：png / jpeg / webp / gif / bmp / 其他扩展名 / 未知
    pub format: String,
    /// 720px 以内的 JPEG 预览 data URL
    pub preview: String,
}

/// 转换落盘结果（界面据此算压缩率）。
#[derive(Debug, Clone, Serialize)]
pub struct ImageOut {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub format: String,
}

/// 目标格式（只支持这三种：都是 `image` 已启用编码器的格式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Png,
    Jpeg,
    Webp,
}

impl Target {
    fn name(self) -> &'static str {
        match self {
            Target::Png => "png",
            Target::Jpeg => "jpeg",
            Target::Webp => "webp",
        }
    }
}

fn parse_target(raw: &str) -> Result<Target, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "png" => Ok(Target::Png),
        "jpg" | "jpeg" => Ok(Target::Jpeg),
        "webp" => Ok(Target::Webp),
        other => Err(format!(
            "不支持的输出格式：{other}（只支持 PNG / JPEG / WebP）"
        )),
    }
}

fn parse_ec(raw: Option<&str>) -> Result<EcLevel, String> {
    match raw.unwrap_or("M").trim().to_ascii_uppercase().as_str() {
        "L" => Ok(EcLevel::L),
        "M" => Ok(EcLevel::M),
        "Q" => Ok(EcLevel::Q),
        "H" => Ok(EcLevel::H),
        other => Err(format!("容错级别只能是 L / M / Q / H，收到 {other}")),
    }
}

/// 算二维码矩阵。空内容直接报错（比生成一张空图更好排查）。
fn qr_code(text: &str, ec: Option<&str>) -> Result<QrCode, String> {
    if text.is_empty() {
        return Err("请输入要生成二维码的内容".into());
    }
    let level = parse_ec(ec)?;
    QrCode::with_error_correction_level(text.as_bytes(), level).map_err(|e| match e {
        QrError::DataTooLong => {
            "内容太长，超出二维码容量上限（字节模式下 L 档约 2900 字节）".to_string()
        }
        other => format!("生成二维码失败：{other}"),
    })
}

/// 每模块像素：夹到 2..=32，再按边长硬上限下调（保证至少 1px）。
fn fit_scale(modules: u32, scale: Option<u32>, quiet: u32) -> u32 {
    let want = scale.unwrap_or(QR_SCALE_DEFAULT).clamp(QR_SCALE_MIN, QR_SCALE_MAX);
    let total = (modules + quiet * 2).max(1);
    want.min((QR_SIDE_LIMIT / total).max(1))
}

/// 模块矩阵 → RGBA 位图。深色用近黑、浅色纯白：二维码识别依赖高对比，
/// 不跟随界面主题色。
fn render_matrix(code: &QrCode, scale: u32, quiet: u32) -> RgbaImage {
    let modules = code.width() as u32;
    let side = (modules + quiet * 2) * scale;
    let mut img = RgbaImage::from_pixel(side, side, Rgba([255, 255, 255, 255]));
    let dark = Rgba([17, 20, 28, 255]);
    for y in 0..modules {
        for x in 0..modules {
            if code[(x as usize, y as usize)] != QrColor::Dark {
                continue;
            }
            let ox = (quiet + x) * scale;
            let oy = (quiet + y) * scale;
            for dy in 0..scale {
                for dx in 0..scale {
                    img.put_pixel(ox + dx, oy + dy, dark);
                }
            }
        }
    }
    img
}

fn encode_rgba_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    // 二维码大片纯色，Default + Adaptive 已经压得很小；Best 在版本 40 上
    // 要多花几百毫秒，不值。
    PngEncoder::new_with_quality(
        &mut png,
        CompressionType::Default,
        PngFilterType::Adaptive,
    )
    .write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        ExtendedColorType::Rgba8,
    )
    .map_err(|e| format!("编码 PNG 失败：{e}"))?;
    Ok(png)
}

/// 生成二维码 PNG：返回 (PNG 字节, 模块数, 像素边长)。
fn qr_png_parts(
    text: &str,
    ec: Option<&str>,
    scale: Option<u32>,
    quiet: Option<u32>,
) -> Result<(Vec<u8>, u32, u32), String> {
    let code = qr_code(text, ec)?;
    let modules = code.width() as u32;
    let quiet = quiet.unwrap_or(QR_QUIET_DEFAULT).min(QR_QUIET_MAX);
    let scale = fit_scale(modules, scale, quiet);
    let img = render_matrix(&code, scale, quiet);
    let png = encode_rgba_png(&img)?;
    Ok((png, modules, img.width()))
}

fn build_qr(
    text: &str,
    ec: Option<&str>,
    scale: Option<u32>,
    quiet: Option<u32>,
) -> Result<QrPng, String> {
    let (png, modules, size) = qr_png_parts(text, ec, scale, quiet)?;
    Ok(QrPng {
        data_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        ),
        modules,
        size,
        bytes: png.len() as u64,
    })
}

/// RGBA 合成到白底再转 RGB（JPEG 用）。无 alpha 的图 alpha 恒 255，
/// 这一步等价于直接 to_rgb8。
fn flatten_on_white(img: &image::DynamicImage) -> RgbImage {
    let rgba = img.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, px) in rgba.enumerate_pixels() {
        let a = px[3] as u32;
        let mix = |c: u8| -> u8 { ((c as u32 * a + 255 * (255 - a)) / 255) as u8 };
        out.put_pixel(x, y, image::Rgb([mix(px[0]), mix(px[1]), mix(px[2])]));
    }
    out
}

fn preview_data_url(img: &image::DynamicImage) -> Result<String, String> {
    let small = if img.width() > PREVIEW_MAX_EDGE || img.height() > PREVIEW_MAX_EDGE {
        img.resize(PREVIEW_MAX_EDGE, PREVIEW_MAX_EDGE, FilterType::Triangle)
    } else {
        img.clone()
    };
    let rgb = flatten_on_white(&small);
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, PREVIEW_JPEG_QUALITY)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("生成预览失败：{e}"))?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&jpeg)
    ))
}

/// 源图格式：按扩展名识别（这里只做展示，解码本身由 image crate 自动识别）。
fn source_format(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "png".into(),
        Some("jpg") | Some("jpeg") => "jpeg".into(),
        Some("webp") => "webp".into(),
        Some("gif") => "gif".into(),
        Some("bmp") => "bmp".into(),
        Some(other) => other.to_string(),
        None => "未知".into(),
    }
}

fn inspect_image(path: &str) -> Result<ImageInfo, String> {
    let file = Path::new(path);
    let meta = std::fs::metadata(file).map_err(|e| format!("读取文件失败：{e}"))?;
    let img = image::open(file).map_err(|e| format!("无法识别的图片格式：{e}"))?;
    Ok(ImageInfo {
        path: path.to_string(),
        width: img.width(),
        height: img.height(),
        bytes: meta.len(),
        format: source_format(file),
        preview: preview_data_url(&img)?,
    })
}

/// 等比缩小到最长边不超过 max_edge（0 / 已足够小则原样返回）。
fn shrink_to(img: image::DynamicImage, max_edge: u32) -> image::DynamicImage {
    if max_edge == 0 || (img.width() <= max_edge && img.height() <= max_edge) {
        return img;
    }
    img.resize(max_edge, max_edge, FilterType::Lanczos3)
}

fn encode_image(
    img: &image::DynamicImage,
    target: Target,
    quality: Option<u32>,
) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    match target {
        Target::Png => {
            let rgba = img.to_rgba8();
            PngEncoder::new_with_quality(
                &mut buf,
                CompressionType::Default,
                PngFilterType::Adaptive,
            )
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(|e| format!("编码 PNG 失败：{e}"))?;
        }
        Target::Jpeg => {
            let q = quality
                .unwrap_or(JPEG_QUALITY_DEFAULT)
                .clamp(JPEG_QUALITY_MIN, JPEG_QUALITY_MAX) as u8;
            let rgb = flatten_on_white(img);
            JpegEncoder::new_with_quality(&mut buf, q)
                .write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
                .map_err(|e| format!("编码 JPEG 失败：{e}"))?;
        }
        Target::Webp => {
            let rgba = img.to_rgba8();
            WebPEncoder::new_lossless(&mut buf)
                .write_image(
                    rgba.as_raw(),
                    rgba.width(),
                    rgba.height(),
                    ExtendedColorType::Rgba8,
                )
                .map_err(|e| format!("编码 WebP 失败：{e}"))?;
        }
    }
    Ok(buf)
}

fn convert_to_file(
    input: &str,
    format: &str,
    quality: Option<u32>,
    max_edge: Option<u32>,
    out: &str,
) -> Result<ImageOut, String> {
    let target = parse_target(format)?;
    let img = image::open(Path::new(input)).map_err(|e| format!("无法识别的图片格式：{e}"))?;
    let img = shrink_to(img, max_edge.unwrap_or(0).min(MAX_EDGE_LIMIT));
    let buf = encode_image(&img, target, quality)?;
    std::fs::write(out, &buf).map_err(|e| format!("写入文件失败：{e}"))?;
    Ok(ImageOut {
        path: out.to_string(),
        width: img.width(),
        height: img.height(),
        bytes: buf.len() as u64,
        format: target.name().to_string(),
    })
}

// ===== 命令 =====

/// 生成二维码并返回 PNG data URL（界面预览用，不落盘）。
#[tauri::command]
pub async fn toolbox_qr_png(
    text: String,
    ec_level: Option<String>,
    scale: Option<u32>,
    quiet: Option<u32>,
) -> Result<QrPng, String> {
    tauri::async_runtime::spawn_blocking(move || {
        build_qr(&text, ec_level.as_deref(), scale, quiet)
    })
    .await
    .map_err(|e| format!("生成二维码任务失败：{e}"))?
}

/// 生成二维码并写盘（路径由前端文件对话框给出）。
#[tauri::command]
pub async fn toolbox_qr_save(
    text: String,
    ec_level: Option<String>,
    scale: Option<u32>,
    quiet: Option<u32>,
    out_path: String,
) -> Result<QrSaved, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (png, _, size) = qr_png_parts(&text, ec_level.as_deref(), scale, quiet)?;
        std::fs::write(&out_path, &png).map_err(|e| format!("写入文件失败：{e}"))?;
        Ok(QrSaved {
            path: out_path,
            bytes: png.len() as u64,
            size,
        })
    })
    .await
    .map_err(|e| format!("保存二维码任务失败：{e}"))?
}

/// 读取图片元信息 + 720px 预览（选中文件后立即调用）。
#[tauri::command]
pub async fn toolbox_image_inspect(path: String) -> Result<ImageInfo, String> {
    tauri::async_runtime::spawn_blocking(move || inspect_image(&path))
        .await
        .map_err(|e| format!("读取图片任务失败：{e}"))?
}

/// 按目标格式 / 质量 / 最长边重编码并写盘。
#[tauri::command]
pub async fn toolbox_image_save(
    input_path: String,
    format: String,
    quality: Option<u32>,
    max_edge: Option<u32>,
    out_path: String,
) -> Result<ImageOut, String> {
    tauri::async_runtime::spawn_blocking(move || {
        convert_to_file(&input_path, &format, quality, max_edge, &out_path)
    })
    .await
    .map_err(|e| format!("转换图片任务失败：{e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时目录：每个用例独立子目录，结束即删。
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "agenthud-toolbox-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).expect("建临时目录");
            TempDir(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn qr_png_parts_matches_version_and_scale() {
        // 19 字节的短链在 M 档落版本 2（25 模块）；边长 = (25 + 4*2) * 6
        let (png, modules, size) =
            qr_png_parts("https://example.com", Some("M"), Some(6), Some(4)).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "应是 PNG 签名");
        assert_eq!(modules, 25);
        assert_eq!(size, 198);
    }

    #[test]
    fn qr_rejects_empty_and_oversized_text() {
        assert!(qr_png_parts("", None, None, None).is_err());
        // H 档上限约 1273 字节，4000 字节必爆
        assert!(qr_png_parts(&"x".repeat(4000), Some("H"), None, None).is_err());
        // 容错级别写错要报错，不能静默回退
        assert!(qr_png_parts("abc", Some("Z"), None, None).is_err());
    }

    #[test]
    fn qr_scale_capped_by_side_limit() {
        // 长文本 + 最大每模块像素：边长仍不超过硬上限
        let text = "y".repeat(1200);
        let (_, modules, size) = qr_png_parts(&text, Some("L"), Some(32), Some(4)).unwrap();
        assert!(modules > 21, "长文本应当是更大的版本");
        assert!(
            size <= QR_SIDE_LIMIT,
            "边长 {size} 不应超过上限 {QR_SIDE_LIMIT}"
        );
    }

    #[test]
    fn image_convert_shrinks_to_max_edge() {
        let dir = TempDir::new();
        let src = dir.path().join("src.png");
        RgbaImage::from_pixel(80, 40, Rgba([200, 30, 30, 255]))
            .save(&src)
            .expect("写源图");
        let out = dir.path().join("out.jpg");
        let info = convert_to_file(
            src.to_str().unwrap(),
            "jpeg",
            Some(60),
            Some(20),
            out.to_str().unwrap(),
        )
        .unwrap();
        assert_eq!((info.width, info.height), (20, 10), "等比缩到最长边 20");
        assert_eq!(info.format, "jpeg");
        assert!(info.bytes > 0);
        assert_eq!(std::fs::metadata(&out).unwrap().len(), info.bytes);
        let decoded = image::open(&out).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (20, 10));
    }

    #[test]
    fn image_inspect_returns_preview_data_url() {
        let dir = TempDir::new();
        let src = dir.path().join("shot.png");
        RgbaImage::from_pixel(30, 20, Rgba([10, 120, 200, 128]))
            .save(&src)
            .expect("写源图");
        let info = inspect_image(src.to_str().unwrap()).unwrap();
        assert_eq!((info.width, info.height), (30, 20));
        assert_eq!(info.format, "png");
        assert!(info.preview.starts_with("data:image/jpeg;base64,"));
        assert!(info.bytes > 0);
    }

    #[test]
    fn unknown_target_format_is_rejected() {
        assert!(parse_target("tiff").is_err());
        assert_eq!(parse_target("JPG").unwrap(), Target::Jpeg);
    }
}
