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
    read.execute(&ctx, json!({"path": "src/file00.py", "limit": 100}))
        .await;
    grep.execute(&ctx, json!({"pattern": "needle_00", "path": "src"}))
        .await;

    // Alternate read/grep samples so machine noise spreads evenly.
    let mut reads = Vec::with_capacity(SAMPLES);
    let mut greps = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let t = Instant::now();
        let out = read
            .execute(
                &ctx,
                json!({"path": format!("src/file{:02}.py", i % 50), "limit": 100}),
            )
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
        let out = find
            .execute(&ctx, json!({"pattern": "*.py", "path": "src"}))
            .await;
        assert!(!out.content.is_empty());
        finds.push(t.elapsed().as_secs_f64() * 1000.0);

        let t = Instant::now();
        let out = bash.execute(&ctx, json!({"command": "echo hi"})).await;
        assert!(!out.content.is_empty());
        bashes.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    // Repeat-read dedup lane: a fully-covered small file read twice.
    // (limit:100 on a 400-line file is a partial window and must NOT stub.)
    std::fs::write(dir.path().join("small.txt"), "hello\n".repeat(50)).unwrap();
    let t = Instant::now();
    read.execute(&ctx, json!({"path": "small.txt", "limit": 100}))
        .await;
    let first = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let stubbed = read
        .execute(&ctx, json!({"path": "small.txt", "limit": 100}))
        .await;
    let second = t.elapsed().as_secs_f64() * 1000.0;

    // Many-match lane: "needle" hits all 20,000 lines, forcing full parse
    // of every match envelope (limit raised past the total). This is the
    // shape where --json vs --vimgrep actually diverge, if at all.
    let mut manys = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let t = Instant::now();
        let out = grep
            .execute(
                &ctx,
                json!({"pattern": "needle", "path": "src", "limit": 50000}),
            )
            .await;
        assert!(!out.content.is_empty());
        manys.push(t.elapsed().as_secs_f64() * 1000.0);
    }

    let (rm, rp) = stats(reads);
    let (gm, gp) = stats(greps);
    let (fm, fp) = stats(finds);
    let (bm, bp) = stats(bashes);
    println!("\n=== tool-call tax baseline ({SAMPLES} alternating samples) ===");
    println!("read  100 lines, fresh files : median {rm:.2}ms  p95 {rp:.2}ms");
    println!("grep  50-file tree search    : median {gm:.2}ms  p95 {gp:.2}ms");
    println!("find  *.py in src            : median {fm:.2}ms  p95 {fp:.2}ms");
    println!("bash  echo hi (spawn+dispatch): median {bm:.2}ms  p95 {bp:.2}ms");
    let (mm, mp) = stats(manys);
    println!("grep  20k matches, full parse : median {mm:.2}ms  p95 {mp:.2}ms");
    println!("read  repeat same file       : first {first:.2}ms  second {second:.2}ms");
    println!(
        "  (stubbed second read should be ~0ms; output len {})",
        stubbed.content.len()
    );

    // Negative-control shape: the suite must actually exercise the tools.
    // If medians ever read 0.00ms, the harness is broken, not fast.
    assert!(
        rm > 0.0 && gm > 0.0 && fm > 0.0 && bm > 0.0,
        "zero medians = broken bench, not fast tools"
    );
}

