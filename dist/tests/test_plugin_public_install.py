"""Opt-in acceptance test: real gray -> pinned public package -> setup CLI."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


@unittest.skipUnless(os.environ.get('GRAY_TEST_BIN') and os.environ.get('GRAY_TEST_PUBLIC_INSTALL') == '1',
                     'Set GRAY_TEST_BIN and GRAY_TEST_PUBLIC_INSTALL=1 (network install)')
class PublicPluginInstallTests(unittest.TestCase):
    def test_public_install_then_setup_and_command_help(self):
        binary = str(Path(os.environ['GRAY_TEST_BIN']).resolve())
        with tempfile.TemporaryDirectory(prefix='gray public install ') as tmp:
            env = {k: v for k, v in os.environ.items() if not k.startswith('GRAY_')}
            env['GRAY_HOME'] = tmp

            def run(*args):
                return subprocess.run([binary, *args], env=env, input='', text=True,
                                      capture_output=True, timeout=240)

            result = run('install', 'plugin', 'discord')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("Installed 'discord'", result.stdout)
            config = str(Path(tmp) / 'unconfigured-plugin.json')
            result = run('discord', '--config', config, 'setup')
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn('Setup needs a terminal for hidden token input', result.stderr)
            self.assertNotIn('unrecognized subcommand', result.stderr)
            self.assertFalse(Path(config).exists())
            for command in ['setup', 'status', 'restart', 'doctor', 'schedule']:
                result = run('discord', command, '--help')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn('gray discord', result.stdout)
            result = run('install', 'plugin', 'discord')
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('already registered', result.stdout)
