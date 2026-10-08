//! The heartbeat: a periodic system wake into the `main` session that
//! follows `~/.gray/HEARTBEAT.md`. Cheap deterministic gates run first so
//! most beats cost nothing. See DESIGN.md §6.

use std::io::Write as _;
use std::path::Path;

use super::settings::{Settings, Window};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Wake the model with this prompt.
    Run(String),
    /// Not this time; the reason is logged (once per reason change).
    Skip(&'static str),
    /// Not due yet.
    NotDue,
}

/// Decide whether a heartbeat runs now.
/// `home` is GRAY_HOME (for HEARTBEAT.md), `dir` the gateway state dir (for
/// `heartbeat.json` last-run bookkeeping). `force` (from `heartbeat now`)
/// skips the interval and active-hours checks but not paused/busy.
pub fn gate(
    home: &Path,
    dir: &Path,
    settings: &Settings,
    now: i64,
    main_busy: bool,
    paused: bool,
    force: bool,
) -> Gate {
    let hb = &settings.heartbeat;
    let active = hb.active_hours.as_deref().and_then(|raw| {
        Window::parse(raw).or_else(|| {
            log::warn!("gateway: heartbeat.active_hours {raw:?} unreadable; ignoring it");
            None
        })
    });
    match verdict(&Inputs {
        enabled: hb.enabled,
        paused,
        force,
        last_at: last_run(dir),
        every_mins: hb.every_mins,
        active,
        minute_of_day: super::settings::local_minute(now),
        now,
        main_busy,
        checklist_empty: checklist_empty(home),
    }) {
        Verdict::NotDue => Gate::NotDue,
        Verdict::Skip(reason) => Gate::Skip(reason),
        Verdict::Go => Gate::Run(prompt(home, dir, now)),
    }
}

/// `Go` means wake the model; the rest explain why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Go,
    Skip(&'static str),
    NotDue,
}

/// Everything a gate decision depends on, resolved by the caller (file
/// reads, the local clock), so every branch is a one-line test.
#[derive(Debug, Default, Clone, Copy)]
struct Inputs {
    enabled: bool,
    paused: bool,
    force: bool,
    /// `heartbeat.json`'s `last_at`; 0 = never ran.
    last_at: i64,
    every_mins: u64,
    active: Option<Window>,
    minute_of_day: u32,
    now: i64,
    main_busy: bool,
    checklist_empty: bool,
}

/// The deterministic gate order, cheapest refusal first: the switch, the
/// estop, the schedule, the lane, then the checklist having any work.
fn verdict(i: &Inputs) -> Verdict {
    if !i.enabled {
        return Verdict::Skip("disabled");
    }
    if i.paused {
        return Verdict::Skip("paused");
    }
    let interval = i64::try_from(i.every_mins)
        .unwrap_or(i64::MAX)
        .saturating_mul(60);
    if !i.force && i.last_at.saturating_add(interval) > i.now {
        return Verdict::NotDue;
    }
    if !i.force && i.active.is_some_and(|w| !w.contains(i.minute_of_day)) {
        return Verdict::Skip("outside active hours");
    }
    if i.main_busy {
        return Verdict::Skip("main session busy");
    }
    if i.checklist_empty {
        return Verdict::Skip("HEARTBEAT.md is empty");
    }
    Verdict::Go
}

