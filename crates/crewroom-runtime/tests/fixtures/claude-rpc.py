#!/usr/bin/python3
"""Synthetic protocol peer, not Claude or authentication evidence."""
import json
import os
import pathlib
import sys
import time
import uuid

if '--version' in sys.argv:
    print('2.1.296 (Claude Code)')
    raise SystemExit

mode = pathlib.Path('mode').read_text()
flag = '--resume' if '--resume' in sys.argv else '--session-id'
session = sys.argv[sys.argv.index(flag) + 1]
model = 'claude-sonnet-5-5'
encoded = ''.join(c if c.isascii() and c.isalnum() else '-' for c in str(pathlib.Path.cwd()))
history = pathlib.Path(os.environ['HOME']) / '.claude/projects' / encoded / (session + '.jsonl')
leaf = None
if history.exists():
    for row in map(json.loads, history.read_text().splitlines()):
        if 'parentUuid' in row:
            leaf = row['uuid']
consumed = []
sequence = 0


def emit(v):
    print(json.dumps(v), flush=True)


def saved(row):
    global leaf
    row.update(sessionId=session, parentUuid=leaf, uuid=row.get('uuid', str(uuid.uuid4())), isSidechain=False)
    leaf = row['uuid']
    with history.open('a') as file:
        file.write(json.dumps(row) + '\n')
        file.flush()
        os.fsync(file.fileno())


def control(ident, request):
    emit({'type': 'control_request', 'request_id': ident, 'request': request})


def lifecycle(wire, state):
    emit({'type': 'command_lifecycle', 'command_uuid': wire, 'state': state, 'session_id': session})


def terminal():
    result = {'type': 'result', 'session_id': session, 'user_message_uuid': consumed[-1], 'user_message_uuids': consumed,
              'subtype': 'success', 'is_error': mode == 'provider-error', 'terminal_reason': 'aborted_streaming' if mode == 'text-interrupted' else 'aborted_tools' if mode == 'tool-unresolved' else 'completed'}
    if mode != 'lost-usage':
        result['modelUsage'] = {model: {'inputTokens': 10, 'outputTokens': 3, 'cacheReadInputTokens': 0, 'cacheCreationInputTokens': 0, 'costUSD': 0.001}}
    emit(result)
    if mode == 'duplicate-result':
        emit(result)
    if mode == 'changed-result':
        emit({**result, 'is_error': True})
    consumed.clear()


for line in sys.stdin:
    v = json.loads(line)
    if v.get('request', {}).get('subtype') == 'initialize':
        emit({'type': 'control_response', 'response': {'subtype': 'success', 'request_id': v['request_id'], 'response': {'models': [{'value': 'sonnet', 'resolvedModel': 'changed' if mode == 'alias-change' else model, 'supportedEffortLevels': ['low']}]}}})
    if v.get('type') == 'user':
        wire = v['uuid']
        sequence += 1
        if mode == 'unconfirmed':
            continue
        if mode != 'terminal-before-queued':
            lifecycle(wire, 'queued')
        lifecycle(wire, 'started')
        consumed.append(wire)
        saved({'type': 'user', 'uuid': wire, 'message': v['message']})
        if mode == 'folded':
            if len(consumed) == 2:
                lifecycle(wire, 'completed')
                terminal()
            continue
        if mode == 'terminal-before-queued':
            terminal()
            lifecycle(wire, 'queued')
            continue
        emit({'type': 'system', 'subtype': 'init', 'session_id': session, 'model': model, 'tools': ['mcp__crewroom__echo'], 'permissionMode': 'dontAsk', 'claude_code_version': '2.1.296'})
        if mode == 'malformed':
            print('{broken', flush=True)
            continue
        if mode == 'oversized':
            print('x' * (8*1024*1024 + 1), flush=True)
            continue
        if mode == 'partial-eof':
            print('{', end='', flush=True)
            break
        if mode == 'partial-stall':
            print('{', end='', flush=True)
            time.sleep(30)
            continue
        if mode == 'eof':
            break
        if mode == 'wrong-usage-session':
            emit({'type': 'result', 'session_id': 'other', 'user_message_uuid': wire, 'modelUsage': {model: {'inputTokens': 999}}})
            terminal()  # Buffered success after rejection must not survive.
            continue
        message_id = 'message' + str(sequence)
        call = 'call' + str(sequence)
        emit({'type': 'stream_event', 'session_id': session, 'api_message_id': message_id, 'event': {'type': 'message_start', 'message': {'id': message_id, 'model': model}}})
        args = {'text': 17 if mode == 'schema' else 'value'}
        message = {'id': message_id, 'role': 'assistant', 'model': 'changed' if mode == 'model-change' else model, 'content': [{'type': 'tool_use', 'id': call, 'name': 'Bash' if mode == 'native-tool' else 'mcp__crewroom__echo', 'input': args}]}
        saved({'type': 'assistant', 'message': message})
        emit({'type': 'assistant', 'session_id': session, 'user_message_uuid': 'stale' if mode == 'wrong-turn' else wire, 'message': message})
        if mode == 'skip-gate':
            control('effect', {'subtype': 'mcp_message', 'server_name': 'crewroom', 'message': {'jsonrpc': '2.0', 'id': 7, 'method': 'tools/call', 'params': {'name': 'echo', 'arguments': args, '_meta': {'claudecode/toolUseId': call}}}})
            continue
        control('gate', {'subtype': 'hook_callback', 'callback_id': 'crewroom-gate', 'tool_use_id': call, 'input': {'hook_event_name': 'PreToolUse', 'session_id': 'other' if mode == 'wrong-session' else session, 'tool_use_id': call, 'cwd': str(pathlib.Path.cwd()), 'effort': {'level': 'high' if mode == 'effort-change' else 'low'}, 'tool_name': 'mcp__crewroom__echo', 'tool_input': args}})
    if v.get('type') == 'control_response' and v['response']['request_id'] == 'gate':
        effect = {'subtype': 'mcp_message', 'server_name': 'other' if mode == 'wrong-server' else 'crewroom', 'message': {'jsonrpc': '2.0', 'id': 7, 'method': 'tools/call', 'params': {'name': 'echo', 'arguments': {'text': 'changed' if mode == 'changed-arguments' else 'value'}, '_meta': {'claudecode/toolUseId': 'stale' if mode == 'stale-call' else call}}}}
        control('effect', effect)
        if mode in ['tool-unresolved', 'premature-success']:
            terminal()
        if mode == 'duplicate-request':
            control('effect', effect)
    if v.get('type') == 'control_response' and v['response']['request_id'] == 'effect':
        if mode != 'receipt-not-delivered':
            result = v['response']['response']['mcp_response']['result']
            saved({'type': 'user', 'message': {'role': 'user', 'content': [{'type': 'tool_result', 'tool_use_id': call, 'content': result['content'], 'is_error': result['isError']}]}})
        terminal()
