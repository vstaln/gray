//! `gray gateway send|wake|activity|pause|resume|heartbeat|inbox`: talk to
//! and steer the always-on agent from a shell.

#[derive(clap::Subcommand, Debug, Clone)]
pub enum AgentCmd {
    /// Say something to the agent's main session and print its reply
    Send {
        /// Message text (joined with spaces)
        #[arg(required = true, trailing_var_arg = true)]
        text: Vec<String>,
        /// Seconds to wait for the reply (0 = don't wait)
        #[arg(long, default_value_t = 900)]
        wait: u64,
    },
    /// Wake the agent with a note (a `trigger` event into main), no waiting
    Wake {
        #[arg(required = true, trailing_var_arg = true)]
        text: Vec<String>,
    },
    /// What the agent did, said, held back and why
    Activity {
        /// Rows to show (`tail -f ~/.gray/gateway/activity.jsonl` to follow)
        #[arg(short = 'n', long, default_value_t = 30)]
        lines: usize,
    },
    /// Stop autonomous wakes (heartbeat, wake); user messages still run
    Pause,
    /// Undo `pause`
    Resume,
    /// Run a heartbeat now (still skipped when paused or busy)
    Heartbeat,
    /// Print and clear replies that had no chat to go to
    Inbox,
}

use std::path::Path;
use std::time::Duration;

use super::event::{self, Event, Kind, MAIN};
use super::outbox::{self, Intent, LOCAL};
use crate::cron::store::Origin as Route;

/// Everything here is files under `~/.gray/gateway/`; the running gateway
/// polls them, so no socket round-trip is needed.
pub async fn run(cmd: AgentCmd) -> anyhow::Result<()> {
    let home = crate::setup::gray_home()?;
    let dir = super::state_dir(&home);
    std::fs::create_dir_all(&dir)?;
    match cmd {
        AgentCmd::Send { text, wait } => send(&home, &dir, &text.join(" "), wait).await?,
        AgentCmd::Wake { text } => {
            event::admit(
                &dir,
                &Event::new(Kind::Trigger, MAIN, &text.join(" "), None),
            )?;
            println!("woke main; see `gray gateway activity`");
            warn_if_down(&home);
        }
        AgentCmd::Activity { lines } => {
            let rows = super::activity::tail(&dir, lines);
            if rows.is_empty() {
                println!("no activity yet");
            }
            for row in rows {
                println!("{}", super::activity::render(&row));
            }
        }
        AgentCmd::Pause => {
            std::fs::write(dir.join("PAUSED"), "")?;
            println!("paused: heartbeat and wake turns wait; messages still run");
        }
        AgentCmd::Resume => {
            let _ = std::fs::remove_file(dir.join("PAUSED"));
            println!("resumed");
        }
        AgentCmd::Heartbeat => {
            std::fs::write(dir.join("heartbeat.force"), "")?;
            println!("heartbeat requested; see `gray gateway activity`");
            warn_if_down(&home);
        }
        AgentCmd::Inbox => {
            let got = outbox::pull(&dir, LOCAL, crate::cron::now_secs(), 100);
            if got.is_empty() {
                println!("inbox empty");
            }
            for i in &got {
                println!("{}  {}", when(i.created_at), label(i));
            }
            outbox::ack(&dir, &got.iter().map(|i| i.id.clone()).collect::<Vec<_>>());
        }
    }
    Ok(())
}

async fn send(home: &Path, dir: &Path, text: &str, wait: u64) -> anyhow::Result<()> {
    let route = Route {
        platform: LOCAL.into(),
        chat: "cli".into(),
        thread: None,
        route: None,
    };
    let sent_at = crate::cron::now_secs();
    event::admit(dir, &Event::new(Kind::User, MAIN, text, Some(route)))?;
    if super::pid::running(home).is_none() {
        println!("queued; the gateway isn't running (`gray gateway start`)");
        return Ok(());
    }
    if wait == 0 {
        println!("sent");
        return Ok(());
    }
    let deadline = sent_at + wait as i64;
    loop {
        let now = crate::cron::now_secs();
        let got = outbox::pull(dir, LOCAL, now, 50);
        for i in &got {
            println!("{}", label(i));
        }
        outbox::ack(dir, &got.iter().map(|i| i.id.clone()).collect::<Vec<_>>());
        if answered(&got, &super::activity::tail(dir, 50), sent_at) {
            return Ok(());
        }
        if now >= deadline {
            println!("no reply yet; `gray gateway inbox` later");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The main turn for a message sent at `sent_at` has ended: its reply (or
/// failure note) arrived, or it chose silence.
fn answered(got: &[Intent], activity: &[serde_json::Value], sent_at: i64) -> bool {
    got.iter()
        .any(|i| i.kind == Kind::User && i.key == MAIN && i.created_at >= sent_at)
        || activity.iter().any(|r| {
            r["what"] == "suppressed"
                && r["key"] == MAIN
                && r["kind"] == "user"
                && r["at"].as_i64().is_some_and(|at| at >= sent_at)
        })
}

/// A user reply prints bare; anything the agent said on its own is tagged.
fn label(i: &Intent) -> String {
    if i.kind == Kind::User {
        i.text.clone()
    } else {
        format!("[{}] {}", i.kind.as_str(), i.text)
    }
}

fn when(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn warn_if_down(home: &Path) {
    if super::pid::running(home).is_none() {
        println!("(the gateway isn't running; it picks this up on `gray gateway start`)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_stops_on_its_reply_or_silence_only() {
        let reply = |kind, at| {
            let mut i = Intent::new(MAIN, kind, None, "x");
            i.created_at = at;
            i
        };
        assert!(answered(&[reply(Kind::User, 10)], &[], 10));
        // A heartbeat landing in the inbox, or an older reply, is not the answer.
        assert!(!answered(&[reply(Kind::Heartbeat, 10)], &[], 10));
        assert!(!answered(&[reply(Kind::User, 9)], &[], 10));
        let row =
            |at| serde_json::json!({"at": at, "what": "suppressed", "key": "main", "kind": "user"});
        assert!(answered(&[], &[row(11)], 10));
        assert!(!answered(&[], &[row(9)], 10));
    }
}
