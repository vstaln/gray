use crate::spill::*;
use crate::tool_out;

fn with_home<T>(body: impl FnOnce() -> T) -> T {
    let _guard = crate::paths::HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().expect("tempdir");

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
            "ABCDEF0123456789",
            "abcdef012345678",
            "abcdef01234567890",
            "abcdef012345678g",
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
        let raw: String = (0..4000)
            .map(|i| format!("line {i:04} ...................................\n"))
            .collect();
        let out = tool_out::finish(raw.clone());
        assert!(!out.is_error);

        assert!(out.content.contains("line 0000"), "head missing");
        assert!(out.content.contains("[truncated"), "annotation missing");

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
    });
}

#[test]
fn the_store_is_bounded_and_keeps_the_newest() {
    with_home(|| {
        for i in 0..MAX_ENTRIES + 20 {
            store(&format!("payload {i}")).expect("stored");

            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let files: Vec<_> = std::fs::read_dir(dir().expect("home"))
            .expect("readable")
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
            .collect();
        assert!(files.len() <= MAX_ENTRIES, "store grew to {}", files.len());

        let newest = handle_for(&format!("payload {}", MAX_ENTRIES + 19));
        assert!(read(&newest).is_ok(), "the newest entry was evicted");
    });
}

#[cfg(unix)]
#[test]
fn the_store_is_written_owner_only() {
    with_home(|| {
        store("secret-ish content").expect("stored");
        let path = dir()
            .expect("home")
            .join(format!("{}.txt", handle_for("secret-ish content")));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "spill files hold raw tool output");
    });
}

#[test]
fn fmt_bytes_is_readable_at_every_scale() {
    assert_eq!(fmt_bytes(512), "512 B");
    assert_eq!(fmt_bytes(2048), "2.0 KiB");
    assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MiB");
}
