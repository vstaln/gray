//! hermes-cli config_defaults — slice 1/6
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/config_defaults.py`
//! slice 1/6 — lines 1–900 of 4 962 (first 900).
//! Covers: module docstring (pure-data leaf: DEFAULT_CONFIG + OPTIONAL_ENV_VARS),
//! `DEFAULT_CONFIG` top-level keys through `compression` mid-section:
//! `model`, `providers`, `fallback_providers`, `credential_pool_strategies`,
//! `toolsets`, `database` (journal_mode, wal_autocheckpoint, journal_size_limit),
//! `runtime` (nofile_soft_limit), `max_concurrent_sessions`, `max_live_sessions`,
//! `session` (terminal_continue), `agent` (max_turns, run_budget_seconds,
//! gateway_timeout, gateway_turn_lease_timeout, agent_cache, restart_drain_timeout,
//! cron_drain_timeout, restart_after_turn_timeout, build_wait_timeout, api_max_retries,
//! empty_response_guard, service_tier, tool_use_enforcement, execution_guidance,
//! intent_ack_continuation, stall_guards, task_completion_guidance,
//! parallel_tool_call_guidance, environment_probe, bot_mode_protocol, environment_hint,
//! coding_context, coding_instructions, verify_guidance, max_verify_nudges,
//! verify_on_stop, gateway_timeout_warning, clarify_timeout, gateway_notify_interval,
//! session_stall_timeout, reconnect_attention_after, gateway_auto_continue_freshness,
//! gateway_startup_restore_drain_timeout, local_stream_stale_timeout, image_input_mode,
//! disabled_toolsets, reasoning_overrides, reasoning_echo) through `agent` tail,
//! `terminal` (backend, modal_mode, degraded_mode, cwd, font_family, timeout,
//! daemon_term_grace_seconds, oneshot_completion_wait_seconds, env_passthrough,
//! home_mode, shell_init_files, auto_source_bashrc, docker_image, docker_forward_env,
//! docker_env, singularity_image, modal_image, daytona_image, vercel_runtime,
//! container_cpu/memory/disk/persistent, docker_volumes, docker_mount_cwd_to_workspace,
//! docker_network, docker_extra_args, docker_shm_size, docker_run_as_host_user,
//! persistent_shell), `web` (backend, search_backend, extract_backend,
//! extract_char_limit, keyless_fallback, keyless_rescue, provider_tier),
//! `browser` (backend, inactivity_timeout, command_timeout, record_sessions, headed,
//! allow_private_urls, engine, auto_local_for_private_urls, cdp_url,
//! allow_unsafe_evaluate, restrict_evaluate, dialog_policy, dialog_timeout_s, camofox,
//! extension_control), `checkpoints` (enabled, max_snapshots, max_total_size_mb,
//! max_file_size_mb, auto_prune, retention_days, min_interval_hours),
//! `context_file_max_chars`, `file_read_max_chars`, `mcp_discovery_timeout`,
//! `mcp_single_query_discovery_timeout`, `mcp` (auto_reload_on_config_change),
//! `tool_output` (max_bytes, max_lines, max_line_length), `tool_loop_guardrails`
//! (warnings_enabled, hard_stop_enabled, warn_after, hard_stop_after, loop_caps),
//! `compression` head (enabled, progress_notices, threshold, threshold_tokens,
//! target_ratio, tail_mode, protect_last_n, min_tail_user_messages, max_attempts,
//! proactive_prune_tokens, proactive_prune_min_result_chars,
//! proactive_prune_min_reclaim_tokens, micro_compact, micro_compact_every_n_turns,
//! micro_compact_defrag_threshold_tokens, hygiene_hard_message_limit,
//! hygiene_timeout_seconds) through line ~900 (compression hygiene_*). Continued
//! in `config_defaults_slice2.rs` (compression tail + prompt_caching/openrouter/…).
//!
//! T0689 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Module doc — mirrors lines 1-5
// ---------------------------------------------------------------------------

/// Pure-data leaf module: DEFAULT_CONFIG and OPTIONAL_ENV_VARS, extracted
/// verbatim from hermes_cli/config.py. Must not import from hermes_cli.config.
/// Mirrors `hermes_cli/config_defaults.py` lines 1-5.
pub const MODULE_DOC: &str =
    "Default configuration data for Hermes Agent — pure-data leaf (DEFAULT_CONFIG + OPTIONAL_ENV_VARS)";

// ---------------------------------------------------------------------------
// DEFAULT_CONFIG — top-level scalar keys — mirrors lines 7-12
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CONFIG["model"] = ""` (line 8).
pub const DEFAULT_MODEL: &str = "";

/// Mirrors `DEFAULT_CONFIG["toolsets"] = ["hermes-cli"]` (line 12).
pub const DEFAULT_TOOLSETS: &[&str] = &["hermes-cli"];

// ---------------------------------------------------------------------------
// database — mirrors lines 13-22
// ---------------------------------------------------------------------------

/// SQLite journal mode used by every Hermes database opener. WAL is the
/// normal default; set DELETE for weak-fsync/shared filesystems where WAL is
/// not crash-safe (for example macOS virtiofs, NFS, or SMB).
/// Mirrors `DEFAULT_CONFIG["database"]` (lines 13-22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseConfig {
    /// Mirrors `journal_mode: "wal"` (line 17).
    pub journal_mode: &'static str,
    /// Optional WAL sizing pragmas, applied when set to integers.
    /// None = SQLite defaults (autocheckpoint 1000 pages, no size limit).
    /// Mirrors `wal_autocheckpoint: None` (line 20).
    pub wal_autocheckpoint: Option<i64>,
    /// Mirrors `journal_size_limit: None` (line 21).
    pub journal_size_limit: Option<i64>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            journal_mode: "wal",
            wal_autocheckpoint: None,
            journal_size_limit: None,
        }
    }
}

// ---------------------------------------------------------------------------
// runtime — mirrors lines 23-27
// ---------------------------------------------------------------------------

