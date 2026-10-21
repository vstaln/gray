# TUI Plugin Marketplace Implementation Plan

**Goal:** Add a core Marketplace tab to `/plugin` for browsing, details, and explicitly consented installation, followed by optional in-session activation.

**Architecture:** Keep uncached catalog access and staged package transactions in `gray-pkg`; keep executable probing, scan policy, capability consent, and prepared-manifest adoption in the host. A plugin-specific modal controller owns a pure UI model, rendering, and cancellable background workers; the REPL owns activation after the modal closes. Leave CLI catalog caching, other managers, and plugin protocol unchanged.

**Tech Stack:** Rust 2024, Tokio, tokio-util cancellation, Reqwest, Serde, tempfile, tar/flate2, Ratatui, Crossterm, Axum test servers, Cargo/Clippy/nextest.

---

## Execution context and rules

Worktree: `/Users/danielou/coding_projects/gray/worktrees/tui-plugin-marketplace`.
Branch: `feat/tui-plugin-marketplace`.
Design: `docs/specs/2026-10-17-tui-plugin-marketplace-design.md`, commit
`75a989df`. References below were checked against this worktree, whose code
base is `origin/main` at `5361733d`, not `local/live-fixes`.

Run every command from the worktree:

```sh
cd /Users/danielou/coding_projects/gray/worktrees/tui-plugin-marketplace
```

Do not push. Each task follows red/green/refactor/commit. A red check must fail
for the intended assertion or missing API, not a broken unrelated fixture.
Only commit named task files and inspect `git diff --cached` first. Do not
overwrite unrelated work. The final phase runs the required workspace checks.

The code blocks below establish the implementation contracts, concrete
algorithms, and executable regression tests. Keep each task's named APIs
consistent across phases. Error strings named in assertions are part of the
new Marketplace path, not changes to existing CLI messages.

## Phase map and file ownership

| Phase | Depends on | Owned files | Exit check |
| --- | --- | --- | --- |
| A: package primitives | approved spec | `crates/gray-pkg/{Cargo.toml,src/lib.rs,src/index.rs,src/index_tests.rs,src/fetch.rs,src/ops.rs,src/staged.rs,src/staged_tests.rs,tests/index.rs}`, `Cargo.lock` dependency list | `cargo test -p gray-pkg` |
| B: host installation | A | `crates/gray/src/{lib.rs,plugin_cli.rs,plugin_install/mod.rs,plugin_install/policy.rs,plugin_install/probe.rs,plugin_install/tests.rs}` | `cargo test -p gray --lib plugin_install` and `cargo test -p gray --lib plugin_cli` |
| C: Marketplace UI | A, B | `crates/gray/src/setup/{mod.rs,install_manager.rs,install_manager_tests.rs,marketplace/mod.rs,marketplace/model.rs,marketplace/draw.rs,marketplace/worker.rs,marketplace/tests.rs}`, `crates/gray/src/repl/plugin_cmds.rs` | `cargo test -p gray --lib marketplace` and `cargo test -p gray --lib install_manager` |
| D: activation and release checks | C | `crates/gray/src/repl/{plugin_cmds.rs,plugin_cmds_tests.rs,dispatch.rs,handlers.rs,handlers_tests.rs}`, `crates/gray/src/setup/marketplace/activation.rs` | focused activation tests, then required workspace CI |

Execute phases in order. A and B expose tested internal APIs without changing
existing user paths. C connects a working install UI; it returns installed
names without silently activating them. D adds the approved activation prompt.
One implementation agent can own each phase after its predecessor passes.
Shared registration files are transferred at the phase boundary, never edited
by simultaneous owners.

### Baseline anchors

- `gray-pkg/src/index.rs:38–76`: kind, author, license, commands, dependency
  and conflict fields already exist. Add only missing display fields.
- `gray-pkg/src/index.rs:140–189`: cached CLI and hub fetchers; leave intact.
- `gray-pkg/src/fetch.rs:65–135`: verified download currently returns a kept
  path; `156` onward bounds tar extraction.
- `gray-pkg/src/ops.rs:100–178`: registry I/O and guard; `312` onward is the
  existing archive replacement/rollback; `373` onward resolves the hub first
  and auto-installs dependencies. Do not use that name-based entry point.
- `gray/src/plugin_cli.rs:288`, `395`: scan and consent adapters;
  `875–918`: CLI probe and wire fallback; `1034–1091`: adoption;
  `1354–1395`: bounded child capture.
- `gray/src/setup/install_manager.rs:32–41`: existing `ManagerOutcome`;
  `199–231`: plugin wrapper around the shared manager.
- `gray/src/setup/tabs.rs:31–37`: shared Installed/Errors enum, not the
  location for an unconditional Marketplace variant.
- `gray/src/setup/connect.rs:115–129`: background-task/channel precedent.
- `gray/src/repl/plugin_cmds.rs:53–86`: bare picker dispatch;
  `gray/src/repl/handlers.rs:489–510`: history-preserving agent reload.

## Phase A: catalog and package transactions

### Task A1: compatible metadata and uncached fetching

**Files**

- Modify: `crates/gray-pkg/src/index.rs:38–76,140–146`
- Modify: `crates/gray-pkg/src/index_tests.rs`
- Modify: `crates/gray-pkg/tests/index.rs`

- [ ] **1. Add a failing compatibility test.**

```rust
#[test]
fn marketplace_metadata_is_optional() {
    let entry: super::Entry = serde_json::from_value(serde_json::json!({
        "ecosystem": "gray-native", "version": "1.0.0",
        "source": {"type": "tarball", "url": "https://example.com/demo.tgz"},
        "hash": "sha256:abc"
    })).unwrap();
    assert!(entry.description.is_none());
    assert!(entry.homepage.is_none());
    assert!(entry.repo.is_none());
    assert!(entry.commands.is_empty());
}
```

Run `cargo test -p gray-pkg --lib marketplace_metadata_is_optional`.
Expected: compile failure for the three missing fields.

- [ ] **2. Add the fields and the cache-free API.**

Add these fields to `Entry`, preserving all existing fields and defaults:

```rust
#[serde(default)]
pub description: Option<String>,
#[serde(default)]
pub homepage: Option<String>,
#[serde(default)]
pub repo: Option<String>,
```

Add this function without routing through `fetch_index_url`:

```rust
pub async fn fetch_index_uncached(client: &reqwest::Client) -> anyhow::Result<Index> {
    let url = index_url();
    crate::fetch::check_url(&url)?;
    let response = client.get(&url).send().await?;
    anyhow::ensure!(!response.status().is_redirection(), "catalog redirect refused");
    anyhow::ensure!(
        response.status() != reqwest::StatusCode::NOT_MODIFIED,
        "catalog returned 304 without a cache"
    );
    Ok(response.error_for_status()?.json().await?)
}
```

- [ ] **3. Add the uncached regression test using the existing integration
  file's `ENV_LOCK`, `spawn`, `use_env`, and `minimal_index` helpers.**

```rust
#[tokio::test]
async fn uncached_fetch_never_uses_or_updates_disk() {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    let _lock = ENV_LOCK.lock().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let observed = hits.clone();
    let url = spawn(Router::new().route("/index.json", get(move || {
        let observed = observed.clone();
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            Json(minimal_index())
        }
    }))).await;
    let home = use_env(&url);
    let cache = home.path().join("plugins/index-cache.json");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(&cache, b"deliberately not JSON").unwrap();
    let client = gray_pkg::fetch::client().unwrap();
    for _ in 0..2 {
        let catalog = gray_pkg::index::fetch_index_uncached(&client).await.unwrap();
        assert!(catalog.plugins.contains_key("demo"));
    }
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read(&cache).unwrap(), b"deliberately not JSON");
    assert!(!home.path().join("plugins/hub-index-cache.json").exists());
}
```

