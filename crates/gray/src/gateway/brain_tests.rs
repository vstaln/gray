use super::*;
use super::event::MAIN;
use crate::gateway::turn::StubRunner;

fn stub(reply: &'static str) -> Arc<dyn TurnRunner> {
    Arc::new(StubRunner {
        reply: Arc::new(move |req: &TurnRequest| TurnOutcome {
            session_id: Some(format!("sid-{}", req.key.replace(':', "-"))),
            text: format!("{reply}: {}", req.prompt),
            error: None,
        }),
    })
}

fn discord(chat: &str) -> Route {
    Route {
        platform: "discord".into(),
        chat: chat.into(),
        thread: None,
        route: Some(chat.into()),
    }
}


async fn drain(brain: &mut Brain, rx: &mut tokio::sync::mpsc::UnboundedReceiver<Done>) {
    while brain.busy_keys() > 0 {
        let done = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("turn finished")
            .expect("channel open");
        brain.finish(done);
    }
}

#[tokio::test]
async fn a_user_message_becomes_one_turn_and_one_reply() {
    let home = tempfile::tempdir().unwrap();
    let mut brain = Brain::new(home.path(), stub("echo"));
    let mut rx = brain.take_done();
    let ev = Event::new(Kind::User, "chat:discord:42", "hi", Some(discord("42")));
    event::admit(&brain.dir, &ev).unwrap();
    brain.step(1_000);
    assert_eq!(brain.busy_keys(), 1);
    assert!(
        event::pending(&brain.dir).is_empty(),
        "event moved to running/"
    );
    assert_eq!(event::spool_files(&brain.dir.join("running")).len(), 1);
    drain(&mut brain, &mut rx).await;
    assert!(event::spool_files(&brain.dir.join("running")).is_empty());
    let sessions = Sessions::load(&brain.dir);
    assert_eq!(
        sessions.session_id("chat:discord:42").as_deref(),
        Some("sid-chat-discord-42")
    );
    let out = crate::gateway::outbox::pull(&brain.dir, "discord", i64::MAX / 2, 10);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "echo: hi");
    assert_eq!(out[0].route.as_ref().unwrap().chat, "42");
}

#[tokio::test]
async fn waiting_events_for_one_key_batch_and_the_cap_holds() {
    let home = tempfile::tempdir().unwrap();
    let mut brain = Brain::new(home.path(), stub("ok"));
    let mut rx = brain.take_done();
    for (key, text) in [
        ("main", "one"),
        ("main", "two"),
        ("chat:x:1", "a"),
        ("chat:x:2", "b"),
    ] {
        event::admit(&brain.dir, &Event::new(Kind::User, key, text, None)).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    brain.step(1_000);
    // Cap 2: main (both messages, one turn) + the next oldest key.
    assert_eq!(brain.busy_keys(), 2);
    assert_eq!(event::pending(&brain.dir).len(), 1);
    drain(&mut brain, &mut rx).await;
    brain.step(1_001);
    drain(&mut brain, &mut rx).await;
    assert!(event::pending(&brain.dir).is_empty());
    let texts: Vec<String> = crate::gateway::outbox::pull(&brain.dir, "local", i64::MAX / 2, 10)
        .into_iter()
        .map(|i| i.text)
        .collect();
    assert_eq!(texts.len(), 3, "{texts:?}");
    let main = texts.iter().find(|t| t.contains("one")).unwrap();
    assert!(
        main.contains("two") && main.contains("2 messages arrived"),
        "{main}"
    );
}

#[tokio::test]
async fn paused_holds_autonomous_events_but_not_the_owner() {
    let home = tempfile::tempdir().unwrap();
    let mut brain = Brain::new(home.path(), stub("ok"));
    let mut rx = brain.take_done();
    std::fs::write(brain.dir.join("PAUSED"), "").unwrap();
    event::admit(
        &brain.dir,
        &Event::new(Kind::Trigger, "job:t", "ci failed", None),
    )
    .unwrap();
    event::admit(
        &brain.dir,
        &Event::new(Kind::User, "chat:x:1", "hello", None),
    )
    .unwrap();
    brain.step(1_000);
    assert_eq!(brain.busy_keys(), 1);
    drain(&mut brain, &mut rx).await;
    let left = event::pending(&brain.dir);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].1.kind, Kind::Trigger);
}

