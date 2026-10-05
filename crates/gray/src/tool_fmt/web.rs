//! `web_search` / `web_fetch` result bodies rendered as readable text.
//!
//! The search sidecar returns bare JSON (`{"provider":…,"results":[…]}`) and
//! fetch returns one very long line; both leak HTML entities (`&nbsp;`,
//! `&#x27;`) into the card without decoding. JSON that isn't the known
//! results shape falls back to the generic pretty-print path.

use gray_markdown::decode_html_entities_text;

/// Display text for a web tool result, or `None` when `tool_name` isn't a
/// web tool or `trimmed` (the fence-stripped output) isn't the known shape.
pub(super) fn web_body_text(tool_name: &str, trimmed: &str) -> Option<String> {
    match tool_name {
        "web_search" => search_body_text(trimmed),
        "web_fetch" => Some(fetch_body_text(trimmed)),
        _ => None,
    }
}

fn search_body_text(trimmed: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let results = v.get("results")?.as_array()?;
    if results.is_empty() {
        return Some("no results".to_string());
    }
    // <title> / <url> / <snippet>, one result per triple, no blank lines.
    let mut lines = Vec::new();
    for result in results {
        for (key, indent) in [("title", ""), ("url", "  "), ("snippet", "  ")] {
            let Some(field) = result.get(key).and_then(|f| f.as_str()) else {
                continue;
            };
            let field = decode_html_entities_text(field);
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            lines.push(format!("{indent}{field}"));
        }
    }
    Some(lines.join("\n"))
}

fn fetch_body_text(trimmed: &str) -> String {
    // A fetch body is one giant line: the 40-line card cap counts raw lines,
    // so the text itself is cut here on a char boundary instead.
    const MAX_CHARS: usize = 2000;
    let decoded = decode_html_entities_text(trimmed);
    if decoded.chars().count() <= MAX_CHARS {
        return decoded;
    }
    let mut cut = decoded
        .char_indices()
        .nth(MAX_CHARS)
        .map(|(i, _)| i)
        .unwrap_or(decoded.len());
    // Back off to the last whitespace in the window's final 200 chars so
    // the cut lands on a word edge when one is nearby.
    if let Some((ws, _)) = decoded[..cut]
        .char_indices()
        .rev()
        .take(200)
        .find(|(_, c)| c.is_whitespace())
    {
        cut = ws;
    }
    let remaining = decoded[cut..].chars().count();
    format!("{}\n… +{remaining} chars", decoded[..cut].trim_end())
}
