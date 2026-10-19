#!/usr/bin/env python3
"""FOR-74 live allowance check: a disposable daemon without runner
configuration reads the remaining subscription allowance of the Codex and
Claude Code logins on this machine.

The daemon gets a new private data directory, a non-default loopback port and
no CREWROOM_RUNTIME_CONFIG. Reading allowance sends no prompt: Codex answers
`account/rateLimits/read` on its app-server and Claude Code the experimental
`get_usage` control request. No agent, assignment or inference starts.

The report is redacted for a public repository: it keeps each harness's state,
window labels, whether a remaining share and reset time were reported, the
observation time and any unavailable reason. It drops the plan name and the
exact shares. It also confirms that no credential string from the local login
files appears in the allowance or workspace views, reporting only counts.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request

PRIVATE = ('CREWROOM_RUNTIME_CONFIG', 'CREWROOM_JIRA_TOKEN', 'CREWROOM_BITBUCKET_TOKEN')


def secrets():
    """Long string values from the local login files; never printed."""
    found = set()
    home = Path.home()
    codex_home = Path(os.environ.get('CODEX_HOME', home / '.codex'))
    for path in (codex_home / 'auth.json', home / '.claude/.credentials.json', home / '.claude.json'):
        try:
            value = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        stack = [value]
        while stack:
            item = stack.pop()
            if isinstance(item, dict):
                stack.extend(item.values())
            elif isinstance(item, list):
                stack.extend(item)
            elif isinstance(item, str) and len(item) >= 16:
                found.add(item)
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='new private directory')
    parser.add_argument('--binaries', type=Path, required=True, help='directory with crewroomd')
    parser.add_argument('--port', type=int, required=True, help='free loopback port, not 4318')
    args = parser.parse_args()
    if args.port == 4318:
        raise ValueError('Port 4318 belongs to the operator service')
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    binaries = args.binaries.resolve()
    connection = root / 'data/native-connection.json'
    env = {k: v for k, v in os.environ.items() if k not in PRIVATE}
    report = {'passed': False, 'scope': 'live allowance read; runner unconfigured; no prompt; no inference'}
    log = (root / 'daemon.log').open('wb')
    process = subprocess.Popen([str(binaries / 'crewroomd'), '--data-dir', str(root / 'data'),
                                '--roles', str(root / 'config/roles.json'), '--port', str(args.port)],
                               env=env, stdout=log, stderr=log)

    def request(info, path, body=None):
        req = urllib.request.Request(info['origin'] + '/api/' + path,
                                     data=None if body is None else json.dumps(body).encode(),
                                     headers={'Authorization': 'Bearer ' + info['token'],
                                              'Origin': info['origin'], 'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(req, timeout=30) as response:
                return json.load(response)
        except Exception as error:
            raise RuntimeError(f'{path}: {type(error).__name__}: {error}') from error

    try:
        deadline = time.monotonic() + 30
        while not connection.exists():
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('Disposable daemon failed to initialize; see daemon.log')
            time.sleep(.05)
        info = json.loads(connection.read_text())
        before = request(info, 'allowance')
        report['beforeRefresh'] = [{k: h.get(k) for k in ('harness', 'state', 'reason', 'observedAt')}
                                   for h in before['harnesses']]
        started = time.time()
        request(info, 'allowance/refresh', {})
        deadline = time.monotonic() + 120
        while True:
            view = request(info, 'allowance')
            if view['state'] == 'idle' and all(h.get('observedAt') for h in view['harnesses']):
                break
            if time.monotonic() > deadline:
                raise TimeoutError('Allowance read did not finish')
            time.sleep(.5)
        report['readSeconds'] = round(time.time() - started, 1)
        report['harnesses'] = [{
            'harness': h['harness'], 'state': h['state'], 'reason': h.get('reason'),
            'source': h.get('source'), 'planReported': h.get('plan') is not None,
            'observedAt': h.get('observedAt'), 'inferenceRequested': h.get('inferenceRequested'),
            'notes': len(h.get('notes', [])),
            'windows': [{'label': w['label'], 'remainingReported': w.get('remainingPercent') is not None,
                         'remainingInRange': w.get('remainingPercent') is None
                         or 0 <= w['remainingPercent'] <= 100,
                         'resetReported': w.get('resetsAt') is not None, 'reason': w.get('reason')}
                        for w in h.get('windows', [])],
        } for h in view['harnesses']]
        workspace = request(info, 'workspace')
        text = json.dumps(view) + json.dumps(workspace)
        known = secrets()
        report['secretScan'] = {'localCredentialStrings': len(known),
                                'found': sum(1 for s in known if s in text)}
        report['runs'] = len(workspace['runs'])
        report['passed'] = (report['runs'] == 0 and report['secretScan']['found'] == 0
                            and all(h['inferenceRequested'] is False and h['observedAt']
                                    and (h['state'] != 'unavailable' or h['reason'])
                                    for h in report['harnesses']))
    except Exception as error:  # Retain the failure in the report.
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        process.terminate()
        try:
            report['daemonExit'] = process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            process.kill()
            report['daemonExit'] = process.wait()
        log.close()
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
