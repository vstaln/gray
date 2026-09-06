# Dogfood Harness Design (black-box, Approach A)

Goal: genuinely dogfood `gray` end-to-end by driving the real binary headless — every slash command, every CLI subcommand, real tasks, full TUI checks. No Rust imports, no unit-test shortcuts.

## Architecture
One throwaway driver outside the repo (`/tmp/gray-dogfood/`): builds `gray` once, then per-case spawns the binary in an isolated sandbox. Print mode (`-p`) for task oracle, PTY (`tmux` capture / `python pty`) for interactive TUI. Optional `ttyd` + `browser-use` skill for screenshots of pickers/transcript.

## Components
- `build.sh`: `cargo build --release -p gray` → binary under test.
- `run-case.sh`: `spawn(cmd, env, keys, expect)` — isolated `GRAY_HOME=$(mktemp -d)`, `cwd` fixture repo, `GRAY_NO_UPDATE_CHECK=1`, `GRAY_GUARD_BYPASS=0`, free provider env passthrough (Muse Spark; shape-checked only, never dumped).
- `matrix.txt`: full command list (see below). Each line = one case with `expect` string.
- `tasks/`: 5 real-task fixtures (explore-fix, edit-flow, shell-contracts, context-mgmt, permissions).

## Data flow
`matrix.txt` → `run-case.sh` → PTY snapshot + exit code + `GRAY_HOME` diff (sessions JSONL, logs) → `results/` PASS/FAIL table with tails. Failures attach snapshot + session JSONL path.

## Scope (full matrix, free-only)
Slash: `/connect` (free/API-key/local only), `/model`, `/thinking`, `/context`, `/resume`, `/new`, `/compact`, `/usage`, `/permissions` + Shift+Tab, `/feedback` (local save), `/acp list|off`, `/agentsmd show`, `/skills`, `/help`, `/quit`, aliases, Enter-fires vs Tab-inserts.
CLI: `-p`, `-c`, `--session`, `--context-window`, `--dump-manifest`, `resume`, `cron`, `proxy status|providers`, `gateway status|pairing list`, `update --help` only.
Skipped (marked manual): xAI/Codex OAuth login, gateway bot tokens + `run/install` daemons, live `update`. Render-only asserts.

## Error handling
Guard stays ON; destructive cases assert the block message. Timeouts kill the PTY pane, mark FAIL with tail. No secrets in output — redact `token|secret|key|password` values.

## Testing
Harness is the test: PASS = exit code + `expect` string + artifact (session JSONL / edited file / block message). TUI asserts are `contains` on snapshots, not pixel-perfect, plus 3–5 screenshots.
