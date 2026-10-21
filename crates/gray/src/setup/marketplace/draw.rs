use gray_pkg::index::Entry;
use gray_pkg::staged::InstallStage;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthChar;

use super::ConsentView;
use super::model::{MarketModel, MarketPhase};
use crate::theme::UiTheme;
use crate::tui::visible_width;

pub(super) fn safe_text(raw: &str) -> String {
    let plain = crate::composer::input::strip_escape_sequences(raw);
    plain
        .lines()
        .map(|line| {
            line.split_whitespace()
                .map(|word| {
                    let word: String = word.chars().filter(|c| !c.is_control()).collect();
                    if word.contains("://") {
                        gray_pkg::fetch::redact(&word)
                    } else {
                        word
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn one_line(raw: &str) -> String {
    safe_text(raw).replace('\n', " ")
}

fn fg(color: Color) -> Style {
    Style::default().fg(color)
}

fn bold(color: Color) -> Style {
    fg(color).add_modifier(Modifier::BOLD)
}

#[derive(Clone, Copy)]
enum Trust {
    Official,
    Verified,
    Unverified,
}

impl Trust {
    fn of(entry: &Entry) -> Self {
        if entry.official {
            Self::Official
        } else if entry.verified {
            Self::Verified
        } else {
            Self::Unverified
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Verified => "verified community",
            Self::Unverified => "unverified community",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Official => "●",
            Self::Verified => "◐",
            Self::Unverified => "○",
        }
    }

    fn color(self, theme: &UiTheme) -> Color {
        match self {
            Self::Official => theme.accent_soft,
            Self::Verified => theme.info,
            Self::Unverified => theme.text_dim,
        }
    }

    fn spans(self, theme: &UiTheme) -> Vec<Span<'static>> {
        vec![
            Span::styled(self.icon(), fg(self.color(theme))),
            Span::styled(format!(" {}", self.label()), fg(theme.text_body)),
        ]
    }
}

pub fn status_text(model: &MarketModel) -> Option<&str> {
    match model.phase {
        MarketPhase::Loading => Some("Loading marketplace…"),
        MarketPhase::LoadError => model.message.as_deref(),
        MarketPhase::Browsing if model.entries.is_empty() => Some("No plugins available"),
        MarketPhase::Browsing if model.visible_names().is_empty() => Some("No matching plugins"),
        _ => None,
    }
}

const SIDE_BY_SIDE_MIN_WIDTH: u16 = 90;
const PANE_GAP: u16 = 3;
const KEY_WIDTH: usize = 10;
const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// Splits the tab area into a body and a footer row, leaving a blank row
/// under the tab bar and above the footer when there is room for them.
fn frame_areas(area: Rect) -> (Rect, Rect) {
    let top = u16::from(area.height >= 10);
    let footer = Rect::new(
        area.x,
        area.bottom().saturating_sub(1),
        area.width,
        area.height.min(1),
    );
    let body = Rect::new(
        area.x,
        area.y.saturating_add(top),
        area.width,
        area.height.saturating_sub(top + 2),
    );
    (body, footer)
}

pub(super) fn browsing_panes(area: Rect) -> (Rect, Rect) {
    let (body, _) = frame_areas(area);
    let skip = (1 + u16::from(body.height >= 8)).min(body.height);
    let content = Rect::new(
        body.x,
        body.y + skip,
        body.width,
        body.height.saturating_sub(skip),
    );
    if area.width >= SIDE_BY_SIDE_MIN_WIDTH {
        // Keep the trust icon, version, and installed mark intact at the split.
        let list_width = ((u32::from(content.width) * 42 / 100) as u16)
            .max(42)
            .min(content.width);
        (
            Rect::new(content.x, content.y, list_width, content.height),
            Rect::new(
                content.x + list_width + PANE_GAP,
                content.y,
                content.width.saturating_sub(list_width + PANE_GAP),
                content.height,
            ),
        )
    } else {
        let list_height = (content.height / 3).clamp(1, 6).min(content.height);
        let gap = content.height.saturating_sub(list_height).min(1);
        (
            Rect::new(content.x, content.y, content.width, list_height),
            Rect::new(
                content.x,
                content.y + list_height + gap,
                content.width,
                content.height.saturating_sub(list_height + gap),
            ),
        )
    }
}

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &MarketModel,
    consent: Option<&ConsentView>,
) {
    if area.width < 12 || area.height < 3 {
        frame.render_widget(Paragraph::new("Resize to browse"), area);
        return;
    }
    let theme = crate::theme::theme();
    let style = Style::default().fg(theme.text_body).bg(theme.surface_bg);
    let (body, footer) = frame_areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(footer_spans(model, &theme))).style(style),
        footer,
    );
    if model.phase == MarketPhase::Browsing && !model.entries.is_empty() {
        render_browsing(frame, area, model, &theme, style);
        return;
    }
    if model.phase == MarketPhase::Loading && render_loading(frame, body, model, style) {
        return;
    }
    let mut doc = Doc::default();
    match model.phase {
        MarketPhase::Loading => doc.line(vec![
            Span::styled(
                format!("{} ", SPINNER[model.spinner % SPINNER.len()]),
                fg(theme.accent),
            ),
            Span::styled("Loading marketplace…", fg(theme.text_muted)),
        ]),
        MarketPhase::LoadError => {
            doc.line(Span::styled("Marketplace unavailable", bold(theme.error)));
            doc.paragraph(
                model.message.as_deref().unwrap_or_default(),
                fg(theme.text_body),
            );
        }
        MarketPhase::Browsing => {
            doc.line(Span::styled(
                status_text(model).unwrap_or_default().to_string(),
                fg(theme.text_muted),
            ));
        }
        MarketPhase::Confirm => confirm_doc(&mut doc, model, &theme),
        MarketPhase::Preparing | MarketPhase::Committing => progress_doc(&mut doc, model, &theme),
        MarketPhase::Consent => consent_doc(&mut doc, consent, &theme),
        MarketPhase::Installed => {
            doc.line(vec![
                Span::styled("✓ ", fg(theme.success)),
                Span::styled("Installed", bold(theme.success)),
            ]);
            doc.blank();
            for line in safe_text(model.message.as_deref().unwrap_or_default()).lines() {
                if let Some(warning) = line.strip_prefix("Warning:") {
                    doc.hanging(
                        vec![
                            Span::styled("! ", bold(theme.accent)),
                            Span::styled(format!("Warning:{warning}"), fg(theme.accent_soft)),
                        ],
                        2,
                    );
                } else {
                    doc.line(Span::styled(line.to_string(), fg(theme.text_body)));
                }
            }
        }
        MarketPhase::InstallFailed => {
            doc.line(vec![
                Span::styled("✗ ", fg(theme.error)),
                Span::styled("Install failed", bold(theme.error)),
            ]);
            doc.blank();
            doc.paragraph(
                model.message.as_deref().unwrap_or_default(),
                fg(theme.text_body),
            );
        }
    }
    doc.render(frame, body, model.scroll, style);
}

/// Replays the startup logo intro while the catalog loads: the mark draws in,
/// holds briefly, then draws again, above a shimmering label.
fn render_loading(frame: &mut Frame<'_>, body: Rect, model: &MarketModel, style: Style) -> bool {
    use crate::composer::{INTRO_TOTAL_MS, MARK_CELLS, intro_frame_index, intro_mark_rows};
    const HOLD_MS: u64 = 700;
    const LABEL: &str = "Loading marketplace…";
    let (mark_w, mark_h) = MARK_CELLS;
    let width = usize::from(body.width);
    if width < mark_w.max(visible_width(LABEL)) || usize::from(body.height) < mark_h + 2 {
        return false;
    }
    let elapsed = model.loading_since.map(|since| since.elapsed());
    let frame_idx = match elapsed {
        Some(elapsed) => {
            let cycle = elapsed.as_millis() as u64 % (INTRO_TOTAL_MS + HOLD_MS);
            intro_frame_index(Some(std::time::Duration::from_millis(cycle)), 1.0)
        }
        None => intro_frame_index(None, 1.0),
    };
    let (mark, _) = intro_mark_rows(frame_idx);
    if mark.is_empty() {
        return false;
    }
    let top = usize::from(body.height).saturating_sub(mark_h + 2) / 2;
    let mut lines = vec![Line::default(); top];
    // Center on the fixed mark size so partially drawn frames do not jitter.
    let mark_pad = " ".repeat(width.saturating_sub(mark_w) / 2);
    lines.extend(mark.into_iter().map(|spans| {
        let mut row = vec![Span::raw(mark_pad.clone())];
        row.extend(spans);
        Line::from(row)
    }));
    lines.push(Line::default());
    let mut label = vec![Span::raw(
        " ".repeat(width.saturating_sub(visible_width(LABEL)) / 2),
    )];
    label.extend(crate::composer::draw::shimmer_spans(
        LABEL,
        elapsed.unwrap_or_default(),
    ));
    lines.push(Line::from(label));
    frame.render_widget(Paragraph::new(lines).style(style), body);
    true
}

fn render_browsing(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &MarketModel,
    theme: &UiTheme,
    style: Style,
) {
    let (body, _) = frame_areas(area);
    let (list_area, details_area) = browsing_panes(area);
    frame.render_widget(
        Paragraph::new(search_line(model, usize::from(body.width), theme)).style(style),
        Rect::new(body.x, body.y, body.width, body.height.min(1)),
    );
    let divider = fg(theme.text_faint).bg(theme.surface_bg);
    if details_area.y == list_area.y && details_area.width > 0 {
        let x = list_area.right() + PANE_GAP / 2;
        let rule = vec![Line::from("│"); usize::from(list_area.height)];
        frame.render_widget(
            Paragraph::new(rule).style(divider),
            Rect::new(x, list_area.y, 1, list_area.height),
        );
    } else if details_area.y > list_area.bottom() {
        frame.render_widget(
            Paragraph::new("─".repeat(usize::from(list_area.width))).style(divider),
            Rect::new(list_area.x, list_area.bottom(), list_area.width, 1),
        );
    }
    if let Some(status) = status_text(model) {
        frame.render_widget(
            Paragraph::new(status).style(style.fg(theme.text_muted)),
            list_area,
        );
        return;
    }
    let mut state = ListState::default()
        .with_selected(Some(model.selected))
        .with_offset(model.offset);
    frame.render_stateful_widget(
        List::new(list_items(model, list_area.width, theme))
            .style(style)
            .highlight_symbol("> ")
            .highlight_style(
                Style::default()
                    .fg(theme.on_selection)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        list_area,
        &mut state,
    );
    let mut doc = Doc::default();
    details_doc(&mut doc, model, theme);
    doc.render(frame, details_area, model.scroll, style);
}

fn search_line(model: &MarketModel, width: usize, theme: &UiTheme) -> Line<'static> {
    let query = one_line(&model.query);
    let mut spans = if model.search_focused {
        vec![
            Span::styled("/ ", bold(theme.accent)),
            Span::styled(query, fg(theme.text_bright)),
            Span::styled("█", fg(theme.accent)),
        ]
    } else if query.is_empty() {
        vec![Span::styled("/ to search", fg(theme.text_dim))]
    } else {
        vec![
            Span::styled("/ ", fg(theme.text_dim)),
            Span::styled(query, fg(theme.text_body)),
        ]
    };
    let total = model.entries.len();
    let shown = model.visible_names().len();
    let count = if model.query.is_empty() {
        format!("{total} plugin{}", if total == 1 { "" } else { "s" })
    } else {
        format!("{shown} of {total}")
    };
    let used: usize = spans.iter().map(|s| visible_width(&s.content)).sum();
    let count_width = visible_width(&count);
    if used + count_width + 2 <= width {
        spans.push(Span::raw(" ".repeat(width - used - count_width)));
        spans.push(Span::styled(count, fg(theme.text_dim)));
    }
    Line::from(spans)
}

fn list_items(model: &MarketModel, width: u16, theme: &UiTheme) -> Vec<ListItem<'static>> {
    let names = model.visible_names();
    // The list reserves the highlight symbol's width on every row.
    let row_width = usize::from(width).saturating_sub(2);
    let show_version = row_width >= 28;
    let version_width = if show_version {
        names
            .iter()
            .map(|name| visible_width(&one_line(&model.entries[*name].version)))
            .max()
            .unwrap_or(0)
            .min(12)
    } else {
        0
    };
    let fixed = 2 + if show_version { version_width + 1 } else { 0 } + 2;
    let name_budget = row_width.saturating_sub(fixed).max(4);
    names
        .into_iter()
        .map(|name| {
            let entry = &model.entries[name];
            let trust = Trust::of(entry);
            let label = elide(&one_line(name), name_budget);
            let pad = name_budget.saturating_sub(visible_width(&label));
            let mut spans = vec![
                Span::styled(trust.icon(), fg(trust.color(theme))),
                Span::raw(" "),
                Span::styled(label, fg(theme.text_body)),
                Span::raw(" ".repeat(pad)),
            ];
            if show_version {
                let version = elide(&one_line(&entry.version), version_width);
                let lead = version_width.saturating_sub(visible_width(&version));
                spans.push(Span::styled(
                    format!(" {}{version}", " ".repeat(lead)),
                    fg(theme.text_dim),
                ));
            }
            spans.push(Span::raw(" "));
            spans.push(if model.installed.contains(name) {
                Span::styled("✓", fg(theme.success))
            } else {
                Span::raw(" ")
            });
            ListItem::new(Line::from(spans))
        })
        .collect()
}

fn footer_spans(model: &MarketModel, theme: &UiTheme) -> Vec<Span<'static>> {
    let keys: &[(&str, &str)] = match model.phase {
        MarketPhase::Loading => &[],
        MarketPhase::LoadError | MarketPhase::InstallFailed => &[("r", "retry"), ("Esc", "back")],
        MarketPhase::Browsing if model.search_focused => &[
            ("←→", "tabs"),
            ("", "Type to search"),
            ("Enter/Esc", "done"),
        ],
        MarketPhase::Browsing => &[
            ("←→", "tabs"),
            ("/", "search"),
            ("↑↓/jk", "select"),
            ("Enter", "install"),
            ("PgUp/PgDn", "details"),
        ],
        MarketPhase::Confirm => &[("Enter", "approve"), ("Esc", "back")],
        MarketPhase::Preparing => &[("Esc", "cancel"), ("", "cleanup finishes before leaving")],
        MarketPhase::Consent => return consent_buttons(model.grant_selected, theme),
        MarketPhase::Committing => &[("", "Finishing installation · cancellation disabled")],
        MarketPhase::Installed => &[("Enter/Esc", "back")],
    };
    let mut spans = Vec::new();
    for (i, (key, label)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", fg(theme.text_faint)));
        }
        if !key.is_empty() {
            spans.push(Span::styled(format!("{key} "), bold(theme.text_bright)));
        }
        spans.push(Span::styled(*label, fg(theme.text_dim)));
    }
    spans
}

