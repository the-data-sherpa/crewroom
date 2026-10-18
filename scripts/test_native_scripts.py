"""Isolated native bootstrap, packaging and launcher contracts; no live service."""
import contextlib
import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

PROJECT = Path(__file__).resolve().parent.parent


def script_module(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), PROJECT / 'scripts' / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def executable(path, body):
    path.write_text(body)
    path.chmod(0o755)


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.module = script_module('bootstrap-native')

    def bootstrap(self, system='Linux', machine='x86_64', damaged=False):
        self.name = f'zig-{machine if system == "Linux" else "aarch64"}-{system.lower() if system == "Linux" else "macos"}-{self.module.VERSION}'
        archive = io.BytesIO()
        content = b'fixture executable'
        with tarfile.open(fileobj=archive, mode='w:xz') as output:
            entry = tarfile.TarInfo(f'{self.name}/zig')
            entry.size = len(content)
            entry.mode = 0o755
            output.addfile(entry, io.BytesIO(content))
        archive_bytes = archive.getvalue()
        checksum = '0' * 64 if damaged else hashlib.sha256(archive_bytes).hexdigest()
        target = self.name.removeprefix('zig-').removesuffix(f'-{self.module.VERSION}')
        with patch.object(self.module.platform, 'system', return_value=system), \
                patch.object(self.module.platform, 'machine', return_value=machine), \
                patch.dict(self.module.TOOLCHAINS, {(system, machine): (target, checksum)}), \
                patch.object(self.module.shutil, 'which', return_value='/fixture/tool'), \
                patch.object(self.module.subprocess, 'run') as calls, \
                patch.object(self.module.subprocess, 'check_output', return_value=self.module.VERSION + '\n'), \
                patch.object(self.module.urllib.request, 'urlopen', return_value=io.BytesIO(archive_bytes)) as download, \
                patch.object(sys, 'argv', ['bootstrap', '--directory', str(self.root)]), \
                contextlib.redirect_stdout(io.StringIO()):
            self.module.main()
        return calls, download

    def test_linux_archive_is_verified_and_reused_without_download(self):
        self.bootstrap()
        target = self.root / self.name
        self.assertTrue((target / 'zig').is_file())
        receipt = self.root / (self.name + '.json')
        self.assertEqual(receipt.stat().st_mode & 0o777, 0o600)
        calls, download = self.bootstrap()
        download.assert_not_called()
        self.assertFalse(any(call.args[0][0] == 'xcrun' for call in calls.call_args_list))

    def test_linux_aarch64_uses_its_own_archive(self):
        self.bootstrap(machine='aarch64')
        self.assertTrue((self.root / 'zig-aarch64-linux-0.16.0/zig').is_file())

    def test_macos_keeps_sdk_validation_and_archive(self):
        calls, _ = self.bootstrap(system='Darwin', machine='arm64')
        self.assertTrue((self.root / 'zig-aarch64-macos-0.16.0/zig').is_file())
        self.assertIn(['xcrun', '--show-sdk-path'], [call.args[0] for call in calls.call_args_list])

    def test_bad_archive_does_not_install_a_toolchain(self):
        with self.assertRaisesRegex(RuntimeError, 'SHA256'):
            self.bootstrap(damaged=True)
        self.assertEqual(list(self.root.iterdir()), [])

    def test_changed_existing_toolchain_is_preserved_and_refused(self):
        self.bootstrap()
        zig = self.root / self.name / 'zig'
        zig.write_text('changed')
        with self.assertRaisesRegex(RuntimeError, 'differs'):
            self.bootstrap()
        self.assertEqual(zig.read_text(), 'changed')

    def test_missing_linux_library_is_reported_before_linking(self):
        diagnostics = io.StringIO()
        with patch.object(self.module.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)) as calls, \
                contextlib.redirect_stderr(diagnostics), self.assertRaises(SystemExit):
            self.module.check_linux_libraries(argparse.ArgumentParser())
        self.assertIn('Missing Linux development libraries', diagnostics.getvalue())
        self.assertFalse(any(call.args[0][0] == 'clang' for call in calls.call_args_list))

    def test_missing_libcxx_has_actionable_arch_diagnostic(self):
        def command(args, **kwargs):
            return subprocess.CompletedProcess(args, 0 if args[0] == 'pkg-config' else 1, stderr='missing fixture libc++')
        diagnostics = io.StringIO()
        with patch.object(self.module.subprocess, 'run', side_effect=command), \
                contextlib.redirect_stderr(diagnostics), self.assertRaises(SystemExit):
            self.module.check_linux_libraries(argparse.ArgumentParser())
        self.assertIn('On Arch install libc++', diagnostics.getvalue())
        self.assertIn('docs/linux.md', diagnostics.getvalue())


