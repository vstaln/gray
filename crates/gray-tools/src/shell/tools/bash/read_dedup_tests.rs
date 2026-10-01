use super::*;

use crate::shell::contract::INLINE_BUDGET_BYTES;

fn fixture() -> (tempfile::TempDir, FileLedger) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = FileLedger::new();
    (dir, ledger)
}

fn write(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, body).expect("write");
    path
}

#[test]
fn only_a_bare_single_file_read_is_a_plain_read() {
    let (dir, _) = fixture();
    let path = write(&dir, "a.rs", "one\ntwo\n");
    let cwd = dir.path();
    let whole = |c: &str| plain_read(c, cwd).map(|r| (r.window, r.whole_file));

    assert_eq!(whole("cat a.rs"), Some(((1, None), true)));
    assert_eq!(whole("  cat   a.rs  "), Some(((1, None), true)));
    assert_eq!(whole("head -n 20 a.rs"), Some(((1, Some(20)), false)));
    assert_eq!(whole("tail -n 20 a.rs"), Some(((0, Some(20)), false)));
    assert_eq!(whole("sed -n '1,40p' a.rs"), Some(((1, Some(40)), false)));
    assert_eq!(whole("sed -n '5,$p' a.rs"), None);
    assert_eq!(whole("sed -n '1,$p' a.rs"), Some(((1, None), true)));
    // Everything that is not one file's bytes is a normal run.
    for c in [
        "cat a.rs | head",
        "cat a.rs b.rs",
        "cat -n a.rs",
        "cat *.rs",
        "cat ~/a.rs",
        "cat $HOME/a.rs",
        "cat a.rs > copy",
        "wc -l a.rs",
        "cat",
        "cat a.rs; rm a.rs",
        "cat missing.rs",
        "cat .",
        "head -c 100 a.rs",
        "sed -n '5,1p' a.rs",
        "sed -e 1p a.rs",
        "",
    ] {
        assert_eq!(whole(c), None, "{c:?} must not be a plain read");
    }
    // An absolute path resolves like any other operand.
    // Forward slashes: a bare Windows backslash path is mangled before it lands.
    let arg = path.display().to_string().replace('\\', "/");
    let abs = plain_read(&format!("cat {arg}"), cwd).expect("abs");
    // macOS /tmp is a symlink: compare resolved paths, not spellings.
    assert_eq!(
        std::fs::canonicalize(&abs.path).expect("abs resolves"),
        std::fs::canonicalize(&path).expect("path resolves")
    );

    // A path the shell would rewrite is declined even when the rewritten
    // target exists: the ledger resolves literally, so `~` and `$VAR` would
    // key the entry to a different file than the one the command read.
    std::fs::create_dir(cwd.join("~")).expect("mkdir");
    write(&dir, "~/a.rs", "tilde\n");
    assert!(plain_read("cat ~/a.rs", cwd).is_none());
    assert!(
        plain_read("cat -a.rs", cwd).is_none(),
        "a flag is not a path"
    );
    assert!(
        plain_read("head -n 0 a.rs", cwd).is_none(),
        "no lines, no window"
    );
}

#[test]
fn a_repeat_of_an_unchanged_file_is_stubbed_once() {
    let (dir, ledger) = fixture();
    let path = write(&dir, "a.rs", "one\ntwo\nthree\n");
    let read = plain_read("cat a.rs", dir.path()).expect("plain read");
    let size = std::fs::metadata(&path).unwrap().len();

    // Nothing recorded yet: the first read runs.
    assert!(check(&ledger, &read, true).is_none());
    record(&ledger, &read, size);

    // Second identical read: stubbed, and the stub names the way back.
    let hit = check(&ledger, &read, true).expect("stubbed");
    assert!(hit.content.contains("unchanged since your previous read"));
    assert!(hit.content.contains("cat a.rs"));
    assert!(!hit.content.contains("three"));
    // Consume-on-hit: the third read runs again.
    assert!(check(&ledger, &read, true).is_none());
    record(&ledger, &read, size);
    assert!(check(&ledger, &read, true).is_some());
}