/// Soft file-descriptor limit for long-running Hermes server processes.
/// Clamped to the OS hard limit; 0/false/null disables the adjustment.
/// Mirrors `DEFAULT_CONFIG["runtime"]` (lines 23-27).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Mirrors `nofile_soft_limit: 4096` (line 26).
    pub nofile_soft_limit: u32,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            nofile_soft_limit: 4096,
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level session caps — mirrors lines 28-35
// ---------------------------------------------------------------------------

/// Global active chat session cap across CLI, TUI/dashboard, and messaging.
/// None/0 = unbounded. Mirrors `max_concurrent_sessions: None` (line 30).
pub const DEFAULT_MAX_CONCURRENT_SESSIONS: Option<u32> = None;

/// Soft LRU cap on in-memory TUI/desktop/dashboard sessions.
/// Mirrors `max_live_sessions: 16` (line 35).
pub const DEFAULT_MAX_LIVE_SESSIONS: Option<u32> = Some(16);

// ---------------------------------------------------------------------------
// session — mirrors lines 36-44
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CONFIG["session"]` (lines 36-44).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// Per-terminal `hermes -c`: each CLI session drops a breadcrumb file
    /// under $HERMES_HOME/terminal-sessions/<terminal-id>, and a bare
    /// -c/--continue resumes THIS terminal's session (tmux pane, kitty
    /// window, wezterm pane, plain tty, ...) instead of the globally
    /// most-recent one. Set false to restore the old latest-session
    /// behavior everywhere. Mirrors `terminal_continue: True` (line 43).
    pub terminal_continue: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            terminal_continue: true,
        }
    }
}

// ---------------------------------------------------------------------------
// agent.agent_cache — mirrors lines 71-89
// ---------------------------------------------------------------------------

/// Per-session AIAgent cache in the gateway. Each cached agent keeps a
/// warm prompt prefix AND the session's full transcript, so the cache
/// trades memory for cost: too small and every turn re-pays an uncached
/// prompt, too large and tool-heavy transcripts fill the heap.
/// Mirrors `DEFAULT_CONFIG["agent"]["agent_cache"]` (lines 71-89).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentCacheConfig {
    /// LRU entry cap. Mirrors `max_size: 128` (line 73).
    pub max_size: u32,
    /// Evict an agent that has been idle this long (seconds).
    /// Mirrors `idle_ttl_secs: 3600` (line 75).
    pub idle_ttl_secs: u64,
    /// Anonymous-RSS budget (MB) above which the gateway starts shedding
    /// least-recently-used transcripts, which reload from the persisted
    /// session on the next turn. "auto" derives the budget from the
    /// cgroup memory limit the gateway runs under (or total RAM when
    /// uncapped); a number sets it explicitly; 0/off disables the pass
    /// and lets memory grow to whatever the two bounds above allow.
    /// Mirrors `memory_high_mb: "auto"` (line 82).
    pub memory_high_mb: &'static str,
    /// Upper bound on how many sessions one pressure pass sheds, so a
    /// burst of teardowns cannot stall the gateway.
    /// Mirrors `max_evictions_per_pass: 16` (line 85).
    pub max_evictions_per_pass: u32,
    /// Most-recently-used sessions the pressure pass never touches —
    /// they are the ones actively paying for a warm prompt cache.
    /// Mirrors `protect_recent: 8` (line 88).
    pub protect_recent: u32,
}

impl Default for AgentCacheConfig {
    fn default() -> Self {
        Self {
            max_size: 128,
            idle_ttl_secs: 3600,
            memory_high_mb: "auto",
            max_evictions_per_pass: 16,
            protect_recent: 8,
        }
    }
}

// ---------------------------------------------------------------------------
// agent.empty_response_guard — mirrors lines 142-151
// ---------------------------------------------------------------------------

/// Empty-response retry guard (NS-503).
/// Mirrors `DEFAULT_CONFIG["agent"]["empty_response_guard"]` (lines 142-151).
#[derive(Debug, Clone, PartialEq)]
pub struct EmptyResponseGuardConfig {
    /// Master switch for both guards below. False restores the
    /// legacy fixed 3-retry behaviour unconditionally.
    /// Mirrors `enabled: True` (line 145).
    pub enabled: bool,
    /// When the estimated input cost of a single empty attempt
    /// meets or exceeds this many USD, the retry budget for the
    /// streak drops from 3 to 1. Unknown pricing or missing usage
    /// leaves the budget untouched.
    /// Mirrors `cost_threshold_usd: 0.25` (line 150).
    pub cost_threshold_usd: f64,
}

impl Default for EmptyResponseGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cost_threshold_usd: 0.25,
        }
    }
}

// ---------------------------------------------------------------------------
// agent — mirrors lines 45-373
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CONFIG["agent"]` (lines 45-373).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentConfig {
    /// Unlimited by default. The agent turn cap caused more problems than
    /// it solved (silent mid-task truncation). null = unlimited; set a
    /// positive integer to cap, or use "none"/"unlimited"/"inf"/0/-1 —
    /// all normalized by hermes_cli.config.resolve_turn_limit.
    /// Mirrors `max_turns: None` (line 50).
    pub max_turns: Option<u32>,
    /// Optional wall-clock budget in seconds per conversation run.
    /// null/absent = feature fully off (zero behavior change). When set,
    /// the agent gets a one-time wrap-up notice at 80% elapsed and
    /// implicit provider stale timeouts are capped to the remaining
    /// budget. CLI one-shot equivalent: `hermes chat --run-budget N`.
    /// Mirrors `run_budget_seconds: None` (line 56).
    pub run_budget_seconds: Option<u64>,
    /// Inactivity timeout for gateway agent execution (seconds).
    /// Mirrors `gateway_timeout: 1800` (line 61).
    pub gateway_timeout: u64,
    /// Maximum time an alias routing key waits for the active turn holding
    /// the same resolved session lease. On expiry the inbound message is
    /// rejected with a resend notice rather than run without serialization.
    /// Mirrors `gateway_turn_lease_timeout: 1800` (line 66).
    pub gateway_turn_lease_timeout: u64,
    /// Per-session AIAgent cache. Mirrors `agent_cache` (lines 71-89).
    pub agent_cache: AgentCacheConfig,
    /// Force-interrupt budget once gateway stop()/drain has begun
    /// (seconds). Mirrors `restart_drain_timeout: 0` (line 99).
    pub restart_drain_timeout: u64,
    /// Cron-only floor under the stop()/drain wait (seconds).
    /// Mirrors `cron_drain_timeout: 30` (line 108).
    pub cron_drain_timeout: u64,
    /// In-band restart wait for active turns to finish before stop()
    /// (seconds). Mirrors `restart_after_turn_timeout: 1800` (line 118).
    pub restart_after_turn_timeout: u64,
    /// Upper bound (seconds) a submitted prompt waits for the deferred
    /// agent build (MCP discovery, model metadata, skills scan) before
    /// failing with a visible error (#63078).
    /// Mirrors `build_wait_timeout: 600` (line 126).
    pub build_wait_timeout: u64,
    /// Max app-level retry attempts for API errors (connection drops,
    /// provider timeouts, 5xx, etc.) before the agent surfaces the
    /// failure. Mirrors `api_max_retries: 3` (line 135).
    pub api_max_retries: u32,
    /// Empty-response retry guard. Mirrors `empty_response_guard` (lines 142-151).
    pub empty_response_guard: EmptyResponseGuardConfig,
    /// Mirrors `service_tier: ""` (line 152).
    pub service_tier: &'static str,
    /// Tool-use enforcement: injects system prompt guidance that tells the
    /// model to actually call tools instead of describing intended actions.
    /// Values: "auto" (default — applies to gpt/codex models), true/false
    /// (force on/off for all models), or a list of model-name substrings
    /// to match (e.g. ["gpt", "codex", "gemini", "qwen"]).
    /// Mirrors `tool_use_enforcement: "auto"` (line 158).
    pub tool_use_enforcement: &'static str,
    /// Execution-discipline guidance. Mirrors `execution_guidance: "auto"` (line 167).
    pub execution_guidance: &'static str,
    /// Intent-ack continuation. Mirrors `intent_ack_continuation: "auto"` (line 177).
    pub intent_ack_continuation: &'static str,
    /// Runtime anti-stall guards. Mirrors `stall_guards: True` (line 186).
    pub stall_guards: bool,
    /// Universal "finish the job" guidance. Mirrors `task_completion_guidance: True` (line 192).
    pub task_completion_guidance: bool,
    /// Universal parallel-tool-call guidance. Mirrors `parallel_tool_call_guidance: True` (line 201).
    pub parallel_tool_call_guidance: bool,
    /// Local-environment toolchain probe. Mirrors `environment_probe: True` (line 209).
    pub environment_probe: bool,
    /// Bot Mode teammate-messaging protocol section (silent unless a
    /// profile is managed by the desktop's Bot Mode).
    /// Mirrors `bot_mode_protocol: True` (line 212).
    pub bot_mode_protocol: bool,
    /// Embedder-supplied environment description appended to the system
    /// prompt's environment-hints block.
    /// Mirrors `environment_hint: ""` (line 219).
    pub environment_hint: &'static str,
    /// Coding posture — on interactive coding surfaces (CLI, TUI, desktop
    /// app, ACP) in a code workspace, Hermes adds a coding operating brief
    /// + a live git/workspace snapshot to the system prompt.
    /// Mirrors `coding_context: "auto"` (line 234).
    pub coding_context: &'static str,
    /// Standing operator instructions for the coding posture.
    /// Mirrors `coding_instructions: ""` (line 241).
    pub coding_instructions: &'static str,
    /// When verify-on-stop finds edited code without fresh verification
    /// evidence, append guidance for creative UI work and clean-diff expectations.
    /// Mirrors `verify_guidance: True` (line 246).
    pub verify_guidance: bool,
    /// Upper bound on consecutive `pre_verify` "continue" nudges in a single
    /// turn. Mirrors `max_verify_nudges: 3` (line 249).
    pub max_verify_nudges: u32,
    /// Verification closure: after the agent edits files in a code workspace,
    /// do not accept a final answer until fresh verification evidence exists
    /// or the agent explains why it cannot run checks.
    /// Mirrors `verify_on_stop: False` (line 262).
    pub verify_on_stop: bool,
    /// Staged inactivity warning: send a warning to the user at this
    /// threshold before escalating to a full timeout.
    /// Mirrors `gateway_timeout_warning: 900` (line 266).
    pub gateway_timeout_warning: u64,
    /// Maximum time (seconds) the gateway will block an agent waiting for
    /// a clarify-tool response from the user.
    /// Mirrors `clarify_timeout: 3600` (line 278).
    pub clarify_timeout: u64,
    /// Periodic "still working" notification interval (seconds).
    /// Mirrors `gateway_notify_interval: 180` (line 286).
    pub gateway_notify_interval: u64,
    /// Session stall watchdog (seconds).
    /// Mirrors `session_stall_timeout: 300` (line 297).
    pub session_stall_timeout: u64,
    /// Long-lived reconnect-loop escalation (seconds).
    /// Mirrors `reconnect_attention_after: 7200` (line 303).
    pub reconnect_attention_after: u64,
    /// Freshness window for the gateway auto-continue note (seconds).
    /// Mirrors `gateway_auto_continue_freshness: 3600` (line 317).
    pub gateway_auto_continue_freshness: u64,
    /// Max seconds the gateway waits for boot auto-resume turns to finish
    /// before it releases the startup-restore inbound gate.
    /// Mirrors `gateway_startup_restore_drain_timeout: 30` (line 329).
    pub gateway_startup_restore_drain_timeout: u64,
    /// Stale-stream ceiling for local providers (Ollama, oMLX, llama-cpp) in
    /// seconds. Mirrors `local_stream_stale_timeout: 900` (line 336).
    pub local_stream_stale_timeout: u64,
    /// How user-attached images are presented to the main model on each turn.
    /// Mirrors `image_input_mode: "auto"` (line 350).
    pub image_input_mode: &'static str,
    /// Per-model reasoning effort overrides (spelling-tolerant).
    /// Mirrors `reasoning_overrides: {}` (line 358) — empty by default.
    pub reasoning_echo: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: None,
            run_budget_seconds: None,
            gateway_timeout: 1800,
            gateway_turn_lease_timeout: 1800,
            agent_cache: AgentCacheConfig::default(),
            restart_drain_timeout: 0,
            cron_drain_timeout: 30,
            restart_after_turn_timeout: 1800,
            build_wait_timeout: 600,
            api_max_retries: 3,
            empty_response_guard: EmptyResponseGuardConfig::default(),
            service_tier: "",
            tool_use_enforcement: "auto",
            execution_guidance: "auto",
            intent_ack_continuation: "auto",
            stall_guards: true,
            task_completion_guidance: true,
            parallel_tool_call_guidance: true,
            environment_probe: true,
            bot_mode_protocol: true,
            environment_hint: "",
            coding_context: "auto",
            coding_instructions: "",
            verify_guidance: true,
            max_verify_nudges: 3,
            verify_on_stop: false,
            gateway_timeout_warning: 900,
            clarify_timeout: 3600,
            gateway_notify_interval: 180,
            session_stall_timeout: 300,
            reconnect_attention_after: 7200,
            gateway_auto_continue_freshness: 3600,
            gateway_startup_restore_drain_timeout: 30,
            local_stream_stale_timeout: 900,
            image_input_mode: "auto",
            reasoning_echo: false,
        }
    }
}