class PackageInspectionTests(unittest.TestCase):
    def setUp(self):
        self.module = script_module('package-native')
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.binary = Path(self.temporary.name) / 'crewroom'
        executable(self.binary, 'fixture')

    def test_linux_records_system_and_explicit_local_dependencies(self):
        with patch.object(self.module, 'run', side_effect=[
                'ELF 64-bit LSB pie executable, x86-64',
                'linux-vdso.so.1 (0x0)\n libc++.so.1 => /private libs/lib/libc++.so.1 (0x0)\n libc.so.6 => /usr/lib/libc.so.6 (0x0)\n /lib64/ld-linux-x86-64.so.2 (0x0)']):
            self.assertEqual(self.module.inspect_binary(self.binary, 'Linux', 'x86_64'), [
                '/private libs/lib/libc++.so.1', '/usr/lib/libc.so.6', '/lib64/ld-linux-x86-64.so.2'])

    def test_linux_refuses_missing_dependency(self):
        with patch.object(self.module, 'run', side_effect=['ELF 64-bit LSB pie executable, x86-64', 'libc++.so.1 => not found']):
            with self.assertRaisesRegex(ValueError, 'missing shared libraries'):
                self.module.inspect_binary(self.binary, 'Linux', 'x86_64')

    def test_linux_refuses_wrong_architecture(self):
        with patch.object(self.module, 'run', return_value='ELF 64-bit LSB pie executable, ARM aarch64'):
            with self.assertRaisesRegex(ValueError, 'native x86_64'):
                self.module.inspect_binary(self.binary, 'Linux', 'x86_64')

    def test_macos_keeps_unbundled_dependency_refusal(self):
        with patch.object(self.module, 'run', side_effect=['Mach-O 64-bit executable arm64', 'crewroom:\n /opt/homebrew/lib/external.dylib (compatibility 1)']):
            with self.assertRaisesRegex(ValueError, 'unbundled'):
                self.module.inspect_binary(self.binary, 'Darwin', 'arm64')

    def test_symlink_is_refused_before_dependency_inspection(self):
        link = self.binary.with_name('link')
        link.symlink_to(self.binary)
        with self.assertRaisesRegex(ValueError, 'regular executable'):
            self.module.inspect_binary(link, 'Linux', 'x86_64')


