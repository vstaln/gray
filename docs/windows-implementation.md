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
