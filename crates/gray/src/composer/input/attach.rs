//! Attachment helpers (split from `input`; re-exported there).

use super::*;

// ---------------------------------------------------------------------------
// Attachment helpers — verbatim from mod.rs 759-885
// ---------------------------------------------------------------------------

/// Placeholder index across both `[Image #n]` (images) and `[File #n]` (pdf/video/other).
fn placeholder_index(ph: &str) -> Option<usize> {
    ph.strip_prefix("[Image #")
        .or_else(|| ph.strip_prefix("[File #"))
        .and_then(|s| s.strip_suffix(']'))
        .and_then(|n| n.parse::<usize>().ok())
}

pub(crate) fn attach_image(tui: &mut Tui, path: PathBuf) {
    let mut max_idx = 0;
    for (ph, _) in &tui.attachments {
        if let Some(n) = placeholder_index(ph) {
            max_idx = max_idx.max(n);
        }
    }
    let text = tui.textarea.text().to_string();
    for prefix in ["[Image #", "[File #"] {
        for cap in text.match_indices(prefix) {
            let substr = &text[cap.0..];
            if let Some(end) = substr.find(']') {
                let num_str = &substr[prefix.len()..end];
                if let Ok(n) = num_str.parse::<usize>() {
                    max_idx = max_idx.max(n);
                }
            }
        }
    }
    let idx = max_idx + 1;
    let placeholder = if crate::repl::attachments::attachment_kind(&path)
        == crate::repl::attachments::AttachmentKind::Image
    {
        format!("[Image #{idx}]")
    } else {
        format!("[File #{idx}]")
    };
    tui.textarea.insert_element(&placeholder);
    tui.attachments.push((placeholder.clone(), path));
    let _ = tui.draw();
}

pub(crate) fn sync_attachments(tui: &mut Tui) {
    let text = tui.textarea.text().to_string();
    tui.attachments.retain(|(ph, _)| text.contains(ph));
}

/// Max attachable file size. `repl::attachments` only caps outputs
/// (`MAX_IMAGE_SIDE` / `MAX_BASE64_BYTES`), with no input-file cap, so this
/// local sane cap refuses multi-GB drops before ffmpeg/image decode OOMs.
pub(crate) const MAX_ATTACH_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Strip `file://` (+ optional `localhost`) and percent-decode (%20 etc.).
/// Raw (non-`file://`) paths pass through untouched so literal `%` in real
/// filenames is never corrupted. Shared by paste + clipboard-text paths.
fn decoded_paste_path(raw: &str) -> String {
    let s = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`');
    if let Some(stripped) = s.strip_prefix("file://") {
        let s = stripped.strip_prefix("localhost").unwrap_or(stripped);
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    } else {
        s.to_string()
    }
}

fn file_size_exceeds(path: &Path, limit: u64) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() > limit)
        .unwrap_or(false)
}

/// Any file the media pipeline accepts (images, pdf, video, audio) —
/// opencode parity: MIME-driven, not image-only.
pub(crate) fn is_attachable_path(path: &str) -> bool {
    use crate::repl::attachments::{AttachmentKind, attachment_kind};
    let decoded = decoded_paste_path(path);
    let p = Path::new(&decoded);
    if !p.exists() || !p.is_file() {
        return false;
    }
    if file_size_exceeds(p, MAX_ATTACH_FILE_BYTES) {
        return false;
    }
    !matches!(attachment_kind(p), AttachmentKind::Unsupported)
}

pub(crate) fn try_attach_image_paste(tui: &mut Tui, pasted: &str) -> bool {
    let trimmed = pasted
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`');
    if trimmed.contains('\n') || trimmed.is_empty() || trimmed.len() > 512 {
        return false;
    }
    let path_str = decoded_paste_path(trimmed);
    let p = Path::new(&path_str);
    if p.exists() && p.is_file() && file_size_exceeds(p, MAX_ATTACH_FILE_BYTES) {
        tui.push_dim(format!(
            "attachment too big (>{}MB, skipped): {}",
            MAX_ATTACH_FILE_BYTES / 1024 / 1024,
            p.display()
        ));
        let _ = tui.draw();
        return true;
    }
    if is_attachable_path(&path_str) {
        let path = PathBuf::from(path_str);
        attach_image(tui, path);
        return true;
    }
    false
}

