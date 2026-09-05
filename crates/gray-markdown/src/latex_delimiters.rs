//! Streaming normalization of LaTeX math delimiters into the canonical
//! `$...$` / `$$...$$` forms that `pulldown-cmark`'s math extension understands.
// 1305 lines for \( \) → $ normalization + streaming edge; simple str::replace suffices for batch, feature-gate when size matters.
//!
//! Models overwhelmingly emit the backslash delimiter forms (`\(...\)`,
//! `\[...\]`) and sometimes `\begin{equation}...\end{equation}`. `pulldown-cmark`
//! only recognizes the `$` forms, so historically the backslash forms were
//! handled by bespoke post-parse source scanners — which were disabled inside
//! table cells (a bug). By rewriting every delimiter form into the
//! canonical `$`/`$$` form *before* parsing, the existing
//! `Event::InlineMath`/`Event::DisplayMath` handlers (which already convert math
//! in both prose and table cells) handle everything uniformly.
//!
//! # Transform set (applied only outside code, respecting escapes)
//!
//! | Input | Output |
//! |-------|--------|
//! | `\( … \)` | `$…$` (whitespace just inside the delimiters trimmed) |
//! | `\)` (unmatched) | `$` |
//! | `\[ … \]` / `$$ … $$` / `\begin{equation} … \end{equation}` | `$$…$$`, interior newlines joined |
//! | `\[` / `\]` (unmatched) | `$$` |
//! | `\begin{equation[*]}` / `\end{equation[*]}` (unmatched) | `$$` |
//!
//! Inline `\( … \)` is converted span-at-once: the matching unescaped `\)` is
//! located and the ASCII whitespace immediately inside the delimiters is
//! trimmed, so the emitted `$…$` has no space right after the opening `$` or
//! before the closing `$`. pulldown-cmark's dollar-math flanking rule rejects
//! `$ … $` (whitespace next to a delimiter) and would otherwise leave a padded
//! span as raw `$ … $` text. Interior newlines join to spaces (TeX treats them
//! as spaces) so a span wrapped across source lines cannot be re-parsed as
//! block structure.
//!
//! # Display spans are joined onto one line
//!
//! Every display-math opener — `\[`, `\begin{equation[*]}`, or a bare `$$` —
//! is resolved span-at-once: the matching close (`\]`, `\end{equation[*]}`, or
//! `$$`, whichever comes first) is located and the span is emitted as
//! `$$…$$` with each interior line trimmed and joined by a single space.
//! CommonMark gives *block* constructs priority over inline math, so a
//! multi-line `$$…$$` whose interior contains a line that looks like a block
//! start — a setext underline (`=`/`-` alone on a line), a `#`
//! heading, or a `-` list item — would otherwise be split into
//! heading/list/paragraph blocks and never reach the math parser. TeX treats
//! interior newlines as spaces, so joining is semantics-preserving (`\\` row
//! separators are untouched and still produce multi-line output downstream).
//!
//! The close-scan is bounded: it gives up (emitting the opener alone, exactly
//! the old behavior) past [`MAX_MATH_SOURCE_LEN`] look-ahead, at a blank line
//! (a paragraph break — two stray `$$` in prose must not fuse across
//! paragraphs), or at a line starting with `>` (blockquoted math carries `>`
//! markers that must not become span content; pulldown already handles the
//! quoted multi-line span after marker stripping).
//!
//! Bare single `$` is left untouched (so the pass is **idempotent**). Escaped
//! openers (`\\(`, `\\[`, `\$`) are left literal via backslash-pair consumption,
//! matching the old scanner's even/odd parity rule. Content inside inline code
//! spans and fenced code blocks is left verbatim (so LaTeX-in-backticks stays
//! raw, preserving prior behavior). Inner LaTeX environments such as
//! `\begin{aligned}` / `\begin{pmatrix}` are *not* touched — they live inside the
//! `$$...$$` span and are rendered by the LaTeX→Unicode converter.
//!
//! # Streaming
//!
//! [`LatexDelimiterNormalizer`] is fed chunks in order and is **chunk-split
//! invariant**: feeding the same total text produces the same output regardless
//! of where the chunk boundaries fall. It achieves this by holding back only a
//! bounded ambiguous suffix (a trailing `\`/`\begin{…` partial, a trailing
//! backtick/tilde run whose length isn't yet known, or an unclosed inline `\(`
//! whose `\)` has not arrived — bounded by the math size cap so an open that
//! never closes cannot stall the stream) until the next chunk, and by flushing
//! that suffix on [`finish`](LatexDelimiterNormalizer::finish).
//!
//! # Known divergences from CommonMark (bounded, documented)
//!
//! - 4-space *indented* code blocks are not treated as code (math inside them
//!   would convert). Rare in model output.
//! - Inline code spans are treated as single-line: an unterminated `` ` `` reverts
//!   to normal at the newline. This only changes behavior next to a stray,
//!   unmatched backtick.
//!
//! Streaming-vs-one-shot equivalence is pinned by an exhaustive byte-split test.

