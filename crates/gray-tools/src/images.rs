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

/// Extension allowlist for video `gray view` will turn into a contact sheet.
/// Widened only where a real clip lives; the sheet path shells out to ffmpeg,
/// which is the thing that actually decodes these, so the list is a cheap
/// first gate rather than a promise.
pub fn is_video_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("mp4")
            | Some("m4v")
            | Some("mov")
            | Some("webm")
            | Some("mkv")
            | Some("avi")
            | Some("mpg")
            | Some("mpeg")
    )
}

/// What `gray view` can show: an image as itself, a video as a sheet.
pub fn is_viewable_extension(path: &Path) -> bool {
    is_image_extension(path) || is_video_extension(path)
}

/// Wire MIME type for a video, by extension. A native video part has to name
/// its type; the extension is the only signal we have, and the provider is
/// what ultimately decides whether the model accepts it.
pub fn video_media_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        Some("m4v") => "video/x-m4v",
        Some("mpg") | Some("mpeg") => "video/mpeg",
        _ => "video/mp4",
    }
}

/// Downscale-before-send (opencode `Image.normalize`): longest side capped
/// at 2000px, JPEG stays JPEG, everything else becomes PNG, base64 under
/// 5MB (halve and retry up to 3 times, then fail loudly like SizeError).
/// Returns `(media_type, bytes)`.
pub fn normalize_image_bytes(bytes: &[u8]) -> Result<(String, Vec<u8>), MediaError> {
    normalize_inner(bytes, Some(MAX_IMAGE_SIDE), true)
}

/// Full-resolution variant: decode, apply EXIF orientation, re-encode at
/// native size — no downscale and no halving retry. Same format mapping as
/// [`normalize_image_bytes`] (jpeg stays jpeg, everything else becomes
/// png) so every provider accepts the part. The base64 size cap stays a
/// hard error rather than a silent shrink: the caller asked for the real
/// pixels, and a shrunk image is a wrong answer to "what does this look
/// like".
pub fn encode_image_full(bytes: &[u8]) -> Result<(String, Vec<u8>), MediaError> {
    normalize_inner(bytes, None, false)
}

fn normalize_inner(
    bytes: &[u8],
    max_side: Option<u32>,
    halve_on_oversize: bool,
) -> Result<(String, Vec<u8>), MediaError> {
    use image::{ImageDecoder, ImageFormat};
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
    let mut decoder = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(|e| MediaError::Decode(e.to_string()))?;
    let orientation = decoder
        .orientation()
        .map_err(|e| MediaError::Decode(e.to_string()))?;
    let mut img = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| MediaError::Decode(e.to_string()))?;
    img.apply_orientation(orientation);
    let mut last_size = 0;
    for attempt in 0..4 {
        if let Some(side) = max_side
            && img.width().max(img.height()) > side
        {
            img = img.resize(side, side, image::imageops::FilterType::Triangle);
        }
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), out_format)
            .map_err(|e| MediaError::Decode(e.to_string()))?;
        last_size = base64_len(&buf);
        if last_size <= MAX_BASE64_BYTES {
            let mime = if out_format == ImageFormat::Jpeg {
                "image/jpeg"
            } else {
                "image/png"
            };
            return Ok((mime.to_string(), buf));
        }
        if !halve_on_oversize || attempt == 3 {
            break;
        }
        // Still too big: halve and retry (animated GIFs arrive as frame 0).
        let (w, h) = (img.width().max(1) / 2, img.height().max(1) / 2);
        img = img.resize(w.max(1), h.max(1), image::imageops::FilterType::Triangle);
    }
    Err(MediaError::TooBig(format!("{} bytes", last_size)))
}

fn base64_len(raw: &[u8]) -> usize {
    raw.len().div_ceil(3) * 4
}
