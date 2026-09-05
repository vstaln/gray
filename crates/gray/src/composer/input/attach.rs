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

/// Any file the media pipeline accepts (images, pdf, video, audio) —
/// opencode parity: MIME-driven, not image-only.
pub(crate) fn is_attachable_path(path: &str) -> bool {
    use crate::repl::attachments::{AttachmentKind, attachment_kind};
    let p = Path::new(
        path.trim()
            .trim_matches(|c| c == '"' || c == '\'' || c == '`'),
    );
    if !p.exists() || !p.is_file() {
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
    let path_str = if let Some(stripped) = trimmed.strip_prefix("file://") {
        stripped
    } else {
        trimmed
    };
    if is_attachable_path(path_str) {
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
        if let Ok(text) = clipboard.get_text()
            && is_attachable_path(&text)
        {
            attach_image(tui, PathBuf::from(text.trim()));
            return true;
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
