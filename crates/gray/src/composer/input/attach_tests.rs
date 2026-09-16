use super::*;
use std::io::Write;

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn file_url_percent_decoding() {
    assert_eq!(
        decoded_paste_path("file:///tmp/my%20photo.png"),
        "/tmp/my photo.png"
    );
    assert_eq!(
        decoded_paste_path("file://localhost/tmp/my%20photo.png"),
        "/tmp/my photo.png"
    );
    assert_eq!(
        decoded_paste_path("\"file:///tmp/a%20b.png\""),
        "/tmp/a b.png"
    );
    // Raw paths pass through: literal % must not be corrupted.
    assert_eq!(decoded_paste_path("/tmp/100%.png"), "/tmp/100%.png");
    assert_eq!(decoded_paste_path("/tmp/plain.png"), "/tmp/plain.png");
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn size_gate_uses_limit() {
    let mut tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
    tmp.write_all(b"0123456789").unwrap();
    tmp.flush().unwrap();
    assert!(file_size_exceeds(tmp.path(), 5));
    assert!(!file_size_exceeds(tmp.path(), MAX_ATTACH_FILE_BYTES));
    assert!(!file_size_exceeds(
        Path::new("/tmp/gray-test-no-such-file-xyz"),
        0
    ));
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn attachable_path_small_png_passes() {
    let mut tmp = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
    tmp.write_all(b"fake-png").unwrap();
    tmp.flush().unwrap();
    assert!(is_attachable_path(&tmp.path().display().to_string()));
    assert!(!is_attachable_path("/tmp/gray-test-no-such-file-xyz.png"));
    assert!(!is_attachable_path("not a path\nwith newline.png"));
}
