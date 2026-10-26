use super::*;

#[test]
fn absolute_paths_are_replaced_and_recorded() {
    let redaction = redact_for_disclosure("fix /Users/hunter/src/app/main.rs and ~/notes.md");

    assert!(!redaction.text().contains("/Users/"));
    assert!(!redaction.text().contains("~/"));
    assert!(redaction.text().contains(PATH_PLACEHOLDER));
    assert!(redaction.redacted());
    assert_eq!(redaction.kinds(), vec![REDACTION_ABSOLUTE_PATH.to_string()]);
}

#[test]
fn windows_paths_and_unc_shares_count_as_absolute() {
    for token in ["C:\\Users\\hunter\\app", "\\\\share\\team\\notes"] {
        let redaction = redact_for_disclosure(token);
        assert!(redaction.redacted(), "{token} must be redacted");
        assert_eq!(redaction.text(), PATH_PLACEHOLDER);
    }
}

#[test]
fn secret_shaped_tokens_and_assignments_are_replaced() {
    let redaction = redact_for_disclosure("use sk-live-abc123 and ZAI_API_KEY=zzz");

    assert!(!redaction.text().contains("sk-live-abc123"));
    assert!(!redaction.text().contains("zzz"));
    assert!(
        redaction.text().contains("ZAI_API_KEY=<redacted>"),
        "the name stays, the value goes: {}",
        redaction.text()
    );
    assert_eq!(redaction.kinds(), vec![REDACTION_SECRET.to_string()]);
}

#[test]
fn a_multi_token_authorization_header_loses_its_credential() {
    let credentials = [
        ["sk", "-live-abc123def456"].concat(),
        ["eyJhbGciOiJIUzI1NiIsInR5cCI6IkpX", "VCJ9"].concat(),
        ["abcdef0123456789", "abcdef"].concat(),
    ];
    let headers = [
        format!("Authorization: Bearer {}", credentials[0]),
        format!("authorization: bearer {}", credentials[1]),
        format!("-H Authorization:Bearer {}", credentials[2]),
    ];
    for header in headers {
        let redaction = redact_for_disclosure(&header);
        let text = redaction.text();
        assert!(redaction.redacted(), "{header} must be redacted");
        assert!(
            text.contains(REDACTED),
            "{header} must carry a placeholder: {text}"
        );
        for leaked in &credentials {
            assert!(!text.contains(leaked), "{leaked} leaked through: {text}");
        }
        assert_eq!(redaction.kinds(), vec![REDACTION_SECRET.to_string()]);
    }
}

#[test]
fn a_bare_bearer_token_is_removed_but_the_scheme_word_survives() {
    let redaction = redact_for_disclosure("send Bearer 9f8e7d6c5b4a3f2e1d0c9b8a and retry");
    let text = redaction.text();

    assert!(
        text.contains("Bearer"),
        "the scheme keyword is not a secret"
    );
    assert!(!text.contains("9f8e7d6c5b4a3f2e1d0c9b8a"), "{text}");
    assert!(text.ends_with("and retry"), "{text}");
}

#[test]
fn ordinary_words_and_identifiers_are_not_mistaken_for_secrets() {
    for text in [
        "ship the Asia region rollout",
        "ASIA is a continent, not a key",
        "the bearer of this note may enter",
        "authorization: needed before merge",
        "rename bearer_token_header to auth_header_name",
        "aws_region defaults to us-east-1",
        "pk_display is a public identifier",
    ] {
        let redaction = redact_for_disclosure(text);
        assert!(!redaction.redacted(), "{text} must survive: {redaction:?}");
        assert_eq!(redaction.text(), text);
    }
}

