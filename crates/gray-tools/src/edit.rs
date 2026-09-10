use std::sync::Arc;

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};

use crate::edit_diff::{
    EDIT_PREFIX_STRIP_NOTE, Edit, apply_edits_to_normalized_content, detect_line_ending,
    normalize_to_lf, restore_line_endings, split_bom, strip_edit_prefixes,
};
use crate::ledger::{FileLedger, LedgerEntry};
use crate::read::notices;
use crate::{Tool, fail, get_str, resolve_path};

pub const EDIT_SNIPPET: &str = "Make precise file edits with exact text replacement, including multiple disjoint edits in one call";
pub const EDIT_GUIDELINES: &[&str] = &[
    "Use edit for precise changes (edits[].oldText must match exactly)",
    "When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls",
    "Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
    "Keep edits[].oldText as small as possible while still being unique in the file. If multiple occurrences exist, provide line_start, occurrence, or replace_all to disambiguate.",
];

pub struct EditTool {
    ledger: Arc<FileLedger>,
}

impl EditTool {
    /// Share `ledger` (the registry/plugin wiring); [`Default`] keeps a
    /// private ledger so existing tests compile.
    pub fn new(ledger: Arc<FileLedger>) -> Self {
        Self { ledger }
    }

    /// T3.2: only the staleness rule applies to edit (it reads the file to
    /// match, so unread/partial needs no guard).
    pub(crate) fn is_stale(entry: &LedgerEntry, meta: &std::fs::Metadata) -> bool {
        meta.modified().is_ok_and(|t| t != entry.mtime) || meta.len() != entry.size
    }
}

impl Default for EditTool {
    fn default() -> Self {
        Self {
            ledger: Arc::new(FileLedger::new()),
        }
    }
}

fn parse_line_hint(v: &Value) -> Option<usize> {
    let val = v
        .get("line_start")
        .or_else(|| v.get("lineStart"))
        .or_else(|| v.get("start_line"))
        .or_else(|| v.get("StartLine"))
        .or_else(|| v.get("startLine"))
        .or_else(|| v.get("line"))
        .or_else(|| v.get("line_number"))
        .or_else(|| v.get("lineNumber"))
        .or_else(|| v.get("lineHint"))
        .or_else(|| v.get("line_hint"))?;

    if let Some(n) = val.as_u64() {
        return Some(n as usize);
    }
    if let Some(n) = val.as_i64()
        && n > 0
    {
        return Some(n as usize);
    }
    if let Some(s) = val.as_str()
        && let Ok(n) = s.trim().parse::<usize>()
    {
        return Some(n);
    }
    None
}

fn parse_occurrence(v: &Value) -> Option<isize> {
    let val = v
        .get("occurrence")
        .or_else(|| v.get("occurrence_index"))
        .or_else(|| v.get("occurrenceIndex"))
        .or_else(|| v.get("nth"))
        .or_else(|| v.get("index"))
        .or_else(|| v.get("match_index"))
        .or_else(|| v.get("matchIndex"))?;

    if let Some(n) = val.as_i64() {
        return Some(n as isize);
    }
    if let Some(s) = val.as_str() {
        let trimmed = s.trim().to_lowercase();
        if trimmed == "first" {
            return Some(1);
        }
        if trimmed == "last" {
            return Some(-1);
        }
        if let Ok(n) = trimmed.parse::<isize>() {
            return Some(n);
        }
    }
    None
}

fn parse_replace_all(v: &Value) -> Option<bool> {
    let val = v
        .get("replace_all")
        .or_else(|| v.get("replaceAll"))
        .or_else(|| v.get("all"))
        .or_else(|| v.get("allow_multiple"))
        .or_else(|| v.get("allowMultiple"))
        .or_else(|| v.get("AllowMultiple"))
        .or_else(|| v.get("multiple"))?;

    if let Some(b) = val.as_bool() {
        return Some(b);
    }
    if let Some(s) = val.as_str() {
        let trimmed = s.trim().to_lowercase();
        if trimmed == "true" || trimmed == "1" || trimmed == "all" {
            return Some(true);
        }
        if trimmed == "false" || trimmed == "0" {
            return Some(false);
        }
    }
    None
}