Add integration cases in this same file for HTTP 503, invalid JSON, an empty
`plugins` map, and a client with a 20-ms timeout against a pending Axum handler:

```rust
let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_millis(20)).build().unwrap();
let error = gray_pkg::index::fetch_index_uncached(&client).await.unwrap_err();
assert!(error.downcast_ref::<reqwest::Error>().unwrap().is_timeout());
```

Keep environment mutations under the existing guard and restore environment
variables when each new test finishes. Do not require public network access.

- [ ] **4. Run green and regression checks.**

Run `cargo test -p gray-pkg`.
Expected: all metadata, uncached, existing cached/ETag, and package tests pass.

- [ ] **5. Commit.**

```sh
git add crates/gray-pkg/src/index.rs crates/gray-pkg/src/index_tests.rs crates/gray-pkg/tests/index.rs
git diff --cached --check
git diff --cached
git commit -m "feat(pkg): add uncached marketplace catalog fetch"
```

### Task A2: owned, cancellable package preparation

**Files**

- Modify: `crates/gray-pkg/Cargo.toml`
- Modify: `Cargo.lock` for the `gray-pkg` dependency list
- Modify: `crates/gray-pkg/src/lib.rs`
- Modify: `crates/gray-pkg/src/fetch.rs:65–135`
- Create: `crates/gray-pkg/src/staged.rs`
- Create: `crates/gray-pkg/src/staged_tests.rs`

Use explicit home paths in new APIs, so tests do not depend on global
`GRAY_HOME`. The approved snapshot is never resolved by name again.

- [ ] **1. Define the new module's test fixture and red cancellation test.**

```rust
fn approved_demo() -> super::ApprovedEntry {
    super::ApprovedEntry::new("demo".into(), serde_json::from_value(
        serde_json::json!({
            "ecosystem": "gray-native", "kind": "plugin", "version": "1.0.0",
            "source": {"type": "tarball", "url": "https://example.invalid/demo.tgz"},
            "hash": format!("sha256:{}", "0".repeat(64))
        })
    ).unwrap()).unwrap()
}

#[tokio::test]
async fn cancelled_prepare_has_no_install_side_effects() {
    let home = tempfile::tempdir().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let result = super::prepare(
        home.path(), approved_demo(), &gray_pkg_client(), cancel, None
    ).await;
    assert!(result.is_err());
    assert!(!home.path().join("plugins/demo").exists());
    assert!(!home.path().join("plugins/lock.json").exists());
}

fn gray_pkg_client() -> reqwest::Client {
    crate::fetch::client().unwrap()
}
```

Wire `#[path = "staged_tests.rs"] #[cfg(test)] mod tests;` in the new module.
Run `cargo test -p gray-pkg --lib staged::tests`.
Expected: missing `ApprovedEntry`/`prepare`.

- [ ] **2. Add the API and identity validation.**

Register `pub mod staged;` in `lib.rs`; add
`tokio-util = { workspace = true }` to package dependencies. Keep fields private:

```rust
#[derive(Clone, Debug)]
pub struct ApprovedEntry {
    name: String,
    entry: crate::index::Entry,
}

#[derive(Debug)]
pub struct PreparedPackage {
    approved: ApprovedEntry,
    home: std::path::PathBuf,
    stage: tempfile::TempDir,
    root: std::path::PathBuf,
}

impl ApprovedEntry {
    pub fn new(name: String, entry: crate::index::Entry) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !name.is_empty() && name.bytes().all(|b|
                b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "invalid marketplace plugin name"
        );
        anyhow::ensure!(
            !matches!(name.as_str(), "tmp" | "pi"),
            "reserved plugin name"
        );
        anyhow::ensure!(
            entry.ecosystem == "gray-native" && entry.source.type_ == "tarball"
                && matches!(entry.kind.as_str(), "" | "plugin"),
            "unsupported marketplace package"
        );
        anyhow::ensure!(entry.hash.primary().is_some_and(|h| !h.is_empty()),
            "marketplace package requires a digest");
        Ok(Self { name, entry })
    }
    pub fn name(&self) -> &str { &self.name }
    pub fn entry(&self) -> &crate::index::Entry { &self.entry }
}

impl PreparedPackage {
    pub fn root(&self) -> &std::path::Path { &self.root }
    pub fn home(&self) -> &std::path::Path { &self.home }
    pub fn approved(&self) -> &ApprovedEntry { &self.approved }
}
```

Do not reject an archive manifest merely because its runtime name differs
from the catalog key.

- [ ] **3. Make archive ownership cancellation-safe.**

Extract the body of `fetch::download` into an internal
`download_owned(client, url, expected, tmp_dir) -> Result<tempfile::TempPath>`.
Use the passed directory, and return the owned `TempPath` instead of calling
`keep()`. Preserve the public CLI wrapper's exact signature:

```rust
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    expected: Option<&crate::index::HashSpec>,
) -> anyhow::Result<std::path::PathBuf> {
    let archive = download_owned(
        client, url, expected, &crate::plugins_dir().join("tmp")
    ).await?;
    Ok(archive.keep()?)
}
```

Implement preparation around ownership rather than detached cleanup:

```rust
pub async fn prepare(
    home: &std::path::Path,
    approved: ApprovedEntry,
    client: &reqwest::Client,
    cancel: tokio_util::sync::CancellationToken,
    progress: Option<tokio::sync::mpsc::UnboundedSender<InstallStage>>,
) -> anyhow::Result<PreparedPackage> {
    anyhow::ensure!(!cancel.is_cancelled(), "preparation cancelled");
    let plugins = home.join("plugins");
    std::fs::create_dir_all(&plugins)?;
    if let Some(tx) = &progress { let _ = tx.send(InstallStage::Downloading); }
    let archive = tokio::select! {
        biased;
        _ = cancel.cancelled() => anyhow::bail!("preparation cancelled"),
        result = crate::fetch::download_owned(
            client, &approved.entry.source.url, Some(&approved.entry.hash),
            &plugins.join("tmp")
        ) => result?,
    };
    let stage = tempfile::tempdir_in(&plugins)?;
    let home = home.to_path_buf();
    if let Some(tx) = &progress { let _ = tx.send(InstallStage::Unpacking); }
    let prepared = tokio::task::spawn_blocking(move || {
        let root = stage.path().join("next");
        std::fs::create_dir(&root)?;
        crate::fetch::unpack_tar_gz(&archive, &root)?;
        Ok::<_, anyhow::Error>(PreparedPackage { approved, home, stage, root })
    }).await??;
    anyhow::ensure!(!cancel.is_cancelled(), "preparation cancelled");
    Ok(prepared)
}
```

