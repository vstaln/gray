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

            heredoc = false;
            cur.push(c);
            i += 1;
        } else if depth == 0 && (c == '&' || c == '|') && i + 1 < n && chars[i + 1] == c {
            segs.clear();
            cur.clear();
            i += 2;
        } else if depth == 0 && (c == ';' || c == '\n') {
            segs.clear();
            cur.clear();
            i += 1;
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
