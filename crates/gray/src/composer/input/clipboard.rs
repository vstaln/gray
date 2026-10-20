use super::Tui;

pub(crate) fn normalize_paste(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

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

        if std::env::var_os("WAYLAND_DISPLAY").is_none()
            && let Some(pos) = out.iter().position(|(c, _)| c == "wl-paste")
        {
            let wl = out.remove(pos);
            out.push(wl);
        }
        out
    }
}

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

pub(crate) fn read_system_clipboard_text() -> Option<String> {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    read_system_clipboard_text_with_paths(&paths.to_string_lossy())
}

const CLIPBOARD_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(4000);

pub(crate) fn output_with_timeout(
    full: &std::path::Path,
    args: &[String],
) -> Option<std::process::Output> {
    let (tx, rx) = std::sync::mpsc::channel();
    let full = full.to_path_buf();
    let args = args.to_owned();
    std::thread::spawn(move || {
        let out = std::process::Command::new(&full).args(&args).output();
        let _ = tx.send(out);
    });
    rx.recv_timeout(CLIPBOARD_CMD_TIMEOUT).ok()?.ok()
}

pub(crate) fn read_system_clipboard_text_with_paths(paths: &str) -> Option<String> {
    for (cmd, args) in text_clipboard_candidates() {
        let Some(full) = resolve_in(&cmd, paths) else {
            continue;
        };

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

#[path = "clipboard_tests.rs"]
#[cfg(test)]
mod tests;
