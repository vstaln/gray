//! Progress bubbles (Hermes `turn.rs` drain pump, adapted to a live task).
//!
//! [`ProgressBubble`] shows per-tool progress lines in one channel message
//! while the agent works, then deletes the bubbles so the chat ends with
//! just the final answer. Replaces streaming answer edits.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::daemon::Adapter;
use crate::platform::SendOptions;

/// Minimum interval between progress-bubble EDITS while working
/// (Discord/Telegram edit rate limits sit around 1/s per chat).
/// The first send is always immediate; only edits are throttled.
const PUMP_MIN_INTERVAL: Duration = Duration::from_millis(1500);

// ---------------------------------------------------------------------------
// Progress bubbles (Hermes `turn.rs` drain pump, adapted to a live task)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) enum ProgressMsg {
    ToolStart { id: String, name: String },
    ToolEnd { id: String, args: serde_json::Value },
    Done,
}

pub(crate) struct ProgressBubble {
    pub(crate) tx: tokio::sync::mpsc::UnboundedSender<ProgressMsg>,
    task: tokio::task::JoinHandle<()>,
}

impl ProgressBubble {
    pub(crate) fn spawn(adapter: Adapter, chat: String, opts: SendOptions, max: usize) -> Self {
        use crate::progress::ProgressLines;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProgressMsg>();
        let task = tokio::spawn(async move {
            let mut lines = ProgressLines::new();
            // ToolCallEnd carries no name — track start id → name here.
            let mut names: HashMap<String, String> = HashMap::new();
            let mut bubble_ids: Vec<String> = Vec::new();
            let mut last_pump = Instant::now() - PUMP_MIN_INTERVAL;
            while let Some(m) = rx.recv().await {
                match m {
                    ProgressMsg::ToolStart { id, name } => {
                        names.insert(id, name.clone());
                        lines.push_start(&name);
                    }
                    ProgressMsg::ToolEnd { id, args } => {
                        // Start always precedes end for an id; fall back to
                        // the id itself so an unpaired end never drops silently.
                        let name = names.remove(&id).unwrap_or_else(|| id.clone());
                        lines.push_end(&name, &args);
                    }
                    ProgressMsg::Done => break,
                }
                Self::pump(&adapter, &chat, &opts, max, &lines, &mut bubble_ids, &mut last_pump).await;
            }
            // Hermes cleanup_msg_ids: progress bubbles are ephemeral — the
            // chat ends with just the final answer.
            for id in &bubble_ids {
                let r = adapter.delete_message(&chat, id).await;
                if !r.success {
                    log::warn!("progress bubble delete failed: {:?}", r.error);
                }
            }
        });
        Self { tx, task }
    }

    /// Send the first bubble immediately; edit it (throttled) afterwards.
    /// Overflow rolls into extra bubbles; every sent id is tracked so the
    /// finish path can delete them all.
    async fn pump(
        adapter: &Adapter,
        chat: &str,
        opts: &SendOptions,
        max: usize,
        lines: &crate::progress::ProgressLines,
        bubble_ids: &mut Vec<String>,
        last_pump: &mut Instant,
    ) {
        if lines.is_empty() {
            return;
        }
        let groups = lines.split_groups(max);
        let Some(first) = groups.first() else { return };
        match bubble_ids.last().cloned() {
            None => {
                let r = adapter.send_ext(chat, first, opts).await;
                if r.success {
                    if let Some(id) = r.message_id {
                        bubble_ids.push(id);
                    }
                    *last_pump = Instant::now();
                } else {
                    log::warn!("progress bubble send failed: {:?}", r.error);
                }
            }
            Some(id) => {
                if last_pump.elapsed() < PUMP_MIN_INTERVAL {
                    return;
                }
                let r = adapter.edit_message(chat, &id, first).await;
                if r.success {
                    *last_pump = Instant::now();
                } else if r.retryable {
                    // Transient (flood control) — skip this tick, retry next.
                } else {
                    log::warn!("progress bubble edit failed permanently, sending fresh: {:?}", r.error);
                    Self::send_fresh(adapter, chat, opts, first, bubble_ids).await;
                    *last_pump = Instant::now();
                }
            }
        }
        // Overflow groups always send as new bubbles (Hermes first-edits-rest-sends).
        for extra in groups.iter().skip(1) {
            Self::send_fresh(adapter, chat, opts, extra, bubble_ids).await;
        }
    }

    async fn send_fresh(
        adapter: &Adapter,
        chat: &str,
        opts: &SendOptions,
        text: &str,
        bubble_ids: &mut Vec<String>,
    ) {
        let r = adapter.send_ext(chat, text, opts).await;
        if r.success {
            if let Some(id) = r.message_id {
                bubble_ids.push(id);
            }
        } else {
            log::warn!("progress bubble send failed: {:?}", r.error);
        }
    }

    /// Delete bubbles, then hand the final text back so the caller delivers
    /// it as fresh message(s) via the normal chunked path. Never fails —
    /// on task panic the text is still returned for delivery.
    pub(crate) async fn finish(self, text: String) -> String {
        let _ = self.tx.send(ProgressMsg::Done);
        drop(self.tx);
        if let Err(e) = self.task.await {
            log::warn!("progress bubble task failed: {e}");
        }
        text
    }
}

