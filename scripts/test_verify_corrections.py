"""Public correction CLI guards. Synthetic records are not FOR-13 semantic evidence."""
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('verify-corrections.py')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False, allow_nan=False).encode()


class CorrectionGuards(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='crewroom-correction-guard-')
        self.root = Path(self.temporary.name)
        self.repo = self.root / 'repo'; self.repo.mkdir()
        (self.repo / 'scripts').mkdir()
        shutil.copyfile(SCRIPT, self.repo / 'scripts/verify-corrections.py')
        self.evidence = self.root / 'evidence'; self.evidence.mkdir()
        self.records = self.repo / 'records'; self.records.mkdir()
        self.record_path = self.records / 'guard.json'
        (self.repo / 'source.rs').write_text('saved replay before current state\n')
        (self.repo / 'bad.rs').write_text('mutable state before saved replay\n')
        (self.repo / 'state-link').symlink_to(self.evidence)
        self.git('init', '-q'); self.git('add', '.')
        self.git('-c', 'user.name=Guard', '-c', 'user.email=guard@example.invalid', 'commit', '-qm', 'Guard only')
        self.commit = self.git('rev-parse', 'HEAD').strip()
        source = self.git_ref('source.rs'); bad = self.git_ref('bad.rs')
        check = self.private('fixture.ts', b'assert saved replay; assert changed input conflicts\n')
        one = self.private('one.md', b'First independent incident\n')
        two = self.private('two.md', b'Second independent incident\n')
        for ref in (one, two):
            ref['excerpt'] = {'startLine': 1, 'endLine': 1, 'sha256': ref['sha256']}
        self.proof = {
            'schemaVersion': 1, 'kind': 'correction_enforcement', 'historicalBadRevision': self.commit,
            'currentRevision': self.commit, 'historicalSourceSha256': bad['sha256'],
            'currentSourceSha256': source['sha256'], 'fixtureSha256': check['sha256'], 'runs': [],
            'cleanup': {'status': 'complete', 'receipt': self.private('cleanup.json', b'{"ownedChildren":0}\n')},
            'limits': ['Synthetic guard fixture only'],
        }
        for case in ('historical_bad', 'legitimate', 'payload_conflict'):
            self.proof['runs'].append({
                'case': case, 'selected': 1, 'executed': 1, 'exitCode': 1 if case == 'historical_bad' else 0,
                'outcome': 'expected_failure' if case == 'historical_bad' else 'passed', 'assertion': case,
                'sourceSha256': bad['sha256'] if case == 'historical_bad' else source['sha256'],
                'binary': self.private(case + '.bin', case.encode()),
                'output': self.private(case + '.log', ('assertion: ' + case).encode()),
            })
        self.record = {
            'schemaVersion': 1, 'id': 'guard-replay', 'issue': 'FOR-13', 'repository': 'the-data-sherpa/crewroom',
            'invariant': 'Replay precedes mutable validation: café', 'scope': 'Authenticated matching requests',
            'occurrences': [{'id': identity, 'observedAt': None, 'affectedRevision': self.commit,
                             'behavior': 'Mutable validation refused replay', 'independence': identity,
                             'source': ref} for identity, ref in [('incident-one', one), ('incident-two', two)]],
            'remedy': {'primaryLayer': 'domain_api', 'supportingLayers': ['behavioral_test'],
                       'rationale': 'The boundary owns receipt ordering',
                       'strongerLayersConsidered': [{'layer': 'types', 'reason': 'Ordering depends on persisted state'}],
                       'existingEnforcementInspection': 'Existing receipt interface and regression'},
            'enforcement': {'claim': 'mechanical', 'source': source, 'symbol': 'replay', 'check': check,
                            'historicalBadSource': bad, 'legitimateBehavior': 'Saved receipt despite later edit'},
            'verification': {'proof': None, 'limits': ['Guard fixture']},
            'disposition': {'kind': 'existing_enforcement_reused', 'links': []},
            'independentReview': None, 'validationOutputs': [],
        }
        self.bind_proof(); self.bind_review()

    def tearDown(self):
        self.temporary.cleanup()

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], text=True)

    def git_ref(self, path):
        return {'kind': 'git', 'revision': self.commit, 'path': path, 'sha256': sha((self.repo / path).read_bytes())}

    def private(self, path, data):
        target = self.evidence / path; target.parent.mkdir(parents=True, exist_ok=True); target.write_bytes(data)
        return {'kind': 'private', 'path': path, 'sha256': sha(data)}

    def bind_proof(self):
        ref = self.private('proof.json', canonical({'correctionProof': self.proof}))
        ref['jsonPointer'] = {'pointer': '/correctionProof', 'sha256': sha(canonical(self.proof))}
        self.record['verification']['proof'] = ref

    def bind_review(self):
        proof = self.record['verification']['proof']
        self.review = {
            'schemaVersion': 1, 'kind': 'correction_claim_review', 'classId': self.record['id'],
            'claimSha256': sha(canonical({key: value for key, value in self.record.items()
                                         if key not in {'independentReview', 'validationOutputs'}})),
            'reviewer': {'identity': 'Synthetic guard reviewer', 'independent': True},
            'reviewedAt': '2026-10-13T00:00:00Z',
            'occurrenceHashes': [{'id': item['id'], 'sourceSha256': item['source']['sha256'],
                                  'selectionSha256': item['source']['excerpt']['sha256']} for item in self.record['occurrences']],
            'proofSha256': proof['jsonPointer']['sha256'] if proof else None,
            'recurrence': 'confirmed' if len(self.record['occurrences']) >= 2 else 'one_off',
            'enforcement': 'confirmed' if self.record['enforcement']['claim'] == 'mechanical' else 'not_claimed',
            'rationale': 'Synthetic assertions only', 'limits': ['Not an actual independent review'],
        }
        self.record['independentReview'] = self.private('review.json', canonical(self.review))

    def cli(self, expected, *, write=True, extra=()):
        if write:
            self.record_path.write_text(json.dumps(self.record, ensure_ascii=False))
        result = subprocess.run([sys.executable, str(self.repo / 'scripts/verify-corrections.py'), 'check',
                                 str(self.records), '--evidence-root', str(self.evidence), *extra], capture_output=True, text=True)
        self.assertEqual(result.returncode, {'verified': 0, 'incomplete': 2, 'invalid': 3}[expected], result.stdout + result.stderr)
        body = json.loads(result.stdout); self.assertEqual(body['outcome'], expected)
        return body

    def test_valid_distinct_occurrences_resolve_and_only_requested_receipt_is_written(self):
        self.record_path.write_text(json.dumps(self.record))
        before = {str(p): sha(p.read_bytes()) for p in self.root.rglob('*') if p.is_file()}
        output = self.root / 'receipt.json'
        result = self.cli('verified', write=False, extra=('--output', str(output)))
        self.assertTrue(result['for13Gate'])
        self.assertEqual(result['records'][0]['recurrenceAssessment']['distinctResolvedOccurrences'], 2)
        self.assertEqual(json.loads(output.read_bytes()), result)
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)
        after = {str(p): sha(p.read_bytes()) for p in self.root.rglob('*') if p.is_file() and p != output}
        self.assertEqual(before, after)

    def test_duplicate_incidents_relabelled_sources_and_duplicate_classes_are_invalid(self):
        original = copy.deepcopy(self.record)
        self.record['occurrences'][1]['id'] = self.record['occurrences'][0]['id']; self.cli('invalid')
        self.record = copy.deepcopy(original)
        self.record['occurrences'][1]['source'] = self.record['occurrences'][0]['source']; self.cli('invalid')
        self.record = original
        (self.records / 'duplicate.json').write_text(json.dumps(original)); self.cli('invalid')

    def test_one_off_and_guidance_are_valid_without_becoming_recurring_enforcement(self):
        self.record['occurrences'].pop(); self.bind_review()
        result = self.cli('verified'); self.assertFalse(result['for13Gate'])
        self.assertEqual(result['records'][0]['recurrenceAssessment']['classification'], 'one_off')
        self.record['remedy']['primaryLayer'] = 'guidance'
        self.record['enforcement'].update(claim='proposed', source=None, symbol=None, check=None, historicalBadSource=None)
        self.record['verification']['proof'] = None
        self.record['disposition']['kind'] = 'judgment_guidance_proposed'; self.bind_review()
        result = self.cli('verified'); self.assertFalse(result['for13Gate'])
        self.assertEqual(result['records'][0]['enforcementProof']['classification'], 'not_claimed')
        self.record['disposition']['adopted'] = True; self.cli('invalid')

    def test_sources_missing_hash_mismatched_bad_extraction_or_git_symlink(self):
        original = copy.deepcopy(self.record)
        self.record['occurrences'][0]['source']['path'] = 'missing'; self.cli('incomplete')
        self.record = copy.deepcopy(original)
        self.record['occurrences'][0]['source']['sha256'] = '0' * 64; self.cli('invalid')
        self.record = copy.deepcopy(original)
        self.record['occurrences'][0]['source']['excerpt']['endLine'] = 2; self.cli('invalid')
        self.record = copy.deepcopy(original)
        self.record['verification']['proof']['jsonPointer']['pointer'] = '/absent'; self.cli('invalid')
        self.record = copy.deepcopy(original)
        self.record['enforcement']['source']['path'] = 'state-link'; self.cli('invalid')

    def test_lexical_and_symlink_escape_and_output_overwrite_are_refused(self):
        original = copy.deepcopy(self.record)
        self.record['occurrences'][0]['source']['path'] = '../outside'; self.cli('invalid')
        self.record = copy.deepcopy(original)
        (self.evidence / 'escape').symlink_to(self.repo)
        self.record['occurrences'][0]['source']['path'] = 'escape/source.rs'; self.cli('invalid')
        self.record = original
        output = self.root / 'existing'; output.write_text('preserve me')
        self.cli('invalid', extra=('--output', str(output)))
        self.assertEqual(output.read_text(), 'preserve me')

    def test_malformed_unknown_schema_duplicate_keys_and_nonfinite_values(self):
        original = copy.deepcopy(self.record)
        for key, value in [('schemaVersion', 2), ('schemaVersion', True), ('execute', 'touch unauthorized')]:
            self.record = copy.deepcopy(original); self.record[key] = value; self.cli('invalid')
        self.record = copy.deepcopy(original); self.record['remedy']['primaryLayer'] = []; self.cli('invalid')
        for raw in ('{', '{"schemaVersion":1,"schemaVersion":1}', '{"number":NaN}', '{"number":Infinity}', '{"number":1e999}'):
            self.record_path.write_text(raw); self.cli('invalid', write=False)
        self.record = original; self.record['scope'] = '\ud800'
        self.record_path.write_text(json.dumps(self.record)); self.cli('invalid', write=False)

    def test_missing_proof_zero_selection_wrong_source_and_missing_behavior(self):
        original = copy.deepcopy(self.record)
        self.record['verification']['proof'] = None; self.bind_review(); self.cli('incomplete')
        self.record = original
        self.proof['runs'][0]['selected'] = self.proof['runs'][0]['executed'] = 0
        self.bind_proof(); self.bind_review(); self.cli('invalid')
        self.proof['runs'][0]['selected'] = self.proof['runs'][0]['executed'] = 1
        self.proof['runs'][0]['sourceSha256'] = '0' * 64
        self.bind_proof(); self.bind_review(); self.cli('invalid')
        self.proof['runs'].pop(0); self.bind_proof(); self.bind_review(); self.cli('incomplete')

    def test_review_binds_claim_and_resolved_hashes_and_excludes_only_reference_fields(self):
        original = copy.deepcopy(self.record)
        self.record['invariant'] += ' changed'; result = self.cli('incomplete')
        self.assertEqual(result['records'][0]['recurrenceAssessment']['review'], 'stale')
        self.record = copy.deepcopy(original)
        self.record['validationOutputs'] = [self.private('prior-validation.json', b'{"fixture":true}')]
        self.cli('verified')
        self.review['occurrenceHashes'][0]['sourceSha256'] = '0' * 64
        self.record['independentReview'] = self.private('review.json', canonical(self.review))
        self.cli('incomplete')
        self.record['independentReview'] = None; self.cli('incomplete')

    def test_guidance_and_structural_links_cannot_claim_execution(self):
        original = copy.deepcopy(self.record)
        self.record['remedy']['primaryLayer'] = 'guidance'; self.cli('invalid')
        self.record = copy.deepcopy(original)
        self.record['disposition']['kind'] = 'structural_work_proposed'; self.cli('invalid')
        self.record = original
        self.record['disposition']['links'] = [{'kind': 'task', 'id': 'FOR-99', 'status': 'executed',
                                              'source': self.private('task.txt', b'proposal only')}]
        self.record['disposition']['executed'] = True; self.cli('invalid')
        del self.record['disposition']['executed']
        self.record['disposition']['links'][0]['source'] = self.private('task.json', b'{"id":"FOR-99","status":"proposed"}')
        self.cli('invalid')


if __name__ == '__main__':
    unittest.main()