Define the shared progress enum in `staged.rs`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallStage { Downloading, Unpacking, Scanning, ReadingManifest, Committing }
```

Pass `None` for progress in package tests. Pre-download constraint enforcement
is added in A3, before this package API is connected to any user path.
Await the blocking unpack even when cancelled; its owned staging/archive
drop afterwards. The UI stays responsive because it is awaiting a worker.
Existing tar limits bound the staging-only operation.

- [ ] **4. Extend the same fixture with a real loopback tarball.**

Build an archive with an executable `plugin.sh`, mode `0o755`, using
`tar::Header::new_gnu`, `tar::Builder`, and
`flate2::write::GzEncoder`. Compute
`format!("sha256:{:x}", sha2::Sha256::digest(&bytes))`, serve those bytes
with an Axum `get` route, and replace the fixture's source/hash before
constructing `ApprovedEntry`.

Assert verified preparation produces `root()/plugin.sh`, leaves no installed
directory/lock, and removes staging when the prepared value drops. A corrupt
digest and an archive containing `../escape` must fail without creating
`home/escape`. Test cancellation during a pending download, not just a
pre-cancelled token. After each result, assert no staging/download files remain.

- [ ] **5. Run and commit.**

Run `cargo test -p gray-pkg`.
Expected: all prior download/archive tests and new staged tests pass.

```sh
git add Cargo.lock crates/gray-pkg/Cargo.toml crates/gray-pkg/src/lib.rs crates/gray-pkg/src/fetch.rs crates/gray-pkg/src/staged.rs crates/gray-pkg/src/staged_tests.rs
git diff --cached --check
git diff --cached
git commit -m "feat(pkg): prepare approved marketplace packages in staging"
```

### Task A3: guarded commit, constraints, and rollback

**Files**

- Modify: `crates/gray-pkg/src/ops.rs:100–178`
- Modify: `crates/gray-pkg/src/staged.rs`
- Modify: `crates/gray-pkg/src/staged_tests.rs`

- [ ] **1. Add a red dependency test.**

```rust
#[test]
fn absent_dependencies_fail_closed_by_name() {
    let home = tempfile::tempdir().unwrap();
    let mut entry = approved_demo().entry().clone();
    entry.requires_plugins = vec!["questions".into(), "ledger".into()];
    let approved = super::ApprovedEntry::new("demo".into(), entry).unwrap();
    let error = super::check_constraints(home.path(), &approved)
        .unwrap_err().to_string();
    assert!(error.contains("questions"));
    assert!(error.contains("ledger"));
    assert!(!home.path().join("plugins/lock.json").exists());
}
```

Run `cargo test -p gray-pkg --lib absent_dependencies_fail_closed_by_name`.
Expected: failure if constraints permit missing dependencies.

- [ ] **2. Extract explicit-directory lock I/O without changing CLI wrappers.**

Create `pub(crate) read_lock_in(plugins: &Path)` and
`pub(crate) write_lock_in(plugins: &Path, lock: &LockFile)` from the existing
read/write bodies. Existing `read_lock`/`write_lock` delegate to them with
`crate::plugins_dir()`. Preserve missing-file-as-empty and corrupt-file errors.

Implement the new path's constraints:

```rust
pub fn check_constraints(
    home: &std::path::Path, approved: &ApprovedEntry
) -> anyhow::Result<()> {
    let lock = crate::ops::read_lock_in(&home.join("plugins"))?
        .unwrap_or_default();
    let missing: Vec<_> = approved.entry.requires_plugins.iter()
        .filter(|name| !lock.plugins.contains_key(name.as_str())).collect();
    anyhow::ensure!(missing.is_empty(), "missing required plugins: {}",
        missing.into_iter().map(String::as_str).collect::<Vec<_>>().join(", "));
    let conflicts: Vec<_> = approved.entry.conflicts.iter()
        .filter(|name| lock.plugins.contains_key(name.as_str())).collect();
    anyhow::ensure!(
        conflicts.is_empty() || std::env::var_os("GRAY_PLUGIN_FORCE").is_some(),
        "conflicts with installed plugins: {}",
        conflicts.into_iter().map(String::as_str).collect::<Vec<_>>().join(", ")
    );
    Ok(())
}
```

This preserves the current explicit conflict override, but never uses that
override to auto-install dependencies or overwrite an installed target.
Disabled lock entries count present, as in existing dependency checks.
Insert `check_constraints(home, &approved)?` at the start of `prepare`, after
the initial cancellation check and before any directory creation or download.

- [ ] **3. Implement the single commit boundary.**

```rust
#[derive(Default)]
pub struct CommitMetadata {
    pub argv: Vec<String>,
    pub cli_argv: Option<Vec<String>>,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub enum CommitOutcome {
    Installed(crate::ops::Report),
    AlreadyInstalled(String),
}
```

Implement `PreparedPackage::commit(self, metadata: CommitMetadata) ->
anyhow::Result<CommitOutcome>` synchronously, called from `spawn_blocking`.
Acquire `hold_registry_lock_in`, and reject unsupported locking rather than
claiming a guarded commit succeeded without a guard. Re-read the lock and
constraints while holding it. Return `AlreadyInstalled` for an existing key.
Reject an unregistered existing destination rather than overwrite user files.

Construct the row from `self.approved`, never caller-supplied identity:

```rust
let mut row = crate::ops::LockEntry {
    ecosystem: self.approved.entry.ecosystem.clone(),
    version: self.approved.entry.version.clone(),
    hash: self.approved.entry.hash.primary().unwrap().to_owned(),
    source: self.approved.entry.source.url.clone(),
    scope: if self.approved.entry.scope.is_empty() {
        "user".into()
    } else { self.approved.entry.scope.clone() },
    argv: metadata.argv,
    cli_argv: metadata.cli_argv,
    extra: metadata.extra,
    adapter_version: env!("CARGO_PKG_VERSION").into(),
    installed_at: crate::now_secs().to_string(),
    ..Default::default()
};
row.enabled = true;
```

Require `extra["capabilities_hash"]` to be a string and
`extra["granted_capabilities"]` to be an array before commit. Rename the staged
root to the destination; write the complete registry atomically through
`write_lock_in`; if publication fails, rename the destination back to staging.
If rollback fails, keep the recovery directory and report its actual path.
Do not reacquire the guard through `record_install`. Return a `Report` with
the approved name/version/path and empty `deps`.

- [ ] **4. Add guarded-commit tests and fault injection.**

Expose an internal `commit_with` helper whose writer closure defaults to
`write_lock_in`; tests pass a writer returning
`anyhow::bail!("injected registry failure")`. The helper is not a public
alternate install API.

```rust
let metadata = super::CommitMetadata {
    extra: serde_json::from_value(serde_json::json!({
        "granted_capabilities": [],
        "capabilities_hash": "consented-empty"
    })).unwrap(),
    ..Default::default()
};
let result = prepared.commit_with(metadata, |_, _| {
    anyhow::bail!("injected registry failure")
});
assert!(result.is_err());
assert!(!home.path().join("plugins/demo").exists());
assert!(!home.path().join("plugins/lock.json").exists());
```

Use the real prepared fixture from A2. Add tests for concurrent target
installation, dependency disappearance and new conflicts between prepare and
commit, corrupt registry, lock contention, unregistered destination
preservation, metadata validation, and committed exact version/hash/source.
Assert no extra packages are installed. A successful committed row must
round-trip through both `gray-pkg`'s shape and later host lock loading.

- [ ] **5. Exit Phase A and commit.**

Run `cargo test -p gray-pkg`.
Expected: package tests all pass; cached CLI behavior remains unchanged.

```sh
git add crates/gray-pkg/src/ops.rs crates/gray-pkg/src/staged.rs crates/gray-pkg/src/staged_tests.rs
git diff --cached --check
git diff --cached
git commit -m "feat(pkg): commit staged plugins with consent and registry guards"
```

## Phase B: host preparation and consent

### Task B1: pure scan and capability policy

**Files**

- Modify: `crates/gray/src/lib.rs:24–26`
- Modify: `crates/gray/src/plugin_cli.rs:288–455`
- Create: `crates/gray/src/plugin_install/mod.rs`
- Create: `crates/gray/src/plugin_install/policy.rs`
- Create: `crates/gray/src/plugin_install/tests.rs`

- [ ] **1. Add red consent tests.**

```rust
#[test]
fn empty_consent_is_not_grandfathered() {
    let record = super::policy::grant(&[]);
    assert!(record.granted.is_empty());
    assert_eq!(record.hash, gray_plugin::capabilities::consent_hash(&[]));
}

#[test]
fn unknown_capabilities_are_not_granted() {
    let record = super::policy::grant(&[
        gray_plugin::HOST_ASK.to_string(), "invented.capability".into()
    ]);
    assert_eq!(record.granted, vec![gray_plugin::HOST_ASK.to_string()]);
}
```

Run `cargo test -p gray --lib plugin_install`.
Expected: missing module/policy APIs.

- [ ] **2. Add the data-only policy layer.**

Register `pub(crate) mod plugin_install;` in `lib.rs`. Define:

```rust
#[derive(Clone, Debug)]
pub struct ConsentRecord {
    pub granted: Vec<String>,
    pub hash: String,
}

pub fn grant(declared: &[String]) -> ConsentRecord {
    let known = gray_plugin::capabilities::parse_declared(declared);
    ConsentRecord {
        hash: gray_plugin::capabilities::consent_hash(&known),
        granted: known,
    }
}

pub fn require_installable_scan(
    report: &gray_plugin::scan::Report
) -> anyhow::Result<()> {
    anyhow::ensure!(
        report.verdict() != gray_plugin::scan::Verdict::Dangerous,
        "plugin scan blocked installation: {}", report.summary()
    );
    Ok(())
}
```

Define the display record and `capability_lines(&[String]) -> Vec<CapabilityLine>`:

```rust
#[derive(Clone, Debug)]
pub struct CapabilityLine {
    pub id: String,
    pub description: String,
    pub grantable: bool,
}
```

Keep unknown raw IDs
visible with `grantable = false`; known IDs use the canonical registry's risk
description. Consent hashing uses the normalized known declaration, matching
the runtime's capability parser. Do not expose a grant-all-unknown path.

Extract `require_installable_scan` and line generation into the existing CLI
adapters, retaining their stdout/TrustGate choices, current scan verdict
messages, and existing empty-declaration CLI behavior. Only the new
Marketplace path always records the empty consent hash.

- [ ] **3. Add verdict and CLI parity tests.**

Construct `Report { findings, files, skipped }` using `Finding` and `Severity` from
`gray-plugin/src/scan.rs`; assert Safe and Caution succeed and Critical fails.
Assert Caution findings remain in data for rendering. Run the existing CLI
scan/capability tests, not a new implementation of their prompting.

- [ ] **4. Run and commit.**

Run `cargo test -p gray --lib plugin_install` and
`cargo test -p gray --lib plugin_cli`.
Expected: both suites pass without output/prompt regressions.

```sh
git add crates/gray/src/lib.rs crates/gray/src/plugin_cli.rs crates/gray/src/plugin_install
git diff --cached --check
git diff --cached
git commit -m "refactor(plugin): separate scan and consent policy from terminal IO"
```

### Task B2: one prepared manifest and cancellation-safe probing

**Files**

- Modify: `crates/gray/src/plugin_install/mod.rs`
- Create: `crates/gray/src/plugin_install/probe.rs`
- Modify: `crates/gray/src/plugin_install/tests.rs`
- Modify: `crates/gray/src/plugin_cli.rs:875–883,1354–1395`

- [ ] **1. Add a real, bounded fixture probe test.**

For Unix tests, write this executable in a `tempdir` with mode `0o755`:

```sh
#!/bin/sh
printf x >> "$0.count"
printf '%s\n' '{"name":"runtime-demo","version":"1.0.0","protocol":"1.1","capabilities":["host.ask"],"commands":["/demo"]}'
```

Call `probe::manifest(&executable, CancellationToken::new())` and assert
`raw["name"] == "runtime-demo"` and the counter contains exactly `x`.
A separate fixture sleeps until cancelled; the test records its PID and
asserts the child has exited before the probe future returns. Guard Unix
shell/PID assertions with `#[cfg(unix)]`; pure policy and fake backend tests
remain portable.

Run `cargo test -p gray --lib plugin_install::tests`.
Expected: missing `probe`/`manifest`.

- [ ] **2. Implement the prepared representation and bounded CLI probe.**

```rust
#[derive(Clone, Debug)]
pub struct PreparedManifest {
    pub raw: serde_json::Value,
    pub declared: Vec<String>,
    pub commands: Vec<String>,
    pub executable_relative: std::path::PathBuf,
    pub cli: bool,
}
```

Implement `probe::manifest(executable: &Path, cancel: CancellationToken) ->
Result<Value>` with `<executable> manifest`, a 10-second deadline, stdout
limited to 65,536 bytes, stdin null, and stderr null. In `tokio::select!`,
select cancellation, timeout, and capture result; on cancellation/error
explicitly `child.kill().await` then `child.wait().await` before returning.
Factor existing `capture`'s bounded reader into a helper rather than duplicate
its error parsing. Existing CLI callers pass a never-cancelled token and retain
their current behavior.

After a valid manifest, derive a relative executable path by stripping the
staging root from `resolve_argv(root)`'s first path. Never persist staging
absolute paths in the lock.

Preserve wire-only compatibility with the existing CLI-first/wire-fallback
policy. The fallback must be bounded and shut down/reap its sidecar, including
on cancellation. Store the first successfully obtained manifest; adoption
never repeats either transport. Here “once” means one preparation attempt and
one successful manifest result, not re-running a successful probe at commit.
Do not turn a cancelled CLI probe into a wire fallback.

- [ ] **3. Add preparation orchestration.**

Define this record in `plugin_install/mod.rs`:

```rust
pub(crate) struct PreparedInstall {
    pub(crate) package: gray_pkg::staged::PreparedPackage,
    pub(crate) manifest: probe::PreparedManifest,
    pub(crate) scan: gray_plugin::scan::Report,
}
```

Expose `policy` and `probe` as `pub(crate)` submodules, not public external API.
Add `prepare(home, approved, cancel, progress) -> Result<PreparedInstall>`,
where `progress` has the same optional sender type introduced in A2:

1. Call Phase A's `staged::prepare` with the supplied progress sender.
2. `spawn_blocking` the scan, await completion even when cancelled, and run
   `require_installable_scan` before any executable probe.
3. Check cancellation, resolve executable, probe, validate manifest name/type,
   and retain raw capabilities/commands.

Emit `InstallStage::Scanning` before scanning and
`InstallStage::ReadingManifest` before probing; no stdout output is involved.

Never call `register_native` on the stage: it writes registry metadata.
Never require the runtime manifest name to equal the catalog key.

Add a fixture with a Critical scan finding and a probe counter; assert the
counter does not exist. Add CLI probe timeout, oversized output, invalid JSON,
wire fallback, and cancellation tests. All pre-commit errors drop staging.

- [ ] **4. Run and commit.**

Run `cargo test -p gray --lib plugin_install` and
`cargo test -p gray --lib plugin_cli`.
Expected: policy, bounded capture, CLI and wire compatibility tests pass.

```sh
git add crates/gray/src/plugin_install crates/gray/src/plugin_cli.rs
git diff --cached --check
git diff --cached
git commit -m "feat(plugin): probe staged manifests before capability consent"
```

### Task B3: consented commit and adoption without re-probing

**Files**

- Modify: `crates/gray/src/plugin_install/mod.rs`
- Modify: `crates/gray/src/plugin_install/tests.rs`
- Modify: `crates/gray/src/plugin_cli.rs:1034–1091`

- [ ] **1. Add a regression that commits the prepared fixture.**

After B2 prepares the counter fixture, store `let consent =
policy::grant(&declared);`, then call
`plugin_install::commit(prepared, consent.clone()).await`. Load
`gray_plugin::lock::LockFile` from the test home. Assert:

```rust
let row = &lock.plugins["demo"];
assert_eq!(row.capabilities_hash.as_deref(), Some(consent.hash.as_str()));
assert_eq!(row.granted_capabilities, consent.granted);
assert_eq!(std::path::Path::new(&row.argv[0]),
    home.path().join("plugins/demo/plugin.sh"));
assert_eq!(std::fs::read_to_string(
    home.path().join("plugins/demo/plugin.sh.count")).unwrap(), "x");
```

Run `cargo test -p gray --lib plugin_install`.
Expected: missing commit API, then failing identity/consent/adoption assertions.

- [ ] **2. Construct complete metadata before Phase A's commit.**

Get `home` from `prepared.package.home()` before consuming the package.
Use the prepared relative executable joined to `home/plugins/<catalog-key>`.
Populate argv plus `sidecar_args`; populate `cli_argv` only for a CLI-speaking
probe. Put these host-owned fields in `CommitMetadata.extra`:

```rust
let extra = serde_json::from_value(serde_json::json!({
    "granted_capabilities": consent.granted,
    "capabilities_hash": consent.hash,
    "runtime_role": runtime_role
}))?;
```

Reuse the existing `provider_only_role_json` policy; extract visibility rather than
redefine protocol checks. Recompute the expected consent hash/grants from the
prepared declaration and reject a mismatched supplied record before commit.

Define:

```rust
#[derive(Debug)]
pub struct InstallResult {
    pub name: String,
    pub installed: bool,
    pub warnings: Vec<String>,
}
```

Run the blocking package commit to completion without a cancellation token.
Map `AlreadyInstalled` to `installed = false` with explanatory status, not a
new install eligible for activation.

- [ ] **3. Extract prepared adoption and retain the CLI wrapper.**

Add `plugin_cli::adopt_prepared(home, name, executable, manifest, cli) ->
Result<()>`. It writes cached manifest metadata atomically and refreshes
provider discovery without executing plugin code or changing granted
capabilities. For the Marketplace, argv/runtime metadata was already committed;
do not write a second consentless lock entry. The existing async `adopt_cli`
still probes for legacy callers, then delegates shared metadata publication.

After package commit, catch adoption errors into `InstallResult.warnings`.
Do not remove installed files or return a cancellation/failure result for a
post-commit adoption problem. Persist manifest metadata for wire-only packages
as well, so capability reporting/provider discovery can inspect it.

- [ ] **4. Test post-commit failure and identity.**

Inject an adoption closure returning `anyhow::bail!("provider refresh failed")`
in an internal commit helper. Assert `installed` remains true, warning text is
present, package files exist, and consent fields remain in the lock. Test empty
capabilities produce `Some(consent_hash([]))`. Set `GRAY_PLUGIN_PATH` and put a
same-named executable on PATH under the existing environment-test guard;
assert neither executes. Confirm catalog version/hash/source remain exact
even when a fake catalog response changes during consent.

- [ ] **5. Exit Phase B and commit.**

Run `cargo test -p gray-pkg`, `cargo test -p gray --lib plugin_install`, and
`cargo test -p gray --lib plugin_cli`.
Expected: all pass; no Marketplace worker API prints prompts.

```sh
git add crates/gray/src/plugin_install crates/gray/src/plugin_cli.rs
git diff --cached --check
git diff --cached
git commit -m "feat(plugin): commit consent and reuse prepared manifest adoption"
```

## Phase C: plugin-only Marketplace interaction

### Task C1: pure model, search, scrolling, and installed identity

**Files**

- Create: `crates/gray/src/setup/marketplace/mod.rs`
- Create: `crates/gray/src/setup/marketplace/model.rs`
- Create: `crates/gray/src/setup/marketplace/tests.rs`
- Modify: `crates/gray/src/setup/mod.rs:104–115`

- [ ] **1. Add red search/focus and identity tests.**

Register `pub(crate) mod marketplace;` beside `install_manager`; declare `mod model;`
and the test module in `marketplace/mod.rs`. Define this local fixture:

```rust
fn index(names: &[&str]) -> gray_pkg::index::Index {
    let plugins = names.iter().map(|name| (name.to_string(), serde_json::json!({
        "ecosystem": "gray-native", "version": "1.0.0",
        "source": {"type": "tarball", "url": "https://example.invalid/demo.tgz"},
        "hash": format!("sha256:{}", "0".repeat(64)),
        "description": format!("{name} helper")
    }))).collect::<serde_json::Map<_, _>>();
    serde_json::from_value(serde_json::json!({"plugins": plugins})).unwrap()
}

#[test]
fn search_owns_j_and_k_and_installed_uses_catalog_key() {
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["jack", "ledger"]), ["jack".to_string()].into());
    model.search_focused = true;
    model.input(crossterm::event::KeyCode::Char('j'));
    assert_eq!(model.query, "j");
    assert_eq!(model.visible_names(), vec!["jack"]);
    assert!(!model.can_install());
}

#[test]
fn scrolling_keeps_last_selection_visible() {
    let mut model = super::model::MarketModel::default();
    model.loaded(index(&["a", "b", "c", "d", "e"]), Default::default());
    for _ in 0..4 { model.move_selection(1, 2); }
    assert_eq!(model.selected, 4);
    assert_eq!(model.offset, 3);
}
```

Run `cargo test -p gray --lib marketplace`.
Expected: missing `MarketModel`/model methods.

- [ ] **2. Implement the model contract.**

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarketPhase {
    #[default]
    Loading,
    LoadError,
    Browsing,
    Confirm,
    Preparing,
    Consent,
    Committing,
    Installed,
    InstallFailed,
}

#[derive(Default)]
pub struct MarketModel {
    pub phase: MarketPhase,
    pub entries: std::collections::BTreeMap<String, gray_pkg::index::Entry>,
    pub installed: std::collections::BTreeSet<String>,
    pub query: String,
    pub search_focused: bool,
    pub selected: usize,
    pub offset: usize,
    pub details: bool,
    pub operation: u64,
    pub message: Option<String>,
    pub close_requested: bool,
}
```

Implement `loaded`, `visible_names`, `selected_name`, `can_install`, `input`,
`move_selection`, `begin`, and `accept_operation`:

```rust
pub fn move_selection(&mut self, delta: isize, rows: usize) {
    let len = self.visible_names().len();
    self.selected = self.selected.saturating_add_signed(delta)
        .min(len.saturating_sub(1));
    let rows = rows.max(1);
    self.offset = self.offset.min(self.selected);
    if self.selected >= self.offset.saturating_add(rows) {
        self.offset = self.selected + 1 - rows;
    }
}

pub fn begin(&mut self, phase: MarketPhase) -> u64 {
    self.operation = self.operation.wrapping_add(1);
    self.phase = phase;
    self.message = None;
    self.operation
}

pub fn accept_operation(&self, operation: u64) -> bool {
    self.operation == operation
}
```

`visible_names` filters lowercased name or optional description against the
lowercased query, retaining BTreeMap order. Query edits reset selection and
offset. `can_install` requires a selected key absent from `installed`, a
supported `ApprovedEntry`, and Browsing/details. All keys in the lock count,
not just enabled or CLI rows. A corrupt registry becomes an error.

Use `/` to focus search, Esc to leave search before leaving details/tab,
Enter to open details, `i` or focused Install to enter Confirm, Tab to cycle
outer tabs only outside an active confirmation/operation. In Confirm, Enter
accepts and Esc returns; in Consent, Grant is explicit and Esc cancels.
Never let an Enter held from opening details automatically grant consent.

- [ ] **3. Add state transition coverage.**

```rust
#[test]
fn stale_completion_cannot_advance_a_new_operation() {
    let mut model = super::model::MarketModel::default();
    let old = model.begin(super::model::MarketPhase::Preparing);
    let current = model.begin(super::model::MarketPhase::Loading);
    assert!(!model.accept_operation(old));
    assert!(model.accept_operation(current));
}
```

Test case-insensitive description search, no matches versus empty catalog,
disabled/older/local installed rows, long names, zero visible rows, and
Committing rejecting cancel transitions. Preserve explicit details state
inside Browsing rather than introduce an inconsistent tenth install phase.

- [ ] **4. Run and commit.**

Run `cargo test -p gray --lib marketplace`.
Expected: model tests pass without a real TTY/network/model.

```sh
git add crates/gray/src/setup/mod.rs crates/gray/src/setup/marketplace
git diff --cached --check
git diff --cached
git commit -m "feat(tui): model marketplace search and installation states"
```

### Task C2: dedicated rendering and deterministic snapshots

**Files**

- Create: `crates/gray/src/setup/marketplace/draw.rs`
- Modify: `crates/gray/src/setup/marketplace/mod.rs`
- Modify: `crates/gray/src/setup/marketplace/tests.rs`

- [ ] **1. Add a failing Ratatui rendering test.**

Define `draw::render(frame, area, &MarketModel, Option<&ConsentView>)`.
`ConsentView` is cloneable display data in `marketplace/mod.rs`:
`capabilities: Vec<CapabilityLine>`, `commands: Vec<String>`,
`scan_text: String`. `CapabilityLine` comes from B1 and must derive Clone.

```rust
fn snapshot(model: &super::model::MarketModel, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| {
        super::draw::render(frame, frame.area(), model, None);
    }).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height).map(|y| {
        (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>()
    }).collect::<Vec<_>>().join("\n")
}

