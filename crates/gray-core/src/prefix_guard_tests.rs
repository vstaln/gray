use crate::message::{ChatRequest, Message, ToolDef};
use crate::prefix_guard::describe;
use crate::prefix_guard::{Change, PrefixGuard, diff, fingerprint};

fn req(system: &str, msgs: &[&str]) -> ChatRequest {
    ChatRequest {
        system: (!system.is_empty()).then(|| system.to_string()),
        messages: msgs.iter().map(|m| Message::user(*m)).collect(),
        tools: Vec::new(),
        max_tokens: None,
    }
}

#[test]
fn appending_is_not_a_change() {
    let a = fingerprint("m", &req("sys", &["hi"]));
    let b = fingerprint("m", &req("sys", &["hi", "more"]));
    assert_eq!(diff(&a, &b), None);
    assert_eq!(diff(&a, &a), None);
}

#[test]
fn the_first_differing_component_wins_in_prefix_order() {
    let base = fingerprint("m", &req("sys", &["a", "b", "c"]));
    assert_eq!(
        diff(&base, &fingerprint("m-max", &req("other", &["x"]))),
        Some(Change::Provider)
    );
    assert_eq!(
        diff(
            &base,
            &fingerprint("m", &req("sys /tmp/xyz", &["a", "b", "c"]))
        ),
        Some(Change::System)
    );
    let mut tools = req("sys", &["a", "b", "c"]);
    tools.tools.push(ToolDef {
        name: "bash".into(),
        description: String::new(),
        parameters: serde_json::json!({}),
        label: Default::default(),
        preview: Default::default(),
    });
    assert_eq!(diff(&base, &fingerprint("m", &tools)), Some(Change::Tools));
    assert_eq!(
        diff(&base, &fingerprint("m", &req("sys", &["a", "B", "c", "d"]))),
        Some(Change::HistoryAt(1))
    );
    assert_eq!(
        diff(&base, &fingerprint("m", &req("sys", &["a"]))),
        Some(Change::HistoryTruncated { from: 3, to: 1 })
    );
}

#[test]
fn guard_reports_unannounced_changes_and_consumes_an_announcement() {
    let mut g = PrefixGuard::default();
    assert_eq!(g.observe("m", &req("sys", &["a"])), None);
    assert_eq!(g.observe("m", &req("sys", &["a", "b"])), None);
    assert_eq!(
        g.observe("m", &req("sys", &["a", "B"])),
        Some((Change::HistoryAt(1), None))
    );
    g.expect_change("compaction");
    assert_eq!(
        g.observe("m", &req("sys", &["summary"])),
        Some((Change::HistoryAt(0), Some("compaction")))
    );

    assert_eq!(g.observe("m", &req("sys", &["summary", "c"])), None);
    assert_eq!(
        g.observe("m", &req("sys2", &["summary", "c"])),
        Some((Change::System, None))
    );
}

#[test]
fn change_displays_as_a_short_locator() {
    assert_eq!(Change::HistoryAt(12).to_string(), "history@12");
    assert_eq!(
        Change::HistoryTruncated { from: 9, to: 2 }.to_string(),
        "history truncated 9->2"
    );
}

#[test]
fn describe_names_known_reasons_and_skips_tail_growth() {
    // A change at the history tail is normal growth: it cannot cost a miss
    // worth a notice.
    assert_eq!(describe(Change::HistoryAt(3), None, 4), None);
    assert_eq!(
        describe(Change::HistoryAt(1), None, 4).as_deref(),
        Some("history changed at message 2")
    );
    assert_eq!(
        describe(Change::HistoryAt(0), Some("media payload budget"), 9).as_deref(),
        Some("older images dropped from context")
    );
    assert_eq!(
        describe(Change::HistoryAt(0), Some("history rewrite"), 9).as_deref(),
        Some("history rewritten")
    );
    assert_eq!(
        describe(Change::Tools, None, 4).as_deref(),
        Some("tool list changed")
    );
    assert_eq!(
        describe(Change::System, None, 4).as_deref(),
        Some("system prompt changed")
    );
    assert_eq!(
        describe(Change::HistoryTruncated { from: 9, to: 2 }, None, 9).as_deref(),
        Some("history truncated")
    );
}
