//! Named color themes (codewhale `palette/` port, gray-sized).
//!
//! Three layers, same as upstream:
//! 1. One [`UiTheme`] struct of semantic roles — every TUI call site reads
//!    `theme().role()` instead of a hardcoded `Color::Rgb`.
//! 2. Built-in presets ([`GRAY_UI_THEME`] default + community themes +
//!    [`TERMINAL_UI_THEME`] which inherits the host terminal scheme).
//! 3. Runtime selection: `--theme` flag > `GRAY_THEME` env > saved config
//!    `theme` > default. `/theme [name]` switches live (call sites read the
//!    global on every draw, so no restart is needed).
//!
//! Deliberately NOT ported: depth adaptation (`adapt.rs`), WCAG contrast
//! (`contrast.rs`), OSC11 background detect (`detect.rs`/`osc11.rs`), and
//! user theme files (`user_theme.rs`) — gray has no light themes yet and no
//! background painting, so there is nothing to adapt. Revisit if custom
//! `~/.gray/themes/*.toml` files happen.

use ratatui::style::Color;
use std::sync::{OnceLock, RwLock};

/// Semantic color roles for the whole TUI. Field order is stable; add new
/// roles at the end-adjacent group, never renumber (const initializers below
/// are positional-independent, but diffs stay reviewable when grouped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiTheme {
    pub name: &'static str,
    // Surfaces
    pub surface_bg: Color,
    pub raised_bg: Color,
    pub input_bg: Color,
    pub selection_bg: Color,
    pub on_selection: Color,
    pub chip_bg: Color,
    // Text ramp (faint → bright)
    pub text_faint: Color,
    pub text_dim: Color,
    pub text_muted: Color,
    pub text_soft: Color,
    pub text_body: Color,
    pub text_bright: Color,
    // Brand accents
    pub accent: Color,
    pub accent_soft: Color,
    // Status
    pub success: Color,
    pub error: Color,
    pub error_soft: Color,
    pub rose: Color,
    pub info: Color,
    // Tool-call rendering (GrokNight/TokyoNight heritage)
    pub tool_accent: Color,
    pub tool_path: Color,
    pub tool_command: Color,
    pub tool_dim: Color,
    // Diffs
    pub diff_add_fg: Color,
    pub diff_add_bg: Color,
    pub diff_del_fg: Color,
    pub diff_del_bg: Color,
    pub diff_gutter: Color,
    // Footer cache-hit readout (positive-neutral, distinct from success)
    pub cache_hit: Color,
    // /context category pastels
    pub ctx_system: Color,
    pub ctx_context: Color,
    pub ctx_tools: Color,
    pub ctx_skills: Color,
    pub ctx_messages: Color,
    pub ctx_free: Color,
    pub ctx_reserve: Color,
    // Marketplace source badges — external brand colors, intentionally
    // identical across themes (like the logo).
    pub badge_pi: Color,
    pub badge_claw: Color,
    pub badge_claude: Color,
    // Shimmer sweep base (highlight is text_bright)
    pub shimmer_base: Color,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Default `gray` theme: today's exact colors, pixel-identical. The neutral
