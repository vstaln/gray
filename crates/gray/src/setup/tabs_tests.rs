use super::*;

#[test]
fn segments_mark_exactly_one_active() {
    let tabs = [("Installed", None), ("Errors", Some(2))];
    for active in 0..tabs.len() {
        let segs = tab_segments(&tabs, active);
        assert_eq!(segs.len(), tabs.len());
        for (i, seg) in segs.iter().enumerate() {
            assert_eq!(seg.active, i == active, "active={active} i={i}");
        }
    }
}

#[test]
fn next_prev_wrap_at_both_ends() {
    assert_eq!(next_tab(0, 2), 1);
    assert_eq!(next_tab(1, 2), 0);
    assert_eq!(prev_tab(0, 2), 1);
    assert_eq!(prev_tab(1, 2), 0);
}

#[test]
fn count_badge_renders_only_when_some() {
    let segs = tab_segments(&[("Installed", None), ("Errors", Some(2))], 0);
    assert_eq!(segs[0].text, "Installed");
    assert_eq!(segs[1].text, "Errors (2)");
    let segs = tab_segments(&[("Installed", None), ("Errors", None)], 1);
    assert_eq!(segs[0].text, "Installed");
    assert_eq!(segs[1].text, "Errors");
}

#[test]
fn tab_enum_round_trips_through_index() {
    assert_eq!(Tab::from_index(0), Tab::Installed);
    assert_eq!(Tab::from_index(1), Tab::Errors);
    assert_eq!(Tab::Installed.next(), Tab::Errors);
    assert_eq!(Tab::Errors.next(), Tab::Installed);
    assert_eq!(Tab::Installed.prev(), Tab::Errors);
    assert_eq!(Tab::Errors.prev(), Tab::Installed);
}