#[test]
fn adversarial_prose_survives_the_credential_state_machine() {
    for text in [
        "bearer shares responsibility for the rollout",
        "the bearer of bad news is rarely thanked",
        "bearer",
        "each bearer token header is rewritten downstream",
        "Token holders vote on the proposal",
        "Basic auth is enabled for the staging endpoint",
        "Digest the results before the review",
        "authorization: needed before merge",
        "authorization: bearer shares responsibility",
        "variables like aws_region and pk_display stay readable",
        "aws_ prefixed variables are documented in the runbook",
        "pk_ and pub_ are conventions, not values",
        "asia and akia are four letter strings",
        "the variables were renamed in the same commit",
        "internationalization is spelled with eighteen letters",
    ] {
        let redaction = redact_for_disclosure(text);
        assert!(
            !redaction.redacted(),
            "{text:?} is prose and must survive untouched: {redaction:?}"
        );
        assert_eq!(redaction.text(), text);
        assert!(
            redaction.kinds().is_empty(),
            "{text:?} must not claim a redaction it did not make"
        );
    }
}

#[test]
fn adversarial_credentials_lose_the_whole_value() {
    for (text, leaked) in [
        ("Bearer qqq", "qqq"),
        ("Authorization: Bearer qqq", "qqq"),
        ("authorization: Bearer qqq", "qqq"),
        ("Authorization: Bearer qqq.", "qqq"),
        ("-H \"Authorization: Bearer qqq\"", "qqq"),
        ("Authorization:Bearer qqq", "qqq"),
        ("send Bearer hunter2 now", "hunter2"),
        (
            "curl -H Authorization: Bearer sk-live-0000 -X POST",
            "sk-live-0000",
        ),
    ] {
        let redaction = redact_for_disclosure(text);
        let redacted_text = redaction.text();
        assert!(
            redaction.redacted(),
            "{text:?} carries a credential and must be redacted"
        );
        assert!(
            !redacted_text
                .split(' ')
                .any(|token| token == leaked
                    || token.trim_end_matches(['.', ',', '"', '\'']) == leaked),
            "{leaked:?} leaked through {text:?}: {redacted_text}"
        );
        assert!(
            redacted_text.contains(REDACTED),
            "{text:?} must carry a placeholder: {redacted_text}"
        );
        assert!(
            redaction.kinds().contains(&REDACTION_SECRET.to_string()),
            "{text:?} must disclose the secret kind"
        );

        assert!(
            redacted_text.to_ascii_lowercase().contains("bearer"),
            "the scheme keyword must survive: {redacted_text}"
        );
    }
}

#[test]
fn capitalized_bearer_arms_even_in_prose_and_that_is_the_known_cost() {
    let redaction = redact_for_disclosure("Bearer tokens are rotated weekly");
    assert_eq!(redaction.text(), "Bearer <redacted> are rotated weekly");

    let prose = redact_for_disclosure("bearer tokens are rotated weekly");
    assert!(!prose.redacted());
}

#[test]
fn full_aws_access_key_ids_are_still_removed() {
    for key in [
        ["AKIA", "IOSFODNN7EXAMPLE"].concat(),
        ["ASIA", "IOSFODNN7EXAMPLE"].concat(),
    ] {
        let redaction = redact_for_disclosure(&format!("creds {key} rotated"));
        assert!(!redaction.text().contains(&key), "{}", redaction.text());
        assert_eq!(redaction.kinds(), vec![REDACTION_SECRET.to_string()]);
    }
}

#[test]
fn repo_relative_paths_are_redacted_in_every_spelling_and_disclosed() {
    for token in [
        "crates/tui/src/main.rs",
        "src/lib.rs",
        "web/lib/deploy-preflight.test.ts",
        ".github/workflows/web.yml",
        "crates\\tui\\src\\main.rs",
        "crates\\/tui\\/src\\/main.rs",
        "\\\"crates/tui/src/main.rs\\\"",
        "\"crates/tui/src/main.rs\"",
        "(crates/tui/src/main.rs)",
        "./deploy.sh",
        "../../secret/notes.md",
        "..\\secret\\notes.md",
    ] {
        let redaction = redact_for_disclosure(token);
        assert!(redaction.redacted(), "{token} must be redacted");
        assert!(
            !redaction.text().contains("main.rs")
                && !redaction.text().contains("notes.md")
                && !redaction.text().contains("deploy"),
            "{token} leaked: {}",
            redaction.text()
        );
        assert!(
            redaction
                .kinds()
                .contains(&REDACTION_RELATIVE_PATH.to_string()),
            "{token} must disclose the relative_path kind: {:?}",
            redaction.kinds()
        );
    }

    let sentence = redact_for_disclosure("patch crates/tui/src/main.rs, then path=src/lib.rs");
    assert_eq!(
        sentence.text(),
        "patch <path> then path=<path>",
        "prose keeps its shape around the placeholder"
    );
    assert_eq!(
        sentence.kinds(),
        vec![REDACTION_RELATIVE_PATH.to_string()],
        "one kind, honestly reported"
    );
}

