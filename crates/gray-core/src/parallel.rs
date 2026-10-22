use std::collections::HashSet;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::task::JoinSet;

use crate::agent::ToolOutput;

pub fn bash_is_batchable(command: &str) -> bool {
    let scrubbed = command.replace("2>&1", "").replace("2>/dev/null", "");

    if scrubbed.contains('>') {
        return false;
    }

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

const MUTATOR_WORDS: &[&str] = &[
    "rm", "mv", "cp", "mkdir", "touch", "rmdir", "unlink", "ln", "install", "dd", "shred",
    "truncate", "chmod", "chown", "chgrp", "sudo", "tee", "kill", "pkill", "killall", "shutdown",
    "reboot", "poweroff", "halt", "npx", "scp", "rsync", "sftp", "-delete", "tar", "bsdtar",
    "unzip", "gunzip", "7z", "cpio",
];

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

fn inline_code_executes(toks: &[&str]) -> bool {
    const INTERPS: &[&str] = &["python", "python3", "node", "ruby", "perl"];
    toks.iter().any(|t| INTERPS.contains(t)) && toks.iter().any(|t| *t == "-c" || *t == "-e")
}

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

const GIT_READ_ONLY: &[&str] = &["status", "diff", "log", "show", "blame"];

const CARGO_READ_ONLY: &[&str] = &["metadata"];

const NPM_READ_ONLY: &[&str] = &[
    "ls", "list", "ll", "la", "view", "info", "show", "v", "ping", "whoami", "help",
];

fn first_verb_allowed(toks: &[&str], at: usize, allowed: &[&str], takes_value: &[&str]) -> bool {
    let mut i = at + 1;
    while i < toks.len() {
        let t = toks[i];
        if takes_value.contains(&t) {
            i += 2;
        } else if t.starts_with('-') || t.starts_with('+') {
            i += 1;
        } else {
            return allowed.contains(&t);
        }
    }
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Parallel(Vec<usize>),
    Single(usize),
}

/// A call can join a parallel run only if it is a read-only bash command.
fn batchable(name: &str, args: &Value) -> bool {
    name == "bash"
        && args
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(bash_is_batchable)
}

/// Groups consecutive read-only bash calls into parallel runs; everything
/// else runs on its own, in order.
pub fn plan_segments(
    tool_uses: &[(String, String, Value)],
    known: &HashSet<String>,
) -> Vec<Segment> {
    let lane = known.contains("bash");
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
        if lane && batchable(name, args) {
            run.push(idx);
        } else {
            flush(&mut run, &mut out);
            out.push(Segment::Single(idx));
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Positions of calls a `tool_before` hook rewrote out of the read-only form.
pub fn rewrite_conflicts(calls: &[(&str, &Value)]) -> Vec<usize> {
    calls
        .iter()
        .enumerate()
        .filter(|(_, (name, args))| !batchable(name, args))
        .map(|(pos, _)| pos)
        .collect()
}

pub const CANCEL_REPORT_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

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

#[path = "parallel_screen_tests.rs"]
#[cfg(test)]
mod screen_tests;
