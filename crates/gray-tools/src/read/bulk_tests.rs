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
        crate::read::notices::MISSING_INPUT_MESSAGE,
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
    let note = crate::read::notices::aggregate_note(shown.len(), files.len(), &skipped);
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
