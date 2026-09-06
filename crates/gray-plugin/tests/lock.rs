use std::collections::BTreeMap;

use gray_plugin::lock::{
    LockEntry, LockFile, disabled_sidecar_argvs, load_disabled_sidecar_argvs, lock_path,
    project_lock_path,
};

fn entry(argv: Vec<&str>) -> LockEntry {
    LockEntry {
        ecosystem: "test".to_string(),
        version: "1.0.0".to_string(),
        hash: "abc123".to_string(),
        source: "test-source".to_string(),
        argv: argv.into_iter().map(str::to_string).collect(),
        adapter_version: "1".to_string(),
        installed_at: "2026-09-05T00:00:00Z".to_string(),
        scope: "test".to_string(),
        enabled: true,
    }
}

fn lock_file() -> LockFile {
    LockFile {
        schema: 1,
        plugins: BTreeMap::from([
            ("alpha".to_string(), entry(vec![])),
            ("beta".to_string(), entry(vec!["my-plugin"])),
        ]),
    }
}

#[test]
fn round_trip_preserves_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = lock_path(dir.path());
    let expected = lock_file();
    expected.save(&path).unwrap();
    assert_eq!(LockFile::load(&path).unwrap(), expected);
}

#[test]
fn corrupt_file_is_err_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = lock_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "not json {{{").unwrap();
    assert!(LockFile::load(&path).is_err());
}

#[test]
fn missing_file_is_empty_schema_1() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = LockFile::load(&lock_path(dir.path())).unwrap();
    assert_eq!(loaded.schema, 1);
    assert!(loaded.plugins.is_empty());
}

#[test]
fn old_lock_without_enabled_loads_as_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = lock_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"schema":1,"plugins":{"alpha":{"ecosystem":"t","version":"1","hash":"h","source":"s","argv":[],"adapter_version":"1","installed_at":"t","scope":"t"}}}"#,
    )
    .unwrap();
    let loaded = LockFile::load(&path).unwrap();
    assert!(loaded.plugins["alpha"].enabled);
}

fn named(name: &str, argv: Vec<&str>, enabled: bool) -> (String, LockEntry) {
    let mut e = entry(argv);
    e.enabled = enabled;
    (name.to_string(), e)
}

fn user_lock() -> LockFile {
    LockFile {
        schema: 1,
        plugins: BTreeMap::from([
            named("on-sidecar", vec!["sidecar-bin"], true),
            named("off-sidecar", vec!["dead-bin"], false),
            named("off-builtin", vec![], false),
        ]),
    }
}

#[test]
fn disabled_argvs_come_from_user_lock() {
    let project = LockFile {
        schema: 1,
        plugins: BTreeMap::new(),
    };
    assert_eq!(
        disabled_sidecar_argvs(&user_lock(), &project),
        vec![vec!["dead-bin".to_string()]]
    );
}

#[test]
fn project_overlay_wins_on_the_flag_only() {
    // Project disables an enabled user entry (by its argv).
    let project = LockFile {
        schema: 1,
        plugins: BTreeMap::from([named("on-sidecar", vec!["other-bin"], false)]),
    };
    assert_eq!(
        disabled_sidecar_argvs(&user_lock(), &project),
        vec![
            vec!["dead-bin".to_string()],
            vec!["sidecar-bin".to_string()]
        ]
    );
    // Project re-enables a user-disabled entry.
    let project = LockFile {
        schema: 1,
        plugins: BTreeMap::from([named("off-sidecar", vec![], true)]),
    };
    assert!(disabled_sidecar_argvs(&user_lock(), &project).is_empty());
    // Project-only names have no known argv: the overlay toggles, it never
    // discovers.
    let project = LockFile {
        schema: 1,
        plugins: BTreeMap::from([named("ghost", vec!["ghost-bin"], false)]),
    };
    assert_eq!(
        disabled_sidecar_argvs(&user_lock(), &project),
        vec![vec!["dead-bin".to_string()]]
    );
}

#[test]
fn load_disabled_argvs_reads_both_files() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Missing files: empty, no warnings.
    let (disabled, warnings) = load_disabled_sidecar_argvs(Some(home.path()), cwd.path());
    assert!(disabled.is_empty());
    assert!(warnings.is_empty(), "{warnings:?}");
    // User lock disables one sidecar; project overlay disables another.
    user_lock().save(&lock_path(home.path())).unwrap();
    LockFile {
        schema: 1,
        plugins: BTreeMap::from([named("on-sidecar", vec![], false)]),
    }
    .save(&project_lock_path(cwd.path()))
    .unwrap();
    let (disabled, warnings) = load_disabled_sidecar_argvs(Some(home.path()), cwd.path());
    assert_eq!(
        disabled,
        vec![
            vec!["dead-bin".to_string()],
            vec!["sidecar-bin".to_string()]
        ]
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    // Corrupt user lock: warning; its argv lists are lost so the project
    // overlay has nothing to match against.
    std::fs::write(lock_path(home.path()), "not json {{{").unwrap();
    let (disabled, warnings) = load_disabled_sidecar_argvs(Some(home.path()), cwd.path());
    assert!(disabled.is_empty());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("lock.json"), "{warnings:?}");
}