#[test]
fn loading_snapshot_has_no_install_action() {
    let screen = snapshot(&super::model::MarketModel::default(), 80, 20);
    assert!(screen.contains("Loading marketplace"));
    assert!(!screen.contains("[ Install ]"));
}
```

Run `cargo test -p gray --lib loading_snapshot_has_no_install_action`.
Expected: missing render module/API.

- [ ] **2. Implement the state-to-view projection.**

Use theme colors and Ratatui `List`/`ListState` for the scrollable catalog.
List state uses the model's selected index and offset. Render within the
supplied content rectangle; outer manager retains title/tabs/footer.
Use `Paragraph::wrap` for details and consent with vertical scrolling when
needed. All dimensions use saturating arithmetic; a tiny area displays a
short hint, never a panic.

Define the literal empty/error texts in one projection:

```rust
pub fn status_text(model: &MarketModel) -> Option<&str> {
    match model.phase {
        MarketPhase::Loading => Some("Loading marketplace…"),
        MarketPhase::LoadError => model.message.as_deref(),
        MarketPhase::Browsing if model.entries.is_empty() =>
            Some("No plugins available"),
        MarketPhase::Browsing if model.visible_names().is_empty() =>
            Some("No matching plugins"),
        _ => None,
    }
}
```

Confirm view shows approved source URL, version, digest and trust plus
`Preparation runs this plugin's manifest command as your user before capability consent.`
Consent view shows risk descriptions, unknown IDs marked not grantable,
commands, and all scan warnings. Both use explicit accept/cancel affordances.
Installed rows append `[installed]`, with Install visibly disabled in details.
Post-commit warning text is under installed success, not an error heading.

