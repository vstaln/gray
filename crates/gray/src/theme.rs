use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::RwLock;

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiTheme {
    pub window_bg: Color,
    pub surface_bg: Color,
    pub raised_bg: Color,
    pub input_bg: Color,
    pub on_selection: Color,
    pub chip_bg: Color,

    pub text_faint: Color,
    pub text_dim: Color,
    pub text_muted: Color,
    pub text_soft: Color,
    pub text_body: Color,
    pub text_bright: Color,

    pub accent: Color,
    pub accent_soft: Color,

    pub success: Color,
    pub error: Color,
    pub error_soft: Color,
    pub rose: Color,
    pub info: Color,

    pub tool_accent: Color,
    pub tool_path: Color,
    pub tool_command: Color,
    pub tool_dim: Color,

    pub diff_add_fg: Color,
    pub diff_add_bg: Color,
    pub diff_del_fg: Color,
    pub diff_del_bg: Color,
    pub diff_gutter: Color,

    pub cache_hit: Color,

    pub ctx_system: Color,
    pub ctx_context: Color,
    pub ctx_tools: Color,
    pub ctx_skills: Color,
    pub ctx_messages: Color,
    pub ctx_free: Color,

    pub shimmer_base: Color,

    pub bezel_hi: Color,
    pub bezel_mid: Color,
    pub bezel_lo: Color,
    pub hue_red: Color,
    pub hue_orange: Color,
    pub hue_yellow: Color,
    pub hue_green: Color,
    pub hue_cyan: Color,
    pub hue_blue: Color,
    pub hue_magenta: Color,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

pub const GRAY_UI_THEME: UiTheme = UiTheme {
    window_bg: Color::Reset,
    surface_bg: rgb(22, 22, 22),
    raised_bg: rgb(28, 28, 28),
    input_bg: rgb(32, 32, 32),
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
    shimmer_base: rgb(150, 148, 144),
    bezel_hi: rgb(106, 106, 106),
    bezel_mid: rgb(74, 74, 74),
    bezel_lo: rgb(46, 46, 46),
    hue_red: rgb(247, 118, 142),
    hue_orange: rgb(255, 158, 100),
    hue_yellow: rgb(224, 175, 104),
    hue_green: rgb(158, 206, 106),
    hue_cyan: rgb(125, 207, 255),
    hue_blue: rgb(122, 162, 247),
    hue_magenta: rgb(187, 154, 247),
};

pub const BUILTIN_THEME_NAME: &str = "gray";

static ACTIVE: RwLock<UiTheme> = RwLock::new(GRAY_UI_THEME);
static ACTIVE_NAME: RwLock<Option<String>> = RwLock::new(None);

#[must_use]
pub fn theme() -> UiTheme {
    ACTIVE.read().map(|t| *t).unwrap_or(GRAY_UI_THEME)
}

#[must_use]
pub fn active_theme_name() -> Option<String> {
    ACTIVE_NAME.read().ok().and_then(|n| n.clone())
}

pub fn set_theme(theme: UiTheme, name: Option<&str>) {
    if let Ok(mut t) = ACTIVE.write() {
        *t = theme;
    }
    if let Ok(mut n) = ACTIVE_NAME.write() {
        *n = name.map(str::to_string);
    }
    gray_markdown::set_palette(Some(markdown_palette(&theme)));
}

pub const ROLE_NAMES: &[&str] = &[
    "window_bg",
    "surface_bg",
    "raised_bg",
    "input_bg",
    "on_selection",
    "chip_bg",
    "text_faint",
    "text_dim",
    "text_muted",
    "text_soft",
    "text_body",
    "text_bright",
    "accent",
    "accent_soft",
    "success",
    "error",
    "error_soft",
    "rose",
    "info",
    "tool_accent",
    "tool_path",
    "tool_command",
    "tool_dim",
    "diff_add_fg",
    "diff_add_bg",
    "diff_del_fg",
    "diff_del_bg",
    "diff_gutter",
    "cache_hit",
    "ctx_system",
    "ctx_context",
    "ctx_tools",
    "ctx_skills",
    "ctx_messages",
    "ctx_free",
    "shimmer_base",
    "bezel_hi",
    "bezel_mid",
    "bezel_lo",
    "hue_red",
    "hue_orange",
    "hue_yellow",
    "hue_green",
    "hue_cyan",
    "hue_blue",
    "hue_magenta",
];

impl UiTheme {
    pub fn role_mut(&mut self, name: &str) -> Option<&mut Color> {
        Some(match name {
            "window_bg" => &mut self.window_bg,
            "surface_bg" => &mut self.surface_bg,
            "raised_bg" => &mut self.raised_bg,
            "input_bg" => &mut self.input_bg,
            "on_selection" => &mut self.on_selection,
            "chip_bg" => &mut self.chip_bg,
            "text_faint" => &mut self.text_faint,
            "text_dim" => &mut self.text_dim,
            "text_muted" => &mut self.text_muted,
            "text_soft" => &mut self.text_soft,
            "text_body" => &mut self.text_body,
            "text_bright" => &mut self.text_bright,
            "accent" => &mut self.accent,
            "accent_soft" => &mut self.accent_soft,
            "success" => &mut self.success,
            "error" => &mut self.error,
            "error_soft" => &mut self.error_soft,
            "rose" => &mut self.rose,
            "info" => &mut self.info,
            "tool_accent" => &mut self.tool_accent,
            "tool_path" => &mut self.tool_path,
            "tool_command" => &mut self.tool_command,
            "tool_dim" => &mut self.tool_dim,
            "diff_add_fg" => &mut self.diff_add_fg,
            "diff_add_bg" => &mut self.diff_add_bg,
            "diff_del_fg" => &mut self.diff_del_fg,
            "diff_del_bg" => &mut self.diff_del_bg,
            "diff_gutter" => &mut self.diff_gutter,
            "cache_hit" => &mut self.cache_hit,
            "ctx_system" => &mut self.ctx_system,
            "ctx_context" => &mut self.ctx_context,
            "ctx_tools" => &mut self.ctx_tools,
            "ctx_skills" => &mut self.ctx_skills,
            "ctx_messages" => &mut self.ctx_messages,
            "ctx_free" => &mut self.ctx_free,
            "shimmer_base" => &mut self.shimmer_base,
            "bezel_hi" => &mut self.bezel_hi,
            "bezel_mid" => &mut self.bezel_mid,
            "bezel_lo" => &mut self.bezel_lo,
            "hue_red" => &mut self.hue_red,
            "hue_orange" => &mut self.hue_orange,
            "hue_yellow" => &mut self.hue_yellow,
            "hue_green" => &mut self.hue_green,
            "hue_cyan" => &mut self.hue_cyan,
            "hue_blue" => &mut self.hue_blue,
            "hue_magenta" => &mut self.hue_magenta,
            _ => return None,
        })
    }

    #[must_use]
    pub fn role(&self, name: &str) -> Option<Color> {
        let mut copy = *self;
        copy.role_mut(name).map(|c| *c)
    }

    #[must_use]
    pub fn window(&self) -> Color {
        self.window_bg
    }
}

pub fn parse_color(
    value: &serde_json::Value,
    vars: &serde_json::Map<String, serde_json::Value>,
) -> Result<Color, String> {
    parse_color_depth(value, vars, 0)
}

fn parse_color_depth(
    value: &serde_json::Value,
    vars: &serde_json::Map<String, serde_json::Value>,
    depth: u8,
) -> Result<Color, String> {
    match value {
        serde_json::Value::Number(n) => n
            .as_u64()
            .and_then(|i| u8::try_from(i).ok())
            .map(Color::Indexed)
            .ok_or_else(|| format!("{n} is not a 0-255 color index")),
        serde_json::Value::String(s) => {
            let s = s.trim();
            if let Some(v) = vars.get(s) {
                if depth >= 8 {
                    return Err(format!("var `{s}` nests too deep (cycle?)"));
                }
                return parse_color_depth(v, vars, depth + 1);
            }
            match s.to_ascii_lowercase().as_str() {
                "" | "default" | "terminal" | "none" => return Ok(Color::Reset),
                _ => {}
            }
            if let Some(hex) = s.strip_prefix('#')
                && hex.len() == 3
                && hex.chars().all(|c| c.is_ascii_hexdigit())
            {
                let d = |i: usize| u8::from_str_radix(&hex[i..=i], 16).map(|v| v * 17);
                if let (Ok(r), Ok(g), Ok(b)) = (d(0), d(1), d(2)) {
                    return Ok(Color::Rgb(r, g, b));
                }
            }
            Color::from_str(s).map_err(|_| format!("`{s}` is not a color"))
        }
        other => Err(format!("{other} is not a color")),
    }
}

#[must_use]
pub fn swatch_spans(theme: &UiTheme) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    [
        (theme.surface_bg, theme.raised_bg),
        (theme.text_dim, theme.text_body),
        (theme.hue_red, theme.error_soft),
        (theme.hue_orange, theme.hue_yellow),
        (theme.hue_green, theme.success),
        (theme.hue_cyan, theme.hue_blue),
        (theme.hue_magenta, theme.rose),
        (theme.accent, theme.accent_soft),
    ]
    .into_iter()
    .map(|(top, bottom)| Span::styled("▀▀", Style::default().fg(top).bg(bottom)))
    .collect()
}

