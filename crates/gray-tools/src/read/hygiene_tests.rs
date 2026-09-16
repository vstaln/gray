use super::*;

#[test]
fn bom_stripped_once_at_start_only() {
    assert_eq!(strip_bom(b"\xEF\xBB\xBFhi"), b"hi");
    assert_eq!(strip_bom(b"hi"), b"hi");
    assert_eq!(strip_bom(b""), b"");
    // Only one, only at position 0.
    assert_eq!(strip_bom(b"\xEF\xBB\xBF\xEF\xBB\xBFhi"), b"\xEF\xBB\xBFhi");
    assert_eq!(strip_bom(b"hi\xEF\xBB\xBF"), b"hi\xEF\xBB\xBF");
}

#[test]
fn svg_and_text_mimes_stay_text() {
    assert!(is_text_mime("image/svg+xml"));
    assert!(is_text_mime("text/plain"));
    assert!(is_text_mime("text/html"));
    assert!(!is_text_mime("image/png"));
    assert!(!is_text_mime("application/pdf"));
}

#[test]
fn boundaries_never_split_a_codepoint() {
    let s = "a\u{1F600}b"; // a + 😀 (4 bytes) + b
    for i in 0..=s.len() {
        let f = s.floor_char_boundary(i);
        let c = s.ceil_char_boundary(i);
        assert!(s.is_char_boundary(f), "floor {i} -> {f}");
        assert!(s.is_char_boundary(c), "ceil {i} -> {c}");
        assert!(f <= i && i <= c, "floor/ceil bracket {i}");
        let _ = &s[..f];
        let _ = &s[c..];
    }
    assert_eq!(s.floor_char_boundary(2), 1);
    assert_eq!(s.ceil_char_boundary(2), 5);
    assert_eq!(s.floor_char_boundary(999), s.len());
    assert_eq!(s.ceil_char_boundary(999), s.len());
}
