use std::collections::BTreeMap;
use std::path::Path;

use gray_plugin::lock::{LockEntry, LockFile, lock_path};

use crate::providers::registry::cache_path;

use super::ProviderRegistry;

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

#[test]
fn provider_only_runtime_role_survives_lock_round_trip() {
    let mut entry = lock_entry(vec!["provider-sidecar".to_string()]);
    entry.runtime_role = Some("provider_only".to_string());
    let mut lock = gray_plugin::lock::LockFile::default();
    lock.plugins = BTreeMap::from([("example-sub".to_string(), entry.clone())]);
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

fn write_user_global_provider(user: &Path, global: bool) {
    std::fs::create_dir_all(user.join("plugins")).unwrap();
    std::fs::write(
        user.join("plugins/global-provider-manifest.json"),
        r#"{"name":"global-provider","providers":[{"id":"p","name":"P","transport":{"kind":"openai-responses","base_url":"https://127.0.0.1:1/","authorization":{"kind":"bearer","secret_name":"k"}},"auth_methods":[{"id":"a","kind":"api_key","name":"A","operations":["models"]}]}]}"#,
    )
    .unwrap();
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "global-provider".to_string(),
        LockEntry {
            global,
            ..lock_entry(vec!["/user/plugins/global-provider/bin/plugin".to_string()])
        },
    );
    lock.save(&lock_path(user)).unwrap();
}

#[test]
fn global_user_home_provider_resolves_from_an_empty_session_home() {
    let user = tempfile::tempdir().unwrap();
    let session = tempfile::tempdir().unwrap();
    let auth_ref = "plugin:global-provider:p:a";

    write_user_global_provider(user.path(), true);
    let registry = ProviderRegistry::load_cached_with_user_home(session.path(), Some(user.path()));
    let installed = registry
        .resolve("global-provider:p", auth_ref)
        .expect("global user-home provider resolves from a session home");
    assert_eq!(
        installed.argv,
        vec!["/user/plugins/global-provider/bin/plugin".to_string()]
    );
    assert_eq!(
        super::plugin_home_with_user(session.path(), Some(user.path()), "global-provider"),
        user.path()
    );

    write_user_global_provider(user.path(), false);
    let registry = ProviderRegistry::load_cached_with_user_home(session.path(), Some(user.path()));
    assert!(registry.resolve("global-provider:p", auth_ref).is_none());
}

#[test]
fn never_asked_plugin_declaring_provider_capability_is_grandfathered() {
    let mut entry = LockEntry {
        granted_capabilities: Vec::new(),
        capabilities_hash: None,
        ..Default::default()
    };
    let declared = vec!["provider.credentials".to_string()];
    assert!(super::provider_granted(&entry, &declared));
    // Not declared → not granted.
    assert!(!super::provider_granted(&entry, &[]));
    // A recorded consent hash does not gate the grant.
    entry.capabilities_hash = Some("h".into());
    assert!(super::provider_granted(&entry, &declared));
    // Explicit grant always counts.
    entry.granted_capabilities = declared.clone();
    assert!(super::provider_granted(&entry, &[]));
}
