use std::collections::BTreeMap;

use gray_plugin::lock::{LockEntry, LockFile, lock_path};

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
