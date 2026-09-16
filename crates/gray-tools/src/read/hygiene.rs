//! T1.4 — hygiene unit: BOM strip, binary sniff, char-boundary cuts.
//!
//! Pure functions + `infer` (no other new deps). Wired by `read/stream.rs`:
//! `LineStream::open` sniffs the first chunk's bytes, then strips BOM per
//! line as it streams. Note wording lives in `notices.rs`.

/// Bytes sniffed for a magic number before any decoding.
pub const SNIFF_SAMPLE_BYTES: usize = 8 * 1024;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// MIME types `infer` may recognize that are still read as text.
pub fn is_text_mime(mime: &str) -> bool {
    mime == "image/svg+xml" || mime.starts_with("text/")
}

/// Strip one leading UTF-8 BOM, if present. Only position 0, only once.
pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)
}

/// Magic-byte sniff over the first 8 KiB. `Ok(())` = text, proceed;
/// `Err(note)` = binary one-liner, return as-is with `is_error=false`.
/// Extension is never consulted — magic bytes (then NUL bytes) only.
pub fn sniff(data: &[u8], display: &str) -> Result<(), String> {
    let sample_len = data.len().min(SNIFF_SAMPLE_BYTES);
    if let Some(kind) = infer::get(&data[..sample_len]) {
        let mime = kind.mime_type();
        if !is_text_mime(mime) {
            return Err(super::notices::mime_note(display, mime, data.len()));
        }
    } else if data[..sample_len].contains(&0) {
        return Err(super::notices::nul_note(display));
    }
    Ok(())
}

#[path = "hygiene_tests.rs"]
#[cfg(test)]
mod tests;
