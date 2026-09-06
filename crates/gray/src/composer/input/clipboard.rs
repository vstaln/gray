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
        return vec![("pbpaste".to_string(), vec![])];
    }
    #[cfg(target_os = "windows")]
    {
        return vec![(
            "powershell.exe".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                "Get-Clipboard".to_string(),
            ],
        )];
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

/// True when `cmd` resolves inside `paths` (a `PATH`-shaped list) to an
/// existing file. Absolute/relative paths are checked directly.
// In-flight (unwired): silenced for CI -D warnings; wire up or delete.
#[allow(dead_code)]
pub(crate) fn have_in(cmd: &str, paths: &str) -> bool {
    resolve_in(cmd, paths).is_some()
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

/// Runs one clipboard helper; `None` on spawn failure, non-zero exit, or
/// empty/whitespace-only output (opencode treats those as "no text").
/// The `probe` gate decides whether the helper may run at all: production
/// passes [`have_in`] against the real PATH, tests inject shims (a real
/// `xclip` on PATH must not shadow the shim under test).
// In-flight (unwired): silenced for CI -D warnings; wire up or delete.
#[allow(dead_code)]
pub(crate) fn run_candidate_with_probe(
    cmd: &str,
    args: &[String],
    probe: impl Fn(&str) -> bool,
) -> Option<String> {
    if !probe(cmd) {
        return None;
    }
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.trim().is_empty() {
        return None;
    }
    Some(text)
}

/// Production runner: a helper runs only when found on the real PATH.
// In-flight (unwired): silenced for CI -D warnings; wire up or delete.
#[allow(dead_code)]
pub(crate) fn run_candidate(cmd: &str, args: &[String]) -> Option<String> {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    let paths = paths.to_string_lossy().into_owned();
    run_candidate_with_probe(cmd, args, |c| have_in(c, &paths))
}

#[cfg(feature = "clipboard")]
fn arboard_text() -> Option<String> {
    if let Ok(clipboard) = arboard::Clipboard::new()
        && let Ok(text) = clipboard.get_text()
        && !text.trim().is_empty()
    {
        return Some(text);
    }
    None
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
        // ponytail: one spawn path; None on spawn failure/non-zero/blank.
        let out = std::process::Command::new(&full)
            .args(&args)
            .output()
            .ok()?;
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

    #[test]
    fn have_in_finds_shims_and_rejects_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = dir.path().join("xclip");
        std::fs::write(&shim, "#!/bin/sh\necho hi\n").expect("write shim");
        let paths = dir.path().to_string_lossy().into_owned();
        assert!(have_in("xclip", &paths));
        assert!(!have_in("wl-paste", &paths));
        assert!(!have_in("xclip", ""));
        // absolute path form
        assert!(have_in(&shim.to_string_lossy(), &paths));
        assert!(!have_in("/nonexistent-probe-xyz", &paths));
    }

    #[test]
    fn run_candidate_accepts_output_rejects_failures() {
        assert_eq!(
            run_candidate("printf", &["hi".to_string()]),
            Some("hi".to_string())
        );
        assert_eq!(run_candidate("false", &[]), None);
        assert_eq!(run_candidate("printf", &["   \n ".to_string()]), None);
        assert_eq!(run_candidate("gray-definitely-not-a-binary", &[]), None);
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
