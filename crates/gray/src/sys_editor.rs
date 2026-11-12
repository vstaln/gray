use std::path::{Path, PathBuf};

fn editor_program(editor: Option<&str>) -> &str {
    #[cfg(windows)]
    const FALLBACK: &str = "notepad";
    #[cfg(not(windows))]
    const FALLBACK: &str = "vi";
    editor.filter(|e| !e.trim().is_empty()).unwrap_or(FALLBACK)
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

pub fn run_external_editor(path: &Path, initial: &str) -> anyhow::Result<Option<String>> {
    let editor = std::env::var("EDITOR").ok();
    let program = editor_program(editor.as_deref());
    let mut words = shlex::split(program)
        .unwrap_or_else(|| {
            program
                .split_whitespace()
                .map(|w| w.trim_matches(['\'', '"']).to_string())
                .collect()
        })
        .into_iter();
    let prog = words.next().unwrap_or_default();
    let mut args: Vec<String> = words.collect();
    args.push(path.to_string_lossy().into_owned());

    let status = std::process::Command::new(&prog)
        .args(&args)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to launch editor: {e}"))?;
    if !status.success() {
        anyhow::bail!("failed to launch editor: editor exited: {status}");
    }
    let after = std::fs::read_to_string(path).unwrap_or_default();
    if after == initial {
        Ok(None)
    } else {
        Ok(Some(after))
    }
}
