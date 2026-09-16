//! Image downscale-before-send, opencode `Image.normalize` parity.
//!
//! Moved here from `gray::repl::attachments` so the `read` tool can attach
//! vision blocks for image files (opencode `read` parity: "Image read
//! successfully" + file attachment). `gray` re-exports these; its
//! MIME-driven kinds (pdf/video/audio) stay there.

use std::io::Cursor;
use std::path::Path;

/// opencode caps: 2000px longest side, 5MB base64.
pub const MAX_IMAGE_SIDE: u32 = 2000;
pub const MAX_BASE64_BYTES: usize = 5 * 1024 * 1024;

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

/// Extension allowlist for attempted image reads. Mirrors the image arm of
/// `gray::repl::attachments::attachment_kind` — keep in sync. Magic bytes
/// still decide inside [`normalize_image_bytes`], so a mislabeled file
/// falls back to the binary note instead of failing loudly.
pub fn is_image_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("png")
            | Some("jpg")
            | Some("jpeg")
            | Some("webp")
            | Some("gif")
            | Some("bmp")
            | Some("heic")
            | Some("heif")
    )
}

/// Downscale-before-send (opencode `Image.normalize`): longest side capped
/// at 2000px, JPEG stays JPEG, everything else becomes PNG, base64 under
/// 5MB (halve and retry up to 3 times, then fail loudly like SizeError).
/// Returns `(media_type, bytes)`.
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

fn base64_len(raw: &[u8]) -> usize {
    raw.len().div_ceil(3) * 4
}