/// gray ramp plus the peach accent and TokyoNight tool/diff slots.
pub const GRAY_UI_THEME: UiTheme = UiTheme {
    name: "gray",
    surface_bg: rgb(22, 22, 22),
    raised_bg: rgb(28, 28, 28),
    input_bg: rgb(32, 32, 32),
    selection_bg: rgb(246, 173, 126),
    on_selection: Color::Black,
    chip_bg: rgb(200, 200, 200),
    text_faint: rgb(70, 70, 70),
    text_dim: rgb(110, 110, 110),
    text_muted: rgb(140, 140, 140),
    text_soft: rgb(180, 180, 180),
    text_body: rgb(225, 225, 225),
    text_bright: rgb(240, 240, 240),
    accent: rgb(246, 173, 126),
    accent_soft: rgb(254, 215, 170),
    success: rgb(74, 222, 128),
    error: rgb(239, 68, 68),
    error_soft: rgb(220, 120, 120),
    rose: rgb(254, 205, 211),
    info: rgb(59, 130, 246),
    tool_accent: rgb(158, 206, 106),
    tool_path: rgb(255, 158, 100),
    tool_command: rgb(224, 175, 104),
    tool_dim: rgb(108, 108, 108),
    diff_add_fg: rgb(158, 206, 106),
    diff_add_bg: rgb(24, 50, 32),
    diff_del_fg: rgb(247, 118, 142),
    diff_del_bg: rgb(55, 25, 28),
    diff_gutter: rgb(108, 108, 108),
    cache_hit: rgb(130, 145, 130),
    ctx_system: rgb(203, 213, 225),
    ctx_context: rgb(167, 243, 208),
    ctx_tools: rgb(186, 230, 253),
    ctx_skills: rgb(217, 249, 157),
    ctx_messages: rgb(254, 240, 138),
    ctx_free: rgb(100, 116, 139),
    ctx_reserve: rgb(254, 205, 211),
    badge_pi: rgb(125, 211, 252),
    badge_claw: rgb(134, 239, 172),
    badge_claude: rgb(196, 181, 253),
    shimmer_base: rgb(150, 148, 144),
};

/// Tokyo Night (hexes cross-checked with opencode's
/// `reference/opencode/packages/tui/src/theme/assets/tokyonight.json` and
/// codewhale's `TOKYO_NIGHT_UI_THEME`). Gray's tool slots are TokyoNight
/// natives, so they carry over unchanged.
pub const TOKYO_NIGHT_UI_THEME: UiTheme = UiTheme {
    name: "tokyo-night",
    surface_bg: rgb(0x16, 0x16, 0x1e),
    raised_bg: rgb(0x1a, 0x1b, 0x26),
    input_bg: rgb(0x29, 0x2e, 0x42),
    selection_bg: rgb(0x28, 0x34, 0x57),
    on_selection: rgb(0xc0, 0xca, 0xf5),
    chip_bg: rgb(0x41, 0x48, 0x68),
    text_faint: rgb(0x41, 0x48, 0x68),
    text_dim: rgb(0x5c, 0x65, 0x8d),
    text_muted: rgb(0xa9, 0xb1, 0xd6),
    text_soft: rgb(0xbb, 0xc2, 0xe0),
    text_body: rgb(0xc0, 0xca, 0xf5),
    text_bright: rgb(0xff, 0xff, 0xff),
    accent: rgb(0xff, 0x9e, 0x64),
    accent_soft: rgb(0xe0, 0xaf, 0x68),
    success: rgb(0x9e, 0xce, 0x6a),
    error: rgb(0xf7, 0x76, 0x8e),
    error_soft: rgb(0xf9, 0x92, 0xa4),
    rose: rgb(0xfa, 0xcc, 0xd4),
    info: rgb(0x7d, 0xcf, 0xff),
    tool_accent: rgb(158, 206, 106),
    tool_path: rgb(255, 158, 100),
    tool_command: rgb(224, 175, 104),
    tool_dim: rgb(0x56, 0x5f, 0x89),
    diff_add_fg: rgb(0x9e, 0xce, 0x6a),
    diff_add_bg: rgb(0x1b, 0x2b, 0x1f),
    diff_del_fg: rgb(0xf7, 0x76, 0x8e),
    diff_del_bg: rgb(0x33, 0x1c, 0x24),
    diff_gutter: rgb(0x56, 0x5f, 0x89),
    cache_hit: rgb(0x73, 0x7a, 0xa2),
    ctx_system: rgb(0xbb, 0xc2, 0xe0),
    ctx_context: rgb(0x9e, 0xce, 0x6a),
    ctx_tools: rgb(0x7d, 0xcf, 0xff),
    ctx_skills: rgb(0x73, 0xd3, 0xa7),
    ctx_messages: rgb(0xe0, 0xaf, 0x68),
    ctx_free: rgb(0x56, 0x5f, 0x89),
    ctx_reserve: rgb(0xfa, 0xcc, 0xd4),
    badge_pi: rgb(125, 211, 252),
    badge_claw: rgb(134, 239, 172),
    badge_claude: rgb(196, 181, 253),
    shimmer_base: rgb(0x56, 0x5f, 0x89),
};

