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

#[test]
fn jpeg_exif_orientation_is_applied_before_encoding() {
    let image =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 4, image::Rgb([90, 50, 20])));
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Jpeg)
        .unwrap();
    let jpeg = encoded.into_inner();
    // EXIF little-endian IFD with Orientation=6 (90 degrees clockwise).
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut oriented = jpeg[..2].to_vec();
    oriented.extend_from_slice(b"\xff\xe1");
    oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    oriented.extend_from_slice(exif);
    oriented.extend_from_slice(&jpeg[2..]);
    let (_, bytes) = normalize_image_bytes(&oriented).unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (4, 8));
}