#[test]
fn a_changed_file_never_stubs() {
    let (dir, ledger) = fixture();
    let path = write(&dir, "a.rs", "one\n");
    let read = plain_read("cat a.rs", dir.path()).expect("plain read");
    let size = std::fs::metadata(&path).unwrap().len();
    record(&ledger, &read, size);
    // Same size, different content: mtime is the other half of the key, and
    // the write below moves it.
    std::fs::write(&path, "two\n").expect("rewrite");
    let changed = plain_read("cat a.rs", dir.path()).expect("plain read");
    assert!(check(&ledger, &changed, true).is_none());
    record(&ledger, &changed, size);
    // Same size AND same mtime (a rewrite that lands inside the filesystem's
    // timestamp granularity) is the documented ceiling of an mtime+size key.
    assert!(check(&ledger, &changed, true).is_some());
}

#[test]
fn a_different_window_is_a_different_read() {
    let (dir, ledger) = fixture();
    write(&dir, "a.rs", "one\ntwo\nthree\n");
    let whole = plain_read("cat a.rs", dir.path()).expect("plain read");
    let head = plain_read("head -n 2 a.rs", dir.path()).expect("plain read");
    let tail = plain_read("tail -n 2 a.rs", dir.path()).expect("plain read");

    record(&ledger, &whole, 14);
    assert!(
        check(&ledger, &head, true).is_none(),
        "a window nobody read is never stubbed"
    );
    assert!(check(&ledger, &tail, true).is_none());
    assert!(check(&ledger, &whole, true).is_some());

    // One entry per file, like the read tool's ledger: reading a window of a
    // file replaces what the ledger remembers about it, so the repeat that
    // stubs is the repeat of the read that came last. A different window then
    // runs again — a miss costs tokens, never output.
    record(&ledger, &head, 8);
    assert!(check(&ledger, &head, true).is_some());
    assert!(check(&ledger, &whole, true).is_none());
}

#[test]
fn an_elided_run_is_never_recorded() {
    let (dir, ledger) = fixture();
    let path = write(&dir, "big.txt", &"x".repeat(INLINE_BUDGET_BYTES + 100));
    let size = std::fs::metadata(&path).unwrap().len();
    let read = plain_read("cat big.txt", dir.path()).expect("plain read");
    // The log held more than the inline budget, so the body the model saw was
    // head+tail: that run cannot be cited as "unchanged since you read it".
    record(&ledger, &read, size);
    assert!(check(&ledger, &read, true).is_none());
}

#[test]
fn a_whole_file_read_needs_a_whole_view_to_stub() {
    let (dir, ledger) = fixture();
    let path = write(&dir, "a.rs", "one\ntwo\n");
    let read = plain_read("cat a.rs", dir.path()).expect("plain read");
    // A record of a *window* on the same file (a head) must not make a later
    // whole-file `cat` stub: the model has not seen the rest.
    let head = plain_read("head -n 1 a.rs", dir.path()).expect("plain read");
    record(&ledger, &head, 4);
    assert!(check(&ledger, &read, true).is_none());
}

#[test]
fn the_kill_switch_and_a_lifecycle_reset_both_land() {
    let (dir, ledger) = fixture();
    let path = write(&dir, "a.rs", "one\n");
    let size = std::fs::metadata(&path).unwrap().len();
    let read = plain_read("cat a.rs", dir.path()).expect("plain read");
    record(&ledger, &read, size);
    assert!(check(&ledger, &read, false).is_none(), "GRAY_READ_DEDUP=0");
    assert!(check(&ledger, &read, true).is_some());
    // `/new` clears the shared ledger: a new session has seen nothing, so no
    // stub may point at a result that is not in its transcript.
    ledger.clear();
    assert!(check(&ledger, &read, true).is_none());
    // Compaction keeps the entries (the write guard needs them) but disarms
    // every stub, because the result it cited is compacted away.
    record(&ledger, &read, size);
    ledger.disarm_all_dedup();
    assert!(check(&ledger, &read, true).is_none());
}
