use super::*;
use std::collections::BTreeMap;

fn config() -> Config {
    Config {
        temperature: None,
        top_p: None,
        model: None,
        base_url: "https://unconfigured".into(),
        api_key: None,
        thinking_effort: None,
        show_reasoning: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
    }
}

fn auth() -> BTreeMap<String, catalog::AuthEntry> {
    BTreeMap::from([
        ("openrouter".into(), catalog::AuthEntry::Key("test".into())),
        ("commandcode".into(), catalog::AuthEntry::Key("test".into())),
        ("custom".into(), catalog::AuthEntry::Key("test".into())),
    ])
}

#[test]
fn connected_then_custom_then_stable_remainder() {
    let mut items = build_connect_items(&load_catalog().unwrap());
    let default = items.clone();
    catalog::sort_connect_items(&mut items, &config(), &BTreeMap::new());
    assert_eq!(
        items.iter().map(|i| &i.id).collect::<Vec<_>>(),
        default.iter().map(|i| &i.id).collect::<Vec<_>>()
    );
    catalog::sort_connect_items(&mut items, &config(), &auth());
    assert_eq!(
        items
            .iter()
            .take(4)
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        ["openrouter", "commandcode", "custom", "openai"]
    );
    assert_eq!(items.len(), default.len());
}

#[test]
fn oauth_and_active_keyless_provider_are_connected() {
    let mut config = config();
    config.base_url = "http://localhost:11434/v1/".into();
    let mut auth = auth();
    auth.insert(
        "xai".into(),
        catalog::AuthEntry::OAuth(catalog::StoredAuth {
            provider: "xai".into(),
            access_token: "test".into(),
            refresh_token: "test".into(),
            expires_at: 0,
            email: None,
        }),
    );
    let mut items = build_connect_items(&load_catalog().unwrap());
    catalog::sort_connect_items(&mut items, &config, &auth);
    assert_eq!(
        items
            .iter()
            .take(5)
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        ["openrouter", "commandcode", "ollama", "xai", "custom"]
    );
}

#[test]
fn provider_selection_stays_visible_through_separator_scroll_and_filter() {
    let config = config();
    let auth = auth();
    let mut items = build_connect_items(&load_catalog().unwrap());
    catalog::sort_connect_items(&mut items, &config, &auth);
    let colors = ConnectColors {
        box_bg: Color::Black,
        input_bg: Color::Black,
        accent_peach: Color::Yellow,
        text_dim: Color::DarkGray,
    };
    for (width, height) in [(80, 24), (42, 10), (20, 6)] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        let mut scroll = 0;
        for filter in ["", "open", "custom", "no-such-provider"] {
            let filtered: Vec<_> = items
                .iter()
                .filter(|i| {
                    i.name.to_lowercase().contains(filter)
                        || i.id.contains(filter)
                        || i.sublabel.to_lowercase().contains(filter)
                })
                .collect();
            for sel in (0..filtered.len().max(1)).chain((0..filtered.len()).rev()) {
                terminal
                    .draw(|frame| {
                        render_selecting(
                            frame,
                            frame.area(),
                            &items,
                            filter,
                            sel,
                            &mut scroll,
                            &config,
                            &auth,
                            &colors,
                        )
                    })
                    .unwrap();
                let buf = terminal.backend().buffer();
                let highlighted: String = buf
                    .content
                    .iter()
                    .filter(|cell| cell.bg == Color::Yellow)
                    .map(|cell| cell.symbol())
                    .collect();
                if height < 10 {
                    continue; // Existing compact layout has no provider rows; draw must not panic.
                }
                if let Some(item) = filtered.get(sel) {
                    // Narrow screens must show the beginning of the selected name.
                    let expected: String = item.name.chars().take(5).collect();
                    assert!(
                        highlighted.contains(&expected),
                        "{width}x{height} {filter:?} {sel}: {highlighted:?}"
                    );
                } else {
                    assert!(highlighted.is_empty());
                }
            }
        }
    }
}

#[test]
fn separator_is_after_custom_not_after_first_connected_provider() {
    let config = config();
    let auth = auth();
    let mut items = build_connect_items(&load_catalog().unwrap());
    catalog::sort_connect_items(&mut items, &config, &auth);
    let colors = ConnectColors {
        box_bg: Color::Black,
        input_bg: Color::Black,
        accent_peach: Color::Yellow,
        text_dim: Color::DarkGray,
    };
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| {
            render_selecting(
                frame,
                frame.area(),
                &items,
                "",
                0,
                &mut 0,
                &config,
                &auth,
                &colors,
            )
        })
        .unwrap();
    let buf = terminal.backend().buffer();
    let rows: Vec<String> = (0..24)
        .map(|y| (0..80).map(|x| buf[(x, y)].symbol()).collect())
        .collect();
    let custom = rows.iter().position(|r| r.contains("Custom")).unwrap();
    assert!(rows[custom - 2].contains("OpenRouter"));
    assert!(rows[custom - 1].contains("CommandCode"));
    assert!(rows[custom + 1].contains("────"));
    assert!(rows[custom + 2].contains("OpenAI"));
}
