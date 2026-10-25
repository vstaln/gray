use super::*;
use serde_json::json;
use std::collections::HashSet;

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
fn planner_demotes_bash_without_parsable_command() {
    let known: HashSet<String> = ["read", "bash"].iter().map(|s| s.to_string()).collect();
    let u = vec![
        ("a".into(), "read".into(), json!({"path": "a"})),
        ("b".into(), "bash".into(), json!({})),
        ("c".into(), "bash".into(), json!({"command": 42})),
        ("d".into(), "read".into(), json!({"path": "d"})),
    ];
    assert_eq!(
        plan_segments(&u, &known, &HashSet::new()),
        vec![
            Segment::Single(0),
            Segment::Single(1),
            Segment::Single(2),
            Segment::Single(3),
        ]
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

#[test]
fn archive_extractors_never_batch_as_reads() {
    for command in [
        "tar xf a.tar",
        "tar -xzf release.tgz -C /tmp/x",
        "bsdtar xf a.tar",
        "unzip bundle.zip",
        "gunzip data.gz",
        "7z x pack.7z",
        "cpio -id < in.cpio",
    ] {
        assert!(
            !bash_is_batchable(command),
            "extractor batched as read-only: {command}"
        );
    }

    assert!(bash_is_batchable("git diff --stat"));
}
