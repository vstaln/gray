use std::io::Cursor;

const MAX_B64_BYTES: usize = 5 * 1024 * 1024;
const MAX_EDGE_PX: u32 = 8000;
const MAX_ENCODE_ATTEMPTS: u32 = 6;

/// How an outgoing image payload resolves against the wire size limit.
pub(crate) enum ImageFit {
    /// Already within limits; send the original bytes.
    Fits,
    /// Re-encoded/resized replacement: (media_type, base64).
    Refit(String, String),
    /// Over the limit and could not be shrunk — must never be sent raw.
    TooLarge,
}

pub(crate) fn fit_image(media_type: &str, data_b64: &str) -> ImageFit {
    use base64::Engine as _;
    use image::{ImageDecoder, ImageFormat};
    let oversized = data_b64.len() > MAX_B64_BYTES;
    // Anything we can't decode is passed through untouched when it is under
    // the byte cap, but must be dropped when it is over: forwarding the raw
    // payload is exactly what produces the provider's "exceeds API limit"
    // stream break.
    let undecodable = || {
        if oversized {
            ImageFit::TooLarge
        } else {
            ImageFit::Fits
        }
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data_b64) else {
        return undecodable();
    };
    let Some(format) = image::guess_format(&bytes).ok().or(match media_type {
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/png" => Some(ImageFormat::Png),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        "image/bmp" | "image/x-bmp" | "image/x-ms-bmp" => Some(ImageFormat::Bmp),
        _ => None,
    }) else {
        return undecodable();
    };
    let Ok(mut decoder) =
        image::ImageReader::with_format(Cursor::new(&bytes), format).into_decoder()
    else {
        return undecodable();
    };
    let (w, h) = decoder.dimensions();
    if !oversized && w.max(h) <= MAX_EDGE_PX {
        return ImageFit::Fits;
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let Ok(mut img) = image::DynamicImage::from_decoder(decoder) else {
        return undecodable();
    };
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
    let out_format = if format == ImageFormat::Jpeg {
        ImageFormat::Jpeg
    } else {
        ImageFormat::Png
    };
    let mime = if out_format == ImageFormat::Jpeg {
        "image/jpeg"
    } else {
        "image/png"
    };
    for attempt in 0..MAX_ENCODE_ATTEMPTS {
        let mut buf = Vec::new();
        if img
            .write_to(&mut Cursor::new(&mut buf), out_format)
            .is_err()
        {
            return undecodable();
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
        if b64.len() <= MAX_B64_BYTES {
            return ImageFit::Refit(mime.to_string(), b64);
        }
        if attempt + 1 < MAX_ENCODE_ATTEMPTS {
            let (w, h) = (img.width().max(1) / 2, img.height().max(1) / 2);
            img = img.resize(w, h, image::imageops::FilterType::Triangle);
        }
    }
    ImageFit::TooLarge
}

#[cfg(test)]
#[path = "image_fit_tests.rs"]
mod tests;