#[tokio::test]
async fn a_failed_user_turn_tells_the_user() {
    let home = tempfile::tempdir().unwrap();
    let runner: Arc<dyn TurnRunner> = Arc::new(StubRunner {
        reply: Arc::new(|_| TurnOutcome {
            session_id: None,
            text: String::new(),
            error: Some("provider 500".into()),
        }),
    });
    let mut brain = Brain::new(home.path(), runner);
    let mut rx = brain.take_done();
    event::admit(&brain.dir, &Event::new(Kind::User, MAIN, "hi", None)).unwrap();
    brain.step(1_000);
    drain(&mut brain, &mut rx).await;
    let out = crate::gateway::outbox::pull(&brain.dir, "local", i64::MAX / 2, 10);
    assert_eq!(out.len(), 1);
    assert!(out[0].text.contains("provider 500"));
}

#[test]
fn recovery_readmits_then_gives_up_and_says_so() {
    let home = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let _g = rt.enter();
    let mut brain = Brain::new(home.path(), stub("x"));
    let fresh = Event::new(Kind::User, MAIN, "build the thing", None);
    let mut tired = Event::new(Kind::User, "chat:discord:9", "again", Some(discord("9")));
    tired.attempt = MAX_RECOVERY;
    let record = Running {
        turn: "t1".into(),
        key: MAIN.into(),
        events: vec![fresh.clone(), tired],
        started_at: 1,
    };
    crate::cron::store::atomic_write_json(&brain.dir.join("running/t1.json"), &record).unwrap();
    brain.recover();
    assert!(event::spool_files(&brain.dir.join("running")).is_empty());
    let pending = event::pending(&brain.dir);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].1.id, fresh.id);
    assert_eq!(pending[0].1.attempt, 1);
    let notice = crate::gateway::outbox::pull(&brain.dir, "discord", i64::MAX / 2, 10);
    assert_eq!(notice.len(), 1);
    assert!(notice[0].text.contains("stopped retrying"));
}

#[test]
fn prompts_say_why_the_agent_woke() {
    let user = Event::new(Kind::User, MAIN, "hey", None);
    assert_eq!(compose_prompt(std::slice::from_ref(&user), 0), "hey");
    let trig = Event::new(Kind::Trigger, MAIN, "Trigger `ci`:\nred", None);
    let p = compose_prompt(&[trig], 0);
    assert!(
        p.contains("Not a message from the owner") && p.contains("NO_REPLY"),
        "{p}"
    );
    let mut again = user.clone();
    again.attempt = 1;
    assert!(compose_prompt(&[again], 0).contains("cut short by a restart"));
    assert_eq!(
        turn_kind(&[Event::new(Kind::Trigger, MAIN, "x", None), user]),
        Kind::User
    );
}

#[tokio::test]
async fn autonomous_turns_neither_resume_nor_replace_the_session() {
    let home = tempfile::tempdir().unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let runner: Arc<dyn TurnRunner> = Arc::new(StubRunner {
        reply: Arc::new(move |req: &TurnRequest| {
            log.lock().unwrap().push(req.session_id.clone());
            TurnOutcome {
                session_id: Some(format!("sid-{}", req.kind.as_str())),
                text: "NO_REPLY".into(),
                error: None,
            }
        }),
    });
    let mut brain = Brain::new(home.path(), runner);
    let mut rx = brain.take_done();
    for (i, kind) in [Kind::User, Kind::Trigger, Kind::User]
        .into_iter()
        .enumerate()
    {
        event::admit(&brain.dir, &Event::new(kind, MAIN, "x", None)).unwrap();
        brain.step(1_000 + i as i64);
        drain(&mut brain, &mut rx).await;
    }
    let user = Some(format!("sid-{}", Kind::User.as_str()));
    assert_eq!(*seen.lock().unwrap(), vec![None, None, user]);
}
