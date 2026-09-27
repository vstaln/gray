//! Tests for the resident fff index (`search_index`) and the find/grep lanes
//! it serves. Fixture trees live in tempdirs; each test gets its own
//! `SearchPool` so frecency state and watchers never leak between tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gray_core::agent::ToolContext;
use tempfile::TempDir;

use super::SearchPool;
use crate::search_cmd::SearchArgs;

fn write(root: &std::path::Path, name: &str, content: &str) {
    let p = root.join(name);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, content).unwrap();
}

/// Fixed fixture: nested files, a dotfile dir, and content shaped to exercise
/// match/context/literal/case lanes. `git init` is required: the index lane
/// only serves git-rooted dirs (fff drops dotfiles on non-git roots while fd
/// --hidden does not — the fallback keeps contract parity there).
fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let ok = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status();
    assert!(
        ok.is_ok_and(|s| s.success()),
        "git init failed — is git installed?"
    );
    let r = dir.path();
    write(
        r,
        "a.txt",
        "alpha needle one\nnothing here\nbeta needle two\n",
    );
    write(r, "sub/c.txt", "deep needle four\n");
    write(r, "src/lib.rs", "pub fn needle_fn() {}\n");
    write(r, "awkward.txt", "a:b:c needle\nUPPER needle lower\n");
    write(r, ".hidden/h.txt", "hidden needle\n");
    write(r, ".gitignore", "ignored.txt\n");
    write(r, "ignored.txt", "needle ignored\n");
    dir
}

fn sorted_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines.sort_unstable();
    lines
}

fn pool_for(dir: &TempDir) -> SearchPool {
    SearchPool::new(dir.path().join("fff-frecency"))
}

