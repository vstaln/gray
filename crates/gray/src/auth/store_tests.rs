use std::collections::BTreeMap;

use gray_core::credential::{CredentialEnvelope, CredentialMaterial, SecretMap};

use super::CredentialStore;
use crate::setup::catalog::{AuthEntry, StoredAuth};

fn envelope(plugin: &str, binding: &str) -> CredentialEnvelope {
    CredentialEnvelope::new(
        plugin,
        "example",
        "example-login",
        binding,
        CredentialMaterial {
            secrets: SecretMap::from_iter([("access_token", "test-access")]),
            metadata: BTreeMap::from([("account_id".into(), "acct_test".into())]),
            expires_at: Some(4102444800),
        },
    )
    .unwrap()
}

#[test]
fn plugin_write_preserves_keys_and_legacy_oauth() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    let mut store = BTreeMap::new();
    store.insert("openai".into(), AuthEntry::Key("test-key".into()));
    store.insert(
        "legacy".into(),
        AuthEntry::OAuth(StoredAuth {
            provider: "legacy".into(),
            access_token: "legacy-access".into(),
            refresh_token: String::new(),
            expires_at: 1,
            email: None,
        }),
    );
    let credential_store = CredentialStore::new(path);
    credential_store.replace(&store).unwrap();
    let next = credential_store
        .put_plugin(envelope("example-sub", "sha256:test"))
        .unwrap();
    assert!(matches!(next.get("openai"), Some(AuthEntry::Key(k)) if k == "test-key"));
    assert!(matches!(next.get("legacy"), Some(AuthEntry::OAuth(_))));
    assert!(matches!(
        next.get("plugin:example-sub:example:example-login"),
        Some(AuthEntry::Plugin(_))
    ));
}

#[test]
fn plugin_crud_is_namespaced_and_bound() {
    let dir = tempfile::tempdir().unwrap();
    let store = CredentialStore::new(dir.path().join("auth.json"));
    store
        .put_plugin(envelope("example-sub", "sha256:one"))
        .unwrap();
    assert!(
        store
            .read_plugin("plugin:example-sub:example:example-login")
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .replace_plugin_if_bound(
                "plugin:example-sub:example:example-login",
                "sha256:other",
                CredentialMaterial::empty(),
            )
            .is_err()
    );
    let next = CredentialMaterial {
        secrets: SecretMap::from_iter([("access_token", "rotated")]),
        ..CredentialMaterial::default()
    };
    store
        .replace_plugin_if_bound(
            "plugin:example-sub:example:example-login",
            "sha256:one",
            next,
        )
        .unwrap();
    assert_eq!(
        store
            .read_plugin("plugin:example-sub:example:example-login")
            .unwrap()
            .unwrap()
            .credential
            .secrets
            .get("access_token"),
        Some("rotated")
    );
    assert_eq!(store.remove_plugin_owner("example-sub").unwrap(), 1);
    assert!(
        store
            .read_plugin("plugin:example-sub:example:example-login")
            .unwrap()
            .is_none()
    );
}

#[test]
fn malformed_store_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "{\"unknown\": 12}").unwrap();
    assert!(CredentialStore::new(path).load().is_err());
}

/// Two gray processes (two terminals, or a REPL plus a cron fire) updating
/// `auth.json` at once. The lock used to cover only the final write, so the
/// second writer's stale map erased the first one's credential.
#[test]
fn concurrent_plugin_writes_keep_every_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    std::thread::scope(|scope| {
        for i in 0..8 {
            let path = path.clone();
            scope.spawn(move || {
                CredentialStore::new(path)
                    .put_plugin(envelope(&format!("plugin{i}"), "binding"))
                    .unwrap();
            });
        }
    });
    let store = CredentialStore::new(path).load().unwrap();
    assert_eq!(store.len(), 8, "a concurrent write dropped credentials");
}