fn consent_buttons(grant_selected: bool, theme: &UiTheme) -> Vec<Span<'static>> {
    let chip = |text: &'static str, selected: bool| {
        Span::styled(
            text,
            if selected {
                Style::default()
                    .fg(theme.on_selection)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_soft).bg(theme.input_bg)
            },
        )
    };
    let (grant, cancel) = if grant_selected {
        (" Grant · Enter/g ", " Cancel · Esc ")
    } else {
        (" Grant · g ", " Cancel · Enter/Esc ")
    };
    vec![
        chip(grant, grant_selected),
        Span::raw("  "),
        chip(cancel, !grant_selected),
        Span::styled("  Tab ", bold(theme.text_bright)),
        Span::styled("switch", fg(theme.text_dim)),
    ]
}

fn details_doc(doc: &mut Doc, model: &MarketModel, theme: &UiTheme) {
    let Some(name) = model.selected_name() else {
        return;
    };
    let entry = &model.entries[name];
    let mut header = vec![
        Span::styled(one_line(name), bold(theme.text_bright)),
        Span::styled(
            format!("  {}", one_line(&entry.version)),
            fg(theme.text_dim),
        ),
    ];
    if model.installed.contains(name) {
        header.push(Span::styled("  ✓ installed", fg(theme.success)));
    }
    doc.line(header);
    if let Some(description) = entry
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        doc.paragraph(description, fg(theme.text_body));
    }
    doc.blank();
    if let Some(author) = entry.author.as_deref().filter(|a| !a.trim().is_empty()) {
        doc.kv("Author", vec![value(author, theme)], theme);
    }
    doc.kv("Trust", Trust::of(entry).spans(theme), theme);
    if !entry.kind.is_empty() {
        doc.kv("Kind", vec![value(&entry.kind, theme)], theme);
    }
    if !entry.license.is_empty() {
        doc.kv("License", vec![value(&entry.license, theme)], theme);
    }
    if !entry.commands.is_empty() {
        doc.blank();
        doc.kv("Commands", command_tokens(&entry.commands, theme), theme);
    }
    let links = [("Homepage", &entry.homepage), ("Repo", &entry.repo)];
    if links.iter().any(|(_, url)| url.is_some()) {
        doc.blank();
        for (key, url) in links {
            if let Some(url) = url {
                doc.kv(
                    key,
                    vec![Span::styled(
                        gray_pkg::fetch::redact(&one_line(url)),
                        fg(theme.text_dim),
                    )],
                    theme,
                );
            }
        }
    }
    if !model.can_install() && !model.installed.contains(name) {
        doc.blank();
        doc.line(Span::styled(
            "This catalog package is not supported by the marketplace.",
            fg(theme.error_soft),
        ));
    }
}

