"""Cross-platform contracts for the native Windows installer.

These run on every platform (unittest discover picks them up in the Linux and
macOS CI legs too), so the native-by-default routing and the installer's
security guards cannot regress behind a Windows-only job.
"""
import re
import unittest
from pathlib import Path

DIST = Path(__file__).resolve().parents[1]
INSTALL = (DIST / 'install.ps1').read_text(encoding='utf-8')
NATIVE = (DIST / 'install-native.ps1').read_text(encoding='utf-8')
REPO = DIST.parent
README = (REPO / 'README.md').read_text(encoding='utf-8')
PREVIEW = (REPO / 'docs' / 'windows-preview.md').read_text(encoding='utf-8')


class NativeIsTheDefaultRoute(unittest.TestCase):
    def test_install_ps1_declares_native_as_the_default_parameter_set(self):
        self.assertIn("DefaultParameterSetName='Native'", INSTALL)

    def test_routing_reads_the_parameter_set_not_the_native_switch(self):
        # A bare invocation must reach native: routing on the -Native switch
        # would send it to WSL, which is the bug this default fixed.
        self.assertIn("$PSCmdlet.ParameterSetName -ne 'Wsl'", INSTALL)
        self.assertNotIn('if ($Native) {', INSTALL)

    def test_wsl_remains_an_explicit_opt_in_route(self):
        self.assertIn("[Parameter(Mandatory=$true, ParameterSetName='Wsl')][switch]$Wsl", INSTALL)
        # The compat route still exists and still shells into the distro.
        self.assertIn('wsl.exe', INSTALL)
        self.assertIn('install.sh | sh', INSTALL)

    def test_native_failures_never_fall_back_to_wsl(self):
        self.assertIn('never fall back to WSL', INSTALL)

    def test_no_preview_or_experimental_language_left_in_the_installers(self):
        for name, text in (('install.ps1', INSTALL), ('install-native.ps1', NATIVE)):
            for word in ('experimental', 'Experimental', 'preview', 'Preview'):
                self.assertNotIn(
                    word, text, f'{name} still ships "{word}" framing'
                )
            self.assertNotIn('use WSL', text, f'{name} still points users at WSL')
            self.assertNotIn(
                'acceptance is still pending', text, f'{name} still calls itself unaccepted'
            )


class InstallerSecurityGuards(unittest.TestCase):
    """The guards that make an unsigned remote install defensible."""

    def test_downloads_require_https(self):
        self.assertIn("Scheme -ne 'https'", NATIVE)

    def test_modern_tls_is_negotiated_without_disabling_validation(self):
        self.assertIn('Tls12', NATIVE)
        self.assertNotIn('ServerCertificateValidationCallback', NATIVE)

    def test_zip_extraction_is_a_strict_allowlist(self):
        self.assertIn(
            "cnotin @('gray.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md')", NATIVE
        )
        self.assertIn('Expanded archive exceeds', NATIVE)

    def test_checksum_is_required_and_verified(self):
        self.assertIn(r"'^[0-9a-fA-F]{64}$'", NATIVE)
        self.assertIn('Archive checksum mismatch', NATIVE)

    def test_version_probe_is_bounded(self):
        self.assertIn('WaitForExit(10000)', NATIVE)
        self.assertIn('Version probe timed out', NATIVE)

    def test_replacement_is_serialized_and_rolled_back(self):
        # FileShare.None models a running gray.exe; a failed replace restores it.
        self.assertIn("'OpenOrCreate', 'ReadWrite', 'None'", NATIVE)
        self.assertIn('Another install is active', NATIVE)
        self.assertIn('Close all Gray sessions', NATIVE)
        # A failed post-install probe restores the previous binary and keeps it.
        self.assertIn('[IO.File]::Replace($backup, $binary, $null)', NATIVE)
        self.assertIn('Previous binary retained at', NATIVE)


class DocumentationClaimsMatchTheCode(unittest.TestCase):
    def test_readme_no_longer_calls_windows_unsupported(self):
        self.assertNotIn('via WSL only', README)
        self.assertNotIn('native Windows unsupported', README)
        self.assertIn('Native Windows 11 x64', README)

    def test_readme_documents_the_bare_invocation(self):
        # The advertised one-liner must land natively, not in WSL.
        self.assertIn(r'.\dist\install.ps1 -ArchivePath', README)
        self.assertNotIn(r'install.ps1 -Native -ArchivePath', README)

    def test_preview_doc_is_no_longer_a_not_supported_yet_notice(self):
        self.assertNotIn('not a supported release yet', PREVIEW)
        self.assertIn('default route is native', PREVIEW)

    def test_preview_doc_keeps_the_honest_limitations(self):
        # Native is the default route, but these remain true and documented.
        self.assertIn('gateway', PREVIEW.lower())
        self.assertIn('not', PREVIEW.lower())
        self.assertIn('Git for Windows', PREVIEW)


if __name__ == '__main__':
    unittest.main()
