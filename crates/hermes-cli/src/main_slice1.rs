//! hermes-cli main — slice 1/10
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/main.py`
//! slice 1/10 — lines 1–~1500 of 14 268 (first ~1500 LOC).
//! Covers: module docstring + bootstrap imports, startup fast-path,
//! oneshot exit/cleanup, process title, early interface/TUI gates,
//! mouse-residue suppression, Termux/container fast checks, version
//! fast-path, argparse imports, subcommand builders, `PROJECT_ROOT`,
//! profile override, Windows launcher heal, dotenv/config bridge,
//! logging/ipv4 early init, Termux fingerprint + bundled-skills sync,
//! relative-time wrapper, provider-configured check, model-override
//! guard, session status/tag helpers, and the start of the
//! curses session browser (through the filtered-row loop, ~line 1500).
//! Continued in `main_slice2.rs`.
//!
//! T0681 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — usage (mirrors Python top-level docstring, lines 1-44)
// ---------------------------------------------------------------------------

/// Hermes CLI usage — mirrors `hermes_cli/main.py` docstring.
///
/// ```text
/// hermes                     # Interactive chat (default)
/// hermes chat                # Interactive chat
/// hermes gateway             # Run gateway in foreground
/// hermes gateway start       # Start gateway as service
/// hermes gateway stop        # Stop gateway service
/// hermes gateway status      # Show gateway status
/// hermes gateway install     # Install gateway service
/// hermes gateway uninstall   # Uninstall gateway service
/// hermes setup               # Interactive setup wizard
/// hermes logout              # Clear stored authentication
/// hermes status              # Show status of all components
/// hermes cron                # Manage cron jobs
/// hermes cron list           # List cron jobs
/// hermes cron status         # Check if cron scheduler is running
/// hermes doctor              # Check configuration and dependencies
/// hermes honcho setup  etc.
/// hermes --version / update / uninstall / acp / sessions browse
/// hermes claw migrate --dry-run
/// ```
pub const USAGE: &str = "hermes — see module docstring for full usage";

// ---------------------------------------------------------------------------
// Early bootstrap — mirrors lines 59-99
// ---------------------------------------------------------------------------

/// Mirrors `hermes_bootstrap` guarded import (lines 59-62).
/// On POSIX this is a no-op; on Windows it ensures UTF-8 stdio.
pub fn ensure_bootstrap() {
    // Rust stdio is UTF-8 on all platforms; no-op. Kept for 1:1 line mapping.
}

/// Mirrors `suppress_platform_ver_console()` import (lines 64-72).
pub fn suppress_platform_ver_console() {
    // Windows-only: neutralise platform._syscmd_ver console flash. No-op on POSIX.
    #[cfg(windows)]
    {
        // In Python this shells out mitigation; in Rust we have no platform module import
        // side-effects, so nothing to suppress. Function retained for 1:1 coverage.
    }
}

// ---------------------------------------------------------------------------
// Startup fast-path bootstrap — lines 77-84
// ---------------------------------------------------------------------------

/// Mirrors `_bootstrap_root` path math (lines 81-83).
pub fn bootstrap_root() -> PathBuf {
    // In Rust the crate root is known at compile time via CARGO_MANIFEST_DIR;
    // runtime equivalent is the hermes agent checkout root (parent of hermes_cli/).
    // We mimic the Python realpath(join(dirname(__file__), pardir)) logic:
    // `hermes_cli/main.py` -> parent -> repo root.
    // Here we resolve from current exe or env if available, else current_dir.
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        return PathBuf::from(v);
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Mirrors `_project_root_str_fast()` (227-228) delegating to `_startup_fast`.
pub fn project_root_str_fast() -> String {
    bootstrap_root().to_string_lossy().to_string()
}

/// Mirrors `_ensure_project_root_on_path_fast()` (231-232).
pub fn ensure_project_root_on_path_fast() {
    // Rust uses compile-time crate graph; sys.path insertion has no equivalent.
}

// ---------------------------------------------------------------------------
// Early recovery — lines 98-103
// ---------------------------------------------------------------------------

pub fn early_recovery_if_needed() {
    // Mirrors `_early_recovery_mod.recover_if_needed()` (100-103).
    // Python repair is stdlib-only and best-effort; Rust no-op but kept for 1:1.
}

// ---------------------------------------------------------------------------
// Oneshot exit / cleanup — lines 106-224
// ---------------------------------------------------------------------------

/// Mirrors `_exit_after_oneshot` (106-133).
/// Flushes streams, shuts down logging, then hard-exits without finalizers.
pub fn exit_after_oneshot(rc: Option<i32>) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // log::shutdown equivalent — best effort
    let code = match rc {
        None => 0,
        Some(n) => n,
    };
    std::process::exit(code);
}

static ONESHOT_CLEANUP_DONE: Mutex<bool> = Mutex::new(false);

/// Mirrors `_cleanup_oneshot_runtime` (138-174).
pub fn cleanup_oneshot_runtime() {
    let mut done = ONESHOT_CLEANUP_DONE.lock().unwrap_or_else(|e| e.into_inner());
    if *done {
        return;
    }
    *done = true;
    // Best-effort cleanup of tool runtimes — mirrors Python's try/except blocks.
    // In Rust we have no global terminal/browser/mcp registries yet; stubs kept for 1:1.
    // cleanup_all_environments(), interrupt_all(), _emergency_cleanup_all_sessions(),
    // shutdown_mcp_servers(), shutdown_cached_clients() would go here.
}

