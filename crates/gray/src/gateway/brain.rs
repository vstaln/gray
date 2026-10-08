//! The brain: the gateway's event loop. Events in, turns out, replies to
//! the outbox. See `docs/always-on/DESIGN.md`.
//!
//! One [`Brain`] owns the lanes: at most one turn per session key, at most
//! `max_turns` at once. Every event that arrives for a busy key waits, and
//! when the key frees all of its waiting events run as one turn (OpenClaw
//! `collect`). Before a turn starts, its events move from `events/` into a
//! `running/<turn>.json` admission record; the record is removed when the
//! turn's reply has been handed to delivery. A record still there at boot is
//! a turn a crash or restart cut short: it is re-admitted (bounded) instead
//! of lost.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::deliver;
use super::event::{self, Event, Kind, MAIN};
use super::heartbeat;
use super::sessions::Sessions;
use super::settings::Settings;
use super::turn::{TurnOutcome, TurnRequest, TurnRunner};
use crate::cron::store::Origin as Route;

/// Times a cut-short turn is retried before the owner is told instead.
pub const MAX_RECOVERY: u32 = 3;

/// Admission record for a turn in flight.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Running {
    pub turn: String,
    pub key: String,
    pub events: Vec<Event>,
    pub started_at: i64,
}

/// A finished turn, sent back from its task to the loop.
#[derive(Debug)]
pub struct Done {
    pub turn: String,
    pub key: String,
    pub kind: Kind,
    pub route: Option<Route>,
    pub outcome: TurnOutcome,
}

pub struct Brain {
    pub home: PathBuf,
    pub dir: PathBuf,
    pub settings: Settings,
    sessions: Sessions,
    runner: Arc<dyn TurnRunner>,
    /// Session key → turn id (and its task, aborted on shutdown).
    busy: HashMap<String, (String, tokio::task::JoinHandle<()>)>,
    done_tx: tokio::sync::mpsc::UnboundedSender<Done>,
    done_rx: Option<tokio::sync::mpsc::UnboundedReceiver<Done>>,
    /// Last heartbeat skip reason, so a skip is logged once, not every poll.
    last_skip: Option<&'static str>,
}

impl Brain {
    pub fn new(home: &Path, runner: Arc<dyn TurnRunner>) -> Self {
        let dir = super::state_dir(home);
        for sub in ["events", "running", "outbox"] {
            let _ = std::fs::create_dir_all(dir.join(sub));
        }
        let (done_tx, done_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            home: home.to_path_buf(),
            settings: Settings::load(&dir),
            sessions: Sessions::load(&dir),
            dir,
            runner,
            busy: HashMap::new(),
            done_tx,
            done_rx: Some(done_rx),
            last_skip: None,
        }
    }

    /// The completion channel; the loop owns it so `finish` can borrow
    /// the brain mutably.
    pub fn take_done(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<Done> {
        self.done_rx.take().expect("take_done called once")
    }

    pub fn busy_keys(&self) -> usize {
        self.busy.len()
    }

    /// Boot: every `running/` record is a turn that never finished. Its
    /// events go back in the queue with `attempt + 1`; past
    /// [`MAX_RECOVERY`] they are dropped and the owner is told.
    pub fn recover(&mut self) {
        for path in event::spool_files(&self.dir.join("running")) {
            let record = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Running>(&b).ok());
            let _ = std::fs::remove_file(&path);
            let Some(record) = record else { continue };
            for mut ev in record.events {
                ev.attempt += 1;
                if ev.attempt > MAX_RECOVERY {
                    super::activity::log(
                        &self.dir,
                        "abandoned",
                        serde_json::json!({"key": ev.key, "kind": ev.kind.as_str(),
                            "attempts": ev.attempt - 1, "text": excerpt(&ev.text, 200)}),
                    );
                    if !ev.kind.autonomous() {
                        let note = format!(
                            "I was interrupted {} times while working on your message and stopped retrying it:\n> {}",
                            ev.attempt - 1,
                            excerpt(&ev.text, 300)
                        );
                        let last = self.sessions.last_route(&ev.key);
                        deliver::decide(
                            &self.dir,
                            &self.settings,
                            &ev.key,
                            Kind::System,
                            ev.route.clone(),
                            last,
                            &note,
                        );
                    }
                    continue;
                }
                super::activity::log(
                    &self.dir,
                    "recovered",
                    serde_json::json!({"key": ev.key, "kind": ev.kind.as_str(), "attempt": ev.attempt}),
                );
                if let Err(e) = event::admit(&self.dir, &ev) {
                    log::warn!("gateway: cannot re-admit interrupted event: {e:#}");
                }
            }
        }
    }

