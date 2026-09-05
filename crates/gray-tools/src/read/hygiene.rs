//! T1.4 — hygiene unit: BOM strip, CRLF→LF, binary sniff, char-boundary cuts.
//!
//! Pure functions + `infer` (no other new deps). Wired in `read/mod.rs`:
//! `ReadTool::execute` routes the raw bytes through [`prepare`] right after
//! `tokio::fs::read`, before line counting/windowing.
//!
//! ```ignore
//! pub mod hygiene;
//! let text = match hygiene::prepare(&data, &display) {
//!     Ok(t) => t,
//!     Err(note) => return ToolOutput::ok(note), // fact, is_error=false
//! };
//! ```
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

/// Normalize line endings to `\n`: CRLF pairs first, then any lone CR.
/// (Line counting must match what an editor shows.)
pub fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
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

/// Full hygiene pass: BOM → sniff → lossy decode → newline normalize.
/// `Ok(text)` is window-ready; `Err(note)` is the binary one-liner.
pub fn prepare(data: &[u8], display: &str) -> Result<String, String> {
    let bytes = strip_bom(data);
    sniff(bytes, display)?;
    Ok(normalize_newlines(&String::from_utf8_lossy(bytes)))
}

/// Byte index moved down to a UTF-8 char boundary (for byte truncation).
/// Lives here per the T1.4 spec; the copies in `gray-core/src/tool_out.rs`
/// stay untouched (shared-file freeze — the integrator dedups them).
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Byte index moved up to a UTF-8 char boundary (see [`floor_char_boundary`]).
pub fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::truncate::format_size;

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
    fn bom_txt_first_line_has_no_bom() {
        let raw = "\u{FEFF}fn main() {}\nprintln!(\"hi\");\n";
        let text = prepare(raw.as_bytes(), "bom.txt").unwrap();
        assert!(!text.contains('\u{FEFF}'));
        assert_eq!(text.lines().count(), 2);
        assert!(text.starts_with("fn main() {}"));
    }

    #[test]
    fn crlf_and_lone_cr_match_lf_line_count() {
        let crlf = prepare(b"alpha\r\nbeta\r\ngamma\r\n", "crlf.txt").unwrap();
        assert_eq!(crlf, "alpha\nbeta\ngamma\n");
        assert_eq!(crlf.lines().count(), 3);
        assert_eq!(prepare(b"a\rb\r\nc", "x").unwrap(), "a\nb\nc");
        // Fast-path content without CR passes through unchanged.
        assert_eq!(normalize_newlines("a\nb\n"), "a\nb\n");
    }

    #[test]
    fn plain_text_shaped_like_png_extension_is_read_as_text() {
        let text = prepare(
            b"this is plain text wearing a .png extension\nsecond line\n",
            "fake.png",
        )
        .unwrap();
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn png_magic_returns_mime_note() {
        let mut real = b"\x89PNG\r\n\x1a\n".to_vec();
        real.extend((0..1024).map(|i| (i % 256) as u8));
        let err = prepare(&real, "real.png").unwrap_err();
        assert_eq!(
            err,
            format!(
                "[read: real.png is image/png ({}), not shown]",
                format_size(real.len())
            )
        );
    }

    #[test]
    fn nul_bytes_return_nul_note() {
        let nul: Vec<u8> = (0..4096)
            .map(|i| if i % 8 == 7 { 0 } else { b'A' + (i % 26) as u8 })
            .collect();
        assert_eq!(
            prepare(&nul, "nul.bin").unwrap_err(),
            "[read: nul.bin looks binary (NUL bytes), not shown]"
        );
    }

    #[test]
    fn magic_wins_over_nul_when_both_present() {
        // real.png's 0..255 junk contains NULs — infer must fire first.
        let mut real = b"\x89PNG\r\n\x1a\n".to_vec();
        real.extend([0, 1, 2, 3]);
        let err = prepare(&real, "real.png").unwrap_err();
        assert!(err.contains("image/png"), "{err}");
        assert!(!err.contains("NUL"), "{err}");
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
    fn svg_shaped_content_is_read_as_text() {
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"5\"/></svg>\n";
        assert!(prepare(svg, "icon.svg").is_ok());
    }

    #[test]
    fn empty_file_is_text() {
        assert_eq!(prepare(b"", "empty.txt").unwrap(), "");
    }

    #[test]
    fn boundaries_never_split_a_codepoint() {
        let s = "a\u{1F600}b"; // a + 😀 (4 bytes) + b
        for i in 0..=s.len() {
            let f = floor_char_boundary(s, i);
            let c = ceil_char_boundary(s, i);
            assert!(s.is_char_boundary(f), "floor {i} -> {f}");
            assert!(s.is_char_boundary(c), "ceil {i} -> {c}");
            assert!(f <= i && i <= c, "floor/ceil bracket {i}");
            let _ = &s[..f];
            let _ = &s[c..];
        }
        assert_eq!(floor_char_boundary(s, 2), 1);
        assert_eq!(ceil_char_boundary(s, 2), 5);
        assert_eq!(floor_char_boundary(s, 999), s.len());
        assert_eq!(ceil_char_boundary(s, 999), s.len());
    }
}
