#!/usr/bin/env python3
"""FOR-72 recorded harness session transcripts: record, check, diff.

`record` runs the REAL installed Codex, Claude Code or OMP executable through
Crewroom's production Rust adapter (`codex-fixture` example) for one synthetic
scenario. A tee shim stands in for the executable and captures every protocol
line in both directions; model output comes from a loopback synthetic provider,
so no operator login, account or private prompt is involved. The transcript is
stamped with the harness `--version` and executable identity (file name,
SHA-256, size), normalized (paths, UUIDs) and redacted, then checked by the same
gate as `check` before it is written. Nothing unchecked is written to the repo.

`check` is the redaction gate run by `npm test` on every committed transcript.
It fails on credentials, account identifiers, home paths, raw UUIDs or
non-synthetic tasks.

`diff` renders two transcripts (for example the same scenario re-recorded on a
new harness version) as protocol shapes and prints a unified diff of what
changed: version and executable identity, frame order, frame kinds and fields.

Replays of these transcripts (crates/crewroom-runtime/tests/harness_replay.rs)
are SIMULATED checks. They never count as live delivery.

    cargo build -p crewroom-runtime --bins --examples
    python3 scripts/harness_transcripts.py record --harness claude-code --binary "$(command -v claude)" --scenario tool-completion
    python3 scripts/harness_transcripts.py check
    python3 scripts/harness_transcripts.py diff OLD.jsonl NEW.jsonl
"""
import argparse
import base64
import difflib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import queue
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid

PROJECT = Path(__file__).resolve().parent.parent
TRANSCRIPTS = PROJECT / 'crates/crewroom-runtime/tests/fixtures/harness-transcripts'
FORMAT = 'crewroom-harness-transcript-v1'
HARNESSES = ('codex', 'claude-code', 'omp')
BINARY_NAMES = {'codex': 'codex', 'claude-code': 'claude', 'omp': 'omp'}
MARKER = 'Synthetic fixture'
TOOLS = [{'name': 'echo', 'description': 'Return the addressed synthetic value.',
          'parameters': {'type': 'object', 'properties': {'text': {'type': 'string'}},
                         'required': ['text'], 'additionalProperties': False}}]
PROMPT = 'Synthetic fixture assignment. Use only the supplied Crewroom tools.'
UUID = re.compile(r'[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}')
PLACEHOLDER = re.compile(r'<<[a-z-]+(?::\d+)?>>')
SECRET_KEYS = re.compile(r'(token|apikey|api_key|authorization|password|secret|cookie|accountid|account_id|'
                         r'accountuuid|organizationuuid|organization_uuid|email|userid|user_id)$', re.I)
FORBIDDEN = [
    ('home path', re.compile(r'/(home|Users)/[^/<\s"]+|/root/')),
    ('JSON web token', re.compile(r'eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{4,}')),
    ('API key', re.compile(r'\b(sk-(ant-)?[A-Za-z0-9_-]{16,}|ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[abpr]-[A-Za-z0-9-]{10,})')),
    ('bearer credential', re.compile(r'Bearer\s+[A-Za-z0-9._~+/=-]{8,}')),
    ('email address', re.compile(r'[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}')),
    ('raw UUID (account or session identifier)', UUID),
]
# Adapter events whose per-type sequence a replay must reproduce.
EXPECTED_EVENTS = ('opened', 'ready', 'setup_failed', 'agent_start', 'agent_end', 'host_tool_call', 'host_tool_cancel',
                   'prompt_result', 'model_policy_rejected', 'harness_compaction', 'message_end')


# ---------------------------------------------------------------- scenarios

