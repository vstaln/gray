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
