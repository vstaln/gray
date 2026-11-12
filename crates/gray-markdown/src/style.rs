use anstyle::{Effects, Style};

use crate::colors::adapt_style;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Palette {
    pub text: [u8; 3],
    pub strong: [u8; 3],
    pub heading: [u8; 3],
    pub heading_minor: [u8; 3],
    pub dim: [u8; 3],
    pub faint: [u8; 3],
    pub bullet: [u8; 3],
    pub inline_code: [u8; 3],
    pub link: [u8; 3],
    pub quote: [u8; 3],
    pub success: [u8; 3],
    pub bg: [u8; 3],
    pub comment: [u8; 3],
    pub keyword: [u8; 3],
    pub string: [u8; 3],
    pub function: [u8; 3],
    pub type_: [u8; 3],
    pub number: [u8; 3],
    pub constant: [u8; 3],
    pub operator: [u8; 3],
    pub tag: [u8; 3],
    pub attribute: [u8; 3],
    pub inserted: [u8; 3],
    pub deleted: [u8; 3],
    pub invalid: [u8; 3],
}

static PALETTE: std::sync::RwLock<Option<Palette>> = std::sync::RwLock::new(None);

pub fn set_palette(p: Option<Palette>) {
    if let Ok(mut slot) = PALETTE.write() {
        *slot = p;
    }
}

pub(crate) fn palette() -> Option<Palette> {
    PALETTE.read().ok().and_then(|p| *p)
}

