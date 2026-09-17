use super::*;

/// Point `GRAY_HOME` at a fresh tempdir. Must be called under the
/// shared `ops::tests::ENV_GUARD` (one process, one process-global env).
fn use_errors_env() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    home
}

#[test]
fn record_list_round_trip() {
    let _guard = crate::ops::tests::env_guard();
    let _home = use_errors_env();
    record("index", "demo", "boom".to_string());
    let entries = list();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].source, "index");
    assert_eq!(entries[0].item, "demo");
    assert_eq!(entries[0].message, "boom");
}

#[test]
fn cap_evicts_oldest() {
    let _guard = crate::ops::tests::env_guard();
    let _home = use_errors_env();
    for i in 0..105 {
        record("s", &format!("item-{i}"), format!("m{i}"));
    }
    let entries = list();
    assert_eq!(entries.len(), 100);
    assert_eq!(entries[0].item, "item-104");
    assert_eq!(entries.last().unwrap().item, "item-5");
    assert!(!entries.iter().any(|e| e.item == "item-0"));
}

#[test]
fn clear_empties_registry() {
    let _guard = crate::ops::tests::env_guard();
    let _home = use_errors_env();
    record("s", "i", "m".to_string());
    assert_eq!(list().len(), 1);
    clear();
    assert!(list().is_empty());
}

#[test]
fn corrupt_file_returns_empty_and_heals_on_record() {
    let _guard = crate::ops::tests::env_guard();
    let _home = use_errors_env();
    std::fs::create_dir_all(crate::gray_home()).unwrap();
    std::fs::write(crate::gray_home().join("errors.json"), "{not json").unwrap();
    assert!(list().is_empty());
    record("s", "i", "m".to_string());
    assert_eq!(list().len(), 1);
}
