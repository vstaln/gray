//! Human labels for shell commands: the command under the setup noise, and a
//! short job name taken from it (`cargo-check`, `npm-test`).
//!
//! Only setup that cannot hide work is peeled: `cd <path>`, `export
//! NAME=value`, `set -e…` segments joined by `&&`, `NAME=value` prefixes,
//! and priority/lock/time wrappers (`nice`, `ionice`, `flock`, `timeout`,
//! `env`, `time`, `nohup`, `stdbuf`). Anything with quotes, `$`, backticks
//! or another operator in the peeled part stops the peel, so a label never
//! hides a command that does something.

/// The command a person would recognise, plus the directory a leading
/// `cd` moved into (shown beside it, never dropped silently).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreCommand<'a> {
    pub command: &'a str,
    pub cwd: Option<&'a str>,
}

/// Peel safe setup off `full` (first line only).
pub fn core_command(full: &str) -> CoreCommand<'_> {
    let mut rest = full.lines().next().unwrap_or("").trim();
    let mut cwd = None;
    while let Some((head, tail)) = rest.split_once(" && ") {
        let head = head.trim();
        if let Some(dir) = head.strip_prefix("cd ").map(str::trim)
            && plain(dir)
            && !dir.contains(' ')
        {
            cwd = Some(dir);
        } else if !(is_export(head) || is_set(head)) {
            break;
        }
        rest = tail.trim();
    }
    CoreCommand {
        command: peel_wrappers(rest),
        cwd,
    }
}

/// Words that never hide work: no quoting, expansion, or operators.
fn plain(word: &str) -> bool {
    !word.is_empty()
        && !word.chars().any(|c| {
            matches!(
                c,
                '\'' | '"' | '$' | '`' | ';' | '&' | '|' | '<' | '>' | '(' | ')'
            )
        })
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, value)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && (value.is_empty() || plain(value))
    })
}

fn is_export(segment: &str) -> bool {
    segment.strip_prefix("export ").is_some_and(|vars| {
        let mut words = vars.split_whitespace().peekable();
        words.peek().is_some() && words.all(is_assignment)
    })
}

fn is_set(segment: &str) -> bool {
    segment.strip_prefix("set ").is_some_and(|flags| {
        flags.split_whitespace().all(|w| {
            (w.starts_with('-') || w.starts_with('+')) && w[1..].chars().all(char::is_alphanumeric)
                || w.chars().all(|c| c.is_ascii_lowercase())
        })
    })
}

/// Drop leading `NAME=value` words and wrapper commands (with their own
/// options) from one simple command, returning the rest of the original
/// text untouched.
fn peel_wrappers(cmd: &str) -> &str {
    let mut rest = cmd;
    loop {
        let trimmed = rest.trim_start();
        let Some(word) = trimmed.split_whitespace().next() else {
            return rest.trim();
        };
        if !plain(word) {
            return trimmed;
        }
        let after = |n: usize| -> Option<&str> {
            // Skip `n` words from `trimmed`; None when the command would vanish.
            let mut s = trimmed;
            for _ in 0..n {
                let w = s.split_whitespace().next()?;
                if !plain(w) {
                    return None;
                }
                let at = s.find(w)? + w.len();
                s = &s[at..];
            }
            let s = s.trim_start();
            (!s.is_empty()).then_some(s)
        };
        let skip = if is_assignment(word) {
            1
        } else {
            match word {
                "time" | "nohup" | "exec" | "command" => 1,
                "nice" => 1 + opts(trimmed, &["-n"], 1),
                "ionice" => 1 + opts(trimmed, &["-c", "-n", "-p"], 1),
                "stdbuf" => 1 + opts(trimmed, &["-i", "-o", "-e"], 1),
                "env" => 1 + env_words(trimmed),
                // flock [opts] <lockfile> <command…>; `-c` runs a string: stop.
                "flock" if !has_word(trimmed, "-c") => 2 + opts(trimmed, &["-w", "-E"], 1),
                // timeout [opts] <duration> <command…>
                "timeout" => 2 + opts(trimmed, &["-s", "-k"], 1),
                _ => return trimmed,
            }
        };
        match after(skip) {
            Some(next) => rest = next,
            None => return trimmed,
        }
    }
}

/// How many words after the wrapper (index `start`) are its options:
/// `-x`, `--long`, `-xVALUE`, or `-x VALUE` for flags in `with_value`.
fn opts(cmd: &str, with_value: &[&str], start: usize) -> usize {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let mut i = start;
    while let Some(w) = words.get(i) {
        if !w.starts_with('-') || *w == "-" {
            break;
        }
        i += if with_value.contains(w) { 2 } else { 1 };
    }
    i - start
}

fn env_words(cmd: &str) -> usize {
    cmd.split_whitespace()
        .skip(1)
        .take_while(|w| is_assignment(w) || matches!(*w, "-i" | "-"))
        .count()
}

fn has_word(cmd: &str, word: &str) -> bool {
    cmd.split_whitespace().any(|w| w == word)
}

/// Tools whose first argument names what they do (`cargo check`).
const WITH_SUBCOMMAND: &[&str] = &[
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "npx",
    "git",
    "go",
    "docker",
    "kubectl",
    "make",
    "just",
    "gh",
    "uv",
    "poetry",
    "pip",
    "dotnet",
    "gradle",
    "mvn",
    "mix",
    "rake",
    "bundle",
    "terraform",
    "helm",
    "systemctl",
    "brew",
    "apt",
    "rustup",
];

/// A short, readable job name from a command: `cargo-check`, `npm-test`,
/// `pytest`, `deploy` (for `./scripts/deploy.sh`). Lowercase letters,
/// digits and `-`, at most 24 chars; `job` when nothing usable is left.
pub fn job_name(full: &str) -> String {
    let core = core_command(full).command;
    let mut words = core.split_whitespace();
    let first = words.next().unwrap_or("");
    if !plain(first) {
        // `$(which cargo) build`: the program is computed, nothing to read.
        return "job".to_string();
    }
    let program = first.rsplit('/').next().unwrap_or(first);
    let program = program
        .strip_suffix(".sh")
        .or_else(|| program.strip_suffix(".py"))
        .unwrap_or(program);
    let mut parts = vec![program.to_string()];
    let rest: Vec<&str> = words.collect();
    if matches!(program, "python" | "python3") && rest.first() == Some(&"-m") {
        // `python -m pytest` is pytest.
        parts = vec![rest.get(1).copied().unwrap_or(program).to_string()];
    } else if WITH_SUBCOMMAND.contains(&program)
        && let Some(sub) = rest
            .iter()
            .find(|w| !w.starts_with('-') && !w.starts_with('+'))
        && sub
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ':')
    {
        parts.push(sub.replace(':', "-"));
    }
    let name = slug(&parts.join("-"));
    if name.is_empty() {
        "job".to_string()
    } else {
        name
    }
}

fn slug(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 24 {
            break;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// `name`, or `name-2`, `name-3`… — the first one `taken` does not claim.
pub fn unique_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    (2..)
        .map(|n| format!("{name}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded counter always finds a free name")
}

#[path = "label_tests.rs"]
#[cfg(test)]
mod tests;
