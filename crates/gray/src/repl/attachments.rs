//! Media attachments, opencode parity (`@opencode-ai/core` media parts +
//! `Image.normalize`): MIME-driven kinds instead of an image-only allowlist,
//! downscale-before-send for images, PDF text via pdftotext, first-frame
//! stills for video. Audio has no model-agnostic wire path on our
//! OpenAI-compatible providers — reported loudly, never silently dropped.

use std::path::Path;
use std::process::Command;

// Image downscale lives in gray-tools (the `read` tool attaches vision
// blocks too); re-exported so existing users keep working.
pub use gray_tools::images::{MAX_BASE64_BYTES, MAX_IMAGE_SIDE, MediaError, normalize_image_bytes};
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

#[path = "attachments_tests.rs"]
#[cfg(test)]
mod tests;