/// Toolrush law: byte-identical output or the lane doesn't ship. The fast
/// `--vimgrep` path must agree with `--json` on match-only searches across
/// shapes that historically diverge (colons in text, case folding, globs,
/// limits, no-match, unicode, CRLF). Context searches bypass the fast path.
///
/// rg's parallel walker emits files in nondeterministic order run-to-run,
/// so multi-file searches that hit the match limit can legitimately return
/// different *sets* from identical code. Those shapes compare notices +
/// shape, not content. Shapes under the limit compare full sorted match-line
/// multisets (slow output keeps its context lines; filtered, not compared).
#[tokio::test]
async fn fast_path_parity() {
    let dir = TempDir::new().unwrap();
    // Small fixed-content tree (contents deterministic; rg's file *order*
    // is not, hence sorted comparison below).
    let files: &[(&str, &str)] = &[
        ("a.txt", "alpha needle one\nnothing here\nbeta needle two\n"),
        ("b.txt", "nothing\nneedle three here\n"),
        ("sub/c.txt", "deep needle four\n"),
        ("awkward.txt", "a:b:c needle\nUPPER needle lower\n"),
        (
            "uni.txt",
            "caf\u{00e9} needle \u{00e9}\nline with \u{00e9}\n",
        ),
        ("crlf.txt", "first needle\r\nsecond line\r\n"),
    ];
    for (name, content) in files {
        let p = dir.path().join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }
    let ctx = ctx_for(dir.path());
    let grep = GrepTool;

    fn is_match_line(l: &str, rels: &[&str]) -> bool {
        rels.iter().any(|r| {
            l.strip_prefix(r).is_some_and(|t| {
                t.starts_with(':') && t[1..].starts_with(|c: char| c.is_ascii_digit())
            })
        })
    }
    fn is_context_line(l: &str, rels: &[&str]) -> bool {
        rels.iter().any(|r| {
            l.strip_prefix(r)
                .and_then(|t| t.strip_prefix('-'))
                .is_some_and(|t| {
                    let n: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
                    !n.is_empty() && t[n.len()..].starts_with("- ")
                })
        })
    }
    // (sorted match lines, sorted non-context remainder incl. notices)
    fn split<'a>(s: &'a str, rels: &[&str]) -> (Vec<&'a str>, Vec<&'a str>) {
        let mut m = Vec::new();
        let mut rest = Vec::new();
        for l in s.lines() {
            if is_match_line(l, rels) {
                m.push(l);
            } else if !is_context_line(l, rels) {
                rest.push(l);
            }
        }
        m.sort_unstable();
        rest.sort_unstable();
        (m, rest)
    }

    // (args, rels visible in output, hits_limit?)
    // Dir search ".": rels are root-relative. Single-file: rel = file_name.
    let dir_rels: &[&str] = &[
        "a.txt",
        "b.txt",
        "sub/c.txt",
        "awkward.txt",
        "uni.txt",
        "crlf.txt",
    ];
    let shapes: Vec<(serde_json::Value, &[&str], bool)> = vec![
        (json!({"pattern": "needle"}), dir_rels, false),
        (
            json!({"pattern": "needle", "glob": "*.txt"}),
            dir_rels,
            false,
        ),
        (json!({"pattern": "a:b", "literal": true}), dir_rels, false),
        (
            json!({"pattern": "NEEDLE", "ignoreCase": true}),
            dir_rels,
            false,
        ),
        (json!({"pattern": "caf\u{00e9}"}), dir_rels, false),
        (json!({"pattern": "no_such_needle_xyz"}), dir_rels, false),
        (
            json!({"pattern": "needle", "path": "a.txt", "limit": 1}),
            &["a.txt"],
            false,
        ),
        // 8 total matches > limit 5: file order decides the set; compare
        // notices + shape only.
        (json!({"pattern": "needle", "limit": 5}), dir_rels, true),
    ];
    // In-process lane gate: literal:true must agree with the --json lane
    // on every shape below (same sorted-match / notices comparison).
    // Non-literal shapes bypass the lane, so they pin vimgrep-vs-json only.
    let mut lit_shapes = shapes.clone();
    lit_shapes.push((
        json!({"pattern": "needle", "literal": true}),
        dir_rels,
        false,
    ));
    lit_shapes.push((
        json!({"pattern": "NEEDLE", "literal": true, "ignoreCase": true}),
        dir_rels,
        false,
    ));
    lit_shapes.push((
        json!({"pattern": "needle", "literal": true, "glob": "*.txt"}),
        dir_rels,
        false,
    ));
    lit_shapes.push((
        json!({"pattern": "needle", "literal": true, "limit": 5}),
        dir_rels,
        true,
    ));
    lit_shapes.push((
        json!({"pattern": "no_such_needle_xyz", "literal": true}),
        dir_rels,
        false,
    ));
    for (args, rels, hits_limit) in lit_shapes.iter().chain(shapes.iter()) {
        let mut fast_args = args.clone();
        fast_args["context"] = json!(0);
        let fast = grep.execute(&ctx, fast_args).await;
        let mut slow_args = args.clone();
        slow_args["context"] = json!(1);
        let slow = grep.execute(&ctx, slow_args).await;
        assert_eq!(
            fast.is_error, slow.is_error,
            "error flag diverges for {args}"
        );
        let (fm, frest) = split(&fast.content, rels);
        let (sm, srest) = split(&slow.content, rels);
        if *hits_limit {
            // Same count (both stop at the limit), same notices; the *set*
            // may differ by rg file order, so content is not compared.
            assert_eq!(fm.len(), sm.len(), "match count diverges for {args}");
            assert_eq!(frest, srest, "notices diverge for {args}");
            assert!(
                frest.iter().any(|l| l.contains("matches limit reached")),
                "limit notice missing for {args}"
            );
        } else {
            assert_eq!(fm, sm, "match lines diverge for {args}");
            assert_eq!(frest, srest, "notices diverge for {args}");
        }
    }
}
