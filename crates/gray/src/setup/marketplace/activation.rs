use crate::setup::{BackgroundSnapshot, open_modal, render_dimmed_background};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivationChoice {
    Now,
    Later,
}

pub(crate) fn choice_for_key(key: KeyCode, selected: ActivationChoice) -> Option<ActivationChoice> {
    match key {
        KeyCode::Esc => Some(ActivationChoice::Later),
        KeyCode::Enter => Some(selected),
        _ => None,
    }
}

fn render(frame: &mut Frame<'_>, installed: &[String], selected: ActivationChoice) {
    let area = frame.area();
    let modal = Rect::new(
        area.x + area.width.saturating_sub(70) / 2,
        area.y + area.height.saturating_sub(12) / 3,
        area.width.min(70),
        area.height.min(12),
    );
    let theme = crate::theme::theme();
    frame.render_widget(Clear, modal);
    let block = Block::bordered()
        .title(" Activate now? ")
        .style(Style::default().bg(theme.surface_bg).fg(theme.text_body));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);
    let names = installed
        .iter()
        .map(|name| super::draw::safe_text(name))
        .collect::<Vec<_>>()
        .join(", ");
    let option = |choice, label| {
        Span::styled(
            label,
            Style::default().fg(if selected == choice {
                theme.accent
            } else {
                theme.text_dim
            }),
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Installed {}: {names}", installed.len())),
            Line::from(""),
            Line::from("Reload plugins while keeping this conversation and session."),
            Line::from(""),
            Line::from(vec![
                option(ActivationChoice::Now, "[ Activate now ]"),
                Span::raw("   "),
                option(ActivationChoice::Later, "[ Later ]"),
            ]),
            Line::from(""),
            Line::from("←/→ or Tab to choose · Enter to confirm · Esc for Later"),
        ])
        .wrap(Wrap { trim: false }),
        inner,
    );
}

pub(crate) fn prompt(
    bg: Option<&BackgroundSnapshot>,
    installed: &[String],
) -> anyhow::Result<ActivationChoice> {
    let (_session, mut terminal) = open_modal()?;
    let bg = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);
    let mut selected = ActivationChoice::Now;
    loop {
        terminal.draw(|frame| {
            render_dimmed_background(frame, &bg);
            render(frame, installed, selected);
        })?;
        if !poll(std::time::Duration::from_millis(100))? {
            continue;
        }
        if let Event::Key(key) = read()?
            && key.kind == KeyEventKind::Press
        {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return Ok(ActivationChoice::Later);
            }
            match key.code {
                KeyCode::Left | KeyCode::Up => selected = ActivationChoice::Now,
                KeyCode::Right | KeyCode::Down => selected = ActivationChoice::Later,
                KeyCode::Tab | KeyCode::BackTab => {
                    selected = match selected {
                        ActivationChoice::Now => ActivationChoice::Later,
                        ActivationChoice::Later => ActivationChoice::Now,
                    };
                }
                _ => {}
            }
            if let Some(choice) = choice_for_key(key.code, selected) {
                return Ok(choice);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_escape_always_defers() {
        use ActivationChoice::{Later, Now};
        assert_eq!(choice_for_key(KeyCode::Esc, Now), Some(Later));
        assert_eq!(choice_for_key(KeyCode::Enter, Now), Some(Now));
        assert_eq!(choice_for_key(KeyCode::Enter, Later), Some(Later));
        assert_eq!(choice_for_key(KeyCode::Char('x'), Now), None);
    }

    #[test]
    fn activation_prompt_names_plugins_and_choices() {
        for (width, height) in [(80, 20), (20, 6), (1, 1)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        &["demo".into(), "ledger".into()],
                        ActivationChoice::Now,
                    )
                })
                .unwrap();
            if width == 80 {
                let buffer = terminal.backend().buffer();
                let text = (0..height)
                    .map(|y| {
                        (0..width)
                            .map(|x| buffer[(x, y)].symbol())
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(text.contains("Activate now?"));
                assert!(text.contains("Installed 2: demo, ledger"));
                assert!(text.contains("[ Activate now ]"));
                assert!(text.contains("[ Later ]"));
            }
        }
    }
}