fn ctx_for(dir: &std::path::Path) -> ToolContext {
    ToolContext {
        cwd: dir.to_path_buf(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Pool behavior
// ---------------------------------------------------------------------------

#[test]
fn pool_caches_one_picker_per_dir() {
    let dir = tree();
    let other = TempDir::new().unwrap();
    let pool = pool_for(&dir);

    assert!(pool.picker(dir.path()).is_some());
    assert!(pool.picker(dir.path()).is_some());
    assert_eq!(pool.len(), 1, "same dir must reuse one picker");

    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(other.path())
        .status()
        .unwrap();
    assert!(pool.picker(other.path()).is_some());
    assert_eq!(pool.len(), 2, "distinct dirs get distinct indexes");
}

#[test]
fn picker_rejects_missing_dir() {
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(pool.picker(&dir.path().join("nope")).is_none());
}

#[test]
fn picker_rejects_file_target() {
    // `grep <file>` must not build an index — the rg/single-file lane owns it.
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(pool.picker(&dir.path().join("a.txt")).is_none());
    assert!(pool.is_empty(), "no index may be built for a file path");
}

// ---------------------------------------------------------------------------
// indexed_glob (the `find` lane)
// ---------------------------------------------------------------------------

#[test]
fn indexed_glob_finds_nested_and_root_files() {
    let dir = tree();
    let pool = pool_for(&dir);
    let hits = pool
        .indexed_glob(dir.path(), "*.txt", 1000)
        .expect("index must serve glob");
    // fd --glob parity: no slash in pattern = basename match at any depth.
    assert!(hits.contains(&"a.txt".to_string()), "got {hits:?}");
    assert!(hits.contains(&"sub/c.txt".to_string()), "got {hits:?}");
    assert!(hits.contains(&"awkward.txt".to_string()), "got {hits:?}");
    assert!(!hits.iter().any(|h| h.ends_with(".rs")), "got {hits:?}");
}

#[test]
fn indexed_glob_honors_limit() {
    let dir = tree();
    let pool = pool_for(&dir);
    let hits = pool.indexed_glob(dir.path(), "*.txt", 1).unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn indexed_glob_returns_directories() {
    // fd --glob returns matching directories too (trailing slash) — the index
    // lane must not silently drop them.
    let dir = tree();
    let pool = pool_for(&dir);
    let hits = pool.indexed_glob(dir.path(), "sub", 100).unwrap();
    assert!(hits.iter().any(|h| h == "sub/"), "got {hits:?}");
}

#[test]
fn indexed_glob_includes_dotfiles() {
    // find runs fd --hidden; the index must not silently drop dotfiles.
    let dir = tree();
    let pool = pool_for(&dir);
    let hits = pool.indexed_glob(dir.path(), "*.txt", 1000).unwrap();
    assert!(
        hits.contains(&".hidden/h.txt".to_string()),
        "dotfiles must be indexed like fd --hidden: {hits:?}"
    );
}

#[test]
fn indexed_glob_no_match_returns_empty_not_none() {
    let dir = tree();
    let pool = pool_for(&dir);
    let hits = pool.indexed_glob(dir.path(), "*.zzz", 1000).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn indexed_glob_and_grep_respect_gitignore() {
    let dir = tree();
    let pool = pool_for(&dir);
    let hits = pool.indexed_glob(dir.path(), "*.txt", 1000).unwrap();
    assert!(
        !hits.contains(&"ignored.txt".to_string()),
        "gitignored file must not appear: {hits:?}"
    );
    let found = pool
        .indexed_grep(
            dir.path(),
            "needle",
            None,
            false,
            false,
            100,
            0,
            &Default::default(),
        )
        .unwrap();
    assert!(
        !found.hits.iter().any(|(p, ..)| p.ends_with("ignored.txt")),
        "gitignored file must not be grepped: {:?}",
        found.hits
    );
}

#[test]
fn non_git_dir_declines_index() {
    // No git init: the index lane must decline so fd/walk keeps parity.
    let dir = TempDir::new().unwrap();
    write(dir.path(), "x.txt", "needle\n");
    let pool = pool_for(&dir);
    assert!(pool.picker(dir.path()).is_none());
    assert!(pool.indexed_glob(dir.path(), "*.txt", 10).is_none());
    assert!(
        pool.indexed_grep(
            dir.path(),
            "needle",
            None,
            false,
            false,
            10,
            0,
            &Default::default()
        )
        .is_none()
    );
}

// ---------------------------------------------------------------------------
// indexed_grep (the `grep` lane) — returns grep_builtin::Found so
// format_matches renders `file:line: text` / `file-line- text` identically.
// ---------------------------------------------------------------------------

#[test]
fn indexed_grep_returns_hit_tuples() {
    let dir = tree();
    let pool = pool_for(&dir);
    let found = pool
        .indexed_grep(
            dir.path(),
            "needle",
            None,
            false,
            false,
            100,
            0,
            &Default::default(),
        )
        .expect("index must serve grep");
    assert!(!found.limit_reached);
    // a.txt has matches on lines 1 and 3.
    let a_hits: Vec<_> = found
        .hits
        .iter()
        .filter(|(p, _, _, m)| p.ends_with("a.txt") && *m)
        .collect();
    assert_eq!(a_hits.len(), 2, "got {:?}", found.hits);
    assert!(
        a_hits
            .iter()
            .any(|(_, n, t, _)| *n == 1 && t.as_deref() == Some("alpha needle one"))
    );
    assert!(
        a_hits
            .iter()
            .any(|(_, n, t, _)| *n == 3 && t.as_deref() == Some("beta needle two"))
    );
}

#[test]
fn indexed_grep_marks_context_lines() {
    let dir = tree();
    let pool = pool_for(&dir);
    let found = pool
        .indexed_grep(
            dir.path(),
            "beta needle",
            None,
            false,
            false,
            100,
            1,
            &Default::default(),
        )
        .unwrap();
    // The match at a.txt:3 must carry a.txt:2 as a non-match context hit.
    assert!(
        found.hits.iter().any(|(p, n, t, m)| p.ends_with("a.txt")
            && *n == 2
            && !*m
            && t.as_deref() == Some("nothing here")),
        "context line missing: {:?}",
        found.hits
    );
}

#[test]
fn indexed_grep_honors_limit() {
    let dir = tree();
    let pool = pool_for(&dir);
    let found = pool
        .indexed_grep(
            dir.path(),
            "needle",
            None,
            false,
            false,
            1,
            0,
            &Default::default(),
        )
        .unwrap();
    assert_eq!(found.hits.iter().filter(|h| h.3).count(), 1);
    assert!(found.limit_reached);
}

#[test]
fn indexed_grep_declines_ignore_case() {
    // fff 0.10.x has only smart_case — no explicit insensitive mode. The lane
    // must decline so rg (real -i) serves it; no regex-shim slowdown.
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(
        pool.indexed_grep(
            dir.path(),
            "needle",
            None,
            true,
            false,
            100,
            0,
            &Default::default()
        )
        .is_none()
    );
}

#[tokio::test]
async fn grep_command_ignore_case_falls_back_to_rg() {
    let dir = tree();
    let pool = Arc::new(pool_for(&dir));
    let ctx = ctx_for(dir.path());

    // Warm the index first, so this is a *decline*, not a cold miss.
    crate::search_cmd::grep_with_pool(
        &SearchArgs {
            pattern: "needle".to_string(),
            limit: Some(100),
            ..Default::default()
        },
        &ctx,
        pool.clone(),
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while pool.warm_picker(dir.path()).is_none() {
        assert!(Instant::now() < deadline, "background index never appeared");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let before = pool.lane_hits();
    let text = crate::search_cmd::grep_with_pool(
        &SearchArgs {
            pattern: "NEEDLE".to_string(),
            ignore_case: true,
            limit: Some(100),
            ..Default::default()
        },
        &ctx,
        pool.clone(),
    )
    .await;
    assert!(text.contains("needle"), "{text}");
    assert_eq!(
        pool.lane_hits(),
        before,
        "ignoreCase must be served by rg, not the index, even when the index is warm"
    );
}

#[test]
fn indexed_grep_case_sensitive_by_default() {
    let dir = tree();
    let pool = pool_for(&dir);
    let found = pool
        .indexed_grep(
            dir.path(),
            "NEEDLE",
            None,
            false,
            false,
            100,
            0,
            &Default::default(),
        )
        .unwrap();
    assert!(
        found.hits.is_empty(),
        "case-sensitive default must not fold"
    );
}

#[test]
fn indexed_grep_literal_mode() {
    let dir = tree();
    let pool = pool_for(&dir);
    let found = pool
        .indexed_grep(
            dir.path(),
            "a:b",
            None,
            false,
            true,
            100,
            0,
            &Default::default(),
        )
        .unwrap();
    assert_eq!(found.hits.len(), 1);
    assert!(found.hits[0].0.ends_with("awkward.txt"));
    assert_eq!(found.hits[0].2.as_deref(), Some("a:b:c needle"));
}

#[test]
fn indexed_grep_regex_by_default() {
    let dir = tree();
    let pool = pool_for(&dir);
    let found = pool
        .indexed_grep(
            dir.path(),
            "needle (one|two)",
            None,
            false,
            false,
            100,
            0,
            &Default::default(),
        )
        .unwrap();
    assert_eq!(
        found.hits.len(),
        2,
        "regex alternation must match: {:?}",
        found.hits
    );
}

#[test]
fn indexed_grep_glob_filters_files() {
    let dir = tree();
    let pool = pool_for(&dir);
    let found = pool
        .indexed_grep(
            dir.path(),
            "needle",
            Some("*.rs"),
            false,
            false,
            100,
            0,
            &Default::default(),
        )
        .unwrap();
    assert_eq!(found.hits.len(), 1);
    assert!(found.hits[0].0.ends_with("lib.rs"));
}

#[test]
fn indexed_grep_declines_negated_glob() {
    // rg -g '!*.rs' means "everything except .rs". fff wants
    // Constraint::Not(Glob(..)) — a bare '!*.rs' Glob compiles as a literal
    // filename and would silently match nothing. Decline to rg.
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(
        pool.indexed_grep(
            dir.path(),
            "needle",
            Some("!*.rs"),
            false,
            false,
            100,
            0,
            &Default::default()
        )
        .is_none()
    );
}

#[test]
fn indexed_grep_declines_invalid_regex() {
    // fff falls back to literal matching on regex compile failure and reports
    // the error via regex_fallback_error; gray's contract is to surface the
    // error text, which the rg lane already produces — decline.
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(
        pool.indexed_grep(
            dir.path(),
            "needle (unclosed",
            None,
            false,
            false,
            100,
            0,
            &Default::default()
        )
        .is_none()
    );
}

#[test]
fn indexed_grep_declines_invalid_glob() {
    // An uncompilable glob matches nothing inside fff, trips the literal
    // fallback, and resurfaces as a decline here.
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(
        pool.indexed_grep(
            dir.path(),
            "needle",
            Some("[unclosed"),
            false,
            false,
            100,
            0,
            &Default::default()
        )
        .is_none()
    );
}

#[test]
fn indexed_grep_declines_literal_fallback() {
    // "a:b" only lives in awkward.txt. Glob-scoped to *.rs, the constrained
    // scan finds nothing — fff's literal fallback would retry WITHOUT the
    // glob and return the .txt hit. rg's contract is zero hits: decline.
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(
        pool.indexed_grep(
            dir.path(),
            "a:b",
            Some("*.rs"),
            false,
            false,
            100,
            0,
            &Default::default()
        )
        .is_none()
    );
}

// ---------------------------------------------------------------------------
// Tool-level: prove the lanes actually serve (not just contract-equal output)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_command_serves_from_the_index_once_it_is_warm() {
    let dir = tree();
    let pool = Arc::new(pool_for(&dir));
    let ctx = ctx_for(dir.path());
    let args = || SearchArgs {
        pattern: "*.txt".to_string(),
        limit: Some(100),
        ..Default::default()
    };

    // Cold: no index in this process, so the tool answers — a full scan would
    // cost more than the question is worth (1.4s vs 20ms on 20k files).
    let first = crate::search_cmd::find_with_pool(&args(), &ctx, pool.clone()).await;
    assert!(first.contains("sub/c.txt"), "{first}");
    assert_eq!(
        pool.lane_hits(),
        0,
        "a cold command must not wait for a scan"
    );

    // Wait for the background build the cold call started, then the index
    // serves the same question.
    let deadline = Instant::now() + Duration::from_secs(10);
    while pool.warm_picker(dir.path()).is_none() {
        assert!(Instant::now() < deadline, "background index never appeared");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let before = pool.lane_hits();
    let second = crate::search_cmd::find_with_pool(&args(), &ctx, pool.clone()).await;
    assert_eq!(
        sorted_lines(&second),
        sorted_lines(&first),
        "both lanes must answer the same question identically"
    );
    assert!(
        pool.lane_hits() > before,
        "a warm command must be served by the index, not fd/walk"
    );
}

#[tokio::test]
async fn grep_command_served_by_index_match_and_context() {
    let dir = tree();
    let pool = Arc::new(pool_for(&dir));
    let ctx = ctx_for(dir.path());
    let args = |context| SearchArgs {
        pattern: "beta needle".to_string(),
        limit: Some(100),
        context,
        ..Default::default()
    };

    // Two cold calls start the build; the rest must be index-served.
    crate::search_cmd::grep_with_pool(&args(None), &ctx, pool.clone()).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while pool.warm_picker(dir.path()).is_none() {
        assert!(Instant::now() < deadline, "background index never appeared");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let before = pool.lane_hits();
    let text = crate::search_cmd::grep_with_pool(&args(None), &ctx, pool.clone()).await;
    assert!(text.contains("a.txt:3: beta needle two"), "{text}");
    assert!(
        pool.lane_hits() > before,
        "match-only grep must hit the index"
    );

    let before = pool.lane_hits();
    let text = crate::search_cmd::grep_with_pool(&args(Some(1)), &ctx, pool.clone()).await;
    let ls: Vec<&str> = text.lines().collect();
    assert!(ls.contains(&"a.txt:3: beta needle two"), "{text}");
    assert!(ls.contains(&"a.txt-2- nothing here"), "{text}");
    assert!(
        pool.lane_hits() > before,
        "context grep must hit the index too"
    );
}

#[tokio::test]
async fn commands_fall_back_outside_git() {
    // No git init → index declines; fd/walk still serves correct output.
    let dir = TempDir::new().unwrap();
    write(dir.path(), "x.txt", "needle here\n");
    let pool = Arc::new(pool_for(&dir));
    let ctx = ctx_for(dir.path());
    let before = pool.lane_hits();

    let text = crate::search_cmd::find_with_pool(
        &SearchArgs {
            pattern: "*.txt".to_string(),
            limit: Some(100),
            ..Default::default()
        },
        &ctx,
        pool.clone(),
    )
    .await;
    assert!(text.contains("x.txt"), "{text}");

    let text = crate::search_cmd::grep_with_pool(
        &SearchArgs {
            pattern: "needle".to_string(),
            limit: Some(100),
            ..Default::default()
        },
        &ctx,
        pool.clone(),
    )
    .await;
    assert!(text.contains("x.txt:1: needle here"), "{text}");
    assert_eq!(
        pool.lane_hits(),
        before,
        "non-git dir must not touch the index"
    );
}

// ---------------------------------------------------------------------------
// The point of a resident index: watcher picks up new files without rescan
// ---------------------------------------------------------------------------

#[tokio::test]
async fn new_file_visible_without_rescan() {
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(pool.picker(dir.path()).is_some());

    write(dir.path(), "fresh.rs", "fn fresh() {}\n");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let hits = pool.indexed_glob(dir.path(), "fresh.rs", 10).unwrap();
        if hits.iter().any(|h| h == "fresh.rs") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "watcher never picked up fresh.rs"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// Exactness: anything the index cannot answer like fd/rg must decline
// ---------------------------------------------------------------------------

/// fff compiles globs with `literal_separator = false`, so `a/*.rs` also
/// matches everything below `a/`. Pagination happens inside fff, before the
/// lane could filter, so a slash pattern has to decline — `fd --full-path`
/// answers it exactly. (Measured on gray itself: 69 index hits vs 23 from fd.)
#[test]
fn indexed_glob_declines_slash_pattern() {
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(pool.indexed_glob(dir.path(), "sub/*.txt", 1000).is_none());
    assert!(pool.indexed_glob(dir.path(), "/abs/*.txt", 1000).is_none());
}

/// An uncompilable glob must reach `fd`, which reports the parse error.
/// Serving fff's "matched nothing" would answer "No files found" instead.
#[test]
fn indexed_glob_declines_invalid_glob() {
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(pool.indexed_glob(dir.path(), "*.{rs", 1000).is_none());
    assert_eq!(pool.lane_hits(), 0, "a declined pattern must not be served");
}

/// A slash-free pattern is a basename glob, so a directory component that
/// matches the pattern must not drag its files in with it: `*_test*` on
/// `dir_test_only/fixture.txt` is an fd miss, not a hit.
#[test]
fn indexed_glob_matches_basename_only() {
    let dir = tree();
    write(dir.path(), "dir_test_only/fixture.txt", "needle basename\n");
    let pool = pool_for(&dir);
    let hits = pool
        .indexed_glob(dir.path(), "*_test_only*", 1000)
        .expect("index must serve a basename glob");
    assert!(
        !hits.iter().any(|h| h == "dir_test_only/fixture.txt"),
        "index matched through a directory component: {hits:?}"
    );
    // The file itself is still findable by basename.
    let hits = pool
        .indexed_glob(dir.path(), "fixture.txt", 1000)
        .expect("index must serve a basename glob");
    assert_eq!(hits, vec!["dir_test_only/fixture.txt".to_string()]);
}

/// Same depth-anchoring rule as `indexed_glob_declines_slash_pattern`, for
/// `grep`'s `glob` argument.
#[test]
fn indexed_grep_declines_slash_glob() {
    let dir = tree();
    let pool = pool_for(&dir);
    assert!(
        pool.indexed_grep(
            dir.path(),
            "needle",
            Some("src/*.rs"),
            false,
            false,
            10,
            0,
            &Default::default()
        )
        .is_none()
    );
    // A slash-free glob stays on the index (basename semantics either way).
    assert!(
        pool.indexed_grep(
            dir.path(),
            "needle",
            Some("*.txt"),
            false,
            false,
            10,
            0,
            &Default::default()
        )
        .is_some()
    );
}

/// A queued cancel must short-circuit the index lane, exactly as it does the
/// fd/rg lanes — otherwise the first search in a cold tree ignores Esc for up
/// to the pool's scan budget.
#[tokio::test]
async fn cancelled_command_skips_the_index_lane() {
    let dir = tree();
    let pool = Arc::new(pool_for(&dir));
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        cancel: cancel.clone(),
        ..Default::default()
    };
    let out = crate::search_cmd::find_with_pool(
        &SearchArgs {
            pattern: "*.txt".to_string(),
            limit: Some(100),
            ..Default::default()
        },
        &ctx,
        pool.clone(),
    )
    .await;
    assert_eq!(out, "cancelled by user");
    let out = crate::search_cmd::grep_with_pool(
        &SearchArgs {
            pattern: "needle".to_string(),
            limit: Some(100),
            ..Default::default()
        },
        &ctx,
        pool.clone(),
    )
    .await;
    assert_eq!(out, "cancelled by user");
    assert_eq!(pool.lane_hits(), 0, "a cancelled call must not be served");
    assert!(
        pool.warm_picker(dir.path()).is_none(),
        "a cancelled call must not start a background index either"
    );
}
