# Security policy

## Threat model

`gray` executes shell commands from the model. The threat surface is:
malicious or confused model output and untrusted tool/plugin results. There is no container or VM
isolation — run gray in a container/VM for untrusted work.

## Tool execution

There is no destructive-command guard and no approval prompt. Model-generated
commands run with your user privileges. `GRAY_GUARD_BYPASS` and
`GRAY_PERMISSION` are not supported controls. The default tool surface is
`bash`; optional plugins may add tools. Treat model and tool output as
untrusted, and use a container or VM when isolation is required.

## Plugin trust

Plugins are sidecar processes running with your user privileges — only
install plugins you trust. Plugin manifests are not an OS sandbox; a `tool/before` deny blocks the call.
Audit a plugin with `gray plugin check <dir>` before installing.
`gray plugin check` runs the plugin's `tool/call` path, so checking untrusted
code executes it. Audit the source first.

## Project content trust

A repo can ship instructions the model follows with unsandboxed tools:
`.claude/commands/*.md`, `.gray/prompts`, `.pi/prompts`, `SKILL.md` files, and
the `AGENTS.md` / `CLAUDE.md` rules gray serves as `<project_context>`. None of
that loads for a project until you trust it:

    gray trust            # the current project (its git root, or the cwd)
    gray trust --revoke   # stop trusting it

The list is `trusted_projects.json` in your gray home. It is never read from
inside the project, so a repo cannot trust itself. Until trusted, gray ignores
project-level content silently; the user-level prompts and skills still load.

## Raw tool output on disk

Full tool output is spilled to a local store (`gray spill cat` reads it back)
before redaction. Redaction applies to what goes into the model's context,
logs, and receipts, not to the spill store. Files are owner-only (0700 dir),
and the store is capped by count, not age. Treat it like the session
transcripts beside it: it can contain secrets a command printed. Delete it
when you no longer need it.

## Update trust model

Installs and `gray update` fetch the installer script, the tarball, and
`SHA256SUMS-<channel>` over HTTPS from one origin (`gray.alignment.id`). The sums
verify integrity in transit, not publisher identity: releases are not
signed yet. Background auto-update (`GRAY_AUTO_UPDATE=1`) runs on the
stable channel only — beta redeploys on every push to main. Operators who
need signed updates should build from source at a reviewed tag.

## Reporting

Private vulnerability reporting is currently disabled on this repository.
For sensitive reports, open an issue requesting a private contact channel
without including exploit details or secrets. Do not publish unpatched
vulnerability details in that request.
