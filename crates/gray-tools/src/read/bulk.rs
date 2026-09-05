//! T4.2 (worktree) — bulk `paths[]` unit: multi-file expansion + caps.
//!
//! Implements the pure parts of plan T6.1 ("Many files in one call"). The
//! worktree calls this task T4.2; plan.ts T4.2 (did-you-mean) is a different
//! task living in `resolve.rs` — NOT touched here.
//!
//! Std only (+ `tempfile` in tests, already a gray-tools dep): no new deps.
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
//! 3. Walk performance: workspace already has `ignore`; switching the walk to
//!    it is a follow-up (needs manifest + `mod.rs` — outside here).
//!
//! // ponytail: hand-rolled `*`/`?`/`**` matcher instead of `ignore`/`globset` —
//! // zero new deps, covers the spec's globs. `[...]`/`{a,b}` are literals.
//! // ponytail: walk collects then filters (no dir pruning) so literally-named
//! // excludes still resolve; symlinks never followed (no cycles).
//! // ponytail: gitignore `!` negation unsupported — such lines are skipped.

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

/// Classic `*`/`?` matcher over one path segment (never crosses `/`).
fn matches_segment(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut back = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            back = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            back += 1;
            ti = back;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Segment matcher where `**` eats zero or more whole segments.
fn matches_segs(pat: &[&str], path: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        return matches_segs(&pat[1..], path)
            || (!path.is_empty() && matches_segs(pat, &path[1..]));
    }
    if path.is_empty() || !matches_segment(pat[0], path[0]) {
        return false;
    }
    matches_segs(&pat[1..], &path[1..])
}

/// Glob match for one pattern against a cwd-relative `/`-joined path.
/// Slash-less patterns match the basename only (fd behavior, same as
/// `find.rs`); otherwise the match is anchored. A trailing `/` means "the
/// dir and everything under it".
pub fn matches_pattern(pattern: &str, rel: &str) -> bool {
    let pat = pattern.strip_prefix("./").unwrap_or(pattern);
    if let Some(dir) = pat.strip_suffix('/') {
        return rel == dir || rel.starts_with(&format!("{dir}/"));
    }
    if !pat.contains('/') {
        let base = rel.rsplit('/').next().unwrap_or(rel);
        return matches_segment(pat, base);
    }
    matches_segs(
        &pat.split('/').collect::<Vec<_>>(),
        &rel.split('/').collect::<Vec<_>>(),
    )
}

/// `.gitignore` patterns at `cwd` (no negation, no I/O failure — missing file
/// means no patterns). Same simple subset as `find.rs`.
fn read_gitignore(cwd: &Path) -> Vec<String> {
    let content = std::fs::read_to_string(cwd.join(".gitignore")).unwrap_or_default();
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .map(str::to_string)
        .collect()
}

/// True when `rel` is gitignored. Wildcard patterns match via
/// [`matches_pattern`] (plus a basename retry for bare `*.log` shapes);
/// plain patterns match the path or any path under them.
fn is_gitignored(rel: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| {
        let p = pat.trim_end_matches('/');
        if p.contains('*') || p.contains('?') {
            if matches_pattern(p, rel) {
                return true;
            }
            if !p.contains('/')
                && let Some(base) = rel.rsplit('/').next()
                && matches_segment(p, base)
            {
                return true;
            }
            false
        } else {
            rel == p || rel.starts_with(&format!("{p}/"))
        }
    })
}

/// Every file under `cwd` as `/`-joined relative paths. No pruning except
/// symlinks (filtering happens in [`expand`] so literals still resolve).
fn walk_files(cwd: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![cwd.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let rel = match entry.path().strip_prefix(cwd) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            if rel.is_empty() {
                continue;
            }
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            } else if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                out.push(rel);
            }
        }
    }
    out
}

/// Expand `paths[]` to a sorted, capped file list (cwd-relative `/` paths).
///
/// Literals (non-globs) resolve first when they exist as files — dirs and
/// missing names are skipped here (the per-file render reports them); globs
/// walk `cwd`, honoring `.gitignore`, [`is_excluded`], and `extra_excludes`
/// (user `exclude[]`, always wins). Result is sorted, deduped, capped at
/// [`MAX_MATCHES`].
pub fn expand(cwd: &Path, inputs: &[String], extra_excludes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let globs: Vec<&String> = inputs.iter().filter(|s| is_glob(s)).collect();
    for lit in inputs.iter().filter(|s| !is_glob(s)) {
        let rel = lit.strip_prefix("./").unwrap_or(lit).replace('\\', "/");
        if out.contains(&rel) {
            continue;
        }
        if extra_excludes.iter().any(|e| matches_pattern(e, &rel)) {
            continue;
        }
        if std::fs::metadata(cwd.join(&rel)).is_ok_and(|m| m.is_file()) {
            out.push(rel);
        }
    }
    if !globs.is_empty() {
        let ignored = read_gitignore(cwd);
        for rel in walk_files(cwd) {
            if out.contains(&rel) {
                continue;
            }
            if extra_excludes.iter().any(|e| matches_pattern(e, &rel)) {
                continue;
            }
            if is_gitignored(&rel, &ignored) {
                continue;
            }
            if is_excluded(&rel, inputs) {
                continue;
            }
            if globs.iter().any(|g| matches_pattern(g, &rel)) {
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

    #[test]
    fn pattern_matching_basics() {
        assert!(matches_pattern("*.rs", "src/a.rs")); // basename rule
        assert!(matches_pattern("src/**/*.rs", "src/a.rs")); // ** eats zero
        assert!(matches_pattern("src/**/*.rs", "src/sub/a.rs"));
        assert!(!matches_pattern("src/*.rs", "src/sub/a.rs")); // * no cross-/
        assert!(!matches_pattern("src/**/*.rs", "other/a.rs")); // anchored
        assert!(matches_pattern("target/", "target/a.rmeta")); // dir prefix
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
