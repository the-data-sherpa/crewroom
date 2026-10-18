"""Release accounting tests; no runner, provider or user configuration is started."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release_suite', Path(__file__).with_name('run-offline-suite.py'))
suite = importlib.util.module_from_spec(spec)
spec.loader.exec_module(suite)


class ReleaseAccounting(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads(suite.MANIFEST.read_text())

    def test_all_specified_ids_have_a_nonempty_mapping(self):
        suite.validate(self.manifest)
        self.manifest['cases'].pop()
        with self.assertRaisesRegex(ValueError, 'missing'): suite.validate(self.manifest)

    def test_successful_commands_do_not_pass_missing_coverage(self):
        self.manifest['cases'][0]['coverage'] = 'partial'
        receipts = {job: {'status': 'passed', 'identity': suite.receipt_identity(self.manifest, job, 'candidate')} for job in self.manifest['jobs']}
        result = suite.summarize(self.manifest, receipts, 'candidate')
        self.assertFalse(result['passed'])
        self.assertTrue(any(case['status'] == 'coverage-gap' for case in result['cases']))

    def test_different_candidate_and_failed_or_absent_receipts_never_pass(self):
        manifest = copy.deepcopy(self.manifest)
        for group in ['cases', 'scenarios']:
            for item in manifest[group]: item['coverage'] = 'mapped'
        receipts = {job: {'status': 'passed', 'identity': suite.receipt_identity(manifest, job, 'candidate')} for job in manifest['jobs']}
        self.assertTrue(suite.summarize(manifest, receipts, 'candidate')['passed'])
        self.assertFalse(suite.summarize(manifest, receipts, 'different-source')['passed'])
        receipts['domain']['status'] = 'failed'
        self.assertFalse(suite.summarize(manifest, receipts, 'candidate')['passed'])
        del receipts['domain']
        self.assertFalse(suite.summarize(manifest, receipts, 'candidate')['passed'])

    def test_changed_job_or_manifest_never_reuses_a_receipt(self):
        receipt = {'status': 'passed', 'identity': suite.receipt_identity(self.manifest, 'domain', 'candidate')}
        self.assertEqual(suite.receipt_status(self.manifest, {'domain': receipt}, 'domain', 'candidate'), 'passed')
        for change in ('command', 'coverage', 'fixture'):
            altered = copy.deepcopy(self.manifest)
            if change == 'command': altered['jobs']['domain']['command'] = ['false']
            elif change == 'coverage': altered['cases'][0]['note'] += ' Changed acceptance.'
            else: altered['cases'][0]['tests'].append('docs/testing/fixtures/new.json')
            self.assertEqual(suite.receipt_status(altered, {'domain': receipt}, 'domain', 'candidate'), 'not-run-on-this-candidate')

    def test_fixture_contents_are_hashed_and_candidate_records_survive(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); fixture = root / 'docs/testing/fixtures/catalog.json'
            fixture.parent.mkdir(parents=True); fixture.write_text('old')
            def git(args, **kwargs):
                return 'docs/testing/fixtures/catalog.json\n' if args[1] == 'ls-files' else 'revision\n'
            with patch.object(suite, 'PROJECT', root), patch.object(suite.subprocess, 'check_output', side_effect=git), patch.object(suite, 'runtime_identity', return_value={'node': 'one'}) as runtime:
                old = suite.source_identity(); first = suite.save_candidate(root, old, self.manifest)
                fixture.write_text('new'); changed = suite.source_identity()
                self.assertNotEqual(old['sha256'], changed['sha256'])
                second = suite.save_candidate(root, changed, self.manifest)
                self.assertNotEqual(first, second)
                self.assertEqual(json.loads((root / first).read_text())['source'], old)
                self.assertEqual(suite.save_candidate(root, changed, self.manifest), second)
                runtime.return_value = {'node': 'two'}
                self.assertNotEqual(suite.source_identity()['sha256'], changed['sha256'], 'Runtime changes invalidate reused command receipts even with identical source.')

    def test_cleanup_refuses_a_foreign_runtime_directory(self):
        # Exercise the real guarded harness cleanup without copying or launching OMP.
        from omp_offline_fixture import Sandbox
        with tempfile.TemporaryDirectory() as path:
            root = Path(path); foreign = root / 'foreign'; foreign.mkdir()
            sentinel = foreign / 'omp'; sentinel.write_text('foreign sentinel')
            (root / 'runtime').symlink_to(foreign, target_is_directory=True)
            sandbox = Sandbox.__new__(Sandbox); sandbox.root = root
            with self.assertRaisesRegex(RuntimeError, 'refusing automatic cleanup'): sandbox.close()
            self.assertEqual(sentinel.read_text(), 'foreign sentinel')


if __name__ == '__main__': unittest.main()
