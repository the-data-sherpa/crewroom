"""Offline test fixtures, not a Crewroom runner or a real model service.

Every OMP process has its own session, synthetic configuration and OS sandbox.
Only the allocated local provider port is reachable. No user configuration is copied.
"""
import hashlib
import shlex
import base64
import http.server
import json
import os
from pathlib import Path
import queue
import signal
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.parse
import uuid


def wait_until(predicate, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = predicate()
        if result:
            return result
        time.sleep(0.01)
    raise TimeoutError("Fixture deadline exceeded")


class Provider:
    def __init__(self, reasoning=False, port=0):
        self.reasoning = reasoning
        self.actions, self.requests, self.errors = {}, [], []
        self.lock = threading.Lock()
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                try:
                    body = json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))))
                    parts = urllib.parse.urlparse(self.path).path.strip('/').split('/')
                    agent, provider = parts[:2]
                    model = body.get('model')
                    if model is None and '/models/' in self.path:
                        model = urllib.parse.unquote(self.path.split('/models/', 1)[1].split(':streamGenerateContent')[0])
                    request = {'agent': agent, 'provider': provider, 'model': model,
                               'path': self.path, 'body': body, 'time': time.monotonic()}
                    with owner.lock:
                        owner.requests.append(request)
                    actions = owner.actions.get(agent)
                    action = actions.get_nowait() if actions else None
                    if action is None:
                        raise RuntimeError('Unscripted model request')
                    if callable(action):
                        action = action(request)
                    if action['kind'] == 'hold':
                        action['entered'].set()
                        action['release'].wait(20)
                        action = {'kind': 'text', 'text': 'RELEASED'}
                    if action['kind'] == 'error':
                        self.send_response(action['status'])
                        self.send_header('Content-Type', 'application/json')
                        for name, value in action.get('headers', {}).items(): self.send_header(name, value)
                        self.end_headers()
                        self.wfile.write(json.dumps({'error': action.get('error', {'type': 'fixture_error', 'message': 'Synthetic failure'})}).encode())
                        return
                    if action['kind'] == 'disconnect':
                        self.connection.shutdown(socket.SHUT_RDWR); self.connection.close(); return
                    if action['kind'] == 'malformed':
                        self.send_response(200); self.send_header('Content-Type', 'text/event-stream'); self.end_headers()
                        self.wfile.write(b'data: {invalid-json}\n\n'); return
                    if action['kind'] == 'empty':
                        self.send_response(200)
                        self.send_header('Content-Type', 'text/event-stream')
                        self.end_headers()
                        self.wfile.write(b'data: {"unexpected_fixture_event":true}\n\n')
                        return
                    events = owner.encode(provider, model, action)
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.send_header('Cache-Control', 'no-cache')
                    self.end_headers()
                    for event in events:
                        self.wfile.write((('event: ' + event['type'] + '\n') if 'type' in event else '').encode())
                        self.wfile.write(('data: ' + json.dumps(event) + '\n\n').encode())
                        self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Expected when an owned process is deliberately interrupted.
                except Exception as exc:
                    owner.errors.append(str(exc))
                    self.send_error(500, 'Unexpected fixture request')

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', port), Handler)
        self.port = self.server.server_port
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def enqueue(self, agent, *actions):
        with self.lock:
            target = self.actions.setdefault(agent, queue.Queue())
        for action in actions:
            target.put(action)

    def encode(self, provider, model, action):
        tool = action['kind'] == 'tool'
        ident = uuid.uuid4().hex
        if provider == 'aperture-anthropic':
            content = {'type': 'tool_use', 'id': 'toolu_' + ident, 'name': action['name'], 'input': {}} if tool else {'type': 'text', 'text': ''}
            delta = {'type': 'input_json_delta', 'partial_json': json.dumps(action['arguments'])} if tool else {'type': 'text_delta', 'text': action['text']}
            deltas = [delta]
            if tool and action.get('chunkSize'):
                raw = delta['partial_json']; size = action['chunkSize']
                deltas = [{'type': 'input_json_delta', 'partial_json': raw[index:index+size]} for index in range(0, len(raw), size)]
            events = [
                {'type': 'message_start', 'message': {'id': 'msg_' + ident, 'type': 'message', 'role': 'assistant', 'model': model, 'content': [], 'stop_reason': None, 'usage': {'input_tokens': 10, 'output_tokens': 0}}},
                {'type': 'content_block_start', 'index': 0, 'content_block': content},
                *[{'type': 'content_block_delta', 'index': 0, 'delta': item} for item in deltas],
                {'type': 'content_block_stop', 'index': 0},
                {'type': 'message_delta', 'delta': {'stop_reason': 'tool_use' if tool else 'end_turn', 'stop_sequence': None}, 'usage': {'output_tokens': 5}},
                {'type': 'message_stop'},
            ]
            if self.reasoning:
                for event in events:
                    if 'index' in event: event['index'] += 1
                events[1:1] = [{'type':'content_block_start','index':0,'content_block':{'type':'redacted_thinking','data':'fixture-opaque-' + ident}}, {'type':'content_block_stop','index':0}]
            return events
        if provider == 'aperture-google':
            part = {'functionCall': {'name': action['name'], 'args': action['arguments']}} if tool else {'text': action['text']}
            if self.reasoning: part['thoughtSignature'] = base64.b64encode(('fixture-opaque-' + ident).encode()).decode()
            return [{'candidates': [{'content': {'role': 'model', 'parts': [part]}, 'finishReason': 'STOP', 'index': 0}],
                     'usageMetadata': {'promptTokenCount': 10, 'candidatesTokenCount': 5, 'totalTokenCount': 15}, 'modelVersion': model}]
        response = {'id': 'resp_' + ident, 'object': 'response', 'status': 'completed', 'model': model,
                    'usage': {'input_tokens': 10, 'output_tokens': 5, 'total_tokens': 15}}
        if tool:
            item = {'type': 'function_call', 'id': 'fc_' + ident, 'call_id': 'call_' + ident, 'name': action['name'], 'arguments': ''}
            delta = {'type': 'response.function_call_arguments.delta', 'item_id': item['id'], 'output_index': 0, 'delta': json.dumps(action['arguments'])}
            done = {**item, 'arguments': json.dumps(action['arguments']), 'status': 'completed'}
        else:
            item = {'type': 'message', 'id': 'msg_' + ident, 'role': 'assistant', 'content': [], 'status': 'in_progress'}
            delta = {'type': 'response.output_text.delta', 'item_id': item['id'], 'output_index': 0, 'content_index': 0, 'delta': action['text']}
            done = {**item, 'status': 'completed', 'content': [{'type': 'output_text', 'text': action['text'], 'annotations': []}]}
        events = [
            {'type': 'response.created', 'response': {**response, 'status': 'in_progress', 'output': []}},
            {'type': 'response.output_item.added', 'output_index': 0, 'item': item}, delta,
            {'type': 'response.output_item.done', 'output_index': 0, 'item': done},
            {'type': 'response.completed', 'response': {**response, 'output': [done]}},
        ]
        if self.reasoning:
            reasoning = {'type':'reasoning','id':'rs_' + ident,'summary':[], 'encrypted_content':'fixture-opaque-' + ident}
            for event in events:
                if 'output_index' in event: event['output_index'] += 1
            events[1:1] = [{'type':'response.output_item.added','output_index':0,'item':reasoning}, {'type':'response.output_item.done','output_index':0,'item':reasoning}]
            events[-1]['response']['output'] = [reasoning, done]
        return events

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(2)


