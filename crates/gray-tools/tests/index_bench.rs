//! Index-vs-spawn tax: wall-time of the fff index lane against the legacy
//! fd/rg/walk lanes on identical trees (toolrush method — alternating
//! samples, medians + p95s, real tool dispatch, no mocks).
//!
//! Two fixture twins: one `git init`'d (the index lane serves it), one plain
//! (the index declines → the old lanes serve it). Run with:
//! `cargo test -p gray-tools --test index_bench -- --nocapture`

use std::sync::Arc;
use std::time::Instant;

use gray_core::agent::{Tool, ToolContext};
use gray_tools::search_index::SearchPool;
use gray_tools::{FindTool, GrepTool};
use serde_json::json;
use tempfile::TempDir;

const FILES: usize = 500;
const LINES: usize = 200;
const SAMPLES: usize = 15;

fn ctx_for(dir: &std::path::Path) -> ToolContext {
    ToolContext {
        cwd: dir.to_path_buf(),
        ..Default::default()
    }
}

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = v[v.len() / 2];
    let p95 = v[((0.95 * v.len() as f64) as usize).min(v.len() - 1)];
    (med, p95)
}

fn write_tree(root: &std::path::Path) {
    for i in 0..FILES {
        let dir = root.join(format!("src/mod{:02}", i % 20));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = String::new();
        for ln in 0..LINES {
            s.push_str(&format!("fn f_{i}_{ln}() {{ let v = \"needle_{i}\"; }}\n"));
        }
        std::fs::write(dir.join(format!("file{i:03}.rs")), s).unwrap();
    }
}

fn git_init(dir: &std::path::Path) {
    let ok = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status();
    assert!(
        ok.is_ok_and(|s| s.success()),
        "git init failed — is git installed?"
    );
}

#[tokio::test]
async fn index_vs_spawn_tax() {
    // Twin trees: identical content, only one is a git worktree.
    let indexed_dir = TempDir::new().unwrap();
    let legacy_dir = TempDir::new().unwrap();
    write_tree(indexed_dir.path());
    write_tree(legacy_dir.path());
    git_init(indexed_dir.path());

    let pool = Arc::new(SearchPool::new(indexed_dir.path().join("fff-frecency")));
    let ictx = ctx_for(indexed_dir.path());
    let lctx = ctx_for(legacy_dir.path());
    let ifind = FindTool::with_pool(pool.clone());
    let igrep = GrepTool::with_pool(pool);
    let lfind = FindTool::default();
    let lgrep = GrepTool::default();

    // Cold-start: first index call pays the scan. Report it separately.
    let t = Instant::now();
    let out = ifind.execute(&ictx, json!({"pattern": "*.rs"})).await;
    assert!(!out.is_error);
    let cold_ms = t.elapsed().as_secs_f64() * 1000.0;

    // Warm the rg path too (page cache) so the comparison is warm-vs-warm.
    lfind.execute(&lctx, json!({"pattern": "*.rs"})).await;
    lgrep.execute(&lctx, json!({"pattern": "needle_0"})).await;
    igrep.execute(&ictx, json!({"pattern": "needle_0"})).await;

    let mut idx_find = Vec::with_capacity(SAMPLES);
    let mut old_find = Vec::with_capacity(SAMPLES);
    let mut idx_grep = Vec::with_capacity(SAMPLES);
    let mut old_grep = Vec::with_capacity(SAMPLES);

    for i in 0..SAMPLES {
        // Alternate lanes so machine noise spreads evenly.
        let pat = format!("*file{:03}.rs", i % 10);
        let t = Instant::now();
        let out = ifind.execute(&ictx, json!({"pattern": pat})).await;
        assert!(!out.is_error);
        idx_find.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let out = lfind.execute(&lctx, json!({"pattern": pat})).await;
        assert!(!out.is_error);
        old_find.push(t.elapsed().as_secs_f64() * 1000.0);

        let needle = format!("needle_{i}", i = i % 50);
        let t = Instant::now();
        let out = igrep.execute(&ictx, json!({"pattern": needle})).await;
        assert!(!out.is_error);
        idx_grep.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let out = lgrep.execute(&lctx, json!({"pattern": needle})).await;
        assert!(!out.is_error);
        old_grep.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    let (if_med, if_p95) = stats(idx_find);
    let (of_med, of_p95) = stats(old_find);
    let (ig_med, ig_p95) = stats(idx_grep);
    let (og_med, og_p95) = stats(old_grep);

    println!("\n=== index vs spawn tax ({FILES} files x {LINES} lines, {SAMPLES} samples) ===");
    println!("first index call (cold scan): {cold_ms:.1} ms");
    println!(
        "{:<14} {:>10} {:>10} {:>10} {:>10}",
        "op", "idx_med", "old_med", "idx_p95", "old_p95"
    );
    println!(
        "{:<14} {:>8.2}ms {:>8.2}ms {:>8.2}ms {:>8.2}ms",
        "find", if_med, of_med, if_p95, of_p95
    );
    println!(
        "{:<14} {:>8.2}ms {:>8.2}ms {:>8.2}ms {:>8.2}ms",
        "grep", ig_med, og_med, ig_p95, og_p95
    );
    if of_med > 0.0 {
        println!("find speedup: {:.1}x", of_med / if_med.max(0.001));
    }
    if og_med > 0.0 {
        println!("grep speedup: {:.1}x", og_med / ig_med.max(0.001));
    }
}
