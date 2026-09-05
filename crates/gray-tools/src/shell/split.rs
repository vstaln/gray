//! shell/split.rs — quote-aware command segment splitter (brief 4B).
//!
//! Shared by `guard::classify` (chain-aware verdicts over `&&`/`||`/`;`/`|`/`&`)
//! and `exit::masked_note` (pipe detection; dedupes its NOTE(1A) local copy).
//! Deliberately not a shell parser: heredoc bodies (`<<…\n…`) are opaque,
//! newline is not an operator, `f() { …; }` keeps its paren depth, and
//! subshell nesting past one level is best-effort.

/// One top-level command segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Segment text, trimmed of surrounding whitespace (quoting kept verbatim).
    pub text: String,
    /// Operator that preceded this segment (`None` for the first one).
    pub op_before: Option<&'static str>,
}

/// Split on `&&` `||` `;` (`;;`) `|` (`|&`) `&` — bare `&` only when followed
/// by whitespace/EOL — outside single/double quotes, backticks and `(…)` depth.
pub fn split_segments(cmd: &str) -> Vec<Segment> {
    split(cmd, false)
}

/// Pipe-only view for `exit::masked_note`: `||`/`&&`/`;`/`&` stay literal,
/// quote/backslash/subshell/heredoc rules identical to [`split_segments`].
pub fn split_pipeline(cmd: &str) -> Vec<String> {
    split(cmd, true).into_iter().map(|s| s.text).collect()
}

/// Top-level `( … )` / `$( … )` innards (one level, quotes respected) so
/// `(rm -rf /)` and `echo $(rm -rf /)` don't slip past the segment scan.
/// Unbalanced input yields nothing for that group.
pub fn subshell_inners(cmd: &str) -> Vec<String> {
    let chars: Vec<char> = cmd.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    let mut single = false;
    let mut double = false;
    while i < n {
        let c = chars[i];
        if single {
            if c == '\'' {
                single = false;
            }
            i += 1;
        } else if double {
            if c == '\\' {
                i += 1;
            } else if c == '"' {
                double = false;
            }
            i += 1;
        } else {
            match c {
                '\'' => {
                    single = true;
                    i += 1;
                }
                '"' => {
                    double = true;
                    i += 1;
                }
                '\\' => {
                    i += 2;
                }
                '$' if i + 1 < n && chars[i + 1] == '(' => {
                    if let Some((inner, end)) = balanced(&chars, i + 2) {
                        out.push(inner);
                        i = end;
                    } else {
                        i += 2;
                    }
                }
                '(' => {
                    if let Some((inner, end)) = balanced(&chars, i + 1) {
                        out.push(inner);
                        i = end;
                    } else {
                        i += 1;
                    }
                }
                _ => {
                    i += 1;
                }
            }
        }
    }
    out
}

/// From just past an opening `(`, collect to its match (quotes respected).
/// Returns the inner text and the index just past the closing `)`.
fn balanced(chars: &[char], mut i: usize) -> Option<(String, usize)> {
    let n = chars.len();
    let mut depth = 1u32;
    let mut inner = String::new();
    let mut single = false;
    let mut double = false;
    while i < n {
        let c = chars[i];
        if single {
            inner.push(c);
            if c == '\'' {
                single = false;
            }
            i += 1;
        } else if double {
            inner.push(c);
            if c == '\\' {
                if i + 1 < n {
                    inner.push(chars[i + 1]);
                    i += 1;
                }
            } else if c == '"' {
                double = false;
            }
            i += 1;
        } else {
            match c {
                '\'' => {
                    single = true;
                    inner.push(c);
                    i += 1;
                }
                '"' => {
                    double = true;
                    inner.push(c);
                    i += 1;
                }
                '\\' => {
                    inner.push(c);
                    if i + 1 < n {
                        inner.push(chars[i + 1]);
                        i += 1;
                    }
                    i += 1;
                }
                '(' => {
                    depth += 1;
                    inner.push(c);
                    i += 1;
                }
                ')' => {
                    depth -= 1;
                    i += 1;
                    if depth == 0 {
                        return Some((inner, i));
                    }
                    inner.push(')');
                }
                _ => {
                    inner.push(c);
                    i += 1;
                }
            }
        }
    }
    None
}

fn push_seg(segs: &mut Vec<Segment>, cur: &mut String, op: Option<&'static str>) {
    let text = cur.trim().to_string();
    cur.clear();
    if !text.is_empty() {
        segs.push(Segment { text, op_before: op });
    }
}

