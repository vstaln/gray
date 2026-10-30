use super::*;

pub(super) fn md_style(t: &Theme) -> TextViewStyle {
    TextViewStyle::default().code_block(
        StyleRefinement::default()
            .border_1()
            .border_color(t.foreground.opacity(0.10))
            .p(px(12.)),
    )
}

/// A timeline row: one item, or a run of tool activity folded together.
pub(super) enum Row {
    Item(usize),
    Group(std::ops::Range<usize>),
}

impl Row {
    pub(super) fn contains(&self, ix: usize) -> bool {
        match self {
            Row::Item(i) => *i == ix,
            Row::Group(r) => r.contains(&ix),
        }
    }
}

/// Checkpoints stay as their own row: they sit before the prompt, not in the turn.
pub(super) fn groupable(item: &Item) -> bool {
    matches!(item, Item::Activity { kind, .. } if kind != "checkpoint")
}

pub(super) fn timeline_rows(items: &[Item]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if groupable(&items[i]) {
            let start = i;
            while i < items.len() && groupable(&items[i]) {
                i += 1;
            }
            rows.push(Row::Group(start..i));
        } else {
            rows.push(Row::Item(i));
            i += 1;
        }
    }
    rows
}

pub(super) fn item_id(item: &Item) -> &str {
    match item {
        Item::Message { id, .. } | Item::Activity { id, .. } => id,
    }
}

pub(super) fn item_at(item: &Item) -> i64 {
    match item {
        Item::Message { at, .. } | Item::Activity { at, .. } => *at,
    }
}

pub(super) fn group_id(first: &Item) -> String {
    format!("grp-{}", item_id(first))
}

pub(super) struct GroupInfo {
    pub(super) id: String,
    pub(super) live: bool,
    pub(super) err: bool,
    pub(super) steps: usize,
    pub(super) elapsed_ms: i64,
}

impl GroupInfo {
    pub(super) fn label(&self) -> String {
        if self.live {
            return "Working…".to_string();
        }
        format!("Worked for {}", fmt_elapsed(self.elapsed_ms.max(0) / 1000))
    }
}

/// "8s", "1m", "1m 12s".
pub(super) fn fmt_elapsed(secs: i64) -> String {
    let (m, s) = (secs / 60, secs % 60);
    match (m, s) {
        (0, s) => format!("{s}s"),
        (m, 0) => format!("{m}m"),
        (m, s) => format!("{m}m {s}s"),
    }
}

/// The turn runs from the prompt before the group to the reply after it
/// (or the group's last step when the turn ended without a reply).
pub(super) fn group_info(items: &[Item], r: &std::ops::Range<usize>, running: bool) -> GroupInfo {
    let start = item_at(&items[r.start.saturating_sub(1)]);
    let end = match items.get(r.end) {
        Some(m @ Item::Message { .. }) => item_at(m),
        _ => item_at(&items[r.end - 1]),
    };
    GroupInfo {
        id: group_id(&items[r.start]),
        live: running && r.end == items.len(),
        err: items[r.clone()]
            .iter()
            .any(|i| matches!(i, Item::Activity { kind, .. } if kind == "error")),
        steps: r.len(),
        elapsed_ms: end - start,
    }
}

