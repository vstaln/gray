use super::*;
use gray_core::message::Message;

fn summary(
    first: Option<&str>,
    last: Option<&str>,
    started_at: u64,
    last_message_at: u64,
) -> SessionSummary {
    SessionSummary {
        id: SessionId::new("30e3f464-aaaa-bbbb-cccc-d60f2104dcd9"),
        started_at,
        cwd: std::path::PathBuf::from("/tmp"),
        first_user_text: first.map(str::to_string),
        last_user_text: last.map(str::to_string),
        last_message_at,
    }
}

#[test]
fn headless_row_shows_short_id_preview_and_age() {
    let s = summary(None, Some("hi there"), now_millis(), now_millis());
    assert_eq!(format_summary_row(&s), "30e3f464 — hi there (just now)");
}

#[test]
fn row_previews_the_latest_message_and_its_timestamp() {
    // A session opened an hour ago whose latest message just landed.
    let s = summary(
        Some("how do I fix the resume picker"),
        Some("ok that worked, thanks"),
        now_millis().saturating_sub(3_600_000),
        now_millis(),
    );
    let row = format_summary_row(&s);
    assert!(
        row.starts_with("30e3f464 \u{2014} ok that worked, thanks"),
        "the row must preview the latest message: {row}"
    );
    assert!(
        row.ends_with("(just now)"),
        "age must be the latest message's: {row}"
    );
    assert!(
        !row.contains("resume picker"),
        "the first message must not be the preview: {row}"
    );
}

#[test]
fn row_without_any_user_message_says_so() {
    let s = summary(None, None, now_millis(), now_millis());
    assert!(format_summary_row(&s).contains("(no message yet)"));
}

#[test]
fn search_matches_the_latest_message() {
    let s = summary(
        Some("old topic about sqlite"),
        Some("now polishing the resume row"),
        now_millis(),
        now_millis(),
    );
    assert!(session_matches(&s, "resume row", None));
    assert!(session_matches(&s, "RESUME ROW", None));
    // The opener stays searchable too: a session is still *about* a topic it
    // opened with, so the filter keeps both ends rather than narrowing.
    assert!(session_matches(&s, "sqlite", None));
    assert!(!session_matches(&s, "sqlite resume row", None));
}

async fn seed(
    store: &JsonlSessionStore,
    id: &str,
    cwd: std::path::PathBuf,
    texts: &[&str],
) -> SessionId {
    let sid = store
        .create(crate::session_store::SessionMeta::new(
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

// ── the list reads as a timeline ──

fn aged(id: &str, started: u64, last_active: u64) -> SessionSummary {
    SessionSummary {
        id: SessionId::new(id),
        started_at: started,
        cwd: std::path::PathBuf::from("/tmp"),
        first_user_text: Some(format!("opened {id}")),
        last_user_text: Some(format!("left off on {id}")),
        last_message_at: last_active,
    }
}

#[test]
fn the_last_session_is_the_most_recently_used_not_the_newest_made() {
    // A session opened last week and used this morning is the one to
    // continue — sorting by creation put it at the bottom.
    let summaries = vec![
        aged("old-but-active", 1_000, 9_000),
        aged("new-but-stale", 5_000, 2_000),
    ];
    let latest = latest_summary(&summaries, None).expect("one session");
    assert_eq!(latest.id.as_str(), "old-but-active");
}

#[test]
fn the_cwd_scope_still_wins_over_recency() {
    let mut other = aged("elsewhere", 9_999, 9_999);
    other.cwd = std::path::PathBuf::from("/other");
    let summaries = vec![aged("here", 1, 1), other];
    let latest = latest_summary(&summaries, Some(std::path::Path::new("/tmp"))).expect("scoped");
    assert_eq!(latest.id.as_str(), "here");
}

#[test]
fn empty_sessions_hide_from_the_picker_filter() {
    let empty = summary(None, None, now_millis(), now_millis());
    assert!(
        !session_matches(&empty, "", None),
        "an empty session must not match the unfiltered picker list"
    );
    assert!(
        !session_matches(&empty, &empty.id.as_str()[..8], None),
        "an empty session must not match even its own id"
    );
    let live = summary(
        Some("real opener"),
        Some("real followup"),
        now_millis(),
        now_millis(),
    );
    assert!(session_matches(&live, "", None));
}

#[test]
fn whitespace_only_user_text_still_counts_as_empty() {
    let mut s = summary(None, None, now_millis(), now_millis());
    s.first_user_text = Some("   ".to_string());
    s.last_user_text = Some("  \n ".to_string());
    assert!(!session_matches(&s, "", None));
    assert!(latest_summary(std::slice::from_ref(&s), None).is_none());
}

#[test]
fn last_and_headless_lists_skip_empty_sessions() {
    let empty = summary(None, None, now_millis(), now_millis());
    let mut live = summary(
        Some("real opener"),
        Some("real followup"),
        now_millis().saturating_sub(60_000),
        now_millis().saturating_sub(60_000),
    );
    live.cwd = std::path::PathBuf::from("/tmp");
    let summaries = vec![empty, live];
    // --last skips the newer-but-empty session for the older live one.
    let latest = latest_summary(&summaries, None).expect("live session");
    assert_eq!(
        latest.last_user_text.as_deref(),
        Some("real followup"),
        "an empty session must never win --last, even when newest"
    );
    // Same skip the headless list applies (mirror its filter chain).
    let listed: Vec<&SessionSummary> = summaries
        .iter()
        .filter(|s| super::session_matches(s, "", None))
        .collect();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].last_user_text.as_deref(), Some("real followup"));
}

#[tokio::test]
async fn empty_sessions_exist_on_disk_but_hide_from_resume() {
    // End-to-end against the real store: a created-but-never-used session
    // is a header-only file. `list()` keeps reporting it (no data loss),
    // but no resume surface offers it — while an explicit id still loads.
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let cwd = std::env::current_dir().unwrap();
    store
        .create(crate::session_store::SessionMeta::new(
            SessionId::new("empty-hide-1"),
            now_millis(),
            cwd,
            "test-model",
        ))
        .await
        .unwrap();
    let summaries = store.list().await;
    assert_eq!(summaries.len(), 1, "the store keeps the empty session");
    assert!(summaries[0].first_user_text.is_none());
    assert!(summaries[0].last_user_text.is_none());
    assert!(!session_matches(&summaries[0], "", None));
    assert!(latest_summary(&summaries, None).is_none());
    assert_eq!(
        resolve_prefix(&store, "empty-hide-1", false)
            .await
            .unwrap()
            .as_str(),
        "empty-hide-1",
        "hiding is a listing rule, not a delete: explicit ids still resolve"
    );
}
