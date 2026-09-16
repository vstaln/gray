use super::{
    MarketTab, SortMode, apply_plugin_view, apply_skill_view, chip_color, filter_short,
    format_install_status, format_preview, format_skill_preview, format_source_row,
    install_spec_for_plugin, install_spec_for_skill, install_status_covered_by_footer,
    installed_summary, next_plugin_filter, next_skill_filter, scroll_top, skill_chip,
    sort_plugins_by_name, sort_skills_by_name, source_chip, split_market_row,
};
use gray_pkg::ops::SearchHit;
use gray_pkg::skills_ops::SkillHit;
use gray_pkg::sources::Source;

fn plugin_hit() -> SearchHit {
    SearchHit {
        name: "demo".to_string(),
        version: "1.2.3".to_string(),
        desc: "does things".to_string(),
        source: Source::GrayIndex,
        version_detail: String::new(),
        trust: String::new(),
        popularity: 0.0,
    }
}

fn skill_hit() -> SkillHit {
    SkillHit {
        name: "arein/gifgrep".to_string(),
        version: "1.2.3".to_string(),
        desc: "grep gifs".to_string(),
        source: "ClawHub".to_string(),
        trust: "community".to_string(),
        popularity: 0.0,
    }
}

#[test]
fn split_market_row_separates_head_and_tail() {
    assert_eq!(
        split_market_row("demo", "1.2.3", "Gray Index", "does things"),
        (
            "demo 1.2.3 [Gray Index]".to_string(),
            " - does things".to_string()
        )
    );
    assert_eq!(
        split_market_row("demo", "1.2.3", "Gray Index", "  "),
        ("demo 1.2.3 [Gray Index]".to_string(), String::new())
    );
}

#[test]
fn source_chips_are_short_and_exact() {
    use gray_pkg::sources::Source as S;
    assert_eq!(source_chip(S::GrayIndex), "gray");
    assert_eq!(source_chip(S::PiGallery), "pi");
    assert_eq!(source_chip(S::ClawHub), "claw");
    assert_eq!(source_chip(S::ClaudeRepo), "claude");
}

#[test]
fn skill_chips_and_colors_follow_source_table() {
    assert_eq!(skill_chip("ClawHub"), "claw");
    assert_eq!(skill_chip("Claude"), "claude");
    assert_eq!(skill_chip("Other"), "Other");
    use ratatui::style::Color as C;
    let peach = C::Rgb(246, 173, 126);
    assert_eq!(chip_color("[gray]", peach), peach);
    assert_eq!(chip_color("[pi]", peach), C::Rgb(125, 211, 252));
    assert_eq!(chip_color("[claw]", peach), C::Rgb(134, 239, 172));
    assert_eq!(chip_color("[claude]", peach), C::Rgb(196, 181, 253));
}

#[test]
fn preview_shows_detail_trust_and_requires_when_known() {
    let mut hit = plugin_hit();
    hit.version_detail = "github:o/r@main".to_string();
    hit.trust = "official + scan:clean".to_string();
    let out = format_preview(&hit, &["git".to_string(), "rg".to_string()]);
    assert!(out.contains("demo 1.2.3 [Gray Index]"), "head: {out:?}");
    assert!(out.contains("does things"), "desc: {out:?}");
    assert!(out.contains("detail: github:o/r@main"), "detail: {out:?}");
    assert!(
        out.contains("trust: official + scan:clean"),
        "trust: {out:?}"
    );
    assert!(out.contains("requires: git, rg"), "requires: {out:?}");
    // Unknown fields stay omitted, never blank lines.
    let bare = format_preview(&plugin_hit(), &[]);
    assert_eq!(bare, "demo 1.2.3 [Gray Index]\ndoes things");
}

#[test]
fn skill_preview_is_skill_shaped_with_origin_note() {
    let out = format_skill_preview(&skill_hit(), "install: clawhub:arein/gifgrep");
    assert!(
        out.contains("arein/gifgrep 1.2.3 [ClawHub]"),
        "head: {out:?}"
    );
    assert!(out.contains("grep gifs"), "desc: {out:?}");
    assert!(out.contains("trust: community"), "trust: {out:?}");
    assert!(
        out.contains("install: clawhub:arein/gifgrep"),
        "origin: {out:?}"
    );
}