pub(super) fn render_item(item: &Item, t: &Theme, burst: bool) -> AnyElement {
    let ink = t.foreground;
    match item {
        Item::Message {
            id,
            role: Role::User,
            text,
            ..
        } => {
            let bubble = div()
                .id(SharedString::from(format!("msg-{id}")))
                .max_w(px(576.))
                .px(px(12.))
                .py(px(8.))
                .bg(ink.opacity(0.08))
                .border_1()
                .border_color(ink.opacity(0.10))
                .text_size(px(14.))
                .line_height(px(21.))
                .child(text.clone());
            let bubble = if burst {
                motion::celebrate(bubble, id, t).into_any_element()
            } else {
                bubble.into_any_element()
            };
            h_flex()
                .justify_end()
                .pt(px(6.))
                .child(bubble)
                .into_any_element()
        }
        Item::Message { id, text, .. } => div()
            .text_size(px(14.))
            .line_height(px(22.))
            .child(
                TextView::markdown(SharedString::from(format!("msg-{id}")), text.clone())
                    .style(md_style(t))
                    .font_family("Plus Jakarta Sans")
                    .selectable(true),
            )
            .into_any_element(),
        Item::Activity {
            kind,
            label,
            detail,
            ..
        } => {
            let err = kind == "error";
            let (icon, icon_color) = match kind.as_str() {
                "error" => (GIcon::CircleX, t.danger.opacity(0.8)),
                "edit" => (GIcon::FileText, t.success.opacity(0.8)),
                "command" => (GIcon::SquareTerminal, ink.opacity(0.45)),
                "checkpoint" => (GIcon::Undo2, ink.opacity(0.45)),
                "thinking" => (GIcon::Info, ink.opacity(0.45)),
                _ => (GIcon::Check, ink.opacity(0.45)),
            };
            let short: String = detail
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect();
            let short = if kind == "checkpoint" {
                short.chars().take(10).collect()
            } else {
                short
            };
            h_flex()
                .gap(px(7.))
                .py(px(1.))
                .min_w_0()
                .child(Icon::new(icon).size(px(14.)).text_color(icon_color))
                .child(
                    div()
                        .flex_none()
                        .max_w(px(460.))
                        .truncate()
                        .px(px(5.))
                        .bg(if err {
                            t.danger.opacity(0.10)
                        } else {
                            ink.opacity(0.06)
                        })
                        .font_family(t.mono_font_family.clone())
                        .text_size(px(12.5))
                        .text_color(if err {
                            t.danger.opacity(0.9)
                        } else {
                            ink.opacity(0.7)
                        })
                        .child(label.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(if err {
                            t.danger.opacity(0.8)
                        } else {
                            ink.opacity(faint(t, 0.4))
                        })
                        .child(short),
                )
                .into_any_element()
        }
    }
}

pub(super) fn background_layer(bg: &str) -> Div {
    div().absolute().inset_0().overflow_hidden().child(
        match bg {
            "andromeda" => img("bg/andromeda.png"),
            "bluemarble" => img("bg/bluemarble.png"),
            path => img(PathBuf::from(path)),
        }
        .w_full()
        .h_full()
        .opacity(0.3)
        .grayscale(true),
    )
}

pub(super) fn bg_rows(cx: &mut Context<GrayApp>, app: &GrayApp) -> Vec<AnyElement> {
    let cur = app.store.state.setting("background", "none");
    [
        ("none", "None"),
        ("andromeda", "Andromeda"),
        ("bluemarble", "Blue Marble"),
    ]
    .into_iter()
    .map(|(id, label)| {
        let id = id.to_string();
        Button::new(SharedString::from(format!("bg-{}", id)))
            .small()
            .label(label)
            .when(cur == id, |b| b.primary())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run(Cmd::SetBackground(id.clone()), window, cx);
                this.settings_open = true;
            }))
            .into_any_element()
    })
    .collect()
}

/// One-line preview of `text` around the first case-insensitive hit of `q`.
pub(super) fn snippet(text: &str, q: &str, width: usize) -> Option<String> {
    // Lowercase one char per original char so match indexes line up with
    // `chars` (`str::to_lowercase` can expand, e.g. 'İ' -> "i̇").
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let needle: Vec<char> = q.chars().collect();
    if needle.is_empty() || needle.len() > lower.len() {
        return None;
    }
    let hit = lower
        .windows(needle.len())
        .position(|w| w == needle.as_slice())?;
    let start = hit.min(chars.len()).saturating_sub(width / 3);
    let end = (start + width).min(chars.len());
    let mut out: String = chars[start..end]
        .iter()
        .map(|c| if c.is_whitespace() { ' ' } else { *c })
        .collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
        out.push('…');
    }
    Some(out)
}

/// First `n` chars of `text`, flattened to one line with markdown
/// emphasis/code markers dropped (notification bodies are plain text).
pub(super) fn preview(text: &str, n: usize) -> String {
    let flat = text
        .split_whitespace()
        .map(|w| w.trim_start_matches('#').replace(['*', '`'], ""))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() <= n {
        return flat;
    }
    let mut out: String = flat.chars().take(n).collect();
    out.push('…');
    out
}

pub(super) fn cmd_action(cmd: &Cmd) -> Option<Box<dyn Action>> {
    Some(match cmd {
        Cmd::NewThread => Box::new(NewThread),
        Cmd::SearchThreads => Box::new(SearchThreads),
        Cmd::ToggleSidebar => Box::new(ToggleSidebar),
        Cmd::ToggleTerminal => Box::new(ToggleTerminal),
        Cmd::ToggleDiff => Box::new(ToggleDiff),
        Cmd::ToggleFiles => Box::new(ToggleFiles),
        Cmd::Settings => Box::new(OpenSettings),
        Cmd::ToggleTheme => Box::new(ToggleTheme),
        Cmd::TogglePlugins => Box::new(TogglePluginsPanel),
        Cmd::ToggleViewer => Box::new(ToggleViewer),
        Cmd::Stop => Box::new(StopTurn),
        _ => return None,
    })
}

pub(super) fn relative(ms: i64) -> String {
    let secs = ((now() - ms) / 1000).max(0);
    if secs < 60 {
        "now".into()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}
