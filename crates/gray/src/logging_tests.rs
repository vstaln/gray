use super::*;

#[test]
fn redact_sk_key() {
    assert_eq!(redact("key sk-abcDEF123-_xyz done"), "key [REDACTED] done");
    // Short suffixes are not keys.
    assert_eq!(redact("sk-abc"), "sk-abc");
}

#[test]
fn redact_bearer_token() {
    assert_eq!(
        redact("Authorization: Bearer tok123"),
        "Authorization: Bearer [REDACTED]"
    );
    assert_eq!(redact("Bearer "), "Bearer ");
}

#[test]
fn redact_x_api_key() {
    assert_eq!(redact("x-api-key: secret123"), "x-api-key: [REDACTED]");
    assert_eq!(
        redact(r#""x-api-key": "abc""#),
        r#""x-api-key": "[REDACTED]""#
    );
    assert_eq!(redact("X-API-KEY=zzz"), "X-API-KEY=[REDACTED]");
}

#[test]
fn redact_leaves_plain_text_alone() {
    assert_eq!(redact("hello world, no secrets"), "hello world, no secrets");
    assert_eq!(redact("my x-api-key is secret"), "my x-api-key is secret");
}

#[test]
fn rotation_helper_caps_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("gray.log");
    std::fs::write(&log, vec![b'x'; (10 * 1024 * 1024 + 1) as usize]).unwrap();
    crate::rotation::rotate_if_needed(&log);
    assert!(std::fs::metadata(&log).unwrap().len() < 10 * 1024 * 1024);
}

// UNRUN (cargo test banned under X per AGENTS.md; verify in TTY/CI).
#[test]
fn runtime_rotation_threshold_matches_boot_cap() {
    assert!(!should_rotate(0));
    assert!(!should_rotate(crate::rotation::LOG_MAX_BYTES));
    assert!(should_rotate(crate::rotation::LOG_MAX_BYTES + 1));
}
