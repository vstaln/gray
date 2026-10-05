use super::*;

#[test]
fn codex_style_server_error_extracts_message_not_raw_json() {
    // Screenshot case: opencode/zen 503 with {"model":..,"error":{...}} blob.
    // Codex-style: show Status/Code/Type/Message, never the raw JSON dump.
    let detail = r#"server error: status 503 Service Unavailable: {"model":"muse-spark-1.3-contributor","error":{"param":null,"type":"server_error","message":"Error from provider (Console Go): Upstream request failed: [service_overloaded] The backend is temporarily overloaded. Please retry."}}, cf-ray: a35cf09b5c1b7537-SEA"#;
    let out = format_core_error(
        &CoreError::Provider(detail.to_string()),
        "https://opencode.ai/zen/go/v1",
    );
    assert!(out.contains("(retryable)"), "must stay retryable: {out}");
    assert!(
        out.contains("The backend is temporarily overloaded"),
        "must surface extracted message: {out}"
    );
    assert!(!out.contains("\"model\":"), "must not dump raw JSON: {out}");
    assert!(!out.contains("\"param\":"), "must not dump raw JSON: {out}");
    assert!(out.contains("503"), "must keep status: {out}");
    assert!(out.contains("cf-ray"), "must keep cf-ray: {out}");
}

#[test]
fn nested_devin_limit_is_actionable_without_retry_logs() {
    let native = "Reached free model rate limit. Please switch to a different model. Your limit will reset in 6 hours 9 minutes (at 21:01 UTC). (trace ID: private-trace)";
    let message = format!(
        "Devin quota exhausted (native: {native}) 2026-10-05T14:50:17Z WARN autofanato::agent::control_loop: attempt=1 max=3 error=Inference(ServerError({native})) Transient inference error; retrying on native"
    );
    let detail = format!(
        "provider-side status 500 Internal Server Error: {} (request was valid)",
        serde_json::json!({"error": {"message": message}})
    );
    let out = format_core_error(&CoreError::ServerError(detail), "https://127.0.0.1:1/");
    assert_eq!(
        out,
        "✗ Model limit reached\n  Resets in 6 hours 9 minutes (21:01 UTC).\n  Run /model to switch to another model."
    );
}

#[test]
fn json_followed_by_another_json_log_still_extracts_first_error() {
    let detail =
        r#"status 503: {"error":{"message":"Service overloaded"}} WARN retry {"attempt":3}"#;
    let out = format_core_error(
        &CoreError::ServerError(detail.into()),
        "https://example.com",
    );
    assert!(
        out.starts_with("✗ Provider server error (retryable):\n"),
        "{out}"
    );
    assert!(out.contains("Service overloaded"), "{out}");
    assert!(!out.contains("WARN"), "{out}");
    assert!(!out.contains("attempt"), "{out}");
}

#[test]
fn nested_provider_json_is_unwrapped() {
    let inner = serde_json::json!({"error": {"message": "Session expired. Sign in again."}});
    let detail =
        serde_json::json!({"error": {"message": format!("native API error: {inner}")}}).to_string();
    let out = format_core_error(&CoreError::Auth(detail), "https://example.com");
    assert!(out.contains("Session expired. Sign in again."), "{out}");
    assert!(!out.contains('{'), "{out}");
    assert!(out.lines().next().unwrap().len() < 60, "{out}");
}

