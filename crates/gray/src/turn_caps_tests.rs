use super::*;

fn cfg() -> Config {
    Config {
        temperature: None,
        top_p: None,
        model: None,
        base_url: String::new(),
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

#[test]
fn no_caps_never_stops() {
    assert_eq!(check_caps(&cfg(), 0, 0.0), None);
}

#[test]
fn turn_cap_fires_at_limit() {
    let mut c = cfg();
    c.max_turns = Some(2);
    assert_eq!(check_caps(&c, 1, 0.0), None);
    assert!(check_caps(&c, 2, 0.0).is_some());
}

#[test]
fn spend_cap_ignores_unpriced_and_fires_when_priced() {
    let mut c = cfg();
    c.max_cost_micros = Some(100_000); // $0.10
    assert_eq!(check_caps(&c, 0, 0.0), None);
    assert!(check_caps(&c, 5, 0.11).is_some());
}
