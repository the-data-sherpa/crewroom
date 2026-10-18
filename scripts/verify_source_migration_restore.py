#!/usr/bin/env python3
"""Verify whole-state backup/restore across the provider-neutral ticket source migration (FOR-40).

A pre-FOR-40 daemon (schema 30, Jira-named `jiraStatus`/`jiraStatusId`) seeds a disposable
installation with Jira and local tickets, operator refinements, notes and requirement revisions
through its authenticated HTTP API. The stopped installation is backed up whole. The live copy is
then discarded, the backup is restored at the same path, and the current daemon opens it, which
migrates the store to the current schema. Every ticket, source field, refinement, note and
requirement revision must survive; only the documented rename (and the dropped local "Not
linked" placeholder) may differ. A later Jira import must still match the migrated tickets by
source identity. Finally the old daemon must refuse the migrated store, and restoring the same
backup must let it reopen the identical pre-migration facts (rollback).

Providers are simulated: Jira issues enter through the daemon's import operation, not a live
Jira site. No credentials are used. Evidence stays in the private --output directory.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import sqlite3
import stat
import subprocess
import time
import urllib.error
import urllib.request

SITE = 'https://fixture.atlassian.net'
HELPERS = ['crewroomd', 'crewroom-process', 'crewroom-host']
RENAMED = {'jiraStatus': 'sourceStatus', 'jiraStatusId': 'sourceStatusId'}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inventory(root):
    result = {}
    for path in sorted(root.rglob('*')):
        mode = path.lstat().st_mode
        if stat.S_ISREG(mode):
            result[str(path.relative_to(root))] = {'sha256': digest(path), 'mode': mode & 0o777}
    return result


def copy_state(source, destination):
    # A stopped Unix socket carries no durable authority and cannot be copied.
    def ephemeral(directory, names):
        return [n for n in names if not any(t((Path(directory) / n).lstat().st_mode)
                for t in (stat.S_ISREG, stat.S_ISDIR, stat.S_ISLNK))]
    shutil.copytree(source, destination, symlinks=True, ignore=ephemeral)


def issue(identifier, title, status, status_id, updated):
    return {'id': identifier, 'key': f'SIM-{identifier}', 'fields': {
        'summary': title, 'description': f'Synthetic description {identifier}',
        'status': {'id': status_id, 'name': status}, 'priority': {'name': 'High'},
        'project': {'name': 'Simulated'}, 'updated': updated}}


def schema(database):
    with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as db:
        return db.execute('PRAGMA user_version').fetchone()[0]


def logical(database):
    """Schema version plus a full SQL dump: equal stores hold equal facts regardless of WAL layout."""
    with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as db:
        dump = '\n'.join(db.iterdump())
        return db.execute('PRAGMA user_version').fetchone()[0], hashlib.sha256(dump.encode()).hexdigest()


# Opening a store rewrites only these: the controller fences ownership with a new epoch before
# it opens the database, and SQLite may checkpoint its write-ahead log on close.
VOLATILE = {'data/controller/epoch.json', 'data/workflow.sqlite', 'data/workflow.sqlite-wal', 'data/workflow.sqlite-shm'}


def ticket_rows(database):
    with sqlite3.connect(f'file:{database}?mode=ro', uri=True) as db:
        return {row[0]: {'imported': json.loads(row[1]), 'refinement': json.loads(row[2])}
                for row in db.execute('SELECT id, imported, refinement FROM tickets ORDER BY rowid')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='New private evidence directory')
    parser.add_argument('--legacy-binaries', type=Path, required=True,
                        help='Directory with crewroomd/crewroom-process/crewroom-host built at the pre-FOR-40 revision')
    parser.add_argument('--binaries', type=Path, default=Path('target/debug'),
                        help='Directory with the current build of the same executables')
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    project = Path(__file__).resolve().parent.parent
    state = root / 'installation'
    data = state / 'data'
    database = data / 'workflow.sqlite'
    state.mkdir(mode=0o700)
    roles = json.loads((project / 'crates/crewroom-core/src/default-roles.json').read_text())
    roles[0]['name'] = 'Migration fixture coordinator'
    (state / 'roles.json').write_text(json.dumps(roles))
    (state / 'roles.json').chmod(0o600)

    builds = {}
    for label, source in [('legacy', args.legacy_binaries), ('current', args.binaries)]:
        target = root / label
        target.mkdir()
        for name in HELPERS:
            shutil.copy2(source.resolve() / name, target / name)
        builds[label] = {name: digest(target / name) for name in HELPERS}
    report = {'passed': False, 'binarySha256': builds,
              'scope': ('Schema-30 whole state written by the pre-FOR-40 daemon; same-path restore into the '
                        'current daemon (migration to the current schema); rollback by restoring the backup '
                        'for the old daemon. Simulated Jira import; no live provider.')}
    processes = []
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(connection, path, body=None):
        headers = {'Origin': connection['origin'], 'Authorization': 'Bearer ' + connection['token']}
        raw = None if body is None else json.dumps(body).encode()
        if raw is not None:
            headers['Content-Type'] = 'application/json'
        req = urllib.request.Request(connection['origin'] + '/api' + path, data=raw, headers=headers)
        try:
            with http.open(req, timeout=20) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise AssertionError(f'{path}: {error.code} {error.read()[:300]!r}') from None

    def start(build, label):
        binary = root / build / 'crewroomd'
        require(digest(binary) == builds[build]['crewroomd'], f'{build} binary changed')
        log = (root / (label + '.log')).open('w')
        env = {k: v for k, v in os.environ.items() if not k.startswith('CREWROOM_')}
        proc = subprocess.Popen([str(binary), '--data-dir', str(data), '--roles', str(state / 'roles.json'),
                                 '--port', '0'], env=env, stdout=log, stderr=subprocess.STDOUT)
        processes.append((proc, log))
        until = time.monotonic() + 30
        while time.monotonic() < until:
            if proc.poll() is not None:
                return proc, None
            try:
                connection = json.loads((data / 'native-connection.json').read_text())
                if connection['pid'] == proc.pid:
                    request(connection, '/workspace')
                    return proc, connection
            except (OSError, ValueError, AssertionError):
                pass
            time.sleep(.05)
        raise TimeoutError(f'{label}: daemon readiness expired')

    def stop(proc):
        proc.send_signal(signal.SIGTERM)
        require(proc.wait(timeout=30) == 0, 'daemon did not stop cleanly')
        require(not (data / 'native-connection.json').exists(), 'connection descriptor retained after stop')

    def facts(connection):
        workspace = request(connection, '/workspace')
        tasks = [t for t in workspace['tasks'] if not t.get('internal')]
        revisions = {t['id']: request(connection, f"/tasks/{t['id']}/revisions") for t in tasks}
        return {'tasks': tasks, 'notes': workspace.get('notes'), 'revisions': revisions}

    def rename(task):
        return {RENAMED.get(k, k): v for k, v in task.items()}

    try:
        # 1. Pre-migration state written by the pre-FOR-40 daemon.
        proc, connection = start('legacy', 'legacy-seed')
        require(connection is not None, 'legacy daemon exited during seed')
        imported = request(connection, '/import/jira', {'siteUrl': SITE + '/', 'issues': [
            issue('100', 'Fix sum', 'In Progress', '3', '2026-10-01T00:00:00.000Z'),
            issue('101', 'Document totals', 'Backlog', '1', '2026-10-01T00:00:00.000Z')]})
        require(all('jiraStatus' in t and 'sourceStatus' not in t for t in imported),
                'legacy daemon must write Jira-named status fields')
        local = [request(connection, '/tasks', {'title': title, 'description': description})
                 for title, description in [('Local work', 'Operator-created ticket'), ('Unrefined local', '')]]
        require(all(t['jiraStatus'] == 'Not linked' for t in local), 'legacy local placeholder missing')
        for task, brief in [(imported[0], 'Handle zero'), (imported[1], 'List every total'), (local[0], 'Keep it local')]:
            request(connection, f"/tasks/{task['id']}/refinement", {
                'revision': task['revision'], 'sourceRevision': task['sourceRevision'],
                'brief': brief, 'criteria': [f'{brief} is verified']})
        request(connection, f"/tasks/{imported[0]['id']}/notes", {'mode': 'decision', 'body': 'Zero is a valid input.'})
        request(connection, f"/tasks/{local[0]['id']}/notes", {'mode': 'explore', 'body': 'No provider for this ticket.'})
        before = facts(connection)
        stop(proc)
        require(schema(database) == 30, f'legacy store is schema {schema(database)}, expected 30')
        legacy_rows = ticket_rows(database)
        require(len(before['tasks']) == 4 and len(legacy_rows) == 4, 'seed did not produce four tickets')

        # 2. Whole-state backup of the stopped installation.
        artifact = data / 'restore-marker.txt'
        artifact.write_text('Retained alongside the workflow database.\n')
        backup = root / 'whole-state-backup'
        copy_state(state, backup)
        backup_inventory = inventory(backup)

        # 3. Discard the live state, restore the backup at the same path, open with the current build.
        shutil.rmtree(state)
        copy_state(backup, state)
        require(inventory(state) == backup_inventory, 'restored bytes or permissions differ from the backup')
        proc, connection = start('current', 'current-restored')
        require(connection is not None, 'current daemon refused the restored schema-30 state')
        after = facts(connection)
        checks = ['legacy daemon wrote schema 30 with jiraStatus/jiraStatusId and local "Not linked"',
                  'whole-state bytes and permissions restored at the original path']

        require([t['id'] for t in after['tasks']] == [t['id'] for t in before['tasks']], 'ticket identity or order changed')
        for old, new in zip(before['tasks'], after['tasks']):
            require('jiraStatus' not in new and 'jiraStatusId' not in new, f"{new['key']} kept a Jira-named field")
            expected = rename(old)
            if old['source'] != 'jira':
                require(old['jiraStatus'] == 'Not linked', 'unexpected local status')
                expected.pop('sourceStatus')
                require('sourceStatus' not in new, f"{new['key']}: local ticket gained a source status")
            for key, value in expected.items():
                require(new.get(key) == value, f"{old['key']}.{key}: {value!r} became {new.get(key)!r}")
        checks.append('every projected ticket field equal after the rename; local placeholder dropped')
        require(after['notes'] == before['notes'], 'notes changed')
        require(after['revisions'] == before['revisions'], 'requirement revisions changed')
        checks.append('notes and requirement revisions identical')
        stop(proc)
        require(schema(database) == report.setdefault('migratedSchema', schema(database)) and schema(database) > 30,
                'store was not migrated')
        migrated_rows = ticket_rows(database)
        for task_id, old in legacy_rows.items():
            new = migrated_rows[task_id]
            require(new['refinement'] == old['refinement'], f'{task_id}: stored refinement changed')
            expected = old['imported'] if old['imported'].get('source') == 'jira' else {
                k: v for k, v in old['imported'].items() if k != 'jiraStatus'}
            require(list(new['imported']) == [RENAMED.get(k, k) for k in expected], f'{task_id}: field order changed')
            require(new['imported'] == rename(expected), f'{task_id}: stored source fields changed')
        checks.append('stored imported JSON renamed in place in key order; stored refinements byte-for-byte equal')
        require(artifact.read_text() == 'Retained alongside the workflow database.\n', 'non-database artifact lost')
        checks.append('non-database artifact restored')

        # 4. Source identity still matches after migration: status moves alone, requirement change re-accepts.
        proc, connection = start('current', 'current-reimport')
        moved = request(connection, '/import/jira', {'siteUrl': SITE + '/', 'issues': [
            issue('100', 'Fix sum', 'Done', '4', '2026-10-02T00:00:00.000Z'),
            issue('101', 'Document all totals', 'Backlog', '1', '2026-10-02T00:00:00.000Z')]})
        require([t['id'] for t in moved] == [t['id'] for t in before['tasks'][:2]], 'Jira import created duplicates')
        status_only, requirement = moved
        require(status_only['sourceStatus'] == 'Done' and status_only['sourceStatusId'] == '4', 'status not refreshed')
        require(status_only['sourceRevision'] == before['tasks'][0]['sourceRevision']
                and status_only['needsReacceptance'] is False, 'a status change forced re-acceptance')
        require(requirement['needsReacceptance'] is True
                and requirement['sourceRevision'] == before['tasks'][1]['sourceRevision'] + 1,
                'a requirement change did not require re-acceptance')
        require(status_only['brief'] == 'Handle zero' and requirement['brief'] == 'List every total',
                'refinements lost on re-import')
        require(len([t for t in request(connection, '/workspace')['tasks'] if not t.get('internal')]) == 4,
                'ticket count changed')
        checks.append('post-migration Jira import matches migrated tickets by source identity; status-only change keeps acceptance, requirement change re-accepts, refinements kept')
        stop(proc)

        # 5. Rollback: the old daemon refuses the migrated store; restoring the backup lets it reopen.
        migrated_inventory = inventory(state)
        migrated_logical = logical(database)
        proc, connection = start('legacy', 'legacy-refuses-migrated')
        require(connection is None and proc.wait(timeout=30) != 0, 'legacy daemon opened a migrated store')
        refused_inventory = inventory(state)
        changed = sorted(k for k in migrated_inventory.keys() | refused_inventory.keys()
                         if migrated_inventory.get(k) != refused_inventory.get(k))
        report['changedByRefusedOpen'] = changed
        require(set(changed) <= VOLATILE, f'refused open changed durable files: {changed}')
        require(logical(database) == migrated_logical, 'refused open changed the migrated database contents')
        checks.append('pre-FOR-40 daemon refuses the migrated store; database contents (schema + full SQL dump) unchanged')
        preserved = root / 'migrated-state-preserved'
        shutil.move(str(state), preserved)
        copy_state(backup, state)
        require(inventory(state) == backup_inventory, 'second restore differs from the backup')
        proc, connection = start('legacy', 'legacy-rollback')
        require(connection is not None, 'legacy daemon refused its own backup')
        require(facts(connection) == before, 'rollback facts differ from the pre-migration facts')
        stop(proc)
        require(schema(database) == 30, 'rollback state is no longer schema 30')
        checks.append('restoring the backup lets the pre-FOR-40 daemon reopen identical facts at schema 30')
        report.update(passed=True, restoredFileCount=len(backup_inventory), checks=checks,
                      tickets=[{'key': t['key'], 'source': t['source'], 'legacyStatus': t['jiraStatus'],
                                'migratedStatus': n.get('sourceStatus'), 'brief': n['brief']}
                               for t, n in zip(before['tasks'], after['tasks'])],
                      backupInventory=backup_inventory)
    finally:
        for proc, log in processes:
            if proc.poll() is None:
                proc.terminate()
                proc.wait(timeout=30)
            log.close()
        report['cleanup'] = {'allOwnedProcessesStopped': all(p.poll() is not None for p, _ in processes),
                             'processCount': len(processes)}
        (root / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({k: v for k, v in report.items() if k != 'backupInventory'}, indent=2))


if __name__ == '__main__':
    main()
