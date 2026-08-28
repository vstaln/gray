//! Minimal retained-mode TUI components:
//! a retained component tree (`Container` of `Text`/`Spacer`/`Border`) whose
//! renderer word-wraps text itself at terminal width and repaints atomically,
//! so the terminal never soft-wraps behind our back and stale frame rows
//! can't survive a redraw. Everything is rebuilt wholesale:
//! container on every change, render, draw. Delete what you don't use.

use std::io::Write;

pub fn sanitize_user_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for t in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&t) {
                    break;
                }
            }
        } else if c.is_control() && c != '\n' && c != '\t' {
            continue;
        } else {
            out.push(c);
        }
    }
    out
}

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
        print!("\r{}{}\r\n", " ".repeat(pad), line);
    }
    let _ = std::io::stdout().flush();
}

pub const LOGO: &str = include_str!("../assets/logo.txt");
pub const LOGO_SMALL: &str = include_str!("../assets/logo_small.txt");

/// The gray logo (icon) as plain indented lines sized to the terminal —
/// for callers that insert it into scrollback rather than printing raw.
pub fn logo_lines() -> Vec<String> {
    let width = crossterm::terminal::size()
        .map(|(c, _)| c as usize)
        .unwrap_or(80);
    let logo = if width < 40 { LOGO_SMALL } else { LOGO };
    logo.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| format!("  {l}"))
        .collect()
}

pub fn blend_color(base: ratatui::style::Color, hilite: ratatui::style::Color, t: f32) -> ratatui::style::Color {
    use ratatui::style::Color;
    match (base, hilite) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let r = (r1 as f32 + (r2 as f32 - r1 as f32) * t).round() as u8;
            let g = (g1 as f32 + (g2 as f32 - g1 as f32) * t).round() as u8;
            let b = (b1 as f32 + (b2 as f32 - b1 as f32) * t).round() as u8;
            Color::Rgb(r, g, b)
        }
        _ => base,
    }
}

/// Prints the gray logo with a subtle diagonal shine gradient, sized to terminal.
/// Falls back to dim if NO_COLOR is set.
pub fn print_logo() {
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let lines = logo_lines();
    let rows = lines.len().max(1) as f32;
    let max_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f32;

    for (row, line) in lines.iter().enumerate() {
        if no_color {
            print!("\r\x1b[2m{line}\x1b[0m\r\n");
        } else {
            let mut formatted = String::new();
            for (col, ch) in line.chars().enumerate() {
                let diag = (col as f32 + (rows - 1.0 - row as f32)) / (max_w + rows);
                let t = (0.2 + 0.8 * diag).clamp(0.0, 1.0);
                let r = (100.0 + 140.0 * t).round() as u8;
                let g = (100.0 + 140.0 * t).round() as u8;
                let b = (100.0 + 140.0 * t).round() as u8;
                formatted.push_str(&format!("\x1b[38;2;{r};{g};{b}m{ch}"));
            }
            print!("\r{formatted}\x1b[0m\r\n");
        }
    }
    let _ = std::io::stdout().flush();
}

/// Clears the terminal; tty-only.
pub fn clear_screen() {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() && crossterm::terminal::size().is_ok() {
        print!("\x1b[2J\x1b[1;1H\r");
        let _ = std::io::stdout().flush();
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
}

