#!/usr/bin/env python3
"""Actual Claude and production comparison tools with explicitly SCRIPTED replies.

The whole fixture runs in a loopback-only network namespace. A private launcher
adds synthetic provider settings, verifies the real Claude executable hash, and
executes that unchanged CLI. The manifest binds the launcher; the receipt binds
the launcher and actual executable separately. No live inference is claimed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading
import uuid

PROJECT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT / 'scripts'))
from probe_claude_runtime import Provider


def sha(value):
    return hashlib.sha256(value.encode() if isinstance(value, str) else value).hexdigest()


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')
    path.chmod(0o600)


def artifact(path):
    return {'path': str(path), 'sha256': sha(path.read_bytes()), 'bytes': path.stat().st_size}


def tool(name, **arguments):
    return {'type': 'tool_use', 'id': 'scripted_' + uuid.uuid4().hex,
            'name': 'mcp__crewroom__' + name, 'input': arguments}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--claude', type=Path, required=True)
    parser.add_argument('--model', default='sonnet')
    parser.add_argument('--resolved-model', default='claude-sonnet-5-5')
    parser.add_argument('--binaries', type=Path, required=True)
    parser.add_argument('--check-image', required=True)
    parser.add_argument('--inside-network-namespace', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    namespace = os.readlink('/proc/self/ns/net')
    if not args.inside_network_namespace:
        # Raising lo needs namespace capabilities. Drop them before the fixture
        # starts: MailGuard's bwrap expects an ordinary unprivileged process.
        setup = "import os,subprocess,sys; subprocess.run(['/usr/bin/ip','link','set','lo','up'],check=True); os.execv('/usr/bin/setpriv',['setpriv','--inh-caps=-all','--ambient-caps=-all','--bounding-set=-all',*sys.argv[1:]])"
        return subprocess.call(['unshare', '--user', '--map-current-user', '--keep-caps', '--net',
            sys.executable, '-c', setup, sys.executable, str(Path(__file__).resolve()),
            *sys.argv[1:], '--inside-network-namespace'],
            env={**os.environ, 'CREWROOM_CLAUDE_CONTRACT_PARENT_NETNS': namespace})
    assert namespace != os.environ.pop('CREWROOM_CLAUDE_CONTRACT_PARENT_NETNS'), 'Use the outer namespace launcher'
    assert {name for _, name in socket.if_nameindex()} == {'lo'}, 'Only loopback is allowed'
    capabilities = [line.split(':', 1)[1].strip() for line in Path('/proc/self/status').read_text().splitlines() if line.startswith('Cap')]
    assert capabilities and all(int(value, 16) == 0 for value in capabilities), 'Fixture must run without capabilities'
    os.umask(0o077)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    actual = args.claude.resolve(strict=True)
    actual_hash = sha(actual.read_bytes())
    original = (PROJECT / 'scripts/fixtures/for14-comparison/v1/cases/task-01/workspace/delivery.cjs').read_text()
    blocks = {
        'coordinator': [tool('playbook_status'), tool('repository_read'),
            tool('send_message', to='Builder', body='SCRIPTED contract: execute the assigned tool sequence.')],
        'worker': [tool('repository_read', path='delivery.cjs'),
            tool('repository_search', query='deliveryLabel'),
            tool('write_file', path='contract-note.txt', content='first\n', expectedHash=None),
            tool('write_file', path='contract-note.txt', content='second\n', expectedHash=sha('first\n')),
            tool('edit_file', path='delivery.cjs', expectedHash=sha(original), replacements=[{
                'oldText': "  return days === 1 ? 'Tomorrow' : `In ${days} days`;",
                'newText': "  if (days === 0) return 'Today';\n  return days === 1 ? 'Tomorrow' : `In ${days} days`;"}]),
            tool('run_check', checkId='unit'), tool('run_check', checkId='acceptance'),
            tool('send_message', to='Lead', body='SCRIPTED edits and both real checks completed.'),
            tool('handoff_assignment', status='complete',
                 summary='SCRIPTED provider contract: production edits, unit and acceptance checks completed.')],
    }
    selected = []
    lock = threading.Lock()

    def role_for(body):
        names = {t['name'] for t in body.get('tools', [])}
        worker = 'mcp__crewroom__handoff_assignment' in names
        coordinator = 'mcp__crewroom__playbook_status' in names
        assert worker != coordinator, 'Provider request must identify one admitted actor catalog'
        return 'worker' if worker else 'coordinator'

    def select(body):
        role = role_for(body)
        with lock:
            block = blocks[role].pop(0) if blocks[role] else {'type': 'text', 'text': 'SCRIPTED actor settled.'}
            selected.append({'role': role, 'kind': block['type'], 'tool': block.get('name')})
            return block

    provider = Provider([], block_selector=select)
    child = root / 'comparison'
    report = {'passed': False, 'inference': 'scripted', 'scope': __doc__,
              'network': {'namespace': namespace, 'interfaces': ['lo'], 'externalNetworkUnavailable': True},
              'actualClaude': {'path': str(actual), 'sha256': actual_hash}, 'cases': []}
    native_home = root / 'native-home'
    native_home.mkdir(mode=0o700)
    save(native_home / '.claude.json', {'hasCompletedOnboarding': True, 'oauthAccount': {
        'accountUuid': '00000000-0000-4000-8000-000000000001',
        'organizationUuid': '00000000-0000-4000-8000-000000000002'}})
    launcher = root / 'claude-synthetic-provider'
    launcher.write_text('#!/usr/bin/python3\nimport hashlib,os,sys\n'
        + 'binary=' + repr(str(actual)) + '\n'
        + 'with open(binary,"rb") as source:\n    assert hashlib.sha256(source.read()).hexdigest()==' + repr(actual_hash) + '\n'
        + 'env=dict(os.environ)\n'
        + 'env.update(' + repr({'HOME': str(native_home),
            'ANTHROPIC_API_KEY': 'synthetic-not-a-credential',
            'ANTHROPIC_BASE_URL': f'http://127.0.0.1:{provider.server.server_port}',
            'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1', 'DISABLE_AUTOUPDATER': '1'}) + ')\n'
        + 'os.execve(binary,[binary,*sys.argv[1:]],env)\n')
    launcher.chmod(0o500)
    report['launcher'] = artifact(launcher)
    settings = root / 'synthetic-provider.json'
    save(settings, {'kind': 'claude-code', 'inference': 'scripted', 'model': args.model,
        'resolvedModel': args.resolved_model, 'claude': str(launcher), 'nativeHome': str(native_home)})
    try:
        with (root / 'driver.log').open('w') as log:
            process = subprocess.run([sys.executable, str(PROJECT / 'scripts/rust/for14-run-live.py'),
                '--harness', 'claude-code', '--mode', 'smoke', '--harness-config', str(settings),
                '--roles', str(root / 'absent-role-catalog.json'), '--check-image', args.check_image,
                '--binaries', str(args.binaries.resolve()), '--output', str(child)],
                cwd=PROJECT, stdout=log, stderr=subprocess.STDOUT)
        report['exitCode'] = process.returncode
        evidence = json.loads((child / 'comparison-result.json').read_text())
        final = json.loads((child / 'comparison-final.json').read_text())
        assert evidence['inference'] == final['manifest']['mode'] == 'scripted'
        assert process.returncode == 0 and evidence['passed'], evidence.get('error')
        assert evidence['cleanup']['passed'] and json.loads((child / 'mail-cleanup.json').read_text())['verifiedStopped']
        assert not provider.errors, provider.errors
        cell = next(c for c in final['cells'] if c.get('result', {}).get('observed'))
        assert cell['result']['scores']['successful'], cell['result']['scores']
        assert len(evidence['toolAudits']) == 1 and evidence['toolAudits'][0]['passed']
        for role in ('coordinator', 'worker'):
            requests = [request for request in provider.requests if role_for(request) == role]
            assert requests, role
            expected = [{'name': 'mcp__crewroom__' + t['name'], 'description': t['description'], 'input_schema': t['parameters']}
                        for t in final['manifest']['execution']['tools'][role]]
            for request in requests:
                assert all(set(t) <= {'name', 'description', 'input_schema', 'cache_control'} for t in request['tools'])
                actual_tools = [{k: t[k] for k in ('name', 'description', 'input_schema')} for t in request['tools']]
                assert actual_tools == expected, role + ' provider-advertised catalog differs'
                assert request['model'] == args.resolved_model, 'Provider model differs from the pinned alias'
            report['cases'].append({'actor': role, 'providerRequests': len(requests),
                'exactAdvertisedSchemas': True, 'catalogSha256': sha(json.dumps(expected, sort_keys=True, separators=(',', ':')))})
        runtime = Path(json.loads((child / 'settings.json').read_text())['runtimeRoot'])
        calls = []
        for attempt in cell['result']['attempts']:
            events = [json.loads(line)['event'] for line in (runtime / attempt['id'] / 'events.jsonl').read_text().splitlines()]
            calls.extend(event for event in events if event['type'] == 'host_tool_call')
        expected_names = {'playbook_status', 'repository_read', 'repository_search', 'write_file',
                          'edit_file', 'run_check', 'send_message', 'handoff_assignment'}
        assert expected_names <= {event['toolName'] for event in calls}
        writes = [event['arguments']['expectedHash'] for event in calls if event['toolName'] == 'write_file']
        assert writes == [None, sha('first\n')], writes
        assert len([event for event in cell['events'] if event['type'] == 'edit']) == 3
        assert len([event for event in cell['events'] if event['type'] == 'handoff']) == 1
        assert len([event for event in calls if event['toolName'] == 'send_message']) == 2
        assert len(cell['result']['checks']) == 2 and all(check['exitCode'] == 0 for check in cell['result']['checks'])
        report.update(passed=True, comparisonId=final['id'], observed=final['coverage']['observed'],
            activeMs=final['activeMs'], tools=sorted(expected_names), nullAndStringWriteHash=True,
            exactHostCatalogAudit=True, realProtectedDockerChecks=2, externalMailToolCalls=2,
            liveInference=False, comparisonResult=artifact(child / 'comparison-result.json'))
    except Exception as error:
        report['error'] = str(error)
    finally:
        provider.close()
        save(root / 'provider-requests.json', provider.requests)
        report['providerRequests'] = artifact(root / 'provider-requests.json')
        report['providerRequestCount'] = len(provider.requests)
        report['providerErrors'] = provider.errors
        report['selectedBlocks'] = selected
        report['providerStopped'] = not provider.thread.is_alive()
        report['actualClaudeUnchanged'] = sha(actual.read_bytes()) == actual_hash
        if (child / 'comparison-result.json').exists():
            receipt = json.loads((child / 'comparison-result.json').read_text())
            report['productionCleanup'] = receipt.get('cleanup', {})
        if (child / 'mail-cleanup.json').exists():
            report['mailCleanup'] = json.loads((child / 'mail-cleanup.json').read_text())
        report['cleanupPassed'] = report['providerStopped'] and report.get('productionCleanup', {}).get('passed') is True and report.get('mailCleanup', {}).get('verifiedStopped') is True
        report['passed'] &= report['cleanupPassed'] and report['actualClaudeUnchanged']
        save(root / 'result.json', report)
        print(json.dumps(report))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