#[test]
fn quota_messages_from_other_plugins_use_the_shared_display() {
    for err in [
        CoreError::RateLimited("status 429: usage limit reached; try again after reset".into()),
        CoreError::Provider(r#"status 500: {"error":{"message":"Quota exceeded"}}"#.into()),
        CoreError::BadRequest("insufficient_quota".into()),
        CoreError::Auth("Your credit balance is too low to access the API.".into()),
    ] {
        let out = format_core_error(&err, "https://example.com");
        assert!(out.starts_with("✗ Model limit reached\n"), "{out}");
        assert!(out.contains("/model"), "{out}");
        assert!(!out.contains("server error"), "{out}");
    }
}

#[test]
fn claude_limit_preserves_the_reset_time() {
    let out = format_core_error(
        &CoreError::ServerError("You've hit your limit · resets at 4pm (Europe/London)".into()),
        "https://127.0.0.1:1/",
    );
    assert!(out.starts_with("✗ Model limit reached\n"), "{out}");
    assert!(out.contains("Resets at 4pm (Europe/London)."), "{out}");
}

#[test]
fn ordinary_rate_limits_are_not_reported_as_exhausted_quota() {
    let out = format_core_error(
        &CoreError::RateLimited("Too many requests".into()),
        "https://example.com",
    );
    assert!(out.starts_with("✗ Rate limited (retryable):\n"), "{out}");
}

#[test]
fn long_unicode_error_details_have_a_visible_truncation_marker() {
    let out = format_core_error(&CoreError::Stream("界".repeat(1000)), "https://example.com");
    assert!(out.contains('…'), "{out}");
    assert!(out.chars().count() < 400, "{out}");
}

#[test]
fn formats_subsecond_as_ms() {
    assert_eq!(fmt_duration_ms(850), "850ms");
}

#[test]
fn formats_seconds_trimming_point_zero() {
    assert_eq!(fmt_duration_ms(6000), "6s");
    assert_eq!(fmt_duration_ms(6500), "6.5s");
}

#[test]
fn formats_minutes() {
    assert_eq!(fmt_duration_ms(125_000), "2m 5s");
    assert_eq!(fmt_duration_ms(120_000), "2m");
}

#[test]
fn typed_image_link_becomes_vision_block() {
    // End-to-end: typed link -> extract -> build -> image block (no paste).
    let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("typed.png");
    img.save(&path).unwrap();
    let found = super::super::attachments::extract_inline_image_paths(
        &format!("look at {} pls", path.display()),
        dir.path(),
    );
    assert_eq!(found, vec![path]);
    let msg = build_user_message_with_attachments("look", &found);
    assert!(
        msg.content
            .iter()
            .any(|b| matches!(b, gray_core::message::ContentBlock::Image { .. })),
        "typed link must produce a vision block: {msg:?}"
    );
}

#[test]
fn typed_auth_error_names_connect_not_agent_error() {
    // Screenshot case: subscription relay 403 arrives as CoreError::Auth
    // (no HTTP status — native classified inside the sidecar). It must
    // read as an auth failure with a next command, never "agent error".
    let out = format_core_error(
        &CoreError::Auth(
            "status 403 Forbidden: Authentication failed. Please check your credentials.".into(),
        ),
        "https://127.0.0.1:1/",
    );
    assert!(out.contains("Auth failed"), "must classify: {out}");
    assert!(out.contains("/connect"), "must name the fix: {out}");
    assert!(!out.contains("agent error"), "must not fall through: {out}");
}

#[test]
fn typed_taxonomy_arms_never_fall_through_to_agent_error() {
    let cases = [
        (
            CoreError::BadRequest("bad request: nope".into()),
            "Bad request",
        ),
        (
            CoreError::RateLimited("rate limited: slow".into()),
            "Rate limited",
        ),
        (
            CoreError::ContextOverflow("context exhausted".into()),
            "Context exhausted",
        ),
        (
            CoreError::ServerError("server error: boom".into()),
            "server error",
        ),
        (
            CoreError::Stream("stream broken: cut".into()),
            "Stream broken",
        ),
        (CoreError::LoopDetected("same call 3x".into()), "Tool loop"),
        (CoreError::Cancelled, "Cancelled"),
    ];
    for (err, want) in cases {
        let out = format_core_error(&err, "https://127.0.0.1:1/");
        assert!(out.contains(want), "missing {want}: {out}");
        assert!(!out.contains("agent error"), "fell through: {out}");
    }
}
