// temporary: appended to lib.rs tests then removed

use super::*;
use gray_core::message::ContentBlock;
use tempfile::tempdir;

#[tokio::test]
async fn create_never_truncates_existing_history() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("s1");
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "t"))
        .await
        .unwrap();
    store.append(&id, &Message::user("keep me")).await.unwrap();
    let err = store
        .create(SessionMeta::new(id.clone(), 2, "/tmp", "t"))
        .await
        .expect_err("double create must fail, not wipe");
    assert!(matches!(err, SessionError::AlreadyExists(_)));
    let (_, entries) = store.load(&id).await.unwrap();
    assert_eq!(entries.len(), 1);
}

#[tokio::test]
async fn prune_before_deletes_only_old_sessions() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let old = SessionId::new("old");
    let new = SessionId::new("new");
    store
        .create(SessionMeta::new(old.clone(), 1_000, "/tmp", "t"))
        .await
        .unwrap();
    store
        .create(SessionMeta::new(new.clone(), 9_000, "/tmp", "t"))
        .await
        .unwrap();
    let removed = store.prune_before(5_000).await.unwrap();
    assert_eq!(removed, vec![old.clone()]);
    let left = store.list().await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, new);
    assert!(matches!(
        store.load(&old).await,
        Err(SessionError::NotFound(_))
    ));
}

#[tokio::test]
async fn append_refuses_damaged_tail() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("s1");
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "t"))
        .await
        .unwrap();
    store.append(&id, &Message::user("one")).await.unwrap();
    let path = store.session_path(&id).unwrap();
    let mut raw = tokio::fs::read_to_string(&path).await.unwrap();
    raw.push_str("{torn");
    tokio::fs::write(&path, raw).await.unwrap();
    assert!(store.append(&id, &Message::user("two")).await.is_err());
    // History untouched by the refused append.
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(raw.ends_with("{torn"));
}

#[tokio::test]
async fn traversal_ids_rejected_at_storage_boundary() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    for bad in [
        "../evil",
        "/abs",
        "..\\win",
        "a/b",
        "",
        "nul\0byte",
        "sp ace",
        &"x".repeat(129),
    ] {
        let id = SessionId::new(bad);
        assert!(store.session_path(&id).is_err(), "id accepted: {bad:?}");
        assert!(store.load(&id).await.is_err(), "load accepted: {bad:?}");
        assert!(store.delete(&id).await.is_err(), "delete accepted: {bad:?}");
    }
    // UUIDs and safe legacy IDs still work.
    let id = SessionId::generate();
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "t"))
        .await
        .unwrap();
    assert!(store.load(&id).await.is_ok());
}

#[tokio::test]
async fn load_ignores_torn_final_line_but_preserves_prior_entries() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("s1"), 1, "/tmp", "test"))
        .await
        .unwrap();
    store.append(&id, &Message::user("hello")).await.unwrap();
    let path = store.session_path(&id).unwrap();
    let mut raw = tokio::fs::read_to_string(&path).await.unwrap();
    raw.push_str("{torn\n");
    tokio::fs::write(&path, raw).await.unwrap();
    let (_, entries) = store.load(&id).await.unwrap();
    assert_eq!(entries.len(), 1);
    store
        .append(&id, &Message::user("after resume"))
        .await
        .unwrap();
    assert_eq!(store.load(&id).await.unwrap().1.len(), 2);
}

#[tokio::test]
async fn device_names_rejected_at_storage_boundary() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    for bad in ["CON", "con", "PRN", "aux", "NUL", "com1", "COM9", "lpt1"] {
        let id = SessionId::new(bad);
        assert!(
            store.session_path(&id).is_err(),
            "device id accepted: {bad}"
        );
    }
    for ok in ["com0", "console", "aux1", "nully", "companion"] {
        assert!(
            store.session_path(&SessionId::new(ok)).is_ok(),
            "valid id rejected: {ok}"
        );
    }
}

#[tokio::test]
async fn list_skips_header_filename_mismatch() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("real");
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "m"))
        .await
        .unwrap();
    let src = tokio::fs::read(dir.path().join("real.jsonl"))
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("other.jsonl"), &src)
        .await
        .unwrap();
    let listed: Vec<String> = store
        .list()
        .await
        .into_iter()
        .map(|s| s.id.as_str().to_string())
        .collect();
    assert_eq!(listed, vec!["real".to_string()]);
}

#[cfg(unix)]
#[tokio::test]
async fn list_skips_symlinked_sessions() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("real");
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "m"))
        .await
        .unwrap();
    std::os::unix::fs::symlink(dir.path().join("real.jsonl"), dir.path().join("link.jsonl"))
        .unwrap();
    let listed: Vec<String> = store
        .list()
        .await
        .into_iter()
        .map(|s| s.id.as_str().to_string())
        .collect();
    assert_eq!(listed, vec!["real".to_string()]);
}

#[tokio::test]
async fn persists_turn_duration_ms() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("s1"), 1, "/tmp", "test"))
        .await
        .unwrap();
    store
        .append_with_usage_and_duration(
            &id,
            &Message::user("hi"),
            Some(gray_core::event::Usage::new(10, 5)),
            Some(6250),
        )
        .await
        .unwrap();
    let (_, entries) = store.load(&id).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].duration_ms, Some(6250));
}

#[tokio::test]
async fn legacy_entry_without_duration_loads_as_none() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("s1"), 1, "/tmp", "test"))
        .await
        .unwrap();
    let path = store.session_path(&id).unwrap();
    let mut raw = tokio::fs::read_to_string(&path).await.unwrap();
    // Legacy entry shape: no duration_ms field.
    raw.push_str(r#"{"entry_id":0,"parent_id":null,"timestamp":1,"message":{"role":"user","content":[{"type":"text","text":"hi"}]},"usage":null}"#);
    raw.push('\n');
    tokio::fs::write(&path, raw).await.unwrap();
    let (_, entries) = store.load(&id).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].duration_ms, None);
}

