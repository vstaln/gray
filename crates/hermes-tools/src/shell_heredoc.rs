//! Conservative heredoc masking for shell-command scanners.
//! Port of `tools/shell_heredoc.py` (359 lines) — 1:1 behavior.
//!
//! Several guards scan raw command text for dangerous shell syntax (the
//! foreground background-'&' guard in `tools/terminal_tool.py`, the
//! blocked-command checks, the gateway lifecycle guard in
//! `cron/lifecycle_guard.py`). Heredoc *bodies* are usually inline data —
//! AppleScript concatenation, Python bitwise-and, literal UI text — and
//! scanning them produces false positives.
//!
//! Naively stripping every heredoc body is unsafe the other way: fake `<<`
//! markers inside quotes or comments can swallow a *real* background operator
//! that follows them, and some heredoc bodies genuinely execute (unquoted
//! delimiters allow `$(...)` expansion; `bash <<'EOF'` runs the body as
//! shell). This module therefore masks a body ONLY when all of the following
//! hold, and otherwise leaves the command untouched:
//!
//! - every heredoc delimiter on the opener is quoted (`<<'EOF'` / `<<"EOF"`
//!   / `<<E\OF`), so the body undergoes no shell expansion;
//! - every heredoc on the opener is terminated by an exact delimiter line;
//! - the opener composes a single command — no `;`, `|`, or `&` list or
//!   pipeline operators, and no nested `$(...)`, backtick, or process
//!   substitution scope;
//! - the consuming command is an allowlisted non-shell interpreter (see
//!   `is_inert_consumer`); consumers that execute their input as shell
//!   (`bash`, `sh`, `eval`, `ssh`, unknown commands) keep their bodies
//!   visible.
//!
//! Conservative retention may cause a false positive (a scanner may still flag
//! payload text in an unquoted or unknown-consumer body), but it can never hide
//! a real background operator or lifecycle command from a guard.
//!
//! Masked bodies are replaced by an equivalent number of newlines so line
//! structure — and any `re.MULTILINE` scanning downstream — is preserved.
//!
//! Adapted from Wolfram Ravenwolf's security-hardened rework of PR #63788
//! (commit 69c7663c6de6b6cb05bf99203fa39673efe01ccf).

// ---------------------------------------------------------------------------
// Inert consumer check — mirrors `_INERT_HEREDOC_CONSUMER_RE` (lines 48-55)
// ---------------------------------------------------------------------------

