//! Project parser-emitted `LinkTarget`s onto rendered display cells.
//!
//! Three coordinate systems are in play:
//!
//! 1. **Source bytes** -- offsets into the raw markdown the parser saw.
//!    `LinkTarget::source_range` lives here.
//! 2. **Transformed bytes** -- what `apply_transforms` produces for a *chunk*
//!    of source bytes between two render events.  In pretty mode the
//!    transforms strip `[` and rewrite `](` as ` (`, so transformed bytes
//!    do not line up with source bytes.
//! 3. **Display cells** -- `(line_index, display_column)`.  What
//!    `HyperlinkTarget` exposes for the OSC 8 layer to consume.
//!
//! A chunk's transformed string is split on `\n` into *segments*; one
//! segment becomes one rendered line.  A link spanning multiple segments
//! (a wrapped or autolink-bracketed link) produces one `HyperlinkTarget`
//! per segment, all sharing the same `id`.

use crate::buffers::{LinkTarget, Transform, unicode_display_width};
use crate::output::HyperlinkTarget;

/// One link's projection onto the current chunk's transformed string.
///
/// Returned by `chunk_link_offsets`; bounds are in coordinate system #2
/// (transformed bytes within the chunk), to be mapped onto display cells
/// later by `emit_segment_hyperlinks`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChunkLinkRange {
    /// Start byte (inclusive) within the chunk's transformed string.
    pub(crate) xform_start: usize,
    /// End byte (exclusive) within the chunk's transformed string.
    pub(crate) xform_end: usize,
    /// Index into the `link_targets` slice the caller passed in.
    pub(crate) link_idx: usize,
}

/// Project a source byte position into the chunk's transformed coordinate
/// space (system #1 -> system #2). See file docstring.
///
/// Walks `transforms` in source order, accumulating `(to.len() - range.len())`
/// for every transform fully consumed before `src_pos`.  When the chunk has
/// no transforms (or `pretty` is false), the caller skips this and uses
/// `src_pos - chunk_start` directly.
///
/// **Invariants assumed of the inputs:**
/// 1. `transforms` is sorted by `range.start` (the existing `apply_transforms`
///    relies on the same invariant; the parser pushes transforms in source
///    order).
/// 2. No transform's source range overlaps the bytes a caller intends to
///    locate — i.e. transforms touch *boundary* characters around link text
///    (the `[` and `](` markers), never the link text itself.  All transforms
///    pushed by the parser today (link bracket removal, bullet substitutions)
///    satisfy this; the `debug_assert!` at the call sites in `render_ratatui`
///    enforces it via the cursor invariant.
///
/// **Straddle policy** (when a transform DOES contain `src_pos` despite the
/// invariant above): the source position is clamped to the start of the
/// transform's replacement string.  Both endpoints (start/end) clamp the
/// same direction, so a link whose endpoint straddles a transform produces
/// a column range that excludes the straddling bytes.  This is intentional
/// rather than precise — a future transform that intentionally rewrites
/// link text should add a typed mapping instead of relying on this clamp.
pub(crate) fn source_to_chunk_offset(
    src_pos: usize,
    chunk_start: usize,
    transforms: &[Transform],
) -> usize {
    let mut delta: isize = 0;
    for t in transforms {
        if t.range.end <= chunk_start {
            continue;
        }
        if t.range.start >= src_pos {
            break;
        }
        let t_src_start = t.range.start.max(chunk_start);
        if t.range.end <= src_pos {
            let src_len = (t.range.end - t_src_start) as isize;
            let dst_len = t.to.len() as isize;
            delta += dst_len - src_len;
        } else {
            // Transform straddles src_pos.  See "Straddle policy" above.
            debug_assert!(
                false,
                "source_to_chunk_offset: transform [{}..{}) straddles src_pos {}; \
                 link text should never overlap a transform.  See straddle policy.",
                t.range.start, t.range.end, src_pos,
            );
            let consumed = (src_pos - t_src_start) as isize;
            delta -= consumed;
            break;
        }
    }
    let raw = (src_pos - chunk_start) as isize + delta;
    raw.max(0) as usize
}

/// One `ChunkLinkRange` per link whose source range overlaps
/// `[chunk_start, chunk_end)`.
///
/// `from_idx` is the caller's monotonic cursor: links before this index
/// have already been processed in earlier chunks (see the module doc on
/// the source-order invariant).  Returned bounds live in the chunk's
/// transformed coordinate space.
pub(crate) fn chunk_link_offsets(
    link_targets: &[LinkTarget],
    from_idx: usize,
    chunk_start: usize,
    chunk_end: usize,
    pretty: bool,
    transforms: &[Transform],
) -> Vec<ChunkLinkRange> {
    let mut out = Vec::new();
    for (idx, lt) in link_targets.iter().enumerate().skip(from_idx) {
        if lt.source_range.start >= chunk_end {
            break;
        }
        if lt.source_range.end <= chunk_start {
            continue;
        }
        let src_start = lt.source_range.start.max(chunk_start);
        let src_end = lt.source_range.end.min(chunk_end);
        let (xform_start, xform_end) = if !pretty || transforms.is_empty() {
            (src_start - chunk_start, src_end - chunk_start)
        } else {
            (
                source_to_chunk_offset(src_start, chunk_start, transforms),
                source_to_chunk_offset(src_end, chunk_start, transforms),
            )
        };
        if xform_end > xform_start {
            out.push(ChunkLinkRange {
                xform_start,
                xform_end,
                link_idx: idx,
            });
        }
    }
    out
}

/// Push one `HyperlinkTarget` per `ChunkLinkRange` that overlaps this
/// segment (system #2 -> system #3).
///
/// `seg_x_offset` is where this segment starts within the chunk's
/// transformed string; the caller advances it by `segment.len() + 1`
/// per iteration to account for the `\n` consumed by `split('\n')`.
/// `col` is the running display column on the in-progress line.
pub(crate) fn emit_segment_hyperlinks(
    chunk_links: &[ChunkLinkRange],
    link_targets: &[LinkTarget],
    segment: &str,
    seg_x_offset: usize,
    col: usize,
    line_index: usize,
    out: &mut Vec<HyperlinkTarget>,
) {
    let seg_x_end = seg_x_offset + segment.len();
    for clr in chunk_links {
        if clr.xform_end <= seg_x_offset || clr.xform_start >= seg_x_end {
            continue;
        }
        let s_in = clr
            .xform_start
            .saturating_sub(seg_x_offset)
            .min(segment.len());
        let e_in = (clr.xform_end - seg_x_offset).min(segment.len());
        let s_in = segment.floor_char_boundary(s_in);
        let e_in = segment.ceil_char_boundary(e_in);
        if s_in >= e_in {
            continue;
        }
        let col_start = col + unicode_display_width(&segment[..s_in]);
        let col_end = col_start + unicode_display_width(&segment[s_in..e_in]);
        let lt = &link_targets[clr.link_idx];
        out.push(HyperlinkTarget {
            line_index,
            column_range: col_start..col_end,
            url: lt.url.clone(),
            id: lt.id,
        });
    }
}

#[path = "hyperlink_tests.rs"]
#[cfg(test)]
mod hyperlink_tests;
