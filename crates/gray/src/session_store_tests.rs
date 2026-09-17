// temporary: appended to lib.rs tests then removed

use super::*;
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
