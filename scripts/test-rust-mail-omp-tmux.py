#!/usr/bin/env python3
"""Real Rust Mail + independent OMP processes in tmux; scripted providers only."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time
import traceback
import urllib.request
import uuid

from mailbox_contract_fixture import PROJECT, RustMail, free_port
from omp_offline_fixture import Provider, Sandbox, wait_until

spec = importlib.util.spec_from_file_location('mail_contract', PROJECT / 'scripts/test-rust-mail-offline.py')
contract = importlib.util.module_from_spec(spec); spec.loader.exec_module(contract)
spec = importlib.util.spec_from_file_location('message_probe', PROJECT / 'scripts/probe-omp-external-messages.py')
probe = importlib.util.module_from_spec(spec); spec.loader.exec_module(probe)


class PaneAgent:
    def __init__(self, name, root, tmux, sandbox, provider, port, model, session=None):
        self.name, self.port, self.token = name, port, uuid.uuid4().hex
        self.config = root / f'{name}-{uuid.uuid4().hex[:8]}.json'
        config = {'name': name, 'root': str(root), 'binary': str(sandbox.binary),
                  'sandbox': str(sandbox.path), 'providerPort': provider.port, 'port': port,
                  'catalog': str(PROJECT / 'docs/testing/fixtures/omp-model-catalog-2026-09-26.json'),
                  'model': model, 'token': self.token, 'session': session}
        self.config.write_text(json.dumps(config)); self.config.chmod(0o600)
        command = sandbox.argv([sys.executable, '-u', str(PROJECT / 'scripts/omp_mailbox_pane_fixture.py'), str(self.config)])
        if tmux('has-session', check=False).returncode:
            result = tmux('new-session', '-d', '-s', 'crewroom-test', '-n', name, '-P', '-F', '#{pane_id}', *command)
        else:
            result = tmux('new-window', '-d', '-n', name, '-P', '-F', '#{pane_id}', *command)
        self.pane = result.stdout.strip()
        tmux('set-option', '-w', '-t', self.pane, 'remain-on-exit', 'on')
        self.closed = False
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            try:
                self.identity = self.request('identity'); break
            except OSError:
                time.sleep(.05)
        else:
            raise TimeoutError(f'{name} host did not start in pane {self.pane}')
        self.command('set_host_tools', tools=[probe.TOOL])

    def request(self, op, **kwargs):
        req = urllib.request.Request(f'http://127.0.0.1:{self.port}/',
            data=json.dumps({'op': op, **kwargs}).encode(),
            headers={'Content-Type': 'application/json', 'Authorization': 'Bearer ' + self.token})
        with urllib.request.urlopen(req, timeout=25) as response:
            result = json.load(response)
        if 'error' in result:
            raise RuntimeError(result['error'])
        return result['result']

    def command(self, kind, **arguments):
        return self.request('command', kind=kind, arguments=arguments)

    @property
    def frames(self):
        return self.request('frames')

    def wait_frame(self, kind, start=0):
        return wait_until(lambda: next((f for f in self.request('frames', start=start) if f.get('type') == kind), None), 20)

    def send(self, frame):
        return self.request('send', frame=frame)

    def close(self, abrupt=False):
        if not self.closed:
            self.request('close', abrupt=abrupt)
            self.closed = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve(); root.mkdir(parents=True, exist_ok=False, mode=0o700)
    provider = Provider(); port = free_port()
    names = ['Coordinator', 'SpecialistOne', 'SpecialistTwo']
    ports = {name: free_port() for name in names}
    sandbox = Sandbox(root, [provider.port, port, *ports.values()], [port, *ports.values()])
    mailbox = RustMail(root / 'mail', sandbox, port)
    socket_path = root / 'tmux.sock'
    env = {k: os.environ[k] for k in ('PATH', 'HOME', 'LANG') if k in os.environ}
    env['TMPDIR'] = str(root)

    def tmux(*arguments, check=True):
        return subprocess.run(['tmux', '-S', str(socket_path), '-f', '/dev/null', *map(str, arguments)],
                              env=env, text=True, capture_output=True, timeout=15, check=check)

    models = {'Coordinator': 'aperture-openai/openai.gpt-6-sol',
              'SpecialistOne': 'aperture-anthropic/anthropic.claude-sonnet-5',
              'SpecialistTwo': 'aperture-google/gemini-3.8-flash'}
    addresses = dict(zip(names, ['BlueLake', 'GreenCastle', 'RedPine']))
    all_agents, crew = [], {}
    report = {'scope': 'Test adapter, real OMP/tmux/Rust Mail, scripted providers; not production workflow acceptance.',
              'contracts': [], 'failures': [], 'processes': [], 'messages': [], 'identity': mailbox.identity()}

    def case(name, fn):
        try:
            evidence = fn(); item = {'name': name, 'status': 'passed', 'evidence': evidence}
        except Exception as exc:
            item = {'name': name, 'status': 'failed', 'error': str(exc)}
            report['failures'].append({**item, 'trace': traceback.format_exc()})
        report['contracts'].append(item)
        print(json.dumps({k: v for k, v in item.items() if k != 'evidence'}), flush=True)

    def launch(name, session=None):
        a = PaneAgent(name, root, tmux, sandbox, provider, ports[name], models[name], session)
        all_agents.append(a); crew[name] = a
        state = a.command('get_state')
        record = {**a.identity, 'pane': a.pane, 'modelSelector': models[name], 'session': state}
        report['processes'].append(record)
        return a

    def text_turn(name, prompt, answer):
        a = crew[name]; start = len(a.frames)
        provider.enqueue(name, {'kind': 'text', 'text': answer})
        a.command('prompt', message=prompt)
        assert a.wait_frame('prompt_result', start)['status'] == 'completed'
        assert answer in json.dumps(a.command('get_messages'))

    def ack(name, mid):
        mailbox.call('acknowledge_message', project_key=mailbox.project, agent_name=addresses[name], message_id=mid)

    try:
        report['guard'] = sandbox.verify(provider.port)
        mailbox.start(); contract.setup(mailbox)
        for name in names:
            launch(name)

        def topology():
            assert len({a.identity['ompPid'] for a in crew.values()}) == 3
            pane_rows = tmux('list-panes', '-a', '-F', '#{pane_id} #{pane_pid} #{pane_dead}').stdout
            for name, a in crew.items():
                assert f'{a.pane} {a.identity["hostPid"]} 0' in pane_rows
                parent = subprocess.check_output(['ps', '-o', 'ppid=', '-p', str(a.identity['ompPid'])], text=True).strip()
                assert int(parent) == a.identity['hostPid']
                state = a.command('get_state')
                assert state['model']['provider'] + '/' + state['model']['id'] == models[name]
                text_turn(name, 'Produce the assigned offline checkpoint.', 'CHECKPOINT_' + name)
                actual = [r for r in provider.requests if r['agent'] == name]
                assert actual[-1]['provider'] + '/' + actual[-1]['model'] == models[name]
            return {'paneRows': pane_rows, 'differentOmpPids': True, 'modelsObservedAtProvider': models}
        case('Three tmux-hosted OMP processes use three assigned provider/model identities', topology)

        def roundtrip():
            hops = [('Coordinator', 'SpecialistOne'), ('SpecialistOne', 'SpecialistTwo'),
                    ('SpecialistTwo', 'SpecialistOne'), ('SpecialistOne', 'Coordinator')]
            receipts = []
            for index, (sender, recipient) in enumerate(hops):
                marker = f'TMUX_HOP_{index}_{uuid.uuid4().hex[:6]}'
                a = crew[sender]; start = len(a.frames)
                provider.enqueue(sender, {'kind': 'tool', 'name': 'send_message', 'arguments': {'to': recipient, 'body': marker}},
                                 {'kind': 'text', 'text': 'SENT'})
                a.command('prompt', message='Execute the scripted peer message step.')
                call = a.wait_frame('host_tool_call', start)
                assert call['arguments'] == {'to': recipient, 'body': marker}
                args = contract.send_args(mailbox, 'hop-' + call['toolCallId'], marker, addresses[sender], addresses[recipient])
                mid = contract.message_id(mailbox.call('send_message', **args))
                a.send({'type': 'host_tool_result', 'id': call['id'],
                        'result': {'content': [{'type': 'text', 'text': json.dumps({'stored': True, 'message_id': mid})}]}})
                assert a.wait_frame('prompt_result', start)['status'] == 'completed'
                incoming = next(m for m in contract.inbox(mailbox, addresses[recipient]) if m['id'] == mid)
                assert incoming['body_md'] == marker and incoming['from'] == addresses[sender]
                text_turn(recipient, json.dumps({'message_id': mid, 'from': sender, 'body': marker}), 'RECEIVED_' + str(index))
                ack(recipient, mid)
                assert contract.receipt(mailbox, mid)['recipients'][0]['acknowledged']
                assert any(e['message_id'] == mid for e in contract.events(mailbox, addresses[recipient], after=0)['events'])
                receipts.append({'from': sender, 'to': recipient, 'messageId': mid, 'body': marker})
            report['messages'].extend(receipts)
            assert all(not any(f.get('type') in ('irc_message', 'subagent_event', 'subagent_lifecycle') for f in a.frames) for a in crew.values())
            return {'hops': receipts, 'allAcknowledged': True, 'passiveObserverSawAll': True, 'nativeTeamBusUnused': True}
        case('Four peer-message hops pass through Rust Mail outside OMP team bus', roundtrip)

        def passive_terminal():
            snapshots = {}
            unread_before = {n: contract.inbox(mailbox, addresses[n], unread_only=True) for n in names}
            clients = []
            try:
                # Attach actual control-mode clients twice, then disconnect each.
                for _ in range(2):
                    client = subprocess.Popen(['tmux', '-S', str(socket_path), '-C', 'attach-session', '-t', 'crewroom-test'],
                                              env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    clients.append(client)
                    output, error = client.communicate('list-panes -a\ndetach-client\n', timeout=5)
                    assert '%begin' in output and '%end' in output, (output, error)
                for name, a in crew.items():
                    output = tmux('capture-pane', '-p', '-S', '-', '-t', a.pane).stdout
                    assert 'host_ready' in output and 'prompt_result' in output
                    snapshots[name] = output
                    (root / f'{name}-pane.txt').write_text(output)
                    assert a.command('get_state')['sessionId'] == a.identity['session']['sessionId']
                assert unread_before == {n: contract.inbox(mailbox, addresses[n], unread_only=True) for n in names}
                return {'controlClientsAttachedAndDetached': 2, 'panesWithRealEvents': list(snapshots), 'sameSessions': True}
            finally:
                for c in clients:
                    if c.poll() is None:
                        c.kill(); c.wait(timeout=3)
        case('Terminal viewers reconnect without consuming messages or replacing agents', passive_terminal)

        def paused_queue():
            # Admission is test-adapter policy, not a Rust Mail pause feature.
            args = contract.send_args(mailbox, 'PAUSED_INPUT', recipient=addresses['SpecialistTwo'])
            mid = contract.message_id(mailbox.call('send_message', **args))
            count = len(provider.requests)
            assert not contract.receipt(mailbox, mid)['recipients'][0]['acknowledged']
            contract.events(mailbox, addresses['SpecialistTwo'], after=0)
            time.sleep(.2)
            assert len(provider.requests) == count
            text_turn('SpecialistTwo', 'Resume fixture with stored message ' + str(mid), 'RESUMED_FROM_QUEUE')
            ack('SpecialistTwo', mid)
            assert len(provider.requests) == count + 1
            return {'messageId': mid, 'noProviderCallBeforeExplicitDispatch': True, 'resumedOnce': True,
                    'scope': 'Test adapter holds dispatch; production pause controller is unimplemented.'}
        case('Stored mail does not execute a paused recipient before adapter dispatch', paused_queue)

        def first_turn_recovery():
            # Replace an existing recipient with a fresh session, then kill that
            # fresh session during its first turn, before OMP persisted its input.
            old = crew['SpecialistTwo']; old.close(abrupt=True)
            wait_until(lambda: tmux('display-message', '-p', '-t', old.pane, '#{pane_dead}').stdout.strip() == '1')
            a = launch('SpecialistTwo')
            marker = 'FIRST_UNFINISHED_TURN_FROM_DURABLE_MAIL'
            mid = contract.message_id(mailbox.call('send_message', **contract.send_args(mailbox, marker, recipient=addresses['SpecialistTwo'])))
            entered, release = threading.Event(), threading.Event()
            provider.enqueue('SpecialistTwo', {'kind': 'hold', 'entered': entered, 'release': release})
            a.command('prompt', message=marker)
            assert entered.wait(10)
            before = a.command('get_state')
            session_file = Path(before['sessionFile'])
            on_disk = session_file.exists() and marker in session_file.read_text()
            assert marker in json.dumps(a.command('get_messages'))
            a.close(abrupt=True); release.set()
            wait_until(lambda: tmux('display-message', '-p', '-t', a.pane, '#{pane_dead}').stdout.strip() == '1')
            retained = next(m for m in contract.inbox(mailbox, addresses['SpecialistTwo']) if m['id'] == mid)
            assert retained['body_md'] == marker
            assert not contract.receipt(mailbox, mid)['recipients'][0]['acknowledged']
            replacement = launch('SpecialistTwo')
            assert replacement.identity['ompPid'] != a.identity['ompPid']
            text_turn('SpecialistTwo', retained['body_md'], 'RECOVERED_SAVED_INPUT')
            ack('SpecialistTwo', mid)
            messages = replacement.command('get_messages')['messages']
            assert sum(marker in json.dumps(m) for m in messages if m.get('role') == 'user') == 1
            return {'messageId': mid, 'ompInputOnDiskBeforeKill': on_disk, 'mailRetainedInput': True,
                    'oldSession': before['sessionId'], 'replacementSession': replacement.command('get_state')['sessionId'],
                    'inputOccurrencesInReplacement': 1, 'scope': 'Explicit fixture replacement after killed process; no unknown external effect.'}
        case('First-turn OMP crash recovers saved mail into a separate replacement process', first_turn_recovery)
    except Exception as exc:
        report['failures'].append({'name': 'harness', 'error': str(exc), 'trace': traceback.format_exc()})
    finally:
        for a in all_agents:
            if not a.closed:
                try:
                    a.close(abrupt=True)
                except Exception:
                    for key in ('ompPid', 'hostPid'):
                        try:
                            os.kill(a.identity[key], signal.SIGKILL)
                        except ProcessLookupError:
                            pass
        tmux('kill-server', check=False)
        mailbox.stop(); provider.close()
        report['providerErrors'] = provider.errors
        report['providerRequests'] = [{'agent': r['agent'], 'provider': r['provider'], 'model': r['model']} for r in provider.requests]
        remaining = []
        for a in all_agents:
            for key in ('ompPid', 'hostPid'):
                try:
                    os.kill(a.identity[key], 0); remaining.append(a.identity[key])
                except ProcessLookupError:
                    pass
        report['remainingPids'] = remaining
        report['allOwnedProcessesExited'] = not remaining and (mailbox.proc is None or mailbox.proc.poll() is not None)
        if report['allOwnedProcessesExited']:
            sandbox.close(); report['disposableOmpCopiesRemoved'] = True
        report['passed'] = not report['failures'] and not provider.errors and report['allOwnedProcessesExited'] and len(report['contracts']) == 5
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps({'report': str(root / 'report.json'), 'passed': report['passed'], 'failures': len(report['failures'])}), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
