use super::*;
use gray_core::message::Message;

#[test]
fn headless_row_shows_short_id_preview_and_age() {
    let s = SessionSummary {
        id: SessionId::new("30e3f464-aaaa-bbbb-cccc-d60f2104dcd9"),
        started_at: now_millis(),
        cwd: std::path::PathBuf::from("/tmp"),
        first_user_text: Some("hi there".to_string()),
    };
    assert_eq!(format_summary_row(&s), "30e3f464 — hi there (just now)");
}

async fn seed(
    store: &JsonlSessionStore,
    id: &str,
    cwd: std::path::PathBuf,
    texts: &[&str],
) -> SessionId {
    let sid = store
        .create(gray_session::SessionMeta::new(
            SessionId::new(id),
            1,
            cwd,
            "test-model",
        ))
        .await
        .unwrap();
    for t in texts {
        store.append(&sid, &Message::user(*t)).await.unwrap();
    }
    sid
}

#[tokio::test]
async fn strict_resolve_accepts_exact_and_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let cwd = std::env::current_dir().unwrap();
    seed(&store, "test-alpha-1", cwd, &["hi"]).await;
    assert_eq!(
        resolve_session_strict(&store, "test-alpha-1", false)
            .await
            .unwrap()
            .as_str(),
        "test-alpha-1"
    );
    assert_eq!(
        resolve_session_strict(&store, "test-alpha", false)
            .await
            .unwrap()
            .as_str(),
        "test-alpha-1"
    );
}

#[tokio::test]
async fn strict_resolve_bogus_errors_like_resume() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let err = resolve_session_strict(&store, "bogus", false)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("no session matching 'bogus'"),
        "bogus id must report like `resume bogus`, got: {err:#}"
    );
}

#[tokio::test]
async fn latest_fallback_empty_store_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    assert!(latest_session_anywhere(&store).await.is_none());
}

#[tokio::test]
async fn latest_fallback_finds_cwd_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let cwd = std::env::current_dir().unwrap();
    seed(&store, "test-latest-1", cwd, &["hi"]).await;
    assert_eq!(
        latest_session_anywhere(&store).await.unwrap().as_str(),
        "test-latest-1"
    );
}

#[test]
fn effective_session_model_prefers_config() {
    assert_eq!(
        effective_session_model(Some("cfg-model"), "meta-model"),
        Some("cfg-model".to_string())
    );
    assert_eq!(
        effective_session_model(None, "meta-model"),
        Some("meta-model".to_string())
    );
    assert_eq!(
        effective_session_model(Some(""), "meta-model"),
        Some("meta-model".to_string())
    );
    assert_eq!(effective_session_model(Some("  "), ""), None);
    assert_eq!(effective_session_model(None, ""), None);
}

#[tokio::test]
async fn resumed_line_reports_id_and_count() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let cwd = std::env::current_dir().unwrap();
    let sid = seed(&store, "test-line-1", cwd, &["one", "two"]).await;
    let line = resumed_session_line(&store, &sid).await.unwrap();
    assert!(
        line.contains("test-line-1") && line.contains("2 messages"),
        "announcement must state id + count, got: {line}"
    );
}

#[tokio::test]
async fn resumed_line_bogus_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    assert!(
        resumed_session_line(&store, &SessionId::new("bogus"))
            .await
            .is_err()
    );
}
