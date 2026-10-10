//! Prompt templates: markdown files that become slash commands.
//!
//! `~/.gray/prompts/review.md` is `/review`. Typing `/review src/lib.rs`
//! sends the file's body with the arguments filled in. The format and the
//! placeholders match pi's prompt templates, so pi and Claude Code command
//! files work unchanged:
//!
//! - `$1`, `$2`, … positional arguments (quotes group words)
//! - `$@` / `$ARGUMENTS` all arguments
//! - `${1:-default}`, `${@:-default}` with a fallback when empty
//! - `${@:2}`, `${@:2:3}` bash-style slices
//!
//! A body with no placeholder gets the arguments appended on a new line, so
//! a plain template still receives what was typed after it.
//!
//! Frontmatter is optional: `description:` (else the first body line) and
//! `argument-hint:` show in the completion popup.
//!
//! Search order, first name wins: the project (cwd up to the git root,
//! nearest first; `.gray/prompts`, `.pi/prompts`, `.claude/commands`), then
//! the user (`~/.gray/prompts`, `~/.pi/agent/prompts`, `~/.claude/commands`).
//! Built-in slash commands always win over a template of the same name.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub content: String,
    pub path: PathBuf,
    /// "project" | "user"
    pub source: &'static str,
}

/// Split an argument string bash-style: whitespace separates, single or
/// double quotes group (no escapes), matching pi's `parseCommandArgs`.
#[must_use]
pub fn parse_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for ch in s.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => cur.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                }
                started = false;
            }
            None => {
                cur.push(ch);
                started = true;
            }
        }
    }
    if started || !cur.is_empty() {
        args.push(cur);
    }
    // pi drops empty tokens (`""`); keep parity.
    args.retain(|a| !a.is_empty());
    args
}

fn placeholder_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"\$\{(\d+|ARGUMENTS|@):-([^}]*)\}|\$\{@:(\d+)(?::(\d+))?\}|\$(ARGUMENTS|@|\d+)",
        )
        .expect("placeholder regex compiles")
    })
}

/// True when `content` uses any argument placeholder.
#[must_use]
pub fn has_placeholders(content: &str) -> bool {
    placeholder_re().is_match(content)
}

/// Fill placeholders in one pass: values are never re-scanned, so an
/// argument containing `$1` stays literal.
#[must_use]
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let all = args.join(" ");
    let nth = |n: &str| -> Option<&String> {
        n.parse::<usize>()
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| args.get(i))
    };
    placeholder_re()
        .replace_all(content, |c: &regex::Captures<'_>| {
            if let Some(target) = c.get(1) {
                let default = c.get(2).map_or("", |m| m.as_str());
                let value = match target.as_str() {
                    "@" | "ARGUMENTS" => all.clone(),
                    n => nth(n).cloned().unwrap_or_default(),
                };
                return if value.is_empty() {
                    default.to_string()
                } else {
                    value
                };
            }
            if let Some(start) = c.get(3) {
                let start = start
                    .as_str()
                    .parse::<usize>()
                    .unwrap_or(1)
                    .saturating_sub(1)
                    .min(args.len());
                let end = match c.get(4).and_then(|l| l.as_str().parse::<usize>().ok()) {
                    Some(len) => start.saturating_add(len).min(args.len()),
                    None => args.len(),
                };
                return args[start..end].join(" ");
            }
            match c.get(5).map_or("", |m| m.as_str()) {
                "@" | "ARGUMENTS" => all.clone(),
                n => nth(n).cloned().unwrap_or_default(),
            }
        })
        .into_owned()
}

/// The prompt a `/name <rest>` invocation sends.
#[must_use]
pub fn expand(template: &PromptTemplate, rest: &str) -> String {
    let args = parse_args(rest);
    let body = template.content.trim();
    if has_placeholders(body) {
        substitute_args(body, &args)
    } else if rest.trim().is_empty() {
        body.to_string()
    } else {
        format!("{body}\n\n{}", rest.trim())
    }
}

/// A template name is a plain file stem a slash command can carry.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_whitespace)
}

/// Load one template file; `None` when unreadable or badly framed.
pub fn load_template(path: &Path, source: &'static str) -> Option<PromptTemplate> {
    let name = path.file_stem()?.to_str()?.to_string();
    if !valid_name(&name) {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    let (fm, body) = crate::skills::parse_frontmatter(&raw).ok()?;
    let description = fm
        .description
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| {
            let first = body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let first = first.trim().trim_start_matches('#').trim();
            let mut out: String = first.chars().take(60).collect();
            if first.chars().count() > 60 {
                out.push_str("...");
            }
            out
        });
    Some(PromptTemplate {
        name,
        description,
        argument_hint: fm.argument_hint.filter(|h| !h.trim().is_empty()),
        content: body,
        path: path.to_path_buf(),
        source,
    })
}

