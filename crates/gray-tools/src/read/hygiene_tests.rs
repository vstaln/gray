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
fn nul_bytes_are_binary_plain_text_passes() {
    assert!(sniff(b"hello world", "p").is_ok());
    assert!(sniff(b"", "p").is_ok());
    assert!(sniff(b"\xEF\xBB\xBFhi", "p").is_ok());
    assert!(sniff(b"ab\x00cd", "p").is_err());
}