def scenario(harness, name):
    """Host steps shared by the recorder and the Rust replay test, plus the
    synthetic provider script. Every task text carries MARKER."""
    omp = harness == 'omp'
    done = (lambda ident, status=None: {'op': 'wait', 'type': 'agent_end'}) if omp else \
        (lambda ident, status=None: {'op': 'wait', 'type': 'prompt_result', 'id': ident, **({'status': status} if status else {})})
    tool = [{'op': 'wait', 'type': 'host_tool_call'}, {'op': 'tool', 'text': 'SYNTHETIC_RESULT'}]
    call = {'kind': 'tool', 'name': 'echo', 'arguments': {'text': 'SYNTHETIC_VALUE'}}
    answer = {'kind': 'text', 'text': 'Synthetic fixture answer: SYNTHETIC_RESULT'}
    ask = 'Synthetic fixture task: call echo with text SYNTHETIC_VALUE, then report its result.'
    if name == 'tool-completion':
        steps = [{'op': 'open'}, {'op': 'input', 'id': 'input-1', 'message': ask}, *tool,
                 done('input-1', 'completed'), {'op': 'close'}]
        return steps, [call, answer]
    if name == 'steer-interrupt':
        steps = [{'op': 'open'},
                 {'op': 'input', 'id': 'input-1', 'message': 'Synthetic fixture task: stream a long synthetic answer.'},
                 {'op': 'wait', 'type': 'message_update'},
                 {'op': 'input', 'id': 'input-2', 'steer': True, 'message': 'Synthetic fixture steering: keep streaming.'},
                 {'op': 'pause', 'ms': 300},
                 {'op': 'interrupt'}, done('input-1'), {'op': 'close'}]
        # The harness decides whether steering restarts the request; both
        # requests stream slowly so the interrupt lands during output.
        return steps, [{'kind': 'slow', 'chunks': 60, 'delay': 0.1}, {'kind': 'slow', 'chunks': 60, 'delay': 0.1}]
    if name == 'reattach':
        steps = [{'op': 'open'}, {'op': 'input', 'id': 'input-1', 'message': ask}, *tool,
                 done('input-1', 'completed'), {'op': 'close'}, {'op': 'open', 'resume': True},
                 {'op': 'input', 'id': 'input-2', 'message': 'Synthetic fixture task: repeat the previous result without tools.'},
                 done('input-2', 'completed'), {'op': 'close'}]
        return steps, [call, answer, {'kind': 'text', 'text': 'Synthetic fixture repeat: SYNTHETIC_RESULT'}]
    raise SystemExit(f'Unknown scenario {name}')


SCENARIOS = ('tool-completion', 'steer-interrupt', 'reattach')


# --------------------------------------------------------- synthetic provider

