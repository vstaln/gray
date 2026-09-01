//! Vertical key/value fallback for markdown tables that no longer scan well
//! as grids. Port of codex-rs `tui/src/markdown_render/table_key_value.rs`
//! (decision heuristics + record layout), adapted to gray's plain-string
//! table model.
//!
//! `# ponytail` ceilings vs the codex original:
//! - cell values render as plain text (codex keeps per-span styles via
//!   HyperlinkLine remapping) — inline code/links inside record values lose
//!   their styling;
//! - no hyperlink remapping (gray's table fallback already pushes empty
//!   hyperlinks).

use crate::buffers::{StyledCell, unicode_display_width};
use crate::parse::wrap_cell_text;
use ratatui::text::{Line, Span};

const FIELD_LEADING_PADDING: usize = 1;
const FIELD_GAP: usize = 2;
const MIN_VALUE_WIDTH: usize = 3;
const MIN_ALIGNED_COMPACT_VALUE_WIDTH: usize = 12;
const MIN_ALIGNED_EXPANSIVE_VALUE_WIDTH: usize = 24;
const MIN_SCANNABLE_NARRATIVE_WIDTH: usize = 12;
const MIN_SCANNABLE_TOKEN_HEAVY_WIDTH: usize = 12;
const CRAMPED_EXPANSIVE_CELL_LINES: usize = 4;
const CATASTROPHIC_NARRATIVE_CELL_LINES: usize = 7;
const STACKED_VALUE_INDENT: usize = 2;

#[derive(Clone, Copy, PartialEq)]
enum ColumnKind {
    Compact,
    TokenHeavy,
    Narrative,
}

struct ColumnMetrics {
    kind: ColumnKind,
}

/// Per-column classification, port of codex `collect_table_column_metrics`
/// trimmed to the fields the decision heuristics actually read.
fn collect_metrics(header: &[StyledCell], rows: &[Vec<StyledCell>], num_cols: usize) -> Vec<ColumnMetrics> {
    (0..num_cols)
        .map(|col| {
            let header_plain = header.get(col).map(|c| c.plain_text()).unwrap_or_default();
            let mut body_token_count = 0usize;
            let mut long_body_token_count = 0usize;
            let mut total_words = 0usize;
            let mut total_cells = 0usize;
            let mut total_cell_width = 0usize;
            for row in rows {
                let Some(cell) = row.get(col) else { continue };
                let plain = cell.plain_text();
                let mut word_count = 0usize;
                for token in plain.split_whitespace() {
                    long_body_token_count += usize::from(unicode_display_width(token) >= 20);
                    word_count += 1;
                }
                if word_count > 0 {
                    body_token_count += word_count;
                    total_words += word_count;
                    total_cells += 1;
                    total_cell_width += unicode_display_width(&plain);
                }
            }
            let avg_words_per_cell = if total_cells == 0 {
                header_plain.split_whitespace().count() as f64
            } else {
                total_words as f64 / total_cells as f64
            };
            let avg_cell_width = if total_cells == 0 {
                unicode_display_width(&header_plain) as f64
            } else {
                total_cell_width as f64 / total_cells as f64
            };
            let kind = if long_body_token_count > 0
                && long_body_token_count >= body_token_count.saturating_sub(long_body_token_count)
            {
                ColumnKind::TokenHeavy
            } else if avg_words_per_cell >= 4.0 || avg_cell_width >= 28.0 {
                ColumnKind::Narrative
            } else {
                ColumnKind::Compact
            };
            ColumnMetrics { kind }
        })
        .collect()
}

fn wrapped_cell_height(cell: &StyledCell, width: usize) -> usize {
    wrap_cell_text(&cell.plain_text(), width.max(1)).len()
}

