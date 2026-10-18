#!/usr/bin/env python3
"""Run native durable message operations against guarded actual Rust Agent Mail."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import http.server
import threading
import urllib.request
import sys
import signal
import socket
import time
import traceback
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
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
private = root / 'guard-private'; private.write_text('fixture-only')
guard.path.write_text(guard.path.read_text() + f'(deny file-read* (literal {json.dumps(str(private))}))\n')
def verify_guard():
    with socket.socket() as blocked:
        blocked.bind(('127.0.0.1', 0)); blocked.listen()
        probe = """
import errno, json, pathlib, socket, sys
root, allowed, denied = pathlib.Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
s = socket.create_connection(('127.0.0.1',allowed), timeout=2); s.close()
(root/'guard-write').write_text('allowed')
for action in [lambda: socket.create_connection(('127.0.0.1',denied),timeout=2), lambda: (root/'guard-private').read_text(), lambda: (root.parent/('guard-denied-'+root.name)).write_text('must not write')]:
    try: action()
    except OSError as e: assert e.errno in (errno.EPERM,errno.EACCES), str(e)
    else: raise AssertionError('Guard allowed a prohibited operation')
print(json.dumps({'allowedLoopback':True,'allowedOwnedWrite':True,'deniedOtherLoopback':True,'deniedPrivateRead':True,'deniedOutsideWrite':True}))
"""
        result = subprocess.run(guard.argv([sys.executable,'-c',probe,str(root),str(port),str(blocked.getsockname()[1])]), capture_output=True, text=True, timeout=10, check=True)
        return json.loads(result.stdout)

def group_alive(pid):
    try: os.killpg(pid, 0); return True
    except ProcessLookupError: return False

report = {'passed':False,'boundary':'Native SQLite messaging and real local Rust Mail; loopback fault proxy; no agent or live provider.'}
driver = None
try:
    mail.start(); report['guard'] = verify_guard()
    config = root / 'driver.json'
    config.write_text(json.dumps({'root': str(root), 'url': f'http://127.0.0.1:{proxy.server_port}/mcp/', 'controlUrl': f'http://127.0.0.1:{proxy.server_port}', 'token': mail.token, 'project': mail.project})); config.chmod(0o600)
    driver = subprocess.Popen(['node', '--import', 'tsx', 'scripts/rust/messaging-live.ts', str(config)], cwd=PROJECT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    stdout, stderr = driver.communicate(timeout=120)
    (root / 'result.txt').write_text(stdout + stderr)
    print(stdout + stderr, end='')
    report.update(passed=driver.returncode == 0, exitCode=driver.returncode, runtime=mail.identity())
except Exception:
    report['failure'] = traceback.format_exc(); print(report['failure'], file=sys.stderr)
finally:
    if driver and group_alive(driver.pid):
        # This dedicated process group contains only this harness and its native
        # children. Reap descendants even if a Node worker failed or timed out.
        report['forcedDriverCleanup'] = True
        os.killpg(driver.pid, signal.SIGKILL); driver.communicate(timeout=10)
        for _ in range(100):
            if not group_alive(driver.pid): break
            time.sleep(.05)
    proxy.shutdown(); proxy.server_close(); mail.stop()
    report['mailGenerations'] = mail.generations
    report['allOwnedProcessesExited'] = all(not group_alive(item['pid']) for item in mail.generations) and (driver is None or not group_alive(driver.pid))
    report['native'] = json.loads((root/'native-report.json').read_text()) if (root/'native-report.json').exists() else None
    report['passed'] = report['passed'] and report['allOwnedProcessesExited'] and not report.get('forcedDriverCleanup', False)
    (root/'report.json').write_text(json.dumps(report,indent=2)+'\n')
raise SystemExit(0 if report['passed'] else 1)
