//! T5.2 — image-to-vision unit: vision-gated seam T5.3 notebook builds on.
//!
//! Pure functions + `infer` sniff only (std + `infer`, no new deps).
//! UNWIRED: no `read/mod.rs` behavior change except `pub mod image;` (this
//! task's allowed wiring). `ReadTool::execute` does NOT call this yet — the
//! integrator wires it after `hygiene::sniff` returns the mime note, and
//! T5.3 reuses [`plan`]/[`scale_note`] for notebook `image/png` outputs.
//!
//! ```ignore
//! pub mod image;
//! // in ReadTool::execute, when hygiene::sniff reports image/*:
//! // if let Some(mime) = image::sniff_is_image(&data) {
//! //     if !image::vision_enabled() { return ok(image::not_attached_note(w, h)); }
//! //     let p = image::plan(&mime, w, h); // w/h from the vision decoder
//! //     return ok(image::scale_note(&p)); // + Attachment (gray-core owner)
//! // }
//! ```
//!
//! Spec: plan.ts T5.2. Long edge ≤ 1568 px; JPEG ladder 95→80→60→40→20 until
//! ≤ 1 MiB; PNG kept only if it already fits and has alpha; text part always
//! present with the scale factor; `GRAY_VISION=0` (or the provider's
//! no-vision flag, owned outside this file) falls back to text only.
//!
//! Contract strings live HERE until `notices.rs` (T1.3 owner) moves them
//! verbatim (one owner per string — same staging as `bulk.rs`/`resolve.rs`).
//!
//! FOLLOW-UPS (not done here — files outside T5.2 ownership):
//! 1. `gray-core/src/agent.rs`: `ToolOutput.attachments: Vec<Attachment>`
//!    (`#[serde(default)]`, NOT persisted — replay stores
//!    `[image omitted on replay]`). Owner: gray-core.
//! 2. `gray-provider/src/**`: encode image parts; endpoints rejecting image
//!    parts in tool results emit the image as a following user message.
//! 3. Pixel ops behind a `vision` cargo feature (`image` crate): decode w/h,
//!    downscale to [`scaled_dims`], JPEG ladder via [`JPEG_QUALITIES`].
//!    This file deliberately has zero pixel deps so the default build is
//!    unchanged in size.
//!
//! // ponytail: dims math + notes only, no `image` crate — covers the T5.3
//! // seam (notebook image outputs reuse plan/scale_note) with zero new deps.
//! // Add the decoder/encoder when the gray-core Attachment type lands.

/// Long edge ceiling for attached images (plan-fixed).
pub const LONG_EDGE_MAX: u32 = 1568;

/// Attachment byte ceiling (plan-fixed: ladder until ≤ 1 MiB).
pub const ATTACH_BYTES_MAX: usize = 1024 * 1024;

/// JPEG quality ladder, in order (plan-fixed).
pub const JPEG_QUALITIES: &[u8] = &[95, 80, 60, 40, 20];

/// Text-only fallback hint (staged; `notices.rs` owner moves it verbatim).
pub const NO_VISION_HINT: &str = "use the vision plugin if available";

/// Gate: `GRAY_VISION=0` forces text-only even when the model could see.
/// The provider's own no-vision flag is checked by the integrator alongside
/// this (outside this file's ownership).
pub fn vision_enabled() -> bool {
    vision_enabled_for(std::env::var("GRAY_VISION").ok().as_deref())
}

/// Pure gate core (in-file tests cover this; the env wrapper stays trivial).
pub fn vision_enabled_for(var: Option<&str>) -> bool {
    match var {
        None => true,
        Some(v) => v.trim() != "0",
    }
}

/// MIME types eligible for vision attach. SVG stays text (per
/// `hygiene::is_text_mime`); everything else under `image/` attaches.
pub fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/svg+xml"
}

/// Magic-byte sniff over the first 8 KiB: `Some(mime)` when the bytes are
/// an attachable image, `None` otherwise (text, SVG, unknown). Extension is
/// never consulted — `fake.png` (text wearing `.png`) returns `None`.
pub fn sniff_is_image(data: &[u8]) -> Option<String> {
    let len = data.len().min(crate::read::hygiene::SNIFF_SAMPLE_BYTES);
    let kind = infer::get(&data[..len])?;
    let mime = kind.mime_type();
    is_image_mime(mime).then(|| mime.to_string())
}

/// Downscaled dims + coordinate scale: long edge capped at
/// [`LONG_EDGE_MAX`]; `scale` = orig / attached (multiply image-space
/// coordinates by it). Passthrough (scale 1.0) when already small.
/// Zero dims pass through (no div-by-zero; the decoder owner rejects them).
pub fn scaled_dims(width: u32, height: u32) -> (u32, u32, f32) {
    let long = width.max(height);
    if long <= LONG_EDGE_MAX || long == 0 {
        return (width, height, 1.0);
    }
    let factor = LONG_EDGE_MAX as f32 / long as f32;
    let w = ((width as f32 * factor).round() as u32).max(1);
    let h = ((height as f32 * factor).round() as u32).max(1);
    (w, h, 1.0 / factor)
}

