use super::*;
use gray_core::agent::{PanelPage, PanelSpan, PanelStyle};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

fn page(tab: &str, title: &str, lines: &[&str]) -> PagerPage {
    PagerPage {
        tab: tab.to_string(),
        lines: lines.iter().map(|l| Line::from(l.to_string())).collect(),
    }
}

fn many_lines(tab: &str, n: usize) -> PagerPage {
    let lines: Vec<String> = (0..n).map(|i| format!("line {i:02}")).collect();
    PagerPage {
        tab: tab.to_string(),
        lines: lines.iter().map(|l| Line::from(l.clone())).collect(),
    }
}

fn screen(
    w: u16,
    h: u16,
    pages: &[PagerPage],
    page: usize,
    scroll: usize,
    refreshable: bool,
) -> Vec<String> {
    let bg = BackgroundSnapshot::default_initial();
    let mut scroll = scroll;
    let mut terminal = ratatui::Terminal::new(TestBackend::new(w, h)).expect("test terminal");
    terminal
        .draw(|frame| draw_pager(frame, &bg, pages, page, &mut scroll, refreshable))
        .expect("draw pager");
    let buf = terminal.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect()
}

#[test]
fn tabs_and_footer_render() {
    let pages = vec![
        page("Alpha", "A", &["first"]),
        page("Beta", "B", &["second"]),
    ];
    let lines = screen(80, 16, &pages, 0, 0, true);
    assert!(lines.iter().any(|l| l.contains(" Alpha ")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains(" Beta ")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("first")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("r refresh")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("esc close")), "{lines:#?}");
}

#[test]
fn footer_hides_refresh_hint_when_not_refreshable() {
    let pages = vec![page("Only", "O", &["body"])];
    let lines = screen(80, 16, &pages, 0, 0, false);
    assert!(!lines.iter().any(|l| l.contains("refresh")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("esc close")), "{lines:#?}");
    assert!(!lines.iter().any(|l| l.contains("←/→ page")), "{lines:#?}");
}

#[test]
fn body_scrolls_within_the_page() {
    let pages = vec![many_lines("Long", 20)];
    let lines = screen(60, 12, &pages, 0, 3, false);
    assert!(lines.iter().any(|l| l.contains("line 03")), "{lines:#?}");
    assert!(!lines.iter().any(|l| l.contains("line 00")), "{lines:#?}");
    assert!(!lines.iter().any(|l| l.contains("line 19")), "{lines:#?}");
}

#[test]
fn pages_from_spec_maps_roles_and_modifiers() {
    let spec = PanelSpec {
        title: "Deploys".into(),
        refreshable: true,
        pages: vec![PanelPage {
            tab: "Prod".into(),
            title: "Prod".into(),
            lines: vec![vec![
                PanelSpan {
                    text: "ok".into(),
                    style: Some(PanelStyle {
                        fg: Some("success".into()),
                        bold: true,
                        italic: true,
                        dim: true,
                    }),
                },
                PanelSpan {
                    text: "fallback".into(),
                    style: Some(PanelStyle {
                        fg: Some("not-a-role".into()),
                        bold: false,
                        italic: false,
                        dim: false,
                    }),
                },
                PanelSpan {
                    text: "plain".into(),
                    style: None,
                },
            ]],
        }],
    };
    let pages = pages_from_spec(&spec);
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].tab, "Prod");
    let th = crate::theme::theme();
    let spans = &pages[0].lines[0].spans;
    assert_eq!(spans[0].style.fg, Some(th.success));
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert!(spans[0].style.add_modifier.contains(Modifier::ITALIC));
    assert!(spans[0].style.add_modifier.contains(Modifier::DIM));
    assert_eq!(spans[1].style.fg, Some(th.text_body));
    assert!(!spans[1].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(spans[2].style.fg, Some(th.text_body));
}

#[test]
fn pages_from_outcome_replaces_only_for_panel_or_say() {
    let spec = PanelSpec {
        title: "T".into(),
        refreshable: false,
        pages: vec![PanelPage {
            tab: "A".into(),
            title: "A".into(),
            lines: vec![vec![PanelSpan {
                text: "x".into(),
                style: None,
            }]],
        }],
    };
    let pages = pages_from_outcome(Some(CommandOutcome::Panel(spec)));
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].lines[0].spans[0].content.as_ref(), "x");

    let pages = pages_from_outcome(Some(CommandOutcome::Say("hello\nworld".into())));
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].lines.len(), 2);

    assert!(pages_from_outcome(Some(CommandOutcome::Prompt("p".into()))).is_empty());
    assert!(pages_from_outcome(None).is_empty());
}

#[test]
fn spec_plain_flattens_titles_and_lines() {
    let spec = PanelSpec {
        title: "Deploys".into(),
        refreshable: false,
        pages: vec![PanelPage {
            tab: "Prod".into(),
            title: "Prod page".into(),
            lines: vec![
                vec![
                    PanelSpan {
                        text: "api ".into(),
                        style: None,
                    },
                    PanelSpan {
                        text: "ok".into(),
                        style: Some(PanelStyle {
                            fg: Some("success".into()),
                            bold: false,
                            italic: false,
                            dim: false,
                        }),
                    },
                ],
                vec![],
            ],
        }],
    };
    let text = spec_plain(&spec);
    assert!(text.contains("Deploys"), "{text}");
    assert!(text.contains("Prod page"), "{text}");
    assert!(text.contains("api ok"), "{text}");
    assert!(!text.contains('\x1b'), "{text}");
}
