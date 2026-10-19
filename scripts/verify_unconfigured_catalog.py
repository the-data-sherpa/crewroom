#!/usr/bin/env python3
"""FOR-38 live metadata check: a disposable daemon without runner configuration
lists the harnesses installed on this machine's PATH.

The daemon gets a new private data directory, a non-default loopback port and
no CREWROOM_RUNTIME_CONFIG. Discovery is metadata only: no agent, assignment
or inference starts. The report holds harness states, versions, offering
counts, causes and remedies; it never contains tokens or account data.

FOR-69: each harness row also reports its version basis (`verified` or
`compatible (probed <version>)`), and the check saves one listed model per
harness as a job selection in the disposable data directory and reads it back.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request

PRIVATE = ('CREWROOM_RUNTIME_CONFIG', 'CREWROOM_JIRA_TOKEN', 'CREWROOM_BITBUCKET_TOKEN')


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
    report = {'passed': False, 'scope': 'live metadata discovery; runner unconfigured; no inference'}
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
        report['health'] = request(info, 'health')
        before = request(info, 'workspace')['modelDiscovery']
        report['beforeRefresh'] = {'runner': before.get('runner'), 'selection': before.get('selection'),
                                   'harnesses': [{k: h.get(k) for k in ('harness', 'configured', 'remedy')}
                                                 for h in before['harnesses']]}
        request(info, 'model-catalog/refresh', {})
        deadline = time.monotonic() + 180
        latencies = []
        while True:
            started = time.monotonic()
            catalog = request(info, 'model-catalog')
            latencies.append(time.monotonic() - started)
            report['maxCatalogReadSeconds'] = round(max(latencies), 3)
            if catalog['state'] != 'refreshing':
                break
            if time.monotonic() > deadline:
                raise TimeoutError('Discovery did not finish')
            time.sleep(.5)
        report['state'] = catalog['state']
        report['harnesses'] = [{'harness': h['harness'], 'configured': h.get('configured'),
                                'outcome': h['outcome'], 'fresh': h.get('fresh'),
                                'version': (h.get('identity') or {}).get('version'),
                                'offerings': len(h.get('offerings', [])), 'error': h.get('error'),
                                'remedy': h.get('remedy'), 'note': h.get('note'),
                                'executionNote': h.get('executionNote'),
                                'compatibility': h.get('compatibility')}
                               for h in catalog['harnesses']]
        choices = catalog.get('choices', [])
        report['choices'] = {'total': len(choices),
                             'byHarness': {h: sum(1 for c in choices if c['key'].startswith(f'offering:{h}:'))
                                           for h in ('omp', 'codex', 'claude-code')},
                             'eligibleAtLow': sum(1 for c in choices if c['eligibility']['low']['eligible']),
                             'sampleKeys': [c['key'] for c in choices[:6]]}
        workspace = request(info, 'workspace')
        # Select one listed model per harness for a job, as the picker does.
        picked = {}
        for job, harness in zip(('code', 'judgment', 'review'), ('claude-code', 'codex', 'omp')):
            key = next((c['key'] for c in choices if c['key'].startswith(f'offering:{harness}:')), None)
            if key:
                picked[job] = key
        jobs = dict(workspace['modelJobs']['jobs'], **picked)
        saved = request(info, 'model-jobs', {'revision': workspace['modelJobs']['revision'],
                                             'budget': workspace['modelJobs']['budget'], 'jobs': jobs})
        workspace = request(info, 'workspace')
        report['selection'] = {'picked': picked, 'savedRevision': saved['revision'],
                               'readBack': {job: workspace['modelJobs']['jobs'][job] for job in picked}}
        report['runs'] = len(workspace['runs'])
        report['passed'] = (catalog['runner']['configured'] is False and catalog['selection'] == 'intent'
                            and report['runs'] == 0 and len(choices) > 0
                            and report['selection']['readBack'] == picked)
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