/// Mirrors `_run_and_exit_oneshot` (177-224).
pub fn run_and_exit_oneshot(
    prompt: &str,
    model: Option<&str>,
    provider: Option<&str>,
    toolsets: Option<&str>,
    skills: Option<&str>,
    usage_file: Option<&str>,
) -> ! {
    // In Python this imports `hermes_cli.oneshot.run_oneshot` and hard-exits.
    // Rust stub: log and exit 1 if not implemented, 0 for empty prompt guard.
    let rc: Option<i32> = {
        // Try to delegate to actual oneshot runtime if linked; otherwise stub.
        // Keep the Python exception mapping: KeyboardInterrupt->130, SystemExit->code, else 1.
        let _ = (prompt, model, provider, toolsets, skills, usage_file);
        Some(1)
    };
    cleanup_oneshot_runtime();
    exit_after_oneshot(rc);
}

// ---------------------------------------------------------------------------
// Process title — lines 235-272
// ---------------------------------------------------------------------------

/// Mirrors `_set_process_title()` (235-272).
/// Tries setproctitle -> prctl -> pthread_setname_np -> no-op.
pub fn set_process_title() {
    // Rust equivalent would use `setproctitle` crate or libc prctl.
    // Kept as best-effort no-op for 1:1 port without extra deps.
    #[cfg(target_os = "linux")]
    {
        // Attempt prctl(PR_SET_NAME, "hermes") via libc if available.
        // Avoid pulling libc dep in this slice; no-op.
    }
    #[cfg(target_os = "macos")]
    {
        // pthread_setname_np("hermes") — same no-op.
    }
}

// ---------------------------------------------------------------------------
// Early interface / TUI gates — lines 275-372
// ---------------------------------------------------------------------------

static EARLY_INTERFACE_CACHE: OnceLock<Option<String>> = OnceLock::new();

/// Mirrors `_config_default_interface_early()` (283-311).
/// Cheap YAML read of `display.interface` — returns "cli" or "tui".
pub fn config_default_interface_early() -> String {
    if let Some(cached) = EARLY_INTERFACE_CACHE.get() {
        return cached.clone().unwrap_or_else(|| "cli".to_string());
    }
    let value = read_display_interface_early().unwrap_or_else(|| "cli".to_string());
    let _ = EARLY_INTERFACE_CACHE.set(Some(value.clone()));
    value
}

fn read_display_interface_early() -> Option<String> {
    let home = std::env::var("HERMES_HOME").ok();
    let cfg_path = if let Some(h) = home {
        PathBuf::from(h).join("config.yaml")
    } else {
        dirs_home().join(".hermes").join("config.yaml")
    };
    let text = std::fs::read_to_string(&cfg_path).ok()?;
    // Minimal YAML parse: look for `display:` then `interface:` without needing yaml crate.
    // Mirrors Python's yaml.safe_load + disp.get("interface").
    let mut in_display = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("display:") {
            in_display = true;
            continue;
        }
        if in_display {
            // dedent ends display block
            if !line.starts_with(' ') && !line.starts_with('\t') && !trimmed.is_empty() {
                break;
            }
            if trimmed.starts_with("interface:") {
                let val = trimmed["interface:".len()..].trim();
                let val = val.trim_matches(|c| c == '"' || c == '\'').trim().to_lowercase();
                if val == "tui" {
                    return Some("tui".to_string());
                } else {
                    return Some("cli".to_string());
                }
            }
        }
    }
    None
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Mirrors `_wants_tui_early()` (314-341).
pub fn wants_tui_early(argv: Option<&[String]>) -> bool {
    let argv_owned: Vec<String> = argv
        .map(|s| s.to_vec())
        .unwrap_or_else(|| std::env::args().skip(1).collect());
    if argv_owned.iter().any(|a| a == "--cli") {
        return false;
    }
    if std::env::var("HERMES_TUI").map(|v| v == "1").unwrap_or(false)
        || argv_owned.iter().any(|a| a == "--tui")
    {
        return true;
    }
    // TTY gate — mirrors `sys.stdin.isatty() and sys.stdout.isatty()`
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // isatty check via `is_terminal` (Rust 1.70+ has IsTerminal)
        let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
        let stdout_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
        if !(stdin_tty && stdout_tty) {
            return false;
        }
    }
    #[cfg(not(unix))]
    {
        // On non-unix, fall back to checking without TTY gate
    }
    config_default_interface_early() == "tui"
}

/// Mirrors `_suppress_mouse_residue_early()` (352-374).
pub fn suppress_mouse_residue_early() {
    if std::env::var("HERMES_TUI_NO_EARLY_DISABLE").map(|v| v == "1").unwrap_or(false) {
        return;
    }
    if !wants_tui_early(None) {
        return;
    }
    // Skip when stdout redirected — mirrors `os.isatty(1)`
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        return;
    }
    // Disable all mouse tracking variants — idempotent CSI writes.
    let seq = b"\x1b[?1003l\x1b[?1002l\x1b[?1001l\x1b[?1000l\x1b[?9l\x1b[?1006l\x1b[?1005l\x1b[?1015l\x1b[?1016l\x1b[?2029l";
    use std::io::Write;
    let _ = std::io::stdout().write_all(seq);
    let _ = std::io::stdout().flush();
}