class Sandbox:
    def __init__(self, root, ports, listen_ports=()):
        self.root = root.resolve()
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        runtime = self.root / 'runtime'
        runtime.mkdir(exist_ok=True)
        self.binary = runtime / 'omp'
        self.linux = None
        if sys.platform == 'linux':
            from linux_offline_guard import LinuxGuard
            project = Path(__file__).resolve().parent.parent
            modules = Path(os.environ.get('CREWROOM_OMP_FIXTURE_MODULES', project / '.crewroom/mh2-omp-fixture/node_modules')).resolve()
            cli = modules / '@oh-my-pi/pi-coding-agent/dist/cli.js'
            bun = Path(shutil.which('bun') or Path.home() / '.bun/bin/bun').resolve()
            if not cli.is_file():
                raise RuntimeError('Install fixture-only OMP 18.4.12 per docs/testing/mh2-runtime-acceptance.md')
            self.binary.write_text('#!/bin/sh\nexec ' + shlex.quote(str(bun)) + ' ' + shlex.quote(str(cli)) + ' "$@"\n')
            self.binary.chmod(0o700)
            self.runtime_identity = {'version': subprocess.run([str(self.binary), '--version'], capture_output=True, text=True, check=True).stdout.strip(),
                                     'cliSha256': hashlib.sha256(cli.read_bytes()).hexdigest(), 'bunSha256': hashlib.sha256(bun.read_bytes()).hexdigest()}
            if self.runtime_identity['version'] != 'omp/18.4.12':
                raise RuntimeError('Fixture requires OMP 18.4.12')
            readonly = [modules, bun, Path(__file__).with_name('linux_offline_guard.py').resolve()]
            mail = project / '.crewroom/experiments/rust-mail-prep/runtime/mcp-agent-mail'
            if mail.exists(): readonly.append(mail.resolve())
            self.linux = LinuxGuard(self.root, ports, readonly)
            self.path = self.linux.path
            return
        shutil.copyfile(str(Path.home() / '.bun/bin/omp'), self.binary)
        self.binary.chmod(0o700)
        # The compiled loader uses ~/.omp/natives/<version> even with a custom agent
        # directory. Its supported adjacent-addon fallback avoids exposing that tree.
        version = subprocess.run([str(self.binary), '--version'], capture_output=True, text=True, timeout=10, check=True).stdout.strip().removeprefix('omp/')
        shutil.copyfile(Path.home() / '.omp/natives' / version / 'pi_natives.darwin-arm64.node',
                        runtime / 'pi_natives.darwin-arm64.node')
        home = Path.home()
        # SBPL accepts localhost (both loopback families), not a literal IPv4 host.
        rules = ['(version 1)', '(allow default)', '(deny network*)', '(deny file-write*)']
        rules += [f'(allow network-outbound (remote ip "localhost:{port}"))' for port in ports]
        rules += [f'(allow network-inbound (local ip "localhost:{port}"))' for port in listen_ports]
        rules += [f'(allow file-write* (subpath {json.dumps(str(self.root))}) (subpath "/dev"))']
        for private in ('.omp', '.pi', '.aws', '.ssh', '.config', 'Library/Keychains'):
            rules += [f'(deny file-read* (subpath {json.dumps(str(home / private))}))']
        self.path = self.root / 'offline.sb'
        self.path.write_text('\n'.join(rules) + '\n')

    def argv(self, command):
        if self.linux:
            return self.linux.argv(command)
        return ['/usr/bin/sandbox-exec', '-f', str(self.path), *command]

    def identity(self):
        if self.linux:
            return self.runtime_identity
        return {'version': subprocess.run([str(self.binary), '--version'], capture_output=True, text=True, check=True).stdout.strip(),
                'binarySha256': hashlib.sha256(self.binary.read_bytes()).hexdigest(),
                'addonSha256': hashlib.sha256((self.binary.parent / 'pi_natives.darwin-arm64.node').read_bytes()).hexdigest()}

    def verify(self, port):
        # The OS rejects these connects before any external packet is sent.
        script = f'''import socket,subprocess,sys,json
a=socket.create_connection(('127.0.0.1',{port}),2);a.close()
checks={{'fixture_port_allowed':True}}
for label,host,p in [('external_denied','192.0.2.1',443),('other_loopback_denied','127.0.0.1',1)]:
 try: socket.create_connection((host,p),2);checks[label]=False
 except PermissionError: checks[label]=True
 except OSError as e: checks[label]=e.errno in (101,111) if sys.platform=='linux' else False
r=subprocess.run([sys.executable,'-c',"import socket; socket.create_connection(('192.0.2.1',443),2)"],capture_output=True,text=True)
checks['child_egress_denied']=any(v in r.stderr for v in ('Operation not permitted', 'Network is unreachable'))
print(json.dumps(checks))
'''
        result = subprocess.run(self.argv([sys.executable, '-c', script]), capture_output=True, text=True, timeout=10)
        if result.returncode:
            raise RuntimeError('Offline guard failed: ' + result.stderr)
        checks = json.loads(result.stdout)
        if not all(checks.values()):
            raise RuntimeError('Offline guard ineffective: ' + json.dumps(checks))
        return checks

    def close(self):
        runtime = self.root / 'runtime'
        expected = {'omp'} if self.linux else {'omp', 'pi_natives.darwin-arm64.node'}
        if self.linux: self.linux.close()
        if runtime.is_symlink() or {p.name for p in runtime.iterdir()} != expected:
            raise RuntimeError('Unexpected runtime contents; refusing automatic cleanup')
        for name in expected:
            (runtime / name).unlink()
        runtime.rmdir()


