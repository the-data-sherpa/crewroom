#!/usr/bin/env python3
"""FOR-8 isolated actual-Claude protocol probe with a synthetic HTTP provider.

No operator login, real API key, repository effect or external MCP server is admitted.
Private raw traces stay in --output; the printed receipt contains assertions only.
"""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import threading
import time
import uuid


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")
    path.chmod(0o600)


class Provider:
    def __init__(self, blocks, block_selector=None):
        self.requests = []
        self.blocks = blocks
        self.errors = []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                if 'count_tokens' in self.path:
                    self.send_response(200)
                    self.end_headers()
                    self.wfile.write(b'{"input_tokens":10}')
                    return
                owner.requests.append(body)
                try:
                    block = block_selector(body) if block_selector else (
                        owner.blocks.pop(0) if owner.blocks else {'type': 'text', 'text': 'DONE'})
                except Exception as error:
                    owner.errors.append(str(error))
                    self.send_error(500, 'Scripted provider rejected request')
                    return
                ident = 'msg_' + uuid.uuid4().hex
                model = body['model']
                message = {'id': ident, 'type': 'message', 'role': 'assistant', 'model': model,
                           'content': [], 'stop_reason': None, 'stop_sequence': None,
                           'usage': {'input_tokens': 10, 'output_tokens': 0,
                                     'cache_creation_input_tokens': 0, 'cache_read_input_tokens': 0}}
                start = {**block, 'input': {}} if block['type'] == 'tool_use' else {'type': 'text', 'text': ''}
                delta = {'type': 'input_json_delta', 'partial_json': json.dumps(block['input'])} if block['type'] == 'tool_use' else {'type': 'text_delta', 'text': block['text']}
                events = [{'type': 'message_start', 'message': message},
                          {'type': 'content_block_start', 'index': 0, 'content_block': start},
                          {'type': 'content_block_delta', 'index': 0, 'delta': delta},
                          {'type': 'content_block_stop', 'index': 0},
                          {'type': 'message_delta', 'delta': {'stop_reason': 'tool_use' if block['type'] == 'tool_use' else 'end_turn', 'stop_sequence': None}, 'usage': {'output_tokens': 4}},
                          {'type': 'message_stop'}]
                data = ''.join('event: '+e['type']+'\ndata: '+json.dumps(e)+'\n\n' for e in events).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class Claude:
    def __init__(self, binary, root, provider, resume=None, safe=True, deny=True, native_home=None, tools=None):
        self.frames = []
        self.effects = []
        self.queue = queue.Queue()
        self.session = resume or str(uuid.uuid4())
        self.root = root
        self.deny = deny
        self.tools = tools or [{'name': 'echo', 'description': 'Echo a text value.', 'inputSchema': {'type': 'object', 'properties': {'text': {'type': 'string'}}, 'required': ['text'], 'additionalProperties': False}}]
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.cwd = root / 'work'
        self.cwd.mkdir(mode=0o700, exist_ok=True)
        self.args = ['-p', '--input-format', 'stream-json', '--output-format', 'stream-json',
                     '--verbose', '--include-partial-messages', '--replay-user-messages',
                     '--tools', '', '--strict-mcp-config', '--mcp-config',
                     json.dumps({'mcpServers': {'crewroom': {'type': 'sdk', 'name': 'crewroom'}}}),
                     '--setting-sources', '', '--disable-slash-commands', '--no-chrome',
                     '--model', 'sonnet', '--effort', 'low', '--permission-mode', 'dontAsk',
                     '--allowedTools', ','.join('mcp__crewroom__' + tool['name'] for tool in self.tools), '--system-prompt', 'Use only Crewroom tools.',
                     '--resume' if resume else '--session-id', self.session]
        if safe:
            self.args.insert(0, '--safe-mode')
        env = {'HOME': str(root), 'CLAUDE_CONFIG_DIR': str(root / 'config'), 'PATH': '/usr/bin:/bin',
               'LANG': 'C.UTF-8', 'ANTHROPIC_API_KEY': 'synthetic-not-a-credential',
               'ANTHROPIC_BASE_URL': 'http://127.0.0.1:'+str(provider.server.server_port),
               'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1', 'CLAUDE_CODE_RESUME_INTERRUPTED_TURN': '0'}
        if native_home:
            for key in ['CLAUDE_CONFIG_DIR', 'ANTHROPIC_API_KEY', 'ANTHROPIC_BASE_URL']:
                env.pop(key)
            env['HOME'] = str(native_home)
        self.child = subprocess.Popen([str(binary), *self.args], cwd=self.cwd, env=env, stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, start_new_session=True)
        def read():
            for line in self.child.stdout:
                try:
                    value = json.loads(line)
                except ValueError:
                    value = {'type': 'invalid'}
                self.frames.append(value)
                self.queue.put(value)
            self.queue.put({'type': 'eof'})
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def send(self, value):
        self.child.stdin.write(json.dumps(value)+'\n')
        self.child.stdin.flush()

    def respond(self, v):
        r = v['request']
        if r['subtype'] == 'mcp_message':
            m = r['message']
            method = m.get('method')
            if method == 'initialize':
                result = {'protocolVersion': '2025-11-25', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'crewroom', 'version': '0.1'}}
            elif method == 'tools/list':
                result = {'tools': self.tools}
            elif method == 'tools/call':
                self.effects.append(m)
                result = {'content': [{'type': 'text', 'text': 'RECEIPT'}]}
            else:
                result = {}
            response = {'mcp_response': {'jsonrpc': '2.0', 'id': m.get('id'), 'result': result}}
        elif r['subtype'] == 'hook_callback':
            # Intentional denial proves that auto-allowed tools still pass this gate.
            response = {'hookSpecificOutput': {'hookEventName': 'PreToolUse', 'permissionDecision': 'deny' if self.deny else 'allow', 'permissionDecisionReason': 'Fixture gate'}}
        elif r['subtype'] == 'can_use_tool':
            response = {'behavior': 'deny', 'message': 'Fixture denied'}
        else:
            raise AssertionError('unexpected control '+r['subtype'])
        self.send({'type': 'control_response', 'response': {'subtype': 'success', 'request_id': v['request_id'], 'response': response}})

    def wait(self, predicate, seconds=30):
        end = time.monotonic()+seconds
        while time.monotonic() < end:
            v = self.queue.get(timeout=max(0.01, end-time.monotonic()))
            if v['type'] == 'control_request':
                self.respond(v)
            if predicate(v):
                return v
            if v['type'] == 'eof':
                raise AssertionError('Claude exited')
        raise AssertionError('Claude timed out')

    def initialize(self):
        self.send({'type': 'control_request', 'request_id': 'init', 'request': {'subtype': 'initialize', 'sdkMcpServers': ['crewroom'], 'hooks': {'PreToolUse': [{'matcher': '.*', 'hookCallbackIds': ['gate']} ]}}})
        return self.wait(lambda v: v['type'] == 'control_response' and v['response']['request_id'] == 'init')

    def stop(self):
        if self.child.poll() is None:
            os.killpg(self.child.pid, signal.SIGTERM)
            try:
                self.child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.child.pid, signal.SIGKILL)
                self.child.wait()
        self.reader.join(timeout=2)
        for frame in self.frames:
            if frame.get('type') == 'control_response':
                frame.get('response', {}).get('response', {}).pop('account', None)
        save(self.root / 'frames.json', self.frames)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--claude', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--without-safe-mode', action='store_true')
    parser.add_argument('--allow-tool', action='store_true')
    parser.add_argument('--nullable-write-tool', action='store_true',
                        help='Synthetic provider sends valid null and string expectedHash values using the actual write_file schema')
    args = parser.parse_args()
    if args.nullable_write_tool and not args.allow_tool:
        parser.error('--nullable-write-tool requires --allow-tool; no repository effects are implemented')
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    tools = None
    expected_writes = []
    if args.nullable_write_tool:
        definition = json.loads((Path(__file__).resolve().parents[1] / 'crates/crewroom-runtime/src/host-tools.json').read_text())['definitions']['write_file']
        tools = [{'name': 'write_file', 'description': definition['description'], 'inputSchema': definition['parameters']}]
        expected_writes = [{'path': 'fixture.cjs', 'content': 'new fixture source', 'expectedHash': None},
                           {'path': 'fixture.cjs', 'content': 'changed fixture source', 'expectedHash': 'a' * 64}]
        blocks = [{'type': 'tool_use', 'id': 'call_write_' + str(index), 'name': 'mcp__crewroom__write_file', 'input': value}
                  for index, value in enumerate(expected_writes)]
    else:
        blocks = [{'type': 'tool_use', 'id': 'call_allowed', 'name': 'mcp__crewroom__echo', 'input': {'text': 'DENY_ME'}},
                  {'type': 'tool_use', 'id': 'call_forbidden', 'name': 'Bash', 'input': {'command': 'touch '+str(root / 'forbidden')}}]
    provider = Provider(blocks)
    agent = Claude(args.claude.resolve(), root / 'agent', provider, safe=not args.without_safe_mode, deny=not args.allow_tool, tools=tools)
    try:
        init = agent.initialize()
        ident = str(uuid.uuid4())
        agent.send({'type': 'user', 'uuid': ident, 'session_id': agent.session, 'message': {'role': 'user', 'content': 'Exercise the fixture tools.'}, 'parent_tool_use_id': None})
        result = agent.wait(lambda v: v['type'] == 'result')
        report = {'scope': 'actual Claude, isolated fake API credentials and synthetic provider',
                  'binarySha256': hashlib.sha256(args.claude.read_bytes()).hexdigest(),
                  'safeMode': not args.without_safe_mode, 'terminal': result.get('subtype'),
                  'toolCatalogs': [[t['name'] for t in r.get('tools', [])] for r in provider.requests],
                  'hookCalls': sum(v.get('request', {}).get('subtype') == 'hook_callback' for v in agent.frames),
                  'effects': len(agent.effects), 'forbiddenFileExists': (root / 'forbidden').exists(),
                  'inputAcknowledged': any(v.get('type') == 'user' and v.get('uuid') == ident for v in agent.frames),
                  'initializationSucceeded': init['response']['subtype'] == 'success'}
        if args.nullable_write_tool:
            report.update(scope='Actual Claude executable SDK protocol with synthetic provider and fake SDK tool results; no live inference, repository writes or production Rust adapter execution',
                          actualToolSchemaSha256=hashlib.sha256(json.dumps(tools[0]['inputSchema'], sort_keys=True, separators=(',', ':')).encode()).hexdigest(),
                          writeArgumentsPreserved=[effect['params']['arguments'] for effect in agent.effects] == expected_writes,
                          nullableSchemaPreserved=all(request.get('tools', [{}])[0].get('input_schema', {}).get('properties', {}).get('expectedHash', {}).get('type') == ['string', 'null'] for request in provider.requests),
                          repositoryWrites=0)
            report['passed'] = report['terminal'] == 'success' and report['initializationSucceeded'] and report['inputAcknowledged'] and report['hookCalls'] == 2 and report['effects'] == 2 and report['writeArgumentsPreserved'] and report['nullableSchemaPreserved']
        save(root / 'result.json', report)
        print(json.dumps(report, indent=2))
        if args.nullable_write_tool:
            assert report['passed'], 'Nullable write tool protocol contract failed; inspect retained receipt'
    finally:
        agent.stop()
        provider.close()
        save(root / 'requests.json', provider.requests)


if __name__ == '__main__':
    main()
