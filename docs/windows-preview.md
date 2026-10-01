# Native Windows installation

Windows 11 x64 installs natively: no WSL, no Linux distro, no elevation. Git for
Windows supplies the shell for tool calls; Gray does not install it or WSL. The
installer's default route is native — `-Wsl` is the explicit compatibility
route that installs the Linux build inside WSL.

Known limitation, unchanged by this release: credential and transcript files are
created with standard user-profile permissions and are **not** ACL-hardened
(`std` has no portable way to do it). Keep that in mind before storing sensitive
material in `%USERPROFILE%\.gray`.

## Install

On Windows 11 x64, in PowerShell (5.1 or 7):

```powershell
irm https://gray.alignment.id/install.ps1 | iex
```

That installs the latest **stable** release. Pass arguments through a script
block, for example the beta channel (rebuilt on every push to main):

```powershell
& ([scriptblock]::Create((irm https://gray.alignment.id/install.ps1))) -Channel beta
```

`install.ps1` fetches `install-native.ps1` from the same site over HTTPS and runs
it as a script block, so no execution-policy change is needed. The native
installer downloads `gray-<channel>-x86_64-windows.zip`, verifies it against the
channel's `SHA256SUMS-<channel>` file (the same file `install.sh` uses), checks the
archive holds only `gray.exe`, `LICENSE` and `THIRD_PARTY_NOTICES.md`, probes
`gray.exe --version`, and only then replaces any existing binary. You can read both
scripts first at `https://gray.alignment.id/install.ps1` and
`https://gray.alignment.id/install-native.ps1`.

Native is the default route; `-Native` is accepted and changes nothing. `-Wsl`
selects the compatibility route. The two cannot be combined. Native failures never
invoke WSL or install system dependencies.

The default destination is `%LOCALAPPDATA%\Programs\gray\bin`. `-InstallDir`
overrides `GRAY_INSTALL_DIR`. `-NoPath` skips user PATH updates. Open a new terminal
if PATH changed, then run `gray --version` and `gray`. Install Git for Windows
with Git Bash for shell commands; Gray does not install it or WSL automatically.

Artifacts are unsigned. The digest detects mismatched/corrupt downloads, not an
independent publisher signature. Follow organizational execution policy; do not
disable antivirus, SmartScreen, or machine policy to run Gray.

### Offline install

Download the release zip and keep `install.ps1` and `install-native.ps1` together
(both are attached to the source tree under `dist`). Pass the archive and its
digest from the release `SHA256SUMS`:

```powershell
.\dist\install.ps1 -ArchivePath .\gray-stable-x86_64-windows.zip -Sha256 <digest>
```

To update, close every Gray session and run the installer again.

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
concurrent installer lock, archive traversal, PATH merging, old-binary
preservation, and checksum lookup in the channel's `SHA256SUMS` file. Linux/macOS workspace checks remain separate regression gates.

