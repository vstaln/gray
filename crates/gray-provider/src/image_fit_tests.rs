use super::*;
use base64::Engine as _;

fn noisy_png_b64(w: u32, h: u32) -> String {
    let mut seed: u32 = 0x9e37_79b9;
    let img = image::RgbImage::from_fn(w, h, |_, _| {
        let mut px = [0u8; 3];
        for c in &mut px {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *c = (seed >> 24) as u8;
        }
        image::Rgb(px)
    });
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    base64::engine::general_purpose::STANDARD.encode(&buf)
}

#[test]
fn small_image_passes_through() {
    assert!(matches!(
        fit_image("image/png", &noisy_png_b64(64, 64)),
        ImageFit::Fits
    ));
}

#[test]
fn oversized_image_is_shrunk_under_the_cap() {
    let b64 = noisy_png_b64(1800, 1400);
    assert!(b64.len() > MAX_B64_BYTES);
    match fit_image("image/png", &b64) {
        ImageFit::Refit(mime, out) => {
            assert_eq!(mime, "image/png");
            assert!(out.len() <= MAX_B64_BYTES);
        }
        _ => panic!("should refit"),
    }
}

#[test]
fn oversized_undecodable_image_is_too_large() {
    let b64 = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 4 * 1024 * 1024]);
    assert!(b64.len() > MAX_B64_BYTES);
    assert!(matches!(
        fit_image("image/x-unknown", &b64),
        ImageFit::TooLarge
    ));
}

#[test]
fn undersized_undecodable_image_passes_through() {
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"not an image");
    assert!(matches!(fit_image("image/x-unknown", &b64), ImageFit::Fits));
}
