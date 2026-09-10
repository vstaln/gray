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
//! Contract strings live in `notices.rs` (moved verbatim at the wave gate);
//! [`aggregate_note`]/[`MISSING_INPUT_MESSAGE`] below delegate there (one
//! owner per string — same staging as `resolve.rs`).
//!
//! FOLLOW-UPS (not done here — files outside T4.2 ownership):
//! 1. Done (T6.1): `read/mod.rs` wiring above.
//! 2. Done (wave gate): `notices.rs` owns [`aggregate_note`]/
//!    [`MISSING_INPUT_MESSAGE`] verbatim.

use std::path::Path;

/// Max files expanded from `paths[]` (spec-fixed).
pub const MAX_MATCHES: usize = 200;

/// Aggregate budget over rendered per-file bytes (spec-fixed ~100 KiB).
pub const AGGREGATE_BYTES: u64 = 100 * 1024;

/// Dirs excluded unless an input pattern names them (spec-fixed list).
pub const DEFAULT_DIR_EXCLUDES: &[&str] = &["node_modules", "target", ".git", "dist"];

/// Enforced when neither `path` nor `paths` is given — delegates to `notices.rs`.
pub const MISSING_INPUT_MESSAGE: &str = super::notices::MISSING_INPUT_MESSAGE;

/// `==> <relative path> <==` — per-file header above the windowed output.
pub fn header(rel: &str) -> String {
    format!("==> {rel} <==")
}

/// True for glob inputs (`*`/`?`, incl. `**`). Anything else is a literal.
pub fn is_glob(s: &str) -> bool {
    s.chars().any(|c| c == '*' || c == '?')
}

