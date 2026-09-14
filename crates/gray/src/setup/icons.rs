//! Hexagon context glyphs with pure-ASCII fallback.
//! One family, three states: filled = taken, hollow = free/reserved.
//! Deliberately not Claude Code's cylinders — same idea, different shape.
use std::sync::OnceLock;

static NERD_FONT: OnceLock<bool> = OnceLock::new();

fn env_override() -> Option<bool> {
    match std::env::var("GRAY_NERD_FONT")
        .unwrap_or_default()
        .trim()
        .to_lowercase()
        .as_str()
    {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// Cached Nerd Font availability: `GRAY_NERD_FONT` opt-in, ASCII otherwise.
/// Auto-detection (TERM_PROGRAM / installed font files) guessed wrong often
/// enough (ssh, tmux passthrough, dotfile-managed fonts) that the explicit
/// env var is the only signal trusted.
pub fn has_nerd_font() -> bool {
    *NERD_FONT.get_or_init(|| env_override().unwrap_or(false))
}

/// Icon by name: hexagons when available, else pure ASCII.
/// `cell` = used (tinted per category), `cell_free` = free (dim),
/// `cell_buffer` = autocompact buffer (rose open-centre asterisk).
pub fn icon(name: &str) -> &'static str {
    if has_nerd_font() {
        match name {
            "cell" => "⬢",
            "cell_free" => "⬡",
            "cell_buffer" => "✲",
            "arrow" => "❯",
            _ => "?",
        }
    } else {
        match name {
            "cell" => "#",
            "cell_free" => ".",
            "cell_buffer" => "x",
            "arrow" => ">",
            _ => "?",
        }
    }
}