Sanitize control characters from catalog/manifest/error strings before
rendering and use `fetch::redact` for source URLs/error display. Do not print
userinfo, query tokens, terminal escapes, or plugin stdout directly.

- [ ] **3. Add complete render snapshots.**

Use the `snapshot` helper to assert exact buffer strings for stable small
fixtures, not screenshots from the user's terminal. Cover Loading, LoadError
with Retry/Back, empty, no matches, populated/installed, details, Confirm,
Preparing stage, Consent, Committing, Installed-with-warning and InstallFailed.
At 20x6 and 80x20 render every state without panic. Confirm a 150-row fixture
shows the selected final row after scrolling and that descriptions/URLs wrap.
Add an ANSI/control-character fixture and assert no raw escape remains.

- [ ] **4. Run and commit.**

Run `cargo test -p gray --lib marketplace`.
Expected: all model and buffer-snapshot tests pass.

```sh
git add crates/gray/src/setup/marketplace
git diff --cached --check
git diff --cached
git commit -m "feat(tui): render marketplace browsing and consent screens"
```

### Task C3: background worker with owned cancellation and fake backend

**Files**

- Create: `crates/gray/src/setup/marketplace/worker.rs`
- Modify: `crates/gray/src/setup/marketplace/mod.rs`
- Modify: `crates/gray/src/setup/marketplace/tests.rs`