#[must_use]
pub fn color_to_string(c: Color) -> serde_json::Value {
    use serde_json::Value;
    match c {
        Color::Rgb(r, g, b) => Value::String(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Indexed(i) => Value::from(i),
        Color::Reset => Value::String("default".into()),
        named => Value::String(
            match named {
                Color::Black => "black",
                Color::Red => "red",
                Color::Green => "green",
                Color::Yellow => "yellow",
                Color::Blue => "blue",
                Color::Magenta => "magenta",
                Color::Cyan => "cyan",
                Color::Gray => "gray",
                Color::DarkGray => "darkgray",
                Color::LightRed => "lightred",
                Color::LightGreen => "lightgreen",
                Color::LightYellow => "lightyellow",
                Color::LightBlue => "lightblue",
                Color::LightMagenta => "lightmagenta",
                Color::LightCyan => "lightcyan",
                _ => "white",
            }
            .into(),
        ),
    }
}

pub(crate) fn derive_window(surface: Color) -> Color {
    let Color::Rgb(r, g, b) = surface else {
        return surface;
    };
    let luma = 0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b);
    let dark = luma < 128.0;
    let shift = |c: u8| {
        let c = f64::from(c);
        let moved = if dark {
            c * 0.55
        } else {
            c + (255.0 - c) * 0.55
        };
        moved.round().clamp(0.0, 255.0) as u8
    };
    let (wr, wg, wb) = (shift(r), shift(g), shift(b));

    let gap = [(r, wr), (g, wg), (b, wb)]
        .iter()
        .map(|(a, w)| a.abs_diff(*w))
        .max()
        .unwrap_or(0);
    if gap >= MIN_WINDOW_STEP {
        return Color::Rgb(wr, wg, wb);
    }
    let step = |c: u8| {
        if dark {
            c.saturating_sub(MIN_WINDOW_STEP)
        } else {
            c.saturating_add(MIN_WINDOW_STEP)
        }
    };
    Color::Rgb(step(r), step(g), step(b))
}

