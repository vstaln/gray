"""Black-box CLI contract; run with GRAY_TEST_BIN pointing at a built gray."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


@unittest.skipUnless(os.environ.get('GRAY_TEST_BIN'), 'Set GRAY_TEST_BIN to run CLI integration')
class PluginCliTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='gray plugin ')
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('GRAY_')}
        self.env['GRAY_HOME'] = str(self.home)
        self.binary = str(Path(os.environ['GRAY_TEST_BIN']).resolve())

    def run_gray(self, *args, input=''):
        return subprocess.run([self.binary, *args], env=self.env, input=input,
                              capture_output=True, text=True, timeout=30)

    def registry(self, enabled=True):
        script = self.home / 'fixture.py'
        script.write_text('import sys,json,os\n'
                          'print(json.dumps([sys.argv[1:],sys.stdin.read(),os.environ.get("GRAY_BIN")]))\n'
                          'sys.exit(23)\n')
        path = self.home / 'plugins/commands.json'
        path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps(dict(schema=1, plugins=dict(discord=dict(
            ecosystem='gray-cli', version='test', hash='', source='fixture',
            argv=[sys.executable, str(script)], adapter_version='1',
            installed_at='', scope='user', enabled=enabled)))))

    def test_install_grammar_and_unknown_name(self):
        help_result = self.run_gray('install', 'plugin', '--help')
        self.assertEqual(help_result.returncode, 0, help_result.stderr)
        self.assertIn('NAME', help_result.stdout)
        result = self.run_gray('install', 'plugin', 'does-not-exist')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Unknown plugin', result.stderr)
        self.assertFalse((self.home / 'plugins/commands.json').exists())

    def test_missing_plugin_has_install_hint_without_loading_provider(self):
        (self.home / 'config.json').write_text('{not valid provider JSON')
        result = self.run_gray('discord', 'setup')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('gray install plugin discord', result.stderr)

    def test_forwards_args_stdio_and_exit_status(self):
        self.registry()
        result = self.run_gray('discord', 'setup', '--help', 'a b', '--', '-x', input='stdin fixture\n')
        self.assertEqual(result.returncode, 23, result.stderr)
        args, stdin, binary = json.loads(result.stdout)
        self.assertEqual(args, ['setup', '--help', 'a b', '--', '-x'])
        self.assertEqual(stdin, 'stdin fixture\n')
        self.assertEqual(binary, self.binary)

    def test_disabled_corrupt_and_invalid_names_fail_closed(self):
        self.registry(enabled=False)
        result = self.run_gray('discord', 'setup')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('disabled', result.stderr)
        (self.home / 'plugins/commands.json').write_text('{bad')
        self.assertNotEqual(self.run_gray('discord', 'setup').returncode, 0)
        self.assertNotEqual(self.run_gray('../discord', 'setup').returncode, 0)

    def test_failed_install_does_not_register(self):
        self.env['GRAY_PLUGIN_PYTHON'] = '/no/such/python'
        result = self.run_gray('install', 'plugin', 'discord')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.home / 'plugins/commands.json').exists())
        self.assertEqual(list((self.home / 'plugins/cli').glob('*')), [])

    def test_installer_nonzero_exit_rolls_back_private_venv(self):
        python = self.home / 'failing-python'
        python.write_text('#!/bin/sh\nexit 7\n')
        python.chmod(0o700)
        self.env['GRAY_PLUGIN_PYTHON'] = str(python)
        result = self.run_gray('install', 'plugin', 'discord')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('installation step failed', result.stderr)
        self.assertFalse((self.home / 'plugins/commands.json').exists())
        self.assertEqual(list((self.home / 'plugins/cli').glob('*')), [])

    def test_reinstall_is_idempotent_and_preserves_other_commands(self):
        self.registry()
        before = (self.home / 'plugins/commands.json').read_bytes()
        result = self.run_gray('install', 'plugin', 'discord')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('already registered', result.stdout)
        self.assertEqual((self.home / 'plugins/commands.json').read_bytes(), before)