#[test]
fn source_row_covers_ok_unreachable_and_checking() {
    assert_eq!(
        format_source_row("Gray Index", Some(true)),
        "Gray Index: ok"
    );
    assert_eq!(
        format_source_row("ClawHub", Some(false)),
        "ClawHub: unreachable"
    );
    assert_eq!(format_source_row("Claude", None), "Claude: checking...");
}

#[test]
fn name_sort_orders_case_insensitive() {
    let mut plugins = vec![
        ("zebra".to_string(), "1.0.0".to_string()),
        ("Apple".to_string(), "2.0.0".to_string()),
        ("mango".to_string(), "0.1.0".to_string()),
    ]
    .into_iter()
    .map(|(name, version)| {
        let mut h = plugin_hit();
        h.name = name;
        h.version = version;
        h
    })
    .collect::<Vec<_>>();
    sort_plugins_by_name(&mut plugins);
    let names: Vec<&str> = plugins.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, vec!["Apple", "mango", "zebra"]);
    let mut skills = vec!["zebra", "Apple", "mango"]
        .into_iter()
        .map(|name| {
            let mut h = skill_hit();
            h.name = name.to_string();
            h
        })
        .collect::<Vec<_>>();
    sort_skills_by_name(&mut skills);
    let names: Vec<&str> = skills.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, vec!["Apple", "mango", "zebra"]);
}

#[test]
fn popularity_sort_orders_desc_then_name() {
    let hit = |name: &str, popularity: f32| {
        let mut h = plugin_hit();
        h.name = name.to_string();
        h.popularity = popularity;
        h
    };
    let all = vec![
        hit("mid", 0.5),
        hit("top", 0.9),
        hit("Zebra", 0.0),
        hit("apple", 0.0),
    ];
    let names = |v: &[SearchHit]| v.iter().map(|h| h.name.clone()).collect::<Vec<String>>();
    // Descending popularity; 0.0 ties break by lowercase name.
    assert_eq!(
        names(&apply_plugin_view(&all, None, SortMode::Popularity)),
        vec!["top", "mid", "apple", "Zebra"]
    );
}

#[test]
fn apply_plugin_view_filters_then_sorts() {
    use gray_pkg::sources::Source as S;
    let hit = |name: &str, source: S| {
        let mut h = plugin_hit();
        h.name = name.to_string();
        h.source = source;
        h
    };
    let all = vec![
        hit("zebra", S::PiGallery),
        hit("Apple", S::GrayIndex),
        hit("mango", S::PiGallery),
    ];
    let names = |v: &[SearchHit]| v.iter().map(|h| h.name.clone()).collect::<Vec<String>>();
    // Filter keeps receipt order.
    assert_eq!(
        names(&apply_plugin_view(
            &all,
            Some(S::PiGallery),
            SortMode::Relevance
        )),
        vec!["zebra", "mango"]
    );
    // Filter + sort.
    assert_eq!(
        names(&apply_plugin_view(&all, Some(S::PiGallery), SortMode::Name)),
        vec!["mango", "zebra"]
    );
    // No filter + sort.
    assert_eq!(
        names(&apply_plugin_view(&all, None, SortMode::Name)),
        vec!["Apple", "mango", "zebra"]
    );
}

#[test]
fn apply_skill_view_matches_label_sources() {
    let hit = |name: &str, source: &str| {
        let mut h = skill_hit();
        h.name = name.to_string();
        h.source = source.to_string();
        h
    };
    let all = vec![
        hit("zebra", "ClawHub"),
        hit("Apple", "Claude"),
        hit("mango", "ClawHub"),
    ];
    let names = |v: &[SkillHit]| v.iter().map(|h| h.name.clone()).collect::<Vec<String>>();
    use gray_pkg::sources::Source as S;
    assert_eq!(
        names(&apply_skill_view(&all, Some(S::ClawHub), SortMode::Name)),
        vec!["mango", "zebra"]
    );
    assert_eq!(
        names(&apply_skill_view(&all, None, SortMode::Relevance)),
        vec!["zebra", "Apple", "mango"]
    );
}

