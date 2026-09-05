use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};

use crate::edit_diff::{
    Edit, apply_edits_to_normalized_content, detect_line_ending, normalize_to_lf,
    restore_line_endings, split_bom,
};
use crate::{Tool, fail, get_opt_bool, resolve_path};

pub const EDIT_SNIPPET: &str = "Make precise file edits with exact text replacement, including multiple disjoint edits in one call";
pub const EDIT_GUIDELINES: &[&str] = &[
    "Use edit for precise changes (edits[].oldText must match exactly)",
    "When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls",
    "Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
    "Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.",
];

pub struct EditTool;

fn parse_edits(args: &Value) -> Result<Vec<Edit>, String> {
    if let Some(edits_val) = args.get("edits") {
        if let Some(s) = edits_val.as_str() {
            let parsed: Value =
                serde_json::from_str(s).map_err(|e| format!("edits JSON parse failed: {e}"))?;
            return parse_edits_array(&parsed);
        }
        if edits_val.is_object() {
            let e = parse_single_edit(edits_val)?;
            return Ok(vec![e]);
        }
        if edits_val.is_array() {
            return parse_edits_array(edits_val);
        }
        if !edits_val.is_null() {
            return Err("edits must be an array of {oldText, newText}".to_string());
        }
    }
    let old = args
        .get("oldText")
        .or_else(|| args.get("old_text"))
        .or_else(|| args.get("TargetContent"))
        .or_else(|| args.get("target_content"))
        .or_else(|| args.get("targetContent"))
        .or_else(|| args.get("search"))
        .or_else(|| args.get("find"));
    let new = args
        .get("newText")
        .or_else(|| args.get("new_text"))
        .or_else(|| args.get("ReplacementContent"))
        .or_else(|| args.get("replacement_content"))
        .or_else(|| args.get("replacementContent"))
        .or_else(|| args.get("replace"));
    if let (Some(o), Some(n)) = (old, new) {
        if let (Some(os), Some(ns)) = (o.as_str(), n.as_str()) {
            return Ok(vec![Edit {
                old_text: os.to_string(),
                new_text: ns.to_string(),
            }]);
        }
        return Err("oldText/newText must be strings".to_string());
    }
    Err("missing edits (provide edits: [{oldText, newText}] or old_text/new_text)".to_string())
}

fn parse_single_edit(v: &Value) -> Result<Edit, String> {
    let old = v
        .get("oldText")
        .or_else(|| v.get("old_text"))
        .or_else(|| v.get("TargetContent"))
        .or_else(|| v.get("target_content"))
        .or_else(|| v.get("targetContent"))
        .or_else(|| v.get("search"))
        .or_else(|| v.get("find"))
        .and_then(|x| x.as_str())
        .ok_or("edit missing oldText / TargetContent")?;
    let new = v
        .get("newText")
        .or_else(|| v.get("new_text"))
        .or_else(|| v.get("ReplacementContent"))
        .or_else(|| v.get("replacement_content"))
        .or_else(|| v.get("replacementContent"))
        .or_else(|| v.get("replace"))
        .and_then(|x| x.as_str())
        .ok_or("edit missing newText / ReplacementContent")?;
    Ok(Edit {
        old_text: old.to_string(),
        new_text: new.to_string(),
    })
}

fn parse_edits_array(v: &Value) -> Result<Vec<Edit>, String> {
    let arr = v.as_array().ok_or("edits must be an array")?;
    let mut out = Vec::new();
    for item in arr {
        out.push(parse_single_edit(item)?);
    }
    Ok(out)
}

#[async_trait]
impl Tool for EditTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "edit",
            "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to edit (relative or absolute)" },
                    "edits": {
                        "type": "array",
                        "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": { "type": "string", "description": "Exact text for one targeted replacement. Must be unique and non-overlapping." },
                                "newText": { "type": "string", "description": "Replacement text." },
                                "old_text": { "type": "string" },
                                "new_text": { "type": "string" }
                            }
                        }
                    },
                    "old_text": { "type": "string", "description": "Legacy single-edit old text (aliases oldText)" },
                    "new_text": { "type": "string", "description": "Legacy single-edit new text (aliases newText)" },
                    "oldText": { "type": "string" },
                    "newText": { "type": "string" },
                    "replace_all": { "type": "boolean", "description": "Legacy: replace every occurrence (single-edit only)" }
                },
                "required": ["path"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(EDIT_SNIPPET)
    }
    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(EDIT_GUIDELINES)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = args
            .get("path")
            .or_else(|| args.get("file_path"))
            .or_else(|| args.get("filePath"))
            .or_else(|| args.get("TargetFile"))
            .or_else(|| args.get("target_file"))
            .or_else(|| args.get("targetFile"))
            .or_else(|| args.get("file"))
            .or_else(|| args.get("filename"))
            .or_else(|| args.get("target"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if path.is_empty() {
            return fail("missing required argument 'path'".to_string());
        }
        let replace_all = match get_opt_bool(&args, "replace_all") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return e,
        };

        let edits = match parse_edits(&args) {
            Ok(e) => e,
            Err(msg) => return fail(format!("edit failed for {path}: {msg}")),
        };
        if edits.is_empty() {
            return fail(format!(
                "edit failed for {path}: edits must contain at least one replacement"
            ));
        }

        let full = resolve_path(&ctx.cwd, &path);
        if replace_all && edits.len() == 1 {
            let content = match tokio::fs::read_to_string(&full).await {
                Ok(c) => c,
                Err(e) => return fail(format!("edit failed for {}: {e}", full.display())),
            };
            let old = &edits[0].old_text;
            let new = &edits[0].new_text;
            let matches = content.matches(old.as_str()).count();
            if matches == 0 {
                return fail(format!(
                    "edit failed for {}: old_text not found in file",
                    full.display()
                ));
            }
            let updated = content.replace(old.as_str(), new.as_str());
            if let Err(e) = tokio::fs::write(&full, updated.as_bytes()).await {
                return fail(format!("edit failed for {}: {e}", full.display()));
            }
            return ToolOutput::ok(format!(
                "edited {}: {} occurrence(s) replaced",
                full.display(),
                matches
            ));
        }

        let raw = match tokio::fs::read_to_string(&full).await {
            Ok(c) => c,
            Err(e) => return fail(format!("edit failed for {}: {e}", full.display())),
        };
        let bom = split_bom(&raw);
        let ending = detect_line_ending(&bom.text);
        let normalized = normalize_to_lf(&bom.text);
        let applied = match apply_edits_to_normalized_content(&normalized, &edits, &path) {
            Ok(r) => r,
            Err(msg) => return fail(format!("edit failed for {}: {msg}", full.display())),
        };
        let final_content = bom.bom + &restore_line_endings(&applied.new_content, ending);
        if let Err(e) = tokio::fs::write(&full, final_content.as_bytes()).await {
            return fail(format!("edit failed for {}: {e}", full.display()));
        }
        let patch = crate::edit_diff::generate_unified_patch(
            &path,
            &applied.base_content,
            &applied.new_content,
            3,
        );
        if patch.is_empty() {
            ToolOutput::ok(format!(
                "Successfully replaced {} block(s) in {}.",
                edits.len(),
                path
            ))
        } else {
            ToolOutput::ok(patch)
        }
    }
}