class Provider:
    """Loopback model provider. Speaks Anthropic Messages and OpenAI Responses
    streaming with scripted synthetic output; it never forwards anything."""

    def __init__(self, actions):
        self.actions, self.requests, self.errors, self.sequence = list(actions), [], [], 0
        owner = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def do_GET(self):
                self.send_response(404)
                self.send_header('Content-Length', '0')
                self.end_headers()

            def do_POST(self):
                try:
                    raw = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                    if self.headers.get('Content-Encoding') == 'zstd':
                        raw = subprocess.run(['zstd', '-dc'], input=raw, capture_output=True, check=True).stdout
                    body = json.loads(raw or b'{}')
                    if 'count_tokens' in self.path:
                        return self.json({'input_tokens': 10})
                    route = self.path.split('?')[0].rstrip('/')
                    if not (route.endswith('/responses') or route.endswith('/messages')):
                        return self.json({})  # Telemetry and other side calls get no model output.
                    owner.requests.append({'path': self.path, 'model': body.get('model')})
                    action = owner.actions.pop(0) if owner.actions else {'kind': 'text', 'text': 'Synthetic fixture answer.'}
                    owner.sequence += 1
                    if route.endswith('/responses'):
                        events = owner.responses(body, action)
                    else:
                        events = owner.anthropic(body, action)
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/event-stream')
                    self.send_header('Cache-Control', 'no-cache')
                    self.send_header('Connection', 'close')
                    self.end_headers()
                    for event, delay in events:
                        if delay:
                            time.sleep(delay)
                        self.wfile.write(('event: ' + event['type'] + '\ndata: ' + json.dumps(event) + '\n\n').encode())
                        self.wfile.flush()
                    self.close_connection = True
                except (BrokenPipeError, ConnectionResetError):
                    pass
                except Exception as error:  # Retained for the recorder's failure report.
                    owner.errors.append(f'{self.path}: {error}')
                    self.send_error(500, 'Synthetic provider rejected request')

            def json(self, value):
                data = json.dumps(value).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.origin = f'http://127.0.0.1:{self.server.server_port}'
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    @staticmethod
    def tool_name(body, wanted):
        names = [t.get('name') or (t.get('function') or {}).get('name') for t in body.get('tools') or []]
        return next((n for n in names if n and (n == wanted or n.endswith('__' + wanted) or n.endswith('_' + wanted))), wanted)

    def anthropic(self, body, action):
        ident = f'msg_synthetic_{self.sequence}'
        message = {'id': ident, 'type': 'message', 'role': 'assistant', 'model': body.get('model'), 'content': [],
                   'stop_reason': None, 'stop_sequence': None,
                   'usage': {'input_tokens': 10, 'output_tokens': 0, 'cache_creation_input_tokens': 0, 'cache_read_input_tokens': 0}}
        events = [({'type': 'message_start', 'message': message}, 0)]
        if action['kind'] == 'tool':
            block = {'type': 'tool_use', 'id': f'toolu_synthetic_{self.sequence}', 'name': self.tool_name(body, action['name']), 'input': {}}
            deltas = [{'type': 'input_json_delta', 'partial_json': json.dumps(action['arguments'])}]
            stop = 'tool_use'
        else:
            block = {'type': 'text', 'text': ''}
            texts = [action['text']] if action['kind'] == 'text' else [f'Synthetic line {i}. ' for i in range(action['chunks'])]
            deltas = [{'type': 'text_delta', 'text': t} for t in texts]
            stop = 'end_turn'
        delay = action.get('delay', 0)
        events.append(({'type': 'content_block_start', 'index': 0, 'content_block': block}, 0))
        events += [({'type': 'content_block_delta', 'index': 0, 'delta': d}, delay) for d in deltas]
        events += [({'type': 'content_block_stop', 'index': 0}, 0),
                   ({'type': 'message_delta', 'delta': {'stop_reason': stop, 'stop_sequence': None}, 'usage': {'output_tokens': 4}}, 0),
                   ({'type': 'message_stop'}, 0)]
        return events

    def responses(self, body, action):
        ident = f'resp_synthetic_{self.sequence}'
        base = {'id': ident, 'object': 'response', 'created_at': 1, 'model': body.get('model'), 'status': 'in_progress', 'output': []}
        events = [({'type': 'response.created', 'response': base}, 0)]
        if action['kind'] == 'tool':
            item = {'type': 'function_call', 'id': f'fc_synthetic_{self.sequence}', 'call_id': f'call_synthetic_{self.sequence}',
                    'name': self.tool_name(body, action['name']), 'arguments': json.dumps(action['arguments']), 'status': 'completed'}
            events += [({'type': 'response.output_item.added', 'output_index': 0, 'item': {**item, 'arguments': '', 'status': 'in_progress'}}, 0),
                       ({'type': 'response.function_call_arguments.delta', 'output_index': 0, 'item_id': item['id'], 'delta': item['arguments']}, 0),
                       ({'type': 'response.output_item.done', 'output_index': 0, 'item': item}, 0)]
        else:
            texts = [action['text']] if action['kind'] == 'text' else [f'Synthetic line {i}. ' for i in range(action['chunks'])]
            item = {'type': 'message', 'id': f'msg_synthetic_{self.sequence}', 'role': 'assistant', 'status': 'in_progress', 'content': []}
            events.append(({'type': 'response.output_item.added', 'output_index': 0, 'item': item}, 0))
            events.append(({'type': 'response.content_part.added', 'output_index': 0, 'item_id': item['id'], 'content_index': 0,
                            'part': {'type': 'output_text', 'text': '', 'annotations': []}}, 0))
            events += [({'type': 'response.output_text.delta', 'output_index': 0, 'item_id': item['id'], 'content_index': 0, 'delta': t},
                        action.get('delay', 0)) for t in texts]
            full = ''.join(texts)
            done = {**item, 'status': 'completed', 'content': [{'type': 'output_text', 'text': full, 'annotations': []}]}
            events += [({'type': 'response.output_text.done', 'output_index': 0, 'item_id': item['id'], 'content_index': 0, 'text': full}, 0),
                       ({'type': 'response.output_item.done', 'output_index': 0, 'item': done}, 0)]
        usage = {'input_tokens': 10, 'input_tokens_details': {'cached_tokens': 0}, 'output_tokens': 4,
                 'output_tokens_details': {'reasoning_tokens': 0}, 'total_tokens': 14}
        events.append(({'type': 'response.completed', 'response': {**base, 'status': 'completed', 'usage': usage}}, 0))
        return events


# ------------------------------------------------------------------ the shim

SHIM = r'''#!/usr/bin/python3
"""FOR-72 recording tee. Runs the real harness and logs each protocol line."""
import json, os, subprocess, sys, threading, uuid
REAL = %(real)r
LOG = %(log)r
EXTRA_ENV = %(env)r
EXTRA_ARGS = %(args)r
SESSION = %(session)r
lock = threading.Lock()
inv = str(uuid.uuid4())
def log(kind, **fields):
    with lock, open(LOG, 'a') as out:
        out.write(json.dumps({'inv': inv, 'kind': kind, **fields}) + '\n')
argv = sys.argv[1:]
env = {**os.environ, **EXTRA_ENV}
if not any(flag in argv for flag in SESSION):
    done = subprocess.run([REAL, *argv], env=env, stdin=subprocess.DEVNULL, capture_output=True)
    log('exec', argv=argv, stdout=done.stdout.decode('utf-8', 'replace'), code=done.returncode)
    sys.stdout.write(done.stdout.decode('utf-8', 'replace')); sys.stdout.flush()
    raise SystemExit(done.returncode)
log('start', argv=argv, cwd=os.getcwd())
child = subprocess.Popen([REAL, *argv, *EXTRA_ARGS], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
def pump_in():
    for line in sys.stdin.buffer:
        log('send', line=line.decode('utf-8', 'replace'))
        try:
            child.stdin.write(line); child.stdin.flush()
        except BrokenPipeError:
            break
    try:
        child.stdin.close()
    except BrokenPipeError:
        pass
threading.Thread(target=pump_in, daemon=True).start()
for line in child.stdout:
    log('recv', line=line.decode('utf-8', 'replace'))
    sys.stdout.buffer.write(line); sys.stdout.buffer.flush()
code = child.wait()
log('exit', code=code)
raise SystemExit(code)
'''


