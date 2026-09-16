//! T4.2 (worktree) — bulk `paths[]` unit: multi-file expansion + caps.
//!
//! Implements the pure parts of plan T6.1 ("Many files in one call"). The
//! worktree calls this task T4.2; plan.ts T4.2 (did-you-mean) is a different
//! task living in `resolve.rs` — NOT touched here.
//!
//! Uses `globset` for matching and `ignore` for the gitignore-aware walk
//! (both workspace deps, same pair `find.rs::fallback_walk` uses).
//! WIRED (wave gate): `read/mod.rs` has `mod bulk;`, the `paths`/`exclude`
//! schema, the per-file render loop (recursive `execute`, per-file
//! ledger/dedup), `fit_within_cap` on rendered bytes + trailing
//! `aggregate_note`, and `MISSING_INPUT_MESSAGE` when neither `path` nor
//! `paths` is given.
//!
//! ```ignore
//! mod bulk;
//! // in ReadTool::execute, after arg parsing, when `paths`/`exclude` present:
//! // let rels = bulk::expand(&ctx.cwd, &paths, &excludes);
//! // for rel in &rels { /* same windowed render as `path`, with
//! //   bulk::header(rel) on top, per-file ledger/dedup (T3.1/T3.3 owners) */ }
//! // fit rendered bytes with bulk::fit_within_cap, trailing note via
//! // notices::aggregate_note; neither path nor paths -> notices::MISSING_INPUT_MESSAGE.
//! ```
//!
//! Spec: plan.ts T6.1. `limit` applies per file; headers are sorted; the
//! aggregate budget (~100 KiB) applies to rendered bytes.
//!
//! FOLLOW-UPS (not done here — files outside T4.2 ownership):
//! 1. Done (T6.1): `read/mod.rs` wiring above.
//! 2. Done (wave gate): `notices.rs` owns [`super::notices::aggregate_note`]/
//!    [`super::notices::MISSING_INPUT_MESSAGE`] verbatim.

use std::path::Path;

/// Max files expanded from `paths[]` (spec-fixed).
pub const MAX_MATCHES: usize = 200;

/// Aggregate budget over rendered per-file bytes (spec-fixed ~100 KiB).
pub const AGGREGATE_BYTES: u64 = 100 * 1024;

/// Dirs excluded unless an input pattern names them (spec-fixed list).
pub const DEFAULT_DIR_EXCLUDES: &[&str] = &["node_modules", "target", ".git", "dist"];

/// `==> <relative path> <==` — per-file header above the windowed output.
pub fn header(rel: &str) -> String {
    format!("==> {rel} <==")
}

/// True for glob inputs (`*`/`?`, incl. `**`). Anything else is a literal.
pub fn is_glob(s: &str) -> bool {
    s.chars().any(|c| c == '*' || c == '?')
}

/// True when a pattern names a dir as a path segment (`node_modules/**/*.js`
/// names `node_modules`; `**/*.js` does not).
fn mentions_dir(pattern: &str, dir: &str) -> bool {
    pattern.split('/').any(|seg| seg == dir)
}

/// Exclusion with the literal bypass: an exact literal input always wins;
/// a default-excluded dir is kept when some input pattern names it; a
/// `*.lock` file needs the exact literal (naming only its dir is not enough).
/// `rel` is cwd-relative with `/` separators (what [`expand`] produces).
pub fn is_excluded(rel: &str, inputs: &[String]) -> bool {
    let norm = rel.strip_prefix("./").unwrap_or(rel);
    if inputs
        .iter()
        .any(|p| p.strip_prefix("./").unwrap_or(p) == norm)
    {
        return false;
    }
    if let Some(hit) = DEFAULT_DIR_EXCLUDES
        .iter()
        .find(|d| norm.split('/').any(|seg| seg == **d))
        && !inputs.iter().any(|p| mentions_dir(p, hit))
    {
        return true;
    }
    norm.rsplit('/')
        .next()
        .is_some_and(|base| base.ends_with(".lock"))
}

/// Compiled glob patterns (native `globset`, fd `--glob` semantics):
/// slash-less patterns match the basename only; all others the full
/// cwd-relative path. A trailing `/` means "the dir and everything under it".
struct Globs {
    full: globset::GlobSet,
    base: globset::GlobSet,
}

