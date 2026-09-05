use std::collections::BTreeMap;
use std::sync::Arc;

use gray_plugin::boot::active_plugins;
use gray_plugin::lock::{LockEntry, LockFile, lock_path};
use gray_plugin::{Manifest, Plugin};

// No env (HOME/GRAY_HOME) touched: profile + home are absolute TempDir
// paths, entries are bare builtins — so no ENV_LOCK needed.

struct Stub {
    name: String,
    version: String,
}

#[async_trait::async_trait]
impl Plugin for Stub {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: self.name.clone(),
            version: self.version.clone(),
            ..Default::default()
        }
    }
}

fn stub(name: &str, version: &str) -> Arc<dyn Plugin> {
    Arc::new(Stub {
        name: name.to_string(),
        version: version.to_string(),
    }) as Arc<dyn Plugin>
}

// Builtin key -> canned manifest. Lock entries below use empty argv so
// they resolve here; only the dead-sidecar case spawns (and must fail).
fn resolve(name: &str) -> Option<Arc<dyn Plugin>> {
    match name {
        "prof-a" => Some(stub("alpha", "1.0")),
        "prof-b" => Some(stub("beta", "1.0")),
        "lock-c" => Some(stub("gamma", "1.0")),
        "prof-dup" => Some(stub("dup", "profile")),
        "lock-dup" => Some(stub("dup", "lock")),
        "lock-good" => Some(stub("good", "1.0")),
        _ => None,
    }
}

fn lock_entry(argv: Vec<&str>) -> LockEntry {
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

fn save_lock(home: &std::path::Path, plugins: BTreeMap<String, LockEntry>) {
    LockFile { schema: 1, plugins }
        .save(&lock_path(home))
        .unwrap();
}

fn write_profile(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("gray.yml");
    std::fs::write(&path, body).unwrap();
    path
}

fn manifest_names(plugins: &[Arc<dyn Plugin>]) -> Vec<String> {
    plugins.iter().map(|p| p.manifest().name).collect()
}

#[tokio::test]
async fn profile_entries_come_before_lock_entries() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let profile = write_profile(dir.path(), "plugins:\n  - prof-a\n  - prof-b\n");
    save_lock(
        home.path(),
        BTreeMap::from([("lock-c".to_string(), lock_entry(vec![]))]),
    );

    let (plugins, report) = active_plugins(Some(&profile), home.path(), &resolve)
        .await
        .unwrap();
    assert_eq!(manifest_names(&plugins), ["alpha", "beta", "gamma"]);
    assert!(!report.used_fallback);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[tokio::test]
async fn name_conflict_later_lock_entry_wins() {
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let profile = write_profile(dir.path(), "plugins:\n  - prof-dup\n");
    save_lock(
        home.path(),
        BTreeMap::from([("lock-dup".to_string(), lock_entry(vec![]))]),
    );

    let (plugins, report) = active_plugins(Some(&profile), home.path(), &resolve)
        .await
        .unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].manifest().name, "dup");
    assert_eq!(plugins[0].manifest().version, "lock");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[tokio::test]
async fn dead_lock_sidecar_warns_and_others_still_load() {
    let home = tempfile::tempdir().unwrap();
    save_lock(
        home.path(),
        BTreeMap::from([
            (
                "dead-plugin".to_string(),
                lock_entry(vec!["definitely-not-a-real-binary-xyz"]),
            ),
            ("lock-good".to_string(), lock_entry(vec![])),
        ]),
    );

    let (plugins, report) = active_plugins(None, home.path(), &resolve).await.unwrap();
    assert_eq!(manifest_names(&plugins), ["good"]);
    assert!(!report.used_fallback);
    assert!(
        !report.warnings.is_empty(),
        "expected a warning for the dead sidecar"
    );
    assert!(
        report.warnings.iter().any(|w| w.contains("dead-plugin")),
        "{:?}",
        report.warnings
    );
    assert!(
        report
            .warnings
            .iter()
            .all(|w| !w.contains("definitely-not-a-real-binary-xyz")),
        "lock warnings must not leak argv: {:?}",
        report.warnings
    );
}