def shim(path, real, log, env, args, session):
    path.write_text(SHIM % {'real': str(real), 'log': str(log), 'env': env, 'args': args, 'session': session})
    path.chmod(0o700)


# ----------------------------------------------------------------- recording

def private(path):
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)
    return path


def fake_token():
    claims = base64.urlsafe_b64encode(json.dumps({'exp': 4102444800}).encode()).rstrip(b'=').decode()
    return f'synthetic.{claims}.signature'


class Driver:
    """Crewroom's production adapter in the `codex-fixture` example process."""

    def __init__(self, root, config):
        self.events, self.queue = [], queue.Queue()
        (root / 'config.json').write_text(json.dumps(config))
        (root / 'config.json').chmod(0o600)
        self.stderr = open(root / 'driver.log', 'w')
        self.child = subprocess.Popen([str(PROJECT / 'target/debug/examples/codex-fixture'), str(root / 'config.json')],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr, text=True)

        def read():
            for line in self.child.stdout:
                try:
                    value = json.loads(line)
                except ValueError:
                    value = {'type': 'invalid_frame'}
                self.events.append(value)
                self.queue.put(value)
            self.queue.put({'type': 'eof'})
        threading.Thread(target=read, daemon=True).start()

    def send(self, value):
        self.child.stdin.write(json.dumps(value) + '\n')
        self.child.stdin.flush()

    def wait(self, predicate, seconds=90):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                value = self.queue.get(timeout=max(0.01, deadline - time.monotonic()))
            except queue.Empty:
                break
            if predicate(value):
                return value
            if value['type'] in ('setup_failed', 'eof', 'model_policy_rejected'):
                raise AssertionError(f'adapter stopped: {value}')
        raise AssertionError('timed out waiting for the adapter')

    def close(self):
        if self.child.poll() is None:
            self.send({'op': 'close'})
            try:
                self.child.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait()
        self.stderr.close()


def matches(step, value):
    return value.get('type') == step['type'] and all(value.get(k) == step[k] for k in ('id', 'status') if k in step)


def harness_config(harness, root, attempt, binary, version, provider, step, previous):
    model = {'adapter': harness, 'effort': 'low'}
    config = {'root': str(attempt), 'helper': str(PROJECT / 'target/debug/crewroom-process'), 'assignmentId': 'synthetic-assignment',
              'runnerCwd': str(root / 'assignment'), 'prompt': PROMPT, 'tools': TOOLS, 'model': {'resolved': model},
              'admitRecordingShim': True, 'recordingVersion': version}
    if harness == 'codex':
        model.update(provider='codex', api='codex-app-server', id='gpt-5.5', selector='codex/gpt-5.5')
        config['codex'] = {'binary': str(binary), 'authFile': str(root / 'codex-login.json'), 'authRoot': str(root / 'auth')}
    elif harness == 'claude-code':
        model.update(provider='claude-code', api='claude-code-stream', id='sonnet', selector='claude-code/sonnet',
                     resolvedModel=os.environ.get('CREWROOM_RECORD_CLAUDE_MODEL', 'claude-sonnet-5-5'))
        config['claudeCode'] = {'binary': str(binary), 'home': str(root / 'harness-home')}
    else:
        model.update(provider='crewroom-fixture', id='synthetic-model', api='openai-responses',
                     selector='crewroom-fixture/synthetic-model', effort='off')
        config.update(command=str(binary), origin=provider.origin, providerToken='synthetic-provider-token',
                      upstreamProvider={'baseUrl': 'http://upstream.invalid/v1', 'api': 'openai-responses',
                                        'apiKey': 'synthetic-upstream-key',
                                        'models': [{'id': 'synthetic-model', 'name': 'Synthetic model', 'reasoning': False,
                                                    'input': ['text'], 'contextWindow': 200000, 'maxTokens': 8192}]},
                      env={'PATH': '/usr/bin:/bin', 'HOME': str(root / 'harness-home'), 'LANG': 'C.UTF-8', 'NO_COLOR': '1',
                           'PI_CONFIG_DIR': os.path.relpath(attempt / 'omp-root', root / 'harness-home'),
                           'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'},
                      args=['--mode', 'rpc', '--no-ui', '--no-tools', '--no-extensions', '--no-rules', '--no-skills', '--no-lsp',
                            '--no-title', '--no-pty', '--cwd', str(root / 'assignment'), '--session-dir', str(attempt / 'sessions'),
                            '--model', model['selector'], '--thinking', model['effort'], '--system-prompt', PROMPT])
        if step.get('resume') and previous:
            config.update(resumeSessionDir=previous['sessionDirectory'], previousSessionId=previous.get('sessionId'),
                          previousAttemptId='synthetic-previous-attempt')
    return config


