use super::*;

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

#[test]
fn miss_suggests_key_for_repo_name() {
    let entry = |url: &str| -> Entry {
        serde_json::from_value(serde_json::json!({
            "ecosystem": "native", "version": "0.1.0",
            "source": {"type": "tarball", "url": url},
            "hash": "sha256:x"
        }))
        .unwrap()
    };
    let mut plugins = BTreeMap::new();
    plugins.insert(
        "devin".to_string(),
        entry("https://github.com/vstaln/gray-devin-sub/releases/download/v1/gray-devin-1.tar.gz"),
    );
    plugins.insert(
        "discord".to_string(),
        entry("https://github.com/vstaln/gray-discord-plugin/releases/download/v1/x.tar.gz"),
    );
    let index = Index { plugins };
    let err = lookup(&index, "devin-sub").unwrap_err().to_string();
    assert_eq!(err, "not in index: devin-sub — did you mean 'devin'?");
    let err = lookup(&index, "discord-plugin").unwrap_err().to_string();
    assert!(err.contains("'discord'"), "{err}");
    let err = lookup(&index, "zzz").unwrap_err().to_string();
    assert!(err.contains("install by https URL"), "{err}");
}
