// UNRUN (cargo test banned under X): run in TTY/CI.
use super::*;

fn job(name: &str, status: Option<crate::cron::RunStatus>) -> crate::cron::CronJob {
    crate::cron::CronJob {
        id: "abc123def456".to_string(),
        name: name.to_string(),
        prompt: "p".to_string(),
        schedule: crate::cron::Schedule::Interval { secs: 3600 },
        enabled: true,
        state: Default::default(),
        created_at: 1,
        next_run_at: Some(1_700_000_000),
        last_run_at: None,
        last_status: status,
        last_error: None,
        last_delivery_error: None,
        deliver: Default::default(),
        origin: None,
        workdir: None,
        fire_claim: None,
        reminder: false,
        skills: vec![],
        script: None,
    }
}

#[test]
fn dashboard_empty() {
    assert!(format_cron_dashboard(&[], None, 1_700_000_000).contains("no cron jobs"));
}

#[test]
fn dashboard_row_shapes() {
    let out = format_cron_dashboard(
        &[
            job("hourly", Some(crate::cron::RunStatus::Ok)),
            job("nightly", Some(crate::cron::RunStatus::Error)),
        ],
        None,
        1_700_000_000,
    );
    assert!(out.contains("hourly"));
    assert!(out.contains("abc123de"));
    assert!(out.contains("Interval"));
    assert!(out.contains("1700000000"));
    assert!(out.contains("Ok"));
    assert!(out.contains("nightly"));
    assert!(out.contains("Error"));
    assert!(out.contains("fire automatically in this session"));
}

#[test]
fn dashboard_reports_ticker_liveness() {
    let health = crate::cron::CronHealth {
        last_tick: None,
        overdue: vec![],
    };
    let out = format_cron_dashboard(&[job("nightly", None)], Some(&health), 1_700_000_000);
    assert!(out.contains("nightly"));
    assert!(out.contains("no tick has ever run"), "{out}");
    assert!(!out.contains("fire automatically in this session"), "{out}");
}

fn paused(name: &str) -> crate::cron::CronJob {
    crate::cron::CronJob {
        state: crate::cron::store::JobState::Paused,
        ..job(name, None)
    }
}

#[test]
fn picker_rows_carry_the_dashboard_fields() {
    let rows = items(
        &[job("hourly", Some(crate::cron::RunStatus::Ok))],
        None,
        1_700_000_000,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "abc123def456");
    assert!(
        rows[0].row.starts_with("\u{2713} hourly (abc123de)"),
        "{}",
        rows[0].row
    );
    assert!(rows[0].row.contains("next 1700000000"), "{}", rows[0].row);
    assert!(rows[0].row.contains("last Ok"), "{}", rows[0].row);
    assert!(rows[0].enabled);
    assert!(!rows[0].read_only);
}

#[test]
fn picker_marks_paused_jobs_dim_and_unticked() {
    let rows = items(&[paused("nightly")], None, 1_700_000_000);
    assert!(rows[0].row.starts_with("\u{25cb}"), "{}", rows[0].row);
    assert!(rows[0].row.ends_with("[paused]"), "{}", rows[0].row);
    assert!(!rows[0].lit);
    assert!(!rows[0].enabled);
}

#[test]
fn picker_appends_the_ticker_liveness_row() {
    let health = crate::cron::CronHealth {
        last_tick: None,
        overdue: vec![],
    };
    let rows = items(&[job("nightly", None)], Some(&health), 1_700_000_000);
    assert_eq!(rows.len(), 2);
    assert!(
        rows[1].row.contains("no tick has ever run"),
        "{}",
        rows[1].row
    );
    assert!(rows[1].read_only);
}

fn finished(name: &str) -> crate::cron::CronJob {
    crate::cron::CronJob {
        state: crate::cron::store::JobState::Done,
        ..job(name, None)
    }
}

fn disabled(name: &str) -> crate::cron::CronJob {
    crate::cron::CronJob {
        enabled: false,
        ..job(name, None)
    }
}

#[test]
fn picker_offers_no_switch_on_states_a_toggle_cannot_change() {
    // claim_due fires only Active *and* enabled: a finished one-shot and a
    // disabled job would change state without changing whether they run, so
    // both render read-only and tagged.
    for (built, tag) in [
        (finished as fn(&str) -> crate::cron::CronJob, "[done]"),
        (disabled, "[disabled]"),
    ] {
        let rows = items(&[built("nightly")], None, 1_700_000_000);
        assert!(rows[0].row.ends_with(tag), "{}", rows[0].row);
        assert!(rows[0].read_only, "{}", rows[0].row);
        assert!(!rows[0].lit, "{}", rows[0].row);
    }
}

#[test]
fn picker_hides_the_ticker_row_when_there_are_no_jobs() {
    // Otherwise the ticker line crowds out the add-a-job hint on a new store.
    let health = crate::cron::CronHealth {
        last_tick: None,
        overdue: vec![],
    };
    assert!(items(&[], Some(&health), 1_700_000_000).is_empty());
    assert_eq!(
        items(&[job("nightly", None)], Some(&health), 1_700_000_000).len(),
        2
    );
}

