use std::path::{Path, PathBuf};

pub fn should_use_external_editor(editor: Option<&str>) -> bool {
    editor.is_some_and(|s| !s.trim().is_empty())
}

fn editor_program(editor: Option<&str>) -> &str {
    #[cfg(windows)]
    const FALLBACK: &str = "notepad";
    #[cfg(not(windows))]
    const FALLBACK: &str = "vi";
    editor
        .filter(|e| should_use_external_editor(Some(e)))
        .unwrap_or(FALLBACK)
}

pub fn split_editor_words(editor: &str) -> Vec<String> {
    shlex::split(editor).unwrap_or_else(|| {
        editor
            .split_whitespace()
            .map(|w| w.trim_matches(['\'', '"']).to_string())
            .collect()
    })
}

pub fn parse_editor_cmd(editor: &str) -> (String, Vec<String>) {
    let mut parts = split_editor_words(editor).into_iter();
    let prog = parts.next().unwrap_or_default();
    (prog, parts.collect())
}

pub fn backup_before_overwrite(path: &Path) -> std::io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let mut backup = path.with_file_name(format!("{name}.bak"));
    for n in 2..100 {
        if !backup.exists() {
            break;
        }
        backup = path.with_file_name(format!("{name}.bak-{n}"));
    }
    std::fs::copy(path, &backup)?;
    Ok(Some(backup))
}

pub fn run_external_editor_with(
    path: &Path,
    initial: &str,
    spawn: impl FnOnce() -> std::io::Result<()>,
) -> anyhow::Result<Option<String>> {
    spawn().map_err(|e| anyhow::anyhow!("failed to launch editor: {e}"))?;
    let after = std::fs::read_to_string(path).unwrap_or_default();
    if after == initial {
        Ok(None)
    } else {
        Ok(Some(after))
    }
}

pub fn run_external_editor(path: &Path, initial: &str) -> anyhow::Result<Option<String>> {
    let editor = std::env::var("EDITOR").ok();
    let (prog, mut args) = parse_editor_cmd(editor_program(editor.as_deref()));
    args.push(path.to_string_lossy().into_owned());
    run_external_editor_with(path, initial, || {
        let status = std::process::Command::new(&prog).args(&args).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("editor exited: {status}")))
        }
    })
}