/// Dracula (from codewhale's `DRACULA_UI_THEME`).
pub const DRACULA_UI_THEME: UiTheme = UiTheme {
    name: "dracula",
    surface_bg: rgb(0x21, 0x22, 0x2c),
    raised_bg: rgb(0x28, 0x2a, 0x36),
    input_bg: rgb(0x34, 0x37, 0x46),
    selection_bg: rgb(0x44, 0x47, 0x5a),
    on_selection: rgb(0xf8, 0xf8, 0xf2),
    chip_bg: rgb(0x44, 0x47, 0x5a),
    text_faint: rgb(0x44, 0x47, 0x5a),
    text_dim: rgb(0x62, 0x72, 0xa4),
    text_muted: rgb(0xc0, 0xc4, 0xd6),
    text_soft: rgb(0xe2, 0xe2, 0xdc),
    text_body: rgb(0xf8, 0xf8, 0xf2),
    text_bright: rgb(0xff, 0xff, 0xff),
    accent: rgb(0xff, 0xb8, 0x6c),
    accent_soft: rgb(0xf1, 0xfa, 0x8c),
    success: rgb(0x50, 0xfa, 0x7b),
    error: rgb(0xff, 0x55, 0x55),
    error_soft: rgb(0xff, 0x7c, 0x7c),
    rose: rgb(0xff, 0xbb, 0xbb),
    info: rgb(0x8b, 0xe9, 0xfd),
    tool_accent: rgb(0x50, 0xfa, 0x7b),
    tool_path: rgb(0xff, 0xb8, 0x6c),
    tool_command: rgb(0xf1, 0xfa, 0x8c),
    tool_dim: rgb(0x62, 0x72, 0xa4),
    diff_add_fg: rgb(0x50, 0xfa, 0x7b),
    diff_add_bg: rgb(0x21, 0x3a, 0x2a),
    diff_del_fg: rgb(0xff, 0x55, 0x55),
    diff_del_bg: rgb(0x3a, 0x1f, 0x22),
    diff_gutter: rgb(0x62, 0x72, 0xa4),
    cache_hit: rgb(0x8a, 0x8e, 0xaa),
    ctx_system: rgb(0xe2, 0xe2, 0xdc),
    ctx_context: rgb(0x50, 0xfa, 0x7b),
    ctx_tools: rgb(0x8b, 0xe9, 0xfd),
    ctx_skills: rgb(0x69, 0xff, 0x94),
    ctx_messages: rgb(0xf1, 0xfa, 0x8c),
    ctx_free: rgb(0x62, 0x72, 0xa4),
    ctx_reserve: rgb(0xff, 0xbb, 0xbb),
    badge_pi: rgb(125, 211, 252),
    badge_claw: rgb(134, 239, 172),
    badge_claude: rgb(196, 181, 253),
    shimmer_base: rgb(0x62, 0x72, 0xa4),
};

