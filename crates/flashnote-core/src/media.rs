use anyhow::{Context, Result, bail};
use image::{ImageFormat, ImageReader, Limits};
use std::{
    io::{Cursor, Read},
    path::Path,
};
const IMAGE_COMPRESSION_THRESHOLD: u64 = 100 * 1024 * 1024;
pub const MAX_FILE: u64 = 128 * 1024 * 1024;
pub struct PreparedAttachment {
    pub name: String,
    pub bytes: Vec<u8>,
    pub preview: Option<Vec<u8>>,
}
pub fn image_preview(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() as u64 > MAX_FILE {
        bail!("图片超过 128 MiB，请选择较小的文件");
    }
    let format = image::guess_format(bytes).context("无法识别图片格式")?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(192 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().context("图片损坏或分辨率过大")?;
    let preview = if decoded.width() > 1920 || decoded.height() > 1920 {
        decoded.thumbnail(1920, 1920)
    } else {
        decoded
    };
    let mut out = Cursor::new(Vec::new());
    preview.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}
/// Produce an optional upload copy. The attachment's format and file name stay paired.
pub(crate) fn sync_image(bytes: &[u8]) -> Option<Vec<u8>> {
    use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageEncoder, codecs::*};

    if !(IMAGE_COMPRESSION_THRESHOLD + 1..=MAX_FILE).contains(&(bytes.len() as u64)) {
        return None;
    }
    let format = image::guess_format(bytes).ok()?;
    let input = Cursor::new(bytes);
    let mut reader = ImageReader::with_format(input.clone(), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(192 * 1024 * 1024);
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder().ok()?;
    if decoder.total_bytes() > limits.max_alloc? {
        return None;
    }
    match format {
        ImageFormat::Png if png::PngDecoder::new(input.clone()).ok()?.is_apng().ok()? => {
            return None;
        }
        ImageFormat::WebP if webp::WebPDecoder::new(input.clone()).ok()?.has_animation() => {
            return None;
        }
        ImageFormat::Gif => {
            let mut gif = gif::GifDecoder::new(input.clone()).ok()?;
            gif.set_limits(limits).ok()?;
            if gif.into_frames().nth(1).is_some() {
                return None;
            }
        }
        ImageFormat::Tiff
            if ::tiff::decoder::Decoder::new(input.clone())
                .ok()?
                .more_images() =>
        {
            return None;
        }
        _ => {}
    }
    let icc = decoder.icc_profile().ok()?;
    let exif = decoder.exif_metadata().ok()?;
    let image = DynamicImage::from_decoder(decoder).ok()?;
    let (width, height) = sync_dimensions(image.width(), image.height());
    let image = if (width, height) != (image.width(), image.height()) {
        image.resize_exact(width, height, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    fn encode(
        image: &DynamicImage,
        mut encoder: impl ImageEncoder,
        icc: Option<Vec<u8>>,
        exif: Option<Vec<u8>>,
    ) -> Option<()> {
        if let Some(profile) = icc {
            encoder.set_icc_profile(profile).ok()?;
        }
        if let Some(metadata) = exif {
            encoder.set_exif_metadata(metadata).ok()?;
        }
        image.write_with_encoder(encoder).ok()
    }
    let mut out = Cursor::new(Vec::new());
    match format {
        ImageFormat::Jpeg => encode(
            &image,
            jpeg::JpegEncoder::new_with_quality(&mut out, 90),
            icc,
            exif,
        ),
        ImageFormat::Png => encode(
            &image,
            png::PngEncoder::new_with_quality(
                &mut out,
                png::CompressionType::Best,
                png::FilterType::Adaptive,
            ),
            icc,
            exif,
        ),
        ImageFormat::WebP => encode(&image, webp::WebPEncoder::new_lossless(&mut out), icc, exif),
        ImageFormat::Bmp => encode(&image, bmp::BmpEncoder::new(&mut out), icc, exif),
        ImageFormat::Gif => encode(&image, gif::GifEncoder::new(&mut out), icc, exif),
        ImageFormat::Tiff => encode(&image, tiff::TiffEncoder::new(&mut out), icc, exif),
        _ => return None,
    }?;
    let out = out.into_inner();
    (out.len() as u64 * 5 <= bytes.len() as u64 * 4).then_some(out)
}

fn sync_dimensions(width: u32, height: u32) -> (u32, u32) {
    let pixels = u64::from(width) * u64::from(height);
    if pixels <= 12_000_000 {
        return (width, height);
    }
    let scale = (12_000_000.0 / pixels as f64).sqrt();
    (
        (f64::from(width) * scale) as u32,
        (f64::from(height) * scale) as u32,
    )
}

pub fn prepare_bytes(
    name: String,
    bytes: Vec<u8>,
    force_image: bool,
) -> Result<PreparedAttachment> {
    if bytes.len() as u64 > MAX_FILE {
        bail!("附件超过 128 MiB");
    }
    let ext = Path::new(&name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_image = force_image
        || ["png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff"].contains(&ext.as_str())
        || image::guess_format(&bytes).is_ok();
    let preview = if is_image {
        Some(image_preview(&bytes)?)
    } else {
        None
    };
    Ok(PreparedAttachment {
        name,
        bytes,
        preview,
    })
}
pub fn prepare_file(path: &Path) -> Result<PreparedAttachment> {
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        bail!("请选择文件");
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    prepare_bytes(
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        bytes,
        false,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pixel_budget_preserves_long_screenshots_and_scales_large_photos() {
        assert_eq!(sync_dimensions(1080, 8000), (1080, 8000));
        assert_eq!(sync_dimensions(8000, 6000), (4000, 3000));
        assert_eq!(sync_dimensions(3000, 4000), (3000, 4000));
    }

    #[test]
    fn already_small_png_and_animated_gif_use_original() {
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        assert!(sync_image(png.get_ref()).is_none());
        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            for _ in 0..2 {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::new(32, 32)))
                    .unwrap();
            }
        }
        gif.resize(gif.len() + 4096, 0);
        assert!(sync_image(&gif).is_none());
    }
    #[test]
    fn validates_content_not_extension_and_preserves_original() {
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 3)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        let original = out.into_inner();
        let p = prepare_bytes("wrong.jpg".into(), original.clone(), false).unwrap();
        assert_eq!(p.bytes, original);
        assert_eq!(
            image::load_from_memory(&p.preview.unwrap())
                .unwrap()
                .height(),
            3
        );
    }
    #[test]
    fn corrupt_images_and_oversize_are_errors() {
        assert!(prepare_bytes("bad.png".into(), b"not png".to_vec(), false).is_err());
        assert!(image_preview(&vec![0; MAX_FILE as usize + 1]).is_err());
        assert!(prepare_bytes("clipboard.svg".into(), b"<svg/>".to_vec(), true).is_err());
        assert!(
            prepare_bytes("note.txt".into(), b"hello".to_vec(), false)
                .unwrap()
                .preview
                .is_none()
        );
    }
}
