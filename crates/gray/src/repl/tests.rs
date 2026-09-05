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
fn gateway_args_parse_all_actions() {
    use super::GatewayAction as G;
    use gray_gateway::config::Platform;
    assert!(matches!(super::parse_gateway_args("/gateway"), G::Status));
    assert!(matches!(
        super::parse_gateway_args("/gateway status"),
        G::Status
    ));
    assert!(matches!(super::parse_gateway_args("/gateway run"), G::Run));
    assert!(matches!(
        super::parse_gateway_args("/gateway stop"),
        G::Stop
    ));
    assert!(matches!(
        super::parse_gateway_args("/gateway install"),
        G::Install
    ));
    assert!(matches!(
        super::parse_gateway_args("/gateway uninstall"),
        G::Uninstall
    ));
    assert!(matches!(
        super::parse_gateway_args("/gateway bogus"),
        G::Help
    ));
    match super::parse_gateway_args("/gateway connect discord abc123") {
        G::Connect(Platform::Discord, tok) => assert_eq!(tok, "abc123"),
        other => panic!("expected connect discord, got {other:?}"),
    }
    match super::parse_gateway_args("/gateway connect TELEGRAM  123:XYZ extra") {
        G::Connect(Platform::Telegram, tok) => assert_eq!(tok, "123:XYZ"), // token = first arg after platform
        other => panic!("expected connect telegram, got {other:?}"),
    }
    assert!(matches!(
        super::parse_gateway_args("/gateway connect slack"), // no token
        G::Help
    ));
    match super::parse_gateway_args("/gateway pairing approve discord ABC123") {
        G::Pairing(super::PairingArgs::Approve(p, c)) => {
            assert_eq!((p, c), ("discord".to_string(), "ABC123".to_string()))
        }
        other => panic!("expected pairing approve, got {other:?}"),
    }
    assert!(matches!(
        super::parse_gateway_args("/gateway pairing list"),
        G::Pairing(super::PairingArgs::List(None))
    ));
    match super::parse_gateway_args("/gateway pairing revoke discord 123") {
        G::Pairing(super::PairingArgs::Revoke(p, u)) => {
            assert_eq!((p, u), ("discord".to_string(), "123".to_string()))
        }
        other => panic!("expected pairing revoke, got {other:?}"),
    }
    assert!(matches!(
        super::parse_gateway_args("/gateway pairing approve discord"),
        G::Help
    ));
    match super::parse_gateway_args("/gateway disconnect slack") {
        G::Disconnect(Platform::Slack) => {}
        other => panic!("expected disconnect slack, got {other:?}"),
    }
    match super::parse_gateway_args("/gateway enable Telegram") {
        G::Enable(Platform::Telegram) => {}
        other => panic!("expected enable telegram, got {other:?}"),
    }
    assert!(matches!(
        super::parse_gateway_args("/gateway autostart on"),
        G::Autostart(true)
    ));
    assert!(matches!(
        super::parse_gateway_args("/gateway autostart OFF"),
        G::Autostart(false)
    ));
    assert!(matches!(
        super::parse_gateway_args("/gateway autostart"),
        G::Help
    ));
    assert!(matches!(
        super::parse_gateway_args("/gateway autostart maybe"),
        G::Help
    ));
    // default-off: fresh config does not autostart (S2)
    assert!(!gray_gateway::config::GatewayConfig::default().autostart);
}

#[test]
fn gateway_connect_disconnect_roundtrip() {
    let mut cfg = gray_gateway::config::GatewayConfig::default();
    super::apply_connect(&mut cfg, gray_gateway::config::Platform::Telegram, "tok-1");
    let pc = cfg
        .platforms
        .get(&gray_gateway::config::Platform::Telegram)
        .unwrap();
    assert!(pc.enabled && pc.token.as_deref() == Some("tok-1"));
    super::apply_disconnect(&mut cfg, gray_gateway::config::Platform::Telegram);
    let pc = cfg
        .platforms
        .get(&gray_gateway::config::Platform::Telegram)
        .unwrap();
    assert!(!pc.enabled && pc.token.as_deref() == Some("tok-1")); // token kept
    assert!(super::apply_enable(
        &mut cfg,
        gray_gateway::config::Platform::Telegram
    ));
    let pc = cfg
        .platforms
        .get(&gray_gateway::config::Platform::Telegram)
        .unwrap();
    assert!(pc.enabled && pc.token.as_deref() == Some("tok-1"));
    assert!(!super::apply_enable(
        &mut cfg,
        gray_gateway::config::Platform::Slack
    )); // no token
}

#[test]
fn gateway_status_lines_hide_tokens() {
    let mut cfg = gray_gateway::config::GatewayConfig::default();
    super::apply_connect(
        &mut cfg,
        gray_gateway::config::Platform::Discord,
        "secret-token",
    );
    let lines = super::gateway_status_lines(&cfg, true);
    let joined = lines.join("\n");
    assert!(joined.contains("Discord"), "should list platform: {joined}");
    assert!(
        joined.contains("connected"),
        "should show in-process running state: {joined}"
    );
    assert!(
        !joined.contains("secret-token"),
        "token must never render: {joined}"
    );
    let lines = super::gateway_status_lines(&cfg, false);
    assert!(lines.join("\n").contains("not running"));
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

#[test]
fn gateway_boot_rows_indent_and_states() {
    use gray_gateway::config::Platform;
    use gray_gateway::status::{GatewayStatusBoard, PlatformConnState as S};
    let b = GatewayStatusBoard::new(&[Platform::Discord, Platform::Telegram]);
    // Canonical order (Telegram first), connecting, two-space card indent.
    assert_eq!(
        super::gateway_boot_rows(&b),
        vec![
            "  ├─ Telegram — connecting…".to_string(),
            "  └─ Discord — connecting…".to_string(),
        ]
    );
    // Staged progress surfaces inline: `└─ {Platform} — {stage}…`.
    b.mark_stage(Platform::Telegram, "validating token");
    b.mark_stage(Platform::Discord, "waiting for ready");
    assert_eq!(
        super::gateway_boot_rows(&b),
        vec![
            "  ├─ Telegram — validating token…".to_string(),
            "  └─ Discord — waiting for ready…".to_string(),
        ]
    );
    b.mark_connected(Platform::Discord, Some("GrayBot".into()));
    b.mark_connected(Platform::Telegram, None);
    assert_eq!(
        super::gateway_boot_rows(&b),
        vec![
            "  ├─ Telegram — connected".to_string(),
            "  └─ Discord — connected as GrayBot".to_string(),
        ]
    );
    // Failed state surfaces the error inline (same single card).
    b.mark_failed(Platform::Telegram, "token rejected");
    let rows = super::gateway_boot_rows(&b);
    assert_eq!(rows[0], "  ├─ Telegram — connect failed: token rejected");
    // No identity leak: rows never contain tokens, only display names.
    assert!(!rows.join("\n").contains("secret"));
    let _ = S::Connecting {
        stage: "connecting",
    };
}

/// Stub plugin claiming `/echo`, like a sidecar manifest with
/// `commands:["/echo"]` answering `command/run`.
struct EchoHook;

#[async_trait::async_trait]
impl gray_core::agent::PluginHooks for EchoHook {
    fn commands(&self) -> Vec<gray_core::agent::PluginCommand> {
        vec![gray_core::agent::PluginCommand {
            name: "/echo".to_string(),
            description: "echo back argv".to_string(),
        }]
    }

    async fn run_command(&self, name: &str, argv: Vec<String>) -> Option<String> {
        (name == "/echo").then(|| argv.join(" "))
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
    assert_eq!(out.as_deref(), Some("hi"));
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
