//! gray-extras: out-of-default-build surface (proxy, OAuth signin).
//!
//! Phase 1 scope cut: these modules left the default `gray` build so the
//! default tree carries no axum/OAuth-signin weight. They still build
//! (and test) under `--workspace`.

pub mod oauth;
pub mod proxy;

// ---------------------------------------------------------------------------
// `webfetch` tool (opencode webfetch.ts parity, minus Turndown)
// ---------------------------------------------------------------------------
//
// Fetches a URL and returns the content as markdown/text (naive tag-strip;
// no HTML->Markdown crate in tree and no new deps allowed) or raw HTML.
// Redirects follow reqwest's default (up to 10), with a request timeout and
// a fetch size cap.
//
// Approval verdict (documented choice, no `approvals.rs` change needed):
// network egress is never read-only-`Allow`. The gate's fail-closed default
// already yields `Ask` in `auto`, `Deny` in `read-only`, `Allow` in `full`
// for tools without an explicit arm — that is the verdict for `webfetch`.
//
// Registry wiring note: this crate is a leaf (`gray-extras -> gray`), so no
// default-tree registry (`gray-tools` `Registry::builtin`,
// `gray-plugin` `default_plugins`) can name this type without a dependency
// cycle. Wire it via [`webfetch_tool`] once a home crate exists.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use gray_core::tool_out::{fail, finish, get_opt_u64, get_str};
use serde_json::{Value, json};

/// Tool name surfaced to the model.
pub const WEBFETCH_TOOL_NAME: &str = "webfetch";

/// One-line snippet for the system prompt's "Available tools" list.
pub const WEBFETCH_SNIPPET: &str = "Fetch a URL and return its content as markdown or text";

/// Default request timeout in seconds.
pub const WEBFETCH_DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Upper bound for the `timeout` arg (clamped, not rejected).
pub const WEBFETCH_MAX_TIMEOUT_SECS: u64 = 120;
/// Fetch size cap in bytes (output still passes through `finish()` truncation).
pub const WEBFETCH_MAX_BYTES: usize = 512 * 1024;

/// Output format for [`WebfetchTool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebfetchFormat {
    Markdown,
    Text,
    Html,
}

/// Fetch a URL and return its content. Redirects are followed, the request
/// has a timeout, and the body is capped at [`WEBFETCH_MAX_BYTES`].
pub struct WebfetchTool;

/// Constructor for registry wiring (see module docs for why wiring lives
/// outside the default tree for now).
pub fn webfetch_tool() -> Arc<dyn Tool> {
    Arc::new(WebfetchTool)
}

/// Requires an absolute `http(s)` URL.
fn check_url(raw: &str) -> Result<reqwest::Url, ToolOutput> {
    match raw.parse::<reqwest::Url>() {
        Ok(url) if url.scheme() == "http" || url.scheme() == "https" => Ok(url),
        _ => Err(fail(
            "invalid argument 'url': expected http(s) URL".to_string(),
        )),
    }
}

/// Re-gates the URL after `send()`: reqwest follows up to 10 redirects, so a
/// benign initial URL can land on `file:`/other schemes. Deny non-http(s).
fn check_final_url(url: &reqwest::Url) -> Result<(), ToolOutput> {
    if url.scheme() == "http" || url.scheme() == "https" {
        Ok(())
    } else {
        Err(fail(format!(
            "fetch denied: redirect led to non-http(s) URL ({} scheme)",
            url.scheme()
        )))
    }
}

/// `format` arg: absent/null means markdown (opencode parity).
fn parse_format(args: &Value) -> Result<WebfetchFormat, ToolOutput> {
    match args.get("format") {
        None | Some(Value::Null) => Ok(WebfetchFormat::Markdown),
        Some(Value::String(s)) => match s.to_ascii_lowercase().as_str() {
            "markdown" | "md" => Ok(WebfetchFormat::Markdown),
            "text" | "txt" => Ok(WebfetchFormat::Text),
            "html" => Ok(WebfetchFormat::Html),
            _ => Err(fail(
                "invalid argument 'format': expected markdown, text, or html".to_string(),
            )),
        },
        Some(_) => Err(fail(
            "invalid argument 'format': expected string".to_string(),
        )),
    }
}

/// Clamps the `timeout` arg into `1..=WEBFETCH_MAX_TIMEOUT_SECS`.
fn clamp_timeout_secs(v: Option<u64>) -> u64 {
    v.unwrap_or(WEBFETCH_DEFAULT_TIMEOUT_SECS)
        .clamp(1, WEBFETCH_MAX_TIMEOUT_SECS)
}

/// ASCII case-insensitive byte search (`needle` must be ASCII, so every hit
/// is a char boundary and slicing there is safe).
fn find_ci(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() || needle.len() > hay.len() - from {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
        .map(|p| from + p)
}

/// Cuts `<tag ...>...</tag>` blocks (script/style). An unclosed block drops
/// the rest of the document (fail safe: never leaks script text).
fn cut_blocks(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        match find_ci(bytes, open.as_bytes(), i) {
            None => {
                out.push_str(&html[i..]);
                break;
            }
            Some(s) => {
                out.push_str(&html[i..s]);
                match html[s..].find('>') {
                    None => break,
                    Some(rel) => match find_ci(bytes, close.as_bytes(), s + rel + 1) {
                        None => break,
                        Some(e) => match html[e..].find('>') {
                            None => break,
                            Some(rel2) => i = e + rel2 + 1,
                        },
                    },
                }
            }
        }
    }
    out
}