/// Compact -> reload roundtrip: the store must reproduce the ACTIVE
/// transcript (no duplication, compaction not undone), stay appendable
/// after the boundary, and let a second compaction supersede the first.
/// Old marker-less files load whole (covered by the pre-existing tests).
#[tokio::test]
async fn compaction_replacement_reloads_as_active_transcript() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("c1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    for t in ["one", "two", "three"] {
        store.append(&id, &Message::user(t)).await.unwrap();
    }
    let replacement = vec![Message::user("summary"), Message::assistant("ack")];
    store
        .append_compaction_replacement(&id, &replacement)
        .await
        .unwrap();
    let (_, entries) = store.load(&id).await.unwrap();
    let texts: Vec<String> = entries.iter().map(|e| e.message.text_content()).collect();
    assert_eq!(
        texts,
        vec!["summary".to_string(), "ack".to_string()],
        "reload must replay the active transcript only, got {texts:?}"
    );
    // Listing describes the post-compact session, not the buried opener.
    let summaries = store.list().await;
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].first_user_text.as_deref(), Some("summary"));
    // Post-compaction turns append after the boundary and still reload.
    store.append(&id, &Message::user("after")).await.unwrap();
    let (_, entries) = store.load(&id).await.unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[2].message.text_content(), "after");
    // A second compaction supersedes the first.
    store
        .append_compaction_replacement(&id, &[Message::user("s2")])
        .await
        .unwrap();
    let (_, entries) = store.load(&id).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message.text_content(), "s2");
}

#[tokio::test]
async fn list_reports_the_latest_user_message_and_its_timestamp() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("latest1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    for t in ["opener", "middle", "newest topic"] {
        store.append(&id, &Message::user(t)).await.unwrap();
    }
    let (_, entries) = store.load(&id).await.unwrap();
    let last_ts = entries.last().unwrap().timestamp;
    let summaries = store.list().await;
    assert_eq!(summaries.len(), 1);
    let s = &summaries[0];
    assert_eq!(
        s.first_user_text.as_deref(),
        Some("opener"),
        "the opener stays available for callers that want it"
    );
    assert_eq!(
        s.last_user_text.as_deref(),
        Some("newest topic"),
        "the list must describe the latest message, not the first: {s:?}"
    );
    assert_eq!(
        s.last_message_at, last_ts,
        "the timestamp must be when the latest message was sent"
    );
}

#[tokio::test]
async fn list_reports_latest_message_after_compaction() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("latest2"), 5, "/tmp", "m"))
        .await
        .unwrap();
    for t in ["one", "two"] {
        store.append(&id, &Message::user(t)).await.unwrap();
    }
    store
        .append_compaction_replacement(&id, &[Message::user("summary")])
        .await
        .unwrap();
    let s = &store.list().await[0];
    assert_eq!(s.last_user_text.as_deref(), Some("summary"));
    // Post-compact turns keep it current.
    store.append(&id, &Message::user("after")).await.unwrap();
    let s = &store.list().await[0];
    assert_eq!(s.last_user_text.as_deref(), Some("after"));
    assert!(s.last_message_at > 5);
}

#[tokio::test]
async fn list_falls_back_to_the_header_timestamp_without_messages() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    store
        .create(SessionMeta::new(
            SessionId::new("latest3"),
            4242,
            "/tmp",
            "m",
        ))
        .await
        .unwrap();
    let s = &store.list().await[0];
    assert_eq!(s.last_user_text, None);
    assert_eq!(
        s.last_message_at, 4242,
        "an empty session must still report a usable timestamp"
    );
}

#[tokio::test]
async fn corrupt_header_is_quarantined_on_load() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("bad1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    let path = store.session_path(&id).unwrap();
    tokio::fs::write(&path, "not json\n").await.unwrap();
    let err = store.load(&id).await.unwrap_err();
    assert!(matches!(err, SessionError::Corrupt { .. }));
    assert!(!path.exists());
    assert!(dir.path().join("bad1.corrupt-1").exists());
    assert!(store.list().await.is_empty());
}

#[tokio::test]
async fn corrupt_header_is_quarantined_on_list_and_keeps_newest_3() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let good = store
        .create(SessionMeta::new(SessionId::new("good"), 1, "/tmp", "m"))
        .await
        .unwrap();
    let bad = SessionId::new("bad2");
    let bad_path = store.session_path(&bad).unwrap();
    for _ in 0..5 {
        tokio::fs::write(&bad_path, "not json\n").await.unwrap();
        let summaries = store.list().await;
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, good);
    }
    let mut kept: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("bad2.corrupt-"))
        .collect();
    kept.sort();
    assert_eq!(
        kept,
        vec!["bad2.corrupt-3", "bad2.corrupt-4", "bad2.corrupt-5"]
    );
    assert!(!bad_path.exists());
}

#[tokio::test]
async fn load_rejects_unsupported_version() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("v1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    let path = store.session_path(&id).unwrap();
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    let mut header: serde_json::Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
    header["version"] = serde_json::json!(999);
    let mut out = serde_json::to_string(&header).unwrap();
    out.push('\n');
    for line in raw.lines().skip(1) {
        out.push_str(line);
        out.push('\n');
    }
    tokio::fs::write(&path, out).await.unwrap();
    let err = store.load(&id).await.unwrap_err();
    assert!(err.to_string().contains("unsupported"), "got: {err}");
}