use crate::latex::MAX_MATH_SOURCE_LEN;

const ENV_BEGIN: &str = "\\begin{equation}";
const ENV_BEGIN_STARRED: &str = "\\begin{equation*}";
const ENV_END: &str = "\\end{equation}";
const ENV_END_STARRED: &str = "\\end{equation*}";

/// Longest environment token we special-case (`\begin{equation*}` = 17 bytes).
/// Bounds how many trailing bytes a `push` may hold back for a `\begin`/`\end`.
const ENV_TOKENS: [&str; 4] = [ENV_BEGIN, ENV_BEGIN_STARRED, ENV_END, ENV_END_STARRED];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    /// Inside an inline code span opened by a run of `run` backticks.
    InlineCode {
        run: usize,
    },
    /// Inside a fenced code block opened by `len` copies of `ch` (`` ` `` or `~`).
    Fenced {
        ch: u8,
        len: usize,
    },
}

/// Streaming, code-aware, escape-aware LaTeX delimiter normalizer.
///
/// Feed chunks via [`push`](Self::push) and call [`finish`](Self::finish) at end
/// of stream. For a complete string in hand, use [`normalize_latex_delimiters`].
#[derive(Debug, Clone)]
pub struct LatexDelimiterNormalizer {
    state: State,
    /// True when the next byte begins a new line (start of input counts).
    at_line_start: bool,
    /// Raw bytes held back from a previous `push` because they may be the prefix
    /// of a construct that needs more input to classify.
    pending: String,
}

impl Default for LatexDelimiterNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl LatexDelimiterNormalizer {
    pub fn new() -> Self {
        Self {
            state: State::Normal,
            at_line_start: true,
            pending: String::new(),
        }
    }

    /// Reset to the initial state, dropping any held-back bytes.
    pub fn reset(&mut self) {
        self.state = State::Normal;
        self.at_line_start = true;
        self.pending.clear();
    }

    /// Push a raw chunk; returns the finalized normalized prefix.
    ///
    /// A bounded ambiguous suffix may be held back and emitted by a later
    /// `push` or by [`finish`](Self::finish).
    pub fn push(&mut self, chunk: &str) -> String {
        if chunk.is_empty() {
            return String::new();
        }
        let mut buf = std::mem::take(&mut self.pending);
        buf.push_str(chunk);
        let (out, consumed) = self.process(&buf, false);
        self.pending = buf[consumed..].to_string();
        out
    }

    /// Flush any held-back bytes as literal (end of stream).
    pub fn finish(&mut self) -> String {
        let buf = std::mem::take(&mut self.pending);
        if buf.is_empty() {
            return String::new();
        }
        let (out, consumed) = self.process(&buf, true);
        debug_assert_eq!(consumed, buf.len(), "final flush must consume all input");
        out
    }

