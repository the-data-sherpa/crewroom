#!/usr/bin/env python3
"""Test-only tmux host for one OMP RPC child. No production supervisor policy."""
import http.server
import json
import os
from pathlib import Path
import sys
import threading
import time
from types import SimpleNamespace

from omp_offline_fixture import Agent


def main():
    config = json.loads(Path(sys.argv[1]).read_text())
    sandbox = SimpleNamespace(binary=Path(config['binary']),
        argv=lambda cmd: ['/usr/bin/sandbox-exec', '-f', config['sandbox'], *map(str, cmd)])
    provider = SimpleNamespace(port=config['providerPort'])
    catalog = json.loads(Path(config['catalog']).read_text())
    agent = Agent(config['name'], Path(config['root']), sandbox, provider, catalog,
                  config['model'], session=config.get('session'))
    finished = threading.Event()
    agent.command('get_state')
    identity = {'hostPid': os.getpid(), 'ompPid': agent.proc.pid, 'name': config['name'],
                'root': str(agent.root), 'session': agent.command('get_state')}
    print(json.dumps({'event': 'host_ready', **identity}), flush=True)

    def render():
        cursor = 0
        while not finished.wait(.05):
            with agent.condition:
                batch = agent.frames[cursor:]
                cursor = len(agent.frames)
            for frame in batch:
                # This is deliberately a fixture display of structured events,
                # not a claim that OMP's native TUI and RPC run together.
                print(json.dumps({'agent': config['name'], 'event': frame}), flush=True)
    renderer = threading.Thread(target=render, daemon=True); renderer.start()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            if self.headers.get('Authorization') != 'Bearer ' + config['token']:
                self.send_error(403); return
            request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            try:
                op = request['op']
                if op == 'command':
                    result = agent.command(request['kind'], **request.get('arguments', {}))
                elif op == 'send':
                    agent.send(request['frame']); result = None
                elif op == 'frames':
                    with agent.condition:
                        result = agent.frames[request.get('start', 0):]
                elif op == 'identity':
                    result = identity
                elif op == 'close':
                    result = agent.close(abrupt=request.get('abrupt', False))
                    finished.set()
                    threading.Thread(target=server.shutdown, daemon=True).start()
                else:
                    raise ValueError('Unknown fixture operation')
                response = {'result': result}
            except Exception as exc:
                response = {'error': f'{type(exc).__name__}: {exc}'}
            data = json.dumps(response).encode()
            self.send_response(200); self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(data))); self.end_headers(); self.wfile.write(data)

    server = http.server.ThreadingHTTPServer(('127.0.0.1', config['port']), Handler)
    try:
        server.serve_forever(poll_interval=.05)
    finally:
        finished.set(); server.server_close()
        if not agent.log.closed:
            agent.close(abrupt=True)
        renderer.join(1)


if __name__ == '__main__':
    main()
