use super::*;

#[tokio::test]
async fn background_rejects_headless_and_malformed_requests() {
    let handler = default_handler(PathBuf::from("/tmp"));
    for params in [
        serde_json::json!({}),
        serde_json::json!({"path": 42}),
        serde_json::json!({"path": null, "raw_escape": "bad"}),
    ] {
        let reply = handler("host/background".into(), params).await;
        assert!(reply.get("error").is_some(), "{reply}");
    }
    let reply = handler("host/background".into(), serde_json::json!({"path": null})).await;
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("interactive Gray TUI")
    );
}
