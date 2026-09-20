//! Built-in content search: the fallback used when `rg` is not on PATH.
//!
//! Mirrors the `grep` tool's `--json` lane contract: matches render as
//! `file:line: text`, context lines as `file-line- text`, `.gitignore` is
//! honored through the `ignore` crate (same walker as `find`'s fd fallback),
//! and the caller's limit counts *matches*, not context lines. A machine
//! without ripgrep therefore keeps a working `grep` tool instead of an
//! "rg is not installed" dead end.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use globset::GlobSet;
use regex::Regex;

/// Cap on the size of a file we are willing to read for a line search. rg
/// streams; we read into memory, so a bound keeps a stray multi-GB log from
/// stalling the tool. Larger files are skipped, never silently truncated.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// One hit: `(file, 1-based line, line text, is_match)`. A context line has
/// `is_match == false`.
pub type Hit = (String, usize, Option<String>, bool);

/// Search result: the hits plus whether the match limit stopped the walk.
#[derive(Debug)]
pub struct Found {
    pub hits: Vec<Hit>,
    pub limit_reached: bool,
}

/// Is ripgrep on PATH? Probed once per process.
pub fn rg_present() -> bool {
    static PRESENT: OnceLock<bool> = OnceLock::new();
    *PRESENT.get_or_init(|| {
        std::process::Command::new("rg")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Compile the caller's pattern the way rg would: regex or literal, with
/// optional case folding.
fn compile(pattern: &str, literal: bool, ignore_case: bool) -> Result<Regex, String> {
    let body = if literal {
        regex::escape(pattern)
    } else {
        pattern.to_string()
    };
    let body = if ignore_case {
        format!("(?i){body}")
    } else {
        body
    };
    Regex::new(&body).map_err(|e| format!("invalid pattern: {e}"))
}

/// `--glob` semantics: a pattern without `/` matches the basename anywhere,
/// a path pattern matches the path relative to the search root.
fn glob_set(glob: &Option<String>) -> Result<Option<GlobSet>, String> {
    let Some(g) = glob else { return Ok(None) };
    let pat = if g.contains('/') {
        g.clone()
    } else {
        format!("**/{g}")
    };
    let built = globset::GlobBuilder::new(&pat)
        .literal_separator(true)
        .build()
        .map_err(|_| format!("invalid glob pattern: {g}"))?;
    let mut b = globset::GlobSetBuilder::new();
    b.add(built);
    Ok(Some(
        b.build()
            .map_err(|e| format!("invalid glob pattern: {g}: {e}"))?,
    ))
}

/// Search `path` (a file or a directory) for `pattern`.
#[allow(clippy::too_many_arguments)]
pub fn search(
    pattern: &str,
    path: &Path,
    is_dir: bool,
    glob: &Option<String>,
    ignore_case: bool,
    literal: bool,
    limit: usize,
    context: usize,
) -> Result<Found, String> {
    let re = compile(pattern, literal, ignore_case)?;
    let globs = glob_set(glob)?;
    let files: Vec<PathBuf> = if is_dir {
        walk(path, globs.as_ref())
    } else {
        vec![path.to_path_buf()]
    };

    let mut hits: Vec<Hit> = Vec::new();
    let mut matches = 0usize;
    let mut limit_reached = false;

    'files: for file in files {
        if let Ok(md) = std::fs::metadata(&file)
            && md.len() > MAX_FILE_BYTES
        {
            continue;
        }
        // read_to_string also rejects binary payloads, which rg skips too.
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let mut emitted: Option<usize> = None; // last line index already emitted
        for (i, line) in lines.iter().enumerate() {
            if !re.is_match(line) {
                continue;
            }
            matches += 1;
            let start = i.saturating_sub(context);
            let end = (i + context + 1).min(lines.len());
            for (j, ctx_line) in lines.iter().enumerate().take(end).skip(start) {
                if emitted.is_some_and(|e| j <= e) {
                    continue;
                }
                hits.push((
                    file.to_string_lossy().to_string(),
                    j + 1,
                    Some((*ctx_line).to_string()),
                    j == i,
                ));
                emitted = Some(j);
            }
            if matches >= limit {
                limit_reached = true;
                break 'files;
            }
        }
    }

    Ok(Found {
        hits,
        limit_reached,
    })
}

/// Walk `root` the way `rg --hidden` does, minus VCS/dependency trees.
fn walk(root: &Path, globs: Option<&GlobSet>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false) // rg --hidden parity: include dotfiles
        .require_git(false) // honor .gitignore outside repos
        .filter_entry(|e| {
            !(e.file_type().is_some_and(|t| t.is_dir())
                && (e.file_name() == ".git" || e.file_name() == "node_modules"))
        })
        .build();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if let Some(gs) = globs
            && !gs.is_match(&rel_str)
        {
            continue;
        }
        out.push(entry.path().to_path_buf());
    }
    out.sort();
    out
}

#[path = "grep_builtin_tests.rs"]
#[cfg(test)]
mod tests;
