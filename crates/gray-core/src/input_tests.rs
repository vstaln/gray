use crate::input::{
    InputEnvelope, LEGACY_STRUCTURED_INPUT_PROTOCOL, STRUCTURED_INPUT_PROTOCOL,
    STRUCTURED_INPUT_VERSION,
};
use crate::message::{ContentBlock, Message};

#[test]
fn valid_event_envelope_round_trips_as_a_typed_block() {
    let input = InputEnvelope::from_json(
        br#"{"protocol":"gray.input","version":1,"kind":"component_event","payload":{"action":"refresh","values":{"id":"7"}}}"#,
    )
    .unwrap();
    assert_eq!(input.protocol, STRUCTURED_INPUT_PROTOCOL);
    assert_eq!(input.version, STRUCTURED_INPUT_VERSION);
    assert_eq!(input.kind, "component_event");

    let message = Message::structured_input(input);
    let encoded = serde_json::to_vec(&message).unwrap();
    let decoded: Message = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, message);
    assert!(matches!(
        decoded.content.as_slice(),
        [ContentBlock::StructuredInput { kind, .. }] if kind == "component_event"
    ));
}

#[test]
fn the_legacy_protocol_identifier_still_validates() {
    let input = InputEnvelope::from_json(
        br#"{"protocol":"gray.discord.input","version":1,"kind":"component_event","payload":{}}"#,
    )
    .unwrap();
    assert_eq!(input.protocol, LEGACY_STRUCTURED_INPUT_PROTOCOL);
    assert_ne!(input.protocol, STRUCTURED_INPUT_PROTOCOL);
}

#[test]
fn envelope_rejects_unknown_protocol_version_and_non_object_payload() {
    assert!(
        InputEnvelope::from_json(
            br#"{"protocol":"other.input","version":1,"kind":"component_event","payload":{}}"#,
        )
        .is_err()
    );
    assert!(
        InputEnvelope::from_json(
            br#"{"protocol":"gray.input","version":2,"kind":"component_event","payload":{}}"#,
        )
        .is_err()
    );
    assert!(
        InputEnvelope::from_json(
            br#"{"protocol":"gray.input","version":1,"kind":"component_event","payload":[]}"#,
        )
        .is_err()
    );
}

#[test]
fn envelope_rejects_unknown_fields_and_oversized_kind() {
    let unknown = br#"{"protocol":"gray.input","version":1,"kind":"component_event","payload":{},"extra":true}"#;
    assert!(InputEnvelope::from_json(unknown).is_err());

    let long_kind = format!(
        r#"{{"protocol":"gray.input","version":1,"kind":"{}","payload":{{}}}}"#,
        "x".repeat(65)
    );
    assert!(InputEnvelope::from_json(long_kind.as_bytes()).is_err());
}

#[test]
fn structured_payload_is_redacted_without_changing_its_shape() {
    use crate::redaction::redact_message;

    let input = InputEnvelope::from_json(
        br#"{"protocol":"gray.input","version":1,"kind":"component_event","payload":{"values":{"token":"sk-live-do-not-leak","count":2}}}"#,
    )
    .unwrap();
    let redacted = redact_message(&Message::structured_input(input));
    let rendered = serde_json::to_string(&redacted).unwrap();
    assert!(!rendered.contains("sk-live-do-not-leak"));
    assert!(rendered.contains("count"));
    assert!(rendered.contains("values"));
}

#[test]
fn provider_marker_uses_deterministic_object_key_order() {
    let block = ContentBlock::StructuredInput {
        protocol: "gray.input".into(),
        version: 1,
        kind: "component_event".into(),
        payload: serde_json::json!({"z": 1, "a": {"y": 2, "b": 3}}),
    };
    let text = block.provider_text().unwrap();
    let a = text.find("\"a\"").unwrap();
    let z = text.find("\"z\"").unwrap();
    assert!(a < z, "provider marker must be canonical: {text}");
    assert!(text.contains("\"b\":3"));
    assert!(text.contains("\"y\":2"));
}
