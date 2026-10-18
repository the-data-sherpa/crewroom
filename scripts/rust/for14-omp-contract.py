#!/usr/bin/env python3
"""Actual OMP + production comparison tools, with explicitly SCRIPTED provider replies."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

PROJECT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT / 'scripts'))
from omp_offline_fixture import Provider


def sha(value):
    return hashlib.sha256(value.encode() if isinstance(value, str) else value).hexdigest()


def provider_tools(tools):
    # Pinned OMP 18.4.12 injectIntentIntoSchema adds this one tracing field and
    # extractIntent removes it before dispatch. Assert the complete projection;
    # do not discard arbitrary provider-facing schema differences.
    result = []
    for tool in tools:
        schema = copy.deepcopy(tool['parameters'])
        assert 'i' not in schema['properties'] and 'i' not in schema['required']
        schema['properties'] = {'i': {'type': 'string', 'description': 'concise intent'}, **schema['properties']}
        schema['required'].append('i')
        result.append({'type': 'function', 'name': tool['name'], 'description': tool['description'], 'parameters': schema})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--harness-config', type=Path, required=True)
    parser.add_argument('--binaries', type=Path, required=True)
    parser.add_argument('--check-image', required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    source = json.loads(args.harness_config.read_text())
    # Copy only executable and non-secret model metadata. No upstream endpoint,
    # credential or user configuration is passed into this scripted contract.
    profile = source['catalogProfile']
    provider_id = profile['provider']
    definition = next(m for m in source['ompModels']['providers'][provider_id]['models'] if m['id'] == source['model'])
    fixture = Provider(reasoning=True)
    report = {'passed': False, 'inference': 'scripted', 'scope': __doc__, 'cases': []}
    child = root / 'comparison'
    tool = lambda name, **arguments: {'kind': 'tool', 'name': name, 'arguments': {'i': 'Exercise ' + name, **arguments}}
    fixture.enqueue('Lead', tool('playbook_status'), tool('repository_read'),
                    tool('send_message', to='Builder', body='SCRIPTED contract: execute assigned tool sequence.'),
                    *[{'kind': 'text', 'text': 'SCRIPTED coordinator settled.'} for _ in range(6)])
    original = (PROJECT / 'scripts/fixtures/for14-comparison/v1/cases/task-01/workspace/delivery.cjs').read_text()
    fixture.enqueue('Builder', tool('repository_read', path='delivery.cjs'),
                    tool('repository_search', query='deliveryLabel'),
                    tool('write_file', path='contract-note.txt', content='first\n', expectedHash=None),
                    tool('write_file', path='contract-note.txt', content='second\n', expectedHash=sha('first\n')),
                    tool('edit_file', path='delivery.cjs', expectedHash=sha(original), replacements=[{
                        'oldText': "  return days === 1 ? 'Tomorrow' : `In ${days} days`;",
                        'newText': "  if (days === 0) return 'Today';\n  return days === 1 ? 'Tomorrow' : `In ${days} days`;"}]),
                    tool('run_check', checkId='unit'), tool('run_check', checkId='acceptance'),
                    tool('send_message', to='Lead', body='SCRIPTED contract edits and both real checks completed.'),
                    tool('handoff_assignment', status='complete', summary='SCRIPTED provider contract: production edits, unit and acceptance checks completed.'),
                    *[{'kind': 'text', 'text': 'SCRIPTED worker settled.'} for _ in range(4)])
    config = {'kind': 'omp', 'inference': 'scripted', 'model': source['model'], 'omp': source['omp'],
              'catalogProfile': profile, 'ompModels': {'providers': {provider_id: {
                  'baseUrl': f'http://127.0.0.1:{fixture.port}/{{agent}}/{provider_id}/v1',
                  'api': profile['api'], 'apiKey': 'synthetic-offline-key', 'models': [definition]}}}}
    settings = root / 'synthetic-provider.json'
    with open(settings, 'x', opener=lambda p, f: os.open(p, f, 0o600)) as out:
        json.dump(config, out)
    try:
        with (root / 'driver.log').open('w') as log:
            result = subprocess.run([sys.executable, str(PROJECT / 'scripts/rust/for14-run-live.py'),
                '--harness', 'omp', '--mode', 'smoke', '--harness-config', str(settings),
                '--check-image', args.check_image, '--binaries', str(args.binaries.resolve()),
                '--output', str(child)], cwd=PROJECT, stdout=log, stderr=subprocess.STDOUT)
        report['exitCode'] = result.returncode
        evidence = json.loads((child / 'comparison-result.json').read_text())
        final = json.loads((child / 'comparison-final.json').read_text())
        assert evidence['inference'] == final['manifest']['mode'] == 'scripted'
        assert result.returncode == 0 and evidence['passed'], evidence.get('error')
        assert evidence['cleanup']['passed'] and json.loads((child / 'mail-cleanup.json').read_text())['verifiedStopped']
        assert not fixture.errors, fixture.errors
        cell = next(c for c in final['cells'] if c.get('result', {}).get('observed'))
        assert cell['result']['scores']['successful'], cell['result']['scores']
        assert len(evidence['toolAudits']) == 1 and evidence['toolAudits'][0]['passed']
        for name, role in [('Lead', 'coordinator'), ('Builder', 'worker')]:
            requests = [r for r in fixture.requests if r['agent'] == name]
            assert requests, name
            expected = provider_tools(final['manifest']['execution']['tools'][role])
            for request in requests:
                actual = [{k: t[k] for k in ['type', 'name', 'description', 'parameters']} for t in request['body']['tools']]
                assert actual == expected, f'{name} provider-advertised catalog differs'
            report['cases'].append({'actor': name, 'providerRequests': len(requests), 'exactPinnedWireSchemas': True,
                                    'catalogSha256': sha(json.dumps(expected, sort_keys=True, separators=(',', ':')))})
        runtime = Path(json.loads((child / 'settings.json').read_text())['runtimeRoot'])
        calls = []
        for attempt in cell['result']['attempts']:
            events = [json.loads(s)['event'] for s in (runtime / attempt['id'] / 'events.jsonl').read_text().splitlines()]
            calls.extend(e for e in events if e['type'] == 'host_tool_call')
        expected_names = {'playbook_status', 'repository_read', 'repository_search', 'write_file', 'edit_file', 'run_check', 'send_message', 'handoff_assignment'}
        assert expected_names <= {e['toolName'] for e in calls}
        assert all('i' not in e['arguments'] for e in calls), 'OMP intent must not escape into Crewroom host arguments'
        writes = [e['arguments']['expectedHash'] for e in calls if e['toolName'] == 'write_file']
        assert writes == [None, sha('first\n')], writes
        assert len([e for e in cell['events'] if e['type'] == 'edit']) == 3
        assert len([e for e in cell['events'] if e['type'] == 'handoff']) == 1
        report.update(passed=True, comparisonId=final['id'], observed=final['coverage']['observed'],
                      activeMs=final['activeMs'], tools=sorted(expected_names), nullAndStringWriteHash=True,
                      cleanupPassed=True, liveInference=False, comparisonResult=str(child / 'comparison-result.json'))
        transform = PROJECT / '.crewroom/mh2-omp-fixture/node_modules/@oh-my-pi/pi-agent-core/src/agent-loop.ts'
        report['wireTransform'] = {'scope': 'OMP 18.4.12 requires i:string concise intent in provider schemas and strips i before host dispatch; all other names/order/descriptions/schema fields compared exactly',
                                   'source': str(transform), 'sha256': sha(transform.read_bytes())}
    except Exception as error:
        report['passed'] = False
        report['error'] = str(error)
    finally:
        fixture.close()
        request_bytes = (json.dumps(fixture.requests, indent=2) + '\n').encode()
        (root / 'provider-requests.json').write_bytes(request_bytes)
        report['providerRequests'] = {'count': len(fixture.requests), 'path': str(root / 'provider-requests.json'), 'sha256': sha(request_bytes)}
        report['providerErrors'] = fixture.errors
        (root / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
