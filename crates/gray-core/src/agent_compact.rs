//! Transcript compaction for context-overflow recovery (move-only split).
//!
//! [`summary_pair`] is the shared compaction envelope (`Agent` recovery and
//! `gray::compact` can never drift); [`Agent::try_compact_once`] summarizes
//! history into that shape via [`Agent::complete_prompt`].

use crate::agent::Agent;
use crate::error::CoreError;
use crate::message::Message;

/// Shared compaction envelope: `[summary_user, summary_ack]` so
/// `Agent::try_compact_once` and `gray::compact` can never drift.
/// Trims `summary`; byte-stable (see `summary_pair_envelope_is_byte_stable`).
pub fn summary_pair(summary: &str) -> [Message; 2] {
    let s = summary.trim();
    [
        Message::user(format!(
            "<conversation_summary>\n{s}\n</conversation_summary>\n\nPlease continue assisting based on the summary above."
        )),
        Message::assistant(
            "Understood. I have reviewed the conversation summary and context, and I am ready to continue.",
        ),
    ]
}

impl Agent {
    /// One-shot transcript compaction for context-overflow recovery: summarizes
    /// history via [`complete_prompt`](Self::complete_prompt) into the 2-message
    /// summary shape. False when there is nothing worth compacting.
    pub(crate) async fn try_compact_once(&mut self) -> Result<bool, CoreError> {
        if self.messages.is_empty() {
            return Ok(false);
        }
        let transcript = self
            .messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.text_content()))
            .collect::<Vec<_>>()
            .join("\n");
        if transcript.trim().is_empty() {
            return Ok(false);
        }
        let summary = self
            .complete_prompt(
                &format!(
                    "Summarize this conversation concisely, preserving key facts, decisions, and pending work:\n{transcript}"
                ),
                Some("You summarize conversations for context compaction."),
            )
            .await?;
        if summary.trim().is_empty() {
            return Ok(false);
        }
        self.messages = summary_pair(&summary).into();
        Ok(true)
    }
}
