# Port Recon: `hermes_cli/` core (excluding web server cluster)

Source: `/home/vstaln/gray/reference/NousResearch/hermes-agent/hermes_cli/`
Scope: everything EXCEPT web_server.py, web_routers/, dashboard_auth/, dashboard_procs.py, web_*.py.
Total hermes_cli ≈ 226k LOC (incl. excluded web files). Core subsystem is still the bulk (~200k).
Labels: PROVEN = read directly; CONJECTURED = inferred.

## 1. Command surface inventory

**Architecture: plain argparse, no click/typer** (PROVEN, `_parser.py`, main.py).

- `_parser.py` — builds ONLY top-level parser + `chat` subparser. Other subparsers built inline in `main.py` (`subparsers.add_parser(...)` calls, ~line 12700+). `PRE_ARGPARSE_INHERITED_FLAGS`: `--profile/-p` consumed pre-argparse (sets HERMES_HOME, strips from argv) — special-cased, must replicate as global flag.
- Relaunch mechanism: actions tagged `inherit_on_relaunch = True`; `relaunch.py` introspects parser to carry flags across self re-exec. Rust equivalent: a struct of carried-over flags, not introspection.
- `main.py` (14.2k lines): giant `_build_parser()` + `_SUBCOMMANDS` dict (line ~10432) + `_BUILTIN_SUBCOMMANDS` frozenset (~11881) + `_AGENT_COMMANDS`/`_AGENT_SUBCOMMANDS` maps deciding whether a command runs in-process vs spawns agent loop. Dispatch is dict-driven with module-level `cmd_*` functions.
- **Dynamic registration**: e.g. `from hermes_cli.portal_cli import add_parser as _add_portal_parser` (main.py:13034) — modules export `add_parser(subparsers)`; plugins may register commands too (CONJECTURED from naming; check plugin loader during port).
- `subcommands/` (~45 files): one file per command group (`acp, approvals, auth, backup, claw, config, console, cron, dashboard, debug, doctor, dump, gateway, gui, hooks, import_agent/import_cmd, insights, login/logout, logs, mcp, memory, model, monitoring, pairing, pause, peer, plugins, profile, prompt_size, security, setup, skills, skin, slack, status, sync, tools, uninstall, update, verify, webhook, whatsapp`, plus `_shared.py`). Each mostly thin wrappers delegating to flat modules in `hermes_cli/`.

### Mixin composition
`cli.py:5011` (PROVEN):
```python
class HermesCLI(CLIAgentSetupMixin, CLICommandsMixin, CLIBillingMixin):
```
- `CLICommandsMixin` (3.9k lines): chat REPL command handlers (`/model`, `/sessions`, etc.), rich Panel rendering.
- `CLIAgentSetupMixin`: setup wizard flows.
- `CLIBillingMixin`: billing/account commands.
Mixins are stateless-ish method bags over shared HermesCLI state → Rust: traits with default methods over a shared `CliContext` struct, or free functions taking `&mut CliContext`. No diamond inheritance issues observed (single linear chain).
Flat `commands.py` (2.4k lines) also holds slash-command implementations.

## 2. Deps reaching OUTSIDE hermes_cli/

PROVEN via grep of imports (flat modules + subcommands + proxy + observability):

- `agent.*`: credential_persistence, credential_pool, model_metadata, models_dev (ModelInfo, PROVIDER_TO_MODELS_DEV), proxy_sources, reasoning_effort (EFFORT_LADDER), relay_runtime, secret_scope, secret_sources (onepassword), skill_bundles, skill_utils (yaml_load), turn_context
- `gateway.*`: config, restart, session_context, status (terminate_pid)
- `tools.*`: ansi_strip, environments.local, managed_tool_gateway, mcp_tool, tool_backend_helpers, voice_mode
- `cron.lifecycle_guard`
- Root package `.py`: `hermes_constants`, `hermes_state`, `plugins` package, `utils`
- Inbound: root `cli.py` composes HermesCLI from these mixins — cli.py itself is NOT in this subsystem but is its primary consumer.

Implication: port order must place agent/gateway/tools type+config primitives before or stubbed alongside CLI work.

## 3. External PyPI deps → Rust crates

Third-party imports seen (PROVEN grep): rich, dotenv, fastapi (web only), httpx, requests, pydantic (web-heavy), yaml, noise(?), plus stdlib-heavy code (argparse, curses, sqlite3, termios, fcntl, ctypes, tarfile, secrets).

