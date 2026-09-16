use super::*;

fn e(old: &str, new: &str) -> Edit {
    Edit {
        old_text: old.to_string(),
        new_text: new.to_string(),
        ..Default::default()
    }
}

#[test]
fn strips_cat_n_prefixes_per_line() {
    assert_eq!(strip_cat_n_prefixes("   412\tfoo"), "foo");
    assert_eq!(strip_cat_n_prefixes("     1\t# Agents"), "# Agents");
    assert_eq!(strip_cat_n_prefixes("   100\ta\n   101\tb"), "a\nb");
}

#[test]
fn leaves_non_prefix_lines_alone() {
    assert_eq!(strip_cat_n_prefixes("plain"), "plain");
    assert_eq!(strip_cat_n_prefixes("\tindented"), "\tindented");
    assert_eq!(strip_cat_n_prefixes("12 \tspaced"), "12 \tspaced");
    assert_eq!(strip_cat_n_prefixes("abc\tdef"), "abc\tdef");
}

#[test]
fn strip_set_gates_on_old_text_both_or_neither() {
    // oldText without a prefix → nothing to retry (None): newText alone
    // is never stripped.
    let only_new = vec![e("foo", "   3\tbar")];
    assert!(strip_edit_prefixes(&only_new).is_none());
    // oldText with a prefix → both stripped together.
    let both = vec![e("   3\tfoo", "   3\tbar")];
    let got = strip_edit_prefixes(&both).unwrap();
    assert_eq!(got[0].old_text, "foo");
    assert_eq!(got[0].new_text, "bar");
    assert_eq!(got[0].line_hint, Some(3));
}

#[test]
fn stripped_retry_order_exact_first() {
    let content = "12\tfoo\n";
    let exact = vec![e("12\tfoo", "12\tbaz")];
    let applied = apply_edits_to_normalized_content(content, &exact, "f").unwrap();
    assert!(applied.new_content.contains("12\tbaz"));

    let prefixed = vec![e("   412\tfoo", "   412\tbaz")];
    assert!(apply_edits_to_normalized_content(content, &prefixed, "f").is_err());
    let stripped = strip_edit_prefixes(&prefixed).unwrap();
    let repaired = apply_edits_to_normalized_content(content, &stripped, "f").unwrap();
    assert!(repaired.new_content.contains("12\tbaz"));
}

#[test]
fn multiple_occurrences_defaults_to_first_with_note() {
    let content = "item\nother\nitem\n";
    let edits = vec![e("item", "replaced")];
    let result = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
    assert_eq!(result.new_content, "replaced\nother\nitem\n");
    assert_eq!(result.notes.len(), 1);
    assert!(result.notes[0].contains("found 2 occurrences"));
    assert!(result.notes[0].contains("edited occurrence 1"));
}

#[test]
fn multiple_occurrences_disambiguated_by_line_hint() {
    let content = "line 1\nmatch\nline 3\nline 4\nmatch\nline 6\n";
    // Second match is at line 5
    let edits = vec![Edit {
        line_hint: Some(5),
        ..e("match", "second")
    }];
    let result = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
    assert_eq!(
        result.new_content,
        "line 1\nmatch\nline 3\nline 4\nsecond\nline 6\n"
    );
    assert!(result.notes[0].contains("disambiguated 2 occurrences"));
    assert!(result.notes[0].contains("line 5"));
}

#[test]
fn multiple_occurrences_disambiguated_by_occurrence_index() {
    let content = "one\nmatch\ntwo\nmatch\nthree\nmatch\n";
    // Target 2nd occurrence
    let edits2 = vec![Edit {
        occurrence: Some(2),
        ..e("match", "HIT")
    }];
    let res2 = apply_edits_to_normalized_content(content, &edits2, "f.txt").unwrap();
    assert_eq!(res2.new_content, "one\nmatch\ntwo\nHIT\nthree\nmatch\n");

    // Target last occurrence (-1)
    let edits_last = vec![Edit {
        occurrence: Some(-1),
        ..e("match", "LAST")
    }];
    let res_last = apply_edits_to_normalized_content(content, &edits_last, "f.txt").unwrap();
    assert_eq!(
        res_last.new_content,
        "one\nmatch\ntwo\nmatch\nthree\nLAST\n"
    );
}

#[test]
fn multiple_occurrences_replace_all() {
    let content = "foo a\nbar\nfoo b\n";
    let edits = vec![Edit {
        replace_all: Some(true),
        ..e("foo", "qux")
    }];
    let result = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
    assert_eq!(result.new_content, "qux a\nbar\nqux b\n");
    assert!(result.notes[0].contains("replaced all 2 occurrences"));
}

#[test]
fn cat_n_prefix_disambiguates_multiple_occurrences() {
    let content = "line 1\nfoo\nline 3\nline 4\nfoo\nline 6\n";
    // User passed cat -n prefix targeting line 5
    let prefixed = vec![e("     5\tfoo", "     5\tbar")];
    assert!(apply_edits_to_normalized_content(content, &prefixed, "f.txt").is_err());
    let stripped = strip_edit_prefixes(&prefixed).unwrap();
    assert_eq!(stripped[0].line_hint, Some(5));
    let repaired = apply_edits_to_normalized_content(content, &stripped, "f.txt").unwrap();
    assert_eq!(
        repaired.new_content,
        "line 1\nfoo\nline 3\nline 4\nbar\nline 6\n"
    );
    assert!(repaired.notes[0].contains("disambiguated 2 occurrences"));
}
