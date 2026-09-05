//! T5.3 — notebook unit: `.ipynb` renders as tagged cells over the T5.2 seam.
//!
//! Pure functions + `serde_json` only (already a gray-tools dep, no new deps).
//! UNWIRED: no `read/mod.rs` behavior change except `pub mod notebook;` (this
//! task's allowed wiring). `ReadTool::execute` does NOT call this yet — the
//! integrator wires it after `hygiene::prepare` returns window-ready text, and
//! windowing (offset/limit → clamp → prefixes → caps) then applies to the
//! rendered text like any file.
//!
//! ```ignore
//! pub mod notebook;
//! // in ReadTool::execute, after hygiene::prepare gives `text`:
//! // if let Some(rendered) = notebook::render(&text, &display) {
//! //     /* window `rendered` through the normal pipeline instead of `text` */
//! //     /* image payloads: decode each notebook::images(&value) base64 with
//! //        the vision decoder, then image::scale_note(&image::plan(m, w, h))
//! //        + Attachment; vision off → image::not_attached_note(w, h) */
//! // }
//! ```
//!
//! Spec: plan.ts T5.3. Detect by JSON shape (`nbformat` key), never the
//! extension. `source` arrays concatenate (elements already carry newlines).
//! Per-output `--- output <n> ---` sections (the index plugs straight into the
//! jq pointer); cell indices are array indices for the same reason. One
//! output > 10,000 chars becomes the plan-exact jq pointer. `image/png`
//! outputs never dump base64 — a one-line stub marks the spot and
//! [`images`] carries the payloads for the T5.2 attach path.
//!
//! Contract strings live HERE until `notices.rs` (T1.3 owner) moves them
//! verbatim (one owner per string — same staging as `bulk.rs`/`image.rs`).
//!
//! FOLLOW-UPS (not done here — files outside T5.3 ownership):
//! 1. `read/mod.rs`: call [`render`] after hygiene, window the result; parse
//!    once via [`render_value`] + [`images`] when images are needed.
//! 2. Pixel ops behind the plan's `vision` feature (`image` crate): base64
//!    decode each [`NotebookImage`], decode dims, downscale via
//!    `image::scaled_dims`, JPEG ladder via `image::JPEG_QUALITIES`.
//! 3. The jq pointer names `.text`; `data`-only outputs keep their text under
//!    `.data["text/plain"]` — a smarter per-kind filter is the integrator's
//!    call (the card pins this shape; reviewer decides).
//!
//! // ponytail: render + payload scan only, no `image`/`base64` crates —
//! // covers the card (tagged cells, jq pointers, vision seam) with zero new
//! // deps. The decoder/encoder lands with the gray-core Attachment type.

use serde_json::Value;

/// A single output is replaced by its jq pointer past this many chars.
pub const OUTPUT_CHARS_MAX: usize = 10_000;

/// An `image/png` output payload for the T5.2 attach path. `cell`/`output`
/// are the same array indices the rendered text and jq pointers use.
pub struct NotebookImage {
    /// Index into `.cells`.
    pub cell: usize,
    /// Index into `.cells[cell].outputs`.
    pub output: usize,
    /// Raw base64 payload (`data["image/png"]`, string or array joined).
    pub base64: String,
}

/// Shape check: parsed JSON with an `nbformat` key. The filename/extension is
/// never consulted — callers pass text only, so `fake.ipynb` holding prose
/// falls through and `notes.txt` holding a notebook renders.
pub fn is_notebook(value: &Value) -> bool {
    value.get("nbformat").is_some()
}

/// Join a Jupyter multiline string: array elements concatenate (they already
/// carry their newlines — NOT `join("\n")`), a bare string passes through,
/// anything else is empty.
pub fn join_source(source: &Value) -> String {
    str_or_joined(source).unwrap_or_default()
}