#[tokio::test]
async fn load_rejects_header_id_mismatch() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("s1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    let path = store.session_path(&id).unwrap();
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    let mut header: serde_json::Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
    header["id"] = serde_json::json!("other");
    let mut out = serde_json::to_string(&header).unwrap();
    out.push('\n');
    tokio::fs::write(&path, out).await.unwrap();
    let err = store.load(&id).await.unwrap_err();
    assert!(err.to_string().contains("mismatch"), "got: {err}");
}

#[tokio::test]
async fn load_rejects_duplicate_entry_ids() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("dup1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    store.append(&id, &Message::user("a")).await.unwrap();
    store.append(&id, &Message::user("b")).await.unwrap();
    let path = store.session_path(&id).unwrap();
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    // Duplicate the last entry line verbatim (same entry_id).
    let last = raw.lines().last().unwrap().to_string();
    let mut dup = raw.clone();
    dup.push_str(&last);
    dup.push('\n');
    tokio::fs::write(&path, dup).await.unwrap();
    let err = store.load(&id).await.unwrap_err();
    assert!(err.to_string().contains("duplicate"), "got: {err}");
}

#[tokio::test]
async fn load_rejects_missing_parent() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("par1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    store.append(&id, &Message::user("a")).await.unwrap();
    store.append(&id, &Message::user("b")).await.unwrap();
    let path = store.session_path(&id).unwrap();
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    let mut lines: Vec<String> = raw.lines().map(str::to_string).collect();
    let mut entry: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    entry["parent_id"] = serde_json::json!(9999);
    *lines.last_mut().unwrap() = serde_json::to_string(&entry).unwrap();
    tokio::fs::write(&path, lines.join("\n") + "\n")
        .await
        .unwrap();
    let err = store.load(&id).await.unwrap_err();
    assert!(
        err.to_string().contains("parent") || err.to_string().contains("missing"),
        "got: {err}"
    );
}

#[tokio::test]
async fn load_rejects_parent_cycle() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("cyc1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    store.append(&id, &Message::user("a")).await.unwrap();
    store.append(&id, &Message::user("b")).await.unwrap();
    let path = store.session_path(&id).unwrap();
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    let mut lines: Vec<String> = raw.lines().map(str::to_string).collect();
    // Entry 0 (lines[1]) parent -> entry 1's id, entry 1 (lines[2]) parent -> entry 0's id.
    let mut e0: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    let mut e1: serde_json::Value = serde_json::from_str(&lines[2]).unwrap();
    let id0 = e0["entry_id"].as_u64().unwrap();
    let id1 = e1["entry_id"].as_u64().unwrap();
    e0["parent_id"] = serde_json::json!(id1);
    e1["parent_id"] = serde_json::json!(id0);
    lines[1] = serde_json::to_string(&e0).unwrap();
    lines[2] = serde_json::to_string(&e1).unwrap();
    tokio::fs::write(&path, lines.join("\n") + "\n")
        .await
        .unwrap();
    let err = store.load(&id).await.unwrap_err();
    assert!(
        err.to_string().contains("cycle") || err.to_string().contains("parent"),
        "got: {err}"
    );
}

#[tokio::test]
async fn per_session_lock_file_exists_after_append() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("lock1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    store.append(&id, &Message::user("hi")).await.unwrap();
    assert!(
        dir.path().join("lock1.lock").exists(),
        "per-session cross-process lock file must exist"
    );
}

#[tokio::test]
async fn concurrent_appends_from_two_handles_keep_unique_ids() {
    let dir = tempdir().unwrap();
    let seed = JsonlSessionStore::new(dir.path());
    let id = seed
        .create(SessionMeta::new(SessionId::new("conc1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    // Two handles = two per-instance mutexes: only a cross-process file
    // lock serializes them. 20 concurrent appends must yield 20 unique IDs.
    let sa = std::sync::Arc::new(JsonlSessionStore::new(dir.path()));
    let sb = std::sync::Arc::new(JsonlSessionStore::new(dir.path()));
    let mut js = Vec::new();
    for i in 0..10 {
        let (sa_c, id_c) = (sa.clone(), id.clone());
        let msg = Message::user(format!("a{i}"));
        js.push(tokio::spawn(async move { sa_c.append(&id_c, &msg).await }));
        let (sb_c, id_c) = (sb.clone(), id.clone());
        let msg = Message::user(format!("b{i}"));
        js.push(tokio::spawn(async move { sb_c.append(&id_c, &msg).await }));
    }
    for j in js {
        j.await.unwrap().unwrap();
    }
    let (_, entries) = sa.load(&id).await.unwrap();
    assert_eq!(entries.len(), 20);
    let mut ids: Vec<u64> = entries.iter().map(|e| e.entry_id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 20, "entry IDs must be unique under concurrency");
}

#[tokio::test]
async fn remember_recall_roundtrip() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let id = SessionId::new("remember1");
    store
        .create(SessionMeta::new(id.clone(), 1, cwd.clone(), "m"))
        .await
        .unwrap();
    // create() remembers; explicit remember is idempotent.
    store.remember(&id, &cwd).await;
    assert_eq!(store.recall(&cwd).await, Some(id.clone()));
    assert_eq!(store.recall_validated(&cwd).await, Some(id.clone()));
}

#[tokio::test]
async fn recall_returns_none_for_other_cwd_and_garbage() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let id = SessionId::new("remember2");
    store
        .create(SessionMeta::new(id.clone(), 1, a.clone(), "m"))
        .await
        .unwrap();
    // Different cwd hashes to a different pointer file.
    assert_eq!(store.recall(&b).await, None);
    // Garbage in the pointer file never surfaces.
    std::fs::write(store.remember_path_for_cwd(&a), "not\na session\n").unwrap();
    assert_eq!(store.recall(&a).await, None);
    assert_eq!(store.recall_validated(&a).await, None);
}

#[tokio::test]
async fn recall_validated_none_after_delete() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let cwd = dir.path().join("w");
    std::fs::create_dir_all(&cwd).unwrap();
    let id = SessionId::new("remember3");
    store
        .create(SessionMeta::new(id.clone(), 1, cwd.clone(), "m"))
        .await
        .unwrap();
    assert!(store.recall_validated(&cwd).await.is_some());
    store.delete(&id).await.unwrap();
    assert_eq!(store.recall_validated(&cwd).await, None);
    // Stale pointer degrades to the list fallback, never an error.
    assert!(store.list().await.is_empty());
}

