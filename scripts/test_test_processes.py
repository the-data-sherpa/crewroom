"""Disposable process checks for the operator's test cleanup command."""
import argparse
from contextlib import redirect_stderr
import io
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
import uuid

import test_processes as cleanup

SLEEP = 'import time; print("ready", flush=True); time.sleep(120)'
IGNORE = 'import signal; signal.signal(signal.SIGTERM, signal.SIG_IGN); ' + SLEEP


class CleanupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='crewroom-cleanup-test-')
        self.repo = Path(self.temp.name).resolve()
        self.probe = cleanup.Processes()
        self.children = []
        self.orphans = []

    def tearDown(self):
        for pid, identity in self.orphans:
            current = self.probe.read(pid)
            if current and current['identity'] == identity:
                os.kill(pid, signal.SIGKILL)
        for p in self.children:
            if p.poll() is None:
                p.kill()
            p.wait(timeout=5)
            if p.stdout:
                p.stdout.close()
        self.temp.cleanup()

    def record(self):
        ident = str(uuid.uuid4())
        path = cleanup.registry(self.repo, create=True) / (ident + '.json')
        path.write_text(json.dumps({'id': ident, 'repo': str(self.repo), 'roots': []}))
        path.chmod(0o600)
        return ident

    def env(self, ident):
        return dict(os.environ, **{cleanup.RUN_KEY: ident, cleanup.REPO_KEY: str(self.repo)})

    def spawn(self, code=SLEEP, env=None, args=()):
        p = subprocess.Popen([sys.executable, '-u', '-c', code, *args], env=env,
                             stdout=subprocess.PIPE, text=True, start_new_session=True)
        self.children.append(p)
        self.assertEqual(p.stdout.readline().strip(), 'ready')
        return p

    def orphan(self, pid):
        p = self.probe.read(pid)
        self.assertIsNotNone(p)
        self.orphans.append((pid, p['identity']))

    def test_process_reader_and_read_only_list(self):
        p = self.probe.read(os.getpid())
        self.assertIsNotNone(p)
        self.assertEqual(p['pid'], os.getpid())
        self.assertEqual(p['identity'], self.probe.read(os.getpid())['identity'])
        found, _ = cleanup.discover(self.probe, self.repo)
        self.assertEqual(found, {})
        self.assertFalse((self.repo / '.crewroom').exists())

    def test_registered_run_stops_only_its_processes_without_exposing_secrets(self):
        ident = self.record()
        env = self.env(ident)
        env['SECRET'] = 'never-print-this'
        target = self.spawn(env=env, args=('private-argument',))
        unrelated = self.spawn(env=dict(os.environ))
        other_run = self.spawn(env=self.env(self.record()))
        found, _ = cleanup.discover(self.probe, self.repo, run_id=ident)
        self.assertEqual(set(found), {target.pid})
        report = json.dumps(cleanup.summary(found))
        self.assertNotIn('private-argument', report)
        self.assertNotIn('never-print-this', report)
        events, remaining = cleanup.stop(self.probe, self.repo, [], ident, .5, False)
        self.assertFalse(remaining)
        self.assertEqual(events, [{'pid': target.pid, 'signal': 'SIGTERM'}])
        target.wait(timeout=3)
        self.assertIsNone(unrelated.poll())
        self.assertIsNone(other_run.poll())

    def test_force_is_required_for_a_term_ignoring_process(self):
        ident = self.record()
        child = self.spawn(IGNORE, env=self.env(ident))
        events, remaining = cleanup.stop(self.probe, self.repo, [], ident, .2, False)
        self.assertIn(child.pid, remaining)
        self.assertEqual([e['signal'] for e in events], ['SIGTERM'])
        events, remaining = cleanup.stop(self.probe, self.repo, [], ident, .5, True)
        self.assertFalse(remaining)
        self.assertEqual([e['signal'] for e in events], ['SIGTERM', 'SIGKILL'])
        child.wait(timeout=3)

    def test_explicit_fixture_path_and_prefix_boundary(self):
        root = self.repo / 'fixture'
        root.mkdir()
        target = self.spawn(args=('--data-dir=' + str(root / 'data'),))
        sibling = self.spawn(args=(str(self.repo / 'fixture-unrelated'),))
        found, _ = cleanup.discover(self.probe, self.repo)
        self.assertNotIn(target.pid, found)
        root = cleanup.fixture_root(root, self.repo)
        events, remaining = cleanup.stop(self.probe, self.repo, [root], None, .5, False)
        self.assertFalse(remaining)
        self.assertEqual([e['pid'] for e in events], [target.pid])
        target.wait(timeout=3)
        self.assertIsNone(sibling.poll())

    def test_known_test_results_and_sanitized_fixture_environment(self):
        root = self.repo / '.crewroom' / 'test-results' / 'fixture'
        root.mkdir(parents=True)
        target = self.spawn(env={'STORAGE_ROOT': str(root / 'archive')})
        # Arbitrary environment paths do not confer test ownership.
        unrelated = self.spawn(env={'SOME_UNRELATED_PATH': str(root)})
        found, _ = cleanup.discover(self.probe, self.repo)
        self.assertEqual(set(found), {target.pid})
        _, remaining = cleanup.stop(self.probe, self.repo, [], None, .5, False)
        self.assertFalse(remaining)
        target.wait(timeout=3)
        self.assertIsNone(unrelated.poll())

    def test_detached_child_remains_discoverable_after_wrapper_exits(self):
        pidfile = self.repo / 'child.pid'
        code = ('import subprocess, sys, pathlib; '
                f'p = subprocess.Popen([sys.executable, "-u", "-c", {SLEEP!r}], '
                'start_new_session=True, stdout=subprocess.DEVNULL); '
                f'pathlib.Path({str(pidfile)!r}).write_text(str(p.pid))')
        args = argparse.Namespace(command=['--', sys.executable, '-c', code], root=[], label='detached')
        with redirect_stderr(io.StringIO()):
            self.assertEqual(cleanup.run_command(args, self.repo), 0)
        pid = int(pidfile.read_text())
        # exec may still be settling just after the parent exits.
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            found, _ = cleanup.discover(self.probe, self.repo)
            if pid in found:
                break
            time.sleep(.02)
        self.orphan(pid)
        self.assertIn(pid, found)
        self.assertEqual(len(cleanup.load_runs(self.repo)), 1)
        _, remaining = cleanup.stop(self.probe, self.repo, [], None, .5, False)
        self.assertFalse(remaining)
        self.assertIsNone(self.probe.read(pid))

    def test_scrubbed_descendant_is_retained_after_parent_exits(self):
        ident = self.record()
        code = ('import subprocess, sys, time; '
                f'p = subprocess.Popen([sys.executable, "-u", "-c", {IGNORE!r}], env={{}}); '
                'print(p.pid, flush=True); time.sleep(120)')
        parent = subprocess.Popen([sys.executable, '-u', '-c', code], env=self.env(ident),
                                  stdout=subprocess.PIPE, text=True, start_new_session=True)
        self.children.append(parent)
        lines = [parent.stdout.readline().strip(), parent.stdout.readline().strip()]
        self.assertIn('ready', lines)
        pid = int(next(line for line in lines if line != 'ready'))
        self.orphan(pid)
        found, _ = cleanup.discover(self.probe, self.repo, run_id=ident)
        self.assertEqual(set(found), {parent.pid, pid})
        self.assertTrue(found[pid][1].startswith('descendant'))
        _, remaining = cleanup.stop(self.probe, self.repo, [], ident, .5, True)
        self.assertFalse(remaining)
        parent.wait(timeout=3)
        self.assertIsNone(self.probe.read(pid))

    def test_default_app_and_pilot_override_even_a_valid_test_marker(self):
        ident = self.record()
        children = [self.spawn(env=self.env(ident), args=('--data-dir', str(root)))
                    for root in cleanup.protected_roots(self.repo)]
        events, remaining = cleanup.stop(self.probe, self.repo, [], ident, .1, True)
        self.assertEqual(events, [])
        self.assertFalse(remaining)
        found, protected = cleanup.discover(self.probe, self.repo)
        self.assertFalse(found)
        self.assertTrue(all(c.pid in protected and c.poll() is None for c in children))

    def test_refuse_wide_or_protected_roots_and_symlinks(self):
        for path in ['/', tempfile.gettempdir(), '/tmp', self.repo,
                     self.repo / '.crewroom' / 'rust', self.repo / '.crewroom' / 'pilot']:
            with self.subTest(path=path), self.assertRaises(ValueError):
                cleanup.fixture_root(path, self.repo)
        link = self.repo / 'alias'
        link.symlink_to(self.repo / '.crewroom' / 'rust', target_is_directory=True)
        with self.assertRaises(ValueError):
            cleanup.fixture_root(link, self.repo)

    def test_untrusted_or_mismatched_registry_refuses_cleanup(self):
        ident = self.record()
        root = cleanup.registry(self.repo)
        path = root / (ident + '.json')
        path.chmod(0o644)
        with self.assertRaises(ValueError):
            cleanup.load_runs(self.repo)
        path.chmod(0o600)
        data = json.loads(path.read_text())
        data['repo'] = '/different/checkout'
        path.write_text(json.dumps(data))
        with self.assertRaises(ValueError):
            cleanup.load_runs(self.repo)
        path.unlink()
        target = self.repo / 'record'
        target.write_text(json.dumps(data))
        target.chmod(0o600)
        path.symlink_to(target)
        with self.assertRaises(ValueError):
            cleanup.load_runs(self.repo)

    def test_reused_pid_is_never_signalled(self):
        ident = self.record()
        pid = 99999999
        observed = {'pid': pid, 'parent': 1, 'born': 'one', 'executable': '/bin/sleep',
                    'args': ['sleep'], 'env': {cleanup.RUN_KEY: ident, cleanup.REPO_KEY: str(self.repo)},
                    'identity': (pid, 'one', '/bin/sleep', 'hash')}
        replaced = dict(observed, identity=(pid, 'two', '/bin/sleep', 'hash'), env={})
        # The snapshot selected the old process; the immediate identity check
        # sees a different birth. All later scans see the unrelated replacement.
        class Reuse:
            def __init__(self):
                self.reads = 0
            def pids(self):
                return [pid]
            def read(self, _pid):
                self.reads += 1
                return observed if self.reads == 1 else replaced
        with patch.object(cleanup.os, 'kill') as kill:
            events, remaining = cleanup.stop(Reuse(), self.repo, [], ident, .1, True)
        kill.assert_not_called()
        self.assertEqual(events, [])
        self.assertFalse(remaining)

    def test_wrapper_preserves_exit_code_and_private_record(self):
        args = argparse.Namespace(command=[sys.executable, '-c', 'raise SystemExit(7)'],
                                  root=[], label=None)
        with redirect_stderr(io.StringIO()):
            self.assertEqual(cleanup.run_command(args, self.repo), 7)
        root = cleanup.registry(self.repo)
        self.assertEqual(root.stat().st_mode & 0o777, 0o700)
        self.assertEqual(next(root.glob('*.json')).stat().st_mode & 0o777, 0o600)

    def test_cli_preview_stop_force_and_exit_codes(self):
        # A byte-for-byte script copy selects a disposable checkout registry.
        script = self.repo / 'scripts' / 'test_processes.py'
        script.parent.mkdir()
        shutil.copyfile(cleanup.__file__, script)
        def cli(*args):
            return subprocess.run([sys.executable, str(script), *args],
                                  text=True, capture_output=True, timeout=10)
        ident = self.record()
        child = self.spawn(IGNORE, env=self.env(ident))
        preview = cli('list', '--run', ident, '--json')
        self.assertEqual(preview.returncode, 0, preview.stderr)
        self.assertEqual([p['pid'] for p in json.loads(preview.stdout)['candidates']], [child.pid])
        self.assertIsNone(child.poll())
        stopped = cli('stop', '--run', ident, '--timeout', '.1', '--json')
        self.assertEqual(stopped.returncode, 1, stopped.stderr)
        self.assertEqual(len(json.loads(stopped.stdout)['remaining']), 1)
        forced = cli('stop', '--run', ident, '--timeout', '.2', '--force', '--json')
        self.assertEqual(forced.returncode, 0, forced.stderr)
        self.assertEqual(json.loads(forced.stdout)['remaining'], [])
        child.wait(timeout=3)
        self.assertEqual(cli('list', '--root', '/').returncode, 2)
        self.assertEqual(cli('list', '--run', str(uuid.uuid4())).returncode, 2)
        failed = cli('run', '--', sys.executable, '-c', 'raise SystemExit(7)')
        self.assertEqual(failed.returncode, 7, failed.stderr)


if __name__ == '__main__':
    unittest.main()
