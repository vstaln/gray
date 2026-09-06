# gateway sidecar

Answers `/gateway` over `command/run` by delegating argv to the existing
`gray gateway …` CLI (`status|install|uninstall|pairing approve|list|revoke|invite`).
`gray` resolves via `PATH`, else the workspace `target/debug|release` build
(`plugins/cron/cron.sh` shape). `gateway run` (foreground daemon) is refused
as text — it would outlive the 30 s `command/run` TTL.
