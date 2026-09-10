//! REPL unit tests (split from `repl`).

use super::format_core_error;
use gray_core::error::CoreError;

#[test]
fn format_core_error_includes_provider_hint_and_cf_ray() {
    let base = "https://opencode.ai/zen/go/v1";
    let detail = "status 422: boom, cf-ray: abc123-sjc";
    let e = CoreError::Provider(detail.to_string());
    let out = format_core_error(&e, base);
    assert!(
        out.contains("Provider: https://opencode.ai/zen/go/v1 — try /model"),
        "missing provider hint, got: {out}"
    );
    assert!(
        out.contains("cf-ray: abc123-sjc"),
        "missing cf-ray, got: {out}"
    );
}

#[test]
fn format_core_error_plain_no_cf_ray_noise() {
    let base = "https://opencode.ai/zen/go/v1";
    let detail = "status 422: boom";
    let e = CoreError::Provider(detail.to_string());
    let out = format_core_error(&e, base);
    assert!(
        out.contains("Provider: https://opencode.ai/zen/go/v1 — try /model"),
        "missing provider hint, got: {out}"
    );
    assert!(
        !out.contains("cf-ray"),
        "should not contain cf-ray, got: {out}"
    );
}

#[test]
fn format_core_error_bounds_detail_and_marks_retryable() {
    let base = "https://opencode.ai/zen/go/v1";
    let long = format!("status 429: {}", "x".repeat(2000));
    let out = format_core_error(&CoreError::Provider(long), base);
    assert!(
        out.contains("(retryable)"),
        "rate arm must say retryable: {out}"
    );
    assert!(
        out.chars().count() < 1200,
        "detail must be capped, got {} chars",
        out.chars().count()
    );
    let auth = format_core_error(&CoreError::Provider("401 unauthorized nope".into()), base);
    assert!(
        auth.contains("(not retryable)"),
        "auth arm must say not retryable: {auth}"
    );
}

#[test]
fn totals_rebuild_from_stored_entries() {
    let v: serde_json::Value = serde_json::json!({
        "test-persist-model": {
            "max_input_tokens": 100000,
            "input_cost_per_token": 0.000001,
            "output_cost_per_token": 0.000002,
        },
    });
    crate::setup::parse_litellm_context_json(&v);
    let entry =
        |id: u64, text: &str, usage: Option<gray_core::event::Usage>| gray_session::SessionEntry {
            compaction_boundary: false,
            entry_id: id,
            parent_id: if id == 1 { None } else { Some(id - 1) },
            timestamp: 0,
            message: gray_core::message::Message::user(text),
            usage,
            duration_ms: None,
        };
    let entries = vec![
        entry(1, "hi", Some(gray_core::event::Usage::new(1000, 500))),
        entry(2, "yo", None),
        entry(3, "again", Some(gray_core::event::Usage::new(2000, 1000))),
    ];
    let t = super::SessionTotals::from_entries(&entries, "test-persist-model");
    assert_eq!(t.turns, 2);
    assert_eq!(t.input, 3000);
    assert_eq!(t.output, 1500);
    let want = 3000.0 * 0.000001 + 1500.0 * 0.000002;
    assert!((t.cost - want).abs() < 1e-12, "got {}, want {want}", t.cost);
}

#[test]
fn totals_sum_durations_and_skip_untimed() {
    let entry = |id: u64, duration_ms: Option<u64>| gray_session::SessionEntry {
        compaction_boundary: false,
        entry_id: id,
        parent_id: None,
        timestamp: 0,
        message: gray_core::message::Message::user("hi"),
        usage: Some(gray_core::event::Usage::new(10, 5)),
        duration_ms,
    };
    let entries = vec![entry(0, Some(6000)), entry(1, Some(4000)), entry(2, None)];
    let t = super::SessionTotals::from_entries(&entries, "test-persist-model");
    assert_eq!(t.turns, 3);
    assert_eq!(t.total_duration_ms, 10_000);
    assert_eq!(t.timed_turns, 2);
}

#[test]
fn turn_footer_includes_duration_when_known() {
    let usage = gray_core::event::Usage::new(1000, 500);
    let totals = super::SessionTotals::default();
    let line = super::turn_footer(&usage, "test-persist-model", &totals, Some(6500));
    assert!(line.contains("6.5s"), "footer should show time: {line}");
    assert!(line.contains("tok"), "footer should keep tokens: {line}");
}