// ---------------------------------------------------------------------------
// terminal — mirrors lines 375-503
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CONFIG["terminal"]` (lines 375-503).
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalConfig {
    /// Mirrors `backend: "local"` (line 376).
    pub backend: &'static str,
    /// Mirrors `modal_mode: "auto"` (line 377).
    pub modal_mode: &'static str,
    /// Remote-backend graceful degradation.
    /// Mirrors `degraded_mode: "warn"` (line 383).
    pub degraded_mode: &'static str,
    /// Mirrors `cwd: "."` (line 384).
    pub cwd: &'static str,
    /// Terminal font family for the desktop app's embedded xterm.js terminal.
    /// Mirrors `font_family: ""` (line 392).
    pub font_family: &'static str,
    /// Mirrors `timeout: 180` (line 393).
    pub timeout: u64,
    /// Bounded grace period (seconds) between SIGTERM and an escalated
    /// SIGKILL when terminating a host process tree.
    /// Mirrors `daemon_term_grace_seconds: 2.0` (line 399).
    pub daemon_term_grace_seconds: f64,
    /// Bounded linger (seconds) for one-shot CLI runs (-q/-Q/-z) that exit
    /// while background processes spawned with notify_on_complete=true are
    /// still running. Mirrors `oneshot_completion_wait_seconds: 600.0` (line 410).
    pub oneshot_completion_wait_seconds: f64,
    /// Mirrors `home_mode: "auto"` (line 421).
    pub home_mode: &'static str,
    /// When true (default), Hermes sources the user's shell rc files.
    /// Mirrors `auto_source_bashrc: True` (line 447).
    pub auto_source_bashrc: bool,
    /// Mirrors `docker_image: "nikolaik/python-nodejs:python3.11-nodejs20"` (line 448).
    pub docker_image: &'static str,
    /// Mirrors `singularity_image: "docker://nikolaik/python-nodejs:python3.11-nodejs20"` (line 456).
    pub singularity_image: &'static str,
    /// Mirrors `modal_image: "nikolaik/python-nodejs:python3.11-nodejs20"` (line 457).
    pub modal_image: &'static str,
    /// Mirrors `daytona_image: "nikolaik/python-nodejs:python3.11-nodejs20"` (line 458).
    pub daytona_image: &'static str,
    /// Vercel Sandbox runtime (vercel_sandbox backend only).
    /// Mirrors `vercel_runtime: "node24"` (line 461).
    pub vercel_runtime: &'static str,
    /// Container resource limits (docker, singularity, modal, daytona, vercel_sandbox — ignored for local/ssh)
    /// Mirrors `container_cpu: 1` (line 463).
    pub container_cpu: u32,
    /// Mirrors `container_memory: 5120` (line 464) — MB (default 5GB).
    pub container_memory: u32,
    /// Mirrors `container_disk: 51200` (line 465) — MB (default 50GB).
    pub container_disk: u32,
    /// Persist filesystem across sessions. Mirrors `container_persistent: True` (line 466).
    pub container_persistent: bool,
    /// Explicit opt-in: mount the host cwd into /workspace for Docker sessions.
    /// Mirrors `docker_mount_cwd_to_workspace: False` (line 477).
    pub docker_mount_cwd_to_workspace: bool,
    /// Opt-in egress lockdown for Docker terminal sessions.
    /// Mirrors `docker_network: True` (line 480).
    pub docker_network: bool,
    /// /dev/shm size for the Docker sandbox.
    /// Mirrors `docker_shm_size: "1g"` (line 486).
    pub docker_shm_size: &'static str,
    /// Explicit opt-in: run the Docker container as the host user's uid:gid.
    /// Mirrors `docker_run_as_host_user: False` (line 497).
    pub docker_run_as_host_user: bool,
    /// Persistent shell — keep a long-lived bash shell across execute() calls.
    /// Mirrors `persistent_shell: True` (line 502).
    pub persistent_shell: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            backend: "local",
            modal_mode: "auto",
            degraded_mode: "warn",
            cwd: ".",
            font_family: "",
            timeout: 180,
            daemon_term_grace_seconds: 2.0,
            oneshot_completion_wait_seconds: 600.0,
            home_mode: "auto",
            auto_source_bashrc: true,
            docker_image: "nikolaik/python-nodejs:python3.11-nodejs20",
            singularity_image: "docker://nikolaik/python-nodejs:python3.11-nodejs20",
            modal_image: "nikolaik/python-nodejs:python3.11-nodejs20",
            daytona_image: "nikolaik/python-nodejs:python3.11-nodejs20",
            vercel_runtime: "node24",
            container_cpu: 1,
            container_memory: 5120,
            container_disk: 51200,
            container_persistent: true,
            docker_mount_cwd_to_workspace: false,
            docker_network: true,
            docker_shm_size: "1g",
            docker_run_as_host_user: false,
            persistent_shell: true,
        }
    }
}

// ---------------------------------------------------------------------------
// web — mirrors lines 505-530
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CONFIG["web"]` (lines 505-530).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConfig {
    /// shared fallback — applies to both search and extract.
    /// Mirrors `backend: ""` (line 506).
    pub backend: &'static str,
    /// per-capability override for web_search (e.g. "searxng").
    /// Mirrors `search_backend: ""` (line 507).
    pub search_backend: &'static str,
    /// per-capability override for web_extract (e.g. "native").
    /// Mirrors `extract_backend: ""` (line 508).
    pub extract_backend: &'static str,
    /// per-page char budget for web_extract; larger pages truncate + store full text in cache/web.
    /// Mirrors `extract_char_limit: 15000` (line 509).
    pub extract_char_limit: u32,
    /// Keyless free-tier ring. Mirrors `keyless_fallback: True` (line 515).
    pub keyless_fallback: bool,
    /// One-shot keyless rescue. Mirrors `keyless_rescue: True` (line 520).
    pub keyless_rescue: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            backend: "",
            search_backend: "",
            extract_backend: "",
            extract_char_limit: 15000,
            keyless_fallback: true,
            keyless_rescue: true,
        }
    }
}

