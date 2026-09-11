//! Turn submission queue (Codex TurnInputMode port, narrowed): admission
//! happens BEFORE anything touches history. Today all submitters hold
//! `&mut Agent`, so Busy is reachable only mid-turn via `steer`-era flows
//! and via forced state in tests — the seam exists so future concurrent
//! callers get Start/Steer/Reject for free. (Recon: steer() callers are
//! REPL loop-top only (repl/mod.rs:667,679,690), &mut-owned between turns,
//! never concurrent; ContentBlock = Text/Image/ToolUse/ToolResult/Thinking,
//! empty vec or all-Text-blank → EmptyInput, any non-Text → admit fail-open.)

use crate::message::{ContentBlock, Message};

/// How a caller wants its input admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitMode {
    /// Start a turn, or steer the live one (record as steer text).
    StartOrSteer,
    /// Start only when idle; otherwise reject without recording.
    StartIfIdle,
}

/// Admission outcome. Rejections guarantee zero history mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submission {
    Started { turn_id: u64 },
    Steered { turn_id: u64 },
    NotSubmitted(RejectReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    EmptyInput,
    NotIdle { turn_id: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnState {
    Idle,
    Busy { turn_id: u64 },
}

pub(crate) fn is_empty_input(msg: &Message) -> bool {
    if msg.content.is_empty() {
        return true;
    }
    msg.content.iter().all(|b| match b {
        ContentBlock::Text { text } => text.trim().is_empty(),
        // Thinking/ToolUse/ToolResult/Image are never "empty input"; any
        // unknown variant → admit (fail-open: never silently drop user input).
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, Provider, ProviderStream, ToolContext, ToolExecutor, ToolOutput};
    use crate::event::{StopReason, StreamEvent};
    use async_trait::async_trait;
    use futures::future::BoxFuture;
    use std::sync::Arc;

    /// Provider that ends the turn immediately with no output.
    struct NullProvider;

    #[async_trait]
    impl Provider for NullProvider {
        fn stream(&self, _req: crate::message::ChatRequest) -> ProviderStream {
            Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::message_complete(Some(StopReason::EndTurn), None),
            )]))
        }
    }

    /// Executor that answers every tool with empty ok output.
    struct NullExecutor;

    #[async_trait]
    impl ToolExecutor for NullExecutor {
        fn execute(
            &self,
            _ctx: &ToolContext,
            _name: &str,
            _args: serde_json::Value,
        ) -> BoxFuture<'static, ToolOutput> {
            Box::pin(async move { ToolOutput::ok("") })
        }
    }

    fn test_agent() -> Agent {
        Agent::new(Box::new(NullProvider), Arc::new(NullExecutor))
    }

    fn user(text: &str) -> Message {
        Message::user(text.to_string())
    }

    #[test]
    fn empty_input_is_rejected_without_touching_history() {
        let mut agent = test_agent();
        agent.messages.push(user("prior"));
        let sub = agent.submit(Message::user("   ".to_string()), SubmitMode::StartOrSteer);
        assert_eq!(sub, Submission::NotSubmitted(RejectReason::EmptyInput));
        assert_eq!(agent.messages.len(), 1, "rejection must not record");
        assert!(agent.pending_steer.is_empty());
    }

    #[test]
    fn idle_start_pushes_and_advances_turn() {
        let mut agent = test_agent();
        let Submission::Started { turn_id } = agent.submit(user("hi"), SubmitMode::StartOrSteer)
        else {
            panic!("must start")
        };
        assert_eq!(turn_id, 1);
        assert_eq!(agent.messages.len(), 1);
        assert_eq!(agent.current_turn(), None, "no live turn outside run()");
    }

    #[test]
    fn busy_start_steers_instead_of_recording() {
        let mut agent = test_agent();
        agent.turn_state = TurnState::Busy { turn_id: 7 }; // pub(crate) — same crate
        let sub = agent.submit(user("wait, also this"), SubmitMode::StartOrSteer);
        assert!(matches!(sub, Submission::Steered { turn_id: 7 }));
        assert!(
            agent.messages.is_empty(),
            "steered input must not touch history directly"
        );
        assert_eq!(agent.pending_steer, vec!["wait, also this".to_string()]);
    }

    #[test]
    fn start_if_idle_rejects_busy_without_recording() {
        let mut agent = test_agent();
        agent.turn_state = TurnState::Busy { turn_id: 7 };
        let sub = agent.submit(user("clobber?"), SubmitMode::StartIfIdle);
        assert_eq!(
            sub,
            Submission::NotSubmitted(RejectReason::NotIdle { turn_id: 7 })
        );
        assert!(agent.messages.is_empty() && agent.pending_steer.is_empty());
    }
}