#[must_use]
pub fn ratatui_color_to_rgb(c: ratatui::style::Color) -> Option<[u8; 3]> {
    use ratatui::style::Color;
    match c {
        Color::Reset => None,
        Color::Rgb(r, g, b) => Some([r, g, b]),
        Color::Indexed(i) => {
            let rgb =
                anstyle_lossy::xterm_to_rgb(anstyle::Ansi256Color(i), anstyle_lossy::palette::VGA);
            Some([rgb.0, rgb.1, rgb.2])
        }
        named => {
            use anstyle::AnsiColor as A;
            let ansi = match named {
                Color::Black => A::Black,
                Color::Red => A::Red,
                Color::Green => A::Green,
                Color::Yellow => A::Yellow,
                Color::Blue => A::Blue,
                Color::Magenta => A::Magenta,
                Color::Cyan => A::Cyan,
                Color::Gray => A::White,
                Color::DarkGray => A::BrightBlack,
                Color::LightRed => A::BrightRed,
                Color::LightGreen => A::BrightGreen,
                Color::LightYellow => A::BrightYellow,
                Color::LightBlue => A::BrightBlue,
                Color::LightMagenta => A::BrightMagenta,
                Color::LightCyan => A::BrightCyan,
                Color::White => A::BrightWhite,
                _ => return None,
            };
            let rgb = anstyle_lossy::ansi_to_rgb(ansi, anstyle_lossy::palette::VGA);
            Some([rgb.0, rgb.1, rgb.2])
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TableBorders {
    chars: [char; 11],
}

impl TableBorders {
    const H: usize = 0;
    const V: usize = 1;
    const TL: usize = 2;
    const TR: usize = 3;
    const BL: usize = 4;
    const BR: usize = 5;
    const T_T: usize = 6;
    const T_B: usize = 7;
    const T_L: usize = 8;
    const T_R: usize = 9;
    const X: usize = 10;

    pub const BOX: Self = Self {
        chars: ['─', '│', '┌', '┐', '└', '┘', '┬', '┴', '├', '┤', '┼'],
    };

    pub const fn h(&self) -> char {
        self.chars[Self::H]
    }
    pub const fn v(&self) -> char {
        self.chars[Self::V]
    }
    pub const fn c_tl(&self) -> char {
        self.chars[Self::TL]
    }
    pub const fn c_tr(&self) -> char {
        self.chars[Self::TR]
    }
    pub const fn c_bl(&self) -> char {
        self.chars[Self::BL]
    }
    pub const fn c_br(&self) -> char {
        self.chars[Self::BR]
    }
    pub const fn t_t(&self) -> char {
        self.chars[Self::T_T]
    }
    pub const fn t_b(&self) -> char {
        self.chars[Self::T_B]
    }
    pub const fn t_l(&self) -> char {
        self.chars[Self::T_L]
    }
    pub const fn t_r(&self) -> char {
        self.chars[Self::T_R]
    }
    pub const fn x(&self) -> char {
        self.chars[Self::X]
    }
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct MarkdownStyle {
    pub heading_inner: [Style; 6],
    pub heading_outer: [Style; 6],
    pub strong_inner: Style,
    pub strong_outer: Style,
    pub emphasis_inner: Style,
    pub emphasis_outer: Style,
    pub strikethrough_inner: Style,
    pub strikethrough_outer: Style,
    pub inline_code_inner: Style,
    pub inline_code_outer: Style,
    pub blockquote_outer: Style,
    pub task_checked: Style,
    pub task_unchecked: Style,
    pub list_item: Style,
    pub rule: Style,
    pub link_outer: Style,
    pub link_text: Style,
    pub link_url: Style,
    pub link_title: Style,
    pub code_outer: Style,
    pub code_language: Style,
    pub code_untagged: Style,
    pub code_background: Style,
    pub table_outer: Style,

    pub text: Style,

    pub math: Style,
}

impl MarkdownStyle {
    pub fn adapt(self) -> Self {
        Self {
            heading_inner: [
                adapt_style(self.heading_inner[0]),
                adapt_style(self.heading_inner[1]),
                adapt_style(self.heading_inner[2]),
                adapt_style(self.heading_inner[3]),
                adapt_style(self.heading_inner[4]),
                adapt_style(self.heading_inner[5]),
            ],
            heading_outer: [
                adapt_style(self.heading_outer[0]),
                adapt_style(self.heading_outer[1]),
                adapt_style(self.heading_outer[2]),
                adapt_style(self.heading_outer[3]),
                adapt_style(self.heading_outer[4]),
                adapt_style(self.heading_outer[5]),
            ],
            strong_inner: adapt_style(self.strong_inner),
            strong_outer: adapt_style(self.strong_outer),
            emphasis_inner: adapt_style(self.emphasis_inner),
            emphasis_outer: adapt_style(self.emphasis_outer),
            strikethrough_inner: adapt_style(self.strikethrough_inner),
            strikethrough_outer: adapt_style(self.strikethrough_outer),
            inline_code_inner: adapt_style(self.inline_code_inner),
            inline_code_outer: adapt_style(self.inline_code_outer),
            blockquote_outer: adapt_style(self.blockquote_outer),
            task_checked: adapt_style(self.task_checked),
            task_unchecked: adapt_style(self.task_unchecked),
            list_item: adapt_style(self.list_item),
            rule: adapt_style(self.rule),
            link_outer: adapt_style(self.link_outer),
            link_text: adapt_style(self.link_text),
            link_url: adapt_style(self.link_url),
            link_title: adapt_style(self.link_title),
            code_outer: adapt_style(self.code_outer),
            code_language: adapt_style(self.code_language),
            code_untagged: adapt_style(self.code_untagged),
            code_background: adapt_style(self.code_background),
            table_outer: adapt_style(self.table_outer),
            text: adapt_style(self.text),
            math: adapt_style(self.math),
        }
    }
}

pub(crate) fn all_hidden(styles: impl IntoIterator<Item = Option<Style>>) -> bool {
    let mut has_any = false;
    let mut all_are_hidden = true;

    for style in styles {
        has_any = true;
        match style {
            Some(s) if s.get_effects().contains(Effects::HIDDEN) => {}
            _ => {
                all_are_hidden = false;
            }
        }
    }

    has_any && all_are_hidden
}

pub(crate) fn merge_styles(styles: impl IntoIterator<Item = Option<Style>>) -> Style {
    let mut out = Style::new();
    let mut prev = Style::new();
    for style in styles {
        if out.get_effects().contains(Effects::HIDDEN) {
            out = prev;
        } else {
            prev = out;
        }
        let Some(style) = style else {
            continue;
        };
        if !style.get_effects().is_plain() {
            out = out.effects(out.get_effects() | style.get_effects());
        }
        if style.get_effects().contains(Effects::DIMMED) {
            out = out.effects(out.get_effects().remove(Effects::BOLD));
        }
        if style.get_effects().contains(Effects::BOLD) {
            out = out.effects(out.get_effects().remove(Effects::DIMMED));
        }
        if let Some(color) = style.get_fg_color() {
            out = out.fg_color(Some(color));
        }
        if let Some(color) = style.get_bg_color() {
            out = out.bg_color(Some(color));
        }
        if let Some(color) = style.get_underline_color() {
            out = out.underline_color(Some(color));
        }
    }
    out.effects(out.get_effects().remove(Effects::HIDDEN))
}