fn split(cmd: &str, pipes_only: bool) -> Vec<Segment> {
    let chars: Vec<char> = cmd.chars().collect();
    let n = chars.len();
    let mut segs: Vec<Segment> = Vec::new();
    let mut cur = String::new();
    let mut op: Option<&'static str> = None;
    let mut i = 0;
    let mut single = false;
    let mut double = false;
    let mut backtick = false;
    let mut depth: u32 = 0;
    // `<<` seen: the first depth-0 newline ends scanning (body opaque).
    let mut heredoc = false;
    while i < n {
        let c = chars[i];
        if single {
            cur.push(c);
            if c == '\'' {
                single = false;
            }
            i += 1;
        } else if double {
            cur.push(c);
            if c == '\\' {
                if i + 1 < n {
                    cur.push(chars[i + 1]);
                    i += 1;
                }
            } else if c == '"' {
                double = false;
            }
            i += 1;
        } else if backtick {
            cur.push(c);
            if c == '\\' {
                if i + 1 < n {
                    cur.push(chars[i + 1]);
                    i += 1;
                }
            } else if c == '`' {
                backtick = false;
            }
            i += 1;
        } else if c == '\'' {
            single = true;
            cur.push(c);
            i += 1;
        } else if c == '"' {
            double = true;
            cur.push(c);
            i += 1;
        } else if c == '`' {
            backtick = true;
            cur.push(c);
            i += 1;
        } else if c == '\\' {
            cur.push(c);
            if i + 1 < n {
                cur.push(chars[i + 1]);
                i += 2;
            } else {
                i += 1;
            }
        } else if c == '(' {
            depth = depth.saturating_add(1);
            cur.push(c);
            i += 1;
        } else if c == ')' {
            depth = depth.saturating_sub(1);
            cur.push(c);
            i += 1;
        } else if c == '<' && i + 1 < n && chars[i + 1] == '<' {
            // Heredoc opener: copy `<<[-]delim` verbatim and arm the body rule.
            cur.push_str("<<");
            i += 2;
            if i < n && chars[i] == '-' {
                cur.push('-');
                i += 1;
            }
            while i < n && (chars[i] == ' ' || chars[i] == '\t') {
                cur.push(chars[i]);
                i += 1;
            }
            if i < n && (chars[i] == '\'' || chars[i] == '"') {
                let q = chars[i];
                cur.push(q);
                i += 1;
                while i < n && chars[i] != q {
                    cur.push(chars[i]);
                    i += 1;
                }
                if i < n {
                    cur.push(q);
                    i += 1;
                }
            } else {
                while i < n
                    && !chars[i].is_whitespace()
                    && !matches!(chars[i], ';' | '&' | '|' | '(' | ')' | '<' | '>')
                {
                    cur.push(chars[i]);
                    i += 1;
                }
            }
            heredoc = true;
        } else if c == '\n' && heredoc {
            if depth == 0 {
                cur.push_str(&chars[i..].iter().collect::<String>());
                break;
            }
            // Heredoc nested in $(…): give up on the body, keep splitting.
            heredoc = false;
            cur.push(c);
            i += 1;
        } else if depth == 0 && c == '&' && i + 1 < n && chars[i + 1] == '&' {
            if pipes_only {
                cur.push_str("&&");
            } else {
                push_seg(&mut segs, &mut cur, op);
                op = Some("&&");
            }
            i += 2;
        } else if depth == 0 && c == '&' && !pipes_only && (i + 1 == n || chars[i + 1].is_whitespace())
        {
            push_seg(&mut segs, &mut cur, op);
            op = Some("&");
            i += 1;
        } else if depth == 0 && c == '|' && i + 1 < n && chars[i + 1] == '|' {
            if pipes_only {
                cur.push_str("||");
            } else {
                push_seg(&mut segs, &mut cur, op);
                op = Some("||");
            }
            i += 2;
        } else if depth == 0 && c == '|' {
            push_seg(&mut segs, &mut cur, op);
            if i + 1 < n && chars[i + 1] == '&' {
                op = Some("|&");
                i += 2;
            } else {
                op = Some("|");
                i += 1;
            }
        } else if depth == 0 && c == ';' && !pipes_only {
            push_seg(&mut segs, &mut cur, op);
            if i + 1 < n && chars[i + 1] == ';' {
                op = Some(";;");
                i += 2;
            } else {
                op = Some(";");
                i += 1;
            }
        } else {
            cur.push(c);
            i += 1;
        }
    }
    push_seg(&mut segs, &mut cur, op);
    segs
}

#[cfg(test)]
mod split_tests {
    use super::*;

    fn texts(cmd: &str) -> Vec<String> {
        split_segments(cmd)
            .into_iter()
            .map(|s| s.text)
            .collect()
    }

    #[test]
    fn pipe_splits_but_or_or_does_not() {
        assert_eq!(split_pipeline("false | tail -1").len(), 2);
        let segs = split_segments("a || b");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].op_before, Some("||"));
        assert_eq!(split_pipeline("a || b").len(), 1);
    }

    #[test]
    fn chain_ops_and_background() {
        assert_eq!(texts("echo a && rm -rf /; echo done"), ["echo a", "rm -rf /", "echo done"]);
        let segs = split_segments("echo a && rm -rf /; echo done");
        assert_eq!(segs[1].op_before, Some("&&"));
        assert_eq!(segs[2].op_before, Some(";"));
        let bg = split_segments("sleep 1 & echo done");
        assert_eq!(bg.len(), 2);
        assert_eq!(bg[1].op_before, Some("&"));
        // Redirections are not background ops.
        assert_eq!(texts("echo 2>&1"), ["echo 2>&1"]);
        assert_eq!(texts("cmd &>f"), ["cmd &>f"]);
    }

    #[test]
    fn quoted_operators_stay_literal() {
        assert_eq!(texts("echo 'a && rm -rf /'"), ["echo 'a && rm -rf /'"]);
        assert_eq!(split_pipeline("echo \"a|b\"").len(), 1);
    }

    #[test]
    fn subshell_depth_and_inners() {
        assert_eq!(texts("echo $(echo a | b)"), ["echo $(echo a | b)"]);
        assert_eq!(subshell_inners("(rm -rf /)"), ["rm -rf /"]);
        assert_eq!(subshell_inners("echo $(rm -rf /)"), ["rm -rf /"]);
    }

    #[test]
    fn heredoc_body_is_opaque_but_same_line_pipe_splits() {
        assert_eq!(texts("cat <<EOF\na && rm -rf /\nEOF").len(), 1);
        assert_eq!(split_pipeline("cat <<EOF | tail -1").len(), 2);
    }
}
