use super::*;

/// A tempdir inside the canonicalized temp dir. macOS `/var` is a symlink and
/// the memory store refuses symlinked ancestors, so the bare `tempdir()` that
/// works on Linux fails there — same helper `memory_tests` uses.
fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

fn store_with(
    body: &str,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    crate::memory::MemoryStore,
) {
    let home = tempdir();
    let cwd = tempdir();
    std::fs::create_dir_all(home.path().join("memory")).unwrap();
    std::fs::write(home.path().join("memory/user.md"), body).unwrap();
    let store = crate::memory::MemoryStore::new(home.path(), cwd.path()).unwrap();
    (home, cwd, store)
}

#[test]
fn entries_list_both_scopes_key_first() {
    let (_home, _cwd, store) =
        store_with("- bench-location: /home/vstaln/bench\n- image-format: SVG only\n");
    let entries = store.entries(crate::memory::Scope::User).unwrap();
    assert_eq!(
        entries,
        vec![
            (
                "bench-location".to_string(),
                "/home/vstaln/bench".to_string()
            ),
            ("image-format".to_string(), "SVG only".to_string()),
        ]
    );
    let items = items_for(&store);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "bench-location");
    assert!(items[0].row.contains("user"), "{}", items[0].row);
    assert!(
        items[0].row.contains("/home/vstaln/bench"),
        "{}",
        items[0].row
    );
    // Read-only listing: no switch, no removal.
    assert!(items[0].read_only);
    assert!(!items[0].enabled);
}

#[test]
fn missing_store_reads_as_no_rows() {
    let home = tempdir();
    let cwd = tempdir();
    let store = crate::memory::MemoryStore::new(home.path(), cwd.path()).unwrap();
    assert!(items_for(&store).is_empty());
}

#[test]
fn long_entries_are_truncated_for_the_row() {
    let long = "x".repeat(200);
    let (_home, _cwd, store) = store_with(&format!("- big: {long}\n"));
    let items = items_for(&store);
    assert!(items[0].row.ends_with('\u{2026}'), "{}", items[0].row);
    assert!(items[0].row.chars().count() < 90, "{}", items[0].row);
}

#[test]
fn spec_is_a_read_only_listing() {
    assert_eq!(MEMORY_SPEC.title, "Memory");
    const {
        assert!(!MEMORY_SPEC.supports_toggle);
    }
    const {
        assert!(!MEMORY_SPEC.supports_remove);
    }
    const {
        assert!(!MEMORY_SPEC.errors_tab);
    }
}

#[test]
fn an_unreadable_store_is_reported_not_shown_as_empty() {
    // A missing store is an empty scope; a malformed one is a failure and gets
    // a row of its own instead of a silent "no entries".
    let (_home, _cwd, store) = store_with("this is not a memory line\n");
    let items = items_for(&store);
    // The project scope has no file at all, so it stays empty; the malformed
    // user scope is the one that must not pass as "no entries".
    assert_eq!(
        items.len(),
        1,
        "rows: {:?}",
        items.iter().map(|i| i.row.as_str()).collect::<Vec<_>>()
    );
    assert!(
        items[0].row.starts_with("cannot read user memory"),
        "{}",
        items[0].row
    );
    assert!(items[0].read_only);
}
