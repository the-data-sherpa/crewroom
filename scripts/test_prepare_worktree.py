"""FOR-59: the fresh-worktree preparation step links installed fixtures idempotently and never creates tracked state."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import prepare_worktree as prep


def checkout(path, ignore='.crewroom/\n'):
    path.mkdir()
    subprocess.run(['git', 'init', '-q', str(path)], check=True)
    (path / '.gitignore').write_text(ignore)
    return path


def install(root):
    for item in prep.FIXTURES:
        marker = root / item.path / item.marker
        marker.parent.mkdir(parents=True)
        marker.write_text('fixture\n')


class PrepareWorktreeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='crewroom-prepare-test-')
        base = Path(self.temp.name).resolve()
        self.source, self.worktree = checkout(base / 'source'), checkout(base / 'worktree')
        install(self.source)
        self.platform = patch.object(sys, 'platform', 'linux')
        self.platform.start()
        self.lines = []

    def tearDown(self):
        self.platform.stop()
        self.temp.cleanup()

    def prepare(self, **kwargs):
        return prep.prepare(self.worktree, self.source, out=self.lines.append, **kwargs)

    def test_links_installed_fixtures_and_a_second_run_changes_nothing(self):
        self.assertEqual(self.prepare(), [])
        for item in prep.FIXTURES:
            target = self.worktree / item.path
            self.assertTrue(target.is_symlink())
            self.assertEqual(target.resolve(), (self.source / item.path).resolve())
            self.assertTrue(item.ready(self.worktree))
        self.lines.clear()
        self.assertEqual(self.prepare(), [])
        self.assertTrue(all(line.startswith('ready') for line in self.lines), self.lines)
        status = subprocess.run(['git', '-C', str(self.worktree), 'status', '--porcelain', '--ignored=no'],
                                capture_output=True, text=True, check=True).stdout
        self.assertNotIn('.crewroom', status)

    def test_a_stale_link_is_replaced_and_remove_keeps_installed_copies(self):
        stale = self.worktree / prep.FIXTURES[0].path
        stale.parent.mkdir(parents=True)
        stale.symlink_to(self.worktree / 'gone', target_is_directory=True)
        self.assertEqual(self.prepare(), [])
        self.assertEqual(stale.resolve(), (self.source / prep.FIXTURES[0].path).resolve())
        prep.remove(self.worktree, self.source, out=self.lines.append)
        self.assertFalse(any((self.worktree / item.path).exists() for item in prep.FIXTURES))
        self.assertTrue(all(item.ready(self.source) for item in prep.FIXTURES))

    def test_missing_fixtures_produce_one_actionable_message_each(self):
        failures = prep.prepare(self.worktree, None, out=self.lines.append)
        self.assertEqual(len(failures), len(prep.FIXTURES))
        for message in failures:
            self.assertIn(prep.STEP, message)
            self.assertIn('--install-missing', message)
        self.assertFalse((self.worktree / '.crewroom').exists())

    def test_refuses_to_create_fixture_state_that_git_would_track(self):
        (self.worktree / '.gitignore').write_text('')
        failures = self.prepare()
        self.assertEqual(len(failures), len(prep.FIXTURES))
        self.assertTrue(all('not ignored' in failure for failure in failures))
        self.assertFalse((self.worktree / '.crewroom').exists())

    def test_an_incomplete_real_directory_is_left_for_the_operator(self):
        partial = self.worktree / prep.FIXTURES[1].path
        partial.mkdir(parents=True)
        failures = self.prepare()
        self.assertEqual(len(failures), 1)
        self.assertIn('incomplete', failures[0])
        self.assertFalse(partial.is_symlink())

    def test_the_omp_fixture_is_only_required_on_linux(self):
        with patch.object(sys, 'platform', 'darwin'):
            names = [item.name for item, _, _ in prep.plan(self.worktree, self.source)]
        self.assertEqual(names, ['Rust Mail test runtime'])

    def test_a_zero_rttime_hard_limit_is_named_with_the_way_around_it(self):
        self.assertIsNone(prep.limits_note((prep.system_limits.resource.RLIM_INFINITY,) * 2))
        note = prep.limits_note((0, 0))
        self.assertTrue(note.startswith('limits'), note)
        self.assertIn('system_limits.py run', note)


if __name__ == '__main__':
    unittest.main()
