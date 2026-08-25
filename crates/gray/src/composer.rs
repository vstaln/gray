//! Ratatui-backed composer: the codex/grok-build architecture sized for gray.
//!
//! An inline viewport owns the bottom rows permanently — slash-completion
//! panel, rule, `›` input, rule — while transcript output goes into real
//! terminal scrollback via [`ratatui::Terminal::insert_before`]. Nothing is
//! ever erased-and-hoped-redrawn, so frames/rules cannot disappear mid-turn.

use std::io::{Stdout, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Terminal;

use crate::repl::completion_matches;

/// Viewport rows: 5 completion panel + status + rule + input + rule.
const VIEWPORT_H: u16 = 9;
const PANEL_ROWS: usize = 5;

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Shared handle: main thread drives prompts/streaming, a small ticker task
/// refreshes the elapsed-seconds counter while a turn is running.
pub struct SharedTui(pub Arc<std::sync::Mutex<Tui>>);

impl std::ops::Deref for SharedTui {
    type Target = Arc<std::sync::Mutex<Tui>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct Tui {
    terminal: Term,
    buffer: String,
    matches: Vec<(&'static str, &'static str)>,
    sel: usize,
    /// Some((start, label)) while a turn is running — rendered above the rule.
    status: Option<(Instant, String)>,
    /// Partial streamed line awaiting its newline.
    pending: String,
    /// 24-bit color support — selects shimmer vs blink for the status row.
    truecolor: bool,
}

/// Codex-rs shimmer (motion.rs/shimmer.rs), ported: a highlight band sweeps
/// the text on a 2s cosine cycle; truecolor blends bg->fg per char with a
/// bold peak, plain terminals step through dim/normal/bold instead.
fn shimmer_spans(text: &str, elapsed: Duration, truecolor: bool) -> Vec<Span<'static>> {
    use ratatui::style::Color;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos = ((elapsed.as_secs_f32() % sweep_seconds) / sweep_seconds * (period as f32)) as usize;
    let band_half_width = 5.0f32;
    const BASE: (u8, u8, u8) = (150, 148, 144);
    const HIGHLIGHT: (u8, u8, u8) = (255, 255, 255);

    chars
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let dist = ((i as isize + padding as isize) - pos as isize).abs() as f32;
            // 0 -> 1 cosine bump across the band (codex-rs motion.rs).
            // Parens matter: .cos() must bind to PI*x only, not 1.0 + PI*x
            // (that variant goes negative and freezes both style paths).
            let t = if dist <= band_half_width {
                0.5 * (1.0 + (std::f32::consts::PI * (dist / band_half_width)).cos())
            } else {
                0.0
            };
            let style = if truecolor {
                let k = t.clamp(0.0, 1.0) * 0.9;
                let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k) as u8;
                Style::default()
                    .fg(Color::Rgb(
                        lerp(BASE.0, HIGHLIGHT.0),
                        lerp(BASE.1, HIGHLIGHT.1),
                        lerp(BASE.2, HIGHLIGHT.2),
                    ))
                    .add_modifier(if t > 0.3 { Modifier::BOLD } else { Modifier::empty() })
            } else if t < 0.2 {
                Style::default().add_modifier(Modifier::DIM)
            } else if t < 0.6 {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

impl Tui {
    pub fn new() -> anyhow::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        // Anchor the viewport to the BOTTOM edge of the terminal (pi/codex
        // behavior): jump to the last row and emit H newlines — the screen
        // scrolls exactly H lines and the cursor ends on the bottom row,
        // where ratatui's inline viewport then sits.
        let (_, rows) = crossterm::terminal::size()?;
        crossterm::execute!(
            std::io::stdout(),
            crossterm::cursor::MoveTo(0, rows.saturating_sub(1))
        )?;
        for _ in 0..VIEWPORT_H {
            write!(std::io::stdout(), "\r\n")?;
        }
        let _ = std::io::stdout().flush();
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(std::io::stdout()),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Inline(VIEWPORT_H),
            },
        )?;
        terminal.clear()?;
        Ok(Self {
            terminal,
            buffer: String::new(),
            matches: Vec::new(),
            sel: 0,
            status: None,
            pending: String::new(),
            truecolor: std::env::var("COLORTERM")
                .map(|v| v.contains("truecolor") || v.contains("24bit"))
                .unwrap_or(false),
        })
    }

    fn width(&self) -> usize {
        self.terminal.size().map(|a| a.width as usize).unwrap_or(80)
    }

    /// One full repaint of the owned bottom pane.
    fn draw(&mut self) -> anyhow::Result<()> {
        self.terminal.draw(|frame| {
            let area = frame.area();
            let w = area.width as usize;
            let rule_style = Style::default().add_modifier(Modifier::DIM);

            // Slash-completion panel (top 5 rows, hidden while empty).
            if !self.matches.is_empty() {
                let start = self.sel.saturating_sub(PANEL_ROWS - 1).min(self.sel);
                for (i, (name, desc)) in
                    self.matches.iter().enumerate().skip(start).take(PANEL_ROWS)
                {
                    let y = i - start;
                    let body = format!("  /{name} \u{2014} {desc}");
                    let line = if i == self.sel {
                        Line::from(body.as_str()).style(Style::default().reversed())
                    } else {
                        Line::from(body.as_str()).style(Style::default().dim())
                    };
                    frame.render_widget(
                        Paragraph::new(line),
                        Rect::new(area.x, area.y + y as u16, area.width, 1),
                    );
                }
            }

            // Status row sits directly above the top rule while a turn runs.
            // Animation copied from codex-rs motion/shimmer: a highlight band
            // sweeps the text on a 2s cosine cycle (truecolor) or steps
            // through dim/bold bands (fallback). ALWAYS rendered — an empty
            // Paragraph writes zero cells, so the stale text would survive
            // ratatui's cell-diff forever.
            {
                let status_rect = Rect::new(area.x, area.y + PANEL_ROWS as u16, area.width, 1);
                match &self.status {
                    Some((started, label)) => {
                        let secs = started.elapsed().as_secs();
                        let blink_bullet = if (started.elapsed().as_millis() / 600) % 2 == 0 {
                            "\u{2022}"
                        } else {
                            "\u{25e6}"
                        };
                        let bullet = if self.truecolor { "\u{2022}" } else { blink_bullet };
                        let text =
                            format!("{bullet} {label}\u{2026} {secs}s (ctrl-c to cancel)");
                        let spans = shimmer_spans(&text, started.elapsed(), self.truecolor);
                        frame.render_widget(Paragraph::new(Line::from(spans)), status_rect);
                    }
                    None => {
                        frame.render_widget(
                            Paragraph::new(Line::from(" ".repeat(w))),
                            status_rect,
                        );
                    }
                }
            }

            // Rule / input / rule.
            let rule_y = area.y + (PANEL_ROWS + 1) as u16;
            frame.render_widget(
                Paragraph::new(Line::from("\u{2500}".repeat(w)).style(rule_style)),
                Rect::new(area.x, rule_y, area.width, 1),
            );
            let budget = w.saturating_sub(3);
            let shown: String = {
                let chars: Vec<char> = self.buffer.chars().collect();
                if chars.len() > budget {
                    chars[chars.len() - budget..].iter().collect()
                } else {
                    self.buffer.clone()
                }
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    "\u{203a} ".dim(),
                    shown.as_str().into(),
                ])),
                Rect::new(area.x, rule_y + 1, area.width, 1),
            );
            frame.render_widget(
                Paragraph::new(Line::from("\u{2500}".repeat(w)).style(rule_style)),
                Rect::new(area.x, rule_y + 2, area.width, 1),
            );

            // Park the terminal cursor right after the typed text.
            let col = (2 + shown.chars().count()).min(w.saturating_sub(1));
            frame.set_cursor_position(Position::new(
                area.x + col as u16,
                rule_y + 1,
            ));
        })?;
        Ok(())
    }

    /// Raw-mode prompt editor. Returns the submitted line, or None on
    /// Ctrl-C / Ctrl-D-on-empty (exit request). Same keys as always:
    /// Enter completes-and-fires, Tab inserts, arrows/Ctrl-N/P navigate.
    pub fn read_line(&mut self) -> anyhow::Result<Option<String>> {
        use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Show)?;
        loop {
            self.matches = if self.buffer.starts_with('/')
                && !self.buffer[1..].contains(char::is_whitespace)
            {
                completion_matches(&self.buffer[1..])
            } else {
                Vec::new()
            };
            if self.sel >= self.matches.len() {
                self.sel = self.matches.len().saturating_sub(1);
            }
            self.draw()?;

            if !poll(Duration::from_millis(250))? {
                continue; // pure timeout: nothing to do while idle
            }
            match read()? {
                Event::Resize(_, _) => {} // next draw picks up the new size
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                Event::Key(KeyEvent {
                    code: KeyCode::Char('d'),
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) && self.buffer.is_empty() => {
                    return Ok(None)
                }
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                    KeyCode::Char('p') => self.sel = self.sel.saturating_sub(1),
                    KeyCode::Char('n') => {
                        self.sel = (self.sel + 1).min(self.matches.len().saturating_sub(1))
                    }
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Enter => {
                        // complete, then submit immediately (fires the command)
                        if self.buffer.chars().count() > 1
                            && let Some((name, _)) = self.matches.get(self.sel)
                        {
                            self.buffer = format!("/{name} ");
                        }
                        let line = std::mem::take(&mut self.buffer);
                        self.matches.clear();
                        self.sel = 0;
                        let trimmed = line.trim_end().to_string();
                        self.push_line(format!("\u{203a} {trimmed}"));
                        return Ok(Some(trimmed));
                    }
                    KeyCode::Tab => {
                        if let Some((name, _)) = self.matches.get(self.sel) {
                            self.buffer = format!("/{name} ");
                        }
                    }
                    KeyCode::Char(c) => self.buffer.push(c),
                    KeyCode::Backspace => {
                        self.buffer.pop();
                    }
                    KeyCode::Esc => self.buffer.clear(),
                    KeyCode::Up => self.sel = self.sel.saturating_sub(1),
                    KeyCode::Down => {
                        self.sel = (self.sel + 1).min(self.matches.len().saturating_sub(1))
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Marks the start of a turn: status row appears ("• Working…").
    pub fn begin_turn(&mut self, label: &str) {
        self.status = Some((Instant::now(), label.to_string()));
        let _ = self.draw();
    }

    /// Marks the end of a turn: status row disappears.
    pub fn end_turn(&mut self) {
        self.status = None;
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.push_line(rest);
        }
        let _ = std::io::stdout().flush();
        let _ = self.draw();
    }

    /// Streams a rendered event chunk into the transcript. Complete lines are
    /// pushed into scrollback immediately; the tail stays in the viewport.
    pub fn stream(&mut self, chunk: &str) {
        self.pending.push_str(&strip_ansi(chunk));
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let line = line.trim_end_matches('\n').to_string();
            self.push_line(line);
        }
        let _ = self.draw();
    }

    /// Pushes one wrapped line into real scrollback above the viewport.
    pub fn push_line(&mut self, line: String) {
        let w = self.width().max(10);
        let chars: Vec<char> = line.chars().collect();
        let mut height = 0usize;
        for chunk in chars.chunks(w.saturating_sub(1)) {
            let text: String = chunk.iter().collect();
            height += 1;
            let _ = self.terminal.insert_before(1, |buf| {
                Paragraph::new(Line::from(text.as_str())).render(buf.area, buf);
            });
        }
        if height == 0 {
            let _ = self.terminal.insert_before(1, |buf| {
                Paragraph::new(Line::from("")).render(buf.area, buf);
            });
        }
        let _ = std::io::stdout().flush();
    }

    /// Pushes one pre-wrapped plain-text line into scrollback, rendered dim
    /// (startup banner).
    pub fn push_dim(&mut self, line: String) {
        let _ = self.terminal.insert_before(1, |buf| {
            Paragraph::new(Line::from(Span::styled(
                line,
                Style::new().add_modifier(Modifier::DIM),
            )))
            .render(buf.area, buf);
        });
        let _ = std::io::stdout().flush();
    }

    /// Refreshes the status/elapsed display (ticker task, whole session).
    /// Unconditional redraw: any stale frame residue self-heals within a tick.
    pub fn tick_status(&mut self) {
        let _ = self.draw();
    }

    /// Restores cooked mode (called on exit). Steps the cursor past the
    /// bottom rule so whatever prints next starts on a clean line,
    /// pi-style. Moves to the screen's last row first so the newlines
    /// always scroll past the bottom rule even if the cursor is parked
    /// on the input row.
    pub fn shutdown(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let (_, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let _ = write!(std::io::stdout(), "\x1b[{};1H\n\n", rows);
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
        let _ = std::io::stdout().flush();
    }
}

/// Strips ANSI escape sequences — ratatui renders plain text; our fmt_event
/// decorations (dim chips etc.) become plain transcript lines.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shimmer_spans_change_across_ticks() {
        let text = "\u{2022} Working\u{2026} 1s (ctrl-c to cancel)";
        // Band needs ~190ms to enter the text from the left padding; sample
        // inside the sweep where consecutive ticks must differ.
        let mut prev = shimmer_spans(text, Duration::from_millis(300), false);
        for ms in (400..=1200).step_by(100) {
            let cur = shimmer_spans(text, Duration::from_millis(ms), false);
            assert_ne!(prev, cur, "no change between {}ms and {}ms", ms - 100, ms);
            prev = cur;
        }
    }

    #[test]
    fn shimmer_truecolor_changes_across_ticks() {
        let text = "\u{2022} Working\u{2026} 1s";
        let a = shimmer_spans(text, Duration::from_millis(500), true);
        let b = shimmer_spans(text, Duration::from_millis(600), true);
        assert_ne!(a, b);
    }

    /// The full draw path: two consecutive tick_status()-style frames with
    /// different elapsed times must produce different backend content.
    #[test]
    fn consecutive_frames_differ_in_test_backend() {
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;
        use std::cell::Cell;

        let status: Cell<Option<(Instant, String)>> = Cell::new(Some((Instant::now(), "Working".into())));
        // Simulate frozen start by overriding elapsed via distinct Instants.
        let t0 = Instant::now();
        let render = |term: &mut Term| unreachable!();
        let _ = (status, t0, render);

        let mut terminal = Terminal::new(TestBackend::new(40, 3)).unwrap();
        let started = Instant::now();
        let frame_at = |ms: u64, term: &mut Terminal<TestBackend>| {
            let fake_started = started - Duration::from_millis(ms);
            term.draw(|f| {
                let text = format!("\u{2022} Working\u{2026} 1s");
                let spans = shimmer_spans(&text, fake_started.elapsed(), false);
                f.render_widget(
                    Paragraph::new(Line::from(spans)),
                    Rect::new(0, 0, 40, 1),
                );
            }).unwrap();
            term.backend().buffer().clone()
        };
        let b1 = frame_at(500, &mut terminal);
        let b2 = frame_at(600, &mut terminal);
        assert_ne!(b1, b2, "ratatui diff should see changing spans across 100ms ticks");
    }
}
