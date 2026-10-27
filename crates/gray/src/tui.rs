use std::io::{IsTerminal, Write};

pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
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

pub fn sanitize_user_text(text: &str) -> String {
    strip_ansi(text)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

pub fn visible_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(strip_ansi(s).as_str())
}

pub fn wrap(s: &str, width: usize) -> Vec<String> {
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
    let mut active_codes: Vec<String> = Vec::new();
    for word in &toks {
        let sep = usize::from(cur_w > 0);
        let w_w = visible_width(word);
        if cur_w + sep + w_w > width && cur_w > 0 {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;

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

        update_active_codes(&mut active_codes, word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

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

pub fn print_wrapped(text: &str, pad: usize) {
    let width = crate::term_width().saturating_sub(pad);
    let safe = unicode_safe();
    for line in wrap(text, width) {
        let line = if safe {
            line
        } else {
            ascii_symbol(&line).map_or(line, |s| s.into_owned())
        };
        print!("\r{}{}\r\n", " ".repeat(pad), line);
    }
    let _ = std::io::stdout().flush();
}

pub const LOGO: &str = include_str!("../assets/logo.txt");
pub const LOGO_ASCII: &str = include_str!("../assets/logo-ascii.txt");

#[cfg(windows)]
fn modern_windows_terminal() -> bool {
    use std::env::{var, var_os};
    var_os("WT_SESSION").is_some()
        || var_os("TERM_PROGRAM").is_some()
        || var_os("WEZTERM_PANE").is_some()
        || var_os("KONSOLE_VERSION").is_some()
        || var_os("VTE_VERSION").is_some()
        || var_os("ANSICON").is_some()
        || var_os("MSYSTEM").is_some()
        || var_os("TERM").is_some()
        || var("ConEmuANSI").is_ok_and(|v| v == "ON")
}

/// Bare conhost only gained a text renderer with font fallback in Windows 11
/// 22H2 (build 22621); before that, a glyph missing from the console font —
/// braille patterns are missing from Consolas entirely — draws a box. Gate
/// unicode art on bare consoles to builds that can actually shape it.
#[cfg(windows)]
fn console_font_fallback() -> bool {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    let status = unsafe { RtlGetVersion(&mut info) };
    status == 0 && info.dwBuildNumber >= 22621
}

pub fn unicode_safe() -> bool {
    match std::env::var("GRAY_ASCII_ART")
        .map(|v| v.trim().to_lowercase())
        .as_deref()
    {
        Ok("1" | "true" | "yes" | "on") => return false,
        Ok("0" | "false" | "no" | "off") => return true,
        _ => {}
    }
    if !std::io::stdout().is_terminal() {
        return true;
    }
    if std::env::var("TERM").is_ok_and(|t| t == "dumb") {
        return false;
    }
    #[cfg(windows)]
    {
        return modern_windows_terminal() || (ansi_supported() && console_font_fallback());
    }
    #[allow(unreachable_code)]
    true
}

/// Density ramp for a braille cell: the intro and the spinners bake
/// frames in braille space, so the ASCII path maps dots to a ramp.
pub(crate) fn braille_fallback(ch: char) -> char {
    if !('\u{2800}'..='\u{28ff}').contains(&ch) {
        return ch;
    }
    match (ch as u32 - 0x2800).count_ones() {
        0 => ' ',
        1..=2 => '.',
        3..=4 => '+',
        _ => '#',
    }
}

/// Map one decorative glyph to an ASCII stand-in. Returns None for ASCII
/// and for characters we never emit decoratively — letters, accents, CJK,
/// emoji — which pass through untouched: a CJK-locale console font has
/// those glyphs, and mangling user content is worse than a fallback box.
fn ascii_char(ch: char) -> Option<char> {
    Some(match ch {
        '\u{2800}'..='\u{28ff}' => braille_fallback(ch),
        '─' | '━' | '═' | '╌' | '╍' | '⎯' | '⏤' | '╼' | '╾' => '-',
        '│' | '┃' | '║' | '╎' | '╏' | '╽' | '╿' | '⌇' => '|',
        '╱' => '/',
        '╲' => '\\',
        '╳' => 'x',
        '\u{2500}'..='\u{257f}' => '+',
        '░' | '▁' | '▂' => '.',
        '▒' | '▃' | '▅' => ':',
        '▓' | '▆' | '▇' => '*',
        '▀' => '"',
        '▄' => '_',
        '\u{2580}'..='\u{259f}' => '#',
        '○' | '◌' | '◍' | '◎' | '◇' | '□' | '▢' | '▫' | '▯' | '◯' | '⬡' => {
            'o'
        }
        '●' | '◉' | '◆' | '◘' | '◙' => '*',
        '■' | '▪' | '▮' | '⬢' => '#',
        '▬' => '-',
        '←' | '↢' | '↤' | '⇐' | '⇤' | '⇦' | '⇷' | '⇺' | '⇽' | '↩' | '↫' | '↶' | '↺' | '↵' | '⏎'
        | '␍' | '↲' | '◀' | '◄' | '◂' | '◃' | '◁' | '⏴' | '⏮' | '⏪' => '<',
        '→' | '↣' | '↦' | '⇒' | '⇥' | '⇨' | '⇻' | '⇾' | '➔' | '➜' | '➤' | '➥' | '↪' | '↬' | '↷'
        | '↻' | '↳' | '⤷' | '⤳' | '▸' | '▹' | '▶' | '►' | '▻' | '⏵' | '⏩' | '⏭' | '‣' | '⏏' => {
            '>'
        }
        '↑' | '↟' | '↥' | '⇑' | '⇧' | '⇪' | '⇫' | '⇬' | '⇭' | '⇮' | '⇯' | '⇡' | '↰' | '↱' | '▴'
        | '▵' | '▲' | '△' | '⏶' | '⏫' | '⤒' => '^',
        '↓' | '↡' | '↧' | '⇓' | '⇩' | '⇣' | '↴' | '▾' | '▿' | '▼' | '▽' | '⏷' | '⏬' | '⤓'
        | '✓' | '✔' => 'v',
        '↔' | '↮' | '↹' | '⇄' | '⇆' | '⇋' | '⇌' | '⇍' | '⇏' | '⇶' | '⇹' | '⇼' | '⇴' | '⇿' => {
            '-'
        }
        '↕' | '↨' | '⇅' | '⇕' | '⇳' | '⇵' | '⇎' => '|',
        '↗' | '⇗' => '/',
        '↘' | '⇘' => '\\',
        '↖' | '⇖' => '\\',
        '↙' | '⇙' => '/',
        '\u{2190}'..='\u{21ff}' | '\u{27f0}'..='\u{27ff}' | '\u{2900}'..='\u{297f}' => '>',
        '✗' | '✘' | '✖' | '⨯' | '☓' | '✕' => 'x',
        '❯' | '❱' => '>',
        '⎿' | '⏊' | '⏉' | '⌐' | '⌜' | '⌝' | '⌞' | '⌟' | '⌈' | '⌉' | '⌊' | '⌋' => {
            '-'
        }
        '␣' => '_',
        '⚠' | '⚡' | '❗' | '❕' | '‼' => '!',
        '❓' | '❔' => '?',
        '⌘' | '⌥' | '⏻' | '⏼' | '⏽' | '⏾' | '⭘' => '*',
        '\u{2300}'..='\u{23ff}' => '*',
        '\u{25a0}'..='\u{25ff}' => '*',
        '\u{2600}'..='\u{26ff}' => '*',
        '\u{2700}'..='\u{27bf}' => '*',
        '\u{2b00}'..='\u{2bff}' => '*',
        _ => return None,
    })
}

/// Transliterate a cell's symbol for consoles that cannot shape decorative
/// glyphs (bare Windows 10 conhost fonts carry no braille or dingbats at
/// all). Returns the replacement, or None to leave the symbol as written —
/// plain ASCII, Latin-1 punctuation, CJK, emoji, and control sequences
/// (hyperlinks, kitty placeholders) all pass through untouched.
pub(crate) fn ascii_symbol(symbol: &str) -> Option<std::borrow::Cow<'static, str>> {
    if symbol.is_ascii() || symbol.starts_with('\x1b') {
        return None;
    }
    if !symbol.chars().any(|ch| ascii_char(ch).is_some()) {
        return None;
    }
    Some(
        symbol
            .chars()
            .map(|ch| ascii_char(ch).unwrap_or(ch))
            .collect::<String>()
            .into(),
    )
}

pub(crate) fn arrow_glyph() -> &'static str {
    glyph("❯", ">")
}

pub(crate) fn glyph(unicode: &'static str, ascii: &'static str) -> &'static str {
    if unicode_safe() { unicode } else { ascii }
}

pub fn logo_mark() -> &'static [String] {
    static MARK: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    MARK.get_or_init(|| {
        LOGO.lines()
            .map(|l| l.trim_end_matches('\u{2800}').to_string())
            .collect()
    })
}

