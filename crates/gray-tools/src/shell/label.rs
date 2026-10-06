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

/// What a read-only command did, for a header that names it (`Read`,
/// `Viewed`) instead of a bare `Ran`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// Bare `cat <media…>`: the bash tool attaches the files to the turn.
    Viewed,
    Read,
    Listed,
    Searched,
}

/// A command recognised as read-only: its verb, what it looked at (files,
/// a directory, or a search pattern), and an optional detail
/// (`lines 10–40`, `in src`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnly {
    pub look: Look,
    /// Files or directories (`Viewed`/`Read`/`Listed`), or the pattern
    /// (`Searched`), exactly as written in the command.
    pub targets: Vec<String>,
    pub detail: Option<String>,
}

/// Classify a command as a read, a listing, a search or a media view.
///
/// Conservative by design: a label must never make a write look like a
/// read. Anything with operators (`;`, `&&`, `$(…)`, backticks), any
/// redirect other than discarding stderr/stdout, a second line, a known
/// writing flag (`sed -i`, `find -delete`, `sort -o`, `rg --pre`, …), or a
/// pipe into something outside a short list of read-only filters is `None`,
/// and the header stays `Ran`.
pub fn read_only(full: &str) -> Option<ReadOnly> {
    let full = full.trim();
    if full.contains('\n') {
        return None;
    }
    let core = core_command(full);
    let stages = pipeline(core.command)?;
    let (first, rest) = stages.split_first()?;
    if !rest.iter().all(|stage| is_filter(stage)) {
        return None;
    }
    let program = program_name(first.first()?);
    let args = &first[1..];
    let classified = match program {
        "cat" if !args.is_empty() => {
            let media = args.iter().all(|a| {
                !a.starts_with('-') && crate::images::is_viewable_extension(std::path::Path::new(a))
            });
            // Only the exact shape the bash tool claims (`cat <media…>` as
            // the whole command) attaches anything; `cd … && cat x.png` or
            // `cat x.png | head` dumps bytes, so it is a plain read.
            let look = if media && rest.is_empty() && core.command == full {
                Look::Viewed
            } else {
                Look::Read
            };
            files(look, args, &[], None)
        }
        "bat" | "batcat" => files(Look::Read, args, BAT_VALUE_FLAGS, None),
        "nl" | "less" | "more" => files(Look::Read, args, NL_VALUE_FLAGS, None),
        "head" | "tail" => {
            let detail = head_tail_detail(program, args);
            files(
                Look::Read,
                args,
                &["-n", "-c", "--lines", "--bytes"],
                detail,
            )
        }
        "sed" => sed_read(args),
        "ls" | "tree" | "eza" | "exa" => {
            if program == "tree" && args.iter().any(|a| a == "-o" || a.starts_with("-o")) {
                return None;
            }
            let dirs = positionals(args, LS_VALUE_FLAGS);
            Some(ReadOnly {
                look: Look::Listed,
                targets: if dirs.is_empty() {
                    vec![".".into()]
                } else {
                    dirs
                },
                detail: None,
            })
        }
        "grep" | "egrep" | "fgrep" | "rg" | "ag" => search(program, args),
        "find" => find_read(args),
        "fd" | "fdfind" => fd_read(args),
        _ => None,
    }?;
    (!classified.targets.is_empty()).then_some(classified)
}

/// Split a single-line command into pipeline stages of shell words. `None`
/// for anything a label cannot vouch for: unbalanced quotes, `$`/backticks
/// (even inside double quotes), `;`, `&&`, `||`, subshells, comments, and
/// redirects other than the harmless ones in [`SAFE_REDIRECTS`].
fn pipeline(cmd: &str) -> Option<Vec<Vec<String>>> {
    let mut stages = vec![Vec::new()];
    let mut word = String::new();
    let mut started = false;
    let mut special = false;
    let mut chars = cmd.chars().peekable();
    let finish = |word: &mut String,
                  started: &mut bool,
                  special: &mut bool,
                  stage: &mut Vec<String>|
     -> Option<()> {
        if *started {
            if *special {
                if !SAFE_REDIRECTS.contains(&word.as_str()) {
                    return None;
                }
            } else {
                stage.push(std::mem::take(word));
            }
        }
        word.clear();
        *started = false;
        *special = false;
        Some(())
    };
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                finish(&mut word, &mut started, &mut special, stages.last_mut()?)?;
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '$' | '`' => return None,
                        '\\' => word.push(chars.next()?),
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                started = true;
                word.push(chars.next()?);
            }
            '#' if !started => return None,
            '|' => {
                if chars.peek() == Some(&'|') || special {
                    return None;
                }
                finish(&mut word, &mut started, &mut special, stages.last_mut()?)?;
                stages.push(Vec::new());
            }
            ';' | '&' | '(' | ')' | '<' | '>' | '$' | '`' | '{' | '}' => {
                started = true;
                special = true;
                word.push(c);
            }
            c => {
                started = true;
                word.push(c);
            }
        }
    }
    finish(&mut word, &mut started, &mut special, stages.last_mut()?)?;
    stages
        .iter()
        .all(|stage| !stage.is_empty())
        .then_some(stages)
}