- [ ] **1. Establish the backend boundary and red lifecycle test.**

Use an async trait with associated prepared type so fake tests need not
construct real package directories or execute binaries:

```rust
#[async_trait::async_trait]
pub trait MarketBackend: Send + Sync + 'static {
    type Prepared: Send + 'static;
    async fn load(
        &self, cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<(gray_pkg::index::Index, std::collections::BTreeSet<String>)>;
    async fn prepare(
        &self, approved: gray_pkg::staged::ApprovedEntry,
        cancel: tokio_util::sync::CancellationToken,
        progress: tokio::sync::mpsc::UnboundedSender<gray_pkg::staged::InstallStage>,
    ) -> anyhow::Result<(Self::Prepared, super::ConsentView)>;
    async fn commit(&self, prepared: Self::Prepared)
        -> anyhow::Result<crate::plugin_install::InstallResult>;
}
```

The production backend stores explicit `home: PathBuf`, uses uncached fetching
and read-only lock loading, B2 prepare, and B3 commit with the prepared consent.
It never calls name-based CLI installation. The worker owns the prepared
object while consent is pending; Cancel drops it.

Define the channel payloads:

```rust
pub enum MarketCommand {
    Load,
    Prepare(gray_pkg::staged::ApprovedEntry),
    Grant,
    Cancel,
    Close,
}
pub struct TaggedCommand {
    pub operation: u64,
    pub command: MarketCommand,
}
pub enum MarketEvent {
    Loaded(gray_pkg::index::Index, std::collections::BTreeSet<String>),
    Stage(gray_pkg::staged::InstallStage),
    Ready(super::ConsentView),
    Committed(crate::plugin_install::InstallResult),
    Failed(String),
    Cancelled,
    Closed,
}
pub struct TaggedEvent {
    pub operation: u64,
    pub event: MarketEvent,
}
```

`Ready` contains only display data; prepared package ownership does not leak
into the UI model.

Add this backend in `marketplace/tests.rs`:

```rust
#[derive(Default)]
struct FakeBackend {
    commits: std::sync::atomic::AtomicUsize,
    prepare_started: tokio_util::sync::CancellationToken,
    prepare_cancelled: tokio_util::sync::CancellationToken,
}

#[async_trait::async_trait]
impl super::worker::MarketBackend for FakeBackend {
    type Prepared = ();

    async fn load(
        &self, _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<(gray_pkg::index::Index, std::collections::BTreeSet<String>)> {
        Ok((index(&["demo"]), Default::default()))
    }

    async fn prepare(
        &self, _: gray_pkg::staged::ApprovedEntry,
        cancel: tokio_util::sync::CancellationToken,
        _: tokio::sync::mpsc::UnboundedSender<gray_pkg::staged::InstallStage>,
    ) -> anyhow::Result<((), super::ConsentView)> {
        self.prepare_started.cancel();
        cancel.cancelled().await;
        self.prepare_cancelled.cancel();
        anyhow::bail!("preparation cancelled")
    }

    async fn commit(&self, _: ()) -> anyhow::Result<crate::plugin_install::InstallResult> {
        self.commits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(crate::plugin_install::InstallResult {
            name: "demo".into(), installed: true, warnings: Vec::new()
        })
    }
}
```

Start prepare, await `fake.prepare_started.cancelled()`, send Close, drain
events through Closed, then:

```rust
assert_eq!(fake.commits.load(std::sync::atomic::Ordering::SeqCst), 0);
assert!(fake.prepare_cancelled.is_cancelled());
worker.task.await.unwrap();
```

Run `cargo test -p gray --lib marketplace`.
Expected: missing worker/runner APIs.

- [ ] **2. Implement owned worker handles.**

```rust
pub struct WorkerHandle {
    pub commands: tokio::sync::mpsc::UnboundedSender<TaggedCommand>,
    pub events: tokio::sync::mpsc::UnboundedReceiver<TaggedEvent>,
    pub task: tokio::task::JoinHandle<()>,
}
```

