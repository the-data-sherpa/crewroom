#!/usr/bin/env python3
"""Disposable matching-M7-build rollback using a stopped pre-native snapshot.

Scope: queued intake, accepted briefs, steering, role configuration and repository
metadata. No legacy executing process, Mail state or remote writes are invented.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import tarfile
import time
import traceback
import urllib.request
import uuid

PROJECT = Path(__file__).resolve().parent.parent
OLD_COMMIT = '965a563b6a42dcc5b4511927585644fe08f4d023'
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--binary-directory', type=Path, default=PROJECT / 'target/debug')
args = parser.parse_args()
root = args.output.resolve()
root.mkdir(parents=True, exist_ok=False, mode=0o700)
old = root / 'old-build'
old.mkdir()
native = root / 'native-build'
native.mkdir()
logs = root / 'logs'
logs.mkdir()
installation = root / 'installation'
installation.mkdir(mode=0o700)
data = installation / 'data'
data.mkdir(mode=0o700)
user = installation / 'user'
user.mkdir(mode=0o700)
roles = user / 'roles.json'
repository = installation / 'repository'
repository.mkdir()
build_home = root / 'build-home'
build_home.mkdir(mode=0o700)
work_tmp = root / 'tmp'
work_tmp.mkdir()
base_env = {'PATH': '/opt/homebrew/opt/node/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin', 'HOME': str(build_home), 'TMPDIR': str(work_tmp) + '/', 'LANG': 'en_US.UTF-8', 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_TERMINAL_PROMPT': '0'}
http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
processes = []
report = {'passed': False, 'legacyCommit': OLD_COMMIT, 'nativeSourceCommit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=PROJECT, text=True).strip(), 'scope': 'Disposable schema-26 queued intake, accepted briefs, steering, roles and repository. No executing legacy workflow, Mail or provider delivery.', 'commands': []}


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def inventory(path):
    values = {}
    for item in [path, *sorted(path.rglob('*'))]:
        mode = item.lstat().st_mode
        value = {'mode': stat.S_IMODE(mode)}
        if stat.S_ISLNK(mode):
            value.update(kind='symlink', target=os.readlink(item))
        elif stat.S_ISREG(mode):
            value.update(kind='file', sha256=digest(item), bytes=item.stat().st_size)
        elif stat.S_ISDIR(mode):
            value.update(kind='directory')
        else:
            raise RuntimeError(f'Unexpected live/special resource in stopped snapshot: {item}')
        values[str(item.relative_to(path))] = value
    return values


def command(label, argv, cwd=old):
    started = time.monotonic()
    path = logs / f'{label}.log'
    with path.open('w') as output:
        result = subprocess.run(argv, cwd=cwd, env={**base_env, 'npm_config_cache': str(root / 'npm-cache'), 'npm_config_audit': 'false', 'npm_config_fund': 'false'}, stdout=output, stderr=subprocess.STDOUT)
    report['commands'].append({'name': label, 'argv': argv, 'exitCode': result.returncode, 'seconds': round(time.monotonic() - started, 3), 'logSha256': digest(path)})
    assert result.returncode == 0, f'{label} failed; see {path}'


def identity(proc):
    if proc.poll() is not None:
        return None
    value = subprocess.check_output(['/bin/ps', '-p', str(proc.pid), '-o', 'uid=,lstart=,command='], text=True).strip()
    assert value.split()[0] == str(os.getuid())
    return value


def request(app, path, body=None):
    headers = {'Origin': app['origin']}
    if app.get('token'):
        headers['Authorization'] = 'Bearer ' + app['token']
    raw = None if body is None else json.dumps(body).encode()
    if raw is not None:
        headers['Content-Type'] = 'application/json'
    req = urllib.request.Request(app['origin'] + '/api' + path, data=raw, headers=headers)
    with http.open(req, timeout=10) as response:
        return json.load(response)


def start(kind, label):
    path = logs / f'{label}.log'
    handle = path.open('w')
    env = {**base_env, 'CREWROOM_DATA_DIR': str(data), 'CREWROOM_ROLE_PATH': str(roles), 'CREWROOM_PORT': '0', 'CREWROOM_JIRA_CONFIG': str(data / 'absent-jira.json'), 'CREWROOM_PUBLICATION_CONFIG': str(data / 'absent-bitbucket.json')}
    argv = ['node', '--import', 'tsx', 'server/index.ts', '--production'] if kind == 'legacy' else [str(native / 'crewroomd'), '--data-dir', str(data), '--roles', str(roles), '--port', '0']
    proc = subprocess.Popen(argv, cwd=old if kind == 'legacy' else root, env=env, stdout=handle, stderr=subprocess.STDOUT, start_new_session=True)
    app = {'kind': kind, 'label': label, 'proc': proc, 'handle': handle, 'identity': identity(proc)}
    processes.append(app)
    by = time.monotonic() + 30
    while time.monotonic() < by:
        assert proc.poll() is None, f'{label} exited: {path.read_text()}'
        if kind == 'legacy':
            match = re.search(r'Crewroom listening on (http://127\.0\.0\.1:\d+)', path.read_text())
            if match:
                app['origin'] = match.group(1)
        elif (data / 'native-connection.json').exists():
            value = json.loads((data / 'native-connection.json').read_text())
            if value['pid'] == proc.pid:
                app.update(origin=value['origin'], token=value['token'])
        if app.get('origin'):
            try:
                request(app, '/workspace')
                app['identity'] = identity(proc)
                return app
            except OSError:
                pass
        time.sleep(.025)
    raise TimeoutError(f'{label} readiness expired')


def stop(app):
    proc = app['proc']
    if proc.poll() is None:
        assert identity(proc) == app['identity'], 'Owned process identity changed; refusing signal.'
        proc.send_signal(signal.SIGTERM)
        proc.wait(timeout=30)
    assert proc.returncode == 0, f'{app["label"]} stopped unsuccessfully'
    app['handle'].close()
    if app['kind'] == 'native':
        assert not (data / 'native-connection.json').exists()
    app['stopped'] = True


def facts(app):
    value = request(app, '/workspace')
    assert value['activity'] == [], 'Queued fixture must never claim executing agents.'
    return {k: value[k] for k in ['tasks', 'roles', 'repositories', 'messages']} | {'runIds': [r['id'] for r in value['runs']], 'queueIds': [r['id'] for r in value['queue']['plans']]}


try:
    archive = root / 'legacy-source.tar'
    with archive.open('wb') as output:
        subprocess.run(['git', 'archive', '--format=tar', OLD_COMMIT], cwd=PROJECT, stdout=output, check=True)
    with tarfile.open(archive) as bundle:
        bundle.extractall(old, filter='data')
    source_files = {str(p.relative_to(old)): digest(p) for p in old.rglob('*') if p.is_file() and not p.is_symlink()}
    report['legacySourceArchiveSha256'] = digest(archive)
    command('npm-ci', ['npm', 'ci'])
    command('legacy-build', ['npm', 'run', 'build'])
    assert all(digest(old / p) == h for p, h in source_files.items()), 'Install/build changed legacy source.'
    for name in ['crewroomd', 'crewroom-host', 'crewroom-process']:
        shutil.copy2(args.binary_directory.resolve() / name, native / name)
    report['nativeBinarySha256'] = {p.name: digest(p) for p in native.iterdir()}
    old_build = inventory(old)
    (root / 'legacy-build-inventory.json').write_text(json.dumps(old_build, indent=2) + '\n')
    report['legacyBuildInventorySha256'] = digest(root / 'legacy-build-inventory.json')
    command('repository-init', ['git', '-C', str(repository), 'init', '-b', 'main'], cwd=root)
    (repository / 'README.md').write_text('Synthetic rollback repository; no remote configured.\n')
    (repository / 'AGENTS.md').write_text('Retain local queued work. No execution is configured.\n')
    command('repository-add', ['git', '-C', str(repository), 'add', '.'], cwd=root)
    command('repository-commit', ['git', '-C', str(repository), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'Rollback fixture'], cwd=root)
    (repository / 'read-only-link').symlink_to('README.md')
    (repository / 'untracked.txt').write_text('Preserve contents, mode and untracked ownership.\n')
    (repository / 'untracked.txt').chmod(0o640)
    (user / 'scope.json').write_text(json.dumps({'runtime': 'unavailable', 'providers': 'unconfigured', 'mail': 'not configured', 'scope': 'queued state only'}) + '\n')
    legacy = start('legacy', 'legacy-before')
    assert 'root' in http.open(legacy['origin'], timeout=10).read().decode()
    workspace = request(legacy, '/workspace')
    coordinator = next(r for r in workspace['roles'] if r['id'] == 'coordinator')
    request(legacy, '/roles/coordinator', {**coordinator, 'revision': coordinator.get('revision', 1), 'instructions': coordinator['instructions'] + '\nRetained rollback role.'})
    repo = request(legacy, '/repositories', {'name': 'Rollback repository', 'path': str(repository), 'targetBranch': 'main'})
    tasks = []
    runs = []
    for n in [1, 2]:
        task = request(legacy, '/tasks', {'title': f'Retained rollback task {n}', 'description': 'Local intake; no provider delivery.', 'repositoryId': repo['id']})
        task = request(legacy, f'/tasks/{task["id"]}/refinement', {'revision': task['revision'], 'sourceRevision': task['sourceRevision'], 'brief': f'Accepted original scope {n}', 'criteria': ['Preserve queued state and operator direction']})
        run = request(legacy, f'/tasks/{task["id"]}/runs', {'revision': task['revision']})
        request(legacy, f'/runs/{run["id"]}/steering', {'requestId': str(uuid.uuid4()), 'body': f'Original pending steering {n}'})
        tasks.append(task)
        runs.append(run)
    original = facts(legacy)
    assert len(original['tasks']) == len(original['queueIds']) == len(original['messages']) == 2
    assert all(m['delivery'] == 'pending' for m in original['messages'])
    stop(legacy)
    before = inventory(installation)
    backup = root / 'pre-native-snapshot'
    shutil.copytree(installation, backup, symlinks=True)
    assert inventory(backup) == before
    (root / 'pre-native-inventory.json').write_text(json.dumps(before, indent=2) + '\n')
    app = start('native', 'native-use')
    assert facts(app) == original, 'Native open changed retained task/role/queue/steering facts.'
    latest = original['tasks'][0]
    request(app, f'/tasks/{latest["id"]}/refinement', {'revision': latest['revision'], 'sourceRevision': latest['sourceRevision'], 'brief': 'New native scope retained only in newer snapshot', 'criteria': ['Native changes are preserved separately']})
    added = request(app, '/tasks', {'title': 'Native-only later task', 'description': 'Must not appear after restoring pre-native snapshot.'})
    request(app, f'/runs/{runs[0]["id"]}/steering', {'requestId': str(uuid.uuid4()), 'body': 'Native-only newer pending instruction'})
    role = next(r for r in facts(app)['roles'] if r['id'] == 'coordinator')
    request(app, '/roles/coordinator', {**role, 'revision': role['revision'], 'instructions': role['instructions'] + '\nNew native guidance.'})
    newer_facts = facts(app)
    assert len(newer_facts['tasks']) == 3 and len(newer_facts['messages']) == 3 and newer_facts != original
    stop(app)
    newer = inventory(installation)
    preserved = root / 'preserved-newer-native-state'
    os.rename(installation, preserved)
    assert inventory(preserved) == newer
    shutil.copytree(backup, installation, symlinks=True)
    assert inventory(installation) == before, 'Restore must be complete and at the original absolute paths.'
    assert inventory(old) == old_build, 'Matching old build changed before rollback.'
    restored = start('legacy', 'legacy-restored')
    assert facts(restored) == original, 'Matching old build did not recover the original facts.'
    task = request(restored, '/tasks', {'title': 'Old build works after rollback', 'description': 'New local intake after verified restore.'})
    assert task['id'] != added['id']
    final = facts(restored)
    assert len(final['tasks']) == 3 and len(final['messages']) == 2
    assert not any(t['title'] == 'Native-only later task' for t in final['tasks'])
    assert http.open(restored['origin'], timeout=10).status == 200
    stop(restored)
    assert inventory(preserved) == newer, 'Rollback modified the preserved newer state.'
    assert inventory(backup) == before, 'Rollback modified the original backup.'
    assert inventory(old) == old_build, 'Old build changed during re-launch.'
    (root / 'newer-native-inventory.json').write_text(json.dumps(newer, indent=2) + '\n')
    (root / 'facts.json').write_text(json.dumps({'original': original, 'newer': newer_facts, 'afterRollbackAndUse': final}, indent=2) + '\n')
    report.update(passed=True, snapshotEntries=len(before), preservedNewerEntries=len(newer), legacyBuildEntries=len(old_build), originalPath=str(installation), snapshotPath=str(backup), preservedNewerPath=str(preserved), preNativeInventorySha256=digest(root / 'pre-native-inventory.json'), newerInventorySha256=digest(root / 'newer-native-inventory.json'), factsSha256=digest(root / 'facts.json'), sameAbsolutePaths=True, contentsLinksModesVerified=True, matchingOldBuildRelaunched=True, olderBuildNeverOpenedNewerState=True, postRollbackNewTaskSaved=True)
except Exception:
    report['error'] = traceback.format_exc()
finally:
    for app in processes:
        if app['proc'].poll() is None:
            try:
                stop(app)
            except Exception:
                report['passed'] = False
                report.setdefault('cleanupErrors', []).append(traceback.format_exc())
    report['processes'] = [{'kind': a['kind'], 'label': a['label'], 'pid': a['proc'].pid, 'identity': a['identity'], 'exitCode': a['proc'].poll(), 'stopped': a.get('stopped', False), 'logSha256': digest(logs / f'{a["label"]}.log')} for a in processes]
    report['passed'] = report['passed'] and all(a['stopped'] and a['exitCode'] == 0 for a in report['processes'])
    (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
raise SystemExit(0 if report['passed'] else 1)
