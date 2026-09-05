//! Built-in lightweight nano-like interactive editor for the Gray system prompt.

use std::io::{Write, stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub struct SysEditor {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    scroll_top: usize,
    scroll_left: usize,
    cut_buffer: Option<String>,
    modified: bool,
    path: PathBuf,
    status_msg: Option<(String, std::time::Instant)>,
}

impl SysEditor {
    pub fn new(initial_text: &str, path: &Path) -> Self {
        let mut lines: Vec<String> = initial_text.lines().map(String::from).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            lines,
            cursor_row: 0,
            cursor_col: 0,
            scroll_top: 0,
            scroll_left: 0,
            cut_buffer: None,
            modified: false,
            path: path.to_path_buf(),
            status_msg: None,
        }
    }

    pub fn run(&mut self) -> anyhow::Result<Option<String>> {
        let composer_was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if !composer_was_raw {
            enable_raw_mode()?;
        }

        crossterm::execute!(
            stdout(),
            EnterAlternateScreen,
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
            crossterm::cursor::Show
        )?;
        // Some tiling WMs deliver a resize between EnterAlternateScreen and
        // Terminal::new, which ratatui would then map to a zero-sized viewport.
        // Match codex tui::init pattern: re-query after the switch so we anchor correctly.
        let _ = crossterm::terminal::size();
        let backend = CrosstermBackend::new(stdout());
        let mut terminal = Terminal::new(backend)?;

        let res = self.event_loop(&mut terminal);

        let _ = terminal.clear();
        crossterm::execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        )?;
        // LeaveAlternateScreen already restores the main screen buffer — do NOT
        // emit ClearType::All / blank-line floods here, they race the compositor's
        // own synchronized-update flush and are exactly the ghost-text your 17:22
        // screenshot shows (codex restore path is LeaveAlternateScreen → raw-mode
        // resync only, see tui.rs restore_common).
        if !composer_was_raw {
            disable_raw_mode()?;
        } else {
            enable_raw_mode()?;
        }
        let _ = std::io::stdout().flush();

        res
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = Some((msg.into(), std::time::Instant::now()));
    }

    fn event_loop<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> anyhow::Result<Option<String>> {
        loop {
            // Expire status message after 3 seconds
            if let Some((_, set_at)) = &self.status_msg
                && set_at.elapsed() > Duration::from_secs(3)
            {
                self.status_msg = None;
            }

            terminal.draw(|frame| self.render(frame))?;

            if !poll(Duration::from_millis(100))? {
                continue;
            }

            match read()? {
                Event::Resize(_, _) => {}
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if modifiers.contains(KeyModifiers::CONTROL) {
                        match code {
                            KeyCode::Char('s') => {
                                // Save and exit
                                return Ok(Some(self.lines.join("\n")));
                            }
                            KeyCode::Char('r') => {
                                // Reset to shipped default
                                self.lines = crate::DEFAULT_SYS_PROMPT
                                    .lines()
                                    .map(String::from)
                                    .collect();
                                if self.lines.is_empty() {
                                    self.lines.push(String::new());
                                }
                                self.cursor_row = 0;
                                self.cursor_col = 0;
                                self.scroll_top = 0;
                                self.scroll_left = 0;
                                self.modified = true;
                                self.set_status("Reset to default system prompt");
                            }
                            KeyCode::Char('x') | KeyCode::Char('c') => {
                                // Cancel / exit without saving
                                return Ok(None);
                            }
                            KeyCode::Char('k') => {
                                // Cut line (nano-style)
                                if self.lines.len() > 1 {
                                    let cut = self.lines.remove(self.cursor_row);
                                    self.cut_buffer = Some(cut);
                                    if self.cursor_row >= self.lines.len() {
                                        self.cursor_row = self.lines.len() - 1;
                                    }
                                } else {
                                    let cut = std::mem::take(&mut self.lines[0]);
                                    self.cut_buffer = Some(cut);
                                }
                                self.cursor_col =
                                    self.cursor_col.min(self.current_line_char_count());
                                self.modified = true;
                                self.set_status("Cut line to buffer (^U to paste)");
                            }
                            KeyCode::Char('u') => {
                                // Uncut / paste line (nano-style)
                                if let Some(cut) = &self.cut_buffer {
                                    self.lines.insert(self.cursor_row, cut.clone());
                                    self.cursor_row += 1;
                                    self.cursor_col = 0;
                                    self.modified = true;
                                    self.set_status("Pasted line from buffer");
                                }
                            }
                            KeyCode::Char('a') => {
                                self.cursor_col = 0;
                            }
                            KeyCode::Char('e') => {
                                self.cursor_col = self.current_line_char_count();
                            }
                            _ => {}
                        }
                    } else {
                        match code {
                            KeyCode::Esc => return Ok(None),
                            KeyCode::Char(c) => {
                                self.insert_char(c);
                            }
                            KeyCode::Tab => {
                                self.insert_char(' ');
                                self.insert_char(' ');
                            }
                            KeyCode::Enter => {
                                self.insert_newline();
                            }
                            KeyCode::Backspace => {
                                self.delete_backward();
                            }
                            KeyCode::Delete => {
                                self.delete_forward();
                            }
                            KeyCode::Left => {
                                self.move_left();
                            }
                            KeyCode::Right => {
                                self.move_right();
                            }
                            KeyCode::Up => {
                                self.move_up();
                            }
                            KeyCode::Down => {
                                self.move_down();
                            }
                            KeyCode::Home => {
                                self.cursor_col = 0;
                            }
                            KeyCode::End => {
                                self.cursor_col = self.current_line_char_count();
                            }
                            KeyCode::PageUp => {
                                let view_h = terminal.size()?.height.saturating_sub(4) as usize;
                                self.cursor_row = self.cursor_row.saturating_sub(view_h.max(1));
                                self.cursor_col =
                                    self.cursor_col.min(self.current_line_char_count());
                            }
                            KeyCode::PageDown => {
                                let view_h = terminal.size()?.height.saturating_sub(4) as usize;
                                self.cursor_row = (self.cursor_row + view_h.max(1))
                                    .min(self.lines.len().saturating_sub(1));
                                self.cursor_col =
                                    self.cursor_col.min(self.current_line_char_count());
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn current_line_char_count(&self) -> usize {
        self.lines
            .get(self.cursor_row)
            .map(|l| l.chars().count())
            .unwrap_or(0)
    }

    fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.cursor_row];
        let mut chars: Vec<char> = line.chars().collect();
        let idx = self.cursor_col.min(chars.len());
        chars.insert(idx, c);
        *line = chars.into_iter().collect();
        self.cursor_col = idx + 1;
        self.modified = true;
    }

    fn insert_newline(&mut self) {
        let line = &self.lines[self.cursor_row];
        let chars: Vec<char> = line.chars().collect();
        let idx = self.cursor_col.min(chars.len());
        let left: String = chars[..idx].iter().collect();
        let right: String = chars[idx..].iter().collect();
        self.lines[self.cursor_row] = left;
        self.lines.insert(self.cursor_row + 1, right);
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.modified = true;
    }

    fn delete_backward(&mut self) {
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_row];
            let mut chars: Vec<char> = line.chars().collect();
            let idx = self.cursor_col.min(chars.len());
            chars.remove(idx - 1);
            *line = chars.into_iter().collect();
            self.cursor_col = idx - 1;
            self.modified = true;
        } else if self.cursor_row > 0 {
            let cur_line = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            let prev_len = self.lines[self.cursor_row].chars().count();
            self.lines[self.cursor_row].push_str(&cur_line);
            self.cursor_col = prev_len;
            self.modified = true;
        }
    }

    fn delete_forward(&mut self) {
        let line_len = self.current_line_char_count();
        if self.cursor_col < line_len {
            let line = &mut self.lines[self.cursor_row];
            let mut chars: Vec<char> = line.chars().collect();
            let idx = self.cursor_col.min(chars.len());
            chars.remove(idx);
            *line = chars.into_iter().collect();
            self.modified = true;
        } else if self.cursor_row + 1 < self.lines.len() {
            let next_line = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next_line);
            self.modified = true;
        }
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.current_line_char_count();
        }
    }

    fn move_right(&mut self) {
        let line_len = self.current_line_char_count();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.cursor_col.min(self.current_line_char_count());
        }
    }

    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.cursor_col.min(self.current_line_char_count());
        }
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        if area.width < 10 || area.height < 4 {
            return;
        }

        let header_h = 1u16;
        let footer_h = 2u16;
        let view_h = area.height.saturating_sub(header_h + footer_h) as usize;

        let total_digits = format!("{}", self.lines.len()).len().max(2);
        let gutter_w = total_digits + 3; // " 12 │ "
        let text_view_w = (area.width as usize).saturating_sub(gutter_w);

        // Adjust scroll_top
        if self.cursor_row < self.scroll_top {
            self.scroll_top = self.cursor_row;
        } else if self.cursor_row >= self.scroll_top + view_h {
            self.scroll_top = self.cursor_row.saturating_sub(view_h.saturating_sub(1));
        }

        // Adjust scroll_left
        if self.cursor_col < self.scroll_left {
            self.scroll_left = self.cursor_col;
        } else if self.cursor_col >= self.scroll_left + text_view_w {
            self.scroll_left = self
                .cursor_col
                .saturating_sub(text_view_w.saturating_sub(1));
        }

        // 1. Render Header Bar
        let title_left = format!(" GRAY SYSTEM PROMPT \u{b7} {}", self.path.display());
        let mod_tag = if self.modified {
            " [Modified]"
        } else {
            " [Saved]"
        };
        let title_right = format!(
            "Ln {}, Col {} ({}) ",
            self.cursor_row + 1,
            self.cursor_col + 1,
            self.lines.len()
        );
        let header_pad = (area.width as usize).saturating_sub(
            title_left.chars().count() + mod_tag.chars().count() + title_right.chars().count(),
        );

        let header_spans = vec![
            Span::styled(
                title_left,
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(246, 173, 126))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                mod_tag,
                Style::default()
                    .fg(if self.modified {
                        Color::Rgb(180, 40, 40)
                    } else {
                        Color::Rgb(40, 120, 40)
                    })
                    .bg(Color::Rgb(246, 173, 126))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " ".repeat(header_pad),
                Style::default().bg(Color::Rgb(246, 173, 126)),
            ),
            Span::styled(
                title_right,
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(246, 173, 126)),
            ),
        ];
        frame.render_widget(
            Paragraph::new(Line::from(header_spans)),
            Rect::new(area.x, area.y, area.width, 1),
        );

        // 2. Render Text Editor Viewport
        let mut body_lines = Vec::with_capacity(view_h);
        for i in 0..view_h {
            let line_idx = self.scroll_top + i;
            if line_idx < self.lines.len() {
                let gutter = format!(" {:>width$} \u{2502} ", line_idx + 1, width = total_digits);
                let text: String = self.lines[line_idx]
                    .chars()
                    .skip(self.scroll_left)
                    .take(text_view_w)
                    .collect();

                body_lines.push(Line::from(vec![
                    Span::styled(gutter, Style::default().fg(Color::Rgb(90, 90, 90))),
                    Span::raw(text),
                ]));
            } else {
                let gutter = format!(" {:>width$} \u{2502} ", "~", width = total_digits);
                body_lines.push(Line::from(vec![Span::styled(
                    gutter,
                    Style::default().fg(Color::Rgb(60, 60, 60)),
                )]));
            }
        }
        frame.render_widget(
            Paragraph::new(body_lines),
            Rect::new(area.x, area.y + header_h, area.width, view_h as u16),
        );

        // 3. Render Status Line (if any) or Rule
        let status_y = area.y + area.height - footer_h;
        let status_line = if let Some((msg, _)) = &self.status_msg {
            Line::from(vec![
                Span::styled(" \u{2022} ", Style::default().fg(Color::Rgb(246, 173, 126))),
                Span::styled(
                    msg.as_str(),
                    Style::default()
                        .fg(Color::Rgb(230, 230, 230))
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        } else {
            Line::from(Span::styled(
                "\u{2500}".repeat(area.width as usize),
                Style::default().fg(Color::Rgb(50, 50, 50)),
            ))
        };
        frame.render_widget(
            Paragraph::new(status_line),
            Rect::new(area.x, status_y, area.width, 1),
        );

        // 4. Render Nano-Style Shortcuts Bar
        let chip = |key: &'static str, desc: &'static str| -> Vec<Span<'static>> {
            vec![
                Span::styled(
                    format!("^{key}"),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Rgb(200, 200, 200))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {desc}  "),
                    Style::default().fg(Color::Rgb(180, 180, 180)),
                ),
            ]
        };

        let mut shortcuts = Vec::new();
        shortcuts.extend(chip("S", "Save & Apply"));
        shortcuts.extend(chip("R", "Reset Default"));
        shortcuts.extend(chip("K", "Cut Line"));
        shortcuts.extend(chip("U", "Paste"));
        shortcuts.extend(chip("X", "Cancel"));

        frame.render_widget(
            Paragraph::new(Line::from(shortcuts)),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );

        // 5. Position cursor
        let cur_screen_x =
            area.x + gutter_w as u16 + (self.cursor_col.saturating_sub(self.scroll_left)) as u16;
        let cur_screen_y =
            area.y + header_h + (self.cursor_row.saturating_sub(self.scroll_top)) as u16;
        let bounded_x = cur_screen_x.min(area.x + area.width.saturating_sub(1));
        let bounded_y = cur_screen_y.min(area.y + area.height.saturating_sub(footer_h + 1));
        frame.set_cursor_position(Position::new(bounded_x, bounded_y));
    }
}
