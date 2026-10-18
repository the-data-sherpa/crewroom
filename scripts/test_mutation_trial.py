#!/usr/bin/env python3
"""Focused guards for the fixed mutation helper, not mutation outcome evidence."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('mutation_trial', Path(__file__).with_name('mutation-trial.py'))
trial = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trial)


class Guards(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='crewroom-mutation-guard-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.target = self.root / trial.TARGET
        self.target.parent.mkdir(parents=True)
        self.original = (trial.REPO / trial.TARGET).read_bytes()
        self.target.write_bytes(self.original)
        self.config, self.patches = trial.load_config()

    def test_five_exact_patches_restore_byte_for_byte(self):
        self.assertEqual(len(self.patches), 5)
        for mutation in self.patches:
            trial.apply_patch(self.root, mutation)
            self.assertEqual(trial.sha(self.target), mutation['postimageSha256'])
            trial.restore(self.root, self.original)
            self.assertEqual(self.target.read_bytes(), self.original)

    def test_unexpected_preimage_or_ambiguous_replacement_refused(self):
        self.target.write_bytes(self.original + b'\n')
        with self.assertRaisesRegex(ValueError, 'preimage'):
            trial.apply_patch(self.root, self.patches[0])
        self.target.write_bytes(self.original)
        with self.assertRaisesRegex(ValueError, 'exactly one match'):
            trial.apply_patch(self.root, dict(self.patches[0], old='unlikely source'))

    def test_paths_symlinks_and_postimage_refused(self):
        with self.assertRaisesRegex(ValueError, 'escapes'):
            trial.apply_patch(self.root, dict(self.patches[0], path='../other'))
        with self.assertRaisesRegex(ValueError, 'postimage'):
            trial.apply_patch(self.root, dict(self.patches[0], new='unreviewed'))
        self.target.unlink()
        self.target.symlink_to(trial.REPO / trial.TARGET)
        with self.assertRaisesRegex(ValueError, 'escapes'):
            trial.apply_patch(self.root, self.patches[0])
        self.assertEqual((trial.REPO / trial.TARGET).read_bytes(), self.original)

    def test_exact_nonzero_selection_and_behavioral_classification(self):
        good = 'ok 1 - ' + trial.NAME + '\n# tests 1\n# {"event":"native-fixture-exit","exitCode":0,"signal":null}\n'
        bad = good.replace('ok 1', 'not ok 1') + "  error: 'Missing expected rejection: artifact hash mismatch must reject'\n"
        self.assertEqual(trial.classify({'exitCode': 0}, {'exitCode': 0}, good), 'survived')
        self.assertEqual(trial.classify({'exitCode': 0}, {'exitCode': 1}, bad, 'M1'), 'killed')
        self.assertEqual(trial.classify({'exitCode': 0}, {'exitCode': 1}, bad.replace('Missing expected rejection', 'broken infrastructure')), 'infrastructure_error')
        for text in (good.replace(trial.NAME, trial.NAME + ' # SKIP'), good + '# cancelled 1\n', '# tests 0\n', good.replace('# tests 1', '# tests 2'), good.replace(trial.NAME, 'other')):
            self.assertEqual(trial.classify({'exitCode': 0}, {'exitCode': 0}, text), 'infrastructure_error')
        self.assertEqual(trial.classify({'exitCode': 0}, {'exitCode': 1}, bad, 'M4'), 'infrastructure_error')
        self.assertEqual(trial.classify({'exitCode': 101}), 'unviable')
        self.assertEqual(trial.classify({'exitCode': -15, 'timedOut': True}), 'timeout')
        self.assertEqual(trial.classify({'exitCode': 0}, {'exitCode': 1, 'timedOut': True}), 'timeout')
        self.assertEqual(trial.classify({'exitCode': 0}, {'exitCode': 0, 'remaining': [1]}, good), 'infrastructure_error')

    def test_config_cannot_supply_other_paths_or_budget(self):
        directory = self.root / 'config'
        directory.mkdir()
        for path in trial.CONFIG.parent.glob('*.json'):
            (directory / path.name).write_bytes(path.read_bytes())
        config = json.loads((directory / 'config.json').read_text())
        config['patches'][0]['file'] = '../M1.json'
        (directory / 'config.json').write_text(json.dumps(config))
        with self.assertRaisesRegex(ValueError, 'fixed local'):
            trial.load_config(directory / 'config.json')
        config['workers'] = 2
        (directory / 'config.json').write_text(json.dumps(config))
        with self.assertRaisesRegex(ValueError, 'fixed FOR-13'):
            trial.load_config(directory / 'config.json')

    def test_existing_output_is_not_overwritten(self):
        marker = self.root / 'keep'
        marker.write_text('unchanged')
        with self.assertRaisesRegex(ValueError, 'already exists'):
            trial.run(self.root, 'node')
        self.assertEqual(marker.read_text(), 'unchanged')

    def test_build_recreates_local_binary_and_binds_its_hash(self):
        obj = object.__new__(trial.Trial)
        obj.checkout = self.root
        binary = self.root / 'target/debug/examples/repository-fixture'
        process = self.root / 'target/debug/crewroom-process'
        binary.parent.mkdir(parents=True)
        binary.write_text('stale-main-copy')
        process.write_text('process')
        def build(_label, argv, _timeout):
            self.assertFalse(binary.exists())
            self.assertIn('--locked', argv)
            binary.write_text('rebuilt-mutant')
            return {'exitCode': 0, 'timedOut': False}
        obj.command = build
        report = obj.build('M1-build')
        self.assertEqual(report['binaries']['target/debug/examples/repository-fixture'], trial.sha(binary))

    def test_retained_timeout_logs_and_owned_cleanup(self):
        # A real disposable sleep process demonstrates timeout/log retention. The
        # scoped cleanup implementation itself is covered by test_processes tests.
        obj = object.__new__(trial.Trial)
        obj.directory = self.root
        obj.checkout = self.root
        obj.deadline = __import__('time').time() + 10
        obj.commands = []
        obj.env = dict(os.environ)
        child_holder = {}
        real_popen = trial.subprocess.Popen
        def launch(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            child_holder['child'] = child
            return child
        def cleanup():
            child_holder['child'].terminate()
            return {'signals': [{'signal': 'SIGTERM'}], 'remaining': []}
        obj.cleanup = cleanup
        with patch.object(trial.subprocess, 'Popen', side_effect=launch):
            result = obj.command('bounded', ['python3', '-u', '-c', 'import time; print("retained failure"); time.sleep(10)'], .1)
        self.assertTrue(result['timedOut'])
        self.assertIn('retained failure', (self.root / 'bounded.stdout').read_text())
        self.assertEqual(result['remaining'], [])
        self.assertEqual(result['stdout']['sha256'], trial.sha(self.root / 'bounded.stdout'))


if __name__ == '__main__':
    unittest.main()
