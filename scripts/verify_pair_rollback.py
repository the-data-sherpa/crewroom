#!/usr/bin/env python3
"""Restore a stopped successful MH6 disposable job at its original paths.

Copies its matching binaries, whole workbench, worktrees, Mail store and exact
assignment-owned Claude transcript directories. Never opens an older binary or
changes the active installation. Private credentials remain in private backups.
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
import uuid


def require(condition, message):
    if not condition:
        raise RuntimeError(str(message))


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inventory(root):
    result = {}
    for path in sorted(root.rglob('*')):
        mode = path.lstat().st_mode
        if stat.S_ISREG(mode):
            result[str(path.relative_to(root))] = {'sha256': digest(path), 'mode': mode & 0o777}
        elif stat.S_ISLNK(mode):
            result[str(path.relative_to(root))] = {'symlink': str(path.readlink())}
    return result


def copy_state(source, destination):
    # Stopped Unix sockets cannot be restored and carry no durable authority.
    def ephemeral(directory, names):
        return [name for name in names if not any(test((Path(directory) / name).lstat().st_mode)
                for test in (stat.S_ISREG, stat.S_ISDIR, stat.S_ISLNK))]
    shutil.copytree(source, destination, symlinks=True, ignore=ephemeral)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seed', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    seed, output = args.seed.resolve(), args.output.resolve()
    require(seed.parent == Path('/tmp') and seed.name.startswith('cr-mh6-'), "Restore check failed: seed.parent == Path('/tmp') and seed.name.startswith('cr-mh6-')")
    require(output != seed and not output.is_relative_to(seed), 'Restore check failed: output != seed and (not output.is_relative_to(seed))')
    acceptance_path = seed / 'pair-result-validated.json'
    if not acceptance_path.exists():
        acceptance_path = seed / 'pair-result.json'
    acceptance = json.loads(acceptance_path.read_text())
    require(acceptance['passed'] and not acceptance.get('cleanupError'), "Restore check failed: acceptance['passed'] and (not acceptance.get('cleanupError'))")
    state = Path(acceptance['fixtureRoot']).resolve()
    require(state.parent == seed and not (state / 'data/native-connection.json').exists(), "Restore check failed: state.parent == seed and (not (state / 'data/native-connection.json').exists())")
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    os.umask(0o077)
    binary_root = Path(acceptance.get('reviewRecovery', str(seed))).resolve()
    require(binary_root.is_relative_to(seed), 'Matching build must belong to the disposable seed')
    binaries = json.loads((binary_root / 'binaries.json').read_text())
    for name, expected in binaries.items():
        require(digest(binary_root / 'bin' / name) == expected, "Restore check failed: digest(binary_root / 'bin' / name) == expected")
    helper = binary_root / 'bin/crewroom-process'
    for path in state.glob('runtime/*/*/owner.json'):
        result = json.loads(subprocess.check_output([str(helper), 'inspect', str(path.parent)]))
        require(result['state'] == 'stopped', path.parent)
    config = json.loads((seed / 'settings.json').read_text())
    proof = json.loads((seed / 'mail-cleanup.json').read_text())
    require(proof.get('verifiedStopped') is True and proof.get('processGroupAbsent') is True,
            'Positive Mail ownership cleanup proof is required')
    try:
        os.killpg(proof['pid'], 0)
    except ProcessLookupError:
        pass
    else:
        raise RuntimeError('Mail process group exists; restore is blocked')
    mail = urllib.parse.urlparse(config['mail']['url'])
    with socket.socket() as probe:
        require(probe.connect_ex((mail.hostname, mail.port)) != 0, 'Stop the owned Mail server first')
    db = sqlite3.connect(state / 'data/workflow.sqlite')
    require(db.execute('PRAGMA user_version').fetchone()[0] == 29, "Restore check failed: db.execute('PRAGMA user_version').fetchone()[0] == 29")
    attempts = [json.loads(row[0]) for row in db.execute('SELECT data FROM attempts')]
    require(attempts and all(a['ownership'] == 'stopped' for a in attempts), "Restore check failed: attempts and all((a['ownership'] == 'stopped' for a in attempts))")
    run_id = acceptance['candidate']['runId']
    external = sorted({Path(a['sessionDirectory']).resolve() for a in attempts
                       if a.get('runnerKind') == 'claude-code'})
    for path in external:
        require(path.parent == Path(config['claude']['nativeHome']).resolve() / '.claude/projects', "Restore check failed: path.parent == Path(config['claude']['nativeHome']).resolve() / '.claude/projects'")
        require(str(state).replace('/', '-') in path.name, "Restore check failed: str(state).replace('/', '-') in path.name")
        require(path.is_dir() and not path.is_symlink(), 'Restore check failed: path.is_dir() and (not path.is_symlink())')
    db.close()
    report = {'passed': False, 'seed': str(seed), 'schema': 29, 'binaries': binaries,
              'scope': 'Stopped whole state and matching build at original paths; no older-binary compatibility claim',
              'externalSessionDirectories': [str(p) for p in external]}
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    owned = []

    def request(connection, path, body=None):
        headers = {'Origin': connection['origin'], 'Authorization': 'Bearer ' + connection['token']}
        if body is not None:
            headers['Content-Type'] = 'application/json'
        req = urllib.request.Request(connection['origin'] + '/api' + path,
                                     data=None if body is None else json.dumps(body).encode(), headers=headers)
        with http.open(req, timeout=20) as response:
            return json.load(response)

    def start(label):
        log = (output / (label + '.log')).open('w')
        env = {k: v for k, v in os.environ.items() if not k.startswith('CREWROOM_')}
        proc = subprocess.Popen([str(binary_root / 'bin/crewroomd'), '--data-dir', str(state / 'data'),
                                 '--roles', str(state / 'roles.json'), '--port', '0'],
                                env=env, stdout=log, stderr=subprocess.STDOUT)
        owned.append((proc, log))
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            require(proc.poll() is None, label + ' exited')
            try:
                connection = json.loads((state / 'data/native-connection.json').read_text())
                if connection['pid'] == proc.pid:
                    request(connection, '/workspace')
                    return proc, connection
            except (OSError, ValueError):
                pass
            time.sleep(.05)
        raise TimeoutError(label)

    def stop(proc):
        proc.terminate()
        require(proc.wait(timeout=30) == 0, 'Restore check failed: proc.wait(timeout=30) == 0')
        require(not (state / 'data/native-connection.json').exists(), "Restore check failed: not (state / 'data/native-connection.json').exists()")

    def facts(connection):
        workspace = request(connection, '/workspace')
        runtime = request(connection, '/runs/' + run_id + '/runtime')
        coding = request(connection, '/runs/' + run_id + '/coding')
        return {'workspace': {k: workspace[k] for k in ('tasks', 'roles', 'repositories', 'messages', 'intake')},
                'runtime': {k: runtime[k] for k in ('run', 'assignments', 'attempts', 'choreography', 'messages')},
                'coding': coding['record']}

    try:
        proc, connection = start('before')
        before = facts(connection)
        stop(proc)
        backup = output / 'whole-state'
        copy_state(seed, backup)
        expected = inventory(backup)
        session_inventories = {}
        for index, path in enumerate(external):
            target = output / ('claude-' + str(index))
            copy_state(path, target)
            session_inventories[str(path)] = inventory(target)
        proc, connection = start('mutated')
        request(connection, '/runs/' + run_id + '/steering',
                {'requestId': str(uuid.uuid4()), 'body': 'Disposable restore probe after backup.'})
        require(facts(connection) != before, 'Restore check failed: facts(connection) != before')
        stop(proc)
        # Exercise non-SQLite restore: user roles and a retained evidence artifact.
        (state / 'roles.json').write_text('[]\n')
        artifacts = list((state / 'data').rglob('*.log'))
        marker = artifacts[0] if artifacts else state / 'data/restore-probe'
        marker.write_text('Changed after backup.\n')
        for path in external:
            (path / 'restore-probe').write_text('Changed after backup.\n')
        copy_state(seed, output / 'changed-state')
        shutil.rmtree(seed)
        copy_state(backup, seed)
        require(inventory(seed) == expected, 'Restore check failed: inventory(seed) == expected')
        for index, path in enumerate(external):
            shutil.rmtree(path)
            copy_state(output / ('claude-' + str(index)), path)
            require(inventory(path) == session_inventories[str(path)], 'Restore check failed: inventory(path) == session_inventories[str(path)]')
        proc, connection = start('restored')
        require(facts(connection) == before, 'Restore check failed: facts(connection) == before')
        repository = before['workspace']['repositories'][0]
        require(Path(repository['path']).is_relative_to(state), "Restore check failed: Path(repository['path']).is_relative_to(state)")
        subprocess.run(['git', '-C', repository['path'], 'rev-parse', '--verify', 'HEAD'],
                       check=True, stdout=subprocess.DEVNULL)
        stop(proc)
        report.update(passed=True, restoredFiles=len(expected), inventorySha256=hashlib.sha256(
            json.dumps(expected, sort_keys=True).encode()).hexdigest(),
            checks=['all owned processes and Mail stopped', 'schema 29',
                    'mutation changed service projection', 'whole-state bytes and modes restored',
                    'exact assignment Claude histories restored', 'matching binaries reopened successfully',
                    'user roles, choreography, exchanges, ownership, paths and evidence identical'])
        (output / 'inventory.json').write_text(json.dumps(expected, indent=2) + '\n')
    finally:
        for proc, log in owned:
            if proc.poll() is None:
                proc.terminate()
                proc.wait(timeout=30)
            log.close()
        report['cleanup'] = {'allOwnedProcessesStopped': all(p.poll() is not None for p, _ in owned)}
        (output / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
