//! The `view` tool: shows an image file to the model as a vision block.
//!
//! `bash` returns text only, so an agent that renders a chart, screenshot,
//! or diagram otherwise has nothing to check its own output against except
//! pixel dumps and ASCII art. `view` closes that gap: one path in, one
//! vision block out — the same downscale-before-send path pasted
//! attachments use ([`crate::images`]). Text files stay bash's job (`cat`,
//! or the opt-in `read` tool); `view` refuses them instead of half-doing
//! both.

use async_trait::async_trait;
use base64::Engine as _;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};

use crate::{Tool, fail, get_str, resolve_path};

/// Shows an image file to the model as an image.
///
/// No ledger entry: nothing was shown to authorize a later write.
pub struct ViewTool;

#[async_trait]
impl Tool for ViewTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "view",
            "See an image file: it is shown to you as an image, not as text. \
             Use it to check anything visual you or the user produced — \
             renders, charts, screenshots, diagrams, photos. Accepts \
             png/jpg/jpeg/gif/webp; downscaled to 2000px before sending. \
             bash output is text only and cannot do this: never pixel-dump \
             or ASCII-art an image to inspect it — call view with its path.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Image file path (absolute or relative to cwd)"
                    }
                },
                "required": ["path"]
            }),
        )
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = match get_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let full = resolve_path(&ctx.cwd, &path);
        // Extension gate first: cheap, and it turns `view notes.md` into a
        // clear refusal naming bash's role instead of a decode failure.
        if !crate::images::is_image_extension(&full) {
            return fail(format!(
                "not an image file: {} — view accepts png/jpg/jpeg/gif/webp; use bash (cat) for text files",
                full.display()
            ));
        }
        let bytes = match tokio::fs::read(&full).await {
            Ok(b) => b,
            Err(e) => return fail(format!("view failed for {}: {e}", full.display())),
        };
        // Magic bytes decide, so a mislabeled file (text named .png) fails
        // loudly here instead of sending the provider an undecodable part.
        match crate::images::normalize_image_bytes(&bytes) {
            Ok((mime, out)) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&out);
                ToolOutput::image(format!("Image viewed: {}", full.display()), mime, b64)
            }
            Err(e) => fail(format!("view failed for {}: {e}", full.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn png_bytes() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([12, 34, 56]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    #[tokio::test]
    async fn view_shows_png_as_vision_block() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plot.png"), png_bytes()).unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            ..ToolContext::default()
        };
        let out = ViewTool
            .execute(&ctx, serde_json::json!({"path": "plot.png"}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Image viewed"), "{}", out.content);
        assert_eq!(out.images.len(), 1, "one vision block per call");
        assert_eq!(out.images[0].media_type, "image/png");
        assert!(!out.images[0].data.is_empty(), "base64 payload missing");
    }

    #[tokio::test]
    async fn view_refuses_text_files_with_bash_hint() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            ..ToolContext::default()
        };
        let out = ViewTool
            .execute(&ctx, serde_json::json!({"path": "notes.txt"}))
            .await;
        assert!(out.is_error, "text files must be refused");
        assert!(out.content.contains("not an image file"), "{}", out.content);
        assert!(out.content.contains("bash"), "{}", out.content);
        assert!(out.images.is_empty(), "no vision block on refusal");
    }

    #[tokio::test]
    async fn view_missing_file_is_error_data() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            ..ToolContext::default()
        };
        let out = ViewTool
            .execute(&ctx, serde_json::json!({"path": "nope.png"}))
            .await;
        assert!(out.is_error, "missing files are error data, not panics");
        assert!(out.content.contains("view failed"), "{}", out.content);
    }

    #[tokio::test]
    async fn view_mislabeled_file_fails_on_magic_bytes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fake.png"), "actually text").unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            ..ToolContext::default()
        };
        let out = ViewTool
            .execute(&ctx, serde_json::json!({"path": "fake.png"}))
            .await;
        assert!(out.is_error, "magic bytes must decide over the extension");
        assert!(out.images.is_empty());
    }

    #[tokio::test]
    async fn view_requires_path_argument() {
        let ctx = ToolContext::default();
        let out = ViewTool.execute(&ctx, serde_json::json!({})).await;
        assert!(out.is_error);
        assert!(
            out.content.contains("missing required argument"),
            "{}",
            out.content
        );
    }
}
