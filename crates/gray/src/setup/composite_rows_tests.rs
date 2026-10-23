use super::composite_decls;
use std::path::Path;

fn write_plugin(home: &Path, enabled: bool, ids: &[&str]) {
    let plugins = home.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let lock = serde_json::json!({
        "schema": 1,
        "plugins": { "memo-fuse": {
            "ecosystem": "native", "version": "1", "hash": "", "source": "",
            "argv": [], "adapter_version": "", "installed_at": "", "scope": "user",
            "enabled": enabled,
        }},
    });
    std::fs::write(plugins.join("lock.json"), lock.to_string()).unwrap();
    let rows: Vec<_> = ids
        .iter()
        .map(|id| serde_json::json!({ "id": id, "name": id }))
        .collect();
    let manifest = serde_json::json!({ "name": "memo-fuse", "composite_rows": rows });
    std::fs::write(
        plugins.join("memo-fuse-manifest.json"),
        manifest.to_string(),
    )
    .unwrap();
}

fn fixture_ids(home: &Path) -> Vec<String> {
    composite_decls(home)
        .into_iter()
        .map(|d| d.id)
        .filter(|id| id.starts_with("memo-test/"))
        .collect()
}

#[test]
fn memoized_decls_follow_manifest_and_lock_edits() {
    let home = tempfile::tempdir().unwrap();
    write_plugin(home.path(), true, &["memo-test/a"]);
    assert_eq!(fixture_ids(home.path()), ["memo-test/a"]);
    assert_eq!(fixture_ids(home.path()), ["memo-test/a"], "memo hit");

    write_plugin(home.path(), true, &["memo-test/a", "memo-test/bb"]);
    assert_eq!(
        fixture_ids(home.path()),
        ["memo-test/a", "memo-test/bb"],
        "a rewritten manifest invalidates the memo"
    );

    write_plugin(home.path(), false, &["memo-test/a", "memo-test/bb"]);
    assert!(
        fixture_ids(home.path()).is_empty(),
        "disabling the plugin in the lock invalidates the memo"
    );
}
