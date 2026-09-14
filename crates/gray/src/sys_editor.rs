//! External `$EDITOR` launch helpers for the Gray system prompt.

use std::path::{Path, PathBuf};

/// Whether `$EDITOR` requests the external editor: set and non-blank.
/// Pure (takes the env value) so unit tests never spawn a real editor.
pub fn should_use_external_editor(editor: Option<&str>) -> bool {
    editor.is_some_and(|s| !s.trim().is_empty())
}

/// `$EDITOR` when set and non-blank, else `vi`.
fn editor_program(editor: Option<&str>) -> &str {
    editor
        .filter(|e| should_use_external_editor(Some(e)))
        .unwrap_or("vi")
}

/// Splits `$EDITOR` into program + args on whitespace; the caller appends
/// the file path (git-style `$EDITOR <file>`).
pub fn parse_editor_cmd(editor: &str) -> (String, Vec<String>) {
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or_default().to_string();
    (prog, parts.map(str::to_string).collect())
}

/// Copies `path` to `<name>.bak` (`<name>.bak-2`… on collision — same `-N`
/// dedup loop as feedback save). `Ok(None)` when there is nothing to back
/// up. Call BEFORE overwriting.
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

/// Runs the external editor via `spawn` (the real `$EDITOR` wait in
/// production, a file-writing stub in tests) and reports the outcome:
/// `Ok(Some(new_body))` when saved, `Ok(None)` when unchanged.
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

/// Git-style `$EDITOR <file>` + wait, then saved/unchanged detection.
/// `$EDITOR` unset or blank falls back to `vi`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_editor_decision() {
        assert!(!should_use_external_editor(None));
        assert!(!should_use_external_editor(Some("")));
        assert!(!should_use_external_editor(Some("   ")));
        assert!(should_use_external_editor(Some("vim")));
        assert!(should_use_external_editor(Some("code --wait")));
    }

    #[test]
    fn editor_defaults_to_vi_when_unset_or_blank() {
        assert_eq!(editor_program(None), "vi");
        assert_eq!(editor_program(Some("")), "vi");
        assert_eq!(editor_program(Some("   ")), "vi");
        assert_eq!(editor_program(Some("vim")), "vim");
        assert_eq!(editor_program(Some("code --wait")), "code --wait");
    }

    #[test]
    fn editor_cmd_splits_program_and_args() {
        assert_eq!(
            parse_editor_cmd("vim"),
            ("vim".to_string(), Vec::<String>::new())
        );
        assert_eq!(
            parse_editor_cmd("code --wait"),
            ("code".to_string(), vec!["--wait".to_string()])
        );
    }

    #[test]
    fn external_edit_stub_reports_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "old").unwrap();
        let res = run_external_editor_with(&path, "old", || {
            std::fs::write(&path, "new").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(res.as_deref(), Some("new"));
    }

    #[test]
    fn external_edit_stub_reports_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "same").unwrap();
        let res = run_external_editor_with(&path, "same", || Ok(())).unwrap();
        assert_eq!(res, None);
    }

    #[test]
    fn reset_backup_copies_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "custom").unwrap();
        let backup = backup_before_overwrite(&path).unwrap().unwrap();
        assert_eq!(
            backup.file_name().unwrap().to_string_lossy(),
            "AGENTS.md.bak"
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "custom");
    }

    #[test]
    fn reset_backup_dedups_when_bak_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "custom").unwrap();
        let first = backup_before_overwrite(&path).unwrap().unwrap();
        assert!(first.exists());
        let second = backup_before_overwrite(&path).unwrap().unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "custom");
    }

    #[test]
    fn reset_backup_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        assert_eq!(backup_before_overwrite(&path).unwrap(), None);
    }
}