/// Cuts `<!--...-->` comments; an unterminated comment drops the rest.
fn cut_comments(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        match find_ci(bytes, b"<!--", i) {
            None => {
                out.push_str(&html[i..]);
                break;
            }
            Some(s) => {
                out.push_str(&html[i..s]);
                match find_ci(bytes, b"-->", s + 4) {
                    None => break,
                    Some(e) => i = e + 3,
                }
            }
        }
    }
    out
}

/// Strips remaining tags; block-level tags become a newline so text from
/// adjacent elements does not glue together.
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        let mut tag = String::new();
        for c2 in chars.by_ref() {
            if c2 == '>' {
                break;
            }
            if tag.len() < 16 {
                tag.push(c2);
            }
        }
        let name: String = tag
            .trim_start_matches('/')
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "br" | "p"
                | "div"
                | "li"
                | "tr"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "section"
                | "article"
                | "header"
                | "footer"
                | "blockquote"
                | "pre"
                | "ul"
                | "ol"
        ) {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
    out
}

/// Decodes the common named entities plus decimal/hex numeric refs.
/// Unknown entities pass through verbatim.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        match rest.find('&') {
            None => {
                out.push_str(rest);
                break;
            }
            Some(i) => {
                out.push_str(&rest[..i]);
                let tail = &rest[i..];
                match tail.char_indices().find(|(_, c)| *c == ';').map(|(p, _)| p) {
                    None => {
                        out.push_str(tail);
                        break;
                    }
                    Some(semi) if semi > 32 => {
                        out.push('&');
                        rest = &rest[i + 1..];
                    }
                    Some(semi) => {
                        let ent = &tail[1..semi];
                        let decoded =
                            if ent.len() > 1 && (ent.starts_with("#x") || ent.starts_with("#X")) {
                                u32::from_str_radix(&ent[2..], 16)
                                    .ok()
                                    .and_then(char::from_u32)
                            } else if let Some(num) = ent.strip_prefix('#') {
                                num.parse::<u32>().ok().and_then(char::from_u32)
                            } else {
                                match ent {
                                    "amp" => Some('&'),
                                    "lt" => Some('<'),
                                    "gt" => Some('>'),
                                    "quot" => Some('"'),
                                    "apos" => Some('\''),
                                    "nbsp" => Some('\u{a0}'),
                                    _ => None,
                                }
                            };
                        match decoded {
                            Some(c) => out.push(c),
                            None => out.push_str(&tail[..=semi]),
                        }
                        rest = &tail[semi + 1..];
                    }
                }
            }
        }
    }
    out
}

/// HTML to plain text: drops script/style/comments, strips tags, decodes
/// entities, and collapses blank lines.
pub fn strip_html_to_text(html: &str) -> String {
    let no_script = cut_blocks(html, "script");
    let no_style = cut_blocks(&no_script, "style");
    let stripped = strip_tags(&cut_comments(&no_style));
    decode_entities(&stripped)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait]
impl Tool for WebfetchTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            WEBFETCH_TOOL_NAME,
            "Fetch a URL and return the content as plain text (the markdown and text formats both return tag-stripped plain text until a Markdown converter lands) or raw HTML. Follows redirects (http/https only), with a request timeout and a fetch size cap.",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch (http or https)" },
                    "format": { "type": "string", "description": "Output format: markdown (default, currently plain text), text, or raw html", "enum": ["markdown", "text", "html"] },
                    "timeout": { "type": "integer", "description": "Request timeout in seconds (default: 30, max: 120)" }
                },
                "required": ["url"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(WEBFETCH_SNIPPET)
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> ToolOutput {
        let url = match get_str(&args, "url") {
            Ok(u) => u,
            Err(e) => return e,
        };
        let url = match check_url(&url) {
            Ok(u) => u,
            Err(e) => return e,
        };
        let format = match parse_format(&args) {
            Ok(f) => f,
            Err(e) => return e,
        };
        let timeout_secs = match get_opt_u64(&args, "timeout") {
            Ok(v) => clamp_timeout_secs(v),
            Err(e) => return e,
        };
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
        {
            Ok(c) => c,
            Err(e) => return fail(format!("fetch failed: {e}")),
        };
        let mut resp = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => return fail(format!("fetch failed: {e}")),
        };
        if let Err(e) = check_final_url(resp.url()) {
            return e;
        }
        if !resp.status().is_success() {
            return fail(format!("fetch failed: HTTP {}", resp.status()));
        }
        let mut buf: Vec<u8> = Vec::new();
        let mut fetch_truncated = false;
        loop {
            match resp.chunk().await {
                Err(e) => return fail(format!("fetch failed: {e}")),
                Ok(None) => break,
                Ok(Some(chunk)) => {
                    if buf.len() + chunk.len() > WEBFETCH_MAX_BYTES {
                        let room = WEBFETCH_MAX_BYTES.saturating_sub(buf.len());
                        buf.extend_from_slice(&chunk[..room]);
                        fetch_truncated = true;
                        break;
                    }
                    buf.extend_from_slice(&chunk);
                }
            }
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        // No Turndown in tree and no new deps: markdown and text share the
        // naive tag-strip for now (documented in the tool description).
        let mut body = match format {
            WebfetchFormat::Html => text,
            WebfetchFormat::Markdown | WebfetchFormat::Text => strip_html_to_text(&text),
        };
        if fetch_truncated {
            body.push_str(&format!(
                "\n[fetch truncated at {WEBFETCH_MAX_BYTES} byte cap]"
            ));
        }
        finish(body)
    }
}

