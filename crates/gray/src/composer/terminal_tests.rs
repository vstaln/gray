use std::cell::RefCell;
use std::rc::Rc;

use ratatui::backend::CrosstermBackend;
use ratatui::style::Style;

use super::*;

#[derive(Clone, Default)]
struct Tape(Rc<RefCell<Vec<u8>>>);

impl Write for Tape {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Tape {
    fn len(&self) -> usize {
        self.0.borrow().len()
    }

    fn since(&self, mark: usize) -> String {
        String::from_utf8_lossy(&self.0.borrow()[mark..]).into_owned()
    }
}

struct Screen {
    w: usize,
    h: usize,

    rows: Vec<Vec<char>>,
    scrollback: Vec<String>,
    x: usize,
    y: usize,
    top: usize,
    bottom: usize,
}

impl Screen {
    fn replay(tape: &Tape, w: u16, h: u16) -> Self {
        let (w, h) = (usize::from(w), usize::from(h));
        let mut s = Screen {
            w,
            h,
            rows: vec![vec![' '; w]; h],
            scrollback: Vec::new(),
            x: 0,
            y: 0,
            top: 0,
            bottom: h - 1,
        };
        let bytes = tape.0.borrow();
        let text = String::from_utf8_lossy(&bytes);
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        let mut fin = ' ';
                        for c in chars.by_ref() {
                            if ('\x40'..='\x7e').contains(&c) {
                                fin = c;
                                break;
                            }
                            params.push(c);
                        }
                        s.csi(&params, fin);
                    }

                    Some(']') => {
                        while let Some(c) = chars.next() {
                            if c == '\x07' {
                                break;
                            }
                            if c == '\x1b' {
                                chars.next();
                                break;
                            }
                        }
                    }
                    Some('M') => s.reverse_index(),
                    _ => {}
                },
                '\r' => s.x = 0,
                '\n' => s.line_feed(),
                c if c < ' ' => {}
                c => s.print(c),
            }
        }
        s
    }

    fn csi(&mut self, params: &str, fin: char) {
        if params.starts_with('?') {
            return;
        }
        let nums: Vec<usize> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
        let raw = |i: usize| nums.get(i).copied().unwrap_or(0);
        let at_least_1 = |i: usize| raw(i).max(1);
        match fin {
            'H' | 'f' => {
                self.y = (at_least_1(0) - 1).min(self.h - 1);
                self.x = (at_least_1(1) - 1).min(self.w - 1);
            }
            'r' => {
                let (top, bottom) = if params.is_empty() {
                    (1, self.h)
                } else {
                    (at_least_1(0), nums.get(1).copied().unwrap_or(self.h))
                };

                if top < bottom && bottom <= self.h {
                    self.top = top - 1;
                    self.bottom = bottom - 1;
                }
                self.x = 0;
                self.y = 0;
            }
            'J' => match raw(0) {
                0 => {
                    let (x, y) = (self.x.min(self.w), self.y);
                    self.rows[y][x..].fill(' ');
                    for row in &mut self.rows[y + 1..] {
                        row.fill(' ');
                    }
                }
                2 => {
                    for row in &mut self.rows {
                        row.fill(' ');
                    }
                }
                3 => self.scrollback.clear(),
                _ => {}
            },
            'K' => {
                let x = self.x.min(self.w);
                self.rows[self.y][x..].fill(' ');
            }
            'G' => self.x = (at_least_1(0) - 1).min(self.w - 1),
            'L' if (self.top..=self.bottom).contains(&self.y) => {
                for _ in 0..at_least_1(0) {
                    self.rows.remove(self.bottom);
                    self.rows.insert(self.y, vec![' '; self.w]);
                }
            }
            'M' if (self.top..=self.bottom).contains(&self.y) => {
                for _ in 0..at_least_1(0) {
                    self.rows.remove(self.y);
                    self.rows.insert(self.bottom, vec![' '; self.w]);
                }
            }
            'S' => self.scroll_up(at_least_1(0)),
            'T' => self.scroll_down(at_least_1(0)),

            _ => {}
        }
    }

    fn line_feed(&mut self) {
        if self.y == self.bottom {
            self.scroll_up(1);
        } else if self.y + 1 < self.h {
            self.y += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.y == self.top {
            self.scroll_down(1);
        } else if self.y > 0 {
            self.y -= 1;
        }
    }

    fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            let row = self.rows.remove(self.top);
            if self.top == 0 {
                self.scrollback.push(text(&row));
            }
            self.rows.insert(self.bottom, vec![' '; self.w]);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        for _ in 0..n {
            self.rows.remove(self.bottom);
            self.rows.insert(self.top, vec![' '; self.w]);
        }
    }

    fn print(&mut self, c: char) {
        let width = crate::text_width::char_width(c);
        if width == 0 {
            return;
        }
        if self.x + width > self.w {
            self.x = 0;
            self.line_feed();
        }
        self.rows[self.y][self.x] = c;
        if width == 2 {
            self.rows[self.y][self.x + 1] = '\0';
        }
        self.x += width;
    }

    fn lines(&self) -> Vec<String> {
        self.rows.iter().map(|r| text(r)).collect()
    }

    /// Scrollback plus the rows above the band.
    fn transcript(&self, band_top: u16) -> Vec<String> {
        let mut all = self.scrollback.clone();
        all.extend(self.lines().into_iter().take(usize::from(band_top)));
        all
    }
}

