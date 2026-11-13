use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(crate) struct PtyPane {
    pub(crate) plugin: String,
    title: String,
    parser: Arc<Mutex<vt100::Parser>>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: (u16, u16),
    exited: Option<String>,
    esc_armed: bool,
}

pub(crate) enum PtyKey {
    Stay,
    Close,
}

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows: rows.max(2),
        cols: cols.max(10),
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl PtyPane {
    pub(crate) fn spawn(
        plugin: &str,
        title: String,
        argv: &[String],
        args: &[&str],
        inner: Rect,
    ) -> anyhow::Result<Self> {
        let size = pty_size(inner.height, inner.width);
        let pair = native_pty_system().openpty(size)?;
        let (prog, base) = argv
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("empty argv"))?;
        let mut cmd = CommandBuilder::new(prog);
        cmd.args(base);
        cmd.args(args);
        cmd.env("GRAY_BIN", super::logins::gray_bin());
        cmd.env("TERM", "xterm-256color");
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 500)));
        let sink = parser.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut p) = sink.lock() {
                            p.process(&buf[..n]);
                        }
                    }
                }
            }
        });
        Ok(Self {
            plugin: plugin.to_string(),
            title,
            parser,
            master: pair.master,
            writer,
            child,
            size: (size.rows, size.cols),
            exited: None,
            esc_armed: false,
        })
    }

    pub(crate) fn tick(&mut self, inner: Rect) {
        let want = pty_size(inner.height, inner.width);
        if (want.rows, want.cols) != self.size {
            self.size = (want.rows, want.cols);
            let _ = self.master.resize(want);
            if let Ok(mut p) = self.parser.lock() {
                p.set_size(want.rows, want.cols);
            }
        }
        if self.exited.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.exited = Some(if status.success() {
                "done".to_string()
            } else {
                format!("exited with {}", status.exit_code())
            });
        }
    }

    pub(crate) fn exited(&self) -> bool {
        self.exited.is_some()
    }

    fn send(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub(crate) fn paste(&mut self, text: &str) {
        if self.exited.is_some() {
            return;
        }
        let bracketed = self
            .parser
            .lock()
            .map(|p| p.screen().bracketed_paste())
            .unwrap_or(false);
        let mut bytes = Vec::new();
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.send(&bytes);
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> PtyKey {
        if self.exited.is_some() {
            return match key.code {
                KeyCode::Enter | KeyCode::Esc => PtyKey::Close,
                _ => PtyKey::Stay,
            };
        }
        if key.code == KeyCode::Esc {
            if std::mem::replace(&mut self.esc_armed, true) {
                return PtyKey::Close;
            }
            self.send(b"\x03");
            return PtyKey::Stay;
        }
        self.esc_armed = false;
        let app = self
            .parser
            .lock()
            .map(|p| p.screen().application_cursor())
            .unwrap_or(false);
        let arrow = |c: u8| {
            if app {
                vec![0x1b, b'O', c]
            } else {
                vec![0x1b, b'[', c]
            }
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let mut bytes = match key.code {
            KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => {
                vec![(c.to_ascii_lowercase() as u8) & 0x1f]
            }
            KeyCode::Char(c) => c.to_string().into_bytes(),
            KeyCode::Enter => vec![b'\r'],
            KeyCode::Backspace => vec![0x7f],
            KeyCode::Tab => vec![b'\t'],
            KeyCode::BackTab => b"\x1b[Z".to_vec(),
            KeyCode::Up => arrow(b'A'),
            KeyCode::Down => arrow(b'B'),
            KeyCode::Right => arrow(b'C'),
            KeyCode::Left => arrow(b'D'),
            KeyCode::Home => arrow(b'H'),
            KeyCode::End => arrow(b'F'),
            KeyCode::Delete => b"\x1b[3~".to_vec(),
            KeyCode::PageUp => b"\x1b[5~".to_vec(),
            KeyCode::PageDown => b"\x1b[6~".to_vec(),
            _ => return PtyKey::Stay,
        };
        if alt {
            bytes.insert(0, 0x1b);
        }
        self.send(&bytes);
        PtyKey::Stay
    }

    pub(crate) fn draw(&self, frame: &mut Frame, area: Rect) {
        let t = crate::theme::theme();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.text_faint))
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(t.text_body),
            ));
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block.style(Style::default().bg(t.raised_bg)), area);
        let Ok(parser) = self.parser.lock() else {
            return;
        };
        let screen = parser.screen();
        if let Some(status) = &self.exited {
            let room = inner.height.saturating_sub(2) as usize;
            let text = screen.contents();
            let rows: Vec<&str> = text.lines().collect();
            let end = rows
                .iter()
                .rposition(|l| !l.trim().is_empty())
                .map_or(0, |i| i + 1);
            let mut lines: Vec<Line> = rows[end.saturating_sub(room)..end]
                .iter()
                .map(|l| {
                    Line::from(Span::styled(
                        l.to_string(),
                        Style::default().fg(t.text_body),
                    ))
                })
                .collect();
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled(format!("{status} · "), Style::default().fg(t.text_dim)),
                Span::styled("press enter to return", Style::default().fg(t.accent)),
            ]));
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
            return;
        }
        let (crow, ccol) = screen.cursor_position();
        let buf = frame.buffer_mut();
        for r in 0..inner.height {
            for c in 0..inner.width {
                let Some(cell) = screen.cell(r, c) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut style = Style::default().fg(t.text_body);
                if let Some(color) = conv(cell.fgcolor()) {
                    style = style.fg(color);
                }
                if let Some(color) = conv(cell.bgcolor()) {
                    style = style.bg(color);
                }
                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() ^ (!screen.hide_cursor() && (r, c) == (crow, ccol)) {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let sym = cell.contents();
                if let Some(out) = buf.cell_mut((inner.x + c, inner.y + r)) {
                    out.set_symbol(if sym.is_empty() { " " } else { &sym });
                    out.set_style(style);
                }
            }
        }
    }
}

fn conv(c: vt100::Color) -> Option<Color> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

impl Drop for PtyPane {
    fn drop(&mut self) {
        if self.exited.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
