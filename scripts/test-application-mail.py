#!/usr/bin/env python3
"""Run production Crewroom message operations against guarded Rust Agent Mail."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import http.server
import threading
import urllib.request
from mailbox_contract_fixture import PROJECT, RustMail, NetworkGuard, free_port

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True)
root = parser.parse_args().output.resolve(); root.mkdir(parents=True, exist_ok=False, mode=0o700)
port = free_port(); guard = NetworkGuard(root, [port], [port]); mail = RustMail(root / 'mail', guard, port)
class Proxy(http.server.BaseHTTPRequestHandler):
    lost = False
    drop_ack = None
    def log_message(self, *args): pass
    def do_POST(self):
        if self.path in ['/drop-ack-before', '/drop-ack-after']:
            Proxy.drop_ack = self.path; self.send_response(200); self.end_headers(); self.wfile.write(b'{}'); return
        if self.path in ['/offline', '/online', '/restart']:
            if self.path in ['/offline', '/restart']: mail.stop(abrupt=True)
            if self.path in ['/online', '/restart']: mail.start()
            self.send_response(200); self.end_headers(); self.wfile.write(b'{}'); return
        raw = self.rfile.read(int(self.headers['Content-Length']))
        payload = json.loads(raw)
        ack = payload.get('params', {}).get('name') == 'acknowledge_message'
        if ack and Proxy.drop_ack == '/drop-ack-before':
            Proxy.drop_ack = None; self.send_response(503); self.end_headers(); return
        request = urllib.request.Request(f'http://127.0.0.1:{port}/mcp/', data=raw,
            headers={key: self.headers[key] for key in ('Content-Type','Accept','Authorization','Mcp-Session-Id') if key in self.headers})
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                data = response.read(); status = response.status; headers = dict(response.headers)
        except OSError:
            self.send_response(503); self.end_headers(); return
        if ack and Proxy.drop_ack == '/drop-ack-after':
            Proxy.drop_ack = None; self.close_connection = True; return
        if payload.get('params', {}).get('arguments', {}).get('body_md') == 'LOSE_REPLY' and not Proxy.lost:
            Proxy.lost = True; self.close_connection = True; return
        self.send_response(status)
        for key in ('Content-Type','Mcp-Session-Id'):
            if key in headers: self.send_header(key, headers[key])
        self.end_headers(); self.wfile.write(data)
proxy = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Proxy)
threading.Thread(target=proxy.serve_forever, daemon=True).start()
try:
    mail.start()
    config = root / 'driver.json'
    config.write_text(json.dumps({'root': str(root), 'url': f'http://127.0.0.1:{proxy.server_port}/mcp/', 'controlUrl': f'http://127.0.0.1:{proxy.server_port}', 'token': mail.token, 'project': mail.project})); config.chmod(0o600)
    result = subprocess.run(['node', '--import', 'tsx', 'scripts/messaging-driver.ts', str(config)], cwd=PROJECT, capture_output=True, text=True, timeout=60)
    (root / 'result.txt').write_text(result.stdout + result.stderr)
    (root / 'report.json').write_text(json.dumps({'passed': result.returncode == 0, 'mailGenerations': len(mail.generations), 'boundary': 'Real local Rust Mail; loopback fault proxy; no live providers.'}, indent=2) + '\n')
    print(result.stdout + result.stderr, end='')
finally:
    proxy.shutdown(); proxy.server_close()
    mail.stop()
raise SystemExit(result.returncode)
