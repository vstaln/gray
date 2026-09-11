//! Context compaction and summarization for Gray conversations.
//!
//! Thin wrappers over `gray_core`'s codex-v2 pipeline ([`Agent::compact_v2`]):
//! manual `/compact` and the REPL auto paths share the same trim → in-band
//! trigger → retention walk → summary-last logic as the in-turn recovery path,
//! plus the gray-specific lifecycle (file-ledger disarm, continuation
//! checkpoint).

use gray_core::agent::Agent;
use gray_core::error::CoreError;
use gray_core::event::Usage;
use gray_core::message::Message;

pub mod policy;

pub use policy::{
    CompactionSettings, compaction_settings_for, estimate_context_tokens, estimate_tokens,
    init_auto_compact_from_env, is_auto_compact_enabled, is_context_overflow_error,
    set_auto_compact_enabled, should_compact,
};

pub async fn auto_compact_if_needed(agent: &mut Agent) -> Result<bool, CoreError> {
    if !is_auto_compact_enabled() {
        return Ok(false);
    }
    let keep = crate::setup::user_keep_recent_tokens();
    Ok(compact_with_keep(agent, None, keep).await?.is_some())
}

/// Manual `/compact` (`custom_instructions` from `/compact <text>`) and REPL
/// auto compaction: one codex-v2 call via [`Agent::compact_v2`] with
/// `keep_tokens` as the retained-history budget (`0` → summary-only). Returns
/// the summary when history was replaced; `None` when nothing was gained
/// (history untouched). Keeps the gray-specific lifecycle: file-ledger dedup
/// entries are disarmed, and the summary is checkpointed for human recovery.
pub async fn compact_with_keep(
    agent: &mut Agent,
    custom_instructions: Option<&str>,
    keep_tokens: usize,
) -> Result<Option<String>, CoreError> {
    let replaced = agent.messages().len();
    if replaced == 0 {
        return Ok(None);
    }
    let Some(summary) = agent
        .compact_v2(custom_instructions, Some(keep_tokens))
        .await?
    else {
        return Ok(None);
    };
    // T3.4 lifecycle: entries stay for the write guard, but no dedup stub
    // may reference a compacted-away result.
    if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
        ledger.disarm_all_dedup();
    }
    // Reversible checkpoint: summary on disk, so nothing is truly lost.
    // Best-effort; compaction succeeds even if it fails.
    let path = write_continuation_checkpoint(&summary, replaced);
    if let Some(p) = path {
        eprintln!("continuation checkpoint: {}", p.display());
    }
    Ok(Some(summary))
}

/// Best-effort snapshot of the compact summary for human recovery. Carries
/// only the summary: the full pre-compact transcript stays durable in the
/// session JSONL (pre-boundary entries) and nothing ever reads this file back
/// into the model, so copying the transcript here only widened secret
/// exposure. Lives under the gray home dir (never /tmp or the workspace) with
/// an unpredictable uuid name, owner-only (0600) on unix; newest 5 kept.
pub fn write_continuation_checkpoint(
    summary: &str,
    replaced_messages: usize,
) -> Option<std::path::PathBuf> {
    let dir = crate::setup::catalog::gray_home()
        .ok()?
        .join("continuation-checkpoints");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!(
        "gray-continuation-{}.md",
        uuid::Uuid::new_v4().as_simple()
    ));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let doc = format!(
        "# Gray continuation checkpoint ({ts})\n\nCompacted {replaced_messages} messages.\n\n## Summary\n\n{summary}\n"
    );
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).ok()?;
    use std::io::Write as _;
    file.write_all(doc.as_bytes()).ok()?;
    let _ = file.sync_all();
    rotate_continuation_checkpoints(&dir);
    Some(path)
}

