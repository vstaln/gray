use super::*;
use serde_json::json;
use std::collections::HashSet;

fn known_rw() -> HashSet<String> {
    ["read", "write", "edit"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn w(id: &str, path: &str) -> (String, String, serde_json::Value) {
    (
        id.into(),
        "write".into(),
        json!({"path": path, "content": "x"}),
    )
}

#[test]
fn disjoint_writes_share_one_parallel_segment() {
    let u = vec![w("a", "a.txt"), w("b", "b.txt")];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Parallel(vec![0, 1])]
    );
}

#[test]
fn disjoint_write_edit_mix_batches() {
    let u = vec![
        w("a", "a.txt"),
        (
            "b".into(),
            "edit".into(),
            json!({"path": "b.txt", "edits": []}),
        ),
    ];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Parallel(vec![0, 1])]
    );
}

#[test]
fn same_path_writes_stay_sequential() {
    let u = vec![w("a", "a.txt"), w("b", "a.txt")];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Single(0), Segment::Single(1)]
    );
}

#[test]
fn same_path_write_edit_stays_sequential() {
    let u = vec![
        w("a", "a.txt"),
        (
            "b".into(),
            "edit".into(),
            json!({"path": "a.txt", "edits": []}),
        ),
    ];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Single(0), Segment::Single(1)]
    );
}

#[test]
fn dir_prefix_overlap_stays_sequential() {
    let u = vec![w("a", "dir/"), w("b", "dir/file.txt")];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Single(0), Segment::Single(1)]
    );
}

#[test]
fn normalization_spellings_still_conflict() {
    // `./a.txt` == `a.txt`, and `a//b` == `a/b` — same file, barrier.
    let u = vec![w("a", "./a.txt"), w("b", "a.txt")];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Single(0), Segment::Single(1)]
    );
    let u = vec![w("a", "sub//f.txt"), w("b", "sub/f.txt")];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Single(0), Segment::Single(1)]
    );
}

#[test]
fn reads_around_a_write_stay_split_but_batch_where_legal() {
    // Single reads around a write: the write is a barrier, singletons demote.
    let u = vec![
        ("a".into(), "read".into(), json!({"path": "a.txt"})),
        w("b", "b.txt"),
        ("c".into(), "read".into(), json!({"path": "c.txt"})),
    ];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Single(0), Segment::Single(1), Segment::Single(2)]
    );
    // Runs of reads still batch on either side of the write.
    let u = vec![
        ("a".into(), "read".into(), json!({"path": "a.txt"})),
        ("b".into(), "read".into(), json!({"path": "b.txt"})),
        w("c", "c.txt"),
        ("d".into(), "read".into(), json!({"path": "d.txt"})),
        ("e".into(), "read".into(), json!({"path": "e.txt"})),
    ];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![
            Segment::Parallel(vec![0, 1]),
            Segment::Single(2),
            Segment::Parallel(vec![3, 4]),
        ]
    );
}

#[test]
fn missing_or_non_string_path_is_barrier() {
    // Missing `path` demotes (fail-safe) and splits the surrounding run.
    let u = vec![
        w("a", "a.txt"),
        ("b".into(), "write".into(), json!({"content": "x"})),
        w("c", "c.txt"),
    ];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Single(0), Segment::Single(1), Segment::Single(2)]
    );
    // Non-string `path` demotes the same way.
    let u = vec![
        w("a", "a.txt"),
        ("b".into(), "edit".into(), json!({"path": 42})),
    ];
    assert_eq!(
        plan_segments(&u, &known_rw()),
        vec![Segment::Single(0), Segment::Single(1)]
    );
}

#[test]
fn lone_write_still_demotes_to_single() {
    let u = vec![w("a", "a.txt")];
    assert_eq!(plan_segments(&u, &known_rw()), vec![Segment::Single(0)]);
}
