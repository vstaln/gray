use crate::buffers::{LinkTarget, Transform, unicode_display_width};
use crate::output::HyperlinkTarget;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChunkLinkRange {
    pub(crate) xform_start: usize,

    pub(crate) xform_end: usize,

    pub(crate) link_idx: usize,
}

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
