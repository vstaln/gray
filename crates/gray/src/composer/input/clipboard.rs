//! System-clipboard paste backend — opencode `prompt.paste` parity.
//!
//! Gray's paste path used to accept terminal-driven bracketed paste
//! (`Event::Paste`) plus an image-only Ctrl+V. There was no text fallback:
//! on a default build (no `clipboard` feature) Ctrl+V was a silent no-op,
//! and any terminal that doesn't emit bracketed paste (or a `$EDITOR` that
//! cleared mode 2004 mid-session) left the user with no way to paste at
//! all. Opencode perfected this with a `prompt.paste` command (bound to
//! ctrl+v) that reads the OS clipboard — image first, then text — plus an
//! empty-bracketed-paste → clipboard-read fallback. This module is that
//! backend half; the frontend (draw code) is untouched.
//!
//! Text reads are dependency-free native helpers (`pbpaste`, `wl-paste`,
//! `xclip`, `xsel`, PowerShell, termux) so they work on the default build.
//! With the `clipboard` feature, arboard is tried first (it also covers
//! Wayland session quirks the CLI helpers sometimes miss).

use super::Tui;

/// opencode parity: Windows ConPTY/Terminal often sends CR-only newlines in
/// bracketed paste — replace CRLF first, then any remaining CR.
pub(crate) fn normalize_paste(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Candidate text-clipboard readers for this OS, in probe order (opencode
/// `clipboard.ts` order: native Wayland/X11 helpers, platform shell-outs).
pub(crate) fn text_clipboard_candidates() -> Vec<(String, Vec<String>)> {
    #[cfg(target_os = "macos")]
    {
        vec![("pbpaste".to_string(), vec![])]
    }
    #[cfg(target_os = "windows")]
    {
        vec![(
            "powershell.exe".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                "Get-Clipboard".to_string(),
            ],
        )]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut out = vec![
            (
                "wl-paste".to_string(),
                vec![
                    "--no-newline".to_string(),
                    "-t".to_string(),
                    "text/plain".to_string(),
                ],
            ),
            (
                "xclip".to_string(),
                vec![
                    "-selection".to_string(),
                    "clipboard".to_string(),
                    "-o".to_string(),
                ],
            ),
            (
                "xsel".to_string(),
                vec!["--clipboard".to_string(), "--output".to_string()],
            ),
            ("termux-clipboard-get".to_string(), vec![]),
        ];
        // Prefer wl-paste under Wayland but still probe the rest: headless
        // boxes often only have one of them installed.
        if std::env::var_os("WAYLAND_DISPLAY").is_none()
            && let Some(pos) = out.iter().position(|(c, _)| c == "wl-paste")
        {
            let wl = out.remove(pos);
            out.push(wl);
        }
        out
    }
}

/// Full path of `cmd` inside `paths`, or `None` when absent.
/// Absolute/relative paths are checked directly.
pub(crate) fn resolve_in(cmd: &str, paths: &str) -> Option<std::path::PathBuf> {
    if cmd.contains('/') || cmd.contains('\\') {
        let p = std::path::Path::new(cmd);
        return p.is_file().then(|| p.to_path_buf());
    }
    let sep = if cfg!(target_os = "windows") {
        ';'
    } else {
        ':'
    };
    paths.split(sep).find_map(|dir| {
        if dir.is_empty() {
            return None;
        }
        let p = std::path::Path::new(dir).join(cmd);
        p.is_file().then_some(p)
    })
}

#[cfg(feature = "clipboard")]
fn arboard_text() -> Option<String> {
    if let Ok(mut clipboard) = arboard::Clipboard::new()
        && let Ok(text) = clipboard.get_text()
        && !text.trim().is_empty()
    {
        return Some(text);
    }
    None
}

/// Bound for one clipboard helper: `wl-paste` blocks until the compositor
/// answers, which used to freeze the draw loop inside `Command::output`.
const CLIPBOARD_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(2000);

/// `Command::output` on a spawned thread (`std::thread::spawn` + mpsc, same
/// shape as the modal flights) with a bounded wait so a hung
/// helper can't block the UI thread. `None` on spawn failure/timeout —
/// same as the old blocking `.ok()?` path. The orphaned thread exits on
/// its own when the helper does; its send then fails silently.
fn output_with_timeout(full: &std::path::Path, args: &[String]) -> Option<std::process::Output> {
    let (tx, rx) = std::sync::mpsc::channel();
    let full = full.to_path_buf();
    let args = args.to_owned();
    std::thread::spawn(move || {
        let out = std::process::Command::new(&full).args(&args).output();
        let _ = tx.send(out);
    });
    rx.recv_timeout(CLIPBOARD_CMD_TIMEOUT).ok()?.ok()
}

/// Reads OS clipboard text via native helpers found in `paths`. Hermetic
/// (takes the PATH explicitly) so tests can point it at shim binaries.
/// Executes the resolved full path — `Command::new(cmd)` would re-resolve
/// via the real PATH and miss the shims.
pub(crate) fn read_system_clipboard_text_with_paths(paths: &str) -> Option<String> {
    for (cmd, args) in text_clipboard_candidates() {
        let Some(full) = resolve_in(&cmd, paths) else {
            continue;
        };
        // ponytail: one spawn path; None on spawn failure/timeout/non-zero/blank.
        let out = output_with_timeout(&full, &args)?;
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        if text.trim().is_empty() {
            continue;
        }
        return Some(text);
    }
    None
}

/// Production entry: arboard first when compiled in, then native helpers on
/// the real PATH.
pub(crate) fn read_system_clipboard_text() -> Option<String> {
    #[cfg(feature = "clipboard")]
    if let Some(text) = arboard_text() {
        return Some(text);
    }
    let paths = std::env::var_os("PATH").unwrap_or_default();
    read_system_clipboard_text_with_paths(&paths.to_string_lossy())
}

/// opencode `prompt.paste`: image attach first, then clipboard text through
/// the normal bracketed-paste path (normalization + large-paste collapse).
/// True when anything landed in the composer.
pub(crate) fn paste_from_system_clipboard(tui: &mut Tui) -> bool {
    if super::try_attach_clipboard_image(tui) {
        return true;
    }
    if let Some(text) = read_system_clipboard_text()
        && !text.trim().is_empty()
    {
        return super::handle_paste(tui, text);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_paste_handles_crlf_and_lone_cr() {
        assert_eq!(normalize_paste("a\r\nb"), "a\nb");
        assert_eq!(normalize_paste("a\rb"), "a\nb");
        assert_eq!(normalize_paste("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(normalize_paste("plain\ntext"), "plain\ntext");
        assert_eq!(normalize_paste(""), "");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn clipboard_chain_reads_through_fake_xclip() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = dir.path().join("xclip");
        std::fs::write(&shim, "#!/bin/sh\nprintf 'pasted-text'").expect("write shim");
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let paths = dir.path().to_string_lossy().into_owned();
        // wl-paste is absent so the chain must fall through to the xclip shim.
        assert_eq!(
            read_system_clipboard_text_with_paths(&paths),
            Some("pasted-text".to_string())
        );
    }

    #[test]
    fn clipboard_chain_empty_path_gives_none_fast() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = dir.path().to_string_lossy().into_owned();
        assert_eq!(read_system_clipboard_text_with_paths(&paths), None);
    }
}
