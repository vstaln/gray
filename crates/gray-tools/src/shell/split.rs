//! shell/split.rs — quote-aware pipeline splitter.
//!
//! Used by `exit::masked_note` (pipe detection). Deliberately not a shell
//! parser: heredoc bodies (`<<…\n…`) are opaque, newline is not an operator,
//! and `f() { …; }` keeps its paren depth.

/// Pipe-only view for `exit::masked_note`: all other operators
/// (`&&`, `||`, `;`, `&`) stay literal, except that they are not split on.
/// Quote/backslash/subshell/heredoc rules stay exact.
pub fn split_pipeline(cmd: &str) -> Vec<String> {
    split(cmd)
}

fn push_seg(segs: &mut Vec<String>, cur: &mut String) {
    let text = cur.trim().to_string();
    cur.clear();
    if !text.is_empty() {
        segs.push(text);
    }
}

fn split(cmd: &str) -> Vec<String> {
    let chars: Vec<char> = cmd.chars().collect();
    let n = chars.len();
    let mut segs: Vec<String> = Vec::new();
    let mut cur = String::new();
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
            cur.push_str("&&");
            i += 2;
        } else if depth == 0 && c == '|' && i + 1 < n && chars[i + 1] == '|' {
            cur.push_str("||");
            i += 2;
        } else if depth == 0 && c == '|' {
            push_seg(&mut segs, &mut cur);
            if i + 1 < n && chars[i + 1] == '&' {
                i += 2;
            } else {
                i += 1;
            }
        } else {
            cur.push(c);
            i += 1;
        }
    }
    push_seg(&mut segs, &mut cur);
    segs
}

#[cfg(test)]
mod split_tests {
    use super::*;

    #[test]
    fn pipe_splits_but_or_or_and_chain_do_not() {
        assert_eq!(split_pipeline("false | tail -1").len(), 2);
        assert_eq!(split_pipeline("a || b").len(), 1);
        assert_eq!(split_pipeline("echo a && rm -rf /").len(), 1);
        assert_eq!(split_pipeline("sleep 1 & echo done").len(), 1);
        assert_eq!(split_pipeline("a; b").len(), 1);
    }

    #[test]
    fn quoted_operators_and_subshell_depth_stay_literal() {
        assert_eq!(split_pipeline("echo 'a|b'"), ["echo 'a|b'"]);
        assert_eq!(split_pipeline("echo \"a|b\"").len(), 1);
        assert_eq!(split_pipeline("echo $(echo a | b)"), ["echo $(echo a | b)"]);
    }

    #[test]
    fn heredoc_body_is_opaque_but_same_line_pipe_splits() {
        assert_eq!(split_pipeline("cat <<EOF\na && rm -rf /\nEOF").len(), 1);
        assert_eq!(split_pipeline("cat <<EOF | tail -1").len(), 2);
    }
}
