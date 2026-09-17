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
