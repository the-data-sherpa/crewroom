"""Disposable mailbox test clients; not a production Crewroom message service."""
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.request
import uuid

PROJECT = Path(__file__).resolve().parent.parent


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


class NetworkGuard:
    def __init__(self, root, ports, listeners=()):
        self.root = Path(root).resolve()
        self.path = self.root / 'mailbox.sb'
        rules = ['(version 1)', '(allow default)', '(deny network*)', '(deny file-write*)']
        rules += [f'(allow network-outbound (remote ip "localhost:{p}"))' for p in ports]
        rules += [f'(allow network-inbound (local ip "localhost:{p}"))' for p in listeners]
        rules += [f'(allow file-write* (subpath {json.dumps(str(self.root))}) (subpath "/dev"))']
        for private in ('.omp', '.pi', '.aws', '.ssh', '.config', '.codex', 'Library/Keychains'):
            rules += [f'(deny file-read* (subpath {json.dumps(str(Path.home() / private))}))']
        self.path.write_text('\n'.join(rules) + '\n')

    def argv(self, command):
        return ['/usr/bin/sandbox-exec', '-f', str(self.path), *map(str, command)]


class RustMail:
    def __init__(self, root, guard, port, binary=None, retention=86400):
        self.root, self.guard, self.port = Path(root).resolve(), guard, port
        self.root.mkdir(parents=True, exist_ok=True)
        self.default_binary = (PROJECT / '.crewroom/experiments/rust-mail-prep/runtime/mcp-agent-mail').resolve()
        self.binary = Path(binary).resolve() if binary else self.default_binary
        self.retention = retention
        self.token = 'synthetic-' + uuid.uuid4().hex
        self.proc = None
        self.generations, self.calls = [], []
        self.project = str(self.root / 'target-repo')
        Path(self.project).mkdir(exist_ok=True)
        self.session = None

    def start(self):
        if not self.binary.is_file():
            from prepare_worktree import fixture, missing_message
            raise RuntimeError(missing_message(fixture('Rust Mail')) if self.binary == self.default_binary
                               else f'Rust Mail binary {self.binary} does not exist.')
        for folder in ('tmp', 'config', 'data', 'cache', 'archive'):
            (self.root / folder).mkdir(exist_ok=True)
        env = {k: os.environ[k] for k in ('PATH', 'HOME', 'LANG') if k in os.environ}
        env.update(TMPDIR=str(self.root / 'tmp'), STORAGE_ROOT=str(self.root / 'archive'),
                   DATABASE_URL='sqlite:///' + str(self.root / 'mail.sqlite3'),
                   HTTP_HOST='127.0.0.1', HTTP_PORT=str(self.port), HTTP_BEARER_TOKEN=self.token,
                   HTTP_ALLOW_LOCALHOST_UNAUTHENTICATED='false', AM_ATC_ENABLED='false',
                   AM_IDEMPOTENCY_RETENTION_SECS=str(self.retention),
                   XDG_CONFIG_HOME=str(self.root / 'config'), XDG_DATA_HOME=str(self.root / 'data'),
                   XDG_CACHE_HOME=str(self.root / 'cache'), GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_CONFIG_NOSYSTEM='1', GIT_AUTHOR_NAME='Crewroom fixture',
                   GIT_AUTHOR_EMAIL='fixture@example.invalid', GIT_COMMITTER_NAME='Crewroom fixture',
                   GIT_COMMITTER_EMAIL='fixture@example.invalid', NO_COLOR='1')
        generation = len(self.generations)
        while (self.root / f'server-{generation}.log').exists():
            generation += 1
        log = self.root / f'server-{generation}.log'
        self.log = log.open('x')
        self.proc = subprocess.Popen(self.guard.argv([self.binary, 'serve', '--no-tui', '--no-reuse-running',
                                    '--host', '127.0.0.1', '--port', str(self.port)]),
                                     cwd=self.root, env=env, stdout=self.log, stderr=self.log, start_new_session=True)
        self.generations.append({'pid': self.proc.pid, 'log': str(log)})
        self.session = None
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(f'Rust Mail exited {self.proc.returncode}; see {log}')
            try:
                self.rpc('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {},
                                       'clientInfo': {'name': 'crewroom-contract-test', 'version': '0.1'}})
                self.rpc('notifications/initialized', {}, notification=True)
                return
            except (OSError, urllib.error.URLError):
                time.sleep(.1)
        raise TimeoutError(f'Rust Mail did not become ready; see {log}')

    def rpc(self, method, params, notification=False):
        body = {'jsonrpc': '2.0', 'method': method, 'params': params}
        if not notification:
            body['id'] = uuid.uuid4().hex
        headers = {'Content-Type': 'application/json', 'Accept': 'application/json, text/event-stream',
                   'Authorization': 'Bearer ' + self.token}
        if self.session:
            headers['Mcp-Session-Id'] = self.session
        req = urllib.request.Request(f'http://127.0.0.1:{self.port}/mcp/', data=json.dumps(body).encode(), headers=headers)
        with urllib.request.urlopen(req, timeout=12) as response:
            if response.headers.get('Mcp-Session-Id'):
                self.session = response.headers['Mcp-Session-Id']
            raw = response.read().decode()
        if not raw or notification:
            return None
        if raw.lstrip().startswith('data:') or raw.lstrip().startswith('event:'):
            frames = [json.loads(line[5:].strip()) for line in raw.splitlines() if line.startswith('data:')]
            result = next(f for f in frames if f.get('id') == body['id'])
        else:
            result = json.loads(raw)
        if 'error' in result:
            raise RuntimeError(json.dumps(result['error']))
        return result['result']

    def call(self, tool_name, **arguments):
        result = self.rpc('tools/call', {'name': tool_name, 'arguments': arguments})
        self.calls.append({'tool': tool_name, 'arguments': arguments, 'result': result})
        if result.get('isError'):
            raise RuntimeError(json.dumps(result))
        if result.get('structuredContent') is not None:
            return result['structuredContent']
        texts = [c['text'] for c in result.get('content', []) if c['type'] == 'text']
        return json.loads('\n'.join(texts)) if texts else result

    def stop(self, abrupt=False):
        if self.proc and self.proc.poll() is None:
            os.killpg(self.proc.pid, signal.SIGKILL if abrupt else signal.SIGTERM)
            try:
                self.proc.wait(timeout=8)
            except subprocess.TimeoutExpired:
                os.killpg(self.proc.pid, signal.SIGKILL)
                self.proc.wait(timeout=3)
        if hasattr(self, 'log'):
            self.log.close()
        (self.root / 'calls.json').write_text(json.dumps(self.calls, indent=2) + '\n')

    def stopped_proof(self):
        if self.proc is None or self.proc.returncode is None:
            raise RuntimeError('Wait for the owned Mail leader before verifying cleanup')
        deadline = time.monotonic() + 10
        while True:
            try:
                os.killpg(self.proc.pid, 0)
                absent = False
            except ProcessLookupError:
                absent = True
            if absent or time.monotonic() >= deadline:
                return {'pid': self.proc.pid, 'waitedExitCode': self.proc.returncode,
                        'processGroupAbsent': absent, 'verifiedStopped': absent}
            time.sleep(.05)

    def identity(self):
        return {'binary': str(self.binary), 'sha256': hashlib.sha256(self.binary.read_bytes()).hexdigest(),
                'release': 'v0.3.36'}
