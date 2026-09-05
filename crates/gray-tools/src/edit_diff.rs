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
        if let Some((last_range, last_repls)) = groups.last_mut()
            && range.start < last_range.end
        {
            last_range.end = last_range.end.max(range.end);
            last_repls.push(r);
            continue;
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
            return Err(format!(
                "No changes made to {path}. The replacements produced identical content."
            ));
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

    for op in &ops {
        match op {
            DiffOp::Equal(_) => {
                equal_run.push(op.clone());
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
                        cur.append(&mut equal_run);
                    }
                }
            }
            _ => {
                cur.append(&mut equal_run);
                cur.push(op.clone());
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

    // Compute hunk headers from the single ops result (no second lcs_diff).
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut op_idx = 0usize;
    // Reuse ops to advance gap counters between hunks.
    let all_ops = &ops;
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
    generate_unified_patch(path, old_content, new_content, 3)
}

// ---------------------------------------------------------------------------
// T1.6: cat -n prefix tolerance (relational fix for T1.2)
// ---------------------------------------------------------------------------

/// Note appended to a successful edit when the prefix repair fired, so the
/// model learns to quote unprefixed text next time.
pub const EDIT_PREFIX_STRIP_NOTE: &str =
    "[edit: stripped cat -n prefixes from oldText/newText]";

/// Strip one leading `cat -n` prefix (`^\s*\d+\t`) from each line that has
/// one; lines without that shape are returned unchanged.
pub fn strip_cat_n_prefixes(text: &str) -> String {
    text.split('\n')
        .map(strip_one_prefix)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip the prefix from a single line, or return it unchanged. The head
/// before the first tab must be whitespace followed by digits (`cat -n`
/// layout); anything else — tab-indented code, `12 \t` with a stray space,
/// `abc\t` — is left alone.
fn strip_one_prefix(line: &str) -> &str {
    let Some(tab) = line.find('\t') else {
        return line;
    };
    let (head, _) = line.split_at(tab);
    let digits = head.trim_start_matches(|c: char| c.is_whitespace());
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return line;
    }
    &line[tab + 1..]
}

/// Strip prefixes from a set of edits for the T1.6 retry. Each edit's
/// `old_text` drives: its `new_text` is stripped only when its `old_text`
/// was (both or neither, never new alone). Returns `None` when no edit's
/// `old_text` changed, i.e. there is nothing to retry.
pub fn strip_edit_prefixes(edits: &[Edit]) -> Option<Vec<Edit>> {
    let mut changed = false;
    let out: Vec<Edit> = edits
        .iter()
        .map(|e| {
            let stripped_old = strip_cat_n_prefixes(&e.old_text);
            if stripped_old != e.old_text {
                changed = true;
                Edit {
                    old_text: stripped_old,
                    new_text: strip_cat_n_prefixes(&e.new_text),
                }
            } else {
                e.clone()
            }
        })
        .collect();
    if changed {
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod prefix_tests {
    use super::*;

    #[test]
    fn strips_cat_n_prefixes_per_line() {
        assert_eq!(strip_cat_n_prefixes("   412\tfoo"), "foo");
        assert_eq!(strip_cat_n_prefixes("     1\t# Agents"), "# Agents");
        assert_eq!(strip_cat_n_prefixes("   100\ta\n   101\tb"), "a\nb");
    }

    #[test]
    fn leaves_non_prefix_lines_alone() {
        assert_eq!(strip_cat_n_prefixes("plain"), "plain");
        assert_eq!(strip_cat_n_prefixes("\tindented"), "\tindented");
        assert_eq!(strip_cat_n_prefixes("12 \tspaced"), "12 \tspaced");
        assert_eq!(strip_cat_n_prefixes("abc\tdef"), "abc\tdef");
    }

    #[test]
    fn strip_set_gates_on_old_text_both_or_neither() {
        // oldText without a prefix → nothing to retry (None): newText alone
        // is never stripped.
        let only_new = vec![Edit {
            old_text: "foo".to_string(),
            new_text: "   3\tbar".to_string(),
        }];
        assert!(strip_edit_prefixes(&only_new).is_none());
        // oldText with a prefix → both stripped together.
        let both = vec![Edit {
            old_text: "   3\tfoo".to_string(),
            new_text: "   3\tbar".to_string(),
        }];
        let got = strip_edit_prefixes(&both).unwrap();
        assert_eq!(got[0].old_text, "foo");
        assert_eq!(got[0].new_text, "bar");
    }

    #[test]
    fn stripped_retry_order_exact_first() {
        // A file whose real content starts with `12\t` still matches
        // exactly — the repair must never fire first (edit.rs retries only
        // on failure; here the exact apply already succeeds).
        let content = "12\tfoo\n";
        let exact = vec![Edit {
            old_text: "12\tfoo".to_string(),
            new_text: "12\tbaz".to_string(),
        }];
        let applied = apply_edits_to_normalized_content(content, &exact, "f").unwrap();
        assert!(applied.new_content.contains("12\tbaz"));
        // A prefixed oldText fails exact, but its stripped form matches —
        // the two calls below are exactly what edit.rs does (try, then retry).
        let prefixed = vec![Edit {
            old_text: "   412\tfoo".to_string(),
            new_text: "   412\tbaz".to_string(),
        }];
        assert!(apply_edits_to_normalized_content(content, &prefixed, "f").is_err());
        let stripped = strip_edit_prefixes(&prefixed).unwrap();
        let repaired = apply_edits_to_normalized_content(content, &stripped, "f").unwrap();
        assert!(repaired.new_content.contains("12\tbaz"));
    }
}