    /// Process `buf` from the start, advancing internal state. Returns the
    /// emitted text and the number of bytes consumed; bytes `[consumed..]` are
    /// the held-back ambiguous suffix (always empty when `final_flush`).
    fn process(&mut self, buf: &str, final_flush: bool) -> (String, usize) {
        let bytes = buf.as_bytes();
        let n = bytes.len();
        let mut out = String::with_capacity(n + 8);
        let mut i = 0;
        while i < n {
            match self.state {
                State::Normal => {
                    if self.at_line_start {
                        match scan_fence_open(bytes, i, final_flush) {
                            FenceScan::NeedMore => break,
                            FenceScan::Match { ch, len, end } => {
                                out.push_str(&buf[i..end]);
                                i = end;
                                self.state = State::Fenced { ch, len };
                                self.at_line_start = false;
                                continue;
                            }
                            FenceScan::No => {}
                        }
                    }
                    match bytes[i] {
                        b'\n' => {
                            out.push('\n');
                            i += 1;
                            self.at_line_start = true;
                        }
                        b'`' => {
                            let run = count_run(bytes, i, b'`');
                            if i + run == n && !final_flush {
                                break; // run may extend; hold it back
                            }
                            out.push_str(&buf[i..i + run]);
                            i += run;
                            self.state = State::InlineCode { run };
                            self.at_line_start = false;
                        }
                        b'\\' => {
                            // Every non-`break` arm below advances past a delimiter
                            // mid-line, so `at_line_start` is cleared once here; the
                            // `break`s (hold-backs) skip it and preserve it for
                            // the retry, making that invariant structural.
                            match classify_backslash(buf, i, final_flush) {
                                Bs::NeedMore => break,
                                Bs::InlineOpen => match find_inline_close(bytes, i, final_flush) {
                                    InlineClose::Found { close } => {
                                        // Trim pulldown's flanking whitespace so `$…$` is
                                        // accepted; the custom set (vs `char::is_ascii_whitespace`)
                                        // exists only to add vertical tab (0x0B).
                                        let inner = buf[i + 2..close]
                                            .trim_matches(|c: char| matches!(c, ' ' | '\t'..='\r'));
                                        if inner.is_empty() {
                                            // Empty after trim: a lone `$` keeps the old
                                            // position-for-position output (`$<ws>$`, or
                                            // `$$` when the interior is truly empty).
                                            out.push('$');
                                            i += 2;
                                        } else {
                                            // Join interior newlines: a `$…$` wrapped
                                            // across source lines would otherwise be
                                            // vulnerable to block re-parsing (setext
                                            // underlines, list markers).
                                            out.push('$');
                                            push_joined_lines(&mut out, inner);
                                            out.push('$');
                                            i = close + 2;
                                        }
                                    }
                                    // Too far, or unclosed at EOF: lone `$`, no trim.
                                    InlineClose::Unmatched => {
                                        out.push('$');
                                        i += 2;
                                    }
                                    // Hold back from `\(` until the `\)` arrives.
                                    InlineClose::NeedMore => break,
                                },
                                Bs::DisplayOpen { len } => {
                                    match find_display_close(buf, i + len, final_flush) {
                                        DisplayClose::Found { close, close_len } => {
                                            emit_display_span(&mut out, &buf[i + len..close]);
                                            i = close + close_len;
                                        }
                                        // No close in reach: emit the canonical opener
                                        // alone (old position-for-position behavior)
                                        // and process the interior normally.
                                        DisplayClose::Unmatched => {
                                            out.push_str("$$");
                                            i += len;
                                        }
                                        // Hold back from the opener until the close
                                        // (or an abort condition) arrives.
                                        DisplayClose::NeedMore => break,
                                    }
                                }
                                Bs::Convert { to, len } => {
                                    out.push_str(to);
                                    i += len;
                                }
                                Bs::Literal { len } => {
                                    out.push_str(&buf[i..i + len]);
                                    i += len;
                                }
                            }
                            self.at_line_start = false;
                        }
                        b'$' => {
                            let run = count_run(bytes, i, b'$');
                            if run == 1 && i + 1 == n && !final_flush {
                                break; // may become `$$`; hold it back
                            }
                            if run >= 2 {
                                // A display opener is exactly two `$`; any further
                                // `$`s are span content for the close-scan. Consuming
                                // two (not the whole run) keeps emitted spans fixed
                                // points: output like `$` + `$$…$$` re-tokenizes to
                                // the same bytes on a second pass (idempotency).
                                match find_display_close(buf, i + 2, final_flush) {
                                    DisplayClose::Found { close, close_len } => {
                                        emit_display_span(&mut out, &buf[i + 2..close]);
                                        i = close + close_len;
                                    }
                                    // No close in reach: `$$` stays literal (pulldown
                                    // decides), interior is processed normally.
                                    DisplayClose::Unmatched => {
                                        out.push_str("$$");
                                        i += 2;
                                    }
                                    DisplayClose::NeedMore => break,
                                }
                            } else {
                                // Single `$` (inline math / currency) passes through
                                // verbatim; pulldown handles it.
                                out.push('$');
                                i += 1;
                            }
                            self.at_line_start = false;
                        }
                        _ => {
                            // Copy a run of ordinary bytes up to the next
                            // interesting ASCII byte. Multibyte UTF-8 bytes
                            // (>= 0x80) never equal the ASCII delimiters, so
                            // they are copied whole and slices stay valid.
                            let start = i;
                            while i < n && !matches!(bytes[i], b'\n' | b'`' | b'\\' | b'$') {
                                i += 1;
                            }
                            out.push_str(&buf[start..i]);
                            self.at_line_start = false;
                        }
                    }
                }
                State::InlineCode { run } => {
                    // Single-line span: copy verbatim until a matching-length
                    // backtick run closes it, the line ends (unterminated → revert
                    // to Normal so later math still converts), or EOF.
                    let start = i;
                    let mut handled = false;
                    while i < n {
                        match bytes[i] {
                            b'\n' => {
                                i += 1;
                                out.push_str(&buf[start..i]);
                                self.state = State::Normal;
                                self.at_line_start = true;
                                handled = true;
                                break;
                            }
                            b'`' => {
                                let r = count_run(bytes, i, b'`');
                                if i + r == n && !final_flush {
                                    out.push_str(&buf[start..i]);
                                    return (out, i); // hold back the trailing run
                                }
                                if r == run {
                                    i += r;
                                    out.push_str(&buf[start..i]);
                                    self.state = State::Normal;
                                    self.at_line_start = false;
                                    handled = true;
                                    break;
                                }
                                i += r; // non-matching run is literal content
                            }
                            _ => i += 1,
                        }
                    }
                    if !handled {
                        out.push_str(&buf[start..i]); // EOF inside code
                    }
                }
                State::Fenced { ch, len } => {
                    if self.at_line_start {
                        match scan_fence_close(bytes, i, ch, len, final_flush) {
                            FenceScan::NeedMore => break,
                            FenceScan::Match { end, .. } => {
                                out.push_str(&buf[i..end]);
                                i = end;
                                self.state = State::Normal;
                                self.at_line_start = false;
                                continue;
                            }
                            FenceScan::No => {}
                        }
                    }
                    // Copy the rest of this line verbatim (fenced content).
                    let start = i;
                    while i < n && bytes[i] != b'\n' {
                        i += 1;
                    }
                    if i < n {
                        i += 1; // include the newline
                        self.at_line_start = true;
                    } else {
                        self.at_line_start = false;
                    }
                    out.push_str(&buf[start..i]);
                }
            }
        }
        (out, i)
    }
}

