use super::*;

fn route(platform: &str) -> Route {
    Route {
        platform: platform.into(),
        chat: "42".into(),
        thread: None,
        route: Some("42".into()),
    }
}

#[test]
fn user_silence_is_strict_autonomous_is_forgiving() {
    assert!(is_silent("NO_REPLY", Kind::User));
    assert!(is_silent("  ", Kind::User));
    assert!(!is_silent("NO_REPLY, nothing to add", Kind::User));
    assert!(!is_silent("all good\nNO_REPLY", Kind::User));
    assert!(is_silent("all good\n**NO_REPLY**", Kind::Trigger));
    assert!(is_silent("`[SILENT]`\nchecked CI", Kind::Trigger));
    assert!(!is_silent("CI failed on main", Kind::Trigger));
    assert!(is_silent("Checked everything.\nHEARTBEAT_OK", Kind::Trigger));
}

#[test]
fn replies_fall_back_to_last_route_then_owner_then_local() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Settings::default();
    let platform = |d: Decision| match d {
        Decision::Deliver(i) => i.platform,
        Decision::Suppressed(r) => format!("suppressed:{r}"),
    };
    let go = |s: &Settings, r: Option<Route>, last: Option<Route>| {
        platform(decide(
            dir.path(),
            s,
            "main",
            Kind::Trigger,
            r,
            last,
            "hi",
        ))
    };
    assert_eq!(go(&s, None, None), "local");
    s.owners.push(route("discord"));
    assert_eq!(go(&s, None, None), "discord");
    assert_eq!(go(&s, None, Some(route("telegram"))), "telegram");
    assert_eq!(
        go(&s, Some(route("slack")), Some(route("telegram"))),
        "slack"
    );
    let silent = decide(
        dir.path(),
        &s,
        "main",
        Kind::Trigger,
        None,
        None,
        "NO_REPLY",
    );
    assert_eq!(silent, Decision::Suppressed("silent"));
    assert_eq!(
        super::super::activity::tail(dir.path(), 1)[0]["what"],
        "suppressed"
    );
}