#[tokio::test]
async fn sequential_appends_keep_monotonic_ids() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("seq1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    for i in 0..5u64 {
        let got = store
            .append(&id, &Message::user(format!("m{i}")))
            .await
            .unwrap();
        assert_eq!(got, i);
    }
    let (_, entries) = store.load(&id).await.unwrap();
    assert_eq!(entries.len(), 5);
}

#[tokio::test]
async fn cross_handle_append_continues_ids_via_rescan() {
    let dir = tempdir().unwrap();
    let seed = JsonlSessionStore::new(dir.path());
    let id = seed
        .create(SessionMeta::new(SessionId::new("tail1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    for i in 0..3 {
        seed.append(&id, &Message::user(format!("m{i}")))
            .await
            .unwrap();
    }
    // Fresh handle, empty cache: ids continue from the file, not 0.
    let fresh = JsonlSessionStore::new(dir.path());
    let got = fresh.append(&id, &Message::user("next")).await.unwrap();
    assert_eq!(got, 3);
}

#[tokio::test]
async fn append_still_refuses_torn_tail() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("torn1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    store.append(&id, &Message::user("ok")).await.unwrap();
    // Tear the tail: drop the final newline. The rescan sees the
    // incomplete tail and refuses the append.
    let path = store.session_path(&id).unwrap();
    let content = std::fs::read(&path).unwrap();
    assert!(content.ends_with(b"\n"));
    std::fs::write(&path, &content[..content.len() - 1]).unwrap();
    let err = store
        .append(&id, &Message::user("after tear"))
        .await
        .expect_err("torn tail must refuse");
    assert!(matches!(err, SessionError::Io(_)));
}

#[cfg(unix)]
#[tokio::test]
async fn store_dir_and_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().join("sessions"));
    let id = store
        .create(SessionMeta::new(SessionId::new("priv1"), 1, "/tmp", "m"))
        .await
        .unwrap();
    store.append(&id, &Message::user("hi")).await.unwrap();
    let dir_mode = std::fs::metadata(store.root_dir())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "store dir must be 0700, got {dir_mode:o}");
    let file_mode = std::fs::metadata(store.session_path(&id).unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        file_mode, 0o600,
        "session file must be 0600, got {file_mode:o}"
    );
}

#[tokio::test]
async fn a_torn_header_is_left_for_its_writer() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    // A creator between `create_new` and the end of its header write: the
    // first line exists but is not terminated yet.
    let id = SessionId::new("torn");
    let path = store.session_path(&id).unwrap();
    std::fs::write(&path, "{\"kind\":\"head").unwrap();
    let summaries = store.list().await;
    assert!(summaries.is_empty(), "a torn header must not be listed");
    assert!(path.exists(), "the writer's file must survive a list");
    let quarantined: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("corrupt"))
        .collect();
    assert!(quarantined.is_empty(), "{quarantined:?}");
    // A complete, unparseable header is still real corruption: it goes.
    std::fs::write(&path, "not json\n").unwrap();
    assert_eq!(store.list().await.len(), 0);
    let quarantined: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("corrupt"))
        .collect();
    assert_eq!(quarantined.len(), 1, "{quarantined:?}");
}

// ── listing reads the two ends, not the whole file ──

/// Write a session file directly: big enough that the head and tail
/// cannot both fit in one `SUMMARY_END_BYTES` slice, with a user turn at
/// each end and a wall of assistant text between. Built as bytes rather
/// than 4,000 locked appends — the reader is what is under test, not the
/// writer.
fn write_big_session(dir: &std::path::Path, id: &str, tail_turn: &str) {
    let header = serde_json::json!({
        "version": 1, "id": id, "timestamp": 1_000u64, "cwd": "/tmp", "model": "test",
    });
    let entry = |role: &str, text: String, ts: u64| {
        serde_json::json!({
            "compaction_boundary": false,
            "entry_id": ts,
            "parent_id": null,
            "timestamp": ts,
            "message": {"role": role, "content": [{"type": "text", "text": text}]},
        })
        .to_string()
    };
    let mut out = String::new();
    out.push_str(&header.to_string());
    out.push('\n');
    out.push_str(&entry("user", "the opening question".to_string(), 1_001));
    out.push('\n');
    for i in 0..4_000 {
        out.push_str(&entry(
            "assistant",
            format!("filler {i} {}", "x".repeat(200)),
            2_000 + i as u64,
        ));
        out.push('\n');
    }
    out.push_str(&entry("user", tail_turn.to_string(), 9_000));
    out.push('\n');
    std::fs::write(dir.join(format!("{id}.jsonl")), out).unwrap();
}

#[tokio::test]
async fn list_summarizes_a_large_session_from_its_two_ends() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    write_big_session(dir.path(), "big1", "the latest question");
    let size = std::fs::metadata(dir.path().join("big1.jsonl"))
        .unwrap()
        .len();
    assert!(
        size > SUMMARY_END_BYTES * 2,
        "fixture must exceed both slices: {size}"
    );

    let listed = store.list().await;
    assert_eq!(listed.len(), 1);
    let s = &listed[0];
    assert_eq!(s.id, SessionId::new("big1"));
    assert_eq!(s.started_at, 1_000);
    assert_eq!(
        s.first_user_text.as_deref(),
        Some("the opening question"),
        "the first turn lives in the head"
    );
    assert_eq!(
        s.last_user_text.as_deref(),
        Some("the latest question"),
        "the newest turn lives in the tail"
    );
    assert!(
        s.last_message_at >= s.started_at,
        "activity time comes from the tail, not the header"
    );
}

