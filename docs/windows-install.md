# Native Windows installation preparation

Status: proposed specification, not an announcement of Windows support.

## Goal and scope

Make Gray installable and usable as a native Windows application without WSL.
This document defines the first supported release and the work required before
publishing it. It does not implement the port or change current support claims.

**Recommended first target:** Windows 11 x64, built with Rust's
`x86_64-pc-windows-msvc` target, running in Windows Terminal. Keep WSL as the
existing alternative. Windows ARM64, older Windows versions, MSI, Store/winget
packages, and a native background service are deferred until separately tested.

The first release must support the CLI, interactive TUI, provider configuration,
print mode, session persistence/resume, and bounded shell execution. Native
sidecar executables must work without changing plugin wire v1. Features excluded
from this initial scope must fail explicitly rather than appear functional.

## Current repository evidence

Re-verified against the tree on 2026-09-21. An earlier revision of this table was
written before the Windows port landed, and several rows asserted the *absence*
of code that now exists behind `#[cfg(windows)]`; they were wrong and are
corrected here rather than left to mislead. Only rows marked **open** still
represent work.

| Area | Verified behavior (2026-09-21) | Status |
| --- | --- | --- |
| Installation | `dist/install.ps1` still defaults to the **WSL** parameter set: with no arguments it checks for `wsl.exe`, may install Ubuntu, and pipes `install.sh` into the distro. `dist/install-native.ps1` is the native installer — no elevation, a 10 s bounded version probe, staged replace with backup, and it rejects any archive entry that is not exactly one of `gray.exe`/`LICENSE`/`THIRD_PARTY_NOTICES.md` at the root. | The WSL default is a deliberate product decision, not a stub. **Open:** the bare `iwr ... | iex` form the site advertises still lands a user in WSL. |
| CI | `ci.yml`'s `windows-runtime` job is a **required** check on windows-2025: full workspace test suite, cross-platform shell lifecycle regression, native shell contract, profile resolution without `HOME`, and the native installer under **both PowerShell 7 and Windows PowerShell 5.1**. No `continue-on-error`. | Closed |
| Release | `release.yml` gained a `windows-release` job: builds `x86_64-pc-windows-msvc`, smoke-tests `gray.exe --version`, packages `gray-<channel>-x86_64-windows.zip` with a lowercase `sha256sum`-compatible checksum, stages on the CDN, and is HTTP-verified in `finalize`; `publish` attaches the zip to the GitHub release. | Closed by this branch |
| Shell | `shell/windows.rs` (`shell_path()`) resolves Git Bash via `GRAY_BASH`, else `ProgramW6432`/`ProgramFiles`/`ProgramFiles(x86)`/`LOCALAPPDATA`/`PATH` candidates. `spawn.rs` calls `super::windows::spawn_owned`, which assigns the child to a **Job Object**. | Closed |
| Cancellation | `kill.rs`'s `term_then_kill` is `#[cfg(windows)]` and calls `job.terminate()`; the job handle closes any remaining descendants when dropped. This is the gate the spec named, and CI exercises it. | Closed |
| Updates | `update.rs`'s `run_installer` **refuses** under `cfg!(windows)` — "self-update is not supported on native Windows: close Gray and rerun install-native.ps1…". It does not fall through to the WSL installer. | Closed (explicit refusal, not a silent fallback) |
| Paths | `HOME`-based resolution audited; `ci.yml` carries a "Resolve native profile without Unix HOME" step. | Closed |
| Persistence | Session/cron/catalog use temp-file + rename; Unix-only permission branches remain `#[cfg(unix)]`. | Covered by the full Windows workspace suite |
| Gateway | `gateway/pid.rs` has both `#[cfg(unix)]` and `#[cfg(windows)]` branches. | Closed for pid; **open:** Windows service/IPC still deferred |
| Clipboard | `composer/input/clipboard.rs` has a Windows branch and helper-path resolution. | Present and CI-covered |
| File reads | `gray-tools/src/read/guard.rs` blocks Unix device paths and has a non-Unix special-file refusal. | Present and CI-covered |

Some README descriptions differ from the current crate layout and shell surface.
Implementation must follow current types, callers, and tests, not copy outdated
README contracts into the port.

## Prerequisites

