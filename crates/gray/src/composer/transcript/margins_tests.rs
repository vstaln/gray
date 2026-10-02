use super::*;
use ratatui::style::{Color, Style};

fn text(s: &str) -> Line<'static> {
    Line::from(s.to_string())
}

fn link(line_index: usize) -> HyperlinkTarget {
    HyperlinkTarget {
        line_index,
        column_range: 0..1,
        url: "https://example.com".into(),
        id: 0,
    }
}

#[test]
fn edge_blanks_become_boundary_marks() {
    let shaped = shape_block(
        vec![text(""), text(""), text("a"), text("b"), text("")],
        vec![link(2), link(4)],
    );
    assert!(shaped.lead);
    assert!(shaped.trail);
    assert_eq!(shaped.lines, vec![text("a"), text("b")]);
    // The link on `a` moves with its row; the one on the trailing blank goes.
    assert_eq!(shaped.hyperlinks, vec![link(0)]);
}

#[test]
fn inner_blanks_are_content_and_stay() {
    // A code block's empty lines: the funnel never collapses them.
    let rows = vec![text("let a = 1;"), text(""), text(""), text("let b = 2;")];
    let shaped = shape_block(rows.clone(), vec![]);
    assert!(!shaped.lead && !shaped.trail);
    assert_eq!(shaped.lines, rows);
}

#[test]
fn an_all_blank_block_is_only_a_boundary() {
    let shaped = shape_block(vec![text(""), text("  ")], vec![]);
    assert!(shaped.lead);
    assert!(shaped.lines.is_empty());
}

#[test]
fn painted_rows_are_edges_not_gaps() {
    // A card's padding row is part of the card: never lifted out.
    let bg = Style::default().bg(Color::Rgb(22, 22, 22));
    let rows = vec![text("").style(bg), text("card"), text("").style(bg)];
    let shaped = shape_block(rows.clone(), vec![]);
    assert!(!shaped.lead && !shaped.trail);
    assert_eq!(shaped.lines, rows);
}

#[test]
fn a_gap_is_paid_only_above_content() {
    assert!(gap_due(true, &[text("a")]));
    assert!(!gap_due(false, &[text("a")]), "nothing owed");
    assert!(!gap_due(true, &[]), "top of an empty transcript");
    assert!(
        !gap_due(true, &[text("a"), text("")]),
        "a blank tail (the welcome banner's) already is the gap"
    );
}

#[test]
fn junction_counts_blanks_across_the_seam() {
    assert_eq!(junction_blank_run(&[text("a")], &[text("b")]), 0);
    assert_eq!(junction_blank_run(&[text("a")], &[text(""), text("b")]), 1);
    assert_eq!(junction_blank_run(&[text("a"), text("")], &[text("b")]), 1);
    assert_eq!(
        junction_blank_run(&[text("a"), text("")], &[text(""), text("b")]),
        2,
        "the doubled margin"
    );
}

/// The `Tui` funnel without a terminal: `ensure_gap` is `boundary`, every
/// commit is `block`, and both decide through the same `admit`.
#[derive(Default)]
struct Session {
    rows: Vec<Line<'static>>,
    owed: bool,
}

impl Session {
    fn boundary(&mut self) {
        self.owed = true;
    }

    fn block(&mut self, rows: Vec<Line<'static>>) {
        let shaped = shape_block(rows, vec![]);
        let mut write = Vec::new();
        if admit(&mut self.owed, &self.rows, &shaped) {
            write.push(Line::default());
        }
        write.extend(shaped.lines);
        assert!(
            junction_blank_run(&self.rows, &write) <= 1,
            "doubled margin writing {write:?} under {:?}",
            self.rows.last()
        );
        self.rows.extend(write);
    }

    fn texts(&self) -> Vec<String> {
        self.rows.iter().map(|l| l.to_string()).collect()
    }
}

fn card(title: &str) -> Vec<Line<'static>> {
    let bg = Style::default().bg(Color::Rgb(22, 22, 22));
    vec![
        text("").style(bg),
        text(title).style(bg),
        text("").style(bg),
    ]
}

/// A whole turn: prompt card, two thought paragraphs, an answer streamed in
/// markdown chunks with a code block, a tool card, the footer. Every block
/// boundary comes out as exactly one unpainted row, whichever writer marked
/// it, however many times, and however the content was chunked.
#[test]
fn a_turn_has_exactly_one_row_between_blocks() {
    let mut s = Session::default();
    s.block(vec![text("gray · Run /help"), text("")]); // welcome ends blank
    s.boundary();
    s.block(card("❯ fix it"));
    s.boundary();
    // Thinking starts: a boundary, then fragments; `\n\n` is a blank one.
    s.boundary();
    s.block(vec![text("First thought.")]);
    s.block(vec![text("")]);
    // Mid-pause the tail is content and a gap is owed: the band's seam is it.
    assert_eq!(s.texts().last().unwrap(), "First thought.");
    assert!(s.owed);
    s.block(vec![text("Second thought.")]);
    s.boundary(); // thinking ends
    // The answer: the renderer's frozen chunks carry edge blanks.
    s.boundary();
    s.block(vec![text("Para one."), text("")]);
    s.block(vec![
        text(""),
        text("let a = 1;"),
        text(""),
        text(""),
        text("let b = 2;"),
    ]);
    s.boundary();
    s.boundary(); // a second mark is no second gap
    s.block(card("⬢ Ran cargo test"));
    s.boundary();
    s.block(vec![text("✻ Thought for 3s")]);
    s.boundary();

    assert_eq!(
        s.texts(),
        vec![
            "gray · Run /help",
            "",
            "",
            "❯ fix it",
            "",
            "",
            "First thought.",
            "",
            "Second thought.",
            "",
            "Para one.",
            "",
            "let a = 1;",
            "",
            "",
            "let b = 2;",
            "",
            "",
            "⬢ Ran cargo test",
            "",
            "",
            "✻ Thought for 3s",
        ]
    );
    // The two blank code rows are content; everything else is one row.
    // (Card rows above: painted padding, gap, painted padding, title, ...)
    let bg = Some(Color::Rgb(22, 22, 22));
    assert_eq!(s.rows[1].style.bg, None, "welcome's own gap");
    assert_eq!(s.rows[2].style.bg, bg, "card padding, not a second gap");
    assert_eq!(s.rows[6].style.bg, None);
    assert!(!transcript_row_is_blank(s.rows.last().unwrap()));
    assert!(
        s.owed,
        "the trailing gap is owed, for the band's seam to show"
    );
}

/// The live stream and a resize reflow see the same blocks cut differently
/// (fragments vs one run, chunks vs one block): the margins must not move.
#[test]
fn chunking_does_not_move_a_margin() {
    let mut live = Session::default();
    live.block(vec![text("a")]);
    live.block(vec![text("")]);
    live.block(vec![text("")]);
    live.block(vec![text("b"), text("")]);
    live.block(vec![text(""), text("c")]);

    let mut reflow = Session::default();
    reflow.block(vec![text("a"), text(""), text("b"), text(""), text("c")]);

    assert_eq!(live.texts(), reflow.texts());
}

#[test]
fn a_boundary_with_nothing_after_it_writes_nothing() {
    let mut s = Session::default();
    s.block(vec![text("last")]);
    s.boundary();
    s.block(vec![text(""), text("")]);
    assert_eq!(s.texts(), vec!["last"], "no trailing blank, ever");
}
