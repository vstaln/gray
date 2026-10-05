use super::*;

fn setup() -> (tempfile::TempDir, MemoryStore) {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = MemoryStore::new(dir.path(), dir.path()).unwrap();
    (dir, store)
}

#[test]
fn large_entries_round_trip_without_a_cap() {
    let (_dir, store) = setup();
    // Far past the old 2 KiB / 4 KiB budgets: no cap remains.
    let big = "é".repeat(20_000);
    assert!(store.set(Scope::User, "k", &big).unwrap());
    assert!(store.list(Scope::User).unwrap().contains(&big));
    let huge = "y".repeat(120_000);
    std::fs::write(store.path(Scope::User), format!("- k: {huge}\n")).unwrap();
    assert!(store.list(Scope::User).unwrap().contains(&huge));
    assert!(store.set(Scope::User, "k2", "small").unwrap());
    let after = std::fs::read(store.path(Scope::User)).unwrap();
    assert!(after.len() > 120_000);
    assert_eq!(after.last(), Some(&b'\n'));
}

#[test]
fn rationale_convention_round_trips_through_the_parser() {
    let (_dir, store) = setup();
    // The paper's entry shape lives inside the one-line text, so quotes,
    // semicolons and parens must survive validate_text + parse + render.
    let text = "Run tests per crate. Why: \"a 3-crate run let a clippy failure reach CI\" (recurs: 1); falsified: nothing yet; replaces: \"old rule\"";
    assert!(store.set(Scope::Project, "gate", text).unwrap());
    assert_eq!(
        store.get(Scope::Project, "gate").unwrap().as_deref(),
        Some(text)
    );
    assert!(store.list(Scope::Project).unwrap().contains(text));
}

#[test]
fn audit_flags_missing_rationale_falsified_outcomes_and_duplicates() {
    let (_dir, store) = setup();
    store
        .set(
            Scope::Project,
            "solid",
            "Decision. Why: \"the build broke\" (recurs: 0)",
        )
        .unwrap();
    store
        .set(Scope::Project, "bare", "A bare decision.")
        .unwrap();
    store
        .set(
            Scope::Project,
            "dead",
            "Old. Why: \"x\"; falsified: it did not help",
        )
        .unwrap();
    store
        .set(Scope::Project, "twin-a", "Same. Why: \"y\"")
        .unwrap();
    store
        .set(Scope::Project, "twin-b", "same.  Why: \"y\"")
        .unwrap();
    let report = store.audit(Scope::Project).unwrap();
    assert!(report.contains("bare: no Why recorded"), "{report}");
    assert!(
        report.contains("dead: records a falsified outcome"),
        "{report}"
    );
    assert!(
        report.contains("twin-a, twin-b: duplicate target"),
        "{report}"
    );
    assert!(
        !report.contains("solid:"),
        "a clean entry must not be flagged: {report}"
    );
    assert!(report.contains("This audit deletes nothing"), "{report}");
    // Auditing never mutates.
    assert_eq!(store.audit(Scope::Project).unwrap(), report);
}

#[test]
fn audit_does_not_flag_a_compliant_falsified_field() {
    let (_dir, store) = setup();
    // The convention's compliant value: nothing has been contradicted yet.
    store
        .set(
            Scope::Project,
            "gate",
            "Run tests per crate. Why: \"a 3-crate run let a failure reach CI\" (recurs: 1); falsified: nothing yet",
        )
        .unwrap();
    let report = store.audit(Scope::Project).unwrap();
    assert!(
        !report.contains("- gate: records a falsified outcome"),
        "a compliant entry was flagged as recurring: {report}"
    );
    // A real failed attempt is still flagged.
    store
        .set(
            Scope::Project,
            "dead",
            "Old. Why: \"x\"; falsified: bumping the timeout did not help",
        )
        .unwrap();
    let report = store.audit(Scope::Project).unwrap();
    assert!(
        report.contains("dead: records a falsified outcome"),
        "{report}"
    );
    // The audit lists findings only, so a clean entry appears nowhere in it.
    assert!(!report.contains("gate"), "{report}");
}

