//! Media attachments, opencode parity (`@opencode-ai/core` media parts +
//! `Image.normalize`): MIME-driven kinds instead of an image-only allowlist,
//! downscale-before-send for images, PDF text via pdftotext, first-frame
//! stills for video. Audio has no model-agnostic wire path on our
//! OpenAI-compatible providers — reported loudly, never silently dropped.

use std::path::{Path, PathBuf};

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

/// Max inline file links auto-attached per turn (DoS cap on `fs::read` below).
const MAX_INLINE_IMAGES: usize = 8;
/// Inline attach skips files at/above this (mirrors composer paste cap).
const MAX_INLINE_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Scan prompt text for image file links (`/tmp/a.png`, `./a.jpg`,
/// `file:///tmp/a%20b.png`) and return existing image files resolved against
/// `cwd`. Typed/piped `-p` links never go through paste-attach, so without
/// this they stay plain text and the model never sees them.
pub fn extract_inline_image_paths(text: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for raw in text.split_whitespace() {
        if out.len() >= MAX_INLINE_IMAGES {
            break;
        }
        let mut tok = raw
            .trim_matches(|c| matches!(c, '"' | '\'' | '`' | '<' | '(' | '['))
            .trim_end_matches(|c| {
                matches!(
                    c,
                    '"' | '\'' | '`' | '>' | ')' | ']' | '.' | ',' | ';' | ':' | '!' | '?'
                )
            });
        if tok.is_empty() || tok.len() > 1024 {
            continue;
        }
        // Strip file:// (+localhost) + percent-decode; raw paths pass through
        // untouched so literal % in real filenames is never corrupted.
        let decoded: std::borrow::Cow<'_, str> = if let Some(stripped) = tok.strip_prefix("file://")
        {
            let s = stripped.strip_prefix("localhost").unwrap_or(stripped);
            percent_encoding::percent_decode_str(s).decode_utf8_lossy()
        } else {
            std::borrow::Cow::Borrowed(tok)
        };
        tok = decoded.as_ref();
        let candidate = Path::new(tok);
        if attachment_kind(candidate) != AttachmentKind::Image {
            continue;
        }
        let full = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            cwd.join(candidate)
        };
        if full.is_file()
            && std::fs::metadata(&full)
                .map(|m| m.len() < MAX_INLINE_FILE_BYTES)
                .unwrap_or(false)
            && !out.contains(&full)
        {
            out.push(full);
        }
    }
    out
}
/// External media helpers (`pdftotext`, `ffmpeg`) run bounded: a malformed
/// file or a wedged helper must not hold the attach flow forever (audit
/// #15). Same mpsc pattern as the clipboard text path.
const MEDIA_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn output_with_timeout_in(
    cmd: &str,
    args: &[&str],
    limit: std::time::Duration,
) -> Result<std::process::Output, MediaError> {
    let program = cmd.to_string();
    let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(
            std::process::Command::new(&program)
                .args(&argv)
                .output()
                .map_err(|e| std::io::Error::other(e.to_string())),
        );
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(MediaError::Extract(format!("{cmd} not available: {e}"))),
        Err(_) => Err(MediaError::Extract(format!(
            "{cmd} timed out after {}s",
            limit.as_secs()
        ))),
    }
}

fn output_with_timeout(cmd: &str, args: &[&str]) -> Result<std::process::Output, MediaError> {
    output_with_timeout_in(cmd, args, MEDIA_CMD_TIMEOUT)
}

/// PDF → text via poppler (`pdftotext -layout file -`). Universal: works on
/// every model with zero provider changes.
pub fn pdf_text(path: &Path) -> Result<String, MediaError> {
    let arg_path = path.display().to_string();
    let out = output_with_timeout("pdftotext", &["-layout", &arg_path, "-"])?;
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

/// Wire MIME type for a video attachment, by extension. A native video part
/// has to name its type, and the model's endpoint is the one that will
/// reject a wrong guess; the extension is the only signal we have.
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

/// Video → first-frame JPEG via ffmpeg (capped 1600px wide), fed back
/// through the image normalizer by the caller. Still used by the preview
/// strip; the pasted attachment prefers a native part or a contact sheet.
pub fn video_frame(path: &Path) -> Result<Vec<u8>, MediaError> {
    let out = output_with_timeout(
        "ffmpeg",
        &[
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
        ],
    )?;
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
