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
