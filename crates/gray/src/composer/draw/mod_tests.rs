use super::banner_rects;
use ratatui::layout::Rect;

#[test]
fn banner_blanks_the_whole_row_before_its_inset_message() {
    let area = Rect::new(0, 0, 60, 10);
    let (row, inset) = banner_rects(area, 8, 44, 3);
    // The blank covers the full width: the transcript band and the home toast
    // write this row too, and a narrower blank leaves their text to the left of
    // the message.
    assert_eq!(row, Rect::new(0, 3, 60, 1));
    // The message itself stays inside the box inset.
    assert_eq!(inset, Rect::new(8, 3, 44, 1));
    assert!(row.x <= inset.x && row.right() >= inset.right());
}

#[test]
fn banner_message_clamps_to_the_edge_of_the_area() {
    let area = Rect::new(5, 2, 20, 4);
    let (row, inset) = banner_rects(area, 15, 99, 3);
    assert_eq!(row, Rect::new(5, 3, 20, 1));
    assert_eq!(inset, Rect::new(20, 3, 5, 1));
    // An inset past the right edge degrades to nothing, never a rect that
    // overruns the area.
    let (_, past) = banner_rects(area, 99, 99, 3);
    assert_eq!(past, Rect::new(25, 3, 0, 1));
    assert!(past.right() <= area.right());
}

#[test]
fn blanking_the_row_erases_the_glyphs_a_bare_block_would_keep() {
    use ratatui::buffer::Buffer;
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Paragraph, Widget};

    let area = Rect::new(0, 0, 26, 1);
    let stale = "pluginNo whisper model xx";

    let mut buf = Buffer::empty(area);
    for (i, ch) in stale.chars().enumerate() {
        buf[(i as u16, 0)].set_char(ch);
    }

    // What the banner renders first: spaces across the whole row. A Block only
    // sets cell styles, so an empty Line would leave the stale glyphs in place
    // and the row would still read "pluginNo whisper model ...".
    Paragraph::new(Line::from(Span::raw(" ".repeat(area.width as usize)))).render(area, &mut buf);

    let row: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
    assert_eq!(row, " ".repeat(area.width as usize));
}
