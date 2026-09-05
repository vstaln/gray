//! Streaming (edit-in-place) reply delivery (move-only split from `daemon.rs`).
//!
//! [`Streamer`] accumulates text deltas and periodically overwrites a
//! placeholder message; [`finalize_stream`] swaps in the first chunk and
//! sends the rest as normal messages.

use std::time::{Duration, Instant};

use crate::daemon::Adapter;
use crate::platform::{
    BasePlatformAdapter, SendOptions, SendResult, split_message_smart, utf16_len,
};

/// Minimum interval between edit-in-place updates while streaming
/// (Telegram/Discord edit rate limits sit around 1/s per chat).
const STREAM_EDIT_INTERVAL: Duration = Duration::from_millis(1500);
/// Don't create the placeholder message until this much text exists.
const STREAM_MIN_CHARS: usize = 24;
const STREAM_CURSOR: &str = " ▍";

// ---------------------------------------------------------------------------
// Streaming (edit-in-place) reply
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) enum StreamMsg {
    Delta(String),
    /// A tool ran; whatever was streamed so far was narration, start over.
    Reset,
    Done(String),
}

pub(crate) struct Streamer {
    pub(crate) tx: tokio::sync::mpsc::UnboundedSender<StreamMsg>,
    task: tokio::task::JoinHandle<SendResult>,
}

impl Streamer {
    pub(crate) fn spawn(adapter: Adapter, chat: String, opts: SendOptions, max: usize) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamMsg>();
        let task = tokio::spawn(async move {
            let mut buf = String::new();
            let mut msg_id: Option<String> = None;
            let mut last_edit = Instant::now() - STREAM_EDIT_INTERVAL;
            let mut final_text: Option<String> = None;
            while let Some(m) = rx.recv().await {
                match m {
                    StreamMsg::Delta(d) => buf.push_str(&d),
                    StreamMsg::Reset => buf.clear(),
                    StreamMsg::Done(t) => {
                        final_text = Some(t);
                        break;
                    }
                }
                let preview = format!("{}{STREAM_CURSOR}", buf.trim_end());
                // Stop live-updating once the reply outgrows one message; the final
                // chunked send handles it.
                if buf.trim().chars().count() < STREAM_MIN_CHARS || utf16_len(&preview) > max {
                    continue;
                }
                if last_edit.elapsed() < STREAM_EDIT_INTERVAL {
                    continue;
                }
                match &msg_id {
                    None => {
                        let r = adapter.send_ext(&chat, &preview, &opts).await;
                        if r.success {
                            msg_id = r.message_id;
                        }
                    }
                    Some(id) => {
                        let _ = adapter.edit_message(&chat, id, &preview).await;
                    }
                }
                last_edit = Instant::now();
            }
            let text = final_text.unwrap_or_else(|| {
                if buf.is_empty() {
                    "(no reply)".into()
                } else {
                    buf.clone()
                }
            });
            finalize_stream(
                adapter.as_ref(),
                &chat,
                &opts,
                msg_id.as_deref(),
                &text,
                max,
            )
            .await
        });
        Self { tx, task }
    }

    pub(crate) async fn finish(self, text: String) -> SendResult {
        let _ = self.tx.send(StreamMsg::Done(text));
        drop(self.tx);
        match self.task.await {
            Ok(r) => r,
            Err(e) => SendResult::fail(format!("stream task failed: {e}"), true),
        }
    }
}

/// Final delivery for a streamed reply: overwrite the placeholder with the first
/// chunk, then send any remaining chunks as normal messages. Falls back to a
/// plain send if the edit fails (e.g. placeholder deleted).
pub(crate) async fn finalize_stream(
    adapter: &dyn BasePlatformAdapter,
    chat: &str,
    opts: &SendOptions,
    msg_id: Option<&str>,
    text: &str,
    max: usize,
) -> SendResult {
    let chunks = split_message_smart(text, max);
    let Some(first) = chunks.first() else {
        return SendResult::ok(msg_id.map(str::to_string));
    };
    let mut rest_start = 0;
    let mut last = SendResult::ok(msg_id.map(str::to_string));
    if let Some(id) = msg_id {
        let r = adapter.edit_message(chat, id, first).await;
        if r.success {
            rest_start = 1;
            last = r;
        }
    }
    for (i, chunk) in chunks.iter().enumerate().skip(rest_start) {
        // Only the very first message replies to the origin.
        let o = if i == 0 {
            opts.clone()
        } else {
            SendOptions {
                reply_to: None,
                thread_id: opts.thread_id.clone(),
            }
        };
        last = adapter.send_ext(chat, chunk, &o).await;
        if !last.success {
            return last;
        }
    }
    last
}