def executable_identity(binary):
    real = Path(binary).resolve()
    content = real.read_bytes()
    return {'name': Path(binary).name, 'sha256': hashlib.sha256(content).hexdigest(), 'size': len(content)}


def record(args):
    harness, binary = args.harness, Path(args.binary).resolve()
    for built in ('target/debug/crewroom-process', 'target/debug/examples/codex-fixture'):
        if not (PROJECT / built).is_file():
            raise SystemExit(f'Missing {built}; run `cargo build -p crewroom-runtime --bins --examples` first.')
    version = subprocess.run([str(binary), '--version'], capture_output=True, text=True, timeout=30).stdout.strip()
    if not version:
        raise SystemExit('The harness did not report a version.')
    steps, actions = scenario(harness, args.scenario)
    provider = Provider(actions)
    # Private, outside any repository checkout (harnesses report repository
    # metadata such as remote URLs) and off tmpfs (harness caches are large).
    cache = Path(os.environ.get('XDG_CACHE_HOME') or Path.home() / '.cache')
    scratch = private(cache / 'crewroom-transcript-recordings')
    root = Path(tempfile.mkdtemp(prefix='record-', dir=scratch)).resolve()
    root.chmod(0o700)
    log = root / 'wire.jsonl'
    try:
        for name in ('assignment', 'auth', 'harness-home'):
            private(root / name)
        env, extra = {}, []
        if harness == 'claude-code':
            env = {'ANTHROPIC_BASE_URL': provider.origin, 'ANTHROPIC_API_KEY': 'synthetic-not-a-credential'}
            account = root / 'harness-home/.claude.json'
            account.write_text(json.dumps({'oauthAccount': {'accountUuid': 'synthetic-account', 'organizationUuid': 'synthetic-organization'},
                                           'hasCompletedOnboarding': True}))
            account.chmod(0o600)
            session = ['--input-format']
        elif harness == 'codex':
            extra = ['-c', f'openai_base_url="{provider.origin}/v1"', '-c', f'chatgpt_base_url="{provider.origin}/backend-api/"']
            env = {'OPENAI_BASE_URL': provider.origin + '/v1'}
            login = root / 'codex-login.json'
            login.write_text(json.dumps({'auth_mode': 'chatgpt', 'tokens': {'access_token': fake_token(), 'refresh_token': 'synthetic-refresh',
                                                                             'account_id': 'synthetic-account'}}))
            login.chmod(0o600)
            session = ['app-server']
        else:
            session = ['--mode']
        stand_in = root / BINARY_NAMES[harness]
        shim(stand_in, binary, log, env, extra, session)
        opens, attempt, previous, driver, failure = 0, None, None, None, None
        observed, snapshots = [], {}
        try:
            for step in steps:
                op = step['op']
                if op == 'open':
                    opens += 1
                    attempt = private(root / f'attempt-{opens}')
                    config = harness_config(harness, root, attempt, stand_in, version, provider, step, previous)
                    step['config'] = {k: v for k, v in config.items() if k not in ('helper', 'admitRecordingShim', 'recordingVersion', 'root', 'origin', 'tools')}
                    driver = Driver(attempt, config)
                    ready = driver.wait(lambda v: v['type'] == 'ready' and 'identity' in v)
                    previous = ready['identity']
                elif op == 'input':
                    driver.send({'op': 'input', 'id': step['id'], 'message': step['message'], 'steer': step.get('steer', False)})
                elif op == 'wait':
                    driver.wait(lambda v: matches(step, v))
                elif op == 'tool':
                    call = next(v for v in reversed(driver.events) if v['type'] == 'host_tool_call')
                    driver.send({'op': 'tool', 'result': {'type': 'host_tool_result', 'id': call['id'], 'isError': False,
                                                          'result': {'content': [{'type': 'text', 'text': step['text']}]}}})
                elif op == 'interrupt':
                    driver.send({'op': 'interrupt'})
                elif op == 'pause':
                    time.sleep(step['ms'] / 1000)
                elif op == 'close':
                    driver.close()
                    observed.append(driver.events)
                    snapshots[opens] = history(root)
                    driver = None
        except AssertionError as error:
            failure = str(error)
        finally:
            if driver:
                driver.close()
                observed.append(driver.events)
        if failure or provider.errors:
            raise SystemExit(f'Recording failed: {failure or ""} {provider.errors or ""} requests={provider.requests}\n'
                             f'Inspect the private recording directory (not for commit): {root}')
        identity = {**executable_identity(binary), 'name': Path(args.binary).name}
        transcript = assemble(harness, args.scenario, version, identity, steps, observed, log, root, snapshots, provider.origin)
        problems = check_transcript(transcript)
        if problems:
            raise SystemExit('Redaction gate refused the transcript:\n  ' + '\n  '.join(problems) +
                             f'\nPrivate recording retained for inspection (not for commit): {root}')
        output = Path(args.output) if args.output else TRANSCRIPTS / harness / f'{args.scenario}.jsonl'
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(''.join(json.dumps(line, sort_keys=True) + '\n' for line in transcript))
        print(json.dumps({'written': str(output.relative_to(PROJECT)) if output.is_relative_to(PROJECT) else str(output),
                          'harness': harness, 'version': version, 'scenario': args.scenario,
                          'frames': len(transcript) - 1, 'evidence': 'live harness binary, synthetic provider'}))
        if not args.keep:
            shutil.rmtree(root, ignore_errors=True)
    finally:
        provider.close()