#[test]
fn audit_flags_duplicates_by_decision_not_by_rationale() {
    let (_dir, store) = setup();
    store
        .set(
            Scope::Project,
            "a",
            "Never push main. Why: \"main is protected\" (recurs: 0)",
        )
        .unwrap();
    store
        .set(
            Scope::Project,
            "b",
            "never push MAIN. Why: \"the user said so on a different day\" (recurs: 2)",
        )
        .unwrap();
    let report = store.audit(Scope::Project).unwrap();
    assert!(
        report.contains("a, b: duplicate target"),
        "same aim, different rationale, must still be a duplicate: {report}"
    );
}

#[test]
fn concurrent_scope_saves_do_not_clobber_the_growth_record() {
    let (_dir, store) = setup();
    let store = std::sync::Arc::new(store);
    let mut handles = Vec::new();
    for i in 0..8 {
        let store = std::sync::Arc::clone(&store);
        handles.push(std::thread::spawn(move || {
            let scope = if i % 2 == 0 {
                Scope::User
            } else {
                Scope::Project
            };
            store
                .set(scope, &format!("k{i}"), &format!("Decision {i}."))
                .unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    // Both scopes must still be recorded: a racy read-modify-write on the
    // shared growth file would drop one of them.
    assert!(store.growth(Scope::User).is_some(), "user scope lost");
    assert!(store.growth(Scope::Project).is_some(), "project scope lost");
}

#[test]
fn growth_streak_warns_after_repeated_adds_and_a_removal_resets_it() {
    let (_dir, store) = setup();
    assert_eq!(store.growth_warning(Scope::Project), None);
    store.set(Scope::Project, "a", "One.").unwrap();
    store.set(Scope::Project, "b", "Two.").unwrap();
    assert_eq!(
        store.growth_warning(Scope::Project),
        None,
        "two net adds must not trip it"
    );
    store.set(Scope::Project, "c", "Three.").unwrap();
    let warning = store
        .growth_warning(Scope::Project)
        .expect("three net adds must warn");
    assert!(warning.contains("3 entries"), "{warning}");
    assert!(warning.contains("gray memory audit"), "{warning}");
    // A no-op edit changes nothing and must not extend the streak.
    assert!(!store.edit(Scope::Project, "c", "Three.").unwrap());
    // A removal resets it.
    store.remove(Scope::Project, "a").unwrap();
    assert_eq!(store.growth_warning(Scope::Project), None);
}

#[test]
fn show_edit_clear_manage_entries() {
    let (_dir, store) = setup();
    assert_eq!(store.get(Scope::Project, "k").unwrap(), None);
    assert!(
        store.edit(Scope::Project, "k", "text").is_err(),
        "edit must never create an entry"
    );
    store.set(Scope::Project, "k", "first").unwrap();
    assert_eq!(
        store.get(Scope::Project, "k").unwrap().as_deref(),
        Some("first")
    );
    store.edit(Scope::Project, "k", "second").unwrap();
    assert_eq!(
        store.get(Scope::Project, "k").unwrap().as_deref(),
        Some("second")
    );
    store.set(Scope::Project, "other", "text").unwrap();
    assert_eq!(store.clear(Scope::Project).unwrap(), 2);
    assert_eq!(store.list(Scope::Project).unwrap(), "");
    assert_eq!(store.clear(Scope::Project).unwrap(), 0);
}

#[test]
fn malformed_files_are_never_overwritten() {
    let (_dir, store) = setup();
    private_dir(&store.root).unwrap();
    let path = store.path(Scope::Project);
    for bad in [
        b"not our format\n".to_vec(),
        b"- k: a\n- k: b\n".to_vec(),
        vec![b'x'; 4097],
        vec![255],
    ] {
        std::fs::write(&path, &bad).unwrap();
        assert!(store.list(Scope::Project).is_err());
        assert!(store.set(Scope::Project, "k", "Good fact.").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bad);
    }
}

#[test]
fn correction_can_match_another_entry_without_keeping_stale_fact() {
    let (_dir, store) = setup();
    store.set(Scope::User, "a", "Old.").unwrap();
    store.set(Scope::User, "b", "New.").unwrap();
    store.set(Scope::User, "a", "New.").unwrap();
    assert!(!store.list(Scope::User).unwrap().contains("Old."));
    assert!(!store.set(Scope::User, "c", "New.").unwrap());
}

#[test]
fn text_validation_preserves_prose_and_paths_but_rejects_known_secrets() {
    for good in [
        "Use Rust.",
        "Project at /home/user/app uses SQLite.",
        "API key rotation is monthly.",
        "喜欢简短回答。",
    ] {
        validate_text(good).unwrap();
    }
    for bad in [
        "",
        "\n",
        "a\r\nb",
        "a\u{200b}b",
        "a\u{202e}b",
        "TOKEN=12345678901234567890",
        "sk-fake12345678901234567890",
    ] {
        assert!(validate_text(bad).is_err(), "accepted invalid input");
    }
}

#[cfg(unix)]
#[test]
fn private_modes_and_symlink_rejection() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (dir, store) = setup();
    store.set(Scope::User, "style", "Concise.").unwrap();
    assert_eq!(
        std::fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let path = store.path(Scope::User);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let outside = dir.path().join("outside");
    std::fs::write(&outside, "untouched").unwrap();
    std::fs::remove_file(&path).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(store.list(Scope::User).is_err());
    assert!(store.set(Scope::User, "style", "Changed.").is_err());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "untouched");
    std::fs::remove_file(&path).unwrap();
    let lock_path = path.with_extension("lock");
    std::fs::remove_file(&lock_path).unwrap();
    symlink(dir.path().join("absent"), lock_path).unwrap();
    assert!(store.set(Scope::User, "style", "Changed.").is_err());
}

#[test]
fn snapshot_is_frozen_on_rebuild_and_resume_but_new_session_gets_updates() {
    let (_dir, store) = setup();
    store.set(Scope::User, "style", "Short.").unwrap();
    let sid = uuid::Uuid::new_v4().to_string();
    let original = store.snapshot(Some(&sid)).unwrap();
    store.set(Scope::User, "style", "Detailed.").unwrap();
    assert_eq!(store.snapshot(Some(&sid)).unwrap(), original);
    let resumed =
        MemoryStore::new(store.root.parent().unwrap(), store.root.parent().unwrap()).unwrap();
    assert_eq!(resumed.snapshot(Some(&sid)).unwrap(), original);
    assert!(
        store
            .snapshot(Some(&uuid::Uuid::new_v4().to_string()))
            .unwrap()
            .contains("Detailed.")
    );
    assert!(store.snapshot(None).unwrap().contains("Detailed."));
    assert!(!original.contains("Detailed."));
}

#[test]
fn invalid_snapshot_and_wrong_project_fail_closed() {
    let (dir, store) = setup();
    assert!(store.snapshot(Some("../escape")).is_err());
    let sid = uuid::Uuid::new_v4().to_string();
    store.snapshot(Some(&sid)).unwrap();
    let other = dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let other_store = MemoryStore::new(dir.path(), &other).unwrap();
    assert!(other_store.snapshot(Some(&sid)).is_err());
    let path = store.root.join("snapshots").join(format!("{sid}.json"));
    std::fs::write(path, "not json").unwrap();
    assert!(store.snapshot(Some(&sid)).is_err());
}

#[test]
fn provenance_round_trips_and_never_reaches_the_served_text() {
    let (dir, store) = setup();
    assert!(store.set(Scope::Project, "gate", "run tests").unwrap());
    let on_disk = std::fs::read_to_string(store.path(Scope::Project)).unwrap();
    // The trailer rides on the line, so the store stays hand-editable.
    assert!(on_disk.contains("<!-- gray:saved="), "{on_disk}");
    assert!(on_disk.contains("source="), "{on_disk}");
    // ...and never reaches what the model is served.
    let served = store.list(Scope::Project).unwrap();
    assert!(!served.contains("<!-- gray:"), "{served}");
    assert!(served.contains("- gate: run tests\n"), "{served}");
    // A second save keeps the trailer rather than accumulating a second one.
    assert!(
        store
            .set(Scope::Project, "gate", "run tests again")
            .unwrap()
    );
    let again = std::fs::read_to_string(store.path(Scope::Project)).unwrap();
    assert_eq!(again.matches("<!-- gray:").count(), 1, "{again}");
    let _ = dir;
}

#[test]
fn entries_without_a_trailer_still_parse() {
    let (_dir, store) = setup();
    // A store written before provenance existed must load unchanged. The
    // directory only exists once a write path has run, so establish it first.
    assert!(store.set(Scope::User, "seed", "x").unwrap());
    std::fs::write(store.path(Scope::User), "- old: kept\n").unwrap();
    assert_eq!(store.list(Scope::User).unwrap(), "- old: kept\n");
    assert_eq!(
        store.get(Scope::User, "old").unwrap().as_deref(),
        Some("kept")
    );
    // And an unrelated save must not invent a date for it.
    assert!(store.set(Scope::User, "new", "added").unwrap());
    let on_disk = std::fs::read_to_string(store.path(Scope::User)).unwrap();
    assert!(on_disk.contains("- old: kept\n"), "{on_disk}");
    assert!(
        on_disk.contains("- new: added <!-- gray:saved="),
        "{on_disk}"
    );
}

#[test]
fn a_hand_edited_line_loses_its_provenance() {
    let (_dir, store) = setup();
    assert!(store.set(Scope::User, "k", "written by gray").unwrap());
    // A human rewrites the line and drops the trailer: the origin is honestly
    // unknown, so it stays unstamped instead of being guessed at.
    std::fs::write(store.path(Scope::User), "- k: rewritten by hand\n").unwrap();
    assert!(store.set(Scope::User, "other", "added").unwrap());
    let on_disk = std::fs::read_to_string(store.path(Scope::User)).unwrap();
    assert!(on_disk.contains("- k: rewritten by hand\n"), "{on_disk}");
    assert!(
        on_disk.contains("- other: added <!-- gray:saved="),
        "{on_disk}"
    );
}

#[test]
fn provenance_survives_an_unrelated_edit() {
    let (_dir, store) = setup();
    assert!(store.set(Scope::User, "a", "first").unwrap());
    let before = std::fs::read_to_string(store.path(Scope::User)).unwrap();
    assert!(store.set(Scope::User, "b", "second").unwrap());
    let after = std::fs::read_to_string(store.path(Scope::User)).unwrap();
    let line_a = |text: &str| {
        text.lines()
            .find(|l| l.starts_with("- a:"))
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(
        line_a(&before),
        line_a(&after),
        "an unrelated save restamped a"
    );
}

#[test]
fn source_from_names_the_session_or_says_cli() {
    assert_eq!(source_from(Some("d6fd9c2b")), "d6fd9c2b");
    assert_eq!(source_from(Some("  spaced  ")), "spaced");
    assert_eq!(source_from(Some("")), "cli");
    assert_eq!(source_from(Some("   ")), "cli");
    assert_eq!(source_from(None), "cli");
}

#[test]
fn list_detailed_shows_the_save_date_and_source() {
    let (_dir, store) = setup();
    assert!(store.set(Scope::User, "k", "text").unwrap());
    let detailed = store.list_detailed(Scope::User).unwrap();
    assert!(detailed.contains("- k: text"), "{detailed}");
    assert!(detailed.contains("saved "), "{detailed}");
    assert!(detailed.contains(" by "), "{detailed}");
    // The plain list stays byte-identical to before: no second line.
    assert_eq!(store.list(Scope::User).unwrap(), "- k: text\n");
}

#[test]
fn the_snapshot_never_carries_trailers() {
    let (dir, store) = setup();
    assert!(store.set(Scope::User, "k", "text").unwrap());
    let snapshot = store.snapshot(None).unwrap();
    assert!(!snapshot.contains("<!-- gray:"), "{snapshot}");
    assert!(snapshot.contains("- k: text"), "{snapshot}");
    // A durable session snapshot is the same text, so it cannot leak either.
    let id = uuid::Uuid::new_v4().to_string();
    let durable = store.snapshot(Some(&id)).unwrap();
    assert!(!durable.contains("<!-- gray:"), "{durable}");
    let _ = dir;
}

#[test]
fn first_sentence_cuts_at_period_space() {
    assert_eq!(first_sentence("One thing. Then another."), "One thing.");
}

#[test]
fn first_sentence_ignores_abbreviations_and_decimals() {
    assert_eq!(
        first_sentence("Use v1.3, e.g. this. Next."),
        "Use v1.3, e.g. this."
    );
    assert_eq!(
        first_sentence("Costs 3.5 USD total. Next."),
        "Costs 3.5 USD total."
    );
    assert_eq!(
        first_sentence("See i.e. this file. Next."),
        "See i.e. this file."
    );
}

#[test]
fn first_sentence_serves_whole_when_unbounded() {
    assert_eq!(first_sentence("No terminal period"), "No terminal period");
}

#[test]
fn profile_matches_list_when_every_entry_is_one_sentence() {
    let (_dir, store) = setup();
    store.set(Scope::Project, "a", "Short.").unwrap();
    store.set(Scope::Project, "b", "Also short.").unwrap();
    assert_eq!(
        store.profile(Scope::Project).unwrap(),
        store.list(Scope::Project).unwrap()
    );
}

#[test]
fn injected_snapshot_keeps_the_newest_entries_and_says_what_it_dropped() {
    let (_dir, store) = setup();
    let long = "x".repeat(2_000);
    // The store path lives under a directory only a write creates; the first
    // save makes it, then the file is rewritten by hand so the save dates
    // differ — the cap has to choose between entries, and newest-wins.
    store.set(Scope::Project, "seed", "x").unwrap();
    std::fs::write(
        store.path(Scope::Project),
        format!(
            "- oldest: {long} <!-- gray:saved=2026-01-01;source=cli -->\n\
             - middle: {long} <!-- gray:saved=2026-06-01;source=cli -->\n\
             - newest: {long} <!-- gray:saved=2026-09-01;source=cli -->\n"
        ),
    )
    .unwrap();

    let served = store.profile(Scope::Project).unwrap();
    assert!(
        served.len() <= SNAPSHOT_SCOPE_BYTES,
        "{} bytes is over the budget",
        served.len()
    );
    assert!(served.contains("- newest: "), "{served}");
    assert!(served.contains("- middle: "), "{served}");
    assert!(!served.contains("- oldest: "), "the oldest goes: {served}");
    assert!(served.contains("+1 older entries not shown"), "{served}");
    // The dropped entry is a command away, not gone.
    assert!(
        store.list(Scope::Project).unwrap().contains("- oldest: "),
        "the store keeps everything"
    );
    // A capped snapshot must replay on resume, notice and all.
    let session = uuid::Uuid::new_v4().to_string();
    let first = store
        .snapshot_with(Some(&session), MemoryInjection::Summary)
        .unwrap();
    let again = store
        .snapshot_with(Some(&session), MemoryInjection::Summary)
        .expect("a capped snapshot replays");
    assert_eq!(first, again);
}

#[test]
fn snapshot_carries_summaries_by_default_and_full_text_on_request() {
    let (_dir, store) = setup();
    store
        .set(
            Scope::Project,
            "long",
            "First bit. Second bit that is long.",
        )
        .unwrap();
    let summary = store.snapshot_with(None, MemoryInjection::Summary).unwrap();
    assert!(summary.contains("First bit."));
    assert!(!summary.contains("Second bit"));
    let full = store.snapshot_with(None, MemoryInjection::Full).unwrap();
    assert!(full.contains("Second bit that is long."));
}

// --- ingest guard: the daily job's verbs physically cannot overwrite,
// delete, skip the rationale contract, or blow the daily caps. The prompt
// contract leaked (a dry run overwrote five entries and invented a quote),
// so these are enforced in the write path, not in prose.

#[test]
fn ingest_set_refuses_an_existing_key() {
    let (_dir, store) = setup();
    let text = "Fact. Why: user: \"x\"; falsified: nothing yet";
    assert!(store.ingest_set(Scope::Project, "a", text).unwrap());
    let err = store
        .ingest_set(Scope::Project, "a", "Other. Why: y; falsified: z")
        .unwrap_err();
    assert!(err.to_string().contains("exists"), "{err}");
    // The original text is intact.
    assert_eq!(
        store.get(Scope::Project, "a").unwrap().as_deref(),
        Some(text)
    );
}

#[test]
fn ingest_set_requires_why_and_falsified() {
    let (_dir, store) = setup();
    assert!(store.ingest_set(Scope::Project, "a", "Fact.").is_err());
    assert!(
        store
            .ingest_set(Scope::Project, "a", "Fact. Why: user: \"x\"")
            .is_err()
    );
    assert!(
        store
            .ingest_set(Scope::Project, "a", "Fact. falsified: nothing yet")
            .is_err()
    );
    assert!(
        store
            .ingest_set(
                Scope::Project,
                "a",
                "Fact. Why: user: \"x\"; falsified: nothing yet"
            )
            .is_ok()
    );
    // A rejected write leaves no trace.
    assert_eq!(store.entry_count(), 1);
}

#[test]
fn ingest_edit_is_append_only_and_keeps_both_claims() {
    let (_dir, store) = setup();
    let old = "Claim A. Why: user: \"x\"; falsified: nothing yet";
    store.ingest_set(Scope::Project, "k", old).unwrap();
    // Unknown key: edit fails.
    assert!(
        store
            .ingest_edit(Scope::Project, "nope", "New. Why: y; falsified: z")
            .is_err()
    );
    // A replacement that drops the old claim fails.
    assert!(
        store
            .ingest_edit(Scope::Project, "k", "Claim B. Why: y; falsified: z")
            .is_err()
    );
    // The reconciliation shape passes and keeps both.
    let new =
        format!("as of 2026-09-23: Claim B. Why: user: \"y\"; falsified: nothing yet (was: {old})");
    assert!(store.ingest_edit(Scope::Project, "k", &new).unwrap());
    let served = store.get(Scope::Project, "k").unwrap().unwrap();
    assert!(served.contains("Claim A."));
    assert!(served.contains("Claim B."));
}

#[test]
fn ingest_daily_caps_stop_the_tenth_write_and_the_third_user_write() {
    let (_dir, store) = setup();
    let text = "Fact. Why: user: \"x\"; falsified: nothing yet";
    for i in 0..INGEST_DAILY_WRITE_CAP {
        store
            .ingest_set(Scope::Project, &format!("p{i}"), &format!("{text} ({i})"))
            .unwrap();
    }
    let err = store
        .ingest_set(Scope::Project, "overflow", text)
        .unwrap_err();
    assert!(err.to_string().contains("daily ingest cap"), "{err}");
    // Fresh day: the budget resets.
    std::fs::remove_file(store.ingest_counter_path()).unwrap();
    assert!(store.ingest_set(Scope::Project, "newday", text).is_ok());
}

#[test]
fn ingest_user_scope_caps_at_two_per_day() {
    let (_dir, store) = setup();
    let text = "Pref. Why: user: \"x\"; falsified: nothing yet";
    for i in 0..INGEST_DAILY_USER_WRITE_CAP {
        store
            .ingest_set(Scope::User, &format!("u{i}"), &format!("{text} ({i})"))
            .unwrap();
    }
    let err = store
        .ingest_set(Scope::User, "u-overflow", text)
        .unwrap_err();
    assert!(err.to_string().contains("user-scope ingest cap"), "{err}");
    // Project writes are unaffected by the user cap.
    assert!(store.ingest_set(Scope::Project, "p", text).is_ok());
}

#[test]
fn ingest_set_refuses_a_verbatim_duplicate_under_a_new_key() {
    let (_dir, store) = setup();
    let text = "Fact. Why: user: \"x\"; falsified: nothing yet";
    store.ingest_set(Scope::Project, "a", text).unwrap();
    let err = store.ingest_set(Scope::Project, "b", text).unwrap_err();
    assert!(err.to_string().contains("already stored"), "{err}");
    // One write was spent, not two.
    assert_eq!(store.entry_count(), 1);
}

#[test]
fn snapshot_accepts_three_word_session_names() {
    let (_dir, store) = setup();
    let named = store.snapshot(Some("chiral-xenon-pulsar")).unwrap();
    assert_eq!(
        store.snapshot(Some("chiral-xenon-pulsar")).unwrap(),
        named,
        "frozen under the name it was written"
    );
    // UUIDs still parse; traversal still fails closed.
    assert!(
        store
            .snapshot(Some(&uuid::Uuid::new_v4().to_string()))
            .is_ok()
    );
    assert!(store.snapshot(Some("../escape")).is_err());
}