#[test]
fn filter_cycles_cover_tab_sources() {
    use gray_pkg::sources::Source as S;
    let mut f = None;
    for expect in [
        Some(S::GrayIndex),
        Some(S::PiGallery),
        Some(S::ClaudeRepo),
        Some(S::ClawHub),
        None,
    ] {
        f = next_plugin_filter(f);
        assert_eq!(f, expect);
    }
    let mut f = None;
    for expect in [Some(S::ClawHub), Some(S::ClaudeRepo), None] {
        f = next_skill_filter(f);
        assert_eq!(f, expect);
    }
    assert_eq!(filter_short(None), "all");
    assert_eq!(filter_short(Some(S::PiGallery)), "pi");
    assert_eq!(filter_short(Some(S::ClawHub)), "clawhub");
}

#[test]
fn scroll_top_follows_selection() {
    // Window of 5: sel inside stays, past bottom pushes, above pulls.
    assert_eq!(scroll_top(0, 0, 5), 0);
    assert_eq!(scroll_top(0, 4, 5), 0);
    assert_eq!(scroll_top(0, 5, 5), 1);
    assert_eq!(scroll_top(3, 9, 5), 5);
    assert_eq!(scroll_top(5, 2, 5), 2);
    // Reset (sel back to 0) rewinds to top.
    assert_eq!(scroll_top(7, 0, 5), 0);
    // Degenerate window keeps the offset.
    assert_eq!(scroll_top(3, 9, 0), 3);
}

#[test]
fn installed_summary_names_versions_or_hides_when_empty() {
    assert_eq!(installed_summary(&[]), None);
    assert_eq!(
        installed_summary(&[
            ("foo".to_string(), "1.0.0".to_string()),
            ("bar".to_string(), String::new()),
        ]),
        Some("Installed (2): foo 1.0.0, bar".to_string())
    );
}

#[test]
fn install_status_reports_active_or_failed_inline() {
    assert_eq!(format_install_status(None), "active");
    assert_eq!(format_install_status(Some("boom")), "failed: boom");
}

#[test]
fn installing_status_hides_only_when_footer_shows_it() {
    // Preview footer prints `installing...` mid-flight: status row stays
    // quiet (no duplicate). Everywhere else it is the only indicator.
    assert!(install_status_covered_by_footer("installing...", true));
    assert!(!install_status_covered_by_footer("installing...", false));
    assert!(!install_status_covered_by_footer("active", true));
    assert!(!install_status_covered_by_footer("failed: boom", true));
}

#[test]
fn install_specs_derive_from_source() {
    assert_eq!(install_spec_for_plugin(&plugin_hit()), "demo");
    let mut pi = plugin_hit();
    pi.source = Source::PiGallery;
    pi.name = "@scope/bar".to_string();
    assert_eq!(install_spec_for_plugin(&pi), "npm:@scope/bar");
    let mut ch = plugin_hit();
    ch.source = Source::ClawHub;
    ch.name = "arein/test".to_string();
    assert_eq!(install_spec_for_plugin(&ch), "clawhub:arein/test");
    let mut cl = plugin_hit();
    cl.source = Source::ClaudeRepo;
    cl.name = "grep-skills".to_string();
    assert_eq!(install_spec_for_plugin(&cl), "claude:grep-skills");
    assert_eq!(
        install_spec_for_skill(&skill_hit()),
        "clawhub:arein/gifgrep"
    );
    let mut cs = skill_hit();
    cs.source = "Claude".to_string();
    cs.name = "grep-skills".to_string();
    assert_eq!(install_spec_for_skill(&cs), "claude:grep-skills");
}

#[test]
fn market_tabs_wrap_at_both_ends() {
    assert_eq!(MarketTab::from_index(0), MarketTab::Plugins);
    assert_eq!(MarketTab::from_index(1), MarketTab::Skills);
    assert_eq!(MarketTab::from_index(2), MarketTab::Marketplaces);
    assert_eq!(MarketTab::Plugins.next(), MarketTab::Skills);
    assert_eq!(MarketTab::Skills.next(), MarketTab::Marketplaces);
    assert_eq!(MarketTab::Marketplaces.next(), MarketTab::Plugins);
    assert_eq!(MarketTab::Plugins.prev(), MarketTab::Marketplaces);
    assert_eq!(MarketTab::Marketplaces.prev(), MarketTab::Skills);
}