def expectations(events):
    result = {}
    for value in events:
        kind = value.get('type')
        if kind == 'ready' and 'identity' in value:
            kind = 'opened'  # The driver's open receipt, not a harness frame.
        if kind not in EXPECTED_EVENTS:
            continue
        item = {'type': kind}
        keys = {'prompt_result': ('id', 'status'), 'host_tool_call': ('toolName', 'arguments')}.get(kind, ())
        item.update({key: value[key] for key in keys if key in value})
        if kind == 'opened':
            item['recoveryMode'] = value['identity'].get('recoveryMode')
        result.setdefault(kind, []).append(item)
    return result


def assemble(harness, name, version, identity, steps, observed, log, root, snapshots, origin):
    rows = [json.loads(line) for line in log.read_text().splitlines()]
    order, execs = {}, []
    for row in rows:
        if row['kind'] == 'exec':
            if not any(e['argv'] == row['argv'] for e in execs):
                execs.append({'argv': row['argv'], 'stdout': trim_exec(row['argv'], row['stdout'], steps), 'code': row['code']})
            continue
        if row['inv'] not in order:
            order[row['inv']] = len(order) + 1
    entries = []
    for row in rows:
        if row['kind'] == 'exec':
            continue
        session = order[row['inv']]
        if row['kind'] == 'start':
            entries.append({'session': session, 'dir': 'start', 'argv': row['argv']})
        elif row['kind'] in ('send', 'recv'):
            try:
                frame = json.loads(row['line'])
            except ValueError:
                frame = {'raw': row['line'].rstrip('\n')}
            entries.append({'session': session, 'dir': row['kind'], 'frame': frame})
        elif row['kind'] == 'exit':
            entries.append({'session': session, 'dir': 'exit', 'code': row['code']})
    if harness == 'claude-code':
        entries = with_history(entries, snapshots)
    header = {'format': FORMAT, 'harness': harness, 'scenario': name, 'version': version, 'executable': identity,
              'binaryName': BINARY_NAMES[harness], 'provider': 'synthetic-loopback', 'tasks': 'synthetic',
              'evidence': 'Recorded from the real harness binary with a synthetic provider. Replays are simulated checks, never live delivery.',
              'tools': TOOLS, 'steps': steps, 'execs': execs,
              'expect': expectations([e for events in observed for e in events]),
              'setupFiles': setup_files(harness)}
    return normalize([header, *entries], root, origin)


def history(root):
    projects = root / 'harness-home/.claude/projects'
    return {str(p): p.read_text() for p in sorted(projects.rglob('*.jsonl'))} if projects.exists() else {}


