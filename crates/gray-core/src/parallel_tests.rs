use super::*;
use serde_json::json;
use std::collections::HashSet;

fn whole_case() -> Vec<(String, String, serde_json::Value)> {
    vec![
        ("a".into(), "read".into(), json!({"path": "x.rs"})),
        ("b".into(), "grep".into(), json!({"pattern": "y"})),
        ("c".into(), "bash".into(), json!({"command": "ls"})),
        ("d".into(), "read".into(), json!({"path": "z.rs"})),
        ("e".into(), "mystery".into(), json!({})),
        ("f".into(), "read".into(), json!("oops")),
    ]
}
fn known() -> HashSet<String> {
    ["read", "grep", "bash"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
#[test]
fn planner_groups_only_contiguous_batchable_runs() {
    // The `bash` call (`ls`: passes the screen) now batches with the
    // surrounding reads; `mystery` (unknown) and the non-object `read`
    // stay barriers.
    assert_eq!(
        plan_segments(&whole_case(), &known()),
        vec![
            Segment::Parallel(vec![0, 1, 2, 3]),
            Segment::Single(4),
            Segment::Single(5),
        ]
    );
}
#[test]
fn singleton_batchable_demotes_to_single() {
    let u = vec![("a".into(), "read".into(), json!({"path": "x"}))];
    assert_eq!(plan_segments(&u, &known()), vec![Segment::Single(0)]);
}
#[test]
fn long_batchable_run_stays_one_segment() {
    // Mixed read + screened-bash run: no cap, still one segment.
    let names = ["read", "bash", "grep", "ls", "find"];
    let u: Vec<(String, String, Value)> = (0..15)
        .map(|i| {
            let name = names[i % names.len()];
            let args = if name == "bash" {
                json!({"command": "grep -rn foo ."})
            } else {
                json!({"path": "x"})
            };
            (format!("c{i}"), name.into(), args)
        })
        .collect();
    let k: HashSet<String> = names.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        plan_segments(&u, &k),
        vec![Segment::Parallel((0..15).collect())]
    );
}
#[test]
fn bash_screen_allows_read_only_verbs() {
    for cmd in [
        "grep -rn foo .",
        "sed -n '1,10p' file.txt",
        "ls -la",
        "cat file.txt",
        "git diff",
        "git status",
        "git log --oneline",
        "git show HEAD --stat",
        "git blame file.rs",
        "cargo metadata --no-deps",
        "cargo --version",
        "cat Cargo.toml | grep name | head -5",
        "ls | wc -l",
        "find . -name '*.rs'",
        "npm ls",
        "npm --version",
        "echo done",
    ] {
        assert!(bash_is_batchable(cmd), "read-only must batch: {cmd}");
    }
}
#[test]
fn bash_screen_demotes_mutators() {
    for cmd in [
        "rm -rf /tmp/x",
        "sed -i 's/a/b/' file",
        "cargo install ripgrep",
        "cargo build",
        "echo hi > file",
        "cat f >> g",
        "cat f | tee g",
        "sudo ls",
        "mv a b",
        "cp -r a b",
        "mkdir -p x",
        "touch x",
        "chmod +x run.sh",
        "git commit -m x",
        "git push",
        "npm i lodash",
        "npm install",
        "npm test",
        "kill 1234",
        "find . -delete",
    ] {
        assert!(!bash_is_batchable(cmd), "mutator must demote: {cmd}");
    }
}
#[test]
fn planner_batches_mixed_read_bash_run_and_splits_at_mutator() {
    let known: HashSet<String> = ["read", "bash", "grep"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let ok = vec![
        ("a".into(), "read".into(), json!({"path": "x.rs"})),
        (
            "b".into(),
            "bash".into(),
            json!({"command": "grep -rn foo ."}),
        ),
        (
            "c".into(),
            "bash".into(),
            json!({"command": "sed -n '1,10p' f"}),
        ),
        ("d".into(), "grep".into(), json!({"pattern": "y"})),
    ];
    assert_eq!(
        plan_segments(&ok, &known),
        vec![Segment::Parallel(vec![0, 1, 2, 3])]
    );
    let split = vec![
        ("a".into(), "read".into(), json!({"path": "x.rs"})),
        (
            "b".into(),
            "bash".into(),
            json!({"command": "rm -rf /tmp/x"}),
        ),
    ];
    assert_eq!(
        plan_segments(&split, &known),
        vec![Segment::Single(0), Segment::Single(1)]
    );
}
#[test]
fn planner_demotes_bash_without_parsable_command() {
    let known: HashSet<String> = ["read", "bash"].iter().map(|s| s.to_string()).collect();
    let u = vec![
        ("a".into(), "read".into(), json!({"path": "a"})),
        ("b".into(), "bash".into(), json!({})),
        ("c".into(), "bash".into(), json!({"command": 42})),
        ("d".into(), "read".into(), json!({"path": "d"})),
    ];
    assert_eq!(
        plan_segments(&u, &known),
        vec![
            Segment::Single(0),
            Segment::Single(1),
            Segment::Single(2),
            Segment::Single(3),
        ]
    );
}
#[test]
fn kill_switch_parses() {
    let _g = ENV_LOCK.lock().unwrap();
    let prev = std::env::var("GRAY_PARALLEL_READS").ok();
    for (val, want) in [
        ("0", false),
        ("false", false),
        ("no", false),
        ("off", false),
        ("FALSE", false),
        ("1", true),
        ("yes", true),
    ] {
        unsafe { std::env::set_var("GRAY_PARALLEL_READS", val) };
        assert_eq!(parallel_enabled(), want, "{val}");
    }
    unsafe { std::env::remove_var("GRAY_PARALLEL_READS") };
    assert!(parallel_enabled(), "default is on");
    match prev {
        Some(v) => unsafe { std::env::set_var("GRAY_PARALLEL_READS", v) },
        None => unsafe { std::env::remove_var("GRAY_PARALLEL_READS") },
    }
}
#[tokio::test]
async fn mixed_batch_of_five_overlaps_wall_clock() {
    use crate::agent::ToolOutput;
    use futures::future::BoxFuture;
    // SPEC-02 live check: a turn emitting 5 read-only calls plans one
    // segment and completes them concurrently (wall ~= slowest, not sum).
    let u = vec![
        ("a".into(), "read".into(), json!({"path": "a.rs"})),
        (
            "b".into(),
            "bash".into(),
            json!({"command": "grep -rn foo ."}),
        ),
        (
            "c".into(),
            "bash".into(),
            json!({"command": "sed -n '1,10p' f"}),
        ),
        ("d".into(), "grep".into(), json!({"pattern": "y"})),
        ("e".into(), "grep".into(), json!({"pattern": "y"})),
    ];
    let k: HashSet<String> = ["read", "bash", "grep"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        plan_segments(&u, &k),
        vec![Segment::Parallel(vec![0, 1, 2, 3, 4])]
    );
    let mk = |i: usize| {
        (
            i,
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                ToolOutput::ok(format!("out{i}"))
            }) as BoxFuture<'static, ToolOutput>,
        )
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let t0 = std::time::Instant::now();
    let got = join_ordered(vec![mk(0), mk(1), mk(2), mk(3), mk(4)], &cancel).await;
    let dt = t0.elapsed();
    assert_eq!(got.len(), 5);
    assert_eq!(
        got.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert!(
        dt < std::time::Duration::from_millis(800),
        "5x200ms overlapped, took {dt:?}"
    );
}
#[tokio::test]
async fn join_runs_concurrently_and_returns_input_order() {
    use crate::agent::ToolOutput;
    use futures::future::BoxFuture;
    let mk = |i: usize| {
        (
            i,
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                ToolOutput::ok(format!("out{i}"))
            }) as BoxFuture<'static, ToolOutput>,
        )
    };
    let futs = vec![mk(0), mk(1), mk(2), mk(3)];
    let cancel = tokio_util::sync::CancellationToken::new();
    let t0 = std::time::Instant::now();
    let got = join_ordered(futs, &cancel).await;
    let dt = t0.elapsed();
    assert_eq!(
        got.into_iter()
            .map(|(i, o)| (i, o.unwrap().content))
            .collect::<Vec<_>>(),
        vec![
            (0, "out0".into()),
            (1, "out1".into()),
            (2, "out2".into()),
            (3, "out3".into())
        ]
    );
    assert!(
        dt < std::time::Duration::from_millis(300),
        "4x80ms overlapped, took {dt:?}"
    );
}
#[tokio::test]
async fn join_task_panic_becomes_error_output() {
    use crate::agent::ToolOutput;
    use futures::future::BoxFuture;
    let futs = vec![(
        0,
        Box::pin(async {
            panic!("boom");
            #[allow(unreachable_code)]
            ToolOutput::error("unreachable")
        }) as BoxFuture<'static, ToolOutput>,
    )];
    let cancel = tokio_util::sync::CancellationToken::new();
    let got = join_ordered(futs, &cancel).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, 0, "panic keeps its real input index");
    let (_, out) = &got[0];
    assert!(
        out.as_ref().unwrap().is_error,
        "panic must be data, not a crash"
    );
}
#[tokio::test]
async fn join_cancel_marks_unfinished_none() {
    use crate::agent::ToolOutput;
    use futures::future::BoxFuture;
    let mk = |i: usize| {
        (
            i,
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                #[allow(unreachable_code)]
                ToolOutput::ok(format!("out{i}"))
            }) as BoxFuture<'static, ToolOutput>,
        )
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        c2.cancel();
    });
    let got = join_ordered(vec![mk(0), mk(1)], &cancel).await;
    assert_eq!(
        got.len(),
        2,
        "every input yields an entry, even when cancelled"
    );
    assert_eq!(got.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 1]);
    assert!(
        got.iter().all(|(_, o)| o.is_none()),
        "cancelled calls yield None"
    );
}
