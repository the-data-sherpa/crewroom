#!/usr/bin/env python3
"""Test local Relaycast delivery and real OMP recovery with scripted model replies.

No production mailbox, credentials or model provider is used. The evaluated
Relaycast package must already exist in the ignored experiment runtime.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import socket
import subprocess
import sys
import threading
import traceback
import uuid

from omp_offline_fixture import Agent, Provider, Sandbox, wait_until

PROJECT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('message_probe', PROJECT / 'scripts/probe-omp-external-messages.py')
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class Mailbox:
    def __init__(self, root, sandbox, port):
        self.root, self.sandbox, self.port = root, sandbox, port
        runtime = PROJECT / '.crewroom/experiments/external-mailbox-prep/runtime'
        (root / 'node_modules').symlink_to(runtime / 'node_modules', target_is_directory=True)
        shutil.copyfile(PROJECT / 'scripts/serve-relaycast-probe.mjs', root / 'relay.mjs')
        (root / 'relay-data').mkdir()
        self.node = shutil.which('node')
        self.proc = None
        self.generations = []

    def start(self):
        self.err = (self.root / f'relay-{len(self.generations)}.stderr').open('w')
        env = {k: os.environ[k] for k in ('PATH', 'HOME', 'LANG', 'TMPDIR') if k in os.environ}
        env.update(CREWROOM_RELAY_PORT=str(self.port), CREWROOM_RELAY_DATA=str(self.root / 'relay-data'))
        self.proc = subprocess.Popen(self.sandbox.argv([self.node, str(self.root / 'relay.mjs')]),
                                     cwd=self.root, env=env, stdout=subprocess.PIPE, stderr=self.err,
                                     text=True, start_new_session=True)
        ready = queue.Queue()
        threading.Thread(target=lambda: ready.put(self.proc.stdout.readline()), daemon=True).start()
        line = ready.get(timeout=10)
        assert line, f'Relaycast startup failed; see {self.err.name}'
        event = json.loads(line)
        assert event['event'] == 'listening', event
        self.generations.append(event)

    def stop(self, abrupt=False):
        if self.proc and self.proc.poll() is None:
            os.killpg(self.proc.pid, signal.SIGKILL if abrupt else signal.SIGTERM)
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.proc.pid, signal.SIGKILL)
                self.proc.wait(timeout=3)
        if self.proc:
            self.proc.stdout.close()
            self.err.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    catalog = json.loads((PROJECT / 'docs/testing/fixtures/omp-model-catalog-2026-09-26.json').read_text())
    provider = Provider()
    sock = socket.socket(); sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]; sock.close()
    sandbox = Sandbox(root, [provider.port, port], [port])
    report = {'scope': 'Local Relaycast and real OMP; scripted model responses; no Crewroom workflow claim.',
              'contracts': [], 'failures': [], 'processes': []}
    mailbox = Mailbox(root, sandbox, port)
    agents = []

    def result(name, fn):
        try:
            evidence = fn()
            report['contracts'].append({'name': name, 'status': 'passed', 'evidence': evidence})
            print(json.dumps({'contract': name, 'status': 'passed'}), flush=True)
        except Exception as exc:
            report['contracts'].append({'name': name, 'status': 'failed', 'error': str(exc)})
            report['failures'].append({'name': name, 'error': str(exc), 'trace': traceback.format_exc()})
            print(json.dumps({'contract': name, 'status': 'failed', 'error': str(exc)}), flush=True)

    def launch(name, selector, session=None):
        a = Agent(name, root, sandbox, provider, catalog, selector, session)
        agents.append(a)
        state = a.command('get_state')
        report['processes'].append({'name': name, 'pid': a.proc.pid, 'sessionId': state['sessionId']})
        a.command('set_host_tools', tools=[probe.TOOL])
        return a

    try:
        report['guard'] = sandbox.verify(provider.port)
        mailbox.start()
        relay = probe.Relay(f'http://127.0.0.1:{port}', root)
        relay.setup()

        def observer_and_dedupe():
            message = relay.send('Coordinator', 'SpecialistOne', 'fixture-delivery', 'same-action-1')
            duplicate = relay.send('Coordinator', 'SpecialistOne', 'fixture-delivery', 'same-action-1')
            assert message['id'] == duplicate['id']
            before = relay.deliveries('SpecialistOne')
            relay.observe(); relay.observe()
            after = relay.deliveries('SpecialistOne')
            assert before == after
            item = next(d for d in after if d['message']['id'] == message['id'])
            assert item['status'] == 'queued', item
            ack = relay.ack('SpecialistOne', item['id'])
            assert ack['status'] == 'acked'
            assert not relay.deliveries('SpecialistOne')
            return {'messageId': message['id'], 'sameIdempotentSend': True, 'passiveObserver': True, 'acknowledged': True}
        result('Stored delivery, same-process dedupe and passive observation', observer_and_dedupe)

        def pending_restart():
            m = relay.send('SpecialistOne', 'SpecialistTwo', 'survive-mailbox-kill', 'restart-action-1')
            before = next(d for d in relay.deliveries('SpecialistTwo') if d['message']['id'] == m['id'])
            mailbox.stop(abrupt=True); mailbox.start()
            after = next(d for d in relay.deliveries('SpecialistTwo') if d['message']['id'] == m['id'])
            assert before['id'] == after['id'] and after['status'] == 'queued'
            relay.ack('SpecialistTwo', after['id'])
            return {'deliveryId': after['id'], 'pendingSurvivedRestart': True}
        result('Pending delivery survives abrupt mailbox restart', pending_restart)

        def lost_reply_restart():
            # Do not acknowledge/reconcile the send locally before killing service.
            m = relay.send('Coordinator', 'SpecialistOne', 'lost-send-reply', 'lost-reply-key')
            mailbox.stop(abrupt=True); mailbox.start()
            repeated = relay.send('Coordinator', 'SpecialistOne', 'lost-send-reply', 'lost-reply-key')
            report['restartIdempotencyObservation'] = {'original': m['id'], 'repeated': repeated['id']}
            for d in relay.deliveries('SpecialistOne'):
                relay.ack('SpecialistOne', d['id'])
            assert repeated['id'] == m['id'], 'Same idempotency key created a second message after service restart'
            return {'messageId': m['id'], 'oneStoredMessage': True}
        result('Lost send reply remains idempotent across mailbox restart', lost_reply_restart)

        def observer_cannot_write():
            try:
                relay.request('POST', '/v1/dm', {'to': 'SpecialistOne', 'text': 'forbidden'}, token=relay.observer_token)
            except RuntimeError as exc:
                assert 'HTTP 403' in str(exc) or 'HTTP 401' in str(exc), str(exc)
                return {'writeDenied': True}
            raise AssertionError('Observer token was allowed to send')
        result('Observer token cannot send a message', observer_cannot_write)

        def first_turn_durability():
            name = 'first-turn-durability'
            model = 'aperture-openai/openai.gpt-6-sol'
            a = launch(name, model)
            entered, release = threading.Event(), threading.Event()
            provider.enqueue(name, {'kind': 'hold', 'entered': entered, 'release': release})
            marker = 'FIRST_TURN_MUST_SURVIVE'
            a.command('prompt', message=marker)
            assert entered.wait(10)
            assert marker in json.dumps(a.command('get_messages'))
            before = a.command('get_state')
            session_path = Path(before['sessionFile'])
            stored = session_path.read_text() if session_path.exists() else ''
            a.close(abrupt=True); release.set()
            b = launch(name, model, session=before['sessionFile'])
            after = b.command('get_state')
            recovered = b.command('get_messages')
            observation = {'rpcInputObserved': True, 'inputOnDiskBeforeKill': marker in stored,
                           'oldSessionId': before['sessionId'], 'newSessionId': after['sessionId'],
                           'inputRecovered': marker in json.dumps(recovered)}
            report['firstTurnObservation'] = observation
            b.close()
            assert observation['inputRecovered'] and before['sessionId'] == after['sessionId'], \
                'First unfinished turn: RPC-visible input was not recovered with the same session identity'
            return observation
        result('First unfinished turn retains RPC-visible input after kill', first_turn_durability)

        def roundtrip_and_receipt_recovery():
            models = {'Coordinator': 'aperture-openai/openai.gpt-6-sol',
                      'SpecialistOne': 'aperture-anthropic/anthropic.claude-sonnet-5',
                      'SpecialistTwo': 'aperture-google/gemini-3.8-flash'}
            crew = {name: launch(name, model) for name, model in models.items()}
            assert len({a.proc.pid for a in crew.values()}) == 3
            # Establish an actual on-disk session before the receipt-boundary
            # experiment. The separate first-turn test above covers that gap.
            crew['SpecialistTwo'].text_turn(provider, 'INITIALIZED_SESSION_CHECKPOINT')
            hops = [('Coordinator', 'SpecialistOne'), ('SpecialistOne', 'SpecialistTwo'),
                    ('SpecialistTwo', 'SpecialistOne'), ('SpecialistOne', 'Coordinator')]
            receipts = []
            for index, (sender, recipient) in enumerate(hops):
                a = crew[sender]
                marker = f'offline-hop-{index}-{uuid.uuid4().hex[:8]}'
                provider.enqueue(sender, {'kind': 'tool', 'name': 'send_message', 'arguments': {'to': recipient, 'body': marker}},
                                 {'kind': 'text', 'text': 'SENT'})
                start = len(a.frames)
                a.command('prompt', message='Execute the next scripted message step.')
                call = a.wait_frame('host_tool_call', start)
                assert call['toolName'] == 'send_message'
                assert call['arguments'] == {'to': recipient, 'body': marker}
                stored = relay.send(sender, recipient, marker, f'{sender}-{call["toolCallId"]}')
                a.send({'type': 'host_tool_result', 'id': call['id'], 'result': {'content': [{'type': 'text', 'text': json.dumps({'stored': stored['id']})}]}})
                assert a.wait_frame('prompt_result', start)['status'] == 'completed'
                delivery = next(d for d in relay.deliveries(recipient) if d['message']['id'] == stored['id'])
                received = {'message_id': stored['id'], 'from': sender, 'body': delivery['message']['text']}
                assert received['body'] == marker
                b = crew[recipient]
                if index == 1:
                    # Stop S2 after OMP persisted the incoming message, before its
                    # model reply or the external delivery acknowledgement.
                    entered, release = threading.Event(), threading.Event()
                    provider.enqueue(recipient, {'kind': 'hold', 'entered': entered, 'release': release})
                    b.command('prompt', message=json.dumps(received))
                    assert entered.wait(10)
                    messages = b.command('get_messages')['messages']
                    assert any(marker in json.dumps(m) for m in messages if m.get('role') == 'user')
                    state = b.command('get_state')
                    assert marker in Path(state['sessionFile']).read_text(), 'Input not durable before injected kill'
                    b.close(abrupt=True); release.set()
                    assert crew['Coordinator'].proc.poll() is None and crew['SpecialistOne'].proc.poll() is None
                    b = crew[recipient] = launch(recipient, models[recipient], session=state['sessionFile'])
                    assert b.command('get_state')['sessionId'] == state['sessionId']
                    messages = b.command('get_messages')['messages']
                    occurrences = sum(marker in json.dumps(m) for m in messages if m.get('role') == 'user')
                    assert occurrences == 1
                    assert any(d['id'] == delivery['id'] and d['status'] == 'queued' for d in relay.deliveries(recipient))
                    # Experimental reconciliation checks actual recipient history;
                    # it does not redeliver an already persisted input.
                else:
                    provider.enqueue(recipient, {'kind': 'text', 'text': 'RECEIVED'})
                    start_b = len(b.frames)
                    b.command('prompt', message=json.dumps(received))
                    assert b.wait_frame('prompt_result', start_b)['status'] == 'completed'
                receipt = relay.ack(recipient, delivery['id'])
                assert receipt['status'] == 'acked'
                receipts.append({'sender': sender, 'recipient': recipient, 'messageId': stored['id'], 'deliveryId': delivery['id']})
                if index == 2:
                    coordinator = crew['Coordinator']
                    before = coordinator.command('get_state')
                    coordinator.close(abrupt=True)
                    crew['Coordinator'] = launch('Coordinator', models['Coordinator'], session=before['sessionFile'])
                    assert crew['Coordinator'].command('get_state')['sessionId'] == before['sessionId']
                    assert crew['SpecialistOne'].proc.poll() is None and crew['SpecialistTwo'].proc.poll() is None
            relay.observe()
            for r in receipts:
                assert any(r['messageId'] in json.dumps(e) for e in relay.observer_events)
            assert not any(relay.deliveries(name) for name in models)
            for a in crew.values():
                a.close()
            return {'hops': receipts, 'specialistKilledAfterDurableReceipt': True, 'recipientInputOccurrences': 1,
                    'coordinatorProcessRestartedBeforeFinalReply': True,
                    'noPendingDeliveries': True, 'observerSawAllMessages': True,
                    'limitation': 'Reconciliation is performed by this test adapter. Crewroom durable inbox/ownership remains unimplemented.'}
        result('Four external hops with specialist crash before acknowledgement', roundtrip_and_receipt_recovery)
    except Exception as exc:
        report['failures'].append({'name': 'harness', 'error': str(exc), 'trace': traceback.format_exc()})
    finally:
        report['interrupted'] = sys.exc_info()[0] is not None
        for a in agents:
            if not a.log.closed:
                a.close(abrupt=True)
        mailbox.stop()
        provider.close()
        report['mailboxGenerations'] = mailbox.generations
        report['providerErrors'] = provider.errors
        report['allOwnedProcessesExited'] = all(a.proc.poll() is not None for a in agents) and (not mailbox.proc or mailbox.proc.poll() is not None)
        if report['allOwnedProcessesExited']:
            sandbox.close()
            report['disposableRuntimeRemoved'] = True
        report['passed'] = (not report['interrupted'] and not report['failures'] and not provider.errors
                            and report['allOwnedProcessesExited'] and len(report['contracts']) == 6)
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        (root / 'provider-requests.json').write_text(json.dumps(provider.requests, indent=2) + '\n')
        print(json.dumps({'report': str(root / 'report.json'), 'passed': report['passed'], 'failures': len(report['failures'])}), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