/// Return true when `masked_opener` is an allowlisted non-shell interpreter.
///
/// Python original (lines 48-55):
/// ```python
/// _INERT_HEREDOC_CONSUMER_RE = re.compile(
///     r"^\s*(?:[A-Z_][A-Z0-9_]*=\S+\s+)*(?:env\s+)?(?:[A-Za-z0-9_./-]+/)?"
///     r"(?:python(?:3(?:\.\d+)*)?|osascript|cat)(?=\s|$)",
///     re.IGNORECASE,
/// )
/// ```
///
/// Mirrors the regex without the `regex` crate (no new dependency) — manual
/// case-insensitive scan with identical `^\s*`, `VAR=...`, `env`, path prefix,
/// and `(?=\s|$)` semantics.
fn is_inert_consumer(masked_opener: &str) -> bool {
    let chars: Vec<char> = masked_opener.chars().collect();
    let n = chars.len();
    let mut pos = 0;

    // ^\s*
    while pos < n && chars[pos].is_whitespace() {
        pos += 1;
    }

    // (?:[A-Z_][A-Z0-9_]*=\S+\s+)*  — IGNORECASE, so [A-Z] matches a-z too
    loop {
        if pos >= n {
            break;
        }
        let first = chars[pos];
        if !(first.is_ascii_alphabetic() || first == '_') {
            break;
        }
        let mut i = pos + 1;
        while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        if i >= n || chars[i] != '=' {
            break;
        }
        i += 1;
        if i >= n || chars[i].is_whitespace() {
            break;
        }
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        if i >= n || !chars[i].is_whitespace() {
            break;
        }
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        pos = i;
    }

    // (?:env\s+)? — case-insensitive
    if pos + 3 <= n {
        let word: String = chars[pos..pos + 3].iter().collect();
        if word.eq_ignore_ascii_case("env") && pos + 3 < n && chars[pos + 3].is_whitespace() {
            let mut i = pos + 3;
            while i < n && chars[i].is_whitespace() {
                i += 1;
            }
            pos = i;
        }
    }

    // Try to match command with optional path prefix.
    // Prefix: (?:[A-Za-z0-9_./-]+/)?  — greedy, ends with '/'
    // Command: (?:python(?:3(?:\.\d+)*)?|osascript|cat)(?=\s|$)
    // We try every feasible prefix boundary (including no prefix).
    // Collect positions where prefix could end (after '/').
    let mut prefix_ends: Vec<usize> = vec![pos];
    // Scan forward from pos while chars are in allowed set, collect '/' boundaries.
    let mut i = pos;
    while i < n {
        let c = chars[i];
        let allowed = c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '/' || c == '-';
        if !allowed {
            break;
        }
        if c == '/' {
            prefix_ends.push(i + 1);
        }
        i += 1;
    }
    // Try each prefix end; succeed if any leaves a valid command.
    for &p in &prefix_ends {
        if p > n {
            continue;
        }
        // Validate prefix chars are all allowed and prefix ends with '/' if non-empty
        if p != pos {
            // prefix is chars[pos..p], must end with '/'
            if chars[p - 1] != '/' {
                continue;
            }
            let mut ok = true;
            for &c in &chars[pos..p] {
                let allowed = c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '/' || c == '-';
                if !allowed {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
        }
        if match_inert_command(&chars, p) {
            return true;
        }
    }
    false
}

fn match_inert_command(chars: &[char], pos: usize) -> bool {
    let n = chars.len();
    if pos >= n {
        return false;
    }
    // Try python
    if match_python(chars, pos) {
        return true;
    }
    // Try osascript (9 chars)
    if pos + 9 <= n {
        let s: String = chars[pos..pos + 9].iter().collect();
        if s.eq_ignore_ascii_case("osascript") {
            let after = pos + 9;
            if after >= n || chars[after].is_whitespace() {
                return true;
            }
        }
    }
    // Try cat (3 chars)
    if pos + 3 <= n {
        let s: String = chars[pos..pos + 3].iter().collect();
        if s.eq_ignore_ascii_case("cat") {
            let after = pos + 3;
            if after >= n || chars[after].is_whitespace() {
                return true;
            }
        }
    }
    false
}

fn match_python(chars: &[char], pos: usize) -> bool {
    let n = chars.len();
    if pos + 6 > n {
        return false;
    }
    let s: String = chars[pos..pos + 6].iter().collect();
    if !s.eq_ignore_ascii_case("python") {
        return false;
    }
    let mut cur = pos + 6;
    if cur < n && chars[cur] == '3' {
        cur += 1;
        // (?:\.\d+)* — zero or more dot + digits
        while cur < n && chars[cur] == '.' {
            if cur + 1 >= n || !chars[cur + 1].is_ascii_digit() {
                break;
            }
            cur += 1; // '.'
            while cur < n && chars[cur].is_ascii_digit() {
                cur += 1;
            }
        }
    }
    // (?=\s|$) — must be whitespace or end
    if cur >= n || chars[cur].is_whitespace() {
        return true;
    }
    // Also allow terminators like ';', '|', '&', '<', '>', '(', ')'? Python uses (?=\s|$) strictly,
    // so only whitespace or end counts. Keep faithful.
    false
}

// ---------------------------------------------------------------------------
// _mask_simple_quotes — mirrors lines 58-109
// ---------------------------------------------------------------------------

fn mask_simple_quotes(command: &str) -> String {
    let chars: Vec<char> = command.chars().collect();
    let n = chars.len();
    let mut result = String::new();
    let mut cursor: usize = 0;

    while cursor < n {
        let ch = chars[cursor];
        if ch == '\'' {
            // Find closing '
            let mut closing: Option<usize> = None;
            for j in cursor + 1..n {
                if chars[j] == '\'' {
                    closing = Some(j);
                    break;
                }
            }
            if let Some(closing) = closing {
                result.push_str("''");
                cursor = closing + 1;
                continue;
            } else {
                for &c in &chars[cursor..] {
                    result.push(c);
                }
                break;
            }
        }
        if ch == '"' {
            let mut end = cursor + 1;
            while end < n {
                if chars[end] == '\\' && end + 1 < n {
                    end += 2;
                    continue;
                }
                if chars[end] == '"' {
                    end += 1;
                    break;
                }
                end += 1;
            }
            // Python: if not command[cursor:end].endswith('"'): append remainder and break
            let ends_with_quote = end > cursor && end <= n && chars[end - 1] == '"';
            if !ends_with_quote {
                for &c in &chars[cursor..] {
                    result.push(c);
                }
                break;
            }
            let segment: String = chars[cursor..end].iter().collect();
            if segment.contains("$(") || segment.contains('`') {
                result.push_str(&segment);
            } else {
                result.push_str("\"\"");
            }
            cursor = end;
            continue;
        }
        if ch == '`' {
            let mut end = cursor + 1;
            while end < n {
                if chars[end] == '\\' && end + 1 < n {
                    end += 2;
                    continue;
                }
                if chars[end] == '`' {
                    end += 1;
                    break;
                }
                end += 1;
            }
            let segment: String = chars[cursor..end.min(n)].iter().collect();
            result.push_str(&segment);
            cursor = end;
            continue;
        }
        result.push(ch);
        cursor += 1;
    }
    result
}

// ---------------------------------------------------------------------------
// _contains_nested_shell_scope — mirrors lines 112-114
// ---------------------------------------------------------------------------

fn contains_nested_shell_scope(masked_opener: &str) -> bool {
    masked_opener.contains("$(") || masked_opener.contains('`') || masked_opener.contains("<(") || masked_opener.contains(">(")
}

// ---------------------------------------------------------------------------
// _parse_heredoc_operator — mirrors lines 117-181
// ---------------------------------------------------------------------------

fn parse_heredoc_operator(chars: &[char], index: usize) -> Option<(usize, String, bool, bool)> {
    let n = chars.len();
    if index + 1 >= n || chars[index] != '<' || chars[index + 1] != '<' {
        return None;
    }
    if index + 2 < n && chars[index + 2] == '<' {
        // here-string <<< — not a heredoc
        return None;
    }
    let mut cursor = index + 2;
    let mut strip_tabs = false;
    if cursor < n && chars[cursor] == '-' {
        strip_tabs = true;
        cursor += 1;
    }
    while cursor < n && (chars[cursor] == ' ' || chars[cursor] == '\t') {
        cursor += 1;
    }
    if cursor >= n || chars[cursor] == '\r' || chars[cursor] == '\n' {
        return None;
    }

    let mut delimiter = String::new();
    let mut quoted = false;

    while cursor < n {
        let ch = chars[cursor];
        if ch.is_whitespace() || matches!(ch, ';' | '&' | '|' | '<' | '>' | '(' | ')') {
            break;
        }
        if ch == '\\' {
            if cursor + 1 >= n || chars[cursor + 1] == '\r' || chars[cursor + 1] == '\n' {
                return None;
            }
            quoted = true;
            delimiter.push(chars[cursor + 1]);
            cursor += 2;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quoted = true;
            let quote = ch;
            cursor += 1;
            while cursor < n && chars[cursor] != quote {
                if quote == '"' && chars[cursor] == '\\' {
                    if cursor + 1 >= n {
                        return None;
                    }
                    let following = chars[cursor + 1];
                    if matches!(following, '$' | '`' | '"' | '\\' | '\n') {
                        delimiter.push(following);
                        cursor += 2;
                        continue;
                    }
                    // In double quotes, backslash is literal before all other chars.
                    delimiter.push('\\');
                    cursor += 1;
                    continue;
                }
                if chars[cursor] == '\r' || chars[cursor] == '\n' {
                    return None;
                }
                delimiter.push(chars[cursor]);
                cursor += 1;
            }
            if cursor >= n {
                return None;
            }
            cursor += 1; // skip closing quote
            continue;
        }
        delimiter.push(ch);
        cursor += 1;
    }

    if delimiter.is_empty() && !quoted {
        return None;
    }
    Some((cursor, delimiter, strip_tabs, quoted))
}

// ---------------------------------------------------------------------------
// _scan_heredoc_command_unit — mirrors lines 184-252
// ---------------------------------------------------------------------------

fn scan_heredoc_command_unit(
    chars: &[char],
    start: usize,
) -> (usize, Vec<(String, bool, bool)>, bool, bool) {
    let n = chars.len();
    let mut cursor = start;
    let mut quote: Option<char> = None;
    let mut comment = false;
    let mut specs: Vec<(String, bool, bool)> = Vec::new();
    let mut unknown_operator = false;
    let mut has_list_operator = false;

    while cursor < n {
        let ch = chars[cursor];
        if comment {
            if ch == '\n' {
                return (cursor, specs, unknown_operator, has_list_operator);
            }
            cursor += 1;
            continue;
        }
        if let Some(q) = quote {
            if (q == '"' || q == '`') && ch == '\\' && cursor + 1 < n {
                cursor += 2;
                continue;
            }
            if ch == q {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        if ch == '\\' && cursor + 1 < n {
            // Includes line continuations: the logical command keeps going on
            // the next physical line, so a heredoc opener there still belongs
            // to this unit.
            cursor += 2;
            continue;
        }
        if ch == '\'' || ch == '"' || ch == '`' {
            quote = Some(ch);
            cursor += 1;
            continue;
        }
        if ch == '#' {
            let prev_is_space_or_op = if cursor == start {
                true
            } else {
                let prev = chars[cursor - 1];
                prev.is_whitespace() || matches!(prev, ';' | '&' | '|' | '(' | ')')
            };
            if prev_is_space_or_op {
                comment = true;
                cursor += 1;
                continue;
            }
        }
        if ch == '\n' {
            return (cursor, specs, unknown_operator, has_list_operator);
        }
        if cursor + 2 < n && chars[cursor] == '<' && chars[cursor + 1] == '<' && chars[cursor + 2] == '<' {
            cursor += 3;
            continue;
        }
        if cursor + 1 < n && chars[cursor] == '<' && chars[cursor + 1] == '<' {
            // Check for <<< already handled above, but keep distinction
            if cursor + 2 < n && chars[cursor + 2] == '<' {
                cursor += 3;
                continue;
            }
            if let Some((next_cursor, delimiter, strip_tabs, quoted)) = parse_heredoc_operator(chars, cursor) {
                cursor = next_cursor;
                specs.push((delimiter, strip_tabs, quoted));
                continue;
            } else {
                unknown_operator = true;
                cursor += 2;
                continue;
            }
        }
        if matches!(ch, ';' | '|' | '&') {
            has_list_operator = true;
        }
        cursor += 1;
    }

    (n, specs, unknown_operator, has_list_operator)
}

// ---------------------------------------------------------------------------
// _find_heredoc_close — mirrors lines 255-278
// ---------------------------------------------------------------------------

fn find_heredoc_close(chars: &[char], body_start: usize, delimiter: &str, strip_tabs: bool) -> Option<usize> {
    let n = chars.len();
    let delim_chars: Vec<char> = delimiter.chars().collect();
    let mut cursor = body_start;

    loop {
        // Find next newline
        let newline_pos = chars[cursor..].iter().position(|&c| c == '\n').map(|p| cursor + p);
        let (line_end, after) = match newline_pos {
            Some(pos) => (pos, pos + 1),
            None => (n, n),
        };
        // line = command[cursor:newline] (or to end)
        // Handle \r
        let mut line_len = line_end - cursor;
        if line_len > 0 && chars[line_end - 1] == '\r' {
            line_len -= 1;
        }
        let line_slice = &chars[cursor..cursor + line_len];
        let candidate: &[char] = if strip_tabs {
            let mut i = 0;
            while i < line_slice.len() && line_slice[i] == '\t' {
                i += 1;
            }
            &line_slice[i..]
        } else {
            line_slice
        };
        if candidate.len() == delim_chars.len() && candidate.iter().zip(delim_chars.iter()).all(|(a, b)| a == b) {
            return Some(after);
        }
        if newline_pos.is_none() {
            return None;
        }
        cursor = after;
    }
}

fn last_opener_index(chars: &[char]) -> Option<usize> {
    if chars.len() < 2 {
        return None;
    }
    for i in (0..chars.len() - 1).rev() {
        if chars[i] == '<' && chars[i + 1] == '<' {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API — mirrors strip_inert_heredoc_bodies (lines 281-359)
// ---------------------------------------------------------------------------

/// Mask heredoc bodies that are provably inert data; keep the rest.
///
/// See the module docstring for the qualification rules. Masked bodies are
/// replaced with an equivalent number of newlines so positions of the
/// surrounding real command text keep their line structure. On ANY ambiguity
/// (unparseable `<<` token, unterminated heredoc, unquoted delimiter,
/// compound opener, nested shell scope, unknown consumer) the original
/// command is returned unchanged — a scanner false positive is acceptable,
/// hiding real shell syntax is not.
pub fn strip_inert_heredoc_bodies(command: &str) -> String {
    let chars: Vec<char> = command.chars().collect();
    let n = chars.len();

    // Fast path: no '<<' anywhere means no heredoc can exist — skip the state
    // machine entirely. This function runs on every terminal tool call.
    if !command.contains("<<") {
        return command.to_string();
    }
    // No heredoc opener can start after the last '<<' occurrence; once the
    // scan passes it, the rest of the command needs no per-char walk.
    let last_opener = match last_opener_index(&chars) {
        Some(v) => v,
        None => return command.to_string(),
    };

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut command_start: usize = 0;

    while command_start < n {
        if command_start > last_opener {
            break;
        }
        let (command_end, specs, unknown_operator, has_list_operator) =
            scan_heredoc_command_unit(&chars, command_start);
        if unknown_operator {
            return command.to_string();
        }
        if specs.is_empty() {
            if command_end >= n {
                break;
            }
            command_start = command_end + 1;
            continue;
        }
        if command_end >= n {
            // Opener with no following body line: nothing to mask, and the
            // heredoc is unterminated — leave everything visible.
            return command.to_string();
        }

        let mut body_cursor = command_end + 1;
        let mut body_ranges: Vec<(usize, usize)> = Vec::new();
        let mut unterminated = false;
        for (delimiter, strip_tabs, _quoted) in &specs {
            match find_heredoc_close(&chars, body_cursor, delimiter, *strip_tabs) {
                Some(close_end) => {
                    body_ranges.push((body_cursor, close_end));
                    body_cursor = close_end;
                }
                None => {
                    unterminated = true;
                    break;
                }
            }
        }
        if unterminated {
            return command.to_string();
        }

        let all_quoted = specs.iter().all(|(_, _, q)| *q);
        if all_quoted && !has_list_operator {
            let opener_str: String = chars[command_start..command_end].iter().collect();
            let masked_opener = mask_simple_quotes(&opener_str);
            if !contains_nested_shell_scope(&masked_opener) && is_inert_consumer(&masked_opener) {
                ranges.extend(body_ranges);
            }
        }
        command_start = body_cursor;
    }

    if ranges.is_empty() {
        return command.to_string();
    }
    // Single-pass rebuild: ranges are sorted and non-overlapping, so join the
    // kept segments with newline-preserving replacements (avoids a quadratic
    // full-string copy per masked range on heredoc-heavy commands).
    let mut out = String::new();
    let mut previous = 0;
    for (start, end) in ranges {
        for &c in &chars[previous..start] {
            out.push(c);
        }
        let nl_count = chars[start..end].iter().filter(|&&c| c == '\n').count();
        for _ in 0..nl_count {
            out.push('\n');
        }
        previous = end;
    }
    for &c in &chars[previous..] {
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_simple_quotes_single() {
        assert_eq!(mask_simple_quotes("echo 'hello'"), "echo ''");
        assert_eq!(mask_simple_quotes("echo 'a' 'b'"), "echo '' ''");
    }

    #[test]
    fn mask_simple_quotes_double_without_substitution() {
        assert_eq!(mask_simple_quotes(r#"echo "hello""#), "echo \"\"");
    }

    #[test]
    fn mask_simple_quotes_double_with_dollar_paren() {
        assert_eq!(mask_simple_quotes(r#"echo "$(echo hi)""#), r#"echo "$(echo hi)""#);
    }

    #[test]
    fn mask_simple_quotes_double_with_backtick() {
        assert_eq!(mask_simple_quotes("echo \"`echo hi`\""), "echo \"`echo hi`\"");
    }

    #[test]
    fn mask_simple_quotes_backtick_always_kept() {
        assert_eq!(mask_simple_quotes("echo `echo hi`"), "echo `echo hi`");
    }

    #[test]
    fn contains_nested_scope() {
        assert!(contains_nested_shell_scope("echo $(ls)"));
        assert!(contains_nested_shell_scope("echo `ls`"));
        assert!(contains_nested_shell_scope("echo <(cat)"));
        assert!(contains_nested_shell_scope("echo >(cat)"));
        assert!(!contains_nested_shell_scope("echo hello"));
    }

    #[test]
    fn inert_consumer_python() {
        assert!(is_inert_consumer("python <<'EOF'"));
        assert!(is_inert_consumer("python3 <<'EOF'"));
        assert!(is_inert_consumer("python3.10 <<'EOF'"));
        assert!(is_inert_consumer("/usr/bin/python <<'EOF'"));
        assert!(is_inert_consumer("  python <<'EOF'"));
        assert!(is_inert_consumer("env python <<'EOF'"));
        assert!(is_inert_consumer("FOO=bar python <<'EOF'"));
        assert!(is_inert_consumer("cat <<'EOF'"));
        assert!(is_inert_consumer("osascript <<'EOF'"));
        assert!(!is_inert_consumer("bash <<'EOF'"));
        assert!(!is_inert_consumer("sh <<'EOF'"));
        assert!(!is_inert_consumer("ssh <<'EOF'"));
        assert!(!is_inert_consumer("unknown <<'EOF'"));
    }

    #[test]
    fn inert_consumer_case_insensitive() {
        assert!(is_inert_consumer("PYTHON <<'EOF'"));
        assert!(is_inert_consumer("Cat <<'EOF'"));
        assert!(is_inert_consumer("OsAsCrIpT <<'EOF'"));
    }

    #[test]
    fn parse_heredoc_operator_basic() {
        let cmd: Vec<char> = "cat <<'EOF'".chars().collect();
        let res = parse_heredoc_operator(&cmd, 4).unwrap();
        assert_eq!(res.1, "EOF");
        assert!(res.3); // quoted
        assert!(!res.2); // strip_tabs
    }

    #[test]
    fn parse_heredoc_operator_strip_tabs() {
        let cmd: Vec<char> = "cat <<-'EOF'".chars().collect();
        let res = parse_heredoc_operator(&cmd, 4).unwrap();
        assert!(res.2);
        assert_eq!(res.1, "EOF");
    }

    #[test]
    fn parse_heredoc_operator_unquoted() {
        let cmd: Vec<char> = "cat <<EOF".chars().collect();
        let res = parse_heredoc_operator(&cmd, 4).unwrap();
        assert_eq!(res.1, "EOF");
        assert!(!res.3);
    }

    #[test]
    fn parse_heredoc_operator_here_string_rejected() {
        let cmd: Vec<char> = "cat <<< 'foo'".chars().collect();
        assert!(parse_heredoc_operator(&cmd, 4).is_none());
    }

    #[test]
    fn find_heredoc_close_basic() {
        let cmd: Vec<char> = "line1\nEOF\nline2".chars().collect();
        let pos = find_heredoc_close(&cmd, 0, "EOF", false).unwrap();
        assert_eq!(pos, 10); // after "EOF\n" (6 + 4)
        let s: String = cmd[0..pos].iter().collect();
        assert!(s.contains("EOF"));
    }

    #[test]
    fn find_heredoc_close_strip_tabs() {
        let cmd: Vec<char> = "line1\n\tEOF\n".chars().collect();
        assert!(find_heredoc_close(&cmd, 6, "EOF", true).is_some());
        assert!(find_heredoc_close(&cmd, 6, "EOF", false).is_none());
    }

    #[test]
    fn strip_inert_masks_quoted_python() {
        let cmd = "python <<'PY'\nprint('hi & hello')\nPY\n";
        let out = strip_inert_heredoc_bodies(cmd);
        // body should be masked to newlines
        assert!(!out.contains("print('hi & hello')"));
        assert_eq!(out.matches('\n').count(), cmd.matches('\n').count());
        // opener preserved
        assert!(out.starts_with("python <<'PY'"));
    }

    #[test]
    fn strip_inert_keeps_unquoted() {
        let cmd = "python <<PY\nprint('hi & hello')\nPY\n";
        let out = strip_inert_heredoc_bodies(cmd);
        assert_eq!(out, cmd);
    }

    #[test]
    fn strip_inert_keeps_unknown_consumer() {
        let cmd = "bash <<'EOF'\necho hi & echo bye\nEOF\n";
        let out = strip_inert_heredoc_bodies(cmd);
        assert_eq!(out, cmd);
    }

    #[test]
    fn strip_inert_keeps_compound_opener() {
        let cmd = "python <<'PY'; echo hi\ncode\nPY\n";
        let out = strip_inert_heredoc_bodies(cmd);
        assert_eq!(out, cmd);
    }

    #[test]
    fn strip_inert_keeps_nested_scope() {
        let cmd = "python <<'PY' $(echo hi)\nbody\nPY\n";
        // opener contains $(...) after masking? Actually mask leaves $(...) visible, so contains_nested returns true
        let out = strip_inert_heredoc_bodies(cmd);
        assert_eq!(out, cmd);
    }

    #[test]
    fn strip_inert_no_heredoc_fast_path() {
        let cmd = "echo hello";
        assert_eq!(strip_inert_heredoc_bodies(cmd), cmd);
    }

    #[test]
    fn strip_inert_unterminated_no_mask() {
        let cmd = "python <<'PY'\nhello\n";
        let out = strip_inert_heredoc_bodies(cmd);
        assert_eq!(out, cmd);
    }

    #[test]
    fn strip_inert_preserves_line_count() {
        let cmd = "cat <<'EOF'\nline1\nline2\nEOF\necho done\n";
        let out = strip_inert_heredoc_bodies(cmd);
        assert_eq!(out.matches('\n').count(), cmd.matches('\n').count());
        assert!(!out.contains("line1"));
        assert!(out.contains("echo done"));
    }
}
