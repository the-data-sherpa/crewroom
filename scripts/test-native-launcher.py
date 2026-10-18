#!/usr/bin/env python3
"""Exercise the nonbuilding bundled launcher in a disposable private installation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import socket
import subprocess
import time
import traceback
import urllib.request

PROJECT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--bundle', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
options = parser.parse_args()
source = options.bundle.resolve()
root = options.output.resolve()
root.mkdir(parents=True, exist_ok=False, mode=0o700)
macos = platform.system() == 'Darwin'
executable_relative = Path('Contents/MacOS') if macos else Path('bin')
bundle = root / 'App' / ('Crewroom.app' if macos else 'Crewroom')
shutil.copytree(source, bundle)
launcher = bundle / executable_relative / 'crewroom-launcher'
shutil.copy2(PROJECT / 'scripts/launch-native.sh', launcher)
launcher.chmod(0o755)
if macos:
    subprocess.run(['codesign', '--force', '--sign', '-', str(launcher)], check=True, capture_output=True)
    subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)], check=True, capture_output=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(bundle)], check=True, capture_output=True)
data = root / 'private data'
roles = root / 'private roles/roles.json'
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
args = [str(launcher), '--data-dir', str(data), '--roles', str(roles), '--port', str(port), '--headless']
# No Rust, Zig, Node, npm or real runtime/provider configuration is supplied.
env = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'LANG': 'C'}
logs = []
identities = {}
info = None
passed = False
error = None

def identity(pid):
    result = subprocess.run(['/bin/ps', '-p', str(pid), '-o', 'uid=,lstart=,command='], capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else None


def launch(extra=None):
    started = time.monotonic()
    result = subprocess.run(extra or args, cwd='/', env=env, capture_output=True, text=True, timeout=90)
    logs.append({'elapsedSeconds': time.monotonic() - started, 'code': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
    return result


def connection():
    value = json.loads((data / 'native-connection.json').read_text())
    assert value['origin'] == f'http://127.0.0.1:{port}'
    current = identity(value['pid'])
    assert current and str(bundle / executable_relative / 'crewroomd') in current and str(data) in current
    assert current.split()[0] == str(os.getuid())
    identities.setdefault(value['pid'], current)
    assert identities[value['pid']] == current
    return value


def request(path, body=None):
    headers = {'Authorization': 'Bearer ' + info['token'], 'Origin': info['origin']}
    raw = None if body is None else json.dumps(body).encode()
    if raw is not None:
        headers['Content-Type'] = 'application/json'
    req = urllib.request.Request(info['origin'] + '/api' + path, data=raw, headers=headers)
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req, timeout=10) as response:
        return json.load(response)


def stop(value):
    pid = value['pid']
    current = identity(pid)
    if current is None:
        return
    assert current == identities[pid], 'Daemon identity changed; refusing cleanup.'
    os.kill(pid, signal.SIGTERM)
    by = time.monotonic() + 30
    while time.monotonic() < by and identity(pid) == current:
        time.sleep(.05)
    assert identity(pid) != current, 'Owned daemon failed to stop; retained for inspection.'
    assert not (data / 'native-connection.json').exists(), 'Shutdown retained its connection descriptor.'


try:
    # All rejected inputs are disposable. No runtime state may be created.
    relative = launch([str(launcher), '--data-dir', 'relative', '--roles', str(roles), '--headless'])
    assert relative.returncode != 0 and not data.exists()
    public = root / 'public'; public.mkdir(mode=0o755); public.chmod(0o755)
    rejected = launch([str(launcher), '--data-dir', str(public), '--roles', str(roles), '--headless'])
    assert rejected.returncode != 0 and not (public / 'workflow.sqlite').exists()
    assert launch().returncode == 0
    info = connection(); first_pid = info['pid']
    workspace = request('/workspace'); assert workspace['tasks'] == [] and workspace['runs'] == []
    role = workspace['roles'][0]; role['revision'] = role.get('revision', 1)
    role['instructions'] += '\nLauncher fixture role.'
    request('/roles/' + role['id'], role)
    task = request('/tasks', {'title': 'Retained launcher task', 'description': 'No integrations configured.'})
    assert launch().returncode == 0
    reopened = connection(); assert reopened == info, 'Reopen started a duplicate or reconfigured the existing service.'
    assert 'existing role, port and runtime configuration remain in effect' in logs[-1]['stdout']
    assert any(row['id'] == task['id'] for row in request('/workspace')['tasks'])
    stop(info); info = None
    assert launch().returncode == 0
    info = connection(); assert info['pid'] != first_pid
    assert any(row['id'] == task['id'] for row in request('/workspace')['tasks'])
    assert next(row for row in request('/workspace')['roles'] if row['id'] == role['id'])['instructions'] == role['instructions']
    assert roles.is_file() and roles.stat().st_mode & 0o077 == 0
    assert data.stat().st_mode & 0o077 == 0
    stop(info); info = None
    passed = True
except BaseException as caught:
    error = traceback.format_exc()
finally:
    if info is not None:
        try:
            stop(info)
        except BaseException as cleanup:
            error = f'{error}; cleanup: {cleanup}'; passed = False
    survivors = [pid for pid, recorded in identities.items() if identity(pid) == recorded]
    passed = passed and not survivors
    report = {'passed': passed, 'error': error, 'sourceBundle': str(source),
              'launcherSha256': hashlib.sha256(launcher.read_bytes()).hexdigest(),
              'sourceRevision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=PROJECT, text=True).strip(),
              'sourceExecutableSha256': {name: hashlib.sha256((source / executable_relative / name).read_bytes()).hexdigest() for name in ['crewroom', 'crewroomd', 'crewroom-host', 'crewroom-process']},
              'launches': logs, 'ownedDaemonCount': len(identities), 'survivingOwnedDaemons': survivors,
              'scope': 'Headless launcher: no GUI/Finder or configured execution claim; minimal system PATH, absolute private paths with spaces, authenticated reopen, same-service reuse and stopped restart preserve a task.'}
    (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
raise SystemExit(0 if passed else 1)