/// One-shot normalization == `push(s)` + `finish()`. Used by batch render
/// entries and tests.
pub fn normalize_latex_delimiters(s: &str) -> String {
    let mut nz = LatexDelimiterNormalizer::new();
    let mut out = nz.push(s);
    out.push_str(&nz.finish());
    out
}

fn count_run(bytes: &[u8], start: usize, ch: u8) -> usize {
    let mut j = start;
    while j < bytes.len() && bytes[j] == ch {
        j += 1;
    }
    j - start
}

/// Result of scanning for a fence open/close marker at a line start.
enum FenceScan {
    /// Marker found; `end` is the index just past the run of fence chars.
    Match { ch: u8, len: usize, end: usize },
    /// Definitely not a fence marker here.
    No,
    /// Not enough input to decide; caller should hold back from the line start.
    NeedMore,
}

/// Scan for an opening fence (`` ``` `` / `~~~`, length >= 3) at line start,
/// allowing up to 3 leading spaces. An info string may follow the run.
fn scan_fence_open(bytes: &[u8], i: usize, final_flush: bool) -> FenceScan {
    let n = bytes.len();
    let mut j = i;
    let mut spaces = 0;
    while j < n && bytes[j] == b' ' && spaces < 4 {
        spaces += 1;
        j += 1;
    }
    if spaces >= 4 {
        return FenceScan::No; // indented; not treated as a fence opener
    }
    if j == n {
        return if final_flush {
            FenceScan::No
        } else {
            FenceScan::NeedMore // ≤3 spaces then EOF: a fence may still start
        };
    }
    let ch = bytes[j];
    if ch != b'`' && ch != b'~' {
        return FenceScan::No;
    }
    let run = count_run(bytes, j, ch);
    if j + run == n && !final_flush {
        return FenceScan::NeedMore; // run may extend
    }
    if run < 3 {
        return FenceScan::No; // inline code / stray tildes, not a fence
    }
    FenceScan::Match {
        ch,
        len: run,
        end: j + run,
    }
}