/// `dir/heartbeat.json`'s `last_at`; 0 when missing or unreadable.
fn last_run(dir: &Path) -> i64 {
    std::fs::read(dir.join("heartbeat.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("last_at")?.as_i64())
        .unwrap_or(0)
}

/// Record that a heartbeat turn was started at `now`.
pub fn mark_ran(dir: &Path, now: i64) {
    let row = serde_json::json!({"last_at": now});
    if let Err(e) = crate::cron::store::atomic_write_json(&dir.join("heartbeat.json"), &row) {
        log::warn!("gateway: cannot write heartbeat.json: {e:#}");
    }
}

/// True when `home/HEARTBEAT.md` has no work in it: missing, unreadable,
/// or nothing but blank lines, headings and HTML comments.
fn checklist_empty(home: &Path) -> bool {
    match std::fs::read_to_string(home.join("HEARTBEAT.md")) {
        Ok(text) => effectively_empty(&text),
        Err(_) => true,
    }
}

/// Pure checklist check: HTML comments (which may span lines) are stripped,
/// then every remaining line must be blank or a `#` heading for the file
/// to count as empty.
fn effectively_empty(text: &str) -> bool {
    let mut rest = text;
    let mut visible = String::with_capacity(text.len());
    while let Some(start) = rest.find("<!--") {
        visible.push_str(&rest[..start]);
        rest = match rest[start + 4..].find("-->") {
            Some(end) => &rest[start + 4 + end + 3..],
            // Unterminated comment: the rest of the file is comment.
            None => "",
        };
    }
    visible.push_str(rest);
    visible
        .lines()
        .all(|l| l.trim().is_empty() || l.trim_start().starts_with('#'))
}

/// The heartbeat turn prompt, deliberately quiet (OpenClaw): follow the
/// checklist, speak only when the owner needs to hear it.
fn prompt(home: &Path, dir: &Path, now: i64) -> String {
    let when = chrono::DateTime::from_timestamp(now, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%a %Y-%m-%d %H:%M %:z")
                .to_string()
        })
        .unwrap_or_default();
    let checklist = std::fs::read_to_string(home.join("HEARTBEAT.md")).unwrap_or_default();
    let said = already_said(dir, now);
    let said = if said.is_empty() {
        String::new()
    } else {
        format!("\n\n# Already told the owner (last 24h)\n\n{said}")
    };
    format!(
        "[gateway · heartbeat · {when}] This is a scheduled heartbeat, not a message from \
         the owner. Work through the checklist below; do not invent tasks and do not repeat \
         what you already told the owner unless it changed. If nothing needs the owner's \
         attention, reply exactly NO_REPLY. Otherwise write the message to the owner \
         directly.\n\n# HEARTBEAT.md\n\n{}{said}",
        super::event::cap(&checklist, 8 * 1024)
    )
}

/// The last few heartbeat messages queued for the owner in the past day.
/// Each heartbeat is a fresh session, so this is its only memory of them.
fn already_said(dir: &Path, now: i64) -> String {
    let said: Vec<String> = super::activity::tail(dir, 500)
        .into_iter()
        .filter(|r| r["what"] == "queued" && r["kind"] == "heartbeat")
        .filter(|r| r["at"].as_i64().is_some_and(|at| now - at < 86_400))
        .filter_map(|r| Some(format!("- {}", r["text"].as_str()?)))
        .collect();
    said[said.len().saturating_sub(3)..].join("\n")
}

/// The template `run` drops on first start.
const TEMPLATE: &str = "\
# Heartbeat checklist

<!--
The periodic heartbeat works through this file while you are away.
Headings and comments do not count as items: while every item below is
commented out, the heartbeat skips without waking the model.

Uncomment or add lines to give it work, for example:

- check my calendar for meetings in the next 2 hours and remind me
- if a CI run on my repos failed, tell me
- if a background task finished and nobody was told, say so
-->
";

/// Write the starter HEARTBEAT.md when none exists — a template that is
/// still *effectively empty* (heading + commented examples) so a fresh
/// install's heartbeats skip instead of waking the model for nothing.
/// `create_new` never overwrites an owner's file.
pub fn ensure_template(home: &Path) {
    let _ = std::fs::create_dir_all(home);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(home.join("HEARTBEAT.md"))
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(TEMPLATE.as_bytes()) {
                log::warn!("gateway: cannot write HEARTBEAT.md: {e:#}");
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => log::warn!("gateway: cannot create HEARTBEAT.md: {e:#}"),
    }
}

#[path = "heartbeat_tests.rs"]
#[cfg(test)]
mod tests;