### End users

- Windows 11 x64, with an ordinary user account and a writable local profile.
- Windows Terminal recommended; validate console behavior rather than depending
  on a particular terminal version implicitly.
- Windows PowerShell 5.1 or PowerShell 7 for installation.
- HTTPS access to the release host and the configured model provider.
- **Git for Windows, including Git Bash**, for the initial shell backend.
  A native Gray executable with an explicit shell dependency is preferable to
  silently executing PowerShell under a tool documented as `bash`.
- No Rust toolchain, WSL, administrator privileges, or global execution-policy
  changes for a release installation.

Git Bash is a proposed product dependency, not verified compatibility. Prove
process-tree cancellation with it before adopting this release design. If it
cannot satisfy that contract, revise the shell design before shipping; do not
substitute a different command language silently.

### Contributors

- Rust stable through rustup, including the MSVC target.
- Visual Studio Build Tools with the C++ build tools and Windows SDK.
- Git for Windows; ripgrep for the grep-tool/parity tests that require it.
- PowerShell and a native Windows test host. Cross-compilation from Linux is a
  useful check but cannot establish console, ACL, or process-lifecycle behavior.

## Proposed installation contract

1. Publish `gray-<channel>-x86_64-windows.zip` containing `gray.exe` and the
   required license notices. Support `stable` and `beta` explicitly. Verify on a
   clean machine whether the MSVC build needs a redistributable; bundle what is
   permitted or document the prerequisite before claiming no runtime dependency.
2. Extend `dist/install.ps1` to accept `-Channel stable|beta`, `-InstallDir`,
   `-NoPath`, and an explicit `-Wsl` compatibility route. Native is the intended
   default only once all release gates below pass. Document that default change.
3. Default binary destination: `%LOCALAPPDATA%\Programs\gray\bin\gray.exe`.
   `-InstallDir` overrides `GRAY_INSTALL_DIR`, which overrides the default.
   Data remains separate from the executable and is never removed by reinstall.
4. Detect OS/architecture before downloading. Reject unsupported systems with
   a nonzero exit and instructions for the WSL alternative; do not install WSL,
   a distro, Git, or a system package manager implicitly.
5. Download into a unique temporary directory, require a successful HTTPS
   response, and verify the archive against the matching published SHA-256.
   Reject missing/mismatched checksums, unexpected archive members, traversal
   paths, and a missing executable before changing the installation. Checksums
   from the same publisher detect corruption; they are not independent publisher
   authentication. Never describe an unsigned artifact as signed or trusted by
   Windows merely because its checksum matches.
6. Stage and verify the candidate with `--version`, then replace the destination.
   Failure must preserve the previous usable executable and report the cause.
   A locked/running destination must produce a close-Gray-and-retry instruction,
   not an elevation prompt, reboot requirement, or false success.
7. Add only the install directory to the **user** PATH, preserving unrelated
   entries and avoiding case-insensitive duplicate entries. Do not use a PATH
   update mechanism that truncates existing values. Report PATH-update failure
   separately from successful binary installation and print the absolute launch
   path. Explain that already-open terminals may need reopening. `-NoPath`
   leaves PATH unchanged.
8. Run the installed absolute path with `--version` before reporting success.
   Repeated installation is safe. Clean temporary files on success/failure.
   Detect an earlier `gray` on PATH and explain command shadowing without
   modifying unrelated installations.
9. Offer manual ZIP extraction as the fallback when enterprise policy blocks
   scripts. Do not instruct users to disable antivirus, SmartScreen, or machine
   execution policies. State signing status and expected warnings in release
   documentation.

Future installer invocation, **not available as native installation today**:

```powershell
# Download and inspect the script before running it.
Invoke-WebRequest https://gray.alignment.id/install.ps1 -OutFile install-gray.ps1
.\install-gray.ps1 -Channel stable
# Open a new terminal after a PATH change.
gray --version
gray
```

Uninstallation for this first release is manual: close Gray, remove its install
folder and only its user PATH entry. Preserve `%USERPROFILE%\.gray` unless the
user separately chooses to delete their credentials, sessions, and configuration.

## Runtime portability requirements

### Shell execution and cancellation — release blocker

