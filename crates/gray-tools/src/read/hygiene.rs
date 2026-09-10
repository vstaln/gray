//! T1.4 — hygiene unit: BOM strip, binary sniff, char-boundary cuts.
//!
//! Pure functions + `infer` (no other new deps). Wired by `read/stream.rs`:
//! `LineStream::open` sniffs the first chunk's bytes, then strips BOM per
//! line as it streams.
//!
//! Contract strings live in `notices.rs` (moved verbatim at the wave gate);
//! [`mime_note`]/[`nul_note`] below delegate there (one owner per string).

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

fn mime_note(display: &str, mime: &str, size: usize) -> String {
    super::notices::mime_note(display, mime, size)
}

fn nul_note(display: &str) -> String {
    super::notices::nul_note(display)
}

/// Magic-byte sniff over the first 8 KiB. `Ok(())` = text, proceed;
/// `Err(note)` = binary one-liner, return as-is with `is_error=false`.
/// Extension is never consulted — magic bytes (then NUL bytes) only.
pub fn sniff(data: &[u8], display: &str) -> Result<(), String> {
    let sample_len = data.len().min(SNIFF_SAMPLE_BYTES);
    if let Some(kind) = infer::get(&data[..sample_len]) {
        let mime = kind.mime_type();
        if !is_text_mime(mime) {
            return Err(mime_note(display, mime, data.len()));
        }
    } else if data[..sample_len].contains(&0) {
        return Err(nul_note(display));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_stripped_once_at_start_only() {
        assert_eq!(strip_bom(b"\xEF\xBB\xBFhi"), b"hi");
        assert_eq!(strip_bom(b"hi"), b"hi");
        assert_eq!(strip_bom(b""), b"");
        // Only one, only at position 0.
        assert_eq!(strip_bom(b"\xEF\xBB\xBF\xEF\xBB\xBFhi"), b"\xEF\xBB\xBFhi");
        assert_eq!(strip_bom(b"hi\xEF\xBB\xBF"), b"hi\xEF\xBB\xBF");
    }

    #[test]
    fn svg_and_text_mimes_stay_text() {
        assert!(is_text_mime("image/svg+xml"));
        assert!(is_text_mime("text/plain"));
        assert!(is_text_mime("text/html"));
        assert!(!is_text_mime("image/png"));
        assert!(!is_text_mime("application/pdf"));
    }

    #[test]
    fn boundaries_never_split_a_codepoint() {
        let s = "a\u{1F600}b"; // a + 😀 (4 bytes) + b
        for i in 0..=s.len() {
            let f = s.floor_char_boundary(i);
            let c = s.ceil_char_boundary(i);
            assert!(s.is_char_boundary(f), "floor {i} -> {f}");
            assert!(s.is_char_boundary(c), "ceil {i} -> {c}");
            assert!(f <= i && i <= c, "floor/ceil bracket {i}");
            let _ = &s[..f];
            let _ = &s[c..];
        }
        assert_eq!(s.floor_char_boundary(2), 1);
        assert_eq!(s.ceil_char_boundary(2), 5);
        assert_eq!(s.floor_char_boundary(999), s.len());
        assert_eq!(s.ceil_char_boundary(999), s.len());
    }
}