    /// One pass: maybe beat, start every turn that can start. Cheap enough
    /// to run on every poll.
    pub fn step(&mut self, now: i64) {
        self.settings = Settings::load(&self.dir);
        self.maybe_heartbeat(now);
        self.dispatch(now);
    }

    fn maybe_heartbeat(&mut self, now: i64) {
        let force_file = self.dir.join("heartbeat.force");
        let force = force_file.exists();
        let main_pending = event::pending(&self.dir)
            .iter()
            .any(|(_, e)| e.key == MAIN && e.kind == Kind::Heartbeat);
        let main_busy = self.busy.contains_key(MAIN) || main_pending;
        let gate = heartbeat::gate(
            &self.home,
            &self.dir,
            &self.settings,
            now,
            main_busy,
            super::paused(&self.dir),
            force,
        );
        if force {
            let _ = std::fs::remove_file(&force_file);
        }
        match gate {
            heartbeat::Gate::NotDue => {}
            heartbeat::Gate::Skip(reason) => {
                if self.last_skip != Some(reason) || force {
                    super::activity::log(
                        &self.dir,
                        "heartbeat_skip",
                        serde_json::json!({"reason": reason, "forced": force}),
                    );
                    self.last_skip = Some(reason);
                }
            }
            heartbeat::Gate::Run(prompt) => {
                self.last_skip = None;
                heartbeat::mark_ran(&self.dir, now);
                let ev = Event::new(Kind::Heartbeat, MAIN, &prompt, None);
                match event::admit(&self.dir, &ev) {
                    Ok(_) => super::activity::log(
                        &self.dir,
                        "heartbeat",
                        serde_json::json!({"forced": force}),
                    ),
                    Err(e) => log::warn!("gateway: cannot admit heartbeat: {e:#}"),
                }
            }
        }
    }

    /// Start a turn for every free key with waiting events, oldest key first,
    /// up to the global cap. While paused only user events start.
    fn dispatch(&mut self, now: i64) {
        let paused = super::paused(&self.dir);
        // Keyed by first position in the (ms-ordered) spool: `created_at` is
        // whole seconds, so it would tie and fall back to key name.
        let mut by_key: BTreeMap<(usize, String), Vec<(PathBuf, Event)>> = BTreeMap::new();
        let mut first_seen: HashMap<String, usize> = HashMap::new();
        for (path, ev) in event::pending(&self.dir) {
            if paused && ev.kind.autonomous() {
                continue;
            }
            let next = first_seen.len();
            let order = *first_seen.entry(ev.key.clone()).or_insert(next);
            by_key
                .entry((order, ev.key.clone()))
                .or_default()
                .push((path, ev));
        }
        for ((_, key), batch) in by_key {
            if self.busy.len() >= self.settings.max_turns.max(1) {
                break;
            }
            if self.busy.contains_key(&key) {
                continue;
            }
            self.start_turn(&key, batch, now);
        }
    }

    fn start_turn(&mut self, key: &str, batch: Vec<(PathBuf, Event)>, now: i64) {
        let turn = uuid::Uuid::new_v4().simple().to_string();
        let events: Vec<Event> = batch.iter().map(|(_, e)| e.clone()).collect();
        let record = Running {
            turn: turn.clone(),
            key: key.to_string(),
            events: events.clone(),
            started_at: now,
        };
        let record_path = self.dir.join("running").join(format!("{turn}.json"));
        if let Err(e) = crate::cron::store::atomic_write_json(&record_path, &record) {
            log::warn!("gateway: cannot write admission record, turn not started: {e:#}");
            return;
        }
        for (path, _) in &batch {
            let _ = std::fs::remove_file(path);
        }
        let kind = turn_kind(&events);
        // A reply goes back where the newest message came from; autonomous
        // turns use the key's last route (decided at delivery).
        let route = events.iter().rev().find_map(|e| e.route.clone());
        // The CLI inbox is not a place to send heartbeat output back to.
        if let Some(r) = route
            .as_ref()
            .filter(|r| r.platform != super::outbox::LOCAL)
        {
            self.sessions.note_route(key, r);
        }
        let request = TurnRequest {
            key: key.to_string(),
            // Autonomous turns start fresh: a heartbeat every 30 min outlives
            // the prompt cache, so resuming would re-bill the whole transcript.
            session_id: (!kind.autonomous())
                .then(|| self.sessions.session_id(key))
                .flatten(),
            prompt: compose_prompt(&events, now),
            route: route.clone().or_else(|| self.sessions.last_route(key)),
            kind,
            cwd: self
                .settings
                .workdir
                .clone()
                .or_else(dirs_home)
                .unwrap_or_else(|| PathBuf::from("/")),
            timeout: Duration::from_secs(self.settings.turn_timeout_secs.max(30)),
        };
        super::activity::log(
            &self.dir,
            "turn_start",
            serde_json::json!({"key": key, "turn": turn, "kind": kind.as_str(),
                "events": events.len(), "text": excerpt(&events[events.len() - 1].text, 160)}),
        );
        let runner = self.runner.clone();
        let tx = self.done_tx.clone();
        let (t, k) = (turn.clone(), key.to_string());
        let task = tokio::spawn(async move {
            let outcome = runner.run(request).await;
            let _ = tx.send(Done {
                turn: t,
                key: k,
                kind,
                route,
                outcome,
            });
        });
        self.busy.insert(key.to_string(), (turn, task));
    }