Preserve the current tool name, argument schema, non-interactive behavior,
stdout/stderr capture, log persistence, exit reporting, and timeout bounds
(default 30 seconds, maximum 600 seconds in the inspected contract).

Resolve Git Bash deterministically: an explicit `GRAY_BASH` absolute executable
path first, then known Git for Windows installation locations, then a validated
Git Bash executable on PATH. Never select the WSL launcher accidentally. Report
missing or invalid shell configuration with actionable instructions. CLI help,
version, and configuration must remain usable without a shell; shell requests
must return a clear tool error, not panic or hang.

Run commands with the existing `sh -c` semantics using Git's shell; do not start
login profiles or translate command strings into PowerShell. Supply the working
directory through the process API and pass the command as one argument. Test
spaces, apostrophes, Unicode, drive letters, and Windows-to-Git path behavior.
Update model-facing platform/shell guidance so generated commands use the actual
backend and valid paths.

Use a Windows Job Object or another demonstrated equivalent to own and terminate
the complete child tree. Ownership must be established before a child can spawn
untracked descendants. Handle job-assignment failure without leaving a running
unowned child. Do not treat the Unix `pgid` field or a PID cast as a Windows
process-tree implementation. Preserve bounded output draining and partial output
on cancellation. Test Ctrl-C, timeout, grandchildren, and early root-process exit.
Do not promise POSIX signal labels on Windows or kill unrelated processes.

### Home directories, files, and credentials

- Preserve `GRAY_HOME` as the explicit data-root override. On native Windows,
  default to `%USERPROFILE%\.gray`; use a platform profile lookup if the variable
  is absent, and return an actionable error if no profile can be resolved.
  Do not silently store credentials in the current project directory.
- Apply the same rule to config/auth, sessions, logs, skills, packages, cron,
  update locks, and plugin-host environments. Prefer existing helpers and small
  shared functions over a new platform abstraction framework. Expand `~` with
  the same resolved user profile, independently of the data-root override.
- Keep Unix home resolution and existing persisted schemas unchanged. WSL and
  native Windows have separate default homes; do not auto-migrate or share them.
- Use path APIs and platform path-list splitting, not literal `/`, `:`, or shell
  quoting. Exercise CRLF, non-ASCII profiles, paths with spaces, and long paths.
  UNC/network homes are outside the first supported configuration; report errors
  clearly and do not claim their locking/permission behavior is verified.
- Preserve old data when rename/replacement fails, a file is locked, disk space
  is exhausted, or writes are denied. Verify supported local NTFS behavior.
- Define and test Windows ACL protection for credentials and raw transcripts.
  Unix `0600` branches do not establish privacy on Windows. Newly created private
  storage must not grant unrelated ordinary users access; explicitly selected
  broadly accessible roots must fail validation or require remediation before
  secrets are persisted. Do not rewrite permissions on unrelated parent folders.
- Reject Windows reserved device names, device namespaces, and named pipes before
  attempting reads that could block. Keep ordinary-file behavior and negative
  Unix read tests intact; this is not a shell sandbox.

### UI, integrations, and deferred features

Verify raw mode cleanup, Ctrl-C, resize, Unicode display, paste/newlines, clipboard
text and images, and non-TTY print mode. Reuse existing crossterm and clipboard
code where possible. A missing clipboard helper must not freeze the TUI.

Native `.exe` stdio plugins must preserve wire v1, startup errors, timeout, and
crash degradation. Declare interpreter requirements for script plugins; do not
pretend Unix shebang scripts run directly on Windows. Validate package executable
discovery and filename extensions before advertising plugin installation support.

The initial native release excludes the gateway daemon/service, cron execution,
and Unix-script delivery hooks. Reject gateway operations and cron execution
explicitly before starting work, including any automatic REPL scheduler startup;
file-only cron list/add/show/remove operations may remain available if tested.
Explain that stored jobs require a supported execution host. Do not silently
accept a schedule and imply Windows will execute it in the background. Native
service registration, IPC, and process identity need a separate spec.

### Update behavior

