//! Port of `pi/packages/coding-agent/src/core/tools/edit-diff.ts`.
//!
//! Provides BOM handling, line-ending normalization, fuzzy matching helpers,
//! multi-edit application with non-overlapping/unique validation, and diff
//! generation. Uses a simple LCS-based line diff (no external `similar` crate
//! needed) — swap in `similar` if it is later added to `Cargo.toml`.

use std::ops::Range;

// ---------------------------------------------------------------------------
// BOM / line endings
// ---------------------------------------------------------------------------

/// Result of [`split_bom`].
pub struct SplitBom {
    pub bom: String,
    pub text: String,
}

/// Split a leading UTF-8 BOM (`\u{FEFF}`) from `content`.
///
/// Models never include the invisible BOM in `oldText`, so matching must be
/// done on the BOM-stripped text and the BOM re-prepended on write.
pub fn split_bom(content: &str) -> SplitBom {
    if let Some(stripped) = content.strip_prefix('\u{FEFF}') {
        SplitBom {
            bom: "\u{FEFF}".to_string(),
            text: stripped.to_string(),
        }
    } else {
        SplitBom {
            bom: String::new(),
            text: content.to_string(),
        }
    }
}

/// Detect the dominant line ending in `content`.
///
/// Mirrors `detectLineEnding` in `edit-diff.ts`: if the first `\r\n` appears
/// before the first `\n`, treat the file as CRLF, otherwise LF. Files with no
/// newlines default to LF.
pub fn detect_line_ending(content: &str) -> &'static str {
    let crlf_idx = content.find("\r\n");
    let lf_idx = content.find('\n');
    match (crlf_idx, lf_idx) {
        (None, None) | (None, Some(_)) => "\n",
        (Some(_), None) => "\n",
        (Some(c), Some(l)) => {
            if c < l {
                "\r\n"
            } else {
                "\n"
            }
        }
    }
}

/// Normalize all line endings to LF (`\n`).
pub fn normalize_to_lf(text: &str) -> String {
    // Cheap path: if no '\r', nothing to do.
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Restore `ending` (`\n` or `\r\n`) in `text` (which is assumed LF-normalized).
pub fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

// ---------------------------------------------------------------------------
// Fuzzy helpers (mirrors normalizeForFuzzyMatch)
// ---------------------------------------------------------------------------

fn normalize_for_fuzzy_match(text: &str) -> String {
    // Trim trailing whitespace per line, then normalize smart quotes/dashes/spaces.
    // NFKC is omitted (requires external crate); the remaining transforms cover
    // the cases models actually emit.
    let trimmed: String = text
        .split('\n')
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        let mapped = match ch {
            // Smart single quotes → '
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            // Smart double quotes → "
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            // Various dashes/hyphens → -
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            // Special spaces → regular space
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        };
        out.push(mapped);
    }
    out
}

// ---------------------------------------------------------------------------
// Edit types
// ---------------------------------------------------------------------------

/// Single replacement, mirroring `Edit` in `edit-diff.ts`.
#[derive(Debug, Clone)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

/// Result of [`apply_edits_to_normalized_content`].
#[derive(Debug, Clone)]
pub struct AppliedEditsResult {
    pub base_content: String,
    pub new_content: String,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn split_lines_with_endings(content: &str) -> Vec<String> {
    // Mirrors `content.match(/[^\n]*\n|[^\n]+/g)` — each entry keeps its trailing '\n' if present.
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = content.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            lines.push(content[start..=i].to_string());
            start = i + 1;
        }
    }
    if start < content.len() {
        lines.push(content[start..].to_string());
    }
    // Handle empty string -> one empty entry? TS returns [] for "".
    // We return [] for "" to match TS (content.match returns null).
    // Our loop above returns [] for "" already.
    lines
}

#[derive(Debug, Clone, Copy)]
struct LineSpan {
    start: usize,
    end: usize,
}

