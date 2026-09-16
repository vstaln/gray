use super::*;

#[test]
fn miss_error_is_exact() {
    let index = Index {
        plugins: BTreeMap::new(),
    };
    let err = lookup(&index, "foo").unwrap_err().to_string();
    assert_eq!(err, "not in index: foo (try /plugin install <https-url>)");
}

#[test]
fn hash_spec_parses_string_or_map() {
    let s: HashSpec = serde_json::from_str(r#""sha256:abc""#).unwrap();
    assert_eq!(s.primary(), Some("sha256:abc"));
    let m: HashSpec = serde_json::from_str(r#"{"x86_64-linux": "sha256:def"}"#).unwrap();
    assert_eq!(m.primary(), Some("sha256:def"));
}
