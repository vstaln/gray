//! Parallel batch lane (Toolrush port): run a turn's read-only tool calls
//! concurrently, preserving input order. Validation, hooks, events, and
//! history writes stay on the loop thread in order — only
//! `executor.execute` runs concurrently. Anything outside the batchable set
//! is a barrier and runs on the existing sequential path.

use std::collections::HashSet;

use serde_json::Value;

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
}
