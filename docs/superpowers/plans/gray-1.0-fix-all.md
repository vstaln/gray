# gray 1.0 fix-all — folded from readiness report (data.ts) + 4-agent verification

Base: fix/release-readiness @ 0e5bd36 (includes #22). Report base was 28dddfe.
HEAD here says version 1.0.0, tags max v0.1.0, docs/ has only superpowers/, CI ubuntu-only no fmt/-D.

## Global Constraints
- Never push to main. Work in fix/release-readiness, PR at end.
- `cargo test --workspace` must stay green. No new deps without need.
- Smallest diff that closes the finding. No speculative refactors.
- Exact values: version honest (0.9.0 or 0.1.0, never phantom 1.0.0); stability list = CLI flags, session JSONL, plugin wire v1, ~/.gray layout; CI = fmt --check, clippy -D warnings, ubuntu+macos, gateway all-platforms check.

## Preflight scan (2026-09-05)
- p0-1 vs p3-5 conflict? No — p0 sets 0.9.0 honest, p3 bumps to 1.0.0 only at tag. Ordered, not contradictory.
- p1-1 (default-members) vs p2-6 (cron as plugin): ordered — cut first, port second. No conflict.
- F1 reopened in this worktree (1.0.0, no tag) vs 86cb6b2 where it was fixed (0.1.0). Ruling: set 0.9.0 + Unreleased here.
- docs/protocol-v1.md missing here (only superpowers/). p2-1 needs spec restore from main lineage (b98e284 has v1.1). Ruling: restore spec from b98e284, don't rewrite.

## Task 1 — Phase 0 honesty

p0-1 version 0.9.0, CHANGELOG [1.0.0]→[Unreleased]. Exact: workspace.package version 0.9.0, changelog block to Unreleased.
p0-2 README links resolve (docs/img/hokusai-kajikazawa.jpg add/drop, grep all relative links and verify each exists).
p0-4 CI fmt --check, clippy -D warnings, matrix ubuntu+macos, cargo check -p gray-gateway --features all-platforms. Fix drift in one `chore: fmt + clippy` commit.
p0-5 decide PR #25/#26 (gh pr list, merge or close with reason).
Test: `cargo test --workspace` green, `cargo fmt --check` clean.

## Task 2 — Phase 1 scope cut

p1-1 workspace.default-members core only (gray, gray-core, gray-provider, gray-session, gray-tools, gray-plugin, gray-markdown; gateway+cron stay members not default).
p1-2 gray-tools depends on gray-core only: cron_tool.rs → gray-cron, skill.rs → plugins/skills, plugin.rs loader → crates/gray/src/profile.rs.
p1-3 Move proxy.rs, oauth.rs, cron_cli.rs out of crates/gray/src into crates/gray-extras/ non-default member.
p1-4 Split repl/mod.rs (177KB) and setup/mod.rs (132KB), target no file in crates/gray over 25KB. Mechanical moves, zero logic.
p1-5 Feature-flag clipboard/image attachments (arboard+image behind `clipboard` default-off). Record binary size before/after in commit msg.
p1-6 Publish gray-markdown 0.1 (or ledger ruling to defer if registry blocked).
Test: `cargo build` default has no twilight/teloxide/slack/axum/arboard/image/cron in `cargo tree`; `cargo test --workspace` green.

## Task 3 — Phase 2 protocol freeze

p2-1 Land v1.1: plugin/shutdown, session:{id,cwd}, turn_end emission. If docs/protocol-v1.md missing restore from b98e284, don't rewrite.
p2-2 Add plugin→host requests host/run {session,prompt} and host/say {text}. Sidecar-originated ids separate namespace.
p2-3 Manifest capabilities[] (exec|http|session|ui) and subcommands[] (cron,gateway → forward argv via command/run).
p2-4 Publish docs/schema/manifest.v1.json + protocol.v1.json, validate echo plugin in CI.
p2-5 `gray plugin check <dir>` conformance runner (hang, crash, reorder, empty_name fixtures).
p2-6 Port cron to plugin using host/run first, then gateway. If cron can't be plugin, protocol isn't done — stop and report.
p2-7 One build_agent() for REPL, -p, gateway, cron (closes F8).
p2-8 docs/plugins.md authoring guide (manifest, methods, TTLs, capabilities, check cmd, publish). Link echo, skills, cron.
Test: protocol doc host-emission zero ❌, cron runs out-of-process over public wire, `cargo test -p gray-plugin` green.

## Task 4 — Phase 3 release 1.0

p3-1 README stability promise verbatim: Stable in 1.x: CLI flags, session JSONL schema, plugin wire v1, ~/.gray layout. Not stable: TUI, internal crate APIs, gray-markdown.
p3-2 SECURITY.md one-page threat model (guard scope, plugin trust, gateway allowlist, report path).
p3-3 permission: ask|auto (default ask interactive, auto for -p) via tool/before seam.
p3-4 Release workflow 4 tarballs + SHA256SUMS, installer verifies.
p3-5 Tag v1.0.0 only after artifacts exist, changelog entry after tag. Then open gray-gateway repo 0.1.0.
Test: DoD 8/8 — cargo tree clean, protocol zero ❌, cron plugin, no file >25KB, CI green linux+macos, release assets 4+SHA, README stable+links, version==tag.
