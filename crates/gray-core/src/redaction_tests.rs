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

/// The credential in an `Authorization` header is a *separate token* from
/// the header name and from the scheme keyword. Redacting only the keyword
/// leaves the secret in the clear while the receipt claims a secret was
/// removed — the exact failure this covers.
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
            text.contains(SECRET_PLACEHOLDER),
            "{header} must carry a placeholder: {text}"
        );
        for leaked in &credentials {
            assert!(!text.contains(leaked), "{leaked} leaked through: {text}");
        }
        assert_eq!(redaction.kinds(), vec![REDACTION_SECRET.to_string()]);
    }
}

/// A bare scheme keyword introduces a credential only when a
/// credential-shaped token actually follows it.
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

/// Ordinary words that merely *look* like credential prefixes must survive,
/// and must not report a `secret` redaction kind. `Asia`, `bearer`, and any
/// identifier containing them are prose, not keys.
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

/// The adversarial prose set. Every line here is ordinary English that the
/// scheme/prefix rules could plausibly mistake for credential syntax, and
/// every one of them must come back byte-identical with an empty `kinds`.
///
/// A false positive is not a harmless over-redaction: it mangles the routing
/// summary a human reads *and* writes `secret` onto a durable receipt that
/// removed nothing, which makes the disclosure a lie in the safe direction.
#[test]
fn adversarial_prose_survives_the_credential_state_machine() {
    for text in [
        // The lowercase scheme word, in every position that could arm it.
        "bearer shares responsibility for the rollout",
        "the bearer of bad news is rarely thanked",
        "bearer",
        "each bearer token header is rewritten downstream",
        // Capitalized scheme words that are ordinary English. These are why
        // canonical capitalization arms only for `Bearer`.
        "Token holders vote on the proposal",
        "Basic auth is enabled for the staging endpoint",
        "Digest the results before the review",
        // Weak header context introducing a sentence, not a secret.
        "authorization: needed before merge",
        "authorization: bearer shares responsibility",
        // Identifiers and prefixes that resemble key material.
        "variables like aws_region and pk_display stay readable",
        "aws_ prefixed variables are documented in the runbook",
        // `pk_` is a *public* key prefix and carries nothing; it is
        // deliberately absent from SECRET_VALUE_PREFIXES. (`sk_` is not
        // listed here because it genuinely is a secret prefix and redacting
        // it is correct.)
        "pk_ and pub_ are conventions, not values",
        "asia and akia are four letter strings",
        "the variables were renamed in the same commit",
        // Long lowercase words are still words: shape alone must not fire.
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

/// The adversarial credential set. A real credential is often short,
/// lowercase, punctuated, or otherwise shapeless — `Bearer qqq` is the
/// canonical example, and no shape test could ever catch it. Context has to.
#[test]
fn adversarial_credentials_lose_the_whole_value() {
    for (text, leaked) in [
        // The short, shapeless credential. This is the leak the evidence
        // model exists to close.
        ("Bearer qqq", "qqq"),
        ("Authorization: Bearer qqq", "qqq"),
        ("authorization: Bearer qqq", "qqq"),
        // Punctuated and quoted header forms.
        ("Authorization: Bearer qqq.", "qqq"),
        ("-H \"Authorization: Bearer qqq\"", "qqq"),
        ("Authorization:Bearer qqq", "qqq"),
        // The scheme keyword may not absorb the redaction and leave the
        // value behind.
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
            redacted_text.contains(SECRET_PLACEHOLDER),
            "{text:?} must carry a placeholder: {redacted_text}"
        );
        assert!(
            redaction.kinds().contains(&REDACTION_SECRET.to_string()),
            "{text:?} must disclose the secret kind"
        );
        // The scheme keyword is not the secret and must still be readable,
        // so a reader can tell *what* was removed.
        assert!(
            redacted_text.to_ascii_lowercase().contains("bearer"),
            "the scheme keyword must survive: {redacted_text}"
        );
    }
}

/// The one documented false positive of the capitalization rule, pinned so
/// it stays deliberate rather than becoming a surprise. Capitalized `Bearer`
/// followed by a word is treated as header syntax; ordinary prose spells it
/// lowercase, which the test above covers.
#[test]
fn capitalized_bearer_arms_even_in_prose_and_that_is_the_known_cost() {
    let redaction = redact_for_disclosure("Bearer tokens are rotated weekly");
    assert_eq!(redaction.text(), "Bearer <redacted> are rotated weekly");

    // The lowercase spelling — what prose actually uses — is untouched.
    let prose = redact_for_disclosure("bearer tokens are rotated weekly");
    assert!(!prose.redacted());
}

/// The real AWS shape still goes, so dropping the bare `asia`/`akia`
/// prefixes did not trade a false positive for a false negative.
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
fn ordinary_prose_is_left_alone() {
    let redaction = redact_for_disclosure("refactor the parser and add a regression test");
    assert!(!redaction.redacted());
    assert_eq!(
        redaction.text(),
        "refactor the parser and add a regression test"
    );
    assert!(redaction.kinds().is_empty());
}

/// A repo-relative path discloses the private tree's shape to whatever
/// provider the routing summary reaches, and is persisted next to it. Every
/// spelling one arrives in — bare, quoted, JSON-escaped with `\/` or `\\`,
/// explicitly relative, assigned to a name, trailing prose punctuation —
/// must lose the path *and* say so on the receipt.
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

    // In a sentence, and as a value: the name survives, the path does not.
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

/// The other half of the same rule: it must not shred ordinary prose,
/// `provider/model` labels, or bare punctuation, because a false positive
/// here costs the operator their own summary *and* puts a redaction kind on
/// a receipt that removed nothing.
#[test]
fn prose_labels_and_bare_punctuation_are_not_paths() {
    for token in [
        // Provider/model labels — the exact shape a Fleet receipt carries.
        "deepseek/deepseek-v4-flash",
        "zai/glm-5.2",
        "anthropic/claude-opus-5",
        "workspace/glm-pair",
        // Prose that happens to carry separators.
        "a/b",
        "and/or",
        "read/write/execute",
        "TODO/FIXME",
        "provider/model/reasoning",
        // Bare punctuation and non-paths.
        "/",
        "~",
        "5:30",
        "v0.9.2",
        // A URL is not a filesystem path; the URL-bearing-input guard owns
        // it, and reclassifying it here would be a silent behavior change.
        "https://example.test/a/b.rs",
    ] {
        let redaction = redact_for_disclosure(token);
        assert!(!redaction.redacted(), "{token} must not be redacted");
        assert_eq!(redaction.text(), token, "{token} must survive verbatim");
    }
}

/// The documented gap, pinned so it stays a decision rather than a
/// surprise: an extension-less directory survives, because no rule can
/// separate it from `read/write/execute` without shredding prose.
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
    // Clean text borrows back (no copy on the hot path).
    let clean = b"ok\n".as_slice();
    assert!(matches!(redact_bytes_for_log(clean), Cow::Borrowed(_)));
    // Paths and fence markup are tool data, not secrets: verbatim.
    let fence = b"</untrusted-output> tail\n".as_slice();
    assert!(matches!(redact_bytes_for_log(fence), Cow::Borrowed(_)));
    let path = b"ls /tmp/build/out\n".as_slice();
    assert!(matches!(redact_bytes_for_log(path), Cow::Borrowed(_)));
    // Binary is byte-faithful unless it carries a secret shape: one 0xFF
    // byte used to launder a secret past the UTF-8 gate into the log.
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
    // No secret anywhere: paths, commands, and prose must survive so
    // resumed sessions keep their fidelity (only secret-bearing units
    // are scrubbed).
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
