"""Exercise the shipped installer with local, checksum-verified tarballs."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'install.sh'

class InstallerTests(unittest.TestCase):
    def run_install(self, args=(), shell='/bin/bash', corrupt=False):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for channel in ('stable', 'beta'):
                archive = root / f'gray-{channel}-x86_64-linux.tar.gz'
                with tarfile.open(archive, 'w:gz') as tar:
                    content = b'#!/bin/sh\necho gray-test\n'
                    info = tarfile.TarInfo('gray')
                    info.size, info.mode = len(content), 0o755
                    tar.addfile(info, io.BytesIO(content))
                digest = '0' * 64 if corrupt else hashlib.sha256(archive.read_bytes()).hexdigest()
                # Only channel-specific sums: old global manifests cannot mask a bug.
                line = f'{digest}  {archive.name}\n'
                (root / f'SHA256SUMS-{channel}').write_text(line)
            dest = root / 'bin'
            dest.mkdir()
            env = dict(os.environ, GRAY_CDN_URL=root.as_uri(), GRAY_INSTALL_DIR=str(dest),
                       SHELL=shell, HOME=str(root))
            env.pop('ZSH_VERSION', None)
            result = subprocess.run(['sh', str(SCRIPT), *args], env=env, text=True, capture_output=True)
            return result, (dest / 'gray').exists()

    def test_system_flag_without_channel(self):
        result, exists = self.run_install(['--system'])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(exists)

    def test_argument_orders(self):
        for args in [('beta', '--system'), ('--system', 'beta'), ('nightly',)]:
            result, exists = self.run_install(args)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue(exists)
            self.assertIn('beta channel', result.stdout)

    def test_shell_hints(self):
        for shell, hint in [('/bin/zsh', '.zshrc'), ('/bin/bash', '.bashrc'),
                            ('/usr/bin/fish', 'fish_add_path'), ('/bin/sh', '.profile')]:
            result, _ = self.run_install(shell=shell)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn(hint, result.stdout)

    def test_refuses_bad_arguments_and_checksum(self):
        for args, corrupt in [(('--bogus',), False), ((), True)]:
            result, exists = self.run_install(args, corrupt=corrupt)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(exists)

if __name__ == '__main__':
    unittest.main()