fn value(raw: &str, theme: &UiTheme) -> Span<'static> {
    Span::styled(one_line(raw), fg(theme.text_body))
}

fn command_tokens(commands: &[String], theme: &UiTheme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, command) in commands.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(one_line(command), fg(theme.accent_soft)));
    }
    spans
}

fn confirm_doc(doc: &mut Doc, model: &MarketModel, theme: &UiTheme) {
    let Some(approved) = model.approved.as_ref() else {
        return;
    };
    let entry = approved.entry();
    doc.line(vec![
        Span::styled("Confirm preparation: ", bold(theme.text_bright)),
        Span::styled(one_line(approved.name()), bold(theme.accent)),
    ]);
    doc.blank();
    doc.kv("Version", vec![value(&entry.version, theme)], theme);
    doc.kv("Trust", Trust::of(entry).spans(theme), theme);
    doc.kv(
        "Source",
        vec![value(&gray_pkg::fetch::redact(&entry.source.url), theme)],
        theme,
    );
    doc.kv(
        "Hash",
        vec![Span::styled(
            one_line(entry.hash.primary().unwrap_or("—")),
            fg(theme.text_dim),
        )],
        theme,
    );
    doc.blank();
    for (note, color) in [
        (
            "A matching hash proves integrity, not safety.",
            theme.accent_soft,
        ),
        (
            "Preparing will run this plugin's manifest command as you.",
            theme.accent,
        ),
    ] {
        doc.hanging(
            vec![
                Span::styled("! ", bold(theme.accent)),
                Span::styled(note, fg(color)),
            ],
            2,
        );
    }
}

