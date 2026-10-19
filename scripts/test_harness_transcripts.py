"""FOR-72: committed harness transcripts pass the redaction gate, the gate refuses
private content, and re-recordings produce a readable protocol diff."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import urllib.request

import harness_transcripts as transcripts

SCRIPT = Path(transcripts.__file__).resolve()


def load(harness, scenario):
    path = transcripts.TRANSCRIPTS / harness / f'{scenario}.jsonl'
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def write(directory, name, lines):
    path = Path(directory) / name
    path.write_text(''.join(json.dumps(line) + '\n' for line in lines))
    return path


class CommittedTranscripts(unittest.TestCase):
    def test_every_harness_has_each_named_scenario_and_passes_the_gate(self):
        for harness in transcripts.HARNESSES:
            for scenario in transcripts.SCENARIOS:
                lines = load(harness, scenario)
                header = lines[0]
                self.assertEqual(transcripts.check_transcript(lines), [], f'{harness}/{scenario}')
                self.assertEqual((header['harness'], header['scenario']), (harness, scenario))
                self.assertTrue(header['version'] and len(header['executable']['sha256']) == 64)
                self.assertIn('simulated', header['evidence'])
                self.assertEqual(header['tasks'], 'synthetic')

    def test_the_cli_gate_passes_committed_transcripts(self):
        result = subprocess.run([sys.executable, str(SCRIPT), 'check'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['checked'], len(transcripts.HARNESSES) * len(transcripts.SCENARIOS))


class RedactionGate(unittest.TestCase):
    def mutate(self, change):
        lines = copy.deepcopy(load('claude-code', 'tool-completion'))
        change(lines)
        return transcripts.check_transcript(lines)

    def frame(self, lines, kind):
        return next(line['frame'] for line in lines[1:] if line.get('frame', {}).get('type') == kind)

    def test_private_content_is_refused(self):
        cases = {
            'home path': lambda l: self.frame(l, 'system').update(cwd='/home/operator/private-repo'),
            'unredacted credential or account field': lambda l: self.frame(l, 'user').update(accessToken='opaque-value'),
            'JSON web token': lambda l: self.frame(l, 'user').update(note='eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln'),
            'API key': lambda l: self.frame(l, 'user').update(note='sk-ant-abcdefghijklmnopqrstuvwxyz'),
            'email address': lambda l: self.frame(l, 'user').update(note='person@example.org'),
            'raw UUID (account or session identifier)': lambda l: self.frame(l, 'user').update(
                session_id='0f8fad5b-d9cb-469f-a165-70867728950e'),
            'is not a synthetic task': lambda l: next(s for s in l[0]['steps'] if s['op'] == 'input').update(message='Fix the billing bug'),
        }
        for label, change in cases.items():
            problems = self.mutate(change)
            self.assertTrue(any(label in p for p in problems), f'{label}: {problems}')

    def test_cli_reports_locations_without_repeating_the_secret(self):
        lines = copy.deepcopy(load('omp', 'tool-completion'))
        lines[3]['frame']['apiKey'] = 'private-value-123'
        with tempfile.TemporaryDirectory() as directory:
            path = write(directory, 'leak.jsonl', lines)
            result = subprocess.run([sys.executable, str(SCRIPT), 'check', str(path)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('line 4/frame/apiKey', result.stderr)
        self.assertNotIn('private-value-123', result.stderr)

    def test_normalizing_replaces_paths_identifiers_and_credentials(self):
        root = Path('/tmp/recording-root')
        identifier = '0f8fad5b-d9cb-469f-a165-70867728950e'
        lines = transcripts.normalize([{'format': transcripts.FORMAT},
                                       {'frame': {'cwd': f'{root}/assignment', 'session_id': identifier, 'again': identifier,
                                                  'accessToken': 'opaque', 'url': 'http://127.0.0.1:5000/v1'}}],
                                      root, 'http://127.0.0.1:5000')
        frame = lines[1]['frame']
        self.assertEqual(frame['cwd'], '<<root>>/assignment')
        self.assertEqual((frame['session_id'], frame['again']), ('<<uuid:1>>', '<<uuid:1>>'))
        self.assertEqual(frame['accessToken'], '<<redacted>>')
        self.assertEqual(frame['url'], '<<provider>>/v1')


class ProtocolDiff(unittest.TestCase):
    def test_a_rerecorded_version_yields_a_readable_protocol_diff(self):
        old = load('codex', 'tool-completion')
        new = copy.deepcopy(old)
        new[0]['version'] = 'codex-cli 9.9.9'
        new[0]['executable']['sha256'] = 'f' * 64
        completed = next(i for i, line in enumerate(new) if line.get('frame', {}).get('method') == 'turn/completed')
        new[completed]['frame']['params']['turn']['reviewState'] = 'pending'
        new.insert(completed, {'session': 1, 'dir': 'recv', 'frame': {'method': 'thread/newSignal/changed', 'params': {'flag': True}}})
        with tempfile.TemporaryDirectory() as directory:
            before, after = write(directory, 'old.jsonl', old), write(directory, 'new.jsonl', new)
            result = subprocess.run([sys.executable, str(SCRIPT), 'diff', str(before), str(after)], capture_output=True, text=True)
            same = subprocess.run([sys.executable, str(SCRIPT), 'diff', str(before), str(before)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('+version codex-cli 9.9.9', result.stdout)
        self.assertIn('-version codex-cli 0.163.0', result.stdout)
        self.assertIn('+[session 1] recv thread/newSignal/changed {"method": "thread/newSignal/changed", "params": {"flag": "bool"}}',
                      result.stdout)
        self.assertTrue(any(l.startswith('+') and '"reviewState": "string"' in l for l in result.stdout.splitlines()))
        self.assertEqual(same.stdout.strip(), 'No protocol shape differences.')


class SyntheticProvider(unittest.TestCase):
    def post(self, provider, path, body):
        request = urllib.request.Request(provider.origin + path, data=json.dumps(body).encode(),
                                         headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.read().decode()

    def events(self, text):
        return [json.loads(line[6:]) for line in text.splitlines() if line.startswith('data: ')]

    def test_scripted_output_in_both_dialects_and_side_calls_consume_nothing(self):
        provider = transcripts.Provider([{'kind': 'tool', 'name': 'echo', 'arguments': {'text': 'x'}},
                                         {'kind': 'text', 'text': 'Synthetic answer'}])
        try:
            self.assertEqual(self.post(provider, '/backend-api/codex/analytics-events/events', {}), '{}')
            first = self.events(self.post(provider, '/v1/messages', {'model': 'm', 'tools': [{'name': 'mcp__crewroom__echo'}]}))
            second = self.events(self.post(provider, '/v1/responses', {'model': 'm'}))
        finally:
            provider.close()
        start = next(e for e in first if e['type'] == 'content_block_start')
        self.assertEqual(start['content_block']['name'], 'mcp__crewroom__echo')
        self.assertEqual(second[-1]['type'], 'response.completed')
        self.assertIn('Synthetic answer', json.dumps(second))
        self.assertEqual(provider.errors, [])


if __name__ == '__main__':
    unittest.main()