def with_history(entries, snapshots):
    """Claude reads its own session history only when it resumes. Attach the
    history files each session changed, after that session's final output."""
    result, before = list(entries), {}
    for session in sorted(snapshots):
        changed = {path: text for path, text in snapshots[session].items() if before.get(path) != text}
        before = snapshots[session]
        last = max((i for i, e in enumerate(result) if e['session'] == session and e['dir'] == 'recv'), default=None)
        if last is None:
            continue
        for offset, (path, text) in enumerate(changed.items()):
            result.insert(last + 1 + offset, {'session': session, 'dir': 'file', 'path': path, 'content': text})
    return result


def trim_exec(argv, stdout, steps):
    """`codex debug models --bundled` lists every model with long instructions;
    keep only the assigned model's row."""
    if argv[:3] != ['debug', 'models', '--bundled']:
        return stdout
    model = next(s['config']['model']['resolved']['id'] for s in steps if s.get('op') == 'open')
    value = json.loads(stdout)
    value['models'] = [m for m in value['models'] if m.get('slug') == model]
    for row in value['models']:
        for key in list(row):
            if isinstance(row[key], str) and len(row[key]) > 400:
                row[key] = 'Synthetic replay placeholder for long bundled instructions.'
    return json.dumps(value) + '\n'


def setup_files(harness):
    if harness == 'claude-code':
        return [{'path': '<<root>>/harness-home/.claude.json', 'mode': 384,
                 'content': json.dumps({'oauthAccount': {'accountUuid': 'synthetic-account', 'organizationUuid': 'synthetic-organization'}})}]
    if harness == 'codex':
        return [{'path': '<<root>>/codex-login.json', 'mode': 384,
                 'content': json.dumps({'auth_mode': 'chatgpt', 'tokens': {'access_token': '<<synthetic-access-token>>',
                                                                           'refresh_token': 'synthetic-refresh', 'account_id': 'synthetic-account'}})}]
    return []


# -------------------------------------------------------------- normalizing

def encoded(path):
    return ''.join(c if c.isascii() and c.isalnum() else '-' for c in str(path))


def normalize(lines, root, origin=None):
    replacements = [*([(origin, '<<provider>>')] if origin else []), (str(root), '<<root>>'), (encoded(root), '<<root-encoded>>'), (str(Path.home()), '<<home>>')]
    user = os.environ.get('USER') or Path.home().name
    host = socket.gethostname()
    uuids = {}

    def text(value):
        for old, new in replacements:
            value = value.replace(old, new)
        if host and len(host) > 2:
            value = value.replace(host, '<<host>>')
        if user and len(user) > 2:
            value = re.sub(rf'\b{re.escape(user)}\b', '<<user>>', value)
        return UUID.sub(lambda m: uuids.setdefault(m.group(0).lower(), f'<<uuid:{len(uuids) + 1}>>'), value)

    def walk(value, key=None):
        if isinstance(value, dict):
            return {walk_key(k): walk(v, k) for k, v in value.items()}
        if isinstance(value, list):
            return [walk(v, key) for v in value]
        if isinstance(value, str):
            if key and SECRET_KEYS.search(key) and value and not PLACEHOLDER.fullmatch(value):
                return '<<redacted>>'
            return text(value)
        return value

    def walk_key(key):
        return text(key)

    def entry(line):
        if line.get('dir') == 'file':
            # Session history is JSONL text; normalize each record's fields.
            records = [walk(json.loads(item)) for item in line['content'].splitlines() if item.strip()]
            return {**walk({k: v for k, v in line.items() if k != 'content'}),
                    'content': ''.join(json.dumps(r, sort_keys=True) + '\n' for r in records)}
        return walk(line)

    return [entry(line) for line in lines]


# ------------------------------------------------------------------- gate

def check_transcript(lines, where=''):
    problems = []
    if not lines or lines[0].get('format') != FORMAT:
        return [f'{where}missing {FORMAT} header']
    header = lines[0]
    for key in ('harness', 'version', 'executable', 'scenario', 'steps', 'expect'):
        if not header.get(key):
            problems.append(f'{where}header lacks {key}')
    if header.get('tasks') != 'synthetic' or header.get('provider') != 'synthetic-loopback':
        problems.append(f'{where}transcript is not declared synthetic')
    for step in header.get('steps', []):
        if step.get('op') == 'input' and MARKER not in step.get('message', ''):
            problems.append(f'{where}step {step.get("id")} is not a synthetic task')
    if header.get('executable', {}).get('name', '/').count('/'):
        problems.append(f'{where}executable identity must be a file name, not a path')

    def scan(value, path):
        if isinstance(value, dict):
            for k, v in value.items():
                scan(k, path + ['<key>'])
                if isinstance(v, str) and SECRET_KEYS.search(k) and v and not PLACEHOLDER.fullmatch(v):
                    problems.append(f'{where}{"/".join(path + [k])}: unredacted credential or account field')
                scan(v, path + [k])
        elif isinstance(value, list):
            for i, v in enumerate(value):
                scan(v, path + [str(i)])
        elif isinstance(value, str):
            for label, pattern in FORBIDDEN:
                if pattern.search(value):
                    problems.append(f'{where}{"/".join(path)}: {label}')
    for number, line in enumerate(lines):
        scan(line, [f'line {number + 1}'])
    return problems