// ---------------------------------------------------------------------------
// browser.camofox — mirrors lines 566-582
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CONFIG["browser"]["camofox"]` (lines 566-582).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserCamofoxConfig {
    /// Mirrors `managed_persistence: False` (line 570).
    pub managed_persistence: bool,
    /// Mirrors `user_id: ""` (line 573).
    pub user_id: &'static str,
    /// Mirrors `session_key: ""` (line 574).
    pub session_key: &'static str,
    /// Rehydrate tab_id from Camofox before creating a new tab.
    /// Mirrors `adopt_existing_tab: False` (line 576).
    pub adopt_existing_tab: bool,
    /// Docker Camofox opens page URLs from inside the container.
    /// Mirrors `rewrite_loopback_urls: False` (line 580).
    pub rewrite_loopback_urls: bool,
    /// Mirrors `loopback_host_alias: "host.docker.internal"` (line 581).
    pub loopback_host_alias: &'static str,
}

impl Default for BrowserCamofoxConfig {
    fn default() -> Self {
        Self {
            managed_persistence: false,
            user_id: "",
            session_key: "",
            adopt_existing_tab: false,
            rewrite_loopback_urls: false,
            loopback_host_alias: "host.docker.internal",
        }
    }
}

// ---------------------------------------------------------------------------
// browser.extension_control — mirrors lines 589-593
// ---------------------------------------------------------------------------

/// Authenticated browser-extension controller lane.
/// Mirrors `DEFAULT_CONFIG["browser"]["extension_control"]` (lines 589-593).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserExtensionControlConfig {
    /// Mirrors `enabled: False` (line 590).
    pub enabled: bool,
    /// Mirrors `developer_mode: False` (line 591).
    pub developer_mode: bool,
}

impl Default for BrowserExtensionControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            developer_mode: false,
        }
    }
}

// ---------------------------------------------------------------------------
// browser — mirrors lines 532-593
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CONFIG["browser"]` (lines 532-593).
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserConfig {
    /// Browser tool implementation. Mirrors `backend: ""` (line 543).
    pub backend: &'static str,
    /// Mirrors `inactivity_timeout: 120` (line 544).
    pub inactivity_timeout: u64,
    /// Timeout for browser commands in seconds (screenshot, navigate, etc.)
    /// Mirrors `command_timeout: 30` (line 545).
    pub command_timeout: u64,
    /// Auto-record browser sessions as WebM videos.
    /// Mirrors `record_sessions: False` (line 546).
    pub record_sessions: bool,
    /// Local mode: launch Chromium with a visible window.
    /// Mirrors `headed: False` (line 547).
    pub headed: bool,
    /// Allow navigating to private/internal IPs (localhost, 192.168.x.x, etc.)
    /// Mirrors `allow_private_urls: False` (line 548).
    pub allow_private_urls: bool,
    /// Browser engine for local mode. Mirrors `engine: "auto"` (line 555).
    pub engine: &'static str,
    /// When a cloud provider is set, auto-spawn local Chromium for LAN/localhost URLs.
    /// Mirrors `auto_local_for_private_urls: True` (line 556).
    pub auto_local_for_private_urls: bool,
    /// Optional persistent CDP endpoint for attaching to an existing Chromium/Chrome.
    /// Mirrors `cdp_url: ""` (line 557).
    pub cdp_url: &'static str,
    /// Legacy override: when true, browser_console(expression=...) bypasses the restrict_evaluate denylist entirely.
    /// Mirrors `allow_unsafe_evaluate: False` (line 558).
    pub allow_unsafe_evaluate: bool,
    /// Opt-in denylist blocking sensitive JS primitives in browser_console(expression=...).
    /// Mirrors `restrict_evaluate: False` (line 559).
    pub restrict_evaluate: bool,
    /// CDP supervisor — dialog + frame detection via a persistent WebSocket.
    /// Mirrors `dialog_policy: "must_respond"` (line 564).
    pub dialog_policy: &'static str,
    /// Safety auto-dismiss after N seconds under must_respond.
    /// Mirrors `dialog_timeout_s: 300` (line 565).
    pub dialog_timeout_s: u64,
    /// Mirrors `camofox` (lines 566-582).
    pub camofox: BrowserCamofoxConfig,
    /// Mirrors `extension_control` (lines 589-593).
    pub extension_control: BrowserExtensionControlConfig,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            backend: "",
            inactivity_timeout: 120,
            command_timeout: 30,
            record_sessions: false,
            headed: false,
            allow_private_urls: false,
            engine: "auto",
            auto_local_for_private_urls: true,
            cdp_url: "",
            allow_unsafe_evaluate: false,
            restrict_evaluate: false,
            dialog_policy: "must_respond",
            dialog_timeout_s: 300,
            camofox: BrowserCamofoxConfig::default(),
            extension_control: BrowserExtensionControlConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// checkpoints — mirrors lines 595-639
// ---------------------------------------------------------------------------

/// Filesystem checkpoints — automatic snapshots before destructive file ops.
/// Mirrors `DEFAULT_CONFIG["checkpoints"]` (lines 595-639).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointsConfig {
    /// Mirrors `enabled: False` (line 606).
    pub enabled: bool,
    /// Max checkpoints to keep per working directory.
    /// Mirrors `max_snapshots: 20` (line 610).
    pub max_snapshots: u32,
    /// Hard ceiling on total `~/.hermes/checkpoints/` size (MB).
    /// Mirrors `max_total_size_mb: 500` (line 615).
    pub max_total_size_mb: u32,
    /// Skip any single file larger than this when staging a checkpoint.
    /// Mirrors `max_file_size_mb: 10` (line 619).
    pub max_file_size_mb: u32,
    /// Auto-maintenance sweep.
    /// Mirrors `auto_prune: True` (line 636).
    pub auto_prune: bool,
    /// Mirrors `retention_days: 7` (line 637).
    pub retention_days: u32,
    /// Mirrors `min_interval_hours: 24` (line 638).
    pub min_interval_hours: u32,
}

