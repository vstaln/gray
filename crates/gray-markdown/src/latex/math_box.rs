//! Single-column math layout: row breaks join with `; ` in flat (inline)
//! mode and split lines in display mode.
//!
//! A 2D box-attach layout used to align multi-row matrices beside their
//! prefix/suffix on an anchor row. Deleted: environments now render
//! single-line (the inline `(1 2; 3 4)` form), so a plain row list suffices.
//! Display `\\` still splits lines. Re-add box attach if display matrices
//! must be grids again.

pub(super) struct MathBox {
    lines: Vec<String>,
    /// Flat mode (inline math): row breaks render as `; `.
    pub(super) flat: bool,
}

impl MathBox {
    pub(super) fn new(flat: bool) -> Self {
        Self {
            lines: vec![String::new()],
            flat,
        }
    }

    fn cur(&mut self) -> &mut String {
        // ponytail: `lines` is never empty (`new` seeds one row).
        self.lines.last_mut().expect("math box keeps one row")
    }

    /// `true` when nothing has been emitted on the current row yet.
    pub(super) fn at_line_start(&self) -> bool {
        self.lines.last().is_some_and(|l| l.is_empty())
    }

    pub(super) fn ends_with_space(&self) -> bool {
        self.lines.last().is_some_and(|l| l.ends_with(' '))
    }

    pub(super) fn push(&mut self, c: char) {
        if c == '\n' {
            self.vbreak();
        } else {
            self.cur().push(c);
        }
    }

    pub(super) fn push_str(&mut self, s: &str) {
        let mut first = true;
        for row in s.split('\n') {
            if !first {
                self.vbreak();
            }
            first = false;
            self.cur().push_str(row);
        }
    }

    /// End the current row; flow continues on a fresh row below. Flat mode
    /// renders the break as `; `.
    fn vbreak(&mut self) {
        if self.flat {
            if !self.at_line_start() {
                let cur = self.cur();
                while cur.ends_with(' ') {
                    cur.pop();
                }
                cur.push_str("; ");
            }
        } else {
            self.lines.push(String::new());
        }
    }

    pub(super) fn into_lines(self) -> Vec<String> {
        self.lines
    }
}

impl std::fmt::Write for MathBox {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.push_str(s);
        Ok(())
    }
}