/// Switch to record layout once enough rows contain values the grid can no
/// longer present in useful chunks. Port of codex `should_render_records`.
pub(crate) fn should_render_records(
    header: &[StyledCell],
    rows: &[Vec<StyledCell>],
    column_widths: &[usize],
) -> bool {
    if rows.is_empty() || column_widths.is_empty() {
        return false;
    }
    let num_cols = column_widths.len();
    let metrics = collect_metrics(header, rows, num_cols);
    let affected_rows = rows
        .iter()
        .filter(|row| {
            let contains_fragmented_value = row.iter().enumerate().any(|(col, cell)| {
                let Some(width) = column_widths.get(col) else { return false };
                let Some(m) = metrics.get(col) else { return false };
                let has_fragmented_token = cell
                    .plain_text()
                    .split_whitespace()
                    .any(|token| unicode_display_width(token) > *width);
                match m.kind {
                    ColumnKind::Compact => has_fragmented_token,
                    ColumnKind::TokenHeavy => {
                        *width < MIN_SCANNABLE_TOKEN_HEAVY_WIDTH && has_fragmented_token
                    }
                    ColumnKind::Narrative => false,
                }
            });
            contains_fragmented_value || expansive_cells_are_starved(row, column_widths, &metrics)
        })
        .count();
    let threshold = if rows.len() == 1 {
        1
    } else {
        2.max(rows.len().div_ceil(3))
    };
    affected_rows >= threshold
}

fn expansive_cells_are_starved(
    row: &[StyledCell],
    column_widths: &[usize],
    metrics: &[ColumnMetrics],
) -> bool {
    let expansive_cells: Vec<(ColumnKind, usize, usize)> = row
        .iter()
        .enumerate()
        .filter(|(col, _)| metrics.get(*col).is_some_and(|m| m.kind != ColumnKind::Compact))
        .filter_map(|(col, cell)| {
            Some((
                metrics.get(col)?.kind,
                *column_widths.get(col)?,
                wrapped_cell_height(cell, *column_widths.get(col)?),
            ))
        })
        .collect();

    expansive_cells
        .iter()
        .filter(|(_, _, height)| *height >= CRAMPED_EXPANSIVE_CELL_LINES)
        .count()
        >= 2
        || expansive_cells.iter().any(|(kind, width, height)| {
            *kind == ColumnKind::Narrative
                && *width < MIN_SCANNABLE_NARRATIVE_WIDTH
                && *height >= CATASTROPHIC_NARRATIVE_CELL_LINES
        })
}

/// Rendered record layout: parallel plain/styled lines plus best-effort
/// source-line offsets (labels → header line 0, separators → line 1,
/// values → body row offset `2 + row_index`).
pub(crate) struct RecordTable {
    pub lines: Vec<String>,
    pub styled_lines: Vec<Line<'static>>,
    pub line_source_offsets: Vec<usize>,
}

fn push_line(out: &mut RecordTable, spans: Vec<Span<'static>>, offset: usize) {
    out.lines
        .push(spans.iter().map(|s| s.content.as_ref()).collect::<String>());
    out.styled_lines.push(Line::from(spans));
    out.line_source_offsets.push(offset);
}

