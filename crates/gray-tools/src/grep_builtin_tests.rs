use super::*;
use std::fs;

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

#[test]
fn finds_matches_in_a_nested_tree() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "src/a.ts", "let x = 1;\nconst target = 2;\n");
    write(
        d.path(),
        "src/deep/b.ts",
        "no hit here\nanother target line\n",
    );
    let found = search("target", d.path(), true, &None, false, false, 100, 0).unwrap();
    assert!(!found.limit_reached);
    let rendered: Vec<String> = found
        .hits
        .iter()
        .map(|(f, n, t, is)| {
            let name = f.rsplit('/').next().unwrap();
            let mark = if *is { "" } else { " (ctx)" };
            format!("{name}:{n}: {}{mark}", t.clone().unwrap())
        })
        .collect();
    assert_eq!(rendered.len(), 2, "{rendered:?}");
    assert!(
        rendered
            .iter()
            .any(|l| l.contains("a.ts") && l.contains("2: const target = 2;"))
    );
    assert!(
        rendered
            .iter()
            .any(|l| l.contains("b.ts") && l.contains("2: another target line"))
    );
}

#[test]
fn context_lines_are_marked_and_deduped() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "f.txt", "one\ntwo\nTHREE\nfour\nfive\n");
    let found = search("THREE", d.path(), true, &None, false, false, 100, 1).unwrap();
    let marks: Vec<(usize, bool)> = found.hits.iter().map(|(_, n, _, m)| (*n, *m)).collect();
    assert_eq!(marks, vec![(2, false), (3, true), (4, false)]);
}

#[test]
fn overlapping_context_windows_do_not_repeat_lines() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "f.txt", "A\nB\nA\nB\nA\n");
    let found = search("A", d.path(), true, &None, false, false, 100, 2).unwrap();
    let lines: Vec<usize> = found.hits.iter().map(|(_, n, _, _)| *n).collect();
    let mut sorted = lines.clone();
    sorted.dedup();
    assert_eq!(lines, sorted, "duplicate line numbers in {lines:?}");
}

#[test]
fn limit_counts_matches_not_context_lines() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "f.txt", "hit\nx\nhit\nx\nhit\nx\n");
    let found = search("hit", d.path(), true, &None, false, false, 2, 1).unwrap();
    assert!(found.limit_reached);
    assert_eq!(found.hits.iter().filter(|h| h.3).count(), 2);
}

#[test]
fn gitignore_is_respected() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), ".gitignore", "ignored/\n");
    write(d.path(), "ignored/hidden.txt", "needle\n");
    write(d.path(), "kept.txt", "needle\n");
    let found = search("needle", d.path(), true, &None, false, false, 100, 0).unwrap();
    let files: Vec<&str> = found.hits.iter().map(|(f, _, _, _)| f.as_str()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].ends_with("kept.txt"), "{files:?}");
}

#[test]
fn literal_and_ignore_case_flags() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "f.txt", "Value: a.b\nvalue: A-B\n");
    // regex a.b matches "a.b" only; literal treats "." as a dot (still one hit here)
    let re = search("a.b", d.path(), true, &None, false, false, 100, 0).unwrap();
    assert_eq!(re.hits.len(), 1);
    let lit = search("a.b", d.path(), true, &None, false, true, 100, 0).unwrap();
    assert_eq!(lit.hits.len(), 1);
    let ci = search("value", d.path(), true, &None, true, false, 100, 0).unwrap();
    assert_eq!(ci.hits.len(), 2);
    let cs = search("value", d.path(), true, &None, false, false, 100, 0).unwrap();
    assert_eq!(cs.hits.len(), 1);
}

#[test]
fn glob_filter_limits_files() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a.ts", "hit\n");
    write(d.path(), "b.py", "hit\n");
    let found = search(
        "hit",
        d.path(),
        true,
        &Some("*.ts".to_string()),
        false,
        false,
        100,
        0,
    )
    .unwrap();
    assert_eq!(found.hits.len(), 1);
    assert!(found.hits[0].0.ends_with("a.ts"));
}

#[test]
fn invalid_regex_is_an_error_not_an_empty_result() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "f.txt", "x\n");
    let err = search("([unclosed", d.path(), true, &None, false, false, 100, 0).unwrap_err();
    assert!(err.contains("invalid pattern"), "{err}");
    let glob_err = search(
        "x",
        d.path(),
        true,
        &Some("[".to_string()),
        false,
        false,
        100,
        0,
    )
    .unwrap_err();
    assert!(glob_err.contains("invalid glob"), "{glob_err}");
}

#[test]
fn binary_files_are_skipped() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("bin.dat"), [0u8, 159, 146, 150, 0, 255]).unwrap();
    write(d.path(), "text.txt", "needle\n");
    let found = search("needle", d.path(), true, &None, false, false, 100, 0).unwrap();
    assert_eq!(found.hits.len(), 1);
    assert!(found.hits[0].0.ends_with("text.txt"));
}

#[test]
fn single_file_search_works_without_a_walk() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "one.txt", "alpha\nbeta\n");
    let found = search(
        "beta",
        &d.path().join("one.txt"),
        false,
        &None,
        false,
        false,
        100,
        0,
    )
    .unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].1, 2);
}

#[test]
fn rg_probe_returns_without_panicking() {
    let _ = rg_present();
}
