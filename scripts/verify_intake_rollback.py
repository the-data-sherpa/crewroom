#!/usr/bin/env python3
"""Verify stopped whole-state restore with its matching MH5 binary on a disposable seed.

The script creates a private seed at its eventual installation path. It proves restoring the
matching schema-28 state/build; it does not claim an older binary can read schema 28.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time
import urllib.request


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inventory(root):
    return {str(p.relative_to(root)): {"sha256": digest(p), "mode": p.stat().st_mode & 0o777}
            for p in sorted(root.rglob('*')) if p.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary', type=Path, default=Path('target/debug/crewroomd'))
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    state = root / 'installation'
    project = Path(__file__).resolve().parent.parent
    with (root / 'seed.log').open('w') as log:
        subprocess.run(['cargo', 'test', '--locked', '-p', 'crewroom-desktop', '-j6', 'seed_intake_visual_review', '--', '--ignored', '--test-threads=1'], cwd=project, env={**os.environ, 'CREWROOM_INTAKE_DEMO_DIR': str(state)}, stdout=log, stderr=subprocess.STDOUT, check=True)
    roles = json.loads((project / 'crates/crewroom-core/src/default-roles.json').read_text())
    roles[0]['name'] = 'Rollback fixture coordinator'
    (state / 'roles.json').write_text(json.dumps(roles))
    (state / 'roles.json').chmod(0o600)
    binary = root / 'crewroomd'
    shutil.copy2(args.binary, binary)
    binary_hash = digest(binary)
    helper_hashes = {}
    for name in ['crewroom-process', 'crewroom-host']:
        shutil.copy2(args.binary.parent / name, root / name)
        helper_hashes[name] = digest(root / name)
    report = {"passed": False, "binarySha256": binary_hash, "seed": str(state),
              "helperSha256": helper_hashes,
              "scope": "Whole schema-28 state plus user roles, artifacts and repository metadata; matching binary restore; no older-schema compatibility claim"}
    processes = []
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(connection, path, body=None):
        headers = {'Origin': connection['origin'], 'Authorization': 'Bearer ' + connection['token']}
        raw = None if body is None else json.dumps(body).encode()
        if raw is not None:
            headers['Content-Type'] = 'application/json'
        req = urllib.request.Request(connection['origin'] + '/api' + path, data=raw, headers=headers)
        with http.open(req, timeout=10) as response:
            return json.load(response)

    def start(label):
        assert digest(binary) == binary_hash
        log = (root / (label + '.log')).open('w')
        env = {k: v for k, v in os.environ.items() if not k.startswith('CREWROOM_')}
        proc = subprocess.Popen([str(binary), '--data-dir', str(state / 'data'), '--roles', str(state / 'roles.json'), '--port', '0'], env=env, stdout=log, stderr=subprocess.STDOUT)
        processes.append((proc, log))
        until = time.monotonic() + 20
        while time.monotonic() < until:
            assert proc.poll() is None, f'{label} exited'
            try:
                connection = json.loads((state / 'data/native-connection.json').read_text())
                if connection['pid'] == proc.pid:
                    request(connection, '/workspace')
                    return proc, connection
            except (OSError, ValueError):
                pass
            time.sleep(.05)
        raise TimeoutError('Daemon readiness expired')

    def stop(proc):
        proc.send_signal(signal.SIGTERM)
        assert proc.wait(timeout=20) == 0
        assert not (state / 'data/native-connection.json').exists()

    def facts(workspace):
        return {k: workspace[k] for k in ['tasks', 'roles', 'repositories', 'messages', 'intake']}

    try:
        proc, connection = start('before-backup')
        before = facts(request(connection, '/workspace'))
        stop(proc)
        # An artifact makes the check fail if restoration only copies SQLite.
        artifact = state / 'data/rollback-evidence.txt'
        artifact.write_text('Retained alongside the workflow database.\n')
        backup = root / 'whole-state-backup'
        shutil.copytree(state, backup)
        backup_inventory = inventory(backup)
        proc, connection = start('after-backup')
        task = next(t for t in before['tasks'] if before['intake'][t['id']]['currentPlan'])
        request(connection, '/tasks/' + task['id'] + '/refinement', {
            'revision': task['revision'], 'sourceRevision': task['sourceRevision'],
            'brief': task['brief'] + ' Changed after backup.', 'criteria': task['criteria']})
        changed = facts(request(connection, '/workspace'))
        assert changed != before
        assert changed['intake'][task['id']]['currentPlan'] is None
        stop(proc)
        artifact.unlink()
        shutil.rmtree(state)
        shutil.copytree(backup, state)
        assert inventory(state) == backup_inventory
        proc, connection = start('restored-matching-binary')
        assert facts(request(connection, '/workspace')) == before
        restored_task = next(t for t in before['tasks'] if before['intake'][t['id']]['currentPlan'])
        source = request(connection, '/tasks/' + restored_task['id'] + '/plan-source', {'revision': restored_task['revision']})
        assert source['path'] == str(state / 'checkout')
        assert request(connection, '/workspace')['intake'][restored_task['id']]['currentPlan'] is not None
        assert artifact.read_text() == 'Retained alongside the workflow database.\n'
        stop(proc)
        report.update(passed=True, restoredFileCount=len(backup_inventory),
                      backupInventory=backup_inventory, checks=['requirements mutation invalidated plan',
                      'whole-state bytes and permissions restored', 'matching binary accepted restored state',
                      'tasks, plans, decisions, questions, answers, messages and roles identical', 'non-database artifact restored', 'restored checkout revalidated at its original installation path'])
    finally:
        for proc, log in processes:
            if proc.poll() is None:
                proc.terminate()
                proc.wait(timeout=20)
            log.close()
        report['cleanup'] = {'allOwnedProcessesStopped': all(p.poll() is not None for p, _ in processes)}
        (root / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({k: v for k, v in report.items() if k != 'backupInventory'}))


if __name__ == '__main__':
    main()