/// Text of one output, if it has any: stream `.text`, then
/// `data["text/plain"]` (either as string or array), then `ename: evalue` +
/// traceback for error outputs. Image-only / unknown outputs are `None` so
/// the caller can skip them or route to the image stub.
pub fn output_text(out: &Value) -> Option<String> {
    if out.get("output_type").and_then(Value::as_str) == Some("error") {
        let mut s = String::new();
        match (
            out.get("ename").and_then(Value::as_str),
            out.get("evalue").and_then(Value::as_str),
        ) {
            (Some(e), Some(v)) => s.push_str(&format!("{e}: {v}")),
            (None, Some(v)) => s.push_str(v),
            _ => {}
        }
        if let Some(tb) = out.get("traceback").and_then(str_or_joined)
            && !tb.is_empty()
        {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&tb);
        }
        return (!s.is_empty()).then_some(s);
    }
    if let Some(t) = out.get("text").and_then(str_or_joined) {
        return Some(t);
    }
    out.get("data")
        .and_then(|d| d.get("text/plain"))
        .and_then(str_or_joined)
}

/// Base64 of an `image/png` output, if present (string or array joined).
pub fn png_base64(out: &Value) -> Option<String> {
    out.get("data")?.get("image/png").and_then(str_or_joined)
}

/// Every `image/png` payload in document order, for the T5.2 attach path
/// (decode → dims → `image::plan`/`image::scale_note` + Attachment).
pub fn images(value: &Value) -> Vec<NotebookImage> {
    let mut out = Vec::new();
    let cells = match value.get("cells").and_then(Value::as_array) {
        Some(c) => c,
        None => return out,
    };
    for (i, cell) in cells.iter().enumerate() {
        let outputs = match cell.get("outputs").and_then(Value::as_array) {
            Some(o) => o,
            None => continue,
        };
        for (n, item) in outputs.iter().enumerate() {
            if let Some(base64) = png_base64(item) {
                out.push(NotebookImage {
                    cell: i,
                    output: n,
                    base64,
                });
            }
        }
    }
    out
}

/// `--- cell <i> [<type>] ---` — `<i>` is the `.cells` array index so it
/// plugs straight into the jq pointer below.
pub fn cell_header(index: usize, cell_type: &str) -> String {
    format!("--- cell {index} [{cell_type}] ---")
}

/// Huge-output pointer — plan-exact wording (fact, `is_error=false`).
pub fn output_too_big_note(cell: usize, output: usize, chars: usize, display: &str) -> String {
    format!(
        "[output {output} is {chars} chars; view with: \
         jq -r '.cells[{cell}].outputs[{output}].text | join(\"\")' {display}]"
    )
}

/// Image-output stub (staged; `notices.rs` owner moves it verbatim). Marks
/// position + payload size; the pixels travel via the T5.2 attach path, never
/// as base64 in the text.
pub fn image_stub_note(cell: usize, output: usize, b64_chars: usize) -> String {
    format!("[cell {cell} output {output}: image/png ({b64_chars} chars base64, attached as vision image)]")
}

/// Render parsed notebook JSON to window-ready text. `None` = not a notebook
/// (or no renderable cells — e.g. `cells` missing/not an array, zero blocks),
/// so the caller falls back to the raw-text path; that also keeps degenerate
/// inputs from ever producing an empty `ok("")`.
pub fn render_value(value: &Value, display: &str) -> Option<String> {
    if !is_notebook(value) {
        return None;
    }
    let cells = value.get("cells")?.as_array()?;
    let mut blocks: Vec<String> = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        let ctype = cell.get("cell_type").and_then(Value::as_str).unwrap_or("unknown");
        blocks.push(cell_header(i, ctype));
        let source = cell.get("source").map(join_source).unwrap_or_default();
        if !source.is_empty() {
            blocks.push(source);
        }
        let outputs = cell
            .get("outputs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (n, out) in outputs.iter().enumerate() {
            let mut body: Vec<String> = Vec::new();
            if let Some(t) = output_text(out).filter(|t| !t.is_empty()) {
                let chars = t.chars().count();
                if chars > OUTPUT_CHARS_MAX {
                    body.push(output_too_big_note(i, n, chars, display));
                } else {
                    body.push(t);
                }
            }
            if let Some(b64) = png_base64(out) {
                body.push(image_stub_note(i, n, b64.chars().count()));
            }
            if !body.is_empty() {
                blocks.push(format!("--- output {n} ---"));
                blocks.extend(body);
            }
        }
    }
    (!blocks.is_empty()).then(|| blocks.join("\n"))
}

/// Parse-once wrapper over [`render_value`]: invalid JSON or non-notebook
/// JSON is `None` (caller keeps the raw-text path).
pub fn render(text: &str, display: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    render_value(&value, display)
}

