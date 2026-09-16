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

/// Shared env-var guard for tests that flip `GRAY_PARALLEL_READS`. Lives at
/// module scope (not inside `mod tests`) so the `agent.rs` lane tests reuse
/// the same lock — env is process-global.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Default-deny batchable set: statically `Allow` in every approval mode
/// (see test), never prompts. `read` / `ls` / `find` / `grep` are
/// pure-read by construction; `bash` is admitted only per call through
/// [`bash_is_batchable`] (applied in `plan_segments`). Everything else —
/// `write`, `edit`, sidecar tools — is a barrier.
pub fn is_batchable(name: &str) -> bool {
    matches!(name, "read" | "ls" | "find" | "grep" | "bash")
}

/// Cheap static screen: is this `bash` `command` plausibly read-only?
///
/// Heuristic, NOT a sandbox. It demotes obvious mutators to the sequential
/// barrier so the common exploration verbs (`grep -rn`, `sed -n`, `ls`,
/// `cat`, pipes, `git diff`, `cargo metadata`, …) overlap in one batch.
/// Anything it misses still executes — exactly as today, just concurrently
/// with its siblings — and hooks still see every call in the ordered
/// pre-pass. Fail-safe direction throughout: doubt demotes. Correctness
/// never depends on this returning `true`.
///
/// Screened on the model-sent command (before any `tool_before` rewrite).
/// NOT screened — residual risk, accepted: pipe-to-shell (`curl … | sh`),
/// script execution (`./run.sh`, `python x.py`), `eval`, fetch-and-write
/// (`curl -o f`), and anything smuggled past whole-string token matching.
/// `>` demotes unconditionally — including the otherwise-harmless `2>&1`
/// (the bash tool already merges stderr into the header, so the redirect
/// buys nothing). `<` (input-only) stays allowed.
pub fn bash_is_batchable(command: &str) -> bool {
    // Output redirection in any form (`>`, `>>`, `2>`, `&>`, `| … >`).
    if command.contains('>') {
        return false;
    }
    // Whole-string word tokens: `scp` as one token never equals `cp`, and
    // `skill` never equals `kill`. Quote and substitution chars are
    // delimiters, so `$(rm …)` still matches `rm`; conversely
    // `grep "rm -rf" f` merely demotes (fail-safe, not wrong).
    let toks: Vec<&str> = command
        .split(|c: char| {
            matches!(
                c,
                ' ' | '\t'
                    | '\n'
                    | '\r'
                    | ';'
                    | '&'
                    | '|'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '`'
                    | '"'
                    | '\''
                    | '$'
                    | ','
                    | '/'
            )
        })
        .filter(|t| !t.is_empty())
        .collect();
    if toks.iter().any(|t| MUTATOR_WORDS.contains(t)) {
        return false;
    }
    if toks.contains(&"sed") && sed_edits_in_place(&toks) {
        return false;
    }
    for (i, t) in toks.iter().enumerate() {
        let ok = match *t {
            "git" => first_verb_allowed(&toks, i, GIT_READ_ONLY, &["-c", "--config", "-C"]),
            "cargo" => first_verb_allowed(&toks, i, CARGO_READ_ONLY, &[]),
            "npm" => first_verb_allowed(&toks, i, NPM_READ_ONLY, &[]),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Bare mutator words (exact-token match). The spec list (`rm`, `mv`, `cp`,
/// `mkdir`, `touch`, `sed -i` via [`sed_edits_in_place`], `sudo`, `chmod`,
/// `chown`, `kill`, `tee`, `cargo install` / `npm i` via the verb allow-list
/// below) plus same-family obvious mutators: `rmdir`, `unlink`, `ln`,
/// `install` (also catches `pip`/`apt`/`gem install`), `dd`, `shred`,
/// `truncate`, `chgrp`, `pkill`/`killall`, `shutdown`/`reboot`/`poweroff`/
/// `halt`, `npx`, `scp`/`rsync`/`sftp`, `find -delete`.
const MUTATOR_WORDS: &[&str] = &[
    "rm", "mv", "cp", "mkdir", "touch", "rmdir", "unlink", "ln", "install", "dd", "shred",
    "truncate", "chmod", "chown", "chgrp", "sudo", "tee", "kill", "pkill", "killall", "shutdown",
    "reboot", "poweroff", "halt", "npx", "scp", "rsync", "sftp", "-delete",
];

/// `sed` is a read-only stream filter unless it edits in place: `-i` in any
/// short-flag cluster (`-i`, `-ni`, `-i.bak`) or `--in-place`. Only
/// single-dash clusters are inspected, so `--quiet` stays allowed.
fn sed_edits_in_place(toks: &[&str]) -> bool {
    toks.iter().any(|t| {
        if *t == "--in-place" {
            return true;
        }
        match t.strip_prefix('-') {
            Some(flags) if !flags.starts_with('-') => flags.contains('i'),
            _ => false,
        }
    })
}

/// `git` subcommands that never touch the repo or the worktree.
const GIT_READ_ONLY: &[&str] = &["status", "diff", "log", "show", "blame"];

/// `cargo` verbs that never write: just `metadata` — everything else
/// (`build`, `test`, `run`, `install`, …) writes to `target/` or worse.
const CARGO_READ_ONLY: &[&str] = &["metadata"];

/// `npm` verbs that neither execute scripts nor write (`ls`, `view`,
/// `ping`, …). `run` / `test` / `install` / `exec` and the rest demote.
const NPM_READ_ONLY: &[&str] = &[
    "ls", "list", "ll", "la", "view", "info", "show", "v", "ping", "whoami", "help",
];

/// The single verb allow-list behind `git` / `cargo` / `npm`: the first
/// positional word after the tool name must be in `allowed`. Leading
/// `-flags` (and `+toolchain`) are skipped; flags in `takes_value` also
/// consume the next word (`git -c k=v`, `git -C dir`). No positional word
/// at all (bare tool, `cargo --version`) just prints help / version, so it
/// stays allowed.
fn first_verb_allowed(toks: &[&str], at: usize, allowed: &[&str], takes_value: &[&str]) -> bool {
    let mut i = at + 1;
    while i < toks.len() {
        let t = toks[i];
        if takes_value.contains(&t) {
            i += 2; // consume the flag's value
        } else if t.starts_with('-') || t.starts_with('+') {
            i += 1;
        } else {
            return allowed.contains(&t);
        }
    }
    true
}

/// Per-call screen inside the batchable set. `bash` must additionally pass
/// [`bash_is_batchable`] on its model-sent `command`; a missing/non-string
/// command demotes (fail-safe — validation reports it later on the
/// sequential path). Every other batchable name is pure-read by
/// construction.
fn call_screen_passes(name: &str, args: &Value) -> bool {
    if name != "bash" {
        return true;
    }
    args.get("command")
        .and_then(Value::as_str)
        .is_some_and(bash_is_batchable)
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
/// batchable-known-object-args calls of length ≥ 2 becomes one `Parallel`
/// segment (no size cap); everything else (barrier tool, unknown tool,
/// non-object args, singleton, `bash` failing [`bash_is_batchable`]) is
/// `Single`.
/// Flattened indices always equal emission order.
pub fn plan_segments(
    tool_uses: &[(String, String, Value)],
    known: &HashSet<String>,
) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let flush = |run: &mut Vec<usize>, out: &mut Vec<Segment>| {
        if run.len() >= 2 {
            out.push(Segment::Parallel(std::mem::take(run)));
        } else {
            out.extend(run.drain(..).map(Segment::Single));
        }
    };
    for (idx, (_, name, args)) in tool_uses.iter().enumerate() {
        if is_batchable(name)
            && known.contains(name)
            && args.is_object()
            && call_screen_passes(name, args)
        {
            run.push(idx);
        } else {
            flush(&mut run, &mut out);
            out.push(Segment::Single(idx));
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Bounded window for cancelled tools to report before they are aborted.
/// A cancelled tool shares the run's `ctx`, so it is already signalled and
/// owns the cleanup that matters (kill its process group, drain its output
/// pump, return partial output). Dropping it first throws that report away
/// and forces the caller to fabricate a placeholder. Kept below the REPL
/// turn's 5s cooperative window (`prompt_turn::run_streaming_cancellable`)
/// so the inner report always lands inside the outer one.
pub const CANCEL_REPORT_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Run `futs` concurrently, returning `(input_index, output)` in
/// input-index order — one entry per input. No cap: every call in the
/// batch is in flight at once.
/// `None` output means the call never completed (cancel fired first).
/// Task panics become `is_error` outputs under the panicking call's real
/// input index — tool failures are data, never crashes.
pub async fn join_ordered(
    futs: Vec<(usize, BoxFuture<'static, ToolOutput>)>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Vec<(usize, Option<ToolOutput>)> {
    use futures::FutureExt as _;
    use std::panic::AssertUnwindSafe;
    let inputs: Vec<usize> = futs.iter().map(|(idx, _)| *idx).collect();
    let mut set: JoinSet<(usize, ToolOutput)> = JoinSet::new();
    for (idx, fut) in futs {
        set.spawn(async move {
            // Panics are data: catch inside the wrapper so the real input
            // index survives — no sentinel, Task 4 reconciles by index.
            match AssertUnwindSafe(fut).catch_unwind().await {
                Ok(out) => (idx, out),
                Err(id) => {
                    let msg = id
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| id.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    (idx, ToolOutput::error(format!("tool task panicked: {msg}")))
                }
            }
        });
    }
    let mut done: Vec<(usize, Option<ToolOutput>)> = Vec::new();
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                // Let already-signalled tools land their own reports first
                // (bounded), then abort whatever is still running. Aborting
                // first would discard partial output the caller can never
                // reconstruct.
                let deadline = tokio::time::sleep(CANCEL_REPORT_GRACE);
                tokio::pin!(deadline);
                loop {
                    tokio::select! {
                        biased;
                        _ = &mut deadline => {
                            set.abort_all();
                            break;
                        }
                        res = set.join_next() => match res {
                            Some(Ok((idx, out))) => done.push((idx, Some(out))),
                            Some(Err(_)) => {}
                            None => break,
                        },
                    }
                }
                while let Some(res) = set.join_next().await {
                    if let Ok((idx, out)) = res {
                        done.push((idx, Some(out)));
                    }
                }
                // Every input not completed yields None — None means
                // cancelled, never "never submitted".
                let completed: HashSet<usize> = done.iter().map(|(i, _)| *i).collect();
                for idx in inputs {
                    if !completed.contains(&idx) {
                        done.push((idx, None));
                    }
                }
                break;
            }
            res = set.join_next() => {
                match res {
                    None => break,
                    Some(Ok((idx, out))) => done.push((idx, Some(out))),
                    // Unreachable in practice: the spawned wrapper catches
                    // panics via catch_unwind, so tasks only end via abort
                    // (handled above) or success.
                    Some(Err(_)) => {}
                }
            }
        }
    }
    done.sort_by_key(|(i, _)| *i);
    done
}

#[path = "parallel_tests.rs"]
#[cfg(test)]
mod tests;
