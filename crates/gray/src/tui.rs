//! Minimal Rust port of the parts of @earendil-works/pi-tui that gray uses:
//! a retained component tree (`Container` of `Text`/`Spacer`/`Border`) whose
//! renderer word-wraps text itself at terminal width and repaints atomically,
//! so the terminal never soft-wraps behind our back and stale frame rows
//! can't survive a redraw. Mirrors pi's first-time-setup pattern: rebuild the
//! container on every change, render, draw. Delete what you don't use.

use std::io::Write;

/// Strips SGR (and other CSI) escape sequences for width measurement.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI: ESC [ params final-byte(@-~); other escape types: consume.
            if chars.next() == Some('[') {
                for t in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&t) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Display width in chars, ANSI-aware.
/// ponytail: char count, not unicode display width — wide glyphs may overflow; swap in unicode-width if that matters.
pub fn visible_width(s: &str) -> usize {
    strip_ansi(s).chars().count()
}

/// Greedy word-wrap to at most `width` visible chars per line. ANSI spans may
/// cross line breaks; callers wrap whole styled strings and re-emit codes per
/// line only when needed (Text keeps styling by wrapping the raw string).
pub fn wrap(s: &str, width: usize) -> Vec<String> {
    // Merge zero-width tokens (bare SGR sequences like \x1b[0m) into a
    // neighbor so they never force their own line break.
    let mut toks: Vec<String> = Vec::new();
    for w in s.split_whitespace() {
        if visible_width(w) == 0 {
            match toks.last_mut() {
                Some(l) => l.push_str(w),
                None => toks.push(w.to_string()),
            }
        } else if toks.last().is_some_and(|l| visible_width(l) == 0) {
            let l = toks.last_mut().unwrap();
            l.push_str(w);
        } else {
            toks.push(w.to_string());
        }
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    let mut active_codes: Vec<String> = Vec::new(); // SGR states open at line start
    for word in &toks {
        // Emptiness is measured by VISIBLE width: reopened SGR codes make
        // `cur` non-empty without occupying columns.
        let sep = usize::from(cur_w > 0);
        let w_w = visible_width(word);
        if cur_w + sep + w_w > width && cur_w > 0 {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
            // Reopen whatever SGR state was active so style survives the break.
            for code in &active_codes {
                cur.push_str(code);
            }
        }
        if cur_w > 0 {
            cur.push(' ');
            cur_w += 1;
        }
        cur.push_str(word);
        cur_w += w_w;
        // Track open SGR codes so wrapped lines restore them (dim/accent etc).
        update_active_codes(&mut active_codes, word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Records `\x1b[..m` sequences inside `chunk` into `active` (reset clears).
fn update_active_codes(active: &mut Vec<String>, chunk: &str) {
    let bytes = chunk.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let start = i;
            i += 2;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            if i < bytes.len() {
                let seq = &chunk[start..=i];
                if seq.ends_with('m') {
                    if seq == "\x1b[0m" || seq == "\x1b[m" {
                        active.clear();
                    } else {
                        active.push(seq.to_string());
                    }
                }
            }
        } else {
            i += 1;
        }
    }
}

/// Prints `text` indented by `pad` spaces, wrapped to the terminal width.
pub fn print_wrapped(text: &str, pad: usize) {
    let width = crate::term_width().saturating_sub(pad);
    for line in wrap(text, width) {
        println!("{}{}", " ".repeat(pad), line);
    }
}

/// Clears the terminal; tty-only.
pub fn clear_screen() {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() && crossterm::terminal::size().is_ok() {
        print!("\x1b[2J\x1b[1;1H");
        let _ = std::io::stdout().flush();
    }
}

/// A pi-tui component: renders to lines guaranteed ≤ `width` visible chars.
pub trait Component {
    fn render(&self, width: usize) -> Vec<String>;
}

/// Styled text with left/right padding, word-wrapped at `width` (pi-tui Text).
pub struct Text {
    pub text: String,
    pub padding_x: usize,
}

impl Text {
    pub fn new(text: impl Into<String>, padding_x: usize) -> Self {
        Self { text: text.into(), padding_x }
    }
}

impl Component for Text {
    fn render(&self, width: usize) -> Vec<String> {
        let content = width.saturating_sub(self.padding_x * 2).max(1);
        let pad = " ".repeat(self.padding_x);
        wrap(&self.text, content)
            .into_iter()
            .map(|l| format!("{pad}{l}"))
            .collect()
    }
}

/// Vertical gap of empty lines (pi-tui Spacer).
pub struct Spacer(pub usize);

impl Component for Spacer {
    fn render(&self, _width: usize) -> Vec<String> {
        vec![String::new(); self.0]
    }
}

/// Horizontal rule filling the width (pi-tui DynamicBorder).
#[derive(Default)]
pub struct Border;

impl Component for Border {
    fn render(&self, width: usize) -> Vec<String> {
        vec!["\u{2500}".repeat(width.max(1))]
    }
}

/// Retained component tree; rebuilt wholesale on every change, pi-style.
#[derive(Default)]
pub struct Container {
    children: Vec<Box<dyn Component>>,
}

impl Container {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }
    pub fn clear(&mut self) {
        self.children.clear();
    }
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

impl Component for Container {
    fn render(&self, width: usize) -> Vec<String> {
        self.children.iter().flat_map(|c| c.render(width)).collect()
    }
}

/// Inline (non-alternate-screen) renderer: draws one frame above the cursor,
/// erasing the previous frame entirely first. Every line must be ≤ terminal
/// width (guaranteed by Component), so logical rows == physical rows and the
/// cursor-up count stays truthful across resizes.
#[derive(Default)]
pub struct InlineFrame {
    drawn: usize,
}

impl InlineFrame {
    /// Renders `frame` at `width`, repaints it over the previous frame.
    pub fn draw(&mut self, out: &mut impl Write, frame: &Container, width: usize) -> anyhow::Result<()> {
        let lines = frame.render(width);
        if self.drawn > 0 {
            // Jump back over the old frame and wipe it to end-of-screen:
            // shorter new frames leave no stale border fragments behind.
            write!(out, "\x1b[{}A\r\x1b[J", self.drawn)?;
        }
        let last = lines.len().saturating_sub(1);
        for (i, l) in lines.iter().enumerate() {
            if i == last {
                // No trailing newline on the pane's bottom row: emitting one
                // scrolls the screen and every later repaint drifts a row.
                write!(out, "\r\x1b[2K{l}")?;
            } else {
                write!(out, "\r\x1b[2K{l}\r\n")?;
            }
        }
        write!(out, "\r")?;
        self.drawn = last;
        out.flush()?;
        Ok(())
    }

    /// Erases the current frame so only the outcome remains in the transcript.
    pub fn erase(&mut self, out: &mut impl Write) -> anyhow::Result<()> {
        if self.drawn > 0 {
            write!(out, "\x1b[{}A\r\x1b[J", self.drawn)?;
            self.drawn = 0;
            out.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_at_words_and_respects_width() {
        assert_eq!(wrap("hello world", 20), vec!["hello world"]);
        assert_eq!(wrap("hello world foo", 11), vec!["hello world", "foo"]);
        assert_eq!(wrap("", 10), Vec::<String>::new());
        // long word harder than width still emitted as its own line
        assert_eq!(wrap("abcdefghij", 5), vec!["abcdefghij"]);
        // bare reset sequence rides along instead of forcing a break
        assert_eq!(wrap("\x1b[2mword word\x1b[0m", 10), vec!["\x1b[2mword word\x1b[0m"]);
    }

    #[test]
    fn visible_width_ignores_ansi() {
        assert_eq!(visible_width("\x1b[2mdim\x1b[0m"), 3);
        assert_eq!(visible_width("plain"), 5);
    }

    #[test]
    fn text_wraps_and_pads() {
        let t = Text::new("aaaa bbbb cccc", 1);
        let lines = t.render(11); // content width 9 fits "aaaa bbbb"
        assert!(lines.iter().all(|l| visible_width(l) <= 10));
        assert_eq!(lines[0], " aaaa bbbb");
        assert_eq!(lines[1], " cccc");
    }

    #[test]
    fn styled_text_restores_state_across_line_breaks() {
        let t = Text::new("\x1b[2mword word word\x1b[0m", 0);
        let lines = t.render(10);
        assert_eq!(lines.len(), 2); // "word word" fits, third wraps
        assert!(lines[1].starts_with("\x1b[2m"), "second line should reopen dim: {:?}", lines[1]);
        assert!(lines[1].ends_with("\x1b[0m"));
    }

    #[test]
    fn border_fills_exact_width() {
        let b = Border;
        let lines = b.render(30);
        assert_eq!(lines.len(), 1);
        assert_eq!(visible_width(&lines[0]), 30);
        assert_eq!(lines[0], "─".repeat(30));
    }

    #[test]
    fn container_concatenates_children() {
        let mut c = Container::new();
        c.push(Box::new(Text::new("hi", 1)));
        c.push(Box::new(Spacer(2)));
        c.push(Box::new(Border));
        let lines = c.render(20);
        assert_eq!(lines.len(), 4); // 1 text + 2 spacer + 1 border
        assert_eq!(lines[0], " hi");
        assert!(lines[1].is_empty() && lines[2].is_empty());
    }
}
