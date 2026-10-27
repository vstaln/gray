use std::io::Cursor;
use std::path::Path;

use syntect::{
    easy::HighlightLines,
    highlighting::{Theme as SyntectTheme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};

fn syntax_set_extra_newlines() -> &'static SyntaxSet {
    static SET: std::sync::OnceLock<SyntaxSet> = std::sync::OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn syntax_set_defaults() -> &'static SyntaxSet {
    static SET: std::sync::OnceLock<SyntaxSet> = std::sync::OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

pub struct Syntect {
    pub theme: SyntectTheme,

    pub syntax_set: &'static SyntaxSet,
    /// Unique per instance; keys the streaming-block cache (an address can be reused once freed).
    id: u64,
}

fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Syntect {
    pub fn new(theme_bytes: &[u8]) -> Self {
        let mut cursor = Cursor::new(theme_bytes);
        let theme =
            ThemeSet::load_from_reader(&mut cursor).unwrap_or_else(|_| Self::default().theme);

        let syntax_set = syntax_set_extra_newlines();
        Self {
            theme,
            syntax_set,
            id: next_id(),
        }
    }
}

impl Default for Syntect {
    fn default() -> Self {
        let ts = ThemeSet::load_defaults();
        let theme = ts
            .themes
            .get("base16-ocean.dark")
            .cloned()
            .or_else(|| ts.themes.values().next().cloned())
            .unwrap_or_default();
        let syntax_set = syntax_set_defaults();
        Self {
            theme,
            syntax_set,
            id: next_id(),
        }
    }
}

pub fn get_syntect() -> &'static Syntect {
    if let Some(p) = crate::style::palette() {
        return themed_syntect(p);
    }
    static SYNTECT: std::sync::OnceLock<Syntect> = std::sync::OnceLock::new();
    SYNTECT.get_or_init(|| Syntect::new(include_bytes!("../assets/tokyo-night.tmTheme")))
}

fn themed_syntect(p: crate::style::Palette) -> &'static Syntect {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static THEMED: OnceLock<Mutex<HashMap<crate::style::Palette, &'static Syntect>>> =
        OnceLock::new();
    let map = THEMED.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(p).or_insert_with(|| {
        Box::leak(Box::new(Syntect {
            theme: palette_theme(&p),
            syntax_set: syntax_set_extra_newlines(),
            id: next_id(),
        }))
    })
}

fn palette_theme(p: &crate::style::Palette) -> SyntectTheme {
    use std::str::FromStr;
    use syntect::highlighting::{
        Color as SynColor, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem,
        ThemeSettings,
    };
    let c = |v: [u8; 3]| SynColor {
        r: v[0],
        g: v[1],
        b: v[2],
        a: 255,
    };
    let item = |scope: &str, fg: Option<SynColor>, font: FontStyle| ThemeItem {
        scope: ScopeSelectors::from_str(scope).unwrap_or_default(),
        style: StyleModifier {
            foreground: fg,
            background: None,
            font_style: (!font.is_empty()).then_some(font),
        },
    };
    Theme {
        name: Some("gray-palette".to_string()),
        settings: ThemeSettings {
            foreground: Some(c(p.text)),
            background: Some(c(p.bg)),
            caret: Some(c(p.text)),
            selection: Some(c(p.faint)),
            ..Default::default()
        },
        scopes: vec![
            item(
                "comment, punctuation.definition.comment",
                Some(c(p.comment)),
                FontStyle::ITALIC,
            ),
            item(
                "keyword, keyword.control, storage.modifier, storage.type.function",
                Some(c(p.keyword)),
                FontStyle::empty(),
            ),
            item("variable.language", Some(c(p.keyword)), FontStyle::ITALIC),
            item(
                "string, punctuation.definition.string",
                Some(c(p.string)),
                FontStyle::empty(),
            ),
            item(
                "string.regexp, constant.character.escape, support.constant, \
                 constant.other, variable.other.constant",
                Some(c(p.constant)),
                FontStyle::empty(),
            ),
            item(
                "constant.numeric, constant.language",
                Some(c(p.number)),
                FontStyle::empty(),
            ),
            item(
                "entity.name.function, support.function, variable.function, \
                 meta.function-call.generic",
                Some(c(p.function)),
                FontStyle::empty(),
            ),
            item(
                "entity.name.type, entity.name.class, entity.name.namespace, \
                 support.type, support.class, storage.type, entity.other.inherited-class",
                Some(c(p.type_)),
                FontStyle::empty(),
            ),
            item(
                "keyword.operator, punctuation",
                Some(c(p.operator)),
                FontStyle::empty(),
            ),
            item("entity.name.tag", Some(c(p.tag)), FontStyle::empty()),
            item(
                "entity.other.attribute-name",
                Some(c(p.attribute)),
                FontStyle::empty(),
            ),
            item("variable.parameter", Some(c(p.text)), FontStyle::ITALIC),
            item("markup.heading", Some(c(p.heading)), FontStyle::BOLD),
            item("markup.bold", None, FontStyle::BOLD),
            item("markup.italic", None, FontStyle::ITALIC),
            item("markup.inserted", Some(c(p.inserted)), FontStyle::empty()),
            item("markup.deleted", Some(c(p.deleted)), FontStyle::empty()),
            item("invalid", Some(c(p.invalid)), FontStyle::empty()),
        ],
        ..Default::default()
    }
}

