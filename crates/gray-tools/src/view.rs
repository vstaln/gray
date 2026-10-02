//! Showing a media file to the model — the bash tool's `cat <media>` claim
//! and pasted attachments both decode here ([`attach`]).
//!
//! bash output is text only, so an agent that renders a chart, screenshot or
//! diagram has nothing to check its own work against. The claim lands here:
//! decode, downscale-before-send (the caps pasted attachments and the
//! `read` tool already use), hand back base64 for a vision block. Text files
//! stay bash's job (`cat`) — the extension gate refuses them before any
//! decoding, so a mislabeled file fails loudly instead of sending an
//! undecodable part.

use std::path::{Path, PathBuf};

use gray_core::agent::{AttachedImage, AttachedMedia};

/// One image, ready to ride along as a vision block. A video path arrives
/// here already tiled by [`crate::video_sheet::video_sheet`], so everything
/// downstream (terminal draw, bash claim, size caps) stays image-only.
#[derive(Debug)]
pub struct Shown {
    pub path: PathBuf,
    pub media_type: String,
    /// base64 of the encoded (downscaled) bytes.
    pub data: String,
    /// The bytes are a contact sheet sampled from a video, not the file
    /// itself. Every consumer that names what it showed reads this so a
    /// sheet is never reported as if the video had been handed over.
    pub derived_from_video: bool,
}

/// Why an image could not be shown, worded for whoever prints it.
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

/// Downscale-before-send one image file: longest side capped at 2000px and
/// base64 under 5MB — [`crate::images::normalize_image_bytes`], the same
/// normalization pasted attachments and the `read` tool take. `cat` is the
/// full-resolution exception; this is the everyday path.
pub fn load(path: &Path) -> Result<Shown, ViewError> {
    // Extension gate first: cheap, and it turns `view notes.md` into a clear
    // refusal naming bash's role instead of a decode failure.
    if !crate::images::is_image_extension(path) && !crate::images::is_video_extension(path) {
        return Err(ViewError::NotAnImage(path.to_path_buf()));
    }
    // A video is not a vision part. Sample it into one tiled JPEG and let
    // that take the identical image path below, so a model without native
    // video input still sees the clip.
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

/// One file as a model part, labeled with what was sent ("x.pdf (pdf)").
/// Shared by the bash `cat` claim and pasted attachments so both agree.
pub enum Attached {
    Image(AttachedImage, String),
    /// Native media; the provider swaps in its fallback per model.
    Media(AttachedMedia, String),
    /// Too big to go natively: its fallback text rides in the tool output.
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
    // A clip under the native cap rides raw, with its sheet as the
    // fallback; the provider sends whichever the model can take.
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

#[path = "view_tests.rs"]
#[cfg(test)]
mod tests;