fn get_line_spans(content: &str) -> Vec<LineSpan> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    for line in split_lines_with_endings(content) {
        let end = offset + line.len();
        spans.push(LineSpan { start: offset, end });
        offset = end;
    }
    spans
}

#[derive(Debug, Clone)]
struct MatchedEdit {
    edit_index: usize,
    match_index: usize,
    match_length: usize,
    new_text: String,
}

type TextReplacement = MatchedEdit;

fn get_replacement_line_range(
    lines: &[LineSpan],
    replacement: &TextReplacement,
) -> Result<Range<usize>, String> {
    let replacement_start = replacement.match_index;
    let replacement_end = replacement.match_index + replacement.match_length;

    let mut start_line: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if replacement_start >= line.start && replacement_start < line.end {
            start_line = Some(i);
            break;
        }
    }
    // Edge: replacement at EOF (content ends without newline, match at exact end)
    // TS throws if not found; but EOF appends should be handled.
    let start_line = match start_line {
        Some(v) => v,
        None => {
            // If replacement starts exactly at end of content (empty file or append),
            // treat as last line.
            if !lines.is_empty()
                && replacement_start == lines.last().unwrap().end
                && replacement.match_length == 0
            {
                lines.len() - 1
            } else {
                return Err("Replacement range is outside the base content.".to_string());
            }
        }
    };

    let mut end_line = start_line;
    while end_line < lines.len() && lines[end_line].end < replacement_end {
        end_line += 1;
    }
    if end_line >= lines.len() {
        return Err("Replacement range is outside the base content.".to_string());
    }
    Ok(start_line..end_line + 1)
}

fn apply_replacements(content: &str, replacements: &[TextReplacement], offset: usize) -> String {
    let mut result = content.to_string();
    // Apply in reverse so offsets remain stable.
    let mut sorted = replacements.to_vec();
    sorted.sort_by_key(|r| r.match_index);
    for r in sorted.iter().rev() {
        let idx = r.match_index - offset;
        // Safety: indices are byte indices from `find`, always on char boundaries.
        result = format!(
            "{}{}{}",
            &result[..idx],
            r.new_text,
            &result[idx + r.match_length..]
        );
    }
    result
}

fn apply_replacements_preserving_unchanged_lines(
    original_content: &str,
    base_content: &str,
    replacements: &[TextReplacement],
) -> Result<String, String> {
    let original_lines = split_lines_with_endings(original_content);
    let base_spans = get_line_spans(base_content);
    if original_lines.len() != base_spans.len() {
        return Err(
            "Cannot preserve unchanged lines because the base content has a different line count."
                .to_string(),
        );
    }

    // Group replacements by line range, merging overlapping groups.
    let mut sorted = replacements.to_vec();
    sorted.sort_by_key(|r| r.match_index);
    let mut groups: Vec<(Range<usize>, Vec<TextReplacement>)> = Vec::new();
    for r in sorted {
        let range = get_replacement_line_range(&base_spans, &r)?;
        if let Some((last_range, last_repls)) = groups.last_mut() {
            if range.start < last_range.end {
                last_range.end = last_range.end.max(range.end);
                last_repls.push(r);
                continue;
            }
        }
        groups.push((range, vec![r]));
    }

    let mut result = String::new();
    let mut original_line_idx = 0usize;
    for (range, repls) in groups {
        for line in &original_lines[original_line_idx..range.start] {
            result.push_str(line);
        }
        let group_start = base_spans[range.start].start;
        let group_end = base_spans[range.end - 1].end;
        let slice = &base_content[group_start..group_end];
        result.push_str(&apply_replacements(slice, &repls, group_start));
        original_line_idx = range.end;
    }
    for line in &original_lines[original_line_idx..] {
        result.push_str(line);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Fuzzy find
// ---------------------------------------------------------------------------

struct FuzzyMatchResult {
    found: bool,
    index: usize,
    match_length: usize,
    used_fuzzy: bool,
}

fn fuzzy_find_text(content: &str, old_text: &str) -> FuzzyMatchResult {
    if let Some(idx) = content.find(old_text) {
        return FuzzyMatchResult {
            found: true,
            index: idx,
            match_length: old_text.len(),
            used_fuzzy: false,
        };
    }
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old_text);
    if let Some(idx) = fuzzy_content.find(fuzzy_old.as_str()) {
        return FuzzyMatchResult {
            found: true,
            index: idx,
            match_length: fuzzy_old.len(),
            used_fuzzy: true,
        };
    }
    FuzzyMatchResult {
        found: false,
        index: 0,
        match_length: 0,
        used_fuzzy: false,
    }
}

fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old_text);
    if fuzzy_old.is_empty() {
        return 0;
    }
    fuzzy_content.matches(fuzzy_old.as_str()).count()
}

