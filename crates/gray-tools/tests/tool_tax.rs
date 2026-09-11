//! Tool-call tax baseline: paired wall-time benchmarks over gray's real tool
//! dispatch paths (toolrush method: same workload, alternating samples,
//! medians + p95s, no mocks).
//!
//! Purpose: lock in today's numbers. Any future fast path (grep envelope,
//! parallel dispatch) must beat these or it doesn't ship. Run with:
//! `cargo test -p gray-tools --test tool_tax -- --nocapture`
//!
//! These are tool-operation wall times, NOT model-inclusive turn speed:
//! tool-heavy turns move with these numbers, chat-heavy turns barely do.

use std::sync::Arc;
use std::time::Instant;

use gray_core::agent::{Tool, ToolContext};
use gray_tools::{BashTool, FileLedger, FindTool, GrepTool, ReadTool};
use serde_json::json;
use tempfile::TempDir;

const SAMPLES: usize = 21;

fn ctx_for(dir: &std::path::Path) -> ToolContext {
    let mut c = ToolContext::default();
    c.cwd = dir.to_path_buf();
    c
}

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = v[v.len() / 2];
    let p95 = v[((0.95 * v.len() as f64) as usize).min(v.len() - 1)];
    (med, p95)
}

/// Build a realistic fixture tree: 50 python-ish files x 400 lines.
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir(&src).unwrap();
    for i in 0..50 {
        let mut s = String::new();
        for ln in 0..400 {
            s.push_str(&format!(
                "def func_{i}_{ln}(): return \"needle_{i}\" if {ln} % 37 == 0 else None\n"
            ));
        }
        std::fs::write(src.join(format!("file{i:02}.py")), s).unwrap();
    }
    dir
}

#[tokio::test]
async fn baseline_tool_tax() {
    let dir = fixture();
    let ctx = ctx_for(dir.path());
    let ledger = Arc::new(FileLedger::new());
    let read = ReadTool::new(ledger);
    let grep = GrepTool;
    let find = FindTool;
    let bash = BashTool;

    // Warm up (page cache, tokio runtime, rg binary).
    read.execute(&ctx, json!({"path": "src/file00.py", "limit": 100})).await;
    grep.execute(&ctx, json!({"pattern": "needle_00", "path": "src"})).await;

    // Alternate read/grep samples so machine noise spreads evenly.
    let mut reads = Vec::with_capacity(SAMPLES);
    let mut greps = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let t = Instant::now();
        let out = read
            .execute(&ctx, json!({"path": format!("src/file{:02}.py", i % 50), "limit": 100}))
            .await;
        assert!(!out.content.is_empty());
        reads.push(t.elapsed().as_secs_f64() * 1000.0);

        let t = Instant::now();
        let out = grep
            .execute(&ctx, json!({"pattern": "needle_00", "path": "src"}))
            .await;
        assert!(!out.content.is_empty());
        greps.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    let mut finds = Vec::with_capacity(SAMPLES);
    let mut bashes = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let t = Instant::now();
        let out = find.execute(&ctx, json!({"pattern": "*.py", "path": "src"})).await;
        assert!(!out.content.is_empty());
        finds.push(t.elapsed().as_secs_f64() * 1000.0);

        let t = Instant::now();
        let out = bash.execute(&ctx, json!({"command": "echo hi"})).await;
        assert!(!out.content.is_empty());
        bashes.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    // Repeat-read dedup lane: same file twice, second hit should stub.
    // (Fresh ledger per ReadTool here shares one ledger, so hits accumulate.)
    let t = Instant::now();
    read.execute(&ctx, json!({"path": "src/file00.py", "limit": 100})).await;
    let first = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let stubbed = read.execute(&ctx, json!({"path": "src/file00.py", "limit": 100})).await;
    let second = t.elapsed().as_secs_f64() * 1000.0;

    let (rm, rp) = stats(reads);
    let (gm, gp) = stats(greps);
    let (fm, fp) = stats(finds);
    let (bm, bp) = stats(bashes);
    println!("\n=== tool-call tax baseline ({SAMPLES} alternating samples) ===");
    println!("read  100 lines, fresh files : median {rm:.2}ms  p95 {rp:.2}ms");
    println!("grep  50-file tree search    : median {gm:.2}ms  p95 {gp:.2}ms");
    println!("find  *.py in src            : median {fm:.2}ms  p95 {fp:.2}ms");
    println!("bash  echo hi (spawn+dispatch): median {bm:.2}ms  p95 {bp:.2}ms");
    println!("read  repeat same file       : first {first:.2}ms  second {second:.2}ms");
    println!("  (stubbed second read should be ~0ms; output len {})", stubbed.content.len());

    // Negative-control shape: the suite must actually exercise the tools.
    // If medians ever read 0.00ms, the harness is broken, not fast.
    assert!(rm > 0.0 && gm > 0.0 && fm > 0.0 && bm > 0.0, "zero medians = broken bench, not fast tools");
}
