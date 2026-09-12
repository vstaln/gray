# Security policy

## Threat model

`gray` executes shell commands from the model. The threat surface is:
malicious or confused model output and untrusted tool/plugin results. There is no container or VM
isolation — run gray in a container/VM for untrusted work.

## Destructive-command guard (scope, not a sandbox)

`crates/gray-tools/src/bash.rs` blocks obvious foot-guns (`rm -rf /`,
`mkfs`, fork bombs, `git reset --hard`) after an allow-prompt. Matching is
prefix/token-based: pipes, `&&` chains, `$(...)`, `eval`, `xargs rm`,
`find -delete`, `python -c 'shutil.rmtree(...)'` and `curl … | sh` pass
through. `GRAY_GUARD_BYPASS=1` disables it entirely.

## Tool permission

`GRAY_PERMISSION=ask|auto` controls guard `Prompt` verdicts (asked at the
tool/before seam, before the tool runs). Default is `ask` in the
interactive REPL and `auto` in `-p` print mode (no TTY to ask on).
`Deny` verdicts always block regardless of mode.

## Plugin trust

Plugins are sidecar processes running with your user privileges — only
install plugins you trust. `capabilities[]` in the manifest is advisory
(not enforced); a `tool/before` deny from any plugin blocks the call.
Audit a plugin with `gray plugin check <dir>` before installing.

## Update trust model

Installs and `gray update` fetch the installer script, the tarball, and
`SHA256SUMS` over HTTPS from one origin (`gray.alignment.id`). The sums
verify integrity in transit, not publisher identity: releases are not
signed yet. Background auto-update (`GRAY_AUTO_UPDATE=1`) runs on the
stable channel only — beta redeploys on every push to main. Operators who
need signed updates should build from source at a reviewed tag.

## Reporting

Report vulnerabilities privately via a GitHub security advisory on
[vstaln/gray](https://github.com/vstaln/gray) (Security tab →
Report a vulnerability). Do not open public issues for unpatched holes.
