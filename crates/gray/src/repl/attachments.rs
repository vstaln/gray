use std::path::{Path, PathBuf};

pub use gray_tools::images::{MediaError, normalize_image_bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Pdf,
    Video,
    Audio,
    Unsupported,
}

impl AttachmentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Pdf => "PDF",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Unsupported => "File",
        }
    }
}

pub fn attachment_kind(path: &Path) -> AttachmentKind {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") | Some("jpg") | Some("jpeg") | Some("webp") | Some("gif") | Some("bmp")
        | Some("heic") | Some("heif") => AttachmentKind::Image,
        Some("pdf") => AttachmentKind::Pdf,
        _ if gray_tools::images::is_video_extension(path) => AttachmentKind::Video,
        _ if gray_tools::images::is_audio_extension(path) => AttachmentKind::Audio,
        _ => AttachmentKind::Unsupported,
    }
}

const MAX_INLINE_IMAGES: usize = 8;

const MAX_INLINE_FILE_BYTES: u64 = 100 * 1024 * 1024;

pub fn extract_inline_image_paths(text: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for raw in text.split_whitespace() {
        if out.len() >= MAX_INLINE_IMAGES {
            break;
        }
        let mut tok = raw
            .trim_matches(|c| matches!(c, '"' | '\'' | '`' | '<' | '(' | '['))
            .trim_end_matches(|c| {
                matches!(
                    c,
                    '"' | '\'' | '`' | '>' | ')' | ']' | '.' | ',' | ';' | ':' | '!' | '?'
                )
            });
        if tok.is_empty() || tok.len() > 1024 {
            continue;
        }

        let decoded: std::borrow::Cow<'_, str> = if tok.starts_with("file://") {
            match url::Url::parse(tok)
                .ok()
                .and_then(|u| u.to_file_path().ok())
            {
                Some(p) => std::borrow::Cow::Owned(p.display().to_string()),
                None => {
                    let s = tok.strip_prefix("file://").unwrap_or(tok);
                    std::borrow::Cow::Borrowed(s.strip_prefix("localhost").unwrap_or(s))
                }
            }
        } else {
            std::borrow::Cow::Borrowed(tok)
        };
        tok = decoded.as_ref();
        let candidate = Path::new(tok);

        if attachment_kind(candidate) == AttachmentKind::Unsupported {
            continue;
        }
        let full = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            cwd.join(candidate)
        };
        if full.is_file()
            && std::fs::metadata(&full)
                .map(|m| m.len() < MAX_INLINE_FILE_BYTES)
                .unwrap_or(false)
            && !out.contains(&full)
        {
            out.push(full);
        }
    }
    out
}

pub fn attachment_label(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("clipboard");
    name.chars()
        .map(|c| {
            if matches!(c, '[' | ']' | '\n' | '\r') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

pub(crate) const MARKER_PREFIXES: [&str; 5] = ["image: ", "video: ", "audio: ", "pdf: ", "file: "];

pub fn attachment_marker(path: &Path) -> String {
    let kind = attachment_kind(path).label().to_ascii_lowercase();
    format!("[{kind}: {}]", attachment_label(path))
}

pub fn attachment_markers(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut out = Vec::new();
    let mut search = 0usize;
    while let Some(rel) = text[search..].find('[') {
        let start = search + rel;
        let name_start = MARKER_PREFIXES.iter().find_map(|p| {
            text[start + 1..]
                .starts_with(p)
                .then_some(start + 1 + p.len())
        });
        let Some(name_start) = name_start else {
            search = start + 1;
            continue;
        };
        let Some(end_rel) = text[name_start..].find(']') else {
            break;
        };
        let end = name_start + end_rel;
        if end > name_start {
            out.push((start..end + 1, text[name_start..end].to_string()));
        }
        search = end + 1;
    }
    out
}