fn progress_doc(doc: &mut Doc, model: &MarketModel, theme: &UiTheme) {
    const STAGES: [(InstallStage, &str); 5] = [
        (InstallStage::Downloading, "Downloading"),
        (InstallStage::Unpacking, "Unpacking"),
        (InstallStage::Scanning, "Scanning"),
        (InstallStage::ReadingManifest, "Reading manifest"),
        (InstallStage::Committing, "Committing"),
    ];
    let name = model
        .approved
        .as_ref()
        .map(|approved| one_line(approved.name()))
        .unwrap_or_else(|| "plugin".into());
    doc.line(vec![
        Span::styled("Installing ", bold(theme.text_bright)),
        Span::styled(name, bold(theme.accent)),
    ]);
    doc.blank();
    let current = model
        .stage
        .and_then(|stage| STAGES.iter().position(|(s, _)| *s == stage))
        .unwrap_or(0);
    for (i, (_, label)) in STAGES.iter().enumerate() {
        let line = if i < current {
            vec![
                Span::styled("✓ ", fg(theme.success)),
                Span::styled(*label, fg(theme.text_muted)),
            ]
        } else if i == current {
            vec![
                Span::styled(
                    format!("{} ", SPINNER[model.spinner % SPINNER.len()]),
                    fg(theme.accent),
                ),
                Span::styled(*label, bold(theme.text_bright)),
            ]
        } else {
            vec![
                Span::styled("· ", fg(theme.text_faint)),
                Span::styled(*label, fg(theme.text_dim)),
            ]
        };
        doc.line(line);
    }
    if model.cancelling || model.close_requested {
        doc.blank();
        doc.line(Span::styled(
            "Waiting for cleanup or commit to finish…",
            fg(theme.accent_soft),
        ));
    }
}

