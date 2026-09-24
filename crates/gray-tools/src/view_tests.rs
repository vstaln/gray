use super::*;

use std::io::Cursor;

fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([9, 8, 7]));
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    buf
}

#[test]
fn load_shows_a_png_as_a_ready_part() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plot.png");
    std::fs::write(&path, png_bytes(8, 8)).unwrap();

    let shown = load(&path).expect("a png must load");
    assert_eq!(shown.media_type, "image/png");
    assert_eq!(shown.path, path);
    assert!(!shown.data.is_empty(), "base64 must carry pixels");
}

#[test]
fn load_refuses_a_text_file_before_decoding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# heading").unwrap();

    let err = load(&path).expect_err("a text file is not an image");
    let text = err.to_string();
    assert!(text.contains("not an image or video file"), "{text}");
    // Names bash's role rather than blaming a decoder.
    assert!(text.contains("use cat for text files"), "{text}");
}

#[test]
fn load_reports_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let err = load(&dir.path().join("gone.png")).expect_err("missing file must fail");
    assert!(err.to_string().contains("view failed"), "{err}");
}

#[test]
fn load_fails_loudly_on_a_text_file_named_png() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("liar.png");
    std::fs::write(&path, "definitely not a png").unwrap();

    // Magic bytes decide, so this never reaches a provider.
    let err = load(&path).expect_err("undecodable bytes must fail");
    assert!(err.to_string().contains("view failed"), "{err}");
}

#[test]
fn load_downscales_past_the_2000px_cap() {
    use base64::Engine as _;
    use image::ImageDecoder;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide.png");
    std::fs::write(&path, png_bytes(2400, 40)).unwrap();

    let shown = load(&path).unwrap();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&shown.data)
        .unwrap();
    let decoded = image::ImageReader::with_format(Cursor::new(&raw), image::ImageFormat::Png)
        .into_decoder()
        .unwrap();
    let (w, _h) = decoded.dimensions();
    assert!(w <= 2000, "longest side must be capped, got {w}");
}

#[test]
fn video_extension_passes_the_gate_and_becomes_a_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    // ffmpeg is the decoder; a box that is not a real clip must still reach
    // it and come back as its refusal, not as a "not an image" gate error.
    std::fs::write(&path, b"not really a video").unwrap();

    let err = load(&path).expect_err("garbage bytes must fail");
    let text = err.to_string();
    assert!(
        !text.contains("not an image"),
        "extension gate let it through: {text}"
    );
}

#[test]
fn video_extension_gate_covers_the_common_containers() {
    for ext in ["mp4", "MP4", "mov", "webm", "mkv", "avi"] {
        let path = std::path::Path::new("clip").with_extension(ext);
        assert!(
            crate::images::is_viewable_extension(&path),
            "{ext} must be viewable"
        );
        assert!(
            crate::images::is_video_extension(&path),
            "{ext} must be video"
        );
    }
    // A text file stays text.
    assert!(!crate::images::is_viewable_extension(std::path::Path::new(
        "a.md"
    )));
}

#[test]
fn native_video_is_raw_bytes_not_a_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    // A real mp4 header, so the assertion is about the payload, not luck.
    let mut bytes = b"\x00\x00\x00\x18ftypmp42".to_vec();
    bytes.extend_from_slice(&[0u8; 64]);
    std::fs::write(&path, &bytes).unwrap();

    let (media_type, data) = load_native_video(&path).expect("video must load");
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .unwrap();
    assert_eq!(raw, bytes, "native must be the file itself, byte for byte");
    assert_eq!(media_type, "video/mp4");
}

#[test]
fn native_refuses_an_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, png_bytes(8, 8)).unwrap();
    assert!(load_native_video(&path).is_err(), "an image is not a video part");
}

#[test]
fn video_media_type_follows_the_extension() {
    use crate::images::video_media_type;
    for (name, want) in [
        ("a.mp4", "video/mp4"),
        ("a.MP4", "video/mp4"),
        ("a.webm", "video/webm"),
        ("a.mov", "video/quicktime"),
        ("a.mkv", "video/x-matroska"),
        ("a.avi", "video/x-msvideo"),
        ("a.m4v", "video/x-m4v"),
    ] {
        assert_eq!(video_media_type(std::path::Path::new(name)), want, "{name}");
    }
}
