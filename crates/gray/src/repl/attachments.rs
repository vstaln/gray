//! Media attachments, opencode parity (`@opencode-ai/core` media parts +
//! `Image.normalize`): MIME-driven kinds instead of an image-only allowlist,
//! downscale-before-send for images, PDF text via pdftotext, first-frame
//! stills for video. Audio has no model-agnostic wire path on our
//! OpenAI-compatible providers — reported loudly, never silently dropped.

#[cfg(feature = "clipboard")]
use std::io::Cursor;
use std::path::Path;
use std::process::Command;

/// opencode caps: 2000px longest side, 5MB base64.
pub const MAX_IMAGE_SIDE: u32 = 2000;
pub const MAX_BASE64_BYTES: usize = 5 * 1024 * 1024;
/// PDF text cap per file (chars).
pub const MAX_PDF_CHARS: usize = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Pdf,
    Video,
    Audio,
    Unsupported,
}

pub fn attachment_kind(path: &Path) -> AttachmentKind {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") | Some("jpg") | Some("jpeg") | Some("webp") | Some("gif") | Some("bmp")
        | Some("heic") | Some("heif") => AttachmentKind::Image,
        Some("pdf") => AttachmentKind::Pdf,
        Some("mp4") | Some("mov") | Some("mkv") | Some("webm") | Some("m4v") => {
            AttachmentKind::Video
        }
        Some("mp3") | Some("wav") | Some("m4a") | Some("ogg") | Some("flac") => {
            AttachmentKind::Audio
        }
        _ => AttachmentKind::Unsupported,
    }
}

#[derive(Debug)]
pub enum MediaError {
    Decode(String),
    TooBig(String),
    Extract(String),
    Unsupported(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "could not decode image: {e}"),
            Self::TooBig(e) => write!(f, "image still too big after downscale: {e}"),
            Self::Extract(e) => write!(f, "extract failed: {e}"),
            Self::Unsupported(e) => write!(f, "{e}"),
        }
    }
}

/// Downscale-before-send (opencode `Image.normalize`): longest side capped
/// at 2000px, JPEG stays JPEG, everything else becomes PNG, base64 under
/// 5MB (halve and retry up to 3 times, then fail loudly like SizeError).
/// Returns `(media_type, bytes)`.
///
/// Only compiled with the `clipboard` feature (needs the `image` crate);
/// without it image attachments degrade to a loud skip at the call site.
#[cfg(feature = "clipboard")]
pub fn normalize_image_bytes(bytes: &[u8]) -> Result<(String, Vec<u8>), MediaError> {
    use image::ImageFormat;
    let format = image::guess_format(bytes).map_err(|e| MediaError::Decode(e.to_string()))?;
    let out_format = match format {
        ImageFormat::Jpeg => ImageFormat::Jpeg,
        ImageFormat::Png | ImageFormat::Gif | ImageFormat::WebP => ImageFormat::Png,
        // bmp/heic/etc: not in our decoder set — loud, like opencode DecodeError.
        other => {
            return Err(MediaError::Decode(format!(
                "{other:?} decoding not enabled"
            )));
        }
    };
    let mut img = image::load_from_memory(bytes).map_err(|e| MediaError::Decode(e.to_string()))?;
    for _ in 0..4 {
        if img.width().max(img.height()) > MAX_IMAGE_SIDE {
            img = img.resize(
                MAX_IMAGE_SIDE,
                MAX_IMAGE_SIDE,
                image::imageops::FilterType::Triangle,
            );
        }
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), out_format)
            .map_err(|e| MediaError::Decode(e.to_string()))?;
        if base64_len(&buf) <= MAX_BASE64_BYTES {
            let mime = if out_format == ImageFormat::Jpeg {
                "image/jpeg"
            } else {
                "image/png"
            };
            return Ok((mime.to_string(), buf));
        }
        // Still too big: halve and retry (animated GIFs arrive as frame 0).
        let (w, h) = (img.width().max(1) / 2, img.height().max(1) / 2);
        img = img.resize(w.max(1), h.max(1), image::imageops::FilterType::Triangle);
    }
    Err(MediaError::TooBig(format!(
        "{} bytes",
        base64_len(&img.to_rgba8().into_raw())
    )))
}