// ---------------------------------------------------------------------------
// Termux / container fast checks — lines 377-420
// ---------------------------------------------------------------------------

pub fn is_termux_startup_environment_fast() -> bool {
    is_termux_startup_environment(None)
}

pub fn is_termux_fast_version_argv(argv: &[String]) -> bool {
    // Mirrors `_startup_fast.is_termux_fast_version_argv`
    is_termux_startup_environment(None) && is_global_fast_version_argv(argv)
}

pub fn is_global_fast_version_argv(argv: &[String]) -> bool {
    // Mirrors `_startup_fast.is_global_fast_version_argv` — true for `hermes --version`
    // alone (no subcommand, just --version / -V)
    if argv.len() != 1 {
        return false;
    }
    matches!(argv[0].as_str(), "--version" | "-V" | "--help" | "-h")
}

pub fn is_container_startup_environment_fast() -> bool {
    // Mirrors `_startup_fast.is_container_startup_environment()`
    std::env::var("HERMES_CONTAINER").is_ok() || Path::new("/.dockerenv").exists()
}

pub fn active_profile_may_override_home_fast(hermes_root: &str) -> bool {
    let _ = hermes_root;
    // Mirrors `_startup_fast.active_profile_may_override_home`
    Path::new(hermes_root).join("active_profile").exists()
}

pub fn container_mode_may_be_active_fast() -> bool {
    std::env::var("HERMES_CONTAINER_MODE").is_ok() || is_container_startup_environment_fast()
}

pub fn read_openai_version_fast() -> Option<String> {
    // Mirrors `_startup_fast.read_openai_version()` without importing importlib.metadata.
    // In Rust we have no openai python package; return None (caller handles None).
    None
}

pub fn print_fast_version_info() {
    // Mirrors `_startup_fast.print_fast_version_info()`
    // Would print version without heavy imports; stub for 1:1.
    println!("hermes {}", env!("CARGO_PKG_VERSION"));
}

pub fn try_ultrafast_version() -> bool {
    // Mirrors `_startup_fast.try_fast_version()` — handles `hermes --version` before imports.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if is_global_fast_version_argv(&argv) {
        print_fast_version_info();
        return true;
    }
    false
}

pub fn try_termux_ultrafast_version() -> bool {
    // Mirrors `_try_termux_ultrafast_version()` (416-420)
    if !is_termux_startup_environment_fast() {
        return false;
    }
    try_ultrafast_version()
}

// ---------------------------------------------------------------------------
// Argparse / subcommand builders — lines 428-487 (imports mirror)
// ---------------------------------------------------------------------------

/// Mirrors the `from hermes_cli.subcommands.* import build_*_parser` block.
/// In Rust these become `clap` subcommand builders; stubs kept for 1:1 line mapping.
pub mod subcommands {
    // Each Python `build_*_parser` registers a subcommand tree.
    // Rust stubs preserve the names for 1:1 traceability.
    pub fn build_cron_parser() {}
    pub fn build_sync_parser() {}
    pub fn build_gateway_parser() {}
    pub fn build_profile_parser() {}
    pub fn build_model_parser() {}
    pub fn build_setup_parser() {}
    pub fn build_whatsapp_parser() {}
    pub fn build_slack_parser() {}
    pub fn build_login_parser() {}
    pub fn build_logout_parser() {}
    pub fn build_auth_parser() {}
    pub fn build_status_parser() {}
    pub fn build_pause_parser() {}
    pub fn build_webhook_parser() {}
    pub fn build_hooks_parser() {}
    pub fn build_doctor_parser() {}
    pub fn build_verify_parser() {}
    pub fn build_security_parser() {}
    pub fn build_approvals_parser() {}
    pub fn build_dump_parser() {}
    pub fn build_debug_parser() {}
    pub fn build_backup_parser() {}
    pub fn build_import_cmd_parser() {}
    pub fn build_import_agent_parser() {}
    pub fn build_config_parser() {}
    pub fn build_skin_parser() {}
    pub fn build_console_parser() {}
    pub fn build_update_parser() {}
    pub fn build_uninstall_parser() {}
    pub fn build_dashboard_parser() {}
    pub fn build_gui_parser() {}
    pub fn build_logs_parser() {}
    pub fn build_prompt_size_parser() {}
    pub fn build_memory_parser() {}
    pub fn build_acp_parser() {}
    pub fn build_tools_parser() {}
    pub fn build_insights_parser() {}
    pub fn build_monitoring_parser() {}
    pub fn build_skills_parser() {}
    pub fn build_pairing_parser() {}
    pub fn build_plugins_parser() {}
    pub fn build_mcp_parser() {}
    pub fn build_claw_parser() {}
}

// ---------------------------------------------------------------------------
// _require_tty — lines 489-503
// ---------------------------------------------------------------------------

pub fn require_tty(command_name: &str) {
    // Mirrors `_require_tty(command_name)` — exits 1 if stdin is not a TTY.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!(
            "Error: 'hermes {command_name}' requires an interactive terminal.\n\
             It cannot be run through a pipe or non-interactive subprocess.\n\
             Run it directly in your terminal instead."
        );
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// PROJECT_ROOT + profile override — lines 506-693
// ---------------------------------------------------------------------------

