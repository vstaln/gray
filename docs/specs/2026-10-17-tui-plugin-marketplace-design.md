# TUI plugin marketplace

- Date: 2026-10-17
- Status: Approved design, implementation not started
- Base: `origin/main` at `5361733d`

## Goal and scope

Add a Marketplace to Gray core, not a sidecar plugin. Version 1 supports
browsing the plugin catalog, viewing details, and installing a plugin.

The Marketplace is a tab in the existing bare `/plugin` picker, including its
`/plugins` alias. Only the plugin picker gets this tab. Skills, gateway, cron,
and memory keep their existing panels. The Installed and Errors tabs retain
their current behavior; this feature does not add update, toggle, or removal
actions to the Marketplace.

Implementation follows this design in one core workstream. It does not change
`graysite`, publish a plugin, or change the sidecar protocol.

## Catalog and browsing

`https://gray.alignment.id/plugins/index.json` is authoritative. Continue to
honor `GRAY_PLUGIN_INDEX`. Do not scrape website pages or use their versions,
download counts, or descriptions as a second source of truth.

Extend `gray_pkg::index::Entry` with backward-compatible, optional display
metadata: description, author, homepage, repo, license, kind, and commands.
Fields already represented on the default branch retain compatible types and
defaults. Missing metadata renders without fabricated values. Preserve the
existing installation and trust fields.

Add an uncached catalog-fetch API. Every transition into the Marketplace tab
starts a fresh request. Retry also starts a fresh request. This API reads and
writes no catalog cache files and does not fall back to cached data. Leave
`fetch_index`, its existing cache, and CLI fetching behavior unchanged.

Use the existing HTTP client's 10-second connect and 120-second total request
deadlines. Loading never blocks keyboard input or rendering. Handle network,
HTTP-status, and malformed-JSON failures as load errors, not empty catalogs.
While loading, show `Loading marketplace…` and disable install actions. On
failure, show a concise error with Retry and Back. A successful response with
zero entries shows `No plugins available`.

Show a searchable list with a scrolling viewport that keeps selection visible.
Search filters catalog names and descriptions, case-insensitively. Keep the
catalog's deterministic name order; there are no sort or type-filter controls.
Text input owns printable keys while search is focused, including `j` and `k`.
Existing navigation keys work when search is not focused.

Selecting an entry opens details with its name, description, version, author,
trust, license, source, homepage/repository, and commands where present. The
details view provides Install and Back. It must remain usable in narrow
terminals and with long descriptions or URLs.

### Installed entries

Match the catalog map key against the unified user `lock.plugins` key. Do not
match against the runtime manifest name, which can differ from the catalog
key. Any matching lock entry counts as installed, including disabled plugins,
older versions, and registered local executables.

Show an `installed` badge and disable Install for these entries. Read the
registry when opening the Marketplace and again after an installation. A
registry read error is an explicit error, not permission to overwrite an
unknown installation.

## Installation and two-step consent

Install the exact catalog entry the user approved. Carry an immutable snapshot
of its key, version, source, hash, and trust through preparation and commit.
Do not re-resolve its name through `GRAY_PLUGIN_PATH`, the working directory,
`PATH`, the hub catalog, or a subsequent catalog fetch. Never silently switch
the approved version, source, or hash.

The new path reuses package download, verification, unpacking, scan, and
registry primitives. It does not build an independent installer with different
archive or trust rules.

### Dependency and conflict constraints

Fail closed if any `requires_plugins` entry is absent from the installed
registry. Block installation and name the missing plugins; never install
additional packages automatically. Keep the existing conflict guards. Check
dependencies and conflicts before preparation and again under the registry
guard before commit, so concurrent registry changes cannot bypass them.
These constraints do not introduce a second catalog source or additional
consent flows.

### 1. Confirm preparation

Before downloading or executing anything, show the source, version, expected
hash, and trust level. Distinguish official, verified community, and unverified
community entries. Explain that a matching hash proves integrity, not safety.

Always require confirmation, including for official entries. State clearly
that preparation runs the plugin's manifest command as the user, before
capability consent. Cancel returns to details without starting preparation.

