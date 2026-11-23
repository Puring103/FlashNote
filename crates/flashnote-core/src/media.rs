use anyhow::{Context, Result, bail};
use image::{ImageFormat, ImageReader, Limits};
use std::{
    io::{Cursor, Read},
    path::Path,
};
pub const MAX_FILE: u64 = 32 * 1024 * 1024;
pub struct PreparedAttachment {
    pub name: String,
    pub bytes: Vec<u8>,
    pub preview: Option<Vec<u8>>,
}
pub fn image_preview(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() as u64 > MAX_FILE {
        bail!("图片超过 32MB，请选择较小的文件");
    }
    let format = image::guess_format(bytes).context("无法识别图片格式")?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
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
pub fn prepare_bytes(
    name: String,
    bytes: Vec<u8>,
    force_image: bool,
) -> Result<PreparedAttachment> {
    if bytes.len() as u64 > MAX_FILE {
        bail!("附件超过 32MB");
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
