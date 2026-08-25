# Port Recon: Root-Level Python Modules of hermes-agent

Source: `/home/vstaln/gray/reference/NousResearch/hermes-agent` (~62.7k LOC in root .py).
Method: `wc -l`, grep of top-level imports and `^class/^def` lists; whole files NOT read.
Labels: [PROVEN] = read directly via grep/wc; [CONJECTURED] = inferred from names/imports.

## 1. Per-module summary

LOC counts [PROVEN]:

| Module | LOC | Responsibility |
|---|---|---|
| cli.py | 21,510 | Monolith CLI/TUI entrypoint |
| hermes_state.py | 14,311 | SQLite session persistence core |
| run_agent.py | 9,269 | Agent loop (`AIAgent` class, ~8.6k lines alone) |
| hermes_state_search.py | 2,510 | Session search queries over state DB |
| hermes_constants.py | 1,710 | Paths/home-dir resolution, contextvar overrides |
| model_tools.py | 1,641 | Tool-definition registry bridge, arg coercion |
| trajectory_compressor.py | 1,598 | LLM-driven conversation compression (has `main()` CLI) |
| hermes_state_schema.py | 1,529 | DB schema/migrations |
| batch_runner.py | 1,330 | Parallel batch runs (`multiprocessing.Pool`) |
| toolsets.py | 1,083 | Toolset composition/resolution |
| mcp_serve.py | 1,060 | Expose sessions over MCP server (`EventBridge`) |
| utils.py | 924 | Misc helpers (yaml config, URL host matching) |
| hermes_state_common.py | 916 | Shared state-DB helpers |
| hermes_state_portability.py | 825 | Import/export sessions |
| hermes_logging.py | 800 | Queue-based async logging setup |
| mini_swe_runner.py | 732 | SWE-bench-style runner |
| toolset_distributions.py | 358 | Named toolset bundles w/ random sampling |
| hermes_bootstrap.py | 239 | Process bootstrap glue |
| hermes_time.py | 135 | Time formatting/config [CONJECTURED from size] |
| registration_lifecycle.py | 128 | `ReplacementCoordinator`/`ReplacementLease` — small concurrency primitive |

Key internals [PROVEN via def/class greps]:

- **cli.py**: giant flat module of sections — cost/usage formatting, prefill messages,
  CLI config loading (`load_cli_config`), a re-export shim layer (`def AIAgent(*args, **kwargs)`
  forwarding wrappers at lines 898–980), exit watchdogs/cleanup, **git worktree management**
  (~lines 1540–2950: setup/prune/merge-check of worktrees), ANSI color/light-mode skin
  detection (OSC11 terminal query). Also uses mixin classes imported from `hermes_cli.*`
  (`CLIAgentSetupMixin`, `CLICommandsMixin`, `CLIBillingMixin`). It is the top of the
  dependency DAG, not a foundation.
- **hermes_state.py**: WAL/journal-mode handling (`apply_wal_with_fallback`,
  `resolve_journal_mode`, WAL-reset-vulnerability detection for specific sqlite builds),
  pragma application, malformed-db/disk-full error classification, cross-process repair
  locking + repair ledger + backup fingerprinting, delegate-child message deletion,
  pytest isolation guards, transcript size limits. Deep sqlite operational-hardening code.
- **run_agent.py**: single `AIAgent` class (line 421 → `main` at 9053): streaming, retries,
  rate-limit recovery, interrupt handling, session persistence hooks. Imports
  `hermes_cli.env_loader`, `agent.iteration_budget`, `model_tools`.
- **model_tools.py**: bridges `tools.registry`; per-contextvar tool-loop/worker-loop
  management, `_run_async` bridging sync→async, JSON-schema-driven argument coercion
  (string→bool/number/nested), tool-error sanitization/truncation.
- **toolsets.py**: pure dict/graph logic — resolve named toolsets with cycle-safe
  visited-set recursion, bundles, custom toolsets. Zero heavy deps (typing only).
- **trajectory_compressor.py**: `CompressionConfig`/`TrajectoryMetrics`/`TrajectoryCompressor`
  classes; calls OpenRouter (OPENROUTER_BASE_URL), rich progress UI, yaml config, jittered
  backoff, fire CLI.
- **hermes_constants.py**: `get_hermes_home()` + ContextVar override tokens, platform
  default home, profile fallback warnings, node-tool discovery helpers.
- **registration_lifecycle.py**: tiny — lease/coordinator for replacement registration
  (threading.Lock based).

## 2. Internal import graph (bottom of DAG first) [PROVEN]