`spawn_backend(Arc<B>) -> WorkerHandle` runs a command loop and retains every
operation JoinHandle. Load/prepare have cancellation tokens; Grant consumes
the current prepared value once and enters a commit state that ignores Cancel.
Close during commit marks close pending, awaits commit and emits its result,
then emits Closed. Close during prepare cancels, awaits child/staging cleanup,
then emits Cancelled and Closed. No dropped JoinHandle may silently detach
an installation. Channel closure is treated as Close.
Implement `spawn_backend` as a generic function over `B: MarketBackend`.
The runner stores `Option<B::Prepared>` locally between Ready and Grant; no
trait-object associated-type erasure or cloning of staged packages is needed.

After commit, load installed names directly from the registry without a
catalog request. Return registry-refresh failure as an installed warning when
files committed; never accidentally re-enable Install because a refresh failed.
Reject Grant for stale IDs or when no prepared value exists.

Stage strings are `Downloading`, `Unpacking`, `Scanning`, `Reading manifest`,
and `Committing`. Pass the A2/B2 progress sender through the production backend
and relay stage events with the operation ID; do not infer fake percentages.

- [ ] **3. Drive deterministic race tests.**

Add fake backend tests for each terminal path: fresh Load on each command;
load/prepare failure; Cancel during download; Close during scan/probe;
Grant exactly once; Close/Cancel during commit waits; late Ready from an old
ID is dropped; backend panic/channel closure becomes a visible failure and
unblocks closing; post-commit adoption failure remains installed success.
Use `oneshot` barriers, not arbitrary sleeps, for commit-boundary races.

The sync modal polls receiver/task status every 100 ms. Closing drains until
Closed and joins a completed worker through the current Tokio handle.
Do not call `block_on` on an unfinished task from a Tokio worker thread.

- [ ] **4. Run and commit.**

Run `cargo test -p gray --lib marketplace` and
`cargo test -p gray --lib plugin_install`.
Expected: no fake worker continues committing after its close completes.

```sh
git add crates/gray/src/setup/marketplace
git diff --cached --check
git diff --cached
git commit -m "feat(tui): run cancellable marketplace installation workers"
```

### Task C4: integrate only the plugin picker, preserving existing panels

**Files**

- Modify: `crates/gray/src/setup/install_manager.rs:199–231,234` onward
- Modify: `crates/gray/src/setup/install_manager_tests.rs`
- Modify: `crates/gray/src/setup/mod.rs:124`
- Modify: `crates/gray/src/setup/marketplace/mod.rs`
- Modify: `crates/gray/src/repl/plugin_cmds.rs:53–86`

- [ ] **1. Add red plugin-only tab tests.**

Add a pure helper `manager_tab_names(has_errors, has_extension)`:

```rust
#[test]
fn only_plugin_wrapper_adds_marketplace() {
    assert_eq!(super::manager_tab_names(true, false), vec!["Installed", "Errors"]);
    assert_eq!(super::manager_tab_names(true, true),
        vec!["Installed", "Errors", "Marketplace"]);
    assert_eq!(super::manager_tab_names(false, false), vec!["Installed"]);
}
```

Run `cargo test -p gray --lib install_manager`.
Expected: missing helper.

- [ ] **2. Add a narrow optional extension to the shared modal loop.**

Keep `tabs::Tab` and existing `run_install_manager` caller signatures intact.
Move the existing loop body behind
`run_install_manager_extended(..., Option<&mut dyn ManagerExtension>)`;
the original wrapper calls it with `None`. Only `run_plugins_modal` supplies
the Marketplace extension. Existing panel specs and callbacks remain unchanged.

Define the extension in `install_manager.rs`:

```rust
pub(crate) trait ManagerExtension {
    fn enter(&mut self);
    fn tick(&mut self);
    fn draw(&mut self, frame: &mut ratatui::Frame<'_>,
        area: ratatui::layout::Rect);
    fn key(&mut self, key: crossterm::event::KeyEvent) -> bool;
    fn request_close(&mut self) -> bool;
}
```

`enter` begins a fresh Load each time the tab gains focus. `tick` drains worker
events and updates the model; `draw` renders the Marketplace content; `key`
returns true when it consumed input, especially search `j/k`; `request_close`
returns false while cleanup/commit must finish. The shared loop uses a local
numeric active-tab index and dynamic tab labels, not an always-visible third
variant. Existing Installed/Errors rendering, toggles, removal confirmation,
error clearing, navigation, and relisting take the same original branches.

Outer tab changes are blocked during Confirm/Preparing/Consent/Committing
unless the extension first cancels and cleans up. Esc may initiate close, but
the loop continues drawing until `request_close` returns true.

Change the plugin-specific return type:

```rust
#[derive(Default)]
pub struct PluginModalOutcome {
    pub changed: bool,
    pub installed: Vec<String>,
    pub warnings: Vec<String>,
}
```

Return actual committed names, not generic `changed`, for later activation.
Update `plugin_cmds` to preserve existing feedback while consuming this type.
Explicit `/plugin list|install|remove|update|enable|disable|check` behavior and
headless commands remain unchanged.

- [ ] **3. Add integration regressions and manual check.**

Test extensions absent for skills/gateway/cron/memory; existing manager specs
and Enter/Space/two-press removal tests still pass. Verify bare `/plugin` and
`/plugins` open the picker, explicit commands keep text behavior, and no
Marketplace operation invokes a model provider.

Run:

```sh
cargo test -p gray --lib marketplace
cargo test -p gray --lib install_manager
cargo test -p gray --lib plugin_cmds
cargo test -p gray --lib tabs
```

Expected: all pass. In a disposable `GRAY_HOME`, open a real TTY:

```sh
GRAY_HOME="$(mktemp -d)" cargo run -p gray
```

Type `/plugin`, switch to Marketplace, search, open details, decline
confirmation, and close. Do not install public third-party code for the smoke
test. Confirm the original composer viewport returns correctly. Record the
manual check as skipped if the execution environment lacks a TTY.

- [ ] **4. Exit Phase C and commit.**

```sh
git add crates/gray/src/setup/install_manager.rs crates/gray/src/setup/install_manager_tests.rs crates/gray/src/setup/mod.rs crates/gray/src/setup/marketplace crates/gray/src/repl/plugin_cmds.rs
git diff --cached --check
git diff --cached
git commit -m "feat(tui): add marketplace tab to the plugin picker"
```

## Phase D: activation and final validation

### Task D1: prompt after close and preserve the running session

**Files**

- Create: `crates/gray/src/setup/marketplace/activation.rs`
- Modify: `crates/gray/src/setup/marketplace/mod.rs`
- Modify: `crates/gray/src/repl/plugin_cmds.rs`
- Modify: `crates/gray/src/repl/plugin_cmds_tests.rs`
- Modify: `crates/gray/src/repl/dispatch.rs`
- Modify: `crates/gray/src/repl/handlers.rs:489–510`
- Modify: `crates/gray/src/repl/handlers_tests.rs`

- [ ] **1. Add a red activation-decision test.**

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivationChoice { Now, Later }

pub(crate) fn should_reload(
    installed: &[String], choice: ActivationChoice, agent_exists: bool
) -> bool {
    !installed.is_empty() && choice == ActivationChoice::Now && agent_exists
}

#[test]
fn activation_requires_new_installs_and_now() {
    use super::ActivationChoice::{Later, Now};
    assert!(!super::should_reload(&[], Now, true));
    assert!(!super::should_reload(&["demo".into()], Later, true));
    assert!(!super::should_reload(&["demo".into()], Now, false));
    assert!(super::should_reload(&["demo".into()], Now, true));
}
```

Put the enum in `setup/marketplace/activation.rs`, make that module
crate-visible, and import the enum into `plugin_cmds.rs` with
`pub(crate) use crate::setup::marketplace::activation::ActivationChoice;`.
Put the function in the REPL plugin module and test in `plugin_cmds_tests.rs`;
write the test before the function. Run
`cargo test -p gray --lib activation_requires_new_installs_and_now`.
Expected: missing decision API, then pass after adding the function.

- [ ] **2. Implement the post-modal activation prompt.**

`activation::prompt(bg, installed) -> Result<ActivationChoice>` is a small
modal using the existing terminal-session/background helpers. Render
`Activate now?`, a count/name summary, and Now/Later options. Esc chooses Later.
Open it only after the plugin picker has restored/closed its own terminal
session, once per modal visit with at least one actual committed package.
Declare `pub(crate) mod activation;` inside the crate-visible Marketplace
module so the REPL can call it without exposing a public external API.

Change `handle_plugin_command` to return `PluginCommandOutcome`:

```rust
pub(crate) fn choice_for_key(
    key: crossterm::event::KeyCode, selected: ActivationChoice,
) -> Option<ActivationChoice> {
    match key {
        crossterm::event::KeyCode::Esc => Some(ActivationChoice::Later),
        crossterm::event::KeyCode::Enter => Some(selected),
        _ => None,
    }
}

