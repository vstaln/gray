//! Terminal color support detection and color conversion utilities.
//!
//! This module provides functionality to detect the terminal's color capabilities
//! and downgrade RGB colors to the appropriate level when needed.

use std::sync::OnceLock;

use anstyle::{Ansi256Color, AnsiColor, Color, Effects, RgbColor};

/// The level of color support detected for the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ColorLevel {
    /// No color support (monochrome terminals)
    None,
    /// Basic 16-color ANSI support (colors 0-15)
    Basic,
    /// 256-color support (colors 0-255)
    Ansi256,
    /// 24-bit truecolor RGB support (16 million colors)
    #[default]
    TrueColor,
}

impl ColorLevel {
    /// Returns true if at least basic color is supported.
    pub fn has_color(self) -> bool {
        self >= Self::Basic
    }

    /// Returns true if 256-color mode is supported.
    pub fn has_256(self) -> bool {
        self >= Self::Ansi256
    }

    /// Returns true if 24-bit truecolor is supported.
    pub fn has_truecolor(self) -> bool {
        self >= Self::TrueColor
    }
}

static COLOR_LEVEL: OnceLock<ColorLevel> = OnceLock::new();

/// Detect the terminal's color support level.
///
/// This uses the `supports-color` crate which checks:
/// - `COLORTERM` environment variable (for truecolor detection)
/// - `TERM` environment variable
/// - Terminal-specific environment variables (like `ITERM_SESSION_ID`)
/// - Whether stdout is a TTY
///
/// The result is cached after the first call.
pub fn detect_color_level() -> ColorLevel {
    *COLOR_LEVEL.get_or_init(|| {
        // Explicit opt-out via NO_COLOR takes priority.
        if std::env::var_os("NO_COLOR").is_some() {
            return ColorLevel::None;
        }

        let level = match supports_color::on(supports_color::Stream::Stdout) {
            // Not a TTY (tests, piped) — default to TrueColor.
            // The pager is a TUI app that always runs inside a terminal;
            // stdout may not be a TTY when the pager renders to stderr.
            None => ColorLevel::TrueColor,
            Some(level) => {
                if level.has_16m {
                    ColorLevel::TrueColor
                } else if level.has_256 {
                    ColorLevel::Ansi256
                } else if level.has_basic {
                    ColorLevel::Basic
                } else {
                    ColorLevel::None
                }
            }
        };

        // The `supports-color` crate relies on COLORTERM=truecolor, but
        // tmux/SSH/mosh often strip that variable.  When the crate reports
        // only 256-color support, upgrade to TrueColor if we can identify
        // a known truecolor-capable terminal via its env vars.
        if level < ColorLevel::TrueColor && terminal_supports_truecolor() {
            return ColorLevel::TrueColor;
        }

        level
    })
}

/// Check whether the terminal emulator is known to support truecolor.
///
/// Used as a fallback when `COLORTERM` is missing (e.g. inside tmux or over
/// SSH).  Checks terminal-specific env vars that survive session forwarding
/// even when `COLORTERM` and `TERM_PROGRAM` are stripped.
fn terminal_supports_truecolor() -> bool {
    use std::env;

    // TERM_PROGRAM is the most reliable signal (set by the emulator itself).
    if let Ok(prog) = env::var("TERM_PROGRAM") {
        let norm: String = prog
            .trim()
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_' | '.'))
            .map(|c| c.to_ascii_lowercase())
            .collect();
        // Every modern terminal except Apple Terminal supports truecolor.
        if matches!(
            norm.as_str(),
            "iterm"
                | "iterm2"
                | "itermapp"
                | "ghostty"
                | "kitty"
                | "wezterm"
                | "alacritty"
                | "warp"
                | "warpterminal"
                | "vscode"
        ) {
            return true;
        }
    }

    // Terminal-specific env vars that often survive tmux/SSH.
    env::var("ITERM_SESSION_ID").is_ok()
        || env::var("ITERM_PROFILE").is_ok()
        || env::var("WEZTERM_VERSION").is_ok()
        || env::var("KITTY_WINDOW_ID").is_ok()
        || env::var("ALACRITTY_SOCKET").is_ok()
}

