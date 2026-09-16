use super::*;
use std::io::Cursor;

#[test]
fn kind_map_covers_media() {
    assert_eq!(attachment_kind(Path::new("a.png")), AttachmentKind::Image);
    assert_eq!(attachment_kind(Path::new("a.JPG")), AttachmentKind::Image);
    assert_eq!(attachment_kind(Path::new("a.pdf")), AttachmentKind::Pdf);
    assert_eq!(attachment_kind(Path::new("a.mp4")), AttachmentKind::Video);
    assert_eq!(attachment_kind(Path::new("a.mkv")), AttachmentKind::Video);
    assert_eq!(attachment_kind(Path::new("a.mp3")), AttachmentKind::Audio);
    assert_eq!(attachment_kind(Path::new("a.wav")), AttachmentKind::Audio);
    assert_eq!(
        attachment_kind(Path::new("a.zip")),
        AttachmentKind::Unsupported
    );
    assert_eq!(
        attachment_kind(Path::new("noext")),
        AttachmentKind::Unsupported
    );
}

#[test]
fn normalize_caps_long_side() {
    let img = image::RgbaImage::from_pixel(3000, 100, image::Rgba([9, 9, 9, 255]));
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    let (mime, out) = normalize_image_bytes(&buf).unwrap();
    assert_eq!(mime, "image/png");
    let back = image::load_from_memory(&out).unwrap();
    assert!(back.width().max(back.height()) <= MAX_IMAGE_SIDE);
}

#[test]
fn normalize_rejects_garbage_loudly() {
    assert!(matches!(
        normalize_image_bytes(b"not an image"),
        Err(MediaError::Decode(_))
    ));
}

#[test]
fn pdf_missing_file_errors() {
    assert!(matches!(
        pdf_text(Path::new("/tmp/gray-test-no-such-file-xyz.pdf")),
        Err(MediaError::Extract(_))
    ));
}