const MIN_WINDOW_STEP: u8 = 12;

pub fn parse_theme(text: &str) -> Result<(UiTheme, Vec<String>), String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = root.as_object().ok_or("a theme file is a JSON object")?;
    let empty = serde_json::Map::new();
    let vars = obj
        .get("vars")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty);
    let colors = match obj.get("colors") {
        Some(serde_json::Value::Object(c)) => c,
        Some(_) => return Err("`colors` must be an object".into()),
        None => return Err("missing `colors` object".into()),
    };
    let mut theme = GRAY_UI_THEME;
    let mut warnings = Vec::new();
    for (role, value) in colors {
        if role == "window_bg"
            && let Some(v) = value.as_str()
            && matches!(v.trim(), "reset" | "none" | "transparent")
        {
            theme.window_bg = Color::Reset;
            continue;
        }
        let Some(slot) = theme.role_mut(role) else {
            warnings.push(format!("unknown role `{role}`"));
            continue;
        };
        match parse_color(value, vars) {
            Ok(c) => *slot = c,
            Err(e) => warnings.push(format!("{role}: {e}")),
        }
    }
    if !colors.contains_key("window_bg") {
        theme.window_bg = derive_window(theme.surface_bg);
    }
    let hue_fallbacks = [
        ("hue_red", theme.error),
        ("hue_orange", theme.tool_path),
        ("hue_yellow", theme.tool_command),
        ("hue_green", theme.success),
        ("hue_cyan", theme.ctx_tools),
        ("hue_blue", theme.info),
        ("hue_magenta", theme.rose),
    ];
    for (role, derived) in hue_fallbacks {
        if !colors.contains_key(role)
            && let Some(slot) = theme.role_mut(role)
        {
            *slot = derived;
        }
    }
    Ok((theme, warnings))
}