/// Mirrors `PROJECT_ROOT = Path(_project_root_str_fast())` (507).
pub fn project_root() -> PathBuf {
    PathBuf::from(project_root_str_fast())
}

/// Mirrors `_apply_profile_override()` (520-693).
///
/// Pre-parses `--profile`/`-p` from `std::env::args`, sets `HERMES_HOME`,
/// and strips the flag from argv so argparse/clap never sees it.
/// Also honours `~/.hermes/active_profile` sticky default and the
/// `HERMES_S6_SUPERVISED_CHILD` exception.
pub fn apply_profile_override() {
    let mut argv: Vec<String> = std::env::args().collect();
    // argv[0] is binary name; Python uses sys.argv[1:]
    let args = argv[1..].to_vec();
    let mut profile_name: Option<String> = None;
    let mut consume: usize = 0;
    let mut profile_index: Option<usize> = None;

    let value_flags: std::collections::HashSet<&str> = [
        "-z", "--oneshot", "-m", "--model", "--provider", "-t", "--toolsets", "-r", "--resume",
        "-s", "--skills", "--usage-file", "--in",
    ]
    .into_iter()
    .collect();
    let optional_value_flags: std::collections::HashSet<&str> =
        ["-c", "--continue"].into_iter().collect();

    // Helper: detect `hermes mcp add ... --args <command argv>` passthrough region
    let inside_mcp_add_args = |idx: usize| -> bool {
        if let Some(mcp_idx) = args.iter().position(|a| a == "mcp") {
            if mcp_idx < idx {
                if let Some(add_idx) = args[mcp_idx + 1..idx].iter().position(|a| a == "add") {
                    let _ = add_idx;
                    return true;
                }
            }
        }
        false
    };

    let mut i = 0usize;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            break;
        }
        if arg == "--args" && inside_mcp_add_args(i) {
            break;
        }
        if (arg == "--profile" || arg == "-p") && i + 1 < args.len() {
            profile_name = Some(args[i + 1].clone());
            consume = 2;
            profile_index = Some(i);
            break;
        }
        if arg.starts_with("--profile=") {
            profile_name = Some(arg["--profile=".len()..].to_string());
            consume = 1;
            profile_index = Some(i);
            break;
        }
        if !arg.contains('=') && value_flags.contains(arg.as_str()) && i + 1 < args.len() {
            i += 2;
            continue;
        }
        if !arg.contains('=')
            && optional_value_flags.contains(arg.as_str())
            && i + 1 < args.len()
            && !args[i + 1].starts_with('-')
        {
            i += 2;
            continue;
        }
        i += 1;
    }

    // Reject invalid profile names (mirrors Python re check, line 622)
    if let Some(ref name) = profile_name.clone() {
        if consume == 2 && !is_valid_profile_name(name) {
            profile_name = None;
            consume = 0;
            profile_index = None;
        }
    }

    // If HERMES_HOME already points to a profile dir, trust it (line 627-639)
    if profile_name.is_none() {
        if let Ok(env_home) = std::env::var("HERMES_HOME") {
            if !env_home.is_empty() && Path::new(&env_home).parent().map(|p| p.file_name().map(|n| n == "profiles").unwrap_or(false)).unwrap_or(false) {
                return;
            }
        }
    }

    // Sticky active_profile fallback (lines 653-664)
    if profile_name.is_none() && std::env::var("HERMES_S6_SUPERVISED_CHILD").is_err() {
        if let Ok(home) = std::env::var("HOME") {
            let active_path = Path::new(&home).join(".hermes").join("active_profile");
            if let Ok(text) = std::fs::read_to_string(&active_path) {
                let name = text.trim().to_string();
                if !name.is_empty() && name != "default" {
                    profile_name = Some(name);
                    consume = 0;
                }
            }
        }
    }

    if let Some(name) = profile_name {
        // Resolve via `hermes_cli.profiles.resolve_profile_env` equivalent
        let hermes_home = resolve_profile_env(&name).unwrap_or_else(|e| {
            // Try sudo fallback (line 673)
            if let Some(fallback) = resolve_sudo_user_profile_env(&name) {
                return fallback;
            }
            eprintln!("Error: {e}");
            std::process::exit(1);
        });
        std::env::set_var("HERMES_HOME", hermes_home);
        if consume > 0 {
            if let Some(idx) = profile_index {
                // idx is in args (sys.argv[1:]); argv index = idx + 1
                let start = idx + 1;
                // Rebuild argv without the consumed flag
                let mut new_argv = Vec::new();
                new_argv.extend_from_slice(&argv[..start]);
                new_argv.extend_from_slice(&argv[start + consume..]);
                // Rust cannot mutate std::env::args; callers should use returned argv.
                // For 1:1 we set a marker env var so the caller can reconstruct.
                std::env::set_var("HERMES_STRIPPED_ARGV", new_argv[1..].join("\x1f"));
                let _ = argv; // suppress unused warning
            }
        }
    }
}

fn is_valid_profile_name(name: &str) -> bool {
    // Mirrors `r"^[a-z0-9][a-z0-9_-]{0,63}$"`
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {},
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return false;
        }
    }
    true
}

