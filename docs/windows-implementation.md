# Windows implementation work log

## Contract and decisions

Implement the reviewed Windows preparation spec in an isolated branch. The user
explicitly requested isolation and explanatory code comments. Base: e46d3fa;
other uncommitted changes in the original checkout are intentionally excluded.
Native release remains gated on actual Windows testing, not cross compilation.

## Plan

1. Establish Linux test baseline and Windows cross-check; retain evidence.
2. Add platform home resolution with tests; migrate all storage consumers without
   changing Unix behavior or persisted schemas.
3. Implement owned Windows process trees and deterministic Git Bash discovery;
   test cancellation/timeout and preserve Unix process-group semantics.
4. Add explicit Windows gateway/cron/update limitations and Windows file guards;
   validate private storage behavior before enabling a release.
5. Implement a native opt-in PowerShell installer and hermetic tests, with comments
   explaining checksum validation, locking, replacement, and PATH decisions.
6. Add native Windows CI/build artifacts. Keep deployment/default installation
   gated until native tests and manual clean-machine acceptance are recorded.
7. Run full relevant test files, Linux build/fmt, Windows cross-check, inspect
   all changes, and report verified results separately from pending native gates.

## Evidence

- Initial GNU cross-check stopped in ring: missing MinGW C compiler.
- Passwordless sudo is unavailable; no interactive sudo or password requested.
- Extracted Void's MinGW compiler/CRT packages into a user-local cache, without
  changing system packages. Windows GNU `cargo check --locked -p gray` passes
  on the unmodified base, with platform-specific warnings. Runtime is unverified.

## Review boundaries

No Windows runtime, PowerShell, or Wine is installed in this Linux session.
Native test results must come from Windows CI/a Windows host. Do not promote a
release, claim native support, or silently change the WSL install default until
those acceptance gates pass. Independent review is not available in this session;
self-review is not described as independent review.

## Draft PR checkpoint

Implemented the first shell portability slice: native Job Object ownership,
suspended attachment before execution, deterministic Git shell discovery, and
explicit kill errors. Added explanatory comments and cross-platform tests for
cwd/exit behavior and descendant termination. Other spec items remain pending;
no native installer or release is ready.

The existing shell contract file is deliberately unchanged and runs on Windows
CI alongside the new lifecycle regression. Native failures are evidence to fix,
not tests to skip. User authorized pushing this isolated branch and a draft PR
against main to obtain Windows CI results; no merge/release is authorized.

Local evidence before this checkpoint: gray-tools full suite passes on Linux;
Windows GNU lifecycle regression compiles but cannot run on this Linux host.
The default parallel workspace run failed the unchanged skills fingerprint
assertion; all 355 gray library tests passed when rerun serially. Windows clippy
also found an existing needless_return in read/guard.rs, left outside this slice.

Final pre-push verification: full workspace tests passed with
`cargo test --locked --workspace -- --test-threads=1`. The parallel failure above
is still disclosed; serial success is not represented as parallel success.
Reviewed job ownership: unnamed non-inheritable handle, KILL_ON_JOB_CLOSE,
suspended child assigned before resume, and TerminateJobObject for termination.
No native runtime conclusion follows from that source inspection.

## Working-directory context (cwd fix)

Report: the model spends its first tool call on `pwd`. Root cause in
system_prompt.rs: the stored prompt builder deliberately sent no directory, while
every caller already resolved one. Reproduced with a local mock provider against
the real binary: the first request carried only the saved instructions.

Fix: build_runtime_prompt appends a quoted `Working directory:` line built from
the same cwd the tools receive. AGENTS.md stays byte-identical (asserted), so
the runtime path varies without touching the stored prompt. Comment-stripping
cannot hide the line (empty/unclosed cases assert it still appears). JSON
quoting keeps Unicode, spaces, quotes and Windows backslashes unambiguous.
- `cargo test -p gray --test working_directory` fails before, passes after.
- All gray tests pass (unit + integration, serial).

## Native CI failure repairs

