use std::collections::BTreeMap;
use std::path::Path;

use gray_plugin::lock::{LockEntry, LockFile, lock_path};

use crate::providers::registry::cache_path;

use super::{ProviderRegistry, refresh_plugin};

fn registry_argv() -> Vec<String> {
    vec!["testdata/provider_registry_plugin.sh".to_string()]
}

fn lock_entry(argv: Vec<String>) -> LockEntry {
    LockEntry {
        runtime_role: None,
        ecosystem: "gray-native".to_string(),
        version: "0.1.0".to_string(),
        hash: "sha256:test".to_string(),
        source: "fixture".to_string(),
        argv,
        adapter_version: "1.2".to_string(),
        installed_at: "0".to_string(),
        scope: "user".to_string(),
        enabled: true,
        granted_capabilities: vec!["provider.credentials".to_string()],
        capabilities_hash: Some("test".to_string()),
        ..Default::default()
    }
}

fn write_lock(home: &Path, entry: LockEntry) -> anyhow::Result<()> {
    let path = lock_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut lock = LockFile::load(&path)?;
    lock.plugins.insert("provider-registry".to_string(), entry);
    lock.save(&path)
}

#[tokio::test]
async fn malformed_peer_is_omitted_without_hiding_valid_provider() {
    let home = tempfile::tempdir().unwrap();
    write_lock(home.path(), lock_entry(registry_argv())).unwrap();
    let cache = refresh_plugin(home.path(), &lock_entry(registry_argv()))
        .await
        .unwrap();
    assert_eq!(cache.plugins["provider-registry"].providers.len(), 1);
    assert_eq!(
        cache.plugins["provider-registry"].providers[0].id,
        "provider-good"
    );
    assert_eq!(cache.plugins["provider-registry"].errors.len(), 1);
}

#[tokio::test]
async fn disabled_entry_hides_cached_provider() {
    let home = tempfile::tempdir().unwrap();
    let entry = lock_entry(registry_argv());
    write_lock(home.path(), entry.clone()).unwrap();
    refresh_plugin(home.path(), &entry).await.unwrap();
    let auth_ref = "plugin:provider-registry:provider-good:example-login";
    let registry = ProviderRegistry::load_cached(home.path());
    let installed = registry
        .resolve("provider-registry:provider-good", auth_ref)
        .expect("provider resolves while enabled");
    assert_eq!(installed.auth_ref(), auth_ref);

    let mut disabled = entry;
    disabled.enabled = false;
    write_lock(home.path(), disabled.clone()).unwrap();
    refresh_plugin(home.path(), &disabled).await.unwrap();
    let registry = ProviderRegistry::load_cached(home.path());
    assert!(
        registry
            .resolve("provider-registry:provider-good", auth_ref)
            .is_none()
    );
}

#[test]
fn provider_only_runtime_role_survives_lock_round_trip() {
    let mut entry = lock_entry(vec!["provider-sidecar".to_string()]);
    entry.runtime_role = Some("provider_only".to_string());
    let lock = gray_plugin::lock::LockFile {
        schema: 1,
        plugins: BTreeMap::from([("example-sub".to_string(), entry.clone())]),
    };
    let dir = tempfile::tempdir().unwrap();
    let path = gray_plugin::lock::lock_path(dir.path());
    lock.save(&path).unwrap();
    let loaded = gray_plugin::lock::LockFile::load(&path).unwrap();
    assert_eq!(
        loaded.plugins["example-sub"].runtime_role.as_deref(),
        Some("provider_only")
    );
}

#[test]
fn provider_cache_accepts_a_manifest_named_differently_from_the_lock_key() {
    // Index installs adopt the CLI manifest of a plugin published under a
    // different registry key (the `-sub` plugins); the `<key>-manifest.json`
    // filename is what binds the manifest to the lock entry.
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("plugins")).unwrap();
    write_lock(home.path(), lock_entry(vec!["demo-exe".to_string()])).unwrap();
    std::fs::write(
        home.path().join("plugins/provider-registry-manifest.json"),
        r#"{"name":"demo-sub","providers":[{"id":"p","name":"P","transport":{"kind":"openai-responses","base_url":"https://127.0.0.1:1/","authorization":{"kind":"bearer","secret_name":"k"}},"auth_methods":[{"id":"a","kind":"api_key","name":"A","operations":["models"]}]}]}"#,
    )
    .unwrap();

    crate::providers::ProviderRegistry::refresh(home.path()).unwrap();

    let cache = crate::providers::ProviderCache::load(&cache_path(home.path()));
    assert_eq!(cache.plugins["provider-registry"].providers.len(), 1);
}

#[test]
fn provider_cache_shrinks_entries_for_removed_provider_plugins() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("plugins")).unwrap();
    let mut cache = crate::providers::ProviderCache::default();
    cache.plugins.insert(
        "ghost".to_string(),
        crate::providers::registry::CachedProviderPlugin::default(),
    );
    cache.save(&cache_path(home.path())).unwrap();

    crate::providers::ProviderRegistry::refresh(home.path()).unwrap();

    let reloaded = crate::providers::ProviderCache::load(&cache_path(home.path()));
    assert!(reloaded.plugins.is_empty());
}

#[tokio::test]
async fn resolved_provider_carries_lock_spawn_argv() {
    let home = tempfile::tempdir().unwrap();
    let argv = registry_argv();
    let entry = lock_entry(argv.clone());
    write_lock(home.path(), entry.clone()).unwrap();
    refresh_plugin(home.path(), &entry).await.unwrap();
    let registry = ProviderRegistry::load_cached(home.path());
    let installed = registry
        .resolve(
            "provider-registry:provider-good",
            "plugin:provider-registry:provider-good:example-login",
        )
        .expect("provider resolves");
    assert_eq!(installed.argv, argv);
    assert!(!installed.argv.is_empty());
}