/// `*.md` files directly in `dir`, sorted by name (symlinks followed).
fn load_dir(dir: &Path, source: &'static str, out: &mut Vec<PromptTemplate>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md") && p.is_file())
        .collect();
    paths.sort();
    for p in paths {
        if let Some(t) = load_template(&p, source)
            && !out.iter().any(|o| o.name == t.name)
        {
            out.push(t);
        }
    }
}

/// Directories searched, highest precedence first.
#[must_use]
pub fn search_dirs(
    cwd: &Path,
    gray_home: Option<&Path>,
    home: Option<&Path>,
) -> Vec<(PathBuf, &'static str)> {
    let mut dirs = Vec::new();
    let git_root = crate::skills::find_git_root(cwd);
    let mut cur = Some(cwd.to_path_buf());
    while let Some(dir) = cur {
        for sub in [
            [".gray", "prompts"],
            [".pi", "prompts"],
            [".claude", "commands"],
        ] {
            dirs.push((dir.join(sub[0]).join(sub[1]), "project"));
        }
        // Without a repo, only the cwd itself counts as the project.
        if git_root.as_ref().is_none_or(|r| *r == dir) {
            break;
        }
        cur = dir.parent().map(Path::to_path_buf);
    }
    if let Some(gh) = gray_home {
        dirs.push((gh.join("prompts"), "user"));
    }
    if let Some(h) = home {
        dirs.push((h.join(".pi").join("agent").join("prompts"), "user"));
        dirs.push((h.join(".claude").join("commands"), "user"));
    }
    // A project dir that is also a user dir (cwd = $HOME) loads once.
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|(d, _)| seen.insert(d.clone()));
    dirs
}

/// Every template visible from `dirs`, first name wins.
#[must_use]
pub fn discover_in(dirs: &[(PathBuf, &'static str)]) -> Vec<PromptTemplate> {
    let mut out = Vec::new();
    for (dir, source) in dirs {
        load_dir(dir, source, &mut out);
    }
    out
}

type Cache = Option<(PathBuf, Instant, Vec<PromptTemplate>)>;
static CACHE: Mutex<Cache> = Mutex::new(None);

/// Every template visible from `cwd`, excluding names a built-in command
/// owns. Cached for a moment: the completion popup asks on every keystroke.
#[must_use]
pub fn discover(cwd: &Path) -> Vec<PromptTemplate> {
    const TTL: Duration = Duration::from_secs(2);
    if let Ok(guard) = CACHE.lock()
        && let Some((dir, at, list)) = guard.as_ref()
        && dir == cwd
        && at.elapsed() < TTL
    {
        return list.clone();
    }
    let gray_home = crate::setup::gray_home().ok();
    let home = gray_core::paths::user_home();
    let mut dirs = search_dirs(cwd, gray_home.as_deref(), home.as_deref());
    // Project templates are repo content: skip them until the project is trusted.
    if !crate::project_trust::is_trusted(&crate::skills::gray_agent_dir(), cwd) {
        dirs.retain(|(_, source)| *source != "project");
    }
    let list: Vec<PromptTemplate> = discover_in(&dirs)
        .into_iter()
        .filter(|t| !crate::repl::is_builtin_command(&t.name))
        .collect();
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((cwd.to_path_buf(), Instant::now(), list.clone()));
    }
    list
}

/// Forgets the discovery cache so the next lookup re-reads the disk.
pub fn invalidate_cache() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = None;
    }
}

/// Whether the last discovery (any cwd) saw a template called `name`. The
/// completion popup's fill step has no cwd; the rows it fills came from
/// that same discovery moments earlier.
#[must_use]
pub fn is_cached_name(name: &str) -> bool {
    CACHE
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref()
                .map(|(_, _, list)| list.iter().any(|t| t.name == name))
        })
        .unwrap_or(false)
}

/// Find the template a `/name` invocation means (case-insensitive).
#[must_use]
pub fn find(cwd: &Path, name: &str) -> Option<PromptTemplate> {
    let name = name.strip_prefix('/').unwrap_or(name);
    discover(cwd)
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
}

#[path = "prompt_templates_tests.rs"]
#[cfg(test)]
mod tests;