fn parse_edits(args: &Value) -> Result<Vec<Edit>, String> {
    let top_line_hint = parse_line_hint(args);
    let top_occurrence = parse_occurrence(args);
    let top_replace_all = parse_replace_all(args);

    let mut edits = if let Some(edits_val) = args.get("edits") {
        if let Some(s) = edits_val.as_str() {
            let parsed: Value =
                serde_json::from_str(s).map_err(|e| format!("edits JSON parse failed: {e}"))?;
            parse_edits_array(&parsed)?
        } else if edits_val.is_object() {
            vec![parse_single_edit(edits_val)?]
        } else if edits_val.is_array() {
            parse_edits_array(edits_val)?
        } else if !edits_val.is_null() {
            return Err("edits must be an array of {oldText, newText}".to_string());
        } else {
            parse_single_or_legacy(args)?
        }
    } else {
        parse_single_or_legacy(args)?
    };

    for e in edits.iter_mut() {
        if e.line_hint.is_none() {
            e.line_hint = top_line_hint;
        }
        if e.occurrence.is_none() {
            e.occurrence = top_occurrence;
        }
        if e.replace_all.is_none() {
            e.replace_all = top_replace_all;
        }
    }

    Ok(edits)
}

/// Top-level single-edit form (`oldText`/`newText`; legacy names are
/// renamed to these by the central ALIASES table in `crate`/coerce_args).
fn parse_single_or_legacy(args: &Value) -> Result<Vec<Edit>, String> {
    let old = args.get("oldText");
    let new = args.get("newText");
    if let (Some(o), Some(n)) = (old, new) {
        if let (Some(os), Some(ns)) = (o.as_str(), n.as_str()) {
            return Ok(vec![Edit {
                old_text: os.to_string(),
                new_text: ns.to_string(),
                line_hint: parse_line_hint(args),
                occurrence: parse_occurrence(args),
                replace_all: parse_replace_all(args),
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
    let line_hint = parse_line_hint(v);
    let occurrence = parse_occurrence(v);
    let replace_all = parse_replace_all(v);

    Ok(Edit {
        old_text: old.to_string(),
        new_text: new.to_string(),
        line_hint,
        occurrence,
        replace_all,
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
            "Edit a single file using exact text replacement. Every edits[].oldText matches a region in the original file. If multiple occurrences exist, provide line_start, occurrence, or replace_all to disambiguate.",
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
                                "oldText": { "type": "string", "description": "Exact text for one targeted replacement." },
                                "newText": { "type": "string", "description": "Replacement text." },
                                "old_text": { "type": "string" },
                                "new_text": { "type": "string" },
                                "line_start": { "type": "integer", "description": "Optional 1-based line number hint to disambiguate multiple occurrences." },
                                "occurrence": { "type": "integer", "description": "Optional 1-based occurrence index to target (e.g. 1 for first match, -1 for last)." },
                                "replace_all": { "type": "boolean", "description": "Replace every occurrence of oldText." }
                            }
                        }
                    },
                    "oldText": { "type": "string" },
                    "newText": { "type": "string" },
                    "line_start": { "type": "integer", "description": "Optional 1-based line number hint to disambiguate multiple occurrences." },
                    "occurrence": { "type": "integer", "description": "Optional 1-based occurrence index to target (e.g. 1 for first match, -1 for last)." },
                    "replace_all": { "type": "boolean", "description": "Replace every occurrence of oldText." }
                },
                "required": ["path"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(EDIT_SNIPPET)
    }
    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(EDIT_GUIDELINES)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        // Legacy path names (file_path/TargetFile/…) are renamed by the
        // ALIASES table in `crate` (coerce_args) before lookup.
        let path = match get_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        if path.is_empty() {
            return fail("missing required argument 'path'".to_string());
        }

        let edits = match parse_edits(&args) {
            Ok(e) => e,
            Err(msg) => return fail(format!("edit failed for {path}: {msg}")),
        };
        if edits.is_empty() {
            return fail(format!(
                "edit failed for {path}: edits must contain at least one replacement"
            ));
        }
        // Explicitly reject empty searches before any I/O: an empty oldText
        // would match everywhere (single fast path or common engine alike).
        for (i, e) in edits.iter().enumerate() {
            if e.old_text.is_empty() {
                let detail = if edits.len() == 1 {
                    format!("oldText must not be empty in {path}.")
                } else {
                    format!("edits[{i}].oldText must not be empty in {path}.")
                };
                return fail(format!("edit failed for {path}: {detail}"));
            }
        }

        let full = resolve_path(&ctx.cwd, &path);
        // T3.2 ledger: refuse only when the file changed since it was read
        // (mtime/size here; byte comparison after the read below).
        if let Some(entry) = self.ledger.get(&full)
            && let Ok(meta) = std::fs::metadata(&full)
            && Self::is_stale(&entry, &meta)
        {
            return fail(notices::edit_changed(&full.display().to_string()));
        }
        // Validate regular files before reading (metadata follows symlinks).
        if let Ok(meta) = std::fs::metadata(&full)
            && !meta.file_type().is_file()
        {
            return fail(format!(
                "edit failed for {}: not a regular file",
                full.display()
            ));
        }

        // Distinguish NotFound (clear miss) from other read errors: neither
        // becomes an empty string.
        let raw = match tokio::fs::read_to_string(&full).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return fail(format!(
                    "edit failed for {}: file not found",
                    full.display()
                ));
            }
            Err(e) => return fail(format!("edit failed for {}: {e}", full.display())),
        };
        // Freshness via bytes when already read: same mtime/size but different
        // bytes still counts as changed.
        if let Some(entry) = self.ledger.get(&full)
            && let (Some(expected), Some(actual)) =
                (entry.content_hash, FileLedger::hash_bytes(raw.as_bytes()))
            && expected != actual
            && let Ok(meta) = std::fs::metadata(&full)
            && !Self::is_stale(&entry, &meta)
        {
            return fail(notices::edit_changed(&full.display().to_string()));
        }
        let bom = split_bom(&raw);
        let ending = detect_line_ending(&bom.text);
        let normalized = normalize_to_lf(&bom.text);
        // T1.6: exact (then existing fuzzy) first; only on failure retry once
        // with cat -n prefixes stripped from oldText/newText together.
        let (applied, repaired) =
            match apply_edits_to_normalized_content(&normalized, &edits, &path) {
                Ok(r) => (r, false),
                Err(msg) => match strip_edit_prefixes(&edits)
                    .and_then(|s| apply_edits_to_normalized_content(&normalized, &s, &path).ok())
                {
                    Some(r) => (r, true),
                    None => return fail(format!("edit failed for {}: {msg}", full.display())),
                },
            };
        let final_content = bom.bom + &restore_line_endings(&applied.new_content, ending);
        // Atomic replace (temp + rename, mode preserved); see
        // crate::write::atomic_write for symlink/hardlink semantics.
        if let Err(e) = crate::write::atomic_write(&full, final_content.as_bytes()).await {
            return fail(format!("edit failed for {}: {e}", full.display()));
        }
        // T3.2 ledger: the whole new content is known — the next write is
        // allowed without a re-read.
        self.ledger.mark_written(&full, final_content.as_bytes());
        let patch = crate::edit_diff::generate_unified_patch(
            &path,
            &applied.base_content,
            &applied.new_content,
            3,
        );
        let mut all_notes = applied.notes;
        if repaired {
            all_notes.push(EDIT_PREFIX_STRIP_NOTE.to_string());
        }
        let notes_str = if all_notes.is_empty() {
            String::new()
        } else {
            format!("\n{}", all_notes.join("\n"))
        };
        if patch.is_empty() {
            ToolOutput::ok(format!(
                "Successfully replaced {} block(s) in {}.{notes_str}",
                edits.len(),
                path
            ))
        } else {
            ToolOutput::ok(format!("{patch}{notes_str}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_only_when_mtime_or_size_drift() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"one\ntwo\n").unwrap();
        let meta = std::fs::metadata(&p).unwrap();
        let entry = LedgerEntry {
            mtime: meta.modified().unwrap(),
            size: meta.len(),
            content_hash: FileLedger::hash_bytes(b"one\ntwo\n"),
            full_view: true,
            window: (1, None),
            first_line: 1,
            last_line: 2,
            dedup_armed: true,
            read_at: std::time::Instant::now(),
        };
        assert!(!EditTool::is_stale(&entry, &meta));
        std::fs::write(&p, b"one\ntwo\nthree\n").unwrap();
        let grown = std::fs::metadata(&p).unwrap();
        assert!(EditTool::is_stale(&entry, &grown));
    }
}
