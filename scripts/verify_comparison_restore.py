#!/usr/bin/env python3
"""Restore a stopped FOR-14 disposable state with its matching schema-30 build.

The check reopens history without a runtime configuration, so it never starts
inference or reconnects Mail. It restores the original paths, including the
isolated binary directory and external private runtime directory when present.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import stat
import subprocess
import time
import urllib.parse
import urllib.request

from verify_pair_rollback import copy_state, digest, inventory, require


PROJECT = Path(__file__).resolve().parents[1]
DISPOSABLE = PROJECT / '.crewroom/test-results/for14'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seed', type=Path, required=True)
    parser.add_argument('--binary-dir', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    seed, output = args.seed.resolve(), args.output.resolve()
    require((seed.parent == Path('/tmp') and seed.name.startswith('cr-wf-'))
            or seed.is_relative_to(DISPOSABLE), 'Use an isolated FOR-14 fixture only')
    require(not output.exists() and not output.is_relative_to(seed), 'Choose a new output outside the seed')
    require(not (seed / 'data/native-connection.json').exists(), 'Stop the seed daemon first')
    live = (seed / 'comparison-result.json').exists()
    proof_path = seed / ('comparison-result.json' if live else 'feature-cleanup.json')
    proof = json.loads(proof_path.read_text())
    require(proof.get('passed') is True, 'The seed must have a passing retained receipt')
    settings = json.loads((seed / 'settings.json').read_text()) if live else {}
    runtime = Path(settings.get('runtimeRoot', seed / 'runtime')).resolve()
    require(runtime.is_relative_to(seed) or (runtime.parent == Path('/tmp') and runtime.name.startswith('cr-job-')),
            'Runtime must be this fixture private directory')
    binary_root = (args.binary_dir or seed / 'bin').resolve()
    require(binary_root.is_relative_to(seed), 'Mutation binaries must be dedicated copies inside this seed')
    for name in ('crewroomd', 'crewroom-host', 'crewroom-process'):
        binary = binary_root / name
        require(not binary.is_symlink() and stat.S_ISREG(binary.stat().st_mode) and binary.stat().st_nlink == 1,
                'Mutation binaries must be regular, unshared copies: ' + str(binary))
    if live:
        require(proof['cleanup']['passed'], 'Daemon and worker cleanup must pass')
        mail = json.loads((seed / 'mail-cleanup.json').read_text())
        require(mail.get('verifiedStopped') and mail.get('processGroupAbsent'), 'Positive Mail cleanup is required')
        try:
            os.killpg(mail['pid'], 0)
        except ProcessLookupError:
            pass
        else:
            raise RuntimeError('The owned Mail process group still exists')
        endpoint = urllib.parse.urlparse(settings['mail']['url'])
        with socket.socket() as probe:
            require(probe.connect_ex((endpoint.hostname, endpoint.port)) != 0, 'The owned Mail endpoint remains open')
    else:
        require(not proof.get('fallback') and not proof.get('errors') and proof.get('daemonExitCode') == 0,
                'Scripted fixture must have verified normal cleanup')
    binaries = {name: digest(binary_root / name) for name in ('crewroomd', 'crewroom-host', 'crewroom-process')}
    db = sqlite3.connect(seed / 'data/workflow.sqlite')
    require(db.execute('PRAGMA user_version').fetchone()[0] == 30, 'Expected schema 30')
    comparisons = [json.loads(row[0]) for row in db.execute('SELECT data FROM comparisons')]
    manifests = [json.loads(row[0]) for row in db.execute('SELECT manifest FROM comparisons')]
    for manifest in manifests:
        application = manifest['execution']['application']
        identities = {'crewroomd': application, 'crewroom-host': application['helpers']['host'],
                      'crewroom-process': application['helpers']['process']}
        require(all(identity['sha256'] == binaries[name] and Path(identity['path']).resolve() == binary_root / name
                    for name, identity in identities.items()), 'Build bytes and paths must match the immutable manifests')
    cells = [json.loads(row[0]) for row in db.execute('SELECT data FROM comparison_cells')]
    attempts = [json.loads(row[0]) for row in db.execute('SELECT data FROM attempts')]
    require(comparisons and all(c['desiredState'] != 'running' for c in comparisons), 'Stop every comparison first')
    require(any(c.get('result', {}).get('observed') for c in cells), 'Require an actual stopped worker observation')
    require(attempts and all(a.get('ownership') == 'stopped' or a['state'] == 'admitted' for a in attempts), 'Every started attempt must be stopped')
    rows = {table: db.execute('SELECT * FROM ' + table + ' ORDER BY rowid').fetchall()
            for table in ('comparisons', 'comparison_cells', 'comparison_events', 'comparison_requests')}
    run_ids = [c['runId'] for c in cells if 'runId' in c]
    db.close()
    roots = [seed]
    for path in (runtime, binary_root):
        if not any(path.is_relative_to(root) for root in roots):
            roots.append(path)
    require(not any(output.is_relative_to(root) for root in roots), 'Backup output must be outside every restored root')
    for attempt in attempts:
        session = attempt.get('sessionDirectory')
        require(not session or any(Path(session).resolve().is_relative_to(root) for root in roots),
                'An external provider transcript requires a separately scoped restore recipe')
    for owner in runtime.rglob('owner.json'):
        result = json.loads(subprocess.check_output([str(binary_root / 'crewroom-process'), 'inspect', str(owner.parent)]))
        require(result['state'] == 'stopped', 'Retained owner is not stopped: ' + str(owner.parent))
    sources = sorted((runtime / 'cases').glob('*/workspace/*.cjs'))
    artifacts = sorted(p for p in (seed / 'data/artifacts').iterdir() if p.is_file())
    require(sources and artifacts, 'Keep the actual evaluation source and artifact bytes')
    output.mkdir(parents=True, mode=0o700)
    os.umask(0o077)
    report = {'passed': False, 'schema': 30, 'seed': str(seed), 'mode': 'live' if live else 'scripted',
              'binaries': binaries, 'comparisonIds': [c['id'] for c in comparisons],
              'scope': 'Stopped complete state and matching build at original paths; no older binary compatibility or inference quality claim'}
    processes = []
    expected = {}
    backups_ready = False
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(connection, route, body=None):
        headers = {'Origin': connection['origin'], 'Authorization': 'Bearer ' + connection['token']}
        if body is not None:
            headers['Content-Type'] = 'application/json'
        req = urllib.request.Request(connection['origin'] + '/api' + route, headers=headers,
                                     data=None if body is None else json.dumps(body).encode())
        with http.open(req, timeout=20) as response:
            return json.load(response)

    def start(label):
        log = (output / (label + '.log')).open('w')
        env = {key: value for key, value in os.environ.items() if not key.startswith('CREWROOM_')}
        proc = subprocess.Popen([str(binary_root / 'crewroomd'), '--data-dir', str(seed / 'data'),
                                 '--roles', str(seed / 'roles.json'), '--port', '0'], env=env,
                                stdout=log, stderr=subprocess.STDOUT)
        processes.append((proc, log))
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            require(proc.poll() is None, label + ' daemon exited')
            try:
                connection = json.loads((seed / 'data/native-connection.json').read_text())
                if connection['pid'] == proc.pid:
                    require(request(connection, '/health')['executionAvailable'] is False, 'History restore must not enable execution')
                    return proc, connection
            except (OSError, ValueError):
                pass
            time.sleep(.05)
        raise TimeoutError(label)

    def stop(proc):
        proc.terminate()
        require(proc.wait(timeout=30) == 0, 'Daemon shutdown failed')
        require(not (seed / 'data/native-connection.json').exists(), 'Connection authority remains after stop')

    def facts(connection):
        workspace = request(connection, '/workspace')
        return {'workspace': {key: workspace[key] for key in ('tasks', 'roles', 'repositories', 'messages', 'intake')},
                'comparisons': [request(connection, '/comparisons/' + c['id']) for c in comparisons],
                'runtime': [{key: value for key, value in request(connection, '/runs/' + run + '/runtime').items()
                             if key in ('run', 'assignments', 'attempts', 'messages', 'history')} for run in run_ids]}

    try:
        for index, root in enumerate(roots):
            backup = output / ('whole-state-' + str(index))
            copy_state(root, backup)
            expected[str(root)] = inventory(backup)
        backups_ready = True
        proc, connection = start('before')
        before = facts(connection)
        stop(proc)
        proc, connection = start('mutated')
        request(connection, '/tasks', {'title': 'Disposable comparison restore probe', 'description': 'Added only after backup.'})
        require(facts(connection) != before, 'Public mutation must change the service projection')
        stop(proc)
        (seed / 'roles.json').write_text('[]\n')
        sources[0].write_text('Changed source after backup.\n')
        artifacts[0].write_text('Changed artifact after backup.\n')
        # The isolated copied build is stopped and dedicated to this seed.
        host_copy = binary_root / 'crewroom-host'
        host_copy.chmod(host_copy.stat().st_mode | stat.S_IWUSR)
        host_copy.write_bytes(b'Changed isolated binary after backup.\n')
        for index, root in enumerate(roots):
            copy_state(root, output / ('changed-state-' + str(index)))
            shutil.rmtree(root)
            copy_state(output / ('whole-state-' + str(index)), root)
            require(inventory(root) == expected[str(root)], 'Whole-state bytes or modes changed: ' + str(root))
        require(all(digest(binary_root / name) == sha for name, sha in binaries.items()), 'Matching binary bytes were not restored')
        proc, connection = start('restored')
        require(facts(connection) == before, 'Restored service projections differ')
        stop(proc)
        db = sqlite3.connect(seed / 'data/workflow.sqlite')
        require(all(db.execute('SELECT * FROM ' + table + ' ORDER BY rowid').fetchall() == values
                    for table, values in rows.items()), 'Comparison ledgers changed across restore')
        db.close()
        report.update(passed=True, restoredFiles=sum(len(values) for values in expected.values()),
                      inventorySha256=hashlib.sha256(json.dumps(expected, sort_keys=True).encode()).hexdigest(),
                      checks=['all actual process owners stopped', 'schema 30', 'mutated public state differs',
                              'all original bytes, modes and paths restored', 'matching binaries restored and reopened',
                              'manifest, cells, events, result receipts, roles, source, artifacts and stopped attempts identical'])
        (output / 'inventory.json').write_text(json.dumps(expected, indent=2) + '\n')
    finally:
        cleanup_errors = []
        for proc, log in processes:
            try:
                if proc.poll() is None:
                    proc.terminate()
                    proc.wait(timeout=30)
            except (OSError, subprocess.TimeoutExpired) as error:
                cleanup_errors.append(str(error))
            log.close()
        report['cleanup'] = {'allOwnedDaemonsStopped': all(p.poll() is not None for p, _ in processes)}
        if backups_ready and report['cleanup']['allOwnedDaemonsStopped']:
            try:
                for index, root in enumerate(roots):
                    backup = output / ('whole-state-' + str(index))
                    require(inventory(backup) == expected[str(root)], 'Original backup changed: ' + str(root))
                    if root.exists():
                        if not report['passed']:
                            copy_state(root, output / ('failed-state-' + str(index)))
                        shutil.rmtree(root)
                    copy_state(backup, root)
                    require(inventory(root) == expected[str(root)], 'Final restoration differs: ' + str(root))
                report['cleanup']['originalBytesModesAndPathsRestored'] = True
            except (OSError, RuntimeError) as error:
                cleanup_errors.append(str(error))
        elif backups_ready:
            cleanup_errors.append('Owned daemon remains alive; original backups retained without replacing live state')
        report['cleanup']['errors'] = cleanup_errors
        if cleanup_errors:
            report['passed'] = False
        (output / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
        require(not cleanup_errors, '; '.join(cleanup_errors))
    print(json.dumps(report))


if __name__ == '__main__':
    main()
