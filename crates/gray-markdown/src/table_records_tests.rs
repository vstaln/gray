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
    let out = render_records(
        &header,
        &rows,
        Some(40),
        Style::new().bold(),
        Style::new().dim(),
    );
    assert_eq!(out.lines[0], " Field  a");
    assert_eq!(out.lines[1], " Value  b");
    assert_eq!(out.line_source_offsets[0], 0);
    assert_eq!(out.line_source_offsets[1], 0);
}