```
hermes_constants        ← imported by hermes_state*, run_agent, trajectory_compressor, hermes_logging, hermes_time
utils                   ← trajectory_compressor (base_url_host_matches); standalone otherwise (yaml only)
toolsets                ← model_tools, toolset_distributions
toolset_distributions   ← batch_runner
model_tools             ← run_agent, batch_runner, cli
hermes_state_common     ← hermes_state_schema, _portability, _search (and agent.skill_commands/context_compressor)
hermes_state            ← COMPOSES the three submodules as mixins:
                           `class ...(SessionPortabilityMixin, SessionSchemaMixin,
                           SessionSearchMixin)` (lines 96–98) and re-exports
                           hermes_state_common for back-compat (line 60).
                        ← mcp_serve (_get_session_db), cli (_run_state_db_auto_maintenance)
run_agent               ← batch_runner (`from run_agent import AIAgent`); cli via shim.
                          Also imports utils (atomic_json_write, base_url_* , env_float...)
mini_swe_runner         ← does NOT import other root modules at top level (agent.* only)
trajectory_compressor   ← standalone CLI (imports utils, hermes_constants only)
hermes_bootstrap        ← leaf; Windows-only UTF-8 env bootstrap (PYTHONUTF8 setdefault,
                         idempotent global flag). Near-noop for Rust; skip or fold into bin main.
mcp_serve / batch_runner / mini_swe_runner ← leaf executables
registration_lifecycle  ← no root-module imports seen (leaf)
```
Cross-package note: these modules also import `agent.*`, `tools.registry`, `hermes_cli.*`
(mixins, env_loader, timeouts, fallback_config) — the root layer is NOT fully self-contained;
the port must sequence those packages too.

## 3. External PyPI deps → Rust crates

Only third-party imports observed in root files:
- `prompt_toolkit` (cli.py, heavy) → `crossterm` (+ `ratatui` if TUI panels needed)
- `rich` (progress/console) → `indicatif` + `console` crates
- `fire` (CLI arg dispatch) → `clap`
- `yaml` → `serde_yaml`
- `sqlite3` (stdlib) → `rusqlite` (bundled feature)
- `dotenv` → `dotenvy`
- `multiprocessing` (batch_runner Pool) → `rayon` or std threads + channels
- stdlib-only elsewhere → std Rust.

## 4. Proposed Rust base-crate layout

```
crates/
  hermes-paths      ← hermes_constants + parts of utils (home dirs, path keys)
  hermes-util       ← utils + hermes_time + hermes_logging (async logger via channel)
  hermes-toolsets   ← toolsets + toolset_distributions + model_tools' coercion layer
  hermes-state      ← hermes_state (facade) + _common/_schema/_search/_portability as
                       Rust modules behind one `SessionStore` type (Python used mixin
                       classes; Rust: impl blocks per module on the same struct)
  hermes-agent-core ← run_agent::AIAgent loop + registration_lifecycle primitives
  hermes-compress   ← trajectory_compressor
  bins: hermes-cli (cli.rs), hermes-mcp-serve (mcp_serve), hermes-batch (batch_runner),
        hermes-swe (mini_swe_runner), bootstrap glue folded into bins
```

## 5. Rust risks

1. **SQLite WAL hardening** (hermes_state.py ~2k lines): Python code works around
   sqlite-build-specific bugs (WAL reset vulnerability, macOS checkpoint barrier,
   `synchronous=FULL`, journal-mode fallback to DELETE). rusqlite/libsqlite3-sys bundles a
   *different* sqlite build — those bug workarounds may be dead weight or wrong. Port the
   pragma/fallback *policy* but re-validate against bundled sqlite version. Cross-process
   repair locking (file locks + ledger) maps to `fs2`/`flock`.
2. **Global singletons**: ContextVar home overrides (`get_hermes_home_override`),
   contextvar tool loops in model_tools, process-wide callbacks in cli. Rust: pass handles
   explicitly or use `tokio::task_local!`; avoid `static mut`. This is the biggest API-shape
   divergence — Python callers rely on ambient state.
3. **YAML config** (utils, trajectory_compressor, load_cli_config returning loose Dict):
   needs typed serde structs; cli.py's 500-line `load_cli_config` with defaults/merging is
   a translation hazard.
4. **Compression** (trajectory_compressor): network-dependent LLM calls with backoff —
   port as async reqwest client; metrics classes are straightforward serde.
5. **cli.py monolith**: mixes pure helpers (table realignment, ANSI) with process control
   (atexit watchdogs, signal handling, worktrees via shelling out to git). Worktree code is
   really "git orchestration" — consider keeping as subprocess calls to git in Rust too
   (lazy, faithful).
6. **Sync/async mixing**: model_tools `_run_async`, asyncio in hermes_state/run_agent —
   pick tokio early; the shim layers will hurt otherwise.

## 6. Port-first order (zero/minimal internal deps)

1. `hermes_constants` (paths; only stdlib) — but strip node-discovery until needed.
2. `utils` (yaml + URL helpers; only serde_yaml dep).
3. `hermes_time` (135 LOC, deps hermes_constants).
4. `hermes_logging` (deps hermes_constants only).
5. `registration_lifecycle` (128 LOC, stdlib only).
6. `toolsets` (pure data logic) → `toolset_distributions`.
7. `hermes_state_common` → `_schema` → `_search` → `_portability` → `hermes_state` (the big
   one; do after rusqlite spike proves WAL pragmas).
8. Then `model_tools`, `run_agent` (needs agent.* package ported), then cli/batch/mcp bins.

Honest gap: `agent.*`, `tools.*`, `hermes_cli.*` packages were not reconnoitered here; they
gate steps 7–8. LOC of `run_agent.AIAgent` body (~8.6k lines single class) is the largest
single translation unit in this layer.