@unittest.skipUnless(platform.system() == 'Linux', 'Linux launcher OS permissions and GUI dispatch')
class LinuxLauncherTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='crewroom launcher ')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.launcher = self.bin / 'crewroom-launcher'
        shutil.copy2(PROJECT / 'scripts/launch-native.sh', self.launcher)
        self.data = self.root / 'private data'
        self.roles = self.root / 'private roles/roles.json'
        self.log = self.root / 'calls.jsonl'
        self.env = {**os.environ, 'CREWROOM_FIXTURE_LOG': str(self.log)}
        self.env.pop('WAYLAND_DISPLAY', None)
        # No daemon process is started: a fixture marker models the authenticated
        # daemon status result while real launcher parsing/permissions/exec run.
        executable(self.bin / 'crewroomd', '''#!/usr/bin/python3
import json, os, pathlib, sys
args=sys.argv[1:]
with open(os.environ['CREWROOM_FIXTURE_LOG'], 'a') as log: log.write(json.dumps(args)+'\\n')
data=pathlib.Path(args[args.index('--data-dir')+1])
state=data/'fixture-service-ready'
if '--status' in args: raise SystemExit(0 if state.exists() else 1)
if '--background' in args:
    state.touch()
    (data/'native-connection.json').write_text('{}')
    (data/'fixture-daemon-env.json').write_text(json.dumps({name:os.environ.get(name) for name in ['CREWROOM_JIRA_TOKEN','CREWROOM_BITBUCKET_TOKEN','CREWROOM_RUNTIME_CONFIG']}))
else: raise SystemExit(2)
''')
        executable(self.bin / 'crewroom', '#!/bin/bash\nprintf "%s\\n" "$@"\nexit 29\n')
        for name in ['crewroom-host', 'crewroom-process']:
            executable(self.bin / name, '#!/bin/bash\nexit 0\n')

    def launch(self, *extra, headless=True, data=None, roles=None):
        return subprocess.run([str(self.launcher), '--data-dir', str(self.data if data is None else data),
                               '--roles', str(self.roles if roles is None else roles),
                               *(['--headless'] if headless else []), *extra], env=self.env, capture_output=True, text=True, timeout=10)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def test_private_first_launch_then_status_reuses_service(self):
        self.assertEqual(self.launch().returncode, 0)
        second = self.launch('--port', '5300')
        self.assertEqual(second.returncode, 0)
        self.assertIn('Reusing the authenticated service', second.stdout)
        self.assertEqual(len([args for args in self.calls() if '--background' in args]), 1)
        self.assertEqual(self.data.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.roles.parent.stat().st_mode & 0o777, 0o700)

    def test_relative_paths_and_invalid_ports_do_not_start_service(self):
        self.assertNotEqual(self.launch(data='relative').returncode, 0)
        for value in ['0', '65536', '-1', 'words']:
            self.assertNotEqual(self.launch('--port', value).returncode, 0)
        self.assertFalse(self.data.exists())
        self.assertEqual(self.calls(), [])

    def test_public_directory_is_preserved_and_refused(self):
        self.data.mkdir(mode=0o755)
        self.data.chmod(0o755)
        self.assertNotEqual(self.launch().returncode, 0)
        self.assertEqual(self.data.stat().st_mode & 0o777, 0o755)
        self.assertEqual(self.calls(), [])

    def test_symlink_directory_is_refused(self):
        destination = self.root / 'other'
        destination.mkdir(mode=0o700)
        self.data.symlink_to(destination, target_is_directory=True)
        self.assertNotEqual(self.launch().returncode, 0)
        self.assertEqual(self.calls(), [])

    def test_public_role_file_is_preserved_and_refused(self):
        self.roles.parent.mkdir(mode=0o700)
        self.roles.write_text('{}')
        self.roles.chmod(0o644)
        self.assertNotEqual(self.launch().returncode, 0)
        self.assertEqual(self.roles.read_text(), '{}')
        self.assertEqual(self.calls(), [])

    def test_symlink_role_file_is_refused(self):
        self.roles.parent.mkdir(mode=0o700)
        other = self.root / 'other.json'
        other.write_text('{}')
        other.chmod(0o600)
        self.roles.symlink_to(other)
        self.assertNotEqual(self.launch().returncode, 0)
        self.assertEqual(self.calls(), [])

    def test_gui_without_wayland_does_not_create_runtime_state(self):
        result = self.launch(headless=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Wayland session', result.stderr)
        self.assertFalse(self.data.exists())
        self.assertEqual(self.calls(), [])

    def test_gui_exec_receives_connection_path_and_preserves_exit_status(self):
        self.env['WAYLAND_DISPLAY'] = 'fixture-wayland'
        result = self.launch(headless=False)
        self.assertEqual(result.returncode, 29)
        self.assertEqual((self.data / 'app.stdout.log').read_text(), f'--connection\n{self.data}/native-connection.json\n')
        self.assertEqual((self.data / 'app.stderr.log').stat().st_mode & 0o777, 0o600)
        self.assertTrue((self.data / 'fixture-service-ready').exists())

    def test_provider_credentials_reach_daemon_but_not_gui(self):
        private = {'CREWROOM_JIRA_TOKEN': 'synthetic-jira', 'CREWROOM_BITBUCKET_TOKEN': 'synthetic-bitbucket',
                   'CREWROOM_RUNTIME_CONFIG': '/fixture/runtime-config.json'}
        self.env.update(private)
        self.env['WAYLAND_DISPLAY'] = 'fixture-wayland'
        executable(self.bin / 'crewroom', '''#!/usr/bin/python3
import json, os
print(json.dumps({name:os.environ.get(name) for name in ['CREWROOM_JIRA_TOKEN','CREWROOM_BITBUCKET_TOKEN','CREWROOM_RUNTIME_CONFIG']}))
''')
        self.assertEqual(self.launch(headless=False).returncode, 0)
        self.assertEqual(json.loads((self.data / 'fixture-daemon-env.json').read_text()), private)
        self.assertEqual(json.loads((self.data / 'app.stdout.log').read_text()), dict.fromkeys(private))


class CheckoutDispatchTests(unittest.TestCase):
    def test_both_package_layouts_and_custom_cargo_target_directory(self):
        for system in ['Linux', 'Darwin']:
            with self.subTest(system=system), tempfile.TemporaryDirectory(prefix='native dispatch ') as temporary:
                root = Path(temporary)
                scripts = root / 'scripts'
                scripts.mkdir()
                shutil.copy2(PROJECT / 'scripts/run-native.sh', scripts / 'run-native.sh')
                stubs = root / 'stubs'
                stubs.mkdir()
                target = root / 'custom target'
                (target / 'release').mkdir(parents=True)
                log = root / 'launch.json'
                executable(stubs / 'uname', f'#!/bin/bash\necho {system}\n')
                executable(stubs / 'cargo', '''#!/usr/bin/python3
import json, os, sys
if sys.argv[1] == 'metadata': print(json.dumps({'target_directory':os.environ['FIXTURE_TARGET']}))
elif sys.argv[1:] != ['build','--locked','-p','crewroom-desktop','-p','crewroom-service','-p','crewroom-runtime','-j','6','--release']: raise SystemExit(3)
''')
                executable(stubs / 'python3', '''#!/usr/bin/python3
import json, os, pathlib, sys
args=sys.argv[1:]
if args[0] == 'scripts/bootstrap-native.py':
    assert args[1:] == ['--check-build-dependencies']
    print('/fixture/zig')
elif args[0] == 'scripts/package-native.py':
    assert args[args.index('--binary-directory')+1] == str(pathlib.Path(os.environ['FIXTURE_TARGET'])/'release')
    bundle=pathlib.Path(args[args.index('--output')+1])
    binary=bundle/('bin' if os.environ['FIXTURE_SYSTEM']=='Linux' else 'Contents/MacOS')
    binary.mkdir(parents=True)
    launcher=binary/'crewroom-launcher'
    launcher.write_text("#!/usr/bin/python3\\nimport json,os,sys\\nopen(os.environ['FIXTURE_LOG'],'w').write(json.dumps(sys.argv[1:]))\\n")
    launcher.chmod(0o755)
else: os.execv('/usr/bin/python3',['python3',*args])
''')
                env = {**os.environ, 'PATH': f'{stubs}:{os.environ["PATH"]}', 'FIXTURE_TARGET': str(target),
                       'FIXTURE_SYSTEM': system, 'FIXTURE_LOG': str(log), 'CREWROOM_NATIVE_DATA_DIR': 'relative data',
                       'CREWROOM_ROLE_PATH': 'relative roles/roles.json'}
                result = subprocess.run([str(scripts / 'run-native.sh'), '--release'], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(log.read_text()), ['--data-dir', str(root / 'relative data'), '--roles', str(root / 'relative roles/roles.json')])
                log.unlink()
                result = subprocess.run([str(scripts / 'run-native.sh'), '--release', '--build-only'], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertFalse(log.exists(), 'Build-only must not launch a service or desktop.')


if __name__ == '__main__':
    unittest.main()
