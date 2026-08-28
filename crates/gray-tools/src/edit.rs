use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{json, Value};

use crate::edit_diff::{
    apply_edits_to_normalized_content, detect_line_ending,
    normalize_to_lf, restore_line_endings, split_bom, Edit,
};
use crate::file_mutation_queue::with_file_mutation_queue;
use crate::{fail, get_opt_bool, get_str, resolve_path, Tool};

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
            let parsed: Value = serde_json::from_str(s).map_err(|e| format!("edits JSON parse failed: {e}"))?;
            return parse_edits_array(&parsed);
        }
        if edits_val.is_object() && edits_val.get("oldText").is_some() || edits_val.get("old_text").is_some() {
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
    let old = args.get("oldText").or_else(|| args.get("old_text")).or_else(|| args.get("oldText"));
    let new = args.get("newText").or_else(|| args.get("new_text")).or_else(|| args.get("newText"));
    if let (Some(o), Some(n)) = (old, new) {
        if let (Some(os), Some(ns)) = (o.as_str(), n.as_str()) {
            return Ok(vec![Edit { old_text: os.to_string(), new_text: ns.to_string() }]);
        }
        return Err("oldText/newText must be strings".to_string());
    }
    Err("missing edits (provide edits: [{oldText, newText}] or old_text/new_text)".to_string())
}

fn parse_single_edit(v: &Value) -> Result<Edit, String> {
    let old = v.get("oldText").or_else(|| v.get("old_text")).and_then(|x| x.as_str()).ok_or("edit missing oldText")?;
    let new = v.get("newText").or_else(|| v.get("new_text")).and_then(|x| x.as_str()).ok_or("edit missing newText")?;
    Ok(Edit { old_text: old.to_string(), new_text: new.to_string() })
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

    fn prompt_snippet(&self) -> Option<&'static str> { Some(EDIT_SNIPPET) }
    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> { Some(EDIT_GUIDELINES) }
    fn is_concurrency_safe(&self, _args: &Value) -> bool { false }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = match get_str(&args, "path") { Ok(p) => p, Err(e) => return e };
        let replace_all = match get_opt_bool(&args, "replace_all") { Ok(v) => v.unwrap_or(false), Err(e) => return e };

        let edits = match parse_edits(&args) {
            Ok(e) => e,
            Err(msg) => return fail(format!("edit failed for {path}: {msg}")),
        };
        if edits.is_empty() {
            return fail(format!("edit failed for {path}: edits must contain at least one replacement"));
        }

        let full = resolve_path(&ctx.cwd, &path);
        let full_for_queue = full.clone();
        let path_clone = path.clone();
        with_file_mutation_queue(full_for_queue, move || {
            let full = full.clone();
            let edits = edits.clone();
            let path_clone = path_clone.clone();
            async move {
                if replace_all && edits.len() == 1 {
                    let content = match tokio::fs::read_to_string(&full).await {
                        Ok(c) => c, Err(e) => return fail(format!("edit failed for {}: {e}", full.display())),
                    };
                    let old = &edits[0].old_text;
                    let new = &edits[0].new_text;
                    let matches = content.matches(old.as_str()).count();
                    if matches == 0 {
                        return fail(format!("edit failed for {}: old_text not found in file", full.display()));
                    }
                    let updated = content.replace(old.as_str(), new.as_str());
                    if let Err(e) = tokio::fs::write(&full, updated.as_bytes()).await {
                        return fail(format!("edit failed for {}: {e}", full.display()));
                    }
                    return ToolOutput::ok(format!("edited {}: {} occurrence(s) replaced", full.display(), matches));
                }

                let raw = match tokio::fs::read_to_string(&full).await {
                    Ok(c) => c, Err(e) => return fail(format!("edit failed for {}: {e}", full.display())),
                };
                let bom = split_bom(&raw);
                let ending = detect_line_ending(&bom.text);
                let normalized = normalize_to_lf(&bom.text);
                let applied = match apply_edits_to_normalized_content(&normalized, &edits, &path_clone) {
                    Ok(r) => r, Err(msg) => return fail(format!("edit failed for {}: {msg}", full.display())),
                };
                let final_content = bom.bom + &restore_line_endings(&applied.new_content, ending);
                if let Err(e) = tokio::fs::write(&full, final_content.as_bytes()).await {
                    return fail(format!("edit failed for {}: {e}", full.display()));
                }
                let patch = crate::edit_diff::generate_unified_patch(&path_clone, &applied.base_content, &applied.new_content, 3);
                if patch.is_empty() {
                    ToolOutput::ok(format!("Successfully replaced {} block(s) in {}.", edits.len(), path_clone))
                } else {
                    ToolOutput::ok(patch)
                }
            }
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ctx(dir: &std::path::Path) -> ToolContext { ToolContext { cwd: dir.to_path_buf(), cancel: Default::default() } }
    fn edit_args(path: &str, old: &str, new: &str) -> Value { json!({"path": path, "old_text": old, "new_text": new}) }

    #[tokio::test]
    async fn replaces_first_occurrence_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "foo bar baz").unwrap();
        let out = EditTool.execute(&ctx(dir.path()), edit_args("a.txt", "foo", "qux")).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "qux bar baz");
    }
    #[tokio::test]
    async fn zero_matches_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
        let out = EditTool.execute(&ctx(dir.path()), edit_args("a.txt", "absent", "x")).await;
        assert!(out.is_error);
        assert!(out.content.contains("Could not find") || out.content.contains("not found"), "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "hello world");
    }
    #[tokio::test]
    async fn ambiguous_match_without_flag_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "dup dup dup").unwrap();
        let out = EditTool.execute(&ctx(dir.path()), edit_args("a.txt", "dup", "x")).await;
        assert!(out.is_error);
        assert!(out.content.contains("occurrences"), "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "dup dup dup");
    }
    #[tokio::test]
    async fn replace_all_replaces_every_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "dup dup dup").unwrap();
        let out = EditTool.execute(&ctx(dir.path()), json!({"path": "a.txt", "old_text": "dup", "new_text": "x", "replace_all": true})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "x x x");
    }
    #[tokio::test]
    async fn multi_edit_non_overlapping() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a=1\nb=2\nc=3\n").unwrap();
        let out = EditTool.execute(&ctx(dir.path()), json!({"path": "a.txt", "edits": [{"oldText": "a=1", "newText": "a=10"}, {"oldText": "c=3", "newText": "c=30"}]})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "a=10\nb=2\nc=30\n");
    }
    #[tokio::test]
    async fn missing_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = EditTool.execute(&ctx(dir.path()), edit_args("nope.txt", "a", "b")).await;
        assert!(out.is_error);
    }
    #[tokio::test]
    async fn empty_new_text_deletes_the_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "keep drop keep").unwrap();
        let out = EditTool.execute(&ctx(dir.path()), edit_args("a.txt", "drop ", "")).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "keep keep");
    }
    #[test]
    fn edit_is_never_concurrency_safe() { assert!(!EditTool.is_concurrency_safe(&json!({}))); }
}