/// Redirects that only throw output away: they cannot write a file.
const SAFE_REDIRECTS: &[&str] = &[
    "2>&1",
    "2>/dev/null",
    ">/dev/null",
    "1>/dev/null",
    "&>/dev/null",
];

fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Read-only filters a read may pipe into (`cat f | head -20`). Each is
/// checked for its writing forms (`sort -o`, `uniq in out`, `rg --pre`).
fn is_filter(stage: &[String]) -> bool {
    let Some(first) = stage.first() else {
        return false;
    };
    let args = &stage[1..];
    match program_name(first) {
        "head" | "tail" | "wc" | "cut" | "nl" | "tr" | "column" | "grep" | "egrep" | "fgrep" => {
            true
        }
        "rg" => !args.iter().any(|a| a.starts_with("--pre")),
        "sort" => !args
            .iter()
            .any(|a| a.starts_with("-o") || a.starts_with("--output")),
        // `uniq [input [output]]`: only the flag-only form is a filter.
        "uniq" => args.iter().all(|a| a.starts_with('-')),
        _ => false,
    }
}

/// Positional words after the flags, skipping the values of `value_flags`
/// (`-n 50`). Attached forms (`-n50`, `--lines=50`) are one word already.
fn positionals(args: &[String], value_flags: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut flags_done = false;
    while let Some(a) = args.get(i) {
        if flags_done || a == "-" || !a.starts_with('-') {
            out.push(a.clone());
        } else if a == "--" {
            flags_done = true;
        } else if value_flags.contains(&a.as_str()) {
            i += 1;
        }
        i += 1;
    }
    out
}

fn files(
    look: Look,
    args: &[String],
    value_flags: &[&str],
    detail: Option<String>,
) -> Option<ReadOnly> {
    Some(ReadOnly {
        look,
        targets: positionals(args, value_flags),
        detail,
    })
}

const BAT_VALUE_FLAGS: &[&str] = &[
    "-l",
    "--language",
    "-r",
    "--line-range",
    "-H",
    "--highlight-line",
    "--style",
    "--theme",
    "-m",
    "--map-syntax",
    "--tabs",
    "--terminal-width",
    "--wrap",
    "--color",
    "--paging",
    "--decorations",
];

const NL_VALUE_FLAGS: &[&str] = &["-b", "-d", "-f", "-h", "-i", "-l", "-n", "-s", "-v", "-w"];

const LS_VALUE_FLAGS: &[&str] = &[
    "-I",
    "--ignore",
    "--hide",
    "-w",
    "-T",
    "-L",
    "-P",
    "--level",
    "--sort",
    "-s",
    "--ignore-glob",
    "-t",
    "--time",
    "--filelimit",
];

/// `head -n 50` → `first 50 lines`; `tail -20` → `last 20 lines`;
/// `tail -n +5` → `from line 5`; `tail -f` → `following`.
fn head_tail_detail(program: &str, args: &[String]) -> Option<String> {
    if program == "tail"
        && args
            .iter()
            .any(|a| a == "-f" || a == "-F" || a == "--follow")
    {
        return Some("following".into());
    }
    let mut count = None;
    for (i, a) in args.iter().enumerate() {
        let n = if a == "-n" || a == "--lines" {
            args.get(i + 1).map(String::as_str)
        } else if let Some(v) = a.strip_prefix("--lines=") {
            Some(v)
        } else if let Some(v) = a.strip_prefix("-n") {
            (!v.is_empty()).then_some(v)
        } else if let Some(v) = a.strip_prefix('-') {
            v.chars().all(|c| c.is_ascii_digit()).then_some(v)
        } else {
            None
        };
        if let Some(n) = n.filter(|n| !n.is_empty()) {
            count = Some(n.to_string());
        }
    }
    let n = count?;
    if let Some(from) = n.strip_prefix('+') {
        return Some(format!("from line {from}"));
    }
    if !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let lines = if n == "1" { "line" } else { "lines" };
    Some(if program == "head" {
        format!("first {n} {lines}")
    } else {
        format!("last {n} {lines}")
    })
}

