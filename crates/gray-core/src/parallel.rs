//! Parallel batch lane (Toolrush port): run a turn's provably
//! non-interfering tool calls concurrently, preserving input order.
//! Validation, hooks, events, and history writes stay on the loop thread in
//! order — only `executor.execute` runs concurrently. Interference is the
//! one admission rule ([`classify_call`]): readers batch with readers,
//! writers only with calls whose enumerated read/write sets are pairwise
//! disjoint from theirs. Anything that cannot be shown disjoint is a
//! barrier and runs on the existing sequential path.

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
/// [`bash_is_batchable`], and `write` / `edit` only per call through the
/// `path` presence screen plus the pairwise disjoint-path check in
/// [`plan_segments`] (both applied in `plan_segments`). Everything else —
/// sidecar tools — is a barrier.
pub fn is_batchable(name: &str) -> bool {
    matches!(
        name,
        "read" | "ls" | "find" | "grep" | "bash" | "write" | "edit"
    )
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
/// file-backed script runs (`python x.py`), `eval`, and anything smuggled
/// past whole-string token matching. Stderr-only redirects (`2>&1`,
/// `2>/dev/null`) create no files, so they are ignored — every other `>`
/// demotes. `<` (input-only) stays allowed.
pub fn bash_is_batchable(command: &str) -> bool {
    // Stderr-only redirects merge into the captured stream (`2>&1`) or
    // discard (`2>/dev/null`): strip them first so harmless
    // `git diff 2>&1` still batches (the bash tool merges stderr anyway).
    let scrubbed = command.replace("2>&1", "").replace("2>/dev/null", "");
    // Output redirection in any form (`>`, `>>`, `2>`, `&>`, `| … >`).
    if scrubbed.contains('>') {
        return false;
    }
    // Whole-string word tokens: `scp` as one token never equals `cp`, and
    // `skill` never equals `kill`. Quote and substitution chars are
    // delimiters, so `$(rm …)` still matches `rm`; conversely
    // `grep "rm -rf" f` merely demotes (fail-safe, not wrong).
    let toks: Vec<&str> = scrubbed
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
    // Fetch-and-run primitives: `wget` writes by default; inline interpreter
    // code (`python -c`, `node -e`) and `curl` with an output flag can write
    // arbitrary files; `./x` / `sh x.sh` execute script files. Bare
    // `python --version`, bare `curl url`, and `bash -c '…'` (payload is
    // screened above as part of the whole string) stay allowed.
    if toks.contains(&"wget") {
        return false;
    }
    if matches!(toks.first(), Some(s) if *s == "." || *s == "..") {
        return false;
    }
    if inline_code_executes(&toks) || curl_writes_file(&toks) || shell_runs_script(&toks) {
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

/// Inline interpreter code (`python -c`, `node -e`, …) can write through the
/// interpreter — demote. Scoped to interpreter + flag co-occurrence so
/// `sed -e` / `grep -e` (no interpreter present) stay allowed; bare
/// `python --version` (no `-c`/`-e`) stays allowed.
fn inline_code_executes(toks: &[&str]) -> bool {
    const INTERPS: &[&str] = &["python", "python3", "node", "ruby", "perl"];
    toks.iter().any(|t| INTERPS.contains(t)) && toks.iter().any(|t| *t == "-c" || *t == "-e")
}

/// `curl` alone just prints — demote only with an output flag (`-o` / `-O`
/// incl. clusters like `-sSO`, `--output` / `--remote-name`).
fn curl_writes_file(toks: &[&str]) -> bool {
    if !toks.contains(&"curl") {
        return false;
    }
    toks.iter().any(|t| {
        *t == "-o" || *t == "-O" || *t == "--output" || *t == "--remote-name" || {
            t.len() > 2 && t.starts_with('-') && !t.starts_with("--") && t[1..].contains(['o', 'O'])
        }
    })
}

/// `sh script.sh` / `bash deploy.sh` runs a file (may mutate) — demote.
/// `bash -c '…'` executes a string whose words are screened above as part
/// of the whole command, so the `-c` payload itself is skipped here;
/// flag-only invocations (`bash --version`) stay allowed.
fn shell_runs_script(toks: &[&str]) -> bool {
    const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash"];
    for (i, t) in toks.iter().enumerate() {
        if !SHELLS.contains(t) {
            continue;
        }
        let mut j = i + 1;
        let mut skip_next = false;
        while j < toks.len() {
            let a = toks[j];
            if skip_next {
                skip_next = false;
            } else if a == "-c" || a == "--command" {
                skip_next = true;
            } else if !a.starts_with('-') {
                return true;
            }
            j += 1;
        }
    }
    false
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

/// The write class: `write` / `edit` both target one filesystem path.
fn is_write_call(name: &str) -> bool {
    matches!(name, "write" | "edit")
}

/// Lexically normalize a `write`/`edit` target path (no I/O): collapse
/// repeated `/`, resolve `.` / `..` segments lexically, drop the trailing
/// `/`. Over-normalizing errs toward barrier (safe); under-normalizing
/// would err toward false-disjoint (unsafe) — hence the `..` resolution.
fn normalize_write_path(raw: &str) -> String {
    let mut segs: Vec<&str> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if segs.last().is_some_and(|s| *s != "..") {
                    segs.pop();
                } else if !raw.starts_with('/') {
                    segs.push("..");
                }
            }
            s => segs.push(s),
        }
    }
    let mut out = segs.join("/");
    if raw.starts_with('/') {
        out.insert(0, '/');
    }
    if out.is_empty() {
        out.push_str(if raw.starts_with('/') { "/" } else { "." });
    }
    out
}

/// Target path of a `write`/`edit` call (already normalized), or `None` for
/// other tools or when `path` is missing/non-string/empty (fail-safe →
/// barrier).
fn write_target_path(name: &str, args: &Value) -> Option<String> {
    if !is_write_call(name) {
        return None;
    }
    match args.get("path").and_then(Value::as_str) {
        Some(p) if !p.is_empty() => Some(normalize_write_path(p)),
        _ => None,
    }
}

/// Verb allow-list: non-flag arguments name the files the command reads.
/// Recursive members (`grep`, `find`, `rg`) are listed too — a `.` operand
/// overlaps everything ([`touched_paths_overlap`]), which is the correct
/// reading anyway.
const READ_VERBS: &[&str] = &[
    "cat",
    "head",
    "tail",
    "wc",
    "stat",
    "file",
    "diff",
    "cmp",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "shasum",
    "xxd",
    "od",
    "strings",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "sort",
    "uniq",
    "cut",
    "tr",
    "tac",
    "nl",
    "ls",
    "grep",
    "rg",
    "find",
    "sed",
    "jq",
    "du",
    "df",
    "test",
];

/// Verbs that name no files: their arguments are output text or settings,
/// so a redirected `echo … > f` touches only `f`.
const NOPATH_VERBS: &[&str] = &[
    "echo", "printf", "true", "false", "pwd", "date", "whoami", "id", "uname", "hostname", "env",
    "printenv", "sleep", "seq", "yes", "expr", "which", "type", "command", "umask", "cal", "clear",
];

/// Verbs interpreted as writing every non-flag argument (`mv a b` writes
/// both; `cp a b` counts `a` as written too — over-approximation only ever
/// costs a batch slot). Deliberately argument-enumerable only: `install`,
/// `scp`, `rsync`, `dd`, `shred`, `sudo`, build tools and script runners
/// stay barriers below.
const WRITE_VERBS: &[&str] = &[
    "rm", "rmdir", "mv", "cp", "mkdir", "touch", "ln", "truncate", "chmod", "chown", "chgrp",
    "unlink", "tee",
];

/// What one call provably touches, for the non-interference lane.
///
/// * `Reads(paths)` — read-only with an enumerated read set.
/// * `ReadsUnknown` — read-only by [`bash_is_batchable`], but the files it
///   reads cannot be enumerated (`git status`, `cargo metadata`, pipes,
///   quoted operands): batches with readers, never with a writer.
/// * `Writes { reads, writes }` — may write; both sets are enumerated.
///
/// `None` from [`classify_call`] is a barrier and runs sequentially. The
/// lane is a *concurrency* decision only: the loop's pre-pass still
/// validates every member in order and runs `tool_before` verdicts and
/// `pre_tool` hooks for all of them, so lane membership can never skip a
/// denial or a rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touch {
    Reads(Vec<String>),
    ReadsUnknown,
    Writes {
        reads: Vec<String>,
        writes: Vec<String>,
    },
}