// UNRUN (cargo test banned under X): run in TTY/CI.
#[cfg(test)]
mod webfetch_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn def_shape_matches_tool_vocabulary() {
        let def = WebfetchTool.def();
        assert_eq!(def.name, WEBFETCH_TOOL_NAME);
        assert_eq!(
            def.parameters.get("required"),
            Some(&json!(["url"])),
            "{}",
            def.parameters
        );
        for key in ["url", "format", "timeout"] {
            assert!(
                def.parameters
                    .get("properties")
                    .and_then(|p| p.get(key))
                    .is_some(),
                "missing property {key}"
            );
        }
        assert!(WebfetchTool.prompt_snippet().is_some());
    }

    #[test]
    fn format_defaults_to_markdown() {
        assert_eq!(parse_format(&json!({})).unwrap(), WebfetchFormat::Markdown);
        assert_eq!(
            parse_format(&json!({"format": "TEXT"})).unwrap(),
            WebfetchFormat::Text
        );
        assert_eq!(
            parse_format(&json!({"format": "html"})).unwrap(),
            WebfetchFormat::Html
        );
        assert!(parse_format(&json!({"format": "pdf"})).is_err());
    }

    #[test]
    fn timeout_clamps_instead_of_rejecting() {
        assert_eq!(clamp_timeout_secs(None), WEBFETCH_DEFAULT_TIMEOUT_SECS);
        assert_eq!(clamp_timeout_secs(Some(0)), 1);
        assert_eq!(clamp_timeout_secs(Some(5)), 5);
        assert_eq!(clamp_timeout_secs(Some(999)), WEBFETCH_MAX_TIMEOUT_SECS);
    }

    #[test]
    fn url_requires_http_scheme() {
        assert!(check_url("https://example.com/x").is_ok());
        assert!(check_url("http://example.com/").is_ok());
        assert!(check_url("ftp://example.com/x").is_err());
        assert!(check_url("not a url").is_err());
    }

    #[test]
    fn redirect_to_non_http_scheme_is_denied() {
        // Code-path assert (no network): execute() runs this on resp.url().
        let https: reqwest::Url = "https://example.com/x".parse().unwrap();
        assert!(check_final_url(&https).is_ok());
        let file: reqwest::Url = "file:///etc/passwd".parse().unwrap();
        let err = check_final_url(&file).expect_err("file: redirect must deny");
        assert!(err.content.contains("non-http"), "{}", err.content);
        assert!(err.is_error);
    }

    #[test]
    fn def_description_discloses_plain_text_fallback() {
        let desc = WebfetchTool.def().description;
        assert!(
            desc.contains("plain text"),
            "def must not promise real markdown: {desc}"
        );
    }

    #[test]
    fn strip_drops_script_style_and_comments() {
        let html = "<html><head><style>.a{color:red}</style>\
            <script>alert(1)</script></head><body><!-- hi --><p>Hello</p></body></html>";
        let text = strip_html_to_text(html);
        assert!(!text.contains("alert"), "{text}");
        assert!(!text.contains("color"), "{text}");
        assert!(!text.contains("hi"), "{text}");
        assert!(text.contains("Hello"), "{text}");
    }

    #[test]
    fn strip_decodes_entities_and_block_newlines() {
        let text = strip_html_to_text("<p>a &amp; b</p><p>&lt;tag&gt; &#65;&#x42;</p>");
        assert_eq!(text, "a & b\n<tag> AB", "{text}");
    }

    #[test]
    fn webfetch_is_ask_in_auto_and_denied_read_only() {
        use gray_core::approvals::{Verdict, verdict};
        let cwd = PathBuf::from("/work/proj");
        // Network egress: never read-only-Allow; fail-closed default owns it.
        assert_eq!(
            verdict("auto", WEBFETCH_TOOL_NAME, &json!({}), &cwd),
            Verdict::Ask
        );
        assert!(matches!(
            verdict("read-only", WEBFETCH_TOOL_NAME, &json!({}), &cwd),
            Verdict::Deny(_)
        ));
        assert_eq!(
            verdict("full", WEBFETCH_TOOL_NAME, &json!({}), &cwd),
            Verdict::Allow
        );
    }
}