/// Scan for a closing fence at line start: up to 3 spaces, a run of `ch` with
/// length >= `len`, then only whitespace to end of line.
fn scan_fence_close(bytes: &[u8], i: usize, ch: u8, len: usize, final_flush: bool) -> FenceScan {
    let n = bytes.len();
    let mut j = i;
    let mut spaces = 0;
    while j < n && bytes[j] == b' ' && spaces < 4 {
        spaces += 1;
        j += 1;
    }
    if spaces >= 4 {
        return FenceScan::No;
    }
    if j == n {
        return if final_flush {
            FenceScan::No
        } else {
            FenceScan::NeedMore
        };
    }
    if bytes[j] != ch {
        return FenceScan::No;
    }
    let run = count_run(bytes, j, ch);
    if j + run == n && !final_flush {
        return FenceScan::NeedMore; // run may still grow to >= len
    }
    if run < len {
        return FenceScan::No;
    }
    // A close line carries no info string: only trailing whitespace allowed.
    let mut k = j + run;
    while k < n && matches!(bytes[k], b' ' | b'\t') {
        k += 1;
    }
    if k == n {
        return if final_flush {
            FenceScan::Match {
                ch,
                len: run,
                end: j + run,
            }
        } else {
            FenceScan::NeedMore
        };
    }
    if bytes[k] == b'\n' {
        FenceScan::Match {
            ch,
            len: run,
            end: j + run,
        }
    } else {
        FenceScan::No // non-whitespace after the run → info string → content
    }
}