    /// A turn ended: remember its session, decide on its reply, drop the
    /// admission record.
    pub fn finish(&mut self, done: Done) {
        if self
            .busy
            .get(&done.key)
            .is_some_and(|(turn, _)| *turn == done.turn)
        {
            self.busy.remove(&done.key);
        }
        if let Some(sid) = done
            .outcome
            .session_id
            .as_ref()
            .filter(|_| !done.kind.autonomous())
        {
            self.sessions.set_session_id(&done.key, sid);
        }
        let last = self.sessions.last_route(&done.key);
        match &done.outcome.error {
            Some(err) => {
                super::activity::log(
                    &self.dir,
                    "turn_failed",
                    serde_json::json!({"key": done.key, "turn": done.turn,
                        "kind": done.kind.as_str(), "error": excerpt(err, 400)}),
                );
                if !done.kind.autonomous() {
                    let text = format!("⚠ I couldn't finish that: {}", excerpt(err, 400));
                    deliver::decide(
                        &self.dir,
                        &self.settings,
                        &done.key,
                        Kind::User,
                        done.route.clone(),
                        last,
                        &text,
                    );
                }
            }
            None => {
                super::activity::log(
                    &self.dir,
                    "turn_end",
                    serde_json::json!({"key": done.key, "turn": done.turn,
                        "kind": done.kind.as_str(), "chars": done.outcome.text.len()}),
                );
                deliver::decide(
                    &self.dir,
                    &self.settings,
                    &done.key,
                    done.kind,
                    done.route.clone(),
                    last,
                    &done.outcome.text,
                );
            }
        }
        let _ = std::fs::remove_file(self.dir.join("running").join(format!("{}.json", done.turn)));
    }

    /// Shutdown: stop every turn in flight. Their admission records stay,
    /// so the next boot resumes them.
    pub fn abort_all(&mut self) {
        for (key, (turn, task)) in self.busy.drain() {
            log::info!("gateway: interrupting turn {turn} ({key}); it resumes on next start");
            task.abort();
        }
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// A batch is a user turn if a person wrote any of it (strict silence,
/// never held); otherwise it carries the first event's provenance.
pub fn turn_kind(events: &[Event]) -> Kind {
    if events.iter().any(|e| e.kind == Kind::User) {
        Kind::User
    } else {
        events.first().map(|e| e.kind).unwrap_or(Kind::System)
    }
}

/// The text the model sees. A lone user message is passed as typed;
/// anything else is labelled with why it
/// woke the agent, so the model never mistakes a heartbeat or a trigger
/// for the owner speaking.
pub fn compose_prompt(events: &[Event], now: i64) -> String {
    let interrupted = events.iter().any(|e| e.attempt > 0);
    let mut parts: Vec<String> = Vec::new();
    if interrupted {
        parts.push(
            "[gateway] The previous attempt at this was cut short by a restart. Check what was \
             already done (files, commands, messages) before redoing anything."
                .to_string(),
        );
    }
    if let [only] = events
        && only.kind == Kind::User
    {
        parts.push(only.text.clone());
        return parts.join("\n\n");
    }
    let users = events.iter().filter(|e| e.kind == Kind::User).count();
    if users > 1 {
        parts.push(format!(
            "[gateway] {users} messages arrived while you were busy; answer them together."
        ));
    }
    let when = chrono::DateTime::from_timestamp(now, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%a %Y-%m-%d %H:%M %:z")
                .to_string()
        })
        .unwrap_or_default();
    for e in events {
        parts.push(match e.kind {
            Kind::User => e.text.clone(),
            // The heartbeat event already is a complete prompt.
            Kind::Heartbeat => e.text.clone(),
            Kind::Trigger => format!(
                "[gateway · trigger · {when}] Not a message from the owner: an external \
                 trigger fired. Act on it if it needs action. If the owner does not need to \
                 hear about it, reply exactly NO_REPLY.\n\n{}",
                e.text
            ),
            other => format!("[gateway · {} · {when}]\n{}", other.as_str(), e.text),
        });
    }
    parts.join("\n\n---\n\n")
}

pub fn excerpt(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

#[path = "brain_tests.rs"]
#[cfg(test)]
mod tests;