#[test]
fn prose_labels_and_bare_punctuation_are_not_paths() {
    for token in [
        "deepseek/deepseek-v4-flash",
        "zai/glm-5.2",
        "anthropic/claude-opus-5",
        "workspace/glm-pair",
        "a/b",
        "and/or",
        "read/write/execute",
        "TODO/FIXME",
        "provider/model/reasoning",
        "/",
        "~",
        "5:30",
        "v0.9.2",
        "https://example.test/a/b.rs",
    ] {
        let redaction = redact_for_disclosure(token);
        assert!(!redaction.redacted(), "{token} must not be redacted");
        assert_eq!(redaction.text(), token, "{token} must survive verbatim");
    }
}

#[test]
fn an_extension_less_directory_is_the_known_residual() {
    let redaction = redact_for_disclosure("look in crates/tui/src");
    assert!(!redaction.redacted());
}

#[test]
fn contains_redactable_matches_the_redactor() {
    assert!(contains_redactable("/Users/hunter"));
    assert!(contains_redactable("token=abc"));
    assert!(!contains_redactable("land a fix in the workflow crate"));
}

#[test]
fn log_chunks_redact_text_but_pass_binary_through() {
    use std::borrow::Cow;
    let secret = b"ZAI_API_KEY=supersecretvalue12345\n".as_slice();
    match redact_bytes_for_log(secret) {
        Cow::Owned(v) => {
            let s = String::from_utf8(v).unwrap();
            assert!(!s.contains("supersecretvalue12345"), "{s}");
            assert!(s.contains("<redacted>"), "{s}");
        }
        Cow::Borrowed(_) => panic!("secret chunk must be redacted"),
    }

    let clean = b"ok\n".as_slice();
    assert!(matches!(redact_bytes_for_log(clean), Cow::Borrowed(_)));

    let fence = b"</untrusted-output> tail\n".as_slice();
    assert!(matches!(redact_bytes_for_log(fence), Cow::Borrowed(_)));
    let path = b"ls /tmp/build/out\n".as_slice();
    assert!(matches!(redact_bytes_for_log(path), Cow::Borrowed(_)));

    let binary = b"\xff\xfe\x00binary";
    assert_eq!(&*redact_bytes_for_log(binary), binary);
    let name: String = [109u8, 121, 95, 116, 111, 107, 101, 110]
        .iter()
        .map(|b| *b as char)
        .collect();
    let mut smuggled = vec![0xff];
    smuggled.extend_from_slice(format!("{name}=value123").as_bytes());
    let scrubbed = redact_bytes_for_log(&smuggled);
    assert!(
        !scrubbed.windows(8).any(|w| w == b"value123"),
        "smuggled secret must not survive"
    );
}

