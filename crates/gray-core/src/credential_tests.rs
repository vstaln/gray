use std::collections::BTreeMap;

use super::credential::{CredentialEnvelope, CredentialMaterial, SecretMap};

#[test]
fn credential_material_round_trips_secret_values_without_debug_exposure() {
    let value = CredentialMaterial {
        secrets: SecretMap::from_iter([
            ("access_token", "test-access"),
            ("refresh_token", "test-refresh"),
        ]),
        metadata: BTreeMap::from([("account_id".into(), "acct_test".into())]),
        expires_at: Some(456),
    };
    let encoded = serde_json::to_string(&value).unwrap();
    let decoded: CredentialMaterial = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.secrets.get("access_token"), Some("test-access"));
    assert_eq!(decoded.secrets.get("refresh_token"), Some("test-refresh"));
    let debug = format!("{decoded:?}");
    assert!(!debug.contains("test-access"));
    assert!(!debug.contains("acct_test"));
    assert!(debug.contains("redacted"));
}

#[test]
fn envelope_rejects_oversized_credential_payload() {
    let material = CredentialMaterial {
        secrets: super::credential::SecretMap::from_iter([(
            "access_token",
            "x".repeat(super::credential::MAX_CREDENTIAL_BYTES),
        )]),
        ..CredentialMaterial::default()
    };
    assert!(CredentialEnvelope::new("p", "d", "a", "b", material).is_err());
}