/// Catppuccin Mocha (from codewhale's `CATPPUCCIN_MOCHA_UI_THEME`).
pub const CATPPUCCIN_MOCHA_UI_THEME: UiTheme = UiTheme {
    name: "catppuccin-mocha",
    surface_bg: rgb(0x18, 0x18, 0x25),
    raised_bg: rgb(0x1e, 0x1e, 0x2e),
    input_bg: rgb(0x31, 0x32, 0x44),
    selection_bg: rgb(0x45, 0x47, 0x5a),
    on_selection: rgb(0xcd, 0xd6, 0xf4),
    chip_bg: rgb(0x45, 0x47, 0x5a),
    text_faint: rgb(0x45, 0x47, 0x5a),
    text_dim: rgb(0x6c, 0x70, 0x86),
    text_muted: rgb(0xa6, 0xad, 0xc8),
    text_soft: rgb(0xba, 0xc2, 0xde),
    text_body: rgb(0xcd, 0xd6, 0xf4),
    text_bright: rgb(0xff, 0xff, 0xff),
    accent: rgb(0xfa, 0xb3, 0x87),
    accent_soft: rgb(0xf9, 0xe2, 0xaf),
    success: rgb(0xa6, 0xe3, 0xa1),
    error: rgb(0xf3, 0x8b, 0xa8),
    error_soft: rgb(0xf5, 0xa2, 0xbc),
    rose: rgb(0xf5, 0xc2, 0xd0),
    info: rgb(0x89, 0xd9, 0xeb),
    tool_accent: rgb(0xa6, 0xe3, 0xa1),
    tool_path: rgb(0xfa, 0xb3, 0x87),
    tool_command: rgb(0xf9, 0xe2, 0xaf),
    tool_dim: rgb(0x6c, 0x70, 0x86),
    diff_add_fg: rgb(0xa6, 0xe3, 0xa1),
    diff_add_bg: rgb(0x1f, 0x33, 0x29),
    diff_del_fg: rgb(0xf3, 0x8b, 0xa8),
    diff_del_bg: rgb(0x3a, 0x1f, 0x2a),
    diff_gutter: rgb(0x6c, 0x70, 0x86),
    cache_hit: rgb(0x7f, 0x84, 0x9c),
    ctx_system: rgb(0xba, 0xc2, 0xde),
    ctx_context: rgb(0xa6, 0xe3, 0xa1),
    ctx_tools: rgb(0x74, 0xc7, 0xec),
    ctx_skills: rgb(0x94, 0xe2, 0xd5),
    ctx_messages: rgb(0xf9, 0xe2, 0xaf),
    ctx_free: rgb(0x6c, 0x70, 0x86),
    ctx_reserve: rgb(0xf5, 0xc2, 0xd0),
    badge_pi: rgb(125, 211, 252),
    badge_claw: rgb(134, 239, 172),
    badge_claude: rgb(196, 181, 253),
    shimmer_base: rgb(0x6c, 0x70, 0x86),
};

/// Gruvbox Dark (from codewhale's `GRUVBOX_DARK_UI_THEME`).
pub const GRUVBOX_DARK_UI_THEME: UiTheme = UiTheme {
    name: "gruvbox-dark",
    surface_bg: rgb(0x1d, 0x20, 0x21),
    raised_bg: rgb(0x28, 0x28, 0x28),
    input_bg: rgb(0x3c, 0x38, 0x36),
    selection_bg: rgb(0x66, 0x5c, 0x54),
    on_selection: rgb(0xeb, 0xdb, 0xb2),
    chip_bg: rgb(0x66, 0x5c, 0x54),
    text_faint: rgb(0x66, 0x5c, 0x54),
    text_dim: rgb(0x92, 0x83, 0x74),
    text_muted: rgb(0xc5, 0xb8, 0xa0),
    text_soft: rgb(0xd5, 0xc4, 0xa1),
    text_body: rgb(0xeb, 0xdb, 0xb2),
    text_bright: rgb(0xfb, 0xeb, 0xc9),
    accent: rgb(0xfe, 0x80, 0x19),
    accent_soft: rgb(0xfa, 0xbd, 0x2f),
    success: rgb(0x8e, 0xc0, 0x7c),
    error: rgb(0xfb, 0x49, 0x34),
    error_soft: rgb(0xfc, 0x7c, 0x6b),
    rose: rgb(0xfc, 0xc4, 0xb8),
    info: rgb(0x83, 0xa5, 0x98),
    tool_accent: rgb(0x8e, 0xc0, 0x7c),
    tool_path: rgb(0xfe, 0x80, 0x19),
    tool_command: rgb(0xfa, 0xbd, 0x2f),
    tool_dim: rgb(0x92, 0x83, 0x74),
    diff_add_fg: rgb(0x8e, 0xc0, 0x7c),
    diff_add_bg: rgb(0x29, 0x32, 0x16),
    diff_del_fg: rgb(0xfb, 0x49, 0x34),
    diff_del_bg: rgb(0x35, 0x1c, 0x18),
    diff_gutter: rgb(0x92, 0x83, 0x74),
    cache_hit: rgb(0xa8, 0x99, 0x84),
    ctx_system: rgb(0xd5, 0xc4, 0xa1),
    ctx_context: rgb(0x8e, 0xc0, 0x7c),
    ctx_tools: rgb(0x83, 0xa5, 0x98),
    ctx_skills: rgb(0x8e, 0xc0, 0x7c),
    ctx_messages: rgb(0xfa, 0xbd, 0x2f),
    ctx_free: rgb(0x92, 0x83, 0x74),
    ctx_reserve: rgb(0xfc, 0xc4, 0xb8),
    badge_pi: rgb(125, 211, 252),
    badge_claw: rgb(134, 239, 172),
    badge_claude: rgb(196, 181, 253),
    shimmer_base: rgb(0x92, 0x83, 0x74),
};