impl Default for CheckpointsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_snapshots: 20,
            max_total_size_mb: 500,
            max_file_size_mb: 10,
            auto_prune: true,
            retention_days: 7,
            min_interval_hours: 24,
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level scalar thresholds — mirrors lines 641-677
// ---------------------------------------------------------------------------

/// Hard cap (chars) for a single automatic context file such as SOUL.md,
/// AGENTS.md, CLAUDE.md, .hermes.md, or .cursorrules before Hermes applies
/// head/tail truncation. `null` (the default) lets the cap scale with the
/// model's context window. Mirrors `context_file_max_chars: None` (line 647).
pub const DEFAULT_CONTEXT_FILE_MAX_CHARS: Option<u32> = None;

/// Maximum characters returned by a single read_file call.
/// Mirrors `file_read_max_chars: 100_000` (line 652).
pub const DEFAULT_FILE_READ_MAX_CHARS: u32 = 100_000;

/// Seconds to wait at agent-build time for in-flight MCP server discovery
/// to finish before the agent snapshots its tool list.
/// Mirrors `mcp_discovery_timeout: 1.5` (line 667).
pub const DEFAULT_MCP_DISCOVERY_TIMEOUT: f64 = 1.5;

/// Single-query (`hermes -q/-z "..."`) variant of mcp_discovery_timeout.
/// Mirrors `mcp_single_query_discovery_timeout: 15.0` (line 677).
pub const DEFAULT_MCP_SINGLE_QUERY_DISCOVERY_TIMEOUT: f64 = 15.0;

