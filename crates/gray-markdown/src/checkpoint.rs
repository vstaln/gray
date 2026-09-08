//! Checkpoint types for incremental markdown rendering.
//!
//! This module defines types for identifying stable boundaries in markdown text
//! where rendered output can be "frozen" and cached. Content before a checkpoint
//! will not change regardless of what text is appended after it.
//!
//! # Design
//!
//! Checkpoints are only created at **top-level** (depth=0) block boundaries. Blocks
//! nested inside lists, blockquotes, or tables cannot be checkpoints because the
//! outer container might continue.
//!
//! # Example
//!
//! ```text
//! # Heading          <- Checkpoint after this (heading at depth=0)
//!
//! Paragraph text.    <- Checkpoint after blank line (paragraph at depth=0)
//!
//! - List item        <- NO checkpoint (inside list)
//!   ```code```       <- NO checkpoint (code block inside list)
//! - Another item
//!                    <- Checkpoint here (list closed at depth=0)
//! ```

/// A position in the source text where rendered content can be frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint {
    /// Byte offset in source text (exclusive end of frozen region).
    /// Content in `text[..source_bytes]` can be cached.
    pub source_bytes: usize,
    /// Number of output lines that correspond to this checkpoint.
    /// Lines `0..output_lines` can be frozen.
    pub output_lines: usize,
    /// What kind of block ended at this checkpoint.
    pub kind: CheckpointKind,
}

/// The type of markdown block that created a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointKind {
    /// A heading (any level: h1-h6)
    Heading,
    /// A paragraph followed by a blank line
    Paragraph,
    /// A fenced or indented code block
    CodeBlock,
    /// A blockquote that closed at top level
    BlockQuote,
    /// A list (ordered or unordered) that closed at top level
    List,
    /// A thematic break (horizontal rule: ---, ***, ___)
    ThematicBreak,
    /// A table that closed at top level
    Table,
    /// A raw HTML block
    HtmlBlock,
}

/// Bound for forcing a checkpoint inside a long open block.
///
/// Streaming re-renders the whole unfrozen tail on every push. When no block
/// boundary ever fires (a huge open fence/list/paragraph) each pass costs
/// O(tail) and the stream degrades to O(N²) tail re-parse. A checkpoint forced
/// once the tail passes a size or age bound keeps the tail bounded and the
/// stream ~O(N).
///
/// NOTE (WIRED, container-safe-only): the streaming renderer
/// (`streaming.rs::rerender_tail`) consults [`should_force_checkpoint`] on
/// every pass. When it fires WITH a parser checkpoint the normal advance
/// applies; when it fires WITHOUT one (a huge open fence/list/paragraph)
/// the freeze is deliberately DEFERRED — a tail starting mid-fence/list
/// loses its container context, so naive freezing corrupts the streaming
/// view until `finish()` heals it. True mid-block freezing needs renderer
/// resume support (`render.rs`, sibling-owned); until that lands, the tail
/// inside an open block stays unbounded and these thresholds act as the
/// live policy hook for that work. Advancing a forced boundary must also
/// rebase the incremental caches (`open_code` offsets, `next_link_id`,
/// hyperlink / code-block spans).
pub const FORCED_CHECKPOINT_TAIL_BYTES: usize = 16 * 1024;
/// Max unfrozen output lines before a checkpoint is forced.
pub const FORCED_CHECKPOINT_TAIL_LINES: usize = 200;
/// Max age of an unadvanced frozen boundary before a checkpoint is forced.
pub const FORCED_CHECKPOINT_TAIL_SECS: u64 = 10;

/// Policy check: force a checkpoint once the tail exceeds the size bound
/// (source bytes or output lines) or the frozen boundary is older than the
/// age bound. See the `FORCED_CHECKPOINT_*` NOTE above.
pub fn should_force_checkpoint(
    tail_source_bytes: usize,
    tail_output_lines: usize,
    frozen_age: std::time::Duration,
) -> bool {
    tail_source_bytes >= FORCED_CHECKPOINT_TAIL_BYTES
        || tail_output_lines >= FORCED_CHECKPOINT_TAIL_LINES
        || frozen_age >= std::time::Duration::from_secs(FORCED_CHECKPOINT_TAIL_SECS)
}

#[cfg(test)]
mod unrun_forced_checkpoint_tests {
    use super::*;

    // UNRUN: marked #[ignore] — run explicitly, never in bulk.
    // (`cargo test` under X kills the session; see repo AGENTS.md.)
    // Run: `cargo test -p gray-markdown forced_checkpoint -- --ignored`

    #[test]
    #[ignore]
    fn unrun_force_on_tail_bytes() {
        assert!(should_force_checkpoint(
            FORCED_CHECKPOINT_TAIL_BYTES,
            0,
            std::time::Duration::ZERO
        ));
        assert!(!should_force_checkpoint(
            FORCED_CHECKPOINT_TAIL_BYTES - 1,
            0,
            std::time::Duration::ZERO
        ));
    }

    #[test]
    #[ignore]
    fn unrun_force_on_tail_lines() {
        assert!(should_force_checkpoint(
            0,
            FORCED_CHECKPOINT_TAIL_LINES,
            std::time::Duration::ZERO
        ));
        assert!(!should_force_checkpoint(
            0,
            FORCED_CHECKPOINT_TAIL_LINES - 1,
            std::time::Duration::ZERO
        ));
    }

    #[test]
    #[ignore]
    fn unrun_force_on_frozen_age() {
        assert!(should_force_checkpoint(
            0,
            0,
            std::time::Duration::from_secs(FORCED_CHECKPOINT_TAIL_SECS)
        ));
        assert!(!should_force_checkpoint(
            0,
            0,
            std::time::Duration::from_secs(FORCED_CHECKPOINT_TAIL_SECS - 1)
        ));
    }
}