/// Claude (warm navy + coral, from codewhale's `CLAUDE_UI_THEME`).
pub const CLAUDE_UI_THEME: UiTheme = UiTheme {
    name: "claude",
    surface_bg: rgb(0x18, 0x17, 0x15),
    raised_bg: rgb(0x1f, 0x1e, 0x1b),
    input_bg: rgb(0x25, 0x23, 0x20),
    selection_bg: rgb(0x30, 0x2d, 0x28),
    on_selection: rgb(0xfa, 0xf9, 0xf5),
    chip_bg: rgb(0x30, 0x2d, 0x28),
    text_faint: rgb(0x30, 0x2d, 0x28),
    text_dim: rgb(0x72, 0x70, 0x6a),
    text_muted: rgb(0xa0, 0x9d, 0x96),
    text_soft: rgb(0xd0, 0xcd, 0xc5),
    text_body: rgb(0xfa, 0xf9, 0xf5),
    text_bright: rgb(0xff, 0xff, 0xff),
    accent: rgb(0xcc, 0x78, 0x5c),
    accent_soft: rgb(0xe8, 0xa5, 0x5a),
    success: rgb(0x5d, 0xb8, 0x72),
    error: rgb(0xe0, 0x60, 0x60),
    error_soft: rgb(0xd9, 0x66, 0x66),
    rose: rgb(0xe8, 0xb8, 0xb8),
    info: rgb(0x5d, 0xb8, 0xa6),
    tool_accent: rgb(0x5d, 0xb8, 0x72),
    tool_path: rgb(0xe8, 0xa5, 0x5a),
    tool_command: rgb(0xd4, 0xa0, 0x17),
    tool_dim: rgb(0x72, 0x70, 0x6a),
    diff_add_fg: rgb(0x5d, 0xb8, 0x72),
    diff_add_bg: rgb(0x1a, 0x24, 0x1d),
    diff_del_fg: rgb(0xc6, 0x45, 0x45),
    diff_del_bg: rgb(0x24, 0x1a, 0x1a),
    diff_gutter: rgb(0x72, 0x70, 0x6a),
    cache_hit: rgb(0x7d, 0x7a, 0x73),
    ctx_system: rgb(0xd0, 0xcd, 0xc5),
    ctx_context: rgb(0x5d, 0xb8, 0x72),
    ctx_tools: rgb(0x5d, 0xb8, 0xa6),
    ctx_skills: rgb(0x5d, 0xb8, 0x72),
    ctx_messages: rgb(0xd4, 0xa0, 0x17),
    ctx_free: rgb(0x72, 0x70, 0x6a),
    ctx_reserve: rgb(0xe8, 0xb8, 0xb8),
    badge_pi: rgb(125, 211, 252),
    badge_claw: rgb(134, 239, 172),
    badge_claude: rgb(196, 181, 253),
    shimmer_base: rgb(0x72, 0x70, 0x6a),
};

