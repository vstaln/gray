use super::*;
use crate::theme::{self, UiTheme};

struct Row {
    name: String,
    theme: UiTheme,
}

fn rows() -> Vec<Row> {
    let mut out = vec![Row {
        name: theme::BUILTIN_THEME_NAME.to_string(),
        theme: theme::GRAY_UI_THEME,
    }];
    if let Ok(dir) = theme::themes_dir() {
        for name in theme::list_themes_in(&dir) {
            if theme::is_builtin_name(&name) {
                continue;
            }
            if let Ok((t, _)) = theme::load_theme_in(&dir, &name) {
                out.push(Row { name, theme: t });
            }
        }
    }
    out
}

fn apply(row: &Row) {
    theme::set_theme(
        row.theme,
        (!theme::is_builtin_name(&row.name)).then_some(row.name.as_str()),
    );
}

fn specimen_lines() -> Vec<ratatui::text::Line<'static>> {
    use ratatui::style::Style;
    use ratatui::text::{Line, Span};

    let th = theme::theme();
    vec![
        Line::from(vec![
            Span::styled("Aa bright", Style::default().fg(th.text_bright)),
            Span::styled("  body text", Style::default().fg(th.text_body)),
            Span::styled("  dim", Style::default().fg(th.text_dim)),
            Span::styled("  faint", Style::default().fg(th.text_faint)),
        ]),
        Line::from(vec![
            Span::styled("✓ success", Style::default().fg(th.success)),
            Span::styled("  × error", Style::default().fg(th.error)),
            Span::styled("  i info", Style::default().fg(th.info)),
            Span::styled("  accent", Style::default().fg(th.accent)),
            Span::styled("  rose", Style::default().fg(th.rose)),
        ]),
    ]
}

fn picker<'a>(
    rows: &'a [Row],
    sel: usize,
    on_highlight: crate::repl::picker::OnHighlight<'a, Row>,
) -> crate::repl::picker::ListPicker<'a, Row> {
    crate::repl::picker::ListPicker {
        title: "Themes",
        items: rows,
        label: Box::new(|r: &Row| r.name.as_str()),
        trailing: Some(Box::new(|r: &Row| theme::swatch_spans(&r.theme))),
        footer: Some(Box::new(specimen_lines)),
        on_highlight,
        selected: sel,
    }
}

pub(crate) fn run_theme_picker(bg: Option<&BackgroundSnapshot>) -> anyhow::Result<Option<String>> {
    let rows = rows();
    if rows.len() <= 1 {
        return Ok(None);
    }
    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);
    let active = theme::active_theme_name();
    let sel = rows
        .iter()
        .position(|r| {
            active.as_deref() == Some(r.name.as_str())
                || (active.is_none() && theme::is_builtin_name(&r.name))
        })
        .unwrap_or(0);
    let orig_theme = theme::theme();
    let orig_name = active.clone();

    let mut picker = picker(&rows, sel, Some(Box::new(|r: &Row| apply(r))));
    match crate::repl::picker::run_list_picker(&mut picker, &bg_snapshot) {
        Ok(Some(i)) => Ok(Some(rows[i].name.clone())),
        Ok(None) => {
            theme::set_theme(orig_theme, orig_name.as_deref());
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
