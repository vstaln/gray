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