/// Classification of a backslash sequence starting at `i` (where `bytes[i]` is
/// `\`).
enum Bs {
    /// Replace `buf[i..i+len]` with `to`.
    Convert { to: &'static str, len: usize },
    /// An inline math open `\(`: the caller locates the matching `\)` and emits
    /// a whitespace-trimmed `$…$` span (see [`find_inline_close`]).
    InlineOpen,
    /// A display math open (`\[` or `\begin{equation[*]}`, `len` bytes): the
    /// caller locates the matching close and emits a line-joined `$$…$$` span
    /// (see [`find_display_close`]).
    DisplayOpen { len: usize },
    /// Emit `buf[i..i+len]` verbatim (consumes the sequence so escape parity
    /// holds; e.g. `\\` is consumed as a pair).
    Literal { len: usize },
    /// Not enough input to classify; hold back from `i`.
    NeedMore,
}

fn classify_backslash(buf: &str, i: usize, final_flush: bool) -> Bs {
    let bytes = buf.as_bytes();
    let n = bytes.len();
    debug_assert_eq!(bytes[i], b'\\');
    if i + 1 >= n {
        return if final_flush {
            Bs::Literal { len: 1 }
        } else {
            Bs::NeedMore
        };
    }
    match bytes[i + 1] {
        // Escaped backslash: emit the pair so a following `(`/`[` is not read as
        // a delimiter (this is the even/odd parity rule, applied incrementally).
        b'\\' => Bs::Literal { len: 2 },
        // Inline open: the caller scans for the matching `\)` to emit a
        // whitespace-trimmed `$…$`. A lone `\)` (unmatched close) still maps to
        // `$` position-for-position.
        b'(' => Bs::InlineOpen,
        b')' => Bs::Convert { to: "$", len: 2 },
        // Display open: the caller scans for the matching close to emit a
        // line-joined `$$…$$`. A lone `\]` (unmatched close) still maps to
        // `$$` position-for-position.
        b'[' => Bs::DisplayOpen { len: 2 },
        b']' => Bs::Convert { to: "$$", len: 2 },
        b'b' | b'e' => match match_env(buf, i, final_flush) {
            // `\begin{equation[*]}` opens a display span; a stray
            // `\end{equation[*]}` still maps to `$$` position-for-position.
            EnvScan::Convert(len) => {
                if bytes[i + 1] == b'b' {
                    Bs::DisplayOpen { len }
                } else {
                    Bs::Convert { to: "$$", len }
                }
            }
            EnvScan::NeedMore => Bs::NeedMore,
            // Not one of our envs (e.g. `\begin{aligned}`): emit just the `\` and
            // let the rest be copied as ordinary text (verbatim).
            EnvScan::No => Bs::Literal { len: 1 },
        },
        // `\$`, `\x`, etc: emit the `\`, process the next char normally.
        _ => Bs::Literal { len: 1 },
    }
}

/// Outcome of scanning an inline `\(` span for its matching close.
enum InlineClose {
    /// Unescaped `\)` found; `close` is the byte index of its backslash.
    Found { close: usize },
    /// No usable close: either none within the look-ahead cap, or the open is
    /// still unclosed at end of stream. The caller emits a lone `$` (no trim),
    /// reproducing the old position-for-position behavior.
    Unmatched,
    /// Buffer ends within the cap without a close and more input may still
    /// arrive; the caller holds back from the open until the `\)` shows up.
    NeedMore,
}

/// Scan for the unescaped `\)` closing an inline `\(` at `open`
/// (`bytes[open..open + 2] == b"\\("`). The inner length is bounded by
/// [`MAX_MATH_SOURCE_LEN`] (the converter's own input cap) so an unclosed `\(`
/// cannot stall the stream; the bound is a distance relative to `open`, so the
/// Found/Unmatched decision is the same whether the input arrives whole or
/// split. Mirrors [`classify_backslash`]'s `final_flush`: at end of stream an
/// unfound close resolves to `Unmatched` instead of `NeedMore`.
///
/// Backslash parity matches [`classify_backslash`]: `\\` is an escaped pair
/// (its following byte is literal), a lone `\)` is the close, and any other
/// `\x` consumes both bytes as span content.
fn find_inline_close(bytes: &[u8], open: usize, final_flush: bool) -> InlineClose {
    debug_assert!(
        bytes.get(open) == Some(&b'\\') && bytes.get(open + 1) == Some(&b'('),
        "find_inline_close must start at a `\\(`"
    );
    let n = bytes.len();
    let mut k = open + 2;
    while k < n {
        // A close at `k` would give inner `buf[open + 2..k]`; stop once that
        // would exceed what `latex_to_unicode_inline` accepts.
        if k - (open + 2) > MAX_MATH_SOURCE_LEN {
            return InlineClose::Unmatched;
        }
        if bytes[k] == b'\\' {
            match bytes.get(k + 1) {
                None => break, // trailing `\`: need the next byte to classify
                Some(b')') => return InlineClose::Found { close: k },
                Some(_) => k += 2, // `\\` pair or `\x` escape: skip both bytes
            }
        } else {
            k += 1;
        }
    }
    // End of buffer within the cap (or a trailing `\`): unclosed at EOF emits a
    // lone `$`; otherwise hold back for more input.
    if final_flush {
        InlineClose::Unmatched
    } else {
        InlineClose::NeedMore
    }
}

/// Outcome of scanning a display span (opened by `\[`, `$$`, or
/// `\begin{equation[*]}`) for its close.
enum DisplayClose {
    /// Close token found; `close` is its byte index, `close_len` its length.
    Found { close: usize, close_len: usize },
    /// No usable close: past the look-ahead cap, aborted at a blank line or a
    /// blockquote marker, or unclosed at end of stream. The caller emits the
    /// canonical `$$` opener alone (the old position-for-position behavior).
    Unmatched,
    /// Buffer ends without a decision and more input may still arrive; the
    /// caller holds back from the opener.
    NeedMore,
}

/// Scan for the token closing a display span whose content starts at
/// `content_start`. Any display close token counts — `\]`, `$$`, or
/// `\end{equation[*]}` — matching the pre-existing behavior where mismatched
/// opener/close pairs (e.g. `\[ … $$`) still formed a span because every
/// delimiter normalized to `$$` independently.
///
/// The scan is bounded by [`MAX_MATH_SOURCE_LEN`] relative to `content_start`
/// (so the Found/Unmatched decision is split-invariant) and aborts — leaving
/// the source for normal processing — at:
///
/// - a blank line: a paragraph break means the opener was almost certainly not
///   math (e.g. `$$` used as prose), and two stray `$$` must not fuse across
///   paragraphs;
/// - a line starting with `>`: blockquoted display math carries `>` markers
///   that would otherwise be joined into the span as literal content
///   (pulldown handles the quoted multi-line span itself after stripping the
///   markers).
///
/// Backslash parity matches [`find_inline_close`]: `\\` and other `\x` pairs
/// are span content, consumed two bytes at a time.
fn find_display_close(buf: &str, content_start: usize, final_flush: bool) -> DisplayClose {
    let bytes = buf.as_bytes();
    let n = bytes.len();
    let mut k = content_start;
    while k < n {
        if k - content_start > MAX_MATH_SOURCE_LEN {
            return DisplayClose::Unmatched;
        }
        match bytes[k] {
            b'\\' => match bytes.get(k + 1) {
                None => break, // trailing `\`: need the next byte to classify
                Some(b']') => {
                    return DisplayClose::Found {
                        close: k,
                        close_len: 2,
                    };
                }
                Some(b'e') => {
                    // `\end{equation}` / `\end{equation*}` closes the span.
                    let rest = &buf[k..];
                    let mut matched = None;
                    let mut could_extend = false;
                    for tok in [ENV_END, ENV_END_STARRED] {
                        if rest.len() >= tok.len() {
                            if rest.starts_with(tok) {
                                matched =
                                    Some(matched.map_or(tok.len(), |m: usize| m.max(tok.len())));
                            }
                        } else if tok.starts_with(rest) {
                            could_extend = true;
                        }
                    }
                    if let Some(close_len) = matched {
                        return DisplayClose::Found {
                            close: k,
                            close_len,
                        };
                    }
                    if could_extend && !final_flush {
                        return DisplayClose::NeedMore;
                    }
                    k += 2; // `\e…` of something else: span content
                }
                Some(_) => k += 2, // `\\` pair or `\x` escape: span content
            },
            b'$' => {
                let run = count_run(bytes, k, b'$');
                if run >= 2 {
                    return DisplayClose::Found {
                        close: k,
                        close_len: 2,
                    };
                }
                if k + run == n && !final_flush {
                    return DisplayClose::NeedMore; // lone `$` at EOB may extend
                }
                k += run;
            }
            b'\n' => {
                // Look at the next line's start: blank line or `>` marker
                // aborts the span (see doc comment).
                let mut j = k + 1;
                while j < n && matches!(bytes[j], b' ' | b'\t') {
                    j += 1;
                }
                if j == n {
                    break; // need the next line's first byte to decide
                }
                if matches!(bytes[j], b'\n' | b'>') {
                    return DisplayClose::Unmatched;
                }
                k += 1;
            }
            _ => k += 1,
        }
    }
    if final_flush {
        DisplayClose::Unmatched
    } else {
        DisplayClose::NeedMore
    }
}

/// Emit `interior` as a canonical `$$…$$` span, joining interior lines.
///
/// Single-line interiors are emitted verbatim (bare `$$…$$` input passes
/// through byte-for-byte, keeping the pass idempotent). Multi-line interiors
/// have each line trimmed and joined with a single space so CommonMark block
/// parsing (setext underlines, list items, headings) cannot split the span;
/// TeX treats the newlines as spaces, so rendering is unchanged.
fn emit_display_span(out: &mut String, interior: &str) {
    out.push_str("$$");
    push_joined_lines(out, interior);
    out.push_str("$$");
}

/// Push `text` onto `out`; if it spans multiple lines, trim each line and
/// join the non-empty ones with single spaces (single-line text is verbatim).
fn push_joined_lines(out: &mut String, text: &str) {
    if !text.contains('\n') {
        out.push_str(text);
        return;
    }
    let mut first = true;
    for line in text.lines() {
        let trimmed = line.trim_matches([' ', '\t', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(trimmed);
        first = false;
    }
}

enum EnvScan {
    Convert(usize),
    No,
    NeedMore,
}

/// Match `\begin{equation}` / `\end{equation}` (and starred variants) at `i`.
fn match_env(buf: &str, i: usize, final_flush: bool) -> EnvScan {
    let rest = &buf[i..];
    let mut best: Option<usize> = None;
    let mut could_extend = false;
    for tok in ENV_TOKENS {
        if rest.len() >= tok.len() {
            if rest.starts_with(tok) {
                best = Some(best.map_or(tok.len(), |b: usize| b.max(tok.len())));
            }
        } else if tok.starts_with(rest) {
            could_extend = true;
        }
    }
    if let Some(len) = best {
        // `\begin{equation}` is not a prefix of `\begin{equation*}` (char 16 is
        // `}` vs `*`), so the longest full match is unambiguous.
        return EnvScan::Convert(len);
    }
    if could_extend && !final_flush {
        return EnvScan::NeedMore;
    }
    EnvScan::No
}
