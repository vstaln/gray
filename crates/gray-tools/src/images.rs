use std::io::Cursor;
use std::path::Path;

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug)]
pub enum MediaError {
    Decode(String),
    TooBig(String),
    Extract(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "could not decode image: {e}"),
            Self::TooBig(e) => write!(f, "image over 20MB: {e}"),
            Self::Extract(e) => write!(f, "extract failed: {e}"),
        }
    }
}

pub fn is_image_extension(path: &Path) -> bool {
    matches!(
        lower_ext(path).as_deref(),
        Some("png")
            | Some("jpg")
            | Some("jpeg")
            | Some("webp")
            | Some("gif")
            | Some("bmp")
            | Some("heic")
            | Some("heif")
    )
}

pub fn is_video_extension(path: &Path) -> bool {
    matches!(
        lower_ext(path).as_deref(),
        Some("mp4")
            | Some("m4v")
            | Some("mov")
            | Some("webm")
            | Some("mkv")
            | Some("avi")
            | Some("mpg")
            | Some("mpeg")
    )
}

fn lower_ext(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

pub fn is_pdf_extension(path: &Path) -> bool {
    lower_ext(path).as_deref() == Some("pdf")
}

pub fn is_audio_extension(path: &Path) -> bool {
    audio_media_type(path).is_some()
}

pub fn audio_media_type(path: &Path) -> Option<&'static str> {
    Some(match lower_ext(path)?.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "aiff" | "aif" => "audio/aiff",
        _ => return None,
    })
}

pub fn is_viewable_extension(path: &Path) -> bool {
    is_image_extension(path)
        || is_video_extension(path)
        || is_pdf_extension(path)
        || is_audio_extension(path)
}

pub const MAX_PDF_CHARS: usize = 60_000;

pub fn pdf_text(path: &Path) -> Result<String, MediaError> {
    let arg_path = path.display().to_string();
    let out = crate::video_sheet::run(
        "pdftotext",
        &["-layout", &arg_path, "-"],
        std::time::Duration::from_secs(30),
    )?;
    if !out.status.success() {
        return Err(crate::video_sheet::detail(&out, "pdftotext failed"));
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err(MediaError::Extract(
            "no extractable text (scanned images?)".to_string(),
        ));
    }
    if text.chars().count() > MAX_PDF_CHARS {
        let cut: String = text.chars().take(MAX_PDF_CHARS).collect();
        Ok(format!("{cut}\n… [truncated at {MAX_PDF_CHARS} chars]"))
    } else {
        Ok(text)
    }
}

pub fn video_media_type(path: &Path) -> &'static str {
    match lower_ext(path).as_deref() {
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        Some("m4v") => "video/x-m4v",
        Some("mpg") | Some("mpeg") => "video/mpeg",
        _ => "video/mp4",
    }
}

pub fn normalize_image_bytes(bytes: &[u8]) -> Result<(String, Vec<u8>), MediaError> {
    use image::ImageFormat;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(MediaError::TooBig(format!("{} bytes", bytes.len())));
    }
    let format = image::guess_format(bytes).map_err(|e| MediaError::Decode(e.to_string()))?;
    let mime = match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Png => "image/png",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",

        other => {
            return Err(MediaError::Decode(format!(
                "{other:?} decoding not enabled"
            )));
        }
    };
    image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(|e| MediaError::Decode(e.to_string()))?;
    Ok((mime.to_string(), bytes.to_vec()))
}
