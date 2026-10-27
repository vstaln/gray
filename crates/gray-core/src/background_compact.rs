use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Instant;

use crate::agent::Agent;
use crate::error::CoreError;
use crate::message::{ContentBlock, Message, Role};

pub(crate) struct HeldSummary {
    pub(crate) covered: usize,
    pub(crate) prefix_digest: u64,
    pub(crate) system_digest: u64,
    pub(crate) model: String,
    pub(crate) anchor: Option<Message>,
    pub(crate) started: Instant,
    pub(crate) task: tokio::task::JoinHandle<Result<String, CoreError>>,
}

impl Drop for HeldSummary {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl HeldSummary {
    pub(crate) fn discard_reason(&self, agent: &Agent) -> Option<&'static str> {
        if agent.messages.len() < self.covered
            || prefix_digest(&agent.messages[..self.covered]) != self.prefix_digest
        {
            return Some("digest");
        }
        if system_digest(agent.system_text()) != self.system_digest {
            return Some("system");
        }
        if agent.provider.model_id() != self.model {
            return Some("model");
        }
        let threshold = compaction_threshold(agent)?;
        let keep = compaction_keep(agent);
        let tail_limit = keep.saturating_mul(2).max(30_000).min(threshold / 4);
        (crate::agent_compact::est_tokens(&agent.messages[self.covered..]) > tail_limit)
            .then_some("tail")
    }
}

impl Agent {
    pub(crate) fn maybe_start_background_compaction(&mut self) {
        if !self.background_compaction
            || self.held_summary.is_some()
            || self.background_failures >= 2
        {
            return;
        }
        let Some(threshold) = compaction_threshold(self) else {
            return;
        };
        let estimate = self.estimate_tokens();
        if estimate < threshold.saturating_mul(85) / 100
            || estimate >= threshold
            || estimate < 64_000.min(threshold / 2)
        {
            return;
        }

        let mut covered = self.messages.len();
        if self.messages.last().is_some_and(|message| {
            message.role == Role::Assistant
                && message
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
        }) {
            covered = covered.saturating_sub(1);
        }
        if covered == 0 {
            return;
        }
        let Some(prepared) =
            self.prepare_compaction(&self.messages[..covered], None, self.compact_keep)
        else {
            return;
        };
        let prefix_digest = prefix_digest(&self.messages[..covered]);
        let system_digest = system_digest(self.system_text());
        let model = self.provider.model_id().to_string();
        let anchor = prepared.anchor.clone();
        let started = Instant::now();
        let task = tokio::spawn(crate::agent_compact::run_compaction_summary(
            prepared.request,
        ));
        self.held_summary = Some(HeldSummary {
            covered,
            prefix_digest,
            system_digest,
            model,
            anchor,
            started,
            task,
        });
        log::info!(
            target: "gray_agent",
            "background compaction: started covered={covered} est={estimate} threshold={threshold}"
        );
    }
}

fn compaction_threshold(agent: &Agent) -> Option<usize> {
    let window = agent.context_window?;
    let threshold = window.saturating_sub(
        agent
            .compact_reserve
            .unwrap_or(crate::agent_compact::COMPACT_RESERVE_TOKENS),
    );
    Some(
        agent
            .compact_ceiling
            .map_or(threshold, |ceiling| threshold.min(ceiling)),
    )
}

fn compaction_keep(agent: &Agent) -> usize {
    let budget = agent.compact_keep.unwrap_or(match agent.context_window {
        None => crate::compact::RETAINED_MESSAGE_TOKEN_BUDGET,
        Some(window) => crate::compact::RETAINED_MESSAGE_TOKEN_BUDGET
            .min(window.saturating_sub(crate::agent_compact::COMPACT_RESERVE_TOKENS)),
    });
    agent.compact_ceiling.map_or(budget, |ceiling| {
        budget.min(ceiling / crate::compact::PRICE_CLIFF_KEEP_DIVISOR)
    })
}

fn prefix_digest(messages: &[Message]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for message in messages {
        crate::prefix_guard::message_digest(message).hash(&mut hasher);
    }
    hasher.finish()
}

fn system_digest(system: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    system.hash(&mut hasher);
    hasher.finish()
}
