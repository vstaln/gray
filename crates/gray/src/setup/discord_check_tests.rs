use super::*;
use serde_json::json;

fn app_fixture() -> Value {
    json!({
        "id": "1234567890123456789",
        "name": "gray-app",
        "bot": {"id": "1234567890123456789", "username": "graybot"},
        "owner": {"id": "111111111111111111", "username": "alice"},
        "flags": (1u64 << 19) | (1u64 << 15),
        "approximate_guild_count": 2
    })
}

#[test]
fn the_invite_integer_is_the_named_bits() {
    assert_eq!(INVITE_PERMISSIONS, 309_240_908_864);
    let names: Vec<&str> = INVITE_PERMISSION_BITS.iter().map(|(n, _)| *n).collect();
    assert!(names.contains(&"Send Messages in Threads"));
    assert!(names.contains(&"Create Public Threads"));
}

#[test]
fn an_owned_app_parses_owner_intents_and_servers() {
    let check = parse_application(&app_fixture()).unwrap();
    assert_eq!(check.app_id, "1234567890123456789");
    assert_eq!(check.bot_name, "graybot");
    assert_eq!(
        check.owners,
        [("111111111111111111".to_string(), "alice".to_string())]
    );
    assert!(check.message_content, "the limited flag counts");
    assert!(check.server_members);
    assert_eq!(check.server_count, Some(2));
}

#[test]
fn a_team_app_lists_only_accepted_members() {
    let app = json!({
        "id": "42",
        "name": "team-app",
        "owner": {"id": "999", "username": "team-user-placeholder"},
        "team": {"members": [
            {"membership_state": 2, "user": {"id": "1", "username": "alice"}},
            {"membership_state": 1, "user": {"id": "2", "username": "invited"}},
            {"membership_state": 2, "user": {"id": "3"}}
        ]},
        "flags": 0
    });
    let check = parse_application(&app).unwrap();
    assert_eq!(
        check.owners,
        [
            ("1".to_string(), "alice".to_string()),
            ("3".to_string(), "3".to_string())
        ]
    );
    assert!(!check.message_content);
    assert!(!check.server_members);
    assert_eq!(check.server_count, None);
    // No bot object: the application name stands in.
    assert_eq!(check.bot_name, "team-app");
}

#[test]
fn an_answer_without_an_id_is_an_error() {
    assert!(parse_application(&json!({"name": "x"})).is_err());
}

#[test]
fn invite_and_settings_urls_carry_the_app_id() {
    let check = parse_application(&app_fixture()).unwrap();
    assert_eq!(
        check.invite_url(),
        "https://discord.com/oauth2/authorize?client_id=1234567890123456789&scope=bot+applications.commands&permissions=309240908864&integration_type=0"
    );
    assert_eq!(
        check.bot_settings_url(),
        "https://discord.com/developers/applications/1234567890123456789/bot"
    );
}

#[test]
fn a_bot_in_no_server_gets_the_direct_invite_line() {
    let mut check = parse_application(&app_fixture()).unwrap();
    assert!(invite_lines(&check)[0].starts_with("Invite link"));
    check.server_count = Some(0);
    let lines = invite_lines(&check);
    assert!(lines[0].contains("isn't in any server yet"), "{lines:?}");
    assert!(lines[1].contains("permissions=309240908864"));
}

#[test]
fn intent_lines_link_straight_to_the_toggle() {
    let check = parse_application(&app_fixture()).unwrap();
    let lines = intent_lines(&check).join("\n");
    assert!(lines.contains("/applications/1234567890123456789/bot"));
    assert!(lines.contains("Privileged Gateway Intents"));
}

#[test]
fn pasted_tokens_lose_curly_quotes_and_non_ascii() {
    assert_eq!(clean_token("\u{201c}abc.def.ghi\u{201d}"), "abc.def.ghi");
    assert_eq!(clean_token("  abc\u{200b}.def  "), "abc.def");
    assert!(has_inner_break("abc\ndef"));
    assert!(!has_inner_break("abc.def"));
}

#[test]
fn a_numeric_paste_is_the_application_id() {
    assert!(
        token_shape_error("1234567890123456789")
            .unwrap()
            .contains("application ID")
    );
    assert!(token_shape_error("abc.def.ghi").is_none());
    assert!(token_shape_error("").is_none());
}

#[test]
fn merging_only_adds() {
    let existing = vec!["999".to_string(), "1".to_string()];
    let merged = merge_allowed(&existing, &["1".to_string(), "2".to_string()]);
    assert_eq!(merged, ["999", "1", "2"]);
    assert_eq!(merge_allowed(&[], &[]), Vec::<String>::new());
}

#[test]
fn errors_never_carry_the_token() {
    for err in [
        CheckError::Rejected,
        CheckError::Status(500),
        CheckError::Unreachable("Discord did not answer".to_string()),
    ] {
        assert!(!err.to_string().contains("sk-SECRET"));
    }
    assert_eq!(auth_header("sk-SECRET"), "Bot sk-SECRET");
}

/// A one-shot `/applications/@me` stand-in on a real port; hands back the
/// raw request so the header Discord sees can be asserted.
pub(crate) fn stub_discord(
    status: &'static str,
    body: &'static str,
) -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Some(Ok(mut s)) = listener.incoming().next() {
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    (format!("http://127.0.0.1:{port}"), rx)
}

#[test]
fn the_live_check_sends_a_bot_header_and_parses_the_answer() {
    let (base, request) = stub_discord(
        "200 OK",
        r#"{"id":"42","bot":{"username":"graybot"},"owner":{"id":"7","username":"alice"},"flags":524288,"approximate_guild_count":0}"#,
    );
    let check = check_bot_token_at(&base, "tok.en.value").unwrap();
    assert_eq!(check.app_id, "42");
    assert!(check.message_content);
    assert_eq!(check.server_count, Some(0));
    let request = request.recv().unwrap().to_ascii_lowercase();
    assert!(request.starts_with("get /applications/@me"), "{request}");
    assert!(
        request.contains("authorization: bot tok.en.value"),
        "{request}"
    );
    assert!(!request.contains("bearer"), "{request}");
}

#[test]
fn a_401_is_a_rejection_and_other_codes_are_unverified() {
    let (base, _rx) = stub_discord("401 Unauthorized", r#"{"message":"401: Unauthorized"}"#);
    assert_eq!(check_bot_token_at(&base, "bad"), Err(CheckError::Rejected));
    let (base, _rx) = stub_discord("503 Service Unavailable", "{}");
    assert_eq!(check_bot_token_at(&base, "x"), Err(CheckError::Status(503)));
}