fn text(row: &[char]) -> String {
    row.iter()
        .filter(|c| **c != '\0')
        .collect::<String>()
        .trim_end()
        .to_string()
}

type Term = CustomTerminal<CrosstermBackend<Tape>>;

const W: u16 = 20;

fn term(h: u16, y: u16, scrollback: Scrollback) -> (Term, Tape) {
    let tape = Tape::default();
    let term = CustomTerminal::at_row(
        CrosstermBackend::new(tape.clone()),
        Size::new(W, h),
        y,
        scrollback,
    );
    (term, tape)
}

fn insert(term: &mut Term, rows: &[&str]) {
    term.insert_before(rows.len() as u16, |buf| {
        for (i, row) in rows.iter().enumerate() {
            buf.set_string(0, i as u16, *row, Style::default());
        }
    })
    .unwrap();
}

fn band(term: &mut Term, height: u16) {
    let screen = term.last_known_screen_size;
    term.set_viewport_height(height, 0, screen).unwrap();
    term.draw(|frame| {
        let area = frame.area();
        for i in 0..area.height {
            let label = if i + 2 == area.height {
                "❯".to_string()
            } else {
                format!("band{i}")
            };
            frame
                .buffer
                .set_string(area.x, area.y + i, label, Style::default());
        }
    })
    .unwrap();
}

fn band_rows(height: u16) -> Vec<String> {
    (0..height)
        .map(|i| {
            if i + 2 == height {
                "❯".to_string()
            } else {
                format!("band{i}")
            }
        })
        .collect()
}

fn rows(range: std::ops::Range<usize>) -> Vec<String> {
    range.map(|i| format!("r{i}")).collect()
}

fn trimmed(rows: &[String]) -> &[String] {
    let start = rows
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(rows.len());
    let end = rows
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(start, |i| i + 1);
    &rows[start..end]
}

#[test]
fn the_band_sits_under_a_short_transcript() {
    let (mut term, tape) = term(16, 0, Scrollback::Regions);
    insert(&mut term, &["banner", "", "/resume"]);
    band(&mut term, 4);

    let screen = Screen::replay(&tape, W, 16);
    let mut want = vec!["banner".to_string(), String::new(), "/resume".to_string()];
    want.extend(band_rows(4));
    assert_eq!(screen.lines()[..7], want[..]);
    assert!(screen.lines()[7..].iter().all(String::is_empty));
    assert_eq!(term.viewport_area, Rect::new(0, 3, W, 4));
}

