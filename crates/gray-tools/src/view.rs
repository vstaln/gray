use std::path::{Path, PathBuf};

use gray_core::agent::{AttachedImage, AttachedMedia};

#[derive(Debug)]
pub struct Shown {
    pub path: PathBuf,
    pub media_type: String,

    pub data: String,

    pub derived_from_video: bool,
}

#[derive(Debug)]
pub enum ViewError {
    NotAnImage(PathBuf),
    Read(PathBuf, std::io::Error),
    Media(PathBuf, crate::images::MediaError),
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnImage(p) => write!(
                f,
                "not an image or video file: {} — expected png/jpg/jpeg/gif/webp/bmp/heic/heif or mp4/mov/webm/mkv/avi; use cat for text files",
                p.display()
            ),
            Self::Read(p, e) => write!(f, "view failed for {}: {e}", p.display()),
            Self::Media(p, e) => write!(f, "view failed for {}: {e}", p.display()),
        }
    }
}

pub fn load(path: &Path) -> Result<Shown, ViewError> {
    if !crate::images::is_image_extension(path) && !crate::images::is_video_extension(path) {
        return Err(ViewError::NotAnImage(path.to_path_buf()));
    }

    let was_video = crate::images::is_video_extension(path);
    let bytes = if was_video {
        crate::video_sheet::video_sheet(path, crate::video_sheet::DEFAULT_FRAMES)
            .map_err(|e| ViewError::Media(path.to_path_buf(), e))?
    } else {
        std::fs::read(path).map_err(|e| ViewError::Read(path.to_path_buf(), e))?
    };
    let (media_type, out) = crate::images::normalize_image_bytes(&bytes)
        .map_err(|e| ViewError::Media(path.to_path_buf(), e))?;
    use base64::Engine as _;
    Ok(Shown {
        path: path.to_path_buf(),
        media_type,
        data: base64::engine::general_purpose::STANDARD.encode(&out),
        derived_from_video: was_video,
    })
}

pub enum Attached {
    Image(AttachedImage, String),

    Media(AttachedMedia, String),

    Text(String, String),
}

pub fn attach(full: &Path) -> Result<Attached, String> {
    use crate::images::{audio_media_type, is_pdf_extension, is_video_extension};
    use gray_core::message::{ContentBlock, MAX_NATIVE_MEDIA_BYTES};
    let name = full.display().to_string();
    let read_raw =
        || -> Result<Vec<u8>, String> { std::fs::read(full).map_err(|e| format!("{name}: {e}")) };
    let b64 = |raw: Vec<u8>| {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(raw)
    };
    if is_pdf_extension(full) {
        let text = match crate::images::pdf_text(full) {
            Ok(t) => format!("--- {name} (PDF text) ---\n{t}"),
            Err(e) => format!("({name}: PDF text extraction failed: {e})"),
        };
        let raw = read_raw()?;
        if raw.len() > MAX_NATIVE_MEDIA_BYTES {
            return Ok(Attached::Text(text, format!("{name} (pdf text)")));
        }
        return Ok(Attached::Media(
            AttachedMedia {
                media_type: "application/pdf".into(),
                data: b64(raw),
                fallback: vec![ContentBlock::text(text)],
            },
            format!("{name} (pdf)"),
        ));
    }
    if let Some(mt) = audio_media_type(full) {
        let raw = read_raw()?;
        if raw.len() > MAX_NATIVE_MEDIA_BYTES {
            return Err(format!("{name}: audio over the 8MB native cap"));
        }
        return Ok(Attached::Media(
            AttachedMedia {
                media_type: mt.into(),
                data: b64(raw),
                fallback: vec![ContentBlock::text(format!(
                    "(audio {name} omitted: this model has no audio input)"
                ))],
            },
            format!("{name} (audio)"),
        ));
    }
    let part = load(full).map_err(|e| e.to_string())?;
    let img = AttachedImage {
        media_type: part.media_type,
        data: part.data,
    };
    if !is_video_extension(full) {
        return Ok(Attached::Image(img, name));
    }

    match std::fs::read(full) {
        Ok(raw) if raw.len() <= MAX_NATIVE_MEDIA_BYTES => Ok(Attached::Media(
            AttachedMedia {
                media_type: crate::images::video_media_type(full).into(),
                data: b64(raw),
                fallback: vec![
                    ContentBlock::text(format!(
                        "(contact sheet of {name}: this model has no native video input)"
                    )),
                    ContentBlock::image(img.media_type, img.data),
                ],
            },
            format!("{name} (video)"),
        )),
        _ => Ok(Attached::Image(img, format!("{name} (contact sheet)"))),
    }
}