CI reproduced macOS SIGKILL/EPERM in the existing timeout test; the kill tests
already reaped concurrently, but the production caller waited until after
escalation. Production now uses try_join for termination plus reaping, retaining
OS failures as errors. Existing shell fixture arguments now use POSIX quoting and
Windows forward slashes; log assertions validate real files with GRAY_HOME rather
than hard-coding a Unix home. No signal-status expectation has been relaxed.

Linux CI also caught socket startup's process-wide umask(0177) removing owner
traversal from concurrently created directories. Use 0077, retaining owner access
while still excluding group/other. Full workspace tests passed in parallel locally;
fmt and workspace clippy passed. Native confirmation remains the next CI run.

## Native profile resolution

Windows run 35142774785 reproduced the missing-HOME failure in home_paths.
Shared gray-core::paths uses Rust's native profile lookup on Windows and preserves
Unix HOME lookup; config, prompt, sessions, shell logs, package roots, plugin
builder/profile and skill discovery consume it. No environment mutation or new
platform dependency is needed. Existing no-home fallback policies remain; broader
private-storage/ACL validation is still a release blocker.

The subprocess test covers unset Windows HOME, Unicode/spaces, GRAY_HOME override,
package lock reading, and profile-relative tilde expansion. Its first package
fixture used the wrong lock filename and was corrected to the existing reader's
plugins/lock.json (production format unchanged). Full local workspace tests,
formatting and clippy passed; native CI is the acceptance check.

## Preview installer checkpoint

Native home and shell CI passed in run 35144531698. Run 35145426953 built a
release-mode Windows beta binary and passed the real installer tests under
PowerShell 7 and Windows PowerShell 5.1; it uploaded a preview ZIP/checksum/script.
The public WSL installer and production release workflow remain unchanged.

Native gateway/cron execution and in-process updates are now explicitly rejected;
REPL cron auto-start is disabled on Windows. The real binary test additionally
uses a Unicode USERPROFILE with HOME and GRAY_HOME absent. CI must verify these
latest paths. docs/windows-preview.md describes artifact use and remaining gates.

Local check/clippy/fmt passed with a worktree-private target directory. Sharing the
original target directory produced a stale gray-core symbol error despite correct
source, so subsequent builds are isolated too. The parallel full workspace run
hit the pre-existing process-global skills env race; serial full workspace passed.
No full native release, ACL guarantee, or interactive TUI acceptance is claimed.


## Test environment isolation

Repeated full-suite failures showed the skills tests alternately seeing a private
HOME and the real user's skills while two tests mutated the same process-global
environment. An isolated run passed. Preserve all assertions but move each
home-sensitive test into a single-test subprocess with HOME/USERPROFILE/GRAY_HOME
set before startup (same pattern as home_paths). Full parallel workspace tests
and workspace clippy passed after removing those unsafe parent env mutations.

## Release-readiness follow-up: full native validation gate

The approved first release remains Windows 11 x64 + Git Bash, external reinstall,
with native gateway/cron execution excluded. This follow-up is not a release
announcement or authorization to publish.

Reproduced `cargo check --locked --workspace --all-targets` for Windows GNU:
`builder_enabled.rs` unconditionally imported Unix permissions. Its two existing
integration tests now use a real native echo sidecar compiled by the host Rust
compiler, retaining enable/disable, fallback, and project-overlay assertions.
The full cross-target check passes. Windows-target workspace clippy reproduced
unused Unix-only imports/constants, unreachable post-signal code, and needless
returns; these are fixed without suppressing warnings or removing assertions.

CI now requires all-target compilation, workspace clippy, and full workspace
runtime tests on Windows, with ripgrep installed and all test binaries attempted
using `--no-fail-fast`. The targeted preview checks remain. Packaging still
requires success; the public installer and release support claims are unchanged.

Verified locally on Linux: complete workspace tests, gray build, workspace
clippy with warnings denied, formatting, and diff whitespace checks. Repeated
Windows GNU all-target check and workspace clippy pass using the previously
cached MinGW toolchain. Cross-compilation does not verify Windows runtime.

