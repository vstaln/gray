//! Kind-aware compression for command output.
//!
//! Truncation is the last resort and it always lies: it drops the middle of a
//! result and leaves a note about what is missing. Compression is what pays —
//! `cargo build` over a 300-crate tree is 30 KiB of `Compiling <crate> v0.1.2`
//! lines saying one thing, and the thing that matters (an `error[E0308]`)
//! is a handful of lines inside it.
//!
//! The rules are deliberately dumb: classify the command once, then one pass
//! that collapses runs of lines carrying no information. A rule may decline,
//! and the output is never longer than the input. The raw log the shell writes
//! to disk stays the recovery path either way, so squeezing what enters the
//! context costs the model nothing it cannot grep back out of that log.

/// A run of fewer than this many like lines is left alone: two `Compiling`
/// lines are a list, three are a count.
const MIN_RUN: usize = 3;

/// Below this size, compression cannot pay for the note it adds.
pub const MIN_SQUEEZE_BYTES: usize = 2048;

/// What a squeeze did, for the caller's note and the meter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Squeezed {
    /// The compressed text (identical to the input when `rule` is `none`).
    pub text: String,
    /// Rule id (`cargo`, `npm`, `pip`, `pytest`, `runs`) or `none`.
    pub rule: &'static str,
    /// Bytes handed in.
    pub raw_bytes: usize,
    /// Bytes handed back.
    pub sent_bytes: usize,
}

impl Squeezed {
    fn none(text: &str) -> Self {
        Self {
            text: text.to_string(),
            rule: "none",
            raw_bytes: text.len(),
            sent_bytes: text.len(),
        }
    }

    /// True when a rule fired and the text actually shrank.
    pub fn squeezed(&self) -> bool {
        self.rule != "none"
    }
}

/// Compress command output by what the command was.
///
/// Returns the input untouched when nothing applied: output too small to pay
/// for the note, a shape no rule knows, or a rule whose output would not be
/// smaller. Those are the normal cases, not failures.
pub fn squeeze(text: &str, command: &str) -> Squeezed {
    if std::env::var_os("GRAY_NO_SQUEEZE").is_some() {
        return Squeezed::none(text);
    }
    if text.len() < MIN_SQUEEZE_BYTES {
        return Squeezed::none(text);
    }
    let rule = classify(command);
    let trailing_newline = text.ends_with('\n');
    let body = collapse(text.lines().collect::<Vec<_>>(), rule);
    // A compressor may decline, and its output must never be bigger than its
    // input: fall back, then pass through.
    let sent_bytes = body.len();
    if sent_bytes >= text.len() {
        return Squeezed::none(text);
    }
    Squeezed {
        text: if trailing_newline && !body.ends_with('\n') {
            format!("{body}\n")
        } else {
            body
        },
        rule: rule.map_or("runs", Rule::id),
        raw_bytes: text.len(),
        sent_bytes,
    }
}

/// A command family with a rule for its output shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    Cargo,
    Npm,
    Pip,
    Pytest,
}

impl Rule {
    fn id(self) -> &'static str {
        match self {
            Rule::Cargo => "cargo",
            Rule::Npm => "npm",
            Rule::Pip => "pip",
            Rule::Pytest => "pytest",
        }
    }
}

/// The tool family one pipeline segment runs, from its first word.
fn segment_family(segment: &str) -> Option<Rule> {
    let mut words = segment.split_whitespace();
    let exe = loop {
        // `FOO=1 cargo build` — the assignment is not the command.
        let word = words.next()?;
        let key = word.split_once('=').map_or("", |(k, _)| k);
        if !key.is_empty() && !key.starts_with('-') && !key.contains('/') && word.contains('=') {
            continue;
        }
        break word;
    };
    // Basename only: `/usr/bin/cargo` and `cargo.exe` are the same tool.
    let exe = exe.rsplit('/').next().unwrap_or(exe);
    let exe = exe.strip_suffix(".exe").unwrap_or(exe);
    match exe {
        "cargo" | "rustc" => Some(Rule::Cargo),
        "npm" | "pnpm" | "yarn" | "bun" => Some(Rule::Npm),
        "pip" | "pip3" | "uv" => Some(Rule::Pip),
        "pytest" => Some(Rule::Pytest),
        _ => None,
    }
}

/// Require every segment of the command line to be the same family.
///
/// `cargo build | grep error` has two families, so only the generic
/// run-collapse applies: squeezing grep's output as if it were cargo's is
/// exactly the kind of clever wrong this file exists to avoid.
fn classify(command: &str) -> Option<Rule> {
    let mut family: Option<Rule> = None;
    let mut segments = 0usize;
    for segment in command.split(['|', ';', '\n', '&']) {
        // `2>&1` and friends leave a near-empty tail; redirections are not
        // commands, so drop them before looking for the program.
        let segment = segment
            .split_once('>')
            .map_or(segment, |(head, _)| head)
            .split_once('<')
            .map_or(segment, |(head, _)| head);
        let segment = match segment.trim() {
            "" => continue,
            // `>&2`, `2>&1`, `&>file`
            "2" | "1" => continue,
            other => other,
        };
        segments += 1;
        match segment_family(segment) {
            Some(f) if family.is_none() => family = Some(f),
            Some(f) if family == Some(f) => {}
            // Unknown or mixed: no family rule.
            _ => return None,
        }
    }
    (segments > 0).then_some(family).flatten()
}