/// Terminal theme (codewhale's `TERMINAL_UI_THEME` idea): surfaces use
/// `Color::Reset` so the host terminal scheme shows through; accents are
/// ANSI named colors so they follow the user's terminal palette too.
/// Selection is reverse-video-ish (white bg, black fg) so it stays visible
/// on any terminal background.
pub const TERMINAL_UI_THEME: UiTheme = UiTheme {
    name: "terminal",
    surface_bg: Color::Reset,
    raised_bg: Color::Reset,
    input_bg: Color::Reset,
    selection_bg: Color::White,
    on_selection: Color::Black,
    chip_bg: Color::White,
    text_faint: Color::DarkGray,
    text_dim: Color::DarkGray,
    text_muted: Color::Reset,
    text_soft: Color::Reset,
    text_body: Color::Reset,
    text_bright: Color::Reset,
    accent: Color::Yellow,
    accent_soft: Color::LightYellow,
    success: Color::Green,
    error: Color::Red,
    error_soft: Color::LightRed,
    rose: Color::LightRed,
    info: Color::Blue,
    tool_accent: Color::Green,
    tool_path: Color::Yellow,
    tool_command: Color::LightYellow,
    tool_dim: Color::DarkGray,
    diff_add_fg: Color::Green,
    diff_add_bg: Color::Reset,
    diff_del_fg: Color::Red,
    diff_del_bg: Color::Reset,
    diff_gutter: Color::DarkGray,
    cache_hit: Color::DarkGray,
    ctx_system: Color::Reset,
    ctx_context: Color::Green,
    ctx_tools: Color::Cyan,
    ctx_skills: Color::Magenta,
    ctx_messages: Color::Yellow,
    ctx_free: Color::DarkGray,
    ctx_reserve: Color::Red,
    badge_pi: Color::Cyan,
    badge_claw: Color::Green,
    badge_claude: Color::Magenta,
    shimmer_base: Color::DarkGray,
};

/// Stable identifiers for the selectable themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeId {
    Gray,
    TokyoNight,
    Dracula,
    CatppuccinMocha,
    GruvboxDark,
    Claude,
    Terminal,
}

impl ThemeId {
    /// Parse a settings string (`"tokyo-night"`, `"dracula"`, …).
    /// Case-insensitive; accepts short aliases (`"tokyo"`, `"mocha"`,
    /// `"gruvbox"`, `"term"`). Unknown names yield `None`.
    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "gray" | "grey" | "default" | "neutral" => Some(Self::Gray),
            "tokyo-night" | "tokyonight" | "tokyo" => Some(Self::TokyoNight),
            "dracula" | "drac" => Some(Self::Dracula),
            "catppuccin-mocha" | "catppuccin" | "mocha" => Some(Self::CatppuccinMocha),
            "gruvbox-dark" | "gruvbox" => Some(Self::GruvboxDark),
            "claude" => Some(Self::Claude),
            "terminal" | "term" | "transparent" | "inherit" | "follow-terminal" => {
                Some(Self::Terminal)
            }
            _ => None,
        }
    }

    /// Canonical settings string (round-trips through [`from_name`]).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gray => "gray",
            Self::TokyoNight => "tokyo-night",
            Self::Dracula => "dracula",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::GruvboxDark => "gruvbox-dark",
            Self::Claude => "claude",
            Self::Terminal => "terminal",
        }
    }

    /// Human-readable label for `/theme` listing.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Gray => "Gray",
            Self::TokyoNight => "Tokyo Night",
            Self::Dracula => "Dracula",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::Claude => "Claude",
            Self::Terminal => "Terminal",
        }
    }

    #[must_use]
    pub fn ui_theme(self) -> UiTheme {
        match self {
            Self::Gray => GRAY_UI_THEME,
            Self::TokyoNight => TOKYO_NIGHT_UI_THEME,
            Self::Dracula => DRACULA_UI_THEME,
            Self::CatppuccinMocha => CATPPUCCIN_MOCHA_UI_THEME,
            Self::GruvboxDark => GRUVBOX_DARK_UI_THEME,
            Self::Claude => CLAUDE_UI_THEME,
            Self::Terminal => TERMINAL_UI_THEME,
        }
    }
}

