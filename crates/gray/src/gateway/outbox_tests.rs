use super::*;

fn discord() -> Route {
    Route {
        platform: "discord".into(),
        chat: "42".into(),
        thread: None,
        route: Some("42".into()),
    }
}

#[test]
fn pull_leases_and_ack_removes() {
    let dir = tempfile::tempdir().unwrap();
    let i = Intent::new("main", Kind::User, Some(discord()), "hello");
    enqueue(dir.path(), &i).unwrap();
    let got = pull(dir.path(), "discord", 100, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].attempts, 1);
    // Leased: a second pull inside the lease sees nothing.
    assert!(pull(dir.path(), "discord", 100 + LEASE_SECS - 1, 10).is_empty());
    // Other platforms never see it.
    assert!(pull(dir.path(), "telegram", 1000, 10).is_empty());
    assert_eq!(ack(dir.path(), std::slice::from_ref(&i.id)), 1);
    assert!(pull(dir.path(), "discord", 10_000, 10).is_empty());
}

#[test]
fn an_unacked_intent_comes_back_then_dies() {
    let dir = tempfile::tempdir().unwrap();
    enqueue(
        dir.path(),
        &Intent::new("main", Kind::Trigger, Some(discord()), "x"),
    )
    .unwrap();
    let mut now = 0;
    for attempt in 1..=MAX_ATTEMPTS {
        let got = pull(dir.path(), "discord", now, 10);
        assert_eq!(got.len(), 1, "attempt {attempt}");
        now += LEASE_SECS + 1;
    }
    assert!(pull(dir.path(), "discord", now, 10).is_empty());
    assert_eq!(
        super::super::event::spool_files(&dir.path().join("outbox/dead")).len(),
        1
    );
    let log = super::super::activity::tail(dir.path(), 5);
    assert_eq!(log.last().unwrap()["what"], "undelivered");
}