pub fn syntect_to_ratatui_fg(style: syntect::highlighting::Style) -> ratatui::style::Style {
    use ratatui::style::{Modifier, Style};
    let rgb = anstyle::RgbColor(style.foreground.r, style.foreground.g, style.foreground.b);
    let mut s = match crate::colors::adapt_color(anstyle::Color::Rgb(rgb)) {
        Some(adapted) => Style::default().fg(crate::colors::anstyle_to_ratatui_color(adapted)),
        None => Style::default(),
    };
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::BOLD)
    {
        s = s.add_modifier(Modifier::BOLD);
    }
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::ITALIC)
    {
        s = s.add_modifier(Modifier::ITALIC);
    }
    s
}

impl Syntect {
    pub fn find_syntax_by_file_path(&self, file_path: &Path) -> Option<&SyntaxReference> {
        let ext = file_path.extension()?.to_str()?;
        self.syntax_set.find_syntax_by_extension(ext)
    }

    pub fn find_syntax_by_token(&self, token: &str) -> Option<&SyntaxReference> {
        self.syntax_set.find_syntax_by_token(token)
    }

    pub fn highlight_lines_by_file_path(&self, file_path: &Path) -> Option<HighlightLines<'_>> {
        Some(HighlightLines::new(
            self.find_syntax_by_file_path(file_path)?,
            &self.theme,
        ))
    }

    pub fn highlight_lines_for_token(&self, token: &str) -> Option<HighlightLines<'_>> {
        Some(HighlightLines::new(
            self.find_syntax_by_token(token)?,
            &self.theme,
        ))
    }

    pub fn highlight_lines_for_fence_info(&self, fence_info: &str) -> Option<HighlightLines<'_>> {
        Some(HighlightLines::new(
            self.find_syntax_for_fence_info(fence_info)?,
            &self.theme,
        ))
    }

    pub(crate) fn find_syntax_for_fence_info(&self, fence_info: &str) -> Option<&SyntaxReference> {
        let token = fence_info.split_whitespace().next().unwrap_or("").trim();
        if token.is_empty() {
            return None;
        }
        if let Some((_, _, path)) = parse_line_citation_fence_info(token)
            && let Some(s) = self.find_syntax_by_file_path(Path::new(path))
        {
            return Some(s);
        }
        self.find_syntax_by_token(token)
            .or_else(|| self.find_syntax_by_token(&token.to_ascii_lowercase()))
            .or_else(|| self.syntax_set.find_syntax_by_extension(token))
            .or_else(|| {
                self.syntax_set
                    .find_syntax_by_extension(&token.to_ascii_lowercase())
            })
    }
}

fn parse_line_citation_fence_info(info: &str) -> Option<(&str, &str, &str)> {
    let mut it = info.splitn(3, ':');
    let start = it.next()?;
    let end = it.next()?;
    let path = it.next()?;
    if start.is_empty() || !start.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if end.is_empty() || !end.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if path.is_empty() {
        return None;
    }
    Some((start, end, path))
}

pub(crate) fn syntax_highlight_raw(
    syntect: Option<&Syntect>,
    fence_info: &str,
    text: &str,
) -> Option<Vec<Vec<(syntect::highlighting::Style, String)>>> {
    use syntect::util::LinesWithEndings;

    let syn = syntect?;
    let key = syn.id;
    let cached = OPEN_BLOCK
        .with(|c| c.borrow_mut().take())
        .filter(|b| b.syntect == key && b.fence == fence_info && text.starts_with(&b.done));
    let (mut hl, mut lines, mut done) = match cached {
        Some(b) => (
            HighlightLines::from_state(&syn.theme, b.highlight, b.parse),
            b.lines,
            b.done,
        ),
        None => (
            syn.highlight_lines_for_fence_info(fence_info)?,
            Vec::new(),
            String::new(),
        ),
    };
    let highlight = |hl: &mut HighlightLines, line: &str| {
        hl.highlight_line(line, syn.syntax_set).ok().map(|h| {
            h.into_iter()
                .map(|(s, t)| (s, t.to_string()))
                .collect::<Vec<_>>()
        })
    };
    let rest = &text[done.len()..];
    let mut partial = None;
    for line in LinesWithEndings::from(rest) {
        if line.ends_with('\n') {
            lines.push(highlight(&mut hl, line)?);
            done.push_str(line);
        } else {
            partial = Some(line);
        }
    }
    let (hs, ps) = hl.state();
    let mut out = lines.clone();
    if let Some(line) = partial {
        let mut tail = HighlightLines::from_state(&syn.theme, hs.clone(), ps.clone());
        out.push(highlight(&mut tail, line)?);
    }
    OPEN_BLOCK.with(|c| {
        *c.borrow_mut() = Some(OpenBlock {
            syntect: key,
            fence: fence_info.to_string(),
            done,
            lines,
            highlight: hs,
            parse: ps,
        })
    });
    Some(out)
}

struct OpenBlock {
    syntect: u64,
    fence: String,
    done: String,
    lines: Vec<Vec<(syntect::highlighting::Style, String)>>,
    highlight: syntect::highlighting::HighlightState,
    parse: syntect::parsing::ParseState,
}

thread_local! {
    static OPEN_BLOCK: std::cell::RefCell<Option<OpenBlock>> = const { std::cell::RefCell::new(None) };
}