// ---------------------------------------------------------------------------
// Public: applyEditsToNormalizedContent
// ---------------------------------------------------------------------------

/// Apply one or more exact-text replacements to LF-normalized `content`.
///
/// All edits are matched against the same original content. Validation:
/// - `oldText` must be non-empty
/// - each `oldText` must occur exactly once (exact or fuzzy match)
/// - edits must not overlap
///
/// Returns `(baseContent, newContent)` where `baseContent` is the original
/// normalized content and `newContent` is the result. Errors are human-readable
/// strings mirroring `edit-diff.ts`.
pub fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<AppliedEditsResult, String> {
    if edits.is_empty() {
        return Err("edits must contain at least one replacement.".to_string());
    }

    // Normalize edits to LF as well.
    let normalized_edits: Vec<Edit> = edits
        .iter()
        .map(|e| Edit {
            old_text: normalize_to_lf(&e.old_text),
            new_text: normalize_to_lf(&e.new_text),
        })
        .collect();

    for (i, e) in normalized_edits.iter().enumerate() {
        if e.old_text.is_empty() {
            if normalized_edits.len() == 1 {
                return Err(format!("oldText must not be empty in {path}."));
            } else {
                return Err(format!("edits[{i}].oldText must not be empty in {path}."));
            }
        }
    }

    // Determine whether any edit requires fuzzy matching.
    let initial_matches: Vec<FuzzyMatchResult> = normalized_edits
        .iter()
        .map(|e| fuzzy_find_text(normalized_content, &e.old_text))
        .collect();
    let used_fuzzy = initial_matches.iter().any(|m| m.used_fuzzy);
    let replacement_base: String = if used_fuzzy {
        normalize_for_fuzzy_match(normalized_content)
    } else {
        normalized_content.to_string()
    };

    let mut matched: Vec<MatchedEdit> = Vec::with_capacity(normalized_edits.len());
    for (i, edit) in normalized_edits.iter().enumerate() {
        let m = fuzzy_find_text(&replacement_base, &edit.old_text);
        if !m.found {
            if normalized_edits.len() == 1 {
                return Err(format!(
                    "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
                ));
            } else {
                return Err(format!(
                    "Could not find edits[{i}] in {path}. The oldText must match exactly including all whitespace and newlines."
                ));
            }
        }
        let occurrences = count_occurrences(&replacement_base, &edit.old_text);
        if occurrences > 1 {
            if normalized_edits.len() == 1 {
                return Err(format!(
                    "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
                ));
            } else {
                return Err(format!(
                    "Found {occurrences} occurrences of edits[{i}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
                ));
            }
        }
        matched.push(MatchedEdit {
            edit_index: i,
            match_index: m.index,
            match_length: m.match_length,
            new_text: edit.new_text.clone(),
        });
    }

    matched.sort_by_key(|m| m.match_index);
    for w in matched.windows(2) {
        let prev = &w[0];
        let curr = &w[1];
        if prev.match_index + prev.match_length > curr.match_index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                prev.edit_index, curr.edit_index
            ));
        }
    }

    let base_content = normalized_content.to_string();
    let new_content = if used_fuzzy {
        // Try preserving unchanged lines; fall back to simple replacement if line counts diverge.
        match apply_replacements_preserving_unchanged_lines(
            normalized_content,
            &replacement_base,
            &matched,
        ) {
            Ok(s) => s,
            Err(_) => apply_replacements(&replacement_base, &matched, 0),
        }
    } else {
        apply_replacements(&replacement_base, &matched, 0)
    };

    if base_content == new_content {
        if normalized_edits.len() == 1 {
            return Err(format!(
                "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
            ));
        } else {
            return Err(format!("No changes made to {path}. The replacements produced identical content."));
        }
    }

    Ok(AppliedEditsResult {
        base_content,
        new_content,
    })
}