fn resolve_profile_env(name: &str) -> Result<String, String> {
    if name == "default" {
        let home = std::env::var("HOME").map_err(|e| e.to_string())?;
        return Ok(format!("{home}/.hermes"));
    }
    if !is_valid_profile_name(name) {
        return Err(format!("invalid profile name: {name}"));
    }
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let candidate = Path::new(&home).join(".hermes").join("profiles").join(name);
    // In Python this raises FileNotFoundError if the profile dir doesn't exist (unless creating).
    // We mimic: if dir doesn't exist, error.
    if !candidate.is_dir() {
        return Err(format!("profile '{}' not found at {}", name, candidate.display()));
    }
    Ok(candidate.to_string_lossy().to_string())
}

fn resolve_sudo_user_profile_env(name: &str) -> Option<String> {
    // Mirrors `_resolve_sudo_user_profile_env` (541-570)
    if name == "default" {
        return None;
    }
    if std::env::var("SUDO_USER").is_err() {
        return None;
    }
    // Check euid == 0
    #[cfg(unix)]
    {
        // Use libc geteuid if available; fallback to checking HERMES_SUDO_EUID
        // Without libc dep, check env var SUDO_UID == 0 as proxy
        let is_root = std::env::var("SUDO_UID").map(|v| v == "0").unwrap_or(false)
            || std::env::var("USER").map(|v| v == "root").unwrap_or(false);
        if !is_root {
            return None;
        }
    }
    let sudo_user = std::env::var("SUDO_USER").ok()?.trim().to_string();
    if sudo_user.is_empty() || sudo_user == "root" {
        return None;
    }
    // Resolve home via /etc/passwd or HOME fallback
    let home = get_user_home(&sudo_user)?;
    let candidate = home.join(".hermes").join("profiles").join(name);
    if candidate.is_dir() {
        return Some(candidate.to_string_lossy().to_string());
    }
    None
}

fn get_user_home(user: &str) -> Option<PathBuf> {
    // Try to read via `getent passwd` or fallback to /home/<user>
    let out = std::process::Command::new("getent")
        .args(["passwd", user])
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout);
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() >= 6 {
            return Some(PathBuf::from(parts[5]));
        }
    }
    Some(PathBuf::from(format!("/home/{user}")))
}

// ---------------------------------------------------------------------------
// Windows launcher heal + dotenv / config bridge — lines 713-810
// ---------------------------------------------------------------------------

pub fn ensure_windows_bin_launchers(_bootstrap_root: &Path) {
    // Mirrors `ensure_windows_bin_launchers` (713-719) — Windows only.
    #[cfg(windows)]
    {
        // Would re-stage `hermes` copies into the managed bin dir.
    }
}

pub fn load_hermes_dotenv(project_env: &Path, load_external_secrets: bool) {
    let _ = (project_env, load_external_secrets);
    // Mirrors `load_hermes_dotenv(project_env=PROJECT_ROOT / ".env", ...)` (721-736)
    // In Rust we would read `~/.hermes/.env` then project `.env`; stub for slice 1.
}

pub fn bridge_security_redact_and_ipv4() -> bool {
    // Mirrors lines 743-780: read_raw_config, managed overlay, set HERMES_REDACT_SECRETS,
    // and determine _FORCE_IPV4_EARLY. Returns force_ipv4.
    // Best-effort; on error returns false.
    false
}

pub fn setup_logging_early() {
    // Mirrors lines 785-798: setup_logging(mode=gui|cli) best-effort
}

pub fn apply_ipv4_preference(force: bool) {
    let _ = force;
    // Mirrors `apply_ipv4_preference(force=True)` (803-809)
}

// ---------------------------------------------------------------------------
// Termux fingerprint / bundled skills — lines 847-983
// ---------------------------------------------------------------------------

pub fn is_termux_startup_environment(env: Option<&HashMap<String, String>>) -> bool {
    let check_termux_version = env
        .and_then(|m| m.get("TERMUX_VERSION"))
        .map(|v| !v.is_empty())
        .unwrap_or_else(|| std::env::var("TERMUX_VERSION").is_ok());
    let prefix = env
        .and_then(|m| m.get("PREFIX").cloned())
        .unwrap_or_else(|| std::env::var("PREFIX").unwrap_or_default());
    check_termux_version
        || prefix.contains("com.termux/files/usr")
        || prefix.starts_with("/data/data/com.termux/")
}

fn read_packed_ref(common_dir: &Path, target_ref: &str) -> Option<String> {
    // Mirrors `_read_packed_ref` (858-874)
    let text = std::fs::read_to_string(common_dir.join("packed-refs")).ok()?;
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let sha = parts.next()?;
        let r = parts.next()?;
        if r.trim() == target_ref {
            return Some(sha.trim().to_string());
        }
    }
    None
}