fn consent_doc(doc: &mut Doc, consent: Option<&ConsentView>, theme: &UiTheme) {
    let Some(consent) = consent else {
        return;
    };
    doc.line(Span::styled("Capability consent", bold(theme.text_bright)));
    doc.line(Span::styled(
        "Review what this plugin can do before granting it.",
        fg(theme.text_dim),
    ));
    doc.blank();
    doc.line(Span::styled("Capabilities", bold(theme.text_muted)));
    if consent.capabilities.is_empty() {
        doc.line(Span::styled("  None declared", fg(theme.text_dim)));
    }
    for capability in &consent.capabilities {
        let mut spans = vec![
            Span::styled("  • ", fg(theme.text_dim)),
            Span::styled(one_line(&capability.id), bold(theme.text_bright)),
        ];
        if !capability.grantable {
            spans.push(Span::styled("  not grantable", bold(theme.error_soft)));
        }
        spans.push(Span::styled(
            format!("  {}", one_line(&capability.description)),
            fg(theme.text_body),
        ));
        doc.hanging(spans, 4);
    }
    doc.blank();
    if consent.commands.is_empty() {
        doc.kv(
            "Commands",
            vec![Span::styled("None declared", fg(theme.text_dim))],
            theme,
        );
    } else {
        doc.kv("Commands", command_tokens(&consent.commands, theme), theme);
    }
    doc.blank();
    doc.line(Span::styled("Scan", bold(theme.text_muted)));
    for line in safe_text(&consent.scan_text).lines() {
        doc.hanging(
            vec![Span::styled(format!("  {line}"), fg(theme.text_body))],
            2,
        );
    }
    doc.blank();
    doc.line(Span::styled(
        "Granting accepts the scan findings above and the known capabilities.",
        fg(theme.text_dim),
    ));
}

