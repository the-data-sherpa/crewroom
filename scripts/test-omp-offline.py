#!/usr/bin/env python3
"""Exercise real independent OMP processes against a scripted loopback provider.

This validates runner contracts, not Crewroom workflow or model intelligence.
macOS/OMP 18.3.1 fixture; remote traffic is denied by the OS sandbox.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import signal
import sys
import threading
import traceback

from omp_offline_fixture import Agent, Provider, Sandbox, wait_until

PROJECT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--roles', type=int, default=19, help='Limit roles only for harness development.')
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    catalog_path = PROJECT / 'docs/testing/fixtures/omp-model-catalog-2026-09-26.json'
    profiles_path = PROJECT / 'docs/testing/model-profiles.json'
    catalog, profiles = json.loads(catalog_path.read_text()), json.loads(profiles_path.read_text())
    report = {'startedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'scope': 'Real OMP RPC, synthetic provider; no Crewroom workflow claim.',
              'fixtureHashes': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in (catalog_path, profiles_path)},
              'bindings': [], 'profileTurns': [], 'contracts': [], 'failures': [], 'processes': []}
    provider = Provider()
    agents = []

    def result(name, action):
        try:
            evidence = action()
            report['contracts'].append({'name': name, 'status': 'passed', 'evidence': evidence})
            print(json.dumps({'contract': name, 'status': 'passed'}), flush=True)
        except Exception as exc:
            report['contracts'].append({'name': name, 'status': 'failed', 'error': str(exc)})
            report['failures'].append({'name': name, 'error': str(exc), 'trace': traceback.format_exc()})
            print(json.dumps({'contract': name, 'status': 'failed', 'error': str(exc)}), flush=True)

    def launch(name, selector='aperture-openai/openai.gpt-6-sol', session=None):
        a = Agent(name, root, sandbox, provider, catalog, selector, session)
        agents.append(a)
        state = a.command('get_state')
        report['processes'].append({'role': name, 'pid': a.proc.pid, 'pgid': os.getpgid(a.proc.pid),
                                    'sessionId': state['sessionId'], 'logs': str(a.root.relative_to(root))})
        return a, state

    try:
        sandbox = Sandbox(root, [provider.port])
        report['guard'] = sandbox.verify(provider.port)
        report['ompBinaryHash'] = hashlib.sha256(sandbox.binary.read_bytes()).hexdigest()
        print(json.dumps({'guard': report['guard']}), flush=True)
        for role in profiles['roles'][:args.roles]:
            a = None
            try:
                a, _ = launch(role['id'])
                for m in catalog['models']:
                    a.command('set_model', provider=m['provider'], modelId=m['id'])
                    observed = a.command('get_state')['model']
                    assert (observed['provider'], observed['id'], observed['api']) == (m['provider'], m['id'], m['api'])
                    report['bindings'].append({'role': role['id'], 'selector': m['selector'], 'api': observed['api'], 'status': 'passed'})
                for profile in profiles['profiles']:
                    assignment = next(x for x in profile['assignments'] if x['roleId'] == role['id'])
                    model = profiles['modelPool'][assignment['modelKey']]
                    a.command('set_model', provider=model['provider'], modelId=model['modelId'])
                    a.command('set_thinking_level', level=model['effort'] or 'off')
                    index = len(provider.requests)
                    marker = f'OFFLINE_{profile["id"]}_{role["id"]}'
                    a.text_turn(provider, marker)
                    requests = provider.requests[index:]
                    assert len(requests) == 1, f'Unexpected secondary/retry request: {len(requests)}'
                    req = requests[0]
                    assert (req['agent'], req['provider'], req['model']) == (role['id'], model['provider'], model['modelId']), req
                    report['profileTurns'].append({'profile': profile['id'], 'role': role['id'], 'selector': model['selector'],
                                                  'path': req['path'], 'status': 'passed', 'marker': marker})
                print(json.dumps({'role': role['id'], 'bindings': 50, 'profileTurns': 3, 'status': 'passed'}), flush=True)
            except Exception as exc:
                report['failures'].append({'role': role['id'], 'error': str(exc), 'trace': traceback.format_exc()})
                print(json.dumps({'role': role['id'], 'status': 'failed', 'error': str(exc)}), flush=True)
            finally:
                if a:
                    a.close()

        def unknown_model():
            a, _ = launch('unknown-model')
            before = a.command('get_state')['model']
            try:
                a.command('set_model', provider='nonexistent-fixture', modelId='not-a-model')
            except RuntimeError as exc:
                error = str(exc)
            else:
                raise AssertionError('Unknown model unexpectedly accepted')
            after = a.command('get_state')['model']
            assert (after['id'], after['provider']) == (before['id'], before['provider'])
            a.close()
            return {'error': error, 'previousModelPreserved': True}
        result('Unknown model rejected without changing selection', unknown_model)

        def independent_recovery():
            c, cs = launch('recovery-coordinator')
            one, _ = launch('recovery-specialist-one')
            two, state = launch('recovery-specialist-two')
            assert len({c.proc.pid, one.proc.pid, two.proc.pid}) == 3
            assert len({os.getpgid(x.proc.pid) for x in (c, one, two)}) == 3
            two.text_turn(provider, 'DURABLE_CHECKPOINT')
            session = two.command('get_state')['sessionFile']
            old_pid = two.proc.pid
            two.close(abrupt=True)
            assert c.proc.poll() is None and one.proc.poll() is None
            c.text_turn(provider, 'COORDINATOR_STILL_RESPONDS')
            one.text_turn(provider, 'SPECIALIST_ONE_STILL_RESPONDS')
            replacement, state2 = launch('recovery-specialist-two', session=session)
            assert replacement.proc.pid != old_pid
            assert state2['sessionId'] == state['sessionId']
            messages = replacement.command('get_messages')['messages']
            assert any('DURABLE_CHECKPOINT' in json.dumps(m) for m in messages)
            replacement.text_turn(provider, 'CONTINUED_AFTER_KILL')
            new_pid = replacement.proc.pid
            for a in (c, one, replacement):
                a.close()
            return {'oldPid': old_pid, 'replacementPid': new_pid, 'sameSession': True, 'peerProcessesSurvived': True,
                    'limitation': 'Runner session continuation only; Crewroom ownership/replacement policy is unimplemented.'}
        result('Separate processes and abrupt session continuation', independent_recovery)

        def abort_stream():
            a, _ = launch('interrupt-provider')
            entered, release = threading.Event(), threading.Event()
            provider.enqueue(a.name, {'kind': 'hold', 'entered': entered, 'release': release})
            start = len(a.frames)
            a.command('prompt', message='Wait for the scripted provider.')
            assert entered.wait(10), 'No request observed'
            a.command('abort')
            outcome = a.wait_frame('prompt_result', start)
            release.set()
            wait_until(lambda: not a.command('get_state')['isStreaming'])
            a.text_turn(provider, 'RESUMED_AFTER_ABORT')
            a.close()
            return {'promptOutcome': outcome, 'resumed': True,
                    'limitation': 'Does not prove interruption of shell/tool descendants.'}
        result('Abort pending model stream and run another turn', abort_stream)

    except Exception as exc:
        report['failures'].append({'name': 'harness', 'error': str(exc), 'trace': traceback.format_exc()})
    finally:
        report['interrupted'] = sys.exc_info()[0] is not None
        for a in agents:
            if not a.log.closed:
                a.close(abrupt=True)
        provider.close()
        report['providerErrors'] = provider.errors
        report['providerRequestCount'] = len(provider.requests)
        report['allOwnedProcessesExited'] = all(a.proc.poll() is not None for a in agents)
        if 'sandbox' in locals() and report['allOwnedProcessesExited']:
            sandbox.close()
            report['disposableRuntimeRemoved'] = True
        report['passed'] = (not report['interrupted'] and not report['failures'] and not provider.errors
                            and report['allOwnedProcessesExited'] and len(report['bindings']) == min(args.roles, 19) * 50
                            and len(report['profileTurns']) == min(args.roles, 19) * 3 and len(report['contracts']) == 3)
        report['counts'] = {'bindings': len(report['bindings']), 'profileTurns': len(report['profileTurns']),
                            'contractsPassed': sum(x['status']=='passed' for x in report['contracts'])}
        (root / 'provider-requests.json').write_text(json.dumps(provider.requests, indent=2))
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps({'report': str(root / 'report.json'), 'passed': report['passed'], **report['counts']}), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