/// Paste an image from the OS clipboard (arboard) or clipboard helpers
/// (wl-paste/xclip). Only compiled with the `clipboard` feature; without it
/// pastes fall through to plain text.
#[cfg(feature = "clipboard")]
pub(crate) fn try_attach_clipboard_image(tui: &mut Tui) -> bool {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(img) = clipboard.get_image() {
            let w = img.width as u32;
            let h = img.height as u32;
            if let Some(rgba) = image::RgbaImage::from_raw(w, h, img.bytes.into_owned())
                && let Ok(mut tmp) = tempfile::Builder::new().suffix(".png").tempfile()
                && image::DynamicImage::ImageRgba8(rgba)
                    .write_to(&mut tmp, image::ImageFormat::Png)
                    .is_ok()
                && let Ok((_file, path)) = tmp.keep()
            {
                attach_image(tui, path);
                return true;
            }
        }
        if let Ok(text) = clipboard.get_text() {
            let decoded = decoded_paste_path(&text);
            let p = Path::new(&decoded);
            if p.exists() && p.is_file() && file_size_exceeds(p, MAX_ATTACH_FILE_BYTES) {
                tui.push_dim(format!(
                    "attachment too big (>{}MB, skipped): {}",
                    MAX_ATTACH_FILE_BYTES / 1024 / 1024,
                    p.display()
                ));
                let _ = tui.draw();
                return true;
            }
            if is_attachable_path(&decoded) {
                attach_image(tui, PathBuf::from(decoded));
                return true;
            }
        }
    }
    for (cmd, args) in [
        ("wl-paste", vec!["--type", "image/png"]),
        (
            "xclip",
            vec!["-selection", "clipboard", "-t", "image/png", "-o"],
        ),
    ] {
        if let Ok(out) = std::process::Command::new(cmd).args(&args).output()
            && !out.stdout.is_empty()
            && out.status.success()
            && let Ok(mut tmp) = tempfile::Builder::new().suffix(".png").tempfile()
            && std::io::Write::write_all(&mut tmp, &out.stdout).is_ok()
            && let Ok((_file, path)) = tmp.keep()
        {
            if image::open(&path).is_ok() {
                attach_image(tui, path);
                return true;
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    false
}

#[cfg(not(feature = "clipboard"))]
pub(crate) fn try_attach_clipboard_image(_tui: &mut Tui) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // UNRUN (cargo test banned under X — verified via check + clippy only).
    #[test]
    fn file_url_percent_decoding() {
        assert_eq!(
            decoded_paste_path("file:///tmp/my%20photo.png"),
            "/tmp/my photo.png"
        );
        assert_eq!(
            decoded_paste_path("file://localhost/tmp/my%20photo.png"),
            "/tmp/my photo.png"
        );
        assert_eq!(
            decoded_paste_path("\"file:///tmp/a%20b.png\""),
            "/tmp/a b.png"
        );
        // Raw paths pass through: literal % must not be corrupted.
        assert_eq!(decoded_paste_path("/tmp/100%.png"), "/tmp/100%.png");
        assert_eq!(decoded_paste_path("/tmp/plain.png"), "/tmp/plain.png");
    }

    // UNRUN (cargo test banned under X — verified via check + clippy only).
    #[test]
    fn size_gate_uses_limit() {
        let mut tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        tmp.write_all(b"0123456789").unwrap();
        tmp.flush().unwrap();
        assert!(file_size_exceeds(tmp.path(), 5));
        assert!(!file_size_exceeds(tmp.path(), MAX_ATTACH_FILE_BYTES));
        assert!(!file_size_exceeds(
            Path::new("/tmp/gray-test-no-such-file-xyz"),
            0
        ));
    }

    // UNRUN (cargo test banned under X — verified via check + clippy only).
    #[test]
    fn attachable_path_small_png_passes() {
        let mut tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        tmp.write_all(b"fake-png").unwrap();
        tmp.flush().unwrap();
        assert!(is_attachable_path(&tmp.path().display().to_string()));
        assert!(!is_attachable_path("/tmp/gray-test-no-such-file-xyz.png"));
        assert!(!is_attachable_path("not a path\nwith newline.png"));
    }
}
