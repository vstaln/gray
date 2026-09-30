use super::*;
use crate::setup::registry::{FieldKind, SetupField};
use std::fs;

const DECL: SetupDecl = SetupDecl {
    config_path: ".config/test-app/config.json",
    fields: &[
        SetupField {
            key: "token",
            kind: FieldKind::Required,
            description: "the bot token",
            url: Some("https://example.com/portal"),
            secret: true,
            picker: None,
        },
        SetupField {
            key: "channel_id",
            kind: FieldKind::Required,
            description: "the home channel",
            url: None,
            secret: false,
            picker: None,
        },
        SetupField {
            key: "allowed_users",
            kind: FieldKind::Optional,
            description: "extra users",
            url: None,
            secret: false,
            picker: None,
        },
        SetupField {
            key: "gray_bin",
            kind: FieldKind::Derived,
            description: "gray binary",
            url: None,
            secret: false,
            picker: None,
        },
    ],
    verify: &["test-app", "doctor"],
    post_steps: &["register", "start"],
    service: Some(&["test-app", "run"]),
};

fn flags(items: &[(&str, &str)]) -> Vec<String> {
    items
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .to_vec()
}

#[test]
fn plan_missing_lists_undanswered_fields_in_declaration_order() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = plan_missing(&DECL, tmp.path());
    let keys: Vec<&str> = missing.iter().map(|f| f.key).collect();
    assert_eq!(keys, ["token", "channel_id", "allowed_users"]);

    // Answering one in the config shortens the plan.
    let path = tmp.path().join(DECL.config_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, r#"{"token": "sk-x"}"#).unwrap();
    let keys: Vec<&str> = plan_missing(&DECL, tmp.path())
        .iter()
        .map(|f| f.key)
        .collect();
    assert_eq!(keys, ["channel_id", "allowed_users"]);
}

#[test]
fn supplied_from_flags_parses_marks_secrets_and_rejects_unknown() {
    let supplied = supplied_from_flags(
        &DECL,
        &flags(&[("token", "sk-DEADBEEF"), ("channel_id", "123")]),
    )
    .unwrap();
    assert_eq!(supplied.get("token"), Some("sk-DEADBEEF"));
    assert!(!format!("{supplied:?}").contains("DEADBEEF"));

    let unknown = supplied_from_flags(&DECL, &flags(&[("nope", "x")]))
        .err()
        .expect("an unknown key must fail");
    assert!(
        unknown.to_string().contains("not a setup field"),
        "{unknown}"
    );

    let bad = supplied_from_flags(&DECL, &["token".to_string()])
        .err()
        .expect("a bare key must fail");
    assert!(bad.to_string().contains("key=value"), "{bad}");

    let derived = supplied_from_flags(&DECL, &flags(&[("gray_bin", "/x")]))
        .err()
        .expect("a derived field must be refused");
    assert!(derived.to_string().contains("derived"), "{derived}");
}

#[test]
fn missing_required_stops_the_flow_before_any_write() {
    let tmp = tempfile::tempdir().unwrap();
    let absent = tmp.path().join("config.json");
    let empty = Supplied::default();
    let missing = missing_required(&DECL, &empty, &absent);
    let keys: Vec<&str> = missing.iter().map(|f| f.key).collect();
    assert_eq!(keys, ["token", "channel_id"]);
    let described = describe_missing(&missing);
    assert!(described.contains("get it at https://example.com/portal"));
    assert!(!described.contains("allowed_users"), "{described}");

    let mut partial = Supplied::default();
    partial.insert("token", "sk-x".to_string(), true);
    let keys: Vec<&str> = missing_required(&DECL, &partial, &absent)
        .iter()
        .map(|f| f.key)
        .collect();
    assert_eq!(keys, ["channel_id"]);
}

#[test]
fn an_existing_config_answers_the_required_fields() {
    // A hand-edited or partially-set-up config means those keys are already
    // answered: the flow proceeds to verify instead of demanding flags that
    // the file already carries. Values are never read here, only presence.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    std::fs::write(&path, r#"{"token": "sk-x", "channel_id": "42"}"#).unwrap();
    let missing = missing_required(&DECL, &Supplied::default(), &path);
    assert!(
        missing.is_empty(),
        "{:?}",
        missing.iter().map(|f| f.key).collect::<Vec<_>>()
    );

    // One key still absent still blocks, by key name only.
    std::fs::write(&path, r#"{"token": "sk-x"}"#).unwrap();
    let keys: Vec<&str> = missing_required(&DECL, &Supplied::default(), &path)
        .iter()
        .map(|f| f.key)
        .collect();
    assert_eq!(keys, ["channel_id"]);
}

#[test]
fn run_step_reports_success_and_real_output() {
    assert!(run_step(&["true".to_string()]).ok);
    assert!(!run_step(&["false".to_string()]).ok);
    // PATH lookup, not /bin: the true/false asserts above already prove the
    // Windows runners carry Git Bash coreutils, and /bin is not a path there.
    let echo = run_step(&["echo".to_string(), "hi".to_string()]);
    assert!(echo.ok);
    assert_eq!(echo.output.trim(), "hi");
    let missing_bin = run_step(&["/nonexistent/gray-test-bin".to_string()]);
    assert!(!missing_bin.ok);
    assert!(missing_bin.output.contains("could not run"));
}

#[test]
fn verify_argv_resolves_the_app_binary_through_the_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dir = home.join("plugins");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("commands.json"),
        r#"{"schema": 1, "plugins": {"test-app": {"ecosystem": "gray-native", "version": "0.0.0", "hash": "", "source": "/bin/echo", "argv": ["/bin/echo", "registered"], "adapter_version": "1.1", "installed_at": "2026-09-22T00:00:00+00:00", "scope": "user", "enabled": true}}}"#,
    )
    .unwrap();
    let argv = verify_argv("test-app", home, &DECL).unwrap();
    assert_eq!(argv, ["/bin/echo", "registered", "doctor"]);
    let unknown = verify_argv("nope", home, &DECL)
        .err()
        .expect("an unregistered app must fail");
    assert!(
        unknown.to_string().contains("no plugin command"),
        "{unknown}"
    );
}

#[test]
fn register_step_is_skipped_when_already_registered() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("plugins");
    fs::create_dir_all(&dir).unwrap();
    assert!(needs_registration(&DECL, home.path(), "test-app"));
    fs::write(
        dir.join("commands.json"),
        r#"{"schema": 1, "plugins": {"test-app": {"ecosystem": "gray-native", "version": "0.0.0", "hash": "", "source": "/bin/echo", "argv": ["/bin/echo", "registered"], "adapter_version": "1.1", "installed_at": "2026-09-22T00:00:00+00:00", "scope": "user", "enabled": true}}}"#,
    )
    .unwrap();
    assert!(!needs_registration(&DECL, home.path(), "test-app"));
    assert!(needs_registration(&DECL, home.path(), "other"));
}