// ---------------------------------------------------------------------------
// Diff generation (simple LCS, no external crate)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum DiffOp {
    Equal(String),
    Delete(String),
    Insert(String),
}

fn lcs_diff(old_lines: &[String], new_lines: &[String]) -> Vec<DiffOp> {
    let n = old_lines.len();
    let m = new_lines.len();
    if n == 0 && m == 0 {
        return vec![];
    }
    // DP table for LCS length. Use u16/u32 optimization for large files? Keep usize.
    // For very large files this is O(n*m) memory; cap to avoid OOM by falling back to
    // naive diff if product is huge.
    const MAX_CELLS: usize = 10_000_000; // ~80MB for usize table
    if n * m > MAX_CELLS {
        // Fallback: treat as delete all + insert all
        let mut ops = Vec::new();
        for l in old_lines {
            ops.push(DiffOp::Delete(l.clone()));
        }
        for l in new_lines {
            ops.push(DiffOp::Insert(l.clone()));
        }
        return ops;
    }

    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            if old_lines[i] == new_lines[j] {
                dp[i][j] = dp[i + 1][j + 1] + 1;
            } else {
                dp[i][j] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old_lines[i] == new_lines[j] {
            ops.push(DiffOp::Equal(old_lines[i].clone()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(DiffOp::Delete(old_lines[i].clone()));
            i += 1;
        } else {
            ops.push(DiffOp::Insert(new_lines[j].clone()));
            j += 1;
        }
    }
    while i < n {
        ops.push(DiffOp::Delete(old_lines[i].clone()));
        i += 1;
    }
    while j < m {
        ops.push(DiffOp::Insert(new_lines[j].clone()));
        j += 1;
    }
    ops
}

/// Result of [`generate_diff_string`].
#[derive(Debug, Clone)]
pub struct DiffStringResult {
    pub diff: String,
    pub first_changed_line: Option<usize>,
}

/// Generate a display-oriented diff string with line numbers and `context` lines
/// of context, mirroring `generateDiffString` in `edit-diff.ts`.
///
/// Also returns `firstChangedLine` (1-based, in the new file) for editor navigation.
pub fn generate_diff_string(
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> DiffStringResult {
    let old_lines: Vec<String> = old_content.split('\n').map(|s| s.to_string()).collect();
    let new_lines: Vec<String> = new_content.split('\n').map(|s| s.to_string()).collect();
    let max_line = old_lines.len().max(new_lines.len()).max(1);
    let width = max_line.to_string().len();

    let ops = lcs_diff(&old_lines, &new_lines);

    // Build hunk-like display: we have flat ops; we need to emit context with collapsing.
    // For simplicity, produce a full numbered diff without collapsing, but cap context collapsing
    // similarly to the TS implementation: only show `context` lines around changes, collapse the rest.
    // Easiest: first collect output segments with their kind, then collapse long equal runs.

    #[derive(Debug, Clone)]
    struct Seg {
        kind: char, // ' ', '+', '-'
        num: usize, // line number (old for ' -', new for '+', old for ' ')
        text: String,
    }

    let mut segs: Vec<Seg> = Vec::new();
    let (mut old_no, mut new_no) = (1usize, 1usize);
    let mut first_changed: Option<usize> = None;
    for op in &ops {
        match op {
            DiffOp::Equal(t) => {
                segs.push(Seg {
                    kind: ' ',
                    num: old_no,
                    text: t.clone(),
                });
                old_no += 1;
                new_no += 1;
            }
            DiffOp::Delete(t) => {
                if first_changed.is_none() {
                    first_changed = Some(new_no);
                }
                segs.push(Seg {
                    kind: '-',
                    num: old_no,
                    text: t.clone(),
                });
                old_no += 1;
            }
            DiffOp::Insert(t) => {
                if first_changed.is_none() {
                    first_changed = Some(new_no);
                }
                segs.push(Seg {
                    kind: '+',
                    num: new_no,
                    text: t.clone(),
                });
                new_no += 1;
            }
        }
    }

    // Collapse long equal runs: keep `context` before/after each change block.
    let mut out: Vec<String> = Vec::new();
    // Identify change indices
    let is_change = |s: &Seg| s.kind != ' ';
    // Find segments that are near changes
    let mut keep = vec![false; segs.len()];
    for (idx, seg) in segs.iter().enumerate() {
        if is_change(seg) {
            let start = idx.saturating_sub(context_lines);
            let end = (idx + context_lines + 1).min(segs.len());
            for k in start..end {
                keep[k] = true;
            }
        }
    }
    // If no changes, keep nothing (diff empty)
    if !segs.iter().any(|s| is_change(s)) {
        return DiffStringResult {
            diff: String::new(),
            first_changed_line: None,
        };
    }
    // If no change-adjacent logic triggered (shouldn't happen), keep all
    if !keep.iter().any(|&k| k) {
        for k in 0..segs.len() {
            keep[k] = true;
        }
    }

    let mut i = 0;
    while i < segs.len() {
        if keep[i] {
            let s = &segs[i];
            let num_str = format!("{:>width$}", s.num, width = width);
            out.push(format!("{}{} {}", s.kind, num_str, s.text));
            i += 1;
        } else {
            // Skipped run
            let mut j = i;
            while j < segs.len() && !keep[j] {
                j += 1;
            }
            let pad = " ".repeat(width);
            out.push(format!(" {pad} ..."));
            i = j;
        }
    }

    DiffStringResult {
        diff: out.join("\n"),
        first_changed_line: first_changed,
    }
}

/// Convenience wrapper with default 4 lines of context, matching TS defaults.
pub fn generate_diff_string_default(old_content: &str, new_content: &str) -> DiffStringResult {
    generate_diff_string(old_content, new_content, 4)
}

/// Generate a standard unified patch, mirroring `generateUnifiedPatch`.
///
/// Uses the `diff` crate semantics: header `--- path` / `+++ path` and
/// `@@ -l,len +l,len @@` hunks with `context` lines. This is a minimal
/// implementation that produces a valid patch for review/automation; it does
/// not aim for 100% `diff` crate fidelity.
pub fn generate_unified_patch(
    path: &str,
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> String {
    let old_lines: Vec<String> = old_content.split('\n').map(|s| s.to_string()).collect();
    let new_lines: Vec<String> = new_content.split('\n').map(|s| s.to_string()).collect();
    let ops = lcs_diff(&old_lines, &new_lines);

    if ops.iter().all(|o| matches!(o, DiffOp::Equal(_))) {
        return String::new();
    }

    // Build hunks with context grouping
    // Walk ops, emit hunks separated by > context*2 equal lines
    let mut hunks: Vec<Vec<DiffOp>> = Vec::new();
    let mut cur: Vec<DiffOp> = Vec::new();
    let mut equal_run: Vec<DiffOp> = Vec::new();

    for op in ops {
        match &op {
            DiffOp::Equal(_) => {
                equal_run.push(op);
                if equal_run.len() > context_lines * 2 {
                    // Flush current hunk
                    if cur.iter().any(|o| !matches!(o, DiffOp::Equal(_))) {
                        // Keep leading context (first `context` of the run) in current hunk
                        let keep = equal_run.drain(..context_lines).collect::<Vec<_>>();
                        cur.extend(keep);
                        hunks.push(std::mem::take(&mut cur));
                        // Remaining equals are skipped (gap), keep trailing context for next hunk
                        let tail = if equal_run.len() > context_lines {
                            equal_run.split_off(equal_run.len() - context_lines)
                        } else {
                            std::mem::take(&mut equal_run)
                        };
                        // tail becomes start of next hunk's leading context
                        cur = tail;
                    } else {
                        // No changes yet, keep only trailing context
                        if equal_run.len() > context_lines {
                            equal_run.drain(..equal_run.len() - context_lines);
                        }
                        cur.extend(equal_run.drain(..));
                    }
                }
            }
            _ => {
                cur.extend(equal_run.drain(..));
                cur.push(op);
            }
        }
    }
    // Flush last
    if !equal_run.is_empty() {
        let keep = equal_run.len().min(context_lines);
        cur.extend(equal_run.into_iter().take(keep));
    }
    if cur.iter().any(|o| !matches!(o, DiffOp::Equal(_))) {
        hunks.push(cur);
    }

    if hunks.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(&format!("--- {path}\n"));
    out.push_str(&format!("+++ {path}\n"));

    // Compute line numbers per hunk
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    // We need to track global position to compute hunk headers.
    // Simpler: recompute per hunk by scanning all ops up to hunk start.
    // Instead, walk hunks sequentially using running counters and counting equals.
    let all_old: Vec<String> = old_content.split('\n').map(|s| s.to_string()).collect();
    let all_new: Vec<String> = new_content.split('\n').map(|s| s.to_string()).collect();
    let all_ops = lcs_diff(&all_old, &all_new);
    // Map hunk contents back to positions by scanning all_ops
    let mut op_idx = 0usize;
    for hunk in hunks {
        // Advance op_idx to start of hunk
        while op_idx < all_ops.len() {
            // Check if remaining all_ops[op_idx..] starts with hunk
            let mut matches = true;
            for (k, hop) in hunk.iter().enumerate() {
                if op_idx + k >= all_ops.len() {
                    matches = false;
                    break;
                }
                let a = &all_ops[op_idx + k];
                match (a, hop) {
                    (DiffOp::Equal(x), DiffOp::Equal(y)) if x == y => {}
                    (DiffOp::Delete(x), DiffOp::Delete(y)) if x == y => {}
                    (DiffOp::Insert(x), DiffOp::Insert(y)) if x == y => {}
                    _ => {
                        matches = false;
                        break;
                    }
                }
            }
            if matches {
                break;
            }
            match &all_ops[op_idx] {
                DiffOp::Equal(_) => {
                    old_line += 1;
                    new_line += 1;
                }
                DiffOp::Delete(_) => old_line += 1,
                DiffOp::Insert(_) => new_line += 1,
            }
            op_idx += 1;
        }
        let hunk_old_start = old_line;
        let hunk_new_start = new_line;
        let mut old_count = 0usize;
        let mut new_count = 0usize;
        for op in &hunk {
            match op {
                DiffOp::Equal(_) => {
                    old_count += 1;
                    new_count += 1;
                }
                DiffOp::Delete(_) => old_count += 1,
                DiffOp::Insert(_) => new_count += 1,
            }
        }
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk_old_start, old_count, hunk_new_start, new_count
        ));
        for op in &hunk {
            match op {
                DiffOp::Equal(t) => out.push_str(&format!(" {t}\n")),
                DiffOp::Delete(t) => out.push_str(&format!("-{t}\n")),
                DiffOp::Insert(t) => out.push_str(&format!("+{t}\n")),
            }
        }
        // Advance counters past hunk + op_idx
        for op in &hunk {
            match op {
                DiffOp::Equal(_) => {
                    old_line += 1;
                    new_line += 1;
                }
                DiffOp::Delete(_) => old_line += 1,
                DiffOp::Insert(_) => new_line += 1,
            }
        }
        op_idx += hunk.len();
        // Skip gap (already accounted via old_line/new_line advancement above? Need to also skip gap equals)
        // The gap equals are those all_ops between hunks that we already skipped in next iteration's while loop,
        // so counters will be updated there.
    }

    out
}

pub fn generate_unified_patch_default(path: &str, old_content: &str, new_content: &str) -> String {
    generate_unified_patch(path, old_content, new_content, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_split_and_restore() {
        let with_bom = "\u{FEFF}hello\nworld\n";
        let s = split_bom(with_bom);
        assert_eq!(s.bom, "\u{FEFF}");
        assert_eq!(s.text, "hello\nworld\n");
        let s2 = split_bom("no bom");
        assert_eq!(s2.bom, "");
        assert_eq!(s2.text, "no bom");
    }

    #[test]
    fn line_ending_round_trip() {
        let crlf = "a\r\nb\r\nc";
        assert_eq!(detect_line_ending(crlf), "\r\n");
        let lf = normalize_to_lf(crlf);
        assert_eq!(lf, "a\nb\nc");
        assert_eq!(restore_line_endings(&lf, "\r\n"), "a\r\nb\r\nc");
        assert_eq!(detect_line_ending("a\nb\n"), "\n");
        assert_eq!(detect_line_ending("no newline"), "\n");
    }

    #[test]
    fn apply_single_edit() {
        let content = "foo bar baz\n";
        let edits = vec![Edit {
            old_text: "bar".to_string(),
            new_text: "qux".to_string(),
        }];
        let r = apply_edits_to_normalized_content(content, &edits, "a.txt").unwrap();
        assert_eq!(r.new_content, "foo qux baz\n");
    }

    #[test]
    fn apply_multi_non_overlapping() {
        let content = "a=1\nb=2\nc=3\n";
        let edits = vec![
            Edit {
                old_text: "a=1".to_string(),
                new_text: "a=10".to_string(),
            },
            Edit {
                old_text: "c=3".to_string(),
                new_text: "c=30".to_string(),
            },
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "a.txt").unwrap();
        assert_eq!(r.new_content, "a=10\nb=2\nc=30\n");
    }

    #[test]
    fn overlapping_is_error() {
        let content = "hello world hello\n";
        let edits = vec![
            Edit {
                old_text: "hello world".to_string(),
                new_text: "hi".to_string(),
            },
            Edit {
                old_text: "world hello".to_string(),
                new_text: "x".to_string(),
            },
        ];
        let err = apply_edits_to_normalized_content(content, &edits, "a.txt").unwrap_err();
        assert!(err.contains("overlap"), "{err}");
    }

    #[test]
    fn duplicate_is_error() {
        let content = "dup dup dup\n";
        let edits = vec![Edit {
            old_text: "dup".to_string(),
            new_text: "x".to_string(),
        }];
        let err = apply_edits_to_normalized_content(content, &edits, "a.txt").unwrap_err();
        assert!(err.contains("occurrences"), "{err}");
    }

    #[test]
    fn not_found_is_error() {
        let content = "hello\n";
        let edits = vec![Edit {
            old_text: "absent".to_string(),
            new_text: "x".to_string(),
        }];
        let err = apply_edits_to_normalized_content(content, &edits, "a.txt").unwrap_err();
        assert!(err.contains("Could not find"), "{err}");
    }

    #[test]
    fn diff_string_has_first_changed_line() {
        let r = generate_diff_string("a\nb\nc\n", "a\nB\nc\n", 4);
        assert!(r.diff.contains("-"), "{}", r.diff);
        assert_eq!(r.first_changed_line, Some(2));
    }

    #[test]
    fn unified_patch_round_trip() {
        let p = generate_unified_patch("a.txt", "a\nb\n", "a\nB\n", 4);
        assert!(p.contains("--- a.txt"), "{p}");
        assert!(p.contains("+++ a.txt"), "{p}");
        assert!(p.contains("-b"), "{p}");
        assert!(p.contains("+B"), "{p}");
    }
}
