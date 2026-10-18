#!/usr/bin/env python3
"""Production TypeScript preflight against OMP 18.3.1; synthetic models, guarded network."""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import time

from omp_offline_fixture import Agent, Provider, Sandbox

PROJECT = Path(__file__).resolve().parent.parent


def invoke(value, agent=None):
    child = subprocess.Popen(['node', '--import', 'tsx', 'scripts/model-policy-driver.ts'], cwd=PROJECT,
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    child.stdin.write(json.dumps(value) + '\n'); child.stdin.flush()
    deadline = time.monotonic() + 25
    try:
        while time.monotonic() < deadline:
            if not select.select([child.stdout], [], [], .1)[0]:
                continue
            line = child.stdout.readline()
            if not line:
                raise RuntimeError(child.stderr.read())
            result = json.loads(line)
            if result.get('done'):
                child.stdin.close(); child.wait(timeout=5)
                return result
            assert result.get('rpc') and agent is not None
            try:
                reply = {'replyTo': result['id'], 'result': agent.command(result['type'], **result['args'])}
            except Exception as error:
                reply = {'replyTo': result['id'], 'error': str(error)}
            child.stdin.write(json.dumps(reply) + '\n'); child.stdin.flush()
        raise TimeoutError('Production preflight did not complete')
    finally:
        if child.poll() is None: child.kill(); child.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    root = parser.parse_args().output.resolve(); root.mkdir(parents=True, exist_ok=False, mode=0o700)
    catalog = json.loads((PROJECT / 'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())
    policy_catalog = [{'provider': m['provider'], 'id': m['id'], 'api': m['api'], 'inputs': m['declaredInput'],
                       'efforts': list(dict.fromkeys(['off', *(m['declaredThinkingEfforts'] or [])])), 'tools': None}
                      for m in catalog['models']]
    role = {'id': 'fixture', 'name': 'Fixture', 'category': 'Test', 'purpose': 'Verify routing',
            'instructions': 'Return the fixture marker.', 'enabled': True}
    provider = Provider(); agents = []; report = {'cases': []}
    sandbox = None
    try:
        sandbox = Sandbox(root, [provider.port]); report['guard'] = sandbox.verify(provider.port)
        for index, (selector, effort) in enumerate([
            ('aperture-openai/openai.gpt-6-sol', 'low'),
            ('aperture-anthropic/anthropic.claude-sonnet-5', 'medium'),
            ('aperture-google/gemini-3.8-flash', 'low'),
        ]):
            value = {'role': {**role, 'model': selector, 'effort': effort}, 'catalog': policy_catalog}
            config = invoke({**value, 'operation': 'settings'})
            assert 'error' not in config, config
            agent = Agent(f'preflight-{index}', root, sandbox, provider, catalog, selector, settings=config['result'])
            agents.append(agent)
            result = invoke(value, agent)
            assert 'error' not in result, result
            before = len(provider.requests)
            agent.text_turn(provider, f'ADMITTED_{index}')
            requests = provider.requests[before:]
            assert len(requests) == 1 and requests[0]['provider'] + '/' + requests[0]['model'] == selector, requests
            report['cases'].append({'selector': selector, 'effort': effort, 'identity': result['result']['identity'],
                                    'providerRequests': 1, 'status': 'passed'})
            if index == 0:
                # OMP 18.3.1 has bounded HTTP retries below its session retry switch.
                # Recovery must remain on the admitted model; hard denial must terminate.
                before = len(provider.requests); start = len(agent.frames)
                provider.enqueue(agent.name, {'kind': 'error', 'status': 429}, {'kind': 'text', 'text': 'RECOVERED_SAME_MODEL'})
                agent.command('prompt', message='Execute the rate-limited fixture request.')
                agent.wait_frame('prompt_result', start)
                assert len(provider.requests) - before == 2
                assert all(r['provider'] + '/' + r['model'] == selector for r in provider.requests[before:])
                report['cases'].append({'name': '429 recovery uses the same model', 'status': 'passed', 'providerRequests': 2})
                before = len(provider.requests); start = len(agent.frames)
                provider.enqueue(agent.name, {'kind': 'error', 'status': 403})
                agent.command('prompt', message='Execute the denied fixture request.')
                agent.wait_frame('prompt_result', start)
                assert len(provider.requests) - before == 1
                assert any(m.get('stopReason') == 'error' for m in agent.command('get_messages')['messages'])
                report['cases'].append({'name': '403 exposes failure without fallback', 'status': 'passed', 'providerRequests': 1})
            agent.close()
        for selector, effort in [
            ('aperture-openai/zai-org/GLM-5.2', 'medium'),
            ('aperture-google/gemini-3.1-pro-preview', 'medium'),
            ('aperture-bedrock-compat/us.anthropic.claude-haiku-4-5-20251001-v1:0', 'high'),
        ]:
            result = invoke({'role': {**role, 'model': selector, 'effort': effort}, 'catalog': policy_catalog})
            assert 'Unsupported effort' in result.get('error', ''), result
            report['cases'].append({'selector': selector, 'effort': effort, 'status': 'rejected-before-launch'})
            # A stale or optimistic capability declaration must also fail against current OMP readback.
            stale_catalog = json.loads(json.dumps(policy_catalog))
            for model in stale_catalog:
                if model['provider'] + '/' + model['id'] == selector: model['efforts'].append(effort)
            value = {'role': {**role, 'model': selector, 'effort': effort}, 'catalog': stale_catalog}
            config = invoke({**value, 'operation': 'settings'})
            agent = Agent('stale-' + str(len(agents)), root, sandbox, provider, catalog, selector, settings=config['result'])
            agents.append(agent)
            result = invoke(value, agent)
            assert 'Unsupported effort' in result.get('error', ''), result
            report['cases'].append({'selector': selector, 'effort': effort, 'status': 'rejected-by-runner-preflight'})
            agent.close()
        report['passed'] = True
    except Exception as error:
        report['passed'] = False; report['error'] = str(error)
    finally:
        for agent in agents:
            if not agent.log.closed: agent.close(abrupt=True)
        provider.close()
        report['allOwnedProcessesExited'] = all(agent.proc.poll() is not None for agent in agents)
        report['providerRequests'] = len(provider.requests)
        report['providerErrors'] = provider.errors
        (root / 'provider-requests.json').write_text(json.dumps(provider.requests, indent=2) + '\n')
        if sandbox and report['allOwnedProcessesExited']: sandbox.close()
        report['passed'] = report.get('passed', False) and report['allOwnedProcessesExited'] and not provider.errors
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