#[must_use]
pub fn theme_to_json(theme: &UiTheme) -> String {
    let mut colors = serde_json::Map::new();
    for name in ROLE_NAMES {
        if let Some(c) = theme.role(name) {
            colors.insert((*name).to_string(), color_to_string(c));
        }
    }
    let doc = serde_json::json!({ "colors": colors });
    serde_json::to_string_pretty(&doc).expect("theme JSON serializes") + "\n"
}

#[must_use]
pub fn markdown_palette(theme: &UiTheme) -> gray_markdown::Palette {
    let fg = |c: Color| gray_markdown::ratatui_color_to_rgb(c).unwrap_or([225, 225, 225]);
    gray_markdown::Palette {
        text: fg(theme.text_body),
        strong: fg(theme.text_bright),
        heading: fg(theme.accent),
        heading_minor: fg(theme.accent_soft),
        dim: fg(theme.text_dim),
        faint: fg(theme.text_faint),
        bullet: fg(theme.accent),
        inline_code: fg(theme.hue_cyan),
        link: fg(theme.hue_blue),
        quote: fg(theme.text_muted),
        success: fg(theme.success),
        bg: gray_markdown::ratatui_color_to_rgb(theme.surface_bg).unwrap_or([22, 22, 22]),
        comment: fg(theme.text_dim),
        keyword: fg(theme.hue_magenta),
        string: fg(theme.hue_green),
        function: fg(theme.hue_blue),
        type_: fg(theme.hue_yellow),
        number: fg(theme.hue_orange),
        constant: fg(theme.hue_cyan),
        operator: fg(theme.text_muted),
        tag: fg(theme.hue_red),
        attribute: fg(theme.hue_yellow),
        inserted: fg(theme.diff_add_fg),
        deleted: fg(theme.diff_del_fg),
        invalid: fg(theme.error),
    }
}

pub fn themes_dir() -> anyhow::Result<PathBuf> {
    Ok(crate::setup::gray_home()?.join("themes"))
}

#[must_use]
pub fn valid_theme_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[must_use]
pub fn is_builtin_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(BUILTIN_THEME_NAME) || name.eq_ignore_ascii_case("default")
}

#[must_use]
pub fn list_themes_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "json"))
                .then(|| p.file_stem()?.to_str().map(str::to_string))
                .flatten()
        })
        .filter(|n| valid_theme_name(n))
        .collect();
    names.sort();
    names
}

pub fn load_theme_in(dir: &Path, name: &str) -> Result<(UiTheme, Vec<String>), String> {
    if !valid_theme_name(name) {
        return Err(format!("`{name}` is not a valid theme name"));
    }
    let path = dir.join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_theme(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn init_from_saved(saved: Option<&str>) {
    gray_markdown::set_palette(Some(markdown_palette(&theme())));
    let env = std::env::var("GRAY_THEME").ok();
    let Some(name) = env.as_deref().or(saved).map(str::trim) else {
        return;
    };
    if name.is_empty() || is_builtin_name(name) {
        return;
    }
    let Ok(dir) = themes_dir() else { return };
    match load_theme_in(&dir, name) {
        Ok((t, warnings)) => {
            for w in warnings {
                log::warn!(target: "gray_theme", "theme {name}: {w}");
            }
            set_theme(t, Some(name));
        }
        Err(e) => log::warn!(target: "gray_theme", "theme {name} not applied: {e}"),
    }
}