def check(args):
    files = [Path(p) for p in args.paths] or sorted(TRANSCRIPTS.rglob('*.jsonl'))
    problems = []
    for path in files:
        try:
            lines = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
        except ValueError as error:
            problems.append(f'{path.name}: invalid JSONL ({error})')
            continue
        problems += check_transcript(lines, f'{path.parent.name}/{path.name}: ')
    if problems:
        print('Harness transcript redaction gate failed (locations only; content is not repeated):', file=sys.stderr)
        for problem in problems:
            print('  ' + problem, file=sys.stderr)
        raise SystemExit(1)
    print(json.dumps({'checked': len(files), 'result': 'pass'}))


# -------------------------------------------------------------------- diff

def shape(value, depth=0):
    """A frame's protocol shape: discriminators keep their values, other
    scalars become their JSON type. Long lists collapse to their first item."""
    if isinstance(value, dict):
        return {k: (v if k in ('type', 'method', 'subtype', 'command', 'kind', 'state', 'status') and isinstance(v, str)
                    else shape(v, depth + 1)) for k, v in sorted(value.items())}
    if isinstance(value, list):
        kinds = []
        for item in value:
            s = shape(item, depth + 1)
            if s not in kinds:
                kinds.append(s)
        return kinds
    if isinstance(value, bool):
        return 'bool'
    if isinstance(value, (int, float)):
        return 'number'
    if value is None:
        return 'null'
    return 'string'


def label(frame):
    for path in (('type',), ('method',), ('request', 'subtype'), ('event', 'type'), ('params', 'item', 'type')):
        value = frame
        for key in path:
            value = value.get(key) if isinstance(value, dict) else None
        if isinstance(value, str):
            yield value
    if 'method' not in frame and ('result' in frame or 'error' in frame):
        yield 'response'


def render(lines):
    header, out = lines[0], []
    out.append(f'harness {header["harness"]} scenario {header["scenario"]}')
    out.append(f'version {header["version"]}')
    out.append(f'executable {header["executable"]["name"]} sha256 {header["executable"]["sha256"]} size {header["executable"]["size"]}')
    for e in header.get('execs', []):
        out.append(f'exec {" ".join(e["argv"])} -> code {e["code"]}')
    for kind, items in sorted(header.get('expect', {}).items()):
        out.append(f'adapter {kind} x{len(items)}')
    previous = None
    for entry in lines[1:]:
        if entry['dir'] == 'start':
            out.append(f'[session {entry["session"]}] start {" ".join(a if len(a) < 60 else a[:57] + "..." for a in entry["argv"])}')
            continue
        if entry['dir'] in ('exit', 'file'):
            out.append(f'[session {entry["session"]}] {entry["dir"]}')
            continue
        frame = entry['frame']
        line = f'[session {entry["session"]}] {entry["dir"]} {"/".join(label(frame)) or "?"} {json.dumps(shape(frame), sort_keys=True)}'
        # Streaming deltas repeat; one line per run keeps the diff readable.
        if line != previous:
            out.append(line)
        previous = line
    return out


def diff(args):
    old, new = ([json.loads(l) for l in Path(p).read_text().splitlines() if l.strip()] for p in (args.old, args.new))
    lines = list(difflib.unified_diff(render(old), render(new), fromfile=args.old, tofile=args.new, lineterm='', n=2))
    print('\n'.join(lines) if lines else 'No protocol shape differences.')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='command', required=True)
    rec = sub.add_parser('record', help='record one synthetic scenario from a real harness binary')
    rec.add_argument('--harness', choices=HARNESSES, required=True)
    rec.add_argument('--binary', required=True)
    rec.add_argument('--scenario', choices=SCENARIOS, required=True)
    rec.add_argument('--output')
    rec.add_argument('--keep', action='store_true', help='retain the private recording directory')
    chk = sub.add_parser('check', help='redaction gate over committed transcripts')
    chk.add_argument('paths', nargs='*')
    dif = sub.add_parser('diff', help='readable protocol diff between two transcripts')
    dif.add_argument('old')
    dif.add_argument('new')
    args = parser.parse_args()
    {'record': record, 'check': check, 'diff': diff}[args.command](args)


if __name__ == '__main__':
    main()