/// String-or-array-of-strings joiner (Jupyter multiline shape). Non-string
/// array items are skipped; other types are `None`.
fn str_or_joined(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let mut s = String::new();
            for item in items {
                if let Some(piece) = item.as_str() {
                    s.push_str(piece);
                }
            }
            Some(s)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn small() -> Value {
        json!({
            "nbformat": 4,
            "nbformat_minor": 5,
            "metadata": {},
            "cells": [
                {
                    "cell_type": "code",
                    "source": ["print(\"hi\")\n", "print(1 + 1)\n"],
                    "outputs": [
                        {"output_type": "stream", "name": "stdout",
                         "text": ["hi\n", "2\n"]}
                    ]
                },
                {"cell_type": "markdown", "source": "# Title\n\nSome *words*."}
            ]
        })
    }

    #[test]
    fn non_notebooks_fall_through_to_none() {
        assert!(render("just some prose\n", "notes.txt").is_none());
        assert!(render("{ not json", "bad.ipynb").is_none());
        assert!(render("[1, 2, 3]", "arr.json").is_none());
        assert!(render(r#"{"cells": []}"#, "n.ipynb").is_none());
        assert!(!is_notebook(&json!({"cells": []})));
        assert!(!is_notebook(&json!([1])));
    }

    #[test]
    fn detection_uses_shape_not_extension() {
        let raw = serde_json::to_string(&small()).unwrap();
        // Notebook shaped JSON renders whatever the filename claims …
        assert!(render(&raw, "notes.txt").is_some());
        assert!(render(&raw, "weird.csv").is_some());
        // … and prose wearing .ipynb does not.
        assert!(render("print(1)\n", "fake.ipynb").is_none());
    }

    #[test]
    fn small_notebook_golden() {
        let raw = serde_json::to_string(&small()).unwrap();
        assert_eq!(
            render(&raw, "nb.ipynb").as_deref(),
            Some(
                "--- cell 0 [code] ---\n\
                 print(\"hi\")\n\
                 print(1 + 1)\n\
                 \n\
                 --- output 0 ---\n\
                 hi\n\
                 2\n\
                 \n\
                 --- cell 1 [markdown] ---\n\
                 # Title\n\
                 \n\
                 Some *words*."
            )
        );
    }

    #[test]
    fn source_arrays_concatenate_without_extra_newlines() {
        assert_eq!(join_source(&json!(["a\n", "b"])), "a\nb");
        assert_eq!(join_source(&json!(["a\n", "b\n"])), "a\nb\n");
        assert_eq!(join_source(&json!("bare")), "bare");
        assert_eq!(join_source(&json!(null)), "");
        assert_eq!(join_source(&json!([1, "x"])), "x");
    }

    #[test]
    fn big_output_becomes_jq_pointer_and_stays_small() {
        let dump = "x".repeat(60_000);
        let nb = json!({
            "nbformat": 4,
            "cells": [{
                "cell_type": "code",
                "source": ["df.head()\n"],
                "outputs": [
                    {"output_type": "stream", "name": "stdout", "text": [dump]}
                ]
            }]
        });
        let raw = serde_json::to_string(&nb).unwrap();
        let rendered = render(&raw, "df.ipynb").unwrap();
        assert!(
            rendered.contains(
                "[output 0 is 60000 chars; view with: \
                 jq -r '.cells[0].outputs[0].text | join(\"\")' df.ipynb]"
            ),
            "{rendered}"
        );
        assert!(!rendered.contains(&dump), "raw dump must not leak");
        // Accept: ≥5× smaller than raw JSON, total < 20 KiB.
        assert!(raw.len() >= 5 * rendered.len(), "{} vs {}", raw.len(), rendered.len());
        assert!(rendered.len() < 20 * 1024, "{}", rendered.len());
    }

    #[test]
    fn output_boundary_is_ten_thousand_chars() {
        let cell_with = |text: String| {
            json!({
                "nbformat": 4,
                "cells": [{
                    "cell_type": "code",
                    "source": ["x\n"],
                    "outputs": [
                        {"output_type": "stream", "name": "stdout", "text": [text]}
                    ]
                }]
            })
        };
        let exact: String = "y".repeat(OUTPUT_CHARS_MAX);
        assert!(render(&serde_json::to_string(&cell_with(exact.clone())).unwrap(), "b.ipynb")
            .unwrap()
            .contains(&exact));
        let over: String = "y".repeat(OUTPUT_CHARS_MAX + 1);
        let rendered =
            render(&serde_json::to_string(&cell_with(over)).unwrap(), "b.ipynb").unwrap();
        assert!(rendered.contains(&format!("[output 0 is {} chars;", OUTPUT_CHARS_MAX + 1)), "{rendered}");
    }

    #[test]
    fn image_png_output_uses_the_vision_seam() {
        let nb = json!({
            "nbformat": 4,
            "cells": [{
                "cell_type": "code",
                "source": ["plot()\n"],
                "outputs": [{
                    "output_type": "display_data",
                    "data": {
                        "text/plain": ["<Figure>"],
                        "image/png": ["aGVsbG8=", "d29ybGQ="]
                    }
                }]
            }]
        });
        let rendered = render_value(&nb, "plot.ipynb").unwrap();
        assert!(rendered.contains("--- output 0 ---"), "{rendered}");
        assert!(rendered.contains("<Figure>"), "{rendered}");
        assert!(
            rendered.contains("[cell 0 output 0: image/png (16 chars base64, attached as vision image)]"),
            "{rendered}"
        );
        assert!(!rendered.contains("aGVsbG8"), "base64 must not leak");
        // Payload scan feeds the T5.2 attach path (dims come from the vision
        // decoder follow-up; the note shape is pinned here).
        let imgs = images(&nb);
        assert_eq!(imgs.len(), 1);
        assert_eq!((imgs[0].cell, imgs[0].output), (0, 0));
        assert_eq!(imgs[0].base64, "aGVsbG8=d29ybGQ=");
        let p = crate::read::image::plan("image/png", 800, 600);
        assert_eq!(
            crate::read::image::scale_note(&p),
            "[read: image 800x600 on disk, attached at 800x600. \
              Multiply coordinates you compute from the image by 1.00.]"
        );
    }

    #[test]
    fn error_output_shows_ename_and_traceback() {
        let nb = json!({
            "nbformat": 4,
            "cells": [{
                "cell_type": "code",
                "source": ["1/0\n"],
                "outputs": [{
                    "output_type": "error",
                    "ename": "ZeroDivisionError",
                    "evalue": "division by zero",
                    "traceback": ["line 1\n"]
                }]
            }]
        });
        let rendered = render_value(&nb, "e.ipynb").unwrap();
        assert!(rendered.contains("ZeroDivisionError: division by zero"), "{rendered}");
        assert!(rendered.contains("line 1"), "{rendered}");
    }

    #[test]
    fn missing_keys_and_unknown_outputs_never_panic() {
        let nb = json!({
            "nbformat": 4,
            "cells": [
                {"source": ["x = 1\n"]},
                {"cell_type": "code"},
                {"cell_type": "code", "source": [],
                 "outputs": [{"output_type": "display_data",
                              "data": {"text/html": ["<b>x</b>"]}}]},
                {"cell_type": "raw", "source": "raw text"}
            ]
        });
        let rendered = render_value(&nb, "m.ipynb").unwrap();
        // Typeless cell is honest, never silently "code".
        assert!(rendered.contains("--- cell 0 [unknown] ---"), "{rendered}");
        assert!(rendered.contains("--- cell 3 [raw] ---"), "{rendered}");
        // html-only output leaves no empty section behind.
        assert!(!rendered.contains("--- output"), "{rendered}");
        // Degenerate shapes fall back to raw text, never empty ok("").
        assert!(render_value(&json!({"nbformat": 4}), "d.ipynb").is_none());
        assert!(render_value(&json!({"nbformat": 4, "cells": {}}), "d.ipynb").is_none());
        assert!(images(&json!({"nbformat": 4})).is_empty());
    }

    #[test]
    fn text_plain_data_arrays_join() {
        let out = json!({
            "output_type": "execute_result",
            "data": {"text/plain": ["a", "b\n", "c"]}
        });
        assert_eq!(output_text(&out).as_deref(), Some("ab\nc"));
        assert!(png_base64(&out).is_none());
        assert_eq!(output_text(&json!({"output_type": "stream"})), None);
    }
}
