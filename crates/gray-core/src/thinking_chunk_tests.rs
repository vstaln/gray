use super::append_thinking_chunk;

fn join(chunks: &[&str]) -> String {
    let mut buf = String::new();
    for c in chunks {
        append_thinking_chunk(&mut buf, c);
    }
    buf
}

#[test]
fn inserts_space_at_bare_sentence_boundaries() {
    // The reported muse-spark shape: sentence-sized deltas, stripped space.
    assert_eq!(
        join(&["text appears truncated.", "Identifying that leading"]),
        "text appears truncated. Identifying that leading"
    );
    assert_eq!(
        join(&[
            "Analyzing reflow_on_resize behavior and cursor position handling to explain why reasoning text appears truncated.",
            "Identifying that leading whitespace is omitted during wrapping.",
            "Diagnosing Paragraph truncation on resize.",
        ]),
        "Analyzing reflow_on_resize behavior and cursor position handling to explain why reasoning text appears truncated. Identifying that leading whitespace is omitted during wrapping. Diagnosing Paragraph truncation on resize."
    );
}

#[test]
fn leaves_ambiguous_letter_boundaries_glued() {
    // `handling` + `to` (dropped word space) is indistinguishable from
    // `Anal` + `yzing` (mid-word BPE split): both are letter+letter with
    // no whitespace. Repairing the former would corrupt every sub-word
    // token of well-behaved providers, so the hardcoded rule only fires
    // on punctuation boundaries and leaves these alone.
    assert_eq!(join(&["handling", "to explain"]), "handlingto explain");
}

#[test]
fn never_doubles_existing_whitespace() {
    assert_eq!(join(&["end. ", "Next"]), "end. Next");
    assert_eq!(join(&["end.", " Next"]), "end. Next");
    assert_eq!(join(&["end.\n", "Next"]), "end.\nNext");
    assert_eq!(join(&["end.", "\nNext"]), "end.\nNext");
}

#[test]
fn leaves_mid_word_bpe_splits_glued() {
    // Well-behaved providers emit sub-word continuations bare and correct.
    assert_eq!(join(&["Anal", "yzing"]), "Analyzing");
    assert_eq!(join(&["trunca", "ted"]), "truncated");
    assert_eq!(join(&["reflow", "OnResize"]), "reflowOnResize");
}

#[test]
fn keeps_numbers_and_closing_punctuation_glued() {
    assert_eq!(join(&["value 3.", "14 total"]), "value 3.14 total");
    assert_eq!(join(&["1,", "000 rows"]), "1,000 rows");
    assert_eq!(join(&["at 12:", "30 sharp"]), "at 12:30 sharp");
    assert_eq!(join(&["etc.", ", and more"]), "etc., and more");
}

#[test]
fn empty_sides_pass_through() {
    assert_eq!(join(&["", "hi"]), "hi");
    assert_eq!(join(&["hi", ""]), "hi");
    assert_eq!(join(&[]), "");
}
