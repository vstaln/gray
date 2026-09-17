use super::*;

fn setup() -> (tempfile::TempDir, MemoryStore) {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = MemoryStore::new(dir.path(), dir.path()).unwrap();
    (dir, store)
}

#[test]
fn byte_budget_is_exact_and_failed_update_preserves_bytes() {
    let (_dir, store) = setup();
    // '- k: ' and final newline take 6 bytes.
    let text = "é".repeat((Scope::User.limit() - 6) / 2);
    store.set(Scope::User, "k", &text).unwrap();
    let path = store.path(Scope::User);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(before.len(), Scope::User.limit());
    assert_eq!(before.last(), Some(&b'\n'));
    assert!(store.set(Scope::User, "k", &(text + "x")).is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn malformed_files_are_never_overwritten() {
    let (_dir, store) = setup();
    private_dir(&store.root).unwrap();
    let path = store.path(Scope::Project);
    for bad in [
        b"not our format\n".to_vec(),
        b"- k: a\n- k: b\n".to_vec(),
        vec![b'x'; 4097],
        vec![255],
    ] {
        std::fs::write(&path, &bad).unwrap();
        assert!(store.list(Scope::Project).is_err());
        assert!(store.set(Scope::Project, "k", "Good fact.").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bad);
    }
}

#[test]
fn correction_can_match_another_entry_without_keeping_stale_fact() {
    let (_dir, store) = setup();
    store.set(Scope::User, "a", "Old.").unwrap();
    store.set(Scope::User, "b", "New.").unwrap();
    store.set(Scope::User, "a", "New.").unwrap();
    assert!(!store.list(Scope::User).unwrap().contains("Old."));
    assert!(!store.set(Scope::User, "c", "New.").unwrap());
}

#[test]
fn text_validation_preserves_prose_and_paths_but_rejects_known_secrets() {
    for good in [
        "Use Rust.",
        "Project at /home/user/app uses SQLite.",
        "API key rotation is monthly.",
        "喜欢简短回答。",
    ] {
        validate_text(good).unwrap();
    }
    for bad in [
        "",
        "\n",
        "a\r\nb",
        "a\u{200b}b",
        "a\u{202e}b",
        "TOKEN=12345678901234567890",
        "sk-fake12345678901234567890",
    ] {
        assert!(validate_text(bad).is_err(), "accepted invalid input");
    }
}

#[cfg(unix)]
#[test]
fn private_modes_and_symlink_rejection() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (dir, store) = setup();
    store.set(Scope::User, "style", "Concise.").unwrap();
    assert_eq!(
        std::fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let path = store.path(Scope::User);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let outside = dir.path().join("outside");
    std::fs::write(&outside, "untouched").unwrap();
    std::fs::remove_file(&path).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(store.list(Scope::User).is_err());
    assert!(store.set(Scope::User, "style", "Changed.").is_err());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "untouched");
    std::fs::remove_file(&path).unwrap();
    let lock_path = path.with_extension("lock");
    std::fs::remove_file(&lock_path).unwrap();
    symlink(dir.path().join("absent"), lock_path).unwrap();
    assert!(store.set(Scope::User, "style", "Changed.").is_err());
}

#[test]
fn snapshot_is_frozen_on_rebuild_and_resume_but_new_session_gets_updates() {
    let (_dir, store) = setup();
    store.set(Scope::User, "style", "Short.").unwrap();
    let sid = uuid::Uuid::new_v4().to_string();
    let original = store.snapshot(Some(&sid)).unwrap();
    store.set(Scope::User, "style", "Detailed.").unwrap();
    assert_eq!(store.snapshot(Some(&sid)).unwrap(), original);
    let resumed =
        MemoryStore::new(store.root.parent().unwrap(), store.root.parent().unwrap()).unwrap();
    assert_eq!(resumed.snapshot(Some(&sid)).unwrap(), original);
    assert!(
        store
            .snapshot(Some(&uuid::Uuid::new_v4().to_string()))
            .unwrap()
            .contains("Detailed.")
    );
    assert!(store.snapshot(None).unwrap().contains("Detailed."));
    assert!(!original.contains("Detailed."));
}

#[test]
fn invalid_snapshot_and_wrong_project_fail_closed() {
    let (dir, store) = setup();
    assert!(store.snapshot(Some("../escape")).is_err());
    let sid = uuid::Uuid::new_v4().to_string();
    store.snapshot(Some(&sid)).unwrap();
    let other = dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let other_store = MemoryStore::new(dir.path(), &other).unwrap();
    assert!(other_store.snapshot(Some(&sid)).is_err());
    let path = store.root.join("snapshots").join(format!("{sid}.json"));
    std::fs::write(path, "not json").unwrap();
    assert!(store.snapshot(Some(&sid)).is_err());
}
