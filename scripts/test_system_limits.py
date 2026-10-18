"""FOR-68: a hard RLIMIT_RTTIME of 0 makes the kernel SIGKILL test processes; `system_limits.py run` avoids it."""
import json
import os
from pathlib import Path
import resource
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

import system_limits as limits

INFINITY = resource.RLIM_INFINITY
SCRIPT = Path(limits.__file__).resolve()


def fixed(soft, hard):
    return lambda _resource: (soft, hard)


def user_systemd_available():
    if shutil.which('systemd-run') is None:
        return False
    return subprocess.run(['systemctl', '--user', 'is-system-running'], capture_output=True).returncode in (0, 1)


class HazardTests(unittest.TestCase):
    def test_only_a_zero_limit_is_a_hazard(self):
        self.assertIsNone(limits.hazard(None))
        self.assertIsNone(limits.hazard((INFINITY, INFINITY)))
        self.assertIsNone(limits.hazard((200_000, 1_000_000)))
        self.assertIn('hard limit is 0', limits.hazard((0, 0)))
        self.assertIn('soft limit is 0', limits.hazard((0, INFINITY)))

    def test_a_zero_soft_limit_is_raised_to_a_nonzero_hard_limit(self):
        calls = []
        self.assertEqual(limits.raise_soft((0, 5_000), lambda *args: calls.append(args)), (5_000, 5_000))
        self.assertEqual(calls, [(limits.RLIMIT_RTTIME, (5_000, 5_000))])
        self.assertEqual(limits.raise_soft((0, 0), lambda *args: calls.append(args)), (0, 0))
        self.assertEqual(len(calls), 1, 'a zero hard limit cannot be raised and is not attempted')

    def test_signal_deaths_report_shell_style_statuses(self):
        self.assertEqual(limits.exit_status(-9), 137)
        self.assertEqual(limits.exit_status(3), 3)


@unittest.skipIf(limits.RLIMIT_RTTIME is None, 'RLIMIT_RTTIME is Linux-only')
class RunTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='crewroom-system-limits-test-')
        self.environ = {'PATH': os.environ['PATH'], 'XDG_RUNTIME_DIR': self.temp.name,
                        'FIXTURE_SECRET': 'fixture-secret-value', 'INVOCATION_ID': 'caller-unit'}
        self.messages = []

    def tearDown(self):
        self.temp.cleanup()

    def run_with(self, soft, hard, which, spawn):
        out = type('Out', (), {'write': lambda _, text: self.messages.append(text), 'flush': lambda _: None})()
        return limits.run(['cargo', 'test'], self.environ, fixed(soft, hard), lambda *_: None, which, spawn, out)

    def test_safe_limits_run_the_command_directly(self):
        calls = []
        code = self.run_with(INFINITY, INFINITY, lambda _: self.fail('systemd-run must not be needed'),
                             lambda argv, **kwargs: calls.append((argv, kwargs)) or 0)
        self.assertEqual(code, 0)
        self.assertEqual(calls[0][0], ['cargo', 'test'])

    def test_a_zero_hard_limit_starts_a_user_service_without_values_on_its_command_line(self):
        seen = {}

        def spawn(argv, **_):
            seen['argv'] = argv
            launch = Path(argv[-1])
            seen['mode'] = stat.S_IMODE(launch.stat().st_mode)
            seen['launch'] = json.loads(launch.read_text())
            return 7

        code = self.run_with(0, 0, lambda name: f'/usr/bin/{name}', spawn)
        self.assertEqual(code, 7, 'the service exit status is returned')
        self.assertEqual(seen['argv'][:6], ['/usr/bin/systemd-run', '--user', '--wait', '--pipe', '--collect', '--quiet'])
        self.assertNotIn('fixture-secret-value', ' '.join(seen['argv']))
        self.assertEqual(seen['mode'], 0o600)
        self.assertEqual(seen['launch']['argv'], ['cargo', 'test'])
        self.assertEqual(seen['launch']['cwd'], str(Path.cwd()))
        self.assertEqual(seen['launch']['env']['FIXTURE_SECRET'], 'fixture-secret-value')
        self.assertNotIn('INVOCATION_ID', seen['launch']['env'], "systemd's own values describe the caller's unit")
        self.assertEqual(list(Path(self.temp.name).iterdir()), [], 'the launch file never outlives the run')

    def test_without_systemd_run_the_command_is_refused_with_the_reason(self):
        code = self.run_with(0, 0, lambda _: None, lambda *_args, **_kwargs: self.fail('nothing may start'))
        self.assertEqual(code, 2)
        self.assertIn('hard limit is 0', ''.join(self.messages))
        self.assertIn('systemd-run is unavailable', ''.join(self.messages))

    def test_a_launch_file_readable_by_others_is_refused(self):
        path = Path(self.temp.name) / 'launch.json'
        path.write_text(json.dumps({'argv': ['true'], 'cwd': '/', 'env': {}}))
        path.chmod(0o644)
        with self.assertRaises(PermissionError):
            limits.read_launch(path)
        self.assertTrue(path.exists())


@unittest.skipUnless(sys.platform.startswith('linux'), 'RLIMIT_RTTIME is Linux-only')
class KernelTests(unittest.TestCase):
    """Live reproduction: the kernel, not a process, sends the SIGKILL."""

    def probe(self, *prefix):
        return subprocess.run([*prefix, sys.executable, '-I', str(SCRIPT), 'probe'], capture_output=True, text=True, timeout=60)

    def test_the_probe_dies_only_under_a_zero_hard_limit(self):
        result = self.probe()
        if limits.hazard(limits.rttime_limits()):
            self.assertEqual(result.returncode, -9, result.stdout + result.stderr)
        else:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        if shutil.which('prlimit'):
            zero = self.probe('prlimit', '--rttime=0:0')
            self.assertEqual(zero.returncode, -9, 'lowering the hard limit to 0 reproduces the kill')

    @unittest.skipUnless(user_systemd_available(), 'needs a systemd user manager')
    def test_run_survives_an_inherited_zero_hard_limit(self):
        result = subprocess.run(['prlimit', '--rttime=0:0', sys.executable, '-I', str(SCRIPT), 'run', '--',
                                 sys.executable, '-I', str(SCRIPT), 'probe'], capture_output=True, text=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('was not killed', result.stdout)


if __name__ == '__main__':
    unittest.main()
