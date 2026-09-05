# Changelog

## [1.0.0] - 2026-09-05

### Added
- Verified installs: SHA256SUMS published per release, checked by install.sh (S1)
- `GRAY_NO_UPDATE_CHECK=1` and 24h update-check cache (L4)
- Gateway autostart defaults off; corrupt gateway.yaml warns instead of silently resetting (S2, S3)
- Safety / Subcommands / Platform / gateway docs in README (D2, S4)

### Fixed
- Stable update channel: `latest-stable.txt` now published; beta builds embed the beta channel (D1)
- Single-writer publish job: all four platform tarballs land atomically (D4, R2)
- Installer defaults to `~/.local/bin` (`--system` / `GRAY_INSTALL_DIR` for system-wide) (L3)
- Swapped unmaintained `serde_yaml` for `serde_yaml_ng`; `cargo audit` in CI (C2)
- Gateway `--help` names all three platforms (Telegram/Discord/Slack)
- TUI: bottom status line spans the full width; diff overlay extends fully to the right edge

### Known issues
- macOS binaries are not notarized (curl-install unaffected) (D3)
- Destructive-command guard is best-effort, not a sandbox — see README Safety (S4)