#[tokio::test]
async fn listing_order_is_newest_activity_first() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    // Started oldest-first, but the middle session is touched last: the
    // row's age column must come out monotonic.
    for (id, started) in [("old", 100_u64), ("mid", 200), ("new", 300)] {
        let sid = SessionId::new(id);
        store
            .create(SessionMeta::new(sid.clone(), started, "/tmp", "test"))
            .await
            .unwrap();
        store.append(&sid, &Message::user(id)).await.unwrap();
    }
    let listed = store.list().await;
    let ages: Vec<u64> = listed.iter().map(|s| s.last_message_at).collect();
    let mut descending = ages.clone();
    descending.sort_by(|a, b| b.cmp(a));
    assert_eq!(ages, descending, "list must be ordered by the age it shows");
}

#[tokio::test]
async fn a_small_session_is_still_summarized_exactly() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let sid = SessionId::new("small");
    store
        .create(SessionMeta::new(sid.clone(), 7, "/tmp", "test"))
        .await
        .unwrap();
    store
        .append(&sid, &Message::user("only question"))
        .await
        .unwrap();
    store
        .append(&sid, &Message::assistant("only answer"))
        .await
        .unwrap();
    let listed = store.list().await;
    assert_eq!(listed[0].first_user_text.as_deref(), Some("only question"));
    assert_eq!(listed[0].last_user_text.as_deref(), Some("only question"));
}

#[tokio::test]
async fn a_compaction_boundary_in_the_tail_resets_the_preview() {
    let dir = tempdir().unwrap();
    write_big_session(dir.path(), "compacted", "after compaction");
    // A boundary just before the last turn: what the head remembered
    // before it is superseded.
    let path = dir.path().join("compacted.jsonl");
    let mut text = std::fs::read_to_string(&path).unwrap();
    let boundary = serde_json::json!({
        "compaction_boundary": true,
        "entry_id": 8_999,
        "parent_id": null,
        "timestamp": 8_999,
        "message": {"role": "system", "content": [{"type": "text", "text": "compaction boundary"}]},
    })
    .to_string();
    let last_two = text.rfind('\n').unwrap();
    let cut = text[..last_two].rfind('\n').unwrap() + 1;
    text.insert_str(cut, &format!("{boundary}\n"));
    std::fs::write(&path, text).unwrap();

    let store = JsonlSessionStore::new(dir.path());
    let listed = store.list().await;
    assert_eq!(
        listed[0].last_user_text.as_deref(),
        Some("after compaction"),
        "the tail's boundary supersedes what the head remembered"
    );
}

#[tokio::test]
async fn a_session_being_written_is_not_quarantined_by_the_tail_read() {
    // A creator streams the header into a create_new file: the first line
    // has no newline yet. Listing must skip it, never rename it away.
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    std::fs::write(
        dir.path().join("halfwritten.jsonl"),
        br#"{"id":"halfwritten","timestamp":1,"cwd":"/tmp","model":"test"}"#,
    )
    .unwrap();
    let listed = store.list().await;
    assert!(listed.is_empty(), "{listed:?}");
    assert!(
        dir.path().join("halfwritten.jsonl").exists(),
        "a mid-write session must survive listing"
    );
}

/// `/undo` drops the tail of the conversation and hands it back, so the REPL
/// can re-send it. The kept entries must still form a valid chain afterwards,
/// which is what the renumbering buys.
#[tokio::test]
async fn rewind_drops_the_tail_and_renumbers_the_chain() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("s1");
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "t"))
        .await
        .unwrap();
    store.append(&id, &Message::user("first")).await.unwrap();
    store
        .append(&id, &Message::assistant("answer"))
        .await
        .unwrap();
    store.append(&id, &Message::user("second")).await.unwrap();
    store
        .append(&id, &Message::assistant("wrong"))
        .await
        .unwrap();

    let dropped = store.rewind(&id, 2).await.unwrap();
    assert_eq!(dropped.len(), 2);
    assert_eq!(dropped[0].message.text_content(), "second");

    let (_, entries) = store.load(&id).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].message.text_content(), "answer");
    // Ids stay verbatim: a tail truncation never invalidates the chain, which
    // only ever points backwards.
    assert_eq!(entries[0].entry_id, 0);
    assert_eq!(entries[0].parent_id, None);
    assert_eq!(entries[1].entry_id, 1);
    assert_eq!(entries[1].parent_id, Some(0));
}

/// After a compaction the file holds the superseded prefix, a boundary marker
/// and the active transcript. Rewinding the active tail must not delete the
/// marker: that is what stops the pre-compaction history from coming back.
#[tokio::test]
async fn rewind_keeps_the_compaction_marker_and_its_superseded_prefix() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("s1");
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "t"))
        .await
        .unwrap();
    store.append(&id, &Message::user("ancient")).await.unwrap();
    store
        .append(&id, &Message::assistant("old answer"))
        .await
        .unwrap();
    store
        .append_compaction_replacement(
            &id,
            &[
                Message::user("summary of the ancient turn"),
                Message::assistant("carrying on"),
            ],
        )
        .await
        .unwrap();
    store.append(&id, &Message::user("fresh")).await.unwrap();

    let before = store.load(&id).await.unwrap().1;
    assert_eq!(before.len(), 3, "only the active transcript is visible");

    let dropped = store.rewind(&id, 2).await.unwrap();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].message.text_content(), "fresh");

    let (_, after) = store.load(&id).await.unwrap();
    assert_eq!(after.len(), 2, "the active tail shrank by one");
    assert_eq!(
        after[0].message.text_content(),
        "summary of the ancient turn",
        "the marker still supersedes the old history"
    );
    assert!(!after.iter().any(|e| e.message.text_content() == "ancient"));
}

