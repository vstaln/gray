//! The spill store: handles, recovery, the trust boundary and the bounds.
//!
//! Every test pins `$GRAY_HOME` to its own tempdir: these write real files, and
//! a test suite must never touch the developer's own store.

use crate::spill::*;
use crate::tool_out;

/// `$GRAY_HOME` is process-global, so these tests take a lock rather than
/// racing each other into another test's directory.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs `body` with `$GRAY_HOME` pointed at a fresh directory, holding the
/// lock for the whole body so no sibling test can repoint it mid-run.
fn with_home<T>(body: impl FnOnce() -> T) -> T {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().expect("tempdir");
    // SAFETY: the lock makes this the only thread touching the var while it
    // is set, and every test reads back only its own directory.
    unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
    let out = body();
    unsafe { std::env::remove_var("GRAY_HOME") };
    out
}

#[test]
fn a_stored_original_comes_back_byte_for_byte() {
    with_home(|| {
        let original = "line one\nline two\n\u{1f600} unicode tail\n";
        let handle = store(original).expect("stored");
        assert_eq!(handle.len(), HANDLE_LEN);
        assert_eq!(read(&handle).expect("read back"), original);
    });
}

#[test]
fn the_same_content_yields_the_same_handle() {
    with_home(|| {
        let a = store("same text").expect("stored");
        let b = store("same text").expect("stored again");
        assert_eq!(a, b);
    });
}

#[test]
fn a_handle_never_names_a_path_outside_the_store() {
    with_home(|| {
        for bad in [
            "../../etc/passwd",
            "..",
            "",
            "ABCDEF0123456789",  // uppercase: not what we print
            "abcdef012345678",   // 15
            "abcdef01234567890", // 17
            "abcdef012345678g",  // not hex
            "abcdef012345678/../x",
        ] {
            let err = read(bad).expect_err("must refuse");
            assert!(
                matches!(err, SpillError::BadHandle(_)),
                "{bad:?} gave {err:?}"
            );
        }
    });
}

#[test]
fn an_evicted_or_unknown_handle_is_an_error_not_an_empty_result() {
    with_home(|| {
        let err = read("0123456789abcdef").expect_err("nothing stored");
        assert!(matches!(err, SpillError::NotFound(_)));
        assert!(
            err.to_string().contains("Re-run"),
            "the message has to tell the model what to do: {err}"
        );
    });
}

#[test]
fn an_over_budget_result_keeps_its_preview_and_names_the_handle() {
    with_home(|| {
        // 4000 lines of 40 bytes: over the 2000-line cap, under 50 KiB.
        let raw: String = (0..4000)
            .map(|i| format!("line {i:04} ...................................\n"))
            .collect();
        let out = tool_out::finish(raw.clone());
        assert!(!out.is_error);
        // The preview is still head + tail, as before.
        assert!(out.content.contains("line 0000"), "head missing");
        assert!(out.content.contains("[truncated"), "annotation missing");
        // …and now the original is recoverable.
        assert!(
            out.content.contains("[spilled"),
            "no footer: {}",
            out.content
        );
        assert!(out.content.contains("gray spill grep"), "no recovery hint");
        let handle = handle_from(&out.content);
        assert_eq!(read(&handle).expect("recovered"), raw);
    });
}

/// The 16 hex characters after `gray spill ` in a footer.
fn handle_from(text: &str) -> String {
    text.split("gray spill head ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("footer carries a handle")
        .to_string()
}

#[test]
fn an_under_budget_result_is_untouched_and_spends_nothing() {
    with_home(|| {
        let raw = "small enough\n".repeat(10);
        let out = tool_out::finish(raw.clone());
        assert_eq!(out.content, raw);
        assert!(!dir().is_some_and(|d| d.join("meter.jsonl").exists()));
    });
}

#[test]
fn the_meter_reports_what_compression_saved() {
    with_home(|| {
        record(MeterEvent {
            ts: now_millis(),
            rule: "cargo".into(),
            raw: 30_000,
            sent: 900,
        });
        record(MeterEvent {
            ts: now_millis(),
            rule: "cargo".into(),
            raw: 2_000,
            sent: 1_900,
        });
        record(MeterEvent {
            ts: now_millis(),
            rule: String::new(),
            raw: 90_000,
            sent: 50_000,
        });
        let totals = totals();
        assert_eq!(totals.events, 3);
        assert_eq!(totals.raw_bytes, 122_000);
        assert_eq!(totals.sent_bytes, 52_800);
        assert_eq!(totals.saved(), 69_200);
        assert!(
            (totals.saved_pct() - 56.7).abs() < 0.1,
            "{}",
            totals.saved_pct()
        );
        // Per rule, so `gray spill stats` can say which rule earns its keep.
        assert_eq!(totals.by_rule["cargo"], (2, 32_000, 2_800));
        assert_eq!(totals.by_rule[""], (1, 90_000, 50_000));
    });
}

#[test]
fn a_torn_meter_line_does_not_hide_the_rest() {
    with_home(|| {
        record(MeterEvent {
            ts: 1,
            rule: "npm".into(),
            raw: 10,
            sent: 5,
        });
        let path = dir().expect("home").join("meter.jsonl");
        let mut text = std::fs::read_to_string(&path).expect("meter");
        text.push_str("{\"ts\":2,\"kind\":\"sque\n");
        std::fs::write(&path, text).expect("write");
        assert_eq!(events().len(), 1, "the good line still reads back");
        assert_eq!(totals().saved(), 5);
    });
}

#[test]
fn the_store_is_bounded_and_keeps_the_newest() {
    with_home(|| {
        for i in 0..MAX_ENTRIES + 20 {
            // Distinct content: distinct handles.
            store(&format!("payload {i}")).expect("stored");
            // Distinct mtimes so "newest" is unambiguous on a coarse clock.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let files: Vec<_> = std::fs::read_dir(dir().expect("home"))
            .expect("readable")
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
            .collect();
        assert!(files.len() <= MAX_ENTRIES, "store grew to {}", files.len());
        // The newest survived the eviction.
        let newest = handle_for(&format!("payload {}", MAX_ENTRIES + 19));
        assert!(read(&newest).is_ok(), "the newest entry was evicted");
    });
}

#[test]
fn the_store_is_written_owner_only() {
    with_home(|| {
        store("secret-ish content").expect("stored");
        let path = dir()
            .expect("home")
            .join(format!("{}.txt", handle_for("secret-ish content")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "spill files hold raw tool output");
        }
    });
}

#[test]
fn fmt_bytes_is_readable_at_every_scale() {
    assert_eq!(fmt_bytes(512), "512 B");
    assert_eq!(fmt_bytes(2048), "2.0 KiB");
    assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MiB");
}