/// Trailing summary once the budget stops the list — delegates to `notices.rs`.
pub fn aggregate_note(shown: usize, total: usize, skipped: &[String]) -> String {
    super::notices::aggregate_note(shown, total, skipped)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tagged(n: usize, size: u64) -> Vec<(String, u64)> {
        (0..n).map(|i| (format!("f{i:03}.rs"), size)).collect()
    }

    #[test]
    fn header_is_contract_exact() {
        assert_eq!(header("src/a.rs"), "==> src/a.rs <==");
    }

    #[test]
    fn missing_input_message_is_contract_exact() {
        assert_eq!(
            MISSING_INPUT_MESSAGE,
            "read: provide path (one file) or paths (list of files/globs)"
        );
    }

    #[test]
    fn glob_detection_is_star_question_only() {
        assert!(is_glob("src/**/*.rs"));
        assert!(is_glob("*.md"));
        assert!(is_glob("a?.txt"));
        assert!(!is_glob("README.md"));
        assert!(!is_glob("src/a.rs"));
        // `[...]`/`{...}` are literals here (matcher is `*`/`?`/`**` only).
        assert!(!is_glob("[abc].txt"));
        assert!(!is_glob("{a,b}.txt"));
    }

    #[test]
    fn default_excludes_with_literal_and_dir_bypass() {
        for rel in [
            "node_modules/x.js",
            "target/a.rmeta",
            ".git/config",
            "dist/b.js",
            "Cargo.lock",
            "src/Cargo.lock",
        ] {
            assert!(is_excluded(rel, &[]), "{rel}");
        }
        assert!(!is_excluded("src/a.rs", &[]));
        // Exact literal always wins (dirs and locks alike).
        assert!(!is_excluded(
            "node_modules/x.js",
            &["node_modules/x.js".to_string()]
        ));
        assert!(!is_excluded("Cargo.lock", &["Cargo.lock".to_string()]));
        // Naming the dir in a glob keeps its files (spec test).
        assert!(!is_excluded(
            "node_modules/x.js",
            &["node_modules/**/*.js".to_string()]
        ));
        assert!(is_excluded(
            "node_modules/x.js",
            &["src/**/*.js".to_string()]
        ));
        // …but a lock still needs its own literal.
        assert!(is_excluded(
            "node_modules/f.lock",
            &["node_modules/**/*.js".to_string()]
        ));
        assert!(!is_excluded(
            "node_modules/f.lock",
            &["node_modules/f.lock".to_string()]
        ));
    }

    fn matches(pattern: &str, rel: &str) -> bool {
        Globs::compile([pattern]).is_match(rel)
    }

    #[test]
    fn pattern_matching_basics() {
        assert!(matches("*.rs", "src/a.rs")); // basename rule
        assert!(matches("src/**/*.rs", "src/a.rs")); // ** eats zero
        assert!(matches("src/**/*.rs", "src/sub/a.rs"));
        assert!(!matches("src/*.rs", "src/sub/a.rs")); // * no cross-/
        assert!(!matches("src/**/*.rs", "other/a.rs")); // anchored
        assert!(matches("target/", "target/a.rmeta")); // dir prefix
    }

    #[test]
    fn mixed_literal_and_glob_read_all_sorted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("README.md"), "r").unwrap();
        std::fs::write(dir.path().join("src/b.rs"), "b").unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "a").unwrap();
        let got = expand(
            dir.path(),
            &["README.md".to_string(), "src/**/*.rs".to_string()],
            &[],
        );
        assert_eq!(got, vec!["README.md", "src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn node_modules_excluded_unless_named() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        std::fs::write(dir.path().join("src/a.js"), "a").unwrap();
        std::fs::write(dir.path().join("node_modules/x.js"), "x").unwrap();
        let got = expand(dir.path(), &["**/*.js".to_string()], &[]);
        assert_eq!(got, vec!["src/a.js".to_string()]);
        let got = expand(dir.path(), &["node_modules/**/*.js".to_string()], &[]);
        assert_eq!(got, vec!["node_modules/x.js".to_string()]);
    }

    #[test]
    fn gitignore_filters_globs_not_literals() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("ignored")).unwrap();
        std::fs::create_dir_all(dir.path().join("keep")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(dir.path().join("ignored/a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("keep/b.txt"), "b").unwrap();
        let got = expand(dir.path(), &["**/*.txt".to_string()], &[]);
        assert_eq!(got, vec!["keep/b.txt".to_string()]);
        let got = expand(dir.path(), &["ignored/a.txt".to_string()], &[]);
        assert_eq!(got, vec!["ignored/a.txt".to_string()]);
    }

    #[test]
    fn extra_excludes_filter_globs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("keep")).unwrap();
        std::fs::write(dir.path().join("keep/b.txt"), "b").unwrap();
        std::fs::write(dir.path().join("keep/c.txt"), "c").unwrap();
        let got = expand(
            dir.path(),
            &["**/*.txt".to_string()],
            &["**/c.txt".to_string()],
        );
        assert_eq!(got, vec!["keep/b.txt".to_string()]);
    }

    #[test]
    fn aggregate_cap_stops_at_100kib_with_skipped_list() {
        let files = tagged(300, 1024);
        let (shown, skipped) = fit_within_cap(&files);
        assert_eq!(shown.len(), 100);
        assert_eq!(skipped.len(), 200);
        assert_eq!(skipped[0], "f100.rs");
        let note = aggregate_note(shown.len(), files.len(), &skipped);
        assert!(
            note.starts_with("[read: showed 100 of 300 files; 200 skipped (over 100 KiB total): "),
            "{note}"
        );
        assert!(note.contains("f100.rs"), "{note}");
        assert!(note.contains("…"), "{note}");
        assert!(
            note.ends_with("Read them individually or narrow the glob.]"),
            "{note}"
        );
    }

    #[test]
    fn first_file_over_cap_is_still_shown() {
        let files = vec![
            ("big.bin".to_string(), AGGREGATE_BYTES + 1),
            ("s.txt".to_string(), 1),
        ];
        let (shown, skipped) = fit_within_cap(&files);
        assert_eq!(shown, vec!["big.bin".to_string()]);
        assert_eq!(skipped, vec!["s.txt".to_string()]);
    }

    #[test]
    fn expand_truncates_to_200_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..210 {
            std::fs::write(dir.path().join(format!("f{i:03}.txt")), "x").unwrap();
        }
        let got = expand(dir.path(), &["*.txt".to_string()], &[]);
        assert_eq!(got.len(), MAX_MATCHES);
        let mut sorted = got.clone();
        sorted.sort();
        assert_eq!(got, sorted);
        assert_eq!(got[0], "f000.txt");
    }
}
