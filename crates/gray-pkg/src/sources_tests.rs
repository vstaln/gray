#![allow(clippy::await_holding_lock)]
use super::*;

#[test]
fn clawhub_slug_split_cases() {
    assert_eq!(
        split_clawhub_slug("arein/test"),
        (Some("arein".to_string()), "test".to_string())
    );
    assert_eq!(split_clawhub_slug("test"), (None, "test".to_string()));
    assert_eq!(split_clawhub_slug("  "), (None, String::new()));
}

#[test]
fn clawhub_canonical_url_is_owner_qualified_or_bare() {
    assert_eq!(
        clawhub_canonical_url("arein", "test"),
        "https://clawhub.ai/arein/skills/test"
    );
    assert_eq!(clawhub_canonical_url("", "test"), "clawhub:test");
}