/// Themes shown by `/theme`, in display order.
pub const SELECTABLE_THEMES: &[ThemeId] = &[
    ThemeId::Gray,
    ThemeId::TokyoNight,
    ThemeId::Dracula,
    ThemeId::CatppuccinMocha,
    ThemeId::GruvboxDark,
    ThemeId::Claude,
    ThemeId::Terminal,
];

static ACTIVE: OnceLock<RwLock<UiTheme>> = OnceLock::new();

fn active_cell() -> &'static RwLock<UiTheme> {
    ACTIVE.get_or_init(|| RwLock::new(GRAY_UI_THEME))
}

/// The currently active theme (starts as [`GRAY_UI_THEME`]).
#[must_use]
pub fn theme() -> UiTheme {
    *active_cell().read().expect("theme lock")
}

/// Switch the active theme (takes effect on the next draw — no restart).
pub fn set_theme(id: ThemeId) {
    *active_cell().write().expect("theme lock") = id.ui_theme();
}

/// Pure name resolution: unknown/empty names fall back to gray.
#[must_use]
pub fn resolve_theme_id(name: Option<&str>) -> ThemeId {
    name.and_then(ThemeId::from_name).unwrap_or(ThemeId::Gray)
}

/// Resolve a theme name and activate it; unknown/empty names fall back to
/// gray. Returns the activated [`ThemeId`].
pub fn init_theme(name: Option<&str>) -> ThemeId {
    let id = resolve_theme_id(name);
    set_theme(id);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_round_trip() {
        for id in SELECTABLE_THEMES {
            assert_eq!(ThemeId::from_name(id.name()), Some(*id), "{}", id.name());
        }
        // Aliases.
        assert_eq!(ThemeId::from_name("TOKYO"), Some(ThemeId::TokyoNight));
        assert_eq!(ThemeId::from_name("mocha"), Some(ThemeId::CatppuccinMocha));
        assert_eq!(ThemeId::from_name("gruvbox"), Some(ThemeId::GruvboxDark));
        assert_eq!(ThemeId::from_name("default"), Some(ThemeId::Gray));
        assert_eq!(ThemeId::from_name("term"), Some(ThemeId::Terminal));
        // Unknown.
        assert_eq!(ThemeId::from_name("nope"), None);
        assert_eq!(ThemeId::from_name(""), None);
    }

    #[test]
    fn selectable_theme_names_unique() {
        let mut names: Vec<&str> = SELECTABLE_THEMES.iter().map(|t| t.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SELECTABLE_THEMES.len());
    }

    #[test]
    fn resolve_unknown_falls_back_to_gray() {
        // NOTE: no `set_theme`/`init_theme` calls in tests — they mutate the
        // process-global ACTIVE cell and would flake parallel tests that
        // assert exact rendered colors. Resolution is pure; activation is a
        // two-line wrapper.
        assert_eq!(resolve_theme_id(Some("bogus")), ThemeId::Gray);
        assert_eq!(resolve_theme_id(None), ThemeId::Gray);
        assert_eq!(resolve_theme_id(Some("dracula")), ThemeId::Dracula);
        assert_eq!(ThemeId::Gray.ui_theme(), GRAY_UI_THEME);
        assert_eq!(ThemeId::Dracula.ui_theme().name, "dracula");
    }
}