/// A rewind is destructive by design, so the complete original must survive in
/// `archive/` — that file is the only way back.
#[tokio::test]
async fn rewind_archives_the_original_file() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("s1");
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "t"))
        .await
        .unwrap();
    store.append(&id, &Message::user("first")).await.unwrap();
    store
        .append(&id, &Message::assistant("answer"))
        .await
        .unwrap();
    store.append(&id, &Message::user("second")).await.unwrap();

    store.rewind(&id, 1).await.unwrap();

    let archive = std::fs::read_dir(dir.path().join("archive"))
        .expect("archive dir")
        .count();
    assert_eq!(archive, 1, "the pre-undo file must be kept");
    let archived = std::fs::read_to_string(
        std::fs::read_dir(dir.path().join("archive"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .unwrap();
    assert!(
        archived.contains("second"),
        "the archive holds the dropped turn, not the truncated file"
    );
}

/// Nothing to drop is not an error: `/undo` twice in a row says so rather than
/// failing, and `keep >= len` must leave the file untouched (no archive).
#[tokio::test]
async fn rewind_past_the_start_drops_nothing() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("s1");
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "t"))
        .await
        .unwrap();
    store.append(&id, &Message::user("only")).await.unwrap();

    assert!(store.rewind(&id, 5).await.unwrap().is_empty());
    assert!(store.rewind(&id, 1).await.unwrap().is_empty());
    assert!(
        !dir.path().join("archive").exists(),
        "no rewrite, no archive"
    );
}

/// Deterministic xorshift64: the property sweep below must be reproducible in
/// CI (a failing seed has to reproduce), and the crate takes no rng dependency
/// for a test.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// One varied turn: the block kinds that actually reach a session file, with
/// the awkward payloads (quotes, newlines, unicode, JSON metacharacters, empty
/// tool args) that a JSONL round-trip has to survive verbatim.
fn random_message(rng: &mut Rng, i: usize) -> Message {
    let role = if rng.below(2) == 0 {
        Role::User
    } else {
        Role::Assistant
    };
    let mut content = Vec::new();
    let kinds = rng.below(5);
    for k in 0..=kinds {
        match k {
            0 => content.push(ContentBlock::text(format!(
                "t{i}-{k} \"quoted\"\nnewline\u{2028}sep — ünïcode \u{1F600}"
            ))),
            1 => content.push(ContentBlock::Thinking {
                text: format!("think {i}"),
                encrypted_content: Some(format!("enc\u{2028}{i}-{k}")),
                item_id: Some(format!("item-{i}-{k}")),
                model: Some("m".to_string()),
            }),
            2 => content.push(ContentBlock::ToolUse {
                id: format!("call-{i}-{k}"),
                name: "bash".to_string(),
                args: serde_json::json!({"command": "echo \"hi\\n\"", "n": i}),
            }),
            3 => content.push(ContentBlock::ToolResult {
                id: format!("call-{i}-{k}"),
                content: format!("out {i} \"quoted\" }}{{"),
                is_error: i.is_multiple_of(3),
            }),
            _ => content.push(ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            }),
        }
    }
    Message::new(role, content)
}

/// The invariant that makes a resumable session actually resumable: whatever was
/// appended must come back out of `load` unchanged, once, in order — and a
/// compaction must leave exactly the post-boundary transcript replayed, nothing
/// more, nothing less. Every existing test here covers one hand-picked shape;
/// this sweeps hundreds of random ones.
#[tokio::test]
async fn random_session_log_replays_exactly_what_was_appended() {
    for seed in 1..=120u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let id = store
            .create(SessionMeta::new(
                SessionId::new(format!("r{seed}")),
                1,
                "/tmp",
                "m",
            ))
            .await
            .unwrap();

        // Append a random transcript, remembering it independently.
        let mut appended: Vec<Message> = Vec::new();
        for i in 0..rng.below(12) {
            let msg = random_message(&mut rng, i as usize);
            store.append(&id, &msg).await.unwrap();
            appended.push(msg);
        }
        // Zero or more compactions, each superseding everything before it.
        let compactions = rng.below(3);
        let mut active: Vec<Message> = appended;
        for c in 0..compactions {
            let mut replacement = Vec::new();
            for i in 0..rng.below(4) {
                replacement.push(random_message(&mut rng, 100 + i as usize));
            }
            store
                .append_compaction_replacement(&id, &replacement)
                .await
                .unwrap();
            active = replacement;
            // Turns appended after a boundary are ordinary history again.
            if rng.below(2) == 0 {
                let msg = random_message(&mut rng, 200 + c as usize);
                store.append(&id, &msg).await.unwrap();
                active.push(msg);
            }
        }

        let (_, entries) = store.load(&id).await.unwrap();
        let replayed: Vec<&Message> = entries.iter().map(|e| &e.message).collect();
        assert_eq!(
            replayed,
            active.iter().collect::<Vec<_>>(),
            "seed {seed}: replay diverged (compactions={compactions})"
        );

        // Sequence ids strictly increase across the active window, and the
        // graph survives a second read with no drift from tail repair.
        for pair in entries.windows(2) {
            assert!(
                pair[1].entry_id > pair[0].entry_id,
                "seed {seed}: entry ids did not increase"
            );
        }
        let (_, again) = store.load(&id).await.unwrap();
        assert_eq!(again, entries, "seed {seed}: second load drifted");
    }
}