impl Globs {
    fn compile<'a>(patterns: impl IntoIterator<Item = &'a str>) -> Self {
        let mut full = globset::GlobSetBuilder::new();
        let mut base = globset::GlobSetBuilder::new();
        for p in patterns {
            let pat = p.strip_prefix("./").unwrap_or(p);
            let (builder, pattern) = if let Some(dir) = pat.strip_suffix('/') {
                (&mut full, format!("{dir}/**"))
            } else if pat.contains('/') {
                (&mut full, pat.to_string())
            } else {
                (&mut base, pat.to_string())
            };
            // Unbuildable patterns are skipped: nothing sensible to match.
            if let Ok(glob) = globset::GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
            {
                builder.add(glob);
            }
        }
        let build =
            |b: globset::GlobSetBuilder| b.build().unwrap_or_else(|_| globset::GlobSet::empty());
        Self {
            full: build(full),
            base: build(base),
        }
    }

    fn is_match(&self, rel: &str) -> bool {
        if self.full.is_match(rel) {
            return true;
        }
        let base = rel.rsplit('/').next().unwrap_or(rel);
        self.base.is_match_candidate(&globset::Candidate::new(base))
    }
}

/// Expand `paths[]` to a sorted, capped file list (cwd-relative `/` paths).
///
/// Literals (non-globs) resolve first when they exist as files — dirs and
/// missing names are skipped here (the per-file render reports them); globs
/// walk `cwd`, honoring `.gitignore` (via the `ignore` crate, same as
/// `find.rs`), [`is_excluded`], and `extra_excludes` (user `exclude[]`,
/// always wins). Result is sorted, deduped, capped at [`MAX_MATCHES`].
pub fn expand(cwd: &Path, inputs: &[String], extra_excludes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let excludes = Globs::compile(extra_excludes.iter().map(String::as_str));
    let globs: Vec<&str> = inputs
        .iter()
        .filter(|s| is_glob(s))
        .map(String::as_str)
        .collect();
    for lit in inputs.iter().filter(|s| !is_glob(s)) {
        let rel = lit.strip_prefix("./").unwrap_or(lit).replace('\\', "/");
        if out.contains(&rel) {
            continue;
        }
        if excludes.is_match(&rel) {
            continue;
        }
        if std::fs::metadata(cwd.join(&rel)).is_ok_and(|m| m.is_file()) {
            out.push(rel);
        }
    }
    if !globs.is_empty() {
        let matcher = Globs::compile(globs.iter().copied());
        let walker = ignore::WalkBuilder::new(cwd)
            .hidden(false) // include dotfiles, as the old walk did
            .require_git(false) // honor .gitignore outside repos, as before
            .build();
        for entry in walker.flatten() {
            let Ok(rel) = entry.path().strip_prefix(cwd) else {
                continue;
            };
            if rel.as_os_str().is_empty() {
                continue; // the root itself
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue; // dirs and symlinks: files only, as before
            }
            let rel = rel.to_string_lossy().replace('\\', "/");
            if out.contains(&rel) {
                continue;
            }
            if excludes.is_match(&rel) {
                continue;
            }
            if is_excluded(&rel, inputs) {
                continue;
            }
            if matcher.is_match(&rel) {
                out.push(rel);
            }
        }
    }
    out.sort();
    out.truncate(MAX_MATCHES);
    out
}

/// Split an ordered `(path, rendered_bytes)` list at [`AGGREGATE_BYTES`]:
/// files that fit are shown, the rest are skipped. The first file is always
/// shown even when it alone exceeds the budget (showing one big file beats
/// showing none). Callers size with rendered (windowed + numbered) bytes.
pub fn fit_within_cap(files: &[(String, u64)]) -> (Vec<String>, Vec<String>) {
    let mut shown = Vec::new();
    let mut total: u64 = 0;
    for (i, (name, size)) in files.iter().enumerate() {
        if i > 0 && total.saturating_add(*size) > AGGREGATE_BYTES {
            return (shown, files[i..].iter().map(|(n, _)| n.clone()).collect());
        }
        total = total.saturating_add(*size);
        shown.push(name.clone());
    }
    (shown, Vec::new())
}

#[path = "bulk_tests.rs"]
#[cfg(test)]
mod tests;
