use super::*;

#[test]
fn marketplace_metadata_is_optional() {
    let entry: Entry = serde_json::from_value(serde_json::json!({
        "ecosystem": "gray-native", "version": "1.0.0",
        "source": {"type": "tarball", "url": "https://example.com/demo.tgz"},
        "hash": "sha256:abc"
    }))
    .unwrap();
    assert!(entry.description.is_none());
    assert!(entry.homepage.is_none());
    assert!(entry.repo.is_none());
    assert!(entry.commands.is_empty());
}

#[test]
fn miss_error_is_exact() {
    let index = Index {
        plugins: BTreeMap::new(),
    };
    let err = lookup(&index, "foo").unwrap_err().to_string();
    assert_eq!(
        err,
        "not in index: foo (install by https URL or local path instead)"
    );
}

#[test]
fn hash_spec_parses_string_or_map() {
    let s: HashSpec = serde_json::from_str(r#""sha256:abc""#).unwrap();
    assert_eq!(s.primary(), Some("sha256:abc"));
    let m: HashSpec = serde_json::from_str(r#"{"x86_64-linux": "sha256:def"}"#).unwrap();
    assert_eq!(m.primary(), Some("sha256:def"));
}