### 2. Prepare without installing

Download the approved archive, verify its expected digest, and safely unpack
it into an uncommitted staging directory. Retain existing download-size,
expanded-size, entry-count, path-confinement, and archive-entry-type checks.

Run `scan_tree` on the staged tree before probing its manifest. A Dangerous
verdict blocks installation without an override. Show Caution findings and
carry them into the final consent screen; never silently suppress them.

Probe the manifest from staging and retain that prepared manifest for consent,
commit, and adoption. Do not probe again after commit. Bound probe duration and
output and ensure cancellation terminates and reaps its subprocess.

Esc cancels preparation, including download, unpacking, scanning, and the
manifest probe. Operations that cannot stop immediately must finish their
current bounded staging-only operation before cleanup and cancellation
complete. Cancellation does not modify installed files or the registry.

### 3. Consent to the prepared package

Show the manifest's declared capabilities, their host-defined risk
descriptions, declared commands, and scan warnings. Offer Grant or Cancel.
Grant explicitly accepts the displayed Caution findings and authorizes the
known declared capabilities. Unknown capability IDs remain ungrantable and
must not be represented as authorized.

Cancel removes staging and returns to details without installing. Nothing is
committed until Grant.

Extract reusable policy/data handling from `scan_or_block` and
`consent_capabilities` in `crates/gray/src/plugin_cli.rs`. The TUI owns all
rendering and input; installer workers must not print or prompt on stdout
under the live modal. Preserve the CLI's existing presentation and behavior.

### 4. Commit the approved package

After Grant, enter a noncancellable commit phase. Acquire the registry guard
and re-read the lock before changing the destination. If the key was installed
concurrently, do not replace it: discard staging, refresh its installed badge,
and explain that it is already installed.

Commit package files, the registry entry, capability grants, and
`capabilities_hash` as one guarded installation transaction. Atomic registry
publication and rollback must prevent reporting success with partially
recorded consent or a registry entry pointing to missing package files.
Retain existing recovery behavior and report any rollback failure clearly.

For an empty capability declaration, explicitly write
`Some(consent_hash([]))`, not `None`. A new installation must never inherit
the legacy grandfathered-capabilities behavior.

Record runtime role and executable paths from the prepared manifest. Paths
published in the lock must refer to the committed location, not staging.

### 5. Adopt and report

CLI forwarding, cached manifest metadata, and provider discovery reuse the
prepared manifest. Refactor the relevant `adopt_cli` path to avoid a second
manifest probe. A failure after the installation transaction committed means
installed with a warning, not a failed or cancelled installation.

Re-read the registry and immediately update installed badges. Show completion
or a concise failure within the modal. Install failures offer Retry and Back;
Retry begins again at confirmation rather than reusing prior consent
silently. Preserve the package error registry for actual failures.
User cancellation is not an installation error.

## State machine, concurrency, and closing

Use these explicit UI states:

| State | Behavior |
| --- | --- |
| Loading | Fetch a fresh catalog; no install actions. |
| Load error | Explain failure; Retry fetches, Back leaves the tab. |
| Browsing | Search, scroll, view details, and select an eligible entry. |
| Confirm | Review exact package and execution warning; confirm or cancel. |
| Preparing | Show the current preparation stage; Esc cancels. |
| Consent | Review capabilities, commands, and warnings; Grant or Cancel. |
| Committing | Finish the installation transaction; cancellation disabled. |
| Installed | Report installed status and any post-commit warning. |
| Install failed | Report failure; Retry or Back. |

Use stage labels and a spinner, never invented percentage progress. At most
one preparation/commit operation runs per modal. Background results belong to
their operation; late messages from a cancelled operation must not change a
new selection or start a commit.

Follow `crates/gray/src/setup/connect.rs`: `tokio::spawn`, `mpsc` progress
messages, `CancellationToken`, and a synchronous modal polling at roughly
100 milliseconds. Keep network and preparation work off the render loop.
Separate UI decisions from filesystem/network operations through a testable
backend boundary.

Closing during loading or preparation cancels and waits for cleanup. Closing
during commit waits for commit and reports its real result before completing
the close. Once commit has begun, never claim Esc undid the install. No
installation task may detach and finish silently after its modal is gone.