Next checkpoint requires permission to push/open a follow-up PR against main so
native CI can execute the expanded suite. No native result exists for these
changes yet. Shell-script fixtures elsewhere, Windows permissions/persistence,
file-device guards, installer/release acceptance, and clean-machine/TUI checks
remain open. Do not mark the release ready until those gates have evidence.

## Windows CI failure round 1: root causes and repairs (evidence-driven)

Full workspace Windows CI (run 35211813924) exposed five distinct root causes;
each fix below cites its captured failure, and no suite was skipped or
weakened. Shell-script test fixtures that exec directly (os error 193) and
shebang-only execution remain the largest known gap, tracked separately.

1. grep fast lane (CI `fast_path_parity` left: []): vimgrep output on Windows
   is `C:\path:line:col:text`; the naive colon split produced an unparseable
   line field and silently dropped every native match. Extraction is now a
   pure `parse_vimgrep` with hand-computed unit tests for drive paths and a
   one-letter Unix path (`a:3:12:x` is not a drive). A second, latent parity
   bug surfaced locally: the extracted loop lost the match increment, so the
   limit never fired; the --json lane's count-then-limit ordering is mirrored
   and the exact failing shape now passes.
2. Sidecar/cron shebang spawns (os error 193): cron pre-scripts now run
   through the same Git Bash resolver as the bash tool (exported via the
   shell facade; a missing shell maps to the normal failure outcome, not a
   panic). Sidecar shell fixtures on Windows are still open work.
3. Plugin-name derivation (CI: cannot derive a plugin name from
   `file://C:\...`): a single-letter drive prefix no longer reads as an
   scp-style host split, pinned by a Windows-only assertion.
4. Archive guards (CI: unpack_tar_gz/unpack_zip traversal assertions):
   `/abs` is drive-relative on Windows, so raw `/` and `\` prefixes are now
   refused in addition to `is_absolute()`.
5. ENV_GUARD PoisonError cascades: one root panic poisoned the shared test
   mutex and failed ~30 unrelated suites. The lock helper now recovers from
   poison (root failures still fail their own test).

Also: `runit_log_script` forces POSIX separators (CI showed
`'/tmp/gh\logs/gateway'`), and `home_relative` matches both separators.
`gray_tools::shell::shell_path` is public for the cron runner; the private
`windows` module and discovery rules are unchanged.

Verified locally: full Linux workspace tests (41 test binaries, all green),
Linux and Windows-GNU workspace clippy, Windows all-target check, fmt, and
diff whitespace checks. Native runtime evidence remains CI's to produce.

## Windows CI failure round 3 preparation: remaining roots from run 35223251606

1. Git URL parsing completed (Linux-runnable reproducers now pass):
   `split_authority` scans `\` after a drive letter, `name_from_git_url`
   keeps the drive in the path and splits on both separators, and the
   no-scheme branch mirrors the same drive-letter rule. The round-1
   `file://C:\tmp\repo@feature` and empty-name shapes are pinned by tests.
2. Key-derivation security invariant preserved: backslash-carrying names
   stay illegal (`install_key("a\b").is_err()` untouched); the mangle
   attempt was reverted.
3. Cron pre-scripts: the script path is data, not shell code —
   `sh -c <path>` ate backslashes (`C:UsersRUNNER~1...sh: command not
   found`). Now `exec "$1"` with a forward-slashed positional.
4. Sidecar `.sh` plugins (os error 193): Windows cannot exec shebang
   scripts, so documented Git Bash shell plugins route through the same
   POSIX shell resolver as the bash tool; native executables unchanged.
5. shell_contract log-path regression from round 2 (self-inflicted):
   home_relative emitted `~\...`; the header now always emits the
   documented `~/...` shape, and the test helper resolves the
   abbreviation against GRAY_HOME (logs live there), HOME fallback.

Verified locally: full Linux workspace suite (41 binaries green), both
Linux and Windows-GNU clippy at `-D warnings`, Windows all-target check,
fmt. Windows runtime evidence remains CI's job (next run).
