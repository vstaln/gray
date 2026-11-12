use crate::agent::Agent;
use crate::message::{ContentBlock, Message, Role};

pub(crate) fn push_synthetic(agent: &mut Agent, id: &str, reason: &str) {
    agent.messages.push(Message {
        role: Role::User,
        injected: false,
        content: vec![ContentBlock::ToolResult {
            id: id.to_string(),
            content: format!("[{reason}]"),
            is_error: true,
        }],
    });
}

pub(crate) fn answer_pending_tools(
    agent: &mut Agent,
    tool_uses: &[(String, String, serde_json::Value)],
    from_idx: usize,
    reason: &str,
) {
    answer_pending_range(agent, tool_uses, from_idx, usize::MAX, reason);
}

pub(crate) fn answer_pending_range(
    agent: &mut Agent,
    tool_uses: &[(String, String, serde_json::Value)],
    from_idx: usize,
    to_idx: usize,
    reason: &str,
) {
    for (id, _, _) in tool_uses
        .iter()
        .skip(from_idx)
        .take(to_idx.saturating_sub(from_idx))
    {
        push_synthetic(agent, id, reason);
    }
}

#[derive(Default)]
pub(crate) struct PendingToolCall {
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) arguments: String,
    pub(crate) thought_signature: Option<String>,

    pub(crate) started: bool,
}

impl PendingToolCall {
    pub(crate) fn parsed_args(&self) -> serde_json::Value {
        if self.arguments.is_empty() {
            return serde_json::Value::Null;
        }
        let mut parsed = serde_json::from_str(&self.arguments)
            .or_else(|_| serde_json::from_str(&escape_raw_controls(&self.arguments)));
        if parsed.is_err()
            && let Some(v) = repair_json_text(&self.arguments)
        {
            log::warn!(target: "gray_agent", "repaired malformed tool-call arguments");
            parsed = Ok(v);
        }
        match parsed {
            Ok(serde_json::Value::String(inner)) => serde_json::from_str(&inner)
                .ok()
                .or_else(|| repair_json_text(&inner))
                .filter(serde_json::Value::is_object)
                .unwrap_or(serde_json::Value::String(inner)),
            Ok(v) => v,
            Err(_) => serde_json::Value::String(self.arguments.clone()),
        }
    }
}

/// Last-resort repair for model-emitted JSON that strict parsing and
/// control-char escaping could not fix: escapes stray quotes that sit inside
/// string values (the classic "unescaped quote in an embedded script" break)
/// and closes structures left unterminated by a truncated stream. Bounded:
/// gives up after 32 repair attempts.
pub(crate) fn repair_json_text(raw: &str) -> Option<serde_json::Value> {
    let mut text = escape_raw_controls(raw);
    for _ in 0..32 {
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => return Some(v),
            Err(e) => {
                if e.is_eof() {
                    text = close_unterminated(&text)?;
                    continue;
                }
                // serde reports line/column in characters; arguments are
                // single-line, so the column is the character offset
                let col = e.column();
                if col == 0 || e.line() != 1 {
                    return None;
                }
                let byte_pos = char_to_byte(&text, col - 1)?;
                if e.to_string().contains("trailing comma")
                    && text.as_bytes().get(byte_pos) == Some(&b',')
                {
                    text.remove(byte_pos);
                    continue;
                }
                // a bare character where a delimiter was expected usually
                // means the previous string closed early on a stray quote —
                // escape that closing quote and retry
                if let Some(q) = last_closing_quote(&text, byte_pos) {
                    text.insert(q, '\\');
                    continue;
                }
                return None;
            }
        }
    }
    None
}

/// Byte offset of the last `"` that closed a string before `byte_pos`.
fn last_closing_quote(text: &str, byte_pos: usize) -> Option<usize> {
    let mut last_close = None;
    let (mut in_str, mut esc) = (false, false);
    for (i, b) in text.bytes().enumerate().take(byte_pos + 1) {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
                last_close = Some(i);
            }
            continue;
        }
        if b == b'"' {
            in_str = true;
        }
    }
    last_close
}

fn char_to_byte(s: &str, char_idx: usize) -> Option<usize> {
    s.char_indices().nth(char_idx).map(|(b, _)| b)
}

/// Close an unterminated string and any open objects/arrays. Returns None
/// when nothing needed closing (no progress possible).
fn close_unterminated(text: &str) -> Option<String> {
    let mut stack: Vec<char> = Vec::new();
    let (mut in_str, mut esc) = (false, false);
    for c in text.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    if !in_str && stack.is_empty() {
        return None;
    }
    let mut out = text.to_string();
    if in_str {
        out.push('"');
    }
    while let Some(c) = stack.pop() {
        out.push(c);
    }
    Some(out)
}

fn escape_raw_controls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut in_str, mut escaped) = (false, false);
    for c in s.chars() {
        if in_str && !escaped && (c as u32) < 0x20 {
            match c {
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => out.push_str(&format!("\\u{:04x}", c as u32)),
            }
            continue;
        }
        out.push(c);
        if escaped {
            escaped = false;
        } else if in_str && c == '\\' {
            escaped = true;
        } else if c == '"' {
            in_str = !in_str;
        }
    }
    out
}

#[cfg(test)]
mod parse_tests {
    use super::PendingToolCall;

    fn parse(raw: &str) -> serde_json::Value {
        PendingToolCall {
            arguments: raw.into(),
            ..Default::default()
        }
        .parsed_args()
    }

    #[test]
    fn stray_quote_inside_string_is_escaped() {
        let v = parse("{\"command\": \"echo \"hello\" done\"}");
        assert_eq!(v["command"], "echo \"hello\" done");
    }

    #[test]
    fn truncated_object_is_closed() {
        let v = parse("{\"command\": \"ls -la");
        assert_eq!(v["command"], "ls -la");
    }

    #[test]
    fn truncated_array_block_recovers_complete_elements() {
        let v =
            parse("[{\"name\": \"bash\", \"arguments\": {\"command\": \"pwd\"}}, {\"name\": \"rea");
        assert!(v.is_array());
        assert_eq!(v[0]["name"], "bash");
    }

    #[test]
    fn unrepairable_text_stays_a_string() {
        let v = parse("not json at all");
        assert_eq!(v, serde_json::Value::String("not json at all".into()));
    }

    #[test]
    fn raw_newlines_in_strings_are_repaired() {
        let v = parse("{\"command\": \"printf 'a\\\\n'\nls\tx\"}");
        assert_eq!(v["command"], "printf 'a\\n'\nls\tx");
    }

    #[test]
    fn double_encoded_object_is_unwrapped() {
        let v = parse(r#""{\"command\":\"ls\"}""#);
        assert_eq!(v["command"], "ls");
    }

    #[test]
    fn garbage_stays_a_string() {
        assert!(parse("{\"command\": ").is_string());
    }
}