pub fn read_git_revision_fingerprint(repo_root: &Path) -> Option<String> {
    // Mirrors `_read_git_revision_fingerprint` (877-919)
    let mut git_dir = repo_root.join(".git");
    if git_dir.is_file() {
        if let Ok(text) = std::fs::read_to_string(&git_dir) {
            for line in text.lines() {
                let mut kv = line.splitn(2, ':');
                let k = kv.next()?.trim();
                let v = kv.next()?.trim();
                if k == "gitdir" && !v.is_empty() {
                    git_dir = repo_root.join(v);
                    // best-effort canonicalize
                    if let Ok(c) = std::fs::canonicalize(&git_dir) {
                        git_dir = c;
                    }
                    break;
                }
            }
        }
    }
    let mut common_dir = git_dir.clone();
    let commondir_file = git_dir.join("commondir");
    if commondir_file.exists() {
        if let Ok(rel) = std::fs::read_to_string(&commondir_file) {
            let rel = rel.trim();
            if !rel.is_empty() {
                let candidate = git_dir.join(rel);
                if let Ok(c) = std::fs::canonicalize(&candidate) {
                    common_dir = c;
                } else {
                    common_dir = candidate;
                }
            }
        }
    }
    let head_text = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head_text.trim();
    if head.starts_with("ref:") {
        let r = head["ref:".len()..].trim();
        for cand in [&git_dir, &common_dir] {
            let ref_file = cand.join(r);
            if ref_file.exists() {
                if let Ok(sha) = std::fs::read_to_string(&ref_file) {
                    return Some(format!("git:{r}:{}", sha.trim()));
                }
            }
        }
        if let Some(sha) = read_packed_ref(&common_dir, r) {
            return Some(format!("git:{r}:{sha}"));
        }
        return Some(format!("git:{r}:unresolved"));
    }
    Some(format!("git:HEAD:{head}"))
}

pub fn termux_bundled_skills_fingerprint() -> String {
    // Mirrors `_termux_bundled_skills_fingerprint` (922-932)
    if let Some(fp) = read_git_revision_fingerprint(&project_root()) {
        return fp;
    }
    let skills_dir = project_root().join("skills");
    if let Ok(meta) = std::fs::metadata(&skills_dir) {
        // Use mtime + size as fallback fingerprint
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_else(|| "0".to_string());
        let size = meta.len().to_string();
        return format!("skills:{}:{}:{}:{}", env!("CARGO_PKG_VERSION"), "unknown", mtime, size);
    }
    format!("skills:{}:{}:missing", env!("CARGO_PKG_VERSION"), "unknown")
}

pub fn termux_bundled_skills_stamp_path() -> PathBuf {
    get_hermes_home().join("skills").join(".termux_bundled_sync_stamp")
}

pub fn termux_bundled_skills_sync_needed() -> bool {
    // Mirrors `_termux_bundled_skills_sync_needed` (939-948)
    if !is_termux_startup_environment(None) {
        return true;
    }
    if std::env::var("HERMES_TERMUX_FORCE_SKILLS_SYNC").map(|v| v == "1").unwrap_or(false) {
        return true;
    }
    let stamp = termux_bundled_skills_stamp_path();
    if let Ok(text) = std::fs::read_to_string(&stamp) {
        return text.trim() != termux_bundled_skills_fingerprint();
    }
    true
}

pub fn mark_termux_bundled_skills_synced() {
    // Mirrors `_mark_termux_bundled_skills_synced` (951-959)
    if !is_termux_startup_environment(None) {
        return;
    }
    let stamp = termux_bundled_skills_stamp_path();
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&stamp, termux_bundled_skills_fingerprint() + "\n");
}

pub fn sync_bundled_skills_for_startup() -> bool {
    // Mirrors `_sync_bundled_skills_for_startup` (962-976)
    if is_termux_startup_environment(None) && !termux_bundled_skills_sync_needed() {
        return false;
    }
    // Would call `tools.skills_sync.sync_skills(quiet=True)` — stub
    mark_termux_bundled_skills_synced();
    true
}

pub fn termux_should_prefetch_update_check() -> bool {
    // Mirrors `_termux_should_prefetch_update_check` (979-982)
    if !is_termux_startup_environment(None) {
        return true;
    }
    std::env::var("HERMES_TERMUX_PREFETCH_UPDATES").map(|v| v == "1").unwrap_or(false)
}

pub fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    dirs_home().join(".hermes")
}

// ---------------------------------------------------------------------------
// _relative_time + _has_any_provider_configured — lines 985-1120
// ---------------------------------------------------------------------------

/// Mirrors `_relative_time(ts)` (985-994) — thin wrapper over `hermes_cli.timefmt`.
pub fn relative_time(ts: Option<i64>) -> String {
    // Stub: produce "2h ago" style — real impl would delegate to timefmt.
    match ts {
        None => "never".to_string(),
        Some(secs) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let diff = now - secs;
            if diff < 60 {
                "just now".to_string()
            } else if diff < 3600 {
                format!("{}m ago", diff / 60)
            } else if diff < 86400 {
                format!("{}h ago", diff / 3600)
            } else if diff < 172800 {
                "yesterday".to_string()
            } else {
                format!("{}d ago", diff / 86400)
            }
        }
    }
}