#[derive(Default)]
pub(crate) struct PluginCommandOutcome {
    pub installed: Vec<String>,
    pub activate_now: bool,
}
```

Use `ActivationChoice` defined once in `activation.rs` in Step 1.
Put `choice_for_key` in `activation.rs` and call it from
the prompt input loop after updating the selected option for navigation keys.
All existing text-only branches return the default. The bare picker branch
returns newly installed names and the explicit choice. `dispatch.rs` owns
agent/config/session and calls `reload_agent` when `should_reload` is true,
passing `session_state.as_ref().map(|s| s.session_id.as_str())`.
Do not build a new agent solely to activate when none exists; normal lazy
construction will read the committed registry.

- [ ] **3. Surface activation warnings without undoing installation.**

Reuse the builder-warning drain already used after agent construction. Extend
`reload_agent` to return a small result describing rebuild success,
without changing existing callers' observable behavior when they ignore it.
Keep the old agent until a replacement build succeeds; preserve messages
and the passed session ID. Plugin startup failures reported as builder
warnings mean installed-but-not-active, not install rollback.

Add the reload-result type in `handlers.rs` and return it from every branch:

```rust
pub(crate) struct ReloadOutcome {
    pub rebuilt: bool,
}
```

When the rebuild fails, return `rebuilt = false` after restoring the old
agent; the existing `say` failure remains. After successful history restoration,
return `rebuilt = true`. Existing callers ignore the new result and keep their
deferred warning behavior. Only the activation dispatcher explicitly drains
`crate::take_profile_warnings()` immediately after this call and renders the
warnings with `say`; the normal composer will then have no duplicates to drain.
Keep the warning queue untouched when Later is selected.

In the activation branch, use the result rather than treating every rebuild
as a success:

```rust
let outcome = reload_agent(
    &mut agent, &config, &cwd,
    session_state.as_ref().map(|s| s.session_id.as_str()),
    tui.as_ref(),
).await;
if !outcome.rebuilt {
    say(tui.as_ref(), "Plugins remain installed; activation failed.");
}
for warning in crate::take_profile_warnings() {
    say(tui.as_ref(), &format!("Plugin activation warning: {warning}"));
}
```

Use the existing dispatch bindings' actual borrow forms for `agent`, `config`,
`cwd`, and `tui`; do not introduce a second agent/session.

- [ ] **4. Add activation and history regressions.**

Extend `handlers_tests.rs`'s existing
`reload_agent_failure_preserves_agent` test: original messages survive and
the failed plugin's installed files remain. Use a fake plugin fixture whose
startup fails to assert a warning after an otherwise successful rebuild.
Test Later never reloads, Now reloads once for multiple installed names,
concurrent AlreadyInstalled never prompts, and no-agent activation remains
lazy. Capture the session ID passed to the rebuild path and assert it matches
the existing session rather than a new shard.

Add a Ratatui buffer test for the Now/Later prompt and an Esc-to-Later test.

```rust
#[test]
fn activation_escape_always_defers() {
    use super::ActivationChoice::{Later, Now};
    assert_eq!(super::choice_for_key(crossterm::event::KeyCode::Esc, Now),
        Some(Later));
    assert_eq!(super::choice_for_key(crossterm::event::KeyCode::Enter, Now),
        Some(Now));
}
```

Place this unit test in `activation.rs`'s test module. In the existing
failure-preservation test, also capture the returned result and assert
`!outcome.rebuilt`; its current empty-agent fixture alone is insufficient for
the history-preservation regression, which must start with a real test agent.

- [ ] **5. Run and commit.**

```sh
cargo test -p gray --lib activation
cargo test -p gray --lib reload_agent
cargo test -p gray --lib plugin_cmds
git add crates/gray/src/setup/marketplace crates/gray/src/repl/plugin_cmds.rs crates/gray/src/repl/plugin_cmds_tests.rs crates/gray/src/repl/dispatch.rs crates/gray/src/repl/handlers.rs crates/gray/src/repl/handlers_tests.rs
git diff --cached --check
git diff --cached
git commit -m "feat(tui): offer plugin activation without restarting the session"
```

Expected: all focused tests pass; the commit contains only activation work.

### Task D2: full failure-path regression and workspace CI

**Files**

- Modify only failing implementation/tests within the earlier phases' owned
  files. Do not change unrelated provider/plugin behavior to hide CI failures.

- [ ] **1. Run the complete focused suites.**

```sh
cargo test -p gray-pkg
cargo test -p gray --lib plugin_install
cargo test -p gray --lib marketplace
cargo test -p gray --lib install_manager
cargo test -p gray --lib plugin_cmds
cargo test -p gray --lib reload_agent
```

Expected: all pass without real credentials or third-party downloads.
Confirm specific tests cover the design's network error, hash failure,
Dangerous scan, declined capability consent, cancellation, close-during-commit,
rollback, post-commit warning, and activation-failure outcomes.

- [ ] **2. Run required CI exactly.**

```sh
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo nextest run --workspace --profile ci
```

Expected: exit 0 for all three. If formatting fails, run `cargo fmt` and inspect
only intended file changes, then rerun checks. For any newly discovered bug,
first add a failing regression in its owning test file, implement the smallest
fix, and rerun the focused suite before repeating affected workspace checks.
Report environmental or pre-existing failures explicitly rather than claiming
a green release.

- [ ] **3. Verify scope, then commit any validation fixes.**

```sh
git diff --check
git status --short
git diff
```

Expected: only intended fix/test files, or no changes. Stage exact changed
paths, review `git diff --cached`, and use
`git commit -m "fix(plugin): address marketplace validation regressions"` only
when there are actual validation fixes. Do not create an empty commit. If
all prior tasks were already green, the previous feature commit is the phase's
completion commit.

## Plan self-review and resolved details

- Coverage: A1 covers catalog/metadata/no-cache; A2/A3 cover exact identity,
  safe staging, constraints, atomic registry publication, rollback and races;
  B1–B3 cover scan/consent/probe/adoption; C1–C4 cover all nine UI states,
  plugin-only tabs, search/scroll/details, fake workers and snapshots;
  D1/D2 cover activation and CI.
- No implementation changes accompany this plan. The first preparation
  confirmation explicitly acknowledges third-party execution before capability
  consent; scans are not a sandbox.
- Main's pre-existing dependency and hub behavior is not inherited: no missing
  dependency auto-install, no hub fallback, no hidden extra packages.
- Existing CLI conflict override remains an explicit operator choice; it does
  not bypass Dangerous scan, missing dependencies, target-key collision or
  capability consent.
- A staged commit is a guarded filesystem/registry transaction with rollback,
  not a claim of a single crash-atomic multi-file filesystem primitive.
- The successful prepared manifest is reused. CLI-first/wire-fallback
  compatibility can start a second transport only when the first is not a
  manifest-capable CLI, never after successful probing or after commit.
- Implementation phases must retain these signatures; if a phase changes a
  contract, update dependent tests and the handoff before assigning its
  successor. No unresolved product decisions are needed to begin Phase A.