#[test]
fn a_shrunk_grounded_band_returns_to_the_bottom() {
    let (mut term, tape) = term(10, 0, Scrollback::Regions);
    let history: Vec<String> = rows(0..20);
    insert(
        &mut term,
        &history.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    band(&mut term, 4);
    assert_eq!(term.viewport_area, Rect::new(0, 6, W, 4));

    // A modal/popup grows the band, scrolling r14..r17 into scrollback; when
    // it closes the band drops back to the screen bottom. The transcript
    // stays put: sliding it down would open blank rows at the top, a tear
    // between r17 in scrollback and r18 on screen.
    band(&mut term, 8);
    assert_eq!(term.viewport_area, Rect::new(0, 2, W, 8));
    band(&mut term, 4);
    assert_eq!(term.viewport_area, Rect::new(0, 6, W, 4));

    let screen = Screen::replay(&tape, W, 10);
    assert_eq!(screen.scrollback, rows(0..18));
    assert_eq!(screen.lines()[..2], rows(18..20)[..]);
    assert!(screen.lines()[2..6].iter().all(String::is_empty));
    assert_eq!(screen.lines()[6..], band_rows(4)[..]);
}

#[test]
fn the_banner_lands_on_the_cursor_row_and_the_band_follows_it() {
    let (mut term, tape) = term(16, 5, Scrollback::Regions);
    insert(&mut term, &["logo", "version"]);
    band(&mut term, 4);
    let lines = Screen::replay(&tape, W, 16).lines();
    assert_eq!(lines[5], "logo");
    assert_eq!(lines[6], "version");
    assert_eq!(lines[7..11], band_rows(4)[..]);
}

#[test]
fn an_insert_shifts_the_band_down_without_repainting_it() {
    let (mut term, tape) = term(16, 0, Scrollback::Regions);
    insert(&mut term, &["banner"]);
    band(&mut term, 4);

    let mark = tape.len();
    insert(&mut term, &["x", "y"]);
    band(&mut term, 4);
    let written = tape.since(mark);
    assert!(
        !written.contains("band"),
        "the band was repainted: {written:?}"
    );
    assert!(
        !written.contains('❯'),
        "the prompt was repainted: {written:?}"
    );

    let lines = Screen::replay(&tape, W, 16).lines();
    assert_eq!(lines[..3], ["banner", "x", "y"]);
    assert_eq!(lines[3..7], band_rows(4)[..]);
}

#[test]
fn on_the_bottom_rows_scroll_into_history_in_order() {
    let (mut term, tape) = term(8, 0, Scrollback::Regions);
    band(&mut term, 3);
    for i in 0..12 {
        insert(&mut term, &[format!("r{i}").as_str()]);
        band(&mut term, 3);
    }
    let screen = Screen::replay(&tape, W, 8);
    assert_eq!(screen.transcript(5), rows(0..12));
    assert_eq!(screen.lines()[5..], band_rows(3)[..]);
    assert_eq!(term.viewport_area, Rect::new(0, 5, W, 3));
}

#[test]
fn a_batch_taller_than_the_screen_keeps_its_order() {
    for scrollback in [Scrollback::Regions, Scrollback::FullScreen] {
        let (mut term, tape) = term(8, 0, scrollback);
        band(&mut term, 3);
        let batch = rows(0..20);
        let refs: Vec<&str> = batch.iter().map(String::as_str).collect();
        insert(&mut term, &refs);
        band(&mut term, 3);

        let screen = Screen::replay(&tape, W, 8);
        assert_eq!(screen.transcript(5), batch, "{scrollback:?}");
        assert_eq!(screen.lines()[5..], band_rows(3)[..], "{scrollback:?}");
    }
}

#[test]
fn growth_and_shrink_keep_the_band_top() {
    let (mut term, tape) = term(16, 0, Scrollback::Regions);
    insert(&mut term, &["logo", "", "version"]);
    for _ in 0..3 {
        band(&mut term, 4);
        assert_eq!(term.viewport_area, Rect::new(0, 3, W, 4));
        band(&mut term, 9);
        assert_eq!(term.viewport_area, Rect::new(0, 3, W, 9));
    }
    band(&mut term, 4);

    let screen = Screen::replay(&tape, W, 16);
    assert_eq!(screen.lines()[..3], ["logo", "", "version"]);
    assert_eq!(screen.lines()[3..7], band_rows(4)[..]);
    assert!(
        screen.lines()[7..].iter().all(String::is_empty),
        "the popup's rows were left behind: {:?}",
        screen.lines()
    );
    assert!(screen.scrollback.is_empty());
}

#[test]
fn growth_past_the_bottom_scrolls_only_the_overflow() {
    let (mut term, tape) = term(10, 0, Scrollback::Regions);
    let transcript = rows(0..6);
    let refs: Vec<&str> = transcript.iter().map(String::as_str).collect();
    insert(&mut term, &refs);
    band(&mut term, 4);
    assert_eq!(term.viewport_area, Rect::new(0, 6, W, 4));

    band(&mut term, 6);
    assert_eq!(term.viewport_area, Rect::new(0, 4, W, 6));
    let screen = Screen::replay(&tape, W, 10);
    assert_eq!(screen.scrollback, rows(0..2));
    assert_eq!(screen.transcript(4), transcript);
    assert_eq!(screen.lines()[4..], band_rows(6)[..]);
}

#[test]
fn a_grounded_shrink_drops_the_band_back_to_the_bottom() {
    let (mut term, tape) = term(10, 0, Scrollback::Regions);
    insert(&mut term, &["r0", "r1", "r2", "r3"]);
    band(&mut term, 6);
    band(&mut term, 4);
    // the band was pinned to the screen bottom, so it stays pinned: it
    // drops back down and the vacated rows wait above it for the next insert
    assert_eq!(term.viewport_area, Rect::new(0, 6, W, 4));
    let screen = Screen::replay(&tape, W, 10);
    assert_eq!(screen.lines()[..4], rows(0..4)[..]);
    assert!(screen.lines()[4..6].iter().all(String::is_empty));
    assert_eq!(screen.lines()[6..], band_rows(4)[..]);
    assert!(screen.scrollback.is_empty());
}

#[test]
fn inserts_under_a_shrunk_band_reclaim_the_vacated_rows() {
    let (mut term, tape) = term(10, 0, Scrollback::Regions);
    insert(&mut term, &["r0", "r1", "r2", "r3"]);
    band(&mut term, 6);
    band(&mut term, 4);
    assert_eq!(term.viewport_area, Rect::new(0, 6, W, 4));

    // two rows are free above the band; two inserted rows land in them
    // without scrolling anything into scrollback
    let mark = tape.len();
    insert(&mut term, &["r4", "r5"]);
    assert_eq!(term.viewport_area, Rect::new(0, 6, W, 4));
    assert!(!tape.since(mark).contains("\r\n"), "history was scrolled");

    band(&mut term, 4);
    let screen = Screen::replay(&tape, W, 10);
    assert!(screen.scrollback.is_empty());
    assert_eq!(screen.transcript(6), rows(0..6));
    assert_eq!(screen.lines()[6..], band_rows(4)[..]);
}

#[test]
fn growth_past_the_vacated_rows_scrolls_only_the_overflow() {
    let (mut term, tape) = term(10, 0, Scrollback::Regions);
    let transcript = rows(0..4);
    let refs: Vec<&str> = transcript.iter().map(String::as_str).collect();
    insert(&mut term, &refs);
    band(&mut term, 6);
    band(&mut term, 4);
    // the band stayed pinned: it dropped back down and the two vacated
    // rows are above it
    assert_eq!(term.viewport_area, Rect::new(0, 6, W, 4));

    // growth rises into the vacated rows first — no scroll yet
    let mark = tape.len();
    band(&mut term, 6);
    assert_eq!(term.viewport_area, Rect::new(0, 4, W, 6));
    assert!(!tape.since(mark).contains("\r\n"), "history was scrolled");
    let screen = Screen::replay(&tape, W, 10);
    assert!(screen.scrollback.is_empty());
    assert_eq!(screen.lines()[..4], transcript[..]);
    assert_eq!(screen.lines()[4..], band_rows(6)[..]);

    // growth past the screen bottom scrolls history by the overflow,
    // and only transcript rows — never vacated blanks — reach scrollback
    band(&mut term, 9);
    assert_eq!(term.viewport_area, Rect::new(0, 1, W, 9));
    let screen = Screen::replay(&tape, W, 10);
    assert_eq!(screen.scrollback, rows(0..3));
    assert_eq!(screen.transcript(1), transcript);
    assert!(screen.transcript(1).iter().all(|l| !l.is_empty()));
    assert_eq!(screen.lines()[1..], band_rows(9)[..]);
}

#[test]
fn a_modal_round_trip_repaints_the_band_in_place() {
    let (mut term, mut tape) = term(16, 0, Scrollback::Regions);
    insert(&mut term, &["banner", "/resume"]);
    band(&mut term, 4);
    let before = Screen::replay(&tape, W, 16).lines();

    write!(tape, "\x1b[3;1H❯ stale\x1b[4;1H❯").unwrap();
    term.clear().unwrap();
    band(&mut term, 4);

    let after = Screen::replay(&tape, W, 16).lines();
    assert_eq!(after, before);
    assert_eq!(after.iter().filter(|l| l.contains('❯')).count(), 1);
}

#[test]
fn full_screen_inserts_keep_the_band_out_of_scrollback() {
    let (mut term, tape) = term(8, 0, Scrollback::FullScreen);
    band(&mut term, 3);
    for i in 0..10 {
        insert(&mut term, &[format!("r{i}").as_str()]);
        band(&mut term, 3);
    }
    let screen = Screen::replay(&tape, W, 8);
    assert_eq!(screen.transcript(5), rows(0..10));
    assert_eq!(screen.lines()[5..], band_rows(3)[..]);
    assert_eq!(term.viewport_area, Rect::new(0, 5, W, 3));
}

#[test]
fn a_one_row_history_region_falls_back_to_whole_screen_scrolling() {
    let (mut term, tape) = term(5, 0, Scrollback::Regions);
    insert(&mut term, &["r0"]);
    band(&mut term, 4);
    assert_eq!(term.viewport_area, Rect::new(0, 1, W, 4));
    insert(&mut term, &["r1", "r2"]);
    band(&mut term, 4);
    let screen = Screen::replay(&tape, W, 5);
    assert_eq!(screen.transcript(1), rows(0..3));
    assert_eq!(screen.lines()[1..], band_rows(4)[..]);
}

#[test]
fn hyperlinks_and_wide_glyphs_keep_the_rest_of_the_row_in_place() {
    let (mut term, tape) = term(8, 0, Scrollback::Regions);
    term.insert_before(2, |buf| {
        buf.set_string(0, 0, "abc", Style::default());
        buf[(0, 0)].set_symbol("\x1b]8;;https://example.com\x07a\x1b]8;;\x07");
        buf.set_string(0, 1, "中文x", Style::default());
    })
    .unwrap();
    let lines = Screen::replay(&tape, W, 8).lines();
    assert_eq!(lines[0], "abc");
    assert_eq!(lines[1], "中文x");
}

#[test]
fn a_frame_on_a_shrunken_screen_does_not_scroll_history() {
    let (mut term, tape) = term(10, 0, Scrollback::Regions);
    let transcript = rows(0..6);
    let refs: Vec<&str> = transcript.iter().map(String::as_str).collect();
    insert(&mut term, &refs);
    band(&mut term, 4);
    let mark = tape.len();
    term.set_viewport_height(4, 0, Size::new(W, 8)).unwrap();
    assert_eq!(term.viewport_area, Rect::new(0, 4, W, 4));
    assert!(!tape.since(mark).contains("\r\n"), "history was scrolled");
}

#[test]
fn a_popup_on_a_full_screen_regrounds_the_band() {
    let (mut term, tape) = term(10, 0, Scrollback::Regions);
    let transcript = rows(0..6);
    let refs: Vec<&str> = transcript.iter().map(String::as_str).collect();
    insert(&mut term, &refs);
    band(&mut term, 4);
    band(&mut term, 6);
    band(&mut term, 4);
    assert_eq!(term.viewport_area, Rect::new(0, 6, W, 4));
    let screen = Screen::replay(&tape, W, 10);
    assert_eq!(screen.scrollback, rows(0..2));
    assert_eq!(screen.lines()[..4], rows(2..6)[..]);
    assert!(screen.lines()[4..6].iter().all(String::is_empty));
    assert_eq!(screen.lines()[6..], band_rows(4)[..]);

    insert(&mut term, &["r6", "r7", "r8"]);
    band(&mut term, 4);
    let screen = Screen::replay(&tape, W, 10);
    assert_eq!(screen.scrollback, rows(0..3));
    assert_eq!(screen.transcript(6), rows(0..9));
}

#[test]
fn a_busy_turn_does_not_tear_the_transcript() {
    fn band_fill(term: &mut Term, height: u16) {
        let screen = term.last_known_screen_size;
        term.set_viewport_height(height, 0, screen).unwrap();
        term.draw(|frame| {
            let area = frame.area();
            for i in 0..area.height {
                let label = if i + 2 == area.height {
                    "\u{276f}".to_string()
                } else {
                    format!("band{i}")
                };
                frame
                    .buffer
                    .set_string(area.x, area.y + i, label, Style::default());
            }
        })
        .unwrap();
    }

    let (mut term, tape) = term(24, 0, Scrollback::Regions);

    let seed = rows(0..30);
    let refs: Vec<&str> = seed.iter().map(String::as_str).collect();
    insert(&mut term, &refs);
    let mut expected: Vec<String> = seed.clone();
    let mut n = 30usize;
    band_fill(&mut term, 4);
    assert_eq!(term.viewport_area, Rect::new(0, 20, W, 4));

    let step = |term: &mut Term, expected: &mut Vec<String>, n: &mut usize| {
        let chunk = rows(*n..*n + 2);
        let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
        insert(term, &refs);
        expected.extend(chunk);
        *n += 2;
        band_fill(term, 7);
        band_fill(term, 4);
    };

    for _ in 0..6 {
        step(&mut term, &mut expected, &mut n);
    }

    let img: Vec<String> = (0..27).map(|i| format!("img{i:02}")).collect();
    let refs: Vec<&str> = img.iter().map(String::as_str).collect();
    insert(&mut term, &refs);
    expected.extend(img.clone());
    band_fill(&mut term, 4);

    for _ in 0..6 {
        step(&mut term, &mut expected, &mut n);
    }

    let screen = Screen::replay(&tape, W, 24);
    let got = screen.transcript(20);
    assert_eq!(trimmed(&got), expected.as_slice());
    assert_eq!(screen.lines()[20..24], band_rows(4)[..]);
}

#[test]
fn a_removed_live_tool_does_not_tear_the_box_below_it() {
    for scrollback in [Scrollback::Regions, Scrollback::FullScreen] {
        let (mut term, tape) = term(24, 0, scrollback);
        let seed = rows(0..30);
        let refs: Vec<&str> = seed.iter().map(String::as_str).collect();
        insert(&mut term, &refs);
        let mut expected = seed.clone();
        band(&mut term, 6);
        // a live tool finishes and its rows come out of the band
        band(&mut term, 4);

        let bx: Vec<String> = (0..10).map(|i| format!("box{i}")).collect();
        let refs: Vec<&str> = bx.iter().map(String::as_str).collect();
        insert(&mut term, &refs);
        expected.extend(bx);
        band(&mut term, 4);

        let mut n = 30usize;
        for _ in 0..10 {
            let chunk = rows(n..n + 4);
            let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
            insert(&mut term, &refs);
            expected.extend(chunk);
            n += 4;
            band(&mut term, 7);
            band(&mut term, 4);
        }

        let screen = Screen::replay(&tape, W, 24);
        let got = screen.transcript(20);
        assert_eq!(trimmed(&got), expected.as_slice(), "{scrollback:?}");
        assert_eq!(screen.lines()[20..24], band_rows(4)[..], "{scrollback:?}");
    }
}

#[test]
fn fuzz_band_ops_never_duplicate_transcript() {
    // enumerate short op sequences: band height changes + inserts, checking
    // that scrollback + visible transcript never duplicates a row.
    let mut failures = Vec::new();
    // pseudo-random deterministic sequence generator
    let mut seed: u64 = 0x9e3779b97f4a7c15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for trial in 0..4000 {
        let h = 8 + (next() % 9) as u16; // screen 8..16
        let (mut term, tape) = term(h, 0, Scrollback::Regions);
        let mut expected: Vec<String> = Vec::new();
        let mut n = 0usize;
        let steps = 6 + (next() % 10) as usize;
        for _ in 0..steps {
            match next() % 3 {
                0 => {
                    let k = 1 + (next() % 4) as usize;
                    let chunk = rows(n..n + k);
                    let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
                    insert(&mut term, &refs);
                    expected.extend(chunk);
                    n += k;
                }
                _ => {
                    let bh = 1 + (next() % (h as u64 - 1)) as u16;
                    band(&mut term, bh);
                }
            }
        }
        band(&mut term, 4);
        let screen = Screen::replay(&tape, W, h);
        let got = screen.transcript(term.viewport_area.top());
        let got_t = trimmed(&got);
        let want_t = trimmed(&expected);
        if got_t != want_t {
            failures.push(format!(
                "trial {trial} h={h}: got {got_t:?} want {want_t:?}"
            ));
            if failures.len() > 3 {
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn settle(term: &mut Term, height: u16, overlay: u16) {
    let screen = term.last_known_screen_size;
    term.set_viewport_height(height, overlay, screen).unwrap();
}

fn band_o(term: &mut Term, height: u16, overlay: u16) {
    settle(term, height, overlay);
    term.draw(|frame| {
        let area = frame.area();
        for i in 0..area.height {
            let label = if i + 2 == area.height {
                "\u{276f}".to_string()
            } else {
                format!("band{i}")
            };
            frame
                .buffer
                .set_string(area.x, area.y + i, label, Style::default());
        }
    })
    .unwrap();
}

fn covered_rows(term: &Term) -> Vec<String> {
    let n = usize::from(term.covered);
    term.tail
        .range(term.tail.len() - n..)
        .map(|row| {
            row.iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

#[test]
fn fuzz_band_ops_with_overlay_never_duplicate_transcript() {
    let mut seed: u64 = 0xdeadbeefcafe1234;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut failures = Vec::new();
    for trial in 0..6000 {
        let h = 8 + (next() % 9) as u16;
        let (mut term, tape) = term(h, 0, Scrollback::Regions);
        let mut expected: Vec<String> = Vec::new();
        let mut ops: Vec<String> = Vec::new();
        let mut n = 0usize;
        let steps = 6 + (next() % 12) as usize;
        for _ in 0..steps {
            match next() % 4 {
                0 => {
                    let k = 1 + (next() % 4) as usize;
                    let chunk = rows(n..n + k);
                    let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
                    insert(&mut term, &refs);
                    expected.extend(chunk);
                    ops.push(format!("i{n}..{}", n + k));
                    n += k;
                }
                1 => {
                    // settle without a repaint — covered rows could stay visible
                    let bh = 1 + (next() % (h as u64 - 1)) as u16;
                    let ov = (next() % 4) as u16;
                    ops.push(format!("s{bh}o{ov}"));
                    settle(&mut term, bh, ov);
                }
                _ => {
                    let bh = 1 + (next() % (h as u64 - 1)) as u16;
                    let ov = (next() % 4) as u16;
                    ops.push(format!("b{bh}o{ov}"));
                    band_o(&mut term, bh, ov);
                }
            }
        }
        band(&mut term, 4);
        let screen = Screen::replay(&tape, W, h);
        let got: Vec<String> = screen
            .transcript(term.viewport_area.top())
            .into_iter()
            .chain(covered_rows(&term))
            .filter(|row| !row.is_empty())
            .collect();
        let got_t = trimmed(&got);
        let want_t = trimmed(&expected);
        if got_t != want_t {
            failures.push(format!(
                "trial {trial} h={h} ops={}: got {got_t:?} want {want_t:?}",
                ops.join(" ")
            ));
            if failures.len() > 4 {
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn covered_rows_do_not_print_twice() {
    // A band shrinking over vacated rows, then growing with overlay, used to
    // mark scrollback copies `covered`; the next insert repainted them and
    // the paragraph printed twice. h=11, band fills most of the screen.
    let (mut term, tape) = term(11, 0, Scrollback::Regions);
    let r0 = rows(0..1);
    insert(
        &mut term,
        &r0.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    band_o(&mut term, 10, 3);
    let r1_5 = rows(1..5);
    insert(
        &mut term,
        &r1_5.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    band_o(&mut term, 4, 1);
    band_o(&mut term, 10, 3);
    let r5 = rows(5..6);
    insert(
        &mut term,
        &r5.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    band(&mut term, 4);

    let screen = Screen::replay(&tape, W, 11);
    let transcript: Vec<String> = screen
        .transcript(term.viewport_area.top())
        .into_iter()
        .chain(covered_rows(&term))
        .filter(|row| !row.is_empty())
        .collect();
    assert_eq!(
        trimmed(&transcript),
        trimmed(&rows(0..6)),
        "scrollback: {:?}",
        screen.scrollback
    );
}

/// Scrollback plus the transcript rows above the band, with any blank rows
/// that sit between two transcript rows. Inserted rows are never blank, so
/// each one is a tear.
fn tears(screen: &Screen, band_top: u16) -> Vec<usize> {
    let all = screen.transcript(band_top);
    let first = all.iter().position(|l| !l.is_empty()).unwrap_or(0);
    let last = all.iter().rposition(|l| !l.is_empty()).unwrap_or(0);
    (first..last).filter(|&i| all[i].is_empty()).collect()
}

#[test]
fn a_shrink_after_a_long_card_does_not_tear_it() {
    // A card taller than the screen lands, then the band loses two rows
    // with nothing inserted after it (an agents-panel row going away). The
    // card continues across the scrollback/screen seam without a gap.
    let (mut term, tape) = term(12, 0, Scrollback::Regions);
    band(&mut term, 6);
    let card = rows(0..30);
    insert(
        &mut term,
        &card.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    band(&mut term, 4);
    let screen = Screen::replay(&tape, W, 12);
    assert!(
        tears(&screen, term.viewport_area.top()).is_empty(),
        "{:?}",
        screen.transcript(term.viewport_area.top())
    );
    insert(&mut term, &["next"]);
    band(&mut term, 4);
    let screen = Screen::replay(&tape, W, 12);
    let mut want = card;
    want.push("next".into());
    assert_eq!(
        trimmed(&screen.transcript(term.viewport_area.top())),
        want.as_slice()
    );
}

#[test]
fn fuzz_band_ops_never_tear_the_transcript() {
    let mut seed: u64 = 0x1234_5678_9abc_def1;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut failures = Vec::new();
    for trial in 0..6000 {
        let h = 8 + (next() % 9) as u16;
        let (mut term, tape) = term(h, 0, Scrollback::Regions);
        let mut ops: Vec<String> = Vec::new();
        let mut n = 0usize;
        for _ in 0..6 + (next() % 14) as usize {
            let bh = 1 + (next() % (h as u64 - 1)) as u16;
            let ov = (next() % 4) as u16;
            match next() % 4 {
                0 => {
                    let k = 1 + (next() % 6) as usize;
                    let chunk = rows(n..n + k);
                    insert(
                        &mut term,
                        &chunk.iter().map(String::as_str).collect::<Vec<_>>(),
                    );
                    ops.push(format!("i{k}"));
                    n += k;
                }
                1 => {
                    ops.push(format!("s{bh}o{ov}"));
                    settle(&mut term, bh, ov);
                }
                _ => {
                    ops.push(format!("b{bh}o{ov}"));
                    band_o(&mut term, bh, ov);
                }
            }
        }
        band(&mut term, 4);
        let screen = Screen::replay(&tape, W, h);
        let gaps = tears(&screen, term.viewport_area.top());
        if !gaps.is_empty() {
            failures.push(format!(
                "trial {trial} h={h} gaps={gaps:?} ops={}",
                ops.join(" ")
            ));
            if failures.len() > 4 {
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
