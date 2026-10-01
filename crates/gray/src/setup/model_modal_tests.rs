use super::validate_direct_model_id;

fn models() -> Vec<(String, String)> {
    vec![
        ("zai/glm-5.2".to_string(), "GLM 5.2".to_string()),
        ("openai/gpt-5".to_string(), "GPT 5".to_string()),
    ]
}

#[test]
fn bogus_model_id_is_rejected_with_browse_hint() {
    let err = validate_direct_model_id("bogus-model-xyz-123", &models()).unwrap_err();
    assert!(err.contains("unknown model"), "{err}");
    assert!(err.contains("bogus-model-xyz-123"), "{err}");
    assert!(err.contains("/model"), "{err}");
}

#[test]
fn exact_id_is_accepted() {
    assert_eq!(
        validate_direct_model_id("zai/glm-5.2", &models()).unwrap(),
        "zai/glm-5.2"
    );
}

#[test]
fn input_is_trimmed() {
    assert_eq!(
        validate_direct_model_id("  zai/glm-5.2  ", &models()).unwrap(),
        "zai/glm-5.2"
    );
}

#[test]
fn case_insensitive_id_canonicalizes() {
    assert_eq!(
        validate_direct_model_id("ZAI/GLM-5.2", &models()).unwrap(),
        "zai/glm-5.2"
    );
}

#[test]
fn unique_provider_tail_resolves() {
    assert_eq!(
        validate_direct_model_id("glm-5.2", &models()).unwrap(),
        "zai/glm-5.2"
    );
}

#[test]
fn ambiguous_tail_is_rejected() {
    let dup = vec![
        ("a/same".to_string(), "A".to_string()),
        ("b/same".to_string(), "B".to_string()),
    ];
    let err = validate_direct_model_id("same", &dup).unwrap_err();
    assert!(err.contains("ambiguous"), "{err}");
    assert!(err.contains("/model"), "{err}");
}

#[test]
fn empty_known_list_fails_open_for_custom_endpoints() {
    assert_eq!(
        validate_direct_model_id("my-local-model", &[]).unwrap(),
        "my-local-model"
    );
}

#[test]
fn empty_input_is_usage_not_silent_default() {
    let err = validate_direct_model_id("   ", &models()).unwrap_err();
    assert!(err.contains("/model"), "{err}");
}

// ── the modal paints before the network answers ──

/// A `/models` endpoint that answers with two models, served on a real
/// port so the fetch path is exercised end to end.
fn mock_models_server(body: &'static str) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(4) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            );
        }
    });
    port
}

#[test]
fn the_live_fetch_works_from_a_thread_with_no_runtime() {
    // The refresher thread has no ambient runtime: before this split the
    // call returned an empty list there and the modal never filled in.
    let body = r#"{"data":[{"id":"m-one","name":"Model One"},{"id":"m-two","name":"Model Two"}]}"#;
    let port = mock_models_server(body);
    let base = format!("http://127.0.0.1:{port}/v1");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let got = super::super::context::fetch_live_provider_models(&base, None);
        let _ = tx.send(got);
    });
    let got = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("background fetch answered");
    let ids: Vec<&str> = got.iter().map(|(id, _)| id.as_str()).collect();
    assert!(ids.contains(&"m-one"), "{got:?}");
    assert!(ids.contains(&"m-two"), "{got:?}");
}

#[test]
fn saved_models_need_no_network() {
    // Whatever the saved config holds, this must not fail or block: it is
    // what the first frame is drawn from.
    let _ = super::saved_models_for("http://127.0.0.1:1/v1");
}

#[test]
fn a_live_list_merges_into_the_saved_ordering() {
    let live = vec![
        ("zeta/1".to_string(), "Zeta".to_string()),
        ("alpha/1".to_string(), "Alpha".to_string()),
    ];
    let merged = super::merge_models("http://127.0.0.1:1/v1", live);
    assert_eq!(merged.len(), 2, "every live model is offered");
    let ids: Vec<&str> = merged.iter().map(|(id, _)| id.as_str()).collect();
    assert!(ids.contains(&"zeta/1") && ids.contains(&"alpha/1"));
}

// ── recent/all divider ──

fn sorted_models() -> Vec<(String, String)> {
    // What `sort_models` yields: current first, then recents, then the rest.
    vec![
        ("cur/m".to_string(), "Current".to_string()),
        ("a/1".to_string(), "A".to_string()),
        ("b/2".to_string(), "B".to_string()),
        ("c/3".to_string(), "C".to_string()),
    ]
}

#[test]
fn recent_prefix_counts_the_sorted_head() {
    let models = sorted_models();
    let n = super::recent_prefix_len(
        Some("cur/m"),
        &["a/1".to_string(), "b/2".to_string()],
        models.iter().map(|(id, _)| id.as_str()),
    );
    assert_eq!(n, 3, "current + two recents park first");
}

#[test]
fn recent_prefix_stops_at_the_first_unknown() {
    let shuffled = [
        ("a/1".to_string(), "A".to_string()),
        ("c/3".to_string(), "C".to_string()),
        ("b/2".to_string(), "B".to_string()),
    ];
    let n = super::recent_prefix_len(
        None,
        &["a/1".to_string(), "b/2".to_string()],
        shuffled.iter().map(|(id, _)| id.as_str()),
    );
    assert_eq!(n, 1, "an unknown id ends the recent section");
}

#[test]
fn selection_never_rest_on_the_divider() {
    use super::Row;
    let rows = vec![Row::Model(0), Row::Divider, Row::Model(1)];
    assert_eq!(super::skip_divider(&rows, 1, true), 2);
    assert_eq!(super::skip_divider(&rows, 1, false), 0);
    assert_eq!(
        super::skip_divider(&rows, 0, true),
        0,
        "models pass through"
    );
    assert_eq!(
        super::skip_divider(&rows, 2, false),
        2,
        "models pass through"
    );
}
