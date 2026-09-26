//! Tests for the resident fff index (`search_index`) and the find/grep lanes
//! it serves. Fixture trees live in tempdirs; each test gets its own
//! `SearchPool` so frecency state and watchers never leak between tests.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gray_core::agent::{Tool, ToolContext};
use serde_json::json;
use tempfile::TempDir;

use super::SearchPool;
use crate::{FindTool, GrepTool};

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

fn pool_for(dir: &TempDir) -> SearchPool {
    SearchPool::new(dir.path().join("fff-frecency"))
}

fn ctx_for(dir: &std::path::Path) -> ToolContext {
    ToolContext {
        cwd: dir.to_path_buf(),
        ..Default::default()
    }
}

fn lines(out: &gray_core::agent::ToolOutput) -> Vec<&str> {
    out.content.lines().collect()
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
fn indexed_glob_slash_pattern_matches_relative_path() {
    let dir = tree();
    let pool = pool_for(&dir);
    let hits = pool
        .indexed_glob(dir.path(), "sub/*.txt", 1000)
        .expect("index must serve glob");
    assert_eq!(hits, vec!["sub/c.txt".to_string()]);
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
async fn grep_tool_ignore_case_falls_back_to_rg() {
    let dir = tree();
    let pool = Arc::new(pool_for(&dir));
    let ctx = ctx_for(dir.path());
    let before = pool.lane_hits();

    let out = GrepTool::with_pool(pool.clone())
        .execute(&ctx, json!({"pattern": "NEEDLE", "ignoreCase": true}))
        .await;
    assert!(!out.is_error, "{:?}", out.content);
    assert!(out.content.contains("needle"), "{}", out.content);
    assert_eq!(
        pool.lane_hits(),
        before,
        "ignoreCase must be served by rg, not the index"
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
async fn find_tool_served_by_index() {
    let dir = tree();
    let pool = Arc::new(pool_for(&dir));
    let before = pool.lane_hits();

    let out = FindTool::with_pool(pool.clone())
        .execute(&ctx_for(dir.path()), json!({"pattern": "*.txt"}))
        .await;
    assert!(!out.is_error, "{:?}", out.content);
    assert!(out.content.contains("sub/c.txt"), "{}", out.content);
    assert!(
        pool.lane_hits() > before,
        "find must be served by the index, not fd/walk"
    );
}

#[tokio::test]
async fn grep_tool_served_by_index_match_and_context() {
    let dir = tree();
    let pool = Arc::new(pool_for(&dir));
    let grep = GrepTool::with_pool(pool.clone());
    let ctx = ctx_for(dir.path());

    let before = pool.lane_hits();
    let out = grep.execute(&ctx, json!({"pattern": "beta needle"})).await;
    assert!(!out.is_error, "{:?}", out.content);
    assert!(
        out.content.contains("a.txt:3: beta needle two"),
        "{}",
        out.content
    );
    assert!(
        pool.lane_hits() > before,
        "match-only grep must hit the index"
    );

    let before = pool.lane_hits();
    let out = grep
        .execute(&ctx, json!({"pattern": "beta needle", "context": 1}))
        .await;
    assert!(!out.is_error, "{:?}", out.content);
    let ls = lines(&out);
    assert!(ls.contains(&"a.txt:3: beta needle two"), "{}", out.content);
    assert!(ls.contains(&"a.txt-2- nothing here"), "{}", out.content);
    assert!(
        pool.lane_hits() > before,
        "context grep must hit the index too"
    );
}

#[tokio::test]
async fn tools_fall_back_outside_git() {
    // No git init → index declines; fd/walk still serves correct output.
    let dir = TempDir::new().unwrap();
    write(dir.path(), "x.txt", "needle here\n");
    let pool = Arc::new(pool_for(&dir));
    let ctx = ctx_for(dir.path());
    let before = pool.lane_hits();

    let out = FindTool::with_pool(pool.clone())
        .execute(&ctx, json!({"pattern": "*.txt"}))
        .await;
    assert!(out.content.contains("x.txt"), "{}", out.content);

    let out = GrepTool::with_pool(pool.clone())
        .execute(&ctx, json!({"pattern": "needle"}))
        .await;
    assert!(
        out.content.contains("x.txt:1: needle here"),
        "{}",
        out.content
    );
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
