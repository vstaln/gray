use super::*;

#[test]
fn repaired_note_is_contract_exact() {
    assert_eq!(
        crate::read::notices::repaired_note("a/café.txt", "a/cafe.txt"),
        "[read: opened a/café.txt (path repaired from a/cafe.txt)]"
    );
}

#[test]
fn nfc_nfd_round_trip_cafe_and_leave_plain_text_alone() {
    let nfc = "caf\u{E9}.txt";
    let nfd = "cafe\u{301}.txt";
    assert_eq!(to_nfd(nfc), nfd);
    assert_eq!(to_nfc(nfd), nfc);
    assert_eq!(to_nfc(nfc), nfc);
    assert_eq!(to_nfd(nfd), nfd);
    assert_eq!(to_nfc("plain.txt"), "plain.txt");
    assert_eq!(to_nfd("plain.txt"), "plain.txt");
}

#[test]
fn candidates_keep_spec_order_and_dedup() {
    let c = candidates("plain.txt");
    assert_eq!(c[0], "plain.txt");
    assert_eq!(c.len(), 1, "no-op spellings dedup to as-given: {c:?}");
    let c = candidates("caf\u{E9} it's.txt");
    assert_eq!(c[0], "caf\u{E9} it's.txt");
    assert!(c.contains(&"cafe\u{301} it's.txt".to_string())); // (3) NFD
    assert!(c.contains(&"caf\u{E9} it\u{2019}s.txt".to_string())); // (6) curly
    assert!(c.contains(&"cafe\u{301} it\u{2019}s.txt".to_string())); // (7) NFD+curly
    assert_eq!(
        c.len(),
        c.iter().collect::<std::collections::HashSet<_>>().len()
    );
}

/// All 7 spellings, one row each (6 has both quote directions).
/// Each row lives in its own subdir so NFC/NFD byte shapes never collide.
#[test]
fn table_all_seven_spellings_resolve() {
    let rows: &[(&str, &str, &str)] = &[
        ("s1", "hello.txt", "hello.txt"),                  // (1) as given
        ("s2", "cafe\u{301}.txt", "caf\u{E9}.txt"),        // (2) NFC
        ("s3", "caf\u{E9}.txt", "cafe\u{301}.txt"),        // (3) NFD
        ("s4", "with\u{202F}space.txt", "with space.txt"), // (4) NBSP→space
        (
            "s5",
            "Screenshot 3.04 PM.png",
            "Screenshot 3.04\u{202F}PM.png",
        ), // (5) space→U+202F
        ("s6", "it's.md", "it\u{2019}s.md"),               // (6a) ascii→curly
        ("s6r", "it\u{2019}s-rev.md", "it's-rev.md"),      // (6b) curly→ascii
        ("s7", "caf\u{E9}'s.md", "cafe\u{301}\u{2019}s.md"), // (7) NFD+curly
    ];
    let outer = tempfile::tempdir().unwrap();
    for (dir, given, actual) in rows {
        let sub = outer.path().join(dir);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(actual), "x").unwrap();
        let got = resolve_existing(&sub.join(given));
        // macOS APFS stores café.txt NFD on disk: compare NFD→NFC folded.
        let got = got.unwrap_or_else(|| panic!("row {dir}: {given:?} did not resolve"));
        assert_eq!(
            to_nfc(&got.display().to_string()),
            to_nfc(&sub.join(actual).display().to_string()),
            "row {dir}: {given:?}"
        );
    }
}

#[test]
fn nbsp_variant_u00a0_also_maps_to_space() {
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(outer.path().join("with space.txt"), "x").unwrap();
    assert_eq!(
        resolve_existing(&outer.path().join("with\u{A0}space.txt")),
        Some(outer.path().join("with space.txt"))
    );
}

#[test]
fn as_given_wins_and_missing_returns_none() {
    let outer = tempfile::tempdir().unwrap();
    // Both shapes exist: the literal wins (stop at first that exists).
    std::fs::write(outer.path().join("caf\u{E9}.txt"), "nfc").unwrap();
    std::fs::write(outer.path().join("cafe\u{301}.txt"), "nfd").unwrap();
    assert_eq!(
        resolve_existing(&outer.path().join("caf\u{E9}.txt")),
        Some(outer.path().join("caf\u{E9}.txt"))
    );
    assert_eq!(resolve_existing(&outer.path().join("nope.txt")), None);
}

#[test]
fn repair_never_changes_directories() {
    let outer = tempfile::tempdir().unwrap();
    let a = outer.path().join("a");
    let b = outer.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("cafe\u{301}.txt"), "x").unwrap();
    // Same spelling in the sibling dir must NOT leak across.
    assert_eq!(resolve_existing(&a.join("caf\u{E9}.txt")), None);
    assert!(!same_parent(&a.join("x"), &b.join("x")));
    assert!(same_parent(&a.join("x"), &a.join("y")));
}
