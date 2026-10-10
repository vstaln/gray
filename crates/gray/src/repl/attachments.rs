//! Media attachments, opencode parity (`@opencode-ai/core` media parts +
//! `Image.normalize`): MIME-driven kinds instead of an image-only allowlist,
//! downscale-before-send for images. Video, PDF and audio go as native media
//! with a fallback (contact sheet, pdftotext text, a note) that the provider
//! swaps in when the model can't take the native part.

use std::path::{Path, PathBuf};

// Image normalization lives in gray-tools (the `read` tool attaches vision
// blocks too); re-exported so existing users keep working.
pub use gray_tools::images::{MAX_BASE64_BYTES, MediaError, normalize_image_bytes};

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
        _ if gray_tools::images::is_video_extension(path) => AttachmentKind::Video,
        _ if gray_tools::images::is_audio_extension(path) => AttachmentKind::Audio,
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
/// Typed file links in `text` that carry media: images, video, PDF, audio. The name
/// predates video support; renaming it would churn every caller for no
/// behavior change, so it stays.
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
        let decoded: std::borrow::Cow<'_, str> = if tok.starts_with("file://") {
            match url::Url::parse(tok)
                .ok()
                .and_then(|u| u.to_file_path().ok())
            {
                Some(p) => std::borrow::Cow::Owned(p.display().to_string()),
                None => {
                    let s = tok.strip_prefix("file://").unwrap_or(tok);
                    std::borrow::Cow::Borrowed(s.strip_prefix("localhost").unwrap_or(s))
                }
            }
        } else {
            std::borrow::Cow::Borrowed(tok)
        };
        tok = decoded.as_ref();
        let candidate = Path::new(tok);
        // A typed link to a video, PDF or audio file is as valid as one to a
        // screenshot: the builder sends it natively or as its fallback.
        // Filtering this arm to images made a pasted video path silently
        // arrive as nothing at all.
        if attachment_kind(candidate) == AttachmentKind::Unsupported {
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
#[path = "attachments_tests.rs"]
#[cfg(test)]
mod tests;
