#![cfg(unix)]

use std::sync::Arc;
use std::time::Instant;

use gray_core::agent::{Tool, ToolContext};
use gray_tools::search_cmd::{self, SearchArgs};
use gray_tools::search_index_fs::FsearchPool;
use gray_tools::{FindTool, GrepTool};
use serde_json::json;
use tempfile::TempDir;

const FILES: usize = 500;
const LINES: usize = 200;
const SAMPLES: usize = 15;

async fn bounded<T>(secs: u64, phase: &str, fut: impl std::future::Future<Output = T>) -> T {
    match tokio::time::timeout(std::time::Duration::from_secs(secs), fut).await {
        Ok(v) => v,
        Err(_) => panic!("index_bench phase `{phase}` exceeded {secs}s — hung, not slow"),
    }
}

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

#[tokio::test]
#[ignore = "benchmark: run on demand — cargo test --test index_bench -- --ignored"]
async fn index_vs_spawn_tax() {
    let indexed_dir = TempDir::new().unwrap();
    let legacy_dir = TempDir::new().unwrap();
    write_tree(indexed_dir.path());
    write_tree(legacy_dir.path());

    let pool = Arc::new(FsearchPool::with_options(fsearch::Options {
        dir: indexed_dir.path().join("index"),
        home: indexed_dir.path().to_string_lossy().into_owned(),
        root: Some(indexed_dir.path().to_path_buf()),
        skip: None,
    }));
    let ictx = ctx_for(indexed_dir.path());
    let lctx = ctx_for(legacy_dir.path());
    let lfind = FindTool;
    let lgrep = GrepTool;

    let args = |pattern: String| SearchArgs {
        pattern,
        limit: Some(SAMPLES),
        ..Default::default()
    };

    let t = Instant::now();
    let out = bounded(
        120,
        "cold find_with_fsearch",
        search_cmd::find_with_fsearch(&args("*.rs".to_string()), &ictx, pool.clone()),
    )
    .await;
    assert!(!out.is_empty());
    let cold_call_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(pool.lane_hits(), 0, "a cold call must not be index-served");

    bounded(
        120,
        "legacy find warm-up",
        lfind.execute(&lctx, json!({"pattern": "*.rs"})),
    )
    .await;
    bounded(
        120,
        "legacy grep warm-up",
        lgrep.execute(&lctx, json!({"pattern": "needle_0"})),
    )
    .await;
    bounded(
        120,
        "cold grep_with_fsearch",
        search_cmd::grep_with_fsearch(&args("needle_0".to_string()), &ictx, pool.clone()),
    )
    .await;
    let deadline = Instant::now() + std::time::Duration::from_secs(90);
    while pool.warm_picker(indexed_dir.path()).is_none() {
        assert!(
            Instant::now() < deadline,
            "the background index never appeared"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let mut idx_find = Vec::with_capacity(SAMPLES);
    let mut old_find = Vec::with_capacity(SAMPLES);
    let mut idx_grep = Vec::with_capacity(SAMPLES);
    let mut old_grep = Vec::with_capacity(SAMPLES);

    for i in 0..SAMPLES {
        let pat = format!("*file{:03}.rs", i % 10);
        let t = Instant::now();
        let out = bounded(
            60,
            "indexed find sample",
            search_cmd::find_with_fsearch(&args(pat.clone()), &ictx, pool.clone()),
        )
        .await;
        assert!(!out.is_empty());
        idx_find.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let out = bounded(
            60,
            "legacy find sample",
            lfind.execute(&lctx, json!({"pattern": pat})),
        )
        .await;
        assert!(!out.is_error);
        old_find.push(t.elapsed().as_secs_f64() * 1000.0);

        let needle = format!("needle_{i}", i = i % 50);
        let t = Instant::now();
        let out = bounded(
            60,
            "indexed grep sample",
            search_cmd::grep_with_fsearch(&args(needle.clone()), &ictx, pool.clone()),
        )
        .await;
        assert!(!out.is_empty());
        idx_grep.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let out = bounded(
            60,
            "legacy grep sample",
            lgrep.execute(&lctx, json!({"pattern": needle})),
        )
        .await;
        assert!(!out.is_error);
        old_grep.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    let (if_med, if_p95) = stats(idx_find);
    let (of_med, of_p95) = stats(old_find);
    let (ig_med, ig_p95) = stats(idx_grep);
    let (og_med, og_p95) = stats(old_grep);

    println!("\n=== index vs spawn tax ({FILES} files x {LINES} lines, {SAMPLES} samples) ===");
    println!(
        "first call in a process (tool lane, index starts in background): {cold_call_ms:.1} ms"
    );
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
