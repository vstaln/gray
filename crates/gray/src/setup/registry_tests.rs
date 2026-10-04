use super::*;
use std::fs;

const TEST_DECL: SetupDecl = SetupDecl {
    config_path: ".config/test-app/config.json",
    fields: &[
        SetupField {
            key: "token",
            kind: FieldKind::Required,
            description: "test bot token",
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
            picker: Some(PICKER_CHANNELS),
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
        SetupField {
            key: "gray_home",
            kind: FieldKind::Derived,
            description: "gray home",
            url: None,
            secret: false,
            picker: None,
        },
        SetupField {
            key: "workdir",
            kind: FieldKind::Derived,
            description: "working dir",
            url: None,
            secret: false,
            picker: None,
        },
    ],
    verify: &["test-app", "doctor"],
    post_steps: &["register", "start"],
    service: Some(&["test-app", "run"]),
};

fn write_config(home: &Path, body: &str) {
    let path = home.join(TEST_DECL.config_path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[test]
fn missing_required_keys_are_reported_by_key_only() {
    let tmp = tempfile::tempdir().unwrap();
    let state = TEST_DECL.state(tmp.path());
    assert_eq!(
        state,
        AppSetupState::NeedsSetup(vec!["token", "channel_id"])
    );
    // The diagnostic carries key names, never anything from the file.
    assert!(!format!("{state:?}").contains('/'));
}

#[test]
fn a_written_secret_is_configured_and_never_surfaced() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        r#"{"token": "sk-DEADBEEF-secret", "channel_id": "1544925612823547984"}"#,
    );
    let state = TEST_DECL.state(tmp.path());
    assert_eq!(state, AppSetupState::Configured);
    assert!(!format!("{state:?}").contains("DEADBEEF"));
}

#[test]
fn optional_keys_do_not_block_configured() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), r#"{"token": "t", "channel_id": "1"}"#);
    assert_eq!(TEST_DECL.state(tmp.path()), AppSetupState::Configured);
}

#[test]
fn unreadable_or_invalid_config_counts_as_needs_setup() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "{not json");
    assert_eq!(
        TEST_DECL.state(tmp.path()),
        AppSetupState::NeedsSetup(vec!["token", "channel_id"])
    );
}

#[test]
fn derived_fields_resolve_to_absolute_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let derived = TEST_DECL.derived(&home.join(".gray"), home);
    let get = |k: &str| {
        derived
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("missing derived field {k}"))
    };
    assert!(Path::new(&get("gray_bin")).is_absolute());
    assert_eq!(get("gray_home"), home.join(".gray").to_string_lossy());
    assert_eq!(
        get("workdir"),
        home.join(".config/test-app").to_string_lossy()
    );
}

#[test]
fn field_flags_survive_the_declaration() {
    let channel = TEST_DECL.field("channel_id").expect("channel field");
    assert_eq!(channel.picker, Some(PICKER_CHANNELS));
    assert!(!channel.secret);
    let token = TEST_DECL.field("token").expect("token field");
    assert!(token.secret);
    assert_eq!(token.url, Some("https://example.com/portal"));
    assert_eq!(token.kind, FieldKind::Required);
}

#[test]
fn broken_is_a_distinct_state_the_registry_never_guesses() {
    // A config with every required key is Configured; only a failed verify
    // (the flow's job) may ever mark an app Broken.
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), r#"{"token": "t", "channel_id": "1"}"#);
    assert_ne!(TEST_DECL.state(tmp.path()), AppSetupState::Broken);
}
