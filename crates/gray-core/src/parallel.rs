//! Parallel batch lane (Toolrush port): run a turn's read-only tool calls
//! concurrently, preserving input order. Validation, hooks, events, and
//! history writes stay on the loop thread in order — only
//! `executor.execute` runs concurrently. Anything outside the batchable set
//! is a barrier and runs on the existing sequential path.

use std::collections::HashSet;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::task::JoinSet;

use crate::agent::ToolOutput;

/// Max calls per parallel run (Toolrush `MAX_BATCH`).
pub const MAX_BATCH: usize = 16;
/// Max concurrent in-flight tool executions (Toolrush `MAX_WORKERS`).
pub const MAX_WORKERS: usize = 4;

/// Default-deny batchable set: statically `Allow` in every approval mode
/// (see test), never prompts, never mutates. Everything else is a barrier.
pub fn is_batchable(name: &str) -> bool {
    matches!(name, "read" | "ls" | "find" | "grep")
}

/// `GRAY_PARALLEL_READS=0|false|no|off` (any case) disables the lane;
/// unset or anything else enables it. Read per turn (matches the
/// `GRAY_PERMISSION` convention of reading env at decision time).
pub fn parallel_enabled() -> bool {
    match std::env::var("GRAY_PARALLEL_READS") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// One execution segment: a parallel run over `tool_uses` indices, or a
/// single call on the sequential path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Parallel(Vec<usize>),
    Single(usize),
}

/// Split `tool_uses` into ordered segments. A maximal contiguous run of
/// batchable-known-object-args calls of length ≥ 2 becomes `Parallel`
/// (capped at `MAX_BATCH` per run — longer runs split); everything else
/// (barrier tool, unknown tool, non-object args, singleton) is `Single`.
/// Flattened indices always equal emission order.
pub fn plan_segments(
    tool_uses: &[(String, String, Value)],
    known: &HashSet<String>,
) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let flush = |run: &mut Vec<usize>, out: &mut Vec<Segment>| {
        for chunk in run.chunks(MAX_BATCH) {
            if chunk.len() >= 2 {
                out.push(Segment::Parallel(chunk.to_vec()));
            } else {
                out.extend(chunk.iter().map(|&i| Segment::Single(i)));
            }
        }
        run.clear();
    };
    for (idx, (_, name, args)) in tool_uses.iter().enumerate() {
        if is_batchable(name) && known.contains(name) && args.is_object() {
            run.push(idx);
        } else {
            flush(&mut run, &mut out);
            out.push(Segment::Single(idx));
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Run `futs` with at most `max_workers` in flight, returning
/// `(input_index, output)` in input-index order. `None` output means the
/// call never completed (cancel fired first). Task panics become
/// `is_error` outputs — tool failures are data, never crashes.
pub async fn join_ordered(
    futs: Vec<(usize, BoxFuture<'static, ToolOutput>)>,
    max_workers: usize,
    cancel: &tokio_util::sync::CancellationToken,
) -> Vec<(usize, Option<ToolOutput>)> {
    use std::sync::Arc;
    let sem = Arc::new(tokio::sync::Semaphore::new(max_workers.max(1)));
    let mut set: JoinSet<(usize, ToolOutput)> = JoinSet::new();
    for (idx, fut) in futs {
        let permit_owner = sem.clone();
        set.spawn(async move {
            let _permit = permit_owner.acquire_owned().await.expect("semaphore closed");
            (idx, fut.await)
        });
    }
    let mut done: Vec<(usize, Option<ToolOutput>)> = Vec::new();
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                set.abort_all();
                while let Some(res) = set.join_next().await {
                    if let Ok((idx, out)) = res {
                        done.push((idx, Some(out)));
                    }
                }
                break;
            }
            res = set.join_next() => {
                match res {
                    None => break,
                    Some(Ok((idx, out))) => done.push((idx, Some(out))),
                    Some(Err(e)) => {
                        // JoinError here means the task panicked (cancellation
                        // is handled above via abort_all + graceful drain).
                        if let Ok(id) = e.try_into_panic() {
                            let msg = id.downcast_ref::<&str>().copied()
                                .or_else(|| id.downcast_ref::<String>().map(String::as_str))
                                .unwrap_or("unknown panic");
                            // Index is lost on panic; attribute to no call —
                            // the loop layer treats a shortfall as an error.
                            done.push((usize::MAX, Some(ToolOutput::error(format!("tool task panicked: {msg}")))));
                        }
                    }
                }
            }
        }
    }
    done.sort_by_key(|(i, _)| *i);
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        assert_eq!(
            plan_segments(&whole_case(), &known()),
            vec![
                Segment::Parallel(vec![0, 1]),
                Segment::Single(2),
                Segment::Single(3),
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
    fn batchable_set_is_statically_allowed_in_every_mode() {
        for tool in ["read", "ls", "find", "grep"] {
            for mode in ["read-only", "auto", "full"] {
                assert_eq!(
                    crate::approvals::verdict(
                        mode,
                        tool,
                        &json!({}),
                        std::path::Path::new("/work")
                    ),
                    crate::approvals::Verdict::Allow,
                    "{tool} in {mode}"
                );
            }
        }
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
    async fn join_runs_concurrently_and_returns_input_order() {
        use crate::agent::ToolOutput;
        use futures::future::BoxFuture;
        let mk = |i: usize| {
            (i, Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                ToolOutput::ok(format!("out{i}"))
            }) as BoxFuture<'static, ToolOutput>)
        };
        let futs = vec![mk(0), mk(1), mk(2), mk(3)];
        let cancel = tokio_util::sync::CancellationToken::new();
        let t0 = std::time::Instant::now();
        let got = join_ordered(futs, 4, &cancel).await;
        let dt = t0.elapsed();
        assert_eq!(
            got.into_iter().map(|(i, o)| (i, o.unwrap().content)).collect::<Vec<_>>(),
            vec![(0, "out0".into()), (1, "out1".into()), (2, "out2".into()), (3, "out3".into())]
        );
        assert!(dt < std::time::Duration::from_millis(300), "4x80ms overlapped, took {dt:?}");
    }
    #[tokio::test]
    async fn join_task_panic_becomes_error_output() {
        use crate::agent::ToolOutput;
        use futures::future::BoxFuture;
        let futs = vec![(0, Box::pin(async { panic!("boom"); #[allow(unreachable_code)] ToolOutput::error("unreachable") }) as BoxFuture<'static, ToolOutput>)];
        let cancel = tokio_util::sync::CancellationToken::new();
        let got = join_ordered(futs, 4, &cancel).await;
        assert_eq!(got.len(), 1);
        let (_, out) = &got[0];
        assert!(out.as_ref().unwrap().is_error, "panic must be data, not a crash");
    }
    #[tokio::test]
    async fn join_cancel_marks_unfinished_none() {
        use crate::agent::ToolOutput;
        use futures::future::BoxFuture;
        let mk = |i: usize| {
            (i, Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                #[allow(unreachable_code)]
                ToolOutput::ok(format!("out{i}"))
            }) as BoxFuture<'static, ToolOutput>)
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            c2.cancel();
        });
        let got = join_ordered(vec![mk(0), mk(1)], 4, &cancel).await;
        assert!(got.iter().all(|(_, o)| o.is_none()), "cancelled calls yield None");
    }
}