/// Path operands of an argument list: non-flag tokens, normalized. Flags
/// whose value is a path (`sed -e SCRIPT`, `truncate -s 5M`) contribute
/// their value too — over-approximation only ever blocks a batch.
fn path_operands(toks: &[&str]) -> Vec<String> {
    toks.iter()
        .filter(|t| !t.starts_with('-'))
        .filter_map(|t| {
            let t = t.trim();
            if t.is_empty() {
                None
            } else {
                Some(normalize_write_path(t))
            }
        })
        .collect()
}

/// Shell syntax that makes a touched set unenumerable: dynamic words,
/// quoting, chains, subshells, input redirection. Output redirection (`>`)
/// is not screened here — [`bash_touch`] handles it structurally.
fn unenumerable_syntax(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\''
            | '"'
            | '`'
            | '$'
            | '*'
            | '?'
            | '~'
            | '{'
            | '}'
            | '('
            | ')'
            | '|'
            | '&'
            | ';'
            | '!'
            | '#'
            | '<'
            | '='
            | '\\'
    )
}

/// Classify one batchable call; `None` = barrier (sequential path).
fn classify_call(name: &str, args: &Value) -> Option<Touch> {
    if is_write_call(name) {
        // The key confirmed against gray-tools `WriteTool` / `EditTool`,
        // which both read `get_str(&args, "path")`; missing/non-string/
        // empty demotes — fail-safe.
        return write_target_path(name, args).map(|p| Touch::Writes {
            reads: Vec::new(),
            writes: vec![p],
        });
    }
    if name == "bash" {
        return bash_touch(args.get("command").and_then(Value::as_str)?);
    }
    match name {
        // The remaining batchable names are pure-read by construction; a
        // literal `path` bounds the read set, anything else stays unknown.
        "read" | "ls" | "find" | "grep" => match args.get("path").and_then(Value::as_str) {
            Some(p) if !p.is_empty() && !p.chars().any(unenumerable_syntax) => {
                Some(Touch::Reads(vec![normalize_write_path(p)]))
            }
            _ => Some(Touch::ReadsUnknown),
        },
        _ => None,
    }
}

