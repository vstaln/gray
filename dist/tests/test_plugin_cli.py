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

    def test_setup_registers_sidecar_and_forwarding_survives(self):
        # Emulates the catalog plugin's own register(): `gray discord setup`
        # writes a sidecar argv into plugins/lock.json and refuses to
        # overwrite a foreign argv. Install must leave no 'discord' lock
        # entry behind, and `gray discord` must keep forwarding after the
        # lock entry appears (register() drops cli_argv).
        script = self.home / 'fixture.py'
        script.write_text('import sys,json,os\n'
                          'from pathlib import Path\n'
                          'if sys.argv[1:] == ["setup"]:\n'
                          '    lock = Path(os.environ["GRAY_HOME"]) / "plugins/lock.json"\n'
                          '    data = json.loads(lock.read_text()) if lock.exists() else {"schema":1,"plugins":{}}\n'
                          '    argv = [sys.executable, str(Path(__file__).resolve()), "sidecar"]\n'
                          '    prev = data["plugins"].get("discord")\n'
                          '    if prev and prev.get("argv") != argv:\n'
                          '        print("refusing to overwrite a foreign argv", file=sys.stderr); sys.exit(1)\n'
                          '    data["plugins"]["discord"] = {"ecosystem":"gray-native","version":"0","hash":"",'
                          '"source":"fixture","argv":argv,"adapter_version":"1","installed_at":"","scope":"user","enabled":True}\n'
                          '    lock.write_text(json.dumps(data))\n'
                          '    sys.exit(0)\n'
                          'print(json.dumps(sys.argv[1:]))\n')
        path = self.home / 'plugins/commands.json'
        path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps(dict(schema=1, plugins=dict(discord=dict(
            ecosystem='gray-cli', version='test', hash='', source='fixture',
            argv=[sys.executable, str(script)], adapter_version='1',
            installed_at='', scope='user', enabled=True)))))

        setup = self.run_gray('discord', 'setup')
        self.assertEqual(setup.returncode, 0, setup.stderr)
        status = self.run_gray('discord', 'status')
        self.assertEqual(status.returncode, 0, status.stderr)
        self.assertEqual(json.loads(status.stdout), ['status'])

    def test_disable_enable_list_and_remove_commands_registry_plugin(self):
        self.registry()
        listing = self.run_gray('plugin', 'list')
        self.assertIn('discord', listing.stdout)

        self.assertEqual(self.run_gray('plugin', 'disable', 'discord').returncode, 0)
        lock = self.home / 'plugins/lock.json'
        if lock.exists():
            self.assertNotIn('discord', json.loads(lock.read_text())['plugins'])
        result = self.run_gray('discord', 'status')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('disabled', result.stderr)

        self.assertEqual(self.run_gray('plugin', 'enable', 'discord').returncode, 0)
        result = self.run_gray('discord', 'status')
        self.assertEqual(result.returncode, 23, result.stderr)

        self.assertEqual(self.run_gray('plugin', 'remove', 'discord').returncode, 0)
        commands = json.loads((self.home / 'plugins/commands.json').read_text())
        self.assertNotIn('discord', commands['plugins'])
        result = self.run_gray('plugin', 'remove', 'discord')
        self.assertNotEqual(result.returncode, 0)

    def test_disabled_corrupt_and_invalid_names_fail_closed(self):
        self.registry(enabled=False)
        result = self.run_gray('discord', 'setup')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('disabled', result.stderr)
        (self.home / 'plugins/commands.json').write_text('{bad')
        self.assertNotEqual(self.run_gray('discord', 'setup').returncode, 0)
        self.assertNotEqual(self.run_gray('../discord', 'setup').returncode, 0)

    def test_reinstall_overwrites_and_keeps_registration(self):
        self.registry()
        result = self.run_gray('install', 'plugin', 'discord')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('installed discord', result.stdout)
        commands = json.loads((self.home / 'plugins/commands.json').read_text())
        self.assertIn('discord', commands['plugins'])
