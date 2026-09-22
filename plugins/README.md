# Official plugins (monorepo — no separate grayplugins repo)

Sidecars ship here so `gray plugin install <name>` never points at a repo
that does not exist. Each dir is one plugin: a single executable sidecar
(`plugin.sh`) speaking protocol v1 NDJSON over stdio (see `echo/` for the
reference). `gray plugin check <dir>` boots it exactly like `gray.yml` would.

| dir | manifest name | commands | asset |
|---|---|---|---|
| `discord/` | `discord` | `/discord` | `graydiscord-<ver>.tar.gz` (no hyphen — matches the published index) |
| `background/` | `background` | `/bg` | `gray-background-<ver>.tar.gz` |
| `permissions/` | `permissions` | `/perms` | `gray-permissions-<ver>.tar.gz` |

Release: push tag `plugins-v<version>` (must equal every manifest's
`version`). `.github/workflows/plugins-release.yml` syntax-checks (`sh -n`),
tarballs each dir, and creates release `plugins-v<version>` with the assets +
`SHA256SUMS-plugins`. Then copy `index.json` to the site, replacing each
`FILL_SHA256` with the real digest.

`discord/` and friends are scaffolds until the real bridge/runner/gate logic
is ported in — each file's TODO says where (token config, hooks, wire notes).
