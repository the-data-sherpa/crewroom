#!/usr/bin/env python3
"""Observe model-error and effort behavior in the real OMP RPC boundary offline."""
import argparse
import json
from pathlib import Path
import sys
import traceback

from omp_offline_fixture import Agent, Provider, Sandbox

PROJECT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve(); root.mkdir(parents=True, exist_ok=False, mode=0o700)
    catalog = json.loads((PROJECT / 'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())
    provider = Provider(); sandbox = Sandbox(root, [provider.port]); agents = []
    report = {'guard': sandbox.verify(provider.port), 'scope': 'OMP observations; Crewroom preflight/error handling not implemented.',
              'contracts': [], 'failures': [], 'effortObservations': []}
    settings = {'retry': {'enabled': True, 'maxRetries': 1, 'baseDelayMs': 10, 'maxDelayMs': 100, 'modelFallback': False}}

    def launch(name, model='aperture-openai/openai.gpt-6-sol'):
        a = Agent(name, root, sandbox, provider, catalog, model, settings=settings)
        agents.append(a); a.command('get_state'); return a

    def case(name, fn):
        try:
            evidence = fn()
            report['contracts'].append({'name': name, 'status': 'passed', 'evidence': evidence})
            print(json.dumps({'contract': name, 'status': 'passed'}), flush=True)
        except Exception as exc:
            report['failures'].append({'name': name, 'error': str(exc), 'trace': traceback.format_exc()})
            report['contracts'].append({'name': name, 'status': 'failed', 'error': str(exc)})
            print(json.dumps({'contract': name, 'status': 'failed', 'error': str(exc)}), flush=True)

    try:
        for status in (401, 403):
            def denied(status=status):
                a = launch(f'http-{status}')
                provider.enqueue(a.name, *[{'kind': 'error', 'status': status} for _ in range(12)])
                start = len(a.frames); request_start = len(provider.requests)
                a.command('prompt', message='Execute an explicitly denied fixture request.')
                outcome = a.wait_frame('prompt_result', start, timeout=25)
                messages = a.command('get_messages')['messages']
                failures = [m for m in messages if m.get('role') == 'assistant' and m.get('stopReason') == 'error']
                assert failures, 'Provider failure was not exposed as an assistant error'
                assert len(provider.requests)-request_start <= 4, 'Unbounded failure retries'
                a.close()
                return {'promptStatus': outcome['status'], 'assistantErrors': len(failures),
                        'requests': len(provider.requests)-request_start, 'gateMustInspectErrorEvidence': True}
            case(f'HTTP {status} exposes error with bounded requests', denied)

        def rate_limit():
            a = launch('rate-limit')
            provider.enqueue(a.name, {'kind': 'error', 'status': 429}, {'kind': 'text', 'text': 'RECOVERED_AFTER_429'})
            start = len(a.frames); index = len(provider.requests)
            a.command('prompt', message='Execute rate limit recovery fixture.')
            outcome = a.wait_frame('prompt_result', start, timeout=25)
            messages = a.command('get_messages')['messages']
            assert outcome['status'] == 'completed'
            assert 'RECOVERED_AFTER_429' in json.dumps(messages)
            assert len(provider.requests)-index == 2
            assert len({r['model'] for r in provider.requests[index:]}) == 1
            a.close()
            return {'requests': 2, 'sameModel': True, 'recoveredTextObserved': True}
        case('One 429 retries the same pinned model then succeeds', rate_limit)

        def empty_stream():
            a = launch('empty-stream')
            provider.enqueue(a.name, *[{'kind': 'empty'} for _ in range(12)])
            start = len(a.frames); index = len(provider.requests)
            a.command('prompt', message='Execute empty/malformed stream fixture.')
            outcome = a.wait_frame('prompt_result', start, timeout=25)
            frames = a.frames[start:]
            errors = [f for f in frames if f.get('type') == 'auto_retry_end' and f.get('success') is False]
            assistant_errors = [m for m in a.command('get_messages')['messages'] if m.get('stopReason') == 'error']
            assert errors or assistant_errors, 'No failure signal for unusable response'
            count = len(provider.requests)-index
            assert count <= 4, f'Expected bounded retry configuration; got {count}'
            a.close()
            return {'promptStatus': outcome['status'], 'requests': count, 'failureEvents': len(errors),
                    'assistantErrors': len(assistant_errors), 'noQualityGateFromPromptStatus': True}
        case('Malformed/empty stream exposes failure after bounded retries', empty_stream)

        for name, selector, requested in [
            ('glm', 'aperture-openai/zai-org/GLM-5.2', 'medium'),
            ('gemini', 'aperture-google/gemini-3.1-pro-preview', 'medium'),
            ('no-reasoning', 'aperture-bedrock-compat/us.anthropic.claude-haiku-4-5-20251001-v1:0', 'high'),
        ]:
            def effort(name=name, selector=selector, requested=requested):
                a = launch('effort-' + name, selector)
                error = None
                try:
                    a.command('set_thinking_level', level=requested)
                except RuntimeError as exc:
                    error = str(exc)
                state = a.command('get_state')
                observation = {'selector': selector, 'requested': requested, 'observed': state.get('thinkingLevel'),
                               'error': error, 'strictlyRejected': error is not None}
                report['effortObservations'].append(observation)
                a.close()
                # This is the approved preflight contract. OMP may normalize,
                # but that cannot be mistaken for Crewroom rejecting the request.
                assert error is not None, f'Unsupported effort accepted; runner reported {state.get("thinkingLevel")}'
                return observation
            case(f'Unsupported effort is rejected: {name}', effort)
    finally:
        report['interrupted'] = sys.exc_info()[0] is not None
        for a in agents:
            if not a.log.closed:
                a.close(abrupt=True)
        provider.close()
        report['providerErrors'] = provider.errors
        report['allOwnedProcessesExited'] = all(a.proc.poll() is not None for a in agents)
        if report['allOwnedProcessesExited']:
            sandbox.close()
            report['disposableRuntimeRemoved'] = True
        report['passed'] = (not report['interrupted'] and not report['failures'] and not provider.errors
                            and report['allOwnedProcessesExited'] and len(report['contracts']) == 7)
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        (root / 'provider-requests.json').write_text(json.dumps(provider.requests, indent=2) + '\n')
        print(json.dumps({'report': str(root/'report.json'), 'passed': report['passed'], 'failures': len(report['failures'])}), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