// ---------------------------------------------------------------------------
// mcp — mirrors lines 679-691
// ---------------------------------------------------------------------------

/// MCP runtime behavior (distinct from the per-server definitions in
/// mcp_servers: and from the auxiliary.mcp side-LLM task settings).
/// Mirrors `DEFAULT_CONFIG["mcp"]` (lines 679-691).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfig {
    /// Auto-reload MCP connections when config.yaml's mcp_servers section
    /// changes at runtime (CLI file watcher, default on).
    /// Mirrors `auto_reload_on_config_change: True` (line 690).
    pub auto_reload_on_config_change: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            auto_reload_on_config_change: true,
        }
    }
}

// ---------------------------------------------------------------------------
// tool_output — mirrors lines 693-711
// ---------------------------------------------------------------------------

/// Tool-output truncation thresholds.
/// Mirrors `DEFAULT_CONFIG["tool_output"]` (lines 693-711).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputConfig {
    /// terminal_tool output cap, in chars (default 50_000 ≈ 12-15K tokens).
    /// Mirrors `max_bytes: 50_000` (line 708).
    pub max_bytes: u32,
    /// read_file pagination cap — the maximum `limit` a single read_file call can request.
    /// Mirrors `max_lines: 2000` (line 709).
    pub max_lines: u32,
    /// per-line cap applied when read_file emits a line-numbered view.
    /// Mirrors `max_line_length: 2000` (line 710).
    pub max_line_length: u32,
}

impl Default for ToolOutputConfig {
    fn default() -> Self {
        Self {
            max_bytes: 50_000,
            max_lines: 2000,
            max_line_length: 2000,
        }
    }
}

// ---------------------------------------------------------------------------
// tool_loop_guardrails — mirrors lines 713-741
// ---------------------------------------------------------------------------

/// Mirrors `warn_after` / `hard_stop_after` thresholds.
/// Mirrors `DEFAULT_CONFIG["tool_loop_guardrails"]["warn_after"]` (lines 719-723).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLoopThresholds {
    /// Mirrors `exact_failure: 2` / `5` (lines 720/725).
    pub exact_failure: u32,
    /// Mirrors `same_tool_failure: 3` / `8` (lines 721/726).
    pub same_tool_failure: u32,
    /// Mirrors `idempotent_no_progress: 2` / `5` (lines 722/727).
    pub idempotent_no_progress: u32,
}

/// Mirrors `loop_caps: { max_web_searches: 50, max_subagents: 50 }` (lines 737-740).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLoopCaps {
    /// max web_search calls per turn (0 = unlimited). Mirrors `max_web_searches: 50` (line 738).
    pub max_web_searches: u32,
    /// max subagents spawned per turn (0 = unlimited). Mirrors `max_subagents: 50` (line 739).
    pub max_subagents: u32,
}

impl Default for ToolLoopCaps {
    fn default() -> Self {
        Self {
            max_web_searches: 50,
            max_subagents: 50,
        }
    }
}