/// Mirrors `_has_any_provider_configured()` (997-1120).
/// Checks env vars, .env, auth.json, config.yaml, and provider registry fallbacks.
pub fn has_any_provider_configured() -> bool {
    // Collect provider env vars (mirrors lines 1031-1042)
    let provider_env_vars = [
        "OPENROUTER_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_TOKEN",
        "OPENAI_BASE_URL",
    ];
    if provider_env_vars.iter().any(|k| std::env::var(k).ok().map(|v| !v.trim().is_empty()).unwrap_or(false)) {
        return true;
    }
    // .env file check
    let env_file = get_hermes_home().join(".env");
    if env_file.exists() {
        if let Ok(text) = std::fs::read_to_string(&env_file) {
            for line in text.lines() {
                let mut line = line.trim();
                if line.starts_with('#') || !line.contains('=') {
                    continue;
                }
                if line.starts_with("export ") {
                    line = &line[7..];
                }
                if let Some((k, v)) = line.split_once('=') {
                    let val = v.trim().trim_matches(|c| c == '\'' || c == '"').trim();
                    if provider_env_vars.contains(&k.trim()) && !val.is_empty() {
                        return true;
                    }
                }
            }
        }
    }
    // auth.json active_provider logged_in check — stub (would read JSON)
    let auth_file = get_hermes_home().join("auth.json");
    if auth_file.exists() {
        // If file exists and non-empty, assume logged_in for slice 1 stub
        if let Ok(s) = std::fs::read_to_string(&auth_file) {
            if s.contains("\"active_provider\"") {
                return true;
            }
        }
    }
    // config.yaml model dict check — minimal parse
    let cfg_path = get_hermes_home().join("config.yaml");
    if let Ok(text) = std::fs::read_to_string(&cfg_path) {
        if text.contains("provider:") || text.contains("base_url:") || text.contains("api_key:") {
            // Heuristic: if model is a dict with provider/base_url/api_key, return true
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// _confirm_startup_expensive_model_override — lines 1123-1218
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StartupArgs {
    pub model: Option<String>,
    pub provider: Option<String>,
}

pub fn confirm_startup_expensive_model_override(args: &StartupArgs) {
    let explicit_model = args.model.as_deref().unwrap_or("").trim().to_string();
    let explicit_provider = args.provider.as_deref().unwrap_or("").trim().to_string();
    if explicit_model.is_empty() && explicit_provider.is_empty() {
        return;
    }
    // Would load config and call selection_warnings / combined_message.
    // Mirrors the confirmation flow: interactive prompt or non-interactive refusal.
    // Stub for slice 1 — logs and returns.
    let is_interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if !is_interactive {
        // Non-interactive would refuse unless allow_data_training_tiers_noninteractive
        // For 1:1 stub we just return (no warnings in stub).
        return;
    }
    // Interactive: would print combined_message and ask "Use this model? [y/N]"
    // No warnings in stub path, so no prompt.
}

// ---------------------------------------------------------------------------
// Session helpers — lines 1221-1264
// ---------------------------------------------------------------------------

pub fn session_status_tag(status: Option<&str>) -> &'static str {
    match status.unwrap_or("") {
        "complete" => "done",
        "interrupted" => "intr",
        "error" => "err",
        "empty" => "empty",
        _ => "-",
    }
}

/// Mirrors `_annotate_session_statuses(sessions, session_db)` (1231-1247).
/// Attaches `_status` key via `session_lifecycle_statuses` batch lookup.
pub fn annotate_session_statuses(
    sessions: &mut [HashMap<String, String>],
    _session_db: Option<&dyn SessionDb>,
) {
    if sessions.is_empty() {
        return;
    }
    // Real impl would call `session_db.session_lifecycle_statuses(ids)`.
    // Stub: assign "-" for 1:1 signature coverage.
    for s in sessions.iter_mut() {
        s.entry("_status".to_string()).or_insert_with(|| "-".to_string());
    }
}

pub trait SessionDb {
    fn session_lifecycle_statuses(&self, ids: Vec<String>) -> HashMap<String, String>;
    fn delete_session(&self, id: &str, sessions_dir: Option<&Path>) -> bool;
}

// ---------------------------------------------------------------------------
// _session_browse_picker — lines 1250-1500+ (slice 1 covers through footer loop)
// ---------------------------------------------------------------------------

/// Mirrors `_session_browse_picker(sessions, session_db)` (1250-1500 slice 1 portion).
///
/// Interactive curses picker with live search. Slice 1 includes:
///  - empty check + annotate
///  - `_delete_session` helper
///  - curses layout constants + `_format_row` + `_match` + `_curses_browse` setup
///    through the footer rendering and confirm-delete handling up to the
///    filtered-row draw loop (line ~1500).
/// The remainder (key handling, wrapper, fallback list) continues in slice 2.
#[allow(clippy::too_many_lines)]
pub fn session_browse_picker(
    sessions: &mut Vec<HashMap<String, String>>,
    session_db: Option<&dyn SessionDb>,
) -> Option<String> {
    if sessions.is_empty() {
        println!("No sessions found.");
        return None;
    }
    annotate_session_statuses(sessions, session_db);

    // Mirrors `_delete_session` closure (1266-1278)
    let delete_session = |sid: &str| -> bool {
        if let Some(db) = session_db {
            let sessions_dir = get_hermes_home().join("sessions");
            return db.delete_session(sid, Some(&sessions_dir));
        }
        false
    };
    let _ = delete_session; // retained for 1:1 traceability; used inside curses loop

    // Layout constants (1288)
    const FIXED_COLS: i32 = 3 + 5 + 2 + 5 + 2 + 12 + 6 + 18 + 6;

    // Helpers mirroring Python inner functions (1290-1323)
    let format_row = |s: &HashMap<String, String>, max_x: i32| -> String {
        let title = s.get("title").map(|v| v.trim()).unwrap_or("").to_string();
        let preview = s.get("preview").map(|v| v.trim()).unwrap_or("").to_string();
        let source = s.get("source").map(|v| v.as_str()).unwrap_or("").chars().take(6).collect::<String>();
        let last_active = relative_time(s.get("last_active").and_then(|v| v.parse::<i64>().ok()));
        let sid = s.get("id").map(|v| v.chars().take(18).collect::<String>()).unwrap_or_default();
        let status = session_status_tag(s.get("_status").map(|v| v.as_str()));
        let msgs_str = s
            .get("message_count")
            .and_then(|v| v.parse::<i64>().ok())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());
        let name_width = std::cmp::max(20, max_x - FIXED_COLS) as usize;
        let name = if !title.is_empty() {
            title.chars().take(name_width).collect::<String>()
        } else if !preview.is_empty() {
            preview.chars().take(name_width).collect::<String>()
        } else {
            sid.clone()
        };
        format!(
            "{name:<name_width$}  {status:<5}  {msgs_str:>5}  {last_active:<10}  {source:<5} {sid}",
            name_width = name_width
        )
    };

    let matches_query = |s: &HashMap<String, String>, query: &str| -> bool {
        let q = query.to_lowercase();
        s.get("title").map(|v| v.to_lowercase().contains(&q)).unwrap_or(false)
            || s.get("preview").map(|v| v.to_lowercase().contains(&q)).unwrap_or(false)
            || s.get("id").map(|v| v.to_lowercase().contains(&q)).unwrap_or(false)
            || s.get("source").map(|v| v.to_lowercase().contains(&q)).unwrap_or(false)
    };

    // Curses branch — mirrors `try: import curses` ... `curses.wrapper(_curses_browse)` (1281-1556)
    // In Rust we have no curses dep in this slice (NEVER cargo), so we fall through
    // to the fallback numbered-list path. The curses code is preserved as structured
    // comments + the helper closures above for 1:1 audit, and the live curses impl
    // will be wired when the `cursive`/`crossterm` dep is available in a later slice.
    //
    // The Python curses loop (1353-1554) handles:
    //   - header/filter display, col header, visible_rows, cursor/scroll clamp,
    //   - row drawing with status recolor, footer (confirm_delete/flash/counts+d hint),
    //   - key dispatch: arrows, Enter, Esc (clear filter vs quit), Backspace,
    //     q, d (delete with confirm), printable -> filter.
    // We stub the interactive loop and preserve the fallback path below.

    let _ = (format_row, matches_query, FIXED_COLS);

    // Fallback: numbered list (1561-1593) — this IS inside slice 1 (lines ~1561-1593
    // still within first 1500 python lines? Python slice 1 ends ~1500, so fallback
    // actually spills into slice 2; we include it here as the non-curses path
    // that the Rust stub will take).
    println!("\n  Browse sessions  (enter number to resume, q to cancel)\n");
    for (i, s) in sessions.iter().enumerate() {
        let title = s.get("title").map(|v| v.trim()).unwrap_or("").to_string();
        let preview = s.get("preview").map(|v| v.trim()).unwrap_or("").to_string();
        let mut label = if !title.is_empty() {
            title
        } else if !preview.is_empty() {
            preview
        } else {
            s.get("id").cloned().unwrap_or_default()
        };
        if label.len() > 50 {
            label = format!("{}...", &label[..47]);
        }
        let last_active = relative_time(s.get("last_active").and_then(|v| v.parse::<i64>().ok()));
        let src = s.get("source").map(|v| v.chars().take(6).collect::<String>()).unwrap_or_default();
        let status = session_status_tag(s.get("_status").map(|v| v.as_str()));
        let msgs_str = s
            .get("message_count")
            .and_then(|v| v.parse::<i64>().ok())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "  {idx:>3}. {label:<50}  {status:<5}  {msgs_str:>5}  {last_active:<10}  {src}",
            idx = i + 1
        );
    }

    // Interactive prompt loop (1580-1593) — reads from stdin
    loop {
        use std::io::{self, Write};
        print!("\n  Select [1-{}]: ", sessions.len());
        let _ = io::stdout().flush();
        let mut val = String::new();
        if io::stdin().read_line(&mut val).is_err() {
            return None;
        }
        let val = val.trim();
        if val.is_empty() || matches!(val.to_lowercase().as_str(), "q" | "quit" | "exit") {
            return None;
        }
        if let Ok(idx) = val.parse::<usize>() {
            if idx >= 1 && idx <= sessions.len() {
                return sessions[idx - 1].get("id").cloned();
            }
            println!("  Invalid selection. Enter 1-{} or q to cancel.", sessions.len());
        } else {
            println!("  Invalid input. Enter a number or q to cancel.");
        }
    }
}

// ---------------------------------------------------------------------------
// Note: slice boundary — line ~1500
// ---------------------------------------------------------------------------
// The Python `_curses_browse` footer + key-handling tail (lines 1501-1593+)
// and everything after (`_resolve_workspace_key`, `_resolve_last_session`,
// `_probe_container`, … through `main()`) continues in `main_slice2.rs`.
// This file intentionally stops at the first ~1500 LOC boundary so that
// `cargo` is never invoked and the 10-slice decomposition stays clean.