pub fn logo_lines() -> Vec<String> {
    let rows: Vec<String> = if unicode_safe() {
        logo_mark().to_vec()
    } else {
        LOGO_ASCII.lines().map(str::to_string).collect()
    };
    rows.into_iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| format!("  {l}"))
        .collect()
}

pub fn blend_color(
    base: ratatui::style::Color,
    hilite: ratatui::style::Color,
    t: f32,
) -> ratatui::style::Color {
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

#[cfg(windows)]
pub(crate) fn ansi_supported() -> bool {
    crossterm::ansi_support::supports_ansi()
}

#[cfg(not(windows))]
pub(crate) fn ansi_supported() -> bool {
    true
}

/// SGR sequence selecting `color` for the foreground / background:
/// `38;2`/`48;2` only when `color` is still RGB, `38;5`/`48;5` for a
/// 256-color index, a named `30-37`/`90-97` (or `40-47`/`100-107`) code for
/// ANSI colors, and the default `39`/`49` for `Reset`. Feed colors through
/// `gray_markdown::adapt_ratatui_color` first and the emitted escape always
/// matches the terminal's level.
pub(crate) fn sgr_fg(color: ratatui::style::Color) -> String {
    sgr_color(color, false)
}

pub(crate) fn sgr_bg(color: ratatui::style::Color) -> String {
    sgr_color(color, true)
}

fn sgr_color(color: ratatui::style::Color, bg: bool) -> String {
    use ratatui::style::Color as C;
    match color {
        C::Rgb(r, g, b) => format!("\x1b[{};2;{r};{g};{b}m", if bg { 48 } else { 38 }),
        C::Indexed(i) => format!("\x1b[{};5;{i}m", if bg { 48 } else { 38 }),
        C::Reset => format!("\x1b[{}m", if bg { 49 } else { 39 }),
        named => {
            let code = match named {
                C::Black => 30,
                C::Red => 31,
                C::Green => 32,
                C::Yellow => 33,
                C::Blue => 34,
                C::Magenta => 35,
                C::Cyan => 36,
                C::Gray => 37,
                C::DarkGray => 90,
                C::LightRed => 91,
                C::LightGreen => 92,
                C::LightYellow => 93,
                C::LightBlue => 94,
                C::LightMagenta => 95,
                C::LightCyan => 96,
                C::White => 97,
                _ => 39,
            };
            format!("\x1b[{}m", code + if bg { 10 } else { 0 })
        }
    }
}

pub fn print_logo() {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return;
    }
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let ansi = ansi_supported();
    let lines = logo_lines();
    let rows = lines.len().max(1) as f32;
    let max_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f32;

    for (row, line) in lines.iter().enumerate() {
        if !ansi {
            print!("\r{line}\r\n");
        } else if no_color {
            print!("\r\x1b[2m{line}\x1b[0m\r\n");
        } else {
            let mut formatted = String::new();
            for (col, ch) in line.chars().enumerate() {
                let diag = (col as f32 + (rows - 1.0 - row as f32)) / (max_w + rows);
                let t = (0.2 + 0.8 * diag).clamp(0.0, 1.0);
                let r = (100.0 + 140.0 * t).round() as u8;
                let g = (100.0 + 140.0 * t).round() as u8;
                let b = (100.0 + 140.0 * t).round() as u8;
                let c = gray_markdown::adapt_ratatui_color(ratatui::style::Color::Rgb(r, g, b));
                formatted.push_str(&sgr_fg(c));
                formatted.push(ch);
            }
            print!("\r{formatted}\x1b[0m\r\n");
        }
    }
    let _ = std::io::stdout().flush();
}

pub fn clear_screen() {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() && crossterm::terminal::size().is_ok() {
        print!("\x1b[2J\x1b[1;1H\r");
        let _ = std::io::stdout().flush();
    }
}
