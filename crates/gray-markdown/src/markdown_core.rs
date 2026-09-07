//! Headless markdown analysis sharing Grok Build's exact `pulldown-cmark` config.
//!
//! This crate is intentionally lean -- it depends only on `pulldown-cmark` -- so it
//! can be used without pulling in the terminal-rendering stack (syntect, ratatui,
//! two-face). [`parser_options`] is the single source of truth for the parser
//! feature set, shared with `xai-grok-markdown` so analysis matches what Grok
//! Build actually renders 1:1.
//!
//! After parsing, Grok applies [`offset_events`]: only `~~…~~` is strikethrough.
//! Single-tilde pairs (`~text~`) are demoted to literal `~` text so LLM output
//! like `~**10%**` is not struck (pulldown treats those pairs as strike; we do not).

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// The exact `pulldown-cmark` option set Grok Build uses to render markdown.
///
/// With `ENABLE_STRIKETHROUGH`, pulldown treats both `~~…~~` and single-`~` pairs as
/// strike. Callers must consume events via [`offset_events`] so only double-tilde
/// strikethrough is retained (LLM-friendly post-policy).
pub fn parser_options() -> Options {
    Options::ENABLE_GFM
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_MATH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_TABLES
}

/// Offset event stream from Grok's parser, with single-tilde strikethrough demoted.
///
/// Prefer this over `Parser::new_ext(...).into_offset_iter()` so analysis and
/// rendering agree on what counts as strikethrough.
pub fn offset_events(text: &str) -> impl Iterator<Item = (Event<'_>, Range<usize>)> + '_ {
    DoubleTildeOnlyStrike {
        text,
        events: Parser::new_ext(text, parser_options()).into_offset_iter(),
    }
}

/// Stackless filter: Start and End share the same byte span in pulldown, so both
/// are classified by whether that span opens with `~~`. Single-tilde frames emit
/// delimiter `Text` instead of strike tags (delimiters are not separate events).
struct DoubleTildeOnlyStrike<'a, I> {
    text: &'a str,
    events: I,
}

/// True when the strike span at `range.start` is the double-tilde form.
fn is_double_tilde_strike(text: &str, range: &Range<usize>) -> bool {
    text.get(range.start..).is_some_and(|s| s.starts_with("~~"))
}

/// Opening or closing delimiter byte as `Text`, with the matching source range.
fn strike_delim_text<'a>(
    text: &'a str,
    range: &Range<usize>,
    opening: bool,
) -> (Event<'a>, Range<usize>) {
    let delim = if opening {
        let end = range.start + 1;
        debug_assert!(text.is_char_boundary(end) && end <= text.len());
        (range.start..end, &text[range.start..end])
    } else {
        let start = range.end - 1;
        debug_assert!(text.is_char_boundary(start) && start < text.len());
        (start..range.end, &text[start..range.end])
    };
    (Event::Text(delim.1.into()), delim.0)
}

impl<'a, I> Iterator for DoubleTildeOnlyStrike<'a, I>
where
    I: Iterator<Item = (Event<'a>, Range<usize>)>,
{
    type Item = (Event<'a>, Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        let (event, range) = self.events.next()?;
        match &event {
            Event::Start(Tag::Strikethrough) if !is_double_tilde_strike(self.text, &range) => {
                Some(strike_delim_text(self.text, &range, true))
            }
            Event::End(TagEnd::Strikethrough) if !is_double_tilde_strike(self.text, &range) => {
                Some(strike_delim_text(self.text, &range, false))
            }
            _ => Some((event, range)),
        }
    }
}