/// Touched set of one `bash` command, or `None` when it cannot be
/// enumerated. Two sources combine: output-redirection targets (`> f`,
/// `>> f`) and the operands of the read/write verb allow-lists.
fn bash_touch(command: &str) -> Option<Touch> {
    // Stderr-only redirects merge into the captured stream (`2>&1`) or
    // discard (`2>/dev/null`), and the bash tool merges stderr anyway.
    let scrubbed = command.replace("2>&1", "").replace("2>/dev/null", "");
    if scrubbed.chars().any(unenumerable_syntax) {
        // Unenumerable shape: read-only calls keep their old lane.
        return bash_is_batchable(command).then_some(Touch::ReadsUnknown);
    }
    let mut parts = scrubbed.split('>');
    let cmd_part = parts.next().unwrap_or("").trim();
    let mut writes = Vec::new();
    for part in parts {
        let word = part
            .trim_start_matches('>')
            .split_whitespace()
            .next()
            .unwrap_or("");
        // A missing or flag-like redirect target is unenumerable.
        if word.is_empty() || word.starts_with('-') {
            return bash_is_batchable(command).then_some(Touch::ReadsUnknown);
        }
        writes.push(normalize_write_path(word));
    }
    let toks: Vec<&str> = cmd_part
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    let Some(verb) = toks.first().copied() else {
        return bash_is_batchable(command).then_some(Touch::ReadsUnknown);
    };
    // Read-only commands keep today's admission exactly (the screen already
    // proved them non-writing), enriched with an enumerated read set.
    if bash_is_batchable(command) {
        return Some(match verb {
            v if READ_VERBS.contains(&v) => Touch::Reads(path_operands(&toks[1..])),
            v if NOPATH_VERBS.contains(&v) => Touch::Reads(Vec::new()),
            _ => Touch::ReadsUnknown,
        });
    }
    // Writing shapes: every write must be an enumerable operand or redirect.
    let mut reads = Vec::new();
    if (verb == "sed" && sed_edits_in_place(&toks)) || WRITE_VERBS.contains(&verb) {
        writes.extend(path_operands(&toks[1..]));
    } else if READ_VERBS.contains(&verb) {
        // `cat f > g`: reads enumerated, target written.
        reads = path_operands(&toks[1..]);
    } else if !NOPATH_VERBS.contains(&verb) {
        // Build tools, script runners, `git`/`cargo`/`npm` mutators, `sudo`,
        // fetch-and-write … — the write set is not enumerable.
        return None;
    }
    if writes.is_empty() && reads.is_empty() {
        return None;
    }
    Some(Touch::Writes { reads, writes })
}

/// True when two calls could interfere: any write/write or read/write path
/// overlap. Read/read never interferes — two readers of one file are what
/// the read-class lane always ran concurrently.
fn touches_interfere(a: &Touch, b: &Touch) -> bool {
    fn any_overlap(xs: &[String], ys: &[String]) -> bool {
        xs.iter()
            .any(|x| ys.iter().any(|y| touched_paths_overlap(x, y)))
    }
    match (a, b) {
        (Touch::Reads(_), Touch::Reads(_)) | (Touch::Reads(_), Touch::ReadsUnknown) => false,
        (Touch::ReadsUnknown, Touch::Reads(_)) | (Touch::ReadsUnknown, Touch::ReadsUnknown) => {
            false
        }
        (Touch::ReadsUnknown, Touch::Writes { .. })
        | (Touch::Writes { .. }, Touch::ReadsUnknown) => true,
        (Touch::Reads(rs), Touch::Writes { writes: ws, .. }) => any_overlap(rs, ws),
        (Touch::Writes { writes: ws, .. }, Touch::Reads(rs)) => any_overlap(ws, rs),
        (
            Touch::Writes {
                reads: ra,
                writes: wa,
            },
            Touch::Writes {
                reads: rb,
                writes: wb,
            },
        ) => any_overlap(wa, wb) || any_overlap(ra, wb) || any_overlap(wa, rb),
    }
}

