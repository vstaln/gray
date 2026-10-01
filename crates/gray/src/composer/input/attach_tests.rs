use super::*;

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
