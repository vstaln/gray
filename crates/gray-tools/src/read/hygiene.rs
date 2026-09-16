//! T1.4 — hygiene unit: BOM strip, binary sniff, char-boundary cuts.
//!
//! Pure functions, no deps. Wired by `read/stream.rs`:
//! `LineStream::open` sniffs the first chunk's bytes, then strips BOM per
//! line as it streams. Note wording lives in `notices.rs`.

/// Bytes sniffed for a magic number before any decoding.
pub const SNIFF_SAMPLE_BYTES: usize = 8 * 1024;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Strip one leading UTF-8 BOM, if present. Only position 0, only once.
pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)
}

/// NUL-byte sniff over the first 8 KiB. `Ok(())` = text, proceed;
/// `Err(note)` = binary one-liner, return as-is with `is_error=false`.
/// Extension is never consulted — NUL bytes only.
// ponytail: magic-sniff dropped, NUL check decides binary. If a NUL-free
// binary format ever slips through as text, restore magic detection.
pub fn sniff(data: &[u8], display: &str) -> Result<(), String> {
    let sample_len = data.len().min(SNIFF_SAMPLE_BYTES);
    if data[..sample_len].contains(&0) {
        return Err(super::notices::nul_note(display));
    }
    Ok(())
}

#[path = "hygiene_tests.rs"]
#[cfg(test)]
mod tests;