/// What a line is, for the purpose of collapsing it. `None` means keep.
fn label(rule: Option<Rule>, line: &str) -> Option<&'static str> {
    let t = line.trim_start();
    let verb = |v: &str| t.starts_with(v) && t.as_bytes().get(v.len()) == Some(&b' ');
    match rule {
        Some(Rule::Cargo) => {
            if [
                "Compiling",
                "Building",
                "Checking",
                "Fresh",
                "Downloading",
                "Downloaded",
                "Updating",
                "Installing",
                "Running",
                "Doc-testing",
                "Blocking",
            ]
            .into_iter()
            .any(verb)
            {
                Some("progress")
            } else {
                None
            }
        }
        Some(Rule::Npm) => {
            if t.starts_with("+ ") || t.starts_with("├") || t.starts_with("└") || t.starts_with('│')
            {
                Some("dependency tree")
            } else if t.starts_with("WARN deprecated") {
                Some("deprecation warnings")
            } else {
                None
            }
        }
        Some(Rule::Pip) => {
            if [
                "Collecting ",
                "Downloading ",
                "Using cached ",
                "Requirement already satisfied:",
            ]
            .into_iter()
            .any(|p| t.starts_with(p))
            {
                Some("package chatter")
            } else {
                None
            }
        }
        Some(Rule::Pytest) => pytest_progress(line).then_some("progress"),
        None => None,
    }
}

/// A `pytest` progress line: dots and letters ending in `[ 42%]`. The dots are
/// the whole content, and they are only a summary of what the lines below say.
fn pytest_progress(line: &str) -> bool {
    let t = line.trim();
    let Some(open) = t.rfind('[') else {
        return false;
    };
    let (head, tail) = t.split_at(open);
    let Some(close) = tail.find(']') else {
        return false;
    };
    !head.is_empty()
        && tail[close + 1..].trim().is_empty()
        && tail[1..close].contains('%')
        && head
            .chars()
            .all(|c| c.is_ascii_digit() || ".sFExXP".contains(c) || c.is_whitespace())
}

/// One pass, every rule: collapse each run of alike lines into one counted
/// line. Three shapes, in priority order — the family rule (a run of `cargo`
/// progress), a run of identical lines (the generic rule), and everything
/// else kept verbatim.
fn collapse(input: Vec<&str>, rule: Option<Rule>) -> String {
    // `pip`'s one-line rewrite belongs to pip: applied to a `grep` result it
    // would be the exact cross-family guess this file refuses to make.
    let keep = |line: &str| {
        if rule == Some(Rule::Pip) {
            pip_summary(line)
        } else {
            line.to_string()
        }
    };
    let mut out: Vec<String> = Vec::with_capacity(input.len());
    let mut i = 0usize;
    while i < input.len() {
        // Blank runs say nothing; a run of them is worth one count.
        if input[i].trim().is_empty() {
            let start = i;
            while i < input.len() && input[i].trim().is_empty() {
                i += 1;
            }
            let run = i - start;
            if run < 4 {
                out.extend(input[start..i].iter().copied().map(String::from));
            } else {
                out.push(format!("[{run} blank lines]"));
            }
            continue;
        }
        if let Some(key) = label(rule, input[i]) {
            let start = i;
            while i < input.len() && label(rule, input[i]) == Some(key) {
                i += 1;
            }
            let run = i - start;
            if run < MIN_RUN {
                out.extend(input[start..i].iter().copied().map(keep));
            } else {
                out.push(run_line(key, &input[start..i], rule));
            }
            continue;
        }
        // The generic rule: the same line, again, and again.
        if i + 1 < input.len() && input[i] == input[i + 1] {
            let start = i;
            while i < input.len() && input[i] == input[start] {
                i += 1;
            }
            let run = i - start;
            if run < MIN_RUN {
                out.extend(input[start..i].iter().copied().map(keep));
            } else {
                out.push(format!("{} ×{}", input[start].trim_end(), run));
            }
            continue;
        }
        out.push(keep(input[i]));
        i += 1;
    }
    out.join("\n")
}

/// One counted line for a collapsed family run.
fn run_line(key: &str, run: &[&str], rule: Option<Rule>) -> String {
    // The last line of a collapsed run is the one that answers "what was it
    // working on when it stopped", so it is quoted rather than lost.
    format!(
        "[{}] {key} ×{} elided (last: {})",
        rule.map_or("runs", Rule::id),
        run.len(),
        run[run.len() - 1].trim()
    )
}

/// `pip` names every package twice — once as collected, once as installed —
/// and the installed list is one enormous line. Only ever called for a pip
/// command.
fn pip_summary(line: &str) -> String {
    let Some(rest) = line
        .trim_start()
        .strip_prefix("Installing collected packages:")
    else {
        return line.to_string();
    };
    let count = rest
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .count();
    if count < 3 {
        return line.to_string();
    }
    format!("[pip] installing {count} collected packages")
}
