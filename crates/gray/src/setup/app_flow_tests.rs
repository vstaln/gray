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
    check: None,
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

// --- Discord onboarding: the token is checked before anything is written.

use crate::setup::discord_check::{BotCheck, CheckError, INVITE_PERMISSIONS};
use std::cell::{Cell, RefCell};

fn bot(message_content: bool) -> BotCheck {
    BotCheck {
        app_id: "42".to_string(),
        bot_name: "graybot".to_string(),
        owners: vec![("1".to_string(), "alice".to_string())],
        message_content,
        server_members: false,
        server_count: Some(0),
    }
}

fn token_flag(value: &str) -> Supplied {
    let mut s = Supplied::default();
    s.insert("token", value.to_string(), true);
    s
}

fn discord_config(tmp: &Path, body: &str) -> std::path::PathBuf {
    let path = tmp.join("config.json");
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn a_rejected_token_is_reasked_then_given_up_and_never_saved() {
    let calls = Cell::new(0);
    let reject = |_: &str| {
        calls.set(calls.get() + 1);
        Err(CheckError::Rejected)
    };
    let mut gate = TokenGate::default();
    for _ in 0..2 {
        let (_, verdict) = gate.submit("not.the.token", &reject);
        let TokenVerdict::Reask(why) = verdict else {
            panic!("a 401 re-asks: {verdict:?}")
        };
        assert!(why.contains("rejected"), "{why}");
    }
    let (_, verdict) = gate.submit("still.not.it", &reject);
    assert!(
        matches!(&verdict, TokenVerdict::GiveUp(why) if why.contains("nothing was saved")),
        "{verdict:?}"
    );
    assert_eq!(calls.get(), 3);
}

#[test]
fn headless_with_a_rejected_token_errors_before_any_write() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    let mut supplied = token_flag("sk-WRONG.token.value");
    let err = onboard_flags(&mut supplied, &path, &|_| Err(CheckError::Rejected), None)
        .err()
        .expect("a rejected token must stop the flow");
    let text = format!("{err:#}");
    assert!(text.contains("Nothing was saved"), "{text}");
    assert!(!text.contains("sk-WRONG"), "{text}");
    assert!(!path.exists(), "nothing may be written");
}

#[test]
fn a_numeric_app_id_is_refused_once_with_guidance() {
    let calls = Cell::new(0);
    let accept = |_: &str| {
        calls.set(calls.get() + 1);
        Ok(bot(true))
    };
    let mut gate = TokenGate::default();
    let (_, verdict) = gate.submit("1234567890123456789", &accept);
    assert!(
        matches!(&verdict, TokenVerdict::Reask(why) if why.contains("application ID")),
        "{verdict:?}"
    );
    assert_eq!(calls.get(), 0, "an app ID is never sent to Discord");
    let (_, verdict) = gate.submit("real.bot.token", &accept);
    assert!(matches!(verdict, TokenVerdict::Accepted(_)));
    assert_eq!(calls.get(), 1);
}

#[test]
fn curly_quotes_and_non_ascii_are_stripped_before_check_and_save() {
    let seen = RefCell::new(String::new());
    let accept = |token: &str| {
        *seen.borrow_mut() = token.to_string();
        Ok(bot(true))
    };
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    let mut supplied = token_flag("\u{201c}tok.en.value\u{201d}\u{200b}");
    onboard_flags(&mut supplied, &path, &accept, None).unwrap();
    assert_eq!(*seen.borrow(), "tok.en.value");
    assert_eq!(supplied.get("token"), Some("tok.en.value"));
}

#[test]
fn offline_saves_with_a_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    let mut supplied = token_flag("tok.en.value");
    let notes = onboard_flags(
        &mut supplied,
        &path,
        &|_| {
            Err(CheckError::Unreachable(
                "Discord did not answer".to_string(),
            ))
        },
        None,
    )
    .unwrap();
    assert_eq!(supplied.get("token"), Some("tok.en.value"));
    let notes = notes.join("\n");
    assert!(notes.contains("saving it anyway"), "{notes}");
    assert!(notes.contains("Message Content Intent"), "{notes}");
}

#[test]
fn the_owner_is_merged_into_the_allowlist_never_replacing_it() {
    let tmp = tempfile::tempdir().unwrap();
    let path = discord_config(tmp.path(), r#"{"allowed_users": ["999"], "keep": true}"#);
    let mut supplied = token_flag("tok.en.value");
    let notes = onboard_flags(&mut supplied, &path, &|_| Ok(bot(true)), None).unwrap();
    assert_eq!(supplied.get("owner_id"), Some("1"));
    assert_eq!(
        supplied.list("allowed_users"),
        Some(&["999".to_string(), "1".to_string()][..])
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("You are allowlisted (@alice)")),
        "{notes:?}"
    );

    write_config(
        &path,
        &DECL,
        &supplied,
        &tmp.path().join(".gray"),
        tmp.path(),
    )
    .unwrap();
    let data: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(data["allowed_users"], serde_json::json!(["999", "1"]));
    assert_eq!(data["owner_id"], "1");
    assert_eq!(data["token"], "tok.en.value");
    assert_eq!(data["keep"], true);
}

