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
    assert!(fit_image("image/png", &noisy_png_b64(64, 64)).is_none());
}

#[test]
fn oversized_image_is_shrunk_under_the_cap() {
    let b64 = noisy_png_b64(1800, 1400);
    assert!(b64.len() > MAX_B64_BYTES);
    let (mime, out) = fit_image("image/png", &b64).expect("should refit");
    assert_eq!(mime, "image/png");
    assert!(out.len() <= MAX_B64_BYTES);
}
