use super::*;

// file:// URL decoding is platform-shaped: Windows file URLs carry a
// drive letter (file:///C:/x), so the unix-form cases are unix-only.
#[cfg(unix)]
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
}

#[test]
fn raw_paths_pass_through() {
    // Literal % must not be corrupted.
    assert_eq!(decoded_paste_path("/tmp/100%.png"), "/tmp/100%.png");
    assert_eq!(decoded_paste_path("/tmp/plain.png"), "/tmp/plain.png");
}