/// Keeps the newest `KEEP` checkpoints; best-effort, failures ignored.
fn rotate_continuation_checkpoints(dir: &std::path::Path) {
    const KEEP: usize = 5;
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(dir)
        .ok()
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("gray-continuation-")
                })
                .filter_map(|e| {
                    let mtime = e.metadata().ok()?.modified().ok()?;
                    Some((mtime, e.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|(mtime, _)| *mtime);
    files.reverse();
    for (_, path) in files.into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::message::{ContentBlock, Role};
    use std::sync::Arc;

    #[test]
    fn should_compact_threshold() {
        let s = CompactionSettings {
            reserve_tokens: 16384,
        };
        assert!(!should_compact(100_000, 128_000, &s));
        assert!(should_compact(115_000, 128_000, &s)); // 115k > 128k-16k
    }

    #[test]
    fn summary_envelope_matches_core_helper_byte_for_byte() {
        let [u1, a1] = gray_core::agent::summary_pair("  shared summary  ");
        assert!(u1.text_content().contains("shared summary"));
        // Both compact paths build via the same helper, so envelopes are identical.
        let [u2, a2] = gray_core::agent::summary_pair("shared summary");
        assert_eq!(u1.text_content().as_bytes(), u2.text_content().as_bytes());
        assert_eq!(a1.text_content().as_bytes(), a2.text_content().as_bytes());
    }

    #[test]
    fn estimate_uses_usage_when_available() {
        let msgs = vec![Message::user("hi"), Message::assistant("hello")];
        let usage = Usage {
            input_tokens: 100000,
            output_tokens: 10000,
            ..Default::default()
        };
        assert_eq!(estimate_context_tokens(&msgs, Some(usage)), 110000);
    }

    #[test]
    fn estimate_falls_back_to_chars() {
        let msgs = vec![Message::user("a".repeat(400))]; // 100 tokens
        assert_eq!(estimate_context_tokens(&msgs, None), 100);
    }

    /// A message holding one capped tool result (`truncate.rs` allows 50 KiB)
    /// must not measure as free. Regression guard for `estimate_tokens`
    /// measuring with a display accessor.
    #[test]
    fn estimate_counts_tool_results() {
        // 4 KiB of tool output => 4096/4 = 1024 tokens.
        let big = Message::new(
            Role::User,
            vec![ContentBlock::tool_result("t1", "x".repeat(4096), false)],
        );
        assert_eq!(
            estimate_tokens(&big),
            1024,
            "a 4 KiB tool result is ~1024 tokens; measuring 0 means the \
             estimator is reading text blocks only"
        );
    }

    /// With no provider `Usage` (resumed session, or an OpenAI-compatible
    /// endpoint that omits usage on streaming), the estimator is the only
    /// signal deciding whether to compact. A tool-heavy history must cross
    /// the reserve line.
    #[test]
    fn tool_heavy_history_trips_threshold_without_provider_usage() {
        let msgs: Vec<Message> = (0..10)
            .map(|i| {
                Message::new(
                    Role::User,
                    vec![ContentBlock::tool_result(
                        format!("t{i}"),
                        "y".repeat(8192),
                        false,
                    )],
                )
            })
            .collect();

        // 10 x 8 KiB = 80 KiB => 20480 tokens. Before the fix: 0.
        let tokens = estimate_context_tokens(&msgs, None);
        assert_eq!(tokens, 20_480, "80 KiB of tool output measured as {tokens}");

        let s = CompactionSettings {
            reserve_tokens: 16_384,
        };
        // 32k window, 16384 reserve => threshold 15616. 20480 is over it.
        assert!(
            should_compact(tokens, 32_000, &s),
            "tool-heavy history must auto-compact; before the fix it measured \
             zero and the session ran straight into a provider overflow"
        );
    }

    #[test]
    fn is_context_overflow_error_detects() {
        assert!(is_context_overflow_error(&CoreError::Provider(
            "context_length_exceeded: too many tokens".into()
        )));
        assert!(is_context_overflow_error(&CoreError::Provider(
            "This model's maximum context length is 128000 tokens".into()
        )));
        assert!(is_context_overflow_error(&CoreError::Provider(
            "context window exceeded".into()
        )));
        assert!(is_context_overflow_error(&CoreError::Provider(
            "max_tokens exceeded".into()
        )));
        // Generic "length" truncation is NOT overflow via this helper (handled via stopReason by the caller)
        assert!(!is_context_overflow_error(&CoreError::Provider(
            "length truncation: output was cut off".into()
        )));
        assert!(!is_context_overflow_error(&CoreError::Provider(
            "rate limit exceeded".into()
        )));
        assert!(!is_context_overflow_error(&CoreError::Cancelled));
    }

    #[test]
    fn repl_threshold_120k_128k_triggers_compact() {
        let usage = Usage {
            input_tokens: 100_000,
            output_tokens: 20_000,
            ..Default::default()
        };
        assert_eq!(usage.total(), 120_000);
        let window = 128_000;
        let tokens = estimate_context_tokens(&[Message::user("hi")], Some(usage));
        assert_eq!(tokens, 120_000);
        let s = CompactionSettings {
            reserve_tokens: 16_384,
        };
        assert!(should_compact(tokens, window, &s));
        // 100k should NOT trigger
        let usage2 = Usage {
            input_tokens: 90_000,
            output_tokens: 10_000,
            ..Default::default()
        };
        let tokens2 = estimate_context_tokens(&[Message::user("hi")], Some(usage2));
        assert!(!should_compact(tokens2, window, &s));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serializes against switch-flipping tests for the whole test
    async fn auto_compact_triggers_on_threshold() {
        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
        let _home = TempGrayHome::set();
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use gray_core::agent::{Agent, Provider, ToolContext, ToolExecutor};
        use gray_core::event::{StopReason, Usage};
        use gray_core::message::ChatRequest;

        struct FakeProvider {
            summary: String,
        }
        #[async_trait]
        impl Provider for FakeProvider {
            fn stream(
                &self,
                _req: ChatRequest,
            ) -> BoxStream<
                'static,
                Result<gray_core::event::StreamEvent, gray_core::agent::ProviderError>,
            > {
                let summary = self.summary.clone();
                let events = vec![
                    gray_core::event::StreamEvent::TextDelta { delta: summary },
                    gray_core::event::StreamEvent::MessageComplete {
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Some(Usage::new(10, 5)),
                    },
                ];
                Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
            }
        }

        struct NoopExecutor;
        #[async_trait]
        impl ToolExecutor for NoopExecutor {
            fn execute(
                &self,
                _ctx: &ToolContext,
                _name: &str,
                _args: serde_json::Value,
            ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
                Box::pin(async { gray_core::agent::ToolOutput::ok("") })
            }
        }

        let provider = FakeProvider {
            summary: "Test summary content".to_string(),
        };
        let executor = NoopExecutor;
        crate::setup::set_user_keep_recent_tokens(Some(0));
        // Large enough that the summary pair strictly shrinks history (the
        // enforced shrink invariant refuses toy histories).
        let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_messages(vec![
            Message::user("hello ".repeat(500)),
            Message::assistant("hi there ".repeat(500)),
            Message::user("more context ".repeat(500)),
        ]);
        let compacted = auto_compact_if_needed(&mut agent)
            .await
            .expect("compact should succeed");
        crate::setup::set_user_keep_recent_tokens(None);
        assert!(compacted, "should have compacted");
        assert_eq!(
            agent.messages().len(),
            2,
            "should be 2 messages after compact"
        );
        assert!(
            agent.messages()[0]
                .text_content()
                .contains("Test summary content")
        );
    }

    /// Redirects `GRAY_HOME` at a test-private dir (restored on drop) so
    /// checkpoint writes never touch the real home. Mutates process env:
    /// hold `COMPACT_SWITCH_SERIAL` for the whole test.
    struct TempGrayHome {
        prev: Option<String>,
        _dir: tempfile::TempDir,
    }
    impl TempGrayHome {
        fn set() -> Self {
            let prev = std::env::var("GRAY_HOME").ok();
            let dir = tempfile::TempDir::new().expect("temp gray home");
            unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
            Self { prev, _dir: dir }
        }
    }
    impl Drop for TempGrayHome {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("GRAY_HOME", v) },
                None => unsafe { std::env::remove_var("GRAY_HOME") },
            }
        }
    }

    /// Scripted `complete_prompt` provider returning one fixed summary text.
    fn scripted_agent(summary_text: &str) -> Agent {
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use gray_core::agent::{Provider, ToolContext, ToolExecutor};
        use gray_core::event::{StopReason, Usage};
        use gray_core::message::ChatRequest;
        struct P {
            text: String,
        }
        #[async_trait]
        impl Provider for P {
            fn stream(
                &self,
                _req: ChatRequest,
            ) -> BoxStream<
                'static,
                Result<gray_core::event::StreamEvent, gray_core::agent::ProviderError>,
            > {
                let text = self.text.clone();
                Box::pin(futures::stream::iter(vec![
                    Ok(gray_core::event::StreamEvent::TextDelta { delta: text }),
                    Ok(gray_core::event::StreamEvent::MessageComplete {
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Some(Usage::new(10, 5)),
                    }),
                ]))
            }
        }
        struct E;
        #[async_trait]
        impl ToolExecutor for E {
            fn execute(
                &self,
                _ctx: &ToolContext,
                _name: &str,
                _args: serde_json::Value,
            ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
                Box::pin(async { gray_core::agent::ToolOutput::ok("") })
            }
        }
        Agent::new(
            Box::new(P {
                text: summary_text.to_string(),
            }),
            Arc::new(E),
        )
    }

    /// One ~100-token message by the estimator (bytes/4): 400 chars.
    fn sized(tag: &str) -> Message {
        Message::user(format!("{tag}:{}", "x".repeat(400 - tag.len() - 1)))
    }

    #[tokio::test]
    async fn compact_with_keep_rejects_blank_summary() {
        let mut ag = scripted_agent("   \n  ");
        ag.set_messages(vec![Message::user("hello"), Message::assistant("hi")]);
        let before = ag.messages().to_vec();
        let out = compact_with_keep(&mut ag, None, 0)
            .await
            .expect("must not error on blank summary");
        assert!(out.is_none(), "blank summary must refuse, not wipe history");
        assert_eq!(ag.messages(), &before);
    }

    #[tokio::test]
    async fn compact_refuses_non_shrinking_replacement() {
        // One tiny message vs a huge summary: the replacement is bigger, so
        // compaction must leave history byte-identical (mirrors core's
        // `budgeted_compact_false_when_summary_would_not_shrink`).
        let long = "z".repeat(2000);
        let mut ag = scripted_agent(&long);
        ag.set_messages(vec![Message::user("hi")]);
        let before = ag.messages().to_vec();
        assert!(
            compact_with_keep(&mut ag, None, 0)
                .await
                .expect("must not error")
                .is_none()
        );
        assert_eq!(ag.messages(), &before);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serial guard must cover the whole test (env + gray home)
    async fn compact_retains_newest_and_boundary_truncates_oldest() {
        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
        let _home = TempGrayHome::set();
        let mut ag = scripted_agent("S");
        ag.set_messages(vec![sized("m1"), sized("m2"), sized("m3"), sized("m4")]);
        // V2 budget walk (keep=250): m4 + m3 fit (200), m2 is the boundary
        // group → middle-truncated into the remaining ~50, m1 dropped.
        let out = compact_with_keep(&mut ag, None, 250)
            .await
            .expect("compact must succeed");
        assert!(out.is_some());
        let msgs = ag.messages();
        assert_eq!(
            msgs.len(),
            5,
            "truncated m2 + m3 + m4 + summary pair, got {}",
            msgs.len()
        );
        assert!(
            msgs[0].text_content().starts_with("m2:"),
            "boundary group kept truncated: {}",
            msgs[0].text_content().chars().take(20).collect::<String>()
        );
        assert!(msgs[1].text_content().contains("m3"), "retained oldest");
        assert!(msgs[2].text_content().contains("m4"), "retained newest");
        assert!(msgs[3].text_content().contains('S'), "summary_user last");
        assert!(msgs[4].text_content().contains("Understood"), "ack closes");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // see compact_retains_newest_and_boundary_truncates_oldest
    async fn compact_drops_tool_pair_atomically() {
        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
        let _home = TempGrayHome::set();
        let mut ag = scripted_agent("S");
        // Tool-use-only assistant group: v2 drops it together with its
        // result — never an orphaned call or result in the replacement.
        let use_msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "t1",
                "sh",
                serde_json::json!({"cmd": "x".repeat(2000)}),
            )],
        );
        let res_msg = Message::new(
            Role::User,
            vec![ContentBlock::tool_result("t1", "ok", false)],
        );
        ag.set_messages(vec![
            Message::user("o".repeat(2000)),
            use_msg,
            res_msg,
            Message::user("new"),
        ]);
        let out = compact_with_keep(&mut ag, None, 250)
            .await
            .expect("compact must succeed");
        assert!(
            out.is_some(),
            "dropping the pair still shrinks, must compact"
        );
        let msgs = ag.messages();
        assert!(
            msgs.iter().flat_map(|m| &m.content).all(|b| !matches!(
                b,
                ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
            )),
            "no orphaned call/result may survive: {msgs:?}"
        );
        assert!(
            msgs.last()
                .expect("history")
                .text_content()
                .contains("Understood")
        );
    }

    #[tokio::test]
    async fn manual_compact_passes_instructions_to_trigger() {
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use gray_core::agent::{Provider, ToolContext, ToolExecutor};
        use gray_core::event::{StopReason, Usage};
        use gray_core::message::ChatRequest;
        use std::sync::Mutex;

        struct CapturingProvider {
            seen: Arc<Mutex<Vec<ChatRequest>>>,
        }
        #[async_trait]
        impl Provider for CapturingProvider {
            fn stream(
                &self,
                req: ChatRequest,
            ) -> BoxStream<
                'static,
                Result<gray_core::event::StreamEvent, gray_core::agent::ProviderError>,
            > {
                self.seen.lock().expect("seen lock").push(req);
                let events = vec![
                    gray_core::event::StreamEvent::TextDelta {
                        delta: "S".to_string(),
                    },
                    gray_core::event::StreamEvent::MessageComplete {
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Some(Usage::new(10, 5)),
                    },
                ];
                Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
            }
        }
        struct NoopExecutor;
        #[async_trait]
        impl ToolExecutor for NoopExecutor {
            fn execute(
                &self,
                _ctx: &ToolContext,
                _name: &str,
                _args: serde_json::Value,
            ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
                Box::pin(async { gray_core::agent::ToolOutput::ok("") })
            }
        }

        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
        let _home = TempGrayHome::set();
        let seen: Arc<Mutex<Vec<ChatRequest>>> = Arc::default();
        let mut ag = Agent::new(
            Box::new(CapturingProvider { seen: seen.clone() }),
            Arc::new(NoopExecutor),
        );
        ag.set_messages(vec![Message::user("m".repeat(4000))]);

        let out = compact_with_keep(&mut ag, Some("focus on auth"), 0)
            .await
            .expect("must not error");

        assert!(out.is_some());
        let reqs = seen.lock().expect("seen lock");
        assert_eq!(reqs.len(), 1, "one in-band trigger call");
        assert!(
            reqs[0].messages.len() > 1,
            "v2 sends the whole history + trigger, not one serialized prompt"
        );
        let trigger = reqs[0].messages.last().expect("trigger message");
        assert!(
            trigger.text_content().contains("focus on auth"),
            "custom instructions ride the trigger: {}",
            trigger.text_content()
        );
    }

    #[test]
    fn checkpoint_is_private_unpredictable_and_rotated() {
        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
        let _home = TempGrayHome::set();
        let a = write_continuation_checkpoint("summary A", 10).expect("write");
        let b = write_continuation_checkpoint("summary B", 20).expect("write");
        assert_ne!(a.file_name(), b.file_name(), "names must be unpredictable");
        let home = std::env::var("GRAY_HOME").expect("test sets GRAY_HOME");
        assert!(a.starts_with(&home), "under gray home, got {}", a.display());
        assert_eq!(
            a.parent().expect("parent"),
            std::path::Path::new(&home).join("continuation-checkpoints"),
            "per gray-home dir, never temp_dir(), got {}",
            a.display()
        );
        let body = std::fs::read_to_string(&a).expect("read back");
        assert!(body.contains("summary A"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&a).expect("stat").permissions().mode() & 0o777,
                0o600,
                "owner-only"
            );
        }
        for i in 0..7 {
            write_continuation_checkpoint(&format!("s{i}"), i);
        }
        let count = std::fs::read_dir(a.parent().expect("parent"))
            .expect("read dir")
            .count();
        assert_eq!(count, 5, "rotation keeps newest 5, kept {count}");
    }

    /// Serializes tests that flip the global auto-compact switch; also guards
    /// the flag back to enabled even when an assertion panics.
    static COMPACT_SWITCH_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    struct EnableGuard;
    impl Drop for EnableGuard {
        fn drop(&mut self) {
            set_auto_compact_enabled(true);
        }
    }

    mod switch_tests {
        #![allow(clippy::await_holding_lock)] // serial guard must cover each whole test (global switch + env)
        use super::*;
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use gray_core::agent::{Agent, Provider, ToolContext, ToolExecutor};
        use gray_core::event::{StopReason, Usage};
        use gray_core::message::ChatRequest;

        struct FakeProvider;
        #[async_trait]
        impl Provider for FakeProvider {
            fn stream(
                &self,
                _req: ChatRequest,
            ) -> BoxStream<
                'static,
                Result<gray_core::event::StreamEvent, gray_core::agent::ProviderError>,
            > {
                let events = vec![
                    gray_core::event::StreamEvent::TextDelta {
                        delta: "summarized".to_string(),
                    },
                    gray_core::event::StreamEvent::MessageComplete {
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Some(Usage::new(10, 5)),
                    },
                ];
                Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
            }
        }

        struct NoopExecutor;
        #[async_trait]
        impl ToolExecutor for NoopExecutor {
            fn execute(
                &self,
                _ctx: &ToolContext,
                _name: &str,
                _args: serde_json::Value,
            ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
                Box::pin(async { gray_core::agent::ToolOutput::ok("") })
            }
        }

        fn agent() -> Agent {
            // Large enough that the summary pair strictly shrinks history
            // (the enforced shrink invariant refuses toy histories).
            Agent::new(Box::new(FakeProvider), Arc::new(NoopExecutor)).with_messages(vec![
                Message::user("hello ".repeat(500)),
                Message::assistant("hi there ".repeat(500)),
            ])
        }

        #[tokio::test]
        async fn auto_compact_disabled_is_noop() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _guard = EnableGuard;
            set_auto_compact_enabled(false);
            let mut ag = agent();
            let out = auto_compact_if_needed(&mut ag)
                .await
                .expect("must not error when disabled");
            assert!(!out);
            assert_eq!(
                ag.messages().len(),
                2,
                "disabled auto-compact must leave history untouched"
            );
        }

        #[tokio::test]
        async fn env_kill_switch_disables_auto_compact() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _guard = EnableGuard;
            let prev = std::env::var("GRAY_NO_AUTO_COMPACT").ok();
            unsafe { std::env::set_var("GRAY_NO_AUTO_COMPACT", "1") };
            init_auto_compact_from_env();
            let mut ag = agent();
            let out = auto_compact_if_needed(&mut ag)
                .await
                .expect("must not error when disabled");
            assert!(!out);
            assert_eq!(
                ag.messages().len(),
                2,
                "env-disabled auto-compact must leave history untouched"
            );
            match prev {
                Some(v) => unsafe { std::env::set_var("GRAY_NO_AUTO_COMPACT", v) },
                None => unsafe { std::env::remove_var("GRAY_NO_AUTO_COMPACT") },
            }
        }

        #[tokio::test]
        async fn env_unset_leaves_auto_compact_enabled() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _home = TempGrayHome::set();
            let _guard = EnableGuard;
            let prev = std::env::var("GRAY_NO_AUTO_COMPACT").ok();
            unsafe { std::env::remove_var("GRAY_NO_AUTO_COMPACT") };
            init_auto_compact_from_env();
            // Zero keep: with the default 20k keep budget the tail would hold
            // everything and the shrink invariant would (correctly) refuse.
            crate::setup::set_user_keep_recent_tokens(Some(0));
            let mut ag = agent();
            let out = auto_compact_if_needed(&mut ag)
                .await
                .expect("compact should succeed");
            crate::setup::set_user_keep_recent_tokens(None);
            assert!(out);
            assert!(ag.messages()[0].text_content().contains("summarized"));
            match prev {
                Some(v) => unsafe { std::env::set_var("GRAY_NO_AUTO_COMPACT", v) },
                None => unsafe { std::env::remove_var("GRAY_NO_AUTO_COMPACT") },
            }
        }

        #[tokio::test]
        async fn manual_compact_bypasses_disabled_switch() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _home = TempGrayHome::set();
            let _guard = EnableGuard;
            set_auto_compact_enabled(false);
            let mut ag = agent();
            let out = compact_with_keep(&mut ag, None, 0)
                .await
                .expect("manual compact must run when disabled");
            assert!(out.is_some());
            assert!(ag.messages()[0].text_content().contains("summarized"));
        }
    }
}
