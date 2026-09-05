# Security policy

## Threat model

`gray` executes shell commands from the model. The threat surface is:
malicious or confused model output, untrusted tool/plugin results, and
(over the gateway) untrusted chat users. There is no container or VM
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

## Gateway allowlist

The gateway (`gray gateway`) is deny-by-default: nobody talks to the agent
unless allowlisted. Keep `platforms.<p>.token` secret, `gateway.yaml` is
written `0600` (owner-only), and prefer `dm_policy: pairing` over `open`
(`allowed_users: "*"` admits everyone). Extra `denied_tools` merge with
the built-in gateway deny set.

## Reporting

Report vulnerabilities privately via a GitHub security advisory on
[vstaln/gray](https://github.com/vstaln/gray) (Security tab →
Report a vulnerability). Do not open public issues for unpatched holes.
