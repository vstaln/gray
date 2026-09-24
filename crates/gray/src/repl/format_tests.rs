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
    let msg = build_user_message_with_attachments("look", &found, "test-model");
    assert!(
        msg.content
            .iter()
            .any(|b| matches!(b, gray_core::message::ContentBlock::Image { .. })),
        "typed link must produce a vision block: {msg:?}"
    );
}