/// A crash-interrupted final write must cost exactly that one entry, and the
/// file must stay appendable afterwards. Cut at every byte offset of the final
/// record rather than one hand-picked spot: a tear can land anywhere, and the
/// repair must drop the torn entry without disturbing the ones before it.
///
/// Two cut classes are covered deliberately. A cut on a character boundary
/// leaves valid UTF-8; a cut *inside* a multi-byte character leaves invalid
/// UTF-8 in the tail and must be treated the same way, because a crash does not
/// know where the character boundaries are. Invalid bytes that precede a
/// complete line are real mid-file corruption and still error (covered by
/// `invalid_utf8_before_the_final_record_is_still_corruption`).
///
/// The last position (`cut == record length`) is distinct from the rest: the
/// record is complete but its newline is missing, which exercises the
/// missing-trailing-newline repair and must keep both entries.
#[tokio::test]
async fn torn_final_line_at_every_offset_costs_only_that_entry() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("torn"), 1, "/tmp", "m"))
        .await
        .unwrap();
    store.append(&id, &Message::user("first")).await.unwrap();
    // A real second entry, written through the store, is the record under test.
    // It must carry multi-byte characters, or the mid-UTF-8 cut class is empty
    // and the sweep below silently loses half its meaning.
    store
        .append(&id, &Message::user("second \u{00e9}\u{4e2d}\u{1F600} tail"))
        .await
        .unwrap();
    let path = store.session_path(&id).unwrap();
    let bytes = tokio::fs::read(&path).await.unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'), "records end with a newline");
    let body = &bytes[..bytes.len() - 1];
    let record_start = match body.iter().rev().position(|&b| b == b'\n') {
        Some(p) => body.len() - p,
        None => 0,
    };
    let record = &bytes[record_start..bytes.len() - 1];
    assert!(
        std::str::from_utf8(record).is_ok(),
        "the record under test must be valid UTF-8 on its own"
    );

    let mut boundary_cuts = 0;
    let mut mid_utf8_cuts = 0;
    for cut in 1..record.len() {
        // Everything before the final record, plus `cut` bytes of it.
        let damaged = &bytes[..record_start + cut];
        tokio::fs::write(&path, damaged).await.unwrap();
        let mid_utf8 = std::str::from_utf8(damaged).is_err();
        let (_, entries) = store.load(&id).await.unwrap_or_else(|e| {
            panic!(
                "cut {cut} (mid-utf8={mid_utf8}): a torn final record must cost only itself, got {e}"
            )
        });
        assert_eq!(
            entries.len(),
            1,
            "cut {cut} (mid-utf8={mid_utf8}): a torn final record must cost only itself"
        );
        assert_eq!(entries[0].message.text_content(), "first");
        // The whole point of the repair: the session is still appendable.
        store
            .append(&id, &Message::user("after"))
            .await
            .unwrap_or_else(|e| panic!("cut {cut}: append after repair failed: {e}"));
        assert_eq!(
            store.load(&id).await.unwrap().1.len(),
            2,
            "cut {cut}: repair dropped an entry"
        );
        if mid_utf8 {
            mid_utf8_cuts += 1;
        } else {
            boundary_cuts += 1;
        }
    }
    // The sweep is only meaningful if it actually exercises both classes, so a
    // record with no multi-byte character (which would make the mid-UTF8 class
    // empty) is a broken fixture, not a lucky pass.
    assert!(
        boundary_cuts > 0,
        "no character-boundary cuts were exercised"
    );
    assert!(
        mid_utf8_cuts > 0,
        "no mid-UTF-8 cuts were exercised; the torn record must contain a multi-byte character"
    );

    // Complete record, missing its newline: a different repair branch, and it
    // must keep both entries.
    let damaged = &bytes[..record_start + record.len()];
    tokio::fs::write(&path, damaged).await.unwrap();
    let (_, entries) = store.load(&id).await.unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| e.message.text_content())
            .collect::<Vec<_>>(),
        vec![
            "first".to_string(),
            "second \u{00e9}\u{4e2d}\u{1F600} tail".to_string()
        ],
        "a complete record with no trailing newline must not lose the entry"
    );
}

/// The tail-tear rule must not swallow real corruption: invalid UTF-8 that sits
/// *before* a complete record means the file was not simply truncated mid-write,
/// so loading it stays an error rather than a silent repair.
#[tokio::test]
async fn invalid_utf8_before_the_final_record_is_still_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = store
        .create(SessionMeta::new(SessionId::new("mid"), 1, "/tmp", "m"))
        .await
        .unwrap();
    store
        .append(&id, &Message::user("first \u{00e9}"))
        .await
        .unwrap();
    store.append(&id, &Message::user("second")).await.unwrap();
    let path = store.session_path(&id).unwrap();
    let bytes = tokio::fs::read(&path).await.unwrap();
    let body = &bytes[..bytes.len() - 1];
    let last_start = match body.iter().rev().position(|&b| b == b'\n') {
        Some(p) => body.len() - p,
        None => 0,
    };
    let first_line_end = match body.iter().position(|&b| b == b'\n') {
        Some(p) => p,
        None => body.len(),
    };
    assert!(
        first_line_end < last_start,
        "need a record before the last one"
    );
    // Invalid bytes inside the first record, with a complete record after it.
    let mut damaged = Vec::new();
    damaged.extend_from_slice(&bytes[..first_line_end]);
    damaged.extend_from_slice(&[0xFF, 0xFE]);
    damaged.extend_from_slice(&bytes[first_line_end..]);
    tokio::fs::write(&path, &damaged).await.unwrap();
    assert!(
        store.load(&id).await.is_err(),
        "invalid UTF-8 before a complete record is corruption, not a torn tail"
    );
}