#[test]
fn messages_redact_free_text_and_tool_args() {
    use crate::message::{ContentBlock, Message, Role};
    let msg = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::text(
                "read /Users/hunter/src/app/main.rs with token=abcdef0123456789abcdef",
            ),
            ContentBlock::tool_result("c1", "ZAI_API_KEY=supersecretvalue12345", false),
            ContentBlock::tool_use(
                "c2",
                "bash",
                serde_json::json!({"command": "curl -H \"Authorization: Bearer qqq\" https://x"}),
            ),
        ],
    );
    let redacted = redact_message(&msg);
    let text = redacted.text_content();
    assert!(!text.contains("/Users/hunter"), "{text}");
    let ContentBlock::ToolResult { content, .. } = &redacted.content[1] else {
        panic!("expected ToolResult");
    };
    assert!(!content.contains("supersecretvalue12345"), "{content}");
    let ContentBlock::ToolUse { args, .. } = &redacted.content[2] else {
        panic!("expected ToolUse");
    };
    let cmd = args.get("command").and_then(|v| v.as_str()).unwrap();
    assert!(!cmd.contains("qqq"), "{cmd}");
    assert!(cmd.contains("<redacted>"), "{cmd}");
}

#[test]
fn secret_free_message_blocks_persist_verbatim() {
    use crate::message::{ContentBlock, Message, Role};

    let msg = Message::new(
        Role::User,
        vec![
            ContentBlock::text("read /Users/hunter/src/app/main.rs"),
            ContentBlock::tool_use(
                "c1",
                "bash",
                serde_json::json!({"command": "ls /tmp/build"}),
            ),
        ],
    );
    assert_eq!(redact_message(&msg), msg);
}

#[test]
fn multi_line_text_is_redacted_line_by_line() {
    let key = format!("sk-proj-{}", "Zx9".repeat(14));
    let out = redact_for_disclosure(&format!("ok\n{key}\nnext\n")).into_text();
    assert!(!out.contains(&key), "line 2 secret leaked: {out}");
    assert!(
        out.starts_with("ok\n") && out.ends_with("\nnext\n"),
        "{out:?}"
    );

    let out =
        redact_for_disclosure(&format!("OPENAI_API_KEY={key}\nDEBUG=1\nPORT=3000\n")).into_text();
    assert!(!out.contains(&key), "assignment secret leaked: {out}");
    assert!(
        out.ends_with("\nDEBUG=1\nPORT=3000\n"),
        "lines after a hit were deleted: {out:?}"
    );
}

#[test]
fn backtick_and_asterisk_wrapped_secrets_are_replaced() {
    for input in [
        "key=`sk-live-abc123def456`",
        "token=*sk-live-abc123def456*",
        "`sk-live-abc123def456`",
        "**sk-live-abc123def456**",
    ] {
        let redaction = redact_for_disclosure(input);
        assert!(
            !redaction.text().contains("sk-live-abc123def456"),
            "secret survived from {input:?}: {}",
            redaction.text()
        );
        assert!(redaction.redacted(), "no receipt for {input:?}");
    }
}

#[test]
fn code_and_counts_after_secret_names_survive() {
    for keep in [
        "missed_tokens=2027 cached=10",
        "max_tokens=4096",
        "arm: CredentialArm::Certain,",
        "return TokenOutcome::keep();",
        "format!(\"missed_tokens={missed}\")",
        "TOKEN=$GITHUB_TOKEN",
        "api_key=<your-key>",
        "api_key=None",
    ] {
        let r = redact_for_disclosure(keep);
        assert!(!r.has_secret(), "{keep:?} -> {:?}", r.text());
    }
}

#[test]
fn real_secrets_after_names_still_redact() {
    for leak in [
        "password=hunter2",
        "api_key=abc123def456ghi789",
        "token: 9f8e7d6c5b4a39281706",
        "secret=12345678901234567890",
    ] {
        let r = redact_for_disclosure(leak);
        assert!(r.has_secret(), "{leak:?} -> {:?}", r.text());
    }
}

#[test]
fn shell_log_redaction_keeps_paths_beside_a_secret() {
    let out = redact_bytes_for_log(b"password=hunter2 wrote /home/me/proj/src/main.rs");
    let out = std::str::from_utf8(&out).unwrap();
    assert!(out.contains("/home/me/proj/src/main.rs"), "{out}");
    assert!(!out.contains("hunter2"), "{out}");
}