/// Styled lines that wrap at word boundaries when rendered. Each line keeps a
/// hanging indent so wrapped values stay aligned under their column.
#[derive(Default)]
struct Doc {
    lines: Vec<(Line<'static>, usize)>,
}

impl Doc {
    fn line(&mut self, line: impl Into<Line<'static>>) {
        self.hanging(line, 0);
    }

    fn hanging(&mut self, line: impl Into<Line<'static>>, hang: usize) {
        self.lines.push((line.into(), hang));
    }

    fn blank(&mut self) {
        self.line(Line::default());
    }

    fn paragraph(&mut self, raw: &str, style: Style) {
        for line in safe_text(raw).lines() {
            self.line(Span::styled(line.to_string(), style));
        }
    }

    fn kv(&mut self, key: &str, value: Vec<Span<'static>>, theme: &UiTheme) {
        let mut spans = vec![Span::styled(
            format!("{key:<KEY_WIDTH$}"),
            fg(theme.text_muted),
        )];
        spans.extend(value);
        self.hanging(spans, KEY_WIDTH);
    }

    fn render(self, frame: &mut Frame<'_>, area: Rect, scroll: u16, style: Style) {
        if area.is_empty() {
            return;
        }
        let width = usize::from(area.width);
        let lines = self
            .lines
            .into_iter()
            .flat_map(|(line, hang)| wrap_line(line, width, hang))
            .collect::<Vec<_>>();
        let max_scroll = lines.len().saturating_sub(usize::from(area.height));
        // Clamp the view, not just input, so scrolling cannot hide all content.
        let offset = usize::from(scroll)
            .min(max_scroll)
            .min(usize::from(u16::MAX)) as u16;
        frame.render_widget(Paragraph::new(lines).style(style).scroll((offset, 0)), area);
    }
}

struct Wrapper {
    width: usize,
    hang: usize,
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    used: usize,
    fresh: bool,
}

impl Wrapper {
    fn newline(&mut self) {
        self.lines.push(Line::from(std::mem::take(&mut self.spans)));
        if self.hang > 0 {
            self.spans.push(Span::raw(" ".repeat(self.hang)));
        }
        self.used = self.hang;
        self.fresh = true;
    }