The terminal session, dimmed background, and viewport restoration keep their
existing ownership through `with_modal_sync`.

## Activation

After the plugin modal closes following a successful installation, prompt
`Activate now?` with Now and Later. One prompt covers the plugins installed
during that modal visit.

Now calls `reload_agent` with the current conversation history and session ID.
This rebuilds the agent and its plugin set; it is not selective hot-loading.
If no agent exists yet, the next normal build loads the installed plugins
without creating a model turn solely for activation.

A plugin startup failure produces an activation warning and keeps its files.
Do not mislabel installation as failed. If the overall rebuild fails,
`reload_agent` restores the previous agent. Later leaves the running agent
unchanged; the plugins are available on a subsequent normal rebuild or restart.
Activation requires no process restart and must not lose conversation history
or change the session's provider cache shard.

## Integration boundaries

- `crates/gray-pkg/src/index.rs`: compatible metadata and uncached fetching.
- `crates/gray-pkg/src/fetch.rs`: verified download and safe staged unpacking.
- `crates/gray-pkg/src/ops.rs`: prepared-package commit and registry guard.
- `crates/gray/src/plugin_cli.rs`: shared scan/consent policy and adoption
  from the already prepared manifest.
- `crates/gray/src/setup/install_manager.rs` and `setup/tabs.rs`: plugin-only
  Marketplace integration, leaving other shared-manager clients unchanged.
- `crates/gray/src/repl/plugin_cmds.rs` and `repl/dispatch.rs`: structured
  install outcome and activation prompt after the modal closes.
- `crates/gray/src/repl/handlers.rs`: existing history-preserving
  `reload_agent` behavior.

Keep catalog state, modal rendering/input, staged installation, and activation
as separate responsibilities. Do not grow the shared manager into a set of
unrelated plugin-specific branches.

## Validation

Use local fixtures and fake backends rather than live third-party plugins.

- **Catalog/package tests:** optional metadata compatibility; a fresh request
  on each tab entry and retry; no cache reads or writes; index override;
  HTTP, timeout, malformed-response, and empty-response handling; verification,
  archive safety, cancellation cleanup, guarded publication, rollback, and
  concurrent-install rechecks. Extend the loopback fixtures in
  `crates/gray-pkg/tests/index.rs` and existing fetch/ops tests.
- **Host-install tests:** the selected snapshot wins over same-named PATH
  binaries and hub entries; no silent version/hash substitution; Dangerous
  blocks before probing; Caution and capabilities require consent; empty
  declarations get a consent hash; probe results are reused; staged paths
  become committed paths; post-commit adoption errors report installed with
  a warning; missing dependencies block with their names and never trigger
  extra installs; conflict guards hold before preparation and commit.
- **TUI model tests:** plugin-only tab availability; installed-key matching;
  search, focus, selection and scrolling; details and disabled Install;
  every state transition; retry/cancel; no late-message commits; close during
  preparation and commit. A fake backend drives progress, failures, and
  cancellation deterministically without executing plugin code.
- **Snapshot tests:** loading, network failure, empty catalog, populated list,
  installed badge, details, both consent steps, progress, completion, warnings,
  and narrow-terminal layouts using a Ratatui test backend.
- **Activation tests:** Now versus Later, preserved messages and session ID,
  lazy first build, startup warnings with retained files, and restoration of
  the previous agent on rebuild failure.

Run focused package, host, and modal tests during implementation, then core CI:

```sh
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo nextest run --workspace --profile ci
```

## Non-goals and known limitation

No new update, enable/disable, or remove workflows; cache or offline mode;
sort or type filters; alternate catalog sources; or website changes.
Existing CLI package behavior remains outside this new interaction.
`GRAY_PLUGIN_INDEX` continues to override the one Marketplace catalog.

Catalog-declared capabilities are a possible later `graysite` change, not a
requirement for this version. Under today's protocol the manifest probe runs
third-party code as the user before capability consent. The first confirmation
must disclose this. Scanning does not sandbox the plugin, and capability grants
limit host surfaces rather than operating-system access.