| Python | Rust crate |
|---|---|
| argparse | clap v4 (derive) |
| rich (Panel/markup/colors) | ratatui only for TUI parts; for styled output use `console`+`indicatif` or port banner.py/colors.py by hand (they're small custom wrappers anyway) |
| prompt_toolkit / curses_ui.curses_single_select | crossterm (+ dialoguer for pickers) |
| psutil (psutil_android.py shim) | sysinfo |
| keyring-style cred storage (auth.py, config save_env_value_secure) | keyring crate |
| python-dotenv | dotenvy |
| requests/httpx | reqwest (+ rustls) |
| PyYAML | serde_yaml |
| sqlite3 | rusqlite |
| fastapi/pydantic | EXCLUDED scope (web cluster) — but proxy/server.py may pull axum; check at port time |
| webbrowser | webbrowser crate |
| keychain/secret storage | keyring crate |

Note: much "rich" usage goes through local wrappers `banner.py` (`cprint`), `colors.py` (`Colors, color`), `cli_output.py` (`line_input, prompt, prompt_yes_no`) — port these once, they become the terminal abstraction seam.

## 4. Port order within subsystem

1. Leaf utilities: timefmt, sizefmt, colors, banner, cli_output, timeouts, input_sanitize, sqlite_util/safe_read, _subprocess_compat
2. config.py + config_defaults + config_migrations + env_loader + get_hermes_home (foundation everything reads)
3. auth.py (9.5k lines — biggest single risk after main.py) + PROVIDER_REGISTRY
4. _parser.py + main.py argparse skeleton + relaunch mechanism (get dispatch table working early)
5. Flat cmd modules by dependency order: sessions_cmd/session_* family, profiles, models/model_*, providers, fallback, moa
6. Mixins + cli.py-facing surface (needs upstream cli.py contract)
7. pty_session/pty_bridge/win_pty_bridge (platform-split, portable core first)
8. kanban* (kanban_db → rusqlite), service_manager (systemd/launchd/windows strategy structs), update_cmd/uninstall/setup (8.5k-line update_cmd — defer, it's self-contained ops tooling)
9. observability/, proxy/ (server-shaped; needs axum decision)
10. Long tail: doctor, diagnostics, plugins*, mcp_*, voice, pets, etc.

## 5. Rust risks

- **Mixin inheritance → trait composition**: mechanical but HermesCLI is 5k lines in cli.py (out of scope); define trait boundary contract first.
- **Dynamic/introspective argparse**: parser introspection for relaunch, runtime `add_parser` from other modules → replace with declarative static clap tree + explicit registry enum. Plugin-added commands are the hard case.
- **curses_ui**: raw curses → crossterm; alternate-screen handling differs per platform.
- **Windows paths**: win_pty_bridge.py, gateway_windows.py, windows_ssh_runtime.py, WindowsServiceManager, linux_desktop_entry.py — cfg-gate per-OS modules; pty on Windows needs conpty (portable-pty crate).
- **Pre-argparse argv mutation** (--profile stripped before parsing) — unusual control flow; do it explicitly in Rust main.
- **14k-line main.py**: don't translate linearly; extract each `cmd_*` into its own module as you go.
- **auth.py 9.5k lines** mixing OAuth flows, keyring, provider registry — split by provider.
- **fcntl/termios/ctypes POSIX-isms** scattered (single-instance locks, terminal size).

## 6. Load-bearing vs peripheral

**Load-bearing**: config.py, auth.py, _parser.py/main.py dispatch, cli mixins, sessions_cmd family, profiles, banner/cli_output terminal seam, relaunch.
**Peripheral** (self-contained, port late or drop): pets.py, tips.py, moa_cmd, journey.py, claws/pets flavor modules, session_export_html, diagnostics_upload, xai_retirement/migrations (one-shot migrations — likely skip entirely), update_receipt/update_lock bookkeeping.
**Ambiguous**: update_cmd.py (8.5k lines of self-updater — question whether Rust version self-updates or delegates to cargo/brew/installer); kanban swarm (large, standalone DB app inside the CLI).

— adventurer recon, session $(date -u +%F). Sources: directory listing, wc -l, targeted greps of imports/parser structure/mixin defs. Whole-file reads: none >60 lines except grep output.
