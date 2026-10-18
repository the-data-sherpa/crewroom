#!/usr/bin/env python3
"""Exercise published Rust Agent Mail through its HTTP MCP API, offline."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import http.client
import json
from pathlib import Path
import sys
import time
import traceback
import urllib.error

from mailbox_contract_fixture import NetworkGuard, RustMail, free_port
from omp_offline_fixture import Sandbox


def setup(mail):
    mail.call('ensure_project', human_key=mail.project)
    for name in ('BlueLake', 'GreenCastle', 'RedPine'):
        mail.call('register_agent', project_key=mail.project, name=name, program='omp', model='offline-fixture')
        mail.call('set_contact_policy', project_key=mail.project, agent_name=name, policy='open')


def send_args(mail, key, body=None, sender='BlueLake', recipient='GreenCastle'):
    return dict(project_key=mail.project, sender_name=sender, to=[recipient], subject=key,
                body_md=body or key, ack_required=True, idempotency_key=key)


def message_id(result):
    assert result['count'] == 1, result
    return result['deliveries'][0]['payload']['id']


def inbox(mail, name='GreenCastle', **kwargs):
    return mail.call('fetch_inbox', project_key=mail.project, agent_name=name,
                     include_bodies=True, mark_read=False, limit=100, **kwargs)


def events(mail, name='GreenCastle', **kwargs):
    return mail.call('fetch_inbox_events', project_key=mail.project, agent_name=name, **kwargs)


def receipt(mail, mid):
    return mail.call('get_message_delivery_receipt', project_key=mail.project, message_id=mid)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve(); root.mkdir(parents=True, exist_ok=False, mode=0o700)
    port = free_port(); expiry_port = free_port()
    guard = NetworkGuard(root, [port, expiry_port], [port, expiry_port])
    mail = RustMail(root / 'mail', guard, port)
    expiry = RustMail(root / 'expiry', guard, expiry_port, retention=2)
    report = {'scope': 'Real Rust Agent Mail HTTP MCP API; no models or live external services.',
              'identity': mail.identity(), 'contracts': [], 'failures': []}

    def case(name, fn):
        start = time.monotonic()
        try:
            evidence = fn()
            item = {'name': name, 'status': 'passed', 'evidence': evidence}
        except Exception as exc:
            item = {'name': name, 'status': 'failed', 'error': str(exc)}
            report['failures'].append({**item, 'trace': traceback.format_exc()})
        item['seconds'] = round(time.monotonic() - start, 3)
        report['contracts'].append(item)
        print(json.dumps({k: v for k, v in item.items() if k != 'evidence'}), flush=True)

    try:
        mail.start()
        report['guard'] = Sandbox.verify(guard, port)
        setup(mail)

        def basic():
            mid = message_id(mail.call('send_message', **send_args(mail, 'stored-1')))
            before = receipt(mail, mid)
            unread_before = inbox(mail, unread_only=True)
            first = events(mail, after=0)
            second = events(mail, after=0)
            assert first == second
            assert any(e['message_id'] == mid for e in first['events'])
            assert unread_before == inbox(mail, unread_only=True)
            assert before == receipt(mail, mid)
            assert before['persisted'] and not before['recipients'][0]['acknowledged']
            ack = mail.call('acknowledge_message', project_key=mail.project, agent_name='GreenCastle', message_id=mid)
            assert receipt(mail, mid)['recipients'][0]['acknowledged']
            return {'messageId': mid, 'passiveEvents': True, 'passiveInboxOptOut': True, 'ack': ack}
        case('Stored message, passive observer and explicit acknowledgement', basic)

        def duplicate():
            args = send_args(mail, 'dedupe-1')
            first = mail.call('send_message', **args)
            second = mail.call('send_message', **args)
            assert message_id(first) == message_id(second)
            assert len([m for m in inbox(mail) if m['subject'] == 'dedupe-1']) == 1
            try:
                mail.call('send_message', **{**args, 'body_md': 'changed-payload'})
            except RuntimeError as exc:
                assert 'idempotenc' in str(exc).lower() and ('conflict' in str(exc).lower() or 'different' in str(exc).lower()), str(exc)
            else:
                raise AssertionError('Reused key accepted a different payload')
            return {'messageId': message_id(first), 'oneMessage': True, 'payloadConflictRejected': True}
        case('Repeated send returns original identity and rejects changed payload', duplicate)

        def concurrent():
            args = send_args(mail, 'concurrent-1')
            with ThreadPoolExecutor(max_workers=4) as pool:
                replies = list(pool.map(lambda _: mail.call('send_message', **args), range(4)))
            ids = [message_id(r) for r in replies]
            assert len(set(ids)) == 1, ids
            assert len([m for m in inbox(mail) if m['subject'] == 'concurrent-1']) == 1
            assert len([e for e in events(mail, after=0)['events'] if e['message_id'] == ids[0]]) == 1
            return {'concurrentRequests': 4, 'messageIds': ids, 'oneDeliveryEvent': True}
        case('Concurrent sends with one key create one message and delivery event', concurrent)

        def lost_reply():
            args = send_args(mail, 'lost-response-1')
            # Receive headers but discard the complete MCP response body. The
            # retrying sender never receives the operation's result/identity.
            connection = http.client.HTTPConnection('127.0.0.1', port, timeout=12)
            frame = {'jsonrpc': '2.0', 'id': 'lost-reply', 'method': 'tools/call',
                     'params': {'name': 'send_message', 'arguments': args}}
            headers = {'Content-Type': 'application/json', 'Accept': 'application/json, text/event-stream',
                       'Authorization': 'Bearer ' + mail.token}
            if mail.session:
                headers['Mcp-Session-Id'] = mail.session
            connection.request('POST', '/mcp/', json.dumps(frame), headers)
            response = connection.getresponse()
            assert response.status == 200
            response.close(); connection.close()
            observed = [m for m in inbox(mail) if m['subject'] == 'lost-response-1']
            assert len(observed) == 1
            original = observed[0]['id']
            mail.stop(abrupt=True); mail.start()
            repeated = message_id(mail.call('send_message', **args))
            assert repeated == original, {'original': original, 'repeated': repeated}
            matches = [e for e in events(mail, after=0)['events'] if e['message_id'] == original]
            assert len(matches) == 1
            assert len([m for m in inbox(mail) if m['subject'] == 'lost-response-1']) == 1
            assert not receipt(mail, original)['recipients'][0]['acknowledged']
            return {'original': original, 'repeated': repeated, 'discardedMcpResponse': True,
                    'mailboxKilledWithSIGKILL': True, 'oneDeliveryEvent': True, 'pendingRetained': True}
        case('Lost send response and abrupt restart preserve one message', lost_reply)

        def acknowledged_restart():
            mid = message_id(mail.call('send_message', **send_args(mail, 'ack-restart')))
            mail.call('acknowledge_message', project_key=mail.project, agent_name='GreenCastle', message_id=mid,
                      idempotency_key='ack-restart-key')
            before = receipt(mail, mid)
            mail.stop(abrupt=True); mail.start()
            assert receipt(mail, mid) == before
            mail.call('acknowledge_message', project_key=mail.project, agent_name='GreenCastle', message_id=mid,
                      idempotency_key='ack-restart-key')
            assert receipt(mail, mid) == before
            return {'messageId': mid, 'ackSurvivedRestartAndReplay': True}
        case('Acknowledgement survives restart and repeated acknowledgement', acknowledged_restart)

        def pagination():
            expected = []
            for index in range(7):
                expected.append(message_id(mail.call('send_message', **send_args(mail, f'page-{index}', recipient='RedPine'))))
            first = events(mail, 'RedPine', after=0, limit=2)
            assert first['has_more']
            cursor_file = root / 'observer-cursor.json'
            cursor_file.write_text(json.dumps({'cursor': first['next_cursor']}))
            unread_before = inbox(mail, 'RedPine', unread_only=True)
            mail.stop(abrupt=True); mail.start()
            cursor = json.loads(cursor_file.read_text())['cursor']
            seen = [e['message_id'] for e in first['events']]
            for _ in range(10):
                page = events(mail, 'RedPine', after=cursor, limit=2)
                seen.extend(e['message_id'] for e in page['events'])
                assert page['next_cursor'] >= cursor
                cursor = page['next_cursor']
                if not page['has_more']:
                    break
            assert seen == expected, {'seen': seen, 'expected': expected}
            assert unread_before == inbox(mail, 'RedPine', unread_only=True)
            assert all(not receipt(mail, mid)['recipients'][0]['acknowledged'] for mid in expected)
            return {'messageIds': seen, 'savedCursor': first['next_cursor'], 'observerResumedWithoutConsuming': True}
        case('Observer resumes paginated event cursor after service restart', pagination)

        def invalid_recipient():
            try:
                mail.call('send_message', **send_args(mail, 'unknown-recipient', recipient='SilverGhost'))
            except RuntimeError as exc:
                assert 'SilverGhost' in str(exc), str(exc)
                return {'unknownRecipientRejected': True}
            raise AssertionError('Unknown recipient accepted')
        case('Unknown recipient is rejected', invalid_recipient)

        def auth():
            token = mail.token; mail.token = 'invalid-fixture-token'
            try:
                mail.rpc('tools/list', {})
            except urllib.error.HTTPError as exc:
                assert exc.code in (401, 403)
                return {'invalidBearerDenied': True, 'httpStatus': exc.code}
            finally:
                mail.token = token
            raise AssertionError('Invalid bearer accepted')
        case('Invalid service credential is rejected', auth)

        def expiration():
            expiry.start(); setup(expiry)
            args = send_args(expiry, 'expiry-1')
            original = message_id(expiry.call('send_message', **args))
            time.sleep(2.2)
            expiry.stop(abrupt=True); expiry.start()
            repeated = message_id(expiry.call('send_message', **args))
            assert repeated != original, 'Expected documented bounded retry retention'
            assert {m['id'] for m in inbox(expiry) if m['subject'] == 'expiry-1'} == {original, repeated}
            return {'configuredRetentionSeconds': 2, 'original': original, 'repeated': repeated,
                    'boundedRetryProtectionConfirmed': True,
                    'requiredPolicy': 'Reconcile retained logical identity after retention; do not blindly resend.'}
        case('Expired retry keys are explicitly characterized, not treated as unlimited dedupe', expiration)
    except Exception as exc:
        report['failures'].append({'name': 'harness', 'error': str(exc), 'trace': traceback.format_exc()})
    finally:
        mail.stop(); expiry.stop()
        report['mailboxGenerations'] = mail.generations
        report['allOwnedProcessesExited'] = all(m.proc is None or m.proc.poll() is not None for m in (mail, expiry))
        report['passed'] = not report['failures'] and len(report['contracts']) == 9 and report['allOwnedProcessesExited']
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps({'report': str(root / 'report.json'), 'passed': report['passed'], 'failures': len(report['failures'])}), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