class Agent:
    def __init__(self, name, root, sandbox, provider, catalog, selector, session=None, settings=None, process_factory=None):
        self.name, self.frames, self.responses, self.lock = name, [], {}, threading.Lock()
        self.condition = threading.Condition()
        self.root = root / name / uuid.uuid4().hex[:8]
        self.root.mkdir(parents=True)
        private = self.root / 'config'
        private.mkdir(mode=0o700)
        self.session_dir = root / name / 'sessions'
        self.session_dir.mkdir(exist_ok=True)
        providers = {}
        for m in catalog['models']:
            definition = {'id': m['id'], 'name': m['id'], 'reasoning': m['declaredReasoning'],
                          'input': m['declaredInput'], 'contextWindow': m['declaredContextWindow'],
                          'maxTokens': m['declaredMaxOutputTokens']}
            if m['declaredThinkingMode']:
                definition['thinking'] = {'mode': m['declaredThinkingMode'], 'efforts': m['declaredThinkingEfforts']}
            providers.setdefault(m['provider'], {'api': m['api'], 'apiKey': 'synthetic-offline-key',
                'baseUrl': f'http://127.0.0.1:{provider.port}/{name}/{m["provider"]}/v1', 'models': []})['models'].append(definition)
        (private / 'models.yml').write_text(json.dumps({'providers': providers}))
        (private / 'config.yml').write_text(json.dumps({'async': {'enabled': False}, **(settings or {})}))
        (private / 'mcp.json').write_text('{"mcpServers": {}}')
        env = {k: os.environ[k] for k in ('PATH', 'HOME', 'LANG', 'TMPDIR') if k in os.environ}
        env.update(PI_CODING_AGENT_DIR=str(private), PI_CODING_AGENT_SESSION_DIR=str(self.session_dir),
                   PI_CONFIG_DIR=os.path.relpath(self.root / 'omp-root', Path.home()),
                   XDG_DATA_HOME=str(root / 'xdg-data'), XDG_CACHE_HOME=str(root / 'xdg-cache'),
                   NO_COLOR='1', GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1')
        self.log = (self.root / 'rpc.jsonl').open('w')
        self.stderr = (self.root / 'stderr.log').open('w')
        args = [str(sandbox.binary), '--mode', 'rpc', '--no-ui', '--no-tools', '--no-extensions',
                '--no-rules', '--no-skills', '--no-lsp', '--no-title', '--no-pty', '--cwd', str(self.root),
                '--session-dir', str(self.session_dir), '--model', selector, '--thinking', 'off',
                '--system-prompt', f'Offline fixture role: {name}. Only use the explicitly assigned host tool.']
        if session:
            args += ['--session', session]
        self.proc = (process_factory or subprocess.Popen)(sandbox.argv(args), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=self.stderr, text=True, cwd=self.root, env=env, start_new_session=True)
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()

    def read(self):
        for line in self.proc.stdout:
            self.log.write(line)
            self.log.flush()
            try:
                frame = json.loads(line)
            except ValueError:
                continue
            with self.condition:
                self.frames.append(frame)
                if frame.get('type') == 'response' and frame.get('id') in self.responses:
                    self.responses[frame['id']].put(frame)
                self.condition.notify_all()

    def send(self, frame):
        with self.lock:
            self.proc.stdin.write(json.dumps(frame) + '\n')
            self.proc.stdin.flush()

    def command(self, kind, timeout=20, **kwargs):
        ident = uuid.uuid4().hex
        target = self.responses[ident] = queue.Queue()
        self.send({'type': kind, 'id': ident, **kwargs})
        try:
            end = time.monotonic() + timeout
            while True:
                try:
                    reply = target.get(timeout=min(0.1, max(0.01, end-time.monotonic())))
                    break
                except queue.Empty:
                    if self.proc.poll() is not None:
                        raise RuntimeError(f'{self.name}: process exited {self.proc.returncode}; see {self.root / "stderr.log"}') from None
                    if time.monotonic() >= end:
                        raise
            if not reply.get('success'):
                raise RuntimeError(f'{kind}: {reply.get("error")}')
            return reply.get('data')
        except queue.Empty:
            raise TimeoutError(f'{self.name}: {kind} timed out; exit={self.proc.poll()}') from None
        finally:
            self.responses.pop(ident, None)

    def wait_frame(self, kind, start=0, timeout=15):
        end = time.monotonic() + timeout
        with self.condition:
            while time.monotonic() < end:
                for frame in self.frames[start:]:
                    if frame.get('type') == kind:
                        return frame
                self.condition.wait(min(0.1, max(0, end-time.monotonic())))
        raise TimeoutError(f'{self.name}: missing {kind}')

    def text_turn(self, provider, text):
        start = len(self.frames)
        provider.enqueue(self.name, {'kind': 'text', 'text': text})
        self.command('prompt', message='Execute the assigned offline fixture response.')
        result = self.wait_frame('prompt_result', start)
        assert result['status'] == 'completed', result
        messages = self.command('get_messages')['messages']
        assistant = [m for m in messages if m.get('role') == 'assistant'][-1]
        actual = ''.join(p.get('text', '') for p in assistant.get('content', []) if p.get('type') == 'text')
        assert actual == text, (self.name, actual, text)
        return result

    def close(self, abrupt=False):
        if self.proc.poll() is None:
            if abrupt:
                os.killpg(self.proc.pid, signal.SIGKILL)
            else:
                self.proc.stdin.close()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.proc.pid, signal.SIGKILL)
                self.proc.wait(timeout=3)
        self.thread.join(2)
        self.log.close()
        self.stderr.close()
        self.proc.stdout.close()
        if not self.proc.stdin.closed:
            self.proc.stdin.close()
        return self.proc.returncode