#[test]
fn generate_makes_three_word_names_from_the_lists() {
    for _ in 0..64 {
        let id = SessionId::generate();
        let s = id.as_str();
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(parts.len(), 3, "{s}");
        assert!(QUALIFIER_WORDS.contains(&parts[0]), "{s}");
        assert!(MIDDLE_WORDS.contains(&parts[1]), "{s}");
        assert!(PHENOMENON_WORDS.contains(&parts[2]), "{s}");
        assert!(valid_session_id(s), "{s}");
    }
    // No duplicates inside a list: a word maps to exactly one slot.
    for list in [QUALIFIER_WORDS, MIDDLE_WORDS, PHENOMENON_WORDS] {
        let mut sorted = list.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), list.len());
    }
}

#[tokio::test]
async fn fresh_id_skips_taken_names_and_salts_when_exhausted() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    store
        .create(SessionMeta::new(
            SessionId::new("taken-one"),
            1,
            "/tmp",
            "t",
        ))
        .await
        .unwrap();
    let mut calls = 0;
    let id = store
        .fresh_id_with(|| {
            calls += 1;
            SessionId::new(if calls == 1 { "taken-one" } else { "free-two" })
        })
        .await;
    assert_eq!(id.as_str(), "free-two");
    assert_eq!(calls, 2);

    // Every candidate taken: after FRESH_ID_ATTEMPTS the id is hex-salted.
    store
        .create(SessionMeta::new(
            SessionId::new("taken-always"),
            1,
            "/tmp",
            "t",
        ))
        .await
        .unwrap();
    let salted = store.fresh_id_with(|| SessionId::new("taken-always")).await;
    let suffix = salted
        .as_str()
        .strip_prefix("taken-always-")
        .unwrap_or_else(|| panic!("{}", salted.as_str()));
    assert_eq!(suffix.len(), 4, "{suffix}");
    assert!(suffix.bytes().all(|b| b.is_ascii_hexdigit()), "{suffix}");
    assert!(valid_session_id(salted.as_str()));
}

// UNRUN (cargo test banned under X): run in TTY/CI.
// Open-locks: `<id>.open` marks the process that owns a live session.

#[tokio::test]
async fn open_lock_blocks_second_owner_and_names_holder_pid() {
    let dir = tempdir().unwrap();
    // Two store handles = two "processes" as far as the flock is
    // concerned (the lock lives on the open file description, not the
    // store instance).
    let sa = JsonlSessionStore::new(dir.path());
    let sb = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("open1");
    sa.create(SessionMeta::new(id.clone(), 1, "/tmp", "m"))
        .await
        .unwrap();

    let guard = sa.acquire_open(&id).await.unwrap();
    assert!(sa.open_lock_held(&id).await);
    assert!(sb.open_lock_held(&id).await);

    // The in-process registry would merge sb's acquire into sa's lock
    // (by design — same process, same owner). Stash it so sb exercises
    // the OS flock path a genuine second process always hits, then
    // restore — sa's re-acquire below must merge again.
    let stashed = std::mem::take(&mut *OPEN_LOCKS.lock().expect("open-lock registry poisoned"));
    let err = sb.acquire_open(&id).await.unwrap_err();
    *OPEN_LOCKS.lock().expect("open-lock registry poisoned") = stashed;
    match err {
        SessionError::Locked { id: locked_id, pid } => {
            assert_eq!(locked_id, id);
            // The holder stamped its own pid for the error to name.
            assert_eq!(pid, Some(std::process::id()));
        }
        other => panic!("expected Locked, got {other:?}"),
    }
    let text = SessionError::Locked {
        id: id.clone(),
        pid: Some(10087),
    }
    .to_string();
    assert_eq!(
        text,
        "session 'open1' is already open in another process (PID 10087). \
         Close the other instance before opening it here."
    );

    // Same-process re-acquire shares the held lock (registry), never
    // reports Locked on ourselves.
    let again = sa.acquire_open(&id).await.unwrap();
    drop(guard);
    assert!(sb.open_lock_held(&id).await, "shared guard keeps the claim");
    drop(again);
    assert!(
        !sb.open_lock_held(&id).await,
        "last clone dropping releases the flock"
    );
    sb.acquire_open(&id).await.unwrap();
}

#[tokio::test]
async fn open_lock_reacquire_after_release_overwrites_stale_pid() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("open2");
    store
        .create(SessionMeta::new(id.clone(), 1, "/tmp", "m"))
        .await
        .unwrap();
    {
        let _g = store.acquire_open(&id).await.unwrap();
    }
    assert!(!store.open_lock_held(&id).await);
    // The released file's stale PID is never mistaken for a live holder.
    let _g = store.acquire_open(&id).await.unwrap();
    assert!(store.open_lock_held(&id).await);
}

#[tokio::test]
async fn delete_and_prune_leave_open_sessions_alone() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let open = SessionId::new("open3");
    let old = SessionId::new("open4");
    for id in [&open, &old] {
        store
            .create(SessionMeta::new(id.clone(), 1, "/tmp", "m"))
            .await
            .unwrap();
    }
    let guard = store.acquire_open(&open).await.unwrap();

    assert!(matches!(
        store.delete(&open).await,
        Err(SessionError::Locked { .. })
    ));
    // Prune removes the old unlocked session, skips the live one.
    let removed = store.prune_before(u64::MAX).await.unwrap();
    assert_eq!(removed, vec![old]);
    assert!(dir.path().join("open3.jsonl").exists());

    drop(guard);
    let removed = store.prune_before(u64::MAX).await.unwrap();
    assert_eq!(removed, vec![open]);
}

#[tokio::test]
async fn open_lock_probe_does_not_create_files() {
    let dir = tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path());
    let id = SessionId::new("open5");
    assert!(!store.open_lock_held(&id).await);
    assert!(
        !dir.path().join("open5.open").exists(),
        "the advisory probe must not mint lock files"
    );
}
