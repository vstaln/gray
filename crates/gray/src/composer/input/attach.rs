use super::*;

pub(crate) fn attach_image(tui: &mut Tui, path: PathBuf) {
    let marker = crate::repl::attachments::attachment_marker(&path);
    tui.attachments.push(path);
    tui.textarea.move_to_end();
    tui.textarea.insert_element(&marker);
    let _ = tui.draw();
}

pub(crate) fn sync_attachment_markers(tui: &mut Tui) {
    let mut open = std::collections::HashMap::<String, usize>::new();
    for (_, name) in crate::repl::attachments::attachment_markers(tui.textarea.text()) {
        *open.entry(name).or_default() += 1;
    }
    tui.attachments.retain(|p| {
        match open.get_mut(&crate::repl::attachments::attachment_label(p)) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        }
    });
}

pub(crate) const MAX_ATTACH_FILE_BYTES: u64 = 100 * 1024 * 1024;

fn decoded_paste_path(raw: &str) -> String {
    let s = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`');
    if let Some(stripped) = s.strip_prefix("file://") {
        url::Url::parse(s)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                stripped
                    .strip_prefix("localhost")
                    .unwrap_or(stripped)
                    .to_string()
            })
    } else {
        s.to_string()
    }
}

fn file_size_exceeds(path: &Path, limit: u64) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() > limit)
        .unwrap_or(false)
}

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

/// Saves a Windows clipboard bitmap to the `__OUT__` path as PNG; exit 0 on
/// success, 1 when the clipboard holds no image. `powershell -Command` folds
/// trailing arguments into the command text, so the path is spliced in.
#[cfg(target_os = "windows")]
const WINDOWS_CLIPBOARD_PNG_SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms,System.Drawing; \
$img=$null; \
for($i=0; $i -lt 10 -and $null -eq $img; $i++){ try { $img=[System.Windows.Forms.Clipboard]::GetImage() } catch { Start-Sleep -Milliseconds 60 } }; \
if($null -ne $img){ $img.Save('__OUT__',[System.Drawing.Imaging.ImageFormat]::Png); $img.Dispose(); exit 0 }; \
exit 1";

/// Reads the Windows clipboard bitmap via PowerShell into a fresh temp PNG.
/// Returns the path when an image was actually saved.
#[cfg(target_os = "windows")]
fn windows_clipboard_image() -> Option<PathBuf> {
    let (file, path) = tempfile::Builder::new()
        .prefix("clipboard-")
        .suffix(".png")
        .tempfile()
        .ok()?
        .keep()
        .ok()?;
    drop(file);
    // An absent file afterwards means the clipboard held no image.
    let _ = std::fs::remove_file(&path);
    let script = WINDOWS_CLIPBOARD_PNG_SCRIPT
        .replace("__OUT__", &path.to_string_lossy().replace('\'', "''"));
    let args: Vec<String> = vec![
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-STA".into(),
        "-Command".into(),
        script,
    ];
    let out = clipboard::output_with_timeout(std::path::Path::new("powershell.exe"), &args)?;
    if !out.status.success() {
        return None;
    }
    match std::fs::metadata(&path) {
        Ok(m) if m.len() > 0 => Some(path),
        _ => {
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

pub(crate) fn try_attach_clipboard_image(tui: &mut Tui) -> bool {
    #[cfg(target_os = "windows")]
    if let Some(path) = windows_clipboard_image() {
        if image::open(&path).is_ok() {
            attach_image(tui, path);
            return true;
        }
        let _ = std::fs::remove_file(&path);
    }
    for (cmd, args) in [
        ("wl-paste", vec!["--type", "image/png"]),
        (
            "xclip",
            vec!["-selection", "clipboard", "-t", "image/png", "-o"],
        ),
    ] {
        if let Some(out) = super::clipboard::output_with_timeout(
            std::path::Path::new(cmd),
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        ) && !out.stdout.is_empty()
            && out.status.success()
            && let Ok(mut tmp) = tempfile::Builder::new()
                .prefix("clipboard-")
                .suffix(".png")
                .tempfile()
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
    if let Some(text) = clipboard::read_system_clipboard_text()
        && try_attach_image_paste(tui, &text)
    {
        return true;
    }
    false
}

#[path = "attach_tests.rs"]
#[cfg(test)]
mod tests;