/// Stub plugin claiming `/echo`, like a sidecar manifest with
/// `commands:["/echo"]` answering `command/run` with `{"text"}`.
struct EchoHook;

#[async_trait::async_trait]
impl gray_core::agent::PluginHooks for EchoHook {
    fn commands(&self) -> Vec<gray_core::agent::PluginCommand> {
        vec![gray_core::agent::PluginCommand {
            name: "/echo".to_string(),
            description: "echo back argv".to_string(),
        }]
    }

    async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<super::CommandOutcome> {
        (name == "/echo").then(|| super::CommandOutcome::Say(argv.join(" ")))
    }
}

/// Stub plugin claiming `/ask`, answering `command/run` with
/// `{"prompt"}` (asks the host to submit a turn instead of printing).
struct PromptHook;

#[async_trait::async_trait]
impl gray_core::agent::PluginHooks for PromptHook {
    fn commands(&self) -> Vec<gray_core::agent::PluginCommand> {
        vec![gray_core::agent::PluginCommand {
            name: "/ask".to_string(),
            description: "submit argv as a prompt".to_string(),
        }]
    }

    async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<super::CommandOutcome> {
        (name == "/ask").then(|| super::CommandOutcome::Prompt(argv.join(" ")))
    }
}

fn echo_hooks() -> Vec<std::sync::Arc<dyn gray_core::agent::PluginHooks>> {
    vec![std::sync::Arc::new(EchoHook)]
}

#[test]
fn plugin_command_split_parses_name_and_argv() {
    assert_eq!(
        super::split_plugin_command("/echo hi there"),
        Some((
            "/echo".to_string(),
            vec!["hi".to_string(), "there".to_string()]
        ))
    );
    assert_eq!(
        super::split_plugin_command("/echo"),
        Some(("/echo".to_string(), vec![]))
    );
    assert_eq!(super::split_plugin_command("hi"), None);
    assert_eq!(super::split_plugin_command("/"), None);
    assert_eq!(super::split_plugin_command(""), None);
}

#[tokio::test]
async fn plugin_command_routes_claimed_to_owner() {
    let hooks = echo_hooks();
    let (name, argv) = super::split_plugin_command("/echo hi").expect("splits");
    let out = super::run_plugin_command(&hooks, &name, argv).await;
    assert_eq!(out, Some(super::CommandOutcome::Say("hi".to_string())));
}

#[tokio::test]
async fn plugin_command_prompt_reply_takes_prompt_path() {
    use super::CommandOutcome;
    let hooks: Vec<std::sync::Arc<dyn gray_core::agent::PluginHooks>> =
        vec![std::sync::Arc::new(PromptHook)];
    let (name, argv) = super::split_plugin_command("/ask write tests").expect("splits");
    let out = super::run_plugin_command(&hooks, &name, argv).await;
    // Prompt replies stay distinct from Say: the `Unknown` handler
    // routes this into `pending_command = ReplCommand::Prompt`, not `say()`.
    assert!(
        matches!(out, Some(CommandOutcome::Prompt(ref p)) if p == "write tests"),
        "got {out:?}"
    );
    // And the text path is untouched.
    let hooks = echo_hooks();
    let (name, argv) = super::split_plugin_command("/echo hi").expect("splits");
    let out = super::run_plugin_command(&hooks, &name, argv).await;
    assert!(
        matches!(out, Some(CommandOutcome::Say(ref s)) if s == "hi"),
        "got {out:?}"
    );
}

#[tokio::test]
async fn plugin_command_unclaimed_returns_none() {
    let hooks = echo_hooks();
    let (name, argv) = super::split_plugin_command("/nope hi").expect("splits");
    assert_eq!(super::run_plugin_command(&hooks, &name, argv).await, None);
    // No hooks at all: same None, so the unknown-command message stays.
    let empty: Vec<std::sync::Arc<dyn gray_core::agent::PluginHooks>> = Vec::new();
    let (name, argv) = super::split_plugin_command("/echo hi").expect("splits");
    assert_eq!(super::run_plugin_command(&empty, &name, argv).await, None);
}

#[test]
fn plugin_help_lists_claimed_commands() {
    let entries = super::plugin_help_entries(&echo_hooks());
    assert!(entries.iter().any(|(n, _)| n == "echo"), "got {entries:?}");
    let empty: Vec<std::sync::Arc<dyn gray_core::agent::PluginHooks>> = Vec::new();
    assert!(super::plugin_help_entries(&empty).is_empty());
}