/// Tool loop guardrails nudge models when they repeat failed or
/// non-progressing tool calls.
/// Mirrors `DEFAULT_CONFIG["tool_loop_guardrails"]` (lines 713-741).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLoopGuardrailsConfig {
    /// Soft warnings are always-on by default.
    /// Mirrors `warnings_enabled: True` (line 717).
    pub warnings_enabled: bool,
    /// Hard stops are opt-in.
    /// Mirrors `hard_stop_enabled: False` (line 718).
    pub hard_stop_enabled: bool,
    /// Mirrors `warn_after: { exact_failure: 2, same_tool_failure: 3, idempotent_no_progress: 2 }` (lines 719-723).
    pub warn_after: ToolLoopThresholds,
    /// Mirrors `hard_stop_after: { exact_failure: 5, same_tool_failure: 8, idempotent_no_progress: 5 }` (lines 724-728).
    pub hard_stop_after: ToolLoopThresholds,
    /// Per-turn runaway-loop caps.
    /// Mirrors `loop_caps` (lines 737-740).
    pub loop_caps: ToolLoopCaps,
}

impl Default for ToolLoopGuardrailsConfig {
    fn default() -> Self {
        Self {
            warnings_enabled: true,
            hard_stop_enabled: false,
            warn_after: ToolLoopThresholds {
                exact_failure: 2,
                same_tool_failure: 3,
                idempotent_no_progress: 2,
            },
            hard_stop_after: ToolLoopThresholds {
                exact_failure: 5,
                same_tool_failure: 8,
                idempotent_no_progress: 5,
            },
            loop_caps: ToolLoopCaps::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// compression — mirrors lines 743-900 (slice 1 covers through hygiene_*)
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CONFIG["compression"]` (lines 743-900 head).
/// Slice 1 covers lines 743-900; tail (idle_compact_after_seconds etc.)
/// continues in slice2. Fields past line 900 are left for slice2 but
/// defaulted here via `Option` so slice1 remains self-contained.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressionConfig {
    /// Mirrors `enabled: True` (line 744).
    pub enabled: bool,
    /// opt-in (#52995): when True, routine compression progress statuses are delivered to chat gateway.
    /// Mirrors `progress_notices: False` (line 745).
    pub progress_notices: bool,
    /// compress when context usage exceeds this ratio.
    /// Mirrors `threshold: 0.50` (line 754).
    pub threshold: f64,
    /// absolute token cap — when set, compression triggers at the lower of the ratio-based threshold and this token count.
    /// Mirrors `threshold_tokens: None` (line 759).
    pub threshold_tokens: Option<u64>,
    /// fraction of threshold to preserve as recent tail.
    /// Mirrors `target_ratio: 0.20` (line 763).
    pub target_ratio: f64,
    /// tail retention policy (#87326): "legacy" | "lean".
    /// Mirrors `tail_mode: "legacy"` (line 764).
    pub tail_mode: &'static str,
    /// minimum recent messages to keep uncompressed.
    /// Mirrors `protect_last_n: 20` (line 775).
    pub protect_last_n: u32,
    /// REAL (actionable) user messages guaranteed to survive in the uncompressed tail.
    /// Mirrors `min_tail_user_messages: 1` (line 776).
    pub min_tail_user_messages: u32,
    /// compression retry rounds before a turn gives up.
    /// Mirrors `max_attempts: 3` (line 782).
    pub max_attempts: u32,
    /// opt-in trigger (tokens) for the deterministic, no-LLM tool-result prune.
    /// Mirrors `proactive_prune_tokens: 0` (line 787).
    pub proactive_prune_tokens: u32,
    /// the prune's summarize pass only touches tool results larger than this (chars).
    /// Mirrors `proactive_prune_min_result_chars: 8000` (line 800).
    pub proactive_prune_min_result_chars: u32,
    /// a proactive prune only commits when it reclaims at least this many tokens.
    /// Mirrors `proactive_prune_min_reclaim_tokens: 4096` (line 804).
    pub proactive_prune_min_reclaim_tokens: u32,
    /// opt-in: after each completed turn, fold the oldest un-absorbed exchange into a rolling summary.
    /// Mirrors `micro_compact: False` (line 810).
    pub micro_compact: bool,
    /// cadence: run a pass every Nth completed turn.
    /// Mirrors `micro_compact_every_n_turns: 1` (line 822).
    pub micro_compact_every_n_turns: u32,
    /// once the rolling summary exceeds this many tokens, the next pass re-summarizes the summary itself.
    /// Mirrors `micro_compact_defrag_threshold_tokens: 2000` (line 829).
    pub micro_compact_defrag_threshold_tokens: u32,
    /// gateway session-hygiene force-compress threshold by message count.
    /// Mirrors `hygiene_hard_message_limit: 5000` (line 833).
    pub hygiene_hard_message_limit: u32,
    /// max seconds gateway waits for pre-agent hygiene compression.
    /// Mirrors `hygiene_timeout_seconds: 30` (line 834).
    pub hygiene_timeout_seconds: u64,
    /// absolute cap on the hygiene compression wait even while tokens are still moving.
    /// Mirrors `hygiene_total_ceiling_seconds: 600` (line 839).
    pub hygiene_total_ceiling_seconds: u64,
    /// skip repeated failed hygiene attempts for this session.
    /// Mirrors `hygiene_failure_cooldown_seconds: 300` (line 842).
    pub hygiene_failure_cooldown_seconds: u64,
    /// inactivity budget for in-agent compress_context.
    /// Mirrors `context_timeout_seconds: 120` (line 843).
    pub context_timeout_seconds: u64,
    /// absolute cap on the *pre-commit* in-agent compress_context wait.
    /// Mirrors `context_total_ceiling_seconds: 600` (line 850).
    pub context_total_ceiling_seconds: u64,
    /// non-system head messages always preserved verbatim.
    /// Mirrors `protect_first_n: 3` (line 864).
    pub protect_first_n: u32,
    /// When True, auto-compression that fails to generate a summary aborts entirely.
    /// Mirrors `abort_on_summary_failure: False` (line 870).
    pub abort_on_summary_failure: bool,
    /// Historical key name kept for compatibility. Mirrors `codex_gpt55_autoraise: True` (line 881).
    pub codex_gpt55_autoraise: bool,
    /// Display the one-time Codex gpt-5.4/5.5/5.6 autoraise banner.
    /// Mirrors `codex_gpt55_autoraise_notice: True` (line 893).
    pub codex_gpt55_autoraise_notice: bool,
    /// Codex app-server (codex CLI runtime) thread compaction mode.
    /// Mirrors `codex_app_server_auto: "native"` (line 897).
    pub codex_app_server_auto: &'static str,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            progress_notices: false,
            threshold: 0.50,
            threshold_tokens: None,
            target_ratio: 0.20,
            tail_mode: "legacy",
            protect_last_n: 20,
            min_tail_user_messages: 1,
            max_attempts: 3,
            proactive_prune_tokens: 0,
            proactive_prune_min_result_chars: 8000,
            proactive_prune_min_reclaim_tokens: 4096,
            micro_compact: false,
            micro_compact_every_n_turns: 1,
            micro_compact_defrag_threshold_tokens: 2000,
            hygiene_hard_message_limit: 5000,
            hygiene_timeout_seconds: 30,
            hygiene_total_ceiling_seconds: 600,
            hygiene_failure_cooldown_seconds: 300,
            context_timeout_seconds: 120,
            context_total_ceiling_seconds: 600,
            protect_first_n: 3,
            abort_on_summary_failure: false,
            codex_gpt55_autoraise: true,
            codex_gpt55_autoraise_notice: true,
            codex_app_server_auto: "native",
        }
    }
}

// ---------------------------------------------------------------------------
// DEFAULT_CONFIG aggregate (slice 1 view) — mirrors lines 7-900
// ---------------------------------------------------------------------------

/// Slice-1 view of `DEFAULT_CONFIG` — all keys whose defining lines fall
/// within 1-900. Keys whose definition extends past 900 (compression tail)
/// are represented with slice-1 defaults; remaining top-level keys
/// (prompt_caching, openrouter, bedrock, auxiliary, display, dashboard, …)
/// live in later slices.
#[derive(Debug, Clone, PartialEq)]
pub struct DefaultConfigSlice1 {
    pub model: &'static str,
    pub database: DatabaseConfig,
    pub runtime: RuntimeConfig,
    pub max_concurrent_sessions: Option<u32>,
    pub max_live_sessions: Option<u32>,
    pub session: SessionConfig,
    pub agent: AgentConfig,
    pub terminal: TerminalConfig,
    pub web: WebConfig,
    pub browser: BrowserConfig,
    pub checkpoints: CheckpointsConfig,
    pub context_file_max_chars: Option<u32>,
    pub file_read_max_chars: u32,
    pub mcp_discovery_timeout: f64,
    pub mcp_single_query_discovery_timeout: f64,
    pub mcp: McpConfig,
    pub tool_output: ToolOutputConfig,
    pub tool_loop_guardrails: ToolLoopGuardrailsConfig,
    pub compression: CompressionConfig,
}

impl Default for DefaultConfigSlice1 {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL,
            database: DatabaseConfig::default(),
            runtime: RuntimeConfig::default(),
            max_concurrent_sessions: DEFAULT_MAX_CONCURRENT_SESSIONS,
            max_live_sessions: DEFAULT_MAX_LIVE_SESSIONS,
            session: SessionConfig::default(),
            agent: AgentConfig::default(),
            terminal: TerminalConfig::default(),
            web: WebConfig::default(),
            browser: BrowserConfig::default(),
            checkpoints: CheckpointsConfig::default(),
            context_file_max_chars: DEFAULT_CONTEXT_FILE_MAX_CHARS,
            file_read_max_chars: DEFAULT_FILE_READ_MAX_CHARS,
            mcp_discovery_timeout: DEFAULT_MCP_DISCOVERY_TIMEOUT,
            mcp_single_query_discovery_timeout: DEFAULT_MCP_SINGLE_QUERY_DISCOVERY_TIMEOUT,
            mcp: McpConfig::default(),
            tool_output: ToolOutputConfig::default(),
            tool_loop_guardrails: ToolLoopGuardrailsConfig::default(),
            compression: CompressionConfig::default(),
        }
    }
}

/// Mirrors `DEFAULT_CONFIG` slice 1 — convenience constructor.
/// Pure-data, no I/O, no env reads.
pub fn default_config_slice1() -> DefaultConfigSlice1 {
    DefaultConfigSlice1::default()
}

/// Mirrors Python's empty `providers: {}` / `fallback_providers: []` /
/// `credential_pool_strategies: {}` sentinels for slice-1.
/// Rust callers use typed maps; these helpers expose the empty defaults
/// explicitly for 1:1 traceability (lines 9-11).
pub fn default_providers() -> HashMap<String, String> {
    HashMap::new()
}
pub fn default_fallback_providers() -> Vec<String> {
    Vec::new()
}
pub fn default_credential_pool_strategies() -> HashMap<String, String> {
    HashMap::new()
}
pub fn default_disabled_toolsets() -> Vec<String> {
    Vec::new()
}
pub fn default_reasoning_overrides() -> HashMap<String, String> {
    HashMap::new()
}
pub fn default_provider_tier() -> HashMap<String, String> {
    HashMap::new()
}

// ---------------------------------------------------------------------------
// OPTIONAL_ENV_VARS — not in slice 1 (defined at line 3763)
// ---------------------------------------------------------------------------
// Slice 1 covers only DEFAULT_CONFIG head (lines 1-900). OPTIONAL_ENV_VARS
// (line 3763+) lives in a later slice (slice5/6). Stub for traceability:

/// Slice 1 does not yet define `OPTIONAL_ENV_VARS` (begins at line 3763).
/// This stub preserves the symbol for cross-slice imports until that slice lands.
pub fn optional_env_vars_stub() -> HashMap<String, String> {
    HashMap::new()
}
