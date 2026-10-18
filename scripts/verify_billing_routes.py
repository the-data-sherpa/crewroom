#!/usr/bin/env python3
"""FOR-42 live billing-route check: a disposable daemon without runner
configuration detects the billing route of each installed harness from the
logins in this machine's HOME.

The daemon gets a new private data directory, a non-default loopback port and
no CREWROOM_RUNTIME_CONFIG. Discovery is metadata only: no agent, assignment
or inference starts. The report holds harness states, detected routes, their
basis (file and field kinds), offering counts per route and labels. It never
contains tokens or account data: every credential string found in the local
login files and *_API_KEY environment variables is checked against the
daemon's catalog, workspace and log, and only the count of matches is kept.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request

PRIVATE = ('CREWROOM_RUNTIME_CONFIG', 'CREWROOM_JIRA_TOKEN', 'CREWROOM_BITBUCKET_TOKEN')


def strings(value):
    if isinstance(value, dict):
        for item in value.values():
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, str):
        yield value


def credential_needles():
    """Credential-shaped strings from local logins; held in memory only."""
    home = Path(os.environ['HOME'])
    codex_home = Path(os.environ.get('CODEX_HOME') or home / '.codex')
    needles = set()
    for path, pick in ((codex_home / 'auth.json', lambda v: [v.get('tokens'), v.get('OPENAI_API_KEY')]),
                       (home / '.claude/.credentials.json', lambda v: [v]),
                       (home / '.claude.json', lambda v: [v.get('primaryApiKey'), v.get('oauthAccount')])):
        try:
            value = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        for part in pick(value):
            needles.update(s for s in strings(part) if len(s) >= 16)
    needles.update(v for k, v in os.environ.items() if k.endswith('API_KEY') and len(v) >= 16)
    return needles


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
    report = {'passed': False, 'scope': 'live billing detection; runner unconfigured; metadata only; no inference'}
    log_path = root / 'daemon.log'
    log = log_path.open('wb')
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

    observed = []
    try:
        deadline = time.monotonic() + 30
        while not connection.exists():
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError('Disposable daemon failed to initialize; see daemon.log')
            time.sleep(.05)
        info = json.loads(connection.read_text())
        request(info, 'model-catalog/refresh', {})
        deadline = time.monotonic() + 180
        while True:
            catalog = request(info, 'model-catalog')
            if catalog['state'] != 'refreshing':
                break
            if time.monotonic() > deadline:
                raise TimeoutError('Discovery did not finish')
            time.sleep(.5)
        workspace = request(info, 'workspace')
        observed = [json.dumps(catalog), json.dumps(workspace)]
        report['state'] = catalog['state']
        report['runner'] = catalog['runner']['configured']
        choices = catalog.get('choices', [])
        report['harnesses'] = []
        for h in catalog['harnesses']:
            name = h['harness']
            mine = [c for c in choices if c['key'].startswith(f'offering:{name}:')]
            by_route = {}
            for c in mine:
                by_route[c['billingRoute']] = by_route.get(c['billingRoute'], 0) + 1
            report['harnesses'].append({
                'harness': name, 'configured': h.get('configured'), 'outcome': h['outcome'],
                'version': (h.get('identity') or {}).get('version'),
                'billing': h.get('billing'), 'offeringsByBillingRoute': by_route,
                'sampleLabels': [c['label'] for c in mine[:2]],
                'keysEndWithRoute': all(c['key'].endswith(':' + c['billingRoute']) for c in mine),
                'note': h.get('note') or h.get('remedy')})
        report['runs'] = len(workspace['runs'])
        report['passed'] = (catalog['runner']['configured'] is False and report['runs'] == 0
                            and all(h['keysEndWithRoute'] for h in report['harnesses']))
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
        observed.append(log_path.read_text(errors='replace'))
        needles = credential_needles()
        text = '\n'.join(observed)
        report['secretScan'] = {'credentialStringsChecked': len(needles),
                                'foundInCatalogWorkspaceOrLog': sum(1 for n in needles if n in text)}
        report['passed'] = report['passed'] and report['secretScan']['foundInCatalogWorkspaceOrLog'] == 0
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