/// Without the `clipboard` feature there is no image decoder: callers
/// report this loudly (never silently dropped).
#[cfg(not(feature = "clipboard"))]
pub fn normalize_image_bytes(_bytes: &[u8]) -> Result<(String, Vec<u8>), MediaError> {
    Err(MediaError::Unsupported(
        "image attachments need gray built with --features clipboard".to_string(),
    ))
}

#[cfg(feature = "clipboard")]
fn base64_len(raw: &[u8]) -> usize {
    raw.len().div_ceil(3) * 4
}

/// PDF → text via poppler (`pdftotext -layout file -`). Universal: works on
/// every model with zero provider changes.
pub fn pdf_text(path: &Path) -> Result<String, MediaError> {
    let out = Command::new("pdftotext")
        .args(["-layout", &path.display().to_string(), "-"])
        .output()
        .map_err(|e| MediaError::Extract(format!("pdftotext not available: {e}")))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(MediaError::Extract(if detail.is_empty() {
            "pdftotext failed".to_string()
        } else {
            detail.chars().take(200).collect()
        }));
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err(MediaError::Extract(
            "no extractable text (scanned images?)".to_string(),
        ));
    }
    if text.chars().count() > MAX_PDF_CHARS {
        let cut: String = text.chars().take(MAX_PDF_CHARS).collect();
        Ok(format!("{cut}\n… [truncated at {MAX_PDF_CHARS} chars]"))
    } else {
        Ok(text)
    }
}

/// Video → first-frame JPEG via ffmpeg (capped 1600px wide), fed back
/// through the image normalizer by the caller.
pub fn video_frame(path: &Path) -> Result<Vec<u8>, MediaError> {
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &path.display().to_string(),
            "-frames:v",
            "1",
            "-vf",
            "scale='min(iw,1600)':-2",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "-",
        ])
        .output()
        .map_err(|e| MediaError::Extract(format!("ffmpeg not available: {e}")))?;
    if !out.status.success() || out.stdout.is_empty() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(MediaError::Extract(if detail.is_empty() {
            "no video frame decoded".to_string()
        } else {
            detail.chars().take(200).collect()
        }));
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_map_covers_media() {
        assert_eq!(attachment_kind(Path::new("a.png")), AttachmentKind::Image);
        assert_eq!(attachment_kind(Path::new("a.JPG")), AttachmentKind::Image);
        assert_eq!(attachment_kind(Path::new("a.pdf")), AttachmentKind::Pdf);
        assert_eq!(attachment_kind(Path::new("a.mp4")), AttachmentKind::Video);
        assert_eq!(attachment_kind(Path::new("a.mkv")), AttachmentKind::Video);
        assert_eq!(attachment_kind(Path::new("a.mp3")), AttachmentKind::Audio);
        assert_eq!(attachment_kind(Path::new("a.wav")), AttachmentKind::Audio);
        assert_eq!(
            attachment_kind(Path::new("a.zip")),
            AttachmentKind::Unsupported
        );
        assert_eq!(
            attachment_kind(Path::new("noext")),
            AttachmentKind::Unsupported
        );
    }

    #[test]
    #[cfg(feature = "clipboard")]
    fn normalize_caps_long_side() {
        let img = image::RgbaImage::from_pixel(3000, 100, image::Rgba([9, 9, 9, 255]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        let (mime, out) = normalize_image_bytes(&buf).unwrap();
        assert_eq!(mime, "image/png");
        let back = image::load_from_memory(&out).unwrap();
        assert!(back.width().max(back.height()) <= MAX_IMAGE_SIDE);
    }

    #[test]
    #[cfg(feature = "clipboard")]
    fn normalize_rejects_garbage_loudly() {
        assert!(matches!(
            normalize_image_bytes(b"not an image"),
            Err(MediaError::Decode(_))
        ));
    }

    #[test]
    #[cfg(not(feature = "clipboard"))]
    fn normalize_without_feature_is_loud_unsupported() {
        assert!(matches!(
            normalize_image_bytes(b"not an image"),
            Err(MediaError::Unsupported(_))
        ));
    }

    #[test]
    fn pdf_missing_file_errors() {
        assert!(matches!(
            pdf_text(Path::new("/tmp/gray-test-no-such-file-xyz.pdf")),
            Err(MediaError::Extract(_))
        ));
    }
}