/// Vision attachment plan: what the decoder (follow-up) attaches.
/// `bytes` are filled by the encoder owner; this seam carries dims + note.
pub struct ImagePlan {
    /// e.g. `image/png` (sniffed, never from the extension).
    pub mime: String,
    /// On-disk dims.
    pub width: u32,
    /// On-disk dims.
    pub height: u32,
    /// Attached dims (after [`scaled_dims`]).
    pub attached_width: u32,
    /// Attached dims (after [`scaled_dims`]).
    pub attached_height: u32,
    /// `orig / attached` for coordinate mapping.
    pub scale: f32,
}

/// Build the plan from sniffed mime + decoded on-disk dims.
pub fn plan(mime: &str, width: u32, height: u32) -> ImagePlan {
    let (w, h, scale) = scaled_dims(width, height);
    ImagePlan {
        mime: mime.to_string(),
        width,
        height,
        attached_width: w,
        attached_height: h,
        scale,
    }
}

/// Scale note — always present with the attachment (plan-exact wording).
pub fn scale_note(p: &ImagePlan) -> String {
    format!(
        "[read: image {}x{} on disk, attached at {}x{}. \
          Multiply coordinates you compute from the image by {:.2}.]",
        p.width, p.height, p.attached_width, p.attached_height, p.scale
    )
}

/// Text-only fallback when vision is off (fact, `is_error=false`).
pub fn not_attached_note(width: u32, height: u32) -> String {
    format!(
        "[read: image {width}x{height}, not attached (vision off). \
          {NO_VISION_HINT}.]"
    )
}

/// PNG fast path: keep the original bytes only when they already fit AND
/// carry alpha (otherwise the JPEG ladder is smaller). Otherwise re-encode.
pub fn keep_png(has_alpha: bool, bytes_len: usize) -> bool {
    has_alpha && bytes_len <= ATTACH_BYTES_MAX
}

/// Ladder stop condition: this quality step fits the cap.
pub fn fits_within_cap(bytes_len: usize) -> bool {
    bytes_len <= ATTACH_BYTES_MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_image_passes_through_at_scale_one() {
        assert_eq!(scaled_dims(800, 600), (800, 600, 1.0));
        assert_eq!(scaled_dims(1568, 1000), (1568, 1000, 1.0));
        let p = plan("image/png", 800, 600);
        assert_eq!(
            scale_note(&p),
            "[read: image 800x600 on disk, attached at 800x600. \
              Multiply coordinates you compute from the image by 1.00.]"
        );
    }

    #[test]
    fn four_k_downscales_with_scale_note() {
        let (w, h, scale) = scaled_dims(3840, 2160);
        assert_eq!((w, h), (1568, 882));
        assert!((scale - 2.4489).abs() < 0.01, "{scale}");
        assert_eq!(format!("{scale:.2}"), "2.45");
        let p = plan("image/png", 3840, 2160);
        assert!(scale_note(&p).contains("attached at 1568x882"), "{}", scale_note(&p));
    }

    #[test]
    fn portrait_uses_long_edge() {
        let (w, h, _) = scaled_dims(1000, 3000);
        assert_eq!((w, h), (523, 1568));
    }

    #[test]
    fn zero_dims_never_panic() {
        assert_eq!(scaled_dims(0, 0), (0, 0, 1.0));
    }

    #[test]
    fn text_wearing_png_extension_is_never_image() {
        assert!(sniff_is_image(b"this is plain text wearing a .png extension\n").is_none());
        assert!(!is_image_mime("image/svg+xml"));
        assert!(is_image_mime("image/png"));
        assert!(is_image_mime("image/jpeg"));
        assert!(!is_image_mime("text/plain"));
        assert!(!is_image_mime("application/pdf"));
    }

    #[test]
    fn png_magic_sniffs_as_image() {
        let mut real = b"\x89PNG\r\n\x1a\n".to_vec();
        real.extend((0..1024).map(|i| (i % 256) as u8));
        assert_eq!(sniff_is_image(&real).as_deref(), Some("image/png"));
    }

    #[test]
    fn vision_gate_defaults_on_and_zero_forces_off() {
        assert!(vision_enabled_for(None));
        assert!(vision_enabled_for(Some("1")));
        assert!(!vision_enabled_for(Some("0")));
        assert!(!vision_enabled_for(Some(" 0 ")));
    }

    #[test]
    fn vision_off_note_names_recovery() {
        assert_eq!(
            not_attached_note(3840, 2160),
            "[read: image 3840x2160, not attached (vision off). \
              use the vision plugin if available.]"
        );
    }

    #[test]
    fn png_kept_only_when_small_and_alpha() {
        assert!(keep_png(true, 512));
        assert!(keep_png(true, ATTACH_BYTES_MAX));
        assert!(!keep_png(true, ATTACH_BYTES_MAX + 1));
        assert!(!keep_png(false, 512));
        assert!(fits_within_cap(ATTACH_BYTES_MAX));
        assert!(!fits_within_cap(ATTACH_BYTES_MAX + 1));
        assert_eq!(JPEG_QUALITIES, &[95, 80, 60, 40, 20]);
    }
}