#[test]
fn an_existing_owner_is_kept_and_a_present_owner_is_not_re_added() {
    let tmp = tempfile::tempdir().unwrap();
    let path = discord_config(tmp.path(), r#"{"owner_id": "555"}"#);
    let mut supplied = token_flag("tok.en.value");
    onboard_flags(&mut supplied, &path, &|_| Ok(bot(true)), None).unwrap();
    assert_eq!(supplied.get("owner_id"), None, "owner_id is never replaced");
    assert_eq!(supplied.list("allowed_users"), Some(&["1".to_string()][..]));

    // Already allowed: nothing to offer, nothing changes.
    let path = discord_config(tmp.path(), r#"{"owner_id": "1"}"#);
    let mut supplied = token_flag("tok.en.value");
    let notes = onboard_flags(&mut supplied, &path, &|_| Ok(bot(true)), None).unwrap();
    assert!(supplied.list("allowed_users").is_none());
    assert!(
        !notes.iter().any(|n| n.contains("allowlisted")),
        "{notes:?}"
    );
}

#[test]
fn a_comma_separated_allowlist_flag_is_written_as_a_merged_array() {
    let tmp = tempfile::tempdir().unwrap();
    let path = discord_config(tmp.path(), r#"{"allowed_users": ["999"]}"#);
    let mut supplied = Supplied::default();
    supplied.insert("allowed_users", "<@!2>, 999,3".to_string(), false);
    onboard_flags(&mut supplied, &path, &|_| Ok(bot(true)), None).unwrap();
    assert_eq!(
        supplied.list("allowed_users"),
        Some(&["999".to_string(), "2".to_string(), "3".to_string()][..])
    );
}

#[test]
fn the_report_carries_the_invite_link_with_the_hermes_permissions() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");
    let mut supplied = token_flag("tok.en.value");
    let notes = onboard_flags(&mut supplied, &path, &|_| Ok(bot(true)), None).unwrap();
    assert_eq!(INVITE_PERMISSIONS, 309_240_908_864);
    let invite = notes
        .iter()
        .find(|n| n.contains("oauth2/authorize"))
        .expect("an invite link");
    assert!(invite.contains("client_id=42"), "{invite}");
    assert!(invite.contains("permissions=309240908864"), "{invite}");
    assert!(
        invite.contains("scope=bot+applications.commands"),
        "{invite}"
    );
    assert!(invite.contains("integration_type=0"), "{invite}");
    // server_count == 0 changes the lead line.
    assert!(
        notes.iter().any(|n| n.contains("isn't in any server yet")),
        "{notes:?}"
    );
}

#[test]
fn intent_off_links_the_toggle_and_rechecks_until_on() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.json");

    // No terminal: link and keep going.
    let mut supplied = token_flag("tok.en.value");
    let notes = onboard_flags(&mut supplied, &path, &|_| Ok(bot(false)), None).unwrap();
    let text = notes.join("\n");
    assert!(
        text.contains("https://discord.com/developers/applications/42/bot"),
        "{text}"
    );
    assert!(text.contains("Privileged Gateway Intents"), "{text}");

    // A terminal: Enter re-checks; the second re-check finds it on.
    let checks = Cell::new(0);
    let flips = |_: &str| {
        checks.set(checks.get() + 1);
        Ok(bot(checks.get() >= 3))
    };
    let asked = Cell::new(0);
    let mut ask = |_: &str| {
        asked.set(asked.get() + 1);
        String::new()
    };
    let mut supplied = token_flag("tok.en.value");
    let notes = onboard_flags(&mut supplied, &path, &flips, Some(&mut ask)).unwrap();
    assert_eq!(asked.get(), 2);
    assert!(
        notes.iter().any(|n| n == "Message Content Intent is on."),
        "{notes:?}"
    );

    // skip keeps going without another check.
    let checks = Cell::new(0);
    let off = |_: &str| {
        checks.set(checks.get() + 1);
        Ok(bot(false))
    };
    let mut skip = |_: &str| "skip\n".to_string();
    let mut supplied = token_flag("tok.en.value");
    onboard_flags(&mut supplied, &path, &off, Some(&mut skip)).unwrap();
    assert_eq!(checks.get(), 1);
    assert_eq!(supplied.get("token"), Some("tok.en.value"));

    // At most five re-checks, then on with the link.
    let checks = Cell::new(0);
    let off = |_: &str| {
        checks.set(checks.get() + 1);
        Ok(bot(false))
    };
    let mut enter = |_: &str| String::new();
    let mut supplied = token_flag("tok.en.value");
    onboard_flags(&mut supplied, &path, &off, Some(&mut enter)).unwrap();
    assert_eq!(checks.get(), 1 + INTENT_RECHECKS as usize);
}