/// Scripted executor: each wait answers the next scripted result (`None` =
/// the slice timed out); `pending` says whether jobs are still running.
struct ScriptedWaits {
    answers: std::sync::Mutex<Vec<Option<()>>>,
    pending: std::sync::atomic::AtomicBool,
    waits: std::sync::atomic::AtomicUsize,
}

impl gray_core::agent::ToolExecutor for ScriptedWaits {
    fn wait_for_notification(
        &self,
        _ctx: &gray_core::agent::ToolContext,
        _timeout: std::time::Duration,
    ) -> futures::future::BoxFuture<'static, Option<()>> {
        self.waits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let answer = self.answers.lock().unwrap().remove(0);
        Box::pin(std::future::ready(answer))
    }

    fn has_pending_background(&self, _ctx: &gray_core::agent::ToolContext) -> bool {
        self.pending.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn execute(
        &self,
        _ctx: &gray_core::agent::ToolContext,
        _name: &str,
        _args: serde_json::Value,
    ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
        unreachable!("the waker never runs tools")
    }
}

fn scripted(answers: Vec<Option<()>>, pending: bool) -> std::sync::Arc<ScriptedWaits> {
    std::sync::Arc::new(ScriptedWaits {
        answers: std::sync::Mutex::new(answers),
        pending: std::sync::atomic::AtomicBool::new(pending),
        waits: std::sync::atomic::AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn a_job_longer_than_one_wait_slice_still_wakes_the_prompt() {
    // The old waiter gave up after one 24h wait; a longer job never woke.
    let exec = scripted(vec![None, None, Some(())], true);
    wait_then_wake(
        exec.clone(),
        gray_core::agent::ToolContext::default(),
        std::time::Duration::from_millis(1),
    )
    .await;
    assert_eq!(exec.waits.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert!(crate::host::wake_requested());
}

#[tokio::test]
async fn the_waiter_stops_once_nothing_is_pending() {
    let exec = scripted(vec![None, Some(())], false);
    wait_then_wake(
        exec.clone(),
        gray_core::agent::ToolContext::default(),
        std::time::Duration::from_millis(1),
    )
    .await;
    assert_eq!(
        exec.waits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a timed-out wait with no jobs left ends the waiter"
    );
}

fn card_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn cron_card_header_names_job_status_and_time() {
    let card = crate::cron_serve::CronCard {
        name: "sandbox-probe".into(),
        body: "Done \u{2014} wrote `~/.gray/sandbox-probe.txt`.".into(),
        failed: false,
        reminder: false,
        elapsed_ms: 18_400,
    };
    let (header, body) = cron_card_lines(&card, 80);
    assert_eq!(
        card_text(&header),
        "\u{2b22} Cron sandbox-probe \u{b7} done \u{b7} 18.4s"
    );
    let joined: String = body.iter().map(card_text).collect::<Vec<_>>().join("\n");
    assert!(joined.contains("sandbox-probe.txt"), "{joined}");
    assert!(!joined.contains("[tool:"), "{joined}");
    assert!(
        body.iter().all(|l| card_text(l).starts_with("  ")),
        "{joined}"
    );
}

#[test]
fn cron_card_reminder_and_failure_headers() {
    let reminder = crate::cron_serve::CronCard {
        name: "clean-my-room".into(),
        body: "clean my room".into(),
        failed: false,
        reminder: true,
        elapsed_ms: 0,
    };
    let (header, body) = cron_card_lines(&reminder, 80);
    assert_eq!(card_text(&header), "\u{2b22} Reminder");
    assert_eq!(card_text(&body[0]), "  clean my room");

    let failed = crate::cron_serve::CronCard {
        name: "nightly".into(),
        body: "provider timed out".into(),
        failed: true,
        reminder: false,
        elapsed_ms: 2_000,
    };
    let (header, _) = cron_card_lines(&failed, 80);
    assert_eq!(
        card_text(&header),
        "\u{2b22} Cron nightly \u{b7} failed \u{b7} 2s"
    );
}

#[test]
fn job_notice_becomes_a_card_without_the_model_hint() {
    let notice = "Background job bwrap-5 finished (exit 0) after 4m26s \u{b7} log /tmp/x/bash-9.log\nRead its output with `tail` (or `grep`) on that log.";
    let job = parse_job_notice(notice).expect("parses");
    assert_eq!(job.id, "bwrap-5");
    assert_eq!(job.outcome, "exit 0");
    assert_eq!(job.elapsed, "4m26s");
    assert_eq!(job.log, "/tmp/x/bash-9.log");
    let (header, body) = job_card_lines(&job);
    assert_eq!(
        card_text(&header),
        "\u{2b22} Background job bwrap-5 \u{b7} exit 0 \u{b7} 4m26s"
    );
    assert_eq!(body.len(), 1);
    assert_eq!(card_text(&body[0]), "  log /tmp/x/bash-9.log");
    assert!(parse_job_notice("something else").is_none());
}
