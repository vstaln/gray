# Native plugin commands and above-editor widgets

Gray hosts plugin commands and display snapshots without depending on any plugin's
implementation. Plugins remain separate executable packages. This bridge adds no
model-facing tools; a plugin's sidecar manifest still controls its own tools.

## Register and invoke

```sh
# Native executable already built/installed locally:
GRAY_PLUGIN_PATH=/absolute/path/to/gray-example gray install plugin example
# Or find gray-example on PATH:
gray install plugin example

gray example settings
```

This registers a local executable, not a download-by-name registry. The existing
`gray plugin install` package manager retains responsibility for package fetching.
Registration executes `manifest` with a 10-second deadline and 64-KiB stdout cap.
Never register untrusted executables: they run with the current user's permissions.

The executable must support:

- `manifest`: emit the metadata object below, then exit successfully.
- Ordinary CLI argv (such as `settings`, `run`, `status`). No shell expansion is
  performed by Gray. Direct CLI forwarding preserves exit status and terminal I/O.
- No arguments: serve the existing NDJSON sidecar protocol (`plugin/manifest`,
  `command/run`, `prompt/context`, `plugin/shutdown`, etc., as claimed).
- `widget`: if opted in, emit one snapshot object and exit.

Example metadata (ordinary sidecar fields omitted for brevity):

```json
{
  "name": "example",
  "version": "1.0.0",
  "protocol": "1.1",
  "tools": [],
  "commands": ["/example", "/ex"],
  "completion": ["settings", "run", "status"],
  "widget": true
}
```

Installed, enabled command names participate in `/help` and completion before an
agent is built. No widget is required. Commands that already belong to Gray retain
precedence. Native aliases are resolved from `commands`, not guessed by pluralizing
names. Project disable overrides use the existing plugin-lock policy.

Slash arguments use shell-like lexical quoting through `shlex`, with **no shell
execution, variable expansion, glob expansion, or command substitution**:

```text
/ex run "two word task" '' '$(literal text)'
```

Unclosed quotes/trailing escapes report an error before invoking the plugin.
Both sidecar dispatch and native fallback receive the same parsed argv. Native
fallback has a 10-second deadline and 64-KiB stdout cap. A bare command receives
empty argv; default behavior is the plugin's decision. Nonzero native fallback
exit status is reported as an error; stderr is not painted into the TUI.

## Widget snapshot v1

```json
{
  "version": 1,
  "text": "Plugin work\nAn active row\nA detail row",
  "shimmer_lines": [1]
}
```

- This version supports **one registered above-editor slot**. A conflicting owner
  is rejected before registering the new command; no silent replacement.
- The slot identifies an installed owner. Executable argv comes from the command
  registry, never arbitrary argv from the slot file.
- A background worker polls every 500 ms, with a two-second execution deadline.
  No plugin I/O runs inside Ratatui draw. Invalid/failed snapshots clear the slot.
- The normal composer ticker animates indexed rows using Gray's existing shimmer.
- At most 64 KiB of stdout, 12 lines, and 4096 characters per line are accepted
  for display. Terminal control characters are removed. Ratatui clips to width;
  height is capped by available input-area space. Empty text hides the widget.
- Disabled/unregistered owners are not executed. Switching workspaces requires a
  fresh composer in this initial implementation.
- Shutdown stops polling; the current request remains bounded by its deadline.
  This is not a sandbox for malicious plugins or deliberately detached descendants.

Data lives under `$GRAY_HOME/plugins` (default `~/.gray/plugins`): `commands.json`,
`lock.json`, `<name>-manifest.json`, and `widgets.json`. Registration checks widget
ownership under a file lock. Files are individually atomically replaced; the
multi-file registration is not a crash-atomic transaction. Re-register the plugin
if interrupted. The executable must remain available at the registered path.

## Verification

```sh
CARGO_BUILD_JOBS=4 cargo check -p gray
CARGO_BUILD_JOBS=4 cargo test -p gray
CARGO_BUILD_JOBS=4 cargo clippy --workspace -- -D warnings
CARGO_BUILD_JOBS=4 cargo test --workspace -- --test-threads=1
cargo fmt --check
```

`crates/gray/tests/native_plugin.rs` runs the real CLI against temporary executable
fixtures and a fresh Gray home. The widget tests exercise registered-owner lookup,
disable handling, painting, shimmer, and control-character filtering without any
locally installed plugin. Quoting regressions also exercise the existing sidecar
splitter. `plugin_cli` tests cover output overflow and timeout cleanup.

A parallel full-workspace test run on the development machine intermittently
reported a pre-existing gateway filesystem permission failure. The serial full
workspace run passed without changing gateway or cron code; this is not a claim
that the parallel failure has been fixed.
