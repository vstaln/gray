//! Gray color palette (single theme).
//!
//! One [`UiTheme`] struct of semantic roles — every TUI call site reads
//! `theme().role()` instead of a hardcoded `Color::Rgb`. There is exactly
//! one palette (today's gray); theme switching was removed, so `theme()`
//! always returns [`GRAY_UI_THEME`] with identical colors.

use ratatui::style::Color;

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
/// The active palette — the single gray theme (no switching).
#[must_use]
pub fn theme() -> UiTheme {
    GRAY_UI_THEME
}
