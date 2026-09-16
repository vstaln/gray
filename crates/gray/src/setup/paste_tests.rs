use super::insert_paste;

#[test]
fn paste_appends_full_key() {
    let mut buf = String::new();
    insert_paste(&mut buf, "sk-opencode-go-abc123XYZ");
    assert_eq!(buf, "sk-opencode-go-abc123XYZ");
}

#[test]
fn paste_strips_trailing_newline() {
    let mut buf = String::new();
    insert_paste(&mut buf, "sk-abc123\n");
    assert_eq!(buf, "sk-abc123");
}

#[test]
fn paste_strips_crlf_and_interior_breaks() {
    let mut buf = String::from("sk-");
    insert_paste(&mut buf, "ab\r\ncd\nef");
    assert_eq!(buf, "sk-abcdef");
}
