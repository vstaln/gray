# Native Windows installation

Windows 11 x64 installs natively: no WSL, no Linux distro, no elevation. Git for
Windows supplies the shell for tool calls; Gray does not install it or WSL. The
installer's default route is native — `-Wsl` is the explicit compatibility
route that installs the Linux build inside WSL.

Known limitation, unchanged by this release: credential and transcript files are
created with standard user-profile permissions and are **not** ACL-hardened
(`std` has no portable way to do it). Keep that in mind before storing sensitive
material in `%USERPROFILE%\.gray`.

## Get and install the preview

1. Open a **successful CI run for the Windows branch** on GitHub Actions and
   download its `windows-native-preview` artifact (GitHub login may be required).
2. Extract the artifact wrapper ZIP. It contains the payload
   `gray-beta-x86_64-windows.zip`, its `.sha256` file, and both installer scripts
   under `dist`. Keep the scripts together and inspect them before running.
3. On Windows 11 x64, run in PowerShell from the directory containing the payload:

```powershell
$hash = ((Get-Content .\gray-beta-x86_64-windows.zip.sha256).Trim() -split '\s+')[0]
# Run from the extracted artifact root.
.\dist\install.ps1 -Native -ArchivePath .\gray-beta-x86_64-windows.zip -Sha256 $hash
```

Native is the default route; `-Native` is accepted and changes nothing. `-Wsl`
selects the compatibility route. The two cannot be combined. Native failures never
invoke WSL or install system dependencies. Older artifacts may contain only
`dist/install-native.ps1`; invoke that script directly with the same archive and
checksum arguments.

The default destination is `%LOCALAPPDATA%\Programs\gray\bin`. `-InstallDir`
overrides `GRAY_INSTALL_DIR`. `-NoPath` skips user PATH updates. Open a new terminal
if PATH changed, then run `gray --version` and `gray`. Install Git for Windows
with Git Bash for shell commands; Gray does not install it or WSL automatically.

Artifacts are unsigned. The digest detects mismatched/corrupt downloads, not an
independent publisher signature. Follow organizational execution policy; do not
disable antivirus, SmartScreen, or machine policy to run the preview. A policy
blocking scripts can use manual extraction of the payload instead.

The network download mode is implemented but **no production Windows payload URL
is promised**. Use the offline artifact mode above, not a guessed CDN command.

## Behavior and limitations

- Native config/session roots use `%USERPROFILE%\.gray`, or `GRAY_HOME` when set.
  Unix `HOME` is not required; WSL and native homes are not automatically migrated.
- Model requests include the launch directory without needing an initial `pwd`.
- Shell execution uses Git's POSIX shell, not PowerShell or WSL. Set `GRAY_BASH`
  to an absolute Git shell executable for custom installations. Use POSIX quoting
  and Git Bash paths in command strings. Background descendants end with the call.
- Manual/self/automatic updating does not replace a running executable. Close all
  Gray sessions and rerun the external installer with a verified newer archive.
- Native gateway and cron execution are unsupported and explicitly rejected.
  File-only cron management is available, but storing a job does not execute it.
- Unix shebang plugins are not automatically converted to Windows executables.
- Credential/transcript ACL hardening, full native plugin/file/clipboard coverage,
  interactive TUI validation, and clean-machine runtime dependencies remain open.

Uninstall: close Gray, remove the installation folder, and remove only that
folder's user PATH entry. Keep `.gray` unless you intentionally want to delete
configuration, keys, sessions, and logs.

## Verification scope

CI runs native shell contract and descendant-termination tests, profile tests
without HOME, and real CLI tests against a local mock provider (no real keys).
The installer tests both PowerShell versions against the actual native binary:
install/reinstall, channel selection, bad/missing checksum, locked destination,
concurrent installer lock, archive traversal, PATH merging, and old-binary
preservation. Linux/macOS workspace checks remain separate regression gates.

This is a testable preview checkpoint, not completion of the preparation spec.