For the first native release, use **external reinstall**, not an in-process
updater: `gray update` prints the native reinstall command and close-Gray
instructions, with a nonzero status indicating no update was performed. Startup
checks may announce a newer version but must not offer an in-process install or
spawn `sh`. `GRAY_NO_UPDATE_CHECK=1` still suppresses checks;
`GRAY_AUTO_UPDATE=1` must explicitly report that native automatic installation is
not supported, rather than silently invoking WSL. Preserve the existing
stable-only auto-update policy on other platforms.

A later self-update design must address locked executables, concurrent sessions,
installer locking, crash recovery, and cleanup of staged versions before it is
enabled. Do not ship a background helper merely to reach the first release.

## Build and verification plan

These are future native validation commands, not commands verified by this spec:

```powershell
rustup target add x86_64-pc-windows-msvc
$env:CARGO_BUILD_JOBS = '4'
cargo check --locked --workspace --target x86_64-pc-windows-msvc
cargo build --locked --release -p gray --target x86_64-pc-windows-msvc
.\target\x86_64-pc-windows-msvc\release\gray.exe --version
.\target\x86_64-pc-windows-msvc\release\gray.exe --help
cargo test --locked --workspace --target x86_64-pc-windows-msvc
cargo clippy --locked --workspace --target x86_64-pc-windows-msvc -- -D warnings
cargo fmt --check
```

Test the project's existing test files in full. Adapt shell fixtures for Windows
or add platform-equivalent fixtures; only truly Unix-specific assertions should
be gated. Do not disable whole suites to obtain a green build. In particular,
cover shell spawn/kill/exit/bash tests, session storage, update tests, plugin
builder/sidecar/package tests, read guards, and clipboard tests.

| Acceptance gate | Required evidence |
| --- | --- |
| Native build | Required Windows CI check, release build, tests, and clippy pass; remove advisory `continue-on-error` only when this is true. |
| Clean install | Fresh standard-user Windows host with no WSL or Rust can install, run `--version`, open the TUI, and use a configured provider. Any runtime prerequisite is explicitly declared. |
| Installer failures | Local test fixtures cover stable/beta, unsupported architecture, failed download, bad/missing checksum, malformed ZIP, traversal, permission denial, locked executable, PATH failure/shadowing, repeat install, and cleanup. Old installation remains usable. |
| Shell lifecycle | Real Git Bash child and grandchild processes are gone after timeout/Ctrl-C; partial output and exit errors remain accurate; missing shell fails clearly. |
| Persistence | Session/cron/catalog use temp-file + rename; Unix-only permission branches remain `#[cfg(unix)]`. | Covered by the full Windows workspace suite |
| UI and plugins | Windows Terminal manual checks plus automated non-TTY and sidecar tests; clipboard helper failure is bounded. |
| Scope honesty | Gateway/cron execution and automatic update attempts produce the documented unsupported behavior; no implicit WSL route. |
| Release integrity | ZIP, checksum, channel selection, installed version, and download URL agree; promotion occurs only after all required artifacts/checksums are available. |
| Regression | Existing Linux/macOS builds, full test suites, installers, and update behavior remain passing. |

Use deterministic local HTTP fixtures for installer tests; production provider
credentials are not required in public CI. Keep one separately authorized manual
provider smoke test for interactive and print-mode validation. Test packaged
artifacts, not only a developer binary. A cross-target check alone cannot satisfy
any runtime gate above.

## Delivery sequence

1. **Portability baseline:** confirm failures with the current code on native
   Windows; implement home/path, shell ownership/cancellation, private storage,
   and explicit unsupported-feature behavior. Keep Linux/macOS tests passing.
2. **Native validation:** add Windows-equivalent fixtures and enforce CI; prove
   the Git Bash dependency and console behavior on a clean standard-user host.
3. **Packaging and installer:** publish beta ZIP/checksum artifacts, implement
   native PowerShell installation, and test rollback/error paths. Stage artifacts
   and checksums before promoting the channel version; preserve current Unix
   asset names and update consumers.
4. **Release readiness:** complete the acceptance table, document source builds,
   dependency/signing status, WSL alternative, manual uninstall/update, and known
   limitations. Only then update README platform support and make native install
   the PowerShell default; promote stable after beta evidence is recorded.

This specification stops at preparation. Shell dependency, limited first-release
feature scope, and external-only updating are proposed decisions for review before
implementation. No implementation, release, or deployment is authorized by this
document alone.