/// `sed -n '<ranges>p' <files>`: only print-range scripts (digits, `,`,
/// `$`, `p`, `;`) count. `-i`, `-e`/`-f`, and any other script (`w file`,
/// `s///`) are not a read.
fn sed_read(args: &[String]) -> Option<ReadOnly> {
    let mut quiet = false;
    let mut rest = Vec::new();
    for a in args {
        match a.as_str() {
            "-n" | "--quiet" | "--silent" => quiet = true,
            "-E" | "-r" | "-s" | "-u" | "-z" | "--regexp-extended" => {}
            a if a.starts_with('-') => return None,
            _ => rest.push(a.clone()),
        }
    }
    let (script, files) = rest.split_first()?;
    let script = script.trim();
    if !quiet
        || files.is_empty()
        || !script.ends_with('p')
        || !script
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ',' | '$' | 'p' | ';' | ' '))
    {
        return None;
    }
    let detail = match script.trim_end_matches('p').split_once(',') {
        _ if script.contains(';') => None,
        Some((a, "$")) => Some(format!("from line {a}")),
        Some((a, b)) => Some(format!("lines {a}\u{2013}{b}")),
        None => Some(format!("line {}", script.trim_end_matches('p'))),
    };
    Some(ReadOnly {
        look: Look::Read,
        targets: files.to_vec(),
        detail,
    })
}

/// `grep`/`rg`/`ag`: the pattern (first positional, or `-e`'s value) is the
/// target; the remaining positionals become `in <paths>`.
fn search(program: &str, args: &[String]) -> Option<ReadOnly> {
    if args.iter().any(|a| a.starts_with("--pre")) {
        return None;
    }
    let value_flags: &[&str] = match program {
        "rg" => &[
            "-e",
            "--regexp",
            "-f",
            "--file",
            "-g",
            "--glob",
            "--iglob",
            "-t",
            "--type",
            "-T",
            "--type-not",
            "-m",
            "--max-count",
            "-A",
            "-B",
            "-C",
            "-j",
            "--threads",
            "-M",
            "--max-columns",
            "-d",
            "--max-depth",
            "--sort",
            "--sortr",
            "-E",
            "--encoding",
            "--color",
            "-r",
            "--replace",
        ],
        "ag" => &["-G", "-A", "-B", "-C", "-m", "--ignore", "--depth", "-g"],
        _ => &[
            "-e", "--regexp", "-f", "--file", "-m", "-A", "-B", "-C", "-d", "-D",
        ],
    };
    if args.iter().any(|a| a == "-f" || a == "--file") {
        return None;
    }
    let explicit = args
        .iter()
        .position(|a| a == "-e" || a == "--regexp")
        .and_then(|i| args.get(i + 1).cloned());
    let mut pos = positionals(args, value_flags);
    let pattern = match explicit {
        Some(p) => p,
        None if pos.is_empty() => return None,
        None => pos.remove(0),
    };
    Some(ReadOnly {
        look: Look::Searched,
        targets: vec![pattern],
        detail: (!pos.is_empty()).then(|| format!("in {}", pos.join(" "))),
    })
}

/// `find <paths> [-name pat …]`: a pattern makes it a search, none a
/// listing. Any action that writes or runs (`-exec`, `-delete`, `-fprint`)
/// is not a read.
fn find_read(args: &[String]) -> Option<ReadOnly> {
    const WRITES: &[&str] = &[
        "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprint0", "-fprintf", "-fls",
    ];
    if args.iter().any(|a| WRITES.contains(&a.as_str())) {
        return None;
    }
    let paths: Vec<String> = args
        .iter()
        .take_while(|a| !a.starts_with('-') && *a != "(" && *a != "!")
        .cloned()
        .collect();
    let pattern = args
        .iter()
        .position(|a| {
            matches!(
                a.as_str(),
                "-name" | "-iname" | "-path" | "-ipath" | "-regex"
            )
        })
        .and_then(|i| args.get(i + 1).cloned());
    let paths = if paths.is_empty() {
        vec![".".to_string()]
    } else {
        paths
    };
    Some(match pattern {
        Some(p) => ReadOnly {
            look: Look::Searched,
            targets: vec![p],
            detail: Some(format!("in {}", paths.join(" "))),
        },
        None => ReadOnly {
            look: Look::Listed,
            targets: paths,
            detail: None,
        },
    })
}

fn fd_read(args: &[String]) -> Option<ReadOnly> {
    if args.iter().any(|a| {
        matches!(a.as_str(), "-x" | "-X" | "--exec" | "--exec-batch") || a.starts_with("--exec")
    }) {
        return None;
    }
    let mut pos = positionals(
        args,
        &[
            "-e",
            "--extension",
            "-t",
            "--type",
            "-d",
            "--max-depth",
            "--min-depth",
            "-E",
            "--exclude",
            "-S",
            "--size",
            "--changed-within",
            "--changed-before",
            "-o",
            "--owner",
            "-c",
            "--color",
            "-j",
            "--threads",
            "--max-results",
            "--base-directory",
            "--path-separator",
            "--search-path",
        ],
    );
    if pos.is_empty() {
        return Some(ReadOnly {
            look: Look::Listed,
            targets: vec![".".into()],
            detail: None,
        });
    }
    let pattern = pos.remove(0);
    Some(ReadOnly {
        look: Look::Searched,
        targets: vec![pattern],
        detail: (!pos.is_empty()).then(|| format!("in {}", pos.join(" "))),
    })
}

#[path = "label_tests.rs"]
#[cfg(test)]
mod tests;