/// True when two touched paths may alias: same file, one a directory-prefix
/// of the other (`dir` vs `dir/file`), either the whole cwd/root (`.` or
/// `/`), or absolute vs relative (unresolvable without I/O). Erring toward
/// overlap only ever costs a batch slot.
fn touched_paths_overlap(a: &str, b: &str) -> bool {
    let (na, nb) = (normalize_write_path(a), normalize_write_path(b));
    if na == nb {
        return true;
    }
    if na == "." || nb == "." || na == "/" || nb == "/" {
        return true;
    }
    if na.starts_with('/') != nb.starts_with('/') {
        return true;
    }
    let sa: Vec<&str> = na.split('/').filter(|p| !p.is_empty()).collect();
    let sb: Vec<&str> = nb.split('/').filter(|p| !p.is_empty()).collect();
    let n = sa.len().min(sb.len());
    sa[..n] == sb[..n]
}

/// Split `tool_uses` into ordered segments. A maximal contiguous run of
/// batchable calls whose touched sets are pairwise non-interfering
/// ([`classify_call`] + [`touches_interfere`]) with length ≥ 2 becomes one
/// `Parallel` segment (no size cap); everything else (barrier tool, unknown
/// tool, non-object args, singleton, unenumerable `bash`, `write`/`edit`
/// without a path, a call overlapping a run sibling) is `Single`.
/// Segments execute in order, so RAW/WAR *across* segments is preserved by
/// construction — only calls inside one `Parallel` segment overlap, and
/// those are proven non-interfering (disjoint files, no shared state).
/// Flattened indices always equal emission order.
pub fn plan_segments(
    tool_uses: &[(String, String, Value)],
    known: &HashSet<String>,
) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let mut run_touches: Vec<Touch> = Vec::new();
    let flush = |run: &mut Vec<usize>, out: &mut Vec<Segment>| {
        if run.len() >= 2 {
            out.push(Segment::Parallel(std::mem::take(run)));
        } else {
            out.extend(run.drain(..).map(Segment::Single));
        }
    };
    for (idx, (_, name, args)) in tool_uses.iter().enumerate() {
        let touch = if is_batchable(name) && known.contains(name) && args.is_object() {
            classify_call(name, args)
        } else {
            None
        };
        match touch {
            Some(t) if run_touches.iter().all(|u| !touches_interfere(u, &t)) => {
                run.push(idx);
                run_touches.push(t);
            }
            _ => {
                flush(&mut run, &mut out);
                run_touches.clear();
                out.push(Segment::Single(idx));
            }
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

/// Max in-flight tasks for [`join_ordered`]: `GRAY_PARALLEL_MAX` env var
/// parsed as `usize`; unset/invalid/0 → unlimited (every spawned call runs
/// at once). Values are clamped to `Semaphore::MAX_PERMITS`, the largest
/// permit count tokio accepts. Read once per [`join_ordered`] call.
pub fn parallel_max() -> usize {
    std::env::var("GRAY_PARALLEL_MAX")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(tokio::sync::Semaphore::MAX_PERMITS)
        .min(tokio::sync::Semaphore::MAX_PERMITS)
}

/// Run `futs` concurrently, returning `(input_index, output)` in
/// input-index order — one entry per input. Bounded: at most
/// [`parallel_max`] tasks execute at once. All wrappers still spawn
/// immediately so ordering/cancel bookkeeping is unchanged; the semaphore
/// only gates execution inside each task.
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
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(parallel_max()));
    for (idx, fut) in futs {
        let sem = sem.clone();
        set.spawn(async move {
            // Bound execution when a cap is configured; the permit drops when the
            // wrapper returns. Acquire failure is impossible (never closed),
            // reported as data rather than a lost index if it ever happens.
            let Ok(_permit) = sem.acquire_owned().await else {
                return (idx, ToolOutput::error("parallel semaphore closed"));
            };
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

#[path = "parallel_bound_tests.rs"]
#[cfg(test)]
mod bound_tests;

#[path = "parallel_dag_tests.rs"]
#[cfg(test)]
mod dag_tests;

#[path = "parallel_screen_tests.rs"]
#[cfg(test)]
mod screen_tests;