    fn line_empty(&self) -> bool {
        self.fresh || self.used == 0
    }

    fn space(&mut self, text: String, style: Style) {
        if self.fresh {
            return;
        }
        let w = visible_width(&text);
        if self.used + w > self.width {
            self.newline();
            return;
        }
        self.spans.push(Span::styled(text, style));
        self.used += w;
    }

    fn word(&mut self, text: String, style: Style) {
        let w = visible_width(&text);
        if self.used + w > self.width && !self.line_empty() && self.hang + w <= self.width {
            self.newline();
        }
        if self.used + w <= self.width {
            self.spans.push(Span::styled(text, style));
            self.used += w;
            self.fresh = false;
            return;
        }
        let mut chunk = String::new();
        let mut chunk_w = 0;
        for c in text.chars() {
            let cw = c.width().unwrap_or_default();
            let empty = chunk_w == 0 && self.line_empty();
            if self.used + chunk_w + cw > self.width && !empty {
                if !chunk.is_empty() {
                    self.spans
                        .push(Span::styled(std::mem::take(&mut chunk), style));
                }
                self.newline();
                chunk_w = 0;
            }
            chunk.push(c);
            chunk_w += cw;
        }
        if !chunk.is_empty() {
            self.spans.push(Span::styled(chunk, style));
            self.used += chunk_w;
            self.fresh = false;
        }
    }
}

fn wrap_line(line: Line<'static>, width: usize, hang: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }
    let mut wrapper = Wrapper {
        width,
        // A hanging indent only helps when it leaves most of the row for values.
        hang: if hang * 3 > width { 0 } else { hang },
        lines: Vec::new(),
        spans: Vec::new(),
        used: 0,
        fresh: false,
    };
    for span in line.spans {
        let style = span.style;
        let mut piece = String::new();
        let mut piece_is_space = false;
        for c in span.content.chars() {
            let is_space = c == ' ';
            if !piece.is_empty() && is_space != piece_is_space {
                let text = std::mem::take(&mut piece);
                if piece_is_space {
                    wrapper.space(text, style);
                } else {
                    wrapper.word(text, style);
                }
            }
            piece_is_space = is_space;
            piece.push(c);
        }
        if !piece.is_empty() {
            if piece_is_space {
                wrapper.space(piece, style);
            } else {
                wrapper.word(piece, style);
            }
        }
    }
    wrapper.lines.push(Line::from(wrapper.spans));
    wrapper.lines
}

fn elide(text: &str, width: usize) -> String {
    if visible_width(text) <= width {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or_default();
        if used + w >= width {
            break;
        }
        result.push(c);
        used += w;
    }
    result.push('…');
    result
}
