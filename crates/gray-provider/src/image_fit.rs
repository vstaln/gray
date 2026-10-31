use std::io::Cursor;

const MAX_B64_BYTES: usize = 5 * 1024 * 1024;
const MAX_EDGE_PX: u32 = 8000;

pub(crate) fn fit_image(media_type: &str, data_b64: &str) -> Option<(String, String)> {
    use base64::Engine as _;
    use image::{ImageDecoder, ImageFormat};
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .ok()?;
    let format = image::guess_format(&bytes).ok().or(match media_type {
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/png" => Some(ImageFormat::Png),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        _ => None,
    })?;
    let out_format = match format {
        ImageFormat::Jpeg => ImageFormat::Jpeg,
        ImageFormat::Png | ImageFormat::Gif | ImageFormat::WebP => ImageFormat::Png,
        _ => return None,
    };
    let mut decoder = image::ImageReader::with_format(Cursor::new(&bytes), format)
        .into_decoder()
        .ok()?;
    let (w, h) = decoder.dimensions();
    if data_b64.len() <= MAX_B64_BYTES && w.max(h) <= MAX_EDGE_PX {
        return None;
    }
    let orientation = decoder.orientation().ok()?;
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    let edge = img.width().max(img.height());
    if edge > MAX_EDGE_PX {
        let scale = f64::from(MAX_EDGE_PX) / f64::from(edge);
        img = img.resize(
            ((f64::from(img.width()) * scale).round() as u32).max(1),
            ((f64::from(img.height()) * scale).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        );
    }
    let mime = if out_format == ImageFormat::Jpeg {
        "image/jpeg"
    } else {
        "image/png"
    };
    for attempt in 0..4 {
        let mut buf = Vec::new();
        if img
            .write_to(&mut Cursor::new(&mut buf), out_format)
            .is_err()
        {
            return None;
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
        if b64.len() <= MAX_B64_BYTES {
            return Some((mime.to_string(), b64));
        }
        if attempt == 3 {
            break;
        }
        let (w, h) = (img.width().max(1) / 2, img.height().max(1) / 2);
        img = img.resize(w, h, image::imageops::FilterType::Triangle);
    }
    None
}

#[cfg(test)]
#[path = "image_fit_tests.rs"]
mod tests;