/// Convert an `anstyle::Color` to the appropriate level based on terminal support.
///
/// This will downgrade colors as needed:
/// - TrueColor terminals: pass through unchanged
/// - 256-color terminals: RGB colors are converted to closest ANSI 256 color
/// - Basic terminals: colors are converted to closest ANSI 16 color
/// - No color: returns None
pub fn adapt_color(color: Color) -> Option<Color> {
    let level = detect_color_level();

    match level {
        ColorLevel::None => None,
        ColorLevel::TrueColor => Some(color),
        ColorLevel::Ansi256 => Some(match color {
            Color::Rgb(rgb) => Color::Ansi256(rgb_to_ansi256(rgb)),
            other => other,
        }),
        ColorLevel::Basic => Some(match color {
            Color::Rgb(rgb) => Color::Ansi(rgb_to_ansi16(rgb)),
            Color::Ansi256(idx) => Color::Ansi(ansi256_to_ansi16(idx)),
            Color::Ansi(ansi) => Color::Ansi(ansi),
        }),
    }
}

/// Convert an `anstyle::Style` to the appropriate color level.
pub fn adapt_style(style: anstyle::Style) -> anstyle::Style {
    let fg = style.get_fg_color().and_then(adapt_color);
    let bg = style.get_bg_color().and_then(adapt_color);
    let effects = style.get_effects();

    let mut new_style = anstyle::Style::new();
    if let Some(fg) = fg {
        new_style = new_style.fg_color(Some(fg));
    }
    if let Some(bg) = bg {
        new_style = new_style.bg_color(Some(bg));
    }
    new_style | effects
}

/// Convert an RGB color to the closest ANSI 256-color palette entry.
pub fn rgb_to_ansi256(rgb: RgbColor) -> Ansi256Color {
    anstyle_lossy::rgb_to_xterm(rgb)
}

/// Convert an RGB color to the closest basic ANSI 16-color.
pub fn rgb_to_ansi16(rgb: RgbColor) -> AnsiColor {
    anstyle_lossy::rgb_to_ansi(rgb, anstyle_lossy::palette::VGA)
}

/// Convert an ANSI 256-color to the closest basic ANSI 16-color.
pub fn ansi256_to_ansi16(idx: Ansi256Color) -> AnsiColor {
    anstyle_lossy::xterm_to_ansi(idx, anstyle_lossy::palette::VGA)
}

/// Convert an anstyle style to a ratatui style.
pub(crate) fn anstyle_to_ratatui_style(style: anstyle::Style) -> ratatui::style::Style {
    use ratatui::style::{Modifier, Style as RStyle};

    let mut out = RStyle::default();

    if let Some(fg) = style.get_fg_color() {
        out = out.fg(anstyle_to_ratatui_color(fg));
    }
    if let Some(bg) = style.get_bg_color() {
        out = out.bg(anstyle_to_ratatui_color(bg));
    }

    let effects = style.get_effects();
    let mut modifiers = Modifier::empty();
    if effects.contains(Effects::BOLD) {
        modifiers |= Modifier::BOLD;
    }
    if effects.contains(Effects::DIMMED) {
        modifiers |= Modifier::DIM;
    }
    if effects.contains(Effects::ITALIC) {
        modifiers |= Modifier::ITALIC;
    }
    if effects.contains(Effects::UNDERLINE) {
        modifiers |= Modifier::UNDERLINED;
    }
    if effects.contains(Effects::STRIKETHROUGH) {
        modifiers |= Modifier::CROSSED_OUT;
    }
    if effects.contains(Effects::HIDDEN) {
        modifiers |= Modifier::HIDDEN;
    }

    out.add_modifier(modifiers)
}

pub(crate) fn anstyle_to_ratatui_color(color: anstyle::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match color {
        anstyle::Color::Ansi(ansi) => match ansi {
            anstyle::AnsiColor::Black => Color::Black,
            anstyle::AnsiColor::Red => Color::Red,
            anstyle::AnsiColor::Green => Color::Green,
            anstyle::AnsiColor::Yellow => Color::Yellow,
            anstyle::AnsiColor::Blue => Color::Blue,
            anstyle::AnsiColor::Magenta => Color::Magenta,
            anstyle::AnsiColor::Cyan => Color::Cyan,
            anstyle::AnsiColor::White => Color::Gray,
            anstyle::AnsiColor::BrightBlack => Color::DarkGray,
            anstyle::AnsiColor::BrightRed => Color::LightRed,
            anstyle::AnsiColor::BrightGreen => Color::LightGreen,
            anstyle::AnsiColor::BrightYellow => Color::LightYellow,
            anstyle::AnsiColor::BrightBlue => Color::LightBlue,
            anstyle::AnsiColor::BrightMagenta => Color::LightMagenta,
            anstyle::AnsiColor::BrightCyan => Color::LightCyan,
            anstyle::AnsiColor::BrightWhite => Color::White,
        },
        anstyle::Color::Ansi256(idx) => Color::Indexed(idx.index()),
        anstyle::Color::Rgb(rgb) => Color::Rgb(rgb.0, rgb.1, rgb.2),
    }
}
