# Releasing

One source of truth: `version` in `Cargo.toml` ([workspace.package]).

## Channels

| event | artifact | who gets it |
|---|---|---|
| push to `main` | `dl/gray-beta-x86_64-linux.tar.gz` | `curl …/install.sh \| sh -s -- beta` |
| tag `v0.2.0-beta.1` | beta file + GitHub **prerelease** | pinned named beta |
| tag `v0.1.0` | `dl/gray-stable-x86_64-linux.tar.gz` + GitHub release | `curl …/install.sh \| sh` |

Tag must equal the Cargo.toml version or CI fails (`v` prefix, e.g. Cargo.toml
`version = "0.2.0"` → tag `v0.2.0`).

## Cutting a stable release

```bash
# 1. bump the version
sed -i 's/^version = ".*"/version = "0.2.0"/' Cargo.toml
git commit -am "release 0.2.0" && git push        # → beta build runs too

# 2. tag it
git tag v0.2.0 && git push origin v0.2.0          # → stable deploy + GH Release
```

## Notes

- Binaries are static musl builds (x86_64 linux); rustls only, no openssl.
- Deploys go to `/var/www/gray/dl/` on oracle-new (nginx, behind Cloudflare).
  Requires the `DEPLOY_KEY` GitHub secret (opc@168.110.210.65).
- Local manual deploy: `scripts/deploy.sh <stable|beta>` after building musl.
- If downloads ever serve stale files, purge the Cloudflare cache for
  `/dl/*` — origin sends `Cache-Control: no-cache` since the CI setup.
