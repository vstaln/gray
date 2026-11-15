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

/// Install a provider plugin under `key` whose manifest calls itself
/// `manifest_name` - the shape `gray plugin install` produces when the index
/// key and the plugin's own name disagree.
fn write_renamed_provider_plugin(
    home: &Path,
    key: &str,
    manifest_name: &str,
    provider: &str,
    method: &str,
) {
    std::fs::create_dir_all(home.join("plugins")).unwrap();
    let path = lock_path(home);
    let mut lock = LockFile::load(&path).unwrap_or_default();
    lock.plugins.insert(
        key.to_string(),
        lock_entry(vec![format!("/plugins/{key}/bin/{key}")]),
    );
    lock.save(&path).unwrap();
    let manifest = serde_json::json!({
        "name": manifest_name,
        "capabilities": ["provider.credentials"],
        "providers": [{
            "id": provider,
            "name": "Provider",
            "transport": {
                "kind": "openai-responses",
                "base_url": "https://127.0.0.1:1/",
                "authorization": {"kind": "bearer", "secret_name": "k"}
            },
            "auth_methods": [{
                "id": method,
                "kind": "api_key",
                "name": "Login",
                "operations": ["models", "chat"]
            }]
        }]
    });
    std::fs::write(
        home.join("plugins").join(format!("{key}-manifest.json")),
        manifest.to_string(),
    )
    .unwrap();
    ProviderRegistry::refresh(home).unwrap();
}

fn temp_registry(home: &Path) -> ProviderRegistry {
    ProviderRegistry::load_cached_with_user_home(home, None)
}

#[test]
fn a_renamed_plugin_still_resolves_the_id_a_session_saved() {
    let home = tempfile::tempdir().unwrap();
    write_renamed_provider_plugin(
        home.path(),
        "claude",
        "claude-sub",
        "claude-subscription",
        "claude-login",
    );
    let registry = temp_registry(home.path());
    let stale_id = "claude-sub:claude-subscription";
    let stale_ref = "plugin:claude-sub:claude-subscription:claude-login";

    assert!(
        registry.resolve(stale_id, stale_ref).is_none(),
        "the id from before the rename is not installed"
    );

    let installed = registry
        .resolve_renamed(stale_id, stale_ref)
        .expect("the renamed plugin is adopted");
    assert_eq!(installed.provider_id(), "claude:claude-subscription");
    assert_eq!(
        installed.auth_ref(),
        "plugin:claude:claude-subscription:claude-login"
    );
    assert_eq!(installed.plugin, "claude");
}

#[test]
fn an_installed_id_beats_the_renamed_fallback() {
    let home = tempfile::tempdir().unwrap();
    write_renamed_provider_plugin(
        home.path(),
        "claude",
        "claude-sub",
        "claude-subscription",
        "claude-login",
    );
    write_renamed_provider_plugin(
        home.path(),
        "claude-sub",
        "claude-sub",
        "claude-subscription",
        "claude-login",
    );
    let registry = temp_registry(home.path());

    let installed = registry
        .resolve(
            "claude-sub:claude-subscription",
            "plugin:claude-sub:claude-subscription:claude-login",
        )
        .expect("the exact id still wins");
    assert_eq!(installed.plugin, "claude-sub");
}

#[test]
fn two_plugins_declaring_one_provider_are_ambiguity_not_a_rename() {
    let home = tempfile::tempdir().unwrap();
    write_renamed_provider_plugin(
        home.path(),
        "alpha",
        "alpha",
        "shared-provider",
        "only-method",
    );
    write_renamed_provider_plugin(
        home.path(),
        "beta",
        "beta",
        "shared-provider",
        "only-method",
    );
    let registry = temp_registry(home.path());

    assert!(
        registry
            .resolve_renamed(
                "gamma:shared-provider",
                "plugin:gamma:shared-provider:only-method"
            )
            .is_none()
    );
}

#[test]
fn a_different_auth_method_is_not_a_rename() {
    let home = tempfile::tempdir().unwrap();
    write_renamed_provider_plugin(
        home.path(),
        "claude",
        "claude-sub",
        "claude-subscription",
        "claude-login",
    );
    let registry = temp_registry(home.path());

    assert!(
        registry
            .resolve_renamed(
                "claude-sub:claude-subscription",
                "plugin:claude-sub:claude-subscription:work-login",
            )
            .is_none()
    );
}

#[test]
fn a_mismatched_auth_ref_is_not_a_rename() {
    let home = tempfile::tempdir().unwrap();
    write_renamed_provider_plugin(
        home.path(),
        "claude",
        "claude-sub",
        "claude-subscription",
        "claude-login",
    );
    let registry = temp_registry(home.path());

    assert!(
        registry
            .resolve_renamed(
                "claude-sub:claude-subscription",
                "plugin:devin-sub:claude-subscription:claude-login",
            )
            .is_none()
    );
}

#[test]
fn the_missing_connection_error_names_what_is_installed() {
    let home = tempfile::tempdir().unwrap();
    write_renamed_provider_plugin(
        home.path(),
        "claude",
        "claude-sub",
        "claude-subscription",
        "claude-login",
    );
    let registry = temp_registry(home.path());

    let message = super::super::catalog::not_installed_error(&registry, "gone:claude-subscription")
        .to_string();
    assert!(message.contains("gone:claude-subscription"), "{message}");
    assert!(
        message.contains("installed: claude:claude-subscription"),
        "{message}"
    );
}

#[test]
fn the_missing_connection_error_says_when_nothing_is_installed() {
    let home = tempfile::tempdir().unwrap();
    let registry = temp_registry(home.path());

    let message = super::super::catalog::not_installed_error(&registry, "gone:claude-subscription")
        .to_string();
    assert!(
        message.contains("no plugin provides a connection"),
        "{message}"
    );
}
