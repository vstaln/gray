use super::*;

#[test]
fn huge_line_count_uses_count_skipped_wording() {
    // Unit-level: the exact total is replaced by a lower bound, but `next`
    // still names an observed line. No 150k-line fixture needed.
    let note = super::notices::line_cap_count_skipped(1, 10, 100_000, 3_000_000, 11);
    assert!(note.contains("count skipped"), "{note}");
    assert!(note.contains("offset=11"), "{note}");
    assert!(note.contains("≥100000 lines"), "{note}");
}