/// Port of codex `table_key_value::render_records`: aligned `label  value`
/// fields when the width allows, stacked label-over-value blocks otherwise,
/// with a separator line between records.
pub(crate) fn render_records(
    header: &[StyledCell],
    rows: &[Vec<StyledCell>],
    available_width: Option<usize>,
    label_style: ratatui::style::Style,
    separator_style: ratatui::style::Style,
) -> RecordTable {
    let num_cols = header.len();
    let metrics = collect_metrics(header, rows, num_cols);
    let label_width = header
        .iter()
        .map(|h| unicode_display_width(&h.plain_text()))
        .max()
        .unwrap_or(0);
    let minimum_value_width = if metrics
        .iter()
        .any(|m| m.kind != ColumnKind::Compact)
    {
        MIN_ALIGNED_EXPANSIVE_VALUE_WIDTH
    } else {
        MIN_ALIGNED_COMPACT_VALUE_WIDTH
    };
    let aligned_fields = available_width.is_none_or(|width| {
        FIELD_LEADING_PADDING + label_width + FIELD_GAP + minimum_value_width <= width
    });

    let mut out = RecordTable {
        lines: Vec::new(),
        styled_lines: Vec::new(),
        line_source_offsets: Vec::new(),
    };

    for (row_index, row) in rows.iter().enumerate() {
        let row_offset = 2 + row_index;
        for (col, (h, value)) in header.iter().zip(row.iter()).enumerate() {
            let _ = metrics.get(col);
            let label = h.plain_text();
            if aligned_fields {
                let value_indent = FIELD_LEADING_PADDING + label_width + FIELD_GAP;
                let value_width = available_width
                    .map(|width| width.saturating_sub(value_indent).max(MIN_VALUE_WIDTH))
                    .unwrap_or_else(|| {
                        value
                            .plain_text()
                            .split('\n')
                            .map(unicode_display_width)
                            .max()
                            .unwrap_or(0)
                            .max(MIN_VALUE_WIDTH)
                    });
                for (line_index, value_line) in
                    wrap_cell_text(&value.plain_text(), value_width).into_iter().enumerate()
                {
                    let mut spans = Vec::new();
                    if line_index == 0 {
                        spans.push(Span::raw(" ".repeat(FIELD_LEADING_PADDING)));
                        spans.push(Span::styled(label.clone(), label_style));
                        spans.push(Span::raw(" ".repeat(
                            label_width.saturating_sub(unicode_display_width(&label)) + FIELD_GAP,
                        )));
                    } else {
                        spans.push(Span::raw(" ".repeat(value_indent)));
                    }
                    spans.push(Span::raw(value_line));
                    push_line(&mut out, spans, if line_index == 0 { 0 } else { row_offset });
                }
            } else {
                let label_width_avail = available_width
                    .map(|width| width.saturating_sub(FIELD_LEADING_PADDING).max(1))
                    .unwrap_or_else(|| unicode_display_width(&label).max(1));
                for label_line in wrap_cell_text(&label, label_width_avail) {
                    let mut spans = vec![Span::raw(" ".repeat(FIELD_LEADING_PADDING))];
                    spans.push(Span::styled(label_line, label_style));
                    push_line(&mut out, spans, 0);
                }
                let value_width = available_width
                    .map(|width| width.saturating_sub(STACKED_VALUE_INDENT).max(1))
                    .unwrap_or(1);
                for value_line in wrap_cell_text(&value.plain_text(), value_width) {
                    push_line(
                        &mut out,
                        vec![
                            Span::raw(" ".repeat(STACKED_VALUE_INDENT)),
                            Span::raw(value_line),
                        ],
                        row_offset,
                    );
                }
            }
        }
        if row_index + 1 < rows.len() {
            let width = available_width.unwrap_or_else(|| {
                out.lines
                    .iter()
                    .map(|l| unicode_display_width(l))
                    .max()
                    .unwrap_or(0)
            });
            push_line(
                &mut out,
                vec![Span::styled("─".repeat(width), separator_style)],
                1,
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Style, Stylize};

    fn cell(text: &str) -> StyledCell {
        let mut c = StyledCell::new();
        c.spans.push(crate::buffers::CellSpan::new(
            text.to_string(),
            false,
            false,
            false,
            None,
        ));
        c
    }

    #[test]
    fn wide_natural_table_stays_grid() {
        let header = vec![cell("Name"), cell("Amount")];
        let rows = vec![vec![cell("alpha"), cell("1,000")]];
        let widths = vec![8, 8];
        assert!(!should_render_records(&header, &rows, &widths));
    }

    #[test]
    fn fragmented_compact_column_switches_to_records() {
        // Single-row table: one unbreakable token far wider than its column.
        let header = vec![cell("ID"), cell("Name")];
        let rows = vec![vec![cell("ID-AA1001001001"), cell("ok")]];
        let widths = vec![4, 8];
        assert!(should_render_records(&header, &rows, &widths));
    }

    #[test]
    fn records_render_label_value_pairs() {
        let header = vec![cell("Field"), cell("Value")];
        let rows = vec![vec![cell("a"), cell("b")]];
        let out = render_records(&header, &rows, Some(40), Style::new().bold(), Style::new().dim());
        assert_eq!(out.lines[0], " Field Value");
        assert_eq!(out.lines[1], "       a b");
        assert_eq!(out.line_source_offsets[0], 0);
        assert_eq!(out.line_source_offsets[1], 2);
    }
}
